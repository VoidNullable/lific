pub(crate) mod schemas;
pub(crate) mod tools;

use std::sync::Arc;
use std::sync::Mutex;

use rmcp::{
    ServerHandler,
    handler::server::router::tool::ToolRouter,
    model::{ProtocolVersion, ServerCapabilities, ServerInfo},
};

use crate::db::DbPool;
use crate::db::models::AuthUser;

/// Serialization lock for MCP request handling.
/// Ensures only one MCP request processes at a time, preventing the race
/// condition where concurrent requests could overwrite each other's user identity.
/// Acceptable throughput cost for a local-first, single-user tool.
static MCP_HANDLER_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Per-request user identity storage.
/// Protected from races by MCP_HANDLER_LOCK ensuring serial access.
/// Uses unwrap_or_else to recover from poison (e.g. if a handler panics).
static MCP_REQUEST_USER: Mutex<Option<AuthUser>> = Mutex::new(None);

/// Acquire the MCP handler lock, set the user, run the provided future,
/// then clean up. Guarantees no identity confusion between concurrent requests.
///
/// LIF-155: also scopes the audit actor context (transport = mcp) around
/// the handler so every DB write a tool performs is attributed to this
/// user via MCP — both the OAuth /mcp route and the authless /mcp/<token>
/// route funnel through here.
pub async fn with_request_user<F, Fut, R>(user: Option<AuthUser>, f: F) -> R
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = R>,
{
    let _guard = MCP_HANDLER_LOCK.lock().await;
    let actor = crate::actor::ActorCtx {
        user_id: user.as_ref().map(|u| u.id),
        transport: crate::actor::Transport::Mcp,
    };
    *MCP_REQUEST_USER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = user;
    let result = crate::actor::scope(actor, f()).await;
    *MCP_REQUEST_USER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    result
}

