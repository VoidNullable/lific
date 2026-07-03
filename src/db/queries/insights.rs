//! LIF-240: per-project analytics ("Insights" tab). Read-only aggregation
//! over `issues` + `audit_log` — no new tables, no writes.
//!
//! ── Week bucketing ──────────────────────────────────────────────────
//! Every trend line buckets by ISO week start (Monday). SQL and Rust must
//! agree on where a week begins, so both sides use the same definition:
//! `date(ts, 'weekday 0', '-6 days')` in SQL, mirrored in Rust by
//! `NaiveDate::weekday().num_days_from_monday()`. Whatever day `ts` falls
//! on, that expression walks forward to the next Sunday (or stays put if
//! `ts` already *is* a Sunday) and then steps back 6 days — landing on the
//! Monday that starts `ts`'s week, both for weekday and Sunday inputs.
//! `week_starts()` produces the same set of Mondays independently so
//! per-week buckets from the SQL query line up with the dense Vec the
//! frontend renders (including weeks with zero activity).
//!
//! ── Closure-counting semantics ──────────────────────────────────────
//! `closed_per_week` counts issue closures once per issue: for each issue,
//! find its most recent `field = 'status'` audit row, and count it toward
//! `closed_per_week` only if that latest transition landed on `done` or
//! `cancelled`. This means:
//!   - An issue closed once: counted once, on its close date.
//!   - An issue closed, reopened, and closed again: counted once, on the
//!     *second* close date (the first close is superseded — the latest
//!     transition is what determines "is it currently closed").
//!   - An issue closed and then reopened (currently open): not counted at
//!     all, in any week — it isn't currently closed.
//!
//! This avoids the double-counting the reopen case would otherwise cause,
//! at the cost of not being a literal "count of close events" — it's a
//! "count of issues whose current state became closed in week W" metric.
//! No join back to `issues.status` is needed: the latest `field='status'`
//! audit row's `new_value` already reflects the issue's current status
//! (or the status it had at deletion, for since-deleted issues — audit
//! rows have no FK and outlive their entity by design, so those still
//! surface here).

use std::collections::HashMap;

use chrono::{Datelike, Duration, NaiveDate, Utc};
use rusqlite::{Connection, params};

use crate::db::models::*;
use crate::error::LificError;

pub const DEFAULT_WEEKS: i64 = 12;
pub const MAX_WEEKS: i64 = 52;

/// Clamp a client-requested week count to `1..=MAX_WEEKS`, defaulting to
/// `DEFAULT_WEEKS` when absent.
pub fn clamp_weeks(weeks: Option<i64>) -> i64 {
    weeks.unwrap_or(DEFAULT_WEEKS).clamp(1, MAX_WEEKS)
}

/// The Monday-aligned week-start dates covering the requested window,
/// oldest first, ending with the week containing "today" (UTC). Always
/// `count` entries long.
fn week_starts(count: i64) -> Vec<NaiveDate> {
    let today = Utc::now().date_naive();
    let this_monday = today - Duration::days(today.weekday().num_days_from_monday() as i64);
    (0..count)
        .rev()
        .map(|k| this_monday - Duration::days(7 * k))
        .collect()
}

/// Run a `(week_bucket TEXT, count INTEGER)` query and fold its rows into a
/// dense `Vec<WeekPoint>` spanning every date in `starts`, defaulting
/// missing weeks to 0. `sql` must select exactly two columns and accept
/// `project_id` as `?1` and the `since` lower-bound (inclusive, `?2`) as a
/// string comparable against the raw `TEXT` timestamp column (works because
/// `datetime('now')`-formatted timestamps sort lexicographically and a bare
/// `YYYY-MM-DD` is a valid prefix-comparable lower bound).
fn bucket_weekly(
    conn: &Connection,
    sql: &str,
    project_id: i64,
    since: &str,
    starts: &[NaiveDate],
) -> Result<Vec<WeekPoint>, LificError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![project_id, since], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut counts: HashMap<String, i64> = HashMap::new();
    for row in rows {
        let (week, n) = row?;
        counts.insert(week, n);
    }
    Ok(starts
        .iter()
        .map(|d| {
            let key = d.format("%Y-%m-%d").to_string();
            let count = counts.get(&key).copied().unwrap_or(0);
            WeekPoint { week_start: key, count }
        })
        .collect())
}

