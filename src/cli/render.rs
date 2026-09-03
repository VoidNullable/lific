//! Output for the data commands, shared by both backends: the human-readable
//! renderers, plus the result types a command has to invent when there is no
//! model to print.
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
//!
//! LIF-341 finished the job for the commands LIF-373 did not reach: exports
//! (the HTTP backend now writes the same files to the same paths, unpacking
//! the project archive locally) and deletes (both backends print the
//! [`Deleted`] below).

use std::fmt::{Display, Write as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cli::ui::TerminalDisplay;
use crate::db::models::{
    Comment, Folder, Issue, Label, Module, Page, Priority, Project, SearchResult, Status,
};

trait TerminalWrite {
    fn line(&mut self, text: impl Display);
    fn block(&mut self, text: impl Display);
}

impl TerminalWrite for String {
    fn line(&mut self, text: impl Display) {
        let _ = writeln!(self, "{}", text.terminal_line());
    }

    fn block(&mut self, text: impl Display) {
        let _ = writeln!(self, "{}", text.terminal_block());
    }
}

/// What a `delete` prints under `--json`.
///
/// Deleting a module, label, or folder returns no row to serialize, so each
/// backend used to hand-roll its own answer: the SQL executor printed a
/// compact `{"deleted": true, "name": ...}` while the HTTP backend turned a
/// `204 No Content` into a pretty-printed `{"deleted": true}` with no name at
/// all. Both now build this (LIF-341).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deleted {
    pub deleted: bool,
    pub name: String,
}

impl Deleted {
    #[must_use]
    pub fn named(name: &str) -> Self {
        Self {
            deleted: true,
            name: name.to_owned(),
        }
    }
}

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
pub fn fmt_priority(priority: Priority) -> &'static str {
    match priority {
        Priority::Urgent => "!!!  urgent",
        Priority::High => "!!   high",
        Priority::Medium => "!    medium",
        Priority::Low => "     low",
        Priority::None => "     none",
    }
}

/// Format a status with visual indicator for human output.
pub fn fmt_status(status: Status) -> &'static str {
    match status {
        Status::Backlog => "[ ] backlog",
        Status::Todo => "[.] todo",
        Status::Active => "[~] active",
        Status::Done => "[x] done",
        Status::Cancelled => "[-] cancelled",
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
        out.line("No issues found.");
        return out;
    }
    out.line(format_args!("{} issue(s):", issues.len()));
    out.push('\n');
    for issue in issues {
        let module = issue
            .module_id
            .and_then(module_name)
            .map(|name| format!(" ({name})"))
            .unwrap_or_default();
        out.line(format_args!(
            "  {:<8} {} | {} | {}{}{}",
            issue.identifier,
            fmt_status(issue.status),
            fmt_priority(issue.priority),
            issue.title,
            bracketed_labels(&issue.labels),
            module
        ));
    }
    out
}

pub fn issue_detail(issue: &Issue, module_name: ModuleName<'_>) -> String {
    let mut out = String::new();
    out.line(format_args!("{} - {}", issue.identifier, issue.title));
    out.line(format_args!("  Status:   {}", issue.status));
    out.line(format_args!("  Priority: {}", issue.priority));
    if !issue.labels.is_empty() {
        out.line(format_args!("  Labels:   {}", issue.labels.join(", ")));
    }
    if let Some(name) = issue.module_id.and_then(module_name) {
        out.line(format_args!("  Module:   {name}"));
    }
    if !issue.blocks.is_empty() {
        out.line(format_args!("  Blocks:   {}", issue.blocks.join(", ")));
    }
    if !issue.blocked_by.is_empty() {
        out.line(format_args!("  Blocked:  {}", issue.blocked_by.join(", ")));
    }
    if !issue.relates_to.is_empty() {
        out.line(format_args!("  Relates:  {}", issue.relates_to.join(", ")));
    }
    if !issue.duplicates.is_empty() {
        out.line(format_args!("  Dupes:    {}", issue.duplicates.join(", ")));
    }
    if !issue.duplicated_by.is_empty() {
        out.line(format_args!(
            "  DupedBy:  {}",
            issue.duplicated_by.join(", ")
        ));
    }
    if !issue.description.is_empty() {
        out.push('\n');
        out.block(&issue.description);
    }
    out
}

