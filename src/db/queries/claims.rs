//! Claims (LIF-525): a worker holds an issue for a limited time.
//!
//! A claim is a lease. It names a holder (an account plus an optional worker
//! id, see [`Claimant`]) and an expiry; claiming again while you hold it
//! moves the expiry and keeps the first claim time. A claim held by someone
//! else that has not expired refuses yours. Every read compares `expires_at`
//! with the clock, and an expired row is the same as no row until the next
//! claim overwrites it. There is no reaper. Releasing a claim moves its expiry
//! to now rather than deleting it, so the stale checks still see when anyone
//! last worked under a claim.
//!
//! The clock is [`now`], which tests can pin per thread the way
//! `waits::pin_today` pins the day, so a test passes a claim's TTL by moving
//! the clock rather than by waiting.

use std::cell::Cell;
use std::collections::HashMap;

use chrono::{Duration, NaiveDateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::db::models::{Claimant, IssueClaim, Status};
use crate::error::LificError;

/// How long a claim lasts when the caller names no length.
pub const DEFAULT_MINUTES: i64 = 15;
/// The longest claim: a day. Anything longer is a status, not a lease.
pub const MAX_MINUTES: i64 = 24 * 60;
/// Longest worker id a caller may pass back.
pub const MAX_WORKER_CHARS: usize = 64;

/// SQLite's `datetime('now')` layout, which every claim timestamp uses.
const STORED_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

thread_local! {
    /// Test override for [`now`]. Reads and writes run synchronously on the
    /// calling thread (REST's `with_read`, MCP's `self.read`), so a
    /// per-thread pin reaches them without a process-wide clock.
    static PINNED_NOW: Cell<Option<NaiveDateTime>> = const { Cell::new(None) };
}

/// The current UTC time, unless a test has pinned one.
pub fn now() -> NaiveDateTime {
    PINNED_NOW
        .with(Cell::get)
        .unwrap_or_else(|| Utc::now().naive_utc())
}

/// [`now`] in the stored layout, for SQL comparison.
pub fn now_text() -> String {
    now().format(STORED_FORMAT).to_string()
}

/// Restores the previous pin when dropped.
#[cfg(test)]
pub(crate) struct PinnedNow(Option<NaiveDateTime>);

#[cfg(test)]
impl Drop for PinnedNow {
    fn drop(&mut self) {
        PINNED_NOW.with(|cell| cell.set(self.0));
    }
}

/// Pin [`now`] on this thread for as long as the guard lives.
#[cfg(test)]
pub(crate) fn pin_now(at: &str) -> PinnedNow {
    let at = NaiveDateTime::parse_from_str(at, STORED_FORMAT).expect("test time");
    PinnedNow(PINNED_NOW.with(|cell| cell.replace(Some(at))))
}

/// A fresh worker id: `prefix` plus eight random hex digits.
pub fn mint_worker(prefix: &str) -> String {
    format!("{prefix}-{:08x}", rand::random::<u32>())
}

/// A worker id derived from a transport session id (an `Mcp-Session-Id` or
/// `X-Lific-Worker` header). Hashed, so the raw session id is never stored or
/// shown, and stable, so every request of the session maps to the same id.
pub fn worker_from_session(session: &str) -> Option<String> {
    use sha2::{Digest, Sha256};
    let session = session.trim();
    if session.is_empty() {
        return None;
    }
    let digest = Sha256::digest(session.as_bytes());
    let head = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]);
    Some(format!("s-{head:08x}"))
}

/// A worker id a caller passed back must look like one Lific hands out:
/// short, and letters, digits, `.`, `_`, `:` or `-` only.
pub fn validate_worker(worker: &str) -> Result<(), LificError> {
    let ok = !worker.is_empty()
        && worker.chars().count() <= MAX_WORKER_CHARS
        && worker
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'));
    if ok {
        Ok(())
    } else {
        Err(LificError::BadRequest(format!(
            "invalid worker '{worker}': use the id a claim returned (up to {MAX_WORKER_CHARS} letters, digits, '.', '_', ':' or '-')"
        )))
    }
}

/// A claim length in minutes, defaulted and bounded.
pub fn minutes(requested: Option<i64>) -> Result<i64, LificError> {
    match requested {
        None => Ok(DEFAULT_MINUTES),
        Some(minutes) if (1..=MAX_MINUTES).contains(&minutes) => Ok(minutes),
        Some(minutes) => Err(LificError::BadRequest(format!(
            "claim_minutes must be 1 to {MAX_MINUTES} (24 hours), got {minutes}"
        ))),
    }
}

/// The status a claim moves an issue to: an issue nobody has started
/// (backlog or todo) becomes active, anything else stays as it is.
pub fn status_on_claim(current: Status) -> Option<Status> {
    matches!(current, Status::Backlog | Status::Todo).then_some(Status::Active)
}

const SELECT: &str = "SELECT c.issue_id, c.user_id, u.username, u.display_name, c.worker,
                             c.claimed_at, c.expires_at
                        FROM issue_claims c JOIN users u ON u.id = c.user_id";

fn claim_row(row: &Row<'_>) -> rusqlite::Result<(i64, IssueClaim)> {
    Ok((
        row.get(0)?,
        IssueClaim {
            user_id: row.get(1)?,
            username: row.get(2)?,
            display_name: row
                .get::<_, Option<String>>(3)?
                .filter(|name| !name.is_empty()),
            worker: row.get(4)?,
            claimed_at: row.get(5)?,
            expires_at: row.get(6)?,
        },
    ))
}

