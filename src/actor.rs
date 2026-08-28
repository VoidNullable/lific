//! LIF-155: actor context — who is performing the current mutation, and
//! through which door.
//!
//! The audit log's capture triggers (migration 018) read actor identity
//! from the one-row `_actor_state` table, which `DbPool::write()` stamps
//! from this module just before handing out the exclusive write
//! connection. Identity flows in via a tokio task-local that each entry
//! surface sets at its boundary:
//!
//! - REST middleware (`auth::require_api_key`) — transport `web` for
//!   session tokens, `api` for API keys / OAuth tokens
//! - MCP (`mcp::with_request_user`) — transport `mcp`
//! - CLI (`main`) — no task-local; sets the process-wide default to `cli`
//!
//! Resolution order in [`current`]: task-local → process default →
//! `system`. The single-writer architecture (one Mutex-guarded write
//! connection) is what makes stamping a plain table race-free.

use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// Browser session (lific_sess_ token).
    Web,
    /// MCP request (any token presented to /mcp).
    Mcp,
    /// Direct REST API usage with an API key or OAuth token.
    Api,
    /// Local CLI commands.
    Cli,
    /// No actor context — migrations, startup, tests that don't set one.
    System,
}

impl Transport {
    pub fn as_str(&self) -> &'static str {
        match self {
            Transport::Web => "web",
            Transport::Mcp => "mcp",
            Transport::Api => "api",
            Transport::Cli => "cli",
            Transport::System => "system",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ActorCtx {
    pub user_id: Option<i64>,
    pub transport: Transport,
}

tokio::task_local! {
    /// Per-request actor identity. Set via [`scope`] at each entry surface.
    static ACTOR: ActorCtx;
}

/// Process-wide fallback transport for surfaces that don't run inside a
/// request task (the CLI). Set once at startup; never set by the server.
static DEFAULT_TRANSPORT: OnceLock<Transport> = OnceLock::new();

/// Declare the process-default transport (CLI entrypoint calls this with
/// [`Transport::Cli`]). Later calls are no-ops.
pub fn set_default_transport(t: Transport) {
    let _ = DEFAULT_TRANSPORT.set(t);
}

/// Run `fut` with the given actor identity in scope.
pub async fn scope<F: std::future::Future>(ctx: ActorCtx, fut: F) -> F::Output {
    ACTOR.scope(ctx, fut).await
}

/// The actor for the current execution context.
/// Task-local (request) → process default (CLI) → system.
pub fn current() -> ActorCtx {
    ACTOR.try_with(|a| *a).unwrap_or(ActorCtx {
        user_id: None,
        transport: *DEFAULT_TRANSPORT.get().unwrap_or(&Transport::System),
    })
}

/// Stamp the actor onto a write connection's `_actor_state` row so the
/// audit triggers attribute the writes that follow. Best-effort by design:
/// a failed stamp must never block the actual mutation (worst case the
/// audit row carries the previous actor; the table always exists after
/// migration 018).
pub fn stamp(conn: &rusqlite::Connection, ctx: &ActorCtx) {
    let _ = conn.execute(
        "UPDATE _actor_state SET user_id = ?1, transport = ?2 WHERE id = 1",
        rusqlite::params![ctx.user_id, ctx.transport.as_str()],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_without_scope_is_system() {
        let actor = current();
        assert!(actor.user_id.is_none());
        // Default transport may have been set by another test binary path;
        // in the library test binary nothing sets it, so this is System.
        assert_eq!(actor.transport, Transport::System);
    }

    #[tokio::test]
    async fn scoped_actor_is_visible_inside_scope() {
        let ctx = ActorCtx {
            user_id: Some(7),
            transport: Transport::Mcp,
        };
        let seen = scope(ctx, async { current() }).await;
        assert_eq!(seen.user_id, Some(7));
        assert_eq!(seen.transport, Transport::Mcp);
    }

    #[tokio::test]
    async fn nested_scopes_inner_wins() {
        let outer = ActorCtx {
            user_id: Some(1),
            transport: Transport::Api,
        };
        let inner = ActorCtx {
            user_id: Some(2),
            transport: Transport::Mcp,
        };
        let seen = scope(
            outer,
            async move { scope(inner, async { current() }).await },
        )
        .await;
        assert_eq!(seen.user_id, Some(2));
        assert_eq!(seen.transport, Transport::Mcp);
    }
}
