//! LIF-484: user and date blockers ("waits").
//!
//! An issue can be blocked by another issue (`issue_relations`), by a person
//! until someone clears the wait, or by a window of days. See migration 054
//! for the table and why clearing deletes the row.
//!
//! ## The day rule
//!
//! A date wait is compared against *today*, the server's local calendar day
//! (`chrono::Local`), never UTC: "they said the 28th" means the 28th where
//! the server's operator lives. On a given day a date wait is:
//!
//! * `holding` while today < earliest: it blocks, like an open issue;
//! * `due` from earliest through latest inclusive: it no longer blocks and
//!   is surfaced as "due to check";
//! * `overdue` once today > latest: not blocking, surfaced prominently.
//!
//! A user wait is `holding` until cleared. Only `holding` waits exclude an
//! issue from `workable=true` or include it in `blocked=true`.
//!
//! Stored dates are plain `YYYY-MM-DD`, so the state is computed per read and
//! nothing needs rewriting at midnight. [`today`] is the one clock every
//! read consults; tests pin it per thread with [`pin_today`].

use std::cell::Cell;
use std::collections::HashMap;

use chrono::NaiveDate;
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::db::models::{CreateWait, IssueWait, WaitKind, WaitState};
use crate::error::LificError;

/// Longest note a wait accepts, in characters. A wait's note is a pointer
/// ("state filing office, 2 to 5 business days"), not a description.
pub const MAX_NOTE_CHARS: usize = 500;

thread_local! {
    /// Test override for [`today`]. Every read path runs synchronously on the
    /// thread that called it (REST's `with_read`, MCP's `self.read`), so a
    /// per-thread pin reaches them without a process-wide clock that parallel
    /// tests would fight over.
    static PINNED_TODAY: Cell<Option<NaiveDate>> = const { Cell::new(None) };
}

/// The server's local calendar day, unless a test has pinned one.
pub fn today() -> NaiveDate {
    PINNED_TODAY
        .with(Cell::get)
        .unwrap_or_else(|| chrono::Local::now().date_naive())
}

/// Restores the previous pin when dropped.
#[cfg(test)]
pub(crate) struct PinnedToday(Option<NaiveDate>);

#[cfg(test)]
impl Drop for PinnedToday {
    fn drop(&mut self) {
        PINNED_TODAY.with(|cell| cell.set(self.0));
    }
}

/// Pin [`today`] on this thread for as long as the guard lives.
#[cfg(test)]
pub(crate) fn pin_today(day: &str) -> PinnedToday {
    let day = NaiveDate::parse_from_str(day, "%Y-%m-%d").expect("test day");
    PinnedToday(PINNED_TODAY.with(|cell| cell.replace(Some(day))))
}

/// The server's current offset from UTC in minutes (east positive), read
/// at the same instant as the local day. Clients that decide a wait's state
/// themselves (the web UI across midnight) compute "today" at this offset,
/// so a browser in another timezone agrees with REST and MCP.
pub fn utc_offset_minutes() -> i32 {
    chrono::Local::now().offset().local_minus_utc() / 60
}

/// [`today`] as the `YYYY-MM-DD` text the table stores, for SQL comparison.
pub fn today_text() -> String {
    today().format("%Y-%m-%d").to_string()
}

/// A wait's standing on `today`.
pub fn state_on(
    kind: WaitKind,
    earliest: Option<&str>,
    latest: Option<&str>,
    today: &str,
) -> WaitState {
    match (kind, earliest, latest) {
        (WaitKind::Date, Some(earliest), _) if today < earliest => WaitState::Holding,
        (WaitKind::Date, _, Some(latest)) if today > latest => WaitState::Overdue,
        (WaitKind::Date, _, _) => WaitState::Due,
        (WaitKind::User, _, _) => WaitState::Holding,
    }
}

/// SQL predicate over `issue_waits w`: the wait blocks today. `today` is the
/// placeholder holding [`today_text`].
pub fn holding_predicate(today: &str) -> String {
    format!("(w.kind = 'user' OR w.earliest > {today})")
}

/// The day after `day`, for "overdue since". Falls back to `day` itself on a
/// value that is not a date, which only a hand-edited row could hold.
pub fn day_after(day: &str) -> String {
    NaiveDate::parse_from_str(day, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.succ_opt())
        .map_or_else(|| day.to_string(), |d| d.format("%Y-%m-%d").to_string())
}

/// `2026-09-28`, `2026-09-28..29`, `2026-09-28..10-02` or
/// `2026-12-30..2027-01-02`: the shortest unambiguous window.
pub fn format_window(earliest: &str, latest: &str) -> String {
    if earliest == latest {
        return earliest.to_string();
    }
    let (ey, em) = (earliest.get(..4), earliest.get(5..7));
    let (ly, lm) = (latest.get(..4), latest.get(5..7));
    let tail = match (ey == ly, em == lm) {
        (true, true) => latest.get(8..),
        (true, false) => latest.get(5..),
        _ => None,
    };
    format!("{earliest}..{}", tail.unwrap_or(latest))
}

