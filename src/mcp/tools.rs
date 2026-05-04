use std::sync::Arc;

use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};

use crate::db::{DbPool, models, queries};

use super::LificMcp;
use super::schemas::*;

impl LificMcp {
    pub(crate) fn create_tool_router() -> rmcp::handler::server::router::tool::ToolRouter<Self> {
        Self::tool_router()
    }
}

pub(crate) fn fmt_issue(i: &models::Issue) -> String {
    let mut s = format!(
        "{} | {} | {} | {}",
        i.identifier, i.status, i.priority, i.title
    );
    if !i.labels.is_empty() {
        s.push_str(&format!(" [{}]", i.labels.join(", ")));
    }
    if !i.blocks.is_empty() {
        s.push_str(&format!(" blocks:{}", i.blocks.join(",")));
    }
    if !i.blocked_by.is_empty() {
        s.push_str(&format!(" blocked_by:{}", i.blocked_by.join(",")));
    }
    s
}

fn resolve_project(db: &Arc<DbPool>, ident: &str) -> Result<i64, String> {
    let conn = db.read().map_err(|e| e.to_string())?;
    queries::resolve_project_identifier(&conn, ident).map_err(|e| e.to_string())
}

fn resolve_module(db: &Arc<DbPool>, project_id: i64, name: &str) -> Result<i64, String> {
    let conn = db.read().map_err(|e| e.to_string())?;
    queries::resolve_module_name(&conn, project_id, name).map_err(|e| e.to_string())
}

fn resolve_folder(db: &Arc<DbPool>, project_id: i64, name: &str) -> Result<i64, String> {
    let conn = db.read().map_err(|e| e.to_string())?;
    queries::resolve_folder_name(&conn, project_id, name).map_err(|e| e.to_string())
}

/// Append a paging hint to `out` if `has_more` is true.
/// `next_offset` is the offset the agent should use to fetch the next page.
fn append_pagination_hint(out: &mut String, has_more: bool, next_offset: i64) {
    if has_more {
        out.push_str(&format!(
            "\n... more results available — call again with offset={next_offset}\n"
        ));
    }
}

