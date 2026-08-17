use rusqlite::Connection;

use crate::db::models::*;
use crate::error::LificError;

/// Default hits per search page. Smaller than the shared page default on
/// purpose. Public so a transport that has to publish the same default in its
/// own paging hints reads it from here rather than restating `20`.
pub const DEFAULT_SEARCH_LIMIT: i64 = 20;

/// Search, discarding the `has_more` signal.
pub fn search(conn: &Connection, q: &SearchQuery) -> Result<Vec<SearchResult>, LificError> {
    Ok(search_page(conn, q)?.items)
}

/// [`search`] as a [`Page`](super::Page). The over-fetch happens under this
/// query's clamp, so `has_more` stays correct at the cap (LIF-388).
pub fn search_page(
    conn: &Connection,
    q: &SearchQuery,
) -> Result<super::Page<SearchResult>, LificError> {
    // Search publishes a smaller default page than the shared 50: an FTS hit
    // list is a preview, not a data dump. The cap stays the shared one.
    let (limit, offset) = super::page_with(
        q.limit,
        q.offset,
        DEFAULT_SEARCH_LIMIT,
        super::MAX_PAGE_LIMIT,
    );
    let fetch = super::over_fetch(limit);

    // Validate enum-ish params up front so a typo'd filter errors instead
    // of silently returning everything.
    if let Some(ref rt) = q.result_type
        && rt != "issue"
        && rt != "page"
        && rt != "comment"
        && rt != "attachment"
    {
        return Err(LificError::BadRequest(format!(
            "invalid result_type '{rt}'. Use issue, page, comment, or attachment."
        )));
    }

    // LIF-304: dispatch on match mode. `fts` (default) tokenizes the query
    // through FTS5; `literal` does a case-insensitive substring scan for
    // punctuation-heavy needles (e.g. `core:sodom`, `[RequiredSpecs]`) that
    // FTS's tokenizer strips away.
    let hits = match q.mode.as_deref() {
        None | Some("fts") => search_fts(conn, q, fetch, offset),
        Some("literal") => search_literal(conn, q, fetch, offset),
        Some(other) => Err(LificError::BadRequest(format!(
            "invalid mode '{other}'. Use fts or literal."
        ))),
    }?;
    Ok(super::Page::from_over_fetch(hits, limit))
}

/// FTS5 full-text path.
///
/// LIF-418: two indexes feed this now — `search_index` (issues, pages,
/// comments) and `attachments_fts` (filenames + extracted text of small text
/// uploads). BM25 scores are not comparable across two separate FTS tables, so
/// rather than pretending to interleave them by relevance, attachment hits are
/// appended after the entity hits and the combined list is paged in Rust. When
/// `result_type` selects one side only, that side is paged in SQL exactly as
/// before.
fn search_fts(
    conn: &Connection,
    q: &SearchQuery,
    limit: i64,
    offset: i64,
) -> Result<Vec<SearchResult>, LificError> {
    let want_entities = q.result_type.as_deref().is_none_or(|rt| rt != "attachment");
    let want_attachments = q.result_type.as_deref().is_none_or(|rt| rt == "attachment");

    match (want_entities, want_attachments) {
        (true, false) => search_entities_fts(conn, q, limit, offset),
        (false, true) => search_attachments_fts(conn, q, limit, offset),
        _ => {
            // Both sides: take everything up to the end of the requested page
            // from each index, concatenate, then apply the offset here so
            // `has_more` still means what the caller thinks it means.
            let window = limit.saturating_add(offset);
            let mut rows = search_entities_fts(conn, q, window, 0)?;
            rows.extend(search_attachments_fts(conn, q, window, 0)?);
            Ok(rows
                .into_iter()
                .skip(offset.max(0) as usize)
                .take(limit.max(0) as usize)
                .collect())
        }
    }
}

