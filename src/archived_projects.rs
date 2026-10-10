//! Archived projects: a project taken out of the live database and kept as one
//! compressed file on disk, and brought back from it.
//!
//! Archiving writes the project's portable archive (see
//! [`crate::project_archive`]) into [`AttachmentStore::archived_dir`], records
//! it in `archived_projects`, and deletes the project. All of that happens in
//! one writer transaction under the store lock, so the file is exactly the
//! project that was deleted, and a failure at any step leaves the project live
//! with no file behind.
//!
//! Unarchiving imports that file through the ordinary importer and, inside the
//! same transaction, puts back what the portable format leaves out on purpose
//! but this instance still knows: the lead, memberships, and the local
//! accounts behind comments, attachments and history. Publication is
//! recorded but never re-applied: making a project public stays a deliberate
//! act.

use std::io::Read;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::actor::ActorCtx;
use crate::db::{DbPool, queries};
use crate::error::LificError;
use crate::project_archive::{self, Grant, ImportHooks, ImportedIds, Limits};
use crate::storage::AttachmentStore;

type Result<T> = std::result::Result<T, LificError>;

/// File names this module writes, and the only ones it will read back.
const SUFFIX: &str = ".lific.tar.gz";
const MAX_FILE_NAME: usize = 160;

/// One row of the archived-projects listing.
#[derive(Debug, Clone, Serialize)]
pub struct ArchivedProject {
    pub id: i64,
    pub identifier: String,
    pub name: String,
    pub description: String,
    pub emoji: Option<String>,
    pub file_name: String,
    pub size_bytes: i64,
    pub issue_count: i64,
    pub page_count: i64,
    pub archived_at: String,
    pub archived_by: Option<i64>,
    pub archived_by_name: Option<String>,
    /// Whether the project was published when it was archived. Unarchiving
    /// brings it back private either way.
    pub was_public: bool,
    /// Whether the archive file is where the row says. A missing file cannot
    /// be unarchived; the listing says so instead of failing later.
    pub file_present: bool,
}

/// What the portable format drops that a same-instance restore can put back.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Restore {
    lead_user_id: Option<i64>,
    is_public: bool,
    members: Vec<Member>,
    authors: Vec<Author>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Member {
    user_id: i64,
    role: String,
}

/// The local account (and, for history rows, the transport) behind one row
/// the archive will carry only as "Name (imported)".
#[derive(Debug, Serialize, Deserialize)]
struct Author {
    table: String,
    id: i64,
    user_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transport: Option<String>,
}

/// Tables whose rows name a local account, and the column that holds it.
/// Comments and attachments reference `users` by foreign key; the two
/// history tables keep the ID without one, on purpose (migration 018).
const AUTHORED: &[(&str, &str, bool)] = &[
    ("comments", "user_id", false),
    ("attachments", "uploader_id", false),
    ("audit_log", "actor_user_id", true),
    ("status_transitions", "actor_user_id", true),
];

/// The result of archiving: the new listing row, plus who could see the
/// project, for the realtime notice that it is gone.
#[derive(Debug)]
pub struct Archived {
    pub record: ArchivedProject,
    pub project_id: i64,
    pub audience: Option<Vec<i64>>,
}

/// The result of unarchiving.
#[derive(Debug, Serialize)]
pub struct Unarchived {
    pub project_id: i64,
    pub identifier: String,
    pub was_public: bool,
}

pub(crate) fn valid_file_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(SUFFIX) else {
        return false;
    };
    name.len() <= MAX_FILE_NAME
        && stem
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && stem
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn internal(context: &str, error: impl std::fmt::Display) -> LificError {
    LificError::Internal(format!("{context}: {error}"))
}

fn ensure_dir(dir: &Path) -> Result<()> {
    let existed = dir.is_dir();
    std::fs::create_dir_all(dir).map_err(|e| internal("create archived projects dir", e))?;
    // A new directory's own entry must be durable before a project is
    // deleted on the strength of a file inside it.
    if !existed && let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
        sync_dir(parent)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| internal("secure archived projects dir", e))?;
    }
    Ok(())
}

#[cfg_attr(
    not(unix),
    expect(clippy::unnecessary_wraps, reason = "fsync is Unix-only")
)]
fn sync_dir(_dir: &Path) -> Result<()> {
    #[cfg(unix)]
    std::fs::File::open(_dir)
        .and_then(|dir| dir.sync_all())
        .map_err(|e| internal("sync archived projects dir", e))?;
    Ok(())
}

