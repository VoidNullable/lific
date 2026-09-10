use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, params};

use crate::db::models::*;
use crate::error::LificError;

use super::unescape_text;

/// Per-project workload signals shown only in the MCP project listing.
///
/// This deliberately lives beside project queries rather than on `Project`:
/// REST and web callers keep their existing project payload and ordering.
#[derive(Debug, Clone, Default)]
pub struct ProjectAgentStats {
    pub workable: i64,
    pub active_plans: i64,
    pub last_activity: Option<String>,
}

pub fn list_projects(conn: &Connection) -> Result<Vec<Project>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, name, identifier, description, emoji, lead_user_id, sort_order, created_at, updated_at, is_public
         FROM projects ORDER BY sort_order, name, id",
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
            is_public: row.get(9)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// Stored personal ranks precede projects the user has not ordered yet.
/// This is unfiltered, like `list_projects`; callers must apply visibility.
pub fn list_projects_for_user(conn: &Connection, user_id: i64) -> Result<Vec<Project>, LificError> {
    let mut projects = list_projects(conn)?;
    let mut stmt = conn.prepare_cached(
        "SELECT project_id, sort_order FROM user_project_order WHERE user_id = ?1",
    )?;
    let ranks: HashMap<i64, i64> = stmt
        .query_map([user_id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    // Stable sorting keeps the legacy (rank, name, id) order within ties and
    // among newly visible projects. Expose the personal rank in REST results.
    projects.sort_by_key(|p| match ranks.get(&p.id) {
        Some(rank) => (false, *rank),
        None => (true, 0),
    });
    for project in &mut projects {
        if let Some(rank) = ranks.get(&project.id) {
            project.sort_order = *rank;
        }
    }
    Ok(projects)
}

/// Fetch workload signals for every project in one SQL statement.
///
/// The workable predicate intentionally mirrors `issues::list_issues` exactly:
/// a blocker is unresolved until its source issue is `done` (a cancelled
/// blocker therefore continues to block, matching the existing list filter).
pub fn project_agent_stats(
    conn: &Connection,
) -> Result<HashMap<i64, ProjectAgentStats>, LificError> {
    let mut stmt = conn.prepare_cached(
        "WITH workable AS (
             SELECT i.project_id, COUNT(*) AS count
             FROM issues i
             WHERE i.status NOT IN ('done', 'cancelled')
               AND i.deleted_at IS NULL
               AND NOT EXISTS (
                   SELECT 1 FROM issue_relations ir
                   JOIN issues blocker ON blocker.id = ir.source_id
                   WHERE ir.target_id = i.id
                     AND ir.relation_type = 'blocks'
                     AND blocker.status != 'done'
                     AND blocker.deleted_at IS NULL
               )
             GROUP BY i.project_id
         ),
         active_plans AS (
             SELECT project_id, COUNT(*) AS count
             FROM plans
             WHERE status = 'active'
             GROUP BY project_id
         ),
         activity AS (
             SELECT project_id, updated_at FROM issues WHERE deleted_at IS NULL
             UNION ALL
             SELECT project_id, updated_at FROM pages
              WHERE project_id IS NOT NULL AND deleted_at IS NULL
             UNION ALL
             SELECT project_id, updated_at FROM plans
         ),
         last_activity AS (
             SELECT project_id, MAX(updated_at) AS updated_at
             FROM activity
             GROUP BY project_id
         )
         SELECT p.id,
                COALESCE(w.count, 0),
                COALESCE(ap.count, 0),
                la.updated_at
         FROM projects p
         LEFT JOIN workable w ON w.project_id = p.id
         LEFT JOIN active_plans ap ON ap.project_id = p.id
         LEFT JOIN last_activity la ON la.project_id = p.id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            ProjectAgentStats {
                workable: row.get(1)?,
                active_plans: row.get(2)?,
                last_activity: row.get(3)?,
            },
        ))
    })?;
    Ok(rows.collect::<Result<HashMap<_, _>, _>>()?)
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
        "SELECT id, name, identifier, description, emoji, lead_user_id, sort_order, created_at, updated_at, is_public
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
                is_public: row.get(9)?,
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
pub(crate) fn validate_identifier(identifier: &str) -> Result<(), LificError> {
    if identifier.is_empty() {
        return Err(LificError::BadRequest(
            "identifier must not be empty".into(),
        ));
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
    // LIF-409: the row and its lead membership are one fact, so they are one
    // savepoint. A rejected membership (an unknown lead id trips the foreign
    // key) used to leave a leaderless project behind that only an admin could
    // then reach.
    super::savepoint(conn, "create_project", || {
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
        let id = conn.last_insert_rowid();
        // LIF-195: keep project_members in sync with the denormalized lead
        // pointer — a project created with a lead gets a 'lead' membership row.
        if let Some(lead_id) = input.lead_user_id {
            super::members::upsert_member(conn, id, lead_id, Role::Lead)?;
        }
        // LIF-409: hydrated inside the savepoint, so a failed final read rolls
        // the project and its membership back together.
        get_project(conn, id)
    })
}

/// Snapshot the complete visible order, with submitted IDs ahead of omitted
/// projects in their current order. The caller must resolve `visible` inside
/// the same transaction as this operation. No legacy project ranks change.
/// A snapshot drops hidden ranks; access regained after that appends the project.
pub fn reorder_projects(
    conn: &Connection,
    user_id: i64,
    ids: &[i64],
    visible: &Option<HashSet<i64>>,
) -> Result<Vec<Project>, LificError> {
    super::savepoint(conn, "reorder_projects", || {
        let current: Vec<Project> = list_projects_for_user(conn, user_id)?
            .into_iter()
            .filter(|p| visible.as_ref().is_none_or(|v| v.contains(&p.id)))
            .collect();
        let allowed: HashSet<i64> = current.iter().map(|p| p.id).collect();
        let mut seen = HashSet::new();
        for id in ids {
            if !seen.insert(*id) {
                return Err(LificError::BadRequest(
                    "duplicate project id in reorder list".into(),
                ));
            }
            if !allowed.contains(id) {
                return Err(LificError::BadRequest(
                    "invalid project id in reorder list".into(),
                ));
            }
        }
        let order = ids
            .iter()
            .copied()
            .chain(current.iter().map(|p| p.id).filter(|id| !seen.contains(id)));
        conn.execute(
            "DELETE FROM user_project_order WHERE user_id = ?1",
            [user_id],
        )?;
        for (rank, id) in order.enumerate() {
            conn.execute(
                "INSERT INTO user_project_order (user_id, project_id, sort_order) VALUES (?1, ?2, ?3)",
                params![user_id, id, rank as i64],
            )?;
        }
        Ok(list_projects_for_user(conn, user_id)?
            .into_iter()
            .filter(|p| visible.as_ref().is_none_or(|v| v.contains(&p.id)))
            .collect())
    })
}

pub fn update_project(
    conn: &Connection,
    id: i64,
    input: &UpdateProject,
) -> Result<Project, LificError> {
    get_project(conn, id)?;
    super::savepoint::<_, Project>(conn, "update_project", || {
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
                let exists =
                    match conn.query_row("SELECT 1 FROM users WHERE id = ?1", params![uid], |_| {
                        Ok(true)
                    }) {
                        Ok(_) => true,
                        Err(rusqlite::Error::QueryReturnedNoRows) => false,
                        Err(e) => return Err(e.into()),
                    };
                if !exists {
                    return Err(LificError::BadRequest(format!("user {uid} not found")));
                }
            }
            conn.execute(
                "UPDATE projects SET lead_user_id = ?1 WHERE id = ?2",
                params![lead, id],
            )?;
            // LIF-195: upsert a 'lead' membership for the new lead. The old
            // lead (if any) keeps their existing membership row — this is
            // additive, not a swap.
            if let Some(uid) = lead {
                super::members::upsert_member(conn, id, uid, Role::Lead)?;
            }
        }
        // A separate update fires the publication audit trigger only when set.
        if let Some(is_public) = input.is_public {
            conn.execute(
                "UPDATE projects SET is_public = ?1 WHERE id = ?2",
                params![is_public, id],
            )?;
        }
        // LIF-409: hydrated inside the savepoint. See `create_project`.
        get_project(conn, id)
    })
}

