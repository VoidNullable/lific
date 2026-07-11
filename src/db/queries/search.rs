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
        && rt != "comment"
    {
        return Err(LificError::BadRequest(format!(
            "invalid result_type '{rt}'. Use issue, page, or comment."
        )));
    }

    // LIF-304: dispatch on match mode. `fts` (default) tokenizes the query
    // through FTS5; `literal` does a case-insensitive substring scan for
    // punctuation-heavy needles (e.g. `core:sodom`, `[RequiredSpecs]`) that
    // FTS's tokenizer strips away.
    match q.mode.as_deref() {
        None | Some("fts") => search_fts(conn, q, limit, offset),
        Some("literal") => search_literal(conn, q, limit, offset),
        Some(other) => Err(LificError::BadRequest(format!(
            "invalid mode '{other}'. Use fts or literal."
        ))),
    }
}

/// FTS5 full-text path (the original `search` body).
fn search_fts(
    conn: &Connection,
    q: &SearchQuery,
    limit: i64,
    offset: i64,
) -> Result<Vec<SearchResult>, LificError> {
    // "relevance" = BM25 rank (FTS5 default). "recent" = most recently
    // updated entity first; both joins are LEFT so COALESCE picks whichever
    // side matched. Fixed fragments only — never interpolated user input.
    let order_clause = match q.sort.as_deref() {
        None | Some("relevance") => "ORDER BY rank",
        Some("recent") => {
            "ORDER BY COALESCE(i.updated_at, pg.updated_at, ci.updated_at, cpg.updated_at) DESC, rank"
        }
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

    // Comment hits (LIF-146) carry no title of their own; they link back to a
    // parent issue or page. `c` is the comment row; `ci`/`cpg` are its parent
    // issue/page, and `cip`/`cpp` those parents' projects — so a comment match
    // renders as "on <parent identifier>" and navigates to the parent.
    let base_sql = "SELECT s.entity_type, s.entity_id, s.title,
                CASE WHEN s.body = '' OR s.body IS NULL
                     THEN snippet(search_index, 0, '**', '**', '...', 32)
                     ELSE snippet(search_index, 1, '**', '**', '...', 32)
                END,
                s.project_id,
                p.identifier, i.sequence, pg.sequence,
                c.issue_id, c.page_id,
                cip.identifier, ci.sequence,
                cpp.identifier, cpg.sequence
         FROM search_index s
         LEFT JOIN issues i ON s.entity_type = 'issue' AND i.id = s.entity_id
         LEFT JOIN pages pg ON s.entity_type = 'page' AND pg.id = s.entity_id
         LEFT JOIN projects p ON p.id = s.project_id
         LEFT JOIN comments c ON s.entity_type = 'comment' AND c.id = s.entity_id
         LEFT JOIN issues ci ON c.issue_id = ci.id
         LEFT JOIN pages cpg ON c.page_id = cpg.id
         LEFT JOIN projects cip ON cip.id = ci.project_id
         LEFT JOIN projects cpp ON cpp.id = cpg.project_id";

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
        // Comment parent linkage (LIF-146): a comment resolves to its parent's
        // identifier so the hit navigates to the issue/page it lives on.
        let cmt_issue_id: Option<i64> = row.get(8)?;
        let cmt_page_id: Option<i64> = row.get(9)?;
        let cmt_issue_proj: Option<String> = row.get(10)?;
        let cmt_issue_seq: Option<i64> = row.get(11)?;
        let cmt_page_proj: Option<String> = row.get(12)?;
        let cmt_page_seq: Option<i64> = row.get(13)?;
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
            "comment" => {
                if cmt_issue_id.is_some() {
                    match (cmt_issue_proj.as_deref(), cmt_issue_seq) {
                        (Some(pi), Some(seq)) => Some(format!("{pi}-{seq}")),
                        _ => None,
                    }
                } else if cmt_page_id.is_some() {
                    match (cmt_page_proj.as_deref(), cmt_page_seq) {
                        (Some(pi), Some(seq)) => Some(format!("{pi}-DOC-{seq}")),
                        (None, Some(seq)) => Some(format!("DOC-{seq}")),
                        _ => None,
                    }
                } else {
                    None
                }
            }
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

/// Case-insensitive substring path (LIF-304).
///
/// Scans the same corpus as the FTS path — issues (title + description),
/// pages (title + content), comments (content) — using
/// `instr(lower(field), lower(?)) > 0`. This avoids LIKE-wildcard injection
/// (a needle containing `%` / `_` is matched literally) at the cost of
/// ASCII-only case folding: SQLite's `lower()` only folds A–Z, so non-ASCII
/// letters compare case-sensitively. That's an acceptable limitation for the
/// punctuation-heavy identifiers this mode targets (`core:sodom`,
/// `[RequiredSpecs]`, `--trace-plans`).
///
/// Ordering is always most-recently-updated first: a substring scan has no
/// relevance rank, so `sort=relevance` and `sort=recent` both order by
/// recency (relevance is accepted without error so callers can pass their
/// usual sort through). Snippets are built in Rust around the first match.
fn search_literal(
    conn: &Connection,
    q: &SearchQuery,
    limit: i64,
    offset: i64,
) -> Result<Vec<SearchResult>, LificError> {
    // Accept the same sort values the FTS path does, but both map to recency
    // here (see doc comment) — reject only genuinely unknown values so the
    // contract stays identical.
    match q.sort.as_deref() {
        None | Some("relevance") | Some("recent") => {}
        Some(other) => {
            return Err(LificError::BadRequest(format!(
                "invalid sort '{other}'. Use relevance or recent."
            )));
        }
    }

    let needle = q.query.trim();
    // LIF-133 parity: an empty / whitespace-only needle returns nothing
    // rather than matching every row (instr(x, '') is always > 0).
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let needle = needle.to_string();

    let want = |rt: &str| q.result_type.as_deref().is_none_or(|f| f == rt);

    // Collect (updated_at, SearchResult) so we can globally sort by recency
    // across the three entity kinds before applying offset/limit. Each branch
    // mirrors the FTS path's identifier + parent-linkage logic.
    let mut rows: Vec<(String, SearchResult)> = Vec::new();

    if want("issue") {
        let mut stmt = conn.prepare(
            "SELECT i.id, p.identifier, i.sequence, i.title, i.description,
                    i.project_id, i.updated_at
             FROM issues i
             JOIN projects p ON p.id = i.project_id
             WHERE instr(lower(i.title), lower(?1)) > 0
                OR instr(lower(i.description), lower(?1)) > 0",
        )?;
        let mapped = stmt.query_map([&needle], |row| {
            let id: i64 = row.get(0)?;
            let proj: String = row.get(1)?;
            let seq: i64 = row.get(2)?;
            let title: String = row.get(3)?;
            let body: String = row.get(4)?;
            let project_id: Option<i64> = row.get(5)?;
            let updated_at: String = row.get(6)?;
            let snippet = literal_snippet(&title, &body, &needle);
            Ok((
                updated_at,
                SearchResult {
                    result_type: "issue".into(),
                    id,
                    identifier: Some(format!("{proj}-{seq}")),
                    title,
                    snippet,
                    project_id,
                },
            ))
        })?;
        for r in mapped {
            rows.push(r?);
        }
    }

    if want("page") {
        let mut stmt = conn.prepare(
            "SELECT pg.id, p.identifier, pg.sequence, pg.title, pg.content,
                    pg.project_id, pg.updated_at
             FROM pages pg
             LEFT JOIN projects p ON p.id = pg.project_id
             WHERE instr(lower(pg.title), lower(?1)) > 0
                OR instr(lower(pg.content), lower(?1)) > 0",
        )?;
        let mapped = stmt.query_map([&needle], |row| {
            let id: i64 = row.get(0)?;
            let proj: Option<String> = row.get(1)?;
            let seq: i64 = row.get(2)?;
            let title: String = row.get(3)?;
            let body: String = row.get(4)?;
            let project_id: Option<i64> = row.get(5)?;
            let updated_at: String = row.get(6)?;
            let identifier = match proj.as_deref() {
                Some(pi) => Some(format!("{pi}-DOC-{seq}")),
                None => Some(format!("DOC-{seq}")),
            };
            let snippet = literal_snippet(&title, &body, &needle);
            Ok((
                updated_at,
                SearchResult {
                    result_type: "page".into(),
                    id,
                    identifier,
                    title,
                    snippet,
                    project_id,
                },
            ))
        })?;
        for r in mapped {
            rows.push(r?);
        }
    }

    if want("comment") {
        // Mirror the FTS path's parent-linkage joins so a comment hit resolves
        // to its parent issue/page identifier and inherits the parent's
        // project_id for visibility filtering.
        let mut stmt = conn.prepare(
            "SELECT c.id, c.content, c.updated_at,
                    c.issue_id, c.page_id,
                    cip.identifier, ci.sequence, ci.project_id,
                    cpp.identifier, cpg.sequence, cpg.project_id
             FROM comments c
             LEFT JOIN issues ci ON c.issue_id = ci.id
             LEFT JOIN pages cpg ON c.page_id = cpg.id
             LEFT JOIN projects cip ON cip.id = ci.project_id
             LEFT JOIN projects cpp ON cpp.id = cpg.project_id
             WHERE instr(lower(c.content), lower(?1)) > 0",
        )?;
        let mapped = stmt.query_map([&needle], |row| {
            let id: i64 = row.get(0)?;
            let content: String = row.get(1)?;
            let updated_at: String = row.get(2)?;
            let cmt_issue_id: Option<i64> = row.get(3)?;
            let cmt_page_id: Option<i64> = row.get(4)?;
            let cmt_issue_proj: Option<String> = row.get(5)?;
            let cmt_issue_seq: Option<i64> = row.get(6)?;
            let cmt_issue_project_id: Option<i64> = row.get(7)?;
            let cmt_page_proj: Option<String> = row.get(8)?;
            let cmt_page_seq: Option<i64> = row.get(9)?;
            let cmt_page_project_id: Option<i64> = row.get(10)?;
            let (identifier, project_id) = if cmt_issue_id.is_some() {
                let ident = match (cmt_issue_proj.as_deref(), cmt_issue_seq) {
                    (Some(pi), Some(seq)) => Some(format!("{pi}-{seq}")),
                    _ => None,
                };
                (ident, cmt_issue_project_id)
            } else if cmt_page_id.is_some() {
                let ident = match (cmt_page_proj.as_deref(), cmt_page_seq) {
                    (Some(pi), Some(seq)) => Some(format!("{pi}-DOC-{seq}")),
                    (None, Some(seq)) => Some(format!("DOC-{seq}")),
                    _ => None,
                };
                (ident, cmt_page_project_id)
            } else {
                (None, None)
            };
            // A comment has no title of its own, so the snippet always comes
            // from the body.
            let snippet = literal_snippet("", &content, &needle);
            Ok((
                updated_at,
                SearchResult {
                    result_type: "comment".into(),
                    id,
                    identifier,
                    title: String::new(),
                    snippet,
                    project_id,
                },
            ))
        })?;
        for r in mapped {
            rows.push(r?);
        }
    }

    // Project filter: applied uniformly across all three entity kinds after
    // collection (a comment's project_id is its parent's, resolved above). A
    // workspace page has project_id = None and is only kept when the caller
    // didn't scope to a project.
    if let Some(pid) = q.project_id {
        rows.retain(|(_, r)| r.project_id == Some(pid));
    }

    // Global recency sort (updated_at DESC), then id DESC as a stable
    // tiebreak, before paging.
    rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.id.cmp(&a.1.id)));

    Ok(rows
        .into_iter()
        .map(|(_, r)| r)
        .skip(offset as usize)
        .take(limit as usize)
        .collect())
}

