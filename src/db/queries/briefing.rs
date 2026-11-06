//! LIF-483: the reads behind `get_briefing` that no list query already
//! answers. Everything here is computed from live rows at call time; there is
//! no stored summary to drift. The one thing stored is each caller's cursor
//! (LIF-490), a position in the audit log that `since='last'` resumes from.

use rusqlite::{Connection, params_from_iter};

use crate::error::LificError;

/// A span of the audit log: rows with `after < id <= upto`. LIF-490: the
/// briefing reads changes by audit position rather than by timestamp, so a
/// stored cursor resumes exactly where the last briefing stopped, and a
/// change made in the same second as a briefing is neither lost nor repeated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChangeWindow {
    pub after: i64,
    pub upto: i64,
}

/// A live issue created in a project inside a window.
#[derive(Debug, Clone)]
pub struct CreatedIssue {
    pub identifier: String,
    pub title: String,
    pub status: String,
    pub priority: String,
    /// Username of whoever created it; `None` for a system write.
    pub actor: Option<String>,
}

/// An issue's net status movement inside a window: the status it had before
/// the first change and the status after the last. Issues that ended where
/// they started are not reported.
#[derive(Debug, Clone)]
pub struct StatusMove {
    pub identifier: String,
    pub title: String,
    pub from: String,
    pub to: String,
    /// Who made the last of the changes.
    pub actor: Option<String>,
}

/// A live page in a project edited inside a window.
#[derive(Debug, Clone)]
pub struct EditedPage {
    pub id: i64,
    pub identifier: String,
    pub title: String,
    /// Created inside the window as well, rather than only edited.
    pub new: bool,
    /// Who made the last edit.
    pub actor: Option<String>,
}

/// A live comment added inside a window to an issue or page of the project.
#[derive(Debug, Clone)]
pub struct NewComment {
    /// The issue or page identifier the comment is on.
    pub parent: String,
    /// Set when the parent is a page, for its link.
    pub page_id: Option<i64>,
    /// The first non-empty line of the comment.
    pub preview: String,
    pub actor: Option<String>,
}

/// Where a caller's previous briefing of a project left off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BriefingCursor {
    pub audit_id: i64,
    /// Stored UTC form, `YYYY-MM-DD HH:MM:SS`.
    pub briefed_at: String,
}

/// An unresolved `blocks` edge onto one of the requested issues.
#[derive(Debug, Clone)]
pub struct OpenBlocker {
    pub target_id: i64,
    pub identifier: String,
    pub status: String,
    pub project_id: i64,
}

/// The newest audit position, 0 on an empty log. Everything up to it has
/// been written; a briefing reports changes up to here and stores it.
pub fn latest_audit_id(conn: &Connection) -> Result<i64, LificError> {
    Ok(
        conn.query_row("SELECT COALESCE(MAX(id), 0) FROM audit_log", [], |row| {
            row.get(0)
        })?,
    )
}

/// The window of changes written strictly after `since` (stored UTC form),
/// up to `upto`. Reads forward from the timestamp index, so its cost follows
/// the number of changes since then, not the size of the log.
pub fn window_since_timestamp(
    conn: &Connection,
    since: &str,
    upto: i64,
) -> Result<ChangeWindow, LificError> {
    let first: Option<i64> = conn.query_row(
        "SELECT MIN(id) FROM audit_log WHERE ts > ?1",
        [since],
        |row| row.get(0),
    )?;
    Ok(ChangeWindow {
        after: first.map_or(upto, |first| first - 1),
        upto,
    })
}

/// The caller's stored cursor for a project, if they have briefed it before.
pub fn get_cursor(
    conn: &Connection,
    user_id: i64,
    project_id: i64,
) -> Result<Option<BriefingCursor>, LificError> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .query_row(
            "SELECT audit_id, briefed_at FROM briefing_cursors
             WHERE user_id = ?1 AND project_id = ?2",
            rusqlite::params![user_id, project_id],
            |row| {
                Ok(BriefingCursor {
                    audit_id: row.get(0)?,
                    briefed_at: row.get(1)?,
                })
            },
        )
        .optional()?)
}

/// Record that the caller has been briefed on a project up to `audit_id`.
pub fn save_cursor(
    conn: &Connection,
    user_id: i64,
    project_id: i64,
    audit_id: i64,
) -> Result<(), LificError> {
    conn.execute(
        "INSERT INTO briefing_cursors (user_id, project_id, audit_id, briefed_at)
         VALUES (?1, ?2, ?3, datetime('now'))
         ON CONFLICT (user_id, project_id)
         DO UPDATE SET audit_id = excluded.audit_id, briefed_at = excluded.briefed_at",
        rusqlite::params![user_id, project_id, audit_id],
    )?;
    Ok(())
}

