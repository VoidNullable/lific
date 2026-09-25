//! LIF-484: user and date waits through `link_issues` / `unlink_issues`, and
//! how `get_issue`, `list_issues` and `get_board` show them.

use rmcp::handler::server::wrapper::Parameters;

use super::*;
use crate::db::queries::waits::pin_today;

fn mcp() -> (LificMcp, McpTestGuard) {
    let db = crate::db::open_memory().expect("test db");
    {
        let conn = db.write().unwrap();
        conn.execute_batch(
            "INSERT INTO users (username, email, password_hash, display_name, is_admin)
             VALUES ('admin', 'admin@test.local', 'x', 'Admin', 1),
                    ('blake', 'blake@test.local', 'x', 'Blake', 0);",
        )
        .unwrap();
    }
    let m = LificMcp::new(db);
    let guard = acquire_test_guard();
    let created = m.manage_resource(Parameters(ManageResourceInput {
        resource_type: "project".into(),
        action: "create".into(),
        name: Some("Waits".into()),
        identifier: Some("WT".into()),
        ..Default::default()
    }));
    assert!(created.starts_with("Created project"), "{created}");
    for title in ["Waiting", "Free"] {
        let created = m.create_issue(Parameters(CreateIssueInput {
            project: Some("WT".into()),
            title: title.into(),
            status: Some("todo".into()),
            ..Default::default()
        }));
        assert!(created.starts_with("Created"), "{created}");
    }
    (m, guard)
}

fn wait_on(m: &LificMcp, input: LinkIssuesInput) -> String {
    m.link_issues(Parameters(LinkIssuesInput {
        target: "WT-1".into(),
        relation_type: "blocks".into(),
        ..input
    }))
}

fn list(m: &LificMcp, workable: bool, blocked: bool) -> String {
    m.list_issues(Parameters(ListIssuesInput {
        project: Some("WT".into()),
        workable: workable.then_some(true),
        blocked: blocked.then_some(true),
        ..Default::default()
    }))
}

fn get(m: &LificMcp) -> String {
    m.get_issue(Parameters(GetIssueInput {
        identifier: "WT-1".into(),
        include_comments: Some("none".into()),
    }))
}

fn board(m: &LificMcp) -> String {
    m.get_board(Parameters(GetBoardInput {
        project: Some("WT".into()),
        ..Default::default()
    }))
}

#[test]
fn a_user_wait_is_added_shown_and_cleared_through_link_and_unlink() {
    let (m, _guard) = mcp();
    let added = wait_on(
        &m,
        LinkIssuesInput {
            user: Some("blake".into()),
            note: Some("decide the schema".into()),
            ..Default::default()
        },
    );
    assert_eq!(added, "WT-1: Waiting on @blake (decide the schema)");

    assert!(
        get(&m).contains("\nWaiting on @blake (decide the schema)\n"),
        "{}",
        get(&m)
    );
    let workable = list(&m, true, false);
    assert!(!workable.contains("WT-1 |"), "{workable}");
    assert!(workable.contains("WT-2 |"), "{workable}");
    let blocked = list(&m, false, true);
    assert!(
        blocked.contains("WT-1 | todo | none | Waiting waiting_on:@blake"),
        "{blocked}"
    );
    assert!(!blocked.contains("WT-2"), "{blocked}");
    assert!(board(&m).contains("WT-1 | todo | none | Waiting waiting_on:@blake"));

    let cleared = m.unlink_issues(Parameters(UnlinkIssuesInput {
        target: "WT-1".into(),
        user: Some("@blake".into()),
        ..Default::default()
    }));
    assert_eq!(cleared, "Cleared WT-1's wait on @blake");
    assert!(list(&m, true, false).contains("WT-1 |"));
    assert!(!get(&m).contains("Waiting on"));

    let activity = m.get_activity(Parameters(GetActivityInput {
        identifier: "WT-1".into(),
        ..Default::default()
    }));
    assert!(
        activity.contains("+wait @blake: decide the schema"),
        "{activity}"
    );
    assert!(
        activity.contains("-wait @blake: decide the schema"),
        "{activity}"
    );
}

#[test]
fn a_date_wait_holds_then_comes_due_then_goes_overdue() {
    let (m, _guard) = mcp();
    let _day = pin_today("2026-09-25");
    let added = wait_on(
        &m,
        LinkIssuesInput {
            from: Some("2026-09-28".into()),
            until: Some("2026-09-29".into()),
            note: Some("state filing office, 2 to 5 business days".into()),
            ..Default::default()
        },
    );
    assert_eq!(
        added,
        "WT-1: Waiting until 2026-09-28..29 (state filing office, 2 to 5 business days)"
    );
    assert!(!list(&m, true, false).contains("WT-1 |"));
    assert!(
        list(&m, false, true).contains("WT-1 | todo | none | Waiting waiting_until:2026-09-28..29")
    );

    let _day = pin_today("2026-09-28");
    assert!(list(&m, true, false).contains("WT-1 | todo | none | Waiting due_since:2026-09-28"));
    assert!(
        get(&m)
            .contains("Due to check since 2026-09-28, expected by 2026-09-29 (state filing office"),
        "{}",
        get(&m)
    );

    let _day = pin_today("2026-10-01");
    assert!(board(&m).contains("Waiting overdue_since:2026-09-30"));
    assert!(get(&m).contains("Overdue since 2026-09-30, expected 2026-09-28..29 ("));

    let cleared = m.unlink_issues(Parameters(UnlinkIssuesInput {
        target: "WT-1".into(),
        from: Some("2026-09-28".into()),
        ..Default::default()
    }));
    assert_eq!(cleared, "Cleared WT-1's wait on 2026-09-28..29");
}