/// Build a snippet around the first case-insensitive match of `needle`.
///
/// Prefers the body; if the match is only in the title, snippets from the
/// title (mirrors the FTS path's title-vs-body CASE). Takes ~32 chars of
/// context on each side, wraps the matched substring in `**`, and adds
/// leading/trailing `...` when the window is clipped. All slicing respects
/// UTF-8 char boundaries.
fn literal_snippet(title: &str, body: &str, needle: &str) -> String {
    const CTX: usize = 32;
    // Prefer the body match; fall back to the title.
    let (source, start) = match find_ci(body, needle) {
        Some(i) => (body, i),
        None => match find_ci(title, needle) {
            Some(i) => (title, i),
            // Neither field contains it (shouldn't happen — the SQL filtered
            // on a match — but stay robust): return a clipped body preview.
            None => return clip_prefix(body.max(title), CTX * 2),
        },
    };
    let match_end = start + needle.len();

    // Expand the window to CTX chars on each side, snapping to char
    // boundaries.
    let win_start = floor_char_boundary(source, start.saturating_sub(CTX));
    let win_end = ceil_char_boundary(source, (match_end + CTX).min(source.len()));

    let mut out = String::new();
    if win_start > 0 {
        out.push_str("...");
    }
    out.push_str(&source[win_start..start]);
    out.push_str("**");
    out.push_str(&source[start..match_end]);
    out.push_str("**");
    out.push_str(&source[match_end..win_end]);
    if win_end < source.len() {
        out.push_str("...");
    }
    out
}

