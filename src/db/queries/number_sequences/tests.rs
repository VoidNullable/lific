use super::*;
use crate::db::{self, models::*, queries};

fn project(conn: &Connection, identifier: &str) -> i64 {
    queries::create_project(
        conn,
        &CreateProject {
            name: identifier.into(),
            identifier: identifier.into(),
            ..Default::default()
        },
    )
    .unwrap()
    .id
}

fn issue(conn: &Connection, project_id: i64) -> Issue {
    queries::create_issue(
        conn,
        &CreateIssue {
            project_id,
            title: "Issue".into(),
            ..Default::default()
        },
    )
    .unwrap()
}

fn page(conn: &Connection, project_id: Option<i64>) -> Page {
    queries::create_page(
        conn,
        &CreatePage {
            project_id,
            title: "Page".into(),
            ..Default::default()
        },
    )
    .unwrap()
}

fn plan_input(project_id: i64) -> CreatePlan {
    CreatePlan {
        project_id,
        title: "Plan".into(),
        issue_id: None,
        steps: Vec::new(),
    }
}

fn purge(conn: &Connection) -> queries::trash::PurgeCounts {
    conn.execute_batch(
        "UPDATE issues SET deleted_at = '2000-01-01' WHERE deleted_at IS NOT NULL;
         UPDATE pages SET deleted_at = '2000-01-01' WHERE deleted_at IS NOT NULL;",
    )
    .unwrap();
    queries::trash::purge_tombstones(conn, 30).unwrap()
}

#[test]
fn issue_numbers_survive_permanent_deletion() {
    let pool = db::open_memory().unwrap();
    let conn = pool.write().unwrap();
    let pid = project(&conn, "ISS");
    for number in 1..=3 {
        let created = issue(&conn, pid);
        assert_eq!(created.sequence, number);
        if number == 3 {
            queries::delete_issue(&conn, created.id).unwrap();
        }
    }
    assert_eq!(purge(&conn).issues, 1);
    assert_eq!(issue(&conn, pid).sequence, 4);
}

fn pages_survive_purge(workspace: bool) {
    let pool = db::open_memory().unwrap();
    let conn = pool.write().unwrap();
    let pid = (!workspace).then(|| project(&conn, "PAG"));
    for number in 1..=3 {
        let created = page(&conn, pid);
        assert_eq!(created.sequence, Some(number));
        if number == 3 {
            queries::delete_page(&conn, created.id).unwrap();
        }
    }
    assert_eq!(purge(&conn).pages, 1);
    assert_eq!(page(&conn, pid).sequence, Some(4));
}

#[test]
fn project_page_numbers_survive_permanent_deletion() {
    pages_survive_purge(false);
}

#[test]
fn workspace_page_numbers_survive_permanent_deletion() {
    pages_survive_purge(true);
}

#[test]
fn plan_numbers_survive_permanent_deletion() {
    let pool = db::open_memory().unwrap();
    let conn = pool.write().unwrap();
    let pid = project(&conn, "PLN");
    for number in 1..=3 {
        let created = queries::plans::create_plan(&conn, &plan_input(pid)).unwrap();
        assert_eq!(created.sequence, number);
        if number == 3 {
            queries::plans::delete_plan(&conn, created.id).unwrap();
        }
    }
    assert_eq!(
        queries::plans::create_plan(&conn, &plan_input(pid))
            .unwrap()
            .sequence,
        4
    );
}

#[test]
fn moves_share_issue_counter_and_keep_purged_alias_numbers_reserved() {
    let pool = db::open_memory().unwrap();
    let conn = pool.write().unwrap();
    let src = project(&conn, "SRC");
    let dst = project(&conn, "DST");
    let mut latest = issue(&conn, dst);
    for _ in 2..=3 {
        latest = issue(&conn, dst);
    }
    queries::delete_issue(&conn, latest.id).unwrap();
    assert_eq!(purge(&conn).issues, 1);
    let original = issue(&conn, src);
    let moved =
        queries::issue_move::move_issue(&conn, original.id, dst, &Default::default()).unwrap();
    assert_eq!(moved.issue.sequence, 4);
    assert_eq!(issue(&conn, dst).sequence, 5);
    queries::delete_issue(&conn, original.id).unwrap();
    assert_eq!(purge(&conn).issues, 1);
    let aliases: i64 = conn
        .query_row("SELECT COUNT(*) FROM issue_aliases", [], |r| r.get(0))
        .unwrap();
    assert_eq!(aliases, 0);
    assert_eq!(issue(&conn, src).sequence, 2);
}

