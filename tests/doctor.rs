use std::process::Command;

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
