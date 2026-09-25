//! LIF-483: `get_briefing`, one call to resume work on a project.
//!
//! Agents were rebuilding "where are we" from four or five reads at the start
//! of every session (search, a canon page, list_issues, modules, plans, the
//! board). The briefing assembles the same answer from live records in one
//! bounded response. Nothing is stored and nothing is summarized by a model:
//! every line is a row that exists right now.
//!
//! Each section is capped on its own, omitted when empty, and says what it
//! left out and which call shows the rest. A final pass trims the longest
//! sections until the whole response fits [`BUDGET_CHARS`].

use std::collections::HashSet;
use std::fmt::Write as _;

use super::*;

/// The whole response stays under this many characters, notes included.
pub(super) const BUDGET_CHARS: usize = 6_000;
/// Lines per issue section before budgeting.
const ISSUE_LINES: usize = 8;
const PLAN_LINES: usize = 5;
/// Lines per kind of change (closed, created, moved, pages).
const CHANGE_LINES_PER_KIND: usize = 5;
const RECENT_PAGES: i64 = 3;
const MAX_NAMED_PAGES: usize = 10;
const TITLE_CHARS: usize = 80;
/// How many rows a section reads to count what it does not show.
const SCAN_LIMIT: i64 = 100;

/// One bounded block of the briefing.
struct Section {
    heading: String,
    summary: Option<String>,
    lines: Vec<String>,
    /// Items that exist, shown or not. Budgeting pops lines, never this.
    total: usize,
    /// `total` is a floor: the scan stopped at [`SCAN_LIMIT`].
    total_is_floor: bool,
    /// The call that shows everything this section trimmed.
    more: String,
}

impl Section {
    fn render(&self, output: &mut String) -> fmt::Result {
        writeln!(output, "\n## {}", self.heading)?;
        if let Some(summary) = &self.summary {
            writeln!(output, "{summary}")?;
        }
        for line in &self.lines {
            writeln!(output, "- {line}")?;
        }
        let omitted = self.total.saturating_sub(self.lines.len());
        match (omitted, self.total_is_floor) {
            (0, false) => Ok(()),
            (omitted, false) => writeln!(output, "(+{omitted} more: {})", self.more),
            (omitted, true) => writeln!(output, "(+{omitted} or more: {})", self.more),
        }
    }
}

/// Render the header and sections, then pop lines from the longest section
/// (the later one on a tie) until the response fits the budget.
fn fit_to_budget(header: &str, sections: &mut [Section]) -> Result<String, String> {
    loop {
        let output = try_render(|output| {
            output.push_str(header);
            sections
                .iter()
                .try_for_each(|section| section.render(output))
        })
        .map_err(|error| format!("failed to format briefing: {error}"))?;
        if output.chars().count() <= BUDGET_CHARS {
            return Ok(output);
        }
        let Some(longest) = sections
            .iter_mut()
            .filter(|section| !section.lines.is_empty())
            .max_by_key(|section| section.lines.len())
        else {
            return Ok(output);
        };
        longest.lines.pop();
    }
}

fn title(value: &str) -> TruncatedValue<'_> {
    truncate_value(value, TITLE_CHARS)
}

fn issue_line(context: Option<&IssueLinkContext>, issue: &models::Issue) -> String {
    format!(
        "{} | {} | {} | {}",
        issue_reference(context, &issue.identifier),
        issue.status,
        issue.priority,
        title(&issue.title)
    )
}

/// The first open step in tree order, descending into its first open child,
/// so the answer is the concrete thing to do next rather than its parent.
fn next_open_step(steps: &[models::PlanStepNode]) -> Option<&models::PlanStepNode> {
    let step = steps.iter().find(|step| !step.done)?;
    Some(next_open_step(&step.children).unwrap_or(step))
}

fn is_closed(status: &str) -> bool {
    matches!(status, "done" | "cancelled")
}

