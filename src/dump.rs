//! LIF-266: backup as an interface — `lific dump` / `lific restore`.
//!
//! The data set is no longer a single file. Since attachments (LIF-262) it is
//! `lific.db` plus a content-addressed `attachments/` sidecar dir living beside
//! it (blobs named by their sha256; in-progress writes carry a `.tmp`
//! extension — see [`crate::storage`]). A DB snapshot alone would restore
//! metadata rows pointing at missing blobs. Archived projects add a second
//! sidecar, `archived-projects/`, holding each archived project's portable
//! `.lific.tar.gz`; those projects exist nowhere else, so a dump that skipped
//! the directory would silently drop them from backup coverage.
//!
//! This module follows the `gitea dump` pattern: one command produces one
//! self-contained, timestamped `lific_YYYYMMDD_HHMMSS.tar.gz` archive with
//! everything needed to restore — the DB, every attachment blob, and a
//! `manifest.json` describing what's inside. [`restore`] validates and unpacks
//! it back into a data dir. The interval backup task (`src/backup.rs`) emits
//! the *same* artifact via [`write_dump`], so there is exactly one backup shape.

use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::db::DbPool;
use crate::error::LificError;

/// The DB filename inside every archive, independent of the on-disk name.
pub const ARCHIVE_DB_NAME: &str = "lific.db";
/// The manifest filename inside every archive.
pub const ARCHIVE_MANIFEST_NAME: &str = "manifest.json";
/// The prefix under which attachment blobs are stored inside the archive.
pub const ARCHIVE_ATTACHMENTS_PREFIX: &str = "attachments/";
/// Directory name of the archived-project store beside the database, a
/// sibling of `attachments/`. Each file in it is one project's portable
/// `.lific.tar.gz` archive, written when the project was archived.
pub const ARCHIVED_PROJECTS_DIR_NAME: &str = "archived-projects";
/// The prefix under which archived-project files are stored inside the dump.
pub const ARCHIVE_ARCHIVED_PROJECTS_PREFIX: &str = "archived-projects/";
/// Longest accepted archived-project filename, in bytes.
pub const MAX_ARCHIVED_PROJECT_NAME_BYTES: usize = 160;
/// Required suffix of every archived-project filename.
const ARCHIVED_PROJECT_SUFFIX: &str = ".lific.tar.gz";

/// Restore limits apply to the uncompressed archive contents. They prevent a
/// tiny gzip/tar upload from allocating unbounded memory or filling the data
/// volume before validation can reject it.
pub const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
pub const MAX_DB_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_ATTACHMENT_BYTES: u64 = 64 * 1024 * 1024;
/// Per-file ceiling for one archived project. A project archive carries the
/// project's rows and its attachment blobs, so it gets the database's ceiling
/// rather than a single attachment's; the running total is still bounded by
/// [`MAX_TOTAL_RESTORE_BYTES`].
pub const MAX_ARCHIVED_PROJECT_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_TOTAL_RESTORE_BYTES: u64 = 1024 * 1024 * 1024;
pub const MAX_RESTORE_ENTRIES: u64 = 10_000;
const ATTACHMENTS_SCHEMA_VERSION: i64 = 31;
const ATTACHMENT_INTEGRITY_SCHEMA_VERSION: i64 = 43;
const TAR_ENTRY_OVERHEAD: u64 = 1024;
const TAR_END_MARKER_BYTES: u64 = 1024;

/// Ceiling used by [`RestoreLimits::trusted`]. Large enough that no dump this
/// binary can produce is categorically unrestorable, small enough that a
/// decompression bomb still hits a wall rather than filling the volume
/// forever.
const TRUSTED_RESTORE_BYTES: u64 = 16 * 1024 * 1024 * 1024 * 1024; // 16 TiB
const TRUSTED_RESTORE_ENTRIES: u64 = 10_000_000;
const TRUSTED_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;

/// Bounds applied to the *uncompressed* contents of an archive being restored.
///
/// [`RestoreLimits::default`] is what every untrusted restore gets: a tiny
/// gzip upload cannot allocate unbounded memory or fill the data volume before
/// validation rejects it. [`RestoreLimits::trusted`] is the `--allow-large`
/// escape hatch for an operator restoring their own legitimately large dump —
/// [`write_dump`] has no size ceiling, so without it a big-but-honest instance
/// could take backups that nothing could ever restore.
#[derive(Debug, Clone, Copy)]
pub struct RestoreLimits {
    pub max_manifest_bytes: u64,
    pub max_db_bytes: u64,
    pub max_attachment_bytes: u64,
    /// Ceiling for one file under `archived-projects/`.
    pub max_archived_project_bytes: u64,
    pub max_total_bytes: u64,
    /// Ceiling on attachment plus archived-project entries.
    pub max_entries: u64,
}

impl Default for RestoreLimits {
    fn default() -> Self {
        Self {
            max_manifest_bytes: MAX_MANIFEST_BYTES,
            max_db_bytes: MAX_DB_BYTES,
            max_attachment_bytes: MAX_ATTACHMENT_BYTES,
            max_archived_project_bytes: MAX_ARCHIVED_PROJECT_BYTES,
            max_total_bytes: MAX_TOTAL_RESTORE_BYTES,
            max_entries: MAX_RESTORE_ENTRIES,
        }
    }
}

impl RestoreLimits {
    /// Limits for an archive the operator vouches for (`lific restore
    /// --allow-large`). Every structural check still runs — entry names, tar
    /// entry types, manifest/content agreement, database integrity and blob
    /// hashes — only the size ceilings are raised.
    pub fn trusted() -> Self {
        Self {
            max_manifest_bytes: TRUSTED_MANIFEST_BYTES,
            max_db_bytes: TRUSTED_RESTORE_BYTES,
            max_attachment_bytes: TRUSTED_RESTORE_BYTES,
            max_archived_project_bytes: TRUSTED_RESTORE_BYTES,
            max_total_bytes: TRUSTED_RESTORE_BYTES,
            max_entries: TRUSTED_RESTORE_ENTRIES,
        }
    }

    /// Hard cap handed to the gzip reader, so a decompression bomb is stopped
    /// by the reader itself rather than by a per-entry check.
    fn max_decompressed_bytes(&self) -> u64 {
        self.max_total_bytes
            .saturating_add(self.max_manifest_bytes)
            .saturating_add(
                self.max_entries
                    .saturating_add(2)
                    .saturating_mul(TAR_ENTRY_OVERHEAD),
            )
            .saturating_add(TAR_END_MARKER_BYTES)
    }
}

/// Everything [`run_restore_with`] needs beyond the paths.
#[derive(Debug, Clone, Copy, Default)]
pub struct RestoreOptions {
    /// Overwrite an existing database, moving the current one aside.
    pub force: bool,
    /// Size bounds for the archive.
    pub limits: RestoreLimits,
}

impl RestoreOptions {
    /// `allow_large` selects [`RestoreLimits::trusted`] over the bounded
    /// defaults.
    pub fn new(force: bool, allow_large: bool) -> Self {
        Self {
            force,
            limits: if allow_large {
                RestoreLimits::trusted()
            } else {
                RestoreLimits::default()
            },
        }
    }
}

/// Metadata describing an archive's contents. Serialized as `manifest.json`
/// at the root of every dump so a restore can validate compatibility and print
/// a summary without opening the DB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Crate version that produced the archive.
    pub lific_version: String,
    /// Highest applied migration version in the snapshotted DB. A restore onto
    /// an older binary (lower [`crate::db::migrate::latest_version`]) is
    /// refused.
    pub schema_version: i64,
    /// ISO 8601 UTC timestamp the dump was taken.
    pub created_at: String,
    /// Size of the snapshotted DB file in bytes.
    pub db_size_bytes: u64,
    /// Number of attachment blobs included.
    pub attachment_count: u64,
    /// Total bytes across all attachment blobs.
    pub attachment_bytes: u64,
    /// Number of archived-project files included. Absent (zero) in dumps
    /// taken before project archiving existed.
    #[serde(default)]
    pub archived_project_count: u64,
    /// Total bytes across all archived-project files.
    #[serde(default)]
    pub archived_project_bytes: u64,
}

/// A UTC timestamp in the archive filename convention (`YYYYMMDD_HHMMSS`),
/// matching the legacy backup naming so both schemes sort together.
pub fn archive_timestamp() -> String {
    chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string()
}

/// The default archive filename for a given DB stem and timestamp, e.g.
/// `lific_20260101_120000.tar.gz`.
pub fn archive_filename(db_stem: &str, timestamp: &str) -> String {
    format!("{db_stem}_{timestamp}.tar.gz")
}

/// Resolve the attachments sidecar dir for a database path (mirrors
/// [`crate::storage::AttachmentStore::from_db_path`]).
fn attachments_dir_for(db_path: &Path) -> PathBuf {
    match db_path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join("attachments"),
        _ => PathBuf::from("attachments"),
    }
}

/// Resolve the archived-projects dir for a database path: a sibling of
/// `attachments/` in the data dir.
pub(crate) fn archived_projects_dir_for(db_path: &Path) -> PathBuf {
    match db_path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(ARCHIVED_PROJECTS_DIR_NAME),
        _ => PathBuf::from(ARCHIVED_PROJECTS_DIR_NAME),
    }
}

/// Whether `name` is an acceptable archived-project filename:
/// `^[A-Za-z0-9][A-Za-z0-9._-]*\.lific\.tar\.gz$`, at most
/// [`MAX_ARCHIVED_PROJECT_NAME_BYTES`] bytes. The rule excludes separators,
/// a leading dot (so `.`/`..` and in-progress `.tmp*` files never match) and
/// anything non-ASCII.
pub(crate) fn valid_archived_project_name(name: &str) -> bool {
    if name.len() > MAX_ARCHIVED_PROJECT_NAME_BYTES {
        return false;
    }
    let Some(stem) = name.strip_suffix(ARCHIVED_PROJECT_SUFFIX) else {
        return false;
    };
    let mut bytes = stem.bytes();
    match bytes.next() {
        Some(first) if first.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Return whether another process currently owns a dump staging lock.
pub(crate) fn staging_is_locked(path: &Path) -> bool {
    let lock = path.join("lock");
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock)
    else {
        return true;
    };
    file.try_lock_exclusive().is_err()
}

/// Take a consistent snapshot of the live DB into the already-reserved staging
/// file `dest`.
///
/// The SQLite online backup API holds no long writer lock and copies into a
/// file this process created with `O_CREAT|O_EXCL|O_NOFOLLOW` — so, unlike
/// `VACUUM INTO`, nothing resolves a pathname a second time between reserving
/// the destination and writing it.
///
/// The source is a dedicated read-only connection opened here and dropped when
/// the snapshot finishes, not a pooled reader. Pooled connections carry
/// `mmap_size = 64MB` and never close, and the backup API reads the entire
/// database — so routing snapshots through the pool faulted each reader's full
/// mmap window into RSS permanently. On the production instance that cost
/// ~430MB of resident double-counted page-cache mappings (one 64MB window per
/// reader the 30-minute backup task had ever touched). A short-lived source
/// connection with SQLite's default `mmap_size = 0` reads through plain I/O
/// and gives every page back when it closes; the pooled readers only ever map
/// what their own queries touch.
fn snapshot_db(db_path: &Path, dest: &TempFile) -> Result<(), LificError> {
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|e| LificError::Internal(format!("open snapshot source connection: {e}")))?;
    conn.busy_timeout(std::time::Duration::from_millis(5000))
        .map_err(|e| LificError::Internal(format!("set snapshot busy timeout: {e}")))?;
    let mut destination = rusqlite::Connection::open_with_flags(
        dest.path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|e| LificError::Internal(format!("open SQLite staging file: {e}")))?;
    let backup = rusqlite::backup::Backup::new(&conn, &mut destination)
        .map_err(|e| LificError::Internal(format!("create SQLite backup: {e}")))?;
    backup
        .run_to_completion(100, std::time::Duration::ZERO, None)
        .map_err(|e| LificError::Internal(format!("SQLite backup failed: {e}")))?;
    Ok(())
}

/// Set 0600 permissions on a file (owner-only) on Unix. No-op elsewhere.
#[cfg_attr(
    not(unix),
    expect(
        clippy::unnecessary_wraps,
        reason = "keep one fallible cross-platform dump API"
    )
)]
fn set_owner_only(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// Set 0600 on an already-open handle, so no pathname is resolved again.
#[cfg_attr(
    not(unix),
    expect(clippy::unnecessary_wraps, reason = "fallible on Unix")
)]
fn set_owner_only_file(file: &File) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        let _ = file;
    }
    Ok(())
}

/// Set 0700 permissions on a directory (owner-only) on Unix. No-op elsewhere.
#[cfg_attr(
    not(unix),
    expect(clippy::unnecessary_wraps, reason = "fallible on Unix")
)]
fn set_owner_only_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// fsync a directory so a rename into it is durable. Unix only: Windows has no
/// directory handle to sync, and its rename ordering does not need one.
#[cfg_attr(
    not(unix),
    expect(clippy::unnecessary_wraps, reason = "fallible on Unix")
)]
fn sync_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// A file this process created exclusively and owns for its lifetime; removed
/// on drop, including while a `?` unwinds out of [`write_dump`].
struct TempFile {
    file: File,
    path: PathBuf,
}

impl TempFile {
    fn create(path: PathBuf) -> Result<Self, LificError> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(&path).map_err(|error| {
            LificError::Internal(format!("create secure staging file: {error}"))
        })?;
        Ok(Self { file, path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// An advisory exclusive lock held for the lifetime of one dump's staging
/// directory. The interval backup sweep reads it through
/// [`staging_is_locked`], so an in-flight dump is never swept out from under
/// itself, and a crashed one releases the lock with the process.
struct StagingLock {
    _file: File,
}

impl StagingLock {
    fn create(path: &Path) -> Result<Self, LificError> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|error| LificError::Internal(format!("create dump lock: {error}")))?;
        file.try_lock_exclusive()
            .map_err(|error| LificError::Internal(format!("lock dump staging: {error}")))?;
        Ok(Self { _file: file })
    }
}

/// Touch the staging activity marker so the age-gated sweep in
/// [`crate::backup`] can tell a long-running dump from a crash leftover.
fn refresh_activity(file: &File) -> std::io::Result<()> {
    file.set_modified(std::time::SystemTime::now())
}

/// Write a self-contained dump archive to `out_path`.
///
/// Shared code path used by both `lific dump` and the interval backup task.
/// Produces a gzip-compressed tar containing `lific.db` (a consistent snapshot
/// taken with SQLite's online backup API), every non-`.tmp` attachment blob
/// under `attachments/`, every archived-project file under
/// `archived-projects/`, and `manifest.json`. The finished file is chmod 0600
/// (it contains the whole DB).
///
/// Everything is staged in a private directory beside the output and published
/// with a single rename, so nothing partial is ever visible at `out_path`, and
/// the destination is checked for symlink/hard-link tricks first.
///
/// Returns the [`Manifest`] that was written, so callers can log/print it.
pub fn write_dump(pool: &DbPool, db_path: &Path, out_path: &Path) -> Result<Manifest, LificError> {
    // Hold the attachment store's lock for the whole dump: the DB snapshot,
    // the blob scan, the hash verification and the publication. An upload,
    // delete or GC sweep landing between the snapshot and the scan would
    // otherwise produce an archive whose database references a blob the
    // archive does not contain, or whose blob bytes no longer match the hash
    // that was verified. Store lock first, then the dedicated snapshot
    // connection inside `snapshot_db`; the pool is only used for metadata
    // queries after the snapshot exists.
    let store = crate::storage::AttachmentStore::from_db_path(db_path);
    store.with_lock(|_| write_dump_locked(pool, db_path, out_path))
}

fn write_dump_locked(
    pool: &DbPool,
    db_path: &Path,
    out_path: &Path,
) -> Result<Manifest, LificError> {
    // A bare filename has an empty parent, which is the current directory.
    let parent = match out_path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    validate_dump_destination(out_path)?;

    // Everything is staged inside a private 0700 directory beside the output.
    // Another user of a shared output directory therefore cannot swap either
    // staging pathname while an open handle to it is live, and a crash leaves
    // exactly one sweepable directory rather than loose `*.tmp` files
    // (LIF-329).
    let staging = tempfile::Builder::new()
        .prefix(".lific-dump-")
        .tempdir_in(parent)
        .map_err(|e| LificError::Internal(format!("create dump staging directory: {e}")))?;
    set_owner_only_dir(staging.path())
        .map_err(|e| LificError::Internal(format!("secure dump staging directory: {e}")))?;
    let _lock = StagingLock::create(&staging.path().join("lock"))?;
    let activity = TempFile::create(staging.path().join("activity"))?;
    refresh_activity(&activity.file)
        .map_err(|e| LificError::Internal(format!("mark dump staging active: {e}")))?;

    let tmp_db = TempFile::create(staging.path().join("dbsnapshot"))?;
    snapshot_db(db_path, &tmp_db)?;
    refresh_activity(&activity.file)
        .map_err(|e| LificError::Internal(format!("refresh dump staging activity: {e}")))?;

    let tmp_archive = TempFile::create(staging.path().join("archive"))?;

    let db_size_bytes = tmp_db
        .file
        .metadata()
        .map(|m| m.len())
        .map_err(|e| LificError::Internal(format!("size db snapshot: {e}")))?;

    // Gather attachment blobs and archived-project files. Each candidate is
    // opened no-follow and verified here; the manifest is built from what those
    // handles reported, and the archive pass below re-opens each one and
    // refuses to archive anything that is no longer the same object. A blob
    // filename is a bare lowercase sha256 (so in-progress `.tmp` writes never
    // qualify); anything else in the store is not ours to archive. The
    // archived-project files are written under the same store lock this dump
    // holds, so the file set agrees with the database snapshot above.
    let (blobs, attachment_bytes) =
        scan_verified_files(&attachments_dir_for(db_path), &ATTACHMENT_FILES, |name| {
            validate_attachment_entry(&format!("{ARCHIVE_ATTACHMENTS_PREFIX}{name}")).is_ok()
        })?;
    let (archived_projects, archived_project_bytes) = scan_verified_files(
        &archived_projects_dir_for(db_path),
        &ARCHIVED_PROJECT_FILES,
        valid_archived_project_name,
    )?;

    let manifest = Manifest {
        lific_version: env!("CARGO_PKG_VERSION").to_string(),
        schema_version: schema_version(pool),
        created_at: chrono::Utc::now().to_rfc3339(),
        db_size_bytes,
        attachment_count: blobs.len() as u64,
        attachment_bytes,
        archived_project_count: archived_projects.len() as u64,
        archived_project_bytes,
    };
    let manifest_json = serde_json::to_vec_pretty(&manifest)
        .map_err(|e| LificError::Internal(format!("serialize manifest: {e}")))?;

    // Build the archive into the reserved staging file, then atomically rename
    // it into place so a partial write is never observed at the final path.
    {
        let file = tmp_archive
            .file
            .try_clone()
            .map_err(|e| LificError::Internal(format!("open archive staging handle: {e}")))?;
        let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut tar = tar::Builder::new(enc);

        // manifest.json
        append_bytes(&mut tar, ARCHIVE_MANIFEST_NAME, &manifest_json)?;
        // lific.db (from the snapshot file)
        tar.append_path_with_name(tmp_db.path(), ARCHIVE_DB_NAME)
            .map_err(|e| LificError::Internal(format!("append db to archive: {e}")))?;
        // attachments/<sha256> and archived-projects/<name>. One handle is
        // open at a time, so a store with thousands of blobs cannot exhaust
        // the process's file descriptors.
        append_verified_files(&mut tar, &activity.file, &blobs, &ATTACHMENT_FILES)?;
        append_verified_files(
            &mut tar,
            &activity.file,
            &archived_projects,
            &ARCHIVED_PROJECT_FILES,
        )?;

        let enc = tar
            .into_inner()
            .map_err(|e| LificError::Internal(format!("finalize tar: {e}")))?;
        enc.finish()
            .map_err(|e| LificError::Internal(format!("finalize gzip: {e}")))?;
    }

    tmp_archive
        .file
        .sync_all()
        .map_err(|e| LificError::Internal(format!("sync archive: {e}")))?;
    set_owner_only_file(&tmp_archive.file)
        .map_err(|e| LificError::Internal(format!("chmod archive: {e}")))?;
    std::fs::rename(tmp_archive.path(), out_path)
        .map_err(|e| LificError::Internal(format!("finalize archive: {e}")))?;
    // The rename is only durable once the directory entry is on disk.
    sync_dir(parent).map_err(|e| LificError::Internal(format!("sync dump destination: {e}")))?;

    Ok(manifest)
}

/// One kind of sidecar file a dump carries beside the database.
struct SidecarFiles {
    /// Singular noun for messages ("attachment").
    label: &'static str,
    /// Tar entry prefix, ending in `/`.
    prefix: &'static str,
    /// Whether each filename is the sha256 of its content, verified while
    /// the bytes stream into the archive.
    content_addressed: bool,
}

const ATTACHMENT_FILES: SidecarFiles = SidecarFiles {
    label: "attachment",
    prefix: ARCHIVE_ATTACHMENTS_PREFIX,
    content_addressed: true,
};

/// Archived-project files are not content addressed: the app checks each
/// one against the sha256 in its `archived_projects` row at unarchive time.
const ARCHIVED_PROJECT_FILES: SidecarFiles = SidecarFiles {
    label: "archived project",
    prefix: ARCHIVE_ARCHIVED_PROJECTS_PREFIX,
    content_addressed: false,
};

/// A file the scan pass accepted: its bare name, its path and the identity
/// the archive pass must find again.
struct ScannedFile {
    name: String,
    path: PathBuf,
    identity: BlobIdentity,
}

/// Scan `dir` for regular files whose names pass `accept`, opening each one
/// no-follow and recording its identity. Symlinks, directories, vanished
/// entries and non-UTF-8 names are skipped; a hard-linked file fails the
/// dump. A missing `dir` yields nothing. Returns the files and their total
/// size.
fn scan_verified_files(
    dir: &Path,
    kind: &SidecarFiles,
    accept: impl Fn(&str) -> bool,
) -> Result<(Vec<ScannedFile>, u64), LificError> {
    let label = kind.label;
    let mut files = Vec::new();
    let mut total: u64 = 0;
    if !dir.is_dir() {
        return Ok((files, total));
    }
    for entry in std::fs::read_dir(dir)
        .map_err(|e| LificError::Internal(format!("read {label} directory: {e}")))?
    {
        let entry = entry.map_err(|e| LificError::Internal(format!("read {label} entry: {e}")))?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !accept(name) {
            continue;
        }
        let Some(file) = open_verified_blob(&path, label)? else {
            continue;
        };
        total = total
            .checked_add(file.identity.size)
            .ok_or_else(|| LificError::Internal(format!("{label} size overflow")))?;
        files.push(ScannedFile {
            name: name.to_string(),
            path: path.clone(),
            identity: file.identity,
        });
    }
    Ok((files, total))
}

/// Archive every scanned file under `kind.prefix`, re-opening each one and
/// refusing anything that is no longer the object the scan recorded.
fn append_verified_files<W: Write>(
    tar: &mut tar::Builder<W>,
    activity: &File,
    files: &[ScannedFile],
    kind: &SidecarFiles,
) -> Result<(), LificError> {
    let label = kind.label;
    for scanned in files {
        let name = &scanned.name;
        refresh_activity(activity)
            .map_err(|e| LificError::Internal(format!("refresh dump staging activity: {e}")))?;
        let Some(mut file) = open_verified_blob(&scanned.path, label)? else {
            return Err(LificError::Internal(format!(
                "{label} {name} was replaced while the dump was running"
            )));
        };
        if file.identity != scanned.identity {
            return Err(LificError::Internal(format!(
                "{label} {name} changed while the dump was running"
            )));
        }
        let entry_name = format!("{}{name}", kind.prefix);
        let digest = file.append_to(tar, &entry_name, label, name)?;
        // In a content-addressed store the name IS the digest. Hashing during
        // the copy is the only way to be sure the bytes in the archive are the
        // bytes that were verified: hashing a second pass over the file would
        // prove something about a different read. A mismatch fails the dump
        // before publication, so a corrupted store (bit rot, a hand-edited
        // blob, a name that never matched its content) can never be published
        // as a valid-looking backup that a restore would then reject.
        if kind.content_addressed && digest != *name {
            return Err(LificError::Internal(format!(
                "{label} {name} does not match its content address (hashed {digest}); \
                 refusing to publish a corrupt archive"
            )));
        }
    }
    Ok(())
}

/// A sidecar file (an attachment blob or an archived project) opened once,
/// verified through that same handle, and archived from it.
///
/// Checking a pathname and then handing the pathname to `tar` for a second
/// open is a TOCTOU window: whoever can write the sidecar directory can
/// swap the name for a symlink between the two. Everything here (the file
/// type, the Unix link count, and the size written into the tar header) comes
/// from the one descriptor whose bytes end up in the archive.
struct VerifiedBlob {
    file: File,
    identity: BlobIdentity,
    mtime: u64,
}

/// What the scan pass recorded about a blob, so the archive pass can prove it
/// is streaming the same object rather than something swapped in behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BlobIdentity {
    size: u64,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
}

