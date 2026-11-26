//! LIF-583: reading a published project without an account.
//!
//! A published project is readable at its ordinary routes. A request that
//! presents no credential at all reaches the same handlers a signed-in reader
//! does, with no identity, and [`crate::authz::require_read`] lets it read
//! published projects and nothing else. Every write still needs an account
//! with a write role: the auth middleware answers a credential-less request
//! for anything outside [`is_anonymous_read`] with 401, as it always has.
//!
//! What this module adds on top of the handlers is what an anonymous caller
//! must not be able to do to the instance:
//!
//! * a per-IP rate limit and an instance-wide concurrency budget, so strangers
//!   cannot crowd signed-in users out of the database;
//! * separate budgets for streaming downloads and for thumbnail and preview
//!   generation, plus a size ceiling on what those may be generated from;
//! * one answer for "not readable": every 403 and 404 becomes the same 404, so
//!   a private project, a missing one and anything inside either look alike;
//! * no caching and the usual browser hardening on every response, because a
//!   published project can be unpublished and a cached copy would outlive that.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    RequestExt,
    body::Body,
    extract::ConnectInfo,
    http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode, header},
    middleware::Next,
    response::IntoResponse,
};
use tokio::io::AsyncReadExt;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::db::models::{Attachment, AuthUser};
use crate::error::LificError;
use crate::ratelimit::{self, IpNetwork, RateLimiter};
use crate::resolve_caller::ResolvedIdentity;
use crate::storage::{self, AttachmentStore};

/// The single answer for "nothing readable here".
const NOT_FOUND: &str = "not found";

/// Anonymous reads allowed per client IP per minute. A person opening the
/// list, an issue, its comments and a body of images stays well under it.
pub(crate) const READS_PER_MINUTE: usize = 240;

/// Anonymous requests that may be in flight at once, across every visitor.
///
/// Below the read-pool size on purpose, so anonymous traffic cannot crowd the
/// signed-in instance out of the database.
pub(crate) const CONCURRENCY: usize = 4;

/// How long an anonymous request waits for one of the [`CONCURRENCY`] slots
/// before it is refused with a 503. Opening a page fires a burst of reads at
/// once; a short wait absorbs the burst without letting a queue grow.
const CONCURRENCY_WAIT: Duration = Duration::from_secs(1);

/// Anonymous attachment downloads that may stream at once. Downloads use their
/// own budget because a slow reader can hold one for minutes, while ordinary
/// reads need the database budget only until their response is built.
pub(crate) const DOWNLOAD_CONCURRENCY: usize = 4;

/// Thumbnail and preview generations allowed at once for anonymous callers. A
/// maximum-size raster can decode to roughly 200 MB before output buffers, so
/// one at a time keeps a 512 MB deployment within its memory budget.
pub(crate) const DERIVE_CONCURRENCY: usize = 1;

/// Largest attachment an anonymous thumbnail or preview is generated from. A
/// thumbnail already on disk is served whatever the original's size.
pub(crate) const DERIVE_MAX_BYTES: i64 = 32 * 1024 * 1024;

/// Bytes read from disk per chunk when streaming an attachment.
const DOWNLOAD_CHUNK_BYTES: usize = 64 * 1024;

/// A stalled anonymous download is ended after this long without progress.
const DOWNLOAD_IDLE_TIMEOUT: Duration = Duration::from_secs(15);

/// An anonymous download is ended after this long whatever its progress.
const DOWNLOAD_MAX_DURATION: Duration = Duration::from_secs(5 * 60);

/// The anonymous budgets, shared by every anonymous request on the instance.
pub struct AnonymousReads {
    limiter: RateLimiter,
    concurrency: Arc<Semaphore>,
    downloads: Arc<Semaphore>,
    derivations: Arc<Semaphore>,
    trusted_proxies: Arc<[IpNetwork]>,
}

impl AnonymousReads {
    /// The production budgets. `trusted_proxies` is the list the rest of the
    /// instance rate-limits with: behind a proxy every request arrives from one
    /// address, and in front of one a client-supplied `X-Forwarded-For` would
    /// mint a fresh bucket per request.
    pub fn new(trusted_proxies: Arc<[IpNetwork]>) -> Self {
        Self::with_limits(
            trusted_proxies,
            READS_PER_MINUTE,
            CONCURRENCY,
            DOWNLOAD_CONCURRENCY,
            DERIVE_CONCURRENCY,
        )
    }