pub fn issue_created(issue: &Issue) -> String {
    let mut out = String::new();
    out.line(format_args!(
        "Created {}: {}",
        issue.identifier, issue.title
    ));
    out
}

pub fn issue_updated(issue: &Issue) -> String {
    let mut out = String::new();
    out.line(format_args!(
        "Updated {}: {}",
        issue.identifier, issue.title
    ));
    out.line(format_args!("  Status:   {}", issue.status));
    out.line(format_args!("  Priority: {}", issue.priority));
    out
}

// ── Project ──────────────────────────────────────────────────

pub fn project_list(projects: &[Project]) -> String {
    let mut out = String::new();
    if projects.is_empty() {
        out.line("No projects.");
        return out;
    }
    out.line(format_args!("{} project(s):", projects.len()));
    out.push('\n');
    for project in projects {
        out.line(format_args!(
            "  {:<5} {}{}",
            project.identifier,
            project.name,
            first_line_suffix(&project.description)
        ));
    }
    out
}

pub fn project_detail(project: &Project) -> String {
    let mut out = String::new();
    out.line(format_args!("{} - {}", project.identifier, project.name));
    if !project.description.is_empty() {
        out.push('\n');
        out.block(&project.description);
    }
    out
}

pub fn project_created(project: &Project) -> String {
    let mut out = String::new();
    out.line(format_args!(
        "Created project {} ({})",
        project.name, project.identifier
    ));
    out
}

pub fn project_updated(project: &Project) -> String {
    let mut out = String::new();
    out.line(format_args!(
        "Updated project {} ({})",
        project.name, project.identifier
    ));
    out
}

// ── Page ─────────────────────────────────────────────────────

pub fn page_list(pages: &[Page]) -> String {
    let mut out = String::new();
    if pages.is_empty() {
        out.line("No pages found.");
        return out;
    }
    out.line(format_args!("{} page(s):", pages.len()));
    out.push('\n');
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
        out.line(format_args!(
            "  {:<12} {} - {}{}",
            page.identifier,
            page.title,
            preview,
            bracketed_labels(&page.labels)
        ));
    }
    out
}

pub fn page_detail(page: &Page) -> String {
    let mut out = String::new();
    out.line(format_args!("{} - {}", page.identifier, page.title));
    if !page.labels.is_empty() {
        out.line(format_args!("  Labels: {}", page.labels.join(", ")));
    }
    if !page.content.is_empty() {
        out.push('\n');
        out.block(&page.content);
    }
    out
}

pub fn page_created(page: &Page) -> String {
    let mut out = String::new();
    out.line(format_args!(
        "Created page {}: {}",
        page.identifier, page.title
    ));
    out
}

pub fn page_updated(page: &Page) -> String {
    let mut out = String::new();
    out.line(format_args!(
        "Updated page {}: {}",
        page.identifier, page.title
    ));
    out
}

// ── Search ───────────────────────────────────────────────────

pub fn search_results(results: &[SearchResult]) -> String {
    let mut out = String::new();
    if results.is_empty() {
        out.line("No results found.");
        return out;
    }
    out.line(format_args!("{} result(s):", results.len()));
    out.push('\n');
    for result in results {
        let identifier = result.identifier.as_deref().unwrap_or("?");
        out.line(format_args!(
            "  {:<12} [{}] {}",
            identifier, result.result_type, result.title
        ));
        if !result.snippet.is_empty() {
            // Clean up snippet for terminal display
            let snippet = result.snippet.replace("**", "").replace('\n', " ");
            let snippet = if snippet.len() > 80 {
                format!("{}...", &snippet[..80])
            } else {
                snippet
            };
            out.line(format_args!("              {snippet}"));
        }
    }
    out
}