/// Whether an open failure means "that name is a symlink and O_NOFOLLOW
/// refused it". Linux reports `ELOOP`, the BSDs `EMLINK`;
/// `ErrorKind::FilesystemLoop` is still unstable, so match the raw codes.
fn is_symlink(error: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        matches!(error.raw_os_error(), Some(code) if code == libc::ELOOP || code == libc::EMLINK)
    }
    #[cfg(not(unix))]
    {
        let _ = error;
        false
    }
}

/// Open `path` without following symlinks and validate it as an archivable
/// blob. `Ok(None)` means "not ours to archive" (a symlink, a directory, or an
/// entry that vanished mid-scan); `Err` means the store is in a state a dump
/// must not silently paper over.
fn open_verified_blob(path: &Path, label: &str) -> Result<Option<VerifiedBlob>, LificError> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        // A concurrent GC can remove a blob between readdir and open, and
        // O_NOFOLLOW refuses a symlink. Neither is a reason to fail the dump;
        // both mean "nothing of ours to archive here".
        Err(error) if error.kind() == std::io::ErrorKind::NotFound || is_symlink(&error) => {
            return Ok(None);
        }
        Err(error) => {
            return Err(LificError::Internal(format!(
                "open {label} {}: {error}",
                path.display()
            )));
        }
    };

    let metadata = file
        .metadata()
        .map_err(|e| LificError::Internal(format!("inspect {label} {}: {e}", path.display())))?;
    if !metadata.is_file() {
        return Ok(None);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // A hard link means the same inode is reachable from outside the
        // store, so its bytes are not under Lific's control.
        if metadata.nlink() != 1 {
            return Err(LificError::Internal(format!(
                "{label} entry is hard-linked: {}",
                path.display()
            )));
        }
    }
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());

    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        BlobIdentity {
            size: metadata.len(),
            dev: metadata.dev(),
            ino: metadata.ino(),
        }
    };
    #[cfg(not(unix))]
    let identity = BlobIdentity {
        size: metadata.len(),
    };

    Ok(Some(VerifiedBlob {
        file,
        identity,
        mtime,
    }))
}

impl VerifiedBlob {
    /// Stream this file into the archive from its verified handle, hashing the
    /// bytes on the way through, and return their sha256 so the caller can
    /// check a content address against exactly the bytes that were archived.
    /// `name` is the bare filename, used in messages.
    fn append_to<W: Write>(
        &mut self,
        tar: &mut tar::Builder<W>,
        entry_name: &str,
        label: &str,
        name: &str,
    ) -> Result<String, LificError> {
        let size = self.identity.size;
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(size);
        header.set_mode(0o600);
        header.set_mtime(self.mtime);
        header.set_cksum();
        self.file
            .rewind()
            .map_err(|e| LificError::Internal(format!("rewind {label} {name}: {e}")))?;
        let mut hashing = HashingReader::new((&self.file).take(size));
        tar.append_data(&mut header, entry_name, &mut hashing)
            .map_err(|e| LificError::Internal(format!("append {label} {name}: {e}")))?;
        let digest = hashing.hex_digest();
        // The header already promised `size` bytes; publishing an archive whose
        // body is shorter would leave every later entry misaligned.
        let written = self
            .file
            .stream_position()
            .map_err(|e| LificError::Internal(format!("measure {label} {name}: {e}")))?;
        if written != size {
            return Err(LificError::Internal(format!(
                "{label} {name} changed size while being archived \
                 ({written} of {size} bytes)"
            )));
        }
        Ok(digest)
    }
}

/// A reader that digests everything it passes through, so a copy and its
/// verification see exactly the same bytes.
struct HashingReader<R> {
    inner: R,
    hasher: Sha256,
}

impl<R: Read> HashingReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
        }
    }

    fn hex_digest(self) -> String {
        crate::auth::hex_encode(&self.hasher.finalize())
    }
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.hasher.update(&buf[..read]);
        Ok(read)
    }
}

/// A writer that digests everything written through it, so a restore can
/// stage a file and learn its sha256 in the same pass.
struct HashingWriter<W> {
    inner: W,
    hasher: Sha256,
}

impl<W: Write> HashingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
        }
    }

    /// The inner writer and the lowercase hex digest of what reached it.
    fn finish(self) -> (W, String) {
        (self.inner, crate::auth::hex_encode(&self.hasher.finalize()))
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.hasher.update(&buf[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Refuse to publish a dump onto a destination another user could have
/// prepared: a symlink or hard link at the target, a symlinked parent, or a
/// group/world-writable parent without the sticky bit.
#[cfg_attr(
    not(unix),
    expect(clippy::unnecessary_wraps, reason = "fallible on Unix")
)]
fn validate_dump_destination(out_path: &Path) -> Result<(), LificError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let parent = out_path.parent().unwrap_or_else(|| Path::new("."));
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        let parent_metadata = std::fs::symlink_metadata(parent)
            .map_err(|e| LificError::Internal(format!("inspect dump destination: {e}")))?;
        if parent_metadata.file_type().is_symlink() {
            return Err(LificError::Internal(
                "dump destination parent is a symlink".into(),
            ));
        }
        let mode = parent_metadata.mode();
        if mode & 0o1000 == 0 && mode & 0o022 != 0 {
            return Err(LificError::Internal(
                "dump destination parent is group/world-writable without sticky protection".into(),
            ));
        }
        if let Ok(metadata) = std::fs::symlink_metadata(out_path)
            && (metadata.file_type().is_symlink() || metadata.nlink() > 1)
        {
            return Err(LificError::Internal(
                "dump destination must not be a symlink or hard link".into(),
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = out_path;
    Ok(())
}

/// Append raw bytes as a tar entry with the given name.
fn append_bytes<W: std::io::Write>(
    tar: &mut tar::Builder<W>,
    name: &str,
    bytes: &[u8],
) -> Result<(), LificError> {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o600);
    header.set_mtime(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    );
    header.set_cksum();
    tar.append_data(&mut header, name, bytes)
        .map_err(|e| LificError::Internal(format!("append {name}: {e}")))
}

/// Read the highest applied migration version from the DB (0 if unavailable).
fn schema_version(pool: &DbPool) -> i64 {
    pool.read()
        .ok()
        .and_then(|conn| {
            conn.query_row(
                "SELECT COALESCE(MAX(version), 0) FROM _migrations",
                [],
                |row| row.get::<_, i64>(0),
            )
            .ok()
        })
        .unwrap_or(0)
}

// ── dump (CLI) ───────────────────────────────────────────────

/// Result of a `lific dump`, for printing/JSON output.
pub struct DumpResult {
    pub archive_path: PathBuf,
    pub manifest: Manifest,
}

/// Run `lific dump`: resolve the output path (file or directory), snapshot the
/// DB at `db_path`, and write the archive. `out` may be `None` (current dir), a
/// directory (default filename inside it), or a full target file path.
pub fn run_dump(db_path: &Path, out: Option<&Path>) -> Result<DumpResult, LificError> {
    let pool = crate::db::open(db_path)?;

    let db_stem = db_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("lific");
    let default_name = archive_filename(db_stem, &archive_timestamp());

    let archive_path = match out {
        None => std::env::current_dir()
            .map_err(|e| LificError::Internal(format!("resolve current dir: {e}")))?
            .join(&default_name),
        Some(p) => {
            // A path that exists as a dir, or ends with a separator, is a
            // target directory → use the default filename inside it.
            if p.is_dir() {
                p.join(&default_name)
            } else {
                p.to_path_buf()
            }
        }
    };

    if let Some(parent) = archive_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| LificError::Internal(format!("create output dir: {e}")))?;
    }

    let manifest = write_dump(&pool, db_path, &archive_path)?;
    Ok(DumpResult {
        archive_path,
        manifest,
    })
}

// ── restore (CLI) ────────────────────────────────────────────

/// Summary returned by a successful restore for printing/JSON.
#[derive(Debug)]
pub struct RestoreResult {
    pub manifest: Manifest,
    pub attachment_count: u64,
    pub archived_project_count: u64,
    pub db_path: PathBuf,
    /// Where the pre-existing DB was moved, if `--force` displaced one.
    pub moved_existing_to: Option<PathBuf>,
}

/// Validate that an attachment entry name is a bare hash under `attachments/`
/// with no path traversal. Returns the bare filename on success.
fn validate_attachment_entry(name: &str) -> Result<String, LificError> {
    let rest = name
        .strip_prefix(ARCHIVE_ATTACHMENTS_PREFIX)
        .ok_or_else(|| LificError::BadRequest(format!("unexpected archive entry: {name}")))?;
    // Reject empty, nested paths, parent refs, absolute paths, or anything with
    // a separator — a blob name is a bare sha256 hex string.
    if rest.is_empty()
        || rest.contains('/')
        || rest.contains('\\')
        || rest.contains("..")
        || rest.starts_with('.')
    {
        return Err(LificError::BadRequest(format!(
            "rejected attachment entry (path traversal or invalid name): {name}"
        )));
    }
    if !crate::storage::valid_sha256(rest) {
        return Err(LificError::BadRequest(format!(
            "rejected attachment entry (expected lowercase SHA-256): {name}"
        )));
    }
    Ok(rest.to_string())
}

/// Validate that an entry is a single archived-project filename under
/// `archived-projects/` (see [`valid_archived_project_name`]). Returns the
/// bare filename on success.
fn validate_archived_project_entry(name: &str) -> Result<String, LificError> {
    let rest = name
        .strip_prefix(ARCHIVE_ARCHIVED_PROJECTS_PREFIX)
        .ok_or_else(|| LificError::BadRequest(format!("unexpected archive entry: {name}")))?;
    if !valid_archived_project_name(rest) {
        return Err(LificError::BadRequest(format!(
            "rejected archived project entry (path traversal or invalid name): {name}"
        )));
    }
    Ok(rest.to_string())
}

fn validate_manifest_limits(manifest: &Manifest, limits: &RestoreLimits) -> Result<(), LificError> {
    if manifest.db_size_bytes > limits.max_db_bytes {
        return Err(LificError::BadRequest(format!(
            "archive database exceeds restore limit ({} > {} bytes)",
            manifest.db_size_bytes, limits.max_db_bytes
        )));
    }
    if manifest.attachment_count > limits.max_entries {
        return Err(LificError::BadRequest(format!(
            "archive has too many attachments ({} > {})",
            manifest.attachment_count, limits.max_entries
        )));
    }
    let entries = manifest
        .attachment_count
        .checked_add(manifest.archived_project_count);
    if entries.is_none_or(|entries| entries > limits.max_entries) {
        return Err(LificError::BadRequest(format!(
            "archive has too many attachments and archived projects (limit {})",
            limits.max_entries
        )));
    }
    let total = manifest
        .db_size_bytes
        .checked_add(manifest.attachment_bytes)
        .and_then(|sum| sum.checked_add(manifest.archived_project_bytes));
    if total.is_none_or(|total| total > limits.max_total_bytes) {
        return Err(LificError::BadRequest(
            "archive contents exceed total restore size limit".into(),
        ));
    }
    Ok(())
}

fn read_entry_bounded<R: Read>(
    entry: &mut tar::Entry<'_, R>,
    max_bytes: u64,
    label: &str,
) -> Result<Vec<u8>, LificError> {
    let size = entry.size();
    if size > max_bytes {
        return Err(LificError::BadRequest(format!(
            "{label} exceeds restore limit ({size} > {max_bytes} bytes)"
        )));
    }
    let mut buf = Vec::with_capacity(size as usize);
    entry
        .read_to_end(&mut buf)
        .map_err(|e| LificError::BadRequest(format!("read {label}: {e}")))?;
    Ok(buf)
}

fn bounded_archive_with_limit(
    file: std::fs::File,
    max_decompressed_bytes: u64,
) -> tar::Archive<std::io::Take<flate2::read::GzDecoder<std::fs::File>>> {
    let decoder = flate2::read::GzDecoder::new(file);
    tar::Archive::new(decoder.take(max_decompressed_bytes))
}

fn bounded_archive(
    file: std::fs::File,
    limits: &RestoreLimits,
) -> tar::Archive<std::io::Take<flate2::read::GzDecoder<std::fs::File>>> {
    bounded_archive_with_limit(file, limits.max_decompressed_bytes())
}

fn validate_tar_entry_type<R: Read>(entry: &tar::Entry<'_, R>) -> Result<(), LificError> {
    if !entry.header().entry_type().is_file() {
        return Err(LificError::BadRequest(
            "archive contains an unsupported tar entry type".into(),
        ));
    }
    Ok(())
}

fn copy_entry_bounded<R: Read, W: Write>(
    entry: &mut tar::Entry<'_, R>,
    output: &mut W,
    max_bytes: u64,
    total_bytes: &mut u64,
    total_limit: u64,
    label: &str,
) -> Result<u64, LificError> {
    let size = entry.size();
    if size > max_bytes {
        return Err(LificError::BadRequest(format!(
            "{label} exceeds restore limit ({size} > {max_bytes} bytes)"
        )));
    }
    let new_total = total_bytes
        .checked_add(size)
        .ok_or_else(|| LificError::BadRequest("archive size overflow".into()))?;
    if new_total > total_limit {
        return Err(LificError::BadRequest(
            "archive contents exceed total restore size limit".into(),
        ));
    }

    let copied = std::io::copy(entry, output)
        .map_err(|e| LificError::BadRequest(format!("read {label}: {e}")))?;
    if copied != size {
        return Err(LificError::BadRequest(format!(
            "read {label}: expected {size} bytes, got {copied}"
        )));
    }
    *total_bytes = new_total;
    Ok(copied)
}

fn hash_file(path: &Path) -> Result<String, LificError> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| LificError::BadRequest(format!("open attachment: {e}")))?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 8192];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| LificError::BadRequest(format!("read attachment: {e}")))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(crate::auth::hex_encode(&digest.finalize()))
}

fn validate_attachment_schema(conn: &rusqlite::Connection) -> Result<(), LificError> {
    let sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'attachments'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| LificError::BadRequest(format!("read staged attachment schema: {e}")))?;
    let probe = rusqlite::Connection::open_in_memory()
        .map_err(|e| LificError::BadRequest(format!("create staged schema probe: {e}")))?;
    probe
        .execute_batch("CREATE TABLE users (id INTEGER PRIMARY KEY);")
        .map_err(|e| LificError::BadRequest(format!("create staged schema probe: {e}")))?;
    probe
        .execute_batch(&sql)
        .map_err(|e| LificError::BadRequest(format!("create staged schema probe: {e}")))?;

    let insert = |sha256: &str, mime: &str, size_bytes: i64| {
        probe.execute(
            "INSERT INTO attachments (sha256, filename, mime, size_bytes)
             VALUES (?1, 'schema-probe', ?2, ?3)",
            rusqlite::params![sha256, mime, size_bytes],
        )
    };
    let valid_sha = crate::storage::AttachmentStore::hash_bytes(b"schema-probe");
    let invalid_mime_sha = "b".repeat(64);
    let invalid_size_sha = "c".repeat(64);
    insert(&valid_sha, "text/plain", 0).map_err(|e| {
        LificError::BadRequest(format!(
            "staged attachment schema rejects valid metadata: {e}"
        ))
    })?;

    for (sha256, mime, size_bytes) in [
        ("../outside", "text/plain", 0),
        (invalid_mime_sha.as_str(), "application/octet-stream", 0),
        (invalid_size_sha.as_str(), "text/plain", -1),
    ] {
        if insert(sha256, mime, size_bytes).is_ok() {
            return Err(LificError::BadRequest(
                "staged attachment table is missing required integrity constraints".into(),
            ));
        }
    }
    Ok(())
}

