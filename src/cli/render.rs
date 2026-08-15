//! Human-readable rendering for the data commands, shared by both backends.
//!
//! LIF-373: `lific issue list` used to print a formatted table when it ran
//! against the local database and a raw JSON dump when it ran against
//! `--url`, because the SQL executor and the HTTP backend each owned their
//! own output code. Both now call these functions with the same
//! `db::models` types (the HTTP backend deserializes the API response into
//! them), so what you see no longer depends on the transport.
//!
//! Every function returns the exact text to print, trailing newlines
//! included, so callers `print!` it and tests can compare the two backends'
//! output directly.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::db::models::{Comment, Folder, Issue, Label, Module, Page, Project, SearchResult};

/// `writeln!` into a `String` cannot fail, so the renderers would otherwise
/// be littered with `let _ =`. With no format arguments this writes a bare
/// blank line.
macro_rules! w {
    ($out:expr) => {
        $out.push('\n')
    };
    ($out:expr, $($arg:tt)+) => {{
        let _ = writeln!($out, $($arg)+);
    }};
}

/// Resolves a module id to its display name. The SQL backend reads it from
/// the database; the HTTP backend looks it up in a map fetched from
/// `/api/modules`. `None` (unresolvable module) renders as no module at all,
/// on both sides.
pub type ModuleName<'a> = &'a dyn Fn(i64) -> Option<String>;

/// Format a priority with visual indicator for human output.
pub fn fmt_priority(priority: &str) -> &str {
    match priority {
        "urgent" => "!!!  urgent",
        "high" => "!!   high",
        "medium" => "!    medium",
        "low" => "     low",
        _ => "     none",
    }
}

/// Format a status with visual indicator for human output.
pub fn fmt_status(status: &str) -> &str {
    match status {
        "backlog" => "[ ] backlog",
        "todo" => "[.] todo",
        "active" => "[~] active",
        "done" => "[x] done",
        "cancelled" => "[-] cancelled",
        other => other,
    }
}

fn bracketed_labels(labels: &[String]) -> String {
    if labels.is_empty() {
        String::new()
    } else {
        format!(" [{}]", labels.join(", "))
    }
}

fn first_line_suffix(text: &str) -> String {
    if text.is_empty() {
        String::new()
    } else {
        format!(" - {}", text.lines().next().unwrap_or(""))
    }
}

// ── Issue ────────────────────────────────────────────────────

pub fn issue_list(issues: &[Issue], module_name: ModuleName<'_>) -> String {
    let mut out = String::new();
    if issues.is_empty() {
        w!(out, "No issues found.");
        return out;
    }
    w!(out, "{} issue(s):\n", issues.len());
    for issue in issues {
        let module = issue
            .module_id
            .and_then(module_name)
            .map(|name| format!(" ({name})"))
            .unwrap_or_default();
        w!(
            out,
            "  {:<8} {} | {} | {}{}{}",
            issue.identifier,
            fmt_status(&issue.status),
            fmt_priority(&issue.priority),
            issue.title,
            bracketed_labels(&issue.labels),
            module
        );
    }
    out
}

pub fn issue_detail(issue: &Issue, module_name: ModuleName<'_>) -> String {
    let mut out = String::new();
    w!(out, "{} - {}", issue.identifier, issue.title);
    w!(out, "  Status:   {}", issue.status);
    w!(out, "  Priority: {}", issue.priority);
    if !issue.labels.is_empty() {
        w!(out, "  Labels:   {}", issue.labels.join(", "));
    }
    if let Some(name) = issue.module_id.and_then(module_name) {
        w!(out, "  Module:   {name}");
    }
    if !issue.blocks.is_empty() {
        w!(out, "  Blocks:   {}", issue.blocks.join(", "));
    }
    if !issue.blocked_by.is_empty() {
        w!(out, "  Blocked:  {}", issue.blocked_by.join(", "));
    }
    if !issue.relates_to.is_empty() {
        w!(out, "  Relates:  {}", issue.relates_to.join(", "));
    }
    if !issue.duplicates.is_empty() {
        w!(out, "  Dupes:    {}", issue.duplicates.join(", "));
    }
    if !issue.duplicated_by.is_empty() {
        w!(out, "  DupedBy:  {}", issue.duplicated_by.join(", "));
    }
    if !issue.description.is_empty() {
        w!(out);
        w!(out, "{}", issue.description);
    }
    out
}

pub fn issue_created(issue: &Issue) -> String {
    let mut out = String::new();
    w!(out, "Created {}: {}", issue.identifier, issue.title);
    out
}

pub fn issue_updated(issue: &Issue) -> String {
    let mut out = String::new();
    w!(out, "Updated {}: {}", issue.identifier, issue.title);
    w!(out, "  Status:   {}", issue.status);
    w!(out, "  Priority: {}", issue.priority);
    out
}

// ── Project ──────────────────────────────────────────────────

