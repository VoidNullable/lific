use rusqlite::Connection;

use crate::db::models::*;
use crate::error::LificError;

pub fn search(conn: &Connection, q: &SearchQuery) -> Result<Vec<SearchResult>, LificError> {
    // Clamp limit to a sane range (LIF-141 class): SQLite treats LIMIT -1 as
    // "no limit", so an unclamped `?limit=-1` would return the whole FTS set.
    let limit = q.limit.unwrap_or(20).clamp(1, 500);
    let offset = q.offset.unwrap_or(0).max(0);

    // Validate enum-ish params up front so a typo'd filter errors instead
    // of silently returning everything.
    if let Some(ref rt) = q.result_type
        && rt != "issue"
        && rt != "page"
    {
        return Err(LificError::BadRequest(format!(
            "invalid result_type '{rt}'. Use issue or page."
        )));
    }
    // "relevance" = BM25 rank (FTS5 default). "recent" = most recently
    // updated entity first; both joins are LEFT so COALESCE picks whichever
    // side matched. Fixed fragments only — never interpolated user input.
    let order_clause = match q.sort.as_deref() {
        None | Some("relevance") => "ORDER BY rank",
        Some("recent") => "ORDER BY COALESCE(i.updated_at, pg.updated_at) DESC, rank",
        Some(other) => {
            return Err(LificError::BadRequest(format!(
                "invalid sort '{other}'. Use relevance or recent."
            )));
        }
    };

    let fts_query: String = q
        .query
        .split_whitespace()
        .map(|word| {
            let escaped = word.replace('"', "\"\"");
            format!("\"{escaped}\"*")
        })
        .collect::<Vec<_>>()
        .join(" ");

    // LIF-133: an empty or whitespace-only query tokenizes to an empty FTS
    // expression, and `MATCH ''` is an fts5 syntax error. Return no results
    // instead of surfacing a database error.
    if fts_query.is_empty() {
        return Ok(Vec::new());
    }

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

    let mut conditions = vec!["search_index MATCH ?1".to_string()];
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(fts_query.clone())];
    if let Some(pid) = q.project_id {
        conditions.push(format!("s.project_id = ?{}", params.len() + 1));
        params.push(Box::new(pid));
    }
    if let Some(ref rt) = q.result_type {
        conditions.push(format!("s.entity_type = ?{}", params.len() + 1));
        params.push(Box::new(rt.clone()));
    }
    let sql = format!(
        "{base_sql} WHERE {} {order_clause} LIMIT ?{} OFFSET ?{}",
        conditions.join(" AND "),
        params.len() + 1,
        params.len() + 2,
    );
    params.push(Box::new(limit));
    params.push(Box::new(offset));

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
                lead_user_id: None,
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
                source: None,
            },
        )
        .unwrap();

        let results = search(
            &conn,
            &SearchQuery {
                query: "authentication".into(),
                project_id: None,
                limit: None,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].result_type, "issue");
        assert_eq!(results[0].identifier, Some("TST-1".into()));
    }

    // LIF-141 class: `?limit=-1` must not become SQLite's "no limit" and
    // return the entire FTS result set. The floor clamps to 1.
    #[test]
    fn search_clamps_negative_limit() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        for i in 0..3 {
            issues::create_issue(
                &conn,
                &CreateIssue {
                    project_id: pid,
                    title: format!("authentication case {i}"),
                    description: String::new(),
                    status: "backlog".into(),
                    priority: "none".into(),
                    module_id: None,
                    start_date: None,
                    target_date: None,
                    labels: vec![],
                    source: None,
                },
            )
            .unwrap();
        }
        let results = search(
            &conn,
            &SearchQuery {
                query: "authentication".into(),
                limit: Some(-1),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(results.len(), 1, "limit=-1 must clamp to 1, not return every match");
    }

    // LIF-133: empty and whitespace-only queries previously built `MATCH ''`,
    // an fts5 syntax error that surfaced as a database error. They must
    // return an empty result set instead.
    #[test]
    fn search_empty_query_returns_no_results() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        issues::create_issue(
            &conn,
            &CreateIssue {
                project_id: pid,
                title: "Findable issue".into(),
                description: String::new(),
                status: "backlog".into(),
                priority: "none".into(),
                module_id: None,
                start_date: None,
                target_date: None,
                labels: vec![],
                source: None,
            },
        )
        .unwrap();

        for query in ["", "   ", "\t\n"] {
            let results = search(
                &conn,
                &SearchQuery {
                    query: query.into(),
                    project_id: None,
                    limit: None,
                    ..Default::default()
                },
            )
            .unwrap_or_else(|e| panic!("query {query:?} must not error: {e}"));
            assert!(results.is_empty(), "query {query:?} must return nothing");
        }
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
                status: "draft".into(),
                labels: vec![],
            },
        )
        .unwrap();

        let results = search(
            &conn,
            &SearchQuery {
                query: "websocket".into(),
                project_id: None,
                limit: None,
                ..Default::default()
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
                source: None,
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
                ..Default::default()
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
                source: None,
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
                source: None,
            },
        )
        .unwrap();

        let results = search(
            &conn,
            &SearchQuery {
                query: "feature".into(),
                project_id: Some(p1),
                limit: None,
                ..Default::default()
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
                source: None,
            },
        )
        .unwrap();

        let results = search(
            &conn,
            &SearchQuery {
                query: "rendering".into(),
                project_id: None,
                limit: None,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!results.is_empty());
        // Snippet should contain something (falls back to title)
        assert!(!results[0].snippet.is_empty());
    }

    // ── result_type filter, sort, offset ──────────────────────

    /// Seed one issue and one page that both match the word "shared".
    fn seed_mixed_results(conn: &rusqlite::Connection, pid: i64) {
        issues::create_issue(
            conn,
            &CreateIssue {
                project_id: pid,
                title: "shared concern in the API".into(),
                description: String::new(),
                status: "backlog".into(),
                priority: "none".into(),
                module_id: None,
                start_date: None,
                target_date: None,
                labels: vec![],
                source: None,
            },
        )
        .unwrap();
        pages::create_page(
            conn,
            &CreatePage {
                project_id: Some(pid),
                folder_id: None,
                title: "shared design notes".into(),
                content: String::new(),
                status: "draft".into(),
                labels: vec![],
            },
        )
        .unwrap();
    }

    #[test]
    fn search_filters_by_result_type() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        seed_mixed_results(&conn, pid);

        let issues_only = search(
            &conn,
            &SearchQuery {
                query: "shared".into(),
                result_type: Some("issue".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(issues_only.len(), 1);
        assert_eq!(issues_only[0].result_type, "issue");

        let pages_only = search(
            &conn,
            &SearchQuery {
                query: "shared".into(),
                result_type: Some("page".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(pages_only.len(), 1);
        assert_eq!(pages_only[0].result_type, "page");
    }

    #[test]
    fn search_rejects_invalid_enum_params() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        seed_project(&conn, "TST");

        let bad_type = search(
            &conn,
            &SearchQuery {
                query: "anything".into(),
                result_type: Some("comment".into()),
                ..Default::default()
            },
        );
        assert!(bad_type.is_err(), "unknown result_type must error");

        let bad_sort = search(
            &conn,
            &SearchQuery {
                query: "anything".into(),
                sort: Some("oldest".into()),
                ..Default::default()
            },
        );
        assert!(bad_sort.is_err(), "unknown sort must error");
    }

    #[test]
    fn search_offset_pages_through_results() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        seed_mixed_results(&conn, pid); // two matches for "shared"

        let first = search(
            &conn,
            &SearchQuery {
                query: "shared".into(),
                limit: Some(1),
                offset: Some(0),
                ..Default::default()
            },
        )
        .unwrap();
        let second = search(
            &conn,
            &SearchQuery {
                query: "shared".into(),
                limit: Some(1),
                offset: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 1);
        assert_ne!(
            (first[0].result_type.clone(), first[0].id),
            (second[0].result_type.clone(), second[0].id),
            "offset must advance past the first result"
        );
    }

    #[test]
    fn search_recent_sort_orders_by_updated() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        seed_mixed_results(&conn, pid);
        // Pin the page fresher than the issue, regardless of insert order.
        // The *_updated triggers rewrite updated_at to now on UPDATE, which
        // would clobber the pins — drop them first.
        conn.execute_batch(
            "DROP TRIGGER IF EXISTS issues_updated;
             DROP TRIGGER IF EXISTS pages_updated;
             UPDATE issues SET updated_at = '2026-01-01 00:00:00';
             UPDATE pages SET updated_at = '2026-06-01 00:00:00';",
        )
        .unwrap();

        let results = search(
            &conn,
            &SearchQuery {
                query: "shared".into(),
                sort: Some("recent".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].result_type, "page", "fresher entity must rank first");
        assert_eq!(results[1].result_type, "issue");
    }
}
