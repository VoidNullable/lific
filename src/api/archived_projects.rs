//! Archived projects over HTTP. See [`crate::archived_projects`].
//!
//! Archiving is gated exactly like deleting the project, because to everyone
//! but an instance admin it is a deletion: Lead with enforcement on, instance
//! admin with it off. Listing and unarchiving need an instance admin in both
//! modes, because unarchiving creates a project and grants a lead membership,
//! the same reason importing a project archive does. Every gate is re-run on
//! state read inside the writer transaction that acts on it.

use axum::Extension;
use axum::extract::{Json, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use rusqlite::Connection;

use crate::actor::{ActorCtx, Transport};
use crate::archived_projects::{self, ArchivedProject};
use crate::db::DbPool;
use crate::db::models::User;
use crate::error::LificError;
use crate::project_archive::Grant;
use crate::realtime::{RealtimeEvent, RealtimeHub};
use crate::resolve_caller::ResolvedIdentity;
use crate::storage::AttachmentStore;

use super::export::blocking_export;
use super::with_read;

fn not_admin() -> LificError {
    LificError::Forbidden("only an admin can manage archived projects".into())
}

/// An active human instance admin, as the database has it on `conn`.
fn fresh_admin(conn: &Connection, caller_id: i64) -> Result<User, LificError> {
    let user = crate::auth::fresh_caller(conn, caller_id)?;
    if !user.is_admin || user.is_bot {
        return Err(not_admin());
    }
    Ok(user)
}

/// The caller's transport, captured before the work moves to a blocking
/// thread where the request's actor context is no longer in scope.
fn transport_of(identity: &Option<ResolvedIdentity>) -> Transport {
    identity.as_ref().map_or(Transport::Web, |i| i.transport)
}

fn require_admin(db: &DbPool, identity: &Option<ResolvedIdentity>) -> Result<User, LificError> {
    let caller = super::require_user(identity)?;
    with_read(db, |conn| fresh_admin(conn, caller.id))
}

// POST /api/projects/{id}/archive
pub(super) async fn archive_project(
    State(db): State<DbPool>,
    Extension(store): Extension<AttachmentStore>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(identity): Extension<Option<ResolvedIdentity>>,
    Path(project_id): Path<i64>,
) -> Result<Response, LificError> {
    // Preflight on a read connection, so a caller without the right never
    // takes the archive slot or the writer.
    super::require_project_delete(&db, &identity, project_id)?;
    let caller_id = super::require_user(&identity)?.id;
    let transport = transport_of(&identity);
    let slot = db.acquire_archive_slot()?;
    let work_db = db.clone();
    let (archived, slot) = blocking_export(slot, move || {
        archived_projects::archive(&work_db, &store, project_id, &|tx| {
            let fresh = crate::auth::fresh_caller(tx, caller_id)?;
            let fresh_identity = Some(crate::auth::fresh_identity(&fresh, transport));
            crate::authz::require_project_delete_role_conn(tx, &fresh_identity, project_id)?;
            Ok(ActorCtx {
                user_id: Some(fresh.id),
                transport,
            })
        })
    })
    .await?;
    drop(slot);
    let event = RealtimeEvent::ProjectDeleted {
        project_id: archived.project_id,
    };
    match archived.audience {
        Some(user_ids) => realtime.send_to_users(event, user_ids),
        None => realtime.send(event),
    }
    Ok((StatusCode::CREATED, Json(archived.record)).into_response())
}

// GET /api/archived-projects
pub(super) async fn list_archived_projects(
    State(db): State<DbPool>,
    Extension(store): Extension<AttachmentStore>,
    Extension(identity): Extension<Option<ResolvedIdentity>>,
) -> Result<Json<Vec<ArchivedProject>>, LificError> {
    require_admin(&db, &identity)?;
    with_read(&db, |conn| archived_projects::list(conn, &store)).map(Json)
}

// POST /api/archived-projects/{id}/unarchive
pub(super) async fn unarchive_project(
    State(db): State<DbPool>,
    Extension(store): Extension<AttachmentStore>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(identity): Extension<Option<ResolvedIdentity>>,
    Path(archive_id): Path<i64>,
) -> Result<Json<archived_projects::Unarchived>, LificError> {
    let caller_id = require_admin(&db, &identity)?.id;
    let transport = transport_of(&identity);
    let slot = db.acquire_archive_slot()?;
    let work_db = db.clone();
    // No timeout: an unarchive that commits must never be reported as a
    // failure, or the caller would retry a project that already exists.
    let (unarchived, slot) = blocking_export(slot, move || {
        let unarchived = archived_projects::unarchive(&work_db, &store, archive_id, &|tx| {
            let admin = fresh_admin(tx, caller_id)?;
            Ok(Grant {
                user_id: admin.id,
                transport,
                actor_user_id: Some(admin.id),
            })
        })?;
        realtime.send(RealtimeEvent::ProjectCreated {
            project_id: unarchived.project_id,
        });
        Ok(unarchived)
    })
    .await?;
    drop(slot);
    Ok(Json(unarchived))
}

#[cfg(test)]
mod tests;
