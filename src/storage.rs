//! LIF-262: content-addressed file storage for attachments.
//!
//! Raw bytes never touch SQLite. Each uploaded blob is written to
//! `<data_dir>/attachments/<sha256>`, where `<data_dir>` is the directory
//! containing the database file (see [`AttachmentStore::from_db_path`]). The
//! file name IS the content hash, so identical bytes uploaded twice collapse
//! onto one file — deduplication is a property of the layout, not extra code.
//!
//! The DB's `attachments` table holds the metadata (filename, mime, uploader,
//! size) and points at a blob via its `sha256`. Because the store is
//! content-addressed, writes are idempotent: re-writing an already-present
//! hash is a cheap no-op, and a delete only removes the sidecar file once the
//! caller has confirmed no `attachments` row still references that hash (the
//! orphan GC's job — see `db::queries::attachments`).

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fs2::FileExt;
use sha2::{Digest, Sha256};

use crate::error::LificError;

/// Cross-process advisory lock file for the store.
///
/// It lives *beside* `attachments/`, in the stable data directory, not inside
/// it. The attachments directory is not stable: a restore renames the whole
/// thing aside and moves a new one into place, and a lock file inside it would
/// go with it. Two processes locking "the store" would then be holding
/// descriptors on two different inodes across that swap, which is no lock at
/// all at exactly the moment one is needed. The data directory outlives every
/// such swap, so the lock identity does too.
///
/// The leading dot also keeps it out of the content-addressed namespace, so
/// nothing that enumerates blobs can mistake it for one.
pub(crate) const STORE_LOCK_FILE: &str = ".lific-attachments.lock";

fn lock_is_busy(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        // LockFileEx reports ERROR_LOCK_VIOLATION rather than mapping it to
        // WouldBlock in std::io on Windows.
        error.raw_os_error() == Some(33)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Handle to the on-disk attachments directory. Cheap to clone (just a
/// `PathBuf`); threaded through the API layer as an axum `Extension` the same
/// way `AuthConfig` is.
#[derive(Debug, Clone)]
pub struct AttachmentStore {
    dir: PathBuf,
    /// Where the cross-process lock lives. Held separately from `dir` because
    /// it must survive `dir` being replaced wholesale by a restore.
    lock_path: PathBuf,
    operation_lock: Arc<Mutex<()>>,
}

fn existing_regular_file(path: &Path, label: &str) -> Result<bool, LificError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(LificError::Internal(format!(
                "inspect {label} path: {error}"
            )));
        }
    };
    if !metadata.file_type().is_file() {
        return Err(LificError::Internal(format!(
            "{label} path is not a regular file"
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(LificError::Internal(format!("{label} path is hard-linked")));
        }
    }
    Ok(true)
}

/// Flush the directory entry a rename just created.
///
/// `sync_all` on the file itself only guarantees the *bytes* survive a crash;
/// the name that makes them findable lives in the parent directory and needs
/// its own fsync. Without this, a crash right after an upload can leave a
/// database row pointing at a blob whose directory entry never landed.
/// Unix only: Windows has no directory handle to sync.
#[cfg_attr(
    not(unix),
    expect(clippy::unnecessary_wraps, reason = "fallible on Unix")
)]
fn sync_dir(_dir: &Path) -> Result<(), LificError> {
    #[cfg(unix)]
    {
        std::fs::File::open(_dir)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| LificError::Internal(format!("sync directory: {e}")))?;
    }
    Ok(())
}

pub(crate) fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

impl AttachmentStore {
    /// Build a store rooted at `<parent-of-db>/attachments`. Mirrors
    /// `Config::backup_dir`'s "resolve relative to the database file" rule so
    /// the whole data set (db + backups + attachments) sits together and the
    /// backup task can include it.
    pub fn from_db_path(db_path: &Path) -> Self {
        let data_dir = match db_path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("."),
        };
        // The lock is a sibling of the attachments directory, in the data dir,
        // which a restore never replaces. See [`STORE_LOCK_FILE`].
        Self {
            dir: data_dir.join("attachments"),
            lock_path: data_dir.join(STORE_LOCK_FILE),
            operation_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Construct a store at an explicit directory. Production always resolves
    /// the directory from the database path via [`Self::from_db_path`]; only
    /// tests point a store at a tempdir, hence the test-scoped allow.
    ///
    /// The lock goes *inside* `dir` here, deterministically, so two stores
    /// built this way from the same path still coordinate, and so a test's
    /// tempdir owns the file and removes it with everything else. Tests point
    /// this at a scratch directory rather than a real data dir, so there is no
    /// restore to survive; production gets the stable sibling path via
    /// [`Self::from_db_path`].
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new(dir: PathBuf) -> Self {
        Self {
            lock_path: dir.join(STORE_LOCK_FILE),
            dir,
            operation_lock: Arc::new(Mutex::new(())),
        }
    }

    /// The attachments directory itself. Callers reach the files through
    /// `read`/`write`/`delete`, so only tests need the raw path today.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Absolute path to the sidecar file for a given content hash. Kept
    /// private-ish (only `pub(crate)`) so callers go through
    /// `read`/`write`/`delete` rather than hand-building paths.
    pub(crate) fn path_for(&self, sha256: &str) -> Result<PathBuf, LificError> {
        if !valid_sha256(sha256) {
            return Err(LificError::BadRequest(
                "attachment hash must be 64 lowercase hexadecimal characters".into(),
            ));
        }
        Ok(self.dir.join(sha256))
    }

    /// Absolute path to the cached thumbnail for a content hash. Thumbnails
    /// live in a `thumbs/` subdirectory so a plain `read_dir` of the store
    /// still enumerates exactly the original blobs, and so a hash can never
    /// collide with its own derivative.
    pub(crate) fn thumb_path_for(&self, sha256: &str) -> Result<PathBuf, LificError> {
        self.path_for(sha256)?;
        Ok(self.dir.join("thumbs").join(format!("{sha256}.webp")))
    }

    /// Cache a generated thumbnail. Same temp-file-then-rename dance as
    /// [`Self::write`], so a concurrent reader never sees a partial webp.
    pub fn write_thumb(&self, sha256: &str, bytes: &[u8]) -> Result<(), LificError> {
        let path = self.thumb_path_for(sha256)?;
        let parent = path
            .parent()
            .ok_or_else(|| LificError::Internal("thumbnail path has no parent".into()))?;
        std::fs::create_dir_all(parent)
            .map_err(|e| LificError::Internal(format!("create thumbnails dir: {e}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| LificError::Internal(format!("secure thumbnails dir: {e}")))?;
        }
        let tmp = parent.join(format!(".{}.{}.tmp", sha256, rand::random::<u64>()));
        if existing_regular_file(&path, "thumbnail")? {
            return Ok(());
        }
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options
            .open(&tmp)
            .map_err(|e| LificError::Internal(format!("create thumbnail: {e}")))?;
        let result = (|| -> std::io::Result<()> {
            std::io::Write::write_all(&mut file, bytes)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&tmp, &path)
        })();
        if let Err(error) = result {
            let _ = std::fs::remove_file(&tmp);
            return Err(LificError::Internal(format!("write thumbnail: {error}")));
        }
        sync_dir(parent)?;
        Ok(())
    }

    /// Read a cached thumbnail. `Ok(None)` when none has been generated yet,
    /// which is the ordinary state for an attachment uploaded before
    /// thumbnails existed.
    pub fn read_thumb(&self, sha256: &str) -> Result<Option<Vec<u8>>, LificError> {
        match std::fs::read(self.thumb_path_for(sha256)?) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(LificError::Internal(format!("read thumbnail: {e}"))),
        }
    }

    /// Drop a cached thumbnail. Missing file is success: thumbnails are a
    /// cache, so every delete here is best-effort and repeatable.
    pub fn delete_thumb(&self, sha256: &str) -> Result<(), LificError> {
        match std::fs::remove_file(self.thumb_path_for(sha256)?) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(LificError::Internal(format!("delete thumbnail: {e}"))),
        }
    }

    /// Whether a usable blob is already present for this hash.
    ///
    /// Uses exactly the definition [`Self::write_unlocked`] uses, so a caller
    /// probing "is it already there?" and the writer that acts on the answer
    /// cannot disagree: a path that exists but is a symlink, a directory or a
    /// hard link is an error here rather than a `false` that would later read
    /// as "this upload created it".
    pub(crate) fn blob_exists(&self, sha256: &str) -> Result<bool, LificError> {
        existing_regular_file(&self.path_for(sha256)?, "attachment")
    }

    /// [`Self::blob_exists`] for the cached thumbnail.
    pub(crate) fn thumb_exists(&self, sha256: &str) -> Result<bool, LificError> {
        existing_regular_file(&self.thumb_path_for(sha256)?, "thumbnail")
    }

    /// Compute the lowercase hex SHA-256 of a byte slice — the content address.
    pub fn hash_bytes(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        crate::auth::hex_encode(&digest)
    }

    /// Path of the store's cross-process lock file.
    pub(crate) fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    /// Open the lock file, creating its parent directory if needed.
    ///
    /// `O_NOFOLLOW` and mode 0600: the lock file is opened for writing, so a
    /// symlink planted at that name would otherwise let another user pick the
    /// file this process opens.
    fn open_lock_file(&self) -> std::io::Result<File> {
        if let Some(parent) = self.lock_path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        options.open(self.lock_path())
    }

