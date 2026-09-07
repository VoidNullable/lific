use rusqlite::{Connection, OptionalExtension, params};

use crate::db::models::{AttachmentActor, AttachmentEntity, Comment, CommentActor};
use crate::error::LificError;

use super::{TOMBSTONE_NOW, unescape_text};

/// Comment bodies are intentionally much smaller than the transport-wide JSON
/// ceiling. This bounds persistent attacker-controlled history and the largest
/// single row loaded by a detail view.
pub const MAX_COMMENT_BYTES: usize = 256 * 1024;

/// The response-byte ceiling for one interactive page of comments (LIF-421).
///
/// A row cap alone does not bound a response: 50 rows of 256 KiB is 12.5 MiB,
/// and the row cap is 500. This is the number that actually binds, and it is
/// counted against the *serialized* page — every row's JSON, escaping and all
/// — not the sum of the raw bodies, because that is what the transport,
/// the browser and an agent's context window actually pay for.
pub const MAX_COMMENT_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

/// Held back from [`MAX_COMMENT_RESPONSE_BYTES`] for the parts of a response
/// that are not comment rows: the array's own framing, the pagination headers,
/// the status line, and the MCP envelope a rendered page is wrapped in. The
/// budget is a promise about the whole response, so the parts that are not
/// rows have to come out of it rather than be added on top.
const RESPONSE_OVERHEAD_BYTES: usize = 8 * 1024;

/// The most distinct `@username` tokens one body may carry.
///
/// LIF-421: mention reconciliation is per-token work against the candidate
/// roster, so an adversarial body full of `@a @b @c ...` turns one write into
/// tens of thousands of lookups. A body past this cap is **refused**, not
/// quietly resolved as far as the cap and no further: resolving a prefix would
/// drop the mention rows behind it, and a comment that says it notified
/// someone while the row that does the notifying was silently discarded is a
/// lie the author cannot see. The refusal names the cap, so the fix is
/// visible.
pub const MAX_MENTION_TOKENS: usize = 256;

/// How many bytes of response one comment costs once serialized.
///
/// Measured, not estimated: the row is written through a counting sink, so
/// JSON escaping (`"` → `\"`, a newline → `\n`, a control character → six
/// bytes of `\u00XX`) and the field names are all counted at their real cost.
/// A body of quotes and newlines is close to twice its own length on the wire,
/// which is the difference between honouring a 2 MiB budget and overshooting
/// it by a megabyte. The extra byte is the comma that joins this row to the
/// next one in the array.
pub fn response_cost(comment: &Comment) -> usize {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 += buf.len();
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    // Serializing a `Comment` cannot fail for any value the database can
    // hold (no maps with non-string keys, no non-finite floats), and the sink
    // never errors. Fall back to the raw body length rather than panic.
    match serde_json::to_writer(&mut counter, comment) {
        Ok(()) => counter.0 + 1,
        Err(_) => comment.content.len() + 1,
    }
}

/// The row bytes available to one page, after reserving response overhead.
fn row_budget() -> usize {
    MAX_COMMENT_RESPONSE_BYTES - RESPONSE_OVERHEAD_BYTES
}

pub fn validate_comment_content(content: &str) -> Result<(), LificError> {
    validate_comment_edit(content, None)
}

/// Size-check a body that is replacing `previous_bytes` of stored content.
///
/// LIF-421: a comment written before the cap existed (or imported from a
/// tracker that had none) is over the limit through no act of its author, and
/// refusing every edit to it makes the one thing they might reasonably want to
/// do — shorten it, fix it, cut it down — impossible. So an edit is measured
/// against `max(cap, what is already stored)`: a grandfathered comment may be
/// rewritten and shrunk freely, it just may not grow. `previous_bytes` is
/// `None` for a create, where the cap is the whole rule.
///
/// This is the *storage* rule and it does not widen the transport one. The
/// HTTP server's global 2 MiB `DefaultBodyLimit` (`server::build_app`) still
/// applies to the request carrying the edit, so a grandfathered comment larger
/// than that cannot be rewritten at its full size over REST — the request is
/// refused before this function is reached. That is deliberate: raising the
/// global body limit to accommodate a handful of legacy rows would widen every
/// endpoint's exposure to pay for one. Such a comment can still be
/// **shortened** through REST (an edit only has to fit the request, not the
/// stored body it replaces), and rewritten at its own size through a transport
/// with no HTTP body limit: the direct-SQL CLI or MCP over stdio.
pub fn validate_comment_edit(
    content: &str,
    previous_bytes: Option<usize>,
) -> Result<(), LificError> {
    let allowance = previous_bytes.unwrap_or(0).max(MAX_COMMENT_BYTES);
    if content.len() <= allowance {
        return Ok(());
    }
    Err(LificError::BadRequest(match previous_bytes {
        Some(previous) if previous > MAX_COMMENT_BYTES => format!(
            "comment is too large (max {MAX_COMMENT_BYTES} bytes; this comment predates \
             the limit at {previous} bytes, so an edit may shrink it but not grow it)"
        ),
        _ => format!("comment is too large (max {MAX_COMMENT_BYTES} bytes)"),
    }))
}

/// What a comment is attached to.
///
/// The `comments` table allows exactly one of (issue_id, page_id) to be set
/// (enforced by a CHECK constraint added in migration 012). This enum mirrors
/// that invariant in Rust so callers can't accidentally construct an
/// orphan or dual-parent comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentParent {
    Issue(i64),
    Page(i64),
}

impl CommentParent {
    fn issue_id(self) -> Option<i64> {
        match self {
            Self::Issue(id) => Some(id),
            Self::Page(_) => None,
        }
    }

    fn page_id(self) -> Option<i64> {
        match self {
            Self::Page(id) => Some(id),
            Self::Issue(_) => None,
        }
    }

    pub fn project_id(self, conn: &Connection) -> Result<Option<i64>, LificError> {
        match self {
            Self::Issue(issue_id) => Ok(Some(super::get_issue(conn, issue_id)?.project_id)),
            Self::Page(page_id) => Ok(super::get_page(conn, page_id)?.project_id),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentContext {
    parent: CommentParent,
    project_id: Option<i64>,
    parent_identifier: String,
}

impl CommentContext {
    pub fn resolve(conn: &Connection, comment: &Comment) -> Result<Self, LificError> {
        match (comment.issue_id, comment.page_id) {
            (Some(issue_id), None) => {
                let issue = super::get_issue(conn, issue_id)?;
                Ok(Self {
                    parent: CommentParent::Issue(issue.id),
                    project_id: Some(issue.project_id),
                    parent_identifier: issue.identifier,
                })
            }
            (None, Some(page_id)) => {
                let page = super::get_page(conn, page_id)?;
                Ok(Self {
                    parent: CommentParent::Page(page.id),
                    project_id: page.project_id,
                    parent_identifier: page.identifier,
                })
            }
            _ => Err(LificError::Internal(format!(
                "comment {} has an invalid parent",
                comment.id
            ))),
        }
    }

    pub fn parent(&self) -> CommentParent {
        self.parent
    }

    pub fn project_id(&self) -> Option<i64> {
        self.project_id
    }

    pub fn parent_identifier(&self) -> &str {
        &self.parent_identifier
    }
}

/// Insert a comment row attached to an issue or page, and nothing else.
///
/// LIF-409: deliberately private. A comment body carries `@mentions` and
/// `/api/attachments/{id}` references, and a comment written without resolving
/// them is a comment that silently notifies nobody and leaves its attachments
/// unlinked for the orphan sweep to collect. Every production write goes
/// through [`create_comment_with_mentions`], which does all three inside one
/// savepoint. Two callers are exempt and each has its own explicit door:
/// [`create_imported_comment`] (bulk import, below) and tests.
fn insert_comment_row(
    conn: &Connection,
    parent: CommentParent,
    user_id: i64,
    content: &str,
) -> Result<Comment, LificError> {
    let content = unescape_text(content);
    validate_comment_content(&content)?;

    // Verify the parent exists. We do this explicitly (vs. relying on the FK)
    // so the error message names the missing entity rather than surfacing a
    // raw SQLite constraint failure.
    let (table, id) = match parent {
        CommentParent::Issue(id) => ("issues", id),
        CommentParent::Page(id) => ("pages", id),
    };
    let exists: bool = conn
        .query_row(
            &format!("SELECT COUNT(*) > 0 FROM {table} WHERE id = ?1 AND deleted_at IS NULL"),
            params![id],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !exists {
        let kind = match parent {
            CommentParent::Issue(_) => "issue",
            CommentParent::Page(_) => "page",
        };
        return Err(LificError::NotFound(format!("{kind} {id} not found")));
    }

    conn.execute(
        "INSERT INTO comments (issue_id, page_id, user_id, content)
         VALUES (?1, ?2, ?3, ?4)",
        params![parent.issue_id(), parent.page_id(), user_id, content],
    )?;

    let id = conn.last_insert_rowid();
    get_comment(conn, id)
}

/// The historical bulk-import path (LIF-264/265), and the one production
/// caller allowed to write a comment without reconciling it.
///
/// An imported comment is a verbatim record of something said in another
/// tracker: its `@handles` name that tracker's users, not this one's, and its
/// attachment references (if any) point at that tracker's URLs, so there is
/// nothing here to resolve. Attributed to the import bot, never to a person.
pub fn create_imported_comment(
    conn: &Connection,
    parent: CommentParent,
    bot_id: i64,
    content: &str,
) -> Result<Comment, LificError> {
    insert_comment_row(conn, parent, bot_id, content)
}

/// LIF-409: test-only handles on the unreconciled primitives, so a test can
/// seed a comment row without exercising mention and attachment
/// reconciliation. Absent from every non-test build, which is what keeps
/// production honest: a new caller cannot reach them by accident, because
/// outside `cfg(test)` these names do not exist.
#[cfg(test)]
pub(crate) fn create_comment(
    conn: &Connection,
    parent: CommentParent,
    user_id: i64,
    content: &str,
) -> Result<Comment, LificError> {
    insert_comment_row(conn, parent, user_id, content)
}

/// See [`create_comment`].
#[cfg(test)]
pub(crate) fn update_comment(
    conn: &Connection,
    id: i64,
    content: &str,
) -> Result<Comment, LificError> {
    write_comment_content(conn, id, content)
}

/// Get a single comment by ID (with author info). Parent-agnostic.
pub fn get_comment(conn: &Connection, id: i64) -> Result<Comment, LificError> {
    conn.query_row(
        "SELECT c.id, c.issue_id, c.page_id, COALESCE(c.user_id, -1),
                COALESCE(c.imported_author, u.username), COALESCE(c.imported_author, u.display_name),
                c.content, c.created_at, c.updated_at, c.seq
         FROM comments c
         LEFT JOIN users u ON u.id = c.user_id
         WHERE c.id = ?1 AND c.deleted_at IS NULL",
        params![id],
        row_to_comment,
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            LificError::NotFound(format!("comment {id} not found"))
        }
        other => other.into(),
    })
}

/// List *every* comment for an issue or page, ordered chronologically (oldest
/// first by default; pass `order = Some("desc")` for newest first).
/// `author` filters by exact username (case-insensitive).
///
/// Test-only. No shipped read is unbounded any more: a comment body may be
/// 256 KiB, so "the whole thread" is not a size anyone can reason about.
/// Production callers pick a window through [`list_comments_page`], or ask
/// for the whole thread deliberately through [`list_comments_exhaustive`].
#[cfg(test)]
pub fn list_comments(
    conn: &Connection,
    parent: CommentParent,
    author: Option<&str>,
    order: Option<&str>,
) -> Result<Vec<Comment>, LificError> {
    list_comments_exhaustive(conn, parent, author, order, None, None)
}

/// Count comments for an issue or page after applying the same optional
/// author filter as `list_comments_paginated`.
pub fn count_comments(
    conn: &Connection,
    parent: CommentParent,
    author: Option<&str>,
) -> Result<i64, LificError> {
    let (parent_col, id) = match parent {
        CommentParent::Issue(id) => ("c.issue_id", id),
        CommentParent::Page(id) => ("c.page_id", id),
    };
    if let Some(username) = author {
        conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM comments c
                 LEFT JOIN users u ON u.id = c.user_id
                 WHERE {parent_col} = ?1 AND c.deleted_at IS NULL
                   AND COALESCE(c.imported_author, u.username) = ?2 COLLATE NOCASE"
            ),
            params![id, username],
            |row| row.get(0),
        )
        .map_err(Into::into)
    } else {
        conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM comments c
                 WHERE {parent_col} = ?1 AND c.deleted_at IS NULL"
            ),
            params![id],
            |row| row.get(0),
        )
        .map_err(Into::into)
    }
}

