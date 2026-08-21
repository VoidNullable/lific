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

use super::with_read;

/// GET /api/projects/{id}/members — visible to any project member
/// (`Viewer`+); non-members are denied same as any other project read.
pub(super) async fn list_project_members(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(project_id): Path<i64>,
) -> Result<Json<Vec<MemberWithUser>>, LificError> {
    authz::require_role(&db, &identity, project_id, Role::Viewer)?;
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
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(project_id): Path<i64>,
) -> Result<Json<serde_json::Value>, LificError> {
    authz::require_role(&db, &identity, project_id, Role::Viewer)?;

    let enforced = authz::authz_enforced(&db)?;
    let (role, is_admin) = with_read(&db, |conn| {
        let auth_user = identity.as_ref().map(|i| i.user.clone());
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

/// Parse a role name the way the DB's CHECK constraint does, so a comparison
/// against the current role is made on the same values the write will use.
fn parse_role(raw: &str) -> Result<Role, LificError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "viewer" => Ok(Role::Viewer),
        "maintainer" => Ok(Role::Maintainer),
        "lead" => Ok(Role::Lead),
        other => Err(LificError::BadRequest(format!("unknown role '{other}'"))),
    }
}

/// POST /api/projects/{id}/members — add a member. `role` defaults to
/// `viewer` when omitted (design: "default grant = viewer"). 409 if the
/// user is already a member (use `PATCH` to change an existing role), 404
/// if `user_id` doesn't resolve to a real user.
///
/// Requires a **recent browser session**. Adding somebody to a project grants
/// them standing access to its contents, which no lockdown on the granter's
/// credentials will take back, so it is an access expansion on the same terms
/// as minting a key. A leaked lead-scoped API key can no longer quietly add an
/// account the attacker controls.
///
/// The session revalidation and the lead check both run inside the write
/// transaction, so neither an account lockdown nor a role change can land
/// between the checks and the write.
pub(super) async fn add_project_member(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(project_id): Path<i64>,
    headers: axum::http::HeaderMap,
    Json(input): Json<AddMember>,
) -> Result<Json<ProjectMember>, LificError> {
    // Cheap pre-check on a read connection, so an unauthorized caller never
    // reaches the writer. Re-run authoritatively inside the transaction below.
    authz::require_role(&db, &identity, project_id, Role::Lead)?;
    let granter = super::require_user(&identity)?;
    let session_token = crate::auth::recent_session_token(&headers)?;
    let role = input.role.as_deref().unwrap_or("viewer").to_string();

    let member = db.transaction(|tx| {
        // A session token is always a human's, so the identity the middleware
        // resolved IS the session's user; `revalidate_recent_session` asserts
        // exactly that. Bot callers cannot reach here at all, because they do
        // not present a `lific_sess_` token.
        let fresh = crate::auth::revalidate_recent_session(tx, &session_token, granter.id)?;
        // The gate runs against the identity as it is *now*: the middleware's
        // copy carries an `is_admin` snapshot, and a lead membership revoked
        // since would still look present in it.
        let fresh_identity = Some(crate::auth::fresh_identity(
            &fresh,
            crate::actor::Transport::Web,
        ));
        authz::require_role_conn(tx, &fresh_identity, project_id, Role::Lead)?;
        members::add_member(tx, project_id, input.user_id, &role)
    })?;
    realtime.send(RealtimeEvent::ProjectUpdated { project_id });
    Ok(Json(member))
}

/// PATCH /api/projects/{id}/members/{user_id} — change an existing
/// member's role. 404 if they aren't a member; 409 if this would demote
/// the project's sole `lead`.
///
/// Requires a recent browser session **only when the new role is higher than
/// the current one**. Raising someone to maintainer or lead expands access and
/// is gated; lowering it or leaving it alone reduces access and is not, so an
/// automation holding a lead-scoped API key can still contain a compromise by
/// downgrading people, which is exactly what it should be able to do at 3am.
///
/// The current role is read inside the write transaction, so the
/// increase-or-not decision cannot be made against a stale value.
pub(super) async fn update_project_member(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path((project_id, user_id)): Path<(i64, i64)>,
    headers: axum::http::HeaderMap,
    Json(input): Json<ChangeMemberRole>,
) -> Result<Json<ProjectMember>, LificError> {
    authz::require_role(&db, &identity, project_id, Role::Lead)?;
    let requested = parse_role(&input.role)?;
    // Parsed before the transaction so a malformed body is a 400 rather than a
    // recent-auth refusal, but only *used* inside it.
    let session_token = crate::auth::recent_session_token(&headers).ok();
    let granter = super::require_user(&identity).ok();

    let member = db.transaction(|tx| {
        let current = members::get_member_role(tx, project_id, user_id)?;
        let is_increase = current.is_none_or(|role| requested > role);
        if is_increase {
            // A raise is a grant, so both the recency check and the lead check
            // run against freshly read state.
            let (Some(token), Some(user)) = (&session_token, &granter) else {
                return Err(LificError::Forbidden(
                    "recent authentication required".into(),
                ));
            };
            let fresh = crate::auth::revalidate_recent_session(tx, token, user.id)?;
            let fresh_identity = Some(crate::auth::fresh_identity(
                &fresh,
                crate::actor::Transport::Web,
            ));
            authz::require_role_conn(tx, &fresh_identity, project_id, Role::Lead)?;
        } else {
            // A reduction: no recency, but the lead check is re-run inside the
            // transaction *against freshly read state*. The middleware's
            // identity carries an `is_admin` snapshot, and `require_role_conn`
            // lets an admin through before it looks at membership at all, so a
            // demoted admin would otherwise keep downgrading people.
            let caller = super::require_user(&identity)?;
            let fresh = crate::auth::fresh_caller(tx, caller.id)?;
            let fresh_identity = Some(crate::auth::fresh_identity(
                &fresh,
                crate::actor::Transport::Web,
            ));
            authz::require_role_conn(tx, &fresh_identity, project_id, Role::Lead)?;
        }
        members::change_role(tx, project_id, user_id, &input.role)
    })?;
    realtime.send(RealtimeEvent::ProjectUpdated { project_id });
    Ok(Json(member))
}

/// DELETE /api/projects/{id}/members/{user_id} — remove a member. 404 if
/// they aren't a member; 409 if they're the project's sole `lead`.
///
/// Not gated on recency: removing a member only ever takes access away.
/// The lead check is re-run inside the write transaction so it cannot act on
/// an authorization that was revoked after the read-connection check.
pub(super) async fn remove_project_member(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path((project_id, user_id)): Path<(i64, i64)>,
) -> Result<Json<serde_json::Value>, LificError> {
    authz::require_role(&db, &identity, project_id, Role::Lead)?;
    let caller = super::require_user(&identity)?;
    db.transaction(|tx| {
        // Same reasoning as the downgrade path: the authoritative check reads
        // the caller inside the transaction, so a demoted admin or a removed
        // lead cannot act on a stale snapshot.
        let fresh = crate::auth::fresh_caller(tx, caller.id)?;
        let fresh_identity = Some(crate::auth::fresh_identity(
            &fresh,
            crate::actor::Transport::Web,
        ));
        authz::require_role_conn(tx, &fresh_identity, project_id, Role::Lead)?;
        members::remove_member_guarded(tx, project_id, user_id)
    })?;
    realtime.send(RealtimeEvent::ProjectUpdated { project_id });
    Ok(Json(serde_json::json!({"deleted": true})))
}

#[cfg(test)]
mod tests {
    use crate::api::test_helpers::*;
    use crate::db::models::{CreateUser, Role};
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

    /// The membership twin of
    /// `api::auth::tests::a_stale_admin_snapshot_cannot_expand_access`.
    ///
    /// `require_role_conn` lets an instance admin through before it looks at
    /// membership at all, and it reads `is_admin` off the identity it is
    /// handed. Given the middleware's snapshot, a demoted admin still walks
    /// through that door. The granting paths build a fresh identity from the
    /// session user read inside the transaction, so they do not.
    #[tokio::test]
    async fn a_stale_admin_snapshot_cannot_grant_project_access() {
        let (db, admin, lead, _maintainer, viewer, non_member, project_id) =
            setup_membership_test();
        // Snapshot taken while they were an admin.
        let app = app_as_user(db.clone(), &admin);
        {
            let conn = db.write().unwrap();
            // A second admin so the last-admin guard permits the demotion.
            crate::db::queries::users::create_user(
                &conn,
                &CreateUser {
                    username: "spare-admin".into(),
                    email: "spare@test.com".into(),
                    password: "testpassword1".into(),
                    display_name: None,
                    is_admin: true,
                    is_bot: false,
                },
            )
            .unwrap();
            crate::db::queries::users::set_admin_guarded(&conn, admin.id, false).unwrap();
        }

        let add = json_post(
            &app,
            &format!("/api/projects/{project_id}/members"),
            serde_json::json!({ "user_id": non_member.id, "role": "viewer" }),
        )
        .await;
        assert_eq!(add.status(), StatusCode::FORBIDDEN, "add");

        let raise = json_patch(
            &app,
            &format!("/api/projects/{project_id}/members/{}", viewer.id),
            serde_json::json!({ "role": "maintainer" }),
        )
        .await;
        assert_eq!(raise.status(), StatusCode::FORBIDDEN, "role increase");

        let name_lead = json_put(
            &app,
            &format!("/api/projects/{project_id}"),
            serde_json::json!({ "lead_user_id": non_member.id }),
        )
        .await;
        assert_eq!(name_lead.status(), StatusCode::FORBIDDEN, "naming a lead");

        let conn = db.read().unwrap();
        assert_eq!(
            crate::db::queries::members::get_member_role(&conn, project_id, non_member.id).unwrap(),
            None,
            "nobody was added"
        );
        assert_eq!(
            crate::db::queries::members::get_member_role(&conn, project_id, viewer.id).unwrap(),
            Some(Role::Viewer),
            "no role was raised"
        );
        let _ = lead;
    }

    // ── Granting access needs a recent sign-in ───────────────
    //
    // Adding somebody to a project, or raising their role, hands them standing
    // access that no lockdown on the granter's credentials takes back. Both
    // are therefore gated on a browser session authenticated in the last 15
    // minutes, and both are exercised here over the real
    // `auth::require_api_key` middleware with real bearer tokens, because the
    // question is what the middleware makes of each credential shape.
    mod recent_auth {
        use crate::api::test_helpers::*;
        use crate::db::DbPool;
        use crate::db::models::*;
        use axum::http::{Request, StatusCode};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        struct Fixture {
            db: DbPool,
            app: axum::Router,
            project_id: i64,
            lead: User,
            viewer: User,
            outsider: User,
            /// A live browser session for the lead.
            session: String,
            /// An API key bound to the lead. Authenticates, leads the project.
            lead_key: String,
            /// An OAuth token bound to the lead.
            lead_oauth: String,
        }

        fn fixture() -> Fixture {
            let (db, _admin, lead, _maintainer, viewer, outsider, project_id) =
                setup_membership_test();
            let manager = crate::auth::create_key_manager().unwrap();
            let session = {
                let conn = db.write().unwrap();
                crate::db::queries::users::create_session(&conn, lead.id, None)
                    .unwrap()
                    .token
            };
            let lead_key =
                crate::auth::create_api_key(&db, &manager, "lead-key", Some(lead.id)).unwrap();
            let lead_oauth = {
                let token = "lific_at_lead".to_string();
                let hash = crate::auth::sha256_hex(token.as_bytes());
                let expires = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
                let conn = db.write().unwrap();
                conn.execute(
                    "INSERT INTO oauth_clients (client_id, client_name, redirect_uris)
                     VALUES ('c', 'Test', '[\"http://localhost\"]')",
                    [],
                )
                .unwrap();
                conn.execute(
                    "INSERT INTO oauth_tokens (access_token, client_id, expires_at, scope, user_id)
                     VALUES (?1, 'c', ?2, 'mcp', ?3)",
                    rusqlite::params![hash, expires, lead.id],
                )
                .unwrap();
                token
            };

            let auth_state = crate::auth::AuthState {
                db: db.clone(),
                manager,
                public_url: "https://example.com".into(),
                required: true,
            };
            let app = crate::api::router(db.clone(), &[])
                .layer(axum::Extension(crate::realtime::RealtimeHub::new()))
                .layer(axum::Extension(crate::config::AuthConfig {
                    allow_signup: true,
                    required: true,
                    secure_cookies: false,
                }))
                .layer(axum::middleware::from_fn_with_state(
                    auth_state,
                    crate::auth::require_api_key,
                ));

            Fixture {
                db,
                app,
                project_id,
                lead,
                viewer,
                outsider,
                session,
                lead_key,
                lead_oauth,
            }
        }

        async fn send(
            f: &Fixture,
            method: &str,
            uri: &str,
            token: &str,
            body: Option<serde_json::Value>,
        ) -> (StatusCode, serde_json::Value) {
            let builder = Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json");
            let request = match &body {
                Some(value) => builder
                    .body(axum::body::Body::from(serde_json::to_vec(value).unwrap()))
                    .unwrap(),
                None => builder.body(axum::body::Body::empty()).unwrap(),
            };
            let response = f.app.clone().oneshot(request).await.unwrap();
            let status = response.status();
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            (
                status,
                serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
            )
        }

        fn role_of(f: &Fixture, user_id: i64) -> Option<Role> {
            crate::db::queries::members::get_member_role(
                &f.db.read().unwrap(),
                f.project_id,
                user_id,
            )
            .unwrap()
        }

        fn age_sessions(f: &Fixture) {
            f.db.write()
                .unwrap()
                .execute(
                    "UPDATE sessions SET created_at = datetime('now', '-16 minutes')",
                    [],
                )
                .unwrap();
        }

        async fn add_outsider(f: &Fixture, token: &str) -> StatusCode {
            send(
                f,
                "POST",
                &format!("/api/projects/{}/members", f.project_id),
                token,
                Some(serde_json::json!({ "user_id": f.outsider.id, "role": "viewer" })),
            )
            .await
            .0
        }

        async fn set_role(f: &Fixture, token: &str, user_id: i64, role: &str) -> StatusCode {
            send(
                f,
                "PATCH",
                &format!("/api/projects/{}/members/{}", f.project_id, user_id),
                token,
                Some(serde_json::json!({ "role": role })),
            )
            .await
            .0
        }

        #[tokio::test]
        async fn a_recent_session_may_add_a_member_and_raise_a_role() {
            let f = fixture();
            assert_eq!(add_outsider(&f, &f.session).await, StatusCode::OK);
            assert_eq!(role_of(&f, f.outsider.id), Some(Role::Viewer));

            assert_eq!(
                set_role(&f, &f.session, f.viewer.id, "maintainer").await,
                StatusCode::OK
            );
            assert_eq!(role_of(&f, f.viewer.id), Some(Role::Maintainer));
        }

        #[tokio::test]
        async fn an_aged_session_may_not_add_a_member_or_raise_a_role() {
            let f = fixture();
            age_sessions(&f);

            assert_eq!(add_outsider(&f, &f.session).await, StatusCode::FORBIDDEN);
            assert_eq!(role_of(&f, f.outsider.id), None, "nobody was added");
            assert_eq!(
                set_role(&f, &f.session, f.viewer.id, "maintainer").await,
                StatusCode::FORBIDDEN
            );
            assert_eq!(role_of(&f, f.viewer.id), Some(Role::Viewer), "unchanged");
        }

        /// A lead-scoped API key still leads the project, so this is a policy
        /// refusal rather than an authorization failure. It is exactly the
        /// credential the rule is aimed at: one that survives being stolen.
        #[tokio::test]
        async fn a_lead_api_key_may_not_add_a_member_or_raise_a_role() {
            let f = fixture();
            let (status, _) = send(
                &f,
                "GET",
                &format!("/api/projects/{}/members", f.project_id),
                &f.lead_key,
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "the key does lead this project");

            assert_eq!(add_outsider(&f, &f.lead_key).await, StatusCode::FORBIDDEN);
            assert_eq!(role_of(&f, f.outsider.id), None);
            assert_eq!(
                set_role(&f, &f.lead_key, f.viewer.id, "maintainer").await,
                StatusCode::FORBIDDEN
            );
            assert_eq!(role_of(&f, f.viewer.id), Some(Role::Viewer));
        }

        #[tokio::test]
        async fn an_oauth_token_may_not_add_a_member_or_raise_a_role() {
            let f = fixture();
            assert_eq!(add_outsider(&f, &f.lead_oauth).await, StatusCode::FORBIDDEN);
            assert_eq!(role_of(&f, f.outsider.id), None);
            assert_eq!(
                set_role(&f, &f.lead_oauth, f.viewer.id, "maintainer").await,
                StatusCode::FORBIDDEN
            );
            assert_eq!(role_of(&f, f.viewer.id), Some(Role::Viewer));
        }

        /// Containment must stay fast. An automation holding a lead key can
        /// still downgrade and remove people at 3am, from an aged session or
        /// no session at all.
        #[tokio::test]
        async fn reductions_need_no_recent_authentication() {
            let f = fixture();
            age_sessions(&f);

            // Raise first, with the one credential allowed to.
            {
                let conn = f.db.write().unwrap();
                crate::db::queries::members::change_role(
                    &conn,
                    f.project_id,
                    f.viewer.id,
                    "maintainer",
                )
                .unwrap();
            }

            assert_eq!(
                set_role(&f, &f.lead_key, f.viewer.id, "viewer").await,
                StatusCode::OK,
                "a downgrade is a reduction"
            );
            assert_eq!(role_of(&f, f.viewer.id), Some(Role::Viewer));

            // Same role again is not an increase either.
            assert_eq!(
                set_role(&f, &f.lead_key, f.viewer.id, "viewer").await,
                StatusCode::OK
            );

            let (status, _) = send(
                &f,
                "DELETE",
                &format!("/api/projects/{}/members/{}", f.project_id, f.viewer.id),
                &f.lead_key,
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "removal is a reduction");
            assert_eq!(role_of(&f, f.viewer.id), None);
        }

        /// `PATCH /api/projects/{id}` with `lead_user_id` upserts a `lead`
        /// membership for whoever is named (LIF-195). That is a membership
        /// grant wearing a project-settings hat, so it carries the same rule
        /// as `POST .../members`. Renaming the project does not.
        #[tokio::test]
        async fn naming_a_project_lead_is_a_grant_and_needs_a_recent_session() {
            let f = fixture();
            let uri = format!("/api/projects/{}", f.project_id);

            // A rename is not an expansion: an aged session may do it.
            age_sessions(&f);
            let (status, _) = send(
                &f,
                "PUT",
                &uri,
                &f.session,
                Some(serde_json::json!({ "name": "Renamed" })),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "a rename grants nothing");

            // Naming a lead is.
            for token in [&f.session, &f.lead_key, &f.lead_oauth] {
                let (status, _) = send(
                    &f,
                    "PUT",
                    &uri,
                    token,
                    Some(serde_json::json!({ "lead_user_id": f.outsider.id })),
                )
                .await;
                assert_eq!(status, StatusCode::FORBIDDEN);
                assert_eq!(role_of(&f, f.outsider.id), None, "no membership was minted");
            }

            // Clearing the lead is a reduction and stays open.
            let (status, body) = send(
                &f,
                "PUT",
                &uri,
                &f.lead_key,
                Some(serde_json::json!({ "lead_user_id": null })),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
        }

        #[tokio::test]
        async fn a_recent_session_may_name_a_project_lead() {
            let f = fixture();
            let (status, body) = send(
                &f,
                "PUT",
                &format!("/api/projects/{}", f.project_id),
                &f.session,
                Some(serde_json::json!({ "lead_user_id": f.outsider.id })),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(role_of(&f, f.outsider.id), Some(Role::Lead));
        }

        // ── Project creation and its implied lead grant ──────

        async fn create_project_led_by(
            f: &Fixture,
            token: &str,
            identifier: &str,
            lead: Option<i64>,
        ) -> (StatusCode, serde_json::Value) {
            let mut body = serde_json::json!({
                "name": format!("Project {identifier}"),
                "identifier": identifier,
            });
            if let Some(lead) = lead {
                body["lead_user_id"] = serde_json::json!(lead);
            }
            send(f, "POST", "/api/projects", token, Some(body)).await
        }

        fn lead_of(f: &Fixture, project_id: i64) -> Option<i64> {
            f.db.read()
                .unwrap()
                .query_row(
                    "SELECT lead_user_id FROM projects WHERE id = ?1",
                    rusqlite::params![project_id],
                    |r| r.get(0),
                )
                .unwrap()
        }

        /// The ordinary case, and the one automation depends on: an API key
        /// creating its own project. It leads it, and nothing asks for a
        /// password.
        #[tokio::test]
        async fn an_api_key_may_still_create_a_project_it_leads() {
            let f = fixture();
            age_sessions(&f);

            let (status, body) = create_project_led_by(&f, &f.lead_key, "OWN", None).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            let created = body["id"].as_i64().unwrap();
            assert_eq!(
                lead_of(&f, created),
                Some(f.lead.id),
                "the creator leads it by default"
            );

            // Naming yourself explicitly is the same grant, so also fine.
            let (status, body) =
                create_project_led_by(&f, &f.lead_key, "SELF", Some(f.lead.id)).await;
            assert_eq!(status, StatusCode::OK, "{body}");
        }

        /// Naming somebody else hands them a lead membership that outlives any
        /// lockdown on the creating credential, so it needs a recent human.
        #[tokio::test]
        async fn a_key_or_token_may_not_create_a_project_led_by_someone_else() {
            let f = fixture();
            for token in [&f.lead_key, &f.lead_oauth] {
                let (status, body) =
                    create_project_led_by(&f, token, "OTHER", Some(f.outsider.id)).await;
                assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
            }

            let conn = f.db.read().unwrap();
            let projects: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM projects WHERE identifier = 'OTHER'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(projects, 0, "no project, and so no membership, was created");
            let memberships: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM project_members WHERE user_id = ?1",
                    rusqlite::params![f.outsider.id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(memberships, 0);
        }

        #[tokio::test]
        async fn an_aged_session_may_not_create_a_project_led_by_someone_else() {
            let f = fixture();
            age_sessions(&f);
            let (status, _) =
                create_project_led_by(&f, &f.session, "AGED", Some(f.outsider.id)).await;
            assert_eq!(status, StatusCode::FORBIDDEN);
        }

        #[tokio::test]
        async fn a_recent_session_may_create_a_project_led_by_someone_else() {
            let f = fixture();
            let (status, body) =
                create_project_led_by(&f, &f.session, "GIFT", Some(f.outsider.id)).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            let created = body["id"].as_i64().unwrap();
            assert_eq!(lead_of(&f, created), Some(f.outsider.id));
            assert_eq!(
                crate::db::queries::members::get_member_role(
                    &f.db.read().unwrap(),
                    created,
                    f.outsider.id
                )
                .unwrap(),
                Some(Role::Lead),
            );
        }

        // ── Reductions read their authorization freshly too ──

        /// A demoted admin must not be able to downgrade, remove, clear a lead
        /// or delete a project on the strength of the middleware snapshot.
        /// These are reductions, so they are *not* recency-gated; the point is
        /// that their authorization is still read inside the transaction.
        #[tokio::test]
        async fn a_stale_admin_snapshot_cannot_perform_reductions() {
            let (db, admin, _lead, _maintainer, viewer, _non_member, project_id) =
                setup_membership_test();
            let app = app_as_user(db.clone(), &admin);
            {
                let conn = db.write().unwrap();
                crate::db::queries::users::create_user(
                    &conn,
                    &CreateUser {
                        username: "spare-admin".into(),
                        email: "spare2@test.com".into(),
                        password: "testpassword1".into(),
                        display_name: None,
                        is_admin: true,
                        is_bot: false,
                    },
                )
                .unwrap();
                crate::db::queries::members::change_role(
                    &conn,
                    project_id,
                    viewer.id,
                    "maintainer",
                )
                .unwrap();
                crate::db::queries::users::set_admin_guarded(&conn, admin.id, false).unwrap();
            }

            let downgrade = json_patch(
                &app,
                &format!("/api/projects/{project_id}/members/{}", viewer.id),
                serde_json::json!({ "role": "viewer" }),
            )
            .await;
            assert_eq!(downgrade.status(), StatusCode::FORBIDDEN, "downgrade");

            let remove = json_delete(
                &app,
                &format!("/api/projects/{project_id}/members/{}", viewer.id),
            )
            .await;
            assert_eq!(remove.status(), StatusCode::FORBIDDEN, "remove");

            let clear_lead = json_put(
                &app,
                &format!("/api/projects/{project_id}"),
                serde_json::json!({ "lead_user_id": null }),
            )
            .await;
            assert_eq!(clear_lead.status(), StatusCode::FORBIDDEN, "clear the lead");

            let rename = json_put(
                &app,
                &format!("/api/projects/{project_id}"),
                serde_json::json!({ "name": "Renamed" }),
            )
            .await;
            assert_eq!(rename.status(), StatusCode::FORBIDDEN, "rename");

            let delete = json_delete(&app, &format!("/api/projects/{project_id}")).await;
            assert_eq!(delete.status(), StatusCode::FORBIDDEN, "delete the project");

            // Nothing moved.
            let conn = db.read().unwrap();
            assert_eq!(
                crate::db::queries::members::get_member_role(&conn, project_id, viewer.id).unwrap(),
                Some(Role::Maintainer),
            );
            let still_there: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM projects WHERE id = ?1",
                    rusqlite::params![project_id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(still_there, 1, "the project survived");
        }

        /// The lead check is re-run inside the write transaction, so an
        /// authorization revoked after the request arrived cannot be acted on.
        #[tokio::test]
        async fn losing_lead_between_requests_stops_the_next_grant() {
            let f = fixture();
            assert_eq!(add_outsider(&f, &f.session).await, StatusCode::OK);

            {
                // Promote the newcomer so the last-lead guard lets the
                // original lead step down, then step them down.
                let conn = f.db.write().unwrap();
                crate::db::queries::members::change_role(
                    &conn,
                    f.project_id,
                    f.outsider.id,
                    "lead",
                )
                .unwrap();
                crate::db::queries::members::change_role(&conn, f.project_id, f.lead.id, "viewer")
                    .unwrap();
            }

            assert_eq!(
                set_role(&f, &f.session, f.viewer.id, "maintainer").await,
                StatusCode::FORBIDDEN,
                "a former lead may not raise anyone"
            );
            assert_eq!(role_of(&f, f.viewer.id), Some(Role::Viewer), "unchanged");
        }
    }
}
