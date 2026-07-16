use std::{cmp::Ordering, sync::Arc};

use chrono::{DateTime, NaiveDateTime, Utc};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};

use crate::db::{DbPool, models, queries};

use super::LificMcp;
use super::schemas::*;

/// Self-onboarding nudge (LIF-257): shown by cold read tools when the DB has
/// **zero** projects, so the first agent connecting to a fresh install learns
/// the bootstrap sequence instead of staring at an empty list. Kept in one
/// place so all call sites and tests share the exact text. Precedent:
/// backlog.md's "init-required" signal.
pub(crate) const NO_PROJECTS_NUDGE: &str = "No projects exist yet. Create one first: manage_resource(resource_type='project', action='create', name='My Project', identifier='PRO'). Then create issues with create_issue(project='PRO', ...).";

impl LificMcp {
    pub(crate) fn create_tool_router() -> rmcp::handler::server::router::tool::ToolRouter<Self> {
        Self::tool_router()
    }
}

pub(crate) fn fmt_issue(i: &models::Issue) -> String {
    let mut s = format!(
        "{} | {} | {} | {}",
        i.identifier, i.status, i.priority, i.title
    );
    if !i.labels.is_empty() {
        s.push_str(&format!(" [{}]", i.labels.join(", ")));
    }
    if !i.blocks.is_empty() {
        s.push_str(&format!(" blocks:{}", i.blocks.join(",")));
    }
    if !i.blocked_by.is_empty() {
        s.push_str(&format!(" blocked_by:{}", i.blocked_by.join(",")));
    }
    if !i.duplicates.is_empty() {
        s.push_str(&format!(" duplicates:{}", i.duplicates.join(",")));
    }
    if !i.duplicated_by.is_empty() {
        s.push_str(&format!(" duplicated_by:{}", i.duplicated_by.join(",")));
    }
    s
}

/// Render a plan + its step tree compactly for get_plan / create_plan output.
/// Step lines carry `#<id>` so the agent can target edits, and issue-linked
/// steps show provenance ("via LIF-42" / "reopened — LIF-42 reopened").
pub(crate) fn fmt_plan(p: &models::Plan) -> String {
    let anchor = p
        .anchor_identifier
        .as_ref()
        .map(|a| format!(" — anchor {a}"))
        .unwrap_or_default();
    let mut out = format!(
        "{} [{}] {}{} — {}/{} done\n",
        p.identifier, p.status, p.title, anchor, p.done_count, p.step_count
    );
    fmt_steps(&p.steps, 0, &mut out);
    out
}

fn relative_age(timestamp: &str, now: DateTime<Utc>) -> Option<String> {
    let activity_at = match NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M:%S") {
        Ok(timestamp) => timestamp.and_utc(),
        Err(_) => DateTime::parse_from_rfc3339(timestamp)
            .ok()?
            .with_timezone(&Utc),
    };
    let minutes = now.signed_duration_since(activity_at).num_minutes().max(0);
    Some(if minutes < 60 {
        format!("{minutes}m ago")
    } else if minutes < 24 * 60 {
        format!("{}h ago", minutes / 60)
    } else {
        format!("{}d ago", minutes / (24 * 60))
    })
}

fn fmt_project_agent_stats(
    stats: &queries::ProjectAgentStats,
    now: DateTime<Utc>,
) -> Option<String> {
    let mut parts = Vec::new();
    if stats.workable > 0 {
        parts.push(format!("{} workable", stats.workable));
    }
    if stats.active_plans > 0 {
        let noun = if stats.active_plans == 1 {
            "active plan"
        } else {
            "active plans"
        };
        parts.push(format!("{} {noun}", stats.active_plans));
    }
    if let Some(last_activity) = stats.last_activity.as_deref()
        && let Some(age) = relative_age(last_activity, now)
    {
        parts.push(format!("last activity {age}"));
    }
    (!parts.is_empty()).then(|| format!(" ({})", parts.join(", ")))
}

fn cmp_projects_by_activity(
    left: &models::Project,
    right: &models::Project,
    stats: &std::collections::HashMap<i64, queries::ProjectAgentStats>,
) -> Ordering {
    let left_activity = stats
        .get(&left.id)
        .and_then(|stats| stats.last_activity.as_deref());
    let right_activity = stats
        .get(&right.id)
        .and_then(|stats| stats.last_activity.as_deref());
    match (left_activity, right_activity) {
        (Some(left_activity), Some(right_activity)) => right_activity.cmp(left_activity),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
    .then_with(|| left.sort_order.cmp(&right.sort_order))
    .then_with(|| left.name.cmp(&right.name))
}

#[derive(Clone, Copy)]
enum PlanStepCascadeAction {
    AutoComplete,
    Reopen,
}

impl PlanStepCascadeAction {
    fn audit_action(self) -> &'static str {
        match self {
            Self::AutoComplete => "auto-complete",
            Self::Reopen => "auto-reopen",
        }
    }

    fn response_verb(self) -> &'static str {
        match self {
            Self::AutoComplete => "auto-completed",
            Self::Reopen => "reopened",
        }
    }
}

struct CascadedPlanStep {
    id: i64,
    plan_identifier: String,
}