fn created_per_week(
    conn: &Connection,
    project_id: i64,
    since: &str,
    starts: &[NaiveDate],
) -> Result<Vec<WeekPoint>, LificError> {
    bucket_weekly(
        conn,
        "SELECT date(created_at, 'weekday 0', '-6 days') AS wk, COUNT(*)
         FROM issues
         WHERE project_id = ?1 AND created_at >= ?2
         GROUP BY wk",
        project_id,
        since,
        starts,
    )
}

/// See the module doc comment for the "latest transition per issue"
/// semantics this query implements.
fn closed_per_week(
    conn: &Connection,
    project_id: i64,
    since: &str,
    starts: &[NaiveDate],
) -> Result<Vec<WeekPoint>, LificError> {
    bucket_weekly(
        conn,
        "SELECT date(a.ts, 'weekday 0', '-6 days') AS wk, COUNT(*)
         FROM audit_log a
         WHERE a.project_id = ?1
           AND a.entity_type = 'issue'
           AND a.field = 'status'
           AND a.ts >= ?2
           AND a.new_value IN ('done', 'cancelled')
           AND a.id = (
               SELECT MAX(a2.id) FROM audit_log a2
               WHERE a2.entity_type = 'issue'
                 AND a2.entity_id = a.entity_id
                 AND a2.field = 'status'
           )
         GROUP BY wk",
        project_id,
        since,
        starts,
    )
}

