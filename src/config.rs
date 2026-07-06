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
    /// Require a bearer credential on every REST/MCP request (default: true).
    ///
    /// LIF-294: setting this to `false` makes auth optional for local
    /// single-user use: a request presenting NO credential at all is treated
    /// as operator-equivalent (the same trust rail as unbound API keys,
    /// LIF-261). A presented-but-invalid token still 401s — bad credentials
    /// are never masked as anonymous. Deliberately a config-file key rather
    /// than a runtime instance setting: turning auth off requires shell
    /// access to the server, same as minting an operator key.
    ///
    /// Guard rails in `lific start`: refuses to boot when this is false and
    /// `server.public_url` points anywhere but localhost, and logs a loud
    /// warning otherwise (the default bind is 0.0.0.0 — LAN-reachable).
    pub required: bool,
    /// Emit the session cookie with the `Secure` attribute (HTTPS-only).
    ///
    /// LIF-207: `Secure` is correct in production (Tailscale Funnel = HTTPS),
    /// but browsers silently DROP a `Secure` cookie sent over plain `http://`
    /// (except `http://localhost` in some browsers). A local-first deployment
    /// reached over plain HTTP would lose the cookie — which breaks the OAuth
    /// approve flow (the one place the cookie is read). This is derived at
    /// startup from whether `server.public_url` is `https://` (see
    /// `AuthConfig::from_server`), defaulting to `true` (secure-by-default) so
    /// nothing is weakened unless an HTTP deployment is explicitly configured.
    #[serde(skip)]
    pub secure_cookies: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            allow_signup: true,
            required: true,
            secure_cookies: true,
        }
    }
}

impl AuthConfig {
    /// Build the runtime auth config, deriving `secure_cookies` from the
    /// server's public URL scheme. Only an explicit `http://` public_url turns
    /// `Secure` off; everything else (https, or unset) stays secure-by-default.
    pub fn from_server(file: &AuthConfig, public_url: Option<&str>) -> Self {
        let secure_cookies = match public_url {
            Some(url) => !url.trim().to_ascii_lowercase().starts_with("http://"),
            None => true,
        };
        Self {
            allow_signup: file.allow_signup,
            required: file.required,
            secure_cookies,
        }
    }
}

