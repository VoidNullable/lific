//! LIF-439: delta backfill and cold-start bootstrap for a syncing client.
//!
//! Migration 045 gave issues, pages and comments one instance-wide monotonic
//! `seq`, and migration 047 made deletes leave a tombstone behind instead of
//! removing the row. Together they make "everything in this project above
//! seq N" a complete description of what a replica missed, deletions
//! included. This module is the read side of that.
//!
//! Two entry points:
//!
//!   * [`list_changes`] — the delta. Issues, pages and comments merged into
//!     one seq-ascending stream, tombstones included, paginated by a cursor
//!     that is just the last seq handed out.
//!   * [`get_index`] — the bootstrap. Live rows only, plus the cursor a
//!     client should resume [`list_changes`] from. See that function's doc
//!     comment for the ordering rule its correctness rests on.
//!
//! Every row is *skinny*: no issue description, no page content, no comment
//! body. A list view never needs them, and shipping them would make the
//! bootstrap proportional to the total prose in the project rather than to
//! the number of rows. Clients fetch bodies from the existing detail
//! endpoints on demand.
//!
//! The one concession is `preview`: the first non-empty line of an issue's
//! description or a page's content, capped at [`PREVIEW_CHARS`]. List rows
//! render one, and making a client fetch every body to draw a single line
//! would cost far more than the bounded string does. The body column is
//! read, reduced to that line, and dropped before serialization — it never
//! reaches the wire.

use std::collections::HashMap;

use rusqlite::{Connection, Row, params};

use crate::db::models::*;
use crate::error::LificError;

/// Page size when a caller does not ask for one. Far above the 50-row
/// default the browsing endpoints use: this is a machine catching up, not a
/// human reading a page, and every round trip is a client that stays stale
/// slightly longer.
pub const DEFAULT_CHANGES_LIMIT: i64 = 5_000;

/// Hard cap on a single delta page.
pub const MAX_CHANGES_LIMIT: i64 = 50_000;

/// Cap on a row's `preview`, in Unicode scalar values (not bytes — slicing
/// bytes would panic mid-codepoint on any non-ASCII body). Long enough for
/// a list row's one-line summary at any density, short enough that 5,000 of
/// them stay a rounding error next to the bodies they stand in for.
pub const PREVIEW_CHARS: usize = 200;

/// The first non-empty line of `text`, trimmed and truncated to
/// [`PREVIEW_CHARS`] characters. Empty when `text` has no non-blank line,
/// which covers both an empty body and one that is only whitespace.
fn preview_of(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(PREVIEW_CHARS).collect())
        .unwrap_or_default()
}

/// Clamp a caller-supplied `limit` into `1..=MAX_CHANGES_LIMIT`, defaulting
/// to [`DEFAULT_CHANGES_LIMIT`] when absent. Floors at 1 for the same reason
/// [`super::page`] does: SQLite reads `LIMIT -1` as "no limit", so an
/// unclamped `?limit=-1` would dump every change in the project.
pub fn clamp_changes_limit(limit: Option<i64>) -> i64 {
    limit
        .unwrap_or(DEFAULT_CHANGES_LIMIT)
        .clamp(1, MAX_CHANGES_LIMIT)
}

// ── The wide row ─────────────────────────────────────────────
//
// Issues, pages and comments are merged by a UNION ALL, which means every
// branch has to present the same column list; the columns a branch has no
// value for are NULL. The alternative — select `(kind, seq, id)` first and
// then fan out to three detail queries — costs three more round trips and a
// second pass to restore the merge order, for a query that already knows
// everything it needs on the first pass.
//
// The mappers below read only the columns their own `kind` populated, so the
// NULLs are never converted.

const KIND: usize = 0;
const SEQ: usize = 1;
const DELETED: usize = 2;
const ID: usize = 3;
const PROJECT_IDENTIFIER: usize = 4;
const SEQUENCE: usize = 5;
const TITLE: usize = 6;
const STATUS: usize = 7;
const PRIORITY: usize = 8;
const MODULE_ID: usize = 9;
const SORT_ORDER: usize = 10;
const START_DATE: usize = 11;
const TARGET_DATE: usize = 12;
const FOLDER_ID: usize = 13;
const PINNED: usize = 14;
const ISSUE_ID: usize = 15;
const PAGE_ID: usize = 16;
const USER_ID: usize = 17;
const USERNAME: usize = 18;
const CREATED_AT: usize = 19;
const UPDATED_AT: usize = 20;
/// The body column — `issues.description` or `pages.content`. Read only to
/// derive [`preview_of`]; never stored on a change row.
const BODY: usize = 21;

