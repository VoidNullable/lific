use rusqlite::{params, Connection, OptionalExtension};

use crate::db::models::*;
use crate::error::LificError;

use super::{savepoint, unescape_text};

/// Outcome of toggling a step's `done`, so the MCP/REST layer can narrate the
/// side effect (e.g. "Step done → LIF-42 marked done").
#[derive(Debug, Clone, serde::Serialize)]
pub struct StepDoneEffect {
    pub step_id: i64,
    pub done: bool,
    /// The linked issue's identifier, if the step references one.
    pub issue_identifier: Option<String>,
    /// True when toggling this step changed the linked issue's status.
    pub issue_status_changed: bool,
    /// The issue's new status after the toggle (when an issue is linked).
    pub issue_new_status: Option<String>,
}

/// Resolve "PROJ-PLAN-7" to a plan id.
pub fn resolve_plan_identifier(conn: &Connection, identifier: &str) -> Result<i64, LificError> {
    let idx = identifier.rfind("-PLAN-").ok_or_else(|| {
        LificError::BadRequest(format!("invalid plan identifier: {identifier}"))
    })?;
    let project_ident = &identifier[..idx];
    let sequence: i64 = identifier[idx + 6..]
        .parse()
        .map_err(|_| LificError::BadRequest(format!("invalid sequence in: {identifier}")))?;

    conn.query_row(
        "SELECT pl.id FROM plans pl
         JOIN projects p ON p.id = pl.project_id
         WHERE p.identifier = ?1 AND pl.sequence = ?2",
        params![project_ident, sequence],
        |row| row.get(0),
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            LificError::NotFound(format!("plan {identifier} not found"))
        }
        _ => e.into(),
    })
}

/// Read a plan's header row (no step tree). Computes identifier + anchor.
fn read_plan_row(conn: &Connection, id: i64) -> Result<Plan, LificError> {
    conn.query_row(
        "SELECT pl.id, pl.project_id, pl.sequence, p.identifier, pl.issue_id, pl.title,
                pl.status, pl.created_at, pl.updated_at,
                (SELECT ap.identifier || '-' || ai.sequence
                   FROM issues ai JOIN projects ap ON ap.id = ai.project_id
                  WHERE ai.id = pl.issue_id),
                (SELECT COUNT(*) FROM plan_steps s WHERE s.plan_id = pl.id),
                (SELECT COUNT(*) FROM plan_steps s WHERE s.plan_id = pl.id AND s.done = 1)
         FROM plans pl
         JOIN projects p ON p.id = pl.project_id
         WHERE pl.id = ?1",
        params![id],
        |row| {
            let proj: String = row.get(3)?;
            let seq: i64 = row.get(2)?;
            Ok(Plan {
                id: row.get(0)?,
                project_id: row.get(1)?,
                sequence: seq,
                identifier: format!("{proj}-PLAN-{seq}"),
                issue_id: row.get(4)?,
                anchor_identifier: row.get(9)?,
                title: row.get(5)?,
                status: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
                steps: Vec::new(),
                step_count: row.get(10)?,
                done_count: row.get(11)?,
            })
        },
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => LificError::NotFound(format!("plan {id} not found")),
        _ => e.into(),
    })
}

/// Read a full plan with its nested step tree assembled in app code.
pub fn get_plan(conn: &Connection, id: i64) -> Result<Plan, LificError> {
    let mut plan = read_plan_row(conn, id)?;

    // Flat fetch of every step (adjacency list), ordered so siblings come out
    // in position order. LEFT JOIN issues for the linked-issue identifier and
    // status (powers "done (via LIF-42)" provenance).
    let mut stmt = conn.prepare_cached(
        "SELECT s.id, s.plan_id, s.parent_step_id, s.position, s.title, s.description,
                s.issue_id,
                CASE WHEN s.issue_id IS NULL THEN NULL
                     ELSE ip.identifier || '-' || i.sequence END,
                i.status,
                s.done, s.reopened_via_issue_at, s.created_at, s.edited_at
         FROM plan_steps s
         LEFT JOIN issues i ON i.id = s.issue_id
         LEFT JOIN projects ip ON ip.id = i.project_id
         WHERE s.plan_id = ?1
         ORDER BY s.position ASC, s.id ASC",
    )?;
    let rows = stmt.query_map(params![id], |row| {
        Ok(PlanStepNode {
            id: row.get(0)?,
            plan_id: row.get(1)?,
            parent_step_id: row.get(2)?,
            position: row.get(3)?,
            title: row.get(4)?,
            description: row.get(5)?,
            issue_id: row.get(6)?,
            issue_identifier: row.get(7)?,
            issue_status: row.get(8)?,
            done: row.get::<_, i64>(9)? != 0,
            reopened_via_issue_at: row.get(10)?,
            created_at: row.get(11)?,
            edited_at: row.get(12)?,
            children: Vec::new(),
        })
    })?;
    let flat: Vec<PlanStepNode> = rows.collect::<Result<Vec<_>, _>>()?;

    plan.steps = assemble_tree(flat);
    Ok(plan)
}

