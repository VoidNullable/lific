//! LIF-199: REST endpoints for a project lead to manage who belongs to
//! their project and at what role. Design: LIF-DOC-7 decision #14 — this
//! is deliberately web/REST only, no MCP tools (schema token budget; it's
//! a human-admin task).
//!
//! Every mutating endpoint here is lead-gated in **both** authz modes:
//! `authz::require_role(.., Role::Lead)` reproduces today's
//! `require_project_lead` semantics verbatim when `authz_enforced` is off
//! (`lead_user_id` or admin, unowned-project admin-only carve-out — see
//! `authz::require_lead_legacy`) and the default-deny membership check
//! when it's on. Unlike `require_structure_role`, there's no legacy
//! behavior to preserve here: membership management is a brand-new
//! surface, so it's simply `Role::Lead` in both modes.
//!
//! Guard rails (last-lead protection, strict non-upsert add,
//! `lead_user_id` pointer upkeep) live in `db::queries::members` — see
//! that module's doc comments on `add_member` / `change_role` /
//! `remove_member_guarded` for the exact rules. Audit logging needs no
//! code here at all: `project_members` writes flow through the normal
//! query layer, and migration 028's triggers capture them the same way
//! every other entity is captured (actor attribution via the
//! `_actor_state` stamp `DbPool::write()` sets — see `src/actor.rs`).

use axum::{
    Extension,
    extract::{Json, Path, State},
};

use crate::authz;
use crate::db::queries::members;
use crate::db::{DbPool, models::*};
use crate::error::LificError;
use crate::realtime::{RealtimeEvent, RealtimeHub};

use super::{with_read, with_write};

/// GET /api/projects/{id}/members — visible to any project member
/// (`Viewer`+); non-members are denied same as any other project read.
pub(super) async fn list_project_members(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path(project_id): Path<i64>,
) -> Result<Json<Vec<MemberWithUser>>, LificError> {
    authz::require_role(&db, &auth_user, project_id, Role::Viewer)?;
    with_read(&db, |conn| {
        members::list_members_with_users(conn, project_id)
    })
    .map(Json)
}

/// GET /api/projects/{id}/my-role — the caller's own effective role on this
/// project, plus whether enforcement is on and whether they're a workspace
/// admin. Viewer-gated so any member (including a plain viewer) can learn
/// their own role cheaply without reading the whole member roster or the
/// admin-only instance settings.
///
/// LIF-234: this is the single source the web app reads to gate mutate
/// affordances (`web/src/lib/projectRole.svelte.ts`). It deliberately
/// answers "what can I do here" rather than exposing the roster:
///   - `enforced=false` → the instance is in legacy mode; the UI stays fully
///     interactive (server still allows everything a non-lead would try).
///   - `is_admin=true` → workspace admin; the UI stays fully interactive.
///   - `role` is the effective role resolved through bot→owner inheritance,
///     so an agent key reports the human it inherits from, and is `null`
///     only for a non-member admin (who is gated by `is_admin` instead).
///
/// Being Viewer-gated, a non-member is denied (403) here exactly as they are
/// on every other project read — the client treats that denial as "no
/// access," never as "full access."
pub(super) async fn my_project_role(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path(project_id): Path<i64>,
) -> Result<Json<serde_json::Value>, LificError> {
    authz::require_role(&db, &auth_user, project_id, Role::Viewer)?;

    let enforced = authz::authz_enforced(&db)?;
    let (role, is_admin) = with_read(&db, |conn| {
        let effective = authz::effective_user(conn, &auth_user);
        let is_admin = matches!(&effective, Some(u) if u.is_admin);
        let role = match &effective {
            Some(u) => members::get_member_role(conn, project_id, u.id)?,
            None => None,
        };
        Ok((role, is_admin))
    })?;

    Ok(Json(serde_json::json!({
        "role": role.map(|r| r.as_str()),
        "enforced": enforced,
        "is_admin": is_admin,
    })))
}

