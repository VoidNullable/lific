//! Idempotency keys for creates (`op_keys`). See migration 073.
//!
//! A client that loses the reply to a create cannot tell whether the write
//! happened, so it calls again. With a key, the second call finds the first
//! one's row and gets its result back instead of writing a duplicate. The row
//! is written by [`record`] inside the create's own transaction, so the key
//! exists exactly when the write committed: a failed or refused write leaves
//! no key behind, and a corrected retry with the same key goes through.

use rusqlite::{Connection, ErrorCode, OptionalExtension, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::LificError;

/// How long a key is honored. Older rows are ignored by [`replay`], replaced by
/// [`record`] and deleted by the retention sweep ([`purge_expired`]).
pub(crate) const TTL_HOURS: i64 = 24;

/// Longest key accepted, in bytes. A UUID is 36; this leaves room for any
/// client's own scheme without letting a key grow into a payload.
pub(crate) const MAX_KEY_BYTES: usize = 255;

/// One create call's key, scoped to its caller and bound to its request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpKey {
    user_id: Option<i64>,
    key: String,
    operation: &'static str,
    request_hash: String,
    /// What the caller calls the key (`op_key`, `Idempotency-Key`), so errors
    /// name the thing they actually sent.
    name: &'static str,
}

impl OpKey {
    /// `None` when the call carried no key. `operation` names the call
    /// (`mcp.create_issue`, `rest.create_page`), and `request` is everything
    /// that decides what it writes, key excluded.
    pub(crate) fn new(
        name: &'static str,
        user_id: Option<i64>,
        key: Option<&str>,
        operation: &'static str,
        request: &impl Serialize,
    ) -> Result<Option<Self>, LificError> {
        let Some(key) = key else {
            return Ok(None);
        };
        if key.is_empty() || key.len() > MAX_KEY_BYTES || key.chars().any(char::is_control) {
            return Err(LificError::BadRequest(format!(
                "{name} must be 1 to {MAX_KEY_BYTES} bytes with no control characters"
            )));
        }
        let body = serde_json::to_vec(request)
            .map_err(|error| LificError::Internal(format!("hashing {name} request: {error}")))?;
        let mut hasher = Sha256::new();
        hasher.update(operation.as_bytes());
        hasher.update([0]);
        hasher.update(&body);
        Ok(Some(Self {
            user_id,
            key: key.to_owned(),
            operation,
            request_hash: format!("{:x}", hasher.finalize()),
            name,
        }))
    }

    pub(crate) fn key(&self) -> &str {
        &self.key
    }
}

/// Largest result kept for a replay. A bigger one (a plan with a huge tree, a
/// page near the request cap) records the key with no result: the retry still
/// writes nothing, and is told to read what was created instead. Dropping the
/// key would have let that retry write a duplicate, which is the one thing a
/// key exists to prevent.
pub(crate) const MAX_RESULT_BYTES: usize = 1024 * 1024;

/// What an earlier call with the same key left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Stored {
    /// The result it returned.
    Result(String),
    /// It committed, but its result was over [`MAX_RESULT_BYTES`].
    TooLarge,
}

/// A create's outcome: its own fresh result, or an earlier call's.
#[derive(Debug)]
pub(crate) enum Written<T> {
    Fresh(T),
    Replayed(Stored),
}

