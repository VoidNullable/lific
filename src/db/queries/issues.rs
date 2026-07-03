use rusqlite::{params, Connection};

use crate::db::models::*;
use crate::error::LificError;

use super::unescape_text;

/// Read a single issue with its computed identifier, labels, and relations.
pub fn get_issue(conn: &Connection, id: i64) -> Result<Issue, LificError> {
    let mut issue = conn
        .prepare_cached(
            "SELECT i.id, i.project_id, i.sequence, p.identifier, i.title, i.description,
                    i.status, i.priority, i.module_id, i.sort_order,
                    i.start_date, i.target_date, i.created_at, i.updated_at, i.source
             FROM issues i
             JOIN projects p ON p.id = i.project_id
             WHERE i.id = ?1",
        )?
        .query_row(params![id], |row| {
            let project_ident: String = row.get(3)?;
            let seq: i64 = row.get(2)?;
            Ok(Issue {
                id: row.get(0)?,
                project_id: row.get(1)?,
                sequence: seq,
                identifier: format!("{project_ident}-{seq}"),
                title: row.get(4)?,
                description: row.get(5)?,
                status: row.get(6)?,
                priority: row.get(7)?,
                module_id: row.get(8)?,
                sort_order: row.get(9)?,
                start_date: row.get(10)?,
                target_date: row.get(11)?,
                created_at: row.get(12)?,
                updated_at: row.get(13)?,
                source: row.get(14)?,
                labels: Vec::new(),
                blocks: Vec::new(),
                blocked_by: Vec::new(),
                relates_to: Vec::new(),
                duplicates: Vec::new(),
                duplicated_by: Vec::new(),
            })
        })
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                LificError::NotFound(format!("issue {id} not found"))
            }
            _ => e.into(),
        })?;

    let mut label_stmt = conn.prepare_cached(
        "SELECT l.name FROM labels l
         JOIN issue_labels il ON il.label_id = l.id
         WHERE il.issue_id = ?1",
    )?;
    issue.labels = label_stmt
        .query_map(params![id], |row| row.get(0))?
        .collect::<Result<Vec<String>, _>>()?;

    let mut blocks_stmt = conn.prepare_cached(
        "SELECT p.identifier, i.sequence FROM issue_relations ir
         JOIN issues i ON i.id = ir.target_id
         JOIN projects p ON p.id = i.project_id
         WHERE ir.source_id = ?1 AND ir.relation_type = 'blocks'",
    )?;
    issue.blocks = blocks_stmt
        .query_map(params![id], |row| {
            let proj: String = row.get(0)?;
            let seq: i64 = row.get(1)?;
            Ok(format!("{proj}-{seq}"))
        })?
        .collect::<Result<Vec<String>, _>>()?;

    let mut blocked_stmt = conn.prepare_cached(
        "SELECT p.identifier, i.sequence FROM issue_relations ir
         JOIN issues i ON i.id = ir.source_id
         JOIN projects p ON p.id = i.project_id
         WHERE ir.target_id = ?1 AND ir.relation_type = 'blocks'",
    )?;
    issue.blocked_by = blocked_stmt
        .query_map(params![id], |row| {
            let proj: String = row.get(0)?;
            let seq: i64 = row.get(1)?;
            Ok(format!("{proj}-{seq}"))
        })?
        .collect::<Result<Vec<String>, _>>()?;

    let mut relates_stmt = conn.prepare_cached(
        "SELECT p.identifier, i.sequence FROM issue_relations ir
         JOIN issues i ON i.id = CASE
            WHEN ir.source_id = ?1 THEN ir.target_id
            ELSE ir.source_id
         END
         JOIN projects p ON p.id = i.project_id
         WHERE (ir.source_id = ?1 OR ir.target_id = ?1)
           AND ir.relation_type = 'relates_to'",
    )?;
    issue.relates_to = relates_stmt
        .query_map(params![id], |row| {
            let proj: String = row.get(0)?;
            let seq: i64 = row.get(1)?;
            Ok(format!("{proj}-{seq}"))
        })?
        .collect::<Result<Vec<String>, _>>()?;

    // Duplicate is directional like `blocks`: a source→target 'duplicate' link
    // means source duplicates target. From the source's perspective the target
    // is what it `duplicates`; from the target's perspective the source is
    // captured in `duplicated_by`.
    let mut duplicates_stmt = conn.prepare_cached(
        "SELECT p.identifier, i.sequence FROM issue_relations ir
         JOIN issues i ON i.id = ir.target_id
         JOIN projects p ON p.id = i.project_id
         WHERE ir.source_id = ?1 AND ir.relation_type = 'duplicate'",
    )?;
    issue.duplicates = duplicates_stmt
        .query_map(params![id], |row| {
            let proj: String = row.get(0)?;
            let seq: i64 = row.get(1)?;
            Ok(format!("{proj}-{seq}"))
        })?
        .collect::<Result<Vec<String>, _>>()?;

    let mut duplicated_by_stmt = conn.prepare_cached(
        "SELECT p.identifier, i.sequence FROM issue_relations ir
         JOIN issues i ON i.id = ir.source_id
         JOIN projects p ON p.id = i.project_id
         WHERE ir.target_id = ?1 AND ir.relation_type = 'duplicate'",
    )?;
    issue.duplicated_by = duplicated_by_stmt
        .query_map(params![id], |row| {
            let proj: String = row.get(0)?;
            let seq: i64 = row.get(1)?;
            Ok(format!("{proj}-{seq}"))
        })?
        .collect::<Result<Vec<String>, _>>()?;

    Ok(issue)
}

