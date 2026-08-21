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
use crate::storage::AttachmentStore;

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

/// The `LIFIC_TOKEN` a stdio MCP session was launched with, plus what it takes
/// to check it.
///
/// `LIFIC_TOKEN` is an API key by documented contract, so this is the same
/// credential the HTTP transport would validate per request. A stdio session
/// has no per-request credential to validate, which is exactly the problem:
/// the process can outlive the key by days. Keeping the raw token here lets
/// [`LificMcp::with_stdio_auth`] re-check it on every tool call, so revoking
/// the key takes effect at the next call instead of the next restart.
///
/// The token is never logged, never rendered, and never leaves this struct;
/// there is deliberately no `Debug`, `Display` or accessor for it.
pub(crate) struct StdioAuth {
    token: String,
    manager: api_keys_simplified::ApiKeyManagerV0,
}

impl StdioAuth {
    pub(crate) fn new(token: String, manager: api_keys_simplified::ApiKeyManagerV0) -> Self {
        Self { token, manager }
    }

    /// Resolve the token against the database as it stands *now*.
    ///
    /// `Ok(Some(user))` is a valid key bound to a live identity;
    /// `Ok(None)` is a valid but unbound key, which keeps the operator
    /// fallback; `Err` is a key that no longer authenticates at all (revoked,
    /// expired, belonging to a deactivated account or to a bot whose owner was
    /// deactivated), or a database failure, which fails closed.
    fn resolve(&self, db: &DbPool) -> Result<Option<AuthUser>, String> {
        crate::auth::resolve_api_key_user(db, &self.manager, &self.token)
    }
}

/// The stdio credential did not revalidate, so the tool was not run.
///
/// Deliberately carries nothing. Why the token failed (revoked, expired, owner
/// deactivated, database unavailable) is operator information and goes to the
/// log; the agent gets one message and one instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StdioAuthFailed;

impl StdioAuthFailed {
    /// What the agent is told. No database state, no distinction between
    /// causes, just the action that fixes every one of them.
    const MESSAGE: &'static str = "This Lific session's credential (LIFIC_TOKEN) is no longer \
         valid, so the tool was not run. Run `lific connect` to reconnect this tool, then \
         restart the MCP server.";

    fn into_tool_result(self) -> rmcp::model::CallToolResult {
        rmcp::model::CallToolResult::error(vec![rmcp::model::Content::text(Self::MESSAGE)])
    }
}

/// LIFIC-11: the resolved identity for the current MCP request. MCP now
/// resolves the caller exactly as REST does — via [`crate::resolve_caller::resolve_caller`] —
/// so a credential-less request (unbound API key, legacy OAuth token, or a
/// stdio session with no bound user) falls back to the first admin, and the
/// gates read `identity.user.is_admin`. The separate operator flag is gone:
/// every credential that authenticates is trusted as the operator, matching
/// REST one-for-one (no transport-specific divergence).
pub(crate) fn current_identity(
    db: &crate::db::DbPool,
) -> Option<crate::resolve_caller::ResolvedIdentity> {
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
    /// LIF-418: where `upload_attachment` writes bytes and `get_attachment`
    /// reads them. Derived from the pool's database path, which is the same
    /// resolution `server.rs` uses for REST's store, so both transports hit
    /// one content-addressed directory.
    store: AttachmentStore,
    tool_router: ToolRouter<Self>,
    /// Present only for a stdio session launched with a `LIFIC_TOKEN`. `None`
    /// covers both the HTTP transport (where per-request middleware already
    /// owns identity, and where re-entering [`with_request_context`] here would
    /// deadlock on [`MCP_HANDLER_LOCK`]) and a tokenless local stdio session,
    /// which keeps its credential-less operator behavior.
    stdio_auth: Option<Arc<StdioAuth>>,
}

