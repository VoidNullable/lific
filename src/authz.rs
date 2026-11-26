//! LIF-196: the single project-scoped authorization primitive shared by REST
//! (LIF-197) and MCP (LIF-198), so the two transports can't drift. Design:
//! LIF-DOC-7. Part of epic LIF-194.
//!
//! ## One rule
//!
//! Every project check resolves the caller's role from `project_members` and
//! requires `role >= min`. A `None` caller or a non-member is `Forbidden`,
//! even for `min = Viewer`. Reads need `Viewer`, content writes `Maintainer`,
//! and managing the project itself `Lead`.
//!
//! Until 3.0 this was an instance setting (`authz_enforced`) with a legacy
//! mode in which any authenticated request could read and write every
//! project. LIF-518 retired that mode: migration 070 seeded memberships from
//! what people had actually done and recorded who lost access (see
//! [`crate::db::queries::access_losses`]), and the switch is gone.
//!
//! Where membership is decided:
//!
//! - [`project_role`] decides what a caller may do on one project. Every gate
//!   ([`require_role_conn`] and the helpers built on it, [`can_view_project`])
//!   answers from it. A rule that grants access without a membership row
//!   (published projects, for instance) is added there.
//! - [`visible_project_ids_conn`] is its set form, for cross-project reads and
//!   the sidebar. It must grant the same projects, so such a rule is mirrored
//!   there; a test compares the two.
//! - [`membership_role`] is the caller's own row without the admin pass, which
//!   `my-role` reports.
//! - [`BELONGS_TO_PROJECT_SQL`] decides whether a *named other person* belongs
//!   to a project (member or admin): who can be assigned or `@`-mentioned. It
//!   is not an access check and does not follow read-without-a-role rules.
//!
//! `is_admin` passes every check. Bots resolve to their owner
//! (`007_bot_owners.sql`) before any check runs: an agent can never exceed
//! the human that owns it.
//!
//! ## Operator-key trust (LIF-261 → LIFIC-7/8/10/11/14)
//!
//! Default-deny for a `None` effective user would brick the zero-user
//! agent-first flow (`lific init` → `start` → `connect`), which runs on
//! **user-unbound API keys**. Those keys can only be minted with shell access
//! to the server (`lific start`'s first-run auto-key, `lific key create`,
//! `connect`'s fresh-install path), so they're operator-trusted by
//! construction: the threat the default guards against is a web-signup
//! stranger with a session/OAuth token, not the operator's own shell-minted
//! key.
//!
//! LIFIC-7 unified this on [`resolve_caller`]: any credential that
//! authenticates but resolves no specific user (an unbound API key, a legacy
//! unbound OAuth token, a credential-less "auth off" request, or a stdio MCP
//! session) falls back to the **first admin**. The gates then read
//! `identity.user.is_admin`, which short-circuits to allow, so the operator
//! bypass is just "the first admin is trusted," applied identically across
//! REST, MCP, and CLI. **Audit unbound keys with `lific key list`.**

use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::models::{AuthUser, Role};
use crate::db::{DbPool, queries};
use crate::error::LificError;
use crate::resolve_caller::ResolvedIdentity;

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

/// LIFIC-10: extract the credential user from a resolved identity for the
/// bot→owner [`effective_user`] lookup. Gates now take `&Option<ResolvedIdentity>`
/// (the type [`resolve_caller`] produces), but `effective_user` still operates
/// on `AuthUser` — its job (map a bot to its owner) is user-level, not
/// identity-level. This is the single adapter between the two.
fn user_of(identity: &Option<ResolvedIdentity>) -> Option<AuthUser> {
    identity.as_ref().map(|i| i.user.clone())
}

// ── The role decision ────────────────────────────────────────────

/// The role `effective` (already resolved through [`effective_user`]) holds
/// on `project_id`, or `None` for no access at all. An admin holds every role.
///
/// Every project gate, read or write, is answered from here; see the module
/// docs.
fn project_role(
    conn: &Connection,
    effective: &Option<AuthUser>,
    project_id: i64,
) -> Result<Option<Role>, LificError> {
    let Some(user) = effective else {
        return Ok(None);
    };
    if user.is_admin {
        return Ok(Some(Role::Lead));
    }
    membership_role(conn, effective, project_id)
}

