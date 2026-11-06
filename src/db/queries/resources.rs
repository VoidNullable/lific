use rusqlite::{Connection, OptionalExtension, params};

use crate::db::models::*;
use crate::error::LificError;

use super::unescape_text;

/// A table [`get_resource_project_id`] can resolve a row in.
///
/// LIF-386: the table name is interpolated into the SQL, because SQLite has
/// no bind parameter for an identifier. The set of legal names is therefore
/// an enum rather than a string: a caller cannot express an injection in the
/// first place, so there is nothing to validate at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceTable {
    Modules,
    Labels,
    Folders,
}

impl ResourceTable {
    /// The SQL identifier for this table. Every value is a literal written
    /// right here, which is what makes the interpolation below safe.
    pub fn as_table(&self) -> &'static str {
        match self {
            ResourceTable::Modules => "modules",
            ResourceTable::Labels => "labels",
            ResourceTable::Folders => "folders",
        }
    }

    /// The word for one row, as error messages name it.
    pub fn singular(&self) -> &'static str {
        match self {
            ResourceTable::Modules => "module",
            ResourceTable::Labels => "label",
            ResourceTable::Folders => "folder",
        }
    }
}

/// Look up the project_id for a module, label, or folder by its id.
pub fn get_resource_project_id(
    conn: &Connection,
    table: ResourceTable,
    id: i64,
) -> Result<i64, LificError> {
    let table = table.as_table();
    let sql = format!("SELECT project_id FROM {table} WHERE id = ?1");
    conn.query_row(&sql, params![id], |row| row.get(0))
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                LificError::NotFound(format!("{table} {id} not found"))
            }
            _ => e.into(),
        })
}

/// How [`find_resource_by_name`] compares a name with the stored ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameMatch {
    /// Byte for byte.
    Exact,
    /// Ignoring case (Unicode lowercase on both sides).
    AnyCase,
}

/// Find a module, label, or folder in a project by name.
///
/// LIF-477: names are not unique by case (`Web` and `web` can both exist),
/// so callers try [`NameMatch::Exact`] first and fall back to
/// [`NameMatch::AnyCase`]. A caseless match that lands on two different
/// stored names is a `BadRequest` naming both, never a guess. Rows sharing
/// one exact name resolve to the oldest, as an exact lookup always has.
pub fn find_resource_by_name(
    conn: &Connection,
    table: ResourceTable,
    project_id: i64,
    name: &str,
    matching: NameMatch,
) -> Result<Option<i64>, LificError> {
    let sql_table = table.as_table();
    if matching == NameMatch::Exact {
        let sql =
            format!("SELECT id FROM {sql_table} WHERE project_id = ?1 AND name = ?2 ORDER BY id");
        return Ok(conn
            .prepare_cached(&sql)?
            .query_row(params![project_id, name], |row| row.get(0))
            .optional()?);
    }
    let wanted = name.to_lowercase();
    let matches: Vec<(i64, String)> = resource_names(conn, table, project_id)?
        .into_iter()
        .filter(|(_, stored)| stored.to_lowercase() == wanted)
        .collect();
    let Some((first_id, _)) = matches.first() else {
        return Ok(None);
    };
    let mut distinct: Vec<&str> = matches.iter().map(|(_, stored)| stored.as_str()).collect();
    distinct.dedup();
    if distinct.len() > 1 {
        return Err(LificError::BadRequest(format!(
            "{} '{name}' matches more than one {} by case ({}); use the exact name",
            table.singular(),
            table.singular(),
            quoted_list(&distinct),
        )));
    }
    Ok(Some(*first_id))
}

