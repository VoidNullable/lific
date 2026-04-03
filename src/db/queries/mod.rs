mod issues;
mod pages;
mod projects;
mod resources;
mod search;

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
