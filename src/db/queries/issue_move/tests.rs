use super::*;
use crate::db::models::*;
use crate::db::queries;

struct Fixture {
    db: crate::db::DbPool,
    src: i64,
    dst: i64,
}

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

fn fixture() -> Fixture {
    let db = crate::db::open_memory().expect("test db");
    let (src, dst) = {
        let conn = db.write().unwrap();
        (project(&conn, "SRC"), project(&conn, "DST"))
    };
    Fixture { db, src, dst }
}

fn issue(conn: &Connection, project_id: i64, title: &str) -> Issue {
    queries::create_issue(
        conn,
        &CreateIssue {
            project_id,
            title: title.into(),
            ..Default::default()
        },
    )
    .unwrap()
}

fn module(conn: &Connection, project_id: i64, name: &str) -> i64 {
    queries::create_module(
        conn,
        &CreateModule {
            project_id,
            name: name.into(),
            description: String::new(),
            status: "active".into(),
            emoji: None,
        },
    )
    .unwrap()
    .id
}

fn label(conn: &Connection, project_id: i64, name: &str) {
    queries::create_label(
        conn,
        &CreateLabel {
            project_id,
            name: name.into(),
            color: "#000000".into(),
        },
    )
    .unwrap();
}

fn move_to(conn: &Connection, id: i64, target: i64) -> MovedIssue {
    move_issue(conn, id, target, &MoveIssue::default()).unwrap()
}

#[test]
fn a_moved_issue_gets_the_next_identifier_and_keeps_its_old_one_as_an_alias() {
    let f = fixture();
    let conn = f.db.write().unwrap();
    issue(&conn, f.dst, "already here");
    let moving = issue(&conn, f.src, "moving");

    let moved = move_to(&conn, moving.id, f.dst);

    assert_eq!(moved.issue.id, moving.id);
    assert_eq!(moved.issue.identifier, "DST-2");
    assert_eq!(moved.previous_identifier, "SRC-1");
    assert_eq!(
        queries::resolve_identifier(&conn, "SRC-1").unwrap(),
        moving.id
    );
    assert_eq!(
        queries::resolve_identifier(&conn, "DST-2").unwrap(),
        moving.id
    );
    assert_eq!(issue_aliases(&conn, moving.id).unwrap(), vec!["SRC-1"]);
}

#[test]
fn an_alias_survives_a_second_move_and_both_old_identifiers_resolve() {
    let f = fixture();
    let conn = f.db.write().unwrap();
    let third = project(&conn, "THD");
    let moving = issue(&conn, f.src, "moving twice");

    move_to(&conn, moving.id, f.dst);
    let moved = move_to(&conn, moving.id, third);

    assert_eq!(moved.issue.identifier, "THD-1");
    for old in ["SRC-1", "DST-1"] {
        assert_eq!(queries::resolve_identifier(&conn, old).unwrap(), moving.id);
    }
}

#[test]
fn a_sequence_an_alias_holds_is_never_handed_out_again() {
    let f = fixture();
    let conn = f.db.write().unwrap();
    let moving = issue(&conn, f.src, "leaves SRC-1 behind");
    move_to(&conn, moving.id, f.dst);

    let next = issue(&conn, f.src, "new in SRC");

    assert_eq!(next.identifier, "SRC-2");
    assert_eq!(
        queries::resolve_identifier(&conn, "SRC-1").unwrap(),
        moving.id
    );
}

#[test]
fn moving_back_gives_a_fresh_identifier_and_the_original_still_resolves() {
    let f = fixture();
    let conn = f.db.write().unwrap();
    let moving = issue(&conn, f.src, "round trip");
    move_to(&conn, moving.id, f.dst);

    let back = move_to(&conn, moving.id, f.src);

    assert_eq!(back.issue.identifier, "SRC-2");
    assert_eq!(
        queries::resolve_identifier(&conn, "SRC-1").unwrap(),
        moving.id
    );
    assert_eq!(
        queries::resolve_identifier(&conn, "DST-1").unwrap(),
        moving.id
    );
}

