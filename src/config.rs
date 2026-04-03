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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Host to bind to
    pub host: String,
    /// Port to listen on
    pub port: u16,
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
