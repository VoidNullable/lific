pub(crate) mod schemas;
pub(crate) mod tools;

#[cfg(test)]
use std::cell::Cell;
use std::sync::Arc;
use std::sync::Mutex;

use rmcp::{
    ServerHandler,
    handler::server::router::tool::ToolRouter,
    model::{ProtocolVersion, ServerCapabilities, ServerInfo},
};

use crate::db::DbPool;
use crate::db::models::AuthUser;
use crate::links::IssueLinkContext;
use crate::realtime::{RealtimeEvent, RealtimeHub};

/// Serialization lock for MCP request handling.
/// Ensures only one MCP request processes at a time, preventing the race
/// condition where concurrent requests could overwrite each other's user identity.
/// Acceptable throughput cost for a local-first, single-user tool.
static MCP_HANDLER_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Per-request user identity storage.
/// Protected from races by MCP_HANDLER_LOCK ensuring serial access.
/// Uses unwrap_or_else to recover from poison (e.g. if a handler panics).
static MCP_REQUEST_USER: Mutex<Option<AuthUser>> = Mutex::new(None);

/// Per-request external origin used for structured resource links.
/// Protected by [`MCP_HANDLER_LOCK`] for the same reason as the identity state.
static MCP_REQUEST_ISSUE_LINKS: Mutex<Option<Arc<IssueLinkContext>>> = Mutex::new(None);

#[cfg(test)]
tokio::task_local! {
    static TEST_REQUEST_ISSUE_LINKS: Option<Arc<IssueLinkContext>>;
}

#[cfg(test)]
thread_local! {
    static TEST_ISSUE_LINK_CONTEXT_READS: Cell<usize> = const { Cell::new(0) };
}

/// Acquire the MCP handler lock, set the user, run the provided future,
/// then clean up. Guarantees no identity confusion between concurrent requests.
///
/// LIF-155: also scopes the audit actor context (transport = mcp) around
/// the handler so every DB write a tool performs is attributed to this
/// user via MCP — both the OAuth /mcp route and the authless /mcp/<token>
/// route funnel through here.
#[cfg_attr(not(test), allow(dead_code))]
pub async fn with_request_user<F, Fut, R>(user: Option<AuthUser>, f: F) -> R
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = R>,
{
    with_request_context(user, None, f).await
}

/// Run an MCP request with its authenticated identity and optional external
/// origin. The origin is transport metadata: stdio callers pass `None`, while
/// HTTP callers pass the validated browser-facing base URL.
pub async fn with_request_context<F, Fut, R>(
    user: Option<AuthUser>,
    issue_links: Option<IssueLinkContext>,
    f: F,
) -> R
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = R>,
{
    let _guard = MCP_HANDLER_LOCK.lock().await;
    let issue_links = issue_links.map(Arc::new);
    #[cfg(test)]
    let test_issue_links = issue_links.clone();
    let actor = crate::actor::ActorCtx {
        user_id: user.as_ref().map(|u| u.id),
        transport: crate::actor::Transport::Mcp,
    };
    *MCP_REQUEST_USER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = user;
    *MCP_REQUEST_ISSUE_LINKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = issue_links;
    // Panic-safe cleanup: clear the globals on scope exit (including if `f`
    // panics), before `_guard` releases MCP_HANDLER_LOCK (reverse declaration
    // order). Without this, a panicking request would leave a stale user in the
    // process-wide global for the next (concurrent) test to read.
    let _clear = RequestGlobalGuard;
    #[cfg(test)]
    let result = TEST_REQUEST_ISSUE_LINKS
        .scope(test_issue_links, crate::actor::scope(actor, f()))
        .await;
    #[cfg(not(test))]
    let result = crate::actor::scope(actor, f()).await;
    result
}

