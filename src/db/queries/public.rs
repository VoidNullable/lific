//! LIF-465: the anonymous read model for a published project.
//!
//! Publication is a predicate inside every statement here, not a gate a caller
//! passes once. Each query joins `projects` and requires `is_public = 1` in the
//! same statement that fetches the content, so no already-resolved id can skip
//! the check and unpublishing takes effect on the next read snapshot.
//!
//! Reachable: the published project's identity, its current issues, their
//! current comments, and attachments linked to those issues or comments.
//! Everything else is excluded by construction rather than by convention: no
//! query joins `pages`, `plans`, `audit_log`, `users` or `project_members`; the
//! comment queries require `c.issue_id IS NOT NULL`; every query takes the
//! project identifier as a bind parameter; tombstones are excluded by
//! `deleted_at IS NULL` on the issue, the comment and the comment's parent.
//!
//! Author identity is absent from every DTO: the contract excludes account
//! metadata, and a username is that.
//!
//! Two resource rules apply to everything below, because this is the only
//! surface with no account behind it:
//! - reads are paged, and a page is a bounded number of rows;
//! - JSON bytes are conservatively bounded before text reaches Rust, including
//!   metadata and envelopes. Callers composing responses must share a transaction.

use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::error::LificError;

/// Issues per page: the default, and the ceiling when a caller asks for more.
pub const PUBLIC_ISSUE_PAGE: i64 = 100;

/// Comments per page. Smaller than the issue page because a comment body is
/// unbounded prose where an issue row is mostly a title.
pub const PUBLIC_COMMENT_PAGE: i64 = 50;

/// Maximum serialized JSON response bytes, including its envelope.
pub const PUBLIC_PAGE_BYTES: usize = 512 * 1024;

// Keys, quotes, nulls, commas and all i64 digits fit these fixed allowances.
const ENVELOPE_BYTES: usize = 256;
const PROJECT_BYTES: usize = 256;
const ISSUE_BYTES: usize = 512;
const COMMENT_BYTES: usize = 256;
const ATTACHMENT_BYTES: usize = 256;
const LABEL_BYTES: usize = 32;

// SQLite counts BLOB bytes past NUL; JSON escaping uses at most six per byte.
fn text_bound(fields: &[&str], overhead: usize) -> String {
    let lengths = fields
        .iter()
        .map(|field| format!("COALESCE(length(CAST({field} AS BLOB)), 0)"))
        .collect::<Vec<_>>()
        .join(" + ");
    format!(
        "min({}, {overhead} + 6 * ({lengths}))",
        PUBLIC_PAGE_BYTES + 1
    )
}

// Reuse the caller's snapshot, or hold our own across preflight and materialization.
fn read_snapshot(conn: &Connection) -> Result<Option<rusqlite::Transaction<'_>>, LificError> {
    Ok(if conn.is_autocommit() {
        Some(conn.unchecked_transaction()?)
    } else {
        None
    })
}

/// Test-only tally of SQL statements this module issues, so an N+1 regression
/// fails a test instead of a production instance. Thread-local because the
/// harness runs each test on its own thread and `#[tokio::test]` drives a
/// current-thread runtime on it.
#[cfg(test)]
pub(crate) mod probe {
    use std::cell::Cell;

    thread_local! {
        pub(crate) static STATEMENTS: Cell<usize> = const { Cell::new(0) };
        pub(crate) static MATERIALIZED: Cell<usize> = const { Cell::new(0) };
    }

    pub(crate) fn count() -> usize {
        STATEMENTS.with(Cell::get)
    }
}

#[inline]
fn statement_issued() {
    #[cfg(test)]
    probe::STATEMENTS.with(|count| count.set(count.get() + 1));
}

#[inline]
fn materializing_row(_conn: &Connection) {
    #[cfg(test)]
    {
        assert!(
            !_conn.is_autocommit(),
            "public materialization needs a snapshot"
        );
        probe::MATERIALIZED.with(|count| count.set(count.get() + 1));
    }
}

/// How many of `sizes` fit in `PUBLIC_PAGE_BYTES`, as a contiguous prefix.
///
/// A prefix, never a subset: skipping a large row to fit a later one leaves a
/// hole a paging client would never come back for. A first row that does not
/// fit on its own is refused rather than returned as an empty page, so
/// oversized legacy content produces a visible error instead of a thread that
/// silently stops.
fn fitting_prefix(sizes: &[usize], base: usize, subject: &str) -> Result<usize, LificError> {
    if base > PUBLIC_PAGE_BYTES {
        return Err(oversize("project metadata", base));
    }
    let mut used = base;
    for (index, size) in sizes.iter().enumerate() {
        let next = used.saturating_add(*size);
        if next > PUBLIC_PAGE_BYTES {
            if index == 0 {
                return Err(oversize(subject, next));
            }
            return Ok(index);
        }
        used = next;
    }
    Ok(sizes.len())
}

fn oversize(subject: &str, size: usize) -> LificError {
    LificError::PayloadTooLarge(format!(
        "Cannot show {subject}: its estimated JSON size ({size} bytes) exceeds \
         the {PUBLIC_PAGE_BYTES}-byte public response limit"
    ))
}

// ── DTOs ─────────────────────────────────────────────────────
//
// Allowlists, not projections of the internal models. `Serialize` only:
// nothing anonymous is deserialized into these.

/// The published project. No id, no lead, no timestamps.
#[derive(Debug, Clone, Serialize)]
pub struct PublicProject {
    pub identifier: String,
    pub name: String,
    pub description: String,
    pub emoji: Option<String>,
}

