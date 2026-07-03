//! LIF-262: attachment metadata + link bookkeeping.
//!
//! Pure data access over the `attachments` / `attachment_links` tables
//! (migration 031). No file I/O and no authorization live here — the API
//! layer (`api::attachments`) owns byte storage (`crate::storage`) and the
//! project-role gate. Bytes are content-addressed on disk; a row here just
//! records the metadata and the `sha256` that points at the blob.

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::models::{Attachment, AttachmentEntity};
use crate::error::LificError;

/// Insert a new attachment metadata row and return it. The caller has already
/// written the bytes to the content-addressed store and computed `sha256`.
pub fn create_attachment(
    conn: &Connection,
    sha256: &str,
    filename: &str,
    mime: &str,
    size_bytes: i64,
    uploader_id: Option<i64>,
) -> Result<Attachment, LificError> {
    conn.execute(
        "INSERT INTO attachments (sha256, filename, mime, size_bytes, uploader_id)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![sha256, filename, mime, size_bytes, uploader_id],
    )?;
    get_attachment(conn, conn.last_insert_rowid())
}

/// Fetch one attachment by id. `NotFound` when it doesn't exist.
pub fn get_attachment(conn: &Connection, id: i64) -> Result<Attachment, LificError> {
    conn.prepare_cached(
        "SELECT id, sha256, filename, mime, size_bytes, uploader_id, created_at
         FROM attachments WHERE id = ?1",
    )?
    .query_row(params![id], row_to_attachment)
    .optional()?
    .ok_or_else(|| LificError::NotFound(format!("attachment {id} not found")))
}

fn row_to_attachment(row: &rusqlite::Row) -> rusqlite::Result<Attachment> {
    Ok(Attachment {
        id: row.get(0)?,
        sha256: row.get(1)?,
        filename: row.get(2)?,
        mime: row.get(3)?,
        size_bytes: row.get(4)?,
        uploader_id: row.get(5)?,
        created_at: row.get(6)?,
    })
}

/// Delete an attachment row by id. Its `attachment_links` rows cascade via the
/// FK. Returns whether a row was removed.
pub fn delete_attachment(conn: &Connection, id: i64) -> Result<bool, LificError> {
    let changed = conn.execute("DELETE FROM attachments WHERE id = ?1", params![id])?;
    Ok(changed > 0)
}

