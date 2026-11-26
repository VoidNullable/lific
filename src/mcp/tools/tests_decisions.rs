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
fn decision_summaries_show_the_newest_fifty_and_point_to_the_rest() {
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
        50
    );
    assert!(result.contains("Decision 501"), "{result}");
    assert!(!result.contains("- #1:"), "{result}");
    assert!(result.contains(
        "- (452 earlier decisions not shown: list_comments identifier=DEC-1 kind=decision)"
    ));
    assert!(!result.contains("body body"));
}

#[test]
fn long_decisions_have_their_own_byte_budget_in_recent_and_none_modes() {
    let (m, _guard) = mcp();
    seed_project(&m, "Decisions", "DEC");
    seed_issue(&m, "DEC", "Choose storage");
    m.transaction(|conn| {
        for i in 0..100 {
            conn.execute(
                "INSERT INTO comments(issue_id,user_id,content,kind) VALUES(1,1,?1,'decision')",
                [format!(
                    "## Decision\nDecision {i}: {}\n{}",
                    "🦀".repeat(400),
                    "body ".repeat(20_000)
                )],
            )?;
        }
        Ok(())
    })
    .unwrap();
    for mode in ["recent", "none"] {
        let result = m.get_issue(Parameters(GetIssueInput {
            identifier: "DEC-1".into(),
            include_comments: Some(mode.into()),
            ..Default::default()
        }));
        assert!(!result.starts_with("Error:"), "{result}");
        let section = result
            .split("--- Comments")
            .next()
            .unwrap()
            .split("\nDecisions:\n")
            .nth(1)
            .unwrap();
        let shown = section
            .lines()
            .filter(|line| line.starts_with("- #"))
            .count();
        assert!(shown > 0 && shown < 50, "{shown}");
        assert!(section.contains("- #100: Decision 99:"), "{section}");
        assert!(section.contains(&format!(
            "{} earlier decisions not shown: list_comments identifier=DEC-1 kind=decision",
            100 - shown
        )));
        assert!(
            encoded_tool_result_bytes(&format!("\nDecisions:\n{section}")).unwrap()
                - encoded_tool_result_bytes("").unwrap()
                <= 32 * 1024
        );
    }
    m.transaction(|conn| {
        conn.execute(
            "UPDATE issues SET description=?1 WHERE id=1",
            ["d".repeat(queries::comments::MAX_COMMENT_RESPONSE_BYTES - 16 * 1024)],
        )?;
        Ok(())
    })
    .unwrap();
    for mode in ["recent", "none"] {
        let result = m.get_issue(Parameters(GetIssueInput {
            identifier: "DEC-1".into(),
            include_comments: Some(mode.into()),
            ..Default::default()
        }));
        assert!(
            !result.starts_with("Error:"),
            "{}",
            &result[..result.len().min(300)]
        );
        assert!(result.contains("list_comments identifier=DEC-1 kind=decision"));
        assert!(
            encoded_tool_result_bytes(&result).unwrap()
                <= queries::comments::MAX_COMMENT_RESPONSE_BYTES
        );
    }
}

#[test]
fn list_comments_filters_kind_before_counting_and_paging() {
    let (m, _guard) = mcp();
    seed_project(&m, "Decisions", "DEC");
    seed_issue(&m, "DEC", "Choose storage");
    m.transaction(|conn| {
        for (content, kind) in [
            ("Discussion", "comment"),
            ("Earlier choice", "decision"),
            ("Tests pass", "verification"),
            ("Latest choice", "decision"),
        ] {
            conn.execute(
                "INSERT INTO comments(issue_id,user_id,content,kind) VALUES(1,1,?1,?2)",
                rusqlite::params![content, kind],
            )?;
        }
        Ok(())
    })
    .unwrap();
    let input = |offset| {
        serde_json::from_value::<ListCommentsInput>(serde_json::json!({
            "identifier":"DEC-1", "kind":"decision", "limit":1, "offset":offset
        }))
        .unwrap()
    };
    let first = m.list_comments(Parameters(input(0)));
    assert!(
        first.contains("Showing 1 most recent of 2 comment(s)"),
        "{first}"
    );
    assert!(first.contains("Latest choice"), "{first}");
    assert!(
        !first.contains("Discussion") && !first.contains("Tests pass"),
        "{first}"
    );
    assert!(
        first.contains("same kind/author/order/limit and offset=1"),
        "{first}"
    );
    let second = m.list_comments(Parameters(input(1)));
    assert!(
        second.contains("Earlier choice") && !second.contains("Latest choice"),
        "{second}"
    );
    let empty = m.list_comments(Parameters(ListCommentsInput {
        author: Some("nobody".into()),
        ..input(0)
    }));
    assert!(empty.starts_with("No comments on"), "{empty}");
    let verification = m.list_comments(Parameters(ListCommentsInput {
        kind: Some(models::CommentKind::Verification),
        ..input(0)
    }));
    assert!(
        verification.contains("[verification]") && verification.contains("1 comment(s)"),
        "{verification}"
    );
    assert!(
        serde_json::from_value::<ListCommentsInput>(serde_json::json!({
            "identifier":"DEC-1", "kind":"unknown"
        }))
        .is_err()
    );
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
