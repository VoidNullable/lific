//! Credential storage for `lific login` (LIF-258).
//!
//! Stores the OAuth access token minted by the device flow so subsequent
//! commands (`lific doctor`, future authed CLI calls) can reuse it. Two
//! backends, tried in order:
//!
//! 1. **OS keyring** — Secret Service (Linux), Keychain (macOS), Credential
//!    Manager (Windows) via the `keyring` crate. This is the preferred, secure
//!    store.
//! 2. **Plaintext file fallback** — `~/.config/lific/credentials.json`, a map
//!    of `base_url → token`, written 0600 under a 0700 parent. Used when the
//!    keyring is unavailable (headless box with no Secret Service, CI, etc.).
//!    A loud one-line warning is printed to stderr whenever this path is taken,
//!    because the token lands on disk in the clear.
//!
//! Load precedence: `LIFIC_TOKEN` env var (when bound to the target origin) >
//! keyring > file. The env var lets an agent or CI inject a token without any
//! on-disk state. (The existing `LIFIC_API_KEY` is for API keys and is
//! deliberately left untouched.)
//!
//! ## Env tokens are bound to an origin (LIF-408)
//!
//! `LIFIC_TOKEN` used to be attached to whatever server URL won resolution: a
//! `--url` flag, or a `lific.toml` discovered in the cwd. Running the CLI in a
//! directory whose config points at a hostile server therefore shipped the
//! user's token to that server. The env token is now only used when
//! `LIFIC_URL` is also set and its origin (scheme + host + port, normalized)
//! equals the origin of the URL we are about to talk to. Anything else falls
//! back to the per-host stored credential and warns on stderr. See
//! [`env_token_for`] for the pure decision, [`origin_of`] for normalization.
//!
//! ## Testability
//!
//! The file backend is factored behind [`FileStore`] with an injectable path,
//! so the round-trip / permission / precedence tests never touch the real
//! keyring or the real `~/.config`. The keyring itself is only reachable
//! through [`store`]/[`load`]/[`delete`]; any test that would hit a live Secret
//! Service is gated `#[ignore]` (CI has none).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Environment variable carrying an OAuth token, used in place of stored
/// credentials when it is bound to the target origin (see [`env_token_for`]).
pub const TOKEN_ENV: &str = "LIFIC_TOKEN";

/// Environment variable that names the server [`TOKEN_ENV`] belongs to. Same
/// variable clap reads for the global `--url` flag.
pub const URL_ENV: &str = "LIFIC_URL";

/// Keyring service name (namespace) for all Lific credentials.
const KEYRING_SERVICE: &str = "lific";

/// Normalize a base URL into a stable credential key: trim, drop any trailing
/// slash, and lowercase the scheme+host. Two spellings of the same server
/// (`http://H:3998` vs `http://H:3998/`) must resolve to one entry.
pub fn normalize_base_url(base: &str) -> String {
    let trimmed = base.trim().trim_end_matches('/');
    let Ok(mut url) = reqwest::Url::parse(trimmed) else {
        return trimmed.to_owned();
    };
    let scheme = url.scheme().to_ascii_lowercase();
    let _ = url.set_scheme(&scheme);
    if let Some(host) = url.host_str().map(str::to_ascii_lowercase) {
        let _ = url.set_host(Some(&host));
    }
    url.to_string().trim_end_matches('/').to_owned()
}

// ── Origin binding for the env token (LIF-408) ───────────────────────────

/// The origin of `url` (scheme, host, port) in a form two spellings of the
/// same server share. Default ports are made explicit (`https://h` and
/// `https://h:443` agree), the host is lowercased, and path, query, fragment
/// and trailing slashes are dropped.
///
/// Returns `None` for anything that is not a parseable `http`/`https` URL with
/// a host. Callers treat `None` as "does not match", so an unparseable value
/// can never bind a token.
pub fn origin_of(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url.trim()).ok()?;
    let scheme = parsed.scheme().to_ascii_lowercase();
    let default_port = match scheme.as_str() {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let host = parsed.host_str()?.to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    let port = parsed.port().unwrap_or(default_port);
    Some(format!("{scheme}://{host}:{port}"))
}