    pub(crate) fn with_limits(
        trusted_proxies: Arc<[IpNetwork]>,
        reads_per_minute: usize,
        concurrency: usize,
        downloads: usize,
        derivations: usize,
    ) -> Self {
        Self {
            limiter: RateLimiter::new(reads_per_minute, Duration::from_secs(60)),
            concurrency: Arc::new(Semaphore::new(concurrency)),
            downloads: Arc::new(Semaphore::new(downloads)),
            derivations: Arc::new(Semaphore::new(derivations)),
            trusted_proxies,
        }
    }

    #[cfg(test)]
    pub(crate) fn concurrency(&self) -> &Arc<Semaphore> {
        &self.concurrency
    }

    #[cfg(test)]
    pub(crate) fn downloads(&self) -> &Arc<Semaphore> {
        &self.downloads
    }

    #[cfg(test)]
    pub(crate) fn derivations(&self) -> &Arc<Semaphore> {
        &self.derivations
    }

    /// Charge one request from `peer` against the per-IP rate limit.
    ///
    /// The key is the client IP as [`ratelimit::client_ip`] resolves it, which
    /// believes a forwarding header only from a trusted proxy. A request with
    /// no peer shares one bucket, so the failure direction is "shared limit",
    /// not "no limit".
    pub(crate) fn admit(
        &self,
        peer: Option<SocketAddr>,
        headers: &HeaderMap,
    ) -> Result<(), LificError> {
        let client = match peer {
            Some(peer) => ratelimit::client_ip(peer.ip(), headers, &self.trusted_proxies)
                .map_err(|_| LificError::Unavailable("invalid proxy identity".into()))?,
            None => "unknown".into(),
        };
        if self.limiter.check(&format!("anonymous:{client}")) {
            Ok(())
        } else {
            Err(LificError::TooManyRequests(
                "too many requests; slow down and try again shortly".into(),
            ))
        }
    }
}

// ── Which requests may be anonymous ─────────────────────────────

/// The routes an anonymous caller may `GET`. `{n}` matches a numeric id,
/// `{s}` any one segment. Every one is a read whose handler gates on
/// [`crate::authz::require_read`] or filters by
/// [`crate::authz::readable_project_ids`]. Member rosters, saved views,
/// mention candidates, exports and every workspace or account view are left
/// out, so they keep answering 401.
const ANONYMOUS_READS: &[&str] = &[
    "/api/projects",
    "/api/projects/{n}",
    "/api/projects/{n}/index",
    "/api/projects/{n}/changes",
    "/api/projects/{n}/board",
    "/api/projects/{n}/issue-counts",
    "/api/projects/{n}/relations",
    "/api/projects/{n}/activity",
    "/api/projects/{n}/activity/actors",
    "/api/projects/{n}/insights",
    "/api/projects/{n}/my-role",
    "/api/projects/{n}/attachments",
    "/api/issues",
    "/api/issues/{n}",
    "/api/issues/resolve/{s}",
    "/api/issues/{n}/comments",
    "/api/issues/{n}/activity",
    "/api/issues/{n}/waits",
    "/api/pages",
    "/api/pages/{n}",
    "/api/pages/resolve/{s}",
    "/api/pages/{n}/comments",
    "/api/pages/{n}/activity",
    "/api/plans",
    "/api/plans/{n}",
    "/api/plans/resolve/{s}",
    "/api/plans/{n}/activity",
    "/api/modules",
    "/api/modules/{n}",
    "/api/labels",
    "/api/folders",
    "/api/attachments",
    "/api/attachments/{n}",
    "/api/attachments/{n}/thumbnail",
    "/api/attachments/{n}/preview",
    "/api/attachments/{n}/links",
    "/api/search",
    "/api/clock",
];

/// The route patterns above, for the coverage test that checks each one is a
/// read gate.
#[cfg(test)]
pub(crate) fn anonymous_read_patterns() -> &'static [&'static str] {
    ANONYMOUS_READS
}

