use rusqlite::{Connection, params};

use crate::db::models::*;
use crate::error::LificError;

use super::unescape_text;

/// Look up the project_id for a module, label, or folder by its id.
pub fn get_resource_project_id(conn: &Connection, table: &str, id: i64) -> Result<i64, LificError> {
    let sql = format!("SELECT project_id FROM {table} WHERE id = ?1");
    conn.query_row(&sql, params![id], |row| row.get(0))
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                LificError::NotFound(format!("{table} {id} not found"))
            }
            _ => e.into(),
        })
}

pub fn resolve_module_name(
    conn: &Connection,
    project_id: i64,
    name: &str,
) -> Result<i64, LificError> {
    conn.query_row(
        "SELECT id FROM modules WHERE project_id = ?1 AND name = ?2 COLLATE NOCASE",
        params![project_id, name],
        |row| row.get(0),
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            LificError::NotFound(format!("module '{name}' not found in project"))
        }
        _ => e.into(),
    })
}

pub fn resolve_folder_name(
    conn: &Connection,
    project_id: i64,
    name: &str,
) -> Result<i64, LificError> {
    conn.query_row(
        "SELECT id FROM folders WHERE project_id = ?1 AND name = ?2 COLLATE NOCASE",
        params![project_id, name],
        |row| row.get(0),
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            LificError::NotFound(format!("folder '{name}' not found in project"))
        }
        _ => e.into(),
    })
}

pub fn resolve_label_name(
    conn: &Connection,
    project_id: i64,
    name: &str,
) -> Result<i64, LificError> {
    conn.query_row(
        "SELECT id FROM labels WHERE project_id = ?1 AND name = ?2 COLLATE NOCASE",
        params![project_id, name],
        |row| row.get(0),
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            LificError::NotFound(format!("label '{name}' not found in project"))
        }
        _ => e.into(),
    })
}

pub fn get_module_name(conn: &Connection, id: i64) -> Result<String, LificError> {
    conn.query_row(
        "SELECT name FROM modules WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            LificError::NotFound(format!("module {id} not found"))
        }
        _ => e.into(),
    })
}