/// Resolve "PRO-42" to an issue ID.
pub fn resolve_identifier(conn: &Connection, identifier: &str) -> Result<i64, LificError> {
    let parts: Vec<&str> = identifier.splitn(2, '-').collect();
    if parts.len() != 2 {
        return Err(LificError::BadRequest(format!(
            "invalid issue identifier: {identifier}"
        )));
    }
    let project_ident = parts[0];
    let sequence: i64 = parts[1]
        .parse()
        .map_err(|_| LificError::BadRequest(format!("invalid sequence number in: {identifier}")))?;

    conn.prepare_cached(
        "SELECT i.id FROM issues i
         JOIN projects p ON p.id = i.project_id
         WHERE p.identifier = ?1 AND i.sequence = ?2",
    )?
    .query_row(params![project_ident, sequence], |row| row.get(0))
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            LificError::NotFound(format!("issue {identifier} not found"))
        }
        _ => e.into(),
    })
}

/// List issues with optional filters.
pub fn list_issues(conn: &Connection, q: &ListIssuesQuery) -> Result<Vec<Issue>, LificError> {
    let mut sql = String::from(
        "SELECT DISTINCT i.id, i.project_id, i.sequence, p.identifier, i.title, i.description,
                i.status, i.priority, i.module_id, i.sort_order,
                i.start_date, i.target_date, i.created_at, i.updated_at
         FROM issues i
         JOIN projects p ON p.id = i.project_id",
    );
    let mut conditions: Vec<String> = Vec::new();
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(pid) = q.project_id {
        conditions.push(format!("i.project_id = ?{}", param_values.len() + 1));
        param_values.push(Box::new(pid));
    }
    if let Some(ref status) = q.status {
        conditions.push(format!("i.status = ?{}", param_values.len() + 1));
        param_values.push(Box::new(status.clone()));
    }
    if let Some(ref priority) = q.priority {
        conditions.push(format!("i.priority = ?{}", param_values.len() + 1));
        param_values.push(Box::new(priority.clone()));
    }
    if let Some(mid) = q.module_id {
        conditions.push(format!("i.module_id = ?{}", param_values.len() + 1));
        param_values.push(Box::new(mid));
    }
    if let Some(ref label) = q.label {
        sql.push_str(
            " JOIN issue_labels il ON il.issue_id = i.id JOIN labels l ON l.id = il.label_id",
        );
        conditions.push(format!("l.name = ?{}", param_values.len() + 1));
        param_values.push(Box::new(label.clone()));
    }
    // Date-window filters. `since` is inclusive, `until` exclusive. Stored
    // timestamps use SQLite's "YYYY-MM-DD HH:MM:SS" form; normalize an ISO
    // 'T' separator so "2026-06-10T12:00:00" compares correctly against it.
    for (col, op, value) in [
        ("i.created_at", ">=", &q.created_since),
        ("i.created_at", "<", &q.created_until),
        ("i.updated_at", ">=", &q.updated_since),
        ("i.updated_at", "<", &q.updated_until),
    ] {
        if let Some(v) = value {
            conditions.push(format!("{col} {op} ?{}", param_values.len() + 1));
            param_values.push(Box::new(v.replace('T', " ")));
        }
    }
    if q.workable == Some(true) {
        conditions.push(
            "NOT EXISTS (
                SELECT 1 FROM issue_relations ir
                JOIN issues blocker ON blocker.id = ir.source_id
                WHERE ir.target_id = i.id
                  AND ir.relation_type = 'blocks'
                  AND blocker.status != 'done'
            )"
            .to_string(),
        );
        conditions.push("i.status NOT IN ('done', 'cancelled')".to_string());
    }

    if !conditions.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conditions.join(" AND "));
    }

    // Whitelisted ORDER BY — user input selects from fixed SQL fragments,
    // it is never interpolated directly.
    let dir = match q.order.as_deref() {
        None | Some("asc") => "ASC",
        Some("desc") => "DESC",
        Some(other) => {
            return Err(LificError::BadRequest(format!(
                "invalid order '{other}'. Use asc or desc."
            )));
        }
    };
    let order_clause = match q.order_by.as_deref() {
        None | Some("sort_order") => format!("i.sort_order {dir}, i.sequence {dir}"),
        Some("sequence") => format!("i.sequence {dir}"),
        Some("created") | Some("created_at") => format!("i.created_at {dir}, i.sequence {dir}"),
        Some("updated") | Some("updated_at") => format!("i.updated_at {dir}, i.sequence {dir}"),
        Some(other) => {
            return Err(LificError::BadRequest(format!(
                "invalid order_by '{other}'. Use sort_order, sequence, created, or updated."
            )));
        }
    };
    sql.push_str(&format!(" ORDER BY {order_clause}"));

    // Clamp to a sane range (LIF-141): SQLite treats LIMIT -1 as "no limit",
    // so an unclamped `?limit=-1` would dump the whole table. Cap at 500 to
    // match MCP conventions; floor at 1 so a 0/negative value still paginates.
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let offset = q.offset.unwrap_or(0).max(0);
    sql.push_str(&format!(
        " LIMIT ?{} OFFSET ?{}",
        param_values.len() + 1,
        param_values.len() + 2
    ));
    param_values.push(Box::new(limit));
    param_values.push(Box::new(offset));

    let params_refs: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_refs.as_slice(), |row| {
        let project_ident: String = row.get(3)?;
        let seq: i64 = row.get(2)?;
        Ok(Issue {
            id: row.get(0)?,
            project_id: row.get(1)?,
            sequence: seq,
            identifier: format!("{project_ident}-{seq}"),
            title: row.get(4)?,
            description: row.get(5)?,
            status: row.get(6)?,
            priority: row.get(7)?,
            module_id: row.get(8)?,
            sort_order: row.get(9)?,
            start_date: row.get(10)?,
            target_date: row.get(11)?,
            created_at: row.get(12)?,
            updated_at: row.get(13)?,
            // `source` is import provenance, not needed for list rendering;
            // fetch it only on the single-issue read path (get_issue) to keep
            // this hot list query's column set stable.
            source: None,
            labels: Vec::new(),
            blocks: Vec::new(),
            blocked_by: Vec::new(),
            relates_to: Vec::new(),
            duplicates: Vec::new(),
            duplicated_by: Vec::new(),
        })
    })?;

    let mut issues: Vec<Issue> = rows.collect::<Result<Vec<_>, _>>()?;

    if !issues.is_empty() {
        // Map issue_id -> position so label rows attach in O(1) instead of a
        // linear `find()` per row. The old scan was O(page x label_rows),
        // which blows up super-linearly on large pages: a 10k-issue page went
        // from ~82ms to ~21ms (3.9x) in benchmarking once this quadratic term
        // was removed.
        let pos_by_id: std::collections::HashMap<i64, usize> = issues
            .iter()
            .enumerate()
            .map(|(idx, issue)| (issue.id, idx))
            .collect();

        let ids: Vec<i64> = issues.iter().map(|i| i.id).collect();
        let placeholders: String = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT il.issue_id, l.name FROM issue_labels il
             JOIN labels l ON l.id = il.label_id
             WHERE il.issue_id IN ({placeholders})"
        );
        let params: Vec<Box<dyn rusqlite::types::ToSql>> = ids
            .iter()
            .map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let label_rows = stmt.query_map(params_refs.as_slice(), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in label_rows {
            let (issue_id, label_name) = row?;
            if let Some(&idx) = pos_by_id.get(&issue_id) {
                issues[idx].labels.push(label_name);
            }
        }
    }

    Ok(issues)
}

