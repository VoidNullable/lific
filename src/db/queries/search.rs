use rusqlite::{Connection, ErrorCode, types::Value};
use std::collections::HashSet;

use crate::db::models::{SearchQuery, SearchResult};
use crate::error::LificError;

/// Default hits per search page. Smaller than the shared page default on
/// purpose. Public so a transport that has to publish the same default in its
/// own paging hints reads from here rather than restating 20.
pub const DEFAULT_SEARCH_LIMIT: i64 = 20;
pub const MAX_LITERAL_QUERY_BYTES: usize = 256;
pub const MAX_LITERAL_MATCHES: usize = 10_000;
pub const MAX_FTS_QUERY_BYTES: usize = 4 * 1024;
pub const MAX_SEARCH_OFFSET: i64 = 100_000;
const LITERAL_PROGRESS_INTERVAL: i32 = 10_000;
const MAX_LITERAL_PROGRESS_CALLBACKS: usize = 100;

/// Search, discarding the has-more signal.
pub fn search(conn: &Connection, q: &SearchQuery) -> Result<Vec<SearchResult>, LificError> {
    Ok(search_page(conn, q, None)?.items)
}

/// Search one page. Project visibility is applied in SQL before ranking,
/// sorting, and pagination; None means the caller is unrestricted.
pub fn search_page(
    conn: &Connection,
    q: &SearchQuery,
    visible_project_ids: Option<&HashSet<i64>>,
) -> Result<super::Page<SearchResult>, LificError> {
    let (limit, raw_offset) = super::page_with(
        q.limit,
        q.offset,
        DEFAULT_SEARCH_LIMIT,
        super::MAX_PAGE_LIMIT,
    );
    let offset = raw_offset.min(MAX_SEARCH_OFFSET);
    let fetch = super::over_fetch(limit);

    let max_query_bytes = if q.mode.as_deref() == Some("literal") {
        MAX_LITERAL_QUERY_BYTES
    } else {
        MAX_FTS_QUERY_BYTES
    };
    if q.query.len() > max_query_bytes {
        return Err(LificError::BadRequest(format!(
            "search query exceeds {max_query_bytes} bytes"
        )));
    }
    // Validate enum-ish params up front so a typo'd filter errors instead
    // of silently returning everything.
    if let Some(result_type) = q.result_type.as_deref()
        && !matches!(result_type, "issue" | "page" | "comment" | "attachment")
    {
        return Err(LificError::BadRequest(format!(
            "invalid result_type '{result_type}'. Use issue, page, comment, or attachment."
        )));
    }

    let hits = match q.mode.as_deref() {
        None | Some("fts") => search_fts(conn, q, fetch, offset, visible_project_ids),
        Some("literal") => search_literal(conn, q, fetch, offset, visible_project_ids),
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
/// appended after the entity hits. When `result_type` selects one side only,
/// that side is paged in SQL exactly as before.
fn search_fts(
    conn: &Connection,
    q: &SearchQuery,
    limit: i64,
    offset: i64,
    visible_project_ids: Option<&HashSet<i64>>,
) -> Result<Vec<SearchResult>, LificError> {
    let want_entities = q.result_type.as_deref().is_none_or(|rt| rt != "attachment");
    let want_attachments = q.result_type.as_deref().is_none_or(|rt| rt == "attachment");

    match (want_entities, want_attachments) {
        (true, false) => search_entities_fts(conn, q, limit, offset, visible_project_ids),
        (false, true) => search_attachments_fts(conn, q, limit, offset, visible_project_ids),
        _ => {
            // Both sides, one page. The concatenation is entities-then-
            // attachments, so the requested window is cut out of the entity
            // list first and only the shortfall is read from the attachment
            // index. Neither index is over-fetched by the offset (LIF-388 read
            // `offset + limit` rows from *each* side, which grows without
            // bound as a caller pages).
            let mut rows = search_entities_fts(conn, q, limit, offset, visible_project_ids)?;
            let remainder = limit - rows.len() as i64;
            if remainder <= 0 {
                return Ok(rows);
            }
            // A full entity page means no attachments were reached; a short
            // one means the entity list ended inside this page, so the
            // attachment side starts at its first row. Only when the page
            // starts past the last entity hit does the offset have to be
            // translated, and only then is the count worth its query.
            let attachment_offset = if rows.is_empty() && offset > 0 {
                offset
                    .saturating_sub(count_entities_fts(conn, q, visible_project_ids)?)
                    .max(0)
            } else {
                0
            };
            rows.extend(search_attachments_fts(
                conn,
                q,
                remainder,
                attachment_offset,
                visible_project_ids,
            )?);
            Ok(rows)
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
    visible_project_ids: Option<&HashSet<i64>>,
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

    // Project scope and caller visibility both run in SQL, before LIMIT, or a
    // page could come back short (or empty) while matches exist further down.
    // An attachment qualifies only if some link resolves to a project the
    // caller may see — and, when scoped, to that project.
    let (link_scope, scope_params) =
        super::attachments::visible_link_scope_sql("a.id", q.project_id, visible_project_ids);

    let sql = format!(
        "SELECT attachments_fts.attachment_id, a.filename,
                CASE WHEN attachments_fts.extracted_text = ''
                     THEN snippet(attachments_fts, 0, '**', '**', '...', 32)
                     ELSE snippet(attachments_fts, 1, '**', '**', '...', 32)
                END
         FROM attachments_fts
         JOIN attachments a ON a.id = attachments_fts.attachment_id
         WHERE attachments_fts MATCH ?
           AND {link_scope}
         {order_clause}
         LIMIT ? OFFSET ?"
    );

    let mut params = vec![Value::Text(fts_query)];
    params.extend(scope_params);
    params.extend([Value::Integer(limit), Value::Integer(offset)]);

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(params), |row| {
        let id: i64 = row.get(0)?;
        let filename: String = row.get(1)?;
        let snippet: String = row.get(2)?;
        Ok((id, filename, snippet))
    })?;

    let mut results = Vec::new();
    for row in rows {
        let (id, filename, snippet) = row?;
        if let Some(result) = attachment_result(
            conn,
            id,
            filename,
            snippet,
            q.project_id,
            visible_project_ids,
        )? {
            results.push(result);
        }
    }
    Ok(results)
}

/// Assemble one attachment [`SearchResult`], resolving the entity it should
/// link to. Returns `None` when the attachment lost its last in-scope link
/// between the index read and this lookup.
///
/// The link is resolved under the same scope the row was selected with, so the
/// rendered target is always one the caller may see. Picking a merely
/// *preferred* link would let a file shared into a hidden project render that
/// project's identifier.
fn attachment_result(
    conn: &Connection,
    id: i64,
    filename: String,
    snippet: String,
    project_id: Option<i64>,
    visible_project_ids: Option<&HashSet<i64>>,
) -> Result<Option<SearchResult>, LificError> {
    let Some(target) =
        super::attachments::visible_primary_link(conn, id, project_id, visible_project_ids)?
    else {
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
    visible_project_ids: Option<&HashSet<i64>>,
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
    let Some((conditions, mut params)) = entity_fts_filter(q, visible_project_ids) else {
        return Ok(Vec::new());
    };

    let base_sql = "SELECT s.entity_type, s.entity_id,
                CASE s.entity_type
                    WHEN 'issue' THEN p.identifier || '-' || i.sequence
                    WHEN 'page' THEN
                        CASE WHEN p.identifier IS NULL
                            THEN 'DOC-' || pg.sequence
                            ELSE p.identifier || '-DOC-' || pg.sequence
                        END
                    WHEN 'comment' THEN
                        CASE WHEN c.issue_id IS NOT NULL
                            THEN cip.identifier || '-' || ci.sequence
                            WHEN cpp.identifier IS NULL
                            THEN 'DOC-' || cpg.sequence
                            ELSE cpp.identifier || '-DOC-' || cpg.sequence
                        END
                END,
                s.title,
                CASE WHEN s.body = '' OR s.body IS NULL
                     THEN snippet(search_index, 0, '**', '**', '...', 32)
                     ELSE snippet(search_index, 1, '**', '**', '...', 32)
                END,
                s.project_id, c.page_id
         FROM search_index s
         LEFT JOIN issues i ON s.entity_type = 'issue' AND i.id = s.entity_id
         LEFT JOIN pages pg ON s.entity_type = 'page' AND pg.id = s.entity_id
         LEFT JOIN projects p ON p.id = s.project_id
         LEFT JOIN comments c ON s.entity_type = 'comment' AND c.id = s.entity_id
         LEFT JOIN issues ci ON c.issue_id = ci.id
         LEFT JOIN pages cpg ON c.page_id = cpg.id
         LEFT JOIN projects cip ON cip.id = ci.project_id
         LEFT JOIN projects cpp ON cpp.id = cpg.project_id";

    let sql = format!(
        "{base_sql} WHERE {} {order_clause} LIMIT ? OFFSET ?",
        conditions.join(" AND "),
    );
    params.extend([Value::Integer(limit), Value::Integer(offset)]);

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(params), row_to_search_result)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// The `WHERE` fragments and their bound values shared by the entity hit page
/// and [`count_entities_fts`]. `None` when the query tokenizes to nothing
/// (LIF-133), which matches no row rather than every row.
///
/// Every fragment constrains `s` alone, so the count can skip the join chain.
fn entity_fts_filter(
    q: &SearchQuery,
    visible_project_ids: Option<&HashSet<i64>>,
) -> Option<(Vec<String>, Vec<Value>)> {
    let fts_query = fts_expression(&q.query)?;
    let mut conditions = vec!["search_index MATCH ?".to_string()];
    let mut params = vec![Value::Text(fts_query)];
    if let Some(project_id) = q.project_id {
        conditions.push("s.project_id = ?".into());
        params.push(Value::Integer(project_id));
    }
    // `attachment` never reaches here: the caller routes it to the attachment
    // index, and `search_page` has already rejected anything else.
    if let Some(result_type) = q.result_type.as_deref() {
        conditions.push("s.entity_type = ?".into());
        params.push(Value::Text(result_type.into()));
    }
    let (visibility, visible_ids) =
        super::project_visibility_sql("s.project_id", visible_project_ids);
    conditions.push(visibility);
    params.extend(visible_ids.into_iter().map(Value::Integer));
    Some((conditions, params))
}

/// How many entity hits the query has in total, so a page that begins past
/// them can be translated into an offset on the attachment index.
fn count_entities_fts(
    conn: &Connection,
    q: &SearchQuery,
    visible_project_ids: Option<&HashSet<i64>>,
) -> Result<i64, LificError> {
    let Some((conditions, params)) = entity_fts_filter(q, visible_project_ids) else {
        return Ok(0);
    };
    let sql = format!(
        "SELECT COUNT(*) FROM search_index s WHERE {}",
        conditions.join(" AND "),
    );
    let count = conn
        .prepare(&sql)?
        .query_row(rusqlite::params_from_iter(params), |row| {
            row.get::<_, i64>(0)
        })?;
    Ok(count)
}

fn row_to_search_result(row: &rusqlite::Row<'_>) -> rusqlite::Result<SearchResult> {
    Ok(SearchResult {
        result_type: row.get(0)?,
        id: row.get(1)?,
        identifier: row.get(2)?,
        title: row.get(3)?,
        snippet: row.get(4)?,
        project_id: row.get(5)?,
        parent_page_id: row.get(6)?,
    })
}

/// Case-insensitive substring path (LIF-304).
///
/// Scans the same corpus as the FTS path — issues (title + description),
/// pages (title + content), comments (content) — using
/// `instr(lower(field), lower(?)) > 0`. This avoids LIKE-wildcard injection
/// (a needle containing `%` / `_` is matched literally) at the cost of
/// ASCII-only case folding: `SQLite`'s `lower()` only folds `A–Z`, so non-ASCII
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
    visible_project_ids: Option<&HashSet<i64>>,
) -> Result<Vec<SearchResult>, LificError> {
    match q.sort.as_deref() {
        None | Some("relevance") | Some("recent") => {}
        Some(other) => {
            return Err(LificError::BadRequest(format!(
                "invalid sort '{other}'. Use relevance or recent."
            )));
        }
    }
    with_literal_progress_budget(conn, || {
        search_literal_unbounded(conn, q, limit, offset, visible_project_ids)
    })
}

fn with_literal_progress_budget<T>(
    conn: &Connection,
    query: impl FnOnce() -> Result<T, LificError>,
) -> Result<T, LificError> {
    let mut callbacks = 0;
    conn.progress_handler(
        LITERAL_PROGRESS_INTERVAL,
        Some(move || {
            callbacks += 1;
            callbacks >= MAX_LITERAL_PROGRESS_CALLBACKS
        }),
    );
    let result = query();
    conn.progress_handler(0, None::<fn() -> bool>);

    result.map_err(|error| match error {
        LificError::Database(error)
            if error.sqlite_error_code() == Some(ErrorCode::OperationInterrupted) =>
        {
            LificError::BadRequest(
                "literal search exceeded its execution budget; narrow the query".into(),
            )
        }
        error => error,
    })
}

fn search_literal_unbounded(
    conn: &Connection,
    q: &SearchQuery,
    limit: i64,
    offset: i64,
    visible_project_ids: Option<&HashSet<i64>>,
) -> Result<Vec<SearchResult>, LificError> {
    let needle = q.query.trim();
    // LIF-133 parity: an empty / whitespace-only needle returns nothing
    // rather than matching every row (instr(x, '') is always > 0).
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    if needle.len() > MAX_LITERAL_QUERY_BYTES {
        return Err(LificError::BadRequest(format!(
            "literal search query exceeds {MAX_LITERAL_QUERY_BYTES} bytes"
        )));
    }
    let needle = needle.to_string();
    let want = |result_type| {
        q.result_type
            .as_deref()
            .is_none_or(|filter| filter == result_type)
    };
    let mut rows: Vec<(String, SearchResult)> = Vec::new();

    if want("issue") {
        search_literal_issues(conn, q, &needle, visible_project_ids, &mut rows)?;
    }
    if want("page") {
        search_literal_pages(conn, q, &needle, visible_project_ids, &mut rows)?;
    }
    if want("comment") {
        search_literal_comments(conn, q, &needle, visible_project_ids, &mut rows)?;
    }
    // LIF-418: attachments join the literal scan too, so a punctuation-heavy
    // needle (`core:sodom`, a stack frame, a config key) finds the log file it
    // appears in, not just the issue that discusses it. They page and bound
    // with everything else: same match cap, same progress budget, same offset
    // clamp.
    if want("attachment") {
        search_literal_attachments(conn, q, &needle, visible_project_ids, &mut rows)?;
    }

    // Global recency sort (updated_at DESC), then id DESC as a stable
    // tiebreak, before paging.
    rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.id.cmp(&a.1.id)));

    let offset = usize::try_from(offset).expect("search offset is clamped");
    let fetch = usize::try_from(limit).expect("search limit is clamped");
    Ok(rows
        .into_iter()
        .map(|(_, r)| r)
        .skip(offset)
        .take(fetch)
        .collect())
}

fn search_literal_issues(
    conn: &Connection,
    q: &SearchQuery,
    needle: &str,
    visible_project_ids: Option<&HashSet<i64>>,
    rows: &mut Vec<(String, SearchResult)>,
) -> Result<(), LificError> {
    let mut params = literal_params(needle, q.project_id);
    let (visibility, visible_ids) =
        super::project_visibility_sql("i.project_id", visible_project_ids);
    params.extend(visible_ids.into_iter().map(Value::Integer));
    let mut stmt = conn.prepare(&format!(
        "SELECT i.id, p.identifier, i.sequence, i.title, i.description,
                i.project_id, i.updated_at
         FROM issues i
         JOIN projects p ON p.id = i.project_id
         WHERE (instr(lower(i.title), lower(?1)) > 0
            OR instr(lower(i.description), lower(?1)) > 0)
           AND (?2 IS NULL OR i.project_id = ?2)
           AND {visibility}",
    ))?;
    let mapped = stmt.query_map(rusqlite::params_from_iter(params), |row| {
        let title: String = row.get(3)?;
        let body: String = row.get(4)?;
        Ok((
            row.get(6)?,
            SearchResult {
                result_type: "issue".into(),
                id: row.get(0)?,
                identifier: Some(format!(
                    "{}-{}",
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?
                )),
                snippet: literal_snippet(&title, &body, needle),
                title,
                project_id: row.get(5)?,
                parent_page_id: None,
            },
        ))
    })?;
    extend_literal_rows(rows, mapped)
}

fn search_literal_pages(
    conn: &Connection,
    q: &SearchQuery,
    needle: &str,
    visible_project_ids: Option<&HashSet<i64>>,
    rows: &mut Vec<(String, SearchResult)>,
) -> Result<(), LificError> {
    let mut params = literal_params(needle, q.project_id);
    let (visibility, visible_ids) =
        super::project_visibility_sql("pg.project_id", visible_project_ids);
    params.extend(visible_ids.into_iter().map(Value::Integer));
    let mut stmt = conn.prepare(&format!(
        "SELECT pg.id, p.identifier, pg.sequence, pg.title, pg.content,
                pg.project_id, pg.updated_at
         FROM pages pg
         LEFT JOIN projects p ON p.id = pg.project_id
         WHERE (instr(lower(pg.title), lower(?1)) > 0
            OR instr(lower(pg.content), lower(?1)) > 0)
           AND (?2 IS NULL OR pg.project_id = ?2)
           AND {visibility}",
    ))?;
    let mapped = stmt.query_map(rusqlite::params_from_iter(params), |row| {
        let project = row.get::<_, Option<String>>(1)?;
        let sequence = row.get::<_, i64>(2)?;
        let title: String = row.get(3)?;
        let body: String = row.get(4)?;
        Ok((
            row.get(6)?,
            SearchResult {
                result_type: "page".into(),
                id: row.get(0)?,
                identifier: Some(match project {
                    Some(project) => format!("{project}-DOC-{sequence}"),
                    None => format!("DOC-{sequence}"),
                }),
                snippet: literal_snippet(&title, &body, needle),
                title,
                project_id: row.get(5)?,
                parent_page_id: None,
            },
        ))
    })?;
    extend_literal_rows(rows, mapped)
}

fn search_literal_comments(
    conn: &Connection,
    q: &SearchQuery,
    needle: &str,
    visible_project_ids: Option<&HashSet<i64>>,
    rows: &mut Vec<(String, SearchResult)>,
) -> Result<(), LificError> {
    let mut params = literal_params(needle, q.project_id);
    let (visibility, visible_ids) = super::project_visibility_sql(
        "COALESCE(ci.project_id, cpg.project_id)",
        visible_project_ids,
    );
    params.extend(visible_ids.into_iter().map(Value::Integer));
    let mut stmt = conn.prepare(&format!(
        "SELECT c.id, c.content, c.updated_at, c.issue_id, c.page_id,
                cip.identifier, ci.sequence, ci.project_id,
                cpp.identifier, cpg.sequence, cpg.project_id
         FROM comments c
         LEFT JOIN issues ci ON c.issue_id = ci.id
         LEFT JOIN pages cpg ON c.page_id = cpg.id
         LEFT JOIN projects cip ON cip.id = ci.project_id
         LEFT JOIN projects cpp ON cpp.id = cpg.project_id
         WHERE instr(lower(c.content), lower(?1)) > 0
           AND (?2 IS NULL OR COALESCE(ci.project_id, cpg.project_id) = ?2)
           AND {visibility}",
    ))?;
    let mapped = stmt.query_map(rusqlite::params_from_iter(params), |row| {
        let content = row.get::<_, String>(1)?;
        let issue_id = row.get::<_, Option<i64>>(3)?;
        let page_id = row.get::<_, Option<i64>>(4)?;
        let (identifier, project_id) = if issue_id.is_some() {
            let project = row.get::<_, Option<String>>(5)?;
            let sequence = row.get::<_, Option<i64>>(6)?;
            (
                project
                    .zip(sequence)
                    .map(|(project, sequence)| format!("{project}-{sequence}")),
                row.get(7)?,
            )
        } else if page_id.is_some() {
            let project = row.get::<_, Option<String>>(8)?;
            let sequence = row.get::<_, Option<i64>>(9)?;
            (
                sequence.map(|sequence| match project {
                    Some(project) => format!("{project}-DOC-{sequence}"),
                    None => format!("DOC-{sequence}"),
                }),
                row.get(10)?,
            )
        } else {
            (None, None)
        };
        Ok((
            row.get(2)?,
            SearchResult {
                result_type: "comment".into(),
                id: row.get(0)?,
                identifier,
                title: String::new(),
                snippet: literal_snippet("", &content, needle),
                project_id,
                parent_page_id: page_id,
            },
        ))
    })?;
    extend_literal_rows(rows, mapped)
}

/// Attachment rows for the literal scan: a match on the filename or on the
/// extracted text of an indexed text upload.
///
/// Scope and visibility are settled in SQL by the same `EXISTS` the FTS path
/// uses, and the link that gets rendered is resolved under the same filters,
/// so a file shared into a project the caller cannot see never surfaces that
/// project's entity.
fn search_literal_attachments(
    conn: &Connection,
    q: &SearchQuery,
    needle: &str,
    visible_project_ids: Option<&HashSet<i64>>,
    rows: &mut Vec<(String, SearchResult)>,
) -> Result<(), LificError> {
    let (link_scope, scope_params) =
        super::attachments::visible_link_scope_sql("a.id", q.project_id, visible_project_ids);
    let mut params = vec![Value::Text(needle.into())];
    params.extend(scope_params);
    let mut stmt = conn.prepare(&format!(
        "SELECT a.id, a.filename, a.created_at, COALESCE(f.extracted_text, '')
         FROM attachments a
         LEFT JOIN attachments_fts f ON f.attachment_id = a.id
         WHERE (instr(lower(a.filename), lower(?1)) > 0
                OR instr(lower(COALESCE(f.extracted_text, '')), lower(?1)) > 0)
           AND {link_scope}",
    ))?;
    let mapped = stmt.query_map(rusqlite::params_from_iter(params), |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    // Collected before resolving links: the link lookup runs its own query per
    // hit, and the cap has to bite on the raw match count either way.
    let mut matches = Vec::new();
    for row in mapped {
        literal_match_budget(rows.len() + matches.len())?;
        matches.push(row?);
    }
    drop(stmt);

    for (id, filename, created_at, text) in matches {
        let snippet = literal_snippet(&filename, &text, needle);
        if let Some(result) = attachment_result(
            conn,
            id,
            filename,
            snippet,
            q.project_id,
            visible_project_ids,
        )? {
            rows.push((created_at, result));
        }
    }
    Ok(())
}

fn literal_params(needle: &str, project_id: Option<i64>) -> Vec<Value> {
    vec![
        Value::Text(needle.into()),
        project_id.map_or(Value::Null, Value::Integer),
    ]
}

/// Refuse to keep collecting once the scan has matched more rows than any
/// caller could page through. `collected` counts the whole combined result
/// set, not one entity kind.
fn literal_match_budget(collected: usize) -> Result<(), LificError> {
    if collected >= MAX_LITERAL_MATCHES {
        return Err(LificError::BadRequest(
            "literal search matched too many records; narrow the query".into(),
        ));
    }
    Ok(())
}

fn extend_literal_rows(
    rows: &mut Vec<(String, SearchResult)>,
    mapped: impl IntoIterator<Item = rusqlite::Result<(String, SearchResult)>>,
) -> Result<(), LificError> {
    for row in mapped {
        literal_match_budget(rows.len())?;
        rows.push(row?);
    }
    Ok(())
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
/// `needle` in `haystack`, or `None`. Matches `SQLite`'s `instr(lower(), lower())`
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
    use crate::db::models::{AttachmentEntity, CreateIssue, CreatePage, CreateProject};
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
        assert_eq!(
            results.len(),
            1,
            "limit=-1 must clamp to 1, not return every match"
        );
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
    fn visible_search_filters_before_pagination() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let visible_project = seed_project(&conn, "VIS");
        let hidden_project = seed_project(&conn, "HID");

        for number in 0..3 {
            seed_issue(&conn, hidden_project, &format!("needle hidden {number}"));
        }
        let visible_issue = seed_issue(&conn, visible_project, "needle visible");

        let results = search_page(
            &conn,
            &SearchQuery {
                query: "needle".into(),
                limit: Some(1),
                ..Default::default()
            },
            Some(&HashSet::from([visible_project])),
        )
        .unwrap();

        assert_eq!(results.items.len(), 1);
        assert_eq!(results.items[0].id, visible_issue);
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
        assert_eq!(
            results[0].result_type, "page",
            "fresher entity must rank first"
        );
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
    fn search_formats_workspace_page_and_comment_identifiers() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let page = pages::create_page(
            &conn,
            &CreatePage {
                title: "Workspace quokka".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let uid = seed_user(&conn, "wren");
        comments::create_comment(&conn, CommentParent::Page(page.id), uid, "quokka details")
            .unwrap();

        let results = search(
            &conn,
            &SearchQuery {
                query: "quokka".into(),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(results.len(), 2);
        assert!(
            results
                .iter()
                .all(|result| result.identifier.as_deref() == Some("DOC-1"))
        );
        assert_eq!(
            results
                .iter()
                .find(|result| result.result_type == "comment")
                .and_then(|result| result.parent_page_id),
            Some(page.id)
        );
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
        comments::create_comment(
            &conn,
            CommentParent::Issue(iid),
            uid,
            "overlap in the comment",
        )
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

    /// A file used by both a visible and a hidden project must render the
    /// visible link. Preferring one is not enough: the hidden link is created
    /// first here, so the unscoped resolution would pick it and leak the
    /// hidden issue's identifier (and, through it, the fact that it exists).
    #[test]
    fn attachment_hit_never_renders_a_link_the_caller_cannot_see() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let hidden = seed_project(&conn, "HID");
        let visible = seed_project(&conn, "VIS");
        // Lower entity id sorts first, so this is the link an unrestricted
        // resolution lands on.
        let hidden_issue = seed_issue(&conn, hidden, "classified rollout plan");
        let visible_issue = seed_issue(&conn, visible, "public tracking issue");
        assert!(hidden_issue < visible_issue);

        use crate::db::queries::attachments as att;
        let attachment =
            att::create_attachment(&conn, "sha", "gribblenaut.log", "text/plain", 9, None).unwrap();
        att::link_attachment(&conn, attachment.id, AttachmentEntity::Issue, hidden_issue).unwrap();
        att::link_attachment(&conn, attachment.id, AttachmentEntity::Issue, visible_issue).unwrap();

        let query = SearchQuery {
            query: "gribblenaut".into(),
            ..Default::default()
        };

        // Unrestricted: the first link wins, and it is the hidden one.
        let unrestricted = search(&conn, &query).unwrap();
        assert_eq!(unrestricted.len(), 1);
        assert_eq!(unrestricted[0].identifier.as_deref(), Some("HID-1"));

        // Restricted to VIS: same file, resolved to the link the caller may
        // see, with no trace of the hidden project.
        let scoped = search_page(&conn, &query, Some(&HashSet::from([visible])))
            .unwrap()
            .items;
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].result_type, "attachment");
        assert_eq!(scoped[0].identifier.as_deref(), Some("VIS-1"));
        assert_eq!(scoped[0].project_id, Some(visible));

        // And the hidden entity itself stays unreachable, title included.
        let leak = search_page(
            &conn,
            &SearchQuery {
                query: "classified".into(),
                ..Default::default()
            },
            Some(&HashSet::from([visible])),
        )
        .unwrap();
        assert!(leak.items.is_empty(), "got: {:?}", leak.items);

        // A caller who can see nothing gets nothing, not everything.
        let nothing = search_page(&conn, &query, Some(&HashSet::new())).unwrap();
        assert!(nothing.items.is_empty(), "got: {:?}", nothing.items);
    }

    /// The combined entity+attachment list pages as one sequence: every hit is
    /// served exactly once, a page past the end is empty rather than wrapping,
    /// and a runaway offset clamps instead of erroring.
    #[test]
    fn combined_paging_stays_within_its_bounds() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let alpha = seed_issue(&conn, pid, "quokka alpha");
        let beta = seed_issue(&conn, pid, "quokka beta");
        seed_attachment(&conn, Some(alpha), "quokka-one.log", "text/plain", None);
        seed_attachment(&conn, Some(beta), "quokka-two.log", "text/plain", None);

        let page = |offset: i64| SearchQuery {
            query: "quokka".into(),
            limit: Some(1),
            offset: Some(offset),
            ..Default::default()
        };
        let mut seen = Vec::new();
        for offset in 0..4 {
            let hits = search(&conn, &page(offset)).unwrap();
            assert_eq!(hits.len(), 1, "offset {offset} lost a hit");
            seen.push((hits[0].result_type.clone(), hits[0].id));
        }
        assert_eq!(seen[0].0, "issue", "entity hits come first");
        assert_eq!(seen[3].0, "attachment");
        assert_eq!(
            seen.iter().collect::<HashSet<_>>().len(),
            4,
            "no hit is served twice: {seen:?}"
        );

        // Past the end: empty, and honest about there being no more.
        let past = search_page(
            &conn,
            &SearchQuery {
                offset: Some(4),
                ..page(0)
            },
            None,
        )
        .unwrap();
        assert!(past.items.is_empty(), "got: {:?}", past.items);
        assert!(!past.has_more);

        // A runaway offset clamps to MAX_SEARCH_OFFSET rather than erroring or
        // overflowing the window arithmetic.
        let runaway = search(&conn, &page(i64::MAX)).unwrap();
        assert!(runaway.is_empty(), "got: {runaway:?}");

        // Literal mode collects the same four hits under the same bounds.
        let literal = search(&conn, &lit("quokka")).unwrap();
        assert_eq!(literal.len(), 4);
        let literal_runaway = search(
            &conn,
            &SearchQuery {
                mode: Some("literal".into()),
                ..page(i64::MAX)
            },
        )
        .unwrap();
        assert!(literal_runaway.is_empty(), "got: {literal_runaway:?}");
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
    fn literal_rejects_unbounded_query_text() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let query = lit(&"x".repeat(MAX_LITERAL_QUERY_BYTES + 1));
        let error = search(&conn, &query).unwrap_err();
        assert!(error.to_string().contains("exceeds"));
    }

    #[test]
    fn literal_progress_budget_interrupts_expensive_sql() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let error = with_literal_progress_budget(&conn, || {
            conn.query_row(
                "WITH RECURSIVE numbers(n) AS (
                     SELECT 1
                     UNION ALL
                     SELECT n + 1 FROM numbers WHERE n < 100000000
                 )
                 SELECT sum(n) FROM numbers",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(Into::into)
        })
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("literal search exceeded its execution budget")
        );
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
        assert!(
            lits[0].snippet.contains("**core:sodom**"),
            "got: {}",
            lits[0].snippet
        );
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
        assert!(
            lits[0].snippet.contains("**[RequiredSpecs]**"),
            "got: {}",
            lits[0].snippet
        );
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
        assert!(
            lits[0].snippet.contains("**--trace-plans**"),
            "got: {}",
            lits[0].snippet
        );
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
            assert!(
                lits.is_empty(),
                "empty needle must match nothing: {query:?}"
            );
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
