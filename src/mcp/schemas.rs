use rmcp::schemars;
use schemars::JsonSchema;
use serde::Deserialize;

// LIF-474: every input denies unknown fields, nested ones included, so a
// misspelled optional parameter fails instead of being silently dropped.
// `crate::mcp::arguments` rewrites the error to suggest the intended name.

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchInput {
    #[schemars(description = "Text to find in issues, pages, comments, and attachments")]
    pub query: String,
    #[schemars(description = "Only this project")]
    pub project: Option<String>,
    #[schemars(description = "Only issue, page, comment, or attachment")]
    pub result_type: Option<String>,
    #[schemars(description = "relevance (default) or recent")]
    pub sort: Option<String>,
    #[schemars(
        description = "fts (default, word prefixes) or literal (substring, for punctuation)"
    )]
    pub mode: Option<String>,
    #[schemars(description = "Max results (default 20)")]
    pub limit: Option<i64>,
    #[schemars(description = "Offset for paging")]
    pub offset: Option<i64>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListIssuesInput {
    #[schemars(description = "Project ID, e.g. LIF. Optional in a repo-bound session")]
    pub project: Option<String>,
    #[schemars(description = "backlog, todo, active, done, or cancelled")]
    pub status: Option<String>,
    #[schemars(description = "urgent, high, medium, low, or none")]
    pub priority: Option<String>,
    #[schemars(description = "Module name")]
    pub module: Option<String>,
    #[schemars(description = "Label name")]
    pub label: Option<String>,
    #[schemars(description = "Only open issues with no open blocker or holding wait")]
    pub workable: Option<bool>,
    #[schemars(description = "Only issues with an open blocker or holding wait")]
    pub blocked: Option<bool>,
    #[schemars(description = "Created at or after this ISO date or datetime")]
    pub created_since: Option<String>,
    #[schemars(description = "Created before this (exclusive)")]
    pub created_until: Option<String>,
    #[schemars(description = "Updated at or after this")]
    pub updated_since: Option<String>,
    #[schemars(description = "Updated before this (exclusive)")]
    pub updated_until: Option<String>,
    #[schemars(description = "sort_order (default), sequence, created, updated, or priority")]
    pub order_by: Option<String>,
    #[schemars(description = "asc (default) or desc")]
    pub order: Option<String>,
    #[schemars(description = "Max results (default 50)")]
    pub limit: Option<i64>,
    #[schemars(description = "Offset for paging")]
    pub offset: Option<i64>,
    #[schemars(
        description = "Issues across the projects where these users have a role (\"me\" is you); project becomes optional. Active and todo unless status/statuses says, sorted by status, priority, recency"
    )]
    pub members: Option<Vec<String>>,
    #[schemars(
        description = "Roles that count for members: lead, maintainer, viewer, or all (default). Alone: your roles"
    )]
    pub roles: Option<Vec<String>>,
    #[schemars(description = "Several statuses, or all. Not with status")]
    pub statuses: Option<Vec<String>>,
    #[schemars(description = "none (free for agents), human (needs a person), me, or a username")]
    pub assignee: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetIssueInput {
    #[schemars(description = "Issue ID, e.g. PRO-42")]
    pub identifier: String,
    #[schemars(description = "recent (default, last 3), all (newest 500), or none (count only)")]
    pub include_comments: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetActivityInput {
    #[schemars(description = "Issue (PRO-42), page (PRO-DOC-3), or project (PRO)")]
    pub identifier: String,
    #[schemars(
        description = "Only entries after this ISO date or datetime (UTC unless offset given), oldest first"
    )]
    pub since: Option<String>,
    #[schemars(description = "Max entries (default 30, cap 200)")]
    pub limit: Option<i64>,
    #[schemars(description = "Offset for paging")]
    pub offset: Option<i64>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetBriefingInput {
    #[schemars(description = "Project ID, e.g. LIF. Optional in a repo-bound session")]
    pub project: Option<String>,
    #[schemars(description = "Also summarize changes after this ISO date or datetime")]
    pub since: Option<String>,
    #[schemars(description = "Page IDs to report on; default recently updated")]
    pub pages: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateIssueInput {
    #[schemars(description = "Project ID, e.g. LIF. Optional in a repo-bound session")]
    pub project: Option<String>,
    // LIF-478: optional so a batch can omit it; the tool requires it
    // whenever `issues` is absent.
    #[serde(default)]
    #[schemars(description = "Issue title (required unless issues is given)")]
    pub title: String,
    #[schemars(description = "Markdown description")]
    pub description: Option<String>,
    #[schemars(description = "backlog (default), todo, active, done, or cancelled")]
    pub status: Option<String>,
    #[schemars(description = "urgent, high, medium, low, or none (default)")]
    pub priority: Option<String>,
    #[schemars(description = "Module name")]
    pub module: Option<String>,
    #[schemars(description = "Label names")]
    pub labels: Option<Vec<String>>,
    #[schemars(description = "Start date, YYYY-MM-DD")]
    pub start_date: Option<String>,
    #[schemars(description = "Due date, YYYY-MM-DD")]
    pub target_date: Option<String>,
    #[schemars(
        description = "Who must do it: [\"human\"] for any person, or usernames (\"me\" is you). Omit to leave it to agents"
    )]
    pub assignees: Option<Vec<String>>,
    #[schemars(
        description = "Up to 50 issues to create in one transaction, each with the fields above. Only project may accompany it",
        schema_with = "create_issue_items_schema"
    )]
    // Without a serde default, the custom schema made schemars list
    // `issues` as required, so strict clients sent a batch on every call.
    #[serde(default)]
    pub issues: Option<Vec<CreateIssueItem>>,
}