/// Turn a flat, position-ordered list of steps into a nested tree. O(n):
/// index children by parent id, then recurse from the roots.
fn assemble_tree(flat: Vec<PlanStepNode>) -> Vec<PlanStepNode> {
    use std::collections::HashMap;
    let mut children_of: HashMap<Option<i64>, Vec<PlanStepNode>> = HashMap::new();
    for node in flat {
        children_of.entry(node.parent_step_id).or_default().push(node);
    }
    fn build(
        parent: Option<i64>,
        children_of: &mut std::collections::HashMap<Option<i64>, Vec<PlanStepNode>>,
    ) -> Vec<PlanStepNode> {
        let mut nodes = children_of.remove(&parent).unwrap_or_default();
        for node in nodes.iter_mut() {
            node.children = build(Some(node.id), children_of);
        }
        nodes
    }
    build(None, &mut children_of)
}

/// List plans (header rows only, no tree) with optional status filter.
pub fn list_plans(conn: &Connection, q: &ListPlansQuery) -> Result<Vec<Plan>, LificError> {
    // Page-then-aggregate. The original ran two correlated COUNT(*) subqueries
    // over plan_steps per output row; benchmarking + EXPLAIN QUERY PLAN showed
    // a naive `LEFT JOIN plan_steps ... GROUP BY` was actually *slower*,
    // because it forced a temp B-tree over every step of every matching plan
    // before ORDER BY/LIMIT could trim to one page (research takeaway: "reduce
    // the result set early"). So we pick + order + limit the plan page FIRST in
    // an inner query (no joins, uses idx_plans_project), then LEFT JOIN steps
    // onto just those rows and aggregate. COUNT(s.id) (not COUNT(*)) reports 0
    // for stepless plans, and COALESCE(SUM(s.done),0) folds the done tally into
    // the same single scan. The anchor identifier stays a scalar PK lookup.
    let mut inner = String::from(
        "SELECT pl.id, pl.project_id, pl.sequence, pl.issue_id, pl.title,
                pl.status, pl.created_at, pl.updated_at
         FROM plans pl",
    );
    let mut conditions: Vec<String> = Vec::new();
    let mut pv: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(pid) = q.project_id {
        conditions.push(format!("pl.project_id = ?{}", pv.len() + 1));
        pv.push(Box::new(pid));
    }
    if let Some(ref status) = q.status {
        conditions.push(format!("pl.status = ?{}", pv.len() + 1));
        pv.push(Box::new(status.clone()));
    }
    if !conditions.is_empty() {
        inner.push_str(" WHERE ");
        inner.push_str(&conditions.join(" AND "));
    }
    inner.push_str(" ORDER BY pl.updated_at DESC, pl.id DESC");

    // LIF-141 class: clamp limit to a sane bound, never unbounded/negative.
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let offset = q.offset.unwrap_or(0).max(0);
    inner.push_str(&format!(" LIMIT ?{} OFFSET ?{}", pv.len() + 1, pv.len() + 2));
    pv.push(Box::new(limit));
    pv.push(Box::new(offset));

    let sql = format!(
        "SELECT pl.id, pl.project_id, pl.sequence, p.identifier, pl.issue_id, pl.title,
                pl.status, pl.created_at, pl.updated_at,
                (SELECT ap.identifier || '-' || ai.sequence
                   FROM issues ai JOIN projects ap ON ap.id = ai.project_id
                  WHERE ai.id = pl.issue_id),
                COUNT(s.id),
                COALESCE(SUM(s.done), 0)
         FROM ({inner}) pl
         JOIN projects p ON p.id = pl.project_id
         LEFT JOIN plan_steps s ON s.plan_id = pl.id
         GROUP BY pl.id
         ORDER BY pl.updated_at DESC, pl.id DESC"
    );

    let refs: Vec<&dyn rusqlite::types::ToSql> = pv.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(refs.as_slice(), |row| {
        let proj: String = row.get(3)?;
        let seq: i64 = row.get(2)?;
        Ok(Plan {
            id: row.get(0)?,
            project_id: row.get(1)?,
            sequence: seq,
            identifier: format!("{proj}-PLAN-{seq}"),
            issue_id: row.get(4)?,
            anchor_identifier: row.get(9)?,
            title: row.get(5)?,
            status: row.get(6)?,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
            steps: Vec::new(),
            step_count: row.get(10)?,
            done_count: row.get(11)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Create a plan + its full nested step tree atomically.
pub fn create_plan(conn: &Connection, input: &CreatePlan) -> Result<Plan, LificError> {
    let next_seq: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM plans WHERE project_id = ?1",
            params![input.project_id],
            |row| row.get(0),
        )
        .unwrap_or(1);

    let id = savepoint(conn, "create_plan", || {
        conn.execute(
            "INSERT INTO plans (project_id, sequence, issue_id, title)
             VALUES (?1, ?2, ?3, ?4)",
            params![input.project_id, next_seq, input.issue_id, input.title],
        )?;
        let plan_id = conn.last_insert_rowid();
        insert_step_tree(conn, plan_id, None, &input.steps)?;
        Ok(plan_id)
    })?;

    get_plan(conn, id)
}

/// Recursively insert a tree of steps, assigning sibling positions 0..n.
fn insert_step_tree(
    conn: &Connection,
    plan_id: i64,
    parent_step_id: Option<i64>,
    steps: &[CreatePlanStep],
) -> Result<(), LificError> {
    for (pos, step) in steps.iter().enumerate() {
        conn.execute(
            "INSERT INTO plan_steps (plan_id, parent_step_id, position, title, description, issue_id, done)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                plan_id,
                parent_step_id,
                pos as i64,
                step.title,
                unescape_text(&step.description),
                step.issue_id,
                step.done as i64,
            ],
        )?;
        let child_id = conn.last_insert_rowid();
        if !step.steps.is_empty() {
            insert_step_tree(conn, plan_id, Some(child_id), &step.steps)?;
        }
    }
    Ok(())
}

pub fn update_plan(conn: &Connection, id: i64, input: &UpdatePlan) -> Result<Plan, LificError> {
    read_plan_row(conn, id)?;
    savepoint(conn, "update_plan", || {
        if let Some(ref title) = input.title {
            conn.execute("UPDATE plans SET title = ?1 WHERE id = ?2", params![title, id])?;
        }
        if let Some(ref status) = input.status {
            if !["active", "done", "archived"].contains(&status.as_str()) {
                return Err(LificError::BadRequest(format!(
                    "invalid plan status '{status}'. Use active, done, or archived."
                )));
            }
            conn.execute(
                "UPDATE plans SET status = ?1 WHERE id = ?2",
                params![status, id],
            )?;
        }
        if let Some(anchor) = input.issue_id {
            conn.execute(
                "UPDATE plans SET issue_id = ?1 WHERE id = ?2",
                params![anchor, id],
            )?;
        }
        Ok(())
    })?;
    get_plan(conn, id)
}

pub fn delete_plan(conn: &Connection, id: i64) -> Result<(), LificError> {
    let changed = conn.execute("DELETE FROM plans WHERE id = ?1", params![id])?;
    if changed == 0 {
        return Err(LificError::NotFound(format!("plan {id} not found")));
    }
    Ok(())
}

// ── Step operations ──────────────────────────────────────────

/// Look up which plan a step belongs to (and validate it exists).
fn step_plan_id(conn: &Connection, step_id: i64) -> Result<i64, LificError> {
    conn.query_row(
        "SELECT plan_id FROM plan_steps WHERE id = ?1",
        params![step_id],
        |row| row.get(0),
    )
    .optional()?
    .ok_or_else(|| LificError::NotFound(format!("plan step {step_id} not found")))
}

/// Verify a step belongs to the given plan (scopes MCP/REST mutations so a
/// step id from another plan can't be edited through the wrong plan handle).
pub fn assert_step_in_plan(
    conn: &Connection,
    plan_id: i64,
    step_id: i64,
) -> Result<(), LificError> {
    if step_plan_id(conn, step_id)? != plan_id {
        return Err(LificError::BadRequest(format!(
            "step {step_id} does not belong to this plan"
        )));
    }
    Ok(())
}

/// Directly set a step's title (full rename, vs the find/replace edit_step_text).
pub fn set_step_title(conn: &Connection, step_id: i64, title: &str) -> Result<(), LificError> {
    let changed = conn.execute(
        "UPDATE plan_steps SET title = ?1, edited_at = datetime('now') WHERE id = ?2",
        params![title, step_id],
    )?;
    if changed == 0 {
        return Err(LificError::NotFound(format!("plan step {step_id} not found")));
    }
    Ok(())
}

/// Directly set a step's description (the web UI body editor — LIF-177).
pub fn set_step_description(
    conn: &Connection,
    step_id: i64,
    description: &str,
) -> Result<(), LificError> {
    let changed = conn.execute(
        "UPDATE plan_steps SET description = ?1, edited_at = datetime('now') WHERE id = ?2",
        params![unescape_text(description), step_id],
    )?;
    if changed == 0 {
        return Err(LificError::NotFound(format!("plan step {step_id} not found")));
    }
    Ok(())
}

/// Read a step's current parent (for position-only moves).
pub fn step_parent(conn: &Connection, step_id: i64) -> Result<Option<i64>, LificError> {
    conn.query_row(
        "SELECT parent_step_id FROM plan_steps WHERE id = ?1",
        params![step_id],
        |row| row.get(0),
    )
    .optional()?
    .ok_or_else(|| LificError::NotFound(format!("plan step {step_id} not found")))
}

/// Add a step under `parent_step_id` (or at the root when None), at the end of
/// its sibling group.
pub fn add_step(
    conn: &Connection,
    plan_id: i64,
    parent_step_id: Option<i64>,
    title: &str,
    description: &str,
    issue_id: Option<i64>,
) -> Result<i64, LificError> {
    let next_pos: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM plan_steps
             WHERE plan_id = ?1 AND parent_step_id IS ?2",
            params![plan_id, parent_step_id],
            |row| row.get(0),
        )
        .unwrap_or(0);
    conn.execute(
        "INSERT INTO plan_steps (plan_id, parent_step_id, position, title, description, issue_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            plan_id,
            parent_step_id,
            next_pos,
            title,
            unescape_text(description),
            issue_id
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Find/replace on a step's title or description (mirrors edit_issue/edit_page).
pub fn edit_step_text(
    conn: &Connection,
    step_id: i64,
    field: &str,
    find: &str,
    replace: &str,
    replace_all: bool,
) -> Result<(), LificError> {
    let column = match field {
        "title" => "title",
        "description" | "" => "description",
        other => {
            return Err(LificError::BadRequest(format!(
                "invalid field '{other}'. Use title or description."
            )))
        }
    };
    let current: String = conn
        .query_row(
            &format!("SELECT {column} FROM plan_steps WHERE id = ?1"),
            params![step_id],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| LificError::NotFound(format!("plan step {step_id} not found")))?;

    let matches = current.matches(find).count();
    if matches == 0 {
        return Err(LificError::BadRequest(format!(
            "find string not found in step {field}"
        )));
    }
    if matches > 1 && !replace_all {
        return Err(LificError::BadRequest(format!(
            "find string matches {matches} times; pass replace_all or disambiguate"
        )));
    }
    let updated = if replace_all {
        current.replace(find, replace)
    } else {
        current.replacen(find, replace, 1)
    };
    conn.execute(
        &format!("UPDATE plan_steps SET {column} = ?1, edited_at = datetime('now') WHERE id = ?2"),
        params![updated, step_id],
    )?;
    Ok(())
}

/// Toggle a step's done state. When marking done and the step references an
/// issue, the issue is marked done too (step→issue side effect). The issue
/// UPDATE then drives the 020 cascade (other steps + anchored plans).
pub fn set_step_done(
    conn: &Connection,
    step_id: i64,
    done: bool,
) -> Result<StepDoneEffect, LificError> {
    let (issue_id, _plan_id): (Option<i64>, i64) = conn
        .query_row(
            "SELECT issue_id, plan_id FROM plan_steps WHERE id = ?1",
            params![step_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| LificError::NotFound(format!("plan step {step_id} not found")))?;

    let mut effect = StepDoneEffect {
        step_id,
        done,
        issue_identifier: None,
        issue_status_changed: false,
        issue_new_status: None,
    };

    savepoint(conn, "set_step_done", || {
        if done {
            conn.execute(
                "UPDATE plan_steps SET done = 1, reopened_via_issue_at = NULL,
                                       edited_at = datetime('now') WHERE id = ?1",
                params![step_id],
            )?;
        } else {
            conn.execute(
                "UPDATE plan_steps SET done = 0, edited_at = datetime('now') WHERE id = ?1",
                params![step_id],
            )?;
        }

        if let Some(iid) = issue_id {
            let (ident, status): (String, String) = conn.query_row(
                "SELECT p.identifier || '-' || i.sequence, i.status
                 FROM issues i JOIN projects p ON p.id = i.project_id WHERE i.id = ?1",
                params![iid],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            effect.issue_identifier = Some(ident);

            // Only the done direction propagates to the issue (issue-authoritative
            // close-down; we never reopen an issue from a step un-check).
            if done && status != "done" {
                conn.execute(
                    "UPDATE issues SET status = 'done' WHERE id = ?1",
                    params![iid],
                )?;
                effect.issue_status_changed = true;
                effect.issue_new_status = Some("done".to_string());
            } else {
                effect.issue_new_status = Some(status);
            }
        }
        Ok(())
    })?;

    Ok(effect)
}

/// Read a step's currently-linked issue id, if any. LIF-198: the MCP layer
/// uses this to check for a Maintainer role on the *issue's* project before
/// a `done` toggle is allowed to close it, when that issue lives in a
/// different project than the step's own plan (mirrors `link_issues`'
/// both-sides check).
pub fn step_issue_id(conn: &Connection, step_id: i64) -> Result<Option<i64>, LificError> {
    conn.query_row(
        "SELECT issue_id FROM plan_steps WHERE id = ?1",
        params![step_id],
        |row| row.get(0),
    )
    .optional()?
    .ok_or_else(|| LificError::NotFound(format!("plan step {step_id} not found")))
}

/// Attach (or clear, with None) the issue a step references.
pub fn set_step_issue(
    conn: &Connection,
    step_id: i64,
    issue_id: Option<i64>,
) -> Result<(), LificError> {
    let changed = conn.execute(
        "UPDATE plan_steps SET issue_id = ?1, edited_at = datetime('now') WHERE id = ?2",
        params![issue_id, step_id],
    )?;
    if changed == 0 {
        return Err(LificError::NotFound(format!("plan step {step_id} not found")));
    }
    Ok(())
}

/// Reparent / reorder a step. Guards against making a step a child of its own
/// descendant (which would orphan a subtree into a cycle).
pub fn move_step(
    conn: &Connection,
    step_id: i64,
    new_parent: Option<i64>,
    new_position: Option<i64>,
) -> Result<(), LificError> {
    let plan_id = step_plan_id(conn, step_id)?;

    if let Some(parent) = new_parent {
        if parent == step_id {
            return Err(LificError::BadRequest("a step cannot be its own parent".into()));
        }
        // Walk up from the proposed parent; if we reach step_id, the parent is
        // a descendant of the step → cycle.
        let mut cursor = Some(parent);
        while let Some(cur) = cursor {
            if cur == step_id {
                return Err(LificError::BadRequest(
                    "cannot move a step beneath its own descendant".into(),
                ));
            }
            cursor = conn
                .query_row(
                    "SELECT parent_step_id FROM plan_steps WHERE id = ?1",
                    params![cur],
                    |row| row.get(0),
                )
                .optional()?
                .flatten();
        }
        // Parent must live in the same plan.
        let parent_plan = step_plan_id(conn, parent)?;
        if parent_plan != plan_id {
            return Err(LificError::BadRequest(
                "cannot move a step into a different plan".into(),
            ));
        }
    }

    let position = match new_position {
        Some(p) => p,
        None => conn
            .query_row(
                "SELECT COALESCE(MAX(position), -1) + 1 FROM plan_steps
                 WHERE plan_id = ?1 AND parent_step_id IS ?2",
                params![plan_id, new_parent],
                |row| row.get(0),
            )
            .unwrap_or(0),
    };

    conn.execute(
        "UPDATE plan_steps SET parent_step_id = ?1, position = ?2, edited_at = datetime('now')
         WHERE id = ?3",
        params![new_parent, position, step_id],
    )?;
    Ok(())
}

/// Delete a step (and, via ON DELETE CASCADE, its whole subtree).
pub fn delete_step(conn: &Connection, step_id: i64) -> Result<(), LificError> {
    let changed = conn.execute("DELETE FROM plan_steps WHERE id = ?1", params![step_id])?;
    if changed == 0 {
        return Err(LificError::NotFound(format!("plan step {step_id} not found")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::queries::{issues, projects};

    fn test_db() -> db::DbPool {
        db::open_memory().expect("test db")
    }

    fn seed_project(conn: &Connection, ident: &str) -> i64 {
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

    fn seed_issue(conn: &Connection, pid: i64, title: &str, status: &str) -> Issue {
        issues::create_issue(
            conn,
            &CreateIssue {
                project_id: pid,
                title: title.into(),
                description: String::new(),
                status: status.into(),
                priority: "none".into(),
                module_id: None,
                start_date: None,
                target_date: None,
                labels: vec![],
                source: None,
            },
        )
        .unwrap()
    }

    fn simple_step(title: &str, issue_id: Option<i64>) -> CreatePlanStep {
        CreatePlanStep {
            title: title.into(),
            description: String::new(),
            issue_id,
            done: false,
            steps: vec![],
        }
    }

    #[test]
    fn create_nested_plan_round_trips() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");

        let plan = create_plan(
            &conn,
            &CreatePlan {
                project_id: pid,
                title: "Build feature".into(),
                issue_id: None,
                steps: vec![
                    CreatePlanStep {
                        title: "Backend".into(),
                        description: "do the backend".into(),
                        issue_id: None,
                        done: false,
                        steps: vec![simple_step("schema", None), simple_step("queries", None)],
                    },
                    simple_step("Frontend", None),
                ],
            },
        )
        .unwrap();

        assert_eq!(plan.identifier, "TST-PLAN-1");
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[0].title, "Backend");
        assert_eq!(plan.steps[0].children.len(), 2);
        assert_eq!(plan.steps[0].children[0].title, "schema");
        assert_eq!(plan.steps[0].children[1].title, "queries");
        assert_eq!(plan.steps[1].title, "Frontend");
        assert_eq!(plan.step_count, 4);
    }

    #[test]
    fn plan_sequence_is_per_project() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let a = seed_project(&conn, "AAA");
        let b = seed_project(&conn, "BBB");
        let p1 = create_plan(&conn, &CreatePlan { project_id: a, title: "a".into(), issue_id: None, steps: vec![] }).unwrap();
        let p2 = create_plan(&conn, &CreatePlan { project_id: b, title: "b".into(), issue_id: None, steps: vec![] }).unwrap();
        assert_eq!(p1.identifier, "AAA-PLAN-1");
        assert_eq!(p2.identifier, "BBB-PLAN-1");
        assert_eq!(resolve_plan_identifier(&conn, "AAA-PLAN-1").unwrap(), p1.id);
    }

    #[test]
    fn step_done_marks_linked_issue_done() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let issue = seed_issue(&conn, pid, "Real work", "todo");
        let plan = create_plan(
            &conn,
            &CreatePlan {
                project_id: pid,
                title: "Plan".into(),
                issue_id: None,
                steps: vec![simple_step("mirror step", Some(issue.id))],
            },
        )
        .unwrap();
        let step_id = plan.steps[0].id;

        let effect = set_step_done(&conn, step_id, true).unwrap();
        assert!(effect.issue_status_changed);
        assert_eq!(effect.issue_identifier.as_deref(), Some("TST-1"));
        let got = issues::get_issue(&conn, issue.id).unwrap();
        assert_eq!(got.status, "done");
    }

    #[test]
    fn issue_done_cascades_to_step_in_active_plan() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let issue = seed_issue(&conn, pid, "Work", "todo");
        let plan = create_plan(
            &conn,
            &CreatePlan {
                project_id: pid,
                title: "Plan".into(),
                issue_id: None,
                steps: vec![simple_step("mirror", Some(issue.id))],
            },
        )
        .unwrap();
        let step_id = plan.steps[0].id;

        issues::update_issue(
            &conn,
            issue.id,
            &UpdateIssue {
                status: Some("done".into()),
                title: None,
                description: None,
                priority: None,
                module_id: None,
                sort_order: None,
                start_date: None,
                target_date: None,
                labels: None,
            },
        )
        .unwrap();

        let refreshed = get_plan(&conn, plan.id).unwrap();
        assert!(refreshed.steps[0].done, "issue close should complete the step");
        let _ = step_id;
    }

    #[test]
    fn issue_reopen_uncompletes_step_and_stamps_reason() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let issue = seed_issue(&conn, pid, "Work", "done");
        let plan = create_plan(
            &conn,
            &CreatePlan {
                project_id: pid,
                title: "Plan".into(),
                issue_id: None,
                steps: vec![CreatePlanStep {
                    title: "mirror".into(),
                    description: String::new(),
                    issue_id: Some(issue.id),
                    done: true,
                    steps: vec![],
                }],
            },
        )
        .unwrap();

        // Reopen the issue.
        issues::update_issue(
            &conn,
            issue.id,
            &UpdateIssue {
                status: Some("active".into()),
                title: None,
                description: None,
                priority: None,
                module_id: None,
                sort_order: None,
                start_date: None,
                target_date: None,
                labels: None,
            },
        )
        .unwrap();

        let refreshed = get_plan(&conn, plan.id).unwrap();
        assert!(!refreshed.steps[0].done, "reopen should un-complete the step");
        assert!(
            refreshed.steps[0].reopened_via_issue_at.is_some(),
            "reopen should stamp the reason"
        );
    }

    #[test]
    fn frozen_plan_is_immune_to_cascade() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let issue = seed_issue(&conn, pid, "Work", "todo");
        let plan = create_plan(
            &conn,
            &CreatePlan {
                project_id: pid,
                title: "Plan".into(),
                issue_id: None,
                steps: vec![simple_step("mirror", Some(issue.id))],
            },
        )
        .unwrap();
        // Archive the plan → frozen.
        update_plan(&conn, plan.id, &UpdatePlan { status: Some("archived".into()), ..Default::default() }).unwrap();

        issues::update_issue(
            &conn,
            issue.id,
            &UpdateIssue {
                status: Some("done".into()),
                title: None, description: None, priority: None, module_id: None,
                sort_order: None, start_date: None, target_date: None, labels: None,
            },
        )
        .unwrap();

        let refreshed = get_plan(&conn, plan.id).unwrap();
        assert!(!refreshed.steps[0].done, "archived plan steps must not cascade");
    }

    #[test]
    fn anchor_issue_done_archives_plan() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let anchor = seed_issue(&conn, pid, "Epic", "active");
        let plan = create_plan(
            &conn,
            &CreatePlan {
                project_id: pid,
                title: "Plan".into(),
                issue_id: Some(anchor.id),
                steps: vec![],
            },
        )
        .unwrap();
        assert_eq!(plan.status, "active");

        issues::update_issue(
            &conn,
            anchor.id,
            &UpdateIssue {
                status: Some("done".into()),
                title: None, description: None, priority: None, module_id: None,
                sort_order: None, start_date: None, target_date: None, labels: None,
            },
        )
        .unwrap();

        let refreshed = get_plan(&conn, plan.id).unwrap();
        assert_eq!(refreshed.status, "archived");
    }

    #[test]
    fn marking_plan_done_does_not_close_anchor_issue() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let anchor = seed_issue(&conn, pid, "Epic", "active");
        let plan = create_plan(&conn, &CreatePlan { project_id: pid, title: "P".into(), issue_id: Some(anchor.id), steps: vec![] }).unwrap();

        update_plan(&conn, plan.id, &UpdatePlan { status: Some("done".into()), ..Default::default() }).unwrap();

        let got = issues::get_issue(&conn, anchor.id).unwrap();
        assert_eq!(got.status, "active", "plan completion must not close the anchor issue");
    }

    #[test]
    fn add_edit_move_delete_step() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let plan = create_plan(&conn, &CreatePlan { project_id: pid, title: "P".into(), issue_id: None, steps: vec![simple_step("root", None)] }).unwrap();
        let root_id = plan.steps[0].id;

        let child = add_step(&conn, plan.id, Some(root_id), "child", "desc", None).unwrap();
        edit_step_text(&conn, child, "title", "child", "renamed", false).unwrap();
        let after_edit = get_plan(&conn, plan.id).unwrap();
        assert_eq!(after_edit.steps[0].children[0].title, "renamed");

        // Cycle guard: cannot move root under its own child.
        let err = move_step(&conn, root_id, Some(child), None).unwrap_err();
        assert!(matches!(err, LificError::BadRequest(_)));

        // Delete root cascades the child.
        delete_step(&conn, root_id).unwrap();
        let empty = get_plan(&conn, plan.id).unwrap();
        assert_eq!(empty.step_count, 0);
    }

    #[test]
    fn delete_plan_removes_steps() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let plan = create_plan(&conn, &CreatePlan { project_id: pid, title: "P".into(), issue_id: None, steps: vec![simple_step("s", None)] }).unwrap();
        delete_plan(&conn, plan.id).unwrap();
        assert!(get_plan(&conn, plan.id).is_err());
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM plan_steps WHERE plan_id = ?1", params![plan.id], |r| r.get(0)).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn list_plans_filters_by_status_with_counts() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let p = create_plan(&conn, &CreatePlan { project_id: pid, title: "Active".into(), issue_id: None, steps: vec![simple_step("a", None), simple_step("b", None)] }).unwrap();
        set_step_done(&conn, p.steps[0].id, true).unwrap();
        let archived = create_plan(&conn, &CreatePlan { project_id: pid, title: "Old".into(), issue_id: None, steps: vec![] }).unwrap();
        update_plan(&conn, archived.id, &UpdatePlan { status: Some("archived".into()), ..Default::default() }).unwrap();

        let active = list_plans(&conn, &ListPlansQuery { project_id: Some(pid), status: Some("active".into()), ..Default::default() }).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].title, "Active");
        assert_eq!(active[0].step_count, 2);
        assert_eq!(active[0].done_count, 1);
    }

    // Guards the page-then-aggregate rewrite: a stepless plan must report
    // step_count = 0. With COUNT(*) the LEFT JOIN's NULL row would wrongly
    // count as 1; COUNT(s.id) ignores it. SUM(done) over no rows is NULL,
    // so COALESCE keeps done_count = 0.
    #[test]
    fn list_plans_stepless_reports_zero_counts() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        create_plan(&conn, &CreatePlan { project_id: pid, title: "Empty".into(), issue_id: None, steps: vec![] }).unwrap();

        let plans = list_plans(&conn, &ListPlansQuery { project_id: Some(pid), ..Default::default() }).unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].step_count, 0, "stepless plan must count 0, not 1");
        assert_eq!(plans[0].done_count, 0);
    }

    // The rewrite pages plans in an inner query, then re-orders after the
    // step join — ordering must survive that round-trip.
    #[test]
    fn list_plans_orders_newest_first_and_pages() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        for i in 0..5 {
            create_plan(&conn, &CreatePlan { project_id: pid, title: format!("Plan {i}"), issue_id: None, steps: vec![simple_step("s", None)] }).unwrap();
        }
        let page = list_plans(&conn, &ListPlansQuery { project_id: Some(pid), status: None, limit: Some(2), offset: Some(0) }).unwrap();
        assert_eq!(page.len(), 2);
        // Newest (highest id / latest updated_at) first.
        assert_eq!(page[0].title, "Plan 4");
        assert_eq!(page[1].title, "Plan 3");
        assert_eq!(page[0].step_count, 1);

        let next = list_plans(&conn, &ListPlansQuery { project_id: Some(pid), status: None, limit: Some(2), offset: Some(2) }).unwrap();
        assert_eq!(next.len(), 2);
        assert_eq!(next[0].title, "Plan 2");
    }

    // ── Audit coverage (LIF-176) ──

    fn audit_rows(conn: &Connection, entity_type: &str) -> Vec<(String, Option<String>, Option<String>)> {
        let mut stmt = conn
            .prepare("SELECT action, field, new_value FROM audit_log WHERE entity_type = ?1 ORDER BY id")
            .unwrap();
        stmt.query_map(params![entity_type], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    }

    #[test]
    fn plan_and_step_mutations_are_audited() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let plan = create_plan(&conn, &CreatePlan { project_id: pid, title: "P".into(), issue_id: None, steps: vec![simple_step("s", None)] }).unwrap();
        let step_id = plan.steps[0].id;
        set_step_done(&conn, step_id, true).unwrap();

        let plan_actions: Vec<String> = audit_rows(&conn, "plan").into_iter().map(|r| r.0).collect();
        assert!(plan_actions.contains(&"create".to_string()), "plan create must be audited: {plan_actions:?}");

        let step_audit = audit_rows(&conn, "plan_step");
        assert!(step_audit.iter().any(|(a, _, _)| a == "create"), "step create audited");
        assert!(
            step_audit.iter().any(|(a, f, _)| a == "update" && f.as_deref() == Some("done")),
            "step done audited: {step_audit:?}"
        );
    }

    #[test]
    fn cascade_auto_complete_is_audited_via_issue() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let issue = seed_issue(&conn, pid, "Work", "todo");
        create_plan(&conn, &CreatePlan { project_id: pid, title: "P".into(), issue_id: None, steps: vec![simple_step("mirror", Some(issue.id))] }).unwrap();

        issues::update_issue(&conn, issue.id, &UpdateIssue {
            status: Some("done".into()), title: None, description: None, priority: None,
            module_id: None, sort_order: None, start_date: None, target_date: None, labels: None,
        }).unwrap();

        let step_audit = audit_rows(&conn, "plan_step");
        assert!(
            step_audit.iter().any(|(a, _, _)| a == "auto-complete"),
            "issue-driven cascade must be audited as auto-complete: {step_audit:?}"
        );
    }

    // ── Coverage backfill: previously-untested step mutators & guards ──

    /// assert_step_in_plan is an integrity guard: it must accept a step that
    /// belongs to the plan and reject one that doesn't, so a caller can't
    /// mutate a step via the wrong plan's identifier.
    #[test]
    fn assert_step_in_plan_accepts_own_rejects_foreign() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let plan_a = create_plan(&conn, &CreatePlan { project_id: pid, title: "A".into(), issue_id: None, steps: vec![simple_step("a-step", None)] }).unwrap();
        let plan_b = create_plan(&conn, &CreatePlan { project_id: pid, title: "B".into(), issue_id: None, steps: vec![simple_step("b-step", None)] }).unwrap();
        let a_step = plan_a.steps[0].id;
        let b_step = plan_b.steps[0].id;

        assert!(assert_step_in_plan(&conn, plan_a.id, a_step).is_ok());
        // a_step belongs to plan_a, not plan_b → BadRequest.
        let err = assert_step_in_plan(&conn, plan_b.id, a_step).unwrap_err();
        assert!(matches!(err, LificError::BadRequest(_)), "foreign step must be rejected, got {err:?}");
        assert!(assert_step_in_plan(&conn, plan_b.id, b_step).is_ok());
    }

    #[test]
    fn set_step_title_renames_and_404s_on_missing() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let plan = create_plan(&conn, &CreatePlan { project_id: pid, title: "P".into(), issue_id: None, steps: vec![simple_step("old title", None)] }).unwrap();
        let step_id = plan.steps[0].id;

        set_step_title(&conn, step_id, "new title").unwrap();
        let reread = get_plan(&conn, plan.id).unwrap();
        assert_eq!(reread.steps[0].title, "new title");

        let err = set_step_title(&conn, 999_999, "ghost").unwrap_err();
        assert!(matches!(err, LificError::NotFound(_)), "missing step must 404, got {err:?}");
    }

    // set_step_description runs unescape_text (LIF-177): literal \n from JSON
    // transport must land as a real newline in the stored body.
    #[test]
    fn set_step_description_sets_body_and_unescapes() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let plan = create_plan(&conn, &CreatePlan { project_id: pid, title: "P".into(), issue_id: None, steps: vec![simple_step("s", None)] }).unwrap();
        let step_id = plan.steps[0].id;

        set_step_description(&conn, step_id, "line one\\nline two").unwrap();
        let reread = get_plan(&conn, plan.id).unwrap();
        assert_eq!(reread.steps[0].description, "line one\nline two", "\\n must be unescaped to a newline");

        let err = set_step_description(&conn, 999_999, "x").unwrap_err();
        assert!(matches!(err, LificError::NotFound(_)));
    }

    #[test]
    fn set_step_issue_links_and_unlinks() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let issue = seed_issue(&conn, pid, "Linkable", "todo");
        let plan = create_plan(&conn, &CreatePlan { project_id: pid, title: "P".into(), issue_id: None, steps: vec![simple_step("s", None)] }).unwrap();
        let step_id = plan.steps[0].id;

        set_step_issue(&conn, step_id, Some(issue.id)).unwrap();
        assert_eq!(get_plan(&conn, plan.id).unwrap().steps[0].issue_id, Some(issue.id));

        // Passing None must clear the link back to NULL.
        set_step_issue(&conn, step_id, None).unwrap();
        assert_eq!(get_plan(&conn, plan.id).unwrap().steps[0].issue_id, None);

        let err = set_step_issue(&conn, 999_999, Some(issue.id)).unwrap_err();
        assert!(matches!(err, LificError::NotFound(_)));
    }

    #[test]
    fn step_parent_reflects_nesting() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let pid = seed_project(&conn, "TST");
        let plan = create_plan(
            &conn,
            &CreatePlan {
                project_id: pid,
                title: "P".into(),
                issue_id: None,
                steps: vec![CreatePlanStep {
                    title: "parent".into(),
                    description: String::new(),
                    issue_id: None,
                    done: false,
                    steps: vec![simple_step("child", None)],
                }],
            },
        )
        .unwrap();
        let parent_id = plan.steps[0].id;
        let child_id = plan.steps[0].children[0].id;

        assert_eq!(step_parent(&conn, parent_id).unwrap(), None, "root step has no parent");
        assert_eq!(step_parent(&conn, child_id).unwrap(), Some(parent_id));

        let err = step_parent(&conn, 999_999).unwrap_err();
        assert!(matches!(err, LificError::NotFound(_)));
    }
}
