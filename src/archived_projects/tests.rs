use super::*;
use crate::actor::Transport;
use crate::db;

struct Fixture {
    pool: DbPool,
    store: AttachmentStore,
    _dir: tempfile::TempDir,
}

const ADMIN: i64 = 1;
const AUTHOR: i64 = 2;
const VIEWER: i64 = 3;

fn fixture() -> Fixture {
    let pool = db::open_memory().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let store = AttachmentStore::new(dir.path().to_path_buf());
    let hash = store.write(b"archived attachment").unwrap();
    {
        let conn = pool.write().unwrap();
        conn.execute_batch(
            "INSERT INTO users(id,username,email,password_hash,is_admin) VALUES
                (1,'owner','owner@example.test','x',1),
                (2,'author','author@example.test','x',0),
                (3,'viewer','viewer@example.test','x',0);
             UPDATE _actor_state SET user_id=2, transport='api';
             INSERT INTO projects(id,name,identifier,description,lead_user_id) VALUES
                (10,'Shelved','OLD','Kept for later',2),
                (20,'Neighbor','NB','',NULL);
             UPDATE projects SET is_public = 1 WHERE id = 10;
             INSERT INTO project_members(project_id,user_id,role) VALUES (10,2,'lead'),(10,3,'viewer');
             INSERT INTO issues(id,project_id,sequence,title) VALUES (30,10,1,'Live issue'),(31,10,2,'Second');
             INSERT INTO pages(id,project_id,sequence,title,content) VALUES (40,10,1,'Notes','Body');
             INSERT INTO comments(id,issue_id,user_id,content) VALUES (70,30,2,'Written by author');
             UPDATE issues SET status='done' WHERE id=30;",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO attachments(id,sha256,filename,mime,size_bytes,uploader_id)
             VALUES (80,?1,'a.txt','text/plain',19,2)",
            [&hash],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO attachment_links VALUES(80,'issue',30,'2025-01-01 00:00:00');",
        )
        .unwrap();
    }
    Fixture {
        pool,
        store,
        _dir: dir,
    }
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the shape of an authorize callback"
)]
fn as_admin(_: &Connection) -> Result<ActorCtx> {
    Ok(ActorCtx {
        user_id: Some(ADMIN),
        transport: Transport::Web,
    })
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the shape of an authorize callback"
)]
fn grant_admin(_: &Connection) -> Result<Grant> {
    Ok(Grant {
        user_id: ADMIN,
        transport: Transport::Web,
        actor_user_id: Some(ADMIN),
    })
}

fn files(store: &AttachmentStore) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(store.archived_dir())
        .map(|entries| {
            entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn count(pool: &DbPool, sql: &str) -> i64 {
    pool.read()
        .unwrap()
        .query_row(sql, [], |r| r.get(0))
        .unwrap()
}

#[test]
fn archiving_moves_the_project_into_one_tracked_file() {
    let f = fixture();
    let archived = archive(&f.pool, &f.store, 10, &as_admin).unwrap();

    assert_eq!(
        count(&f.pool, "SELECT count(*) FROM projects WHERE id = 10"),
        0
    );
    assert_eq!(
        count(&f.pool, "SELECT count(*) FROM issues WHERE project_id = 10"),
        0
    );
    assert_eq!(
        count(&f.pool, "SELECT count(*) FROM projects WHERE id = 20"),
        1
    );

    let names = files(&f.store);
    assert_eq!(names.len(), 1, "exactly one file: {names:?}");
    assert!(names[0].starts_with("OLD-") && names[0].ends_with(".lific.tar.gz"));
    assert!(valid_file_name(&names[0]));

    let record = archived.record;
    assert_eq!(record.identifier, "OLD");
    assert_eq!(record.name, "Shelved");
    assert_eq!(record.description, "Kept for later");
    assert_eq!(record.file_name, names[0]);
    assert_eq!((record.issue_count, record.page_count), (2, 1));
    assert_eq!(record.archived_by, Some(ADMIN));
    assert_eq!(record.archived_by_name.as_deref(), Some("owner"));
    assert!(record.was_public);
    assert!(record.file_present);
    let path = f.store.archived_dir().join(&record.file_name);
    let (sha, size) = hash_file(&path).unwrap();
    assert_eq!(size, record.size_bytes);
    let stored: String = f
        .pool
        .read()
        .unwrap()
        .query_row(
            "SELECT sha256 FROM archived_projects WHERE id = ?1",
            [record.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, sha);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "archive file is owner-only");
    }

    let listed = list(&f.pool.read().unwrap(), &f.store).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, record.id);
}

