//! LIF-266: backup as an interface — `lific dump` / `lific restore`.
//!
//! The data set is no longer a single file. Since attachments (LIF-262) it is
//! `lific.db` plus a content-addressed `attachments/` sidecar dir living beside
//! it (blobs named by their sha256; in-progress writes carry a `.tmp`
//! extension — see [`crate::storage`]). A DB snapshot alone would restore
//! metadata rows pointing at missing blobs.
//!
//! This module follows the `gitea dump` pattern: one command produces one
//! self-contained, timestamped `lific_YYYYMMDD_HHMMSS.tar.gz` archive with
//! everything needed to restore — the DB, every attachment blob, and a
//! `manifest.json` describing what's inside. [`restore`] validates and unpacks
//! it back into a data dir. The interval backup task (`src/backup.rs`) emits
//! the *same* artifact via [`write_dump`], so there is exactly one backup shape.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::db::DbPool;
use crate::error::LificError;

/// The DB filename inside every archive, independent of the on-disk name.
pub const ARCHIVE_DB_NAME: &str = "lific.db";
/// The manifest filename inside every archive.
pub const ARCHIVE_MANIFEST_NAME: &str = "manifest.json";
/// The prefix under which attachment blobs are stored inside the archive.
pub const ARCHIVE_ATTACHMENTS_PREFIX: &str = "attachments/";

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

/// Take a consistent snapshot of the live DB into `dest` using `VACUUM INTO`.
///
/// `VACUUM INTO` runs on a read connection, holds no long writer lock, and
/// compacts + snapshots in one step — safe while the server is running. The
/// destination must not already exist (SQLite requirement).
fn snapshot_db(pool: &DbPool, dest: &Path) -> Result<(), LificError> {
    if dest.exists() {
        std::fs::remove_file(dest)
            .map_err(|e| LificError::Internal(format!("clear snapshot target: {e}")))?;
    }
    let conn = pool.read()?;
    // Parameterized VACUUM INTO with the destination path as a bound value.
    conn.execute("VACUUM INTO ?1", [&dest.to_string_lossy()])
        .map_err(|e| LificError::Internal(format!("VACUUM INTO snapshot failed: {e}")))?;
    Ok(())
}

/// Set 0600 permissions on a file (owner-only) on Unix. No-op elsewhere.
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