    /// Take the store's advisory exclusive lock, blocking until it is free.
    ///
    /// The returned handle owns the lock: dropping it (including while a panic
    /// unwinds) closes the descriptor, which releases the lock, and a process
    /// that dies mid-operation releases it too.
    fn acquire_file_lock(&self) -> std::io::Result<File> {
        let file = self.open_lock_file()?;
        FileExt::lock_exclusive(&file)?;
        Ok(file)
    }

    /// Whether some other open handle currently holds the store lock.
    ///
    /// A probe, not an acquisition: it takes and immediately releases the lock
    /// when it is free. Used by tests to observe serialization without risking
    /// a hang, and safe to call from any thread or process.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn lock_is_held(&self) -> bool {
        let Ok(file) = self.open_lock_file() else {
            return true;
        };
        match file.try_lock_exclusive() {
            Ok(()) => {
                let _ = FileExt::unlock(&file);
                false
            }
            Err(_) => true,
        }
    }

    /// Serialize filesystem operations with the metadata changes that callers
    /// perform under [`Self::with_lock`].
    ///
    /// Two locks, both needed. The in-process mutex is the cheap one and
    /// cloned stores share it. The advisory file lock is what makes this
    /// correct for *independently constructed* stores and for a second Lific
    /// process (a `lific dump`, a GC sweep, an editor's MCP server) pointed at
    /// the same data directory: without it, one process's failed-upload
    /// cleanup could delete a blob another process had just written and
    /// committed a row for.
    ///
    /// Lock ordering is store then database, everywhere. Callers open their DB
    /// connection or transaction *inside* the closure; taking a write
    /// connection first and the store lock second would invert the order and
    /// can deadlock.
    pub(crate) fn with_lock<T>(
        &self,
        operation: impl FnOnce(&Self) -> Result<T, LificError>,
    ) -> Result<T, LificError> {
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|_| LificError::Internal("attachment store lock poisoned".into()))?;
        let _file_lock = self
            .acquire_file_lock()
            .map_err(|e| LificError::Internal(format!("lock attachment store: {e}")))?;
        operation(self)
    }

    /// [`Self::with_lock`] that gives up instead of waiting.
    ///
    /// The lock can be held for as long as an operation takes, and the longest
    /// one is a dump of the whole data set. A REST handler runs on a Tokio
    /// worker thread, so blocking it for the length of a multi-gigabyte dump
    /// does not just delay that upload: it takes a worker out of the pool and
    /// stalls unrelated requests behind it. HTTP has a better answer than a
    /// stalled connection, so the request handlers use this and return
    /// `503 Service Unavailable` with a `Retry-After`, which a client can act
    /// on.
    ///
    /// `Ok(None)` means the store is busy. Errors are real failures.
    /// Everything not on a request path (CLI, MCP, background sweeps) uses the
    /// blocking [`Self::with_lock`], because there is nothing to be gained by
    /// failing those and something to lose.
    pub(crate) fn try_with_lock<T>(
        &self,
        operation: impl FnOnce(&Self) -> Result<T, LificError>,
    ) -> Result<Option<T>, LificError> {
        let _guard = match self.operation_lock.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::WouldBlock) => return Ok(None),
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(LificError::Internal(
                    "attachment store lock poisoned".into(),
                ));
            }
        };
        let file = self
            .open_lock_file()
            .map_err(|e| LificError::Internal(format!("lock attachment store: {e}")))?;
        match file.try_lock_exclusive() {
            Ok(()) => {}
            Err(error) if lock_is_busy(&error) => return Ok(None),
            Err(error) => {
                return Err(LificError::Internal(format!(
                    "lock attachment store: {error}"
                )));
            }
        }
        // `file` owns the lock for the rest of this scope; dropping it, even
        // while a panic unwinds, releases it.
        let result = operation(self);
        let _ = FileExt::unlock(&file);
        result.map(Some)
    }

    /// String-error counterpart to [`Self::try_with_lock`] for MCP tools.
    pub(crate) fn try_with_string_lock<T>(
        &self,
        operation: impl FnOnce(&Self) -> Result<T, String>,
    ) -> Result<Option<T>, String> {
        let _guard = match self.operation_lock.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::WouldBlock) => return Ok(None),
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err("attachment store lock poisoned".to_string());
            }
        };
        let file = self
            .open_lock_file()
            .map_err(|error| format!("lock attachment store: {error}"))?;
        match file.try_lock_exclusive() {
            Ok(()) => {}
            Err(error) if lock_is_busy(&error) => return Ok(None),
            Err(error) => return Err(format!("lock attachment store: {error}")),
        }
        let result = operation(self);
        let _ = FileExt::unlock(&file);
        result.map(Some)
    }

    /// The error a request handler returns when [`Self::try_with_lock`] finds
    /// the store busy. Retryable by construction: the caller did nothing
    /// wrong and the same request will work once the dump or restore holding
    /// the store finishes.
    pub(crate) fn busy_error() -> LificError {
        LificError::Unavailable(
            "attachment storage is busy (a backup or restore is running); retry shortly".into(),
        )
    }

    /// Write `bytes` to `<dir>/<sha256>`, creating the directory if needed.
    /// Idempotent: if the file already exists (same content), this is a no-op
    /// rather than a rewrite. Returns the content hash so the caller can store
    /// it on the metadata row.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn write(&self, bytes: &[u8]) -> Result<String, LificError> {
        self.with_lock(|store| store.write_unlocked(bytes))
    }

    pub(crate) fn write_unlocked(&self, bytes: &[u8]) -> Result<String, LificError> {
        let sha = Self::hash_bytes(bytes);
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| LificError::Internal(format!("create attachments dir: {e}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| LificError::Internal(format!("secure attachments dir: {e}")))?;
        }
        let path = self.path_for(&sha)?;
        if existing_regular_file(&path, "attachment")? {
            return Ok(sha);
        }
        // Write to a temp file then rename, so a concurrent reader never sees a
        // half-written blob at the final content-addressed path.
        let tmp = self
            .dir
            .join(format!(".{sha}.{:016x}.tmp", rand::random::<u64>()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options
            .open(&tmp)
            .map_err(|e| LificError::Internal(format!("create attachment temp file: {e}")))?;
        if let Err(error) = std::io::Write::write_all(&mut file, bytes) {
            let _ = std::fs::remove_file(&tmp);
            return Err(LificError::Internal(format!("write attachment: {error}")));
        }
        if let Err(error) = file.sync_all() {
            let _ = std::fs::remove_file(&tmp);
            return Err(LificError::Internal(format!("sync attachment: {error}")));
        }
        drop(file);
        if let Err(error) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(LificError::Internal(format!(
                "finalize attachment: {error}"
            )));
        }
        sync_dir(&self.dir)?;
        Ok(sha)
    }

    /// Read the bytes for a content hash. `NotFound` when the sidecar file is
    /// missing (e.g. the DB row survived but the blob was manually removed).
    pub fn read(&self, sha256: &str) -> Result<Vec<u8>, LificError> {
        let path = self.path_for(sha256)?;
        std::fs::read(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                LificError::NotFound("attachment bytes not found on disk".into())
            } else {
                LificError::Internal(format!("read attachment: {e}"))
            }
        })
    }

    /// Open a regular blob without following links. Unix reads are anchored to
    /// an open store directory, so replacing its path cannot redirect the read.
    pub(crate) fn open_blob(&self, sha256: &str) -> Result<(std::fs::File, u64), LificError> {
        let path = self.path_for(sha256)?;
        #[cfg(unix)]
        let file = {
            use std::os::fd::{AsRawFd, FromRawFd};
            use std::os::unix::fs::OpenOptionsExt;
            let dir = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
                .open(&self.dir)
                .map_err(|e| LificError::Internal(format!("open attachment directory: {e}")))?;
            let name = std::ffi::CString::new(sha256).expect("validated hash");
            // `dir` stays open across openat; `name` is a validated single hash.
            let fd = unsafe {
                libc::openat(
                    dir.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                // openat returned a new descriptor whose ownership moves here.
                Ok(unsafe { std::fs::File::from_raw_fd(fd) })
            }
        };
        #[cfg(not(unix))]
        let file = {
            let mut options = std::fs::OpenOptions::new();
            options.read(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
                // Reject junctions as well as symlinks in the store path.
                for parent in self.dir.ancestors().filter(|p| !p.as_os_str().is_empty()) {
                    let metadata = std::fs::symlink_metadata(parent).map_err(|e| {
                        LificError::Internal(format!("inspect attachment directory: {e}"))
                    })?;
                    if metadata.file_attributes() & 0x400 != 0 {
                        return Err(LificError::Internal(
                            "attachment directory is a reparse point".into(),
                        ));
                    }
                }
                options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
            }
            options.open(&path)
        };
        #[cfg(unix)]
        let _ = path;
        let file = file.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                LificError::NotFound("attachment bytes not found on disk".into())
            } else {
                LificError::Internal(format!("open attachment: {e}"))
            }
        })?;
        let meta = file
            .metadata()
            .map_err(|e| LificError::Internal(format!("stat attachment: {e}")))?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & 0x400 != 0 {
                return Err(LificError::NotFound(
                    "attachment bytes not found on disk".into(),
                ));
            }
        }
        if !meta.is_file() {
            return Err(LificError::NotFound(
                "attachment bytes not found on disk".into(),
            ));
        }
        Ok((file, meta.len()))
    }

    /// Delete the sidecar file for a content hash. Missing file is treated as
    /// success (idempotent) — the GC only calls this once no DB row references
    /// the hash, so a double-delete or a manual prior removal is fine.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn delete(&self, sha256: &str) -> Result<(), LificError> {
        self.with_lock(|store| store.delete_unlocked(sha256))
    }

    pub(crate) fn delete_unlocked(&self, sha256: &str) -> Result<(), LificError> {
        // The thumbnail is derived from these exact bytes, so it dies with
        // them. Best-effort: a failure to remove a cache file must not block
        // the blob delete the caller actually asked for.
        let _ = self.delete_thumb(sha256);
        let path = self.path_for(sha256)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(LificError::Internal(format!("delete attachment: {e}"))),
        }
    }
}