pub fn project_list(projects: &[Project]) -> String {
    let mut out = String::new();
    if projects.is_empty() {
        w!(out, "No projects.");
        return out;
    }
    w!(out, "{} project(s):\n", projects.len());
    for project in projects {
        w!(
            out,
            "  {:<5} {}{}",
            project.identifier,
            project.name,
            first_line_suffix(&project.description)
        );
    }
    out
}

pub fn project_detail(project: &Project) -> String {
    let mut out = String::new();
    w!(out, "{} - {}", project.identifier, project.name);
    if !project.description.is_empty() {
        w!(out);
        w!(out, "{}", project.description);
    }
    out
}

pub fn project_created(project: &Project) -> String {
    let mut out = String::new();
    w!(
        out,
        "Created project {} ({})",
        project.name,
        project.identifier
    );
    out
}

pub fn project_updated(project: &Project) -> String {
    let mut out = String::new();
    w!(
        out,
        "Updated project {} ({})",
        project.name,
        project.identifier
    );
    out
}

// ── Page ─────────────────────────────────────────────────────

pub fn page_list(pages: &[Page]) -> String {
    let mut out = String::new();
    if pages.is_empty() {
        w!(out, "No pages found.");
        return out;
    }
    w!(out, "{} page(s):\n", pages.len());
    for page in pages {
        let preview = if page.content.is_empty() {
            "(empty)".to_string()
        } else {
            let first_line = page.content.lines().next().unwrap_or("");
            if first_line.len() > 60 {
                format!("{}...", &first_line[..60])
            } else {
                first_line.to_string()
            }
        };
        w!(
            out,
            "  {:<12} {} - {}{}",
            page.identifier,
            page.title,
            preview,
            bracketed_labels(&page.labels)
        );
    }
    out
}

pub fn page_detail(page: &Page) -> String {
    let mut out = String::new();
    w!(out, "{} - {}", page.identifier, page.title);
    if !page.labels.is_empty() {
        w!(out, "  Labels: {}", page.labels.join(", "));
    }
    if !page.content.is_empty() {
        w!(out);
        w!(out, "{}", page.content);
    }
    out
}

pub fn page_created(page: &Page) -> String {
    let mut out = String::new();
    w!(out, "Created page {}: {}", page.identifier, page.title);
    out
}

pub fn page_updated(page: &Page) -> String {
    let mut out = String::new();
    w!(out, "Updated page {}: {}", page.identifier, page.title);
    out
}

// ── Search ───────────────────────────────────────────────────

pub fn search_results(results: &[SearchResult]) -> String {
    let mut out = String::new();
    if results.is_empty() {
        w!(out, "No results found.");
        return out;
    }
    w!(out, "{} result(s):\n", results.len());
    for result in results {
        let identifier = result.identifier.as_deref().unwrap_or("?");
        w!(
            out,
            "  {:<12} [{}] {}",
            identifier,
            result.result_type,
            result.title
        );
        if !result.snippet.is_empty() {
            // Clean up snippet for terminal display
            let snippet = result.snippet.replace("**", "").replace('\n', " ");
            let snippet = if snippet.len() > 80 {
                format!("{}...", &snippet[..80])
            } else {
                snippet
            };
            w!(out, "              {}", snippet);
        }
    }
    out
}

// ── Comment ──────────────────────────────────────────────────

pub fn comment_list(comments: &[Comment], identifier: &str) -> String {
    let mut out = String::new();
    if comments.is_empty() {
        w!(out, "No comments on {}.", identifier);
        return out;
    }
    w!(out, "{} comment(s) on {}:\n", comments.len(), identifier);
    for comment in comments {
        w!(
            out,
            "  {} ({}) - {}:",
            comment.author_display_name,
            comment.author,
            comment.created_at
        );
        for line in comment.content.lines() {
            w!(out, "    {line}");
        }
        w!(out);
    }
    out
}

pub fn comment_added(comment: &Comment, identifier: &str) -> String {
    let mut out = String::new();
    w!(out, "Added comment to {} by {}:", identifier, comment.author);
    w!(out, "  {}", comment.content);
    out
}

// ── Module ───────────────────────────────────────────────────

pub fn module_list(modules: &[Module], project: &str) -> String {
    let mut out = String::new();
    if modules.is_empty() {
        w!(out, "No modules in {}.", project);
        return out;
    }
    w!(out, "{} module(s) in {}:\n", modules.len(), project);
    for module in modules {
        w!(
            out,
            "  {:<20} [{}]{}",
            module.name,
            module.status,
            first_line_suffix(&module.description)
        );
    }
    out
}

pub fn module_created(module: &Module, project: &str) -> String {
    let mut out = String::new();
    w!(
        out,
        "Created module '{}' [{}] in {}",
        module.name,
        module.status,
        project
    );
    out
}

pub fn module_updated(module: &Module) -> String {
    let mut out = String::new();
    w!(out, "Updated module '{}' [{}]", module.name, module.status);
    out
}

pub fn module_deleted(name: &str) -> String {
    let mut out = String::new();
    w!(out, "Deleted module '{}'", name);
    out
}

// ── Label ────────────────────────────────────────────────────

