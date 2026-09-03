//! Terminal / TTY awareness for the CLI.
//!
//! Follows the CLI guidelines (clig.dev / clispec.dev): commands should behave
//! well for both humans at an interactive terminal and non-interactive callers
//! (agents, CI, pipes). The two rules encoded here:
//!
//! 1. **Output auto-upgrades to JSON when stdout is not a TTY.** A human at a
//!    terminal gets pretty human output; a pipe (`| jq`, capture in a script,
//!    an agent reading our stdout) gets machine-readable JSON without needing
//!    to remember `--json`. An explicit `--json` always wins.
//! 2. **Interactive prompts refuse rather than hang when stdin is not a TTY.**
//!    A confirmation prompt that blocks forever in CI is a hang; instead we
//!    error and name the flag that bypasses the prompt non-interactively.

use std::io::{self, IsTerminal, Write};

use crate::cli::ui::TerminalDisplay;

/// A tracing writer that makes every formatted event safe for a terminal.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SanitizedStderr;

pub(crate) fn sanitized_stderr() -> SanitizedStderr {
    SanitizedStderr
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SanitizedStderr {
    type Writer = SanitizedWriter<io::Stderr>;

    fn make_writer(&'a self) -> Self::Writer {
        SanitizedWriter {
            inner: io::stderr(),
            pending: Vec::new(),
        }
    }
}

pub(crate) struct SanitizedWriter<W> {
    inner: W,
    pending: Vec<u8>,
}

impl<W: Write> Write for SanitizedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            let text = String::from_utf8_lossy(&line);
            let has_trailing_newline = text.ends_with('\n');
            let text = text.strip_suffix('\n').unwrap_or(&text);
            let safe = crate::cli::ui::sanitize_terminal_line(text);
            self.inner.write_all(safe.as_bytes())?;
            if has_trailing_newline {
                self.inner.write_all(b"\n")?;
            }
        }
        self.inner.flush()
    }
}

/// Serialize JSON for terminal-visible stdout without changing its parsed value.
///
/// Serde escapes C0 controls, but leaves C1 and Unicode formatting controls
/// literal. Escape those only while they are inside JSON strings; structural
/// pretty-print whitespace remains valid JSON formatting.
pub fn json_string<T: serde::Serialize>(value: &T) -> Result<String, serde_json::Error> {
    let serialized = serde_json::to_string_pretty(value)?;
    let mut output = String::with_capacity(serialized.len());
    let mut in_string = false;
    let mut escaped = false;

    for ch in serialized.chars() {
        if !in_string {
            output.push(ch);
            in_string = ch == '"';
            continue;
        }

        if escaped {
            output.push(ch);
            escaped = false;
            continue;
        }

        match ch {
            '\\' => {
                output.push(ch);
                escaped = true;
            }
            '"' => {
                output.push(ch);
                in_string = false;
            }
            ch if json_terminal_control(ch) => {
                use std::fmt::Write;
                write!(output, "\\u{:04x}", ch as u32).expect("String write cannot fail");
            }
            _ => output.push(ch),
        }
    }

    Ok(output)
}

fn json_terminal_control(ch: char) -> bool {
    ch.is_control()
        || ch == '\u{007f}'
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

/// Whether stdout is connected to an interactive terminal.
pub fn stdout_is_tty() -> bool {
    std::io::stdout().is_terminal()
}

/// Whether stdin is connected to an interactive terminal.
pub fn stdin_is_tty() -> bool {
    std::io::stdin().is_terminal()
}

/// Decide whether output should be JSON.
///
/// Explicit `--json` always wins. Otherwise, per clispec.dev, output
/// auto-upgrades to JSON when stdout is piped/redirected (not a TTY) so that
/// scripts and agents get machine-readable output by default.
pub fn wants_json(json_flag: bool) -> bool {
    wants_json_inner(json_flag, stdout_is_tty())
}

/// Pure decision function for [`wants_json`], factored out so the boolean logic
/// is unit-testable without a real terminal.
pub fn wants_json_inner(json_flag: bool, stdout_tty: bool) -> bool {
    json_flag || !stdout_tty
}

/// Ask the user to confirm an action.
///
/// If stdin is not a TTY we cannot prompt, so rather than hang forever we
/// return an error naming `bypass_flag` — the flag a non-interactive caller
/// (agent, CI) should pass to proceed without a prompt.
pub fn confirm(prompt: &str, bypass_flag: &str) -> Result<bool, String> {
    confirm_inner(
        prompt,
        bypass_flag,
        stdin_is_tty(),
        &mut std::io::stdin().lock(),
        &mut std::io::stderr(),
    )
}

/// Pure/injected implementation of [`confirm`], factored out so the non-TTY
/// refusal branch (and the reader/writer plumbing) is testable.
pub fn confirm_inner<R: std::io::BufRead, W: std::io::Write>(
    prompt: &str,
    bypass_flag: &str,
    stdin_tty: bool,
    reader: &mut R,
    writer: &mut W,
) -> Result<bool, String> {
    if !stdin_tty {
        return Err(format!(
            "interactive confirmation required; re-run with {bypass_flag} to proceed non-interactively"
        ));
    }

    let _ = write!(writer, "{} [y/N] ", prompt.terminal_line());
    let _ = writer.flush();

    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|e| format!("failed to read confirmation: {e}"))?;
    let answer = line.trim().to_ascii_lowercase();
    Ok(answer == "y" || answer == "yes")
}

