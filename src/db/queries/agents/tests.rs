use super::*;
use crate::db::models::{CreateIssue, CreateProject};
use crate::db::queries;

/// 2026-11-12 16:03:12 UTC.
fn now() -> NaiveDateTime {
    at("2026-11-12 16:03:12")
}

fn at(ts: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(ts, TS_FORMAT).unwrap()
}

fn ago(minutes: i64, seconds: i64) -> String {
    (now() - Duration::minutes(minutes) - Duration::seconds(seconds))
        .format(TS_FORMAT)
        .to_string()
}

fn input(window: Window) -> ActivityInput<'static> {
    ActivityInput {
        visible: None,
        caller_id: None,
        window,
        tz_minutes: 0,
        now: now(),
    }
}

fn db() -> crate::db::DbPool {
    crate::db::open_memory().unwrap()
}

fn project(conn: &Connection, ident: &str) -> i64 {
    queries::create_project(
        conn,
        &CreateProject {
            name: format!("{ident} project"),
            identifier: ident.into(),
            ..Default::default()
        },
    )
    .unwrap()
    .id
}

fn issue(conn: &Connection, pid: i64, title: &str) -> i64 {
    queries::create_issue(
        conn,
        &CreateIssue {
            project_id: pid,
            title: title.into(),
            ..Default::default()
        },
    )
    .unwrap()
    .id
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

/// One audit row with full control over actor and time.
#[allow(clippy::too_many_arguments)]
fn audit(
    conn: &Connection,
    actor: i64,
    ts: &str,
    entity: (&str, i64),
    project_id: Option<i64>,
    issue_id: Option<i64>,
    action: &str,
    field: Option<&str>,
    new_value: Option<&str>,
) {
    conn.execute(
        "INSERT INTO audit_log (ts, actor_user_id, transport, entity_type, entity_id,
                                entity_label, project_id, issue_id, action, field, new_value)
         VALUES (?1, ?2, 'mcp', ?3, ?4, 'SNAP-1', ?5, ?6, ?7, ?8, ?9)",
        params![
            ts, actor, entity.0, entity.1, project_id, issue_id, action, field, new_value
        ],
    )
    .unwrap();
}

fn read_row(
    conn: &Connection,
    actor: i64,
    ts: &str,
    project_id: Option<i64>,
    entity: Option<(&str, i64, &str)>,
) {
    conn.execute(
        "INSERT INTO agent_reads (ts, user_id, tool, project_id, entity_type, entity_id, entity_label)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            ts,
            actor,
            if entity.is_some() { "get_issue" } else { "list_issues" },
            project_id,
            entity.map(|e| e.0),
            entity.map(|e| e.1),
            entity.map(|e| e.2),
        ],
    )
    .unwrap();
}

fn counts(read: i64, edit: i64, create: i64, close: i64) -> Counts {
    Counts {
        read,
        edit,
        create,
        close,
    }
}

#[test]
fn rows_from_one_call_collapse_into_one_action_per_kind() {
    let pool = db();
    let conn = pool.write().unwrap();
    let owner = user(&conn, "owner", None);
    let bot = user(&conn, "bot", Some(owner));
    let pid = project(&conn, "AGT");
    let iid = issue(&conn, pid, "Work");
    let t1 = ago(2, 0);
    let t2 = ago(1, 0);
    let issue = ("issue", iid);

    // A create carrying a label attach in the same call: one create, one edit.
    audit(
        &conn,
        bot,
        &t1,
        issue,
        Some(pid),
        Some(iid),
        "create",
        None,
        Some("Work"),
    );
    audit(
        &conn,
        bot,
        &t1,
        issue,
        Some(pid),
        Some(iid),
        "attach",
        Some("label"),
        Some("bug"),
    );
    // An update touching three fields, one of them closing it.
    audit(
        &conn,
        bot,
        &t2,
        issue,
        Some(pid),
        Some(iid),
        "update",
        Some("title"),
        Some("Done work"),
    );
    audit(
        &conn,
        bot,
        &t2,
        issue,
        Some(pid),
        Some(iid),
        "update",
        Some("priority"),
        Some("high"),
    );
    audit(
        &conn,
        bot,
        &t2,
        issue,
        Some(pid),
        Some(iid),
        "update",
        Some("status"),
        Some("done"),
    );
    // A comment and the mention rows it wrote.
    audit(
        &conn,
        bot,
        &t2,
        ("comment", 9),
        Some(pid),
        Some(iid),
        "create",
        None,
        Some("hi"),
    );
    audit(
        &conn,
        bot,
        &t2,
        ("comment", 9),
        Some(pid),
        Some(iid),
        "mention",
        Some("user"),
        None,
    );
    // Two reads, one list and one of the issue.
    read_row(&conn, bot, &t2, Some(pid), None);
    read_row(&conn, bot, &t2, Some(pid), Some(("issue", iid, "AGT-1")));

    let out = activity(&conn, &input(Window::Hour)).unwrap();
    assert_eq!(out.totals, counts(2, 3, 1, 1));
    assert_eq!(out.agents.len(), 1);
    assert_eq!(out.agents[0].counts, out.totals);
    assert_eq!(out.agents[0].series.iter().sum::<i64>(), 7);

    // Recent: collapsed the same way, list reads left out, the comment
    // reported on its issue.
    let kinds: Vec<(&str, Option<&str>)> = out
        .recent
        .iter()
        .map(|r| (r.kind, r.detail.as_deref()))
        .collect();
    assert_eq!(
        kinds,
        [
            ("edit", Some("comment")),
            ("close", Some("status")),
            ("edit", Some("priority")),
            ("read", None),
            ("edit", Some("label")),
            ("create", None),
        ]
    );
    let comment = &out.recent[0];
    assert_eq!(comment.entity_type, "issue");
    assert_eq!(comment.identifier.as_deref(), Some("AGT-1"));
    assert_eq!(comment.title.as_deref(), Some("Work"));
    assert_eq!(comment.route.as_deref(), Some("/AGT/issues/AGT-1"));
    let read = &out.recent[3];
    assert_eq!(read.tool.as_deref(), Some("get_issue"));
    assert_eq!(read.route.as_deref(), Some("/AGT/issues/AGT-1"));
}