/// Every `(id, name)` in the project's table, ordered by name then id.
fn resource_names(
    conn: &Connection,
    table: ResourceTable,
    project_id: i64,
) -> Result<Vec<(i64, String)>, LificError> {
    let sql = format!(
        "SELECT id, name FROM {} WHERE project_id = ?1 ORDER BY name, id",
        table.as_table()
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(params![project_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// How many valid names a not-found error lists before summarizing the rest.
const SUGGESTED_NAMES: usize = 10;

/// LIF-477: the error for a name that matched nothing. It lists the
/// project's names, closest first, so a caller can retry without first
/// spending a call on listing them.
pub fn resource_name_not_found(
    conn: &Connection,
    table: ResourceTable,
    project_id: i64,
    name: &str,
) -> LificError {
    let singular = table.singular();
    let names = match resource_names(conn, table, project_id) {
        Ok(rows) => rows.into_iter().map(|(_, stored)| stored).collect(),
        Err(error) => return error,
    };
    let ranked = closest_names_first(name, names);
    let hint = match ranked.len() {
        0 => format!("the project has no {}", table.as_table()),
        total => {
            let shown: Vec<&str> = ranked
                .iter()
                .take(SUGGESTED_NAMES)
                .map(String::as_str)
                .collect();
            let more = total - shown.len();
            let tail = if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            };
            format!("valid {}: {}{tail}", table.as_table(), quoted_list(&shown))
        }
    };
    LificError::NotFound(format!("{singular} '{name}' not found in project; {hint}"))
}

/// Distinct names ordered by likeness to `wanted`: names that contain it or
/// that it contains first, then by edit distance, then alphabetically.
fn closest_names_first(wanted: &str, mut names: Vec<String>) -> Vec<String> {
    let key = wanted.to_lowercase();
    names.sort();
    names.dedup();
    let mut ranked: Vec<(bool, usize, String, String)> = names
        .into_iter()
        .map(|stored| {
            let lower = stored.to_lowercase();
            let unrelated = !(lower.contains(&key) || key.contains(&lower));
            (unrelated, levenshtein(&key, &lower), lower, stored)
        })
        .collect();
    ranked.sort();
    ranked.into_iter().map(|(_, _, _, stored)| stored).collect()
}

fn quoted_list(names: &[&str]) -> String {
    names
        .iter()
        .map(|stored| format!("'{stored}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut current = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != *cb);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        previous = current;
    }
    previous[b.len()]
}

/// Resolve a name exactly, then ignoring case; a miss lists the valid names.
fn resolve_resource_name(
    conn: &Connection,
    table: ResourceTable,
    project_id: i64,
    name: &str,
) -> Result<i64, LificError> {
    for matching in [NameMatch::Exact, NameMatch::AnyCase] {
        if let Some(id) = find_resource_by_name(conn, table, project_id, name, matching)? {
            return Ok(id);
        }
    }
    Err(resource_name_not_found(conn, table, project_id, name))
}

pub fn resolve_module_name(
    conn: &Connection,
    project_id: i64,
    name: &str,
) -> Result<i64, LificError> {
    resolve_resource_name(conn, ResourceTable::Modules, project_id, name)
}

pub fn resolve_folder_name(
    conn: &Connection,
    project_id: i64,
    name: &str,
) -> Result<i64, LificError> {
    resolve_resource_name(conn, ResourceTable::Folders, project_id, name)
}

pub fn resolve_label_name(
    conn: &Connection,
    project_id: i64,
    name: &str,
) -> Result<i64, LificError> {
    resolve_resource_name(conn, ResourceTable::Labels, project_id, name)
}

pub fn get_module_name(conn: &Connection, id: i64) -> Result<String, LificError> {
    conn.prepare_cached("SELECT name FROM modules WHERE id = ?1")?
        .query_row(params![id], |row| row.get(0))
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                LificError::NotFound(format!("module {id} not found"))
            }
            _ => e.into(),
        })
}

/// Folder id → name. Used by MCP page output to surface the folder a
/// page lives in without callers juggling a `list_folders` round-trip.
pub fn get_folder_name(conn: &Connection, id: i64) -> Result<String, LificError> {
    conn.query_row(
        "SELECT name FROM folders WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            LificError::NotFound(format!("folder {id} not found"))
        }
        _ => e.into(),
    })
}

