//! LIF-507: the Home dashboard's cross-project aggregates, in one read.
//!
//! Home used to assemble everything from capped list calls. These numbers
//! are counts over every open issue the caller can see, so a 500-row page
//! would quietly undercount them; they are computed here instead, over
//! `issues`, `issue_assignees`, `issue_relations`, `issue_waits` and
//! `audit_log`, with the caller's project filter applied in Rust.
//!
//! "Agent" means a bot account (`users.is_bot`). A person's own MCP key is
//! a person.
//!
//! Days are the caller's local days: the client sends its offset from UTC
//! and every per-day series shifts stored UTC timestamps by it.

use std::collections::{HashMap, HashSet};

use chrono::{Duration, NaiveDate, NaiveDateTime};
use rusqlite::{Connection, params};
use serde::Serialize;

use crate::error::LificError;

/// Days in the open-work trend, ending today.
pub const TREND_DAYS: i64 = 90;
/// Days in the caller's closure heatmap: twelve weeks, ending today.
pub const HEATMAP_DAYS: i64 = 84;
/// Widest real UTC offset is +14:00, so this bounds a client's `tz`.
const MAX_TZ_MINUTES: i32 = 14 * 60;

/// Age buckets, youngest first: key and exclusive upper bound in days.
const AGE_BUCKETS: [(&str, Option<i64>); 5] = [
    ("week", Some(7)),
    ("month", Some(28)),
    ("quarter", Some(91)),
    ("half", Some(182)),
    ("older", None),
];

#[derive(Debug, Serialize)]
pub struct HomeOverview {
    /// Open (not done or cancelled) issues across visible projects.
    pub open_total: i64,
    /// Open issues by days since created, youngest bucket first.
    pub age_buckets: Vec<AgeBucket>,
    /// Open issues at the end of each of the last [`TREND_DAYS`] + 1 days.
    pub open_trend: Vec<DayCount>,
    /// Open count and most recent audit entry per project that has either.
    pub projects: Vec<ProjectPulse>,
    /// What agents and people opened and closed after `since`. Absent when
    /// the request named no `since`.
    pub since: Option<SinceSummary>,
    /// Issues the caller or one of their bots moved to done, per day.
    pub my_done: Vec<DayCount>,
}

#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub struct AgeBucket {
    /// `week`, `month`, `quarter`, `half`, or `older`.
    pub key: &'static str,
    pub total: i64,
    pub urgent: i64,
    pub high: i64,
    pub medium: i64,
    pub low: i64,
    pub none: i64,
    /// Unassigned, unblocked, and not already active: what an agent can
    /// pick up.
    pub agent_ready: i64,
    /// Assigned to named people or marked for any person.
    pub needs_human: i64,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct DayCount {
    /// Local `YYYY-MM-DD`.
    pub date: String,
    pub count: i64,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ProjectPulse {
    pub project_id: i64,
    pub open: i64,
    /// UTC timestamp of the project's latest audit entry.
    pub last_activity: Option<String>,
}

#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub struct SinceSummary {
    pub agents_opened: i64,
    pub agents_closed: i64,
    pub people_opened: i64,
    pub people_closed: i64,
}

pub struct OverviewInput<'a> {
    /// The caller's project filter (`None` = every project).
    pub visible: Option<&'a HashSet<i64>>,
    /// The caller, for the closure heatmap. `None` leaves it all zeros.
    pub user_id: Option<i64>,
    /// Count opens and closes after this instant (ISO 8601, UTC).
    pub since: Option<&'a str>,
    /// The caller's offset from UTC in minutes, east positive.
    pub tz_minutes: i32,
    /// Current UTC time, injected so tests can pin it.
    pub now: NaiveDateTime,
}

