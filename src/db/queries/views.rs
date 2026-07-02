//! LIF-242: saved views — named filter/group/sort presets per project,
//! personal to each user. Pure data access plus the invariants the REST
//! surface (`api::views`) needs: config validation (size + JSON
//! well-formedness, never schema), default-uniqueness upkeep, and strict
//! per-user ownership. No project-role authorization lives here — callers
//! gate with `authz::require_role(.., Role::Viewer)` first (see
//! `api::views`'s doc comment for why Viewer, not Maintainer: saving a
//! *personal* view is a read-side convenience, not a project mutation).
//!
//! ## Ownership model
//!
//! Every read/write below that targets an existing row (`update_view`,
//! `delete_view`) goes through [`get_owned_view`], which folds "doesn't
//! exist" and "exists but belongs to someone else" into the same
//! `NotFound` — deliberately indistinguishable, so a view id can't be used
//! to probe whether another user has a view with that id at all. `list_views`
//! doesn't need this: it only ever queries by `(project_id, user_id)`, so
//! there's no id to leak through.

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::models::{CreateSavedView, SavedView, UpdateSavedView};
use crate::error::LificError;

/// Config payload size cap. Generous for any realistic filter/sort/group
/// preset (a handful of short strings + a lane/density enum) while still
/// bounding what a malicious or buggy client can shove into a TEXT column
/// with no schema validation.
const MAX_CONFIG_BYTES: usize = 8 * 1024;

/// Validate the opaque `config` payload: must be within the size cap and
/// must parse as JSON. Deliberately does **not** look at the JSON's shape —
/// that's the frontend's `ViewConfig` contract, which can evolve without a
/// backend migration.
fn validate_config(config: &str) -> Result<(), LificError> {
    if config.len() > MAX_CONFIG_BYTES {
        return Err(LificError::BadRequest(format!(
            "config exceeds the {MAX_CONFIG_BYTES}-byte limit"
        )));
    }
    if serde_json::from_str::<serde_json::Value>(config).is_err() {
        return Err(LificError::BadRequest("config must be valid JSON".into()));
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), LificError> {
    if name.trim().is_empty() {
        return Err(LificError::BadRequest("name must not be empty".into()));
    }
    Ok(())
}

/// List the caller's own saved views for a project, alphabetically. Never
/// returns another user's views — the `user_id` filter is baked into the
/// query, not applied after the fact.
pub fn list_views(
    conn: &Connection,
    project_id: i64,
    user_id: i64,
) -> Result<Vec<SavedView>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, project_id, user_id, name, config, is_default, created_at, updated_at
         FROM saved_views WHERE project_id = ?1 AND user_id = ?2
         ORDER BY name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map(params![project_id, user_id], row_to_view)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn get_view_row(conn: &Connection, id: i64) -> Result<SavedView, LificError> {
    conn.query_row(
        "SELECT id, project_id, user_id, name, config, is_default, created_at, updated_at
         FROM saved_views WHERE id = ?1",
        params![id],
        row_to_view,
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            LificError::NotFound(format!("saved view {id} not found"))
        }
        other => other.into(),
    })
}

/// Fetch a view by id, scoped to the expected project and owning user.
/// Returns `NotFound` (never `Forbidden`) when the view doesn't exist, or
/// exists on a different project, or belongs to a different user — see the
/// module doc comment for why these collapse to one outcome.
pub fn get_owned_view(
    conn: &Connection,
    id: i64,
    project_id: i64,
    user_id: i64,
) -> Result<SavedView, LificError> {
    let view = get_view_row(conn, id)?;
    if view.user_id != user_id || view.project_id != project_id {
        return Err(LificError::NotFound(format!("saved view {id} not found")));
    }
    Ok(view)
}

fn constraint_err(name: &str) -> impl Fn(rusqlite::Error) -> LificError + '_ {
    move |e| match e {
        rusqlite::Error::SqliteFailure(err, _)
            if err.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            LificError::Conflict(format!("a view named '{name}' already exists"))
        }
        other => other.into(),
    }
}