/// Whether a request with no credential may go ahead as an anonymous read.
pub(crate) fn is_anonymous_read(method: &Method, path: &str) -> bool {
    method == Method::GET
        && ANONYMOUS_READS
            .iter()
            .any(|pattern| path_matches(pattern, path))
}

fn path_matches(pattern: &str, path: &str) -> bool {
    let mut want = pattern.split('/');
    let mut have = path.split('/');
    loop {
        match (want.next(), have.next()) {
            (None, None) => return true,
            (Some("{n}"), Some(segment)) => {
                if segment.is_empty() || !segment.bytes().all(|b| b.is_ascii_digit()) {
                    return false;
                }
            }
            (Some("{s}"), Some(segment)) => {
                if segment.is_empty() {
                    return false;
                }
            }
            (Some(expected), Some(segment)) if expected == segment => {}
            _ => return false,
        }
    }
}

// ── Serving an anonymous read ───────────────────────────────────

/// Run an anonymous read: charge the budgets, attach "no identity", run the
/// handler, then make a refusal indistinguishable from absence and stamp the
/// response with the anonymous headers.
///
/// Called by the auth middleware for a credential-less request that
/// [`is_anonymous_read`] admits; the 429 and 503 refusals leave through the
/// same header stamping as everything else.
pub(crate) async fn serve(
    reads: Arc<AnonymousReads>,
    mut request: Request<Body>,
    next: Next,
) -> Response<Body> {
    // Through the extractor, not `extensions().get()`: a test router supplies
    // the peer with `MockConnectInfo`, which stores itself rather than a
    // `ConnectInfo`, and only the extractor looks for both.
    let peer = request
        .extract_parts::<ConnectInfo<SocketAddr>>()
        .await
        .ok()
        .map(|ConnectInfo(addr)| addr);

    let response = match reads.admit(peer, request.headers()) {
        Err(refusal) => refusal.into_response(),
        Ok(()) => {
            let permit = match acquire_read_slot(&reads).await {
                Ok(permit) => permit,
                Err(refusal) => return harden(refusal.into_response()),
            };
            request
                .extensions_mut()
                .insert(Option::<ResolvedIdentity>::None);
            request.extensions_mut().insert(Option::<AuthUser>::None);
            let actor = crate::actor::ActorCtx {
                user_id: None,
                transport: crate::actor::Transport::Web,
            };
            let response = crate::actor::scope(actor, next.run(request)).await;
            drop(permit);
            conceal_refusal(response)
        }
    };
    harden(response)
}

async fn acquire_read_slot(reads: &AnonymousReads) -> Result<OwnedSemaphorePermit, LificError> {
    let busy = || LificError::Unavailable("too many anonymous readers; try again shortly".into());
    if let Ok(permit) = Arc::clone(&reads.concurrency).try_acquire_owned() {
        return Ok(permit);
    }
    match tokio::time::timeout(
        CONCURRENCY_WAIT,
        Arc::clone(&reads.concurrency).acquire_owned(),
    )
    .await
    {
        Ok(Ok(permit)) => Ok(permit),
        _ => Err(busy()),
    }
}

/// Every 403 and 404 an anonymous reader receives is this one 404, so the
/// answer never says whether something exists but is private.
fn conceal_refusal(response: Response<Body>) -> Response<Body> {
    match response.status() {
        StatusCode::FORBIDDEN | StatusCode::NOT_FOUND => {
            LificError::NotFound(NOT_FOUND.into()).into_response()
        }
        _ => response,
    }
}

