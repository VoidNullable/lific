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
