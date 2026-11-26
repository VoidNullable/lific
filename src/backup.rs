//! Automatic backups, for every process that opens a local database.
//!
//! LIF-509: the schedule used to live in `server::run`, so an instance used
//! only through `lific mcp` (stdio) or local CLI commands was never backed up.
//! Any of those processes can now take the backup, and several of them often
//! run at once against one database: the HTTP server, one stdio MCP server per
//! agent session, and CLI calls in between. A per-process timer would multiply
//! the archives and rotate a day of history away in an hour, so the schedule
//! lives in the backup directory instead. A backup is due when the newest
//! archive there is at least `interval_minutes` old, whoever wrote it, and an
//! advisory lock beside the archives makes sure only one process writes the
//! next one.
//!
//! - [`start_scheduler`]: long-running processes (`lific start`, `lific mcp`).
//! - [`run_if_due`]: one-shot commands, in-line, after their own work.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use fs2::FileExt;
use tracing::{error, info, warn};

use crate::config::{BackupConfig, Config};
use crate::db::DbPool;
use crate::dump;

/// How long a long-running process waits before its first check, so the
/// backup does not compete with startup.
const STARTUP_DELAY: Duration = Duration::from_secs(5);

/// How long to wait when another process holds the schedule lock. It is
/// writing the backup this process would have written, so the next check
/// after that should find a fresh archive and wait a full interval.
const BUSY_RETRY: Duration = Duration::from_secs(60);

/// Floor on any scheduler sleep, so no combination of clock and filesystem
/// state can turn the loop into a busy one.
const MIN_WAIT: Duration = Duration::from_secs(1);

/// Where and how often one database is backed up, resolved from config once.
#[derive(Debug, Clone)]
struct Plan {
    db_path: PathBuf,
    dir: PathBuf,
    interval: Duration,
    retain: usize,
    audit_retention_days: Option<u32>,
}

impl Plan {
    fn from_config(cfg: &Config) -> Self {
        Self {
            db_path: cfg.database.path.clone(),
            dir: cfg.backup_dir(),
            interval: Duration::from_secs(
                checked_interval_minutes(cfg.backup.interval_minutes) * 60,
            ),
            retain: checked_retain(cfg.backup.retain),
            audit_retention_days: cfg.backup.audit_retention_days,
        }
    }

    fn db_stem(&self) -> &str {
        db_stem(&self.db_path)
    }

    /// The schedule lock, per database so two databases sharing one absolute
    /// backup directory keep separate schedules. The leading dot keeps it out
    /// of rotation and the staging sweep.
    fn lock_path(&self) -> PathBuf {
        self.dir.join(format!(".{}.backup.lock", self.db_stem()))
    }
}

fn db_stem(db_path: &Path) -> &str {
    db_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("lific")
}

/// What one scheduling attempt did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Attempt {
    /// The newest archive is younger than the interval.
    NotDue,
    /// Another process holds the schedule lock.
    Busy,
    Written,
    Failed,
}

/// Run automatic backups for as long as this process lives, when `[backup]`
/// is enabled. For processes that keep running: the HTTP server and the stdio
/// MCP server.
pub fn start_scheduler(cfg: &Config, pool: DbPool) -> Option<tokio::task::JoinHandle<()>> {
    if !cfg.backup.enabled {
        return None;
    }
    let plan = Plan::from_config(cfg);
    info!(
        dir = %plan.dir.display(),
        interval_min = plan.interval.as_secs() / 60,
        retain = plan.retain,
        audit_retention_days = plan.audit_retention_days.unwrap_or(0),
        "automatic backups enabled"
    );
    Some(tokio::spawn(schedule(Arc::new(pool), plan)))
}

/// Take a backup now if one is due, when `[backup]` is enabled. For one-shot
/// commands that open the local database: they have no lifetime to schedule
/// across, so they catch up on the way out. Cheap when nothing is due, which
/// is always the case while a server or stdio session keeps backups current.
pub fn run_if_due(cfg: &Config, pool: &DbPool) {
    if cfg.backup.enabled {
        backup_if_due(pool, &Plan::from_config(cfg));
    }
}

async fn schedule(pool: Arc<DbPool>, plan: Plan) {
    tokio::time::sleep(STARTUP_DELAY).await;
    loop {
        // Awaited, so this task never has more than one backup in flight; a
        // backup that outruns its interval delays the next check instead of
        // stacking blocking jobs on the pool.
        let wait = match attempt_blocking(&pool, &plan).await {
            Attempt::Written => plan.interval,
            Attempt::NotDue => time_until_due(&plan),
            Attempt::Busy => BUSY_RETRY.min(plan.interval),
            // Retry on the next interval, not immediately: whatever failed
            // (a full disk, an unwritable directory) is unlikely to have
            // cleared a second later, and every attempt logs an error.
            Attempt::Failed => plan.interval,
        };
        tokio::time::sleep(wait.max(MIN_WAIT)).await;
    }
}