/// One issue. `identifier` is the readable form; the numeric row id is not
/// exposed, so nothing a reader sees can be replayed against `/api/issues/{id}`.
#[derive(Debug, Clone, Serialize)]
pub struct PublicIssue {
    pub identifier: String,
    pub title: String,
    pub status: String,
    pub priority: String,
    pub module: Option<String>,
    pub labels: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicIssueDetail {
    #[serde(flatten)]
    pub issue: PublicIssue,
    pub description: String,
    pub attachments: Vec<PublicAttachment>,
}

/// One comment. No author, no user id, no mention list.
#[derive(Debug, Clone, Serialize)]
pub struct PublicComment {
    pub id: i64,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
    pub attachments: Vec<PublicAttachment>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicComments {
    pub comments: Vec<PublicComment>,
    /// Total live comments on the issue, so a client can show progress and
    /// know the thread is fully readable by paging.
    pub total: i64,
    pub has_more: bool,
}

/// Attachment metadata a public reader may see. No `sha256` (the content
/// address the private store is keyed by), no uploader, no timestamps.
#[derive(Debug, Clone, Serialize)]
pub struct PublicAttachment {
    pub id: i64,
    pub filename: String,
    pub mime: String,
    pub size_bytes: i64,
    pub alt_text: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
}

/// What the download handler needs. Not `Serialize`: these shape headers and
/// open a file, they are never a response body.
#[derive(Debug, Clone)]
pub struct PublicBlob {
    pub sha256: String,
    pub filename: String,
    pub mime: String,
}

// ── Project ──────────────────────────────────────────────────

/// The published project named by `identifier`, or `None`.
///
/// `None` covers "no such project", "private project" and "just unpublished"
/// alike; the caller must not be able to tell them apart, which is why this is
/// an `Option` rather than distinguishable errors. `projects.identifier` is
/// NOCASE (migration 039), so `/public/lif` and `/public/LIF` are one project.
pub fn get_public_project(
    conn: &Connection,
    identifier: &str,
) -> Result<Option<PublicProject>, LificError> {
    let _snapshot = read_snapshot(conn)?;
    let Some(bytes) = project_bound(conn, identifier)? else {
        return Ok(None);
    };
    fitting_prefix(&[], bytes + ENVELOPE_BYTES, "project metadata")?;
    statement_issued();
    conn.prepare_cached(
        "SELECT identifier, name, description, emoji
           FROM projects
          WHERE identifier = ?1 AND is_public = 1",
    )?
    .query_row(params![identifier], |row| {
        materializing_row(conn);
        Ok(PublicProject {
            identifier: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            emoji: row.get(3)?,
        })
    })
    .optional()
    .map_err(Into::into)
}

fn project_bound(conn: &Connection, identifier: &str) -> Result<Option<usize>, LificError> {
    let bound = text_bound(
        &["identifier", "name", "description", "emoji"],
        PROJECT_BYTES,
    );
    statement_issued();
    Ok(conn
        .prepare_cached(&format!(
            "SELECT {bound} FROM projects WHERE identifier = ?1 AND is_public = 1"
        ))?
        .query_row([identifier], |row| row.get::<_, usize>(0))
        .optional()?)
}

fn issue_bound() -> String {
    text_bound(
        &[
            "p.identifier || '-' || i.sequence",
            "i.title",
            "i.status",
            "i.priority",
            "m.name",
            "i.created_at",
            "i.updated_at",
        ],
        ISSUE_BYTES,
    )
}

// Aggregate only the bounded candidate window. No child strings cross into Rust.
fn child_bounds(
    conn: &Connection,
    ids: &[i64],
    entity_type: Option<&str>,
) -> Result<HashMap<i64, usize>, LificError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let (key, source, bound) = match entity_type {
        None => (
            "il.issue_id",
            "issue_labels il JOIN labels l ON l.id = il.label_id".to_string(),
            text_bound(&["l.name"], LABEL_BYTES),
        ),
        Some(kind) => (
            "l.entity_id",
            format!(
                "attachment_links l JOIN attachments a ON a.id = l.attachment_id AND l.entity_type = '{kind}'"
            ),
            text_bound(&["a.filename", "a.mime", "a.alt_text"], ATTACHMENT_BYTES),
        ),
    };
    statement_issued();
    let mut stmt = conn.prepare(&format!(
        "SELECT {key}, CAST(min(total({bound}), {}) AS INTEGER)
         FROM {source} WHERE {key} IN ({placeholders}) GROUP BY {key}",
        PUBLIC_PAGE_BYTES + 1
    ))?;
    Ok(stmt
        .query_map(rusqlite::params_from_iter(ids), |row| {
            Ok((row.get(0)?, row.get::<_, usize>(1)?))
        })?
        .collect::<Result<HashMap<_, _>, _>>()?)
}

fn add_child_bounds(window: &mut [(i64, usize)], bounds: &HashMap<i64, usize>) {
    for (id, size) in window {
        *size = size.saturating_add(bounds.get(id).copied().unwrap_or(0));
    }
}

// ── Issues ───────────────────────────────────────────────────

/// One page of current issues, oldest sequence first, with whether more exist.
///
/// `has_more` comes from a row fetched past the end of the page, so a project
/// holding exactly `limit` issues does not advertise an empty next page.
/// Statement count is flat in the page size: a preflight, the rows, the labels.
pub fn list_public_issues(
    conn: &Connection,
    project_identifier: &str,
    limit: i64,
    offset: i64,
) -> Result<(Vec<PublicIssue>, bool), LificError> {
    let _snapshot = read_snapshot(conn)?;
    let base = ENVELOPE_BYTES + project_bound(conn, project_identifier)?.unwrap_or(0);
    let limit = limit.clamp(1, PUBLIC_ISSUE_PAGE);
    let offset = offset.max(0);

    // Preflight: ids and byte sizes only, so nothing oversized is read.
    statement_issued();
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT i.id, {}
           FROM issues i
           JOIN projects p ON p.id = i.project_id
      LEFT JOIN modules m ON m.id = i.module_id
          WHERE p.identifier = ?1
            AND p.is_public = 1
            AND i.deleted_at IS NULL
        ORDER BY i.sequence, i.id
           LIMIT ?2 OFFSET ?3",
        issue_bound(),
    ))?;
    let mut window: Vec<(i64, usize)> = stmt
        .query_map(params![project_identifier, limit + 1, offset], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?.unsigned_abs() as usize,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let capped = window.len().min(limit.unsigned_abs() as usize);
    let candidate_ids: Vec<i64> = window[..capped].iter().map(|(id, _)| *id).collect();
    add_child_bounds(
        &mut window[..capped],
        &child_bounds(conn, &candidate_ids, None)?,
    );
    let sizes: Vec<usize> = window[..capped].iter().map(|(_, size)| *size).collect();
    let fitted = fitting_prefix(&sizes, base, "an issue in this project")?;
    let has_more = window.len() as i64 > limit || fitted < capped;
    let ids: Vec<i64> = window[..fitted].iter().map(|(id, _)| *id).collect();

    let issues = read_issue_rows(conn, project_identifier, &ids)?;
    Ok((issues, has_more))
}