/// Get the authenticated user for the current MCP request, if any.
pub(crate) fn current_auth_user() -> Option<AuthUser> {
    MCP_REQUEST_USER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

#[derive(Clone)]
pub struct LificMcp {
    db: Arc<DbPool>,
    tool_router: ToolRouter<Self>,
}

impl LificMcp {
    pub fn new(db: DbPool) -> Self {
        Self {
            db: Arc::new(db),
            tool_router: Self::create_tool_router(),
        }
    }

    fn read<F, T>(&self, f: F) -> Result<T, String>
    where
        F: FnOnce(&rusqlite::Connection) -> Result<T, crate::error::LificError>,
    {
        let conn = self.db.read().map_err(|e| e.to_string())?;
        f(&conn).map_err(|e| e.to_string())
    }

    fn write<F, T>(&self, f: F) -> Result<T, String>
    where
        F: FnOnce(&rusqlite::Connection) -> Result<T, crate::error::LificError>,
    {
        let conn = self.db.write().map_err(|e| e.to_string())?;
        // LIF-155: re-stamp the audit actor from the MCP request-user
        // global. The task-local stamped by DbPool::write() does NOT
        // survive rmcp's internal task spawns (verified in production:
        // tool writes attributed to 'system'), but MCP_REQUEST_USER does
        // — it's a global guarded by the serialization lock, so it is
        // exactly this request's identity.
        let user = current_auth_user();
        crate::actor::stamp(
            &conn,
            &crate::actor::ActorCtx {
                user_id: user.map(|u| u.id),
                transport: crate::actor::Transport::Mcp,
            },
        );
        f(&conn).map_err(|e| e.to_string())
    }
}

impl ServerHandler for LificMcp {
    fn get_info(&self) -> ServerInfo {
        // Pin to 2025-03-26: rmcp defaults to 2025-06-18 which many clients
        // (including Zed) skipped, going straight from 2025-03-26 to 2025-11-25.
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(ProtocolVersion::V_2025_03_26)
            .with_instructions(
                "Lific is a local-first issue tracker. Use list_resources(type='project') to discover projects. \
                 Use list_issues to browse issues with filters. Use get_issue with an identifier like 'PRO-42' \
                 for details. Use workable=true to find issues ready to work on (no unresolved blockers). \
                 Use search to find anything by text across issues and pages.",
            )
    }

    fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<rmcp::model::ListToolsResult, rmcp::ErrorData>>
    + rmcp::service::MaybeSendFuture
    + '_ {
        std::future::ready(Ok(rmcp::model::ListToolsResult {
            tools: self.tool_router.list_all(),
            ..Default::default()
        }))
    }

    fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<rmcp::model::CallToolResult, rmcp::ErrorData>>
    + rmcp::service::MaybeSendFuture
    + '_ {
        let tool_context =
            rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        self.tool_router.call(tool_context)
    }

    fn get_tool(&self, name: &str) -> Option<rmcp::model::Tool> {
        self.tool_router.get(name).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Extension, Router,
        body::Body,
        http::{Request, StatusCode},
        middleware,
        routing::get,
    };
    use http_body_util::BodyExt;
    use rusqlite::params;
    use tower::ServiceExt;

    // ── LIF-204: OAuth-token user_id -> resolved AuthUser (MCP path) ─────
    //
    // The /mcp route (see main.rs) sits behind the same `require_api_key`
    // REST middleware, then pulls `Extension<Option<AuthUser>>` out of the
    // request and threads it into `with_request_user` so MCP tools can read
    // it back via `current_auth_user()`. This test reproduces that exact
    // wiring (minus the rmcp transport itself) to prove an OAuth-token-backed
    // MCP session resolves to the correct, real user rather than None.

    fn test_hex_encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn insert_oauth_token(pool: &DbPool, suffix: &str, user_id: Option<i64>) -> String {
        use sha2::{Digest, Sha256};
        let token = format!("lific_at_test-{suffix}");
        let hash = test_hex_encode(&Sha256::digest(token.as_bytes()));
        let expires = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        let client_id = format!("client-{suffix}");
        let conn = pool.write().unwrap();
        conn.execute(
            "INSERT INTO oauth_clients (client_id, client_name, redirect_uris) VALUES (?1, 'Test', '[\"http://localhost\"]')",
            params![client_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO oauth_tokens (access_token, client_id, expires_at, scope, user_id) VALUES (?1, ?2, ?3, 'mcp', ?4)",
            params![hash, client_id, expires, user_id],
        )
        .unwrap();
        token
    }

    /// Mirrors the production `/mcp` route in main.rs: `require_api_key`
    /// resolves the bearer token to `Extension<Option<AuthUser>>`, then the
    /// handler extracts it and runs `with_request_user` around the rest of
    /// the request. Here the "rest of the request" is just reading back
    /// `current_auth_user()`, which is what every MCP tool does.
    fn mcp_echo_app(auth_state: crate::auth::AuthState) -> Router {
        async fn echo(Extension(auth_user): Extension<Option<AuthUser>>) -> String {
            crate::mcp::with_request_user(auth_user, || async {
                match crate::mcp::current_auth_user() {
                    Some(u) => format!("user:{}:{}:{}", u.id, u.username, u.is_admin),
                    None => "none".to_string(),
                }
            })
            .await
        }
        Router::new().route("/mcp-echo", get(echo)).layer(
            middleware::from_fn_with_state(auth_state, crate::auth::require_api_key),
        )
    }

    #[tokio::test]
    async fn oauth_token_backed_mcp_session_resolves_current_auth_user() {
        let pool = crate::db::open_memory().expect("test db");
        let user_id = {
            let conn = pool.write().unwrap();
            crate::db::queries::users::create_user(
                &conn,
                &crate::db::models::CreateUser {
                    username: "mcp-token-user".into(),
                    email: "mcp-token-user@test.com".into(),
                    password: "testpassword1".into(),
                    display_name: Some("MCP Token User".into()),
                    is_admin: false,
                    is_bot: false,
                },
            )
            .unwrap()
            .id
        };
        let token = insert_oauth_token(&pool, "mcp", Some(user_id));

        let auth_state = crate::auth::AuthState {
            db: pool.clone(),
            manager: crate::auth::create_key_manager().unwrap(),
            public_url: "https://example.com".into(),
        };

        let resp = mcp_echo_app(auth_state)
            .oneshot(
                Request::builder()
                    .uri("/mcp-echo")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            bytes.as_ref(),
            format!("user:{user_id}:mcp-token-user:false").as_bytes(),
            "OAuth-token-backed MCP session must resolve current_auth_user() to the bound user"
        );

        // The global must be cleared after the request completes so it
        // never leaks into an unrelated subsequent request.
        assert!(current_auth_user().is_none());
    }
}
