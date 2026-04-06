use rusqlite::{params, Connection};

use crate::db::models::Comment;
use crate::error::LificError;

use super::unescape_text;

/// Create a comment on an issue.
pub fn create_comment(
    conn: &Connection,
    issue_id: i64,
    user_id: i64,
    content: &str,
) -> Result<Comment, LificError> {
    let content = unescape_text(content);

    // Verify issue exists
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM issues WHERE id = ?1",
            params![issue_id],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if !exists {
        return Err(LificError::NotFound(format!("issue {issue_id} not found")));
    }

    conn.execute(
        "INSERT INTO comments (issue_id, user_id, content) VALUES (?1, ?2, ?3)",
        params![issue_id, user_id, content],
    )?;

    let id = conn.last_insert_rowid();
    get_comment(conn, id)
}

/// Get a single comment by ID (with author info).
pub fn get_comment(conn: &Connection, id: i64) -> Result<Comment, LificError> {
    conn.query_row(
        "SELECT c.id, c.issue_id, c.user_id, u.username, u.display_name,
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

/// List all comments for an issue, ordered chronologically.
pub fn list_comments(conn: &Connection, issue_id: i64) -> Result<Vec<Comment>, LificError> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.issue_id, c.user_id, u.username, u.display_name,
                c.content, c.created_at, c.updated_at
         FROM comments c
         JOIN users u ON u.id = c.user_id
         WHERE c.issue_id = ?1
         ORDER BY c.created_at ASC",
    )?;
    let rows = stmt.query_map(params![issue_id], row_to_comment)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// Update a comment's content.
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

/// Delete a comment.
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
        user_id: row.get(2)?,
        author: row.get(3)?,
        author_display_name: row.get(4)?,
        content: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::models::*;
    use crate::db::queries;

    fn setup() -> (db::DbPool, i64, i64) {
        let pool = db::open_memory().expect("test db");
        let conn = pool.write().unwrap();

        // Create a user
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

        // Create a project
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

        // Create an issue
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

        drop(conn);
        (pool, issue.id, user.id)
    }

    #[test]
    fn create_and_list_comments() {
        let (pool, issue_id, user_id) = setup();
        let conn = pool.write().unwrap();

        let c1 = create_comment(&conn, issue_id, user_id, "First comment").unwrap();
        assert_eq!(c1.content, "First comment");
        assert_eq!(c1.author, "blake");
        assert_eq!(c1.author_display_name, "Blake");
        assert_eq!(c1.issue_id, issue_id);
        assert_eq!(c1.user_id, user_id);

        let c2 = create_comment(&conn, issue_id, user_id, "Second comment").unwrap();
        assert_eq!(c2.content, "Second comment");

        let comments = list_comments(&conn, issue_id).unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].content, "First comment");
        assert_eq!(comments[1].content, "Second comment");
    }

    #[test]
    fn get_comment_by_id() {
        let (pool, issue_id, user_id) = setup();
        let conn = pool.write().unwrap();

        let created = create_comment(&conn, issue_id, user_id, "Hello").unwrap();
        let fetched = get_comment(&conn, created.id).unwrap();
        assert_eq!(fetched.content, "Hello");
        assert_eq!(fetched.author, "blake");
    }

    #[test]
    fn update_comment_content() {
        let (pool, issue_id, user_id) = setup();
        let conn = pool.write().unwrap();

        let created = create_comment(&conn, issue_id, user_id, "Original").unwrap();
        let updated = update_comment(&conn, created.id, "Edited").unwrap();
        assert_eq!(updated.content, "Edited");
        assert_eq!(updated.id, created.id);
    }

    #[test]
    fn delete_comment_removes_it() {
        let (pool, issue_id, user_id) = setup();
        let conn = pool.write().unwrap();

        let created = create_comment(&conn, issue_id, user_id, "Delete me").unwrap();
        delete_comment(&conn, created.id).unwrap();

        let result = get_comment(&conn, created.id);
        assert!(result.is_err());
    }

    #[test]
    fn comment_on_nonexistent_issue_fails() {
        let (pool, _, user_id) = setup();
        let conn = pool.write().unwrap();

        let result = create_comment(&conn, 99999, user_id, "Orphan");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn delete_nonexistent_comment_fails() {
        let (pool, _, _) = setup();
        let conn = pool.write().unwrap();

        let result = delete_comment(&conn, 99999);
        assert!(result.is_err());
    }

    #[test]
    fn comments_cascade_on_issue_delete() {
        let (pool, issue_id, user_id) = setup();
        let conn = pool.write().unwrap();

        let c = create_comment(&conn, issue_id, user_id, "Will be cascaded").unwrap();

        // Delete the issue
        queries::delete_issue(&conn, issue_id).unwrap();

        // Comment should be gone
        let result = get_comment(&conn, c.id);
        assert!(result.is_err());
    }

    #[test]
    fn comment_unescapes_newlines() {
        let (pool, issue_id, user_id) = setup();
        let conn = pool.write().unwrap();

        let c = create_comment(&conn, issue_id, user_id, "line1\\nline2").unwrap();
        assert_eq!(c.content, "line1\nline2");
    }

    #[test]
    fn list_comments_empty_issue() {
        let (pool, issue_id, _) = setup();
        let conn = pool.read().unwrap();

        let comments = list_comments(&conn, issue_id).unwrap();
        assert!(comments.is_empty());
    }

    #[test]
    fn multiple_users_comment() {
        let (pool, issue_id, user1_id) = setup();
        let conn = pool.write().unwrap();

        // Create a second user
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

        create_comment(&conn, issue_id, user1_id, "Blake says hi").unwrap();
        create_comment(&conn, issue_id, user2.id, "Ada responds").unwrap();

        let comments = list_comments(&conn, issue_id).unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].author, "blake");
        assert_eq!(comments[1].author, "ada");
        assert_eq!(comments[1].author_display_name, "Ada");
    }
}