/// Create a new saved view. `is_default: true` atomically clears any
/// existing default the same user holds on the same project first (inside
/// one SAVEPOINT), so default-ness stays a single row per (project, user)
/// without a partial unique index. A duplicate `(project_id, user_id,
/// name)` is a 409 `Conflict`, not a silent overwrite.
pub fn create_view(
    conn: &Connection,
    project_id: i64,
    user_id: i64,
    input: &CreateSavedView,
) -> Result<SavedView, LificError> {
    validate_name(&input.name)?;
    validate_config(&input.config)?;

    super::savepoint(conn, "create_saved_view", || {
        if input.is_default {
            clear_other_defaults(conn, project_id, user_id, None)?;
        }
        conn.execute(
            "INSERT INTO saved_views (project_id, user_id, name, config, is_default)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![project_id, user_id, input.name, input.config, input.is_default],
        )
        .map_err(constraint_err(&input.name))?;
        get_view_row(conn, conn.last_insert_rowid())
    })
}

/// Update an existing view: rename, replace the config, and/or (un)set the
/// default flag. Only owner-writable — callers must resolve `id` through
/// [`get_owned_view`] first if they need a pre-check, but this function also
/// re-checks ownership itself so it's safe to call directly.
pub fn update_view(
    conn: &Connection,
    id: i64,
    project_id: i64,
    user_id: i64,
    input: &UpdateSavedView,
) -> Result<SavedView, LificError> {
    let existing = get_owned_view(conn, id, project_id, user_id)?;

    if let Some(name) = &input.name {
        validate_name(name)?;
    }
    if let Some(config) = &input.config {
        validate_config(config)?;
    }

    super::savepoint(conn, "update_saved_view", || {
        if input.is_default == Some(true) {
            clear_other_defaults(conn, project_id, user_id, Some(id))?;
        }

        let name = input.name.as_deref().unwrap_or(&existing.name);
        let config = input.config.as_deref().unwrap_or(&existing.config);
        let is_default = input.is_default.unwrap_or(existing.is_default);

        conn.execute(
            "UPDATE saved_views SET name = ?1, config = ?2, is_default = ?3,
             updated_at = datetime('now') WHERE id = ?4",
            params![name, config, is_default, id],
        )
        .map_err(constraint_err(name))?;

        get_view_row(conn, id)
    })
}

/// Delete a view. 404s (via [`get_owned_view`]) when it doesn't exist,
/// belongs to a different project, or belongs to a different user.
pub fn delete_view(
    conn: &Connection,
    id: i64,
    project_id: i64,
    user_id: i64,
) -> Result<(), LificError> {
    get_owned_view(conn, id, project_id, user_id)?;
    conn.execute("DELETE FROM saved_views WHERE id = ?1", params![id])?;
    Ok(())
}

/// Clear `is_default` on every other view the same user holds on the same
/// project. `except_id` skips the row being updated in place (so
/// re-affirming an already-default view's other fields doesn't need a
/// clear-then-set round trip that would momentarily leave zero defaults).
fn clear_other_defaults(
    conn: &Connection,
    project_id: i64,
    user_id: i64,
    except_id: Option<i64>,
) -> Result<(), LificError> {
    match except_id {
        Some(id) => conn.execute(
            "UPDATE saved_views SET is_default = 0
             WHERE project_id = ?1 AND user_id = ?2 AND id != ?3 AND is_default = 1",
            params![project_id, user_id, id],
        ),
        None => conn.execute(
            "UPDATE saved_views SET is_default = 0
             WHERE project_id = ?1 AND user_id = ?2 AND is_default = 1",
            params![project_id, user_id],
        ),
    }?;
    Ok(())
}

/// Look up a user's default view for a project, if any. Used by the
/// frontend's "auto-apply my default view on project load" flow (LIF-242) —
/// exposed here mainly so that flow can be exercised without a full
/// `list_views` round trip, though today's REST surface just filters
/// client-side; kept `#[allow(dead_code)]`-free by being reachable from
/// tests and any future dedicated endpoint.
#[allow(dead_code)]
pub fn get_default_view(
    conn: &Connection,
    project_id: i64,
    user_id: i64,
) -> Result<Option<SavedView>, LificError> {
    conn.query_row(
        "SELECT id, project_id, user_id, name, config, is_default, created_at, updated_at
         FROM saved_views WHERE project_id = ?1 AND user_id = ?2 AND is_default = 1",
        params![project_id, user_id],
        row_to_view,
    )
    .optional()
    .map_err(Into::into)
}