/// Fetch only plan-step changes made by the issue-status cascade that just
/// ran. The audit rows are emitted by migration 020 before each step mutation,
/// which distinguishes an actual cascade from a linked step that was already
/// in its target state.
fn issue_status_cascaded_plan_steps(
    conn: &rusqlite::Connection,
    audit_checkpoint: i64,
    issue_id: i64,
    action: PlanStepCascadeAction,
) -> Result<Vec<CascadedPlanStep>, crate::error::LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT ps.id, p.identifier || '-PLAN-' || pl.sequence
           FROM audit_log al
           JOIN plan_steps ps ON ps.id = al.entity_id
           JOIN plans pl ON pl.id = ps.plan_id
           JOIN projects p ON p.id = pl.project_id
          WHERE al.id > ?1
            AND al.issue_id = ?2
            AND al.entity_type = 'plan_step'
            AND al.action = ?3
          ORDER BY al.id ASC",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![audit_checkpoint, issue_id, action.audit_action()],
        |row| {
            Ok(CascadedPlanStep {
                id: row.get(0)?,
                plan_identifier: row.get(1)?,
            })
        },
    )?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn fmt_issue_plan_step_cascade(
    action: PlanStepCascadeAction,
    steps: &[CascadedPlanStep],
) -> Option<String> {
    match steps {
        [] => None,
        [step] => Some(format!(
            " ({} plan step #{} in {})",
            action.response_verb(),
            step.id,
            step.plan_identifier
        )),
        steps => Some(format!(
            " ({} {} plan steps: {})",
            action.response_verb(),
            steps.len(),
            steps
                .iter()
                .map(|step| format!("#{} in {}", step.id, step.plan_identifier))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn fmt_steps(nodes: &[models::PlanStepNode], depth: usize, out: &mut String) {
    for n in nodes {
        let indent = "  ".repeat(depth);
        let check = if n.done { "x" } else { " " };
        // Provenance suffix for issue-linked steps.
        let suffix = match (&n.issue_identifier, n.issue_status.as_deref()) {
            (Some(iss), Some("done")) if n.done => format!(" (via {iss})"),
            (Some(iss), _) if n.reopened_via_issue_at.is_some() && !n.done => {
                format!(" (reopened — {iss} reopened)")
            }
            (Some(iss), Some(st)) => format!(" [{iss}: {st}]"),
            (Some(iss), None) => format!(" [{iss}]"),
            _ => String::new(),
        };
        out.push_str(&format!(
            "{indent}- [{check}] #{} {}{}\n",
            n.id, n.title, suffix
        ));
        if !n.description.is_empty() {
            out.push_str(&format!(
                "{indent}    {}\n",
                truncate_value(&n.description, 100)
            ));
        }
        if !n.children.is_empty() {
            fmt_steps(&n.children, depth + 1, out);
        }
    }
}

/// Relation identifiers annotated with the related issue's current status,
/// used only by `get_issue` (LIF-303). Computed inside the read closure so all
/// the status lookups share one connection.
struct AnnotatedRelations {
    blocks: Vec<String>,
    blocked_by: Vec<String>,
    relates_to: Vec<String>,
    duplicates: Vec<String>,
    duplicated_by: Vec<String>,
}

/// Surgical string replacement for `edit_issue` / `edit_page`.
///
/// Mirrors the semantics of the Edit tool that coding agents already know:
/// fail loudly when `old_string` is missing or ambiguous so the agent gets a
/// clear error and can self-correct, rather than silently mangling content.
///
/// Returns the new content on success.
fn apply_edit(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Result<String, crate::error::LificError> {
    if old.is_empty() {
        return Err(crate::error::LificError::BadRequest(
            "old_string cannot be empty".into(),
        ));
    }
    if old == new {
        return Err(crate::error::LificError::BadRequest(
            "old_string and new_string must differ".into(),
        ));
    }

    let count = content.matches(old).count();
    match count {
        0 => Err(crate::error::LificError::BadRequest(
            "old_string not found in content; check exact whitespace and newlines".into(),
        )),
        1 => Ok(content.replacen(old, new, 1)),
        _ if replace_all => Ok(content.replace(old, new)),
        n => Err(crate::error::LificError::BadRequest(format!(
            "old_string matches {n} locations; provide more surrounding context to make it unique, or set replace_all=true"
        ))),
    }
}

fn resolve_project(db: &Arc<DbPool>, ident: &str) -> Result<i64, String> {
    let conn = db.read().map_err(|e| e.to_string())?;
    queries::resolve_project_identifier(&conn, ident).map_err(|e| e.to_string())
}

fn resolve_module(db: &Arc<DbPool>, project_id: i64, name: &str) -> Result<i64, String> {
    let conn = db.read().map_err(|e| e.to_string())?;
    queries::resolve_module_name(&conn, project_id, name).map_err(|e| e.to_string())
}

fn resolve_folder(db: &Arc<DbPool>, project_id: i64, name: &str) -> Result<i64, String> {
    let conn = db.read().map_err(|e| e.to_string())?;
    queries::resolve_folder_name(&conn, project_id, name).map_err(|e| e.to_string())
}

/// LIF-145 sentinel for a create's simple `Option<String>` icon field: field
/// omitted or empty string both mean "no icon"; a non-empty string sets it.
fn emoji_for_create(emoji: &Option<String>) -> Option<String> {
    emoji.as_ref().filter(|s| !s.is_empty()).cloned()
}

/// LIF-145 sentinel for an update's tristate `Option<Option<String>>` icon
/// field: omitted (None) = leave unchanged, empty string = clear to NULL,
/// non-empty = set.
fn emoji_for_update(emoji: &Option<String>) -> Option<Option<String>> {
    match emoji {
        None => None,
        Some(s) if s.is_empty() => Some(None),
        Some(s) => Some(Some(s.clone())),
    }
}

/// Heuristic to tell page identifiers (`PRO-DOC-1`, `DOC-1`) apart from issue
/// identifiers (`PRO-42`). Both shapes use uppercase project prefixes and
/// numeric sequences, but the literal `DOC` segment is unique to pages.
fn looks_like_page_identifier(s: &str) -> bool {
    s.starts_with("DOC-") || s.contains("-DOC-")
}

/// Resolve an identifier string into a CommentParent (Issue or Page) so MCP
/// `add_comment` / `list_comments` accept both shapes through one entry point.
fn resolve_comment_parent(
    mcp: &LificMcp,
    identifier: &str,
) -> Result<queries::comments::CommentParent, String> {
    if looks_like_page_identifier(identifier) {
        let conn = mcp.db.read().map_err(|e| e.to_string())?;
        let id = queries::resolve_page_identifier(&conn, identifier).map_err(|e| e.to_string())?;
        Ok(queries::comments::CommentParent::Page(id))
    } else {
        let conn = mcp.db.read().map_err(|e| e.to_string())?;
        let id = queries::resolve_identifier(&conn, identifier).map_err(|e| e.to_string())?;
        Ok(queries::comments::CommentParent::Issue(id))
    }
}

/// LIF-143: resolve the project a comment belongs to, via its issue or page
/// parent. Mirrors `api::comments::resolve_comment_project` — a page comment
/// may have a NULL project (workspace page), which `mention_candidates`
/// handles.
fn resolve_comment_project(
    conn: &rusqlite::Connection,
    comment: &models::Comment,
) -> Result<Option<i64>, crate::error::LificError> {
    if let Some(issue_id) = comment.issue_id {
        Ok(Some(queries::get_issue(conn, issue_id)?.project_id))
    } else if let Some(page_id) = comment.page_id {
        Ok(queries::get_page(conn, page_id)?.project_id)
    } else {
        Ok(None)
    }
}

/// Append a paging hint to `out` if `has_more` is true.
/// `next_offset` is the offset the agent should use to fetch the next page.
fn append_pagination_hint(out: &mut String, has_more: bool, next_offset: i64) {
    if has_more {
        out.push_str(&format!(
            "\n... more results available — call again with offset={next_offset}\n"
        ));
    }
}

/// Truncate long values (descriptions, page bodies) for one-line activity
/// rendering. Newlines collapse so an entry never spans lines.
fn truncate_value(v: &str, max: usize) -> String {
    let flat = v.replace('\n', " ");
    if flat.chars().count() <= max {
        flat
    } else {
        let cut: String = flat.chars().take(max).collect();
        format!("{cut}…")
    }
}

/// Render one audit entry as a compact line:
/// `[ts] actor via transport — LABEL action detail`
fn fmt_activity(a: &models::Activity) -> String {
    let who = match (&a.actor_display_name, &a.actor_username) {
        (Some(d), _) if !d.is_empty() => d.clone(),
        (_, Some(u)) => u.clone(),
        _ => "system".into(),
    };
    let agent = if a.actor_is_bot { " (agent)" } else { "" };
    let label = a.entity_label.as_deref().unwrap_or("?");

    let detail = match a.action.as_str() {
        "create" => format!(
            "created {} {}: {}",
            a.entity_type,
            label,
            truncate_value(a.new_value.as_deref().unwrap_or(""), 60)
        ),
        "delete" => format!(
            "deleted {} {} ({})",
            a.entity_type,
            label,
            truncate_value(a.old_value.as_deref().unwrap_or(""), 60)
        ),
        "update" => format!(
            "{} {}: {} → {}",
            label,
            a.field.as_deref().unwrap_or("?"),
            truncate_value(a.old_value.as_deref().unwrap_or("(none)"), 60),
            truncate_value(a.new_value.as_deref().unwrap_or("(none)"), 60)
        ),
        "attach" => format!("{} +label {}", label, a.new_value.as_deref().unwrap_or("?")),
        "detach" => format!("{} -label {}", label, a.old_value.as_deref().unwrap_or("?")),
        "link" => format!(
            "{} {} → {}",
            label,
            a.field.as_deref().unwrap_or("relates_to"),
            a.new_value.as_deref().unwrap_or("?")
        ),
        "unlink" => format!(
            "{} un-{} → {}",
            label,
            a.field.as_deref().unwrap_or("relates_to"),
            a.old_value.as_deref().unwrap_or("?")
        ),
        other => format!("{label} {other}"),
    };

    format!(
        "[{}] {}{} via {} — {}",
        a.ts, who, agent, a.transport, detail
    )
}

// ── LIF-198: MCP authorization gates ────────────────────────────
//
// Same enforcement primitives as REST (LIF-197): `crate::authz`. But unlike
// the REST wrappers in `api/mod.rs` / `api/pages.rs` / `api/resources.rs`
// (which forward straight into `authz::require_structure_role` /
// `require_project_delete_role` / `require_role(.., Lead)` and rely on
// *those* functions' legacy branches to reproduce REST's pre-existing
// behavior), MCP never had ANY project-scoped gate before this issue — every
// tool was wide open. Those legacy branches reproduce specifically REST's
// history (e.g. structure endpoints were Lead-gated pre-LIF-194, project
// delete was admin-only), which is a *regression* if borrowed verbatim by
// MCP: it would newly deny calls that MCP always allowed.
//
// So every MCP gate below checks `authz_enforced` itself first and
// short-circuits to an unconditional allow while the flag is off —
// reproducing MCP's actual historical behavior (fully open), not REST's.
// Once `authz_enforced` is on, each gate delegates to the exact same
// `crate::authz` primitive REST uses, so enforced-mode semantics are
// identical across both transports. `mcp_gate` centralizes that flag check;
// `LificError` denials translate to the `String` error type
// `self.read`/`self.write` already use, so a denial renders as the same
// `Error: Forbidden: <msg>` shape every other MCP error uses.
fn mcp_gate(
    db: &Arc<DbPool>,
    check: impl FnOnce() -> Result<(), crate::error::LificError>,
) -> Result<(), String> {
    match crate::authz::authz_enforced(db) {
        Ok(false) => Ok(()),
        Ok(true) => check().map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// Require the caller hold at least `min` role on `project_id`.
fn require_role_mcp(db: &Arc<DbPool>, project_id: i64, min: models::Role) -> Result<(), String> {
    mcp_gate(db, || {
        crate::authz::require_role(db, &super::current_auth_user(), project_id, min)
    })
}

/// Gate for module/label/folder ("structure") mutations — Maintainer once
/// enforcement is on; a no-op (MCP's historical behavior) in legacy mode.
fn require_structure_role_mcp(db: &Arc<DbPool>, project_id: i64) -> Result<(), String> {
    mcp_gate(db, || {
        crate::authz::require_structure_role(db, &super::current_auth_user(), project_id)
    })
}

/// Gate for `delete(resource_type="project")` — Lead once enforcement is on
/// (design decision #6); a no-op (MCP's historical behavior) in legacy mode.
fn require_project_delete_role_mcp(db: &Arc<DbPool>, project_id: i64) -> Result<(), String> {
    mcp_gate(db, || {
        crate::authz::require_project_delete_role(db, &super::current_auth_user(), project_id)
    })
}

/// Gate for workspace-level (project-less) pages/comments — admin-only once
/// enforcement is on, a no-op in legacy mode either way (matches
/// `authz::require_workspace_admin`'s own legacy branch, so `mcp_gate`'s
/// short-circuit here is redundant but harmless).
fn require_workspace_admin_mcp(db: &Arc<DbPool>) -> Result<(), String> {
    mcp_gate(db, || {
        crate::authz::require_workspace_admin(db, &super::current_auth_user())
    })
}

/// Gate for a page/comment target whose `project_id` may be `None`
/// (workspace-level): project-scoped falls to `require_role_mcp`,
/// project-less falls to `require_workspace_admin_mcp`. Mirrors
/// `api::pages::require_page_role` / `api::comments::require_comment_viewer`.
fn require_page_role_mcp(
    db: &Arc<DbPool>,
    project_id: Option<i64>,
    min: models::Role,
) -> Result<(), String> {
    match project_id {
        Some(pid) => require_role_mcp(db, pid, min),
        None => require_workspace_admin_mcp(db),
    }
}

/// The cross-project read filter (LIF-197 scope item 2, MCP-side): a
/// non-visible project's items are silently absent, never an error. Mirrors
/// `api::filter_visible`.
fn filter_visible<T>(
    items: Vec<T>,
    visible: &Option<std::collections::HashSet<i64>>,
    project_id_of: impl Fn(&T) -> Option<i64>,
) -> Vec<T> {
    match visible {
        None => items,
        Some(ids) => items
            .into_iter()
            .filter(|it| project_id_of(it).is_some_and(|pid| ids.contains(&pid)))
            .collect(),
    }
}

fn visible_project_ids_mcp(
    db: &Arc<DbPool>,
) -> Result<Option<std::collections::HashSet<i64>>, String> {
    crate::authz::visible_project_ids(db, &super::current_auth_user()).map_err(|e| e.to_string())
}

impl LificMcp {
    /// LIF-257: return the self-onboarding nudge iff the DB genuinely has
    /// **zero** projects. Uses the unfiltered `list_projects` (not the
    /// authz-filtered visible set) on purpose: "projects exist but none are
    /// visible to this user" is an authz state that must keep its current
    /// output — nudging there would both mislead a real user and leak that
    /// projects exist. Only a truly empty DB gets the nudge.
    fn no_projects_nudge(&self) -> Option<String> {
        match self.read(queries::list_projects) {
            Ok(ps) if ps.is_empty() => Some(NO_PROJECTS_NUDGE.to_string()),
            _ => None,
        }
    }

    /// Gate for comment read/create: Viewer (or Maintainer, for edges that
    /// need it) on the comment parent's project, falling back to
    /// workspace-admin for a page with no project. Mirrors
    /// `api::comments::require_comment_viewer`.
    fn require_comment_role_mcp(
        &self,
        parent: queries::comments::CommentParent,
        min: models::Role,
    ) -> Result<(), String> {
        let project_id: Option<i64> = match parent {
            queries::comments::CommentParent::Issue(id) => {
                Some(self.read(|conn| queries::get_issue(conn, id))?.project_id)
            }
            queries::comments::CommentParent::Page(id) => {
                self.read(|conn| queries::get_page(conn, id))?.project_id
            }
        };
        require_page_role_mcp(&self.db, project_id, min)
    }

    /// LIF-143: resolve the acting MCP user for a comment edit/delete the
    /// same way `add_comment` resolves the author: the HTTP-auth user set in
    /// the task-local, else fall back to the first admin for stdio/local
    /// sessions. Returns `(user_id, is_admin)` for the author-or-admin
    /// ownership check.
    fn resolve_comment_actor(&self) -> Result<(i64, bool), String> {
        match super::current_auth_user() {
            Some(u) => Ok((u.id, u.is_admin)),
            None => match self.read(queries::users::first_admin)? {
                Some(admin) => Ok((admin.id, admin.is_admin)),
                None => Err("no admin user exists to attribute comment edits to.".into()),
            },
        }
    }

    /// LIF-198: if `step_id` has a linked issue, require `min` role on that
    /// issue's own project too — the same "both sides" check `link_issues`
    /// applies, since a step's issue can live in a different project than
    /// the plan it belongs to.
    fn require_step_issue_role_mcp(&self, step_id: i64, min: models::Role) -> Result<(), String> {
        match self.read(|conn| queries::plans::step_issue_id(conn, step_id))? {
            Some(issue_id) => {
                let project_id = self
                    .read(|conn| queries::get_issue(conn, issue_id))?
                    .project_id;
                require_role_mcp(&self.db, project_id, min)
            }
            None => Ok(()),
        }
    }

    /// LIF-198: require `min` role on the project of the issue identified
    /// by `ident` — used for `attach_issue` / `add_child_issue` cross-project
    /// edges in `update_plan_step`, mirroring `link_issues`.
    fn require_issue_ident_role_mcp(&self, ident: &str, min: models::Role) -> Result<(), String> {
        let project_id = self.read(|conn| {
            let iid = queries::resolve_identifier(conn, ident)?;
            Ok(queries::get_issue(conn, iid)?.project_id)
        })?;
        require_role_mcp(&self.db, project_id, min)
    }
}

#[tool_router]
impl LificMcp {
    #[tool(description = "Search across all issues, pages, and comments by text")]
    fn search(&self, Parameters(input): Parameters<SearchInput>) -> String {
        let project_id = match &input.project {
            Some(p) => match resolve_project(&self.db, p) {
                Ok(id) => Some(id),
                Err(e) => return format!("Error: {e}"),
            },
            None => None,
        };
        let limit = input.limit.unwrap_or(20).max(1);
        let offset = input.offset.unwrap_or(0).max(0);
        // Cross-project read (LIF-198 scope item 2): non-visible projects'
        // hits are silently absent, never a 403 — even when `project` narrows
        // the search to one project a non-member can't see, mirroring the
        // REST /api/search handler.
        let visible = match visible_project_ids_mcp(&self.db) {
            Ok(v) => v,
            Err(e) => return format!("Error: {e}"),
        };
        // Over-fetch by one to detect whether more results exist beyond this page.
        match self.read(|conn| {
            queries::search(
                conn,
                &models::SearchQuery {
                    query: input.query.clone(),
                    project_id,
                    result_type: input.result_type.clone(),
                    sort: input.sort.clone(),
                    mode: input.mode.clone(),
                    limit: Some(limit + 1),
                    offset: Some(offset),
                },
            )
        }) {
            Ok(results) => {
                let mut results = filter_visible(results, &visible, |r| r.project_id);
                if results.is_empty() {
                    // LIF-257: nudge only when the search itself came up
                    // empty AND the DB has no projects — checked after the
                    // query so workspace-level pages (which can exist with
                    // zero projects) are never hidden by the nudge.
                    if let Some(nudge) = self.no_projects_nudge() {
                        return nudge;
                    }
                    return "No results found.".into();
                }
                let has_more = results.len() as i64 > limit;
                if has_more {
                    results.truncate(limit as usize);
                }
                let mut out = format!("{} results:\n", results.len());
                for r in &results {
                    let ident = r.identifier.as_deref().unwrap_or("");
                    // A comment hit has no title of its own; render it as a
                    // match on its parent so the reader knows to open the
                    // parent issue/page to find the thread (LIF-146).
                    if r.result_type == "comment" {
                        out.push_str(&format!("- [comment] on {ident} — {}\n", r.snippet));
                    } else {
                        out.push_str(&format!(
                            "- [{}] {} {} — {}\n",
                            r.result_type, ident, r.title, r.snippet
                        ));
                    }
                }
                append_pagination_hint(&mut out, has_more, offset + limit);
                out
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(
        description = "Read the audit log: who changed what, when, and through which door (web UI, MCP, API, CLI). Takes an issue, page, or project ID; project scope covers the whole feed. Newest-first with old and new values."
    )]
    fn get_activity(&self, Parameters(input): Parameters<GetActivityInput>) -> String {
        let limit = input.limit.unwrap_or(30).clamp(1, 200);
        let offset = input.offset.unwrap_or(0).max(0);
        let ident = input.identifier.trim();

        // Resolve the identifier shape: page → issue → project. Pages are
        // unambiguous (DOC segment); issue resolution requires a numeric
        // tail, so a bare project identifier falls through cleanly.
        let scope = if looks_like_page_identifier(ident) {
            match self.read(|conn| queries::resolve_page_identifier(conn, ident)) {
                Ok(id) => queries::activity::ActivityScope::Page(id),
                Err(e) => return format!("Error: {e}"),
            }
        } else if let Ok(id) = self.read(|conn| queries::resolve_identifier(conn, ident)) {
            queries::activity::ActivityScope::Issue(id)
        } else {
            match self.read(|conn| queries::resolve_project_identifier(conn, ident)) {
                Ok(id) => queries::activity::ActivityScope::Project(id),
                Err(_) => {
                    return format!(
                        "Error: '{ident}' is not a known issue, page, or project identifier"
                    );
                }
            }
        };

        // LIF-198: resolve the scope's project (Viewer gate); workspace
        // pages (project_id None) fall back to admin-only.
        let project_id: Option<i64> = match scope {
            queries::activity::ActivityScope::Issue(id) => {
                match self.read(|conn| queries::get_issue(conn, id)) {
                    Ok(issue) => Some(issue.project_id),
                    Err(e) => return format!("Error: {e}"),
                }
            }
            queries::activity::ActivityScope::Page(id) => {
                match self.read(|conn| queries::get_page(conn, id)) {
                    Ok(page) => page.project_id,
                    Err(e) => return format!("Error: {e}"),
                }
            }
            queries::activity::ActivityScope::Plan(id) => {
                match self.read(|conn| queries::plans::get_plan(conn, id)) {
                    Ok(plan) => Some(plan.project_id),
                    Err(e) => return format!("Error: {e}"),
                }
            }
            queries::activity::ActivityScope::Project(id) => Some(id),
        };
        if let Err(e) = require_page_role_mcp(&self.db, project_id, models::Role::Viewer) {
            return format!("Error: {e}");
        }

        match self
            .read(|conn| queries::activity::list_activity(conn, scope, Some(limit), Some(offset)))
        {
            Ok(feed) if feed.items.is_empty() && offset == 0 => {
                format!("No recorded activity for {ident} yet.")
            }
            Ok(feed) => {
                let mut out = format!("{} activity entries for {ident}:\n", feed.items.len());
                for a in &feed.items {
                    out.push_str(&format!("- {}\n", fmt_activity(a)));
                }
                append_pagination_hint(&mut out, feed.has_more, offset + limit);
                out
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(
        description = "List issues for a project. workable=true gives issues with no blockers, blocked=true for issues with at least one blocker."
    )]
    fn list_issues(&self, Parameters(input): Parameters<ListIssuesInput>) -> String {
        if let Some(nudge) = self.no_projects_nudge() {
            return nudge;
        }
        let pid = match resolve_project(&self.db, &input.project) {
            Ok(id) => id,
            Err(e) => return format!("Error: {e}"),
        };
        if let Err(e) = require_role_mcp(&self.db, pid, models::Role::Viewer) {
            return format!("Error: {e}");
        }
        let module_id = match &input.module {
            Some(name) => match resolve_module(&self.db, pid, name) {
                Ok(id) => Some(id),
                Err(e) => return format!("Error: {e}"),
            },
            None => None,
        };
        let limit = input.limit.unwrap_or(50).max(1);
        let offset = input.offset.unwrap_or(0).max(0);
        // Over-fetch by one to detect whether more results exist beyond this page.
        match self.read(|conn| {
            queries::list_issues(
                conn,
                &models::ListIssuesQuery {
                    project_id: Some(pid),
                    status: input.status.clone(),
                    priority: input.priority.clone(),
                    module_id,
                    label: input.label.clone(),
                    workable: input.workable,
                    blocked: input.blocked,
                    created_since: input.created_since.clone(),
                    created_until: input.created_until.clone(),
                    updated_since: input.updated_since.clone(),
                    updated_until: input.updated_until.clone(),
                    order_by: input.order_by.clone(),
                    order: input.order.clone(),
                    limit: Some(limit + 1),
                    offset: Some(offset),
                },
            )
        }) {
            Ok(issues) if issues.is_empty() => "No issues found.".into(),
            Ok(mut issues) => {
                let has_more = issues.len() as i64 > limit;
                if has_more {
                    issues.truncate(limit as usize);
                }
                let mut out = format!("{} issues:\n", issues.len());
                for i in &issues {
                    out.push_str(&format!("- {}\n", fmt_issue(i)));
                }
                append_pagination_hint(&mut out, has_more, offset + limit);
                out
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(
        description = "Get an issue by ID (e.g. LIF-1): full details plus the last 3 comments by default."
    )]
    fn get_issue(&self, Parameters(input): Parameters<GetIssueInput>) -> String {
        // Validate the comment-trail mode up front so a typo errors instead of
        // silently defaulting.
        let comment_mode = input.include_comments.as_deref().unwrap_or("recent");
        if !matches!(comment_mode, "recent" | "all" | "none") {
            return format!(
                "Error: invalid include_comments '{comment_mode}'. Use recent, all, or none."
            );
        }
        match self.read(|conn| {
            let id = queries::resolve_identifier(conn, &input.identifier)?;
            let issue = queries::get_issue(conn, id)?;
            let module_name = match issue.module_id {
                Some(mid) => queries::get_module_name(conn, mid).unwrap_or("unknown".into()),
                None => "none".into(),
            };
            // LIF-303: annotate each relation identifier with the related
            // issue's current status (get_issue only — fmt_issue/board/list
            // stay bare). A failed lookup falls back to the bare identifier so
            // one bad edge never errors the whole read.
            let annotate = |idents: &[String]| -> Vec<String> {
                idents
                    .iter()
                    .map(|ident| {
                        match queries::resolve_identifier(conn, ident)
                            .and_then(|iid| queries::issue_status(conn, iid))
                        {
                            Ok(status) => format!("{ident} ({status})"),
                            Err(_) => ident.clone(),
                        }
                    })
                    .collect()
            };
            let rels = AnnotatedRelations {
                blocks: annotate(&issue.blocks),
                blocked_by: annotate(&issue.blocked_by),
                relates_to: annotate(&issue.relates_to),
                duplicates: annotate(&issue.duplicates),
                duplicated_by: annotate(&issue.duplicated_by),
            };
            Ok((issue, module_name, rels))
        }) {
            Ok((issue, module_name, rels)) => {
                if let Err(e) = require_role_mcp(&self.db, issue.project_id, models::Role::Viewer) {
                    return format!("Error: {e}");
                }
                let mut out = format!(
                    "{} — {}\nStatus: {} | Priority: {} | Module: {}\n",
                    issue.identifier, issue.title, issue.status, issue.priority, module_name
                );
                if !issue.labels.is_empty() {
                    out.push_str(&format!("Labels: {}\n", issue.labels.join(", ")));
                }
                if !rels.blocks.is_empty() {
                    out.push_str(&format!("Blocks: {}\n", rels.blocks.join(", ")));
                }
                if !rels.blocked_by.is_empty() {
                    out.push_str(&format!("Blocked by: {}\n", rels.blocked_by.join(", ")));
                }
                if !rels.relates_to.is_empty() {
                    out.push_str(&format!("Relates to: {}\n", rels.relates_to.join(", ")));
                }
                if !rels.duplicates.is_empty() {
                    out.push_str(&format!("Duplicates: {}\n", rels.duplicates.join(", ")));
                }
                if !rels.duplicated_by.is_empty() {
                    out.push_str(&format!(
                        "Duplicated by: {}\n",
                        rels.duplicated_by.join(", ")
                    ));
                }
                if !issue.description.is_empty() {
                    out.push_str(&format!("\n{}\n", issue.description));
                }
                // Include comments per the requested trail mode (LIF-301).
                if let Ok(comments) = self.read(|conn| {
                    queries::comments::list_comments(
                        conn,
                        queries::comments::CommentParent::Issue(issue.id),
                        None,
                        None,
                    )
                }) && !comments.is_empty()
                {
                    let total = comments.len();
                    match comment_mode {
                        // Emit only a stub pointing at list_comments.
                        "none" => {
                            out.push_str(&format!(
                                "\n--- Comments ({total}, omitted — use list_comments) ---\n"
                            ));
                        }
                        // Last 3 by default; full header only when truncating.
                        "recent" if total > 3 => {
                            out.push_str(&format!(
                                "\n--- Comments ({total}, showing last 3 — use list_comments) ---\n"
                            ));
                            for c in &comments[total - 3..] {
                                out.push_str(&format!(
                                    "[{}] {} ({}): {}\n",
                                    c.created_at, c.author, c.author_display_name, c.content
                                ));
                            }
                        }
                        // "all", or "recent" with <= 3 comments: today's format.
                        _ => {
                            out.push_str(&format!("\n--- Comments ({total}) ---\n"));
                            for c in &comments {
                                out.push_str(&format!(
                                    "[{}] {} ({}): {}\n",
                                    c.created_at, c.author, c.author_display_name, c.content
                                ));
                            }
                        }
                    }
                }
                out
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(
        description = "Export as markdown: an issue (PRO-42), a page (PRO-DOC-3), or a whole project (PRO). Issues and pages return the markdown; projects return the exported file paths."
    )]
    fn export(&self, Parameters(input): Parameters<ExportInput>) -> String {
        let ident = input.identifier.trim();
        // Same identifier-shape dispatch as get_activity: pages are
        // unambiguous (DOC segment); issue resolution requires a numeric
        // tail, so a bare project identifier falls through cleanly.
        if looks_like_page_identifier(ident) {
            let project_id = match self.read(|conn| {
                let id = queries::resolve_page_identifier(conn, ident)?;
                Ok(queries::get_page(conn, id)?.project_id)
            }) {
                Ok(pid) => pid,
                Err(e) => return format!("Error: {e}"),
            };
            if let Err(e) = require_page_role_mcp(&self.db, project_id, models::Role::Viewer) {
                return format!("Error: {e}");
            }
            match self.read(|conn| crate::export::export_page(conn, ident)) {
                Ok(bundle) => bundle
                    .files
                    .into_iter()
                    .next()
                    .map(|file| file.content)
                    .unwrap_or_else(|| "Error: page export produced no files".into()),
                Err(e) => format!("Error: {e}"),
            }
        } else if let Ok(project_id) = self.read(|conn| {
            let id = queries::resolve_identifier(conn, ident)?;
            Ok(queries::get_issue(conn, id)?.project_id)
        }) {
            if let Err(e) = require_role_mcp(&self.db, project_id, models::Role::Viewer) {
                return format!("Error: {e}");
            }
            match self.read(|conn| crate::export::export_issue(conn, ident)) {
                Ok(bundle) => bundle
                    .files
                    .into_iter()
                    .next()
                    .map(|file| file.content)
                    .unwrap_or_else(|| "Error: issue export produced no files".into()),
                Err(e) => format!("Error: {e}"),
            }
        } else {
            let pid = match resolve_project(&self.db, ident) {
                Ok(id) => id,
                Err(_) => {
                    return format!(
                        "Error: '{ident}' is not a known issue, page, or project identifier"
                    );
                }
            };
            if let Err(e) = require_role_mcp(&self.db, pid, models::Role::Viewer) {
                return format!("Error: {e}");
            }
            match self.read(|conn| crate::export::export_project(conn, ident)) {
                Ok(bundle) => {
                    let mut out = format!("{} exported file(s):\n", bundle.files.len());
                    for file in bundle.files {
                        out.push_str(&format!("- {}\n", file.path));
                    }
                    out
                }
                Err(e) => format!("Error: {e}"),
            }
        }
    }

    #[tool(description = "Create a new issue in a project")]
    fn create_issue(&self, Parameters(input): Parameters<CreateIssueInput>) -> String {
        let pid = match resolve_project(&self.db, &input.project) {
            Ok(id) => id,
            Err(e) => return format!("Error: {e}"),
        };
        if let Err(e) = require_role_mcp(&self.db, pid, models::Role::Maintainer) {
            return format!("Error: {e}");
        }
        let module_id = match &input.module {
            Some(name) => match resolve_module(&self.db, pid, name) {
                Ok(id) => Some(id),
                Err(e) => return format!("Error: {e}"),
            },
            None => None,
        };
        match self.write(|conn| {
            queries::create_issue(
                conn,
                &models::CreateIssue {
                    project_id: pid,
                    title: input.title.clone(),
                    description: input.description.clone().unwrap_or_default(),
                    status: input.status.clone().unwrap_or("backlog".into()),
                    priority: input.priority.clone().unwrap_or("none".into()),
                    module_id,
                    start_date: input.start_date.clone(),
                    target_date: input.target_date.clone(),
                    labels: input.labels.clone().unwrap_or_default(),
                    source: None,
                },
            )
        }) {
            Ok(issue) => {
                self.emit(crate::realtime::RealtimeEvent::IssueCreated {
                    project_id: issue.project_id,
                    issue_id: issue.id,
                });
                format!("Created {}: {}", issue.identifier, issue.title)
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(
        description = "Update an existing issue by identifier. Only provided fields are changed."
    )]
    fn update_issue(&self, Parameters(input): Parameters<UpdateIssueInput>) -> String {
        let (id, project_id) = match self.read(|conn| {
            let id = queries::resolve_identifier(conn, &input.identifier)?;
            let project_id = queries::get_issue(conn, id)?.project_id;
            Ok((id, project_id))
        }) {
            Ok(v) => v,
            Err(e) => return format!("Error: {e}"),
        };
        if let Err(e) = require_role_mcp(&self.db, project_id, models::Role::Maintainer) {
            return format!("Error: {e}");
        }
        match self.write(|conn| {
            // Migration 020's cascades key exclusively on transitions to or
            // from `done`, not on the broader cancelled/open distinction.
            // Keep an audit checkpoint before the direct issue update so the
            // response can name only steps the trigger actually changed.
            let previous_issue = queries::get_issue(conn, id)?;
            let audit_checkpoint = if input.status.is_some() {
                Some(
                    conn.query_row("SELECT COALESCE(MAX(id), 0) FROM audit_log", [], |row| {
                        row.get(0)
                    })?,
                )
            } else {
                None
            };
            // LIF-145 sentinel: field omitted (None) = skip, empty string = clear
            // (unassign module), non-empty = resolve + set.
            let module_id = match &input.module {
                Some(name) if name.is_empty() => Some(None),
                Some(name) => Some(Some(queries::resolve_module_name(
                    conn,
                    previous_issue.project_id,
                    name,
                )?)),
                None => None,
            };
            let issue = queries::update_issue(
                conn,
                id,
                &models::UpdateIssue {
                    title: input.title.clone(),
                    description: input.description.clone(),
                    status: input.status.clone(),
                    priority: input.priority.clone(),
                    module_id,
                    sort_order: None,
                    start_date: input.start_date.clone(),
                    target_date: input.target_date.clone(),
                    labels: input.labels.clone(),
                },
            )?;
            let cascade_action = match (previous_issue.status.as_str(), issue.status.as_str()) {
                (previous, "done") if previous != "done" => {
                    Some(PlanStepCascadeAction::AutoComplete)
                }
                ("done", current) if current != "done" => Some(PlanStepCascadeAction::Reopen),
                _ => None,
            };
            let cascaded_steps = match (cascade_action, audit_checkpoint) {
                (Some(action), Some(checkpoint)) => {
                    issue_status_cascaded_plan_steps(conn, checkpoint, id, action)?
                }
                _ => Vec::new(),
            };
            Ok((issue, cascade_action, cascaded_steps))
        }) {
            Ok((issue, cascade_action, cascaded_steps)) => {
                self.emit(crate::realtime::RealtimeEvent::IssueUpdated {
                    project_id: issue.project_id,
                    issue_id: issue.id,
                });
                let mut output = format!("Updated {}: {}", issue.identifier, fmt_issue(&issue));
                if let Some(note) = cascade_action
                    .and_then(|action| fmt_issue_plan_step_cascade(action, &cascaded_steps))
                {
                    output.push_str(&note);
                }
                output
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(
        description = "Apply field changes to matching issues in one call. At most 500 matching issues are selected; narrow filters when more matches exist. Returns the number of issues updated."
    )]
    fn bulk_update(&self, Parameters(input): Parameters<BulkUpdateInput>) -> String {
        let pid = match resolve_project(&self.db, &input.project) {
            Ok(id) => id,
            Err(e) => return format!("Error: {e}"),
        };
        // Authz: mirror update_issue — project-scoped Maintainer gate.
        if let Err(e) = require_role_mcp(&self.db, pid, models::Role::Maintainer) {
            return format!("Error: {e}");
        }
        let filter_module_id = match &input.filter_module {
            Some(name) => match resolve_module(&self.db, pid, name) {
                Ok(id) => Some(id),
                Err(e) => return format!("Error: {e}"),
            },
            None => None,
        };
        // Resolve the target module once (present => set it on every match).
        let set_module_id = match &input.set_module {
            Some(name) => match resolve_module(&self.db, pid, name) {
                Ok(id) => Some(id),
                Err(e) => return format!("Error: {e}"),
            },
            None => None,
        };
        // Cap the selection like get_board does; bulk changes over 500 issues
        // in a single call are out of scope for this tool.
        const BULK_CAP: i64 = 500;
        match self.write(|conn| {
            let issues = queries::list_issues(
                conn,
                &models::ListIssuesQuery {
                    project_id: Some(pid),
                    status: input.filter_status.clone(),
                    priority: input.filter_priority.clone(),
                    module_id: filter_module_id,
                    label: input.filter_label.clone(),
                    limit: Some(BULK_CAP),
                    ..Default::default()
                },
            )?;
            issues
                .iter()
                .map(|issue| {
                    queries::update_issue(
                        conn,
                        issue.id,
                        &models::UpdateIssue {
                            title: None,
                            description: None,
                            status: input.set_status.clone(),
                            priority: input.set_priority.clone(),
                            module_id: set_module_id.map(Some),
                            sort_order: None,
                            start_date: None,
                            target_date: None,
                            labels: None,
                        },
                    )
                    .map(|issue| (issue.project_id, issue.id))
                })
                .collect::<Result<Vec<_>, _>>()
        }) {
            Ok(events) => {
                for (project_id, issue_id) in &events {
                    self.emit(crate::realtime::RealtimeEvent::IssueUpdated {
                        project_id: *project_id,
                        issue_id: *issue_id,
                    });
                }
                format!("Updated {} issue(s)", events.len())
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(
        description = "Edit an issue by replacing an exact string. Targets the description by default; pass field='title' for the title. Fails if old_string is missing or ambiguous (unless replace_all=true). Cheaper than update_issue for small changes."
    )]
    fn edit_issue(&self, Parameters(input): Parameters<EditIssueInput>) -> String {
        let (id, project_id) = match self.read(|conn| {
            let id = queries::resolve_identifier(conn, &input.identifier)?;
            let project_id = queries::get_issue(conn, id)?.project_id;
            Ok((id, project_id))
        }) {
            Ok(v) => v,
            Err(e) => return format!("Error: {e}"),
        };
        if let Err(e) = require_role_mcp(&self.db, project_id, models::Role::Maintainer) {
            return format!("Error: {e}");
        }
        match self.write(|conn| {
            let issue = queries::get_issue(conn, id)?;

            let field = input.field.as_deref().unwrap_or("description");
            // Normalize string-field inputs through the same proper-JSON
            // heuristic that `update_issue` applies, so an edit sourced from
            // a double-escaping client matches stored content.
            let (current, old_norm, new_norm) = match field {
                "description" => (
                    issue.description.clone(),
                    queries::unescape_text(&input.old_string),
                    queries::unescape_text(&input.new_string),
                ),
                "title" => (
                    issue.title.clone(),
                    input.old_string.clone(),
                    input.new_string.clone(),
                ),
                other => {
                    return Err(crate::error::LificError::BadRequest(format!(
                        "invalid field '{other}'; expected 'description' or 'title'"
                    )));
                }
            };

            let updated = apply_edit(
                &current,
                &old_norm,
                &new_norm,
                input.replace_all.unwrap_or(false),
            )?;

            let mut patch = models::UpdateIssue {
                title: None,
                description: None,
                status: None,
                priority: None,
                module_id: None,
                sort_order: None,
                start_date: None,
                target_date: None,
                labels: None,
            };
            match field {
                "title" => patch.title = Some(updated),
                "description" => patch.description = Some(updated),
                _ => unreachable!(),
            }

            queries::update_issue(conn, id, &patch)
        }) {
            Ok(issue) => {
                self.emit(crate::realtime::RealtimeEvent::IssueUpdated {
                    project_id: issue.project_id,
                    issue_id: issue.id,
                });
                format!("Edited {}: {}", issue.identifier, fmt_issue(&issue))
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(
        description = "Board view of issues grouped by status (default), priority, or module. Done/cancelled are count-only stubs unless include_closed=true."
    )]
    fn get_board(&self, Parameters(input): Parameters<GetBoardInput>) -> String {
        if let Some(nudge) = self.no_projects_nudge() {
            return nudge;
        }
        let pid = match resolve_project(&self.db, &input.project) {
            Ok(id) => id,
            Err(e) => return format!("Error: {e}"),
        };
        if let Err(e) = require_role_mcp(&self.db, pid, models::Role::Viewer) {
            return format!("Error: {e}");
        }
        const BOARD_CAP: i64 = 500;
        match self.read(|conn| {
            queries::list_issues(
                conn,
                &models::ListIssuesQuery {
                    project_id: Some(pid),
                    // Over-fetch by one to detect truncation.
                    limit: Some(BOARD_CAP + 1),
                    ..Default::default()
                },
            )
        }) {
            Ok(mut issues) => {
                let truncated = issues.len() as i64 > BOARD_CAP;
                if truncated {
                    issues.truncate(BOARD_CAP as usize);
                }
                let group_by = input.group_by.as_deref().unwrap_or("status");
                let include_closed = input.include_closed.unwrap_or(false);
                let is_closed = |i: &models::Issue| i.status == "done" || i.status == "cancelled";
                // For priority/module grouping, closed issues are dropped from
                // every column entirely (a status column would be meaningless
                // there); count how many we drop so a trailing note can report
                // the omission. Status grouping keeps the groups but renders
                // them as count-only stubs (handled below).
                let mut closed_omitted = 0i64;
                if !include_closed && group_by != "status" {
                    let before = issues.len();
                    issues.retain(|i| !is_closed(i));
                    closed_omitted = (before - issues.len()) as i64;
                }
                let module_names: std::collections::HashMap<i64, String> = if group_by == "module" {
                    if let Ok(conn) = self.db.read() {
                        queries::list_modules(&conn, pid)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|m| (m.id, m.name))
                            .collect()
                    } else {
                        std::collections::HashMap::new()
                    }
                } else {
                    std::collections::HashMap::new()
                };
                let mut groups: std::collections::BTreeMap<String, Vec<&models::Issue>> =
                    std::collections::BTreeMap::new();
                for issue in &issues {
                    let key = match group_by {
                        "priority" => issue.priority.clone(),
                        "module" => issue
                            .module_id
                            .and_then(|m| module_names.get(&m).cloned())
                            .unwrap_or("unassigned".into()),
                        _ => issue.status.clone(),
                    };
                    groups.entry(key).or_default().push(issue);
                }
                // LIF-140: order columns by workflow rank, not alphabetically.
                // Status flows backlog → todo → active → done → cancelled;
                // priority flows urgent → none. Module grouping stays
                // alphabetical (the BTreeMap order), which is what you want
                // for arbitrary names. Unknown keys sort after known ones.
                let rank: fn(&str) -> usize = match group_by {
                    "priority" => |k| {
                        ["urgent", "high", "medium", "low", "none"]
                            .iter()
                            .position(|s| *s == k)
                            .unwrap_or(usize::MAX)
                    },
                    "module" => |_| 0, // stable sort keeps alphabetical order
                    _ => |k| {
                        ["backlog", "todo", "active", "done", "cancelled"]
                            .iter()
                            .position(|s| *s == k)
                            .unwrap_or(usize::MAX)
                    },
                };
                let mut ordered: Vec<(&String, &Vec<&models::Issue>)> = groups.iter().collect();
                ordered.sort_by_key(|(key, _)| rank(key));
                let mut out = String::new();
                if truncated {
                    out.push_str(&format!(
                        "warning: board view capped at {BOARD_CAP} issues — older issues are not shown. Use list_issues with offset for full paging.\n\n"
                    ));
                }
                let max_per_column = input.max_per_column.filter(|n| *n >= 0);
                for (group, items) in ordered {
                    // Status grouping keeps closed groups as count-only stubs:
                    // header + stub on ONE line, replacing the item lines. Only
                    // reached for non-empty groups (empty groups never enter
                    // `groups`).
                    if !include_closed
                        && group_by == "status"
                        && (group == "done" || group == "cancelled")
                    {
                        out.push_str(&format!(
                            "── {} ({}) ── [omitted — pass include_closed=true]\n\n",
                            group,
                            items.len()
                        ));
                        continue;
                    }
                    out.push_str(&format!("── {} ({}) ──\n", group, items.len()));
                    let shown = match max_per_column {
                        Some(n) => (n as usize).min(items.len()),
                        None => items.len(),
                    };
                    for i in &items[..shown] {
                        out.push_str(&format!("  {}\n", fmt_issue(i)));
                    }
                    if shown < items.len() {
                        out.push_str(&format!(
                            "  … +{} more (use list_issues)\n",
                            items.len() - shown
                        ));
                    }
                    out.push('\n');
                }
                if closed_omitted > 0 {
                    out.push_str(&format!(
                        "({closed_omitted} closed issues omitted — pass include_closed=true)\n"
                    ));
                }
                out
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Link two issues with a relation: blocks, relates_to, or duplicate")]
    fn link_issues(&self, Parameters(input): Parameters<LinkIssuesInput>) -> String {
        let (source, target) = match self.read(|conn| {
            let source_id = queries::resolve_identifier(conn, &input.source)?;
            let target_id = queries::resolve_identifier(conn, &input.target)?;
            Ok((
                queries::get_issue(conn, source_id)?,
                queries::get_issue(conn, target_id)?,
            ))
        }) {
            Ok(v) => v,
            Err(e) => return format!("Error: {e}"),
        };
        // Cross-project relation: Maintainer required on BOTH sides (LIF-198
        // scope item 3), even when source and target share a project.
        if let Err(e) = require_role_mcp(&self.db, source.project_id, models::Role::Maintainer) {
            return format!("Error: {e}");
        }
        if let Err(e) = require_role_mcp(&self.db, target.project_id, models::Role::Maintainer) {
            return format!("Error: {e}");
        }
        match self
            .write(|conn| queries::link_issues(conn, source.id, target.id, &input.relation_type))
        {
            Ok(()) => {
                self.emit(crate::realtime::RealtimeEvent::IssueLinked {
                    project_id: source.project_id,
                    issue_id: source.id,
                });
                self.emit(crate::realtime::RealtimeEvent::IssueLinked {
                    project_id: target.project_id,
                    issue_id: target.id,
                });
                format!("{} {} {}", input.source, input.relation_type, input.target)
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Remove a relation between two issues")]
    fn unlink_issues(&self, Parameters(input): Parameters<UnlinkIssuesInput>) -> String {
        let (source, target) = match self.read(|conn| {
            let source_id = queries::resolve_identifier(conn, &input.source)?;
            let target_id = queries::resolve_identifier(conn, &input.target)?;
            Ok((
                queries::get_issue(conn, source_id)?,
                queries::get_issue(conn, target_id)?,
            ))
        }) {
            Ok(v) => v,
            Err(e) => return format!("Error: {e}"),
        };
        if let Err(e) = require_role_mcp(&self.db, source.project_id, models::Role::Maintainer) {
            return format!("Error: {e}");
        }
        if let Err(e) = require_role_mcp(&self.db, target.project_id, models::Role::Maintainer) {
            return format!("Error: {e}");
        }
        match self.write(|conn| queries::unlink_issues(conn, source.id, target.id)) {
            Ok(()) => {
                self.emit(crate::realtime::RealtimeEvent::IssueUnlinked {
                    project_id: source.project_id,
                    issue_id: source.id,
                });
                self.emit(crate::realtime::RealtimeEvent::IssueUnlinked {
                    project_id: target.project_id,
                    issue_id: target.id,
                });
                format!("Unlinked {} and {}", input.source, input.target)
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Get a page by identifier (e.g. LIF-DOC-1). Returns full content.")]
    fn get_page(&self, Parameters(input): Parameters<GetPageInput>) -> String {
        match self.read(|conn| {
            let id = queries::resolve_page_identifier(conn, &input.identifier)?;
            let page = queries::get_page(conn, id)?;
            let folder_name = match page.folder_id {
                Some(fid) => Some(queries::get_folder_name(conn, fid)?),
                None => None,
            };
            Ok((page, folder_name))
        }) {
            Ok((page, folder_name)) => {
                if let Err(e) =
                    require_page_role_mcp(&self.db, page.project_id, models::Role::Viewer)
                {
                    return format!("Error: {e}");
                }
                let mut out = format!(
                    "{}{} — {}\nStatus: {} | Folder: {}\nCreated: {} | Updated: {}\n",
                    if page.pinned { "📌 " } else { "" },
                    page.identifier,
                    page.title,
                    page.status,
                    folder_name.as_deref().unwrap_or("none"),
                    page.created_at,
                    page.updated_at
                );
                if !page.labels.is_empty() {
                    out.push_str(&format!("Labels: {}\n", page.labels.join(", ")));
                }
                if !page.content.is_empty() {
                    out.push_str(&format!("\n{}\n", page.content));
                }
                out
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Create a new page in a project")]
    fn create_page(&self, Parameters(input): Parameters<CreatePageInput>) -> String {
        let project_id = match &input.project {
            Some(p) => match resolve_project(&self.db, p) {
                Ok(id) => Some(id),
                Err(e) => return format!("Error: {e}"),
            },
            None => None,
        };
        if let Err(e) = require_page_role_mcp(&self.db, project_id, models::Role::Maintainer) {
            return format!("Error: {e}");
        }
        let folder_id = match (&input.folder, project_id) {
            (Some(name), Some(pid)) => match resolve_folder(&self.db, pid, name) {
                Ok(id) => Some(id),
                Err(e) => return format!("Error: {e}"),
            },
            (Some(_), None) => return "Error: folder requires a project".into(),
            _ => None,
        };
        match self.write(|conn| {
            queries::create_page(
                conn,
                &models::CreatePage {
                    project_id,
                    folder_id,
                    title: input.title.clone(),
                    content: input.content.clone().unwrap_or_default(),
                    status: input.status.clone().unwrap_or_else(|| "draft".into()),
                    labels: input.labels.clone().unwrap_or_default(),
                },
            )
        }) {
            Ok(page) => {
                if let Some(project_id) = page.project_id {
                    self.emit(crate::realtime::RealtimeEvent::ProjectUpdated { project_id });
                }
                format!("Created {}: {}", page.identifier, page.title)
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Update a page by identifier. Only provided fields are changed.")]
    fn update_page(&self, Parameters(input): Parameters<UpdatePageInput>) -> String {
        let (id, project_id) = match self.read(|conn| {
            let id = queries::resolve_page_identifier(conn, &input.identifier)?;
            let project_id = queries::get_page(conn, id)?.project_id;
            Ok((id, project_id))
        }) {
            Ok(v) => v,
            Err(e) => return format!("Error: {e}"),
        };
        if let Err(e) = require_page_role_mcp(&self.db, project_id, models::Role::Maintainer) {
            return format!("Error: {e}");
        }
        match self.write(|conn| {
            // LIF-145 sentinel: field omitted (None) = skip, empty string = clear
            // (move page to root), non-empty = resolve folder + set.
            let folder_id = match &input.folder {
                Some(name) if name.is_empty() => Some(None),
                Some(name) => {
                    let page = queries::get_page(conn, id)?;
                    let pid = page.project_id.ok_or_else(|| {
                        crate::error::LificError::BadRequest(
                            "page has no project for folder resolution".into(),
                        )
                    })?;
                    Some(Some(queries::resolve_folder_name(conn, pid, name)?))
                }
                None => None,
            };
            queries::update_page(
                conn,
                id,
                &models::UpdatePage {
                    title: input.title.clone(),
                    content: input.content.clone(),
                    folder_id,
                    sort_order: None,
                    status: input.status.clone(),
                    pinned: input.pinned,
                    labels: input.labels.clone(),
                },
            )
        }) {
            Ok(page) => {
                if let Some(project_id) = page.project_id {
                    self.emit(crate::realtime::RealtimeEvent::ProjectUpdated { project_id });
                }
                format!("Updated {}: {}", page.identifier, page.title)
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(
        description = "Edit a page by exact string replacement; same contract as edit_issue. Targets the content by default; pass field='title' for the title."
    )]
    fn edit_page(&self, Parameters(input): Parameters<EditPageInput>) -> String {
        let (id, project_id) = match self.read(|conn| {
            let id = queries::resolve_page_identifier(conn, &input.identifier)?;
            let project_id = queries::get_page(conn, id)?.project_id;
            Ok((id, project_id))
        }) {
            Ok(v) => v,
            Err(e) => return format!("Error: {e}"),
        };
        if let Err(e) = require_page_role_mcp(&self.db, project_id, models::Role::Maintainer) {
            return format!("Error: {e}");
        }
        match self.write(|conn| {
            let page = queries::get_page(conn, id)?;

            let field = input.field.as_deref().unwrap_or("content");
            // Mirror update_page's proper-JSON heuristic on content so an
            // edit from a double-escaping client matches stored content.
            let (current, old_norm, new_norm) = match field {
                "content" => (
                    page.content.clone(),
                    queries::unescape_text(&input.old_string),
                    queries::unescape_text(&input.new_string),
                ),
                "title" => (
                    page.title.clone(),
                    input.old_string.clone(),
                    input.new_string.clone(),
                ),
                other => {
                    return Err(crate::error::LificError::BadRequest(format!(
                        "invalid field '{other}'; expected 'content' or 'title'"
                    )));
                }
            };

            let updated = apply_edit(
                &current,
                &old_norm,
                &new_norm,
                input.replace_all.unwrap_or(false),
            )?;

            let mut patch = models::UpdatePage {
                title: None,
                content: None,
                folder_id: None,
                sort_order: None,
                status: None,
                pinned: None,
                labels: None,
            };
            match field {
                "title" => patch.title = Some(updated),
                "content" => patch.content = Some(updated),
                _ => unreachable!(),
            }

            queries::update_page(conn, id, &patch)
        }) {
            Ok(page) => {
                if let Some(project_id) = page.project_id {
                    self.emit(crate::realtime::RealtimeEvent::ProjectUpdated { project_id });
                }
                format!("Edited {}: {}", page.identifier, page.title)
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(
        description = "Delete any resource by type and identifier. Types: issue, page, plan, project, module, label, folder."
    )]
    fn delete(&self, Parameters(input): Parameters<DeleteInput>) -> String {
        match input.resource_type.as_str() {
            "issue" => {
                let issue = match self.read(|conn| {
                    let id = queries::resolve_identifier(conn, &input.identifier)?;
                    queries::get_issue(conn, id)
                }) {
                    Ok(v) => v,
                    Err(e) => return format!("Error: {e}"),
                };
                if let Err(e) =
                    require_role_mcp(&self.db, issue.project_id, models::Role::Maintainer)
                {
                    return format!("Error: {e}");
                }
                match self.write(|conn| queries::delete_issue(conn, issue.id)) {
                    Ok(()) => {
                        self.emit(crate::realtime::RealtimeEvent::IssueDeleted {
                            project_id: issue.project_id,
                            issue_id: issue.id,
                        });
                        format!("Deleted issue {}", input.identifier)
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }
            "plan" => {
                let (id, project_id) = match self.read(|conn| {
                    let id = queries::plans::resolve_plan_identifier(conn, &input.identifier)?;
                    let project_id = queries::plans::get_plan(conn, id)?.project_id;
                    Ok((id, project_id))
                }) {
                    Ok(v) => v,
                    Err(e) => return format!("Error: {e}"),
                };
                if let Err(e) = require_role_mcp(&self.db, project_id, models::Role::Maintainer) {
                    return format!("Error: {e}");
                }
                match self.write(|conn| queries::plans::delete_plan(conn, id)) {
                    Ok(()) => {
                        self.emit(crate::realtime::RealtimeEvent::ProjectUpdated { project_id });
                        format!("Deleted plan {}", input.identifier)
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }
            "page" => {
                let (id, project_id) = match self.read(|conn| {
                    let id = queries::resolve_page_identifier(conn, &input.identifier)?;
                    let project_id = queries::get_page(conn, id)?.project_id;
                    Ok((id, project_id))
                }) {
                    Ok(v) => v,
                    Err(e) => return format!("Error: {e}"),
                };
                if let Err(e) =
                    require_page_role_mcp(&self.db, project_id, models::Role::Maintainer)
                {
                    return format!("Error: {e}");
                }
                match self.write(|conn| queries::delete_page(conn, id)) {
                    Ok(()) => {
                        if let Some(project_id) = project_id {
                            self.emit(crate::realtime::RealtimeEvent::ProjectUpdated {
                                project_id,
                            });
                        }
                        format!("Deleted page {}", input.identifier)
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }
            "project" => {
                let id = match self
                    .read(|conn| queries::resolve_project_identifier(conn, &input.identifier))
                {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                if let Err(e) = require_project_delete_role_mcp(&self.db, id) {
                    return format!("Error: {e}");
                }
                match self.write(|conn| queries::delete_project_with_audience(conn, id)) {
                    Ok((project, audience)) => {
                        let event = crate::realtime::RealtimeEvent::ProjectDeleted {
                            project_id: project.id,
                        };
                        match audience {
                            Some(user_ids) => self.realtime.send_to_users(event, user_ids),
                            None => self.emit(event),
                        }
                        format!("Deleted project {}", input.identifier)
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }
            "module" | "label" | "folder" => {
                let Some(ref proj) = input.project else {
                    return format!(
                        "Error: project required to delete {} by name",
                        input.resource_type
                    );
                };
                let pid = match resolve_project(&self.db, proj) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                if let Err(e) = require_structure_role_mcp(&self.db, pid) {
                    return format!("Error: {e}");
                }
                let result = match input.resource_type.as_str() {
                    "module" => self.write(|conn| {
                        let id = queries::resolve_module_name(conn, pid, &input.identifier)?;
                        queries::delete_module(conn, id)
                    }),
                    "label" => self.write(|conn| {
                        let id = queries::resolve_label_name(conn, pid, &input.identifier)?;
                        queries::delete_label(conn, id)
                    }),
                    "folder" => self.write(|conn| {
                        let id = queries::resolve_folder_name(conn, pid, &input.identifier)?;
                        queries::delete_folder(conn, id)
                    }),
                    _ => unreachable!(),
                };
                match result {
                    Ok(()) => {
                        self.emit(crate::realtime::RealtimeEvent::ProjectUpdated {
                            project_id: pid,
                        });
                        format!("Deleted {} '{}'", input.resource_type, input.identifier)
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }
            other => format!(
                "Unknown type '{other}'. Use issue, page, plan, project, module, label, or folder."
            ),
        }
    }

    #[tool(
        description = "List resources by type: project, module, label, folder, page, issue, or plan. Most types need a project identifier."
    )]
    fn list_resources(&self, Parameters(input): Parameters<ListResourcesInput>) -> String {
        match input.resource_type.as_str() {
            "plan" => {
                let Some(ref proj) = input.project else {
                    return "Error: project required".into();
                };
                let pid = match resolve_project(&self.db, proj) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                if let Err(e) = require_role_mcp(&self.db, pid, models::Role::Viewer) {
                    return format!("Error: {e}");
                }
                let limit = input.limit.unwrap_or(50).clamp(1, 500);
                let offset = input.offset.unwrap_or(0).max(0);
                match self.read(|conn| {
                    queries::plans::list_plans(
                        conn,
                        &models::ListPlansQuery {
                            project_id: Some(pid),
                            status: input.status.clone(),
                            limit: Some(limit),
                            offset: Some(offset),
                            ..Default::default()
                        },
                    )
                }) {
                    Ok(plans) if plans.is_empty() => "No plans found.".into(),
                    Ok(plans) => {
                        let mut out = format!("{} plans:\n", plans.len());
                        for p in &plans {
                            let anchor = p
                                .anchor_identifier
                                .as_ref()
                                .map(|a| format!(" — anchor {a}"))
                                .unwrap_or_default();
                            out.push_str(&format!(
                                "- {} | {} | {} ({}/{} done){}\n",
                                p.identifier, p.status, p.title, p.done_count, p.step_count, anchor
                            ));
                        }
                        out
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }
            // Cross-project list (LIF-198 scope item 2): filter, don't deny.
            "project" => {
                // LIF-257: a genuinely empty DB gets the onboarding nudge.
                // Checked before the visibility filter so "projects exist but
                // none visible to this user" keeps its "0 projects" output.
                if let Some(nudge) = self.no_projects_nudge() {
                    return nudge;
                }
                let visible = match visible_project_ids_mcp(&self.db) {
                    Ok(v) => v,
                    Err(e) => return format!("Error: {e}"),
                };
                match self.read(|conn| {
                    Ok((
                        queries::list_projects(conn)?,
                        queries::project_agent_stats(conn)?,
                    ))
                }) {
                    Ok((ps, stats)) => {
                        let mut ps = filter_visible(ps, &visible, |p| Some(p.id));
                        ps.sort_by(|left, right| cmp_projects_by_activity(left, right, &stats));
                        let mut out = format!("{} projects:\n", ps.len());
                        let now = Utc::now();
                        for p in &ps {
                            out.push_str(&format!("- {} | {}", p.identifier, p.name));
                            if !p.description.is_empty() {
                                out.push_str(&format!(" — {}", p.description));
                            }
                            if let Some(suffix) = stats
                                .get(&p.id)
                                .and_then(|stats| fmt_project_agent_stats(stats, now))
                            {
                                out.push_str(&suffix);
                            }
                            out.push('\n');
                        }
                        out
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }
            "issue" => {
                let Some(ref proj) = input.project else {
                    return "Error: project required".into();
                };
                let pid = match resolve_project(&self.db, proj) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                if let Err(e) = require_role_mcp(&self.db, pid, models::Role::Viewer) {
                    return format!("Error: {e}");
                }
                let limit = input.limit.unwrap_or(100).max(1);
                let offset = input.offset.unwrap_or(0).max(0);
                match self.read(|conn| {
                    queries::list_issues(
                        conn,
                        &models::ListIssuesQuery {
                            project_id: Some(pid),
                            limit: Some(limit + 1),
                            offset: Some(offset),
                            ..Default::default()
                        },
                    )
                }) {
                    Ok(mut issues) => {
                        let has_more = issues.len() as i64 > limit;
                        if has_more {
                            issues.truncate(limit as usize);
                        }
                        let mut out =
                            format!("{} issues (use list_issues for filtering):\n", issues.len());
                        for i in &issues {
                            out.push_str(&format!(
                                "- {} | {} | {}\n",
                                i.identifier, i.status, i.title
                            ));
                        }
                        append_pagination_hint(&mut out, has_more, offset + limit);
                        out
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }
            "page" => {
                let project_id = match &input.project {
                    Some(p) => match resolve_project(&self.db, p) {
                        Ok(id) => Some(id),
                        Err(e) => return format!("Error: {e}"),
                    },
                    None => None,
                };
                // Project-scoped: Viewer gate. Cross-project (no `project`
                // filter): filter by visibility instead of denying (LIF-198
                // scope item 2) — a workspace page (project_id None) is
                // excluded for any non-admin once enforcement is on.
                let visible = if let Some(pid) = project_id {
                    if let Err(e) = require_role_mcp(&self.db, pid, models::Role::Viewer) {
                        return format!("Error: {e}");
                    }
                    None
                } else {
                    match visible_project_ids_mcp(&self.db) {
                        Ok(v) => v,
                        Err(e) => return format!("Error: {e}"),
                    }
                };
                let folder_id = match (&input.folder, project_id) {
                    (Some(name), Some(pid)) => match resolve_folder(&self.db, pid, name) {
                        Ok(id) => Some(id),
                        Err(e) => return format!("Error: {e}"),
                    },
                    _ => None,
                };
                let label = input.label.as_deref();
                let status = input.status.as_deref();
                let order_by = input.order_by.as_deref();
                let order = input.order.as_deref();
                let limit = input.limit.unwrap_or(100).max(1);
                let offset = input.offset.unwrap_or(0).max(0);
                match self.read(|conn| {
                    // Over-fetch by one to detect truncation (mirrors the
                    // issue branch above).
                    let pages = queries::list_pages(
                        conn,
                        project_id,
                        folder_id,
                        label,
                        status,
                        order_by,
                        order,
                        Some(limit + 1),
                        Some(offset),
                    )?;
                    // One folders round-trip for the whole listing — folders
                    // are project-scoped, so workspace pages never have one.
                    let folder_names: std::collections::HashMap<i64, String> = match project_id {
                        Some(pid) => queries::list_folders(conn, pid)?
                            .into_iter()
                            .map(|f| (f.id, f.name))
                            .collect(),
                        None => std::collections::HashMap::new(),
                    };
                    Ok((pages, folder_names))
                }) {
                    Ok((pages, folder_names)) => {
                        let mut pages = filter_visible(pages, &visible, |p| p.project_id);
                        if pages.is_empty() {
                            return "No pages found.".into();
                        }
                        // Over-fetched by one: detect truncation, then trim
                        // back to the requested page size before rendering.
                        let has_more = pages.len() as i64 > limit;
                        if has_more {
                            pages.truncate(limit as usize);
                        }
                        let mut out = format!("{} pages:\n", pages.len());
                        for p in &pages {
                            // Mirror `fmt_issue`: only suffix the label
                            // bracket when there's something to show, so
                            // unlabeled pages stay terse.
                            let labels = if p.labels.is_empty() {
                                String::new()
                            } else {
                                format!(" [{}]", p.labels.join(", "))
                            };
                            let folder = p
                                .folder_id
                                .and_then(|fid| folder_names.get(&fid))
                                .map(|name| format!(" (folder: {name})"))
                                .unwrap_or_default();
                            // Timestamps are "YYYY-MM-DD HH:MM:SS"; the date
                            // part keeps list lines scannable. Full stamps
                            // live on get_page.
                            let updated = p.updated_at.split(' ').next().unwrap_or(&p.updated_at);
                            let pin = if p.pinned { "📌 " } else { "" };
                            out.push_str(&format!(
                                "- {}{} | {} | {}{}{} — updated {}\n",
                                pin, p.identifier, p.status, p.title, labels, folder, updated
                            ));
                        }
                        append_pagination_hint(&mut out, has_more, offset + limit);
                        out
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }
            "module" => {
                let Some(ref proj) = input.project else {
                    return "Error: project required".into();
                };
                let pid = match resolve_project(&self.db, proj) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                if let Err(e) = require_role_mcp(&self.db, pid, models::Role::Viewer) {
                    return format!("Error: {e}");
                }
                match self.read(|conn| queries::list_modules(conn, pid)) {
                    Ok(ms) => {
                        let mut out = format!("{} modules:\n", ms.len());
                        for m in &ms {
                            out.push_str(&format!("- {} ({})", m.name, m.status));
                            if !m.description.is_empty() {
                                out.push_str(&format!(" — {}", m.description));
                            }
                            out.push('\n');
                        }
                        out
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }
            "label" => {
                let Some(ref proj) = input.project else {
                    return "Error: project required".into();
                };
                let pid = match resolve_project(&self.db, proj) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                if let Err(e) = require_role_mcp(&self.db, pid, models::Role::Viewer) {
                    return format!("Error: {e}");
                }
                match self.read(|conn| queries::list_labels(conn, pid)) {
                    Ok(ls) => {
                        let mut out = format!("{} labels:\n", ls.len());
                        for l in &ls {
                            out.push_str(&format!("- {} ({})\n", l.name, l.color));
                        }
                        out
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }
            "folder" => {
                let Some(ref proj) = input.project else {
                    return "Error: project required".into();
                };
                let pid = match resolve_project(&self.db, proj) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                if let Err(e) = require_role_mcp(&self.db, pid, models::Role::Viewer) {
                    return format!("Error: {e}");
                }
                match self.read(|conn| queries::list_folders(conn, pid)) {
                    Ok(fs) => {
                        let mut out = format!("{} folders:\n", fs.len());
                        for f in &fs {
                            out.push_str(&format!("- [{}] {}\n", f.id, f.name));
                        }
                        out
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }
            other => format!(
                "Unknown type '{other}'. Use project, module, label, folder, page, issue, or plan."
            ),
        }
    }

    #[tool(
        description = "Create or update a project, module, label, or folder. Create project requires name and identifier; project update requires project=<IDENT>; module/label/folder create requires project and name; module/label/folder update requires project and current_name. Use delete for deletion."
    )]
    fn manage_resource(&self, Parameters(input): Parameters<ManageResourceInput>) -> String {
        match (input.resource_type.as_str(), input.action.as_str()) {
            ("project", "create") => {
                let Some(ref name) = input.name else {
                    return "Error: name required".into();
                };
                let Some(ref ident) = input.identifier else {
                    return "Error: identifier required".into();
                };
                // LIF-102: default the project lead to the MCP caller so the
                // project isn't left unowned. Validate the user actually exists
                // in this DB before assigning — MCP_REQUEST_USER is a process-
                // global static and can hold stale state from prior sessions.
                let lead_user_id = super::current_auth_user().and_then(|u| {
                    self.read(|conn| {
                        Ok(conn
                            .query_row(
                                "SELECT 1 FROM users WHERE id = ?1",
                                rusqlite::params![u.id],
                                |_| Ok(()),
                            )
                            .is_ok())
                    })
                    .ok()
                    .filter(|exists| *exists)
                    .map(|_| u.id)
                });
                match self.write(|conn| {
                    queries::create_project(
                        conn,
                        &models::CreateProject {
                            name: name.clone(),
                            identifier: ident.clone(),
                            description: input.description.clone().unwrap_or_default(),
                            emoji: emoji_for_create(&input.emoji),
                            lead_user_id,
                        },
                    )
                }) {
                    Ok(p) => {
                        self.emit(crate::realtime::RealtimeEvent::ProjectCreated {
                            project_id: p.id,
                        });
                        format!("Created project {} | {}", p.identifier, p.name)
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }
            ("project", "update") => {
                if input.current_name.is_some() && input.project.is_none() {
                    return "Error: project updates must be targeted with project=<identifier>, not current_name"
                        .into();
                }
                let Some(ref proj) = input.project else {
                    return "Error: project identifier required".into();
                };
                let pid = match resolve_project(&self.db, proj) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                if let Err(e) = require_role_mcp(&self.db, pid, models::Role::Lead) {
                    return format!("Error: {e}");
                }
                match self.write(|conn| {
                    queries::update_project(
                        conn,
                        pid,
                        &models::UpdateProject {
                            name: input.name.clone(),
                            identifier: input.identifier.clone(),
                            description: input.description.clone(),
                            emoji: emoji_for_update(&input.emoji),
                            lead_user_id: None,
                        },
                    )
                }) {
                    Ok(p) => {
                        self.emit(crate::realtime::RealtimeEvent::ProjectUpdated {
                            project_id: p.id,
                        });
                        format!("Updated project {} | {}", p.identifier, p.name)
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }
            ("module", "create") => {
                let Some(ref proj) = input.project else {
                    return "Error: project required".into();
                };
                let pid = match resolve_project(&self.db, proj) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                if let Err(e) = require_structure_role_mcp(&self.db, pid) {
                    return format!("Error: {e}");
                }
                let Some(ref name) = input.name else {
                    return "Error: name required".into();
                };
                match self.write(|conn| {
                    queries::create_module(
                        conn,
                        &models::CreateModule {
                            project_id: pid,
                            name: name.clone(),
                            description: input.description.clone().unwrap_or_default(),
                            status: input.status.clone().unwrap_or("active".into()),
                            emoji: emoji_for_create(&input.emoji),
                        },
                    )
                }) {
                    Ok(m) => {
                        self.emit(crate::realtime::RealtimeEvent::ProjectUpdated {
                            project_id: pid,
                        });
                        format!("Created module [{}]: {}", m.id, m.name)
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }
            ("module", "update") => {
                let Some(ref proj) = input.project else {
                    return "Error: project required".into();
                };
                let pid = match resolve_project(&self.db, proj) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                if let Err(e) = require_structure_role_mcp(&self.db, pid) {
                    return format!("Error: {e}");
                }
                let Some(ref current) = input.current_name else {
                    return "Error: current_name required to identify module".into();
                };
                let mid = match resolve_module(&self.db, pid, current) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                match self.write(|conn| {
                    queries::update_module(
                        conn,
                        mid,
                        &models::UpdateModule {
                            name: input.name.clone(),
                            description: input.description.clone(),
                            status: input.status.clone(),
                            emoji: emoji_for_update(&input.emoji),
                        },
                    )
                }) {
                    Ok(m) => {
                        self.emit(crate::realtime::RealtimeEvent::ProjectUpdated {
                            project_id: pid,
                        });
                        format!("Updated module: {}", m.name)
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }
            ("label", "create") => {
                let Some(ref proj) = input.project else {
                    return "Error: project required".into();
                };
                let pid = match resolve_project(&self.db, proj) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                if let Err(e) = require_structure_role_mcp(&self.db, pid) {
                    return format!("Error: {e}");
                }
                let Some(ref name) = input.name else {
                    return "Error: name required".into();
                };
                match self.write(|conn| {
                    queries::create_label(
                        conn,
                        &models::CreateLabel {
                            project_id: pid,
                            name: name.clone(),
                            color: input.color.clone().unwrap_or("#6B7280".into()),
                        },
                    )
                }) {
                    Ok(l) => {
                        self.emit(crate::realtime::RealtimeEvent::ProjectUpdated {
                            project_id: pid,
                        });
                        format!("Created label: {} ({})", l.name, l.color)
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }
            ("label", "update") => {
                let Some(ref proj) = input.project else {
                    return "Error: project required".into();
                };
                let pid = match resolve_project(&self.db, proj) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                if let Err(e) = require_structure_role_mcp(&self.db, pid) {
                    return format!("Error: {e}");
                }
                let Some(ref current) = input.current_name else {
                    return "Error: current_name required to identify label".into();
                };
                let lid = match self.read(|conn| queries::resolve_label_name(conn, pid, current)) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                match self.write(|conn| {
                    queries::update_label(
                        conn,
                        lid,
                        &models::UpdateLabel {
                            name: input.name.clone(),
                            color: input.color.clone(),
                        },
                    )
                }) {
                    Ok(l) => {
                        self.emit(crate::realtime::RealtimeEvent::ProjectUpdated {
                            project_id: pid,
                        });
                        format!("Updated label: {} ({})", l.name, l.color)
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }
            ("folder", "create") => {
                let Some(ref proj) = input.project else {
                    return "Error: project required".into();
                };
                let pid = match resolve_project(&self.db, proj) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                if let Err(e) = require_structure_role_mcp(&self.db, pid) {
                    return format!("Error: {e}");
                }
                let Some(ref name) = input.name else {
                    return "Error: name required".into();
                };
                match self.write(|conn| {
                    queries::create_folder(
                        conn,
                        &models::CreateFolder {
                            project_id: pid,
                            parent_id: None,
                            name: name.clone(),
                        },
                    )
                }) {
                    Ok(f) => {
                        self.emit(crate::realtime::RealtimeEvent::ProjectUpdated {
                            project_id: pid,
                        });
                        format!("Created folder [{}]: {}", f.id, f.name)
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }
            ("folder", "update") => {
                let Some(ref proj) = input.project else {
                    return "Error: project required".into();
                };
                let pid = match resolve_project(&self.db, proj) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                if let Err(e) = require_structure_role_mcp(&self.db, pid) {
                    return format!("Error: {e}");
                }
                let Some(ref current) = input.current_name else {
                    return "Error: current_name required to identify folder".into();
                };
                let fid = match self.read(|conn| queries::resolve_folder_name(conn, pid, current)) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                match self.write(|conn| {
                    queries::update_folder(
                        conn,
                        fid,
                        &models::UpdateFolder {
                            name: input.name.clone(),
                        },
                    )
                }) {
                    Ok(f) => {
                        self.emit(crate::realtime::RealtimeEvent::ProjectUpdated {
                            project_id: pid,
                        });
                        format!("Updated folder: {}", f.name)
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }
            (rt, act) => format!(
                "Unsupported: {rt}/{act}. Types: project, module, label, folder. Actions: create, update."
            ),
        }
    }

    #[tool(
        description = "Add a comment to an issue (LIF-42) or page (LIF-DOC-3; DOC-3 for workspace pages). The author is the authenticated user."
    )]
    fn add_comment(&self, Parameters(input): Parameters<AddCommentInput>) -> String {
        let parent = match resolve_comment_parent(self, &input.identifier) {
            Ok(p) => p,
            Err(e) => return format!("Error: {e}"),
        };
        if let Err(e) = self.require_comment_role_mcp(parent, models::Role::Viewer) {
            return format!("Error: {e}");
        }

        // Resolve the authenticated user from the task-local set by the HTTP handler.
        // For stdio/local MCP sessions (no HTTP auth), fall back to the first admin user.
        let user_id = match super::current_auth_user() {
            Some(u) => u.id,
            None => match self.read(queries::users::first_admin) {
                Ok(Some(admin)) => admin.id,
                Ok(None) => {
                    return "Error: no admin user exists to attribute comments to.".into();
                }
                Err(e) => return format!("Error: {e}"),
            },
        };

        // LIF-263: resolve the parent's project + the enforcement flag up
        // front (read connections), then record @mentions in the same write
        // that creates the comment. Same visible-member rules as the REST
        // path — only tokens matching a visible member resolve.
        let project_id: Option<i64> = match parent {
            queries::comments::CommentParent::Issue(id) => {
                match self.read(|conn| queries::get_issue(conn, id)) {
                    Ok(i) => Some(i.project_id),
                    Err(e) => return format!("Error: {e}"),
                }
            }
            queries::comments::CommentParent::Page(id) => {
                match self.read(|conn| queries::get_page(conn, id)) {
                    Ok(p) => p.project_id,
                    Err(e) => return format!("Error: {e}"),
                }
            }
        };
        let member_scoped = match crate::authz::authz_enforced(&self.db) {
            Ok(v) => v,
            Err(e) => return format!("Error: {e}"),
        };

        match self.write(|conn| {
            let candidates =
                queries::comments::mention_candidates(conn, project_id, member_scoped)?;
            let c = queries::comments::create_comment(conn, parent, user_id, &input.content)?;
            queries::comments::sync_mentions(conn, c.id, &c.content, &candidates)?;
            let event = match (c.issue_id, project_id) {
                (Some(issue_id), Some(project_id)) => {
                    Some(crate::realtime::RealtimeEvent::IssueUpdated {
                        project_id,
                        issue_id,
                    })
                }
                (None, Some(project_id)) => {
                    Some(crate::realtime::RealtimeEvent::ProjectUpdated { project_id })
                }
                (_, None) => None,
            };
            Ok((c, event))
        }) {
            // Don't echo c.content back — the agent already supplied it in the
            // tool args, so repeating it just duplicates tokens in context
            // (LIF-115). The comment id is the useful new handle for any
            // follow-up edit/delete.
            Ok((c, event)) => {
                if let Some(event) = event {
                    self.emit(event);
                }
                format!(
                    "Comment #{} added to {} by {} at {}",
                    c.id, input.identifier, c.author, c.created_at
                )
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(
        description = "List comments on an issue (LIF-42) or page (LIF-DOC-3; DOC-3 for workspace pages)."
    )]
    fn list_comments(&self, Parameters(input): Parameters<ListCommentsInput>) -> String {
        let parent = match resolve_comment_parent(self, &input.identifier) {
            Ok(p) => p,
            Err(e) => return format!("Error: {e}"),
        };
        if let Err(e) = self.require_comment_role_mcp(parent, models::Role::Viewer) {
            return format!("Error: {e}");
        }

        let limit = input.limit.map(|limit| limit.clamp(1, 500));
        let offset = input.offset.unwrap_or(0).max(0);
        let order = input.order.as_deref().unwrap_or("asc");

        match self.read(|conn| {
            let comments = queries::comments::list_comments_paginated(
                conn,
                parent,
                input.author.as_deref(),
                Some(order),
                limit.map(|limit| limit + 1),
                input.offset.map(|offset| offset.max(0)),
            )?;
            let total = queries::comments::count_comments(conn, parent, input.author.as_deref())?;
            Ok((comments, total))
        }) {
            Ok((comments, total)) if comments.is_empty() => {
                if total == 0 {
                    format!("No comments on {}.", input.identifier)
                } else {
                    format!(
                        "No comments in this range on {} (offset {offset}, {total} total).",
                        input.identifier
                    )
                }
            }
            Ok((mut comments, total)) => {
                let page_limit = limit.filter(|&limit| comments.len() as i64 > limit);
                let has_more = page_limit.is_some();
                if let Some(limit) = page_limit {
                    comments.truncate(limit as usize);
                }
                let shown = comments.len() as i64;
                let mut out = if limit.is_some() && offset == 0 && shown < total {
                    let edge = if order == "desc" {
                        "most recent"
                    } else {
                        "oldest"
                    };
                    format!(
                        "Showing {shown} {edge} of {total} comment(s) on {}:\n",
                        input.identifier
                    )
                } else if offset > 0 {
                    format!(
                        "Showing comments {}-{} of {total} on {} ({} first):\n",
                        offset + 1,
                        offset + shown,
                        input.identifier,
                        if order == "desc" { "newest" } else { "oldest" }
                    )
                } else {
                    format!("{total} comment(s) on {}:\n", input.identifier)
                };
                for c in &comments {
                    out.push_str(&format!(
                        "[{}] {} ({}): {}\n",
                        c.created_at, c.author, c.author_display_name, c.content
                    ));
                }
                if has_more {
                    let next_offset = offset + shown;
                    let remaining = total.saturating_sub(next_offset);
                    out.push_str(&format!(
                        "\n... {remaining} more comment(s) — call again with the same author/order/limit and offset={next_offset}\n"
                    ));
                }
                out
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(
        description = "Edit a comment's content by id. Author or admin only; @mentions re-resolve."
    )]
    fn edit_comment(&self, Parameters(input): Parameters<EditCommentInput>) -> String {
        // Resolve the acting user the same way add_comment does: the
        // task-local HTTP-auth user, else fall back to the first admin for
        // stdio/local sessions.
        let (user_id, is_admin) = match self.resolve_comment_actor() {
            Ok(u) => u,
            Err(e) => return format!("Error: {e}"),
        };

        // Ownership: only the author or an admin may edit (mirrors
        // api::comments::update_comment_handler).
        let existing =
            match self.read(|conn| queries::comments::get_comment(conn, input.comment_id)) {
                Ok(c) => c,
                Err(e) => return format!("Error: {e}"),
            };
        if existing.user_id != user_id && !is_admin {
            return "Error: you can only edit your own comments".into();
        }

        // LIF-263: recompute the mention set against the parent project's
        // visible members, resolving the project from the comment's parent.
        let project_id = match self.read(|conn| resolve_comment_project(conn, &existing)) {
            Ok(p) => p,
            Err(e) => return format!("Error: {e}"),
        };
        let member_scoped = match crate::authz::authz_enforced(&self.db) {
            Ok(v) => v,
            Err(e) => return format!("Error: {e}"),
        };

        match self.write(|conn| {
            let candidates =
                queries::comments::mention_candidates(conn, project_id, member_scoped)?;
            let c = queries::comments::update_comment(conn, input.comment_id, &input.content)?;
            queries::comments::sync_mentions(conn, c.id, &c.content, &candidates)?;
            Ok(c)
        }) {
            // Don't echo the new content back — the agent already supplied it
            // (LIF-115). The id is the stable handle.
            Ok(c) => {
                if let (Some(issue_id), Some(project_id)) = (c.issue_id, project_id) {
                    self.emit(crate::realtime::RealtimeEvent::IssueUpdated {
                        project_id,
                        issue_id,
                    });
                } else if let Some(project_id) = project_id {
                    self.emit(crate::realtime::RealtimeEvent::ProjectUpdated { project_id });
                }
                format!("Comment #{} edited at {}", c.id, c.updated_at)
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(
        description = "Delete a comment by id. Author or admin only."
    )]
    fn delete_comment(&self, Parameters(input): Parameters<DeleteCommentInput>) -> String {
        // Resolve the acting user the same way add_comment does.
        let (user_id, is_admin) = match self.resolve_comment_actor() {
            Ok(u) => u,
            Err(e) => return format!("Error: {e}"),
        };

        // Ownership: only the author or an admin may delete (mirrors
        // api::comments::delete_comment_handler).
        let existing =
            match self.read(|conn| queries::comments::get_comment(conn, input.comment_id)) {
                Ok(c) => c,
                Err(e) => return format!("Error: {e}"),
            };
        if existing.user_id != user_id && !is_admin {
            return "Error: you can only delete your own comments".into();
        }

        let project_id = match self.read(|conn| resolve_comment_project(conn, &existing)) {
            Ok(project_id) => project_id,
            Err(e) => return format!("Error: {e}"),
        };

        match self.write(|conn| queries::comments::delete_comment(conn, input.comment_id)) {
            Ok(()) => {
                if let (Some(issue_id), Some(project_id)) = (existing.issue_id, project_id) {
                    self.emit(crate::realtime::RealtimeEvent::IssueUpdated {
                        project_id,
                        issue_id,
                    });
                } else if let Some(project_id) = project_id {
                    self.emit(crate::realtime::RealtimeEvent::ProjectUpdated { project_id });
                }
                format!("Comment #{} deleted", input.comment_id)
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(
        description = "Create a nestable step-by-step plan that survives outside the context window. Steps can mirror issues via 'issue': closing the issue completes the step and vice versa."
    )]
    fn create_plan(&self, Parameters(input): Parameters<CreatePlanInput>) -> String {
        let pid = match resolve_project(&self.db, &input.project) {
            Ok(id) => id,
            Err(e) => return format!("Error: {e}"),
        };
        if let Err(e) = require_role_mcp(&self.db, pid, models::Role::Maintainer) {
            return format!("Error: {e}");
        }
        match self.write(|conn| {
            let anchor = match &input.anchor_issue {
                Some(ident) => Some(queries::resolve_identifier(conn, ident)?),
                None => None,
            };
            let mut steps = Vec::new();
            if let Some(input_steps) = &input.steps {
                for s in input_steps {
                    steps.push(build_create_step(conn, s)?);
                }
            }
            queries::plans::create_plan(
                conn,
                &models::CreatePlan {
                    project_id: pid,
                    title: input.title.clone(),
                    issue_id: anchor,
                    steps,
                },
            )
        }) {
            Ok(plan) => {
                self.emit(crate::realtime::RealtimeEvent::ProjectUpdated {
                    project_id: plan.project_id,
                });
                format!("Created {}\n{}", plan.identifier, fmt_plan(&plan))
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(
        description = "Rehydrate a plan's full step tree (e.g. LIF-PLAN-3) when resuming work. Step lines show the #id used by edit_plan_step and update_plan_step, done state, and linked issues."
    )]
    fn get_plan(&self, Parameters(input): Parameters<GetPlanInput>) -> String {
        match self.read(|conn| {
            let id = queries::plans::resolve_plan_identifier(conn, &input.plan)?;
            queries::plans::get_plan(conn, id)
        }) {
            Ok(plan) => {
                if let Err(e) = require_role_mcp(&self.db, plan.project_id, models::Role::Viewer) {
                    return format!("Error: {e}");
                }
                fmt_plan(&plan)
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(
        description = "Edit a plan step's text by exact string replacement; same contract as edit_issue. Targets description by default; pass field='title'."
    )]
    fn edit_plan_step(&self, Parameters(input): Parameters<EditPlanStepInput>) -> String {
        let field = input.field.clone().unwrap_or_else(|| "description".into());
        let plan_project_id = match self.read(|conn| {
            let plan_id = queries::plans::resolve_plan_identifier(conn, &input.plan)?;
            Ok(queries::plans::get_plan(conn, plan_id)?.project_id)
        }) {
            Ok(pid) => pid,
            Err(e) => return format!("Error: {e}"),
        };
        if let Err(e) = require_role_mcp(&self.db, plan_project_id, models::Role::Maintainer) {
            return format!("Error: {e}");
        }
        match self.write(|conn| {
            let plan_id = queries::plans::resolve_plan_identifier(conn, &input.plan)?;
            queries::plans::assert_step_in_plan(conn, plan_id, input.step_id)?;
            let (find, replace) = if field == "description" {
                (
                    queries::unescape_text(&input.old_string),
                    queries::unescape_text(&input.new_string),
                )
            } else {
                (input.old_string.clone(), input.new_string.clone())
            };
            queries::plans::edit_step_text(
                conn,
                input.step_id,
                &field,
                &find,
                &replace,
                input.replace_all.unwrap_or(false),
            )?;
            queries::plans::get_plan(conn, plan_id)
        }) {
            Ok(plan) => {
                self.emit(crate::realtime::RealtimeEvent::ProjectUpdated {
                    project_id: plan.project_id,
                });
                format!(
                    "Edited step #{} in {}\n{}",
                    input.step_id,
                    plan.identifier,
                    fmt_plan(&plan)
                )
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(
        description = "Mutate a plan or one step. With step_id: step CRUD, done toggling, attach/detach issue (linked issues sync state). Without step_id: update the plan itself; plan status never closes the anchor issue. Returns a delta."
    )]
    fn update_plan_step(&self, Parameters(input): Parameters<UpdatePlanStepInput>) -> String {
        // LIF-198: Maintainer on the plan's own project gates every mutation
        // below (plan-level and step-level alike).
        let plan_project_id = match self.read(|conn| {
            let plan_id = queries::plans::resolve_plan_identifier(conn, &input.plan)?;
            Ok(queries::plans::get_plan(conn, plan_id)?.project_id)
        }) {
            Ok(pid) => pid,
            Err(e) => return format!("Error: {e}"),
        };
        if let Err(e) = require_role_mcp(&self.db, plan_project_id, models::Role::Maintainer) {
            return format!("Error: {e}");
        }
        // Cross-project step↔issue edges need Maintainer on the *issue's*
        // project too, mirroring `link_issues`' both-sides check: attaching
        // an issue to a step, adding a child step pre-linked to an issue,
        // and toggling `done` when the step already references an issue
        // that could live in a different project than the plan.
        if let Some(ref ident) = input.attach_issue
            && let Err(e) = self.require_issue_ident_role_mcp(ident, models::Role::Maintainer)
        {
            return format!("Error: {e}");
        }
        if let Some(ref ident) = input.add_child_issue
            && let Err(e) = self.require_issue_ident_role_mcp(ident, models::Role::Maintainer)
        {
            return format!("Error: {e}");
        }
        if input.done == Some(true)
            && let Some(step_id) = input.step_id
            && let Err(e) = self.require_step_issue_role_mcp(step_id, models::Role::Maintainer)
        {
            return format!("Error: {e}");
        }

        match self.write(|conn| {
            let plan_id = queries::plans::resolve_plan_identifier(conn, &input.plan)?;
            let mut notes: Vec<String> = Vec::new();
            let mut events = Vec::new();

            match input.step_id {
                // ── Plan-level update ──
                None => {
                    let anchor = match (&input.anchor_issue, input.clear_anchor) {
                        (Some(ident), _) => Some(Some(queries::resolve_identifier(conn, ident)?)),
                        (None, Some(true)) => Some(None),
                        _ => None,
                    };
                    queries::plans::update_plan(
                        conn,
                        plan_id,
                        &models::UpdatePlan {
                            title: input.title.clone(),
                            status: input.status.clone(),
                            issue_id: anchor,
                        },
                    )?;
                    notes.push("Updated plan".into());
                }
                // ── Step-level update ──
                Some(step_id) => {
                    queries::plans::assert_step_in_plan(conn, plan_id, step_id)?;
                    if input.delete.unwrap_or(false) {
                        queries::plans::delete_step(conn, step_id)?;
                        notes.push(format!("Deleted step #{step_id} (and its subtree)"));
                    } else {
                        if let Some(ref t) = input.title {
                            queries::plans::set_step_title(conn, step_id, t)?;
                            notes.push(format!("Renamed step #{step_id}"));
                        }
                        if let Some(ref ident) = input.attach_issue {
                            let iid = queries::resolve_identifier(conn, ident)?;
                            queries::plans::set_step_issue(conn, step_id, Some(iid))?;
                            notes.push(format!("Attached {ident} to step #{step_id}"));
                        }
                        if input.detach_issue.unwrap_or(false) {
                            queries::plans::set_step_issue(conn, step_id, None)?;
                            notes.push(format!("Detached issue from step #{step_id}"));
                        }
                        if let Some(done) = input.done {
                            let effect = queries::plans::set_step_done(conn, step_id, done)?;
                            let mut msg = format!(
                                "Step #{step_id} {}",
                                if done { "marked done" } else { "reopened" }
                            );
                            if let Some(iss) = &effect.issue_identifier {
                                if effect.issue_status_changed {
                                    let issue_id = queries::resolve_identifier(conn, iss)?;
                                    let issue = queries::get_issue(conn, issue_id)?;
                                    events.push(crate::realtime::RealtimeEvent::IssueUpdated {
                                        project_id: issue.project_id,
                                        issue_id: issue.id,
                                    });
                                    msg.push_str(&format!(" → {iss} marked done"));
                                } else if done {
                                    msg.push_str(&format!(" (linked {iss} already done)"));
                                }
                            }
                            notes.push(msg);
                        }
                        if let Some(ref child_title) = input.add_child_title {
                            let child_issue = match &input.add_child_issue {
                                Some(ident) => Some(queries::resolve_identifier(conn, ident)?),
                                None => None,
                            };
                            let child_id = queries::plans::add_step(
                                conn,
                                plan_id,
                                Some(step_id),
                                child_title,
                                input.add_child_description.as_deref().unwrap_or(""),
                                child_issue,
                            )?;
                            notes.push(format!("Added child step #{child_id}"));
                        }
                        if input.move_to_root.unwrap_or(false)
                            || input.move_parent_step_id.is_some()
                            || input.move_position.is_some()
                        {
                            let new_parent = if input.move_to_root.unwrap_or(false) {
                                None
                            } else if let Some(p) = input.move_parent_step_id {
                                Some(p)
                            } else {
                                queries::plans::step_parent(conn, step_id)?
                            };
                            queries::plans::move_step(
                                conn,
                                step_id,
                                new_parent,
                                input.move_position,
                            )?;
                            notes.push(format!("Moved step #{step_id}"));
                        }
                        if notes.is_empty() {
                            notes.push("No changes specified".into());
                        }
                    }
                }
            }

            let plan = queries::plans::get_plan(conn, plan_id)?;
            Ok((notes, plan, events))
        }) {
            Ok((notes, plan, events)) => {
                events.into_iter().for_each(|event| self.emit(event));
                self.emit(crate::realtime::RealtimeEvent::ProjectUpdated {
                    project_id: plan.project_id,
                });
                // LIF-302: default to a compact receipt (notes + a one-line
                // progress summary built from the same Plan fields as
                // fmt_plan's header, minus the title and step tree). Pass
                // echo_tree=true to restore the full re-rendered tree.
                if input.echo_tree.unwrap_or(false) {
                    format!("{}\n{}", notes.join("; "), fmt_plan(&plan))
                } else {
                    format!(
                        "{}\n{} [{}]: {}/{} done",
                        notes.join("; "),
                        plan.identifier,
                        plan.status,
                        plan.done_count,
                        plan.step_count
                    )
                }
            }
            Err(e) => format!("Error: {e}"),
        }
    }
}

/// Recursively resolve an MCP PlanStepInput (issue identifiers → ids) into a
/// query-layer CreatePlanStep, so create_plan can author the whole tree in one
/// transaction.
fn build_create_step(
    conn: &rusqlite::Connection,
    s: &PlanStepInput,
) -> Result<models::CreatePlanStep, crate::error::LificError> {
    let issue_id = match &s.issue {
        Some(ident) => Some(queries::resolve_identifier(conn, ident)?),
        None => None,
    };
    let mut steps = Vec::new();
    if let Some(children) = &s.steps {
        for c in children {
            steps.push(build_create_step(conn, c)?);
        }
    }
    Ok(models::CreatePlanStep {
        title: s.title.clone(),
        description: s.description.clone().unwrap_or_default(),
        issue_id,
        done: s.done.unwrap_or(false),
        steps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::handler::server::wrapper::Parameters;

    fn mcp() -> LificMcp {
        let db = crate::db::open_memory().expect("test db");
        LificMcp::new(db)
    }

    fn mcp_with_realtime() -> (
        LificMcp,
        tokio::sync::broadcast::Receiver<crate::realtime::RealtimeMessage>,
    ) {
        let db = crate::db::open_memory().expect("test db");
        let realtime = crate::realtime::RealtimeHub::new();
        let rx = realtime.subscribe();
        (LificMcp::with_realtime(db, realtime), rx)
    }

    fn drain_realtime(rx: &mut tokio::sync::broadcast::Receiver<crate::realtime::RealtimeMessage>) {
        while rx.try_recv().is_ok() {}
    }

    fn realtime_events(
        rx: &mut tokio::sync::broadcast::Receiver<crate::realtime::RealtimeMessage>,
    ) -> Vec<crate::realtime::RealtimeEvent> {
        std::iter::from_fn(|| rx.try_recv().ok().map(|message| message.event)).collect()
    }

    /// Seed a project via manage_resource, return identifier.
    fn seed_project(mcp: &LificMcp, name: &str, ident: &str) -> String {
        let result = mcp.manage_resource(Parameters(ManageResourceInput {
            resource_type: "project".into(),
            action: "create".into(),
            name: Some(name.into()),
            identifier: Some(ident.into()),
            description: None,
            project: None,
            current_name: None,
            status: None,
            color: None,
            emoji: None,
        }));
        assert!(result.starts_with("Created project"), "got: {result}");
        ident.to_string()
    }

    fn seed_issue(mcp: &LificMcp, project: &str, title: &str) -> String {
        let result = mcp.create_issue(Parameters(CreateIssueInput {
            project: project.into(),
            title: title.into(),
            description: None,
            status: None,
            priority: None,
            module: None,
            labels: None,
            ..Default::default()
        }));
        assert!(result.starts_with("Created"), "got: {result}");
        result
    }

    fn project_id_for(mcp: &LificMcp, identifier: &str) -> i64 {
        mcp.read(|conn| queries::resolve_project_identifier(conn, identifier))
            .expect("project id")
    }

    fn issue_id_for(mcp: &LificMcp, identifier: &str) -> i64 {
        mcp.read(|conn| queries::resolve_identifier(conn, identifier))
            .expect("issue id")
    }

    /// LIF-198: MCP-side mirror of `api::test_helpers::setup_membership_test`
    /// — reuses the exact same fixture (DB with `authz_enforced` ON, an
    /// admin, and a project with a lead/maintainer/viewer member plus a
    /// non-member) so the flag-ON test matrix stays byte-identical to
    /// REST's (LIF-197), just wrapped as an `LificMcp` + `AuthUser`s ready
    /// for `with_request_user` instead of an axum `Router` + `Extension`.
    pub(super) fn setup_membership_mcp() -> (
        LificMcp,
        models::AuthUser,
        models::AuthUser,
        models::AuthUser,
        models::AuthUser,
        models::AuthUser,
        i64,
    ) {
        let (db, admin, lead, maintainer, viewer, non_member, project_id) =
            crate::api::test_helpers::setup_membership_test();
        let mcp = LificMcp::new(db);
        let au = |u: crate::db::models::User| models::AuthUser {
            id: u.id,
            username: u.username,
            display_name: u.display_name,
            is_admin: u.is_admin,
        };
        (
            mcp,
            au(admin),
            au(lead),
            au(maintainer),
            au(viewer),
            au(non_member),
            project_id,
        )
    }

    // ── manage_resource ──

    #[test]
    fn manage_create_project() {
        let m = mcp();
        let result = seed_project(&m, "Alpha", "ALP");
        assert_eq!(result, "ALP");
    }

    #[test]
    fn manage_update_project() {
        let m = mcp();
        seed_project(&m, "Old", "UPD");
        let result = m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "project".into(),
            action: "update".into(),
            project: Some("UPD".into()),
            name: Some("New Name".into()),
            identifier: None,
            description: None,
            current_name: Some("Old".into()),
            status: None,
            color: None,
            emoji: None,
        }));
        assert!(result.contains("New Name"), "got: {result}");
    }

    #[test]
    fn manage_update_project_description_persists() {
        let m = mcp();
        seed_project(&m, "Project", "DSC");
        let project_id = project_id_for(&m, "DSC");

        let result = m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "project".into(),
            action: "update".into(),
            project: Some("DSC".into()),
            name: None,
            identifier: None,
            description: Some("Updated description".into()),
            current_name: None,
            status: None,
            color: None,
            emoji: None,
        }));

        assert!(result.contains("Updated project"), "got: {result}");
        let project = m
            .read(|conn| queries::get_project(conn, project_id))
            .expect("updated project");
        assert_eq!(project.description, "Updated description");
    }

    #[test]
    fn manage_update_project_identifier_persists() {
        let m = mcp();
        seed_project(&m, "Project", "OLD");
        let project_id = project_id_for(&m, "OLD");

        let result = m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "project".into(),
            action: "update".into(),
            project: Some("OLD".into()),
            name: None,
            identifier: Some("NEW".into()),
            description: None,
            current_name: None,
            status: None,
            color: None,
            emoji: None,
        }));

        assert!(result.contains("Updated project NEW"), "got: {result}");
        let project = m
            .read(|conn| queries::get_project(conn, project_id))
            .expect("updated project");
        assert_eq!(project.identifier, "NEW");
        assert_eq!(project_id_for(&m, "NEW"), project_id);
    }

    #[test]
    fn manage_update_project_with_current_name_requires_project_identifier() {
        let m = mcp();
        seed_project(&m, "Original", "ORG");
        let project_id = project_id_for(&m, "ORG");

        let result = m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "project".into(),
            action: "update".into(),
            project: None,
            name: Some("Changed".into()),
            identifier: Some("NEW".into()),
            description: Some("Changed description".into()),
            current_name: Some("Original".into()),
            status: None,
            color: None,
            emoji: None,
        }));

        assert_eq!(
            result,
            "Error: project updates must be targeted with project=<identifier>, not current_name"
        );
        let project = m
            .read(|conn| queries::get_project(conn, project_id))
            .expect("unchanged project");
        assert_eq!(project.name, "Original");
        assert_eq!(project.identifier, "ORG");
        assert_eq!(project.description, "");
    }

    #[test]
    fn manage_create_module() {
        let m = mcp();
        seed_project(&m, "Test", "MOD");
        let result = m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "module".into(),
            action: "create".into(),
            project: Some("MOD".into()),
            name: Some("Backend".into()),
            description: Some("Server-side logic".into()),
            identifier: None,
            current_name: None,
            status: None,
            color: None,
            emoji: None,
        }));
        assert!(result.contains("Backend"), "got: {result}");
    }

    #[test]
    fn manage_create_label() {
        let m = mcp();
        seed_project(&m, "Test", "LBL");
        let result = m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "label".into(),
            action: "create".into(),
            project: Some("LBL".into()),
            name: Some("bug".into()),
            color: Some("#EF4444".into()),
            identifier: None,
            description: None,
            current_name: None,
            status: None,
            emoji: None,
        }));
        assert!(result.contains("bug"), "got: {result}");
        assert!(result.contains("#EF4444"), "got: {result}");
    }

    #[test]
    fn manage_create_folder() {
        let m = mcp();
        seed_project(&m, "Test", "FLD");
        let result = m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "folder".into(),
            action: "create".into(),
            project: Some("FLD".into()),
            name: Some("Docs".into()),
            identifier: None,
            description: None,
            current_name: None,
            status: None,
            color: None,
            emoji: None,
        }));
        assert!(result.contains("Docs"), "got: {result}");
    }

    #[test]
    fn manage_missing_name_errors() {
        let m = mcp();
        let result = m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "project".into(),
            action: "create".into(),
            name: None,
            identifier: Some("X".into()),
            description: None,
            project: None,
            current_name: None,
            status: None,
            color: None,
            emoji: None,
        }));
        assert!(result.contains("name required"), "got: {result}");
    }

    #[test]
    fn manage_unknown_type() {
        let m = mcp();
        let result = m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "widget".into(),
            action: "create".into(),
            name: None,
            identifier: None,
            description: None,
            project: None,
            current_name: None,
            status: None,
            color: None,
            emoji: None,
        }));
        assert!(result.contains("Unsupported"), "got: {result}");
    }

    // ── create / get / update / delete issue ──

    #[test]
    fn issue_create_and_get() {
        let m = mcp();
        seed_project(&m, "Test", "TST");
        let created = seed_issue(&m, "TST", "First issue");
        assert!(created.contains("TST-1"), "got: {created}");

        let detail = m.get_issue(Parameters(GetIssueInput {
            identifier: "TST-1".into(),
            ..Default::default()
        }));
        assert!(detail.contains("First issue"), "got: {detail}");
        assert!(detail.contains("backlog"), "got: {detail}");
    }

    #[test]
    fn issue_create_with_options() {
        let m = mcp();
        seed_project(&m, "Test", "OPT");
        m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "label".into(),
            action: "create".into(),
            project: Some("OPT".into()),
            name: Some("feature".into()),
            color: None,
            identifier: None,
            description: None,
            current_name: None,
            status: None,
            emoji: None,
        }));
        let result = m.create_issue(Parameters(CreateIssueInput {
            project: "OPT".into(),
            title: "Detailed issue".into(),
            description: Some("Some markdown".into()),
            status: Some("todo".into()),
            priority: Some("high".into()),
            module: None,
            labels: Some(vec!["feature".into()]),
            ..Default::default()
        }));
        assert!(result.contains("OPT-1"), "got: {result}");

        let detail = m.get_issue(Parameters(GetIssueInput {
            identifier: "OPT-1".into(),
            ..Default::default()
        }));
        assert!(detail.contains("high"), "got: {detail}");
        assert!(detail.contains("todo"), "got: {detail}");
        assert!(detail.contains("feature"), "got: {detail}");
        assert!(detail.contains("Some markdown"), "got: {detail}");
    }

    #[test]
    fn issue_update() {
        let m = mcp();
        seed_project(&m, "Test", "UPI");
        seed_issue(&m, "UPI", "Original");

        let result = m.update_issue(Parameters(UpdateIssueInput {
            identifier: "UPI-1".into(),
            title: Some("Renamed".into()),
            status: Some("active".into()),
            priority: Some("urgent".into()),
            description: None,
            module: None,
            labels: None,
            ..Default::default()
        }));
        assert!(result.contains("Renamed"), "got: {result}");
        assert!(result.contains("active"), "got: {result}");
        assert!(result.contains("urgent"), "got: {result}");
    }

    #[test]
    fn update_issue_without_linked_plan_steps_has_plain_response() {
        let m = mcp();
        seed_project(&m, "Test", "PLN");
        seed_issue(&m, "PLN", "Standalone");

        let result = m.update_issue(Parameters(UpdateIssueInput {
            identifier: "PLN-1".into(),
            status: Some("done".into()),
            ..Default::default()
        }));

        assert_eq!(
            result, "Updated PLN-1: PLN-1 | done | none | Standalone",
            "unlinked issue must not invent a cascade note"
        );
    }

    // LIF-144: start_date/target_date are settable through the MCP layer.

    #[test]
    fn create_issue_persists_target_date() {
        let m = mcp();
        seed_project(&m, "Test", "SCH");

        let result = m.create_issue(Parameters(CreateIssueInput {
            project: "SCH".into(),
            title: "Scheduled".into(),
            target_date: Some("2026-06-15".into()),
            ..Default::default()
        }));
        assert!(result.contains("SCH-1"), "got: {result}");

        let conn = m.db.read().unwrap();
        let id = queries::resolve_identifier(&conn, "SCH-1").unwrap();
        let issue = queries::get_issue(&conn, id).unwrap();
        assert_eq!(issue.target_date.as_deref(), Some("2026-06-15"));
        assert_eq!(issue.start_date, None);
    }

    #[test]
    fn update_issue_sets_start_date() {
        let m = mcp();
        seed_project(&m, "Test", "STD");
        seed_issue(&m, "STD", "Original");

        let result = m.update_issue(Parameters(UpdateIssueInput {
            identifier: "STD-1".into(),
            start_date: Some("2026-06-01".into()),
            ..Default::default()
        }));
        assert!(result.contains("STD-1"), "got: {result}");

        let conn = m.db.read().unwrap();
        let id = queries::resolve_identifier(&conn, "STD-1").unwrap();
        let issue = queries::get_issue(&conn, id).unwrap();
        assert_eq!(issue.start_date.as_deref(), Some("2026-06-01"));
    }

    #[test]
    fn update_issue_omitting_dates_leaves_them_unchanged() {
        let m = mcp();
        seed_project(&m, "Test", "UNC");

        m.create_issue(Parameters(CreateIssueInput {
            project: "UNC".into(),
            title: "Prescheduled".into(),
            start_date: Some("2026-01-01".into()),
            target_date: Some("2026-02-01".into()),
            ..Default::default()
        }));

        // Update an unrelated field; omit both dates.
        m.update_issue(Parameters(UpdateIssueInput {
            identifier: "UNC-1".into(),
            title: Some("Renamed".into()),
            ..Default::default()
        }));

        let conn = m.db.read().unwrap();
        let id = queries::resolve_identifier(&conn, "UNC-1").unwrap();
        let issue = queries::get_issue(&conn, id).unwrap();
        assert_eq!(issue.title, "Renamed");
        assert_eq!(issue.start_date.as_deref(), Some("2026-01-01"));
        assert_eq!(issue.target_date.as_deref(), Some("2026-02-01"));
    }

    // ── bulk_update (LIF-24) ──

    #[test]
    fn bulk_update_sets_status_on_module_matches_only() {
        let m = mcp();
        seed_project(&m, "Bulk", "BLK");
        // Module to target.
        m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "module".into(),
            action: "create".into(),
            project: Some("BLK".into()),
            name: Some("Backend".into()),
            identifier: None,
            current_name: None,
            description: None,
            status: None,
            color: None,
            ..Default::default()
        }));
        // Two active issues in the Backend module (should be updated).
        for title in ["In-module A", "In-module B"] {
            m.create_issue(Parameters(CreateIssueInput {
                project: "BLK".into(),
                title: title.into(),
                status: Some("active".into()),
                priority: None,
                description: None,
                module: Some("Backend".into()),
                labels: None,
                ..Default::default()
            }));
        }
        // One active issue with NO module (outside the filter).
        m.create_issue(Parameters(CreateIssueInput {
            project: "BLK".into(),
            title: "Loose one".into(),
            status: Some("active".into()),
            priority: None,
            description: None,
            module: None,
            labels: None,
            ..Default::default()
        }));

        let result = m.bulk_update(Parameters(BulkUpdateInput {
            project: "BLK".into(),
            filter_status: Some("active".into()),
            filter_module: Some("Backend".into()),
            set_status: Some("done".into()),
            ..Default::default()
        }));
        assert_eq!(result, "Updated 2 issue(s)", "got: {result}");

        // The two in-module issues are now done.
        for id in ["BLK-1", "BLK-2"] {
            let got = m.get_issue(Parameters(GetIssueInput {
                identifier: id.into(),
                ..Default::default()
            }));
            assert!(got.contains("Status: done"), "{id} not done: {got}");
        }
        // The out-of-filter issue is untouched (still active).
        let loose = m.get_issue(Parameters(GetIssueInput {
            identifier: "BLK-3".into(),
            ..Default::default()
        }));
        assert!(loose.contains("Status: active"), "loose changed: {loose}");
    }

    #[test]
    fn bulk_update_emits_issue_updates() {
        let (m, mut rx) = mcp_with_realtime();
        seed_project(&m, "Bulk Events", "BLE");
        seed_issue(&m, "BLE", "One");
        seed_issue(&m, "BLE", "Two");
        let project_id = project_id_for(&m, "BLE");
        let first_issue_id = issue_id_for(&m, "BLE-1");
        let second_issue_id = issue_id_for(&m, "BLE-2");
        drain_realtime(&mut rx);

        let result = m.bulk_update(Parameters(BulkUpdateInput {
            project: "BLE".into(),
            set_status: Some("done".into()),
            ..Default::default()
        }));

        assert_eq!(result, "Updated 2 issue(s)", "got: {result}");
        assert_eq!(
            realtime_events(&mut rx),
            vec![
                crate::realtime::RealtimeEvent::IssueUpdated {
                    project_id,
                    issue_id: first_issue_id,
                },
                crate::realtime::RealtimeEvent::IssueUpdated {
                    project_id,
                    issue_id: second_issue_id,
                },
            ]
        );
    }

    #[test]
    fn issue_delete() {
        let m = mcp();
        seed_project(&m, "Test", "DEL");
        seed_issue(&m, "DEL", "Doomed");

        let result = m.delete(Parameters(DeleteInput {
            resource_type: "issue".into(),
            identifier: "DEL-1".into(),
            project: None,
        }));
        assert!(result.contains("Deleted issue"), "got: {result}");

        // Verify gone
        let get = m.get_issue(Parameters(GetIssueInput {
            identifier: "DEL-1".into(),
            ..Default::default()
        }));
        assert!(get.starts_with("Error"), "got: {get}");
    }

    #[test]
    fn get_nonexistent_issue_errors() {
        let m = mcp();
        let result = m.get_issue(Parameters(GetIssueInput {
            identifier: "NOPE-999".into(),
            ..Default::default()
        }));
        assert!(result.starts_with("Error"), "got: {result}");
    }

    // ── list_issues ──

    #[test]
    fn list_issues_with_filters() {
        let m = mcp();
        seed_project(&m, "Test", "LST");

        m.create_issue(Parameters(CreateIssueInput {
            project: "LST".into(),
            title: "Todo one".into(),
            status: Some("todo".into()),
            priority: Some("high".into()),
            description: None,
            module: None,
            labels: None,
            ..Default::default()
        }));
        m.create_issue(Parameters(CreateIssueInput {
            project: "LST".into(),
            title: "Active one".into(),
            status: Some("active".into()),
            priority: Some("low".into()),
            description: None,
            module: None,
            labels: None,
            ..Default::default()
        }));

        // Filter by status
        let result = m.list_issues(Parameters(ListIssuesInput {
            project: "LST".into(),
            status: Some("todo".into()),
            priority: None,
            module: None,
            label: None,
            workable: None,
            limit: None,
            offset: None,
            ..Default::default()
        }));
        assert!(result.contains("1 issues"), "got: {result}");
        assert!(result.contains("Todo one"), "got: {result}");
    }

    #[test]
    fn list_issues_empty() {
        let m = mcp();
        seed_project(&m, "Empty", "EMP");
        let result = m.list_issues(Parameters(ListIssuesInput {
            project: "EMP".into(),
            status: None,
            priority: None,
            module: None,
            label: None,
            workable: None,
            limit: None,
            offset: None,
            ..Default::default()
        }));
        assert_eq!(result, "No issues found.");
    }

    #[test]
    fn list_issues_bad_project_errors() {
        let m = mcp();
        // A project must exist, else the LIF-257 onboarding nudge fires
        // before the (bad) project identifier is ever resolved.
        seed_project(&m, "Alpha", "AAA");
        let result = m.list_issues(Parameters(ListIssuesInput {
            project: "NOPE".into(),
            status: None,
            priority: None,
            module: None,
            label: None,
            workable: None,
            limit: None,
            offset: None,
            ..Default::default()
        }));
        assert!(result.starts_with("Error"), "got: {result}");
    }

    #[test]
    fn list_issues_pagination_emits_has_more_hint() {
        let m = mcp();
        seed_project(&m, "Pages", "PAG");
        // Seed 5 issues; ask for 2 — should report has_more with offset=2.
        for i in 0..5 {
            seed_issue(&m, "PAG", &format!("Issue {i}"));
        }
        let page1 = m.list_issues(Parameters(ListIssuesInput {
            project: "PAG".into(),
            status: None,
            priority: None,
            module: None,
            label: None,
            workable: None,
            limit: Some(2),
            offset: None,
            ..Default::default()
        }));
        assert!(page1.contains("2 issues"), "got: {page1}");
        assert!(
            page1.contains("offset=2"),
            "expected has_more hint, got: {page1}"
        );

        // Page 2: offset=2, limit=2 — still more.
        let page2 = m.list_issues(Parameters(ListIssuesInput {
            project: "PAG".into(),
            status: None,
            priority: None,
            module: None,
            label: None,
            workable: None,
            limit: Some(2),
            offset: Some(2),
            ..Default::default()
        }));
        assert!(page2.contains("2 issues"), "got: {page2}");
        assert!(
            page2.contains("offset=4"),
            "expected has_more hint, got: {page2}"
        );

        // Page 3: offset=4, limit=2 — only 1 remaining, no hint.
        let page3 = m.list_issues(Parameters(ListIssuesInput {
            project: "PAG".into(),
            status: None,
            priority: None,
            module: None,
            label: None,
            workable: None,
            limit: Some(2),
            offset: Some(4),
            ..Default::default()
        }));
        assert!(page3.contains("1 issues"), "got: {page3}");
        assert!(
            !page3.contains("more results available"),
            "should NOT have hint on last page, got: {page3}"
        );
    }

    #[test]
    fn list_issues_no_hint_when_under_limit() {
        let m = mcp();
        seed_project(&m, "Small", "SML");
        seed_issue(&m, "SML", "Only one");
        let result = m.list_issues(Parameters(ListIssuesInput {
            project: "SML".into(),
            status: None,
            priority: None,
            module: None,
            label: None,
            workable: None,
            limit: Some(10),
            offset: None,
            ..Default::default()
        }));
        assert!(result.contains("1 issues"), "got: {result}");
        assert!(!result.contains("more results available"), "got: {result}");
    }

    // ── link / unlink ──

    #[test]
    fn link_and_unlink_issues() {
        let m = mcp();
        seed_project(&m, "Test", "LNK");
        seed_issue(&m, "LNK", "Blocker");
        seed_issue(&m, "LNK", "Blocked");

        let result = m.link_issues(Parameters(LinkIssuesInput {
            source: "LNK-1".into(),
            target: "LNK-2".into(),
            relation_type: "blocks".into(),
        }));
        assert!(result.contains("blocks"), "got: {result}");

        // Verify relation shows in get_issue, now annotated with the related
        // issue's status (LIF-303). LNK-2 is a fresh issue → backlog.
        let detail = m.get_issue(Parameters(GetIssueInput {
            identifier: "LNK-1".into(),
            ..Default::default()
        }));
        assert!(detail.contains("Blocks: LNK-2 (backlog)"), "got: {detail}");

        let result = m.unlink_issues(Parameters(UnlinkIssuesInput {
            source: "LNK-1".into(),
            target: "LNK-2".into(),
        }));
        assert!(result.contains("Unlinked"), "got: {result}");
    }

    // LIF-303: get_issue annotates relation identifiers with the related
    // issue's current status; the annotation reflects each blocker's own
    // status, not a shared one.
    #[test]
    fn get_issue_relations_carry_status() {
        let m = mcp();
        seed_project(&m, "Rel", "REL");
        seed_issue(&m, "REL", "target"); // REL-1
        seed_issue(&m, "REL", "blocker-a"); // REL-2
        seed_issue(&m, "REL", "blocker-b"); // REL-3

        // REL-2 and REL-3 both block REL-1; give them distinct statuses.
        m.link_issues(Parameters(LinkIssuesInput {
            source: "REL-2".into(),
            target: "REL-1".into(),
            relation_type: "blocks".into(),
        }));
        m.link_issues(Parameters(LinkIssuesInput {
            source: "REL-3".into(),
            target: "REL-1".into(),
            relation_type: "blocks".into(),
        }));
        m.update_issue(Parameters(UpdateIssueInput {
            identifier: "REL-2".into(),
            status: Some("done".into()),
            ..Default::default()
        }));

        let detail = m.get_issue(Parameters(GetIssueInput {
            identifier: "REL-1".into(),
            ..Default::default()
        }));
        // Each blocker carries its own status.
        assert!(detail.contains("REL-2 (done)"), "got: {detail}");
        assert!(detail.contains("REL-3 (backlog)"), "got: {detail}");
        assert!(detail.contains("Blocked by:"), "got: {detail}");
    }

    #[test]
    fn list_issues_blocked_filter_surfaces_blocked_by() {
        let m = mcp();
        seed_project(&m, "Test", "BLK");
        seed_issue(&m, "BLK", "Blocker"); // BLK-1
        seed_issue(&m, "BLK", "Blocked"); // BLK-2
        m.link_issues(Parameters(LinkIssuesInput {
            source: "BLK-1".into(),
            target: "BLK-2".into(),
            relation_type: "blocks".into(),
        }));

        let result = m.list_issues(Parameters(ListIssuesInput {
            project: "BLK".into(),
            blocked: Some(true),
            ..Default::default()
        }));
        // Only the blocked issue is returned, and its blocker identifier is
        // rendered inline as blocked_by:BLK-1.
        assert!(result.contains("1 issues"), "got: {result}");
        assert!(result.contains("Blocked"), "got: {result}");
        assert!(result.contains("blocked_by:BLK-1"), "got: {result}");
    }

    // ── board ──

    #[test]
    fn board_groups_by_status() {
        let m = mcp();
        seed_project(&m, "Test", "BRD");
        m.create_issue(Parameters(CreateIssueInput {
            project: "BRD".into(),
            title: "A".into(),
            status: Some("todo".into()),
            description: None,
            priority: None,
            module: None,
            labels: None,
            ..Default::default()
        }));
        m.create_issue(Parameters(CreateIssueInput {
            project: "BRD".into(),
            title: "B".into(),
            status: Some("active".into()),
            description: None,
            priority: None,
            module: None,
            labels: None,
            ..Default::default()
        }));

        let result = m.get_board(Parameters(GetBoardInput {
            project: "BRD".into(),
            group_by: None,
            ..Default::default()
        }));
        assert!(result.contains("todo"), "got: {result}");
        assert!(result.contains("active"), "got: {result}");
    }

    // LIF-140: board columns follow workflow order, not alphabetical order.
    #[test]
    fn board_status_columns_in_workflow_order() {
        let m = mcp();
        seed_project(&m, "Board Order", "BRO");
        for status in ["done", "active", "backlog", "todo", "cancelled"] {
            m.create_issue(Parameters(CreateIssueInput {
                project: "BRO".into(),
                title: format!("issue {status}"),
                status: Some(status.into()),
                description: None,
                priority: None,
                module: None,
                labels: None,
                ..Default::default()
            }));
        }

        // include_closed so done/cancelled render as full columns (not
        // count-only stubs); the ordering assertion below still works either
        // way, but this keeps the test focused on ordering, not omission.
        let result = m.get_board(Parameters(GetBoardInput {
            project: "BRO".into(),
            group_by: None,
            include_closed: Some(true),
            ..Default::default()
        }));

        let pos = |s: &str| {
            result
                .find(&format!("── {s} ("))
                .unwrap_or_else(|| panic!("missing column {s}: {result}"))
        };
        assert!(pos("backlog") < pos("todo"), "got: {result}");
        assert!(pos("todo") < pos("active"), "got: {result}");
        assert!(pos("active") < pos("done"), "got: {result}");
        assert!(pos("done") < pos("cancelled"), "got: {result}");
    }

    #[test]
    fn board_priority_columns_in_severity_order() {
        let m = mcp();
        seed_project(&m, "Board Prio", "BRP");
        for priority in ["none", "medium", "urgent", "low", "high"] {
            m.create_issue(Parameters(CreateIssueInput {
                project: "BRP".into(),
                title: format!("issue {priority}"),
                status: None,
                description: None,
                priority: Some(priority.into()),
                module: None,
                labels: None,
                ..Default::default()
            }));
        }

        let result = m.get_board(Parameters(GetBoardInput {
            project: "BRP".into(),
            group_by: Some("priority".into()),
            ..Default::default()
        }));

        let pos = |s: &str| {
            result
                .find(&format!("── {s} ("))
                .unwrap_or_else(|| panic!("missing column {s}: {result}"))
        };
        assert!(pos("urgent") < pos("high"), "got: {result}");
        assert!(pos("high") < pos("medium"), "got: {result}");
        assert!(pos("medium") < pos("low"), "got: {result}");
        assert!(pos("low") < pos("none"), "got: {result}");
    }

    // ── board: closed-column omission (LIF-300) ──

    /// Seed a project with a mix of open and closed issues for the LIF-300
    /// omission tests: one todo, one active, two done, one cancelled.
    fn seed_board_mix(m: &LificMcp, ident: &str) {
        seed_project(m, "Closed Board", ident);
        for (title, status) in [
            ("open-todo", "todo"),
            ("open-active", "active"),
            ("shipped-1", "done"),
            ("shipped-2", "done"),
            ("scrapped", "cancelled"),
        ] {
            m.create_issue(Parameters(CreateIssueInput {
                project: ident.into(),
                title: title.into(),
                status: Some(status.into()),
                ..Default::default()
            }));
        }
    }

    #[test]
    fn board_default_omits_closed_contents_but_shows_counts() {
        let m = mcp();
        seed_board_mix(&m, "BCA");
        let result = m.get_board(Parameters(GetBoardInput {
            project: "BCA".into(),
            ..Default::default()
        }));
        // Closed columns keep a header + count but stub out their contents.
        assert!(
            result.contains("── done (2) ── [omitted — pass include_closed=true]"),
            "got: {result}"
        );
        assert!(
            result.contains("── cancelled (1) ── [omitted — pass include_closed=true]"),
            "got: {result}"
        );
        // The closed issue titles must NOT appear.
        assert!(!result.contains("shipped-1"), "got: {result}");
        assert!(!result.contains("scrapped"), "got: {result}");
        // Open columns still list their issues.
        assert!(result.contains("open-todo"), "got: {result}");
        assert!(result.contains("open-active"), "got: {result}");
    }

    #[test]
    fn board_include_closed_shows_closed_issues() {
        let m = mcp();
        seed_board_mix(&m, "BCB");
        let result = m.get_board(Parameters(GetBoardInput {
            project: "BCB".into(),
            include_closed: Some(true),
            ..Default::default()
        }));
        assert!(!result.contains("[omitted"), "got: {result}");
        assert!(result.contains("shipped-1"), "got: {result}");
        assert!(result.contains("shipped-2"), "got: {result}");
        assert!(result.contains("scrapped"), "got: {result}");
    }

    #[test]
    fn board_priority_grouping_excludes_closed_with_trailing_note() {
        let m = mcp();
        seed_board_mix(&m, "BCC");
        let result = m.get_board(Parameters(GetBoardInput {
            project: "BCC".into(),
            group_by: Some("priority".into()),
            ..Default::default()
        }));
        // Closed issues dropped entirely; a trailing note reports the count.
        assert!(!result.contains("shipped-1"), "got: {result}");
        assert!(!result.contains("scrapped"), "got: {result}");
        assert!(
            result.contains("(3 closed issues omitted — pass include_closed=true)"),
            "got: {result}"
        );
        // No status stub in priority grouping.
        assert!(
            !result.contains("[omitted — pass include_closed=true]"),
            "got: {result}"
        );
    }

    #[test]
    fn board_max_per_column_truncates_with_tail() {
        let m = mcp();
        seed_project(&m, "Capped Board", "BCD");
        for i in 0..4 {
            m.create_issue(Parameters(CreateIssueInput {
                project: "BCD".into(),
                title: format!("todo-{i}"),
                status: Some("todo".into()),
                ..Default::default()
            }));
        }
        let result = m.get_board(Parameters(GetBoardInput {
            project: "BCD".into(),
            max_per_column: Some(2),
            ..Default::default()
        }));
        assert!(result.contains("── todo (4) ──"), "got: {result}");
        assert!(
            result.contains("… +2 more (use list_issues)"),
            "got: {result}"
        );
        // Exactly two issue lines rendered.
        let rendered = result.matches("| todo | ").count();
        assert_eq!(rendered, 2, "got: {result}");
    }

    #[test]
    fn board_empty_done_group_produces_no_stub() {
        let m = mcp();
        seed_project(&m, "No Done", "BCE");
        m.create_issue(Parameters(CreateIssueInput {
            project: "BCE".into(),
            title: "just-todo".into(),
            status: Some("todo".into()),
            ..Default::default()
        }));
        let result = m.get_board(Parameters(GetBoardInput {
            project: "BCE".into(),
            ..Default::default()
        }));
        // No done/cancelled issues exist → no stub line at all.
        assert!(!result.contains("done"), "got: {result}");
        assert!(!result.contains("cancelled"), "got: {result}");
        assert!(!result.contains("[omitted"), "got: {result}");
    }

    // ── pages ──

    #[test]
    fn page_create_get_update() {
        let m = mcp();
        seed_project(&m, "Test", "PG");

        let created = m.create_page(Parameters(CreatePageInput {
            project: Some("PG".into()),
            title: "Design Doc".into(),
            content: Some("# Overview\nSome content".into()),
            folder: None,
            status: None,
            labels: None,
        }));
        assert!(created.contains("PG-DOC-1"), "got: {created}");

        let detail = m.get_page(Parameters(GetPageInput {
            identifier: "PG-DOC-1".into(),
        }));
        assert!(detail.contains("Design Doc"), "got: {detail}");
        assert!(detail.contains("# Overview"), "got: {detail}");

        let updated = m.update_page(Parameters(UpdatePageInput {
            identifier: "PG-DOC-1".into(),
            title: Some("Updated Doc".into()),
            content: None,
            folder: None,
            status: None,
            pinned: None,
            labels: None,
        }));
        assert!(updated.contains("Updated Doc"), "got: {updated}");
    }

    #[test]
    fn workspace_page_no_project() {
        let m = mcp();
        let created = m.create_page(Parameters(CreatePageInput {
            project: None,
            title: "Global Note".into(),
            content: None,
            folder: None,
            status: None,
            labels: None,
        }));
        assert!(created.contains("DOC-"), "got: {created}");
    }

    #[test]
    fn page_delete() {
        let m = mcp();
        seed_project(&m, "Test", "PGD");
        m.create_page(Parameters(CreatePageInput {
            project: Some("PGD".into()),
            title: "Temp".into(),
            content: None,
            folder: None,
            status: None,
            labels: None,
        }));
        let result = m.delete(Parameters(DeleteInput {
            resource_type: "page".into(),
            identifier: "PGD-DOC-1".into(),
            project: None,
        }));
        assert!(result.contains("Deleted page"), "got: {result}");
    }

    // ── search ──

    #[test]
    fn search_finds_issue() {
        let m = mcp();
        seed_project(&m, "Test", "SRC");
        seed_issue(&m, "SRC", "Unique searchterm xyz");

        let result = m.search(Parameters(SearchInput {
            query: "searchterm".into(),
            project: None,
            limit: None,
            ..Default::default()
        }));
        assert!(result.contains("1 results"), "got: {result}");
        assert!(result.contains("searchterm"), "got: {result}");
    }

    // LIF-304: literal mode surfaces punctuation-heavy needles FTS tokenizes
    // away, and passes the snippet (with **needle**) through the MCP layer.
    #[test]
    fn mcp_search_literal_mode_finds_punctuation_needle() {
        let m = mcp();
        seed_project(&m, "Test", "SRC");
        seed_issue(&m, "SRC", "wire up core:sodom pipeline");

        let result = m.search(Parameters(SearchInput {
            query: "core:sodom".into(),
            mode: Some("literal".into()),
            ..Default::default()
        }));
        assert!(result.contains("1 results"), "got: {result}");
        assert!(result.contains("SRC-1"), "got: {result}");
        assert!(result.contains("**core:sodom**"), "got: {result}");
    }

    #[test]
    fn mcp_search_invalid_mode_errors() {
        let m = mcp();
        seed_project(&m, "Test", "SRC");
        let result = m.search(Parameters(SearchInput {
            query: "anything".into(),
            mode: Some("regex".into()),
            ..Default::default()
        }));
        assert!(result.contains("invalid mode"), "got: {result}");
    }

    #[test]
    fn search_no_results() {
        let m = mcp();
        // A project must exist, else the LIF-257 onboarding nudge fires
        // before the query is ever run.
        seed_project(&m, "Alpha", "AAA");
        let result = m.search(Parameters(SearchInput {
            query: "nonexistent_gibberish_zzz".into(),
            project: None,
            limit: None,
            ..Default::default()
        }));
        assert_eq!(result, "No results found.");
    }

    // ── list_resources ──

    #[test]
    fn list_resources_projects() {
        let m = mcp();
        seed_project(&m, "Alpha", "AAA");
        seed_project(&m, "Beta", "BBB");

        let result = m.list_resources(Parameters(ListResourcesInput {
            resource_type: "project".into(),
            project: None,
            folder: None,
            label: None,
            limit: None,
            offset: None,
            ..Default::default()
        }));
        assert!(result.contains("2 projects"), "got: {result}");
        assert!(result.contains("AAA"), "got: {result}");
        assert!(result.contains("BBB"), "got: {result}");
    }

    #[test]
    fn list_resources_projects_shows_agent_stats_and_recent_work_first() {
        let m = mcp();
        seed_project(&m, "Stale", "STA");
        seed_project(&m, "Recent", "REC");
        seed_project(&m, "Empty", "EMP");

        let stale_project_id = project_id_for(&m, "STA");
        m.write(|conn| {
            conn.execute(
                "INSERT INTO issues (project_id, sequence, title, status, updated_at)
                 VALUES (?1, 1, 'Old work', 'todo', '2000-01-01 00:00:00')",
                rusqlite::params![stale_project_id],
            )?;
            Ok(())
        })
        .unwrap();
        seed_issue(&m, "REC", "Current work");
        let created = m.create_plan(Parameters(CreatePlanInput {
            project: "REC".into(),
            title: "Current plan".into(),
            anchor_issue: None,
            steps: None,
        }));
        assert!(created.starts_with("Created REC-PLAN-1"), "got: {created}");

        let result = m.list_resources(Parameters(ListResourcesInput {
            resource_type: "project".into(),
            ..Default::default()
        }));
        let recent = result
            .lines()
            .find(|line| line.starts_with("- REC | Recent"))
            .expect("recent project line");
        assert!(
            recent.starts_with("- REC | Recent (1 workable, 1 active plan, last activity "),
            "got: {recent}"
        );
        assert!(recent.ends_with(" ago)"), "got: {recent}");
        assert_eq!(
            result
                .lines()
                .find(|line| line.starts_with("- EMP | Empty")),
            Some("- EMP | Empty"),
            "fresh empty project must have no stats suffix: {result}"
        );
        assert!(
            result.find("- REC | Recent").unwrap() < result.find("- STA | Stale").unwrap(),
            "recent work must sort before stale work: {result}"
        );
    }

    // ── LIF-257: zero-projects onboarding nudge ──

    #[test]
    fn nudge_list_resources_project_on_empty_db() {
        let m = mcp();
        let result = m.list_resources(Parameters(ListResourcesInput {
            resource_type: "project".into(),
            ..Default::default()
        }));
        assert_eq!(result, NO_PROJECTS_NUDGE, "got: {result}");
    }

    #[test]
    fn nudge_list_issues_on_empty_db() {
        let m = mcp();
        // Even with a bogus project filter, an empty DB nudges rather than
        // returning "project not found".
        let result = m.list_issues(Parameters(ListIssuesInput {
            project: "ANY".into(),
            ..Default::default()
        }));
        assert_eq!(result, NO_PROJECTS_NUDGE, "got: {result}");
    }

    #[test]
    fn nudge_search_on_empty_db() {
        let m = mcp();
        let result = m.search(Parameters(SearchInput {
            query: "anything".into(),
            ..Default::default()
        }));
        assert_eq!(result, NO_PROJECTS_NUDGE, "got: {result}");
    }

    #[test]
    fn nudge_get_board_on_empty_db() {
        let m = mcp();
        let result = m.get_board(Parameters(GetBoardInput {
            project: "ANY".into(),
            ..Default::default()
        }));
        assert_eq!(result, NO_PROJECTS_NUDGE, "got: {result}");
    }

    #[test]
    fn no_nudge_once_a_project_exists() {
        let m = mcp();
        seed_project(&m, "Alpha", "AAA");

        let listed = m.list_resources(Parameters(ListResourcesInput {
            resource_type: "project".into(),
            ..Default::default()
        }));
        assert!(!listed.contains(NO_PROJECTS_NUDGE), "got: {listed}");
        assert!(listed.contains("1 projects"), "got: {listed}");

        let issues = m.list_issues(Parameters(ListIssuesInput {
            project: "AAA".into(),
            ..Default::default()
        }));
        assert!(!issues.contains(NO_PROJECTS_NUDGE), "got: {issues}");

        let searched = m.search(Parameters(SearchInput {
            query: "anything".into(),
            ..Default::default()
        }));
        assert!(!searched.contains(NO_PROJECTS_NUDGE), "got: {searched}");

        let board = m.get_board(Parameters(GetBoardInput {
            project: "AAA".into(),
            ..Default::default()
        }));
        assert!(!board.contains(NO_PROJECTS_NUDGE), "got: {board}");
    }

    #[test]
    fn list_resources_requires_project() {
        let m = mcp();
        for rt in ["module", "label", "folder", "issue"] {
            let result = m.list_resources(Parameters(ListResourcesInput {
                resource_type: rt.into(),
                project: None,
                folder: None,
                label: None,
                limit: None,
                offset: None,
                ..Default::default()
            }));
            assert!(result.contains("project required"), "{rt} got: {result}");
        }
    }

    #[test]
    fn list_resources_unknown_type() {
        let m = mcp();
        let result = m.list_resources(Parameters(ListResourcesInput {
            resource_type: "widget".into(),
            project: None,
            folder: None,
            label: None,
            limit: None,
            offset: None,
            ..Default::default()
        }));
        assert!(result.contains("Unknown type"), "got: {result}");
    }

    #[test]
    fn list_resources_issues_pagination() {
        let m = mcp();
        seed_project(&m, "Bulk", "BLK");
        for i in 0..4 {
            seed_issue(&m, "BLK", &format!("Issue {i}"));
        }
        let result = m.list_resources(Parameters(ListResourcesInput {
            resource_type: "issue".into(),
            project: Some("BLK".into()),
            folder: None,
            label: None,
            limit: Some(2),
            offset: None,
            ..Default::default()
        }));
        assert!(result.contains("2 issues"), "got: {result}");
        assert!(result.contains("offset=2"), "got: {result}");
    }

    // ── delete ──

    #[test]
    fn delete_project() {
        let m = mcp();
        seed_project(&m, "Doomed", "DPJ");
        let result = m.delete(Parameters(DeleteInput {
            resource_type: "project".into(),
            identifier: "DPJ".into(),
            project: None,
        }));
        assert!(result.contains("Deleted project"), "got: {result}");
    }

    #[test]
    fn delete_module_requires_project() {
        let m = mcp();
        let result = m.delete(Parameters(DeleteInput {
            resource_type: "module".into(),
            identifier: "Backend".into(),
            project: None,
        }));
        assert!(result.contains("project required"), "got: {result}");
    }

    #[test]
    fn delete_unknown_type() {
        let m = mcp();
        let result = m.delete(Parameters(DeleteInput {
            resource_type: "widget".into(),
            identifier: "x".into(),
            project: None,
        }));
        assert!(result.contains("Unknown type"), "got: {result}");
    }

    // ── manage_resource update label/folder ──

    #[test]
    fn manage_update_label() {
        let m = mcp();
        seed_project(&m, "Test", "UPL");
        m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "label".into(),
            action: "create".into(),
            project: Some("UPL".into()),
            name: Some("bug".into()),
            color: Some("#EF4444".into()),
            identifier: None,
            description: None,
            current_name: None,
            status: None,
            emoji: None,
        }));
        let result = m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "label".into(),
            action: "update".into(),
            project: Some("UPL".into()),
            current_name: Some("bug".into()),
            name: Some("defect".into()),
            color: Some("#FF0000".into()),
            identifier: None,
            description: None,
            status: None,
            emoji: None,
        }));
        assert!(result.contains("defect"), "got: {result}");
        assert!(result.contains("#FF0000"), "got: {result}");
    }

    #[test]
    fn manage_update_folder() {
        let m = mcp();
        seed_project(&m, "Test", "UPF");
        m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "folder".into(),
            action: "create".into(),
            project: Some("UPF".into()),
            name: Some("Docs".into()),
            identifier: None,
            description: None,
            current_name: None,
            status: None,
            color: None,
            emoji: None,
        }));
        let result = m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "folder".into(),
            action: "update".into(),
            project: Some("UPF".into()),
            current_name: Some("Docs".into()),
            name: Some("Documentation".into()),
            identifier: None,
            description: None,
            status: None,
            color: None,
            emoji: None,
        }));
        assert!(result.contains("Documentation"), "got: {result}");
    }

    #[test]
    fn manage_resource_structure_mutations_emit_project_updates() {
        let (m, mut rx) = mcp_with_realtime();
        seed_project(&m, "Structure Events", "STR");
        let project_id = project_id_for(&m, "STR");
        drain_realtime(&mut rx);

        for (resource_type, name, updated_name) in [
            ("module", "Core", "Core v2"),
            ("label", "bug", "defect"),
            ("folder", "Docs", "Documentation"),
        ] {
            let created = m.manage_resource(Parameters(ManageResourceInput {
                resource_type: resource_type.into(),
                action: "create".into(),
                project: Some("STR".into()),
                name: Some(name.into()),
                ..Default::default()
            }));
            assert!(!created.starts_with("Error"), "got: {created}");
            assert_eq!(
                realtime_events(&mut rx),
                vec![crate::realtime::RealtimeEvent::ProjectUpdated { project_id }]
            );

            let updated = m.manage_resource(Parameters(ManageResourceInput {
                resource_type: resource_type.into(),
                action: "update".into(),
                project: Some("STR".into()),
                current_name: Some(name.into()),
                name: Some(updated_name.into()),
                ..Default::default()
            }));
            assert!(!updated.starts_with("Error"), "got: {updated}");
            assert_eq!(
                realtime_events(&mut rx),
                vec![crate::realtime::RealtimeEvent::ProjectUpdated { project_id }]
            );

            let deleted = m.delete(Parameters(DeleteInput {
                resource_type: resource_type.into(),
                identifier: updated_name.into(),
                project: Some("STR".into()),
            }));
            assert!(!deleted.starts_with("Error"), "got: {deleted}");
            assert_eq!(
                realtime_events(&mut rx),
                vec![crate::realtime::RealtimeEvent::ProjectUpdated { project_id }]
            );
        }
    }

    // ── fmt_issue ──

    #[test]
    fn fmt_issue_includes_relations() {
        let issue = models::Issue {
            id: 1,
            project_id: 1,
            sequence: 1,
            identifier: "T-1".into(),
            title: "Test".into(),
            description: String::new(),
            status: "todo".into(),
            priority: "high".into(),
            module_id: None,
            sort_order: 0.0,
            start_date: None,
            target_date: None,
            created_at: String::new(),
            updated_at: String::new(),
            source: None,
            labels: vec!["bug".into()],
            blocks: vec!["T-2".into()],
            blocked_by: vec![],
            relates_to: vec![],
            duplicates: vec!["T-3".into()],
            duplicated_by: vec!["T-4".into()],
        };
        let s = fmt_issue(&issue);
        assert!(s.contains("[bug]"), "got: {s}");
        assert!(s.contains("blocks:T-2"), "got: {s}");
        assert!(s.contains("duplicates:T-3"), "got: {s}");
        assert!(s.contains("duplicated_by:T-4"), "got: {s}");
    }

    // ── comments ──

    /// Set the authenticated user context so `add_comment` works in tests
    /// that call MCP tool methods directly (no request/response cycle).
    ///
    /// `MCP_REQUEST_USER` is a process-wide static (see `mcp::mod`'s doc
    /// comment), so mutating it here races against every OTHER test in the
    /// binary that reads it — including via `LificMcp::write()`'s actor
    /// stamping, which EVERY write-tool test triggers, not just comment
    /// tests. `with_request_user`'s callers (production, and the
    /// `authz_gating_tests` module) are already race-free because they hold
    /// `MCP_HANDLER_LOCK` for the whole scoped call. This helper joins that
    /// same lock (via `blocking_lock`, since ordinary `#[test]` fns aren't
    /// async) and hands the guard back so the caller holds it for its whole
    /// body — `let _guard = seed_user(&m);` — keeping the global stable
    /// until the test's tool calls that depend on it are done.
    fn seed_user(mcp: &LificMcp) -> tokio::sync::MutexGuard<'static, ()> {
        let guard = crate::mcp::MCP_HANDLER_LOCK.blocking_lock();
        let conn = mcp.db.write().unwrap();
        let user = crate::db::queries::users::create_user(
            &conn,
            &models::CreateUser {
                username: "testuser".into(),
                email: "test@test.com".into(),
                password: "testpassword1".into(),
                display_name: Some("Test User".into()),
                is_admin: true,
                is_bot: false,
            },
        )
        .unwrap();
        drop(conn);
        *crate::mcp::MCP_REQUEST_USER
            .lock()
            .unwrap_or_else(|e: std::sync::PoisonError<_>| e.into_inner()) =
            Some(models::AuthUser {
                id: user.id,
                username: user.username.clone(),
                display_name: user.display_name,
                is_admin: user.is_admin,
            });
        guard
    }

    #[test]
    fn add_and_list_comments() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "Test issue");
        let _guard = seed_user(&m);

        let result = m.add_comment(Parameters(AddCommentInput {
            identifier: "PRJ-1".into(),
            content: "Hello from MCP".into(),
        }));
        assert!(result.starts_with("Comment #"), "got: {result}");
        assert!(result.contains("testuser"), "got: {result}");
        // LIF-115: the response must NOT echo the comment body back — the
        // agent already supplied it in the tool args, so repeating it just
        // burns context tokens. The new contract leads with the comment id.
        assert!(
            !result.contains("Hello from MCP"),
            "response should not echo content back: {result}"
        );

        let result = m.add_comment(Parameters(AddCommentInput {
            identifier: "PRJ-1".into(),
            content: "Second comment".into(),
        }));
        assert!(result.starts_with("Comment #"), "got: {result}");

        let result = m.list_comments(Parameters(ListCommentsInput {
            identifier: "PRJ-1".into(),
            ..Default::default()
        }));
        assert!(result.contains("2 comment(s)"), "got: {result}");
        assert!(result.contains("Hello from MCP"), "got: {result}");
        assert!(result.contains("Second comment"), "got: {result}");
    }

    // LIF-263: the MCP add_comment path records @mentions too, using the
    // same visible-member rules as REST (enforcement is off in this test
    // fixture, so all non-bot users are candidates).
    #[test]
    fn add_comment_records_mentions() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "Mention issue");
        let _guard = seed_user(&m);

        // Seed a second user to mention.
        let ada_id = {
            let conn = m.db.write().unwrap();
            crate::db::queries::users::create_user(
                &conn,
                &models::CreateUser {
                    username: "ada".into(),
                    email: "ada@test.com".into(),
                    password: "testpassword1".into(),
                    display_name: Some("Ada".into()),
                    is_admin: false,
                    is_bot: false,
                },
            )
            .unwrap()
            .id
        };

        let result = m.add_comment(Parameters(AddCommentInput {
            identifier: "PRJ-1".into(),
            content: "cc @ada and @ghost".into(),
        }));
        assert!(result.starts_with("Comment #"), "got: {result}");
        let comment_id: i64 = result
            .trim_start_matches("Comment #")
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();

        let conn = m.db.read().unwrap();
        let mentions =
            crate::db::queries::comments::list_mention_user_ids(&conn, comment_id).unwrap();
        assert_eq!(mentions, vec![ada_id], "only the real user @ada resolves");
    }

    #[test]
    fn get_issue_includes_comments() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "Commented issue");
        let _guard = seed_user(&m);

        m.add_comment(Parameters(AddCommentInput {
            identifier: "PRJ-1".into(),
            content: "Visible in get_issue".into(),
        }));

        let result = m.get_issue(Parameters(GetIssueInput {
            identifier: "PRJ-1".into(),
            ..Default::default()
        }));
        assert!(result.contains("Comments (1)"), "got: {result}");
        assert!(result.contains("Visible in get_issue"), "got: {result}");
    }

    // ── get_issue comment-trail controls (LIF-301) ──

    /// Post `n` numbered comments on PRJ-1 and return the mcp + guard.
    fn seed_comment_trail(m: &LificMcp, n: usize) {
        for i in 1..=n {
            m.add_comment(Parameters(AddCommentInput {
                identifier: "PRJ-1".into(),
                content: format!("comment number {i}"),
            }));
        }
    }

    #[test]
    fn get_issue_recent_truncates_over_three_comments() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "Chatty issue");
        let _guard = seed_user(&m);
        seed_comment_trail(&m, 9);

        let result = m.get_issue(Parameters(GetIssueInput {
            identifier: "PRJ-1".into(),
            ..Default::default()
        }));
        // Stub header reports the total and the truncation.
        assert!(
            result.contains("--- Comments (9, showing last 3 — use list_comments) ---"),
            "got: {result}"
        );
        // Only the LAST 3 (7,8,9) appear; earlier ones are hidden.
        assert!(result.contains("comment number 7"), "got: {result}");
        assert!(result.contains("comment number 8"), "got: {result}");
        assert!(result.contains("comment number 9"), "got: {result}");
        assert!(!result.contains("comment number 6"), "got: {result}");
        assert!(!result.contains("comment number 1 "), "got: {result}");
    }

    #[test]
    fn get_issue_recent_unchanged_at_three_or_fewer() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "Few comments");
        let _guard = seed_user(&m);
        seed_comment_trail(&m, 3);

        let result = m.get_issue(Parameters(GetIssueInput {
            identifier: "PRJ-1".into(),
            ..Default::default()
        }));
        assert!(result.contains("--- Comments (3) ---"), "got: {result}");
        assert!(!result.contains("showing last 3"), "got: {result}");
        assert!(result.contains("comment number 1"), "got: {result}");
    }

    #[test]
    fn get_issue_all_shows_every_comment() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "Full history");
        let _guard = seed_user(&m);
        seed_comment_trail(&m, 5);

        let result = m.get_issue(Parameters(GetIssueInput {
            identifier: "PRJ-1".into(),
            include_comments: Some("all".into()),
        }));
        assert!(result.contains("--- Comments (5) ---"), "got: {result}");
        for i in 1..=5 {
            assert!(
                result.contains(&format!("comment number {i}")),
                "got: {result}"
            );
        }
    }

    #[test]
    fn get_issue_none_emits_stub_only() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "Suppressed");
        let _guard = seed_user(&m);
        seed_comment_trail(&m, 9);

        let result = m.get_issue(Parameters(GetIssueInput {
            identifier: "PRJ-1".into(),
            include_comments: Some("none".into()),
        }));
        assert!(
            result.contains("--- Comments (9, omitted — use list_comments) ---"),
            "got: {result}"
        );
        assert!(!result.contains("comment number"), "got: {result}");
    }

    #[test]
    fn get_issue_none_with_zero_comments_shows_nothing() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "Quiet");

        let result = m.get_issue(Parameters(GetIssueInput {
            identifier: "PRJ-1".into(),
            include_comments: Some("none".into()),
        }));
        assert!(!result.contains("Comments"), "got: {result}");
    }

    #[test]
    fn get_issue_invalid_include_comments_errors() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "Bad mode");

        let result = m.get_issue(Parameters(GetIssueInput {
            identifier: "PRJ-1".into(),
            include_comments: Some("last5".into()),
        }));
        assert_eq!(
            result,
            "Error: invalid include_comments 'last5'. Use recent, all, or none."
        );
    }

    #[test]
    fn list_comments_limit_paginates_with_hint() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "Limited");
        let _guard = seed_user(&m);
        seed_comment_trail(&m, 9);

        let result = m.list_comments(Parameters(ListCommentsInput {
            identifier: "PRJ-1".into(),
            limit: Some(3),
            ..Default::default()
        }));
        assert!(
            result.starts_with("Showing 3 oldest of 9 comment(s) on PRJ-1:"),
            "got: {result}"
        );
        assert!(
            result.contains("call again with the same author/order/limit and offset=3"),
            "got: {result}"
        );
        // Default order is asc, so the first 3 are the oldest: 1,2,3.
        assert!(result.contains("comment number 1"), "got: {result}");
        assert!(result.contains("comment number 3"), "got: {result}");
        assert!(!result.contains("comment number 4"), "got: {result}");
    }

    #[test]
    fn list_comments_limit_desc_returns_newest_first() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "Newest N");
        let _guard = seed_user(&m);
        seed_comment_trail(&m, 9);

        let result = m.list_comments(Parameters(ListCommentsInput {
            identifier: "PRJ-1".into(),
            order: Some("desc".into()),
            limit: Some(2),
            ..Default::default()
        }));
        assert!(
            result.starts_with("Showing 2 most recent of 9 comment(s) on PRJ-1:"),
            "got: {result}"
        );
        // desc + limit 2 → the two newest (9, 8), not the oldest.
        assert!(result.contains("comment number 9"), "got: {result}");
        assert!(result.contains("comment number 8"), "got: {result}");
        assert!(!result.contains("comment number 1"), "got: {result}");
    }

    #[test]
    fn list_comments_offset_returns_next_page() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "Paged");
        let _guard = seed_user(&m);
        seed_comment_trail(&m, 5);

        let result = m.list_comments(Parameters(ListCommentsInput {
            identifier: "PRJ-1".into(),
            limit: Some(2),
            offset: Some(2),
            ..Default::default()
        }));
        assert!(result.contains("comment number 3"), "got: {result}");
        assert!(result.contains("comment number 4"), "got: {result}");
        assert!(!result.contains("comment number 2"), "got: {result}");
        assert!(!result.contains("comment number 5"), "got: {result}");
        assert!(
            result.contains("call again with the same author/order/limit and offset=4"),
            "got: {result}"
        );
        assert!(
            result.starts_with("Showing comments 3-4 of 5 on PRJ-1 (oldest first):"),
            "got: {result}"
        );
    }

    #[test]
    fn list_comments_no_limit_keeps_plain_header() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "Unlimited");
        let _guard = seed_user(&m);
        seed_comment_trail(&m, 9);

        let result = m.list_comments(Parameters(ListCommentsInput {
            identifier: "PRJ-1".into(),
            ..Default::default()
        }));
        assert!(
            result.starts_with("9 comment(s) on PRJ-1:"),
            "got: {result}"
        );
        for n in 1..=9 {
            assert!(
                result.contains(&format!("comment number {n}")),
                "got: {result}"
            );
        }
        assert!(!result.contains("Showing"), "got: {result}");
        assert!(!result.contains("more comment(s)"), "got: {result}");
    }

    #[test]
    fn list_comments_offset_without_limit_returns_unbounded_remainder() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "Offset remainder");
        let _guard = seed_user(&m);
        seed_comment_trail(&m, 5);

        let result = m.list_comments(Parameters(ListCommentsInput {
            identifier: "PRJ-1".into(),
            offset: Some(2),
            ..Default::default()
        }));
        assert!(
            result.starts_with("Showing comments 3-5 of 5 on PRJ-1 (oldest first):"),
            "got: {result}"
        );
        for n in 3..=5 {
            assert!(
                result.contains(&format!("comment number {n}")),
                "got: {result}"
            );
        }
        assert!(!result.contains("comment number 2"), "got: {result}");
        assert!(!result.contains("more comment(s)"), "got: {result}");
    }

    #[test]
    fn list_comments_offset_past_end_reports_total() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "Exhausted");
        let _guard = seed_user(&m);
        seed_comment_trail(&m, 3);

        let result = m.list_comments(Parameters(ListCommentsInput {
            identifier: "PRJ-1".into(),
            limit: Some(2),
            offset: Some(10),
            ..Default::default()
        }));
        assert!(result.contains("offset 10, 3 total"), "got: {result}");
        assert!(!result.contains("No comments on"), "got: {result}");
    }

    #[test]
    fn list_comments_with_zero_comments_reports_empty_thread() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "No comments");

        let result = m.list_comments(Parameters(ListCommentsInput {
            identifier: "PRJ-1".into(),
            ..Default::default()
        }));
        assert_eq!(result, "No comments on PRJ-1.");
    }

    #[test]
    fn add_comment_bad_identifier() {
        let m = mcp();
        let _guard = seed_user(&m);

        let result = m.add_comment(Parameters(AddCommentInput {
            identifier: "NOPE-999".into(),
            content: "Orphan".into(),
        }));
        assert!(result.contains("Error"), "got: {result}");
    }

    #[test]
    fn add_comment_falls_back_to_first_admin() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "Test issue");

        // Create an admin user but do NOT set MCP_REQUEST_USER — simulates stdio/local auth.
        let conn = m.db.write().unwrap();
        queries::users::create_user(
            &conn,
            &models::CreateUser {
                username: "admin".into(),
                email: "admin@local.test".into(),
                password: "adminpass123".into(),
                display_name: Some("Admin User".into()),
                is_admin: true,
                is_bot: false,
            },
        )
        .unwrap();
        drop(conn);

        // Clear any leftover auth context. Holds MCP_HANDLER_LOCK (see
        // `seed_user`'s doc comment) so this "clear, then rely on it staying
        // None" window can't be raced by a concurrently-running
        // `with_request_user` caller in another test.
        let _guard = crate::mcp::MCP_HANDLER_LOCK.blocking_lock();
        *crate::mcp::MCP_REQUEST_USER
            .lock()
            .unwrap_or_else(|e: std::sync::PoisonError<_>| e.into_inner()) = None;

        let result = m.add_comment(Parameters(AddCommentInput {
            identifier: "PRJ-1".into(),
            content: "Comment via stdio fallback".into(),
        }));
        assert!(result.starts_with("Comment #"), "got: {result}");
        assert!(result.contains("admin"), "got: {result}");
    }

    // ── LIF-106: page comments via add_comment/list_comments dispatch ────

    #[test]
    fn add_comment_on_page_identifier_creates_page_comment() {
        let m = mcp();
        seed_project(&m, "Pages", "PGC");
        m.create_page(Parameters(CreatePageInput {
            project: Some("PGC".into()),
            title: "Design".into(),
            content: None,
            folder: None,
            status: None,
            labels: None,
        }));
        let _guard = seed_user(&m);

        let result = m.add_comment(Parameters(AddCommentInput {
            identifier: "PGC-DOC-1".into(),
            content: "Comment on a page".into(),
        }));
        assert!(result.starts_with("Comment #"), "got: {result}");
        assert!(result.contains("PGC-DOC-1"), "got: {result}");

        let listing = m.list_comments(Parameters(ListCommentsInput {
            identifier: "PGC-DOC-1".into(),
            ..Default::default()
        }));
        assert!(listing.contains("1 comment(s)"), "got: {listing}");
        assert!(listing.contains("Comment on a page"), "got: {listing}");
    }

    #[test]
    fn project_page_comment_mutations_emit_project_updates() {
        let (m, mut rx) = mcp_with_realtime();
        seed_project(&m, "Page Comments", "PCO");
        let project_id = project_id_for(&m, "PCO");
        m.create_page(Parameters(CreatePageInput {
            project: Some("PCO".into()),
            title: "Design".into(),
            content: None,
            folder: None,
            status: None,
            labels: None,
        }));
        let _guard = seed_user(&m);
        drain_realtime(&mut rx);

        let added = m.add_comment(Parameters(AddCommentInput {
            identifier: "PCO-DOC-1".into(),
            content: "original".into(),
        }));
        let comment_id = comment_id_from(&added);
        assert_eq!(
            realtime_events(&mut rx),
            vec![crate::realtime::RealtimeEvent::ProjectUpdated { project_id }]
        );

        let edited = m.edit_comment(Parameters(EditCommentInput {
            comment_id,
            content: "revised".into(),
        }));
        assert!(edited.starts_with("Comment #"), "got: {edited}");
        assert_eq!(
            realtime_events(&mut rx),
            vec![crate::realtime::RealtimeEvent::ProjectUpdated { project_id }]
        );

        let deleted = m.delete_comment(Parameters(DeleteCommentInput { comment_id }));
        assert!(deleted.starts_with("Comment #"), "got: {deleted}");
        assert_eq!(
            realtime_events(&mut rx),
            vec![crate::realtime::RealtimeEvent::ProjectUpdated { project_id }]
        );
    }

    #[test]
    fn page_and_issue_comments_do_not_cross_contaminate_via_mcp() {
        let m = mcp();
        seed_project(&m, "Mix", "MIX");
        seed_issue(&m, "MIX", "An issue");
        m.create_page(Parameters(CreatePageInput {
            project: Some("MIX".into()),
            title: "A page".into(),
            content: None,
            folder: None,
            status: None,
            labels: None,
        }));
        let _guard = seed_user(&m);

        m.add_comment(Parameters(AddCommentInput {
            identifier: "MIX-1".into(),
            content: "issue thread".into(),
        }));
        m.add_comment(Parameters(AddCommentInput {
            identifier: "MIX-DOC-1".into(),
            content: "page thread".into(),
        }));

        let issue_listing = m.list_comments(Parameters(ListCommentsInput {
            identifier: "MIX-1".into(),
            ..Default::default()
        }));
        assert!(
            issue_listing.contains("issue thread"),
            "got: {issue_listing}"
        );
        assert!(
            !issue_listing.contains("page thread"),
            "got: {issue_listing}"
        );

        let page_listing = m.list_comments(Parameters(ListCommentsInput {
            identifier: "MIX-DOC-1".into(),
            ..Default::default()
        }));
        assert!(page_listing.contains("page thread"), "got: {page_listing}");
        assert!(
            !page_listing.contains("issue thread"),
            "got: {page_listing}"
        );
    }

    #[test]
    fn add_comment_on_workspace_page() {
        let m = mcp();
        // Workspace pages have no project prefix: identifier is DOC-N.
        m.create_page(Parameters(CreatePageInput {
            project: None,
            title: "Workspace note".into(),
            content: None,
            folder: None,
            status: None,
            labels: None,
        }));
        let _guard = seed_user(&m);

        let result = m.add_comment(Parameters(AddCommentInput {
            identifier: "DOC-1".into(),
            content: "comment on workspace page".into(),
        }));
        assert!(result.starts_with("Comment #"), "got: {result}");

        let listing = m.list_comments(Parameters(ListCommentsInput {
            identifier: "DOC-1".into(),
            ..Default::default()
        }));
        assert!(
            listing.contains("comment on workspace page"),
            "got: {listing}"
        );
    }

    // ── LIF-113: edit_issue / edit_page (surgical string replacement) ────

    // Pure-function tests of the apply_edit helper. Kept thin because the
    // tool-level tests below exercise the same paths through the real DB.

    #[test]
    fn apply_edit_replaces_single_match() {
        let result = apply_edit("hello world", "world", "there", false).unwrap();
        assert_eq!(result, "hello there");
    }

    #[test]
    fn apply_edit_rejects_empty_old_string() {
        let err = apply_edit("foo", "", "bar", false).unwrap_err();
        assert!(matches!(err, crate::error::LificError::BadRequest(_)));
    }

    #[test]
    fn apply_edit_rejects_identical_old_and_new() {
        let err = apply_edit("foo", "foo", "foo", false).unwrap_err();
        assert!(matches!(err, crate::error::LificError::BadRequest(_)));
    }

    #[test]
    fn apply_edit_rejects_no_match() {
        let err = apply_edit("hello", "missing", "x", false).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not found"), "got: {msg}");
    }

    #[test]
    fn apply_edit_rejects_multiple_matches_without_replace_all() {
        let err = apply_edit("foo foo foo", "foo", "bar", false).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("matches 3 locations"), "got: {msg}");
        assert!(msg.contains("replace_all=true"), "got: {msg}");
    }

    #[test]
    fn apply_edit_replace_all_substitutes_every_occurrence() {
        let result = apply_edit("foo foo foo", "foo", "bar", true).unwrap();
        assert_eq!(result, "bar bar bar");
    }

    #[test]
    fn apply_edit_handles_unicode() {
        // Make sure str::matches + str::replace are unicode-safe.
        let result = apply_edit("café ☕ shop", "café ☕", "tea 🍵", false).unwrap();
        assert_eq!(result, "tea 🍵 shop");
    }

    // ── edit_issue (MCP tool) ────

    fn seed_issue_with_description(
        mcp: &LificMcp,
        project: &str,
        title: &str,
        desc: &str,
    ) -> String {
        let result = mcp.create_issue(Parameters(CreateIssueInput {
            project: project.into(),
            title: title.into(),
            description: Some(desc.into()),
            status: None,
            priority: None,
            module: None,
            labels: None,
            ..Default::default()
        }));
        assert!(result.starts_with("Created"), "got: {result}");
        result
    }

    #[test]
    fn edit_issue_unique_match_succeeds() {
        let m = mcp();
        seed_project(&m, "Test", "EDI");
        seed_issue_with_description(&m, "EDI", "T", "The quick brown fox");

        let result = m.edit_issue(Parameters(EditIssueInput {
            identifier: "EDI-1".into(),
            old_string: "brown".into(),
            new_string: "red".into(),
            field: None,
            replace_all: None,
        }));
        assert!(result.starts_with("Edited"), "got: {result}");

        let detail = m.get_issue(Parameters(GetIssueInput {
            identifier: "EDI-1".into(),
            ..Default::default()
        }));
        assert!(detail.contains("The quick red fox"), "got: {detail}");
        assert!(!detail.contains("brown"), "got: {detail}");
    }

    #[test]
    fn export_dispatches_on_identifier_shape() {
        let m = mcp();
        seed_project(&m, "Test", "EXP");
        seed_issue_with_description(&m, "EXP", "Ship it", "issue body here");
        let created = m.create_page(Parameters(CreatePageInput {
            project: Some("EXP".into()),
            title: "Design notes".into(),
            content: Some("page body here".into()),
            ..Default::default()
        }));
        assert!(created.starts_with("Created"), "got: {created}");

        // Issue shape (EXP-1) returns the issue markdown.
        let issue = m.export(Parameters(ExportInput {
            identifier: "EXP-1".into(),
        }));
        assert!(issue.contains("issue body here"), "got: {issue}");

        // Page shape (EXP-DOC-1) returns the page markdown.
        let page = m.export(Parameters(ExportInput {
            identifier: "EXP-DOC-1".into(),
        }));
        assert!(page.contains("page body here"), "got: {page}");

        // Bare project shape (EXP) returns the exported file listing.
        let project = m.export(Parameters(ExportInput {
            identifier: "EXP".into(),
        }));
        assert!(project.contains("exported file(s)"), "got: {project}");

        // Unknown identifiers name all three shapes in the error.
        let err = m.export(Parameters(ExportInput {
            identifier: "NOPE-999".into(),
        }));
        assert!(
            err.contains("not a known issue, page, or project"),
            "got: {err}"
        );
    }

    #[test]
    fn create_and_edit_issue_preserves_literal_escapes_in_multiline_code() {
        let m = mcp();
        seed_project(&m, "Test", "ESC");
        let description = "Example:\n```c\nprintf(\"\\n\");\n```\n";
        let created = m.create_issue(Parameters(CreateIssueInput {
            project: "ESC".into(),
            title: "Preserve code escapes".into(),
            description: Some(description.into()),
            ..Default::default()
        }));
        assert!(created.starts_with("Created"), "got: {created}");

        let detail = m.get_issue(Parameters(GetIssueInput {
            identifier: "ESC-1".into(),
            ..Default::default()
        }));
        assert!(
            detail.contains(description),
            "get_issue mangled content: {detail}"
        );

        let exported = m.export(Parameters(ExportInput {
            identifier: "ESC-1".into(),
        }));
        assert!(
            exported.contains(description.trim_end()),
            "export mangled content: {exported}"
        );

        let result = m.edit_issue(Parameters(EditIssueInput {
            identifier: "ESC-1".into(),
            old_string: "```c\nprintf(\"\\n\");\n```".into(),
            new_string: "```c\nputs(\"hello\");\n```".into(),
            field: None,
            replace_all: None,
        }));
        assert!(result.starts_with("Edited"), "got: {result}");

        let updated = m.get_issue(Parameters(GetIssueInput {
            identifier: "ESC-1".into(),
            ..Default::default()
        }));
        assert!(updated.contains("puts(\"hello\");"), "got: {updated}");
        assert!(
            !updated.contains("printf(\"\\n\");"),
            "literal escape was not replaced: {updated}"
        );
    }

    #[test]
    fn edit_issue_emits_issue_update() {
        let (m, mut rx) = mcp_with_realtime();
        seed_project(&m, "Edit Events", "EDE");
        seed_issue_with_description(&m, "EDE", "T", "hello world");
        let project_id = project_id_for(&m, "EDE");
        let issue_id = issue_id_for(&m, "EDE-1");
        drain_realtime(&mut rx);

        let result = m.edit_issue(Parameters(EditIssueInput {
            identifier: "EDE-1".into(),
            old_string: "world".into(),
            new_string: "there".into(),
            field: None,
            replace_all: None,
        }));

        assert!(result.starts_with("Edited"), "got: {result}");
        assert_eq!(
            realtime_events(&mut rx),
            vec![crate::realtime::RealtimeEvent::IssueUpdated {
                project_id,
                issue_id,
            }]
        );
    }

    #[test]
    fn edit_issue_no_match_fails_with_clear_error() {
        let m = mcp();
        seed_project(&m, "Test", "EDN");
        seed_issue_with_description(&m, "EDN", "T", "hello world");

        let result = m.edit_issue(Parameters(EditIssueInput {
            identifier: "EDN-1".into(),
            old_string: "missing".into(),
            new_string: "x".into(),
            field: None,
            replace_all: None,
        }));
        assert!(result.starts_with("Error"), "got: {result}");
        assert!(result.contains("not found"), "got: {result}");

        // Original content untouched.
        let detail = m.get_issue(Parameters(GetIssueInput {
            identifier: "EDN-1".into(),
            ..Default::default()
        }));
        assert!(detail.contains("hello world"), "got: {detail}");
    }

    #[test]
    fn edit_issue_multiple_match_fails_without_replace_all() {
        let m = mcp();
        seed_project(&m, "Test", "EDM");
        seed_issue_with_description(&m, "EDM", "T", "foo foo foo");

        let result = m.edit_issue(Parameters(EditIssueInput {
            identifier: "EDM-1".into(),
            old_string: "foo".into(),
            new_string: "bar".into(),
            field: None,
            replace_all: None,
        }));
        assert!(result.starts_with("Error"), "got: {result}");
        assert!(result.contains("3 locations"), "got: {result}");
        assert!(result.contains("replace_all"), "got: {result}");
    }

    #[test]
    fn edit_issue_replace_all_succeeds_when_set() {
        let m = mcp();
        seed_project(&m, "Test", "EDA");
        seed_issue_with_description(&m, "EDA", "T", "foo foo foo");

        let result = m.edit_issue(Parameters(EditIssueInput {
            identifier: "EDA-1".into(),
            old_string: "foo".into(),
            new_string: "bar".into(),
            field: None,
            replace_all: Some(true),
        }));
        assert!(result.starts_with("Edited"), "got: {result}");

        let detail = m.get_issue(Parameters(GetIssueInput {
            identifier: "EDA-1".into(),
            ..Default::default()
        }));
        assert!(detail.contains("bar bar bar"), "got: {detail}");
    }

    #[test]
    fn edit_issue_empty_old_string_fails() {
        let m = mcp();
        seed_project(&m, "Test", "EDE");
        seed_issue_with_description(&m, "EDE", "T", "anything");

        let result = m.edit_issue(Parameters(EditIssueInput {
            identifier: "EDE-1".into(),
            old_string: "".into(),
            new_string: "x".into(),
            field: None,
            replace_all: None,
        }));
        assert!(result.starts_with("Error"), "got: {result}");
        assert!(result.contains("empty"), "got: {result}");
    }

    #[test]
    fn edit_issue_identical_old_new_fails() {
        let m = mcp();
        seed_project(&m, "Test", "EDS");
        seed_issue_with_description(&m, "EDS", "T", "hello");

        let result = m.edit_issue(Parameters(EditIssueInput {
            identifier: "EDS-1".into(),
            old_string: "hello".into(),
            new_string: "hello".into(),
            field: None,
            replace_all: None,
        }));
        assert!(result.starts_with("Error"), "got: {result}");
        assert!(result.contains("differ"), "got: {result}");
    }

    #[test]
    fn edit_issue_title_field_works() {
        let m = mcp();
        seed_project(&m, "Test", "EDT");
        seed_issue_with_description(&m, "EDT", "Old name here", "body");

        let result = m.edit_issue(Parameters(EditIssueInput {
            identifier: "EDT-1".into(),
            old_string: "Old".into(),
            new_string: "New".into(),
            field: Some("title".into()),
            replace_all: None,
        }));
        assert!(result.starts_with("Edited"), "got: {result}");
        assert!(result.contains("New name here"), "got: {result}");

        // Description untouched.
        let detail = m.get_issue(Parameters(GetIssueInput {
            identifier: "EDT-1".into(),
            ..Default::default()
        }));
        assert!(detail.contains("body"), "got: {detail}");
    }

    #[test]
    fn edit_issue_invalid_field_fails() {
        let m = mcp();
        seed_project(&m, "Test", "EDX");
        seed_issue_with_description(&m, "EDX", "T", "body");

        let result = m.edit_issue(Parameters(EditIssueInput {
            identifier: "EDX-1".into(),
            old_string: "body".into(),
            new_string: "x".into(),
            field: Some("status".into()),
            replace_all: None,
        }));
        assert!(result.starts_with("Error"), "got: {result}");
        assert!(result.contains("invalid field"), "got: {result}");
    }

    #[test]
    fn edit_issue_preserves_other_fields() {
        let m = mcp();
        seed_project(&m, "Test", "EDP");
        m.create_issue(Parameters(CreateIssueInput {
            project: "EDP".into(),
            title: "Stays".into(),
            description: Some("change me".into()),
            status: Some("active".into()),
            priority: Some("high".into()),
            module: None,
            labels: None,
            ..Default::default()
        }));

        m.edit_issue(Parameters(EditIssueInput {
            identifier: "EDP-1".into(),
            old_string: "change".into(),
            new_string: "kept".into(),
            field: None,
            replace_all: None,
        }));

        let detail = m.get_issue(Parameters(GetIssueInput {
            identifier: "EDP-1".into(),
            ..Default::default()
        }));
        assert!(detail.contains("Stays"), "title preserved, got: {detail}");
        assert!(detail.contains("active"), "status preserved, got: {detail}");
        assert!(detail.contains("high"), "priority preserved, got: {detail}");
        assert!(
            detail.contains("kept me"),
            "description edited, got: {detail}"
        );
    }

    // ── edit_page (MCP tool) ────

    #[test]
    fn edit_page_content_works() {
        let m = mcp();
        seed_project(&m, "Test", "EPC");
        m.create_page(Parameters(CreatePageInput {
            project: Some("EPC".into()),
            title: "Doc".into(),
            content: Some("# Heading\nold body".into()),
            folder: None,
            status: None,
            labels: None,
        }));

        let result = m.edit_page(Parameters(EditPageInput {
            identifier: "EPC-DOC-1".into(),
            old_string: "old body".into(),
            new_string: "new body".into(),
            field: None,
            replace_all: None,
        }));
        assert!(result.starts_with("Edited"), "got: {result}");

        let detail = m.get_page(Parameters(GetPageInput {
            identifier: "EPC-DOC-1".into(),
        }));
        assert!(detail.contains("new body"), "got: {detail}");
        assert!(!detail.contains("old body"), "got: {detail}");
    }

    #[test]
    fn project_scoped_page_mutations_emit_project_updates() {
        let (m, mut rx) = mcp_with_realtime();
        seed_project(&m, "Page Events", "PGE");
        let project_id = project_id_for(&m, "PGE");
        drain_realtime(&mut rx);

        let created = m.create_page(Parameters(CreatePageInput {
            project: Some("PGE".into()),
            title: "Design".into(),
            content: Some("draft".into()),
            folder: None,
            status: None,
            labels: None,
        }));
        assert!(created.starts_with("Created"), "got: {created}");
        assert_eq!(
            realtime_events(&mut rx),
            vec![crate::realtime::RealtimeEvent::ProjectUpdated { project_id }]
        );

        let updated = m.update_page(Parameters(UpdatePageInput {
            identifier: "PGE-DOC-1".into(),
            title: Some("Design v2".into()),
            ..Default::default()
        }));
        assert!(updated.starts_with("Updated"), "got: {updated}");
        assert_eq!(
            realtime_events(&mut rx),
            vec![crate::realtime::RealtimeEvent::ProjectUpdated { project_id }]
        );

        let edited = m.edit_page(Parameters(EditPageInput {
            identifier: "PGE-DOC-1".into(),
            old_string: "draft".into(),
            new_string: "published".into(),
            field: None,
            replace_all: None,
        }));
        assert!(edited.starts_with("Edited"), "got: {edited}");
        assert_eq!(
            realtime_events(&mut rx),
            vec![crate::realtime::RealtimeEvent::ProjectUpdated { project_id }]
        );
    }

    #[test]
    fn edit_page_title_field_works() {
        let m = mcp();
        seed_project(&m, "Test", "EPT");
        m.create_page(Parameters(CreatePageInput {
            project: Some("EPT".into()),
            title: "Draft Spec".into(),
            content: Some("body".into()),
            folder: None,
            status: None,
            labels: None,
        }));

        let result = m.edit_page(Parameters(EditPageInput {
            identifier: "EPT-DOC-1".into(),
            old_string: "Draft".into(),
            new_string: "Final".into(),
            field: Some("title".into()),
            replace_all: None,
        }));
        assert!(result.starts_with("Edited"), "got: {result}");
        assert!(result.contains("Final Spec"), "got: {result}");
    }

    #[test]
    fn edit_page_preserves_other_fields() {
        let m = mcp();
        seed_project(&m, "Test", "EPP");
        // Folder so we can verify it's preserved.
        m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "folder".into(),
            action: "create".into(),
            project: Some("EPP".into()),
            name: Some("Specs".into()),
            identifier: None,
            description: None,
            current_name: None,
            status: None,
            color: None,
            emoji: None,
        }));
        m.create_page(Parameters(CreatePageInput {
            project: Some("EPP".into()),
            title: "Original Title".into(),
            content: Some("change me".into()),
            folder: Some("Specs".into()),
            status: None,
            labels: None,
        }));

        m.edit_page(Parameters(EditPageInput {
            identifier: "EPP-DOC-1".into(),
            old_string: "change".into(),
            new_string: "kept".into(),
            field: None,
            replace_all: None,
        }));

        let detail = m.get_page(Parameters(GetPageInput {
            identifier: "EPP-DOC-1".into(),
        }));
        // Title preserved, content edited.
        assert!(
            detail.contains("Original Title"),
            "title preserved, got: {detail}"
        );
        assert!(detail.contains("kept me"), "content edited, got: {detail}");

        // Folder preserved — verified via list_pages with the folder filter.
        let listing = m.list_resources(Parameters(ListResourcesInput {
            resource_type: "page".into(),
            project: Some("EPP".into()),
            folder: Some("Specs".into()),
            label: None,
            limit: None,
            offset: None,
            ..Default::default()
        }));
        assert!(
            listing.contains("EPP-DOC-1"),
            "folder preserved, got: {listing}"
        );
    }

    #[test]
    fn edit_page_no_match_fails() {
        let m = mcp();
        seed_project(&m, "Test", "EPN");
        m.create_page(Parameters(CreatePageInput {
            project: Some("EPN".into()),
            title: "Doc".into(),
            content: Some("hello".into()),
            folder: None,
            status: None,
            labels: None,
        }));

        let result = m.edit_page(Parameters(EditPageInput {
            identifier: "EPN-DOC-1".into(),
            old_string: "missing".into(),
            new_string: "x".into(),
            field: None,
            replace_all: None,
        }));
        assert!(result.starts_with("Error"), "got: {result}");
        assert!(result.contains("not found"), "got: {result}");
    }

    #[test]
    fn edit_page_invalid_field_fails() {
        let m = mcp();
        seed_project(&m, "Test", "EPX");
        m.create_page(Parameters(CreatePageInput {
            project: Some("EPX".into()),
            title: "Doc".into(),
            content: Some("body".into()),
            folder: None,
            status: None,
            labels: None,
        }));

        let result = m.edit_page(Parameters(EditPageInput {
            identifier: "EPX-DOC-1".into(),
            old_string: "body".into(),
            new_string: "x".into(),
            field: Some("folder".into()),
            replace_all: None,
        }));
        assert!(result.starts_with("Error"), "got: {result}");
        assert!(result.contains("invalid field"), "got: {result}");
    }

    // ── LIF-105: page labels (MCP surface) ─────────────────────────

    /// Helper: seed two labels in a project, return the project identifier.
    fn seed_labels_for_pages(m: &LificMcp, project_ident: &str, project_name: &str) {
        seed_project(m, project_name, project_ident);
        for (name, color) in [("design", "#22C55E"), ("draft", "#F59E0B")] {
            m.manage_resource(Parameters(ManageResourceInput {
                resource_type: "label".into(),
                action: "create".into(),
                project: Some(project_ident.into()),
                name: Some(name.into()),
                color: Some(color.into()),
                identifier: None,
                description: None,
                current_name: None,
                status: None,
                emoji: None,
            }));
        }
    }

    #[test]
    fn mcp_create_page_with_labels_returns_them_in_get() {
        let m = mcp();
        seed_labels_for_pages(&m, "PGL", "Pages with Labels");

        let created = m.create_page(Parameters(CreatePageInput {
            project: Some("PGL".into()),
            title: "Spec".into(),
            content: None,
            folder: None,
            status: None,
            labels: Some(vec!["design".into()]),
        }));
        assert!(created.contains("PGL-DOC-1"), "got: {created}");

        let detail = m.get_page(Parameters(GetPageInput {
            identifier: "PGL-DOC-1".into(),
        }));
        // get_page emits `Labels: <names>` when non-empty (mirrors get_issue).
        assert!(detail.contains("Labels: design"), "got: {detail}");
    }

    #[test]
    fn mcp_update_page_replaces_labels() {
        let m = mcp();
        seed_labels_for_pages(&m, "PUL", "Page Update Labels");
        m.create_page(Parameters(CreatePageInput {
            project: Some("PUL".into()),
            title: "Spec".into(),
            content: None,
            folder: None,
            status: None,
            labels: Some(vec!["design".into()]),
        }));

        m.update_page(Parameters(UpdatePageInput {
            identifier: "PUL-DOC-1".into(),
            title: None,
            content: None,
            folder: None,
            status: None,
            pinned: None,
            labels: Some(vec!["draft".into()]),
        }));

        let detail = m.get_page(Parameters(GetPageInput {
            identifier: "PUL-DOC-1".into(),
        }));
        assert!(detail.contains("Labels: draft"), "got: {detail}");
        assert!(!detail.contains("design"), "got: {detail}");
    }

    #[test]
    fn mcp_update_issue_clears_module_with_empty_string() {
        let m = mcp();
        seed_project(&m, "Clear Module", "CLM");
        m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "module".into(),
            action: "create".into(),
            project: Some("CLM".into()),
            name: Some("Core".into()),
            identifier: None,
            description: None,
            current_name: None,
            status: None,
            color: None,
            emoji: None,
        }));
        seed_issue(&m, "CLM", "Task");

        // Assign to module.
        let set = m.update_issue(Parameters(UpdateIssueInput {
            identifier: "CLM-1".into(),
            title: None,
            description: None,
            status: None,
            priority: None,
            module: Some("Core".into()),
            labels: None,
            ..Default::default()
        }));
        assert!(!set.starts_with("Error"), "set failed: {set}");
        let detail = m.get_issue(Parameters(GetIssueInput {
            identifier: "CLM-1".into(),
            ..Default::default()
        }));
        assert!(detail.contains("Module: Core"), "got: {detail}");

        // Clear via empty-string sentinel.
        let cleared = m.update_issue(Parameters(UpdateIssueInput {
            identifier: "CLM-1".into(),
            title: None,
            description: None,
            status: None,
            priority: None,
            module: Some(String::new()),
            labels: None,
            ..Default::default()
        }));
        assert!(!cleared.starts_with("Error"), "clear failed: {cleared}");
        let detail = m.get_issue(Parameters(GetIssueInput {
            identifier: "CLM-1".into(),
            ..Default::default()
        }));
        assert!(detail.contains("Module: none"), "got: {detail}");

        // DB truth: module_id is NULL.
        let conn = m.db.read().unwrap();
        let id = queries::resolve_identifier(&conn, "CLM-1").unwrap();
        assert_eq!(queries::get_issue(&conn, id).unwrap().module_id, None);
    }

    /// Move a page into a folder, then move it back to root via the
    /// empty-string sentinel (folder_id = NULL).
    #[test]
    fn mcp_update_page_clears_folder_with_empty_string() {
        let m = mcp();
        seed_project(&m, "Clear Folder", "CLF");
        m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "folder".into(),
            action: "create".into(),
            project: Some("CLF".into()),
            name: Some("Docs".into()),
            identifier: None,
            description: None,
            current_name: None,
            status: None,
            color: None,
            emoji: None,
        }));
        m.create_page(Parameters(CreatePageInput {
            project: Some("CLF".into()),
            title: "Spec".into(),
            content: None,
            folder: None,
            status: None,
            labels: None,
        }));

        // Move into folder.
        let moved = m.update_page(Parameters(UpdatePageInput {
            identifier: "CLF-DOC-1".into(),
            title: None,
            content: None,
            folder: Some("Docs".into()),
            status: None,
            pinned: None,
            labels: None,
        }));
        assert!(!moved.starts_with("Error"), "move failed: {moved}");
        {
            let conn = m.db.read().unwrap();
            let id = queries::resolve_page_identifier(&conn, "CLF-DOC-1").unwrap();
            assert!(queries::get_page(&conn, id).unwrap().folder_id.is_some());
        }

        // Move back to root via empty-string sentinel.
        let rooted = m.update_page(Parameters(UpdatePageInput {
            identifier: "CLF-DOC-1".into(),
            title: None,
            content: None,
            folder: Some(String::new()),
            status: None,
            pinned: None,
            labels: None,
        }));
        assert!(!rooted.starts_with("Error"), "root failed: {rooted}");
        let conn = m.db.read().unwrap();
        let id = queries::resolve_page_identifier(&conn, "CLF-DOC-1").unwrap();
        assert_eq!(queries::get_page(&conn, id).unwrap().folder_id, None);
    }

    /// Set a project emoji on update, then clear it via the empty-string
    /// sentinel (emoji = NULL).
    #[test]
    fn mcp_manage_resource_sets_then_clears_project_emoji() {
        let m = mcp();
        seed_project(&m, "Emoji Project", "EMP");

        // Set emoji.
        let set = m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "project".into(),
            action: "update".into(),
            project: Some("EMP".into()),
            name: None,
            identifier: None,
            description: None,
            current_name: None,
            status: None,
            color: None,
            emoji: Some("lucide:Rocket".into()),
        }));
        assert!(!set.starts_with("Error"), "set failed: {set}");
        {
            let conn = m.db.read().unwrap();
            let pid = queries::resolve_project_identifier(&conn, "EMP").unwrap();
            assert_eq!(
                queries::get_project(&conn, pid).unwrap().emoji.as_deref(),
                Some("lucide:Rocket")
            );
        }

        // Clear via empty-string sentinel.
        let cleared = m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "project".into(),
            action: "update".into(),
            project: Some("EMP".into()),
            name: None,
            identifier: None,
            description: None,
            current_name: None,
            status: None,
            color: None,
            emoji: Some(String::new()),
        }));
        assert!(!cleared.starts_with("Error"), "clear failed: {cleared}");
        let conn = m.db.read().unwrap();
        let pid = queries::resolve_project_identifier(&conn, "EMP").unwrap();
        assert_eq!(queries::get_project(&conn, pid).unwrap().emoji, None);
    }

    #[test]
    fn mcp_manage_resource_sets_module_emoji() {
        let m = mcp();
        seed_project(&m, "Module Emoji", "MEM");
        m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "module".into(),
            action: "create".into(),
            project: Some("MEM".into()),
            name: Some("Core".into()),
            identifier: None,
            description: None,
            current_name: None,
            status: None,
            color: None,
            emoji: None,
        }));

        let set = m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "module".into(),
            action: "update".into(),
            project: Some("MEM".into()),
            name: None,
            identifier: None,
            description: None,
            current_name: Some("Core".into()),
            status: None,
            color: None,
            emoji: Some("lucide:Boxes".into()),
        }));
        assert!(!set.starts_with("Error"), "set failed: {set}");

        let conn = m.db.read().unwrap();
        let pid = queries::resolve_project_identifier(&conn, "MEM").unwrap();
        let mid = queries::resolve_module_name(&conn, pid, "Core").unwrap();
        assert_eq!(
            queries::get_module(&conn, mid).unwrap().emoji.as_deref(),
            Some("lucide:Boxes")
        );
    }

    #[test]
    fn mcp_list_resources_pages_renders_label_brackets() {
        // The MCP page list line is
        // `- {id} | {status} | {title}[ [labels]][ (folder: F)] — updated {date}`
        // — matches the issue list formatter so an agent reading both
        // surfaces sees one mental model.
        let m = mcp();
        seed_labels_for_pages(&m, "PLI", "Page List");
        m.create_page(Parameters(CreatePageInput {
            project: Some("PLI".into()),
            title: "Tagged".into(),
            content: None,
            folder: None,
            status: None,
            labels: Some(vec!["design".into()]),
        }));
        m.create_page(Parameters(CreatePageInput {
            project: Some("PLI".into()),
            title: "Bare".into(),
            content: None,
            folder: None,
            status: None,
            labels: None,
        }));

        let listing = m.list_resources(Parameters(ListResourcesInput {
            resource_type: "page".into(),
            project: Some("PLI".into()),
            folder: None,
            label: None,
            limit: None,
            offset: None,
            ..Default::default()
        }));
        assert!(listing.contains("Tagged [design]"), "got: {listing}");
        // Bare page should NOT carry an empty `[]` bracket — keeps the
        // common case terse. The line continues with the updated stamp.
        assert!(listing.contains("| Bare — updated"), "got: {listing}");
        assert!(!listing.contains("Bare []"), "got: {listing}");
    }

    #[test]
    fn mcp_list_resources_pages_label_filter() {
        let m = mcp();
        seed_labels_for_pages(&m, "PLF", "Page Label Filter");
        m.create_page(Parameters(CreatePageInput {
            project: Some("PLF".into()),
            title: "Designy".into(),
            content: None,
            folder: None,
            status: None,
            labels: Some(vec!["design".into()]),
        }));
        m.create_page(Parameters(CreatePageInput {
            project: Some("PLF".into()),
            title: "Plain".into(),
            content: None,
            folder: None,
            status: None,
            labels: None,
        }));

        let filtered = m.list_resources(Parameters(ListResourcesInput {
            resource_type: "page".into(),
            project: Some("PLF".into()),
            folder: None,
            label: Some("design".into()),
            limit: None,
            offset: None,
            ..Default::default()
        }));
        assert!(filtered.contains("Designy"), "got: {filtered}");
        assert!(!filtered.contains("Plain"), "got: {filtered}");
    }

    #[test]
    fn mcp_workspace_page_create_with_labels_silently_drops_them() {
        let m = mcp();
        // No seed_project: workspace pages live outside any project. The
        // labels list is silently ignored (project-scoped labels can't
        // attach without a project).
        let created = m.create_page(Parameters(CreatePageInput {
            project: None,
            title: "Floating".into(),
            content: None,
            folder: None,
            status: None,
            labels: Some(vec!["anything".into()]),
        }));
        assert!(created.contains("DOC-1"), "got: {created}");

        let detail = m.get_page(Parameters(GetPageInput {
            identifier: "DOC-1".into(),
        }));
        // No `Labels:` line present.
        assert!(!detail.contains("Labels:"), "got: {detail}");
    }

    // ── Page metadata surfacing + filters (status/folder/timestamps) ──────

    #[test]
    fn mcp_get_page_surfaces_status_folder_and_timestamps() {
        let m = mcp();
        seed_project(&m, "Meta", "MET");
        m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "folder".into(),
            action: "create".into(),
            project: Some("MET".into()),
            name: Some("Specs".into()),
            identifier: None,
            description: None,
            current_name: None,
            status: None,
            color: None,
            emoji: None,
        }));
        m.create_page(Parameters(CreatePageInput {
            project: Some("MET".into()),
            title: "Spec doc".into(),
            content: Some("Body text".into()),
            folder: Some("Specs".into()),
            status: Some("active".into()),
            labels: None,
        }));

        let detail = m.get_page(Parameters(GetPageInput {
            identifier: "MET-DOC-1".into(),
        }));
        assert!(
            detail.contains("Status: active | Folder: Specs"),
            "got: {detail}"
        );
        assert!(detail.contains("Created: "), "got: {detail}");
        assert!(detail.contains("Updated: "), "got: {detail}");
        // Metadata header comes BEFORE the content.
        let header_pos = detail.find("Status: active").unwrap();
        let body_pos = detail.find("Body text").unwrap();
        assert!(
            header_pos < body_pos,
            "metadata must precede content: {detail}"
        );
    }

    #[test]
    fn mcp_get_page_without_folder_says_none() {
        let m = mcp();
        seed_project(&m, "Meta", "MET");
        m.create_page(Parameters(CreatePageInput {
            project: Some("MET".into()),
            title: "Loose doc".into(),
            content: None,
            folder: None,
            status: None,
            labels: None,
        }));

        let detail = m.get_page(Parameters(GetPageInput {
            identifier: "MET-DOC-1".into(),
        }));
        assert!(
            detail.contains("Status: draft | Folder: none"),
            "got: {detail}"
        );
    }

    #[test]
    fn mcp_list_resources_pages_filters_by_status() {
        let m = mcp();
        seed_project(&m, "Stat", "STA");
        m.create_page(Parameters(CreatePageInput {
            project: Some("STA".into()),
            title: "Live".into(),
            content: None,
            folder: None,
            status: Some("active".into()),
            labels: None,
        }));
        m.create_page(Parameters(CreatePageInput {
            project: Some("STA".into()),
            title: "Old".into(),
            content: None,
            folder: None,
            status: Some("archived".into()),
            labels: None,
        }));

        let active = m.list_resources(Parameters(ListResourcesInput {
            resource_type: "page".into(),
            project: Some("STA".into()),
            status: Some("active".into()),
            ..Default::default()
        }));
        assert!(active.contains("Live"), "got: {active}");
        assert!(!active.contains("Old"), "got: {active}");
        // Status is also visible on each listing line.
        assert!(active.contains("| active |"), "got: {active}");
    }

    #[test]
    fn mcp_list_resources_pages_orders_by_title_desc() {
        let m = mcp();
        seed_project(&m, "Ord", "ORD");
        for title in ["Alpha", "Zulu", "Mike"] {
            m.create_page(Parameters(CreatePageInput {
                project: Some("ORD".into()),
                title: title.into(),
                content: None,
                folder: None,
                status: None,
                labels: None,
            }));
        }

        let listing = m.list_resources(Parameters(ListResourcesInput {
            resource_type: "page".into(),
            project: Some("ORD".into()),
            order_by: Some("title".into()),
            order: Some("desc".into()),
            ..Default::default()
        }));
        let zulu = listing.find("Zulu").unwrap();
        let mike = listing.find("Mike").unwrap();
        let alpha = listing.find("Alpha").unwrap();
        assert!(zulu < mike && mike < alpha, "got: {listing}");
    }

    #[test]
    fn mcp_list_resources_pages_shows_folder_name() {
        let m = mcp();
        seed_project(&m, "Fold", "FOL");
        m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "folder".into(),
            action: "create".into(),
            project: Some("FOL".into()),
            name: Some("Design".into()),
            identifier: None,
            description: None,
            current_name: None,
            status: None,
            color: None,
            emoji: None,
        }));
        m.create_page(Parameters(CreatePageInput {
            project: Some("FOL".into()),
            title: "Foldered".into(),
            content: None,
            folder: Some("Design".into()),
            status: None,
            labels: None,
        }));

        let listing = m.list_resources(Parameters(ListResourcesInput {
            resource_type: "page".into(),
            project: Some("FOL".into()),
            ..Default::default()
        }));
        assert!(listing.contains("(folder: Design)"), "got: {listing}");
    }

    // ── list_resources(page) pagination (LIF-137) ─────────────────────────

    #[test]
    fn mcp_list_resources_pages_respects_limit() {
        let m = mcp();
        seed_project(&m, "Pag", "PAG");
        for title in ["P1", "P2", "P3", "P4", "P5"] {
            m.create_page(Parameters(CreatePageInput {
                project: Some("PAG".into()),
                title: title.into(),
                content: None,
                folder: None,
                status: None,
                labels: None,
            }));
        }

        let listing = m.list_resources(Parameters(ListResourcesInput {
            resource_type: "page".into(),
            project: Some("PAG".into()),
            limit: Some(2),
            ..Default::default()
        }));
        // Only 2 of the 5 pages should render.
        assert!(listing.starts_with("2 pages:"), "got: {listing}");
        assert_eq!(
            listing.matches("| draft |").count(),
            2,
            "expected exactly 2 page lines, got: {listing}"
        );
        // More remain, so the hint points to the next page.
        assert!(
            listing.contains("call again with offset=2"),
            "got: {listing}"
        );
    }

    #[test]
    fn mcp_list_resources_pages_offset_pages_correctly() {
        let m = mcp();
        seed_project(&m, "Off", "OFF");
        // Deterministic order: sort by title asc so we know which page lands
        // on which offset.
        for title in ["A", "B", "C", "D"] {
            m.create_page(Parameters(CreatePageInput {
                project: Some("OFF".into()),
                title: title.into(),
                content: None,
                folder: None,
                status: None,
                labels: None,
            }));
        }

        let page2 = m.list_resources(Parameters(ListResourcesInput {
            resource_type: "page".into(),
            project: Some("OFF".into()),
            order_by: Some("title".into()),
            order: Some("asc".into()),
            limit: Some(2),
            offset: Some(2),
            ..Default::default()
        }));
        // Offset 2 with limit 2 skips A/B and shows C/D.
        assert!(!page2.contains("| A "), "got: {page2}");
        assert!(!page2.contains("| B "), "got: {page2}");
        assert!(page2.contains("| C "), "got: {page2}");
        assert!(page2.contains("| D "), "got: {page2}");
    }

    #[test]
    fn mcp_list_resources_pages_hint_absent_on_last_page() {
        let m = mcp();
        seed_project(&m, "Last", "LST");
        for title in ["X", "Y", "Z"] {
            m.create_page(Parameters(CreatePageInput {
                project: Some("LST".into()),
                title: title.into(),
                content: None,
                folder: None,
                status: None,
                labels: None,
            }));
        }

        // A limit that exactly covers the remainder must NOT append a hint.
        let listing = m.list_resources(Parameters(ListResourcesInput {
            resource_type: "page".into(),
            project: Some("LST".into()),
            limit: Some(3),
            ..Default::default()
        }));
        assert!(listing.starts_with("3 pages:"), "got: {listing}");
        assert!(
            !listing.contains("more results available"),
            "hint should be absent when no more pages remain, got: {listing}"
        );
    }

    // ── list_comments author filter + sort ────────────────────────────────

    #[test]
    fn mcp_list_comments_author_filter() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "Authored");
        let _guard = seed_user(&m);
        m.add_comment(Parameters(AddCommentInput {
            identifier: "PRJ-1".into(),
            content: "Mine".into(),
        }));

        let mine = m.list_comments(Parameters(ListCommentsInput {
            identifier: "PRJ-1".into(),
            author: Some("testuser".into()),
            order: None,
            limit: None,
            offset: None,
        }));
        assert!(mine.contains("Mine"), "got: {mine}");

        let ghost = m.list_comments(Parameters(ListCommentsInput {
            identifier: "PRJ-1".into(),
            author: Some("ghost".into()),
            order: None,
            limit: None,
            offset: None,
        }));
        assert!(ghost.contains("No comments"), "got: {ghost}");
    }

    #[test]
    fn mcp_list_comments_desc_order() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "Threaded");
        let _guard = seed_user(&m);
        m.add_comment(Parameters(AddCommentInput {
            identifier: "PRJ-1".into(),
            content: "first".into(),
        }));
        m.add_comment(Parameters(AddCommentInput {
            identifier: "PRJ-1".into(),
            content: "second".into(),
        }));

        // Same-second timestamps tiebreak on id, so desc puts the later
        // comment first.
        let listing = m.list_comments(Parameters(ListCommentsInput {
            identifier: "PRJ-1".into(),
            author: None,
            order: Some("desc".into()),
            limit: None,
            offset: None,
        }));
        let second = listing.find("second").unwrap();
        let first = listing.find("first").unwrap();
        assert!(second < first, "desc must list newest first: {listing}");

        let bad = m.list_comments(Parameters(ListCommentsInput {
            identifier: "PRJ-1".into(),
            author: None,
            order: Some("newest".into()),
            limit: None,
            offset: None,
        }));
        assert!(bad.contains("Error"), "got: {bad}");
    }

    // ── LIF-143: edit_comment / delete_comment ─────────────────────────────

    /// Set `MCP_REQUEST_USER` to `user` (identity for the acting-user
    /// resolution in edit/delete_comment) and hand the handler lock back so
    /// the caller holds it for its whole body — same discipline as
    /// `seed_user`. Create a fresh user first, then call this.
    fn act_as(user: &models::User) -> tokio::sync::MutexGuard<'static, ()> {
        let guard = crate::mcp::MCP_HANDLER_LOCK.blocking_lock();
        *crate::mcp::MCP_REQUEST_USER
            .lock()
            .unwrap_or_else(|e: std::sync::PoisonError<_>| e.into_inner()) =
            Some(models::AuthUser {
                id: user.id,
                username: user.username.clone(),
                display_name: user.display_name.clone(),
                is_admin: user.is_admin,
            });
        guard
    }

    fn make_user(m: &LificMcp, username: &str, is_admin: bool) -> models::User {
        let conn = m.db.write().unwrap();
        let u = queries::users::create_user(
            &conn,
            &models::CreateUser {
                username: username.into(),
                email: format!("{username}@test.com"),
                password: "testpassword1".into(),
                display_name: Some(username.into()),
                is_admin,
                is_bot: false,
            },
        )
        .unwrap();
        drop(conn);
        u
    }

    /// Extract the "#N" comment id from an `add_comment` success string.
    fn comment_id_from(result: &str) -> i64 {
        result
            .split('#')
            .nth(1)
            .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or_else(|| panic!("no comment id in: {result}"))
    }

    #[test]
    fn edit_comment_author_can_edit_own() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "Editable");
        let author = make_user(&m, "author", false);
        let _guard = act_as(&author);

        let added = m.add_comment(Parameters(AddCommentInput {
            identifier: "PRJ-1".into(),
            content: "original".into(),
        }));
        let cid = comment_id_from(&added);

        let edited = m.edit_comment(Parameters(EditCommentInput {
            comment_id: cid,
            content: "revised".into(),
        }));
        assert!(
            edited.contains(&format!("Comment #{cid} edited")),
            "got: {edited}"
        );
        // LIF-115: must not echo the new content back.
        assert!(
            !edited.contains("revised"),
            "must not echo content: {edited}"
        );

        // The listing reflects the new body.
        let listing = m.list_comments(Parameters(ListCommentsInput {
            identifier: "PRJ-1".into(),
            ..Default::default()
        }));
        assert!(listing.contains("revised"), "got: {listing}");
        assert!(
            !listing.contains("original"),
            "old body should be gone: {listing}"
        );
    }

    #[test]
    fn delete_comment_author_can_delete_own() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "Deletable");
        let author = make_user(&m, "author", false);
        let _guard = act_as(&author);

        let added = m.add_comment(Parameters(AddCommentInput {
            identifier: "PRJ-1".into(),
            content: "delete me".into(),
        }));
        let cid = comment_id_from(&added);

        let deleted = m.delete_comment(Parameters(DeleteCommentInput { comment_id: cid }));
        assert!(
            deleted.contains(&format!("Comment #{cid} deleted")),
            "got: {deleted}"
        );

        let listing = m.list_comments(Parameters(ListCommentsInput {
            identifier: "PRJ-1".into(),
            ..Default::default()
        }));
        assert!(
            listing.contains("No comments"),
            "comment should be gone: {listing}"
        );
    }

    #[test]
    fn issue_comment_edit_and_delete_emit_updates() {
        let (m, mut events) = mcp_with_realtime();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "Realtime comments");
        let project_id = project_id_for(&m, "PRJ");
        let issue_id = issue_id_for(&m, "PRJ-1");
        let author = make_user(&m, "author", false);
        let _guard = act_as(&author);

        let added = m.add_comment(Parameters(AddCommentInput {
            identifier: "PRJ-1".into(),
            content: "original".into(),
        }));
        let comment_id = comment_id_from(&added);
        drain_realtime(&mut events);

        m.edit_comment(Parameters(EditCommentInput {
            comment_id,
            content: "revised".into(),
        }));
        assert_eq!(
            realtime_events(&mut events),
            vec![crate::realtime::RealtimeEvent::IssueUpdated {
                project_id,
                issue_id,
            }]
        );

        m.delete_comment(Parameters(DeleteCommentInput { comment_id }));
        assert_eq!(
            realtime_events(&mut events),
            vec![crate::realtime::RealtimeEvent::IssueUpdated {
                project_id,
                issue_id,
            }]
        );
    }

    #[test]
    fn edit_and_delete_comment_refuse_non_author_non_admin() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "Guarded");
        let author = make_user(&m, "author", false);
        let other = make_user(&m, "other", false);

        // Author posts a comment.
        let cid = {
            let _g = act_as(&author);
            let added = m.add_comment(Parameters(AddCommentInput {
                identifier: "PRJ-1".into(),
                content: "mine".into(),
            }));
            comment_id_from(&added)
        };

        // A non-author, non-admin is refused for both operations.
        let _guard = act_as(&other);
        let edit = m.edit_comment(Parameters(EditCommentInput {
            comment_id: cid,
            content: "hijacked".into(),
        }));
        assert!(
            edit.contains("Error") && edit.contains("only edit your own"),
            "non-author edit must be refused: {edit}"
        );

        let del = m.delete_comment(Parameters(DeleteCommentInput { comment_id: cid }));
        assert!(
            del.contains("Error") && del.contains("only delete your own"),
            "non-author delete must be refused: {del}"
        );

        // The comment survives untouched.
        let listing = m.list_comments(Parameters(ListCommentsInput {
            identifier: "PRJ-1".into(),
            ..Default::default()
        }));
        assert!(listing.contains("mine"), "comment must survive: {listing}");
    }

    #[test]
    fn admin_can_delete_another_users_comment() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "AdminTarget");
        let author = make_user(&m, "author", false);
        let admin = make_user(&m, "boss", true);

        let cid = {
            let _g = act_as(&author);
            let added = m.add_comment(Parameters(AddCommentInput {
                identifier: "PRJ-1".into(),
                content: "regular user's comment".into(),
            }));
            comment_id_from(&added)
        };

        let _guard = act_as(&admin);
        let deleted = m.delete_comment(Parameters(DeleteCommentInput { comment_id: cid }));
        assert!(
            deleted.contains("deleted"),
            "admin delete must succeed: {deleted}"
        );
    }

    #[test]
    fn edit_and_delete_comment_unknown_id_errors_cleanly() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "Empty");
        let author = make_user(&m, "author", false);
        let _guard = act_as(&author);

        let edit = m.edit_comment(Parameters(EditCommentInput {
            comment_id: 9999,
            content: "nope".into(),
        }));
        assert!(edit.contains("Error"), "unknown edit must error: {edit}");
        assert!(edit.contains("9999"), "error should name the id: {edit}");

        let del = m.delete_comment(Parameters(DeleteCommentInput { comment_id: 9999 }));
        assert!(del.contains("Error"), "unknown delete must error: {del}");
        assert!(del.contains("9999"), "error should name the id: {del}");
    }

    // ── search result_type filter, sort validation, pagination ────────────

    #[test]
    fn mcp_search_result_type_filter() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "findable widget issue");
        m.create_page(Parameters(CreatePageInput {
            project: Some("PRJ".into()),
            title: "findable widget page".into(),
            content: None,
            folder: None,
            status: None,
            labels: None,
        }));

        let issues_only = m.search(Parameters(SearchInput {
            query: "findable".into(),
            result_type: Some("issue".into()),
            ..Default::default()
        }));
        assert!(issues_only.contains("[issue]"), "got: {issues_only}");
        assert!(!issues_only.contains("[page]"), "got: {issues_only}");

        let bad = m.search(Parameters(SearchInput {
            query: "findable".into(),
            result_type: Some("widget".into()),
            ..Default::default()
        }));
        assert!(bad.contains("Error"), "got: {bad}");
    }

    #[test]
    fn mcp_search_pagination_emits_has_more_hint() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        for i in 0..3 {
            seed_issue(&m, "PRJ", &format!("paginated result {i}"));
        }

        let page1 = m.search(Parameters(SearchInput {
            query: "paginated".into(),
            limit: Some(2),
            ..Default::default()
        }));
        assert!(page1.contains("offset=2"), "got: {page1}");

        let page2 = m.search(Parameters(SearchInput {
            query: "paginated".into(),
            limit: Some(2),
            offset: Some(2),
            ..Default::default()
        }));
        assert!(!page2.contains("more results available"), "got: {page2}");
    }

    // ── list_issues date filters + sort control ───────────────────────────

    #[test]
    fn mcp_list_issues_date_filters() {
        let m = mcp();
        let ident = seed_project(&m, "Dated", "DAT");
        seed_issue(&m, &ident, "Recent issue");

        // Everything was just created, so a far-future since excludes all…
        let none = m.list_issues(Parameters(ListIssuesInput {
            project: ident.clone(),
            created_since: Some("2099-01-01".into()),
            ..Default::default()
        }));
        assert!(none.contains("No issues"), "got: {none}");

        // …and a far-past since includes them.
        let all = m.list_issues(Parameters(ListIssuesInput {
            project: ident,
            created_since: Some("2000-01-01".into()),
            ..Default::default()
        }));
        assert!(all.contains("Recent issue"), "got: {all}");
    }

    #[test]
    fn mcp_list_issues_order_by_sequence_desc() {
        let m = mcp();
        let ident = seed_project(&m, "Sorted", "SRT");
        seed_issue(&m, &ident, "Oldest");
        seed_issue(&m, &ident, "Newest");

        let listing = m.list_issues(Parameters(ListIssuesInput {
            project: ident.clone(),
            order_by: Some("sequence".into()),
            order: Some("desc".into()),
            ..Default::default()
        }));
        let newest = listing.find("Newest").unwrap();
        let oldest = listing.find("Oldest").unwrap();
        assert!(newest < oldest, "got: {listing}");

        let bad = m.list_issues(Parameters(ListIssuesInput {
            project: ident,
            order_by: Some("votes".into()),
            ..Default::default()
        }));
        assert!(bad.contains("Error"), "got: {bad}");
    }

    // ── get_activity (LIF-156) ──

    #[test]
    fn get_activity_renders_issue_history() {
        let m = mcp();
        seed_project(&m, "Audit", "TST");
        seed_issue(&m, "TST", "Watched issue");

        let result = m.update_issue(Parameters(UpdateIssueInput {
            identifier: "TST-1".into(),
            status: Some("active".into()),
            ..Default::default()
        }));
        assert!(result.contains("Updated"), "got: {result}");

        let out = m.get_activity(Parameters(GetActivityInput {
            identifier: "TST-1".into(),
            ..Default::default()
        }));
        assert!(out.contains("status: backlog → active"), "got: {out}");
        assert!(out.contains("created issue TST-1"), "got: {out}");
    }

    #[test]
    fn get_activity_project_feed_pages_with_hint() {
        let m = mcp();
        seed_project(&m, "Audit", "TST");
        for i in 0..4 {
            seed_issue(&m, "TST", &format!("issue {i}"));
        }

        let out = m.get_activity(Parameters(GetActivityInput {
            identifier: "TST".into(),
            limit: Some(2),
            ..Default::default()
        }));
        assert!(out.contains("2 activity entries"), "got: {out}");
        assert!(out.contains("offset=2"), "paging hint expected: {out}");

        let next = m.get_activity(Parameters(GetActivityInput {
            identifier: "TST".into(),
            limit: Some(50),
            offset: Some(2),
        }));
        // 5 total (project create + 4 issues) − 2 already seen = 3.
        assert!(next.contains("3 activity entries"), "got: {next}");
        assert!(!next.contains("offset="), "no further hint: {next}");
    }

    #[test]
    fn get_activity_rejects_unknown_identifier() {
        let m = mcp();
        seed_project(&m, "Audit", "TST");
        let out = m.get_activity(Parameters(GetActivityInput {
            identifier: "NOPE-999".into(),
            ..Default::default()
        }));
        assert!(out.starts_with("Error"), "got: {out}");
    }

    #[tokio::test]
    async fn get_activity_attributes_mcp_actor() {
        let m = mcp();
        seed_project(&m, "Audit", "TST");

        // Seed a bot user, then act through the production MCP identity
        // wrapper — the audit entry must say "<bot> (agent) via mcp".
        let bot_id = {
            let conn = m.db.write().unwrap();
            conn.execute(
                "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot)
                 VALUES ('opencode-blake', 'oc@test.local', 'x', 'opencode-blake', 0, 1)",
                [],
            )
            .unwrap();
            conn.last_insert_rowid()
        };
        let user = crate::db::models::AuthUser {
            id: bot_id,
            username: "opencode-blake".into(),
            display_name: "opencode-blake".into(),
            is_admin: false,
        };

        crate::mcp::with_request_user(Some(user), || async {
            seed_issue(&m, "TST", "Agent-made");
        })
        .await;

        let out = m.get_activity(Parameters(GetActivityInput {
            identifier: "TST-1".into(),
            ..Default::default()
        }));
        assert!(out.contains("opencode-blake (agent) via mcp"), "got: {out}");
    }

    /// Regression (LIF-155): rmcp executes tools on internally-spawned
    /// tasks, where tokio task-locals set around `service.handle()` are
    /// invisible. Attribution must therefore come from the serialized
    /// MCP_REQUEST_USER global via LificMcp::write()'s explicit re-stamp.
    /// This test reproduces the boundary with a literal tokio::spawn.
    #[tokio::test]
    async fn mcp_attribution_survives_task_spawn() {
        let m = mcp();
        seed_project(&m, "Audit", "TST");

        let bot_id = {
            let conn = m.db.write().unwrap();
            conn.execute(
                "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot)
                 VALUES ('opencode-blake', 'oc@test.local', 'x', 'opencode-blake', 0, 1)",
                [],
            )
            .unwrap();
            conn.last_insert_rowid()
        };
        let user = crate::db::models::AuthUser {
            id: bot_id,
            username: "opencode-blake".into(),
            display_name: "opencode-blake".into(),
            is_admin: false,
        };

        crate::mcp::with_request_user(Some(user), || async {
            let m2 = m.clone();
            // The spawned task has NO task-local actor — like production.
            tokio::spawn(async move {
                let result = m2.create_issue(Parameters(CreateIssueInput {
                    project: "TST".into(),
                    title: "Spawned write".into(),
                    description: None,
                    status: None,
                    priority: None,
                    module: None,
                    labels: None,
                    ..Default::default()
                }));
                assert!(result.starts_with("Created"), "got: {result}");
            })
            .await
            .unwrap();
        })
        .await;

        let out = m.get_activity(Parameters(GetActivityInput {
            identifier: "TST-1".into(),
            ..Default::default()
        }));
        assert!(
            out.contains("opencode-blake (agent) via mcp"),
            "spawned tool write must still attribute: {out}"
        );
    }

    // ── Plans (LIF-168/169/170/171) ──

    #[test]
    fn create_plan_authors_nested_tree_and_get_plan_rehydrates() {
        let m = mcp();
        seed_project(&m, "Plans", "PLN");

        let created = m.create_plan(Parameters(CreatePlanInput {
            project: "PLN".into(),
            title: "Ship feature".into(),
            anchor_issue: None,
            steps: Some(vec![
                PlanStepInput {
                    title: "Backend".into(),
                    steps: Some(vec![
                        PlanStepInput {
                            title: "schema".into(),
                            ..Default::default()
                        },
                        PlanStepInput {
                            title: "queries".into(),
                            ..Default::default()
                        },
                    ]),
                    ..Default::default()
                },
                PlanStepInput {
                    title: "Frontend".into(),
                    ..Default::default()
                },
            ]),
        }));
        assert!(created.contains("PLN-PLAN-1"), "got: {created}");
        assert!(created.contains("Backend"));
        assert!(created.contains("schema"));

        let got = m.get_plan(Parameters(GetPlanInput {
            plan: "PLN-PLAN-1".into(),
        }));
        assert!(
            got.contains("Frontend"),
            "get_plan should rehydrate tree: {got}"
        );
        assert!(got.contains("0/4 done"), "header should count steps: {got}");
    }

    #[test]
    fn update_plan_step_done_closes_linked_issue_and_narrates() {
        let m = mcp();
        seed_project(&m, "Plans", "PLN");
        seed_issue(&m, "PLN", "Real work"); // PLN-1

        let created = m.create_plan(Parameters(CreatePlanInput {
            project: "PLN".into(),
            title: "Plan".into(),
            anchor_issue: None,
            steps: Some(vec![PlanStepInput {
                title: "mirror".into(),
                issue: Some("PLN-1".into()),
                ..Default::default()
            }]),
        }));
        // Pull the step id from the rendered "#N".
        let step_id: i64 = created
            .split('#')
            .nth(1)
            .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|s| s.parse().ok())
            .expect("step id in output");

        let out = m.update_plan_step(Parameters(UpdatePlanStepInput {
            plan: "PLN-PLAN-1".into(),
            step_id: Some(step_id),
            done: Some(true),
            ..Default::default()
        }));
        assert!(
            out.contains("→ PLN-1 marked done"),
            "must narrate the issue side effect: {out}"
        );
        // LIF-302: default output is a compact receipt — notes plus a one-line
        // progress summary, and NO step tree lines.
        assert!(
            out.contains("PLN-PLAN-1 [active]: 1/1 done"),
            "receipt must carry progress counts: {out}"
        );
        // No step-tree lines (the "- [x] #N" render fmt_plan would emit).
        assert!(
            !out.contains("- [x]"),
            "receipt must omit step lines: {out}"
        );

        // The issue is actually closed.
        let issue = m.get_issue(Parameters(GetIssueInput {
            identifier: "PLN-1".into(),
            ..Default::default()
        }));
        assert!(issue.contains("done"), "issue should be done: {issue}");
    }

    // LIF-302: echo_tree=true restores the full re-rendered plan tree.
    #[test]
    fn update_plan_step_echo_tree_returns_full_tree() {
        let m = mcp();
        seed_project(&m, "Plans", "PET");
        let created = m.create_plan(Parameters(CreatePlanInput {
            project: "PET".into(),
            title: "Tree plan".into(),
            anchor_issue: None,
            steps: Some(vec![
                PlanStepInput {
                    title: "first step".into(),
                    ..Default::default()
                },
                PlanStepInput {
                    title: "second step".into(),
                    ..Default::default()
                },
            ]),
        }));
        let step_id: i64 = created
            .split('#')
            .nth(1)
            .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|s| s.parse().ok())
            .expect("step id in output");

        let out = m.update_plan_step(Parameters(UpdatePlanStepInput {
            plan: "PET-PLAN-1".into(),
            step_id: Some(step_id),
            done: Some(true),
            echo_tree: Some(true),
            ..Default::default()
        }));
        // The full tree renders both step titles and step-line markers.
        assert!(out.contains("first step"), "got: {out}");
        assert!(out.contains("second step"), "got: {out}");
        assert!(
            out.contains("- ["),
            "echo_tree must render step lines: {out}"
        );
    }

    // LIF-302: a plan-level update (no step_id) also returns the compact
    // receipt, not the tree.
    #[test]
    fn update_plan_step_plan_level_returns_receipt() {
        let m = mcp();
        seed_project(&m, "Plans", "PPR");
        m.create_plan(Parameters(CreatePlanInput {
            project: "PPR".into(),
            title: "Rename me".into(),
            anchor_issue: None,
            steps: Some(vec![PlanStepInput {
                title: "only step".into(),
                ..Default::default()
            }]),
        }));

        let out = m.update_plan_step(Parameters(UpdatePlanStepInput {
            plan: "PPR-PLAN-1".into(),
            step_id: None,
            status: Some("done".into()),
            ..Default::default()
        }));
        assert!(
            out.contains("PPR-PLAN-1 [done]: 0/1 done"),
            "plan-level receipt must carry status + progress: {out}"
        );
        // No step tree — the only step's title must not appear.
        assert!(
            !out.contains("only step"),
            "receipt must omit the tree: {out}"
        );
    }

    #[test]
    fn update_plan_step_done_emits_issue_update() {
        let (m, mut rx) = mcp_with_realtime();
        seed_project(&m, "Plan Events", "PLE");
        seed_issue(&m, "PLE", "Real work");
        let project_id = project_id_for(&m, "PLE");
        let issue_id = issue_id_for(&m, "PLE-1");

        let created = m.create_plan(Parameters(CreatePlanInput {
            project: "PLE".into(),
            title: "Plan".into(),
            anchor_issue: None,
            steps: Some(vec![PlanStepInput {
                title: "mirror".into(),
                issue: Some("PLE-1".into()),
                ..Default::default()
            }]),
        }));
        let step_id: i64 = created
            .split('#')
            .nth(1)
            .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|s| s.parse().ok())
            .expect("step id in output");
        drain_realtime(&mut rx);

        m.update_plan_step(Parameters(UpdatePlanStepInput {
            plan: "PLE-PLAN-1".into(),
            step_id: Some(step_id),
            done: Some(true),
            ..Default::default()
        }));

        assert_eq!(
            realtime_events(&mut rx),
            vec![
                crate::realtime::RealtimeEvent::IssueUpdated {
                    project_id,
                    issue_id,
                },
                crate::realtime::RealtimeEvent::ProjectUpdated { project_id },
            ]
        );
    }

    #[test]
    fn edit_plan_step_find_replace() {
        let m = mcp();
        seed_project(&m, "Plans", "PLN");
        let created = m.create_plan(Parameters(CreatePlanInput {
            project: "PLN".into(),
            title: "Plan".into(),
            anchor_issue: None,
            steps: Some(vec![PlanStepInput {
                title: "step".into(),
                description: Some("old text".into()),
                ..Default::default()
            }]),
        }));
        let step_id: i64 = created
            .split('#')
            .nth(1)
            .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|s| s.parse().ok())
            .unwrap();

        let out = m.edit_plan_step(Parameters(EditPlanStepInput {
            plan: "PLN-PLAN-1".into(),
            step_id,
            old_string: "old text".into(),
            new_string: "new text".into(),
            field: None,
            replace_all: None,
        }));
        assert!(out.contains("Edited step"), "got: {out}");
        let got = m.get_plan(Parameters(GetPlanInput {
            plan: "PLN-PLAN-1".into(),
        }));
        assert!(got.contains("new text"), "edit should persist: {got}");
    }

    #[test]
    fn plan_level_update_archives_and_lists() {
        let m = mcp();
        seed_project(&m, "Plans", "PLN");
        m.create_plan(Parameters(CreatePlanInput {
            project: "PLN".into(),
            title: "Plan".into(),
            anchor_issue: None,
            steps: None,
        }));

        // Plan-level status change (no step_id).
        let out = m.update_plan_step(Parameters(UpdatePlanStepInput {
            plan: "PLN-PLAN-1".into(),
            step_id: None,
            status: Some("archived".into()),
            ..Default::default()
        }));
        assert!(out.contains("archived"), "got: {out}");

        // Listing via list_resources filters by status.
        let active = m.list_resources(Parameters(ListResourcesInput {
            resource_type: "plan".into(),
            project: Some("PLN".into()),
            status: Some("active".into()),
            ..Default::default()
        }));
        assert!(
            active.contains("No plans found."),
            "archived plan must not show as active: {active}"
        );

        let archived = m.list_resources(Parameters(ListResourcesInput {
            resource_type: "plan".into(),
            project: Some("PLN".into()),
            status: Some("archived".into()),
            ..Default::default()
        }));
        assert!(archived.contains("PLN-PLAN-1"), "got: {archived}");
    }

    // LIF-175: end-to-end across surfaces — a plan authored via MCP, then the
    // linked issue closed via the issue tool, must show the step auto-completed
    // with provenance when the plan is rehydrated.
    #[test]
    fn closing_issue_autocompletes_step_visible_in_get_plan() {
        let m = mcp();
        seed_project(&m, "Plans", "PLN");
        seed_issue(&m, "PLN", "Mirrored work"); // PLN-1
        let created = m.create_plan(Parameters(CreatePlanInput {
            project: "PLN".into(),
            title: "Plan".into(),
            anchor_issue: None,
            steps: Some(vec![PlanStepInput {
                title: "mirror".into(),
                issue: Some("PLN-1".into()),
                ..Default::default()
            }]),
        }));
        let step_id: i64 = created
            .split('#')
            .nth(1)
            .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|s| s.parse().ok())
            .expect("step id in output");

        // Close the issue through the normal issue tool.
        let updated = m.update_issue(Parameters(UpdateIssueInput {
            identifier: "PLN-1".into(),
            status: Some("done".into()),
            ..Default::default()
        }));
        assert!(
            updated.contains(&format!(
                "auto-completed plan step #{step_id} in PLN-PLAN-1"
            )),
            "issue update must narrate the cascade: {updated}"
        );

        let got = m.get_plan(Parameters(GetPlanInput {
            plan: "PLN-PLAN-1".into(),
        }));
        assert!(got.contains("[x]"), "step should be auto-completed: {got}");
        assert!(got.contains("via PLN-1"), "provenance should show: {got}");
    }

    #[test]
    fn reopening_issue_narrates_reopened_plan_step() {
        let m = mcp();
        seed_project(&m, "Plans", "RPN");
        seed_issue(&m, "RPN", "Mirrored work"); // RPN-1
        let created = m.create_plan(Parameters(CreatePlanInput {
            project: "RPN".into(),
            title: "Plan".into(),
            anchor_issue: None,
            steps: Some(vec![PlanStepInput {
                title: "mirror".into(),
                issue: Some("RPN-1".into()),
                ..Default::default()
            }]),
        }));
        let step_id: i64 = created
            .split('#')
            .nth(1)
            .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|s| s.parse().ok())
            .expect("step id in output");

        m.update_issue(Parameters(UpdateIssueInput {
            identifier: "RPN-1".into(),
            status: Some("done".into()),
            ..Default::default()
        }));
        let reopened = m.update_issue(Parameters(UpdateIssueInput {
            identifier: "RPN-1".into(),
            status: Some("todo".into()),
            ..Default::default()
        }));

        assert!(
            reopened.contains(&format!("reopened plan step #{step_id} in RPN-PLAN-1")),
            "issue reopen must narrate the cascade: {reopened}"
        );
    }

    #[test]
    fn closing_issue_skips_steps_in_archived_plans_without_note() {
        let m = mcp();
        seed_project(&m, "Plans", "ARC");
        seed_issue(&m, "ARC", "Mirrored work"); // ARC-1
        m.create_plan(Parameters(CreatePlanInput {
            project: "ARC".into(),
            title: "Archived plan".into(),
            anchor_issue: None,
            steps: Some(vec![PlanStepInput {
                title: "mirror".into(),
                issue: Some("ARC-1".into()),
                ..Default::default()
            }]),
        }));
        m.update_plan_step(Parameters(UpdatePlanStepInput {
            plan: "ARC-PLAN-1".into(),
            status: Some("archived".into()),
            ..Default::default()
        }));

        let updated = m.update_issue(Parameters(UpdateIssueInput {
            identifier: "ARC-1".into(),
            status: Some("done".into()),
            ..Default::default()
        }));

        assert_eq!(
            updated, "Updated ARC-1: ARC-1 | done | none | Mirrored work",
            "archived-plan steps are frozen and must not produce a note"
        );
        let plan = m.get_plan(Parameters(GetPlanInput {
            plan: "ARC-PLAN-1".into(),
        }));
        assert!(
            plan.contains("- [ ]"),
            "archived step must remain open: {plan}"
        );
    }

    #[test]
    fn delete_plan_via_delete_tool() {
        let m = mcp();
        seed_project(&m, "Plans", "PLN");
        m.create_plan(Parameters(CreatePlanInput {
            project: "PLN".into(),
            title: "Doomed".into(),
            anchor_issue: None,
            steps: None,
        }));
        let out = m.delete(Parameters(DeleteInput {
            resource_type: "plan".into(),
            identifier: "PLN-PLAN-1".into(),
            project: None,
        }));
        assert!(out.contains("Deleted plan"), "got: {out}");
        let got = m.get_plan(Parameters(GetPlanInput {
            plan: "PLN-PLAN-1".into(),
        }));
        assert!(
            got.contains("Error"),
            "deleted plan should not be found: {got}"
        );
    }

    // ── LIF-299: MCP outputs never HTML-escape stored text ──
    //
    // No code path in this repo escapes; the `&amp;`/`&lt;` seen in the wild
    // arrived pre-escaped from writer clients (stored data). These are
    // regression guards: raw punctuation the agent stores must round-trip
    // through every read surface verbatim, and the HTML entities must never
    // appear. If any of these fail, a formatter started escaping — fix it
    // there, not by weakening the test.

    /// The gnarly title used across the guards: ampersand, angle-bracketed
    /// generic, and double quotes.
    const RAW_TITLE: &str = r#"Fix & polish <Store<T>> "quotes""#;

    /// Assert `s` contains the raw needles and none of the HTML entities.
    fn assert_no_html_escape(s: &str, needles: &[&str]) {
        for n in needles {
            assert!(s.contains(n), "missing raw {n:?} in: {s}");
        }
        for ent in ["&amp;", "&lt;", "&gt;", "&quot;", "&#"] {
            assert!(!s.contains(ent), "found HTML entity {ent:?} in: {s}");
        }
    }

    #[test]
    fn no_html_escape_across_issue_read_surfaces() {
        let m = mcp();
        seed_project(&m, "Escape", "ESC");
        let created = m.create_issue(Parameters(CreateIssueInput {
            project: "ESC".into(),
            title: RAW_TITLE.into(),
            description: Some(r#"body with & and < and > and "quotes""#.into()),
            ..Default::default()
        }));
        // create_issue echoes the title.
        assert_no_html_escape(&created, &[RAW_TITLE]);

        // get_board
        let board = m.get_board(Parameters(GetBoardInput {
            project: "ESC".into(),
            ..Default::default()
        }));
        assert_no_html_escape(&board, &[RAW_TITLE]);

        // list_resources (issue path → fmt_issue)
        let listed = m.list_resources(Parameters(ListResourcesInput {
            resource_type: "issue".into(),
            project: Some("ESC".into()),
            ..Default::default()
        }));
        assert_no_html_escape(&listed, &[RAW_TITLE]);

        // list_issues (fmt_issue path)
        let issues = m.list_issues(Parameters(ListIssuesInput {
            project: "ESC".into(),
            ..Default::default()
        }));
        assert_no_html_escape(&issues, &[RAW_TITLE]);

        // get_issue (title + description)
        let detail = m.get_issue(Parameters(GetIssueInput {
            identifier: "ESC-1".into(),
            ..Default::default()
        }));
        assert_no_html_escape(&detail, &[RAW_TITLE, "body with & and < and >"]);

        // search — both modes must return the raw text.
        let fts = m.search(Parameters(SearchInput {
            query: "polish".into(),
            ..Default::default()
        }));
        assert_no_html_escape(&fts, &["Fix & polish"]);
        let lit = m.search(Parameters(SearchInput {
            query: "<Store<T>>".into(),
            mode: Some("literal".into()),
            ..Default::default()
        }));
        assert_no_html_escape(&lit, &["<Store<T>>"]);
    }

    #[test]
    fn no_html_escape_in_comment_surfaces() {
        let m = mcp();
        seed_project(&m, "Escape", "ESC");
        seed_issue(&m, "ESC", "Host issue");
        let _guard = seed_user(&m);

        let raw_comment = r#"needs & review of <T> before "ship""#;
        m.add_comment(Parameters(AddCommentInput {
            identifier: "ESC-1".into(),
            content: raw_comment.into(),
        }));

        // via get_issue
        let detail = m.get_issue(Parameters(GetIssueInput {
            identifier: "ESC-1".into(),
            include_comments: Some("all".into()),
        }));
        assert_no_html_escape(&detail, &[raw_comment]);

        // via list_comments
        let comments = m.list_comments(Parameters(ListCommentsInput {
            identifier: "ESC-1".into(),
            ..Default::default()
        }));
        assert_no_html_escape(&comments, &[raw_comment]);
    }

    #[test]
    fn no_html_escape_in_plan_step_title() {
        let m = mcp();
        seed_project(&m, "Escape", "ESC");
        let raw_step = r#"land A & B <fast> "now""#;
        let created = m.create_plan(Parameters(CreatePlanInput {
            project: "ESC".into(),
            title: "Escaping plan".into(),
            anchor_issue: None,
            steps: Some(vec![PlanStepInput {
                title: raw_step.into(),
                ..Default::default()
            }]),
        }));
        assert_no_html_escape(&created, &[raw_step]);

        // via get_plan
        let plan = m.get_plan(Parameters(GetPlanInput {
            plan: "ESC-PLAN-1".into(),
        }));
        assert_no_html_escape(&plan, &[raw_step]);
    }
}