// ── Orphan GC sweep ──────────────────────────────────────────

/// Grace window before an unlinked attachment is collectable: an upload that's
/// been sitting linkless for longer than this is assumed abandoned (the
/// compose that created it never saved). 24h is generous — a draft can live a
/// long time before it's posted.
pub const ORPHAN_GRACE_SECONDS: i64 = 24 * 60 * 60;

/// Delete abandoned attachments: rows with zero links older than the grace
/// window, plus their sidecar blobs when no surviving row shares the content
/// hash. Returns the number of rows collected. Safe to call repeatedly (a
/// background task drives it on an interval — see `start_gc_task`).
pub fn sweep_orphans(
    pool: &crate::db::DbPool,
    store: &AttachmentStore,
    grace_seconds: i64,
) -> Result<usize, LificError> {
    use crate::db::queries::attachments as q;

    let orphans = {
        let conn = pool.read()?;
        q::find_orphans(&conn, grace_seconds)?
    };
    let mut collected = 0;
    for orphan in orphans {
        let removed = store.with_lock(|store| {
            let conn = pool.write()?;
            let Some(sha256) = q::delete_orphan_attachment(&conn, orphan.id)? else {
                return Ok(false);
            };
            if q::count_rows_for_sha(&conn, &sha256)? == 0 {
                store.delete_unlocked(&sha256)?;
            }
            Ok(true)
        })?;
        if removed {
            collected += 1;
        }
    }
    Ok(collected)
}

/// LIF-418: index the contents of text attachments that aren't in
/// `attachments_fts` yet.
///
/// Two populations need this: uploads that predate migration 042 (the
/// migration can seed filenames from SQLite, but the bytes live on disk where
/// SQL can't reach them), and any upload whose extraction was interrupted.
/// Idempotent by construction — the driving query only returns rows with an
/// empty `extracted_text`, so a second run finds nothing and does nothing.
///
/// A blob missing from disk or holding invalid UTF-8 is skipped rather than
/// failing the pass; one bad file must not stop the rest from being indexed.
pub fn backfill_attachment_text(
    pool: &crate::db::DbPool,
    store: &AttachmentStore,
) -> Result<usize, LificError> {
    use crate::db::queries::attachments as q;

    let pending = {
        let conn = pool.read()?;
        q::unindexed_text_attachments(&conn)?
    };

    let mut indexed = 0;
    for (id, sha256) in pending {
        let Ok(bytes) = store.read(&sha256) else {
            continue;
        };
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        // An empty file has nothing to index; skipping it also keeps it out of
        // the "still empty" retry set forever, which is correct — there is
        // nothing to find in it.
        if text.is_empty() {
            continue;
        }
        {
            let conn = pool.write()?;
            q::set_extracted_text(&conn, id, &text)?;
        }
        indexed += 1;
    }
    Ok(indexed)
}

/// Spawn a background task that sweeps orphaned attachments hourly. Mirrors the
/// backup task's shape (initial delay then a fixed interval).
///
/// LIF-418: this is also where the attachment text backfill runs, once, before
/// the sweep loop starts. Same task on purpose: both are "reconcile the
/// attachment store with the database" chores that must not block startup.
pub fn start_gc_task(
    pool: crate::db::DbPool,
    store: AttachmentStore,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Both jobs below are synchronous file + SQLite work, and the sweep
        // takes the attachment store lock, where it can wait behind a dump of
        // the whole data set. Running either directly here would park a Tokio
        // worker thread — one of the threads serving HTTP — for that whole
        // time, so both go to the blocking pool and are awaited. Awaiting is
        // what keeps this to one blocking job at a time: a sweep that outruns
        // its hour delays the next tick instead of queueing another job.
        run_blocking("attachment text backfill", {
            let pool = pool.clone();
            let store = store.clone();
            move || match backfill_attachment_text(&pool, &store) {
                Ok(n) if n > 0 => {
                    tracing::info!(indexed = n, "attachment text backfill indexed files")
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "attachment text backfill failed"),
            }
        })
        .await;

        // Let the server settle before the first sweep.
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60 * 60));
        interval.tick().await; // skip the immediate tick
        loop {
            interval.tick().await;
            run_blocking("attachment GC sweep", {
                let pool = pool.clone();
                let store = store.clone();
                move || match sweep_orphans(&pool, &store, ORPHAN_GRACE_SECONDS) {
                    Ok(n) if n > 0 => {
                        tracing::info!(collected = n, "attachment GC swept orphans")
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "attachment GC sweep failed"),
                }
            })
            .await;
        }
    })
}

/// Run one synchronous chore on the blocking pool and wait for it, logging a
/// panic rather than letting it take the scheduling loop down with it.
async fn run_blocking(label: &'static str, job: impl FnOnce() + Send + 'static) {
    if let Err(e) = tokio::task::spawn_blocking(job).await {
        tracing::error!(job = label, error = %e, "background job failed to run to completion");
    }
}

// ── MIME sniffing + allowlist ────────────────────────────────
//
// Never trust the client-supplied content-type: a browser (or a malicious
// uploader) can claim `image/png` for an HTML file and get it served back
// inline. We sniff the leading magic bytes and cross-check against the
// allowlist, falling back to the declared type only for the formats that have
// no reliable signature (plain text / log).

/// The upload MIME allowlist: images + a few safe document/archive types.
/// Executables and everything else are rejected. Returned type is the
/// canonical MIME we store and serve (so a `image/jpg` claim normalizes to
/// `image/jpeg`, etc.).
pub const ALLOWED_MIMES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/svg+xml",
    "application/pdf",
    "text/plain",
    "application/zip",
    // LIF-418: media. Every one of these is magic-byte validated below, and
    // none of them is a script-execution vector in a browser, so they serve
    // inline (see `is_inline_safe_mime`) behind the same CSP sandbox as
    // everything else.
    "video/mp4",
    "video/webm",
    "audio/webm",
    "audio/ogg",
    "audio/mpeg",
    // LIF-418: SQLite databases, so `GET /api/attachments/{id}/preview` can
    // list their tables. Always served as a download, never inline.
    "application/vnd.sqlite3",
];

/// The raster image formats we can decode dimensions for and thumbnail. SVG is
/// excluded: it is a vector document with no intrinsic pixel size, and we do
/// not run an XML parser over untrusted uploads to guess one.
pub const RASTER_MIMES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

/// Whether a canonical MIME is a raster image we decode.
pub fn is_raster_mime(mime: &str) -> bool {
    RASTER_MIMES.contains(&mime)
}

/// Whether a canonical MIME is safe to hand a browser as a *top-level
/// document* (`Content-Disposition: inline`).
///
/// This is deliberately an allowlist of raster formats rather than
/// `is_image_mime`. SVG is an image by MIME, but it is also an XML document
/// that can carry `<script>`, and browsers execute that script when the SVG
/// is navigated to directly (or framed) from the app's own origin. Serving
/// one inline is stored XSS: the script runs on the Lific origin and can read
/// the session bearer token the SPA keeps in `localStorage`, escalating any
/// upload-capable account to whoever views the file.
///
/// Rendering is unaffected. `Content-Disposition` is ignored for subresource
/// loads, so `<img src="/api/attachments/1">` still displays an SVG, and
/// scripts never run in that context.
/// LIF-418 extends the list to audio and video. An `<video>` or `<audio>`
/// element needs a real navigable URL with byte-range support, and a media
/// container is not a document: browsers hand mp4/webm/ogg/mp3 to the media
/// pipeline, which has no scripting surface. The CSP sandbox and `nosniff`
/// still ride along on every response, so a container that turned out to be
/// something else could still not execute.
pub fn is_inline_safe_mime(mime: &str) -> bool {
    matches!(
        mime,
        "image/png"
            | "image/jpeg"
            | "image/gif"
            | "image/webp"
            | "video/mp4"
            | "video/webm"
            | "audio/webm"
            | "audio/ogg"
            | "audio/mpeg"
    )
}

/// Sniff the real content type from magic bytes, cross-checked against the
/// declared type and the allowlist. Returns the canonical MIME to store, or an
/// error describing why the upload was rejected.
///
/// The declared type is only honored for signature-less formats (plain text),
/// and even then only when it's on the allowlist. Everything with a real
/// signature (images, pdf, zip) is decided purely by the bytes — a lie in the
/// header can't smuggle an executable past this.
pub fn sniff_and_validate(bytes: &[u8], declared: Option<&str>) -> Result<String, LificError> {
    match classify_prefix(prefix_of(bytes), declared) {
        PrefixVerdict::Decided(mime) => Ok(mime),
        PrefixVerdict::Rejected(error) => Err(error),
        // Text is the only verdict the head cannot settle: serving arbitrary
        // binary as `text/plain` is the thing to avoid, so every byte has to
        // be valid UTF-8, not just the ones we sniffed.
        PrefixVerdict::TextIfUtf8 => {
            if std::str::from_utf8(bytes).is_ok() {
                Ok("text/plain".to_string())
            } else {
                Err(unrecognized_type())
            }
        }
    }
}