impl LificMcp {
    pub(super) fn get_briefing_inner(&self, input: GetBriefingInput) -> Result<String, String> {
        if let Some(nudge) = self.no_projects_nudge() {
            return Ok(nudge);
        }
        let since = input
            .since
            .as_deref()
            .map(queries::activity::normalize_since)
            .transpose()
            .map_err(|error| error.to_string())?;
        let project = {
            let conn = self.read_conn()?;
            let pid = resolve_project(&conn, self.project_or_bound(input.project.as_deref())?)?;
            require_role_mcp(&self.db, pid, models::Role::Viewer)?;
            queries::get_project(&conn, pid).map_err(|error| error.to_string())?
        };
        let context = current_issue_link_context();
        let context = context.as_deref();
        let ident = project.identifier.as_str();

        let now = Utc::now();
        let mut header = format!(
            "{} briefing at {} UTC. Resume later with since='{}'.\n",
            project_reference(context, ident),
            now.format("%Y-%m-%d %H:%M:%S"),
            now.format("%Y-%m-%dT%H:%M:%SZ"),
        );

        let changes = match &since {
            Some(since) => {
                let section = self.changes_section(context, project.id, ident, since)?;
                if section.is_none() {
                    let _ = writeln!(header, "No changes since {since} UTC.");
                }
                section
            }
            None => None,
        };
        let plans = self.plans_section(context, project.id, ident)?;
        let blocked = self.blocked_section(context, project.id, ident)?;
        // LIF-484: waits that still hold are named in `blocked`. A date wait
        // whose window has arrived no longer blocks, so without this section
        // the follow-up it asks for would hide among workable issues.
        let due = self.due_section(context, project.id, ident)?;
        let workable = self.workable_section(context, project.id, ident)?;
        let pages = self.pages_section(
            context,
            project.id,
            ident,
            since.as_deref(),
            input.pages.as_deref().unwrap_or_default(),
        )?;
        let active = self.active_section(context, project.id, ident)?;

        let mut sections: Vec<Section> = [changes, plans, due, blocked, workable, pages, active]
            .into_iter()
            .flatten()
            .collect();
        if sections.is_empty() {
            header.push_str("Nothing is planned, active, blocked or workable.\n");
        }
        fit_to_budget(&header, &mut sections)
    }

    fn changes_section(
        &self,
        context: Option<&IssueLinkContext>,
        project_id: i64,
        ident: &str,
        since: &str,
    ) -> Result<Option<Section>, String> {
        let (created, moves, pages) = self.read(|conn| {
            Ok((
                queries::briefing::issues_created_since(conn, project_id, since)?,
                queries::briefing::status_moves_since(conn, project_id, since)?,
                queries::briefing::pages_edited_since(conn, project_id, since)?,
            ))
        })?;
        let (closed, moved): (Vec<_>, Vec<_>) =
            moves.into_iter().partition(|change| is_closed(&change.to));
        let total = created.len() + closed.len() + moved.len() + pages.len();
        if total == 0 {
            return Ok(None);
        }

        let mut lines = Vec::new();
        lines.extend(closed.iter().take(CHANGE_LINES_PER_KIND).map(|change| {
            format!(
                "closed {} ({}) | {}",
                issue_reference(context, &change.identifier),
                change.to,
                title(&change.title)
            )
        }));
        lines.extend(created.iter().take(CHANGE_LINES_PER_KIND).map(|issue| {
            format!(
                "new {} | {} | {} | {}",
                issue_reference(context, &issue.identifier),
                issue.status,
                issue.priority,
                title(&issue.title)
            )
        }));
        lines.extend(moved.iter().take(CHANGE_LINES_PER_KIND).map(|change| {
            format!(
                "moved {} {} -> {} | {}",
                issue_reference(context, &change.identifier),
                change.from,
                change.to,
                title(&change.title)
            )
        }));
        lines.extend(pages.iter().take(CHANGE_LINES_PER_KIND).map(|page| {
            format!(
                "page {} {}{}",
                reference_with_context(context, ReferenceKind::Page(&page.identifier, page.id)),
                title(&page.title),
                if page.new { " (new)" } else { "" }
            )
        }));

        Ok(Some(Section {
            heading: format!("Since {since} UTC"),
            summary: Some(format!(
                "Issues created {}, closed {}, otherwise moved {}. Pages edited {}.",
                created.len(),
                closed.len(),
                moved.len(),
                pages.len()
            )),
            lines,
            total,
            total_is_floor: false,
            more: format!(
                "get_activity(identifier='{ident}', since='{}Z')",
                since.replace(' ', "T")
            ),
        }))
    }

    fn plans_section(
        &self,
        context: Option<&IssueLinkContext>,
        project_id: i64,
        ident: &str,
    ) -> Result<Option<Section>, String> {
        let (plans, shown) = self.read(|conn| {
            let plans = queries::plans::list_plans(
                conn,
                &models::ListPlansQuery {
                    project_id: Some(project_id),
                    status: Some("active".into()),
                    limit: Some(SCAN_LIMIT),
                    ..Default::default()
                },
            )?;
            let shown = plans
                .iter()
                .take(PLAN_LINES)
                .map(|plan| queries::plans::get_plan(conn, plan.id))
                .collect::<Result<Vec<_>, _>>()?;
            Ok((plans, shown))
        })?;
        if plans.is_empty() {
            return Ok(None);
        }
        let lines = shown
            .iter()
            .map(|plan| {
                let mut line = format!(
                    "{} {} ({}/{} done)",
                    plan_reference(context, plan),
                    title(&plan.title),
                    plan.done_count,
                    plan.step_count
                );
                match next_open_step(&plan.steps) {
                    Some(step) => {
                        let _ = write!(line, ", next: #{} {}", step.id, title(&step.title));
                        if let Some(issue) = &step.issue_identifier {
                            let _ = write!(line, " [{}]", issue_reference(context, issue));
                        }
                    }
                    None if plan.step_count > 0 => line.push_str(", all steps done"),
                    None => line.push_str(", no steps yet"),
                }
                line
            })
            .collect();
        Ok(Some(Section {
            heading: format!("Active plans ({})", plans.len()),
            summary: None,
            lines,
            total: plans.len(),
            total_is_floor: plans.len() as i64 >= SCAN_LIMIT,
            more: format!("list_resources(resource_type='plan', project='{ident}')"),
        }))
    }

