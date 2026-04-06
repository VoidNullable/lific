use rusqlite::{params, Connection};

use crate::db::models::*;
use crate::error::LificError;

use super::unescape_text;

pub fn list_projects(conn: &Connection) -> Result<Vec<Project>, LificError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, identifier, description, emoji, lead_user_id, created_at, updated_at
         FROM projects ORDER BY name",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(Project {
            id: row.get(0)?,
            name: row.get(1)?,
            identifier: row.get(2)?,
            description: row.get(3)?,
            emoji: row.get(4)?,
            lead_user_id: row.get(5)?,
            created_at: row.get(6)?,
            updated_at: row.get(7)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn resolve_project_identifier(conn: &Connection, identifier: &str) -> Result<i64, LificError> {
    conn.query_row(
        "SELECT id FROM projects WHERE identifier = ?1",
        params![identifier],
        |row| row.get(0),
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            LificError::NotFound(format!("project '{identifier}' not found"))
        }
        _ => e.into(),
    })
}

pub fn get_project(conn: &Connection, id: i64) -> Result<Project, LificError> {
    conn.query_row(
        "SELECT id, name, identifier, description, emoji, lead_user_id, created_at, updated_at
         FROM projects WHERE id = ?1",
        params![id],
        |row| {
            Ok(Project {
                id: row.get(0)?,
                name: row.get(1)?,
                identifier: row.get(2)?,
                description: row.get(3)?,
                emoji: row.get(4)?,
                lead_user_id: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        },
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            LificError::NotFound(format!("project {id} not found"))
        }
        _ => e.into(),
    })
}

pub fn create_project(conn: &Connection, input: &CreateProject) -> Result<Project, LificError> {
    if input.identifier.len() > 5 {
        return Err(LificError::BadRequest(
            "identifier must be 5 characters or fewer".into(),
        ));
    }
    conn.execute(
        "INSERT INTO projects (name, identifier, description, emoji, lead_user_id)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            input.name,
            input.identifier,
            unescape_text(&input.description),
            input.emoji,
            input.lead_user_id
        ],
    )?;
    get_project(conn, conn.last_insert_rowid())
}

pub fn update_project(
    conn: &Connection,
    id: i64,
    input: &UpdateProject,
) -> Result<Project, LificError> {
    get_project(conn, id)?;
    super::savepoint(conn, "update_project", || {
        if let Some(ref name) = input.name {
            conn.execute(
                "UPDATE projects SET name = ?1 WHERE id = ?2",
                params![name, id],
            )?;
        }
        if let Some(ref identifier) = input.identifier {
            if identifier.len() > 5 {
                return Err(LificError::BadRequest(
                    "identifier must be 5 characters or fewer".into(),
                ));
            }
            conn.execute(
                "UPDATE projects SET identifier = ?1 WHERE id = ?2",
                params![identifier, id],
            )?;
        }
        if let Some(ref description) = input.description {
            conn.execute(
                "UPDATE projects SET description = ?1 WHERE id = ?2",
                params![unescape_text(description), id],
            )?;
        }
        if let Some(ref emoji) = input.emoji {
            conn.execute(
                "UPDATE projects SET emoji = ?1 WHERE id = ?2",
                params![emoji, id],
            )?;
        }
        if let Some(lead_user_id) = input.lead_user_id {
            conn.execute(
                "UPDATE projects SET lead_user_id = ?1 WHERE id = ?2",
                params![lead_user_id, id],
            )?;
        }
        Ok(())
    })?;
    get_project(conn, id)
}

pub fn delete_project(conn: &Connection, id: i64) -> Result<(), LificError> {
    let changed = conn.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
    if changed == 0 {
        return Err(LificError::NotFound(format!("project {id} not found")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn test_db() -> db::DbPool {
        db::open_memory().expect("test db")
    }

    #[test]
    fn create_and_get_project() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = create_project(
            &conn,
            &CreateProject {
                name: "Test".into(),
                identifier: "TST".into(),
                description: "A test project".into(),
                emoji: Some("🧪".into()),
                lead_user_id: None,
            },
        )
        .unwrap();

        assert_eq!(project.name, "Test");
        assert_eq!(project.identifier, "TST");
        assert_eq!(project.description, "A test project");
        assert_eq!(project.emoji, Some("🧪".into()));

        let fetched = get_project(&conn, project.id).unwrap();
        assert_eq!(fetched.identifier, "TST");
    }

    #[test]
    fn resolve_project_identifier_works() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        create_project(
            &conn,
            &CreateProject {
                name: "Lific".into(),
                identifier: "LIF".into(),
                description: String::new(),
                emoji: None,
                lead_user_id: None,
            },
        )
        .unwrap();

        let id = resolve_project_identifier(&conn, "LIF").unwrap();
        assert!(id > 0);
    }

    #[test]
    fn resolve_project_not_found() {
        let pool = test_db();
        let conn = pool.read().unwrap();
        let result = resolve_project_identifier(&conn, "NOPE");
        assert!(result.is_err());
    }

    #[test]
    fn duplicate_identifier_rejected() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        create_project(
            &conn,
            &CreateProject {
                name: "First".into(),
                identifier: "DUP".into(),
                description: String::new(),
                emoji: None,
                lead_user_id: None,
            },
        )
        .unwrap();

        let result = create_project(
            &conn,
            &CreateProject {
                name: "Second".into(),
                identifier: "DUP".into(),
                description: String::new(),
                emoji: None,
                lead_user_id: None,
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn update_project_fields() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = create_project(
            &conn,
            &CreateProject {
                name: "Old Name".into(),
                identifier: "OLD".into(),
                description: String::new(),
                emoji: None,
                lead_user_id: None,
            },
        )
        .unwrap();

        let updated = update_project(
            &conn,
            project.id,
            &UpdateProject {
                name: Some("New Name".into()),
                identifier: None,
                description: Some("Now with description".into()),
                emoji: None,
                lead_user_id: None,
            },
        )
        .unwrap();

        assert_eq!(updated.name, "New Name");
        assert_eq!(updated.identifier, "OLD"); // unchanged
        assert_eq!(updated.description, "Now with description");
    }

    #[test]
    fn delete_project_removes_it() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = create_project(
            &conn,
            &CreateProject {
                name: "Doomed".into(),
                identifier: "DEL".into(),
                description: String::new(),
                emoji: None,
                lead_user_id: None,
            },
        )
        .unwrap();

        delete_project(&conn, project.id).unwrap();
        assert!(get_project(&conn, project.id).is_err());
    }

    #[test]
    fn delete_project_not_found() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let result = delete_project(&conn, 99999);
        assert!(result.is_err());
    }

    #[test]
    fn list_projects_returns_all() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        for (name, ident) in [("Alpha", "A"), ("Beta", "B"), ("Gamma", "G")] {
            create_project(
                &conn,
                &CreateProject {
                    name: name.into(),
                    identifier: ident.into(),
                    description: String::new(),
                    emoji: None,
                    lead_user_id: None,
                },
            )
            .unwrap();
        }

        let projects = list_projects(&conn).unwrap();
        assert_eq!(projects.len(), 3);
    }

    #[test]
    fn unescape_in_description() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = create_project(
            &conn,
            &CreateProject {
                name: "Escaped".into(),
                identifier: "ESC".into(),
                description: "line1\\nline2\\ttab".into(),
                emoji: None,
                lead_user_id: None,
            },
        )
        .unwrap();

        assert_eq!(project.description, "line1\nline2\ttab");
    }
}
