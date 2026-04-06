pub(crate) mod comments;
mod issues;
mod pages;
mod projects;
mod resources;
mod search;
pub(crate) mod users;

/// Unescape literal \n and \t sequences that come through JSON transport.
pub(crate) fn unescape_text(s: &str) -> String {
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
pub use issues::*;
pub use pages::*;
pub use projects::*;
pub use resources::*;
pub use search::*;
// users module is accessed via queries::users:: directly (not wildcard re-exported)
// to keep the namespace clean — user functions are only used by auth/CLI code.