pub fn list_modules(conn: &Connection, project_id: i64) -> Result<Vec<Module>, LificError> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, name, description, status, created_at, updated_at
         FROM modules WHERE project_id = ?1 ORDER BY name",
    )?;
    let rows = stmt.query_map(params![project_id], |row| {
        Ok(Module {
            id: row.get(0)?,
            project_id: row.get(1)?,
            name: row.get(2)?,
            description: row.get(3)?,
            status: row.get(4)?,
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn create_module(conn: &Connection, input: &CreateModule) -> Result<Module, LificError> {
    conn.execute(
        "INSERT INTO modules (project_id, name, description, status) VALUES (?1, ?2, ?3, ?4)",
        params![
            input.project_id,
            input.name,
            unescape_text(&input.description),
            input.status
        ],
    )?;
    let id = conn.last_insert_rowid();
    conn.query_row(
        "SELECT id, project_id, name, description, status, created_at, updated_at FROM modules WHERE id = ?1",
        params![id],
        |row| Ok(Module {
            id: row.get(0)?, project_id: row.get(1)?, name: row.get(2)?,
            description: row.get(3)?, status: row.get(4)?, created_at: row.get(5)?, updated_at: row.get(6)?,
        }),
    ).map_err(Into::into)
}

pub fn update_module(
    conn: &Connection,
    id: i64,
    input: &UpdateModule,
) -> Result<Module, LificError> {
    if let Some(ref name) = input.name {
        conn.execute(
            "UPDATE modules SET name = ?1 WHERE id = ?2",
            params![name, id],
        )?;
    }
    if let Some(ref description) = input.description {
        conn.execute(
            "UPDATE modules SET description = ?1 WHERE id = ?2",
            params![unescape_text(description), id],
        )?;
    }
    if let Some(ref status) = input.status {
        conn.execute(
            "UPDATE modules SET status = ?1 WHERE id = ?2",
            params![status, id],
        )?;
    }
    conn.query_row(
        "SELECT id, project_id, name, description, status, created_at, updated_at FROM modules WHERE id = ?1",
        params![id],
        |row| Ok(Module {
            id: row.get(0)?, project_id: row.get(1)?, name: row.get(2)?,
            description: row.get(3)?, status: row.get(4)?, created_at: row.get(5)?, updated_at: row.get(6)?,
        }),
    ).map_err(|e| match e { rusqlite::Error::QueryReturnedNoRows => LificError::NotFound(format!("module {id} not found")), _ => e.into() })
}

pub fn delete_module(conn: &Connection, id: i64) -> Result<(), LificError> {
    let changed = conn.execute("DELETE FROM modules WHERE id = ?1", params![id])?;
    if changed == 0 {
        return Err(LificError::NotFound(format!("module {id} not found")));
    }
    Ok(())
}

pub fn list_labels(conn: &Connection, project_id: i64) -> Result<Vec<Label>, LificError> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, name, color FROM labels WHERE project_id = ?1 ORDER BY name",
    )?;
    let rows = stmt.query_map(params![project_id], |row| {
        Ok(Label {
            id: row.get(0)?,
            project_id: row.get(1)?,
            name: row.get(2)?,
            color: row.get(3)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn create_label(conn: &Connection, input: &CreateLabel) -> Result<Label, LificError> {
    conn.execute(
        "INSERT INTO labels (project_id, name, color) VALUES (?1, ?2, ?3)",
        params![input.project_id, input.name, input.color],
    )?;
    Ok(Label {
        id: conn.last_insert_rowid(),
        project_id: input.project_id,
        name: input.name.clone(),
        color: input.color.clone(),
    })
}

pub fn update_label(conn: &Connection, id: i64, input: &UpdateLabel) -> Result<Label, LificError> {
    if let Some(ref name) = input.name {
        conn.execute(
            "UPDATE labels SET name = ?1 WHERE id = ?2",
            params![name, id],
        )?;
    }
    if let Some(ref color) = input.color {
        conn.execute(
            "UPDATE labels SET color = ?1 WHERE id = ?2",
            params![color, id],
        )?;
    }
    conn.query_row(
        "SELECT id, project_id, name, color FROM labels WHERE id = ?1",
        params![id],
        |row| {
            Ok(Label {
                id: row.get(0)?,
                project_id: row.get(1)?,
                name: row.get(2)?,
                color: row.get(3)?,
            })
        },
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            LificError::NotFound(format!("label {id} not found"))
        }
        _ => e.into(),
    })
}

pub fn delete_label(conn: &Connection, id: i64) -> Result<(), LificError> {
    let changed = conn.execute("DELETE FROM labels WHERE id = ?1", params![id])?;
    if changed == 0 {
        return Err(LificError::NotFound(format!("label {id} not found")));
    }
    Ok(())
}

pub fn list_folders(conn: &Connection, project_id: i64) -> Result<Vec<Folder>, LificError> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, parent_id, name, sort_order FROM folders WHERE project_id = ?1 ORDER BY sort_order, name",
    )?;
    let rows = stmt.query_map(params![project_id], |row| {
        Ok(Folder {
            id: row.get(0)?,
            project_id: row.get(1)?,
            parent_id: row.get(2)?,
            name: row.get(3)?,
            sort_order: row.get(4)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn create_folder(conn: &Connection, input: &CreateFolder) -> Result<Folder, LificError> {
    conn.execute(
        "INSERT INTO folders (project_id, parent_id, name) VALUES (?1, ?2, ?3)",
        params![input.project_id, input.parent_id, input.name],
    )?;
    Ok(Folder {
        id: conn.last_insert_rowid(),
        project_id: input.project_id,
        parent_id: input.parent_id,
        name: input.name.clone(),
        sort_order: 0.0,
    })
}

pub fn update_folder(
    conn: &Connection,
    id: i64,
    input: &UpdateFolder,
) -> Result<Folder, LificError> {
    if let Some(ref name) = input.name {
        conn.execute(
            "UPDATE folders SET name = ?1 WHERE id = ?2",
            params![name, id],
        )?;
    }
    conn.query_row(
        "SELECT id, project_id, parent_id, name, sort_order FROM folders WHERE id = ?1",
        params![id],
        |row| {
            Ok(Folder {
                id: row.get(0)?,
                project_id: row.get(1)?,
                parent_id: row.get(2)?,
                name: row.get(3)?,
                sort_order: row.get(4)?,
            })
        },
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            LificError::NotFound(format!("folder {id} not found"))
        }
        _ => e.into(),
    })
}

pub fn delete_folder(conn: &Connection, id: i64) -> Result<(), LificError> {
    let changed = conn.execute("DELETE FROM folders WHERE id = ?1", params![id])?;
    if changed == 0 {
        return Err(LificError::NotFound(format!("folder {id} not found")));
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

    fn seed_project(conn: &rusqlite::Connection) -> i64 {
        projects::create_project(
            conn,
            &CreateProject {
                name: "Test".into(),
                identifier: "TST".into(),
                description: String::new(),
                emoji: None,
                lead_user_id: None,
            },
        )
        .unwrap()
        .id
    }

    #[test]
    fn module_crud() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn);

        let module = create_module(
            &conn,
            &CreateModule {
                project_id: pid,
                name: "Core".into(),
                description: "The core".into(),
                status: "active".into(),
            },
        )
        .unwrap();
        assert_eq!(module.name, "Core");
        assert_eq!(module.status, "active");

        let updated = update_module(
            &conn,
            module.id,
            &UpdateModule {
                name: Some("Core DB".into()),
                description: None,
                status: Some("done".into()),
            },
        )
        .unwrap();
        assert_eq!(updated.name, "Core DB");
        assert_eq!(updated.status, "done");

        let modules = list_modules(&conn, pid).unwrap();
        assert_eq!(modules.len(), 1);

        delete_module(&conn, module.id).unwrap();
        assert_eq!(list_modules(&conn, pid).unwrap().len(), 0);
    }

    #[test]
    fn resolve_module_name_case_insensitive() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn);
        let module = create_module(
            &conn,
            &CreateModule {
                project_id: pid,
                name: "Authentication".into(),
                description: String::new(),
                status: "active".into(),
            },
        )
        .unwrap();

        assert_eq!(
            resolve_module_name(&conn, pid, "Authentication").unwrap(),
            module.id
        );
        assert_eq!(
            resolve_module_name(&conn, pid, "authentication").unwrap(),
            module.id
        );
        assert_eq!(
            resolve_module_name(&conn, pid, "AUTHENTICATION").unwrap(),
            module.id
        );
        assert!(resolve_module_name(&conn, pid, "nonexistent").is_err());
    }

    #[test]
    fn get_module_name_by_id() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn);
        let module = create_module(
            &conn,
            &CreateModule {
                project_id: pid,
                name: "MCP Server".into(),
                description: String::new(),
                status: "active".into(),
            },
        )
        .unwrap();

        assert_eq!(get_module_name(&conn, module.id).unwrap(), "MCP Server");
        assert!(get_module_name(&conn, 99999).is_err());
    }

    #[test]
    fn label_crud() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn);

        let label = create_label(
            &conn,
            &CreateLabel {
                project_id: pid,
                name: "bug".into(),
                color: "#EF4444".into(),
            },
        )
        .unwrap();
        assert_eq!(label.name, "bug");
        assert_eq!(label.color, "#EF4444");

        let labels = list_labels(&conn, pid).unwrap();
        assert_eq!(labels.len(), 1);

        delete_label(&conn, label.id).unwrap();
        assert_eq!(list_labels(&conn, pid).unwrap().len(), 0);
    }

    #[test]
    fn update_label_fields() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn);
        let label = create_label(
            &conn,
            &CreateLabel {
                project_id: pid,
                name: "bug".into(),
                color: "#EF4444".into(),
            },
        )
        .unwrap();

        let updated = update_label(
            &conn,
            label.id,
            &UpdateLabel {
                name: Some("defect".into()),
                color: None,
            },
        )
        .unwrap();
        assert_eq!(updated.name, "defect");
        assert_eq!(updated.color, "#EF4444"); // unchanged

        let updated = update_label(
            &conn,
            label.id,
            &UpdateLabel {
                name: None,
                color: Some("#FF0000".into()),
            },
        )
        .unwrap();
        assert_eq!(updated.name, "defect"); // unchanged
        assert_eq!(updated.color, "#FF0000");
    }

    #[test]
    fn update_folder_name() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn);
        let folder = create_folder(
            &conn,
            &CreateFolder {
                project_id: pid,
                parent_id: None,
                name: "Docs".into(),
            },
        )
        .unwrap();

        let updated = update_folder(
            &conn,
            folder.id,
            &UpdateFolder {
                name: Some("Documentation".into()),
            },
        )
        .unwrap();
        assert_eq!(updated.name, "Documentation");
    }

    #[test]
    fn resolve_label_name_case_insensitive() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn);
        let label = create_label(
            &conn,
            &CreateLabel {
                project_id: pid,
                name: "Feature".into(),
                color: "#22C55E".into(),
            },
        )
        .unwrap();

        assert_eq!(resolve_label_name(&conn, pid, "Feature").unwrap(), label.id);
        assert_eq!(resolve_label_name(&conn, pid, "feature").unwrap(), label.id);
        assert!(resolve_label_name(&conn, pid, "nope").is_err());
    }

    #[test]
    fn folder_crud() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn);

        let folder = create_folder(
            &conn,
            &CreateFolder {
                project_id: pid,
                parent_id: None,
                name: "Docs".into(),
            },
        )
        .unwrap();
        assert_eq!(folder.name, "Docs");

        let folders = list_folders(&conn, pid).unwrap();
        assert_eq!(folders.len(), 1);

        delete_folder(&conn, folder.id).unwrap();
        assert_eq!(list_folders(&conn, pid).unwrap().len(), 0);
    }

    #[test]
    fn resolve_folder_name_case_insensitive() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn);
        let folder = create_folder(
            &conn,
            &CreateFolder {
                project_id: pid,
                parent_id: None,
                name: "Architecture".into(),
            },
        )
        .unwrap();

        assert_eq!(
            resolve_folder_name(&conn, pid, "Architecture").unwrap(),
            folder.id
        );
        assert_eq!(
            resolve_folder_name(&conn, pid, "architecture").unwrap(),
            folder.id
        );
        assert!(resolve_folder_name(&conn, pid, "nope").is_err());
    }

    #[test]
    fn module_unescape_description() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn);
        let module = create_module(
            &conn,
            &CreateModule {
                project_id: pid,
                name: "Test".into(),
                description: "line1\\nline2".into(),
                status: "active".into(),
            },
        )
        .unwrap();
        assert_eq!(module.description, "line1\nline2");
    }
}
