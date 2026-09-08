//! LIF-467: whole-project archives over HTTP.
//!
//! An archive is a project's entire history, tombstones and audit log
//! included, so only a live browser session reaches these routes: never an API
//! key, operator key, OAuth token, bot, or the first-admin identity an
//! authentication-disabled instance hands a credential-less request. The
//! session is re-read at every decision point, including inside the
//! transaction that reads the history or writes the project.
//!
//! Export needs Lead or instance admin. Import needs instance admin in both
//! authorization modes, because it creates a project and grants a lead
//! membership. Limits come from [`Limits::WEB`] and are published by
//! `GET /api/project-archives`.

use std::path::PathBuf;
use std::time::Duration;

use axum::Extension;
use axum::extract::{Multipart, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use rusqlite::Connection;
use tokio::io::AsyncWriteExt;

use crate::authz;
use crate::db::models::{Role, User};
use crate::db::{DbPool, queries};
use crate::error::LificError;
use crate::project_archive::{self, Limits};
use crate::realtime::{RealtimeEvent, RealtimeHub};
use crate::storage::AttachmentStore;

use super::export::{PreparedExport, blocking_export, stream_response};
use super::with_read;

/// The only accepted multipart field. Anything else, including a second copy
/// of this one, is refused rather than partially honored.
const ARCHIVE_FIELD: &str = "archive";

/// A slot is held for the whole upload, so neither bound can be open-ended.
const UPLOAD_MAX_DURATION: Duration = Duration::from_secs(120);
const UPLOAD_IDLE_TIMEOUT: Duration = Duration::from_secs(20);

/// Transport ceiling for the upload route, sitting above the real limit plus
/// multipart framing so the envelope never rejects a legal archive.
pub(super) const ARCHIVE_UPLOAD_BODY_LIMIT: usize = 128 * 1024 * 1024 + 1024 * 1024;

#[cfg(test)]
tokio::task_local! {
    /// Test-only profile, so the refusal paths can be proven with kilobytes
    /// instead of a 128 MiB fixture. Task-scoped, not global, so parallel
    /// tests cannot see each other's.
    static TEST_WEB_LIMITS: Limits;
}

/// The profile this request runs under.
fn web_limits() -> Limits {
    #[cfg(test)]
    if let Ok(limits) = TEST_WEB_LIMITS.try_with(|limits| *limits) {
        return limits;
    }
    Limits::WEB
}

fn denied() -> LificError {
    LificError::Forbidden("this action requires a signed-in browser session".into())
}

fn oversize() -> LificError {
    LificError::PayloadTooLarge(format!(
        "a project archive upload may not exceed {} bytes",
        web_limits().max_compressed
    ))
}

fn not_admin() -> LificError {
    LificError::Forbidden("only an admin can import a project archive".into())
}

/// A caller that presented a live browser session.
struct SessionCaller {
    /// The session's own user. Immutable for the rest of the request: every
    /// later check re-reads the database and compares against this id.
    user_id: i64,
    token: String,
    /// Instance admin as of the read that produced this value. Never reused
    /// as the authority for a write; the transaction re-reads it.
    is_admin: bool,
}

/// The whole credential policy in one function, so the routes and the
/// in-transaction re-checks cannot drift apart. `validate_session` covers
/// expiry and deactivation; `is_bot` closes the door on a connected tool that
/// somehow holds a session of its own.
fn session_user(conn: &Connection, token: &str, expected_user_id: i64) -> Result<User, LificError> {
    let user = queries::users::validate_session(conn, token).map_err(|_| denied())?;
    if user.id != expected_user_id || user.is_bot || !user.is_active {
        return Err(denied());
    }
    Ok(user)
}

/// The gate every route runs first. `session_bearer_token` keeps API keys,
/// operator keys and OAuth tokens out; comparing the session's user to the
/// middleware's identity keeps a token swapped between the two reads out.
fn require_human_session(
    db: &DbPool,
    identity: &Option<crate::resolve_caller::ResolvedIdentity>,
    headers: &HeaderMap,
) -> Result<SessionCaller, LificError> {
    let caller = super::require_user(identity).map_err(|_| denied())?;
    let token = crate::auth::session_bearer_token(headers)?;
    let user = with_read(db, |conn| session_user(conn, &token, caller.id))?;
    Ok(SessionCaller {
        user_id: user.id,
        token,
        is_admin: user.is_admin,
    })
}

/// Re-run the export gate on `conn` against the session as the database has
/// it right now, not against the middleware's snapshot.
fn authorize_export(
    conn: &Connection,
    token: &str,
    user_id: i64,
    project_id: i64,
) -> Result<(), LificError> {
    let user = session_user(conn, token, user_id)?;
    let identity = Some(crate::auth::fresh_identity(
        &user,
        crate::actor::Transport::Web,
    ));
    authz::require_role_conn(conn, &identity, project_id, Role::Lead)
}

/// Instance admin in both authorization modes: this creates a project and
/// grants a lead membership, which the legacy mode has no role model for.
fn authorize_import(conn: &Connection, token: &str, user_id: i64) -> Result<User, LificError> {
    let user = session_user(conn, token, user_id)?;
    if !user.is_admin {
        return Err(not_admin());
    }
    Ok(user)
}

// GET /api/project-archives
#[derive(serde::Serialize)]
pub(super) struct ArchiveCapabilities {
    /// From the same freshly-read session user the import gate uses, so it
    /// cannot promise something the import will refuse.
    can_import: bool,
    max_upload_bytes: u64,
    max_expanded_bytes: u64,
    max_metadata_bytes: u64,
    max_blob_bytes: u64,
    /// Combined blob bytes. Not derivable from the others, and an archive can
    /// be under every one of them and still fail this.
    max_blob_total_bytes: u64,
    max_rows: usize,
    max_blobs: usize,
}

pub(super) async fn archive_capabilities(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    headers: HeaderMap,
) -> Result<axum::Json<ArchiveCapabilities>, LificError> {
    let caller = require_human_session(&db, &identity, &headers)?;
    let limits = web_limits();
    Ok(axum::Json(ArchiveCapabilities {
        can_import: caller.is_admin,
        max_upload_bytes: limits.max_compressed,
        max_expanded_bytes: limits.max_expanded,
        max_metadata_bytes: limits.max_metadata,
        max_blob_bytes: limits.max_blob,
        max_blob_total_bytes: limits.max_blob_total,
        max_rows: limits.max_rows,
        max_blobs: limits.max_blobs,
    }))
}

// GET /api/project-archives/{identifier}
pub(super) async fn export_project_archive(
    State(db): State<DbPool>,
    Extension(store): Extension<AttachmentStore>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(identifier): Path<String>,
    headers: HeaderMap,
) -> Result<Response, LificError> {
    let caller = require_human_session(&db, &identity, &headers)?;
    // The path segment is resolved to a row ID once, and everything after
    // this point uses the ID. An identifier is a mutable label: re-resolving
    // it in the snapshot could land on a project this caller was never
    // authorized for.
    let project_id = with_read(&db, |conn| {
        queries::resolve_project_identifier(conn, &identifier)
    })?;
    // Denied before a slot is taken, so a caller with no access cannot make
    // the instance refuse somebody else's archive.
    with_read(&db, |conn| {
        authorize_export(conn, &caller.token, caller.user_id, project_id)
    })?;

    let slot = db.acquire_archive_slot()?;
    let temp_dir = tempfile::tempdir()
        .map_err(|error| LificError::Internal(format!("create archive temp dir: {error}")))?;
    let path = temp_dir.path().join("archive.tar.gz");

    let work_db = db.clone();
    let work_path = path.clone();
    let token = caller.token.clone();
    let user_id = caller.user_id;
    let limits = web_limits();
    // The temp directory is owned by the blocking closure, so a client that
    // disconnects mid-export cannot delete the file the worker is writing.
    let (report, temp_dir, slot) = blocking_export(slot, move || {
        let report = project_archive::export_by_id_with(
            &work_db,
            &store,
            project_id,
            &work_path,
            limits,
            // Re-checked inside the snapshot, before a single issue body,
            // comment or audit row is read.
            &|conn| authorize_export(conn, &token, user_id, project_id),
        )?;
        Ok((report, temp_dir))
    })
    .await
    .map(|((report, temp_dir), slot)| (report, temp_dir, slot))?;

    // A queued export can land long after it was authorized, so the gate runs
    // once more before any byte is sent.
    with_read(&db, |conn| {
        authorize_export(conn, &caller.token, caller.user_id, project_id)
    })?;

    let mut extra_headers = HeaderMap::new();
    extra_headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    extra_headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    extra_headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    stream_response(
        PreparedExport {
            temp_dir,
            path,
            content_type: HeaderValue::from_static("application/gzip"),
            // The identifier the snapshot itself saw, so the filename always
            // names the project whose bytes these are.
            download_name: Some(format!("{}.lific.tar.gz", report.project)),
            extra_headers,
        },
        slot,
    )
    .await
}

// POST /api/project-archives
#[derive(serde::Serialize)]
struct ImportedProject {
    id: i64,
    identifier: String,
    /// Always false. Format 1 carries no publication flag and the importer
    /// never sets one; publishing is a separate, deliberate decision.
    is_public: bool,
}

#[derive(serde::Serialize)]
struct ImportReport {
    project: String,
    rows: std::collections::BTreeMap<&'static str, usize>,
    blobs: usize,
    /// At most [`MAX_REPORTED_REFERENCES`] messages. A hostile or merely
    /// enormous archive can generate tens of thousands, and a response nobody
    /// can render is not a better answer than a truncated one.
    external_references: Vec<String>,
    /// How many were generated in total, so a client can say "showing 100 of
    /// 12,431" rather than silently implying it has them all.
    external_reference_count: usize,
}

#[derive(serde::Serialize)]
struct ImportResponse {
    project: ImportedProject,
    report: ImportReport,
}

/// The report is a response body, not a log file. The full list is stored on
/// `project_archive_provenance` and comes back out in the next archive.
const MAX_REPORTED_REFERENCES: usize = 100;

pub(super) async fn import_project_archive(
    State(db): State<DbPool>,
    Extension(store): Extension<AttachmentStore>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    headers: HeaderMap,
    // Last, because this is what consumes the request body. Every check above
    // runs before a byte of a potentially 128 MiB upload is parsed.
    multipart: Multipart,
) -> Result<Response, LificError> {
    let caller = require_human_session(&db, &identity, &headers)?;
    if !caller.is_admin {
        return Err(not_admin());
    }
    // Capacity before body: a request that will not run should not cost the
    // instance a spooled upload.
    let slot = db.acquire_archive_slot()?;
    let (temp_dir, path) = spool_archive(multipart).await?;

    let work_db = db.clone();
    let token = caller.token.clone();
    let user_id = caller.user_id;
    // `temp_dir` moves into the closure and dies with it, so a cancelled
    // request cannot pull the staged upload out from under a running import.
    // No timeout wraps this: an import that commits must never be reported as
    // a failure, because the caller would then retry a project that exists.
    let limits = web_limits();
    let (outcome, slot) = blocking_export(slot, move || {
        let _staged = temp_dir;
        let outcome = project_archive::import_with(&work_db, &store, &path, limits, &|tx| {
            let admin = authorize_import(tx, &token, user_id)?;
            Ok(project_archive::Grant {
                user_id: admin.id,
                // This person, through the browser. The CLI's actorless
                // grant is the CLI's answer, not this one.
                transport: crate::actor::Transport::Web,
                actor_user_id: Some(admin.id),
            })
        })?;
        // The commit and notification outlive a disconnected HTTP request.
        realtime.send(RealtimeEvent::ProjectUpdated {
            project_id: outcome.project_id,
        });
        Ok(outcome)
    })
    .await?;
    drop(slot);

    let mut external_references = outcome.report.external_references;
    external_references.truncate(MAX_REPORTED_REFERENCES);
    Ok((
        StatusCode::CREATED,
        axum::Json(ImportResponse {
            project: ImportedProject {
                id: outcome.project_id,
                identifier: outcome.report.project.clone(),
                is_public: false,
            },
            report: ImportReport {
                project: outcome.report.project,
                rows: outcome.rows_by_table,
                blobs: outcome.report.blobs,
                external_references,
                external_reference_count: outcome.external_reference_count,
            },
        }),
    )
        .into_response())
}

/// Stream the uploaded archive to a private temp file.
///
/// Nothing the client says about the payload is trusted: not the filename,
/// not the content type, not a declared length. The bytes are counted as they
/// arrive and the upload is cut off the moment it passes the ceiling.
async fn spool_archive(
    mut multipart: Multipart,
) -> Result<(tempfile::TempDir, PathBuf), LificError> {
    let temp_dir = tempfile::tempdir()
        .map_err(|error| LificError::Internal(format!("create upload temp dir: {error}")))?;
    let path = temp_dir.path().join("upload.tar.gz");
    let spooled = tokio::time::timeout(
        UPLOAD_MAX_DURATION,
        read_archive_field(&mut multipart, &path),
    )
    .await
    .map_err(|_| LificError::BadRequest("the upload took too long".into()))?;
    spooled?;
    Ok((temp_dir, path))
}

async fn read_archive_field(
    multipart: &mut Multipart,
    path: &std::path::Path,
) -> Result<(), LificError> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .await
        .map_err(|error| LificError::Internal(format!("stage upload: {error}")))?;

    let mut written = 0u64;
    let mut seen = false;
    while let Some(mut field) = idle(multipart.next_field()).await?? {
        if seen || field.name() != Some(ARCHIVE_FIELD) {
            return Err(LificError::BadRequest(
                "send exactly one multipart field named 'archive'".into(),
            ));
        }
        seen = true;
        while let Some(chunk) = idle(field.chunk()).await?? {
            written = written.saturating_add(chunk.len() as u64);
            if written > web_limits().max_compressed {
                return Err(oversize());
            }
            file.write_all(&chunk)
                .await
                .map_err(|error| LificError::Internal(format!("stage upload: {error}")))?;
        }
    }
    if !seen {
        return Err(LificError::BadRequest(
            "send exactly one multipart field named 'archive'".into(),
        ));
    }
    file.flush()
        .await
        .map_err(|error| LificError::Internal(format!("stage upload: {error}")))?;
    file.sync_all()
        .await
        .map_err(|error| LificError::Internal(format!("stage upload: {error}")))
}

/// Bound one read of the request body, so a client that opens a connection
/// and then trickles cannot hold the archive slot for the full deadline.
async fn idle<T>(
    read: impl Future<Output = Result<T, axum::extract::multipart::MultipartError>>,
) -> Result<Result<T, LificError>, LificError> {
    match tokio::time::timeout(UPLOAD_IDLE_TIMEOUT, read).await {
        Err(_) => Err(LificError::BadRequest("the upload stalled".into())),
        Ok(Ok(value)) => Ok(Ok(value)),
        Ok(Err(error)) => Ok(Err(multipart_error(&error))),
    }
}

/// Size is a 413, framing is a 400. The message is ours, never the caller's
/// bytes echoed back.
fn multipart_error(error: &axum::extract::multipart::MultipartError) -> LificError {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        oversize()
    } else {
        LificError::BadRequest("malformed multipart upload".into())
    }
}

#[cfg(test)]
mod tests;