/// How many `attachments` rows still reference a given content hash. Used by
/// the orphan GC to decide whether the sidecar blob can be removed: bytes are
/// shared across rows, so a file is only deletable when this hits zero.
pub fn count_rows_for_sha(conn: &Connection, sha256: &str) -> Result<i64, LificError> {
    conn.query_row(
        "SELECT COUNT(*) FROM attachments WHERE sha256 = ?1",
        params![sha256],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

// ── Links ────────────────────────────────────────────────────

/// Record that `entity` references `attachment_id`. Idempotent (the composite
/// PK makes a repeat a no-op via ON CONFLICT).
pub fn link_attachment(
    conn: &Connection,
    attachment_id: i64,
    entity: AttachmentEntity,
    entity_id: i64,
) -> Result<(), LificError> {
    conn.execute(
        "INSERT INTO attachment_links (attachment_id, entity_type, entity_id)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(attachment_id, entity_type, entity_id) DO NOTHING",
        params![attachment_id, entity.as_str(), entity_id],
    )?;
    Ok(())
}

/// Remove one link. Silent when the link doesn't exist.
pub fn unlink_attachment(
    conn: &Connection,
    attachment_id: i64,
    entity: AttachmentEntity,
    entity_id: i64,
) -> Result<(), LificError> {
    conn.execute(
        "DELETE FROM attachment_links
         WHERE attachment_id = ?1 AND entity_type = ?2 AND entity_id = ?3",
        params![attachment_id, entity.as_str(), entity_id],
    )?;
    Ok(())
}

/// Total number of links an attachment currently has (across every entity).
/// Exposed for tests and future callers (e.g. a "safe to delete?" precheck).
#[allow(dead_code)]
pub fn count_links(conn: &Connection, attachment_id: i64) -> Result<i64, LificError> {
    conn.query_row(
        "SELECT COUNT(*) FROM attachment_links WHERE attachment_id = ?1",
        params![attachment_id],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

/// List the attachments linked to a given entity, newest-linked last (stable
/// display order for the detail-view "Attachments (n)" section).
pub fn list_for_entity(
    conn: &Connection,
    entity: AttachmentEntity,
    entity_id: i64,
) -> Result<Vec<Attachment>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT a.id, a.sha256, a.filename, a.mime, a.size_bytes, a.uploader_id, a.created_at
         FROM attachments a
         JOIN attachment_links l ON l.attachment_id = a.id
         WHERE l.entity_type = ?1 AND l.entity_id = ?2
         ORDER BY l.created_at, a.id",
    )?;
    let rows = stmt.query_map(params![entity.as_str(), entity_id], row_to_attachment)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// Replace an entity's link set to exactly the given attachment ids. Adds
/// missing links, removes ones no longer referenced. Called after an
/// issue/page description or a comment is saved, with the ids parsed out of the
/// markdown (`/api/attachments/{id}` references). This is the "re-scan on save"
/// mechanism: the source of truth for which attachments an entity uses is the
/// entity's own text, and this reconciles the join table to match.
pub fn sync_entity_links(
    conn: &Connection,
    entity: AttachmentEntity,
    entity_id: i64,
    referenced_ids: &[i64],
) -> Result<(), LificError> {
    // Current links for this entity.
    let mut stmt = conn.prepare_cached(
        "SELECT attachment_id FROM attachment_links WHERE entity_type = ?1 AND entity_id = ?2",
    )?;
    let current: Vec<i64> = stmt
        .query_map(params![entity.as_str(), entity_id], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;

    // Remove links whose attachment id is no longer referenced.
    for existing in &current {
        if !referenced_ids.contains(existing) {
            unlink_attachment(conn, *existing, entity, entity_id)?;
        }
    }
    // Add newly-referenced links (skip ids that don't correspond to a real
    // attachment row — a stale/typo reference in the text shouldn't create a
    // dangling link).
    for id in referenced_ids {
        if !current.contains(id) && attachment_exists(conn, *id)? {
            link_attachment(conn, *id, entity, entity_id)?;
        }
    }
    Ok(())
}

fn attachment_exists(conn: &Connection, id: i64) -> Result<bool, LificError> {
    let exists: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM attachments WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(exists.is_some())
}

// ── Orphan GC ────────────────────────────────────────────────

/// One collectable orphan: an attachment row with zero links, older than the
/// grace window. Carries `sha256` so the sweep can decide whether to delete the
/// sidecar file (only when no OTHER row shares the hash).
#[derive(Debug, Clone)]
pub struct OrphanAttachment {
    pub id: i64,
    pub sha256: String,
}

/// Find attachments with no links whose `created_at` is older than
/// `grace_seconds` ago. The grace window keeps a just-uploaded attachment
/// (linked a moment later, once its markdown is saved) from being swept out
/// from under an in-progress compose.
pub fn find_orphans(
    conn: &Connection,
    grace_seconds: i64,
) -> Result<Vec<OrphanAttachment>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT a.id, a.sha256
         FROM attachments a
         LEFT JOIN attachment_links l ON l.attachment_id = a.id
         WHERE l.attachment_id IS NULL
           AND a.created_at < datetime('now', ?1)",
    )?;
    // A negative grace means "collect everything, even brand-new" (used by
    // tests) — expressed as a positive offset into the future so the
    // `created_at < cutoff` comparison passes for just-created rows. SQLite's
    // datetime modifier needs an explicit sign, and `--1` is invalid, so build
    // the sign ourselves rather than interpolating a bare negative.
    let modifier = if grace_seconds >= 0 {
        format!("-{grace_seconds} seconds")
    } else {
        format!("+{} seconds", -grace_seconds)
    };
    let rows = stmt.query_map(params![modifier], |row| {
        Ok(OrphanAttachment {
            id: row.get(0)?,
            sha256: row.get(1)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// Parse `/api/attachments/{id}` references out of a markdown body, returning
/// the distinct attachment ids it mentions. Matches both image embeds
/// (`![alt](/api/attachments/12)`) and link chips (`[file](/api/attachments/12)`),
/// plus a bare occurrence of the path, so re-scan-on-save catches every form
/// the composer can insert.
pub fn parse_referenced_ids(markdown: &str) -> Vec<i64> {
    let mut ids = Vec::new();
    let needle = "/api/attachments/";
    let bytes = markdown.as_bytes();
    let mut search_from = 0;
    while let Some(rel) = markdown[search_from..].find(needle) {
        let start = search_from + rel + needle.len();
        // Consume the run of ASCII digits following the path.
        let mut end = start;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end > start
            && let Ok(id) = markdown[start..end].parse::<i64>()
            && !ids.contains(&id)
        {
            ids.push(id);
        }
        search_from = end.max(search_from + rel + needle.len());
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::CreateProject;
    use crate::db::{self, queries};

    fn test_db() -> db::DbPool {
        db::open_memory().expect("test db")
    }

    fn seed_user(conn: &Connection, username: &str) -> i64 {
        conn.execute(
            "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot)
             VALUES (?1, ?2, 'x', ?1, 0, 0)",
            params![username, format!("{username}@test.local")],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn seed_issue(conn: &Connection) -> i64 {
        let project = queries::create_project(
            conn,
            &CreateProject {
                name: "Att".into(),
                identifier: "ATT".into(),
                description: String::new(),
                emoji: None,
                lead_user_id: None,
            },
        )
        .unwrap();
        queries::create_issue(
            conn,
            &crate::db::models::CreateIssue {
                project_id: project.id,
                title: "i".into(),
                description: String::new(),
                status: "todo".into(),
                priority: "medium".into(),
                module_id: None,
                start_date: None,
                target_date: None,
                labels: vec![],
            },
        )
        .unwrap()
        .id
    }

    #[test]
    fn create_get_delete_roundtrip() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let uploader = seed_user(&conn, "up");
        let att = create_attachment(&conn, "abc123", "shot.png", "image/png", 42, Some(uploader))
            .unwrap();
        assert_eq!(att.filename, "shot.png");
        assert_eq!(att.size_bytes, 42);

        let fetched = get_attachment(&conn, att.id).unwrap();
        assert_eq!(fetched.sha256, "abc123");

        assert!(delete_attachment(&conn, att.id).unwrap());
        assert!(get_attachment(&conn, att.id).is_err());
    }

    #[test]
    fn link_list_and_count() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let issue = seed_issue(&conn);
        let att = create_attachment(&conn, "h1", "a.pdf", "application/pdf", 10, None).unwrap();

        assert_eq!(count_links(&conn, att.id).unwrap(), 0);
        link_attachment(&conn, att.id, AttachmentEntity::Issue, issue).unwrap();
        link_attachment(&conn, att.id, AttachmentEntity::Issue, issue).unwrap(); // idempotent
        assert_eq!(count_links(&conn, att.id).unwrap(), 1);

        let listed = list_for_entity(&conn, AttachmentEntity::Issue, issue).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, att.id);

        unlink_attachment(&conn, att.id, AttachmentEntity::Issue, issue).unwrap();
        assert_eq!(count_links(&conn, att.id).unwrap(), 0);
    }

    #[test]
    fn dedup_count_rows_for_sha() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        create_attachment(&conn, "same", "a.png", "image/png", 1, None).unwrap();
        create_attachment(&conn, "same", "b.png", "image/png", 1, None).unwrap();
        assert_eq!(count_rows_for_sha(&conn, "same").unwrap(), 2);
    }

    #[test]
    fn sync_entity_links_reconciles() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let issue = seed_issue(&conn);
        let a = create_attachment(&conn, "a", "a.png", "image/png", 1, None).unwrap();
        let b = create_attachment(&conn, "b", "b.png", "image/png", 1, None).unwrap();
        let c = create_attachment(&conn, "c", "c.png", "image/png", 1, None).unwrap();

        // Start linking a + b.
        sync_entity_links(&conn, AttachmentEntity::Issue, issue, &[a.id, b.id]).unwrap();
        let ids: Vec<i64> = list_for_entity(&conn, AttachmentEntity::Issue, issue)
            .unwrap()
            .into_iter()
            .map(|x| x.id)
            .collect();
        assert_eq!(ids, vec![a.id, b.id]);

        // Re-sync to b + c: a is unlinked, c is added.
        sync_entity_links(&conn, AttachmentEntity::Issue, issue, &[b.id, c.id]).unwrap();
        let mut ids: Vec<i64> = list_for_entity(&conn, AttachmentEntity::Issue, issue)
            .unwrap()
            .into_iter()
            .map(|x| x.id)
            .collect();
        ids.sort();
        let mut want = vec![b.id, c.id];
        want.sort();
        assert_eq!(ids, want);
    }

    #[test]
    fn sync_ignores_nonexistent_ids() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let issue = seed_issue(&conn);
        // 99999 doesn't exist — must not create a dangling link.
        sync_entity_links(&conn, AttachmentEntity::Issue, issue, &[99999]).unwrap();
        assert!(
            list_for_entity(&conn, AttachmentEntity::Issue, issue)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn find_orphans_respects_grace_window() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let issue = seed_issue(&conn);
        let linked = create_attachment(&conn, "l", "l.png", "image/png", 1, None).unwrap();
        link_attachment(&conn, linked.id, AttachmentEntity::Issue, issue).unwrap();
        let orphan = create_attachment(&conn, "o", "o.png", "image/png", 1, None).unwrap();

        // Grace of 1 hour: the just-created orphan is too new to collect.
        assert!(find_orphans(&conn, 3600).unwrap().is_empty());

        // Grace of 0 (or negative): the unlinked orphan surfaces; the linked
        // one never does.
        let found = find_orphans(&conn, -1).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, orphan.id);
    }

    #[test]
    fn link_cascades_on_entity_delete_via_trigger() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let issue = seed_issue(&conn);
        let att = create_attachment(&conn, "h", "a.png", "image/png", 1, None).unwrap();
        link_attachment(&conn, att.id, AttachmentEntity::Issue, issue).unwrap();
        assert_eq!(count_links(&conn, att.id).unwrap(), 1);

        queries::delete_issue(&conn, issue).unwrap();
        // Trigger drops the link; the attachment row itself survives (GC's job).
        assert_eq!(count_links(&conn, att.id).unwrap(), 0);
        assert!(get_attachment(&conn, att.id).is_ok());
    }

    // ── parse_referenced_ids ─────────────────────────────────

    #[test]
    fn parse_ids_from_image_and_link_forms() {
        let md = "text ![alt](/api/attachments/12) more [file.pdf](/api/attachments/7) \
                  and bare /api/attachments/12 again and /api/attachments/99";
        let ids = parse_referenced_ids(md);
        assert_eq!(ids, vec![12, 7, 99]); // distinct, in first-seen order
    }

    #[test]
    fn parse_ids_empty_when_none() {
        assert!(parse_referenced_ids("no attachments here").is_empty());
        assert!(parse_referenced_ids("/api/attachments/ trailing slash no id").is_empty());
    }
}