#[tool_router]
impl LificMcp {
    #[tool(description = "Search across all issues and pages by text")]
    fn search(&self, Parameters(input): Parameters<SearchInput>) -> String {
        let project_id = match &input.project {
            Some(p) => match resolve_project(&self.db, p) {
                Ok(id) => Some(id),
                Err(e) => return format!("Error: {e}"),
            },
            None => None,
        };
        match self.read(|conn| {
            queries::search(
                conn,
                &models::SearchQuery {
                    query: input.query.clone(),
                    project_id,
                    limit: input.limit,
                },
            )
        }) {
            Ok(results) if results.is_empty() => "No results found.".into(),
            Ok(results) => {
                let mut out = format!("{} results:\n", results.len());
                for r in &results {
                    let ident = r.identifier.as_deref().unwrap_or("");
                    out.push_str(&format!(
                        "- [{}] {} {} — {}\n",
                        r.result_type, ident, r.title, r.snippet
                    ));
                }
                out
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(
        description = "List issues for a project. Use workable=true for issues with no unresolved blockers."
    )]
    fn list_issues(&self, Parameters(input): Parameters<ListIssuesInput>) -> String {
        let pid = match resolve_project(&self.db, &input.project) {
            Ok(id) => id,
            Err(e) => return format!("Error: {e}"),
        };
        let module_id = match &input.module {
            Some(name) => match resolve_module(&self.db, pid, name) {
                Ok(id) => Some(id),
                Err(e) => return format!("Error: {e}"),
            },
            None => None,
        };
        let limit = input.limit.unwrap_or(50).max(1);
        let offset = input.offset.unwrap_or(0).max(0);
        // Over-fetch by one to detect whether more results exist beyond this page.
        match self.read(|conn| {
            queries::list_issues(
                conn,
                &models::ListIssuesQuery {
                    project_id: Some(pid),
                    status: input.status.clone(),
                    priority: input.priority.clone(),
                    module_id,
                    label: input.label.clone(),
                    workable: input.workable,
                    limit: Some(limit + 1),
                    offset: Some(offset),
                },
            )
        }) {
            Ok(issues) if issues.is_empty() => "No issues found.".into(),
            Ok(mut issues) => {
                let has_more = issues.len() as i64 > limit;
                if has_more {
                    issues.truncate(limit as usize);
                }
                let mut out = format!("{} issues:\n", issues.len());
                for i in &issues {
                    out.push_str(&format!("- {}\n", fmt_issue(i)));
                }
                append_pagination_hint(&mut out, has_more, offset + limit);
                out
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(
        description = "Get a single issue by identifier (e.g. LIF-1). Returns full details with relations."
    )]
    fn get_issue(&self, Parameters(input): Parameters<GetIssueInput>) -> String {
        match self.read(|conn| {
            let id = queries::resolve_identifier(conn, &input.identifier)?;
            let issue = queries::get_issue(conn, id)?;
            let module_name = match issue.module_id {
                Some(mid) => queries::get_module_name(conn, mid).unwrap_or("unknown".into()),
                None => "none".into(),
            };
            Ok((issue, module_name))
        }) {
            Ok((issue, module_name)) => {
                let mut out = format!(
                    "{} — {}\nStatus: {} | Priority: {} | Module: {}\n",
                    issue.identifier, issue.title, issue.status, issue.priority, module_name
                );
                if !issue.labels.is_empty() {
                    out.push_str(&format!("Labels: {}\n", issue.labels.join(", ")));
                }
                if !issue.blocks.is_empty() {
                    out.push_str(&format!("Blocks: {}\n", issue.blocks.join(", ")));
                }
                if !issue.blocked_by.is_empty() {
                    out.push_str(&format!("Blocked by: {}\n", issue.blocked_by.join(", ")));
                }
                if !issue.relates_to.is_empty() {
                    out.push_str(&format!("Relates to: {}\n", issue.relates_to.join(", ")));
                }
                if !issue.description.is_empty() {
                    out.push_str(&format!("\n{}\n", issue.description));
                }
                // Include comments
                if let Ok(comments) =
                    self.read(|conn| queries::comments::list_comments(conn, issue.id))
                    && !comments.is_empty()
                {
                    out.push_str(&format!("\n--- Comments ({}) ---\n", comments.len()));
                    for c in &comments {
                        out.push_str(&format!(
                            "[{}] {} ({}): {}\n",
                            c.created_at, c.author, c.author_display_name, c.content
                        ));
                    }
                }
                out
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Export a single issue as markdown. Returns the markdown content.")]
    fn export_issue(&self, Parameters(input): Parameters<ExportIssueInput>) -> String {
        match self.read(|conn| crate::export::export_issue(conn, &input.identifier)) {
            Ok(bundle) => bundle
                .files
                .into_iter()
                .next()
                .map(|file| file.content)
                .unwrap_or_else(|| "Error: issue export produced no files".into()),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Create a new issue in a project")]
    fn create_issue(&self, Parameters(input): Parameters<CreateIssueInput>) -> String {
        let pid = match resolve_project(&self.db, &input.project) {
            Ok(id) => id,
            Err(e) => return format!("Error: {e}"),
        };
        let module_id = match &input.module {
            Some(name) => match resolve_module(&self.db, pid, name) {
                Ok(id) => Some(id),
                Err(e) => return format!("Error: {e}"),
            },
            None => None,
        };
        match self.write(|conn| {
            queries::create_issue(
                conn,
                &models::CreateIssue {
                    project_id: pid,
                    title: input.title.clone(),
                    description: input.description.clone().unwrap_or_default(),
                    status: input.status.clone().unwrap_or("backlog".into()),
                    priority: input.priority.clone().unwrap_or("none".into()),
                    module_id,
                    start_date: None,
                    target_date: None,
                    labels: input.labels.clone().unwrap_or_default(),
                },
            )
        }) {
            Ok(issue) => format!("Created {}: {}", issue.identifier, issue.title),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(
        description = "Update an existing issue by identifier. Only provided fields are changed."
    )]
    fn update_issue(&self, Parameters(input): Parameters<UpdateIssueInput>) -> String {
        match self.write(|conn| {
            let id = queries::resolve_identifier(conn, &input.identifier)?;
            let module_id = match &input.module {
                Some(name) => {
                    let issue = queries::get_issue(conn, id)?;
                    Some(queries::resolve_module_name(conn, issue.project_id, name)?)
                }
                None => None,
            };
            queries::update_issue(
                conn,
                id,
                &models::UpdateIssue {
                    title: input.title.clone(),
                    description: input.description.clone(),
                    status: input.status.clone(),
                    priority: input.priority.clone(),
                    module_id,
                    sort_order: None,
                    start_date: None,
                    target_date: None,
                    labels: input.labels.clone(),
                },
            )
        }) {
            Ok(issue) => format!("Updated {}: {}", issue.identifier, fmt_issue(&issue)),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Get board view of issues grouped by status, priority, or module")]
    fn get_board(&self, Parameters(input): Parameters<GetBoardInput>) -> String {
        let pid = match resolve_project(&self.db, &input.project) {
            Ok(id) => id,
            Err(e) => return format!("Error: {e}"),
        };
        const BOARD_CAP: i64 = 500;
        match self.read(|conn| {
            queries::list_issues(
                conn,
                &models::ListIssuesQuery {
                    project_id: Some(pid),
                    status: None,
                    priority: None,
                    module_id: None,
                    label: None,
                    workable: None,
                    // Over-fetch by one to detect truncation.
                    limit: Some(BOARD_CAP + 1),
                    offset: None,
                },
            )
        }) {
            Ok(mut issues) => {
                let truncated = issues.len() as i64 > BOARD_CAP;
                if truncated {
                    issues.truncate(BOARD_CAP as usize);
                }
                let group_by = input.group_by.as_deref().unwrap_or("status");
                let module_names: std::collections::HashMap<i64, String> = if group_by == "module" {
                    if let Ok(conn) = self.db.read() {
                        queries::list_modules(&conn, pid)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|m| (m.id, m.name))
                            .collect()
                    } else {
                        std::collections::HashMap::new()
                    }
                } else {
                    std::collections::HashMap::new()
                };
                let mut groups: std::collections::BTreeMap<String, Vec<&models::Issue>> =
                    std::collections::BTreeMap::new();
                for issue in &issues {
                    let key = match group_by {
                        "priority" => issue.priority.clone(),
                        "module" => issue
                            .module_id
                            .and_then(|m| module_names.get(&m).cloned())
                            .unwrap_or("unassigned".into()),
                        _ => issue.status.clone(),
                    };
                    groups.entry(key).or_default().push(issue);
                }
                let mut out = String::new();
                if truncated {
                    out.push_str(&format!(
                        "warning: board view capped at {BOARD_CAP} issues — older issues are not shown. Use list_issues with offset for full paging.\n\n"
                    ));
                }
                for (group, items) in &groups {
                    out.push_str(&format!("── {} ({}) ──\n", group, items.len()));
                    for i in items {
                        out.push_str(&format!("  {}\n", fmt_issue(i)));
                    }
                    out.push('\n');
                }
                out
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Link two issues with a relation: blocks, relates_to, or duplicate")]
    fn link_issues(&self, Parameters(input): Parameters<LinkIssuesInput>) -> String {
        match self.write(|conn| {
            let source_id = queries::resolve_identifier(conn, &input.source)?;
            let target_id = queries::resolve_identifier(conn, &input.target)?;
            queries::link_issues(conn, source_id, target_id, &input.relation_type)
        }) {
            Ok(()) => format!("{} {} {}", input.source, input.relation_type, input.target),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Remove a relation between two issues")]
    fn unlink_issues(&self, Parameters(input): Parameters<UnlinkIssuesInput>) -> String {
        match self.write(|conn| {
            let source_id = queries::resolve_identifier(conn, &input.source)?;
            let target_id = queries::resolve_identifier(conn, &input.target)?;
            queries::unlink_issues(conn, source_id, target_id)
        }) {
            Ok(()) => format!("Unlinked {} and {}", input.source, input.target),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Get a page by identifier (e.g. LIF-DOC-1). Returns full content.")]
    fn get_page(&self, Parameters(input): Parameters<GetPageInput>) -> String {
        match self.read(|conn| {
            let id = queries::resolve_page_identifier(conn, &input.identifier)?;
            queries::get_page(conn, id)
        }) {
            Ok(page) => {
                let mut out = format!("{} — {}\n", page.identifier, page.title);
                if !page.content.is_empty() {
                    out.push_str(&format!("\n{}\n", page.content));
                }
                out
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Export a single page as markdown. Returns the markdown content.")]
    fn export_page(&self, Parameters(input): Parameters<ExportPageInput>) -> String {
        match self.read(|conn| crate::export::export_page(conn, &input.identifier)) {
            Ok(bundle) => bundle
                .files
                .into_iter()
                .next()
                .map(|file| file.content)
                .unwrap_or_else(|| "Error: page export produced no files".into()),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Export an entire project as markdown. Returns exported file paths.")]
    fn export_project(&self, Parameters(input): Parameters<ExportProjectInput>) -> String {
        match self.read(|conn| crate::export::export_project(conn, &input.project)) {
            Ok(bundle) => {
                let mut out = format!("{} exported file(s):\n", bundle.files.len());
                for file in bundle.files {
                    out.push_str(&format!("- {}\n", file.path));
                }
                out
            }
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Create a new page in a project")]
    fn create_page(&self, Parameters(input): Parameters<CreatePageInput>) -> String {
        let project_id = match &input.project {
            Some(p) => match resolve_project(&self.db, p) {
                Ok(id) => Some(id),
                Err(e) => return format!("Error: {e}"),
            },
            None => None,
        };
        let folder_id = match (&input.folder, project_id) {
            (Some(name), Some(pid)) => match resolve_folder(&self.db, pid, name) {
                Ok(id) => Some(id),
                Err(e) => return format!("Error: {e}"),
            },
            (Some(_), None) => return "Error: folder requires a project".into(),
            _ => None,
        };
        match self.write(|conn| {
            queries::create_page(
                conn,
                &models::CreatePage {
                    project_id,
                    folder_id,
                    title: input.title.clone(),
                    content: input.content.clone().unwrap_or_default(),
                },
            )
        }) {
            Ok(page) => format!("Created {}: {}", page.identifier, page.title),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Update a page by identifier. Only provided fields are changed.")]
    fn update_page(&self, Parameters(input): Parameters<UpdatePageInput>) -> String {
        match self.write(|conn| {
            let id = queries::resolve_page_identifier(conn, &input.identifier)?;
            let folder_id = match &input.folder {
                Some(name) => {
                    let page = queries::get_page(conn, id)?;
                    let pid = page.project_id.ok_or_else(|| {
                        crate::error::LificError::BadRequest(
                            "page has no project for folder resolution".into(),
                        )
                    })?;
                    Some(queries::resolve_folder_name(conn, pid, name)?)
                }
                None => None,
            };
            queries::update_page(
                conn,
                id,
                &models::UpdatePage {
                    title: input.title.clone(),
                    content: input.content.clone(),
                    folder_id: folder_id.map(Some),
                    sort_order: None,
                },
            )
        }) {
            Ok(page) => format!("Updated {}: {}", page.identifier, page.title),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(
        description = "Delete any resource by type and identifier. Types: issue, page, project, module, label, folder."
    )]
    fn delete(&self, Parameters(input): Parameters<DeleteInput>) -> String {
        match input.resource_type.as_str() {
            "issue" => match self.write(|conn| {
                let id = queries::resolve_identifier(conn, &input.identifier)?;
                queries::delete_issue(conn, id)
            }) {
                Ok(()) => format!("Deleted issue {}", input.identifier),
                Err(e) => format!("Error: {e}"),
            },
            "page" => match self.write(|conn| {
                let id = queries::resolve_page_identifier(conn, &input.identifier)?;
                queries::delete_page(conn, id)
            }) {
                Ok(()) => format!("Deleted page {}", input.identifier),
                Err(e) => format!("Error: {e}"),
            },
            "project" => match self.write(|conn| {
                let id = queries::resolve_project_identifier(conn, &input.identifier)?;
                queries::delete_project(conn, id)
            }) {
                Ok(()) => format!("Deleted project {}", input.identifier),
                Err(e) => format!("Error: {e}"),
            },
            "module" | "label" | "folder" => {
                let Some(ref proj) = input.project else {
                    return format!(
                        "Error: project required to delete {} by name",
                        input.resource_type
                    );
                };
                let pid = match resolve_project(&self.db, proj) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                let result = match input.resource_type.as_str() {
                    "module" => self.write(|conn| {
                        let id = queries::resolve_module_name(conn, pid, &input.identifier)?;
                        queries::delete_module(conn, id)
                    }),
                    "label" => self.write(|conn| {
                        let id = queries::resolve_label_name(conn, pid, &input.identifier)?;
                        queries::delete_label(conn, id)
                    }),
                    "folder" => self.write(|conn| {
                        let id = queries::resolve_folder_name(conn, pid, &input.identifier)?;
                        queries::delete_folder(conn, id)
                    }),
                    _ => unreachable!(),
                };
                match result {
                    Ok(()) => format!("Deleted {} '{}'", input.resource_type, input.identifier),
                    Err(e) => format!("Error: {e}"),
                }
            }
            other => format!(
                "Unknown type '{other}'. Use issue, page, project, module, label, or folder."
            ),
        }
    }

    #[tool(
        description = "List resources by type: project, module, label, folder, page, or issue. Most types need a project identifier."
    )]
    fn list_resources(&self, Parameters(input): Parameters<ListResourcesInput>) -> String {
        match input.resource_type.as_str() {
            "project" => match self.read(queries::list_projects) {
                Ok(ps) => {
                    let mut out = format!("{} projects:\n", ps.len());
                    for p in &ps {
                        out.push_str(&format!("- {} | {}", p.identifier, p.name));
                        if !p.description.is_empty() {
                            out.push_str(&format!(" — {}", p.description));
                        }
                        out.push('\n');
                    }
                    out
                }
                Err(e) => format!("Error: {e}"),
            },
            "issue" => {
                let Some(ref proj) = input.project else {
                    return "Error: project required".into();
                };
                let pid = match resolve_project(&self.db, proj) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                let limit = input.limit.unwrap_or(100).max(1);
                let offset = input.offset.unwrap_or(0).max(0);
                match self.read(|conn| {
                    queries::list_issues(
                        conn,
                        &models::ListIssuesQuery {
                            project_id: Some(pid),
                            status: None,
                            priority: None,
                            module_id: None,
                            label: None,
                            workable: None,
                            limit: Some(limit + 1),
                            offset: Some(offset),
                        },
                    )
                }) {
                    Ok(mut issues) => {
                        let has_more = issues.len() as i64 > limit;
                        if has_more {
                            issues.truncate(limit as usize);
                        }
                        let mut out =
                            format!("{} issues (use list_issues for filtering):\n", issues.len());
                        for i in &issues {
                            out.push_str(&format!(
                                "- {} | {} | {}\n",
                                i.identifier, i.status, i.title
                            ));
                        }
                        append_pagination_hint(&mut out, has_more, offset + limit);
                        out
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }
            "page" => {
                let project_id = match &input.project {
                    Some(p) => match resolve_project(&self.db, p) {
                        Ok(id) => Some(id),
                        Err(e) => return format!("Error: {e}"),
                    },
                    None => None,
                };
                let folder_id = match (&input.folder, project_id) {
                    (Some(name), Some(pid)) => match resolve_folder(&self.db, pid, name) {
                        Ok(id) => Some(id),
                        Err(e) => return format!("Error: {e}"),
                    },
                    _ => None,
                };
                match self.read(|conn| queries::list_pages(conn, project_id, folder_id)) {
                    Ok(pages) if pages.is_empty() => "No pages found.".into(),
                    Ok(pages) => {
                        let mut out = format!("{} pages:\n", pages.len());
                        for p in &pages {
                            out.push_str(&format!("- {} | {}\n", p.identifier, p.title));
                        }
                        out
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }
            "module" => {
                let Some(ref proj) = input.project else {
                    return "Error: project required".into();
                };
                let pid = match resolve_project(&self.db, proj) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                match self.read(|conn| queries::list_modules(conn, pid)) {
                    Ok(ms) => {
                        let mut out = format!("{} modules:\n", ms.len());
                        for m in &ms {
                            out.push_str(&format!("- {} ({})", m.name, m.status));
                            if !m.description.is_empty() {
                                out.push_str(&format!(" — {}", m.description));
                            }
                            out.push('\n');
                        }
                        out
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }
            "label" => {
                let Some(ref proj) = input.project else {
                    return "Error: project required".into();
                };
                let pid = match resolve_project(&self.db, proj) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                match self.read(|conn| queries::list_labels(conn, pid)) {
                    Ok(ls) => {
                        let mut out = format!("{} labels:\n", ls.len());
                        for l in &ls {
                            out.push_str(&format!("- {} ({})\n", l.name, l.color));
                        }
                        out
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }
            "folder" => {
                let Some(ref proj) = input.project else {
                    return "Error: project required".into();
                };
                let pid = match resolve_project(&self.db, proj) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                match self.read(|conn| queries::list_folders(conn, pid)) {
                    Ok(fs) => {
                        let mut out = format!("{} folders:\n", fs.len());
                        for f in &fs {
                            out.push_str(&format!("- [{}] {}\n", f.id, f.name));
                        }
                        out
                    }
                    Err(e) => format!("Error: {e}"),
                }
            }
            other => format!(
                "Unknown type '{other}'. Use project, module, label, folder, page, or issue."
            ),
        }
    }

    #[tool(
        description = "Create or update a resource (project, module, label, folder). Use the delete tool for deletion."
    )]
    fn manage_resource(&self, Parameters(input): Parameters<ManageResourceInput>) -> String {
        match (input.resource_type.as_str(), input.action.as_str()) {
            ("project", "create") => {
                let Some(ref name) = input.name else {
                    return "Error: name required".into();
                };
                let Some(ref ident) = input.identifier else {
                    return "Error: identifier required".into();
                };
                match self.write(|conn| {
                    queries::create_project(
                        conn,
                        &models::CreateProject {
                            name: name.clone(),
                            identifier: ident.clone(),
                            description: input.description.clone().unwrap_or_default(),
                            emoji: None,
                            lead_user_id: None,
                        },
                    )
                }) {
                    Ok(p) => format!("Created project {} | {}", p.identifier, p.name),
                    Err(e) => format!("Error: {e}"),
                }
            }
            ("project", "update") => {
                let Some(ref proj) = input.project else {
                    return "Error: project identifier required".into();
                };
                let pid = match resolve_project(&self.db, proj) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                match self.write(|conn| {
                    queries::update_project(
                        conn,
                        pid,
                        &models::UpdateProject {
                            name: input.name.clone(),
                            identifier: input.identifier.clone(),
                            description: input.description.clone(),
                            emoji: None,
                            lead_user_id: None,
                        },
                    )
                }) {
                    Ok(p) => format!("Updated project {} | {}", p.identifier, p.name),
                    Err(e) => format!("Error: {e}"),
                }
            }
            ("module", "create") => {
                let Some(ref proj) = input.project else {
                    return "Error: project required".into();
                };
                let pid = match resolve_project(&self.db, proj) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                let Some(ref name) = input.name else {
                    return "Error: name required".into();
                };
                match self.write(|conn| {
                    queries::create_module(
                        conn,
                        &models::CreateModule {
                            project_id: pid,
                            name: name.clone(),
                            description: input.description.clone().unwrap_or_default(),
                            status: input.status.clone().unwrap_or("active".into()),
                        },
                    )
                }) {
                    Ok(m) => format!("Created module [{}]: {}", m.id, m.name),
                    Err(e) => format!("Error: {e}"),
                }
            }
            ("module", "update") => {
                let Some(ref proj) = input.project else {
                    return "Error: project required".into();
                };
                let pid = match resolve_project(&self.db, proj) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                let Some(ref current) = input.current_name else {
                    return "Error: current_name required to identify module".into();
                };
                let mid = match resolve_module(&self.db, pid, current) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                match self.write(|conn| {
                    queries::update_module(
                        conn,
                        mid,
                        &models::UpdateModule {
                            name: input.name.clone(),
                            description: input.description.clone(),
                            status: input.status.clone(),
                        },
                    )
                }) {
                    Ok(m) => format!("Updated module: {}", m.name),
                    Err(e) => format!("Error: {e}"),
                }
            }
            ("label", "create") => {
                let Some(ref proj) = input.project else {
                    return "Error: project required".into();
                };
                let pid = match resolve_project(&self.db, proj) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                let Some(ref name) = input.name else {
                    return "Error: name required".into();
                };
                match self.write(|conn| {
                    queries::create_label(
                        conn,
                        &models::CreateLabel {
                            project_id: pid,
                            name: name.clone(),
                            color: input.color.clone().unwrap_or("#6B7280".into()),
                        },
                    )
                }) {
                    Ok(l) => format!("Created label: {} ({})", l.name, l.color),
                    Err(e) => format!("Error: {e}"),
                }
            }
            ("label", "update") => {
                let Some(ref proj) = input.project else {
                    return "Error: project required".into();
                };
                let pid = match resolve_project(&self.db, proj) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                let Some(ref current) = input.current_name else {
                    return "Error: current_name required to identify label".into();
                };
                let lid = match self.read(|conn| queries::resolve_label_name(conn, pid, current)) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                match self.write(|conn| {
                    queries::update_label(
                        conn,
                        lid,
                        &models::UpdateLabel {
                            name: input.name.clone(),
                            color: input.color.clone(),
                        },
                    )
                }) {
                    Ok(l) => format!("Updated label: {} ({})", l.name, l.color),
                    Err(e) => format!("Error: {e}"),
                }
            }
            ("folder", "create") => {
                let Some(ref proj) = input.project else {
                    return "Error: project required".into();
                };
                let pid = match resolve_project(&self.db, proj) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                let Some(ref name) = input.name else {
                    return "Error: name required".into();
                };
                match self.write(|conn| {
                    queries::create_folder(
                        conn,
                        &models::CreateFolder {
                            project_id: pid,
                            parent_id: None,
                            name: name.clone(),
                        },
                    )
                }) {
                    Ok(f) => format!("Created folder [{}]: {}", f.id, f.name),
                    Err(e) => format!("Error: {e}"),
                }
            }
            ("folder", "update") => {
                let Some(ref proj) = input.project else {
                    return "Error: project required".into();
                };
                let pid = match resolve_project(&self.db, proj) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                let Some(ref current) = input.current_name else {
                    return "Error: current_name required to identify folder".into();
                };
                let fid = match self.read(|conn| queries::resolve_folder_name(conn, pid, current)) {
                    Ok(id) => id,
                    Err(e) => return format!("Error: {e}"),
                };
                match self.write(|conn| {
                    queries::update_folder(
                        conn,
                        fid,
                        &models::UpdateFolder {
                            name: input.name.clone(),
                        },
                    )
                }) {
                    Ok(f) => format!("Updated folder: {}", f.name),
                    Err(e) => format!("Error: {e}"),
                }
            }
            (rt, act) => format!(
                "Unsupported: {rt}/{act}. Types: project, module, label, folder. Actions: create, update."
            ),
        }
    }

    #[tool(
        description = "Add a comment to an issue. The author is the user who owns the API key authenticating this MCP session."
    )]
    fn add_comment(&self, Parameters(input): Parameters<AddCommentInput>) -> String {
        let issue_id = match self.read(|conn| queries::resolve_identifier(conn, &input.identifier))
        {
            Ok(id) => id,
            Err(e) => return format!("Error: {e}"),
        };

        // Resolve the authenticated user from the task-local set by the HTTP handler.
        // For stdio/local MCP sessions (no HTTP auth), fall back to the first admin user.
        let user_id = match super::current_auth_user() {
            Some(u) => u.id,
            None => {
                match self.read(queries::users::first_admin) {
                    Ok(Some(admin)) => admin.id,
                    Ok(None) => {
                        return "Error: no admin user exists to attribute comments to.".into();
                    }
                    Err(e) => return format!("Error: {e}"),
                }
            }
        };

        match self.write(|conn| {
            queries::comments::create_comment(conn, issue_id, user_id, &input.content)
        }) {
            Ok(c) => format!(
                "Comment added to {} by {} at {}: {}",
                input.identifier, c.author, c.created_at, c.content
            ),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "List comments on an issue")]
    fn list_comments(&self, Parameters(input): Parameters<ListCommentsInput>) -> String {
        let issue_id = match self.read(|conn| queries::resolve_identifier(conn, &input.identifier))
        {
            Ok(id) => id,
            Err(e) => return format!("Error: {e}"),
        };

        match self.read(|conn| queries::comments::list_comments(conn, issue_id)) {
            Ok(comments) if comments.is_empty() => {
                format!("No comments on {}.", input.identifier)
            }
            Ok(comments) => {
                let mut out = format!("{} comment(s) on {}:\n", comments.len(), input.identifier);
                for c in &comments {
                    out.push_str(&format!(
                        "[{}] {} ({}): {}\n",
                        c.created_at, c.author, c.author_display_name, c.content
                    ));
                }
                out
            }
            Err(e) => format!("Error: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::handler::server::wrapper::Parameters;

    fn mcp() -> LificMcp {
        let db = crate::db::open_memory().expect("test db");
        LificMcp::new(db)
    }

    /// Seed a project via manage_resource, return identifier.
    fn seed_project(mcp: &LificMcp, name: &str, ident: &str) -> String {
        let result = mcp.manage_resource(Parameters(ManageResourceInput {
            resource_type: "project".into(),
            action: "create".into(),
            name: Some(name.into()),
            identifier: Some(ident.into()),
            description: None,
            project: None,
            current_name: None,
            status: None,
            color: None,
        }));
        assert!(result.starts_with("Created project"), "got: {result}");
        ident.to_string()
    }

    fn seed_issue(mcp: &LificMcp, project: &str, title: &str) -> String {
        let result = mcp.create_issue(Parameters(CreateIssueInput {
            project: project.into(),
            title: title.into(),
            description: None,
            status: None,
            priority: None,
            module: None,
            labels: None,
        }));
        assert!(result.starts_with("Created"), "got: {result}");
        result
    }

    // ── manage_resource ──

    #[test]
    fn manage_create_project() {
        let m = mcp();
        let result = seed_project(&m, "Alpha", "ALP");
        assert_eq!(result, "ALP");
    }

    #[test]
    fn manage_update_project() {
        let m = mcp();
        seed_project(&m, "Old", "UPD");
        let result = m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "project".into(),
            action: "update".into(),
            project: Some("UPD".into()),
            name: Some("New Name".into()),
            identifier: None,
            description: None,
            current_name: None,
            status: None,
            color: None,
        }));
        assert!(result.contains("New Name"), "got: {result}");
    }

    #[test]
    fn manage_create_module() {
        let m = mcp();
        seed_project(&m, "Test", "MOD");
        let result = m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "module".into(),
            action: "create".into(),
            project: Some("MOD".into()),
            name: Some("Backend".into()),
            description: Some("Server-side logic".into()),
            identifier: None,
            current_name: None,
            status: None,
            color: None,
        }));
        assert!(result.contains("Backend"), "got: {result}");
    }

    #[test]
    fn manage_create_label() {
        let m = mcp();
        seed_project(&m, "Test", "LBL");
        let result = m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "label".into(),
            action: "create".into(),
            project: Some("LBL".into()),
            name: Some("bug".into()),
            color: Some("#EF4444".into()),
            identifier: None,
            description: None,
            current_name: None,
            status: None,
        }));
        assert!(result.contains("bug"), "got: {result}");
        assert!(result.contains("#EF4444"), "got: {result}");
    }

    #[test]
    fn manage_create_folder() {
        let m = mcp();
        seed_project(&m, "Test", "FLD");
        let result = m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "folder".into(),
            action: "create".into(),
            project: Some("FLD".into()),
            name: Some("Docs".into()),
            identifier: None,
            description: None,
            current_name: None,
            status: None,
            color: None,
        }));
        assert!(result.contains("Docs"), "got: {result}");
    }

    #[test]
    fn manage_missing_name_errors() {
        let m = mcp();
        let result = m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "project".into(),
            action: "create".into(),
            name: None,
            identifier: Some("X".into()),
            description: None,
            project: None,
            current_name: None,
            status: None,
            color: None,
        }));
        assert!(result.contains("name required"), "got: {result}");
    }

    #[test]
    fn manage_unknown_type() {
        let m = mcp();
        let result = m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "widget".into(),
            action: "create".into(),
            name: None,
            identifier: None,
            description: None,
            project: None,
            current_name: None,
            status: None,
            color: None,
        }));
        assert!(result.contains("Unsupported"), "got: {result}");
    }

    // ── create / get / update / delete issue ──

    #[test]
    fn issue_create_and_get() {
        let m = mcp();
        seed_project(&m, "Test", "TST");
        let created = seed_issue(&m, "TST", "First issue");
        assert!(created.contains("TST-1"), "got: {created}");

        let detail = m.get_issue(Parameters(GetIssueInput {
            identifier: "TST-1".into(),
        }));
        assert!(detail.contains("First issue"), "got: {detail}");
        assert!(detail.contains("backlog"), "got: {detail}");
    }

    #[test]
    fn issue_create_with_options() {
        let m = mcp();
        seed_project(&m, "Test", "OPT");
        m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "label".into(),
            action: "create".into(),
            project: Some("OPT".into()),
            name: Some("feature".into()),
            color: None,
            identifier: None,
            description: None,
            current_name: None,
            status: None,
        }));
        let result = m.create_issue(Parameters(CreateIssueInput {
            project: "OPT".into(),
            title: "Detailed issue".into(),
            description: Some("Some markdown".into()),
            status: Some("todo".into()),
            priority: Some("high".into()),
            module: None,
            labels: Some(vec!["feature".into()]),
        }));
        assert!(result.contains("OPT-1"), "got: {result}");

        let detail = m.get_issue(Parameters(GetIssueInput {
            identifier: "OPT-1".into(),
        }));
        assert!(detail.contains("high"), "got: {detail}");
        assert!(detail.contains("todo"), "got: {detail}");
        assert!(detail.contains("feature"), "got: {detail}");
        assert!(detail.contains("Some markdown"), "got: {detail}");
    }

    #[test]
    fn issue_update() {
        let m = mcp();
        seed_project(&m, "Test", "UPI");
        seed_issue(&m, "UPI", "Original");

        let result = m.update_issue(Parameters(UpdateIssueInput {
            identifier: "UPI-1".into(),
            title: Some("Renamed".into()),
            status: Some("active".into()),
            priority: Some("urgent".into()),
            description: None,
            module: None,
            labels: None,
        }));
        assert!(result.contains("Renamed"), "got: {result}");
        assert!(result.contains("active"), "got: {result}");
        assert!(result.contains("urgent"), "got: {result}");
    }

    #[test]
    fn issue_delete() {
        let m = mcp();
        seed_project(&m, "Test", "DEL");
        seed_issue(&m, "DEL", "Doomed");

        let result = m.delete(Parameters(DeleteInput {
            resource_type: "issue".into(),
            identifier: "DEL-1".into(),
            project: None,
        }));
        assert!(result.contains("Deleted issue"), "got: {result}");

        // Verify gone
        let get = m.get_issue(Parameters(GetIssueInput {
            identifier: "DEL-1".into(),
        }));
        assert!(get.starts_with("Error"), "got: {get}");
    }

    #[test]
    fn get_nonexistent_issue_errors() {
        let m = mcp();
        let result = m.get_issue(Parameters(GetIssueInput {
            identifier: "NOPE-999".into(),
        }));
        assert!(result.starts_with("Error"), "got: {result}");
    }

    // ── list_issues ──

    #[test]
    fn list_issues_with_filters() {
        let m = mcp();
        seed_project(&m, "Test", "LST");

        m.create_issue(Parameters(CreateIssueInput {
            project: "LST".into(),
            title: "Todo one".into(),
            status: Some("todo".into()),
            priority: Some("high".into()),
            description: None,
            module: None,
            labels: None,
        }));
        m.create_issue(Parameters(CreateIssueInput {
            project: "LST".into(),
            title: "Active one".into(),
            status: Some("active".into()),
            priority: Some("low".into()),
            description: None,
            module: None,
            labels: None,
        }));

        // Filter by status
        let result = m.list_issues(Parameters(ListIssuesInput {
            project: "LST".into(),
            status: Some("todo".into()),
            priority: None,
            module: None,
            label: None,
            workable: None,
            limit: None,
            offset: None,
        }));
        assert!(result.contains("1 issues"), "got: {result}");
        assert!(result.contains("Todo one"), "got: {result}");
    }

    #[test]
    fn list_issues_empty() {
        let m = mcp();
        seed_project(&m, "Empty", "EMP");
        let result = m.list_issues(Parameters(ListIssuesInput {
            project: "EMP".into(),
            status: None,
            priority: None,
            module: None,
            label: None,
            workable: None,
            limit: None,
            offset: None,
        }));
        assert_eq!(result, "No issues found.");
    }

    #[test]
    fn list_issues_bad_project_errors() {
        let m = mcp();
        let result = m.list_issues(Parameters(ListIssuesInput {
            project: "NOPE".into(),
            status: None,
            priority: None,
            module: None,
            label: None,
            workable: None,
            limit: None,
            offset: None,
        }));
        assert!(result.starts_with("Error"), "got: {result}");
    }

    #[test]
    fn list_issues_pagination_emits_has_more_hint() {
        let m = mcp();
        seed_project(&m, "Pages", "PAG");
        // Seed 5 issues; ask for 2 — should report has_more with offset=2.
        for i in 0..5 {
            seed_issue(&m, "PAG", &format!("Issue {i}"));
        }
        let page1 = m.list_issues(Parameters(ListIssuesInput {
            project: "PAG".into(),
            status: None,
            priority: None,
            module: None,
            label: None,
            workable: None,
            limit: Some(2),
            offset: None,
        }));
        assert!(page1.contains("2 issues"), "got: {page1}");
        assert!(
            page1.contains("offset=2"),
            "expected has_more hint, got: {page1}"
        );

        // Page 2: offset=2, limit=2 — still more.
        let page2 = m.list_issues(Parameters(ListIssuesInput {
            project: "PAG".into(),
            status: None,
            priority: None,
            module: None,
            label: None,
            workable: None,
            limit: Some(2),
            offset: Some(2),
        }));
        assert!(page2.contains("2 issues"), "got: {page2}");
        assert!(
            page2.contains("offset=4"),
            "expected has_more hint, got: {page2}"
        );

        // Page 3: offset=4, limit=2 — only 1 remaining, no hint.
        let page3 = m.list_issues(Parameters(ListIssuesInput {
            project: "PAG".into(),
            status: None,
            priority: None,
            module: None,
            label: None,
            workable: None,
            limit: Some(2),
            offset: Some(4),
        }));
        assert!(page3.contains("1 issues"), "got: {page3}");
        assert!(
            !page3.contains("more results available"),
            "should NOT have hint on last page, got: {page3}"
        );
    }

    #[test]
    fn list_issues_no_hint_when_under_limit() {
        let m = mcp();
        seed_project(&m, "Small", "SML");
        seed_issue(&m, "SML", "Only one");
        let result = m.list_issues(Parameters(ListIssuesInput {
            project: "SML".into(),
            status: None,
            priority: None,
            module: None,
            label: None,
            workable: None,
            limit: Some(10),
            offset: None,
        }));
        assert!(result.contains("1 issues"), "got: {result}");
        assert!(
            !result.contains("more results available"),
            "got: {result}"
        );
    }

    // ── link / unlink ──

    #[test]
    fn link_and_unlink_issues() {
        let m = mcp();
        seed_project(&m, "Test", "LNK");
        seed_issue(&m, "LNK", "Blocker");
        seed_issue(&m, "LNK", "Blocked");

        let result = m.link_issues(Parameters(LinkIssuesInput {
            source: "LNK-1".into(),
            target: "LNK-2".into(),
            relation_type: "blocks".into(),
        }));
        assert!(result.contains("blocks"), "got: {result}");

        // Verify relation shows in get_issue
        let detail = m.get_issue(Parameters(GetIssueInput {
            identifier: "LNK-1".into(),
        }));
        assert!(detail.contains("Blocks"), "got: {detail}");

        let result = m.unlink_issues(Parameters(UnlinkIssuesInput {
            source: "LNK-1".into(),
            target: "LNK-2".into(),
        }));
        assert!(result.contains("Unlinked"), "got: {result}");
    }

    // ── board ──

    #[test]
    fn board_groups_by_status() {
        let m = mcp();
        seed_project(&m, "Test", "BRD");
        m.create_issue(Parameters(CreateIssueInput {
            project: "BRD".into(),
            title: "A".into(),
            status: Some("todo".into()),
            description: None,
            priority: None,
            module: None,
            labels: None,
        }));
        m.create_issue(Parameters(CreateIssueInput {
            project: "BRD".into(),
            title: "B".into(),
            status: Some("active".into()),
            description: None,
            priority: None,
            module: None,
            labels: None,
        }));

        let result = m.get_board(Parameters(GetBoardInput {
            project: "BRD".into(),
            group_by: None,
        }));
        assert!(result.contains("todo"), "got: {result}");
        assert!(result.contains("active"), "got: {result}");
    }

    // ── pages ──

    #[test]
    fn page_create_get_update() {
        let m = mcp();
        seed_project(&m, "Test", "PG");

        let created = m.create_page(Parameters(CreatePageInput {
            project: Some("PG".into()),
            title: "Design Doc".into(),
            content: Some("# Overview\nSome content".into()),
            folder: None,
        }));
        assert!(created.contains("PG-DOC-1"), "got: {created}");

        let detail = m.get_page(Parameters(GetPageInput {
            identifier: "PG-DOC-1".into(),
        }));
        assert!(detail.contains("Design Doc"), "got: {detail}");
        assert!(detail.contains("# Overview"), "got: {detail}");

        let updated = m.update_page(Parameters(UpdatePageInput {
            identifier: "PG-DOC-1".into(),
            title: Some("Updated Doc".into()),
            content: None,
            folder: None,
        }));
        assert!(updated.contains("Updated Doc"), "got: {updated}");
    }

    #[test]
    fn workspace_page_no_project() {
        let m = mcp();
        let created = m.create_page(Parameters(CreatePageInput {
            project: None,
            title: "Global Note".into(),
            content: None,
            folder: None,
        }));
        assert!(created.contains("DOC-"), "got: {created}");
    }

    #[test]
    fn page_delete() {
        let m = mcp();
        seed_project(&m, "Test", "PGD");
        m.create_page(Parameters(CreatePageInput {
            project: Some("PGD".into()),
            title: "Temp".into(),
            content: None,
            folder: None,
        }));
        let result = m.delete(Parameters(DeleteInput {
            resource_type: "page".into(),
            identifier: "PGD-DOC-1".into(),
            project: None,
        }));
        assert!(result.contains("Deleted page"), "got: {result}");
    }

    // ── search ──

    #[test]
    fn search_finds_issue() {
        let m = mcp();
        seed_project(&m, "Test", "SRC");
        seed_issue(&m, "SRC", "Unique searchterm xyz");

        let result = m.search(Parameters(SearchInput {
            query: "searchterm".into(),
            project: None,
            limit: None,
        }));
        assert!(result.contains("1 results"), "got: {result}");
        assert!(result.contains("searchterm"), "got: {result}");
    }

    #[test]
    fn search_no_results() {
        let m = mcp();
        let result = m.search(Parameters(SearchInput {
            query: "nonexistent_gibberish_zzz".into(),
            project: None,
            limit: None,
        }));
        assert_eq!(result, "No results found.");
    }

    // ── list_resources ──

    #[test]
    fn list_resources_projects() {
        let m = mcp();
        seed_project(&m, "Alpha", "AAA");
        seed_project(&m, "Beta", "BBB");

        let result = m.list_resources(Parameters(ListResourcesInput {
            resource_type: "project".into(),
            project: None,
            folder: None,
            limit: None,
            offset: None,
        }));
        assert!(result.contains("2 projects"), "got: {result}");
        assert!(result.contains("AAA"), "got: {result}");
        assert!(result.contains("BBB"), "got: {result}");
    }

    #[test]
    fn list_resources_requires_project() {
        let m = mcp();
        for rt in ["module", "label", "folder", "issue"] {
            let result = m.list_resources(Parameters(ListResourcesInput {
                resource_type: rt.into(),
                project: None,
                folder: None,
                limit: None,
                offset: None,
            }));
            assert!(result.contains("project required"), "{rt} got: {result}");
        }
    }

    #[test]
    fn list_resources_unknown_type() {
        let m = mcp();
        let result = m.list_resources(Parameters(ListResourcesInput {
            resource_type: "widget".into(),
            project: None,
            folder: None,
            limit: None,
            offset: None,
        }));
        assert!(result.contains("Unknown type"), "got: {result}");
    }

    #[test]
    fn list_resources_issues_pagination() {
        let m = mcp();
        seed_project(&m, "Bulk", "BLK");
        for i in 0..4 {
            seed_issue(&m, "BLK", &format!("Issue {i}"));
        }
        let result = m.list_resources(Parameters(ListResourcesInput {
            resource_type: "issue".into(),
            project: Some("BLK".into()),
            folder: None,
            limit: Some(2),
            offset: None,
        }));
        assert!(result.contains("2 issues"), "got: {result}");
        assert!(result.contains("offset=2"), "got: {result}");
    }

    // ── delete ──

    #[test]
    fn delete_project() {
        let m = mcp();
        seed_project(&m, "Doomed", "DPJ");
        let result = m.delete(Parameters(DeleteInput {
            resource_type: "project".into(),
            identifier: "DPJ".into(),
            project: None,
        }));
        assert!(result.contains("Deleted project"), "got: {result}");
    }

    #[test]
    fn delete_module_requires_project() {
        let m = mcp();
        let result = m.delete(Parameters(DeleteInput {
            resource_type: "module".into(),
            identifier: "Backend".into(),
            project: None,
        }));
        assert!(result.contains("project required"), "got: {result}");
    }

    #[test]
    fn delete_unknown_type() {
        let m = mcp();
        let result = m.delete(Parameters(DeleteInput {
            resource_type: "widget".into(),
            identifier: "x".into(),
            project: None,
        }));
        assert!(result.contains("Unknown type"), "got: {result}");
    }

    // ── manage_resource update label/folder ──

    #[test]
    fn manage_update_label() {
        let m = mcp();
        seed_project(&m, "Test", "UPL");
        m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "label".into(),
            action: "create".into(),
            project: Some("UPL".into()),
            name: Some("bug".into()),
            color: Some("#EF4444".into()),
            identifier: None,
            description: None,
            current_name: None,
            status: None,
        }));
        let result = m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "label".into(),
            action: "update".into(),
            project: Some("UPL".into()),
            current_name: Some("bug".into()),
            name: Some("defect".into()),
            color: Some("#FF0000".into()),
            identifier: None,
            description: None,
            status: None,
        }));
        assert!(result.contains("defect"), "got: {result}");
        assert!(result.contains("#FF0000"), "got: {result}");
    }

    #[test]
    fn manage_update_folder() {
        let m = mcp();
        seed_project(&m, "Test", "UPF");
        m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "folder".into(),
            action: "create".into(),
            project: Some("UPF".into()),
            name: Some("Docs".into()),
            identifier: None,
            description: None,
            current_name: None,
            status: None,
            color: None,
        }));
        let result = m.manage_resource(Parameters(ManageResourceInput {
            resource_type: "folder".into(),
            action: "update".into(),
            project: Some("UPF".into()),
            current_name: Some("Docs".into()),
            name: Some("Documentation".into()),
            identifier: None,
            description: None,
            status: None,
            color: None,
        }));
        assert!(result.contains("Documentation"), "got: {result}");
    }

    // ── fmt_issue ──

    #[test]
    fn fmt_issue_includes_relations() {
        let issue = models::Issue {
            id: 1,
            project_id: 1,
            sequence: 1,
            identifier: "T-1".into(),
            title: "Test".into(),
            description: String::new(),
            status: "todo".into(),
            priority: "high".into(),
            module_id: None,
            sort_order: 0.0,
            start_date: None,
            target_date: None,
            created_at: String::new(),
            updated_at: String::new(),
            labels: vec!["bug".into()],
            blocks: vec!["T-2".into()],
            blocked_by: vec![],
            relates_to: vec![],
        };
        let s = fmt_issue(&issue);
        assert!(s.contains("[bug]"), "got: {s}");
        assert!(s.contains("blocks:T-2"), "got: {s}");
    }

    // ── comments ──

    fn seed_user(mcp: &LificMcp) {
        let conn = mcp.db.write().unwrap();
        let user = crate::db::queries::users::create_user(
            &conn,
            &models::CreateUser {
                username: "testuser".into(),
                email: "test@test.com".into(),
                password: "testpassword1".into(),
                display_name: Some("Test User".into()),
                is_admin: true,
                is_bot: false,
            },
        )
        .unwrap();
        // Set the authenticated user context so add_comment works in tests
        *crate::mcp::MCP_REQUEST_USER
            .lock()
            .unwrap_or_else(|e: std::sync::PoisonError<_>| e.into_inner()) = Some(models::AuthUser {
            id: user.id,
            username: user.username.clone(),
            display_name: user.display_name,
            is_admin: user.is_admin,
        });
    }

    #[test]
    fn add_and_list_comments() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "Test issue");
        seed_user(&m);

        let result = m.add_comment(Parameters(AddCommentInput {
            identifier: "PRJ-1".into(),
            content: "Hello from MCP".into(),
        }));
        assert!(result.contains("Comment added"), "got: {result}");
        assert!(result.contains("testuser"), "got: {result}");

        let result = m.add_comment(Parameters(AddCommentInput {
            identifier: "PRJ-1".into(),
            content: "Second comment".into(),
        }));
        assert!(result.contains("Comment added"), "got: {result}");

        let result = m.list_comments(Parameters(ListCommentsInput {
            identifier: "PRJ-1".into(),
        }));
        assert!(result.contains("2 comment(s)"), "got: {result}");
        assert!(result.contains("Hello from MCP"), "got: {result}");
        assert!(result.contains("Second comment"), "got: {result}");
    }

    #[test]
    fn get_issue_includes_comments() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "Commented issue");
        seed_user(&m);

