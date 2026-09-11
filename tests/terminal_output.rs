use std::path::Path;
use std::process::{Command, Stdio};

fn cli(directory: &Path, config: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lific"));
    command
        .current_dir(directory)
        .env_remove("LIFIC_URL")
        .env_remove("LIFIC_TOKEN")
        .env_remove("LIFIC_API_KEY")
        .stdin(Stdio::null())
        .arg("--config")
        .arg(config);
    command
}

#[test]
fn init_database_warning_is_safe_in_explicit_and_automatic_json_modes() {
    for explicit_json in [false, true] {
        let scratch = tempfile::tempdir().unwrap();
        let config = scratch.path().join("selected.toml");
        let database = scratch.path().join("override\u{009b}2J\u{202e}.db");
        std::fs::write(&config, "[database]\npath = \"configured.db\"\n").unwrap();

        let mut command = cli(scratch.path(), &config);
        command.arg("--db").arg(&database).args([
            "init",
            "--no-service",
            "--name",
            "operator",
            "--auth-mode",
            "login-free",
        ]);
        if explicit_json {
            command.arg("--json");
        }
        let output = command.output().unwrap();
        assert!(output.status.success(), "{output:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("warning: --db points at"), "{stderr:?}");
        assert!(
            stderr
                .chars()
                .all(|ch| (!ch.is_control() || ch == '\n') && ch != '\u{202e}'),
            "{stderr:?}"
        );
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(json.is_object());
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(!stdout.contains(['\u{009b}', '\u{202e}']), "{stdout:?}");
    }
}

#[test]
fn clap_output_cannot_render_terminal_controls_from_arguments_or_environment() {
    let binary = env!("CARGO_BIN_EXE_lific");
    let output = Command::new(binary)
        .arg("bad\nSUCCESS: forged\u{202e}")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains("\nSUCCESS: forged"), "{stderr:?}");
    assert!(!stderr.contains('\u{202e}'), "{stderr:?}");

    let output = Command::new(binary)
        .env("LIFIC_URL", "https://host/\u{202e}forged")
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains('\u{202e}'), "{stdout:?}");

    let output = Command::new(binary)
        .env("LIFIC_URL", "https://host/secret\nFORGED URL")
        .env("LIFIC_API_KEY", "api-secret\t\u{009b}2J\u{202e}")
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("https://host/secret"), "{stdout:?}");
    assert!(!stdout.contains("FORGED URL"), "{stdout:?}");
    assert!(!stdout.contains("api-secret"), "{stdout:?}");
    assert!(stdout.contains("Usage:"), "{stdout:?}");

    let output = Command::new(binary).arg("--version").output().unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[cfg(unix)]
#[test]
fn clap_parse_failure_uses_original_arguments_without_forged_lines() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::process::CommandExt;

    let output = Command::new(env!("CARGO_BIN_EXE_lific"))
        .arg0("lific\nFORGED STATUS")
        .args([
            OsString::from("--url"),
            OsString::from_vec(vec![0xff]),
            OsString::from("project"),
            OsString::from("list"),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("\nFORGED STATUS"), "{stderr:?}");
    assert!(!stderr.contains("FORGED STATUS\n"), "{stderr:?}");
}