// ── Comment ──────────────────────────────────────────────────

/// What the CLI knows about comments sitting past the page it just printed.
///
/// This is three states, not an `Option`, because "there is no next page" and
/// "I could not find out" are different things to tell a user. Collapsing them
/// meant a failed lookup rendered as a complete thread, which is the one
/// answer a bounded listing must never give by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentContinuation {
    /// The page reached the end of the thread.
    End,
    /// More comments exist; this offset reads the next page.
    Next(i64),
    /// The page came back full and the check for a row past it failed, so
    /// whether more exist is genuinely unknown. Only the remote backend can
    /// reach this: the direct-SQL page answers `has_more` from an over-fetch
    /// inside its own query, which cannot fail on its own.
    Unknown(i64),
}

/// Render one page of a comment thread.
///
/// Both backends bound `comment list` now, so without the trailing line a
/// truncated thread would be indistinguishable from a complete one, which is
/// the failure mode the bound would otherwise introduce.
pub fn comment_list(
    comments: &[Comment],
    identifier: &str,
    continuation: CommentContinuation,
) -> String {
    let mut out = String::new();
    if comments.is_empty() {
        out.line(format_args!("No comments on {identifier}."));
        return out;
    }
    out.line(format_args!(
        "{} comment(s) on {}:",
        comments.len(),
        identifier
    ));
    out.push('\n');
    for comment in comments {
        out.line(format_args!(
            "  {} ({}) - {}:",
            comment.author_display_name, comment.author, comment.created_at
        ));
        for line in comment.content.lines() {
            out.line(format_args!("    {line}"));
        }
        out.push('\n');
    }
    match continuation {
        CommentContinuation::End => {}
        CommentContinuation::Next(next) => {
            w!(out, "More comments available. Next page: --offset {}", next);
        }
        CommentContinuation::Unknown(next) => {
            w!(
                out,
                "Could not check whether more comments exist. If they do, the next page is --offset {}.",
                next
            );
        }
    }
    out
}

pub fn comment_added(comment: &Comment, identifier: &str) -> String {
    let mut out = String::new();
    out.line(format_args!(
        "Added comment to {} by {}:",
        identifier, comment.author
    ));
    out.block(format_args!("  {}", comment.content));
    out
}

// ── Module ───────────────────────────────────────────────────

pub fn module_list(modules: &[Module], project: &str) -> String {
    let mut out = String::new();
    if modules.is_empty() {
        out.line(format_args!("No modules in {project}."));
        return out;
    }
    out.line(format_args!("{} module(s) in {}:", modules.len(), project));
    out.push('\n');
    for module in modules {
        out.line(format_args!(
            "  {:<20} [{}]{}",
            module.name,
            module.status,
            first_line_suffix(&module.description)
        ));
    }
    out
}

pub fn module_created(module: &Module, project: &str) -> String {
    let mut out = String::new();
    out.line(format_args!(
        "Created module '{}' [{}] in {}",
        module.name, module.status, project
    ));
    out
}

pub fn module_updated(module: &Module) -> String {
    let mut out = String::new();
    out.line(format_args!(
        "Updated module '{}' [{}]",
        module.name, module.status
    ));
    out
}

pub fn module_deleted(name: &str) -> String {
    let mut out = String::new();
    out.line(format_args!("Deleted module '{name}'"));
    out
}

// ── Label ────────────────────────────────────────────────────

pub fn label_list(labels: &[Label], project: &str) -> String {
    let mut out = String::new();
    if labels.is_empty() {
        out.line(format_args!("No labels in {project}."));
        return out;
    }
    out.line(format_args!("{} label(s) in {}:", labels.len(), project));
    out.push('\n');
    for label in labels {
        out.line(format_args!("  {} ({})", label.name, label.color));
    }
    out
}

