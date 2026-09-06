use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::info;

const CONFIG_FILENAME: &str = "lific.toml";

/// A config file that has been opened and read.
///
/// The descriptor is kept alongside the contents so the permission tightening
/// below acts on *the file we read*, not on whatever the pathname resolves to
/// a moment later. Checking a path and then chmod-ing that same path is two
/// lookups: between them the file can be replaced with a symlink to something
/// else, and the chmod lands on the attacker's choice of target.
struct ConfigFile {
    contents: String,
    /// Held only for `fchmod` on unix; nothing else reads it.
    #[cfg(unix)]
    file: std::fs::File,
}

/// Make a group/other-readable config owner-only, through the descriptor it
/// was read from. Fails closed on symlinks: [`read_config_file`] opens with
/// `O_NOFOLLOW`, so a symlinked config never produces a [`ConfigFile`] to
/// tighten in the first place.
#[cfg_attr(
    not(unix),
    expect(clippy::unnecessary_wraps, reason = "fallible on Unix")
)]
fn tighten_config_permissions(_config: &ConfigFile) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = _config.file.metadata()?;
        if metadata.mode() & 0o077 != 0 {
            _config
                .file
                .set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn read_config_file(path: &Path) -> std::io::Result<ConfigFile> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = std::fs::OpenOptions::new();
    options.read(true).custom_flags(libc::O_NOFOLLOW);
    let mut file = options.open(path)?;
    // A directory opens fine on Linux and only fails at read time; a device or
    // fifo is never a config either. Reject anything that is not a plain file
    // before its mode is touched.
    if !file.metadata()?.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "configuration path must be a regular file",
        ));
    }
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    Ok(ConfigFile { contents, file })
}

#[cfg(not(unix))]
fn read_config_file(path: &Path) -> std::io::Result<ConfigFile> {
    Ok(ConfigFile {
        contents: std::fs::read_to_string(path)?,
    })
}

/// A config file was found but could not be honored.
///
/// This is always fatal. Booting on defaults because a file failed to parse
/// silently discards operator intent: `allow_signup = false`, a loopback
/// `host`, and a `cors_origins` allowlist all revert to their permissive
/// defaults, and the operator's only clue is a warning on stderr. Refusing to
/// start is the safe failure mode.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("--config path '{}' does not exist", path.display())]
    MissingExplicit { path: PathBuf },

    #[error("failed to read config {}: {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse config {}: {source}", path.display())]
    Parse {
        path: PathBuf,
        /// Boxed to keep `ConfigError` small. `toml::de::Error` is over 100
        /// bytes on its own, which makes every `Result<Config, _>` in the
        /// program pay for the failure path (clippy::result_large_err).
        #[source]
        source: Box<toml::de::Error>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[derive(Default)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub backup: BackupConfig,
    pub log: LogConfig,
    pub auth: AuthConfig,
    pub retention: RetentionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
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

/// Where the selected configuration came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSource {
    Explicit,
    ProjectLocal,
    User,
    System,
    BuiltInDefault,
}

impl ConfigSource {
    /// Stable human-readable provenance for diagnostics and service planning.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::ProjectLocal => "project-local",
            Self::User => "user",
            Self::System => "system",
            Self::BuiltInDefault => "built-in default",
        }
    }
}

/// The one configuration-selection result shared by startup and diagnostics.
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub config: Config,
    pub path: Option<PathBuf>,
    pub source: ConfigSource,
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

/// The two auth modes an operator can choose at `lific init` (LIFIC-25).
///
/// A single conceptual thing — "the auth mode" — bundles every consequence of
/// the choice into one type, so the menu and `cmd_init` never drift on how the
/// mode maps to config (`[auth] required`, `[server] host`), the database
/// (`web_auto_login`), and admin creation (passwordless vs passworded).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    /// No password; browser auto-login as the operator; binds loopback.
    LoginFree,
    /// Password sign-in on the web; leaves the bind host unchanged.
    Passwords,
}

impl AuthMode {
    /// `[auth] required`: login-free turns auth off; passwords keeps it on.
    pub fn required(self) -> bool {
        matches!(self, AuthMode::Passwords)
    }

