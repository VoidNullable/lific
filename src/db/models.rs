use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub identifier: String,
    pub description: String,
    pub emoji: Option<String>,
    pub lead_user_id: Option<i64>,
    /// LIF-233: sidebar ordering rank. Reindexed 0..N on every reorder; new
    /// projects append at the end. list_projects orders by this then name.
    pub sort_order: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// LIF-233: payload for `PUT /api/projects/reorder` — the full project id list
/// in the desired top-to-bottom order. The server reindexes `sort_order` to the
/// list position, sidestepping float-midpoint exhaustion and all-equal-rank
/// collisions.
#[derive(Debug, Deserialize)]
pub struct ReorderProjects {
    pub ids: Vec<i64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateProject {
    pub name: String,
    pub identifier: String,
    #[serde(default)]
    pub description: String,
    pub emoji: Option<String>,
    pub lead_user_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateProject {
    pub name: Option<String>,
    pub identifier: Option<String>,
    pub description: Option<String>,
    /// LIF-103: tristate so clients can explicitly clear the emoji back to NULL.
    /// None = field absent (don't change), Some(None) = set NULL, Some(Some(s)) = set string.
    #[serde(default, deserialize_with = "crate::db::models::deserialize_nullable")]
    pub emoji: Option<Option<String>>,
    /// LIF-103: tristate so clients can explicitly clear the lead back to NULL.
    /// None = field absent (don't change), Some(None) = set NULL, Some(Some(id)) = set id.
    #[serde(default, deserialize_with = "crate::db::models::deserialize_nullable")]
    pub lead_user_id: Option<Option<i64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub id: i64,
    pub project_id: i64,
    pub sequence: i64,
    /// Computed: "{project.identifier}-{sequence}"
    pub identifier: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub priority: String,
    pub module_id: Option<i64>,
    pub sort_order: f64,
    pub start_date: Option<String>,
    pub target_date: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Import provenance marker (LIF-264/265): stable per-external-issue string
    /// like `github:owner/name#12`. `None` for hand-created issues.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Labels attached to this issue (populated on read)
    #[serde(default)]
    pub labels: Vec<String>,
    /// Relations (populated on read for get_issue)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relates_to: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateIssue {
    pub project_id: i64,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default = "default_priority")]
    pub priority: String,
    pub module_id: Option<i64>,
    pub start_date: Option<String>,
    pub target_date: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    /// Import provenance marker (LIF-264/265). `None` for hand-created issues.
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateIssue {
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub module_id: Option<i64>,
    pub sort_order: Option<f64>,
    pub start_date: Option<String>,
    pub target_date: Option<String>,
    pub labels: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ListIssuesQuery {
    pub project_id: Option<i64>,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub module_id: Option<i64>,
    pub label: Option<String>,
    pub workable: Option<bool>,
    pub blocked: Option<bool>,
    /// Inclusive lower bound on `created_at` (ISO date or datetime).
    pub created_since: Option<String>,
    /// Exclusive upper bound on `created_at`.
    pub created_until: Option<String>,
    /// Inclusive lower bound on `updated_at`.
    pub updated_since: Option<String>,
    /// Exclusive upper bound on `updated_at`.
    pub updated_until: Option<String>,
    /// Sort column: sort_order (default), sequence, created, updated.
    /// Whitelisted in `list_issues` — never interpolated raw.
    pub order_by: Option<String>,
    /// Sort direction: asc (default) or desc.
    pub order: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// Per-status issue counts for a project (LIF-161). `total` is the sum of
/// all statuses so the UI never has to add them up (or worse, infer the
/// total from a length-capped list fetch).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct IssueStatusCounts {
    pub backlog: i64,
    pub todo: i64,
    pub active: i64,
    pub done: i64,
    pub cancelled: i64,
    pub total: i64,
}

fn default_status() -> String {
    "backlog".to_string()
}

fn default_priority() -> String {
    "none".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Module {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub description: String,
    pub status: String,
    /// Icon: "lucide:<Name>" or a literal emoji char. Mirrors Project.emoji.
    pub emoji: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateModule {
    pub project_id: i64,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_module_status")]
    pub status: String,
    pub emoji: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateModule {
    pub name: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    /// LIF-124: tristate so clients can clear the icon back to NULL.
    /// None = absent (don't change), Some(None) = NULL, Some(Some(s)) = set.
    #[serde(default, deserialize_with = "crate::db::models::deserialize_nullable")]
    pub emoji: Option<Option<String>>,
}

fn default_module_status() -> String {
    "active".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Label {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub color: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateLabel {
    pub project_id: i64,
    pub name: String,
    #[serde(default = "default_label_color")]
    pub color: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateLabel {
    pub name: Option<String>,
    pub color: Option<String>,
}

fn default_label_color() -> String {
    "#6B7280".to_string()
}

#[derive(Debug, Deserialize)]
pub struct UpdateFolder {
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page {
    pub id: i64,
    pub project_id: Option<i64>,
    pub sequence: Option<i64>,
    /// Computed: "{project.identifier}-DOC-{sequence}"
    pub identifier: String,
    pub folder_id: Option<i64>,
    pub title: String,
    pub content: String,
    pub sort_order: f64,
    /// LIF-112: lifecycle status — one of draft/active/complete/archived.
    pub status: String,
    /// LIF-183: user-pinned to the top of the page list.
    #[serde(default)]
    pub pinned: bool,
    pub created_at: String,
    pub updated_at: String,
    /// Labels attached to this page (populated on read). Empty for
    /// workspace-level pages — labels are project-scoped (LIF-105).
    #[serde(default)]
    pub labels: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreatePage {
    pub project_id: Option<i64>,
    pub folder_id: Option<i64>,
    pub title: String,
    #[serde(default)]
    pub content: String,
    /// LIF-112: lifecycle status. Defaults to "draft".
    #[serde(default = "default_page_status")]
    pub status: String,
    /// Label names to attach. Silently ignored for workspace pages (no
    /// project_id), since labels are project-scoped (LIF-105).
    #[serde(default)]
    pub labels: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdatePage {
    pub title: Option<String>,
    pub content: Option<String>,
    /// None = don't change, Some(None) = set to NULL, Some(Some(id)) = set to id
    #[serde(default, deserialize_with = "crate::db::models::deserialize_nullable")]
    pub folder_id: Option<Option<i64>>,
    pub sort_order: Option<f64>,
    /// LIF-112: lifecycle status. None = don't change.
    pub status: Option<String>,
    /// LIF-183: pin/unpin. None = don't change.
    pub pinned: Option<bool>,
    /// Replace the full label set. None = don't touch, Some(vec) = replace
    /// (delete-all + insert-by-name, mirroring `UpdateIssue`). Silently
    /// no-ops for workspace pages.
    pub labels: Option<Vec<String>>,
}

fn default_page_status() -> String {
    "draft".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Folder {
    pub id: i64,
    pub project_id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub sort_order: f64,
}

#[derive(Debug, Deserialize)]
pub struct CreateFolder {
    pub project_id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
}

// ── Project Members (LIF-195) ────────────────────────────────
//
// Per-project (user_id, role) pairs — the source of truth for project-scoped
// authorization (epic LIF-194). This is the data model only; no enforcement
// lives here or anywhere yet. `projects.lead_user_id` (migration 008) stays
// as the denormalized "primary lead" pointer; the query layer keeps both
// consistent on write (see db::queries::projects::create_project /
// update_project).

/// A project role, ordered by privilege: `Viewer < Maintainer < Lead`.
/// Variant declaration order drives the derived `Ord`, so don't reorder
/// these without checking `role_ordering_is_viewer_lt_maintainer_lt_lead`.
///
/// String form matches the DB's CHECK-constrained `role` column values
/// exactly ('viewer' / 'maintainer' / 'lead') via `FromSql`/`ToSql`, so
/// `row.get::<_, Role>(..)` and `params![.., role]` work directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Viewer,
    Maintainer,
    Lead,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Maintainer => "maintainer",
            Role::Lead => "lead",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Role {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "viewer" => Ok(Role::Viewer),
            "maintainer" => Ok(Role::Maintainer),
            "lead" => Ok(Role::Lead),
            other => Err(format!("invalid role: {other:?}")),
        }
    }
}

impl rusqlite::types::FromSql for Role {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        value.as_str()?.parse().map_err(|_| rusqlite::types::FromSqlError::InvalidType)
    }
}

impl rusqlite::types::ToSql for Role {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMember {
    pub project_id: i64,
    pub user_id: i64,
    pub role: Role,
    pub created_at: String,
}

/// LIF-199: a membership row joined with the target user's display
/// identity. Powers `GET /api/projects/{id}/members` — the web UI needs a
/// name to render, not just a bare `user_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberWithUser {
    pub project_id: i64,
    pub user_id: i64,
    pub role: Role,
    pub created_at: String,
    pub username: String,
    pub display_name: String,
}

/// `POST /api/projects/{id}/members` body. `role` defaults to `Viewer`
/// (design LIF-DOC-7: "default grant = viewer") when omitted.
///
/// `role` is a raw `String`, not [`Role`]: deserializing straight into the
/// enum would make axum's `Json<T>` extractor reject a bad value with 422
/// before the handler ever runs, but this API contracts for 400 on an
/// invalid role — so parsing (and the `BadRequest` it produces on failure)
/// happens explicitly in `db::queries::members::add_member`.
#[derive(Debug, Deserialize)]
pub struct AddMember {
    pub user_id: i64,
    pub role: Option<String>,
}

/// `PATCH /api/projects/{id}/members/{user_id}` body. See [`AddMember`]'s
/// doc comment for why `role` is a raw `String`.
#[derive(Debug, Deserialize)]
pub struct ChangeMemberRole {
    pub role: String,
}

// ── Users & Sessions ─────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub email: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub display_name: String,
    pub is_admin: bool,
    pub is_bot: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateUser {
    pub username: String,
    pub email: String,
    pub password: String,
    pub display_name: Option<String>,
    #[serde(default)]
    pub is_admin: bool,
    #[serde(default)]
    pub is_bot: bool,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    /// Accepts either username or email
    pub identity: String,
    pub password: String,
}

/// Lightweight user identity extracted from auth middleware.
/// Inserted into request extensions after token resolution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthUser {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub is_admin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub token: String,
    pub user_id: i64,
    pub expires_at: String,
    pub created_at: String,
}

// ── Bots (tool connections) ───────────────────────────────────

/// A bot (connected tool) with its owner info and key status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bot {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub owner_id: Option<i64>,
    pub created_at: String,
    /// Whether the bot has an active (non-revoked) API key.
    pub has_active_key: bool,
}

// ── API Key (user-facing) ────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserApiKey {
    pub id: i64,
    pub name: String,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub revoked: bool,
}

// ── Comments ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comment {
    pub id: i64,
    /// Set when the comment belongs to an issue. Mutually exclusive with `page_id`.
    pub issue_id: Option<i64>,
    /// Set when the comment belongs to a page. Mutually exclusive with `issue_id`.
    pub page_id: Option<i64>,
    pub user_id: i64,
    /// Author username (joined from users table on read)
    pub author: String,
    /// Author display name (joined from users table on read)
    pub author_display_name: String,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateComment {
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateComment {
    pub content: String,
}

/// LIF-263: a user who can be `@`-mentioned in a comment. Powers
/// `GET /api/projects/{id}/mention-candidates` — the autocomplete list the
/// composer fuzzy-filters client-side. Scoped to project members when
/// `authz_enforced` is on, all users otherwise (see
/// `db::queries::comments::mention_candidates`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MentionCandidate {
    pub user_id: i64,
    pub username: String,
    pub display_name: String,
}

// ── Search ───────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
pub struct SearchQuery {
    pub query: String,
    pub project_id: Option<i64>,
    /// Restrict to one entity type: "issue" or "page".
    pub result_type: Option<String>,
    /// Sort mode: "relevance" (default, BM25 rank) or "recent"
    /// (most recently updated first).
    pub sort: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub result_type: String,
    pub id: i64,
    pub identifier: Option<String>,
    pub title: String,
    pub snippet: String,
    pub project_id: Option<i64>,
}

// ── Audit log (LIF-155/156) ──────────────────────────────────

/// One audit-log entry, joined with the actor's user row at read time.
/// The LEFT JOIN means a deleted user degrades to None fields rather
/// than losing history.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Activity {
    pub id: i64,
    pub ts: String,
    pub actor_user_id: Option<i64>,
    pub actor_username: Option<String>,
    pub actor_display_name: Option<String>,
    pub actor_is_bot: bool,
    /// web | mcp | api | cli | system
    pub transport: String,
    pub entity_type: String,
    pub entity_id: i64,
    pub entity_label: Option<String>,
    pub project_id: Option<i64>,
    pub issue_id: Option<i64>,
    pub page_id: Option<i64>,
    /// create | update | delete | attach | detach | link | unlink
    pub action: String,
    pub field: Option<String>,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
}

/// A page of activity plus a "there's more" hint for clients.
#[derive(Debug, serde::Serialize)]
pub struct ActivityFeed {
    pub items: Vec<Activity>,
    pub has_more: bool,
}

/// Per-actor rollup for a project's audit history (LIF-158): powers the
/// actor rail on the Activity page and the "N actions in this project"
/// detail when an entry is expanded.
#[derive(Debug, serde::Serialize)]
pub struct ActorStat {
    pub actor_user_id: Option<i64>,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub is_bot: bool,
    /// Total audit entries by this actor in the project.
    pub actions: i64,
    /// Timestamp of their most recent action.
    pub last_ts: String,
    /// Most-used transport for this actor in this project.
    pub top_transport: String,
}

// ── Insights (LIF-240) ────────────────────────────────────────
//
// Per-project analytics tab: created/closed trend lines, current
// status/priority/module distributions, and a top-actors rollup scoped to
// the same window as the trend lines. Everything here is read-only,
// computed straight from `issues` + `audit_log` — no new tables.

/// One point on a created/closed trend line. `week_start` is the Monday
/// (ISO week start) the bucket covers, formatted `YYYY-MM-DD`. Buckets are
/// dense — every week in the requested range is present with `count: 0`
/// when there's no data, so the frontend never has to fill gaps itself.
#[derive(Debug, Clone, Serialize)]
pub struct WeekPoint {
    pub week_start: String,
    pub count: i64,
}

/// Current per-priority issue counts for a project. Mirrors
/// `IssueStatusCounts`'s shape (fixed fields + `total`) since priority, like
/// status, is a closed set the API validates on write.
#[derive(Debug, Default, Serialize)]
pub struct PriorityCounts {
    pub urgent: i64,
    pub high: i64,
    pub medium: i64,
    pub low: i64,
    pub none: i64,
    pub total: i64,
}

/// Current issue count for one module (or the `module_id: None` "no
/// module" bucket), ordered largest-first.
#[derive(Debug, Serialize)]
pub struct ModuleCount {
    pub module_id: Option<i64>,
    pub name: String,
    pub count: i64,
}

/// `GET /api/projects/{id}/insights` response — everything the Insights
/// tab needs in one round trip.
#[derive(Debug, Serialize)]
pub struct InsightsPayload {
    /// The (clamped) week count this payload was computed over — echoed
    /// back so the frontend's selector can confirm what it got.
    pub weeks: i64,
    pub created_per_week: Vec<WeekPoint>,
    /// See `queries::insights::get_insights` doc comment for the closure
    /// semantics: the most recent status-field transition per issue,
    /// counted only when it landed on done/cancelled — so a reopened issue
    /// isn't double-counted and a closed-then-reopened issue drops out.
    pub closed_per_week: Vec<WeekPoint>,
    pub status_counts: IssueStatusCounts,
    pub priority_counts: PriorityCounts,
    pub module_counts: Vec<ModuleCount>,
    /// Actor rollup scoped to the same `weeks` window as the trend lines
    /// (unlike `ActorStat`'s all-time project rollup on the Activity tab).
    pub top_actors: Vec<ActorStat>,
}

// ── Plans (LIF-165/166) ──────────────────────────────────────
//
// A plan is a project-level tree of steps that survives across sessions.
// Issues stay flat; the hierarchy lives here. A step optionally mirrors a
// flat issue (plan_steps.issue_id). Storage is an adjacency list; the nested
// `steps` tree is assembled in the query layer.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub id: i64,
    pub project_id: i64,
    pub sequence: i64,
    /// Computed: "{project.identifier}-PLAN-{sequence}"
    pub identifier: String,
    /// Anchor issue: the issue this plan decomposes (optional).
    pub issue_id: Option<i64>,
    /// Computed identifier of the anchor issue, when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor_identifier: Option<String>,
    pub title: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    /// Nested step tree (populated on read for get_plan). Empty in list views.
    #[serde(default)]
    pub steps: Vec<PlanStepNode>,
    /// Step counts (populated for list views and headers).
    #[serde(default)]
    pub step_count: i64,
    #[serde(default)]
    pub done_count: i64,
}

/// A node in a plan's step tree. `children` makes the adjacency-list rows
/// nested for rendering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStepNode {
    pub id: i64,
    pub plan_id: i64,
    pub parent_step_id: Option<i64>,
    pub position: i64,
    pub title: String,
    pub description: String,
    pub issue_id: Option<i64>,
    /// Computed identifier of the referenced issue, when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue_identifier: Option<String>,
    /// Current status of the referenced issue (so renderers can show
    /// "done (via LIF-42)" provenance). None when no issue is linked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue_status: Option<String>,
    pub done: bool,
    /// Set when an issue reopen auto-unchecked this step (LIF-167).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reopened_via_issue_at: Option<String>,
    pub created_at: String,
    pub edited_at: Option<String>,
    #[serde(default)]
    pub children: Vec<PlanStepNode>,
}

/// Create a plan, optionally anchored to an issue, with a full nested step
/// tree authored in one call. Issue references are pre-resolved to ids by the
/// MCP/REST layer.
#[derive(Debug, Deserialize)]
pub struct CreatePlan {
    pub project_id: i64,
    pub title: String,
    pub issue_id: Option<i64>,
    #[serde(default)]
    pub steps: Vec<CreatePlanStep>,
}

/// A step in a create_plan tree. Recursive via `steps`.
#[derive(Debug, Deserialize)]
pub struct CreatePlanStep {
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub issue_id: Option<i64>,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub steps: Vec<CreatePlanStep>,
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdatePlan {
    pub title: Option<String>,
    pub status: Option<String>,
    /// Tristate anchor issue: None = don't change, Some(None) = clear,
    /// Some(Some(id)) = set.
    #[serde(default, deserialize_with = "crate::db::models::deserialize_nullable")]
    pub issue_id: Option<Option<i64>>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ListPlansQuery {
    pub project_id: Option<i64>,
    pub status: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

// ── Saved views (LIF-242) ────────────────────────────────────
//
// Named filter/group/sort presets per project, personal to each user (no
// team-shared views — see api::views doc comment). `config` is an opaque
// JSON string as far as the backend is concerned: validated for size and
// well-formedness only (db::queries::views::validate_config), never
// schema-validated. The frontend's `ViewConfig` (web/src/lib/issues/views.ts)
// owns the actual shape.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedView {
    pub id: i64,
    pub project_id: i64,
    pub user_id: i64,
    pub name: String,
    pub config: String,
    pub is_default: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateSavedView {
    pub name: String,
    pub config: String,
    #[serde(default)]
    pub is_default: bool,
}

/// `PATCH /api/projects/{id}/views/{view_id}` body. All fields optional —
/// only provided ones change. Renaming, updating the config, and (un)setting
/// the default can all be done independently or together in one call.
#[derive(Debug, Default, Deserialize)]
pub struct UpdateSavedView {
    pub name: Option<String>,
    pub config: Option<String>,
    pub is_default: Option<bool>,
}

// ── Attachments (LIF-262) ────────────────────────────────────
//
// Image + file uploads on issues, comments, and pages. Bytes live on disk at
// `<data_dir>/attachments/<sha256>` (content-addressed sidecar — see
// migration 031 and src/storage.rs); this row is metadata only. The
// `attachment_links` join (many-to-many) records which entities reference an
// attachment so the orphan GC knows when a sidecar file is collectable.

/// One uploaded file's metadata. Serialized straight to the upload/list
/// responses; `sha256` is intentionally NOT serialized (it's an internal
/// storage key, and the public handle is the numeric `id` + `/api/attachments`
/// URL).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id: i64,
    #[serde(skip_serializing)]
    pub sha256: String,
    pub filename: String,
    pub mime: String,
    pub size_bytes: i64,
    pub uploader_id: Option<i64>,
    pub created_at: String,
}

/// The kind of entity an attachment is linked to. Mirrors the
/// `attachment_links.entity_type` CHECK values exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttachmentEntity {
    Issue,
    Page,
    Comment,
}

impl AttachmentEntity {
    pub fn as_str(self) -> &'static str {
        match self {
            AttachmentEntity::Issue => "issue",
            AttachmentEntity::Page => "page",
            AttachmentEntity::Comment => "comment",
        }
    }
}

impl std::str::FromStr for AttachmentEntity {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "issue" => Ok(AttachmentEntity::Issue),
            "page" => Ok(AttachmentEntity::Page),
            "comment" => Ok(AttachmentEntity::Comment),
            other => Err(format!("invalid attachment entity: {other:?}")),
        }
    }
}

/// Deserializes a JSON field as Option<Option<T>>:
/// - absent key → None (don't change)
/// - "field": null → Some(None) (set to null)
/// - "field": value → Some(Some(value))
pub fn deserialize_nullable<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: serde::Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::deserialize(deserializer)?))
}