/// Number of leading bytes every content-type decision is made from.
///
/// Each check below reads only the head: magic signatures are at most 16 bytes
/// in, the executable markers are 4, and the SVG/XML probe looks at 512. 4 KiB
/// is comfortably past all of them and is the entire buffer
/// [`sniff_and_validate_stream`] ever holds.
pub(crate) const SNIFF_PREFIX_BYTES: usize = 4096;

fn prefix_of(bytes: &[u8]) -> &[u8] {
    &bytes[..bytes.len().min(SNIFF_PREFIX_BYTES)]
}

fn unrecognized_type() -> LificError {
    LificError::BadRequest("rejected: unsupported or unrecognized file type".into())
}

/// What the leading bytes alone can say about a file's type.
enum PrefixVerdict {
    /// Settled by signature; the rest of the file cannot change it.
    Decided(String),
    Rejected(LificError),
    /// `text/plain`, but only if the *whole* file is valid UTF-8.
    TextIfUtf8,
}

/// The shared content-type decision, made from a bounded prefix.
///
/// [`sniff_and_validate`] and [`sniff_and_validate_stream`] both route through
/// this, so an in-memory upload and a streamed restore cannot drift into
/// disagreeing about what a file is.
fn classify_prefix(prefix: &[u8], declared: Option<&str>) -> PrefixVerdict {
    let declared = declared.map(|d| d.split(';').next().unwrap_or(d).trim().to_ascii_lowercase());

    // Signature-based detection first (authoritative).
    if let Some(mime) = sniff_magic(prefix) {
        // One container, two canonical types: a WebM/Matroska file with no
        // video track is `audio/webm`, and telling them apart needs a full
        // track parse. The bytes still decide that this IS a WebM container;
        // the declared type only picks which of the two allowlisted labels we
        // record, and both are inline-safe media, so a lie here buys nothing.
        if mime == "video/webm" && declared.as_deref() == Some("audio/webm") {
            return PrefixVerdict::Decided("audio/webm".to_string());
        }
        return PrefixVerdict::Decided(mime.to_string());
    }

    // No recognizable binary signature. Reject anything that structurally
    // looks like an executable or script, regardless of the declared type.
    if looks_executable(prefix) {
        return PrefixVerdict::Rejected(LificError::BadRequest(
            "rejected: file looks like an executable".into(),
        ));
    }

    // SVG is XML-based (text signature): accept when it declares an svg/xml
    // type and the content opens like SVG/XML.
    if declared.as_deref() == Some("image/svg+xml") && looks_like_svg(prefix) {
        return PrefixVerdict::Decided("image/svg+xml".to_string());
    }

    // An empty file is not text, it is nothing.
    if prefix.is_empty() {
        return PrefixVerdict::Rejected(unrecognized_type());
    }

    // Everything left is text-if-it-parses. That covers both the declared
    // `text/plain`/`text/x-log` case and the last-resort fallback for a file
    // uploaded with no or an incorrect content-type; both require valid UTF-8,
    // so they collapse into one verdict.
    PrefixVerdict::TextIfUtf8
}

/// [`sniff_and_validate`] over a reader, holding a bounded buffer instead of
/// the whole file.
///
/// Same allowlist, same signature checks, same "text must be valid UTF-8 end
/// to end" rule. The difference is only in memory: the type is decided from a
/// [`SNIFF_PREFIX_BYTES`] prefix, and the UTF-8 requirement is checked by
/// streaming the rest through an incremental validator. Restoring an archive
/// with `--allow-large` must not allocate an attachment-sized `Vec` per blob
/// just to confirm its content type.
pub fn sniff_and_validate_stream<R: Read>(
    mut reader: R,
    declared: Option<&str>,
) -> Result<String, LificError> {
    let mut prefix = vec![0u8; SNIFF_PREFIX_BYTES];
    let filled = read_fully(&mut reader, &mut prefix)
        .map_err(|e| LificError::BadRequest(format!("read attachment: {e}")))?;
    prefix.truncate(filled);

    match classify_prefix(&prefix, declared) {
        PrefixVerdict::Decided(mime) => Ok(mime),
        PrefixVerdict::Rejected(error) => Err(error),
        PrefixVerdict::TextIfUtf8 => {
            let valid = stream_is_utf8(&prefix, reader)
                .map_err(|e| LificError::BadRequest(format!("read attachment: {e}")))?;
            if valid {
                Ok("text/plain".to_string())
            } else {
                Err(unrecognized_type())
            }
        }
    }
}

/// Read until `buf` is full or the reader ends, returning the bytes filled.
/// A short `read` is not EOF, and a prefix sniffed from one would misjudge a
/// file whose signature straddles the boundary.
fn read_fully<R: Read>(reader: &mut R, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

/// Chunk size for the streaming UTF-8 pass. With the prefix buffer this is the
/// entire footprint of validating a blob of any size.
const UTF8_STREAM_CHUNK: usize = 64 * 1024;

/// Whether `prefix` followed by everything left in `reader` is valid UTF-8.
///
/// A multi-byte character can straddle a chunk boundary, so the incomplete
/// tail of each chunk (never more than 3 bytes) carries into the next one. A
/// carry still pending at EOF means the file ends mid-character, which is not
/// valid UTF-8.
fn stream_is_utf8<R: Read>(prefix: &[u8], mut reader: R) -> std::io::Result<bool> {
    let mut carry: Vec<u8> = Vec::with_capacity(4);
    if !feed_utf8(&mut carry, prefix) {
        return Ok(false);
    }
    let mut buf = vec![0u8; UTF8_STREAM_CHUNK];
    loop {
        let read = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if !feed_utf8(&mut carry, &buf[..read]) {
            return Ok(false);
        }
    }
    Ok(carry.is_empty())
}

/// Validate one chunk, given the incomplete tail of the previous one. Returns
/// false on a definitively invalid sequence; otherwise leaves any incomplete
/// trailing sequence in `carry`.
fn feed_utf8(carry: &mut Vec<u8>, chunk: &[u8]) -> bool {
    let joined: Vec<u8>;
    let bytes: &[u8] = if carry.is_empty() {
        chunk
    } else {
        joined = carry.iter().copied().chain(chunk.iter().copied()).collect();
        &joined
    };
    match std::str::from_utf8(bytes) {
        Ok(_) => {
            carry.clear();
            true
        }
        // `error_len() == None` means "ran out of bytes mid-character", the
        // one error that the next chunk can still resolve.
        Err(error) if error.error_len().is_none() => {
            let tail = bytes[error.valid_up_to()..].to_vec();
            // A truncated UTF-8 sequence is at most 3 bytes; anything longer
            // is not a boundary artifact.
            if tail.len() > 3 {
                return false;
            }
            *carry = tail;
            true
        }
        Err(_) => false,
    }
}

/// Detect the canonical MIME from leading magic bytes for the binary formats
/// on the allowlist. `None` when no signature matches.
fn sniff_magic(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 8 && bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("image/png");
    }
    if bytes.len() >= 3 && bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.len() >= 6 && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.len() >= 5 && bytes.starts_with(b"%PDF-") {
        return Some("application/pdf");
    }
    // ZIP (also the container for docx/xlsx/etc, but we only advertise zip).
    if bytes.len() >= 4
        && bytes[0] == 0x50
        && bytes[1] == 0x4B
        && (bytes[2] == 0x03 || bytes[2] == 0x05 || bytes[2] == 0x07)
    {
        return Some("application/zip");
    }
    // SQLite database: a fixed 16-byte header string including its NUL.
    if bytes.starts_with(SQLITE_MAGIC) {
        return Some("application/vnd.sqlite3");
    }
    // ISO base media (mp4): a `ftyp` box at offset 4. The box's major brand
    // says which flavour; QuickTime rides the same container and is not on
    // the allowlist, so it is filtered out rather than relabelled as mp4.
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        let brand = &bytes[8..12];
        if brand != b"qt  " {
            return Some("video/mp4");
        }
        return None;
    }
    // EBML (Matroska / WebM). We only advertise WebM, so require the DocType
    // string to appear in the header region rather than accepting any EBML.
    if bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        let head = &bytes[..bytes.len().min(64)];
        if head.windows(4).any(|w| w == b"webm") {
            return Some("video/webm");
        }
        return None;
    }
    // Ogg container. Vorbis/Opus audio is all we advertise, so it is labelled
    // audio/ogg rather than the generic application/ogg.
    if bytes.starts_with(b"OggS") {
        return Some("audio/ogg");
    }
    // MP3: either an ID3v2 tag or a bare MPEG audio frame header. The frame
    // sync is eleven set bits, and the two fields right after it have
    // reserved encodings that a real frame never uses, so checking them keeps
    // arbitrary 0xFF-leading binary from being waved through as audio.
    if bytes.starts_with(b"ID3") {
        return Some("audio/mpeg");
    }
    if bytes.len() >= 2 && bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0 {
        let version = (bytes[1] >> 3) & 0b11; // 01 is reserved
        let layer = (bytes[1] >> 1) & 0b11; // 00 is reserved
        if version != 0b01 && layer != 0b00 {
            return Some("audio/mpeg");
        }
    }
    None
}

