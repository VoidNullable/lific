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
//!    of `base_url → token`, written with owner-only protection under a
//!    private parent. Used when the keyring is unavailable (headless box with
//!    no Secret Service, CI, etc.).
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
//! The file backend is factored behind [`PlaintextCredentialFileStore`] with an injectable path,
//! so the round-trip / permission / precedence tests never touch the real
//! keyring or the real `~/.config`. The keyring itself is only reachable
//! through [`store`]/[`load`]/[`delete`]; any test that would hit a live Secret
//! Service is gated `#[ignore]` (CI has none).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use thiserror::Error;

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
    crate::cli::ui::stderr_line(format_args!(
        "warning: {TOKEN_ENV} is set but not bound to {target_url}; ignoring it and using the \
         stored credential for that server instead. Set {URL_ENV} to that server to send the \
         env token there."
    ));
}

/// Where the plaintext fallback file lives: `~/.config/lific/credentials.json`.
fn default_file_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("lific").join("credentials.json"))
}

// ── Registered OAuth client ids ──────────────────────────────────────────
//
// The device flow needs a registered `client_id`, and registering a fresh one
// on every `lific login` is not free on the server: a client that has ever
// minted a token can never be reclaimed, so each login would permanently
// consume one of the instance's dynamic-client slots, and each would also
// spend one of that IP's ten hourly registrations. So the id is remembered
// per server and reused.
//
// A `client_id` is a public identifier, not a credential, so it lives in a
// plain file next to the credential store rather than in the keyring, and its
// absence is never an error.

/// Where remembered client ids live: `~/.config/lific/clients.json`, a map of
/// `base_url → client_id`.
fn default_client_file_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("lific").join("clients.json"))
}

fn client_store() -> Option<OAuthClientRegistrationCache> {
    default_client_file_path().map(OAuthClientRegistrationCache::new)
}

// The three operations are factored to take the store, like the credential
// file backend, so the round-trip is testable without touching a real
// `~/.config`. A missing file is harmless; a present but unreadable file is
// reported to the login flow rather than silently discarded.

fn store_client_id_in(
    store: &OAuthClientRegistrationCache,
    base_url: &str,
    client_id: &str,
) -> Result<(), OAuthClientCacheError> {
    store.remember(base_url, client_id)
}

fn load_client_id_from(
    store: &OAuthClientRegistrationCache,
    base_url: &str,
) -> Result<Option<String>, OAuthClientCacheError> {
    store.load(base_url)
}

fn forget_client_id_in(
    store: &OAuthClientRegistrationCache,
    base_url: &str,
) -> Result<(), OAuthClientCacheError> {
    store.forget(base_url)
}

/// Remember the OAuth `client_id` registered with `base_url`.
pub fn store_client_id(base_url: &str, client_id: &str) -> Result<(), OAuthClientCacheError> {
    if let Some(store) = client_store() {
        store_client_id_in(&store, base_url, client_id)
    } else {
        Ok(())
    }
}

/// The `client_id` previously registered with `base_url`, if any.
pub fn load_client_id(base_url: &str) -> Result<Option<String>, OAuthClientCacheError> {
    match client_store() {
        Some(store) => load_client_id_from(&store, base_url),
        None => Ok(None),
    }
}

/// Drop the remembered `client_id` for `base_url`, after the server said it
/// does not know it (reclaimed, or a rebuilt database).
pub fn forget_client_id(base_url: &str) -> Result<(), OAuthClientCacheError> {
    if let Some(store) = client_store() {
        forget_client_id_in(&store, base_url)
    } else {
        Ok(())
    }
}

/// The public OAuth client-registration cache deliberately has different
/// naming from the credential store. Its contents are public identifiers;
/// malformed or unsafe data is still a hard error when read, but callers may
/// choose to warn and continue after a successful remote registration.
#[derive(Debug, Error)]
pub enum OAuthClientCacheError {
    #[error("OAuth client cache {path}: {detail}")]
    Storage {
        path: PathBuf,
        detail: String,
        #[source]
        source: Box<PlaintextCredentialFileError>,
    },
}

struct OAuthClientRegistrationCache {
    store: PlaintextCredentialFileStore,
}

impl OAuthClientRegistrationCache {
    fn new(path: PathBuf) -> Self {
        Self {
            store: PlaintextCredentialFileStore::new(path),
        }
    }

    fn load(&self, base_url: &str) -> Result<Option<String>, OAuthClientCacheError> {
        self.store
            .load_credential_for_server_key(&CredentialStoreKey::from_base_url(base_url))
            .map_err(|source| self.error("could not load", source))
    }

    fn remember(&self, base_url: &str, client_id: &str) -> Result<(), OAuthClientCacheError> {
        self.store
            .store_credential_for_server_key(
                &CredentialStoreKey::from_base_url(base_url),
                client_id,
            )
            .map_err(|source| self.error("could not store", source))
    }

    fn forget(&self, base_url: &str) -> Result<(), OAuthClientCacheError> {
        self.store
            .delete_credential_for_server_key(&CredentialStoreKey::from_base_url(base_url))
            .map(|_| ())
            .map_err(|source| self.error("could not delete", source))
    }

