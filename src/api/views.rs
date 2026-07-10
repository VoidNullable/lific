//! LIF-242: REST endpoints for saved views — named filter/group/sort
//! presets per project, personal to each user (no team-shared views; that's
//! a possible future extension, not this one).
//!
//! ## Authorization
//!
//! Every endpoint here is gated at `Role::Viewer` on the project named in
//! the URL: `authz::require_role(.., Role::Viewer)` — a viewer can save
//! *personal* views (it's a read-side convenience over data they can
//! already see), unlike the Maintainer bar for actual content mutation.
//! That's the project-role half of the story; see the module doc comment on
//! `db::queries::views` for the **ownership** half, which the Viewer gate
//! alone does not provide: a project member with the Viewer role can only
//! ever see/modify their *own* saved views, never a teammate's, even though
//! both hold the same project role. Ownership is enforced in the query
//! layer (`views::get_owned_view`, used by every id-scoped operation),
//! collapsing "doesn't exist" and "exists but isn't yours" into the same
//! 404 — never a 403 that would confirm another user's view id is real.
//!
//! `config` is treated as an opaque JSON string here and in the query
//! layer: validated for size (<=8KB) and well-formedness, never
//! schema-validated against `ViewConfig` (web/src/lib/issues/views.ts) —
//! that contract lives entirely on the frontend so it can evolve without a
//! backend migration.

use axum::{
    Extension,
    extract::{Json, Path, State},
};

use crate::authz;
use crate::db::queries::views;
use crate::db::{DbPool, models::*};
use crate::error::LificError;

use super::{with_read, with_write};

/// An OAuth/legacy-key request with no resolved `AuthUser` can pass the
/// project-role gate in legacy mode (`require_role`'s `Viewer` branch is an
/// unconditional allow when `authz_enforced` is off — see `authz.rs`), but
/// saved views are inherently per-user, so there is no sensible "anonymous
/// owner" to attribute a view to. Every handler below requires a resolved
/// user on top of the role gate.
fn require_user(auth_user: Option<AuthUser>) -> Result<AuthUser, LificError> {
    auth_user.ok_or_else(|| {
        LificError::Forbidden("authentication required to manage saved views".into())
    })
}

/// GET /api/projects/{id}/views — the caller's own views only.
pub(super) async fn list_views(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path(project_id): Path<i64>,
) -> Result<Json<Vec<SavedView>>, LificError> {
    authz::require_role(&db, &auth_user, project_id, Role::Viewer)?;
    let user = require_user(auth_user)?;
    with_read(&db, |conn| views::list_views(conn, project_id, user.id)).map(Json)
}

/// POST /api/projects/{id}/views — create a new saved view owned by the
/// caller. `is_default: true` atomically demotes any other default the
/// caller holds on this project (see `views::create_view`). 409 on a
/// duplicate `(project, caller, name)`; 400 on an empty name or an invalid
/// `config` (too large, or not valid JSON).
pub(super) async fn create_view(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path(project_id): Path<i64>,
    Json(input): Json<CreateSavedView>,
) -> Result<Json<SavedView>, LificError> {
    authz::require_role(&db, &auth_user, project_id, Role::Viewer)?;
    let user = require_user(auth_user)?;
    with_write(&db, |conn| {
        views::create_view(conn, project_id, user.id, &input)
    })
    .map(Json)
}

/// PATCH /api/projects/{id}/views/{view_id} — rename, replace the config,
/// and/or (un)set the default flag, all independently addressable. 404 if
/// `view_id` doesn't exist, belongs to a different project, or belongs to a
/// different user (see the module doc comment — never a 403 here).
pub(super) async fn update_view(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path((project_id, view_id)): Path<(i64, i64)>,
    Json(input): Json<UpdateSavedView>,
) -> Result<Json<SavedView>, LificError> {
    authz::require_role(&db, &auth_user, project_id, Role::Viewer)?;
    let user = require_user(auth_user)?;
    with_write(&db, |conn| {
        views::update_view(conn, view_id, project_id, user.id, &input)
    })
        .map(Json)
}