/// What the environment says about the token for one target server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvToken {
    /// No usable `LIFIC_TOKEN` is set; the stored backends decide.
    Absent,
    /// `LIFIC_TOKEN` is bound to the target origin, so send it.
    Bound(String),
    /// `LIFIC_TOKEN` is set but not bound to the target origin. Ignore it,
    /// fall back to the stored credential for that origin, and warn.
    Unbound,
}

/// Decide whether an env token may be sent to `target_url`. Pure: every input
/// is a parameter, so tests never touch the process environment (LIF-401).
///
/// The token travels only when `LIFIC_URL` is set and names the same origin we
/// are about to talk to. A target that came from a `--url` flag or a cwd
/// `lific.toml` pointing somewhere else gets no env token.
#[must_use = "the caller decides what to do with the env token"]
pub fn env_token_for(env_token: Option<&str>, env_url: Option<&str>, target_url: &str) -> EnvToken {
    let Some(token) = env_token.map(str::trim).filter(|t| !t.is_empty()) else {
        return EnvToken::Absent;
    };
    let Some(env_url) = env_url.map(str::trim).filter(|u| !u.is_empty()) else {
        return EnvToken::Unbound;
    };
    match (origin_of(env_url), origin_of(target_url)) {
        (Some(bound), Some(target)) if bound == target => EnvToken::Bound(token.to_owned()),
        _ => EnvToken::Unbound,
    }
}

/// Warn once per process that an env token was dropped. Goes to stderr only,
/// so it never lands in the JSON a `--json` caller parses from stdout.
fn warn_env_token_unbound(target_url: &str) {
    static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    eprintln!(
        "warning: {TOKEN_ENV} is set but not bound to {target_url}; ignoring it and using the \
         stored credential for that server instead. Set {URL_ENV} to that server to send the \
         env token there."
    );
}

/// Where the plaintext fallback file lives: `~/.config/lific/credentials.json`.
fn default_file_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("lific").join("credentials.json"))
}

// ── File backend ─────────────────────────────────────────────────────────

/// The JSON-on-disk fallback store, parameterized on its path so tests can
/// point it at a tempdir.
pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn read_map(&self) -> BTreeMap<String, String> {
        match std::fs::read_to_string(&self.path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => BTreeMap::new(),
        }
    }

    fn write_map(&self, map: &BTreeMap<String, String>) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
            // Tighten the parent dir to 0700 (best-effort; only meaningful on unix).
            set_dir_private(parent);
        }
        let json = serde_json::to_string_pretty(map).map_err(std::io::Error::other)?;
        std::fs::write(&self.path, json)?;
        set_file_private(&self.path);
        Ok(())
    }

    /// Store `token` under `key`, creating the file if needed.
    pub fn store(&self, key: &str, token: &str) -> std::io::Result<()> {
        let mut map = self.read_map();
        map.insert(key.to_string(), token.to_string());
        self.write_map(&map)
    }

    /// Load the token for `key`, if present.
    pub fn load(&self, key: &str) -> Option<String> {
        self.read_map().get(key).cloned()
    }

    /// Remove `key`. Returns whether an entry was actually removed.
    pub fn delete(&self, key: &str) -> std::io::Result<bool> {
        let mut map = self.read_map();
        let removed = map.remove(key).is_some();
        if removed {
            self.write_map(&map)?;
        }
        Ok(removed)
    }
}

/// Best-effort chmod 0600 on the credentials file (unix only).
fn set_file_private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Best-effort chmod 0700 on the parent dir (unix only).
fn set_dir_private(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
}

// ── Public API (keyring + file, with env override on load) ───────────────

/// Store a token for `base_url`. Tries the keyring first; on any keyring error
/// falls back to the plaintext file and prints a loud warning to stderr.
pub fn store(base_url: &str, token: &str) -> Result<(), String> {
    let key = normalize_base_url(base_url);
    match keyring_store(&key, token) {
        Ok(()) => Ok(()),
        Err(e) => {
            let store = FileStore::new(
                default_file_path().ok_or_else(|| "cannot resolve config dir".to_string())?,
            );
            eprintln!(
                "warning: OS keyring unavailable ({e}); storing token in PLAINTEXT at {} (0600). \
                 Set up a Secret Service/Keychain to secure it, or use {TOKEN_ENV} to avoid on-disk storage.",
                store.path.display()
            );
            store
                .store(&key, token)
                .map_err(|e| format!("failed to write credentials file: {e}"))
        }
    }
}

