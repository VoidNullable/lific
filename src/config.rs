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

/// Canonical plain-language caution for login-free mode (`[auth] required =
/// false`). LIFIC-22: this is the single source of truth that both the `lific
/// init` auth-mode menu and the `lific start` startup warning consume, so the
/// two surfaces can never diverge. It names the risk, states the safe
/// condition, and gives the recovery path — no shock all-caps language, no
/// internal jargon like "credential-less request".
pub fn login_free_caution() -> &'static str {
    "LIFIC is running in login-free mode: anyone who can reach it can administer \
     it. Keep it on a machine only you and trusted people can reach. To switch \
     to passwords, set [auth] required = true and run lific init again."
}

/// Does a `[server] host` bind value point at the local machine? Backs the
/// LIFIC-24 startup guard: login-free mode must refuse to bind anywhere but
/// loopback, so the safety check and the actual listening socket agree.
/// Conservative — anything unparseable counts as NOT loopback.
pub fn is_localhost_host(host: &str) -> bool {
    let host = host.trim();
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
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
    /// IP addresses or CIDR ranges allowed to supply client-IP proxy headers.
    /// Plain IPs are allowed; defaults to loopback so Tailscale Funnel keeps
    /// working while directly exposed listeners ignore spoofed X-Forwarded-For.
    /// Example: ["127.0.0.0/8", "::1/128", "10.0.0.0/8"]
    pub trusted_proxies: Vec<String>,
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
            trusted_proxies: vec!["127.0.0.0/8".into(), "::1/128".into()],
            mcp_path_token: None,
            mcp_path_user: None,
        }
    }
}

impl ServerConfig {
    /// Validate the configured proxy ranges once during startup before request
    /// handlers use them for rate-limit and audit client-IP keys.
    pub fn trusted_proxy_ranges(&self) -> Result<Vec<crate::ratelimit::IpNetwork>, String> {
        crate::ratelimit::parse_trusted_proxies(&self.trusted_proxies)
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

    /// Merge the auth-mode menu's choice into a config document, preserving
    /// every other section and setting.
    ///
    /// LIFIC-23: sets `[auth] required` and, when `host` is supplied,
    /// `[server] host`. When `existing` is empty/absent the function builds a
    /// fresh default config carrying the chosen values; otherwise it edits the
    /// existing TOML in place (formatting and comments survive via toml_edit).
    /// Pure — no filesystem side effects. This is what the `lific init`
    /// auth-mode menu (LIFIC-25) uses to persist the operator's choice.
    ///
    /// Returns `Err` (leaving the caller with the untouched source to fix by
    /// hand) when an existing document does not parse — never destroys a
    /// user's config, mirroring the connect writers.
    pub fn apply_auth_mode(
        existing: &str,
        required: bool,
        host: Option<&str>,
    ) -> Result<String, String> {
        let mut doc: toml_edit::DocumentMut = if existing.trim().is_empty() {
            Config::default_toml()
                .parse::<toml_edit::DocumentMut>()
                .expect("default config parses")
        } else {
            existing
                .parse()
                .map_err(|e| format!("existing config does not parse: {e}"))?
        };
        doc["auth"]["required"] = toml_edit::value(required);
        if let Some(host) = host {
            doc["server"]["host"] = toml_edit::value(host);
        }
        Ok(doc.to_string())
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

    /// An absolute `database.path` literal for the host platform.
    ///
    /// Anchoring only rewrites *relative* paths, so any test asserting that an
    /// absolute path survives untouched has to spell one the host actually
    /// considers absolute. `/srv/lific/lific.db` qualifies on unix but is
    /// merely drive-relative on Windows, where anchoring correctly resolves it
    /// against the config file's drive and the assertion would fail on a
    /// fixture detail rather than on behavior.
    #[cfg(unix)]
    const ABSOLUTE_DB_PATH: &str = "/srv/lific/lific.db";
    /// Forward slashes on purpose: Windows accepts them as separators, and a
    /// backslash would need escaping inside the TOML fixtures below.
    #[cfg(not(unix))]
    const ABSOLUTE_DB_PATH: &str = "C:/srv/lific/lific.db";

    #[test]
    fn defaults_are_sensible() {
        let config = Config::default();
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 3456);
        assert_eq!(
            config.server.trusted_proxies,
            vec!["127.0.0.0/8", "::1/128"]
        );
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
path = "{ABSOLUTE_DB_PATH}"

[backup]
enabled = false
"#
        )
        .unwrap();

        let config = Config::load(Some(&path));
        assert_eq!(config.server.port, 9999);
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.database.path, PathBuf::from(ABSOLUTE_DB_PATH));
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
        std::fs::write(&path, format!("[database]\npath = \"{ABSOLUTE_DB_PATH}\"\n")).unwrap();

