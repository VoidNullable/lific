use super::*;
use crate::actor::{self, ActorCtx, Transport};
use crate::db::{
    models::{CreateIssue, CreateProject, UpdateIssue},
    queries,
};
use std::sync::{Arc, Barrier, mpsc};
use std::time::Duration;

fn create_issue(conn: &Connection, project_id: i64, title: &str) -> Result<i64, LificError> {
    Ok(queries::create_issue(
        conn,
        &CreateIssue {
            project_id,
            title: title.into(),
            ..Default::default()
        },
    )?
    .id)
}

fn issue_and_audit_counts(db: &DbPool, project_id: i64) -> (i64, i64, i64) {
    db.read()
        .unwrap()
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM issues WHERE project_id = ?1),
                (SELECT COUNT(DISTINCT sequence) FROM issues WHERE project_id = ?1),
                (SELECT COUNT(*) FROM audit_log
                 WHERE entity_type = 'issue' AND action = 'create' AND project_id = ?1)",
            [project_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap()
}

#[test]
fn shared_pool_writers_commit_once_and_readers_see_only_committed_issues() {
    let directory = tempfile::tempdir().unwrap();
    let db = open(&directory.path().join("shared.db")).unwrap();
    let project_id = {
        let conn = db.write().unwrap();
        queries::create_project(
            &conn,
            &CreateProject {
                name: "Concurrent writes".into(),
                identifier: "CONC".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id
    };
    const OTHER_WRITERS: usize = 8;
    let start = Arc::new(Barrier::new(OTHER_WRITERS + 1));
    let (inserted, wait_for_insert) = mpsc::channel();
    let (release, wait_for_release) = mpsc::channel();

    std::thread::scope(|scope| {
        let first_pool = db.clone();
        let first = scope.spawn(move || {
            first_pool
                .transaction(|conn| {
                    let id = create_issue(conn, project_id, "first")?;
                    inserted.send(()).unwrap();
                    wait_for_release
                        .recv_timeout(Duration::from_secs(10))
                        .unwrap();
                    Ok(id)
                })
                .unwrap()
        });
        wait_for_insert
            .recv_timeout(Duration::from_secs(10))
            .unwrap();

        let reader_pool = db.clone();
        let (read_result, wait_for_read) = mpsc::channel();
        let reader = scope.spawn(move || {
            read_result
                .send(issue_and_audit_counts(&reader_pool, project_id))
                .unwrap();
        });
        assert_eq!(
            wait_for_read.recv_timeout(Duration::from_secs(5)).unwrap(),
            (0, 0, 0),
            "a reader should complete while the first write is uncommitted"
        );
        reader.join().unwrap();

        let mut others = Vec::with_capacity(OTHER_WRITERS);
        for index in 0..OTHER_WRITERS {
            let pool = db.clone();
            let start = start.clone();
            others.push(scope.spawn(move || {
                start.wait();
                pool.transaction(|conn| create_issue(conn, project_id, &format!("writer {index}")))
                    .unwrap()
            }));
        }
        start.wait();
        release.send(()).unwrap();
        let mut ids = vec![first.join().unwrap()];
        ids.extend(others.into_iter().map(|writer| writer.join().unwrap()));
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), OTHER_WRITERS + 1);
    });

    let expected = (OTHER_WRITERS + 1) as i64;
    assert_eq!(
        issue_and_audit_counts(&db, project_id),
        (expected, expected, expected)
    );
}

