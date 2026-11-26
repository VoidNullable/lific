//! Explicit, authenticated imports of remote images. DNS is checked once and
//! pinned to the connection; proxies and redirects cannot bypass that check.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::{Extension, Json, extract::State};
use futures_util::future::BoxFuture;

use super::attachments::{
    AttachmentConfig, AttachmentUploadLimiter, NewAttachment, UploadResponse, authorize_link,
    sanitize_filename, store_upload,
};
use crate::db::{DbPool, models::AttachmentEntity};
use crate::error::LificError;
use crate::realtime::RealtimeHub;
use crate::resolve_caller::ResolvedIdentity;
use crate::storage::{self, AttachmentStore};

const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(serde::Deserialize)]
pub(super) struct ImportBody {
    url: String,
    entity_type: Option<AttachmentEntity>,
    entity_id: Option<i64>,
}

pub(super) async fn import_attachment(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<ResolvedIdentity>>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(store): Extension<AttachmentStore>,
    Extension(config): Extension<AttachmentConfig>,
    Extension(limiter): Extension<Arc<AttachmentUploadLimiter>>,
    Json(body): Json<ImportBody>,
) -> Result<Json<UploadResponse>, LificError> {
    import_from(
        &HttpSource,
        &db,
        &identity,
        &realtime,
        &store,
        &config,
        &limiter,
        body,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn import_from(
    source: &dyn ImageSource,
    db: &DbPool,
    identity: &Option<ResolvedIdentity>,
    realtime: &RealtimeHub,
    store: &AttachmentStore,
    config: &AttachmentConfig,
    limiter: &AttachmentUploadLimiter,
    body: ImportBody,
) -> Result<Json<UploadResponse>, LificError> {
    let user = super::require_user(identity)?;
    if !limiter.0.check(&format!("user:{}", user.id)) {
        return Err(LificError::Forbidden(
            "upload rate limit exceeded; try again shortly".into(),
        ));
    }
    let link = match (body.entity_type, body.entity_id) {
        (Some(entity), Some(id)) => {
            authorize_link(db, identity, entity, id)?;
            Some((entity, id))
        }
        (None, None) => None,
        _ => {
            return Err(LificError::BadRequest(
                "entity_type and entity_id must be provided together".into(),
            ));
        }
    };
    let image = load(source, &body.url, config.max_bytes)
        .await
        .map_err(LificError::BadRequest)?;
    let (attachment, event) = store_upload(
        db,
        store,
        identity,
        &NewAttachment {
            filename: &image.filename,
            mime: &image.mime,
            bytes: &image.bytes,
            uploader: user.id,
            alt_text: None,
            link,
        },
    )?;
    if let Some(event) = event {
        realtime.send(event);
    }
    Ok(Json(attachment.into()))
}

type ImageBody = (Vec<u8>, Option<String>);

trait ImageSource: Send + Sync {
    fn resolve(&self, host: String, port: u16) -> BoxFuture<'_, Result<Vec<IpAddr>, String>>;
    fn fetch(
        &self,
        url: reqwest::Url,
        address: SocketAddr,
        max: usize,
    ) -> BoxFuture<'_, Result<ImageBody, String>>;
}

struct HttpSource;

impl ImageSource for HttpSource {
    fn resolve(&self, host: String, port: u16) -> BoxFuture<'_, Result<Vec<IpAddr>, String>> {
        Box::pin(async move {
            tokio::net::lookup_host((host.as_str(), port))
                .await
                .map(|addresses| addresses.map(|address| address.ip()).collect())
                .map_err(|_| "image host could not be resolved".into())
        })
    }

    fn fetch(
        &self,
        url: reqwest::Url,
        address: SocketAddr,
        max: usize,
    ) -> BoxFuture<'_, Result<ImageBody, String>> {
        Box::pin(fetch_image(url, address, max, FETCH_TIMEOUT))
    }
}

