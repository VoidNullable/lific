use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::info;

const CONFIG_FILENAME: &str = "lific.toml";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub backup: BackupConfig,
    pub log: LogConfig,
    pub auth: AuthConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// Allow self-service signup via the API. If false, only admins can create users via CLI.
    pub allow_signup: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self { allow_signup: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Host to bind to
    pub host: String,
    /// Port to listen on
    pub port: u16,
    /// Public URL for OAuth discovery (e.g. https://your-server.example.com/lific)
    pub public_url: Option<String>,
    /// Allowed CORS origins. If empty, allows all origins (not recommended for production).
    /// Example: ["https://your-app.example.com"]
    pub cors_origins: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    /// Path to the SQLite database file
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BackupConfig {
    /// Enable automatic backups
    pub enabled: bool,
    /// Directory to store backups (relative to DB or absolute)
    pub dir: PathBuf,
    /// Backup interval in minutes
    pub interval_minutes: u64,
    /// Maximum number of backups to retain
    pub retain: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LogConfig {
    /// Log level: trace, debug, info, warn, error
    pub level: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 3456,
            public_url: None,
            cors_origins: Vec::new(),
        }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("lific.db"),
        }
    }
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            dir: PathBuf::from("backups"),
            interval_minutes: 60,
            retain: 24, // keep 24 hourly backups = 1 day of history
        }
    }
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
        }
    }
}

impl Config {
    /// Load config from the first file found, or return defaults.
    /// Search order:
    /// 1. Explicit path (if provided)
    /// 2. ./lific.toml (working directory)
    /// 3. ~/.config/lific/lific.toml
    pub fn load(explicit_path: Option<&Path>) -> Self {
        let candidates: Vec<PathBuf> = if let Some(p) = explicit_path {
            vec![p.to_path_buf()]
        } else {
            let mut c = vec![PathBuf::from(CONFIG_FILENAME)];
            if let Some(config_dir) = dirs::config_dir() {
                c.push(config_dir.join("lific").join(CONFIG_FILENAME));
            }
            c
        };

        for path in &candidates {
            if path.exists() {
                match std::fs::read_to_string(path) {
                    Ok(contents) => match toml::from_str::<Config>(&contents) {
                        Ok(config) => {
                            info!(path = %path.display(), "loaded config");
                            return config;
                        }
                        Err(e) => {
                            eprintln!("Warning: failed to parse {}: {e}", path.display());
                        }
                    },
                    Err(e) => {
                        eprintln!("Warning: failed to read {}: {e}", path.display());
                    }
                }
            }
        }

        Config::default()
    }

    /// Generate a default config file as a TOML string.
    pub fn default_toml() -> String {
        toml::to_string_pretty(&Config::default()).unwrap_or_default()
    }

    /// Resolve the backup directory relative to the database path if not absolute.
    pub fn backup_dir(&self) -> PathBuf {
        if self.backup.dir.is_absolute() {
            self.backup.dir.clone()
        } else if let Some(parent) = self.database.path.parent() {
            parent.join(&self.backup.dir)
        } else {
            self.backup.dir.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn defaults_are_sensible() {
        let config = Config::default();
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 3456);
        assert_eq!(config.database.path, PathBuf::from("lific.db"));
        assert!(config.backup.enabled);
        assert_eq!(config.backup.retain, 24);
        assert_eq!(config.log.level, "info");
    }

    #[test]
    fn load_from_explicit_path() {
        let dir = std::env::temp_dir().join(format!("lific_cfg_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.toml");

        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"
[server]
port = 9999
host = "127.0.0.1"

[database]
path = "/tmp/custom.db"

[backup]
enabled = false
"#
        )
        .unwrap();

        let config = Config::load(Some(&path));
        assert_eq!(config.server.port, 9999);
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.database.path, PathBuf::from("/tmp/custom.db"));
        assert!(!config.backup.enabled);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_file_returns_defaults() {
        let config = Config::load(Some(Path::new("/tmp/nonexistent_lific_cfg_12345.toml")));
        assert_eq!(config.server.port, 3456);
    }

    #[test]
    fn invalid_toml_returns_defaults() {
        let dir = std::env::temp_dir().join(format!("lific_bad_cfg_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.toml");
        std::fs::write(&path, "{{{{not valid toml!!!!").unwrap();

        let config = Config::load(Some(&path));
        assert_eq!(config.server.port, 3456); // fell back to defaults

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn partial_config_fills_defaults() {
        let dir = std::env::temp_dir().join(format!("lific_partial_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("partial.toml");
        std::fs::write(&path, "[server]\nport = 7777\n").unwrap();

        let config = Config::load(Some(&path));
        assert_eq!(config.server.port, 7777);
        assert_eq!(config.server.host, "0.0.0.0"); // default
        assert_eq!(config.database.path, PathBuf::from("lific.db")); // default

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn backup_dir_resolves_relative_to_db() {
        let mut config = Config::default();
        config.database.path = PathBuf::from("/data/lific/main.db");
        config.backup.dir = PathBuf::from("backups");

        assert_eq!(config.backup_dir(), PathBuf::from("/data/lific/backups"));
    }

    #[test]
    fn backup_dir_absolute_stays_absolute() {
        let mut config = Config::default();
        config.backup.dir = PathBuf::from("/mnt/backups");

        assert_eq!(config.backup_dir(), PathBuf::from("/mnt/backups"));
    }

    #[test]
    fn default_toml_roundtrips() {
        let toml_str = Config::default_toml();
        let parsed: Config = toml::from_str(&toml_str).expect("default toml should parse");
        assert_eq!(parsed.server.port, 3456);
    }
}