/// Fetch a single module by id. Used by the web detail route and any
/// client that already knows the id but not the project — list_modules
/// requires the project_id up front, which makes URL→data resolution
/// awkward when you don't have it in hand.
pub fn get_module(conn: &Connection, id: i64) -> Result<Module, LificError> {
    conn.query_row(
        "SELECT id, project_id, name, description, status, emoji, created_at, updated_at
         FROM modules WHERE id = ?1",
        params![id],
        |row| {
            Ok(Module {
                id: row.get(0)?,
                project_id: row.get(1)?,
                name: row.get(2)?,
                description: row.get(3)?,
                status: row.get(4)?,
                emoji: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        },
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            LificError::NotFound(format!("module {id} not found"))
        }
        _ => e.into(),
    })
}

pub fn list_modules(conn: &Connection, project_id: i64) -> Result<Vec<Module>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, project_id, name, description, status, emoji, created_at, updated_at
         FROM modules WHERE project_id = ?1 ORDER BY name",
    )?;
    let rows = stmt.query_map(params![project_id], |row| {
        Ok(Module {
            id: row.get(0)?,
            project_id: row.get(1)?,
            name: row.get(2)?,
            description: row.get(3)?,
            status: row.get(4)?,
            emoji: row.get(5)?,
            created_at: row.get(6)?,
            updated_at: row.get(7)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// LIF-397: module status is stored as TEXT, but only these six values are
/// documented, shown by the UI, and promised by the MCP `manage_resource`
/// schema. Reject anything else at the query boundary so REST, MCP, and the
/// CLI all get the same contract (mirrors how issue status became a real
/// type in 286d4ac; modules keep the lighter String representation because
/// nothing branches on module status server-side).
const MODULE_STATUSES: [&str; 6] = [
    "backlog",
    "planned",
    "active",
    "paused",
    "done",
    "cancelled",
];

fn validate_module_status(status: &str) -> Result<(), LificError> {
    if MODULE_STATUSES.contains(&status) {
        Ok(())
    } else {
        Err(LificError::BadRequest(format!(
            "invalid module status '{status}'. Use backlog, planned, active, paused, done, or cancelled."
        )))
    }
}

pub fn create_module(conn: &Connection, input: &CreateModule) -> Result<Module, LificError> {
    validate_module_status(&input.status)?;
    conn.execute(
        "INSERT INTO modules (project_id, name, description, status, emoji) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            input.project_id,
            input.name,
            unescape_text(&input.description),
            input.status,
            input.emoji,
        ],
    )?;
    let id = conn.last_insert_rowid();
    conn.query_row(
        "SELECT id, project_id, name, description, status, emoji, created_at, updated_at FROM modules WHERE id = ?1",
        params![id],
        |row| Ok(Module {
            id: row.get(0)?, project_id: row.get(1)?, name: row.get(2)?,
            description: row.get(3)?, status: row.get(4)?, emoji: row.get(5)?,
            created_at: row.get(6)?, updated_at: row.get(7)?,
        }),
    ).map_err(Into::into)
}

pub fn update_module(
    conn: &Connection,
    id: i64,
    input: &UpdateModule,
) -> Result<Module, LificError> {
    super::savepoint(conn, "update_module", || {
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
            validate_module_status(status)?;
            conn.execute(
                "UPDATE modules SET status = ?1 WHERE id = ?2",
                params![status, id],
            )?;
        }
        // Tristate: Some(None) clears to NULL, Some(Some) sets, None skips.
        if let Some(emoji) = &input.emoji {
            conn.execute(
                "UPDATE modules SET emoji = ?1 WHERE id = ?2",
                params![emoji.as_ref(), id],
            )?;
        }
        Ok(())
    })?;
    conn.query_row(
        "SELECT id, project_id, name, description, status, emoji, created_at, updated_at FROM modules WHERE id = ?1",
        params![id],
        |row| Ok(Module {
            id: row.get(0)?, project_id: row.get(1)?, name: row.get(2)?,
            description: row.get(3)?, status: row.get(4)?, emoji: row.get(5)?,
            created_at: row.get(6)?, updated_at: row.get(7)?,
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
    let mut stmt = conn.prepare_cached(
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
    super::savepoint(conn, "update_label", || {
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
        Ok(())
    })?;
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

/// Merge `source_id` into `target_id`: every issue/page wearing the source
/// label is re-pointed at the target (deduped via INSERT OR IGNORE so an
/// object already carrying both doesn't violate the composite PK), then the
/// source label is deleted. Atomic via savepoint. Returns the surviving
/// target label. The attach/detach triggers fire naturally, so the audit log
/// records the reassignment and issue `updated_at` bumps as usual.
pub fn merge_label(conn: &Connection, source_id: i64, target_id: i64) -> Result<Label, LificError> {
    if source_id == target_id {
        return Err(LificError::BadRequest(
            "cannot merge a label into itself".into(),
        ));
    }
    super::savepoint(conn, "merge_label", || {
        conn.execute(
            "INSERT OR IGNORE INTO issue_labels (issue_id, label_id)
             SELECT issue_id, ?1 FROM issue_labels WHERE label_id = ?2",
            params![target_id, source_id],
        )?;
        conn.execute(
            "DELETE FROM issue_labels WHERE label_id = ?1",
            params![source_id],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO page_labels (page_id, label_id)
             SELECT page_id, ?1 FROM page_labels WHERE label_id = ?2",
            params![target_id, source_id],
        )?;
        conn.execute(
            "DELETE FROM page_labels WHERE label_id = ?1",
            params![source_id],
        )?;
        let changed = conn.execute("DELETE FROM labels WHERE id = ?1", params![source_id])?;
        if changed == 0 {
            return Err(LificError::NotFound(format!("label {source_id} not found")));
        }
        Ok(())
    })?;
    conn.query_row(
        "SELECT id, project_id, name, color FROM labels WHERE id = ?1",
        params![target_id],
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
            LificError::NotFound(format!("label {target_id} not found"))
        }
        _ => e.into(),
    })
}

pub fn list_folders(conn: &Connection, project_id: i64) -> Result<Vec<Folder>, LificError> {
    let mut stmt = conn.prepare_cached(
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
    if let Some(parent_id) = input.parent_id {
        let parent_project_id: Option<i64> = conn
            .query_row(
                "SELECT project_id FROM folders WHERE id = ?1",
                params![parent_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(parent_project_id) = parent_project_id
            && parent_project_id != input.project_id
        {
            return Err(LificError::BadRequest(format!(
                "parent folder {parent_id} belongs to project {parent_project_id}, not project {}",
                input.project_id
            )));
        }
    }
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
                ..Default::default()
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
                emoji: None,
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
                status: Some("done".into()),
                ..Default::default()
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

    /// LIF-397: docs and the MCP schema promise six module statuses, but the
    /// column is TEXT and used to take anything ("bananas" included). The six
    /// documented values round-trip; everything else is a BadRequest naming
    /// them, on both create and update, and a rejected update leaves the row
    /// untouched.
    #[test]
    fn module_status_rejects_undocumented_values() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn);

        let err = create_module(
            &conn,
            &CreateModule {
                project_id: pid,
                name: "Bad".into(),
                description: String::new(),
                status: "bananas".into(),
                emoji: None,
            },
        )
        .unwrap_err();
        assert!(matches!(err, LificError::BadRequest(_)), "got: {err:?}");
        assert!(
            err.to_string().contains("paused"),
            "error must list the six: {err}"
        );

        let module = create_module(
            &conn,
            &CreateModule {
                project_id: pid,
                name: "Guarded".into(),
                description: String::new(),
                status: "backlog".into(),
                emoji: None,
            },
        )
        .unwrap();
        for status in [
            "backlog",
            "planned",
            "active",
            "paused",
            "done",
            "cancelled",
        ] {
            update_module(
                &conn,
                module.id,
                &UpdateModule {
                    status: Some(status.into()),
                    ..Default::default()
                },
            )
            .unwrap();
        }

        let err = update_module(
            &conn,
            module.id,
            &UpdateModule {
                status: Some("shipped".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(matches!(err, LificError::BadRequest(_)), "got: {err:?}");
        // The rejected update must not have altered the stored status.
        assert_eq!(get_module(&conn, module.id).unwrap().status, "cancelled");
    }

    #[test]
    fn get_module_by_id_round_trip() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn);

        let created = create_module(
            &conn,
            &CreateModule {
                project_id: pid,
                name: "Auth".into(),
                description: "Login + tokens".into(),
                status: "planned".into(),
                emoji: None,
            },
        )
        .unwrap();

        let fetched = get_module(&conn, created.id).unwrap();
        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.name, "Auth");
        assert_eq!(fetched.description, "Login + tokens");
        assert_eq!(fetched.status, "planned");
        assert_eq!(fetched.project_id, pid);
    }

    #[test]
    fn get_module_not_found_returns_404_kind() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let err = get_module(&conn, 99_999).unwrap_err();
        assert!(matches!(err, LificError::NotFound(_)));
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
                emoji: None,
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
                emoji: None,
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
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(updated.name, "defect");
        assert_eq!(updated.color, "#EF4444"); // unchanged

        let updated = update_label(
            &conn,
            label.id,
            &UpdateLabel {
                color: Some("#FF0000".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(updated.name, "defect"); // unchanged
        assert_eq!(updated.color, "#FF0000");
    }

    #[test]
    fn merge_label_moves_attachments_and_dedups() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn);
        let bug = create_label(
            &conn,
            &CreateLabel {
                project_id: pid,
                name: "bug".into(),
                color: "#EF4444".into(),
            },
        )
        .unwrap();
        let defect = create_label(
            &conn,
            &CreateLabel {
                project_id: pid,
                name: "defect".into(),
                color: "#F97316".into(),
            },
        )
        .unwrap();

        // Issue A carries only the source label.
        let a = crate::db::queries::create_issue(
            &conn,
            &CreateIssue {
                project_id: pid,
                title: "A".into(),
                labels: vec!["bug".into()],
                ..Default::default()
            },
        )
        .unwrap();
        // Issue B carries BOTH — the merge must dedup, not double-insert.
        let b = crate::db::queries::create_issue(
            &conn,
            &CreateIssue {
                project_id: pid,
                title: "B".into(),
                labels: vec!["bug".into(), "defect".into()],
                ..Default::default()
            },
        )
        .unwrap();

        let survivor = merge_label(&conn, bug.id, defect.id).unwrap();
        assert_eq!(survivor.id, defect.id);

        // Source label is gone; only the target remains.
        let labels = list_labels(&conn, pid).unwrap();
        assert_eq!(labels.len(), 1);
        assert_eq!(labels[0].name, "defect");

        // A was re-pointed; B kept exactly one copy of the target.
        let a2 = crate::db::queries::get_issue(&conn, a.id).unwrap();
        assert_eq!(a2.labels, vec!["defect".to_string()]);
        let b2 = crate::db::queries::get_issue(&conn, b.id).unwrap();
        assert_eq!(b2.labels, vec!["defect".to_string()]);
    }

    #[test]
    fn merge_label_into_itself_errors() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn);
        let l = create_label(
            &conn,
            &CreateLabel {
                project_id: pid,
                name: "bug".into(),
                color: "#EF4444".into(),
            },
        )
        .unwrap();
        let err = merge_label(&conn, l.id, l.id).unwrap_err();
        assert!(matches!(err, LificError::BadRequest(_)));
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
    fn create_folder_rejects_parent_from_another_project() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project_id = seed_project(&conn);
        let other_project_id = projects::create_project(
            &conn,
            &CreateProject {
                name: "Other".into(),
                identifier: "OTH".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id;
        let parent = create_folder(
            &conn,
            &CreateFolder {
                project_id: other_project_id,
                parent_id: None,
                name: "Other docs".into(),
            },
        )
        .unwrap();

        let err = create_folder(
            &conn,
            &CreateFolder {
                project_id,
                parent_id: Some(parent.id),
                name: "Invalid child".into(),
            },
        )
        .unwrap_err();

        assert!(
            matches!(err, LificError::BadRequest(message) if message.contains("belongs to project"))
        );
        assert!(list_folders(&conn, project_id).unwrap().is_empty());
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

    fn module_named(conn: &Connection, project_id: i64, name: &str) -> i64 {
        create_module(
            conn,
            &CreateModule {
                project_id,
                name: name.into(),
                description: String::new(),
                status: "active".into(),
                emoji: None,
            },
        )
        .unwrap()
        .id
    }

    #[test]
    fn names_differing_only_by_case_resolve_to_the_exact_spelling_first() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn);
        let upper = module_named(&conn, pid, "Web");
        let lower = module_named(&conn, pid, "web");

        assert_eq!(resolve_module_name(&conn, pid, "Web").unwrap(), upper);
        assert_eq!(resolve_module_name(&conn, pid, "web").unwrap(), lower);
        match resolve_module_name(&conn, pid, "WEB").unwrap_err() {
            LificError::BadRequest(message) => assert_eq!(
                message,
                "module 'WEB' matches more than one module by case ('Web', 'web'); use the exact name"
            ),
            other => panic!("expected an ambiguity error, got {other:?}"),
        }
    }

    #[test]
    fn caseless_lookup_folds_non_ascii_letters() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn);
        let id = module_named(&conn, pid, "Ébauche");

        assert_eq!(resolve_module_name(&conn, pid, "ébauche").unwrap(), id);
        assert_eq!(resolve_module_name(&conn, pid, "ÉBAUCHE").unwrap(), id);
    }

    #[test]
    fn duplicate_exact_names_resolve_to_the_oldest_by_either_case() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn);
        let oldest = module_named(&conn, pid, "Core");
        module_named(&conn, pid, "Core");

        assert_eq!(resolve_module_name(&conn, pid, "Core").unwrap(), oldest);
        assert_eq!(resolve_module_name(&conn, pid, "core").unwrap(), oldest);
    }

    fn not_found_message(error: LificError) -> String {
        match error {
            LificError::NotFound(message) => message,
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_module_lists_the_valid_names_closest_first() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn);
        for name in ["Docs", "Web UI", "Backend"] {
            module_named(&conn, pid, name);
        }

        assert_eq!(
            not_found_message(resolve_module_name(&conn, pid, "Web").unwrap_err()),
            "module 'Web' not found in project; valid modules: 'Web UI', 'Docs', 'Backend'"
        );
        assert_eq!(
            not_found_message(resolve_module_name(&conn, pid, "Frontend").unwrap_err()),
            "module 'Frontend' not found in project; valid modules: 'Backend', 'Docs', 'Web UI'"
        );
    }

    #[test]
    fn a_missing_name_lists_at_most_ten_and_counts_the_rest() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn);
        for n in 0..12 {
            module_named(&conn, pid, &format!("Area {n:02}"));
        }

        let message = not_found_message(resolve_module_name(&conn, pid, "Area 11x").unwrap_err());
        assert!(
            message
                .starts_with("module 'Area 11x' not found in project; valid modules: 'Area 11', "),
            "got: {message}"
        );
        assert!(message.ends_with(" and 2 more"), "got: {message}");
        let (_, listed) = message.split_once("valid modules: ").unwrap();
        assert_eq!(listed.matches("'Area ").count(), 10, "got: {message}");
    }

    #[test]
    fn a_missing_name_in_an_empty_project_says_there_are_none() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn);

        assert_eq!(
            not_found_message(resolve_label_name(&conn, pid, "bug").unwrap_err()),
            "label 'bug' not found in project; the project has no labels"
        );
    }

    #[test]
    fn missing_labels_and_folders_list_their_own_names_once() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn);
        create_label(
            &conn,
            &CreateLabel {
                project_id: pid,
                name: "bug".into(),
                color: "#EF4444".into(),
            },
        )
        .unwrap();
        let parent = create_folder(
            &conn,
            &CreateFolder {
                project_id: pid,
                parent_id: None,
                name: "Specs".into(),
            },
        )
        .unwrap();
        for parent_id in [None, Some(parent.id)] {
            create_folder(
                &conn,
                &CreateFolder {
                    project_id: pid,
                    parent_id,
                    name: "Notes".into(),
                },
            )
            .unwrap();
        }

        assert_eq!(
            not_found_message(resolve_label_name(&conn, pid, "bugs").unwrap_err()),
            "label 'bugs' not found in project; valid labels: 'bug'"
        );
        assert_eq!(
            not_found_message(resolve_folder_name(&conn, pid, "Note").unwrap_err()),
            "folder 'Note' not found in project; valid folders: 'Notes', 'Specs'"
        );
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
                emoji: None,
            },
        )
        .unwrap();
        assert_eq!(module.description, "line1\nline2");
    }

    #[test]
    fn module_emoji_set_and_clear() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn);

        // Create with an icon.
        let module = create_module(
            &conn,
            &CreateModule {
                project_id: pid,
                name: "Icons".into(),
                description: String::new(),
                status: "active".into(),
                emoji: Some("lucide:Rocket".into()),
            },
        )
        .unwrap();
        assert_eq!(module.emoji.as_deref(), Some("lucide:Rocket"));
        // Survives a round-trip read.
        assert_eq!(
            get_module(&conn, module.id).unwrap().emoji.as_deref(),
            Some("lucide:Rocket")
        );

        // Change to a literal emoji.
        let updated = update_module(
            &conn,
            module.id,
            &UpdateModule {
                emoji: Some(Some("🚀".into())),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(updated.emoji.as_deref(), Some("🚀"));

        // Absent emoji field preserves the existing value.
        let preserved = update_module(
            &conn,
            module.id,
            &UpdateModule {
                name: Some("Icons!".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(preserved.emoji.as_deref(), Some("🚀"));

        // Explicit null clears it.
        let cleared = update_module(
            &conn,
            module.id,
            &UpdateModule {
                emoji: Some(None),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(cleared.emoji, None);
    }

    // ── Coverage backfill: previously-untested lookups & guards ──

    // get_resource_project_id interpolates the table name into SQL, so the
    // name may only ever come from `ResourceTable` (LIF-386). An unknown
    // table is not rejected at runtime any more, it is unrepresentable: the
    // enum's every value is one of the three literals asserted below, and a
    // caller has no other way to name a table.
    #[test]
    fn get_resource_project_id_resolves_and_rejects_bad_table() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn);
        let module = create_module(
            &conn,
            &CreateModule {
                project_id: pid,
                name: "M".into(),
                description: String::new(),
                status: "active".into(),
                emoji: None,
            },
        )
        .unwrap();
        let label = create_label(
            &conn,
            &CreateLabel {
                project_id: pid,
                name: "bug".into(),
                color: "#EF4444".into(),
            },
        )
        .unwrap();
        let folder = create_folder(
            &conn,
            &CreateFolder {
                project_id: pid,
                parent_id: None,
                name: "Docs".into(),
            },
        )
        .unwrap();

        assert_eq!(
            get_resource_project_id(&conn, ResourceTable::Modules, module.id).unwrap(),
            pid
        );
        assert_eq!(
            get_resource_project_id(&conn, ResourceTable::Labels, label.id).unwrap(),
            pid
        );
        assert_eq!(
            get_resource_project_id(&conn, ResourceTable::Folders, folder.id).unwrap(),
            pid
        );

        // The only table names reachable from the enum are these three bare
        // identifiers, so no call site can smuggle SQL in through the name.
        for table in [
            ResourceTable::Modules,
            ResourceTable::Labels,
            ResourceTable::Folders,
        ] {
            let name = table.as_table();
            assert!(
                ["modules", "labels", "folders"].contains(&name),
                "unexpected table identifier: {name}"
            );
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase()),
                "table identifier must be a bare identifier, got {name}"
            );
        }

        // Known table, missing row → NotFound.
        let err = get_resource_project_id(&conn, ResourceTable::Modules, 999_999).unwrap_err();
        assert!(matches!(err, LificError::NotFound(_)));
    }

    #[test]
    fn get_folder_name_returns_name_and_404s() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn);
        let folder = create_folder(
            &conn,
            &CreateFolder {
                project_id: pid,
                parent_id: None,
                name: "Design".into(),
            },
        )
        .unwrap();

        assert_eq!(get_folder_name(&conn, folder.id).unwrap(), "Design");
        let err = get_folder_name(&conn, 999_999).unwrap_err();
        assert!(matches!(err, LificError::NotFound(_)));
    }
}