/// POST /api/projects/{id}/members — add a member. `role` defaults to
/// `viewer` when omitted (design: "default grant = viewer"). 409 if the
/// user is already a member (use `PATCH` to change an existing role), 404
/// if `user_id` doesn't resolve to a real user.
pub(super) async fn add_project_member(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path(project_id): Path<i64>,
    Json(input): Json<AddMember>,
) -> Result<Json<ProjectMember>, LificError> {
    authz::require_role(&db, &auth_user, project_id, Role::Lead)?;
    let role = input.role.as_deref().unwrap_or("viewer").to_string();
    let member = with_write(&db, |conn| {
        members::add_member(conn, project_id, input.user_id, &role)
    })?;
    realtime.send(RealtimeEvent::ProjectUpdated { project_id });
    Ok(Json(member))
}

/// PATCH /api/projects/{id}/members/{user_id} — change an existing
/// member's role. 404 if they aren't a member; 409 if this would demote
/// the project's sole `lead`.
pub(super) async fn update_project_member(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path((project_id, user_id)): Path<(i64, i64)>,
    Json(input): Json<ChangeMemberRole>,
) -> Result<Json<ProjectMember>, LificError> {
    authz::require_role(&db, &auth_user, project_id, Role::Lead)?;
    let member = with_write(&db, |conn| {
        members::change_role(conn, project_id, user_id, &input.role)
    })?;
    realtime.send(RealtimeEvent::ProjectUpdated { project_id });
    Ok(Json(member))
}

/// DELETE /api/projects/{id}/members/{user_id} — remove a member. 404 if
/// they aren't a member; 409 if they're the project's sole `lead`.
pub(super) async fn remove_project_member(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path((project_id, user_id)): Path<(i64, i64)>,
) -> Result<Json<serde_json::Value>, LificError> {
    authz::require_role(&db, &auth_user, project_id, Role::Lead)?;
    with_write(&db, |conn| {
        members::remove_member_guarded(conn, project_id, user_id)
    })?;
    realtime.send(RealtimeEvent::ProjectUpdated { project_id });
    Ok(Json(serde_json::json!({"deleted": true})))
}

#[cfg(test)]
mod tests {
    use crate::api::test_helpers::*;
    use axum::http::StatusCode;

    // ── Lead can add / change / remove ───────────────────────────

