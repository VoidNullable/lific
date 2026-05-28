use rusqlite::{params, Connection};

use crate::db::models::*;
use crate::error::LificError;

use super::unescape_text;

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
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        labels: Vec::new(),
    })
}

const PAGE_SELECT: &str = "SELECT pg.id, pg.project_id, pg.sequence, p.identifier,
            pg.folder_id, pg.title, pg.content, pg.sort_order,
            pg.created_at, pg.updated_at
     FROM pages pg
     LEFT JOIN projects p ON p.id = pg.project_id";

/// Look up label names attached to a single page. Used by `get_page` to
/// populate `Page.labels`. Returns empty for pages with no labels (or for
/// workspace pages, which can't carry labels yet — LIF-105).
fn page_labels(conn: &Connection, page_id: i64) -> Result<Vec<String>, LificError> {
    let mut stmt = conn.prepare(
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
    let params_refs: Vec<&dyn rusqlite::types::ToSql> =
        params.iter().map(|p| p.as_ref()).collect();
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

pub fn list_pages(
    conn: &Connection,
    project_id: Option<i64>,
    folder_id: Option<i64>,
    label: Option<&str>,
) -> Result<Vec<Page>, LificError> {
    // Build the query incrementally so the optional label filter can
    // graft on a JOIN. Using DISTINCT shields against a page joining
    // multiple labels and double-appearing if the filter weren't there
    // (it should be impossible since label is filtered by name, but
    // DISTINCT keeps the query robust to future joins).
    let mut sql = String::from(
        "SELECT DISTINCT pg.id, pg.project_id, pg.sequence, p.identifier,
                pg.folder_id, pg.title, pg.content, pg.sort_order,
                pg.created_at, pg.updated_at
         FROM pages pg
         LEFT JOIN projects p ON p.id = pg.project_id",
    );
    let mut conditions: Vec<String> = Vec::new();
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

    if !conditions.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conditions.join(" AND "));
    }
    sql.push_str(" ORDER BY pg.sort_order");

    let params_refs: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_refs.as_slice(), page_from_row)?;
    let mut pages: Vec<Page> = rows.collect::<Result<Vec<_>, _>>()?;
    populate_page_labels(conn, &mut pages)?;
    Ok(pages)
}