#[test]
fn wait_arguments_are_checked_before_anything_is_written() {
    let (m, _guard) = mcp();
    let unknown = wait_on(
        &m,
        LinkIssuesInput {
            user: Some("nobody".into()),
            ..Default::default()
        },
    );
    assert_eq!(unknown, "Error: Bad request: no active user named 'nobody'");

    let wrong_type = m.link_issues(Parameters(LinkIssuesInput {
        target: "WT-1".into(),
        relation_type: "relates_to".into(),
        user: Some("blake".into()),
        ..Default::default()
    }));
    assert!(wrong_type.contains("make a blocks link"), "{wrong_type}");

    let both = wait_on(
        &m,
        LinkIssuesInput {
            source: "WT-2".into(),
            user: Some("blake".into()),
            ..Default::default()
        },
    );
    assert!(both.contains("not both"), "{both}");

    let backwards = wait_on(
        &m,
        LinkIssuesInput {
            from: Some("2026-09-28".into()),
            until: Some("2026-09-01".into()),
            ..Default::default()
        },
    );
    assert!(backwards.contains("is before from"), "{backwards}");

    let no_source = m.link_issues(Parameters(LinkIssuesInput {
        target: "WT-1".into(),
        relation_type: "blocks".into(),
        ..Default::default()
    }));
    assert!(no_source.contains("source is required"), "{no_source}");

    wait_on(
        &m,
        LinkIssuesInput {
            user: Some("blake".into()),
            ..Default::default()
        },
    );
    let duplicate = wait_on(
        &m,
        LinkIssuesInput {
            user: Some("blake".into()),
            ..Default::default()
        },
    );
    assert!(
        duplicate.contains("WT-1 already waits on @blake"),
        "{duplicate}"
    );

    let not_waiting = m.unlink_issues(Parameters(UnlinkIssuesInput {
        target: "WT-2".into(),
        user: Some("blake".into()),
        ..Default::default()
    }));
    assert!(
        not_waiting.contains("WT-2 is not waiting on @blake"),
        "{not_waiting}"
    );
}

#[test]
fn a_wait_publishes_the_issue_update_at_its_new_seq() {
    let db = crate::db::open_memory().expect("test db");
    {
        let conn = db.write().unwrap();
        conn.execute_batch(
            "INSERT INTO users (username, email, password_hash, is_admin)
             VALUES ('admin', 'admin@test.local', 'x', 1);",
        )
        .unwrap();
    }
    let realtime = crate::realtime::RealtimeHub::new();
    let mut rx = realtime.subscribe();
    let m = LificMcp::with_realtime(db, realtime);
    let _guard = acquire_test_guard();
    m.manage_resource(Parameters(ManageResourceInput {
        resource_type: "project".into(),
        action: "create".into(),
        name: Some("Waits".into()),
        identifier: Some("WT".into()),
        ..Default::default()
    }));
    m.create_issue(Parameters(CreateIssueInput {
        project: Some("WT".into()),
        title: "Waiting".into(),
        ..Default::default()
    }));
    while rx.try_recv().is_ok() {}
    wait_on(
        &m,
        LinkIssuesInput {
            from: Some("2030-01-01".into()),
            ..Default::default()
        },
    );
    let message = rx.try_recv().expect("an event");
    let seq =
        m.db.read()
            .map(|conn| crate::db::queries::issue_seq(&conn, 1).unwrap())
            .unwrap();
    assert!(matches!(
        message.event,
        crate::realtime::RealtimeEvent::IssueUpdated { issue_id: 1, .. }
    ));
    let axum::extract::ws::Message::Text(text) = message.message else {
        panic!("expected a text event");
    };
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["seq"], seq);
}

#[test]
fn adding_or_clearing_a_wait_requires_maintainer_on_the_waiting_issue() {
    let (m, _admin, lead, _maintainer, viewer, non_member, _project_id, _guard) = {
        let (db, admin, lead, maintainer, viewer, non_member, project_id) =
            crate::api::test_helpers::setup_membership_test();
        let au = |u: models::User| models::AuthUser {
            id: u.id,
            username: u.username,
            display_name: u.display_name,
            is_admin: u.is_admin,
        };
        (
            LificMcp::new(db),
            au(admin),
            au(lead),
            au(maintainer),
            au(viewer),
            au(non_member),
            project_id,
            acquire_test_guard(),
        )
    };
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
            title: "Gate".into(),
            ..Default::default()
        }))
    });
    assert!(created.starts_with("Created"), "{created}");
    let add = || {
        m.link_issues(Parameters(LinkIssuesInput {
            target: "MEM-1".into(),
            relation_type: "blocks".into(),
            user: Some("lead".into()),
            ..Default::default()
        }))
    };
    let clear = || {
        m.unlink_issues(Parameters(UnlinkIssuesInput {
            target: "MEM-1".into(),
            user: Some("lead".into()),
            ..Default::default()
        }))
    };
    for denied in [&viewer, &non_member] {
        let result = as_user(denied, &add);
        assert!(result.starts_with("Error: Forbidden:"), "{result}");
    }
    assert!(as_user(&lead, &add).starts_with("MEM-1: Waiting on @lead"));
    for denied in [&viewer, &non_member] {
        let result = as_user(denied, &clear);
        assert!(result.starts_with("Error: Forbidden:"), "{result}");
    }
    // A viewer still sees the wait.
    let read = as_user(&viewer, &|| {
        m.get_issue(Parameters(GetIssueInput {
            identifier: "MEM-1".into(),
            include_comments: Some("none".into()),
        }))
    });
    assert!(read.contains("Waiting on @lead"), "{read}");
    assert!(as_user(&lead, &clear).starts_with("Cleared MEM-1's wait"));
}
