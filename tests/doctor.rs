use std::process::Command;

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