/// Every comment the row bounds admit, with **no response-byte budget**.
///
/// LIF-421: the one deliberate exit from the byte budget, for the export
/// path. An export is a file, not a response, and silently dropping comments
/// out of it would be the exact failure the budget exists to prevent
/// elsewhere: an artifact that looks complete and is not. Callers must carry
/// their own bound instead — `export::bounded_issue_comments` refuses a thread
/// past `MAX_EXPORT_COMMENTS` and `ensure_comment_sizes` refuses one past the
/// aggregate export byte limit *before* calling this.
///
/// `limit` is clamped to 1..=[`MAX_PAGE_LIMIT`](super::MAX_PAGE_LIMIT) and
/// `offset` to zero or greater, the same bounds every other paginated query
/// uses; passing neither reads the whole thread.
pub fn list_comments_exhaustive(
    conn: &Connection,
    parent: CommentParent,
    author: Option<&str>,
    order: Option<&str>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<Comment>, LificError> {
    Ok(scan_comments(
        conn,
        &CommentScan {
            parent,
            author,
            order,
            limit,
            offset,
            before: None,
            budget: None,
        },
    )?
    .items)
}

/// A position in a comment thread, named by the ordering key itself.
///
/// Comments are ordered by `(created_at, id)`, so that pair identifies a row's
/// place in the thread exactly. Unlike an offset it does not move when someone
/// posts or deletes a comment while a reader is paging: "the rows before this
/// one" stays the same question no matter what happened above it. The id is
/// part of the cursor because `created_at` has one-second resolution and
/// several comments routinely share a timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentCursor {
    pub created_at: String,
    pub id: i64,
}

impl CommentCursor {
    /// The cursor that pages to the rows *before* `comment`.
    ///
    /// Test-only in Rust: the shipped consumer is the web client, which
    /// derives the same pair from the JSON it already holds and sends it back
    /// as `before_created_at` + `before_id`.
    #[cfg(test)]
    pub fn before(comment: &Comment) -> Self {
        Self {
            created_at: comment.created_at.clone(),
            id: comment.id,
        }
    }
}

/// One page of a comment thread, and everything a caller needs to ask for the
/// next one (LIF-421).
///
/// `has_more` is authoritative: it is true when the row limit cut the page
/// *or* when the response-byte budget did, so a client can trust it instead of
/// inferring "the page came back full, so there is probably more". Inference
/// is what breaks the moment the byte budget can end a page early — a
/// three-row page is no longer evidence that the thread has three rows left.
#[derive(Debug, Clone)]
pub struct CommentPage {
    /// The rows, in the order the query asked for.
    pub items: Vec<Comment>,
    /// Whether anything at all lies past this page.
    pub has_more: bool,
    /// Where the next offset-paged request starts: the offset this page began
    /// at plus the number of rows actually returned. Counting returned rows
    /// rather than the requested limit is what keeps continuation correct when
    /// the byte budget shortened the page.
    pub next_offset: i64,
    /// True when the response-byte budget ended this page rather than the row
    /// limit. Transports use it to say *why* a page is short.
    pub budget_limited: bool,
}

/// What one scan of a comment thread asks for.
///
/// A struct rather than eight positional arguments: the byte budget joined a
/// signature that was already at the limit, and `Some(500), Some(0), None`
/// tells a reader nothing about which knob is which.
struct CommentScan<'a> {
    parent: CommentParent,
    author: Option<&'a str>,
    order: Option<&'a str>,
    limit: Option<i64>,
    offset: Option<i64>,
    before: Option<&'a CommentCursor>,
    /// Response bytes this page may spend, or `None` for an exhaustive read.
    budget: Option<usize>,
}

