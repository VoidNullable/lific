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

/// List comments for an issue or page, ordered chronologically.
pub fn list_comments(
    conn: &Connection,
    parent: CommentParent,
) -> Result<Vec<Comment>, LificError> {
    let (sql, id) = match parent {
        CommentParent::Issue(id) => (
            "SELECT c.id, c.issue_id, c.page_id, c.user_id, u.username, u.display_name,
                    c.content, c.created_at, c.updated_at
             FROM comments c
             JOIN users u ON u.id = c.user_id
             WHERE c.issue_id = ?1
             ORDER BY c.created_at ASC",
            id,
        ),
        CommentParent::Page(id) => (
            "SELECT c.id, c.issue_id, c.page_id, c.user_id, u.username, u.display_name,
                    c.content, c.created_at, c.updated_at
             FROM comments c
             JOIN users u ON u.id = c.user_id
             WHERE c.page_id = ?1
             ORDER BY c.created_at ASC",
            id,
        ),
    };
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![id], row_to_comment)?;
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

        let comments = list_comments(&conn, CommentParent::Issue(issue_id)).unwrap();
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

        let comments = list_comments(&conn, CommentParent::Page(page_id)).unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].content, "Hello page");
        assert_eq!(comments[1].content, "Another");
    }

    #[test]
    fn page_and_issue_comment_threads_are_independent() {
        let (pool, issue_id, page_id, user_id) = setup();
        let conn = pool.write().unwrap();

        create_comment(&conn, CommentParent::Issue(issue_id), user_id, "Issue thread").unwrap();
        create_comment(&conn, CommentParent::Page(page_id), user_id, "Page thread").unwrap();

        let issue_comments = list_comments(&conn, CommentParent::Issue(issue_id)).unwrap();
        let page_comments = list_comments(&conn, CommentParent::Page(page_id)).unwrap();

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

        let comments = list_comments(&conn, CommentParent::Issue(issue_id)).unwrap();
        assert!(comments.is_empty());
    }

    #[test]
    fn list_comments_empty_page() {
        let (pool, _, page_id, _) = setup();
        let conn = pool.read().unwrap();

        let comments = list_comments(&conn, CommentParent::Page(page_id)).unwrap();
        assert!(comments.is_empty());
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

        let comments = list_comments(&conn, CommentParent::Issue(issue_id)).unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].author, "blake");
        assert_eq!(comments[1].author, "ada");
        assert_eq!(comments[1].author_display_name, "Ada");
    }
}
