//! LIF-525: a live claim is coordination state on this instance, so a
//! project archive leaves it out and an imported issue starts unclaimed.

use super::*;
use crate::db::models::Claimant;
use crate::db::queries;

#[test]
fn an_archive_carries_no_live_claim() {
    let pool = db::open_memory().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let store = AttachmentStore::new(dir.path().to_path_buf());
    {
        let conn = pool.write().unwrap();
        conn.execute_batch(
            "INSERT INTO users(id,username,email,password_hash,is_admin)
               VALUES (1,'owner','owner@example.test','x',1);
             INSERT INTO projects(id,name,identifier) VALUES (10,'Portable','LIF');
             INSERT INTO issues(id,project_id,sequence,title,status)
               VALUES (30,10,1,'Held','active');",
        )
        .unwrap();
        let holder = Claimant {
            user_id: 1,
            worker: Some("p-1".into()),
        };
        queries::claims::claim(&conn, 30, "LIF-1", &holder, 60).unwrap();
        assert!(queries::get_issue(&conn, 30).unwrap().claim.is_some());
    }

    let archive = dir.path().join("held.tar.gz");
    export(&pool, &store, "LIF", &archive).unwrap();
    let manifest = stage(&archive).unwrap().manifest;
    assert!(
        manifest
            .tables
            .iter()
            .all(|table| table.name != "issue_claims"),
        "the archive has no claims table"
    );

    let dest = db::open_memory().unwrap();
    let dest_dir = tempfile::tempdir().unwrap();
    let dest_store = AttachmentStore::new(dest_dir.path().to_path_buf());
    dest.write()
        .unwrap()
        .execute_batch(
            "INSERT INTO users(id,username,email,password_hash,is_admin)
               VALUES (1,'owner','owner@example.test','x',1);",
        )
        .unwrap();
    import(&dest, &dest_store, &archive, "owner").unwrap();
    let conn = dest.read().unwrap();
    let issue =
        queries::get_issue(&conn, queries::resolve_identifier(&conn, "LIF-1").unwrap()).unwrap();
    assert_eq!(issue.status.as_str(), "active", "the status travels");
    assert!(issue.claim.is_none(), "the claim does not");
    let rows: i64 = conn
        .query_row("SELECT count(*) FROM issue_claims", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0);
}
