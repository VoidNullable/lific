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

// Re-export everything so callers don't need to know the internal split.
pub use issues::*;
pub use pages::*;
pub use projects::*;
pub use resources::*;
pub use search::*;
// users module is accessed via queries::users:: directly (not wildcard re-exported)
// to keep the namespace clean — user functions are only used by auth/CLI code.
