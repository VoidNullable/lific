//! Per-user project groups for the sidebar.
//!
//! Ownership mirrors `queries::views`: every function targeting an existing
//! row resolves it through [`get_owned_group`], which folds "no such group"
//! and "someone else's group" into the same `NotFound`, so a group id can't
//! be used to probe another user's sidebar.

use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::models::{CreateProjectGroup, ProjectGroup, UpdateProjectGroup};
use crate::error::LificError;

fn validate_name(name: &str) -> Result<(), LificError> {
    if name.trim().is_empty() {
        return Err(LificError::BadRequest("name must not be empty".into()));
    }
    Ok(())
}

/// Map the `UNIQUE (user_id, name)` violation onto a 409 the client can show
/// verbatim. Mirrors `views::constraint_err`.
fn constraint_err(name: &str) -> impl Fn(rusqlite::Error) -> LificError + '_ {
    move |e| match e {
        rusqlite::Error::SqliteFailure(err, _)
            if err.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            LificError::Conflict(format!("a group named '{name}' already exists"))
        }
        other => other.into(),
    }
}

fn row_to_group(row: &rusqlite::Row) -> rusqlite::Result<ProjectGroup> {
    Ok(ProjectGroup {
        id: row.get(0)?,
        user_id: row.get(1)?,
        name: row.get(2)?,
        sort_order: row.get(3)?,
        // Filled in by list_groups; a bare row carries no membership.
        project_ids: Vec::new(),
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

fn get_owned_group(conn: &Connection, id: i64, user_id: i64) -> Result<ProjectGroup, LificError> {
    conn.query_row(
        "SELECT id, user_id, name, sort_order, created_at, updated_at
         FROM project_groups WHERE id = ?1 AND user_id = ?2",
        params![id, user_id],
        row_to_group,
    )
    .optional()?
    .ok_or_else(|| LificError::NotFound(format!("project group {id} not found")))
}

/// The caller's groups with their member project ids. Memberships come back
/// in one extra statement and are bucketed in memory, not one query per group.
pub fn list_groups(conn: &Connection, user_id: i64) -> Result<Vec<ProjectGroup>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, user_id, name, sort_order, created_at, updated_at
         FROM project_groups WHERE user_id = ?1
         ORDER BY sort_order, name COLLATE NOCASE",
    )?;
    let mut groups: Vec<ProjectGroup> = stmt
        .query_map(params![user_id], row_to_group)?
        .collect::<Result<Vec<_>, _>>()?;

    let mut stmt = conn.prepare_cached(
        "SELECT i.group_id, i.project_id
         FROM project_group_items i
         JOIN project_groups g ON g.id = i.group_id
         WHERE g.user_id = ?1",
    )?;
    let mut by_group: HashMap<i64, Vec<i64>> = HashMap::new();
    let rows = stmt.query_map(params![user_id], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (group_id, project_id) = row?;
        by_group.entry(group_id).or_default().push(project_id);
    }
    for group in &mut groups {
        group.project_ids = by_group.remove(&group.id).unwrap_or_default();
    }
    Ok(groups)
}

/// Create a group, appended after the caller's existing ones.
pub fn create_group(
    conn: &Connection,
    user_id: i64,
    input: &CreateProjectGroup,
) -> Result<ProjectGroup, LificError> {
    validate_name(&input.name)?;
    let name = input.name.trim();
    let next_order: i64 = conn.query_row(
        "SELECT COALESCE(MAX(sort_order) + 1, 0) FROM project_groups WHERE user_id = ?1",
        params![user_id],
        |row| row.get(0),
    )?;
    conn.execute(
        "INSERT INTO project_groups (user_id, name, sort_order) VALUES (?1, ?2, ?3)",
        params![user_id, name, next_order],
    )
    .map_err(constraint_err(name))?;
    get_owned_group(conn, conn.last_insert_rowid(), user_id)
}

/// Rename a group. Ownership is re-checked here, so callers may pass an id
/// straight from a request body.
pub fn update_group(
    conn: &Connection,
    id: i64,
    user_id: i64,
    input: &UpdateProjectGroup,
) -> Result<ProjectGroup, LificError> {
    get_owned_group(conn, id, user_id)?;
    if let Some(name) = &input.name {
        validate_name(name)?;
        let name = name.trim();
        conn.execute(
            "UPDATE project_groups SET name = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![name, id],
        )
        .map_err(constraint_err(name))?;
    }
    get_owned_group(conn, id, user_id)
}

/// Delete a group. Its memberships go with it through `ON DELETE CASCADE`;
/// the projects themselves are untouched and become ungrouped.
pub fn delete_group(conn: &Connection, id: i64, user_id: i64) -> Result<bool, LificError> {
    get_owned_group(conn, id, user_id)?;
    let changed = conn.execute("DELETE FROM project_groups WHERE id = ?1", params![id])?;
    Ok(changed > 0)
}

/// Move `project_id` into `group_id`, or out of every one of the caller's
/// groups when `group_id` is `None`. Both halves run in one SAVEPOINT, which
/// is what keeps a project from ever sitting in two of the same user's groups.
pub fn assign_project(
    conn: &Connection,
    user_id: i64,
    project_id: i64,
    group_id: Option<i64>,
) -> Result<(), LificError> {
    let target = group_id
        .map(|id| get_owned_group(conn, id, user_id))
        .transpose()?;
    super::savepoint(conn, "assign_project_group", || {
        conn.execute(
            "DELETE FROM project_group_items
             WHERE project_id = ?1
               AND group_id IN (SELECT id FROM project_groups WHERE user_id = ?2)",
            params![project_id, user_id],
        )?;
        if let Some(group) = &target {
            conn.execute(
                "INSERT INTO project_group_items (group_id, project_id) VALUES (?1, ?2)",
                params![group.id, project_id],
            )?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::CreateUser;
    use crate::db::{self, queries};

    fn test_db() -> db::DbPool {
        db::open_memory().expect("test db")
    }

    fn seed_user(conn: &Connection, username: &str) -> i64 {
        queries::users::create_user(
            conn,
            &CreateUser {
                username: username.into(),
                email: format!("{username}@test.local"),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap()
        .id
    }

    fn seed_project(conn: &Connection, ident: &str) -> i64 {
        queries::create_project(
            conn,
            &crate::db::models::CreateProject {
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
    fn created_group_comes_back_empty_and_named() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");

        let created =
            create_group(&conn, alice, &CreateProjectGroup { name: "Work".into() }).unwrap();
        assert_eq!(created.name, "Work");
        assert!(created.project_ids.is_empty());

        let listed = list_groups(&conn, alice).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, created.id);
    }

    #[test]
    fn duplicate_name_for_same_user_conflicts_but_another_user_may_reuse_it() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let bob = seed_user(&conn, "bob");

        create_group(&conn, alice, &CreateProjectGroup { name: "Work".into() }).unwrap();

        let err =
            create_group(&conn, alice, &CreateProjectGroup { name: "Work".into() }).unwrap_err();
        assert!(matches!(err, LificError::Conflict(_)), "got {err:?}");

        create_group(&conn, bob, &CreateProjectGroup { name: "Work".into() })
            .expect("a different user may reuse the name");
    }

    #[test]
    fn one_users_groups_are_invisible_to_another() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let bob = seed_user(&conn, "bob");

        create_group(&conn, alice, &CreateProjectGroup { name: "Personal".into() }).unwrap();

        assert!(list_groups(&conn, bob).unwrap().is_empty());
    }

    #[test]
    fn blank_name_is_rejected() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");

        let err =
            create_group(&conn, alice, &CreateProjectGroup { name: "   ".into() }).unwrap_err();
        assert!(matches!(err, LificError::BadRequest(_)), "got {err:?}");
    }

    // ── assignment ──────────────────────────────────────────────

    #[test]
    fn assigning_a_project_twice_moves_it_instead_of_duplicating_it() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let project = seed_project(&conn, "APP");
        let work =
            create_group(&conn, alice, &CreateProjectGroup { name: "Work".into() }).unwrap();
        let personal =
            create_group(&conn, alice, &CreateProjectGroup { name: "Personal".into() }).unwrap();

        assign_project(&conn, alice, project, Some(work.id)).unwrap();
        assign_project(&conn, alice, project, Some(personal.id)).unwrap();

        let groups = list_groups(&conn, alice).unwrap();
        let work_now = groups.iter().find(|g| g.id == work.id).unwrap();
        let personal_now = groups.iter().find(|g| g.id == personal.id).unwrap();
        assert!(
            work_now.project_ids.is_empty(),
            "project should have moved out of Work"
        );
        assert_eq!(personal_now.project_ids, vec![project]);
    }

    #[test]
    fn assigning_none_ungroups_the_project() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let project = seed_project(&conn, "APP");
        let work =
            create_group(&conn, alice, &CreateProjectGroup { name: "Work".into() }).unwrap();

        assign_project(&conn, alice, project, Some(work.id)).unwrap();
        assign_project(&conn, alice, project, None).unwrap();

        let groups = list_groups(&conn, alice).unwrap();
        assert!(groups[0].project_ids.is_empty());
    }

    #[test]
    fn two_users_may_file_the_same_project_differently() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let bob = seed_user(&conn, "bob");
        let project = seed_project(&conn, "APP");
        let alice_work =
            create_group(&conn, alice, &CreateProjectGroup { name: "Work".into() }).unwrap();
        let bob_side =
            create_group(&conn, bob, &CreateProjectGroup { name: "Side".into() }).unwrap();

        assign_project(&conn, alice, project, Some(alice_work.id)).unwrap();
        assign_project(&conn, bob, project, Some(bob_side.id)).unwrap();

        assert_eq!(
            list_groups(&conn, alice).unwrap()[0].project_ids,
            vec![project]
        );
        assert_eq!(
            list_groups(&conn, bob).unwrap()[0].project_ids,
            vec![project]
        );
    }

    #[test]
    fn assigning_into_another_users_group_is_not_found() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let bob = seed_user(&conn, "bob");
        let project = seed_project(&conn, "APP");
        let alice_work =
            create_group(&conn, alice, &CreateProjectGroup { name: "Work".into() }).unwrap();

        let err = assign_project(&conn, bob, project, Some(alice_work.id)).unwrap_err();
        assert!(matches!(err, LificError::NotFound(_)), "got {err:?}");
    }

    // ── delete and cascades ─────────────────────────────────────

    #[test]
    fn deleting_a_group_leaves_its_projects_intact_and_ungrouped() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let project = seed_project(&conn, "APP");
        let work =
            create_group(&conn, alice, &CreateProjectGroup { name: "Work".into() }).unwrap();
        assign_project(&conn, alice, project, Some(work.id)).unwrap();

        assert!(delete_group(&conn, work.id, alice).unwrap());

        assert!(list_groups(&conn, alice).unwrap().is_empty());
        queries::get_project(&conn, project).expect("project must survive its group");
    }

    #[test]
    fn deleting_another_users_group_is_not_found() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let bob = seed_user(&conn, "bob");
        let work =
            create_group(&conn, alice, &CreateProjectGroup { name: "Work".into() }).unwrap();

        let err = delete_group(&conn, work.id, bob).unwrap_err();
        assert!(matches!(err, LificError::NotFound(_)), "got {err:?}");
    }

    #[test]
    fn deleting_a_project_drops_its_memberships() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let project = seed_project(&conn, "APP");
        let work =
            create_group(&conn, alice, &CreateProjectGroup { name: "Work".into() }).unwrap();
        assign_project(&conn, alice, project, Some(work.id)).unwrap();

        queries::delete_project(&conn, project).unwrap();

        assert!(list_groups(&conn, alice).unwrap()[0].project_ids.is_empty());
    }

    #[test]
    fn deleting_a_user_cascades_their_groups_away() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        create_group(&conn, alice, &CreateProjectGroup { name: "Work".into() }).unwrap();

        conn.execute("DELETE FROM users WHERE id = ?1", params![alice])
            .unwrap();

        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM project_groups", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, 0);
    }

    // ── rename ──────────────────────────────────────────────────

    #[test]
    fn renaming_onto_an_existing_name_conflicts() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        create_group(&conn, alice, &CreateProjectGroup { name: "Work".into() }).unwrap();
        let personal =
            create_group(&conn, alice, &CreateProjectGroup { name: "Personal".into() }).unwrap();

        let err = update_group(
            &conn,
            personal.id,
            alice,
            &UpdateProjectGroup {
                name: Some("Work".into()),
            },
        )
        .unwrap_err();
        assert!(matches!(err, LificError::Conflict(_)), "got {err:?}");
    }

    #[test]
    fn renaming_keeps_the_groups_membership() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let project = seed_project(&conn, "APP");
        let work =
            create_group(&conn, alice, &CreateProjectGroup { name: "Work".into() }).unwrap();
        assign_project(&conn, alice, project, Some(work.id)).unwrap();

        let renamed = update_group(
            &conn,
            work.id,
            alice,
            &UpdateProjectGroup {
                name: Some("Day job".into()),
            },
        )
        .unwrap();
        assert_eq!(renamed.name, "Day job");
        assert_eq!(list_groups(&conn, alice).unwrap()[0].project_ids, vec![project]);
    }
}