/// Fetch the issue rows for ids the preflight already accepted, in id order
/// matching `ids`. Publication is still in the statement: this is a read of
/// public data, not a trusted continuation.
fn read_issue_rows(
    conn: &Connection,
    project_identifier: &str,
    ids: &[i64],
) -> Result<Vec<PublicIssue>, LificError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(",");
    statement_issued();
    let mut stmt = conn.prepare(&format!(
        "SELECT i.id, p.identifier || '-' || i.sequence,
                i.title, i.status, i.priority, m.name,
                i.created_at, i.updated_at
           FROM issues i
           JOIN projects p ON p.id = i.project_id
      LEFT JOIN modules m ON m.id = i.module_id
          WHERE p.identifier = ?1
            AND p.is_public = 1
            AND i.deleted_at IS NULL
            AND i.id IN ({placeholders})
        ORDER BY i.sequence, i.id"
    ))?;
    let bound = rusqlite::params_from_iter(
        std::iter::once(rusqlite::types::Value::from(project_identifier.to_string()))
            .chain(ids.iter().copied().map(rusqlite::types::Value::from)),
    );
    let rows = stmt.query_map(bound, |row| {
        materializing_row(conn);
        Ok((
            row.get::<_, i64>(0)?,
            PublicIssue {
                identifier: row.get(1)?,
                title: row.get(2)?,
                status: row.get(3)?,
                priority: row.get(4)?,
                module: row.get(5)?,
                labels: Vec::new(),
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            },
        ))
    })?;
    let mut issues: Vec<(i64, PublicIssue)> = rows.collect::<Result<Vec<_>, _>>()?;
    let mut labels = labels_for_issues(conn, ids)?;
    Ok(issues
        .drain(..)
        .map(|(id, mut issue)| {
            issue.labels = labels.remove(&id).unwrap_or_default();
            issue
        })
        .collect())
}

/// One current issue by sequence. The project identifier and the sequence are
/// bind parameters of the same statement, so an identifier naming another
/// project cannot be answered from this one.
///
/// The description is size-checked before it is read, so a legacy body past the
/// public budget produces a visible error rather than a huge response.
pub fn get_public_issue(
    conn: &Connection,
    project_identifier: &str,
    sequence: i64,
) -> Result<Option<PublicIssueDetail>, LificError> {
    let _snapshot = read_snapshot(conn)?;
    let base = ENVELOPE_BYTES + project_bound(conn, project_identifier)?.unwrap_or(0);
    statement_issued();
    let size: Option<(i64, usize)> = conn
        .prepare_cached(&format!(
            "SELECT i.id, {} + {}
               FROM issues i
               JOIN projects p ON p.id = i.project_id
          LEFT JOIN modules m ON m.id = i.module_id
              WHERE p.identifier = ?1
                AND p.is_public = 1
                AND i.sequence = ?2
                 AND i.deleted_at IS NULL",
            issue_bound(),
            text_bound(&["i.description"], 0),
        ))?
        .query_row(params![project_identifier, sequence], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?.unsigned_abs() as usize,
            ))
        })
        .optional()?;

    let Some((id, bytes)) = size else {
        return Ok(None);
    };
    let mut window = [(id, bytes)];
    add_child_bounds(&mut window, &child_bounds(conn, &[id], None)?);
    add_child_bounds(&mut window, &child_bounds(conn, &[id], Some("issue"))?);
    fitting_prefix(&[window[0].1], base, "this issue")?;

    statement_issued();
    let found = conn
        .prepare_cached(
            "SELECT p.identifier || '-' || i.sequence,
                    i.title, i.status, i.priority, m.name,
                    i.created_at, i.updated_at, i.description
               FROM issues i
               JOIN projects p ON p.id = i.project_id
          LEFT JOIN modules m ON m.id = i.module_id
              WHERE p.identifier = ?1
                AND p.is_public = 1
                AND i.sequence = ?2
                AND i.deleted_at IS NULL",
        )?
        .query_row(params![project_identifier, sequence], |row| {
            materializing_row(conn);
            Ok(PublicIssueDetail {
                issue: PublicIssue {
                    identifier: row.get(0)?,
                    title: row.get(1)?,
                    status: row.get(2)?,
                    priority: row.get(3)?,
                    module: row.get(4)?,
                    labels: Vec::new(),
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                },
                description: row.get(7)?,
                attachments: Vec::new(),
            })
        })
        .optional()?;

    let Some(mut detail) = found else {
        return Ok(None);
    };
    detail.issue.labels = labels_for_issues(conn, &[id])?
        .remove(&id)
        .unwrap_or_default();
    detail.attachments = attachments_for(conn, "issue", &[id])?
        .remove(&id)
        .unwrap_or_default();
    Ok(Some(detail))
}