const ISSUE_COLUMNS: &str = "'issue' AS kind, i.seq AS seq, (i.deleted_at IS NOT NULL) AS deleted,
            i.id AS id, p.identifier AS project_identifier, i.sequence AS sequence,
            i.title AS title, i.status AS status, i.priority AS priority,
            i.module_id AS module_id, i.sort_order AS sort_order,
            i.start_date AS start_date, i.target_date AS target_date,
            NULL AS folder_id, NULL AS pinned,
            NULL AS issue_id, NULL AS page_id, NULL AS user_id, NULL AS username,
            i.created_at AS created_at, i.updated_at AS updated_at,
            i.description AS body";

const ISSUE_FROM: &str = "FROM issues i JOIN projects p ON p.id = i.project_id";

const PAGE_COLUMNS: &str = "'page', pg.seq, (pg.deleted_at IS NOT NULL),
            pg.id, p.identifier, pg.sequence,
            pg.title, pg.status, NULL,
            NULL, NULL,
            NULL, NULL,
            pg.folder_id, pg.pinned,
            NULL, NULL, NULL, NULL,
            pg.created_at, pg.updated_at,
            pg.content";

const PAGE_FROM: &str = "FROM pages pg JOIN projects p ON p.id = pg.project_id";

const COMMENT_COLUMNS: &str = "'comment', c.seq, (c.deleted_at IS NOT NULL),
            c.id, NULL, NULL,
            NULL, NULL, NULL,
            NULL, NULL,
            NULL, NULL,
            NULL, NULL,
            c.issue_id, c.page_id, COALESCE(c.user_id, -1), COALESCE(c.imported_author, u.username),
            c.created_at, c.updated_at,
            NULL";

const COMMENT_FROM: &str = "FROM comments c LEFT JOIN users u ON u.id = c.user_id";

/// A comment carries no `project_id` of its own — it hangs off an issue XOR a
/// page — so its scope is recovered from whichever parent it has. The parent
/// lookup deliberately ignores `deleted_at`: a tombstoned parent keeps its
/// row (migration 047), so a comment orphaned by a cascade still resolves to
/// the project whose replica needs to hear about it.
const COMMENT_PROJECT_SCOPE: &str = "COALESCE(
        (SELECT project_id FROM issues WHERE id = c.issue_id),
        (SELECT project_id FROM pages  WHERE id = c.page_id)
    ) = ?1";

fn issue_change(row: &Row) -> rusqlite::Result<IssueChange> {
    let project_identifier: String = row.get(PROJECT_IDENTIFIER)?;
    let sequence: i64 = row.get(SEQUENCE)?;
    Ok(IssueChange {
        kind: ChangeKind::Issue,
        seq: row.get(SEQ)?,
        deleted: false,
        id: row.get(ID)?,
        identifier: format!("{project_identifier}-{sequence}"),
        title: row.get(TITLE)?,
        status: row.get(STATUS)?,
        priority: row.get(PRIORITY)?,
        module_id: row.get(MODULE_ID)?,
        sort_order: row.get(SORT_ORDER)?,
        start_date: row.get(START_DATE)?,
        target_date: row.get(TARGET_DATE)?,
        created_at: row.get(CREATED_AT)?,
        updated_at: row.get(UPDATED_AT)?,
        preview: preview_of(&row.get::<_, Option<String>>(BODY)?.unwrap_or_default()),
        labels: Vec::new(),
    })
}

fn page_change(row: &Row) -> rusqlite::Result<PageChange> {
    let project_identifier: String = row.get(PROJECT_IDENTIFIER)?;
    let sequence: i64 = row.get(SEQUENCE)?;
    Ok(PageChange {
        kind: ChangeKind::Page,
        seq: row.get(SEQ)?,
        deleted: false,
        id: row.get(ID)?,
        identifier: format!("{project_identifier}-DOC-{sequence}"),
        title: row.get(TITLE)?,
        status: row.get(STATUS)?,
        folder_id: row.get(FOLDER_ID)?,
        pinned: row.get(PINNED)?,
        created_at: row.get(CREATED_AT)?,
        updated_at: row.get(UPDATED_AT)?,
        preview: preview_of(&row.get::<_, Option<String>>(BODY)?.unwrap_or_default()),
        labels: Vec::new(),
    })
}

