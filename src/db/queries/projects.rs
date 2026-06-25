use rusqlite::{params, Connection};

use crate::db::models::*;
use crate::error::LificError;

use super::unescape_text;

pub fn list_projects(conn: &Connection) -> Result<Vec<Project>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, name, identifier, description, emoji, lead_user_id, sort_order, created_at, updated_at
         FROM projects ORDER BY sort_order, name",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(Project {
            id: row.get(0)?,
            name: row.get(1)?,
            identifier: row.get(2)?,
            description: row.get(3)?,
            emoji: row.get(4)?,
            lead_user_id: row.get(5)?,
            sort_order: row.get(6)?,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn resolve_project_identifier(conn: &Connection, identifier: &str) -> Result<i64, LificError> {
    conn.prepare_cached("SELECT id FROM projects WHERE identifier = ?1")?
        .query_row(params![identifier], |row| row.get(0))
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                LificError::NotFound(format!("project '{identifier}' not found"))
            }
            _ => e.into(),
        })
}

pub fn get_project(conn: &Connection, id: i64) -> Result<Project, LificError> {
    conn.query_row(
        "SELECT id, name, identifier, description, emoji, lead_user_id, sort_order, created_at, updated_at
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
                sort_order: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
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

/// Validate a project identifier (LIF-134).
///
/// Identifiers are woven into issue (`LIF-42`) and page (`LIF-DOC-1`)
/// identifiers, so the grammar must keep parsing unambiguous:
/// - non-empty, at most 5 characters
/// - uppercase ASCII letters and digits only, starting with a letter
///   (a hyphen would break `resolve_identifier`, which splits at the
///   first `-`; lowercase would make lookups case-sensitive surprises)
/// - not the reserved word `DOC`, which marks page identifiers — a project
///   named DOC would make its issues (`DOC-1`) indistinguishable from
///   workspace pages
fn validate_identifier(identifier: &str) -> Result<(), LificError> {
    if identifier.is_empty() {
        return Err(LificError::BadRequest("identifier must not be empty".into()));
    }
    if identifier.chars().count() > 5 {
        return Err(LificError::BadRequest(
            "identifier must be 5 characters or fewer".into(),
        ));
    }
    let mut chars = identifier.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_uppercase());
    let rest_ok = chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
    if !first_ok || !rest_ok {
        return Err(LificError::BadRequest(
            "identifier must be uppercase letters/digits starting with a letter (e.g. LIF, PRO2)"
                .into(),
        ));
    }
    if identifier == "DOC" {
        return Err(LificError::BadRequest(
            "identifier 'DOC' is reserved for page identifiers".into(),
        ));
    }
    Ok(())
}

pub fn create_project(conn: &Connection, input: &CreateProject) -> Result<Project, LificError> {
    validate_identifier(&input.identifier)?;
    // LIF-233: append new projects below existing ones rather than letting them
    // default to rank 0 (which would jump them to the top once the user has
    // reordered). COALESCE handles the first-ever project (no rows yet).
    conn.execute(
        "INSERT INTO projects (name, identifier, description, emoji, lead_user_id, sort_order)
         VALUES (?1, ?2, ?3, ?4, ?5, (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM projects))",
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

/// LIF-233: reindex project `sort_order` to match the supplied id order
/// (position 0 = top of the sidebar). Full reindex on every call keeps ranks
/// dense and deterministic, avoiding the float-midpoint exhaustion and
/// all-equal-rank collisions a per-item PATCH scheme would hit.
///
/// Rejects duplicate ids and any id that doesn't exist (BadRequest). Projects
/// not present in `ids` keep their current rank — callers should send the full
/// list to guarantee a total order.
pub fn reorder_projects(conn: &Connection, ids: &[i64]) -> Result<Vec<Project>, LificError> {
    // Reject duplicates: an id appearing twice would silently clobber its own
    // rank and signals a malformed client request.
    let mut seen = std::collections::HashSet::new();
    for id in ids {
        if !seen.insert(*id) {
            return Err(LificError::BadRequest(format!(
                "duplicate project id {id} in reorder list"
            )));
        }
    }
    super::savepoint(conn, "reorder_projects", || {
        for (position, id) in ids.iter().enumerate() {
            let changed = conn.execute(
                "UPDATE projects SET sort_order = ?1 WHERE id = ?2",
                params![position as i64, id],
            )?;
            if changed == 0 {
                return Err(LificError::BadRequest(format!("project {id} not found")));
            }
        }
        Ok(())
    })?;
    list_projects(conn)
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
            validate_identifier(identifier)?;
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
        // LIF-103: tristate fields. Outer Some means the client sent the key;
        // inner None means they want NULL. rusqlite binds Option<T> to NULL
        // automatically when the inner is None.
        if let Some(emoji) = &input.emoji {
            conn.execute(
                "UPDATE projects SET emoji = ?1 WHERE id = ?2",
                params![emoji.as_ref(), id],
            )?;
        }
        if let Some(lead) = input.lead_user_id {
            // When setting a non-null lead, validate the user exists so we
            // return a 400 with a clear message instead of letting the FK
            // constraint surface as a generic 500.
            if let Some(uid) = lead {
                let exists = match conn.query_row(
                    "SELECT 1 FROM users WHERE id = ?1",
                    params![uid],
                    |_| Ok(true),
                ) {
                    Ok(_) => true,
                    Err(rusqlite::Error::QueryReturnedNoRows) => false,
                    Err(e) => return Err(e.into()),
                };
                if !exists {
                    return Err(LificError::BadRequest(format!(
                        "user {uid} not found"
                    )));
                }
            }
            conn.execute(
                "UPDATE projects SET lead_user_id = ?1 WHERE id = ?2",
                params![lead, id],
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

    // ── LIF-134: identifier grammar ──────────────────────────

    fn try_create(conn: &Connection, ident: &str) -> Result<Project, LificError> {
        create_project(
            conn,
            &CreateProject {
                name: format!("P {ident}"),
                identifier: ident.into(),
                description: String::new(),
                emoji: None,
                lead_user_id: None,
            },
        )
    }

    #[test]
    fn identifier_rejects_malformed_values() {
        let pool = test_db();
        let conn = pool.write().unwrap();

        // Empty, lowercase, hyphenated (breaks resolve_identifier), leading
        // digit, reserved page marker, >5 chars (counted in chars, not bytes).
        for bad in ["", "lif", "A-B", "1AB", "DOC", "TOOLNG", "🧪🧪"] {
            let result = try_create(&conn, bad);
            assert!(
                matches!(result, Err(LificError::BadRequest(_))),
                "identifier {bad:?} must be rejected, got: {result:?}"
            );
        }
    }

    #[test]
    fn identifier_accepts_uppercase_alphanumeric() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        for good in ["A", "LIF", "PRO2", "AB1C5"] {
            assert!(
                try_create(&conn, good).is_ok(),
                "identifier {good:?} must be accepted"
            );
        }
    }

    #[test]
    fn update_rejects_malformed_identifier() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = try_create(&conn, "GOOD").unwrap();

        for bad in ["A-B", "DOC", "bad"] {
            let result = update_project(
                &conn,
                project.id,
                &UpdateProject {
                    name: None,
                    identifier: Some(bad.into()),
                    description: None,
                    emoji: None,
                    lead_user_id: None,
                },
            );
            assert!(
                matches!(result, Err(LificError::BadRequest(_))),
                "identifier {bad:?} must be rejected on update, got: {result:?}"
            );
        }
        // Unchanged after the failed updates.
        assert_eq!(get_project(&conn, project.id).unwrap().identifier, "GOOD");
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

    // ── LIF-233: sidebar ordering ────────────────────────────

    fn seed_named(conn: &Connection, name: &str, ident: &str) -> i64 {
        create_project(
            conn,
            &CreateProject {
                name: name.into(),
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
    fn list_projects_tie_breaks_by_name_at_equal_rank() {
        // After the 025 migration every pre-existing project has sort_order 0,
        // so the listing must stay deterministic — alphabetical, as before the
        // feature. Force equal ranks to simulate that post-migration state
        // (create_project otherwise appends distinct ranks).
        let pool = test_db();
        let conn = pool.write().unwrap();
        seed_named(&conn, "Gamma", "G");
        seed_named(&conn, "Alpha", "A");
        seed_named(&conn, "Beta", "B");
        conn.execute("UPDATE projects SET sort_order = 0", []).unwrap();

        let names: Vec<String> = list_projects(&conn)
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names, ["Alpha", "Beta", "Gamma"]);
    }

    #[test]
    fn reorder_projects_sets_explicit_order() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let a = seed_named(&conn, "Alpha", "A");
        let b = seed_named(&conn, "Beta", "B");
        let g = seed_named(&conn, "Gamma", "G");

        // Put Gamma first, then Alpha, then Beta — the opposite of alphabetical.
        let reordered = reorder_projects(&conn, &[g, a, b]).unwrap();
        let names: Vec<String> = reordered.into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["Gamma", "Alpha", "Beta"]);

        // Order persists across a fresh list (sort_order, not query happenstance).
        let names: Vec<String> = list_projects(&conn)
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names, ["Gamma", "Alpha", "Beta"]);
    }

    #[test]
    fn reorder_rejects_unknown_id() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let a = seed_named(&conn, "Alpha", "A");
        let err = reorder_projects(&conn, &[a, 99999]).unwrap_err();
        assert!(matches!(err, LificError::BadRequest(_)), "got {err:?}");
        // The failed reorder is rolled back: Alpha keeps its original rank.
        assert_eq!(list_projects(&conn).unwrap()[0].name, "Alpha");
    }

    #[test]
    fn reorder_rejects_duplicate_id() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let a = seed_named(&conn, "Alpha", "A");
        let b = seed_named(&conn, "Beta", "B");
        let err = reorder_projects(&conn, &[a, b, a]).unwrap_err();
        assert!(matches!(err, LificError::BadRequest(_)), "got {err:?}");
    }

    #[test]
    fn new_project_appends_after_reorder() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let a = seed_named(&conn, "Alpha", "A");
        let b = seed_named(&conn, "Beta", "B");
        // Reorder so Beta(rank 0) precedes Alpha(rank 1).
        reorder_projects(&conn, &[b, a]).unwrap();
        // A brand-new project should land at the bottom, not jump to rank 0.
        seed_named(&conn, "Zeta", "Z");
        let names: Vec<String> = list_projects(&conn)
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names, ["Beta", "Alpha", "Zeta"]);
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

    // ── LIF-103: tristate clear-to-NULL semantics for emoji + lead_user_id ──

    /// Seed a real user so projects with lead_user_id pass the FK constraint.
    fn seed_user(conn: &Connection, username: &str) -> i64 {
        conn.execute(
            "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot)
             VALUES (?1, ?2, 'x', ?1, 0, 0)",
            params![username, format!("{username}@test.local")],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn update_can_clear_emoji() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = create_project(
            &conn,
            &CreateProject {
                name: "Has Emoji".into(),
                identifier: "EMJ".into(),
                description: String::new(),
                emoji: Some("🧪".into()),
                lead_user_id: None,
            },
        )
        .unwrap();
        assert_eq!(project.emoji.as_deref(), Some("🧪"));

        let updated = update_project(
            &conn,
            project.id,
            &UpdateProject {
                name: None,
                identifier: None,
                description: None,
                emoji: Some(None), // explicit clear
                lead_user_id: None,
            },
        )
        .unwrap();
        assert_eq!(updated.emoji, None);
    }

    #[test]
    fn update_can_clear_lead() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let uid = seed_user(&conn, "alice");
        let project = create_project(
            &conn,
            &CreateProject {
                name: "Has Lead".into(),
                identifier: "LDP".into(),
                description: String::new(),
                emoji: None,
                lead_user_id: Some(uid),
            },
        )
        .unwrap();
        assert_eq!(project.lead_user_id, Some(uid));

        let updated = update_project(
            &conn,
            project.id,
            &UpdateProject {
                name: None,
                identifier: None,
                description: None,
                emoji: None,
                lead_user_id: Some(None), // explicit clear
            },
        )
        .unwrap();
        assert_eq!(updated.lead_user_id, None);
    }

    #[test]
    fn update_absent_field_preserves_value() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let uid = seed_user(&conn, "bob");
        let project = create_project(
            &conn,
            &CreateProject {
                name: "Keep".into(),
                identifier: "KEP".into(),
                description: String::new(),
                emoji: Some("🎯".into()),
                lead_user_id: Some(uid),
            },
        )
        .unwrap();

        // Update unrelated field; emoji + lead should survive.
        let updated = update_project(
            &conn,
            project.id,
            &UpdateProject {
                name: Some("Keep Renamed".into()),
                identifier: None,
                description: None,
                emoji: None, // absent — preserve
                lead_user_id: None, // absent — preserve
            },
        )
        .unwrap();
        assert_eq!(updated.name, "Keep Renamed");
        assert_eq!(updated.emoji.as_deref(), Some("🎯"));
        assert_eq!(updated.lead_user_id, Some(uid));
    }

    #[test]
    fn update_lead_to_nonexistent_user_fails_with_bad_request() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = create_project(
            &conn,
            &CreateProject {
                name: "Orphan".into(),
                identifier: "ORP".into(),
                description: String::new(),
                emoji: None,
                lead_user_id: None,
            },
        )
        .unwrap();

        // 99999 doesn't exist. Should be a BadRequest, not a Database error.
        let result = update_project(
            &conn,
            project.id,
            &UpdateProject {
                name: None,
                identifier: None,
                description: None,
                emoji: None,
                lead_user_id: Some(Some(99999)),
            },
        );
        match result {
            Err(LificError::BadRequest(msg)) => {
                assert!(msg.contains("99999"), "got: {msg}");
                assert!(msg.contains("not found"), "got: {msg}");
            }
            other => panic!("expected BadRequest, got: {other:?}"),
        }

        // And the project should be unchanged (savepoint rolled back).
        let fetched = get_project(&conn, project.id).unwrap();
        assert_eq!(fetched.lead_user_id, None);
    }
}