/// Labels for issue ids this module already resolved through a published path.
/// One statement whatever the page size; the `IN` list length is the only thing
/// interpolated and it is bounded by the page.
fn labels_for_issues(
    conn: &Connection,
    ids: &[i64],
) -> Result<HashMap<i64, Vec<String>>, LificError> {
    let mut out: HashMap<i64, Vec<String>> = HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(",");
    statement_issued();
    let mut stmt = conn.prepare(&format!(
        "SELECT il.issue_id, l.name
           FROM issue_labels il
           JOIN labels l ON l.id = il.label_id
          WHERE il.issue_id IN ({placeholders})
       ORDER BY il.issue_id, l.name"
    ))?;
    let rows = stmt.query_map(rusqlite::params_from_iter(ids), |row| {
        materializing_row(conn);
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (issue_id, name) = row?;
        out.entry(issue_id).or_default().push(name);
    }
    Ok(out)
}

// ── Comments ─────────────────────────────────────────────────

/// Count of live comments on a live issue of the published project.
pub fn public_issue_exists(
    conn: &Connection,
    project_identifier: &str,
    sequence: i64,
) -> Result<bool, LificError> {
    statement_issued();
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM issues i JOIN projects p ON p.id=i.project_id
         WHERE p.identifier=?1 AND p.is_public=1 AND i.sequence=?2 AND i.deleted_at IS NULL)",
        params![project_identifier, sequence],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

pub fn count_public_comments(
    conn: &Connection,
    project_identifier: &str,
    sequence: i64,
) -> Result<i64, LificError> {
    statement_issued();
    conn.prepare_cached(
        "SELECT count(*)
           FROM comments c
           JOIN issues i ON i.id = c.issue_id
           JOIN projects p ON p.id = i.project_id
          WHERE p.identifier = ?1
            AND p.is_public = 1
            AND i.sequence = ?2
            AND i.deleted_at IS NULL
            AND c.deleted_at IS NULL
            AND c.issue_id IS NOT NULL",
    )?
    .query_row(params![project_identifier, sequence], |row| row.get(0))
    .map_err(Into::into)
}

/// One page of current comments on one current issue.
///
/// `c.issue_id IS NOT NULL` is redundant beside the join and deliberate: it
/// states in the WHERE clause that page comments are out of scope, so loosening
/// the join later cannot quietly pull them in.
pub fn list_public_comments(
    conn: &Connection,
    project_identifier: &str,
    sequence: i64,
    limit: i64,
    offset: i64,
) -> Result<PublicComments, LificError> {
    let _snapshot = read_snapshot(conn)?;
    let limit = limit.clamp(1, PUBLIC_COMMENT_PAGE);
    let offset = offset.max(0);
    let total = count_public_comments(conn, project_identifier, sequence)?;

    statement_issued();
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT c.id, {}
           FROM comments c
           JOIN issues i ON i.id = c.issue_id
           JOIN projects p ON p.id = i.project_id
          WHERE p.identifier = ?1
            AND p.is_public = 1
            AND i.sequence = ?2
            AND i.deleted_at IS NULL
            AND c.deleted_at IS NULL
            AND c.issue_id IS NOT NULL
       ORDER BY c.created_at, c.id
          LIMIT ?3 OFFSET ?4",
        text_bound(
            &["c.content", "c.created_at", "c.updated_at"],
            COMMENT_BYTES
        ),
    ))?;
    let mut window: Vec<(i64, usize)> = stmt
        .query_map(
            params![project_identifier, sequence, limit + 1, offset],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?.unsigned_abs() as usize,
                ))
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;

    let capped = window.len().min(limit.unsigned_abs() as usize);
    let candidate_ids: Vec<i64> = window[..capped].iter().map(|(id, _)| *id).collect();
    add_child_bounds(
        &mut window[..capped],
        &child_bounds(conn, &candidate_ids, Some("comment"))?,
    );
    let sizes: Vec<usize> = window[..capped].iter().map(|(_, size)| *size).collect();
    let fitted = fitting_prefix(&sizes, ENVELOPE_BYTES, "a comment on this issue")?;
    let has_more = window.len() as i64 > limit || fitted < capped;
    let ids: Vec<i64> = window[..fitted].iter().map(|(id, _)| *id).collect();

    let comments = read_comment_rows(conn, project_identifier, sequence, &ids)?;
    Ok(PublicComments {
        comments,
        total,
        has_more,
    })
}

fn read_comment_rows(
    conn: &Connection,
    project_identifier: &str,
    sequence: i64,
    ids: &[i64],
) -> Result<Vec<PublicComment>, LificError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(",");
    statement_issued();
    let mut stmt = conn.prepare(&format!(
        "SELECT c.id, c.content, c.created_at, c.updated_at
           FROM comments c
           JOIN issues i ON i.id = c.issue_id
           JOIN projects p ON p.id = i.project_id
          WHERE p.identifier = ?1
            AND p.is_public = 1
            AND i.sequence = ?2
            AND i.deleted_at IS NULL
            AND c.deleted_at IS NULL
            AND c.issue_id IS NOT NULL
            AND c.id IN ({placeholders})
       ORDER BY c.created_at, c.id"
    ))?;
    let bound = rusqlite::params_from_iter(
        [
            rusqlite::types::Value::from(project_identifier.to_string()),
            rusqlite::types::Value::from(sequence),
        ]
        .into_iter()
        .chain(ids.iter().copied().map(rusqlite::types::Value::from)),
    );
    let rows = stmt.query_map(bound, |row| {
        materializing_row(conn);
        Ok(PublicComment {
            id: row.get(0)?,
            content: row.get(1)?,
            created_at: row.get(2)?,
            updated_at: row.get(3)?,
            attachments: Vec::new(),
        })
    })?;
    let mut comments: Vec<PublicComment> = rows.collect::<Result<Vec<_>, _>>()?;
    let mut attachments = attachments_for(conn, "comment", ids)?;
    for comment in &mut comments {
        comment.attachments = attachments.remove(&comment.id).unwrap_or_default();
    }
    Ok(comments)
}