#[test]
fn comments_relations_waits_and_plan_steps_follow_the_issue() {
    let f = fixture();
    let conn = f.db.write().unwrap();
    conn.execute(
        "INSERT INTO users (username, email, password_hash) VALUES ('ann', 'a@t.local', 'x')",
        [],
    )
    .unwrap();
    let user: i64 = conn.last_insert_rowid();
    let moving = issue(&conn, f.src, "moving");
    let neighbour = issue(&conn, f.src, "stays");
    queries::link_issues(&conn, neighbour.id, moving.id, "blocks").unwrap();
    queries::comments::create_comment(
        &conn,
        queries::comments::CommentParent::Issue(moving.id),
        user,
        "a decision worth keeping",
    )
    .unwrap();
    let plan = queries::plans::create_plan(
        &conn,
        &CreatePlan {
            project_id: f.src,
            title: "Plan".into(),
            issue_id: None,
            steps: Vec::new(),
        },
    )
    .unwrap();
    let step =
        queries::plans::add_step(&conn, plan.id, None, "Do it", "", Some(moving.id)).unwrap();

    let moved = move_to(&conn, moving.id, f.dst);

    assert_eq!(moved.issue.blocked_by, vec!["SRC-2"]);
    let neighbour = queries::get_issue(&conn, neighbour.id).unwrap();
    assert_eq!(neighbour.blocks, vec!["DST-1"]);
    let comments: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM comments WHERE issue_id = ?1",
            [moving.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(comments, 1);
    assert_eq!(
        queries::plans::step_issue_id(&conn, step).unwrap(),
        Some(moving.id)
    );
}

#[test]
fn labels_follow_by_name_and_the_rest_are_reported() {
    let f = fixture();
    let conn = f.db.write().unwrap();
    for name in ["bug", "ux"] {
        label(&conn, f.src, name);
    }
    label(&conn, f.dst, "bug");
    let moving = queries::create_issue(
        &conn,
        &CreateIssue {
            project_id: f.src,
            title: "labelled".into(),
            labels: vec!["bug".into(), "ux".into()],
            ..Default::default()
        },
    )
    .unwrap();

    let moved = move_to(&conn, moving.id, f.dst);

    assert_eq!(moved.issue.labels, vec!["bug"]);
    assert_eq!(moved.dropped_labels, vec!["ux"]);
    let attached_to_dst: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM issue_labels il JOIN labels l ON l.id = il.label_id
             WHERE il.issue_id = ?1 AND l.project_id = ?2",
            params![moving.id, f.dst],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(attached_to_dst, 1);
}

#[test]
fn a_module_follows_by_name_or_is_cleared_and_reported() {
    let f = fixture();
    let conn = f.db.write().unwrap();
    let core_src = module(&conn, f.src, "Core");
    let web_src = module(&conn, f.src, "Web");
    let core_dst = module(&conn, f.dst, "Core");
    let in_core = queries::create_issue(
        &conn,
        &CreateIssue {
            project_id: f.src,
            title: "core".into(),
            module_id: Some(core_src),
            ..Default::default()
        },
    )
    .unwrap();
    let in_web = queries::create_issue(
        &conn,
        &CreateIssue {
            project_id: f.src,
            title: "web".into(),
            module_id: Some(web_src),
            ..Default::default()
        },
    )
    .unwrap();

    let core = move_to(&conn, in_core.id, f.dst);
    let web = move_to(&conn, in_web.id, f.dst);

    assert_eq!(core.issue.module_id, Some(core_dst));
    assert_eq!(core.dropped_module, None);
    assert_eq!(web.issue.module_id, None);
    assert_eq!(web.dropped_module.as_deref(), Some("Web"));
}

#[test]
fn an_explicit_module_must_belong_to_the_target() {
    let f = fixture();
    let conn = f.db.write().unwrap();
    let src_module = module(&conn, f.src, "Core");
    let moving = issue(&conn, f.src, "moving");

    let err = move_issue(
        &conn,
        moving.id,
        f.dst,
        &MoveIssue {
            module_id: Some(src_module),
        },
    )
    .unwrap_err();

    assert!(matches!(err, LificError::BadRequest(_)), "{err:?}");
    assert_eq!(
        queries::get_issue(&conn, moving.id).unwrap().identifier,
        "SRC-1"
    );
}

#[test]
fn moving_into_the_same_project_is_refused() {
    let f = fixture();
    let conn = f.db.write().unwrap();
    let moving = issue(&conn, f.src, "stays");

    let err = move_issue(&conn, moving.id, f.src, &MoveIssue::default()).unwrap_err();

    assert!(matches!(err, LificError::BadRequest(_)), "{err:?}");
}

#[test]
fn the_move_is_recorded_in_the_issue_history() {
    let f = fixture();
    let conn = f.db.write().unwrap();
    let moving = issue(&conn, f.src, "moving");

    move_to(&conn, moving.id, f.dst);

    let (old, new): (String, String) = conn
        .query_row(
            "SELECT old_value, new_value FROM audit_log
             WHERE issue_id = ?1 AND field = 'project'",
            [moving.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((old.as_str(), new.as_str()), ("SRC-1", "DST-1"));
}

#[test]
fn search_scoped_to_the_new_project_finds_the_issue_and_its_comments() {
    let f = fixture();
    let conn = f.db.write().unwrap();
    conn.execute(
        "INSERT INTO users (username, email, password_hash) VALUES ('ann', 'a@t.local', 'x')",
        [],
    )
    .unwrap();
    let user: i64 = conn.last_insert_rowid();
    let moving = issue(&conn, f.src, "zebracorn migration");
    queries::comments::create_comment(
        &conn,
        queries::comments::CommentParent::Issue(moving.id),
        user,
        "quokkamode decision",
    )
    .unwrap();

    move_to(&conn, moving.id, f.dst);

    let scoped = |query: &str, project_id: i64| {
        queries::search(
            &conn,
            &SearchQuery {
                query: query.into(),
                project_id: Some(project_id),
                ..Default::default()
            },
        )
        .unwrap()
        .len()
    };
    assert_eq!(scoped("zebracorn", f.dst), 1);
    assert_eq!(scoped("quokkamode", f.dst), 1);
    assert_eq!(scoped("zebracorn", f.src), 0);
    assert_eq!(scoped("quokkamode", f.src), 0);
}

#[test]
fn searching_an_old_identifier_finds_the_moved_issue_first() {
    let f = fixture();
    let conn = f.db.write().unwrap();
    let moving = issue(&conn, f.src, "moving");
    move_to(&conn, moving.id, f.dst);

    let hits = queries::search(
        &conn,
        &SearchQuery {
            query: "SRC-1".into(),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(hits[0].id, moving.id);
    assert_eq!(hits[0].identifier.as_deref(), Some("DST-1"));
    assert!(hits[0].snippet.contains("SRC-1"), "{}", hits[0].snippet);
}

#[test]
fn an_old_identifier_hit_respects_visibility() {
    let f = fixture();
    let conn = f.db.write().unwrap();
    let moving = issue(&conn, f.src, "moving");
    move_to(&conn, moving.id, f.dst);
    let only_src: std::collections::HashSet<i64> = [f.src].into();

    let page = queries::search_page(
        &conn,
        &SearchQuery {
            query: "SRC-1".into(),
            ..Default::default()
        },
        Some(&only_src),
    )
    .unwrap();

    assert!(page.items.iter().all(|hit| hit.id != moving.id));
}

#[test]
fn the_old_projects_changes_report_the_departure_as_a_tombstone() {
    let f = fixture();
    let conn = f.db.write().unwrap();
    let moving = issue(&conn, f.src, "moving");
    let cursor = queries::changes::index_cursor(&conn).unwrap();

    move_to(&conn, moving.id, f.dst);

    let src = queries::changes::list_changes(&conn, f.src, cursor, 100).unwrap();
    assert!(
        src.changes.iter().any(|change| matches!(
            change,
            Change::Tombstone(t) if t.id == moving.id && t.kind == ChangeKind::Issue
        )),
        "{:?}",
        src.changes
    );
    let dst = queries::changes::list_changes(&conn, f.dst, cursor, 100).unwrap();
    assert!(
        dst.changes.iter().any(|change| matches!(
            change,
            Change::Issue(i) if i.id == moving.id && i.identifier == "DST-1"
        )),
        "{:?}",
        dst.changes
    );
}

#[test]
fn assignees_who_cannot_see_the_target_are_unassigned_and_reported() {
    let f = fixture();
    let conn = f.db.write().unwrap();
    conn.execute_batch(
        "INSERT INTO users (username, email, password_hash) VALUES
             ('ann', 'a@t.local', 'x'), ('bo', 'b@t.local', 'x');
         INSERT OR REPLACE INTO instance_settings (id, allow_signup, authz_enforced)
         VALUES (1, 0, 1);",
    )
    .unwrap();
    let id_of = |name: &str| -> i64 {
        conn.query_row("SELECT id FROM users WHERE username = ?1", [name], |row| {
            row.get(0)
        })
        .unwrap()
    };
    let (ann, bo) = (id_of("ann"), id_of("bo"));
    for user in [ann, bo] {
        queries::members::upsert_member(&conn, f.src, user, Role::Maintainer).unwrap();
    }
    queries::members::upsert_member(&conn, f.dst, ann, Role::Viewer).unwrap();
    let moving = queries::create_issue(
        &conn,
        &CreateIssue {
            project_id: f.src,
            title: "for two".into(),
            assignees: Some(vec!["ann".into(), "bo".into()]),
            ..Default::default()
        },
    )
    .unwrap();

    let moved = move_to(&conn, moving.id, f.dst);

    let kept: Vec<_> = moved
        .issue
        .assignees
        .iter()
        .map(|a| a.username.as_str())
        .collect();
    assert_eq!(kept, vec!["ann"]);
    assert_eq!(moved.dropped_assignees, vec!["bo"]);
}