/// Run one scheduling attempt on the blocking pool and wait for it.
///
/// `run_backup` is synchronous, file-heavy work that takes the attachment
/// store lock, and under that lock it can wait on whatever else holds the
/// store. Calling it directly from the async task would park a Tokio worker
/// thread for that whole time, and workers are the threads serving every HTTP
/// request. `spawn_blocking` puts it on the pool meant for exactly this.
async fn attempt_blocking(pool: &Arc<DbPool>, plan: &Plan) -> Attempt {
    let pool = Arc::clone(pool);
    let plan = plan.clone();
    match tokio::task::spawn_blocking(move || backup_if_due(&pool, &plan)).await {
        Ok(attempt) => attempt,
        Err(e) => {
            // A panic inside the backup must not kill the scheduling loop: the
            // next interval should still try.
            error!(error = %e, "backup job failed to run to completion");
            Attempt::Failed
        }
    }
}

/// Write a backup if the newest one is at least an interval old.
///
/// The due check runs twice: once without the lock, which is the common and
/// cheap answer, and again under it, because another process may have
/// finished an archive between the first check and taking the lock.
fn backup_if_due(pool: &DbPool, plan: &Plan) -> Attempt {
    if !time_until_due(plan).is_zero() {
        return Attempt::NotDue;
    }
    if let Err(e) = std::fs::create_dir_all(&plan.dir) {
        error!(dir = %plan.dir.display(), error = %e, "failed to create backup directory");
        return Attempt::Failed;
    }
    let _lock = match try_schedule_lock(&plan.lock_path()) {
        Ok(Some(lock)) => lock,
        Ok(None) => return Attempt::Busy,
        Err(e) => {
            error!(dir = %plan.dir.display(), error = %e, "failed to open backup schedule lock");
            return Attempt::Failed;
        }
    };
    if !time_until_due(plan).is_zero() {
        return Attempt::NotDue;
    }
    if run_backup(
        pool,
        &plan.db_path,
        &plan.dir,
        plan.retain,
        plan.audit_retention_days,
    ) {
        Attempt::Written
    } else {
        Attempt::Failed
    }
}

/// Take the schedule lock without waiting. `Ok(None)` means another process
/// holds it. Dropping the returned handle releases it, and so does a crash.
fn try_schedule_lock(path: &Path) -> std::io::Result<Option<File>> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(file)),
        Err(_) => Ok(None),
    }
}

/// How long until the next backup is due. Zero when there is no backup yet or
/// the newest is at least an interval old.
fn time_until_due(plan: &Plan) -> Duration {
    newest_backup_age(&plan.dir, plan.db_stem())
        .map_or(Duration::ZERO, |age| plan.interval.saturating_sub(age))
}

/// Age of the newest backup artifact for `db_stem` in `dir`, by mtime.
///
/// An artifact dated in the future is skipped rather than read as brand new:
/// one such file (a clock that jumped back, a copy from another machine)
/// would otherwise keep every backup "not due" until real time caught up.
fn newest_backup_age(dir: &Path, db_stem: &str) -> Option<Duration> {
    let prefix = format!("{db_stem}_");
    let now = SystemTime::now();
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| is_backup_artifact(name, &prefix))
        })
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            if !metadata.is_file() {
                return None;
            }
            now.duration_since(metadata.modified().ok()?).ok()
        })
        .min()
}

/// A backup archive (`.tar.gz`) or a legacy bare-`.db` snapshot from the
/// pre-archive scheme. Both share the `{stem}_` prefix and a sortable
/// timestamp.
fn is_backup_artifact(name: &str, prefix: &str) -> bool {
    name.starts_with(prefix) && (name.ends_with(".tar.gz") || name.ends_with(".db"))
}

/// The configured backup interval, or the default when the config says `0`
/// (LIF-415).
///
/// Zero is never a schedule anyone wants: every backup would be due the moment
/// the previous one landed, so the instance would do nothing but write
/// archives (it once panicked the old interval timer instead). Falling back
/// to the default with a warning keeps the instance backed up and tells the
/// operator their value was ignored, which matches how the rest of the config
/// treats a value it cannot honor.
fn checked_interval_minutes(configured: u64) -> u64 {
    if configured == 0 {
        let fallback = BackupConfig::default().interval_minutes;
        warn!(
            fallback_minutes = fallback,
            "[backup] interval_minutes = 0 is not a valid schedule; using the default instead"
        );
        return fallback;
    }
    configured
}

/// The configured retention count, or the default when the config says `0`
/// (LIF-415).
///
/// `retain = 0` reads like "keep no old backups" but means "keep nothing at
/// all": rotation runs right after each archive is written, so every
/// successful backup is deleted seconds after it lands and the instance ends
/// up with no backups while reporting healthy ones. An operator who genuinely
/// wants no backups sets `enabled = false`.
fn checked_retain(configured: usize) -> usize {
    if configured == 0 {
        let fallback = BackupConfig::default().retain;
        warn!(
            fallback_retain = fallback,
            "[backup] retain = 0 would delete every backup as soon as it is written; using \
             the default instead (set [backup] enabled = false to turn backups off)"
        );
        return fallback;
    }
    configured
}

/// Whether we've already logged the one-time hint about the legacy mirrored
/// `attachments/` dir left behind by the pre-LIF-266 backup scheme.
static LEGACY_MIRROR_HINTED: AtomicBool = AtomicBool::new(false);

