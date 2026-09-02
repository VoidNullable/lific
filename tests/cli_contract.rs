//! Characterization tests for the CLI's process-level contract.
//!
//! These intentionally execute the binary instead of calling parser internals:
//! the overhaul must preserve exit status and stdout/stderr routing as well as
//! the parsed command shape.

use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;

fn lific(args: &[&str]) -> assert_cmd::assert::Assert {
    cargo_bin_cmd!("lific").args(args).assert()
}

#[test]
fn help_contract_exposes_the_stable_cli_surface() {
    let mut assertion = lific(&["--help"])
        .success()
        .code(0)
        .stderr(predicate::str::is_empty());
    for expected in [
        "Usage: lific [OPTIONS] <COMMAND>",
        "Commands:\n  start       ",
        "  mcp         ",
        "  login       ",
        "  logout      ",
        "  doctor      ",
        "  connect     ",
        "  completion  ",
        "      --config <CONFIG>\n",
        "      --db <DB>\n",
        "      --json\n",
        "      --backend <BACKEND>\n",
        "      --url <URL>\n",
        "      --api-key <API_KEY>\n",
    ] {
        assertion = assertion.stdout(predicate::str::contains(expected));
    }
}

#[test]
fn version_contract_is_stdout_only() {
    lific(&["--version"])
        .success()
        .code(0)
        .stdout(predicate::str::starts_with("lific "))
        .stderr(predicate::str::is_empty());
}

#[test]
fn completion_contract_is_stdout_only_and_contains_the_program_name() {
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        lific(&["completion", shell])
            .success()
            .code(0)
            .stdout(predicate::str::contains("lific"))
            .stderr(predicate::str::is_empty());
    }
}

#[test]
fn invalid_subcommands_never_emit_stdout() {
    for argument in ["unknown", "unknown-command", "project?", "--not-a-command"] {
        lific(&[argument])
            .failure()
            .code(2)
            .stdout(predicate::str::is_empty())
            .stderr(predicate::str::is_empty().not());
    }
}