#[test]
fn independent_pool_contention_times_out_then_commits_without_partial_rows() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("independent.db");
    let first_pool = open(&path).unwrap();
    let second_pool = open(&path).unwrap();
    second_pool
        .lock_writer()
        .unwrap()
        .busy_timeout(Duration::from_millis(50))
        .unwrap();
    let project_id = {
        let conn = first_pool.write().unwrap();
        queries::create_project(
            &conn,
            &CreateProject {
                name: "Independent pools".into(),
                identifier: "INDEP".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id
    };
    let (holding, wait_for_hold) = mpsc::channel();
    let (release, wait_for_release) = mpsc::channel();

    std::thread::scope(|scope| {
        let holder_pool = first_pool.clone();
        let holder = scope.spawn(move || {
            holder_pool
                .transaction(|conn| {
                    create_issue(conn, project_id, "first committed")?;
                    holding.send(()).unwrap();
                    wait_for_release
                        .recv_timeout(Duration::from_secs(10))
                        .unwrap();
                    Ok(())
                })
                .unwrap();
        });
        wait_for_hold.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(issue_and_audit_counts(&second_pool, project_id), (0, 0, 0));
        let failure = second_pool.transaction(|conn| {
            create_issue(conn, project_id, "timed out")?;
            Ok(())
        });
        assert!(
            matches!(failure, Err(LificError::Database(rusqlite::Error::SqliteFailure(error, _)))
                if error.code == rusqlite::ErrorCode::DatabaseBusy),
            "{failure:?}"
        );
        assert_eq!(issue_and_audit_counts(&second_pool, project_id), (0, 0, 0));
        release.send(()).unwrap();
        holder.join().unwrap();
    });

    second_pool
        .transaction(|conn| {
            create_issue(conn, project_id, "second committed")?;
            create_issue(conn, project_id, "third committed")?;
            Ok(())
        })
        .unwrap();
    let rollback: Result<(), LificError> = second_pool.transaction(|conn| {
        create_issue(conn, project_id, "rolled back one")?;
        create_issue(conn, project_id, "rolled back two")?;
        Err(LificError::BadRequest("rollback probe".into()))
    });
    assert!(matches!(rollback, Err(LificError::BadRequest(_))));
    assert_eq!(issue_and_audit_counts(&first_pool, project_id), (3, 3, 3));
}

#[test]
fn external_sqlite_writer_uses_the_database_busy_timeout() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("external.db");
    let db = open(&path).unwrap();
    {
        let conn = db.write().unwrap();
        conn.busy_timeout(Duration::from_millis(50)).unwrap();
    }
    let mut external = Connection::open(&path).unwrap();
    let transaction = external
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let result = db.transaction(|_| Ok(()));
    assert!(
        matches!(
            result,
            Err(LificError::Database(rusqlite::Error::SqliteFailure(error, _)))
                if error.code == rusqlite::ErrorCode::DatabaseBusy
        ),
        "an external writer should reach SQLite's busy timeout"
    );
    transaction.rollback().unwrap();
    db.transaction(|_| Ok(())).unwrap();
}

#[test]
fn transaction_rejects_a_missing_actor_stamp_before_mutating() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("stamp.db");
    let db = open(&path).unwrap();
    let external = Connection::open(&path).unwrap();
    external.execute("DELETE FROM _actor_state", []).unwrap();
    let result: Result<(), LificError> = db.transaction(|_| {
        panic!("must not run a mutation without an audit actor stamp");
    });
    assert!(matches!(
        result,
        Err(LificError::Database(rusqlite::Error::QueryReturnedNoRows))
    ));
    external
        .execute(
            "INSERT INTO _actor_state (id, user_id, transport) VALUES (1, NULL, 'system')",
            [],
        )
        .unwrap();
    db.transaction(|_| Ok(())).unwrap();
}

#[test]
fn sqlite_reserves_the_writer_before_actor_stamping_and_rolls_back_on_drop() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("reserved.db");
    let db = open(&path).unwrap();
    let external = Connection::open(&path).unwrap();
    external.busy_timeout(Duration::from_millis(50)).unwrap();
    let mut writer = db.writer().unwrap();
    let tx = writer.transaction().unwrap();
    let stamp = external.execute("UPDATE _actor_state SET user_id = 99", []);
    assert!(
        matches!(stamp, Err(rusqlite::Error::SqliteFailure(error, _))
        if error.code == rusqlite::ErrorCode::DatabaseBusy)
    );
    queries::create_project(
        &tx,
        &CreateProject {
            name: "Uncommitted".into(),
            identifier: "DROP".into(),
            ..Default::default()
        },
    )
    .unwrap();
    drop(tx);
    drop(writer);
    let count: i64 = external
        .query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
    external
        .execute("UPDATE _actor_state SET user_id = 99", [])
        .unwrap();
    assert!(!directory.path().join("reserved.db.write.lock").exists());
}