fn comment_change(row: &Row) -> rusqlite::Result<CommentChange> {
    Ok(CommentChange {
        kind: ChangeKind::Comment,
        seq: row.get(SEQ)?,
        deleted: false,
        id: row.get(ID)?,
        issue_id: row.get(ISSUE_ID)?,
        page_id: row.get(PAGE_ID)?,
        user_id: row.get(USER_ID)?,
        username: row.get(USERNAME)?,
        created_at: row.get(CREATED_AT)?,
        updated_at: row.get(UPDATED_AT)?,
    })
}

/// Turn one merged row into a [`Change`]. A tombstoned row short-circuits to
/// [`Tombstone`]: identity, position in the stream, and the fact that it is
/// gone. Nothing else is meaningful about a deleted row, and a client that
/// receives one deletes its copy rather than merging fields into it.
fn change_from_row(row: &Row) -> rusqlite::Result<Change> {
    let kind: String = row.get(KIND)?;
    let deleted: bool = row.get(DELETED)?;
    let kind = match kind.as_str() {
        "issue" => ChangeKind::Issue,
        "page" => ChangeKind::Page,
        _ => ChangeKind::Comment,
    };
    if deleted {
        return Ok(Change::Tombstone(Tombstone {
            kind,
            seq: row.get(SEQ)?,
            deleted: true,
            id: row.get(ID)?,
        }));
    }
    Ok(match kind {
        ChangeKind::Issue => Change::Issue(issue_change(row)?),
        ChangeKind::Page => Change::Page(page_change(row)?),
        ChangeKind::Comment => Change::Comment(comment_change(row)?),
    })
}

/// Label names per owner id, in one round trip. Mirrors `list_issues`'s and
/// `list_pages`'s grouped lookups rather than issuing a query per row — a
/// 5,000-row delta page would otherwise be 5,000 extra statements.
///
/// `join_table` and `owner_column` are the only difference between the
/// issue and page variants, and both are compile-time constants from the
/// two call sites below, never caller input.
fn labels_by_owner(
    conn: &Connection,
    ids: &[i64],
    join_table: &str,
    owner_column: &str,
) -> Result<HashMap<i64, Vec<String>>, LificError> {
    let mut by_owner: HashMap<i64, Vec<String>> = HashMap::new();
    if ids.is_empty() {
        return Ok(by_owner);
    }
    let placeholders: String = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT j.{owner_column}, l.name FROM {join_table} j
         JOIN labels l ON l.id = j.label_id
         WHERE j.{owner_column} IN ({placeholders})
         ORDER BY l.name"
    );
    let boxed: Vec<Box<dyn rusqlite::types::ToSql>> = ids
        .iter()
        .map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>)
        .collect();
    let refs: Vec<&dyn rusqlite::types::ToSql> = boxed.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(refs.as_slice(), |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (owner_id, name) = row?;
        by_owner.entry(owner_id).or_default().push(name);
    }
    Ok(by_owner)
}

fn labels_by_issue(
    conn: &Connection,
    ids: &[i64],
) -> Result<HashMap<i64, Vec<String>>, LificError> {
    labels_by_owner(conn, ids, "issue_labels", "issue_id")
}

fn labels_by_page(conn: &Connection, ids: &[i64]) -> Result<HashMap<i64, Vec<String>>, LificError> {
    labels_by_owner(conn, ids, "page_labels", "page_id")
}