/// Write a self-contained dump archive to `out_path`.
///
/// Shared code path used by both `lific dump` and the interval backup task.
/// Produces a gzip-compressed tar containing `lific.db` (a consistent snapshot
/// via `VACUUM INTO`), every non-`.tmp` attachment blob under `attachments/`,
/// and `manifest.json`. The finished file is chmod 0600 (it contains the whole
/// DB).
///
/// Returns the [`Manifest`] that was written, so callers can log/print it.
pub fn write_dump(pool: &DbPool, db_path: &Path, out_path: &Path) -> Result<Manifest, LificError> {
    // Snapshot the DB to a temp file next to the output, so the archive holds a
    // consistent point-in-time copy rather than a possibly-mid-write live file.
    let tmp_db = out_path.with_extension("dbsnapshot.tmp");
    snapshot_db(pool, &tmp_db)?;

    // Staging path for the archive itself; the closure writes here and
    // atomically renames into place on success. Declared out here so the
    // error path below can clean up a partial archive (LIF-329).
    let tmp_archive = out_path.with_extension("archive.tmp");

    // Guard: always clean the temp snapshot even on the error paths below.
    let result = (|| {
        let db_size_bytes = std::fs::metadata(&tmp_db).map(|m| m.len()).unwrap_or(0);

        // Gather attachment blobs (skip .tmp in-progress writes).
        let attachments_dir = attachments_dir_for(db_path);
        let mut blobs: Vec<(String, PathBuf, u64)> = Vec::new();
        let mut attachment_bytes: u64 = 0;
        if attachments_dir.is_dir() {
            for entry in std::fs::read_dir(&attachments_dir)
                .map_err(|e| LificError::Internal(format!("read attachments dir: {e}")))?
            {
                let entry = entry
                    .map_err(|e| LificError::Internal(format!("read attachments entry: {e}")))?;
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) == Some("tmp") {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                attachment_bytes += size;
                blobs.push((name.to_string(), path.clone(), size));
            }
        }

        let manifest = Manifest {
            lific_version: env!("CARGO_PKG_VERSION").to_string(),
            schema_version: schema_version(pool),
            created_at: chrono::Utc::now().to_rfc3339(),
            db_size_bytes,
            attachment_count: blobs.len() as u64,
            attachment_bytes,
        };
        let manifest_json = serde_json::to_vec_pretty(&manifest)
            .map_err(|e| LificError::Internal(format!("serialize manifest: {e}")))?;

        // Build the archive into a temp file, then atomically rename into place
        // so a partial write is never observed at the final path.
        {
            let file = std::fs::File::create(&tmp_archive)
                .map_err(|e| LificError::Internal(format!("create archive: {e}")))?;
            let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            let mut tar = tar::Builder::new(enc);

            // manifest.json
            append_bytes(&mut tar, ARCHIVE_MANIFEST_NAME, &manifest_json)?;
            // lific.db (from the snapshot file)
            tar.append_path_with_name(&tmp_db, ARCHIVE_DB_NAME)
                .map_err(|e| LificError::Internal(format!("append db to archive: {e}")))?;
            // attachments/<sha256>
            for (name, path, _size) in &blobs {
                let entry_name = format!("{ARCHIVE_ATTACHMENTS_PREFIX}{name}");
                tar.append_path_with_name(path, &entry_name).map_err(|e| {
                    LificError::Internal(format!("append attachment {name}: {e}"))
                })?;
            }

            let enc = tar
                .into_inner()
                .map_err(|e| LificError::Internal(format!("finalize tar: {e}")))?;
            enc.finish()
                .map_err(|e| LificError::Internal(format!("finalize gzip: {e}")))?;
        }

        set_owner_only(&tmp_archive)
            .map_err(|e| LificError::Internal(format!("chmod archive: {e}")))?;
        std::fs::rename(&tmp_archive, out_path)
            .map_err(|e| LificError::Internal(format!("finalize archive: {e}")))?;

        Ok(manifest)
    })();

    let _ = std::fs::remove_file(&tmp_db);
    if result.is_err() {
        // A failure after the archive staging file was created would otherwise
        // strand a partial `*.archive.tmp` that rotation never touches
        // (LIF-329).
        let _ = std::fs::remove_file(&tmp_archive);
    }
    result
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
            .map(|d| d.as_secs())
            .unwrap_or(0),
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
    Ok(rest.to_string())
}

/// Read and validate an archive's manifest + entry list without extracting.
/// Returns the parsed manifest. Rejects archives missing `manifest.json` or
/// `lific.db`, and any attachment entry that fails [`validate_attachment_entry`].
pub fn inspect_archive(archive: &Path) -> Result<Manifest, LificError> {
    let manifest = read_manifest(archive)?;

    // Second pass: validate every entry name (traversal guard) and require the
    // DB member is present.
    let file = std::fs::File::open(archive)
        .map_err(|e| LificError::BadRequest(format!("open archive: {e}")))?;
    let dec = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(dec);
    let mut has_db = false;
    for entry in tar
        .entries()
        .map_err(|e| LificError::BadRequest(format!("read archive entries: {e}")))?
    {
        let entry = entry.map_err(|e| LificError::BadRequest(format!("read entry: {e}")))?;
        let path = entry
            .path()
            .map_err(|e| LificError::BadRequest(format!("entry path: {e}")))?;
        let name = path.to_string_lossy().replace('\\', "/");
        if name == ARCHIVE_MANIFEST_NAME {
            continue;
        } else if name == ARCHIVE_DB_NAME {
            has_db = true;
        } else if name.starts_with(ARCHIVE_ATTACHMENTS_PREFIX) {
            validate_attachment_entry(&name)?;
        } else {
            return Err(LificError::BadRequest(format!(
                "rejected unexpected archive entry: {name}"
            )));
        }
    }
    if !has_db {
        return Err(LificError::BadRequest(
            "archive is missing lific.db".into(),
        ));
    }
    Ok(manifest)
}