/// Ask the user for a short (single-line) piece of text, e.g. an operator name.
///
/// Same contract as [`confirm`]: refuses, rather than hangs, when stdin is not
/// a TTY, and names `bypass_flag` — the flag a non-interactive caller should
/// pass to supply the value without a prompt. Returns the trimmed input; errors
/// on empty input or no-TTY.
pub fn prompt_text(prompt: &str, bypass_flag: &str) -> Result<String, String> {
    prompt_text_inner(
        prompt,
        bypass_flag,
        stdin_is_tty(),
        &mut std::io::stdin().lock(),
        &mut std::io::stdout(),
    )
}

/// Pure/injected implementation of [`prompt_text`], factored out so the non-TTY
/// refusal branch and the reader plumbing are testable.
pub fn prompt_text_inner<R: std::io::BufRead, W: std::io::Write>(
    prompt: &str,
    bypass_flag: &str,
    stdin_tty: bool,
    reader: &mut R,
    writer: &mut W,
) -> Result<String, String> {
    if !stdin_tty {
        return Err(format!(
            "interactive input required; re-run with {bypass_flag} to supply it non-interactively"
        ));
    }
    let _ = write!(writer, "{} ", prompt.terminal_line());
    let _ = writer.flush();

    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|e| format!("failed to read input: {e}"))?;
    let value = line.trim().to_string();
    if value.is_empty() {
        return Err("input cannot be empty".into());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn assert_json_strings_contain_no_terminal_controls(encoded: &str) {
        let mut in_string = false;
        let mut escaped = false;
        for ch in encoded.chars() {
            if !in_string {
                if ch == '"' {
                    in_string = true;
                }
                continue;
            }
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => assert!(!json_terminal_control(ch), "literal control {ch:?}"),
            }
        }
    }

    #[test]
    fn json_string_escapes_terminal_controls_losslessly() {
        let value = serde_json::json!({
            "text": "\u{001b}]52;c;clipboard\u{0007}\u{009b}2J\u{007f}\u{202e}\u{200b}\u{feff}"
        });
        let encoded = json_string(&value).unwrap();

        assert!(encoded.contains("\\u009b"));
        assert!(encoded.contains("\\u007f"));
        assert!(encoded.contains("\\u202e"));
        assert!(encoded.contains("\\ufeff"));

        let without_layout = encoded.replace('\n', "");
        assert!(!without_layout.chars().any(json_terminal_control));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&encoded).unwrap(),
            value
        );
    }

    proptest! {
        #[test]
        fn json_string_round_trips_arbitrary_text_without_literal_controls(
            text in proptest::collection::vec(any::<char>(), 0..256)
                .prop_map(String::from_iter)
        ) {
            let value = serde_json::json!({"text": text});
            let encoded = json_string(&value).unwrap();

            prop_assert_eq!(
                serde_json::from_str::<serde_json::Value>(&encoded).unwrap(),
                value
            );
            assert_json_strings_contain_no_terminal_controls(&encoded);
        }
    }

    #[test]
    fn wants_json_explicit_flag_always_wins() {
        // --json requested: JSON regardless of TTY state.
        assert!(wants_json_inner(true, true));
        assert!(wants_json_inner(true, false));
    }

    #[test]
    fn wants_json_piped_auto_upgrades() {
        // No flag, stdout is NOT a tty (piped/redirected): auto-JSON.
        assert!(wants_json_inner(false, false));
    }

    #[test]
    fn wants_json_interactive_terminal_stays_human() {
        // No flag, stdout IS a tty: human output.
        assert!(!wants_json_inner(false, true));
    }

    #[test]
    fn confirm_refuses_without_tty_and_names_bypass_flag() {
        let mut input: &[u8] = b"";
        let mut out: Vec<u8> = Vec::new();
        let err = confirm_inner("Delete everything?", "--yes", false, &mut input, &mut out)
            .expect_err("must refuse when stdin is not a TTY");
        assert!(
            err.contains("--yes"),
            "error should name the bypass flag, got: {err}"
        );
        assert!(
            err.contains("interactive"),
            "error should explain why: {err}"
        );
    }

    #[test]
    fn confirm_reads_yes_on_tty() {
        let mut input: &[u8] = b"y\n";
        let mut out: Vec<u8> = Vec::new();
        let ok = confirm_inner("Proceed?", "--yes", true, &mut input, &mut out).unwrap();
        assert!(ok);
    }

    #[test]
    fn prompts_neutralize_terminal_controls() {
        let mut input: &[u8] = b"y\n";
        let mut out: Vec<u8> = Vec::new();
        confirm_inner(
            "Delete \u{001b}]52;c;clipboard\u{0007}?",
            "--yes",
            true,
            &mut input,
            &mut out,
        )
        .unwrap();
        let rendered = String::from_utf8(out).unwrap();
        assert_eq!(rendered, "Delete ^[]52;c;clipboard ? [y/N] ");
        assert!(
            rendered
                .lines()
                .all(|line| !line.chars().any(char::is_control))
        );
    }

    #[test]
    fn tracing_writer_neutralizes_control_sequences() {
        use std::io::Write;

        let mut writer = SanitizedWriter {
            inner: Vec::new(),
            pending: Vec::new(),
        };
        writer
            .write_all("path: evil\nforged\x1b]8;;https://evil\x1b\\\u{009b}2J\n".as_bytes())
            .unwrap();
        writer.flush().unwrap();
        let rendered = String::from_utf8(writer.inner).unwrap();
        assert_eq!(rendered, "path: evil forged^[]8;;https://evil^[\\ 2J\n");
        assert!(
            rendered
                .lines()
                .all(|line| !line.chars().any(char::is_control))
        );
    }

    #[test]
    fn cli_json_sinks_do_not_bypass_terminal_encoder() {
        for path in [
            "src/main.rs",
            "src/cli/exec.rs",
            "src/cli/http.rs",
            "src/cli/instance.rs",
            "src/cli/member.rs",
            "src/cli/user.rs",
            "src/cli/key.rs",
            "src/cli/import.rs",
            "src/cli/doctor.rs",
            "src/cli/login.rs",
            "src/cli/connect/mod.rs",
            "src/cli/connect/writer.rs",
        ] {
            let source = std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path),
            )
            .unwrap();
            assert!(
                !source.contains("serde_json::to_string_pretty"),
                "{path} must use term::json_string for terminal JSON"
            );
        }
    }

    #[test]
    fn confirm_reads_no_on_tty() {
        let mut input: &[u8] = b"n\n";
        let mut out: Vec<u8> = Vec::new();
        let ok = confirm_inner("Proceed?", "--yes", true, &mut input, &mut out).unwrap();
        assert!(!ok);
    }

    #[test]
    fn confirm_empty_input_defaults_to_no() {
        let mut input: &[u8] = b"\n";
        let mut out: Vec<u8> = Vec::new();
        let ok = confirm_inner("Proceed?", "--yes", true, &mut input, &mut out).unwrap();
        assert!(!ok);
    }

    // LIFIC-9: the operator-name prompt shares confirm's non-TTY contract.
    #[test]
    fn prompt_text_refuses_without_tty_and_names_bypass_flag() {
        let mut input: &[u8] = b"";
        let err = prompt_text_inner(
            "What's your name?",
            "--name",
            false,
            &mut input,
            &mut Vec::new(),
        )
        .expect_err("must refuse when stdin is not a TTY");
        assert!(
            err.contains("--name"),
            "error should name the bypass flag, got: {err}"
        );
        assert!(
            err.contains("interactive"),
            "error should explain why: {err}"
        );
    }

    #[test]
    fn prompt_text_trims_response_on_tty() {
        let mut input: &[u8] = b"  Blake Alston  \n";
        let value = prompt_text_inner(
            "What's your name?",
            "--name",
            true,
            &mut input,
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(value, "Blake Alston");
    }

    #[test]
    fn prompt_text_rejects_empty_input() {
        let mut input: &[u8] = b"\n";
        let err = prompt_text_inner(
            "What's your name?",
            "--name",
            true,
            &mut input,
            &mut Vec::new(),
        )
        .expect_err("empty input must fail");
        assert!(err.contains("empty"), "error should explain why: {err}");
    }
}
