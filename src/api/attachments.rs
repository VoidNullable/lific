//! LIF-262: attachment upload / download / delete endpoints.
//!
//! Storage is content-addressed on disk (`crate::storage::AttachmentStore`);
//! this module owns the HTTP surface and the authorization gates. The
//! `AttachmentStore` and `AttachmentConfig` are injected as axum `Extension`s
//! (wired in `main.rs`), mirroring how `AuthConfig` reaches handlers.
//!
//! Authorization model (project-scoped, LIF-196/197):
//! - **Upload** requires any authenticated user (attachments are owned by
//!   their uploader and only become project-visible once linked into an
//!   issue/page/comment). A per-user rate limit caps abuse. LIF-405: the
//!   optional `entity_type`/`entity_id` form fields, which link the new blob
//!   into an entity in the same request, are gated separately by
//!   [`authorize_link`] — that part *is* a mutation of someone else's issue,
//!   page or comment.
//! - **Download** requires `Viewer` on the owning project when
//!   `authz_enforced` is on. An unlinked attachment (not yet referenced
//!   anywhere) is readable only by its uploader / an admin — there's no
//!   project to gate on yet.
//! - **Delete** requires the uploader, or `Maintainer` on any owning project,
//!   or an admin.

use axum::{
    Extension,
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use std::sync::Arc;

use crate::authz;
use crate::db::models::*;
use crate::db::queries::attachments as q;
use crate::db::{DbPool, queries};
use crate::error::LificError;
use crate::ratelimit::RateLimiter;
use crate::realtime::{RealtimeEvent, RealtimeHub};
use crate::storage::{self, AttachmentStore};

use super::{require_user, with_read, with_write};

/// Runtime config for the upload endpoint. Injected as an `Extension` so it can
/// be tuned per instance without threading through every call. `max_bytes`
/// defaults to 10 MB (see `main.rs`); the global 2 MB body limit is raised for
/// the upload route specifically to this value.
#[derive(Debug, Clone)]
pub struct AttachmentConfig {
    pub max_bytes: usize,
}

impl Default for AttachmentConfig {
    fn default() -> Self {
        Self {
            max_bytes: 10 * 1024 * 1024,
        }
    }
}

/// The upload success payload: enough for the composer to insert a markdown
/// reference and render a chip without a second round trip.
#[derive(Debug, serde::Serialize)]
pub struct UploadResponse {
    pub id: i64,
    pub url: String,
    pub filename: String,
    pub mime: String,
    pub size: i64,
    /// LIF-418: decoded raster dimensions, so the composer can write a
    /// correctly-sized placeholder before the image loads.
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub alt_text: Option<String>,
    pub has_thumbnail: bool,
}

/// `POST /api/attachments` (multipart). Reads the first file part, validates
/// size + MIME (magic-byte sniffed, never trusting the client header), stores
/// the bytes content-addressed, and records the metadata row. Optional form
/// field `entity_type` + `entity_id` immediately links the new attachment
/// (used by the "attach to this issue's section" flow); otherwise it stays
/// unlinked until the entity's markdown is saved and re-scanned.
pub(super) async fn upload_attachment(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(store): Extension<AttachmentStore>,
    Extension(config): Extension<AttachmentConfig>,
    Extension(limiter): Extension<Arc<AttachmentUploadLimiter>>,
    mut multipart: Multipart,
) -> Result<Response, LificError> {
    let user = require_user(&identity)?;

    // Per-user rate limit (mirrors the signup/login limiter pattern).
    if !limiter.0.check(&format!("user:{}", user.id)) {
        return Err(LificError::Forbidden(
            "upload rate limit exceeded — try again shortly".into(),
        ));
    }

    let mut file_bytes: Option<Vec<u8>> = None;
    let mut filename = "upload".to_string();
    let mut declared_mime: Option<String> = None;
    let mut link_entity: Option<AttachmentEntity> = None;
    let mut link_entity_id: Option<i64> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| LificError::BadRequest(format!("malformed multipart: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                if let Some(fname) = field.file_name() {
                    filename = sanitize_filename(fname);
                }
                declared_mime = field.content_type().map(|s| s.to_string());
                let data = field
                    .bytes()
                    .await
                    .map_err(|e| LificError::BadRequest(format!("failed to read upload: {e}")))?;
                if data.len() > config.max_bytes {
                    return Err(LificError::BadRequest(format!(
                        "file too large: {} bytes (max {})",
                        data.len(),
                        config.max_bytes
                    )));
                }
                file_bytes = Some(data.to_vec());
            }
            "entity_type" => {
                let v = field.text().await.unwrap_or_default();
                link_entity = v.parse().ok();
            }
            "entity_id" => {
                let v = field.text().await.unwrap_or_default();
                link_entity_id = v.trim().parse().ok();
            }
            _ => {
                // Drain and ignore unknown fields.
                let _ = field.bytes().await;
            }
        }
    }

    // LIF-405: authorize the requested link target BEFORE any bytes hit the
    // store or any row is written, so a rejected link leaves nothing behind.
    if let (Some(entity), Some(entity_id)) = (link_entity, link_entity_id) {
        authorize_link(&db, &identity, entity, entity_id)?;
    }

    let bytes =
        file_bytes.ok_or_else(|| LificError::BadRequest("no 'file' field in upload".into()))?;
    if bytes.is_empty() {
        return Err(LificError::BadRequest("empty file".into()));
    }

    // Validate the content type from magic bytes (allowlist), never trusting
    // the client-declared header alone.
    let mime = storage::sniff_and_validate(&bytes, declared_mime.as_deref())?;
    if !storage::ALLOWED_MIMES.contains(&mime.as_str()) {
        return Err(LificError::BadRequest(format!(
            "rejected: '{mime}' is not an allowed file type"
        )));
    }

    let size = bytes.len() as i64;
    // Store bytes first (content-addressed), then record metadata.
    let sha = store.write(&bytes)?;

    // LIF-418: decode dimensions and pre-generate a thumbnail for rasters.
    // Both are best-effort: a picture the `image` crate cannot read is still a
    // perfectly good attachment, and refusing the upload over a missing
    // derivative would be a regression against every format it already
    // accepted.
    let dimensions = if storage::is_raster_mime(&mime) {
        storage::image_dimensions(&bytes)
    } else {
        None
    };
    if dimensions.is_some() {
        cache_thumbnail(&store, &sha, &bytes);
    }

    let (attachment, event) = with_write(&db, |conn| {
        let mut att = q::create_attachment(conn, &sha, &filename, &mime, size, Some(user.id))?;
        if let Some((w, h)) = dimensions {
            q::set_dimensions(conn, att.id, i64::from(w), i64::from(h))?;
            att = q::get_attachment(conn, att.id)?;
        }
        // LIF-418: index a small text upload's contents so search can find the
        // file by what's inside it, not just by its name. Migration 042's
        // insert trigger has already put the filename in `attachments_fts`;
        // this fills in the text column. Non-UTF-8 bytes can't reach here for
        // a `text/*` mime (the sniffer requires valid UTF-8), but the check
        // keeps this honest if the allowlist ever widens.
        if q::is_extractable(&mime, size)
            && let Ok(text) = std::str::from_utf8(&bytes)
        {
            q::set_extracted_text(conn, att.id, text)?;
        }
        // If the caller asked to link immediately, do it here in the same txn.
        let event = if let (Some(entity), Some(eid)) = (link_entity, link_entity_id) {
            q::link_attachment(conn, att.id, entity, eid)?;
            linked_entity_event(conn, entity, eid)?
        } else {
            None
        };
        Ok((att, event))
    })?;
    if let Some(event) = event {
        realtime.send(event);
    }

    let resp = UploadResponse {
        id: attachment.id,
        url: format!("/api/attachments/{}", attachment.id),
        filename: attachment.filename,
        mime: attachment.mime,
        size: attachment.size_bytes,
        width: attachment.width,
        height: attachment.height,
        alt_text: attachment.alt_text,
        has_thumbnail: attachment.has_thumbnail,
    };
    Ok((StatusCode::OK, axum::Json(resp)).into_response())
}

/// Query params for `GET /api/attachments?entity_type=&entity_id=` — lists the
/// attachments linked to one entity (the detail-view "Attachments (n)"
/// section).
#[derive(Debug, serde::Deserialize)]
pub(super) struct ListForEntityQuery {
    entity_type: String,
    entity_id: i64,
}

/// `GET /api/attachments?entity_type=issue&entity_id=42` — the attachments
/// linked to an entity. Gated at Viewer on the entity's project (same as
/// reading the entity itself).
pub(super) async fn list_entity_attachments(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Query(query): Query<ListForEntityQuery>,
) -> Result<axum::Json<Vec<Attachment>>, LificError> {
    let entity: AttachmentEntity = query.entity_type.parse().map_err(LificError::BadRequest)?;

    // The entity's owning project gates the read (Viewer). Workspace-level
    // pages (no project) fall back to workspace-admin.
    let project_id = resolve_entity_project(&db, entity, query.entity_id)?;
    match project_id {
        Some(pid) => authz::require_role(&db, &identity, pid, Role::Viewer)?,
        None => authz::require_workspace_admin(&db, &identity)?,
    }

    let items = with_read(&db, |conn| {
        q::list_for_entity(conn, entity, query.entity_id)
    })?;
    Ok(axum::Json(items))
}

/// Resolve the project id owning an entity (for the list endpoint's gate).
/// `None` for a workspace-level page. Errors if the entity doesn't exist.
pub(crate) fn resolve_entity_project(
    db: &DbPool,
    entity: AttachmentEntity,
    entity_id: i64,
) -> Result<Option<i64>, LificError> {
    with_read(db, |conn| match entity {
        AttachmentEntity::Issue => queries::get_issue(conn, entity_id).map(|i| Some(i.project_id)),
        AttachmentEntity::Page => queries::get_page(conn, entity_id).map(|p| p.project_id),
        AttachmentEntity::Comment => {
            let c = queries::comments::get_comment(conn, entity_id)?;
            if let Some(iid) = c.issue_id {
                queries::get_issue(conn, iid).map(|i| Some(i.project_id))
            } else if let Some(pid) = c.page_id {
                queries::get_page(conn, pid).map(|p| p.project_id)
            } else {
                Ok(None)
            }
        }
    })
}

/// `GET /api/projects/{id}/attachments` — the project files manager listing
/// (LIF-418): every attachment linked to any issue, page, or comment in the
/// project, paginated, filterable by `mime_class` / `uploader` /
/// `entity_type`, sortable by `created_at` (default, newest first) / `size` /
/// `filename`, with a `total_count` + `total_bytes` header for the whole
/// filtered set.
///
/// Viewer-gated. The gate runs *before* anything touches the project row, so a
/// non-member gets the same 403 whether or not the project exists — they can't
/// probe for it through a 404-vs-403 side channel (the same reasoning
/// `/api/search` applies to hidden projects).
pub(super) async fn list_project_attachments(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(project_id): Path<i64>,
    Query(query): Query<ProjectAttachmentQuery>,
) -> Result<axum::Json<ProjectAttachmentPage>, LificError> {
    authz::require_role(&db, &identity, project_id, Role::Viewer)?;
    with_read(&db, |conn| {
        q::list_project_attachments(conn, project_id, &query)
    })
    .map(axum::Json)
}

/// `GET /api/projects/{id}/attachments/orphans` — uploads by this project's
/// members that have no links and are queued for the orphan sweeper, with the
/// time each has left before collection.
///
/// Same Viewer gate, same reasoning, as the listing above.
pub(super) async fn list_project_orphans(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(project_id): Path<i64>,
) -> Result<axum::Json<PendingOrphanList>, LificError> {
    authz::require_role(&db, &identity, project_id, Role::Viewer)?;
    let items = with_read(&db, |conn| {
        q::list_project_orphans(conn, project_id, storage::ORPHAN_GRACE_SECONDS)
    })?;
    let total_bytes = items.iter().map(|orphan| orphan.size_bytes).sum();
    Ok(axum::Json(PendingOrphanList {
        items,
        grace_seconds: storage::ORPHAN_GRACE_SECONDS,
        total_bytes,
    }))
}

