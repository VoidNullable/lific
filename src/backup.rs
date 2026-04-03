use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rusqlite::backup;
use tracing::{error, info, warn};

use crate::config::BackupConfig;
use crate::db::DbPool;

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

    tokio::spawn(async move {
        if let Err(e) = std::fs::create_dir_all(&backup_dir) {
            error!(dir = %backup_dir.display(), error = %e, "failed to create backup directory");
            return;
        }

        info!(
            dir = %backup_dir.display(),
            interval_min = config.interval_minutes,
            retain = retain,
            "backup task started"
        );

        // Run initial backup after a short delay (let the server finish starting)
        tokio::time::sleep(Duration::from_secs(5)).await;
        run_backup(&pool, &db_path, &backup_dir, retain);

        // Then run on interval
        let mut interval_timer = tokio::time::interval(interval);
        interval_timer.tick().await; // skip first immediate tick
        loop {
            interval_timer.tick().await;
            run_backup(&pool, &db_path, &backup_dir, retain);
        }
    })
}

/// Perform a single backup using SQLite's online backup API.
fn run_backup(pool: &DbPool, db_path: &Path, backup_dir: &Path, retain: usize) {
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let db_stem = db_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("lific");
    let backup_filename = format!("{db_stem}_{timestamp}.db");
    let backup_path = backup_dir.join(&backup_filename);

    // Use a read connection so we don't block writes
    let source = match pool.read() {
        Ok(conn) => conn,
        Err(e) => {
            error!(error = %e, "failed to acquire read connection for backup");
            return;
        }
    };

    // Open a new connection to the backup destination
    let mut dest = match rusqlite::Connection::open(&backup_path) {
        Ok(conn) => conn,
        Err(e) => {
            error!(path = %backup_path.display(), error = %e, "failed to open backup destination");
            return;
        }
    };

    // Use SQLite's backup API -- consistent snapshot, no locking
    match backup::Backup::new(&source, &mut dest) {
        Ok(b) => {
            // Step through the backup in chunks to avoid holding locks too long
            // -1 means copy all pages at once (fine for small DBs)
            if let Err(e) = b.step(-1) {
                error!(error = %e, "backup step failed");
                let _ = std::fs::remove_file(&backup_path);
                return;
            }
            let size = std::fs::metadata(&backup_path)
                .map(|m| m.len())
                .unwrap_or(0);
            info!(
                path = %backup_path.display(),
                size_kb = size / 1024,
                "backup completed"
            );
        }
        Err(e) => {
            error!(error = %e, "failed to initialize backup");
            let _ = std::fs::remove_file(&backup_path);
            return;
        }
    }

    // Drop the dest connection to flush
    drop(dest);

    // Rotate old backups
    rotate_backups(backup_dir, db_stem, retain);
}

/// Keep only the N most recent backups, delete the rest.
fn rotate_backups(backup_dir: &Path, db_stem: &str, retain: usize) {
    let prefix = format!("{db_stem}_");
    let mut backups: Vec<PathBuf> = match std::fs::read_dir(backup_dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.extension().and_then(|e| e.to_str()) == Some("db")
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with(&prefix))
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