// ── Attachments ──────────────────────────────────────────────

/// Complete metadata for preflight-approved entities. Per-attachment overhead
/// bounds the row count as well as bytes; no LIMIT may silently drop links.
fn attachments_for(
    conn: &Connection,
    entity_type: &str,
    entity_ids: &[i64],
) -> Result<HashMap<i64, Vec<PublicAttachment>>, LificError> {
    let mut out: HashMap<i64, Vec<PublicAttachment>> = HashMap::new();
    if entity_ids.is_empty() {
        return Ok(out);
    }
    let placeholders = std::iter::repeat_n("?", entity_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    statement_issued();
    let mut stmt = conn.prepare(&format!(
        "SELECT l.entity_id,
                a.id, a.filename, a.mime, a.size_bytes, a.alt_text, a.width, a.height
           FROM attachments a
           JOIN attachment_links l ON l.attachment_id = a.id
          WHERE l.entity_type = ?1 AND l.entity_id IN ({placeholders})
       ORDER BY l.entity_id, l.created_at, a.id"
    ))?;
    let bound = rusqlite::params_from_iter(
        std::iter::once(rusqlite::types::Value::from(entity_type.to_string()))
            .chain(entity_ids.iter().copied().map(rusqlite::types::Value::from)),
    );
    let rows = stmt.query_map(bound, |row| {
        materializing_row(conn);
        Ok((
            row.get::<_, i64>(0)?,
            PublicAttachment {
                id: row.get(1)?,
                filename: row.get(2)?,
                mime: row.get(3)?,
                size_bytes: row.get(4)?,
                alt_text: row.get(5)?,
                width: row.get(6)?,
                height: row.get(7)?,
            },
        ))
    })?;
    for row in rows {
        let (entity_id, attachment) = row?;
        out.entry(entity_id).or_default().push(attachment);
    }
    Ok(out)
}

/// Resolve an attachment for download within a published project, or `None`.
///
/// An attachment id is a bare integer an anonymous visitor can count through,
/// so the `EXISTS` clause is the whole gate: the attachment must be linked to a
/// live issue, or to a live comment on a live issue, in a project whose
/// identifier is `?1` and whose `is_public` is 1. Orphans have no link row,
/// page links are not enumerated, tombstoned parents fail `deleted_at IS NULL`,
/// and another project's copy fails the identifier comparison. An attachment
/// shared between a private and a published project does match through the
/// published path, correctly: publishing an issue publishes the files its body
/// embeds, which is what the UI warning says.
pub fn get_public_attachment(
    conn: &Connection,
    project_identifier: &str,
    attachment_id: i64,
) -> Result<Option<PublicBlob>, LificError> {
    statement_issued();
    conn.prepare_cached(
        "SELECT a.sha256, substr(a.filename, 1, 200), substr(a.mime, 1, 128)
           FROM attachments a
          WHERE a.id = ?2
            AND EXISTS (
                SELECT 1
                  FROM attachment_links l
                 WHERE l.attachment_id = a.id
                   AND (
                        (l.entity_type = 'issue' AND EXISTS (
                             SELECT 1 FROM issues i
                               JOIN projects p ON p.id = i.project_id
                              WHERE i.id = l.entity_id
                                AND i.deleted_at IS NULL
                                AND p.identifier = ?1
                                AND p.is_public = 1))
                     OR (l.entity_type = 'comment' AND EXISTS (
                             SELECT 1 FROM comments c
                               JOIN issues i ON i.id = c.issue_id
                               JOIN projects p ON p.id = i.project_id
                              WHERE c.id = l.entity_id
                                AND c.deleted_at IS NULL
                                AND i.deleted_at IS NULL
                                AND p.identifier = ?1
                                AND p.is_public = 1))
                   )
            )",
    )?
    .query_row(params![project_identifier, attachment_id], |row| {
        Ok(PublicBlob {
            sha256: row.get(0)?,
            filename: row.get(1)?,
            mime: row.get(2)?,
        })
    })
    .optional()
    .map_err(Into::into)
}

#[cfg(test)]
mod byte_tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> crate::db::DbPool {
        let db = crate::db::open_memory().unwrap();
        db.write()
            .unwrap()
            .execute_batch(
                "INSERT INTO users (username, email, password_hash, display_name)
             VALUES ('owner', 'owner@example.test', 'x', 'Owner');
             INSERT INTO projects (identifier, name, is_public) VALUES ('PUB', 'Public', 1);
             INSERT INTO modules (project_id, name) VALUES (1, 'Module');
             INSERT INTO issues (project_id, sequence, title, module_id) VALUES (1, 1, 'Issue', 1);
             INSERT INTO comments (issue_id, user_id, content) VALUES (1, 1, 'Comment');
             INSERT INTO labels (project_id, name) VALUES (1, 'Label');
             INSERT INTO issue_labels VALUES (1, 1);
             INSERT INTO attachments (sha256, filename, mime, size_bytes)
             VALUES (printf('%064d', 0), 'file', 'image/png', 1);
             INSERT INTO attachment_links (attachment_id, entity_type, entity_id)
             VALUES (1, 'issue', 1), (1, 'comment', 1);
             DROP TRIGGER IF EXISTS issues_updated;
             DROP TRIGGER IF EXISTS comments_updated;
             PRAGMA ignore_check_constraints = ON;",
            )
            .unwrap();
        db
    }

    fn assert_preflight_refuses<T: std::fmt::Debug>(read: impl FnOnce() -> Result<T, LificError>) {
        let before = probe::MATERIALIZED.with(std::cell::Cell::get);
        let error = read().unwrap_err();
        assert!(matches!(error, LificError::PayloadTooLarge(_)), "{error:?}");
        assert_eq!(
            probe::MATERIALIZED.with(std::cell::Cell::get),
            before,
            "oversized metadata reached a Rust row mapper"
        );
    }

    fn assert_json_fits(value: &impl Serialize) -> usize {
        let bytes = serde_json::to_vec(value).unwrap().len();
        assert!(bytes <= PUBLIC_PAGE_BYTES, "serialized {bytes} bytes");
        bytes
    }

    #[derive(Serialize)]
    struct DetailEnvelope {
        project: PublicProject,
        #[serde(flatten)]
        issue: PublicIssueDetail,
    }

    #[test]
    fn every_text_field_is_preflighted_as_bytes_before_materialization() {
        let db = fixture();
        let conn = db.write().unwrap();
        for text in [
            "\0".repeat(90_000),
            "🦀".repeat(25_000),
            "\u{1}".repeat(90_000),
        ] {
            for field in ["identifier", "name", "description", "emoji"] {
                conn.execute_batch("SAVEPOINT field").unwrap();
                conn.execute(
                    &format!("UPDATE projects SET {field} = ?1 WHERE id = 1"),
                    [&text],
                )
                .unwrap();
                let identifier = if field == "identifier" { &text } else { "PUB" };
                assert_preflight_refuses(|| get_public_project(&conn, identifier));
                assert_preflight_refuses(|| list_public_issues(&conn, identifier, 100, 0));
                assert_preflight_refuses(|| get_public_issue(&conn, identifier, 1));
                conn.execute_batch("ROLLBACK TO field; RELEASE field")
                    .unwrap();
            }
            for (table, field) in [
                ("issues", "title"),
                ("issues", "status"),
                ("issues", "priority"),
                ("issues", "created_at"),
                ("issues", "updated_at"),
                ("modules", "name"),
                ("labels", "name"),
            ] {
                conn.execute_batch("SAVEPOINT field").unwrap();
                conn.execute(
                    &format!("UPDATE {table} SET {field} = ?1 WHERE id = 1"),
                    [&text],
                )
                .unwrap();
                assert_preflight_refuses(|| list_public_issues(&conn, "PUB", 100, 0));
                assert_preflight_refuses(|| get_public_issue(&conn, "PUB", 1));
                conn.execute_batch("ROLLBACK TO field; RELEASE field")
                    .unwrap();
            }
            conn.execute("UPDATE issues SET description = ?1 WHERE id = 1", [&text])
                .unwrap();
            assert_preflight_refuses(|| get_public_issue(&conn, "PUB", 1));
            conn.execute("UPDATE issues SET description = '' WHERE id = 1", [])
                .unwrap();
            for field in ["content", "created_at", "updated_at"] {
                conn.execute_batch("SAVEPOINT field").unwrap();
                conn.execute(
                    &format!("UPDATE comments SET {field} = ?1 WHERE id = 1"),
                    [&text],
                )
                .unwrap();
                assert_preflight_refuses(|| list_public_comments(&conn, "PUB", 1, 50, 0));
                conn.execute_batch("ROLLBACK TO field; RELEASE field")
                    .unwrap();
            }
            for field in ["filename", "mime", "alt_text"] {
                conn.execute_batch("SAVEPOINT field").unwrap();
                conn.execute(
                    &format!("UPDATE attachments SET {field} = ?1 WHERE id = 1"),
                    [&text],
                )
                .unwrap();
                assert_preflight_refuses(|| get_public_issue(&conn, "PUB", 1));
                assert_preflight_refuses(|| list_public_comments(&conn, "PUB", 1, 50, 0));
                conn.execute_batch("ROLLBACK TO field; RELEASE field")
                    .unwrap();
            }
        }
    }

    #[test]
    fn real_envelopes_fit_with_escaped_utf8_metadata_nulls_and_integer_extremes() {
        let db = fixture();
        let conn = db.write().unwrap();
        let text = "\0🦀\u{1}\"\\\n".repeat(150);
        conn.execute(
            "UPDATE projects SET identifier = ?1, name = ?1, description = ?1, emoji = ?1",
            [&text],
        )
        .unwrap();
        conn.execute("UPDATE issues SET title = ?1, status = ?1, priority = ?1, created_at = ?1, updated_at = ?1", [&text]).unwrap();
        conn.execute("UPDATE modules SET name = ?1", [&text])
            .unwrap();
        conn.execute("UPDATE labels SET name = ?1", [&text])
            .unwrap();
        conn.execute(
            "UPDATE comments SET created_at = ?1, updated_at = ?1",
            [&text],
        )
        .unwrap();
        conn.execute("UPDATE attachments SET filename = ?1, mime = ?1, alt_text = ?1, width = ?2, height = ?3, size_bytes = ?3",
                     params![text, i64::MAX, i64::MIN]).unwrap();
        conn.execute("UPDATE issues SET description = ?1", ["\0".repeat(50_000)])
            .unwrap();
        conn.execute("UPDATE comments SET content = ?1", ["\0".repeat(60_000)])
            .unwrap();

        let tx = conn.unchecked_transaction().unwrap();
        let project = get_public_project(&tx, &text).unwrap().unwrap();
        assert_json_fits(&json!({"project": project}));
        let (issues, has_more) = list_public_issues(&tx, &text, 100, 0).unwrap();
        assert_eq!(issues[0].labels, vec![text.clone()]);
        assert_json_fits(&json!({"project": project, "issues": issues, "limit": 100,
                                "offset": i64::MAX, "has_more": has_more}));
        let issue = get_public_issue(&tx, &text, 1).unwrap().unwrap();
        assert_eq!(issue.attachments[0].height, Some(i64::MIN));
        let size = assert_json_fits(&DetailEnvelope { project, issue });
        assert!(size > 300_000);
        let page = list_public_comments(&tx, &text, 1, 50, 0).unwrap();
        assert_json_fits(&json!({"comments": page.comments, "total": page.total,
            "limit": 50, "offset": i64::MAX, "has_more": page.has_more}));
        drop(tx);
        assert!(conn.is_autocommit());

        conn.execute_batch(
            "UPDATE projects SET emoji = NULL;
            UPDATE issues SET module_id = NULL;
            UPDATE attachments SET alt_text = NULL, width = NULL, height = NULL;",
        )
        .unwrap();
        let project = get_public_project(&conn, &text).unwrap().unwrap();
        let issue = get_public_issue(&conn, &text, 1).unwrap().unwrap();
        assert_eq!(project.emoji, None);
        assert_eq!(issue.issue.module, None);
        assert_eq!(issue.attachments[0].width, None);
        assert_json_fits(&DetailEnvelope { project, issue });
        assert!(
            conn.is_autocommit(),
            "standalone reads must release their snapshot"
        );
    }

    #[test]
    fn project_envelope_is_reserved_when_fitting_issue_pages_and_details() {
        let db = fixture();
        let conn = db.write().unwrap();
        conn.execute(
            "UPDATE projects SET description = ?1",
            ["\0".repeat(40_000)],
        )
        .unwrap();
        conn.execute("UPDATE issues SET title = ?1", ["\0".repeat(30_000)])
            .unwrap();
        conn.execute(
            "INSERT INTO issues (project_id, sequence, title) VALUES (1, 2, ?1)",
            ["\0".repeat(30_000)],
        )
        .unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        let project = get_public_project(&tx, "PUB").unwrap().unwrap();
        let (issues, has_more) = list_public_issues(&tx, "PUB", 100, 0).unwrap();
        assert_eq!(issues.len(), 1);
        assert!(has_more);
        assert_json_fits(&json!({"project": project, "issues": issues, "limit": 100,
                                "offset": 0, "has_more": has_more}));
        let (next, more) = list_public_issues(&tx, "PUB", 100, issues.len() as i64).unwrap();
        assert_eq!(next[0].identifier, "PUB-2");
        assert!(!more);
        assert_json_fits(&json!({"project": project, "issues": next, "limit": 100,
                                "offset": 1, "has_more": more}));
        drop(tx);
        conn.execute(
            "UPDATE issues SET description = ?1 WHERE id = 1",
            ["\0".repeat(30_000)],
        )
        .unwrap();
        assert_preflight_refuses(|| get_public_issue(&conn, "PUB", 1));
        conn.execute(
            "UPDATE projects SET description = ?1",
            ["\0".repeat(60_000)],
        )
        .unwrap();
        assert_preflight_refuses(|| list_public_issues(&conn, "PUB", 100, 0));
    }

    #[test]
    fn comment_prefix_includes_complete_attachments_and_never_skips_a_giant_row() {
        let db = fixture();
        let conn = db.write().unwrap();
        conn.execute(
            "UPDATE attachments SET alt_text = ?1",
            ["\0".repeat(50_000)],
        )
        .unwrap();
        conn.execute_batch("INSERT INTO comments (issue_id, user_id, content) VALUES (1, 1, 'second'), (1, 1, 'third');
            INSERT INTO attachment_links (attachment_id, entity_type, entity_id) VALUES (1, 'comment', 2);").unwrap();
        let first = list_public_comments(&conn, "PUB", 1, 50, 0).unwrap();
        assert_eq!(first.comments.len(), 1);
        assert_eq!(first.comments[0].attachments.len(), 1);
        assert!(first.has_more);
        let next = list_public_comments(&conn, "PUB", 1, 50, first.comments.len() as i64).unwrap();
        assert_eq!(
            next.comments.iter().map(|c| c.id).collect::<Vec<_>>(),
            [2, 3]
        );
        assert!(!next.has_more);
        for (offset, page) in [(0, first), (1, next)] {
            assert_json_fits(&json!({"comments": page.comments, "total": page.total,
                "limit": 50, "offset": offset, "has_more": page.has_more}));
        }
        conn.execute(
            "UPDATE comments SET content = ?1 WHERE id = 2",
            ["\0".repeat(90_000)],
        )
        .unwrap();
        let first = list_public_comments(&conn, "PUB", 1, 50, 0).unwrap();
        assert_eq!(first.comments.len(), 1);
        assert!(first.has_more);
        assert_preflight_refuses(|| list_public_comments(&conn, "PUB", 1, 50, 1));
    }

    #[test]
    fn more_than_two_hundred_attachments_are_complete_or_explicitly_refused() {
        let db = fixture();
        let conn = db.write().unwrap();
        conn.execute_batch(
            "WITH RECURSIVE n(x) AS (VALUES(2) UNION ALL SELECT x+1 FROM n WHERE x<201)
            INSERT INTO attachments (id, sha256, filename, mime, size_bytes)
            SELECT x, 'hash', 'file', 'image/png', 1 FROM n;
            INSERT INTO attachment_links (attachment_id, entity_type, entity_id)
            SELECT id, 'issue', 1 FROM attachments WHERE id > 1;
            INSERT INTO attachment_links (attachment_id, entity_type, entity_id)
            SELECT id, 'comment', 1 FROM attachments WHERE id > 1;",
        )
        .unwrap();
        let issue = get_public_issue(&conn, "PUB", 1).unwrap().unwrap();
        assert_eq!(issue.attachments.len(), 201);
        assert_json_fits(&DetailEnvelope {
            project: get_public_project(&conn, "PUB").unwrap().unwrap(),
            issue,
        });
        let page = list_public_comments(&conn, "PUB", 1, 50, 0).unwrap();
        assert_eq!(page.comments[0].attachments.len(), 201);
        assert_json_fits(&json!({"comments": page.comments, "total": page.total,
            "limit": 50, "offset": 0, "has_more": page.has_more}));
        conn.execute_batch(
            "WITH RECURSIVE n(x) AS (VALUES(202) UNION ALL SELECT x+1 FROM n WHERE x<2200)
            INSERT INTO attachments (id, sha256, filename, mime, size_bytes)
            SELECT x, 'hash', '', '', 1 FROM n;
            INSERT INTO attachment_links (attachment_id, entity_type, entity_id)
            SELECT id, 'issue', 1 FROM attachments WHERE id > 201;
            INSERT INTO attachment_links (attachment_id, entity_type, entity_id)
            SELECT id, 'comment', 1 FROM attachments WHERE id > 201;",
        )
        .unwrap();
        assert_preflight_refuses(|| get_public_issue(&conn, "PUB", 1));
        assert_preflight_refuses(|| list_public_comments(&conn, "PUB", 1, 50, 0));
    }

    #[test]
    fn excessive_label_count_is_refused_before_loading_the_labels() {
        let db = fixture();
        let conn = db.write().unwrap();
        conn.execute_batch(
            "WITH RECURSIVE n(x) AS (VALUES(2) UNION ALL SELECT x+1 FROM n WHERE x<17000)
            INSERT INTO labels (id, project_id, name) SELECT x, 1, CAST(x AS TEXT) FROM n;
            INSERT INTO issue_labels SELECT 1, id FROM labels WHERE id > 1;",
        )
        .unwrap();
        assert_preflight_refuses(|| list_public_issues(&conn, "PUB", 100, 0));
        assert_preflight_refuses(|| get_public_issue(&conn, "PUB", 1));
    }

    #[test]
    fn a_second_connection_cannot_change_metadata_between_snapshot_reads() {
        let db = fixture();
        let reader = db.read().unwrap();
        let writer = db.write().unwrap();
        let snapshot = read_snapshot(&reader).unwrap();
        let original = project_bound(&reader, "PUB").unwrap();
        let growth = "\0".repeat(90_000);
        let error = writer
            .execute("UPDATE projects SET name = ?1", [&growth])
            .unwrap_err();
        assert_eq!(
            error.sqlite_error_code(),
            Some(rusqlite::ErrorCode::DatabaseLocked)
        );
        assert_eq!(project_bound(&reader, "PUB").unwrap(), original);
        assert_eq!(
            get_public_project(&reader, "PUB").unwrap().unwrap().name,
            "Public"
        );
        assert!(
            !reader.is_autocommit(),
            "nested read must retain the outer snapshot"
        );
        drop(snapshot);
        writer
            .execute("UPDATE projects SET name = ?1", [&growth])
            .unwrap();
        assert_preflight_refuses(|| get_public_project(&reader, "PUB"));
        assert!(
            reader.is_autocommit(),
            "failed reads must release their snapshot"
        );
    }

    #[test]
    fn exact_budget_is_inclusive_and_fixed_allowances_cover_serialized_fields() {
        assert_eq!(
            fitting_prefix(&[PUBLIC_PAGE_BYTES - ENVELOPE_BYTES], ENVELOPE_BYTES, "row").unwrap(),
            1
        );
        assert!(
            fitting_prefix(
                &[PUBLIC_PAGE_BYTES - ENVELOPE_BYTES + 1],
                ENVELOPE_BYTES,
                "row"
            )
            .is_err()
        );
        let attachment = PublicAttachment {
            id: i64::MIN,
            filename: String::new(),
            mime: String::new(),
            size_bytes: i64::MIN,
            alt_text: None,
            width: Some(i64::MIN),
            height: Some(i64::MIN),
        };
        assert!(serde_json::to_vec(&attachment).unwrap().len() < ATTACHMENT_BYTES);
        let comment = PublicComment {
            id: i64::MIN,
            content: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
            attachments: vec![],
        };
        assert!(serde_json::to_vec(&comment).unwrap().len() < COMMENT_BYTES);
        let project = PublicProject {
            identifier: String::new(),
            name: String::new(),
            description: String::new(),
            emoji: None,
        };
        let issue = PublicIssueDetail {
            issue: PublicIssue {
                identifier: String::new(),
                title: String::new(),
                status: String::new(),
                priority: String::new(),
                module: None,
                labels: vec![],
                created_at: String::new(),
                updated_at: String::new(),
            },
            description: String::new(),
            attachments: vec![],
        };
        assert!(serde_json::to_vec(&project).unwrap().len() < PROJECT_BYTES);
        assert!(serde_json::to_vec(&issue).unwrap().len() < ISSUE_BYTES);
        let project_size = serde_json::to_vec(&project).unwrap().len();
        let issue_size = serde_json::to_vec(&issue).unwrap().len();
        let envelope = DetailEnvelope { project, issue };
        assert!(
            serde_json::to_vec(&envelope).unwrap().len()
                < project_size + issue_size + ENVELOPE_BYTES
        );
        let list_envelope = json!({"project": {}, "issues": [], "total": i64::MIN,
            "limit": i64::MIN, "offset": i64::MIN, "has_more": false});
        let comment_envelope = json!({"comments": [], "total": i64::MIN,
            "limit": i64::MIN, "offset": i64::MIN, "has_more": false});
        assert!(serde_json::to_vec(&list_envelope).unwrap().len() < ENVELOPE_BYTES);
        assert!(serde_json::to_vec(&comment_envelope).unwrap().len() < ENVELOPE_BYTES);
    }
}