        let config = Config::load(Some(&path));
        assert_eq!(config.database.path, PathBuf::from(ABSOLUTE_DB_PATH));

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
        assert_eq!(parsed.server.trusted_proxies, vec!["127.0.0.0/8", "::1/128"]);
    }

    #[test]
    fn invalid_trusted_proxy_config_is_rejected() {
        let config: Config = toml::from_str(
            r#"
[server]
trusted_proxies = ["not-a-cidr"]
"#,
        )
        .unwrap();
        let error = config.server.trusted_proxy_ranges().unwrap_err();
        assert!(error.contains("trusted_proxies[0]"));
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

    // LIFIC-22: the shared login-free caution is set once and plain-language.
    #[test]
    fn login_free_caution_is_plain_and_complete() {
        let text = login_free_caution();
        // Names the mode.
        assert!(text.contains("login-free mode"));
        // States the risk in plain words.
        assert!(text.contains("anyone who can reach it can administer it"));
        // States the safe condition.
        assert!(text.contains("Keep it on a machine only you and trusted people can reach"));
        // States the recovery path.
        assert!(text.contains("required = true"));
        // No shock all-caps language is allowed to leak back in from the old
        // "AUTH IS DISABLED" warning.
        assert!(!text.contains("AUTH IS DISABLED"));
        // No internal jargon either.
        assert!(!text.contains("credential-less"));
    }

    // LIFIC-23: applying the auth-mode choice edits in place and preserves
    // every other section, setting, and comment.
    #[test]
    fn apply_auth_mode_edits_required_and_host_and_preserves_sections() {
        let existing = r#"# my cruft
[server]
host = "0.0.0.0"
port = 3456

[auth]
required = true
allow_signup = false

[backup]
enabled = false
"#;
        let out = Config::apply_auth_mode(existing, false, Some("127.0.0.1")).unwrap();
        // Comment survives.
        assert!(out.contains("# my cruft"), "comment must survive");
        // Our two values are set.
        let doc: toml_edit::DocumentMut = out.parse().unwrap();
        assert_eq!(doc["auth"]["required"].as_bool(), Some(false));
        assert_eq!(doc["server"]["host"].as_str(), Some("127.0.0.1"));
        // Untouched siblings survive with their values.
        assert_eq!(doc["server"]["port"].as_integer(), Some(3456));
        assert_eq!(doc["auth"]["allow_signup"].as_bool(), Some(false));
        assert_eq!(doc["backup"]["enabled"].as_bool(), Some(false));
    }

    // LIFIC-23: password mode only touches required, leaving host untouched.
    #[test]
    fn apply_auth_mode_password_leaves_host_alone() {
        let existing = "[server]\nhost = \"0.0.0.0\"\nport = 9000\n\n[auth]\nrequired = false\n";
        let out = Config::apply_auth_mode(existing, true, None).unwrap();
        let doc: toml_edit::DocumentMut = out.parse().unwrap();
        assert_eq!(doc["auth"]["required"].as_bool(), Some(true));
        assert_eq!(doc["server"]["host"].as_str(), Some("0.0.0.0"));
        assert_eq!(doc["server"]["port"].as_integer(), Some(9000));
    }

    // LIFIC-23: an absent/empty document produces a fresh default config.
    #[test]
    fn apply_auth_mode_creates_fresh_when_absent() {
        let out = Config::apply_auth_mode("", false, Some("127.0.0.1")).unwrap();
        let doc: toml_edit::DocumentMut = out.parse().unwrap();
        assert_eq!(doc["auth"]["required"].as_bool(), Some(false));
        assert_eq!(doc["server"]["host"].as_str(), Some("127.0.0.1"));
        // Defaults still present.
        assert_eq!(doc["server"]["port"].as_integer(), Some(3456));
        assert_eq!(doc["auth"]["allow_signup"].as_bool(), Some(true));
    }

    // LIFIC-23: an unparseable existing document must not be destroyed — the
    // editor refuses and returns an error, mirroring the connect writers.
    #[test]
    fn apply_auth_mode_refuses_unparseable_and_returns_error() {
        let existing = "this is = = not valid toml [[[\n";
        let err = Config::apply_auth_mode(existing, false, Some("127.0.0.1")).unwrap_err();
        assert!(err.contains("does not parse"), "error must say why: {err}");
    }

    // LIFIC-24: the startup guard's bind-host check.
    #[test]
    fn is_localhost_host_accepts_only_loopback() {
        for host in [
            "127.0.0.1",
            "127.5.5.5",
            "::1",
            "localhost",
            "LOCALHOST",
        ] {
            assert!(is_localhost_host(host), "{host} should count as loopback");
        }
        for host in ["0.0.0.0", "::", "[::]", "192.168.1.10", "lific.example", ""] {
            assert!(!is_localhost_host(host), "{host} must NOT count as loopback");
        }
    }
}
