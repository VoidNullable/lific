//! Characterization tests for the CLI's process-level contract.
//!
//! These intentionally execute the binary instead of calling parser internals:
//! the overhaul must preserve exit status and stdout/stderr routing as well as
//! the parsed command shape.

use std::path::Path;

use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use rusqlite::Connection;

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

#[test]
fn doctor_process_contract_requires_explicit_repair() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("legacy.db");
    let config_path = tmp.path().join("lific.toml");
    std::fs::write(&config_path, "[backup]\nenabled = false\n").unwrap();
    Connection::open(&db_path)
        .unwrap()
        .execute("CREATE TABLE marker (value TEXT NOT NULL)", [])
        .unwrap();

    let db = db_path.to_str().unwrap();
    let config = config_path.to_str().unwrap();
    lific(&["--config", config, "--db", db, "--json", "doctor"])
        .failure()
        .code(1)
        .stdout(predicate::str::contains("\"status\": \"fail\""));
    assert!(!migration_table_exists(&db_path));

    lific(&[
        "--config", config, "--db", db, "--json", "doctor", "--repair",
    ])
    .success()
    .stdout(predicate::str::contains("\"status\": \"pass\""));
    assert!(migration_table_exists(&db_path));
}

#[test]
fn doctor_process_contract_honors_database_override_after_config_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("override.db");
    let valid_config = tmp.path().join("valid.toml");
    let missing_config = tmp.path().join("missing.toml");
    std::fs::write(&valid_config, "[backup]\nenabled = false\n").unwrap();
    Connection::open(&db_path)
        .unwrap()
        .execute("CREATE TABLE marker (value TEXT NOT NULL)", [])
        .unwrap();

    let db = db_path.to_str().unwrap();
    lific(&[
        "--config",
        valid_config.to_str().unwrap(),
        "--db",
        db,
        "--json",
        "doctor",
        "--repair",
    ])
    .success();

    let output = cargo_bin_cmd!("lific")
        .current_dir(tmp.path())
        .args([
            "--config",
            missing_config.to_str().unwrap(),
            "--db",
            db,
            "--json",
            "doctor",
            "--key",
            "unused-test-key",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("doctor:"), "stderr: {stderr}");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let check = |name: &str| {
        report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["name"] == name)
            .unwrap()
    };

    assert_eq!(check("config")["status"], "fail");
    assert_eq!(check("database")["status"], "pass");
    assert!(check("database")["detail"].as_str().unwrap().contains(db));
    assert!(!tmp.path().join("lific.db").exists());
}

fn migration_table_exists(path: &Path) -> bool {
    Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_migrations'",
            [],
            |_| Ok(()),
        )
        .is_ok()
}
