//! LIF-196: the single project-scoped authorization primitive shared by REST
//! (LIF-197) and MCP (LIF-198), so the two transports can't drift. Design:
//! LIF-DOC-7. Part of epic LIF-194.
//!
//! ## Two modes, one runtime flag
//!
//! Enforcement is gated by the instance setting `authz_enforced` (migration
//! 027 — see [`db::queries::settings`]). This mirrors how `web_auto_login`
//! (LIF-215) and `allow_signup` (LIF-211) are read: a plain column on the
//! single `instance_settings` row, read fresh on every check so flipping it
//! takes effect without a restart.
//!
//! LIF-261: the *seed* default is install-dependent. `settings::ensure` turns
//! it ON for a fresh install (zero users at row-creation time) and OFF for an
//! upgrade (users already exist), then the row is authoritative forever. The
//! zero-user agent-first flow survives the default-on because unbound API keys
//! are operator-trusted (see below).
//!
//! - **Flag off (legacy mode)** — reproduces today's behavior exactly:
//!   `min = Lead` runs the pre-LIF-194 `require_project_lead` check
//!   (`projects.lead_user_id`, plus the LIF-102 unowned-project carve-out);
//!   `min = Maintainer` / `min = Viewer` allow any request, authenticated or
//!   not. No new denials are introduced in this mode.
//! - **Flag on (default-deny mode)** — resolves the effective role from
//!   `project_members` and requires `role >= min`. A `None` auth user or a
//!   non-member is always `Forbidden`, even for `min = Viewer`.
//!
//! `is_admin` bypasses both modes unconditionally. Bots resolve to their
//! owner (`007_bot_owners.sql`) before either check runs, in both modes — an
//! agent can never exceed the human that owns it.
//!
//! ## Operator-key trust (LIF-261)
//!
//! Enforced mode is default-deny for a `None` effective user. That alone would
//! brick the zero-user agent-first flow (`lific init` → `start` → `connect`),
//! which runs on **user-unbound API keys** that resolve to `AuthUser = None`.
//! Those keys can only be minted with shell access to the server (`lific
//! start`'s first-run auto-key, `lific key create`, `connect`'s fresh-install
//! path), so they're operator-trusted by construction — the threat the default
//! guards against is a web-signup stranger with a session/OAuth token, not the
//! operator's own shell-minted key.
//!
//! So enforced mode treats an **unbound API key** as admin-equivalent. The
//! signal is credential-type-specific and comes only from the auth layer's
//! unbound-API-key path — it is deliberately NOT "any `None`," because a legacy
//! pre-binding OAuth token (`src/auth.rs`) also resolves to `None` and must
//! stay default-denied. The auth middleware sets [`operator_scope`] (REST) or
//! `mcp::with_request_user`'s operator flag (MCP); [`operator_context`] reads
//! whichever surface is active. **Unbound API keys therefore bypass authz by
//! design; audit them with `lific key list`.**

use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::models::{AuthUser, Role};
use crate::db::{DbPool, queries};
use crate::error::LificError;

// ── Operator-key trust signal (LIF-261) ─────────────────────────

tokio::task_local! {
    /// REST-side, request-scoped "the credential is an operator-trusted
    /// unbound API key" flag. Set by `auth::require_api_key` via
    /// [`operator_scope`] around the downstream handler, which runs in the
    /// same task — so ambient reads in `require_role` see it. MCP can't use a
    /// task-local (rmcp spawns internal tasks that drop it), so it mirrors the
    /// flag into `mcp::current_is_operator()` instead.
    static OPERATOR: bool;
}

/// Run `fut` with the REST operator flag in scope. `auth::require_api_key`
/// wraps the unbound-API-key request path in this so `authz`'s ambient
/// [`operator_context`] reads `true` for the duration of the request.
pub async fn operator_scope<F: std::future::Future>(is_operator: bool, fut: F) -> F::Output {
    OPERATOR.scope(is_operator, fut).await
}

/// Whether the current request's credential is an operator-trusted unbound
/// API key. Checks the REST task-local first, then the MCP request global —
/// exactly one is ever set for a given request. Defaults to `false`
/// (including every synchronous / CLI / test context that sets neither), so
/// no code path silently gains operator power without an explicit signal.
fn operator_context() -> bool {
    OPERATOR.try_with(|o| *o).unwrap_or(false) || crate::mcp::current_is_operator()
}

// ── Bot → owner resolution ──────────────────────────────────────

