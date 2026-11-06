//! Idle active issues (LIF-492).
//!
//! An issue moved to `active` says someone is on it. Nothing ends that claim
//! when the session that made it stops, so "active" drifts into "was active
//! once". Idle means the issue's `updated_at` is old: comments, label changes
//! and every field edit bump it (migration 017 and the base triggers), so it is
//! the last time anyone touched the issue at all.
//!
//! The predicate is `status = 'active' AND updated_at < cutoff`. Listings
//! express it through `ListIssuesQuery { status: Active, updated_until }` so it
//! composes with every other filter; this module owns the cutoff arithmetic so
//! all callers agree on it to the second.

use chrono::{DateTime, Duration, NaiveDateTime, Utc};

/// SQLite's `datetime('now')` layout, the form `updated_at` is stored in.
const STORED_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

/// The `updated_at` bound for "idle at least `days` days as of `now`": an
/// active issue updated strictly before this is idle.
pub fn cutoff(now: DateTime<Utc>, days: i64) -> String {
    (now - Duration::days(days))
        .format(STORED_FORMAT)
        .to_string()
}

/// How long ago `updated_at` was, as of `now`. `None` when the stored value
/// does not parse, which a row written by SQLite itself never produces.
pub fn idle_for(updated_at: &str, now: DateTime<Utc>) -> Option<Duration> {
    let then = NaiveDateTime::parse_from_str(updated_at, STORED_FORMAT).ok()?;
    Some(now.naive_utc() - then)
}

/// A short idle age for listings: `4d`, `7h`, or `<1h`.
pub fn short_age(idle: Duration) -> String {
    if idle.num_days() >= 1 {
        format!("{}d", idle.num_days())
    } else if idle.num_hours() >= 1 {
        format!("{}h", idle.num_hours())
    } else {
        "<1h".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(text: &str) -> DateTime<Utc> {
        Utc.from_utc_datetime(&NaiveDateTime::parse_from_str(text, STORED_FORMAT).unwrap())
    }

    #[test]
    fn cutoff_is_the_stored_layout_days_before_now() {
        assert_eq!(cutoff(at("2026-11-06 12:00:00"), 3), "2026-11-03 12:00:00");
    }

    #[test]
    fn idle_age_reads_stored_timestamps() {
        let now = at("2026-11-06 12:00:00");
        let age = |stored| short_age(idle_for(stored, now).unwrap());
        assert_eq!(age("2026-10-20 08:00:00"), "17d");
        assert_eq!(age("2026-11-06 05:00:00"), "7h");
        assert_eq!(age("2026-11-06 11:30:00"), "<1h");
        assert!(idle_for("yesterday", now).is_none());
    }
}