/// Parse a strict `YYYY-MM-DD` day.
fn parse_day(value: &str, field: &str) -> Result<NaiveDate, LificError> {
    let trimmed = value.trim();
    if trimmed.len() != 10 {
        return Err(LificError::BadRequest(format!(
            "{field} must be a date like 2026-09-28, got '{value}'"
        )));
    }
    NaiveDate::parse_from_str(trimmed, "%Y-%m-%d").map_err(|_| {
        LificError::BadRequest(format!(
            "{field} must be a date like 2026-09-28, got '{value}'"
        ))
    })
}

const SELECT: &str = "SELECT w.id, w.issue_id, w.kind, w.user_id, u.username, u.display_name,
            w.earliest, w.latest, w.note, w.created_at
       FROM issue_waits w
       LEFT JOIN users u ON u.id = w.user_id";

const ORDER: &str = "ORDER BY w.kind DESC, w.earliest, w.id";

fn wait_from_row(row: &Row, today: &str) -> rusqlite::Result<IssueWait> {
    let kind = match row.get::<_, String>(2)?.as_str() {
        "user" => WaitKind::User,
        _ => WaitKind::Date,
    };
    let earliest: Option<String> = row.get(6)?;
    let latest: Option<String> = row.get(7)?;
    let display_name: Option<String> = row.get(5)?;
    Ok(IssueWait {
        id: row.get(0)?,
        issue_id: row.get(1)?,
        kind,
        user_id: row.get(3)?,
        username: row.get(4)?,
        display_name: display_name.filter(|name| !name.is_empty()),
        state: state_on(kind, earliest.as_deref(), latest.as_deref(), today),
        earliest,
        latest,
        note: row.get(8)?,
        created_at: row.get(9)?,
    })
}

/// Every wait on one issue, user waits first, then by earliest day.
pub fn list_waits(conn: &Connection, issue_id: i64) -> Result<Vec<IssueWait>, LificError> {
    let today = today_text();
    let mut stmt = conn.prepare_cached(&format!("{SELECT} WHERE w.issue_id = ?1 {ORDER}"))?;
    let rows = stmt
        .query_map(params![issue_id], |row| wait_from_row(row, &today))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Waits for many issues in one round trip, keyed by issue id.
pub fn waits_by_issue(
    conn: &Connection,
    issue_ids: &[i64],
) -> Result<HashMap<i64, Vec<IssueWait>>, LificError> {
    let mut by_issue: HashMap<i64, Vec<IssueWait>> = HashMap::new();
    if issue_ids.is_empty() {
        return Ok(by_issue);
    }
    let today = today_text();
    let placeholders = issue_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let mut stmt = conn.prepare(&format!(
        "{SELECT} WHERE w.issue_id IN ({placeholders}) {ORDER}"
    ))?;
    let rows = stmt.query_map(rusqlite::params_from_iter(issue_ids), |row| {
        wait_from_row(row, &today)
    })?;
    for row in rows {
        let wait = row?;
        by_issue.entry(wait.issue_id).or_default().push(wait);
    }
    Ok(by_issue)
}

pub fn get_wait(conn: &Connection, wait_id: i64) -> Result<IssueWait, LificError> {
    let today = today_text();
    conn.prepare_cached(&format!("{SELECT} WHERE w.id = ?1"))?
        .query_row(params![wait_id], |row| wait_from_row(row, &today))
        .optional()?
        .ok_or_else(|| LificError::NotFound(format!("wait {wait_id} not found")))
}

/// An active account by username, with or without a leading `@`.
fn resolve_user(conn: &Connection, name: &str) -> Result<(i64, String), LificError> {
    let name = name.trim().trim_start_matches('@');
    conn.query_row(
        "SELECT id, username FROM users
          WHERE username = ?1 COLLATE NOCASE AND is_active = 1",
        params![name],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()?
    .ok_or_else(|| LificError::BadRequest(format!("no active user named '{name}'")))
}

/// A wait as one line of prose, for messages: `@blake` or `2026-09-28..29`.
pub fn describe(wait: &IssueWait) -> String {
    match wait.kind {
        WaitKind::User => format!("@{}", wait.username.as_deref().unwrap_or("deleted user")),
        WaitKind::Date => format_window(
            wait.earliest.as_deref().unwrap_or(""),
            wait.latest.as_deref().unwrap_or(""),
        ),
    }
}

/// Add a wait to a live issue. Exactly one of `user` or `from` is required;
/// `until` defaults to `from`. Duplicates (the same person, or the same
/// window, on the same issue) are refused rather than silently merged, so the
/// caller learns the blocker already exists.
pub fn add_wait(
    conn: &Connection,
    issue_id: i64,
    input: &CreateWait,
    created_by: Option<i64>,
) -> Result<IssueWait, LificError> {
    let identifier = super::get_issue(conn, issue_id)?.identifier;
    let note = input.note.as_deref().unwrap_or("").trim().to_string();
    if note.chars().count() > MAX_NOTE_CHARS {
        return Err(LificError::BadRequest(format!(
            "note is longer than {MAX_NOTE_CHARS} characters"
        )));
    }
    let blank = |value: &Option<String>| value.as_deref().is_none_or(|v| v.trim().is_empty());
    let (user, from) = (
        input.user.as_ref().filter(|_| !blank(&input.user)),
        input.from.as_ref().filter(|_| !blank(&input.from)),
    );
    let id = match (user, from) {
        (Some(_), Some(_)) => {
            return Err(LificError::BadRequest(
                "a wait is on a user or on dates, not both: pass user or from".into(),
            ));
        }
        (None, None) => {
            if !blank(&input.until) {
                return Err(LificError::BadRequest(
                    "until needs from: pass from (and optionally until) for a date wait".into(),
                ));
            }
            return Err(LificError::BadRequest(
                "pass user (a username) or from (a date) to add a wait".into(),
            ));
        }
        (Some(name), None) => {
            if !blank(&input.until) {
                return Err(LificError::BadRequest(
                    "until applies to date waits only".into(),
                ));
            }
            let (user_id, username) = resolve_user(conn, name)?;
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM issue_waits
                                WHERE issue_id = ?1 AND kind = 'user' AND user_id = ?2)",
                params![issue_id, user_id],
                |row| row.get(0),
            )?;
            if exists {
                return Err(LificError::Conflict(format!(
                    "{identifier} already waits on @{username}"
                )));
            }
            conn.execute(
                "INSERT INTO issue_waits (issue_id, kind, user_id, note, created_by)
                 VALUES (?1, 'user', ?2, ?3, ?4)",
                params![issue_id, user_id, note, created_by],
            )?;
            conn.last_insert_rowid()
        }
        (None, Some(from)) => {
            let earliest = parse_day(from, "from")?;
            let latest = match input.until.as_deref().filter(|v| !v.trim().is_empty()) {
                Some(until) => parse_day(until, "until")?,
                None => earliest,
            };
            if latest < earliest {
                return Err(LificError::BadRequest(format!(
                    "until ({latest}) is before from ({earliest})"
                )));
            }
            let (earliest, latest) = (
                earliest.format("%Y-%m-%d").to_string(),
                latest.format("%Y-%m-%d").to_string(),
            );
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM issue_waits
                                WHERE issue_id = ?1 AND kind = 'date'
                                  AND earliest = ?2 AND latest = ?3)",
                params![issue_id, earliest, latest],
                |row| row.get(0),
            )?;
            if exists {
                return Err(LificError::Conflict(format!(
                    "{identifier} already waits until {}",
                    format_window(&earliest, &latest)
                )));
            }
            conn.execute(
                "INSERT INTO issue_waits (issue_id, kind, earliest, latest, note, created_by)
                 VALUES (?1, 'date', ?2, ?3, ?4, ?5)",
                params![issue_id, earliest, latest, note, created_by],
            )?;
            conn.last_insert_rowid()
        }
    };
    get_wait(conn, id)
}