/// Everything in `project_id` that changed above `since`, oldest first.
///
/// Tombstones are included — that is the entire point of the endpoint. So
/// are comments, scoped through whichever parent (issue or page) they hang
/// off. `since = 0` means "everything", which is legal but is what
/// [`get_index`] exists to do better.
///
/// `limit` is over-fetched by one so `has_more` is answered without a second
/// COUNT, exactly like [`super::Page::from_over_fetch`]. The returned cursor
/// is the last seq actually handed out, so resuming from it is gapless: the
/// stream is totally ordered by an instance-wide counter, and seqs are never
/// reused.
pub fn list_changes(
    conn: &Connection,
    project_id: i64,
    since: i64,
    limit: i64,
) -> Result<ChangesPage, LificError> {
    // Negative `since` would be harmless here (nothing has a negative seq)
    // but floor it anyway so the cursor echoed back on an empty page can
    // never move a client backwards.
    let since = since.max(0);
    let limit = limit.max(1);

    let sql = format!(
        "SELECT * FROM (
             SELECT {ISSUE_COLUMNS} {ISSUE_FROM}
              WHERE i.project_id = ?1 AND i.seq > ?2
             UNION ALL
             SELECT {PAGE_COLUMNS} {PAGE_FROM}
              WHERE pg.project_id = ?1 AND pg.seq > ?2
             UNION ALL
             SELECT {COMMENT_COLUMNS} {COMMENT_FROM}
              WHERE c.seq > ?2 AND {COMMENT_PROJECT_SCOPE}
         )
         ORDER BY seq ASC
         LIMIT ?3"
    );

    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(params![project_id, since, limit.saturating_add(1)], |row| {
        change_from_row(row)
    })?;
    let super::Page {
        items: mut changes,
        has_more,
    } = super::Page::from_over_fetch(rows.collect::<Result<Vec<_>, _>>()?, limit);

    // Only live rows need labels; a tombstone carries no fields at all.
    // One grouped query per kind, not one per row.
    let issue_ids: Vec<i64> = changes
        .iter()
        .filter_map(|change| match change {
            Change::Issue(issue) => Some(issue.id),
            _ => None,
        })
        .collect();
    let page_ids: Vec<i64> = changes
        .iter()
        .filter_map(|change| match change {
            Change::Page(page) => Some(page.id),
            _ => None,
        })
        .collect();
    let mut issue_labels = labels_by_issue(conn, &issue_ids)?;
    let mut page_labels = labels_by_page(conn, &page_ids)?;
    for change in &mut changes {
        match change {
            Change::Issue(issue) => {
                issue.labels = issue_labels.remove(&issue.id).unwrap_or_default();
            }
            Change::Page(page) => {
                page.labels = page_labels.remove(&page.id).unwrap_or_default();
            }
            _ => {}
        }
    }

    let cursor = changes.last().map_or(since, Change::seq);
    Ok(ChangesPage {
        changes,
        cursor,
        has_more,
    })
}

/// The instance-wide watermark a bootstrap should resume from.
///
/// `sync_seq.value` is the counter every stamp trigger draws from, so it is
/// equal to the highest seq handed out anywhere and is never behind one.
/// Reading the counter rather than `MAX(seq)` over three tables is both
/// cheaper and impossible to get subtly wrong (a `MAX` that missed a table
/// would under-report and strand rows).
pub fn index_cursor(conn: &Connection) -> Result<i64, LificError> {
    Ok(conn
        .prepare_cached("SELECT value FROM sync_seq WHERE id = 1")?
        .query_row([], |row| row.get::<_, i64>(0))
        .unwrap_or(0))
}

/// The live issues and pages of one project, in the same skinny shape
/// [`list_changes`] emits. Deliberately separate from [`index_cursor`] so
/// that the ordering [`get_index`] depends on is a visible property of the
/// call site rather than a comment inside one long function.
pub fn index_rows(
    conn: &Connection,
    project_id: i64,
) -> Result<(Vec<IssueChange>, Vec<PageChange>), LificError> {
    let issue_sql = format!(
        "SELECT {ISSUE_COLUMNS} {ISSUE_FROM}
          WHERE i.project_id = ?1 AND i.deleted_at IS NULL
          ORDER BY i.seq ASC"
    );
    let mut stmt = conn.prepare_cached(&issue_sql)?;
    let mut issues = stmt
        .query_map(params![project_id], issue_change)?
        .collect::<Result<Vec<_>, _>>()?;

    let page_sql = format!(
        "SELECT {PAGE_COLUMNS} {PAGE_FROM}
          WHERE pg.project_id = ?1 AND pg.deleted_at IS NULL
          ORDER BY pg.seq ASC"
    );
    let mut stmt = conn.prepare_cached(&page_sql)?;
    let mut pages = stmt
        .query_map(params![project_id], page_change)?
        .collect::<Result<Vec<_>, _>>()?;

    let issue_ids: Vec<i64> = issues.iter().map(|issue| issue.id).collect();
    let mut issue_labels = labels_by_issue(conn, &issue_ids)?;
    for issue in &mut issues {
        issue.labels = issue_labels.remove(&issue.id).unwrap_or_default();
    }

    let page_ids: Vec<i64> = pages.iter().map(|page| page.id).collect();
    let mut page_labels = labels_by_page(conn, &page_ids)?;
    for page in &mut pages {
        page.labels = page_labels.remove(&page.id).unwrap_or_default();
    }

    Ok((issues, pages))
}