    /// The `[server] host` to write, or `None` to leave it unchanged.
    /// Login-free must bind loopback so the startup guard (LIFIC-24) and the
    /// actual listening socket agree; password mode never touches it.
    pub fn host(self) -> Option<&'static str> {
        match self {
            AuthMode::LoginFree => Some("127.0.0.1"),
            AuthMode::Passwords => None,
        }
    }

    /// The `instance_settings.web_auto_login` flag: on for login-free so the
    /// browser signs the operator in without a password.
    pub fn web_auto_login(self) -> bool {
        matches!(self, AuthMode::LoginFree)
    }

    /// Whether the first admin is created passwordless (login-free) or with a
    /// real password (passwords).
    pub fn passwordless(self) -> bool {
        matches!(self, AuthMode::LoginFree)
    }

    /// The stable string used for the `--auth-mode` CLI flag and menu labels.
    pub fn as_str(self) -> &'static str {
        match self {
            AuthMode::LoginFree => "login-free",
            AuthMode::Passwords => "passwords",
        }
    }

    /// Parse a `--auth-mode` flag value, case-insensitive.
    pub fn parse(s: &str) -> Option<AuthMode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "login-free" => Some(AuthMode::LoginFree),
            "passwords" => Some(AuthMode::Passwords),
            _ => None,
        }
    }
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
#[serde(default, deny_unknown_fields)]
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
    /// Plain IPs are allowed; defaults to no trusted peers. Configure only
    /// isolated reverse-proxy addresses that cannot be reached directly.
    /// Example: ["10.0.0.0/8"]
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
#[serde(default, deny_unknown_fields)]
pub struct DatabaseConfig {
    /// Path to the SQLite database file
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BackupConfig {
    /// Enable automatic backups
    pub enabled: bool,
    /// Directory to store backups (relative to DB or absolute)
    pub dir: PathBuf,
    /// Backup interval in minutes
    pub interval_minutes: u64,
    /// Maximum number of backups to retain
    pub retain: usize,
    /// How many days of `audit_log` history to keep. Pruning runs at the end
    /// of each backup cycle, so retention rides the same schedule as backup
    /// rotation and never deletes history the current archive doesn't hold.
    ///
    /// LIF-158: unset (or `0`) means keep forever, which is the pre-existing
    /// behavior and stays the default. Nothing silently starts discarding an
    /// operator's audit trail on upgrade; they have to ask for it.
    pub audit_retention_days: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LogConfig {
    /// Log level: trace, debug, info, warn, error
    pub level: String,
}

/// How long tombstoned issues, pages and comments stay recoverable (LIF-438).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RetentionConfig {
    /// Days a soft-deleted row is kept before it is physically removed.
    ///
    /// A delete is a tombstone now: the row survives with `deleted_at` set so
    /// a replica syncing later can learn the deletion happened, and so a
    /// mistaken delete can be undone. Neither reason lasts forever, and a
    /// database that only ever grows is its own problem, so a background
    /// sweep collects tombstones past this window and runs the real DELETE
    /// (which is what finally fires the foreign-key cascades).
    ///
    /// `0` disables the sweep entirely: tombstones accumulate until an
    /// operator removes them by hand. That is a deliberate opt-out for anyone
    /// who wants deletion to be reversible indefinitely, not the default,
    /// because most instances want their trash emptied.
    pub trash_days: u32,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 3456,
            public_url: None,
            cors_origins: Vec::new(),
            trusted_proxies: Vec::new(),
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
            retain: 24,                 // keep 24 hourly backups = 1 day of history
            audit_retention_days: None, // keep audit history forever
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

impl Default for RetentionConfig {
    fn default() -> Self {
        Self { trash_days: 30 }
    }
}

/// Resolve `path` against the process cwd when it is relative.
///
/// Unlike [`std::fs::canonicalize`] this works for a file that does not exist
/// yet, which is exactly the case that matters when `init` is about to write
/// the path into a config file for later commands to read back.
pub fn absolutize(path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(path),
            Err(_) => path.to_path_buf(),
        }
    };
    // Drop `.` components so `lific.db` and `./lific.db` resolve to the same
    // path and callers comparing two of these do not report a spurious
    // disagreement. `..` is deliberately left alone: resolving it lexically is
    // wrong when a symlink is involved, and these paths are compared and
    // displayed rather than walked.
    joined
        .components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect()
}