pub fn overview(conn: &Connection, input: &OverviewInput) -> Result<HomeOverview, LificError> {
    let tz = input.tz_minutes.clamp(-MAX_TZ_MINUTES, MAX_TZ_MINUTES);
    let today = (input.now + Duration::minutes(tz as i64)).date();
    let visible = |pid: Option<i64>| match (input.visible, pid) {
        (None, _) => true,
        (Some(set), Some(pid)) => set.contains(&pid),
        (Some(_), None) => false,
    };

    let mut buckets: Vec<AgeBucket> = AGE_BUCKETS
        .iter()
        .map(|(key, _)| AgeBucket {
            key,
            ..Default::default()
        })
        .collect();
    let mut open_by_project: HashMap<i64, i64> = HashMap::new();
    let mut open_total = 0;

    let today_text = super::waits::today_text();
    let mut stmt = conn.prepare(&format!(
        "SELECT i.project_id, i.priority, i.status, i.created_at,
                EXISTS (SELECT 1 FROM issue_assignees a WHERE a.issue_id = i.id),
                EXISTS (SELECT 1 FROM issue_relations ir
                          JOIN issues b ON b.id = ir.source_id
                         WHERE ir.target_id = i.id AND ir.relation_type = 'blocks'
                           AND b.status != 'done' AND b.deleted_at IS NULL)
                OR EXISTS (SELECT 1 FROM issue_waits w
                            WHERE w.issue_id = i.id AND {})
           FROM issues i
          WHERE i.deleted_at IS NULL AND i.status NOT IN ('done', 'cancelled')",
        super::waits::holding_predicate("?1")
    ))?;
    let rows = stmt.query_map(params![today_text], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, bool>(4)?,
            row.get::<_, bool>(5)?,
        ))
    })?;
    for row in rows {
        let (project_id, priority, status, created_at, needs_human, blocked) = row?;
        if !visible(Some(project_id)) {
            continue;
        }
        open_total += 1;
        *open_by_project.entry(project_id).or_default() += 1;

        let age =
            parse_ts(&created_at).map_or(0, |created| (input.now - created).num_days().max(0));
        let index = AGE_BUCKETS
            .iter()
            .position(|(_, bound)| bound.is_none_or(|limit| age < limit))
            .unwrap_or(AGE_BUCKETS.len() - 1);
        let bucket = &mut buckets[index];
        bucket.total += 1;
        match priority.as_str() {
            "urgent" => bucket.urgent += 1,
            "high" => bucket.high += 1,
            "medium" => bucket.medium += 1,
            "low" => bucket.low += 1,
            _ => bucket.none += 1,
        }
        if needs_human {
            bucket.needs_human += 1;
        } else if !blocked && status != "active" {
            bucket.agent_ready += 1;
        }
    }

    Ok(HomeOverview {
        open_total,
        age_buckets: buckets,
        open_trend: open_trend(conn, &visible, input.now, tz, today)?,
        projects: project_pulse(conn, &visible, open_by_project)?,
        since: match input.since {
            Some(since) => Some(since_summary(conn, &visible, since)?),
            None => None,
        },
        my_done: my_done(conn, &visible, input.user_id, tz, today)?,
    })
}

/// Stored timestamps are `YYYY-MM-DD HH:MM:SS` (UTC), sometimes with
/// fractional seconds.
fn parse_ts(ts: &str) -> Option<NaiveDateTime> {
    let head = ts.get(..19).unwrap_or(ts).replace('T', " ");
    NaiveDateTime::parse_from_str(&head, "%Y-%m-%d %H:%M:%S").ok()
}

/// SQLite date modifier shifting a UTC timestamp to the caller's day.
fn tz_modifier(tz: i32) -> String {
    format!("{tz:+} minutes")
}