#[test]
fn independent_pools_serialize_conditional_updates_to_one_issue() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("conflict.db");
    let first_pool = open(&path).unwrap();
    let second_pool = open(&path).unwrap();
    let (project_id, issue) = first_pool
        .transaction(|conn| {
            let project = queries::create_project(
                conn,
                &CreateProject {
                    name: "Concurrent edits".into(),
                    identifier: "EDIT".into(),
                    ..Default::default()
                },
            )?;
            let issue = queries::create_issue(
                conn,
                &CreateIssue {
                    project_id: project.id,
                    title: "Original".into(),
                    ..Default::default()
                },
            )?;
            Ok((project.id, issue))
        })
        .unwrap();
    let issue_id = issue.id;
    let expected_seq = issue.seq;
    let start = Arc::new(Barrier::new(3));
    let results: Vec<_> = std::thread::scope(|scope| {
        let writers: Vec<_> = [
            (first_pool.clone(), "First edit"),
            (second_pool, "Second edit"),
        ]
        .into_iter()
        .map(|(pool, title)| {
            let start = start.clone();
            scope.spawn(move || {
                start.wait();
                pool.transaction(|conn| {
                    queries::update_issue(
                        conn,
                        issue_id,
                        &UpdateIssue {
                            title: Some(title.into()),
                            expected_seq: Some(expected_seq),
                            ..Default::default()
                        },
                    )
                })
            })
        })
        .collect();
        start.wait();
        writers
            .into_iter()
            .map(|writer| writer.join().unwrap())
            .collect()
    });
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(LificError::UpdateConflict { .. })))
            .count(),
        1
    );
    let winner = results.into_iter().find_map(Result::ok).unwrap();
    let conn = first_pool.read().unwrap();
    let saved = queries::get_issue(&conn, issue.id).unwrap();
    assert_eq!(saved.title, winner.title);
    assert_eq!(saved.seq, winner.seq);
    let changes: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM audit_log
             WHERE entity_type = 'issue' AND entity_id = ?1
               AND action = 'update' AND field = 'title'",
            [issue.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(changes, 1);
    assert_eq!(issue_and_audit_counts(&first_pool, project_id), (1, 1, 1));
}

#[test]
fn independent_pools_do_not_mix_write_audit_actors() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("contention.db");
    let first_pool = open(&path).unwrap();
    let second_pool = open(&path).unwrap();
    let (first_stamped, wait_for_first) = mpsc::channel();
    let (second_started, wait_for_second_start) = mpsc::channel();
    let (second_stamped, wait_for_second_stamp) = mpsc::channel();

    let (first_id, second_id) = std::thread::scope(|scope| {
        let first = scope.spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            runtime.block_on(actor::scope(
                ActorCtx {
                    user_id: Some(11),
                    transport: Transport::Api,
                },
                async move {
                    let mut writer = first_pool.writer().unwrap();
                    let conn = writer.transaction().unwrap();
                    first_stamped.send(()).unwrap();
                    wait_for_second_start
                        .recv_timeout(Duration::from_secs(10))
                        .unwrap();
                    // Before cross-pool serialization, the second writer can
                    // stamp its actor while this guard is still held.
                    assert!(matches!(
                        wait_for_second_stamp.recv_timeout(Duration::from_millis(200)),
                        Err(mpsc::RecvTimeoutError::Timeout)
                    ));
                    let project = queries::create_project(
                        &conn,
                        &CreateProject {
                            name: "First writer".into(),
                            identifier: "FIRST".into(),
                            ..Default::default()
                        },
                    )
                    .unwrap();
                    conn.commit().unwrap();
                    project.id
                },
            ))
        });
        let second = scope.spawn(move || {
            wait_for_first
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
            second_started.send(()).unwrap();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            runtime.block_on(actor::scope(
                ActorCtx {
                    user_id: Some(22),
                    transport: Transport::Mcp,
                },
                async move {
                    let mut writer = second_pool.writer().unwrap();
                    let conn = writer.transaction().unwrap();
                    // The first writer may have finished after its timeout check.
                    let _ = second_stamped.send(());
                    let project = queries::create_project(
                        &conn,
                        &CreateProject {
                            name: "Second writer".into(),
                            identifier: "SECND".into(),
                            ..Default::default()
                        },
                    )
                    .unwrap();
                    conn.commit().unwrap();
                    project.id
                },
            ))
        });
        (first.join().unwrap(), second.join().unwrap())
    });

    let conn = open(&path).unwrap().read().unwrap();
    for (id, expected) in [
        (first_id, (Some(11), "api")),
        (second_id, (Some(22), "mcp")),
    ] {
        let actual: (Option<i64>, String) = conn
            .query_row(
                "SELECT actor_user_id, transport FROM audit_log
                 WHERE entity_type = 'project' AND entity_id = ?1 AND action = 'create'",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(actual.0, expected.0, "project {id} has the wrong actor");
        assert_eq!(actual.1, expected.1, "project {id} has the wrong transport");
    }
}
