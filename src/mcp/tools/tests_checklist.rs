//! LIF-487: acceptance checklist progress for issues with task lists.

use super::tests::{mcp, seed_project};
use super::*;
use rmcp::handler::server::wrapper::Parameters;

const CRITERIA: &str =
    "Done when:\n- [x] parser\n- [ ] tests\n* [X] docs\n\n```markdown\n- [ ] quoted example\n```\n";

fn create(m: &LificMcp, title: &str, description: Option<&str>) {
    let result = m.create_issue(Parameters(CreateIssueInput {
        project: Some("CHK".into()),
        title: title.into(),
        description: description.map(Into::into),
        ..Default::default()
    }));
    assert!(result.starts_with("Created"), "got: {result}");
}

fn get(m: &LificMcp, identifier: &str) -> String {
    m.get_issue(Parameters(GetIssueInput {
        identifier: identifier.into(),
        ..Default::default()
    }))
}

fn set_status(m: &LificMcp, identifier: &str, status: &str) -> String {
    m.update_issue(Parameters(UpdateIssueInput {
        identifier: identifier.into(),
        status: Some(status.into()),
        ..Default::default()
    }))
}

#[test]
fn get_issue_shows_checklist_progress_without_fenced_examples() {
    let (m, _guard) = mcp();
    seed_project(&m, "Checklist", "CHK");
    create(&m, "With criteria", Some(CRITERIA));

    let detail = get(&m, "CHK-1");
    assert!(detail.contains("\nChecklist: 2/3 done\n"), "got: {detail}");
}

#[test]
fn get_issue_has_no_checklist_line_without_task_items() {
    let (m, _guard) = mcp();
    seed_project(&m, "Checklist", "CHK");
    create(
        &m,
        "Plain",
        Some("- a bullet\n```\n- [ ] only in code\n```"),
    );
    create(&m, "Empty", None);

    for identifier in ["CHK-1", "CHK-2"] {
        let detail = get(&m, identifier);
        assert!(!detail.contains("Checklist"), "{identifier}: {detail}");
    }
}

#[test]
fn list_issues_marks_only_issues_with_task_items() {
    let (m, _guard) = mcp();
    seed_project(&m, "Checklist", "CHK");
    create(&m, "With criteria", Some(CRITERIA));
    create(&m, "Plain", Some("No task list here."));

    let listed = m.list_issues(Parameters(ListIssuesInput {
        project: Some("CHK".into()),
        ..Default::default()
    }));
    let line = |title: &str| {
        listed
            .lines()
            .find(|line| line.contains(title))
            .unwrap_or_else(|| panic!("no line for {title}: {listed}"))
            .to_string()
    };
    assert!(
        line("With criteria").ends_with("With criteria checklist: 2/3"),
        "got: {listed}"
    );
    assert!(line("Plain").ends_with("| Plain"), "got: {listed}");
    assert!(!line("Plain").contains("checklist"), "got: {listed}");
}

#[test]
fn closing_with_unchecked_items_succeeds_with_a_warning() {
    let (m, _guard) = mcp();
    seed_project(&m, "Checklist", "CHK");
    create(&m, "With criteria", Some(CRITERIA));

    let result = set_status(&m, "CHK-1", "done");
    assert!(result.contains("| done |"), "the close must land: {result}");
    assert!(
        result.ends_with("\nWarning: 1 of 3 checklist items still unchecked."),
        "got: {result}"
    );
    assert!(get(&m, "CHK-1").contains("Status: done"));
}

#[test]
fn no_warning_when_every_item_is_checked_or_there_is_no_list() {
    let (m, _guard) = mcp();
    seed_project(&m, "Checklist", "CHK");
    create(&m, "All checked", Some("- [x] one\n- [X] two"));
    create(&m, "Plain", None);
    create(&m, "Open", Some("- [ ] pending"));

    for identifier in ["CHK-1", "CHK-2"] {
        let result = set_status(&m, identifier, "done");
        assert!(!result.contains("Warning"), "{identifier}: {result}");
    }
    // Only a close warns; other status changes stay quiet.
    let result = set_status(&m, "CHK-3", "active");
    assert!(!result.contains("Warning"), "got: {result}");
}
