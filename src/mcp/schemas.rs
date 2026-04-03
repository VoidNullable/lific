use rmcp::schemars;
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchInput {
    #[schemars(description = "Text to search for across issues and pages")]
    pub query: String,
    #[schemars(description = "Filter to a specific project (e.g. LIF)")]
    pub project: Option<String>,
    #[schemars(description = "Max results (default 20)")]
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListIssuesInput {
    #[schemars(description = "Project identifier (e.g. LIF)")]
    pub project: String,
    #[schemars(description = "Filter by status: backlog, todo, active, done, cancelled")]
    pub status: Option<String>,
    #[schemars(description = "Filter by priority: urgent, high, medium, low, none")]
    pub priority: Option<String>,
    #[schemars(description = "Filter by module name")]
    pub module: Option<String>,
    #[schemars(description = "Filter by label name")]
    pub label: Option<String>,
    #[schemars(description = "Only return issues with no unresolved blockers")]
    pub workable: Option<bool>,
    #[schemars(description = "Max results (default 50)")]
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetIssueInput {
    #[schemars(description = "Issue identifier like PRO-42 or ADA-7")]
    pub identifier: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateIssueInput {
    #[schemars(description = "Project identifier (e.g. LIF)")]
    pub project: String,
    #[schemars(description = "Issue title")]
    pub title: String,
    #[schemars(description = "Markdown description")]
    pub description: Option<String>,
    #[schemars(description = "Status: backlog, todo, active, done, cancelled (default: backlog)")]
    pub status: Option<String>,
    #[schemars(description = "Priority: urgent, high, medium, low, none (default: none)")]
    pub priority: Option<String>,
    #[schemars(description = "Module name to assign to")]
    pub module: Option<String>,
    #[schemars(description = "Label names to attach")]
    pub labels: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateIssueInput {
    #[schemars(description = "Issue identifier like PRO-42")]
    pub identifier: String,
    #[schemars(description = "New title")]
    pub title: Option<String>,
    #[schemars(description = "New description (markdown)")]
    pub description: Option<String>,
    #[schemars(description = "New status: backlog, todo, active, done, cancelled")]
    pub status: Option<String>,
    #[schemars(description = "New priority: urgent, high, medium, low, none")]
    pub priority: Option<String>,
    #[schemars(description = "New module name")]
    pub module: Option<String>,
    #[schemars(description = "Replace labels")]
    pub labels: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetBoardInput {
    #[schemars(description = "Project identifier (e.g. LIF)")]
    pub project: String,
    #[schemars(description = "Group by: status, priority, or module (default: status)")]
    pub group_by: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LinkIssuesInput {
    #[schemars(description = "Source issue identifier (e.g. PRO-1)")]
    pub source: String,
    #[schemars(description = "Target issue identifier (e.g. PRO-2)")]
    pub target: String,
    #[schemars(description = "Relation type: blocks, relates_to, or duplicate")]
    pub relation_type: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UnlinkIssuesInput {
    #[schemars(description = "First issue identifier")]
    pub source: String,
    #[schemars(description = "Second issue identifier")]
    pub target: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetPageInput {
    #[schemars(description = "Page identifier like LIF-DOC-1")]
    pub identifier: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreatePageInput {
    #[schemars(description = "Project identifier (e.g. LIF). Omit for workspace-level page.")]
    pub project: Option<String>,
    #[schemars(description = "Page title")]
    pub title: String,
    #[schemars(description = "Markdown content")]
    pub content: Option<String>,
    #[schemars(description = "Folder name to place page in")]
    pub folder: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdatePageInput {
    #[schemars(description = "Page identifier like LIF-DOC-1")]
    pub identifier: String,
    #[schemars(description = "New title")]
    pub title: Option<String>,
    #[schemars(description = "New markdown content")]
    pub content: Option<String>,
    #[schemars(description = "Move to folder name")]
    pub folder: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteInput {
    #[schemars(
        description = "Type of thing to delete: issue, page, project, module, label, or folder"
    )]
    pub resource_type: String,
    #[schemars(
        description = "Identifier or name (e.g. LIF-1, LIF-DOC-1, LIF for projects, or name for modules/labels/folders)"
    )]
    pub identifier: String,
    #[schemars(
        description = "Project identifier (required for deleting module/label/folder by name)"
    )]
    pub project: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListResourcesInput {
    #[schemars(description = "Resource type: project, module, label, folder, page, or issue")]
    pub resource_type: String,
    #[schemars(description = "Project identifier (required for most types)")]
    pub project: Option<String>,
    #[schemars(description = "Folder name (for page filtering)")]
    pub folder: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ManageResourceInput {
    #[schemars(description = "Resource type: project, module, label, or folder")]
    pub resource_type: String,
    #[schemars(description = "Action: create or update")]
    pub action: String,
    #[schemars(description = "Resource name (required for update, identifies which to update)")]
    pub current_name: Option<String>,
    #[schemars(description = "Project identifier (for create, e.g. LIF)")]
    pub project: Option<String>,
    #[schemars(description = "Name")]
    pub name: Option<String>,
    #[schemars(description = "Identifier (for project create, e.g. PRO)")]
    pub identifier: Option<String>,
    #[schemars(description = "Description")]
    pub description: Option<String>,
    #[schemars(
        description = "Status (for module: backlog, planned, active, paused, done, cancelled)"
    )]
    pub status: Option<String>,
    #[schemars(description = "Color hex (for label, e.g. #EF4444)")]
    pub color: Option<String>,
}