/// The 16-byte header every SQLite database file starts with, NUL included.
pub const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";

/// Heuristic executable/script sniff for the signature-less path. Blocks the
/// obvious dangerous headers so a "text/plain" claim can't smuggle a binary.
fn looks_executable(bytes: &[u8]) -> bool {
    // ELF, Mach-O (32/64, both endian), PE/DOS (MZ), Java class, shebang, WASM.
    const SIGS: &[&[u8]] = &[
        b"\x7FELF",
        &[0xFE, 0xED, 0xFA, 0xCE],
        &[0xFE, 0xED, 0xFA, 0xCF],
        &[0xCF, 0xFA, 0xED, 0xFE],
        &[0xCE, 0xFA, 0xED, 0xFE],
        b"MZ",
        &[0xCA, 0xFE, 0xBA, 0xBE],
        b"#!",
        &[0x00, 0x61, 0x73, 0x6D], // \0asm (WebAssembly)
    ];
    SIGS.iter().any(|sig| bytes.starts_with(sig))
}

// ── Raster decoding: dimensions + thumbnails (LIF-418) ───────
//
// Both entry points below run the `image` crate over bytes a stranger
// uploaded, so both go through `reader_for` and inherit its decode limits. A
// 40-byte PNG header can legitimately declare a 60000x60000 canvas, and
// decoding it would allocate ~14 GB before anything noticed; the limits turn
// that into a decode error instead of an OOM kill.

/// Long edge of a generated thumbnail, in pixels. Images at or under this on
/// both axes get no thumbnail at all: the original is already small enough to
/// send, and a second nearly-identical file would only cost disk.
pub const THUMBNAIL_MAX_EDGE: u32 = 480;

/// Largest pixel count we will decode. 50 megapixels is far beyond any
/// screenshot or photo a tracker attachment plausibly holds, and caps the
/// decoded RGBA buffer at roughly 200 MB.
const MAX_DECODE_PIXELS: u64 = 50_000_000;

/// Build a limited `ImageReader` over an in-memory buffer, with the format
/// guessed from the bytes rather than taken from any header.
fn reader_for(
    bytes: &[u8],
) -> Result<image::ImageReader<std::io::Cursor<&[u8]>>, image::ImageError> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(image::ImageError::IoError)?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(20_000);
    limits.max_image_height = Some(20_000);
    limits.max_alloc = Some(MAX_DECODE_PIXELS * 4);
    reader.limits(limits);
    Ok(reader)
}

/// Decode just the header of a raster image and return `(width, height)`.
/// `None` when the bytes are not a decodable raster, which is never fatal:
/// dimensions are an optimization, not a correctness requirement.
pub fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    reader_for(bytes).ok()?.into_dimensions().ok()
}

/// Downscale a raster image so its long edge is [`THUMBNAIL_MAX_EDGE`] and
/// encode the result as lossless WebP.
///
/// `Ok(None)` means "no thumbnail is warranted": the image already fits inside
/// the thumbnail box, so callers should serve the original. An `Err` means the
/// bytes would not decode, which callers treat as "no thumbnail" too rather
/// than failing the request that triggered generation.
pub fn generate_thumbnail(bytes: &[u8]) -> Result<Option<Vec<u8>>, LificError> {
    let reader =
        reader_for(bytes).map_err(|e| LificError::BadRequest(format!("undecodable image: {e}")))?;
    let (w, h) = reader
        .into_dimensions()
        .map_err(|e| LificError::BadRequest(format!("undecodable image: {e}")))?;
    if w.max(h) <= THUMBNAIL_MAX_EDGE {
        return Ok(None);
    }
    if u64::from(w) * u64::from(h) > MAX_DECODE_PIXELS {
        return Err(LificError::BadRequest(
            "image is too large to thumbnail".into(),
        ));
    }

    let image = reader_for(bytes)
        .and_then(|r| r.decode())
        .map_err(|e| LificError::BadRequest(format!("undecodable image: {e}")))?;
    // `thumbnail` preserves the aspect ratio and fits inside the box, so a
    // 1000x200 strip comes back 480x96 rather than stretched.
    let small = image.thumbnail(THUMBNAIL_MAX_EDGE, THUMBNAIL_MAX_EDGE);
    let rgba = small.to_rgba8();
    let (tw, th) = (rgba.width(), rgba.height());

    let mut out = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(std::io::Cursor::new(&mut out))
        .encode(rgba.as_raw(), tw, th, image::ExtendedColorType::Rgba8)
        .map_err(|e| LificError::Internal(format!("encode thumbnail: {e}")))?;
    Ok(Some(out))
}

/// Whether a thumbnail is expected to exist for an attachment, given its mime
/// and recorded dimensions. This is the value serialized as `has_thumbnail`,
/// and it deliberately describes what the thumbnail endpoint will do rather
/// than what is currently on disk: generation is lazy, so "no file yet" and
/// "no thumbnail" are different states and only the second is a 404.
pub fn expects_thumbnail(mime: &str, width: Option<i64>, height: Option<i64>) -> bool {
    if !is_raster_mime(mime) {
        return false;
    }
    let long_edge = width.unwrap_or(0).max(height.unwrap_or(0));
    long_edge > i64::from(THUMBNAIL_MAX_EDGE)
}

/// Cheap check that the head of a buffer opens like SVG/XML.
fn looks_like_svg(bytes: &[u8]) -> bool {
    let head_len = bytes.len().min(512);
    let head = String::from_utf8_lossy(&bytes[..head_len]).to_ascii_lowercase();
    head.contains("<svg") || head.trim_start().starts_with("<?xml")
}

