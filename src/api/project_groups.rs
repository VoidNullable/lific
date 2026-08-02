//! Per-user project groups for the sidebar.
//!
//! Authorization here is identity-only: any authenticated user manages their
//! own groups, and every query is scoped by `user_id` in the query layer, so
//! group CRUD has no project role to check. Assignment is the exception — its
//! body names a project id, so it takes the normal Viewer gate. Without that,
//! a user could file a project they can't see into a group and learn the id
//! exists.
//!
//! The `ProjectGroupsChanged` events below are addressed to the owning user,
//! which in this codebase still means admins receive them — see that
//! variant's doc comment for what that does and doesn't expose.

use axum::{
    Extension,
    extract::{Json, Path, State},
};

use crate::authz;
use crate::db::queries::project_groups;
use crate::db::{DbPool, models::*};
use crate::error::LificError;
use crate::realtime::{RealtimeEvent, RealtimeHub};

use super::{with_read, with_write};

/// `LificError` has no Unauthorized variant — Forbidden is what the codebase
/// uses for "no authenticated caller", same as `views::require_user`.
fn require_user(
    identity: Option<crate::resolve_caller::ResolvedIdentity>,
) -> Result<AuthUser, LificError> {
    identity
        .map(|i| i.user)
        .ok_or_else(|| {
            LificError::Forbidden("authentication required to manage project groups".into())
        })
}

pub(super) async fn list_groups(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
) -> Result<Json<Vec<ProjectGroup>>, LificError> {
    let visible = authz::visible_project_ids(&db, &identity)?;
    let user = require_user(identity)?;
    let mut groups = with_read(&db, |conn| project_groups::list_groups(conn, user.id))?;
    // A membership outlives the caller's access to that project when a
    // project_members row is revoked. Drop those ids rather than render a
    // sidebar entry that 403s the moment it's clicked.
    if let Some(ids) = &visible {
        for group in &mut groups {
            group.project_ids.retain(|id| ids.contains(id));
        }
    }
    Ok(Json(groups))
}

pub(super) async fn create_group(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Json(input): Json<CreateProjectGroup>,
) -> Result<Json<ProjectGroup>, LificError> {
    let user = require_user(identity)?;
    let group = with_write(&db, |conn| {
        project_groups::create_group(conn, user.id, &input)
    })?;
    realtime.send_to_users(RealtimeEvent::ProjectGroupsChanged, vec![user.id]);
    Ok(Json(group))
}

pub(super) async fn update_group(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(id): Path<i64>,
    Json(input): Json<UpdateProjectGroup>,
) -> Result<Json<ProjectGroup>, LificError> {
    let user = require_user(identity)?;
    let group = with_write(&db, |conn| {
        project_groups::update_group(conn, id, user.id, &input)
    })?;
    realtime.send_to_users(RealtimeEvent::ProjectGroupsChanged, vec![user.id]);
    Ok(Json(group))
}