fn row_to_view(row: &rusqlite::Row) -> Result<SavedView, rusqlite::Error> {
    Ok(SavedView {
        id: row.get(0)?,
        project_id: row.get(1)?,
        user_id: row.get(2)?,
        name: row.get(3)?,
        config: row.get(4)?,
        is_default: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{CreateProject, CreateUser};
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

    fn view_input(name: &str, is_default: bool) -> CreateSavedView {
        CreateSavedView {
            name: name.into(),
            config: r#"{"groupBy":"status","sortField":"priority"}"#.into(),
            is_default,
        }
    }

    // ── CRUD round-trip ─────────────────────────────────────────

    #[test]
    fn create_and_list_views() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "VWS");
        let alice = seed_user(&conn, "alice");

        let created = create_view(&conn, project, alice, &view_input("My view", false)).unwrap();
        assert_eq!(created.name, "My view");
        assert!(!created.is_default);
        assert_eq!(created.project_id, project);
        assert_eq!(created.user_id, alice);

        let listed = list_views(&conn, project, alice).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, created.id);
    }

    #[test]
    fn list_views_only_returns_own_views() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "OWN");
        let alice = seed_user(&conn, "alice");
        let bob = seed_user(&conn, "bob");

        create_view(&conn, project, alice, &view_input("Alice's", false)).unwrap();
        create_view(&conn, project, bob, &view_input("Bob's", false)).unwrap();

        let alice_views = list_views(&conn, project, alice).unwrap();
        assert_eq!(alice_views.len(), 1);
        assert_eq!(alice_views[0].name, "Alice's");

        let bob_views = list_views(&conn, project, bob).unwrap();
        assert_eq!(bob_views.len(), 1);
        assert_eq!(bob_views[0].name, "Bob's");
    }

    #[test]
    fn update_renames_and_replaces_config() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "UPD");
        let alice = seed_user(&conn, "alice");
        let created = create_view(&conn, project, alice, &view_input("Original", false)).unwrap();

        let updated = update_view(
            &conn,
            created.id,
            project,
            alice,
            &UpdateSavedView {
                name: Some("Renamed".into()),
                config: Some(r#"{"groupBy":"module"}"#.into()),
                is_default: None,
            },
        )
        .unwrap();

        assert_eq!(updated.name, "Renamed");
        assert_eq!(updated.config, r#"{"groupBy":"module"}"#);
        assert_eq!(updated.id, created.id);
    }

    #[test]
    fn update_partial_leaves_other_fields_alone() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "PRT");
        let alice = seed_user(&conn, "alice");
        let created = create_view(&conn, project, alice, &view_input("Keep me", false)).unwrap();

        let updated = update_view(
            &conn,
            created.id,
            project,
            alice,
            &UpdateSavedView { name: None, config: Some(r#"{"a":1}"#.into()), is_default: None },
        )
        .unwrap();

        assert_eq!(updated.name, "Keep me", "name must be untouched");
        assert_eq!(updated.config, r#"{"a":1}"#);
    }

    #[test]
    fn delete_removes_the_view() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "DEL");
        let alice = seed_user(&conn, "alice");
        let created = create_view(&conn, project, alice, &view_input("Bye", false)).unwrap();

        delete_view(&conn, created.id, project, alice).unwrap();

        assert!(list_views(&conn, project, alice).unwrap().is_empty());
    }

    // ── Ownership isolation (LIF-242) ────────────────────────────

    #[test]
    fn other_user_cannot_get_update_or_delete_a_view_they_dont_own() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "ISO");
        let alice = seed_user(&conn, "alice");
        let bob = seed_user(&conn, "bob");
        let alice_view = create_view(&conn, project, alice, &view_input("Alice's", false)).unwrap();

        let err = get_owned_view(&conn, alice_view.id, project, bob).unwrap_err();
        assert!(matches!(err, LificError::NotFound(_)), "got {err:?}");

        let err = update_view(
            &conn,
            alice_view.id,
            project,
            bob,
            &UpdateSavedView { name: Some("Hijacked".into()), config: None, is_default: None },
        )
        .unwrap_err();
        assert!(matches!(err, LificError::NotFound(_)), "got {err:?}");

        let err = delete_view(&conn, alice_view.id, project, bob).unwrap_err();
        assert!(matches!(err, LificError::NotFound(_)), "got {err:?}");

        // Untouched: alice's view survives every rejected attempt.
        let still_there = list_views(&conn, project, alice).unwrap();
        assert_eq!(still_there.len(), 1);
        assert_eq!(still_there[0].name, "Alice's");
    }

    #[test]
    fn view_id_from_a_different_project_is_not_found() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project_a = seed_project(&conn, "PJA");
        let project_b = seed_project(&conn, "PJB");
        let alice = seed_user(&conn, "alice");
        let view = create_view(&conn, project_a, alice, &view_input("A's view", false)).unwrap();

        let err = get_owned_view(&conn, view.id, project_b, alice).unwrap_err();
        assert!(matches!(err, LificError::NotFound(_)), "got {err:?}");
    }

    // ── Default uniqueness ────────────────────────────────────────

    #[test]
    fn setting_a_new_default_clears_the_previous_one() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "DFT");
        let alice = seed_user(&conn, "alice");
        let first = create_view(&conn, project, alice, &view_input("First", true)).unwrap();
        assert!(first.is_default);

        let second = create_view(&conn, project, alice, &view_input("Second", true)).unwrap();
        assert!(second.is_default);

        let refreshed_first = get_view_row(&conn, first.id).unwrap();
        assert!(!refreshed_first.is_default, "creating a new default must clear the old one");

        let defaults: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM saved_views WHERE project_id = ?1 AND user_id = ?2 AND is_default = 1",
                params![project, alice],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(defaults, 1);
    }

    #[test]
    fn update_can_promote_a_non_default_view_to_default() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "PRM");
        let alice = seed_user(&conn, "alice");
        let a = create_view(&conn, project, alice, &view_input("A", true)).unwrap();
        let b = create_view(&conn, project, alice, &view_input("B", false)).unwrap();
        assert!(a.is_default);
        assert!(!b.is_default);

        let promoted = update_view(
            &conn,
            b.id,
            project,
            alice,
            &UpdateSavedView { name: None, config: None, is_default: Some(true) },
        )
        .unwrap();
        assert!(promoted.is_default);

        let refreshed_a = get_view_row(&conn, a.id).unwrap();
        assert!(!refreshed_a.is_default);
    }

    #[test]
    fn defaults_are_independent_per_user() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "IND");
        let alice = seed_user(&conn, "alice");
        let bob = seed_user(&conn, "bob");

        let alice_default = create_view(&conn, project, alice, &view_input("Alice default", true)).unwrap();
        let bob_default = create_view(&conn, project, bob, &view_input("Bob default", true)).unwrap();

        assert!(alice_default.is_default);
        assert!(bob_default.is_default, "bob's default must not be cleared by alice's");
    }

    #[test]
    fn get_default_view_returns_none_when_no_default_set() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "NDF");
        let alice = seed_user(&conn, "alice");
        create_view(&conn, project, alice, &view_input("Not default", false)).unwrap();

        assert!(get_default_view(&conn, project, alice).unwrap().is_none());
    }

    #[test]
    fn get_default_view_returns_the_default() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "GDV");
        let alice = seed_user(&conn, "alice");
        let created = create_view(&conn, project, alice, &view_input("Default", true)).unwrap();

        let found = get_default_view(&conn, project, alice).unwrap().unwrap();
        assert_eq!(found.id, created.id);
    }

    // ── Validation ──────────────────────────────────────────────

    #[test]
    fn create_rejects_invalid_json_config() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "BJS");
        let alice = seed_user(&conn, "alice");

        let err = create_view(
            &conn,
            project,
            alice,
            &CreateSavedView { name: "Bad".into(), config: "{not json".into(), is_default: false },
        )
        .unwrap_err();
        assert!(matches!(err, LificError::BadRequest(_)), "got {err:?}");
    }

    #[test]
    fn create_rejects_oversized_config() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "BIG");
        let alice = seed_user(&conn, "alice");

        let huge = format!(r#"{{"padding":"{}"}}"#, "x".repeat(MAX_CONFIG_BYTES));
        let err = create_view(
            &conn,
            project,
            alice,
            &CreateSavedView { name: "Too big".into(), config: huge, is_default: false },
        )
        .unwrap_err();
        assert!(matches!(err, LificError::BadRequest(_)), "got {err:?}");
    }

    #[test]
    fn create_rejects_empty_name() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "ENM");
        let alice = seed_user(&conn, "alice");

        let err = create_view(&conn, project, alice, &view_input("   ", false)).unwrap_err();
        assert!(matches!(err, LificError::BadRequest(_)), "got {err:?}");
    }

    #[test]
    fn create_accepts_any_valid_json_shape_without_schema_validation() {
        // The backend deliberately does not enforce a schema on `config` —
        // an array, a bare number, whatever — only "is it valid JSON".
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "SHP");
        let alice = seed_user(&conn, "alice");

        for config in ["[1,2,3]", "42", "\"just a string\"", "null", "{}"] {
            let name = format!("shape-{config}");
            let result = create_view(
                &conn,
                project,
                alice,
                &CreateSavedView { name, config: config.into(), is_default: false },
            );
            assert!(result.is_ok(), "expected {config} to be accepted, got {result:?}");
        }
    }

    // ── Name uniqueness per (project, user) ────────────────────

    #[test]
    fn duplicate_name_for_same_user_and_project_is_conflict() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "DUP");
        let alice = seed_user(&conn, "alice");
        create_view(&conn, project, alice, &view_input("Mine", false)).unwrap();

        let err = create_view(&conn, project, alice, &view_input("Mine", false)).unwrap_err();
        assert!(matches!(err, LificError::Conflict(_)), "got {err:?}");
    }

    #[test]
    fn same_name_allowed_for_different_users_or_different_projects() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project_a = seed_project(&conn, "SNA");
        let project_b = seed_project(&conn, "SNB");
        let alice = seed_user(&conn, "alice");
        let bob = seed_user(&conn, "bob");

        create_view(&conn, project_a, alice, &view_input("Shared name", false)).unwrap();
        // Different user, same project, same name: fine.
        assert!(create_view(&conn, project_a, bob, &view_input("Shared name", false)).is_ok());
        // Same user, different project, same name: fine.
        assert!(create_view(&conn, project_b, alice, &view_input("Shared name", false)).is_ok());
    }

    // ── Not-found ───────────────────────────────────────────────

    #[test]
    fn update_and_delete_nonexistent_view_are_not_found() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "NF1");
        let alice = seed_user(&conn, "alice");

        let err = update_view(
            &conn,
            999999,
            project,
            alice,
            &UpdateSavedView { name: Some("x".into()), config: None, is_default: None },
        )
        .unwrap_err();
        assert!(matches!(err, LificError::NotFound(_)), "got {err:?}");

        let err = delete_view(&conn, 999999, project, alice).unwrap_err();
        assert!(matches!(err, LificError::NotFound(_)), "got {err:?}");
    }

    // ── Cascade deletes ────────────────────────────────────────

    #[test]
    fn cascade_delete_on_project_removes_views() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "CDP");
        let alice = seed_user(&conn, "alice");
        create_view(&conn, project, alice, &view_input("Gone with project", false)).unwrap();

        queries::delete_project(&conn, project).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM saved_views WHERE project_id = ?1", params![project], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn cascade_delete_on_user_removes_views() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_project(&conn, "CDU");
        let alice = seed_user(&conn, "alice");
        create_view(&conn, project, alice, &view_input("Gone with user", false)).unwrap();

        conn.execute("DELETE FROM users WHERE id = ?1", params![alice]).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM saved_views WHERE user_id = ?1", params![alice], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
}
