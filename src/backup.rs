use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tracing::{error, info, warn};

use crate::config::BackupConfig;
use crate::db::DbPool;
use crate::dump;

/// Start the background backup task. Returns the JoinHandle.
pub fn start_backup_task(
    pool: Arc<DbPool>,
    db_path: PathBuf,
    config: BackupConfig,
) -> tokio::task::JoinHandle<()> {
    let backup_dir = if config.dir.is_absolute() {
        config.dir.clone()
    } else if let Some(parent) = db_path.parent() {
        parent.join(&config.dir)
    } else {
        config.dir.clone()
    };

    let interval = Duration::from_secs(config.interval_minutes * 60);
    let retain = config.retain;
    let audit_retention_days = config.audit_retention_days;

    tokio::spawn(async move {
        if let Err(e) = std::fs::create_dir_all(&backup_dir) {
            error!(dir = %backup_dir.display(), error = %e, "failed to create backup directory");
            return;
        }

        info!(
            dir = %backup_dir.display(),
            interval_min = config.interval_minutes,
            retain = retain,
            audit_retention_days = audit_retention_days.unwrap_or(0),
            "backup task started"
        );

        // Run initial backup after a short delay (let the server finish starting)
        tokio::time::sleep(Duration::from_secs(5)).await;
        run_backup(&pool, &db_path, &backup_dir, retain, audit_retention_days);

        // Then run on interval
        let mut interval_timer = tokio::time::interval(interval);
        interval_timer.tick().await; // skip first immediate tick
        loop {
            interval_timer.tick().await;
            run_backup(&pool, &db_path, &backup_dir, retain, audit_retention_days);
        }
    })
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
fn run_backup(
    pool: &DbPool,
    db_path: &Path,
    backup_dir: &Path,
    retain: usize,
    audit_retention_days: Option<u32>,
) {
    let db_stem = db_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("lific");

    // Sweep staging leftovers from a previous crashed/failed run first, so
    // they get cleaned even if this run's backup fails too (LIF-329).
    sweep_stale_tmps(backup_dir, db_stem);

    let filename = dump::archive_filename(db_stem, &dump::archive_timestamp());
    let backup_path = backup_dir.join(&filename);

    match dump::write_dump(pool, db_path, &backup_path) {
        Ok(manifest) => {
            let size = std::fs::metadata(&backup_path)
                .map(|m| m.len())
                .unwrap_or(0);
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
            return;
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
        Ok(deleted) => info!(deleted, retention_days = days, "pruned old audit log entries"),
        Err(e) => warn!(error = %e, "audit log pruning failed"),
    }
}

/// How old a dump staging file must be before the sweep considers it stale.
/// A live dump finishes in seconds; anything an hour old is a crash leftover.
const STALE_TMP_AGE: Duration = Duration::from_secs(60 * 60);

/// Delete stale dump staging files leaked by a crash mid-backup (LIF-329).
///
/// `write_dump` stages `{stem}_<ts>.tar.dbsnapshot.tmp` and
/// `{stem}_<ts>.tar.archive.tmp` beside the output archive and cleans them
/// itself on success or error — but a hard crash between staging and cleanup
/// strands them, and `rotate_backups` only matches `.tar.gz`/`.db`, so they
/// would otherwise accumulate forever. Age-gated so an in-flight dump's
/// staging files are never swept.
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
        if !name.starts_with(&prefix)
            || !(name.ends_with(".dbsnapshot.tmp") || name.ends_with(".archive.tmp"))
        {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > STALE_TMP_AGE);
        if stale {
            match std::fs::remove_file(&path) {
                Ok(()) => info!(path = %path.display(), "removed stale backup staging file"),
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "failed to remove stale staging file")
                }
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
                let name = match p.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n,
                    None => return false,
                };
                if !name.starts_with(&prefix) {
                    return false;
                }
                // New archives (`.tar.gz`) or legacy snapshots (`.db`).
                name.ends_with(".tar.gz")
                    || p.extension().and_then(|e| e.to_str()) == Some("db")
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
    fn backdate(path: &Path, by: Duration) {
        let f = fs::File::options().write(true).open(path).unwrap();
        f.set_modified(std::time::SystemTime::now() - by).unwrap();
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
        for p in [&stale_snap, &stale_arch, &fresh_arch, &other_stem, &real_backup] {
            fs::write(p, "x").unwrap();
        }
        backdate(&stale_snap, old);
        backdate(&stale_arch, old);
        backdate(&other_stem, old);
        backdate(&real_backup, old);

        sweep_stale_tmps(dir, "lific");

        assert!(!stale_snap.exists(), "stale snapshot tmp must be swept");
        assert!(!stale_arch.exists(), "stale archive tmp must be swept");
        assert!(fresh_arch.exists(), "fresh staging tmp must survive");
        assert!(other_stem.exists(), "other stems are not ours to sweep");
        assert!(real_backup.exists(), "real archives are rotation's job, not the sweep's");
    }

    #[test]
    fn run_backup_sweeps_stale_tmps_even_when_it_writes_nothing_new() {
        // The sweep runs at the top of run_backup, so leftovers age out on
        // the next interval even if that run's dump were to fail.
        let tmp = make_temp_dir();
        let dir = tmp.path();
        let db_path = dir.join("lific.db");
        let backup_dir = dir.join("backups");
        fs::create_dir_all(&backup_dir).unwrap();
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
        let backup_dir = dir.join("backups");
        fs::create_dir_all(&backup_dir).unwrap();

        // Seed the DB and an attachments sidecar dir next to it.
        let pool = crate::db::open(&db_path).expect("open test db");
        {
            let conn = pool.write().unwrap();
            crate::db::queries::create_project(
                &conn,
                &crate::db::models::CreateProject {
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
        fs::write(att_dir.join("deadbeefsha"), b"blob contents").unwrap();
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
        assert!(names.iter().any(|n| n == crate::dump::ARCHIVE_MANIFEST_NAME));
        assert!(names.iter().any(|n| n == "attachments/deadbeefsha"));
        assert!(
            !names.iter().any(|n| n.ends_with(".tmp")),
            "in-progress .tmp writes must not be archived: {names:?}"
        );
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
        let backup_dir = dir.join("backups");
        fs::create_dir_all(&backup_dir).unwrap();
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
