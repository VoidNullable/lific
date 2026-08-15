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

use std::io::IsTerminal;

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

    let _ = write!(writer, "{prompt} [y/N] ");
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
    let _ = write!(writer, "{prompt} ");
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
        assert!(err.contains("interactive"), "error should explain why: {err}");
    }

    #[test]
    fn confirm_reads_yes_on_tty() {
        let mut input: &[u8] = b"y\n";
        let mut out: Vec<u8> = Vec::new();
        let ok = confirm_inner("Proceed?", "--yes", true, &mut input, &mut out).unwrap();
        assert!(ok);
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
        let err = prompt_text_inner("What's your name?", "--name", false, &mut input, &mut Vec::new())
            .expect_err("must refuse when stdin is not a TTY");
        assert!(
            err.contains("--name"),
            "error should name the bypass flag, got: {err}"
        );
        assert!(err.contains("interactive"), "error should explain why: {err}");
    }

    #[test]
    fn prompt_text_trims_response_on_tty() {
        let mut input: &[u8] = b"  Blake Alston  \n";
        let value =
            prompt_text_inner("What's your name?", "--name", true, &mut input, &mut Vec::new())
                .unwrap();
        assert_eq!(value, "Blake Alston");
    }

    #[test]
    fn prompt_text_rejects_empty_input() {
        let mut input: &[u8] = b"\n";
        let err =
            prompt_text_inner("What's your name?", "--name", true, &mut input, &mut Vec::new())
                .expect_err("empty input must fail");
        assert!(err.contains("empty"), "error should explain why: {err}");
    }
}
