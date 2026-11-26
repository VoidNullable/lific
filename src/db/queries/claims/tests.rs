use super::*;
use crate::db::models::{CreateIssue, CreateProject, Priority};
use crate::db::{DbPool, queries};

/// A project with one todo issue and two accounts. Returns the pool, the
/// issue id and both user ids.
fn setup() -> (DbPool, i64, i64, i64) {
    let pool = crate::db::open_memory().unwrap();
    let conn = pool.write().unwrap();
    let project = queries::create_project(
        &conn,
        &CreateProject {
            name: "Claims".into(),
            identifier: "CLM".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let issue = queries::create_issue(
        &conn,
        &CreateIssue {
            project_id: project.id,
            title: "Shared work".into(),
            status: Status::Todo,
            priority: Priority::None,
            ..Default::default()
        },
    )
    .unwrap();
    let mut users = Vec::new();
    for name in ["alice", "bob"] {
        conn.execute(
            "INSERT INTO users (username, email, password_hash) VALUES (?1, ?1 || '@test', 'x')",
            [name],
        )
        .unwrap();
        users.push(conn.last_insert_rowid());
    }
    drop(conn);
    (pool, issue.id, users[0], users[1])
}

fn worker(user_id: i64, worker: Option<&str>) -> Claimant {
    Claimant {
        user_id,
        worker: worker.map(str::to_string),
    }
}

#[test]
fn a_second_worker_is_refused_until_the_first_releases() {
    let (pool, issue, alice, _) = setup();
    let _now = pin_now("2026-11-26 12:00:00");
    let conn = pool.write().unwrap();
    let first = worker(alice, Some("p-1"));
    let second = worker(alice, Some("p-2"));

    let held = claim(&conn, issue, "CLM-1", &first, 15).unwrap();
    assert_eq!(held.worker.as_deref(), Some("p-1"));
    assert_eq!(held.expires_at, "2026-11-26 12:15:00");

    // Same account, different worker: refused, naming the holder.
    let refused = claim(&conn, issue, "CLM-1", &second, 15).unwrap_err();
    let message = refused.to_string();
    assert!(
        message.contains("CLM-1 is claimed by @alice (worker p-1) until 2026-11-26 12:15 UTC"),
        "{message}"
    );
    assert!(matches!(refused, LificError::Conflict(_)));
    // And it may not release what it does not hold.
    assert!(matches!(
        release(&conn, issue, "CLM-1", &second),
        Err(LificError::Conflict(_))
    ));

    assert!(release(&conn, issue, "CLM-1", &first).unwrap().is_some());
    assert!(live_claim(&conn, issue).unwrap().is_none());
    let taken = claim(&conn, issue, "CLM-1", &second, 15).unwrap();
    assert_eq!(taken.worker.as_deref(), Some("p-2"));
}

#[test]
fn an_expired_claim_is_ignored_without_any_cleanup() {
    let (pool, issue, alice, bob) = setup();
    let conn = pool.write().unwrap();
    {
        let _now = pin_now("2026-11-26 12:00:00");
        claim(&conn, issue, "CLM-1", &worker(alice, Some("p-1")), 15).unwrap();
    }
    // One second before expiry it still holds.
    {
        let _now = pin_now("2026-11-26 12:14:59");
        assert!(live_claim(&conn, issue).unwrap().is_some());
        assert!(claim(&conn, issue, "CLM-1", &worker(bob, None), 15).is_err());
    }
    // At expiry the row is still there, and every read ignores it.
    let _now = pin_now("2026-11-26 12:15:00");
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM issue_claims", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 1, "nothing swept the expired row");
    assert!(live_claim(&conn, issue).unwrap().is_none());
    assert!(claims_by_issue(&conn, &[issue]).unwrap().is_empty());
    assert!(queries::get_issue(&conn, issue).unwrap().claim.is_none());

    let taken = claim(&conn, issue, "CLM-1", &worker(bob, None), 15).unwrap();
    assert_eq!((taken.username.as_str(), taken.worker), ("bob", None));
    assert_eq!(taken.claimed_at, "2026-11-26 12:15:00");
}

#[test]
fn a_reclaim_moves_the_expiry_and_keeps_the_claim_time() {
    let (pool, issue, alice, _) = setup();
    let conn = pool.write().unwrap();
    let me = worker(alice, Some("p-1"));
    {
        let _now = pin_now("2026-11-26 12:00:00");
        claim(&conn, issue, "CLM-1", &me, 15).unwrap();
    }
    let _now = pin_now("2026-11-26 12:10:00");
    let renewed = claim(&conn, issue, "CLM-1", &me, 30).unwrap();
    assert_eq!(renewed.claimed_at, "2026-11-26 12:00:00");
    assert_eq!(renewed.expires_at, "2026-11-26 12:40:00");
    let renewed_at: String = conn
        .query_row(
            "SELECT renewed_at FROM issue_claims WHERE issue_id = ?1",
            [issue],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(renewed_at, "2026-11-26 12:10:00");
}

#[test]
fn an_account_claim_is_held_by_every_session_of_that_account() {
    let (pool, issue, alice, bob) = setup();
    let _now = pin_now("2026-11-26 12:00:00");
    let conn = pool.write().unwrap();
    let held = claim(&conn, issue, "CLM-1", &worker(alice, None), 15).unwrap();
    assert!(held.held_by(&worker(alice, Some("p-9"))));
    assert!(held.held_by(&worker(alice, None)));
    assert!(!held.held_by(&worker(bob, None)));
    // A worker-tied claim is not held by the bare account.
    release(&conn, issue, "CLM-1", &worker(alice, None)).unwrap();
    let held = claim(&conn, issue, "CLM-1", &worker(alice, Some("p-1")), 15).unwrap();
    assert!(!held.held_by(&worker(alice, None)));
}

#[test]
fn closing_or_deleting_an_issue_ends_its_claim() {
    let (pool, issue, alice, _) = setup();
    let _now = pin_now("2026-11-26 12:00:00");
    let conn = pool.write().unwrap();
    let me = worker(alice, None);
    claim(&conn, issue, "CLM-1", &me, 15).unwrap();
    conn.execute("UPDATE issues SET status = 'done' WHERE id = ?1", [issue])
        .unwrap();
    assert!(live_claim(&conn, issue).unwrap().is_none());

    conn.execute("UPDATE issues SET status = 'todo' WHERE id = ?1", [issue])
        .unwrap();
    claim(&conn, issue, "CLM-1", &me, 15).unwrap();
    queries::delete_issue(&conn, issue).unwrap();
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM issue_claims", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 0);
}

#[test]
fn workable_leaves_out_only_what_someone_else_holds() {
    let (pool, issue, alice, bob) = setup();
    let _now = pin_now("2026-11-26 12:00:00");
    let conn = pool.write().unwrap();
    claim(&conn, issue, "CLM-1", &worker(alice, Some("p-1")), 15).unwrap();
    let workable = |claimant: Option<Claimant>| {
        queries::list_issues(
            &conn,
            &crate::db::models::ListIssuesQuery {
                workable: Some(true),
                claimant,
                ..Default::default()
            },
        )
        .unwrap()
        .len()
    };
    assert_eq!(workable(Some(worker(alice, Some("p-1")))), 1, "the holder");
    assert_eq!(workable(Some(worker(alice, Some("p-2")))), 0, "a sibling");
    assert_eq!(workable(Some(worker(alice, None))), 0, "the bare account");
    assert_eq!(workable(Some(worker(bob, None))), 0, "another account");
    assert_eq!(workable(None), 0, "nobody in particular");
    drop(_now);
    let _later = pin_now("2026-11-26 12:15:00");
    assert_eq!(workable(Some(worker(bob, None))), 1, "after expiry");
}

#[test]
fn claim_lengths_and_worker_ids_are_bounded() {
    assert_eq!(minutes(None).unwrap(), DEFAULT_MINUTES);
    assert_eq!(minutes(Some(MAX_MINUTES)).unwrap(), MAX_MINUTES);
    assert!(minutes(Some(0)).is_err());
    assert!(minutes(Some(MAX_MINUTES + 1)).is_err());
    assert!(validate_worker("p-1a2b3c4d").is_ok());
    assert!(validate_worker("").is_err());
    assert!(validate_worker("has space").is_err());
    assert!(validate_worker(&"x".repeat(MAX_WORKER_CHARS + 1)).is_err());
    let a = worker_from_session("session-a").unwrap();
    assert_eq!(a, worker_from_session("session-a").unwrap(), "stable");
    assert_ne!(a, worker_from_session("session-b").unwrap());
    assert!(a.starts_with("s-") && !a.contains("session"));
    assert!(worker_from_session("  ").is_none());
}