/// Perform a single backup: write one self-contained `.tar.gz` archive via the
/// shared dump code path (same artifact `lific dump` produces), then rotate.
///
/// LIF-266: this replaces the old bare-`.db` snapshot plus additive
/// attachments-mirror scheme. The mirror grew forever (blobs were never GC'd);
/// self-contained archives sidestep that (at the cost of duplicating blobs per
/// archive — acceptable at current scale).
///
/// Returns whether an archive was written.
fn run_backup(
    pool: &DbPool,
    db_path: &Path,
    backup_dir: &Path,
    retain: usize,
    audit_retention_days: Option<u32>,
) -> bool {
    let db_stem = db_stem(db_path);

    // Sweep staging leftovers from a previous crashed/failed run first, so
    // they get cleaned even if this run's backup fails too (LIF-329).
    sweep_stale_tmps(backup_dir, db_stem);

    let filename = dump::archive_filename(db_stem, &dump::archive_timestamp());
    let backup_path = backup_dir.join(&filename);

    match dump::write_dump(pool, db_path, &backup_path) {
        Ok(manifest) => {
            let size = std::fs::metadata(&backup_path).map_or(0, |m| m.len());
            info!(
                path = %backup_path.display(),
                size_kb = size / 1024,
                attachments = manifest.attachment_count,
                "backup archive written"
            );
        }
        Err(e) => {
            error!(error = %e, "backup archive failed");
            let _ = std::fs::remove_file(&backup_path);
            return false;
        }
    }

    // One-time hint about the legacy mirrored attachments dir (old scheme). It
    // is no longer written to or read from; the operator can delete it.
    let legacy_mirror = backup_dir.join("attachments");
    if legacy_mirror.is_dir() && !LEGACY_MIRROR_HINTED.swap(true, Ordering::Relaxed) {
        info!(
            dir = %legacy_mirror.display(),
            "legacy mirrored attachments dir from the pre-archive backup scheme is no longer \
             used and can be deleted"
        );
    }

    rotate_backups(backup_dir, db_stem, retain);

    // Audit retention runs last, and only on a run that produced an archive:
    // the dump failure path above returns early, so history is never dropped
    // by a cycle that failed to preserve it first.
    prune_audit_log(pool, audit_retention_days);

    trim_retained_heap();
    true
}

/// Hand freed heap back to the OS after each backup cycle (glibc only).
///
/// glibc's malloc never returns freed sub-128KB chunks to the kernel on its
/// own: each request burst (a project export, a sync-index bootstrap, an
/// attachment round-trip) ratchets some arena's high-water mark up, and the
/// freed memory then sits in that arena's free lists forever. Profiling the
/// production instance after six days of agent traffic found 448MB of
/// freed-but-retained heap across 22 arenas against 27MB of live objects.
///
/// `malloc_trim(0)` walks every arena and releases its free chunks back to
/// the kernel (`MADV_DONTNEED`), which is exactly the memory described above.
/// The backup task is a convenient heartbeat for it: every cycle, off the
/// request path, on a blocking thread. The call is process-wide, costs
/// milliseconds at this heap size, and is a no-op for whatever is actually
/// live.
///
/// Gated to `linux + gnu`: musl, macOS, and Windows allocators don't have the
/// arena-retention behavior (or the function).
fn trim_retained_heap() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: malloc_trim has no preconditions; it takes the arena locks
    // itself and is safe to call from any thread at any time.
    unsafe {
        libc::malloc_trim(0);
    }
}

/// Delete `audit_log` rows older than the configured retention window
/// (LIF-158).
///
/// `None` and `Some(0)` both mean "keep forever" and do nothing, so the
/// default install keeps the append-only history it has always kept. Anything
/// larger deletes rows whose `ts` predates the cutoff.
///
/// The cutoff is computed by SQLite itself (`datetime('now', '-N days')`)
/// rather than in Rust: `audit_log.ts` is written by the migration-018
/// triggers as `datetime('now')`, so comparing against a value from the same
/// clock and the same format is the only way the comparison is honest. The
/// negative-modifier-as-bind-parameter shape matches
/// `queries::attachments::find_orphans`.
fn prune_audit_log(pool: &DbPool, audit_retention_days: Option<u32>) {
    let Some(days) = audit_retention_days.filter(|d| *d > 0) else {
        return;
    };

    let conn = match pool.write() {
        Ok(conn) => conn,
        Err(e) => {
            warn!(error = %e, "could not acquire write connection for audit log pruning");
            return;
        }
    };

    let cutoff = format!("-{days} days");
    match conn.execute(
        "DELETE FROM audit_log WHERE ts < datetime('now', ?1)",
        rusqlite::params![cutoff],
    ) {
        Ok(0) => {}
        Ok(deleted) => info!(
            deleted,
            retention_days = days,
            "pruned old audit log entries"
        ),
        Err(e) => warn!(error = %e, "audit log pruning failed"),
    }
}

/// How old a dump staging file must be before the sweep considers it stale.
/// A live dump finishes in seconds; anything an hour old is a crash leftover.
const STALE_TMP_AGE: Duration = Duration::from_secs(60 * 60);

