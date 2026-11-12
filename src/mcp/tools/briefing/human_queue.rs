//! LIF-491: the briefing's first section, the human queue across every
//! project the caller can view. Agents resuming one project otherwise never
//! see the person-only work holding theirs up elsewhere.

use super::*;

impl LificMcp {
    pub(super) fn human_queue_section(
        &self,
        context: Option<&IssueLinkContext>,
    ) -> Result<Option<Section>, String> {
        let visible = visible_project_ids_mcp(&self.db)?;
        let queue = self.read(|conn| queries::human_queue::human_queue(conn, visible.as_ref()))?;
        if queue.items.is_empty() {
            return Ok(None);
        }

        let mut per_project: Vec<(&str, usize)> = Vec::new();
        for item in &queue.items {
            let ident = item
                .identifier
                .rsplit_once('-')
                .map_or(item.identifier.as_str(), |(project, _)| project);
            match per_project.iter_mut().find(|(name, _)| *name == ident) {
                Some((_, n)) => *n += 1,
                None => per_project.push((ident, 1)),
            }
        }
        per_project.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        let by_project = per_project
            .iter()
            .map(|(name, n)| format!("{name} {n}"))
            .collect::<Vec<_>>()
            .join(", ");

        let (lines, items) = queue
            .items
            .iter()
            .take(ISSUE_LINES)
            .map(|item| {
                let reference = issue_reference(context, &item.identifier);
                let mut line = format!(
                    "{} | {} | {} | {} | for {}",
                    reference,
                    item.status,
                    item.priority,
                    title(&item.title),
                    item.assignees
                );
                if item.blocks > 0 {
                    let _ = write!(line, " | blocks {}", item.blocks);
                }
                if let Some(due) = &item.target_date {
                    let _ = write!(line, " | due {due}");
                }
                let opened = item.created_at.get(..10).unwrap_or(&item.created_at);
                let _ = write!(line, " | opened {opened}");
                if !item.duplicates.is_empty() {
                    let others = item
                        .duplicates
                        .iter()
                        .map(|other| issue_reference(context, other).to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    let _ = write!(line, " | possible duplicate of {others}");
                }
                let structured = super::item(
                    "issue",
                    Some(reference),
                    &item.identifier,
                    Some(&item.title),
                    Some(&item.status.to_string()),
                );
                (line, structured)
            })
            .unzip();

        let total = queue.items.len();
        Ok(Some(Section {
            key: "human_queue",
            items,
            heading: format!(
                "Human queue ({}), every project",
                count(total, queue.truncated)
            ),
            summary: Some(format!(
                "Open issues assigned to human or to named people, most open issues blocked first, then due date, then oldest. By project: {by_project}."
            )),
            lines,
            total,
            total_is_floor: queue.truncated,
            more: "list_issues(project='<PROJECT>', assignee='human') per project".into(),
        }))
    }
}
