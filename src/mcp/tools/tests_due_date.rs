//! LIF-480: list and board rows carry the due date when one is set.

use super::tests::{mcp, seed_project};
use super::*;
use rmcp::handler::server::wrapper::Parameters;

fn create(m: &LificMcp, title: &str, target_date: Option<&str>) {
    let result = m.create_issue(Parameters(CreateIssueInput {
        project: Some("DUE".into()),
        title: title.into(),
        target_date: target_date.map(Into::into),
        ..Default::default()
    }));
    assert!(result.starts_with("Created"), "got: {result}");
}

fn line(output: &str, title: &str) -> String {
    output
        .lines()
        .find(|line| line.contains(title))
        .unwrap_or_else(|| panic!("no line for {title}: {output}"))
        .to_string()
}

#[test]
fn list_issues_rows_show_due_date_only_when_set() {
    let (m, _guard) = mcp();
    seed_project(&m, "Due", "DUE");
    create(&m, "Deadline", Some("2026-11-20"));
    create(&m, "Open ended", None);

    let listed = m.list_issues(Parameters(ListIssuesInput {
        project: Some("DUE".into()),
        ..Default::default()
    }));
    assert_eq!(
        line(&listed, "Deadline"),
        "- DUE-1 | backlog | none | Deadline due:2026-11-20",
        "got: {listed}"
    );
    assert_eq!(
        line(&listed, "Open ended"),
        "- DUE-2 | backlog | none | Open ended",
        "got: {listed}"
    );
}

#[test]
fn due_date_precedes_checklist_progress() {
    let (m, _guard) = mcp();
    seed_project(&m, "Due", "DUE");
    let result = m.create_issue(Parameters(CreateIssueInput {
        project: Some("DUE".into()),
        title: "Both".into(),
        description: Some("- [x] one\n- [ ] two".into()),
        target_date: Some("2026-12-01".into()),
        ..Default::default()
    }));
    assert!(result.starts_with("Created"), "got: {result}");

    let listed = m.list_issues(Parameters(ListIssuesInput {
        project: Some("DUE".into()),
        ..Default::default()
    }));
    assert!(
        line(&listed, "Both").ends_with("| Both due:2026-12-01 checklist: 1/2"),
        "got: {listed}"
    );
}

#[test]
fn get_board_rows_show_due_date_only_when_set() {
    let (m, _guard) = mcp();
    seed_project(&m, "Due", "DUE");
    create(&m, "Deadline", Some("2026-11-20"));
    create(&m, "Open ended", None);

    let board = m.get_board(Parameters(GetBoardInput {
        project: Some("DUE".into()),
        ..Default::default()
    }));
    assert!(
        line(&board, "Deadline").ends_with("| Deadline due:2026-11-20"),
        "got: {board}"
    );
    assert!(
        line(&board, "Open ended").ends_with("| Open ended"),
        "got: {board}"
    );
}

#[test]
fn blank_due_date_renders_nothing() {
    let (m, _guard) = mcp();
    seed_project(&m, "Due", "DUE");
    create(&m, "Blank", Some(""));

    let listed = m.list_issues(Parameters(ListIssuesInput {
        project: Some("DUE".into()),
        ..Default::default()
    }));
    assert!(!line(&listed, "Blank").contains("due"), "got: {listed}");
}
