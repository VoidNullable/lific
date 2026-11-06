//! LIF-491: the human queue. Open issues that need a person (marked `human`
//! or naming people), across every project the caller can see, ranked so
//! the item holding up the most work comes first.
//!
//! Computed from live rows at call time, like the rest of the briefing.

use std::collections::{HashMap, HashSet};

use rusqlite::Connection;

use crate::error::LificError;

/// Most rows one read ranks. Further items are counted as a floor.
pub const SCAN_LIMIT: usize = 500;

/// One open issue waiting on a person.
#[derive(Debug, Clone)]
pub struct HumanQueueItem {
    pub id: i64,
    pub identifier: String,
    pub title: String,
    pub status: String,
    pub priority: String,
    pub target_date: Option<String>,
    pub created_at: String,
    /// Open, live issues this one blocks, in any project.
    pub blocks: i64,
    /// `human`, or the named people as `@alice, @bob`.
    pub assignees: String,
    /// Other queue items with the same title, ignoring case and spacing.
    pub duplicates: Vec<String>,
}

#[derive(Debug, Default)]
pub struct HumanQueue {
    /// Ranked: most open issues blocked, then earliest target date (undated
    /// last), then oldest.
    pub items: Vec<HumanQueueItem>,
    /// The scan stopped at [`SCAN_LIMIT`], so there may be more.
    pub truncated: bool,
}

/// Lowercased, with runs of whitespace collapsed, so titles that differ only
/// in case or spacing compare equal.
fn normalized_title(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The human queue across `visible` projects (`None` = every project).
pub fn human_queue(
    conn: &Connection,
    visible: Option<&HashSet<i64>>,
) -> Result<HumanQueue, LificError> {
    let scope = match visible {
        None => String::new(),
        Some(ids) if ids.is_empty() => return Ok(HumanQueue::default()),
        Some(ids) => {
            let list = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
            format!("AND i.project_id IN ({list})")
        }
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT i.id, p.identifier || '-' || i.sequence, i.title,
                i.status, i.priority, i.target_date, i.created_at,
                (SELECT COUNT(*) FROM issue_relations ir
                   JOIN issues t ON t.id = ir.target_id
                  WHERE ir.source_id = i.id AND ir.relation_type = 'blocks'
                    AND t.deleted_at IS NULL
                    AND t.status NOT IN ('done', 'cancelled')) AS blocks
           FROM issues i JOIN projects p ON p.id = i.project_id
          WHERE i.deleted_at IS NULL
            AND i.status NOT IN ('done', 'cancelled')
            AND EXISTS (SELECT 1 FROM issue_assignees a WHERE a.issue_id = i.id)
            {scope}
          ORDER BY blocks DESC, i.target_date IS NULL, i.target_date,
                   i.created_at, i.id
          LIMIT {}",
        SCAN_LIMIT + 1
    ))?;
    let mut items = stmt
        .query_map([], |row| {
            Ok(HumanQueueItem {
                id: row.get(0)?,
                identifier: row.get(1)?,
                title: row.get(2)?,
                status: row.get(3)?,
                priority: row.get(4)?,
                target_date: row.get(5)?,
                created_at: row.get(6)?,
                blocks: row.get(7)?,
                assignees: String::new(),
                duplicates: Vec::new(),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let truncated = items.len() > SCAN_LIMIT;
    items.truncate(SCAN_LIMIT);

    let ids: Vec<i64> = items.iter().map(|item| item.id).collect();
    let assignments = super::assignees::assignments_by_issue(conn, &ids)?;
    let mut by_title: HashMap<String, Vec<String>> = HashMap::new();
    for item in &items {
        by_title
            .entry(normalized_title(&item.title))
            .or_default()
            .push(item.identifier.clone());
    }
    for item in &mut items {
        item.assignees = assignments
            .get(&item.id)
            .and_then(|a| super::assignees::describe(a.needs_human, &a.assignees))
            .unwrap_or_else(|| super::assignees::HUMAN.to_string());
        item.duplicates = by_title[&normalized_title(&item.title)]
            .iter()
            .filter(|other| **other != item.identifier)
            .cloned()
            .collect();
    }
    Ok(HumanQueue { items, truncated })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_differing_only_in_case_and_spacing_normalize_alike() {
        assert_eq!(
            normalized_title("  Renew   the Domain "),
            normalized_title("renew the domain")
        );
        assert_ne!(
            normalized_title("Renew domain"),
            normalized_title("Renew domains")
        );
    }
}