    fn error(
        &self,
        operation: &str,
        source: PlaintextCredentialFileError,
    ) -> OAuthClientCacheError {
        let source_detail = source
            .to_string()
            .replace("credential", "OAuth client cache");
        let detail = format!("{operation}: {source_detail}");
        OAuthClientCacheError::Storage {
            path: self.store.path.clone(),
            detail,
            source: Box::new(source),
        }
    }
}

// ── Plaintext credential file backend ───────────────────────────────────

/// Normalized key used to select one server's credential in the file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CredentialStoreKey(String);

impl CredentialStoreKey {
    /// Normalize a server URL for use as a credential-file key.
    pub fn from_base_url(base_url: &str) -> Self {
        Self(normalize_base_url(base_url))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

/// A failure while reading or mutating the plaintext credential file.
#[derive(Debug, Error)]
pub enum PlaintextCredentialFileError {
    #[error("failed to read credential file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("credential file {path} is a symbolic link")]
    Symlink { path: PathBuf },
    #[error("credential lock {path} is a symbolic link")]
    LockSymlink { path: PathBuf },
    #[error("credential lock {path} is not a regular file")]
    LockNotRegular { path: PathBuf },
    #[error("credential lock {path} has multiple hard links")]
    LockHardLinked { path: PathBuf },
    #[error("credential file {path} is not a regular file")]
    NotRegular { path: PathBuf },
    #[error("credential file {path} has multiple hard links")]
    HardLinked { path: PathBuf },
    #[error("credential file {path} is not valid UTF-8: {source}")]
    InvalidUtf8 {
        path: PathBuf,
        #[source]
        source: std::string::FromUtf8Error,
    },
    #[error("credential file {path} contains invalid JSON: {source}")]
    InvalidJson {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to open credential lock {path}: {source}")]
    LockOpen {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to lock credential store {path}: {source}")]
    Lock {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to secure credential directory {path}: {source}")]
    SecureDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("credential directory {path} is not a directory")]
    DirectoryNotRegular { path: PathBuf },
    #[error("credential directory {path} is a symbolic link")]
    DirectorySymlink { path: PathBuf },
    #[cfg(unix)]
    #[error("failed to apply private credential permissions to {path}: {source}")]
    Permissions {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to create credential staging file in {path}: {source}")]
    StageOpen {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write staged credential file for {path}: {source}")]
    StageWrite {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to synchronize staged credential file for {path}: {source}")]
    StageSync {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to publish credential file {path}: {source}")]
    Publish {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[cfg(unix)]
    #[error("failed to synchronize credential directory {path}: {source}")]
    DirectorySync {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// JSON-on-disk fallback for credentials, parameterized on its path for tests.
pub struct PlaintextCredentialFileStore {
    path: PathBuf,
}

impl PlaintextCredentialFileStore {
    /// Create a store backed by `path`. Reads and mutations use the path only
    /// as a name; existing files are opened with no-follow semantics.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn parent_directory(&self) -> &Path {
        self.path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
    }

    fn lock_path(&self) -> PathBuf {
        let mut name = OsString::from(self.path.as_os_str());
        name.push(".lock");
        PathBuf::from(name)
    }

    /// Read one complete credential generation, or report that the file is
    /// absent. The descriptor supplies both metadata and bytes, so a final
    /// path-component symlink or hard link is never followed or replaced.
    fn read_existing_credentials_or_return_absent(
        &self,
    ) -> Result<Option<BTreeMap<String, String>>, PlaintextCredentialFileError> {
        #[cfg(windows)]
        match std::fs::symlink_metadata(&self.path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(PlaintextCredentialFileError::Symlink {
                    path: self.path.clone(),
                });
            }
            Ok(metadata) if !metadata.file_type().is_file() => {
                return Err(PlaintextCredentialFileError::NotRegular {
                    path: self.path.clone(),
                });
            }
            Ok(_) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(PlaintextCredentialFileError::Read {
                    path: self.path.clone(),
                    source,
                });
            }
        }

        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        configure_no_follow(&mut options);
        let mut file = match options.open(&self.path) {
            Ok(file) => file,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) if is_symlink_open_error(&source) => {
                return Err(PlaintextCredentialFileError::Symlink {
                    path: self.path.clone(),
                });
            }
            Err(source) => {
                return Err(PlaintextCredentialFileError::Read {
                    path: self.path.clone(),
                    source,
                });
            }
        };

        #[cfg(windows)]
        if std::fs::symlink_metadata(&self.path)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(PlaintextCredentialFileError::Symlink {
                path: self.path.clone(),
            });
        }

        let metadata = file
            .metadata()
            .map_err(|source| PlaintextCredentialFileError::Read {
                path: self.path.clone(),
                source,
            })?;
        if !metadata.file_type().is_file() {
            return Err(PlaintextCredentialFileError::NotRegular {
                path: self.path.clone(),
            });
        }
        if file_has_multiple_links(&metadata) {
            return Err(PlaintextCredentialFileError::HardLinked {
                path: self.path.clone(),
            });
        }

        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|source| PlaintextCredentialFileError::Read {
                path: self.path.clone(),
                source,
            })?;
        let contents = String::from_utf8(bytes).map_err(|source| {
            PlaintextCredentialFileError::InvalidUtf8 {
                path: self.path.clone(),
                source,
            }
        })?;
        serde_json::from_str(&contents).map(Some).map_err(|source| {
            PlaintextCredentialFileError::InvalidJson {
                path: self.path.clone(),
                source,
            }
        })
    }

    /// Create and restrict the immediate parent directory before any mutation.
    /// Existing parents are tightened to owner-only Unix permissions; on
    /// Windows the platform ACL/default security model remains authoritative.
    fn secure_parent_directory(&self) -> Result<(), PlaintextCredentialFileError> {
        let parent = self.parent_directory();
        std::fs::create_dir_all(parent).map_err(|source| {
            PlaintextCredentialFileError::SecureDirectory {
                path: parent.to_owned(),
                source,
            }
        })?;

        let metadata = std::fs::symlink_metadata(parent).map_err(|source| {
            PlaintextCredentialFileError::SecureDirectory {
                path: parent.to_owned(),
                source,
            }
        })?;
        if metadata.file_type().is_symlink() {
            return Err(PlaintextCredentialFileError::DirectorySymlink {
                path: parent.to_owned(),
            });
        }
        if !metadata.file_type().is_dir() {
            return Err(PlaintextCredentialFileError::DirectoryNotRegular {
                path: parent.to_owned(),
            });
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            let mut options = std::fs::OpenOptions::new();
            options.read(true).custom_flags(libc::O_NOFOLLOW);
            let directory = options.open(parent).map_err(|source| {
                PlaintextCredentialFileError::SecureDirectory {
                    path: parent.to_owned(),
                    source,
                }
            })?;
            directory
                .set_permissions(std::fs::Permissions::from_mode(0o700))
                .map_err(|source| PlaintextCredentialFileError::SecureDirectory {
                    path: parent.to_owned(),
                    source,
                })?;
        }
        Ok(())
    }

    /// Open and exclusively lock the stable sibling lock file. The returned
    /// file owns the lock; dropping it releases the lock after publication.
    fn lock_for_mutation(&self) -> Result<std::fs::File, PlaintextCredentialFileError> {
        let path = self.lock_path();
        #[cfg(windows)]
        if std::fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(PlaintextCredentialFileError::LockSymlink { path });
        }

        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true);
        configure_no_follow(&mut options);
        let file = options.open(&path).map_err(|source| {
            if is_symlink_open_error(&source) {
                PlaintextCredentialFileError::LockSymlink { path: path.clone() }
            } else {
                PlaintextCredentialFileError::LockOpen {
                    path: path.clone(),
                    source,
                }
            }
        })?;
        let metadata =
            file.metadata()
                .map_err(|source| PlaintextCredentialFileError::LockOpen {
                    path: path.clone(),
                    source,
                })?;
        if !metadata.file_type().is_file() {
            return Err(PlaintextCredentialFileError::LockNotRegular { path });
        }
        if file_has_multiple_links(&metadata) {
            return Err(PlaintextCredentialFileError::LockHardLinked { path });
        }
        set_open_credential_file_owner_only_permissions(&file, &path)?;
        file.lock_exclusive()
            .map_err(|source| PlaintextCredentialFileError::Lock { path, source })?;
        Ok(file)
    }

    /// Read the current valid map, treating only an absent document as empty.
    fn read_credential_map(
        &self,
    ) -> Result<BTreeMap<String, String>, PlaintextCredentialFileError> {
        Ok(self
            .read_existing_credentials_or_return_absent()?
            .unwrap_or_default())
    }

    /// Load the credential for one normalized server key. `Ok(None)` means
    /// only an absent file, a valid empty map, or an absent key; corruption,
    /// links, permissions, and I/O failures remain errors.
    pub fn load_credential_for_server_key(
        &self,
        key: &CredentialStoreKey,
    ) -> Result<Option<String>, PlaintextCredentialFileError> {
        Ok(self
            .read_existing_credentials_or_return_absent()?
            .and_then(|map| map.get(key.as_str()).cloned()))
    }

    /// Store a credential while holding a stable sibling lock across the
    /// complete read-modify-write-publication transaction.
    pub fn store_credential_for_server_key(
        &self,
        key: &CredentialStoreKey,
        credential: &str,
    ) -> Result<(), PlaintextCredentialFileError> {
        self.secure_parent_directory()?;
        let _lock = self.lock_for_mutation()?;
        let mut map = self.read_credential_map()?;
        map.insert(key.as_str().to_owned(), credential.to_owned());
        self.atomically_publish_synchronized_credential_document(&map)
    }

    /// Delete a credential under the same lock used for stores. A valid map
    /// with no matching key is a successful no-op; malformed or unsafe files
    /// are errors and are left untouched.
    pub fn delete_credential_for_server_key(
        &self,
        key: &CredentialStoreKey,
    ) -> Result<bool, PlaintextCredentialFileError> {
        self.secure_parent_directory()?;
        let _lock = self.lock_for_mutation()?;
        let mut map = self.read_credential_map()?;
        let removed = map.remove(key.as_str()).is_some();
        if removed {
            self.atomically_publish_synchronized_credential_document(&map)?;
        }
        Ok(removed)
    }

    /// Serialize, protect, flush, atomically publish, and durably sync one
    /// credential generation. The caller holds the sibling lock for the full
    /// read/modify/stage/publish transaction.
    fn atomically_publish_synchronized_credential_document(
        &self,
        map: &BTreeMap<String, String>,
    ) -> Result<(), PlaintextCredentialFileError> {
        let json = serde_json::to_vec_pretty(map).map_err(|source| {
            PlaintextCredentialFileError::StageWrite {
                path: self.path.clone(),
                source: std::io::Error::other(source),
            }
        })?;
        let parent = self.parent_directory();
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|source| {
            PlaintextCredentialFileError::StageOpen {
                path: parent.to_owned(),
                source,
            }
        })?;
        set_open_credential_file_owner_only_permissions(temporary.as_file(), temporary.path())?;
        temporary.as_file_mut().write_all(&json).map_err(|source| {
            PlaintextCredentialFileError::StageWrite {
                path: self.path.clone(),
                source,
            }
        })?;
        temporary.as_file().sync_all().map_err(|source| {
            PlaintextCredentialFileError::StageSync {
                path: self.path.clone(),
                source,
            }
        })?;
        temporary
            .persist(&self.path)
            .map_err(|error| PlaintextCredentialFileError::Publish {
                path: self.path.clone(),
                source: error.error,
            })?;
        synchronize_credential_parent_directory(parent)
    }
}

/// Apply owner-only protection to an open credential or lock file.
#[cfg_attr(
    not(unix),
    expect(clippy::unnecessary_wraps, reason = "fallible on Unix")
)]
fn set_open_credential_file_owner_only_permissions(
    file: &std::fs::File,
    path: &Path,
) -> Result<(), PlaintextCredentialFileError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|source| PlaintextCredentialFileError::Permissions {
                path: path.to_owned(),
                source,
            })?;
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        let _ = path;
    }
    Ok(())
}