/// Load a token for `base_url`. Precedence: `LIFIC_TOKEN` env (only when bound
/// to `base_url`'s origin, see [`env_token_for`]) > keyring > file.
pub fn load(base_url: &str) -> Option<String> {
    load_with_source(base_url).map(|(token, _)| token)
}

/// Describes where a loaded token came from, for `doctor`'s detail note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    Env,
    Keyring,
    File,
}

impl TokenSource {
    pub fn label(self) -> &'static str {
        match self {
            TokenSource::Env => "LIFIC_TOKEN env",
            TokenSource::Keyring => "OS keyring",
            TokenSource::File => "credentials file",
        }
    }
}

/// Like [`load`] but also reports which backend supplied the token, so callers
/// (doctor) can tell the user where it came from.
pub fn load_with_source(base_url: &str) -> Option<(String, TokenSource)> {
    match env_token_for(
        std::env::var(TOKEN_ENV).ok().as_deref(),
        std::env::var(URL_ENV).ok().as_deref(),
        base_url,
    ) {
        EnvToken::Bound(token) => return Some((token, TokenSource::Env)),
        EnvToken::Unbound => warn_env_token_unbound(base_url),
        EnvToken::Absent => {}
    }
    let key = normalize_base_url(base_url);
    if let Some(tok) = keyring_load(&key) {
        return Some((tok, TokenSource::Keyring));
    }
    default_file_path()
        .and_then(|p| FileStore::new(p).load(&key))
        .map(|tok| (tok, TokenSource::File))
}

/// Delete the stored credential for `base_url` from BOTH backends. Returns
/// whether anything was removed from either.
pub fn delete(base_url: &str) -> bool {
    let key = normalize_base_url(base_url);
    let kr = keyring_delete(&key);
    let file = default_file_path()
        .map(|p| FileStore::new(p).delete(&key).unwrap_or(false))
        .unwrap_or(false);
    kr || file
}

// ── Keyring backend (thin wrappers so the public API stays backend-agnostic) ─

fn keyring_entry(key: &str) -> Result<keyring::Entry, keyring::Error> {
    keyring::Entry::new(KEYRING_SERVICE, key)
}

fn keyring_store(key: &str, token: &str) -> Result<(), String> {
    let entry = keyring_entry(key).map_err(|e| e.to_string())?;
    entry.set_password(token).map_err(|e| e.to_string())
}

fn keyring_load(key: &str) -> Option<String> {
    keyring_entry(key).ok()?.get_password().ok()
}