/// The published shape of `CreateIssueInput::issues`: objects that need a
/// title. The derived item schema would repeat all eight fields in `$defs`
/// (about 100 tokens on every `tools/list`) to say what the description
/// already does. Deserialization is still strict: `CreateIssueItem` denies
/// unknown keys.
fn create_issue_items_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": ["array", "null"],
        "items": { "type": "object", "required": ["title"] }
    })
}

/// One issue in a `create_issue` batch (LIF-478). Field meanings match
/// [`CreateIssueInput`].
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateIssueItem {
    pub title: String,
    pub description: Option<String>,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub module: Option<String>,
    pub labels: Option<Vec<String>>,
    pub start_date: Option<String>,
    pub target_date: Option<String>,
    pub assignees: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateIssueInput {
    #[schemars(description = "Issue ID like PRO-42")]
    pub identifier: String,
    #[schemars(description = "New title")]
    pub title: Option<String>,
    #[schemars(description = "New description (markdown)")]
    pub description: Option<String>,
    #[schemars(description = "backlog, todo, active, done, or cancelled")]
    pub status: Option<String>,
    #[schemars(description = "urgent, high, medium, low, or none")]
    pub priority: Option<String>,
    #[schemars(description = "Module name; \"\" clears it")]
    pub module: Option<String>,
    #[schemars(description = "Replace labels")]
    pub labels: Option<Vec<String>>,
    #[schemars(description = "Start date, YYYY-MM-DD")]
    pub start_date: Option<String>,
    #[schemars(description = "Due date, YYYY-MM-DD")]
    pub target_date: Option<String>,
    #[schemars(
        description = "The seq you last read; the update is refused if the issue changed since"
    )]
    pub expected_seq: Option<i64>,
    #[schemars(
        description = "Replace who must do it: [] frees it for agents, [\"human\"] any person, or usernames (\"me\" is you)"
    )]
    pub assignees: Option<Vec<String>>,
    #[schemars(
        description = "How you verified the work (markdown), only with status=done. Saved as a verification comment"
    )]
    pub evidence: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BulkUpdateInput {
    #[schemars(description = "Project ID (e.g. LIF)")]
    pub project: String,
    // ── Filter (which issues to change; mirrors list_issues) ──
    #[schemars(description = "Only issues with this status")]
    pub filter_status: Option<String>,
    #[schemars(description = "Only issues with this priority")]
    pub filter_priority: Option<String>,
    #[schemars(description = "Only issues in this module")]
    pub filter_module: Option<String>,
    #[schemars(description = "Only issues with this label")]
    pub filter_label: Option<String>,
    // ── Target (fields to set on every matching issue) ──
    #[schemars(description = "Status to set")]
    pub set_status: Option<String>,
    #[schemars(description = "Priority to set")]
    pub set_priority: Option<String>,
    #[schemars(description = "Module to set")]
    pub set_module: Option<String>,
    #[schemars(description = "Assignment to set, as in update_issue")]
    pub set_assignees: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetBoardInput {
    #[schemars(description = "Project ID, e.g. LIF. Optional in a repo-bound session")]
    pub project: Option<String>,
    #[schemars(description = "status (default), priority, or module")]
    pub group_by: Option<String>,
    #[schemars(description = "Show done and cancelled issues (default false)")]
    pub include_closed: Option<bool>,
    #[schemars(description = "Max issues per column")]
    pub max_per_column: Option<i64>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkIssuesInput {
    #[schemars(description = "Source issue ID (e.g. PRO-1); omit for a wait")]
    #[serde(default)]
    pub source: String,
    #[schemars(description = "Target issue ID (e.g. PRO-2)")]
    pub target: String,
    #[schemars(description = "Relation type: blocks, relates_to, or duplicate")]
    pub relation_type: String,
    #[schemars(description = "Wait: username target waits on until cleared")]
    pub user: Option<String>,
    #[schemars(description = "Wait: day (YYYY-MM-DD) target stops being blocked")]
    pub from: Option<String>,
    #[schemars(description = "Wait: last expected day; overdue after it")]
    pub until: Option<String>,
    #[schemars(description = "Wait note")]
    pub note: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnlinkIssuesInput {
    #[schemars(description = "First issue ID; omit to clear a wait")]
    #[serde(default)]
    pub source: String,
    #[schemars(description = "Second issue ID")]
    pub target: String,
    #[schemars(description = "Clear target's wait on this user")]
    pub user: Option<String>,
    #[schemars(description = "Clear target's date wait starting this day")]
    pub from: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetPageInput {
    #[schemars(description = "Page ID like LIF-DOC-1")]
    pub identifier: String,
    #[schemars(description = "Read one section by heading text or anchor, with its subsections")]
    pub section: Option<String>,
    #[schemars(description = "Return only headings, section sizes, and seq")]
    pub outline: Option<bool>,
    #[schemars(description = "Return only the diff since this earlier seq")]
    pub since_seq: Option<i64>,
    #[schemars(description = "Continue a cut read from this character offset")]
    pub offset: Option<usize>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreatePageInput {
    #[schemars(description = "Project ID; omit for a workspace page")]
    pub project: Option<String>,
    #[schemars(description = "Page title")]
    pub title: String,
    #[schemars(description = "Markdown content")]
    pub content: Option<String>,
    #[schemars(description = "Folder name")]
    pub folder: Option<String>,
    #[schemars(description = "Status: draft, active, complete, archived")]
    pub status: Option<String>,
    #[schemars(description = "Label names (project pages only)")]
    pub labels: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdatePageInput {
    #[schemars(description = "Page ID like LIF-DOC-1")]
    pub identifier: String,
    #[schemars(description = "New title")]
    pub title: Option<String>,
    #[schemars(description = "New markdown content")]
    pub content: Option<String>,
    #[schemars(description = "Folder name; \"\" moves to the project root")]
    pub folder: Option<String>,
    #[schemars(description = "Status: draft, active, complete, archived")]
    pub status: Option<String>,
    #[schemars(description = "Pin to the top of the page list")]
    pub pinned: Option<bool>,
    #[schemars(description = "Replace labels; [] clears")]
    pub labels: Option<Vec<String>>,
    #[schemars(
        description = "The seq you last read; the update is refused if the page changed since"
    )]
    pub expected_seq: Option<i64>,
}

/// LIF-472: the four edit tools also accept `oldString`, `newString` and
/// `replaceAll`, the spelling agents trained on camelCase edit tools send.
/// Serde aliases only: the advertised schema stays snake_case.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditIssueInput {
    #[schemars(description = "Issue ID like PRO-42")]
    pub identifier: String,
    #[schemars(description = "Exact text to find; must match once unless replace_all")]
    #[serde(alias = "oldString")]
    pub old_string: String,
    #[schemars(description = "Replacement; must differ")]
    #[serde(alias = "newString")]
    pub new_string: String,
    #[schemars(description = "Field to edit: 'description' (default) or 'title'")]
    pub field: Option<String>,
    #[schemars(description = "Replace every match (default false)")]
    #[serde(alias = "replaceAll")]
    pub replace_all: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditPageInput {
    #[schemars(description = "Page ID like LIF-DOC-1")]
    pub identifier: String,
    #[schemars(description = "Exact text to find; must match once unless replace_all")]
    #[serde(alias = "oldString")]
    pub old_string: String,
    #[schemars(description = "Replacement; must differ")]
    #[serde(alias = "newString")]
    pub new_string: String,
    #[schemars(description = "Field to edit: 'content' (default) or 'title'")]
    pub field: Option<String>,
    #[schemars(description = "Replace every match (default false)")]
    #[serde(alias = "replaceAll")]
    pub replace_all: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteInput {
    #[schemars(description = "issue, page, plan, project, module, label, or folder")]
    pub resource_type: String,
    #[schemars(description = "ID (LIF-1, LIF-DOC-1, LIF) or module, label, or folder name")]
    pub identifier: String,
    #[schemars(description = "Project ID, for a module, label, or folder")]
    pub project: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListResourcesInput {
    #[schemars(description = "project, module, label, folder, page, issue, or plan")]
    pub resource_type: String,
    #[schemars(description = "Project ID; needed for most types unless the session is repo-bound")]
    pub project: Option<String>,
    #[schemars(description = "Folder name (pages)")]
    pub folder: Option<String>,
    #[schemars(description = "Label name (issues, pages)")]
    pub label: Option<String>,
    #[schemars(description = "Page or plan status")]
    pub status: Option<String>,
    #[schemars(description = "Pages: sort_order (default), title, status, created, or updated")]
    pub order_by: Option<String>,
    #[schemars(description = "Pages: asc (default) or desc")]
    pub order: Option<String>,
    #[schemars(description = "Max results (issues and pages 100, plans 50, cap 500)")]
    pub limit: Option<i64>,
    #[schemars(description = "Offset for issues, pages, or plans")]
    pub offset: Option<i64>,
    #[schemars(
        description = "Projects only: keep projects where these users have a role (\"me\" is you)"
    )]
    pub members: Option<Vec<String>>,
    #[schemars(description = "Projects only: roles that count for members (default all)")]
    pub roles: Option<Vec<String>>,
    #[schemars(description = "Projects only: statuses to count (default all); [] prints none")]
    pub statuses: Option<Vec<String>>,
    #[schemars(
        description = "Projects only: roles to name every member of (lead, maintainer, viewer, all); [] prints counts only"
    )]
    pub show_members: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManageResourceInput {
    #[schemars(description = "project, module, label, or folder")]
    pub resource_type: String,
    #[schemars(description = "create or update")]
    pub action: String,
    #[schemars(description = "Current name, to update a module, label, or folder")]
    pub current_name: Option<String>,
    #[schemars(description = "Project ID, for modules, labels, folders, and project updates")]
    pub project: Option<String>,
    #[schemars(description = "Name; required to create")]
    pub name: Option<String>,
    #[schemars(description = "Project identifier, e.g. PRO")]
    pub identifier: Option<String>,
    #[schemars(description = "Description")]
    pub description: Option<String>,
    #[schemars(description = "Module status: backlog, planned, active, paused, done, cancelled")]
    pub status: Option<String>,
    #[schemars(description = "Label color, e.g. #EF4444")]
    pub color: Option<String>,
    #[schemars(description = "Project or module icon: 'lucide:<Name>' or an emoji; \"\" clears")]
    pub emoji: Option<String>,
}

impl ManageResourceInput {
    /// Normalize quote wrappers on scalar arguments at the tool boundary.
    /// Keep descriptions verbatim and current_name intact for exact-name lookup.
    #[must_use]
    pub(super) fn normalize_quotes(self) -> Self {
        use super::arguments::unquote;

        Self {
            resource_type: unquote(self.resource_type),
            action: unquote(self.action),
            project: self.project.map(unquote),
            name: self.name.map(unquote),
            identifier: self.identifier.map(unquote),
            status: self.status.map(unquote),
            color: self.color.map(unquote),
            emoji: self.emoji.map(unquote),
            ..self
        }
    }
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddCommentInput {
    #[schemars(description = "Issue (LIF-1), page (LIF-DOC-1), or workspace page (DOC-1) ID")]
    pub identifier: String,
    #[schemars(description = "Comment content (markdown)")]
    pub content: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListCommentsInput {
    #[schemars(description = "Issue (LIF-1), page (LIF-DOC-1), or workspace page (DOC-1) ID")]
    pub identifier: String,
    #[schemars(description = "Only this author's comments")]
    pub author: Option<String>,
    #[schemars(description = "desc (default, newest first) or asc")]
    pub order: Option<String>,
    #[schemars(description = "Max comments (default 50, cap 500)")]
    pub limit: Option<i64>,
    #[schemars(description = "Offset for paging")]
    pub offset: Option<i64>,
}

/// Two modes, exactly one per call: `content` replaces the whole body, or
/// `old_string` + `new_string` does the exact-string replacement that
/// `edit_issue` and `edit_page` do. Unknown fields are rejected so a caller
/// that guesses the contract wrong gets an error instead of a silently
/// rewritten comment (GitHub #64).
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditCommentInput {
    #[schemars(description = "Comment id")]
    pub comment_id: i64,
    #[schemars(description = "New body, replacing all of it. Not with old_string")]
    pub content: Option<String>,
    #[schemars(description = "Exact text to find; must match once unless replace_all")]
    #[serde(alias = "oldString")]
    pub old_string: Option<String>,
    #[schemars(description = "Replacement; must differ")]
    #[serde(alias = "newString")]
    pub new_string: Option<String>,
    #[schemars(description = "Replace every match (default false)")]
    #[serde(alias = "replaceAll")]
    pub replace_all: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteCommentInput {
    #[schemars(description = "Comment id")]
    pub comment_id: i64,
}

// ── Plans (LIF-168/169/170/171) ──────────────────────────────

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanStepInput {
    #[schemars(description = "Step title, short and imperative")]
    pub title: String,
    #[schemars(description = "Optional longer notes/description for this step")]
    pub description: Option<String>,
    #[schemars(description = "Issue ID this step mirrors (e.g. LIF-42)")]
    pub issue: Option<String>,
    #[schemars(description = "Pre-mark this step done (default false)")]
    pub done: Option<bool>,
    #[schemars(description = "Nested child steps (any depth)")]
    pub steps: Option<Vec<PlanStepInput>>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreatePlanInput {
    #[schemars(description = "Project ID, e.g. LIF. Optional in a repo-bound session")]
    pub project: Option<String>,
    #[schemars(description = "Plan title")]
    pub title: String,
    #[schemars(description = "Anchor issue; closing it archives the plan")]
    pub anchor_issue: Option<String>,
    #[schemars(
        description = "Full nested step tree. Each step: {title, description?, issue?, done?, steps?[]}"
    )]
    pub steps: Option<Vec<PlanStepInput>>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetPlanInput {
    #[schemars(description = "Plan ID like LIF-PLAN-3")]
    pub plan: String,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditPlanStepInput {
    #[schemars(description = "Plan ID like LIF-PLAN-3")]
    pub plan: String,
    #[schemars(
        description = "Step ID from get_plan, not its position. For #18, pass integer step_id: 18 (no # or quotes)"
    )]
    pub step_id: i64,
    #[schemars(description = "Exact text to find; must match once unless replace_all")]
    #[serde(alias = "oldString")]
    pub old_string: String,
    #[schemars(description = "Replacement string")]
    #[serde(alias = "newString")]
    pub new_string: String,
    #[schemars(description = "Field to edit: 'description' (default) or 'title'")]
    pub field: Option<String>,
    #[schemars(description = "Replace every match (default false)")]
    #[serde(alias = "replaceAll")]
    pub replace_all: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdatePlanStepInput {
    #[schemars(description = "Plan ID like LIF-PLAN-3")]
    pub plan: String,
    #[schemars(
        description = "Step ID from get_plan, not its position. For #18, pass integer step_id: 18 (no # or quotes). Omit to change the plan"
    )]
    pub step_id: Option<i64>,
    #[schemars(description = "New title for the target")]
    pub title: Option<String>,
    // ── Plan-level (step_id omitted) ──
    #[schemars(description = "Plan status: active, done, or archived")]
    pub status: Option<String>,
    #[schemars(description = "Set the plan's anchor issue")]
    pub anchor_issue: Option<String>,
    #[schemars(description = "Clear the plan's anchor issue")]
    pub clear_anchor: Option<bool>,
    // ── Step-level (step_id set) ──
    #[schemars(description = "Mark the step done or not; a linked issue follows")]
    pub done: Option<bool>,
    #[schemars(description = "Attach an issue (e.g. LIF-42) to the step")]
    pub attach_issue: Option<String>,
    #[schemars(description = "Detach the step's issue")]
    pub detach_issue: Option<bool>,
    #[schemars(
        description = "Add a step with this title: under step_id, or top-level when step_id is omitted"
    )]
    pub add_child_title: Option<String>,
    #[schemars(description = "Description for the new step")]
    pub add_child_description: Option<String>,
    #[schemars(description = "Issue the new step mirrors")]
    pub add_child_issue: Option<String>,
    #[schemars(description = "Move the step under this step")]
    pub move_parent_step_id: Option<i64>,
    #[schemars(description = "Move the step to the top level")]
    pub move_to_root: Option<bool>,
    #[schemars(description = "New position among siblings, from 0")]
    pub move_position: Option<i64>,
    #[schemars(description = "Delete the step and its children")]
    pub delete: Option<bool>,
    #[schemars(description = "Return the whole tree, not a receipt")]
    pub echo_tree: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UploadAttachmentInput {
    #[schemars(description = "File name, e.g. crash-log.txt or screenshot.png")]
    pub filename: String,
    #[schemars(description = "File bytes, base64 (max 10 MiB decoded)")]
    pub content_base64: String,
    #[schemars(description = "Link to this issue or page")]
    pub entity: Option<String>,
    #[schemars(description = "Link to this comment instead")]
    pub comment_id: Option<i64>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetAttachmentInput {
    #[schemars(description = "Attachment id, e.g. 12 for /api/attachments/12")]
    pub attachment_id: i64,
    #[schemars(description = "Text only: zero-indexed first line to return")]
    pub offset: Option<i64>,
    #[schemars(description = "Text only: max lines to return (default 200, cap 500)")]
    pub limit: Option<i64>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListAttachmentsInput {
    #[schemars(description = "Issue or page ID")]
    pub entity: Option<String>,
    #[schemars(description = "Every attachment in this project instead")]
    pub project: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportInput {
    #[schemars(description = "Issue (PRO-42), page (PRO-DOC-3), or project (PRO)")]
    pub identifier: String,
    #[schemars(description = "Project only: zero-indexed document offset")]
    pub offset: Option<i64>,
    #[schemars(description = "Project only: max documents (default 20, cap 100)")]
    pub limit: Option<i64>,
}
