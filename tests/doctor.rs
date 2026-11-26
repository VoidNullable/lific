use std::fmt::Write;
use std::process::Command;

#[test]
fn doctor_only_fails_legacy_urls_identified_as_lific() {
    let scratch = tempfile::tempdir().unwrap();
    let config = scratch.path().join("lific.toml");
    std::fs::write(&config, "[server]\nhost = '127.0.0.1'\nport = 0\n").unwrap();
    for (name, url, expected) in [
        (
            "zapier",
            "https://mcp.zapier.com/api/mcp/s/SECRET/mcp",
            None,
        ),
        ("LiFiC", "https://foreign.example/mcp/SECRET", Some("fail")),
        ("custom", "https://foreign.example/mcp/SECRET", Some("warn")),
    ] {
        std::fs::write(
            scratch.path().join(".mcp.json"),
            serde_json::json!({
                "mcpServers": {name: {"url": url}}
            })
            .to_string(),
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_lific"))
            .current_dir(scratch.path())
            .env("LIFIC_CONNECT_HOME", scratch.path().join("home"))
            .env_remove("LIFIC_URL")
            .env_remove("LIFIC_TOKEN")
            .env_remove("LIFIC_API_KEY")
            .arg("--config")
            .arg(&config)
            .args(["doctor", "--json"])
            .output()
            .unwrap();
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let check = report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["name"] == "client_config");
        assert_eq!(
            check.and_then(|check| check["status"].as_str()),
            expected,
            "{report}"
        );
        assert_eq!(
            output.status.success(),
            expected != Some("fail"),
            "{report}"
        );
        if expected == Some("warn") {
            assert!(
                check.unwrap()["detail"]
                    .as_str()
                    .unwrap()
                    .contains("if this entry points at Lific")
            );
        }
        for bytes in [&output.stdout, &output.stderr] {
            assert!(!String::from_utf8_lossy(bytes).contains("SECRET"));
        }
    }
}

#[test]
fn obsolete_mcp_settings_warn_at_startup_and_in_doctor_without_disclosing_values() {
    for keys in [
        vec!["mcp_path_token"],
        vec!["mcp_path_user"],
        vec!["mcp_path_token", "mcp_path_user"],
    ] {
        let scratch = tempfile::tempdir().unwrap();
        let config = scratch.path().join("lific.toml");
        let mut settings = String::new();
        for key in &keys {
            writeln!(settings, "{key} = 'SECRET_VALUE'").unwrap();
        }
        std::fs::write(
            &config,
            format!("[server]\nhost = '127.0.0.1'\nport = 0\n{settings}"),
        )
        .unwrap();
        let command = || {
            let mut command = Command::new(env!("CARGO_BIN_EXE_lific"));
            command
                .current_dir(scratch.path())
                .env("LIFIC_CONNECT_HOME", scratch.path().join("home"))
                .env("RUST_LOG", "lific=warn")
                .env_remove("LIFIC_URL")
                .env_remove("LIFIC_TOKEN")
                .env_remove("LIFIC_API_KEY")
                .arg("--config")
                .arg(&config);
            command
        };
        let output = command().args(["doctor", "--json"]).output().unwrap();
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let warnings = report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|check| check["name"] == "removed_setting")
            .collect::<Vec<_>>();
        assert_eq!(warnings.len(), keys.len(), "{report}");
        for key in &keys {
            let matching = warnings
                .iter()
                .filter(|check| check["detail"].as_str().unwrap().contains(key))
                .collect::<Vec<_>>();
            assert_eq!(matching.len(), 1);
            assert_eq!(matching[0]["status"], "warn");
            assert!(
                matching[0]["detail"]
                    .as_str()
                    .unwrap()
                    .contains("no longer has any effect and should be removed")
            );
        }
        for bytes in [&output.stdout, &output.stderr] {
            assert!(!String::from_utf8_lossy(bytes).contains("SECRET_VALUE"));
        }

        // Stop startup after logging, before it opens a database or binds a listener.
        std::fs::write(scratch.path().join("lific.db"), []).unwrap();
        std::fs::write(
            &config,
            format!("[server]\ntrusted_proxies = ['invalid-cidr']\n{settings}"),
        )
        .unwrap();
        let output = command().arg("start").output().unwrap();
        assert!(!output.status.success());
        let logs = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(logs.contains("invalid server.trusted_proxies"), "{logs}");
        for key in &keys {
            let message = format!("server.{key} no longer has any effect and should be removed");
            assert_eq!(logs.matches(&message).count(), 1, "{logs}");
        }
        assert!(!logs.contains("SECRET_VALUE"));
    }
}