#[test]
fn import_help_hides_environment_values() {
    let binary = env!("CARGO_BIN_EXE_lific");
    let cases = vec![
        (
            "github",
            vec![("GITHUB_TOKEN", "github-secret\u{1b}[2J")],
            vec!["github-secret\u{1b}[2J", "FORGED JIRA STATUS"],
        ),
        (
            "linear",
            vec![("LINEAR_API_KEY", "linear-secret\u{202e}")],
            vec!["linear-secret\u{202e}", "FORGED JIRA STATUS"],
        ),
        (
            "jira",
            vec![
                ("JIRA_EMAIL", "jira@example.test\nFORGED JIRA STATUS"),
                ("JIRA_API_TOKEN", "jira-token-secret"),
            ],
            vec![
                "jira@example.test\nFORGED JIRA STATUS",
                "jira-token-secret",
                "FORGED JIRA STATUS",
            ],
        ),
    ];

    for (provider, variables, secrets) in cases {
        let mut command = Command::new(binary);
        for (variable, value) in variables {
            command.env(variable, value);
        }
        let output = command
            .args(["import", provider, "--help"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{provider}: {output:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        for secret in secrets {
            assert!(!stdout.contains(secret), "{provider}: {stdout:?}");
        }
        assert!(stdout.contains("Usage:"), "{provider}: {stdout:?}");
        assert!(stdout.contains("--"), "{provider}: {stdout:?}");
    }
}

#[test]
fn invalid_log_filter_is_reported_safely_before_tracing_starts() {
    let scratch = tempfile::tempdir().unwrap();
    let config = scratch.path().join("lific.toml");
    std::fs::write(
        &config,
        "[log]\nlevel = \"not-a-level\\u001B[2J\\nFORGED LOG STATUS\"\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_lific"))
        .env_remove("RUST_LOG")
        .args([
            "--config",
            config.to_str().unwrap(),
            "mcp",
            "--remote",
            "--url",
            "http://127.0.0.1:1",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains('\u{001b}'), "{stderr:?}");
    assert!(!stderr.contains('\u{009b}'), "{stderr:?}");
    assert!(!stderr.contains("\nFORGED LOG STATUS"), "{stderr:?}");
    assert!(stderr.contains("not-a-level"), "{stderr:?}");
}

#[test]
fn valid_log_levels_and_rust_log_precedence_are_preserved() {
    let scratch = tempfile::tempdir().unwrap();
    let config = scratch.path().join("lific.toml");
    let run = |level: &str, rust_log: Option<&str>| {
        std::fs::write(&config, format!("[log]\nlevel = \"{level}\"\n")).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_lific"));
        command.env_remove("RUST_LOG");
        if let Some(rust_log) = rust_log {
            command.env("RUST_LOG", rust_log);
        }
        command
            .args([
                "--config",
                config.to_str().unwrap(),
                "mcp",
                "--remote",
                "--url",
                "http://127.0.0.1:1",
            ])
            .output()
            .unwrap()
    };

    for level in ["OFF", "InFo"] {
        let output = run(level, None);
        assert!(
            matches!(output.status.code(), Some(0 | 1)),
            "{level}: {output:?}"
        );
        assert!(!String::from_utf8_lossy(&output.stderr).contains("invalid configured log level"));
    }

    let output = run("not-a-level", Some("off"));
    assert!(matches!(output.status.code(), Some(0 | 1)), "{output:?}");
    assert!(!String::from_utf8_lossy(&output.stderr).contains("invalid configured log level"));
}

#[cfg(unix)]
#[test]
fn clap_early_output_ignores_closed_pipes() {
    use std::fs::File;
    use std::os::fd::{FromRawFd, RawFd};

    unsafe fn closed_reader_pipe() -> File {
        let mut fds = [0 as RawFd; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        assert_eq!(unsafe { libc::close(fds[0]) }, 0);
        unsafe { File::from_raw_fd(fds[1]) }
    }

    for (args, expected_code) in [(["--help"], 0), (["not-a-command"], 2)] {
        let output_fd = unsafe { closed_reader_pipe() };
        let output = Command::new(env!("CARGO_BIN_EXE_lific"))
            .args(args)
            .stdout(Stdio::from(output_fd.try_clone().unwrap()))
            .stderr(Stdio::from(output_fd))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(expected_code), "{output:?}");
    }
}

#[cfg(unix)]
#[test]
fn clap_help_cannot_render_a_terminal_control_in_the_program_name() {
    use std::os::unix::process::CommandExt;

    for argv0 in [
        "lific\nFORGED_BARE",
        "./lific\tFORGED_RELATIVE",
        "/tmp/lific\u{1b}[2J\u{202e}FORGED_ABSOLUTE",
    ] {
        for args in [&["--help"][..], &["project", "--help"][..]] {
            let output = Command::new(env!("CARGO_BIN_EXE_lific"))
                .arg0(argv0)
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{argv0:?} {args:?}: {output:?}");
            assert!(output.stderr.is_empty(), "{argv0:?} {args:?}: {output:?}");
            let stdout = String::from_utf8(output.stdout).unwrap();
            assert!(stdout.contains("Usage: lific"), "{argv0:?}: {stdout:?}");
            assert!(!stdout.contains("FORGED_"), "{argv0:?}: {stdout:?}");
            assert!(!stdout.contains('\u{1b}'), "{argv0:?}: {stdout:?}");
            assert!(!stdout.contains('\u{202e}'), "{argv0:?}: {stdout:?}");
        }
    }
}

#[cfg(unix)]
#[test]
fn clap_diagnostics_preserve_their_original_scope_and_layout() {
    let nested = Command::new(env!("CARGO_BIN_EXE_lific"))
        .arg("project")
        .output()
        .unwrap();
    assert_eq!(nested.status.code(), Some(2), "{nested:?}");
    let nested_stderr = String::from_utf8(nested.stderr).unwrap();
    assert!(
        nested_stderr.contains("\n\nUsage: lific project [OPTIONS] <COMMAND>"),
        "{nested_stderr:?}"
    );
    assert!(nested_stderr.contains("\n\nCommands:"), "{nested_stderr:?}");
    assert!(!nested_stderr.contains("Usage: lific [OPTIONS] <COMMAND>"));

    let implicit_help = Command::new(env!("CARGO_BIN_EXE_lific")).output().unwrap();
    assert_eq!(implicit_help.status.code(), Some(2), "{implicit_help:?}");
    assert!(implicit_help.stdout.is_empty(), "{implicit_help:?}");
    let implicit_stderr = String::from_utf8(implicit_help.stderr).unwrap();
    assert!(
        implicit_stderr.contains("\n\nUsage: lific [OPTIONS] <COMMAND>"),
        "{implicit_stderr:?}"
    );
    assert!(
        implicit_stderr.contains("\n\nCommands:"),
        "{implicit_stderr:?}"
    );
}

#[cfg(unix)]
#[test]
fn clap_help_preserves_layout_without_forged_program_name_lines() {
    use std::os::unix::process::CommandExt;

    let output = Command::new(env!("CARGO_BIN_EXE_lific"))
        .arg0("lific\nFORGED_DIAGNOSTIC")
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains("\nFORGED_DIAGNOSTIC"), "{stdout:?}");
    assert!(stdout.contains("\nUsage:"), "{stdout:?}");
}

#[cfg(target_os = "linux")]
#[test]
fn service_status_does_not_leak_subprocess_streams_into_json_or_stderr() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = tempfile::tempdir().unwrap();
    let config = scratch.path().join("selected.toml");
    std::fs::write(&config, "").unwrap();
    let systemctl = scratch.path().join("systemctl");
    std::fs::write(
        &systemctl,
        "#!/bin/sh\nprintf '\\033]52;c;YQ==\\007'\nprintf '\\033[2J' >&2\nexit 0\n",
    )
    .unwrap();
    std::fs::set_permissions(&systemctl, std::fs::Permissions::from_mode(0o700)).unwrap();

    for explicit_json in [false, true] {
        let mut command = cli(scratch.path(), &config);
        command
            .env("PATH", scratch.path())
            .env("HOME", scratch.path())
            .args(["service", "status"]);
        if explicit_json {
            command.arg("--json");
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["active"], true);
    }
}