/// `GET /api/attachments/{id}` — stream the bytes with the correct
/// `Content-Type`.
///
/// Three layers keep a hostile upload from executing on our origin:
/// - `X-Content-Type-Options: nosniff`, so a browser never re-guesses the type.
/// - `Content-Disposition: inline` only for the raster formats in
///   [`storage::is_inline_safe_mime`]; everything else, SVG included, is
///   forced to `attachment`. An SVG is an image that can carry `<script>`,
///   and browsers run it when the file is navigated to directly.
/// - `Content-Security-Policy: default-src 'none'; sandbox`, which neuters
///   script, fetch and form submission even if a future MIME slips through
///   the disposition rule.
///
/// Content-addressed, so the response is immutable-cacheable forever.
///
/// LIF-418: the route also answers byte-range requests (`Accept-Ranges:
/// bytes`). Without them a `<video>` element can only play a file from the
/// start, because seeking is implemented as "re-request the middle of the
/// resource"; a server that answers 200-with-everything to every request makes
/// the scrub bar inert.
pub(super) async fn download_attachment(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Extension(store): Extension<AttachmentStore>,
    headers: axum::http::HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, LificError> {
    let attachment = with_read(&db, |conn| q::get_attachment(conn, id))?;

    // Authorize: the caller must be able to view SOME project this attachment
    // is linked into (Viewer), or be the uploader / an admin for a still-
    // unlinked attachment.
    authorize_read(&db, &identity, &attachment)?;

    let bytes = store.read(&attachment.sha256)?;
    let total = bytes.len() as u64;
    let inline_safe = storage::is_inline_safe_mime(&attachment.mime);

    // Force download for anything that isn't a plain raster image or a media
    // container. Either way the filename is offered for the "Save as" dialog.
    let disposition = if inline_safe {
        format!("inline; filename=\"{}\"", header_safe(&attachment.filename))
    } else {
        format!(
            "attachment; filename=\"{}\"",
            header_safe(&attachment.filename)
        )
    };

    let requested = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|v| parse_range(v, total))
        .unwrap_or(RangeRequest::Whole);

    let (status, body, content_range) = match requested {
        RangeRequest::Whole => (StatusCode::OK, bytes, None),
        RangeRequest::Unsatisfiable => {
            // RFC 9110: a 416 carries the resource's real length so the client
            // can re-ask sensibly, and no body worth reading.
            return Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::CONTENT_RANGE, format!("bytes */{total}"))
                .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
                .body(Body::empty())
                .map_err(|e| LificError::Internal(format!("build response: {e}")));
        }
        RangeRequest::Partial { start, end } => {
            let slice = bytes[start as usize..=end as usize].to_vec();
            (
                StatusCode::PARTIAL_CONTENT,
                slice,
                Some(format!("bytes {start}-{end}/{total}")),
            )
        }
    };

    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, &attachment.mime)
        .header(header::CONTENT_LENGTH, body.len())
        .header(header::ACCEPT_RANGES, "bytes")
        // Content-addressed: the same id always returns the same bytes.
        .header(
            header::CACHE_CONTROL,
            "private, max-age=31536000, immutable",
        )
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        // Defense in depth behind the disposition rule: even if a scriptable
        // type were ever served inline, this document can't load or run
        // anything. `sandbox` (no allow-* tokens) also drops it into an
        // opaque origin, so it can't touch our localStorage or cookies.
        .header(
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; sandbox",
        )
        .header(header::CONTENT_DISPOSITION, disposition);
    if let Some(range) = content_range {
        builder = builder.header(header::CONTENT_RANGE, range);
    }

    builder
        .body(Body::from(body))
        .map_err(|e| LificError::Internal(format!("build response: {e}")))
}

/// What a `Range` header asked for, resolved against the resource length.
#[derive(Debug, PartialEq, Eq)]
enum RangeRequest {
    /// Serve the entire resource with a 200. Also the answer for a header we
    /// are entitled to ignore.
    Whole,
    /// Serve `[start, end]` inclusive with a 206.
    Partial { start: u64, end: u64 },
    /// The range cannot be satisfied: 416.
    Unsatisfiable,
}

/// Parse a single-range `bytes=` header against a known resource length.
///
/// Supports the three forms a media element actually emits: `bytes=start-end`,
/// `bytes=start-` (from here to the end, the common first probe) and
/// `bytes=-suffix` (the last N bytes, used to find an MP4 moov atom at the
/// tail).
///
/// Anything else returns [`RangeRequest::Whole`], which RFC 9110 explicitly
/// permits: a server must ignore a range unit it does not understand, and may
/// ignore a multi-range request rather than build a multipart body. Only a
/// well-formed range that points outside the resource is a 416, because that
/// one is a genuine client error rather than a capability gap.
fn parse_range(value: &str, total: u64) -> RangeRequest {
    let Some(spec) = value.trim().strip_prefix("bytes=") else {
        return RangeRequest::Whole;
    };
    if spec.contains(',') {
        return RangeRequest::Whole;
    }
    let Some((raw_start, raw_end)) = spec.split_once('-') else {
        return RangeRequest::Whole;
    };
    let (raw_start, raw_end) = (raw_start.trim(), raw_end.trim());

    // A zero-length resource can satisfy no range at all.
    if total == 0 {
        return RangeRequest::Unsatisfiable;
    }

    if raw_start.is_empty() {
        // Suffix form: the last `n` bytes. `bytes=-0` asks for nothing, which
        // is unsatisfiable rather than an empty 206.
        let Ok(suffix) = raw_end.parse::<u64>() else {
            return RangeRequest::Whole;
        };
        if suffix == 0 {
            return RangeRequest::Unsatisfiable;
        }
        let start = total.saturating_sub(suffix);
        return RangeRequest::Partial {
            start,
            end: total - 1,
        };
    }

    let Ok(start) = raw_start.parse::<u64>() else {
        return RangeRequest::Whole;
    };
    if start >= total {
        return RangeRequest::Unsatisfiable;
    }
    let end = if raw_end.is_empty() {
        total - 1
    } else {
        match raw_end.parse::<u64>() {
            // A last-byte-pos past the end is clamped, not rejected.
            Ok(end) => end.min(total - 1),
            Err(_) => return RangeRequest::Whole,
        }
    };
    if end < start {
        return RangeRequest::Unsatisfiable;
    }
    RangeRequest::Partial { start, end }
}

// ── Thumbnails (LIF-418) ─────────────────────────────────────

/// Generate and cache a thumbnail for freshly-stored bytes, swallowing every
/// failure. A thumbnail is a convenience derived from the blob; if it cannot
/// be produced the endpoint simply 404s and callers fall back to the original.
/// Nothing here is allowed to fail an upload.
fn cache_thumbnail(store: &AttachmentStore, sha: &str, bytes: &[u8]) {
    match storage::generate_thumbnail(bytes) {
        Ok(Some(thumb)) => {
            if let Err(e) = store.write_thumb(sha, &thumb) {
                tracing::warn!(error = %e, "failed to cache attachment thumbnail");
            }
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(error = %e, "failed to generate attachment thumbnail"),
    }
}

/// `GET /api/attachments/{id}/thumbnail`: a 480px-long-edge WebP preview of a
/// raster attachment.
///
/// 404 when the attachment is not a raster image, or is already small enough
/// that the original IS the thumbnail. Generation is lazy: attachments
/// uploaded before LIF-418 have no cached file, so the first request builds
/// one and every later request is served from disk.
pub(super) async fn attachment_thumbnail(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Extension(store): Extension<AttachmentStore>,
    Path(id): Path<i64>,
) -> Result<Response, LificError> {
    let attachment = with_read(&db, |conn| q::get_attachment(conn, id))?;
    authorize_read(&db, &identity, &attachment)?;

    if !storage::is_raster_mime(&attachment.mime) {
        return Err(LificError::NotFound(
            "no thumbnail for this attachment".into(),
        ));
    }

    let thumb = match store.read_thumb(&attachment.sha256)? {
        Some(bytes) => bytes,
        None => {
            let source = store.read(&attachment.sha256)?;
            match storage::generate_thumbnail(&source) {
                Ok(Some(bytes)) => {
                    // Best-effort cache write: a read-only or full disk should
                    // still serve the thumbnail it just built.
                    if let Err(e) = store.write_thumb(&attachment.sha256, &bytes) {
                        tracing::warn!(error = %e, "failed to cache attachment thumbnail");
                    }
                    bytes
                }
                // Small enough to need none, or undecodable: both are "there
                // is no thumbnail here", not a server error.
                Ok(None) | Err(_) => {
                    return Err(LificError::NotFound(
                        "no thumbnail for this attachment".into(),
                    ));
                }
            }
        }
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/webp")
        .header(header::CONTENT_LENGTH, thumb.len())
        .header(
            header::CACHE_CONTROL,
            "private, max-age=31536000, immutable",
        )
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; sandbox",
        )
        .header(
            header::CONTENT_DISPOSITION,
            format!(
                "inline; filename=\"{}.webp\"",
                header_safe(&attachment.filename)
            ),
        )
        .body(Body::from(thumb))
        .map_err(|e| LificError::Internal(format!("build response: {e}")))
}

// ── Alt text (LIF-418) ───────────────────────────────────────

/// Body of `PATCH /api/attachments/{id}`. `alt_text: null` clears the
/// description; omitting the key entirely leaves it alone.
#[derive(Debug, Default, serde::Deserialize)]
pub(super) struct UpdateAttachmentBody {
    #[serde(default, deserialize_with = "crate::db::models::deserialize_nullable")]
    alt_text: Option<Option<String>>,
}

/// Longest alt text we store. Screen readers stop being useful long before
/// this; the cap exists so the column cannot be used as free-form storage.
const MAX_ALT_TEXT: usize = 1000;

/// `PATCH /api/attachments/{id}`: set or clear the accessibility description.
///
/// Gated exactly like `DELETE`: the uploader, an admin, or a Maintainer on a
/// project the attachment is linked into. Describing a file is an edit of that
/// file's metadata, so it should cost the same permission as removing it, and
/// sharing one gate means the two can never drift apart.
pub(super) async fn update_attachment(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(id): Path<i64>,
    axum::Json(body): axum::Json<UpdateAttachmentBody>,
) -> Result<axum::Json<Attachment>, LificError> {
    let user = require_user(&identity)?;
    let attachment = with_read(&db, |conn| q::get_attachment(conn, id))?;
    authorize_delete(&db, &identity, &user, &attachment)?;

    let Some(alt_text) = body.alt_text else {
        // Nothing to change; echo the current row rather than 400, so a
        // client that PATCHes an unchanged form is a no-op.
        return Ok(axum::Json(attachment));
    };

    // Normalize so there is one representation of "no alt text": trim, and
    // fold the empty string onto NULL.
    let normalized = alt_text
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(text) = normalized.as_deref()
        && text.chars().count() > MAX_ALT_TEXT
    {
        return Err(LificError::BadRequest(format!(
            "alt_text too long: max {MAX_ALT_TEXT} characters"
        )));
    }

    let updated = with_write(&db, |conn| {
        q::update_alt_text(conn, id, normalized.as_deref())
    })?;
    Ok(axum::Json(updated))
}

/// `DELETE /api/attachments/{id}` — uploader, a Maintainer on an owning
/// project, or an admin. Removes the metadata row (links cascade), then sweeps
/// the sidecar file if no other row shares the content hash.
pub(super) async fn delete_attachment(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(store): Extension<AttachmentStore>,
    Path(id): Path<i64>,
) -> Result<axum::Json<serde_json::Value>, LificError> {
    let user = require_user(&identity)?;
    let attachment = with_read(&db, |conn| q::get_attachment(conn, id))?;

    authorize_delete(&db, &identity, &user, &attachment)?;

    let events = with_write(&db, |conn| {
        let events = linked_attachment_events(conn, id)?;
        q::delete_attachment(conn, id)?;
        Ok(events)
    })?;
    for event in events {
        realtime.send(event);
    }

    // GC the sidecar only when no remaining row references the same bytes.
    let remaining = with_read(&db, |conn| q::count_rows_for_sha(conn, &attachment.sha256))?;
    if remaining == 0 {
        store.delete(&attachment.sha256)?;
    }

    Ok(axum::Json(serde_json::json!({ "deleted": true })))
}

// ── Where-used + dedup (LIF-418) ─────────────────────────────

/// One place an attachment is referenced from. `identifier` is the
/// human-facing key (`LIF-12`, `LIF-DOC-3`) where the entity has one, and null
/// for a comment, which is addressed through its parent rather than in its own
/// right.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LinkedEntity {
    pub entity_type: String,
    pub entity_id: i64,
    pub identifier: Option<String>,
    pub title: String,
}

/// Another attachment row over the same bytes, with its own usages.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DuplicateAttachment {
    pub attachment_id: i64,
    pub filename: String,
    pub entities: Vec<LinkedEntity>,
}

/// Body of `GET /api/attachments/{id}/links`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AttachmentLinks {
    pub entities: Vec<LinkedEntity>,
    pub duplicates: Vec<DuplicateAttachment>,
}