/// Delete stale dump staging files leaked by a crash mid-backup (LIF-329).
///
/// `write_dump` stages a private `.lific-dump-*` directory beside the output
/// archive and cleans it itself on success or error. Age-gated so an
/// in-flight dump's staging directory is never swept.
fn sweep_stale_tmps(backup_dir: &Path, db_stem: &str) {
    let prefix = format!("{db_stem}_");
    let entries = match std::fs::read_dir(backup_dir) {
        Ok(entries) => entries,
        Err(e) => {
            warn!(error = %e, "failed to read backup directory for tmp sweep");
            return;
        }
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let is_new_staging_dir = name.starts_with(".lific-dump-");
        let is_legacy_tmp = name.starts_with(&prefix)
            && (name.ends_with(".dbsnapshot.tmp") || name.ends_with(".archive.tmp"));
        if !is_new_staging_dir && !is_legacy_tmp {
            continue;
        }
        let age_path = if is_new_staging_dir {
            path.join("activity")
        } else {
            path.clone()
        };
        let stale = std::fs::symlink_metadata(&age_path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > STALE_TMP_AGE);
        if !stale {
            continue;
        }
        if is_new_staging_dir {
            if dump::staging_is_locked(&path) {
                continue;
            }
            let activity = path.join("activity");
            let active = std::fs::symlink_metadata(&activity)
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age <= STALE_TMP_AGE);
            if active {
                continue;
            }
        }
        let result = if is_new_staging_dir {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        match result {
            Ok(()) => info!(path = %path.display(), "removed stale backup staging file"),
            Err(e) => {
                warn!(path = %path.display(), error = %e, "failed to remove stale staging file")
            }
        }
    }
}

/// Keep only the N most recent backup archives, delete the rest.
///
/// LIF-266: rotation candidates are the new `.tar.gz` archives AND legacy
/// bare-`.db` snapshots from the old scheme (both share the `{stem}_` prefix
/// and a sortable timestamp), so old snapshots age out naturally alongside new
/// archives. The legacy mirrored `attachments/` dir is left alone (it isn't a
/// per-run artifact); a one-time hint in `run_backup` notes it can be deleted.
fn rotate_backups(backup_dir: &Path, db_stem: &str, retain: usize) {
    let prefix = format!("{db_stem}_");
    let mut backups: Vec<PathBuf> = match std::fs::read_dir(backup_dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|name| is_backup_artifact(name, &prefix))
            })
            .collect(),
        Err(e) => {
            warn!(error = %e, "failed to read backup directory for rotation");
            return;
        }
    };

    // Sort by filename (timestamps sort lexicographically)
    backups.sort();

    // Remove oldest backups beyond retention
    if backups.len() > retain {
        let to_remove = backups.len() - retain;
        for path in backups.iter().take(to_remove) {
            match std::fs::remove_file(path) {
                Ok(()) => info!(path = %path.display(), "removed old backup"),
                Err(e) => warn!(path = %path.display(), error = %e, "failed to remove old backup"),
            }
        }
    }
}

