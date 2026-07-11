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
//!   issue/page/comment). A per-user rate limit caps abuse.
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

use super::{with_read, with_write};

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
    Extension(auth_user): Extension<Option<AuthUser>>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(store): Extension<AttachmentStore>,
    Extension(config): Extension<AttachmentConfig>,
    Extension(limiter): Extension<Arc<AttachmentUploadLimiter>>,
    mut multipart: Multipart,
) -> Result<Response, LificError> {
    let user = auth_user
        .clone()
        .ok_or_else(|| LificError::Forbidden("authentication required to upload".into()))?;

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
    Extension(auth_user): Extension<Option<AuthUser>>,
    Query(query): Query<ListForEntityQuery>,
) -> Result<axum::Json<Vec<Attachment>>, LificError> {
    let entity: AttachmentEntity = query.entity_type.parse().map_err(LificError::BadRequest)?;

    // The entity's owning project gates the read (Viewer). Workspace-level
    // pages (no project) fall back to workspace-admin.
    let project_id = resolve_entity_project(&db, entity, query.entity_id)?;
    match project_id {
        Some(pid) => authz::require_role(&db, &auth_user, pid, Role::Viewer)?,
        None => authz::require_workspace_admin(&db, &auth_user)?,
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

/// `GET /api/attachments/{id}` — stream the bytes with the correct
/// `Content-Type`. Non-image types get `Content-Disposition: attachment` and
/// `X-Content-Type-Options: nosniff` so a browser never renders them inline
/// (defense against a malicious "image" that's actually HTML). Content-
/// addressed, so the response is immutable-cacheable forever.
pub(super) async fn download_attachment(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Extension(store): Extension<AttachmentStore>,
    Path(id): Path<i64>,
) -> Result<Response, LificError> {
    let attachment = with_read(&db, |conn| q::get_attachment(conn, id))?;

    // Authorize: the caller must be able to view SOME project this attachment
    // is linked into (Viewer), or be the uploader / an admin for a still-
    // unlinked attachment.
    authorize_read(&db, &auth_user, &attachment)?;

    let bytes = store.read(&attachment.sha256)?;
    let is_image = storage::is_image_mime(&attachment.mime);

    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, &attachment.mime)
        .header(header::CONTENT_LENGTH, bytes.len())
        // Content-addressed: the same id always returns the same bytes.
        .header(
            header::CACHE_CONTROL,
            "private, max-age=31536000, immutable",
        )
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");

    // Force download for non-images; inline for images. Either way the
    // filename is offered for the "Save as" dialog.
    let disposition = if is_image {
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
    Extension(auth_user): Extension<Option<AuthUser>>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(store): Extension<AttachmentStore>,
    Path(id): Path<i64>,
) -> Result<axum::Json<serde_json::Value>, LificError> {
    let user = auth_user
        .clone()
        .ok_or_else(|| LificError::Forbidden("authentication required".into()))?;
    let attachment = with_read(&db, |conn| q::get_attachment(conn, id))?;

    authorize_delete(&db, &auth_user, &user, &attachment)?;

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

/// Re-scan an entity's markdown body for `/api/attachments/{id}` references
/// and reconcile the link table to match (LIF-262 "re-scan on save"). Called
/// from the issue/page/comment create+update handlers inside their write txn.
/// The entity's own text is the source of truth for which attachments it uses;
/// this makes the join table agree.
pub(crate) fn sync_links(
    conn: &rusqlite::Connection,
    entity: AttachmentEntity,
    entity_id: i64,
    markdown: &str,
) -> Result<(), LificError> {
    let ids = q::parse_referenced_ids(markdown);
    q::sync_entity_links(conn, entity, entity_id, &ids)
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

/// Read gate: Viewer on any owning project, or uploader/admin for an unlinked
/// attachment. When enforcement is off, `require_role(.., Viewer)` is an
/// unconditional allow (legacy mode), so this reduces to today's open read
/// behavior — matching every other GET while the flag is off.
fn authorize_read(
    db: &DbPool,
    auth_user: &Option<AuthUser>,
    attachment: &Attachment,
) -> Result<(), LificError> {
    let project_ids = with_read(db, |conn| owning_project_ids(conn, attachment.id))?;

    if project_ids.is_empty() {
        // Unlinked: only the uploader or an admin can read it. (When
        // enforcement is off we still restrict unlinked reads to the uploader
        // to avoid an enumeration hole on freshly-uploaded blobs.)
        match auth_user {
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
            match authz::require_role(db, auth_user, pid, Role::Viewer) {
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
    auth_user: &Option<AuthUser>,
    user: &AuthUser,
    attachment: &Attachment,
) -> Result<(), LificError> {
    if user.is_admin || Some(user.id) == attachment.uploader_id {
        return Ok(());
    }
    let project_ids = with_read(db, |conn| owning_project_ids(conn, attachment.id))?;
    for pid in project_ids {
        if authz::require_role(db, auth_user, pid, Role::Maintainer).is_ok() {
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
            .unwrap()
            .unwrap();
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
        let app = crate::api::router(db, &[])
            .layer(Extension(crate::realtime::RealtimeHub::new()))
            .layer(Extension(crate::storage::AttachmentStore::new(
                std::env::temp_dir().join(format!("lific_sz_{}", std::process::id())),
            )))
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
        let store = test_attachment_store();
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
        let key = crate::auth::create_api_key(&db, &manager, "cookie-key").unwrap();

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
