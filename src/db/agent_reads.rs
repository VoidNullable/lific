//! LIF-541: read telemetry for the Agent activity page.
//!
//! Writes already leave a trail in `audit_log`; reads leave none, so an agent
//! that spends an hour reading looks idle. Every successful MCP read tool
//! call is recorded here as one `agent_reads` row (who, which tool, which
//! project and entity).
//!
//! Reads are far more frequent than writes and must stay cheap, so a read
//! never touches the database. [`DbPool::record_agent_read`] appends to an
//! in-memory buffer, and [`DbPool::flush_agent_reads`] drains it into the
//! table in one transaction. A background task flushes every few seconds
//! ([`start_flush_task`]), the activity endpoint flushes before it reads, and
//! shutdown flushes once more. A crash loses at most those few seconds of
//! rows, which is acceptable: this is non-authoritative telemetry, not
//! history anything else depends on. The buffer is capped and drops its
//! oldest rows when full, so a stalled writer cannot grow memory without
//! bound.
//!
//! Rows are kept for [`RETENTION_DAYS`]: the widest window the page shows is
//! 7 days and its roster looks back 30, so nothing older is ever read. The
//! sweep lives in `crate::retention`.

use std::collections::VecDeque;
use std::time::Duration;

use rusqlite::{Connection, TransactionBehavior, params};
use tracing::warn;

use super::DbPool;
use crate::error::LificError;

/// Most rows the buffer holds between flushes. At one flush every five
/// seconds this is far beyond any real read rate; it only matters when the
/// writer is stuck, and then the oldest rows go first.
pub const BUFFER_CAP: usize = 10_000;

/// How long `agent_reads` rows are kept.
pub const RETENTION_DAYS: i64 = 30;

/// How often the background task drains the buffer.
pub const FLUSH_INTERVAL: Duration = Duration::from_secs(5);

/// Timestamp format shared with `audit_log.ts`.
const TS_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

/// One recorded read, as it will be inserted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRead {
    /// UTC, `YYYY-MM-DD HH:MM:SS`, taken when the read was recorded.
    pub ts: String,
    pub user_id: i64,
    pub tool: &'static str,
    pub project_id: Option<i64>,
    pub entity_type: Option<&'static str>,
    pub entity_id: Option<i64>,
    pub entity_label: Option<String>,
}

/// The pending rows, shared by every clone of a pool.
pub(super) type ReadBuffer = std::sync::Arc<std::sync::Mutex<VecDeque<AgentRead>>>;

pub(super) fn new_buffer() -> ReadBuffer {
    std::sync::Arc::new(std::sync::Mutex::new(VecDeque::new()))
}

/// The current UTC time in the stored format.
pub fn now_ts() -> String {
    chrono::Utc::now().naive_utc().format(TS_FORMAT).to_string()
}

impl DbPool {
    /// Queue one read for the next flush. Never blocks on the database.
    pub fn record_agent_read(&self, read: AgentRead) {
        let mut buffer = self
            .agent_reads
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if buffer.len() >= BUFFER_CAP {
            buffer.pop_front();
        }
        buffer.push_back(read);
    }

    /// Insert every queued read in one transaction and return how many were
    /// written. An empty buffer costs nothing and takes no lock on the
    /// writer. On failure the rows go back to the front of the buffer (still
    /// under the cap) so the next flush can retry them.
    pub fn flush_agent_reads(&self) -> Result<usize, LificError> {
        let pending: Vec<AgentRead> = {
            let mut buffer = self
                .agent_reads
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            buffer.drain(..).collect()
        };
        if pending.is_empty() {
            return Ok(0);
        }
        match self.insert_agent_reads(&pending) {
            Ok(()) => Ok(pending.len()),
            Err(error) => {
                let mut buffer = self
                    .agent_reads
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                for read in pending.into_iter().rev() {
                    if buffer.len() >= BUFFER_CAP {
                        break;
                    }
                    buffer.push_front(read);
                }
                Err(error)
            }
        }
    }