impl LificMcp {
    /// An HTTP-transport server with a private realtime hub. Production wires
    /// [`Self::with_realtime`] (to share the hub) and [`Self::for_stdio`];
    /// this is the convenience shape the test suites build on.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new(db: DbPool) -> Self {
        Self::with_realtime(db, RealtimeHub::new())
    }

    pub fn with_realtime(db: DbPool, realtime: RealtimeHub) -> Self {
        let store = AttachmentStore::from_db_path(db.path());
        Self {
            db: Arc::new(db),
            realtime,
            store,
            tool_router: Self::create_tool_router(),
            stdio_auth: None,
        }
    }

    /// The `lific mcp` (stdio) constructor.
    ///
    /// `auth` is `Some` when the session was launched with a `LIFIC_TOKEN`,
    /// which is then revalidated before every tool call. `None` is the
    /// tokenless local session: no credential to check, operator behavior, the
    /// same as before.
    pub fn for_stdio(db: DbPool, auth: Option<StdioAuth>) -> Self {
        Self {
            stdio_auth: auth.map(Arc::new),
            ..Self::with_realtime(db, RealtimeHub::new())
        }
    }

    /// The stdio revalidation seam. Every tool call goes through here.
    ///
    /// With no stdio credential this is a pass-through, which is what the HTTP
    /// transport needs: `server.rs` already wraps each request in
    /// [`with_request_context`], and taking [`MCP_HANDLER_LOCK`] a second time
    /// here would deadlock.
    ///
    /// With one, the token is re-resolved against the database *on this call*
    /// and the tool runs inside [`with_request_user`] with whatever came back,
    /// so both the authorization gates and the audit actor read the identity as
    /// it is now rather than as it was at launch. A token that no longer
    /// authenticates returns [`StdioAuthFailed`] and `f` is never awaited, so a
    /// revoked agent cannot mutate anything on its next call.
    ///
    /// The failure is a typed local error rather than a wire type, so the seam
    /// stays reusable and the decision about how a client should see it lives
    /// in one place ([`Self::dispatch_tool`]).
    async fn with_stdio_auth<F, Fut, R>(&self, f: F) -> Result<R, StdioAuthFailed>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = R>,
    {
        let Some(auth) = self.stdio_auth.clone() else {
            return Ok(f().await);
        };
        let user = auth.resolve(&self.db).map_err(|reason| {
            // The reason names DB state (revoked, expired, deactivated owner,
            // backend fault). It is useful in the operator's log and is not
            // something the agent needs, or should be told.
            tracing::warn!(reason, "stdio LIFIC_TOKEN no longer authenticates");
            StdioAuthFailed
        })?;
        Ok(with_request_user(user, f).await)
    }

    /// The central tool-call seam: revalidate, then dispatch.
    ///
    /// A stdio credential that no longer authenticates comes back as a failed
    /// **tool result** (`is_error: true`) rather than a JSON-RPC protocol
    /// error. The request itself was perfectly well formed, so `-32600
    /// Invalid Request` is a lie about what went wrong, and MCP clients treat
    /// protocol errors as a broken server: several drop the session or stop
    /// surfacing anything to the model. A tool error reaches the agent as text
    /// it can read and act on, which is the whole point of telling it to
    /// reconnect. Either way `f` never runs.
    async fn dispatch_tool<F, Fut>(
        &self,
        f: F,
    ) -> Result<rmcp::model::CallToolResult, rmcp::ErrorData>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<rmcp::model::CallToolResult, rmcp::ErrorData>>,
    {
        match self.with_stdio_auth(f).await {
            Ok(result) => result,
            Err(StdioAuthFailed) => Ok(StdioAuthFailed.into_tool_result()),
        }
    }

    /// Point the attachment tools at an explicit store. An in-memory pool has
    /// no real database file to derive a directory from, so tests that upload
    /// bytes hand in a scratch directory instead of writing into the process's
    /// working directory.
    #[cfg(test)]
    pub(crate) fn with_attachment_store(mut self, store: AttachmentStore) -> Self {
        self.store = store;
        self
    }

    fn emit(&self, event: RealtimeEvent) {
        self.realtime.send(event);
    }

    /// LIF-387: one borrowed read connection for a tool's whole pre-flight
    /// (resolve the project, then its module or folder), instead of a fresh
    /// checkout per resolver. Reads never block each other, so holding it
    /// across the authz gates that follow costs nothing.
    pub(crate) fn read_conn(&self) -> Result<crate::db::ReadConn, String> {
        self.db.read().map_err(sanitize_error)
    }

    fn read<F, T>(&self, f: F) -> Result<T, String>
    where
        F: FnOnce(&rusqlite::Connection) -> Result<T, crate::error::LificError>,
    {
        let conn = self.db.read().map_err(sanitize_error)?;
        f(&conn).map_err(sanitize_error)
    }

    /// LIF-155: re-stamp the audit actor from the MCP request-user global.
    /// The task-local stamped by `DbPool::write()` does NOT survive rmcp's
    /// internal task spawns (verified in production: tool writes attributed
    /// to 'system'), but `MCP_REQUEST_USER` does — it's a global guarded by
    /// the serialization lock, so it is exactly this request's identity.
    fn stamp_request_actor(conn: &rusqlite::Connection) {
        let user = current_auth_user();
        crate::actor::stamp(
            conn,
            &crate::actor::ActorCtx {
                user_id: user.map(|user| user.id),
                transport: crate::actor::Transport::Mcp,
            },
        );
    }

    fn write<F, T>(&self, f: F) -> Result<T, String>
    where
        F: FnOnce(&rusqlite::Connection) -> Result<T, crate::error::LificError>,
    {
        let conn = self.db.write().map_err(sanitize_error)?;
        Self::stamp_request_actor(&conn);
        f(&conn).map_err(sanitize_error)
    }

    fn transaction<F, T>(&self, f: F) -> Result<T, String>
    where
        F: FnOnce(&rusqlite::Connection) -> Result<T, crate::error::LificError>,
    {
        self.db
            .transaction(|conn| {
                Self::stamp_request_actor(conn);
                f(conn)
            })
            .map_err(sanitize_error)
    }
}

