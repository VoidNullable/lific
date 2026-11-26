//! LIF-526: `op_key` on the create tools. A retry with the key of a call that
//! committed replays that call's result instead of writing again.

use super::tests::{mcp, seed_issue, seed_project};
use super::*;
use rmcp::handler::server::wrapper::Parameters;

fn count(m: &LificMcp, table: &str) -> i64 {
    m.read(|conn| {
        Ok(
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })?,
        )
    })
    .unwrap()
}

fn create(m: &LificMcp, title: &str, op_key: &str) -> String {
    m.create_issue(Parameters(CreateIssueInput {
        project: Some("TST".into()),
        title: title.into(),
        op_key: Some(op_key.into()),
        ..Default::default()
    }))
}

fn item(title: &str) -> CreateIssueItem {
    CreateIssueItem {
        title: title.into(),
        ..Default::default()
    }
}

fn create_batch(m: &LificMcp, items: Vec<CreateIssueItem>, op_key: &str) -> String {
    m.create_issue(Parameters(CreateIssueInput {
        project: Some("TST".into()),
        issues: Some(items),
        op_key: Some(op_key.into()),
        ..Default::default()
    }))
}

const REPLAYED: &str = "this call already ran, so nothing new was written";

#[test]
fn a_retried_create_after_a_lost_reply_returns_the_same_issue_and_writes_once() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");

    // The first call commits; its reply never reaches the agent.
    let _lost = create(&m, "Ship it", "retry-1");
    let retried = create(&m, "Ship it", "retry-1");

    assert!(retried.contains(REPLAYED), "got: {retried}");
    assert!(retried.contains("Created TST-1: Ship it"), "got: {retried}");
    assert_eq!(count(&m, "issues"), 1);
    assert_eq!(count(&m, "op_keys"), 1);
}

#[test]
fn a_create_without_op_key_still_writes_every_time() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");
    seed_issue(&m, "TST", "Twice");
    seed_issue(&m, "TST", "Twice");
    assert_eq!(count(&m, "issues"), 2);
    assert_eq!(count(&m, "op_keys"), 0);
}

#[test]
fn the_same_op_key_with_a_different_title_is_refused_and_writes_nothing() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");
    create(&m, "First", "shared");

    let refused = create(&m, "Second", "shared");

    assert!(refused.starts_with("Error"), "got: {refused}");
    assert!(refused.contains("different request"), "got: {refused}");
    assert_eq!(count(&m, "issues"), 1);
    assert_eq!(count(&m, "op_keys"), 1);
    // A new key is a new write.
    assert!(create(&m, "Second", "fresh").starts_with("Created TST-2"));
}

#[test]
fn a_refused_create_records_no_key_so_a_corrected_retry_goes_through() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");
    let refused = m.create_issue(Parameters(CreateIssueInput {
        project: Some("TST".into()),
        title: "Typo".into(),
        status: Some("doing".into()),
        op_key: Some("fix-me".into()),
        ..Default::default()
    }));
    assert!(refused.starts_with("Error"), "got: {refused}");
    assert_eq!(count(&m, "op_keys"), 0);

    let corrected = create(&m, "Typo", "fix-me");
    assert!(corrected.starts_with("Created TST-1"), "got: {corrected}");
    assert!(!corrected.contains(REPLAYED));
}

#[test]
fn a_retried_batch_replays_every_item_and_writes_none_again() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");
    let first = create_batch(&m, vec![item("One"), item("Two"), item("Three")], "batch");
    assert!(first.starts_with("Created 3 issues:"), "got: {first}");

    let retried = create_batch(&m, vec![item("One"), item("Two"), item("Three")], "batch");

    assert!(retried.contains(REPLAYED), "got: {retried}");
    for line in ["- TST-1: One", "- TST-2: Two", "- TST-3: Three"] {
        assert!(retried.contains(line), "missing {line} in: {retried}");
    }
    assert_eq!(count(&m, "issues"), 3);
}

#[test]
fn a_failed_batch_records_no_key_and_a_corrected_batch_creates_all_of_it() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");
    let bad = CreateIssueItem {
        title: "Bad".into(),
        status: Some("doing".into()),
        ..Default::default()
    };
    let refused = create_batch(&m, vec![item("One"), bad], "batch");
    assert!(refused.starts_with("Error"), "got: {refused}");
    assert_eq!(count(&m, "issues"), 0);
    assert_eq!(count(&m, "op_keys"), 0);

    let created = create_batch(&m, vec![item("One"), item("Two")], "batch");
    assert!(created.starts_with("Created 2 issues:"), "got: {created}");
    assert_eq!(count(&m, "issues"), 2);
}

#[test]
fn a_batch_key_reused_for_a_single_create_is_refused() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");
    create_batch(&m, vec![item("One")], "batch");
    let refused = create(&m, "One", "batch");
    assert!(refused.contains("different request"), "got: {refused}");
    assert_eq!(count(&m, "issues"), 1);
}

#[test]
fn add_comment_replays_on_retry() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");
    seed_issue(&m, "TST", "Talk");
    let comment = || {
        m.add_comment(Parameters(AddCommentInput {
            identifier: "TST-1".into(),
            content: "Looks good".into(),
            op_key: Some("c1".into()),
        }))
    };
    let first = comment();
    let retried = comment();
    assert!(retried.contains(REPLAYED), "got: {retried}");
    assert!(
        retried.ends_with(&first),
        "first: {first}\nretried: {retried}"
    );
    assert_eq!(count(&m, "comments"), 1);
}

#[test]
fn create_page_replays_on_retry() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");
    let page = || {
        m.create_page(Parameters(CreatePageInput {
            project: Some("TST".into()),
            title: "Notes".into(),
            op_key: Some("p1".into()),
            ..Default::default()
        }))
    };
    let first = page();
    let retried = page();
    assert!(first.starts_with("Created"), "got: {first}");
    assert!(
        retried.ends_with(&first),
        "first: {first}\nretried: {retried}"
    );
    assert_eq!(count(&m, "pages"), 1);
}

#[test]
fn create_plan_replays_on_retry() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");
    let plan = || {
        m.create_plan(Parameters(CreatePlanInput {
            project: Some("TST".into()),
            title: "Roadmap".into(),
            steps: Some(vec![PlanStepInput {
                title: "Begin".into(),
                ..Default::default()
            }]),
            op_key: Some("plan1".into()),
            ..Default::default()
        }))
    };
    let first = plan();
    let retried = plan();
    assert!(first.starts_with("Created"), "got: {first}");
    assert!(
        retried.ends_with(&first),
        "first: {first}\nretried: {retried}"
    );
    assert_eq!(count(&m, "plans"), 1);
    assert_eq!(count(&m, "plan_steps"), 1);
}

/// Agents only learn the retry rule from the schema, so every create tool
/// must advertise `op_key` and say to reuse it.
#[test]
fn every_create_tool_tells_agents_to_reuse_op_key_on_retry() {
    let (m, _guard) = mcp();
    let schemas: std::collections::HashMap<_, _> = m.list_tool_schemas().into_iter().collect();
    for tool in ["create_issue", "add_comment", "create_page", "create_plan"] {
        let description = schemas[tool]["properties"]["op_key"]["description"]
            .as_str()
            .unwrap_or_else(|| panic!("{tool} does not advertise op_key"));
        assert!(description.contains("retrying"), "{tool}: {description}");
        assert!(description.contains("reuse"), "{tool}: {description}");
    }
}