/// Sync the directory entry after publishing a credential file. File sync
/// persists bytes; directory sync persists the name-to-inode replacement.
#[cfg_attr(
    not(unix),
    expect(clippy::unnecessary_wraps, reason = "fallible on Unix")
)]
fn synchronize_credential_parent_directory(
    parent: &Path,
) -> Result<(), PlaintextCredentialFileError> {
    #[cfg(unix)]
    {
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|source| PlaintextCredentialFileError::DirectorySync {
                path: parent.to_owned(),
                source,
            })?;
    }
    #[cfg(not(unix))]
    {
        let _ = parent;
    }
    Ok(())
}

/// Configure the strongest stable final-component no-follow flag available
/// on the target platform. Windows reparse points are rejected by handle
/// metadata; ancestor replacement remains outside this path-level guarantee.
fn configure_no_follow(options: &mut std::fs::OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_OPEN_REPARSE_POINT: open a reparse point itself instead
        // of traversing it. The handle is then rejected as a symlink below.
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
}

fn is_symlink_open_error(error: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        error.raw_os_error() == Some(libc::ELOOP)
    }
    #[cfg(not(unix))]
    {
        let _ = error;
        false
    }
}

fn file_has_multiple_links(metadata: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.nlink() != 1
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

// ── Public API (keyring + file, with env override on load) ───────────────

#[derive(Debug, Error)]
pub enum CredentialStoreError {
    #[error("cannot resolve the platform configuration directory")]
    ConfigDirectoryUnavailable,
    #[error(transparent)]
    Plaintext(#[from] PlaintextCredentialFileError),
}

/// Store a token for `base_url`. Tries the keyring first; on any keyring error
/// falls back to the plaintext file and prints a loud warning to stderr.
pub fn store(base_url: &str, token: &str) -> Result<(), CredentialStoreError> {
    let key = CredentialStoreKey::from_base_url(base_url);
    match keyring_store(key.as_str(), token) {
        Ok(()) => Ok(()),
        Err(e) => {
            let store = PlaintextCredentialFileStore::new(
                default_file_path().ok_or(CredentialStoreError::ConfigDirectoryUnavailable)?,
            );
            crate::cli::ui::stderr_line(format_args!(
                "warning: OS keyring unavailable ({e}); storing token in PLAINTEXT at {} ({}). \
                 Set up a Secret Service/Keychain to secure it, or use {TOKEN_ENV} to avoid on-disk storage.",
                store.path.display(),
                plaintext_protection_description()
            ));
            store
                .store_credential_for_server_key(&key, token)
                .map_err(CredentialStoreError::from)
        }
    }
}

/// Load a token for `base_url`. Precedence: `LIFIC_TOKEN` env (only when bound
/// to `base_url`'s origin, see [`env_token_for`]) > keyring > file.
pub fn load(base_url: &str) -> Result<Option<String>, PlaintextCredentialFileError> {
    load_with_source(base_url).map(|value| value.map(|(token, _)| token))
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
pub fn load_with_source(
    base_url: &str,
) -> Result<Option<(String, TokenSource)>, PlaintextCredentialFileError> {
    match env_token_for(
        std::env::var(TOKEN_ENV).ok().as_deref(),
        std::env::var(URL_ENV).ok().as_deref(),
        base_url,
    ) {
        EnvToken::Bound(token) => return Ok(Some((token, TokenSource::Env))),
        EnvToken::Unbound => warn_env_token_unbound(base_url),
        EnvToken::Absent => {}
    }
    let key = CredentialStoreKey::from_base_url(base_url);
    if let Some(tok) = keyring_load(key.as_str()) {
        return Ok(Some((tok, TokenSource::Keyring)));
    }
    match default_file_path() {
        Some(path) => PlaintextCredentialFileStore::new(path)
            .load_credential_for_server_key(&key)
            .map(|token| token.map(|token| (token, TokenSource::File))),
        None => Ok(None),
    }
}

/// Load a token for `base_url` from the on-disk stores only: keyring, then
/// file. `LIFIC_TOKEN` is not consulted.
///
/// [`load`] gives an origin-matching `LIFIC_TOKEN` highest precedence, which is
/// right for a typed CLI invocation and wrong for the multi-instance MCP proxy
/// (LIF-466): there it would become the credential for every alias sharing that
/// origin. An alias that wants an env var names it with `token_env`.
pub fn load_stored(base_url: &str) -> Result<Option<String>, PlaintextCredentialFileError> {
    let key = CredentialStoreKey::from_base_url(base_url);
    if let Some(token) = keyring_load(key.as_str()) {
        return Ok(Some(token));
    }
    match default_file_path() {
        Some(path) => PlaintextCredentialFileStore::new(path).load_credential_for_server_key(&key),
        None => Ok(None),
    }
}

/// Delete the stored credential for `base_url` from BOTH backends.
///
/// The plaintext store is attempted first. That means a plaintext corruption
/// or safety error returns before the keyring is changed. Keyring deletion is
/// best-effort, so a successful result can still mean that only the plaintext
/// copy was removed; the two backends are not a transaction.
pub fn delete(base_url: &str) -> Result<bool, PlaintextCredentialFileError> {
    let key = CredentialStoreKey::from_base_url(base_url);
    let file = match default_file_path() {
        Some(path) => {
            PlaintextCredentialFileStore::new(path).delete_credential_for_server_key(&key)?
        }
        None => false,
    };
    let kr = keyring_delete(key.as_str());
    Ok(kr || file)
}

#[cfg(unix)]
const fn plaintext_protection_description() -> &'static str {
    "0600 file permissions"
}

#[cfg(not(unix))]
const fn plaintext_protection_description() -> &'static str {
    "platform file permissions"
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
    use proptest::prelude::*;

    /// A file store in a fresh scratch directory. Hold onto the returned
    /// [`TempDir`]: dropping it removes the directory, which also happens
    /// while a failed assertion unwinds.
    fn tmp_store() -> (PlaintextCredentialFileStore, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("credentials.json");
        (PlaintextCredentialFileStore::new(path), tmp)
    }

    /// The remembered client id is what keeps `lific login` from registering
    /// a fresh OAuth client on every run. Store and load have to agree on the
    /// key, so two spellings of the same server must hit one entry, and a
    /// server that has forgotten the client must be forgettable here too.
    #[test]
    fn a_client_id_round_trips_per_server_and_can_be_forgotten() {
        let tmp = tempfile::tempdir().unwrap();
        let store = OAuthClientRegistrationCache::new(tmp.path().join("clients.json"));

        assert_eq!(
            load_client_id_from(&store, "http://127.0.0.1:3998").unwrap(),
            None
        );

        store_client_id_in(&store, "http://127.0.0.1:3998", "client-abc").unwrap();
        // Same server, different spelling → same entry.
        assert_eq!(
            load_client_id_from(&store, "http://127.0.0.1:3998/").unwrap(),
            Some("client-abc".to_string())
        );

        // A second server keeps its own id.
        store_client_id_in(&store, "https://lific.example", "client-xyz").unwrap();
        assert_eq!(
            load_client_id_from(&store, "https://lific.example").unwrap(),
            Some("client-xyz".to_string())
        );
        assert_eq!(
            load_client_id_from(&store, "http://127.0.0.1:3998").unwrap(),
            Some("client-abc".to_string())
        );

        forget_client_id_in(&store, "http://127.0.0.1:3998/").unwrap();
        assert_eq!(
            load_client_id_from(&store, "http://127.0.0.1:3998").unwrap(),
            None
        );
        assert_eq!(
            load_client_id_from(&store, "https://lific.example").unwrap(),
            Some("client-xyz".to_string()),
            "forgetting one server must not clear another"
        );
    }

    #[test]
    fn client_cache_errors_name_the_public_cache_not_credentials() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("clients.json");
        std::fs::write(&path, br#"{"http://a": broken}"#).unwrap();
        let cache = OAuthClientRegistrationCache::new(path);

        let error = cache.load("http://a").unwrap_err();

        assert!(error.to_string().contains("OAuth client cache"));
        assert!(error.to_string().contains("invalid JSON"));
        assert!(!error.to_string().contains("credential file"));
    }

    proptest! {
        #[test]
        fn oauth_client_cache_round_trips_arbitrary_identifiers(client_id in any::<String>()) {
            let tmp = tempfile::tempdir().unwrap();
            let cache = OAuthClientRegistrationCache::new(tmp.path().join("clients.json"));

            cache.remember("https://lific.example", &client_id).unwrap();

            prop_assert_eq!(cache.load("https://lific.example").unwrap(), Some(client_id));
        }
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
    fn plaintext_store_round_trip() {
        let (store, _g) = tmp_store();
        let a = CredentialStoreKey::from_base_url("http://a");
        let b = CredentialStoreKey::from_base_url("http://b");
        assert_eq!(store.load_credential_for_server_key(&a).unwrap(), None);
        store.store_credential_for_server_key(&a, "tok-a").unwrap();
        store.store_credential_for_server_key(&b, "tok-b").unwrap();
        assert_eq!(
            store.load_credential_for_server_key(&a).unwrap().as_deref(),
            Some("tok-a")
        );
        assert_eq!(
            store.load_credential_for_server_key(&b).unwrap().as_deref(),
            Some("tok-b")
        );

        // Overwrite existing key.
        store.store_credential_for_server_key(&a, "tok-a2").unwrap();
        assert_eq!(
            store.load_credential_for_server_key(&a).unwrap().as_deref(),
            Some("tok-a2")
        );
    }

    #[test]
    fn plaintext_store_delete_removes_only_target() {
        let (store, _g) = tmp_store();
        let a = CredentialStoreKey::from_base_url("http://a");
        let b = CredentialStoreKey::from_base_url("http://b");
        let missing = CredentialStoreKey::from_base_url("http://missing");
        store.store_credential_for_server_key(&a, "tok-a").unwrap();
        store.store_credential_for_server_key(&b, "tok-b").unwrap();

        assert!(
            store.delete_credential_for_server_key(&a).unwrap(),
            "delete reports removal"
        );
        assert_eq!(store.load_credential_for_server_key(&a).unwrap(), None);
        assert_eq!(
            store.load_credential_for_server_key(&b).unwrap().as_deref(),
            Some("tok-b")
        );

        // Deleting a missing key is a no-op that reports false.
        assert!(!store.delete_credential_for_server_key(&missing).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn plaintext_store_writes_0600_file_and_0700_dir() {
        use std::os::unix::fs::PermissionsExt;
        let (store, _g) = tmp_store();
        store
            .store_credential_for_server_key(
                &CredentialStoreKey::from_base_url("http://a"),
                "secret",
            )
            .unwrap();

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
    fn plaintext_store_creates_missing_parent_dir() {
        let tmp = tempfile::tempdir().unwrap();
        // Path two levels deep, neither of which exists yet.
        let path = tmp
            .path()
            .join("deep")
            .join("nested")
            .join("credentials.json");
        let store = PlaintextCredentialFileStore::new(path.clone());
        store
            .store_credential_for_server_key(&CredentialStoreKey::from_base_url("http://a"), "tok")
            .unwrap();
        assert!(path.exists());
        assert_eq!(
            store
                .load_credential_for_server_key(&CredentialStoreKey::from_base_url("http://a"))
                .unwrap()
                .as_deref(),
            Some("tok")
        );
    }

    #[test]
    fn malformed_credential_file_is_a_typed_read_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("credentials.json");
        let original = br#"{"http://a": broken}"#;
        std::fs::write(&path, original).unwrap();
        let store = PlaintextCredentialFileStore::new(path.clone());
        let key = CredentialStoreKey::from_base_url("http://a");

        assert!(matches!(
            store.load_credential_for_server_key(&key),
            Err(PlaintextCredentialFileError::InvalidJson { .. })
        ));
        assert_eq!(std::fs::read(path).unwrap(), original);
    }

    #[test]
    fn plaintext_store_rejects_malformed_existing_data_without_replacing_it() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("credentials.json");
        let original = br#"{"http://a":"old-token", broken}"#;
        std::fs::write(&path, original).unwrap();
        let store = PlaintextCredentialFileStore::new(path.clone());

        assert!(
            store
                .store_credential_for_server_key(
                    &CredentialStoreKey::from_base_url("http://a"),
                    "new-token"
                )
                .is_err()
        );
        assert_eq!(std::fs::read(path).unwrap(), original);
    }

    #[test]
    fn invalid_credential_documents_are_errors_and_remain_unchanged() {
        let cases: &[(&str, &[u8])] = &[
            ("malformed syntax", br#"{"http://a": broken}"#),
            ("truncated object", br#"{"http://a":"token""#),
            ("wrong top-level type", br#"["token"]"#),
            ("non-string value", br#"{"http://a":42}"#),
            ("invalid UTF-8", b"{\xff"),
        ];
        for (name, original) in cases {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("credentials.json");
            std::fs::write(&path, original).unwrap();
            let store = PlaintextCredentialFileStore::new(path.clone());
            let key = CredentialStoreKey::from_base_url("http://a");

            assert!(
                store.load_credential_for_server_key(&key).is_err(),
                "{name}"
            );
            assert!(
                store.store_credential_for_server_key(&key, "new").is_err(),
                "{name}"
            );
            assert!(
                store.delete_credential_for_server_key(&key).is_err(),
                "{name}"
            );
            assert_eq!(std::fs::read(path).unwrap(), *original, "{name}");
        }
    }

    #[test]
    fn absent_empty_and_populated_documents_have_distinct_successful_results() {
        let (store, _tmp) = tmp_store();
        let key = CredentialStoreKey::from_base_url("http://a");
        assert_eq!(store.load_credential_for_server_key(&key).unwrap(), None);

        std::fs::write(&store.path, b"{}").unwrap();
        assert_eq!(store.load_credential_for_server_key(&key).unwrap(), None);

        store
            .store_credential_for_server_key(&key, "token")
            .unwrap();
        assert_eq!(
            store
                .load_credential_for_server_key(&key)
                .unwrap()
                .as_deref(),
            Some("token")
        );
    }

    #[test]
    fn credential_path_must_be_a_regular_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("credentials.json");
        std::fs::create_dir(&path).unwrap();
        let store = PlaintextCredentialFileStore::new(path.clone());
        let key = CredentialStoreKey::from_base_url("http://a");

        assert!(matches!(
            store.load_credential_for_server_key(&key),
            Err(PlaintextCredentialFileError::NotRegular { .. })
        ));
        assert!(matches!(
            store.store_credential_for_server_key(&key, "new"),
            Err(PlaintextCredentialFileError::NotRegular { .. })
        ));
        assert!(
            path.is_dir(),
            "a rejected directory must remain a directory"
        );
    }

    proptest! {
        // Arbitrary Unicode exercises JSON escaping and round-trip behavior
        // that a finite table of representative strings cannot cover.
        #[test]
        fn arbitrary_credential_text_round_trips(token in any::<String>()) {
            let tmp = tempfile::tempdir().unwrap();
            let store = PlaintextCredentialFileStore::new(tmp.path().join("credentials.json"));
            let key = CredentialStoreKey::from_base_url("http://unicode");

            store.store_credential_for_server_key(&key, &token).unwrap();
            prop_assert_eq!(store.load_credential_for_server_key(&key).unwrap(), Some(token));
        }

        #[test]
        fn malformed_file_content_is_never_replaced(payload in any::<String>()) {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("credentials.json");
            let original = format!("not-json:{payload}");
            std::fs::write(&path, original.as_bytes()).unwrap();
            let store = PlaintextCredentialFileStore::new(path.clone());

            prop_assert!(store.store_credential_for_server_key(&CredentialStoreKey::from_base_url("http://a"), "new-token").is_err());
            prop_assert_eq!(std::fs::read(path).unwrap(), original.into_bytes());
        }
    }

    #[test]
    fn concurrent_stores_preserve_both_credentials() {
        use std::sync::{Arc, Barrier};

        for _ in 0..16 {
            let (store, _tmp) = tmp_store();
            let barrier = Arc::new(Barrier::new(3));
            let left = Arc::new(store);
            let left_for_thread = Arc::clone(&left);
            let right = Arc::clone(&left);
            let left_barrier = Arc::clone(&barrier);
            let right_barrier = Arc::clone(&barrier);
            let left_thread = std::thread::spawn(move || {
                left_barrier.wait();
                left_for_thread.store_credential_for_server_key(
                    &CredentialStoreKey::from_base_url("http://left"),
                    "left-token",
                )
            });
            let right_thread = std::thread::spawn(move || {
                right_barrier.wait();
                right.store_credential_for_server_key(
                    &CredentialStoreKey::from_base_url("http://right"),
                    "right-token",
                )
            });
            barrier.wait();
            left_thread.join().unwrap().unwrap();
            right_thread.join().unwrap().unwrap();

            assert_eq!(
                left.load_credential_for_server_key(&CredentialStoreKey::from_base_url(
                    "http://left"
                ))
                .unwrap()
                .as_deref(),
                Some("left-token")
            );
            assert_eq!(
                left.load_credential_for_server_key(&CredentialStoreKey::from_base_url(
                    "http://right"
                ))
                .unwrap()
                .as_deref(),
                Some("right-token")
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn credential_store_rejects_symlink_without_touching_target() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target.json");
        let link = tmp.path().join("credentials.json");
        std::fs::write(&target, br#"{"http://a":"old-token"}"#).unwrap();
        symlink(&target, &link).unwrap();

        let store = PlaintextCredentialFileStore::new(link.clone());
        assert!(matches!(
            store.store_credential_for_server_key(
                &CredentialStoreKey::from_base_url("http://a"),
                "new-token"
            ),
            Err(PlaintextCredentialFileError::Symlink { .. })
        ));
        assert!(matches!(
            store.load_credential_for_server_key(&CredentialStoreKey::from_base_url("http://a")),
            Err(PlaintextCredentialFileError::Symlink { .. })
        ));

        assert_eq!(std::fs::read_link(&link).unwrap(), target);
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            r#"{"http://a":"old-token"}"#
        );
    }

    #[cfg(unix)]
    #[test]
    fn credential_store_rejects_hardlink_without_touching_either_name() {
        use std::fs::hard_link;

        let tmp = tempfile::tempdir().unwrap();
        let original = tmp.path().join("original.json");
        let link = tmp.path().join("credentials.json");
        let bytes = br#"{"http://a":"old-token"}"#;
        std::fs::write(&original, bytes).unwrap();
        hard_link(&original, &link).unwrap();

        let store = PlaintextCredentialFileStore::new(link.clone());
        assert!(matches!(
            store.store_credential_for_server_key(
                &CredentialStoreKey::from_base_url("http://a"),
                "new-token"
            ),
            Err(PlaintextCredentialFileError::HardLinked { .. })
        ));
        assert!(matches!(
            store.load_credential_for_server_key(&CredentialStoreKey::from_base_url("http://a")),
            Err(PlaintextCredentialFileError::HardLinked { .. })
        ));
        assert_eq!(std::fs::read(&original).unwrap(), bytes);
        assert_eq!(std::fs::read(&link).unwrap(), bytes);
    }

    // ── Origin binding for the env token (LIF-408) ───────────────────────
    //
    // These exercise the decision as a pure function so nothing here reads or
    // writes the process environment (LIF-401: env-var tests in this repo race
    // across modules). `LIFIC_URL` in particular is read by the clap tests in
    // `cli::mod`, so no test may set it.

    #[test]
    fn origin_of_makes_default_ports_explicit() {
        assert_eq!(
            origin_of("https://h.example").unwrap(),
            "https://h.example:443"
        );
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
        assert_eq!(
            origin_of("HTTPS://LIFIC.EXAMPLE:3998/a/b?q=1#f").unwrap(),
            base
        );
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
            env_token_for(
                Some("env-tok"),
                Some("https://ci.example"),
                "https://ci.example"
            ),
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
            env_token_for(
                Some("env-tok"),
                Some("http://127.0.0.1:3998"),
                "http://127.0.0.1:3998/"
            ),
            EnvToken::Bound("env-tok".into())
        );
    }

    #[test]
    fn env_token_is_dropped_when_target_origin_differs() {
        // The attack: a cwd config (or --url) points somewhere else.
        assert_eq!(
            env_token_for(
                Some("env-tok"),
                Some("https://ci.example"),
                "https://hostile.example"
            ),
            EnvToken::Unbound
        );
        // Different port, same host.
        assert_eq!(
            env_token_for(
                Some("env-tok"),
                Some("http://127.0.0.1:3998"),
                "http://127.0.0.1:4000"
            ),
            EnvToken::Unbound
        );
        // Different scheme, same host: an http downgrade must not carry it.
        assert_eq!(
            env_token_for(
                Some("env-tok"),
                Some("https://ci.example"),
                "http://ci.example"
            ),
            EnvToken::Unbound
        );
        // Subdomain is a different origin.
        assert_eq!(
            env_token_for(
                Some("env-tok"),
                Some("https://ci.example"),
                "https://evil.ci.example"
            ),
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
            env_token_for(
                Some("   "),
                Some("https://ci.example"),
                "https://ci.example"
            ),
            EnvToken::Absent
        );
        // Absent, not Unbound, even with no LIFIC_URL: nothing to warn about.
        assert_eq!(
            env_token_for(None, None, "https://ci.example"),
            EnvToken::Absent
        );
    }

    // The remaining tests mutate the process env, so they serialize on the
    // crate-wide `LIFIC_TOKEN` lock (LIF-401) — the auth and doctor tests
    // read the same variable, and a module-local mutex cannot serialize
    // against them. They touch `LIFIC_TOKEN` only, never `LIFIC_URL`, which
    // the clap tests in `cli::mod` read.
    use crate::test_env::lock_lific_token_env_blocking;

    #[test]
    fn unbound_env_var_does_not_shadow_stored_credentials() {
        let _lock = lock_lific_token_env_blocking();
        // A target no plausible ambient LIFIC_URL names, so the env token is
        // unbound whatever the developer's shell exports.
        let target = "http://unbound-envtest.invalid:1";

        // SAFETY: guarded by the crate-wide LIFIC_TOKEN lock; restored below.
        unsafe { std::env::set_var(TOKEN_ENV, "env-tok") };
        let got = load(target).unwrap();
        unsafe { std::env::remove_var(TOKEN_ENV) };

        assert_ne!(
            got.as_deref(),
            Some("env-tok"),
            "an env token not bound to the target origin must never be sent there"
        );
    }

    #[test]
    fn empty_env_var_is_ignored() {
        let _lock = lock_lific_token_env_blocking();
        unsafe { std::env::set_var(TOKEN_ENV, "   ") };
        // An all-whitespace env var must not shadow real backends.
        let got_source = load_with_source("http://noenv-empty").unwrap();
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