/// Serializes restores against one data directory.
///
/// The lock lives in the kernel (an fs2 advisory exclusive lock), not in the
/// existence of a path. A crashed restore therefore leaves at most a zero-byte
/// `.lific-restore.lock` file that the next restore reuses immediately, while
/// a restore running *right now* still rejects a second one. Owning a
/// directory instead, as the first cut of this did, made a crash permanently
/// wedge the data dir until someone found and deleted the leftover by hand.
struct RestoreLock {
    file: File,
}

impl RestoreLock {
    fn acquire(data_dir: &Path) -> Result<Self, LificError> {
        let path = data_dir.join(".lific-restore.lock");
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // No-follow: the lock file must not be a symlink someone planted
            // to have us open (and later truncate-free-write) a file elsewhere.
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options
            .open(&path)
            .map_err(|error| LificError::Internal(format!("create restore lock: {error}")))?;
        file.try_lock_exclusive().map_err(|_| {
            LificError::Conflict(
                "another restore is already running for this data directory".into(),
            )
        })?;
        Ok(Self { file })
    }
}

impl Drop for RestoreLock {
    fn drop(&mut self) {
        // Releasing the lock is what matters; the (empty) file is left in place
        // deliberately, since unlinking it races another process that already
        // has it open.
        let _ = FileExt::unlock(&self.file);
    }
}

/// Create a staging file this process exclusively owns: never following a
/// symlink, never adopting an existing file, owner-only from creation.
fn create_staging_file(path: &Path) -> Result<File, LificError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options
        .open(path)
        .map_err(|e| LificError::Internal(format!("create staging file: {e}")))
}

/// Validate the extracted SQLite file before it can replace the live DB. This
/// catches corrupt archives and ensures every metadata attachment reference is
/// a safe content-addressed filename with matching staged bytes.
fn validate_staged_database(
    staging: &Path,
    manifest: &Manifest,
    limits: &RestoreLimits,
) -> Result<(), LificError> {
    let db = staging.join(ARCHIVE_DB_NAME);
    let conn =
        rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| LificError::BadRequest(format!("open staged database: {e}")))?;
    let check: String = conn
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|e| LificError::BadRequest(format!("validate staged database: {e}")))?;
    if check != "ok" {
        return Err(LificError::BadRequest(format!(
            "staged database integrity check failed: {check}"
        )));
    }

    let schema_version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM _migrations",
            [],
            |row| row.get(0),
        )
        .map_err(|e| LificError::BadRequest(format!("read staged schema version: {e}")))?;
    if schema_version != manifest.schema_version {
        return Err(LificError::BadRequest(format!(
            "staged database schema version {schema_version} does not match manifest {}",
            manifest.schema_version
        )));
    }
    if schema_version > crate::db::migrate::latest_version() {
        return Err(LificError::BadRequest(format!(
            "staged database schema version {schema_version} is newer than this binary"
        )));
    }

    let has_attachments: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'attachments')",
            [],
            |row| row.get(0),
        )
        .map_err(|e| LificError::BadRequest(format!("inspect staged schema: {e}")))?;
    if !has_attachments
        && (schema_version >= ATTACHMENTS_SCHEMA_VERSION
            || manifest.attachment_count != 0
            || manifest.attachment_bytes != 0)
    {
        return Err(LificError::BadRequest(
            "staged database is missing the attachments table".into(),
        ));
    }

    if has_attachments && schema_version < ATTACHMENTS_SCHEMA_VERSION {
        return Err(LificError::BadRequest(
            "staged database has attachment tables at an unsupported schema version".into(),
        ));
    }

    if has_attachments && schema_version >= ATTACHMENT_INTEGRITY_SCHEMA_VERSION {
        validate_attachment_schema(&conn)?;
    }
    if has_attachments {
        let mime_placeholders = vec!["?"; crate::storage::ALLOWED_MIMES.len()].join(", ");
        let mut stmt = conn
            .prepare(&format!(
                "SELECT sha256, MIN(size_bytes), MAX(size_bytes),
                        SUM(CASE WHEN mime IN ({mime_placeholders}) THEN 0 ELSE 1 END)
                 FROM attachments
                 GROUP BY sha256"
            ))
            .map_err(|e| LificError::BadRequest(format!("read staged attachment metadata: {e}")))?;
        let rows = stmt
            .query_map(
                rusqlite::params_from_iter(crate::storage::ALLOWED_MIMES.iter().copied()),
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .map_err(|e| LificError::BadRequest(format!("read staged attachment metadata: {e}")))?;
        for row in rows {
            let (sha, min_size, max_size, invalid_mimes) = row.map_err(|e| {
                LificError::BadRequest(format!("read staged attachment metadata: {e}"))
            })?;
            if !crate::storage::valid_sha256(&sha)
                || min_size < 0
                || min_size != max_size
                || invalid_mimes != 0
            {
                return Err(LificError::BadRequest(
                    "staged attachment metadata has an invalid content address, MIME, or size"
                        .into(),
                ));
            }
            let size = u64::try_from(max_size).map_err(|_| {
                LificError::BadRequest(
                    "staged attachment metadata has an invalid content address, MIME, or size"
                        .into(),
                )
            })?;
            if size > limits.max_attachment_bytes {
                return Err(LificError::BadRequest(
                    "staged attachment metadata has an invalid content address, MIME, or size"
                        .into(),
                ));
            }
            let path = staging.join("attachments").join(&sha);
            let metadata = std::fs::metadata(&path).map_err(|_| {
                LificError::BadRequest(format!(
                    "staged database references missing attachment {sha}"
                ))
            })?;
            if !metadata.is_file() || metadata.len() != size {
                return Err(LificError::BadRequest(format!(
                    "staged attachment {sha} does not match database metadata"
                )));
            }
            if hash_file(&path)? != sha {
                return Err(LificError::BadRequest(format!(
                    "staged attachment {sha} does not match its content address"
                )));
            }
        }

        let mut mime_stmt = conn
            .prepare("SELECT DISTINCT sha256, mime FROM attachments")
            .map_err(|e| LificError::BadRequest(format!("read staged attachment MIME: {e}")))?;
        let mime_rows = mime_stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| LificError::BadRequest(format!("read staged attachment MIME: {e}")))?;
        for row in mime_rows {
            let (sha, declared_mime) = row
                .map_err(|e| LificError::BadRequest(format!("read staged attachment MIME: {e}")))?;
            let path = staging.join("attachments").join(&sha);
            // Streamed, not `std::fs::read`: under `--allow-large` a blob can
            // be far bigger than the bounded defaults, and confirming its
            // content type must not cost an attachment-sized allocation.
            let file = std::fs::File::open(&path).map_err(|e| {
                LificError::BadRequest(format!("read staged attachment {sha}: {e}"))
            })?;
            let detected_mime = crate::storage::sniff_and_validate_stream(
                std::io::BufReader::new(file),
                Some(&declared_mime),
            )?;
            if detected_mime != declared_mime {
                return Err(LificError::BadRequest(format!(
                    "staged attachment {sha} MIME {declared_mime} does not match its content ({detected_mime})"
                )));
            }
        }
    }

    let attachments_dir = staging.join("attachments");
    for entry in std::fs::read_dir(&attachments_dir)
        .map_err(|e| LificError::BadRequest(format!("read staged attachments: {e}")))?
    {
        let entry =
            entry.map_err(|e| LificError::BadRequest(format!("read staged attachment: {e}")))?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path).map_err(|e| {
            LificError::BadRequest(format!("inspect staged attachment {}: {e}", path.display()))
        })?;
        if !metadata.is_file() {
            return Err(LificError::BadRequest(format!(
                "staged attachment is not a regular file: {}",
                path.display()
            )));
        }
        let name = entry.file_name().into_string().map_err(|_| {
            LificError::BadRequest(format!(
                "staged attachment has a non-UTF-8 name: {}",
                path.display()
            ))
        })?;
        if !crate::storage::valid_sha256(&name) {
            return Err(LificError::BadRequest(format!(
                "staged attachment has an invalid content address: {name}"
            )));
        }
        if metadata.len() > limits.max_attachment_bytes {
            return Err(LificError::BadRequest(format!(
                "staged attachment exceeds restore limit: {name}"
            )));
        }
        if hash_file(&path)? != name {
            return Err(LificError::BadRequest(format!(
                "staged attachment {name} does not match its content address"
            )));
        }
    }
    Ok(())
}

/// Size and sha256 of one archived-project file, recorded while it was
/// written into the restore staging tree.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StagedFile {
    size: u64,
    digest: String,
}

/// Check every `archived_projects` row in the staged database against the
/// staged `archived-projects/` files, before anything is installed.
///
/// A restore replaces the live directory wholesale, so a row whose file is
/// missing or whose bytes differ would swap a healthy archive for one that can
/// never be unarchived. `staged` maps each staged filename to the size and
/// digest computed while it was written; the staging tree is private to this
/// restore, so it holds exactly those files. Files with no row are accepted:
/// a crash between writing an archive file and committing its row leaves such
/// a stray, and it does no harm. A database from before migration 059 has no
/// table and nothing to check.
fn validate_staged_archived_projects(
    staging: &Path,
    staged: &std::collections::HashMap<String, StagedFile>,
) -> Result<(), LificError> {
    let conn = rusqlite::Connection::open_with_flags(
        staging.join(ARCHIVE_DB_NAME),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| LificError::BadRequest(format!("open staged database: {e}")))?;
    let has_table: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master
                           WHERE type = 'table' AND name = 'archived_projects')",
            [],
            |row| row.get(0),
        )
        .map_err(|e| LificError::BadRequest(format!("inspect staged schema: {e}")))?;
    if !has_table {
        return Ok(());
    }

    let read_error =
        |e: rusqlite::Error| LificError::BadRequest(format!("read staged archived projects: {e}"));
    let mut stmt = conn
        .prepare("SELECT file_name, sha256, size_bytes FROM archived_projects ORDER BY id")
        .map_err(read_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(read_error)?;
    for row in rows {
        let (file_name, sha256, size_bytes) = row.map_err(read_error)?;
        if !valid_archived_project_name(&file_name) {
            return Err(LificError::BadRequest(format!(
                "staged archived project row has an invalid file name: {file_name:?}"
            )));
        }
        let Some(file) = staged.get(&file_name) else {
            return Err(LificError::BadRequest(format!(
                "staged database references missing archived project file {file_name}"
            )));
        };
        // The map is the authority for what was staged; still confirm the
        // file is where the install will look for it.
        let on_disk =
            std::fs::symlink_metadata(staging.join(ARCHIVED_PROJECTS_DIR_NAME).join(&file_name));
        if !on_disk.is_ok_and(|metadata| metadata.is_file() && metadata.len() == file.size) {
            return Err(LificError::BadRequest(format!(
                "staged archived project file {file_name} is not a regular file of the staged size"
            )));
        }
        if u64::try_from(size_bytes).ok() != Some(file.size) {
            return Err(LificError::BadRequest(format!(
                "staged archived project file {file_name} is {} bytes, but its row records {size_bytes}",
                file.size
            )));
        }
        if file.digest != sha256 {
            return Err(LificError::BadRequest(format!(
                "staged archived project file {file_name} does not match the sha256 its row records"
            )));
        }
    }
    Ok(())
}

/// Read and validate an archive's manifest + entry list without extracting.
/// Returns the parsed manifest. Rejects archives missing `manifest.json` or
/// `lific.db`, and any attachment entry that fails [`validate_attachment_entry`].
pub fn inspect_archive(archive: &Path, limits: &RestoreLimits) -> Result<Manifest, LificError> {
    let manifest = read_manifest(archive, limits)?;
    validate_manifest_limits(&manifest, limits)?;

    // Second pass: validate every entry name (traversal guard) and require the
    // DB member is present.
    let file = std::fs::File::open(archive)
        .map_err(|e| LificError::BadRequest(format!("open archive: {e}")))?;
    let mut tar = bounded_archive(file, limits);
    let mut has_db = false;
    let mut db_bytes = None;
    let mut manifest_entries = 0u64;
    let mut attachment_names = std::collections::HashSet::new();
    let mut entry_count = 0u64;
    let mut attachment_count = 0u64;
    let mut attachment_bytes = 0u64;
    let mut archived_project_names = std::collections::HashSet::new();
    let mut archived_project_count = 0u64;
    let mut archived_project_bytes = 0u64;
    let mut payload_bytes = 0u64;
    for entry in tar
        .entries()
        .map_err(|e| LificError::BadRequest(format!("read archive entries: {e}")))?
        .raw(true)
    {
        let entry = entry.map_err(|e| LificError::BadRequest(format!("read entry: {e}")))?;
        validate_tar_entry_type(&entry)?;
        entry_count += 1;
        if entry_count > limits.max_entries + 2 {
            return Err(LificError::BadRequest(
                "archive has too many entries".into(),
            ));
        }
        let size = entry.size();
        let path = entry
            .path()
            .map_err(|e| LificError::BadRequest(format!("entry path: {e}")))?;
        let name = path.to_string_lossy().replace('\\', "/");
        if name == ARCHIVE_MANIFEST_NAME {
            manifest_entries += 1;
            if manifest_entries > 1 {
                return Err(LificError::BadRequest(
                    "archive contains duplicate manifests".into(),
                ));
            }
            continue;
        } else if name == ARCHIVE_DB_NAME {
            if has_db {
                return Err(LificError::BadRequest(
                    "archive contains duplicate databases".into(),
                ));
            }
            if size > limits.max_db_bytes {
                return Err(LificError::BadRequest(
                    "archive database exceeds restore limit".into(),
                ));
            }
            has_db = true;
            db_bytes = Some(size);
            payload_bytes = payload_bytes
                .checked_add(size)
                .ok_or_else(|| LificError::BadRequest("archive size overflow".into()))?;
        } else if name.starts_with(ARCHIVE_ATTACHMENTS_PREFIX) {
            validate_attachment_entry(&name)?;
            if !attachment_names.insert(name.clone()) {
                return Err(LificError::BadRequest(format!(
                    "archive contains duplicate attachment: {name}"
                )));
            }
            if size > limits.max_attachment_bytes {
                return Err(LificError::BadRequest(
                    "archive attachment exceeds restore limit".into(),
                ));
            }
            attachment_count += 1;
            attachment_bytes = attachment_bytes
                .checked_add(size)
                .ok_or_else(|| LificError::BadRequest("archive attachment size overflow".into()))?;
            payload_bytes = payload_bytes
                .checked_add(size)
                .ok_or_else(|| LificError::BadRequest("archive size overflow".into()))?;
            if attachment_count.saturating_add(archived_project_count) > limits.max_entries
                || attachment_bytes > manifest.attachment_bytes
                || payload_bytes > limits.max_total_bytes
            {
                return Err(LificError::BadRequest(
                    "archive attachment contents exceed manifest limits".into(),
                ));
            }
        } else if name.starts_with(ARCHIVE_ARCHIVED_PROJECTS_PREFIX) {
            validate_archived_project_entry(&name)?;
            if !archived_project_names.insert(name.clone()) {
                return Err(LificError::BadRequest(format!(
                    "archive contains duplicate archived project: {name}"
                )));
            }
            if size > limits.max_archived_project_bytes {
                return Err(LificError::BadRequest(
                    "archived project exceeds restore limit".into(),
                ));
            }
            archived_project_count += 1;
            archived_project_bytes = archived_project_bytes
                .checked_add(size)
                .ok_or_else(|| LificError::BadRequest("archived project size overflow".into()))?;
            payload_bytes = payload_bytes
                .checked_add(size)
                .ok_or_else(|| LificError::BadRequest("archive size overflow".into()))?;
            if attachment_count.saturating_add(archived_project_count) > limits.max_entries
                || archived_project_bytes > manifest.archived_project_bytes
                || payload_bytes > limits.max_total_bytes
            {
                return Err(LificError::BadRequest(
                    "archived project contents exceed manifest limits".into(),
                ));
            }
        } else {
            return Err(LificError::BadRequest(format!(
                "rejected unexpected archive entry: {name}"
            )));
        }
    }
    if archived_project_count != manifest.archived_project_count
        || archived_project_bytes != manifest.archived_project_bytes
    {
        return Err(LificError::BadRequest(
            "archive archived-project entries do not match manifest".into(),
        ));
    }
    if !has_db {
        return Err(LificError::BadRequest("archive is missing lific.db".into()));
    }
    if db_bytes != Some(manifest.db_size_bytes) {
        return Err(LificError::BadRequest(
            "archive database size does not match manifest".into(),
        ));
    }
    if attachment_count != manifest.attachment_count
        || attachment_bytes != manifest.attachment_bytes
    {
        return Err(LificError::BadRequest(
            "archive attachment entries do not match manifest".into(),
        ));
    }
    Ok(manifest)
}

/// Read just the manifest from the archive (first matching entry).
fn read_manifest(archive: &Path, limits: &RestoreLimits) -> Result<Manifest, LificError> {
    let file = std::fs::File::open(archive)
        .map_err(|e| LificError::BadRequest(format!("open archive: {e}")))?;
    let mut tar = bounded_archive(file, limits);
    let mut entry_count = 0u64;
    for entry in tar
        .entries()
        .map_err(|e| LificError::BadRequest(format!("read archive entries: {e}")))?
        .raw(true)
    {
        let mut entry = entry.map_err(|e| LificError::BadRequest(format!("read entry: {e}")))?;
        validate_tar_entry_type(&entry)?;
        entry_count += 1;
        if entry_count > limits.max_entries + 2 {
            return Err(LificError::BadRequest(
                "archive has too many entries before its manifest".into(),
            ));
        }
        let path = entry
            .path()
            .map_err(|e| LificError::BadRequest(format!("entry path: {e}")))?;
        if path.to_string_lossy() == ARCHIVE_MANIFEST_NAME {
            let bytes = read_entry_bounded(&mut entry, limits.max_manifest_bytes, "manifest")?;
            let buf = String::from_utf8(bytes)
                .map_err(|e| LificError::BadRequest(format!("manifest is not UTF-8: {e}")))?;
            return serde_json::from_str(&buf)
                .map_err(|e| LificError::BadRequest(format!("parse manifest.json: {e}")));
        }
    }
    Err(LificError::BadRequest(
        "archive is missing manifest.json".into(),
    ))
}

/// Whether a hot WAL sidecar is present next to `db_path`, hinting the server
/// may still be running (best-effort — see command help).
fn wal_is_hot(db_path: &Path) -> bool {
    let wal = PathBuf::from(format!("{}-wal", db_path.display()));
    std::fs::metadata(&wal).is_ok_and(|m| m.len() > 0)
}

/// Run `lific restore`: validate the archive, then stage-extract it into the
/// data dir at `db_path`. Refuses to clobber an existing DB unless `force`;
/// with `force`, moves the existing DB + `-wal`/`-shm` aside. Refuses archives
/// created by a newer Lific (higher schema_version than this binary).
// The bounded-default entry point kept for callers with no options to express
// (the tests, and anything embedding a restore); `lific restore` itself goes
// through `run_restore_with` so it can pass `--allow-large`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn run_restore(
    archive: &Path,
    db_path: &Path,
    force: bool,
) -> Result<RestoreResult, LificError> {
    run_restore_with(archive, db_path, &RestoreOptions::new(force, false))
}

