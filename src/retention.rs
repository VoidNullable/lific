//! Background retention sweep: empties the trash (LIF-438) and expires agent
//! read telemetry (LIF-541).
//!
//! Deleting an issue, page or comment leaves a tombstone: the row stays, with
//! `deleted_at` set, so a replica can sync the deletion and a user can undo
//! it. This task is what makes that bounded. Every `[retention] trash_days`
//! the sweep runs a real DELETE over anything past the window, which is also
//! the only moment the foreign-key cascades fire.
//!
//! The same sweep deletes `agent_reads` rows older than
//! [`agent_reads::RETENTION_DAYS`] and idempotency keys older than
//! [`op_keys::TTL_HOURS`]. Those parts always run: `trash_days = 0` keeps
//! tombstones forever, which says nothing about read telemetry or keys, so
//! the task is spawned either way and only the trash half is skipped.
//!
//! Shape copied from `backup::start_backup_task`: settle, sweep once at
//! startup, then sweep on an interval, with the synchronous SQLite work handed
//! to the blocking pool and awaited so one sweep can never queue behind
//! another.

use std::time::Duration;

use tracing::{error, info, warn};

use crate::db::DbPool;
use crate::db::agent_reads;
use crate::db::queries::{op_keys, trash};

/// How often the sweep runs. Four times a day: a tombstone lives for days, so
/// collecting it within six hours of its expiry is as precise as this needs to
/// be, and the sweep is a couple of indexed DELETEs.
const SWEEP_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Let the server finish starting before the first sweep, the same courtesy
/// the backup task extends.
const STARTUP_DELAY: Duration = Duration::from_secs(30);

/// Start the retention sweep. `trash_days = 0` means the operator asked for
/// tombstones to be kept indefinitely, so the trash half is skipped; the
/// agent read half still runs.
pub fn start_retention_task(pool: DbPool, trash_days: u32) -> tokio::task::JoinHandle<()> {
    if trash_days == 0 {
        info!("[retention] trash_days = 0; soft-deleted rows are kept until removed by hand");
    }

    tokio::spawn(async move {
        info!(trash_days, "retention task started");
        tokio::time::sleep(STARTUP_DELAY).await;
        run_sweep_blocking(&pool, trash_days).await;

        let mut interval = tokio::time::interval(SWEEP_INTERVAL);
        interval.tick().await; // skip the immediate tick
        loop {
            interval.tick().await;
            run_sweep_blocking(&pool, trash_days).await;
        }
    })
}

/// One sweep on the blocking pool, awaited.
///
/// The sweep is synchronous SQLite work that takes the exclusive write
/// connection and can wait behind a backup. Running it inline would park a
/// Tokio worker, one of the threads serving HTTP, for that whole time.
async fn run_sweep_blocking(pool: &DbPool, trash_days: u32) {
    let pool = pool.clone();
    if let Err(e) = tokio::task::spawn_blocking(move || sweep_once(&pool, trash_days)).await {
        // A panic here must not take the scheduling loop with it: the next
        // interval should still try.
        error!(error = %e, "retention sweep failed to run to completion");
    }
}

fn sweep_once(pool: &DbPool, trash_days: u32) {
    let conn = match pool.write() {
        Ok(conn) => conn,
        Err(e) => {
            warn!(error = %e, "could not acquire write connection for retention sweep");
            return;
        }
    };
    if trash_days > 0 {
        match trash::purge_tombstones(&conn, trash_days) {
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
    match agent_reads::prune(&conn, chrono::Utc::now().naive_utc()) {
        Ok(0) => {}
        Ok(rows) => info!(rows, "expired old agent reads"),
        Err(e) => warn!(error = %e, "agent read expiry failed"),
    }
    expire_op_keys(&conn);
}

fn expire_op_keys(conn: &rusqlite::Connection) {
    match op_keys::purge_expired(conn) {
        Ok(0) => {}
        Ok(rows) => info!(rows, "expired old op keys"),
        Err(e) => warn!(error = %e, "op key expiry failed"),
    }
}

/// LIF-526: a stdio `lific mcp` session writes idempotency keys but never
/// runs the server's retention sweep, and it may be the only process that
/// ever opens its database. It expires keys at start and on the sweep's
/// schedule. Only the keys: emptying the trash stays the server's job.
pub fn start_op_key_expiry_task(pool: DbPool) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(SWEEP_INTERVAL);
        loop {
            interval.tick().await;
            let pool = pool.clone();
            let swept = tokio::task::spawn_blocking(move || expire_op_keys_once(&pool)).await;
            if let Err(e) = swept {
                error!(error = %e, "op key expiry failed to run to completion");
            }
        }
    })
}

