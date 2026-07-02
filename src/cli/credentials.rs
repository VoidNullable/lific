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
//! Load precedence: `LIFIC_TOKEN` env var > keyring > file. The env var lets an
//! agent or CI inject a token without any on-disk state. (The existing
//! `LIFIC_API_KEY` is for API keys and is deliberately left untouched.)
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

/// Environment variable that overrides all stored credentials for OAuth tokens.
pub const TOKEN_ENV: &str = "LIFIC_TOKEN";

/// Keyring service name (namespace) for all Lific credentials.
const KEYRING_SERVICE: &str = "lific";

/// Normalize a base URL into a stable credential key: trim, drop any trailing
/// slash, and lowercase the scheme+host. Two spellings of the same server
/// (`http://H:3998` vs `http://H:3998/`) must resolve to one entry.
pub fn normalize_base_url(base: &str) -> String {
    let trimmed = base.trim().trim_end_matches('/');
    // Lowercase only the scheme://host portion; leave any path/port digits as-is
    // (ports are digits, hosts are case-insensitive). Simplest robust approach:
    // lowercase the whole thing — URLs have no case-sensitive component that
    // matters for our keying (we never store paths with case-sensitive segments).
    trimmed.to_ascii_lowercase()
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

/// Load a token for `base_url`. Precedence: `LIFIC_TOKEN` env > keyring > file.
pub fn load(base_url: &str) -> Option<String> {
    if let Ok(tok) = std::env::var(TOKEN_ENV) {
        let tok = tok.trim().to_string();
        if !tok.is_empty() {
            return Some(tok);
        }
    }
    let key = normalize_base_url(base_url);
    if let Some(tok) = keyring_load(&key) {
        return Some(tok);
    }
    default_file_path().and_then(|p| FileStore::new(p).load(&key))
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
    if let Ok(tok) = std::env::var(TOKEN_ENV) {
        let tok = tok.trim().to_string();
        if !tok.is_empty() {
            return Some((tok, TokenSource::Env));
        }
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

    fn tmp_store() -> (FileStore, tempdir::Guard) {
        let dir = std::env::temp_dir().join(format!(
            "lific_creds_{}_{}",
            std::process::id(),
            rand::random::<u32>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("credentials.json");
        (FileStore::new(path), tempdir::Guard(dir))
    }

    // Minimal RAII tempdir cleanup so tests don't litter /tmp.
    mod tempdir {
        pub struct Guard(pub std::path::PathBuf);
        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn normalize_base_url_strips_trailing_slash_and_lowercases() {
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
        let base = std::env::temp_dir().join(format!(
            "lific_creds_deep_{}_{}",
            std::process::id(),
            rand::random::<u32>()
        ));
        let _g = tempdir::Guard(base.clone());
        // Path two levels deep, neither of which exists yet.
        let path = base.join("nested").join("credentials.json");
        let store = FileStore::new(path.clone());
        store.store("http://a", "tok").unwrap();
        assert!(path.exists());
        assert_eq!(store.load("http://a").as_deref(), Some("tok"));
    }

    // Env precedence is tested at the FileStore level here (no keyring), plus a
    // direct check that a set env var wins. We serialize env mutation with a
    // mutex because the process env is global.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn env_var_takes_precedence_over_file() {
        let _lock = ENV_LOCK.lock().unwrap();
        let (store, _g) = tmp_store();
        store.store(&normalize_base_url("http://envtest"), "file-tok").unwrap();

        // With env set, load() must return the env token regardless of file.
        // SAFETY: guarded by ENV_LOCK; restored below.
        unsafe { std::env::set_var(TOKEN_ENV, "env-tok") };
        let got = load("http://envtest");
        unsafe { std::env::remove_var(TOKEN_ENV) };

        assert_eq!(got.as_deref(), Some("env-tok"));
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
