use super::*;
use crate::db::models::{CreateIssue, CreateProject, Priority};
use crate::db::queries;

fn now() -> NaiveDateTime {
    chrono::Utc::now().naive_utc()
}

fn input(now: NaiveDateTime) -> OverviewInput<'static> {
    OverviewInput {
        visible: None,
        user_id: None,
        since: None,
        tz_minutes: 0,
        now,
    }
}

fn project(conn: &Connection, ident: &str) -> i64 {
    queries::create_project(
        conn,
        &CreateProject {
            name: ident.into(),
            identifier: ident.into(),
            ..Default::default()
        },
    )
    .unwrap()
    .id
}

fn issue(conn: &Connection, pid: i64, title: &str, priority: Priority, age_days: i64) -> i64 {
    let id = queries::create_issue(
        conn,
        &CreateIssue {
            project_id: pid,
            title: title.into(),
            priority,
            ..Default::default()
        },
    )
    .unwrap()
    .id;
    conn.execute(
        "UPDATE issues SET created_at = datetime('now', ?2) WHERE id = ?1",
        params![id, format!("-{age_days} days")],
    )
    .unwrap();
    id
}

fn user(conn: &Connection, name: &str, owner: Option<i64>) -> i64 {
    conn.execute(
        "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot, owner_id)
         VALUES (?1, ?1 || '@test.local', 'x', ?1, 0, ?2, ?3)",
        params![name, owner.is_some(), owner],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn act_as(conn: &Connection, user_id: i64) {
    conn.execute("UPDATE _actor_state SET user_id = ?1", params![user_id])
        .unwrap();
}

fn set_status(conn: &Connection, id: i64, status: &str) {
    conn.execute(
        "UPDATE issues SET status = ?2 WHERE id = ?1",
        params![id, status],
    )
    .unwrap();
}

#[test]
fn age_buckets_split_by_priority_and_who_can_pick_the_work_up() {
    let pool = crate::db::open_memory().unwrap();
    let conn = pool.write().unwrap();
    let pid = project(&conn, "HOM");

    let blocker = issue(&conn, pid, "Fresh urgent", Priority::Urgent, 1);
    let person = issue(&conn, pid, "Needs a person", Priority::High, 40);
    conn.execute(
        "INSERT INTO issue_assignees (issue_id, user_id) VALUES (?1, NULL)",
        params![person],
    )
    .unwrap();
    let blocked = issue(&conn, pid, "Blocked", Priority::Medium, 10);
    conn.execute(
        "INSERT INTO issue_relations (source_id, target_id, relation_type) VALUES (?1, ?2, 'blocks')",
        params![blocker, blocked],
    )
    .unwrap();
    let active = issue(&conn, pid, "In progress", Priority::None, 200);
    set_status(&conn, active, "active");
    let done = issue(&conn, pid, "Finished", Priority::Low, 3);
    set_status(&conn, done, "done");

    let out = overview(&conn, &input(now())).unwrap();
    assert_eq!(out.open_total, 4);
    let keys: Vec<_> = out.age_buckets.iter().map(|b| b.key).collect();
    assert_eq!(keys, ["week", "month", "quarter", "half", "older"]);

    let [week, month, quarter, half, older] = &out.age_buckets[..] else {
        panic!("five buckets");
    };
    assert_eq!((week.total, week.urgent, week.agent_ready), (1, 1, 1));
    assert_eq!((month.total, month.medium, month.agent_ready), (1, 1, 0));
    assert_eq!(
        (quarter.total, quarter.high, quarter.needs_human),
        (1, 1, 1)
    );
    assert_eq!(quarter.agent_ready, 0);
    assert_eq!(half.total, 0);
    assert_eq!((older.total, older.none, older.agent_ready), (1, 1, 0));

    assert_eq!(
        out.projects,
        [ProjectPulse {
            project_id: pid,
            open: 4,
            last_activity: out.projects[0].last_activity.clone(),
        }]
    );
    assert!(out.projects[0].last_activity.is_some());
}

#[test]
fn every_aggregate_skips_projects_the_caller_cannot_see() {
    let pool = crate::db::open_memory().unwrap();
    let conn = pool.write().unwrap();
    let mine = project(&conn, "MINE");
    let theirs = project(&conn, "HIDE");
    let me = user(&conn, "me", None);
    act_as(&conn, me);
    issue(&conn, mine, "Mine", Priority::High, 2);
    let hidden = issue(&conn, theirs, "Hidden", Priority::Urgent, 2);
    set_status(&conn, hidden, "done");
    issue(&conn, theirs, "Hidden open", Priority::Urgent, 2);

    let visible: HashSet<i64> = [mine].into();
    let out = overview(
        &conn,
        &OverviewInput {
            visible: Some(&visible),
            user_id: Some(me),
            since: Some("2000-01-01T00:00:00Z"),
            ..input(now())
        },
    )
    .unwrap();

    assert_eq!(out.open_total, 1);
    assert_eq!(out.age_buckets[0].urgent, 0);
    assert_eq!(out.open_trend.last().unwrap().count, 1);
    assert!(out.projects.iter().all(|p| p.project_id == mine));
    let since = out.since.unwrap();
    assert_eq!((since.people_opened, since.people_closed), (1, 0));
    assert_eq!(out.my_done.iter().map(|d| d.count).sum::<i64>(), 0);
}

#[test]
fn open_trend_counts_an_issue_from_creation_until_it_closed() {
    let pool = crate::db::open_memory().unwrap();
    let conn = pool.write().unwrap();
    let pid = project(&conn, "TRD");
    let id = issue(&conn, pid, "Lived ten days", Priority::None, 20);
    set_status(&conn, id, "done");
    conn.execute(
        "UPDATE audit_log SET ts = datetime('now', '-10 days')
          WHERE entity_type = 'issue' AND entity_id = ?1 AND field = 'status'",
        params![id],
    )
    .unwrap();

    let out = overview(&conn, &input(now())).unwrap();
    assert_eq!(out.open_trend.len() as i64, TREND_DAYS + 1);
    let at = |back: usize| out.open_trend[out.open_trend.len() - 1 - back].count;
    assert_eq!(at(25), 0, "before it was created");
    assert_eq!(at(15), 1, "open between creation and close");
    assert_eq!(at(5), 0, "closed ten days ago");
    assert_eq!(at(0), 0);
    assert_eq!(
        out.open_trend.last().unwrap().date,
        now().date().format("%Y-%m-%d").to_string()
    );
}

#[test]
fn since_summary_separates_agents_from_people() {
    let pool = crate::db::open_memory().unwrap();
    let conn = pool.write().unwrap();
    let pid = project(&conn, "SNC");
    let person = user(&conn, "blake", None);
    let bot = user(&conn, "blake-agent", Some(person));

    act_as(&conn, bot);
    let a = issue(&conn, pid, "Agent filed", Priority::None, 0);
    let b = issue(&conn, pid, "Agent filed too", Priority::None, 0);
    set_status(&conn, a, "done");
    set_status(&conn, b, "cancelled");
    act_as(&conn, person);
    let c = issue(&conn, pid, "Person filed", Priority::None, 0);
    set_status(&conn, c, "active");

    let earlier = (now() - Duration::hours(1))
        .format("%Y-%m-%dT%H:%M:%S.000Z")
        .to_string();
    let out = overview(
        &conn,
        &OverviewInput {
            since: Some(&earlier),
            ..input(now())
        },
    )
    .unwrap();
    assert_eq!(
        out.since.unwrap(),
        SinceSummary {
            agents_opened: 2,
            agents_closed: 2,
            people_opened: 1,
            people_closed: 0,
        }
    );

    let later = (now() + Duration::hours(1))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let out = overview(
        &conn,
        &OverviewInput {
            since: Some(&later),
            ..input(now())
        },
    )
    .unwrap();
    assert_eq!(out.since.unwrap(), SinceSummary::default());
}

#[test]
fn my_done_counts_the_caller_and_their_own_agents_only() {
    let pool = crate::db::open_memory().unwrap();
    let conn = pool.write().unwrap();
    let pid = project(&conn, "DNE");
    let me = user(&conn, "me", None);
    let my_bot = user(&conn, "my-bot", Some(me));
    let other = user(&conn, "other", None);
    let other_bot = user(&conn, "other-bot", Some(other));

    for (actor, status) in [
        (me, "done"),
        (my_bot, "done"),
        (my_bot, "cancelled"),
        (other, "done"),
        (other_bot, "done"),
    ] {
        act_as(&conn, actor);
        let id = issue(&conn, pid, "Work", Priority::None, 0);
        set_status(&conn, id, status);
    }

    let out = overview(
        &conn,
        &OverviewInput {
            user_id: Some(me),
            ..input(now())
        },
    )
    .unwrap();
    assert_eq!(out.my_done.len() as i64, HEATMAP_DAYS);
    assert_eq!(out.my_done.last().unwrap().count, 2);
    assert_eq!(out.my_done.iter().map(|d| d.count).sum::<i64>(), 2);
}

#[test]
fn project_freshness_reads_the_latest_timestamp_not_the_latest_row() {
    let pool = crate::db::open_memory().unwrap();
    let conn = pool.write().unwrap();
    let pid = project(&conn, "IMP");
    issue(&conn, pid, "Recent", Priority::None, 0);
    conn.execute(
        "UPDATE audit_log SET ts = '2026-09-01 10:00:00' WHERE project_id = ?1",
        params![pid],
    )
    .unwrap();
    // An imported row lands later in the table with an older timestamp.
    conn.execute(
        "INSERT INTO audit_log (ts, transport, entity_type, entity_id, project_id, action)
         VALUES ('2025-01-01 00:00:00', 'system', 'issue', 999, ?1, 'create')",
        params![pid],
    )
    .unwrap();

    let out = overview(&conn, &input(now())).unwrap();
    assert_eq!(
        out.projects[0].last_activity.as_deref(),
        Some("2026-09-01 10:00:00")
    );
}

#[test]
fn a_since_that_is_not_a_timestamp_is_rejected() {
    let pool = crate::db::open_memory().unwrap();
    let conn = pool.write().unwrap();
    let err = overview(
        &conn,
        &OverviewInput {
            since: Some("yesterday"),
            ..input(now())
        },
    )
    .unwrap_err();
    assert!(matches!(err, LificError::BadRequest(_)), "{err:?}");
}

#[test]
fn local_days_follow_the_callers_offset() {
    let pool = crate::db::open_memory().unwrap();
    let conn = pool.write().unwrap();
    let at = NaiveDate::from_ymd_opt(2026, 10, 9)
        .unwrap()
        .and_hms_opt(2, 0, 0)
        .unwrap();
    let out = overview(
        &conn,
        &OverviewInput {
            tz_minutes: -300,
            ..input(at)
        },
    )
    .unwrap();
    assert_eq!(out.open_trend.last().unwrap().date, "2026-10-08");
    assert_eq!(out.my_done.last().unwrap().date, "2026-10-08");
}