/// Attachment hits: a match on the filename or on the extracted text of a text
/// upload, resolved to the entity that references the file so the caller can
/// jump to where it is used.
///
/// Unlinked attachments are excluded on purpose. They belong to no project
/// yet, so there is nothing for the caller's `visible_project_ids` filter to
/// authorize them against, and a freshly uploaded blob is nobody's search
/// result.
fn search_attachments_fts(
    conn: &Connection,
    q: &SearchQuery,
    limit: i64,
    offset: i64,
) -> Result<Vec<SearchResult>, LificError> {
    let order_clause = match q.sort.as_deref() {
        None | Some("relevance") => "ORDER BY rank",
        Some("recent") => "ORDER BY a.created_at DESC, rank",
        Some(other) => {
            return Err(LificError::BadRequest(format!(
                "invalid sort '{other}'. Use relevance or recent."
            )));
        }
    };
    let Some(fts_query) = fts_expression(&q.query) else {
        return Ok(Vec::new());
    };

    // The project filter has to run in SQL, before LIMIT, or a page could come
    // back empty while matches exist further down.
    let project_clause = match q.project_id {
        Some(_) => {
            "AND EXISTS (
                 SELECT 1
                 FROM attachment_links l
                 LEFT JOIN issues   i   ON l.entity_type = 'issue'   AND i.id   = l.entity_id
                 LEFT JOIN pages    pg  ON l.entity_type = 'page'    AND pg.id  = l.entity_id
                 LEFT JOIN comments c   ON l.entity_type = 'comment' AND c.id   = l.entity_id
                 LEFT JOIN issues   ci  ON ci.id  = c.issue_id
                 LEFT JOIN pages    cpg ON cpg.id = c.page_id
                 WHERE l.attachment_id = a.id
                   AND ?4 IN (i.project_id, pg.project_id, ci.project_id, cpg.project_id)
             )"
        }
        None => "AND EXISTS (SELECT 1 FROM attachment_links l WHERE l.attachment_id = a.id AND ?4 IS NULL)",
    };

    let sql = format!(
        "SELECT attachments_fts.attachment_id, a.filename,
                CASE WHEN attachments_fts.extracted_text = ''
                     THEN snippet(attachments_fts, 0, '**', '**', '...', 32)
                     ELSE snippet(attachments_fts, 1, '**', '**', '...', 32)
                END
         FROM attachments_fts
         JOIN attachments a ON a.id = attachments_fts.attachment_id
         WHERE attachments_fts MATCH ?1
         {project_clause}
         {order_clause}
         LIMIT ?2 OFFSET ?3"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params![fts_query, limit, offset, q.project_id],
        |row| {
            let id: i64 = row.get(0)?;
            let filename: String = row.get(1)?;
            let snippet: String = row.get(2)?;
            Ok((id, filename, snippet))
        },
    )?;

    let mut results = Vec::new();
    for row in rows {
        let (id, filename, snippet) = row?;
        if let Some(result) = attachment_result(conn, id, filename, snippet, q.project_id)? {
            results.push(result);
        }
    }
    Ok(results)
}

/// Assemble one attachment [`SearchResult`], resolving the entity it should
/// link to. Returns `None` when the attachment lost its last link between the
/// index read and this lookup.
fn attachment_result(
    conn: &Connection,
    id: i64,
    filename: String,
    snippet: String,
    project_id: Option<i64>,
) -> Result<Option<SearchResult>, LificError> {
    let Some(target) = super::attachments::primary_link(conn, id, project_id)? else {
        return Ok(None);
    };
    Ok(Some(SearchResult {
        result_type: "attachment".into(),
        id,
        identifier: target.identifier,
        title: filename,
        snippet,
        project_id: target.project_id,
        parent_page_id: target.page_id,
    }))
}

/// Turn a user query into the prefix-matching FTS5 expression both indexes are
/// searched with. `None` for an empty or whitespace-only query: `MATCH ''` is
/// an fts5 syntax error, so the caller returns no results instead (LIF-133).
fn fts_expression(query: &str) -> Option<String> {
    let expression: String = query
        .split_whitespace()
        .map(|word| {
            let escaped = word.replace('"', "\"\"");
            format!("\"{escaped}\"*")
        })
        .collect::<Vec<_>>()
        .join(" ");
    (!expression.is_empty()).then_some(expression)
}

