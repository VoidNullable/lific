//! LIF-147: assignments travel with a project archive, by username.

use super::*;
use crate::db::queries;

fn source() -> (DbPool, tempfile::TempDir, AttachmentStore) {
    let pool = db::open_memory().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let store = AttachmentStore::new(dir.path().to_path_buf());
    {
        let conn = pool.write().unwrap();
        conn.execute_batch(
            "INSERT INTO users(id,username,email,password_hash,is_admin)
               VALUES (1,'owner','owner@example.test','x',1),
                      (2,'blake','blake@example.test','x',0),
                      (3,'outsider','outsider@example.test','x',0);
             INSERT INTO projects(id,name,identifier) VALUES (10,'Portable','LIF');
             INSERT INTO issues(id,project_id,sequence,title)
               VALUES (30,10,1,'Named'),(31,10,2,'Anyone'),(32,10,3,'Stranger'),(33,10,4,'Free');",
        )
        .unwrap();
        for (issue, who) in [
            (30, &["blake", "outsider"][..]),
            (31, &["human"][..]),
            (32, &["outsider"][..]),
        ] {
            let names: Vec<String> = who.iter().map(|n| n.to_string()).collect();
            queries::assignees::set_assignment(&conn, issue, 10, &names, Some(1)).unwrap();
        }
    }
    (pool, dir, store)
}

/// A destination that knows `owner` and `blake` but not `outsider`.
fn destination() -> (DbPool, tempfile::TempDir, AttachmentStore) {
    let pool = db::open_memory().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let store = AttachmentStore::new(dir.path().to_path_buf());
    pool.write()
        .unwrap()
        .execute_batch(
            "INSERT INTO users(id,username,email,password_hash,is_admin)
               VALUES (5,'owner','owner@example.test','x',1),
                      (6,'Blake','blake@example.test','x',0);",
        )
        .unwrap();
    (pool, dir, store)
}

fn assignment(conn: &Connection, identifier: &str) -> (bool, Vec<String>) {
    let issue =
        queries::get_issue(conn, queries::resolve_identifier(conn, identifier).unwrap()).unwrap();
    (
        issue.needs_human,
        issue.assignees.into_iter().map(|a| a.username).collect(),
    )
}

#[test]
fn assignments_round_trip_and_unknown_people_fall_back_to_human() {
    let (source, dir, store) = source();
    let archive = dir.path().join("assign.tar.gz");
    export(&source, &store, "LIF", &archive).unwrap();
    let manifest = stage(&archive).unwrap().manifest;
    assert_eq!(manifest.rows("issue_assignees").len(), 4);

    let (dest, _dest_dir, dest_store) = destination();
    let report = import(&dest, &dest_store, &archive, "owner").unwrap();
    assert!(
        report
            .external_references
            .iter()
            .any(|r| r.contains("no active person named outsider")),
        "{:?}",
        report.external_references
    );

    let conn = dest.read().unwrap();
    // Blake binds; the unknown co-assignee is dropped, not turned into a mark.
    assert_eq!(assignment(&conn, "LIF-1"), (true, vec!["Blake".into()]));
    assert_eq!(assignment(&conn, "LIF-2"), (true, vec![]));
    // The only named person is unknown here: the issue still needs a person.
    assert_eq!(assignment(&conn, "LIF-3"), (true, vec![]));
    assert_eq!(assignment(&conn, "LIF-4"), (false, vec![]));
    drop(conn);

    let again = dir.path().join("again.tar.gz");
    export(&dest, &dest_store, "LIF", &again).unwrap();
    assert_eq!(
        stage(&again)
            .unwrap()
            .manifest
            .rows("issue_assignees")
            .len(),
        3
    );
}

#[test]
fn an_archive_from_before_assignees_still_imports() {
    let (source, dir, _store) = source();
    let conn = source.read().unwrap();
    let mut manifest = collect_manifest(&conn, "LIF").unwrap();
    drop(conn);
    manifest
        .tables
        .retain(|table| table.name != "issue_assignees");
    let path = dir.path().join("old.tar.gz");
    super::tests::write_manifest(&path, &manifest, &[]);
    let (dest, _dest_dir, dest_store) = destination();
    import(&dest, &dest_store, &path, "owner").unwrap();
    let count: i64 = dest
        .read()
        .unwrap()
        .query_row("SELECT count(*) FROM issue_assignees", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn contradictory_assignment_rows_are_rejected() {
    let (source, _dir, _store) = source();
    let conn = source.read().unwrap();
    let s = spec("issue_assignees").unwrap();
    let with_extra = |row: Row| {
        let mut m = collect_manifest(&conn, "LIF").unwrap();
        m.tables
            .iter_mut()
            .find(|t| t.name == "issue_assignees")
            .unwrap()
            .rows
            .push(row);
        validate_manifest(&m)
    };
    assert!(validate_manifest(&collect_manifest(&conn, "LIF").unwrap()).is_ok());
    let mut human_on_named = collect_manifest(&conn, "LIF")
        .unwrap()
        .rows("issue_assignees")[0]
        .clone();
    s.set(&mut human_on_named, "id", 900.into());
    s.set(&mut human_on_named, "username", Value::Null);
    assert!(with_extra(human_on_named).is_err());
    let mut duplicate = collect_manifest(&conn, "LIF")
        .unwrap()
        .rows("issue_assignees")[0]
        .clone();
    s.set(&mut duplicate, "id", 901.into());
    assert!(with_extra(duplicate).is_err());
}