pub fn label_created(label: &Label) -> String {
    let mut out = String::new();
    out.line(format_args!(
        "Created label '{}' ({})",
        label.name, label.color
    ));
    out
}

pub fn label_updated(label: &Label) -> String {
    let mut out = String::new();
    out.line(format_args!(
        "Updated label '{}' ({})",
        label.name, label.color
    ));
    out
}

pub fn label_deleted(name: &str) -> String {
    let mut out = String::new();
    out.line(format_args!("Deleted label '{name}'"));
    out
}

// ── Folder ───────────────────────────────────────────────────

pub fn folder_list(folders: &[Folder], project: &str) -> String {
    let mut out = String::new();
    if folders.is_empty() {
        out.line(format_args!("No folders in {project}."));
        return out;
    }
    out.line(format_args!("{} folder(s) in {}:", folders.len(), project));
    out.push('\n');
    for folder in folders {
        out.line(format_args!("  {}", folder.name));
    }
    out
}

pub fn folder_created(folder: &Folder) -> String {
    let mut out = String::new();
    out.line(format_args!("Created folder '{}'", folder.name));
    out
}

pub fn folder_updated(previous_name: &str, folder: &Folder) -> String {
    let mut out = String::new();
    out.line(format_args!(
        "Renamed folder '{}' -> '{}'",
        previous_name, folder.name
    ));
    out
}

pub fn folder_deleted(name: &str) -> String {
    let mut out = String::new();
    out.line(format_args!("Deleted folder '{name}'"));
    out
}

// ── Export ───────────────────────────────────────────────────