/// Does this URL point at the local machine? Backs the LIF-294 startup guard:
/// an auth-optional instance must never have a non-localhost `public_url`.
/// Conservative — anything unparseable counts as NOT localhost.
pub fn is_localhost_url(url: &str) -> bool {
    let rest = url.trim();
    let rest = rest.split("://").nth(1).unwrap_or(rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    let host = host.to_ascii_lowercase();
    // A literal IP must actually be loopback ("127.evil.com" is a valid DNS
    // name pointing anywhere, so prefix matching would be a hole).
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return ip.is_loopback();
    }
    host == "localhost"
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
    /// If set, exposes an authless MCP endpoint at `/mcp/<token>` that skips the
    /// OAuth flow entirely — the path secret itself is the credential. This is an
    /// escape hatch for clients whose OAuth connector flow is broken (notably
    /// claude.ai web, which completes OAuth, obtains a token, then never sends the
    /// authenticated MCP request). Treat the token like a bearer secret: anyone
    /// with the URL gets full MCP access, so use a long random value and only
    /// expose it over HTTPS.
    pub mcp_path_token: Option<String>,
    /// Username the authless `/mcp/<token>` endpoint acts as, for attribution.
    /// Defaults to the first admin user when unset.
    pub mcp_path_user: Option<String>,
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
            mcp_path_token: None,
            mcp_path_user: None,
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
    /// 1. Explicit path (if provided — used alone, no fallback)
    /// 2. ./lific.toml (working directory)
    /// 3. User config dir: ~/.config/lific/lific.toml on Linux
    ///    (`$XDG_CONFIG_HOME` respected), ~/Library/Application Support/lific/
    ///    on macOS, %APPDATA%\lific\ on Windows
    /// 4. System config dir (LIF-293): /etc/lific/ on Linux/BSD,
    ///    /Library/Application Support/Lific/ on macOS,
    ///    %ProgramData%\lific\ on Windows
    pub fn load(explicit_path: Option<&Path>) -> Self {
        let candidates = Self::candidate_paths(explicit_path);

        for path in &candidates {
            if path.exists() {
                match std::fs::read_to_string(path) {
                    Ok(contents) => match toml::from_str::<Config>(&contents) {
                        Ok(mut config) => {
                            info!(path = %path.display(), "loaded config");
                            // Anchor a relative database path to the config
                            // file's own directory, not the process cwd —
                            // `lific --config /srv/lific/lific.toml <cmd>` must
                            // find /srv/lific/lific.db no matter where it runs
                            // from. (backup_dir derives from database.path, so
                            // backups inherit the same anchoring.)
                            if config.database.path.is_relative()
                                && let Some(parent) = path.parent()
                                && !parent.as_os_str().is_empty()
                            {
                                config.database.path = parent.join(&config.database.path);
                            }
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

    /// The ordered list of paths [`Config::load`] probes. Split out so the
    /// search order is testable without creating files in /etc.
    fn candidate_paths(explicit_path: Option<&Path>) -> Vec<PathBuf> {
        if let Some(p) = explicit_path {
            return vec![p.to_path_buf()];
        }
        let mut c = vec![PathBuf::from(CONFIG_FILENAME)];
        if let Some(config_dir) = dirs::config_dir() {
            c.push(config_dir.join("lific").join(CONFIG_FILENAME));
        }
        if let Some(system_dir) = Self::system_config_dir() {
            c.push(system_dir.join(CONFIG_FILENAME));
        }
        c
    }

    /// The platform's system-wide config directory for Lific (LIF-293): the
    /// last-resort fallback, matching where other applications keep
    /// machine-level config.
    fn system_config_dir() -> Option<PathBuf> {
        if cfg!(target_os = "macos") {
            Some(PathBuf::from("/Library/Application Support/Lific"))
        } else if cfg!(windows) {
            std::env::var_os("ProgramData").map(|d| PathBuf::from(d).join("lific"))
        } else {
            Some(PathBuf::from("/etc/lific"))
        }
    }

    /// First existing config file among the standard search locations, if
    /// any. Used by commands that operate on "the instance" without an
    /// explicit `--config` (e.g. `lific service install`) so they agree with
    /// what `Config::load` would pick.
    pub fn discover_path() -> Option<PathBuf> {
        Self::candidate_paths(None).into_iter().find(|p| p.exists())
    }

    /// LIF-295: the OS-standard home for a `lific init` instance — config
    /// file in the user config dir, database in the user data dir (Linux:
    /// `~/.config/lific/lific.toml` + `~/.local/share/lific/lific.db`;
    /// macOS/Windows equivalents via `dirs`). `None` when the platform dirs
    /// can't be resolved (no HOME) — callers fall back to the cwd.
    pub fn os_default_instance() -> Option<(PathBuf, PathBuf)> {
        let config = dirs::config_dir()?.join("lific").join(CONFIG_FILENAME);
        let db = dirs::data_dir()?.join("lific").join("lific.db");
        Some((config, db))
    }

    /// Generate a default config file as a TOML string.
    pub fn default_toml() -> String {
        toml::to_string_pretty(&Config::default()).unwrap_or_default()
    }

    /// Like [`Config::default_toml`] but with an explicit database path
    /// (LIF-295: the XDG-split init writes an absolute data-dir path so the
    /// config and data can live in different standard directories).
    pub fn default_toml_with_db(db_path: &Path) -> String {
        let mut cfg = Config::default();
        cfg.database.path = db_path.to_path_buf();
        toml::to_string_pretty(&cfg).unwrap_or_default()
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
    fn relative_db_path_anchors_to_config_dir_not_cwd() {
        let dir = std::env::temp_dir().join(format!("lific_cfg_anchor_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lific.toml");
        std::fs::write(&path, "[database]\npath = \"lific.db\"\n").unwrap();

        // Loaded from an explicit path in another directory: the relative db
        // path must resolve beside the config file, not in the process cwd.
        let config = Config::load(Some(&path));
        assert_eq!(config.database.path, dir.join("lific.db"));
        // backup_dir derives from database.path, so it anchors too.
        assert_eq!(config.backup_dir(), dir.join("backups"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn absolute_db_path_is_untouched_by_anchoring() {
        let dir = std::env::temp_dir().join(format!("lific_cfg_abs_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lific.toml");
        std::fs::write(&path, "[database]\npath = \"/srv/lific/lific.db\"\n").unwrap();

        let config = Config::load(Some(&path));
        assert_eq!(config.database.path, PathBuf::from("/srv/lific/lific.db"));

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
        // Default db filename, anchored beside the config file it came from.
        assert_eq!(config.database.path, dir.join("lific.db"));

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

    // LIF-293: standard config locations — cwd, then user config dir, then
    // the system config dir; an explicit --config path short-circuits all.
    #[test]
    fn candidate_paths_search_cwd_then_user_then_system() {
        let paths = Config::candidate_paths(None);
        assert_eq!(paths[0], PathBuf::from("lific.toml"), "cwd first");
        assert!(
            paths
                .iter()
                .any(|p| p.ends_with(Path::new("lific").join("lific.toml"))),
            "user config dir must be probed: {paths:?}"
        );
        #[cfg(all(unix, not(target_os = "macos")))]
        assert_eq!(
            paths.last().unwrap(),
            &PathBuf::from("/etc/lific/lific.toml"),
            "system config dir is the last-resort fallback"
        );
        #[cfg(target_os = "macos")]
        assert_eq!(
            paths.last().unwrap(),
            &PathBuf::from("/Library/Application Support/Lific/lific.toml")
        );
    }

    #[test]
    fn candidate_paths_explicit_path_short_circuits() {
        let paths = Config::candidate_paths(Some(Path::new("/srv/lific/custom.toml")));
        assert_eq!(paths, vec![PathBuf::from("/srv/lific/custom.toml")]);
    }

    #[test]
    fn default_toml_roundtrips() {
        let toml_str = Config::default_toml();
        let parsed: Config = toml::from_str(&toml_str).expect("default toml should parse");
        assert_eq!(parsed.server.port, 3456);
    }

    // LIF-207: Secure cookie flag is derived from the public_url scheme.
    #[test]
    fn auth_config_secure_cookies_from_scheme() {
        let auth = AuthConfig::default();
        // HTTPS public URL → Secure on.
        assert!(AuthConfig::from_server(&auth, Some("https://lific.example")).secure_cookies);
        // Explicit HTTP → Secure off (otherwise the browser drops the cookie).
        assert!(!AuthConfig::from_server(&auth, Some("http://localhost:3456")).secure_cookies);
        assert!(!AuthConfig::from_server(&auth, Some("HTTP://Localhost")).secure_cookies);
        // Unset → secure-by-default (don't weaken when we don't know).
        assert!(AuthConfig::from_server(&auth, None).secure_cookies);
        // allow_signup / required are passed through untouched.
        let closed = AuthConfig {
            allow_signup: false,
            required: false,
            ..AuthConfig::default()
        };
        let runtime = AuthConfig::from_server(&closed, None);
        assert!(!runtime.allow_signup);
        assert!(!runtime.required);
    }

    // LIF-294: auth is required unless the config explicitly opts out.
    #[test]
    fn auth_required_defaults_to_true_and_parses_from_toml() {
        assert!(AuthConfig::default().required);
        let cfg: Config = toml::from_str("[auth]\nrequired = false\n").unwrap();
        assert!(!cfg.auth.required);
        // Omitting the key keeps the secure default even when [auth] is present.
        let cfg: Config = toml::from_str("[auth]\nallow_signup = false\n").unwrap();
        assert!(cfg.auth.required);
    }

    // LIF-294: the startup guard's localhost check.
    #[test]
    fn is_localhost_url_accepts_only_loopback() {
        for url in [
            "http://localhost:3456",
            "http://localhost",
            "https://LOCALHOST/lific",
            "http://127.0.0.1:3456",
            "http://127.5.5.5",
            "http://[::1]:3456",
            "http://user@localhost:3456/path",
        ] {
            assert!(is_localhost_url(url), "{url} should count as localhost");
        }
        for url in [
            "https://magi.tailb93ac8.ts.net",
            "http://192.168.1.10:3456",
            "http://127.evil.com",       // DNS name, not a loopback IP
            "https://localhost.example", // ditto
            "http://[::2]",
            "",
        ] {
            assert!(!is_localhost_url(url), "{url} must NOT count as localhost");
        }
    }
}