/// The role `effective`'s own membership row grants on `project_id`, without
/// the admin pass. What `GET /api/projects/{id}/my-role` reports as `role`;
/// access decisions go through [`project_role`].
pub(crate) fn membership_role(
    conn: &Connection,
    effective: &Option<AuthUser>,
    project_id: i64,
) -> Result<Option<Role>, LificError> {
    match effective {
        Some(user) => queries::members::get_member_role(conn, project_id, user.id),
        None => Ok(None),
    }
}

/// SQL predicate: the user aliased `u` belongs to the project bound to the
/// named parameter `:project_id`, as a member or an instance admin.
///
/// This answers a different question from [`project_role`]: not "may this
/// caller read or write here" but "is this named person someone the project
/// can assign work to or `@`-mention". Assignee eligibility
/// (`db::queries::assignees`) and mention candidates
/// (`db::queries::comments::mention_candidates`) both use it, so the two
/// cannot drift. A rule that lets a caller read a project without a role does
/// not belong here.
pub(crate) const BELONGS_TO_PROJECT_SQL: &str = "(u.is_admin = 1 OR EXISTS (\
     SELECT 1 FROM project_members m \
     WHERE m.project_id = :project_id AND m.user_id = u.id))";

fn insufficient_role(min: Role) -> LificError {
    LificError::Forbidden(format!(
        "requires at least '{}' access to this project",
        min.as_str()
    ))
}

// ── require_role ─────────────────────────────────────────────────

/// Require the effective caller to hold at least `min` role on `project_id`.
/// `Ok(())` = allowed.
///
/// LIFIC-10/14: an operator-trusted unbound API key resolves (via
/// [`resolve_caller`]) to the first admin, so it passes here as an admin.
pub fn require_role(
    db: &DbPool,
    identity: &Option<ResolvedIdentity>,
    project_id: i64,
    min: Role,
) -> Result<(), LificError> {
    let conn = db.read()?;
    require_role_conn(&conn, identity, project_id, min)
}

/// Whether the caller can read `project_id` at all (any role).
pub(crate) fn can_view_project(
    db: &DbPool,
    identity: &ResolvedIdentity,
    project_id: i64,
) -> Result<bool, LificError> {
    let conn = db.read()?;
    let effective = effective_user(&conn, &Some(identity.user.clone()));
    Ok(project_role(&conn, &effective, project_id)?.is_some())
}

pub(crate) fn require_role_conn(
    conn: &Connection,
    identity: &Option<ResolvedIdentity>,
    project_id: i64,
    min: Role,
) -> Result<(), LificError> {
    let effective = effective_user(conn, &user_of(identity));
    match project_role(conn, &effective, project_id)? {
        Some(role) if role >= min => Ok(()),
        _ => Err(insufficient_role(min)),
    }
}

/// LIF-197: gate for the module/label/folder ("structure") endpoints:
/// `Maintainer`, the same bar as any other content write.
pub fn require_structure_role(
    db: &DbPool,
    identity: &Option<ResolvedIdentity>,
    project_id: i64,
) -> Result<(), LificError> {
    require_role(db, identity, project_id, Role::Maintainer)
}

/// LIF-197: gate for `DELETE /api/projects/{id}`: the project's lead (design
/// decision #6) or an admin.
pub fn require_project_delete_role(
    db: &DbPool,
    identity: &Option<ResolvedIdentity>,
    project_id: i64,
) -> Result<(), LificError> {
    let conn = db.read()?;
    require_project_delete_role_conn(&conn, identity, project_id)
}

/// [`require_project_delete_role`] on a caller-supplied connection, so the
/// check can run inside the transaction that performs the delete.
///
/// Deleting a project destroys every issue, page and plan in it. Deciding that
/// from a role snapshot taken before the request was routed means an admin
/// demoted, or a lead removed, a moment ago can still do it. The read has to
/// be inside the transaction that acts on it.
pub(crate) fn require_project_delete_role_conn(
    conn: &Connection,
    identity: &Option<ResolvedIdentity>,
    project_id: i64,
) -> Result<(), LificError> {
    require_role_conn(conn, identity, project_id, Role::Lead)
}

