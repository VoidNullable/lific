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
                    limit: input.limit,
                    offset: None,
                },
            )
        }) {
            Ok(issues) if issues.is_empty() => "No issues found.".into(),
            Ok(issues) => {
                let mut out = format!("{} issues:\n", issues.len());
                for i in &issues {
                    out.push_str(&format!("- {}\n", fmt_issue(i)));
                }
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
                out
            }
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
                    limit: Some(500),
                    offset: None,
                },
            )
        }) {
            Ok(issues) => {
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
                    folder_id,
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
                            limit: Some(100),
                            offset: None,
                        },
                    )
                }) {
                    Ok(issues) => {
                        let mut out =
                            format!("{} issues (use list_issues for filtering):\n", issues.len());
                        for i in &issues {
                            out.push_str(&format!(
                                "- {} | {} | {}\n",
                                i.identifier, i.status, i.title
                            ));
                        }
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
            (rt, act) => format!(
                "Unsupported: {rt}/{act}. Types: project, module, label, folder. Actions: create, update."
            ),
        }
    }
}