    /// Issues in `project_id` matching `query`, priority first, as
    /// `(shown, total, total_is_floor)`. `keep` drops rows after the read.
    fn briefing_issues(
        &self,
        project_id: i64,
        query: models::ListIssuesQuery,
        keep: impl Fn(&models::Issue) -> bool,
    ) -> Result<(Vec<models::Issue>, usize, bool), String> {
        let page = self.read(|conn| {
            queries::list_issues_page(
                conn,
                &models::ListIssuesQuery {
                    project_id: Some(project_id),
                    order_by: Some("priority".into()),
                    limit: Some(SCAN_LIMIT),
                    ..query
                },
            )
        })?;
        let kept: Vec<models::Issue> = page.items.into_iter().filter(|issue| keep(issue)).collect();
        let total = kept.len();
        Ok((
            kept.into_iter().take(ISSUE_LINES).collect(),
            total,
            page.has_more,
        ))
    }

    fn blocked_section(
        &self,
        context: Option<&IssueLinkContext>,
        project_id: i64,
        ident: &str,
    ) -> Result<Option<Section>, String> {
        let (issues, total, total_is_floor) = self.briefing_issues(
            project_id,
            models::ListIssuesQuery {
                blocked: Some(true),
                ..Default::default()
            },
            |issue| !is_closed(issue.status.as_str()),
        )?;
        if issues.is_empty() {
            return Ok(None);
        }
        let ids: Vec<i64> = issues.iter().map(|issue| issue.id).collect();
        let blockers = self.read(|conn| queries::briefing::open_blockers(conn, &ids))?;
        // A blocker in a project the caller cannot see is counted, never named.
        let visible: Option<HashSet<i64>> = visible_project_ids_mcp(&self.db)?;
        let lines = issues
            .iter()
            .map(|issue| {
                let mut named = Vec::new();
                let mut hidden = 0;
                for blocker in blockers.iter().filter(|b| b.target_id == issue.id) {
                    if visible
                        .as_ref()
                        .is_none_or(|ids| ids.contains(&blocker.project_id))
                    {
                        named.push(format!(
                            "{} ({})",
                            issue_reference(context, &blocker.identifier),
                            blocker.status
                        ));
                    } else {
                        hidden += 1;
                    }
                }
                if hidden > 0 {
                    named.push(format!("{hidden} in a project you cannot view"));
                }
                let mut line = issue_line(context, issue);
                if !named.is_empty() {
                    let _ = write!(line, "; blocked by {}", named.join(", "));
                }
                // LIF-484: a person or a not-yet-started date window blocks
                // like an issue does, so it is named here too.
                for wait in issue
                    .waits
                    .iter()
                    .filter(|wait| wait.state == models::WaitState::Holding)
                {
                    let text = crate::mcp::waits::WaitLine(wait).to_string();
                    let mut chars = text.chars();
                    let lowered: String = chars
                        .next()
                        .map(|first| first.to_lowercase().chain(chars).collect())
                        .unwrap_or_default();
                    let _ = write!(line, "; {lowered}");
                }
                line
            })
            .collect();
        Ok(Some(Section {
            heading: format!("Blocked ({total})"),
            summary: None,
            lines,
            total,
            total_is_floor,
            more: format!("list_issues(project='{ident}', blocked=true)"),
        }))
    }

    fn due_section(
        &self,
        context: Option<&IssueLinkContext>,
        project_id: i64,
        ident: &str,
    ) -> Result<Option<Section>, String> {
        let today = queries::waits::today_text();
        let ids =
            self.read(|conn| queries::briefing::issues_with_due_waits(conn, project_id, &today))?;
        if ids.is_empty() {
            return Ok(None);
        }
        let issues = self.read(|conn| {
            ids.iter()
                .take(ISSUE_LINES)
                .map(|&id| queries::get_issue(conn, id))
                .collect::<Result<Vec<_>, _>>()
        })?;
        let lines = issues
            .iter()
            .map(|issue| {
                let mut line = issue_line(context, issue);
                for wait in issue
                    .waits
                    .iter()
                    .filter(|wait| wait.state != models::WaitState::Holding)
                {
                    let _ = write!(line, "; {}", crate::mcp::waits::WaitLine(wait));
                }
                line
            })
            .collect();
        Ok(Some(Section {
            heading: format!("Due to check ({})", ids.len()),
            summary: Some(
                "Date waits whose window has arrived. Follow up, then clear the wait with unlink_issues.".into(),
            ),
            lines,
            total: ids.len(),
            total_is_floor: false,
            more: format!("list_issues(project='{ident}'), rows tagged due_since or overdue_since"),
        }))
    }