/// Checkpoint the WAL into the main database file.
/// Call this on clean shutdown so the .db file is fully self-contained.
pub fn checkpoint_wal(pool: &DbPool) {
    match pool.write() {
        Ok(conn) => match conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);") {
            Ok(()) => info!("WAL checkpointed on shutdown"),
            Err(e) => warn!(error = %e, "WAL checkpoint failed"),
        },
        Err(e) => warn!(error = %e, "could not acquire write connection for checkpoint"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Scratch directory for one test. The returned guard removes the
    /// directory on Drop, including while a failing assertion unwinds, so a
    /// panicking test cannot leave stale state behind for a later run.
    fn make_temp_dir() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    fn make_backup_dir(parent: &Path) -> PathBuf {
        let backup_dir = parent.join("backups");
        fs::create_dir_all(&backup_dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&backup_dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        backup_dir
    }

    #[test]
    fn rotate_keeps_only_retain_count() {
        let tmp = make_temp_dir();
        let dir = tmp.path();

        // Create 5 fake archive files with lexicographic timestamps
        for i in 1..=5 {
            fs::write(dir.join(format!("lific_2026010{i}_120000.tar.gz")), "fake").unwrap();
        }

        rotate_backups(dir, "lific", 3);

        let remaining: Vec<_> = fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).collect();
        assert_eq!(remaining.len(), 3);

        // Oldest two (01, 02) should be gone, newest three (03, 04, 05) kept
        assert!(!dir.join("lific_20260101_120000.tar.gz").exists());
        assert!(!dir.join("lific_20260102_120000.tar.gz").exists());
        assert!(dir.join("lific_20260103_120000.tar.gz").exists());
        assert!(dir.join("lific_20260105_120000.tar.gz").exists());
    }

    #[test]
    fn rotate_does_nothing_under_retain() {
        let tmp = make_temp_dir();
        let dir = tmp.path();

        fs::write(dir.join("lific_20260101_120000.tar.gz"), "fake").unwrap();
        fs::write(dir.join("lific_20260102_120000.tar.gz"), "fake").unwrap();

        rotate_backups(dir, "lific", 5);

        let count = fs::read_dir(dir).unwrap().count();
        assert_eq!(count, 2);
    }

    #[test]
    fn rotate_ignores_other_files() {
        let tmp = make_temp_dir();
        let dir = tmp.path();

        // These should be ignored (wrong prefix / extension)
        fs::write(dir.join("other_20260101_120000.tar.gz"), "x").unwrap();
        fs::write(dir.join("lific_20260101_120000.txt"), "x").unwrap();
        // These are real archives
        fs::write(dir.join("lific_20260101_120000.tar.gz"), "x").unwrap();
        fs::write(dir.join("lific_20260102_120000.tar.gz"), "x").unwrap();

        rotate_backups(dir, "lific", 1);

        // Only 1 backup kept, non-matching files untouched
        assert!(dir.join("other_20260101_120000.tar.gz").exists());
        assert!(dir.join("lific_20260101_120000.txt").exists());
        assert!(!dir.join("lific_20260101_120000.tar.gz").exists()); // oldest removed
        assert!(dir.join("lific_20260102_120000.tar.gz").exists()); // kept
    }

    #[test]
    fn rotate_ages_out_legacy_db_snapshots_alongside_archives() {
        // LIF-266: pre-archive `.db` snapshots from the old scheme are
        // rotation candidates too, so they age out naturally instead of
        // accumulating forever next to the new `.tar.gz` archives.
        let tmp = make_temp_dir();
        let dir = tmp.path();
        // Two legacy .db snapshots (older timestamps) + two new archives.
        fs::write(dir.join("lific_20260101_120000.db"), "old1").unwrap();
        fs::write(dir.join("lific_20260102_120000.db"), "old2").unwrap();
        fs::write(dir.join("lific_20260103_120000.tar.gz"), "new1").unwrap();
        fs::write(dir.join("lific_20260104_120000.tar.gz"), "new2").unwrap();

        rotate_backups(dir, "lific", 2);

        // The two oldest (legacy .db) are gone; the two newest archives kept.
        assert!(!dir.join("lific_20260101_120000.db").exists());
        assert!(!dir.join("lific_20260102_120000.db").exists());
        assert!(dir.join("lific_20260103_120000.tar.gz").exists());
        assert!(dir.join("lific_20260104_120000.tar.gz").exists());
    }

    /// Backdate a file's mtime so the sweep sees it as stale.
    #[tokio::test]
    async fn a_scheduled_backup_does_not_park_the_async_runtime() {
        // A `#[tokio::test]` runs on a single-threaded runtime, so this is a
        // real discriminator: if the backup ran inline on the async task, the
        // counter task below could not tick at all while it worked. It has to
        // be on the blocking pool for both to make progress.
        use std::sync::Arc;
        use std::sync::atomic::AtomicUsize;

        let tmp = make_temp_dir();
        let dir = tmp.path();
        let db_path = dir.join("lific.db");
        let backup_dir = make_backup_dir(dir);
        let pool = Arc::new(crate::db::open(&db_path).expect("open test db"));

        // Hold the attachment store lock briefly, so the backup has something
        // real to wait on inside its blocking job.
        let store = crate::storage::AttachmentStore::from_db_path(&db_path);
        let (holding_tx, holding_rx) = std::sync::mpsc::sync_channel::<()>(1);
        let holder = std::thread::spawn(move || {
            store
                .with_lock(|_| {
                    holding_tx.send(()).unwrap();
                    std::thread::sleep(Duration::from_millis(200));
                    Ok(())
                })
                .unwrap();
        });
        holding_rx.recv_timeout(Duration::from_secs(10)).unwrap();

        let ticks = Arc::new(AtomicUsize::new(0));
        let counter = tokio::spawn({
            let ticks = Arc::clone(&ticks);
            async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    ticks.fetch_add(1, Ordering::Relaxed);
                }
            }
        });

        let attempt = attempt_blocking(&pool, &test_plan(&db_path, &backup_dir)).await;
        counter.abort();
        holder.join().unwrap();

        assert_eq!(attempt, Attempt::Written);
        assert!(
            ticks.load(Ordering::Relaxed) > 0,
            "async work must keep running while a backup is in flight"
        );
        let archives = fs::read_dir(&backup_dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tar.gz"))
            .count();
        assert_eq!(archives, 1, "and the backup still produced its archive");
    }

    fn backdate(path: &Path, by: Duration) {
        let f = if cfg!(unix) && path.is_dir() {
            fs::File::open(path).unwrap()
        } else {
            fs::File::options().write(true).open(path).unwrap()
        };
        f.set_modified(std::time::SystemTime::now() - by).unwrap();
    }

    fn test_plan(db_path: &Path, backup_dir: &Path) -> Plan {
        Plan {
            db_path: db_path.to_path_buf(),
            dir: backup_dir.to_path_buf(),
            interval: Duration::from_secs(30 * 60),
            retain: 5,
            audit_retention_days: None,
        }
    }

    fn archive_count(dir: &Path) -> usize {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tar.gz"))
            .count()
    }

    // ── LIF-509: one schedule shared by every process ─────────────────────

    #[test]
    fn a_backup_is_due_when_none_exists_and_not_again_until_an_interval_passes() {
        let tmp = make_temp_dir();
        let db_path = tmp.path().join("lific.db");
        let backup_dir = tmp.path().join("backups");
        let pool = crate::db::open(&db_path).unwrap();
        let plan = test_plan(&db_path, &backup_dir);

        assert_eq!(backup_if_due(&pool, &plan), Attempt::Written);
        assert_eq!(
            archive_count(&backup_dir),
            1,
            "and it created the directory"
        );

        // A second process arriving moments later finds the fresh archive.
        assert_eq!(backup_if_due(&pool, &plan), Attempt::NotDue);
        assert_eq!(archive_count(&backup_dir), 1);
    }

    #[test]
    fn an_archive_older_than_the_interval_makes_the_next_backup_due() {
        let tmp = make_temp_dir();
        let db_path = tmp.path().join("lific.db");
        let backup_dir = make_backup_dir(tmp.path());
        let pool = crate::db::open(&db_path).unwrap();
        let plan = test_plan(&db_path, &backup_dir);

        // Written by any process, even a different binary: the directory is
        // the schedule.
        let old = backup_dir.join("lific_20260101_120000.tar.gz");
        fs::write(&old, "old").unwrap();
        backdate(&old, plan.interval.saturating_sub(Duration::from_secs(60)));
        assert_eq!(backup_if_due(&pool, &plan), Attempt::NotDue);

        backdate(&old, plan.interval + Duration::from_secs(60));
        assert_eq!(backup_if_due(&pool, &plan), Attempt::Written);
        assert_eq!(archive_count(&backup_dir), 2);
    }

    #[test]
    fn a_held_schedule_lock_means_another_process_is_writing_the_backup() {
        let tmp = make_temp_dir();
        let db_path = tmp.path().join("lific.db");
        let backup_dir = make_backup_dir(tmp.path());
        let pool = crate::db::open(&db_path).unwrap();
        let plan = test_plan(&db_path, &backup_dir);

        // A separate open handle stands in for the other process: the lock
        // is per open file, so this conflicts exactly as a second process's
        // would.
        let held = try_schedule_lock(&plan.lock_path()).unwrap().unwrap();
        assert_eq!(backup_if_due(&pool, &plan), Attempt::Busy);
        assert_eq!(archive_count(&backup_dir), 0);

        drop(held);
        assert_eq!(backup_if_due(&pool, &plan), Attempt::Written);
    }

    #[test]
    fn the_schedule_reads_only_this_databases_archives() {
        let tmp = make_temp_dir();
        let dir = tmp.path();
        let plan = test_plan(&dir.join("lific.db"), dir);

        // Another database's archive, an unrelated file, and one dated in the
        // future: none of them is evidence of a recent backup of this one.
        fs::write(dir.join("other_20260101_120000.tar.gz"), "x").unwrap();
        fs::write(dir.join("lific_notes.txt"), "x").unwrap();
        let future = dir.join("lific_20991231_235959.tar.gz");
        fs::write(&future, "x").unwrap();
        fs::File::options()
            .write(true)
            .open(&future)
            .unwrap()
            .set_modified(SystemTime::now() + Duration::from_secs(24 * 60 * 60))
            .unwrap();
        assert_eq!(newest_backup_age(dir, "lific"), None);
        assert_eq!(time_until_due(&plan), Duration::ZERO);

        // A legacy bare-`.db` snapshot still counts.
        let legacy = dir.join("lific_20260101_120000.db");
        fs::write(&legacy, "x").unwrap();
        backdate(&legacy, Duration::from_secs(10 * 60));
        let wait = time_until_due(&plan);
        assert!(
            wait > Duration::from_secs(19 * 60) && wait <= Duration::from_secs(20 * 60),
            "expected about 20 minutes, got {wait:?}"
        );
    }

    #[test]
    fn run_if_due_does_nothing_when_backups_are_disabled() {
        let tmp = make_temp_dir();
        let mut cfg = Config::default();
        cfg.database.path = tmp.path().join("lific.db");
        cfg.backup.enabled = false;
        let pool = crate::db::open(&cfg.database.path).unwrap();

        run_if_due(&cfg, &pool);
        assert!(!cfg.backup_dir().exists());

        cfg.backup.enabled = true;
        run_if_due(&cfg, &pool);
        assert_eq!(archive_count(&cfg.backup_dir()), 1);
    }

    #[test]
    fn sweep_removes_stale_staging_tmps_keeps_fresh_and_unrelated() {
        // LIF-329: crash leftovers (`*.dbsnapshot.tmp` / `*.archive.tmp`)
        // older than the stale threshold are swept; fresh staging files (a
        // possibly in-flight dump) and unrelated files are untouched.
        let tmp = make_temp_dir();
        let dir = tmp.path();
        let old = STALE_TMP_AGE + Duration::from_secs(60);

        let stale_snap = dir.join("lific_20260101_120000.tar.dbsnapshot.tmp");
        let stale_arch = dir.join("lific_20260101_120000.tar.archive.tmp");
        let fresh_arch = dir.join("lific_20260714_120000.tar.archive.tmp");
        let other_stem = dir.join("other_20260101_120000.tar.archive.tmp");
        let real_backup = dir.join("lific_20260101_120000.tar.gz");
        let stale_dir = dir.join(".lific-dump-stale");
        for p in [
            &stale_snap,
            &stale_arch,
            &fresh_arch,
            &other_stem,
            &real_backup,
        ] {
            fs::write(p, "x").unwrap();
        }
        fs::create_dir(&stale_dir).unwrap();
        fs::write(stale_dir.join("activity"), "stale").unwrap();
        backdate(&stale_snap, old);
        backdate(&stale_arch, old);
        backdate(&other_stem, old);
        backdate(&real_backup, old);
        backdate(&stale_dir.join("activity"), old);

        sweep_stale_tmps(dir, "lific");

        assert!(!stale_snap.exists(), "stale snapshot tmp must be swept");
        assert!(!stale_arch.exists(), "stale archive tmp must be swept");
        assert!(fresh_arch.exists(), "fresh staging tmp must survive");
        assert!(other_stem.exists(), "other stems are not ours to sweep");
        assert!(
            real_backup.exists(),
            "real archives are rotation's job, not the sweep's"
        );
        assert!(
            !stale_dir.exists(),
            "stale dump staging directory must be swept"
        );
    }

    #[test]
    fn sweep_keeps_stale_directory_while_dump_lock_is_held() {
        use fs2::FileExt;

        let tmp = make_temp_dir();
        let dir = tmp.path();
        let staging = dir.join(".lific-dump-active");
        fs::create_dir(&staging).unwrap();
        fs::write(staging.join("activity"), "active").unwrap();
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(staging.join("lock"))
            .unwrap();
        lock.try_lock_exclusive().unwrap();
        backdate(
            &staging.join("activity"),
            STALE_TMP_AGE + Duration::from_secs(60),
        );

        sweep_stale_tmps(dir, "lific");

        assert!(staging.exists(), "a locked dump must not be swept");
        drop(lock);
    }

    #[test]
    fn run_backup_sweeps_stale_tmps_even_when_it_writes_nothing_new() {
        // The sweep runs at the top of run_backup, so leftovers age out on
        // the next interval even if that run's dump were to fail.
        let tmp = make_temp_dir();
        let dir = tmp.path();
        let db_path = dir.join("lific.db");
        let backup_dir = make_backup_dir(dir);
        let pool = crate::db::open(&db_path).expect("open test db");

        let stale = backup_dir.join("lific_20260101_120000.tar.archive.tmp");
        fs::write(&stale, "partial").unwrap();
        backdate(&stale, STALE_TMP_AGE + Duration::from_secs(60));

        run_backup(&pool, &db_path, &backup_dir, 5, None);

        assert!(!stale.exists(), "run_backup must sweep stale staging files");
    }

    #[test]
    fn run_backup_emits_tar_gz_archive_with_data_and_blobs() {
        // LIF-266: the interval task now emits a single self-contained
        // `.tar.gz` archive (same artifact as `lific dump`) carrying the DB
        // snapshot and every non-.tmp attachment blob.
        let tmp = make_temp_dir();
        let dir = tmp.path();
        let db_path = dir.join("lific.db");
        let backup_dir = make_backup_dir(dir);

        // Seed the DB and an attachments sidecar dir next to it.
        let pool = crate::db::open(&db_path).expect("open test db");
        {
            let conn = pool.write().unwrap();
            crate::db::queries::create_project(
                &conn,
                &crate::db::models::CreateProject {
                    conventions: None,
                    name: "BackupTest".into(),
                    identifier: "BKP".into(),
                    description: String::new(),
                    emoji: None,
                    lead_user_id: None,
                },
            )
            .unwrap();
        }
        let att_dir = dir.join("attachments");
        fs::create_dir_all(&att_dir).unwrap();
        // The name must be the content's real hash: a dump verifies every blob
        // against its content address while archiving it.
        let blob_name = crate::storage::AttachmentStore::hash_bytes(b"blob contents");
        fs::write(att_dir.join(&blob_name), b"blob contents").unwrap();
        fs::write(att_dir.join("deadbeefsha.tmp"), b"partial").unwrap();

        run_backup(&pool, &db_path, &backup_dir, 5, None);

        // Exactly one `.tar.gz` archive, no bare `.db` snapshot.
        let archives: Vec<_> = fs::read_dir(&backup_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".tar.gz"))
            .collect();
        assert_eq!(archives.len(), 1, "expected one archive, got {archives:?}");

        // Its contents: db + manifest + the blob, excluding the .tmp write.
        let archive_path = backup_dir.join(&archives[0]);
        let file = fs::File::open(&archive_path).unwrap();
        let dec = flate2::read::GzDecoder::new(file);
        let mut tar = tar::Archive::new(dec);
        let names: Vec<String> = tar
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.iter().any(|n| n == crate::dump::ARCHIVE_DB_NAME));
        assert!(
            names
                .iter()
                .any(|n| n == crate::dump::ARCHIVE_MANIFEST_NAME)
        );
        assert!(
            names
                .iter()
                .any(|n| n == &format!("attachments/{blob_name}"))
        );
        assert!(
            !names.iter().any(|n| n.ends_with(".tmp")),
            "in-progress .tmp writes must not be archived: {names:?}"
        );
    }

    // ── LIF-415: backup config validation ─────────────────────────────────

    /// `interval_minutes = 0` builds a zero-period `tokio::time::interval`,
    /// which panics inside the spawned task and takes backups down without
    /// stopping the server. The value is replaced instead.
    #[test]
    fn zero_interval_falls_back_to_the_default_schedule() {
        assert_eq!(
            checked_interval_minutes(0),
            BackupConfig::default().interval_minutes
        );
        // Real values pass through untouched.
        assert_eq!(checked_interval_minutes(1), 1);
        assert_eq!(checked_interval_minutes(30), 30);
    }

    /// `retain = 0` means "delete every archive right after writing it", not
    /// "keep no history": rotation runs at the end of each cycle.
    #[test]
    fn zero_retain_falls_back_to_the_default_count() {
        assert_eq!(checked_retain(0), BackupConfig::default().retain);
        assert_eq!(checked_retain(1), 1);
        assert_eq!(checked_retain(48), 48);
    }

    /// What the clamp prevents: rotation with `retain = 0` removes the
    /// archive the same cycle just produced.
    #[test]
    fn rotate_with_zero_retain_would_delete_everything() {
        let tmp = make_temp_dir();
        let dir = tmp.path();
        fs::write(dir.join("lific_20260101_120000.tar.gz"), "fake").unwrap();
        fs::write(dir.join("lific_20260102_120000.tar.gz"), "fake").unwrap();

        rotate_backups(dir, "lific", 0);
        assert_eq!(fs::read_dir(dir).unwrap().count(), 0);

        // With the clamped value the same directory keeps its archives.
        fs::write(dir.join("lific_20260101_120000.tar.gz"), "fake").unwrap();
        fs::write(dir.join("lific_20260102_120000.tar.gz"), "fake").unwrap();
        rotate_backups(dir, "lific", checked_retain(0));
        assert_eq!(fs::read_dir(dir).unwrap().count(), 2);
    }

    // ── LIF-158: audit log retention ──────────────────────────────────────

    /// Insert one audit row aged `days_ago` days, and return its label so the
    /// assertions can name what survived.
    fn seed_audit_row(pool: &DbPool, label: &str, days_ago: i64) {
        let conn = pool.write().unwrap();
        conn.execute(
            "INSERT INTO audit_log (ts, transport, entity_type, entity_id, entity_label, action)
             VALUES (datetime('now', ?1), 'system', 'issue', 1, ?2, 'create')",
            rusqlite::params![format!("-{days_ago} days"), label],
        )
        .unwrap();
    }

    fn audit_labels(pool: &DbPool) -> Vec<String> {
        let conn = pool.read().unwrap();
        let mut stmt = conn
            .prepare("SELECT entity_label FROM audit_log ORDER BY entity_label")
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    #[test]
    fn prune_audit_log_deletes_rows_past_the_window_and_keeps_recent_ones() {
        let pool = crate::db::open_memory().unwrap();
        seed_audit_row(&pool, "ancient", 400);
        seed_audit_row(&pool, "old", 31);
        seed_audit_row(&pool, "recent", 29);
        seed_audit_row(&pool, "today", 0);

        prune_audit_log(&pool, Some(30));

        assert_eq!(audit_labels(&pool), vec!["recent", "today"]);
    }

    #[test]
    fn prune_audit_log_keeps_everything_when_retention_is_unset_or_zero() {
        let pool = crate::db::open_memory().unwrap();
        seed_audit_row(&pool, "ancient", 4000);
        seed_audit_row(&pool, "today", 0);
        let before = audit_labels(&pool);

        // Unset: the default, and the pre-LIF-158 behavior.
        prune_audit_log(&pool, None);
        assert_eq!(audit_labels(&pool), before);

        // Explicit 0 means the same thing, not "delete everything".
        prune_audit_log(&pool, Some(0));
        assert_eq!(audit_labels(&pool), before);
    }

    /// A one-day window must not take today's rows with it: the cutoff is
    /// `now - N days`, so a row written moments ago always survives.
    #[test]
    fn prune_audit_log_one_day_window_spares_todays_rows() {
        let pool = crate::db::open_memory().unwrap();
        seed_audit_row(&pool, "yesterday", 2);
        seed_audit_row(&pool, "now", 0);

        prune_audit_log(&pool, Some(1));

        assert_eq!(audit_labels(&pool), vec!["now"]);
    }

    #[test]
    fn run_backup_prunes_audit_log_only_when_retention_is_configured() {
        let tmp = make_temp_dir();
        let dir = tmp.path();
        let db_path = dir.join("lific.db");
        let backup_dir = make_backup_dir(dir);
        let pool = crate::db::open(&db_path).expect("open test db");
        seed_audit_row(&pool, "ancient", 400);
        seed_audit_row(&pool, "today", 0);

        // Retention off: the backup cycle leaves the audit log alone.
        run_backup(&pool, &db_path, &backup_dir, 5, None);
        assert_eq!(audit_labels(&pool), vec!["ancient", "today"]);

        // Retention on: the same cycle prunes past the window.
        run_backup(&pool, &db_path, &backup_dir, 5, Some(30));
        assert_eq!(audit_labels(&pool), vec!["today"]);
    }
}