/// A name no other file in `dir` has: `<IDENT>-<UTC timestamp>.lific.tar.gz`,
/// with a counter when one project is archived twice in a second.
fn fresh_file_name(dir: &Path, identifier: &str) -> String {
    let ident: String = identifier
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let ident = if ident.starts_with(|c: char| c.is_ascii_alphanumeric()) {
        ident
    } else {
        format!("project{ident}")
    };
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let base = format!("{ident}-{stamp}");
    let mut name = format!("{base}{SUFFIX}");
    let mut n = 2;
    while std::fs::symlink_metadata(dir.join(&name)).is_ok() {
        name = format!("{base}-{n}{SUFFIX}");
        n += 1;
    }
    name
}

fn hash_file(path: &Path) -> Result<(String, i64)> {
    let mut file = std::fs::File::open(path).map_err(|e| internal("open archive file", e))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut size: u64 = 0;
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| internal("read archive file", e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    let size = i64::try_from(size).map_err(|e| internal("archive file size", e))?;
    Ok((crate::auth::hex_encode(&hasher.finalize()), size))
}

fn collect_restore(conn: &Connection, project_id: i64) -> Result<Restore> {
    let (lead_user_id, is_public): (Option<i64>, bool) = conn.query_row(
        "SELECT lead_user_id, is_public FROM projects WHERE id = ?1",
        [project_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let members = conn
        .prepare(
            "SELECT user_id, role FROM project_members WHERE project_id = ?1 ORDER BY user_id",
        )?
        .query_map([project_id], |r| {
            Ok(Member {
                user_id: r.get(0)?,
                role: r.get(1)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut authors = Vec::new();
    for &(table, column, history) in AUTHORED {
        let scope = project_archive::table_scope(table)?;
        // Rows that already came in through an import keep their imported
        // attribution: there is no local account behind them to restore.
        let sql = if history {
            format!(
                "SELECT id, {column}, transport FROM {table}
                 WHERE ({scope}) AND imported_source IS NULL AND imported_author IS NULL"
            )
        } else {
            format!(
                "SELECT id, {column}, NULL FROM {table}
                 WHERE ({scope}) AND imported_author IS NULL AND {column} IS NOT NULL"
            )
        };
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([project_id], |r| {
            Ok(Author {
                table: table.to_string(),
                id: r.get(0)?,
                user_id: r.get(1)?,
                transport: r.get(2)?,
            })
        })?;
        for row in rows {
            authors.push(row?);
        }
    }
    Ok(Restore {
        lead_user_id,
        is_public,
        members,
        authors,
    })
}

fn user_exists(conn: &Connection, id: i64) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM users WHERE id = ?1)",
        [id],
        |r| r.get(0),
    )?)
}

/// Put authorship back. Runs with triggers suspended, so none of these
/// corrections is itself audited or counted as an edit.
fn restore_authors(conn: &Connection, ids: &ImportedIds<'_>, restore: &Restore) -> Result<()> {
    for author in &restore.authors {
        let Some(&(table, column, history)) = AUTHORED.iter().find(|(t, ..)| *t == author.table)
        else {
            continue;
        };
        let Some(id) = ids.get(table, author.id) else {
            continue;
        };
        if history {
            // History keeps the account ID even after the account is gone,
            // which is exactly what it would hold had the project stayed live.
            // An audit row about an entity that no longer exists imports with
            // `entity_id = 0`, and its provenance is the only record of what it
            // referred to, so that row keeps it.
            let transport = author.transport.as_deref().unwrap_or("system");
            let provenance = if table == "audit_log" {
                "CASE WHEN entity_id = 0 THEN imported_source END"
            } else {
                "NULL"
            };
            conn.execute(
                &format!(
                    "UPDATE {table} SET {column} = ?1, transport = ?2,
                     imported_author = NULL, imported_source = {provenance} WHERE id = ?3"
                ),
                params![author.user_id, transport, id],
            )?;
        } else if let Some(user_id) = author.user_id
            && user_exists(conn, user_id)?
        {
            conn.execute(
                &format!("UPDATE {table} SET {column} = ?1, imported_author = NULL WHERE id = ?2"),
                params![user_id, id],
            )?;
        }
    }
    Ok(())
}

/// Put the lead and memberships back. Runs after the importer's own lead
/// grant, with its actor recorded, so each restored membership is audited as
/// that admin's doing. Accounts deleted since the archive are skipped.
fn restore_people(conn: &Connection, project: i64, restore: &Restore) -> Result<()> {
    for member in &restore.members {
        if !user_exists(conn, member.user_id)? {
            continue;
        }
        conn.execute(
            "INSERT INTO project_members(project_id, user_id, role) VALUES (?1, ?2, ?3)
             ON CONFLICT(project_id, user_id) DO UPDATE SET role = excluded.role
             WHERE role <> excluded.role",
            params![project, member.user_id, member.role],
        )?;
    }
    let lead = match restore.lead_user_id {
        Some(id) if user_exists(conn, id)? => Some(id),
        _ => None,
    };
    conn.execute(
        "UPDATE projects SET lead_user_id = ?1 WHERE id = ?2 AND lead_user_id IS NOT ?1",
        params![lead, project],
    )?;
    Ok(())
}

/// Archive project `project_id`.
///
/// `authorize` runs first inside the writer transaction and returns the
/// actor the archive is attributed to; refusing there leaves everything
/// untouched.
pub fn archive(
    pool: &DbPool,
    store: &AttachmentStore,
    project_id: i64,
    authorize: &dyn Fn(&Connection) -> Result<ActorCtx>,
) -> Result<Archived> {
    project_archive::with_limits(Limits::CLI, || {
        store.with_lock(|store| {
            let dir = store.archived_dir().to_path_buf();
            ensure_dir(&dir)?;
            let mut conn = pool.write()?;
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let actor = authorize(&tx)?;
            crate::actor::stamp(&tx, &actor);
            let project = queries::get_project(&tx, project_id)?;
            let restore = collect_restore(&tx, project_id)?;
            let (issue_count, page_count): (i64, i64) = tx.query_row(
                "SELECT (SELECT count(*) FROM issues WHERE project_id = ?1 AND deleted_at IS NULL),
                        (SELECT count(*) FROM pages WHERE project_id = ?1 AND deleted_at IS NULL)",
                [project_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let file_name = fresh_file_name(&dir, &project.identifier);
            let path = dir.join(&file_name);
            project_archive::write_archive_locked(&tx, store, project_id, &path)?;
            let committed = (|| {
                let (sha256, size_bytes) = hash_file(&path)?;
                let restore_json =
                    serde_json::to_string(&restore).map_err(|e| internal("encode restore", e))?;
                let id: i64 = tx.query_row(
                    "INSERT INTO archived_projects
                       (identifier, name, description, emoji, file_name, sha256, size_bytes,
                        issue_count, page_count, restore, archived_by)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                     RETURNING id",
                    params![
                        project.identifier,
                        project.name,
                        project.description,
                        project.emoji,
                        file_name,
                        sha256,
                        size_bytes,
                        issue_count,
                        page_count,
                        restore_json,
                        actor.user_id,
                    ],
                    |r| r.get(0),
                )?;
                let (_, audience) = queries::delete_project_with_audience(&tx, project_id)?;
                let record = get_on(&tx, store, id)?;
                Ok((record, audience))
            })()
            .and_then(|done| {
                tx.commit()?;
                Ok(done)
            });
            match committed {
                Ok((record, audience)) => Ok(Archived {
                    record,
                    project_id,
                    audience,
                }),
                Err(error) => {
                    // The project is still live, so the file is only a stray
                    // copy. Remove it rather than leave an untracked archive.
                    let _ = std::fs::remove_file(&path);
                    let _ = sync_dir(&dir);
                    Err(error)
                }
            }
        })
    })
}

/// Bring archived project `archive_id` back.
///
/// `authorize` names the admin the project is handed to, exactly as for a
/// project archive import; it runs first inside the import's writer
/// transaction. The listing row is removed in that same transaction, and the
/// file once it has committed.
pub fn unarchive(
    pool: &DbPool,
    store: &AttachmentStore,
    archive_id: i64,
    authorize: &dyn Fn(&Connection) -> Result<Grant>,
) -> Result<Unarchived> {
    let (identifier, file_name, sha256, restore_json): (String, String, String, String) = {
        let conn = pool.read()?;
        conn.query_row(
            "SELECT identifier, file_name, sha256, restore FROM archived_projects WHERE id = ?1",
            [archive_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?
        .ok_or_else(|| LificError::NotFound(format!("archived project {archive_id} not found")))?
    };
    if !valid_file_name(&file_name) {
        return Err(LificError::Internal(format!(
            "archived project {archive_id} names an invalid file"
        )));
    }
    let path = store.archived_dir().join(&file_name);
    if !std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
        return Err(LificError::NotFound(format!(
            "the archive file {file_name} is missing from {}",
            store.archived_dir().display()
        )));
    }
    let (actual, _) = hash_file(&path)?;
    if actual != sha256 {
        return Err(LificError::Conflict(format!(
            "the archive file {file_name} does not match the checksum recorded when it was archived"
        )));
    }
    {
        let conn = pool.read()?;
        let taken: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE identifier = ?1 COLLATE NOCASE)",
            [&identifier],
            |r| r.get(0),
        )?;
        if taken {
            return Err(LificError::Conflict(format!(
                "a project with the identifier {identifier} already exists; change its identifier, then unarchive"
            )));
        }
    }
    let restore: Restore =
        serde_json::from_str(&restore_json).map_err(|e| internal("read restore data", e))?;

    let restore_hook =
        |conn: &Connection, ids: &ImportedIds<'_>| restore_authors(conn, ids, &restore);
    let finish_hook = |conn: &Connection, ids: &ImportedIds<'_>| {
        restore_people(conn, ids.project, &restore)?;
        let removed = conn.execute("DELETE FROM archived_projects WHERE id = ?1", [archive_id])?;
        if removed == 0 {
            return Err(LificError::Conflict(format!(
                "archived project {archive_id} was already unarchived"
            )));
        }
        Ok(())
    };
    let outcome = project_archive::import_with_hooks(
        pool,
        store,
        &path,
        Limits::CLI,
        authorize,
        &ImportHooks {
            restore: &restore_hook,
            finish: &finish_hook,
        },
    )?;

    // The project is back and nothing tracks the file any more. Failing to
    // remove it leaves a harmless stray, never a lost project. The WAL runs
    // with `synchronous = NORMAL`, so the commit is only certainly on disk
    // after a checkpoint: until one completes, the file is the recovery copy.
    let removed = durable(pool).and_then(|()| {
        store.with_lock(|store| {
            std::fs::remove_file(store.archived_dir().join(&file_name))
                .map_err(|e| internal("remove archive file", e))?;
            sync_dir(store.archived_dir())
        })
    });
    if let Err(error) = removed {
        tracing::warn!(%error, file = %file_name, "unarchived project left its archive file behind");
    }

    Ok(Unarchived {
        project_id: outcome.project_id,
        identifier: outcome.report.project,
        was_public: restore.is_public,
    })
}

/// Make every committed transaction durable, or say it could not.
fn durable(pool: &DbPool) -> Result<()> {
    let conn = pool.write()?;
    let busy: i64 = conn.query_row("PRAGMA wal_checkpoint(FULL)", [], |r| r.get(0))?;
    if busy != 0 {
        return Err(LificError::Internal(
            "the database checkpoint did not complete".into(),
        ));
    }
    Ok(())
}

const SELECT: &str = "SELECT a.id, a.identifier, a.name, a.description, a.emoji, a.file_name,
        a.size_bytes, a.issue_count, a.page_count, a.archived_at, a.archived_by,
        COALESCE(NULLIF(u.display_name, ''), u.username),
        COALESCE(json_extract(a.restore, '$.is_public'), 0)
   FROM archived_projects a LEFT JOIN users u ON u.id = a.archived_by";

fn from_row(row: &rusqlite::Row<'_>, dir: &Path) -> rusqlite::Result<ArchivedProject> {
    let file_name: String = row.get(5)?;
    let file_present = valid_file_name(&file_name)
        && std::fs::symlink_metadata(dir.join(&file_name)).is_ok_and(|m| m.is_file());
    Ok(ArchivedProject {
        id: row.get(0)?,
        identifier: row.get(1)?,
        name: row.get(2)?,
        description: row.get(3)?,
        emoji: row.get(4)?,
        file_name,
        size_bytes: row.get(6)?,
        issue_count: row.get(7)?,
        page_count: row.get(8)?,
        archived_at: row.get(9)?,
        archived_by: row.get(10)?,
        archived_by_name: row.get(11)?,
        was_public: row.get(12)?,
        file_present,
    })
}

fn get_on(conn: &Connection, store: &AttachmentStore, id: i64) -> Result<ArchivedProject> {
    let dir: PathBuf = store.archived_dir().to_path_buf();
    conn.query_row(&format!("{SELECT} WHERE a.id = ?1"), [id], |r| {
        from_row(r, &dir)
    })
    .optional()?
    .ok_or_else(|| LificError::NotFound(format!("archived project {id} not found")))
}

/// Every archived project, most recently archived first.
pub fn list(conn: &Connection, store: &AttachmentStore) -> Result<Vec<ArchivedProject>> {
    let dir = store.archived_dir();
    let mut stmt = conn.prepare(&format!("{SELECT} ORDER BY a.archived_at DESC, a.id DESC"))?;
    let rows = stmt.query_map([], |r| from_row(r, dir))?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

#[cfg(test)]
mod tests;