    fn workable_section(
        &self,
        context: Option<&IssueLinkContext>,
        project_id: i64,
        ident: &str,
    ) -> Result<Option<Section>, String> {
        // Active issues have their own section; this one is what to pick up.
        let (issues, total, total_is_floor) = self.briefing_issues(
            project_id,
            models::ListIssuesQuery {
                workable: Some(true),
                ..Default::default()
            },
            |issue| issue.status != models::Status::Active,
        )?;
        if issues.is_empty() {
            return Ok(None);
        }
        Ok(Some(Section {
            heading: format!("Workable, not yet active ({total}), by priority"),
            summary: None,
            lines: issues
                .iter()
                .map(|issue| issue_line(context, issue))
                .collect(),
            total,
            total_is_floor,
            more: format!("list_issues(project='{ident}', workable=true, order_by='priority')"),
        }))
    }

    fn active_section(
        &self,
        context: Option<&IssueLinkContext>,
        project_id: i64,
        ident: &str,
    ) -> Result<Option<Section>, String> {
        let (issues, total, total_is_floor) = self.briefing_issues(
            project_id,
            models::ListIssuesQuery {
                status: Some(models::Status::Active),
                ..Default::default()
            },
            |_| true,
        )?;
        if issues.is_empty() {
            return Ok(None);
        }
        Ok(Some(Section {
            heading: format!("Active ({total})"),
            summary: None,
            lines: issues
                .iter()
                .map(|issue| issue_line(context, issue))
                .collect(),
            total,
            total_is_floor,
            more: format!("list_issues(project='{ident}', status='active')"),
        }))
    }

    /// Named pages when the caller lists them (each gated like `get_page`),
    /// otherwise the project's most recently updated pages. Metadata only:
    /// bodies stay behind `get_page`.
    fn pages_section(
        &self,
        context: Option<&IssueLinkContext>,
        project_id: i64,
        ident: &str,
        since: Option<&str>,
        named: &[String],
    ) -> Result<Option<Section>, String> {
        let describe = |page: &models::Page| {
            let mut line = format!(
                "{} {} | seq {} | {} chars",
                page_reference(context, page),
                title(&page.title),
                page.seq,
                page.content.chars().count()
            );
            match since {
                Some(since) if page.updated_at.as_str() > since => {
                    line.push_str(" | changed since cursor");
                }
                Some(_) => line.push_str(" | unchanged since cursor"),
                None => {
                    let _ = write!(line, " | updated {} UTC", page.updated_at);
                }
            }
            line
        };

        if named.is_empty() {
            let pages = self.read(|conn| {
                queries::list_pages_page(
                    conn,
                    Some(project_id),
                    None,
                    None,
                    None,
                    Some("updated"),
                    Some("desc"),
                    Some(RECENT_PAGES),
                    None,
                )
            })?;
            if pages.items.is_empty() {
                return Ok(None);
            }
            return Ok(Some(Section {
                heading: "Recently updated pages".into(),
                summary: None,
                lines: pages.items.iter().map(describe).collect(),
                total: pages.items.len(),
                total_is_floor: false,
                more: format!("list_resources(resource_type='page', project='{ident}')"),
            }));
        }

        let mut seen = HashSet::new();
        let requested: Vec<&str> = named
            .iter()
            .map(|identifier| identifier.trim())
            .filter(|identifier| !identifier.is_empty() && seen.insert(identifier.to_uppercase()))
            .collect();
        let lines = requested
            .iter()
            .take(MAX_NAMED_PAGES)
            .map(|identifier| {
                // Missing and not visible read the same, so a briefing cannot
                // be used to probe for pages in other projects.
                let page = self
                    .read(|conn| {
                        let id = queries::resolve_page_identifier(conn, identifier)?;
                        queries::get_page(conn, id)
                    })
                    .ok()
                    .filter(|page| {
                        require_page_role_mcp(&self.db, page.project_id, models::Role::Viewer)
                            .is_ok()
                    });
                match page {
                    Some(page) => describe(&page),
                    None => format!("{identifier}: not found"),
                }
            })
            .collect();
        Ok(Some(Section {
            heading: "Key pages".into(),
            summary: None,
            lines,
            total: requested.len(),
            total_is_floor: false,
            more: format!("at most {MAX_NAMED_PAGES} named pages per briefing"),
        }))
    }
}

#[cfg(test)]
mod tests;