/// Open count at the end of each local day. An issue is open from its
/// creation until the latest status change that closed it, which is the
/// same "latest transition" reading `queries::insights` uses: an issue
/// reopened since is open now and was counted closed nowhere. A closed
/// issue with no status history (imported that way) closes at its last
/// update.
fn open_trend(
    conn: &Connection,
    visible: &dyn Fn(Option<i64>) -> bool,
    now: NaiveDateTime,
    tz: i32,
    today: NaiveDate,
) -> Result<Vec<DayCount>, LificError> {
    let mut stmt = conn.prepare(
        "SELECT i.project_id, i.created_at,
                CASE WHEN i.status IN ('done', 'cancelled') THEN
                    COALESCE((SELECT MAX(a.ts) FROM audit_log a
                               WHERE a.entity_type = 'issue' AND a.entity_id = i.id
                                 AND a.field = 'status'),
                             i.updated_at)
                END
           FROM issues i
          WHERE i.deleted_at IS NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut spans: Vec<(NaiveDateTime, Option<NaiveDateTime>)> = Vec::new();
    for row in rows {
        let (project_id, created, closed) = row?;
        if !visible(Some(project_id)) {
            continue;
        }
        let Some(created) = parse_ts(&created) else {
            continue;
        };
        spans.push((created, closed.as_deref().and_then(parse_ts)));
    }

    let offset = Duration::minutes(tz as i64);
    Ok((0..=TREND_DAYS)
        .rev()
        .map(|back| {
            let day = today - Duration::days(back);
            // The local day's end, in UTC, capped at now for today.
            let end = (day + Duration::days(1)).and_hms_opt(0, 0, 0).unwrap() - offset;
            let end = end.min(now);
            let count = spans
                .iter()
                .filter(|(created, closed)| {
                    *created <= end && closed.is_none_or(|closed| closed > end)
                })
                .count() as i64;
            DayCount {
                date: day.format("%Y-%m-%d").to_string(),
                count,
            }
        })
        .collect())
}

fn project_pulse(
    conn: &Connection,
    visible: &dyn Fn(Option<i64>) -> bool,
    mut open: HashMap<i64, i64>,
) -> Result<Vec<ProjectPulse>, LificError> {
    // MAX(id) per project walks idx_audit_project instead of the table.
    let mut stmt = conn.prepare(
        "SELECT a.project_id, a.ts FROM audit_log a
          WHERE a.id IN (SELECT MAX(id) FROM audit_log
                          WHERE project_id IS NOT NULL GROUP BY project_id)",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (project_id, ts) = row?;
        if !visible(Some(project_id)) {
            continue;
        }
        out.push(ProjectPulse {
            project_id,
            open: open.remove(&project_id).unwrap_or(0),
            last_activity: Some(ts),
        });
    }
    out.extend(open.into_iter().map(|(project_id, open)| ProjectPulse {
        project_id,
        open,
        last_activity: None,
    }));
    out.sort_by_key(|p| p.project_id);
    Ok(out)
}

fn since_summary(
    conn: &Connection,
    visible: &dyn Fn(Option<i64>) -> bool,
    since: &str,
) -> Result<SinceSummary, LificError> {
    let since = parse_ts(since)
        .ok_or_else(|| LificError::BadRequest(format!("invalid since '{since}'")))?
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let mut stmt = conn.prepare(
        "SELECT a.project_id, COALESCE(u.is_bot, 0),
                a.action = 'create', COUNT(*)
           FROM audit_log a LEFT JOIN users u ON u.id = a.actor_user_id
          WHERE a.ts > ?1 AND a.entity_type = 'issue'
            AND (a.action = 'create'
                 OR (a.field = 'status' AND a.new_value IN ('done', 'cancelled')))
          GROUP BY a.project_id, 2, 3",
    )?;
    let rows = stmt.query_map(params![since], |row| {
        Ok((
            row.get::<_, Option<i64>>(0)?,
            row.get::<_, bool>(1)?,
            row.get::<_, bool>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    let mut out = SinceSummary::default();
    for row in rows {
        let (project_id, is_bot, created, n) = row?;
        if !visible(project_id) {
            continue;
        }
        let slot = match (is_bot, created) {
            (true, true) => &mut out.agents_opened,
            (true, false) => &mut out.agents_closed,
            (false, true) => &mut out.people_opened,
            (false, false) => &mut out.people_closed,
        };
        *slot += n;
    }
    Ok(out)
}

fn my_done(
    conn: &Connection,
    visible: &dyn Fn(Option<i64>) -> bool,
    user_id: Option<i64>,
    tz: i32,
    today: NaiveDate,
) -> Result<Vec<DayCount>, LificError> {
    let first = today - Duration::days(HEATMAP_DAYS - 1);
    let mut counts: HashMap<String, i64> = HashMap::new();
    if let Some(user_id) = user_id {
        // A day early in UTC, so every local day in range is covered.
        let floor = (first - Duration::days(1)).format("%Y-%m-%d").to_string();
        let mut stmt = conn.prepare(
            "SELECT a.project_id, date(a.ts, ?3) AS day, COUNT(*)
               FROM audit_log a
              WHERE a.ts >= ?2 AND a.entity_type = 'issue'
                AND a.field = 'status' AND a.new_value = 'done'
                AND a.actor_user_id IN (SELECT ?1 UNION ALL
                                        SELECT id FROM users WHERE is_bot = 1 AND owner_id = ?1)
              GROUP BY a.project_id, day",
        )?;
        let rows = stmt.query_map(params![user_id, floor, tz_modifier(tz)], |row| {
            Ok((
                row.get::<_, Option<i64>>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (project_id, day, n) = row?;
            if visible(project_id) {
                *counts.entry(day).or_default() += n;
            }
        }
    }
    Ok((0..HEATMAP_DAYS)
        .map(|k| {
            let date = (first + Duration::days(k)).format("%Y-%m-%d").to_string();
            let count = counts.get(&date).copied().unwrap_or(0);
            DayCount { date, count }
        })
        .collect())
}

#[cfg(test)]
mod tests;
