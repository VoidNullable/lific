//! LIFIC-8: the single place that decides who the caller is.
//!
//! Produces a [`ResolvedIdentity`] — a resolved identity with a *real* user
//! and the transport they came in on. The defining property, stated in the
//! spec ([LIFIC-7](http://localhost:3456/LIFIC/issues/LIFIC-7)): **there is
//! always a user — no anonymous.** Whenever a credential resolves no specific
//! user (an unbound API key, a legacy unbound OAuth token, a credential-less
//! "auth off" request, or a stdio MCP session), [`resolve_caller`] falls back
//! to the first admin — the same `first_admin` decision that was previously
//! scattered across four call sites (auto-login, authless MCP, comment-create,
//! comment-edit). Consolidating it here is the expand step: the new identity
//! exists *alongside* the legacy `Option<AuthUser>` and nothing breaks while
//! downstream tickets (LIFIC-10/11) migrate the gates onto it.
//!
//! `None` is returned only in the degenerate zero-user bootstrap case — no
//! credential resolved a user *and* no admin exists yet. LIFIC-9 eliminates
//! that case by minting a first admin at `lific init` time; until then the
//! callers map `None` to the same error they already raised.

use rusqlite::Connection;

use crate::actor::Transport;
use crate::db::models::AuthUser;
use crate::db::queries;
use crate::error::LificError;

/// The resolved identity for the current caller. The `user` is always a real
/// user (never `Option`); `transport` records which door they came in on, so
/// the audit log and per-transport logic keep working without a separate
/// operator signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedIdentity {
    pub user: AuthUser,
    pub transport: Transport,
}

/// Resolve the caller's identity.
///
/// `credential_user` is whoever the credential itself named — a session's
/// user, an OAuth/API-key binding, or `None` when the credential carried no
/// user (unbound key, legacy OAuth) or there was no credential at all ("auth
/// off"). When that is `None`, the first admin is the passwordless fallback,
/// consolidating the four historical `first_admin` call sites into one
/// decision.
///
/// Returns `Ok(None)` only when no user can be resolved at all (no credential
/// user *and* no admin exists). Callers preserve their existing behavior by
/// mapping that `None` to the same error they raise today.
pub fn resolve_caller_conn(
    conn: &Connection,
    credential_user: Option<AuthUser>,
    transport: Transport,
) -> Result<Option<ResolvedIdentity>, LificError> {
    let user = match credential_user {
        Some(u) => u,
        None => match queries::users::first_admin(conn)? {
            Some(admin) => admin,
            None => return Ok(None),
        },
    };
    Ok(Some(ResolvedIdentity { user, transport }))
}