/// [`run_restore`] with explicit options, including the size bounds applied to
/// the archive (`lific restore --allow-large` raises them).
pub fn run_restore_with(
    archive: &Path,
    db_path: &Path,
    options: &RestoreOptions,
) -> Result<RestoreResult, LificError> {
    let force = options.force;
    let limits = &options.limits;
    let manifest = inspect_archive(archive, limits)?;

    // Schema compatibility gate.
    let latest = crate::db::migrate::latest_version();
    if manifest.schema_version > latest {
        return Err(LificError::BadRequest(format!(
            "archive was created by a newer Lific (schema v{} > this binary's v{}); \
             upgrade Lific before restoring",
            manifest.schema_version, latest
        )));
    }

    let data_dir = db_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), |p| p.to_path_buf());
    std::fs::create_dir_all(&data_dir)
        .map_err(|e| LificError::Internal(format!("create data dir: {e}")))?;

    let _restore_lock = RestoreLock::acquire(&data_dir)?;

    if db_path.exists() && !force {
        return Err(LificError::Conflict(format!(
            "{} already exists; pass --force to restore over it (stop the server first)",
            db_path.display()
        )));
    }

    // Stage and validate the complete restore before moving any live state.
    let staging = tempfile::Builder::new()
        .prefix(".lific-restore-")
        .tempdir_in(&data_dir)
        .map_err(|e| LificError::Internal(format!("create staging dir: {e}")))?;
    let staging_path = staging.path();
    std::fs::create_dir(staging_path.join("attachments"))
        .map_err(|e| LificError::Internal(format!("create staging dir: {e}")))?;
    set_owner_only_dir(staging_path)
        .map_err(|e| LificError::Internal(format!("secure restore staging dir: {e}")))?;
    set_owner_only_dir(&staging_path.join("attachments"))
        .map_err(|e| LificError::Internal(format!("secure restore attachments dir: {e}")))?;
    // Always staged, even when the dump carries no archived projects: the
    // install replaces the live directory wholesale, so restoring an older
    // dump leaves an empty directory rather than files no restored row tracks.
    let staged_archived = staging_path.join(ARCHIVED_PROJECTS_DIR_NAME);
    std::fs::create_dir(&staged_archived)
        .map_err(|e| LificError::Internal(format!("create staging dir: {e}")))?;
    set_owner_only_dir(&staged_archived)
        .map_err(|e| LificError::Internal(format!("secure restore archived projects dir: {e}")))?;

    let extract = (|| -> Result<(u64, u64), LificError> {
        let file = std::fs::File::open(archive)
            .map_err(|e| LificError::BadRequest(format!("open archive: {e}")))?;
        let mut tar = bounded_archive(file, limits);
        let mut attachment_count = 0u64;
        let mut archived_project_count = 0u64;
        let mut staged_archived_digests = std::collections::HashMap::new();
        let mut total_bytes = 0u64;
        for entry in tar
            .entries()
            .map_err(|e| LificError::BadRequest(format!("read archive entries: {e}")))?
            .raw(true)
        {
            let mut entry =
                entry.map_err(|e| LificError::BadRequest(format!("read entry: {e}")))?;
            validate_tar_entry_type(&entry)?;
            let epath = entry
                .path()
                .map_err(|e| LificError::BadRequest(format!("entry path: {e}")))?;
            let name = epath.to_string_lossy().replace('\\', "/");
            if name == ARCHIVE_MANIFEST_NAME {
                // Keep the manifest in staging for validation; return it to the
                // caller in RestoreResult after the install succeeds.
                let buf = read_entry_bounded(&mut entry, limits.max_manifest_bytes, "manifest")?;
                let mut output = create_staging_file(&staging_path.join(ARCHIVE_MANIFEST_NAME))?;
                output
                    .write_all(&buf)
                    .map_err(|e| LificError::Internal(format!("write manifest: {e}")))?;
                output
                    .sync_all()
                    .map_err(|e| LificError::Internal(format!("sync manifest: {e}")))?;
            } else if name == ARCHIVE_DB_NAME {
                let db = staging_path.join(ARCHIVE_DB_NAME);
                let mut output = create_staging_file(&db)?;
                copy_entry_bounded(
                    &mut entry,
                    &mut output,
                    limits.max_db_bytes,
                    &mut total_bytes,
                    limits.max_total_bytes,
                    "database",
                )?;
                set_owner_only(&db)
                    .map_err(|e| LificError::Internal(format!("chmod staged db: {e}")))?;
                output
                    .sync_all()
                    .map_err(|e| LificError::Internal(format!("sync staged db: {e}")))?;
            } else if name.starts_with(ARCHIVE_ATTACHMENTS_PREFIX) {
                let bare = validate_attachment_entry(&name)?;
                let path = staging_path.join("attachments").join(&bare);
                let mut output = create_staging_file(&path)?;
                copy_entry_bounded(
                    &mut entry,
                    &mut output,
                    limits.max_attachment_bytes,
                    &mut total_bytes,
                    limits.max_total_bytes,
                    "attachment",
                )?;
                set_owner_only(&path)
                    .map_err(|e| LificError::Internal(format!("chmod staged attachment: {e}")))?;
                output
                    .sync_all()
                    .map_err(|e| LificError::Internal(format!("sync staged attachment: {e}")))?;
                attachment_count += 1;
            } else if name.starts_with(ARCHIVE_ARCHIVED_PROJECTS_PREFIX) {
                let bare = validate_archived_project_entry(&name)?;
                let path = staged_archived.join(&bare);
                // Hash while staging, so validating the staged database
                // against these bytes does not have to read them again.
                let mut hashing = HashingWriter::new(create_staging_file(&path)?);
                let size = copy_entry_bounded(
                    &mut entry,
                    &mut hashing,
                    limits.max_archived_project_bytes,
                    &mut total_bytes,
                    limits.max_total_bytes,
                    "archived project",
                )?;
                let (output, digest) = hashing.finish();
                staged_archived_digests.insert(bare, StagedFile { size, digest });
                set_owner_only_file(&output).map_err(|e| {
                    LificError::Internal(format!("chmod staged archived project: {e}"))
                })?;
                output.sync_all().map_err(|e| {
                    LificError::Internal(format!("sync staged archived project: {e}"))
                })?;
                archived_project_count += 1;
            } else {
                return Err(LificError::BadRequest(format!(
                    "rejected unexpected archive entry: {name}"
                )));
            }
        }
        if !staging_path.join(ARCHIVE_DB_NAME).exists() {
            return Err(LificError::BadRequest("archive is missing lific.db".into()));
        }
        // `inspect_archive` already matched the entries against the manifest;
        // recheck what was actually staged, since the archive was reopened.
        if archived_project_count != manifest.archived_project_count {
            return Err(LificError::BadRequest(
                "archive archived-project entries do not match manifest".into(),
            ));
        }
        sync_dir(&staged_archived)
            .map_err(|e| LificError::Internal(format!("sync staged archived projects: {e}")))?;
        validate_staged_database(staging_path, &manifest, limits)?;
        validate_staged_archived_projects(staging_path, &staged_archived_digests)?;
        Ok((attachment_count, archived_project_count))
    })();

    let (attachment_count, archived_project_count) = extract?;

    // Recheck after staging in case another process created the destination
    // while the archive was being validated.
    if db_path.exists() && !force {
        return Err(LificError::Conflict(format!(
            "{} already exists; pass --force to restore over it (stop the server first)",
            db_path.display()
        )));
    }

    let restore_id = staging
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("restore")
        .to_string();

    // The install phase swaps the live database and the whole attachments and
    // archived-projects directories. A dump, upload, delete, GC sweep,
    // archive or unarchive crossing that swap would be reading one half of the
    // old data set and one half of the new one, so it runs under the
    // attachment store's lock like every other operation that moves blobs or
    // archived-project files. The lock file is a sibling of `attachments/`, not
    // inside it, so it is the same lock before and after the directory is
    // replaced. Store lock first, then the database work inside: the ordering
    // every other caller uses.
    let store = crate::storage::AttachmentStore::from_db_path(db_path);
    let moved_existing_to =
        store.with_lock(|_| install_restore(staging_path, db_path, &restore_id))?;

    Ok(RestoreResult {
        manifest,
        attachment_count,
        archived_project_count,
        db_path: db_path.to_path_buf(),
        moved_existing_to,
    })
}

/// Move the validated staging tree into place, displacing any live state.
///
/// This is the only part of a restore that touches the user's data, and every
/// failure inside it must leave that data where the user expects to find it.
/// Once the live database has been renamed aside, *every* remaining error path
/// goes through [`fail_after_move`], which renames it back — including the
/// "an attachment backup with this name already exists" refusal, which used to
/// return straight to the caller and leave `db_path` missing entirely.
///
/// Returns where an existing database was moved, if one was.
fn install_restore(
    staging_path: &Path,
    db_path: &Path,
    restore_id: &str,
) -> Result<Option<PathBuf>, LificError> {
    let moved_existing_to = if db_path.exists() {
        checkpoint_db_file(db_path)?;
        let dest = PathBuf::from(format!("{}.pre-restore-{restore_id}", db_path.display()));
        if std::fs::symlink_metadata(&dest).is_ok() {
            return Err(LificError::Conflict(format!(
                "restore backup already exists: {}",
                dest.display()
            )));
        }
        if let Err(error) = std::fs::rename(db_path, &dest) {
            return Err(LificError::Internal(format!(
                "move existing db aside: {error}"
            )));
        }
        // From here on the live database is at `dest`, not `db_path`.
        for ext in ["-wal", "-shm"] {
            let side = PathBuf::from(format!("{}{ext}", db_path.display()));
            if let Err(error) = remove_file_if_present(&side) {
                let cause = LificError::Internal(format!("remove old {ext}: {error}"));
                return Err(rollback_moved_db(&dest, db_path, cause));
            }
        }
        Some(dest)
    } else {
        None
    };
    let moved = moved_existing_to.clone();
    let fail = move |cause: LificError| fail_after_move(moved.as_deref(), db_path, cause);

    // Move restored files into place as one recoverable transaction. Keep the
    // old sidecar directories until the DB and every new directory are live so
    // a filesystem failure cannot leave mismatched metadata and files. Both
    // directories are replaced wholesale: an archived-project file the restored
    // database has no row for would be unreachable and never cleaned up.
    let mut sidecars = [
        SidecarSwap::new(
            attachments_dir_for(db_path),
            "attachments",
            "attachment",
            restore_id,
        ),
        SidecarSwap::new(
            archived_projects_dir_for(db_path),
            ARCHIVED_PROJECTS_DIR_NAME,
            "archived project",
            restore_id,
        ),
    ];
    // Refuse before moving any directory, so this path only has the database
    // to put back.
    for sidecar in &sidecars {
        if std::fs::symlink_metadata(&sidecar.live).is_ok()
            && std::fs::symlink_metadata(&sidecar.aside).is_ok()
        {
            return Err(fail(LificError::Conflict(format!(
                "restore {} backup already exists: {}",
                sidecar.label,
                sidecar.aside.display()
            ))));
        }
    }

    let mut db_installed = false;
    let install_result = (|| -> Result<(), LificError> {
        for sidecar in &mut sidecars {
            if std::fs::symlink_metadata(&sidecar.live).is_ok() {
                std::fs::rename(&sidecar.live, &sidecar.aside).map_err(|e| {
                    LificError::Internal(format!("move existing {} aside: {e}", sidecar.dir_name))
                })?;
                sidecar.moved_aside = true;
            }
        }
        std::fs::rename(staging_path.join(ARCHIVE_DB_NAME), db_path)
            .map_err(|e| LificError::Internal(format!("install restored db: {e}")))?;
        db_installed = true;
        set_owner_only(db_path)
            .map_err(|e| LificError::Internal(format!("chmod restored db: {e}")))?;
        for sidecar in &mut sidecars {
            std::fs::rename(staging_path.join(sidecar.dir_name), &sidecar.live).map_err(|e| {
                LificError::Internal(format!("install restored {}: {e}", sidecar.dir_name))
            })?;
            sidecar.installed = true;
        }
        let data_dir = db_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        sync_dir(data_dir)
            .map_err(|e| LificError::Internal(format!("sync restored data directory: {e}")))?;
        Ok(())
    })();

    if let Err(error) = install_result {
        return Err(rollback_install(
            error,
            staging_path,
            db_path,
            db_installed,
            &sidecars,
            moved_existing_to.as_deref(),
        ));
    }

    for sidecar in &sidecars {
        if sidecar.moved_aside {
            let _ = std::fs::remove_dir_all(&sidecar.aside);
        }
    }

    Ok(moved_existing_to)
}

/// One sidecar directory beside the database that a restore swaps wholesale,
/// with enough state for [`rollback_install`] to undo exactly what happened.
struct SidecarSwap {
    /// Directory name inside the staging tree.
    dir_name: &'static str,
    /// Singular noun for messages.
    label: &'static str,
    /// The live directory in the data dir.
    live: PathBuf,
    /// Where the previous live directory is kept until the install succeeds.
    aside: PathBuf,
    /// The previous live directory now sits at `aside`.
    moved_aside: bool,
    /// The staged directory now sits at `live`.
    installed: bool,
}

impl SidecarSwap {
    fn new(live: PathBuf, dir_name: &'static str, label: &'static str, restore_id: &str) -> Self {
        let aside = PathBuf::from(format!("{}.pre-restore-{restore_id}", live.display()));
        Self {
            dir_name,
            label,
            live,
            aside,
            moved_aside: false,
            installed: false,
        }
    }
}

/// Surface `cause`, first putting a moved-aside database back if there is one.
fn fail_after_move(moved: Option<&Path>, db_path: &Path, cause: LificError) -> LificError {
    match moved {
        Some(moved) => rollback_moved_db(moved, db_path, cause),
        None => cause,
    }
}

fn remove_file_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn remove_dir_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Undo a partial install: take out whatever was installed, put back every
/// directory and the database that were moved aside, and drop the staging
/// tree. Only steps that actually happened are reversed, so a failure while
/// moving the second directory aside never deletes that directory.
fn rollback_install(
    cause: LificError,
    staging: &Path,
    db_path: &Path,
    db_installed: bool,
    sidecars: &[SidecarSwap],
    moved_existing_to: Option<&Path>,
) -> LificError {
    let mut failures = Vec::new();
    if db_installed && let Err(error) = remove_file_if_present(db_path) {
        failures.push(format!("remove installed database: {error}"));
    }
    for sidecar in sidecars.iter().rev() {
        if sidecar.installed
            && let Err(error) = remove_dir_if_present(&sidecar.live)
        {
            failures.push(format!("remove installed {}: {error}", sidecar.dir_name));
        }
        if sidecar.moved_aside
            && let Err(error) = std::fs::rename(&sidecar.aside, &sidecar.live)
        {
            failures.push(format!("restore previous {}: {error}", sidecar.dir_name));
        }
    }
    if let Some(moved) = moved_existing_to
        && let Err(error) = std::fs::rename(moved, db_path)
    {
        failures.push(format!(
            "restore previous database from {}: {error}",
            moved.display()
        ));
    }
    if let Err(error) = remove_dir_if_present(staging) {
        failures.push(format!("remove staging directory: {error}"));
    }

    if failures.is_empty() {
        cause
    } else {
        LificError::Internal(format!(
            "restore failed ({cause}); rollback also failed: {}",
            failures.join("; ")
        ))
    }
}

/// Put the database that `--force` moved aside back at `db_path` after a failed
/// restore, and decide which error the caller should surface.
///
/// On success the original failure (`cause`) is returned unchanged. If the
/// rollback rename itself fails, swallowing it would tell the user only that
/// the restore failed while their database sits at a path they were never
/// shown, reading exactly like data loss. So that case returns a combined
/// error carrying both failures and the exact path the original
/// database still occupies, plus where to move it back to.
fn rollback_moved_db(moved: &Path, db_path: &Path, cause: LificError) -> LificError {
    match std::fs::rename(moved, db_path) {
        Ok(()) => cause,
        Err(rollback_err) => LificError::Internal(format!(
            "restore failed ({cause}), and rolling the previous database back to {} failed too \
             ({rollback_err}). Your original database was NOT deleted: it is still at {}. \
             Move it back to {} by hand to recover it.",
            db_path.display(),
            moved.display(),
            db_path.display()
        )),
    }
}

/// Checkpoint a database file's WAL into the main file, so the `.db` is
/// self-contained (used before moving an existing db aside under `--force`).
/// Failing closed is important: deleting the sidecars after a failed
/// checkpoint could discard committed WAL-backed data.
fn checkpoint_db_file(db_path: &Path) -> Result<(), LificError> {
    let conn = rusqlite::Connection::open(db_path)
        .map_err(|e| LificError::Internal(format!("open database for WAL checkpoint: {e}")))?;
    let busy: i64 = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
        .map_err(|e| LificError::Internal(format!("checkpoint database WAL: {e}")))?;
    if busy != 0 {
        return Err(LificError::Conflict(
            "database WAL is busy; stop the server before forcing a restore".into(),
        ));
    }
    Ok(())
}

