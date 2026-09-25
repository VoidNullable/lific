//! Agent-input tolerance for the MCP tools: camelCase edit keys (LIF-472).

use super::tests::{comment_id_from, mcp, seed_issue, seed_project};
use super::*;
use rmcp::handler::server::wrapper::Parameters;
use serde_json::json;

/// Deserialize exactly as the MCP transport does, from the raw JSON object.
fn parse<T: serde::de::DeserializeOwned>(arguments: serde_json::Value) -> T {
    serde_json::from_value(arguments).expect("arguments deserialize")
}

// ── LIF-472: camelCase oldString/newString/replaceAll ────────

#[test]
fn edit_issue_accepts_camel_case_keys() {
    let (m, _guard) = mcp();
    seed_project(&m, "Camel", "CML");
    m.create_issue(Parameters(CreateIssueInput {
        project: Some("CML".into()),
        title: "Camel".into(),
        description: Some("one fish, one fish".into()),
        ..Default::default()
    }));

    let edited = m.edit_issue(Parameters(parse(json!({
        "identifier": "CML-1",
        "oldString": "one",
        "newString": "two",
        "replaceAll": true,
    }))));
    assert!(!edited.starts_with("Error"), "got: {edited}");
    let issue = m
        .read(|conn| queries::get_issue(conn, queries::resolve_identifier(conn, "CML-1")?))
        .unwrap();
    assert_eq!(issue.description, "two fish, two fish");
}

#[test]
fn edit_page_accepts_camel_case_keys() {
    let (m, _guard) = mcp();
    seed_project(&m, "Camel", "CML");
    m.create_page(Parameters(CreatePageInput {
        project: Some("CML".into()),
        title: "Notes".into(),
        content: Some("draft text".into()),
        ..Default::default()
    }));

    let edited = m.edit_page(Parameters(parse(json!({
        "identifier": "CML-DOC-1",
        "oldString": "draft",
        "newString": "final",
    }))));
    assert!(!edited.starts_with("Error"), "got: {edited}");
    let page = m
        .read(|conn| queries::get_page(conn, queries::resolve_page_identifier(conn, "CML-DOC-1")?))
        .unwrap();
    assert_eq!(page.content, "final text");
}

#[test]
fn edit_comment_accepts_camel_case_keys() {
    let (m, _guard) = mcp();
    seed_project(&m, "Camel", "CML");
    seed_issue(&m, "CML", "Commented");
    let added = m.add_comment(Parameters(AddCommentInput {
        identifier: "CML-1".into(),
        content: "a b a".into(),
    }));
    let cid = comment_id_from(&added);

    let edited = m.edit_comment(Parameters(parse(json!({
        "comment_id": cid,
        "oldString": "a",
        "newString": "c",
        "replaceAll": true,
    }))));
    assert!(!edited.starts_with("Error"), "got: {edited}");
    let body = m
        .read(|conn| queries::comments::get_comment(conn, cid))
        .unwrap()
        .content;
    assert_eq!(body, "c b c");
}

#[test]
fn edit_plan_step_accepts_camel_case_keys() {
    let (m, _guard) = mcp();
    seed_project(&m, "Camel", "CML");
    let created = m.create_plan(Parameters(CreatePlanInput {
        project: Some("CML".into()),
        title: "Plan".into(),
        steps: Some(vec![PlanStepInput {
            title: "Write the draft".into(),
            ..Default::default()
        }]),
        ..Default::default()
    }));
    let step_id: i64 = created
        .split_once(" Write the draft")
        .and_then(|(head, _)| head.rsplit_once('#'))
        .and_then(|(_, id)| id.parse().ok())
        .unwrap_or_else(|| panic!("no step id in: {created}"));

    let edited = m.edit_plan_step(Parameters(parse(json!({
        "plan": "CML-PLAN-1",
        "step_id": step_id,
        "field": "title",
        "oldString": "draft",
        "newString": "final",
    }))));
    assert!(edited.contains("Edited step"), "got: {edited}");
    let plan = m.get_plan(Parameters(GetPlanInput {
        plan: "CML-PLAN-1".into(),
    }));
    assert!(plan.contains("Write the final"), "got: {plan}");
}

#[test]
fn edit_inputs_still_accept_snake_case_keys() {
    let snake: EditIssueInput = parse(json!({
        "identifier": "CML-1",
        "old_string": "a",
        "new_string": "b",
        "replace_all": true,
    }));
    assert_eq!(
        (snake.old_string.as_str(), snake.new_string.as_str()),
        ("a", "b")
    );
    assert_eq!(snake.replace_all, Some(true));
}
