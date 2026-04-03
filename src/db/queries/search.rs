use rusqlite::Connection;

use crate::db::models::*;
use crate::error::LificError;

pub fn search(conn: &Connection, q: &SearchQuery) -> Result<Vec<SearchResult>, LificError> {
    let limit = q.limit.unwrap_or(20);

    let fts_query: String = q
        .query
        .split_whitespace()
        .map(|word| {
            let escaped = word.replace('"', "\"\"");
            format!("\"{escaped}\"*")
        })
        .collect::<Vec<_>>()
        .join(" ");

    let base_sql = "SELECT s.entity_type, s.entity_id, s.title,
                CASE WHEN s.body = '' OR s.body IS NULL
                     THEN snippet(search_index, 0, '**', '**', '...', 32)
                     ELSE snippet(search_index, 1, '**', '**', '...', 32)
                END,
                s.project_id,
                p.identifier, i.sequence, pg.sequence
         FROM search_index s
         LEFT JOIN issues i ON s.entity_type = 'issue' AND i.id = s.entity_id
         LEFT JOIN pages pg ON s.entity_type = 'page' AND pg.id = s.entity_id
         LEFT JOIN projects p ON p.id = s.project_id";

    let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(pid) =
        q.project_id
    {
        (
            format!(
                "{base_sql} WHERE search_index MATCH ?1 AND s.project_id = ?2 ORDER BY rank LIMIT ?3"
            ),
            vec![Box::new(fts_query.clone()), Box::new(pid), Box::new(limit)],
        )
    } else {
        (
            format!("{base_sql} WHERE search_index MATCH ?1 ORDER BY rank LIMIT ?2"),
            vec![Box::new(fts_query.clone()), Box::new(limit)],
        )
    };

    let params_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_refs.as_slice(), |row| {
        let entity_type: String = row.get(0)?;
        let project_ident: Option<String> = row.get(5)?;
        let issue_seq: Option<i64> = row.get(6)?;
        let page_seq: Option<i64> = row.get(7)?;
        let identifier = match entity_type.as_str() {
            "issue" => match (project_ident.as_deref(), issue_seq) {
                (Some(pi), Some(seq)) => Some(format!("{pi}-{seq}")),
                _ => None,
            },
            "page" => match (project_ident.as_deref(), page_seq) {
                (Some(pi), Some(seq)) => Some(format!("{pi}-DOC-{seq}")),
                (None, Some(seq)) => Some(format!("DOC-{seq}")),
                _ => None,
            },
            _ => None,
        };
        Ok(SearchResult {
            result_type: entity_type,
            id: row.get(1)?,
            identifier,
            title: row.get(2)?,
            snippet: row.get(3)?,
            project_id: row.get(4)?,
        })
    })?;

    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::queries::{issues, pages, projects};

    fn test_db() -> db::DbPool {
        db::open_memory().expect("test db")
    }

    fn seed_project(conn: &rusqlite::Connection, ident: &str) -> i64 {
        projects::create_project(
            conn,
            &CreateProject {
                name: format!("Project {ident}"),
                identifier: ident.into(),
                description: String::new(),
                emoji: None,
            },
        )
        .unwrap()
        .id
    }

    #[test]
    fn search_finds_issue_by_title() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        issues::create_issue(
            &conn,
            &CreateIssue {
                project_id: pid,
                title: "Implement authentication flow".into(),
                description: String::new(),
                status: "backlog".into(),
                priority: "none".into(),
                module_id: None,
                start_date: None,
                target_date: None,
                labels: vec![],
            },
        )
        .unwrap();

        let results = search(
            &conn,
            &SearchQuery {
                query: "authentication".into(),
                project_id: None,
                limit: None,
            },
        )
        .unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].result_type, "issue");
        assert_eq!(results[0].identifier, Some("TST-1".into()));
    }

    #[test]
    fn search_finds_page_by_content() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        pages::create_page(
            &conn,
            &CreatePage {
                project_id: Some(pid),
                folder_id: None,
                title: "Design Doc".into(),
                content: "This covers the WebSocket protocol design".into(),
            },
        )
        .unwrap();

        let results = search(
            &conn,
            &SearchQuery {
                query: "websocket".into(),
                project_id: None,
                limit: None,
            },
        )
        .unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].result_type, "page");
        assert_eq!(results[0].identifier, Some("TST-DOC-1".into()));
    }

    #[test]
    fn search_prefix_matching() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        issues::create_issue(
            &conn,
            &CreateIssue {
                project_id: pid,
                title: "Implement authentication system".into(),
                description: String::new(),
                status: "backlog".into(),
                priority: "none".into(),
                module_id: None,
                start_date: None,
                target_date: None,
                labels: vec![],
            },
        )
        .unwrap();

        // "auth" should match "authentication" via prefix wildcard
        let results = search(
            &conn,
            &SearchQuery {
                query: "auth".into(),
                project_id: None,
                limit: None,
            },
        )
        .unwrap();
        assert!(!results.is_empty());
    }

    #[test]
    fn search_respects_project_filter() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let p1 = seed_project(&conn, "AAA");
        let p2 = seed_project(&conn, "BBB");
        issues::create_issue(
            &conn,
            &CreateIssue {
                project_id: p1,
                title: "Alpha feature".into(),
                description: String::new(),
                status: "backlog".into(),
                priority: "none".into(),
                module_id: None,
                start_date: None,
                target_date: None,
                labels: vec![],
            },
        )
        .unwrap();
        issues::create_issue(
            &conn,
            &CreateIssue {
                project_id: p2,
                title: "Beta feature".into(),
                description: String::new(),
                status: "backlog".into(),
                priority: "none".into(),
                module_id: None,
                start_date: None,
                target_date: None,
                labels: vec![],
            },
        )
        .unwrap();

        let results = search(
            &conn,
            &SearchQuery {
                query: "feature".into(),
                project_id: Some(p1),
                limit: None,
            },
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].identifier, Some("AAA-1".into()));
    }

    #[test]
    fn search_empty_description_uses_title_snippet() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        issues::create_issue(
            &conn,
            &CreateIssue {
                project_id: pid,
                title: "Fix the rendering pipeline".into(),
                description: String::new(), // empty body
                status: "backlog".into(),
                priority: "none".into(),
                module_id: None,
                start_date: None,
                target_date: None,
                labels: vec![],
            },
        )
        .unwrap();

        let results = search(
            &conn,
            &SearchQuery {
                query: "rendering".into(),
                project_id: None,
                limit: None,
            },
        )
        .unwrap();
        assert!(!results.is_empty());
        // Snippet should contain something (falls back to title)
        assert!(!results[0].snippet.is_empty());
    }
}
