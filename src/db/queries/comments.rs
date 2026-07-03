use rusqlite::{params, Connection};

use crate::db::models::Comment;
use crate::error::LificError;

use super::unescape_text;

/// What a comment is attached to.
///
/// The `comments` table allows exactly one of (issue_id, page_id) to be set
/// (enforced by a CHECK constraint added in migration 012). This enum mirrors
/// that invariant in Rust so callers can't accidentally construct an
/// orphan or dual-parent comment.
#[derive(Debug, Clone, Copy)]
pub enum CommentParent {
    Issue(i64),
    Page(i64),
}

impl CommentParent {
    fn issue_id(&self) -> Option<i64> {
        match self {
            CommentParent::Issue(id) => Some(*id),
            CommentParent::Page(_) => None,
        }
    }

    fn page_id(&self) -> Option<i64> {
        match self {
            CommentParent::Page(id) => Some(*id),
            CommentParent::Issue(_) => None,
        }
    }
}

/// Create a comment attached to an issue or page.
pub fn create_comment(
    conn: &Connection,
    parent: CommentParent,
    user_id: i64,
    content: &str,
) -> Result<Comment, LificError> {
    let content = unescape_text(content);

    // Verify the parent exists. We do this explicitly (vs. relying on the FK)
    // so the error message names the missing entity rather than surfacing a
    // raw SQLite constraint failure.
    let (table, id) = match parent {
        CommentParent::Issue(id) => ("issues", id),
        CommentParent::Page(id) => ("pages", id),
    };
    let exists: bool = conn
        .query_row(
            &format!("SELECT COUNT(*) > 0 FROM {table} WHERE id = ?1"),
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

/// Get a single comment by ID (with author info). Parent-agnostic.
pub fn get_comment(conn: &Connection, id: i64) -> Result<Comment, LificError> {
    conn.query_row(
        "SELECT c.id, c.issue_id, c.page_id, c.user_id, u.username, u.display_name,
                c.content, c.created_at, c.updated_at
         FROM comments c
         JOIN users u ON u.id = c.user_id
         WHERE c.id = ?1",
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

/// List comments for an issue or page, ordered chronologically (oldest
/// first by default; pass `order = Some("desc")` for newest first).
/// `author` filters by exact username (case-insensitive).
pub fn list_comments(
    conn: &Connection,
    parent: CommentParent,
    author: Option<&str>,
    order: Option<&str>,
) -> Result<Vec<Comment>, LificError> {
    let dir = match order {
        None | Some("asc") => "ASC",
        Some("desc") => "DESC",
        Some(other) => {
            return Err(LificError::BadRequest(format!(
                "invalid order '{other}'. Use asc or desc."
            )));
        }
    };
    let (parent_col, id) = match parent {
        CommentParent::Issue(id) => ("c.issue_id", id),
        CommentParent::Page(id) => ("c.page_id", id),
    };
    let mut sql = format!(
        "SELECT c.id, c.issue_id, c.page_id, c.user_id, u.username, u.display_name,
                c.content, c.created_at, c.updated_at
         FROM comments c
         JOIN users u ON u.id = c.user_id
         WHERE {parent_col} = ?1"
    );
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(id)];
    if let Some(username) = author {
        sql.push_str(&format!(
            " AND u.username = ?{} COLLATE NOCASE",
            param_values.len() + 1
        ));
        param_values.push(Box::new(username.to_string()));
    }
    // `dir` comes from the two-value whitelist above, never raw input.
    sql.push_str(&format!(" ORDER BY c.created_at {dir}, c.id {dir}"));

    let params_refs: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_refs.as_slice(), row_to_comment)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// Update a comment's content. Parent-agnostic.
pub fn update_comment(conn: &Connection, id: i64, content: &str) -> Result<Comment, LificError> {
    let content = unescape_text(content);

    let changed = conn.execute(
        "UPDATE comments SET content = ?1, updated_at = datetime('now') WHERE id = ?2",
        params![content, id],
    )?;

    if changed == 0 {
        return Err(LificError::NotFound(format!("comment {id} not found")));
    }

    get_comment(conn, id)
}

/// Delete a comment. Parent-agnostic.
pub fn delete_comment(conn: &Connection, id: i64) -> Result<(), LificError> {
    let changed = conn.execute("DELETE FROM comments WHERE id = ?1", params![id])?;
    if changed == 0 {
        return Err(LificError::NotFound(format!("comment {id} not found")));
    }
    Ok(())
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
/// collapsed. This is pure text parsing — it does not touch the DB, so it
/// can be unit-tested in isolation.
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
/// the same rules as [`mention_candidates`] — and rewrites the comment's
/// `comment_mentions` rows to exactly that set. Called on both create and
/// edit, so an edit that removes a mention drops its row and an edit that
/// adds one inserts it (firing the audit trigger for the new "mention"
/// activity event). Unmatched tokens are silently ignored; they remain
/// literal text in the stored body.
///
/// Returns the user ids that were (re)mentioned, in body order.
pub fn sync_mentions(
    conn: &Connection,
    comment_id: i64,
    body: &str,
    candidates: &[crate::db::models::MentionCandidate],
) -> Result<Vec<i64>, LificError> {
    use std::collections::HashMap;
    let by_name: HashMap<String, i64> = candidates
        .iter()
        .map(|c| (c.username.to_lowercase(), c.user_id))
        .collect();

    let mut resolved: Vec<i64> = Vec::new();
    let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
    for token in extract_mention_usernames(body) {
        if let Some(&uid) = by_name.get(&token.to_lowercase())
            && seen.insert(uid)
        {
            resolved.push(uid);
        }
    }

    // Rewrite the set: clear then re-insert. Cheap (a comment has a handful
    // of mentions at most) and keeps create/edit on one code path.
    conn.execute(
        "DELETE FROM comment_mentions WHERE comment_id = ?1",
        params![comment_id],
    )?;
    for &uid in &resolved {
        conn.execute(
            "INSERT INTO comment_mentions (comment_id, user_id) VALUES (?1, ?2)",
            params![comment_id, uid],
        )?;
    }
    Ok(resolved)
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
                description: String::new(),
                emoji: None,
                lead_user_id: None,
            },
        )
        .unwrap();

        let issue = queries::create_issue(
            &conn,
            &CreateIssue {
                project_id: project.id,
                title: "Test issue".into(),
                description: String::new(),
                status: "todo".into(),
                priority: "medium".into(),
                module_id: None,
                start_date: None,
                target_date: None,
                labels: vec![],
            },
        )
        .unwrap();

        let page = queries::create_page(
            &conn,
            &CreatePage {
                project_id: Some(project.id),
                folder_id: None,
                title: "Test page".into(),
                content: "Body".into(),
                status: "draft".into(),
                labels: vec![],
            },
        )
        .unwrap();

        drop(conn);
        (pool, issue.id, page.id, user.id)
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

        let c1 = create_comment(&conn, CommentParent::Page(page_id), user_id, "Hello page").unwrap();
        assert_eq!(c1.content, "Hello page");
        assert_eq!(c1.issue_id, None);
        assert_eq!(c1.page_id, Some(page_id));

        create_comment(&conn, CommentParent::Page(page_id), user_id, "Another").unwrap();

        let comments = list_comments(&conn, CommentParent::Page(page_id), None, None).unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].content, "Hello page");
        assert_eq!(comments[1].content, "Another");
    }

    // ── Author filter + sort direction ────────────────────────

    #[test]
    fn list_comments_filters_by_author() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();
        let other = queries::users::create_user(
            &conn,
            &CreateUser {
                username: "ada".into(),
                email: "ada@test.com".into(),
                password: "testpassword1".into(),
                display_name: Some("Ada".into()),
                is_admin: false,
                is_bot: true,
            },
        )
        .unwrap();

        create_comment(&conn, CommentParent::Issue(issue_id), user_id, "from blake").unwrap();
        create_comment(&conn, CommentParent::Issue(issue_id), other.id, "from ada").unwrap();

        let ada_only =
            list_comments(&conn, CommentParent::Issue(issue_id), Some("ada"), None).unwrap();
        assert_eq!(ada_only.len(), 1);
        assert_eq!(ada_only[0].content, "from ada");

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

    #[test]
    fn list_comments_rejects_invalid_order() {
        let (pool, issue_id, _, _) = setup();
        let conn = pool.read().unwrap();
        assert!(list_comments(&conn, CommentParent::Issue(issue_id), None, Some("newest")).is_err());
    }

    #[test]
    fn page_and_issue_comment_threads_are_independent() {
        let (pool, issue_id, page_id, user_id) = setup();
        let conn = pool.write().unwrap();

        create_comment(&conn, CommentParent::Issue(issue_id), user_id, "Issue thread").unwrap();
        create_comment(&conn, CommentParent::Page(page_id), user_id, "Page thread").unwrap();

        let issue_comments = list_comments(&conn, CommentParent::Issue(issue_id), None, None).unwrap();
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

        let created = create_comment(&conn, CommentParent::Issue(issue_id), user_id, "Hello").unwrap();
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

        let created = create_comment(&conn, CommentParent::Issue(issue_id), user_id, "Original").unwrap();
        let updated = update_comment(&conn, created.id, "Edited").unwrap();
        assert_eq!(updated.content, "Edited");
        assert_eq!(updated.id, created.id);
    }

    #[test]
    fn delete_comment_removes_it() {
        let (pool, issue_id, _, user_id) = setup();
        let conn = pool.write().unwrap();

        let created = create_comment(&conn, CommentParent::Issue(issue_id), user_id, "Delete me").unwrap();
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

        let c = create_comment(&conn, CommentParent::Issue(issue_id), user_id, "Will be cascaded").unwrap();
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
        assert!(result.is_err(), "expected CHECK constraint to reject dual-parent row");
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
        assert!(result.is_err(), "expected CHECK constraint to reject parentless row");
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

        let c = create_comment(&conn, CommentParent::Issue(issue_id), user_id, "line1\\nline2").unwrap();
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
        assert_eq!(
            extract_mention_usernames("@ada @Ada @ADA"),
            vec!["ada"]
        );
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
        let c = create_comment(&conn, CommentParent::Issue(issue_id), user_id, "note to @blake").unwrap();
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
        assert!(feed.items.iter().any(|a| a.action == "mention" && a.new_value.as_deref() == Some("ada")));
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
        assert!(!names.contains(&"botty"), "bots are never mention candidates");
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
                description: String::new(),
                emoji: None,
                lead_user_id: None,
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
        assert!(!ids.contains(&outsider.id), "non-member must not be a candidate");

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

        create_comment(&conn, CommentParent::Issue(issue_id), user1_id, "Blake says hi").unwrap();
        create_comment(&conn, CommentParent::Issue(issue_id), user2.id, "Ada responds").unwrap();

        let comments = list_comments(&conn, CommentParent::Issue(issue_id), None, None).unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].author, "blake");
        assert_eq!(comments[1].author, "ada");
        assert_eq!(comments[1].author_display_name, "Ada");
    }
}