/// Live issues in `project_id` created inside `window`, newest first.
pub fn issues_created_in(
    conn: &Connection,
    project_id: i64,
    window: ChangeWindow,
) -> Result<Vec<CreatedIssue>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT p.identifier || '-' || i.sequence, i.title, i.status, i.priority, u.username
         FROM audit_log a
         JOIN issues i ON i.id = a.entity_id AND i.deleted_at IS NULL AND i.project_id = ?1
         JOIN projects p ON p.id = i.project_id
         LEFT JOIN users u ON u.id = a.actor_user_id
         WHERE a.project_id = ?1 AND a.id > ?2 AND a.id <= ?3
           AND a.entity_type = 'issue' AND a.action = 'create'
         ORDER BY a.id DESC",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![project_id, window.after, window.upto],
        |row| {
            Ok(CreatedIssue {
                identifier: row.get(0)?,
                title: row.get(1)?,
                status: row.get(2)?,
                priority: row.get(3)?,
                actor: row.get(4)?,
            })
        },
    )?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Net status movement per live issue in `project_id` inside `window`, read
/// from the audit log, most recently moved first.
pub fn status_moves_in(
    conn: &Connection,
    project_id: i64,
    window: ChangeWindow,
) -> Result<Vec<StatusMove>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT a.entity_id, p.identifier || '-' || i.sequence, i.title,
                COALESCE(a.old_value, ''), COALESCE(a.new_value, ''), u.username
         FROM audit_log a
         JOIN issues i ON i.id = a.entity_id AND i.deleted_at IS NULL
         JOIN projects p ON p.id = i.project_id
         LEFT JOIN users u ON u.id = a.actor_user_id
         WHERE a.project_id = ?1 AND a.id > ?2 AND a.id <= ?3
           AND a.entity_type = 'issue' AND a.action = 'update' AND a.field = 'status'
           AND i.project_id = ?1
         ORDER BY a.id ASC",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![project_id, window.after, window.upto],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                StatusMove {
                    identifier: row.get(1)?,
                    title: row.get(2)?,
                    from: row.get(3)?,
                    to: row.get(4)?,
                    actor: row.get(5)?,
                },
            ))
        },
    )?;
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
                existing.actor.clone_from(&change.actor);
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

/// Live pages in `project_id` created or edited inside `window`, most
/// recently edited first.
pub fn pages_edited_in(
    conn: &Connection,
    project_id: i64,
    window: ChangeWindow,
) -> Result<Vec<EditedPage>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT pg.id, p.identifier || '-DOC-' || pg.sequence, pg.title,
                a.action = 'create', u.username
         FROM audit_log a
         JOIN pages pg ON pg.id = a.entity_id AND pg.deleted_at IS NULL AND pg.project_id = ?1
         JOIN projects p ON p.id = pg.project_id
         LEFT JOIN users u ON u.id = a.actor_user_id
         WHERE a.project_id = ?1 AND a.id > ?2 AND a.id <= ?3
           AND a.entity_type = 'page' AND a.action IN ('create', 'update')
         ORDER BY a.id ASC",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![project_id, window.after, window.upto],
        |row| {
            Ok(EditedPage {
                id: row.get(0)?,
                identifier: row.get(1)?,
                title: row.get(2)?,
                new: row.get(3)?,
                actor: row.get(4)?,
            })
        },
    )?;
    let mut pages: std::collections::HashMap<i64, (usize, EditedPage)> =
        std::collections::HashMap::new();
    for (index, row) in rows.enumerate() {
        let edit = row?;
        pages
            .entry(edit.id)
            .and_modify(|(last, existing)| {
                *last = index;
                existing.new |= edit.new;
                existing.actor.clone_from(&edit.actor);
            })
            .or_insert((index, edit));
    }
    let mut pages: Vec<(usize, EditedPage)> = pages.into_values().collect();
    pages.sort_by_key(|(last, _)| std::cmp::Reverse(*last));
    Ok(pages.into_iter().map(|(_, page)| page).collect())
}

/// Live comments added inside `window` to live issues and pages of
/// `project_id`, newest first.
pub fn comments_added_in(
    conn: &Connection,
    project_id: i64,
    window: ChangeWindow,
) -> Result<Vec<NewComment>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT COALESCE(ip.identifier || '-' || i.sequence,
                         pp.identifier || '-DOC-' || pg.sequence),
                pg.id, c.content, u.username
         FROM audit_log a
         JOIN comments c ON c.id = a.entity_id AND c.deleted_at IS NULL
         LEFT JOIN issues i ON i.id = c.issue_id AND i.deleted_at IS NULL
         LEFT JOIN projects ip ON ip.id = i.project_id
         LEFT JOIN pages pg ON pg.id = c.page_id AND pg.deleted_at IS NULL
         LEFT JOIN projects pp ON pp.id = pg.project_id
         LEFT JOIN users u ON u.id = a.actor_user_id
         WHERE a.project_id = ?1 AND a.id > ?2 AND a.id <= ?3
           AND a.entity_type = 'comment' AND a.action = 'create'
           AND (i.project_id = ?1 OR pg.project_id = ?1)
         ORDER BY a.id DESC",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![project_id, window.after, window.upto],
        |row| {
            let content: String = row.get(2)?;
            Ok(NewComment {
                parent: row.get(0)?,
                page_id: row.get(1)?,
                preview: content
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .unwrap_or_default()
                    .to_string(),
                actor: row.get(3)?,
            })
        },
    )?;
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
