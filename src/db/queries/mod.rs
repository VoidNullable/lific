pub(crate) mod activity;
pub(crate) mod attachments;
pub(crate) mod comments;
pub(crate) mod insights;
mod issues;
pub(crate) mod members;
mod pages;
pub(crate) mod plans;
pub(crate) mod project_groups;
mod projects;
mod resources;
mod search;
pub(crate) mod settings;
pub(crate) mod users;
pub(crate) mod views;

/// Repair literal `\n` and `\t` sequences from clients that double-escape JSON.
///
/// A real newline or tab indicates that the client sent proper JSON, so preserve
/// the input unchanged and treat any literal escapes as intentional content. This
/// still repairs intentional literal escapes in single-line content with no real
/// control characters, an acceptable tradeoff because the common corruption case
/// is multi-line code blocks, which contain real newlines.
pub(crate) fn unescape_text(s: &str) -> String {
    if s.contains('\n') || s.contains('\t') {
        return s.to_string();
    }
    s.replace("\\n", "\n").replace("\\t", "\t")
}

/// Default page size when a caller does not ask for one.
pub const DEFAULT_PAGE_LIMIT: i64 = 50;

/// Hard cap on a single page. Every paginated query clamps to this unless it
/// documents a deliberately lower cap of its own (see `activity::list_activity`).
pub const MAX_PAGE_LIMIT: i64 = 500;

/// SQLite's "no limit" sentinel: `LIMIT -1` returns every row. Only reachable
/// through `page_unbounded`, where an absent limit means "no limit" by design.
pub const NO_LIMIT: i64 = -1;

/// Clamp caller-supplied pagination into `(limit, offset)` ready for SQL.
///
/// LIF-141 class: SQLite treats `LIMIT -1` as "no limit", so an unclamped
/// `?limit=-1` would dump the whole table. Floor at 1 so a 0/negative value
/// still paginates, cap at [`MAX_PAGE_LIMIT`], and floor the offset at 0.
pub fn page(limit: Option<i64>, offset: Option<i64>) -> (i64, i64) {
    page_with(limit, offset, DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT)
}

/// [`page`] with a query-specific default and cap, for the few queries whose
/// published defaults differ (search defaults to 20, activity caps at 200).
pub fn page_with(
    limit: Option<i64>,
    offset: Option<i64>,
    default_limit: i64,
    max_limit: i64,
) -> (i64, i64) {
    (
        limit.unwrap_or(default_limit).clamp(1, max_limit),
        offset.unwrap_or(0).max(0),
    )
}

/// [`page`] for queries where an absent limit means "return everything".
///
/// Used by the page and comment listings, whose export/REST/CLI callers rely on
/// unbounded reads; only explicit paging callers pass a limit. A limit that is
/// present is clamped exactly like [`page`].
pub fn page_unbounded(limit: Option<i64>, offset: Option<i64>) -> (i64, i64) {
    (
        limit.map_or(NO_LIMIT, |n| n.clamp(1, MAX_PAGE_LIMIT)),
        offset.unwrap_or(0).max(0),
    )
}

/// One page of rows plus whether the query saw another row past it.
///
/// LIF-388: every paginated read surface used to open-code the same dance —
/// ask for `limit + 1` rows, compare the length against the limit, truncate,
/// hand `has_more` to the renderer. Doing it here instead of at the call site
/// is not only shorter, it is the only place it can be *correct*: a caller
/// that clamps to the cap and then asks for `cap + 1` gets clamped back down
/// to `cap`, so `has_more` is always false on the last legal page size.
/// [`Page::from_over_fetch`] runs inside the query, after its own clamp, so
/// the extra row is never subject to the cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub has_more: bool,
}

impl<T> Page<T> {
    /// Split rows fetched with [`over_fetch`] into the page the caller asked
    /// for, plus the `has_more` the extra row implies.
    pub fn from_over_fetch(mut items: Vec<T>, limit: i64) -> Self {
        let has_more = items.len() as i64 > limit;
        if has_more {
            items.truncate(limit.max(0) as usize);
        }
        Self { items, has_more }
    }

    /// A page that is known to hold everything there is: an unbounded read
    /// ([`NO_LIMIT`]) has nothing beyond it by definition.
    pub fn complete(items: Vec<T>) -> Self {
        Self {
            items,
            has_more: false,
        }
    }
}

/// The SQL `LIMIT` that reads one row past `limit` so [`Page::from_over_fetch`]
/// can answer `has_more` without a second COUNT query. [`NO_LIMIT`] passes
/// through: there is no row past "everything".
pub fn over_fetch(limit: i64) -> i64 {
    match limit {
        NO_LIMIT => NO_LIMIT,
        n => n.saturating_add(1),
    }
}