/// Caching off and browser hardening on, for every anonymous response.
///
/// `insert` overwrites: an attachment download would otherwise keep an
/// `immutable` year-long cache for a file that can be unpublished tomorrow.
fn harden(mut response: Response<Body>) -> Response<Body> {
    let headers = response.headers_mut();
    for (name, value) in [
        (header::CACHE_CONTROL, "no-store, no-cache, must-revalidate"),
        (header::PRAGMA, "no-cache"),
        (header::EXPIRES, "0"),
        // A body can carry links a reader clicks; the destination must not
        // learn which instance, let alone which issue, they came from.
        (header::REFERRER_POLICY, "no-referrer"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::X_FRAME_OPTIONS, "DENY"),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    headers.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
    response
}

// ── Attachments for anonymous readers ───────────────────────────

/// Stream an attachment to an anonymous reader.
///
/// Streamed rather than buffered, because an imported attachment can be
/// hundreds of megabytes and these callers are anonymous and concurrent. The
/// producer holds a download permit until it finishes, the reader stops, or a
/// timeout ends it, including when the client stops polling the body. Anything
/// that is not a plain raster or media file downloads rather than opening, and
/// SVG is served as `application/octet-stream`.
pub(crate) fn download(
    reads: &AnonymousReads,
    store: &AttachmentStore,
    attachment: &Attachment,
) -> Result<Response<Body>, LificError> {
    // The store lock is held only to open the handle. Streaming under it would
    // block a dump or restore for as long as a reader takes.
    let (file, len) = store
        .try_with_lock(|store| store.open_blob(&attachment.sha256))?
        .ok_or_else(AttachmentStore::busy_error)?;

    let inline_safe = storage::is_inline_safe_mime(&attachment.mime);
    let content_type = if attachment.mime == "image/svg+xml"
        || !storage::ALLOWED_MIMES.contains(&attachment.mime.as_str())
    {
        "application/octet-stream"
    } else {
        &attachment.mime
    };
    let disposition = format!(
        "{}; filename=\"{}\"",
        if inline_safe { "inline" } else { "attachment" },
        header_safe(&attachment.filename)
    );

    let permit = Arc::clone(&reads.downloads)
        .try_acquire_owned()
        .map_err(|_| LificError::Unavailable("anonymous downloads are busy".into()))?;
    let body = download_body(file, permit, DOWNLOAD_IDLE_TIMEOUT, DOWNLOAD_MAX_DURATION);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, len)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; sandbox",
        )
        .header(header::CONTENT_DISPOSITION, disposition)
        .body(body)
        .map_err(|e| LificError::Internal(format!("build response: {e}")))
}

/// Whether an anonymous reader may have a thumbnail or preview generated from
/// `attachment` now. Returns the permit the generation must hold.
pub(crate) fn derivation_permit(
    reads: &AnonymousReads,
    attachment: &Attachment,
) -> Result<OwnedSemaphorePermit, LificError> {
    if attachment.size_bytes > DERIVE_MAX_BYTES {
        return Err(LificError::NotFound(NOT_FOUND.into()));
    }
    Arc::clone(&reads.derivations)
        .try_acquire_owned()
        .map_err(|_| LificError::Unavailable("media processing is busy".into()))
}

/// Run blocking derivation work off the async runtime, holding `permit` until
/// the work stops, even if the request that asked for it has gone away.
pub(crate) async fn run_derivation<T, F>(
    permit: OwnedSemaphorePermit,
    job: F,
) -> Result<T, LificError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, LificError> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        job()
    })
    .await
    .map_err(|error| LificError::Internal(format!("media task failed: {error}")))?
}

fn download_body(
    file: std::fs::File,
    permit: OwnedSemaphorePermit,
    idle_timeout: Duration,
    max_duration: Duration,
) -> Body {
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    let (terminal_sender, terminal_receiver) = tokio::sync::oneshot::channel();
    // A producer deadline runs even when the client stops polling the body.
    tokio::spawn(async move {
        let _permit = permit;
        let mut file = tokio::fs::File::from_std(file);
        let mut buffer = vec![0; DOWNLOAD_CHUNK_BYTES];
        let result = tokio::time::timeout(max_duration, async {
            loop {
                let read = file.read(&mut buffer).await?;
                if read == 0 {
                    return Ok(());
                }
                let chunk = axum::body::Bytes::copy_from_slice(&buffer[..read]);
                match tokio::time::timeout(idle_timeout, sender.send(chunk)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) => return Ok(()),
                    Err(_) => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "anonymous download idle timeout",
                        ));
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "anonymous download deadline exceeded",
            ))
        });
        let _ = terminal_sender.send(result);
    });
    super::export::stream_body(receiver, terminal_receiver)
}

/// Strip anything that could break out of a quoted `Content-Disposition`
/// filename, including control characters an imported filename can carry.
fn header_safe(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control() && !matches!(c, '"' | '\\'))
        .take(200)
        .collect()
}

#[cfg(test)]
mod tests;