/// LIF-411: the MCP twin of REST's [`LificError::into_response`] sanitization
/// (`error.rs`). `Database` and `Internal` carry raw SQLite/driver text —
/// table and column names, constraint bodies, file paths — which REST has
/// never returned to a client and MCP was handing straight to the agent (and
/// therefore to whoever reads its transcript). Log the detail server-side,
/// return the same generic message REST does. Every other variant is a
/// caller-facing message and passes through unchanged.
pub(crate) fn sanitize_error(error: crate::error::LificError) -> String {
    use crate::error::LificError;
    match &error {
        LificError::Database(inner) => {
            tracing::error!(error = %inner, "database error");
            "internal server error".to_string()
        }
        LificError::Internal(message) => {
            tracing::error!(error = %message, "internal error");
            "internal server error".to_string()
        }
        other => other.to_string(),
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

    /// The one place every MCP tool call passes through, whatever the
    /// transport. The stdio credential check lives here rather than in each
    /// tool for exactly that reason.
    ///
    /// Not an `async fn`: the `rmcp` trait declares an explicit
    /// `MaybeSendFuture` bound on the return type, which the desugared form
    /// has to restate.
    #[allow(clippy::manual_async_fn)]
    fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<rmcp::model::CallToolResult, rmcp::ErrorData>>
    + rmcp::service::MaybeSendFuture
    + '_ {
        async move {
            let tool_context =
                rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
            self.dispatch_tool(|| self.tool_router.call(tool_context))
                .await
        }
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

    /// LIF-411: `Database` and `Internal` carry driver/schema detail REST has
    /// always withheld from clients; every other variant is a message written
    /// for the caller and must survive intact.
    #[test]
    fn sanitize_error_hides_only_database_and_internal_detail() {
        use crate::error::LificError;

        assert_eq!(
            sanitize_error(LificError::Database(
                rusqlite::Error::InvalidColumnName("secret_column".into())
            )),
            "internal server error"
        );
        assert_eq!(
            sanitize_error(LificError::Internal("/srv/lific/lific.db is locked".into())),
            "internal server error"
        );

        for (error, expected) in [
            (
                LificError::NotFound("issue LIF-1 not found".into()),
                "Not found: issue LIF-1 not found",
            ),
            (
                LificError::BadRequest("invalid status 'nope'".into()),
                "Bad request: invalid status 'nope'",
            ),
            (
                LificError::Forbidden("requires at least 'maintainer' access to this project".into()),
                "Forbidden: requires at least 'maintainer' access to this project",
            ),
            (
                LificError::Conflict("identifier already exists".into()),
                "Conflict: identifier already exists",
            ),
        ] {
            assert_eq!(sanitize_error(error), expected);
        }
    }

    fn insert_oauth_token(pool: &DbPool, suffix: &str, user_id: Option<i64>) -> String {
        let token = format!("lific_at_test-{suffix}");
        let hash = crate::auth::sha256_hex(token.as_bytes());
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
        let unbound_key =
            crate::auth::create_api_key(&pool, &manager, "mcp-operator", None).unwrap();
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

    fn seed_user(
        pool: &crate::db::DbPool,
        username: &str,
        admin: bool,
    ) -> crate::db::models::AuthUser {
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

    /// Seed a human owner plus a connected-tool bot they own, and mint the
    /// bot an API key: the exact shape `lific connect` produces and hands to
    /// an agent as `LIFIC_TOKEN`.
    fn connected_agent(
        pool: &crate::db::DbPool,
        manager: &api_keys_simplified::ApiKeyManagerV0,
    ) -> (crate::db::models::AuthUser, crate::db::models::User, String) {
        // A separate instance admin exists as the operator fallback, so
        // deactivating the owner is not "the last admin" and the agent's
        // identity is provably not just that fallback.
        seed_user(pool, "operator", true);
        let owner = seed_user(pool, "owner", false);
        let bot = {
            let conn = pool.write().expect("write conn");
            crate::db::queries::users::create_bot_user(
                &conn,
                owner.id,
                "opencode-owner",
                "OpenCode",
                Some("opencode"),
            )
            .expect("create bot")
        };
        let token = crate::auth::create_api_key(pool, manager, "opencode-owner", Some(bot.id))
            .expect("mint agent key");
        (owner, bot, token)
    }

    fn server_for(pool: &crate::db::DbPool, auth: Option<StdioAuth>) -> LificMcp {
        LificMcp::for_stdio(pool.clone(), auth)
    }

    /// Run a body through the revalidation seam and report what identity it
    /// saw. Standing in for a real tool body: `with_stdio_auth` is what every
    /// tool runs inside, so whatever this observes, a tool observes.
    async fn observed_identity(
        server: &LificMcp,
        pool: &crate::db::DbPool,
    ) -> Result<Option<crate::resolve_caller::ResolvedIdentity>, StdioAuthFailed> {
        server
            .with_stdio_auth(|| async { current_identity(pool) })
            .await
    }

    /// The shape `dispatch_tool` dispatches: a boxed future producing what the
    /// real `ToolRouter::call` produces.
    type ToolBody = std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<rmcp::model::CallToolResult, rmcp::ErrorData>>
                + Send,
        >,
    >;

    /// A tool body that records whether it ran and writes if it does, plus the
    /// flag to check afterwards. Anything that reaches this has been let
    /// through the seam.
    fn mutating_tool(
        pool: &crate::db::DbPool,
    ) -> (
        impl FnOnce() -> ToolBody + use<>,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = ran.clone();
        let pool = pool.clone();
        let body = move || {
            Box::pin(async move {
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
                let conn = pool.write().unwrap();
                conn.execute(
                    "INSERT INTO oauth_clients (client_id, client_name, redirect_uris)
                     VALUES ('mutation-marker', 'Tool ran', '[]')",
                    [],
                )
                .unwrap();
                Ok(rmcp::model::CallToolResult::success(vec![
                    rmcp::model::Content::text("ran"),
                ]))
            }) as ToolBody
        };
        (body, ran)
    }

    #[tokio::test]
    async fn a_stdio_tool_call_resolves_as_the_agent_the_token_names() {
        let _sguard = crate::mcp::tools::acquire_test_guard();
        let pool = crate::db::open_memory().expect("test db");
        let manager = crate::auth::create_key_manager().unwrap();
        let (owner, bot, token) = connected_agent(&pool, &manager);
        let server = server_for(&pool, Some(StdioAuth::new(token, manager)));

        let identity = observed_identity(&server, &pool)
            .await
            .expect("a live token authenticates")
            .expect("a bound stdio session resolves");
        assert_eq!(
            identity.user.id, bot.id,
            "the audit actor is the bot, not the operator it inherits from"
        );
        assert_ne!(identity.user.id, owner.id);
        assert_eq!(identity.transport, crate::actor::Transport::Mcp);
    }

    /// The whole point of the seam: a long-running session must not keep
    /// working after its credential is revoked.
    #[tokio::test]
    async fn revoking_the_token_stops_the_very_next_tool_call() {
        let _sguard = crate::mcp::tools::acquire_test_guard();
        let pool = crate::db::open_memory().expect("test db");
        let manager = crate::auth::create_key_manager().unwrap();
        let (_owner, _bot, token) = connected_agent(&pool, &manager);
        let server = server_for(&pool, Some(StdioAuth::new(token, manager)));

        assert!(observed_identity(&server, &pool).await.is_ok());

        crate::auth::revoke_api_key(&pool, "opencode-owner").expect("revoke");

        // Through `dispatch_tool`, which is the exact seam `call_tool` uses,
        // so this is the mapping a client actually receives.
        let (body, ran) = mutating_tool(&pool);
        let result = server
            .dispatch_tool(body)
            .await
            .expect("a dead credential is a tool failure, not a protocol failure");

        assert_eq!(
            result.is_error,
            Some(true),
            "the agent must see this as a failed tool call"
        );
        assert!(
            !ran.load(std::sync::atomic::Ordering::SeqCst),
            "the tool body must not run at all, so nothing is mutated"
        );
        let mutations: i64 = pool
            .read()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM oauth_clients WHERE client_id = 'mutation-marker'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(mutations, 0, "nothing was written");

        // The message tells the agent what to do and leaks no database state.
        let text = format!("{:?}", result.content);
        assert!(text.contains("lific connect"), "{text}");
        assert!(text.contains("restart"), "{text}");
        for internal in ["revoked", "deactivated", "database", "expired", "hash"] {
            assert!(
                !text.contains(internal),
                "the agent must not be told why: {text}"
            );
        }
    }

    /// The success side of the same seam: a live credential dispatches, and
    /// the tool's own result comes back untouched.
    #[tokio::test]
    async fn a_live_credential_dispatches_the_tool_through_the_central_seam() {
        let _sguard = crate::mcp::tools::acquire_test_guard();
        let pool = crate::db::open_memory().expect("test db");
        let manager = crate::auth::create_key_manager().unwrap();
        let (_owner, _bot, token) = connected_agent(&pool, &manager);
        let server = server_for(&pool, Some(StdioAuth::new(token, manager)));

        let (body, ran) = mutating_tool(&pool);
        let result = server.dispatch_tool(body).await.expect("dispatches");

        assert_eq!(result.is_error, Some(false));
        assert!(ran.load(std::sync::atomic::Ordering::SeqCst));
    }

    /// A password change or sign-out-everywhere revokes the owner's bots'
    /// keys, so the same seam catches it.
    #[tokio::test]
    async fn an_account_lockdown_stops_the_agents_next_tool_call() {
        let _sguard = crate::mcp::tools::acquire_test_guard();
        let pool = crate::db::open_memory().expect("test db");
        let manager = crate::auth::create_key_manager().unwrap();
        let (owner, _bot, token) = connected_agent(&pool, &manager);
        let server = server_for(&pool, Some(StdioAuth::new(token, manager)));

        assert!(observed_identity(&server, &pool).await.is_ok());
        {
            let conn = pool.write().unwrap();
            crate::db::queries::users::lock_down_account(&conn, owner.id).unwrap();
        }
        assert!(observed_identity(&server, &pool).await.is_err());
    }

    #[tokio::test]
    async fn deactivating_the_owner_stops_the_agents_next_tool_call() {
        let _sguard = crate::mcp::tools::acquire_test_guard();
        let pool = crate::db::open_memory().expect("test db");
        let manager = crate::auth::create_key_manager().unwrap();
        let (owner, _bot, token) = connected_agent(&pool, &manager);
        let server = server_for(&pool, Some(StdioAuth::new(token, manager)));

        assert!(observed_identity(&server, &pool).await.is_ok());
        {
            let conn = pool.write().unwrap();
            crate::db::queries::users::set_active(&conn, owner.id, false).unwrap();
        }
        assert!(
            observed_identity(&server, &pool).await.is_err(),
            "a bot whose owner is deactivated is a dead credential"
        );
    }

    #[tokio::test]
    async fn an_unbound_key_still_resolves_to_the_operator() {
        let _sguard = crate::mcp::tools::acquire_test_guard();
        let pool = crate::db::open_memory().expect("test db");
        let manager = crate::auth::create_key_manager().unwrap();
        let admin = seed_user(&pool, "operator", true);
        let token = crate::auth::create_api_key(&pool, &manager, "default", None).unwrap();
        let server = server_for(&pool, Some(StdioAuth::new(token, manager)));

        let identity = observed_identity(&server, &pool)
            .await
            .expect("an unbound key is valid")
            .expect("operator fallback resolves");
        assert_eq!(identity.user.id, admin.id);
        assert!(identity.user.is_admin);
    }

    #[tokio::test]
    async fn a_tokenless_stdio_session_keeps_operator_behavior() {
        let _sguard = crate::mcp::tools::acquire_test_guard();
        let pool = crate::db::open_memory().expect("test db");
        let admin = seed_user(&pool, "operator", true);
        let server = server_for(&pool, None);

        let identity = observed_identity(&server, &pool)
            .await
            .expect("no credential to fail")
            .expect("operator fallback resolves");
        assert_eq!(
            identity.user.id, admin.id,
            "no-token stdio session must resolve to the first admin"
        );
        assert_eq!(identity.transport, crate::actor::Transport::Mcp);
    }

    /// The HTTP transport is already wrapped in `with_request_context` by
    /// `server.rs`, which holds `MCP_HANDLER_LOCK` for the whole request. If
    /// the seam took that lock again the request would deadlock, so an
    /// HTTP-shaped server must pass straight through.
    #[tokio::test]
    async fn the_http_transport_seam_does_not_retake_the_handler_lock() {
        let _sguard = crate::mcp::tools::acquire_test_guard();
        let pool = crate::db::open_memory().expect("test db");
        let server = LificMcp::new(pool.clone());
        let user = seed_user(&pool, "http-caller", true);

        let seen = with_request_context(Some(user.clone()), None, || async {
            server
                .with_stdio_auth(|| async { current_auth_user() })
                .await
                .expect("pass-through")
        })
        .await;
        assert_eq!(
            seen.map(|u| u.id),
            Some(user.id),
            "the middleware's identity survives the seam untouched"
        );
    }
}
