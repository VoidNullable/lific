use rusqlite::{Connection, OptionalExtension, params};

use crate::error::LificError;

#[derive(Clone, Copy)]
pub(crate) enum Entity {
    Issue,
    Page,
    Plan,
}

impl Entity {
    fn column(self) -> &'static str {
        match self {
            Self::Issue => "issues",
            Self::Page => "pages",
            Self::Plan => "plans",
        }
    }
}

/// Reserve inside the caller's savepoint so a failed create rolls back both.
/// SQLite serializes this write, including writers in other processes.
pub(crate) fn next(
    conn: &Connection,
    project_id: Option<i64>,
    entity: Entity,
) -> Result<i64, LificError> {
    let value = if let Some(project_id) = project_id {
        let column = entity.column();
        conn.query_row(
            &format!(
                "INSERT INTO project_number_sequences (project_id, {column}) VALUES (?1, 1)
                 ON CONFLICT(project_id) DO UPDATE SET {column} = {column} + 1
                 WHERE {column} < 9223372036854775807
                 RETURNING {column}"
            ),
            params![project_id],
            |row| row.get(0),
        )
        .optional()?
    } else {
        if !matches!(entity, Entity::Page) {
            return Err(LificError::Internal(
                "only pages can allocate workspace identifier numbers".into(),
            ));
        }
        conn.query_row(
            "UPDATE workspace_page_sequence SET value = value + 1
             WHERE id = 1 AND value < 9223372036854775807 RETURNING value",
            [],
            |row| row.get(0),
        )
        .optional()?
    };
    // Refuse at SQLite's signed integer limit rather than wrap or reuse.
    value.ok_or_else(|| LificError::Internal("identifier number sequence exhausted".into()))
}

#[cfg(test)]
mod tests;