#[test]
fn a_gone_entity_falls_back_to_the_audit_snapshot() {
    let pool = db();
    let conn = pool.write().unwrap();
    let bot = user(&conn, "bot", Some(user(&conn, "owner", None)));
    let pid = project(&conn, "AGT");
    audit(
        &conn,
        bot,
        &ago(1, 0),
        ("issue", 777),
        Some(pid),
        Some(777),
        "delete",
        None,
        None,
    );
    conn.execute(
        "UPDATE audit_log SET old_value = 'Removed thing' WHERE entity_id = 777",
        [],
    )
    .unwrap();

    let out = activity(&conn, &input(Window::Hour)).unwrap();
    let row = &out.recent[0];
    assert_eq!(row.detail.as_deref(), Some("delete"));
    assert_eq!(row.identifier.as_deref(), Some("SNAP-1"));
    assert_eq!(row.title.as_deref(), Some("Removed thing"));
    assert_eq!(row.route, None);
}

#[test]
fn buckets_align_to_the_callers_local_clock() {
    let pool = db();
    let conn = pool.write().unwrap();
    let cases = [
        (Window::Hour, 0, 12, 300, "2026-11-12T15:05:00Z"),
        (Window::Day, 0, 24, 3600, "2026-11-11T17:00:00Z"),
        (Window::Week, 0, 7, 86_400, "2026-11-06T00:00:00Z"),
        // 21:33 local at +05:30: the hour began 15:30 UTC.
        (Window::Day, 330, 24, 3600, "2026-11-11T16:30:00Z"),
        // 11:03 local at -05:00: local midnight is 05:00 UTC.
        (Window::Week, -300, 7, 86_400, "2026-11-06T05:00:00Z"),
        // Clamped to +14:00: 06:03 local the next day.
        (Window::Week, 2000, 7, 86_400, "2026-11-06T10:00:00Z"),
    ];
    for (window, tz, len, size, first) in cases {
        let out = activity(
            &conn,
            &ActivityInput {
                tz_minutes: tz,
                ..input(window)
            },
        )
        .unwrap();
        assert_eq!(out.buckets.len(), len, "{window:?} {tz}");
        assert_eq!(out.bucket_seconds, size);
        assert_eq!(out.buckets[0].start, first, "{window:?} {tz}");
        let last = parse_ts(&out.buckets[len - 1].start).unwrap();
        assert!(last <= now() && now() < last + Duration::seconds(size));
    }
    assert_eq!(
        activity(&conn, &input(Window::Hour)).unwrap().now,
        "2026-11-12T16:03:12Z"
    );
}

#[test]
fn rows_land_in_their_bucket_and_the_window_bounds_every_count() {
    let pool = db();
    let conn = pool.write().unwrap();
    let bot = user(&conn, "bot", Some(user(&conn, "owner", None)));
    let pid = project(&conn, "AGT");
    read_row(&conn, bot, "2026-11-12 15:04:59", Some(pid), None); // before the window
    read_row(&conn, bot, "2026-11-12 15:05:00", Some(pid), None); // first bucket
    read_row(&conn, bot, "2026-11-12 15:59:59", Some(pid), None); // 15:55 bucket
    read_row(&conn, bot, "2026-11-12 16:03:12", Some(pid), None); // now, last bucket
    read_row(&conn, bot, "2026-11-12 16:03:13", Some(pid), None); // after now

    let out = activity(&conn, &input(Window::Hour)).unwrap();
    let reads: Vec<i64> = out.buckets.iter().map(|b| b.counts.read).collect();
    assert_eq!(reads, [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1]);
    assert_eq!(out.agents[0].series, reads);
    assert_eq!(out.totals.read, 3);
}