/// One page of comments for an issue or page, bounded by rows *and* bytes.
///
/// LIF-388: the over-fetch that answers `has_more` happens here, after the
/// clamp, rather than at the transport. A caller that asked for
/// `MAX_PAGE_LIMIT` comments and then over-fetched itself would have its
/// `limit + 1` clamped straight back to the cap, and would report "no more
/// comments" on the one page size where the answer matters most.
pub fn list_comments_page(
    conn: &Connection,
    parent: CommentParent,
    author: Option<&str>,
    order: Option<&str>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<CommentPage, LificError> {
    list_comments_keyset(conn, parent, author, order, limit, offset, None)
}

/// [`list_comments_page`] with an optional keyset cursor.
///
/// `before` returns only the rows strictly older than that position, which is
/// what makes "load the previous page" stable while a thread is being written
/// to. It requires `order = desc` (paging backwards is the only direction a
/// "before" cursor describes) and no offset, since mixing the two would mean
/// skipping rows relative to a position that already did the skipping.
pub fn list_comments_keyset(
    conn: &Connection,
    parent: CommentParent,
    author: Option<&str>,
    order: Option<&str>,
    limit: Option<i64>,
    offset: Option<i64>,
    before: Option<&CommentCursor>,
) -> Result<CommentPage, LificError> {
    scan_comments(
        conn,
        &CommentScan {
            parent,
            author,
            order,
            limit,
            offset,
            before,
            budget: Some(row_budget()),
        },
    )
}

/// [`list_comments_page`] with the response-byte allowance named explicitly.
///
/// For a caller whose response carries something other than comments. The
/// budget is a promise about the *whole* response, so a renderer that has
/// already spent 300 KiB on an issue's description and relations has 300 KiB
/// less to spend on its comment trail, and it is the only party that knows
/// that. [`remaining_budget`] turns what it has already written into what is
/// left to pass here.
pub fn list_comments_page_within(
    conn: &Connection,
    parent: CommentParent,
    author: Option<&str>,
    order: Option<&str>,
    limit: Option<i64>,
    offset: Option<i64>,
    budget: usize,
) -> Result<CommentPage, LificError> {
    scan_comments(
        conn,
        &CommentScan {
            parent,
            author,
            order,
            limit,
            offset,
            before: None,
            budget: Some(budget),
        },
    )
}

/// What a response has left for comments after `spent` bytes of everything
/// else, or the refusal when there is nothing left.
///
/// The error is deliberate and named after what actually happened: an issue
/// whose own description and relations do not fit a 2 MiB response is not a
/// comment problem, and quietly rendering it with an empty comment trail would
/// report "no comments" about a thread that exists. `subject` names the
/// entity so the message points at the thing to fix.
pub fn remaining_budget(spent: usize, subject: &str) -> Result<usize, LificError> {
    let budget = row_budget();
    budget
        .checked_sub(spent)
        .filter(|left| *left > 0)
        .ok_or_else(|| {
            LificError::PayloadTooLarge(format!(
                "{subject} is {spent} bytes before any comments, past the \
             {budget}-byte response budget. Read it in narrower pieces \
             (get_issue with comments=none, then list_comments)."
            ))
        })
}

/// [`list_comments_page_within`] for the tests that pin the prefix rule.
///
/// The shipped budget is 2 MiB, and pinning its behaviour with real 2 MiB
/// pages would trade a slow test suite for no extra confidence: the prefix
/// rule, the continuation offset and the oversized refusal behave the same at
/// 500 bytes as at 2 MiB.
#[cfg(test)]
pub(crate) fn list_comments_within(
    conn: &Connection,
    parent: CommentParent,
    order: Option<&str>,
    limit: Option<i64>,
    offset: Option<i64>,
    budget: usize,
) -> Result<CommentPage, LificError> {
    list_comments_page_within(conn, parent, None, order, limit, offset, budget)
}

/// The one place a comment thread is read from (LIF-421).
///
/// Every transport reads through here, so the row cap, the author filter, the
/// tombstone exclusion, the keyset cursor and the byte budget are applied
/// once, in that order, and cannot drift apart per surface.
///
/// The budget takes the **longest contiguous prefix** that fits, and stops. It
/// never skips a row to fit a later one: a page with a hole in it is not a
/// page, and a client walking continuations would never come back for what was
/// stepped over. A single row too large for the whole budget is therefore not
/// something this can answer with a short page, and is refused explicitly
/// rather than returned as an empty success that would leave a client asking
/// for the same offset forever.
///
/// A budgeted read runs in two stages, and the order is the point. Stage one
/// asks SQLite only for each candidate row's *size* — `length(CAST(content AS
/// BLOB))`, never the content — and decides how many rows could possibly fit.
/// Stage two fetches exactly those and measures them exactly. A 40 MiB legacy
/// comment is therefore refused having never been read into memory: reading it
/// in order to say it is too big to read would be the same denial of service
/// the budget exists to prevent.
fn scan_comments(conn: &Connection, scan: &CommentScan<'_>) -> Result<CommentPage, LificError> {
    let query = CommentQuery::build(scan)?;
    let Some(budget) = scan.budget else {
        // Exhaustive: one query, no measuring, and no lookahead row either —
        // "everything up to the caller's own limit" has nothing past it to
        // report. See `list_comments_exhaustive` for who is allowed here.
        let items = query.fetch(conn, query.page_limit)?;
        let next_offset = query.start_offset.saturating_add(items.len() as i64);
        return Ok(CommentPage {
            items,
            has_more: false,
            next_offset,
            budget_limited: false,
        });
    };

    let plan = query.plan(conn, budget)?;
    let mut items = query.fetch(conn, plan.admitted)?;

    // Stage one measured stored bytes, which is a *lower* bound on what the
    // row costs serialized: escaping only ever adds. So the prefix it admitted
    // may still be one row too long once the real cost is known, and this is
    // where that is settled. Rows are only ever dropped from the end.
    let mut bytes = 0usize;
    let mut trimmed = false;
    for (index, comment) in items.iter().enumerate() {
        let cost = response_cost(comment);
        if bytes + cost > budget {
            if index == 0 {
                // Its stored length fit; its escaping did not. Rare, and still
                // has to be a refusal rather than an empty page.
                return Err(oversized_comment(
                    comment.id,
                    &comment.created_at,
                    cost,
                    budget,
                    query.dir,
                    query.start_offset,
                ));
            }
            trimmed = true;
            items.truncate(index);
            break;
        }
        bytes += cost;
    }

    let next_offset = query.start_offset.saturating_add(items.len() as i64);
    Ok(CommentPage {
        items,
        has_more: plan.has_more || trimmed,
        next_offset,
        budget_limited: plan.budget_limited || trimmed,
    })
}

/// How many rows a budgeted page may fetch, and what lies past them.
struct CommentPlan {
    /// Rows stage two is allowed to read.
    admitted: i64,
    has_more: bool,
    budget_limited: bool,
}

/// One comment listing's `FROM`/`WHERE`/`ORDER BY`, ready to be run with
/// either a metadata or a full column list.
///
/// The two stages have to filter and order identically or the sizes measured
/// would belong to different rows than the ones returned, so the predicate is
/// built once and both stages borrow it.
struct CommentQuery {
    tail: String,
    values: Vec<Box<dyn rusqlite::types::ToSql>>,
    dir: &'static str,
    page_limit: i64,
    start_offset: i64,
}

impl CommentQuery {
    fn build(scan: &CommentScan<'_>) -> Result<Self, LificError> {
        let dir = match scan.order {
            None | Some("asc") => "ASC",
            Some("desc") => "DESC",
            Some(other) => {
                return Err(LificError::BadRequest(format!(
                    "invalid order '{other}'. Use asc or desc."
                )));
            }
        };
        if scan.before.is_some() {
            if dir != "DESC" {
                return Err(LificError::BadRequest(
                    "keyset paging requires order=desc".into(),
                ));
            }
            if scan.offset.is_some_and(|offset| offset != 0) {
                return Err(LificError::BadRequest(
                    "keyset paging cannot be combined with a non-zero offset".into(),
                ));
            }
        }
        let (parent_col, id) = match scan.parent {
            CommentParent::Issue(id) => ("c.issue_id", id),
            CommentParent::Page(id) => ("c.page_id", id),
        };
        let mut tail = format!(
            "FROM comments c
             LEFT JOIN users u ON u.id = c.user_id
             WHERE {parent_col} = ?1 AND c.deleted_at IS NULL"
        );
        let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(id)];
        if let Some(username) = scan.author {
            tail.push_str(&format!(
                " AND COALESCE(c.imported_author, u.username) = ?{} COLLATE NOCASE",
                values.len() + 1
            ));
            values.push(Box::new(username.to_string()));
        }
        if let Some(cursor) = scan.before {
            // Strictly older than the cursor under the same (created_at, id)
            // ordering the query sorts by. Both halves are bound parameters;
            // the cursor is caller-supplied and never reaches the SQL text.
            tail.push_str(&format!(
                " AND (c.created_at < ?{ts} OR (c.created_at = ?{ts} AND c.id < ?{id}))",
                ts = values.len() + 1,
                id = values.len() + 2
            ));
            values.push(Box::new(cursor.created_at.clone()));
            values.push(Box::new(cursor.id));
        }
        // `dir` comes from the two-value whitelist above, never raw input.
        tail.push_str(&format!(" ORDER BY c.created_at {dir}, c.id {dir}"));

        let (page_limit, start_offset) = match (scan.limit, scan.offset) {
            (None, None) => (super::NO_LIMIT, 0),
            _ => super::page_unbounded(scan.limit, scan.offset),
        };
        Ok(Self {
            tail,
            values,
            dir,
            page_limit,
            start_offset,
        })
    }

    /// The row limit including the over-fetched lookahead row.
    fn over_fetched_limit(&self) -> i64 {
        super::over_fetch(self.page_limit)
    }

    fn bind(&self, limit: i64) -> (String, Vec<Box<dyn rusqlite::types::ToSql>>) {
        let clause = format!(
            " LIMIT ?{} OFFSET ?{}",
            self.values.len() + 1,
            self.values.len() + 2
        );
        (clause, vec![Box::new(limit), Box::new(self.start_offset)])
    }

    /// Stage one: how big is each candidate row, without reading any of them.
    fn plan(&self, conn: &Connection, budget: usize) -> Result<CommentPlan, LificError> {
        let (limit_clause, extra) = self.bind(self.over_fetched_limit());
        let sql = format!(
            "SELECT c.id, c.created_at, length(CAST(c.content AS BLOB)) {}{limit_clause}",
            self.tail
        );
        let mut bound: Vec<&dyn rusqlite::types::ToSql> =
            self.values.iter().map(|value| value.as_ref()).collect();
        bound.extend(extra.iter().map(|value| &**value));

        let row_cap = match self.page_limit {
            super::NO_LIMIT => i64::MAX,
            limit => limit,
        };
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(bound.as_slice())?;
        let mut admitted = 0i64;
        let mut bytes = 0usize;
        while let Some(row) = rows.next()? {
            if admitted >= row_cap {
                // The lookahead row: proof the thread continues, never part of
                // the page, and never fetched.
                return Ok(CommentPlan {
                    admitted,
                    has_more: true,
                    budget_limited: false,
                });
            }
            let id: i64 = row.get(0)?;
            let created_at: String = row.get(1)?;
            let stored: i64 = row.get(2)?;
            // A lower bound on the serialized cost: the body's own bytes plus
            // the comma that joins it to the next row. Escaping and the
            // surrounding fields only add, so a row this rules out could never
            // have fitted, and one it admits is re-measured exactly in stage
            // two.
            let floor = usize::try_from(stored)
                .unwrap_or(usize::MAX)
                .saturating_add(1);
            if bytes.saturating_add(floor) > budget {
                if admitted == 0 {
                    return Err(oversized_comment(
                        id,
                        &created_at,
                        floor,
                        budget,
                        self.dir,
                        self.start_offset,
                    ));
                }
                return Ok(CommentPlan {
                    admitted,
                    has_more: true,
                    budget_limited: true,
                });
            }
            bytes += floor;
            admitted += 1;
        }
        Ok(CommentPlan {
            admitted,
            has_more: false,
            budget_limited: false,
        })
    }

    /// Stage two: read the rows, bodies and all.
    fn fetch(&self, conn: &Connection, limit: i64) -> Result<Vec<Comment>, LificError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let (limit_clause, extra) = self.bind(limit);
        let sql = format!(
            "SELECT c.id, c.issue_id, c.page_id, COALESCE(c.user_id, -1),
                    COALESCE(c.imported_author, u.username), COALESCE(c.imported_author, u.display_name),
                    c.content, c.created_at, c.updated_at, c.seq {}{limit_clause}",
            self.tail
        );
        let mut bound: Vec<&dyn rusqlite::types::ToSql> =
            self.values.iter().map(|value| value.as_ref()).collect();
        bound.extend(extra.iter().map(|value| &**value));
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(bound.as_slice(), row_to_comment)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

/// The refusal for a single row that cannot fit any page (LIF-421).
///
/// Only reachable for a comment written before the 256 KiB body cap, or
/// imported past it. Three things have to be in the message or the caller is
/// stuck: which comment (so it can be found and fixed), how big it is (so the
/// number is not a mystery), and the exact way to page past it. Without that
/// last part a client retries the same request forever, which is worse than
/// the oversized row itself.
fn oversized_comment(
    id: i64,
    created_at: &str,
    bytes: usize,
    budget: usize,
    dir: &str,
    offset: i64,
) -> LificError {
    let skip = if dir == "DESC" {
        format!(
            "before_created_at={created_at}&before_id={id} (keyset) or offset={}",
            offset.saturating_add(1)
        )
    } else {
        format!("offset={}", offset.saturating_add(1))
    };
    LificError::PayloadTooLarge(format!(
        "comment {id} needs at least {bytes} bytes on its own, past the {budget}-byte \
         page budget, so no page can include it. Read it directly with get_comment, \
         or skip it with {skip}."
    ))
}

/// Overwrite a comment's content, and nothing else. Parent-agnostic.
///
/// LIF-409: private for the same reason as [`insert_comment_row`] — an edit
/// that skips reconciliation strands the mentions and links the previous body
/// established. Production edits go through [`update_comment_with_mentions`].
fn write_comment_content(conn: &Connection, id: i64, content: &str) -> Result<Comment, LificError> {
    let content = unescape_text(content);
    // SQLite's `length(TEXT)` counts characters; the cap counts UTF-8 bytes,
    // so the stored body is measured through its BLOB form. Reading the length
    // rather than the row keeps a 3 MiB legacy comment out of memory just to
    // find out how long it is.
    let previous_bytes: Option<i64> = conn
        .query_row(
            "SELECT length(CAST(content AS BLOB)) FROM comments
              WHERE id = ?1 AND deleted_at IS NULL",
            params![id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(previous_bytes) = previous_bytes else {
        return Err(LificError::NotFound(format!("comment {id} not found")));
    };
    validate_comment_edit(&content, Some(usize::try_from(previous_bytes).unwrap_or(0)))?;

    let changed = conn.execute(
        "UPDATE comments SET content = ?1, updated_at = datetime('now')
          WHERE id = ?2 AND deleted_at IS NULL",
        params![content, id],
    )?;

    if changed == 0 {
        return Err(LificError::NotFound(format!("comment {id} not found")));
    }

    get_comment(conn, id)
}

/// Tombstone a comment (LIF-438). Parent-agnostic.
///
/// The row stays, carrying `deleted_at` and a fresh `seq`, so a replica can
/// learn the comment went away. A comment deleted this way keeps its own
/// timestamp, which is what makes it survive a later restore of its parent:
/// the restore cascade only revives children whose `deleted_at` matches the
/// parent's.
pub fn delete_comment(conn: &Connection, id: i64) -> Result<(), LificError> {
    let changed = conn.execute(
        &format!(
            "UPDATE comments SET deleted_at = {TOMBSTONE_NOW} \
             WHERE id = ?1 AND deleted_at IS NULL"
        ),
        params![id],
    )?;
    if changed == 0 {
        return Err(LificError::NotFound(format!("comment {id} not found")));
    }
    Ok(())
}

/// A comment's current `seq`, tombstone or not (LIF-440). See
/// [`super::issues::issue_seq`] for why deletes have to read this back rather
/// than reuse the copy of the row they authorized against.
pub fn comment_seq(conn: &Connection, id: i64) -> Result<i64, LificError> {
    conn.query_row("SELECT seq FROM comments WHERE id = ?1", [id], |row| {
        row.get(0)
    })
    .map_err(|error| match error {
        rusqlite::Error::QueryReturnedNoRows => {
            LificError::NotFound(format!("comment {id} not found"))
        }
        other => other.into(),
    })
}

// ── @mentions (LIF-263) ──────────────────────────────────────────

/// Extract the set of candidate `@username` tokens from a comment body.
///
/// A mention is `@` immediately followed by a run of username characters
/// (`[A-Za-z0-9_-]`). The `@` must sit at a word boundary — start of
/// string or after whitespace / most punctuation — so `foo@bar.com`
/// (an email) and `a@b` (mid-word) never register. Trailing punctuation
/// is naturally excluded because it isn't a username character: `@ada,`
/// yields `ada`, `(@bob)` yields `bob`.
///
/// Returns raw token strings (case preserved as typed); matching against
/// real users happens later and is case-insensitive. Duplicates are
/// collapsed through a hash set, so a body repeating one handle ten thousand
/// times costs one entry rather than a linear rescan per occurrence. This is
/// pure text parsing — it does not touch the DB, so it can be unit-tested in
/// isolation.
///
/// LIF-421: scanning stops one token past [`MAX_MENTION_TOKENS`], which is
/// all [`sync_mentions`] needs to refuse the write. Bounded work either way:
/// an adversarial body never costs more than the cap in lookups.
pub fn extract_mention_usernames(body: &str) -> Vec<String> {
    let bytes = body.as_bytes();
    let is_username_char = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'-';
    // The character immediately before `@` must be a boundary: nothing
    // (start), whitespace, or punctuation that isn't a username char. This
    // rejects `foo@bar` while allowing `(@bob`, `@ada`, `hi @you`.
    let is_boundary = |c: u8| !is_username_char(c) && c != b'@';

    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'@' {
            let prev_ok = i == 0 || is_boundary(bytes[i - 1]);
            if prev_ok {
                let start = i + 1;
                let mut j = start;
                while j < bytes.len() && is_username_char(bytes[j]) {
                    j += 1;
                }
                if j > start {
                    let token = &body[start..j];
                    let key = token.to_lowercase();
                    if seen.insert(key) {
                        out.push(token.to_string());
                        // One past the cap is enough to prove the body is over
                        // it, and stopping there bounds the scan.
                        // `sync_mentions` turns that extra token into the
                        // refusal.
                        if out.len() > MAX_MENTION_TOKENS {
                            return out;
                        }
                    }
                    i = j;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// List the users who may be `@`-mentioned in a given project's comments.
///
/// When `member_scoped` is true (the caller passes the live `authz_enforced`
/// flag), the candidate list is exactly the project's members — nobody who
/// can't see the project is ever suggested. When false, every user is a
/// candidate (legacy mode has no concept of project-hidden users). Bots are
/// excluded: an `@`-mention targets a person, and a connected tool isn't one
/// a human would address in a thread.
///
/// `project_id = None` (workspace-level page) has no membership list, so the
/// member-scoped branch returns an empty set — matching the design decision
/// that workspace pages are admin-only surfaces.
pub fn mention_candidates(
    conn: &Connection,
    project_id: Option<i64>,
    member_scoped: bool,
) -> Result<Vec<crate::db::models::MentionCandidate>, LificError> {
    let map_row = |row: &rusqlite::Row| {
        Ok(crate::db::models::MentionCandidate {
            user_id: row.get(0)?,
            username: row.get(1)?,
            display_name: row.get(2)?,
        })
    };

    let rows: Vec<crate::db::models::MentionCandidate> = if member_scoped {
        let Some(pid) = project_id else {
            return Ok(Vec::new());
        };
        let mut stmt = conn.prepare_cached(
            "SELECT u.id, u.username, u.display_name
             FROM project_members m
             JOIN users u ON u.id = m.user_id
             WHERE m.project_id = ?1 AND u.is_bot = 0
             ORDER BY u.username COLLATE NOCASE",
        )?;
        stmt.query_map(params![pid], map_row)?
            .collect::<Result<Vec<_>, _>>()?
    } else {
        let mut stmt = conn.prepare_cached(
            "SELECT id, username, display_name FROM users
             WHERE is_bot = 0 ORDER BY username COLLATE NOCASE",
        )?;
        stmt.query_map([], map_row)?
            .collect::<Result<Vec<_>, _>>()?
    };
    Ok(rows)
}

/// Recompute the resolved mention set for a comment.
///
/// Parses `body` for `@username` tokens, resolves each (case-insensitively)
/// against `candidates` — the visible-member set the API layer built from
/// the same rules as [`mention_candidates`] — and reconciles the comment's
/// `comment_mentions` rows to exactly that set. Called on both create and
/// edit, so an edit that removes a mention drops its row and an edit that
/// adds one inserts it (firing the audit trigger for the new "mention"
/// activity event). Unmatched tokens are silently ignored; they remain
/// literal text in the stored body.
///
/// LIF-421, two changes with the same reason. It is a **set difference in two
/// batched statements**, not a `DELETE` of everything followed by an `INSERT`
/// per mention: the old shape was one statement per mention on every edit, and
/// worse, it deleted and re-inserted rows that had not changed, so every edit
/// to a comment re-fired the audit trigger for mentions that were already
/// there. A mention that did not change is now not touched at all. And a body
/// past [`MAX_MENTION_TOKENS`] is refused rather than resolved as far as the
/// cap, because a silently dropped mention is a notification the author
/// believes they sent.
///
/// Returns the user ids that were (re)mentioned, in body order.
pub fn sync_mentions(
    conn: &Connection,
    comment_id: i64,
    body: &str,
    candidates: &[crate::db::models::MentionCandidate],
) -> Result<Vec<i64>, LificError> {
    use std::collections::{HashMap, HashSet};
    let tokens = extract_mention_usernames(body);
    if tokens.len() > MAX_MENTION_TOKENS {
        return Err(LificError::BadRequest(format!(
            "this comment mentions more than {MAX_MENTION_TOKENS} distinct people; \
             reduce them before saving (resolving only the first {MAX_MENTION_TOKENS} \
             would drop the rest without saying so)"
        )));
    }
    let by_name: HashMap<String, i64> = candidates
        .iter()
        .map(|c| (c.username.to_lowercase(), c.user_id))
        .collect();

    let mut resolved: Vec<i64> = Vec::new();
    let mut seen: HashSet<i64> = HashSet::new();
    for token in tokens {
        if let Some(&uid) = by_name.get(&token.to_lowercase())
            && seen.insert(uid)
        {
            resolved.push(uid);
        }
    }

    let mut stmt =
        conn.prepare_cached("SELECT user_id FROM comment_mentions WHERE comment_id = ?1")?;
    let current: HashSet<i64> = stmt
        .query_map(params![comment_id], |row| row.get(0))?
        .collect::<Result<HashSet<_>, _>>()?;
    drop(stmt);

    let stale: Vec<i64> = current.difference(&seen).copied().collect();
    if !stale.is_empty() {
        let sql = format!(
            "DELETE FROM comment_mentions WHERE comment_id = ?1 AND user_id IN ({})",
            super::placeholders(stale.len())
        );
        let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(comment_id)];
        values.extend(
            stale
                .iter()
                .map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>),
        );
        let bound: Vec<&dyn rusqlite::types::ToSql> =
            values.iter().map(|value| value.as_ref()).collect();
        conn.execute(&sql, bound.as_slice())?;
    }

    let added: Vec<i64> = resolved
        .iter()
        .copied()
        .filter(|uid| !current.contains(uid))
        .collect();
    if !added.is_empty() {
        let rows = (0..added.len())
            .map(|index| format!("(?1, ?{})", index + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("INSERT INTO comment_mentions (comment_id, user_id) VALUES {rows}");
        let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(comment_id)];
        values.extend(
            added
                .iter()
                .map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>),
        );
        let bound: Vec<&dyn rusqlite::types::ToSql> =
            values.iter().map(|value| value.as_ref()).collect();
        conn.execute(&sql, bound.as_slice())?;
    }
    Ok(resolved)
}

/// Create a comment and reconcile everything derived from its body in one
/// write: resolved mentions and attachment links.
///
/// `author` is who the comment belongs to; `attachments` is whose reach its
/// `/api/attachments/{id}` references inherit. They are separate arguments on
/// purpose (LIF-409): the direct-SQL CLI attributes the comment to a fallback
/// administrator without granting that administrator's reach.
///
/// LIF-409: runs in its own SAVEPOINT so the comment, its mentions and its
/// links are all-or-nothing on any caller's connection, whether or not that
/// caller opened a transaction of its own. The CLI, which writes without one,
/// used to leave a comment behind when mention resolution failed.
pub fn create_comment_with_mentions(
    conn: &Connection,
    parent: CommentParent,
    project_id: Option<i64>,
    author: CommentActor,
    attachments: AttachmentActor,
    content: &str,
    member_scoped: bool,
) -> Result<Comment, LificError> {
    super::savepoint(conn, "create_comment_with_mentions", || {
        let candidates = mention_candidates(conn, project_id, member_scoped)?;
        let comment = insert_comment_row(conn, parent, author.user_id, content)?;
        sync_mentions(conn, comment.id, &comment.content, &candidates)?;
        super::attachments::sync_links(
            conn,
            AttachmentEntity::Comment,
            comment.id,
            &comment.content,
            attachments,
            project_id,
        )?;
        Ok(comment)
    })
}

/// Edit a comment's content and re-derive its mentions and attachment links.
/// See [`create_comment_with_mentions`] for the `author` / `attachments`
/// split and the savepoint. An edit reconciles with the *editor's* reach, not
/// the original author's: introducing a reference is the editor's act.
pub fn update_comment_with_mentions(
    conn: &Connection,
    comment_id: i64,
    project_id: Option<i64>,
    attachments: AttachmentActor,
    content: &str,
    member_scoped: bool,
) -> Result<Comment, LificError> {
    super::savepoint(conn, "update_comment_with_mentions", || {
        let candidates = mention_candidates(conn, project_id, member_scoped)?;
        let comment = write_comment_content(conn, comment_id, content)?;
        sync_mentions(conn, comment.id, &comment.content, &candidates)?;
        super::attachments::sync_links(
            conn,
            AttachmentEntity::Comment,
            comment.id,
            &comment.content,
            attachments,
            project_id,
        )?;
        Ok(comment)
    })
}

/// The user ids currently recorded as mentioned by a comment. Test-only
/// read helper — production reads mentions through the audit feed / render
/// pipeline, not this table directly.
#[cfg(test)]
pub fn list_mention_user_ids(conn: &Connection, comment_id: i64) -> Result<Vec<i64>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT user_id FROM comment_mentions WHERE comment_id = ?1 ORDER BY user_id",
    )?;
    let rows = stmt.query_map(params![comment_id], |row| row.get(0))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn row_to_comment(row: &rusqlite::Row) -> Result<Comment, rusqlite::Error> {
    Ok(Comment {
        id: row.get(0)?,
        issue_id: row.get(1)?,
        page_id: row.get(2)?,
        user_id: row.get(3)?,
        author: row.get(4)?,
        author_display_name: row.get(5)?,
        content: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        seq: row.get::<_, Option<i64>>(9)?.unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::models::*;
    use crate::db::queries;

    /// Seed a user, a project, an issue, and a page. Returns (pool, issue_id, page_id, user_id).
    fn setup() -> (db::DbPool, i64, i64, i64) {
        let pool = db::open_memory().expect("test db");
        let conn = pool.write().unwrap();

        let user = queries::users::create_user(
            &conn,
            &CreateUser {
                username: "blake".into(),
                email: "blake@test.com".into(),
                password: "testpassword1".into(),
                display_name: Some("Blake".into()),
                is_admin: true,
                is_bot: false,
            },
        )
        .unwrap();

        let project = queries::create_project(
            &conn,
            &CreateProject {
                name: "Test".into(),
                identifier: "TST".into(),
                ..Default::default()
            },
        )
        .unwrap();

        let issue = queries::create_issue(
            &conn,
            &CreateIssue {
                project_id: project.id,
                title: "Test issue".into(),
                status: Status::Todo,
                priority: Priority::Medium,
                ..Default::default()
            },
        )
        .unwrap();

        let page = queries::create_page(
            &conn,
            &CreatePage {
                project_id: Some(project.id),
                title: "Test page".into(),
                content: "Body".into(),
                ..Default::default()
            },
        )
        .unwrap();

        drop(conn);
        (pool, issue.id, page.id, user.id)
    }

    #[test]
    fn comment_body_limit_is_inclusive_for_create_and_update() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        let boundary = "x".repeat(MAX_COMMENT_BYTES);
        let comment = create_comment(&conn, CommentParent::Issue(issue_id), user_id, &boundary)
            .expect("the maximum comment body is allowed");
        assert_eq!(comment.content.len(), MAX_COMMENT_BYTES);

        let escaped_boundary = format!("{}\\n", "x".repeat(MAX_COMMENT_BYTES - 1));
        let normalized = create_comment(
            &conn,
            CommentParent::Issue(issue_id),
            user_id,
            &escaped_boundary,
        )
        .expect("the limit applies to normalized content");
        assert_eq!(normalized.content.len(), MAX_COMMENT_BYTES);

        let oversized = format!("{boundary}x");
        assert!(matches!(
            create_comment(&conn, CommentParent::Issue(issue_id), user_id, &oversized),
            Err(crate::error::LificError::BadRequest(_))
        ));
        assert!(matches!(
            update_comment(&conn, comment.id, &oversized),
            Err(crate::error::LificError::BadRequest(_))
        ));
        assert_eq!(get_comment(&conn, comment.id).unwrap().content, boundary);
    }

    #[test]
    fn create_and_list_issue_comments() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();

        let c1 = create_comment(&conn, CommentParent::Issue(issue_id), user_id, "First").unwrap();
        assert_eq!(c1.content, "First");
        assert_eq!(c1.author, "blake");
        assert_eq!(c1.author_display_name, "Blake");
        assert_eq!(c1.issue_id, Some(issue_id));
        assert_eq!(c1.page_id, None);
        assert_eq!(c1.user_id, user_id);

        create_comment(&conn, CommentParent::Issue(issue_id), user_id, "Second").unwrap();

        let comments = list_comments(&conn, CommentParent::Issue(issue_id), None, None).unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].content, "First");
        assert_eq!(comments[1].content, "Second");
    }

    #[test]
    fn create_page_comment_and_list() {
        let (pool, _, page_id, user_id) = setup();
        let conn = pool.write().unwrap();

        let c1 =
            create_comment(&conn, CommentParent::Page(page_id), user_id, "Hello page").unwrap();
        assert_eq!(c1.content, "Hello page");
        assert_eq!(c1.issue_id, None);
        assert_eq!(c1.page_id, Some(page_id));

        create_comment(&conn, CommentParent::Page(page_id), user_id, "Another").unwrap();

        let comments = list_comments(&conn, CommentParent::Page(page_id), None, None).unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].content, "Hello page");
        assert_eq!(comments[1].content, "Another");
    }

    #[test]
    fn comment_attachment_scope_is_independent_of_authz_enforcement() {
        let (pool, issue_id, _, _) = setup();
        let conn = pool.write().unwrap();
        let [editor, owner] =
            [("editor", "Editor"), ("owner", "Owner")].map(|(username, display_name)| {
                queries::users::create_user(
                    &conn,
                    &CreateUser {
                        username: username.into(),
                        email: format!("{username}@test.com"),
                        password: "testpassword1".into(),
                        display_name: Some(display_name.into()),
                        is_admin: false,
                        is_bot: false,
                    },
                )
                .unwrap()
            });
        let editor = CommentActor {
            user_id: editor.id,
            is_admin: editor.is_admin,
        };
        let other_project = queries::create_project(
            &conn,
            &CreateProject {
                name: "Other".into(),
                identifier: "OTH".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let other_issue = queries::create_issue(
            &conn,
            &CreateIssue {
                project_id: other_project.id,
                title: "Other issue".into(),
                status: Status::Todo,
                priority: Priority::Medium,
                ..Default::default()
            },
        )
        .unwrap();
        let attachment = queries::attachments::create_attachment(
            &conn,
            &crate::storage::AttachmentStore::hash_bytes(b"foreign"),
            "foreign.txt",
            "text/plain",
            7,
            Some(owner.id),
        )
        .unwrap();
        queries::attachments::link_attachment(
            &conn,
            attachment.id,
            AttachmentEntity::Issue,
            other_issue.id,
        )
        .unwrap();

        let project_id = queries::get_issue(&conn, issue_id).unwrap().project_id;
        let content = format!("[foreign](/api/attachments/{})", attachment.id);
        let comment = create_comment_with_mentions(
            &conn,
            CommentParent::Issue(issue_id),
            Some(project_id),
            editor,
            AttachmentActor::Authenticated(editor),
            &content,
            true,
        )
        .unwrap();
        assert!(
            queries::attachments::list_for_entity(&conn, AttachmentEntity::Comment, comment.id,)
                .unwrap()
                .is_empty()
        );

        queries::attachments::link_attachment(
            &conn,
            attachment.id,
            AttachmentEntity::Issue,
            issue_id,
        )
        .unwrap();
        update_comment_with_mentions(
            &conn,
            comment.id,
            Some(project_id),
            AttachmentActor::Authenticated(editor),
            &content,
            true,
        )
        .unwrap();
        assert_eq!(
            queries::attachments::list_for_entity(&conn, AttachmentEntity::Comment, comment.id,)
                .unwrap()
                .len(),
            1
        );
    }

    // ── Author filter + sort direction ────────────────────────

    #[test]
    fn list_comments_filters_by_author() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        let other = queries::users::create_user(
            &conn,
            &CreateUser {
                username: "Ada".into(),
                email: "ada@test.com".into(),
                password: "testpassword1".into(),
                display_name: Some("Ada".into()),
                is_admin: false,
                is_bot: true,
            },
        )
        .unwrap();

        create_comment(&conn, CommentParent::Issue(issue_id), user_id, "from blake").unwrap();
        create_comment(&conn, CommentParent::Issue(issue_id), other.id, "from Ada").unwrap();

        let ada_only =
            list_comments(&conn, CommentParent::Issue(issue_id), Some("ada"), None).unwrap();
        assert_eq!(ada_only.len(), 1);
        assert_eq!(ada_only[0].content, "from Ada");
        assert_eq!(
            count_comments(&conn, CommentParent::Issue(issue_id), Some("ada")).unwrap(),
            1
        );
        assert_eq!(
            count_comments(&conn, CommentParent::Issue(issue_id), None).unwrap(),
            2
        );

        // Username match is case-insensitive — agents shouldn't have to
        // know the stored casing.
        let ada_caps =
            list_comments(&conn, CommentParent::Issue(issue_id), Some("ADA"), None).unwrap();
        assert_eq!(ada_caps.len(), 1);

        let nobody =
            list_comments(&conn, CommentParent::Issue(issue_id), Some("ghost"), None).unwrap();
        assert!(nobody.is_empty());
    }

    #[test]
    fn list_comments_paginated_clamps_negative_offset_to_zero() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        for content in ["first", "second", "third"] {
            create_comment(&conn, CommentParent::Issue(issue_id), user_id, content).unwrap();
        }

        let comments = list_comments_exhaustive(
            &conn,
            CommentParent::Issue(issue_id),
            None,
            None,
            Some(2),
            Some(-10),
        )
        .unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].content, "first");
        assert_eq!(comments[1].content, "second");
    }

    #[test]
    fn list_comments_paginated_clamps_limit_to_max_page_limit() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        for index in 0..502 {
            create_comment(
                &conn,
                CommentParent::Issue(issue_id),
                user_id,
                &format!("comment {index}"),
            )
            .unwrap();
        }

        let comments = list_comments_exhaustive(
            &conn,
            CommentParent::Issue(issue_id),
            None,
            None,
            Some(9999),
            None,
        )
        .unwrap();
        assert_eq!(comments.len(), super::super::MAX_PAGE_LIMIT as usize);
    }

    #[test]
    fn list_comments_desc_returns_newest_first() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        let c1 = create_comment(&conn, CommentParent::Issue(issue_id), user_id, "oldest").unwrap();
        let c2 = create_comment(&conn, CommentParent::Issue(issue_id), user_id, "newest").unwrap();
        // datetime('now') is 1-second resolution, so both rows likely share
        // a timestamp; pin them apart to make the assertion meaningful.
        conn.execute(
            "UPDATE comments SET created_at = '2026-01-01 00:00:00' WHERE id = ?1",
            params![c1.id],
        )
        .unwrap();
        conn.execute(
            "UPDATE comments SET created_at = '2026-02-01 00:00:00' WHERE id = ?1",
            params![c2.id],
        )
        .unwrap();

        let desc =
            list_comments(&conn, CommentParent::Issue(issue_id), None, Some("desc")).unwrap();
        assert_eq!(desc[0].content, "newest");
        assert_eq!(desc[1].content, "oldest");

        let asc = list_comments(&conn, CommentParent::Issue(issue_id), None, Some("asc")).unwrap();
        assert_eq!(asc[0].content, "oldest");
    }

    /// Keyset paging names a row's place by the ordering key itself, so it
    /// survives writes that an offset cannot: a comment posted above the
    /// reader shifts every offset by one, and the reader silently re-reads a
    /// row or skips one. The cursor asks a question inserts cannot change.
    #[test]
    fn keyset_pages_backwards_stably_while_the_thread_grows() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        let parent = CommentParent::Issue(issue_id);
        // Same timestamp on every row, so the id half of the cursor is what
        // makes the boundary exact. This is the common case in practice:
        // created_at has one-second resolution.
        for index in 1..=6 {
            let comment =
                create_comment(&conn, parent, user_id, &format!("comment {index}")).unwrap();
            conn.execute(
                "UPDATE comments SET created_at = '2026-01-01 00:00:00' WHERE id = ?1",
                params![comment.id],
            )
            .unwrap();
        }

        let newest =
            list_comments_keyset(&conn, parent, None, Some("desc"), Some(2), None, None).unwrap();
        assert_eq!(
            newest
                .items
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>(),
            ["comment 6", "comment 5"]
        );
        assert!(newest.has_more);

        // A new comment lands while the reader is paging. With an offset the
        // next page would repeat "comment 5"; the cursor is unmoved.
        create_comment(&conn, parent, user_id, "comment 7").unwrap();
        let cursor = CommentCursor::before(newest.items.last().unwrap());
        let older = list_comments_keyset(
            &conn,
            parent,
            None,
            Some("desc"),
            Some(2),
            None,
            Some(&cursor),
        )
        .unwrap();
        assert_eq!(
            older
                .items
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>(),
            ["comment 4", "comment 3"]
        );
        assert!(older.has_more);

        // Paging to the start reports no more, and the pages never overlap.
        let cursor = CommentCursor::before(older.items.last().unwrap());
        let tail = list_comments_keyset(
            &conn,
            parent,
            None,
            Some("desc"),
            Some(2),
            None,
            Some(&cursor),
        )
        .unwrap();
        assert_eq!(
            tail.items
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>(),
            ["comment 2", "comment 1"]
        );
        assert!(!tail.has_more);
    }

    #[test]
    fn keyset_requires_desc_and_no_offset() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        let parent = CommentParent::Issue(issue_id);
        let comment = create_comment(&conn, parent, user_id, "only").unwrap();
        let cursor = CommentCursor::before(&comment);

        // A "before" cursor only describes paging backwards.
        assert!(matches!(
            list_comments_keyset(
                &conn,
                parent,
                None,
                Some("asc"),
                Some(2),
                None,
                Some(&cursor)
            ),
            Err(LificError::BadRequest(_))
        ));
        assert!(matches!(
            list_comments_keyset(&conn, parent, None, None, Some(2), None, Some(&cursor)),
            Err(LificError::BadRequest(_))
        ));
        // Skipping relative to a position that already skipped is incoherent.
        assert!(matches!(
            list_comments_keyset(
                &conn,
                parent,
                None,
                Some("desc"),
                Some(2),
                Some(5),
                Some(&cursor)
            ),
            Err(LificError::BadRequest(_))
        ));
        // An explicit zero offset is the same request as no offset.
        assert!(
            list_comments_keyset(
                &conn,
                parent,
                None,
                Some("desc"),
                Some(2),
                Some(0),
                Some(&cursor)
            )
            .is_ok()
        );
    }

    /// A cursor is caller-controlled text. It must be a bound parameter, not
    /// spliced into the statement.
    #[test]
    fn keyset_cursor_is_bound_not_interpolated() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        let parent = CommentParent::Issue(issue_id);
        create_comment(&conn, parent, user_id, "survivor").unwrap();

        // Sorts before any real timestamp, so a bound parameter matches
        // nothing. Interpolated, the trailing `OR '1'='1` would either widen
        // the predicate to every row or blow up as a syntax error.
        let hostile = CommentCursor {
            created_at: "0000-01-01' OR '1'='1".into(),
            id: i64::MAX,
        };
        let page = list_comments_keyset(
            &conn,
            parent,
            None,
            Some("desc"),
            Some(10),
            None,
            Some(&hostile),
        )
        .unwrap();
        // The literal never matches a real timestamp, so nothing comes back
        // and, crucially, the row is still there afterwards.
        assert!(page.items.is_empty());
        assert_eq!(count_comments(&conn, parent, None).unwrap(), 1);
    }

    #[test]
    fn list_comments_rejects_invalid_order() {
        let (pool, issue_id, _, _) = setup();
        let conn = pool.read().unwrap();
        assert!(
            list_comments(&conn, CommentParent::Issue(issue_id), None, Some("newest")).is_err()
        );
    }

    #[test]
    fn page_and_issue_comment_threads_are_independent() {
        let (pool, issue_id, page_id, user_id) = setup();
        let conn = pool.write().unwrap();

        create_comment(
            &conn,
            CommentParent::Issue(issue_id),
            user_id,
            "Issue thread",
        )
        .unwrap();
        create_comment(&conn, CommentParent::Page(page_id), user_id, "Page thread").unwrap();

        let issue_comments =
            list_comments(&conn, CommentParent::Issue(issue_id), None, None).unwrap();
        let page_comments = list_comments(&conn, CommentParent::Page(page_id), None, None).unwrap();

        assert_eq!(issue_comments.len(), 1);
        assert_eq!(issue_comments[0].content, "Issue thread");
        assert_eq!(page_comments.len(), 1);
        assert_eq!(page_comments[0].content, "Page thread");
    }

    #[test]
    fn get_comment_by_id() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();

        let created =
            create_comment(&conn, CommentParent::Issue(issue_id), user_id, "Hello").unwrap();
        let fetched = get_comment(&conn, created.id).unwrap();
        assert_eq!(fetched.content, "Hello");
        assert_eq!(fetched.author, "blake");
        assert_eq!(fetched.issue_id, Some(issue_id));
        assert_eq!(fetched.page_id, None);
    }

    #[test]
    fn update_comment_content() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();

        let created =
            create_comment(&conn, CommentParent::Issue(issue_id), user_id, "Original").unwrap();
        let updated = update_comment(&conn, created.id, "Edited").unwrap();
        assert_eq!(updated.content, "Edited");
        assert_eq!(updated.id, created.id);
    }

    #[test]
    fn delete_comment_removes_it() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();

        let created =
            create_comment(&conn, CommentParent::Issue(issue_id), user_id, "Delete me").unwrap();
        delete_comment(&conn, created.id).unwrap();

        assert!(get_comment(&conn, created.id).is_err());
    }

    #[test]
    fn comment_on_nonexistent_issue_fails() {
        let (pool, _, _, user_id) = setup();
        let conn = pool.write().unwrap();

        let result = create_comment(&conn, CommentParent::Issue(99999), user_id, "Orphan");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn comment_on_nonexistent_page_fails() {
        let (pool, _, _, user_id) = setup();
        let conn = pool.write().unwrap();

        let result = create_comment(&conn, CommentParent::Page(99999), user_id, "Orphan");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn delete_nonexistent_comment_fails() {
        let (pool, _, _, _) = setup();
        let conn = pool.write().unwrap();

        let result = delete_comment(&conn, 99999);
        assert!(result.is_err());
    }

    #[test]
    fn comments_cascade_on_issue_delete() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();

        let c = create_comment(
            &conn,
            CommentParent::Issue(issue_id),
            user_id,
            "Will be cascaded",
        )
        .unwrap();
        queries::delete_issue(&conn, issue_id).unwrap();

        assert!(get_comment(&conn, c.id).is_err());
    }

    #[test]
    fn page_comment_cascade_on_page_delete() {
        let (pool, _, page_id, user_id) = setup();
        let conn = pool.write().unwrap();

        let c = create_comment(&conn, CommentParent::Page(page_id), user_id, "Cascade me").unwrap();
        queries::delete_page(&conn, page_id).unwrap();

        assert!(get_comment(&conn, c.id).is_err());
    }

    // ── LIF-438: comment tombstones and the parent cascade ───

    fn raw_comment(conn: &Connection, id: i64) -> (Option<String>, i64) {
        conn.query_row(
            "SELECT deleted_at, seq FROM comments WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get::<_, Option<i64>>(1)?.unwrap_or(0))),
        )
        .unwrap()
    }

    #[test]
    fn deleting_a_comment_leaves_a_tombstone_with_a_fresh_seq() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        let c = create_comment(&conn, CommentParent::Issue(issue_id), user_id, "Bye").unwrap();
        let (_, before) = raw_comment(&conn, c.id);

        delete_comment(&conn, c.id).unwrap();

        let (deleted_at, seq) = raw_comment(&conn, c.id);
        assert!(deleted_at.is_some());
        assert!(seq > before);
        assert_eq!(
            count_comments(&conn, CommentParent::Issue(issue_id), None).unwrap(),
            0
        );
        assert!(
            list_comments(&conn, CommentParent::Issue(issue_id), None, None)
                .unwrap()
                .is_empty()
        );
        assert!(update_comment(&conn, c.id, "resurrect me").is_err());
    }

    #[test]
    fn deleting_an_issue_tombstones_its_comments_with_their_own_seqs() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        let first = create_comment(&conn, CommentParent::Issue(issue_id), user_id, "One").unwrap();
        let second = create_comment(&conn, CommentParent::Issue(issue_id), user_id, "Two").unwrap();
        let (_, first_seq) = raw_comment(&conn, first.id);
        let (_, second_seq) = raw_comment(&conn, second.id);

        queries::delete_issue(&conn, issue_id).unwrap();

        let (first_deleted, first_after) = raw_comment(&conn, first.id);
        let (second_deleted, second_after) = raw_comment(&conn, second.id);
        assert!(first_deleted.is_some() && second_deleted.is_some());
        assert!(first_after > first_seq);
        assert!(second_after > second_seq);
        // The cascade copies the parent's exact timestamp; that shared value is
        // what makes the restore below selective.
        assert_eq!(first_deleted, second_deleted);
        let issue_deleted: Option<String> = conn
            .query_row(
                "SELECT deleted_at FROM issues WHERE id = ?1",
                params![issue_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(first_deleted, issue_deleted);
    }

    #[test]
    fn restoring_an_issue_revives_only_the_comments_that_went_with_it() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        let earlier =
            create_comment(&conn, CommentParent::Issue(issue_id), user_id, "Retracted").unwrap();
        let cascaded =
            create_comment(&conn, CommentParent::Issue(issue_id), user_id, "Innocent").unwrap();

        // Deleted on its own first, so it carries a different `deleted_at`.
        // Backdated a day so the test asserts the cascade's *matching rule*
        // rather than how many milliseconds apart two statements happen to run.
        delete_comment(&conn, earlier.id).unwrap();
        conn.execute(
            "UPDATE comments SET deleted_at = datetime(deleted_at, '-1 day') WHERE id = ?1",
            params![earlier.id],
        )
        .unwrap();
        queries::delete_issue(&conn, issue_id).unwrap();
        queries::restore_issue(&conn, issue_id).unwrap();

        assert!(
            get_comment(&conn, cascaded.id).is_ok(),
            "a comment that went down with the issue comes back with it"
        );
        assert!(
            get_comment(&conn, earlier.id).is_err(),
            "a comment deleted beforehand stays deleted"
        );
        let live = list_comments(&conn, CommentParent::Issue(issue_id), None, None).unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].content, "Innocent");
    }

    #[test]
    fn restoring_a_page_revives_its_cascaded_comments() {
        let (pool, _, page_id, user_id) = setup();
        let conn = pool.write().unwrap();
        let c = create_comment(&conn, CommentParent::Page(page_id), user_id, "Doc note").unwrap();
        queries::delete_page(&conn, page_id).unwrap();
        assert!(get_comment(&conn, c.id).is_err());

        queries::restore_page(&conn, page_id).unwrap();
        assert!(get_comment(&conn, c.id).is_ok());
    }

    #[test]
    fn a_deleted_issue_accepts_no_new_comments() {
        let (pool, issue_id, page_id, user_id) = setup();
        let conn = pool.write().unwrap();
        queries::delete_issue(&conn, issue_id).unwrap();
        queries::delete_page(&conn, page_id).unwrap();

        let issue_err = create_comment(&conn, CommentParent::Issue(issue_id), user_id, "Late")
            .unwrap_err()
            .to_string();
        assert!(issue_err.contains("not found"), "{issue_err}");
        let page_err = create_comment(&conn, CommentParent::Page(page_id), user_id, "Late")
            .unwrap_err()
            .to_string();
        assert!(page_err.contains("not found"), "{page_err}");
    }

    #[test]
    fn comment_delete_and_restore_are_audited_once_each() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        let c = create_comment(&conn, CommentParent::Issue(issue_id), user_id, "Logged").unwrap();

        delete_comment(&conn, c.id).unwrap();
        conn.execute(
            "UPDATE comments SET deleted_at = datetime(deleted_at, '-1 day') WHERE id = ?1",
            params![c.id],
        )
        .unwrap();
        queries::delete_issue(&conn, issue_id).unwrap();
        queries::restore_issue(&conn, issue_id).unwrap();

        let actions: Vec<String> = conn
            .prepare(
                "SELECT action FROM audit_log
                  WHERE entity_type = 'comment' AND entity_id = ?1 ORDER BY id",
            )
            .unwrap()
            .query_map(params![c.id], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            actions,
            vec!["create", "delete"],
            "the parent's restore must not log a 'restored' for a comment it did not revive"
        );
    }

    #[test]
    fn comment_check_constraint_rejects_both_parents_set() {
        let (pool, issue_id, page_id, user_id) = setup();
        let conn = pool.write().unwrap();

        // Bypass the safe enum and try to insert a row with both parents set.
        let result = conn.execute(
            "INSERT INTO comments (issue_id, page_id, user_id, content)
             VALUES (?1, ?2, ?3, 'bad')",
            params![issue_id, page_id, user_id],
        );
        assert!(
            result.is_err(),
            "expected CHECK constraint to reject dual-parent row"
        );
        let msg = result.unwrap_err().to_string().to_lowercase();
        assert!(
            msg.contains("check") || msg.contains("constraint"),
            "expected CHECK-constraint error, got: {msg}"
        );
    }

    #[test]
    fn comment_check_constraint_rejects_no_parent_set() {
        let (pool, _, _, user_id) = setup();
        let conn = pool.write().unwrap();

        let result = conn.execute(
            "INSERT INTO comments (issue_id, page_id, user_id, content)
             VALUES (NULL, NULL, ?1, 'orphan')",
            params![user_id],
        );
        assert!(
            result.is_err(),
            "expected CHECK constraint to reject parentless row"
        );
        let msg = result.unwrap_err().to_string().to_lowercase();
        assert!(
            msg.contains("check") || msg.contains("constraint"),
            "expected CHECK-constraint error, got: {msg}"
        );
    }

    #[test]
    fn comment_unescapes_newlines() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();

        let c = create_comment(
            &conn,
            CommentParent::Issue(issue_id),
            user_id,
            "line1\\nline2",
        )
        .unwrap();
        assert_eq!(c.content, "line1\nline2");
    }

    #[test]
    fn list_comments_empty_issue() {
        let (pool, issue_id, _, _) = setup();
        let conn = pool.read().unwrap();

        let comments = list_comments(&conn, CommentParent::Issue(issue_id), None, None).unwrap();
        assert!(comments.is_empty());
    }

    #[test]
    fn list_comments_empty_page() {
        let (pool, _, page_id, _) = setup();
        let conn = pool.read().unwrap();

        let comments = list_comments(&conn, CommentParent::Page(page_id), None, None).unwrap();
        assert!(comments.is_empty());
    }

    // LIF-388: the page size that matters most is the cap itself. When the
    // over-fetch lived at the transport, asking for MAX_PAGE_LIMIT comments
    // and then fetching MAX_PAGE_LIMIT + 1 got clamped straight back to the
    // cap, so `has_more` was false on a thread that plainly had more. The
    // over-fetch now happens inside the query, after its own clamp.
    #[test]
    fn has_more_holds_at_the_page_cap() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        // One comment past a full capped page. Inserted directly: this test is
        // about the LIMIT arithmetic, not about comment creation.
        for n in 0..=super::super::MAX_PAGE_LIMIT {
            conn.execute(
                "INSERT INTO comments (issue_id, user_id, content) VALUES (?1, ?2, ?3)",
                params![issue_id, user_id, format!("comment {n}")],
            )
            .unwrap();
        }

        let capped = list_comments_page(
            &conn,
            CommentParent::Issue(issue_id),
            None,
            None,
            Some(super::super::MAX_PAGE_LIMIT),
            None,
        )
        .unwrap();
        assert_eq!(capped.items.len() as i64, super::super::MAX_PAGE_LIMIT);
        assert!(
            capped.has_more,
            "a capped page with a row past it must report has_more"
        );

        // Over the cap clamps down to it, and the answer must not change.
        let over_cap = list_comments_page(
            &conn,
            CommentParent::Issue(issue_id),
            None,
            None,
            Some(super::super::MAX_PAGE_LIMIT + 100),
            None,
        )
        .unwrap();
        assert_eq!(over_cap.items.len() as i64, super::super::MAX_PAGE_LIMIT);
        assert!(over_cap.has_more);

        // The last page has nothing past it.
        let tail = list_comments_page(
            &conn,
            CommentParent::Issue(issue_id),
            None,
            None,
            Some(super::super::MAX_PAGE_LIMIT),
            Some(super::super::MAX_PAGE_LIMIT),
        )
        .unwrap();
        assert_eq!(tail.items.len(), 1);
        assert!(!tail.has_more);
    }

    /// Read an issue's raw updated_at timestamp directly from the table.
    fn issue_updated_at(conn: &Connection, issue_id: i64) -> String {
        conn.query_row(
            "SELECT updated_at FROM issues WHERE id = ?1",
            params![issue_id],
            |row| row.get(0),
        )
        .unwrap()
    }

    // LIF-116: creating a comment is "activity" on the parent issue, so the
    // trigger added in migration 017 must bump issues.updated_at. SQLite's
    // datetime('now') is 1-second resolution, so we sleep > 1s to guarantee a
    // strictly-greater timestamp.
    #[test]
    fn creating_comment_bumps_issue_updated_at() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();

        let before = issue_updated_at(&conn, issue_id);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        create_comment(&conn, CommentParent::Issue(issue_id), user_id, "Activity").unwrap();
        let after = issue_updated_at(&conn, issue_id);

        assert!(
            after > before,
            "expected comment creation to bump issue updated_at: before={before}, after={after}"
        );
    }

    // LIF-116: deleting a comment is also activity; the AFTER DELETE trigger
    // bumps updated_at using OLD.issue_id.
    #[test]
    fn deleting_comment_bumps_issue_updated_at() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();

        let c = create_comment(&conn, CommentParent::Issue(issue_id), user_id, "Temp").unwrap();
        let before = issue_updated_at(&conn, issue_id);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        delete_comment(&conn, c.id).unwrap();
        let after = issue_updated_at(&conn, issue_id);

        assert!(
            after > before,
            "expected comment deletion to bump issue updated_at: before={before}, after={after}"
        );
    }

    // ── LIF-263: @mention extraction + sync ──────────────────

    #[test]
    fn extract_basic_and_dedup() {
        assert_eq!(extract_mention_usernames("hey @ada"), vec!["ada"]);
        // Multiple distinct mentions, in order.
        assert_eq!(
            extract_mention_usernames("@ada and @blake ship it"),
            vec!["ada", "blake"]
        );
        // Duplicates collapse (case-insensitively), first spelling kept.
        assert_eq!(extract_mention_usernames("@ada @Ada @ADA"), vec!["ada"]);
    }

    #[test]
    fn extract_respects_punctuation_boundaries() {
        // Trailing punctuation isn't part of the username.
        assert_eq!(extract_mention_usernames("thanks @ada, nice"), vec!["ada"]);
        assert_eq!(extract_mention_usernames("(@bob) here"), vec!["bob"]);
        assert_eq!(extract_mention_usernames("cc: @ada."), vec!["ada"]);
        // Start of string.
        assert_eq!(extract_mention_usernames("@lead go"), vec!["lead"]);
        // Underscores and hyphens are valid username chars.
        assert_eq!(
            extract_mention_usernames("ping @opencode-blake now"),
            vec!["opencode-blake"]
        );
    }

    #[test]
    fn extract_ignores_emails_and_midword_at() {
        // Email: the `@` is preceded by a username char, so no boundary.
        assert!(extract_mention_usernames("mail me at ada@example.com").is_empty());
        // Mid-word @ (no boundary before).
        assert!(extract_mention_usernames("a@b c").is_empty());
        // Bare `@` with nothing after yields nothing.
        assert!(extract_mention_usernames("just @ symbol").is_empty());
    }

    /// Build a candidate list straight from usernames for sync tests.
    fn candidates(rows: &[(i64, &str)]) -> Vec<crate::db::models::MentionCandidate> {
        rows.iter()
            .map(|(id, name)| crate::db::models::MentionCandidate {
                user_id: *id,
                username: (*name).into(),
                display_name: (*name).into(),
            })
            .collect()
    }

    #[test]
    fn sync_resolves_only_visible_members() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        let ada = queries::users::create_user(
            &conn,
            &CreateUser {
                username: "ada".into(),
                email: "ada@test.com".into(),
                password: "testpassword1".into(),
                display_name: Some("Ada".into()),
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap();

        let c = create_comment(
            &conn,
            CommentParent::Issue(issue_id),
            user_id,
            "hey @ada and @ghost",
        )
        .unwrap();

        // Only `ada` is a candidate; `ghost` is unmatched and stays literal.
        let cands = candidates(&[(ada.id, "ada")]);
        let resolved = sync_mentions(&conn, c.id, &c.content, &cands).unwrap();
        assert_eq!(resolved, vec![ada.id]);
        assert_eq!(list_mention_user_ids(&conn, c.id).unwrap(), vec![ada.id]);
        // The literal token survives in the stored body untouched.
        assert!(c.content.contains("@ghost"));
    }

    #[test]
    fn sync_recomputes_on_edit() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        let ada = queries::users::create_user(
            &conn,
            &CreateUser {
                username: "ada".into(),
                email: "ada@test.com".into(),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap();
        let bob = queries::users::create_user(
            &conn,
            &CreateUser {
                username: "bob".into(),
                email: "bob@test.com".into(),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap();
        let cands = candidates(&[(ada.id, "ada"), (bob.id, "bob")]);

        let c = create_comment(&conn, CommentParent::Issue(issue_id), user_id, "@ada").unwrap();
        sync_mentions(&conn, c.id, "@ada", &cands).unwrap();
        assert_eq!(list_mention_user_ids(&conn, c.id).unwrap(), vec![ada.id]);

        // Edit to mention bob instead — the set is fully recomputed.
        let edited = update_comment(&conn, c.id, "now @bob").unwrap();
        sync_mentions(&conn, c.id, &edited.content, &cands).unwrap();
        assert_eq!(list_mention_user_ids(&conn, c.id).unwrap(), vec![bob.id]);

        // Edit to mention nobody — set is emptied.
        let edited = update_comment(&conn, c.id, "no mentions").unwrap();
        sync_mentions(&conn, c.id, &edited.content, &cands).unwrap();
        assert!(list_mention_user_ids(&conn, c.id).unwrap().is_empty());
    }

    #[test]
    fn sync_allows_self_mention() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        // The author "blake" mentions themselves.
        let cands = candidates(&[(user_id, "blake")]);
        let c = create_comment(
            &conn,
            CommentParent::Issue(issue_id),
            user_id,
            "note to @blake",
        )
        .unwrap();
        let resolved = sync_mentions(&conn, c.id, &c.content, &cands).unwrap();
        assert_eq!(resolved, vec![user_id]);
    }

    #[test]
    fn mention_insert_writes_activity_row() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        let ada = queries::users::create_user(
            &conn,
            &CreateUser {
                username: "ada".into(),
                email: "ada@test.com".into(),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap();
        let cands = candidates(&[(ada.id, "ada")]);
        let c = create_comment(&conn, CommentParent::Issue(issue_id), user_id, "hi @ada").unwrap();
        sync_mentions(&conn, c.id, &c.content, &cands).unwrap();

        let (action, new_value, entity_type): (String, String, String) = conn
            .query_row(
                "SELECT action, new_value, entity_type FROM audit_log
                 WHERE action = 'mention' ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(action, "mention");
        assert_eq!(new_value, "ada");
        assert_eq!(entity_type, "comment");

        // And it lands on the parent issue's feed (issue_id denormalized).
        let feed = crate::db::queries::activity::list_activity(
            &conn,
            crate::db::queries::activity::ActivityScope::Issue(issue_id),
            Some(100),
            None,
        )
        .unwrap();
        assert!(
            feed.items
                .iter()
                .any(|a| a.action == "mention" && a.new_value.as_deref() == Some("ada"))
        );
    }

    #[test]
    fn mention_candidates_all_users_when_not_scoped() {
        let (pool, _, _, _user_id) = setup();
        let conn = pool.write().unwrap();
        queries::users::create_user(
            &conn,
            &CreateUser {
                username: "ada".into(),
                email: "ada@test.com".into(),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap();
        // A bot must never be a candidate.
        queries::users::create_user(
            &conn,
            &CreateUser {
                username: "botty".into(),
                email: "botty@test.com".into(),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: false,
                is_bot: true,
            },
        )
        .unwrap();

        let cands = mention_candidates(&conn, None, false).unwrap();
        let names: Vec<&str> = cands.iter().map(|c| c.username.as_str()).collect();
        assert!(names.contains(&"blake"));
        assert!(names.contains(&"ada"));
        assert!(
            !names.contains(&"botty"),
            "bots are never mention candidates"
        );
    }

    #[test]
    fn mention_candidates_member_scoped_excludes_non_members() {
        let pool = crate::db::open_memory().expect("test db");
        let conn = pool.write().unwrap();
        let project = queries::create_project(
            &conn,
            &CreateProject {
                name: "Scoped".into(),
                identifier: "SCP".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let member = queries::users::create_user(
            &conn,
            &CreateUser {
                username: "member".into(),
                email: "m@test.com".into(),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap();
        let outsider = queries::users::create_user(
            &conn,
            &CreateUser {
                username: "outsider".into(),
                email: "o@test.com".into(),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap();
        queries::members::upsert_member(&conn, project.id, member.id, Role::Viewer).unwrap();

        let cands = mention_candidates(&conn, Some(project.id), true).unwrap();
        let ids: Vec<i64> = cands.iter().map(|c| c.user_id).collect();
        assert!(ids.contains(&member.id));
        assert!(
            !ids.contains(&outsider.id),
            "non-member must not be a candidate"
        );

        // A workspace page (no project) member-scoped yields nothing.
        assert!(mention_candidates(&conn, None, true).unwrap().is_empty());
    }

    #[test]
    fn multiple_users_comment() {
        let (pool, issue_id, _, user1_id) = setup();
        let conn = pool.write().unwrap();

        let user2 = queries::users::create_user(
            &conn,
            &CreateUser {
                username: "ada".into(),
                email: "ada@test.com".into(),
                password: "testpassword2".into(),
                display_name: Some("Ada".into()),
                is_admin: false,
                is_bot: true,
            },
        )
        .unwrap();

        create_comment(
            &conn,
            CommentParent::Issue(issue_id),
            user1_id,
            "Blake says hi",
        )
        .unwrap();
        create_comment(
            &conn,
            CommentParent::Issue(issue_id),
            user2.id,
            "Ada responds",
        )
        .unwrap();

        let comments = list_comments(&conn, CommentParent::Issue(issue_id), None, None).unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].author, "blake");
        assert_eq!(comments[1].author, "ada");
        assert_eq!(comments[1].author_display_name, "Ada");
    }
    // ── LIF-421: bytes, not just rows ───────────────────────────

    /// Seed `count` comments whose bodies are `size` bytes each.
    fn seed_sized(conn: &Connection, issue_id: i64, user_id: i64, count: usize, size: usize) {
        for i in 0..count {
            let body = format!("{i:04}{}", "x".repeat(size.saturating_sub(4)));
            create_comment(conn, CommentParent::Issue(issue_id), user_id, &body).unwrap();
        }
    }

    #[test]
    fn a_comment_costs_what_its_json_costs_not_what_its_body_measures() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        // Every character here needs escaping: a quote and a newline are two
        // bytes each on the wire, a control character is six. Counting the raw
        // body would under-count the response by more than half.
        let hostile = "\"\n\u{1}".repeat(64);
        let plain = "x".repeat(hostile.len());
        let escaped =
            create_comment(&conn, CommentParent::Issue(issue_id), user_id, &hostile).unwrap();
        let ordinary =
            create_comment(&conn, CommentParent::Issue(issue_id), user_id, &plain).unwrap();

        assert!(
            response_cost(&escaped) > response_cost(&ordinary),
            "escaping has to be counted, or the budget is a guess"
        );
        // And the ordinary row already costs more than its body: field names,
        // the author, the timestamps and the separating comma are response
        // bytes too.
        assert!(response_cost(&ordinary) > ordinary.content.len());
        // Multi-byte UTF-8 survives the trip as its own byte length, not its
        // character count.
        let emoji =
            create_comment(&conn, CommentParent::Issue(issue_id), user_id, "🙂🙂🙂").unwrap();
        assert!(response_cost(&emoji) >= 12);
    }

    #[test]
    fn a_page_stops_at_the_longest_prefix_that_fits_the_budget() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        seed_sized(&conn, issue_id, user_id, 6, 1_000);

        let whole = list_comments_within(
            &conn,
            CommentParent::Issue(issue_id),
            Some("asc"),
            Some(50),
            Some(0),
            usize::MAX,
        )
        .unwrap();
        assert_eq!(whole.items.len(), 6);
        assert!(!whole.has_more);
        assert!(!whole.budget_limited);

        // A budget that fits two rows and part of a third.
        let budget = response_cost(&whole.items[0]) + response_cost(&whole.items[1]) + 10;
        let page = list_comments_within(
            &conn,
            CommentParent::Issue(issue_id),
            Some("asc"),
            Some(50),
            Some(0),
            budget,
        )
        .unwrap();

        // A prefix, not a subset: the rows are the first two, in order, and
        // the third was not skipped over to fit a smaller row behind it.
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.items[0].id, whole.items[0].id);
        assert_eq!(page.items[1].id, whole.items[1].id);
        assert!(page.has_more, "the budget ended the page, so more remains");
        assert!(page.budget_limited);
        // Continuation counts what was returned, not what was asked for.
        assert_eq!(page.next_offset, 2);

        // And following that offset resumes exactly where the page stopped,
        // with nothing skipped and nothing repeated.
        let next = list_comments_within(
            &conn,
            CommentParent::Issue(issue_id),
            Some("asc"),
            Some(50),
            Some(page.next_offset),
            budget,
        )
        .unwrap();
        assert_eq!(next.items[0].id, whole.items[2].id);

        // Walking the whole thread by continuation offset terminates and
        // yields every comment exactly once.
        let mut walked = Vec::new();
        let mut offset = 0;
        loop {
            let page = list_comments_within(
                &conn,
                CommentParent::Issue(issue_id),
                Some("asc"),
                Some(50),
                Some(offset),
                budget,
            )
            .unwrap();
            assert!(!page.items.is_empty(), "a page must never be empty-success");
            walked.extend(page.items.iter().map(|c| c.id));
            if !page.has_more {
                break;
            }
            offset = page.next_offset;
        }
        assert_eq!(walked, whole.items.iter().map(|c| c.id).collect::<Vec<_>>());
    }

    #[test]
    fn the_row_cap_still_binds_underneath_the_byte_budget() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        seed_sized(&conn, issue_id, user_id, 5, 10);

        let page = list_comments_within(
            &conn,
            CommentParent::Issue(issue_id),
            Some("asc"),
            Some(2),
            Some(0),
            usize::MAX,
        )
        .unwrap();
        assert_eq!(page.items.len(), 2);
        assert!(page.has_more);
        // Rows, not bytes, ended this one.
        assert!(!page.budget_limited);
        assert_eq!(page.next_offset, 2);
    }

    #[test]
    fn a_row_too_large_for_any_page_is_refused_with_the_way_around_it() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        seed_sized(&conn, issue_id, user_id, 2, 1_000);
        let first =
            list_comments(&conn, CommentParent::Issue(issue_id), None, None).unwrap()[0].clone();

        let error = list_comments_within(
            &conn,
            CommentParent::Issue(issue_id),
            Some("asc"),
            Some(50),
            Some(0),
            32,
        )
        .expect_err("a row that cannot fit any page is not a short page");
        let crate::error::LificError::PayloadTooLarge(message) = error else {
            panic!("an unreadable page is a size refusal, not a bad request");
        };
        // Names the row, its size, and how to get past it. Without the last
        // part the caller retries the same offset forever.
        assert!(
            message.contains(&format!("comment {}", first.id)),
            "{message}"
        );
        // The size named is the one the planner knew without ever reading the
        // body: its stored bytes, plus the comma that would join it to a
        // neighbour.
        assert!(
            message.contains(&(first.content.len() + 1).to_string()),
            "{message}"
        );
        assert!(message.contains("offset=1"), "{message}");

        // Newest-first reads get the keyset escape too, since that is what a
        // keyset client would have to send.
        let desc = list_comments_within(
            &conn,
            CommentParent::Issue(issue_id),
            Some("desc"),
            Some(50),
            Some(0),
            32,
        )
        .expect_err("same refusal in the other direction");
        assert!(desc.to_string().contains("before_id="), "{desc}");
    }

    #[test]
    fn an_unreadable_row_is_refused_without_ever_being_read() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        // Far past any budget, and written the way an import or a pre-cap
        // release wrote one.
        let huge = 8 * 1024 * 1024;
        let id = seed_oversized(&conn, issue_id, user_id, huge);

        let error = list_comments_within(
            &conn,
            CommentParent::Issue(issue_id),
            Some("asc"),
            Some(50),
            Some(0),
            1024,
        )
        .expect_err("8 MiB cannot fit a 1 KiB budget");
        // The refusal knows the row's id and its size, both of which came from
        // `length(CAST(content AS BLOB))`. Reading 8 MiB into memory in order
        // to announce that 8 MiB is too much to read is the denial of service
        // the budget exists to prevent.
        let message = error.to_string();
        assert!(message.contains(&format!("comment {id}")), "{message}");
        assert!(message.contains(&(huge + 1).to_string()), "{message}");

        // The planner reads sizes, not bodies: the statement it runs never
        // names `c.content` except inside `length(...)`.
        let query = CommentQuery::build(&CommentScan {
            parent: CommentParent::Issue(issue_id),
            author: None,
            order: Some("asc"),
            limit: Some(50),
            offset: Some(0),
            before: None,
            budget: Some(1024),
        })
        .unwrap();
        let (clause, _) = query.bind(1);
        let planning_sql = format!(
            "SELECT c.id, c.created_at, length(CAST(c.content AS BLOB)) {}{clause}",
            query.tail
        );
        assert!(!planning_sql.contains("c.content,"), "{planning_sql}");
    }

    #[test]
    fn an_export_read_is_exhaustive_on_purpose() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        // Well past the shipped 2 MiB budget: ten rows at 250 KiB each.
        seed_sized(&conn, issue_id, user_id, 10, 250 * 1024);

        let budgeted = list_comments_page(
            &conn,
            CommentParent::Issue(issue_id),
            None,
            Some("asc"),
            Some(50),
            Some(0),
        )
        .unwrap();
        assert!(
            budgeted.items.len() < 10,
            "the budget binds interactive reads"
        );
        assert!(budgeted.budget_limited);

        let exhaustive = list_comments_exhaustive(
            &conn,
            CommentParent::Issue(issue_id),
            None,
            Some("asc"),
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            exhaustive.len(),
            10,
            "an export that silently dropped comments is the bug the budget exists to prevent"
        );
    }

    // ── LIF-421: grandfathered bodies ───────────────────────────

    /// Write a body straight past the cap, the way an import or a pre-cap
    /// release did.
    fn seed_oversized(conn: &Connection, issue_id: i64, user_id: i64, bytes: usize) -> i64 {
        conn.execute(
            "INSERT INTO comments (issue_id, user_id, content) VALUES (?1, ?2, ?3)",
            params![issue_id, user_id, "y".repeat(bytes)],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn a_comment_that_predates_the_cap_may_shrink_but_never_grow() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        let legacy = MAX_COMMENT_BYTES + 5_000;
        let id = seed_oversized(&conn, issue_id, user_id, legacy);

        // Rewriting it at its own size is allowed: an author must be able to
        // fix a comment that was legal when they wrote it.
        update_comment(&conn, id, &"z".repeat(legacy)).expect("an edit that does not grow it");
        // As is shrinking it, all the way past the cap.
        update_comment(&conn, id, "short").expect("shrinking is always allowed");
        // But once it is inside the cap the ordinary limit applies again.
        assert!(matches!(
            update_comment(&conn, id, &"z".repeat(legacy)),
            Err(crate::error::LificError::BadRequest(_))
        ));

        // Growing a grandfathered body by even one byte is refused, and the
        // refusal explains why rather than restating the cap.
        let id = seed_oversized(&conn, issue_id, user_id, legacy);
        let error = update_comment(&conn, id, &"z".repeat(legacy + 1)).unwrap_err();
        assert!(error.to_string().contains("predates"), "{error}");
        assert_eq!(get_comment(&conn, id).unwrap().content.len(), legacy);
    }

    #[test]
    fn shrinking_a_grandfathered_comment_needs_only_a_small_request() {
        // The storage rule does not widen the transport one: the HTTP body
        // limit still bounds the *edit*, and the edit that matters for a body
        // nobody can send whole is the one that makes it smaller. Shrinking a
        // comment far past the transport limit costs a request the size of the
        // new body, not the old one.
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        let id = seed_oversized(&conn, issue_id, user_id, 4 * 1024 * 1024);

        let shrunk = update_comment(&conn, id, "sorry, pasted a log file").unwrap();
        assert_eq!(shrunk.content, "sorry, pasted a log file");
        // And it is an ordinary comment again afterwards.
        assert!(matches!(
            update_comment(&conn, id, &"z".repeat(MAX_COMMENT_BYTES + 1)),
            Err(crate::error::LificError::BadRequest(_))
        ));
    }

    #[test]
    fn the_edit_allowance_is_measured_after_normalization() {
        // `\n` in the input is two characters that become one byte, and the
        // cap has always applied to what is stored, not what was typed.
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        let id = seed_oversized(&conn, issue_id, user_id, MAX_COMMENT_BYTES + 10);
        let escaped = format!("{}\\n", "z".repeat(MAX_COMMENT_BYTES + 9));
        let stored = update_comment(&conn, id, &escaped).unwrap();
        assert_eq!(stored.content.len(), MAX_COMMENT_BYTES + 10);
    }

    // ── LIF-421: bounded derived references ─────────────────────

    #[test]
    fn a_body_past_the_mention_cap_is_refused_rather_than_half_resolved() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        // The scan stops one token past the cap: enough to prove the body is
        // over it, and bounded work either way.
        let body = (0..MAX_MENTION_TOKENS + 50).fold(String::new(), |mut body, i| {
            use std::fmt::Write;
            let _ = write!(body, "@user{i} ");
            body
        });
        assert_eq!(
            extract_mention_usernames(&body).len(),
            MAX_MENTION_TOKENS + 1
        );

        // And the write is refused, rather than resolving a prefix and
        // dropping the rest without telling the author.
        let error = sync_mentions(&conn, 1, &body, &[]).unwrap_err();
        assert!(error.to_string().contains("distinct people"), "{error}");

        // The cap counts *distinct* handles, so a body repeating one name ten
        // thousand times still resolves that one name and is not refused.
        let repeated = "@ada ".repeat(10_000);
        assert_eq!(
            extract_mention_usernames(&repeated),
            vec!["ada".to_string()]
        );
        let comment =
            create_comment(&conn, CommentParent::Issue(issue_id), user_id, &repeated).unwrap();
        sync_mentions(&conn, comment.id, &repeated, &[]).expect("one distinct handle is fine");
    }

    #[test]
    fn reconciling_mentions_leaves_the_unchanged_ones_alone() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        let roster: Vec<MentionCandidate> = ["ada", "bob", "cyd"]
            .iter()
            .map(|name| {
                let user = queries::users::create_user(
                    &conn,
                    &CreateUser {
                        username: (*name).into(),
                        email: format!("{name}@test.com"),
                        password: "testpassword1".into(),
                        display_name: Some((*name).into()),
                        is_admin: false,
                        is_bot: false,
                    },
                )
                .unwrap();
                MentionCandidate {
                    user_id: user.id,
                    username: user.username,
                    display_name: user.display_name,
                }
            })
            .collect();
        let comment =
            create_comment(&conn, CommentParent::Issue(issue_id), user_id, "@ada @bob").unwrap();
        sync_mentions(&conn, comment.id, "@ada @bob", &roster).unwrap();

        let rowid_of = |user: &MentionCandidate| -> i64 {
            conn.query_row(
                "SELECT rowid FROM comment_mentions WHERE comment_id = ?1 AND user_id = ?2",
                params![comment.id, user.user_id],
                |row| row.get(0),
            )
            .unwrap()
        };
        let ada_row = rowid_of(&roster[0]);

        // Edit: bob out, cyd in, ada untouched.
        let resolved = sync_mentions(&conn, comment.id, "@ada @cyd", &roster).unwrap();
        assert_eq!(resolved, vec![roster[0].user_id, roster[2].user_id]);
        let mut stored = list_mention_user_ids(&conn, comment.id).unwrap();
        stored.sort_unstable();
        let mut expected = vec![roster[0].user_id, roster[2].user_id];
        expected.sort_unstable();
        assert_eq!(stored, expected);
        // Ada's row is the same row, not a delete and a re-insert. That is
        // what keeps an unrelated edit from re-firing her mention in the
        // activity feed every time somebody fixes a typo.
        assert_eq!(rowid_of(&roster[0]), ada_row);
    }

    /// A body naming `count` distinct attachments.
    fn reference_body(count: usize) -> String {
        (1..=count).fold(String::new(), |mut body, id| {
            use std::fmt::Write;
            let _ = write!(body, "[f](/api/attachments/{id}) ");
            body
        })
    }

    #[test]
    fn a_body_past_the_attachment_reference_cap_is_refused_whole() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        let project_id = queries::get_issue(&conn, issue_id).unwrap().project_id;
        let actor = CommentActor {
            user_id,
            is_admin: true,
        };
        let comment = create_comment_with_mentions(
            &conn,
            CommentParent::Issue(issue_id),
            Some(project_id),
            actor,
            AttachmentActor::Authenticated(actor),
            "before",
            false,
        )
        .unwrap();

        let over_cap = reference_body(super::super::attachments::MAX_BODY_REFERENCES + 1);
        let error = update_comment_with_mentions(
            &conn,
            comment.id,
            Some(project_id),
            AttachmentActor::Authenticated(actor),
            &over_cap,
            false,
        )
        .expect_err("reconciling a truncated reference set would unlink live attachments");
        assert!(
            error.to_string().contains("distinct attachments"),
            "{error}"
        );

        // The refusal rolls the whole write back: the savepoint means the body
        // is not left saved with a half-reconciled link set.
        assert_eq!(get_comment(&conn, comment.id).unwrap().content, "before");

        // One reference under the cap still saves.
        let at_cap = reference_body(super::super::attachments::MAX_BODY_REFERENCES);
        update_comment_with_mentions(
            &conn,
            comment.id,
            Some(project_id),
            AttachmentActor::Authenticated(actor),
            &at_cap,
            false,
        )
        .expect("the cap itself is allowed");
    }
}