    #[tokio::test]
    async fn lead_can_add_change_and_remove_a_member() {
        let (db, _admin, lead, _maintainer, _viewer, non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db.clone(), &lead);

        // Add with default role (viewer).
        let resp = json_post(
            &lead_app,
            &format!("/api/projects/{project_id}/members"),
            serde_json::json!({ "user_id": non_member.id }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let added = parse_json(resp).await;
        assert_eq!(added["user_id"], non_member.id);
        assert_eq!(added["role"], "viewer");
        assert_eq!(added["project_id"], project_id);

        // Change role.
        let resp = json_patch(
            &lead_app,
            &format!("/api/projects/{project_id}/members/{}", non_member.id),
            serde_json::json!({ "role": "maintainer" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(parse_json(resp).await["role"], "maintainer");

        // Remove.
        let resp = json_delete(
            &lead_app,
            &format!("/api/projects/{project_id}/members/{}", non_member.id),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        // Confirm gone: listing no longer includes them.
        let list =
            parse_json(json_get(&lead_app, &format!("/api/projects/{project_id}/members")).await)
                .await;
        assert!(
            list.as_array()
                .unwrap()
                .iter()
                .all(|m| m["user_id"] != non_member.id),
            "removed member must not appear in the list: {list:#?}"
        );
    }

    #[tokio::test]
    async fn added_member_with_explicit_role_is_honored() {
        let (db, _admin, lead, _maintainer, _viewer, non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db, &lead);

        let resp = json_post(
            &lead_app,
            &format!("/api/projects/{project_id}/members"),
            serde_json::json!({ "user_id": non_member.id, "role": "lead" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(parse_json(resp).await["role"], "lead");
    }

    // ── Maintainer / viewer / non-member denied on write ─────────

    #[tokio::test]
    async fn maintainer_viewer_and_non_member_denied_on_add() {
        let (db, _admin, _lead, maintainer, viewer, non_member, project_id) =
            setup_membership_test();
        let target = non_member.id; // reuse as a fresh target user id in a couple of cases

        for actor in [&maintainer, &viewer, &non_member] {
            let app = app_as_user(db.clone(), actor);
            let resp = json_post(
                &app,
                &format!("/api/projects/{project_id}/members"),
                serde_json::json!({ "user_id": target }),
            )
            .await;
            assert_eq!(
                resp.status(),
                StatusCode::FORBIDDEN,
                "{} must be denied on add",
                actor.username
            );
        }
    }

    #[tokio::test]
    async fn maintainer_viewer_and_non_member_denied_on_patch_and_delete() {
        let (db, _admin, lead, maintainer, viewer, non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db.clone(), &lead);
        // maintainer/viewer are already members via setup_membership_test.

        for actor in [&maintainer, &viewer, &non_member] {
            let app = app_as_user(db.clone(), actor);
            let resp = json_patch(
                &app,
                &format!("/api/projects/{project_id}/members/{}", maintainer.id),
                serde_json::json!({ "role": "lead" }),
            )
            .await;
            assert_eq!(
                resp.status(),
                StatusCode::FORBIDDEN,
                "{} must be denied on patch",
                actor.username
            );

            let resp = json_delete(
                &app,
                &format!("/api/projects/{project_id}/members/{}", viewer.id),
            )
            .await;
            assert_eq!(
                resp.status(),
                StatusCode::FORBIDDEN,
                "{} must be denied on delete",
                actor.username
            );
        }

        // Sanity: the lead can still actually do it (proves the denials
        // above weren't just "endpoint broken").
        let resp = json_patch(
            &lead_app,
            &format!("/api/projects/{project_id}/members/{}", viewer.id),
            serde_json::json!({ "role": "maintainer" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── Members list: viewer allowed, non-member denied ──────────

    #[tokio::test]
    async fn members_list_visible_to_viewer_denied_to_non_member() {
        let (db, _admin, _lead, _maintainer, viewer, non_member, project_id) =
            setup_membership_test();

        let viewer_app = app_as_user(db.clone(), &viewer);
        let resp = json_get(&viewer_app, &format!("/api/projects/{project_id}/members")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let list = parse_json(resp).await;
        let arr = list.as_array().unwrap();
        assert!(arr.iter().any(|m| m["username"] == "lead"));
        assert!(arr.iter().any(|m| m["username"] == "maintainer"));
        assert!(arr.iter().any(|m| m["username"] == "viewer"));

        let non_member_app = app_as_user(db, &non_member);
        assert_eq!(
            json_get(
                &non_member_app,
                &format!("/api/projects/{project_id}/members")
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
    }

    // ── Last-lead protection ──────────────────────────────────────

    #[tokio::test]
    async fn demoting_or_removing_the_sole_lead_is_rejected_until_a_second_lead_exists() {
        let (db, _admin, lead, _maintainer, _viewer, _non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db.clone(), &lead);

        // Sole lead: demotion and removal both rejected.
        let resp = json_patch(
            &lead_app,
            &format!("/api/projects/{project_id}/members/{}", lead.id),
            serde_json::json!({ "role": "maintainer" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);

        let resp = json_delete(
            &lead_app,
            &format!("/api/projects/{project_id}/members/{}", lead.id),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);

        // Promote a second lead, then the original can be demoted/removed.
        let resp = json_post(
            &lead_app,
            &format!("/api/projects/{project_id}/members"),
            serde_json::json!({ "user_id": _non_member.id, "role": "lead" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = json_patch(
            &lead_app,
            &format!("/api/projects/{project_id}/members/{}", lead.id),
            serde_json::json!({ "role": "maintainer" }),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "demotion allowed once a second lead exists"
        );
    }

    // ── POST duplicate / unknown user / bad role ──────────────────

    #[tokio::test]
    async fn post_duplicate_member_is_409() {
        let (db, _admin, lead, maintainer, _viewer, _non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db, &lead);

        let resp = json_post(
            &lead_app,
            &format!("/api/projects/{project_id}/members"),
            serde_json::json!({ "user_id": maintainer.id }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn post_unknown_user_is_404() {
        let (db, _admin, lead, _maintainer, _viewer, _non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db, &lead);

        let resp = json_post(
            &lead_app,
            &format!("/api/projects/{project_id}/members"),
            serde_json::json!({ "user_id": 999999 }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn post_bad_role_is_400() {
        let (db, _admin, lead, _maintainer, _viewer, non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db, &lead);

        let resp = json_post(
            &lead_app,
            &format!("/api/projects/{project_id}/members"),
            serde_json::json!({ "user_id": non_member.id, "role": "owner" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── Admin can manage members of a project they're not in ──────

    #[tokio::test]
    async fn admin_can_manage_members_of_a_project_they_are_not_in() {
        let (db, admin, _lead, _maintainer, _viewer, non_member, project_id) =
            setup_membership_test();
        let admin_app = app_as_user(db, &admin);

        let resp = json_post(
            &admin_app,
            &format!("/api/projects/{project_id}/members"),
            serde_json::json!({ "user_id": non_member.id, "role": "maintainer" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = json_patch(
            &admin_app,
            &format!("/api/projects/{project_id}/members/{}", non_member.id),
            serde_json::json!({ "role": "viewer" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = json_delete(
            &admin_app,
            &format!("/api/projects/{project_id}/members/{}", non_member.id),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── Membership changes land in the project activity feed ──────

    #[tokio::test]
    async fn membership_changes_appear_in_project_activity_with_acting_user() {
        let (db, _admin, lead, _maintainer, _viewer, non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db, &lead);
        let lead_id = lead.id;

        // The tower test harness injects `AuthUser` straight into request
        // extensions (see `test_helpers::app_as_user`), bypassing the real
        // `auth::require_api_key` middleware that normally scopes
        // `actor::current()` for the audit triggers to read (`src/actor.rs`).
        // Wrapping the requests in the same actor scope the middleware would
        // set reproduces that attribution for this test, without needing a
        // second full auth stack — `oneshot()` polls the router's future
        // in-task, so the task-local set here is still visible when the
        // handler calls `DbPool::write()`.
        crate::actor::scope(
            crate::actor::ActorCtx {
                user_id: Some(lead_id),
                transport: crate::actor::Transport::Web,
            },
            async {
                json_post(
                    &lead_app,
                    &format!("/api/projects/{project_id}/members"),
                    serde_json::json!({ "user_id": non_member.id, "role": "viewer" }),
                )
                .await;
                json_patch(
                    &lead_app,
                    &format!("/api/projects/{project_id}/members/{}", non_member.id),
                    serde_json::json!({ "role": "maintainer" }),
                )
                .await;
                json_delete(
                    &lead_app,
                    &format!("/api/projects/{project_id}/members/{}", non_member.id),
                )
                .await;
            },
        )
        .await;

        let feed = parse_json(
            json_get(
                &lead_app,
                &format!("/api/projects/{project_id}/activity?limit=100"),
            )
            .await,
        )
        .await;
        let items = feed["items"].as_array().unwrap();
        // Scope to rows about the member we just added/changed/removed —
        // `setup_membership_test` seeds the lead/maintainer/viewer rows via
        // direct DB calls with no actor scope, so they'd read as 'system'
        // and pollute an unscoped "every member row" assertion.
        let member_rows: Vec<&serde_json::Value> = items
            .iter()
            .filter(|a| a["entity_type"] == "member" && a["entity_id"] == non_member.id)
            .collect();

        assert!(
            member_rows
                .iter()
                .any(|a| a["action"] == "create" && a["new_value"] == "viewer"),
            "expected a member create row: {member_rows:#?}"
        );
        assert!(
            member_rows.iter().any(|a| a["action"] == "update"
                && a["field"] == "role"
                && a["old_value"] == "viewer"
                && a["new_value"] == "maintainer"),
            "expected a member role-change row: {member_rows:#?}"
        );
        assert!(
            member_rows
                .iter()
                .any(|a| a["action"] == "delete" && a["old_value"] == "maintainer"),
            "expected a member delete row: {member_rows:#?}"
        );
        assert!(
            member_rows.iter().all(|a| a["actor_username"] == "lead"),
            "every membership change must be attributed to the acting user: {member_rows:#?}"
        );
    }

    // ── my-role (LIF-234) ────────────────────────────────────────

    #[tokio::test]
    async fn my_role_reports_each_members_own_role_when_enforced() {
        let (db, _admin, lead, maintainer, viewer, _non_member, project_id) =
            setup_membership_test();

        for (user, expected) in [
            (&lead, "lead"),
            (&maintainer, "maintainer"),
            (&viewer, "viewer"),
        ] {
            let app = app_as_user(db.clone(), user);
            let resp = json_get(&app, &format!("/api/projects/{project_id}/my-role")).await;
            assert_eq!(resp.status(), StatusCode::OK, "{} my-role", user.username);
            let body = parse_json(resp).await;
            assert_eq!(body["role"], expected, "{} role", user.username);
            assert_eq!(body["enforced"], true);
            assert_eq!(body["is_admin"], false);
        }
    }

    #[tokio::test]
    async fn my_role_denies_non_member_when_enforced() {
        // Same Viewer gate as every other project read: a non-member gets a
        // 403, which the client reads as "no access," never "full access."
        let (db, _admin, _lead, _maintainer, _viewer, non_member, project_id) =
            setup_membership_test();
        let app = app_as_user(db, &non_member);
        let resp = json_get(&app, &format!("/api/projects/{project_id}/my-role")).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn my_role_reports_admin_bypass_when_enforced() {
        // A workspace admin who holds no membership row still passes the
        // Viewer gate and is flagged `is_admin` so the UI stays fully
        // interactive.
        let (db, admin, _lead, _maintainer, _viewer, _non_member, project_id) =
            setup_membership_test();
        let app = app_as_user(db, &admin);
        let resp = json_get(&app, &format!("/api/projects/{project_id}/my-role")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = parse_json(resp).await;
        assert_eq!(body["is_admin"], true);
        assert_eq!(body["enforced"], true);
        // Admin has no membership row in this fixture → role is null; the
        // client relies on is_admin, not role, for the bypass.
        assert!(body["role"].is_null());
    }

    #[tokio::test]
    async fn my_role_reports_enforced_false_in_legacy_mode() {
        // Flag OFF (default): the endpoint reports enforced=false so the web
        // app keeps every affordance interactive, matching the server, which
        // still allows a non-lead everything in legacy mode.
        let (db, _admin, _lead, regular, project_id) = setup_lead_test();
        let app = app_as_user(db, &regular);
        let resp = json_get(&app, &format!("/api/projects/{project_id}/my-role")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = parse_json(resp).await;
        assert_eq!(body["enforced"], false);
        assert_eq!(body["is_admin"], false);
        // No membership row for a legacy-mode regular user → role null, but
        // enforced=false is what the client keys off.
        assert!(body["role"].is_null());
    }

    // ── Flag OFF: still lead/admin-gated; list allowed ─────────────

    #[tokio::test]
    async fn flag_off_writes_stay_lead_gated_list_stays_open() {
        let (db, admin, lead, regular, project_id) = setup_lead_test();

        // A random authenticated non-lead user is denied on write...
        let regular_app = app_as_user(db.clone(), &regular);
        let resp = json_post(
            &regular_app,
            &format!("/api/projects/{project_id}/members"),
            serde_json::json!({ "user_id": admin.id }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        // ...but the members list stays open (legacy `Viewer` = unconditional
        // allow), same as every other read.
        let resp = json_get(&regular_app, &format!("/api/projects/{project_id}/members")).await;
        assert_eq!(resp.status(), StatusCode::OK);

        // The lead (via lead_user_id, no project_members row required in
        // legacy mode) can add/patch/remove.
        let lead_app = app_as_user(db.clone(), &lead);
        let resp = json_post(
            &lead_app,
            &format!("/api/projects/{project_id}/members"),
            serde_json::json!({ "user_id": regular.id }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = json_patch(
            &lead_app,
            &format!("/api/projects/{project_id}/members/{}", regular.id),
            serde_json::json!({ "role": "maintainer" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = json_delete(
            &lead_app,
            &format!("/api/projects/{project_id}/members/{}", regular.id),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        // Admin (global override) can also manage, flag off.
        let admin_app = app_as_user(db, &admin);
        let resp = json_post(
            &admin_app,
            &format!("/api/projects/{project_id}/members"),
            serde_json::json!({ "user_id": regular.id }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
