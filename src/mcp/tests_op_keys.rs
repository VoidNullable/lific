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
            kind: Default::default(),
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
fn add_comment_key_reused_with_another_kind_is_refused() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");
    seed_issue(&m, "TST", "Talk");
    let comment = |kind| {
        m.add_comment(Parameters(AddCommentInput {
            kind,
            identifier: "TST-1".into(),
            content: "Use SQLite".into(),
            op_key: Some("c1".into()),
        }))
    };
    let first = comment(models::CommentWriteKind::Comment);
    assert!(first.starts_with("Comment #"), "got: {first}");

    let refused = comment(models::CommentWriteKind::Decision);

    assert!(refused.contains("different request"), "got: {refused}");
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
fn a_retried_conventions_page_replays_before_the_duplicate_check() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");
    let page = |conventions| {
        m.create_page(Parameters(CreatePageInput {
            project: Some("TST".into()),
            title: "Conventions".into(),
            conventions,
            op_key: Some("conv".into()),
            ..Default::default()
        }))
    };
    let first = page(Some(true));
    assert!(first.contains("conventions page"), "got: {first}");

    // The page now exists, so a fresh create would be refused as a second
    // conventions page; the retry gets the first result instead.
    let retried = page(Some(true));
    assert!(retried.contains(REPLAYED), "got: {retried}");
    assert!(
        retried.ends_with(&first),
        "first: {first}\nretried: {retried}"
    );

    // The flag is part of the request, so the key cannot be reused without it.
    let refused = page(None);
    assert!(refused.contains("different request"), "got: {refused}");
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

/// A session's bound project is not in the request, so it is part of the
/// fingerprint: a session bound elsewhere never receives another project's
/// result for the same key. It is refused and writes nothing.
#[test]
fn the_same_op_key_from_sessions_bound_to_other_projects_never_replays() {
    let (m, _guard) = mcp();
    seed_project(&m, "Alpha", "AAA");
    seed_project(&m, "Beta", "BBB");
    let bound = |project: &str| m.clone().with_bound_project(Some(project.into()));
    let (alpha, beta) = (bound("AAA"), bound("BBB"));
    let single = |m: &LificMcp| {
        m.create_issue(Parameters(CreateIssueInput {
            title: "Same".into(),
            op_key: Some("single".into()),
            ..Default::default()
        }))
    };
    let batch = |m: &LificMcp| {
        m.create_issue(Parameters(CreateIssueInput {
            issues: Some(vec![item("Batched")]),
            op_key: Some("batch".into()),
            ..Default::default()
        }))
    };
    let plan = |m: &LificMcp| {
        m.create_plan(Parameters(CreatePlanInput {
            title: "Plan".into(),
            op_key: Some("plan".into()),
            ..Default::default()
        }))
    };

    assert!(single(&alpha).contains("AAA-1"));
    assert!(batch(&alpha).contains("AAA-2"));
    assert!(plan(&alpha).starts_with("Created"));
    for (tool, answer) in [
        ("single", single(&beta)),
        ("batch", batch(&beta)),
        ("plan", plan(&beta)),
    ] {
        assert!(answer.contains("different request"), "{tool}: {answer}");
        assert!(!answer.contains("AAA"), "{tool} leaked: {answer}");
    }
    assert_eq!(count(&m, "issues"), 2);
    assert_eq!(count(&m, "plans"), 1);
    // The same session still replays its own result.
    assert!(single(&alpha).contains(REPLAYED));
}

/// A retry sent while the original is still writing waits for it and
/// replays its result, instead of failing on the key the original records.
/// The test holds the writer so both calls pass the early replay check
/// before either can write.
#[test]
fn two_racing_calls_with_one_op_key_both_end_with_the_same_result() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");
    let writer = m.db.write().unwrap();
    let mut calls = Vec::new();
    for _ in 0..2 {
        let m = m.clone();
        calls.push(std::thread::spawn(move || create(&m, "Raced", "race")));
    }
    std::thread::sleep(std::time::Duration::from_millis(300));
    drop(writer);
    let answers: Vec<String> = calls.into_iter().map(|c| c.join().unwrap()).collect();

    for answer in &answers {
        assert!(answer.contains("Created TST-1: Raced"), "got: {answer}");
    }
    assert_eq!(
        answers.iter().filter(|a| a.contains(REPLAYED)).count(),
        1,
        "{answers:?}"
    );
    assert_eq!(count(&m, "issues"), 1);
}

fn as_user(user: &models::AuthUser, f: impl FnOnce() -> String) -> String {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(crate::mcp::with_request_user(
            Some(user.clone()),
            || async { f() },
        ))
}

/// A replay hands back what the first call created, so it needs the same
/// permission a fresh create would. A member removed since gets the refusal.
#[test]
fn a_member_removed_since_the_first_call_gets_no_replay() {
    let (db, _admin, _lead, maintainer, _viewer, _outsider, project) =
        crate::api::test_helpers::setup_membership_test();
    let m = LificMcp::new(db.clone());
    let _guard = super::acquire_test_guard();
    let user = models::AuthUser {
        id: maintainer.id,
        username: maintainer.username,
        display_name: maintainer.display_name,
        is_admin: false,
    };
    let issue = || {
        m.create_issue(Parameters(CreateIssueInput {
            project: Some("MEM".into()),
            title: "Private".into(),
            op_key: Some("issue".into()),
            ..Default::default()
        }))
    };
    let comment = || {
        m.add_comment(Parameters(AddCommentInput {
            kind: Default::default(),
            identifier: "MEM-1".into(),
            content: "Secret notes".into(),
            op_key: Some("comment".into()),
        }))
    };
    let page = || {
        m.create_page(Parameters(CreatePageInput {
            project: Some("MEM".into()),
            title: "Hidden".into(),
            op_key: Some("page".into()),
            ..Default::default()
        }))
    };
    let plan = || {
        m.create_plan(Parameters(CreatePlanInput {
            project: Some("MEM".into()),
            title: "Hidden plan".into(),
            op_key: Some("plan".into()),
            ..Default::default()
        }))
    };
    let calls: [(&str, &dyn Fn() -> String); 4] = [
        ("issue", &issue),
        ("comment", &comment),
        ("page", &page),
        ("plan", &plan),
    ];
    for (tool, call) in calls {
        let first = as_user(&user, call);
        assert!(!first.starts_with("Error"), "{tool}: {first}");
    }
    queries::members::remove_member(&db.write().unwrap(), project, user.id).unwrap();

    for (tool, call) in calls {
        let retried = as_user(&user, call);
        assert!(retried.starts_with("Error"), "{tool}: {retried}");
        assert!(!retried.contains(REPLAYED), "{tool}: {retried}");
        assert!(!retried.contains("Secret notes"), "{tool}: {retried}");
    }
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
