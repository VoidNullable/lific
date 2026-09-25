//! LIF-486: closing an issue can record typed verification evidence.

use super::tests::{mcp, seed_issue, seed_project};
use super::*;
use rmcp::handler::server::wrapper::Parameters;

fn close(m: &LificMcp, identifier: &str, evidence: Option<&str>) -> String {
    m.update_issue(Parameters(UpdateIssueInput {
        identifier: identifier.into(),
        status: Some("done".into()),
        evidence: evidence.map(Into::into),
        ..Default::default()
    }))
}

fn get(m: &LificMcp, identifier: &str) -> String {
    m.get_issue(Parameters(GetIssueInput {
        identifier: identifier.into(),
        ..Default::default()
    }))
}

fn comment_kinds(m: &LificMcp) -> Vec<String> {
    m.read(|conn| {
        let mut stmt = conn.prepare("SELECT kind FROM comments ORDER BY id")?;
        let kinds = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(kinds)
    })
    .unwrap()
}

#[test]
fn closing_with_evidence_records_a_marked_verification_comment() {
    let (m, _guard) = mcp();
    seed_project(&m, "Verify", "VER");
    seed_issue(&m, "VER", "Ship it");

    let result = close(&m, "VER-1", Some("cargo test: 2595 passed"));
    assert!(result.contains("| done |"), "got: {result}");
    assert!(
        result.contains("Verification recorded as comment #"),
        "got: {result}"
    );

    let detail = get(&m, "VER-1");
    assert!(
        detail.contains("[verification] admin"),
        "get_issue must mark the evidence: {detail}"
    );
    assert!(detail.contains("cargo test: 2595 passed"), "got: {detail}");

    let listed = m.list_comments(Parameters(ListCommentsInput {
        identifier: "VER-1".into(),
        ..Default::default()
    }));
    assert!(listed.contains("[verification] admin"), "got: {listed}");
    assert_eq!(comment_kinds(&m), ["verification"]);
}

#[test]
fn closing_without_evidence_writes_no_comment() {
    let (m, _guard) = mcp();
    seed_project(&m, "Verify", "VER");
    seed_issue(&m, "VER", "Ship it");

    let result = close(&m, "VER-1", None);
    assert!(result.starts_with("Updated VER-1:"), "got: {result}");
    assert!(!result.contains("Verification"), "got: {result}");
    assert!(comment_kinds(&m).is_empty());
    assert!(!get(&m, "VER-1").contains("[verification]"));
}

#[test]
fn ordinary_comments_stay_unmarked() {
    let (m, _guard) = mcp();
    seed_project(&m, "Verify", "VER");
    seed_issue(&m, "VER", "Ship it");
    m.add_comment(Parameters(AddCommentInput {
        identifier: "VER-1".into(),
        content: "a plain note".into(),
    }));
    let detail = get(&m, "VER-1");
    assert!(detail.contains("a plain note"), "got: {detail}");
    assert!(!detail.contains("[verification]"), "got: {detail}");
}

#[test]
fn evidence_is_refused_without_a_close_and_changes_nothing() {
    let (m, _guard) = mcp();
    seed_project(&m, "Verify", "VER");
    seed_issue(&m, "VER", "Ship it");

    for status in [None, Some("active"), Some("cancelled")] {
        let result = m.update_issue(Parameters(UpdateIssueInput {
            identifier: "VER-1".into(),
            status: status.map(Into::into),
            title: Some("Renamed".into()),
            evidence: Some("tests pass".into()),
            ..Default::default()
        }));
        assert!(
            result.starts_with("Error:") && result.contains("status=done"),
            "status {status:?}: {result}"
        );
    }
    let empty = close(&m, "VER-1", Some("  \n"));
    assert!(empty.contains("evidence is empty"), "got: {empty}");

    let detail = get(&m, "VER-1");
    assert!(detail.contains("Ship it"), "no field may change: {detail}");
    assert!(detail.contains("Status: backlog"), "got: {detail}");
    assert!(comment_kinds(&m).is_empty());
}

#[test]
fn evidence_on_an_already_done_issue_is_refused_and_rolled_back() {
    let (m, _guard) = mcp();
    seed_project(&m, "Verify", "VER");
    seed_issue(&m, "VER", "Ship it");
    close(&m, "VER-1", None);

    let result = m.update_issue(Parameters(UpdateIssueInput {
        identifier: "VER-1".into(),
        status: Some("done".into()),
        title: Some("Renamed".into()),
        evidence: Some("late evidence".into()),
        ..Default::default()
    }));
    assert!(result.contains("already done"), "got: {result}");
    assert!(get(&m, "VER-1").contains("Ship it"));
    assert!(comment_kinds(&m).is_empty());
}

#[test]
fn verification_kind_reaches_the_changes_feed_and_rest_json() {
    let (m, _guard) = mcp();
    seed_project(&m, "Verify", "VER");
    seed_issue(&m, "VER", "Ship it");
    close(&m, "VER-1", Some("checked by hand"));

    let (project_id, issue_id) = m
        .read(|conn| {
            let issue = queries::resolve_identifier(conn, "VER-1")?;
            Ok((queries::get_issue(conn, issue)?.project_id, issue))
        })
        .unwrap();
    let changes = m
        .read(|conn| queries::changes::list_changes(conn, project_id, 0, 100))
        .unwrap();
    let comment = changes
        .changes
        .iter()
        .find_map(|change| match change {
            models::Change::Comment(comment) => Some(comment.clone()),
            _ => None,
        })
        .expect("the verification comment is in the feed");
    assert_eq!(comment.comment_kind, models::CommentKind::Verification);
    let wire = serde_json::to_value(&comment).unwrap();
    assert_eq!(wire["kind"], "comment");
    assert_eq!(wire["comment_kind"], "verification");

    let admin = m
        .read(|conn| queries::users::get_user_by_username(conn, "admin"))
        .unwrap();
    let app = crate::api::test_helpers::app_as_user((*m.db).clone(), &admin);
    let body = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let response = crate::api::test_helpers::json_get(
                &app,
                &format!("/api/issues/{issue_id}/comments"),
            )
            .await;
            assert_eq!(response.status(), axum::http::StatusCode::OK);
            crate::api::test_helpers::parse_json(response).await
        });
    assert_eq!(body[0]["kind"], "verification", "got: {body}");
}

#[test]
fn a_viewer_cannot_close_with_evidence() {
    let (m, _admin, lead, _maintainer, viewer, _non_member, _project_id, _guard) =
        super::tests::setup_membership_mcp();
    let as_user = |user: &models::AuthUser, f: &dyn Fn() -> String| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(crate::mcp::with_request_user(
                Some(user.clone()),
                || async { f() },
            ))
    };
    let created = as_user(&lead, &|| {
        m.create_issue(Parameters(CreateIssueInput {
            project: Some("MEM".into()),
            title: "Guarded".into(),
            ..Default::default()
        }))
    });
    assert!(created.starts_with("Created"), "got: {created}");

    let denied = as_user(&viewer, &|| close(&m, "MEM-1", Some("trust me")));
    assert!(denied.starts_with("Error: Forbidden:"), "got: {denied}");
    assert!(comment_kinds(&m).is_empty());

    let allowed = as_user(&lead, &|| close(&m, "MEM-1", Some("verified")));
    assert!(allowed.contains("Verification recorded"), "got: {allowed}");
    assert_eq!(comment_kinds(&m), ["verification"]);
}