/// LIF-198: project-scoped authorization enforcement across every MCP tool.
/// Flag-ON cases mirror `api::mod.rs`'s `authz_gating_tests` matrix
/// byte-for-byte (same fixture — `setup_membership_mcp`, wrapping
/// `api::test_helpers::setup_membership_test`); the flag-OFF smoke test is
/// the regression proof that MCP's historical (fully open) behavior hasn't
/// moved — the 93 pre-existing `mcp::tools::tests` above all run flag-OFF
/// by default and already prove that in full.
#[cfg(test)]
mod authz_gating_tests {
    use super::tests::setup_membership_mcp;
    use super::*;
    use rmcp::handler::server::wrapper::Parameters;

    fn run<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }

    /// Run `f` as `user` via the production MCP identity wrapper, return
    /// the tool's string result.
    fn as_user(user: &models::AuthUser, f: impl FnOnce() -> String) -> String {
        run(crate::mcp::with_request_user(
            Some(user.clone()),
            || async { f() },
        ))
    }

    fn is_forbidden(s: &str) -> bool {
        s.starts_with("Error: Forbidden:")
    }

    // ── bulk_update (LIF-24): project-scoped Maintainer gate ─────

    #[test]
    fn bulk_update_denies_non_member_when_enforced() {
        let (m, _admin, _lead, maintainer, _viewer, non_member, _project_id) =
            setup_membership_mcp();
        // Seed an active issue as a permitted member.
        let created = as_user(&maintainer, || {
            m.create_issue(Parameters(CreateIssueInput {
                project: "MEM".into(),
                title: "Target".into(),
                status: Some("active".into()),
                priority: None,
                description: None,
                module: None,
                labels: None,
                ..Default::default()
            }))
        });
        assert!(created.starts_with("Created"), "got: {created}");

        // Non-member is refused with enforcement on (mirrors update_issue).
        let denied = as_user(&non_member, || {
            m.bulk_update(Parameters(BulkUpdateInput {
                project: "MEM".into(),
                filter_status: Some("active".into()),
                set_status: Some("done".into()),
                ..Default::default()
            }))
        });
        assert!(is_forbidden(&denied), "got: {denied}");

        // A maintainer is allowed and updates the match.
        let allowed = as_user(&maintainer, || {
            m.bulk_update(Parameters(BulkUpdateInput {
                project: "MEM".into(),
                filter_status: Some("active".into()),
                set_status: Some("done".into()),
                ..Default::default()
            }))
        });
        assert_eq!(allowed, "Updated 1 issue(s)", "got: {allowed}");
    }

    // ── Reads: single-resource Viewer gate ──────────────────────

    #[test]
    fn issue_read_denies_non_member_allows_viewer() {
        let (m, _admin, lead, _maintainer, viewer, non_member, project_id) = setup_membership_mcp();
        let _ = project_id;
        let created = as_user(&lead, || {
            m.create_issue(Parameters(CreateIssueInput {
                project: "MEM".into(),
                title: "Secret work".into(),
                description: None,
                status: None,
                priority: None,
                module: None,
                labels: None,
                ..Default::default()
            }))
        });
        assert!(created.starts_with("Created"), "got: {created}");

        let denied = as_user(&non_member, || {
            m.get_issue(Parameters(GetIssueInput {
                identifier: "MEM-1".into(),
                ..Default::default()
            }))
        });
        assert!(is_forbidden(&denied), "non-member get_issue: {denied}");

        let allowed = as_user(&viewer, || {
            m.get_issue(Parameters(GetIssueInput {
                identifier: "MEM-1".into(),
                ..Default::default()
            }))
        });
        assert!(!is_forbidden(&allowed), "viewer get_issue: {allowed}");
        assert!(allowed.contains("Secret work"), "got: {allowed}");
    }

    #[test]
    fn page_and_plan_reads_follow_the_same_viewer_gate() {
        let (m, _admin, lead, _maintainer, viewer, non_member, _project_id) =
            setup_membership_mcp();
        let page = as_user(&lead, || {
            m.create_page(Parameters(CreatePageInput {
                project: Some("MEM".into()),
                title: "Doc".into(),
                content: None,
                folder: None,
                status: None,
                labels: None,
            }))
        });
        assert!(page.starts_with("Created"), "got: {page}");
        let plan = as_user(&lead, || {
            m.create_plan(Parameters(CreatePlanInput {
                project: "MEM".into(),
                title: "Plan".into(),
                anchor_issue: None,
                steps: None,
            }))
        });
        assert!(plan.starts_with("Created"), "got: {plan}");

        let denied_page = as_user(&non_member, || {
            m.get_page(Parameters(GetPageInput {
                identifier: "MEM-DOC-1".into(),
            }))
        });
        assert!(is_forbidden(&denied_page), "got: {denied_page}");
        let denied_plan = as_user(&non_member, || {
            m.get_plan(Parameters(GetPlanInput {
                plan: "MEM-PLAN-1".into(),
            }))
        });
        assert!(is_forbidden(&denied_plan), "got: {denied_plan}");

        let allowed_page = as_user(&viewer, || {
            m.get_page(Parameters(GetPageInput {
                identifier: "MEM-DOC-1".into(),
            }))
        });
        assert!(!is_forbidden(&allowed_page), "got: {allowed_page}");
        let allowed_plan = as_user(&viewer, || {
            m.get_plan(Parameters(GetPlanInput {
                plan: "MEM-PLAN-1".into(),
            }))
        });
        assert!(!is_forbidden(&allowed_plan), "got: {allowed_plan}");
    }

    // ── Reads: cross-project search/list filter instead of denying ──

    #[test]
    fn search_and_list_resources_project_filter_instead_of_denying() {
        let (m, _admin, lead, _maintainer, viewer, non_member, _project_id) =
            setup_membership_mcp();
        let created = as_user(&lead, || {
            m.create_issue(Parameters(CreateIssueInput {
                project: "MEM".into(),
                title: "Unique searchable xyzzy".into(),
                description: None,
                status: None,
                priority: None,
                module: None,
                labels: None,
                ..Default::default()
            }))
        });
        assert!(created.starts_with("Created"), "got: {created}");

        // search: non-member sees nothing, never an error.
        let denied = as_user(&non_member, || {
            m.search(Parameters(SearchInput {
                query: "xyzzy".into(),
                ..Default::default()
            }))
        });
        assert!(!is_forbidden(&denied), "search must not error: {denied}");
        assert!(denied.contains("No results"), "got: {denied}");

        let found = as_user(&viewer, || {
            m.search(Parameters(SearchInput {
                query: "xyzzy".into(),
                ..Default::default()
            }))
        });
        assert!(found.contains("1 results"), "got: {found}");

        // list_resources(type="project"): non-member sees 0, member sees theirs.
        let none_visible = as_user(&non_member, || {
            m.list_resources(Parameters(ListResourcesInput {
                resource_type: "project".into(),
                ..Default::default()
            }))
        });
        assert!(
            none_visible.starts_with("0 projects"),
            "got: {none_visible}"
        );

        let one_visible = as_user(&viewer, || {
            m.list_resources(Parameters(ListResourcesInput {
                resource_type: "project".into(),
                ..Default::default()
            }))
        });
        assert!(one_visible.starts_with("1 projects"), "got: {one_visible}");
    }

    /// LIF-257: the onboarding nudge fires only on a genuinely empty DB. A
    /// user who can see no projects (but projects DO exist) must keep the
    /// current "0 projects" / "No results" output — nudging would leak that
    /// projects exist and mislead a real member.
    #[test]
    fn nudge_not_shown_when_projects_exist_but_none_visible() {
        let (m, _admin, _lead, _maintainer, _viewer, non_member, _project_id) =
            setup_membership_mcp();

        let projects = as_user(&non_member, || {
            m.list_resources(Parameters(ListResourcesInput {
                resource_type: "project".into(),
                ..Default::default()
            }))
        });
        assert!(
            !projects.contains(NO_PROJECTS_NUDGE),
            "leaked nudge: {projects}"
        );
        assert!(projects.starts_with("0 projects"), "got: {projects}");

        let searched = as_user(&non_member, || {
            m.search(Parameters(SearchInput {
                query: "anything".into(),
                ..Default::default()
            }))
        });
        assert!(
            !searched.contains(NO_PROJECTS_NUDGE),
            "leaked nudge: {searched}"
        );
    }

    // ── Writes: content mutations gated at Maintainer ────────────

    #[test]
    fn issue_create_gated_by_maintainer_role() {
        let (m, admin, lead, maintainer, viewer, non_member, _project_id) = setup_membership_mcp();

        for (user, expect_ok) in [
            (&non_member, false),
            (&viewer, false),
            (&maintainer, true),
            (&lead, true),
            (&admin, true),
        ] {
            let result = as_user(user, || {
                m.create_issue(Parameters(CreateIssueInput {
                    project: "MEM".into(),
                    title: format!("by {}", user.username),
                    description: None,
                    status: None,
                    priority: None,
                    module: None,
                    labels: None,
                    ..Default::default()
                }))
            });
            assert_eq!(
                is_forbidden(&result),
                !expect_ok,
                "{} create_issue expected ok={expect_ok}, got: {result}",
                user.username
            );
        }
    }

    #[test]
    fn issue_update_and_delete_gated_by_maintainer_role() {
        let (m, _admin, lead, maintainer, viewer, non_member, _project_id) = setup_membership_mcp();
        let created = as_user(&maintainer, || {
            m.create_issue(Parameters(CreateIssueInput {
                project: "MEM".into(),
                title: "Target".into(),
                description: None,
                status: None,
                priority: None,
                module: None,
                labels: None,
                ..Default::default()
            }))
        });
        assert!(created.starts_with("Created"), "got: {created}");

        let denied = as_user(&viewer, || {
            m.update_issue(Parameters(UpdateIssueInput {
                identifier: "MEM-1".into(),
                title: Some("hijack".into()),
                ..Default::default()
            }))
        });
        assert!(is_forbidden(&denied), "got: {denied}");
        let denied2 = as_user(&non_member, || {
            m.update_issue(Parameters(UpdateIssueInput {
                identifier: "MEM-1".into(),
                title: Some("hijack".into()),
                ..Default::default()
            }))
        });
        assert!(is_forbidden(&denied2), "got: {denied2}");

        let allowed = as_user(&lead, || {
            m.update_issue(Parameters(UpdateIssueInput {
                identifier: "MEM-1".into(),
                title: Some("renamed".into()),
                ..Default::default()
            }))
        });
        assert!(!is_forbidden(&allowed), "got: {allowed}");

        let denied_delete = as_user(&viewer, || {
            m.delete(Parameters(DeleteInput {
                resource_type: "issue".into(),
                identifier: "MEM-1".into(),
                project: None,
            }))
        });
        assert!(is_forbidden(&denied_delete), "got: {denied_delete}");

        let allowed_delete = as_user(&lead, || {
            m.delete(Parameters(DeleteInput {
                resource_type: "issue".into(),
                identifier: "MEM-1".into(),
                project: None,
            }))
        });
        assert!(!is_forbidden(&allowed_delete), "got: {allowed_delete}");
    }

    // ── Comments: Viewer can read + create; non-member cannot ───

    #[test]
    fn comment_create_allows_viewer_denies_non_member() {
        let (m, _admin, lead, _maintainer, viewer, non_member, _project_id) =
            setup_membership_mcp();
        let created = as_user(&lead, || {
            m.create_issue(Parameters(CreateIssueInput {
                project: "MEM".into(),
                title: "Commentable".into(),
                description: None,
                status: None,
                priority: None,
                module: None,
                labels: None,
                ..Default::default()
            }))
        });
        assert!(created.starts_with("Created"), "got: {created}");

        let allowed = as_user(&viewer, || {
            m.add_comment(Parameters(AddCommentInput {
                identifier: "MEM-1".into(),
                content: "viewers can comment".into(),
            }))
        });
        assert!(
            !is_forbidden(&allowed),
            "viewer must be allowed to comment: {allowed}"
        );

        let denied = as_user(&non_member, || {
            m.add_comment(Parameters(AddCommentInput {
                identifier: "MEM-1".into(),
                content: "should not land".into(),
            }))
        });
        assert!(is_forbidden(&denied), "got: {denied}");
    }

    // ── Structure endpoints: loosened to Maintainer once enforced ──

    #[test]
    fn structure_endpoints_viewer_denied_maintainer_allowed() {
        let (m, _admin, _lead, maintainer, viewer, non_member, _project_id) =
            setup_membership_mcp();

        let denied = as_user(&viewer, || {
            m.manage_resource(Parameters(ManageResourceInput {
                resource_type: "module".into(),
                action: "create".into(),
                project: Some("MEM".into()),
                name: Some("Nope".into()),
                identifier: None,
                description: None,
                current_name: None,
                status: None,
                color: None,
                emoji: None,
            }))
        });
        assert!(is_forbidden(&denied), "got: {denied}");
        let denied2 = as_user(&non_member, || {
            m.manage_resource(Parameters(ManageResourceInput {
                resource_type: "label".into(),
                action: "create".into(),
                project: Some("MEM".into()),
                name: Some("nope".into()),
                identifier: None,
                description: None,
                current_name: None,
                status: None,
                color: None,
                emoji: None,
            }))
        });
        assert!(is_forbidden(&denied2), "got: {denied2}");

        let allowed = as_user(&maintainer, || {
            m.manage_resource(Parameters(ManageResourceInput {
                resource_type: "module".into(),
                action: "create".into(),
                project: Some("MEM".into()),
                name: Some("Backend".into()),
                identifier: None,
                description: None,
                current_name: None,
                status: None,
                color: None,
                emoji: None,
            }))
        });
        assert!(
            !is_forbidden(&allowed),
            "maintainer should manage structure once enforcement loosens the gate: {allowed}"
        );
        let allowed2 = as_user(&maintainer, || {
            m.manage_resource(Parameters(ManageResourceInput {
                resource_type: "folder".into(),
                action: "create".into(),
                project: Some("MEM".into()),
                name: Some("Docs".into()),
                identifier: None,
                description: None,
                current_name: None,
                status: None,
                color: None,
                emoji: None,
            }))
        });
        assert!(!is_forbidden(&allowed2), "got: {allowed2}");

        // Maintainer cannot manage project settings or delete the project.
        let denied_settings = as_user(&maintainer, || {
            m.manage_resource(Parameters(ManageResourceInput {
                resource_type: "project".into(),
                action: "update".into(),
                project: Some("MEM".into()),
                name: Some("Nope".into()),
                identifier: None,
                description: None,
                current_name: None,
                status: None,
                color: None,
                emoji: None,
            }))
        });
        assert!(is_forbidden(&denied_settings), "got: {denied_settings}");
        let denied_delete = as_user(&maintainer, || {
            m.delete(Parameters(DeleteInput {
                resource_type: "project".into(),
                identifier: "MEM".into(),
                project: None,
            }))
        });
        assert!(is_forbidden(&denied_delete), "got: {denied_delete}");
    }

    // ── Project settings / delete: Lead ──────────────────────────

    #[test]
    fn project_settings_update_maintainer_denied_lead_allowed() {
        let (m, _admin, lead, maintainer, _viewer, _non_member, _project_id) =
            setup_membership_mcp();

        let denied = as_user(&maintainer, || {
            m.manage_resource(Parameters(ManageResourceInput {
                resource_type: "project".into(),
                action: "update".into(),
                project: Some("MEM".into()),
                name: Some("Nope".into()),
                identifier: None,
                description: None,
                current_name: None,
                status: None,
                color: None,
                emoji: None,
            }))
        });
        assert!(is_forbidden(&denied), "got: {denied}");

        let allowed = as_user(&lead, || {
            m.manage_resource(Parameters(ManageResourceInput {
                resource_type: "project".into(),
                action: "update".into(),
                project: Some("MEM".into()),
                name: Some("Renamed".into()),
                identifier: None,
                description: None,
                current_name: None,
                status: None,
                color: None,
                emoji: None,
            }))
        });
        assert!(!is_forbidden(&allowed), "got: {allowed}");
    }

    #[test]
    fn project_delete_maintainer_denied_lead_allowed_when_enforced() {
        let (m, _admin, lead, maintainer, _viewer, _non_member, _project_id) =
            setup_membership_mcp();

        let denied = as_user(&maintainer, || {
            m.delete(Parameters(DeleteInput {
                resource_type: "project".into(),
                identifier: "MEM".into(),
                project: None,
            }))
        });
        assert!(is_forbidden(&denied), "got: {denied}");

        let allowed = as_user(&lead, || {
            m.delete(Parameters(DeleteInput {
                resource_type: "project".into(),
                identifier: "MEM".into(),
                project: None,
            }))
        });
        assert!(!is_forbidden(&allowed), "got: {allowed}");
    }

    // ── Cross-project relations: role required on BOTH sides ─────

    #[test]
    fn relation_link_requires_maintainer_on_both_projects() {
        let (m, _admin, lead, maintainer, _viewer, _non_member, project_id) =
            setup_membership_mcp();
        let issue_a = as_user(&lead, || {
            m.create_issue(Parameters(CreateIssueInput {
                project: "MEM".into(),
                title: "A".into(),
                description: None,
                status: None,
                priority: None,
                module: None,
                labels: None,
                ..Default::default()
            }))
        });
        assert!(issue_a.starts_with("Created"), "got: {issue_a}");

        let other_project_id = {
            let conn = m.db.write().unwrap();
            crate::db::queries::create_project(
                &conn,
                &models::CreateProject {
                    name: "Other".into(),
                    identifier: "OTH".into(),
                    description: String::new(),
                    emoji: None,
                    lead_user_id: Some(lead.id),
                },
            )
            .unwrap()
            .id
        };
        let _ = project_id;
        let issue_b = as_user(&lead, || {
            m.create_issue(Parameters(CreateIssueInput {
                project: "OTH".into(),
                title: "B".into(),
                description: None,
                status: None,
                priority: None,
                module: None,
                labels: None,
                ..Default::default()
            }))
        });
        assert!(issue_b.starts_with("Created"), "got: {issue_b}");

        let denied = as_user(&maintainer, || {
            m.link_issues(Parameters(LinkIssuesInput {
                source: "MEM-1".into(),
                target: "OTH-1".into(),
                relation_type: "relates_to".into(),
            }))
        });
        assert!(
            is_forbidden(&denied),
            "maintainer has no role on target's project: {denied}"
        );

        {
            let conn = m.db.write().unwrap();
            crate::db::queries::members::upsert_member(
                &conn,
                other_project_id,
                maintainer.id,
                models::Role::Maintainer,
            )
            .unwrap();
        }
        let allowed = as_user(&maintainer, || {
            m.link_issues(Parameters(LinkIssuesInput {
                source: "MEM-1".into(),
                target: "OTH-1".into(),
                relation_type: "relates_to".into(),
            }))
        });
        assert!(
            !is_forbidden(&allowed),
            "maintainer now has Maintainer on both sides: {allowed}"
        );
    }

    /// LIF-198 scope: `update_plan_step`'s `attach_issue` is the plan
    /// equivalent of `link_issues` — attaching a foreign-project issue to a
    /// step requires Maintainer on that issue's project too.
    #[test]
    fn plan_step_attach_issue_requires_maintainer_on_issue_project() {
        let (m, _admin, lead, maintainer, _viewer, _non_member, _project_id) =
            setup_membership_mcp();
        let plan = as_user(&maintainer, || {
            m.create_plan(Parameters(CreatePlanInput {
                project: "MEM".into(),
                title: "Plan".into(),
                anchor_issue: None,
                steps: Some(vec![PlanStepInput {
                    title: "step".into(),
                    ..Default::default()
                }]),
            }))
        });
        assert!(plan.starts_with("Created"), "got: {plan}");
        let step_id: i64 = plan
            .split('#')
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .unwrap()
            .parse()
            .unwrap();

        let other_project_id = {
            let conn = m.db.write().unwrap();
            crate::db::queries::create_project(
                &conn,
                &models::CreateProject {
                    name: "Other".into(),
                    identifier: "OT2".into(),
                    description: String::new(),
                    emoji: None,
                    lead_user_id: Some(lead.id),
                },
            )
            .unwrap()
            .id
        };
        let foreign_issue = as_user(&lead, || {
            m.create_issue(Parameters(CreateIssueInput {
                project: "OT2".into(),
                title: "Foreign".into(),
                description: None,
                status: None,
                priority: None,
                module: None,
                labels: None,
                ..Default::default()
            }))
        });
        assert!(foreign_issue.starts_with("Created"), "got: {foreign_issue}");

        let denied = as_user(&maintainer, || {
            m.update_plan_step(Parameters(UpdatePlanStepInput {
                plan: "MEM-PLAN-1".into(),
                step_id: Some(step_id),
                attach_issue: Some("OT2-1".into()),
                ..Default::default()
            }))
        });
        assert!(
            is_forbidden(&denied),
            "maintainer has no role on the foreign issue's project: {denied}"
        );

        {
            let conn = m.db.write().unwrap();
            crate::db::queries::members::upsert_member(
                &conn,
                other_project_id,
                maintainer.id,
                models::Role::Maintainer,
            )
            .unwrap();
        }
        let allowed = as_user(&maintainer, || {
            m.update_plan_step(Parameters(UpdatePlanStepInput {
                plan: "MEM-PLAN-1".into(),
                step_id: Some(step_id),
                attach_issue: Some("OT2-1".into()),
                ..Default::default()
            }))
        });
        assert!(!is_forbidden(&allowed), "got: {allowed}");
    }

    // ── Workspace-level (project-less) pages: admin-only ─────────

    #[test]
    fn workspace_page_mutation_requires_admin() {
        let (m, admin, _lead, maintainer, _viewer, _non_member, _project_id) =
            setup_membership_mcp();

        let denied = as_user(&maintainer, || {
            m.create_page(Parameters(CreatePageInput {
                project: None,
                title: "Workspace doc".into(),
                content: None,
                folder: None,
                status: None,
                labels: None,
            }))
        });
        assert!(is_forbidden(&denied), "got: {denied}");

        let allowed = as_user(&admin, || {
            m.create_page(Parameters(CreatePageInput {
                project: None,
                title: "Workspace doc".into(),
                content: None,
                folder: None,
                status: None,
                labels: None,
            }))
        });
        assert!(!is_forbidden(&allowed), "got: {allowed}");
    }

    // ── Bot -> owner inheritance (spot-check one read + one write) ──

    #[test]
    fn bot_owned_by_maintainer_inherits_role() {
        let (m, _admin, _lead, maintainer, _viewer, _non_member, _project_id) =
            setup_membership_mcp();
        let bot = {
            let conn = m.db.write().unwrap();
            let bot =
                crate::db::queries::users::create_bot_user(&conn, maintainer.id, "bot1", "bot1")
                    .unwrap();
            models::AuthUser {
                id: bot.id,
                username: bot.username,
                display_name: bot.display_name,
                is_admin: bot.is_admin,
            }
        };

        let created = as_user(&bot, || {
            m.create_issue(Parameters(CreateIssueInput {
                project: "MEM".into(),
                title: "Bot-made".into(),
                description: None,
                status: None,
                priority: None,
                module: None,
                labels: None,
                ..Default::default()
            }))
        });
        assert!(
            !is_forbidden(&created),
            "bot inherits maintainer's write access: {created}"
        );

        let read = as_user(&bot, || {
            m.get_issue(Parameters(GetIssueInput {
                identifier: "MEM-1".into(),
                ..Default::default()
            }))
        });
        assert!(
            !is_forbidden(&read),
            "bot inherits maintainer's read access: {read}"
        );
    }

    // ── Headline regression: non-member denied everywhere ────────

    #[test]
    fn non_member_denied_on_reads_mutations_and_delete() {
        let (m, _admin, lead, _maintainer, _viewer, non_member, _project_id) =
            setup_membership_mcp();
        let created = as_user(&lead, || {
            m.create_issue(Parameters(CreateIssueInput {
                project: "MEM".into(),
                title: "Guarded".into(),
                description: None,
                status: None,
                priority: None,
                module: None,
                labels: None,
                ..Default::default()
            }))
        });
        assert!(created.starts_with("Created"), "got: {created}");

        let read = as_user(&non_member, || {
            m.get_issue(Parameters(GetIssueInput {
                identifier: "MEM-1".into(),
                ..Default::default()
            }))
        });
        assert!(is_forbidden(&read), "read: {read}");

        let mutate = as_user(&non_member, || {
            m.update_issue(Parameters(UpdateIssueInput {
                identifier: "MEM-1".into(),
                title: Some("hijacked".into()),
                ..Default::default()
            }))
        });
        assert!(is_forbidden(&mutate), "mutation: {mutate}");

        let delete = as_user(&non_member, || {
            m.delete(Parameters(DeleteInput {
                resource_type: "issue".into(),
                identifier: "MEM-1".into(),
                project: None,
            }))
        });
        assert!(is_forbidden(&delete), "delete: {delete}");
    }

    // ── Admin override: non-member admin reads/writes via MCP ────

    #[test]
    fn admin_non_member_can_read_and_write_via_mcp() {
        // LIF-201 gap: `enforced_admin_non_member_allowed_all_levels`
        // (authz.rs) exercises the require_role primitive directly, and
        // `issue_create_gated_by_maintainer_role` above spot-checks the
        // write side through a tool call; this adds the READ side through
        // an actual tool call on a project the admin holds no membership
        // row on at all.
        let (m, admin, lead, _maintainer, _viewer, _non_member, _project_id) =
            setup_membership_mcp();
        let created = as_user(&lead, || {
            m.create_issue(Parameters(CreateIssueInput {
                project: "MEM".into(),
                title: "Admin spot-check".into(),
                description: None,
                status: None,
                priority: None,
                module: None,
                labels: None,
                ..Default::default()
            }))
        });
        assert!(created.starts_with("Created"), "got: {created}");

        let read = as_user(&admin, || {
            m.get_issue(Parameters(GetIssueInput {
                identifier: "MEM-1".into(),
                ..Default::default()
            }))
        });
        assert!(
            !is_forbidden(&read),
            "admin must read a project they're not a member of: {read}"
        );
        assert!(read.contains("Admin spot-check"), "got: {read}");

        let write = as_user(&admin, || {
            m.create_issue(Parameters(CreateIssueInput {
                project: "MEM".into(),
                title: "by admin".into(),
                description: None,
                status: None,
                priority: None,
                module: None,
                labels: None,
                ..Default::default()
            }))
        });
        assert!(
            !is_forbidden(&write),
            "admin must write to a project they're not a member of: {write}"
        );
    }

    // ── Token-backed lockout regression (the epic's landmine) ────
    //
    // Mirrors `api::authz_gating_tests::
    // oauth_token_backed_member_succeeds_non_member_denied_when_enforced`
    // on the MCP transport. Every other test in this module resolves
    // identity via `as_user`, which calls `with_request_user(Some(user), ..)`
    // directly — it never exercises the real bearer-token resolution path.
    // This drives an actual MCP tool call through the SAME
    // `require_api_key` middleware production wires in front of `/mcp`
    // (`main.rs`), proving a token bound to a project member (maintainer)
    // succeeds on read + write once `authz_enforced` is on, and a token
    // bound to a non-member is still denied on both — the specific
    // "default-deny bricks token-backed agents" failure mode LIF-DOC-7
    // decision #9 exists to prevent.

    #[test]
    fn oauth_token_backed_member_succeeds_non_member_denied_when_enforced() {
        use axum::body::Body;
        use axum::extract::{Json as JsonExtract, Path as PathExtract, State};
        use axum::http::{Request, StatusCode};
        use axum::routing::{get, post};
        use axum::{Extension, Router};
        use rusqlite::params;
        use sha2::{Digest, Sha256};
        use tower::ServiceExt;

        let (m, _admin, lead, maintainer, _viewer, non_member, _project_id) =
            setup_membership_mcp();
        let created = as_user(&lead, || {
            m.create_issue(Parameters(CreateIssueInput {
                project: "MEM".into(),
                title: "Token-guarded".into(),
                description: None,
                status: None,
                priority: None,
                module: None,
                labels: None,
                ..Default::default()
            }))
        });
        assert!(created.starts_with("Created"), "got: {created}");

        fn insert_oauth_token(db: &crate::db::DbPool, suffix: &str, user_id: i64) -> String {
            let token = format!("lific_at_test-{suffix}");
            let hash: String = Sha256::digest(token.as_bytes())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let expires = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
            let client_id = format!("client-{suffix}");
            let conn = db.write().unwrap();
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

        let member_token = insert_oauth_token(&m.db, "member", maintainer.id);
        let outsider_token = insert_oauth_token(&m.db, "outsider", non_member.id);

        // Minimal router exercising the exact production wiring: an
        // Extension<Option<AuthUser>> resolved by the real require_api_key
        // middleware, threaded into with_request_user around a real tool
        // call — mirrors main.rs's `/mcp` route handler (mcp::with_request_user
        // wrapping `mcp_service.handle(request)`) with the transport swapped
        // for a plain axum handler calling the tool method directly.
        async fn get_issue_h(
            State(mcp): State<LificMcp>,
            Extension(auth_user): Extension<Option<models::AuthUser>>,
            PathExtract(ident): PathExtract<String>,
        ) -> String {
            crate::mcp::with_request_user(auth_user, || async move {
                mcp.get_issue(Parameters(GetIssueInput {
                    identifier: ident,
                    ..Default::default()
                }))
            })
            .await
        }
        async fn create_issue_h(
            State(mcp): State<LificMcp>,
            Extension(auth_user): Extension<Option<models::AuthUser>>,
            JsonExtract(input): JsonExtract<CreateIssueInput>,
        ) -> String {
            crate::mcp::with_request_user(auth_user, || async move {
                mcp.create_issue(Parameters(input))
            })
            .await
        }

        let auth_state = crate::auth::AuthState {
            db: (*m.db).clone(),
            manager: crate::auth::create_key_manager().unwrap(),
            public_url: "https://example.com".into(),
            required: true,
        };
        let app = Router::new()
            .route("/call/get_issue/{ident}", get(get_issue_h))
            .route("/call/create_issue", post(create_issue_h))
            .layer(axum::middleware::from_fn_with_state(
                auth_state,
                crate::auth::require_api_key,
            ))
            .with_state(m.clone());

        async fn call_get(app: Router, uri: String, token: &str) -> String {
            let resp = app
                .oneshot(
                    Request::builder()
                        .uri(uri)
                        .header("authorization", format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "dispatcher itself must not error"
            );
            let bytes = http_body_util::BodyExt::collect(resp.into_body())
                .await
                .unwrap()
                .to_bytes();
            String::from_utf8(bytes.to_vec()).unwrap()
        }
        async fn call_create(app: Router, body: serde_json::Value, token: &str) -> String {
            let resp = app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/call/create_issue")
                        .header("content-type", "application/json")
                        .header("authorization", format!("Bearer {token}"))
                        .body(Body::from(serde_json::to_vec(&body).unwrap()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "dispatcher itself must not error"
            );
            let bytes = http_body_util::BodyExt::collect(resp.into_body())
                .await
                .unwrap()
                .to_bytes();
            String::from_utf8(bytes.to_vec()).unwrap()
        }

        let member_read = run(call_get(
            app.clone(),
            "/call/get_issue/MEM-1".into(),
            &member_token,
        ));
        assert!(
            !is_forbidden(&member_read),
            "token-backed member must be able to read: {member_read}"
        );

        let member_write = run(call_create(
            app.clone(),
            serde_json::json!({"project": "MEM", "title": "by token member"}),
            &member_token,
        ));
        assert!(
            !is_forbidden(&member_write),
            "token-backed member must be able to write: {member_write}"
        );

        let outsider_read = run(call_get(
            app.clone(),
            "/call/get_issue/MEM-1".into(),
            &outsider_token,
        ));
        assert!(
            is_forbidden(&outsider_read),
            "token-backed non-member must be denied on read: {outsider_read}"
        );

        let outsider_write = run(call_create(
            app.clone(),
            serde_json::json!({"project": "MEM", "title": "by token outsider"}),
            &outsider_token,
        ));
        assert!(
            is_forbidden(&outsider_write),
            "token-backed non-member must be denied on write: {outsider_write}"
        );
    }

    // ── Flag OFF smoke: non-member agent can still mutate ────────

    #[test]
    fn flag_off_non_member_agent_can_still_mutate() {
        // setup_lead_test-equivalent: a plain project with authz_enforced
        // left at its default (off), an unrelated authenticated user.
        let db = crate::db::open_memory().expect("test db");
        let (project_lead, outsider) = {
            let conn = db.write().unwrap();
            let lead = crate::db::queries::users::create_user(
                &conn,
                &crate::db::models::CreateUser {
                    username: "lead".into(),
                    email: "lead@test.com".into(),
                    password: "testpassword1".into(),
                    display_name: None,
                    is_admin: false,
                    is_bot: false,
                },
            )
            .unwrap();
            let outsider = crate::db::queries::users::create_user(
                &conn,
                &crate::db::models::CreateUser {
                    username: "outsider".into(),
                    email: "outsider@test.com".into(),
                    password: "testpassword1".into(),
                    display_name: None,
                    is_admin: false,
                    is_bot: false,
                },
            )
            .unwrap();
            crate::db::queries::create_project(
                &conn,
                &models::CreateProject {
                    name: "Legacy".into(),
                    identifier: "LEG".into(),
                    description: String::new(),
                    emoji: None,
                    lead_user_id: Some(lead.id),
                },
            )
            .unwrap();
            (lead, outsider)
        };
        let m = LificMcp::new(db);
        let outsider_user = models::AuthUser {
            id: outsider.id,
            username: outsider.username,
            display_name: outsider.display_name,
            is_admin: outsider.is_admin,
        };
        let _ = project_lead;

        let result = as_user(&outsider_user, || {
            m.create_issue(Parameters(CreateIssueInput {
                project: "LEG".into(),
                title: "Legacy open".into(),
                description: None,
                status: None,
                priority: None,
                module: None,
                labels: None,
                ..Default::default()
            }))
        });
        assert!(
            !is_forbidden(&result),
            "flag off: a non-member agent must still be able to mutate (MCP's historical behavior): {result}"
        );
    }
}
