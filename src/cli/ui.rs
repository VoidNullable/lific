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

use std::fmt::{self, Display};

/// Begin a command session: prints the `┌ <title>` header.
pub fn intro(title: &str) {
    let _ = cliclack::intro(
        console::style(format!(" {} ", title.terminal_line()))
            .on_cyan()
            .black()
            .to_string(),
    );
}

/// A completed step: `◇ <msg>`.
pub fn step(msg: impl std::fmt::Display) {
    let _ = cliclack::log::success(msg.terminal_line());
}

/// A neutral informational line: `● <msg>`.
pub fn info(msg: impl std::fmt::Display) {
    let _ = cliclack::log::info(msg.terminal_line());
}

/// A warning line: `▲ <msg>`.
pub fn warn(msg: impl std::fmt::Display) {
    let _ = cliclack::log::warning(msg.terminal_line());
}

/// An error line: `■ <msg>`.
pub fn error(msg: impl std::fmt::Display) {
    let _ = cliclack::log::error(msg.terminal_line());
}

/// A skipped/dimmed line: `◌ <msg>` (rendered via a plain step with dim text).
pub fn skipped(msg: impl std::fmt::Display) {
    let _ = cliclack::log::step(console::style(msg.terminal_line()).dim().to_string());
}

/// A boxed note block with a title — for content the user must actually read
/// (API keys, manual snippets, next steps).
pub fn note(title: impl std::fmt::Display, body: impl std::fmt::Display) {
    let _ = cliclack::note(title.terminal_line(), body.terminal_block());
}

/// End the session on a success: `└ <msg>`.
pub fn outro(msg: impl std::fmt::Display) {
    let _ = cliclack::outro(msg.terminal_line());
}

/// End the session on a failure: `└ <msg>` in red.
pub fn outro_cancel(msg: impl std::fmt::Display) {
    let _ = cliclack::outro_cancel(msg.terminal_line());
}

/// A plain human-readable line for commands that intentionally do not use a
/// cliclack session.
pub fn line(msg: impl std::fmt::Display) {
    println!("{}", msg.terminal_line());
}

/// A plain sanitized human-readable line for diagnostics that belong on stderr.
pub fn stderr_line(msg: impl std::fmt::Display) {
    eprintln!("{}", msg.terminal_line());
}

/// Sanitize secondary text (paths, hints) for composition into a UI message.
///
/// Styling is deliberately applied only by the final cliclack call. Returning
/// ANSI from a composable string would make the outer terminal sanitizer
/// display escape bytes literally.
pub fn dim(s: impl std::fmt::Display) -> String {
    s.terminal_line().to_string()
}

/// Sanitize a command for composition into a UI message.
pub fn command(s: impl std::fmt::Display) -> String {
    s.terminal_line().to_string()
}

pub(crate) fn sanitize_terminal_line(s: &str) -> String {
    s.terminal_line().to_string()
}

pub(crate) fn sanitize_terminal_block(s: &str) -> String {
    s.terminal_block().to_string()
}

pub(crate) trait TerminalDisplay: Display + Sized {
    fn terminal_line(self) -> impl Display {
        Terminal {
            value: self,
            preserve_layout: false,
        }
    }

    fn terminal_block(self) -> impl Display {
        Terminal {
            value: self,
            preserve_layout: true,
        }
    }
}

impl<T: Display> TerminalDisplay for T {}

struct Terminal<T> {
    value: T,
    preserve_layout: bool,
}

impl<T: Display> Display for Terminal<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = if formatter.alternate() {
            format!("{:#}", self.value)
        } else {
            self.value.to_string()
        };
        formatter.pad(&sanitize_terminal_text(&value, self.preserve_layout))
    }
}

fn sanitize_terminal_text(input: &str, preserve_layout: bool) -> String {
    let mut output = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\n' | '\t' if preserve_layout => output.push(ch),
            '\x1b' => output.push_str("^["),
            ch if ch.is_control()
                || matches!(
                    ch,
                    '\u{061c}'
                        | '\u{200b}'..='\u{200f}'
                        | '\u{2028}'..='\u{202e}'
                        | '\u{2060}'
                        | '\u{2066}'..='\u{206f}'
                        | '\u{feff}'
                ) =>
            {
                output.push(' ');
            }
            _ => output.push(ch),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::TerminalDisplay;
    use proptest::prelude::*;

    fn terminal_formatting_control(ch: char) -> bool {
        ch.is_control()
            || matches!(
                ch,
                '\u{061c}'
                    | '\u{200b}'..='\u{200f}'
                    | '\u{2028}'..='\u{202e}'
                    | '\u{2060}'
                    | '\u{2066}'..='\u{206f}'
                    | '\u{feff}'
            )
    }

    #[test]
    fn block_controls_are_neutralized_without_flattening_layout() {
        assert_eq!(
            "name\x1b[2J\r\x07\u{85}\u{202e}\u{2066}\nnext\tline\x7f"
                .terminal_block()
                .to_string(),
            "name^[[2J     \nnext\tline "
        );
    }

    #[test]
    fn line_controls_cannot_forge_another_status_line() {
        assert_eq!(
            "title\n[ok]\tuser\u{061c}\u{200f}\u{2028}\u{206f}"
                .terminal_line()
                .to_string(),
            "title [ok] user    "
        );
    }

    #[test]
    fn line_values_neutralize_osc8_c1_csi_and_backspace() {
        let rendered = "label\x1b]8;;https://evil\x1b\\click\x1b]8;;\x1b\\\u{009b}2J\u{0008}"
            .terminal_line()
            .to_string();

        assert_eq!(rendered, "label^[]8;;https://evil^[\\click^[]8;;^[\\ 2J ");
        assert!(!rendered.chars().any(char::is_control));
    }

    proptest! {
        #[test]
        fn line_sanitization_never_emits_terminal_controls(
            input in proptest::collection::vec(any::<char>(), 0..256)
                .prop_map(String::from_iter)
        ) {
            let rendered = input.terminal_line().to_string();
            prop_assert!(rendered.chars().all(|ch| !terminal_formatting_control(ch)));
        }

        #[test]
        fn block_sanitization_only_preserves_layout_controls(
            input in proptest::collection::vec(any::<char>(), 0..256)
                .prop_map(String::from_iter)
        ) {
            let rendered = input.terminal_block().to_string();
            let safe = rendered.chars().all(|ch| {
                !terminal_formatting_control(ch) || ch == '\n' || ch == '\t'
            });
            prop_assert!(safe);
        }
    }
}