/// LIF-197: gate for mutations/reads on workspace-level (project-less)
/// pages, the only entity that can have `project_id = NULL` (plans always
/// require a project; see `CreatePlan.project_id: i64`). Design decision
/// #10: admin-only.
pub fn require_workspace_admin(
    db: &DbPool,
    identity: &Option<ResolvedIdentity>,
) -> Result<(), LificError> {
    let conn = db.read()?;
    require_workspace_admin_conn(&conn, identity)
}

pub(crate) fn require_workspace_admin_conn(
    conn: &Connection,
    identity: &Option<ResolvedIdentity>,
) -> Result<(), LificError> {
    let effective = effective_user(conn, &user_of(identity));
    match &effective {
        Some(u) if u.is_admin => Ok(()),
        _ => Err(LificError::Forbidden(
            "only an admin can access workspace-level pages".into(),
        )),
    }
}

pub(crate) fn require_project_or_workspace_role_conn(
    conn: &Connection,
    identity: &Option<ResolvedIdentity>,
    project_id: Option<i64>,
    minimum_role: Role,
) -> Result<(), LificError> {
    match project_id {
        Some(project_id) => require_role_conn(conn, identity, project_id, minimum_role),
        None => require_workspace_admin_conn(conn, identity),
    }
}

// ── visible_project_ids ──────────────────────────────────────────

/// The cross-project read filter for search / project listing / any
/// workspace-spanning read (LIF-197/LIF-198 call sites): the set form of
/// [`project_role`], so the two must grant the same projects.
///
/// `None` = unrestricted (admins): the caller should apply no filter at all.
/// `Some(ids)` = only these project ids are visible: the effective caller's
/// memberships (any role), or the empty set for a `None` auth user / a member
/// of nothing.
///
/// LIFIC-10/14: an unbound key resolves to the first admin and
/// short-circuits below.
pub fn visible_project_ids(
    db: &DbPool,
    identity: &Option<ResolvedIdentity>,
) -> Result<Option<HashSet<i64>>, LificError> {
    let conn = db.read()?;
    let effective = effective_user(&conn, &user_of(identity));
    visible_project_ids_conn(&conn, &effective)
}

/// [`visible_project_ids`] for an already resolved effective user, on a
/// caller-supplied connection (the sidebar snapshot resolves its own).
///
/// It must grant exactly the projects [`project_role`] answers `Some` for;
/// `visible_projects_match_project_role` pins that. It is a set query rather
/// than a loop over `project_role` because every list request runs it.
pub(crate) fn visible_project_ids_conn(
    conn: &Connection,
    effective: &Option<AuthUser>,
) -> Result<Option<HashSet<i64>>, LificError> {
    let Some(user) = effective else {
        return Ok(Some(HashSet::new()));
    };
    if user.is_admin {
        return Ok(None);
    }
    let ids = queries::members::list_project_ids_for_user(conn, user.id)?;
    Ok(Some(ids.into_iter().collect()))
}