/// DELETE /api/projects/{id}/views/{view_id} — same ownership 404 as PATCH.
pub(super) async fn delete_view(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path((project_id, view_id)): Path<(i64, i64)>,
) -> Result<Json<serde_json::Value>, LificError> {
    authz::require_role(&db, &auth_user, project_id, Role::Viewer)?;
    let user = require_user(auth_user)?;
    with_write(&db, |conn| {
        views::delete_view(conn, view_id, project_id, user.id)
    })?;
    Ok(Json(serde_json::json!({"deleted": true})))
}

#[cfg(test)]
mod tests {
    use crate::api::test_helpers::*;
    use axum::http::StatusCode;

    // ── CRUD happy path ───────────────────────────────────────────

    #[tokio::test]
    async fn viewer_can_create_list_update_and_delete_their_own_view() {
        let (db, _admin, _lead, _maintainer, viewer, _non_member, project_id) =
            setup_membership_test();
        let app = app_as_user(db, &viewer);

        let resp = json_post(
            &app,
            &format!("/api/projects/{project_id}/views"),
            serde_json::json!({ "name": "My triage", "config": "{\"groupBy\":\"status\"}" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let created = parse_json(resp).await;
        assert_eq!(created["name"], "My triage");
        assert_eq!(created["is_default"], false);
        let view_id = created["id"].as_i64().unwrap();

        let list =
            parse_json(json_get(&app, &format!("/api/projects/{project_id}/views")).await).await;
        assert_eq!(list.as_array().unwrap().len(), 1);

        let resp = json_patch(
            &app,
            &format!("/api/projects/{project_id}/views/{view_id}"),
            serde_json::json!({ "name": "Renamed" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(parse_json(resp).await["name"], "Renamed");

        let resp = json_delete(&app, &format!("/api/projects/{project_id}/views/{view_id}")).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let list =
            parse_json(json_get(&app, &format!("/api/projects/{project_id}/views")).await).await;
        assert!(list.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn maintainer_and_lead_can_also_save_personal_views() {
        // Viewer is the floor, not the ceiling — every role at or above it
        // can save personal views too.
        let (db, _admin, lead, maintainer, _viewer, _non_member, project_id) =
            setup_membership_test();

        for actor in [&maintainer, &lead] {
            let app = app_as_user(db.clone(), actor);
            let resp = json_post(
                &app,
                &format!("/api/projects/{project_id}/views"),
                serde_json::json!({ "name": format!("{}'s view", actor.username), "config": "{}" }),
            )
            .await;
            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "{} should be able to save a view",
                actor.username
            );
        }
    }

    // ── Ownership isolation ─────────────────────────────────────────

    #[tokio::test]
    async fn user_b_cannot_list_patch_or_delete_user_as_views() {
        let (db, _admin, _lead, _maintainer, viewer, non_member, project_id) =
            setup_membership_test();
        // Give non_member a Viewer role too so this isolates ownership from
        // the project-role gate (both users pass the role check equally).
        {
            let conn = db.write().unwrap();
            crate::db::queries::members::upsert_member(
                &conn,
                project_id,
                non_member.id,
                crate::db::models::Role::Viewer,
            )
            .unwrap();
        }

        let a_app = app_as_user(db.clone(), &viewer);
        let created = parse_json(
            json_post(
                &a_app,
                &format!("/api/projects/{project_id}/views"),
                serde_json::json!({ "name": "A's private view", "config": "{}" }),
            )
            .await,
        )
        .await;
        let view_id = created["id"].as_i64().unwrap();

        let b_app = app_as_user(db, &non_member);

        // B's list must not include A's view.
        let b_list =
            parse_json(json_get(&b_app, &format!("/api/projects/{project_id}/views")).await).await;
        assert!(
            b_list.as_array().unwrap().is_empty(),
            "B must not see A's views: {b_list:#?}"
        );

        // B's PATCH/DELETE on A's view_id must 404, not 403 (existence
        // must not be confirmable via a different status code) and not 200.
        let resp = json_patch(
            &b_app,
            &format!("/api/projects/{project_id}/views/{view_id}"),
            serde_json::json!({ "name": "Hijacked" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let resp = json_delete(
            &b_app,
            &format!("/api/projects/{project_id}/views/{view_id}"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        // A's view is untouched by B's rejected attempts.
        let a_list =
            parse_json(json_get(&a_app, &format!("/api/projects/{project_id}/views")).await).await;
        assert_eq!(a_list.as_array().unwrap().len(), 1);
        assert_eq!(a_list.as_array().unwrap()[0]["name"], "A's private view");
    }

    // ── Default uniqueness (REST-level) ─────────────────────────────

    #[tokio::test]
    async fn setting_default_via_rest_clears_the_previous_default() {
        let (db, _admin, _lead, _maintainer, viewer, _non_member, project_id) =
            setup_membership_test();
        let app = app_as_user(db, &viewer);

        let first = parse_json(
            json_post(
                &app,
                &format!("/api/projects/{project_id}/views"),
                serde_json::json!({ "name": "First", "config": "{}", "is_default": true }),
            )
            .await,
        )
        .await;
        assert_eq!(first["is_default"], true);
        let first_id = first["id"].as_i64().unwrap();

        let second = parse_json(
            json_post(
                &app,
                &format!("/api/projects/{project_id}/views"),
                serde_json::json!({ "name": "Second", "config": "{}", "is_default": true }),
            )
            .await,
        )
        .await;
        assert_eq!(second["is_default"], true);

        let list =
            parse_json(json_get(&app, &format!("/api/projects/{project_id}/views")).await).await;
        let first_after = list
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["id"] == first_id)
            .unwrap();
        assert_eq!(
            first_after["is_default"], false,
            "first default must be cleared: {list:#?}"
        );
    }

    // ── Validation ───────────────────────────────────────────────────

    #[tokio::test]
    async fn create_rejects_invalid_json_config_and_empty_name() {
        let (db, _admin, _lead, _maintainer, viewer, _non_member, project_id) =
            setup_membership_test();
        let app = app_as_user(db, &viewer);

        let resp = json_post(
            &app,
            &format!("/api/projects/{project_id}/views"),
            serde_json::json!({ "name": "Bad JSON", "config": "{not json" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let resp = json_post(
            &app,
            &format!("/api/projects/{project_id}/views"),
            serde_json::json!({ "name": "  ", "config": "{}" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn duplicate_name_for_the_same_user_is_conflict() {
        let (db, _admin, _lead, _maintainer, viewer, _non_member, project_id) =
            setup_membership_test();
        let app = app_as_user(db, &viewer);

        json_post(
            &app,
            &format!("/api/projects/{project_id}/views"),
            serde_json::json!({ "name": "Dup", "config": "{}" }),
        )
        .await;
        let resp = json_post(
            &app,
            &format!("/api/projects/{project_id}/views"),
            serde_json::json!({ "name": "Dup", "config": "{}" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    // ── Gating: non-member denied when authz_enforced is on ──────────

    #[tokio::test]
    async fn non_member_denied_on_every_endpoint_when_enforced() {
        let (db, _admin, lead, _maintainer, _viewer, non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db.clone(), &lead);
        let created = parse_json(
            json_post(
                &lead_app,
                &format!("/api/projects/{project_id}/views"),
                serde_json::json!({ "name": "Lead's view", "config": "{}" }),
            )
            .await,
        )
        .await;
        let view_id = created["id"].as_i64().unwrap();

        let non_member_app = app_as_user(db, &non_member);

        assert_eq!(
            json_get(
                &non_member_app,
                &format!("/api/projects/{project_id}/views")
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            json_post(
                &non_member_app,
                &format!("/api/projects/{project_id}/views"),
                serde_json::json!({ "name": "Nope", "config": "{}" }),
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            json_patch(
                &non_member_app,
                &format!("/api/projects/{project_id}/views/{view_id}"),
                serde_json::json!({ "name": "Nope" }),
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            json_delete(
                &non_member_app,
                &format!("/api/projects/{project_id}/views/{view_id}")
            )
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }

    // ── Legacy mode (flag off): open to any authenticated user ───────

    #[tokio::test]
    async fn flag_off_any_authenticated_user_can_manage_their_own_views() {
        let (db, _admin, _lead, regular, project_id) = setup_lead_test();
        let app = app_as_user(db, &regular);

        let resp = json_post(
            &app,
            &format!("/api/projects/{project_id}/views"),
            serde_json::json!({ "name": "Legacy view", "config": "{}" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
