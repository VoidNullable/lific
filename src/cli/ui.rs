//! Human-facing terminal output conventions, built on `cliclack` (a Rust
//! implementation of the @clack/prompts look: `┌` intro, `│` gutter, `◇`
//! completed steps, `◆` active prompts, `└` outro).
//!
//! Rules of engagement:
//!
//! - **Only the human path uses this module.** Every command decides
//!   human-vs-JSON via [`crate::cli::term::wants_json`] first; JSON output and
//!   non-TTY fail-fast behavior are untouched. cliclack's underlying `console`
//!   crate additionally strips styling when stdout isn't a terminal, so even a
//!   mis-routed call degrades to plain text rather than ANSI soup.
//! - **Rendering failures are ignored.** These wrappers return `()`, not
//!   `io::Result`: pretty output must never turn a succeeded command into a
//!   failed one because a write to a weird terminal failed.
//! - **One vocabulary.** `step` for completed work, `info` for neutral facts,
//!   `warn`/`error` for problems, `note` for blocks the user must read (keys,
//!   snippets, next steps), `intro`/`outro` bracketing every session.

/// Begin a command session: prints the `┌ <title>` header.
pub fn intro(title: &str) {
    let _ = cliclack::intro(console::style(format!(" {title} ")).on_cyan().black().to_string());
}

/// A completed step: `◇ <msg>`.
pub fn step(msg: impl std::fmt::Display) {
    let _ = cliclack::log::success(msg);
}

/// A neutral informational line: `● <msg>`.
pub fn info(msg: impl std::fmt::Display) {
    let _ = cliclack::log::info(msg);
}

/// A warning line: `▲ <msg>`.
pub fn warn(msg: impl std::fmt::Display) {
    let _ = cliclack::log::warning(msg);
}

/// An error line: `■ <msg>`.
pub fn error(msg: impl std::fmt::Display) {
    let _ = cliclack::log::error(msg);
}

/// A skipped/dimmed line: `◌ <msg>` (rendered via a plain step with dim text).
pub fn skipped(msg: impl std::fmt::Display) {
    let _ = cliclack::log::step(console::style(msg).dim().to_string());
}

/// A boxed note block with a title — for content the user must actually read
/// (API keys, manual snippets, next steps).
pub fn note(title: impl std::fmt::Display, body: impl std::fmt::Display) {
    let _ = cliclack::note(title, body);
}

/// End the session on a success: `└ <msg>`.
pub fn outro(msg: impl std::fmt::Display) {
    let _ = cliclack::outro(msg);
}

/// End the session on a failure: `└ <msg>` in red.
pub fn outro_cancel(msg: impl std::fmt::Display) {
    let _ = cliclack::outro_cancel(msg);
}

/// Style helper: dim secondary text (paths, hints) consistently.
pub fn dim(s: impl std::fmt::Display) -> String {
    console::style(s).dim().to_string()
}

/// Style helper: emphasize a command the user should run.
pub fn command(s: impl std::fmt::Display) -> String {
    console::style(s).cyan().to_string()
}
