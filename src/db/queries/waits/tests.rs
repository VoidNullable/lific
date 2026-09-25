use super::*;
use crate::db::models::*;
use crate::db::queries;

struct Fixture {
    db: crate::db::DbPool,
    project: i64,
    issue: i64,
    blake: i64,
}

fn fixture() -> Fixture {
    let db = crate::db::open_memory().expect("test db");
    let (project, issue, blake) = {
        let conn = db.write().unwrap();
        conn.execute_batch(
            "INSERT INTO users (username, email, password_hash, display_name, is_admin)
             VALUES ('blake', 'blake@test.local', 'x', 'Blake', 1),
                    ('gone', 'gone@test.local', 'x', '', 0);
             UPDATE users SET is_active = 0 WHERE username = 'gone';",
        )
        .unwrap();
        let blake: i64 = conn
            .query_row("SELECT id FROM users WHERE username = 'blake'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let project = queries::create_project(
            &conn,
            &CreateProject {
                name: "Waits".into(),
                identifier: "WAIT".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id;
        let issue = new_issue(&conn, project, "Waiting issue");
        (project, issue, blake)
    };
    Fixture {
        db,
        project,
        issue,
        blake,
    }
}

fn new_issue(conn: &Connection, project_id: i64, title: &str) -> i64 {
    queries::create_issue(
        conn,
        &CreateIssue {
            project_id,
            title: title.into(),
            status: Status::Todo,
            ..Default::default()
        },
    )
    .unwrap()
    .id
}

fn user_wait(name: &str, note: &str) -> CreateWait {
    CreateWait {
        user: Some(name.into()),
        note: Some(note.into()),
        ..Default::default()
    }
}

fn date_wait(from: &str, until: Option<&str>, note: &str) -> CreateWait {
    CreateWait {
        from: Some(from.into()),
        until: until.map(Into::into),
        note: Some(note.into()),
        ..Default::default()
    }
}

fn listed(f: &Fixture, workable: bool, blocked: bool) -> Vec<i64> {
    let conn = f.db.read().unwrap();
    queries::list_issues(
        &conn,
        &ListIssuesQuery {
            project_id: Some(f.project),
            workable: workable.then_some(true),
            blocked: blocked.then_some(true),
            ..Default::default()
        },
    )
    .unwrap()
    .into_iter()
    .map(|issue| issue.id)
    .collect()
}

#[test]
fn a_user_wait_blocks_until_cleared() {
    let f = fixture();
    let wait = {
        let conn = f.db.write().unwrap();
        add_wait(
            &conn,
            f.issue,
            &user_wait("@Blake", "decide the schema"),
            None,
        )
        .unwrap()
    };
    assert_eq!(wait.kind, WaitKind::User);
    assert_eq!(wait.user_id, Some(f.blake));
    assert_eq!(wait.username.as_deref(), Some("blake"));
    assert_eq!(wait.state, WaitState::Holding);
    assert_eq!(wait.note, "decide the schema");

    assert!(!listed(&f, true, false).contains(&f.issue), "not workable");
    assert_eq!(listed(&f, false, true), vec![f.issue], "blocked");
    {
        let conn = f.db.read().unwrap();
        let stats = queries::project_agent_stats(&conn).unwrap();
        assert_eq!(stats.get(&f.project).map_or(0, |s| s.workable), 0);
    }

    // Every issue read carries it.
    let conn = f.db.read().unwrap();
    assert_eq!(
        queries::get_issue(&conn, f.issue).unwrap().waits,
        vec![wait.clone()]
    );
    let page = queries::list_issues(
        &conn,
        &ListIssuesQuery {
            project_id: Some(f.project),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(page[0].waits.len(), 1);
    drop(conn);

    let conn = f.db.write().unwrap();
    let cleared = clear_user_wait(&conn, f.issue, "blake").unwrap();
    assert_eq!(cleared.id, wait.id);
    drop(conn);
    assert!(listed(&f, true, false).contains(&f.issue), "workable again");
    assert!(listed(&f, false, true).is_empty());
}

#[test]
fn a_date_wait_lapses_on_its_earliest_day_and_is_overdue_after_its_latest() {
    let f = fixture();
    {
        let conn = f.db.write().unwrap();
        add_wait(
            &conn,
            f.issue,
            &date_wait("2026-09-28", Some("2026-09-29"), "state filing office"),
            None,
        )
        .unwrap();
    }
    let state = |f: &Fixture| {
        let conn = f.db.read().unwrap();
        queries::get_issue(&conn, f.issue).unwrap().waits[0].state
    };

    let _day = pin_today("2026-09-27");
    assert_eq!(state(&f), WaitState::Holding);
    assert!(!listed(&f, true, false).contains(&f.issue));
    assert_eq!(listed(&f, false, true), vec![f.issue]);

    let _day = pin_today("2026-09-28");
    assert_eq!(state(&f), WaitState::Due, "lapses on its earliest day");
    assert!(listed(&f, true, false).contains(&f.issue));
    assert!(listed(&f, false, true).is_empty());

    let _day = pin_today("2026-09-29");
    assert_eq!(state(&f), WaitState::Due, "still due on its latest day");

    let _day = pin_today("2026-09-30");
    assert_eq!(state(&f), WaitState::Overdue);
    assert!(listed(&f, true, false).contains(&f.issue));
}

#[test]
fn a_single_date_is_a_one_day_window() {
    let f = fixture();
    let conn = f.db.write().unwrap();
    let wait = add_wait(&conn, f.issue, &date_wait("2026-10-01", None, ""), None).unwrap();
    assert_eq!(wait.earliest.as_deref(), Some("2026-10-01"));
    assert_eq!(wait.latest.as_deref(), Some("2026-10-01"));
    assert_eq!(
        state_on(
            WaitKind::Date,
            Some("2026-10-01"),
            Some("2026-10-01"),
            "2026-10-01"
        ),
        WaitState::Due
    );
    assert_eq!(
        state_on(
            WaitKind::Date,
            Some("2026-10-01"),
            Some("2026-10-01"),
            "2026-10-02"
        ),
        WaitState::Overdue
    );
}

#[test]
fn waits_are_validated() {
    let f = fixture();
    let conn = f.db.write().unwrap();
    let error = |input: CreateWait| {
        add_wait(&conn, f.issue, &input, None)
            .unwrap_err()
            .to_string()
    };

    assert!(error(user_wait("nobody", "")).contains("no active user named 'nobody'"));
    assert!(error(user_wait("gone", "")).contains("no active user named 'gone'"));
    assert!(
        error(date_wait("2026-09-28", Some("2026-09-27"), ""))
            .contains("until (2026-09-27) is before from (2026-09-28)")
    );
    assert!(error(date_wait("next week", None, "")).contains("from must be a date"));
    assert!(error(date_wait("2026-9-28", None, "")).contains("from must be a date"));
    assert!(
        error(CreateWait {
            user: Some("blake".into()),
            from: Some("2026-09-28".into()),
            ..Default::default()
        })
        .contains("not both")
    );
    assert!(error(CreateWait::default()).contains("pass user"));
    assert!(error(user_wait("blake", &"x".repeat(MAX_NOTE_CHARS + 1))).contains("note"));

    add_wait(&conn, f.issue, &user_wait("blake", "first"), None).unwrap();
    let duplicate = add_wait(&conn, f.issue, &user_wait("BLAKE", "second"), None).unwrap_err();
    assert!(matches!(duplicate, LificError::Conflict(_)));
    assert!(
        duplicate
            .to_string()
            .contains("WAIT-1 already waits on @blake")
    );

    add_wait(
        &conn,
        f.issue,
        &date_wait("2026-09-28", Some("2026-09-29"), ""),
        None,
    )
    .unwrap();
    let duplicate = add_wait(
        &conn,
        f.issue,
        &date_wait("2026-09-28", Some("2026-09-29"), "other note"),
        None,
    )
    .unwrap_err();
    assert!(
        duplicate
            .to_string()
            .contains("already waits until 2026-09-28..29")
    );
    // A different window on the same issue is a different blocker.
    add_wait(&conn, f.issue, &date_wait("2026-09-28", None, ""), None).unwrap();
}

#[test]
fn clearing_date_waits_by_start_day_and_by_id() {
    let f = fixture();
    let conn = f.db.write().unwrap();
    let one = add_wait(&conn, f.issue, &date_wait("2026-09-28", None, ""), None).unwrap();
    add_wait(
        &conn,
        f.issue,
        &date_wait("2026-09-28", Some("2026-10-02"), ""),
        None,
    )
    .unwrap();
    let later = add_wait(&conn, f.issue, &date_wait("2026-11-01", None, ""), None).unwrap();

    assert_eq!(
        clear_date_waits(&conn, f.issue, "2026-09-28")
            .unwrap()
            .len(),
        2
    );
    assert!(
        clear_date_waits(&conn, f.issue, "2026-09-28")
            .unwrap_err()
            .to_string()
            .contains("no date wait starting 2026-09-28")
    );
    assert!(clear_wait(&conn, f.issue, one.id).is_err(), "already gone");

    // A wait id from another issue does not clear through this one.
    let other = new_issue(&conn, f.project, "Other");
    assert!(matches!(
        clear_wait(&conn, other, later.id),
        Err(LificError::NotFound(_))
    ));
    clear_wait(&conn, f.issue, later.id).unwrap();
    assert!(list_waits(&conn, f.issue).unwrap().is_empty());
    assert!(
        clear_user_wait(&conn, f.issue, "blake")
            .unwrap_err()
            .to_string()
            .contains("WAIT-1 is not waiting on @blake")
    );
}

#[test]
fn adding_and_clearing_a_wait_advances_the_issue_in_the_sync_stream() {
    let f = fixture();
    let seq = |f: &Fixture| {
        let conn = f.db.read().unwrap();
        queries::issue_seq(&conn, f.issue).unwrap()
    };
    let before = seq(&f);
    let wait = {
        let conn = f.db.write().unwrap();
        add_wait(&conn, f.issue, &user_wait("blake", ""), None).unwrap()
    };
    let added = seq(&f);
    assert!(added > before);

    let conn = f.db.read().unwrap();
    let page = queries::changes::list_changes(&conn, f.project, before, 100).unwrap();
    let row = page
        .changes
        .iter()
        .find_map(|change| match change {
            Change::Issue(issue) if issue.id == f.issue => Some(issue.clone()),
            _ => None,
        })
        .expect("the issue is re-delivered");
    assert_eq!(row.waits, vec![wait.clone()]);
    let (index, _) = queries::changes::index_rows(&conn, f.project).unwrap();
    assert_eq!(index[0].waits, vec![wait.clone()]);
    drop(conn);

    {
        let conn = f.db.write().unwrap();
        clear_wait(&conn, f.issue, wait.id).unwrap();
    }
    assert!(seq(&f) > added);
    let conn = f.db.read().unwrap();
    let page = queries::changes::list_changes(&conn, f.project, added, 100).unwrap();
    assert!(page.changes.iter().any(|change| matches!(
        change,
        Change::Issue(issue) if issue.id == f.issue && issue.waits.is_empty()
    )));
}

#[test]
fn waits_are_audited_as_added_and_cleared() {
    let f = fixture();
    let conn = f.db.write().unwrap();
    let wait = add_wait(&conn, f.issue, &user_wait("blake", "decide"), None).unwrap();
    add_wait(
        &conn,
        f.issue,
        &date_wait("2026-09-28", Some("2026-09-29"), "office"),
        None,
    )
    .unwrap();
    clear_wait(&conn, f.issue, wait.id).unwrap();
    let rows: Vec<(String, String, Option<String>, Option<String>)> = conn
        .prepare(
            "SELECT action, field, new_value, old_value FROM audit_log
              WHERE issue_id = ?1 AND action IN ('wait', 'unwait') ORDER BY id",
        )
        .unwrap()
        .query_map([f.issue], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            (
                "wait".into(),
                "user".into(),
                Some("@blake: decide".into()),
                None
            ),
            (
                "wait".into(),
                "date".into(),
                Some("2026-09-28..2026-09-29: office".into()),
                None
            ),
            (
                "unwait".into(),
                "user".into(),
                None,
                Some("@blake: decide".into())
            ),
        ]
    );
}

#[test]
fn waits_follow_their_issue_through_trash_and_purge() {
    let f = fixture();
    let conn = f.db.write().unwrap();
    add_wait(&conn, f.issue, &user_wait("blake", ""), None).unwrap();
    queries::delete_issue(&conn, f.issue).unwrap();
    drop(conn);
    assert!(
        listed(&f, false, true).is_empty(),
        "a tombstone is not listed"
    );

    let conn = f.db.write().unwrap();
    let restored = queries::restore_issue(&conn, f.issue).unwrap();
    assert_eq!(restored.waits.len(), 1, "a restore brings the wait back");

    // Purge: the cascade removes the row without auditing a clear.
    queries::delete_issue(&conn, f.issue).unwrap();
    conn.execute(
        "UPDATE issues SET deleted_at = '2000-01-01 00:00:00.000' WHERE id = ?1",
        [f.issue],
    )
    .unwrap();
    queries::trash::purge_tombstones(&conn, 1).unwrap();
    let (waits, unwaits): (i64, i64) = conn
        .query_row(
            "SELECT (SELECT count(*) FROM issue_waits),
                    (SELECT count(*) FROM audit_log WHERE action = 'unwait')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((waits, unwaits), (0, 0));
}

#[test]
fn the_public_view_carries_no_waits() {
    let f = fixture();
    {
        let conn = f.db.write().unwrap();
        conn.execute(
            "UPDATE projects SET is_public = 1 WHERE id = ?1",
            [f.project],
        )
        .unwrap();
        add_wait(&conn, f.issue, &user_wait("blake", "private note"), None).unwrap();
    }
    let conn = f.db.read().unwrap();
    let tx = conn.unchecked_transaction().unwrap();
    let project = queries::public::public_project(&tx, "WAIT")
        .unwrap()
        .unwrap();
    let issue = queries::public::public_issue(&tx, &project, f.issue)
        .unwrap()
        .unwrap();
    assert!(issue.waits.is_empty());
    let index = queries::public::public_index(&tx, &project).unwrap();
    assert!(index.issues.iter().all(|row| row.waits.is_empty()));
    let changes = queries::public::public_changes(&tx, &project, 0, 100).unwrap();
    let json = serde_json::to_string(&(issue, index, changes)).unwrap();
    assert!(!json.contains("blake"), "{json}");
    assert!(!json.contains("private note"), "{json}");
}

#[test]
fn windows_render_in_their_shortest_unambiguous_form() {
    assert_eq!(format_window("2026-09-28", "2026-09-28"), "2026-09-28");
    assert_eq!(format_window("2026-09-28", "2026-09-29"), "2026-09-28..29");
    assert_eq!(
        format_window("2026-09-28", "2026-10-02"),
        "2026-09-28..10-02"
    );
    assert_eq!(
        format_window("2026-12-30", "2027-01-02"),
        "2026-12-30..2027-01-02"
    );
    assert_eq!(day_after("2026-09-30"), "2026-10-01");
    assert_eq!(day_after("2026-12-31"), "2027-01-01");
}

#[test]
fn today_is_the_local_day_unless_pinned() {
    let local = chrono::Local::now().date_naive();
    // A test straddling midnight could see the day turn; either side is fine.
    let unpinned = today();
    assert!(unpinned == local || unpinned == local.succ_opt().unwrap());
    {
        let _day = pin_today("2030-01-02");
        assert_eq!(today_text(), "2030-01-02");
    }
    assert_ne!(today_text(), "2030-01-02", "the pin is scoped to its guard");
}