/// Per-status issue counts for a project (LIF-161). One indexed GROUP BY
/// scan — cheap even on large projects, unlike pulling every row (which the
/// list endpoint caps anyway, so counting client-side undercounts).
pub fn count_issues_by_status(
    conn: &Connection,
    project_id: i64,
) -> Result<IssueStatusCounts, LificError> {
    let mut counts = IssueStatusCounts::default();
    let mut stmt = conn.prepare_cached(
        "SELECT status, COUNT(*) FROM issues WHERE project_id = ?1 GROUP BY status",
    )?;
    let rows = stmt.query_map(params![project_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (status, n) = row?;
        match status.as_str() {
            "backlog" => counts.backlog = n,
            "todo" => counts.todo = n,
            "active" => counts.active = n,
            "done" => counts.done = n,
            "cancelled" => counts.cancelled = n,
            // Unknown statuses can't be created through the API, but a
            // hand-edited DB row still counts toward the total.
            _ => {}
        }
        counts.total += n;
    }
    Ok(counts)
}

/// Create a new issue with auto-incremented sequence.
pub fn create_issue(conn: &Connection, input: &CreateIssue) -> Result<Issue, LificError> {
    let next_seq: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM issues WHERE project_id = ?1",
            params![input.project_id],
            |row| row.get(0),
        )
        .unwrap_or(1);

    // LIF-130: wrap the issue INSERT + label inserts in a savepoint so a
    // failed label attach can't leave a half-created issue behind. The id is
    // captured inside the closure because `last_insert_rowid()` after the
    // label loop would reflect the last issue_labels row, not the issue.
    let id = super::savepoint(conn, "create_issue", || {
        conn.execute(
            "INSERT INTO issues (project_id, sequence, title, description, status, priority, module_id, start_date, target_date, source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                input.project_id, next_seq, input.title, unescape_text(&input.description),
                input.status, input.priority, input.module_id, input.start_date, input.target_date,
                input.source,
            ],
        )?;
        let id = conn.last_insert_rowid();

        for label_name in &input.labels {
            conn.execute(
                "INSERT OR IGNORE INTO issue_labels (issue_id, label_id)
                 SELECT ?1, l.id FROM labels l
                 WHERE l.project_id = ?2 AND l.name = ?3",
                params![id, input.project_id, label_name],
            )?;
        }
        Ok(id)
    })?;

    get_issue(conn, id)
}

