//! Characterization tests for the CLI's process-level contract.
//!
//! These intentionally execute the binary instead of calling parser internals:
//! the overhaul must preserve exit status and stdout/stderr routing as well as
//! the parsed command shape.

use std::process::{Command, Output};

use proptest::prelude::*;

fn lific(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lific"))
        .args(args)
        .output()
        .expect("the lific test binary should be runnable")
}

#[test]
fn help_contract_exposes_the_stable_cli_surface() {
    let output = lific(&["--help"]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let help = String::from_utf8(output.stdout).unwrap();
    for expected in [
        "Usage:",
        "Commands:",
        "start",
        "mcp",
        "login",
        "logout",
        "doctor",
        "connect",
        "completion",
        "--config",
        "--json",
    ] {
        assert!(help.contains(expected), "help is missing {expected:?}");
    }
}

#[test]
fn version_contract_is_stdout_only() {
    let output = lific(&["--version"]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .starts_with("lific ")
    );
}

#[test]
fn completion_contract_is_stdout_only_and_contains_the_program_name() {
    let output = lific(&["completion", "bash"]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(String::from_utf8(output.stdout).unwrap().contains("lific"));
}

proptest! {
    #[test]
    fn invalid_subcommands_never_emit_stdout(argument in "[a-z]{1,16}") {
        let invalid = format!("unknown-{argument}");
        let output = lific(&[invalid.as_str()]);

        prop_assert!(!output.status.success());
        prop_assert!(output.stdout.is_empty());
        prop_assert!(!output.stderr.is_empty());
    }
}
