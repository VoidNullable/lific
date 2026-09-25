//! LIF-484: user and date waits travel with a project archive.

use super::*;
use crate::db::models::CreateWait;
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
             INSERT INTO issues(id,project_id,sequence,title) VALUES (30,10,1,'Waiting'),(31,10,2,'Free');",
        )
        .unwrap();
        for input in [
            CreateWait {
                user: Some("blake".into()),
                note: Some("decide".into()),
                ..Default::default()
            },
            CreateWait {
                user: Some("outsider".into()),
                ..Default::default()
            },
            CreateWait {
                from: Some("2026-09-28".into()),
                until: Some("2026-09-29".into()),
                note: Some("filing office".into()),
                ..Default::default()
            },
        ] {
            queries::waits::add_wait(&conn, 30, &input, Some(2)).unwrap();
        }
    }
    (pool, dir, store)
}

/// A destination that knows `owner` and `blake` but not `outsider`.
fn destination() -> (DbPool, tempfile::TempDir, AttachmentStore) {
    let pool = db::open_memory().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let store = AttachmentStore::new(dir.path().to_path_buf());
    {
        let conn = pool.write().unwrap();
        conn.execute_batch(
            "INSERT INTO users(id,username,email,password_hash,is_admin)
               VALUES (5,'owner','owner@example.test','x',1),
                      (6,'Blake','blake@example.test','x',0);",
        )
        .unwrap();
    }
    (pool, dir, store)
}

#[test]
fn waits_round_trip_by_username_and_unknown_accounts_are_reported() {
    let (source, dir, store) = source();
    let archive = dir.path().join("waits.tar.gz");
    export(&source, &store, "LIF", &archive).unwrap();
    let manifest = stage(&archive).unwrap().manifest;
    let json = serde_json::to_string(&manifest.rows("issue_waits")).unwrap();
    assert!(
        json.contains("\"blake\""),
        "waits travel by username: {json}"
    );

    let (dest, _dest_dir, dest_store) = destination();
    let report = import(&dest, &dest_store, &archive, "owner").unwrap();
    assert!(
        report
            .external_references
            .iter()
            .any(|r| r.contains("no active user named outsider")),
        "{:?}",
        report.external_references
    );

    let conn = dest.read().unwrap();
    let issue = queries::resolve_identifier(&conn, "LIF-1").unwrap();
    let waits = queries::waits::list_waits(&conn, issue).unwrap();
    assert_eq!(waits.len(), 2, "{waits:?}");
    assert_eq!(
        waits[0].user_id,
        Some(6),
        "bound to the destination account"
    );
    assert_eq!(waits[0].note, "decide");
    assert_eq!(waits[1].earliest.as_deref(), Some("2026-09-28"));
    assert_eq!(waits[1].latest.as_deref(), Some("2026-09-29"));
    assert_eq!(waits[1].note, "filing office");

    // The imported waits block exactly as they did at the source.
    let _day = queries::waits::pin_today("2026-09-01");
    let workable = queries::list_issues(
        &conn,
        &crate::db::models::ListIssuesQuery {
            project_id: Some(queries::issue_project_id(&conn, issue).unwrap()),
            workable: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        workable
            .iter()
            .map(|i| i.identifier.as_str())
            .collect::<Vec<_>>(),
        ["LIF-2"]
    );
    drop(conn);

    // And they export again from the destination.
    let again = dir.path().join("again.tar.gz");
    export(&dest, &dest_store, "LIF", &again).unwrap();
    assert_eq!(stage(&again).unwrap().manifest.rows("issue_waits").len(), 2);
}

#[test]
fn an_archive_from_before_waits_still_imports() {
    let (source, dir, store) = source();
    let conn = source.read().unwrap();
    let mut manifest = collect_manifest(&conn, "LIF").unwrap();
    drop(conn);
    manifest.tables.retain(|table| table.name != "issue_waits");
    let path = dir.path().join("old.tar.gz");
    super::tests::write_manifest(&path, &manifest, &[]);
    let (dest, _dest_dir, dest_store) = destination();
    import(&dest, &dest_store, &path, "owner").unwrap();
    let conn = dest.read().unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM issue_waits", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
    let _ = store;
}

#[test]
fn malformed_wait_rows_are_rejected_before_import() {
    let (source, _dir, _store) = source();
    let conn = source.read().unwrap();
    let base = collect_manifest(&conn, "LIF").unwrap();
    let s = spec("issue_waits").unwrap();
    let mutate = |f: &dyn Fn(&mut Row)| {
        let mut m = collect_manifest(&conn, "LIF").unwrap();
        let table = m
            .tables
            .iter_mut()
            .find(|t| t.name == "issue_waits")
            .unwrap();
        f(&mut table.rows[0]);
        validate_manifest(&m)
    };
    assert!(validate_manifest(&base).is_ok());
    assert!(mutate(&|row| s.set(row, "kind", "someday".into())).is_err());
    assert!(mutate(&|row| s.set(row, "earliest", "2026-09-28".into())).is_err());
    assert!(
        mutate(&|row| {
            s.set(row, "kind", "date".into());
            s.set(row, "username", Value::Null);
            s.set(row, "earliest", "2026-09-30".into());
            s.set(row, "latest", "2026-09-01".into());
        })
        .is_err()
    );
    assert!(
        mutate(&|row| {
            s.set(row, "kind", "date".into());
            s.set(row, "username", Value::Null);
            s.set(row, "earliest", "soon".into());
            s.set(row, "latest", "soon".into());
        })
        .is_err()
    );
    let mut duplicated = collect_manifest(&conn, "LIF").unwrap();
    let table = duplicated
        .tables
        .iter_mut()
        .find(|t| t.name == "issue_waits")
        .unwrap();
    let mut copy = table.rows[0].clone();
    s.set(&mut copy, "id", 999.into());
    table.rows.push(copy);
    assert!(validate_manifest(&duplicated).is_err());
}
