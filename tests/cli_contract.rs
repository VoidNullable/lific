//! Characterization tests for the CLI's process-level contract.
//!
//! These intentionally execute the binary instead of calling parser internals:
//! the overhaul must preserve exit status and stdout/stderr routing as well as
//! the parsed command shape.

use std::path::Path;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use rusqlite::Connection;

fn private_tempdir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        let mut perms = std::fs::metadata(dir.path()).unwrap().permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(dir.path(), perms).unwrap();
    }
    dir
}

fn lific_command() -> assert_cmd::Command {
    let mut command = cargo_bin_cmd!("lific");
    configure_command(&mut command);
    command
}

fn configure_command(command: &mut assert_cmd::Command) {
    command.env_clear();
    // Retain only platform/runtime loader configuration, including Cargo's
    // library search path. Application settings and proxies stay isolated.
    for name in [
        #[cfg(windows)]
        "PATH",
        "SystemRoot",
        "WINDIR",
        "LD_LIBRARY_PATH",
        "DYLD_LIBRARY_PATH",
        "DYLD_FALLBACK_LIBRARY_PATH",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .timeout(Duration::from_secs(20))
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("COLUMNS", "120");
}

fn doctor_command(dir: &Path) -> assert_cmd::Command {
    let mut command = lific_command();
    command.current_dir(dir);
    for name in [
        "HOME",
        "USERPROFILE",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "APPDATA",
        "LOCALAPPDATA",
    ] {
        command.env(name, dir);
    }
    // Never look up the operator's login token in the system keyring.
    command.env("LIFIC_API_KEY", "unused-contract-test-key");
    command
}

#[test]
fn command_environment_does_not_inherit_credentials_or_proxies() {
    let mut command = cargo_bin_cmd!("lific");
    command.env("LIFIC_TOKEN", "fixture-token");
    command.env("HTTPS_PROXY", "http://proxy.invalid:1234");
    configure_command(&mut command);
    for name in ["LIFIC_TOKEN", "HTTPS_PROXY"] {
        assert!(
            command
                .get_envs()
                .all(|(key, value)| key != name || value.is_none())
        );
    }
}

fn lific(args: &[&str]) -> assert_cmd::assert::Assert {
    lific_command().args(args).assert()
}

#[test]
fn help_contract_exposes_the_stable_cli_surface() {
    let assertion = lific(&["--help"])
        .success()
        .stderr(predicate::str::is_empty())
        .stdout(predicate::str::contains("Usage: lific"));
    let stdout =
        std::str::from_utf8(&assertion.get_output().stdout).expect("help must be valid UTF-8");

    for command in [
        "start",
        "mcp",
        "login",
        "logout",
        "doctor",
        "connect",
        "completion",
    ] {
        assert!(
            stdout
                .lines()
                .any(|line| { line.split_whitespace().next() == Some(command) }),
            "missing command {command:?} in help:\n{stdout}"
        );
    }

    for option in [
        "--config",
        "--db",
        "--json",
        "--backend",
        "--url",
        "--api-key",
    ] {
        assert!(
            stdout.lines().any(|line| {
                let line = line.trim_start();
                line == option
                    || line
                        .strip_prefix(option)
                        .is_some_and(|rest| rest.starts_with([' ', '\t']))
            }),
            "missing option {option:?} in help:\n{stdout}"
        );
    }
}

#[test]
fn version_contract_is_stdout_only() {
    lific(&["--version"])
        .success()
        .stdout(format!("lific {}\n", env!("CARGO_PKG_VERSION")))
        .stderr(predicate::str::is_empty());
}

#[test]
fn completion_contract_is_stdout_only_and_contains_the_program_name() {
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        lific(&["completion", shell])
            .success()
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
    let tmp = private_tempdir();
    let db_path = tmp.path().join("legacy.db");
    let config_path = tmp.path().join("lific.toml");
    std::fs::write(
        &config_path,
        "[server]\nhost = '127.0.0.1'\nport = 0\n[backup]\nenabled = false\n",
    )
    .unwrap();
    Connection::open(&db_path)
        .unwrap()
        .execute("CREATE TABLE marker (value TEXT NOT NULL)", [])
        .unwrap();

    let db = db_path.to_str().unwrap();
    let config = config_path.to_str().unwrap();
    let before = std::fs::read(&db_path).unwrap();
    let assertion = doctor_command(tmp.path())
        .args(["--config", config, "--db", db, "--json", "doctor"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("doctor:"));
    let report: serde_json::Value = serde_json::from_slice(&assertion.get_output().stdout).unwrap();
    assert_eq!(report["ok"], false);
    assert_eq!(std::fs::read(&db_path).unwrap(), before);
    assert!(!migration_table_exists(&db_path));

    let assertion = doctor_command(tmp.path())
        .args([
            "--config", config, "--db", db, "--json", "doctor", "--repair",
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let report: serde_json::Value = serde_json::from_slice(&assertion.get_output().stdout).unwrap();
    assert_eq!(report["ok"], true);
    assert!(migration_table_exists(&db_path));
}

#[test]
fn doctor_process_contract_honors_database_override_after_config_failure() {
    let tmp = private_tempdir();
    let db_path = tmp.path().join("override.db");
    let valid_config = tmp.path().join("valid.toml");
    let missing_config = tmp.path().join("missing.toml");
    std::fs::write(
        &valid_config,
        "[server]\nhost = '127.0.0.1'\nport = 0\n[backup]\nenabled = false\n",
    )
    .unwrap();
    Connection::open(&db_path)
        .unwrap()
        .execute("CREATE TABLE marker (value TEXT NOT NULL)", [])
        .unwrap();

    let db = db_path.to_str().unwrap();
    doctor_command(tmp.path())
        .args([
            "--config",
            valid_config.to_str().unwrap(),
            "--db",
            db,
            "--json",
            "doctor",
            "--repair",
        ])
        .assert()
        .success();
    assert!(migration_table_exists(&db_path));

    let assertion = doctor_command(tmp.path())
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
        .assert()
        .code(1)
        .stderr(predicate::str::contains("doctor:"));
    let report: serde_json::Value = serde_json::from_slice(&assertion.get_output().stdout).unwrap();
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
    assert_eq!(check("server")["status"], "skipped");
    assert_eq!(check("mcp")["status"], "skipped");
    assert!(check("database")["detail"].as_str().unwrap().contains(db));
    assert!(!tmp.path().join("lific.db").exists());
}

fn migration_table_exists(path: &Path) -> bool {
    Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_migrations')",
            [],
            |row| row.get(0),
        )
        .expect("migration lookup must succeed")
}

#[test]
fn invalid_config_never_repairs_an_implicit_database() {
    let tmp = private_tempdir();
    let missing = tmp.path().join("missing.toml");
    for repair in [false, true] {
        let mut command = doctor_command(tmp.path());
        command
            .arg("--config")
            .arg(&missing)
            .args(["--json", "doctor"]);
        if repair {
            command.arg("--repair");
        }
        let assertion = command
            .assert()
            .code(1)
            .stderr(predicate::str::contains("doctor:"));
        let report: serde_json::Value =
            serde_json::from_slice(&assertion.get_output().stdout).unwrap();
        assert_eq!(report["ok"], false);
        for check in report["checks"].as_array().unwrap() {
            let expected = if check["name"] == "config" {
                "fail"
            } else {
                "skipped"
            };
            assert_eq!(check["status"], expected, "{check}");
        }
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
    }
}