#[test]
fn status_follows_the_last_action_and_the_roster_sorts_by_it() {
    let pool = db();
    let conn = pool.write().unwrap();
    let caller = user(&conn, "caller", None);
    let other = user(&conn, "other", None);
    let pid = project(&conn, "AGT");
    let cases = [
        ("active-edge", (9, 59), "active"),
        ("idle-start", (10, 0), "idle"),
        ("idle-edge", (59, 59), "idle"),
        ("inactive", (60, 0), "inactive"),
        ("old", (60 * 24 * 3, 0), "inactive"),
    ];
    for (name, (minutes, seconds), _) in cases {
        let bot = user(&conn, name, Some(other));
        read_row(&conn, bot, &ago(minutes, seconds), Some(pid), None);
    }
    // Owned by the caller and silent: listed as inactive. Someone else's
    // silent bot is not listed at all.
    user(&conn, "zz-quiet-mine", Some(caller));
    user(&conn, "quiet-theirs", Some(other));

    let out = activity(
        &conn,
        &ActivityInput {
            caller_id: Some(caller),
            ..input(Window::Hour)
        },
    )
    .unwrap();
    let roster: Vec<(&str, &str)> = out
        .agents
        .iter()
        .map(|a| (a.username.as_str(), a.status))
        .collect();
    assert_eq!(
        roster,
        [
            ("active-edge", "active"),
            ("idle-start", "idle"),
            ("idle-edge", "idle"),
            ("inactive", "inactive"),
            ("old", "inactive"),
            ("zz-quiet-mine", "inactive"),
        ]
    );
    assert_eq!(out.active_count, 1);
    let quiet = out.agents.last().unwrap();
    assert_eq!(quiet.last_seen, None);
    assert_eq!(quiet.series, vec![0; 12]);
    assert_eq!(out.agents[0].owner.as_ref().unwrap().username, "other");
    // Older than the window: on the roster, counted nowhere.
    assert_eq!(out.agents[4].counts, Counts::default());
}

#[test]
fn hidden_projects_and_other_owners_workspace_rows_stay_hidden() {
    let pool = db();
    let conn = pool.write().unwrap();
    let caller = user(&conn, "caller", None);
    let other = user(&conn, "other", None);
    let mine = user(&conn, "mine", Some(caller));
    let theirs = user(&conn, "theirs", Some(other));
    let seen = project(&conn, "SEE");
    let hidden = project(&conn, "HID");
    let hidden_issue = issue(&conn, hidden, "Secret");
    let t = ago(1, 0);
    audit(
        &conn,
        theirs,
        &t,
        ("issue", hidden_issue),
        Some(hidden),
        Some(hidden_issue),
        "update",
        Some("title"),
        Some("Secret"),
    );
    read_row(
        &conn,
        theirs,
        &t,
        Some(hidden),
        Some(("issue", hidden_issue, "HID-1")),
    );
    read_row(&conn, theirs, &t, None, None); // someone else's workspace search
    read_row(&conn, mine, &t, None, None); // the caller's own bot's search
    read_row(&conn, mine, &t, Some(seen), None);

    let visible: HashSet<i64> = [seen].into();
    let out = activity(
        &conn,
        &ActivityInput {
            visible: Some(&visible),
            caller_id: Some(caller),
            ..input(Window::Hour)
        },
    )
    .unwrap();
    assert_eq!(out.totals, counts(2, 0, 0, 0));
    let roster: Vec<&str> = out.agents.iter().map(|a| a.username.as_str()).collect();
    assert_eq!(roster, ["mine"]);
    assert!(out.recent.is_empty());

    // An unrestricted caller sees all of it.
    let out = activity(&conn, &input(Window::Hour)).unwrap();
    assert_eq!(out.totals, counts(4, 1, 0, 0));
    assert_eq!(out.agents.len(), 2);
}

#[test]
fn people_are_not_agents() {
    let pool = db();
    let conn = pool.write().unwrap();
    let person = user(&conn, "person", None);
    let pid = project(&conn, "AGT");
    let iid = issue(&conn, pid, "Work");
    let t = ago(1, 0);
    audit(
        &conn,
        person,
        &t,
        ("issue", iid),
        Some(pid),
        Some(iid),
        "update",
        Some("title"),
        Some("x"),
    );
    read_row(&conn, person, &t, Some(pid), Some(("issue", iid, "AGT-1")));

    let out = activity(&conn, &input(Window::Hour)).unwrap();
    assert_eq!(out.totals, Counts::default());
    assert!(out.agents.is_empty());
    assert!(out.recent.is_empty());
}

#[test]
fn an_unknown_window_is_a_bad_request() {
    assert!(matches!(
        Window::parse(Some("30d")),
        Err(LificError::BadRequest(_))
    ));
    assert_eq!(Window::parse(None).unwrap(), Window::Hour);
}