impl Config {
    /// Load config from the first file found, or return defaults when no file
    /// exists anywhere in the search path.
    ///
    /// A file that exists but cannot be read or parsed is a hard error, not a
    /// fallback. Probing past a broken file would be just as surprising: if
    /// `./lific.toml` is malformed, quietly using `/etc/lific/lific.toml`
    /// instead is not what the operator asked for either.
    ///
    /// Search order:
    /// 1. Explicit path (if provided — used alone, no fallback)
    /// 2. ./lific.toml (working directory)
    /// 3. User config dir: ~/.config/lific/lific.toml on Linux
    ///    (`$XDG_CONFIG_HOME` respected), ~/Library/Application Support/lific/
    ///    on macOS, %APPDATA%\lific\ on Windows
    /// 4. System config dir (LIF-293): /etc/lific/ on Linux/BSD,
    ///    /Library/Application Support/Lific/ on macOS,
    ///    %ProgramData%\lific\ on Windows
    #[cfg(test)]
    pub fn load(explicit_path: Option<&Path>) -> Result<Self, ConfigError> {
        Self::resolve(explicit_path).map(|resolved| resolved.config)
    }

    /// Resolve one configuration file and retain both its path and provenance.
    /// Resolution never creates files. An explicit path is the sole candidate:
    /// missing, unreadable, or malformed input is never replaced with defaults.
    /// With no explicit path, the first existing candidate wins and only
    /// complete absence returns in-memory defaults; `lific init` creates the
    /// selected file.
    pub fn resolve(explicit_path: Option<&Path>) -> Result<ResolvedConfig, ConfigError> {
        for (source, path) in Self::candidate_sources(explicit_path) {
            match std::fs::symlink_metadata(&path) {
                Ok(_) => {
                    let file = read_config_file(&path).map_err(|source| ConfigError::Read {
                        path: path.clone(),
                        source,
                    })?;
                    let mut config = match toml::from_str::<Config>(&file.contents) {
                        Ok(config) => config,
                        Err(source) => {
                            return Err(ConfigError::Parse {
                                path,
                                source: Box::new(source),
                            });
                        }
                    };
                    match tighten_config_permissions(&file) {
                        Err(source) if source.kind() != std::io::ErrorKind::PermissionDenied => {
                            return Err(ConfigError::Read { path, source });
                        }
                        Ok(()) | Err(_) => {}
                    }
                    info!(path = %path.display(), source = source.label(), "loaded config");
                    if config.database.path.is_relative()
                        && let Some(parent) = path.parent()
                        && !parent.as_os_str().is_empty()
                    {
                        config.database.path = parent.join(&config.database.path);
                    }
                    return Ok(ResolvedConfig {
                        config,
                        path: Some(path),
                        source,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    if source == ConfigSource::Explicit {
                        return Err(ConfigError::MissingExplicit { path });
                    }
                }
                Err(source_error) => {
                    return Err(ConfigError::Read {
                        path,
                        source: source_error,
                    });
                }
            }
        }

        Ok(ResolvedConfig {
            config: Config::default(),
            path: None,
            source: ConfigSource::BuiltInDefault,
        })
    }

    /// The ordered list of paths [`Config::resolve`] probes. Split out so the
    /// search order is testable without creating files in /etc.
    #[cfg(test)]
    fn candidate_paths(explicit_path: Option<&Path>) -> Vec<PathBuf> {
        Self::candidate_sources(explicit_path)
            .into_iter()
            .map(|(_, path)| path)
            .collect()
    }

    fn candidate_sources(explicit_path: Option<&Path>) -> Vec<(ConfigSource, PathBuf)> {
        if let Some(path) = explicit_path {
            return vec![(ConfigSource::Explicit, path.to_path_buf())];
        }
        let mut candidates = vec![(ConfigSource::ProjectLocal, PathBuf::from(CONFIG_FILENAME))];
        if let Some(config_dir) = dirs::config_dir() {
            candidates.push((
                ConfigSource::User,
                config_dir.join("lific").join(CONFIG_FILENAME),
            ));
        }
        if let Some(system_dir) = Self::system_config_dir() {
            candidates.push((ConfigSource::System, system_dir.join(CONFIG_FILENAME)));
        }
        candidates
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

    /// Absolute, display-ready form of `database.path`, so an error can name
    /// the file the caller is actually about to get rather than a bare
    /// `lific.db` that says nothing about which directory it landed in.
    pub fn absolute_database_path(&self) -> String {
        if let Ok(canon) = std::fs::canonicalize(&self.database.path) {
            return canon.display().to_string();
        }
        absolutize(&self.database.path).display().to_string()
    }

    /// Refuse to operate on a database that does not exist yet.
    ///
    /// `lific init` is the only command that may create one. Everything else
    /// must resolve to a database that is already there, because the
    /// alternative is what used to happen: with no config file anywhere,
    /// `database.path` falls back to the bare relative default `lific.db`,
    /// which resolves against the process cwd. A config-less `lific mcp` —
    /// exactly how `server.json` tells MCP clients to launch us — would then
    /// create `lific.db`, `lific.db-wal` and `lific.db-shm` in whatever
    /// directory the agent happened to be sitting in, migrate them, and serve
    /// an empty tracker. The user gets no projects, three junk files in
    /// `git status`, and no explanation.
    ///
    /// `lific connect` has guarded this since it shipped; this is the same
    /// rule applied to every other door.
    pub fn require_existing_database(&self) -> Result<(), String> {
        if self.database.path.exists() {
            return Ok(());
        }
        // One line on purpose: `fn main() -> Result<_, Box<dyn Error>>` prints
        // errors with `{:?}`, so an embedded newline reaches the user as a
        // literal `\n`. Matches `connect::ensure_instance_exists`.
        Err(format!(
            "no Lific instance found: {} does not exist. Only `lific init` creates a database; \
             run it to set one up, or point at an existing instance with --config <file> or \
             --db <file>.",
            self.absolute_database_path()
        ))
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
    use proptest::prop_assert_eq;
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
        assert_eq!(config.server.trusted_proxies, Vec::<String>::new());
        assert_eq!(config.database.path, PathBuf::from("lific.db"));
        assert!(config.backup.enabled);
        assert_eq!(config.backup.retain, 24);
        assert_eq!(config.log.level, "info");
    }

    #[test]
    fn load_from_explicit_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
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

        let config = Config::load(Some(&path)).unwrap();
        assert_eq!(config.server.port, 9999);
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.database.path, PathBuf::from(ABSOLUTE_DB_PATH));
        assert!(!config.backup.enabled);
    }

    #[cfg(unix)]
    #[test]
    fn loading_config_tightens_existing_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("lific.toml");
        std::fs::write(&path, Config::default_toml()).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        Config::load(Some(&path)).unwrap();

        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn loading_config_rejects_symlink_without_chmodding_target() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target.toml");
        let link = tmp.path().join("lific.toml");
        std::fs::write(&target, Config::default_toml()).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        symlink(&target, &link).unwrap();

        assert!(Config::load(Some(&link)).is_err());
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    /// The chmod rides the descriptor the contents were read from, so it can
    /// only ever land on that inode. Swapping the *name* for a symlink to a
    /// sensitive file after the read cannot redirect the permission change,
    /// which the old check-then-chmod-by-path pair allowed.
    #[cfg(unix)]
    #[test]
    fn tightening_follows_the_opened_file_not_the_pathname() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("lific.toml");
        std::fs::write(&path, Config::default_toml()).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let inode = std::fs::metadata(&path).unwrap().ino();

        // A bystander an attacker would want the chmod to hit.
        let bystander = tmp.path().join("secrets");
        std::fs::write(&bystander, "secret").unwrap();
        std::fs::set_permissions(&bystander, std::fs::Permissions::from_mode(0o644)).unwrap();

        Config::load(Some(&path)).unwrap();

        let after = std::fs::metadata(&path).unwrap();
        assert_eq!(after.ino(), inode, "same file, start to finish");
        assert_eq!(after.permissions().mode() & 0o777, 0o600);
        assert_eq!(
            std::fs::metadata(&bystander).unwrap().permissions().mode() & 0o777,
            0o644,
            "nothing else may be chmodded"
        );

        // And once the name IS a symlink, loading fails closed rather than
        // tightening whatever it points at.
        let link = tmp.path().join("link.toml");
        symlink(&bystander, &link).unwrap();
        assert!(Config::load(Some(&link)).is_err());
        assert_eq!(
            std::fs::metadata(&bystander).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    /// Anything that is not a plain file is refused before its mode is
    /// touched: a directory opens fine on Linux, so the type check is what
    /// stops it.
    #[cfg(unix)]
    #[test]
    fn loading_config_rejects_a_non_regular_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("lific.toml");
        std::fs::create_dir(&dir).unwrap();

        assert!(matches!(
            Config::load(Some(&dir)),
            Err(ConfigError::Read { .. })
        ));
    }

    /// An already-tight config is left exactly as it is — no needless chmod.
    #[cfg(unix)]
    #[test]
    fn loading_config_leaves_owner_only_permissions_alone() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("lific.toml");
        std::fs::write(&path, Config::default_toml()).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();

        Config::load(Some(&path)).unwrap();

        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o400
        );
    }

    #[cfg(unix)]
    #[test]
    fn loading_config_rejects_dangling_symlink() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let link = tmp.path().join("lific.toml");
        symlink(tmp.path().join("missing.toml"), &link).unwrap();

        assert!(matches!(
            Config::load(Some(&link)),
            Err(ConfigError::Read { .. })
        ));
    }

    #[test]
    fn relative_db_path_anchors_to_config_dir_not_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let path = dir.join("lific.toml");
        std::fs::write(&path, "[database]\npath = \"lific.db\"\n").unwrap();

        // Loaded from an explicit path in another directory: the relative db
        // path must resolve beside the config file, not in the process cwd.
        let config = Config::load(Some(&path)).unwrap();
        assert_eq!(config.database.path, dir.join("lific.db"));
        // backup_dir derives from database.path, so it anchors too.
        assert_eq!(config.backup_dir(), dir.join("backups"));
    }

    #[test]
    fn absolute_db_path_is_untouched_by_anchoring() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("lific.toml");
        std::fs::write(
            &path,
            format!("[database]\npath = \"{ABSOLUTE_DB_PATH}\"\n"),
        )
        .unwrap();

        let config = Config::load(Some(&path)).unwrap();
        assert_eq!(config.database.path, PathBuf::from(ABSOLUTE_DB_PATH));
    }

    /// An explicit `--config` path names one file. A missing file is a hard
    /// error; only complete absence of every candidate may return defaults.
    #[test]
    fn missing_explicit_file_is_a_hard_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("missing.toml");
        let error = Config::load(Some(&path)).unwrap_err();
        assert!(matches!(error, ConfigError::MissingExplicit { .. }));
        assert_eq!(
            error.to_string(),
            format!("--config path '{}' does not exist", path.display())
        );
    }

    #[test]
    fn resolve_reports_explicit_path_and_provenance() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("lific.toml");
        std::fs::write(&path, "[server]\nport = 3998\n").unwrap();

        let resolved = Config::resolve(Some(&path)).unwrap();

        assert_eq!(resolved.path, Some(path));
        assert_eq!(resolved.source, ConfigSource::Explicit);
        assert_eq!(resolved.config.server.port, 3998);
    }

    proptest::proptest! {
        #[test]
        fn explicit_resolution_anchors_any_safe_relative_database_name(
            database_name in "[a-zA-Z0-9_-]{1,24}"
        ) {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("lific.toml");
            std::fs::write(
                &path,
                format!("[database]\npath = \"{database_name}.db\"\n"),
            ).unwrap();

            let resolved = Config::resolve(Some(&path)).unwrap();
            let expected_database = tmp.path().join(format!("{database_name}.db"));

            prop_assert_eq!(resolved.source, ConfigSource::Explicit);
            prop_assert_eq!(resolved.path, Some(path.clone()));
            prop_assert_eq!(
                resolved.config.database.path.as_path(),
                expected_database.as_path()
            );
            let loaded = Config::load(Some(&path)).unwrap();
            prop_assert_eq!(
                loaded.database.path.as_path(),
                resolved.config.database.path.as_path()
            );
            prop_assert_eq!(
                Config::load(Some(&path)).unwrap().server.port,
                resolved.config.server.port
            );
        }
    }

    /// A malformed config must refuse to load rather than silently reverting
    /// to defaults. The old behavior reverted `host` to `0.0.0.0`,
    /// `allow_signup` to `true` and `cors_origins` to allow-any, leaving only
    /// a stderr warning behind.
    #[test]
    fn invalid_toml_is_a_hard_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bad.toml");
        std::fs::write(&path, "{{{{not valid toml!!!!").unwrap();

        let err = Config::load(Some(&path)).unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }));
        // The message names the offending file so the operator can fix it.
        assert!(err.to_string().contains("bad.toml"));
    }

    /// A misspelled key is the exact scenario reported: the operator believes
    /// they closed signup, the key is ignored, and the instance comes up with
    /// `allow_signup = true`. It must fail loudly instead.
    #[test]
    fn unknown_key_is_rejected_rather_than_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("typo.toml");
        // `allow_signup` fat-fingered.
        std::fs::write(&path, "[auth]\nallow_signupp = false\n").unwrap();

        let err = Config::load(Some(&path)).unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }));
        assert!(err.to_string().contains("allow_signupp"));
    }

    /// The whole point of the change: the permissive defaults an operator
    /// would have silently inherited are genuinely permissive, so inheriting
    /// them by accident matters.
    #[test]
    fn defaults_are_permissive_enough_to_warrant_failing_closed() {
        let d = Config::default();
        assert_eq!(d.server.host, "0.0.0.0");
        assert!(d.auth.allow_signup);
        assert!(d.server.cors_origins.is_empty()); // empty == allow any origin
    }

    #[test]
    fn partial_config_fills_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let path = dir.join("partial.toml");
        std::fs::write(&path, "[server]\nport = 7777\n").unwrap();

        let config = Config::load(Some(&path)).unwrap();
        assert_eq!(config.server.port, 7777);
        assert_eq!(config.server.host, "0.0.0.0"); // default
        // Default db filename, anchored beside the config file it came from.
        assert_eq!(config.database.path, dir.join("lific.db"));
    }

    // LIF-158: audit log retention is opt-in. Unset and 0 both mean "keep
    // forever", so no upgrade quietly starts discarding history.
    #[test]
    fn audit_retention_days_defaults_to_off_and_parses_from_toml() {
        assert_eq!(BackupConfig::default().audit_retention_days, None);

        // Present.
        let cfg: Config = toml::from_str("[backup]\naudit_retention_days = 90\n").unwrap();
        assert_eq!(cfg.backup.audit_retention_days, Some(90));

        // Absent, with the section present: still off, and the neighboring
        // backup knobs keep their defaults.
        let cfg: Config = toml::from_str("[backup]\nretain = 12\n").unwrap();
        assert_eq!(cfg.backup.audit_retention_days, None);
        assert_eq!(cfg.backup.retain, 12);

        // Explicit 0: parses, and means keep forever rather than "prune all".
        let cfg: Config = toml::from_str("[backup]\naudit_retention_days = 0\n").unwrap();
        assert_eq!(cfg.backup.audit_retention_days, Some(0));
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
        assert!(parsed.server.trusted_proxies.is_empty());
    }

    // LIF-438: soft-delete retention. 30 days by default, 0 means never purge.
    #[test]
    fn retention_defaults_to_thirty_days_and_parses_from_toml() {
        assert_eq!(Config::default().retention.trash_days, 30);
        let cfg: Config = toml::from_str("[retention]\ntrash_days = 7\n").unwrap();
        assert_eq!(cfg.retention.trash_days, 7);
        let cfg: Config = toml::from_str("[retention]\ntrash_days = 0\n").unwrap();
        assert_eq!(cfg.retention.trash_days, 0);
        // Absent section keeps the default rather than disabling the sweep.
        let cfg: Config = toml::from_str("[server]\nport = 1234\n").unwrap();
        assert_eq!(cfg.retention.trash_days, 30);
        // And it survives the default-config round trip `lific init` writes.
        let written: Config = toml::from_str(&Config::default_toml()).unwrap();
        assert_eq!(written.retention.trash_days, 30);
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
        for host in ["127.0.0.1", "127.5.5.5", "::1", "localhost", "LOCALHOST"] {
            assert!(is_localhost_host(host), "{host} should count as loopback");
        }
        for host in ["0.0.0.0", "::", "[::]", "192.168.1.10", "lific.example", ""] {
            assert!(
                !is_localhost_host(host),
                "{host} must NOT count as loopback"
            );
        }
    }

    // LIFIC-25: the auth-mode menu's two choices bundle `(required, host,
    // web_auto_login, admin-passwordless)` into one concept.
    #[test]
    fn auth_mode_bundles_its_consequences() {
        let free = AuthMode::LoginFree;
        assert!(!free.required());
        assert_eq!(free.host(), Some("127.0.0.1"));
        assert!(free.web_auto_login());
        assert!(free.passwordless());

        let pw = AuthMode::Passwords;
        assert!(pw.required());
        assert_eq!(pw.host(), None, "password mode leaves host unchanged");
        assert!(!pw.web_auto_login());
        assert!(!pw.passwordless());
    }

    #[test]
    fn auth_mode_parses_and_names() {
        assert_eq!(AuthMode::parse("login-free"), Some(AuthMode::LoginFree));
        assert_eq!(AuthMode::parse("passwords"), Some(AuthMode::Passwords));
        assert_eq!(AuthMode::parse("LOGIN-FREE"), Some(AuthMode::LoginFree));
        assert_eq!(AuthMode::parse("bogus"), None);
        assert_eq!(AuthMode::LoginFree.as_str(), "login-free");
        assert_eq!(AuthMode::Passwords.as_str(), "passwords");
    }
}

