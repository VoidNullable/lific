//! LIF-486: page content edits are audited as line diffs, not full bodies.
//!
//! Migration 018's `audit_pages_update` trigger stores a page's whole old and
//! new content on every edit. For a large page rewritten many times that made
//! `audit_log` most of the database (one 1.2 MB page edited 143 times held
//! 176 MB). `page_revisions` cannot stand in for those rows: it keeps only the
//! newest 50 versions within a size budget, while the audit log keeps
//! history for as long as `audit_retention_days` says.
//!
//! So each full-content row is rewritten into `field = 'content_diff'`: a
//! unified diff (three lines of context, no file header) in `new_value` and
//! `old_value` NULL. The diff is self-contained, survives revision pruning,
//! and travels with the row in project archives.
//!
//! The trigger is left as it is so every writer, including a raw SQL update
//! outside Lific, is still audited. `update_page` compacts the rows its write
//! produced, and migration 063 compacts everything already stored. A full row
//! that arrives some other way (an older archive, a manual update) stays valid
//! and readers keep rendering both shapes.

use rusqlite::{Connection, OptionalExtension, params};
use std::time::Duration;

/// `audit_log.field` of a page content edit stored as a diff.
pub const DIFF_FIELD: &str = "content_diff";

const CONTEXT_LINES: usize = 3;

/// Myers degrades on a wholesale rewrite of a long page; past this the diff
/// is approximated (still correct, just less minimal) instead of stalling the
/// write.
const DIFF_TIMEOUT: Duration = Duration::from_secs(1);

/// The stored form of a content change: a unified diff without file headers.
pub fn content_diff(old: &str, new: &str) -> String {
    similar::TextDiff::configure()
        .timeout(DIFF_TIMEOUT)
        .diff_lines(old, new)
        .unified_diff()
        .context_radius(CONTEXT_LINES)
        .to_string()
}

/// Lines added and removed by a stored diff, for one-line summaries.
pub fn diff_stats(diff: &str) -> (usize, usize) {
    diff.lines().fold((0, 0), |(added, removed), line| {
        if line.starts_with('+') {
            (added + 1, removed)
        } else if line.starts_with('-') {
            (added, removed + 1)
        } else {
            (added, removed)
        }
    })
}

/// Highest `audit_log.id`, so a caller can compact only the rows its own
/// write is about to add.
pub fn last_audit_id(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT COALESCE(MAX(id), 0) FROM audit_log", [], |row| {
        row.get(0)
    })
}

/// Rewrite full-content page edits with `id > after_id` into diffs, limited
/// to one page when `page_id` is given. Returns the number of rows rewritten.
///
/// Rows are read one at a time: a single legacy row can carry megabytes of
/// text, and the migration walks all of them.
pub fn compact(conn: &Connection, page_id: Option<i64>, after_id: i64) -> rusqlite::Result<usize> {
    let ids: Vec<i64> = match page_id {
        Some(page_id) => conn
            .prepare_cached(
                "SELECT id FROM audit_log
                 WHERE entity_type = 'page' AND entity_id = ?1 AND id > ?2
                   AND action = 'update' AND field = 'content'
                 ORDER BY id",
            )?
            .query_map(params![page_id, after_id], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?,
        None => conn
            .prepare(
                "SELECT id FROM audit_log
                 WHERE entity_type = 'page' AND id > ?1
                   AND action = 'update' AND field = 'content'
                 ORDER BY id",
            )?
            .query_map(params![after_id], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?,
    };

    for &id in &ids {
        let values: Option<(Option<String>, Option<String>)> = conn
            .prepare_cached("SELECT old_value, new_value FROM audit_log WHERE id = ?1")?
            .query_row(params![id], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()?;
        let Some((old, new)) = values else { continue };
        let diff = content_diff(old.as_deref().unwrap_or(""), new.as_deref().unwrap_or(""));
        conn.prepare_cached(
            "UPDATE audit_log SET field = ?1, old_value = NULL, new_value = ?2 WHERE id = ?3",
        )?
        .execute(params![DIFF_FIELD, diff, id])?;
    }
    Ok(ids.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_one_line_edit_stores_only_the_changed_hunk() {
        let old: String = (1..=100)
            .map(|n| format!("line {n}\n"))
            .collect::<Vec<_>>()
            .concat();
        let new = old.replace("line 50\n", "line fifty\n");
        let diff = content_diff(&old, &new);
        assert!(diff.starts_with("@@ -47,7 +47,7 @@"), "{diff}");
        assert!(diff.contains("-line 50\n+line fifty\n"));
        assert!(!diff.contains("line 1\n"));
        assert!(!diff.contains("line 100"));
        assert_eq!(diff_stats(&diff), (1, 1));
    }

    #[test]
    fn content_written_from_nothing_is_all_additions() {
        let diff = content_diff("", "# Title\nbody\n");
        assert_eq!(diff_stats(&diff), (2, 0));
    }
}
