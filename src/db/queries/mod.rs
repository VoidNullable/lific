pub(crate) mod activity;
pub(crate) mod attachments;
pub(crate) mod comments;
pub(crate) mod insights;
mod issues;
pub(crate) mod members;
mod pages;
pub(crate) mod plans;
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
    use super::unescape_text;

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
