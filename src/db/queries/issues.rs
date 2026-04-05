use rusqlite::{params, Connection};

use crate::db::models::*;
use crate::error::LificError;

use super::unescape_text;

/// Read a single issue with its computed identifier, labels, and relations.
pub fn get_issue(conn: &Connection, id: i64) -> Result<Issue, LificError> {
    let mut issue = conn
        .query_row(
            "SELECT i.id, i.project_id, i.sequence, p.identifier, i.title, i.description,
                    i.status, i.priority, i.module_id, i.sort_order,
                    i.start_date, i.target_date, i.created_at, i.updated_at
             FROM issues i
             JOIN projects p ON p.id = i.project_id
             WHERE i.id = ?1",
            params![id],
            |row| {
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
                    labels: Vec::new(),
                    blocks: Vec::new(),
                    blocked_by: Vec::new(),
                    relates_to: Vec::new(),
                })
            },
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                LificError::NotFound(format!("issue {id} not found"))
            }
            _ => e.into(),
        })?;

    let mut label_stmt = conn.prepare(
        "SELECT l.name FROM labels l
         JOIN issue_labels il ON il.label_id = l.id
         WHERE il.issue_id = ?1",
    )?;
    issue.labels = label_stmt
        .query_map(params![id], |row| row.get(0))?
        .collect::<Result<Vec<String>, _>>()?;

    let project_ident = &issue.identifier.split('-').next().unwrap_or("").to_string();

    let mut blocks_stmt = conn.prepare(
        "SELECT i.sequence FROM issue_relations ir
         JOIN issues i ON i.id = ir.target_id
         WHERE ir.source_id = ?1 AND ir.relation_type = 'blocks'",
    )?;
    issue.blocks = blocks_stmt
        .query_map(params![id], |row| {
            let seq: i64 = row.get(0)?;
            Ok(format!("{project_ident}-{seq}"))
        })?
        .collect::<Result<Vec<String>, _>>()?;

    let mut blocked_stmt = conn.prepare(
        "SELECT i.sequence FROM issue_relations ir
         JOIN issues i ON i.id = ir.source_id
         WHERE ir.target_id = ?1 AND ir.relation_type = 'blocks'",
    )?;
    issue.blocked_by = blocked_stmt
        .query_map(params![id], |row| {
            let seq: i64 = row.get(0)?;
            Ok(format!("{project_ident}-{seq}"))
        })?
        .collect::<Result<Vec<String>, _>>()?;

    let mut relates_stmt = conn.prepare(
        "SELECT i.sequence FROM issue_relations ir
         JOIN issues i ON i.id = CASE
            WHEN ir.source_id = ?1 THEN ir.target_id
            ELSE ir.source_id
         END
         WHERE (ir.source_id = ?1 OR ir.target_id = ?1)
           AND ir.relation_type = 'relates_to'",
    )?;
    issue.relates_to = relates_stmt
        .query_map(params![id], |row| {
            let seq: i64 = row.get(0)?;
            Ok(format!("{project_ident}-{seq}"))
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

    conn.query_row(
        "SELECT i.id FROM issues i
         JOIN projects p ON p.id = i.project_id
         WHERE p.identifier = ?1 AND i.sequence = ?2",
        params![project_ident, sequence],
        |row| row.get(0),
    )
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

    sql.push_str(" ORDER BY i.sort_order, i.sequence");

    let limit = q.limit.unwrap_or(50);
    let offset = q.offset.unwrap_or(0);
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
            labels: Vec::new(),
            blocks: Vec::new(),
            blocked_by: Vec::new(),
            relates_to: Vec::new(),
        })
    })?;

    let mut issues: Vec<Issue> = rows.collect::<Result<Vec<_>, _>>()?;

    if !issues.is_empty() {
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
            if let Some(issue) = issues.iter_mut().find(|i| i.id == issue_id) {
                issue.labels.push(label_name);
            }
        }
    }

    Ok(issues)
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

    conn.execute(
        "INSERT INTO issues (project_id, sequence, title, description, status, priority, module_id, start_date, target_date)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            input.project_id, next_seq, input.title, unescape_text(&input.description),
            input.status, input.priority, input.module_id, input.start_date, input.target_date,
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

    get_issue(conn, id)
}

pub fn update_issue(conn: &Connection, id: i64, input: &UpdateIssue) -> Result<Issue, LificError> {
    get_issue(conn, id)?;

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
            },
        )
        .unwrap();

        assert_eq!(issue.labels.len(), 2);
        assert!(issue.labels.contains(&"bug".to_string()));
        assert!(issue.labels.contains(&"feature".to_string()));
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
            },
        )
        .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].status, "active");
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
            },
        )
        .unwrap();
        assert_eq!(limited.len(), 3);
    }
}
