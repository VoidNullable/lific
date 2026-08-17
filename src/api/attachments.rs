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

    let (attachment, event) = with_write(&db, |conn| {
        let att = q::create_attachment(conn, &sha, &filename, &mime, size, Some(user.id))?;
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
fn resolve_entity_project(
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
pub(super) async fn download_attachment(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Extension(store): Extension<AttachmentStore>,
    Path(id): Path<i64>,
) -> Result<Response, LificError> {
    let attachment = with_read(&db, |conn| q::get_attachment(conn, id))?;

    // Authorize: the caller must be able to view SOME project this attachment
    // is linked into (Viewer), or be the uploader / an admin for a still-
    // unlinked attachment.
    authorize_read(&db, &identity, &attachment)?;

    let bytes = store.read(&attachment.sha256)?;
    let inline_safe = storage::is_inline_safe_mime(&attachment.mime);

    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, &attachment.mime)
        .header(header::CONTENT_LENGTH, bytes.len())
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
        );

    // Force download for anything that isn't a plain raster image. Either way
    // the filename is offered for the "Save as" dialog.
    let disposition = if inline_safe {
        format!("inline; filename=\"{}\"", header_safe(&attachment.filename))
    } else {
        format!(
            "attachment; filename=\"{}\"",
            header_safe(&attachment.filename)
        )
    };
    builder = builder.header(header::CONTENT_DISPOSITION, disposition);

    builder
        .body(Body::from(bytes))
        .map_err(|e| LificError::Internal(format!("build response: {e}")))
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

/// Return the invalidation event for one attachment link. Comment links refresh
/// their parent issue or project page. Missing entities are ignored so an old
/// dangling link does not prevent attachment deletion.
fn linked_entity_event(
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
fn authorize_link(
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
fn authorize_read(
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
fn sanitize_filename(name: &str) -> String {
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

    async fn upload(
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
        const BOUNDARY: &str = "----lifictestboundary267";
        let mut body = Vec::new();
        let push = |b: &mut Vec<u8>, s: &str| b.extend_from_slice(s.as_bytes());
        push(&mut body, &format!("--{BOUNDARY}\r\n"));
        push(
            &mut body,
            "Content-Disposition: form-data; name=\"file\"; filename=\"shot.png\"\r\n",
        );
        push(&mut body, "Content-Type: image/png\r\n\r\n");
        body.extend_from_slice(&png_bytes());
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
