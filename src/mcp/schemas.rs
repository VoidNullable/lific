use rmcp::schemars;
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct SearchInput {
    #[schemars(description = "Text to search for across issues, pages, and comments")]
    pub query: String,
    #[schemars(description = "Filter to a specific project (e.g. LIF)")]
    pub project: Option<String>,
    #[schemars(description = "Restrict results to one type: issue, page, or comment")]
    pub result_type: Option<String>,
    #[schemars(
        description = "Sort mode: relevance (default, best match first) or recent (most recently updated first)"
    )]
    pub sort: Option<String>,
    #[schemars(description = "Max results (default 20)")]
    pub limit: Option<i64>,
    #[schemars(
        description = "Zero-indexed offset for paging. Output appends a hint when more results exist."
    )]
    pub offset: Option<i64>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
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
    #[schemars(
        description = "Only return issues with at least one unresolved blocker (inverse of workable). Each result lists its blockers as blocked_by:LIF-3,LIF-7."
    )]
    pub blocked: Option<bool>,
    #[schemars(
        description = "Only issues created at/after this ISO date or datetime (e.g. 2026-06-01)"
    )]
    pub created_since: Option<String>,
    #[schemars(description = "Only issues created before this ISO date or datetime (exclusive)")]
    pub created_until: Option<String>,
    #[schemars(
        description = "Only issues updated at/after this ISO date or datetime. Handy for 'what changed recently'."
    )]
    pub updated_since: Option<String>,
    #[schemars(description = "Only issues updated before this ISO date or datetime (exclusive)")]
    pub updated_until: Option<String>,
    #[schemars(
        description = "Sort column: sort_order (default), sequence, created, or updated"
    )]
    pub order_by: Option<String>,
    #[schemars(description = "Sort direction: asc (default) or desc")]
    pub order: Option<String>,
    #[schemars(description = "Max results (default 50)")]
    pub limit: Option<i64>,
    #[schemars(
        description = "Zero-indexed offset for paging. The output appends a hint like 'has_more: use offset=N' when more results exist."
    )]
    pub offset: Option<i64>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct GetIssueInput {
    #[schemars(description = "Issue identifier like PRO-42 or ADA-7")]
    pub identifier: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct GetActivityInput {
    #[schemars(
        description = "What to read history for: an issue identifier (PRO-42), a page identifier (PRO-DOC-3 or DOC-3), or a bare project identifier (PRO) for the whole project's feed"
    )]
    pub identifier: String,
    #[schemars(description = "Max entries (default 30, cap 200)")]
    pub limit: Option<i64>,
    #[schemars(
        description = "Zero-indexed offset for paging. Output appends a hint when more entries exist."
    )]
    pub offset: Option<i64>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
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
    #[schemars(description = "Start date (ISO 8601 date, e.g. 2026-06-01)")]
    pub start_date: Option<String>,
    #[schemars(description = "Target/due date (ISO 8601 date, e.g. 2026-06-15)")]
    pub target_date: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
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
    #[schemars(
        description = "New module name. Omit to leave unchanged; pass an empty string \"\" to unassign the issue from its module. (LIF-145)"
    )]
    pub module: Option<String>,
    #[schemars(description = "Replace labels")]
    pub labels: Option<Vec<String>>,
    #[schemars(description = "New start date (ISO 8601 date, e.g. 2026-06-01)")]
    pub start_date: Option<String>,
    #[schemars(description = "New target/due date (ISO 8601 date, e.g. 2026-06-15)")]
    pub target_date: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct BulkUpdateInput {
    #[schemars(description = "Project identifier (e.g. LIF)")]
    pub project: String,
    // ── Filter (which issues to change; mirrors list_issues) ──
    #[schemars(description = "Only affect issues with this status: backlog, todo, active, done, cancelled")]
    pub filter_status: Option<String>,
    #[schemars(description = "Only affect issues with this priority: urgent, high, medium, low, none")]
    pub filter_priority: Option<String>,
    #[schemars(description = "Only affect issues in this module (by name)")]
    pub filter_module: Option<String>,
    #[schemars(description = "Only affect issues carrying this label (by name)")]
    pub filter_label: Option<String>,
    // ── Target (fields to set on every matching issue) ──
    #[schemars(description = "New status to set: backlog, todo, active, done, cancelled")]
    pub set_status: Option<String>,
    #[schemars(description = "New priority to set: urgent, high, medium, low, none")]
    pub set_priority: Option<String>,
    #[schemars(description = "New module (by name) to set")]
    pub set_module: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct GetBoardInput {
    #[schemars(description = "Project identifier (e.g. LIF)")]
    pub project: String,
    #[schemars(description = "Group by: status, priority, or module (default: status)")]
    pub group_by: Option<String>,
    #[schemars(
        description = "Include done and cancelled issues (default false). By default closed columns appear as count-only stubs."
    )]
    pub include_closed: Option<bool>,
    #[schemars(
        description = "Cap issues rendered per column; a '… +N more (use list_issues)' tail shows the remainder."
    )]
    pub max_per_column: Option<i64>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct LinkIssuesInput {
    #[schemars(description = "Source issue identifier (e.g. PRO-1)")]
    pub source: String,
    #[schemars(description = "Target issue identifier (e.g. PRO-2)")]
    pub target: String,
    #[schemars(description = "Relation type: blocks, relates_to, or duplicate")]
    pub relation_type: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct UnlinkIssuesInput {
    #[schemars(description = "First issue identifier")]
    pub source: String,
    #[schemars(description = "Second issue identifier")]
    pub target: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct GetPageInput {
    #[schemars(description = "Page identifier like LIF-DOC-1")]
    pub identifier: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct CreatePageInput {
    #[schemars(description = "Project identifier (e.g. LIF). Omit for workspace-level page.")]
    pub project: Option<String>,
    #[schemars(description = "Page title")]
    pub title: String,
    #[schemars(description = "Markdown content")]
    pub content: Option<String>,
    #[schemars(description = "Folder name to place page in")]
    pub folder: Option<String>,
    #[schemars(description = "Status: draft, active, complete, archived")]
    pub status: Option<String>,
    #[schemars(
        description = "Label names to attach. Labels are project-scoped, so this is ignored on workspace pages (LIF-105)."
    )]
    pub labels: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct UpdatePageInput {
    #[schemars(description = "Page identifier like LIF-DOC-1")]
    pub identifier: String,
    #[schemars(description = "New title")]
    pub title: Option<String>,
    #[schemars(description = "New markdown content")]
    pub content: Option<String>,
    #[schemars(
        description = "Move to folder name. Omit to leave unchanged; pass an empty string \"\" to move the page back to the project root. (LIF-145)"
    )]
    pub folder: Option<String>,
    #[schemars(description = "Status: draft, active, complete, archived")]
    pub status: Option<String>,
    #[schemars(description = "Pin (true) or unpin (false) the page to the top of the page list.")]
    pub pinned: Option<bool>,
    #[schemars(
        description = "Replace labels. Pass [] to clear all. Labels are project-scoped (LIF-105)."
    )]
    pub labels: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct EditIssueInput {
    #[schemars(description = "Issue identifier like PRO-42")]
    pub identifier: String,
    #[schemars(description = "Exact string to find. Must be unique unless replace_all is true.")]
    pub old_string: String,
    #[schemars(description = "Replacement string (must differ from old_string)")]
    pub new_string: String,
    #[schemars(description = "Field to edit: 'description' (default) or 'title'")]
    pub field: Option<String>,
    #[schemars(description = "Replace all occurrences (default false)")]
    pub replace_all: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct EditPageInput {
    #[schemars(description = "Page identifier like LIF-DOC-1")]
    pub identifier: String,
    #[schemars(description = "Exact string to find. Must be unique unless replace_all is true.")]
    pub old_string: String,
    #[schemars(description = "Replacement string (must differ from old_string)")]
    pub new_string: String,
    #[schemars(description = "Field to edit: 'content' (default) or 'title'")]
    pub field: Option<String>,
    #[schemars(description = "Replace all occurrences (default false)")]
    pub replace_all: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
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

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ListResourcesInput {
    #[schemars(description = "Resource type: project, module, label, folder, page, or issue")]
    pub resource_type: String,
    #[schemars(description = "Project identifier (required for most types)")]
    pub project: Option<String>,
    #[schemars(description = "Folder name (for page filtering)")]
    pub folder: Option<String>,
    #[schemars(description = "Label name (for issue or page filtering — LIF-105)")]
    pub label: Option<String>,
    #[schemars(
        description = "Status filter (for page lists): draft, active, complete, or archived"
    )]
    pub status: Option<String>,
    #[schemars(
        description = "Sort column (for page lists): sort_order (default), title, status, created, or updated"
    )]
    pub order_by: Option<String>,
    #[schemars(description = "Sort direction (for page lists): asc (default) or desc")]
    pub order: Option<String>,
    #[schemars(description = "Max results (applies to issue/page lists; default 100 for issues)")]
    pub limit: Option<i64>,
    #[schemars(
        description = "Zero-indexed offset for paging (applies to issue/page lists). Output appends a hint when more results exist."
    )]
    pub offset: Option<i64>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
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
    #[schemars(
        description = "Icon for project or module: 'lucide:<Name>' or a literal emoji. Omit to leave unchanged; pass an empty string \"\" to clear it back to none. (LIF-145)"
    )]
    pub emoji: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct AddCommentInput {
    #[schemars(description = "Issue identifier (e.g. LIF-1)")]
    pub identifier: String,
    #[schemars(description = "Comment content (markdown)")]
    pub content: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ListCommentsInput {
    #[schemars(description = "Issue identifier (e.g. LIF-1)")]
    pub identifier: String,
    #[schemars(description = "Filter to comments by this author username")]
    pub author: Option<String>,
    #[schemars(
        description = "Sort direction by creation time: asc (default, oldest first) or desc (newest first)"
    )]
    pub order: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct EditCommentInput {
    #[schemars(
        description = "Comment id (the numeric handle returned by add_comment / shown by list_comments)"
    )]
    pub comment_id: i64,
    #[schemars(description = "New comment content (markdown)")]
    pub content: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct DeleteCommentInput {
    #[schemars(
        description = "Comment id (the numeric handle returned by add_comment / shown by list_comments)"
    )]
    pub comment_id: i64,
}