/// The live claim on an issue, if any.
pub fn live_claim(conn: &Connection, issue_id: i64) -> Result<Option<IssueClaim>, LificError> {
    Ok(conn
        .prepare_cached(&format!(
            "{SELECT} WHERE c.issue_id = ?1 AND c.expires_at > ?2"
        ))?
        .query_row(params![issue_id, now_text()], claim_row)
        .optional()?
        .map(|(_, claim)| claim))
}

/// The live claims on `ids`, keyed by issue id, in one round trip.
pub fn claims_by_issue(
    conn: &Connection,
    ids: &[i64],
) -> Result<HashMap<i64, IssueClaim>, LificError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = (0..ids.len())
        .map(|n| format!("?{}", n + 2))
        .collect::<Vec<_>>()
        .join(",");
    let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(now_text())];
    values.extend(
        ids.iter()
            .map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>),
    );
    let refs: Vec<&dyn rusqlite::types::ToSql> = values.iter().map(|v| v.as_ref()).collect();
    let mut stmt = conn.prepare(&format!(
        "{SELECT} WHERE c.expires_at > ?1 AND c.issue_id IN ({placeholders})"
    ))?;
    let rows = stmt.query_map(refs.as_slice(), claim_row)?;
    Ok(rows.collect::<Result<HashMap<_, _>, _>>()?)
}

fn held_elsewhere(identifier: &str, claim: &IssueClaim) -> LificError {
    LificError::Conflict(format!(
        "{identifier} is claimed by {}. Pick another issue, or wait for the claim to end.",
        claim.describe()
    ))
}

/// Claim `issue_id` for `claimant` for `minutes`, or renew the claim they
/// already hold. Renewing moves the expiry and keeps the first claim time.
/// A live claim held by someone else is refused, naming the holder.
///
/// The caller checks the issue is open and sets its status; this touches
/// only the claim.
pub fn claim(
    conn: &Connection,
    issue_id: i64,
    identifier: &str,
    claimant: &Claimant,
    minutes: i64,
) -> Result<IssueClaim, LificError> {
    let at = now();
    let stamp = at.format(STORED_FORMAT).to_string();
    let expires = (at + Duration::minutes(minutes))
        .format(STORED_FORMAT)
        .to_string();
    match live_claim(conn, issue_id)? {
        Some(current) if !current.held_by(claimant) => {
            return Err(held_elsewhere(identifier, &current));
        }
        Some(_) => {
            conn.execute(
                "UPDATE issue_claims SET renewed_at = ?2, expires_at = ?3 WHERE issue_id = ?1",
                params![issue_id, stamp, expires],
            )?;
        }
        None => {
            conn.execute(
                "INSERT OR REPLACE INTO issue_claims
                     (issue_id, user_id, worker, claimed_at, renewed_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
                params![issue_id, claimant.user_id, claimant.worker, stamp, expires],
            )?;
        }
    }
    live_claim(conn, issue_id)?
        .ok_or_else(|| LificError::Internal(format!("claim on {identifier} did not persist")))
}

/// End the claim `claimant` holds on `issue_id`. Returns the claim that
/// ended, or `None` when there was no live claim (releasing nothing is not
/// an error). A live claim held by someone else is refused.
pub fn release(
    conn: &Connection,
    issue_id: i64,
    identifier: &str,
    claimant: &Claimant,
) -> Result<Option<IssueClaim>, LificError> {
    let current = live_claim(conn, issue_id)?;
    if let Some(current) = &current
        && !current.held_by(claimant)
    {
        return Err(LificError::Conflict(format!(
            "{identifier} is claimed by {}; only its holder can release it, and it ends by itself at expiry.",
            current.describe()
        )));
    }
    if current.is_some() {
        end(conn, issue_id)?;
    }
    Ok(current)
}

/// End whatever live claim `issue_id` has, whoever holds it. The row stays,
/// expired now, so the release still counts as the issue's last claim
/// activity for the stale checks; the next claim overwrites it.
pub fn end(conn: &Connection, issue_id: i64) -> Result<(), LificError> {
    let stamp = now_text();
    conn.execute(
        "UPDATE issue_claims SET expires_at = ?2, renewed_at = ?2
          WHERE issue_id = ?1 AND expires_at > ?2",
        params![issue_id, stamp],
    )?;
    Ok(())
}

/// SQL true when issue `{alias}.id` has a live claim held by anyone other
/// than the claimant. `now` names the bound current time; `holder` names the
/// bound claimant user id and worker, or `None` when every live claim is
/// someone else's.
pub(crate) fn held_by_other_sql(alias: &str, now: &str, holder: Option<(&str, &str)>) -> String {
    let mine = holder.map_or_else(String::new, |(user, worker)| {
        format!(" AND NOT (c.user_id = {user} AND (c.worker IS NULL OR c.worker IS {worker}))")
    });
    format!(
        "EXISTS (SELECT 1 FROM issue_claims c
                  WHERE c.issue_id = {alias}.id AND c.expires_at > {now}{mine})"
    )
}

/// SQL for an issue's last activity: the later of `updated_at` and its last
/// claim or re-claim, or `now` while a claim is live. A worker renewing a
/// claim is working on the issue even when nothing else about it changes.
pub(crate) fn last_activity_sql(alias: &str, now: &str) -> String {
    format!(
        "MAX({alias}.updated_at, COALESCE((SELECT CASE WHEN c.expires_at > {now} THEN {now}
                                                       ELSE c.renewed_at END
                                             FROM issue_claims c WHERE c.issue_id = {alias}.id), ''))"
    )
}

#[cfg(test)]
mod tests;
