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
    })
}

const PAGE_SELECT: &str = "SELECT pg.id, pg.project_id, pg.sequence, p.identifier,
            pg.folder_id, pg.title, pg.content, pg.sort_order,
            pg.created_at, pg.updated_at
     FROM pages pg
     LEFT JOIN projects p ON p.id = pg.project_id";

pub fn list_pages(
    conn: &Connection,
    project_id: Option<i64>,
    folder_id: Option<i64>,
) -> Result<Vec<Page>, LificError> {
    let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match (
        project_id, folder_id,
    ) {
        (Some(pid), Some(fid)) => (
            format!(
                "{PAGE_SELECT} WHERE pg.project_id = ?1 AND pg.folder_id = ?2 ORDER BY pg.sort_order"
            ),
            vec![
                Box::new(pid) as Box<dyn rusqlite::types::ToSql>,
                Box::new(fid),
            ],
        ),
        (Some(pid), None) => (
            format!("{PAGE_SELECT} WHERE pg.project_id = ?1 ORDER BY pg.sort_order"),
            vec![Box::new(pid) as Box<dyn rusqlite::types::ToSql>],
        ),
        (None, _) => (
            format!("{PAGE_SELECT} WHERE pg.project_id IS NULL ORDER BY pg.sort_order"),
            vec![],
        ),
    };
    let params_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_refs.as_slice(), page_from_row)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn get_page(conn: &Connection, id: i64) -> Result<Page, LificError> {
    conn.query_row(
        &format!("{PAGE_SELECT} WHERE pg.id = ?1"),
        params![id],
        page_from_row,
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            LificError::NotFound(format!("page {id} not found"))
        }
        _ => e.into(),
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
    conn.execute(
        "INSERT INTO pages (project_id, folder_id, title, content, sequence) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![input.project_id, input.folder_id, input.title, unescape_text(&input.content), next_seq],
    )?;
    get_page(conn, conn.last_insert_rowid())
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
    use crate::db::queries::projects;

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

    #[test]
    fn create_page_auto_sequences() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");

        let p1 = create_page(
            &conn,
            &CreatePage {
                project_id: Some(pid),
                folder_id: None,
                title: "First".into(),
                content: String::new(),
            },
        )
        .unwrap();
        let p2 = create_page(
            &conn,
            &CreatePage {
                project_id: Some(pid),
                folder_id: None,
                title: "Second".into(),
                content: String::new(),
            },
        )
        .unwrap();

        assert_eq!(p1.sequence, Some(1));
        assert_eq!(p2.sequence, Some(2));
    }

    #[test]
    fn page_identifier_format() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "LIF");

        let page = create_page(
            &conn,
            &CreatePage {
                project_id: Some(pid),
                folder_id: None,
                title: "Arch Doc".into(),
                content: String::new(),
            },
        )
        .unwrap();
        assert_eq!(page.identifier, "LIF-DOC-1");
    }

    #[test]
    fn workspace_page_identifier() {
        let pool = test_db();
        let conn = pool.write().unwrap();

        let page = create_page(
            &conn,
            &CreatePage {
                project_id: None,
                folder_id: None,
                title: "Global doc".into(),
                content: String::new(),
            },
        )
        .unwrap();
        assert_eq!(page.identifier, "DOC-1");
    }

    #[test]
    fn resolve_page_identifier_project() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "PRO");
        let page = create_page(
            &conn,
            &CreatePage {
                project_id: Some(pid),
                folder_id: None,
                title: "Design".into(),
                content: String::new(),
            },
        )
        .unwrap();

        let id = resolve_page_identifier(&conn, "PRO-DOC-1").unwrap();
        assert_eq!(id, page.id);
    }

    #[test]
    fn resolve_page_identifier_workspace() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let page = create_page(
            &conn,
            &CreatePage {
                project_id: None,
                folder_id: None,
                title: "Global".into(),
                content: String::new(),
            },
        )
        .unwrap();

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
            },
        )
        .unwrap();
        assert_eq!(page.content, "# Title\n\nParagraph");
    }
}