pub fn export_written(written: &[PathBuf], output: &Path) -> String {
    let mut out = String::new();
    out.line(format_args!(
        "Exported {} file(s) to {}",
        written.len(),
        output.display()
    ));
    for path in written {
        out.line(format_args!("  {}", path.display()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(identifier: &str, status: Status, priority: Priority) -> Issue {
        Issue {
            id: 1,
            project_id: 1,
            sequence: 1,
            identifier: identifier.into(),
            title: "Fix the bug".into(),
            description: String::new(),
            status,
            priority,
            module_id: None,
            sort_order: 0.0,
            start_date: None,
            target_date: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            seq: 1,
            source: None,
            labels: Vec::new(),
            blocks: Vec::new(),
            blocked_by: Vec::new(),
            relates_to: Vec::new(),
            duplicates: Vec::new(),
            duplicated_by: Vec::new(),
        }
    }

    fn comment(id: i64, content: &str) -> Comment {
        Comment {
            id,
            issue_id: Some(1),
            page_id: None,
            user_id: 1,
            author: "ada".into(),
            author_display_name: "Ada".into(),
            content: content.into(),
            created_at: "2026-01-01 00:00:00".into(),
            updated_at: "2026-01-01 00:00:00".into(),
            seq: 1,
        }
    }

    /// The three continuation states have to read differently. A page that
    /// ends the thread says nothing extra, a page with more says where to go,
    /// and a page whose lookahead failed admits it instead of passing for a
    /// complete thread.
    #[test]
    fn distinguishes_a_finished_thread_from_an_unchecked_one() {
        let page = [comment(1, "hello")];

        let complete = comment_list(&page, "TST-1", CommentContinuation::End);
        assert!(!complete.contains("More comments"), "got: {complete}");
        assert!(!complete.contains("Could not check"), "got: {complete}");

        let more = comment_list(&page, "TST-1", CommentContinuation::Next(50));
        assert!(
            more.contains("More comments available. Next page: --offset 50\n"),
            "got: {more}"
        );

        let unknown = comment_list(&page, "TST-1", CommentContinuation::Unknown(50));
        assert!(
            unknown.contains(
                "Could not check whether more comments exist. \
                 If they do, the next page is --offset 50.\n"
            ),
            "got: {unknown}"
        );
        assert!(
            !unknown.contains("More comments available"),
            "got: {unknown}"
        );
    }

    #[test]
    fn marks_statuses_and_priorities_with_indicators() {
        assert_eq!(fmt_status(Status::Active), "[~] active");
        assert_eq!(fmt_status(Status::Cancelled), "[-] cancelled");
        assert_eq!(fmt_priority(Priority::Urgent), "!!!  urgent");
        assert_eq!(fmt_priority(Priority::None), "     none");
    }

    #[test]
    fn lists_issues_with_labels_and_module_name() {
        let mut one = issue("TST-1", Status::Active, Priority::High);
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
        let mut one = issue("TST-1", Status::Todo, Priority::Low);
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
        assert_eq!(
            comment_list(&[], "TST-1", CommentContinuation::End),
            "No comments on TST-1.\n"
        );
        assert_eq!(module_list(&[], "TST"), "No modules in TST.\n");
        assert_eq!(label_list(&[], "TST"), "No labels in TST.\n");
        assert_eq!(folder_list(&[], "TST"), "No folders in TST.\n");
    }

    #[test]
    fn details_an_issue_with_its_relations_and_description() {
        let mut one = issue("TST-1", Status::Active, Priority::Urgent);
        one.description = "Details".into();
        one.blocks = vec!["TST-2".into()];
        one.duplicated_by = vec!["TST-3".into()];

        assert_eq!(
            issue_detail(&one, &|_| None),
            "TST-1 - Fix the bug\n  Status:   active\n  Priority: urgent\n  \
             Blocks:   TST-2\n  DupedBy:  TST-3\n\nDetails\n"
        );
    }

    #[test]
    fn stored_controls_cannot_escape_renderer_lines_or_blocks() {
        let mut one = issue("TST-1", Status::Active, Priority::High);
        one.title = "forged\nSUCCESS\x1b]52;c;YQ==\x07\u{202e}".into();
        one.description = "line one\nline two\x1b[2J".into();

        let rendered = issue_detail(&one, &|_| None);

        assert!(rendered.starts_with("TST-1 - forged SUCCESS^[]52;c;YQ==  \n"));
        assert!(rendered.ends_with("line one\nline two^[[2J\n"));
    }

    #[test]
    fn stored_controls_are_inert_in_project_page_and_comment_renderers() {
        let project = Project {
            id: 1,
            name: "project\x1b]8;;https://evil\x1b\\".into(),
            identifier: "TST".into(),
            description: "description\u{009b}2J".into(),
            emoji: None,
            lead_user_id: None,
            sort_order: 0,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let page = Page {
            id: 1,
            seq: 0,
            project_id: Some(1),
            sequence: Some(1),
            identifier: "TST-DOC-1".into(),
            folder_id: None,
            title: "page\u{0008}".into(),
            content: "body\x1b]52;c;YQ==\x07".into(),
            sort_order: 0.0,
            status: "active".into(),
            pinned: false,
            created_at: String::new(),
            updated_at: String::new(),
            labels: vec!["label\u{202e}".into()],
        };
        let comment = Comment {
            id: 1,
            seq: 0,
            issue_id: Some(1),
            page_id: None,
            user_id: 1,
            author: "author\x1b[2J".into(),
            author_display_name: "display\u{2066}".into(),
            content: "comment\rforged\tline".into(),
            created_at: String::new(),
            updated_at: String::new(),
        };

        for rendered in [
            project_detail(&project),
            page_detail(&page),
            comment_list(&[comment], "TST-1", CommentContinuation::End),
        ] {
            assert!(
                !rendered.chars().any(|character| character.is_control()
                    && character != '\n'
                    && character != '\t')
            );
        }
    }

    #[test]
    fn json_output_preserves_control_characters_as_escaped_data() {
        let mut one = issue("TST-1", Status::Active, Priority::High);
        one.title = "stored\x1b]52;c;YQ==\x07".into();

        let encoded = serde_json::to_string(&one).unwrap();
        assert!(encoded.contains("\\u001b"));
        assert!(encoded.contains("\\u0007"));

        let decoded: Issue = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.title, one.title);
    }
}
