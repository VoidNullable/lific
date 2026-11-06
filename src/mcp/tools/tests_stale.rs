//! LIF-492: `list_issues(stale=true)` finds active issues idle past the
//! instance threshold.

use super::tests::{mcp, seed_project};
use super::*;
use rmcp::handler::server::wrapper::Parameters;

fn create(m: &LificMcp, title: &str, status: &str) {
    let result = m.create_issue(Parameters(CreateIssueInput {
        project: Some("STA".into()),
        title: title.into(),
        status: Some(status.into()),
        ..Default::default()
    }));
    assert!(result.starts_with("Created"), "got: {result}");
}

/// Fixed timestamps, far enough from today that the real clock the tool reads
/// cannot move an issue across the threshold.
fn pin(m: &LificMcp, title: &str, updated_at: &str) {
    m.write(|conn| {
        conn.execute(
            "UPDATE issues SET updated_at = ?1, seq = seq + 1000000 WHERE title = ?2",
            rusqlite::params![updated_at, title],
        )?;
        Ok(())
    })
    .unwrap();
}

fn stale(m: &LificMcp, input: ListIssuesInput) -> String {
    m.list_issues(Parameters(ListIssuesInput {
        project: Some("STA".into()),
        stale: Some(true),
        ..input
    }))
}

#[test]
fn stale_lists_only_idle_active_issues() {
    let (m, _guard) = mcp();
    seed_project(&m, "Stale list", "STA");
    create(&m, "Abandoned claim", "active");
    create(&m, "Fresh claim", "active");
    create(&m, "Old todo", "todo");
    pin(&m, "Abandoned claim", "2001-01-01 00:00:00");
    pin(&m, "Old todo", "2001-01-01 00:00:00");

    let out = stale(&m, ListIssuesInput::default());
    assert!(out.starts_with("1 issues:"), "{out}");
    assert!(out.contains("Abandoned claim"), "{out}");
    assert!(!out.contains("Fresh claim"), "{out}");
    assert!(!out.contains("Old todo"), "{out}");

    // A caller's own updated_until still narrows the result.
    let out = stale(
        &m,
        ListIssuesInput {
            updated_until: Some("2000-01-01".into()),
            ..Default::default()
        },
    );
    assert_eq!(out, "No issues found.");

    // Any other status contradicts the filter.
    let out = stale(
        &m,
        ListIssuesInput {
            status: Some("todo".into()),
            ..Default::default()
        },
    );
    assert!(out.contains("stale lists active issues only"), "{out}");
}