/// Resolve the identity whose permissions actually govern this request.
///
/// A bot (bearer-token-authenticated tool connection) inherits its owning
/// human's role — it never exceeds what its owner can do. An ownerless bot
/// (shouldn't normally happen, but the FK allows `owner_id IS NULL`) is
/// evaluated as itself. Non-bot users pass through unchanged. Returns `None`
/// only when `auth_user` itself is `None`.
pub fn effective_user(conn: &Connection, auth_user: &Option<AuthUser>) -> Option<AuthUser> {
    let user = auth_user.as_ref()?;

    // Some(Some(owner_id)) = bot with an owner
    // Some(None)           = bot with no owner
    // None                 = not a bot (or the id doesn't exist)
    let bot_owner: Option<Option<i64>> = conn
        .query_row(
            "SELECT owner_id FROM users WHERE id = ?1 AND is_bot = 1",
            params![user.id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .optional()
        .ok()
        .flatten();

    match bot_owner {
        Some(Some(owner_id)) => queries::users::get_user_by_id(conn, owner_id)
            .ok()
            .map(|owner| AuthUser {
                id: owner.id,
                username: owner.username,
                display_name: owner.display_name,
                is_admin: owner.is_admin,
            })
            // Dangling owner_id (owner deleted without cascading the bot):
            // fall back to evaluating the bot as itself rather than erroring.
            .or_else(|| Some(user.clone())),
        _ => Some(user.clone()),
    }
}

// ── Instance setting read ───────────────────────────────────────

fn authz_enforced_conn(conn: &Connection) -> Result<bool, LificError> {
    Ok(queries::settings::get(conn)?.authz_enforced)
}

/// Whether project-scoped default-deny authorization is currently on.
/// Reads the live `instance_settings` row, so a runtime toggle takes effect
/// on the very next call — no restart required. Exposed for LIF-197/LIF-198
/// call sites that need to branch on the mode directly.
pub fn authz_enforced(db: &DbPool) -> Result<bool, LificError> {
    let conn = db.read()?;
    authz_enforced_conn(&conn)
}

// ── require_role ─────────────────────────────────────────────────

fn insufficient_role(min: Role) -> LificError {
    LificError::Forbidden(format!(
        "requires at least '{}' access to this project",
        min.as_str()
    ))
}

/// Require the effective caller to hold at least `min` role on `project_id`.
/// `Ok(())` = allowed. See the module docs for the legacy-vs-enforced
/// semantics; `is_admin` always short-circuits to allow in both modes.
pub fn require_role(
    db: &DbPool,
    auth_user: &Option<AuthUser>,
    project_id: i64,
    min: Role,
) -> Result<(), LificError> {
    let conn = db.read()?;
    require_role_conn(&conn, auth_user, project_id, min)
}

fn require_role_conn(
    conn: &Connection,
    auth_user: &Option<AuthUser>,
    project_id: i64,
    min: Role,
) -> Result<(), LificError> {
    require_role_conn_op(conn, auth_user, project_id, min, operator_context())
}

/// Same as [`require_role_conn`] but with the operator signal passed
/// explicitly, so tests can exercise both the operator and non-operator paths
/// deterministically without an ambient task-local / MCP global.
fn require_role_conn_op(
    conn: &Connection,
    auth_user: &Option<AuthUser>,
    project_id: i64,
    min: Role,
    is_operator: bool,
) -> Result<(), LificError> {
    let effective = effective_user(conn, auth_user);

    // Admin — resolved *after* bot→owner inheritance — always wins, in
    // both modes.
    if matches!(&effective, Some(u) if u.is_admin) {
        return Ok(());
    }

    if authz_enforced_conn(conn)? {
        // LIF-261: an operator-trusted unbound API key is admin-equivalent in
        // enforced mode. This is gated on `is_operator` (a credential-type
        // signal from the auth layer), NOT on `effective` being `None`, so a
        // legacy unbound OAuth token — which is also `None` here — still falls
        // through to the default-deny check below.
        if is_operator {
            return Ok(());
        }
        require_role_enforced(conn, &effective, project_id, min)
    } else {
        require_role_legacy(conn, &effective, project_id, min)
    }
}

/// Default-deny mode: resolve the membership row and compare against `min`.
fn require_role_enforced(
    conn: &Connection,
    effective: &Option<AuthUser>,
    project_id: i64,
    min: Role,
) -> Result<(), LificError> {
    let Some(user) = effective else {
        return Err(insufficient_role(min));
    };
    match queries::members::get_member_role(conn, project_id, user.id)? {
        Some(role) if role >= min => Ok(()),
        _ => Err(insufficient_role(min)),
    }
}

/// Legacy mode: reproduces today's exact behavior. Only `min = Lead` can
/// ever deny; `Maintainer`/`Viewer` allow any request, matching every
/// existing (non-lead-gated) REST route today.
fn require_role_legacy(
    conn: &Connection,
    effective: &Option<AuthUser>,
    project_id: i64,
    min: Role,
) -> Result<(), LificError> {
    match min {
        Role::Lead => require_lead_legacy(conn, effective, project_id),
        Role::Maintainer | Role::Viewer => Ok(()),
    }
}

/// The pre-LIF-194 `require_project_lead` check, preserved bit-for-bit
/// (see `api/mod.rs`'s original implementation and its LIF-102 doc comment),
/// with one additive extension: a `lead` `project_members` row also passes,
/// so a co-lead added purely via membership (not `projects.lead_user_id`)
/// isn't locked out even while enforcement is off. This never changes the
/// outcome for any project that only has the classic `lead_user_id` pointer,
/// which is every project in the existing test suite.
fn require_lead_legacy(
    conn: &Connection,
    effective: &Option<AuthUser>,
    project_id: i64,
) -> Result<(), LificError> {
    const DENIED: &str = "only the project lead or an admin can do this";
    const NO_LEAD: &str = "this project has no lead — only an admin can edit it";

    let Some(user) = effective else {
        return Err(LificError::Forbidden(DENIED.into()));
    };

    let project = queries::get_project(conn, project_id)?;
    if project.lead_user_id == Some(user.id) {
        return Ok(());
    }
    if queries::members::get_member_role(conn, project_id, user.id)? == Some(Role::Lead) {
        return Ok(());
    }

    if project.lead_user_id.is_some() {
        Err(LificError::Forbidden(DENIED.into()))
    } else {
        Err(LificError::Forbidden(NO_LEAD.into()))
    }
}

/// LIF-197: gate for the module/label/folder ("structure") REST endpoints.
/// These were lead-gated pre-LIF-194 via `require_project_lead`
/// (`api::require_project_lead`, itself `require_role(.., Lead)`). The
/// design loosens them to `Maintainer` once enforcement is on, but flag-OFF
/// must keep reproducing the exact pre-existing Lead gate — if this just
/// called `require_role(.., Maintainer)` unconditionally, the legacy branch
/// of `require_role` treats `Maintainer` as an unconditional allow (see
/// `require_role_legacy`), which would *regress* today's lead-only structure
/// endpoints the moment this function is wired in, flag or no flag. So the
/// two modes call `require_role` with two different `min` values instead of
/// letting `require_role` infer it:
/// - enforced: `require_role(.., Maintainer)` — the new, looser bar.
/// - legacy: `require_role(.., Lead)` — literally what `require_project_lead`
///   already does, unchanged.
pub fn require_structure_role(
    db: &DbPool,
    auth_user: &Option<AuthUser>,
    project_id: i64,
) -> Result<(), LificError> {
    if authz_enforced(db)? {
        require_role(db, auth_user, project_id, Role::Maintainer)
    } else {
        require_role(db, auth_user, project_id, Role::Lead)
    }
}

/// LIF-197: gate for `DELETE /api/projects/{id}`. Pre-LIF-194 this was
/// `require_admin` — a strict `auth_user.is_admin` check with **no** project
/// involvement at all (notably stricter than `require_lead_legacy`, which
/// also lets a project's lead through). Design decision #6 loosens deletion
/// to `Lead` once enforcement is on, but flag-OFF must keep the exact
/// admin-only behavior — routing it through `require_role(.., Lead)`
/// unconditionally would let a project lead delete their own project even
/// with the flag off, which never worked before. So legacy mode reproduces
/// `require_admin` verbatim here (deliberately not `effective_user`-aware:
/// the pre-existing check never resolved bot ownership either).
pub fn require_project_delete_role(
    db: &DbPool,
    auth_user: &Option<AuthUser>,
    project_id: i64,
) -> Result<(), LificError> {
    if authz_enforced(db)? {
        require_role(db, auth_user, project_id, Role::Lead)
    } else {
        match auth_user {
            Some(user) if user.is_admin => Ok(()),
            _ => Err(LificError::Forbidden("only an admin can do this".into())),
        }
    }
}

/// LIF-197: gate for mutations/reads on workspace-level (project-less)
/// pages — the only entity that can have `project_id = NULL` (plans always
/// require a project; see `CreatePlan.project_id: i64`). Design decision
/// #10: admin-only once enforcement is on. Legacy mode has never gated these
/// at all (no `require_*` call existed on the pre-LIF-194 page handlers), so
/// flag-off stays a no-op here to avoid a behavior change.
pub fn require_workspace_admin(
    db: &DbPool,
    auth_user: &Option<AuthUser>,
) -> Result<(), LificError> {
    if !authz_enforced(db)? {
        return Ok(());
    }
    // LIF-261: operator-trusted unbound API keys are admin-equivalent.
    if operator_context() {
        return Ok(());
    }
    let conn = db.read()?;
    let effective = effective_user(&conn, auth_user);
    match &effective {
        Some(u) if u.is_admin => Ok(()),
        _ => Err(LificError::Forbidden(
            "only an admin can access workspace-level pages".into(),
        )),
    }
}

// ── visible_project_ids ──────────────────────────────────────────

/// The cross-project read filter for search / project listing / any
/// workspace-spanning read (LIF-197/LIF-198 call sites).
///
/// `None` = unrestricted — caller should apply no filter at all. Returned
/// for admins, operator-trusted unbound API keys (LIF-261), and whenever
/// enforcement is off (legacy mode has no concept of hidden projects).
/// `Some(ids)` = only these project ids are visible: the effective caller's
/// memberships (any role), or the empty set for a `None` auth user / a member
/// of nothing.
pub fn visible_project_ids(
    db: &DbPool,
    auth_user: &Option<AuthUser>,
) -> Result<Option<HashSet<i64>>, LificError> {
    let conn = db.read()?;

    let effective = effective_user(&conn, auth_user);
    if matches!(&effective, Some(u) if u.is_admin) {
        return Ok(None);
    }
    if !authz_enforced_conn(&conn)? {
        return Ok(None);
    }
    // LIF-261: an operator-trusted unbound API key sees everything, like an
    // admin. Gated on the credential-type signal, not on `effective` being
    // `None` — a legacy unbound OAuth token stays scoped to the empty set.
    if operator_context() {
        return Ok(None);
    }
    let Some(user) = effective else {
        return Ok(Some(HashSet::new()));
    };

    let ids = queries::members::list_project_ids_for_user(&conn, user.id)?;
    Ok(Some(ids.into_iter().collect()))
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{CreateProject, CreateUser};
    use crate::db::queries::members::upsert_member;
    use crate::db::queries::settings::{InstanceSettingsPatch, update as update_settings};
    use crate::db::{self, queries};

    fn test_db() -> DbPool {
        db::open_memory().expect("test db")
    }

    fn seed_user(conn: &Connection, username: &str, is_admin: bool) -> AuthUser {
        let user = queries::users::create_user(
            conn,
            &CreateUser {
                username: username.into(),
                email: format!("{username}@test.local"),
                password: "testpassword1".into(),
                display_name: None,
                is_admin,
                is_bot: false,
            },
        )
        .unwrap();
        AuthUser {
            id: user.id,
            username: user.username,
            display_name: user.display_name,
            is_admin: user.is_admin,
        }
    }

    /// Seed a bot user owned by `owner_id` (or ownerless when `None`).
    fn seed_bot(conn: &Connection, username: &str, owner_id: Option<i64>) -> AuthUser {
        match owner_id {
            Some(owner) => {
                let bot = queries::users::create_bot_user(conn, owner, username, username).unwrap();
                AuthUser {
                    id: bot.id,
                    username: bot.username,
                    display_name: bot.display_name,
                    is_admin: bot.is_admin,
                }
            }
            None => {
                // create_bot_user requires an owner; insert an ownerless bot directly.
                conn.execute(
                    "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot, owner_id)
                     VALUES (?1, ?2, 'x', ?1, 0, 1, NULL)",
                    rusqlite::params![username, format!("{username}@bot.local")],
                )
                .unwrap();
                let id = conn.last_insert_rowid();
                AuthUser {
                    id,
                    username: username.into(),
                    display_name: username.into(),
                    is_admin: false,
                }
            }
        }
    }

    fn seed_project(conn: &Connection, ident: &str) -> i64 {
        queries::create_project(
            conn,
            &CreateProject {
                name: format!("Project {ident}"),
                identifier: ident.into(),
                description: String::new(),
                emoji: None,
                lead_user_id: None,
            },
        )
        .unwrap()
        .id
    }

    fn enable_enforcement(conn: &Connection) {
        update_settings(
            conn,
            InstanceSettingsPatch { authz_enforced: Some(true), ..Default::default() },
        )
        .unwrap();
    }

    // ── Enforced mode (flag ON) ──────────────────────────────────

    #[test]
    fn enforced_non_member_denied_at_every_level() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        enable_enforcement(&conn);
        let project = seed_project(&conn, "ENF");
        let outsider = seed_user(&conn, "outsider", false);

        for min in [Role::Viewer, Role::Maintainer, Role::Lead] {
            let err = require_role_conn(&conn, &Some(outsider.clone()), project, min).unwrap_err();
            assert!(matches!(err, LificError::Forbidden(_)), "denied at {min} got {err:?}");
        }
    }

    #[test]
    fn enforced_viewer_allowed_viewer_denied_maintainer_and_lead() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        enable_enforcement(&conn);
        let project = seed_project(&conn, "VWR");
        let viewer = seed_user(&conn, "viewer", false);
        upsert_member(&conn, project, viewer.id, Role::Viewer).unwrap();

        assert!(require_role_conn(&conn, &Some(viewer.clone()), project, Role::Viewer).is_ok());
        assert!(require_role_conn(&conn, &Some(viewer.clone()), project, Role::Maintainer).is_err());
        assert!(require_role_conn(&conn, &Some(viewer.clone()), project, Role::Lead).is_err());
    }

    #[test]
    fn enforced_maintainer_allowed_viewer_and_maintainer_denied_lead() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        enable_enforcement(&conn);
        let project = seed_project(&conn, "MNT");
        let maintainer = seed_user(&conn, "maintainer", false);
        upsert_member(&conn, project, maintainer.id, Role::Maintainer).unwrap();

        assert!(require_role_conn(&conn, &Some(maintainer.clone()), project, Role::Viewer).is_ok());
        assert!(require_role_conn(&conn, &Some(maintainer.clone()), project, Role::Maintainer).is_ok());
        assert!(require_role_conn(&conn, &Some(maintainer.clone()), project, Role::Lead).is_err());
    }

    #[test]
    fn enforced_lead_allowed_all_levels() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        enable_enforcement(&conn);
        let project = seed_project(&conn, "LED");
        let lead = seed_user(&conn, "lead", false);
        upsert_member(&conn, project, lead.id, Role::Lead).unwrap();

        for min in [Role::Viewer, Role::Maintainer, Role::Lead] {
            assert!(require_role_conn(&conn, &Some(lead.clone()), project, min).is_ok());
        }
    }

    #[test]
    fn enforced_admin_non_member_allowed_all_levels() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        enable_enforcement(&conn);
        let project = seed_project(&conn, "ADM");
        let admin = seed_user(&conn, "admin", true);

        for min in [Role::Viewer, Role::Maintainer, Role::Lead] {
            assert!(require_role_conn(&conn, &Some(admin.clone()), project, min).is_ok());
        }
    }

    #[test]
    fn enforced_none_user_denied_all_levels() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        enable_enforcement(&conn);
        let project = seed_project(&conn, "ANO");

        for min in [Role::Viewer, Role::Maintainer, Role::Lead] {
            assert!(require_role_conn(&conn, &None, project, min).is_err());
        }
    }

    // ── Operator-key trust rule (LIF-261) ────────────────────────
    //
    // These call `require_role_conn_op` with the operator signal passed
    // explicitly so both the operator and non-operator determination are
    // exercised deterministically, without depending on an ambient task-local
    // or MCP global. The end-to-end wiring (auth middleware → operator_scope /
    // MCP global) is proven by the middleware tests in `auth.rs` / `mcp/mod.rs`.

    #[test]
    fn enforced_operator_unbound_key_passes_all_levels() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        enable_enforcement(&conn);
        let project = seed_project(&conn, "OPR"); // no membership rows at all

        // A `None` effective user WITH the operator signal is admin-equivalent.
        for min in [Role::Viewer, Role::Maintainer, Role::Lead] {
            assert!(
                require_role_conn_op(&conn, &None, project, min, true).is_ok(),
                "operator-trusted unbound key must pass {min} in enforced mode"
            );
        }
    }

    // THE critical test: a legacy pre-binding OAuth token also resolves to
    // `None`, but it is NOT an operator credential, so it must stay
    // default-denied in enforced mode. Proves the operator bypass is gated on
    // the credential-type signal, never on `effective` being `None`.
    #[test]
    fn enforced_legacy_unbound_oauth_none_stays_forbidden_all_levels() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        enable_enforcement(&conn);
        let project = seed_project(&conn, "OAU");

        for min in [Role::Viewer, Role::Maintainer, Role::Lead] {
            assert!(
                require_role_conn_op(&conn, &None, project, min, false).is_err(),
                "a legacy unbound OAuth token (None, non-operator) must stay Forbidden at {min}"
            );
        }
    }

    // The operator signal never *demotes* a real user: it's a bypass, so with
    // it set a real non-member is still allowed (admin-equivalent). And with it
    // unset the same non-member is denied — i.e. the signal is what flips it.
    #[test]
    fn enforced_operator_signal_is_the_only_difference_for_a_nonmember() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        enable_enforcement(&conn);
        let project = seed_project(&conn, "SIG");

        assert!(
            require_role_conn_op(&conn, &None, project, Role::Viewer, false).is_err(),
            "no operator signal: denied"
        );
        assert!(
            require_role_conn_op(&conn, &None, project, Role::Viewer, true).is_ok(),
            "operator signal present: allowed"
        );
    }

    // The operator bypass is enforced-mode only; in legacy mode the flag is a
    // no-op because unbound `None` already passes Viewer/Maintainer there and
    // the Lead gate stays admin/lead-only regardless.
    #[test]
    fn legacy_mode_operator_flag_does_not_change_lead_gate() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        // flag OFF (default)
        let project = seed_project(&conn, "LGO"); // unowned

        // Even with the operator flag set, legacy Lead gate on an unowned
        // project denies a None user (matches pre-existing require_admin/lead).
        assert!(require_role_conn_op(&conn, &None, project, Role::Lead, true).is_err());
    }

    // ── Legacy mode (flag OFF, default) ─────────────────────────

    #[test]
    fn legacy_none_user_and_non_member_allowed_at_viewer_and_maintainer() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "LGC");
        let outsider = seed_user(&conn, "outsider", false);

        for min in [Role::Viewer, Role::Maintainer] {
            assert!(require_role_conn(&conn, &None, project, min).is_ok(), "None user at {min}");
            assert!(
                require_role_conn(&conn, &Some(outsider.clone()), project, min).is_ok(),
                "non-member at {min}"
            );
        }
    }

    #[test]
    fn legacy_lead_level_matches_require_project_lead_semantics() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let admin = seed_user(&conn, "admin", true);
        let lead = seed_user(&conn, "lead", false);
        let regular = seed_user(&conn, "regular", false);

        let project = queries::create_project(
            &conn,
            &CreateProject {
                name: "Legacy Lead".into(),
                identifier: "LLD".into(),
                description: String::new(),
                emoji: None,
                lead_user_id: Some(lead.id),
            },
        )
        .unwrap()
        .id;

        assert!(require_role_conn(&conn, &Some(lead.clone()), project, Role::Lead).is_ok());
        assert!(require_role_conn(&conn, &Some(admin.clone()), project, Role::Lead).is_ok());
        assert!(require_role_conn(&conn, &Some(regular.clone()), project, Role::Lead).is_err());
        assert!(require_role_conn(&conn, &None, project, Role::Lead).is_err());

        // Additive: a co-lead granted purely via project_members (no
        // lead_user_id change) also passes at Lead in legacy mode.
        let co_lead = seed_user(&conn, "co_lead", false);
        upsert_member(&conn, project, co_lead.id, Role::Lead).unwrap();
        assert!(require_role_conn(&conn, &Some(co_lead), project, Role::Lead).is_ok());

        // A plain viewer/maintainer membership does NOT grant Lead in legacy mode.
        let viewer_only = seed_user(&conn, "viewer_only", false);
        upsert_member(&conn, project, viewer_only.id, Role::Viewer).unwrap();
        assert!(require_role_conn(&conn, &Some(viewer_only), project, Role::Lead).is_err());
    }

    #[test]
    fn legacy_unowned_project_denies_non_admin_with_no_lead_message() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let regular = seed_user(&conn, "regular", false);
        let project = seed_project(&conn, "UNO"); // lead_user_id = None

        let err = require_role_conn(&conn, &Some(regular), project, Role::Lead).unwrap_err();
        match err {
            LificError::Forbidden(msg) => assert!(
                msg.contains("no lead"),
                "expected 'no lead' carve-out message, got: {msg}"
            ),
            other => panic!("expected Forbidden, got {other:?}"),
        }
    }

    #[test]
    fn legacy_admin_can_lead_unowned_project() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let admin = seed_user(&conn, "admin", true);
        let project = seed_project(&conn, "UN2");

        assert!(require_role_conn(&conn, &Some(admin), project, Role::Lead).is_ok());
    }

    // ── Bot → owner inheritance ──────────────────────────────────

    #[test]
    fn bot_inherits_owner_maintainer_role_passes_maintainer_fails_lead() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        enable_enforcement(&conn);
        let project = seed_project(&conn, "BOT");
        let owner = seed_user(&conn, "owner", false);
        upsert_member(&conn, project, owner.id, Role::Maintainer).unwrap();
        let bot = seed_bot(&conn, "bot1", Some(owner.id));

        assert!(require_role_conn(&conn, &Some(bot.clone()), project, Role::Viewer).is_ok());
        assert!(require_role_conn(&conn, &Some(bot.clone()), project, Role::Maintainer).is_ok());
        assert!(
            require_role_conn(&conn, &Some(bot), project, Role::Lead).is_err(),
            "bot must never exceed its owner's role"
        );
    }

    #[test]
    fn bot_with_admin_owner_passes_everything() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        enable_enforcement(&conn);
        let project = seed_project(&conn, "BAD"); // non-member project on purpose
        let owner = seed_user(&conn, "admin_owner", true);
        let bot = seed_bot(&conn, "bot2", Some(owner.id));

        for min in [Role::Viewer, Role::Maintainer, Role::Lead] {
            assert!(require_role_conn(&conn, &Some(bot.clone()), project, min).is_ok());
        }
    }

    #[test]
    fn ownerless_bot_is_evaluated_as_itself() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        enable_enforcement(&conn);
        let project = seed_project(&conn, "OWL");
        let bot = seed_bot(&conn, "lonebot", None);

        // No membership row for the bot itself -> denied, same as any
        // non-member, proving it did NOT silently inherit anyone else's role.
        assert!(require_role_conn(&conn, &Some(bot.clone()), project, Role::Viewer).is_err());

        upsert_member(&conn, project, bot.id, Role::Viewer).unwrap();
        assert!(require_role_conn(&conn, &Some(bot), project, Role::Viewer).is_ok());
    }

    // ── visible_project_ids ──────────────────────────────────────

    #[test]
    fn visible_project_ids_admin_returns_none() {
        let pool = test_db();
        let admin = {
            let conn = pool.write().unwrap();
            enable_enforcement(&conn);
            seed_user(&conn, "admin", true)
        };
        assert_eq!(visible_project_ids(&pool, &Some(admin)).unwrap(), None);
    }

    #[test]
    fn visible_project_ids_flag_off_returns_none() {
        let pool = test_db();
        let user = {
            let conn = pool.write().unwrap();
            seed_user(&conn, "someone", false)
        };
        assert_eq!(visible_project_ids(&pool, &Some(user)).unwrap(), None);
        assert_eq!(visible_project_ids(&pool, &None).unwrap(), None);
    }

    #[test]
    fn visible_project_ids_flag_on_member_of_two_of_three_returns_those_two() {
        let pool = test_db();
        let (user, p1, p2, _p3) = {
            let conn = pool.write().unwrap();
            enable_enforcement(&conn);
            let user = seed_user(&conn, "member", false);
            let p1 = seed_project(&conn, "PA1");
            let p2 = seed_project(&conn, "PA2");
            let p3 = seed_project(&conn, "PA3");
            upsert_member(&conn, p1, user.id, Role::Viewer).unwrap();
            upsert_member(&conn, p2, user.id, Role::Lead).unwrap();
            (user, p1, p2, p3)
        };

        let visible = visible_project_ids(&pool, &Some(user)).unwrap().unwrap();
        assert_eq!(visible, HashSet::from([p1, p2]));
    }

    #[test]
    fn visible_project_ids_flag_on_none_user_returns_empty_set() {
        let pool = test_db();
        {
            let conn = pool.write().unwrap();
            enable_enforcement(&conn);
        }
        assert_eq!(visible_project_ids(&pool, &None).unwrap(), Some(HashSet::new()));
    }

    // LIF-261: an operator-trusted unbound key sees everything (unrestricted),
    // like an admin — proven here through the ambient `operator_scope`
    // task-local, which is the exact carrier the REST middleware uses.
    #[tokio::test]
    async fn visible_project_ids_operator_scope_returns_none() {
        let pool = test_db();
        {
            let conn = pool.write().unwrap();
            enable_enforcement(&conn);
            seed_project(&conn, "V1");
            seed_project(&conn, "V2");
        }
        // Without the operator scope, a None user is confined to the empty set.
        assert_eq!(
            visible_project_ids(&pool, &None).unwrap(),
            Some(HashSet::new())
        );
        // Inside operator_scope(true), the same None user is unrestricted.
        let got = operator_scope(true, async { visible_project_ids(&pool, &None) }).await.unwrap();
        assert_eq!(got, None, "operator sees all projects (unrestricted)");
    }

    // LIF-261: the ambient operator context flips require_role end-to-end via
    // the same task-local the REST auth middleware scopes around a request.
    #[tokio::test]
    async fn require_role_reads_ambient_operator_scope() {
        let pool = test_db();
        let project = {
            let conn = pool.write().unwrap();
            enable_enforcement(&conn);
            seed_project(&conn, "AMB")
        };

        // No scope: None user denied at Viewer in enforced mode.
        assert!(require_role(&pool, &None, project, Role::Viewer).is_err());

        // Inside operator_scope(true): allowed at every level.
        operator_scope(true, async {
            for min in [Role::Viewer, Role::Maintainer, Role::Lead] {
                assert!(
                    require_role(&pool, &None, project, min).is_ok(),
                    "ambient operator scope must pass {min}"
                );
            }
        })
        .await;

        // operator_scope(false) must NOT grant the bypass.
        operator_scope(false, async {
            assert!(
                require_role(&pool, &None, project, Role::Viewer).is_err(),
                "operator_scope(false) is not a bypass"
            );
        })
        .await;
    }

    // ── require_structure_role (LIF-197) ────────────────────────

    #[test]
    fn structure_role_legacy_mode_keeps_lead_gate() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let lead = seed_user(&conn, "lead", false);
        let maintainer = seed_user(&conn, "maintainer", false);
        let project = queries::create_project(
            &conn,
            &CreateProject {
                name: "Structure Legacy".into(),
                identifier: "SL1".into(),
                description: String::new(),
                emoji: None,
                lead_user_id: Some(lead.id),
            },
        )
        .unwrap()
        .id;
        // A plain membership (non-lead_user_id) at Maintainer must NOT be
        // enough in legacy mode — this is the exact regression the branching
        // helper exists to prevent.
        upsert_member(&conn, project, maintainer.id, Role::Maintainer).unwrap();
        drop(conn);

        assert!(require_structure_role(&pool, &Some(lead), project).is_ok());
        assert!(
            require_structure_role(&pool, &Some(maintainer), project).is_err(),
            "maintainer-only membership must not pass the legacy structure gate"
        );
    }

    #[test]
    fn structure_role_enforced_mode_loosens_to_maintainer() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        enable_enforcement(&conn);
        let project = seed_project(&conn, "SE1");
        let maintainer = seed_user(&conn, "maintainer", false);
        let viewer = seed_user(&conn, "viewer", false);
        upsert_member(&conn, project, maintainer.id, Role::Maintainer).unwrap();
        upsert_member(&conn, project, viewer.id, Role::Viewer).unwrap();
        drop(conn);

        assert!(require_structure_role(&pool, &Some(maintainer), project).is_ok());
        assert!(require_structure_role(&pool, &Some(viewer), project).is_err());
    }

    // ── require_project_delete_role (LIF-197) ───────────────────

    #[test]
    fn delete_role_legacy_mode_is_admin_only_even_for_the_lead() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let admin = seed_user(&conn, "admin", true);
        let lead = seed_user(&conn, "lead", false);
        let project = queries::create_project(
            &conn,
            &CreateProject {
                name: "Delete Legacy".into(),
                identifier: "DL1".into(),
                description: String::new(),
                emoji: None,
                lead_user_id: Some(lead.id),
            },
        )
        .unwrap()
        .id;
        drop(conn);

        assert!(require_project_delete_role(&pool, &Some(admin), project).is_ok());
        assert!(
            require_project_delete_role(&pool, &Some(lead), project).is_err(),
            "legacy mode's delete gate is admin-only — a lead must still be refused, matching pre-LIF-194 require_admin"
        );
    }

    #[test]
    fn delete_role_enforced_mode_allows_lead() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        enable_enforcement(&conn);
        let project = seed_project(&conn, "DE1");
        let lead = seed_user(&conn, "lead", false);
        let maintainer = seed_user(&conn, "maintainer", false);
        upsert_member(&conn, project, lead.id, Role::Lead).unwrap();
        upsert_member(&conn, project, maintainer.id, Role::Maintainer).unwrap();
        drop(conn);

        assert!(require_project_delete_role(&pool, &Some(lead), project).is_ok());
        assert!(require_project_delete_role(&pool, &Some(maintainer), project).is_err());
    }

    // ── require_workspace_admin (LIF-197) ───────────────────────

    #[test]
    fn workspace_admin_legacy_mode_is_a_no_op() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let regular = seed_user(&conn, "regular", false);
        drop(conn);

        assert!(require_workspace_admin(&pool, &Some(regular)).is_ok());
        assert!(require_workspace_admin(&pool, &None).is_ok());
    }

    #[test]
    fn workspace_admin_enforced_mode_requires_admin() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        enable_enforcement(&conn);
        let admin = seed_user(&conn, "admin", true);
        let regular = seed_user(&conn, "regular", false);
        drop(conn);

        assert!(require_workspace_admin(&pool, &Some(admin)).is_ok());
        assert!(require_workspace_admin(&pool, &Some(regular)).is_err());
        assert!(require_workspace_admin(&pool, &None).is_err());
    }

    // ── Backfill regression (LIF-201) ───────────────────────────
    //
    // `create_project_with_lead_seeds_lead_membership` (db/queries/members.rs)
    // already proves the data-layer half: a project created with a lead
    // gets exactly one `lead` membership row. This test proves the other
    // half end-to-end through `require_role` itself — that the row seeded
    // at *creation* time (not via a manual `upsert_member` like the other
    // tests in this file use) is what `authz_enforced` actually reads once
    // the flag flips on later, mirroring the real rollout sequence from
    // LIF-DOC-7 ("ship schema + backfill first ... then turn on
    // enforcement"). A second, non-member user must have no implicit
    // access at any level — including read.

    #[test]
    fn enforced_mode_creation_seeded_lead_retains_access_second_user_has_none() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let lead = seed_user(&conn, "creator_lead", false);
        let second = seed_user(&conn, "second_user", false);

        // Flag starts off (default) at project-creation time — exactly the
        // pre-rollout state. create_project's LIF-195 seeding still runs
        // unconditionally.
        let project = queries::create_project(
            &conn,
            &CreateProject {
                name: "Backfill".into(),
                identifier: "BAK".into(),
                description: String::new(),
                emoji: None,
                lead_user_id: Some(lead.id),
            },
        )
        .unwrap()
        .id;

        // Flip enforcement on *after* creation — the rollout sequence.
        enable_enforcement(&conn);

        for min in [Role::Viewer, Role::Maintainer, Role::Lead] {
            assert!(
                require_role_conn(&conn, &Some(lead.clone()), project, min).is_ok(),
                "creation-time-seeded lead must retain {min} access once enforcement is on"
            );
            assert!(
                require_role_conn(&conn, &Some(second.clone()), project, min).is_err(),
                "a second user with no membership row must have no implicit access at {min}, including read"
            );
        }
    }

    // ── Runtime toggle ────────────────────────────────────────────

    #[test]
    fn toggling_authz_enforced_at_runtime_changes_behavior_without_restart() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "TGL"); // unowned, no memberships
        let outsider = seed_user(&conn, "outsider", false);

        // Flag off (default): legacy mode, Viewer is unconditionally allowed.
        assert!(!authz_enforced_conn(&conn).unwrap());
        assert!(require_role_conn(&conn, &Some(outsider.clone()), project, Role::Viewer).is_ok());

        // Flip it on mid-test, same connection/pool, no restart.
        enable_enforcement(&conn);
        assert!(authz_enforced_conn(&conn).unwrap());
        assert!(
            require_role_conn(&conn, &Some(outsider.clone()), project, Role::Viewer).is_err(),
            "outsider must now be denied — no membership row"
        );

        // Flip it back off: legacy behavior returns immediately.
        update_settings(
            &conn,
            InstanceSettingsPatch { authz_enforced: Some(false), ..Default::default() },
        )
        .unwrap();
        assert!(require_role_conn(&conn, &Some(outsider), project, Role::Viewer).is_ok());
    }
}
