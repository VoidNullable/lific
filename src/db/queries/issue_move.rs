//! LIF-487: moving an issue to another project.
//!
//! The issue keeps its row, so everything keyed by issue id (comments,
//! relations, history, waits, plan steps, attachments, page links) moves with
//! it untouched. What changes is the identifier: the issue takes the next
//! sequence in its new project, and the identifier it gave up is recorded in
//! `issue_aliases` (migration 064) so it keeps resolving.
//!
//! Labels and modules belong to a project, so they cannot follow by id. A
//! label is kept when the new project has one of the same name; a module is
//! taken from the caller, or kept by name, or cleared. Named assignees who
//! cannot see the new project are unassigned. Everything dropped is reported
//! back rather than lost silently.

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::LificError;

use super::get_issue;

/// What a move should do besides changing the project.
#[derive(Debug, Default, Clone)]
pub struct MoveIssue {
    /// Module in the target project. `None` keeps the issue's current module
    /// when the target has one of the same name, otherwise clears it.
    pub module_id: Option<i64>,
}

/// The moved issue, plus what could not come along.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MovedIssue {
    pub issue: crate::db::models::Issue,
    /// The identifier the issue had before the move, now an alias.
    pub previous_identifier: String,
    pub previous_project_id: i64,
    /// Labels the target project has no label of the same name for.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dropped_labels: Vec<String>,
    /// The module that was cleared because the target has none of that name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dropped_module: Option<String>,
    /// Assignees who cannot see the target project.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dropped_assignees: Vec<String>,
}

/// The next free sequence in `project_id`. Counts tombstoned issues (LIF-438)
/// and aliases, so no identifier is ever handed out twice: a restored issue
/// and an old identifier both keep meaning what they meant.
pub fn next_issue_sequence(conn: &Connection, project_id: i64) -> Result<i64, LificError> {
    Ok(conn.query_row(
        "SELECT MAX(
             (SELECT COALESCE(MAX(sequence), 0) FROM issues WHERE project_id = ?1),
             (SELECT COALESCE(MAX(sequence), 0) FROM issue_aliases WHERE project_id = ?1)
         ) + 1",
        params![project_id],
        |row| row.get(0),
    )?)
}

/// The live issue an old identifier (`PROJ`, `sequence`) now points to.
pub fn resolve_alias(
    conn: &Connection,
    project_identifier: &str,
    sequence: i64,
) -> Result<Option<i64>, LificError> {
    Ok(conn
        .prepare_cached(
            "SELECT a.issue_id FROM issue_aliases a
             JOIN projects p ON p.id = a.project_id
             JOIN issues i ON i.id = a.issue_id AND i.deleted_at IS NULL
             WHERE p.identifier = ?1 AND a.sequence = ?2",
        )?
        .query_row(params![project_identifier, sequence], |row| row.get(0))
        .optional()?)
}

/// Every identifier an issue has given up, oldest first.
pub fn issue_aliases(conn: &Connection, issue_id: i64) -> Result<Vec<String>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT p.identifier || '-' || a.sequence FROM issue_aliases a
         JOIN projects p ON p.id = a.project_id
         WHERE a.issue_id = ?1
         ORDER BY a.seq",
    )?;
    let rows = stmt.query_map(params![issue_id], |row| row.get(0))?;
    Ok(rows.collect::<Result<Vec<String>, _>>()?)
}

/// Move a live issue to `target_project_id`. Authorization is the caller's:
/// a move writes to both projects, so it needs write rights on each.
pub fn move_issue(
    conn: &Connection,
    issue_id: i64,
    target_project_id: i64,
    input: &MoveIssue,
) -> Result<MovedIssue, LificError> {
    super::savepoint(conn, "move_issue", || {
        let before = get_issue(conn, issue_id)?;
        if before.project_id == target_project_id {
            return Err(LificError::BadRequest(format!(
                "{} is already in that project",
                before.identifier
            )));
        }
        // Validates that the target exists before anything is written.
        super::get_project(conn, target_project_id)?;
        if let Some(module_id) = input.module_id {
            super::validate_module_project(conn, target_project_id, module_id)?;
        }

        let module_id = match input.module_id {
            Some(id) => Some(id),
            None => match before.module_id {
                Some(current) => conn
                    .query_row(
                        "SELECT t.id FROM modules m
                         JOIN modules t ON t.project_id = ?1 AND t.name = m.name
                         WHERE m.id = ?2",
                        params![target_project_id, current],
                        |row| row.get(0),
                    )
                    .optional()?,
                None => None,
            },
        };
        let dropped_module = match (before.module_id, module_id, input.module_id) {
            (Some(current), None, None) => Some(super::get_module_name(conn, current)?),
            _ => None,
        };

        // Relabel by name. Rows for labels the target lacks are deleted and
        // reported; the rest are repointed at the target's label.
        let dropped_labels: Vec<String> = before
            .labels
            .iter()
            .filter(|name| {
                conn.query_row(
                    "SELECT 1 FROM labels WHERE project_id = ?1 AND name = ?2",
                    params![target_project_id, name],
                    |_| Ok(()),
                )
                .optional()
                .map(|found| found.is_none())
                .unwrap_or(true)
            })
            .cloned()
            .collect();
        if !before.labels.is_empty() {
            conn.execute(
                "DELETE FROM issue_labels WHERE issue_id = ?1",
                params![issue_id],
            )?;
            for name in &before.labels {
                conn.execute(
                    "INSERT OR IGNORE INTO issue_labels (issue_id, label_id)
                     SELECT ?1, id FROM labels WHERE project_id = ?2 AND name = ?3",
                    params![issue_id, target_project_id, name],
                )?;
            }
        }

        let dropped_assignees =
            super::assignees::drop_non_members(conn, issue_id, target_project_id)?;

        // The alias takes its seq before the issue's update stamps a newer
        // one, so the old project's tombstone sorts before the new project's
        // live row in any stream that sees both.
        conn.execute("UPDATE sync_seq SET value = value + 1 WHERE id = 1", [])?;
        conn.execute(
            "INSERT INTO issue_aliases (project_id, sequence, issue_id, seq)
             VALUES (?1, ?2, ?3, (SELECT value FROM sync_seq WHERE id = 1))",
            params![before.project_id, before.sequence, issue_id],
        )?;

        let sequence = next_issue_sequence(conn, target_project_id)?;
        conn.execute(
            "UPDATE issues SET project_id = ?1, sequence = ?2, module_id = ?3 WHERE id = ?4",
            params![target_project_id, sequence, module_id, issue_id],
        )?;

        Ok(MovedIssue {
            issue: get_issue(conn, issue_id)?,
            previous_identifier: before.identifier,
            previous_project_id: before.project_id,
            dropped_labels,
            dropped_module,
            dropped_assignees,
        })
    })
}

#[cfg(test)]
mod tests;
