use super::*;
use crate::db::models::*;
use crate::db::queries;

struct Fixture {
    db: crate::db::DbPool,
    project: i64,
    issue: i64,
    alice: i64,
    bob: i64,
}

fn user_id(conn: &Connection, name: &str) -> i64 {
    conn.query_row(
        "SELECT id FROM users WHERE username = ?1",
        params![name],
        |row| row.get(0),
    )
    .unwrap()
}

/// Admin `blake`, members `alice` and `bob`, non-member `carol`, a bot
/// owned by blake, and deactivated `gone`. Authorization is enforced so
/// membership matters.
fn fixture() -> Fixture {
    let db = crate::db::open_memory().expect("test db");
    let (project, issue, alice, bob) = {
        let conn = db.write().unwrap();
        conn.execute_batch(
            "INSERT INTO users (username, email, password_hash, display_name, is_admin)
             VALUES ('blake', 'blake@t.local', 'x', 'Blake', 1),
                    ('alice', 'alice@t.local', 'x', 'Alice A', 0),
                    ('bob', 'bob@t.local', 'x', '', 0),
                    ('carol', 'carol@t.local', 'x', '', 0),
                    ('gone', 'gone@t.local', 'x', '', 0);
             INSERT INTO users (username, email, password_hash, is_bot, owner_id)
             VALUES ('blake-codex', 'bot@t.local', 'x', 1,
                     (SELECT id FROM users WHERE username = 'blake'));
             UPDATE users SET is_active = 0 WHERE username = 'gone';
             INSERT OR REPLACE INTO instance_settings (id, allow_signup, authz_enforced)
             VALUES (1, 0, 1);",
        )
        .unwrap();
        let project = queries::create_project(
            &conn,
            &CreateProject {
                name: "Assign".into(),
                identifier: "ASN".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id;
        let (alice, bob) = (user_id(&conn, "alice"), user_id(&conn, "bob"));
        for (user, role) in [(alice, Role::Maintainer), (bob, Role::Viewer)] {
            queries::members::upsert_member(&conn, project, user, role).unwrap();
        }
        let gone = user_id(&conn, "gone");
        queries::members::upsert_member(&conn, project, gone, Role::Viewer).unwrap();
        let issue = new_issue(&conn, project, "Needs a decision", None);
        (project, issue, alice, bob)
    };
    Fixture {
        db,
        project,
        issue,
        alice,
        bob,
    }
}

fn new_issue(conn: &Connection, project_id: i64, title: &str, who: Option<&[&str]>) -> i64 {
    queries::create_issue(
        conn,
        &CreateIssue {
            project_id,
            title: title.into(),
            status: Status::Todo,
            assignees: who.map(|names| names.iter().map(|n| n.to_string()).collect()),
            ..Default::default()
        },
    )
    .unwrap()
    .id
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| name.to_string()).collect()
}

fn set(f: &Fixture, who: &[&str]) -> Result<(), LificError> {
    let conn = f.db.write().unwrap();
    set_assignment(&conn, f.issue, f.project, &names(who), None)
}

fn read(f: &Fixture) -> (bool, Vec<String>) {
    let conn = f.db.read().unwrap();
    let issue = queries::get_issue(&conn, f.issue).unwrap();
    (
        issue.needs_human,
        issue.assignees.into_iter().map(|a| a.username).collect(),
    )
}

fn audit(f: &Fixture) -> Vec<(String, String)> {
    let conn = f.db.read().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT action, COALESCE(new_value, old_value) FROM audit_log
              WHERE issue_id = ?1 AND action IN ('assign', 'unassign') ORDER BY id",
        )
        .unwrap();
    stmt.query_map(params![f.issue], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn listed(f: &Fixture, filter: &str, caller: Option<i64>) -> Result<Vec<i64>, LificError> {
    let conn = f.db.read().unwrap();
    Ok(queries::list_issues(
        &conn,
        &ListIssuesQuery {
            project_id: Some(f.project),
            assignee: Some(filter.into()),
            caller_user_id: caller,
            ..Default::default()
        },
    )?
    .into_iter()
    .map(|issue| issue.id)
    .collect())
}

#[test]
fn a_new_issue_is_unassigned() {
    let f = fixture();
    assert_eq!(read(&f), (false, vec![]));
}

#[test]
fn each_state_transition_reads_back() {
    let f = fixture();
    set(&f, &["human"]).unwrap();
    assert_eq!(read(&f), (true, vec![]));

    set(&f, &["@Alice", "bob"]).unwrap();
    assert_eq!(read(&f), (true, names(&["alice", "bob"])));

    set(&f, &["bob"]).unwrap();
    assert_eq!(read(&f), (true, names(&["bob"])));

    set(&f, &["HUMAN"]).unwrap();
    assert_eq!(read(&f), (true, vec![]));

    set(&f, &[]).unwrap();
    assert_eq!(read(&f), (false, vec![]));
}

#[test]
fn audit_records_only_what_changed() {
    let f = fixture();
    set(&f, &["alice", "bob"]).unwrap();
    set(&f, &["alice", "bob"]).unwrap();
    set(&f, &["alice"]).unwrap();
    set(&f, &["human"]).unwrap();
    set(&f, &[]).unwrap();
    assert_eq!(
        audit(&f),
        [
            ("assign", "@alice"),
            ("assign", "@bob"),
            ("unassign", "@bob"),
            ("unassign", "@alice"),
            ("assign", "human"),
            ("unassign", "human"),
        ]
        .map(|(a, b)| (a.to_string(), b.to_string()))
    );
}

#[test]
fn an_unchanged_assignment_does_not_bump_seq() {
    let f = fixture();
    set(&f, &["alice"]).unwrap();
    let before = queries::issue_seq(&f.db.read().unwrap(), f.issue).unwrap();
    set(&f, &["alice"]).unwrap();
    let after = queries::issue_seq(&f.db.read().unwrap(), f.issue).unwrap();
    assert_eq!(before, after);
    set(&f, &["human"]).unwrap();
    assert!(queries::issue_seq(&f.db.read().unwrap(), f.issue).unwrap() > after);
}

#[test]
fn refuses_bots_strangers_mixtures_and_unknowns() {
    let f = fixture();
    let err = set(&f, &["blake-codex"]).unwrap_err().to_string();
    assert!(err.contains("only people can be assigned"), "{err}");
    let err = set(&f, &["carol"]).unwrap_err().to_string();
    assert!(err.contains("not a member of this project"), "{err}");
    let err = set(&f, &["human", "alice"]).unwrap_err().to_string();
    assert!(err.contains("not both"), "{err}");
    let err = set(&f, &["nobody"]).unwrap_err().to_string();
    assert!(err.contains("no active user named 'nobody'"), "{err}");
    let err = set(&f, &["gone"]).unwrap_err().to_string();
    assert!(err.contains("no active user named 'gone'"), "{err}");
    // An admin sees every project, so needs no membership.
    set(&f, &["blake"]).unwrap();
    assert_eq!(read(&f), (true, names(&["blake"])));
}

#[test]
fn a_refused_name_writes_nothing() {
    let f = fixture();
    set(&f, &["alice"]).unwrap();
    set(&f, &["bob", "carol"]).unwrap_err();
    assert_eq!(read(&f), (true, names(&["alice"])));
}

#[test]
fn without_enforcement_anyone_active_and_human_can_be_named() {
    let f = fixture();
    f.db.write()
        .unwrap()
        .execute("UPDATE instance_settings SET authz_enforced = 0", [])
        .unwrap();
    set(&f, &["carol"]).unwrap();
    assert_eq!(read(&f), (true, names(&["carol"])));
    assert!(set(&f, &["blake-codex"]).is_err());
}

#[test]
fn list_filter_values() {
    let f = fixture();
    let (human, named, free) = {
        let conn = f.db.write().unwrap();
        (
            new_issue(&conn, f.project, "any person", Some(&["human"])),
            new_issue(&conn, f.project, "alice's", Some(&["alice"])),
            new_issue(&conn, f.project, "agents'", None),
        )
    };
    let mut all_free = listed(&f, "none", None).unwrap();
    all_free.sort_unstable();
    assert_eq!(all_free, [f.issue, free]);
    assert_eq!(listed(&f, "human", None).unwrap(), [human, named]);
    assert_eq!(listed(&f, "@alice", None).unwrap(), [named]);
    assert_eq!(listed(&f, "me", Some(f.alice)).unwrap(), [named]);
    assert!(listed(&f, "me", Some(f.bob)).unwrap().is_empty());
    let err = listed(&f, "me", None).unwrap_err().to_string();
    assert!(err.contains("signed-in caller"), "{err}");
    let err = listed(&f, "ghost", None).unwrap_err().to_string();
    assert!(err.contains("no user named 'ghost'"), "{err}");
}

#[test]
fn list_pages_carry_the_assignment() {
    let f = fixture();
    set(&f, &["alice"]).unwrap();
    let conn = f.db.read().unwrap();
    let issues = queries::list_issues(
        &conn,
        &ListIssuesQuery {
            project_id: Some(f.project),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(issues[0].needs_human);
    assert_eq!(issues[0].assignees[0].username, "alice");
    assert_eq!(
        issues[0].assignees[0].display_name.as_deref(),
        Some("Alice A")
    );
}

#[test]
fn update_issue_replaces_the_assignment_in_its_savepoint() {
    let f = fixture();
    let conn = f.db.write().unwrap();
    let updated = queries::update_issue(
        &conn,
        f.issue,
        &UpdateIssue {
            assignees: Some(names(&["alice"])),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(updated.assignees[0].username, "alice");
    // A refused assignment rolls back the rest of the update too.
    let err = queries::update_issue(
        &conn,
        f.issue,
        &UpdateIssue {
            title: Some("renamed".into()),
            assignees: Some(names(&["carol"])),
            ..Default::default()
        },
    );
    assert!(err.is_err());
    assert_eq!(
        queries::get_issue(&conn, f.issue).unwrap().title,
        "Needs a decision"
    );
}

#[test]
fn assignment_survives_soft_delete_and_restore() {
    let f = fixture();
    set(&f, &["alice", "bob"]).unwrap();
    let conn = f.db.write().unwrap();
    queries::delete_issue(&conn, f.issue).unwrap();
    let restored = queries::restore_issue(&conn, f.issue).unwrap();
    assert_eq!(restored.assignees.len(), 2);
    let unassign: i64 = conn
        .query_row(
            "SELECT count(*) FROM audit_log WHERE action = 'unassign'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(unassign, 0, "tombstoning is not an unassignment");
}

#[test]
fn deleting_the_only_named_person_falls_back_to_human() {
    let f = fixture();
    set(&f, &["alice"]).unwrap();
    let other = {
        let conn = f.db.write().unwrap();
        new_issue(&conn, f.project, "shared", Some(&["alice", "bob"]))
    };
    {
        let conn = f.db.write().unwrap();
        conn.execute("DELETE FROM users WHERE id = ?1", params![f.alice])
            .unwrap();
    }
    assert_eq!(read(&f), (true, vec![]), "still needs a person");
    let conn = f.db.read().unwrap();
    let shared = queries::get_issue(&conn, other).unwrap();
    assert_eq!(
        shared
            .assignees
            .iter()
            .map(|a| a.username.as_str())
            .collect::<Vec<_>>(),
        ["bob"]
    );
}

#[test]
fn resolve_me_needs_a_caller() {
    let mut list = names(&["me", "@ME", "bob"]);
    resolve_me(&mut list, Some("alice")).unwrap();
    assert_eq!(list, names(&["alice", "alice", "bob"]));
    assert!(resolve_me(&mut names(&["me"]), None).is_err());
}

#[test]
fn describe_renders_each_state() {
    let person = |name: &str| IssueAssignee {
        user_id: 1,
        username: name.into(),
        display_name: None,
    };
    assert_eq!(describe(false, &[]), None);
    assert_eq!(describe(true, &[]).as_deref(), Some("human"));
    assert_eq!(
        describe(true, &[person("alice"), person("bob")]).as_deref(),
        Some("@alice, @bob")
    );
}