/// Clear one wait by id. `issue_id` must be the issue it belongs to, so a
/// caller authorized on one issue cannot clear a wait on another.
pub fn clear_wait(conn: &Connection, issue_id: i64, wait_id: i64) -> Result<IssueWait, LificError> {
    let wait = get_wait(conn, wait_id)?;
    if wait.issue_id != issue_id {
        return Err(LificError::NotFound(format!("wait {wait_id} not found")));
    }
    conn.execute("DELETE FROM issue_waits WHERE id = ?1", params![wait_id])?;
    Ok(wait)
}

/// Clear the issue's wait on `user`.
pub fn clear_user_wait(
    conn: &Connection,
    issue_id: i64,
    user: &str,
) -> Result<IssueWait, LificError> {
    let identifier = super::get_issue(conn, issue_id)?.identifier;
    let name = user.trim().trim_start_matches('@');
    let wait_id: Option<i64> = conn
        .query_row(
            "SELECT w.id FROM issue_waits w JOIN users u ON u.id = w.user_id
              WHERE w.issue_id = ?1 AND w.kind = 'user' AND u.username = ?2 COLLATE NOCASE",
            params![issue_id, name],
            |row| row.get(0),
        )
        .optional()?;
    let wait_id = wait_id
        .ok_or_else(|| LificError::NotFound(format!("{identifier} is not waiting on @{name}")))?;
    clear_wait(conn, issue_id, wait_id)
}

/// Clear every date wait on the issue whose window starts on `from`.
pub fn clear_date_waits(
    conn: &Connection,
    issue_id: i64,
    from: &str,
) -> Result<Vec<IssueWait>, LificError> {
    let identifier = super::get_issue(conn, issue_id)?.identifier;
    let earliest = parse_day(from, "from")?.format("%Y-%m-%d").to_string();
    let waits: Vec<IssueWait> = list_waits(conn, issue_id)?
        .into_iter()
        .filter(|wait| wait.kind == WaitKind::Date && wait.earliest.as_deref() == Some(&earliest))
        .collect();
    if waits.is_empty() {
        return Err(LificError::NotFound(format!(
            "{identifier} has no date wait starting {earliest}"
        )));
    }
    for wait in &waits {
        conn.execute("DELETE FROM issue_waits WHERE id = ?1", params![wait.id])?;
    }
    Ok(waits)
}

#[cfg(test)]
mod tests;