/// Byte offset of the first case-insensitive (ASCII-fold) occurrence of
/// `needle` in `haystack`, or None. Matches SQLite's `instr(lower(), lower())`
/// semantics (ASCII-only folding), so query and render agree.
fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    let hay = haystack.to_ascii_lowercase();
    let nee = needle.to_ascii_lowercase();
    hay.find(&nee)
}

/// Largest char boundary <= `idx`.
fn floor_char_boundary(s: &str, mut idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// Smallest char boundary >= `idx`.
fn ceil_char_boundary(s: &str, mut idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

/// Clip a string to at most `max` bytes on a char boundary, adding a trailing
/// `...` if clipped. Fallback preview only.
fn clip_prefix(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let end = floor_char_boundary(s, max);
    format!("{}...", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::queries::comments::{self, CommentParent};
    use crate::db::queries::{issues, pages, projects};
    use rusqlite::params;

    fn test_db() -> db::DbPool {
        db::open_memory().expect("test db")
    }

    fn seed_user(conn: &rusqlite::Connection, username: &str) -> i64 {
        conn.execute(
            "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot)
             VALUES (?1, ?2, 'x', ?1, 0, 0)",
            params![username, format!("{username}@test.local")],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn seed_issue(conn: &rusqlite::Connection, pid: i64, title: &str) -> i64 {
        issues::create_issue(
            conn,
            &CreateIssue {
                project_id: pid,
                title: title.into(),
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
        .unwrap()
        .id
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
                result_type: Some("widget".into()),
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

    // ── Comment indexing (LIF-146) ────────────────────────────

    #[test]
    fn search_finds_issue_comment_and_links_to_parent() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let iid = seed_issue(&conn, pid, "Some issue");
        let uid = seed_user(&conn, "alice");
        comments::create_comment(
            &conn,
            CommentParent::Issue(iid),
            uid,
            "we decided to use the flux capacitor approach",
        )
        .unwrap();

        let results = search(
            &conn,
            &SearchQuery {
                query: "flux".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].result_type, "comment");
        // A comment hit links back to its parent issue's identifier.
        assert_eq!(results[0].identifier, Some("TST-1".into()));
        assert!(results[0].snippet.contains("flux"));
    }

    #[test]
    fn search_finds_page_comment_and_links_to_parent() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let page = pages::create_page(
            &conn,
            &CreatePage {
                project_id: Some(pid),
                folder_id: None,
                title: "Design Doc".into(),
                content: String::new(),
                status: "draft".into(),
                labels: vec![],
            },
        )
        .unwrap();
        let uid = seed_user(&conn, "bob");
        comments::create_comment(
            &conn,
            CommentParent::Page(page.id),
            uid,
            "the quokka migration plan lives here",
        )
        .unwrap();

        let results = search(
            &conn,
            &SearchQuery {
                query: "quokka".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].result_type, "comment");
        // A page comment links back to its parent page's DOC identifier.
        assert_eq!(results[0].identifier, Some("TST-DOC-1".into()));
    }

    #[test]
    fn search_reflects_comment_edit() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let iid = seed_issue(&conn, pid, "Some issue");
        let uid = seed_user(&conn, "alice");
        let comment = comments::create_comment(
            &conn,
            CommentParent::Issue(iid),
            uid,
            "original zorblatt wording",
        )
        .unwrap();

        // Original term is findable.
        assert_eq!(
            search(
                &conn,
                &SearchQuery {
                    query: "zorblatt".into(),
                    ..Default::default()
                },
            )
            .unwrap()
            .len(),
            1
        );

        comments::update_comment(&conn, comment.id, "revised gribblenaut wording").unwrap();

        // Old term is gone from the index...
        assert!(
            search(
                &conn,
                &SearchQuery {
                    query: "zorblatt".into(),
                    ..Default::default()
                },
            )
            .unwrap()
            .is_empty(),
            "edited-away term must no longer match"
        );
        // ...and the new term is now searchable, still linked to the parent.
        let after = search(
            &conn,
            &SearchQuery {
                query: "gribblenaut".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].result_type, "comment");
        assert_eq!(after[0].identifier, Some("TST-1".into()));
    }

    #[test]
    fn search_drops_deleted_comment_from_index() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let iid = seed_issue(&conn, pid, "Some issue");
        let uid = seed_user(&conn, "alice");
        let comment = comments::create_comment(
            &conn,
            CommentParent::Issue(iid),
            uid,
            "ephemeral snorfblat note",
        )
        .unwrap();

        comments::delete_comment(&conn, comment.id).unwrap();

        let results = search(
            &conn,
            &SearchQuery {
                query: "snorfblat".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(results.is_empty(), "deleted comment must leave the index");
    }

    #[test]
    fn search_filters_by_comment_result_type() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        // An issue and a comment that both match "overlap".
        let iid = seed_issue(&conn, pid, "overlap in the issue title");
        let uid = seed_user(&conn, "alice");
        comments::create_comment(&conn, CommentParent::Issue(iid), uid, "overlap in the comment")
            .unwrap();

        let comments_only = search(
            &conn,
            &SearchQuery {
                query: "overlap".into(),
                result_type: Some("comment".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(comments_only.len(), 1);
        assert_eq!(comments_only[0].result_type, "comment");
    }

    #[test]
    fn search_backfills_preexisting_comments() {
        // Comments written before the trigger fires (simulated by inserting a
        // comment then rebuilding the index the way migration 034's backfill
        // does) must become searchable. We approximate a "pre-existing" row by
        // clearing the FTS entry the trigger created, then running the same
        // INSERT...SELECT the migration uses.
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let iid = seed_issue(&conn, pid, "Some issue");
        let uid = seed_user(&conn, "alice");
        let comment =
            comments::create_comment(&conn, CommentParent::Issue(iid), uid, "backfillme term")
                .unwrap();
        // Remove the trigger-created FTS row to simulate an un-indexed comment.
        conn.execute(
            "DELETE FROM search_index WHERE entity_type = 'comment' AND entity_id = ?1",
            params![comment.id],
        )
        .unwrap();
        assert!(
            search(
                &conn,
                &SearchQuery {
                    query: "backfillme".into(),
                    ..Default::default()
                },
            )
            .unwrap()
            .is_empty(),
            "precondition: comment is not yet indexed"
        );

        // Re-run the migration's backfill statement.
        conn.execute_batch(
            "INSERT INTO search_index(title, body, entity_type, entity_id, project_id)
             SELECT '', c.content, 'comment', c.id,
                    COALESCE(i.project_id, pg.project_id)
             FROM comments c
             LEFT JOIN issues i ON c.issue_id = i.id
             LEFT JOIN pages  pg ON c.page_id  = pg.id
             WHERE NOT EXISTS (
                 SELECT 1 FROM search_index s
                 WHERE s.entity_type = 'comment' AND s.entity_id = c.id
             );",
        )
        .unwrap();

        let results = search(
            &conn,
            &SearchQuery {
                query: "backfillme".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].result_type, "comment");
        assert_eq!(results[0].identifier, Some("TST-1".into()));
    }

    // ── literal mode (LIF-304) ────────────────────────────────

    fn lit(query: &str) -> SearchQuery {
        SearchQuery {
            query: query.into(),
            mode: Some("literal".into()),
            ..Default::default()
        }
    }

    #[test]
    fn literal_finds_punctuation_needle_that_fts_misses() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        issues::create_issue(
            &conn,
            &CreateIssue {
                project_id: pid,
                title: "wire up core:sodom pipeline".into(),
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

        // FTS tokenizes "core:sodom" into separate words and the `:` is
        // dropped, so a literal search for the exact token is the point.
        let fts = search(
            &conn,
            &SearchQuery {
                query: "core:sodom".into(),
                ..Default::default()
            },
        )
        .unwrap();
        // FTS may match on "core" or "sodom" tokens; literal matches the exact
        // punctuation-joined needle.
        let lits = search(&conn, &lit("core:sodom")).unwrap();
        assert_eq!(lits.len(), 1, "literal must find the exact needle");
        assert_eq!(lits[0].identifier, Some("TST-1".into()));
        assert!(lits[0].snippet.contains("**core:sodom**"), "got: {}", lits[0].snippet);
        // Sanity: the presence/absence of the FTS hit isn't what we assert;
        // literal is the reliable path here.
        let _ = fts;
    }

    #[test]
    fn literal_matches_bracketed_needle() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        pages::create_page(
            &conn,
            &CreatePage {
                project_id: Some(pid),
                folder_id: None,
                title: "Spec".into(),
                content: "see [RequiredSpecs] for the contract".into(),
                status: "draft".into(),
                labels: vec![],
            },
        )
        .unwrap();

        let lits = search(&conn, &lit("[RequiredSpecs]")).unwrap();
        assert_eq!(lits.len(), 1);
        assert_eq!(lits[0].result_type, "page");
        assert!(lits[0].snippet.contains("**[RequiredSpecs]**"), "got: {}", lits[0].snippet);
    }

    #[test]
    fn literal_is_case_insensitive() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        seed_issue(&conn, pid, "Handle the FooBar case");

        let lits = search(&conn, &lit("foobar")).unwrap();
        assert_eq!(lits.len(), 1);
        assert_eq!(lits[0].identifier, Some("TST-1".into()));
    }

    #[test]
    fn literal_treats_like_wildcards_as_literal() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        seed_issue(&conn, pid, "progress is 50% done");
        seed_issue(&conn, pid, "unrelated 50 percent");

        // `%` must match a literal percent sign, not "any characters".
        let lits = search(&conn, &lit("50%")).unwrap();
        assert_eq!(lits.len(), 1, "%/_ must be literal, not wildcards");
        assert_eq!(lits[0].identifier, Some("TST-1".into()));

        // `_` is literal too.
        seed_issue(&conn, pid, "call trace_plans here");
        let underscore = search(&conn, &lit("trace_plans")).unwrap();
        assert_eq!(underscore.len(), 1);
    }

    #[test]
    fn literal_respects_project_filter() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let p1 = seed_project(&conn, "AAA");
        let p2 = seed_project(&conn, "BBB");
        seed_issue(&conn, p1, "core:sodom in alpha");
        seed_issue(&conn, p2, "core:sodom in beta");

        let mut q = lit("core:sodom");
        q.project_id = Some(p1);
        let lits = search(&conn, &q).unwrap();
        assert_eq!(lits.len(), 1);
        assert_eq!(lits[0].identifier, Some("AAA-1".into()));
    }

    #[test]
    fn literal_respects_result_type_filter() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        seed_issue(&conn, pid, "widget:alpha issue");
        pages::create_page(
            &conn,
            &CreatePage {
                project_id: Some(pid),
                folder_id: None,
                title: "widget:alpha page".into(),
                content: String::new(),
                status: "draft".into(),
                labels: vec![],
            },
        )
        .unwrap();

        let mut q = lit("widget:alpha");
        q.result_type = Some("page".into());
        let lits = search(&conn, &q).unwrap();
        assert_eq!(lits.len(), 1);
        assert_eq!(lits[0].result_type, "page");
    }

    #[test]
    fn literal_comment_resolves_parent_identifier() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let iid = seed_issue(&conn, pid, "Some issue");
        let uid = seed_user(&conn, "alice");
        comments::create_comment(
            &conn,
            CommentParent::Issue(iid),
            uid,
            "the --trace-plans flag is the fix",
        )
        .unwrap();

        let lits = search(&conn, &lit("--trace-plans")).unwrap();
        assert_eq!(lits.len(), 1);
        assert_eq!(lits[0].result_type, "comment");
        assert_eq!(lits[0].identifier, Some("TST-1".into()));
        assert!(lits[0].snippet.contains("**--trace-plans**"), "got: {}", lits[0].snippet);
    }

    #[test]
    fn literal_invalid_mode_errors() {
        let pool = test_db();
        let conn = pool.read().unwrap();
        let err = search(
            &conn,
            &SearchQuery {
                query: "x".into(),
                mode: Some("regex".into()),
                ..Default::default()
            },
        );
        assert!(err.is_err(), "unknown mode must error");
        assert!(err.unwrap_err().to_string().contains("invalid mode"));
    }

    #[test]
    fn literal_empty_query_returns_no_results() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        seed_issue(&conn, pid, "findable core:sodom");

        for query in ["", "   ", "\t\n"] {
            let lits = search(
                &conn,
                &SearchQuery {
                    query: query.into(),
                    mode: Some("literal".into()),
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(lits.is_empty(), "empty needle must match nothing: {query:?}");
        }
    }

    #[test]
    fn literal_orders_by_recency() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        seed_issue(&conn, pid, "core:sodom older"); // TST-1
        seed_issue(&conn, pid, "core:sodom newer"); // TST-2
        conn.execute_batch(
            "DROP TRIGGER IF EXISTS issues_updated;
             UPDATE issues SET updated_at = '2026-01-01 00:00:00' WHERE sequence = 1;
             UPDATE issues SET updated_at = '2026-06-01 00:00:00' WHERE sequence = 2;",
        )
        .unwrap();

        let lits = search(&conn, &lit("core:sodom")).unwrap();
        assert_eq!(lits.len(), 2);
        assert_eq!(lits[0].identifier, Some("TST-2".into()), "newest first");
        assert_eq!(lits[1].identifier, Some("TST-1".into()));
    }
}