/// Read just the manifest from the archive (first matching entry).
fn read_manifest(archive: &Path) -> Result<Manifest, LificError> {
    let file = std::fs::File::open(archive)
        .map_err(|e| LificError::BadRequest(format!("open archive: {e}")))?;
    let dec = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(dec);
    for entry in tar
        .entries()
        .map_err(|e| LificError::BadRequest(format!("read archive entries: {e}")))?
    {
        let mut entry = entry.map_err(|e| LificError::BadRequest(format!("read entry: {e}")))?;
        let path = entry
            .path()
            .map_err(|e| LificError::BadRequest(format!("entry path: {e}")))?;
        if path.to_string_lossy() == ARCHIVE_MANIFEST_NAME {
            let mut buf = String::new();
            entry
                .read_to_string(&mut buf)
                .map_err(|e| LificError::BadRequest(format!("read manifest: {e}")))?;
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
    std::fs::metadata(&wal).map(|m| m.len() > 0).unwrap_or(false)
}

/// Run `lific restore`: validate the archive, then stage-extract it into the
/// data dir at `db_path`. Refuses to clobber an existing DB unless `force`;
/// with `force`, moves the existing DB + `-wal`/`-shm` aside. Refuses archives
/// created by a newer Lific (higher schema_version than this binary).
pub fn run_restore(
    archive: &Path,
    db_path: &Path,
    force: bool,
) -> Result<RestoreResult, LificError> {
    let manifest = inspect_archive(archive)?;

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
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&data_dir)
        .map_err(|e| LificError::Internal(format!("create data dir: {e}")))?;

    // Existing-DB guard.
    let mut moved_existing_to = None;
    if db_path.exists() {
        if !force {
            return Err(LificError::Conflict(format!(
                "{} already exists; pass --force to restore over it (stop the server first)",
                db_path.display()
            )));
        }
        // --force: move the existing DB aside rather than deleting, so a
        // mistaken restore is recoverable. First checkpoint its WAL into the
        // main file so the moved-aside `.db` is self-contained (the WAL/SHM are
        // named after the live path and won't follow a rename). Best-effort —
        // if the server holds the db open we still move what's on disk.
        checkpoint_db_file(db_path);
        let ts = archive_timestamp();
        let suffix = format!("pre-restore-{ts}");
        let dest = PathBuf::from(format!("{}.{suffix}", db_path.display()));
        std::fs::rename(db_path, &dest)
            .map_err(|e| LificError::Internal(format!("move existing db aside: {e}")))?;
        // Discard the now-redundant WAL/SHM (already folded into the moved db).
        for ext in ["-wal", "-shm"] {
            let side = PathBuf::from(format!("{}{ext}", db_path.display()));
            let _ = std::fs::remove_file(&side);
        }
        moved_existing_to = Some(dest);
    }

    // Staged extraction: unpack into a temp dir next to the data dir, then move
    // into place. A failure mid-extract leaves the original data dir untouched.
    let staging = data_dir.join(format!(".lific-restore-{}", archive_timestamp()));
    if staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    std::fs::create_dir_all(staging.join("attachments"))
        .map_err(|e| LificError::Internal(format!("create staging dir: {e}")))?;

    let extract = (|| -> Result<u64, LificError> {
        let file = std::fs::File::open(archive)
            .map_err(|e| LificError::BadRequest(format!("open archive: {e}")))?;
        let dec = flate2::read::GzDecoder::new(file);
        let mut tar = tar::Archive::new(dec);
        let mut attachment_count = 0u64;
        for entry in tar
            .entries()
            .map_err(|e| LificError::BadRequest(format!("read archive entries: {e}")))?
        {
            let mut entry =
                entry.map_err(|e| LificError::BadRequest(format!("read entry: {e}")))?;
            let epath = entry
                .path()
                .map_err(|e| LificError::BadRequest(format!("entry path: {e}")))?;
            let name = epath.to_string_lossy().replace('\\', "/");
            if name == ARCHIVE_MANIFEST_NAME {
                // Persist the manifest alongside the restored DB for provenance.
                let mut buf = Vec::new();
                entry
                    .read_to_end(&mut buf)
                    .map_err(|e| LificError::BadRequest(format!("read manifest: {e}")))?;
                std::fs::write(staging.join(ARCHIVE_MANIFEST_NAME), &buf)
                    .map_err(|e| LificError::Internal(format!("write manifest: {e}")))?;
            } else if name == ARCHIVE_DB_NAME {
                let mut buf = Vec::new();
                entry
                    .read_to_end(&mut buf)
                    .map_err(|e| LificError::BadRequest(format!("read db from archive: {e}")))?;
                std::fs::write(staging.join(ARCHIVE_DB_NAME), &buf)
                    .map_err(|e| LificError::Internal(format!("write db: {e}")))?;
            } else if name.starts_with(ARCHIVE_ATTACHMENTS_PREFIX) {
                let bare = validate_attachment_entry(&name)?;
                let mut buf = Vec::new();
                entry
                    .read_to_end(&mut buf)
                    .map_err(|e| LificError::BadRequest(format!("read attachment: {e}")))?;
                std::fs::write(staging.join("attachments").join(&bare), &buf)
                    .map_err(|e| LificError::Internal(format!("write attachment: {e}")))?;
                attachment_count += 1;
            } else {
                return Err(LificError::BadRequest(format!(
                    "rejected unexpected archive entry: {name}"
                )));
            }
        }
        if !staging.join(ARCHIVE_DB_NAME).exists() {
            return Err(LificError::BadRequest(
                "archive is missing lific.db".into(),
            ));
        }
        Ok(attachment_count)
    })();

    let attachment_count = match extract {
        Ok(n) => n,
        Err(e) => {
            // Roll back: discard staging; restore the moved-aside DB if any.
            // The moved db is self-contained (WAL was checkpointed before the
            // move), so a bare rename back is enough.
            let _ = std::fs::remove_dir_all(&staging);
            if let Some(moved) = &moved_existing_to {
                let _ = std::fs::rename(moved, db_path);
            }
            return Err(e);
        }
    };

    // Move restored files into place. DB first, then swap the attachments dir.
    std::fs::rename(staging.join(ARCHIVE_DB_NAME), db_path)
        .map_err(|e| LificError::Internal(format!("install restored db: {e}")))?;
    set_owner_only(db_path)
        .map_err(|e| LificError::Internal(format!("chmod restored db: {e}")))?;

    let attachments_dest = attachments_dir_for(db_path);
    if attachments_dest.exists() {
        let _ = std::fs::remove_dir_all(&attachments_dest);
    }
    std::fs::rename(staging.join("attachments"), &attachments_dest)
        .map_err(|e| LificError::Internal(format!("install restored attachments: {e}")))?;

    let _ = std::fs::remove_dir_all(&staging);

    Ok(RestoreResult {
        manifest,
        attachment_count,
        db_path: db_path.to_path_buf(),
        moved_existing_to,
    })
}

/// Checkpoint a database file's WAL into the main file, so the `.db` is
/// self-contained (used before moving an existing db aside under `--force`).
/// Best-effort: opening or checkpointing failure is ignored (nothing on disk
/// changes for the worse).
fn checkpoint_db_file(db_path: &Path) {
    if let Ok(conn) = rusqlite::Connection::open(db_path) {
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    }
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
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(tag: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "lific_dump_{tag}_{}_{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Build a real on-disk DB with a seeded project, plus an attachments dir
    /// containing one real blob and one `.tmp` stray. Returns (dir, db_path).
    fn seed_data_dir(tag: &str) -> (PathBuf, PathBuf) {
        let dir = temp_dir(tag);
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
        fs::write(att.join("deadbeef01"), b"blob one").unwrap();
        fs::write(att.join("deadbeef02"), b"second blob bytes").unwrap();
        fs::write(att.join("deadbeef02.tmp"), b"partial write").unwrap();
        (dir, db_path)
    }

    /// List the entry names inside an archive.
    fn archive_entries(archive: &Path) -> Vec<String> {
        let file = fs::File::open(archive).unwrap();
        let dec = flate2::read::GzDecoder::new(file);
        let mut tar = tar::Archive::new(dec);
        tar.entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().replace('\\', "/"))
            .collect()
    }

    #[test]
    fn dump_archive_contains_db_manifest_and_blobs_excluding_tmp() {
        let (dir, db_path) = seed_data_dir("contents");
        let out = dir.join("out.tar.gz");
        let manifest = write_dump(&crate::db::open(&db_path).unwrap(), &db_path, &out).unwrap();

        let entries = archive_entries(&out);
        assert!(entries.contains(&ARCHIVE_DB_NAME.to_string()));
        assert!(entries.contains(&ARCHIVE_MANIFEST_NAME.to_string()));
        assert!(entries.contains(&"attachments/deadbeef01".to_string()));
        assert!(entries.contains(&"attachments/deadbeef02".to_string()));
        assert!(
            !entries.iter().any(|e| e.ends_with(".tmp")),
            "in-progress .tmp writes must be excluded: {entries:?}"
        );

        assert_eq!(manifest.attachment_count, 2);
        assert_eq!(manifest.attachment_bytes, (b"blob one".len() + b"second blob bytes".len()) as u64);
        assert_eq!(manifest.lific_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(manifest.schema_version, crate::db::migrate::latest_version());
        assert!(manifest.db_size_bytes > 0);

        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn dump_archive_is_owner_only_0600() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, db_path) = seed_data_dir("perms");
        let out = dir.join("out.tar.gz");
        write_dump(&crate::db::open(&db_path).unwrap(), &db_path, &out).unwrap();
        let mode = fs::metadata(&out).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "archive must be chmod 0600");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn failed_dump_cleans_up_its_staging_files() {
        // LIF-329: force a failure *after* the staging archive exists by
        // squatting the final path with a directory (rename onto a directory
        // fails on every platform). Neither staging file may survive.
        let (dir, db_path) = seed_data_dir("errclean");
        let out = dir.join("blocked.tar.gz");
        fs::create_dir_all(&out).unwrap();

        let result = write_dump(&crate::db::open(&db_path).unwrap(), &db_path, &out);
        assert!(result.is_err(), "rename onto a directory must fail");

        assert!(
            !out.with_extension("archive.tmp").exists(),
            "partial archive staging file must be cleaned on error"
        );
        assert!(
            !out.with_extension("dbsnapshot.tmp").exists(),
            "db snapshot staging file must be cleaned on error"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dumped_db_is_openable_sqlite_with_seeded_data() {
        let (dir, db_path) = seed_data_dir("snapshot");
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
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_dump_into_directory_uses_default_filename() {
        let (dir, db_path) = seed_data_dir("outdir");
        let target = dir.join("dumps");
        fs::create_dir_all(&target).unwrap();
        let res = run_dump(&db_path, Some(&target)).unwrap();
        assert_eq!(res.archive_path.parent().unwrap(), target);
        let fname = res.archive_path.file_name().unwrap().to_string_lossy();
        assert!(fname.starts_with("lific_"));
        assert!(fname.ends_with(".tar.gz"));
        assert!(res.archive_path.exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn restore_round_trip_matches_entities_and_blob_bytes() {
        let (src_dir, src_db) = seed_data_dir("rt_src");
        let out = src_dir.join("backup.tar.gz");
        write_dump(&crate::db::open(&src_db).unwrap(), &src_db, &out).unwrap();

        // Fresh, empty destination dir.
        let dst_dir = temp_dir("rt_dst");
        let dst_db = dst_dir.join("lific.db");
        let res = run_restore(&out, &dst_db, false).unwrap();
        assert_eq!(res.attachment_count, 2);

        // Entities: the seeded project is present.
        let pool = crate::db::open(&dst_db).unwrap();
        let conn = pool.read().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM projects WHERE identifier = 'DMP'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);

        // Blob bytes identical.
        assert_eq!(
            fs::read(dst_dir.join("attachments").join("deadbeef01")).unwrap(),
            b"blob one"
        );
        assert_eq!(
            fs::read(dst_dir.join("attachments").join("deadbeef02")).unwrap(),
            b"second blob bytes"
        );

        fs::remove_dir_all(&src_dir).ok();
        fs::remove_dir_all(&dst_dir).ok();
    }

    #[test]
    fn restore_refuses_existing_db_without_force() {
        let (src_dir, src_db) = seed_data_dir("guard_src");
        let out = src_dir.join("b.tar.gz");
        write_dump(&crate::db::open(&src_db).unwrap(), &src_db, &out).unwrap();

        // Destination already has a db.
        let dst_dir = temp_dir("guard_dst");
        let dst_db = dst_dir.join("lific.db");
        let _ = crate::db::open(&dst_db).unwrap();

        let err = run_restore(&out, &dst_db, false).unwrap_err();
        assert!(matches!(err, LificError::Conflict(_)), "got {err:?}");

        fs::remove_dir_all(&src_dir).ok();
        fs::remove_dir_all(&dst_dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn restore_force_moves_existing_db_aside() {
        let (src_dir, src_db) = seed_data_dir("force_src");
        let out = src_dir.join("b.tar.gz");
        write_dump(&crate::db::open(&src_db).unwrap(), &src_db, &out).unwrap();

        let dst_dir = temp_dir("force_dst");
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
        let moved = res.moved_existing_to.expect("existing db should be moved aside");
        assert!(moved.exists(), "moved-aside db must still exist");
        assert!(
            moved.file_name().unwrap().to_string_lossy().contains("pre-restore-"),
            "moved db name must include pre-restore-: {}",
            moved.display()
        );
        // The moved-aside db still has the OLD project.
        let conn = rusqlite::Connection::open(&moved).unwrap();
        let old: i64 = conn
            .query_row("SELECT COUNT(*) FROM projects WHERE identifier='OLD'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(old, 1);
        // The live db now has the restored project.
        let pool = crate::db::open(&dst_db).unwrap();
        let conn = pool.read().unwrap();
        let dmp: i64 = conn
            .query_row("SELECT COUNT(*) FROM projects WHERE identifier='DMP'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(dmp, 1);

        fs::remove_dir_all(&src_dir).ok();
        fs::remove_dir_all(&dst_dir).ok();
    }

    #[test]
    fn restore_refuses_newer_schema_version() {
        let (src_dir, src_db) = seed_data_dir("newer_src");
        let out = src_dir.join("b.tar.gz");
        write_dump(&crate::db::open(&src_db).unwrap(), &src_db, &out).unwrap();

        // Rewrite the archive with a bumped schema_version to simulate a dump
        // from a newer Lific.
        let bumped = src_dir.join("bumped.tar.gz");
        rewrite_archive_with_schema(&out, &bumped, crate::db::migrate::latest_version() + 5);

        let dst_dir = temp_dir("newer_dst");
        let dst_db = dst_dir.join("lific.db");
        let err = run_restore(&bumped, &dst_db, false).unwrap_err();
        assert!(matches!(err, LificError::BadRequest(_)), "got {err:?}");
        assert!(!dst_db.exists(), "nothing should be restored on schema refusal");

        fs::remove_dir_all(&src_dir).ok();
        fs::remove_dir_all(&dst_dir).ok();
    }

    #[test]
    fn restore_rejects_path_traversal_entry() {
        let dir = temp_dir("traversal");
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
        let err = inspect_archive(&archive).unwrap_err();
        assert!(matches!(err, LificError::BadRequest(_)), "got {err:?}");

        // And a full restore attempt must also refuse, leaving no db behind.
        let dst_dir = temp_dir("traversal_dst");
        let dst_db = dst_dir.join("lific.db");
        assert!(run_restore(&archive, &dst_db, false).is_err());
        assert!(!dst_db.exists());
        assert!(
            !dst_dir.join("attachments").join("sub").exists(),
            "traversal entry must not write nested dirs"
        );

        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&dst_dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn restore_mid_extract_failure_leaves_original_untouched() {
        // A corrupt archive (valid manifest+db header claim, truncated body)
        // must fail extraction WITHOUT clobbering the pre-existing db when
        // --force moved it aside — the rollback restores it.
        let (src_dir, src_db) = seed_data_dir("midfail_src");
        let good = src_dir.join("good.tar.gz");
        write_dump(&crate::db::open(&src_db).unwrap(), &src_db, &good).unwrap();

        // Truncate the good archive to corrupt it mid-stream.
        let corrupt = src_dir.join("corrupt.tar.gz");
        let bytes = fs::read(&good).unwrap();
        fs::write(&corrupt, &bytes[..bytes.len() / 2]).unwrap();

        let dst_dir = temp_dir("midfail_dst");
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
            .query_row("SELECT COUNT(*) FROM projects WHERE identifier='ORG'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(org, 1, "original data must survive a failed restore");

        fs::remove_dir_all(&src_dir).ok();
        fs::remove_dir_all(&dst_dir).ok();
    }

    #[test]
    fn validate_attachment_entry_accepts_bare_hash_rejects_traversal() {
        assert!(validate_attachment_entry("attachments/abc123def").is_ok());
        assert!(validate_attachment_entry("attachments/../etc/passwd").is_err());
        assert!(validate_attachment_entry("attachments/sub/dir").is_err());
        assert!(validate_attachment_entry("attachments/").is_err());
        assert!(validate_attachment_entry("notattachments/x").is_err());
        assert!(validate_attachment_entry("attachments/.hidden").is_err());
    }

    // Test helper: re-pack an archive but overwrite the manifest's
    // schema_version, to simulate an archive from a newer binary.
    fn rewrite_archive_with_schema(src: &Path, dst: &Path, schema_version: i64) {
        let mut manifest = read_manifest(src).unwrap();
        manifest.schema_version = schema_version;
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