fn priority_counts(conn: &Connection, project_id: i64) -> Result<PriorityCounts, LificError> {
    let mut counts = PriorityCounts::default();
    let mut stmt = conn.prepare_cached(
        "SELECT priority, COUNT(*) FROM issues WHERE project_id = ?1 GROUP BY priority",
    )?;
    let rows = stmt.query_map(params![project_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (priority, n) = row?;
        match priority.as_str() {
            "urgent" => counts.urgent = n,
            "high" => counts.high = n,
            "medium" => counts.medium = n,
            "low" => counts.low = n,
            "none" => counts.none = n,
            // Priorities are validated on write, but a hand-edited DB row
            // still counts toward the total (mirrors count_issues_by_status).
            _ => {}
        }
        counts.total += n;
    }
    Ok(counts)
}

fn module_counts(conn: &Connection, project_id: i64) -> Result<Vec<ModuleCount>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT i.module_id, COALESCE(m.name, 'No module'), COUNT(*)
         FROM issues i LEFT JOIN modules m ON m.id = i.module_id
         WHERE i.project_id = ?1
         GROUP BY i.module_id
         ORDER BY COUNT(*) DESC, name ASC",
    )?;
    let rows = stmt.query_map(params![project_id], |row| {
        Ok(ModuleCount {
            module_id: row.get(0)?,
            name: row.get(1)?,
            count: row.get(2)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

const TOP_ACTORS_LIMIT: i64 = 10;

/// Actor rollup scoped to `since..` (unlike `activity::actor_stats`, which
/// is all-time). Query shape mirrors that function; kept separate rather
/// than adding a date-filter parameter to the Activity-tab query so that
/// surface's behavior can't shift as a side effect of this one.
fn top_actors(
    conn: &Connection,
    project_id: i64,
    since: &str,
) -> Result<Vec<ActorStat>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT a.actor_user_id, u.username, u.display_name, COALESCE(u.is_bot, 0),
                COUNT(*) AS actions, MAX(a.ts) AS last_ts,
                (SELECT t.transport FROM audit_log t
                 WHERE t.project_id = a.project_id
                   AND t.actor_user_id IS a.actor_user_id
                   AND t.ts >= ?2
                 GROUP BY t.transport ORDER BY COUNT(*) DESC LIMIT 1) AS top_transport
         FROM audit_log a
         LEFT JOIN users u ON u.id = a.actor_user_id
         WHERE a.project_id = ?1 AND a.ts >= ?2
         GROUP BY a.actor_user_id
         ORDER BY actions DESC
         LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![project_id, since, TOP_ACTORS_LIMIT], |row| {
        Ok(ActorStat {
            actor_user_id: row.get(0)?,
            username: row.get(1)?,
            display_name: row.get(2)?,
            is_bot: row.get::<_, i64>(3)? != 0,
            actions: row.get(4)?,
            last_ts: row.get(5)?,
            top_transport: row.get(6)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(LificError::Database)
}

/// Compute the full Insights payload for a project. `weeks` should already
/// be clamped via `clamp_weeks` — this function trusts its caller.
pub fn get_insights(
    conn: &Connection,
    project_id: i64,
    weeks: i64,
) -> Result<InsightsPayload, LificError> {
    let starts = week_starts(weeks);
    let since = starts[0].format("%Y-%m-%d").to_string();

    Ok(InsightsPayload {
        weeks,
        created_per_week: created_per_week(conn, project_id, &since, &starts)?,
        closed_per_week: closed_per_week(conn, project_id, &since, &starts)?,
        status_counts: super::count_issues_by_status(conn, project_id)?,
        priority_counts: priority_counts(conn, project_id)?,
        module_counts: module_counts(conn, project_id)?,
        top_actors: top_actors(conn, project_id, &since)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::queries;

    fn seeded() -> (crate::db::DbPool, i64) {
        let pool = crate::db::open_memory().expect("test db");
        let project = {
            let conn = pool.write().unwrap();
            queries::create_project(
                &conn,
                &CreateProject {
                    name: "Insights Test".into(),
                    identifier: "INS".into(),
                    description: String::new(),
                    emoji: None,
                    lead_user_id: None,
                },
            )
            .unwrap()
        };
        (pool, project.id)
    }

    fn quick_issue(conn: &Connection, pid: i64, title: &str, priority: &str) -> Issue {
        queries::create_issue(
            conn,
            &CreateIssue {
                project_id: pid,
                title: title.into(),
                description: String::new(),
                status: "backlog".into(),
                priority: priority.into(),
                module_id: None,
                start_date: None,
                target_date: None,
                labels: vec![],
                source: None,
            },
        )
        .unwrap()
    }

    fn no_update() -> UpdateIssue {
        UpdateIssue {
            title: None,
            description: None,
            status: None,
            priority: None,
            module_id: None,
            sort_order: None,
            start_date: None,
            target_date: None,
            labels: None,
        }
    }

    // ── clamp_weeks ───────────────────────────────────────────

    #[test]
    fn clamp_weeks_defaults_and_bounds() {
        assert_eq!(clamp_weeks(None), DEFAULT_WEEKS);
        assert_eq!(clamp_weeks(Some(0)), 1);
        assert_eq!(clamp_weeks(Some(1)), 1);
        assert_eq!(clamp_weeks(Some(4)), 4);
        assert_eq!(clamp_weeks(Some(999)), MAX_WEEKS);
        assert_eq!(clamp_weeks(Some(-5)), 1);
    }

    // ── week_starts ───────────────────────────────────────────

    #[test]
    fn week_starts_are_dense_ascending_mondays() {
        let starts = week_starts(6);
        assert_eq!(starts.len(), 6);
        for w in &starts {
            assert_eq!(w.weekday(), chrono::Weekday::Mon, "{w} must be a Monday");
        }
        for pair in starts.windows(2) {
            assert_eq!(
                (pair[1] - pair[0]).num_days(),
                7,
                "buckets must be exactly one week apart"
            );
        }
        // The last bucket must contain "today".
        let today = Utc::now().date_naive();
        let last = *starts.last().unwrap();
        assert!(today >= last && (today - last).num_days() < 7);
    }

    // ── created_per_week ──────────────────────────────────────

    #[test]
    fn created_per_week_buckets_by_iso_week_and_fills_gaps() {
        let (pool, pid) = seeded();
        let conn = pool.write().unwrap();
        quick_issue(&conn, pid, "A", "none");
        quick_issue(&conn, pid, "B", "none");
        drop(conn);

        let conn = pool.read().unwrap();
        let payload = get_insights(&conn, pid, 4).unwrap();
        assert_eq!(payload.weeks, 4);
        assert_eq!(payload.created_per_week.len(), 4, "dense — one point per week");
        // Every bucket is Monday-aligned.
        for pt in &payload.created_per_week {
            let d = NaiveDate::parse_from_str(&pt.week_start, "%Y-%m-%d").unwrap();
            assert_eq!(d.weekday(), chrono::Weekday::Mon);
        }
        // Both issues were created "now", so the current week's bucket
        // holds both and every other bucket is 0.
        let total: i64 = payload.created_per_week.iter().map(|p| p.count).sum();
        assert_eq!(total, 2);
        assert_eq!(payload.created_per_week.last().unwrap().count, 2);
        assert!(payload.created_per_week[..3].iter().all(|p| p.count == 0));
    }

    // ── closed_per_week semantics ─────────────────────────────

    #[test]
    fn closed_per_week_counts_a_simple_close_once() {
        let (pool, pid) = seeded();
        let conn = pool.write().unwrap();
        let issue = quick_issue(&conn, pid, "Close me", "none");
        queries::update_issue(
            &conn,
            issue.id,
            &UpdateIssue { status: Some("done".into()), ..no_update() },
        )
        .unwrap();
        drop(conn);

        let conn = pool.read().unwrap();
        let payload = get_insights(&conn, pid, 4).unwrap();
        let total: i64 = payload.closed_per_week.iter().map(|p| p.count).sum();
        assert_eq!(total, 1, "one close event must be counted exactly once");
    }

    #[test]
    fn closed_per_week_excludes_reopened_issues() {
        let (pool, pid) = seeded();
        let conn = pool.write().unwrap();
        let issue = quick_issue(&conn, pid, "Reopened", "none");
        queries::update_issue(
            &conn,
            issue.id,
            &UpdateIssue { status: Some("done".into()), ..no_update() },
        )
        .unwrap();
        // Reopen: latest status transition is no longer terminal.
        queries::update_issue(
            &conn,
            issue.id,
            &UpdateIssue { status: Some("todo".into()), ..no_update() },
        )
        .unwrap();
        drop(conn);

        let conn = pool.read().unwrap();
        let payload = get_insights(&conn, pid, 4).unwrap();
        let total: i64 = payload.closed_per_week.iter().map(|p| p.count).sum();
        assert_eq!(total, 0, "currently-open issue must not appear in closed_per_week");
    }

    #[test]
    fn closed_per_week_counts_reclosed_issue_once_not_twice() {
        let (pool, pid) = seeded();
        let conn = pool.write().unwrap();
        let issue = quick_issue(&conn, pid, "Closed twice", "none");
        queries::update_issue(
            &conn,
            issue.id,
            &UpdateIssue { status: Some("done".into()), ..no_update() },
        )
        .unwrap();
        queries::update_issue(
            &conn,
            issue.id,
            &UpdateIssue { status: Some("todo".into()), ..no_update() },
        )
        .unwrap();
        queries::update_issue(
            &conn,
            issue.id,
            &UpdateIssue { status: Some("cancelled".into()), ..no_update() },
        )
        .unwrap();
        drop(conn);

        let conn = pool.read().unwrap();
        let payload = get_insights(&conn, pid, 4).unwrap();
        let total: i64 = payload.closed_per_week.iter().map(|p| p.count).sum();
        assert_eq!(
            total, 1,
            "the superseded first close must not be double-counted alongside the second"
        );
    }

    // ── status / priority / module counts ──────────────────────

    #[test]
    fn status_and_priority_counts_match_seeded_issues() {
        let (pool, pid) = seeded();
        let conn = pool.write().unwrap();
        quick_issue(&conn, pid, "A", "urgent");
        quick_issue(&conn, pid, "B", "high");
        let c = quick_issue(&conn, pid, "C", "none");
        queries::update_issue(
            &conn,
            c.id,
            &UpdateIssue { status: Some("done".into()), ..no_update() },
        )
        .unwrap();
        drop(conn);

        let conn = pool.read().unwrap();
        let payload = get_insights(&conn, pid, 4).unwrap();
        assert_eq!(payload.status_counts.total, 3);
        assert_eq!(payload.status_counts.backlog, 2);
        assert_eq!(payload.status_counts.done, 1);
        assert_eq!(payload.priority_counts.total, 3);
        assert_eq!(payload.priority_counts.urgent, 1);
        assert_eq!(payload.priority_counts.high, 1);
        assert_eq!(payload.priority_counts.none, 1);
    }

    #[test]
    fn module_counts_include_a_no_module_bucket() {
        let (pool, pid) = seeded();
        let conn = pool.write().unwrap();
        let module = queries::create_module(
            &conn,
            &CreateModule {
                project_id: pid,
                name: "Backend".into(),
                description: String::new(),
                status: "active".into(),
                emoji: None,
            },
        )
        .unwrap();
        let a = quick_issue(&conn, pid, "A", "none");
        queries::update_issue(
            &conn,
            a.id,
            &UpdateIssue { module_id: Some(module.id), ..no_update() },
        )
        .unwrap();
        quick_issue(&conn, pid, "B", "none");
        drop(conn);

        let conn = pool.read().unwrap();
        let payload = get_insights(&conn, pid, 4).unwrap();
        assert_eq!(payload.module_counts.len(), 2);
        let backend = payload
            .module_counts
            .iter()
            .find(|m| m.module_id == Some(module.id))
            .unwrap();
        assert_eq!(backend.name, "Backend");
        assert_eq!(backend.count, 1);
        let unassigned = payload.module_counts.iter().find(|m| m.module_id.is_none()).unwrap();
        assert_eq!(unassigned.name, "No module");
        assert_eq!(unassigned.count, 1);
    }

    // ── top_actors ──────────────────────────────────────────────

    #[test]
    fn top_actors_ranks_by_action_count_within_window() {
        let (pool, pid) = seeded();
        let conn = pool.write().unwrap();
        let alice = {
            conn.execute(
                "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot)
                 VALUES ('alice', 'alice@test.local', 'x', 'Alice', 0, 0)",
                [],
            )
            .unwrap();
            conn.last_insert_rowid()
        };
        crate::actor::stamp(
            &conn,
            &crate::actor::ActorCtx { user_id: Some(alice), transport: crate::actor::Transport::Web },
        );
        quick_issue(&conn, pid, "A1", "none");
        quick_issue(&conn, pid, "A2", "none");
        drop(conn);

        let conn = pool.read().unwrap();
        let payload = get_insights(&conn, pid, 4).unwrap();
        assert!(!payload.top_actors.is_empty());
        assert_eq!(payload.top_actors[0].username.as_deref(), Some("alice"));
        assert_eq!(payload.top_actors[0].actions, 2);
    }

    // ── project isolation ───────────────────────────────────────

    #[test]
    fn insights_are_scoped_to_the_requested_project() {
        let (pool, pid) = seeded();
        let other_pid = {
            let conn = pool.write().unwrap();
            queries::create_project(
                &conn,
                &CreateProject {
                    name: "Other".into(),
                    identifier: "OTH".into(),
                    description: String::new(),
                    emoji: None,
                    lead_user_id: None,
                },
            )
            .unwrap()
            .id
        };
        let conn = pool.write().unwrap();
        quick_issue(&conn, pid, "Mine", "none");
        quick_issue(&conn, other_pid, "Not mine", "none");
        quick_issue(&conn, other_pid, "Also not mine", "none");
        drop(conn);

        let conn = pool.read().unwrap();
        let payload = get_insights(&conn, pid, 4).unwrap();
        assert_eq!(payload.status_counts.total, 1);
    }
}
