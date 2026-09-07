use rusqlite::{Connection, OptionalExtension, params};

use crate::db::models::*;
use crate::error::LificError;

use super::{TOMBSTONE_NOW, unescape_text};

fn page_from_row(row: &rusqlite::Row) -> rusqlite::Result<Page> {
    let project_id: Option<i64> = row.get(1)?;
    let sequence: Option<i64> = row.get(2)?;
    let project_ident: Option<String> = row.get(3)?;
    let identifier = match (project_ident, sequence) {
        (Some(pi), Some(seq)) => format!("{pi}-DOC-{seq}"),
        (None, Some(seq)) => format!("DOC-{seq}"),
        _ => String::new(),
    };
    Ok(Page {
        id: row.get(0)?,
        project_id,
        sequence,
        identifier,
        folder_id: row.get(4)?,
        title: row.get(5)?,
        content: row.get(6)?,
        sort_order: row.get(7)?,
        status: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
        pinned: row.get(11)?,
        seq: row.get::<_, Option<i64>>(12)?.unwrap_or(0),
        labels: Vec::new(),
    })
}

const PAGE_SELECT: &str = "SELECT pg.id, pg.project_id, pg.sequence, p.identifier,
            pg.folder_id, pg.title, pg.content, pg.sort_order, pg.status,
            pg.created_at, pg.updated_at, pg.pinned, pg.seq
     FROM pages pg
     LEFT JOIN projects p ON p.id = pg.project_id";

/// Look up label names attached to a single page. Used by `get_page` to
/// populate `Page.labels`. Returns empty for pages with no labels (or for
/// workspace pages, which can't carry labels yet — LIF-105).
fn page_labels(conn: &Connection, page_id: i64) -> Result<Vec<String>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT l.name FROM labels l
         JOIN page_labels pl ON pl.label_id = l.id
         WHERE pl.page_id = ?1
         ORDER BY l.name",
    )?;
    let rows = stmt.query_map(params![page_id], |row| row.get(0))?;
    rows.collect::<Result<Vec<String>, _>>().map_err(Into::into)
}

/// Bulk-populate `labels` on each page in `pages` using one round-trip,
/// mirroring the issue list pattern in `list_issues`. No-op when the list
/// is empty so the placeholder-builder doesn't generate `IN ()`.
fn populate_page_labels(conn: &Connection, pages: &mut [Page]) -> Result<(), LificError> {
    if pages.is_empty() {
        return Ok(());
    }
    let ids: Vec<i64> = pages.iter().map(|p| p.id).collect();
    let placeholders: String = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT pl.page_id, l.name FROM page_labels pl
         JOIN labels l ON l.id = pl.label_id
         WHERE pl.page_id IN ({placeholders})
         ORDER BY l.name"
    );
    let params: Vec<Box<dyn rusqlite::types::ToSql>> = ids
        .iter()
        .map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>)
        .collect();
    let params_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_refs.as_slice(), |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (page_id, label_name) = row?;
        if let Some(page) = pages.iter_mut().find(|p| p.id == page_id) {
            page.labels.push(label_name);
        }
    }
    Ok(())
}