async fn fetch_image(
    url: reqwest::Url,
    address: SocketAddr,
    max: usize,
    timeout: Duration,
) -> Result<(Vec<u8>, Option<String>), String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .resolve(url.host_str().unwrap_or_default(), address)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .connect_timeout(timeout)
        .build()
        .map_err(|_| "could not prepare image fetch")?;
    let mut response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "image/*")
        .send()
        .await
        .map_err(|_| "image fetch failed or timed out")?;
    if response.status().is_redirection() {
        return Err("image redirects are not allowed; paste the final image URL".into());
    }
    if response.status() != reqwest::StatusCode::OK {
        return Err(format!("image host answered HTTP {}", response.status()));
    }
    if response
        .content_length()
        .is_some_and(|size| size > max as u64)
    {
        return Err(format!("file too large: more than {max} bytes"));
    }
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "image read failed or timed out")?
    {
        if bytes.len().saturating_add(chunk.len()) > max {
            return Err(format!("file too large: more than {max} bytes"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok((bytes, mime))
}

#[derive(Debug)]
struct ImportedImage {
    filename: String,
    mime: String,
    bytes: Vec<u8>,
}

async fn load(source: &dyn ImageSource, raw: &str, max: usize) -> Result<ImportedImage, String> {
    tokio::time::timeout(FETCH_TIMEOUT, load_inner(source, raw, max))
        .await
        .map_err(|_| "image import timed out".to_owned())?
}

async fn load_inner(
    source: &dyn ImageSource,
    raw: &str,
    max: usize,
) -> Result<ImportedImage, String> {
    if raw.len() > 8192 {
        return Err("image URL is too long".into());
    }
    let url = reqwest::Url::parse(raw).map_err(|_| "invalid image URL")?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("image URL must use http or https and have a host".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("image URL must not contain credentials".into());
    }
    let host = url.host_str().unwrap_or_default();
    let port = url.port_or_known_default().unwrap_or(443);
    let addresses = match host.trim_matches(['[', ']']).parse::<IpAddr>() {
        Ok(ip) => vec![ip],
        Err(_) => source.resolve(host.to_owned(), port).await?,
    };
    if addresses.is_empty() || addresses.iter().any(|ip| !public_address(*ip)) {
        return Err("image host must resolve only to public addresses".into());
    }
    let filename = sanitize_filename(
        &urlencoding::decode(
            url.path_segments()
                .and_then(|mut parts| parts.next_back())
                .unwrap_or("image"),
        )
        .unwrap_or_default(),
    );
    let (bytes, declared) = source
        .fetch(url, SocketAddr::new(addresses[0], port), max)
        .await?;
    if bytes.len() > max {
        return Err(format!("file too large: more than {max} bytes"));
    }
    let mime =
        storage::sniff_and_validate(&bytes, declared.as_deref()).map_err(|e| e.to_string())?;
    if !mime.starts_with("image/") || !storage::ALLOWED_MIMES.contains(&mime.as_str()) {
        return Err("URL did not return an allowed image".into());
    }
    if storage::is_raster_mime(&mime) && storage::image_dimensions(&bytes).is_none() {
        return Err("URL did not return a readable image header".into());
    }
    if mime == "image/svg+xml" && !has_svg_root(&bytes) {
        return Err("URL did not return an SVG image".into());
    }
    Ok(ImportedImage {
        filename,
        mime,
        bytes,
    })
}

fn has_svg_root(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let mut text = text.trim_start_matches('\u{feff}').trim_start();
    loop {
        let end = if text.starts_with("<?xml") {
            "?>"
        } else if text.starts_with("<!--") {
            "-->"
        } else {
            break;
        };
        let Some((_, rest)) = text.split_once(end) else {
            return false;
        };
        text = rest.trim_start();
    }
    text.strip_prefix("<svg").is_some_and(|rest| {
        rest.starts_with(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
    })
}

#[cfg(test)]
mod tests;

fn public_address(ip: IpAddr) -> bool {
    if !crate::oauth::cimd::is_public_address(ip) {
        return false;
    }
    // Only native global unicast IPv6, excluding IETF assignments (including
    // Teredo). Mapped IPv4 has already gone through the shared IPv4 guard.
    match ip {
        IpAddr::V6(v6) if v6.to_ipv4_mapped().is_none() => {
            let segments = v6.segments();
            segments[0] & 0xe000 == 0x2000 && !(segments[0] == 0x2001 && segments[1] < 0x0200)
        }
        _ => true,
    }
}