#[test]
fn the_archive_file_is_an_ordinary_project_archive() {
    let f = fixture();
    let archived = archive(&f.pool, &f.store, 10, &as_admin).unwrap();
    let path = f.store.archived_dir().join(&archived.record.file_name);

    let other = fixture();
    {
        let conn = other.pool.write().unwrap();
        conn.execute("DELETE FROM projects WHERE identifier = 'OLD'", [])
            .unwrap();
    }
    let report = project_archive::import(&other.pool, &other.store, &path, "owner").unwrap();
    assert_eq!(report.project, "OLD");
}

#[test]
fn unarchiving_restores_content_people_and_authorship() {
    let f = fixture();
    let archived = archive(&f.pool, &f.store, 10, &as_admin).unwrap();
    let back = unarchive(&f.pool, &f.store, archived.record.id, &grant_admin).unwrap();

    assert_eq!(back.identifier, "OLD");
    assert!(back.was_public);
    let conn = f.pool.read().unwrap();
    let (name, description, lead, public): (String, String, Option<i64>, bool) = conn
        .query_row(
            "SELECT name, description, lead_user_id, is_public FROM projects WHERE id = ?1",
            [back.project_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        (name.as_str(), description.as_str()),
        ("Shelved", "Kept for later")
    );
    assert_eq!(
        lead,
        Some(AUTHOR),
        "the original lead, not the unarchiving admin"
    );
    assert!(!public, "publication is never re-applied");

    let members: Vec<(i64, String)> = conn
        .prepare("SELECT user_id, role FROM project_members WHERE project_id = ?1 ORDER BY user_id")
        .unwrap()
        .query_map([back.project_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert_eq!(
        members,
        vec![
            (ADMIN, "lead".to_string()),
            (AUTHOR, "lead".to_string()),
            (VIEWER, "viewer".to_string()),
        ]
    );

    let issues: Vec<(i64, String, String)> = conn
        .prepare(
            "SELECT sequence, title, status FROM issues WHERE project_id = ?1 ORDER BY sequence",
        )
        .unwrap()
        .query_map([back.project_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert_eq!(
        issues,
        vec![
            (1, "Live issue".to_string(), "done".to_string()),
            (2, "Second".to_string(), "backlog".to_string()),
        ]
    );

    let comment: (Option<i64>, Option<String>) = conn
        .query_row(
            "SELECT c.user_id, c.imported_author FROM comments c
             JOIN issues i ON i.id = c.issue_id WHERE i.project_id = ?1",
            [back.project_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(comment, (Some(AUTHOR), None));
    let uploader: (Option<i64>, Option<String>) = conn
        .query_row(
            "SELECT uploader_id, imported_author FROM attachments WHERE filename = 'a.txt'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(uploader, (Some(AUTHOR), None));

    // History reads as it did before the archive: the author's own writes,
    // over their own transport, with no import residue.
    let imported: i64 = conn
        .query_row(
            "SELECT count(*) FROM audit_log WHERE project_id = ?1
             AND (transport = 'imported' OR imported_source IS NOT NULL OR imported_author IS NOT NULL)",
            [back.project_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(imported, 0);
    let by_author: i64 = conn
        .query_row(
            "SELECT count(*) FROM audit_log WHERE project_id = ?1
             AND actor_user_id = 2 AND transport = 'api' AND entity_type = 'issue'",
            [back.project_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(by_author >= 2, "issue history keeps its author");
    let transitions: (Option<i64>, String) = conn
        .query_row(
            "SELECT s.actor_user_id, s.transport FROM status_transitions s
             JOIN issues i ON i.id = s.issue_id WHERE i.project_id = ?1 AND s.to_status = 'done'",
            [back.project_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(transitions, (Some(AUTHOR), "api".to_string()));

    assert_eq!(count(&f.pool, "SELECT count(*) FROM archived_projects"), 0);
    assert!(
        files(&f.store).is_empty(),
        "the file goes with the listing row"
    );
}

#[test]
fn a_refused_archive_changes_nothing() {
    let f = fixture();
    let error = archive(&f.pool, &f.store, 10, &|_| {
        Err(LificError::Forbidden("no".into()))
    })
    .unwrap_err();
    assert!(matches!(error, LificError::Forbidden(_)));
    assert_eq!(
        count(&f.pool, "SELECT count(*) FROM projects WHERE id = 10"),
        1
    );
    assert_eq!(count(&f.pool, "SELECT count(*) FROM archived_projects"), 0);
    assert!(files(&f.store).is_empty());
}

#[test]
fn archiving_a_missing_project_leaves_no_file() {
    let f = fixture();
    let error = archive(&f.pool, &f.store, 999, &as_admin).unwrap_err();
    assert!(matches!(error, LificError::NotFound(_)));
    assert!(files(&f.store).is_empty());
}

#[test]
fn archiving_the_same_identifier_twice_keeps_both_files() {
    let f = fixture();
    let first = archive(&f.pool, &f.store, 10, &as_admin).unwrap();
    {
        let conn = f.pool.write().unwrap();
        conn.execute(
            "INSERT INTO projects(id,name,identifier) VALUES (11,'Again','OLD')",
            [],
        )
        .unwrap();
    }
    let second = archive(&f.pool, &f.store, 11, &as_admin).unwrap();
    assert_ne!(first.record.file_name, second.record.file_name);
    assert_eq!(files(&f.store).len(), 2);
    assert_eq!(list(&f.pool.read().unwrap(), &f.store).unwrap().len(), 2);
}

#[test]
fn unarchive_refuses_an_identifier_that_is_taken_and_keeps_the_archive() {
    let f = fixture();
    let archived = archive(&f.pool, &f.store, 10, &as_admin).unwrap();
    {
        let conn = f.pool.write().unwrap();
        conn.execute(
            "INSERT INTO projects(id,name,identifier) VALUES (12,'Usurper','OLD')",
            [],
        )
        .unwrap();
    }
    let error = unarchive(&f.pool, &f.store, archived.record.id, &grant_admin).unwrap_err();
    assert!(
        matches!(error, LificError::Conflict(ref m) if m.contains("OLD")),
        "{error}"
    );
    assert_eq!(count(&f.pool, "SELECT count(*) FROM archived_projects"), 1);
    assert_eq!(files(&f.store).len(), 1);
}

#[test]
fn unarchive_refuses_a_file_that_changed_since_archiving() {
    let f = fixture();
    let archived = archive(&f.pool, &f.store, 10, &as_admin).unwrap();
    let path = f.store.archived_dir().join(&archived.record.file_name);
    let mut bytes = std::fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    std::fs::write(&path, bytes).unwrap();

    let error = unarchive(&f.pool, &f.store, archived.record.id, &grant_admin).unwrap_err();
    assert!(
        matches!(error, LificError::Conflict(ref m) if m.contains("checksum")),
        "{error}"
    );
    assert_eq!(
        count(
            &f.pool,
            "SELECT count(*) FROM projects WHERE identifier = 'OLD'"
        ),
        0
    );
    assert_eq!(count(&f.pool, "SELECT count(*) FROM archived_projects"), 1);
}

#[test]
fn a_missing_file_is_listed_and_refused() {
    let f = fixture();
    let archived = archive(&f.pool, &f.store, 10, &as_admin).unwrap();
    std::fs::remove_file(f.store.archived_dir().join(&archived.record.file_name)).unwrap();

    let listed = list(&f.pool.read().unwrap(), &f.store).unwrap();
    assert!(!listed[0].file_present);
    let error = unarchive(&f.pool, &f.store, archived.record.id, &grant_admin).unwrap_err();
    assert!(matches!(error, LificError::NotFound(_)), "{error}");
    assert_eq!(count(&f.pool, "SELECT count(*) FROM archived_projects"), 1);
}

#[test]
fn a_refused_unarchive_keeps_the_archive() {
    let f = fixture();
    let archived = archive(&f.pool, &f.store, 10, &as_admin).unwrap();
    let error = unarchive(&f.pool, &f.store, archived.record.id, &|_| {
        Err(LificError::Forbidden("no".into()))
    })
    .unwrap_err();
    assert!(matches!(error, LificError::Forbidden(_)));
    assert_eq!(
        count(
            &f.pool,
            "SELECT count(*) FROM projects WHERE identifier = 'OLD'"
        ),
        0
    );
    assert_eq!(count(&f.pool, "SELECT count(*) FROM archived_projects"), 1);
    assert_eq!(files(&f.store).len(), 1);
}

#[test]
fn people_deleted_while_archived_are_skipped_on_unarchive() {
    let f = fixture();
    let archived = archive(&f.pool, &f.store, 10, &as_admin).unwrap();
    {
        let conn = f.pool.write().unwrap();
        conn.execute("DELETE FROM users WHERE id IN (2, 3)", [])
            .unwrap();
    }
    let back = unarchive(&f.pool, &f.store, archived.record.id, &grant_admin).unwrap();
    let conn = f.pool.read().unwrap();
    let lead: Option<i64> = conn
        .query_row(
            "SELECT lead_user_id FROM projects WHERE id = ?1",
            [back.project_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(lead, None);
    let members: i64 = conn
        .query_row(
            "SELECT count(*) FROM project_members WHERE project_id = ?1",
            [back.project_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(members, 1, "only the unarchiving admin");
    let author: Option<String> = conn
        .query_row(
            "SELECT c.imported_author FROM comments c JOIN issues i ON i.id = c.issue_id
             WHERE i.project_id = ?1",
            [back.project_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(author.as_deref(), Some("author (imported)"));
}

#[test]
fn unarchiving_an_unknown_id_is_not_found() {
    let f = fixture();
    let error = unarchive(&f.pool, &f.store, 404, &grant_admin).unwrap_err();
    assert!(matches!(error, LificError::NotFound(_)));
}

#[test]
fn file_names_are_restricted_to_what_archiving_writes() {
    assert!(valid_file_name("LIF-20261009-120000.lific.tar.gz"));
    assert!(valid_file_name("LIF-20261009-120000-2.lific.tar.gz"));
    assert!(!valid_file_name(".LIF.lific.tar.gz"));
    assert!(!valid_file_name("../LIF.lific.tar.gz"));
    assert!(!valid_file_name("LIF/x.lific.tar.gz"));
    assert!(!valid_file_name("LIF.tar.gz"));
    assert!(!valid_file_name(".lific.tar.gz"));
    assert!(!valid_file_name(&format!(
        "{}.lific.tar.gz",
        "A".repeat(200)
    )));
}

#[test]
fn history_about_a_purged_entity_keeps_its_provenance() {
    let f = fixture();
    {
        let conn = f.pool.write().unwrap();
        conn.execute(
            "INSERT INTO audit_log(transport,entity_type,entity_id,entity_label,project_id,action,actor_user_id)
             VALUES ('api','issue',999,'OLD-9',10,'delete',2)",
            [],
        )
        .unwrap();
    }
    let archived = archive(&f.pool, &f.store, 10, &as_admin).unwrap();
    let back = unarchive(&f.pool, &f.store, archived.record.id, &grant_admin).unwrap();
    let (actor, transport, source): (Option<i64>, String, Option<String>) = f
        .pool
        .read()
        .unwrap()
        .query_row(
            "SELECT actor_user_id, transport, imported_source FROM audit_log
             WHERE project_id = ?1 AND entity_label = 'OLD-9'",
            [back.project_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!((actor, transport.as_str()), (Some(AUTHOR), "api"));
    assert!(
        source.is_some_and(|s| s.contains("999")),
        "the original entity ID survives"
    );
}