pub fn get_page(conn: &Connection, id: i64) -> Result<Page, LificError> {
    let mut page = conn
        .query_row(
            &format!("{PAGE_SELECT} WHERE pg.id = ?1"),
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

pub fn resolve_page_identifier(conn: &Connection, identifier: &str) -> Result<i64, LificError> {
    let parts: Vec<&str> = identifier.split('-').collect();
    match parts.as_slice() {
        [project_ident, "DOC", seq_str] => {
            let sequence: i64 = seq_str.parse().map_err(|_| {
                LificError::BadRequest(format!("invalid page identifier: {identifier}"))
            })?;
            conn.query_row(
                "SELECT pg.id FROM pages pg JOIN projects p ON p.id = pg.project_id WHERE p.identifier = ?1 AND pg.sequence = ?2",
                params![project_ident, sequence], |row| row.get(0),
            ).map_err(|e| match e { rusqlite::Error::QueryReturnedNoRows => LificError::NotFound(format!("page {identifier} not found")), _ => e.into() })
        }
        ["DOC", seq_str] => {
            let sequence: i64 = seq_str.parse().map_err(|_| {
                LificError::BadRequest(format!("invalid page identifier: {identifier}"))
            })?;
            conn.query_row(
                "SELECT id FROM pages WHERE project_id IS NULL AND sequence = ?1",
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

pub fn create_page(conn: &Connection, input: &CreatePage) -> Result<Page, LificError> {
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
    // Capture the page id from inside the savepoint closure and propagate
    // it out — `conn.last_insert_rowid()` after the savepoint releases will
    // reflect the most recent INSERT, which may be a page_labels row, not
    // the page itself. Mirrors the same shape `create_issue` would need
    // when wrapped (it currently isn't — see how it captures `id` once
    // before the label inserts).
    let id = super::savepoint(conn, "create_page", || {
        conn.execute(
            "INSERT INTO pages (project_id, folder_id, title, content, sequence) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![input.project_id, input.folder_id, input.title, unescape_text(&input.content), next_seq],
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
        Ok(id)
    })?;
    get_page(conn, id)
}

pub fn update_page(conn: &Connection, id: i64, input: &UpdatePage) -> Result<Page, LificError> {
    get_page(conn, id)?;
    super::savepoint(conn, "update_page", || {
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
        Ok(())
    })?;
    get_page(conn, id)
}

pub fn delete_page(conn: &Connection, id: i64) -> Result<(), LificError> {
    let changed = conn.execute("DELETE FROM pages WHERE id = ?1", params![id])?;
    if changed == 0 {
        return Err(LificError::NotFound(format!("page {id} not found")));
    }
    Ok(())
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
                description: String::new(),
                emoji: None,
                lead_user_id: None,
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

    /// Convenience builder so tests with no label requirements stay terse.
    fn blank_page(project_id: Option<i64>, title: &str) -> CreatePage {
        CreatePage {
            project_id,
            folder_id: None,
            title: title.into(),
            content: String::new(),
            labels: vec![],
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
                folder_id: None,
                title: "Original".into(),
                content: "# Hello".into(),
                labels: vec![],
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
                content: None,
                folder_id: None,
                sort_order: None,
                labels: None,
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
                folder_id: None,
                title: "Escaped".into(),
                content: "# Title\\n\\nParagraph".into(),
                labels: vec![],
            },
        )
        .unwrap();
        assert_eq!(page.content, "# Title\n\nParagraph");
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
                folder_id: None,
                title: "Spec".into(),
                content: String::new(),
                labels: vec!["design".into(), "draft".into()],
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
                folder_id: None,
                title: "Spec".into(),
                content: String::new(),
                labels: vec!["design".into(), "phantom".into()],
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
                folder_id: None,
                title: "P".into(),
                content: String::new(),
                labels: vec!["old".into()],
            },
        )
        .unwrap();
        assert_eq!(page.labels, vec!["old".to_string()]);

        let updated = update_page(
            &conn,
            page.id,
            &UpdatePage {
                title: None,
                content: None,
                folder_id: None,
                sort_order: None,
                labels: Some(vec!["new".into()]),
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
                folder_id: None,
                title: "P".into(),
                content: String::new(),
                labels: vec!["x".into()],
            },
        )
        .unwrap();
        assert_eq!(page.labels.len(), 1);

        let cleared = update_page(
            &conn,
            page.id,
            &UpdatePage {
                title: None,
                content: None,
                folder_id: None,
                sort_order: None,
                labels: Some(vec![]),
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
                folder_id: None,
                title: "P".into(),
                content: String::new(),
                labels: vec!["keep".into()],
            },
        )
        .unwrap();

        let updated = update_page(
            &conn,
            page.id,
            &UpdatePage {
                title: Some("Renamed".into()),
                content: None,
                folder_id: None,
                sort_order: None,
                labels: None,
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
                project_id: None,
                folder_id: None,
                title: "Workspace doc".into(),
                content: String::new(),
                // These labels can't exist in any project scope here, so
                // even if we tried to attach them the lookup would miss.
                labels: vec!["anything".into()],
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
                folder_id: None,
                title: "One".into(),
                content: String::new(),
                labels: vec!["a".into()],
            },
        )
        .unwrap();
        create_page(
            &conn,
            &CreatePage {
                project_id: Some(pid),
                folder_id: None,
                title: "Two".into(),
                content: String::new(),
                labels: vec!["a".into(), "b".into()],
            },
        )
        .unwrap();

        let pages = list_pages(&conn, Some(pid), None, None).unwrap();
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
                folder_id: None,
                title: "Designy".into(),
                content: String::new(),
                labels: vec!["design".into()],
            },
        )
        .unwrap();
        create_page(&conn, &blank_page(Some(pid), "Plain")).unwrap();

        let filtered = list_pages(&conn, Some(pid), None, Some("design")).unwrap();
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
                folder_id: None,
                title: "P".into(),
                content: String::new(),
                labels: vec!["doomed".into()],
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
                folder_id: None,
                title: "P".into(),
                content: String::new(),
                labels: vec!["x".into()],
            },
        )
        .unwrap();

        delete_page(&conn, page.id).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM page_labels WHERE page_id = ?1",
                params![page.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
}