pub fn delete_project(conn: &Connection, id: i64) -> Result<(), LificError> {
    let changed = conn.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
    if changed == 0 {
        return Err(LificError::NotFound(format!("project {id} not found")));
    }
    Ok(())
}

pub fn delete_project_with_audience(
    conn: &Connection,
    id: i64,
) -> Result<(Project, Option<Vec<i64>>), LificError> {
    let project = get_project(conn, id)?;
    let audience = if super::settings::get(conn)?.authz_enforced {
        Some(project_viewer_ids(conn, &project)?)
    } else {
        None
    };
    delete_project(conn, id)?;
    Ok((project, audience))
}

fn project_viewer_ids(conn: &Connection, project: &Project) -> Result<Vec<i64>, LificError> {
    let mut ids: Vec<_> = super::members::list_members(conn, project.id)?
        .into_iter()
        .map(|member| member.user_id)
        .collect();
    if let Some(lead_id) = project.lead_user_id
        && !ids.contains(&lead_id)
    {
        ids.push(lead_id);
    }
    let mut stmt = conn.prepare("SELECT id FROM users WHERE is_admin = 1")?;
    let admins = stmt
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    admins.into_iter().for_each(|id| {
        if !ids.contains(&id) {
            ids.push(id);
        }
    });
    Ok(ids)
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
                ..Default::default()
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
                ..Default::default()
            },
        )
        .unwrap();

        let id = resolve_project_identifier(&conn, "LIF").unwrap();
        assert!(id > 0);
    }

    /// LIF-348: `projects.identifier` carries `COLLATE NOCASE` (migration
    /// 039), so a bare `identifier = ?` is case-insensitive everywhere —
    /// no per-callsite COLLATE, and the same fix reaches issue and page
    /// identifier resolution, which resolve their project half the same way.
    #[test]
    fn resolve_project_identifier_is_case_insensitive() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = create_project(
            &conn,
            &CreateProject {
                name: "Lific".into(),
                identifier: "LIF".into(),
                ..Default::default()
            },
        )
        .unwrap();

        for spelling in ["LIF", "lif", "Lif", "lIF"] {
            assert_eq!(
                resolve_project_identifier(&conn, spelling)
                    .unwrap_or_else(|e| panic!("{spelling} should resolve: {e}")),
                project.id
            );
        }

        // Still whole-string matching: a prefix is not a match.
        assert!(resolve_project_identifier(&conn, "li").is_err());
    }

    /// LIF-348: the NOCASE unique index rejects a case-variant duplicate.
    /// `validate_identifier` gets there first for a lowercase spelling (it
    /// only accepts uppercase), so both layers are asserted: the API-level
    /// rejection, and the constraint itself via a raw INSERT that bypasses
    /// validation the way a legacy row or a manual edit would.
    #[test]
    fn case_variant_identifier_is_rejected() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        create_project(
            &conn,
            &CreateProject {
                name: "Upper".into(),
                identifier: "ABC".into(),
                ..Default::default()
            },
        )
        .unwrap();

        let result = try_create(&conn, "abc");
        assert!(result.is_err(), "got: {result:?}");

        let raw = conn.execute(
            "INSERT INTO projects (name, identifier) VALUES ('Lower', 'abc')",
            [],
        );
        assert!(
            raw.is_err(),
            "NOCASE unique index must reject 'abc' alongside 'ABC'"
        );
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
                ..Default::default()
            },
        )
        .unwrap();

        let result = create_project(
            &conn,
            &CreateProject {
                name: "Second".into(),
                identifier: "DUP".into(),
                ..Default::default()
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
                ..Default::default()
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
                    identifier: Some(bad.into()),
                    ..Default::default()
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
                ..Default::default()
            },
        )
        .unwrap();

        let updated = update_project(
            &conn,
            project.id,
            &UpdateProject {
                name: Some("New Name".into()),
                description: Some("Now with description".into()),
                ..Default::default()
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
                ..Default::default()
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
                    ..Default::default()
                },
            )
            .unwrap();
        }

        let projects = list_projects(&conn).unwrap();
        assert_eq!(projects.len(), 3);
    }

    #[test]
    fn project_agent_stats_counts_work_and_latest_activity() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project_id = seed_named(&conn, "Signals", "SIG");

        let insert_issue = |sequence, title, status, updated_at| {
            conn.execute(
                "INSERT INTO issues (project_id, sequence, title, status, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![project_id, sequence, title, status, updated_at],
            )
            .unwrap();
            conn.last_insert_rowid()
        };
        let blocker = insert_issue(1, "Blocker", "active", "2026-01-01 00:00:00");
        insert_issue(2, "Unblocked", "todo", "2026-01-02 00:00:00");
        let blocked = insert_issue(3, "Blocked", "todo", "2026-01-03 00:00:00");
        insert_issue(4, "Done", "done", "2026-01-04 00:00:00");
        conn.execute(
            "INSERT INTO issue_relations (source_id, target_id, relation_type)
             VALUES (?1, ?2, 'blocks')",
            params![blocker, blocked],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO pages (project_id, sequence, title, status, updated_at)
             VALUES (?1, 1, 'Page activity', 'active', '2026-01-05 00:00:00')",
            params![project_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO plans (project_id, sequence, title, status, updated_at)
             VALUES (?1, 1, 'Active plan', 'active', '2026-01-06 00:00:00')",
            params![project_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO plans (project_id, sequence, title, status, updated_at)
             VALUES (?1, 2, 'Archived plan', 'archived', '2026-01-04 12:00:00')",
            params![project_id],
        )
        .unwrap();

        let stats = project_agent_stats(&conn).unwrap();
        let stats = stats.get(&project_id).expect("stats for project");
        assert_eq!(stats.workable, 2);
        assert_eq!(stats.active_plans, 1);
        assert_eq!(stats.last_activity.as_deref(), Some("2026-01-06 00:00:00"));
    }

    // ── LIF-233: sidebar ordering ────────────────────────────

    fn seed_named(conn: &Connection, name: &str, ident: &str) -> i64 {
        create_project(
            conn,
            &CreateProject {
                name: name.into(),
                identifier: ident.into(),
                ..Default::default()
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
        conn.execute("UPDATE projects SET sort_order = 0", [])
            .unwrap();

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
        let user = seed_user(&conn, "ordering");
        let a = seed_named(&conn, "Alpha", "A");
        let b = seed_named(&conn, "Beta", "B");
        let g = seed_named(&conn, "Gamma", "G");

        let reordered = reorder_projects(&conn, user, &[g, a, b], &None).unwrap();
        let names: Vec<String> = reordered.into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["Gamma", "Alpha", "Beta"]);

        // Order persists across a fresh list (sort_order, not query happenstance).
        let names: Vec<String> = list_projects_for_user(&conn, user)
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
        let user = seed_user(&conn, "ordering");
        let a = seed_named(&conn, "Alpha", "A");
        let err = reorder_projects(&conn, user, &[a, 99999], &None).unwrap_err();
        assert!(matches!(err, LificError::BadRequest(_)), "got {err:?}");
        // The failed reorder is rolled back: Alpha keeps its original rank.
        assert_eq!(list_projects(&conn).unwrap()[0].name, "Alpha");
    }

    #[test]
    fn reorder_rejects_duplicate_id() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let user = seed_user(&conn, "ordering");
        let a = seed_named(&conn, "Alpha", "A");
        let b = seed_named(&conn, "Beta", "B");
        let err = reorder_projects(&conn, user, &[a, b, a], &None).unwrap_err();
        assert!(matches!(err, LificError::BadRequest(_)), "got {err:?}");
    }

    #[test]
    fn new_project_appends_after_reorder() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let user = seed_user(&conn, "ordering");
        let a = seed_named(&conn, "Alpha", "A");
        let b = seed_named(&conn, "Beta", "B");
        // Reorder so Beta(rank 0) precedes Alpha(rank 1).
        reorder_projects(&conn, user, &[b, a], &None).unwrap();
        // A brand-new project should land at the bottom, not jump to rank 0.
        seed_named(&conn, "Zeta", "Z");
        let names: Vec<String> = list_projects_for_user(&conn, user)
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names, ["Beta", "Alpha", "Zeta"]);
    }

    #[test]
    fn personal_order_snapshots_visible_projects_and_preserves_other_users_and_legacy_ranks() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice");
        let bob = seed_user(&conn, "bob");
        let a = seed_named(&conn, "Alpha", "A");
        let b = seed_named(&conn, "Beta", "B");
        let c = seed_named(&conn, "Charlie", "C");
        let hidden = seed_named(&conn, "Hidden", "H");
        let visible = Some(HashSet::from([a, b, c]));
        let ids = |projects: Vec<Project>| projects.into_iter().map(|p| p.id).collect::<Vec<_>>();
        let legacy = list_projects(&conn).unwrap();
        assert_eq!(
            ids(reorder_projects(&conn, alice, &[c], &visible).unwrap()),
            [c, a, b]
        );
        assert_eq!(
            ids(reorder_projects(&conn, alice, &[b], &visible).unwrap()),
            [b, c, a]
        );
        assert_eq!(
            ids(reorder_projects(&conn, alice, &[], &visible).unwrap()),
            [b, c, a]
        );
        assert_eq!(
            ids(list_projects_for_user(&conn, bob).unwrap()),
            [a, b, c, hidden]
        );
        assert_eq!(
            list_projects(&conn)
                .unwrap()
                .iter()
                .map(|p| p.sort_order)
                .collect::<Vec<_>>(),
            legacy.iter().map(|p| p.sort_order).collect::<Vec<_>>()
        );
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM user_project_order WHERE user_id = ?1",
                [alice],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 3);
        // Newly granted access appends even when the legacy rank would put it first.
        conn.execute(
            "UPDATE projects SET sort_order = -1 WHERE id = ?1",
            [hidden],
        )
        .unwrap();
        assert_eq!(
            ids(list_projects_for_user(&conn, alice).unwrap()),
            [b, c, a, hidden]
        );
        let invalid = reorder_projects(&conn, alice, &[a, hidden], &visible).unwrap_err();
        let unknown = reorder_projects(&conn, alice, &[a, 99999], &visible).unwrap_err();
        assert_eq!(invalid.to_string(), unknown.to_string());
        assert_eq!(
            ids(list_projects_for_user(&conn, alice).unwrap()),
            [b, c, a, hidden]
        );
        reorder_projects(&conn, bob, &[hidden], &None).unwrap();
        reorder_projects(&conn, alice, &[a], &visible).unwrap();
        assert_eq!(
            ids(list_projects_for_user(&conn, bob).unwrap()),
            [hidden, a, b, c]
        );
        assert_eq!(
            ids(list_projects_for_user(&conn, alice).unwrap()),
            [a, b, c, hidden]
        );
    }

    #[test]
    fn personal_order_rolls_back_a_database_failure_after_snapshot_deletion() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let user = seed_user(&conn, "ordering");
        let a = seed_named(&conn, "Alpha", "A");
        let b = seed_named(&conn, "Beta", "B");
        reorder_projects(&conn, user, &[b, a], &None).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER reject_order_insert BEFORE INSERT ON user_project_order
            WHEN NEW.sort_order = 1 BEGIN SELECT RAISE(ABORT, 'test failure'); END;",
        )
        .unwrap();
        assert!(reorder_projects(&conn, user, &[a, b], &None).is_err());
        let ids: Vec<_> = list_projects_for_user(&conn, user)
            .unwrap()
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(ids, [b, a]);
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
                ..Default::default()
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
                emoji: Some("🧪".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(project.emoji.as_deref(), Some("🧪"));

        let updated = update_project(
            &conn,
            project.id,
            &UpdateProject {
                emoji: Some(None), // explicit clear
                ..Default::default()
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
                lead_user_id: Some(uid),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(project.lead_user_id, Some(uid));

        let updated = update_project(
            &conn,
            project.id,
            &UpdateProject {
                lead_user_id: Some(None), // explicit clear
                ..Default::default()
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
                emoji: Some("🎯".into()),
                lead_user_id: Some(uid),
                ..Default::default()
            },
        )
        .unwrap();

        // Update unrelated field; emoji + lead should survive.
        let updated = update_project(
            &conn,
            project.id,
            &UpdateProject {
                // emoji and lead_user_id are left absent (None) by the
                // default, which must preserve the existing values.
                name: Some("Keep Renamed".into()),
                ..Default::default()
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
                ..Default::default()
            },
        )
        .unwrap();

        // 99999 doesn't exist. Should be a BadRequest, not a Database error.
        let result = update_project(
            &conn,
            project.id,
            &UpdateProject {
                lead_user_id: Some(Some(99999)),
                ..Default::default()
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