/// LIF-197: apply the [`visible_project_ids`] cross-project read filter to a
/// list of items. `None` (unrestricted: admin) keeps everything. `Some(ids)`
/// keeps only items whose `project_id_of` result is `Some(pid)` with `pid` in
/// `ids`; a workspace-level item (`None` project_id) is therefore excluded
/// for any non-admin, matching design decision #10 (workspace pages are
/// admin-only).
///
/// LIF-377: one implementation for both read surfaces. REST and MCP each had
/// a byte-identical copy, so "silently absent, never an error" could drift on
/// one transport without the other noticing.
pub fn filter_visible<T>(
    items: Vec<T>,
    visible: &Option<HashSet<i64>>,
    project_id_of: impl Fn(&T) -> Option<i64>,
) -> Vec<T> {
    match visible {
        None => items,
        Some(ids) => items
            .into_iter()
            .filter(|it| project_id_of(it).is_some_and(|pid| ids.contains(&pid)))
            .collect(),
    }
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::Transport;
    use crate::db::models::{CreateProject, CreateUser};
    use crate::db::queries::members::upsert_member;
    use crate::db::{self, queries};

    /// Wrap an `AuthUser` into a `ResolvedIdentity` (transport is irrelevant to
    /// the gates — only `user` drives membership/admin checks). Mirrors what
    /// [`crate::resolve_caller`] produces in production for a credential that
    /// already names a user.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "test helper mirrors the optional production identity API"
    )]
    fn id(user: AuthUser) -> Option<ResolvedIdentity> {
        Some(ResolvedIdentity {
            user,
            transport: Transport::Api,
        })
    }

    /// The identity an unbound API key resolves to under the new design: the
    /// first admin, via [`crate::resolve_caller`]. Tests use this to prove the
    /// operator bypass now lives in `identity.user.is_admin` (LIFIC-10) rather
    /// than a separate `is_operator` signal.
    fn first_admin_identity(conn: &Connection) -> Option<ResolvedIdentity> {
        queries::users::first_admin(conn)
            .unwrap()
            .map(|admin| ResolvedIdentity {
                user: admin,
                transport: Transport::Api,
            })
    }

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
                let bot =
                    queries::users::create_bot_user(conn, owner, username, username, None).unwrap();
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
                ..Default::default()
            },
        )
        .unwrap()
        .id
    }

    /// `visible_project_ids_conn` is the set form of `project_role`: for
    /// every kind of caller it must grant exactly the projects `project_role`
    /// gives any role on.
    #[test]
    fn visible_projects_match_project_role() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let projects = [
            seed_project(&conn, "ONE"),
            seed_project(&conn, "TWO"),
            seed_project(&conn, "THR"),
        ];
        let admin = seed_user(&conn, "admin", true);
        let member = seed_user(&conn, "member", false);
        let outsider = seed_user(&conn, "outsider", false);
        let owned_bot = seed_bot(&conn, "member-bot", Some(member.id));
        let lone_bot = seed_bot(&conn, "lone-bot", None);
        upsert_member(&conn, projects[0], member.id, Role::Viewer).unwrap();
        upsert_member(&conn, projects[2], member.id, Role::Lead).unwrap();
        upsert_member(&conn, projects[1], lone_bot.id, Role::Maintainer).unwrap();

        for caller in [
            None,
            Some(admin),
            Some(member),
            Some(outsider),
            Some(owned_bot),
            Some(lone_bot),
        ] {
            let effective = effective_user(&conn, &caller);
            let by_role: HashSet<i64> = projects
                .iter()
                .copied()
                .filter(|p| project_role(&conn, &effective, *p).unwrap().is_some())
                .collect();
            let visible = visible_project_ids_conn(&conn, &effective)
                .unwrap()
                .unwrap_or_else(|| projects.iter().copied().collect());
            assert_eq!(visible, by_role, "caller {caller:?}");
        }
    }

    // ── Role matrix ──────────────────────────────────────────────

    #[test]
    fn enforced_non_member_denied_at_every_level() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "ENF");
        let outsider = seed_user(&conn, "outsider", false);

        for min in [Role::Viewer, Role::Maintainer, Role::Lead] {
            let err = require_role_conn(&conn, &id(outsider.clone()), project, min).unwrap_err();
            assert!(
                matches!(err, LificError::Forbidden(_)),
                "denied at {min} got {err:?}"
            );
        }
    }

    #[test]
    fn enforced_viewer_allowed_viewer_denied_maintainer_and_lead() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "VWR");
        let viewer = seed_user(&conn, "viewer", false);
        upsert_member(&conn, project, viewer.id, Role::Viewer).unwrap();

        assert!(require_role_conn(&conn, &id(viewer.clone()), project, Role::Viewer).is_ok());
        assert!(require_role_conn(&conn, &id(viewer.clone()), project, Role::Maintainer).is_err());
        assert!(require_role_conn(&conn, &id(viewer), project, Role::Lead).is_err());
    }

    #[test]
    fn enforced_maintainer_allowed_viewer_and_maintainer_denied_lead() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "MNT");
        let maintainer = seed_user(&conn, "maintainer", false);
        upsert_member(&conn, project, maintainer.id, Role::Maintainer).unwrap();

        assert!(require_role_conn(&conn, &id(maintainer.clone()), project, Role::Viewer).is_ok());
        assert!(
            require_role_conn(&conn, &id(maintainer.clone()), project, Role::Maintainer).is_ok()
        );
        assert!(require_role_conn(&conn, &id(maintainer), project, Role::Lead).is_err());
    }

    #[test]
    fn enforced_lead_allowed_all_levels() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "LED");
        let lead = seed_user(&conn, "lead", false);
        upsert_member(&conn, project, lead.id, Role::Lead).unwrap();

        for min in [Role::Viewer, Role::Maintainer, Role::Lead] {
            assert!(require_role_conn(&conn, &id(lead.clone()), project, min).is_ok());
        }
    }

    #[test]
    fn enforced_admin_non_member_allowed_all_levels() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "ADM");
        let admin = seed_user(&conn, "admin", true);

        for min in [Role::Viewer, Role::Maintainer, Role::Lead] {
            assert!(require_role_conn(&conn, &id(admin.clone()), project, min).is_ok());
        }
    }

    #[test]
    fn enforced_none_user_denied_all_levels() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "ANO");

        for min in [Role::Viewer, Role::Maintainer, Role::Lead] {
            assert!(require_role_conn(&conn, &None, project, min).is_err());
        }
    }

    // ── Operator-key trust (LIF-261) — LIFIC-10 redesign ──────────
    //
    // The operator bypass is no longer a separate signal. Under the new
    // design, an unbound API key resolves (via `resolve_caller`) to the first
    // admin, so `identity.user.is_admin` admits it in the same `is_admin`
    // short-circuit every admin hits. These tests prove that contract through
    // the resolved identity the middleware would actually produce.

    // An unbound API key's resolved identity is the first admin → passes every
    // level, exactly as the LIF-261 carrier-signal path did before. Replaces
    // the deleted `require_role_conn_op(.., true)` test. LIF-518: this holds on
    // a project with no lead and no members, which legacy mode used to cover.
    #[test]
    fn resolved_first_admin_passes_all_levels_like_operator_key_did() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let _admin = seed_user(&conn, "admin", true); // becomes first_admin
        let project = seed_project(&conn, "OPR"); // no membership rows at all

        let identity = first_admin_identity(&conn).expect("first admin resolves");
        for min in [Role::Viewer, Role::Maintainer, Role::Lead] {
            assert!(
                require_role_conn(&conn, &Some(identity.clone()), project, min).is_ok(),
                "resolved-first-admin (unbound key) must pass {min}"
            );
        }
        assert!(require_project_delete_role_conn(&conn, &Some(identity), project).is_ok());
        assert!(require_workspace_admin_conn(&conn, &first_admin_identity(&conn)).is_ok());
    }

    // A non-member resolved to a non-admin stays forbidden at every level —
    // the bypass is `is_admin`, not merely "an identity exists."
    #[test]
    fn resolved_non_admin_non_member_stays_forbidden_all_levels() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        seed_user(&conn, "someone", true); // first_admin, but irrelevant here
        let project = seed_project(&conn, "ONA");
        let regular = seed_user(&conn, "regular", false);

        for min in [Role::Viewer, Role::Maintainer, Role::Lead] {
            assert!(
                require_role_conn(&conn, &id(regular.clone()), project, min).is_err(),
                "a resolved non-admin non-member must stay Forbidden at {min}"
            );
        }
        assert!(require_project_delete_role_conn(&conn, &id(regular), project).is_err());
    }

    // A `None` identity (no admin exists to resolve an unbound key to) is
    // denied everywhere, including the lead gate of an unowned project.
    #[test]
    fn none_identity_is_denied_even_on_an_unowned_project() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "NOP");

        for min in [Role::Viewer, Role::Maintainer, Role::Lead] {
            assert!(require_role_conn(&conn, &None, project, min).is_err());
        }
    }

    // ── Bot → owner inheritance ──────────────────────────────────

    #[test]
    fn bot_inherits_owner_maintainer_role_passes_maintainer_fails_lead() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "BOT");
        let owner = seed_user(&conn, "owner", false);
        upsert_member(&conn, project, owner.id, Role::Maintainer).unwrap();
        let bot = seed_bot(&conn, "bot1", Some(owner.id));

        assert!(require_role_conn(&conn, &id(bot.clone()), project, Role::Viewer).is_ok());
        assert!(require_role_conn(&conn, &id(bot.clone()), project, Role::Maintainer).is_ok());
        assert!(
            require_role_conn(&conn, &id(bot), project, Role::Lead).is_err(),
            "bot must never exceed its owner's role"
        );
    }

    #[test]
    fn bot_with_admin_owner_passes_everything() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "BAD"); // non-member project on purpose
        let owner = seed_user(&conn, "admin_owner", true);
        let bot = seed_bot(&conn, "bot2", Some(owner.id));

        for min in [Role::Viewer, Role::Maintainer, Role::Lead] {
            assert!(require_role_conn(&conn, &id(bot.clone()), project, min).is_ok());
        }
    }

    #[test]
    fn ownerless_bot_is_evaluated_as_itself() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "OWL");
        let bot = seed_bot(&conn, "lonebot", None);

        // No membership row for the bot itself -> denied, same as any
        // non-member, proving it did NOT silently inherit anyone else's role.
        assert!(require_role_conn(&conn, &id(bot.clone()), project, Role::Viewer).is_err());

        upsert_member(&conn, project, bot.id, Role::Viewer).unwrap();
        assert!(require_role_conn(&conn, &id(bot), project, Role::Viewer).is_ok());
    }

    // ── visible_project_ids ──────────────────────────────────────

    #[test]
    fn visible_project_ids_admin_returns_none() {
        let pool = test_db();
        let admin = {
            let conn = pool.write().unwrap();
            seed_user(&conn, "admin", true)
        };
        assert_eq!(visible_project_ids(&pool, &id(admin)).unwrap(), None);
    }

    #[test]
    fn visible_project_ids_member_of_two_of_three_returns_those_two() {
        let pool = test_db();
        let (user, p1, p2, _p3) = {
            let conn = pool.write().unwrap();
            let user = seed_user(&conn, "member", false);
            let p1 = seed_project(&conn, "PA1");
            let p2 = seed_project(&conn, "PA2");
            let p3 = seed_project(&conn, "PA3");
            upsert_member(&conn, p1, user.id, Role::Viewer).unwrap();
            upsert_member(&conn, p2, user.id, Role::Lead).unwrap();
            (user, p1, p2, p3)
        };

        let visible = visible_project_ids(&pool, &id(user)).unwrap().unwrap();
        assert_eq!(visible, HashSet::from([p1, p2]));
    }

    #[test]
    fn visible_project_ids_none_user_returns_empty_set() {
        let pool = test_db();
        assert_eq!(
            visible_project_ids(&pool, &None).unwrap(),
            Some(HashSet::new())
        );
    }

    // LIF-261 / LIFIC-10/14: an operator-trusted unbound key resolves (via
    // `resolve_caller`) to the first admin, so its resolved identity is
    // `is_admin` and `visible_project_ids` returns `None` (unrestricted). The
    // old ambient `operator_scope` carrier that produced this outcome is gone.
    #[tokio::test]
    async fn visible_project_ids_resolved_first_admin_returns_none() {
        let pool = test_db();
        let identity = {
            let conn = pool.write().unwrap();
            seed_user(&conn, "admin", true);
            seed_project(&conn, "V1");
            seed_project(&conn, "V2");
            first_admin_identity(&conn).expect("first admin resolves")
        };
        // A None identity (pre-LIFIC-9 bootstrap) is confined to the empty set.
        assert_eq!(
            visible_project_ids(&pool, &None).unwrap(),
            Some(HashSet::new())
        );
        // The resolved-first-admin identity an unbound key produces is unrestricted.
        assert_eq!(
            visible_project_ids(&pool, &Some(identity)).unwrap(),
            None,
            "resolved-first-admin (unbound key) sees all projects"
        );
    }

    // LIF-261 / LIFIC-10: the operator bypass now flows through the resolved
    // identity rather than an ambient task-local. A resolved first-admin
    // (unbound-key path) passes every level; a None identity is denied.
    #[tokio::test]
    async fn require_role_admits_resolved_first_admin_denies_none() {
        let pool = test_db();
        let (project, identity, regular) = {
            let conn = pool.write().unwrap();
            seed_user(&conn, "admin", true);
            let project = seed_project(&conn, "AMB");
            let identity = first_admin_identity(&conn).expect("first admin resolves");
            let regular = seed_user(&conn, "regular", false);
            (project, identity, regular)
        };

        // None identity: denied at Viewer.
        assert!(require_role(&pool, &None, project, Role::Viewer).is_err());

        // Resolved first-admin: allowed at every level (the unbound-key bypass).
        for min in [Role::Viewer, Role::Maintainer, Role::Lead] {
            assert!(
                require_role(&pool, &Some(identity.clone()), project, min).is_ok(),
                "resolved-first-admin must pass {min}"
            );
        }

        // A resolved non-admin non-member is still denied.
        assert!(
            require_role(&pool, &id(regular), project, Role::Viewer).is_err(),
            "resolved non-admin is not a bypass"
        );
    }

    // ── require_structure_role (LIF-197) ────────────────────────

    #[test]
    fn structure_role_needs_maintainer() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "SE1");
        let maintainer = seed_user(&conn, "maintainer", false);
        let viewer = seed_user(&conn, "viewer", false);
        upsert_member(&conn, project, maintainer.id, Role::Maintainer).unwrap();
        upsert_member(&conn, project, viewer.id, Role::Viewer).unwrap();
        drop(conn);

        assert!(require_structure_role(&pool, &id(maintainer), project).is_ok());
        assert!(require_structure_role(&pool, &id(viewer), project).is_err());
    }

    // ── require_project_delete_role (LIF-197) ───────────────────

    #[test]
    fn delete_role_allows_lead_and_refuses_maintainer() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "DE1");
        let lead = seed_user(&conn, "lead", false);
        let maintainer = seed_user(&conn, "maintainer", false);
        upsert_member(&conn, project, lead.id, Role::Lead).unwrap();
        upsert_member(&conn, project, maintainer.id, Role::Maintainer).unwrap();
        drop(conn);

        assert!(require_project_delete_role(&pool, &id(lead), project).is_ok());
        assert!(require_project_delete_role(&pool, &id(maintainer), project).is_err());
    }

    // ── require_workspace_admin (LIF-197) ───────────────────────

    #[test]
    fn workspace_admin_requires_admin() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let admin = seed_user(&conn, "admin", true);
        let regular = seed_user(&conn, "regular", false);
        drop(conn);

        assert!(require_workspace_admin(&pool, &id(admin)).is_ok());
        assert!(require_workspace_admin(&pool, &id(regular)).is_err());
        assert!(require_workspace_admin(&pool, &None).is_err());
    }

    // ── Backfill regression (LIF-201) ───────────────────────────
    //
    // `create_project_with_lead_seeds_lead_membership` (db/queries/members.rs)
    // already proves the data-layer half: a project created with a lead
    // gets exactly one `lead` membership row. This test proves the other
    // half end-to-end through `require_role` itself — that the row seeded
    // at *creation* time (not via a manual `upsert_member` like the other
    // tests in this file use) is what the gate reads. A second, non-member
    // user must have no implicit access at any level, including read.

    #[test]
    fn creation_seeded_lead_retains_access_second_user_has_none() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let lead = seed_user(&conn, "creator_lead", false);
        let second = seed_user(&conn, "second_user", false);

        let project = queries::create_project(
            &conn,
            &CreateProject {
                name: "Backfill".into(),
                identifier: "BAK".into(),
                lead_user_id: Some(lead.id),
                ..Default::default()
            },
        )
        .unwrap()
        .id;

        for min in [Role::Viewer, Role::Maintainer, Role::Lead] {
            assert!(
                require_role_conn(&conn, &id(lead.clone()), project, min).is_ok(),
                "creation-time-seeded lead must retain {min} access"
            );
            assert!(
                require_role_conn(&conn, &id(second.clone()), project, min).is_err(),
                "a second user with no membership row must have no implicit access at {min}, including read"
            );
        }
    }

    // `can_view_project` answers from the same role decision as the gates, so
    // a bot sees what its owner sees.
    #[test]
    fn can_view_project_follows_the_bot_owner() {
        let pool = test_db();
        let (project, bot, stranger_bot) = {
            let conn = pool.write().unwrap();
            let project = seed_project(&conn, "CVP");
            let owner = seed_user(&conn, "owner", false);
            upsert_member(&conn, project, owner.id, Role::Viewer).unwrap();
            let bot = seed_bot(&conn, "cvbot", Some(owner.id));
            let stranger = seed_user(&conn, "stranger", false);
            let stranger_bot = seed_bot(&conn, "strangerbot", Some(stranger.id));
            (project, bot, stranger_bot)
        };
        let view = |u: AuthUser| {
            can_view_project(
                &pool,
                &ResolvedIdentity {
                    user: u,
                    transport: Transport::Api,
                },
                project,
            )
            .unwrap()
        };
        assert!(view(bot));
        assert!(!view(stranger_bot));
    }
}