// Positional filters mirror the REST/CLI call sites; a params struct would
// churn every caller. Pagination (limit/offset) pushed it past the 7-arg
// lint threshold (LIF-137).
#[allow(clippy::too_many_arguments)]
pub fn list_pages(
    conn: &Connection,
    project_id: Option<i64>,
    folder_id: Option<i64>,
    label: Option<&str>,
    status: Option<&str>,
    order_by: Option<&str>,
    order: Option<&str>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<Page>, LificError> {
    Ok(list_pages_page(
        conn, project_id, folder_id, label, status, order_by, order, limit, offset,
    )?
    .items)
}

/// [`list_pages`] as a [`Page`](super::Page) (the pagination kind, not the
/// document kind). An absent `limit` still means "everything", and such a read
/// reports `has_more: false` because there is nothing past everything. A
/// present limit is clamped here and over-fetched by one, so `has_more` holds
/// at the cap (LIF-388).
#[allow(clippy::too_many_arguments)]
pub fn list_pages_page(
    conn: &Connection,
    project_id: Option<i64>,
    folder_id: Option<i64>,
    label: Option<&str>,
    status: Option<&str>,
    order_by: Option<&str>,
    order: Option<&str>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<super::Page<Page>, LificError> {
    // Build the query incrementally so the optional label filter can
    // graft on a JOIN. Using DISTINCT shields against a page joining
    // multiple labels and double-appearing if the filter weren't there
    // (it should be impossible since label is filtered by name, but
    // DISTINCT keeps the query robust to future joins).
    let mut sql = String::from(
        "SELECT DISTINCT pg.id, pg.project_id, pg.sequence, p.identifier,
                pg.folder_id, pg.title, pg.content, pg.sort_order, pg.status,
                pg.created_at, pg.updated_at, pg.pinned, pg.seq
         FROM pages pg
         LEFT JOIN projects p ON p.id = pg.project_id",
    );
    // LIF-438: tombstones keep their row for delta sync; reads are live-only.
    let mut conditions: Vec<String> = vec!["pg.deleted_at IS NULL".to_string()];
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    match (project_id, folder_id) {
        (Some(pid), Some(fid)) => {
            conditions.push(format!("pg.project_id = ?{}", param_values.len() + 1));
            param_values.push(Box::new(pid));
            conditions.push(format!("pg.folder_id = ?{}", param_values.len() + 1));
            param_values.push(Box::new(fid));
        }
        (Some(pid), None) => {
            conditions.push(format!("pg.project_id = ?{}", param_values.len() + 1));
            param_values.push(Box::new(pid));
        }
        (None, _) => {
            conditions.push("pg.project_id IS NULL".to_string());
        }
    }

    if let Some(label_name) = label {
        sql.push_str(
            " JOIN page_labels pl ON pl.page_id = pg.id JOIN labels l ON l.id = pl.label_id",
        );
        conditions.push(format!("l.name = ?{}", param_values.len() + 1));
        param_values.push(Box::new(label_name.to_string()));
    }

    if let Some(status_filter) = status {
        conditions.push(format!("pg.status = ?{}", param_values.len() + 1));
        param_values.push(Box::new(status_filter.to_string()));
    }

    if !conditions.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conditions.join(" AND "));
    }
    // Whitelisted ORDER BY — user input selects from fixed SQL fragments,
    // it is never interpolated directly. `pg.id` tiebreaks for stability.
    let dir = match order {
        None | Some("asc") => "ASC",
        Some("desc") => "DESC",
        Some(other) => {
            return Err(LificError::BadRequest(format!(
                "invalid order '{other}'. Use asc or desc."
            )));
        }
    };
    let order_col = match order_by {
        None | Some("sort_order") => "pg.sort_order",
        Some("title") => "pg.title COLLATE NOCASE",
        Some("status") => "pg.status",
        Some("created") | Some("created_at") => "pg.created_at",
        Some("updated") | Some("updated_at") => "pg.updated_at",
        Some(other) => {
            return Err(LificError::BadRequest(format!(
                "invalid order_by '{other}'. Use sort_order, title, status, created, or updated."
            )));
        }
    };
    sql.push_str(&format!(" ORDER BY {order_col} {dir}, pg.id {dir}"));

    // Optional pagination. `None` limit means "no limit" so existing
    // callers (export, REST, CLI) keep their unbounded behavior; only the
    // MCP list_resources(page) branch passes explicit values (LIF-137).
    // Offset applies whenever provided.
    let mut page_limit = super::NO_LIMIT;
    if limit.is_some() || offset.is_some() {
        let (limit, offset) = super::page_unbounded(limit, offset);
        page_limit = limit;
        sql.push_str(&format!(
            " LIMIT ?{} OFFSET ?{}",
            param_values.len() + 1,
            param_values.len() + 2
        ));
        param_values.push(Box::new(super::over_fetch(limit)));
        param_values.push(Box::new(offset));
    }

    let params_refs: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_refs.as_slice(), page_from_row)?;
    let rows: Vec<Page> = rows.collect::<Result<Vec<_>, _>>()?;
    // Trim before the label round-trip so the over-fetched row costs nothing.
    let mut page = match page_limit {
        super::NO_LIMIT => super::Page::complete(rows),
        limit => super::Page::from_over_fetch(rows, limit),
    };
    populate_page_labels(conn, &mut page.items)?;
    Ok(page)
}

pub fn get_page(conn: &Connection, id: i64) -> Result<Page, LificError> {
    let mut page = conn
        .query_row(
            &format!("{PAGE_SELECT} WHERE pg.id = ?1 AND pg.deleted_at IS NULL"),
            params![id],
            page_from_row,
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                LificError::NotFound(format!("page {id} not found"))
            }
            _ => e.into(),
        })?;
    page.labels = page_labels(conn, id)?;
    Ok(page)
}

pub fn page_project_id(conn: &Connection, id: i64) -> Result<Option<i64>, LificError> {
    conn.query_row(
        "SELECT project_id FROM pages WHERE id = ?1 AND deleted_at IS NULL",
        [id],
        |row| row.get(0),
    )
    .map_err(|error| match error {
        rusqlite::Error::QueryReturnedNoRows => {
            LificError::NotFound(format!("page {id} not found"))
        }
        other => other.into(),
    })
}