// ── Plans (LIF-168/169/170/171) ──────────────────────────────

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct PlanStepInput {
    #[schemars(description = "Step title — short and imperative")]
    pub title: String,
    #[schemars(description = "Optional longer notes/description for this step")]
    pub description: Option<String>,
    #[schemars(
        description = "Issue identifier this step mirrors (e.g. LIF-42). When set, the step auto-completes when the issue is closed, and marking the step done closes the issue."
    )]
    pub issue: Option<String>,
    #[schemars(description = "Pre-mark this step done (default false)")]
    pub done: Option<bool>,
    #[schemars(description = "Nested child steps (any depth)")]
    pub steps: Option<Vec<PlanStepInput>>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct CreatePlanInput {
    #[schemars(description = "Project identifier (e.g. LIF)")]
    pub project: String,
    #[schemars(description = "Plan title — what this plan accomplishes")]
    pub title: String,
    #[schemars(
        description = "Optional anchor issue this plan decomposes (e.g. LIF-42). Closing it auto-archives the plan."
    )]
    pub anchor_issue: Option<String>,
    #[schemars(
        description = "The full nested step tree, authored in one call. Each step: {title, description?, issue?, done?, steps?[]}."
    )]
    pub steps: Option<Vec<PlanStepInput>>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct GetPlanInput {
    #[schemars(description = "Plan identifier like LIF-PLAN-3")]
    pub plan: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct EditPlanStepInput {
    #[schemars(description = "Plan identifier like LIF-PLAN-3")]
    pub plan: String,
    #[schemars(description = "Numeric step id (the #N shown by get_plan)")]
    pub step_id: i64,
    #[schemars(description = "Exact string to find. Must be unique unless replace_all is true.")]
    pub old_string: String,
    #[schemars(description = "Replacement string")]
    pub new_string: String,
    #[schemars(description = "Field to edit: 'description' (default) or 'title'")]
    pub field: Option<String>,
    #[schemars(description = "Replace all occurrences (default false)")]
    pub replace_all: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct UpdatePlanStepInput {
    #[schemars(description = "Plan identifier like LIF-PLAN-3")]
    pub plan: String,
    #[schemars(
        description = "Numeric step id to operate on (the #N from get_plan). OMIT to operate on the plan itself (status/title/anchor)."
    )]
    pub step_id: Option<i64>,
    #[schemars(description = "New title for the targeted step or plan")]
    pub title: Option<String>,
    // ── Plan-level (step_id omitted) ──
    #[schemars(description = "Plan status: active, done, or archived (plan-level; omit step_id)")]
    pub status: Option<String>,
    #[schemars(description = "Set the plan's anchor issue (plan-level; omit step_id)")]
    pub anchor_issue: Option<String>,
    #[schemars(description = "Clear the plan's anchor issue (plan-level; omit step_id)")]
    pub clear_anchor: Option<bool>,
    // ── Step-level (step_id set) ──
    #[schemars(
        description = "Mark the step done/undone. Marking done also closes a linked issue; the result text reports the side effect."
    )]
    pub done: Option<bool>,
    #[schemars(description = "Attach an issue (e.g. LIF-42) to the step")]
    pub attach_issue: Option<String>,
    #[schemars(description = "Detach the step's issue reference")]
    pub detach_issue: Option<bool>,
    #[schemars(description = "Add a child step with this title under the targeted step")]
    pub add_child_title: Option<String>,
    #[schemars(description = "Description for the added child step")]
    pub add_child_description: Option<String>,
    #[schemars(description = "Issue identifier for the added child step")]
    pub add_child_issue: Option<String>,
    #[schemars(description = "Reparent the step under this step id")]
    pub move_parent_step_id: Option<i64>,
    #[schemars(description = "Reparent the step to the plan root")]
    pub move_to_root: Option<bool>,
    #[schemars(description = "New position among siblings (0-based)")]
    pub move_position: Option<i64>,
    #[schemars(description = "Delete the step and its whole subtree")]
    pub delete: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ExportIssueInput {
    #[schemars(description = "Issue identifier like PRO-42")]
    pub identifier: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ExportPageInput {
    #[schemars(description = "Page identifier like LIF-DOC-1")]
    pub identifier: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ExportProjectInput {
    #[schemars(description = "Project identifier (e.g. LIF)")]
    pub project: String,
}