#[test]
fn scopes_are_independent_and_empty_scopes_do_not_restart() {
    let pool = db::open_memory().unwrap();
    let conn = pool.write().unwrap();
    let first = project(&conn, "ONE");
    let second = project(&conn, "TWO");
    let created = issue(&conn, first);
    queries::delete_issue(&conn, created.id).unwrap();
    purge(&conn);
    assert_eq!(issue(&conn, first).sequence, 2);
    assert_eq!(issue(&conn, second).sequence, 1);
    assert_eq!(page(&conn, Some(first)).sequence, Some(1));
    assert_eq!(page(&conn, Some(second)).sequence, Some(1));
    assert_eq!(page(&conn, None).sequence, Some(1));
    assert_eq!(
        queries::plans::create_plan(&conn, &plan_input(first))
            .unwrap()
            .sequence,
        1
    );
}

#[test]
fn failed_creates_rollback_number_reservations() {
    let pool = db::open_memory().unwrap();
    let conn = pool.write().unwrap();
    let pid = project(&conn, "ERR");
    conn.execute_batch(
        "CREATE TRIGGER reject_issue BEFORE INSERT ON issues BEGIN SELECT RAISE(ABORT, 'test failure'); END;
         CREATE TRIGGER reject_page BEFORE INSERT ON pages BEGIN SELECT RAISE(ABORT, 'test failure'); END;
         CREATE TRIGGER reject_plan BEFORE INSERT ON plans BEGIN SELECT RAISE(ABORT, 'test failure'); END;",
    ).unwrap();
    assert!(
        queries::create_issue(
            &conn,
            &CreateIssue {
                project_id: pid,
                title: "Fail".into(),
                ..Default::default()
            }
        )
        .is_err()
    );
    for project_id in [Some(pid), None] {
        assert!(
            queries::create_page(
                &conn,
                &CreatePage {
                    project_id,
                    title: "Fail".into(),
                    ..Default::default()
                }
            )
            .is_err()
        );
    }
    assert!(queries::plans::create_plan(&conn, &plan_input(pid)).is_err());
    conn.execute_batch(
        "DROP TRIGGER reject_issue; DROP TRIGGER reject_page; DROP TRIGGER reject_plan;",
    )
    .unwrap();
    assert_eq!(issue(&conn, pid).sequence, 1);
    assert_eq!(page(&conn, Some(pid)).sequence, Some(1));
    assert_eq!(page(&conn, None).sequence, Some(1));
    assert_eq!(
        queries::plans::create_plan(&conn, &plan_input(pid))
            .unwrap()
            .sequence,
        1
    );
}

#[test]
fn exhausted_sequences_fail_without_wrapping_or_changing_the_mark() {
    let pool = db::open_memory().unwrap();
    let conn = pool.write().unwrap();
    let pid = project(&conn, "MAX");
    conn.execute(
        "INSERT INTO project_number_sequences (project_id, issues, pages, plans) VALUES (?1, ?2, ?2, ?2)",
        params![pid, i64::MAX - 1],
    ).unwrap();
    conn.execute(
        "UPDATE workspace_page_sequence SET value = ?1",
        [i64::MAX - 1],
    )
    .unwrap();
    for (scope, kind) in [
        (Some(pid), Entity::Issue),
        (Some(pid), Entity::Page),
        (Some(pid), Entity::Plan),
        (None, Entity::Page),
    ] {
        assert_eq!(next(&conn, scope, kind).unwrap(), i64::MAX);
        assert!(
            next(&conn, scope, kind)
                .unwrap_err()
                .to_string()
                .contains("sequence exhausted")
        );
    }
    let marks: (i64, i64, i64) = conn
        .query_row(
            "SELECT issues, pages, plans FROM project_number_sequences WHERE project_id = ?1",
            [pid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(marks, (i64::MAX, i64::MAX, i64::MAX));
}

#[test]
fn unnumbered_legacy_pages_do_not_change_marks() {
    let pool = db::open_memory().unwrap();
    let conn = pool.write().unwrap();
    let pid = project(&conn, "OLD");
    conn.execute(
        "INSERT INTO pages(project_id, title) VALUES (?1, 'Unnumbered'), (NULL, 'Workspace')",
        [pid],
    )
    .unwrap();
    assert_eq!(page(&conn, Some(pid)).sequence, Some(1));
    assert_eq!(page(&conn, None).sequence, Some(1));
}

#[test]
fn workspace_issue_and_plan_allocations_return_errors_without_reserving_numbers() {
    let pool = db::open_memory().unwrap();
    let conn = pool.write().unwrap();
    for entity in [Entity::Issue, Entity::Plan] {
        assert!(matches!(
            next(&conn, None, entity),
            Err(LificError::Internal(message))
                if message == "only pages can allocate workspace identifier numbers"
        ));
    }
    assert_eq!(page(&conn, None).sequence, Some(1));
}