/// Convenience wrapper that opens its own read connection. The auth
/// middleware uses this: it has a [`DbPool`](crate::db::DbPool) but no live
/// borrow at its success return points. The credential-user path is DB-free,
/// so only the fallback hits the database.
pub fn resolve_caller(
    db: &crate::db::DbPool,
    credential_user: Option<AuthUser>,
    transport: Transport,
) -> Result<Option<ResolvedIdentity>, LificError> {
    // Fast path: a credential already named a user — no DB read needed.
    if let Some(user) = credential_user {
        return Ok(Some(ResolvedIdentity { user, transport }));
    }
    let conn = db.read()?;
    resolve_caller_conn(&conn, None, transport)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{self, queries};
    use crate::db::models::CreateUser;

    fn test_db() -> db::DbPool {
        db::open_memory().expect("test db")
    }

    fn seed_admin(conn: &Connection, username: &str) -> AuthUser {
        let u = queries::users::create_user(
            conn,
            &CreateUser {
                username: username.into(),
                email: format!("{username}@local.test"),
                password: "adminpass123".into(),
                display_name: Some(format!("Admin {username}")),
                is_admin: true,
                is_bot: false,
            },
        )
        .unwrap();
        AuthUser {
            id: u.id,
            username: u.username,
            display_name: u.display_name,
            is_admin: u.is_admin,
        }
    }

    fn seed_regular(conn: &Connection, username: &str) -> AuthUser {
        let u = queries::users::create_user(
            conn,
            &CreateUser {
                username: username.into(),
                email: format!("{username}@local.test"),
                password: "userpass123".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap();
        AuthUser {
            id: u.id,
            username: u.username,
            display_name: u.display_name,
            is_admin: u.is_admin,
        }
    }

    // ── credential-user path: pure, no DB, transport passes through ──────

    #[test]
    fn credential_user_is_returned_unchanged_with_its_transport() {
        let pool = test_db();
        let conn = pool.read().unwrap();
        let regular = seed_regular(&conn, "alice");

        for transport in [Transport::Web, Transport::Mcp, Transport::Api, Transport::Cli] {
            let id = resolve_caller_conn(&conn, Some(regular.clone()), transport)
                .unwrap()
                .expect("Some(credential) always resolves");
            assert_eq!(id.user, regular);
            assert_eq!(id.transport, transport);
        }
    }

    // The credential-user path never touches the DB: it resolves even when no
    // users exist at all (the user came in on the credential).
    #[test]
    fn credential_user_resolves_with_zero_users_in_db() {
        let pool = test_db(); // no users seeded
        let conn = pool.read().unwrap();
        let phantom = AuthUser {
            id: 999,
            username: "phantom".into(),
            display_name: String::new(),
            is_admin: false,
        };
        let id = resolve_caller_conn(&conn, Some(phantom.clone()), Transport::Api)
            .unwrap()
            .expect("credential user resolves regardless of DB state");
        assert_eq!(id.user, phantom);
    }

    // ── fallback path: first_admin when no credential user ────────────────

    #[test]
    fn none_credential_falls_back_to_first_admin() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let admin = seed_admin(&conn, "admin");
        // A second admin created later must NOT win — first_admin is ordered
        // by created_at, so the earliest admin is the stable fallback.
        let later = seed_admin(&conn, "later");
        assert_ne!(admin.id, later.id);
        drop(conn);

        let conn = pool.read().unwrap();
        let id = resolve_caller_conn(&conn, None, Transport::Mcp)
            .unwrap()
            .expect("first_admin fallback should resolve");
        assert_eq!(id.user, admin, "fallback must be the earliest admin");
        assert_eq!(id.transport, Transport::Mcp);
    }

    // The fallback only considers admins; a non-admin user alone is not a
    // fallback candidate, so None credential + no admin → None.
    #[test]
    fn none_credential_with_no_admin_returns_none() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        seed_regular(&conn, "onlyuser");
        drop(conn);

        let conn = pool.read().unwrap();
        assert!(resolve_caller_conn(&conn, None, Transport::Api)
            .unwrap()
            .is_none());
    }

    // Zero-user bootstrap: no credential and no users at all → None. This is
    // the degenerate case LIFIC-9 eliminates by minting an admin at init.
    #[test]
    fn none_credential_zero_users_returns_none() {
        let pool = test_db();
        let conn = pool.read().unwrap();
        assert!(resolve_caller_conn(&conn, None, Transport::System)
            .unwrap()
            .is_none());
    }

    // ── DbPool wrapper mirrors the conn core ──────────────────────────────

    #[test]
    fn dbpool_overload_and_conn_core_agree_on_first_admin_fallback() {
        let pool = test_db();
        {
            let conn = pool.write().unwrap();
            seed_admin(&conn, "admin");
        }
        let via_conn = {
            let conn = pool.read().unwrap();
            resolve_caller_conn(&conn, None, Transport::Api)
                .unwrap()
                .expect("conn fallback resolves")
        };
        let via_pool = resolve_caller(&pool, None, Transport::Api)
            .unwrap()
            .expect("pool wrapper fallback resolves");
        assert_eq!(via_conn, via_pool);
    }

    #[test]
    fn credential_user_resolves_without_opening_a_db_connection() {
        // No users at all, yet a credential user resolves — proves the pool
        // wrapper's fast path never opens a read connection.
        let pool = test_db();
        let user = AuthUser {
            id: 1,
            username: "cred".into(),
            display_name: String::new(),
            is_admin: false,
        };
        let id = resolve_caller(&pool, Some(user.clone()), Transport::Web)
            .unwrap()
            .expect("credential fast path");
        assert_eq!(id.user, user);
        assert_eq!(id.transport, Transport::Web);
    }
}