/// Run a closure inside a SQLite SAVEPOINT so that multi-statement writes are atomic.
/// On success the savepoint is released; on error it is rolled back.
pub(crate) fn savepoint<F, T>(
    conn: &rusqlite::Connection,
    name: &str,
    f: F,
) -> Result<T, crate::error::LificError>
where
    F: FnOnce() -> Result<T, crate::error::LificError>,
{
    conn.execute_batch(&format!("SAVEPOINT {name}"))?;
    match f() {
        Ok(val) => {
            conn.execute_batch(&format!("RELEASE {name}"))?;
            Ok(val)
        }
        Err(e) => {
            // Best-effort rollback — if this fails, the outer transaction will
            // still see the savepoint and rollback at its level.
            let _ = conn.execute_batch(&format!("ROLLBACK TO {name}"));
            let _ = conn.execute_batch(&format!("RELEASE {name}"));
            Err(e)
        }
    }
}

// Re-export everything so callers don't need to know the internal split.
// (activity is accessed via queries::activity:: directly, like users —
// its names are only used by the API/MCP read surface.)
pub use issues::*;
pub use pages::*;
pub use projects::*;
pub use resources::*;
pub use search::*;
// users module is accessed via queries::users:: directly (not wildcard re-exported)
// to keep the namespace clean — user functions are only used by auth/CLI code.

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT, NO_LIMIT, page, page_unbounded, page_with,
        unescape_text,
    };

    // ── page ──────────────────────────────────────────────────

    #[test]
    fn page_defaults_when_nothing_is_supplied() {
        assert_eq!(page(None, None), (DEFAULT_PAGE_LIMIT, 0));
    }

    #[test]
    fn page_passes_through_values_inside_the_bounds() {
        assert_eq!(page(Some(10), Some(25)), (10, 25));
        assert_eq!(page(Some(1), Some(0)), (1, 0));
        assert_eq!(page(Some(MAX_PAGE_LIMIT), Some(0)), (MAX_PAGE_LIMIT, 0));
    }

    // SQLite reads LIMIT -1 as "no limit", so a negative or zero limit must
    // floor at 1 rather than dumping the whole table (LIF-141 class).
    #[test]
    fn page_floors_zero_and_negative_limits_at_one() {
        assert_eq!(page(Some(0), None).0, 1);
        assert_eq!(page(Some(-1), None).0, 1);
        assert_eq!(page(Some(i64::MIN), None).0, 1);
    }

    #[test]
    fn page_caps_oversized_limits() {
        assert_eq!(page(Some(MAX_PAGE_LIMIT + 1), None).0, MAX_PAGE_LIMIT);
        assert_eq!(page(Some(i64::MAX), None).0, MAX_PAGE_LIMIT);
    }

    #[test]
    fn page_floors_negative_offsets_at_zero() {
        assert_eq!(page(None, Some(-10)).1, 0);
        assert_eq!(page(None, Some(i64::MIN)).1, 0);
    }

    // ── page_with ─────────────────────────────────────────────

    #[test]
    fn page_with_honours_a_query_specific_default_and_cap() {
        assert_eq!(page_with(None, None, 20, 200), (20, 0));
        assert_eq!(page_with(Some(999), None, 20, 200).0, 200);
        assert_eq!(page_with(Some(0), Some(-3), 20, 200), (1, 0));
    }

    // ── page_unbounded ────────────────────────────────────────

    #[test]
    fn page_unbounded_treats_an_absent_limit_as_no_limit() {
        assert_eq!(page_unbounded(None, None), (NO_LIMIT, 0));
        assert_eq!(page_unbounded(None, Some(7)), (NO_LIMIT, 7));
    }

    #[test]
    fn page_unbounded_clamps_a_supplied_limit_like_page() {
        assert_eq!(page_unbounded(Some(10), None).0, 10);
        assert_eq!(page_unbounded(Some(0), None).0, 1);
        assert_eq!(page_unbounded(Some(-5), None).0, 1);
        assert_eq!(page_unbounded(Some(i64::MAX), None).0, MAX_PAGE_LIMIT);
        assert_eq!(page_unbounded(Some(5), Some(-5)).1, 0);
    }

    #[test]
    fn unescape_text_preserves_literal_newline_escape_in_multiline_content() {
        let input = "```c\nprintf(\"\\n\");\n```";

        assert_eq!(unescape_text(input), input);
    }

    #[test]
    fn unescape_text_preserves_literal_tab_escape_when_input_has_real_tab() {
        let input = "column\tprintf(\"\\t\");";

        assert_eq!(unescape_text(input), input);
    }

    #[test]
    fn unescape_text_repairs_single_line_double_escaped_newline() {
        assert_eq!(unescape_text("line1\\nline2"), "line1\nline2");
    }
}