/// Issue / page / comment hits from `search_index` (the original `search_fts`
/// body).
fn search_entities_fts(
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

    // LIF-133: an empty or whitespace-only query tokenizes to an empty FTS
    // expression, and `MATCH ''` is an fts5 syntax error. Return no results
    // instead of surfacing a database error.
    let Some(fts_query) = fts_expression(&q.query) else {
        return Ok(Vec::new());
    };

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
            parent_page_id: cmt_page_id,
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
                    parent_page_id: None,
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
                    parent_page_id: None,
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
                    parent_page_id: cmt_page_id,
                },
            ))
        })?;
        for r in mapped {
            rows.push(r?);
        }
    }

    // LIF-418: attachments join the literal scan too, so a punctuation-heavy
    // needle (`core:sodom`, a stack frame, a config key) finds the log file it
    // appears in, not just the issue that discusses it. Filenames are always
    // scanned; contents only exist for indexed text uploads.
    if want("attachment") {
        let mut stmt = conn.prepare(
            "SELECT a.id, a.filename, a.created_at, COALESCE(f.extracted_text, '')
             FROM attachments a
             LEFT JOIN attachments_fts f ON f.attachment_id = a.id
             WHERE (instr(lower(a.filename), lower(?1)) > 0
                    OR instr(lower(COALESCE(f.extracted_text, '')), lower(?1)) > 0)
               AND EXISTS (SELECT 1 FROM attachment_links l WHERE l.attachment_id = a.id)",
        )?;
        let mapped = stmt.query_map([&needle], |row| {
            let id: i64 = row.get(0)?;
            let filename: String = row.get(1)?;
            let created_at: String = row.get(2)?;
            let text: String = row.get(3)?;
            Ok((id, filename, created_at, text))
        })?;
        for row in mapped {
            let (id, filename, created_at, text) = row?;
            let snippet = literal_snippet(&filename, &text, &needle);
            if let Some(result) =
                attachment_result(conn, id, filename, snippet, q.project_id)?
            {
                rows.push((created_at, result));
            }
        }
    }

    // Project filter: applied uniformly across all entity kinds after
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                    ..Default::default()
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
                ..Default::default()
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
                title: "Design Doc".into(),
                content: "This covers the WebSocket protocol design".into(),
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
            },
        )
        .unwrap();
        issues::create_issue(
            &conn,
            &CreateIssue {
                project_id: p2,
                title: "Beta feature".into(),
                ..Default::default()
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
                description: String::new(), // empty body: the subject of this test
                ..Default::default()
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
                ..Default::default()
            },
        )
        .unwrap();
        pages::create_page(
            conn,
            &CreatePage {
                project_id: Some(pid),
                title: "shared design notes".into(),
                ..Default::default()
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
                title: "Design Doc".into(),
                ..Default::default()
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
        assert_eq!(results[0].parent_page_id, Some(page.id));
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

    // ── Attachment hits (LIF-418) ─────────────────────────────

    /// Attach a file to `issue_id`, optionally with extracted text in the FTS
    /// index (as the upload path does for small `text/*` uploads).
    fn seed_attachment(
        conn: &rusqlite::Connection,
        issue_id: Option<i64>,
        filename: &str,
        mime: &str,
        text: Option<&str>,
    ) -> i64 {
        use crate::db::queries::attachments as att;
        let attachment =
            att::create_attachment(conn, filename, filename, mime, 42, None).unwrap();
        if let Some(issue_id) = issue_id {
            att::link_attachment(conn, attachment.id, AttachmentEntity::Issue, issue_id).unwrap();
        }
        if let Some(text) = text {
            att::set_extracted_text(conn, attachment.id, text).unwrap();
        }
        attachment.id
    }

    #[test]
    fn search_finds_attachment_by_filename_and_links_to_its_entity() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let iid = seed_issue(&conn, pid, "Crash report");
        let attachment = seed_attachment(&conn, Some(iid), "heapdump.log", "text/plain", None);

        let results = search(
            &conn,
            &SearchQuery {
                query: "heapdump".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let hit = results
            .iter()
            .find(|r| r.result_type == "attachment")
            .expect("the file must be findable by name");
        assert_eq!(hit.id, attachment);
        assert_eq!(hit.title, "heapdump.log");
        assert_eq!(
            hit.identifier.as_deref(),
            Some("TST-1"),
            "a file hit carries the entity it is attached to"
        );
        assert_eq!(hit.project_id, Some(pid));
    }

    #[test]
    fn search_finds_attachment_by_extracted_text() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let iid = seed_issue(&conn, pid, "Nothing to see in the title");
        seed_attachment(
            &conn,
            Some(iid),
            "server.log",
            "text/plain",
            Some("panicked at gribblenaut::render line 12"),
        );

        let results = search(
            &conn,
            &SearchQuery {
                query: "gribblenaut".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].result_type, "attachment");
        assert!(
            results[0].snippet.contains("gribblenaut"),
            "the snippet comes from the file's contents, got: {}",
            results[0].snippet
        );
    }

    #[test]
    fn search_excludes_unlinked_attachments() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        seed_project(&conn, "TST");
        // Uploaded but never referenced: it belongs to no project, so there is
        // nothing to authorize it against and it must not surface.
        seed_attachment(&conn, None, "snorfblat.log", "text/plain", None);

        let results = search(
            &conn,
            &SearchQuery {
                query: "snorfblat".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(results.is_empty(), "got: {results:?}");
    }

    #[test]
    fn search_respects_project_filter_for_attachments() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let p1 = seed_project(&conn, "AAA");
        let p2 = seed_project(&conn, "BBB");
        let i1 = seed_issue(&conn, p1, "alpha");
        let i2 = seed_issue(&conn, p2, "beta");
        seed_attachment(&conn, Some(i1), "shared-report.log", "text/plain", None);
        seed_attachment(&conn, Some(i2), "shared-report.log", "text/plain", None);

        let results = search(
            &conn,
            &SearchQuery {
                query: "shared-report".into(),
                project_id: Some(p1),
                result_type: Some("attachment".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].identifier.as_deref(), Some("AAA-1"));
    }

    #[test]
    fn search_filters_by_attachment_result_type() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        // An issue and a file that both match "overlap".
        let iid = seed_issue(&conn, pid, "overlap in the issue title");
        seed_attachment(&conn, Some(iid), "overlap.log", "text/plain", None);

        let files_only = search(
            &conn,
            &SearchQuery {
                query: "overlap".into(),
                result_type: Some("attachment".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(files_only.len(), 1);
        assert_eq!(files_only[0].result_type, "attachment");

        let issues_only = search(
            &conn,
            &SearchQuery {
                query: "overlap".into(),
                result_type: Some("issue".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(issues_only.len(), 1);
        assert_eq!(issues_only[0].result_type, "issue");

        // Unfiltered sees both.
        let both = search(
            &conn,
            &SearchQuery {
                query: "overlap".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(both.len(), 2);
    }

    #[test]
    fn search_drops_attachment_from_the_index_on_delete() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let iid = seed_issue(&conn, pid, "parent");
        let attachment = seed_attachment(&conn, Some(iid), "zorblatt.log", "text/plain", None);

        crate::db::queries::attachments::delete_attachment(&conn, attachment).unwrap();

        let results = search(
            &conn,
            &SearchQuery {
                query: "zorblatt".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            results.is_empty(),
            "a deleted attachment must leave the index"
        );
    }

    #[test]
    fn attachment_hits_page_alongside_entity_hits() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let iid = seed_issue(&conn, pid, "quokka in the title");
        seed_attachment(&conn, Some(iid), "quokka.log", "text/plain", None);

        let first = search(
            &conn,
            &SearchQuery {
                query: "quokka".into(),
                limit: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
        let second = search(
            &conn,
            &SearchQuery {
                query: "quokka".into(),
                limit: Some(1),
                offset: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 1);
        assert_eq!(first[0].result_type, "issue", "entity hits come first");
        assert_eq!(second[0].result_type, "attachment");
    }

    #[test]
    fn literal_mode_finds_attachments_too() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let iid = seed_issue(&conn, pid, "parent issue");
        seed_attachment(
            &conn,
            Some(iid),
            "trace.log",
            "text/plain",
            Some("thread panicked at core:sodom::run"),
        );

        let hits = search(&conn, &lit("core:sodom")).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].result_type, "attachment");
        assert_eq!(hits[0].title, "trace.log");
        assert!(
            hits[0].snippet.contains("**core:sodom**"),
            "got: {}",
            hits[0].snippet
        );
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
                ..Default::default()
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
                title: "Spec".into(),
                content: "see [RequiredSpecs] for the contract".into(),
                ..Default::default()
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
                title: "widget:alpha page".into(),
                ..Default::default()
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
