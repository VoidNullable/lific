//! Background sweep that empties the trash (LIF-438).
//!
//! Deleting an issue, page or comment leaves a tombstone: the row stays, with
//! `deleted_at` set, so a replica can sync the deletion and a user can undo
//! it. This task is what makes that bounded. Every `[retention] trash_days`
//! the sweep runs a real DELETE over anything past the window, which is also
//! the only moment the foreign-key cascades fire.
//!
//! Shape copied from `backup::start_backup_task`: settle, sweep once at
//! startup, then sweep on an interval, with the synchronous SQLite work handed
//! to the blocking pool and awaited so one sweep can never queue behind
//! another.

use std::time::Duration;

use tracing::{error, info, warn};

use crate::db::DbPool;
use crate::db::queries::trash;

/// How often the sweep runs. Four times a day: a tombstone lives for days, so
/// collecting it within six hours of its expiry is as precise as this needs to
/// be, and the sweep is a couple of indexed DELETEs.
const SWEEP_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Let the server finish starting before the first sweep, the same courtesy
/// the backup task extends.
const STARTUP_DELAY: Duration = Duration::from_secs(30);

/// Start the trash sweep. Returns `None` when `trash_days` is `0`, which means
/// the operator asked for tombstones to be kept indefinitely: no task is
/// spawned at all rather than one that wakes up to do nothing.
pub fn start_trash_purge_task(
    pool: DbPool,
    trash_days: u32,
) -> Option<tokio::task::JoinHandle<()>> {
    if trash_days == 0 {
        info!("[retention] trash_days = 0; soft-deleted rows are kept until removed by hand");
        return None;
    }

    Some(tokio::spawn(async move {
        info!(trash_days, "trash purge task started");
        tokio::time::sleep(STARTUP_DELAY).await;
        run_purge_blocking(&pool, trash_days).await;

        let mut interval = tokio::time::interval(SWEEP_INTERVAL);
        interval.tick().await; // skip the immediate tick
        loop {
            interval.tick().await;
            run_purge_blocking(&pool, trash_days).await;
        }
    }))
}

/// One sweep on the blocking pool, awaited.
///
/// `purge_tombstones` is synchronous SQLite work that takes the exclusive
/// write connection and can wait behind a backup. Running it inline would park
/// a Tokio worker — one of the threads serving HTTP — for that whole time.
async fn run_purge_blocking(pool: &DbPool, trash_days: u32) {
    let pool = pool.clone();
    if let Err(e) = tokio::task::spawn_blocking(move || purge_once(&pool, trash_days)).await {
        // A panic here must not take the scheduling loop with it: the next
        // interval should still try.
        error!(error = %e, "trash purge failed to run to completion");
    }
}

fn purge_once(pool: &DbPool, trash_days: u32) {
    match pool.transaction(|conn| trash::purge_tombstones(conn, trash_days)) {
        Ok(counts) if counts.is_empty() => {}
        Ok(counts) => info!(
            issues = counts.issues,
            pages = counts.pages,
            comments = counts.comments,
            trash_days,
            "purged expired tombstones"
        ),
        Err(e) => warn!(error = %e, "trash purge failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::models::*;
    use crate::db::queries;

    fn seeded() -> (db::DbPool, i64) {
        let pool = db::open_memory().expect("test db");
        let project_id = {
            let conn = pool.write().unwrap();
            queries::create_project(
                &conn,
                &CreateProject {
                    name: "Sweep".into(),
                    identifier: "SWP".into(),
                    ..Default::default()
                },
            )
            .unwrap()
            .id
        };
        (pool, project_id)
    }

    fn doomed_issue(pool: &db::DbPool, project_id: i64, age_days: i64) -> i64 {
        let conn = pool.write().unwrap();
        let issue = queries::create_issue(
            &conn,
            &CreateIssue {
                project_id,
                title: "Doomed".into(),
                ..Default::default()
            },
        )
        .unwrap();
        queries::delete_issue(&conn, issue.id).unwrap();
        conn.execute(
            &format!(
                "UPDATE issues SET deleted_at = datetime('now', '-{age_days} days') WHERE id = ?1"
            ),
            rusqlite::params![issue.id],
        )
        .unwrap();
        issue.id
    }

    fn issue_rows(pool: &db::DbPool, id: i64) -> i64 {
        let conn = pool.read().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM issues WHERE id = ?1",
            rusqlite::params![id],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn one_sweep_collects_expired_tombstones() {
        let (pool, pid) = seeded();
        let id = doomed_issue(&pool, pid, 90);
        purge_once(&pool, 30);
        assert_eq!(issue_rows(&pool, id), 0);
    }

    #[test]
    fn one_sweep_leaves_fresh_tombstones_alone() {
        let (pool, pid) = seeded();
        let id = doomed_issue(&pool, pid, 2);
        purge_once(&pool, 30);
        assert_eq!(issue_rows(&pool, id), 1);
    }

    /// `trash_days = 0` must not spawn a task at all — a scheduler that wakes
    /// four times a day to purge nothing is pure noise, and the sweep it would
    /// call is a no-op anyway.
    #[tokio::test]
    async fn zero_trash_days_spawns_no_task() {
        let (pool, _) = seeded();
        assert!(start_trash_purge_task(pool, 0).is_none());
    }
}