/// `GET /api/attachments/{id}/links`: where this file is used, and which
/// other attachment rows point at the same bytes.
///
/// Uploading the same screenshot twice creates two rows over one blob (the
/// store is content-addressed, so the bytes are shared but the metadata is
/// not). Before deleting an attachment, or before uploading a third copy, the
/// useful question is "what would this affect", and that is what this answers.
///
/// Every entity in the response is filtered through the caller's project
/// visibility, including the ones reached via a duplicate. Otherwise the
/// endpoint would be an oracle: upload a file, and the duplicate list tells
/// you the titles of issues in projects you cannot see that happen to contain
/// the same image. A duplicate whose usages are all invisible still appears,
/// with an empty `entities` array, since knowing a copy exists is not the same
/// as learning where.
pub(super) async fn attachment_links(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(id): Path<i64>,
) -> Result<axum::Json<AttachmentLinks>, LificError> {
    let attachment = with_read(&db, |conn| q::get_attachment(conn, id))?;
    authorize_read(&db, &identity, &attachment)?;

    let entities = visible_links(&db, &identity, id)?;

    let siblings = with_read(&db, |conn| {
        q::duplicates_of(conn, attachment.id, &attachment.sha256)
    })?;
    let mut duplicates = Vec::with_capacity(siblings.len());
    for sibling in siblings {
        duplicates.push(DuplicateAttachment {
            attachment_id: sibling.id,
            entities: visible_links(&db, &identity, sibling.id)?,
            filename: sibling.filename,
        });
    }

    Ok(axum::Json(AttachmentLinks {
        entities,
        duplicates,
    }))
}

/// The entities linking `attachment_id` that this caller may see. A link into
/// a project the caller has no Viewer role on is dropped silently, as is a
/// link to an entity that has since been deleted.
fn visible_links(
    db: &DbPool,
    identity: &Option<crate::resolve_caller::ResolvedIdentity>,
    attachment_id: i64,
) -> Result<Vec<LinkedEntity>, LificError> {
    let links = with_read(db, |conn| q::links_for_attachment(conn, attachment_id))?;

    let mut out = Vec::new();
    for (entity_type, entity_id) in links {
        let Ok(entity) = entity_type.parse::<AttachmentEntity>() else {
            continue;
        };
        // A dangling link (entity deleted between the link row and now) is
        // skipped rather than surfaced as a 404 for the whole request.
        let Ok(project_id) = resolve_entity_project(db, entity, entity_id) else {
            continue;
        };
        let visible = match project_id {
            Some(pid) => authz::require_role(db, identity, pid, Role::Viewer).is_ok(),
            None => authz::require_workspace_admin(db, identity).is_ok(),
        };
        if !visible {
            continue;
        }
        if let Some(described) = describe_entity(db, entity, entity_id)? {
            out.push(described);
        }
    }
    Ok(out)
}

/// Longest comment excerpt used as a link title. A comment has no title of its
/// own, so its first line stands in for one.
const COMMENT_TITLE_CHARS: usize = 80;

/// Resolve an entity to the identifier + title shown in a "where used" list.
fn describe_entity(
    db: &DbPool,
    entity: AttachmentEntity,
    entity_id: i64,
) -> Result<Option<LinkedEntity>, LificError> {
    with_read(db, |conn| {
        let described = match entity {
            AttachmentEntity::Issue => queries::get_issue(conn, entity_id).ok().map(|issue| {
                LinkedEntity {
                    entity_type: "issue".into(),
                    entity_id,
                    identifier: Some(issue.identifier),
                    title: issue.title,
                }
            }),
            AttachmentEntity::Page => {
                queries::get_page(conn, entity_id).ok().map(|page| LinkedEntity {
                    entity_type: "page".into(),
                    entity_id,
                    identifier: Some(page.identifier),
                    title: page.title,
                })
            }
            AttachmentEntity::Comment => queries::comments::get_comment(conn, entity_id)
                .ok()
                .map(|comment| LinkedEntity {
                    entity_type: "comment".into(),
                    entity_id,
                    // A comment is not addressable by identifier; the client
                    // navigates to it through its parent issue or page.
                    identifier: None,
                    title: comment_title(&comment.content),
                }),
        };
        Ok(described)
    })
}

/// A comment's first non-empty line, trimmed to a label-sized excerpt.
fn comment_title(content: &str) -> String {
    let line = content
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if line.chars().count() > COMMENT_TITLE_CHARS {
        let head: String = line.chars().take(COMMENT_TITLE_CHARS).collect();
        format!("{head}...")
    } else {
        line.to_string()
    }
}

// ── Structured preview (LIF-418) ─────────────────────────────

/// `GET /api/attachments/{id}/preview`: what is inside a container upload.
///
/// Zip archives report their central directory, SQLite databases report their
/// tables and row counts, and everything else reports `{"kind":"none"}`. Gated
/// exactly like the download, because a preview is a read of the same bytes in
/// a more convenient shape. See `crate::preview` for how both parsers are kept
/// from trusting the file.
pub(super) async fn attachment_preview(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Extension(store): Extension<AttachmentStore>,
    Path(id): Path<i64>,
) -> Result<axum::Json<crate::preview::Preview>, LificError> {
    let attachment = with_read(&db, |conn| q::get_attachment(conn, id))?;
    authorize_read(&db, &identity, &attachment)?;

    let bytes = store.read(&attachment.sha256)?;
    Ok(axum::Json(crate::preview::preview_bytes(&bytes)?))
}

/// Return the invalidation event for one attachment link. Comment links refresh
/// their parent issue or project page. Missing entities are ignored so an old
/// dangling link does not prevent attachment deletion.
pub(crate) fn linked_entity_event(
    conn: &rusqlite::Connection,
    entity: AttachmentEntity,
    entity_id: i64,
) -> Result<Option<RealtimeEvent>, LificError> {
    match entity {
        AttachmentEntity::Issue => match queries::get_issue(conn, entity_id) {
            Ok(issue) => Ok(Some(RealtimeEvent::IssueUpdated {
                project_id: issue.project_id,
                issue_id: issue.id,
            })),
            Err(LificError::NotFound(_)) => Ok(None),
            Err(error) => Err(error),
        },
        AttachmentEntity::Page => match queries::get_page(conn, entity_id) {
            Ok(page) => Ok(page
                .project_id
                .map(|project_id| RealtimeEvent::ProjectUpdated { project_id })),
            Err(LificError::NotFound(_)) => Ok(None),
            Err(error) => Err(error),
        },
        AttachmentEntity::Comment => match queries::comments::get_comment(conn, entity_id) {
            Ok(comment) => {
                if let Some(issue_id) = comment.issue_id {
                    linked_entity_event(conn, AttachmentEntity::Issue, issue_id)
                } else if let Some(page_id) = comment.page_id {
                    linked_entity_event(conn, AttachmentEntity::Page, page_id)
                } else {
                    Ok(None)
                }
            }
            Err(LificError::NotFound(_)) => Ok(None),
            Err(error) => Err(error),
        },
    }
}

/// Snapshot all affected issue/page entities before an attachment's link rows
/// cascade away. A single attachment can affect multiple projects.
fn linked_attachment_events(
    conn: &rusqlite::Connection,
    attachment_id: i64,
) -> Result<Vec<RealtimeEvent>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT entity_type, entity_id FROM attachment_links WHERE attachment_id = ?1",
    )?;
    let links: Vec<(String, i64)> = stmt
        .query_map([attachment_id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;

    let mut events = Vec::new();
    for (entity_type, entity_id) in links {
        let event = match entity_type.parse::<AttachmentEntity>() {
            Ok(entity) => linked_entity_event(conn, entity, entity_id)?,
            Err(_) => None,
        };
        if let Some(event) = event
            && !events.contains(&event)
        {
            events.push(event);
        }
    }
    Ok(events)
}

// ── Authorization helpers ────────────────────────────────────

/// Resolve every distinct project id an attachment is linked into (via its
/// issue/page/comment links). Empty when the attachment is unlinked.
fn owning_project_ids(
    conn: &rusqlite::Connection,
    attachment_id: i64,
) -> Result<Vec<i64>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT entity_type, entity_id FROM attachment_links WHERE attachment_id = ?1",
    )?;
    let links: Vec<(String, i64)> = stmt
        .query_map([attachment_id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;

    let mut project_ids = Vec::new();
    for (entity_type, entity_id) in links {
        let pid = match entity_type.as_str() {
            "issue" => queries::get_issue(conn, entity_id)
                .ok()
                .map(|i| i.project_id),
            "page" => queries::get_page(conn, entity_id)
                .ok()
                .and_then(|p| p.project_id),
            "comment" => {
                let comment = queries::comments::get_comment(conn, entity_id).ok();
                match comment {
                    Some(c) if c.issue_id.is_some() => {
                        queries::get_issue(conn, c.issue_id.unwrap())
                            .ok()
                            .map(|i| i.project_id)
                    }
                    Some(c) if c.page_id.is_some() => queries::get_page(conn, c.page_id.unwrap())
                        .ok()
                        .and_then(|p| p.project_id),
                    _ => None,
                }
            }
            _ => None,
        };
        if let Some(pid) = pid
            && !project_ids.contains(&pid)
        {
            project_ids.push(pid);
        }
    }
    Ok(project_ids)
}

/// LIF-405: link gate. Uploading a blob is open to any authenticated user,
/// but attaching it to an entity mutates *that* entity — before this check a
/// stranger could drop files onto issues, pages and comments in projects they
/// had no membership in at all.
///
/// The required role mirrors the entity's own mutation path, so linking never
/// buys more than editing the thing directly would:
/// - issue / page → `Maintainer`, matching `PUT /api/issues/{id}` and
///   `PUT /api/pages/{id}`.
/// - comment → `Viewer`, matching `POST /api/issues/{id}/comments`, which is
///   the path that puts an attachment on a comment in the first place.
///
/// A workspace-level page (no project) falls back to workspace-admin, the
/// same fallback `list_entity_attachments` uses on the read side. A link to a
/// nonexistent entity is `NotFound`, via `resolve_entity_project`.
pub(crate) fn authorize_link(
    db: &DbPool,
    identity: &Option<crate::resolve_caller::ResolvedIdentity>,
    entity: AttachmentEntity,
    entity_id: i64,
) -> Result<(), LificError> {
    let project_id = resolve_entity_project(db, entity, entity_id)?;
    let min = match entity {
        AttachmentEntity::Issue | AttachmentEntity::Page => Role::Maintainer,
        AttachmentEntity::Comment => Role::Viewer,
    };
    match project_id {
        Some(pid) => authz::require_role(db, identity, pid, min),
        None => authz::require_workspace_admin(db, identity),
    }
}

/// Read gate: Viewer on any owning project, or uploader/admin for an unlinked
/// attachment. When enforcement is off, `require_role(.., Viewer)` is an
/// unconditional allow (legacy mode), so this reduces to today's open read
/// behavior — matching every other GET while the flag is off.
pub(crate) fn authorize_read(
    db: &DbPool,
    identity: &Option<crate::resolve_caller::ResolvedIdentity>,
    attachment: &Attachment,
) -> Result<(), LificError> {
    let project_ids = with_read(db, |conn| owning_project_ids(conn, attachment.id))?;

    if project_ids.is_empty() {
        // Unlinked: only the uploader or an admin can read it. (When
        // enforcement is off we still restrict unlinked reads to the uploader
        // to avoid an enumeration hole on freshly-uploaded blobs.)
        match identity.as_ref().map(|i| &i.user) {
            Some(u) if u.is_admin => Ok(()),
            Some(u) if Some(u.id) == attachment.uploader_id => Ok(()),
            _ => Err(LificError::Forbidden(
                "not authorized to read this attachment".into(),
            )),
        }
    } else {
        // Viewer on ANY linked project is enough to read.
        let mut last_err = None;
        for pid in project_ids {
            match authz::require_role(db, identity, pid, Role::Viewer) {
                Ok(()) => return Ok(()),
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| {
            LificError::Forbidden("not authorized to read this attachment".into())
        }))
    }
}

/// Delete gate: uploader, admin, or Maintainer on any owning project.
fn authorize_delete(
    db: &DbPool,
    identity: &Option<crate::resolve_caller::ResolvedIdentity>,
    user: &AuthUser,
    attachment: &Attachment,
) -> Result<(), LificError> {
    if user.is_admin || Some(user.id) == attachment.uploader_id {
        return Ok(());
    }
    let project_ids = with_read(db, |conn| owning_project_ids(conn, attachment.id))?;
    for pid in project_ids {
        if authz::require_role(db, identity, pid, Role::Maintainer).is_ok() {
            return Ok(());
        }
    }
    Err(LificError::Forbidden(
        "only the uploader, a project maintainer, or an admin can delete this attachment".into(),
    ))
}

// ── Rate limiter newtype ─────────────────────────────────────

/// Per-user upload rate limiter. Newtyped so it's a distinct `Extension` type
/// from the login/OAuth limiters that share the same `RateLimiter` shape.
pub struct AttachmentUploadLimiter(pub RateLimiter);

// ── Filename / header hygiene ────────────────────────────────

/// Strip path components and control characters from a client-supplied
/// filename so it's safe to store and echo back. Never used as an on-disk path
/// (bytes are content-addressed) — this is purely the display/download name.
pub(crate) fn sanitize_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name).trim();
    let cleaned: String = base.chars().filter(|c| !c.is_control()).take(255).collect();
    if cleaned.is_empty() {
        "upload".to_string()
    } else {
        cleaned
    }
}

