//! LIF-195: project membership — the (user, role) source of truth for
//! project-scoped authorization (epic LIF-194).
//!
//! Pure data access, no enforcement: nothing here checks whether the caller
//! is *allowed* to do this. `projects.lead_user_id` stays the denormalized
//! "primary lead" pointer read by today's LIF-102 access check; the write
//! paths that set it (`queries::projects::create_project` /
//! `update_project`) call [`upsert_member`] to keep both in sync.

use rusqlite::{params, Connection, OptionalExtension};

use crate::db::models::{MemberWithUser, ProjectMember, Role};
use crate::error::LificError;

/// List a project's members, oldest membership first.
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

/// LIF-199: `list_members` joined with `users` for display — powers
/// `GET /api/projects/{id}/members`, the only caller that needs a name to
/// render alongside the role.
pub fn list_members_with_users(
    conn: &Connection,
    project_id: i64,
) -> Result<Vec<MemberWithUser>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT m.project_id, m.user_id, m.role, m.created_at, u.username, u.display_name
         FROM project_members m
         JOIN users u ON u.id = m.user_id
         WHERE m.project_id = ?1
         ORDER BY m.created_at, m.user_id",
    )?;
    let rows = stmt.query_map(params![project_id], |row| {
        Ok(MemberWithUser {
            project_id: row.get(0)?,
            user_id: row.get(1)?,
            role: row.get(2)?,
            created_at: row.get(3)?,
            username: row.get(4)?,
            display_name: row.get(5)?,
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

/// Remove a member from a project. Pure delete — no last-lead guard, no
/// `lead_user_id` bookkeeping. Callers that need those (the REST endpoint)
/// use [`remove_member_guarded`] instead; this stays exposed for tests and
/// any future caller that genuinely wants an unguarded delete.
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

/// Count how many `lead` members a project currently has. Backs the
/// last-lead guard in [`change_role`] / [`remove_member_guarded`] (LIF-199).
pub fn count_leads(conn: &Connection, project_id: i64) -> Result<i64, LificError> {
    conn.query_row(
        "SELECT COUNT(*) FROM project_members WHERE project_id = ?1 AND role = 'lead'",
        params![project_id],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

// ── Write endpoints (LIF-199) ───────────────────────────────────
//
// The three functions below back `POST` / `PATCH` / `DELETE
// /api/projects/{id}/members[/{user_id}]` (`api::members`). Unlike the
// bare CRUD helpers above, these carry the guard rails the REST surface
// needs: strict (non-upsert) add semantics, the last-lead protection, and
// `projects.lead_user_id` pointer upkeep. No authorization check lives
// here — callers gate with `authz::require_role(.., Role::Lead)` first;
// this module is pure data access plus invariants, same as the rest of
// the file.

/// Add a brand-new member. Strict, not upsert: an existing `(project_id,
/// user_id)` row is a 409 `Conflict`, not a silent role overwrite — `PATCH`
/// is the endpoint for changing an existing member's role. Validates the
/// target user exists first so a bad `user_id` reads as 404, not a raw FK
/// constraint failure.
///
/// `role` is a raw string (the caller's `"viewer"` default, or whatever the
/// client sent) rather than a pre-parsed [`Role`]: unlike `axum::Json<T>`
/// deserializing straight into the `Role` enum (which axum would 422 on a
/// bad value, not the 400 this API contracts for), parsing here goes
/// through the normal [`LificError::BadRequest`] path — the same
/// validate-in-the-query-layer convention `projects::validate_identifier`
/// uses.
pub fn add_member(
    conn: &Connection,
    project_id: i64,
    user_id: i64,
    role: &str,
) -> Result<ProjectMember, LificError> {
    let role: Role = role.parse().map_err(LificError::BadRequest)?;

    crate::db::queries::users::get_user_by_id(conn, user_id)?;

    if get_member_role(conn, project_id, user_id)?.is_some() {
        return Err(LificError::Conflict(format!(
            "user {user_id} is already a member of project {project_id}"
        )));
    }

    upsert_member(conn, project_id, user_id, role)
}

/// Change an existing member's role. 404s if they aren't a member at all
/// (PATCH edits an existing row, it doesn't create one — that's `POST`'s
/// job). Blocks demoting the project's sole `lead` (409 `Conflict`): a
/// project must always keep at least one lead once it has any, matching
/// the last-lead guard in [`remove_member_guarded`]. Re-affirming an
/// existing lead's role (`new_role == Lead`) is never a demotion and is
/// always allowed.
///
/// Deliberately does **not** touch `projects.lead_user_id` — a demotion
/// away from `lead` can only happen here when a *second* lead exists (the
/// guard blocks the sole-lead case), so the denormalized pointer, if it
/// still names the demoted user, simply keeps pointing at someone who is
/// no longer a `lead`-role member. This is a narrower version of the same
/// looseness `update_project`'s `lead_user_id` write path already has
/// (LIF-195: "old lead keeps their membership row — memberships are
/// additive"); [`remove_member_guarded`] is the path that actively repairs
/// the pointer, because removal (unlike demotion) can leave it dangling at
/// a user who is no longer a member at all.
///
/// `new_role` is a raw string for the same reason as [`add_member`]'s
/// `role` param — a bad value must 400, not 422.
pub fn change_role(
    conn: &Connection,
    project_id: i64,
    user_id: i64,
    new_role: &str,
) -> Result<ProjectMember, LificError> {
    let new_role: Role = new_role.parse().map_err(LificError::BadRequest)?;

    let current = get_member_role(conn, project_id, user_id)?.ok_or_else(|| {
        LificError::NotFound(format!("user {user_id} is not a member of project {project_id}"))
    })?;

    if current == Role::Lead && new_role != Role::Lead && count_leads(conn, project_id)? <= 1 {
        return Err(LificError::Conflict(
            "cannot demote the project's last lead — promote another member to lead first".into(),
        ));
    }

    upsert_member(conn, project_id, user_id, new_role)
}

/// Remove a member, with the same last-lead guard as [`change_role`] (409
/// `Conflict` when the target is the project's sole lead), and then repairs
/// `projects.lead_user_id` if the removed user was the denormalized primary
/// lead: reassigned to the remaining lead with the lowest `user_id`
/// (deterministic), or cleared to `NULL` if none remain (matches the
/// existing "unowned project" carve-out — `require_lead_legacy` already
/// handles `lead_user_id IS NULL` as admin-only). The reassignment check is
/// unconditional on the removed member's role (not just `Lead`): it's a
/// pointer-hygiene fix-up for whoever `lead_user_id` names, independent of
/// their current `project_members` role, which also self-heals the rare
/// case where a `PATCH` demotion (see [`change_role`]'s doc comment) left
/// the pointer aimed at a non-lead member who is now being removed outright.
pub fn remove_member_guarded(
    conn: &Connection,
    project_id: i64,
    user_id: i64,
) -> Result<(), LificError> {
    let current = get_member_role(conn, project_id, user_id)?.ok_or_else(|| {
        LificError::NotFound(format!("user {user_id} is not a member of project {project_id}"))
    })?;

    if current == Role::Lead && count_leads(conn, project_id)? <= 1 {
        return Err(LificError::Conflict(
            "cannot remove the project's last lead — promote another member to lead first".into(),
        ));
    }

    remove_member(conn, project_id, user_id)?;

    let project = super::get_project(conn, project_id)?;
    if project.lead_user_id == Some(user_id) {
        let new_lead: Option<i64> = conn.query_row(
            "SELECT MIN(user_id) FROM project_members WHERE project_id = ?1 AND role = 'lead'",
            params![project_id],
            |row| row.get(0),
        )?;
        conn.execute(
            "UPDATE projects SET lead_user_id = ?1 WHERE id = ?2",
            params![new_lead, project_id],
        )?;
    }

    Ok(())
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
                ..Default::default()
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
                lead_user_id: Some(alice),
                ..Default::default()
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
                ..Default::default()
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
                lead_user_id: Some(alice),
                ..Default::default()
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
                lead_user_id: Some(Some(bob)),
                ..Default::default()
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
                lead_user_id: Some(alice),
                ..Default::default()
            },
        )
        .unwrap();

        let updated = projects::update_project(
            &conn,
            project.id,
            &UpdateProject {
                lead_user_id: Some(None), // explicit clear
                ..Default::default()
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

    // ── Write endpoints (LIF-199) ────────────────────────────

    #[test]
    fn add_member_defaults_and_rejects_duplicate() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project_id = seed_project(&conn, "ADD");
        let alice = seed_user(&conn, "alice");

        let member = add_member(&conn, project_id, alice, "viewer").unwrap();
        assert_eq!(member.role, Role::Viewer);

        let err = add_member(&conn, project_id, alice, "maintainer").unwrap_err();
        assert!(matches!(err, LificError::Conflict(_)), "got {err:?}");
        // The conflicting call must not have overwritten the role.
        assert_eq!(get_member_role(&conn, project_id, alice).unwrap(), Some(Role::Viewer));
    }

    #[test]
    fn add_member_unknown_user_is_not_found() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project_id = seed_project(&conn, "ADU");

        let err = add_member(&conn, project_id, 99999, "viewer").unwrap_err();
        assert!(matches!(err, LificError::NotFound(_)), "got {err:?}");
    }

    #[test]
    fn add_member_bad_role_is_bad_request() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project_id = seed_project(&conn, "ADR");
        let alice = seed_user(&conn, "alice");

        let err = add_member(&conn, project_id, alice, "owner").unwrap_err();
        assert!(matches!(err, LificError::BadRequest(_)), "got {err:?}");
        assert_eq!(get_member_role(&conn, project_id, alice).unwrap(), None);
    }

    #[test]
    fn change_role_requires_existing_membership() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project_id = seed_project(&conn, "CHR");
        let alice = seed_user(&conn, "alice");

        let err = change_role(&conn, project_id, alice, "maintainer").unwrap_err();
        assert!(matches!(err, LificError::NotFound(_)), "got {err:?}");
    }

    #[test]
    fn change_role_bad_role_is_bad_request() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project_id = seed_project(&conn, "CHB");
        let alice = seed_user(&conn, "alice");
        upsert_member(&conn, project_id, alice, Role::Viewer).unwrap();

        let err = change_role(&conn, project_id, alice, "owner").unwrap_err();
        assert!(matches!(err, LificError::BadRequest(_)), "got {err:?}");
    }

    #[test]
    fn change_role_blocks_demoting_the_sole_lead() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project_id = seed_project(&conn, "SOL");
        let alice = seed_user(&conn, "alice");
        upsert_member(&conn, project_id, alice, Role::Lead).unwrap();

        let err = change_role(&conn, project_id, alice, "maintainer").unwrap_err();
        assert!(matches!(err, LificError::Conflict(_)), "got {err:?}");
        assert_eq!(get_member_role(&conn, project_id, alice).unwrap(), Some(Role::Lead));

        // Re-affirming the same role is not a demotion — always fine.
        assert!(change_role(&conn, project_id, alice, "lead").is_ok());
    }

    #[test]
    fn change_role_allows_demotion_once_a_second_lead_exists() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project_id = seed_project(&conn, "TWL");
        let alice = seed_user(&conn, "alice");
        let bob = seed_user(&conn, "bob");
        upsert_member(&conn, project_id, alice, Role::Lead).unwrap();
        upsert_member(&conn, project_id, bob, Role::Lead).unwrap();

        change_role(&conn, project_id, alice, "maintainer").unwrap();
        assert_eq!(get_member_role(&conn, project_id, alice).unwrap(), Some(Role::Maintainer));
    }

    #[test]
    fn remove_member_guarded_blocks_the_sole_lead() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project_id = seed_project(&conn, "RSL");
        let alice = seed_user(&conn, "alice");
        upsert_member(&conn, project_id, alice, Role::Lead).unwrap();

        let err = remove_member_guarded(&conn, project_id, alice).unwrap_err();
        assert!(matches!(err, LificError::Conflict(_)), "got {err:?}");
        assert_eq!(get_member_role(&conn, project_id, alice).unwrap(), Some(Role::Lead));
    }

    #[test]
    fn remove_member_guarded_allows_non_lead_on_a_project_with_no_lead() {
        // The "backfilled with no lead_user_id" carve-out: a project with
        // zero lead rows must not block removal of its (non-lead) members.
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project_id = seed_project(&conn, "NLD"); // unowned, no lead rows at all
        let alice = seed_user(&conn, "alice");
        upsert_member(&conn, project_id, alice, Role::Viewer).unwrap();

        remove_member_guarded(&conn, project_id, alice).unwrap();
        assert_eq!(get_member_role(&conn, project_id, alice).unwrap(), None);
    }

    #[test]
    fn remove_member_guarded_reassigns_lead_user_id_to_lowest_remaining_lead() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let bob = seed_user(&conn, "bob");
        let carol = seed_user(&conn, "carol");
        // alice is the primary (lead_user_id) lead; bob and carol are co-leads.
        let project = projects::create_project(
            &conn,
            &CreateProject {
                name: "Reassign".into(),
                identifier: "RAS".into(),
                lead_user_id: Some(alice),
                ..Default::default()
            },
        )
        .unwrap();
        upsert_member(&conn, project.id, bob, Role::Lead).unwrap();
        upsert_member(&conn, project.id, carol, Role::Lead).unwrap();
        assert!(bob < carol, "test assumes bob has the lower id");

        remove_member_guarded(&conn, project.id, alice).unwrap();

        let updated = projects::get_project(&conn, project.id).unwrap();
        assert_eq!(
            updated.lead_user_id,
            Some(bob),
            "pointer must move to the remaining lead with the lowest user_id"
        );
    }

    #[test]
    fn remove_member_guarded_clears_lead_user_id_when_no_leads_remain() {
        // Removing the sole lead is normally blocked, but if lead_user_id
        // points at a member who is no longer lead-role (e.g. via a prior
        // PATCH demotion — see change_role's doc comment) and no leads
        // remain at all, removal must still null out the dangling pointer
        // rather than leave it referencing a non-member after deletion.
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let project = projects::create_project(
            &conn,
            &CreateProject {
                name: "Dangling".into(),
                identifier: "DNG".into(),
                lead_user_id: Some(alice),
                ..Default::default()
            },
        )
        .unwrap();
        // Demote alice out of the lead role directly (bypassing change_role's
        // guard, which would otherwise correctly refuse this as the sole lead —
        // simulating the desync change_role's doc comment describes).
        upsert_member(&conn, project.id, alice, Role::Maintainer).unwrap();

        remove_member_guarded(&conn, project.id, alice).unwrap();

        let updated = projects::get_project(&conn, project.id).unwrap();
        assert_eq!(updated.lead_user_id, None);
    }

    #[test]
    fn remove_member_guarded_leaves_lead_user_id_untouched_for_unrelated_removal() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let bob = seed_user(&conn, "bob");
        let project = projects::create_project(
            &conn,
            &CreateProject {
                name: "Untouched".into(),
                identifier: "UNT".into(),
                lead_user_id: Some(alice),
                ..Default::default()
            },
        )
        .unwrap();
        upsert_member(&conn, project.id, bob, Role::Viewer).unwrap();

        remove_member_guarded(&conn, project.id, bob).unwrap();

        let updated = projects::get_project(&conn, project.id).unwrap();
        assert_eq!(updated.lead_user_id, Some(alice), "unrelated removal must not touch the pointer");
    }
}