    fn insert_agent_reads(&self, reads: &[AgentRead]) -> Result<(), LificError> {
        // The raw writer, not `write()`: these rows carry their own user and
        // fire no audit triggers, so there is no actor to stamp.
        let mut connection = self.lock_writer()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut stmt = transaction.prepare_cached(
                "INSERT INTO agent_reads
                    (ts, user_id, tool, project_id, entity_type, entity_id, entity_label)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for read in reads {
                stmt.execute(params![
                    read.ts,
                    read.user_id,
                    read.tool,
                    read.project_id,
                    read.entity_type,
                    read.entity_id,
                    read.entity_label,
                ])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    /// Rows waiting for the next flush.
    #[cfg(test)]
    pub fn pending_agent_reads(&self) -> usize {
        self.agent_reads
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }
}

/// Delete reads older than [`RETENTION_DAYS`] before `now`. Returns the
/// number of rows removed.
pub fn prune(conn: &Connection, now: chrono::NaiveDateTime) -> Result<usize, LificError> {
    let cutoff = (now - chrono::Duration::days(RETENTION_DAYS))
        .format(TS_FORMAT)
        .to_string();
    Ok(conn.execute("DELETE FROM agent_reads WHERE ts < ?1", params![cutoff])?)
}

/// Drain the buffer every [`FLUSH_INTERVAL`]. The insert is synchronous
/// SQLite work on the single writer, so it runs on the blocking pool.
pub fn start_flush_task(pool: DbPool) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(FLUSH_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval.tick().await; // skip the immediate tick
        loop {
            interval.tick().await;
            flush_blocking(&pool).await;
        }
    })
}

/// One flush on the blocking pool, awaited. Failures are logged, not fatal:
/// the rows stay queued for the next attempt.
pub async fn flush_blocking(pool: &DbPool) {
    let pool = pool.clone();
    match tokio::task::spawn_blocking(move || pool.flush_agent_reads()).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => warn!(error = %error, "agent read flush failed"),
        Err(error) => warn!(error = %error, "agent read flush did not run to completion"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(user_id: i64, tool: &'static str) -> AgentRead {
        AgentRead {
            ts: now_ts(),
            user_id,
            tool,
            project_id: None,
            entity_type: None,
            entity_id: None,
            entity_label: None,
        }
    }

    fn row_count(pool: &DbPool) -> i64 {
        pool.read()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM agent_reads", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn flush_writes_every_queued_read_and_empties_the_buffer() {
        let pool = crate::db::open_memory().unwrap();
        pool.record_agent_read(read(1, "search"));
        pool.record_agent_read(read(2, "get_issue"));
        assert_eq!(pool.flush_agent_reads().unwrap(), 2);
        assert_eq!(pool.pending_agent_reads(), 0);
        assert_eq!(row_count(&pool), 2);
        assert_eq!(pool.flush_agent_reads().unwrap(), 0);
    }

    #[test]
    fn a_full_buffer_drops_its_oldest_rows() {
        let pool = crate::db::open_memory().unwrap();
        for user_id in 0..(BUFFER_CAP as i64 + 5) {
            pool.record_agent_read(read(user_id, "search"));
        }
        assert_eq!(pool.pending_agent_reads(), BUFFER_CAP);
        pool.flush_agent_reads().unwrap();
        let oldest: i64 = pool
            .read()
            .unwrap()
            .query_row("SELECT MIN(user_id) FROM agent_reads", [], |row| row.get(0))
            .unwrap();
        assert_eq!(oldest, 5);
    }

    #[test]
    fn prune_removes_only_rows_past_the_retention_horizon() {
        let pool = crate::db::open_memory().unwrap();
        let now = chrono::NaiveDate::from_ymd_opt(2026, 11, 12)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        {
            let conn = pool.write().unwrap();
            for ts in [
                "2026-10-13 11:59:59",
                "2026-10-13 12:00:00",
                "2026-11-12 11:00:00",
            ] {
                conn.execute(
                    "INSERT INTO agent_reads (ts, user_id, tool) VALUES (?1, 1, 'search')",
                    params![ts],
                )
                .unwrap();
            }
            assert_eq!(prune(&conn, now).unwrap(), 1);
        }
        assert_eq!(row_count(&pool), 2);
    }
}