/// True when a hot WAL warns the server may be running. Exposed so the CLI can
/// print the best-effort warning documented in the command help.
pub fn server_maybe_running(db_path: &Path) -> bool {
    wal_is_hot(db_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Scratch directory for one test. The guard removes it on Drop, which
    /// also runs while a failed assertion unwinds, so nothing is left for a
    /// later run to trip over.
    fn temp_dir(tag: &str) -> TempDir {
        tempfile::Builder::new()
            .prefix(&format!("lific_dump_{tag}_"))
            .tempdir()
            .unwrap()
    }

    /// Build a real on-disk DB with a seeded project, plus an attachments dir
    /// containing one real blob and one `.tmp` stray. Returns (dir guard,
    /// db_path).
    fn seed_data_dir(tag: &str) -> (TempDir, PathBuf) {
        let tmp = temp_dir(tag);
        let dir = tmp.path();
        let db_path = dir.join("lific.db");
        {
            let pool = crate::db::open(&db_path).unwrap();
            let conn = pool.write().unwrap();
            crate::db::queries::create_project(
                &conn,
                &crate::db::models::CreateProject {
                    name: "DumpTest".into(),
                    identifier: "DMP".into(),
                    description: String::new(),
                    emoji: None,
                    lead_user_id: None,
                },
            )
            .unwrap();
        }
        let att = dir.join("attachments");
        fs::create_dir_all(&att).unwrap();
        let first_sha = crate::storage::AttachmentStore::hash_bytes(b"blob one");
        let second_sha = crate::storage::AttachmentStore::hash_bytes(b"second blob bytes");
        fs::write(att.join(&first_sha), b"blob one").unwrap();
        fs::write(att.join(&second_sha), b"second blob bytes").unwrap();
        fs::write(att.join(format!("{second_sha}.tmp")), b"partial write").unwrap();
        (tmp, db_path)
    }

    /// List the entry names inside an archive.
    fn archive_entries(archive: &Path) -> Vec<String> {
        let file = fs::File::open(archive).unwrap();
        let dec = flate2::read::GzDecoder::new(file);
        let mut tar = tar::Archive::new(dec);
        tar.entries()
            .unwrap()
            .map(|e| {
                e.unwrap()
                    .path()
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    #[test]
    fn dump_archive_contains_db_manifest_and_blobs_excluding_tmp() {
        let (dir_tmp, db_path) = seed_data_dir("contents");
        let dir = dir_tmp.path();
        let out = dir.join("out.tar.gz");
        let manifest = write_dump(&crate::db::open(&db_path).unwrap(), &db_path, &out).unwrap();

        let entries = archive_entries(&out);
        assert!(entries.contains(&ARCHIVE_DB_NAME.to_string()));
        assert!(entries.contains(&ARCHIVE_MANIFEST_NAME.to_string()));
        assert!(entries.contains(&format!(
            "{ARCHIVE_ATTACHMENTS_PREFIX}{}",
            crate::storage::AttachmentStore::hash_bytes(b"blob one")
        )));
        assert!(entries.contains(&format!(
            "{ARCHIVE_ATTACHMENTS_PREFIX}{}",
            crate::storage::AttachmentStore::hash_bytes(b"second blob bytes")
        )));
        assert!(
            !entries.iter().any(|e| e.ends_with(".tmp")),
            "in-progress .tmp writes must be excluded: {entries:?}"
        );

        assert_eq!(manifest.attachment_count, 2);
        assert_eq!(
            manifest.attachment_bytes,
            (b"blob one".len() + b"second blob bytes".len()) as u64
        );
        assert_eq!(manifest.lific_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(
            manifest.schema_version,
            crate::db::migrate::latest_version()
        );
        assert!(manifest.db_size_bytes > 0);
    }

    #[cfg(unix)]
    #[test]
    fn dump_archive_is_owner_only_0600() {
        use std::os::unix::fs::PermissionsExt;
        let (dir_tmp, db_path) = seed_data_dir("perms");
        let dir = dir_tmp.path();
        let out = dir.join("out.tar.gz");
        write_dump(&crate::db::open(&db_path).unwrap(), &db_path, &out).unwrap();
        let mode = fs::metadata(&out).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "archive must be chmod 0600");
    }

    /// Whether any dump staging directory is left in `dir`.
    fn has_dump_staging(dir: &Path) -> bool {
        fs::read_dir(dir).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".lific-dump-")
        })
    }

    #[test]
    fn failed_dump_cleans_up_its_staging_files() {
        // LIF-329: a dump that cannot publish its archive must leave nothing
        // behind. Squatting the final path with a directory is a destination
        // no dump may write to, on every platform.
        let (dir_tmp, db_path) = seed_data_dir("errclean");
        let dir = dir_tmp.path();
        let out = dir.join("blocked.tar.gz");
        fs::create_dir_all(&out).unwrap();

        let result = write_dump(&crate::db::open(&db_path).unwrap(), &db_path, &out);
        assert!(result.is_err(), "a directory is not a dump destination");

        assert!(
            !has_dump_staging(dir),
            "the private staging directory must not survive a failed dump"
        );
        assert!(
            !out.with_extension("archive.tmp").exists(),
            "no loose staging file may be left beside the output"
        );
        assert!(!out.with_extension("dbsnapshot.tmp").exists());
    }

    #[test]
    fn successful_dump_leaves_no_staging_directory_behind() {
        let (dir_tmp, db_path) = seed_data_dir("staging_clean");
        let dir = dir_tmp.path();
        let out = dir.join("out.tar.gz");
        write_dump(&crate::db::open(&db_path).unwrap(), &db_path, &out).unwrap();
        assert!(out.exists());
        assert!(
            !has_dump_staging(dir),
            "staging is removed on the happy path"
        );
    }

    #[cfg(unix)]
    #[test]
    fn dump_failing_after_staging_exists_still_cleans_up() {
        // A hard-linked blob fails the scan, which happens *after* the private
        // staging directory and its reserved files exist — the case a loose
        // `*.tmp` scheme used to strand for the backup sweep to find.
        let (dir_tmp, db_path) = seed_data_dir("errclean_late");
        let dir = dir_tmp.path();
        let outside = dir.join("outside");
        fs::write(&outside, b"outside").unwrap();
        fs::hard_link(&outside, dir.join("attachments").join("c".repeat(64))).unwrap();

        let out = dir.join("late.tar.gz");
        let error = write_dump(&crate::db::open(&db_path).unwrap(), &db_path, &out).unwrap_err();

        assert!(error.to_string().contains("hard-linked"), "got {error}");
        assert!(!out.exists(), "a failed dump publishes nothing");
        assert!(
            !has_dump_staging(dir),
            "staging must not survive the failure"
        );
    }

    #[cfg(unix)]
    #[test]
    fn dump_staging_lock_is_visible_to_the_backup_sweep() {
        // `backup::sweep_stale_tmps` skips a staging directory whose lock is
        // held, so a slow dump is never swept out from under itself. A crashed
        // dump releases the lock with its process and becomes sweepable.
        let dir_tmp = temp_dir("staging_lock");
        let staging = dir_tmp.path().join(".lific-dump-probe");
        fs::create_dir(&staging).unwrap();
        assert!(
            !staging_is_locked(&staging),
            "an abandoned staging dir must be sweepable"
        );

        let lock = StagingLock::create(&staging.join("lock")).unwrap();
        assert!(
            staging_is_locked(&staging),
            "an in-flight dump must not be swept"
        );
        drop(lock);
        assert!(!staging_is_locked(&staging));
    }

    #[cfg(unix)]
    #[test]
    fn dump_rejects_unsafe_destinations() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let (dir_tmp, db_path) = seed_data_dir("unsafe_destinations");
        let dir = dir_tmp.path();
        let target = dir.join("target.tar.gz");
        fs::write(&target, b"existing").unwrap();

        // A symlink at the destination would redirect the whole database dump
        // to wherever it points.
        let link = dir.join("link.tar.gz");
        symlink(&target, &link).unwrap();
        assert!(write_dump(&crate::db::open(&db_path).unwrap(), &db_path, &link).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"existing");

        // Same for a hard link: the other name would see the dump's bytes.
        let hardlink = dir.join("hardlink.tar.gz");
        fs::hard_link(&target, &hardlink).unwrap();
        assert!(write_dump(&crate::db::open(&db_path).unwrap(), &db_path, &hardlink).is_err());

        // And for a symlinked parent directory.
        let real_parent = dir.join("real-parent");
        fs::create_dir(&real_parent).unwrap();
        let parent_link = dir.join("parent-link");
        symlink(&real_parent, &parent_link).unwrap();
        let nested = parent_link.join("nested.tar.gz");
        assert!(write_dump(&crate::db::open(&db_path).unwrap(), &db_path, &nested).is_err());

        // A world-writable parent without the sticky bit lets anyone swap the
        // name out from under the rename.
        let loose_parent = dir.join("loose-parent");
        fs::create_dir(&loose_parent).unwrap();
        fs::set_permissions(&loose_parent, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(
            write_dump(
                &crate::db::open(&db_path).unwrap(),
                &db_path,
                &loose_parent.join("out.tar.gz")
            )
            .is_err()
        );

        // An ordinary private parent still works.
        let safe_parent = dir.join("safe-parent");
        fs::create_dir(&safe_parent).unwrap();
        fs::set_permissions(&safe_parent, fs::Permissions::from_mode(0o755)).unwrap();
        write_dump(
            &crate::db::open(&db_path).unwrap(),
            &db_path,
            &safe_parent.join("out.tar.gz"),
        )
        .expect("a private parent directory is a fine destination");
    }

    #[cfg(unix)]
    #[test]
    fn dump_skips_symlinked_blobs_and_refuses_hard_linked_ones() {
        use std::os::unix::fs::symlink;

        let (dir_tmp, db_path) = seed_data_dir("unsafe_attachments");
        let dir = dir_tmp.path();
        let attachments = dir.join("attachments");
        let name = "c".repeat(64);
        let entry = attachments.join(&name);
        let outside = dir.join("outside");
        fs::write(&outside, b"outside secret").unwrap();
        symlink(&outside, &entry).unwrap();

        // A symlinked store entry is not attachment data; archiving it would
        // copy a file from outside the store into the backup.
        let out = dir.join("symlink.tar.gz");
        let manifest = write_dump(&crate::db::open(&db_path).unwrap(), &db_path, &out).unwrap();
        assert!(!archive_entries(&out).contains(&format!("attachments/{name}")));
        assert_eq!(manifest.attachment_count, 2, "only the real blobs count");

        // A hard link means the same bytes are reachable and mutable from
        // outside the store, so the dump refuses rather than guessing.
        fs::remove_file(&entry).unwrap();
        fs::hard_link(&outside, &entry).unwrap();
        let out = dir.join("hardlink.tar.gz");
        let error = write_dump(&crate::db::open(&db_path).unwrap(), &db_path, &out).unwrap_err();
        assert!(error.to_string().contains("hard-linked"), "got {error}");
    }

    #[test]
    fn dump_refuses_a_blob_whose_bytes_do_not_match_its_name() {
        // The name of a blob IS its digest. Publishing an archive where that
        // is untrue would ship a backup every restore refuses, discovered only
        // on the day someone needs it.
        let (dir_tmp, db_path) = seed_data_dir("corrupt_blob");
        let dir = dir_tmp.path();
        let lying_name = crate::storage::AttachmentStore::hash_bytes(b"the honest bytes");
        fs::write(dir.join("attachments").join(&lying_name), b"tampered bytes").unwrap();

        let out = dir.join("corrupt.tar.gz");
        let error = write_dump(&crate::db::open(&db_path).unwrap(), &db_path, &out).unwrap_err();

        assert!(
            error.to_string().contains("content address"),
            "the failure must name the mismatch: {error}"
        );
        assert!(!out.exists(), "a corrupt store must not publish an archive");
        assert!(!has_dump_staging(dir));
    }

    #[test]
    fn dump_hashes_every_blob_it_archives_and_still_round_trips() {
        // Positive control for the check above: honest blobs still dump and
        // restore, bytes intact.
        let (src_tmp, src_db) = seed_data_dir("hash_round_trip");
        let archive = src_tmp.path().join("backup.tar.gz");
        let manifest = write_dump(&crate::db::open(&src_db).unwrap(), &src_db, &archive).unwrap();
        assert_eq!(manifest.attachment_count, 2);

        let dst = temp_dir("hash_round_trip_dst");
        let dst_db = dst.path().join(ARCHIVE_DB_NAME);
        run_restore(&archive, &dst_db, false).unwrap();
        let sha = crate::storage::AttachmentStore::hash_bytes(b"blob one");
        assert_eq!(
            fs::read(dst.path().join("attachments").join(&sha)).unwrap(),
            b"blob one"
        );
    }

    #[test]
    fn dump_holds_the_attachment_store_lock_for_its_whole_run() {
        // An upload, delete or GC sweep landing between the database snapshot
        // and the blob scan would archive a database referencing blobs the
        // archive does not contain. The dump takes the same store lock those
        // operations take, so it cannot interleave with them.
        use std::sync::mpsc::sync_channel;
        use std::time::Duration;

        let (dir_tmp, db_path) = seed_data_dir("dump_store_lock");
        let store = crate::storage::AttachmentStore::from_db_path(&db_path);
        let out = dir_tmp.path().join("locked.tar.gz");

        let (holding_tx, holding_rx) = sync_channel::<()>(1);
        let (release_tx, release_rx) = sync_channel::<()>(1);
        let holder = std::thread::spawn(move || {
            store.with_lock(|_| {
                holding_tx.send(()).unwrap();
                let _ = release_rx.recv_timeout(Duration::from_secs(10));
                Ok(())
            })
        });
        holding_rx.recv_timeout(Duration::from_secs(10)).unwrap();

        let (done_tx, done_rx) = sync_channel::<()>(1);
        let dumper = std::thread::spawn({
            let out = out.clone();
            move || {
                let result = write_dump(&crate::db::open(&db_path).unwrap(), &db_path, &out);
                done_tx.send(()).unwrap();
                result
            }
        });

        assert!(
            done_rx.recv_timeout(Duration::from_millis(250)).is_err(),
            "the dump must wait for the store lock instead of racing the store"
        );

        release_tx.send(()).unwrap();
        holder.join().unwrap().unwrap();
        done_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the dump proceeds once the store lock is free");
        dumper.join().unwrap().expect("dump succeeds");
        assert!(out.exists());
    }

    #[test]
    fn restore_install_waits_for_the_attachment_store_lock() {
        // The install phase swaps the attachments directory out from under the
        // whole instance. Anything holding the store (a dump, an upload, a GC
        // sweep) must finish first, or it reads half of each data set.
        use std::sync::mpsc::sync_channel;
        use std::time::Duration;

        let (src_tmp, src_db) = seed_data_dir("restore_store_lock_src");
        let archive = src_tmp.path().join("backup.tar.gz");
        write_dump(&crate::db::open(&src_db).unwrap(), &src_db, &archive).unwrap();

        let dst = temp_dir("restore_store_lock_dst");
        let dst_db = dst.path().join(ARCHIVE_DB_NAME);
        let holder_store = crate::storage::AttachmentStore::from_db_path(&dst_db);
        let observer = crate::storage::AttachmentStore::from_db_path(&dst_db);

        let (entered_tx, entered_rx) = sync_channel::<()>(1);
        let (release_tx, release_rx) = sync_channel::<()>(1);
        let holder = std::thread::spawn(move || {
            holder_store.with_lock(|_| {
                entered_tx.send(()).unwrap();
                let _ = release_rx.recv_timeout(Duration::from_secs(10));
                Ok(())
            })
        });
        entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();

        let (done_tx, done_rx) = sync_channel::<()>(1);
        let restorer = std::thread::spawn({
            let dst_db = dst_db.clone();
            move || {
                let result = run_restore(&archive, &dst_db, false);
                done_tx.send(()).unwrap();
                result
            }
        });

        assert!(
            done_rx.recv_timeout(Duration::from_millis(250)).is_err(),
            "the install must wait for the store lock rather than swap under it"
        );
        assert!(
            !dst_db.exists(),
            "nothing is installed while the store is held"
        );

        release_tx.send(()).unwrap();
        holder.join().unwrap().unwrap();
        done_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the restore proceeds once the store lock is free");
        let result = restorer.join().unwrap().expect("restore succeeds");
        assert_eq!(result.attachment_count, 2);

        // The lock is a sibling of `attachments/`, so replacing that directory
        // did not replace the lock: it still coordinates afterwards.
        assert!(observer.lock_path().is_file());
        assert!(!observer.lock_is_held());
        let after = crate::storage::AttachmentStore::from_db_path(&dst_db);
        let (held_tx, held_rx) = sync_channel::<()>(1);
        let (drop_tx, drop_rx) = sync_channel::<()>(1);
        let second = std::thread::spawn(move || {
            after.with_lock(|_| {
                held_tx.send(()).unwrap();
                let _ = drop_rx.recv_timeout(Duration::from_secs(10));
                Ok(())
            })
        });
        held_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(
            observer.lock_is_held(),
            "the same lock still serializes work after the attachments swap"
        );
        drop_tx.send(()).unwrap();
        second.join().unwrap().unwrap();
    }

    #[test]
    fn dump_skips_store_entries_that_are_not_content_addressed() {
        let (dir_tmp, db_path) = seed_data_dir("bad_blob_names");
        let dir = dir_tmp.path();
        fs::write(dir.join("attachments").join("not-a-hash"), b"junk").unwrap();
        fs::create_dir(dir.join("attachments").join("nested")).unwrap();

        let out = dir.join("out.tar.gz");
        let manifest = write_dump(&crate::db::open(&db_path).unwrap(), &db_path, &out).unwrap();

        assert_eq!(manifest.attachment_count, 2);
        assert!(
            !archive_entries(&out)
                .iter()
                .any(|entry| entry.contains("not-a-hash") || entry.contains("nested")),
            "only bare sha256 blobs belong in an archive"
        );
        // The archive still round-trips through the full validator.
        inspect_archive(&out, &RestoreLimits::default()).unwrap();
    }

    #[test]
    fn dumped_db_is_openable_sqlite_with_seeded_data() {
        let (dir_tmp, db_path) = seed_data_dir("snapshot");
        let dir = dir_tmp.path();
        let out = dir.join("snap.tar.gz");
        write_dump(&crate::db::open(&db_path).unwrap(), &db_path, &out).unwrap();

        // Extract the db member and open it.
        let extract_dir = dir.join("extracted");
        fs::create_dir_all(&extract_dir).unwrap();
        let file = fs::File::open(&out).unwrap();
        let dec = flate2::read::GzDecoder::new(file);
        let mut tar = tar::Archive::new(dec);
        for entry in tar.entries().unwrap() {
            let mut entry = entry.unwrap();
            if entry.path().unwrap().to_string_lossy() == ARCHIVE_DB_NAME {
                let mut buf = Vec::new();
                std::io::Read::read_to_end(&mut entry, &mut buf).unwrap();
                fs::write(extract_dir.join("lific.db"), &buf).unwrap();
            }
        }
        let conn = rusqlite::Connection::open(extract_dir.join("lific.db")).unwrap();
        let name: String = conn
            .query_row(
                "SELECT name FROM projects WHERE identifier = 'DMP'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(name, "DumpTest");
    }

    #[test]
    fn run_dump_into_directory_uses_default_filename() {
        let (dir_tmp, db_path) = seed_data_dir("outdir");
        let dir = dir_tmp.path();
        let target = dir.join("dumps");
        fs::create_dir_all(&target).unwrap();
        let res = run_dump(&db_path, Some(&target)).unwrap();
        assert_eq!(res.archive_path.parent().unwrap(), target);
        let fname = res.archive_path.file_name().unwrap().to_string_lossy();
        assert!(fname.starts_with("lific_"));
        assert!(fname.ends_with(".tar.gz"));
        assert!(res.archive_path.exists());
    }

    #[test]
    fn restore_round_trip_matches_entities_and_blob_bytes() {
        let (src_dir_tmp, src_db) = seed_data_dir("rt_src");
        let src_dir = src_dir_tmp.path();
        let out = src_dir.join("backup.tar.gz");
        write_dump(&crate::db::open(&src_db).unwrap(), &src_db, &out).unwrap();

        // Fresh, empty destination dir.
        let dst_dir_tmp = temp_dir("rt_dst");
        let dst_dir = dst_dir_tmp.path();
        let dst_db = dst_dir.join("lific.db");
        let res = run_restore(&out, &dst_db, false).unwrap();
        assert_eq!(res.attachment_count, 2);

        // Entities: the seeded project is present.
        let pool = crate::db::open(&dst_db).unwrap();
        let conn = pool.read().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM projects WHERE identifier = 'DMP'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        // Blob bytes identical.
        assert_eq!(
            fs::read(
                dst_dir
                    .join("attachments")
                    .join(crate::storage::AttachmentStore::hash_bytes(b"blob one"),)
            )
            .unwrap(),
            b"blob one"
        );
        assert_eq!(
            fs::read(dst_dir.join("attachments").join(
                crate::storage::AttachmentStore::hash_bytes(b"second blob bytes"),
            ))
            .unwrap(),
            b"second blob bytes"
        );
    }

    #[test]
    fn restore_refuses_existing_db_without_force() {
        let (src_dir_tmp, src_db) = seed_data_dir("guard_src");
        let src_dir = src_dir_tmp.path();
        let out = src_dir.join("b.tar.gz");
        write_dump(&crate::db::open(&src_db).unwrap(), &src_db, &out).unwrap();

        // Destination already has a db.
        let dst_dir_tmp = temp_dir("guard_dst");
        let dst_dir = dst_dir_tmp.path();
        let dst_db = dst_dir.join("lific.db");
        let _ = crate::db::open(&dst_db).unwrap();

        let err = run_restore(&out, &dst_db, false).unwrap_err();
        assert!(matches!(err, LificError::Conflict(_)), "got {err:?}");
    }

    #[cfg(unix)]
    #[test]
    fn restore_force_moves_existing_db_aside() {
        let (src_dir_tmp, src_db) = seed_data_dir("force_src");
        let src_dir = src_dir_tmp.path();
        let out = src_dir.join("b.tar.gz");
        write_dump(&crate::db::open(&src_db).unwrap(), &src_db, &out).unwrap();

        let dst_dir_tmp = temp_dir("force_dst");
        let dst_dir = dst_dir_tmp.path();
        let dst_db = dst_dir.join("lific.db");
        // Seed a DIFFERENT project so we can tell the old db apart.
        {
            let pool = crate::db::open(&dst_db).unwrap();
            let conn = pool.write().unwrap();
            crate::db::queries::create_project(
                &conn,
                &crate::db::models::CreateProject {
                    name: "OldData".into(),
                    identifier: "OLD".into(),
                    description: String::new(),
                    emoji: None,
                    lead_user_id: None,
                },
            )
            .unwrap();
        }

        let res = run_restore(&out, &dst_db, true).unwrap();
        let moved = res
            .moved_existing_to
            .expect("existing db should be moved aside");
        assert!(moved.exists(), "moved-aside db must still exist");
        assert!(
            moved
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains("pre-restore-"),
            "moved db name must include pre-restore-: {}",
            moved.display()
        );
        // The moved-aside db still has the OLD project.
        let conn = rusqlite::Connection::open(&moved).unwrap();
        let old: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM projects WHERE identifier='OLD'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(old, 1);
        // The live db now has the restored project.
        let pool = crate::db::open(&dst_db).unwrap();
        let conn = pool.read().unwrap();
        let dmp: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM projects WHERE identifier='DMP'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dmp, 1);
    }

    #[test]
    fn force_restore_refuses_an_uncheckpointable_database() {
        let (src_dir_tmp, src_db) = seed_data_dir("checkpoint_src");
        let archive = src_dir_tmp.path().join("backup.tar.gz");
        write_dump(&crate::db::open(&src_db).unwrap(), &src_db, &archive).unwrap();

        let destination = temp_dir("checkpoint_dst");
        let db_path = destination.path().join(ARCHIVE_DB_NAME);
        fs::create_dir(&db_path).unwrap();
        let sentinel = db_path.join("must-survive");
        fs::write(&sentinel, b"original state").unwrap();

        let error = run_restore(&archive, &db_path, true).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("open database for WAL checkpoint")
        );
        assert!(db_path.is_dir());
        assert_eq!(fs::read(sentinel).unwrap(), b"original state");
    }

    #[test]
    fn restore_refuses_newer_schema_version() {
        let (src_dir_tmp, src_db) = seed_data_dir("newer_src");
        let src_dir = src_dir_tmp.path();
        let out = src_dir.join("b.tar.gz");
        write_dump(&crate::db::open(&src_db).unwrap(), &src_db, &out).unwrap();

        // Rewrite the archive with a bumped schema_version to simulate a dump
        // from a newer Lific.
        let bumped = src_dir.join("bumped.tar.gz");
        rewrite_archive_with_schema(&out, &bumped, crate::db::migrate::latest_version() + 5);

        let dst_dir_tmp = temp_dir("newer_dst");
        let dst_dir = dst_dir_tmp.path();
        let dst_db = dst_dir.join("lific.db");
        let err = run_restore(&bumped, &dst_db, false).unwrap_err();
        assert!(matches!(err, LificError::BadRequest(_)), "got {err:?}");
        assert!(
            !dst_db.exists(),
            "nothing should be restored on schema refusal"
        );
    }

    #[test]
    fn restore_accepts_pre_attachment_schema_without_attachment_table() {
        let dir_tmp = temp_dir("legacy_schema");
        let dir = dir_tmp.path();
        let legacy_db = dir.join("legacy.db");
        {
            let conn = rusqlite::Connection::open(&legacy_db).unwrap();
            conn.execute_batch(
                "CREATE TABLE _migrations (version INTEGER PRIMARY KEY);
                 INSERT INTO _migrations (version) VALUES (30);",
            )
            .unwrap();
        }

        let archive = dir.join("legacy.tar.gz");
        let manifest = Manifest {
            lific_version: "old".into(),
            schema_version: 30,
            created_at: "now".into(),
            db_size_bytes: fs::metadata(&legacy_db).unwrap().len(),
            attachment_count: 0,
            attachment_bytes: 0,
            archived_project_count: 0,
            archived_project_bytes: 0,
        };
        let file = fs::File::create(&archive).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        append_bytes(
            &mut builder,
            ARCHIVE_MANIFEST_NAME,
            &serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        builder
            .append_path_with_name(&legacy_db, ARCHIVE_DB_NAME)
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();

        let destination = temp_dir("legacy_schema_dst");
        let db_path = destination.path().join(ARCHIVE_DB_NAME);
        run_restore(&archive, &db_path, false).expect("older empty backups remain restorable");
        assert!(db_path.exists());
        assert!(
            !destination
                .path()
                .join("attachments")
                .join("unexpected")
                .exists()
        );
    }

    #[test]
    fn staged_database_accepts_pre_integrity_attachment_schema() {
        let dir_tmp = temp_dir("legacy_attachments");
        let dir = dir_tmp.path();
        let db = dir.join(ARCHIVE_DB_NAME);
        let sha = crate::storage::AttachmentStore::hash_bytes(b"legacy");
        {
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE _migrations (version INTEGER PRIMARY KEY);
                 INSERT INTO _migrations (version) VALUES (42);
                 CREATE TABLE attachments (
                     sha256 TEXT NOT NULL,
                     filename TEXT NOT NULL,
                     mime TEXT NOT NULL,
                     size_bytes INTEGER NOT NULL
                 );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO attachments (sha256, filename, mime, size_bytes)
                 VALUES (?1, 'legacy.txt', 'text/plain', 6)",
                rusqlite::params![sha],
            )
            .unwrap();
        }

        let staging = dir.join("staging");
        fs::create_dir_all(staging.join("attachments")).unwrap();
        fs::copy(&db, staging.join(ARCHIVE_DB_NAME)).unwrap();
        fs::write(staging.join("attachments").join(&sha), b"legacy").unwrap();
        let manifest = Manifest {
            lific_version: "old".into(),
            schema_version: 42,
            created_at: "now".into(),
            db_size_bytes: fs::metadata(&db).unwrap().len(),
            attachment_count: 1,
            attachment_bytes: 6,
            archived_project_count: 0,
            archived_project_bytes: 0,
        };

        validate_staged_database(&staging, &manifest, &RestoreLimits::default())
            .expect("pre-integrity archives remain restorable");
    }

    #[test]
    fn restore_rejects_path_traversal_entry() {
        let dir_tmp = temp_dir("traversal");
        let dir = dir_tmp.path();
        let archive = dir.join("evil.tar.gz");
        // Craft an archive with a manifest, a db, and a malicious attachment
        // entry that tries to escape the attachments dir.
        {
            let file = fs::File::create(&archive).unwrap();
            let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            let mut tar = tar::Builder::new(enc);
            let manifest = Manifest {
                lific_version: "x".into(),
                schema_version: 1,
                created_at: "now".into(),
                db_size_bytes: 1,
                attachment_count: 1,
                attachment_bytes: 1,
                archived_project_count: 0,
                archived_project_bytes: 0,
            };
            let mj = serde_json::to_vec(&manifest).unwrap();
            append_bytes(&mut tar, ARCHIVE_MANIFEST_NAME, &mj).unwrap();
            append_bytes(&mut tar, ARCHIVE_DB_NAME, b"not a real db").unwrap();
            // A nested path under attachments/ — tar permits writing it, but a
            // blob name must be a bare hash, so the validator must reject it
            // (this is the class of entry a path-traversal payload uses).
            append_bytes(&mut tar, "attachments/sub/escape", b"pwned").unwrap();
            let enc = tar.into_inner().unwrap();
            enc.finish().unwrap();
        }

        // inspect_archive must reject it.
        let err = inspect_archive(&archive, &RestoreLimits::default()).unwrap_err();
        assert!(matches!(err, LificError::BadRequest(_)), "got {err:?}");

        // And a full restore attempt must also refuse, leaving no db behind.
        let dst_dir_tmp = temp_dir("traversal_dst");
        let dst_dir = dst_dir_tmp.path();
        let dst_db = dst_dir.join("lific.db");
        assert!(run_restore(&archive, &dst_db, false).is_err());
        assert!(!dst_db.exists());
        assert!(
            !dst_dir.join("attachments").join("sub").exists(),
            "traversal entry must not write nested dirs"
        );
    }

    #[cfg(unix)]
    #[test]
    fn restore_mid_extract_failure_leaves_original_untouched() {
        // A corrupt archive (valid manifest+db header claim, truncated body)
        // must fail extraction WITHOUT clobbering the pre-existing db when
        // --force moved it aside — the rollback restores it.
        let (src_dir_tmp, src_db) = seed_data_dir("midfail_src");
        let src_dir = src_dir_tmp.path();
        let good = src_dir.join("good.tar.gz");
        write_dump(&crate::db::open(&src_db).unwrap(), &src_db, &good).unwrap();

        // Truncate the good archive to corrupt it mid-stream.
        let corrupt = src_dir.join("corrupt.tar.gz");
        let bytes = fs::read(&good).unwrap();
        fs::write(&corrupt, &bytes[..bytes.len() / 2]).unwrap();

        let dst_dir_tmp = temp_dir("midfail_dst");
        let dst_dir = dst_dir_tmp.path();
        let dst_db = dst_dir.join("lific.db");
        {
            let pool = crate::db::open(&dst_db).unwrap();
            let conn = pool.write().unwrap();
            crate::db::queries::create_project(
                &conn,
                &crate::db::models::CreateProject {
                    name: "Original".into(),
                    identifier: "ORG".into(),
                    description: String::new(),
                    emoji: None,
                    lead_user_id: None,
                },
            )
            .unwrap();
        }
        // Force restore of the corrupt archive should error but roll back.
        let result = run_restore(&corrupt, &dst_db, true);
        assert!(result.is_err(), "corrupt archive must fail");
        // Original db must still be present and openable with its project.
        assert!(dst_db.exists(), "original db must be restored on rollback");
        let pool = crate::db::open(&dst_db).unwrap();
        let conn = pool.read().unwrap();
        let org: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM projects WHERE identifier='ORG'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(org, 1, "original data must survive a failed restore");
    }

    #[test]
    fn failed_rollback_error_names_both_failures_and_the_surviving_db_path() {
        // LIF-371: when the moved-aside db cannot be renamed back, the user
        // must be told the restore failed AND where their database still is.
        let dir_tmp = temp_dir("rollback_fail");
        let dir = dir_tmp.path();
        let db_path = dir.join("lific.db");
        // A source that does not exist makes the rollback rename fail.
        let moved = dir.join("lific.db.pre-restore-gone");

        let err = rollback_moved_db(
            &moved,
            &db_path,
            LificError::BadRequest("read db from archive: unexpected eof".into()),
        );
        let msg = err.to_string();

        assert!(matches!(err, LificError::Internal(_)), "got {err:?}");
        assert!(
            msg.contains("read db from archive: unexpected eof"),
            "must carry the original failure: {msg}"
        );
        assert!(
            msg.contains(&moved.display().to_string()),
            "must name the path the original db still lives at: {msg}"
        );
        assert!(
            msg.contains(&db_path.display().to_string()),
            "must name where to move it back to: {msg}"
        );
    }

    #[test]
    fn successful_rollback_surfaces_the_original_error_unchanged() {
        let dir_tmp = temp_dir("rollback_ok");
        let dir = dir_tmp.path();
        let db_path = dir.join("lific.db");
        let moved = dir.join("lific.db.pre-restore-1");
        fs::write(&moved, b"original db bytes").unwrap();

        let err = rollback_moved_db(
            &moved,
            &db_path,
            LificError::BadRequest("archive is missing lific.db".into()),
        );

        assert!(matches!(err, LificError::BadRequest(ref m) if m == "archive is missing lific.db"));
        assert_eq!(fs::read(&db_path).unwrap(), b"original db bytes");
        assert!(!moved.exists());
    }

    #[test]
    fn validate_attachment_entry_accepts_bare_hash_rejects_traversal() {
        assert!(
            validate_attachment_entry(
                "attachments/0000000000000000000000000000000000000000000000000000000000000001"
            )
            .is_ok()
        );
        assert!(validate_attachment_entry("attachments/../etc/passwd").is_err());
        assert!(validate_attachment_entry("attachments/sub/dir").is_err());
        assert!(validate_attachment_entry("attachments/").is_err());
        assert!(validate_attachment_entry("notattachments/x").is_err());
        assert!(validate_attachment_entry("attachments/.hidden").is_err());
    }

    #[test]
    fn inspect_rejects_manifest_size_bomb() {
        let (dir_tmp, db_path) = seed_data_dir("manifest_limit");
        let dir = dir_tmp.path();
        let out = dir.join("source.tar.gz");
        write_dump(&crate::db::open(&db_path).unwrap(), &db_path, &out).unwrap();
        let mut manifest = read_manifest(&out, &RestoreLimits::default()).unwrap();
        manifest.attachment_bytes = MAX_TOTAL_RESTORE_BYTES;
        let rewritten = dir.join("oversized.tar.gz");
        rewrite_archive_manifest(&out, &rewritten, &manifest);
        let error = inspect_archive(&rewritten, &RestoreLimits::default()).unwrap_err();
        assert!(error.to_string().contains("total restore size"));
    }

    #[test]
    fn compressed_archive_expansion_is_bounded() {
        const TEST_LIMIT: u64 = 1024 * 1024;
        let dir_tmp = temp_dir("compressed_expansion");
        let archive = dir_tmp.path().join("bomb.tar.gz");
        let expanded_bytes = vec![b'x'; (TEST_LIMIT * 2) as usize];
        let file = fs::File::create(&archive).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::best());
        let mut builder = tar::Builder::new(encoder);
        append_bytes(&mut builder, "payload", &expanded_bytes).unwrap();
        builder.into_inner().unwrap().finish().unwrap();

        assert!(
            fs::metadata(&archive).unwrap().len() < expanded_bytes.len() as u64 / 100,
            "the fixture must be much smaller while expanding past the reader limit"
        );
        let file = fs::File::open(&archive).unwrap();
        let mut archive = bounded_archive_with_limit(file, TEST_LIMIT);
        let mut entries = archive.entries().unwrap().raw(true);
        let mut entry = entries.next().unwrap().unwrap();
        let mut total_bytes = 0;
        let error = copy_entry_bounded(
            &mut entry,
            &mut std::io::sink(),
            MAX_ATTACHMENT_BYTES,
            &mut total_bytes,
            MAX_TOTAL_RESTORE_BYTES,
            "payload",
        )
        .unwrap_err();
        assert!(error.to_string().contains("expected 2097152 bytes"));
    }

    #[test]
    fn inspect_rejects_tar_extension_entries_before_reading_their_body() {
        let dir_tmp = temp_dir("tar_extension");
        let archive = dir_tmp.path().join("extension.tar.gz");
        let file = fs::File::create(&archive).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::best());
        let mut builder = tar::Builder::new(encoder);
        let extension = vec![b'x'; MAX_MANIFEST_BYTES as usize + 1];
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::XHeader);
        header.set_size(extension.len() as u64);
        header.set_mode(0o600);
        header.set_cksum();
        builder
            .append_data(&mut header, "pax", extension.as_slice())
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();

        let error = inspect_archive(&archive, &RestoreLimits::default()).unwrap_err();
        assert!(error.to_string().contains("unsupported tar entry type"));
    }

    #[test]
    fn manifest_scan_rejects_excessive_entries_before_manifest() {
        let dir_tmp = temp_dir("manifest_entries");
        let archive = dir_tmp.path().join("too-many.tar.gz");
        let file = fs::File::create(&archive).unwrap();
        let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut builder = tar::Builder::new(enc);
        for index in 0..(MAX_RESTORE_ENTRIES + 3) {
            append_bytes(&mut builder, &format!("ignored-{index}"), &[0]).unwrap();
        }
        let encoder = builder.into_inner().unwrap();
        encoder.finish().unwrap();
        let error = inspect_archive(&archive, &RestoreLimits::default()).unwrap_err();
        assert!(error.to_string().contains("before its manifest"));
    }

    #[test]
    fn inspect_rejects_duplicate_control_entries() {
        let dir_tmp = temp_dir("duplicate_entries");
        let archive = dir_tmp.path().join("duplicate.tar.gz");
        let file = fs::File::create(&archive).unwrap();
        let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut builder = tar::Builder::new(enc);
        let manifest = Manifest {
            lific_version: env!("CARGO_PKG_VERSION").into(),
            schema_version: 1,
            created_at: String::new(),
            db_size_bytes: 1,
            attachment_count: 0,
            attachment_bytes: 0,
            archived_project_count: 0,
            archived_project_bytes: 0,
        };
        let bytes = serde_json::to_vec(&manifest).unwrap();
        append_bytes(&mut builder, ARCHIVE_MANIFEST_NAME, &bytes).unwrap();
        append_bytes(&mut builder, ARCHIVE_MANIFEST_NAME, &bytes).unwrap();
        let encoder = builder.into_inner().unwrap();
        encoder.finish().unwrap();
        let error = inspect_archive(&archive, &RestoreLimits::default()).unwrap_err();
        assert!(error.to_string().contains("duplicate manifest"));
    }

    #[test]
    fn restore_rejects_manifest_schema_that_does_not_match_database() {
        let (dir_tmp, db_path) = seed_data_dir("schema_mismatch");
        let archive = dir_tmp.path().join("source.tar.gz");
        write_dump(&crate::db::open(&db_path).unwrap(), &db_path, &archive).unwrap();
        let rewritten = dir_tmp.path().join("mismatch.tar.gz");
        rewrite_archive_with_schema(
            &archive,
            &rewritten,
            crate::db::migrate::latest_version() - 1,
        );

        let destination = temp_dir("schema_mismatch_dst");
        let error =
            run_restore(&rewritten, &destination.path().join(ARCHIVE_DB_NAME), false).unwrap_err();
        assert!(error.to_string().contains("does not match manifest"));
    }

    #[test]
    fn staged_database_rejects_unconstrained_attachment_schema() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE attachments (
                sha256 TEXT NOT NULL,
                filename TEXT NOT NULL,
                mime TEXT NOT NULL /* mime TEXT NOT NULL CHECK (mime IN ( */,
                size_bytes INTEGER NOT NULL /* size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0) */
                /* CHECK (length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*') */
            )",
        )
        .unwrap();

        let error = validate_attachment_schema(&conn).unwrap_err();
        assert!(error.to_string().contains("integrity constraints"));
    }

    #[test]
    fn staged_schema_probe_does_not_trust_source_triggers() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE attachments (
                sha256 TEXT NOT NULL,
                filename TEXT NOT NULL,
                mime TEXT NOT NULL,
                size_bytes INTEGER NOT NULL
            );
            CREATE TRIGGER reject_probe_values BEFORE INSERT ON attachments
            WHEN NEW.sha256 = '../outside'
              OR NEW.mime = 'application/octet-stream'
              OR NEW.size_bytes < 0
            BEGIN
                SELECT RAISE(ABORT, 'probe value rejected');
            END;",
        )
        .unwrap();

        let error = validate_attachment_schema(&conn).unwrap_err();
        assert!(error.to_string().contains("integrity constraints"));
    }

    #[test]
    fn restore_lock_serializes_restores() {
        let dir = temp_dir("restore_lock");
        let first = RestoreLock::acquire(dir.path()).unwrap();
        assert!(matches!(
            RestoreLock::acquire(dir.path()),
            Err(LificError::Conflict(_))
        ));
        drop(first);
        RestoreLock::acquire(dir.path()).expect("lock is released after restore");
    }

    #[test]
    fn a_lock_file_left_by_a_crashed_restore_is_immediately_reusable() {
        // The lock is advisory and lives in the kernel, so a process that dies
        // mid-restore releases it. All it leaves on disk is the file itself,
        // and that must not wedge the data directory: owning a *directory*
        // instead meant a crash locked the user out until they found and
        // deleted the leftover by hand.
        let dir = temp_dir("restore_lock_stale");
        let path = dir.path().join(".lific-restore.lock");
        fs::write(&path, b"").unwrap();

        let lock = RestoreLock::acquire(dir.path())
            .expect("a stale lock file must not block the next restore");
        drop(lock);

        assert!(path.is_file(), "the lock file is reused, not unlinked");
        RestoreLock::acquire(dir.path()).expect("still reusable after release");
    }

    #[test]
    fn restore_refuses_to_start_while_another_restore_holds_the_lock() {
        let (src_dir_tmp, src_db) = seed_data_dir("lock_busy_src");
        let archive = src_dir_tmp.path().join("backup.tar.gz");
        write_dump(&crate::db::open(&src_db).unwrap(), &src_db, &archive).unwrap();

        let dst = temp_dir("lock_busy_dst");
        let dst_db = dst.path().join(ARCHIVE_DB_NAME);
        let held = RestoreLock::acquire(dst.path()).unwrap();

        let error = run_restore(&archive, &dst_db, false).unwrap_err();
        assert!(matches!(error, LificError::Conflict(_)), "got {error:?}");
        assert!(error.to_string().contains("another restore"));
        assert!(!dst_db.exists(), "a rejected restore installs nothing");

        drop(held);
        run_restore(&archive, &dst_db, false).expect("restore proceeds once the lock is free");
    }

    /// A live data dir with a seeded database and one attachment blob, plus a
    /// validated staging tree ready to be installed over it. Returns the guard,
    /// the data dir, the db path, the staging path and the blob's sha.
    fn seed_install_fixture(tag: &str) -> (TempDir, PathBuf, PathBuf, String) {
        let dir_tmp = temp_dir(tag);
        let root = dir_tmp.path().to_path_buf();
        let db_path = root.join(ARCHIVE_DB_NAME);
        {
            let pool = crate::db::open(&db_path).unwrap();
            let conn = pool.write().unwrap();
            crate::db::queries::create_project(
                &conn,
                &crate::db::models::CreateProject {
                    name: "LiveData".into(),
                    identifier: "LIV".into(),
                    description: String::new(),
                    emoji: None,
                    lead_user_id: None,
                },
            )
            .unwrap();
        }
        let sha = crate::storage::AttachmentStore::hash_bytes(b"live blob");
        fs::create_dir(root.join("attachments")).unwrap();
        fs::write(root.join("attachments").join(&sha), b"live blob").unwrap();

        let staging = root.join("staging");
        fs::create_dir_all(staging.join("attachments")).unwrap();
        fs::create_dir_all(staging.join(ARCHIVED_PROJECTS_DIR_NAME)).unwrap();
        fs::write(staging.join(ARCHIVE_DB_NAME), b"restored database bytes").unwrap();
        (dir_tmp, db_path, staging, sha)
    }

    #[test]
    fn install_rolls_back_the_moved_db_when_the_attachment_backup_path_is_taken() {
        // The refusal happens *after* the live database has been renamed
        // aside. Returning it straight to the caller left the user with no
        // database at db_path at all, which reads exactly like data loss.
        let (dir_tmp, db_path, staging, sha) = seed_install_fixture("install_backup_clash");
        let root = dir_tmp.path();
        fs::create_dir(root.join("attachments.pre-restore-clash")).unwrap();

        let error = install_restore(&staging, &db_path, "clash").unwrap_err();

        assert!(matches!(error, LificError::Conflict(_)), "got {error:?}");
        assert!(
            error
                .to_string()
                .contains("attachment backup already exists"),
            "the original cause must survive the rollback: {error}"
        );
        assert!(
            db_path.exists(),
            "the live database must be rolled back into place"
        );
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let live: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM projects WHERE identifier = 'LIV'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            live, 1,
            "and it must be the user's database, not the dump's"
        );
        assert!(
            !root
                .join(format!("{}.pre-restore-clash", ARCHIVE_DB_NAME))
                .exists(),
            "nothing may be stranded under the pre-restore name"
        );
        assert_eq!(
            fs::read(root.join("attachments").join(&sha)).unwrap(),
            b"live blob",
            "the live attachments dir is untouched"
        );
    }

    #[test]
    fn install_rolls_back_the_moved_db_when_the_db_backup_path_is_taken() {
        let (dir_tmp, db_path, staging, _sha) = seed_install_fixture("install_db_clash");
        let root = dir_tmp.path();
        fs::write(
            root.join(format!("{}.pre-restore-clash", ARCHIVE_DB_NAME)),
            b"someone else's file",
        )
        .unwrap();

        let error = install_restore(&staging, &db_path, "clash").unwrap_err();

        assert!(matches!(error, LificError::Conflict(_)), "got {error:?}");
        assert!(db_path.exists(), "nothing was moved, nothing is missing");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let live: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM projects WHERE identifier = 'LIV'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(live, 1);
        assert_eq!(
            fs::read(root.join(format!("{}.pre-restore-clash", ARCHIVE_DB_NAME))).unwrap(),
            b"someone else's file",
            "an unrelated file at the backup path is never clobbered"
        );
    }

    #[test]
    fn install_moves_live_state_aside_and_publishes_the_staged_tree() {
        // Positive control for the two rollback tests above.
        let (dir_tmp, db_path, staging, sha) = seed_install_fixture("install_ok");
        let root = dir_tmp.path();
        let restored_sha = crate::storage::AttachmentStore::hash_bytes(b"restored blob");
        fs::write(
            staging.join("attachments").join(&restored_sha),
            b"restored blob",
        )
        .unwrap();

        let moved = install_restore(&staging, &db_path, "ok")
            .unwrap()
            .expect("the live db is moved aside");

        assert_eq!(fs::read(&db_path).unwrap(), b"restored database bytes");
        assert!(moved.exists(), "the previous database is kept, not deleted");
        assert!(
            root.join("attachments").join(&restored_sha).exists(),
            "restored blobs are live"
        );
        assert!(
            !root.join("attachments").join(&sha).exists(),
            "the old attachments dir is replaced wholesale"
        );
        assert!(
            !root.join("attachments.pre-restore-ok").exists(),
            "its backup is cleaned up on success"
        );
    }

    #[test]
    fn restore_options_default_to_the_bounded_limits() {
        let bounded = RestoreOptions::new(false, false);
        assert_eq!(bounded.limits.max_db_bytes, MAX_DB_BYTES);
        assert_eq!(bounded.limits.max_attachment_bytes, MAX_ATTACHMENT_BYTES);
        assert_eq!(bounded.limits.max_total_bytes, MAX_TOTAL_RESTORE_BYTES);
        assert_eq!(bounded.limits.max_entries, MAX_RESTORE_ENTRIES);
        assert!(!bounded.force);

        let large = RestoreOptions::new(true, true);
        assert!(large.force);
        assert!(large.limits.max_db_bytes > MAX_DB_BYTES);
        assert!(large.limits.max_total_bytes > MAX_TOTAL_RESTORE_BYTES);
        assert!(large.limits.max_entries > MAX_RESTORE_ENTRIES);
        // Still finite: a decompression bomb must still hit a wall.
        assert!(large.limits.max_decompressed_bytes() < u64::MAX);
    }

    #[test]
    fn allow_large_restores_an_archive_the_default_limits_refuse() {
        // `write_dump` has no size ceiling, so a big enough instance can take
        // an honest backup that the bounded defaults reject. Standing in for a
        // 512 MiB database here: limits this small, perfectly good archive
        // exceeds.
        let (src_dir_tmp, src_db) = seed_data_dir("allow_large_src");
        let archive = src_dir_tmp.path().join("backup.tar.gz");
        let manifest = write_dump(&crate::db::open(&src_db).unwrap(), &src_db, &archive).unwrap();
        let tight = RestoreLimits {
            max_db_bytes: manifest.db_size_bytes - 1,
            ..RestoreLimits::default()
        };

        let error = inspect_archive(&archive, &tight).unwrap_err();
        assert!(
            error.to_string().contains("exceeds restore limit"),
            "got {error}"
        );
        inspect_archive(&archive, &RestoreLimits::trusted())
            .expect("--allow-large accepts a trusted oversized archive");

        let dst = temp_dir("allow_large_dst");
        let dst_db = dst.path().join(ARCHIVE_DB_NAME);
        let bounded = RestoreOptions {
            force: false,
            limits: tight,
        };
        assert!(run_restore_with(&archive, &dst_db, &bounded).is_err());
        assert!(!dst_db.exists(), "a refused restore installs nothing");

        let result = run_restore_with(&archive, &dst_db, &RestoreOptions::new(false, true))
            .expect("--allow-large completes the restore");
        assert_eq!(result.attachment_count, 2);
        assert!(dst_db.exists());
    }

    #[test]
    fn allow_large_still_rejects_a_hostile_archive() {
        // The escape hatch raises size ceilings and nothing else: every
        // structural check still runs under trusted limits.
        let dir_tmp = temp_dir("allow_large_hostile");
        let dir = dir_tmp.path();
        let archive = dir.join("evil.tar.gz");
        {
            let file = fs::File::create(&archive).unwrap();
            let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            let mut tar = tar::Builder::new(enc);
            let manifest = Manifest {
                lific_version: "x".into(),
                schema_version: 1,
                created_at: "now".into(),
                db_size_bytes: 13,
                attachment_count: 1,
                attachment_bytes: 5,
                archived_project_count: 0,
                archived_project_bytes: 0,
            };
            let mj = serde_json::to_vec(&manifest).unwrap();
            append_bytes(&mut tar, ARCHIVE_MANIFEST_NAME, &mj).unwrap();
            append_bytes(&mut tar, ARCHIVE_DB_NAME, b"not a real db").unwrap();
            append_bytes(&mut tar, "attachments/sub/escape", b"pwned").unwrap();
            tar.into_inner().unwrap().finish().unwrap();
        }

        let error = inspect_archive(&archive, &RestoreLimits::trusted()).unwrap_err();
        assert!(matches!(error, LificError::BadRequest(_)), "got {error:?}");

        let dst = temp_dir("allow_large_hostile_dst");
        let dst_db = dst.path().join(ARCHIVE_DB_NAME);
        assert!(
            run_restore_with(&archive, &dst_db, &RestoreOptions::new(true, true)).is_err(),
            "--allow-large is not --skip-validation"
        );
        assert!(!dst_db.exists());
        assert!(!dst.path().join("attachments").join("sub").exists());
    }

    #[cfg(unix)]
    #[test]
    fn staging_file_does_not_follow_symlinks() {
        let dir = temp_dir("staging_symlink");
        let target = dir.path().join("outside");
        let link = dir.path().join("staged");
        fs::write(&target, b"untouched").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert!(create_staging_file(&link).is_err());
        assert_eq!(fs::read(target).unwrap(), b"untouched");
    }

    #[test]
    fn staged_database_rejects_blob_content_hash_mismatch() {
        let (dir_tmp, db_path) = seed_data_dir("content_hash");
        let dir = dir_tmp.path();
        let pool = crate::db::open(&db_path).unwrap();
        let sha = crate::storage::AttachmentStore::hash_bytes(b"good");
        {
            let conn = pool.write().unwrap();
            crate::db::queries::attachments::create_attachment(
                &conn,
                &sha,
                "note.txt",
                "text/plain",
                4,
                None,
            )
            .unwrap();
        }
        checkpoint_db_file(&db_path).unwrap();
        let staging = dir.join("staging");
        fs::create_dir_all(staging.join("attachments")).unwrap();
        fs::copy(&db_path, staging.join(ARCHIVE_DB_NAME)).unwrap();
        fs::write(staging.join("attachments").join(&sha), b"evil").unwrap();
        let manifest = Manifest {
            lific_version: env!("CARGO_PKG_VERSION").into(),
            schema_version: crate::db::migrate::latest_version(),
            created_at: String::new(),
            db_size_bytes: fs::metadata(&db_path).unwrap().len(),
            attachment_count: 1,
            attachment_bytes: 4,
            archived_project_count: 0,
            archived_project_bytes: 0,
        };

        let error =
            validate_staged_database(&staging, &manifest, &RestoreLimits::default()).unwrap_err();
        assert!(error.to_string().contains("content address"));
    }

    #[test]
    fn staged_database_rejects_blob_content_mime_mismatch() {
        let (dir_tmp, db_path) = seed_data_dir("content_mime");
        let dir = dir_tmp.path();
        let pool = crate::db::open(&db_path).unwrap();
        let sha = crate::storage::AttachmentStore::hash_bytes(b"good");
        {
            let conn = pool.write().unwrap();
            crate::db::queries::attachments::create_attachment(
                &conn,
                &sha,
                "image.png",
                "image/png",
                4,
                None,
            )
            .unwrap();
        }
        checkpoint_db_file(&db_path).unwrap();
        let staging = dir.join("staging");
        fs::create_dir_all(staging.join("attachments")).unwrap();
        fs::copy(&db_path, staging.join(ARCHIVE_DB_NAME)).unwrap();
        fs::write(staging.join("attachments").join(&sha), b"good").unwrap();
        let manifest = Manifest {
            lific_version: env!("CARGO_PKG_VERSION").into(),
            schema_version: crate::db::migrate::latest_version(),
            created_at: String::new(),
            db_size_bytes: fs::metadata(&db_path).unwrap().len(),
            attachment_count: 1,
            attachment_bytes: 4,
            archived_project_count: 0,
            archived_project_bytes: 0,
        };

        let error =
            validate_staged_database(&staging, &manifest, &RestoreLimits::default()).unwrap_err();
        assert!(error.to_string().contains("MIME image/png"));
    }

    #[test]
    fn staged_database_rejects_duplicate_hashes_with_inconsistent_sizes() {
        let (dir_tmp, db_path) = seed_data_dir("duplicate_sizes");
        let dir = dir_tmp.path();
        let pool = crate::db::open(&db_path).unwrap();
        let sha = crate::storage::AttachmentStore::hash_bytes(b"good");
        {
            let conn = pool.write().unwrap();
            for (filename, size) in [("one.txt", 4), ("two.txt", 5)] {
                crate::db::queries::attachments::create_attachment(
                    &conn,
                    &sha,
                    filename,
                    "text/plain",
                    size,
                    None,
                )
                .unwrap();
            }
        }
        checkpoint_db_file(&db_path).unwrap();
        let staging = dir.join("staging");
        fs::create_dir_all(staging.join("attachments")).unwrap();
        fs::copy(&db_path, staging.join(ARCHIVE_DB_NAME)).unwrap();
        fs::write(staging.join("attachments").join(&sha), b"good").unwrap();
        let manifest = Manifest {
            lific_version: env!("CARGO_PKG_VERSION").into(),
            schema_version: crate::db::migrate::latest_version(),
            created_at: String::new(),
            db_size_bytes: fs::metadata(&db_path).unwrap().len(),
            attachment_count: 1,
            attachment_bytes: 4,
            archived_project_count: 0,
            archived_project_bytes: 0,
        };

        let error =
            validate_staged_database(&staging, &manifest, &RestoreLimits::default()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("invalid content address, MIME, or size")
        );
    }

    #[test]
    fn staged_database_rejects_unreferenced_blob_content_hash_mismatch() {
        let (dir_tmp, db_path) = seed_data_dir("unreferenced_content_hash");
        let dir = dir_tmp.path();
        let pool = crate::db::open(&db_path).unwrap();
        checkpoint_db_file(&db_path).unwrap();
        drop(pool);

        let staging = dir.join("staging");
        fs::create_dir_all(staging.join("attachments")).unwrap();
        fs::copy(&db_path, staging.join(ARCHIVE_DB_NAME)).unwrap();
        let sha = crate::storage::AttachmentStore::hash_bytes(b"good");
        fs::write(staging.join("attachments").join(&sha), b"evil").unwrap();
        let manifest = Manifest {
            lific_version: env!("CARGO_PKG_VERSION").into(),
            schema_version: crate::db::migrate::latest_version(),
            created_at: String::new(),
            db_size_bytes: fs::metadata(&db_path).unwrap().len(),
            attachment_count: 1,
            attachment_bytes: 4,
            archived_project_count: 0,
            archived_project_bytes: 0,
        };

        let error =
            validate_staged_database(&staging, &manifest, &RestoreLimits::default()).unwrap_err();
        assert!(error.to_string().contains("content address"));
    }

    #[test]
    fn staged_database_requires_attachment_table() {
        let (dir_tmp, db_path) = seed_data_dir("missing_attachments_table");
        let dir = dir_tmp.path();
        checkpoint_db_file(&db_path).unwrap();
        let staging = dir.join("staging");
        fs::create_dir_all(staging.join("attachments")).unwrap();
        fs::copy(&db_path, staging.join(ARCHIVE_DB_NAME)).unwrap();
        {
            let conn = rusqlite::Connection::open(staging.join(ARCHIVE_DB_NAME)).unwrap();
            conn.execute_batch(
                "DROP TRIGGER IF EXISTS attachments_fts_ai;
                 DROP TRIGGER IF EXISTS attachments_fts_au;
                 DROP TRIGGER IF EXISTS attachments_fts_ad;
                 DROP TABLE attachments;",
            )
            .unwrap();
        }
        let manifest = Manifest {
            lific_version: env!("CARGO_PKG_VERSION").into(),
            schema_version: crate::db::migrate::latest_version(),
            created_at: String::new(),
            db_size_bytes: 0,
            attachment_count: 0,
            attachment_bytes: 0,
            archived_project_count: 0,
            archived_project_bytes: 0,
        };

        let error =
            validate_staged_database(&staging, &manifest, &RestoreLimits::default()).unwrap_err();
        assert!(error.to_string().contains("attachments table"));
    }

    #[test]
    fn staged_database_rejects_missing_or_mismatched_attachment_metadata() {
        let (dir_tmp, db_path) = seed_data_dir("staged_metadata");
        let dir = dir_tmp.path();
        let pool = crate::db::open(&db_path).unwrap();
        let sha = "a".repeat(64);
        {
            let conn = pool.write().unwrap();
            crate::db::queries::attachments::create_attachment(
                &conn,
                &sha,
                "note.txt",
                "text/plain",
                4,
                None,
            )
            .unwrap();
        }
        checkpoint_db_file(&db_path).unwrap();
        let staging = dir.join("staging");
        fs::create_dir_all(staging.join("attachments")).unwrap();
        fs::copy(&db_path, staging.join(ARCHIVE_DB_NAME)).unwrap();
        let manifest = Manifest {
            lific_version: env!("CARGO_PKG_VERSION").into(),
            schema_version: crate::db::migrate::latest_version(),
            created_at: String::new(),
            db_size_bytes: fs::metadata(&db_path).unwrap().len(),
            attachment_count: 1,
            attachment_bytes: 4,
            archived_project_count: 0,
            archived_project_bytes: 0,
        };
        let error =
            validate_staged_database(&staging, &manifest, &RestoreLimits::default()).unwrap_err();
        assert!(error.to_string().contains("missing attachment"));
    }

    // ── archived projects ────────────────────────────────────

    const ARCHIVED_ALPHA: (&str, &[u8]) = ("ALPHA-1.lific.tar.gz", b"alpha project archive");
    const ARCHIVED_BETA: (&str, &[u8]) = ("beta_2.v1.lific.tar.gz", b"beta project archive bytes");

    /// Write two qualifying archived-project files into the data dir beside
    /// `db_path`, owner-only like the app writes them. Returns the dir.
    fn seed_archived_projects(db_path: &Path) -> PathBuf {
        let dir = archived_projects_dir_for(db_path);
        fs::create_dir_all(&dir).unwrap();
        set_owner_only_dir(&dir).unwrap();
        for (name, bytes) in [ARCHIVED_ALPHA, ARCHIVED_BETA] {
            fs::write(dir.join(name), bytes).unwrap();
            set_owner_only(&dir.join(name)).unwrap();
        }
        dir
    }

    /// Build an archive with a manifest declaring one archived project, a
    /// placeholder database, and one entry under a hand-written name. The
    /// name goes straight into the header bytes, because `tar` refuses to
    /// write a `..` component through its path setters.
    fn archive_with_archived_project_entry(path: &Path, entry_name: &str) {
        let file = fs::File::create(path).unwrap();
        let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut tar = tar::Builder::new(enc);
        let body = b"project bytes";
        let manifest = Manifest {
            lific_version: "x".into(),
            schema_version: 1,
            created_at: "now".into(),
            db_size_bytes: 13,
            attachment_count: 0,
            attachment_bytes: 0,
            archived_project_count: 1,
            archived_project_bytes: body.len() as u64,
        };
        append_bytes(
            &mut tar,
            ARCHIVE_MANIFEST_NAME,
            &serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        append_bytes(&mut tar, ARCHIVE_DB_NAME, b"not a real db").unwrap();
        let mut header = tar::Header::new_old();
        let raw = entry_name.as_bytes();
        header.as_old_mut().name[..raw.len()].copy_from_slice(raw);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(body.len() as u64);
        header.set_mode(0o600);
        header.set_cksum();
        tar.append(&header, &body[..]).unwrap();
        tar.into_inner().unwrap().finish().unwrap();
    }

    #[test]
    fn archived_project_names_follow_the_published_rule() {
        for good in [
            "a.lific.tar.gz",
            "ALPHA-1.lific.tar.gz",
            "beta_2.v1.lific.tar.gz",
            "9.lific.tar.gz",
        ] {
            assert!(valid_archived_project_name(good), "{good} must qualify");
        }
        let longest = format!("{}.lific.tar.gz", "a".repeat(160 - ".lific.tar.gz".len()));
        assert_eq!(longest.len(), 160);
        assert!(valid_archived_project_name(&longest));
        for bad in [
            ".lific.tar.gz",
            ".tmpAbC12",
            ".hidden.lific.tar.gz",
            "-dash.lific.tar.gz",
            "_under.lific.tar.gz",
            "name.tar.gz",
            "name.lific.tar.gz.tmp",
            "sub/name.lific.tar.gz",
            "sub\\name.lific.tar.gz",
            "spa ce.lific.tar.gz",
            "caf\u{e9}.lific.tar.gz",
            &format!("{}.lific.tar.gz", "a".repeat(161 - ".lific.tar.gz".len())),
        ] {
            assert!(!valid_archived_project_name(bad), "{bad} must not qualify");
        }
        assert_eq!(
            validate_archived_project_entry("archived-projects/ALPHA-1.lific.tar.gz").unwrap(),
            "ALPHA-1.lific.tar.gz"
        );
        assert!(validate_archived_project_entry("archived-projects/../x.lific.tar.gz").is_err());
        assert!(validate_archived_project_entry("archived-projects/").is_err());
        assert!(validate_archived_project_entry("attachments/x.lific.tar.gz").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn dump_includes_archived_project_files_and_skips_everything_else() {
        use std::os::unix::fs::symlink;

        let (dir_tmp, db_path) = seed_data_dir("archived_dump");
        let dir = dir_tmp.path();
        let archived = seed_archived_projects(&db_path);
        // Not ours: an in-progress temp file, a stray name, a hidden name, a
        // name over the length cap, a directory and a symlink that both carry
        // a qualifying name.
        fs::write(archived.join(".tmpXy12Ab"), b"partial").unwrap();
        fs::write(archived.join("notes.txt"), b"stray").unwrap();
        fs::write(archived.join(".hidden.lific.tar.gz"), b"hidden").unwrap();
        let too_long = format!("{}.lific.tar.gz", "a".repeat(161 - ".lific.tar.gz".len()));
        fs::write(archived.join(&too_long), b"long").unwrap();
        fs::create_dir(archived.join("nested.lific.tar.gz")).unwrap();
        let outside = dir.join("outside-secret");
        fs::write(&outside, b"outside secret").unwrap();
        symlink(&outside, archived.join("link.lific.tar.gz")).unwrap();

        let out = dir.join("out.tar.gz");
        let manifest = write_dump(&crate::db::open(&db_path).unwrap(), &db_path, &out).unwrap();

        let entries = archive_entries(&out);
        let mut expected = vec![
            format!("{ARCHIVE_ARCHIVED_PROJECTS_PREFIX}{}", ARCHIVED_ALPHA.0),
            format!("{ARCHIVE_ARCHIVED_PROJECTS_PREFIX}{}", ARCHIVED_BETA.0),
        ];
        expected.sort();
        let mut found: Vec<String> = entries
            .into_iter()
            .filter(|e| e.starts_with(ARCHIVE_ARCHIVED_PROJECTS_PREFIX))
            .collect();
        found.sort();
        assert_eq!(found, expected, "only qualifying regular files are dumped");

        assert_eq!(manifest.archived_project_count, 2);
        assert_eq!(
            manifest.archived_project_bytes,
            (ARCHIVED_ALPHA.1.len() + ARCHIVED_BETA.1.len()) as u64
        );
        assert_eq!(
            manifest.attachment_count, 2,
            "attachments are counted apart"
        );
        assert_eq!(
            read_manifest(&out, &RestoreLimits::default())
                .unwrap()
                .archived_project_count,
            2,
            "the counts are written into manifest.json"
        );
        inspect_archive(&out, &RestoreLimits::default()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn dump_refuses_a_hard_linked_archived_project() {
        let (dir_tmp, db_path) = seed_data_dir("archived_hardlink");
        let archived = seed_archived_projects(&db_path);
        let outside = dir_tmp.path().join("outside");
        fs::write(&outside, b"outside").unwrap();
        fs::hard_link(&outside, archived.join("linked.lific.tar.gz")).unwrap();

        let out = dir_tmp.path().join("out.tar.gz");
        let error = write_dump(&crate::db::open(&db_path).unwrap(), &db_path, &out).unwrap_err();
        assert!(error.to_string().contains("hard-linked"), "got {error}");
        assert!(!out.exists());
    }

    #[test]
    fn dump_without_an_archived_projects_dir_declares_none() {
        let (dir_tmp, db_path) = seed_data_dir("archived_absent");
        let out = dir_tmp.path().join("out.tar.gz");
        let manifest = write_dump(&crate::db::open(&db_path).unwrap(), &db_path, &out).unwrap();
        assert_eq!(manifest.archived_project_count, 0);
        assert_eq!(manifest.archived_project_bytes, 0);
        assert!(
            !archive_entries(&out)
                .iter()
                .any(|e| e.starts_with(ARCHIVE_ARCHIVED_PROJECTS_PREFIX))
        );
    }

    #[test]
    fn manifest_from_before_archived_projects_still_parses() {
        let json = r#"{
            "lific_version": "2.10.0",
            "schema_version": 56,
            "created_at": "2026-09-25T00:00:00Z",
            "db_size_bytes": 4096,
            "attachment_count": 3,
            "attachment_bytes": 120
        }"#;
        let manifest: Manifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.attachment_count, 3);
        assert_eq!(manifest.archived_project_count, 0);
        assert_eq!(manifest.archived_project_bytes, 0);
    }

    #[cfg(unix)]
    #[test]
    fn restore_round_trips_archived_projects_byte_identical_and_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let (src_tmp, src_db) = seed_data_dir("archived_rt_src");
        seed_archived_projects(&src_db);
        let archive = src_tmp.path().join("backup.tar.gz");
        write_dump(&crate::db::open(&src_db).unwrap(), &src_db, &archive).unwrap();

        let dst = temp_dir("archived_rt_dst");
        let dst_db = dst.path().join(ARCHIVE_DB_NAME);
        let result = run_restore(&archive, &dst_db, false).unwrap();
        assert_eq!(result.archived_project_count, 2);
        assert_eq!(result.attachment_count, 2);

        let restored = dst.path().join(ARCHIVED_PROJECTS_DIR_NAME);
        assert_eq!(
            fs::metadata(&restored).unwrap().permissions().mode() & 0o777,
            0o700,
            "the restored directory is owner-only"
        );
        for (name, bytes) in [ARCHIVED_ALPHA, ARCHIVED_BETA] {
            let path = restored.join(name);
            assert_eq!(fs::read(&path).unwrap(), bytes, "{name} is byte-identical");
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600,
                "{name} is owner-only"
            );
        }
        assert_eq!(fs::read_dir(&restored).unwrap().count(), 2);
    }

    #[test]
    fn restore_of_a_dump_without_archived_projects_empties_the_live_directory() {
        // The restored database replaces the live one wholesale, so an
        // archived-project file left over from before would have no row
        // tracking it: unreachable and never cleaned up.
        let (src_tmp, src_db) = seed_data_dir("archived_stale_src");
        let archive = src_tmp.path().join("backup.tar.gz");
        write_dump(&crate::db::open(&src_db).unwrap(), &src_db, &archive).unwrap();

        let (dst_tmp, dst_db) = seed_data_dir("archived_stale_dst");
        let stale = seed_archived_projects(&dst_db);
        checkpoint_db_file(&dst_db).unwrap();

        let result = run_restore(&archive, &dst_db, true).unwrap();
        assert_eq!(result.archived_project_count, 0);

        assert!(stale.is_dir(), "the directory exists after the restore");
        assert_eq!(
            fs::read_dir(&stale).unwrap().count(),
            0,
            "stale archived-project files are gone"
        );
        let leftovers: Vec<String> = fs::read_dir(dst_tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(ARCHIVED_PROJECTS_DIR_NAME) && n.contains(".pre-restore-"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "the aside copy is deleted: {leftovers:?}"
        );
    }

    #[test]
    fn restore_rejects_bad_archived_project_entry_names() {
        for (tag, entry) in [
            ("dotdot", "archived-projects/../escape.lific.tar.gz"),
            ("nested", "archived-projects/sub/nested.lific.tar.gz"),
            ("hidden", "archived-projects/.hidden.lific.tar.gz"),
            ("stray", "archived-projects/notes.txt"),
            ("tmp", "archived-projects/.tmpAbC123"),
        ] {
            let dir = temp_dir(&format!("archived_bad_{tag}"));
            let archive = dir.path().join("evil.tar.gz");
            archive_with_archived_project_entry(&archive, entry);

            let error = inspect_archive(&archive, &RestoreLimits::default()).unwrap_err();
            assert!(
                matches!(error, LificError::BadRequest(_)),
                "{entry}: got {error:?}"
            );
            assert!(
                error.to_string().contains("archived project entry"),
                "{entry}: the name check rejects it, got {error}"
            );

            let dst = temp_dir(&format!("archived_bad_{tag}_dst"));
            let dst_db = dst.path().join(ARCHIVE_DB_NAME);
            assert!(run_restore(&archive, &dst_db, false).is_err(), "{entry}");
            assert!(!dst_db.exists(), "{entry}: nothing is installed");
            assert!(!dst.path().join(ARCHIVED_PROJECTS_DIR_NAME).exists());
            assert!(!dst.path().join("escape.lific.tar.gz").exists());
        }
    }

    #[test]
    fn restore_rejects_archived_projects_the_manifest_does_not_declare() {
        let (src_tmp, src_db) = seed_data_dir("archived_undeclared");
        seed_archived_projects(&src_db);
        let archive = src_tmp.path().join("backup.tar.gz");
        let mut manifest =
            write_dump(&crate::db::open(&src_db).unwrap(), &src_db, &archive).unwrap();
        manifest.archived_project_count = 0;
        manifest.archived_project_bytes = 0;
        let rewritten = src_tmp.path().join("undeclared.tar.gz");
        rewrite_archive_manifest(&archive, &rewritten, &manifest);

        let error = inspect_archive(&rewritten, &RestoreLimits::default()).unwrap_err();
        assert!(
            error.to_string().contains("manifest"),
            "an undeclared archived project is refused: {error}"
        );
    }

    #[test]
    fn restore_applies_the_per_file_archived_project_limit() {
        let (src_tmp, src_db) = seed_data_dir("archived_limit");
        seed_archived_projects(&src_db);
        let archive = src_tmp.path().join("backup.tar.gz");
        write_dump(&crate::db::open(&src_db).unwrap(), &src_db, &archive).unwrap();
        let tight = RestoreLimits {
            max_archived_project_bytes: ARCHIVED_ALPHA.1.len() as u64,
            ..RestoreLimits::default()
        };
        let error = inspect_archive(&archive, &tight).unwrap_err();
        assert!(
            error.to_string().contains("archived project exceeds"),
            "got {error}"
        );
        inspect_archive(&archive, &RestoreLimits::trusted()).unwrap();
    }

    /// Record an `archived_projects` row the way the app does when it
    /// archives a project.
    fn track_archived_project(db_path: &Path, file_name: &str, sha256: &str, size_bytes: i64) {
        let pool = crate::db::open(db_path).unwrap();
        let conn = pool.write().unwrap();
        conn.execute(
            "INSERT INTO archived_projects (identifier, name, file_name, sha256, size_bytes)
             VALUES ('ARC', 'Archived', ?1, ?2, ?3)",
            rusqlite::params![file_name, sha256, size_bytes],
        )
        .unwrap();
    }

    fn track_seeded(db_path: &Path, (name, bytes): (&str, &[u8])) {
        track_archived_project(
            db_path,
            name,
            &crate::storage::AttachmentStore::hash_bytes(bytes),
            bytes.len() as i64,
        );
    }

    /// Force-restore `archive` over a live data dir that holds healthy
    /// archived projects, assert it is refused naming `expected`, and assert
    /// the live archived projects were not replaced.
    fn assert_restore_refused_keeping_live_archives(tag: &str, archive: &Path, expected: &str) {
        let (dst_tmp, dst_db) = seed_data_dir(tag);
        let live = seed_archived_projects(&dst_db);
        let error = run_restore(archive, &dst_db, true).unwrap_err();
        assert!(matches!(error, LificError::BadRequest(_)), "got {error:?}");
        assert!(error.to_string().contains(expected), "got {error}");
        for (name, bytes) in [ARCHIVED_ALPHA, ARCHIVED_BETA] {
            assert_eq!(
                fs::read(live.join(name)).unwrap(),
                bytes,
                "the healthy live archive {name} is untouched"
            );
        }
        let conn = rusqlite::Connection::open(&dst_db).unwrap();
        let live_project: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM projects WHERE identifier = 'DMP'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(live_project, 1, "the live database is still in place");
        drop(dst_tmp);
    }

    #[test]
    fn restore_rejects_an_archived_project_row_whose_file_is_missing() {
        let (src_tmp, src_db) = seed_data_dir("archived_row_missing_src");
        seed_archived_projects(&src_db);
        track_seeded(&src_db, ARCHIVED_ALPHA);
        track_seeded(&src_db, ("gone.lific.tar.gz", b"never written"));
        let archive = src_tmp.path().join("backup.tar.gz");
        write_dump(&crate::db::open(&src_db).unwrap(), &src_db, &archive).unwrap();

        let fresh = temp_dir("archived_row_missing_fresh");
        let fresh_db = fresh.path().join(ARCHIVE_DB_NAME);
        let error = run_restore(&archive, &fresh_db, false).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("missing archived project file gone.lific.tar.gz"),
            "got {error}"
        );
        assert!(!fresh_db.exists(), "nothing is installed");

        assert_restore_refused_keeping_live_archives(
            "archived_row_missing_dst",
            &archive,
            "gone.lific.tar.gz",
        );
    }

    #[test]
    fn restore_rejects_an_archived_project_whose_bytes_differ_from_its_row() {
        // Same size, different bytes: only the digest can tell.
        let (src_tmp, src_db) = seed_data_dir("archived_row_hash_src");
        seed_archived_projects(&src_db);
        let (name, bytes) = ARCHIVED_ALPHA;
        let impostor = vec![b'x'; bytes.len()];
        track_archived_project(
            &src_db,
            name,
            &crate::storage::AttachmentStore::hash_bytes(&impostor),
            bytes.len() as i64,
        );
        let archive = src_tmp.path().join("backup.tar.gz");
        write_dump(&crate::db::open(&src_db).unwrap(), &src_db, &archive).unwrap();
        assert_restore_refused_keeping_live_archives("archived_row_hash_dst", &archive, name);
        let error = run_restore(
            &archive,
            &temp_dir("archived_row_hash_fresh")
                .path()
                .join(ARCHIVE_DB_NAME),
            false,
        )
        .unwrap_err();
        assert!(error.to_string().contains("sha256"), "got {error}");

        // A size that disagrees with the row is refused too.
        let (src_tmp, src_db) = seed_data_dir("archived_row_size_src");
        seed_archived_projects(&src_db);
        track_archived_project(
            &src_db,
            name,
            &crate::storage::AttachmentStore::hash_bytes(bytes),
            bytes.len() as i64 + 1,
        );
        let archive = src_tmp.path().join("backup.tar.gz");
        write_dump(&crate::db::open(&src_db).unwrap(), &src_db, &archive).unwrap();
        assert_restore_refused_keeping_live_archives(
            "archived_row_size_dst",
            &archive,
            "its row records",
        );
    }

    #[test]
    fn restore_accepts_an_archived_project_file_no_row_tracks() {
        // A crash between writing the file and committing its row leaves a
        // stray. It is harmless and must not make the backup unrestorable.
        let (src_tmp, src_db) = seed_data_dir("archived_stray_src");
        seed_archived_projects(&src_db);
        track_seeded(&src_db, ARCHIVED_ALPHA);
        let archive = src_tmp.path().join("backup.tar.gz");
        write_dump(&crate::db::open(&src_db).unwrap(), &src_db, &archive).unwrap();

        let dst = temp_dir("archived_stray_dst");
        let dst_db = dst.path().join(ARCHIVE_DB_NAME);
        let result = run_restore(&archive, &dst_db, false).expect("a stray file is accepted");
        assert_eq!(result.archived_project_count, 2);
        let restored = dst.path().join(ARCHIVED_PROJECTS_DIR_NAME);
        for (name, bytes) in [ARCHIVED_ALPHA, ARCHIVED_BETA] {
            assert_eq!(fs::read(restored.join(name)).unwrap(), bytes);
        }
    }

    #[test]
    fn restore_accepts_a_database_from_before_the_archived_projects_table() {
        let dir_tmp = temp_dir("archived_legacy");
        let dir = dir_tmp.path();
        let legacy_db = dir.join("legacy.db");
        {
            let conn = rusqlite::Connection::open(&legacy_db).unwrap();
            conn.execute_batch(
                "CREATE TABLE _migrations (version INTEGER PRIMARY KEY);
                 INSERT INTO _migrations (version) VALUES (30);",
            )
            .unwrap();
        }
        let (name, bytes) = ARCHIVED_ALPHA;
        let manifest = Manifest {
            lific_version: "old".into(),
            schema_version: 30,
            created_at: "now".into(),
            db_size_bytes: fs::metadata(&legacy_db).unwrap().len(),
            attachment_count: 0,
            attachment_bytes: 0,
            archived_project_count: 1,
            archived_project_bytes: bytes.len() as u64,
        };
        let archive = dir.join("legacy.tar.gz");
        let file = fs::File::create(&archive).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        append_bytes(
            &mut builder,
            ARCHIVE_MANIFEST_NAME,
            &serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        builder
            .append_path_with_name(&legacy_db, ARCHIVE_DB_NAME)
            .unwrap();
        append_bytes(
            &mut builder,
            &format!("{ARCHIVE_ARCHIVED_PROJECTS_PREFIX}{name}"),
            bytes,
        )
        .unwrap();
        builder.into_inner().unwrap().finish().unwrap();

        let dst = temp_dir("archived_legacy_dst");
        let dst_db = dst.path().join(ARCHIVE_DB_NAME);
        run_restore(&archive, &dst_db, false).expect("a database without the table restores");
        assert_eq!(
            fs::read(dst.path().join(ARCHIVED_PROJECTS_DIR_NAME).join(name)).unwrap(),
            bytes
        );
    }

    #[test]
    fn install_rollback_restores_the_previous_archived_projects_directory() {
        // The staged archived-projects directory is the last thing installed.
        // Taking it away makes that final rename fail after the database and
        // attachments are already live, so every swap has to be undone.
        let (dir_tmp, db_path, staging, sha) = seed_install_fixture("archived_rollback");
        let root = dir_tmp.path();
        let live_archived = seed_archived_projects(&db_path);
        fs::remove_dir(staging.join(ARCHIVED_PROJECTS_DIR_NAME)).unwrap();

        let error = install_restore(&staging, &db_path, "boom").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("install restored archived-projects"),
            "the original cause surfaces: {error}"
        );

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let live: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM projects WHERE identifier = 'LIV'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(live, 1, "the user's database is back in place");
        assert_eq!(
            fs::read(root.join("attachments").join(&sha)).unwrap(),
            b"live blob",
            "the previous attachments are back"
        );
        for (name, bytes) in [ARCHIVED_ALPHA, ARCHIVED_BETA] {
            assert_eq!(
                fs::read(live_archived.join(name)).unwrap(),
                bytes,
                "the previous archived projects are back"
            );
        }
        for aside in [
            format!("{ARCHIVE_DB_NAME}.pre-restore-boom"),
            "attachments.pre-restore-boom".to_string(),
            format!("{ARCHIVED_PROJECTS_DIR_NAME}.pre-restore-boom"),
        ] {
            assert!(!root.join(&aside).exists(), "{aside} is not stranded");
        }
    }

    #[test]
    fn install_refuses_a_taken_archived_projects_backup_path_before_moving_anything() {
        let (dir_tmp, db_path, staging, sha) = seed_install_fixture("archived_clash");
        let root = dir_tmp.path();
        let live_archived = seed_archived_projects(&db_path);
        fs::create_dir(root.join(format!("{ARCHIVED_PROJECTS_DIR_NAME}.pre-restore-clash")))
            .unwrap();

        let error = install_restore(&staging, &db_path, "clash").unwrap_err();
        assert!(matches!(error, LificError::Conflict(_)), "got {error:?}");
        assert!(
            error
                .to_string()
                .contains("archived project backup already exists"),
            "got {error}"
        );
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let live: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM projects WHERE identifier = 'LIV'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(live, 1);
        assert!(root.join("attachments").join(&sha).exists());
        assert!(live_archived.join(ARCHIVED_ALPHA.0).exists());
        assert!(!root.join("attachments.pre-restore-clash").exists());
    }

    #[test]
    fn install_replaces_the_archived_projects_directory_wholesale() {
        let (dir_tmp, db_path, staging, _sha) = seed_install_fixture("archived_install_ok");
        let root = dir_tmp.path();
        let live_archived = seed_archived_projects(&db_path);
        fs::write(
            staging
                .join(ARCHIVED_PROJECTS_DIR_NAME)
                .join("restored.lific.tar.gz"),
            b"restored project",
        )
        .unwrap();

        install_restore(&staging, &db_path, "ok").unwrap();

        assert_eq!(
            fs::read(live_archived.join("restored.lific.tar.gz")).unwrap(),
            b"restored project"
        );
        assert!(!live_archived.join(ARCHIVED_ALPHA.0).exists());
        assert!(
            !root
                .join(format!("{ARCHIVED_PROJECTS_DIR_NAME}.pre-restore-ok"))
                .exists(),
            "the aside copy is removed on success"
        );
    }

    // Test helper: re-pack an archive but overwrite the manifest's
    // schema_version, to simulate an archive from a newer binary.
    fn rewrite_archive_with_schema(src: &Path, dst: &Path, schema_version: i64) {
        let mut manifest = read_manifest(src, &RestoreLimits::default()).unwrap();
        manifest.schema_version = schema_version;
        rewrite_archive_manifest(src, dst, &manifest);
    }

    fn rewrite_archive_manifest(src: &Path, dst: &Path, manifest: &Manifest) {
        let mj = serde_json::to_vec_pretty(&manifest).unwrap();

        let out = fs::File::create(dst).unwrap();
        let enc = flate2::write::GzEncoder::new(out, flate2::Compression::default());
        let mut builder = tar::Builder::new(enc);
        append_bytes(&mut builder, ARCHIVE_MANIFEST_NAME, &mj).unwrap();

        // Copy the db + attachments through unchanged.
        let file = fs::File::open(src).unwrap();
        let dec = flate2::read::GzDecoder::new(file);
        let mut tar = tar::Archive::new(dec);
        for entry in tar.entries().unwrap() {
            let mut entry = entry.unwrap();
            let name = entry.path().unwrap().to_string_lossy().to_string();
            if name == ARCHIVE_MANIFEST_NAME {
                continue;
            }
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut buf).unwrap();
            append_bytes(&mut builder, &name, &buf).unwrap();
        }
        let enc = builder.into_inner().unwrap();
        enc.finish().unwrap();
    }
}
