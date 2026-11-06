//! Nightly return of long-idle active issues to todo (LIF-494).
//!
//! `active` says someone is working on an issue, and nothing withdraws that
//! claim when the session that made it ends. The briefing flags such issues
//! after a few days (LIF-492); this job ends the claim after longer, so every
//! later reader stops treating abandoned work as in flight. Each moved issue
//! gets a comment saying why, and the writes are audited as `system`.
//!
//! Idle uses the same predicate as `list_issues(stale=true)`: active, with
//! `updated_at` before a cutoff (see `db::queries::stale`). The threshold is
//! the `stale_active_reset_days` instance setting, read on every run so an
//! admin's change applies without a restart; `0` turns the job off.
//!
//! Shape copied from `retention`: settle, run once at startup, then on an
//! interval, with the SQLite work on the blocking pool.

use std::time::Duration;

use chrono::{DateTime, Utc};
use tracing::{error, info, warn};

use crate::actor::{ActorCtx, Transport};
use crate::db::DbPool;
use crate::db::models::{ListIssuesQuery, Status, UpdateIssue};
use crate::db::queries;
use crate::error::LificError;

/// Once a day: the threshold is counted in days, so a day late is the most
/// any issue can overstay.
const RUN_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Let the server finish starting first, as the backup and trash tasks do.
const STARTUP_DELAY: Duration = Duration::from_secs(45);

/// Issues read per pass. Moved issues leave the idle set, so passes repeat
/// from the start until nothing idle is left.
const BATCH: i64 = 500;

pub fn start_task(pool: DbPool) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tokio::time::sleep(STARTUP_DELAY).await;
        loop {
            run_blocking(&pool).await;
            tokio::time::sleep(RUN_INTERVAL).await;
        }
    })
}

async fn run_blocking(pool: &DbPool) {
    let pool = pool.clone();
    if let Err(error) = tokio::task::spawn_blocking(move || run_once(&pool)).await {
        error!(%error, "stale active reset failed to run to completion");
    }
}

fn run_once(pool: &DbPool) {
    let days = match pool.read().and_then(|conn| queries::settings::get(&conn)) {
        Ok(settings) => settings.stale_active_reset_days,
        Err(error) => {
            warn!(%error, "could not read settings for the stale active reset");
            return;
        }
    };
    if days == 0 {
        return;
    }
    match reset_idle(pool, Utc::now(), days) {
        Ok(moved) if moved.is_empty() => {}
        Ok(moved) => info!(
            count = moved.len(),
            issues = %moved.join(", "),
            days,
            "returned idle active issues to todo"
        ),
        Err(error) => warn!(%error, "stale active reset failed"),
    }
}