#[test]
fn doctor_flags_an_existing_path_token_client_config_offline() {
    let scratch = tempfile::tempdir().unwrap();
    let config = scratch.path().join("lific.toml");
    std::fs::write(&config, "[server]\nhost = '127.0.0.1'\nport = 0\n").unwrap();
    let client_config = scratch.path().join("opencode.json");
    let original = r#"{"mcp":{"lific":{"type":"remote","url":"https://example.test/mcp/DO_NOT_PRINT_THIS_SECRET"}}}"#;
    std::fs::write(&client_config, original).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_lific"))
        .current_dir(scratch.path())
        .env("LIFIC_CONNECT_HOME", scratch.path().join("home"))
        .env_remove("LIFIC_URL")
        .env_remove("LIFIC_TOKEN")
        .env_remove("LIFIC_API_KEY")
        .arg("--config")
        .arg(config)
        .args(["doctor", "--json"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let check = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["name"] == "client_config")
        .unwrap();
    assert_eq!(check["status"], "fail", "{check}");
    assert!(check["detail"].as_str().unwrap().contains("opencode.json"));
    assert!(check["detail"].as_str().unwrap().contains("Authorization"));
    for bytes in [&output.stdout, &output.stderr] {
        assert!(!String::from_utf8_lossy(bytes).contains("DO_NOT_PRINT_THIS_SECRET"));
    }
    assert_eq!(std::fs::read_to_string(client_config).unwrap(), original);
}

#[test]
fn invalid_config_repair_leaves_the_default_database_untouched() {
    for malformed in [false, true] {
        let scratch = tempfile::tempdir().unwrap();
        let database = scratch.path().join("lific.db");
        let config = scratch.path().join("selected.toml");
        if malformed {
            std::fs::write(&config, "{{{ not toml").unwrap();
        }
        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .execute_batch("CREATE TABLE marker (value TEXT); INSERT INTO marker VALUES ('keep');")
            .unwrap();
        drop(connection);
        let before = std::fs::read(&database).unwrap();

        let output = Command::new(env!("CARGO_BIN_EXE_lific"))
            .current_dir(scratch.path())
            .env_remove("LIFIC_URL")
            .env_remove("LIFIC_TOKEN")
            .env_remove("LIFIC_API_KEY")
            .arg("--config")
            .arg(config)
            .args(["doctor", "--repair", "--json"])
            .output()
            .unwrap();

        assert!(!output.status.success());
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["ok"], false);
        for check in report["checks"].as_array().unwrap() {
            // The service check reads no configured value — it inspects the
            // machine's installed service — so it runs even though
            // configuration selection failed, and its status is whatever
            // this machine's service state is (not assertable here).
            if check["name"] == "service" {
                continue;
            }
            let expected = if check["name"] == "config" {
                "fail"
            } else {
                "skipped"
            };
            assert_eq!(check["status"], expected, "{check}");
        }
        assert_eq!(std::fs::read(&database).unwrap(), before);
        assert!(!scratch.path().join("lific.db-wal").exists());
        assert!(!scratch.path().join("lific.db-shm").exists());
        assert!(!scratch.path().join("backups").exists());
    }
}