/// Escape a filename for safe inclusion in a `Content-Disposition` header
/// value (quote + backslash are the only bytes that break the quoted-string).
fn header_safe(name: &str) -> String {
    name.replace('\\', "_").replace('"', "'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_paths_and_control_chars() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("C:\\Windows\\evil.exe"), "evil.exe");
        assert_eq!(sanitize_filename("nam\u{0007}e.png"), "name.png");
        assert_eq!(sanitize_filename("   "), "upload");
    }

    #[test]
    fn header_safe_neutralizes_quotes() {
        assert_eq!(header_safe(r#"a"b\c.png"#), "a'b_c.png");
    }

    // ── LIF-418: Range header parsing ────────────────────────

    #[test]
    fn parses_the_three_range_forms_a_media_element_sends() {
        assert_eq!(
            parse_range("bytes=0-99", 1000),
            RangeRequest::Partial { start: 0, end: 99 }
        );
        // Open-ended: the first probe a video element makes.
        assert_eq!(
            parse_range("bytes=500-", 1000),
            RangeRequest::Partial {
                start: 500,
                end: 999
            }
        );
        // Suffix: how an mp4 player finds a trailing moov atom.
        assert_eq!(
            parse_range("bytes=-100", 1000),
            RangeRequest::Partial {
                start: 900,
                end: 999
            }
        );
    }

    #[test]
    fn range_end_past_the_resource_is_clamped_not_rejected() {
        assert_eq!(
            parse_range("bytes=990-99999", 1000),
            RangeRequest::Partial {
                start: 990,
                end: 999
            }
        );
        // A suffix longer than the resource is the whole resource.
        assert_eq!(
            parse_range("bytes=-99999", 1000),
            RangeRequest::Partial { start: 0, end: 999 }
        );
    }

    #[test]
    fn ranges_outside_the_resource_are_unsatisfiable() {
        assert_eq!(parse_range("bytes=1000-", 1000), RangeRequest::Unsatisfiable);
        assert_eq!(
            parse_range("bytes=1500-1600", 1000),
            RangeRequest::Unsatisfiable
        );
        assert_eq!(parse_range("bytes=50-10", 1000), RangeRequest::Unsatisfiable);
        assert_eq!(parse_range("bytes=-0", 1000), RangeRequest::Unsatisfiable);
        assert_eq!(parse_range("bytes=0-0", 0), RangeRequest::Unsatisfiable);
    }

    /// RFC 9110 says a server must ignore a range unit it does not
    /// understand, and lets it ignore requests it cannot answer in one part.
    /// Ignoring means a normal 200 with the whole body, never an error.
    #[test]
    fn unsupported_range_syntax_falls_back_to_the_whole_body() {
        assert_eq!(parse_range("items=0-10", 1000), RangeRequest::Whole);
        assert_eq!(parse_range("bytes=0-10,20-30", 1000), RangeRequest::Whole);
        assert_eq!(parse_range("bytes=abc-def", 1000), RangeRequest::Whole);
        assert_eq!(parse_range("bytes=", 1000), RangeRequest::Whole);
        assert_eq!(parse_range("nonsense", 1000), RangeRequest::Whole);
    }

    #[test]
    fn comment_titles_are_first_line_excerpts() {
        assert_eq!(comment_title("hello there"), "hello there");
        assert_eq!(comment_title("\n\n  second line\nthird"), "second line");
        assert_eq!(comment_title(""), "");
        let long = "x".repeat(200);
        let title = comment_title(&long);
        assert_eq!(title.chars().count(), COMMENT_TITLE_CHARS + 3);
        assert!(title.ends_with("..."));
    }
}

#[cfg(test)]
mod api_tests {
    use crate::api::test_helpers::*;
    use crate::db::models::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const BOUNDARY: &str = "----lifictestboundary";

    /// Minimal PNG: the 8-byte signature is enough for the magic-byte sniffer.
    fn png_bytes() -> Vec<u8> {
        let mut v = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        v.extend_from_slice(b"the rest is arbitrary pixel data");
        v
    }

    /// Build a multipart body with a single `file` part (and optional link
    /// fields).
    fn multipart_body(
        filename: &str,
        content_type: &str,
        bytes: &[u8],
        link: Option<(&str, i64)>,
    ) -> Vec<u8> {
        let mut body = Vec::new();
        let push = |body: &mut Vec<u8>, s: &str| body.extend_from_slice(s.as_bytes());
        push(&mut body, &format!("--{BOUNDARY}\r\n"));
        push(
            &mut body,
            &format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n"),
        );
        push(&mut body, &format!("Content-Type: {content_type}\r\n\r\n"));
        body.extend_from_slice(bytes);
        push(&mut body, "\r\n");
        if let Some((entity_type, entity_id)) = link {
            push(&mut body, &format!("--{BOUNDARY}\r\n"));
            push(
                &mut body,
                "Content-Disposition: form-data; name=\"entity_type\"\r\n\r\n",
            );
            push(&mut body, &format!("{entity_type}\r\n"));
            push(&mut body, &format!("--{BOUNDARY}\r\n"));
            push(
                &mut body,
                "Content-Disposition: form-data; name=\"entity_id\"\r\n\r\n",
            );
            push(&mut body, &format!("{entity_id}\r\n"));
        }
        push(&mut body, &format!("--{BOUNDARY}--\r\n"));
        body
    }

    /// Shared with `media_tests` (LIF-418) so both suites drive uploads
    /// through the same multipart body builder.
    pub(super) async fn upload(
        app: &axum::Router,
        filename: &str,
        content_type: &str,
        bytes: &[u8],
        link: Option<(&str, i64)>,
    ) -> axum::response::Response {
        let body = multipart_body(filename, content_type, bytes, link);
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/attachments")
                    .header(
                        "content-type",
                        format!("multipart/form-data; boundary={BOUNDARY}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn next_realtime_event(
        events: &mut tokio::sync::broadcast::Receiver<crate::realtime::RealtimeMessage>,
    ) -> serde_json::Value {
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .expect("no realtime event arrived within 1s (see LIF-347 before blaming load)")
            .expect("realtime broadcast recv failed (Closed or Lagged); capacity is 256, so Lagged here means something structural");
        let axum::extract::ws::Message::Text(text) = event.message else {
            panic!("expected text realtime event");
        };
        serde_json::from_str(&text).unwrap()
    }

    #[tokio::test]
    async fn issue_linked_upload_and_delete_emit_issue_updated_events() {
        let test = test_app_with_realtime();
        let (project_id, _) = seed_project(&test.app).await;
        let issue = parse_json(
            json_post(
                &test.app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id, "title": "Attachment target" }),
            )
            .await,
        )
        .await;
        let issue_id = issue["id"].as_i64().unwrap();
        let mut events = test.realtime.subscribe();

        let resp = upload(
            &test.app,
            "issue.png",
            "image/png",
            &png_bytes(),
            Some(("issue", issue_id)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let attachment_id = parse_json(resp).await["id"].as_i64().unwrap();
        let event = next_realtime_event(&mut events).await;
        assert_eq!(event["type"], "issue.updated");
        assert_eq!(event["project_id"], project_id);
        assert_eq!(event["issue_id"], issue_id);

        let resp = json_delete(&test.app, &format!("/api/attachments/{attachment_id}")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let event = next_realtime_event(&mut events).await;
        assert_eq!(event["type"], "issue.updated");
        assert_eq!(event["project_id"], project_id);
        assert_eq!(event["issue_id"], issue_id);
    }

    #[tokio::test]
    async fn project_page_linked_upload_and_delete_emit_project_updated_events() {
        let test = test_app_with_realtime();
        let (project_id, _) = seed_project(&test.app).await;
        let page = parse_json(
            json_post(
                &test.app,
                "/api/pages",
                serde_json::json!({
                    "project_id": project_id,
                    "title": "Attachment target page",
                }),
            )
            .await,
        )
        .await;
        let page_id = page["id"].as_i64().unwrap();
        let mut events = test.realtime.subscribe();

        let resp = upload(
            &test.app,
            "page.png",
            "image/png",
            &png_bytes(),
            Some(("page", page_id)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let attachment_id = parse_json(resp).await["id"].as_i64().unwrap();
        let event = next_realtime_event(&mut events).await;
        assert_eq!(event["type"], "project.updated");
        assert_eq!(event["project_id"], project_id);

        let resp = json_delete(&test.app, &format!("/api/attachments/{attachment_id}")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let event = next_realtime_event(&mut events).await;
        assert_eq!(event["type"], "project.updated");
        assert_eq!(event["project_id"], project_id);
    }

    #[tokio::test]
    async fn upload_and_download_happy_path() {
        let app = test_app();
        let resp = upload(&app, "shot.png", "image/png", &png_bytes(), None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        assert_eq!(data["mime"], "image/png");
        assert_eq!(data["filename"], "shot.png");
        let url = data["url"].as_str().unwrap().to_string();
        assert!(url.starts_with("/api/attachments/"));

        // Download it back.
        let resp = json_get(&app, &url).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get("content-type").unwrap(), "image/png");
        // Images render inline.
        assert!(
            resp.headers()
                .get("content-disposition")
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("inline"),
        );
        assert_eq!(
            resp.headers().get("x-content-type-options").unwrap(),
            "nosniff"
        );
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(bytes.as_ref(), png_bytes().as_slice());
    }

    #[tokio::test]
    async fn non_image_download_forces_attachment_disposition() {
        let app = test_app();
        let resp = upload(&app, "notes.txt", "text/plain", b"hello log file\n", None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let id = parse_json(resp).await["id"].as_i64().unwrap();

        let resp = json_get(&app, &format!("/api/attachments/{id}")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let disp = resp
            .headers()
            .get("content-disposition")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(disp.starts_with("attachment"), "got {disp}");
    }

    /// An SVG carrying a `<script>` is a real upload a real user can make.
    /// Served `inline` from our own origin it becomes stored XSS: the script
    /// runs as the viewer and can read the bearer token the SPA keeps in
    /// localStorage, turning any upload-capable account into whoever opens
    /// the file. It must download instead of rendering.
    #[tokio::test]
    async fn svg_download_forces_attachment_disposition() {
        let app = test_app();
        let hostile = br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#;

        let resp = upload(&app, "logo.svg", "image/svg+xml", hostile, None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        // SVG is still an accepted upload; only how we serve it changed.
        assert_eq!(data["mime"], "image/svg+xml");
        let id = data["id"].as_i64().unwrap();

        let resp = json_get(&app, &format!("/api/attachments/{id}")).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let disp = resp
            .headers()
            .get("content-disposition")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(
            disp.starts_with("attachment"),
            "SVG must never render inline, got {disp}"
        );
    }

    /// Defense in depth: every attachment response, whatever its type,
    /// carries a CSP that forbids script execution and network access.
    #[tokio::test]
    async fn every_attachment_response_carries_a_locked_down_csp() {
        let app = test_app();

        for (name, mime, bytes) in [
            ("shot.png", "image/png", png_bytes()),
            ("logo.svg", "image/svg+xml", b"<svg xmlns=\"x\"></svg>".to_vec()),
            ("notes.txt", "text/plain", b"hello\n".to_vec()),
        ] {
            let resp = upload(&app, name, mime, &bytes, None).await;
            assert_eq!(resp.status(), StatusCode::OK, "upload {name}");
            let id = parse_json(resp).await["id"].as_i64().unwrap();

            let resp = json_get(&app, &format!("/api/attachments/{id}")).await;
            let csp = resp
                .headers()
                .get("content-security-policy")
                .unwrap_or_else(|| panic!("{name} served without a CSP"))
                .to_str()
                .unwrap();
            assert!(csp.contains("default-src 'none'"), "{name}: {csp}");
            assert!(csp.contains("sandbox"), "{name}: {csp}");
        }
    }

    #[tokio::test]
    async fn upload_rejects_oversize() {
        // Build an app with a tiny max-bytes config so a small body trips it.
        let db = crate::db::open_memory().unwrap();
        let admin_id = {
            let conn = db.write().unwrap();
            conn.execute(
                "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot)
                 VALUES ('a','a@a','x','A',1,0)",
                [],
            )
            .unwrap();
            conn.last_insert_rowid()
        };
        use axum::Extension;
        use std::sync::Arc;
        let (store, _tmp) = test_attachment_store();
        let app = crate::api::router(db, &[])
            .layer(Extension(crate::realtime::RealtimeHub::new()))
            .layer(Extension(store))
            .layer(Extension(super::AttachmentConfig { max_bytes: 4 }))
            .layer(Extension(Arc::new(super::AttachmentUploadLimiter(
                crate::ratelimit::RateLimiter::new(1000, std::time::Duration::from_secs(3600)),
            ))))
            .layer(Extension(crate::config::AuthConfig {
                allow_signup: true,
                required: true,
                secure_cookies: false,
            }))
            .layer(Extension(Some(AuthUser {
                id: admin_id,
                username: "a".into(),
                display_name: "A".into(),
                is_admin: true,
            })))
            .layer(Extension(Some(crate::resolve_caller::ResolvedIdentity {
                user: AuthUser {
                    id: admin_id,
                    username: "a".into(),
                    display_name: "A".into(),
                    is_admin: true,
                },
                transport: crate::actor::Transport::Web,
            })))
            .layer(Extension(Some(crate::resolve_caller::ResolvedIdentity {
                user: AuthUser {
                    id: admin_id,
                    username: "a".into(),
                    display_name: "A".into(),
                    is_admin: true,
                },
                transport: crate::actor::Transport::Web,
            })));

        let resp = upload(&app, "big.png", "image/png", &png_bytes(), None).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let err = parse_json(resp).await;
        assert!(err["error"].as_str().unwrap().contains("too large"));
    }

    #[tokio::test]
    async fn upload_rejects_disallowed_mime_executable() {
        let app = test_app();
        // ELF header — must be rejected even when declared as an image.
        let resp = upload(&app, "evil", "image/png", b"\x7FELF\x02\x01\x01", None).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let err = parse_json(resp).await;
        assert!(err["error"].as_str().unwrap().contains("executable"));
    }

    #[tokio::test]
    async fn download_denies_non_member_when_enforced() {
        let (db, _admin, lead, _maintainer, _viewer, non_member, project_id) =
            setup_membership_test();

        // Lead creates an issue and uploads an attachment linked to it.
        let lead_app = with_attachment_layers(crate::api::router(db.clone(), &[]))
            .layer(axum::Extension(crate::realtime::RealtimeHub::new()))
            .layer(axum::Extension(crate::config::AuthConfig {
                allow_signup: true,
                required: true,
                secure_cookies: false,
            }))
            .layer(axum::Extension(Some(AuthUser {
                id: lead.id,
                username: lead.username.clone(),
                display_name: lead.display_name.clone(),
                is_admin: false,
            })))
            .layer(axum::Extension(Some(crate::resolve_caller::ResolvedIdentity {
                user: AuthUser {
                    id: lead.id,
                    username: lead.username.clone(),
                    display_name: lead.display_name.clone(),
                    is_admin: false,
                },
                transport: crate::actor::Transport::Web,
            })));

        let issue = parse_json(
            json_post(
                &lead_app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id, "title": "secret" }),
            )
            .await,
        )
        .await;
        let issue_id = issue["id"].as_i64().unwrap();

        let resp = upload(
            &lead_app,
            "s.png",
            "image/png",
            &png_bytes(),
            Some(("issue", issue_id)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let att_id = parse_json(resp).await["id"].as_i64().unwrap();

        // A non-member must be denied reading the linked attachment.
        let non_member_app = app_as_user(db.clone(), &non_member);
        let resp = json_get(&non_member_app, &format!("/api/attachments/{att_id}")).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        // The lead (member) can read it.
        let resp = json_get(&lead_app, &format!("/api/attachments/{att_id}")).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// LIF-405: the upload's optional link fields used to be taken on trust.
    /// Any authenticated account could post a file with `entity_type=issue`
    /// and an id from a project it had no membership in, and the attachment
    /// would show up in that issue's Attachments section.
    #[tokio::test]
    async fn non_member_cannot_link_an_upload_to_a_foreign_issue() {
        let (db, _admin, lead, maintainer, viewer, non_member, project_id) =
            setup_membership_test();

        let lead_app = app_as_user(db.clone(), &lead);
        let issue = parse_json(
            json_post(
                &lead_app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id, "title": "not yours" }),
            )
            .await,
        )
        .await;
        let issue_id = issue["id"].as_i64().unwrap();

        let link_count = |db: &crate::db::DbPool| -> i64 {
            let conn = db.read().unwrap();
            conn.query_row("SELECT COUNT(*) FROM attachment_links", [], |r| r.get(0))
                .unwrap()
        };

        // A non-member is refused, and nothing is recorded.
        let non_member_app = app_as_user(db.clone(), &non_member);
        let resp = upload(
            &non_member_app,
            "sneak.png",
            "image/png",
            &png_bytes(),
            Some(("issue", issue_id)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(link_count(&db), 0, "a rejected link must leave no row");

        // So is a Viewer: attaching to an issue is an edit of that issue, and
        // `PUT /api/issues/{id}` is Maintainer-gated.
        let viewer_app = app_as_user(db.clone(), &viewer);
        let resp = upload(
            &viewer_app,
            "viewer.png",
            "image/png",
            &png_bytes(),
            Some(("issue", issue_id)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(link_count(&db), 0);

        // A Maintainer on the project still links normally.
        let maintainer_app = app_as_user(db.clone(), &maintainer);
        let resp = upload(
            &maintainer_app,
            "ok.png",
            "image/png",
            &png_bytes(),
            Some(("issue", issue_id)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let att_id = parse_json(resp).await["id"].as_i64().unwrap();
        assert_eq!(link_count(&db), 1);

        let list = parse_json(
            json_get(
                &maintainer_app,
                &format!("/api/attachments?entity_type=issue&entity_id={issue_id}"),
            )
            .await,
        )
        .await;
        let arr = list.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"].as_i64().unwrap(), att_id);
    }

    /// A plain unlinked upload is still open to any authenticated user —
    /// LIF-405 gates the link, not the blob.
    #[tokio::test]
    async fn non_member_can_still_upload_without_a_link() {
        let (db, _admin, _lead, _maintainer, _viewer, non_member, _project_id) =
            setup_membership_test();

        let non_member_app = app_as_user(db.clone(), &non_member);
        let resp = upload(&non_member_app, "mine.png", "image/png", &png_bytes(), None).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn delete_permissions_uploader_and_maintainer() {
        let (db, _admin, lead, maintainer, viewer, _non_member, project_id) =
            setup_membership_test();

        // Maintainer uploads (linked to a lead-created issue).
        let lead_app = app_as_user(db.clone(), &lead);
        let issue = parse_json(
            json_post(
                &lead_app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id, "title": "t" }),
            )
            .await,
        )
        .await;
        let issue_id = issue["id"].as_i64().unwrap();

        let maintainer_app = app_as_user(db.clone(), &maintainer);
        let resp = upload(
            &maintainer_app,
            "m.png",
            "image/png",
            &png_bytes(),
            Some(("issue", issue_id)),
        )
        .await;
        let att_id = parse_json(resp).await["id"].as_i64().unwrap();

        // A viewer can't delete it.
        let viewer_app = app_as_user(db.clone(), &viewer);
        assert_eq!(
            json_delete(&viewer_app, &format!("/api/attachments/{att_id}"))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );

        // The uploader (maintainer) can.
        assert_eq!(
            json_delete(&maintainer_app, &format!("/api/attachments/{att_id}"))
                .await
                .status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn markdown_reference_records_link_on_issue_save() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;

        // Upload an (unlinked) attachment first.
        let resp = upload(&app, "img.png", "image/png", &png_bytes(), None).await;
        let att_id = parse_json(resp).await["id"].as_i64().unwrap();

        // Create an issue whose description embeds the attachment.
        let body = serde_json::json!({
            "project_id": project_id,
            "title": "with image",
            "description": format!("Here: ![shot](/api/attachments/{att_id})"),
        });
        let resp = json_post(&app, "/api/issues", body).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let issue_id = parse_json(resp).await["id"].as_i64().unwrap();

        // The link is now recorded — the entity-list endpoint returns it.
        let resp = json_get(
            &app,
            &format!("/api/attachments?entity_type=issue&entity_id={issue_id}"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let list = parse_json(resp).await;
        let arr = list.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"].as_i64().unwrap(), att_id);

        // Editing the description to drop the reference unlinks it.
        let resp = json_put(
            &app,
            &format!("/api/issues/{issue_id}"),
            serde_json::json!({ "description": "no more image" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = json_get(
            &app,
            &format!("/api/attachments?entity_type=issue&entity_id={issue_id}"),
        )
        .await;
        assert!(parse_json(resp).await.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn orphan_gc_collects_unlinked_and_keeps_linked() {
        // A dedicated store + db so the sweep sees exactly this app's data.
        let (store, _tmp) = test_attachment_store();
        let db = crate::db::open_memory().unwrap();
        let admin_id = {
            let conn = db.write().unwrap();
            conn.execute(
                "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot)
                 VALUES ('gc','gc@a','x','GC',1,0)",
                [],
            )
            .unwrap();
            conn.last_insert_rowid()
        };
        let app = with_attachment_layers_store(crate::api::router(db.clone(), &[]), store.clone())
            .layer(axum::Extension(crate::realtime::RealtimeHub::new()))
            .layer(axum::Extension(crate::config::AuthConfig {
                allow_signup: true,
                required: true,
                secure_cookies: false,
            }))
            .layer(axum::Extension(Some(AuthUser {
                id: admin_id,
                username: "gc".into(),
                display_name: "GC".into(),
                is_admin: true,
            })))
            .layer(axum::Extension(Some(crate::resolve_caller::ResolvedIdentity {
                user: AuthUser {
                    id: admin_id,
                    username: "gc".into(),
                    display_name: "GC".into(),
                    is_admin: true,
                },
                transport: crate::actor::Transport::Web,
            })));
        let (project_id2, _) = seed_project(&app).await;

        // Upload two: one linked to an issue, one left dangling.
        let issue = parse_json(
            json_post(
                &app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id2, "title": "t" }),
            )
            .await,
        )
        .await;
        let issue_id = issue["id"].as_i64().unwrap();

        let linked_id = parse_json(
            upload(
                &app,
                "keep.png",
                "image/png",
                &png_bytes(),
                Some(("issue", issue_id)),
            )
            .await,
        )
        .await["id"]
            .as_i64()
            .unwrap();
        let orphan_bytes = {
            let mut v = png_bytes();
            v.extend_from_slice(b"orphan-distinct");
            v
        };
        let orphan_id = parse_json(
            upload(&app, "drop.png", "image/png", &orphan_bytes, None).await,
        )
        .await["id"]
            .as_i64()
            .unwrap();

        // Sweep with a zero grace window: only the unlinked one is collected.
        let collected = crate::storage::sweep_orphans(&db, &store, -1).unwrap();
        assert_eq!(collected, 1);

        // Linked survives, orphan is gone.
        assert_eq!(
            json_get(&app, &format!("/api/attachments/{linked_id}"))
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            json_get(&app, &format!("/api/attachments/{orphan_id}"))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );

        std::fs::remove_dir_all(store.dir()).ok();
    }

    // ── Project files manager (LIF-418) ──────────────────────

    /// Seed a project with one issue and return `(project_id, issue_id)`.
    async fn seed_project_with_issue(app: &axum::Router) -> (i64, i64) {
        let (project_id, _) = seed_project(app).await;
        let issue = parse_json(
            json_post(
                app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id, "title": "Files target" }),
            )
            .await,
        )
        .await;
        (project_id, issue["id"].as_i64().unwrap())
    }

    #[tokio::test]
    async fn project_files_listing_returns_linked_uploads_with_totals() {
        let app = test_app();
        let (project_id, issue_id) = seed_project_with_issue(&app).await;
        upload(
            &app,
            "shot.png",
            "image/png",
            &png_bytes(),
            Some(("issue", issue_id)),
        )
        .await;
        upload(
            &app,
            "notes.txt",
            "text/plain",
            b"a plain text upload\n",
            Some(("issue", issue_id)),
        )
        .await;
        // Unlinked: belongs to no project, so it stays out of the listing.
        upload(&app, "stray.txt", "text/plain", b"nowhere\n", None).await;

        let body = parse_json(json_get(&app, &format!("/api/projects/{project_id}/attachments")).await)
            .await;
        assert_eq!(body["total_count"], 2);
        assert_eq!(body["has_more"], false);
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        let total_bytes: i64 = items
            .iter()
            .map(|item| item["size_bytes"].as_i64().unwrap())
            .sum();
        assert_eq!(body["total_bytes"].as_i64().unwrap(), total_bytes);

        let png = items
            .iter()
            .find(|item| item["filename"] == "shot.png")
            .expect("the linked image is listed");
        assert_eq!(png["mime_class"], "image");
        assert_eq!(png["uploader"], "test-admin");
        let entities = png["entities"].as_array().unwrap();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0]["entity_type"], "issue");
        assert_eq!(entities[0]["entity_id"].as_i64().unwrap(), issue_id);
        assert_eq!(entities[0]["identifier"], "TST-1");
        assert_eq!(entities[0]["title"], "Files target");
    }

    #[tokio::test]
    async fn project_files_listing_filters_sorts_and_pages() {
        let app = test_app();
        let (project_id, issue_id) = seed_project_with_issue(&app).await;
        upload(
            &app,
            "shot.png",
            "image/png",
            &png_bytes(),
            Some(("issue", issue_id)),
        )
        .await;
        upload(
            &app,
            "notes.txt",
            "text/plain",
            b"a plain text upload\n",
            Some(("issue", issue_id)),
        )
        .await;

        let text_only = parse_json(
            json_get(
                &app,
                &format!("/api/projects/{project_id}/attachments?mime_class=text"),
            )
            .await,
        )
        .await;
        assert_eq!(text_only["total_count"], 1);
        assert_eq!(text_only["items"][0]["filename"], "notes.txt");

        let by_uploader = parse_json(
            json_get(
                &app,
                &format!("/api/projects/{project_id}/attachments?uploader=test-admin"),
            )
            .await,
        )
        .await;
        assert_eq!(by_uploader["total_count"], 2);

        let paged = parse_json(
            json_get(
                &app,
                &format!("/api/projects/{project_id}/attachments?limit=1&sort=filename"),
            )
            .await,
        )
        .await;
        assert_eq!(paged["items"].as_array().unwrap().len(), 1);
        assert_eq!(paged["has_more"], true);
        assert_eq!(
            paged["items"][0]["filename"], "notes.txt",
            "filename sorts A to Z by default"
        );

        // A filter value that isn't a known class is a 400, not a silently
        // wider result set.
        let bad = json_get(
            &app,
            &format!("/api/projects/{project_id}/attachments?mime_class=hologram"),
        )
        .await;
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn project_orphans_lists_unlinked_uploads_with_a_countdown() {
        let app = test_app();
        let (project_id, issue_id) = seed_project_with_issue(&app).await;
        upload(
            &app,
            "kept.png",
            "image/png",
            &png_bytes(),
            Some(("issue", issue_id)),
        )
        .await;
        upload(&app, "abandoned.txt", "text/plain", b"draft that never landed\n", None).await;

        let body = parse_json(
            json_get(
                &app,
                &format!("/api/projects/{project_id}/attachments/orphans"),
            )
            .await,
        )
        .await;
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 1, "only the unlinked upload is pending sweep");
        assert_eq!(items[0]["filename"], "abandoned.txt");
        assert_eq!(items[0]["uploader"], "test-admin");
        assert_eq!(
            body["grace_seconds"].as_i64().unwrap(),
            crate::storage::ORPHAN_GRACE_SECONDS
        );
        assert!(
            items[0]["seconds_until_sweep"].as_i64().unwrap() > 23 * 60 * 60,
            "a fresh upload has most of the grace window left"
        );
        assert_eq!(
            body["total_bytes"].as_i64().unwrap(),
            items[0]["size_bytes"].as_i64().unwrap()
        );
    }

    /// A non-member must not be able to tell an existing project from a
    /// nonexistent one: both answer 403, never 404 and never an empty 200.
    #[tokio::test]
    async fn project_files_endpoints_hide_the_project_from_non_members() {
        let (db, _admin, _lead, _maintainer, viewer, non_member, project_id) =
            setup_membership_test();

        let stranger = app_as_user(db.clone(), &non_member);
        for uri in [
            format!("/api/projects/{project_id}/attachments"),
            format!("/api/projects/{project_id}/attachments/orphans"),
        ] {
            assert_eq!(
                json_get(&stranger, &uri).await.status(),
                StatusCode::FORBIDDEN,
                "{uri} must be forbidden for a non-member"
            );
        }
        // The same answer for a project id that doesn't exist at all.
        assert_eq!(
            json_get(&stranger, "/api/projects/999999/attachments")
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            json_get(&stranger, "/api/projects/999999/attachments/orphans")
                .await
                .status(),
            StatusCode::FORBIDDEN
        );

        // A viewer on the project reads it fine.
        let member = app_as_user(db, &viewer);
        assert_eq!(
            json_get(&member, &format!("/api/projects/{project_id}/attachments"))
                .await
                .status(),
            StatusCode::OK
        );
    }

    /// End to end: a text upload's contents reach the FTS index and come back
    /// out of `/api/search` as an attachment hit pointing at the issue it is
    /// attached to.
    #[tokio::test]
    async fn uploaded_text_is_searchable_by_its_contents() {
        let app = test_app();
        let (_project_id, issue_id) = seed_project_with_issue(&app).await;
        upload(
            &app,
            "server.log",
            "text/plain",
            b"thread panicked at gribblenaut::render\n",
            Some(("issue", issue_id)),
        )
        .await;

        let results = parse_json(json_get(&app, "/api/search?query=gribblenaut").await).await;
        let hits = results.as_array().unwrap();
        assert_eq!(hits.len(), 1, "got: {results}");
        assert_eq!(hits[0]["result_type"], "attachment");
        assert_eq!(hits[0]["title"], "server.log");
        assert_eq!(hits[0]["identifier"], "TST-1");

        // The filename is indexed too, without any extraction.
        let by_name = parse_json(json_get(&app, "/api/search?query=server").await).await;
        assert!(
            by_name
                .as_array()
                .unwrap()
                .iter()
                .any(|hit| hit["result_type"] == "attachment"),
            "got: {by_name}"
        );
    }
}

// ── LIF-418: media, thumbnails, alt text, links, previews ────
#[cfg(test)]
mod media_tests {
    use super::api_tests::upload;
    use crate::api::test_helpers::*;
    use crate::db::models::*;
    use crate::preview::fixtures::{build_sqlite, build_zip};
    use crate::storage::fixtures::png_image;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// An app wired to a caller-owned attachment store, so a test can inspect
    /// the files the handlers write. Mirrors `test_app` otherwise.
    fn app_with_store(
        db: crate::db::DbPool,
        store: crate::storage::AttachmentStore,
        user_id: i64,
        username: &str,
        display_name: &str,
    ) -> axum::Router {
        let user = AuthUser {
            id: user_id,
            username: username.into(),
            display_name: display_name.into(),
            is_admin: true,
        };
        with_attachment_layers_store(crate::api::router(db, &[]), store)
            .layer(axum::Extension(crate::realtime::RealtimeHub::new()))
            .layer(axum::Extension(crate::config::AuthConfig {
                allow_signup: true,
                required: true,
                secure_cookies: false,
            }))
            .layer(axum::Extension(Some(user.clone())))
            .layer(axum::Extension(Some(
                crate::resolve_caller::ResolvedIdentity {
                    user,
                    transport: crate::actor::Transport::Web,
                },
            )))
    }

    /// A tiny but structurally valid WebM header the sniffer accepts.
    fn webm_bytes() -> Vec<u8> {
        let mut v = vec![0x1A, 0x45, 0xDF, 0xA3];
        v.extend_from_slice(&[0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1F]);
        v.extend_from_slice(b"\x42\x82\x84webm");
        v.extend_from_slice(&[0; 64]);
        v
    }

    fn mp4_bytes() -> Vec<u8> {
        let mut v = vec![0, 0, 0, 0x20];
        v.extend_from_slice(b"ftypisom");
        v.extend_from_slice(b"\0\0\x02\0isomiso2avc1mp41");
        v.extend_from_slice(&[0; 128]);
        v
    }

    async fn body_bytes(resp: axum::response::Response) -> Vec<u8> {
        resp.into_body().collect().await.unwrap().to_bytes().to_vec()
    }

    fn header(resp: &axum::response::Response, name: &str) -> Option<String> {
        resp.headers()
            .get(name)
            .map(|v| v.to_str().unwrap().to_string())
    }

    async fn range_get(app: &axum::Router, uri: &str, range: &str) -> axum::response::Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(uri)
                    .header("range", range)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    // ── Media uploads ────────────────────────────────────────

    /// Video and audio are the point of LIF-418: they must upload, and they
    /// must come back playable in place rather than as a download prompt.
    #[tokio::test]
    async fn media_uploads_and_serves_inline() {
        let app = test_app();
        let mut ogg = Vec::from(*b"OggS");
        ogg.extend_from_slice(&[0; 64]);
        let mut mp3 = Vec::from(*b"ID3\x03\x00\x00");
        mp3.extend_from_slice(&[0; 64]);

        for (name, declared, bytes, expected) in [
            ("clip.mp4", "video/mp4", mp4_bytes(), "video/mp4"),
            ("clip.webm", "video/webm", webm_bytes(), "video/webm"),
            ("voice.webm", "audio/webm", webm_bytes(), "audio/webm"),
            ("tune.ogg", "audio/ogg", ogg, "audio/ogg"),
            ("tune.mp3", "audio/mpeg", mp3, "audio/mpeg"),
        ] {
            let resp = upload(&app, name, declared, &bytes, None).await;
            assert_eq!(resp.status(), StatusCode::OK, "upload {name}");
            let data = parse_json(resp).await;
            assert_eq!(data["mime"], expected, "{name}");
            let id = data["id"].as_i64().unwrap();

            let resp = json_get(&app, &format!("/api/attachments/{id}")).await;
            assert_eq!(resp.status(), StatusCode::OK);
            assert_eq!(header(&resp, "content-type").unwrap(), expected);
            assert!(
                header(&resp, "content-disposition")
                    .unwrap()
                    .starts_with("inline"),
                "{name} must play in place"
            );
            // The sandbox and nosniff guarantees are unchanged for media.
            assert_eq!(header(&resp, "x-content-type-options").unwrap(), "nosniff");
            let csp = header(&resp, "content-security-policy").unwrap();
            assert!(csp.contains("default-src 'none'") && csp.contains("sandbox"));
        }
    }

    // ── Range requests ───────────────────────────────────────

    #[tokio::test]
    async fn range_request_returns_partial_content() {
        let app = test_app();
        let bytes = mp4_bytes();
        let id = parse_json(upload(&app, "v.mp4", "video/mp4", &bytes, None).await).await["id"]
            .as_i64()
            .unwrap();
        let total = bytes.len();

        let resp = range_get(&app, &format!("/api/attachments/{id}"), "bytes=4-11").await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(header(&resp, "accept-ranges").unwrap(), "bytes");
        assert_eq!(
            header(&resp, "content-range").unwrap(),
            format!("bytes 4-11/{total}")
        );
        assert_eq!(header(&resp, "content-length").unwrap(), "8");
        assert_eq!(body_bytes(resp).await, b"ftypisom".to_vec());
    }

    #[tokio::test]
    async fn open_ended_and_suffix_ranges_are_served() {
        let app = test_app();
        let bytes = mp4_bytes();
        let id = parse_json(upload(&app, "v.mp4", "video/mp4", &bytes, None).await).await["id"]
            .as_i64()
            .unwrap();
        let total = bytes.len();

        // The "give me everything from here" probe.
        let resp = range_get(&app, &format!("/api/attachments/{id}"), "bytes=4-").await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            header(&resp, "content-range").unwrap(),
            format!("bytes 4-{}/{total}", total - 1)
        );
        assert_eq!(body_bytes(resp).await, bytes[4..].to_vec());

        // The "read the tail" probe an mp4 player uses to find its index.
        let resp = range_get(&app, &format!("/api/attachments/{id}"), "bytes=-16").await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(body_bytes(resp).await, bytes[total - 16..].to_vec());
    }

    #[tokio::test]
    async fn unsatisfiable_range_is_416_with_the_real_length() {
        let app = test_app();
        let bytes = mp4_bytes();
        let id = parse_json(upload(&app, "v.mp4", "video/mp4", &bytes, None).await).await["id"]
            .as_i64()
            .unwrap();

        let resp = range_get(&app, &format!("/api/attachments/{id}"), "bytes=99999-").await;
        assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(
            header(&resp, "content-range").unwrap(),
            format!("bytes */{}", bytes.len())
        );
        assert_eq!(header(&resp, "accept-ranges").unwrap(), "bytes");
    }

    /// Range support must not change what a plain GET does, and every
    /// response has to advertise the capability so a player bothers asking.
    #[tokio::test]
    async fn full_response_still_works_and_advertises_ranges() {
        let app = test_app();
        let bytes = mp4_bytes();
        let id = parse_json(upload(&app, "v.mp4", "video/mp4", &bytes, None).await).await["id"]
            .as_i64()
            .unwrap();

        let resp = json_get(&app, &format!("/api/attachments/{id}")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(header(&resp, "accept-ranges").unwrap(), "bytes");
        assert!(header(&resp, "content-range").is_none());
        assert_eq!(body_bytes(resp).await, bytes);

        // A range unit we do not implement is ignored, not rejected.
        let resp = range_get(&app, &format!("/api/attachments/{id}"), "frames=1-2").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_bytes(resp).await, bytes);
    }

    #[tokio::test]
    async fn range_request_still_enforces_the_read_gate() {
        let (db, _admin, lead, _maintainer, _viewer, non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db.clone(), &lead);
        let issue_id = parse_json(
            json_post(
                &lead_app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id, "title": "private clip" }),
            )
            .await,
        )
        .await["id"]
            .as_i64()
            .unwrap();
        let att_id = parse_json(
            upload(
                &lead_app,
                "v.mp4",
                "video/mp4",
                &mp4_bytes(),
                Some(("issue", issue_id)),
            )
            .await,
        )
        .await["id"]
            .as_i64()
            .unwrap();

        let non_member_app = app_as_user(db.clone(), &non_member);
        let resp = range_get(
            &non_member_app,
            &format!("/api/attachments/{att_id}"),
            "bytes=0-3",
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    // ── Dimensions + thumbnails ──────────────────────────────

    #[tokio::test]
    async fn upload_records_raster_dimensions() {
        let app = test_app();
        let data = parse_json(upload(&app, "big.png", "image/png", &png_image(800, 200), None).await)
            .await;
        assert_eq!(data["width"], 800);
        assert_eq!(data["height"], 200);
        assert_eq!(data["has_thumbnail"], true);
        assert_eq!(data["alt_text"], serde_json::Value::Null);

        // A non-raster upload records no dimensions and offers no thumbnail.
        let data = parse_json(upload(&app, "n.txt", "text/plain", b"hello\n", None).await).await;
        assert_eq!(data["width"], serde_json::Value::Null);
        assert_eq!(data["height"], serde_json::Value::Null);
        assert_eq!(data["has_thumbnail"], false);
    }

    #[tokio::test]
    async fn thumbnail_endpoint_serves_a_downscaled_webp() {
        let app = test_app();
        let id = parse_json(upload(&app, "big.png", "image/png", &png_image(1200, 600), None).await)
            .await["id"]
            .as_i64()
            .unwrap();

        let resp = json_get(&app, &format!("/api/attachments/{id}/thumbnail")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(header(&resp, "content-type").unwrap(), "image/webp");
        assert_eq!(
            header(&resp, "cache-control").unwrap(),
            "private, max-age=31536000, immutable"
        );
        assert_eq!(header(&resp, "x-content-type-options").unwrap(), "nosniff");

        let thumb = body_bytes(resp).await;
        assert_eq!(
            crate::storage::image_dimensions(&thumb),
            Some((480, 240)),
            "the long edge is capped and the aspect ratio held"
        );
    }

    #[tokio::test]
    async fn thumbnail_is_404_when_there_is_nothing_to_shrink() {
        let app = test_app();
        // Already inside the thumbnail box.
        let small = parse_json(upload(&app, "s.png", "image/png", &png_image(64, 64), None).await)
            .await["id"]
            .as_i64()
            .unwrap();
        assert_eq!(
            json_get(&app, &format!("/api/attachments/{small}/thumbnail"))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );

        // Not a raster image at all.
        let text = parse_json(upload(&app, "n.txt", "text/plain", b"hello\n", None).await).await
            ["id"]
            .as_i64()
            .unwrap();
        assert_eq!(
            json_get(&app, &format!("/api/attachments/{text}/thumbnail"))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }

    /// Attachments uploaded before LIF-418 have no cached thumbnail on disk.
    /// The endpoint has to build one on demand, or every historical image
    /// would be permanently thumbnail-less.
    #[tokio::test]
    async fn thumbnail_is_generated_lazily_for_a_pre_existing_upload() {
        let (store, _tmp) = test_attachment_store();
        let db = crate::db::open_memory().unwrap();
        let admin_id = {
            let conn = db.write().unwrap();
            conn.execute(
                "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot)
                 VALUES ('lazy','lazy@a','x','Lazy',1,0)",
                [],
            )
            .unwrap();
            conn.last_insert_rowid()
        };
        let app = app_with_store(db.clone(), store.clone(), admin_id, "lazy", "Lazy");

        let id = parse_json(upload(&app, "big.png", "image/png", &png_image(900, 900), None).await)
            .await["id"]
            .as_i64()
            .unwrap();
        let sha: String = {
            let conn = db.read().unwrap();
            conn.query_row(
                "SELECT sha256 FROM attachments WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
        };

        // Simulate the pre-LIF-418 state: the blob exists, the derivative
        // does not.
        store.delete_thumb(&sha).unwrap();
        assert!(store.read_thumb(&sha).unwrap().is_none());

        let resp = json_get(&app, &format!("/api/attachments/{id}/thumbnail")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            store.read_thumb(&sha).unwrap().is_some(),
            "the generated thumbnail is cached for next time"
        );
    }

    #[tokio::test]
    async fn thumbnail_denies_a_non_member() {
        let (db, _admin, lead, _maintainer, _viewer, non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db.clone(), &lead);
        let issue_id = parse_json(
            json_post(
                &lead_app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id, "title": "private" }),
            )
            .await,
        )
        .await["id"]
            .as_i64()
            .unwrap();
        let att_id = parse_json(
            upload(
                &lead_app,
                "big.png",
                "image/png",
                &png_image(800, 800),
                Some(("issue", issue_id)),
            )
            .await,
        )
        .await["id"]
            .as_i64()
            .unwrap();

        let non_member_app = app_as_user(db.clone(), &non_member);
        assert_eq!(
            json_get(
                &non_member_app,
                &format!("/api/attachments/{att_id}/thumbnail")
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            json_get(&lead_app, &format!("/api/attachments/{att_id}/thumbnail"))
                .await
                .status(),
            StatusCode::OK
        );
    }

    // ── Alt text ─────────────────────────────────────────────

    #[tokio::test]
    async fn alt_text_is_set_cleared_and_echoed_everywhere() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;
        let issue_id = parse_json(
            json_post(
                &app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id, "title": "described" }),
            )
            .await,
        )
        .await["id"]
            .as_i64()
            .unwrap();
        let id = parse_json(
            upload(
                &app,
                "chart.png",
                "image/png",
                &png_image(600, 600),
                Some(("issue", issue_id)),
            )
            .await,
        )
        .await["id"]
            .as_i64()
            .unwrap();

        let resp = json_patch(
            &app,
            &format!("/api/attachments/{id}"),
            serde_json::json!({ "alt_text": "  A bar chart of weekly signups  " }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        assert_eq!(data["alt_text"], "A bar chart of weekly signups");

        // It rides along on the entity listing, which is what renders the
        // image in the UI.
        let list = parse_json(
            json_get(
                &app,
                &format!("/api/attachments?entity_type=issue&entity_id={issue_id}"),
            )
            .await,
        )
        .await;
        assert_eq!(list[0]["alt_text"], "A bar chart of weekly signups");
        assert_eq!(list[0]["width"], 600);
        assert_eq!(list[0]["has_thumbnail"], true);

        // Explicit null clears it; whitespace-only is the same as clearing,
        // so there is exactly one representation of "undescribed".
        let data = parse_json(
            json_patch(
                &app,
                &format!("/api/attachments/{id}"),
                serde_json::json!({ "alt_text": "   " }),
            )
            .await,
        )
        .await;
        assert_eq!(data["alt_text"], serde_json::Value::Null);

        let data = parse_json(
            json_patch(
                &app,
                &format!("/api/attachments/{id}"),
                serde_json::json!({ "alt_text": "back again" }),
            )
            .await,
        )
        .await;
        assert_eq!(data["alt_text"], "back again");
        let data = parse_json(
            json_patch(
                &app,
                &format!("/api/attachments/{id}"),
                serde_json::json!({ "alt_text": null }),
            )
            .await,
        )
        .await;
        assert_eq!(data["alt_text"], serde_json::Value::Null);

        // An empty patch is a no-op rather than a 400.
        let resp = json_patch(&app, &format!("/api/attachments/{id}"), serde_json::json!({})).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn alt_text_is_length_capped() {
        let app = test_app();
        let id = parse_json(upload(&app, "a.png", "image/png", &png_image(10, 10), None).await)
            .await["id"]
            .as_i64()
            .unwrap();
        let resp = json_patch(
            &app,
            &format!("/api/attachments/{id}"),
            serde_json::json!({ "alt_text": "x".repeat(1001) }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    /// Describing a file is an edit of it, so it costs what deleting costs:
    /// a project Viewer who did not upload it cannot rewrite the alt text.
    #[tokio::test]
    async fn alt_text_mirrors_the_delete_gate() {
        let (db, _admin, lead, maintainer, viewer, non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db.clone(), &lead);
        let issue_id = parse_json(
            json_post(
                &lead_app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id, "title": "t" }),
            )
            .await,
        )
        .await["id"]
            .as_i64()
            .unwrap();

        let maintainer_app = app_as_user(db.clone(), &maintainer);
        let att_id = parse_json(
            upload(
                &maintainer_app,
                "m.png",
                "image/png",
                &png_image(40, 40),
                Some(("issue", issue_id)),
            )
            .await,
        )
        .await["id"]
            .as_i64()
            .unwrap();

        let body = serde_json::json!({ "alt_text": "mine now" });
        for app in [
            app_as_user(db.clone(), &viewer),
            app_as_user(db.clone(), &non_member),
        ] {
            assert_eq!(
                json_patch(&app, &format!("/api/attachments/{att_id}"), body.clone())
                    .await
                    .status(),
                StatusCode::FORBIDDEN
            );
        }
        // The uploader can.
        assert_eq!(
            json_patch(
                &maintainer_app,
                &format!("/api/attachments/{att_id}"),
                body.clone()
            )
            .await
            .status(),
            StatusCode::OK
        );
        // So can the project lead, who is a Maintainer on it.
        assert_eq!(
            json_patch(&lead_app, &format!("/api/attachments/{att_id}"), body)
                .await
                .status(),
            StatusCode::OK
        );
    }

    // ── Where-used + duplicates ──────────────────────────────

    #[tokio::test]
    async fn links_report_every_place_a_file_is_used() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;
        let issue_id = parse_json(
            json_post(
                &app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id, "title": "Broken export" }),
            )
            .await,
        )
        .await["id"]
            .as_i64()
            .unwrap();
        let page_id = parse_json(
            json_post(
                &app,
                "/api/pages",
                serde_json::json!({ "project_id": project_id, "title": "Runbook" }),
            )
            .await,
        )
        .await["id"]
            .as_i64()
            .unwrap();

        let bytes = png_image(20, 20);
        let id = parse_json(
            upload(&app, "shot.png", "image/png", &bytes, Some(("issue", issue_id))).await,
        )
        .await["id"]
            .as_i64()
            .unwrap();

        // Reference the same attachment from a page and a comment.
        json_put(
            &app,
            &format!("/api/pages/{page_id}"),
            serde_json::json!({ "content": format!("![shot](/api/attachments/{id})") }),
        )
        .await;
        json_post(
            &app,
            &format!("/api/issues/{issue_id}/comments"),
            serde_json::json!({
                "content": format!("see this\nand more: ![shot](/api/attachments/{id})"),
            }),
        )
        .await;

        let links = parse_json(json_get(&app, &format!("/api/attachments/{id}/links")).await).await;
        let entities = links["entities"].as_array().unwrap();
        assert_eq!(entities.len(), 3, "issue, page and comment: {entities:#?}");

        let issue = entities
            .iter()
            .find(|e| e["entity_type"] == "issue")
            .unwrap();
        assert_eq!(issue["entity_id"], issue_id);
        assert_eq!(issue["title"], "Broken export");
        assert!(issue["identifier"].as_str().unwrap().contains('-'));

        let page = entities.iter().find(|e| e["entity_type"] == "page").unwrap();
        assert_eq!(page["title"], "Runbook");
        assert!(page["identifier"].as_str().unwrap().contains("-DOC-"));

        let comment = entities
            .iter()
            .find(|e| e["entity_type"] == "comment")
            .unwrap();
        assert_eq!(
            comment["identifier"],
            serde_json::Value::Null,
            "a comment is reached through its parent, not by identifier"
        );
        assert_eq!(comment["title"], "see this");

        assert!(links["duplicates"].as_array().unwrap().is_empty());
    }

    /// Two uploads of identical bytes are two rows over one blob. The point
    /// of the duplicates list is telling the caller the file is already here.
    #[tokio::test]
    async fn duplicates_surface_other_rows_over_the_same_bytes() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;
        let issue_id = parse_json(
            json_post(
                &app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id, "title": "Original home" }),
            )
            .await,
        )
        .await["id"]
            .as_i64()
            .unwrap();

        let bytes = png_image(24, 24);
        let first = parse_json(
            upload(&app, "a.png", "image/png", &bytes, Some(("issue", issue_id))).await,
        )
        .await["id"]
            .as_i64()
            .unwrap();
        let second =
            parse_json(upload(&app, "again.png", "image/png", &bytes, None).await).await["id"]
                .as_i64()
                .unwrap();
        assert_ne!(first, second);

        let links =
            parse_json(json_get(&app, &format!("/api/attachments/{second}/links")).await).await;
        assert!(links["entities"].as_array().unwrap().is_empty());
        let dupes = links["duplicates"].as_array().unwrap();
        assert_eq!(dupes.len(), 1);
        assert_eq!(dupes[0]["attachment_id"], first);
        assert_eq!(dupes[0]["filename"], "a.png");
        assert_eq!(dupes[0]["entities"][0]["title"], "Original home");
    }

    /// The duplicate list must never become a read oracle: a caller who
    /// uploads a file already used in a project they cannot see learns that
    /// a copy exists, and nothing about where.
    #[tokio::test]
    async fn duplicate_usages_in_invisible_projects_are_withheld() {
        let (db, _admin, lead, _maintainer, _viewer, non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db.clone(), &lead);
        let issue_id = parse_json(
            json_post(
                &lead_app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id, "title": "Confidential title" }),
            )
            .await,
        )
        .await["id"]
            .as_i64()
            .unwrap();

        let bytes = png_image(18, 18);
        let hidden = parse_json(
            upload(
                &lead_app,
                "secret.png",
                "image/png",
                &bytes,
                Some(("issue", issue_id)),
            )
            .await,
        )
        .await["id"]
            .as_i64()
            .unwrap();

        // The outsider uploads the very same bytes.
        let outsider_app = app_as_user(db.clone(), &non_member);
        let mine = parse_json(upload(&outsider_app, "mine.png", "image/png", &bytes, None).await)
            .await["id"]
            .as_i64()
            .unwrap();

        let links = parse_json(
            json_get(&outsider_app, &format!("/api/attachments/{mine}/links")).await,
        )
        .await;
        let dupes = links["duplicates"].as_array().unwrap();
        assert_eq!(dupes.len(), 1);
        assert_eq!(dupes[0]["attachment_id"], hidden);
        assert!(
            dupes[0]["entities"].as_array().unwrap().is_empty(),
            "the outsider must not learn the title of an issue they cannot read"
        );
        let rendered = serde_json::to_string(&links).unwrap();
        assert!(!rendered.contains("Confidential title"));

        // And the linked attachment itself is still unreadable to them.
        assert_eq!(
            json_get(&outsider_app, &format!("/api/attachments/{hidden}/links"))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }

    // ── Structured previews ──────────────────────────────────

    #[tokio::test]
    async fn zip_preview_lists_the_archive_contents() {
        let app = test_app();
        let zip = build_zip(&[("readme.txt", b"hi"), ("logs/app.log", b"line one\n")]);
        let id = parse_json(upload(&app, "bundle.zip", "application/zip", &zip, None).await).await
            ["id"]
            .as_i64()
            .unwrap();

        let preview =
            parse_json(json_get(&app, &format!("/api/attachments/{id}/preview")).await).await;
        assert_eq!(preview["kind"], "zip");
        assert_eq!(preview["total_entries"], 2);
        assert_eq!(preview["truncated"], false);
        assert_eq!(preview["entries"][0]["name"], "readme.txt");
        assert_eq!(preview["entries"][0]["size"], 2);
        assert_eq!(preview["entries"][1]["name"], "logs/app.log");
        assert_eq!(preview["entries"][1]["compressed"], 9);
    }

    #[tokio::test]
    async fn sqlite_preview_lists_tables_and_row_counts() {
        let app = test_app();
        let db_bytes = build_sqlite();
        let id = parse_json(
            upload(
                &app,
                "repro.sqlite3",
                "application/vnd.sqlite3",
                &db_bytes,
                None,
            )
            .await,
        )
        .await["id"]
            .as_i64()
            .unwrap();

        let preview =
            parse_json(json_get(&app, &format!("/api/attachments/{id}/preview")).await).await;
        assert_eq!(preview["kind"], "sqlite");
        assert_eq!(
            preview["tables"],
            serde_json::json!([
                { "name": "empty_shelf", "rows": 0 },
                { "name": "widgets", "rows": 3 },
            ])
        );
    }

    #[tokio::test]
    async fn preview_of_an_ordinary_file_is_kind_none() {
        let app = test_app();
        for (name, mime, bytes) in [
            ("a.png", "image/png", png_image(8, 8)),
            ("n.txt", "text/plain", b"just notes\n".to_vec()),
            ("v.mp4", "video/mp4", mp4_bytes()),
        ] {
            let id = parse_json(upload(&app, name, mime, &bytes, None).await).await["id"]
                .as_i64()
                .unwrap();
            let preview =
                parse_json(json_get(&app, &format!("/api/attachments/{id}/preview")).await).await;
            assert_eq!(preview, serde_json::json!({ "kind": "none" }), "{name}");
        }
    }

    #[tokio::test]
    async fn preview_denies_a_non_member() {
        let (db, _admin, lead, _maintainer, _viewer, non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db.clone(), &lead);
        let issue_id = parse_json(
            json_post(
                &lead_app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id, "title": "t" }),
            )
            .await,
        )
        .await["id"]
            .as_i64()
            .unwrap();
        let zip = build_zip(&[("secret.txt", b"shh")]);
        let att_id = parse_json(
            upload(
                &lead_app,
                "b.zip",
                "application/zip",
                &zip,
                Some(("issue", issue_id)),
            )
            .await,
        )
        .await["id"]
            .as_i64()
            .unwrap();

        let non_member_app = app_as_user(db.clone(), &non_member);
        assert_eq!(
            json_get(
                &non_member_app,
                &format!("/api/attachments/{att_id}/preview")
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            json_get(&lead_app, &format!("/api/attachments/{att_id}/preview"))
                .await
                .status(),
            StatusCode::OK
        );
    }
}

// ── LIF-267: session-cookie fallback for browser <img> attachment GETs ──────
//
// These drive the REAL `require_api_key` middleware (not the `app_as_user`
// Extension-injection shortcut) so the cookie path is genuinely exercised end
// to end through the production router. A browser-native `<img>` can't attach
// an Authorization header, so the middleware must accept the `lific_token`
// session cookie — but ONLY on `GET /api/attachments/{id}`.
#[cfg(test)]
mod cookie_fallback_tests {
    use crate::api::test_helpers::*;
    use crate::db::models::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// Minimal PNG (magic bytes + filler) the sniffer accepts.
    fn png_bytes() -> Vec<u8> {
        let mut v = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        v.extend_from_slice(b"cookie-fallback pixel data");
        v
    }

    /// Build a router that wraps the production `api::router` in the real
    /// `require_api_key` middleware, plus the attachment layers. Returns the
    /// app and the shared DbPool.
    fn real_middleware_app(db: crate::db::DbPool) -> axum::Router {
        let auth_state = crate::auth::AuthState {
            db: db.clone(),
            manager: crate::auth::create_key_manager().unwrap(),
            public_url: "https://example.com".into(),
            required: true,
        };
        with_attachment_layers(crate::api::router(db, &[]))
            .layer(axum::Extension(crate::realtime::RealtimeHub::new()))
            .layer(axum::Extension(crate::config::AuthConfig {
                allow_signup: true,
                required: true,
                secure_cookies: false,
            }))
            .layer(axum::middleware::from_fn_with_state(
                auth_state,
                crate::auth::require_api_key,
            ))
    }

    /// Create a user and a live session, returning (user_id, session token).
    fn user_with_session(db: &crate::db::DbPool, username: &str) -> (i64, String) {
        let conn = db.write().unwrap();
        let user = crate::db::queries::users::create_user(
            &conn,
            &CreateUser {
                username: username.into(),
                email: format!("{username}@test.com"),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap();
        let session = crate::db::queries::users::create_session(&conn, user.id, None).unwrap();
        (user.id, session.token)
    }

    /// Upload a PNG via the header-authed session path and return its id. Also
    /// proves the ordinary header path still works end to end.
    async fn upload_png(app: &axum::Router, session: &str) -> i64 {
        upload_png_bytes(app, session, &png_bytes()).await
    }

    async fn upload_png_bytes(app: &axum::Router, session: &str, bytes: &[u8]) -> i64 {
        const BOUNDARY: &str = "----lifictestboundary267";
        let mut body = Vec::new();
        let push = |b: &mut Vec<u8>, s: &str| b.extend_from_slice(s.as_bytes());
        push(&mut body, &format!("--{BOUNDARY}\r\n"));
        push(
            &mut body,
            "Content-Disposition: form-data; name=\"file\"; filename=\"shot.png\"\r\n",
        );
        push(&mut body, "Content-Type: image/png\r\n\r\n");
        body.extend_from_slice(bytes);
        push(&mut body, "\r\n");
        push(&mut body, &format!("--{BOUNDARY}--\r\n"));

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/attachments")
                    .header("authorization", format!("Bearer {session}"))
                    .header(
                        "content-type",
                        format!("multipart/form-data; boundary={BOUNDARY}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "upload should succeed");
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        val["id"].as_i64().unwrap()
    }

    // 1) Cookie-authed GET on the download route returns 200 with the bytes.
    #[tokio::test]
    async fn cookie_authed_download_succeeds() {
        let db = crate::db::open_memory().unwrap();
        let app = real_middleware_app(db.clone());
        let (_uid, session) = user_with_session(&db, "cookieuser");
        let att_id = upload_png(&app, &session).await;

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/attachments/{att_id}"))
                    // No Authorization header — only the browser session cookie.
                    .header("cookie", format!("lific_token={session}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get("content-type").unwrap(), "image/png");
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(bytes.as_ref(), png_bytes().as_slice());
    }

    // 1b) LIF-418: the thumbnail is loaded by an `<img>` too, so the same
    //     cookie fallback has to reach it or every thumbnail in the UI 401s.
    #[tokio::test]
    async fn cookie_authed_thumbnail_succeeds() {
        let db = crate::db::open_memory().unwrap();
        let app = real_middleware_app(db.clone());
        let (_uid, session) = user_with_session(&db, "thumbuser");
        let att_id = upload_png_bytes(
            &app,
            &session,
            &crate::storage::fixtures::png_image(900, 300),
        )
        .await;

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/attachments/{att_id}/thumbnail"))
                    .header("cookie", format!("lific_token={session}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get("content-type").unwrap(), "image/webp");
    }

    // 1c) The two XHR-only derived routes stay header-only.
    #[tokio::test]
    async fn cookie_authed_links_and_preview_are_unauthorized() {
        let db = crate::db::open_memory().unwrap();
        let app = real_middleware_app(db.clone());
        let (_uid, session) = user_with_session(&db, "derivuser");
        let att_id = upload_png(&app, &session).await;

        for suffix in ["links", "preview"] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!("/api/attachments/{att_id}/{suffix}"))
                        .header("cookie", format!("lific_token={session}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{suffix}");
        }
    }

    // 2) Garbage cookie value → 401.
    #[tokio::test]
    async fn garbage_cookie_download_is_unauthorized() {
        let db = crate::db::open_memory().unwrap();
        let app = real_middleware_app(db.clone());
        let (_uid, session) = user_with_session(&db, "garbageuser");
        let att_id = upload_png(&app, &session).await;

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/attachments/{att_id}"))
                    // A well-formed-looking but invalid session token.
                    .header("cookie", "lific_token=lific_sess_not_a_real_token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // 3) Cookie carrying an API key (valid key, wrong prefix) → 401.
    #[tokio::test]
    async fn api_key_in_cookie_download_is_unauthorized() {
        let db = crate::db::open_memory().unwrap();
        let app = real_middleware_app(db.clone());
        let (_uid, session) = user_with_session(&db, "apikeyuser");
        let att_id = upload_png(&app, &session).await;

        // A genuinely valid API key — must still be refused via the cookie,
        // because the cookie path accepts ONLY session tokens.
        let manager = crate::auth::create_key_manager().unwrap();
        let key = crate::auth::create_api_key(&db, &manager, "cookie-key", None).unwrap();

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/attachments/{att_id}"))
                    .header("cookie", format!("lific_token={key}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // 4) Cookie-authed DELETE → 401 (method not GET; mutations stay header-only).
    #[tokio::test]
    async fn cookie_authed_delete_is_unauthorized() {
        let db = crate::db::open_memory().unwrap();
        let app = real_middleware_app(db.clone());
        let (_uid, session) = user_with_session(&db, "deleteuser");
        let att_id = upload_png(&app, &session).await;

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/attachments/{att_id}"))
                    .header("cookie", format!("lific_token={session}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // 5) Cookie-authed GET on the list route → 401 (path is not the download
    //    route; the list endpoint stays header-only).
    #[tokio::test]
    async fn cookie_authed_list_route_is_unauthorized() {
        let db = crate::db::open_memory().unwrap();
        let app = real_middleware_app(db.clone());
        let (_uid, session) = user_with_session(&db, "listuser");

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/attachments?entity_type=issue&entity_id=1")
                    .header("cookie", format!("lific_token={session}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // 6) Cookie-authed GET on an unrelated route → 401.
    #[tokio::test]
    async fn cookie_authed_unrelated_route_is_unauthorized() {
        let db = crate::db::open_memory().unwrap();
        let app = real_middleware_app(db.clone());
        let (_uid, session) = user_with_session(&db, "otheruser");

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/projects")
                    .header("cookie", format!("lific_token={session}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