/// Image fixtures shared with the API-level attachment tests, so both layers
/// assert against bytes a real decoder will accept rather than magic-byte
/// stubs that only the sniffer is happy with.
#[cfg(test)]
pub(crate) mod fixtures {
    /// Encode a real solid-colour PNG at the requested size.
    pub(crate) fn png_image(width: u32, height: u32) -> Vec<u8> {
        let buffer = image::RgbaImage::from_pixel(width, height, image::Rgba([9, 40, 90, 255]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(buffer)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .expect("encode png fixture");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::png_image;
    use super::*;

    #[test]
    fn streaming_blob_handle_reads_only_regular_files() {
        let (store, _tmp) = tmp_store();
        let hash = store.write(b"streamed bytes").unwrap();
        let (mut file, size) = store.open_blob(&hash).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"streamed bytes");
        assert_eq!(size, 14);
        assert!(store.open_blob("../outside").is_err());
        drop(file);
        let path = store.path_for(&hash).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(path).unwrap();
        assert!(store.open_blob(&hash).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn streaming_blob_open_rejects_links_and_fifos_without_blocking() {
        use std::os::unix::{ffi::OsStrExt, fs::symlink};
        let (store, tmp) = tmp_store();
        let hash = store.write(b"public bytes").unwrap();
        let alias = tmp.path().join("alias");
        symlink(store.dir(), &alias).unwrap();
        assert!(AttachmentStore::new(alias).open_blob(&hash).is_err());
        let outside = tmp.path().join("private");
        std::fs::write(&outside, b"private bytes").unwrap();
        let path = store.path_for(&hash).unwrap();
        std::fs::remove_file(&path).unwrap();
        symlink(outside, &path).unwrap();
        assert!(store.open_blob(&hash).is_err());
        std::fs::remove_file(&path).unwrap();
        let cpath = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // The temporary path is NUL-terminated and owned for this call.
        assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
        assert!(store.open_blob(&hash).is_err());
    }

    /// A store rooted in a fresh scratch directory. The caller must keep the
    /// returned [`TempDir`] alive for as long as it uses the store; dropping
    /// it removes the directory, which also happens while a failed assertion
    /// unwinds.
    fn tmp_store() -> (AttachmentStore, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        // A subdirectory that does not exist yet, matching production, where
        // the attachments dir is created on first write.
        let store = AttachmentStore::new(tmp.path().join("attachments"));
        (store, tmp)
    }

    // ── Cross-process store lock ─────────────────────────────

    /// How long a test waits for something that should happen. Generous: it
    /// only bounds a failure, it is not a timing assertion.
    const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(10);
    /// How long a test waits to conclude something is NOT happening.
    const LOCK_BLOCK_PROBE: std::time::Duration = std::time::Duration::from_millis(250);

    #[test]
    fn independently_constructed_stores_share_the_on_disk_lock() {
        // Two stores built separately from the same directory have *different*
        // in-process mutexes, exactly like two Lific processes. Only the file
        // lock can make them coordinate, so this is the property that matters.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("attachments");
        let holder = AttachmentStore::new(dir.clone());
        let observer = AttachmentStore::new(dir);
        assert!(
            !Arc::ptr_eq(&holder.operation_lock, &observer.operation_lock),
            "the two stores must not be sharing an in-process mutex"
        );

        assert!(!observer.lock_is_held(), "nothing holds the lock yet");

        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel::<()>(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel::<()>(1);
        let worker = std::thread::spawn(move || {
            holder.with_lock(|_| {
                entered_tx.send(()).unwrap();
                // Bounded: a lost release signal fails the test instead of
                // hanging the suite.
                let _ = release_rx.recv_timeout(LOCK_WAIT);
                Ok(())
            })
        });

        entered_rx
            .recv_timeout(LOCK_WAIT)
            .expect("the worker must acquire the lock");
        assert!(
            observer.lock_is_held(),
            "a separately constructed store must see the held lock"
        );

        release_tx.send(()).unwrap();
        worker.join().unwrap().unwrap();
        assert!(
            !observer.lock_is_held(),
            "the lock is released when the operation ends"
        );
    }

    #[test]
    fn a_second_store_waits_for_the_first_to_finish() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("attachments");
        let first = AttachmentStore::new(dir.clone());
        let second = AttachmentStore::new(dir);

        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel::<()>(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel::<()>(1);
        let holder = std::thread::spawn(move || {
            first.with_lock(|_| {
                entered_tx.send(()).unwrap();
                let _ = release_rx.recv_timeout(LOCK_WAIT);
                Ok(())
            })
        });
        entered_rx.recv_timeout(LOCK_WAIT).unwrap();

        let (acquired_tx, acquired_rx) = std::sync::mpsc::sync_channel::<()>(1);
        let waiter = std::thread::spawn(move || {
            second.with_lock(|store| {
                acquired_tx.send(()).unwrap();
                store.write_unlocked(b"second store bytes")
            })
        });

        assert!(
            acquired_rx.recv_timeout(LOCK_BLOCK_PROBE).is_err(),
            "the second store must not enter the critical section while the first holds it"
        );

        release_tx.send(()).unwrap();
        holder.join().unwrap().unwrap();
        acquired_rx
            .recv_timeout(LOCK_WAIT)
            .expect("the second store proceeds once the lock is free");
        let sha = waiter.join().unwrap().unwrap();
        assert_eq!(sha, AttachmentStore::hash_bytes(b"second store bytes"));
    }

    #[test]
    fn a_production_store_locks_beside_the_attachments_dir_not_inside_it() {
        // A restore replaces `attachments/` wholesale. A lock file inside it
        // would be replaced too, leaving two processes holding descriptors on
        // two different inodes: no lock, at the one moment it matters.
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("lific.db");
        let store = AttachmentStore::from_db_path(&db_path);

        assert_eq!(store.lock_path(), tmp.path().join(STORE_LOCK_FILE));
        assert!(
            !store.lock_path().starts_with(store.dir()),
            "the lock must not live inside the replaceable attachments dir"
        );

        store.with_lock(|_| Ok(())).unwrap();
        assert!(store.lock_path().is_file());
    }

    #[test]
    fn the_store_lock_survives_the_attachments_directory_being_replaced() {
        // Exactly what a restore does to the store: move the live directory
        // aside and rename a new one into its place. The lock must still be
        // the same lock, and must still coordinate two independent stores.
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("lific.db");
        let live = AttachmentStore::from_db_path(&db_path);
        let observer = AttachmentStore::from_db_path(&db_path);
        live.write(b"before the restore").unwrap();

        let replacement = tmp.path().join("staged-attachments");
        std::fs::create_dir_all(&replacement).unwrap();
        std::fs::remove_dir_all(live.dir()).unwrap();
        std::fs::rename(&replacement, live.dir()).unwrap();

        assert!(
            live.lock_path().is_file(),
            "the lock file is not collateral damage of the swap"
        );

        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel::<()>(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel::<()>(1);
        let worker = std::thread::spawn(move || {
            live.with_lock(|_| {
                entered_tx.send(()).unwrap();
                let _ = release_rx.recv_timeout(LOCK_WAIT);
                Ok(())
            })
        });
        entered_rx.recv_timeout(LOCK_WAIT).unwrap();
        assert!(
            observer.lock_is_held(),
            "the same lock still coordinates after the directory was replaced"
        );
        release_tx.send(()).unwrap();
        worker.join().unwrap().unwrap();
        assert!(!observer.lock_is_held());
    }

    #[test]
    fn a_test_store_keeps_its_lock_inside_the_directory_it_owns() {
        // `new` is the test constructor: its lock goes inside the directory
        // under test so the tempdir that owns that directory removes it, and
        // nothing is left beside the tempdir for a later run to trip over.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("attachments");
        let store = AttachmentStore::new(dir.clone());
        assert_eq!(store.lock_path(), dir.join(STORE_LOCK_FILE));

        store.with_lock(|_| Ok(())).unwrap();

        let siblings: Vec<String> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(
            siblings,
            vec!["attachments".to_string()],
            "a test store must not leak files beside its directory"
        );
    }

    #[test]
    fn try_with_lock_reports_a_busy_store_instead_of_waiting() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("attachments");
        let holder = AttachmentStore::new(dir.clone());
        // Independently constructed, so only the file lock connects them —
        // the same situation a request handler is in while a dump runs.
        let requester = AttachmentStore::new(dir);

        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel::<()>(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel::<()>(1);
        let worker = std::thread::spawn(move || {
            holder.with_lock(|_| {
                entered_tx.send(()).unwrap();
                let _ = release_rx.recv_timeout(LOCK_WAIT);
                Ok(())
            })
        });
        entered_rx.recv_timeout(LOCK_WAIT).unwrap();

        let busy = requester.try_with_lock(|_| Ok(())).unwrap();
        assert!(busy.is_none(), "a busy store must not block the caller");
        assert!(
            requester
                .try_with_string_lock(|_| Ok(()))
                .unwrap()
                .is_none(),
            "the MCP string-error path must not block either"
        );
        assert!(matches!(
            AttachmentStore::busy_error(),
            LificError::Unavailable(_)
        ));

        release_tx.send(()).unwrap();
        worker.join().unwrap().unwrap();
        assert_eq!(
            requester.try_with_lock(|_| Ok(7)).unwrap(),
            Some(7),
            "and must proceed once the store is free"
        );
        assert_eq!(requester.try_with_string_lock(|_| Ok(8)).unwrap(), Some(8));
    }

    #[test]
    fn try_with_lock_also_declines_when_the_in_process_mutex_is_held() {
        let (store, _tmp) = tmp_store();
        let clone = store.clone();
        store
            .with_lock(|_| {
                // Same store, so the in-process mutex is the first thing in
                // the way; it must decline rather than deadlock.
                assert!(clone.try_with_lock(|_| Ok(())).unwrap().is_none());
                Ok(())
            })
            .unwrap();
        assert!(store.try_with_lock(|_| Ok(())).unwrap().is_some());
    }

    #[cfg(windows)]
    #[test]
    fn windows_lock_violation_is_a_busy_store() {
        assert!(lock_is_busy(&std::io::Error::from_raw_os_error(33)));
    }

    #[cfg(unix)]
    #[test]
    fn the_store_lock_file_is_owner_only_and_not_a_blob() {
        use std::os::unix::fs::PermissionsExt;

        let (store, _tmp) = tmp_store();
        store.with_lock(|_| Ok(())).unwrap();

        let lock = store.lock_path();
        assert!(lock.is_file());
        let mode = std::fs::metadata(lock).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the lock file must be owner-only");
        let name = lock.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            !valid_sha256(&name),
            "the lock file must never look like a content address: {name}"
        );
    }

    #[test]
    fn write_read_roundtrip_and_dedup() {
        let (store, _tmp) = tmp_store();
        let bytes = b"hello attachment world";
        let sha1 = store.write(bytes).unwrap();
        let sha2 = store.write(bytes).unwrap();
        assert_eq!(sha1, sha2, "same content hashes to same file");
        assert_eq!(store.read(&sha1).unwrap(), bytes);
        // Only one blob on disk for the duplicate write. The store also holds
        // its cross-process lock file, which is not content-addressed data.
        let blobs: Vec<String> = std::fs::read_dir(store.dir())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .filter(|name| valid_sha256(name))
            .collect();
        assert_eq!(blobs, vec![sha1]);
    }

    #[cfg(unix)]
    #[test]
    fn attachment_storage_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let (store, _tmp) = tmp_store();
        let sha = store.write(b"private attachment").unwrap();
        let dir_mode = std::fs::metadata(store.dir()).unwrap().permissions().mode() & 0o777;
        let file_mode = std::fs::metadata(store.dir().join(sha))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn attachment_write_rejects_existing_symlink() {
        use std::os::unix::fs::symlink;

        let (store, _tmp) = tmp_store();
        let bytes = b"private attachment";
        let sha = AttachmentStore::hash_bytes(bytes);
        std::fs::create_dir_all(store.dir()).unwrap();
        let target = store.dir().join("outside");
        std::fs::write(&target, b"outside").unwrap();
        symlink(&target, store.dir().join(&sha)).unwrap();

        assert!(store.write(bytes).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn thumbnail_write_rejects_existing_symlink() {
        use std::os::unix::fs::symlink;

        let (store, _tmp) = tmp_store();
        let sha = "a".repeat(64);
        let thumbnail = store.thumb_path_for(&sha).unwrap();
        std::fs::create_dir_all(thumbnail.parent().unwrap()).unwrap();
        let target = store.dir().join("outside.webp");
        std::fs::write(&target, b"outside").unwrap();
        symlink(&target, &thumbnail).unwrap();

        assert!(store.write_thumb(&sha, b"thumbnail").is_err());
    }

    #[test]
    fn delete_is_idempotent() {
        let (store, _tmp) = tmp_store();
        let sha = store.write(b"x").unwrap();
        store.delete(&sha).unwrap();
        store.delete(&sha).unwrap(); // second delete: no error
        assert!(store.read(&sha).is_err());
    }

    #[test]
    fn invalid_hashes_cannot_escape_the_store() {
        let (store, tmp) = tmp_store();
        let outside = tmp.path().join("outside");
        std::fs::write(&outside, b"must survive").unwrap();

        assert!(store.read("../outside").is_err());
        assert!(store.delete("../outside").is_err());
        assert!(store.read_thumb("../outside").is_err());
        assert!(store.delete_thumb("../outside").is_err());
        assert_eq!(std::fs::read(outside).unwrap(), b"must survive");
    }

    #[test]
    fn from_db_path_puts_attachments_next_to_db() {
        let store = AttachmentStore::from_db_path(Path::new("/data/lific/lific.db"));
        assert_eq!(store.dir(), Path::new("/data/lific/attachments"));
    }

    #[test]
    fn hash_is_stable_lowercase_hex() {
        // Known SHA-256 of the empty string.
        assert_eq!(
            AttachmentStore::hash_bytes(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    // ── Attachment text backfill (LIF-418) ───────────────────

    #[test]
    fn backfill_indexes_text_uploads_and_is_idempotent() {
        use crate::db::queries::attachments as q;

        let (store, _tmp) = tmp_store();
        let pool = crate::db::open_memory().expect("test db");

        // A text upload whose bytes are on disk but whose contents never made
        // it into the index: exactly the shape of a pre-migration-042 row.
        let body = b"thread panicked at gribblenaut::render";
        let sha = store.write(body).unwrap();
        let text_id = {
            let conn = pool.write().unwrap();
            q::create_attachment(
                &conn,
                &sha,
                "server.log",
                "text/plain",
                body.len() as i64,
                None,
            )
            .unwrap()
            .id
        };
        // A row whose blob is missing from disk must be skipped, not fatal.
        {
            let conn = pool.write().unwrap();
            let sha = AttachmentStore::hash_bytes(b"no-such-blob");
            q::create_attachment(&conn, &sha, "ghost.log", "text/plain", 10, None).unwrap();
        }

        assert_eq!(backfill_attachment_text(&pool, &store).unwrap(), 1);

        let indexed: String = {
            let conn = pool.read().unwrap();
            conn.query_row(
                "SELECT extracted_text FROM attachments_fts WHERE attachment_id = ?1",
                rusqlite::params![text_id],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(indexed, String::from_utf8_lossy(body));

        assert_eq!(
            backfill_attachment_text(&pool, &store).unwrap(),
            0,
            "a second pass has nothing left to index"
        );
    }

    #[test]
    fn path_rejects_traversal_and_noncanonical_hashes() {
        let (store, dir) = tmp_store();
        for value in ["../lific.toml", &"A".repeat(64)] {
            assert!(store.read(value).is_err());
            assert!(store.delete(value).is_err());
        }
        let valid = "a".repeat(64);
        assert!(store.read(&valid).is_err());
        assert!(store.delete(&valid).is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── MIME sniffing ────────────────────────────────────────

    #[test]
    fn sniff_png_by_signature_ignores_lying_header() {
        let png = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0];
        assert_eq!(
            sniff_and_validate(&png, Some("application/x-msdownload")).unwrap(),
            "image/png"
        );
    }

    #[test]
    fn sniff_jpeg_gif_webp_pdf_zip() {
        assert_eq!(
            sniff_and_validate(&[0xFF, 0xD8, 0xFF, 0], None).unwrap(),
            "image/jpeg"
        );
        assert_eq!(
            sniff_and_validate(b"GIF89a....", None).unwrap(),
            "image/gif"
        );
        let mut webp = Vec::from(*b"RIFF____WEBPVP8 ");
        webp.extend_from_slice(&[0; 4]);
        assert_eq!(sniff_and_validate(&webp, None).unwrap(), "image/webp");
        assert_eq!(
            sniff_and_validate(b"%PDF-1.7\n%...", None).unwrap(),
            "application/pdf"
        );
        assert_eq!(
            sniff_and_validate(&[0x50, 0x4B, 0x03, 0x04, 0], None).unwrap(),
            "application/zip"
        );
    }

    #[test]
    fn rejects_elf_and_pe_executables() {
        assert!(sniff_and_validate(b"\x7FELF....", Some("text/plain")).is_err());
        assert!(sniff_and_validate(b"MZ\x90\x00", Some("text/plain")).is_err());
        assert!(sniff_and_validate(b"#!/bin/sh\n", Some("text/plain")).is_err());
    }

    #[test]
    fn plain_text_accepted_via_declared_type() {
        assert_eq!(
            sniff_and_validate(b"just some log lines\n", Some("text/plain")).unwrap(),
            "text/plain"
        );
    }

    #[test]
    fn svg_accepted_when_declared_and_looks_like_svg() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg"></svg>"#;
        assert_eq!(
            sniff_and_validate(svg, Some("image/svg+xml")).unwrap(),
            "image/svg+xml"
        );
    }

    #[test]
    fn only_raster_images_are_inline_safe() {
        assert!(is_inline_safe_mime("image/png"));
        assert!(is_inline_safe_mime("image/jpeg"));
        assert!(is_inline_safe_mime("image/gif"));
        assert!(is_inline_safe_mime("image/webp"));
        assert!(!is_inline_safe_mime("application/pdf"));
        assert!(!is_inline_safe_mime("text/plain"));
    }

    /// SVG is an image by MIME but a scriptable document in a browser.
    /// Serving it inline from our own origin is stored XSS, so it must never
    /// be classified inline-safe no matter how the allowlist evolves.
    #[test]
    fn svg_is_never_inline_safe() {
        assert!(!is_inline_safe_mime("image/svg+xml"));
    }

    /// Every inline-safe type must still be an accepted upload type. The two
    /// lists are allowed to differ, but not to contradict each other.
    #[test]
    fn inline_safe_types_are_all_allowed_uploads() {
        for mime in ALLOWED_MIMES {
            if is_inline_safe_mime(mime) {
                assert!(ALLOWED_MIMES.contains(mime));
            }
        }
        assert!(ALLOWED_MIMES.contains(&"image/svg+xml"));
    }

    // ── Streamed sniffing (bounded memory) ───────────────────

    /// A reader that manufactures `total` bytes without ever holding them,
    /// recording the largest buffer the consumer offered. If the consumer
    /// slurps the whole stream, the buffer it asks for grows with the content;
    /// if it streams, the request size stays flat.
    struct SyntheticReader {
        byte: u8,
        remaining: u64,
        largest_request: usize,
        served: u64,
    }

    impl SyntheticReader {
        fn new(byte: u8, total: u64) -> Self {
            Self {
                byte,
                remaining: total,
                largest_request: 0,
                served: 0,
            }
        }
    }

    impl Read for SyntheticReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.largest_request = self.largest_request.max(buf.len());
            if self.remaining == 0 {
                return Ok(0);
            }
            let take = buf.len().min(self.remaining as usize);
            buf[..take].fill(self.byte);
            self.remaining -= take as u64;
            self.served += take as u64;
            Ok(take)
        }
    }

    #[test]
    fn streamed_sniffing_agrees_with_the_in_memory_sniffer() {
        let svg = b"<svg xmlns='http://www.w3.org/2000/svg'></svg>".to_vec();
        let cases: Vec<(Vec<u8>, Option<&str>)> = vec![
            (png_image(2, 2), None),
            (png_image(2, 2), Some("application/x-msdownload")),
            (mp4_bytes(), None),
            (webm_bytes(), Some("audio/webm")),
            (b"%PDF-1.7\n%...".to_vec(), None),
            (b"just some log lines\n".to_vec(), Some("text/plain")),
            (b"plain text with no declared type".to_vec(), None),
            (svg, Some("image/svg+xml")),
            (b"\x7FELF....".to_vec(), Some("text/plain")),
            (b"#!/bin/sh\n".to_vec(), None),
            (vec![0xFF, 0xFE, 0x00], Some("text/plain")),
            (Vec::new(), None),
        ];
        for (bytes, declared) in cases {
            let in_memory = sniff_and_validate(&bytes, declared);
            let streamed = sniff_and_validate_stream(bytes.as_slice(), declared);
            match (&in_memory, &streamed) {
                (Ok(a), Ok(b)) => assert_eq!(a, b, "disagreement on {declared:?} {bytes:?}"),
                (Err(_), Err(_)) => {}
                _ => panic!(
                    "streamed and in-memory sniffing disagree: {in_memory:?} vs {streamed:?}"
                ),
            }
        }
    }

    #[test]
    fn streamed_sniffing_reads_a_huge_text_attachment_in_bounded_chunks() {
        // 512 MiB of text. `std::fs::read` would allocate every byte of it;
        // this must not, which is what makes `--allow-large` restores safe.
        const TOTAL: u64 = 512 * 1024 * 1024;
        let mut reader = SyntheticReader::new(b'a', TOTAL);
        let mime = sniff_and_validate_stream(&mut reader, Some("text/plain")).unwrap();

        assert_eq!(mime, "text/plain");
        assert_eq!(reader.served, TOTAL, "every byte must be validated");
        assert!(
            reader.largest_request <= UTF8_STREAM_CHUNK.max(SNIFF_PREFIX_BYTES),
            "the consumer asked for a {}-byte buffer; validation must stay bounded",
            reader.largest_request
        );
    }

    /// Serves `head`, then fails any further read. Proves a decision was made
    /// from the prefix alone rather than by consuming the whole file.
    struct ExplodingTail {
        head: std::io::Cursor<Vec<u8>>,
    }

    impl Read for ExplodingTail {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self.head.read(buf)? {
                0 => Err(std::io::Error::other("read past the sniff prefix")),
                n => Ok(n),
            }
        }
    }

    #[test]
    fn streamed_sniffing_stops_at_the_prefix_once_a_signature_decides_it() {
        let mut png = png_image(2, 2);
        png.resize(SNIFF_PREFIX_BYTES, 0);
        let reader = ExplodingTail {
            head: std::io::Cursor::new(png),
        };
        assert_eq!(
            sniff_and_validate_stream(reader, None).unwrap(),
            "image/png",
            "a signature settles the type without reading the rest of the file"
        );
    }

    #[test]
    fn streamed_utf8_validation_handles_characters_split_across_chunks() {
        // A 3-byte character straddling the prefix boundary must not read as
        // invalid UTF-8, and a truncated one at EOF must not read as valid.
        let mut text = vec![b'a'; SNIFF_PREFIX_BYTES - 1];
        text.extend_from_slice("€ tail".as_bytes());
        assert_eq!(
            sniff_and_validate_stream(text.as_slice(), Some("text/plain")).unwrap(),
            "text/plain"
        );
        assert_eq!(
            sniff_and_validate(&text, Some("text/plain")).unwrap(),
            "text/plain"
        );

        let mut truncated = vec![b'a'; SNIFF_PREFIX_BYTES + 16];
        truncated.extend_from_slice(&"€".as_bytes()[..2]);
        assert!(
            sniff_and_validate_stream(truncated.as_slice(), Some("text/plain")).is_err(),
            "a file ending mid-character is not valid text"
        );
        assert!(sniff_and_validate(&truncated, Some("text/plain")).is_err());

        // Invalid past the prefix: the streaming pass must still catch it.
        let mut binary = vec![b'a'; SNIFF_PREFIX_BYTES + 8];
        binary.extend_from_slice(&[0xC3, 0x28]);
        assert!(sniff_and_validate_stream(binary.as_slice(), Some("text/plain")).is_err());
    }

    // ── LIF-418: media types ─────────────────────────────────

    /// The smallest byte sequences that identify each media container.
    fn mp4_bytes() -> Vec<u8> {
        let mut v = vec![0, 0, 0, 0x20];
        v.extend_from_slice(b"ftypisom");
        v.extend_from_slice(b"\0\0\x02\0isomiso2avc1mp41");
        v
    }

    fn webm_bytes() -> Vec<u8> {
        let mut v = vec![0x1A, 0x45, 0xDF, 0xA3];
        // A trimmed EBML header: enough structure to carry the DocType string
        // our sniffer looks for.
        v.extend_from_slice(&[0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1F]);
        v.extend_from_slice(b"\x42\x82\x84webm");
        v.extend_from_slice(&[0; 16]);
        v
    }

    #[test]
    fn sniffs_every_new_media_container() {
        assert_eq!(sniff_and_validate(&mp4_bytes(), None).unwrap(), "video/mp4");
        assert_eq!(
            sniff_and_validate(&webm_bytes(), None).unwrap(),
            "video/webm"
        );
        let mut ogg = Vec::from(*b"OggS");
        ogg.extend_from_slice(&[0; 32]);
        assert_eq!(sniff_and_validate(&ogg, None).unwrap(), "audio/ogg");
        let mut id3 = Vec::from(*b"ID3\x03\x00\x00");
        id3.extend_from_slice(&[0; 32]);
        assert_eq!(sniff_and_validate(&id3, None).unwrap(), "audio/mpeg");
        // A bare MPEG frame header (sync + MPEG1 Layer III).
        assert_eq!(
            sniff_and_validate(&[0xFF, 0xFB, 0x90, 0x00, 0, 0, 0, 0], None).unwrap(),
            "audio/mpeg"
        );
    }

    /// The container decides it IS webm; the declared type only chooses
    /// between the two allowlisted labels for that one container.
    #[test]
    fn webm_declared_as_audio_is_recorded_as_audio() {
        assert_eq!(
            sniff_and_validate(&webm_bytes(), Some("audio/webm")).unwrap(),
            "audio/webm"
        );
        // A lie in the other direction cannot make a PNG into a video.
        assert_eq!(
            sniff_and_validate(&png_image(2, 2), Some("video/mp4")).unwrap(),
            "image/png"
        );
    }

    /// QuickTime shares the ISO base media container with mp4 but is not on
    /// the allowlist, so it must not be waved through wearing an mp4 label.
    #[test]
    fn quicktime_is_not_relabelled_as_mp4() {
        let mut mov = vec![0, 0, 0, 0x14];
        mov.extend_from_slice(b"ftypqt  ");
        mov.extend_from_slice(&[0xFF; 16]);
        // No signature match and not valid UTF-8, so nothing is left to fall
        // back to: rejected outright rather than stored as video.
        assert!(sniff_and_validate(&mov, Some("video/mp4")).is_err());
        assert_eq!(sniff_magic(&mov), None);
    }

    /// A 0xFF lead byte is not enough: the version and layer fields both have
    /// reserved encodings that a real MPEG frame never carries.
    #[test]
    fn reserved_mpeg_header_fields_are_not_audio() {
        // Version bits 01 (reserved).
        let reserved_version = [0xFF, 0xEA, 0x00, 0x00, 0xFF, 0xFE];
        assert_ne!(sniff_magic(&reserved_version), Some("audio/mpeg"));
        // Layer bits 00 (reserved).
        let reserved_layer = [0xFF, 0xF9, 0x00, 0x00, 0xFF, 0xFE];
        assert_ne!(sniff_magic(&reserved_layer), Some("audio/mpeg"));
    }

    #[test]
    fn sqlite_databases_are_allowed_and_never_inline() {
        let mut db = SQLITE_MAGIC.to_vec();
        db.extend_from_slice(&[0; 64]);
        assert_eq!(
            sniff_and_validate(&db, None).unwrap(),
            "application/vnd.sqlite3"
        );
        assert!(ALLOWED_MIMES.contains(&"application/vnd.sqlite3"));
        assert!(!is_inline_safe_mime("application/vnd.sqlite3"));
    }

    /// Media plays in place. It is not a scripting surface, and a `<video>`
    /// element cannot work against a forced download.
    #[test]
    fn media_types_serve_inline() {
        for mime in [
            "video/mp4",
            "video/webm",
            "audio/webm",
            "audio/ogg",
            "audio/mpeg",
        ] {
            assert!(is_inline_safe_mime(mime), "{mime} must serve inline");
            assert!(ALLOWED_MIMES.contains(&mime), "{mime} must be uploadable");
        }
    }

    // ── LIF-418: dimensions + thumbnails ─────────────────────

    #[test]
    fn decodes_raster_dimensions() {
        assert_eq!(image_dimensions(&png_image(37, 11)), Some((37, 11)));
        assert_eq!(image_dimensions(b"not an image at all"), None);
    }

    #[test]
    fn raster_mimes_are_the_decodable_ones() {
        for mime in ["image/png", "image/jpeg", "image/gif", "image/webp"] {
            assert!(is_raster_mime(mime));
        }
        // A vector has no intrinsic pixel size, and a video is not decoded.
        assert!(!is_raster_mime("image/svg+xml"));
        assert!(!is_raster_mime("video/mp4"));
    }

    #[test]
    fn thumbnail_fits_the_long_edge_and_preserves_aspect() {
        let thumb = generate_thumbnail(&png_image(1200, 300))
            .unwrap()
            .expect("an oversize image gets a thumbnail");
        let (w, h) = image_dimensions(&thumb).expect("thumbnail decodes");
        assert_eq!((w, h), (480, 120));
        // WebP, per the endpoint's declared content type.
        assert!(thumb.starts_with(b"RIFF"));
        assert_eq!(&thumb[8..12], b"WEBP");
    }

    /// An image already inside the thumbnail box gets none: the original IS
    /// the small version, and a second copy would only cost disk.
    #[test]
    fn small_images_get_no_thumbnail() {
        assert!(generate_thumbnail(&png_image(480, 100)).unwrap().is_none());
        assert!(generate_thumbnail(&png_image(64, 64)).unwrap().is_none());
        assert!(generate_thumbnail(b"not an image").is_err());
    }

    #[test]
    fn expects_thumbnail_tracks_mime_and_long_edge() {
        assert!(expects_thumbnail("image/png", Some(1000), Some(10)));
        assert!(expects_thumbnail("image/jpeg", Some(10), Some(1000)));
        assert!(!expects_thumbnail("image/png", Some(480), Some(480)));
        assert!(!expects_thumbnail("image/png", None, None));
        assert!(!expects_thumbnail("video/mp4", Some(1920), Some(1080)));
        assert!(!expects_thumbnail(
            "application/pdf",
            Some(1920),
            Some(1080)
        ));
    }

    #[test]
    fn thumbnails_round_trip_and_live_in_a_subdirectory() {
        let (store, _tmp) = tmp_store();
        let sha = store.write(&png_image(600, 600)).unwrap();
        assert!(store.read_thumb(&sha).unwrap().is_none());

        store.write_thumb(&sha, b"pretend webp").unwrap();
        assert_eq!(store.read_thumb(&sha).unwrap().unwrap(), b"pretend webp");
        assert_eq!(
            store.thumb_path_for(&sha).unwrap().parent().unwrap(),
            store.dir().join("thumbs")
        );

        // Deleting the blob takes its derivative with it.
        store.delete(&sha).unwrap();
        assert!(store.read_thumb(&sha).unwrap().is_none());
        // And deleting a thumbnail that is already gone is fine.
        store.delete_thumb(&sha).unwrap();
    }
}