/// Move every active issue idle at least `days` days as of `now` back to todo,
/// each with a comment, in one transaction. Returns the moved identifiers.
pub fn reset_idle(pool: &DbPool, now: DateTime<Utc>, days: i64) -> Result<Vec<String>, LificError> {
    let cutoff = queries::stale::cutoff(now, days);
    pool.transaction(|tx| {
        // A background task has no request actor; say so explicitly rather
        // than relying on the process default.
        crate::actor::stamp(
            tx,
            &ActorCtx {
                user_id: None,
                transport: Transport::System,
            },
        );
        let mut moved = Vec::new();
        loop {
            let page = queries::list_issues_page(
                tx,
                &ListIssuesQuery {
                    status: Some(Status::Active),
                    updated_until: Some(cutoff.clone()),
                    order_by: Some("updated".into()),
                    limit: Some(BATCH),
                    ..Default::default()
                },
            )?;
            for issue in &page.items {
                let idle_days = queries::stale::idle_for(&issue.updated_at, now)
                    .map_or(days, |idle| idle.num_days());
                queries::update_issue(
                    tx,
                    issue.id,
                    &UpdateIssue {
                        status: Some(Status::Todo),
                        ..Default::default()
                    },
                )?;
                queries::comments::create_system_comment(
                    tx,
                    issue.id,
                    &format!(
                        "Returned to todo: this issue was active with no activity for {idle_days} days \
                         (the instance limit is {days}). Set it active again when work resumes."
                    ),
                )?;
                moved.push(issue.identifier.clone());
            }
            if !page.has_more || page.items.is_empty() {
                return Ok(moved);
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{CreateIssue, CreateProject, Priority};
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 6, 3, 0, 0).unwrap()
    }

    /// One project with issues pinned to fixed `updated_at` values. Moving
    /// `seq` in the same statement keeps the bump triggers from undoing it.
    fn seed(pool: &DbPool, issues: &[(&str, Status, &str)]) {
        let conn = pool.write().unwrap();
        let project = queries::create_project(
            &conn,
            &CreateProject {
                name: "Reset".into(),
                identifier: "RST".into(),
                ..Default::default()
            },
        )
        .unwrap();
        for (title, status, updated_at) in issues {
            let issue = queries::create_issue(
                &conn,
                &CreateIssue {
                    project_id: project.id,
                    title: (*title).into(),
                    status: *status,
                    priority: Priority::None,
                    ..Default::default()
                },
            )
            .unwrap();
            conn.execute(
                "UPDATE issues SET updated_at = ?1, seq = seq + 1000000 WHERE id = ?2",
                rusqlite::params![updated_at, issue.id],
            )
            .unwrap();
        }
    }

    fn status_of(pool: &DbPool, identifier: &str) -> String {
        let conn = pool.read().unwrap();
        let id = queries::resolve_identifier(&conn, identifier).unwrap();
        queries::get_issue(&conn, id)
            .unwrap()
            .status
            .as_str()
            .to_string()
    }

    #[test]
    fn long_idle_active_issues_return_to_todo_with_a_system_comment() {
        let pool = crate::db::open_memory().unwrap();
        seed(
            &pool,
            &[
                ("Abandoned", Status::Active, "2026-10-01 03:00:00"),
                (
                    "Just inside the limit",
                    Status::Active,
                    "2026-10-23 03:00:01",
                ),
                ("Old todo", Status::Todo, "2026-01-01 00:00:00"),
            ],
        );

        let moved = reset_idle(&pool, now(), 14).unwrap();
        assert_eq!(moved, ["RST-1"]);
        assert_eq!(status_of(&pool, "RST-1"), "todo");
        assert_eq!(status_of(&pool, "RST-2"), "active");
        assert_eq!(status_of(&pool, "RST-3"), "todo");

        let conn = pool.read().unwrap();
        let (author, user_id, content): (String, Option<i64>, String) = conn
            .query_row(
                "SELECT imported_author, user_id, content FROM comments WHERE issue_id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(author, "Lific");
        assert_eq!(user_id, None);
        assert!(
            content.contains("no activity for 36 days (the instance limit is 14)"),
            "{content}"
        );

        // The status change is in the audit log as the system's, not a user's.
        let (transport, actor): (String, Option<i64>) = conn
            .query_row(
                "SELECT transport, actor_user_id FROM audit_log
                 WHERE entity_type = 'issue' AND entity_id = 1 AND field = 'status'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((transport.as_str(), actor), ("system", None));

        // The move itself is activity, so the next run leaves it alone.
        drop(conn);
        assert!(reset_idle(&pool, now(), 14).unwrap().is_empty());
    }

    #[test]
    fn every_idle_issue_moves_even_past_one_batch() {
        let pool = crate::db::open_memory().unwrap();
        let rows: Vec<(String, Status, &str)> = (0..BATCH + 3)
            .map(|n| (format!("Idle {n}"), Status::Active, "2026-01-01 00:00:00"))
            .collect();
        let rows: Vec<(&str, Status, &str)> = rows
            .iter()
            .map(|(title, status, at)| (title.as_str(), *status, *at))
            .collect();
        seed(&pool, &rows);
        assert_eq!(reset_idle(&pool, now(), 14).unwrap().len(), rows.len());
    }
}