pub fn resolve_page_identifier(conn: &Connection, identifier: &str) -> Result<i64, LificError> {
    let parts: Vec<&str> = identifier.split('-').collect();
    match parts.as_slice() {
        [project_ident, "DOC", seq_str] => {
            let sequence: i64 = seq_str.parse().map_err(|_| {
                LificError::BadRequest(format!("invalid page identifier: {identifier}"))
            })?;
            conn.query_row(
                "SELECT pg.id FROM pages pg JOIN projects p ON p.id = pg.project_id WHERE p.identifier = ?1 AND pg.sequence = ?2 AND pg.deleted_at IS NULL",
                params![project_ident, sequence], |row| row.get(0),
            ).map_err(|e| match e { rusqlite::Error::QueryReturnedNoRows => LificError::NotFound(format!("page {identifier} not found")), _ => e.into() })
        }
        ["DOC", seq_str] => {
            let sequence: i64 = seq_str.parse().map_err(|_| {
                LificError::BadRequest(format!("invalid page identifier: {identifier}"))
            })?;
            conn.query_row(
                "SELECT id FROM pages WHERE project_id IS NULL AND sequence = ?1 AND deleted_at IS NULL",
                params![sequence],
                |row| row.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    LificError::NotFound(format!("page {identifier} not found"))
                }
                _ => e.into(),
            })
        }
        _ => Err(LificError::BadRequest(format!(
            "invalid page identifier: {identifier}. Expected format: PRO-DOC-1 or DOC-1"
        ))),
    }
}

fn validate_page_folder(
    conn: &Connection,
    project_id: Option<i64>,
    folder_id: Option<i64>,
) -> Result<(), LificError> {
    let Some(folder_id) = folder_id else {
        return Ok(());
    };
    let Some(project_id) = project_id else {
        return Err(LificError::BadRequest(
            "workspace pages cannot have a folder".into(),
        ));
    };
    let folder_project_id: Option<i64> = conn
        .query_row(
            "SELECT project_id FROM folders WHERE id = ?1",
            params![folder_id],
            |row| row.get(0),
        )
        .optional()?;
    match folder_project_id {
        Some(folder_project_id) if folder_project_id == project_id => Ok(()),
        Some(folder_project_id) => Err(LificError::BadRequest(format!(
            "folder {folder_id} belongs to project {folder_project_id}, not page project {project_id}"
        ))),
        None => Err(LificError::BadRequest(format!(
            "folder {folder_id} not found"
        ))),
    }
}

pub fn create_page(conn: &Connection, input: &CreatePage) -> Result<Page, LificError> {
    validate_page_folder(conn, input.project_id, input.folder_id)?;
    // Counts tombstones on purpose (LIF-438): a soft-deleted page still owns
    // its `PROJ-DOC-n` identifier and would collide on restore if reissued.
    let next_seq: i64 = if let Some(pid) = input.project_id {
        conn.query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM pages WHERE project_id = ?1",
            params![pid],
            |row| row.get(0),
        )
        .unwrap_or(1)
    } else {
        conn.query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM pages WHERE project_id IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap_or(1)
    };
    // The page id is captured inside the savepoint closure —
    // `conn.last_insert_rowid()` after the savepoint releases would reflect
    // the most recent INSERT, which may be a page_labels row, not the page
    // itself. `create_issue` uses the same shape (LIF-130), including
    // LIF-409's hydrating read inside the savepoint.
    let content = unescape_text(&input.content);
    super::savepoint(conn, "create_page", || {
        conn.execute(
            "INSERT INTO pages (project_id, folder_id, title, content, sequence, status) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![input.project_id, input.folder_id, input.title, content, next_seq, input.status],
        )?;
        let id = conn.last_insert_rowid();

        // Labels are project-scoped, so workspace pages (no project_id) can't
        // carry them. Silently skip rather than erroring — keeps create
        // forgiving for clients that always send `labels: []`.
        if let Some(pid) = input.project_id {
            for label_name in &input.labels {
                conn.execute(
                    "INSERT OR IGNORE INTO page_labels (page_id, label_id)
                     SELECT ?1, l.id FROM labels l
                     WHERE l.project_id = ?2 AND l.name = ?3",
                    params![id, pid, label_name],
                )?;
            }
        }
        // LIF-409: see `create_issue` — link reconciliation belongs to the
        // write that stored the content, not to each transport in turn.
        super::attachments::sync_links(
            conn,
            AttachmentEntity::Page,
            id,
            &content,
            input.attachments,
            input.project_id,
        )?;
        get_page(conn, id)
    })
}