/// Cold-start bootstrap: every live issue and page in the project, plus the
/// cursor to resume [`list_changes`] from.
///
/// ── The ordering rule (non-negotiable) ────────────────────────────────
/// The cursor is read BEFORE the lists, and that order is the whole
/// correctness argument. A write landing between the two reads is delivered
/// twice — once in the list, once again by the next `/changes` call, because
/// its seq is above the cursor we already took. Duplicate delivery is
/// harmless: a replica applies changes as upserts keyed by id.
///
/// Reversing the order swaps that harmless duplicate for permanent data
/// loss. Lists first, cursor second, and a row written in between is absent
/// from the snapshot *and* below the cursor, so no future `/changes` call
/// ever mentions it. The replica is silently missing a row until something
/// else happens to touch it. That is why [`index_cursor`] and
/// [`index_rows`] are separate functions called in this order here, rather
/// than one query that could be reordered without anyone noticing.
pub fn get_index(conn: &Connection, project_id: i64) -> Result<IndexSnapshot, LificError> {
    // Cursor first. See the doc comment above before changing this.
    let cursor = index_cursor(conn)?;
    let (issues, pages) = index_rows(conn, project_id)?;
    Ok(IndexSnapshot {
        cursor,
        issues,
        pages,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbPool;

    fn seed() -> (DbPool, i64, i64) {
        let db = crate::db::open_memory().expect("test db");
        let (project, other) = {
            let conn = db.write().unwrap();
            let user = crate::db::queries::users::create_user(
                &conn,
                &CreateUser {
                    username: "syncer".into(),
                    email: "syncer@test.com".into(),
                    password: "testpassword1".into(),
                    display_name: None,
                    is_admin: true,
                    is_bot: false,
                },
            )
            .unwrap();
            let project = crate::db::queries::create_project(
                &conn,
                &CreateProject {
                    name: "Sync".into(),
                    identifier: "SYN".into(),
                    lead_user_id: Some(user.id),
                    ..Default::default()
                },
            )
            .unwrap();
            let other = crate::db::queries::create_project(
                &conn,
                &CreateProject {
                    name: "Other".into(),
                    identifier: "OTH".into(),
                    lead_user_id: Some(user.id),
                    ..Default::default()
                },
            )
            .unwrap();
            (project.id, other.id)
        };
        (db, project, other)
    }

    fn new_issue(db: &DbPool, project_id: i64, title: &str) -> Issue {
        let conn = db.write().unwrap();
        crate::db::queries::create_issue(
            &conn,
            &CreateIssue {
                project_id,
                title: title.into(),
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn clamp_changes_limit_defaults_floors_and_caps() {
        assert_eq!(clamp_changes_limit(None), DEFAULT_CHANGES_LIMIT);
        assert_eq!(clamp_changes_limit(Some(10)), 10);
        assert_eq!(clamp_changes_limit(Some(0)), 1);
        assert_eq!(clamp_changes_limit(Some(-1)), 1);
        assert_eq!(clamp_changes_limit(Some(i64::MAX)), MAX_CHANGES_LIMIT);
        assert_eq!(
            clamp_changes_limit(Some(MAX_CHANGES_LIMIT + 1)),
            MAX_CHANGES_LIMIT
        );
    }

    #[test]
    fn changes_are_ordered_ascending_by_seq() {
        let (db, project, _) = seed();
        new_issue(&db, project, "one");
        new_issue(&db, project, "two");
        new_issue(&db, project, "three");

        let conn = db.read().unwrap();
        let page = list_changes(&conn, project, 0, 100).unwrap();
        let seqs: Vec<i64> = page.changes.iter().map(Change::seq).collect();
        let mut sorted = seqs.clone();
        sorted.sort_unstable();
        assert_eq!(seqs, sorted);
        assert_eq!(page.cursor, *seqs.last().unwrap());
        assert!(!page.has_more);
    }

    /// A deleted row must arrive as a tombstone rather than simply stopping
    /// to appear — that distinction is the reason migration 047 exists.
    #[test]
    fn deleting_an_issue_emits_a_tombstone_above_the_previous_cursor() {
        let (db, project, _) = seed();
        let issue = new_issue(&db, project, "doomed");
        let before = {
            let conn = db.read().unwrap();
            index_cursor(&conn).unwrap()
        };
        {
            let conn = db.write().unwrap();
            crate::db::queries::delete_issue(&conn, issue.id).unwrap();
        }

        let conn = db.read().unwrap();
        let page = list_changes(&conn, project, before, 100).unwrap();
        let tombstones: Vec<&Change> = page
            .changes
            .iter()
            .filter(|change| matches!(change, Change::Tombstone(_)))
            .collect();
        assert_eq!(tombstones.len(), 1);
        let Change::Tombstone(tombstone) = tombstones[0] else {
            unreachable!()
        };
        assert_eq!(tombstone.id, issue.id);
        assert_eq!(tombstone.kind, ChangeKind::Issue);
        assert!(tombstone.deleted);
    }

    #[test]
    fn comments_are_scoped_to_their_parents_project() {
        let (db, project, other) = seed();
        let mine = new_issue(&db, project, "mine");
        let theirs = new_issue(&db, other, "theirs");
        let author = {
            let conn = db.read().unwrap();
            crate::db::queries::users::get_user_by_username(&conn, "syncer")
                .unwrap()
                .id
        };
        {
            let conn = db.write().unwrap();
            crate::db::queries::comments::create_comment(
                &conn,
                crate::db::queries::comments::CommentParent::Issue(mine.id),
                author,
                "in scope",
            )
            .unwrap();
            crate::db::queries::comments::create_comment(
                &conn,
                crate::db::queries::comments::CommentParent::Issue(theirs.id),
                author,
                "out of scope",
            )
            .unwrap();
        }

        let conn = db.read().unwrap();
        let page = list_changes(&conn, project, 0, 100).unwrap();
        let comments: Vec<&CommentChange> = page
            .changes
            .iter()
            .filter_map(|change| match change {
                Change::Comment(comment) => Some(comment),
                _ => None,
            })
            .collect();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].issue_id, Some(mine.id));
        assert_eq!(comments[0].username, "syncer");
    }

    #[test]
    fn resuming_from_a_cursor_yields_the_remainder_without_gaps_or_duplicates() {
        let (db, project, _) = seed();
        for n in 0..6 {
            new_issue(&db, project, &format!("issue {n}"));
        }

        let conn = db.read().unwrap();
        let first = list_changes(&conn, project, 0, 2).unwrap();
        assert_eq!(first.changes.len(), 2);
        assert!(first.has_more);
        assert_eq!(first.cursor, first.changes.last().map(Change::seq).unwrap());

        let mut seen: Vec<i64> = first.changes.iter().map(Change::seq).collect();
        let mut cursor = first.cursor;
        loop {
            let next = list_changes(&conn, project, cursor, 2).unwrap();
            seen.extend(next.changes.iter().map(Change::seq));
            cursor = next.cursor;
            if !next.has_more {
                break;
            }
        }

        let all = list_changes(&conn, project, 0, 1000).unwrap();
        let expected: Vec<i64> = all.changes.iter().map(Change::seq).collect();
        assert_eq!(seen, expected);
        let mut deduped = seen.clone();
        deduped.dedup();
        assert_eq!(deduped, seen, "no duplicates across pages");
    }

    #[test]
    fn an_empty_page_echoes_the_cursor_it_was_given() {
        let (db, project, _) = seed();
        new_issue(&db, project, "only");

        let conn = db.read().unwrap();
        let cursor = index_cursor(&conn).unwrap();
        let page = list_changes(&conn, project, cursor, 100).unwrap();
        assert!(page.changes.is_empty());
        assert_eq!(page.cursor, cursor, "cursor must never move backwards");
        assert!(!page.has_more);
    }

    #[test]
    fn index_returns_live_rows_only_and_a_cursor_at_or_above_all_of_them() {
        let (db, project, _) = seed();
        let live = new_issue(&db, project, "live");
        let doomed = new_issue(&db, project, "doomed");
        {
            let conn = db.write().unwrap();
            crate::db::queries::delete_issue(&conn, doomed.id).unwrap();
        }

        let conn = db.read().unwrap();
        let snapshot = get_index(&conn, project).unwrap();
        let ids: Vec<i64> = snapshot.issues.iter().map(|issue| issue.id).collect();
        assert_eq!(ids, vec![live.id]);
        assert!(snapshot.issues.iter().all(|issue| !issue.deleted));
        assert!(
            snapshot
                .issues
                .iter()
                .all(|issue| issue.seq <= snapshot.cursor)
        );
        assert!(
            snapshot
                .pages
                .iter()
                .all(|page| page.seq <= snapshot.cursor)
        );
    }

    /// LIF-439's race rule, exercised against the real composition
    /// `get_index` performs: cursor first, lists second. A write that lands
    /// between the two is allowed to show up twice, and must never show up
    /// zero times.
    #[test]
    fn a_write_racing_the_bootstrap_is_redelivered_rather_than_skipped() {
        let (db, project, _) = seed();
        new_issue(&db, project, "already there");

        let conn = db.read().unwrap();
        // Step 1 of `get_index`.
        let cursor = index_cursor(&conn).unwrap();
        // The racing write, landing between the cursor read and the lists.
        let raced = new_issue(&db, project, "raced in");
        // Step 2 of `get_index`.
        let (issues, _pages) = index_rows(&conn, project).unwrap();

        assert!(
            raced.seq > cursor,
            "the raced row must sit above the cursor the bootstrap took"
        );
        let delta = list_changes(&conn, project, cursor, 100).unwrap();
        let redelivered = delta.changes.iter().any(|change| match change {
            Change::Issue(issue) => issue.id == raced.id,
            _ => false,
        });
        assert!(
            redelivered,
            "a row written after the cursor must come back through /changes"
        );
        // Whether the snapshot also caught it is timing-dependent and fine
        // either way; what matters is that it is never lost.
        let _ = issues;
    }

    #[test]
    fn skinny_issue_rows_carry_labels_but_no_description() {
        let (db, project, _) = seed();
        let issue = {
            let conn = db.write().unwrap();
            conn.execute(
                "INSERT INTO labels (project_id, name, color) VALUES (?1, 'bug', '#ff0000')",
                params![project],
            )
            .unwrap();
            crate::db::queries::create_issue(
                &conn,
                &CreateIssue {
                    project_id: project,
                    title: "labelled".into(),
                    description: "a long body nobody should be shipping".into(),
                    labels: vec!["bug".into()],
                    ..Default::default()
                },
            )
            .unwrap()
        };

        let conn = db.read().unwrap();
        let page = list_changes(&conn, project, 0, 100).unwrap();
        let Some(Change::Issue(row)) = page.changes.iter().find(|change| match change {
            Change::Issue(candidate) => candidate.id == issue.id,
            _ => false,
        }) else {
            panic!("expected the issue in the delta");
        };
        assert_eq!(row.labels, vec!["bug".to_string()]);
        assert_eq!(row.identifier, "SYN-1");
        let json = serde_json::to_value(row).unwrap();
        assert!(json.get("description").is_none(), "skinny rows omit bodies");
        assert_eq!(json["kind"], "issue");
        assert_eq!(json["deleted"], false);
    }

    #[test]
    fn page_changes_carry_the_doc_identifier_and_no_content() {
        let (db, project, _) = seed();
        let page_row = {
            let conn = db.write().unwrap();
            crate::db::queries::create_page(
                &conn,
                &CreatePage {
                    project_id: Some(project),
                    title: "Design".into(),
                    content: "long prose".into(),
                    ..Default::default()
                },
            )
            .unwrap()
        };

        let conn = db.read().unwrap();
        let changes = list_changes(&conn, project, 0, 100).unwrap();
        let Some(Change::Page(row)) = changes.changes.iter().find(|change| match change {
            Change::Page(candidate) => candidate.id == page_row.id,
            _ => false,
        }) else {
            panic!("expected the page in the delta");
        };
        assert_eq!(row.identifier, "SYN-DOC-1");
        let json = serde_json::to_value(row).unwrap();
        assert!(json.get("content").is_none(), "skinny rows omit bodies");
        assert_eq!(json["kind"], "page");
    }

    // ── preview derivation ───────────────────────────────

    #[test]
    fn preview_takes_the_first_line_of_a_multi_line_body() {
        assert_eq!(
            preview_of("the summary line\nand then the rest\nand more"),
            "the summary line"
        );
    }

    #[test]
    fn preview_skips_leading_blank_lines() {
        assert_eq!(preview_of("\n\n   \n\treal content\nafter"), "real content");
    }

    #[test]
    fn preview_of_an_empty_or_blank_body_is_empty() {
        assert_eq!(preview_of(""), "");
        assert_eq!(preview_of("\n\n"), "");
        assert_eq!(preview_of("   \n\t\n  "), "");
    }

    /// The cap counts characters, not bytes. A byte slice at 200 would panic
    /// mid-codepoint on any of these lines.
    #[test]
    fn preview_truncates_at_200_characters_without_splitting_a_codepoint() {
        for body in ["é", "字", "🦎"] {
            let line = body.repeat(500);
            let preview = preview_of(&line);
            assert_eq!(preview.chars().count(), PREVIEW_CHARS);
            assert_eq!(preview, body.repeat(PREVIEW_CHARS));
        }
        // ASCII shorter than the cap is returned whole.
        let short = "x".repeat(199);
        assert_eq!(preview_of(&short), short);
    }

    #[test]
    fn issue_and_page_rows_carry_a_preview_but_never_the_body() {
        let (db, project, _) = seed();
        let (issue, page_row) = {
            let conn = db.write().unwrap();
            let issue = crate::db::queries::create_issue(
                &conn,
                &CreateIssue {
                    project_id: project,
                    title: "previewed".into(),
                    description: "\n\n# heading\nsecond line".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            let page = crate::db::queries::create_page(
                &conn,
                &CreatePage {
                    project_id: Some(project),
                    title: "Design".into(),
                    content: "  \nintro paragraph\nmore prose".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            (issue, page)
        };

        let conn = db.read().unwrap();
        let snapshot = get_index(&conn, project).unwrap();
        let indexed_issue = snapshot
            .issues
            .iter()
            .find(|row| row.id == issue.id)
            .expect("issue in the index");
        let indexed_page = snapshot
            .pages
            .iter()
            .find(|row| row.id == page_row.id)
            .expect("page in the index");
        assert_eq!(indexed_issue.preview, "# heading");
        assert_eq!(indexed_page.preview, "intro paragraph");

        let json = serde_json::to_value(indexed_page).unwrap();
        assert!(json.get("content").is_none(), "skinny rows omit bodies");
        assert_eq!(json["preview"], "intro paragraph");

        let delta = list_changes(&conn, project, 0, 100).unwrap();
        for change in &delta.changes {
            match change {
                Change::Issue(row) => assert_eq!(row.preview, "# heading"),
                Change::Page(row) => assert_eq!(row.preview, "intro paragraph"),
                _ => {}
            }
        }
    }

    #[test]
    fn a_body_less_issue_previews_as_an_empty_string() {
        let (db, project, _) = seed();
        let issue = new_issue(&db, project, "no body");

        let conn = db.read().unwrap();
        let snapshot = get_index(&conn, project).unwrap();
        let row = snapshot
            .issues
            .iter()
            .find(|row| row.id == issue.id)
            .unwrap();
        assert_eq!(row.preview, "");
    }

    // ── page labels ──────────────────────────────────────

    #[test]
    fn page_rows_carry_their_labels_in_both_the_index_and_the_delta() {
        let (db, project, _) = seed();
        let (labelled, bare) = {
            let conn = db.write().unwrap();
            for name in ["design", "spec"] {
                conn.execute(
                    "INSERT INTO labels (project_id, name, color) VALUES (?1, ?2, '#00ff00')",
                    params![project, name],
                )
                .unwrap();
            }
            let labelled = crate::db::queries::create_page(
                &conn,
                &CreatePage {
                    project_id: Some(project),
                    title: "Labelled".into(),
                    labels: vec!["spec".into(), "design".into()],
                    ..Default::default()
                },
            )
            .unwrap();
            let bare = crate::db::queries::create_page(
                &conn,
                &CreatePage {
                    project_id: Some(project),
                    title: "Bare".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            (labelled, bare)
        };

        let conn = db.read().unwrap();
        let snapshot = get_index(&conn, project).unwrap();
        let indexed = |id: i64| {
            snapshot
                .pages
                .iter()
                .find(|row| row.id == id)
                .expect("page in the index")
                .labels
                .clone()
        };
        // Sorted by name in the grouped lookup, not by insertion order.
        assert_eq!(indexed(labelled.id), vec!["design", "spec"]);
        assert!(indexed(bare.id).is_empty());

        let delta = list_changes(&conn, project, 0, 100).unwrap();
        let Some(Change::Page(row)) = delta.changes.iter().find(|change| match change {
            Change::Page(candidate) => candidate.id == labelled.id,
            _ => false,
        }) else {
            panic!("expected the page in the delta");
        };
        assert_eq!(row.labels, vec!["design".to_string(), "spec".to_string()]);
    }

    #[test]
    fn a_tombstone_serializes_to_identity_and_nothing_else() {
        let (db, project, _) = seed();
        let issue = new_issue(&db, project, "doomed");
        {
            let conn = db.write().unwrap();
            crate::db::queries::delete_issue(&conn, issue.id).unwrap();
        }

        let conn = db.read().unwrap();
        let page = list_changes(&conn, project, 0, 100).unwrap();
        let tombstone = page
            .changes
            .iter()
            .find(|change| matches!(change, Change::Tombstone(_)))
            .unwrap();
        let json = serde_json::to_value(tombstone).unwrap();
        let object = json.as_object().unwrap();
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["deleted", "id", "kind", "seq"]);
    }
}
