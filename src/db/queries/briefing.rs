//! LIF-483: the reads behind `get_briefing` that no list query already
//! answers. Everything here is computed from live rows at call time; there is
//! no stored summary to drift.

use rusqlite::{Connection, params_from_iter};

use crate::error::LificError;

/// A live issue created in a project after a cursor.
#[derive(Debug, Clone)]
pub struct CreatedIssue {
    pub identifier: String,
    pub title: String,
    pub status: String,
    pub priority: String,
}

/// An issue's net status movement after a cursor: the status it had before
/// the first change and the status after the last. Issues that ended where
/// they started are not reported.
#[derive(Debug, Clone)]
pub struct StatusMove {
    pub identifier: String,
    pub title: String,
    pub from: String,
    pub to: String,
}

/// A live page in a project edited after a cursor.
#[derive(Debug, Clone)]
pub struct EditedPage {
    pub id: i64,
    pub identifier: String,
    pub title: String,
    /// Created after the cursor as well, rather than only edited.
    pub new: bool,
}

/// An unresolved `blocks` edge onto one of the requested issues.
#[derive(Debug, Clone)]
pub struct OpenBlocker {
    pub target_id: i64,
    pub identifier: String,
    pub status: String,
    pub project_id: i64,
}

/// Live issues in `project_id` created strictly after `since` (stored UTC
/// form), newest first.
pub fn issues_created_since(
    conn: &Connection,
    project_id: i64,
    since: &str,
) -> Result<Vec<CreatedIssue>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT p.identifier || '-' || i.sequence, i.title, i.status, i.priority
         FROM issues i JOIN projects p ON p.id = i.project_id
         WHERE i.project_id = ?1 AND i.created_at > ?2 AND i.deleted_at IS NULL
         ORDER BY i.created_at DESC, i.sequence DESC",
    )?;
    let rows = stmt.query_map(rusqlite::params![project_id, since], |row| {
        Ok(CreatedIssue {
            identifier: row.get(0)?,
            title: row.get(1)?,
            status: row.get(2)?,
            priority: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Net status movement per live issue in `project_id` after `since`, read
/// from the audit log, most recently moved first.
pub fn status_moves_since(
    conn: &Connection,
    project_id: i64,
    since: &str,
) -> Result<Vec<StatusMove>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT a.entity_id, p.identifier || '-' || i.sequence, i.title,
                COALESCE(a.old_value, ''), COALESCE(a.new_value, '')
         FROM audit_log a
         JOIN issues i ON i.id = a.entity_id AND i.deleted_at IS NULL
         JOIN projects p ON p.id = i.project_id
         WHERE a.project_id = ?1 AND a.ts > ?2
           AND a.entity_type = 'issue' AND a.action = 'update' AND a.field = 'status'
           AND i.project_id = ?1
         ORDER BY a.ts ASC, a.id ASC",
    )?;
    let rows = stmt.query_map(rusqlite::params![project_id, since], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            StatusMove {
                identifier: row.get(1)?,
                title: row.get(2)?,
                from: row.get(3)?,
                to: row.get(4)?,
            },
        ))
    })?;
    // Fold each issue's changes into one first-from, last-to move, remembering
    // the row index of its last change so the newest movement sorts first.
    let mut moves: std::collections::HashMap<i64, (usize, StatusMove)> =
        std::collections::HashMap::new();
    for (index, row) in rows.enumerate() {
        let (issue_id, change) = row?;
        moves
            .entry(issue_id)
            .and_modify(|(last, existing)| {
                *last = index;
                existing.to.clone_from(&change.to);
            })
            .or_insert((index, change));
    }
    let mut moves: Vec<(usize, StatusMove)> = moves
        .into_values()
        .filter(|(_, change)| change.from != change.to)
        .collect();
    moves.sort_by_key(|(last, _)| std::cmp::Reverse(*last));
    Ok(moves.into_iter().map(|(_, change)| change).collect())
}

/// Live pages in `project_id` updated strictly after `since`, most recent
/// first.
pub fn pages_edited_since(
    conn: &Connection,
    project_id: i64,
    since: &str,
) -> Result<Vec<EditedPage>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT pg.id, p.identifier || '-DOC-' || pg.sequence, pg.title, pg.created_at > ?2
         FROM pages pg JOIN projects p ON p.id = pg.project_id
         WHERE pg.project_id = ?1 AND pg.updated_at > ?2 AND pg.deleted_at IS NULL
         ORDER BY pg.updated_at DESC, pg.id DESC",
    )?;
    let rows = stmt.query_map(rusqlite::params![project_id, since], |row| {
        Ok(EditedPage {
            id: row.get(0)?,
            identifier: row.get(1)?,
            title: row.get(2)?,
            new: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Every unresolved blocker of the given issues, using the same definition as
/// `list_issues(blocked=true)`: a live source issue whose status is not done.
/// Ordered by target, then blocker identifier, for stable output.
pub fn open_blockers(
    conn: &Connection,
    target_ids: &[i64],
) -> Result<Vec<OpenBlocker>, LificError> {
    if target_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = (1..=target_ids.len())
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT ir.target_id, bp.identifier || '-' || b.sequence, b.status, b.project_id
         FROM issue_relations ir
         JOIN issues b ON b.id = ir.source_id
         JOIN projects bp ON bp.id = b.project_id
         WHERE ir.target_id IN ({placeholders})
           AND ir.relation_type = 'blocks'
           AND b.status != 'done'
           AND b.deleted_at IS NULL
         ORDER BY ir.target_id, bp.identifier, b.sequence"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(target_ids), |row| {
        Ok(OpenBlocker {
            target_id: row.get(0)?,
            identifier: row.get(1)?,
            status: row.get(2)?,
            project_id: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Open, live issues in a project with a date wait that no longer holds
/// (LIF-484): its earliest day has arrived, so it is due to check or overdue.
/// Most overdue first (earliest `latest`), then by identifier. `today` is
/// [`super::waits::today_text`].
pub fn issues_with_due_waits(
    conn: &Connection,
    project_id: i64,
    today: &str,
) -> Result<Vec<i64>, LificError> {
    let mut stmt = conn.prepare(
        "SELECT i.id
           FROM issues i
           JOIN issue_waits w ON w.issue_id = i.id
          WHERE i.project_id = ?1
            AND i.deleted_at IS NULL
            AND i.status NOT IN ('done', 'cancelled')
            AND w.kind = 'date'
            AND w.earliest <= ?2
          GROUP BY i.id
          ORDER BY MIN(w.latest), i.sequence",
    )?;
    let rows = stmt.query_map(rusqlite::params![project_id, today], |row| row.get(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}