pub(super) async fn delete_group(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, LificError> {
    let user = require_user(identity)?;
    let deleted = with_write(&db, |conn| project_groups::delete_group(conn, id, user.id))?;
    realtime.send_to_users(RealtimeEvent::ProjectGroupsChanged, vec![user.id]);
    Ok(Json(serde_json::json!({ "deleted": deleted })))
}

pub(super) async fn assign_project(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Json(input): Json<AssignProjectGroup>,
) -> Result<Json<serde_json::Value>, LificError> {
    authz::require_role(&db, &identity, input.project_id, Role::Viewer)?;
    let user = require_user(identity)?;
    with_write(&db, |conn| {
        project_groups::assign_project(conn, user.id, input.project_id, input.group_id)
    })?;
    realtime.send_to_users(RealtimeEvent::ProjectGroupsChanged, vec![user.id]);
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use crate::api::test_helpers::*;
    use axum::http::StatusCode;

    #[tokio::test]
    async fn a_user_can_create_list_rename_and_delete_their_own_group() {
        let (db, _admin, _lead, _maintainer, viewer, _non_member, _project_id) =
            setup_membership_test();
        let app = app_as_user(db, &viewer);

        let resp = json_post(
            &app,
            "/api/project-groups",
            serde_json::json!({ "name": "Work" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let created = parse_json(resp).await;
        let group_id = created["id"].as_i64().unwrap();
        assert_eq!(created["name"], "Work");
        assert_eq!(created["project_ids"].as_array().unwrap().len(), 0);

        let resp = json_get(&app, "/api/project-groups").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(parse_json(resp).await.as_array().unwrap().len(), 1);

        let resp = json_patch(
            &app,
            &format!("/api/project-groups/{group_id}"),
            serde_json::json!({ "name": "Day job" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(parse_json(resp).await["name"], "Day job");

        let resp = json_delete(&app, &format!("/api/project-groups/{group_id}")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(parse_json(resp).await["deleted"], true);
    }

    #[tokio::test]
    async fn duplicate_group_name_conflicts() {
        let (db, _admin, _lead, _maintainer, viewer, _non_member, _project_id) =
            setup_membership_test();
        let app = app_as_user(db, &viewer);

        json_post(
            &app,
            "/api/project-groups",
            serde_json::json!({ "name": "Work" }),
        )
        .await;
        let resp = json_post(
            &app,
            "/api/project-groups",
            serde_json::json!({ "name": "Work" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn another_users_group_is_not_found_rather_than_forbidden() {
        let (db, _admin, _lead, _maintainer, viewer, non_member, _project_id) =
            setup_membership_test();
        let owner_app = app_as_user(db.clone(), &viewer);
        let other_app = app_as_user(db, &non_member);

        let created = parse_json(
            json_post(
                &owner_app,
                "/api/project-groups",
                serde_json::json!({ "name": "Work" }),
            )
            .await,
        )
        .await;
        let group_id = created["id"].as_i64().unwrap();

        let resp = json_patch(
            &other_app,
            &format!("/api/project-groups/{group_id}"),
            serde_json::json!({ "name": "Stolen" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // And it must not appear in their own list either.
        let resp = json_get(&other_app, "/api/project-groups").await;
        assert_eq!(parse_json(resp).await.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn assigning_a_project_the_caller_cannot_view_is_refused() {
        let (db, _admin, _lead, _maintainer, _viewer, non_member, project_id) =
            setup_membership_test();
        let app = app_as_user(db, &non_member);

        let created = parse_json(
            json_post(
                &app,
                "/api/project-groups",
                serde_json::json!({ "name": "Mine" }),
            )
            .await,
        )
        .await;
        let group_id = created["id"].as_i64().unwrap();

        let resp = json_put(
            &app,
            "/api/project-groups/assign",
            serde_json::json!({ "project_id": project_id, "group_id": group_id }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn assigned_project_appears_in_the_group_listing() {
        let (db, _admin, _lead, _maintainer, viewer, _non_member, project_id) =
            setup_membership_test();
        let app = app_as_user(db, &viewer);

        let created = parse_json(
            json_post(
                &app,
                "/api/project-groups",
                serde_json::json!({ "name": "Work" }),
            )
            .await,
        )
        .await;
        let group_id = created["id"].as_i64().unwrap();

        let resp = json_put(
            &app,
            "/api/project-groups/assign",
            serde_json::json!({ "project_id": project_id, "group_id": group_id }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = json_get(&app, "/api/project-groups").await;
        let groups = parse_json(resp).await;
        assert_eq!(groups[0]["project_ids"][0].as_i64().unwrap(), project_id);
    }

    #[tokio::test]
    async fn deleting_another_users_group_is_not_found_and_leaves_it_intact() {
        let (db, _admin, _lead, _maintainer, viewer, non_member, _project_id) =
            setup_membership_test();
        let owner_app = app_as_user(db.clone(), &viewer);
        let other_app = app_as_user(db, &non_member);

        let created = parse_json(
            json_post(
                &owner_app,
                "/api/project-groups",
                serde_json::json!({ "name": "Work" }),
            )
            .await,
        )
        .await;
        let group_id = created["id"].as_i64().unwrap();

        let resp = json_delete(&other_app, &format!("/api/project-groups/{group_id}")).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // 404 has to mean "left alone", not "deleted but reported as missing":
        // delete_group runs get_owned_group first, and this is what proves the
        // ownership check gates the DELETE rather than merely shaping its
        // response.
        let resp = json_get(&owner_app, "/api/project-groups").await;
        let groups = parse_json(resp).await;
        assert_eq!(groups.as_array().unwrap().len(), 1);
        assert_eq!(groups[0]["id"].as_i64().unwrap(), group_id);
    }

    #[tokio::test]
    async fn revoking_access_drops_the_project_from_the_group_listing() {
        let (db, _admin, _lead, _maintainer, viewer, _non_member, project_id) =
            setup_membership_test();
        let app = app_as_user(db.clone(), &viewer);

        let created = parse_json(
            json_post(
                &app,
                "/api/project-groups",
                serde_json::json!({ "name": "Work" }),
            )
            .await,
        )
        .await;
        let group_id = created["id"].as_i64().unwrap();

        let resp = json_put(
            &app,
            "/api/project-groups/assign",
            serde_json::json!({ "project_id": project_id, "group_id": group_id }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        // The membership row outlives the caller's access to the project.
        // list_groups intersects project_ids against visible_project_ids so the
        // sidebar never renders an entry that 403s the moment it is clicked.
        {
            let conn = db.write().unwrap();
            crate::db::queries::members::remove_member(&conn, project_id, viewer.id).unwrap();
        }

        let resp = json_get(&app, "/api/project-groups").await;
        let groups = parse_json(resp).await;
        assert_eq!(groups.as_array().unwrap().len(), 1, "the group itself stays");
        assert!(
            groups[0]["project_ids"].as_array().unwrap().is_empty(),
            "a project the caller can no longer see must not be listed"
        );

        // Filtered from the response, not deleted: restoring access brings it
        // back without the user having to file the project again.
        {
            let conn = db.write().unwrap();
            crate::db::queries::members::upsert_member(
                &conn,
                project_id,
                viewer.id,
                crate::db::models::Role::Viewer,
            )
            .unwrap();
        }

        let resp = json_get(&app, "/api/project-groups").await;
        let groups = parse_json(resp).await;
        assert_eq!(groups[0]["project_ids"][0].as_i64().unwrap(), project_id);
    }
}
