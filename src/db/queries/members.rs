//! LIF-195: project membership — the (user, role) source of truth for
//! project-scoped authorization (epic LIF-194).
//!
//! Pure data access, no enforcement: nothing here checks whether the caller
//! is *allowed* to do this. `projects.lead_user_id` stays the denormalized
//! "primary lead" pointer read by today's LIF-102 access check; the write
//! paths that set it (`queries::projects::create_project` /
//! `update_project`) call [`upsert_member`] to keep both in sync.

use rusqlite::{params, Connection, OptionalExtension};

use crate::db::models::{ProjectMember, Role};
use crate::error::LificError;

/// List a project's members, oldest membership first.
/// Not yet called outside tests — the read surface for LIF-194's
/// enforcement/UI layer (member list endpoint) lands in a later issue.
#[allow(dead_code)]
pub fn list_members(conn: &Connection, project_id: i64) -> Result<Vec<ProjectMember>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT project_id, user_id, role, created_at FROM project_members
         WHERE project_id = ?1 ORDER BY created_at, user_id",
    )?;
    let rows = stmt.query_map(params![project_id], |row| {
        Ok(ProjectMember {
            project_id: row.get(0)?,
            user_id: row.get(1)?,
            role: row.get(2)?,
            created_at: row.get(3)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// Look up a single user's role on a project. `None` when they aren't a
/// member (distinct from an error — "not a member" is a normal state).
/// Called by `authz::require_role` (LIF-196).
pub fn get_member_role(
    conn: &Connection,
    project_id: i64,
    user_id: i64,
) -> Result<Option<Role>, LificError> {
    conn.prepare_cached(
        "SELECT role FROM project_members WHERE project_id = ?1 AND user_id = ?2",
    )?
    .query_row(params![project_id, user_id], |row| row.get(0))
    .optional()
    .map_err(Into::into)
}

/// Insert or update a member's role. Idempotent — re-running with the same
/// role is a no-op; a different role overwrites it in place (membership
/// rows aren't versioned, so a role change has no history of its own here).
pub fn upsert_member(
    conn: &Connection,
    project_id: i64,
    user_id: i64,
    role: Role,
) -> Result<ProjectMember, LificError> {
    conn.execute(
        "INSERT INTO project_members (project_id, user_id, role)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(project_id, user_id) DO UPDATE SET role = excluded.role",
        params![project_id, user_id, role],
    )?;
    conn.query_row(
        "SELECT project_id, user_id, role, created_at FROM project_members
         WHERE project_id = ?1 AND user_id = ?2",
        params![project_id, user_id],
        |row| {
            Ok(ProjectMember {
                project_id: row.get(0)?,
                user_id: row.get(1)?,
                role: row.get(2)?,
                created_at: row.get(3)?,
            })
        },
    )
    .map_err(Into::into)
}

/// Remove a member from a project.
/// Not yet called outside tests — no API/MCP surface exists to remove a
/// member yet; that lands with LIF-194's write endpoints.
#[allow(dead_code)]
pub fn remove_member(conn: &Connection, project_id: i64, user_id: i64) -> Result<(), LificError> {
    let changed = conn.execute(
        "DELETE FROM project_members WHERE project_id = ?1 AND user_id = ?2",
        params![project_id, user_id],
    )?;
    if changed == 0 {
        return Err(LificError::NotFound(format!(
            "user {user_id} is not a member of project {project_id}"
        )));
    }
    Ok(())
}

/// List every project id the given user has a membership row on (any role).
/// Powers `authz::visible_project_ids` (LIF-196) — the cross-project read
/// filter for search / project listing once enforcement is wired in.
pub fn list_project_ids_for_user(conn: &Connection, user_id: i64) -> Result<Vec<i64>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT project_id FROM project_members WHERE user_id = ?1",
    )?;
    let rows = stmt.query_map(params![user_id], |row| row.get(0))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// Count how many `lead` members a project currently has. Not used for
/// enforcement yet (LIF-194 is out of scope here), but the natural query
/// a future "don't demote the last lead" guard will need.
#[allow(dead_code)]
pub fn count_leads(conn: &Connection, project_id: i64) -> Result<i64, LificError> {
    conn.query_row(
        "SELECT COUNT(*) FROM project_members WHERE project_id = ?1 AND role = 'lead'",
        params![project_id],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{self, queries::projects};
    use crate::db::models::{CreateProject, UpdateProject};

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

    fn seed_project(conn: &Connection, ident: &str) -> i64 {
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

    // ── Role enum ─────────────────────────────────────────────

    #[test]
    fn role_ordering_is_viewer_lt_maintainer_lt_lead() {
        assert!(Role::Viewer < Role::Maintainer);
        assert!(Role::Maintainer < Role::Lead);
        assert!(Role::Viewer < Role::Lead);
        assert_eq!(Role::Viewer.max(Role::Lead), Role::Lead);
    }

    #[test]
    fn role_parse_and_display_round_trip() {
        for (s, role) in [
            ("viewer", Role::Viewer),
            ("maintainer", Role::Maintainer),
            ("lead", Role::Lead),
        ] {
            let parsed: Role = s.parse().unwrap();
            assert_eq!(parsed, role);
            assert_eq!(role.to_string(), s);
            assert_eq!(role.as_str(), s);
        }
    }

    #[test]
    fn role_parse_rejects_unknown_string() {
        assert!("owner".parse::<Role>().is_err());
        assert!("".parse::<Role>().is_err());
        assert!("Lead".parse::<Role>().is_err()); // case-sensitive, matches CHECK values
    }

    // ── CRUD round-trips ─────────────────────────────────────

    #[test]
    fn upsert_and_list_members() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project_id = seed_project(&conn, "MEM");
        let alice = seed_user(&conn, "alice");
        let bob = seed_user(&conn, "bob");

        upsert_member(&conn, project_id, alice, Role::Lead).unwrap();
        upsert_member(&conn, project_id, bob, Role::Viewer).unwrap();

        let members = list_members(&conn, project_id).unwrap();
        assert_eq!(members.len(), 2);
        assert_eq!(
            get_member_role(&conn, project_id, alice).unwrap(),
            Some(Role::Lead)
        );
        assert_eq!(
            get_member_role(&conn, project_id, bob).unwrap(),
            Some(Role::Viewer)
        );
    }

    #[test]
    fn get_member_role_none_when_not_a_member() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project_id = seed_project(&conn, "NOM");
        let alice = seed_user(&conn, "alice");
        assert_eq!(get_member_role(&conn, project_id, alice).unwrap(), None);
    }

    #[test]
    fn upsert_overwrites_existing_role() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project_id = seed_project(&conn, "OVR");
        let alice = seed_user(&conn, "alice");

        upsert_member(&conn, project_id, alice, Role::Viewer).unwrap();
        assert_eq!(
            get_member_role(&conn, project_id, alice).unwrap(),
            Some(Role::Viewer)
        );

        upsert_member(&conn, project_id, alice, Role::Maintainer).unwrap();
        let members = list_members(&conn, project_id).unwrap();
        assert_eq!(members.len(), 1, "upsert must not duplicate the row");
        assert_eq!(
            get_member_role(&conn, project_id, alice).unwrap(),
            Some(Role::Maintainer)
        );
    }

    #[test]
    fn remove_member_deletes_the_row() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project_id = seed_project(&conn, "DEL");
        let alice = seed_user(&conn, "alice");
        upsert_member(&conn, project_id, alice, Role::Viewer).unwrap();

        remove_member(&conn, project_id, alice).unwrap();
        assert_eq!(get_member_role(&conn, project_id, alice).unwrap(), None);
    }

    #[test]
    fn remove_member_not_found() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project_id = seed_project(&conn, "NF");
        let alice = seed_user(&conn, "alice");
        let err = remove_member(&conn, project_id, alice).unwrap_err();
        assert!(matches!(err, LificError::NotFound(_)), "got {err:?}");
    }

    #[test]
    fn count_leads_counts_only_lead_role() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project_id = seed_project(&conn, "CNT");
        let alice = seed_user(&conn, "alice");
        let bob = seed_user(&conn, "bob");
        let carol = seed_user(&conn, "carol");

        upsert_member(&conn, project_id, alice, Role::Lead).unwrap();
        upsert_member(&conn, project_id, bob, Role::Lead).unwrap();
        upsert_member(&conn, project_id, carol, Role::Viewer).unwrap();

        assert_eq!(count_leads(&conn, project_id).unwrap(), 2);
    }

    #[test]
    fn invalid_role_rejected_by_check_constraint() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project_id = seed_project(&conn, "CHK");
        let alice = seed_user(&conn, "alice");

        // Bypass the Rust-level enum entirely to prove the DB-level CHECK
        // constraint (not just app code) rejects bad roles.
        let result = conn.execute(
            "INSERT INTO project_members (project_id, user_id, role) VALUES (?1, ?2, 'owner')",
            params![project_id, alice],
        );
        assert!(result.is_err(), "CHECK constraint must reject 'owner'");
    }

    // ── Cascade deletes ────────────────────────────────────────

    #[test]
    fn cascade_delete_on_project_removes_membership() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project_id = seed_project(&conn, "CPD");
        let alice = seed_user(&conn, "alice");
        upsert_member(&conn, project_id, alice, Role::Viewer).unwrap();

        projects::delete_project(&conn, project_id).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project_members WHERE project_id = ?1",
                params![project_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn cascade_delete_on_user_removes_membership() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project_id = seed_project(&conn, "CUD");
        let alice = seed_user(&conn, "alice");
        upsert_member(&conn, project_id, alice, Role::Viewer).unwrap();

        conn.execute("DELETE FROM users WHERE id = ?1", params![alice])
            .unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project_members WHERE user_id = ?1",
                params![alice],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    // ── Write-path consistency (LIF-195 scope item 4) ──────────

    #[test]
    fn create_project_with_lead_seeds_lead_membership() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let project = projects::create_project(
            &conn,
            &CreateProject {
                name: "Has Lead".into(),
                identifier: "HLD".into(),
                description: String::new(),
                emoji: None,
                lead_user_id: Some(alice),
            },
        )
        .unwrap();

        assert_eq!(
            get_member_role(&conn, project.id, alice).unwrap(),
            Some(Role::Lead)
        );
    }

    #[test]
    fn create_project_without_lead_seeds_no_membership() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = projects::create_project(
            &conn,
            &CreateProject {
                name: "No Lead".into(),
                identifier: "NLD".into(),
                description: String::new(),
                emoji: None,
                lead_user_id: None,
            },
        )
        .unwrap();

        assert!(list_members(&conn, project.id).unwrap().is_empty());
    }

    #[test]
    fn update_project_lead_upserts_new_lead_membership() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let bob = seed_user(&conn, "bob");
        let project = projects::create_project(
            &conn,
            &CreateProject {
                name: "Handoff".into(),
                identifier: "HND".into(),
                description: String::new(),
                emoji: None,
                lead_user_id: Some(alice),
            },
        )
        .unwrap();
        assert_eq!(
            get_member_role(&conn, project.id, alice).unwrap(),
            Some(Role::Lead)
        );

        projects::update_project(
            &conn,
            project.id,
            &UpdateProject {
                name: None,
                identifier: None,
                description: None,
                emoji: None,
                lead_user_id: Some(Some(bob)),
            },
        )
        .unwrap();

        assert_eq!(
            get_member_role(&conn, project.id, bob).unwrap(),
            Some(Role::Lead),
            "new lead must get a membership row"
        );
        assert_eq!(
            get_member_role(&conn, project.id, alice).unwrap(),
            Some(Role::Lead),
            "old lead keeps their membership row — memberships are additive"
        );
    }

    #[test]
    fn update_project_clearing_lead_does_not_touch_memberships() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let project = projects::create_project(
            &conn,
            &CreateProject {
                name: "Clearable".into(),
                identifier: "CLR".into(),
                description: String::new(),
                emoji: None,
                lead_user_id: Some(alice),
            },
        )
        .unwrap();

        let updated = projects::update_project(
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
        // Alice's membership row is untouched by clearing the denormalized pointer.
        assert_eq!(
            get_member_role(&conn, project.id, alice).unwrap(),
            Some(Role::Lead)
        );
    }
}