/// Drops the per-request globals on scope exit (panic-safe). Declared after the
/// globals are set in [`with_request_context`], so it runs before the handler
/// lock is released.
struct RequestGlobalGuard;
impl Drop for RequestGlobalGuard {
    fn drop(&mut self) {
        *MCP_REQUEST_USER
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *MCP_REQUEST_ISSUE_LINKS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

/// Get the authenticated user for the current MCP request, if any.
pub(crate) fn current_auth_user() -> Option<AuthUser> {
    MCP_REQUEST_USER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// LIFIC-18: install the session-level identity for a stdio MCP server.
///
/// A stdio session is one long-lived, serialized process that is never wrapped
/// in [`with_request_context`] (there is no HTTP request to carry the user), so
/// installing the resolved agent here makes every tool call resolve as that
/// agent via [`current_auth_user`] until the process exits. The operator
/// fallback (a missing/unbound `LIFIC_TOKEN`) is the caller passing `None`,
/// which keeps the existing credential-less resolution — also the operator.
pub(crate) fn set_stdio_user(user: Option<AuthUser>) {
    *MCP_REQUEST_USER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = user;
}

/// LIFIC-11: the resolved identity for the current MCP request. MCP now
/// resolves the caller exactly as REST does — via [`crate::resolve_caller::resolve_caller`] —
/// so a credential-less request (unbound API key, legacy OAuth token, or a
/// stdio session with no bound user) falls back to the first admin, and the
/// gates read `identity.user.is_admin`. The separate operator flag is gone:
/// every credential that authenticates is trusted as the operator, matching
/// REST one-for-one (no transport-specific divergence).
pub(crate) fn current_identity(db: &crate::db::DbPool) -> Option<crate::resolve_caller::ResolvedIdentity> {
    crate::resolve_caller::resolve_caller(db, current_auth_user(), crate::actor::Transport::Mcp)
        .ok()
        .flatten()
}

/// Get the validated external origin for structured resource links, if this MCP
/// request arrived through an HTTP transport that knows it.
pub(crate) fn current_issue_link_context() -> Option<Arc<IssueLinkContext>> {
    #[cfg(test)]
    {
        TEST_ISSUE_LINK_CONTEXT_READS.set(TEST_ISSUE_LINK_CONTEXT_READS.get() + 1);
        TEST_REQUEST_ISSUE_LINKS
            .try_with(Clone::clone)
            .unwrap_or(None)
    }
    #[cfg(not(test))]
    MCP_REQUEST_ISSUE_LINKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

#[cfg(test)]
pub(crate) fn reset_issue_link_context_reads() {
    TEST_ISSUE_LINK_CONTEXT_READS.set(0);
}

#[cfg(test)]
pub(crate) fn issue_link_context_reads() -> usize {
    TEST_ISSUE_LINK_CONTEXT_READS.get()
}

/// Per-session instructions handed to every connected MCP agent via
/// `get_info`. This is unconditional context cost on every session, so the
/// convention guidance appended after the discovery guidance is kept tight
/// (imperative, no filler). Extracted as a const so it stays testable.
const SERVER_INSTRUCTIONS: &str = "Lific is a local-first issue tracker. Use list_resources(type='project') to discover projects. \
     Use list_issues to browse issues with filters. Use get_issue with an identifier like 'PRO-42' \
     for details. Use workable=true to find issues ready to work on (no unresolved blockers). \
     Use search to find anything by text across issues and pages. \
     Conventions: when you finish work on an issue, mark it done (status='done'). \
     Organize issues into modules; keep each issue a self-contained work item. \
     Prefer edit_issue/edit_page (exact string replacement) over update_issue/update_page for small changes. \
      Use plans (create_plan/get_plan) for multi-step or multi-session work; steps can mirror issues and stay in sync. On resume, check for existing plans first: list_resources(type='plan', project='X'), then get_plan to see where you left off. \
     Use pages for documentation and design notes.";

#[derive(Clone)]
pub struct LificMcp {
    db: Arc<DbPool>,
    realtime: RealtimeHub,
    tool_router: ToolRouter<Self>,
}

impl LificMcp {
    pub fn new(db: DbPool) -> Self {
        Self::with_realtime(db, RealtimeHub::new())
    }

    pub fn with_realtime(db: DbPool, realtime: RealtimeHub) -> Self {
        Self {
            db: Arc::new(db),
            realtime,
            tool_router: Self::create_tool_router(),
        }
    }

    fn emit(&self, event: RealtimeEvent) {
        self.realtime.send(event);
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

#[cfg(test)]
impl LificMcp {
    /// LIF-201: expose the live `ToolRouter`'s tool names for the
    /// enumeration-derived MCP completeness check (`authz_coverage_tests`).
    /// Reads the same `list_all()` the production `list_tools` MCP handler
    /// serves, so a tool that's registered but forgotten in the
    /// classification manifest can't hide.
    pub(crate) fn list_tool_names(&self) -> Vec<String> {
        self.tool_router
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect()
    }
}

impl ServerHandler for LificMcp {
    fn get_info(&self) -> ServerInfo {
        // Pin to 2025-03-26: rmcp defaults to 2025-06-18 which many clients
        // (including Zed) skipped, going straight from 2025-03-26 to 2025-11-25.
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(ProtocolVersion::V_2025_03_26)
            // Identify as lific, not rmcp's build-env default — this name is
            // what connected clients (and `lific doctor`) display.
            .with_server_info(rmcp::model::Implementation::new(
                "lific",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(SERVER_INSTRUCTIONS)
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
        Router::new()
            .route("/mcp-echo", get(echo))
            .layer(middleware::from_fn_with_state(
                auth_state,
                crate::auth::require_api_key,
            ))
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
            required: true,
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

    #[tokio::test]
    async fn with_request_context_scopes_issue_link_origin() {
        let context = IssueLinkContext::parse("https://tracker.example/base");
        let (seen, global_seen) = with_request_context(None, context, || async {
            let scoped = current_issue_link_context()
                .expect("request origin should be visible")
                .issue_markdown("LIF-1")
                .to_string();
            let global = MCP_REQUEST_ISSUE_LINKS
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
                .expect("production request context should also be populated")
                .issue_markdown("LIF-1")
                .to_string();
            (scoped, global)
        })
        .await;

        assert_eq!(
            seen,
            "[LIF-1](https://tracker.example/base/LIF/issues/LIF-1)"
        );
        assert_eq!(global_seen, seen);
        assert!(current_issue_link_context().is_none());
        assert!(
            MCP_REQUEST_ISSUE_LINKS
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_none()
        );
    }

    // End-to-end: a credential-less MCP request resolves to the first admin
    // (via resolve_caller), so it passes an enforced-mode Viewer gate. LIFIC-11
    // unified MCP onto the same resolve_caller path REST uses, so the old
    // operator-vs-legacy-OAuth distinction is gone: an unbound API key and a
    // legacy unbound OAuth token both authenticate and both resolve to the
    // first admin. Mirrors the /mcp route wiring (require_api_key → with_request_user
    // → current_identity), then runs a real authz gate.
    #[tokio::test]
    async fn unbound_credentials_resolve_to_first_admin_and_pass_mcp_gate() {
        use axum::extract::State;
        use axum::response::IntoResponse;

        let pool = crate::db::open_memory().expect("test db");
        // resolve_caller needs a first_admin to resolve credential-less requests to
        {
            let conn = pool.write().unwrap();
            crate::db::queries::users::create_user(
                &conn,
                &crate::db::models::CreateUser {
                    username: "admin".into(),
                    email: "admin@test.local".into(),
                    password: "adminpass123".into(),
                    display_name: None,
                    is_admin: true,
                    is_bot: false,
                },
            )
            .unwrap();
        }
        let manager = crate::auth::create_key_manager().unwrap();
        let unbound_key = crate::auth::create_api_key(&pool, &manager, "mcp-operator").unwrap();
        let project = {
            let conn = pool.write().unwrap();
            crate::db::queries::settings::update(
                &conn,
                crate::db::queries::settings::InstanceSettingsPatch {
                    authz_enforced: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
            crate::db::queries::create_project(
                &conn,
                &crate::db::models::CreateProject {
                    name: "MCP Gate".into(),
                    identifier: "MGT".into(),
                    description: String::new(),
                    emoji: None,
                    lead_user_id: None,
                },
            )
            .unwrap()
            .id
        };
        let oauth_token = insert_oauth_token(&pool, "mcp-legacy-unbound", None);

        // Route that mirrors main.rs's /mcp identity plumbing, then runs the
        // same authz gate an MCP Viewer-tool would (authz::require_role over
        // the resolved identity).
        async fn gate(
            State((pool, project_id)): State<(DbPool, i64)>,
            axum::Extension(auth_user): axum::Extension<Option<AuthUser>>,
        ) -> axum::response::Response {
            crate::mcp::with_request_user(auth_user, || async {
                let db = std::sync::Arc::new(pool);
                match crate::authz::require_role(
                    &db,
                    &crate::mcp::current_identity(&db),
                    project_id,
                    crate::db::models::Role::Viewer,
                ) {
                    Ok(()) => (StatusCode::OK, "allowed").into_response(),
                    Err(e) => e.into_response(),
                }
            })
            .await
        }

        let auth_state = crate::auth::AuthState {
            db: pool.clone(),
            manager,
            public_url: "https://example.com".into(),
            required: true,
        };
        let app = Router::new()
            .route("/mcp-gate", get(gate))
            .with_state((pool.clone(), project))
            .layer(middleware::from_fn_with_state(
                auth_state,
                crate::auth::require_api_key,
            ));

        let status = |key: String, app: Router| async move {
            app.oneshot(
                Request::builder()
                    .uri("/mcp-gate")
                    .header("authorization", format!("Bearer {key}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
        };

        assert_eq!(
            status(unbound_key, app.clone()).await,
            StatusCode::OK,
            "an unbound API key authenticates and resolves to the first admin, passing the enforced MCP Viewer gate"
        );
        assert_eq!(
            status(oauth_token, app).await,
            StatusCode::OK,
            "a legacy unbound OAuth token also authenticates and resolves to the first admin — MCP and REST now share one resolve_caller path"
        );
    }

    // ── LIF-256: session instructions carry Lific's workflow conventions ──
    //
    // Every connected agent receives these at session start, so the string
    // must keep the discovery guidance AND surface the key conventions
    // (mark done, prefer edit_* for small changes, use plans/pages/modules).
    #[test]
    fn get_info_instructions_include_conventions() {
        let pool = crate::db::open_memory().expect("test db");
        let mcp = LificMcp::new(pool);
        let info = mcp.get_info();
        let instructions = info
            .instructions
            .expect("server info must carry instructions");

        // Discovery guidance is preserved.
        assert!(instructions.contains("list_resources(type='project')"));
        assert!(instructions.contains("workable=true"));

        // Convention guidance is present.
        assert!(
            instructions.contains("done"),
            "instructions must tell agents to mark finished issues done"
        );
        assert!(
            instructions.contains("edit_issue"),
            "instructions must steer agents to edit_issue for small changes"
        );
        assert!(instructions.contains("edit_page"));
        assert!(instructions.contains("modules"));
        assert!(instructions.contains("create_plan"));
        assert!(instructions.contains("check for existing plans"));
        assert!(instructions.contains("list_resources(type='plan', project='X')"));
        assert!(instructions.contains("then get_plan to see where you left off"));
        assert!(instructions.contains("pages for documentation"));
    }

    // Clients display serverInfo.name — it must say lific, not rmcp's
    // build-env default.
    #[test]
    fn get_info_identifies_as_lific() {
        let pool = crate::db::open_memory().expect("test db");
        let mcp = LificMcp::new(pool);
        let info = mcp.get_info();
        assert_eq!(info.server_info.name, "lific");
        assert_eq!(info.server_info.version, env!("CARGO_PKG_VERSION"));
    }

    // The appended convention guidance is unconditional per-session context
    // cost; keep the whole addition tight (~150 tokens / ~600 chars).
    #[test]
    fn server_instructions_stay_compact() {
        let base = "Lific is a local-first issue tracker. Use list_resources(type='project') to discover projects. \
     Use list_issues to browse issues with filters. Use get_issue with an identifier like 'PRO-42' \
     for details. Use workable=true to find issues ready to work on (no unresolved blockers). \
     Use search to find anything by text across issues and pages. ";
        let addition = SERVER_INSTRUCTIONS.len() - base.len();
        assert!(
            addition <= 700,
            "convention addition grew to {addition} chars; keep it tight"
        );
    }

    // ── LIFIC-18: stdio session identity (set_stdio_user seam) ─────────────
    //
    // The `lific mcp` entrypoint installs the session identity once at startup
    // via `set_stdio_user` (see main.rs), and every tool call resolves through
    // `current_identity`. This is seam two of the spec: with a valid bound
    // token the agent resolves as itself; with none, the operator (first
    // admin) fallback applies.

    fn seed_user(pool: &crate::db::DbPool, username: &str, admin: bool) -> crate::db::models::AuthUser {
        let conn = pool.write().expect("write conn");
        let u = crate::db::queries::users::create_user(
            &conn,
            &crate::db::models::CreateUser {
                username: username.into(),
                email: format!("{username}@local.test"),
                password: "somepass123".into(),
                display_name: None,
                is_admin: admin,
                is_bot: false,
            },
        )
        .expect("create user");
        crate::db::models::AuthUser {
            id: u.id,
            username: u.username,
            display_name: u.display_name,
            is_admin: u.is_admin,
        }
    }

    #[test]
    fn stdio_session_with_agent_identity_resolves_as_that_agent() {
        // A first admin exists as the operator fallback, but the bound session
        // must resolve to the agent, NOT the admin.
        let pool = crate::db::open_memory().expect("test db");
        let admin = seed_user(&pool, "admin", true);
        let agent = seed_user(&pool, "opencode-solo", false);

        // Mirrors main.rs: LIFIC_TOKEN resolved to `agent`, installed for the
        // whole session.
        set_stdio_user(Some(agent.clone()));

        let identity = current_identity(&pool).expect("a bound stdio session resolves");
        assert_eq!(
            identity.user, agent,
            "agent session must resolve as the agent, not the operator"
        );
        assert_ne!(identity.user.id, admin.id);
        assert_eq!(identity.transport, crate::actor::Transport::Mcp);

        // Clean up the process-wide global so it never leaks into other tests.
        set_stdio_user(None);
    }

    #[test]
    fn stdio_session_without_identity_falls_back_to_operator() {
        // No LIFIC_TOKEN / unbound → `set_stdio_user(None)` → the operator
        // (first admin) fallback, the same pre-LIFIC-18 behavior.
        let pool = crate::db::open_memory().expect("test db");
        let admin = seed_user(&pool, "operator", true);

        set_stdio_user(None);
        let identity = current_identity(&pool).expect("operator fallback resolves");
        assert_eq!(
            identity.user.id, admin.id,
            "no-token stdio session must resolve to the first admin"
        );
        assert!(identity.user.is_admin);
        assert_eq!(identity.transport, crate::actor::Transport::Mcp);

        set_stdio_user(None);
    }
}
