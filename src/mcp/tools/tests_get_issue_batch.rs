//! LIF-481: get_issue reads several issues in one call.

use super::tests::{mcp, seed_issue, seed_project};
use super::*;
use rmcp::handler::server::wrapper::Parameters;

fn batch(m: &LificMcp, identifiers: &[&str]) -> String {
    m.get_issue(Parameters(GetIssueInput {
        identifiers: Some(identifiers.iter().map(|id| (*id).to_string()).collect()),
        ..Default::default()
    }))
}

fn single(m: &LificMcp, identifier: &str) -> String {
    m.get_issue(Parameters(GetIssueInput {
        identifier: identifier.into(),
        ..Default::default()
    }))
}

#[test]
fn a_batch_reads_three_issues_and_reports_an_unknown_one_inline() {
    let (m, _guard) = mcp();
    seed_project(&m, "Batch", "BAT");
    seed_issue(&m, "BAT", "First read");
    seed_issue(&m, "BAT", "Second read");

    let output = batch(&m, &["BAT-2", "BAT-99", "BAT-1"]);

    assert!(!output.starts_with("Error"), "got: {output}");
    let blocks: Vec<&str> = output.split("\n\n=====\n\n").collect();
    assert_eq!(blocks.len(), 3, "got: {output}");
    assert_eq!(blocks[0], single(&m, "BAT-2").trim_end());
    assert_eq!(blocks[2], single(&m, "BAT-1").trim_end());
    assert!(
        blocks[1].starts_with("BAT-99: Error: ") && blocks[1].contains("not found"),
        "unknown id is its own error line: {}",
        blocks[1]
    );
}

#[test]
fn a_batch_applies_include_comments_to_every_issue() {
    let (m, _guard) = mcp();
    seed_project(&m, "Batch", "BAT");
    seed_issue(&m, "BAT", "One");
    seed_issue(&m, "BAT", "Two");

    let output = m.get_issue(Parameters(GetIssueInput {
        identifiers: Some(vec!["BAT-1".into(), "BAT-2".into()]),
        include_comments: Some("none".into()),
        ..Default::default()
    }));
    assert_eq!(
        output.matches("--- Comments (0, omitted").count(),
        2,
        "got: {output}"
    );
}

#[test]
fn a_batch_refuses_bad_shapes_before_reading_anything() {
    let (m, _guard) = mcp();
    seed_project(&m, "Batch", "BAT");
    seed_issue(&m, "BAT", "Only");

    let both = m.get_issue(Parameters(GetIssueInput {
        identifier: "BAT-1".into(),
        identifiers: Some(vec!["BAT-1".into()]),
        ..Default::default()
    }));
    assert!(
        both.starts_with("Error: pass identifier or identifiers"),
        "got: {both}"
    );

    let empty = batch(&m, &[]);
    assert!(
        empty.starts_with("Error: identifiers is empty"),
        "got: {empty}"
    );

    let ids: Vec<String> = (1..=51).map(|n| format!("BAT-{n}")).collect();
    let too_many = m.get_issue(Parameters(GetIssueInput {
        identifiers: Some(ids),
        ..Default::default()
    }));
    assert!(too_many.contains("at most 50"), "got: {too_many}");

    let typo = m.get_issue(Parameters(GetIssueInput {
        identifiers: Some(vec!["BAT-1".into()]),
        include_comments: Some("some".into()),
        ..Default::default()
    }));
    assert!(
        typo.starts_with("Error: invalid include_comments"),
        "got: {typo}"
    );

    let neither = m.get_issue(Parameters(GetIssueInput::default()));
    assert!(
        neither.starts_with("Error: identifier is required"),
        "got: {neither}"
    );
}

#[test]
fn a_batch_stops_at_the_response_budget_and_names_what_it_left_out() {
    let (m, _guard) = mcp();
    seed_project(&m, "Batch", "BAT");
    for title in ["Big one", "Big two", "Big three"] {
        seed_issue(&m, "BAT", title);
    }
    {
        // 900 KB each: two fit the 2 MiB budget, the third does not.
        let conn = m.db.write().unwrap();
        conn.execute(
            "UPDATE issues SET description = ?1",
            [format!("big {}", "x".repeat(900 * 1024))],
        )
        .unwrap();
    }

    let output = batch(&m, &["BAT-1", "BAT-2", "BAT-3"]);

    assert!(
        output.contains("Big one") && output.contains("Big two"),
        "first two shown"
    );
    assert!(!output.contains("Big three"), "third left out");
    assert!(
        output.ends_with(
            "Not shown, past the response size budget: BAT-3. Read them in another call."
        ),
        "tail: {}",
        &output[output.len().saturating_sub(200)..]
    );
    assert!(
        encoded_tool_result_bytes(&output).unwrap()
            <= queries::comments::MAX_COMMENT_RESPONSE_BYTES
    );
}

#[tokio::test]
async fn a_batch_reports_an_issue_the_caller_cannot_view_without_its_contents() {
    let (m, _, _, _, viewer, _, project_id, _guard) = super::tests::setup_membership_mcp();
    let [blocker, visible, hidden] = {
        let conn = m.db.write().unwrap();
        crate::export::seed_hidden_relation(&conn, project_id)
    };
    let hidden_title = m
        .read(|conn| queries::get_issue(conn, queries::resolve_identifier(conn, &hidden)?))
        .unwrap()
        .title;

    let m2 = m.clone();
    let ids = vec![blocker.clone(), hidden.clone(), visible.clone()];
    let output = crate::mcp::with_request_user(Some(viewer), || async move {
        m2.get_issue(Parameters(GetIssueInput {
            identifiers: Some(ids),
            ..Default::default()
        }))
    })
    .await;

    let blocks: Vec<&str> = output.split("\n\n=====\n\n").collect();
    assert_eq!(blocks.len(), 3, "got: {output}");
    assert!(
        blocks[1].starts_with(&format!("{hidden}: Error: ")),
        "got: {}",
        blocks[1]
    );
    assert!(
        !output.contains(&hidden_title),
        "leaked the hidden title: {output}"
    );
    assert!(blocks[0].contains(&blocker) && blocks[2].contains(&visible));
}

#[test]
fn get_issue_input_accepts_identifiers_without_identifier() {
    let input: GetIssueInput =
        serde_json::from_value(serde_json::json!({"identifiers": ["BAT-1", "BAT-2"]})).unwrap();
    assert!(input.identifier.is_empty());
    assert_eq!(input.identifiers.unwrap(), ["BAT-1", "BAT-2"]);
}