        m.add_comment(Parameters(AddCommentInput {
            identifier: "PRJ-1".into(),
            content: "Visible in get_issue".into(),
        }));

        let result = m.get_issue(Parameters(GetIssueInput {
            identifier: "PRJ-1".into(),
        }));
        assert!(result.contains("Comments (1)"), "got: {result}");
        assert!(result.contains("Visible in get_issue"), "got: {result}");
    }

    #[test]
    fn list_comments_empty() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "No comments");

        let result = m.list_comments(Parameters(ListCommentsInput {
            identifier: "PRJ-1".into(),
        }));
        assert!(result.contains("No comments"), "got: {result}");
    }

    #[test]
    fn add_comment_bad_identifier() {
        let m = mcp();
        seed_user(&m);

        let result = m.add_comment(Parameters(AddCommentInput {
            identifier: "NOPE-999".into(),
            content: "Orphan".into(),
        }));
        assert!(result.contains("Error"), "got: {result}");
    }

    #[test]
    fn add_comment_falls_back_to_first_admin() {
        let m = mcp();
        seed_project(&m, "Proj", "PRJ");
        seed_issue(&m, "PRJ", "Test issue");

        // Create an admin user but do NOT set MCP_REQUEST_USER — simulates stdio/local auth.
        let conn = m.db.write().unwrap();
        queries::users::create_user(
            &conn,
            &models::CreateUser {
                username: "admin".into(),
                email: "admin@local.test".into(),
                password: "adminpass123".into(),
                display_name: Some("Admin User".into()),
                is_admin: true,
                is_bot: false,
            },
        )
        .unwrap();
        drop(conn);

        // Clear any leftover auth context
        *crate::mcp::MCP_REQUEST_USER
            .lock()
            .unwrap_or_else(|e: std::sync::PoisonError<_>| e.into_inner()) = None;

        let result = m.add_comment(Parameters(AddCommentInput {
            identifier: "PRJ-1".into(),
            content: "Comment via stdio fallback".into(),
        }));
        assert!(result.contains("Comment added"), "got: {result}");
        assert!(result.contains("admin"), "got: {result}");
    }
}