/// The result stored by the call that first used this key, when that call
/// committed within [`TTL_HOURS`] and asked for the same thing. The same key
/// with a different request is refused with a conflict.
///
/// A create runs this inside its own immediate transaction, after its
/// authorization checks and before it writes. The transaction serializes it
/// with a concurrent call holding the same key, so a retry that races the
/// still-running original waits for it and replays its result; and a caller
/// who lost access since gets the same refusal a fresh create would.
pub(crate) fn replay(conn: &Connection, key: &OpKey) -> Result<Option<Stored>, LificError> {
    let row: Option<(String, String, Option<String>)> = conn
        .query_row(
            "SELECT operation, request_hash, result FROM op_keys
             WHERE IFNULL(user_id, 0) = IFNULL(?1, 0) AND op_key = ?2
               AND created_at > datetime('now', ?3)",
            params![key.user_id, key.key, ttl_modifier()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    match row {
        None => Ok(None),
        Some((operation, hash, result))
            if operation == key.operation && hash == key.request_hash =>
        {
            Ok(Some(result.map_or(Stored::TooLarge, Stored::Result)))
        }
        Some(_) => Err(LificError::Conflict(format!(
            "{} \"{}\" was already used for a different request; nothing was written. Use a new key for a new write",
            key.name, key.key
        ))),
    }
}

/// [`replay`] when the call carried a key; `None` otherwise.
pub(crate) fn replay_opt(
    conn: &Connection,
    key: Option<&OpKey>,
) -> Result<Option<Stored>, LificError> {
    key.map_or(Ok(None), |key| replay(conn, key))
}

/// Mark the key done with `result`. Call inside the create's transaction,
/// after the write, so the key and the write commit or roll back together.
/// A result over [`MAX_RESULT_BYTES`] is not kept; see [`Stored::TooLarge`].
pub(crate) fn record(conn: &Connection, key: &OpKey, result: &str) -> Result<(), LificError> {
    let result = (result.len() <= MAX_RESULT_BYTES).then_some(result);
    conn.execute(
        "DELETE FROM op_keys
         WHERE IFNULL(user_id, 0) = IFNULL(?1, 0) AND op_key = ?2
           AND created_at <= datetime('now', ?3)",
        params![key.user_id, key.key, ttl_modifier()],
    )?;
    let inserted = conn.execute(
        "INSERT INTO op_keys (user_id, op_key, operation, request_hash, result)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            key.user_id,
            key.key,
            key.operation,
            key.request_hash,
            result
        ],
    );
    match inserted {
        Ok(_) => Ok(()),
        // Only reachable when a caller skipped `replay` in its transaction.
        // Failing here rolls this one back, and a retry replays the winner.
        Err(rusqlite::Error::SqliteFailure(error, _))
            if error.code == ErrorCode::ConstraintViolation =>
        {
            Err(LificError::Conflict(format!(
                "{} \"{}\" was used by another call that just finished; nothing was written. Retry with the same key to get its result",
                key.name, key.key
            )))
        }
        Err(error) => Err(error.into()),
    }
}

/// [`record`] when the call carried a key; nothing otherwise.
pub(crate) fn record_opt(
    conn: &Connection,
    key: Option<&OpKey>,
    result: impl FnOnce() -> Result<String, LificError>,
) -> Result<(), LificError> {
    match key {
        Some(key) => record(conn, key, &result()?),
        None => Ok(()),
    }
}

/// Delete keys past [`TTL_HOURS`]. Run by the retention sweep.
pub(crate) fn purge_expired(conn: &Connection) -> Result<usize, LificError> {
    Ok(conn.execute(
        "DELETE FROM op_keys WHERE created_at <= datetime('now', ?1)",
        [ttl_modifier()],
    )?)
}

fn ttl_modifier() -> String {
    format!("-{TTL_HOURS} hours")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(user_id: Option<i64>, key: &str, request: &str) -> OpKey {
        OpKey::new("op_key", user_id, Some(key), "test.create", &request)
            .unwrap()
            .unwrap()
    }

    fn rows(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM op_keys", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn a_recorded_key_replays_its_result_for_the_same_request() {
        let pool = crate::db::open_memory().unwrap();
        let conn = pool.write().unwrap();
        let first = key(None, "k1", "same");
        assert_eq!(replay(&conn, &first).unwrap(), None);
        record(&conn, &first, "Created X-1").unwrap();
        assert_eq!(
            replay(&conn, &key(None, "k1", "same")).unwrap(),
            Some(Stored::Result("Created X-1".into()))
        );
    }

    #[test]
    fn an_oversized_result_still_records_the_key_without_the_result() {
        let pool = crate::db::open_memory().unwrap();
        let conn = pool.write().unwrap();
        let big = "x".repeat(MAX_RESULT_BYTES + 1);
        record(&conn, &key(None, "big", "same"), &big).unwrap();
        assert_eq!(
            replay(&conn, &key(None, "big", "same")).unwrap(),
            Some(Stored::TooLarge)
        );
        let at_cap = "x".repeat(MAX_RESULT_BYTES);
        record(&conn, &key(None, "cap", "same"), &at_cap).unwrap();
        assert_eq!(
            replay(&conn, &key(None, "cap", "same")).unwrap(),
            Some(Stored::Result(at_cap))
        );
    }

    #[test]
    fn the_same_key_with_a_different_request_is_a_conflict() {
        let pool = crate::db::open_memory().unwrap();
        let conn = pool.write().unwrap();
        record(&conn, &key(None, "k1", "first"), "Created X-1").unwrap();
        let error = replay(&conn, &key(None, "k1", "second")).unwrap_err();
        assert!(
            matches!(error, LificError::Conflict(ref message) if message.contains("different request"))
        );
    }

    #[test]
    fn keys_are_scoped_to_their_user() {
        let pool = crate::db::open_memory().unwrap();
        let conn = pool.write().unwrap();
        record(&conn, &key(None, "k1", "first"), "anonymous").unwrap();
        assert_eq!(replay(&conn, &key(Some(1), "k1", "other")).unwrap(), None);
    }

    #[test]
    fn a_second_record_of_one_key_is_a_conflict() {
        let pool = crate::db::open_memory().unwrap();
        let conn = pool.write().unwrap();
        record(&conn, &key(None, "k1", "same"), "first").unwrap();
        let error = record(&conn, &key(None, "k1", "same"), "second").unwrap_err();
        assert!(matches!(error, LificError::Conflict(ref message) if message.contains("Retry")));
        assert_eq!(rows(&conn), 1);
    }

    #[test]
    fn an_expired_key_is_ignored_replaced_and_purged() {
        let pool = crate::db::open_memory().unwrap();
        let conn = pool.write().unwrap();
        record(&conn, &key(None, "old", "first"), "stale").unwrap();
        record(&conn, &key(None, "fresh", "first"), "kept").unwrap();
        conn.execute(
            "UPDATE op_keys SET created_at = datetime('now', '-25 hours') WHERE op_key = 'old'",
            [],
        )
        .unwrap();
        assert_eq!(replay(&conn, &key(None, "old", "other")).unwrap(), None);
        assert_eq!(purge_expired(&conn).unwrap(), 1);
        assert_eq!(rows(&conn), 1);

        record(&conn, &key(None, "fresh2", "first"), "x").unwrap();
        conn.execute(
            "UPDATE op_keys SET created_at = datetime('now', '-25 hours') WHERE op_key = 'fresh2'",
            [],
        )
        .unwrap();
        record(&conn, &key(None, "fresh2", "second"), "reused").unwrap();
        assert_eq!(
            replay(&conn, &key(None, "fresh2", "second")).unwrap(),
            Some(Stored::Result("reused".into()))
        );
    }

    #[test]
    fn malformed_keys_are_refused_by_name() {
        for bad in ["", "a\nb", &"k".repeat(MAX_KEY_BYTES + 1)] {
            let error =
                OpKey::new("Idempotency-Key", None, Some(bad), "test.create", &"x").unwrap_err();
            assert!(
                matches!(error, LificError::BadRequest(ref message) if message.starts_with("Idempotency-Key"))
            );
        }
        assert_eq!(
            OpKey::new("op_key", None, None, "test.create", &"x").unwrap(),
            None
        );
    }
}