#[cfg(test)]
mod instance_guard_tests {
    //! The "only `lific init` creates a database" rule.
    //!
    //! Regression cover for the first-run failure: `server.json` tells MCP
    //! clients to launch a bare `lific mcp`, which with no config file anywhere
    //! resolved `database.path` to the relative default `lific.db` against the
    //! agent's cwd and created a fresh empty instance inside the user's repo.

    use super::*;

    /// An absolute path literal the host actually agrees is absolute; see the
    /// note on the sibling module's copy for why Windows needs its own.
    #[cfg(unix)]
    const ABSOLUTE_DB_PATH: &str = "/srv/lific/lific.db";
    #[cfg(not(unix))]
    const ABSOLUTE_DB_PATH: &str = "C:/srv/lific/lific.db";

    #[test]
    fn missing_database_is_refused_and_the_error_names_the_absolute_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut cfg = Config::default();
        cfg.database.path = dir.path().join("nope.db");

        let err = cfg
            .require_existing_database()
            .expect_err("a database that does not exist must not be conjured");

        assert!(
            err.contains(&dir.path().join("nope.db").display().to_string()),
            "the error has to name the file the caller would otherwise have \
             silently created, got: {err}"
        );
        assert!(
            err.contains("lific init"),
            "the error must point at the one command that does create a database, got: {err}"
        );
    }

    #[test]
    fn existing_database_is_allowed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("lific.db");
        std::fs::write(&db, b"").expect("touch db");
        let mut cfg = Config::default();
        cfg.database.path = db;

        assert!(cfg.require_existing_database().is_ok());
    }

    #[test]
    fn the_bare_default_config_points_at_a_relative_path() {
        // This is the property that made the bug possible, pinned so nobody
        // "tidies" the default into something absolute and quietly removes the
        // reason the guard above exists.
        assert!(
            Config::default().database.path.is_relative(),
            "the no-config-found default is relative, which is exactly why it \
             must never be opened implicitly"
        );
    }

    #[test]
    fn absolutize_resolves_relative_paths_without_requiring_them_to_exist() {
        let resolved = absolutize(Path::new("does-not-exist.db"));
        assert!(
            resolved.is_absolute(),
            "a relative path must come back absolute even when no such file exists"
        );
        assert!(resolved.ends_with("does-not-exist.db"));
    }

    #[test]
    fn absolutize_leaves_absolute_paths_alone() {
        let already = absolutize(Path::new(ABSOLUTE_DB_PATH));
        assert_eq!(already, PathBuf::from(ABSOLUTE_DB_PATH));
    }

    #[test]
    fn absolutize_normalizes_dot_components() {
        // `init`'s --db-versus-config warning compares two absolutized paths,
        // so `lific.db` and `./lific.db` disagreeing would fire it spuriously.
        assert_eq!(
            absolutize(Path::new("lific.db")),
            absolutize(Path::new("./lific.db"))
        );
        assert_eq!(
            absolutize(Path::new("data/./lific.db")),
            absolutize(Path::new("data/lific.db"))
        );
    }
}