pub fn update_page(conn: &Connection, id: i64, input: &UpdatePage) -> Result<Page, LificError> {
    let page = get_page(conn, id)?;
    if let Some(folder_id) = input.folder_id {
        validate_page_folder(conn, page.project_id, folder_id)?;
    }
    super::savepoint::<_, Page>(conn, "update_page", || {
        // LIF-441: see `update_issue` — checked inside the savepoint so the
        // precondition and the write cannot be separated.
        if let Some(expected) = input.expected_seq {
            let current = get_page(conn, id)?;
            if current.seq != expected {
                return Err(LificError::update_conflict(
                    &current.identifier,
                    expected,
                    current.seq,
                    &current,
                ));
            }
        }
        if let Some(ref title) = input.title {
            conn.execute(
                "UPDATE pages SET title = ?1 WHERE id = ?2",
                params![title, id],
            )?;
        }
        if let Some(ref content) = input.content {
            conn.execute(
                "UPDATE pages SET content = ?1 WHERE id = ?2",
                params![unescape_text(content), id],
            )?;
        }
        if let Some(ref folder_id) = input.folder_id {
            conn.execute(
                "UPDATE pages SET folder_id = ?1 WHERE id = ?2",
                params![folder_id, id],
            )?;
        }
        if let Some(sort_order) = input.sort_order {
            conn.execute(
                "UPDATE pages SET sort_order = ?1 WHERE id = ?2",
                params![sort_order, id],
            )?;
        }
        if let Some(ref status) = input.status {
            conn.execute(
                "UPDATE pages SET status = ?1 WHERE id = ?2",
                params![status, id],
            )?;
        }
        if let Some(pinned) = input.pinned {
            conn.execute(
                "UPDATE pages SET pinned = ?1 WHERE id = ?2",
                params![pinned, id],
            )?;
        }
        if let Some(ref labels) = input.labels {
            // Mirror `update_issue`: delete-all + insert-by-name. Labels
            // are project-scoped, so for workspace pages we silently
            // clear (the DELETE always runs) and skip the inserts.
            conn.execute("DELETE FROM page_labels WHERE page_id = ?1", params![id])?;
            let project_id: Option<i64> = conn.query_row(
                "SELECT project_id FROM pages WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )?;
            if let Some(pid) = project_id {
                for label_name in labels {
                    conn.execute(
                        "INSERT OR IGNORE INTO page_labels (page_id, label_id)
                         SELECT ?1, l.id FROM labels l
                         WHERE l.project_id = ?2 AND l.name = ?3",
                        params![id, pid, label_name],
                    )?;
                }
            }
        }
        // LIF-409: re-scan the stored content and reconcile links inside the
        // same savepoint. Read back from the row so an update that leaves the
        // content alone still reconciles against what is actually stored.
        if !matches!(input.attachments, AttachmentActor::Unattributed) {
            let (project_id, content): (Option<i64>, String) = conn.query_row(
                "SELECT project_id, content FROM pages WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            super::attachments::sync_links(
                conn,
                AttachmentEntity::Page,
                id,
                &content,
                input.attachments,
                project_id,
            )?;
        }
        // LIF-409: see `update_issue` — a failed final read must roll the
        // update back, not report an error over a committed change.
        get_page(conn, id)
    })
}

/// Tombstone a page (LIF-438). See [`super::delete_issue`] for the rationale;
/// migration 047's cascade tombstones the page's live comments in the same
/// statement, carrying the page's exact `deleted_at`.
pub fn delete_page(conn: &Connection, id: i64) -> Result<(), LificError> {
    let changed = conn.execute(
        &format!(
            "UPDATE pages SET deleted_at = {TOMBSTONE_NOW} \
             WHERE id = ?1 AND deleted_at IS NULL"
        ),
        params![id],
    )?;
    if changed == 0 {
        return Err(LificError::NotFound(format!("page {id} not found")));
    }
    Ok(())
}

/// A page's current `seq`, tombstone or not (LIF-440). The delete-path
/// counterpart of [`super::issues::issue_seq`]; see that function for why the
/// caller's pre-delete copy of the row is the wrong seq to publish.
pub fn page_seq(conn: &Connection, id: i64) -> Result<i64, LificError> {
    conn.query_row("SELECT seq FROM pages WHERE id = ?1", [id], |row| {
        row.get(0)
    })
    .map_err(|error| match error {
        rusqlite::Error::QueryReturnedNoRows => {
            LificError::NotFound(format!("page {id} not found"))
        }
        other => other.into(),
    })
}

/// Bring a tombstoned page back, together with the comments that went down
/// with it (LIF-438).
pub fn restore_page(conn: &Connection, id: i64) -> Result<Page, LificError> {
    let changed = conn.execute(
        "UPDATE pages SET deleted_at = NULL
          WHERE id = ?1 AND deleted_at IS NOT NULL",
        params![id],
    )?;
    if changed == 0 {
        return Err(LificError::NotFound(format!("deleted page {id} not found")));
    }
    get_page(conn, id)
}

/// The project a tombstoned page belongs to (`None` for a workspace page).
///
/// Restore authorizes against this before the page is visible to
/// [`get_page`], so it deliberately looks past the tombstone filter.
pub fn deleted_page_project_id(conn: &Connection, id: i64) -> Result<Option<i64>, LificError> {
    conn.query_row(
        "SELECT project_id FROM pages WHERE id = ?1 AND deleted_at IS NOT NULL",
        [id],
        |row| row.get(0),
    )
    .map_err(|error| match error {
        rusqlite::Error::QueryReturnedNoRows => {
            LificError::NotFound(format!("deleted page {id} not found"))
        }
        other => other.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::queries::{projects, resources};

    fn test_db() -> db::DbPool {
        db::open_memory().expect("test db")
    }

    fn seed_project(conn: &rusqlite::Connection, ident: &str) -> i64 {
        projects::create_project(
            conn,
            &CreateProject {
                name: format!("Project {ident}"),
                identifier: ident.into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id
    }

    fn seed_label(conn: &rusqlite::Connection, project_id: i64, name: &str) -> i64 {
        resources::create_label(
            conn,
            &CreateLabel {
                project_id,
                name: name.into(),
                color: "#22C55E".into(),
            },
        )
        .unwrap()
        .id
    }

    fn seed_folder(conn: &rusqlite::Connection, project_id: i64, name: &str) -> i64 {
        resources::create_folder(
            conn,
            &CreateFolder {
                project_id,
                parent_id: None,
                name: name.into(),
            },
        )
        .unwrap()
        .id
    }

    /// Convenience builder so tests with no label requirements stay terse.
    fn blank_page(project_id: Option<i64>, title: &str) -> CreatePage {
        CreatePage {
            project_id,
            title: title.into(),
            status: "draft".into(),
            ..Default::default()
        }
    }

    #[test]
    fn create_page_auto_sequences() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");

        let p1 = create_page(&conn, &blank_page(Some(pid), "First")).unwrap();
        let p2 = create_page(&conn, &blank_page(Some(pid), "Second")).unwrap();

        assert_eq!(p1.sequence, Some(1));
        assert_eq!(p2.sequence, Some(2));
    }

    #[test]
    fn page_identifier_format() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "LIF");

        let page = create_page(&conn, &blank_page(Some(pid), "Arch Doc")).unwrap();
        assert_eq!(page.identifier, "LIF-DOC-1");
    }

    #[test]
    fn workspace_page_identifier() {
        let pool = test_db();
        let conn = pool.write().unwrap();

        let page = create_page(&conn, &blank_page(None, "Global doc")).unwrap();
        assert_eq!(page.identifier, "DOC-1");
    }

    #[test]
    fn resolve_page_identifier_project() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "PRO");
        let page = create_page(&conn, &blank_page(Some(pid), "Design")).unwrap();

        let id = resolve_page_identifier(&conn, "PRO-DOC-1").unwrap();
        assert_eq!(id, page.id);
    }

    #[test]
    fn resolve_page_identifier_workspace() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let page = create_page(&conn, &blank_page(None, "Global")).unwrap();

        let id = resolve_page_identifier(&conn, "DOC-1").unwrap();
        assert_eq!(id, page.id);
    }

    #[test]
    fn resolve_page_identifier_rejects_garbage() {
        let pool = test_db();
        let conn = pool.read().unwrap();
        assert!(resolve_page_identifier(&conn, "garbage").is_err());
        assert!(resolve_page_identifier(&conn, "LIF-DOC-abc").is_err());
    }

    #[test]
    fn page_crud() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");

        let page = create_page(
            &conn,
            &CreatePage {
                project_id: Some(pid),
                title: "Original".into(),
                content: "# Hello".into(),
                status: "draft".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(page.title, "Original");
        assert_eq!(page.content, "# Hello");

        let updated = update_page(
            &conn,
            page.id,
            &UpdatePage {
                title: Some("Renamed".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(updated.title, "Renamed");
        assert_eq!(updated.content, "# Hello"); // unchanged

        delete_page(&conn, page.id).unwrap();
        assert!(get_page(&conn, page.id).is_err());
    }

    #[test]
    fn page_unescape_content() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");

        let page = create_page(
            &conn,
            &CreatePage {
                project_id: Some(pid),
                title: "Escaped".into(),
                content: "# Title\\n\\nParagraph".into(),
                status: "draft".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(page.content, "# Title\n\nParagraph");
    }

    // ── LIF-311: page folder project scope ────────────────────

    #[test]
    fn create_page_rejects_folder_from_another_project() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let page_project_id = seed_project(&conn, "ONE");
        let folder_project_id = seed_project(&conn, "TWO");
        let folder_id = seed_folder(&conn, folder_project_id, "Other project");

        let result = create_page(
            &conn,
            &CreatePage {
                project_id: Some(page_project_id),
                folder_id: Some(folder_id),
                title: "Invalid folder".into(),
                status: "draft".into(),
                ..Default::default()
            },
        );

        assert!(matches!(
            result,
            Err(LificError::BadRequest(message))
                if message == format!(
                    "folder {folder_id} belongs to project {folder_project_id}, not page project {page_project_id}"
                )
        ));
    }

    #[test]
    fn update_page_rejects_folder_from_another_project() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let page_project_id = seed_project(&conn, "ONE");
        let folder_project_id = seed_project(&conn, "TWO");
        let folder_id = seed_folder(&conn, folder_project_id, "Other project");
        let page = create_page(&conn, &blank_page(Some(page_project_id), "Page")).unwrap();

        let result = update_page(
            &conn,
            page.id,
            &UpdatePage {
                folder_id: Some(Some(folder_id)),
                ..Default::default()
            },
        );

        assert!(matches!(
            result,
            Err(LificError::BadRequest(message))
                if message == format!(
                    "folder {folder_id} belongs to project {folder_project_id}, not page project {page_project_id}"
                )
        ));
        assert_eq!(get_page(&conn, page.id).unwrap().folder_id, None);
    }

    #[test]
    fn workspace_pages_reject_non_null_folders() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project_id = seed_project(&conn, "TST");
        let folder_id = seed_folder(&conn, project_id, "Project folder");

        let create_result = create_page(
            &conn,
            &CreatePage {
                folder_id: Some(folder_id),
                title: "Workspace page".into(),
                status: "draft".into(),
                ..Default::default()
            },
        );
        assert!(matches!(
            create_result,
            Err(LificError::BadRequest(message)) if message == "workspace pages cannot have a folder"
        ));

        let page = create_page(&conn, &blank_page(None, "Workspace page")).unwrap();
        let update_result = update_page(
            &conn,
            page.id,
            &UpdatePage {
                folder_id: Some(Some(folder_id)),
                ..Default::default()
            },
        );
        assert!(matches!(
            update_result,
            Err(LificError::BadRequest(message)) if message == "workspace pages cannot have a folder"
        ));
    }

    #[test]
    fn update_page_allows_clearing_folder() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project_id = seed_project(&conn, "TST");
        let folder_id = seed_folder(&conn, project_id, "Folder");
        let page = create_page(
            &conn,
            &CreatePage {
                project_id: Some(project_id),
                folder_id: Some(folder_id),
                title: "Page".into(),
                status: "draft".into(),
                ..Default::default()
            },
        )
        .unwrap();

        let updated = update_page(
            &conn,
            page.id,
            &UpdatePage {
                folder_id: Some(None),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(updated.folder_id, None);
    }

    // ── LIF-105: page labels ─────────────────────────────────

    #[test]
    fn create_page_with_labels_attaches_them() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        seed_label(&conn, pid, "design");
        seed_label(&conn, pid, "draft");

        let page = create_page(
            &conn,
            &CreatePage {
                project_id: Some(pid),
                title: "Spec".into(),
                status: "draft".into(),
                labels: vec!["design".into(), "draft".into()],
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(page.labels.len(), 2);
        assert!(page.labels.contains(&"design".to_string()));
        assert!(page.labels.contains(&"draft".to_string()));
    }

    #[test]
    fn create_page_silently_skips_unknown_labels() {
        // Mirrors `create_issue` — `INSERT OR IGNORE` on a SELECT that
        // returns no rows is a no-op. Caller gets the page back with
        // labels reflecting only what actually existed.
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        seed_label(&conn, pid, "design");

        let page = create_page(
            &conn,
            &CreatePage {
                project_id: Some(pid),
                title: "Spec".into(),
                status: "draft".into(),
                labels: vec!["design".into(), "phantom".into()],
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(page.labels, vec!["design".to_string()]);
    }

    #[test]
    fn update_page_replaces_labels() {
        // Replace semantics, not additive — same pattern as `update_issue`.
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        seed_label(&conn, pid, "old");
        seed_label(&conn, pid, "new");

        let page = create_page(
            &conn,
            &CreatePage {
                project_id: Some(pid),
                title: "P".into(),
                status: "draft".into(),
                labels: vec!["old".into()],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(page.labels, vec!["old".to_string()]);

        let updated = update_page(
            &conn,
            page.id,
            &UpdatePage {
                labels: Some(vec!["new".into()]),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(updated.labels, vec!["new".to_string()]);
    }

    #[test]
    fn update_page_with_empty_labels_clears_all() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        seed_label(&conn, pid, "x");

        let page = create_page(
            &conn,
            &CreatePage {
                project_id: Some(pid),
                title: "P".into(),
                status: "draft".into(),
                labels: vec!["x".into()],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(page.labels.len(), 1);

        let cleared = update_page(
            &conn,
            page.id,
            &UpdatePage {
                labels: Some(vec![]),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(cleared.labels.is_empty());
    }

    #[test]
    fn update_page_without_labels_field_leaves_them_alone() {
        // Partial-update semantics: omitting `labels` must not delete
        // existing rows. Critical for clients that PATCH a single field.
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        seed_label(&conn, pid, "keep");

        let page = create_page(
            &conn,
            &CreatePage {
                project_id: Some(pid),
                title: "P".into(),
                status: "draft".into(),
                labels: vec!["keep".into()],
                ..Default::default()
            },
        )
        .unwrap();

        let updated = update_page(
            &conn,
            page.id,
            &UpdatePage {
                title: Some("Renamed".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(updated.title, "Renamed");
        assert_eq!(updated.labels, vec!["keep".to_string()]);
    }

    #[test]
    fn workspace_page_create_ignores_labels() {
        // No project = no scope for project-scoped labels. Silently drop
        // them rather than erroring — matches the "defer workspace pages"
        // decision in LIF-105.
        let pool = test_db();
        let conn = pool.write().unwrap();

        let page = create_page(
            &conn,
            &CreatePage {
                title: "Workspace doc".into(),
                status: "draft".into(),
                // These labels can't exist in any project scope here, so
                // even if we tried to attach them the lookup would miss.
                labels: vec!["anything".into()],
                ..Default::default()
            },
        )
        .unwrap();
        assert!(page.labels.is_empty());
    }

    #[test]
    fn list_pages_populates_labels() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        seed_label(&conn, pid, "a");
        seed_label(&conn, pid, "b");

        create_page(
            &conn,
            &CreatePage {
                project_id: Some(pid),
                title: "One".into(),
                status: "draft".into(),
                labels: vec!["a".into()],
                ..Default::default()
            },
        )
        .unwrap();
        create_page(
            &conn,
            &CreatePage {
                project_id: Some(pid),
                title: "Two".into(),
                status: "draft".into(),
                labels: vec!["a".into(), "b".into()],
                ..Default::default()
            },
        )
        .unwrap();

        let pages = list_pages(&conn, Some(pid), None, None, None, None, None, None, None).unwrap();
        let by_title: std::collections::HashMap<_, _> =
            pages.into_iter().map(|p| (p.title.clone(), p)).collect();
        assert_eq!(by_title["One"].labels, vec!["a".to_string()]);
        assert_eq!(
            by_title["Two"].labels,
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn list_pages_filter_by_label() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        seed_label(&conn, pid, "design");

        create_page(
            &conn,
            &CreatePage {
                project_id: Some(pid),
                title: "Designy".into(),
                status: "draft".into(),
                labels: vec!["design".into()],
                ..Default::default()
            },
        )
        .unwrap();
        create_page(&conn, &blank_page(Some(pid), "Plain")).unwrap();

        let filtered = list_pages(
            &conn,
            Some(pid),
            None,
            Some("design"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].title, "Designy");
    }

    #[test]
    fn deleting_label_cascades_to_page_labels() {
        // FK CASCADE means removing a label drops every join row,
        // and subsequent reads simply don't list it.
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let label_id = seed_label(&conn, pid, "doomed");

        let page = create_page(
            &conn,
            &CreatePage {
                project_id: Some(pid),
                title: "P".into(),
                status: "draft".into(),
                labels: vec!["doomed".into()],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(page.labels.len(), 1);

        resources::delete_label(&conn, label_id).unwrap();
        let after = get_page(&conn, page.id).unwrap();
        assert!(after.labels.is_empty());
    }

    #[test]
    fn deleting_page_cascades_to_page_labels() {
        // Confirm the inverse direction works too — orphaned join rows
        // would cause an integrity drift if the FK weren't honored.
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        seed_label(&conn, pid, "x");

        let page = create_page(
            &conn,
            &CreatePage {
                project_id: Some(pid),
                title: "P".into(),
                status: "draft".into(),
                labels: vec!["x".into()],
                ..Default::default()
            },
        )
        .unwrap();

        let label_rows = |conn: &rusqlite::Connection| -> i64 {
            conn.query_row(
                "SELECT COUNT(*) FROM page_labels WHERE page_id = ?1",
                params![page.id],
                |row| row.get(0),
            )
            .unwrap()
        };

        // LIF-438: a soft delete keeps the join rows, so a restore brings the
        // page back with its labels intact. The FK cascade below is what the
        // retention purge eventually fires.
        delete_page(&conn, page.id).unwrap();
        assert_eq!(label_rows(&conn), 1);

        conn.execute("DELETE FROM pages WHERE id = ?1", params![page.id])
            .unwrap();
        assert_eq!(label_rows(&conn), 0);
    }

    // ── LIF-112: page status (lifecycle) ─────────────────────

    #[test]
    fn create_page_defaults_to_draft_status() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");

        let page = create_page(&conn, &blank_page(Some(pid), "Fresh")).unwrap();
        assert_eq!(page.status, "draft");
    }

    #[test]
    fn create_page_honors_explicit_status() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");

        let page = create_page(
            &conn,
            &CreatePage {
                project_id: Some(pid),
                title: "Active doc".into(),
                status: "active".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(page.status, "active");
    }

    #[test]
    fn update_page_transitions_status() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");

        let page = create_page(&conn, &blank_page(Some(pid), "P")).unwrap();
        assert_eq!(page.status, "draft");

        let updated = update_page(
            &conn,
            page.id,
            &UpdatePage {
                status: Some("complete".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(updated.status, "complete");
    }

    #[test]
    fn update_page_without_status_leaves_it_alone() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");

        let page = create_page(
            &conn,
            &CreatePage {
                project_id: Some(pid),
                title: "P".into(),
                status: "active".into(),
                ..Default::default()
            },
        )
        .unwrap();

        let updated = update_page(
            &conn,
            page.id,
            &UpdatePage {
                title: Some("Renamed".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(updated.status, "active");
    }

    #[test]
    fn list_pages_filters_by_status() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");

        create_page(&conn, &blank_page(Some(pid), "Drafty")).unwrap();
        create_page(
            &conn,
            &CreatePage {
                project_id: Some(pid),
                title: "Archived doc".into(),
                status: "archived".into(),
                ..Default::default()
            },
        )
        .unwrap();

        let archived = list_pages(
            &conn,
            Some(pid),
            None,
            None,
            Some("archived"),
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].title, "Archived doc");

        let drafts = list_pages(
            &conn,
            Some(pid),
            None,
            None,
            Some("draft"),
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].title, "Drafty");
    }

    // ── Sort control (order_by / order) ───────────────────────

    /// Pin a page's timestamps so ordering tests don't race the clock.
    /// The `pages_updated` trigger rewrites updated_at to now on every
    /// UPDATE, which would silently overwrite the pin — drop it first.
    fn pin_page_timestamps(
        conn: &rusqlite::Connection,
        page_id: i64,
        created: &str,
        updated: &str,
    ) {
        conn.execute_batch("DROP TRIGGER IF EXISTS pages_updated;")
            .unwrap();
        conn.execute(
            "UPDATE pages SET created_at = ?1, updated_at = ?2 WHERE id = ?3",
            params![created, updated, page_id],
        )
        .unwrap();
    }

    #[test]
    fn list_pages_orders_by_title() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        create_page(&conn, &blank_page(Some(pid), "banana")).unwrap();
        create_page(&conn, &blank_page(Some(pid), "Apple")).unwrap();
        create_page(&conn, &blank_page(Some(pid), "cherry")).unwrap();

        // COLLATE NOCASE: "Apple" sorts before "banana" despite case.
        let asc = list_pages(
            &conn,
            Some(pid),
            None,
            None,
            None,
            Some("title"),
            None,
            None,
            None,
        )
        .unwrap();
        let titles: Vec<&str> = asc.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(titles, vec!["Apple", "banana", "cherry"]);

        let desc = list_pages(
            &conn,
            Some(pid),
            None,
            None,
            None,
            Some("title"),
            Some("desc"),
            None,
            None,
        )
        .unwrap();
        let titles: Vec<&str> = desc.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(titles, vec!["cherry", "banana", "Apple"]);
    }

    #[test]
    fn list_pages_orders_by_updated_desc() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let stale = create_page(&conn, &blank_page(Some(pid), "Stale")).unwrap();
        let fresh = create_page(&conn, &blank_page(Some(pid), "Fresh")).unwrap();
        pin_page_timestamps(
            &conn,
            stale.id,
            "2026-01-01 00:00:00",
            "2026-01-02 00:00:00",
        );
        pin_page_timestamps(
            &conn,
            fresh.id,
            "2026-01-01 00:00:00",
            "2026-06-01 00:00:00",
        );

        let pages = list_pages(
            &conn,
            Some(pid),
            None,
            None,
            None,
            Some("updated"),
            Some("desc"),
            None,
            None,
        )
        .unwrap();
        let titles: Vec<&str> = pages.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(titles, vec!["Fresh", "Stale"]);
    }

    #[test]
    fn list_pages_rejects_invalid_order_params() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        create_page(&conn, &blank_page(Some(pid), "P")).unwrap();

        assert!(
            list_pages(
                &conn,
                Some(pid),
                None,
                None,
                None,
                Some("content; --"),
                None,
                None,
                None
            )
            .is_err(),
            "unknown order_by must error, not be interpolated"
        );
        assert!(
            list_pages(
                &conn,
                Some(pid),
                None,
                None,
                None,
                None,
                Some("up"),
                None,
                None
            )
            .is_err()
        );
    }

    // ── LIF-183: page pinning ────────────────────────────────

    #[test]
    fn create_page_defaults_to_unpinned() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let page = create_page(&conn, &blank_page(Some(pid), "P")).unwrap();
        assert!(!page.pinned);
    }

    #[test]
    fn update_page_pins_and_unpins() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let page = create_page(&conn, &blank_page(Some(pid), "P")).unwrap();

        let pinned = update_page(
            &conn,
            page.id,
            &UpdatePage {
                pinned: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(pinned.pinned);

        let unpinned = update_page(
            &conn,
            page.id,
            &UpdatePage {
                pinned: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!unpinned.pinned);
    }

    #[test]
    fn update_page_without_pinned_leaves_it_alone() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let page = create_page(&conn, &blank_page(Some(pid), "P")).unwrap();
        update_page(
            &conn,
            page.id,
            &UpdatePage {
                pinned: Some(true),
                ..Default::default()
            },
        )
        .unwrap();

        // A title-only patch must not disturb the pin.
        let renamed = update_page(
            &conn,
            page.id,
            &UpdatePage {
                title: Some("Renamed".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(renamed.pinned);
    }

    #[test]
    fn list_pages_reflects_pinned() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let p = create_page(&conn, &blank_page(Some(pid), "P")).unwrap();
        update_page(
            &conn,
            p.id,
            &UpdatePage {
                pinned: Some(true),
                ..Default::default()
            },
        )
        .unwrap();

        let pages = list_pages(&conn, Some(pid), None, None, None, None, None, None, None).unwrap();
        assert_eq!(pages.len(), 1);
        assert!(pages[0].pinned);
    }

    #[test]
    fn invalid_status_rejected_by_check_constraint() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");

        let result = create_page(
            &conn,
            &CreatePage {
                project_id: Some(pid),
                title: "Bad".into(),
                status: "bogus".into(),
                ..Default::default()
            },
        );
        assert!(result.is_err());
    }
}