pub fn update_issue(conn: &Connection, id: i64, input: &UpdateIssue) -> Result<Issue, LificError> {
    get_issue(conn, id)?;

    super::savepoint(conn, "update_issue", || {
        if let Some(ref title) = input.title {
            conn.execute(
                "UPDATE issues SET title = ?1 WHERE id = ?2",
                params![title, id],
            )?;
        }
        if let Some(ref description) = input.description {
            conn.execute(
                "UPDATE issues SET description = ?1 WHERE id = ?2",
                params![unescape_text(description), id],
            )?;
        }
        if let Some(ref status) = input.status {
            conn.execute(
                "UPDATE issues SET status = ?1 WHERE id = ?2",
                params![status, id],
            )?;
        }
        if let Some(ref priority) = input.priority {
            conn.execute(
                "UPDATE issues SET priority = ?1 WHERE id = ?2",
                params![priority, id],
            )?;
        }
        // LIF-145: tristate. Outer Some means the client set the key; inner
        // None unassigns (NULL). rusqlite binds Option<i64> to NULL when None.
        if let Some(module_id) = input.module_id {
            conn.execute(
                "UPDATE issues SET module_id = ?1 WHERE id = ?2",
                params![module_id, id],
            )?;
        }
        if let Some(sort_order) = input.sort_order {
            conn.execute(
                "UPDATE issues SET sort_order = ?1 WHERE id = ?2",
                params![sort_order, id],
            )?;
        }
        if let Some(ref start_date) = input.start_date {
            conn.execute(
                "UPDATE issues SET start_date = ?1 WHERE id = ?2",
                params![start_date, id],
            )?;
        }
        if let Some(ref target_date) = input.target_date {
            conn.execute(
                "UPDATE issues SET target_date = ?1 WHERE id = ?2",
                params![target_date, id],
            )?;
        }
        if let Some(ref labels) = input.labels {
            conn.execute("DELETE FROM issue_labels WHERE issue_id = ?1", params![id])?;
            let project_id: i64 = conn.query_row(
                "SELECT project_id FROM issues WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )?;
            for label_name in labels {
                conn.execute(
                    "INSERT OR IGNORE INTO issue_labels (issue_id, label_id)
                     SELECT ?1, l.id FROM labels l
                     WHERE l.project_id = ?2 AND l.name = ?3",
                    params![id, project_id, label_name],
                )?;
            }
        }
        Ok(())
    })?;

    get_issue(conn, id)
}

pub fn delete_issue(conn: &Connection, id: i64) -> Result<(), LificError> {
    let changed = conn.execute("DELETE FROM issues WHERE id = ?1", params![id])?;
    if changed == 0 {
        return Err(LificError::NotFound(format!("issue {id} not found")));
    }
    Ok(())
}

pub fn link_issues(
    conn: &Connection,
    source_id: i64,
    target_id: i64,
    relation_type: &str,
) -> Result<(), LificError> {
    if !["blocks", "relates_to", "duplicate"].contains(&relation_type) {
        return Err(LificError::BadRequest(format!(
            "invalid relation type: {relation_type}"
        )));
    }
    // LIF-135: an issue relating to itself is never meaningful, and a
    // self-"blocks" makes the issue permanently non-workable.
    if source_id == target_id {
        return Err(LificError::BadRequest(
            "an issue cannot be linked to itself".into(),
        ));
    }
    conn.execute(
        "INSERT OR IGNORE INTO issue_relations (source_id, target_id, relation_type) VALUES (?1, ?2, ?3)",
        params![source_id, target_id, relation_type],
    )?;
    Ok(())
}

pub fn unlink_issues(conn: &Connection, source_id: i64, target_id: i64) -> Result<(), LificError> {
    conn.execute(
        "DELETE FROM issue_relations
         WHERE (source_id = ?1 AND target_id = ?2)
            OR (source_id = ?2 AND target_id = ?1)",
        params![source_id, target_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::queries::{projects, resources};

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

    fn seed_module(conn: &rusqlite::Connection, project_id: i64, name: &str) -> i64 {
        resources::create_module(
            conn,
            &CreateModule {
                project_id,
                name: name.into(),
                description: String::new(),
                status: "active".into(),
                emoji: None,
            },
        )
        .unwrap()
        .id
    }

    fn seed_label(conn: &rusqlite::Connection, project_id: i64, name: &str) -> i64 {
        resources::create_label(
            conn,
            &CreateLabel {
                project_id,
                name: name.into(),
                color: "#EF4444".into(),
            },
        )
        .unwrap()
        .id
    }

    fn quick_issue(
        conn: &rusqlite::Connection,
        pid: i64,
        title: &str,
        status: &str,
        priority: &str,
    ) -> Issue {
        create_issue(
            conn,
            &CreateIssue {
                project_id: pid,
                title: title.into(),
                description: String::new(),
                status: status.into(),
                priority: priority.into(),
                module_id: None,
                start_date: None,
                target_date: None,
                labels: vec![],
                source: None,
            },
        )
        .unwrap()
    }

    #[test]
    fn create_issue_auto_sequences() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let i1 = quick_issue(&conn, pid, "First", "backlog", "none");
        let i2 = quick_issue(&conn, pid, "Second", "backlog", "none");
        assert_eq!(i1.sequence, 1);
        assert_eq!(i2.sequence, 2);
        assert_eq!(i1.identifier, "TST-1");
        assert_eq!(i2.identifier, "TST-2");
    }

    #[test]
    fn sequences_are_per_project() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let p1 = seed_project(&conn, "AAA");
        let p2 = seed_project(&conn, "BBB");
        let a1 = quick_issue(&conn, p1, "A1", "backlog", "none");
        let b1 = quick_issue(&conn, p2, "B1", "backlog", "none");
        assert_eq!(a1.identifier, "AAA-1");
        assert_eq!(b1.identifier, "BBB-1");
    }

    #[test]
    fn create_issue_with_labels() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        seed_label(&conn, pid, "bug");
        seed_label(&conn, pid, "feature");

        let issue = create_issue(
            &conn,
            &CreateIssue {
                project_id: pid,
                title: "Labeled".into(),
                description: String::new(),
                status: "backlog".into(),
                priority: "none".into(),
                module_id: None,
                start_date: None,
                target_date: None,
                labels: vec!["bug".into(), "feature".into()],
                source: None,
            },
        )
        .unwrap();

        assert_eq!(issue.labels.len(), 2);
        assert!(issue.labels.contains(&"bug".to_string()));
        assert!(issue.labels.contains(&"feature".to_string()));
    }

    // LIF-130: the issue INSERT and its label attaches are one atomic unit.
    // If a label insert fails after the issue row is written, the savepoint
    // must roll the issue back too — no half-created issues.
    #[test]
    fn create_issue_rolls_back_when_label_attach_fails() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        seed_label(&conn, pid, "bug");

        // Force the label attach to fail after the issue INSERT succeeds.
        // RAISE(ABORT) in a trigger propagates even through INSERT OR IGNORE.
        conn.execute_batch(
            "CREATE TEMP TRIGGER fail_label_attach BEFORE INSERT ON issue_labels
             BEGIN SELECT RAISE(ABORT, 'label attach forced to fail'); END;",
        )
        .unwrap();

        let result = create_issue(
            &conn,
            &CreateIssue {
                project_id: pid,
                title: "Doomed".into(),
                description: String::new(),
                status: "backlog".into(),
                priority: "none".into(),
                module_id: None,
                start_date: None,
                target_date: None,
                labels: vec!["bug".into()],
                source: None,
            },
        );
        assert!(result.is_err(), "label attach failure must surface");

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM issues WHERE project_id = ?1",
                params![pid],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "failed create must not leave a half-created issue");
    }

    #[test]
    fn resolve_identifier_parses_correctly() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "PRO");
        quick_issue(&conn, pid, "Resolvable", "backlog", "none");
        let id = resolve_identifier(&conn, "PRO-1").unwrap();
        let issue = get_issue(&conn, id).unwrap();
        assert_eq!(issue.title, "Resolvable");
    }

    #[test]
    fn resolve_identifier_rejects_garbage() {
        let pool = test_db();
        let conn = pool.read().unwrap();
        assert!(resolve_identifier(&conn, "garbage").is_err());
        assert!(resolve_identifier(&conn, "PRO-abc").is_err());
        assert!(resolve_identifier(&conn, "").is_err());
    }

    #[test]
    fn list_filter_by_status() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        for s in &["backlog", "todo", "active", "done"] {
            quick_issue(&conn, pid, &format!("Issue {s}"), s, "none");
        }
        let active = list_issues(
            &conn,
            &ListIssuesQuery {
                project_id: Some(pid),
                status: Some("active".into()),
                priority: None,
                module_id: None,
                label: None,
                workable: None,
                limit: None,
                offset: None,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].status, "active");
    }

    // LIF-141: `?limit=-1` must not become SQLite's "no limit" and dump the
    // whole table. The floor clamps a negative/zero value to 1.
    #[test]
    fn list_issues_clamps_negative_limit() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        for i in 0..3 {
            quick_issue(&conn, pid, &format!("Issue {i}"), "backlog", "none");
        }
        let got = list_issues(
            &conn,
            &ListIssuesQuery {
                project_id: Some(pid),
                limit: Some(-1),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(got.len(), 1, "limit=-1 must clamp to 1, not return everything");
    }

    #[test]
    fn count_issues_by_status_tallies_each_bucket_and_total() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        // 2 backlog, 1 todo, 3 done; active/cancelled stay 0.
        for (status, n) in [("backlog", 2), ("todo", 1), ("done", 3)] {
            for i in 0..n {
                quick_issue(&conn, pid, &format!("{status} {i}"), status, "none");
            }
        }
        let counts = count_issues_by_status(&conn, pid).unwrap();
        assert_eq!(counts.backlog, 2);
        assert_eq!(counts.todo, 1);
        assert_eq!(counts.active, 0);
        assert_eq!(counts.done, 3);
        assert_eq!(counts.cancelled, 0);
        assert_eq!(counts.total, 6);
    }

    #[test]
    fn count_issues_by_status_scoped_to_project() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid_a = seed_project(&conn, "AAA");
        let pid_b = seed_project(&conn, "BBB");
        quick_issue(&conn, pid_a, "Mine", "todo", "none");
        quick_issue(&conn, pid_b, "Not mine", "todo", "none");
        quick_issue(&conn, pid_b, "Also not mine", "done", "none");

        let counts = count_issues_by_status(&conn, pid_a).unwrap();
        assert_eq!(counts.total, 1, "must not count other projects' issues");
        assert_eq!(counts.todo, 1);

        let empty = count_issues_by_status(&conn, pid_a + 999).unwrap();
        assert_eq!(empty.total, 0, "unknown project yields all-zero counts");
    }

    #[test]
    fn list_filter_by_priority() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        for p in &["urgent", "high", "medium", "low", "none"] {
            quick_issue(&conn, pid, &format!("Issue {p}"), "backlog", p);
        }
        let urgent = list_issues(
            &conn,
            &ListIssuesQuery {
                project_id: Some(pid),
                status: None,
                priority: Some("urgent".into()),
                module_id: None,
                label: None,
                workable: None,
                limit: None,
                offset: None,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(urgent.len(), 1);
        assert_eq!(urgent[0].priority, "urgent");
    }

    #[test]
    fn list_filter_by_module() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let mid = seed_module(&conn, pid, "Core");
        create_issue(
            &conn,
            &CreateIssue {
                project_id: pid,
                title: "In module".into(),
                description: String::new(),
                status: "backlog".into(),
                priority: "none".into(),
                module_id: Some(mid),
                start_date: None,
                target_date: None,
                labels: vec![],
                source: None,
            },
        )
        .unwrap();
        quick_issue(&conn, pid, "No module", "backlog", "none");

        let filtered = list_issues(
            &conn,
            &ListIssuesQuery {
                project_id: Some(pid),
                status: None,
                priority: None,
                module_id: Some(mid),
                label: None,
                workable: None,
                limit: None,
                offset: None,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].title, "In module");
    }

    #[test]
    fn list_filter_by_label() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        seed_label(&conn, pid, "bug");
        create_issue(
            &conn,
            &CreateIssue {
                project_id: pid,
                title: "Buggy".into(),
                description: String::new(),
                status: "backlog".into(),
                priority: "none".into(),
                module_id: None,
                start_date: None,
                target_date: None,
                labels: vec!["bug".into()],
                source: None,
            },
        )
        .unwrap();
        quick_issue(&conn, pid, "Clean", "backlog", "none");

        let bugs = list_issues(
            &conn,
            &ListIssuesQuery {
                project_id: Some(pid),
                status: None,
                priority: None,
                module_id: None,
                label: Some("bug".into()),
                workable: None,
                limit: None,
                offset: None,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(bugs.len(), 1);
        assert_eq!(bugs[0].title, "Buggy");
    }

    #[test]
    fn workable_excludes_blocked() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let blocker = quick_issue(&conn, pid, "Blocker", "todo", "none");
        let blocked = quick_issue(&conn, pid, "Blocked", "todo", "none");
        link_issues(&conn, blocker.id, blocked.id, "blocks").unwrap();

        let workable = list_issues(
            &conn,
            &ListIssuesQuery {
                project_id: Some(pid),
                status: None,
                priority: None,
                module_id: None,
                label: None,
                workable: Some(true),
                limit: None,
                offset: None,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(workable.len(), 1);
        assert_eq!(workable[0].title, "Blocker");
    }

    #[test]
    fn workable_unblocks_when_blocker_done() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let blocker = quick_issue(&conn, pid, "Blocker", "done", "none");
        let was_blocked = quick_issue(&conn, pid, "Was blocked", "todo", "none");
        link_issues(&conn, blocker.id, was_blocked.id, "blocks").unwrap();

        let workable = list_issues(
            &conn,
            &ListIssuesQuery {
                project_id: Some(pid),
                status: None,
                priority: None,
                module_id: None,
                label: None,
                workable: Some(true),
                limit: None,
                offset: None,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(workable.len(), 1);
        assert_eq!(workable[0].title, "Was blocked");
    }

    #[test]
    fn workable_excludes_done_and_cancelled() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        quick_issue(&conn, pid, "Active", "active", "none");
        quick_issue(&conn, pid, "Done", "done", "none");
        quick_issue(&conn, pid, "Cancelled", "cancelled", "none");

        let workable = list_issues(
            &conn,
            &ListIssuesQuery {
                project_id: Some(pid),
                status: None,
                priority: None,
                module_id: None,
                label: None,
                workable: Some(true),
                limit: None,
                offset: None,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(workable.len(), 1);
        assert_eq!(workable[0].title, "Active");
    }

    #[test]
    fn get_issue_includes_relations() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let i1 = quick_issue(&conn, pid, "Blocker", "todo", "none");
        let i2 = quick_issue(&conn, pid, "Blocked", "todo", "none");
        link_issues(&conn, i1.id, i2.id, "blocks").unwrap();

        let blocker = get_issue(&conn, i1.id).unwrap();
        let blocked = get_issue(&conn, i2.id).unwrap();
        assert!(blocker.blocks.contains(&"TST-2".to_string()));
        assert!(blocked.blocked_by.contains(&"TST-1".to_string()));
    }

    // LIF-136: a source→target 'duplicate' link must surface on both issues —
    // the source `duplicates` the target, the target is `duplicated_by` the
    // source. Previously this went write-only into issue_relations.
    #[test]
    fn get_issue_includes_duplicate_relation_both_directions() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let dup = quick_issue(&conn, pid, "Dup", "todo", "none");
        let canonical = quick_issue(&conn, pid, "Canonical", "todo", "none");
        link_issues(&conn, dup.id, canonical.id, "duplicate").unwrap();

        let got_dup = get_issue(&conn, dup.id).unwrap();
        assert_eq!(got_dup.duplicates, vec!["TST-2".to_string()]);
        assert!(got_dup.duplicated_by.is_empty());

        let got_canonical = get_issue(&conn, canonical.id).unwrap();
        assert_eq!(got_canonical.duplicated_by, vec!["TST-1".to_string()]);
        assert!(got_canonical.duplicates.is_empty());
    }

    #[test]
    fn get_issue_relations_preserve_cross_project_identifier() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid_a = seed_project(&conn, "AAA");
        let pid_b = seed_project(&conn, "BBB");
        // AAA-1 blocks BBB-1; AAA-2 relates_to BBB-1
        let a1 = quick_issue(&conn, pid_a, "A one", "todo", "none");
        let a2 = quick_issue(&conn, pid_a, "A two", "todo", "none");
        let b1 = quick_issue(&conn, pid_b, "B one", "todo", "none");
        link_issues(&conn, a1.id, b1.id, "blocks").unwrap();
        link_issues(&conn, a2.id, b1.id, "relates_to").unwrap();

        let got_a1 = get_issue(&conn, a1.id).unwrap();
        assert_eq!(got_a1.blocks, vec!["BBB-1".to_string()]);

        let got_b1 = get_issue(&conn, b1.id).unwrap();
        assert_eq!(got_b1.blocked_by, vec!["AAA-1".to_string()]);
        assert_eq!(got_b1.relates_to, vec!["AAA-2".to_string()]);

        let got_a2 = get_issue(&conn, a2.id).unwrap();
        assert_eq!(got_a2.relates_to, vec!["BBB-1".to_string()]);
    }

    #[test]
    fn unlink_removes_relation() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let i1 = quick_issue(&conn, pid, "A", "todo", "none");
        let i2 = quick_issue(&conn, pid, "B", "todo", "none");
        link_issues(&conn, i1.id, i2.id, "blocks").unwrap();
        unlink_issues(&conn, i1.id, i2.id).unwrap();
        assert!(get_issue(&conn, i1.id).unwrap().blocks.is_empty());
    }

    #[test]
    fn update_issue_partial_fields() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let issue = quick_issue(&conn, pid, "Original", "backlog", "low");

        let updated = update_issue(
            &conn,
            issue.id,
            &UpdateIssue {
                title: None,
                description: None,
                status: Some("active".into()),
                priority: Some("urgent".into()),
                module_id: None,
                sort_order: None,
                start_date: None,
                target_date: None,
                labels: None,
            },
        )
        .unwrap();

        assert_eq!(updated.title, "Original");
        assert_eq!(updated.status, "active");
        assert_eq!(updated.priority, "urgent");
    }

    #[test]
    fn delete_cascades_relations() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let i1 = quick_issue(&conn, pid, "Doomed", "todo", "none");
        let i2 = quick_issue(&conn, pid, "Survivor", "todo", "none");
        link_issues(&conn, i1.id, i2.id, "blocks").unwrap();
        delete_issue(&conn, i1.id).unwrap();
        assert!(get_issue(&conn, i2.id).unwrap().blocked_by.is_empty());
    }

    // LIF-135: self-links are rejected — a self-"blocks" would make the
    // issue permanently non-workable.
    #[test]
    fn link_rejects_self_link() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let issue = quick_issue(&conn, pid, "Loner", "todo", "none");

        for rel in ["blocks", "relates_to", "duplicate"] {
            let err = link_issues(&conn, issue.id, issue.id, rel).unwrap_err();
            assert!(
                matches!(err, LificError::BadRequest(_)),
                "self-link via '{rel}' must be BadRequest, got: {err:?}"
            );
        }

        // And the issue is still workable (no phantom self-block).
        let got = get_issue(&conn, issue.id).unwrap();
        assert!(got.blocks.is_empty());
        assert!(got.blocked_by.is_empty());
    }

    #[test]
    fn link_rejects_invalid_type() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let i1 = quick_issue(&conn, pid, "A", "todo", "none");
        let i2 = quick_issue(&conn, pid, "B", "todo", "none");
        assert!(link_issues(&conn, i1.id, i2.id, "invalid_type").is_err());
    }

    #[test]
    fn list_respects_limit() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        for i in 0..10 {
            quick_issue(&conn, pid, &format!("Issue {i}"), "backlog", "none");
        }

        let limited = list_issues(
            &conn,
            &ListIssuesQuery {
                project_id: Some(pid),
                status: None,
                priority: None,
                module_id: None,
                label: None,
                workable: None,
                limit: Some(3),
                offset: None,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(limited.len(), 3);
    }

    /// Read an issue's raw updated_at timestamp directly from the table.
    fn issue_updated_at(conn: &rusqlite::Connection, issue_id: i64) -> String {
        conn.query_row(
            "SELECT updated_at FROM issues WHERE id = ?1",
            params![issue_id],
            |row| row.get(0),
        )
        .unwrap()
    }

    // LIF-116: attaching a label is activity on the issue; the AFTER INSERT
    // trigger on issue_labels (migration 017) bumps issues.updated_at.
    // datetime('now') is 1-second resolution, so we sleep > 1s first.
    #[test]
    fn attaching_label_bumps_issue_updated_at() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let label_id = seed_label(&conn, pid, "bug");
        let issue = quick_issue(&conn, pid, "Labelable", "todo", "none");

        let before = issue_updated_at(&conn, issue.id);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        conn.execute(
            "INSERT INTO issue_labels (issue_id, label_id) VALUES (?1, ?2)",
            params![issue.id, label_id],
        )
        .unwrap();
        let after = issue_updated_at(&conn, issue.id);

        assert!(
            after > before,
            "expected attaching a label to bump issue updated_at: before={before}, after={after}"
        );
    }

    // LIF-116: detaching a label is activity too; the AFTER DELETE trigger
    // bumps updated_at using OLD.issue_id.
    #[test]
    fn detaching_label_bumps_issue_updated_at() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let label_id = seed_label(&conn, pid, "bug");
        let issue = quick_issue(&conn, pid, "Labelable", "todo", "none");
        conn.execute(
            "INSERT INTO issue_labels (issue_id, label_id) VALUES (?1, ?2)",
            params![issue.id, label_id],
        )
        .unwrap();

        let before = issue_updated_at(&conn, issue.id);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        conn.execute(
            "DELETE FROM issue_labels WHERE issue_id = ?1 AND label_id = ?2",
            params![issue.id, label_id],
        )
        .unwrap();
        let after = issue_updated_at(&conn, issue.id);

        assert!(
            after > before,
            "expected detaching a label to bump issue updated_at: before={before}, after={after}"
        );
    }

    // ── Date-window filters + sort control ───────────────────

    /// Pin an issue's created_at/updated_at to explicit values so date
    /// filter and ordering tests don't depend on wall-clock timing. The
    /// `issues_updated` trigger rewrites updated_at to now on every UPDATE,
    /// which would silently overwrite the pin — drop it first.
    fn pin_timestamps(conn: &rusqlite::Connection, issue_id: i64, created: &str, updated: &str) {
        conn.execute_batch("DROP TRIGGER IF EXISTS issues_updated;")
            .unwrap();
        conn.execute(
            "UPDATE issues SET created_at = ?1, updated_at = ?2 WHERE id = ?3",
            params![created, updated, issue_id],
        )
        .unwrap();
    }

    #[test]
    fn list_filters_by_created_window() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let old = quick_issue(&conn, pid, "Old", "todo", "none");
        let new = quick_issue(&conn, pid, "New", "todo", "none");
        pin_timestamps(&conn, old.id, "2026-01-05 10:00:00", "2026-01-05 10:00:00");
        pin_timestamps(&conn, new.id, "2026-03-20 10:00:00", "2026-03-20 10:00:00");

        let since = list_issues(
            &conn,
            &ListIssuesQuery {
                project_id: Some(pid),
                created_since: Some("2026-02-01".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(since.len(), 1);
        assert_eq!(since[0].title, "New");

        // `until` is exclusive: a bound equal to the row's timestamp drops it.
        let until = list_issues(
            &conn,
            &ListIssuesQuery {
                project_id: Some(pid),
                created_until: Some("2026-03-20 10:00:00".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(until.len(), 1);
        assert_eq!(until[0].title, "Old");
    }

    #[test]
    fn list_filters_by_updated_window() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let stale = quick_issue(&conn, pid, "Stale", "todo", "none");
        let fresh = quick_issue(&conn, pid, "Fresh", "todo", "none");
        pin_timestamps(&conn, stale.id, "2026-01-01 00:00:00", "2026-01-02 00:00:00");
        pin_timestamps(&conn, fresh.id, "2026-01-01 00:00:00", "2026-06-01 12:00:00");

        let recent = list_issues(
            &conn,
            &ListIssuesQuery {
                project_id: Some(pid),
                updated_since: Some("2026-05-01".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].title, "Fresh");
    }

    // Stored timestamps use a space separator ("2026-06-01 12:00:00") while
    // agents tend to send ISO 8601 with a 'T'. The filter must treat both
    // identically — 'T' (0x54) sorts after ' ' (0x20), so without
    // normalization a same-day ISO bound would skew the comparison.
    #[test]
    fn list_date_filter_normalizes_iso_t_separator() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let issue = quick_issue(&conn, pid, "Edge", "todo", "none");
        pin_timestamps(&conn, issue.id, "2026-06-01 12:00:00", "2026-06-01 12:00:00");

        let hit = list_issues(
            &conn,
            &ListIssuesQuery {
                project_id: Some(pid),
                created_since: Some("2026-06-01T12:00:00".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(hit.len(), 1, "inclusive bound equal to row timestamp must match");
    }

    #[test]
    fn list_orders_by_created_desc() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let a = quick_issue(&conn, pid, "First", "todo", "none");
        let b = quick_issue(&conn, pid, "Second", "todo", "none");
        pin_timestamps(&conn, a.id, "2026-01-01 00:00:00", "2026-01-01 00:00:00");
        pin_timestamps(&conn, b.id, "2026-02-01 00:00:00", "2026-02-01 00:00:00");

        let issues = list_issues(
            &conn,
            &ListIssuesQuery {
                project_id: Some(pid),
                order_by: Some("created".into()),
                order: Some("desc".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let titles: Vec<&str> = issues.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, vec!["Second", "First"]);
    }

    #[test]
    fn list_rejects_invalid_order_params() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");

        let bad_col = list_issues(
            &conn,
            &ListIssuesQuery {
                project_id: Some(pid),
                order_by: Some("priority; DROP TABLE issues".into()),
                ..Default::default()
            },
        );
        assert!(bad_col.is_err(), "unknown order_by must error, not be interpolated");

        let bad_dir = list_issues(
            &conn,
            &ListIssuesQuery {
                project_id: Some(pid),
                order: Some("sideways".into()),
                ..Default::default()
            },
        );
        assert!(bad_dir.is_err());
    }
}
