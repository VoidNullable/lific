//! LIF-484: REST surface for user and date blockers ("waits").
//!
//! `GET` lists an issue's waits (Viewer), `POST` adds one and `DELETE`
//! clears one (Maintainer, the bar every other relation write takes). Each
//! write advances the issue's seq through migration 054's triggers, and the
//! handler publishes `issue.updated` at that seq so the web read model pulls
//! the re-delivered row.

use axum::{
    Extension,
    extract::{Json, Path, State},
};

use crate::authz;
use crate::db::queries::{self, waits};
use crate::db::{DbPool, models::*};
use crate::error::LificError;
use crate::realtime::{RealtimeEvent, RealtimeHub};

use super::with_read;

/// `GET /api/clock`: the server's calendar day and UTC offset, which decide
/// whether a date wait is holding, due or overdue. The web client computes
/// "today" at this offset rather than in the browser's timezone. Signed-in
/// callers only: the offset hints at where the server lives.
pub(super) async fn server_clock(
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
) -> Result<Json<serde_json::Value>, LificError> {
    super::require_user(&identity)?;
    Ok(Json(serde_json::json!({
        "today": waits::today_text(),
        "utc_offset_minutes": waits::utc_offset_minutes(),
    })))
}

pub(super) async fn list_waits(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<IssueWait>>, LificError> {
    let project_id = with_read(&db, |conn| queries::issue_project_id(conn, id))?;
    authz::require_role(&db, &identity, project_id, Role::Viewer)?;
    with_read(&db, |conn| waits::list_waits(conn, id)).map(Json)
}

pub(super) async fn add_wait(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(id): Path<i64>,
    Json(input): Json<CreateWait>,
) -> Result<Json<IssueWait>, LificError> {
    let project_id = with_read(&db, |conn| queries::issue_project_id(conn, id))?;
    authz::require_role(&db, &identity, project_id, Role::Maintainer)?;
    let user = super::require_user(&identity)?;
    let (wait, seq) = db.transaction(|conn| {
        authz::require_role_conn(conn, &identity, project_id, Role::Maintainer)?;
        let wait = waits::add_wait(conn, id, &input, Some(user.id))?;
        Ok((wait, queries::issue_seq(conn, id)?))
    })?;
    realtime.send_with_seq(
        RealtimeEvent::IssueUpdated {
            project_id,
            issue_id: id,
        },
        seq,
    );
    Ok(Json(wait))
}

pub(super) async fn clear_wait(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path((id, wait_id)): Path<(i64, i64)>,
) -> Result<Json<serde_json::Value>, LificError> {
    let project_id = with_read(&db, |conn| queries::issue_project_id(conn, id))?;
    authz::require_role(&db, &identity, project_id, Role::Maintainer)?;
    let seq = db.transaction(|conn| {
        authz::require_role_conn(conn, &identity, project_id, Role::Maintainer)?;
        waits::clear_wait(conn, id, wait_id)?;
        queries::issue_seq(conn, id)
    })?;
    realtime.send_with_seq(
        RealtimeEvent::IssueUpdated {
            project_id,
            issue_id: id,
        },
        seq,
    );
    Ok(Json(serde_json::json!({"cleared": true})))
}

#[cfg(test)]
mod tests;