fn expire_op_keys_once(pool: &DbPool) {
    match pool.write() {
        Ok(conn) => expire_op_keys(&conn),
        Err(e) => warn!(error = %e, "could not acquire write connection for op key expiry"),
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
        sweep_once(&pool, 30);
        assert_eq!(issue_rows(&pool, id), 0);
    }

    #[test]
    fn one_sweep_leaves_fresh_tombstones_alone() {
        let (pool, pid) = seeded();
        let id = doomed_issue(&pool, pid, 2);
        sweep_once(&pool, 30);
        assert_eq!(issue_rows(&pool, id), 1);
    }

    fn agent_read_rows(pool: &db::DbPool) -> Vec<String> {
        let conn = pool.read().unwrap();
        let mut stmt = conn
            .prepare("SELECT tool FROM agent_reads ORDER BY id")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    fn agent_read(pool: &db::DbPool, tool: &str, age_days: i64) {
        let conn = pool.write().unwrap();
        conn.execute(
            &format!(
                "INSERT INTO agent_reads (ts, user_id, tool)
                 VALUES (datetime('now', '-{age_days} days'), 1, ?1)"
            ),
            rusqlite::params![tool],
        )
        .unwrap();
    }

    /// LIF-541: agent reads past the 30-day horizon go on every sweep.
    #[test]
    fn one_sweep_expires_old_agent_reads() {
        let (pool, _) = seeded();
        agent_read(&pool, "old", 31);
        agent_read(&pool, "fresh", 2);
        sweep_once(&pool, 30);
        assert_eq!(agent_read_rows(&pool), ["fresh"]);
    }

    fn op_key_names(pool: &db::DbPool) -> Vec<String> {
        let conn = pool.read().unwrap();
        let mut stmt = conn
            .prepare("SELECT op_key FROM op_keys ORDER BY id")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    fn op_key(pool: &db::DbPool, key: &str, age_hours: i64) {
        let conn = pool.write().unwrap();
        conn.execute(
            &format!(
                "INSERT INTO op_keys (op_key, operation, request_hash, result, created_at)
                 VALUES (?1, 'mcp.create_issue', 'h', 'r', datetime('now', '-{age_hours} hours'))"
            ),
            rusqlite::params![key],
        )
        .unwrap();
    }

    /// LIF-526: idempotency keys live 24 hours; every sweep drops older
    /// ones, whatever `trash_days` says.
    #[test]
    fn one_sweep_expires_op_keys_older_than_a_day() {
        let (pool, _) = seeded();
        op_key(&pool, "old", 25);
        op_key(&pool, "fresh", 1);
        sweep_once(&pool, 0);
        assert_eq!(op_key_names(&pool), ["fresh"]);
    }

    /// The stdio session's expiry: old keys go, fresh keys and the trash stay.
    #[test]
    fn the_stdio_expiry_drops_old_op_keys_and_leaves_the_trash() {
        let (pool, pid) = seeded();
        let id = doomed_issue(&pool, pid, 90);
        op_key(&pool, "old", 25);
        op_key(&pool, "fresh", 1);
        expire_op_keys_once(&pool);
        assert_eq!(op_key_names(&pool), ["fresh"]);
        assert_eq!(issue_rows(&pool, id), 1);
    }

    /// The task runs its first expiry as soon as it starts.
    #[tokio::test]
    async fn the_stdio_expiry_task_runs_at_start() {
        let (pool, _) = seeded();
        op_key(&pool, "old", 25);
        let task = start_op_key_expiry_task(pool.clone());
        for _ in 0..200 {
            if op_key_names(&pool).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        task.abort();
        assert!(op_key_names(&pool).is_empty());
    }

    /// `trash_days = 0` keeps tombstones forever but must not keep agent
    /// reads forever: that setting is about the trash only.
    #[test]
    fn zero_trash_days_keeps_tombstones_but_still_expires_agent_reads() {
        let (pool, pid) = seeded();
        let id = doomed_issue(&pool, pid, 90);
        agent_read(&pool, "old", 31);
        sweep_once(&pool, 0);
        assert_eq!(issue_rows(&pool, id), 1);
        assert!(agent_read_rows(&pool).is_empty());
    }
}