pub fn label_list(labels: &[Label], project: &str) -> String {
    let mut out = String::new();
    if labels.is_empty() {
        w!(out, "No labels in {}.", project);
        return out;
    }
    w!(out, "{} label(s) in {}:\n", labels.len(), project);
    for label in labels {
        w!(out, "  {} ({})", label.name, label.color);
    }
    out
}

pub fn label_created(label: &Label) -> String {
    let mut out = String::new();
    w!(out, "Created label '{}' ({})", label.name, label.color);
    out
}

pub fn label_updated(label: &Label) -> String {
    let mut out = String::new();
    w!(out, "Updated label '{}' ({})", label.name, label.color);
    out
}

pub fn label_deleted(name: &str) -> String {
    let mut out = String::new();
    w!(out, "Deleted label '{}'", name);
    out
}

// ── Folder ───────────────────────────────────────────────────

pub fn folder_list(folders: &[Folder], project: &str) -> String {
    let mut out = String::new();
    if folders.is_empty() {
        w!(out, "No folders in {}.", project);
        return out;
    }
    w!(out, "{} folder(s) in {}:\n", folders.len(), project);
    for folder in folders {
        w!(out, "  {}", folder.name);
    }
    out
}

pub fn folder_created(folder: &Folder) -> String {
    let mut out = String::new();
    w!(out, "Created folder '{}'", folder.name);
    out
}

pub fn folder_updated(previous_name: &str, folder: &Folder) -> String {
    let mut out = String::new();
    w!(out, "Renamed folder '{}' -> '{}'", previous_name, folder.name);
    out
}

pub fn folder_deleted(name: &str) -> String {
    let mut out = String::new();
    w!(out, "Deleted folder '{}'", name);
    out
}

// ── Export ───────────────────────────────────────────────────

pub fn export_written(written: &[PathBuf], output: &Path) -> String {
    let mut out = String::new();
    w!(
        out,
        "Exported {} file(s) to {}",
        written.len(),
        output.display()
    );
    for path in written {
        w!(out, "  {}", path.display());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(identifier: &str, status: &str, priority: &str) -> Issue {
        Issue {
            id: 1,
            project_id: 1,
            sequence: 1,
            identifier: identifier.into(),
            title: "Fix the bug".into(),
            description: String::new(),
            status: status.into(),
            priority: priority.into(),
            module_id: None,
            sort_order: 0.0,
            start_date: None,
            target_date: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            source: None,
            labels: Vec::new(),
            blocks: Vec::new(),
            blocked_by: Vec::new(),
            relates_to: Vec::new(),
            duplicates: Vec::new(),
            duplicated_by: Vec::new(),
        }
    }

    #[test]
    fn marks_statuses_and_priorities_with_indicators() {
        assert_eq!(fmt_status("active"), "[~] active");
        assert_eq!(fmt_status("unknown"), "unknown");
        assert_eq!(fmt_priority("urgent"), "!!!  urgent");
        assert_eq!(fmt_priority("nonsense"), "     none");
    }

    #[test]
    fn lists_issues_with_labels_and_module_name() {
        let mut one = issue("TST-1", "active", "high");
        one.labels = vec!["bug".into(), "urgent".into()];
        one.module_id = Some(4);

        let rendered = issue_list(std::slice::from_ref(&one), &|id| {
            (id == 4).then(|| "Core".to_owned())
        });

        assert_eq!(
            rendered,
            "1 issue(s):\n\n  TST-1    [~] active | !!   high | Fix the bug [bug, urgent] (Core)\n"
        );
    }

    #[test]
    fn omits_the_module_when_the_name_cannot_be_resolved() {
        let mut one = issue("TST-1", "todo", "low");
        one.module_id = Some(9);

        let rendered = issue_list(std::slice::from_ref(&one), &|_| None);

        assert_eq!(
            rendered,
            "1 issue(s):\n\n  TST-1    [.] todo |      low | Fix the bug\n"
        );
    }

    #[test]
    fn reports_empty_collections_without_a_count_header() {
        assert_eq!(issue_list(&[], &|_| None), "No issues found.\n");
        assert_eq!(project_list(&[]), "No projects.\n");
        assert_eq!(page_list(&[]), "No pages found.\n");
        assert_eq!(search_results(&[]), "No results found.\n");
        assert_eq!(comment_list(&[], "TST-1"), "No comments on TST-1.\n");
        assert_eq!(module_list(&[], "TST"), "No modules in TST.\n");
        assert_eq!(label_list(&[], "TST"), "No labels in TST.\n");
        assert_eq!(folder_list(&[], "TST"), "No folders in TST.\n");
    }

    #[test]
    fn details_an_issue_with_its_relations_and_description() {
        let mut one = issue("TST-1", "active", "urgent");
        one.description = "Details".into();
        one.blocks = vec!["TST-2".into()];
        one.duplicated_by = vec!["TST-3".into()];

        assert_eq!(
            issue_detail(&one, &|_| None),
            "TST-1 - Fix the bug\n  Status:   active\n  Priority: urgent\n  \
             Blocks:   TST-2\n  DupedBy:  TST-3\n\nDetails\n"
        );
    }
}
