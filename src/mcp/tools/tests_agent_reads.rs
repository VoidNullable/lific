//! LIF-541: successful MCP reads leave one `agent_reads` row each, flushed
//! from the pool's buffer; failed and anonymous reads leave none.

use super::tests::{mcp, seed_issue, seed_project, setup_membership_mcp};
use super::*;
use rmcp::handler::server::wrapper::Parameters;

#[derive(Debug, PartialEq, Eq)]
struct Row {
    user_id: i64,
    tool: String,
    project_id: Option<i64>,
    entity_type: Option<String>,
    entity_id: Option<i64>,
    entity_label: Option<String>,
}

fn run<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

fn as_user(user: &models::AuthUser, f: impl FnOnce() -> String) -> String {
    run(crate::mcp::with_request_user(
        Some(user.clone()),
        || async { f() },
    ))
}

/// Flush the buffer and read back every row, oldest first.
fn flushed_rows(m: &LificMcp) -> Vec<Row> {
    m.db.flush_agent_reads().unwrap();
    let conn = m.db.read().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT user_id, tool, project_id, entity_type, entity_id, entity_label
               FROM agent_reads ORDER BY id",
        )
        .unwrap();
    stmt.query_map([], |r| {
        Ok(Row {
            user_id: r.get(0)?,
            tool: r.get(1)?,
            project_id: r.get(2)?,
            entity_type: r.get(3)?,
            entity_id: r.get(4)?,
            entity_label: r.get(5)?,
        })
    })
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

fn admin(m: &LificMcp) -> models::AuthUser {
    m.read(|conn| {
        let user = queries::users::get_user_by_username(conn, "admin")?;
        Ok(models::AuthUser {
            id: user.id,
            username: user.username,
            display_name: user.display_name,
            is_admin: user.is_admin,
        })
    })
    .unwrap()
}

fn get_issue(m: &LificMcp, identifier: &str) -> String {
    m.get_issue(Parameters(GetIssueInput {
        identifier: identifier.into(),
        ..Default::default()
    }))
}

#[test]
fn a_single_issue_read_records_the_caller_project_and_issue() {
    let (m, _guard) = mcp();
    seed_project(&m, "Reads", "RDS");
    seed_issue(&m, "RDS", "Read me");
    let user = admin(&m);
    let project_id = super::tests::project_id_for(&m, "RDS");
    let issue_id = m
        .read(|conn| queries::resolve_identifier(conn, "RDS-1"))
        .unwrap();

    let output = as_user(&user, || get_issue(&m, "RDS-1"));
    assert!(output.contains("Read me"), "got: {output}");

    assert_eq!(
        flushed_rows(&m),
        [Row {
            user_id: user.id,
            tool: "get_issue".into(),
            project_id: Some(project_id),
            entity_type: Some("issue".into()),
            entity_id: Some(issue_id),
            entity_label: Some("RDS-1".into()),
        }]
    );
}

#[test]
fn a_page_read_records_the_page() {
    let (m, _guard) = mcp();
    seed_project(&m, "Reads", "RDS");
    let created = m.create_page(Parameters(CreatePageInput {
        project: Some("RDS".into()),
        title: "Notes".into(),
        content: Some("body".into()),
        ..Default::default()
    }));
    assert!(created.starts_with("Created "), "got: {created}");
    let user = admin(&m);

    as_user(&user, || {
        m.get_page(Parameters(GetPageInput {
            identifier: "RDS-DOC-1".into(),
            ..Default::default()
        }))
    });

    let rows = flushed_rows(&m);
    assert_eq!(rows.len(), 1, "got: {rows:?}");
    assert_eq!(rows[0].tool, "get_page");
    assert_eq!(rows[0].entity_type.as_deref(), Some("page"));
    assert_eq!(rows[0].entity_label.as_deref(), Some("RDS-DOC-1"));
}

#[test]
fn a_list_read_records_its_project_without_an_entity() {
    let (m, _guard) = mcp();
    seed_project(&m, "Reads", "RDS");
    seed_issue(&m, "RDS", "Listed");
    let user = admin(&m);
    let project_id = super::tests::project_id_for(&m, "RDS");

    let output = as_user(&user, || {
        m.list_issues(Parameters(ListIssuesInput {
            project: Some("RDS".into()),
            ..Default::default()
        }))
        .text
    });
    assert!(output.contains("Listed"), "got: {output}");

    assert_eq!(
        flushed_rows(&m),
        [Row {
            user_id: user.id,
            tool: "list_issues".into(),
            project_id: Some(project_id),
            entity_type: None,
            entity_id: None,
            entity_label: None,
        }]
    );
}

#[test]
fn a_workspace_search_records_no_project() {
    let (m, _guard) = mcp();
    seed_project(&m, "Reads", "RDS");
    seed_issue(&m, "RDS", "Findable");
    let user = admin(&m);

    as_user(&user, || {
        m.search(Parameters(SearchInput {
            query: "Findable".into(),
            ..Default::default()
        }))
        .text
    });

    let rows = flushed_rows(&m);
    assert_eq!(rows.len(), 1, "got: {rows:?}");
    assert_eq!(rows[0].tool, "search");
    assert_eq!(rows[0].project_id, None);
    assert_eq!(rows[0].entity_type, None);
}

#[test]
fn a_batch_records_one_row_per_issue_it_could_read() {
    let (m, _guard) = mcp();
    seed_project(&m, "Reads", "RDS");
    seed_issue(&m, "RDS", "One");
    seed_issue(&m, "RDS", "Two");
    let user = admin(&m);

    as_user(&user, || {
        m.get_issue(Parameters(GetIssueInput {
            identifiers: Some(vec!["RDS-2".into(), "RDS-99".into(), "RDS-1".into()]),
            ..Default::default()
        }))
    });

    let labels: Vec<Option<String>> = flushed_rows(&m)
        .into_iter()
        .map(|row| row.entity_label)
        .collect();
    assert_eq!(labels, [Some("RDS-2".into()), Some("RDS-1".into())]);
}

#[test]
fn a_failed_read_records_nothing() {
    let (m, _guard) = mcp();
    seed_project(&m, "Reads", "RDS");
    let user = admin(&m);

    let output = as_user(&user, || get_issue(&m, "RDS-42"));
    assert!(output.starts_with("Error:"), "got: {output}");

    assert!(flushed_rows(&m).is_empty());
}

#[test]
fn a_forbidden_read_records_nothing() {
    let (m, _admin, lead, _maintainer, _viewer, non_member, _project_id, _guard) =
        setup_membership_mcp();
    let created = as_user(&lead, || {
        m.create_issue(Parameters(CreateIssueInput {
            project: Some("MEM".into()),
            title: "Members only".into(),
            ..Default::default()
        }))
    });
    assert!(created.starts_with("Created"), "got: {created}");

    let output = as_user(&non_member, || get_issue(&m, "MEM-1"));
    assert!(output.starts_with("Error: Forbidden"), "got: {output}");

    assert!(flushed_rows(&m).is_empty());
}

#[test]
fn an_unauthenticated_read_records_nothing() {
    let (m, _guard) = mcp();
    seed_project(&m, "Reads", "RDS");
    seed_issue(&m, "RDS", "Anonymous");

    let output = get_issue(&m, "RDS-1");
    assert!(output.contains("Anonymous"), "got: {output}");

    assert_eq!(m.db.pending_agent_reads(), 0);
    assert!(flushed_rows(&m).is_empty());
}