fn keyring_delete(key: &str) -> bool {
    match keyring_entry(key) {
        Ok(entry) => entry.delete_credential().is_ok(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file store in a fresh scratch directory. Hold onto the returned
    /// [`TempDir`]: dropping it removes the directory, which also happens
    /// while a failed assertion unwinds.
    fn tmp_store() -> (FileStore, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("credentials.json");
        (FileStore::new(path), tmp)
    }

    #[test]
    fn normalize_base_url_strips_trailing_slash_and_lowercases_scheme_and_host() {
        assert_eq!(
            normalize_base_url("http://Example.com:3998/"),
            "http://example.com:3998"
        );
        assert_eq!(
            normalize_base_url("  https://LIFIC.example  "),
            "https://lific.example"
        );
        // Same server, two spellings → one key.
        assert_eq!(
            normalize_base_url("http://127.0.0.1:3998"),
            normalize_base_url("http://127.0.0.1:3998/")
        );
    }

    #[test]
    fn normalize_base_url_preserves_path_case() {
        assert_eq!(
            normalize_base_url(" HTTPS://LIFIC.Example/CaseSensitive/Path/ "),
            "https://lific.example/CaseSensitive/Path"
        );
    }

    #[test]
    fn file_store_round_trip() {
        let (store, _g) = tmp_store();
        assert_eq!(store.load("http://a"), None);
        store.store("http://a", "tok-a").unwrap();
        store.store("http://b", "tok-b").unwrap();
        assert_eq!(store.load("http://a").as_deref(), Some("tok-a"));
        assert_eq!(store.load("http://b").as_deref(), Some("tok-b"));

        // Overwrite existing key.
        store.store("http://a", "tok-a2").unwrap();
        assert_eq!(store.load("http://a").as_deref(), Some("tok-a2"));
    }

    #[test]
    fn file_store_delete_removes_only_target() {
        let (store, _g) = tmp_store();
        store.store("http://a", "tok-a").unwrap();
        store.store("http://b", "tok-b").unwrap();

        assert!(store.delete("http://a").unwrap(), "delete reports removal");
        assert_eq!(store.load("http://a"), None);
        assert_eq!(store.load("http://b").as_deref(), Some("tok-b"));

        // Deleting a missing key is a no-op that reports false.
        assert!(!store.delete("http://missing").unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn file_store_writes_0600_file_and_0700_dir() {
        use std::os::unix::fs::PermissionsExt;
        let (store, _g) = tmp_store();
        store.store("http://a", "secret").unwrap();

        let file_mode = std::fs::metadata(&store.path).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600, "credentials file must be 0600");

        let dir_mode = std::fs::metadata(store.path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700, "parent dir must be 0700");
    }

    #[test]
    fn file_store_creates_missing_parent_dir() {
        let tmp = tempfile::tempdir().unwrap();
        // Path two levels deep, neither of which exists yet.
        let path = tmp.path().join("deep").join("nested").join("credentials.json");
        let store = FileStore::new(path.clone());
        store.store("http://a", "tok").unwrap();
        assert!(path.exists());
        assert_eq!(store.load("http://a").as_deref(), Some("tok"));
    }

    // ── Origin binding for the env token (LIF-408) ───────────────────────
    //
    // These exercise the decision as a pure function so nothing here reads or
    // writes the process environment (LIF-401: env-var tests in this repo race
    // across modules). `LIFIC_URL` in particular is read by the clap tests in
    // `cli::mod`, so no test may set it.

    #[test]
    fn origin_of_makes_default_ports_explicit() {
        assert_eq!(origin_of("https://h.example").unwrap(), "https://h.example:443");
        assert_eq!(
            origin_of("https://h.example:443").unwrap(),
            origin_of("https://h.example").unwrap()
        );
        assert_eq!(
            origin_of("http://h.example:80").unwrap(),
            origin_of("http://h.example").unwrap()
        );
        // The two default ports do not collapse into each other.
        assert_ne!(
            origin_of("http://h.example").unwrap(),
            origin_of("https://h.example").unwrap()
        );
    }

    #[test]
    fn origin_of_ignores_case_path_and_trailing_slash() {
        let base = origin_of("https://Lific.Example:3998").unwrap();
        assert_eq!(origin_of("https://lific.example:3998/").unwrap(), base);
        assert_eq!(origin_of("HTTPS://LIFIC.EXAMPLE:3998/a/b?q=1#f").unwrap(), base);
        assert_eq!(origin_of("  https://lific.example:3998  ").unwrap(), base);
    }

    #[test]
    fn origin_of_rejects_non_http_and_unparseable_urls() {
        assert_eq!(origin_of("file:///etc/passwd"), None);
        assert_eq!(origin_of("ftp://h.example"), None);
        assert_eq!(origin_of("not a url"), None);
        assert_eq!(origin_of(""), None);
    }

    #[test]
    fn env_token_attaches_when_target_origin_matches_env_url() {
        assert_eq!(
            env_token_for(Some("env-tok"), Some("https://ci.example"), "https://ci.example"),
            EnvToken::Bound("env-tok".into())
        );
        // Same origin spelled differently on either side still binds.
        assert_eq!(
            env_token_for(
                Some("  env-tok  "),
                Some("https://CI.Example:443/"),
                "https://ci.example/api/issues"
            ),
            EnvToken::Bound("env-tok".into())
        );
        assert_eq!(
            env_token_for(Some("env-tok"), Some("http://127.0.0.1:3998"), "http://127.0.0.1:3998/"),
            EnvToken::Bound("env-tok".into())
        );
    }

    #[test]
    fn env_token_is_dropped_when_target_origin_differs() {
        // The attack: a cwd config (or --url) points somewhere else.
        assert_eq!(
            env_token_for(Some("env-tok"), Some("https://ci.example"), "https://hostile.example"),
            EnvToken::Unbound
        );
        // Different port, same host.
        assert_eq!(
            env_token_for(Some("env-tok"), Some("http://127.0.0.1:3998"), "http://127.0.0.1:4000"),
            EnvToken::Unbound
        );
        // Different scheme, same host: an http downgrade must not carry it.
        assert_eq!(
            env_token_for(Some("env-tok"), Some("https://ci.example"), "http://ci.example"),
            EnvToken::Unbound
        );
        // Subdomain is a different origin.
        assert_eq!(
            env_token_for(Some("env-tok"), Some("https://ci.example"), "https://evil.ci.example"),
            EnvToken::Unbound
        );
        // Unparseable/non-http on either side never binds.
        assert_eq!(
            env_token_for(Some("env-tok"), Some("not a url"), "https://ci.example"),
            EnvToken::Unbound
        );
        assert_eq!(
            env_token_for(Some("env-tok"), Some("https://ci.example"), "not a url"),
            EnvToken::Unbound
        );
    }

    #[test]
    fn env_token_is_dropped_when_env_url_is_unset() {
        // LIFIC_URL unset and the target came from config/flag: no binding, so
        // the stored per-host credential must be used instead.
        assert_eq!(
            env_token_for(Some("env-tok"), None, "https://config.example"),
            EnvToken::Unbound
        );
        assert_eq!(
            env_token_for(Some("env-tok"), Some("   "), "https://config.example"),
            EnvToken::Unbound
        );
    }

    #[test]
    fn absent_env_token_leaves_stored_backends_alone() {
        assert_eq!(
            env_token_for(None, Some("https://ci.example"), "https://ci.example"),
            EnvToken::Absent
        );
        assert_eq!(
            env_token_for(Some("   "), Some("https://ci.example"), "https://ci.example"),
            EnvToken::Absent
        );
        // Absent, not Unbound, even with no LIFIC_URL: nothing to warn about.
        assert_eq!(env_token_for(None, None, "https://ci.example"), EnvToken::Absent);
    }

    // The remaining tests mutate the process env, so they serialize on a mutex
    // (LIF-401). They touch `LIFIC_TOKEN` only, never `LIFIC_URL`, which the
    // clap tests in `cli::mod` read.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn unbound_env_var_does_not_shadow_stored_credentials() {
        let _lock = ENV_LOCK.lock().unwrap();
        // A target no plausible ambient LIFIC_URL names, so the env token is
        // unbound whatever the developer's shell exports.
        let target = "http://unbound-envtest.invalid:1";

        // SAFETY: guarded by ENV_LOCK; restored below.
        unsafe { std::env::set_var(TOKEN_ENV, "env-tok") };
        let got = load(target);
        unsafe { std::env::remove_var(TOKEN_ENV) };

        assert_ne!(
            got.as_deref(),
            Some("env-tok"),
            "an env token not bound to the target origin must never be sent there"
        );
    }

    #[test]
    fn empty_env_var_is_ignored() {
        let _lock = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var(TOKEN_ENV, "   ") };
        // An all-whitespace env var must not shadow real backends.
        let got_source = load_with_source("http://noenv-empty");
        unsafe { std::env::remove_var(TOKEN_ENV) };
        // No token anywhere for this URL → None (env ignored).
        assert!(got_source.is_none() || got_source.unwrap().1 != TokenSource::Env);
    }

    #[test]
    fn token_source_labels() {
        assert_eq!(TokenSource::Env.label(), "LIFIC_TOKEN env");
        assert_eq!(TokenSource::Keyring.label(), "OS keyring");
        assert_eq!(TokenSource::File.label(), "credentials file");
    }
}
