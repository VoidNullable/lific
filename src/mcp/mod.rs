pub(crate) mod schemas;
pub(crate) mod tools;

use std::sync::Arc;
use std::sync::Mutex;

use rmcp::{
    handler::server::router::tool::ToolRouter,
    model::{ServerCapabilities, ServerInfo},
    ServerHandler,
};

use crate::db::models::AuthUser;
use crate::db::DbPool;

/// Global storage for the most recent MCP request's authenticated user.
/// This is a workaround for rmcp spawning tool calls on different tasks
/// where tokio::task_local doesn't propagate.
///
/// Safe for single-server use: the mutex serializes access and the value
/// is set immediately before mcp_service.handle() and read during tool execution.
static MCP_REQUEST_USER: Mutex<Option<AuthUser>> = Mutex::new(None);

/// Set the authenticated user for the current MCP request.
pub fn set_request_user(user: Option<AuthUser>) {
    *MCP_REQUEST_USER.lock().unwrap() = user;
}

/// Get the authenticated user for the current MCP request, if any.
pub(crate) fn current_auth_user() -> Option<AuthUser> {
    MCP_REQUEST_USER.lock().unwrap().clone()
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
        f(&conn).map_err(|e| e.to_string())
    }
}

impl ServerHandler for LificMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
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
