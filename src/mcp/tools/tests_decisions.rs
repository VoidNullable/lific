use super::tests::{mcp, seed_issue, seed_project};
use super::*;
use rmcp::handler::server::wrapper::Parameters;

#[test]
fn decisions_survive_every_comment_window_and_deleted_ones_disappear() {
    let (m, _guard) = mcp();
    seed_project(&m, "Decisions", "DEC");
    seed_issue(&m, "DEC", "Choose storage");
    let input: AddCommentInput = serde_json::from_value(serde_json::json!({
        "identifier": "DEC-1", "content": "\nUse SQLite\nThe full rationale.", "kind": "decision"
    }))
    .unwrap();
    let result = m.add_comment(Parameters(input));
    assert!(!result.starts_with("Error:"), "{result}");
    for i in 0..6 {
        m.add_comment(Parameters(AddCommentInput {
            identifier: "DEC-1".into(),
            content: format!("Discussion {i}"),
            ..Default::default()
        }));
    }
    for mode in ["recent", "all", "none"] {
        let result = m.get_issue(Parameters(GetIssueInput {
            identifier: "DEC-1".into(),
            include_comments: Some(mode.into()),
            ..Default::default()
        }));
        assert!(
            result.contains("Decisions:\n- #1: Use SQLite\n"),
            "{result}"
        );
        if mode != "all" {
            assert!(!result.contains("The full rationale."), "{result}");
        }
    }
    let listed = m.list_comments(Parameters(ListCommentsInput {
        identifier: "DEC-1".into(),
        ..Default::default()
    }));
    assert!(listed.contains("[decision]"), "{listed}");
    m.transaction(|conn| queries::comments::delete_comment(conn, 1))
        .unwrap();
    let result = m.get_issue(Parameters(GetIssueInput {
        identifier: "DEC-1".into(),
        ..Default::default()
    }));
    assert!(!result.contains("Decisions:"), "{result}");
}

#[test]
fn decision_summaries_cover_more_than_the_comment_row_cap() {
    let (m, _guard) = mcp();
    seed_project(&m, "Decisions", "DEC");
    seed_issue(&m, "DEC", "Choose storage");
    m.transaction(|conn| {
        for i in 0..502 {
            conn.execute(
                "INSERT INTO comments(issue_id, user_id, content, kind) VALUES(1, 1, ?1, 'decision')",
                [format!("Decision {i}\n{}", "body ".repeat(1000))],
            )?;
        }
        Ok(())
    }).unwrap();
    let result = m.get_issue(Parameters(GetIssueInput {
        identifier: "DEC-1".into(),
        include_comments: Some("none".into()),
        ..Default::default()
    }));
    assert_eq!(
        result
            .lines()
            .filter(|line| line.starts_with("- #"))
            .count(),
        502
    );
    assert!(result.contains("Decision 501"), "{result}");
    assert!(!result.contains("body body"));
}

#[test]
fn comment_kind_defaults_and_rejects_reserved_or_unknown_values() {
    let input: AddCommentInput = serde_json::from_value(serde_json::json!({
        "identifier": "DEC-1", "content": "ordinary"
    }))
    .unwrap();
    assert!(matches!(input.kind, models::CommentWriteKind::Comment));
    for kind in ["verification", "note", "Decision"] {
        assert!(
            serde_json::from_value::<AddCommentInput>(serde_json::json!({
                "identifier": "DEC-1", "content": "bad kind", "kind": kind
            }))
            .is_err()
        );
    }
}
