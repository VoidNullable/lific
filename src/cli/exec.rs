use crate::db::models::*;
use crate::db::queries;
use crate::db::DbPool;
use crate::error::LificError;

use super::*;

/// Run a CLI CRUD command against the database.
/// Returns Ok(()) on success, printing output to stdout.
pub fn run(pool: &DbPool, command: &Command, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        Command::Issue { action } => issue(pool, action, json),
        Command::Project { action } => project(pool, action, json),
        Command::Page { action } => page(pool, action, json),
        Command::Export { action } => export(pool, action, json),
        Command::Search {
            query,
            project,
            limit,
        } => search(pool, query, project.as_deref(), *limit, json),
        Command::Comment { action } => comment(pool, action, json),
        Command::Module { action } => module(pool, action, json),
        Command::Label { action } => label(pool, action, json),
        Command::Folder { action } => folder(pool, action, json),
        _ => unreachable!("non-CRUD commands are handled in main.rs"),
    }
}

fn export(
    pool: &DbPool,
    action: &ExportAction,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let conn = pool.read()?;
    let (bundle, output) = match action {
        ExportAction::Issue { identifier, output } => {
            (crate::export::export_issue(&conn, identifier)?, output)
        }
        ExportAction::Page { identifier, output } => {
            (crate::export::export_page(&conn, identifier)?, output)
        }
        ExportAction::Project { project, output } => {
            (crate::export::export_project(&conn, project)?, output)
        }
    };

    let written = crate::export::write_bundle_to_directory(&bundle, output)?;
    if json {
        print_json(&written);
    } else {
        println!("Exported {} file(s) to {}", written.len(), output.display());
        for path in written {
            println!("  {}", path.display());
        }
    }
    Ok(())
}

// ── Helpers ──────────────────────────────────────────────────

fn print_json<T: serde::Serialize>(val: &T) {
    println!("{}", serde_json::to_string_pretty(val).unwrap());
}

/// Format a priority with visual indicator for human output.
fn fmt_priority(p: &str) -> &str {
    match p {
        "urgent" => "!!!  urgent",
        "high" => "!!   high",
        "medium" => "!    medium",
        "low" => "     low",
        _ => "     none",
    }
}

/// Format a status with visual indicator for human output.
fn fmt_status(s: &str) -> &str {
    match s {
        "backlog" => "[ ] backlog",
        "todo" => "[.] todo",
        "active" => "[~] active",
        "done" => "[x] done",
        "cancelled" => "[-] cancelled",
        _ => s,
    }
}

// ── Issue ────────────────────────────────────────────────────

fn issue(
    pool: &DbPool,
    action: &IssueAction,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        IssueAction::List {
            project,
            status,
            priority,
            module,
            label,
            workable,
            limit,
        } => {
            let conn = pool.read()?;
            let project_id = queries::resolve_project_identifier(&conn, project)?;

            let module_id = if let Some(name) = module {
                Some(queries::resolve_module_name(&conn, project_id, name)?)
            } else {
                None
            };

            let issues = queries::list_issues(
                &conn,
                &ListIssuesQuery {
                    project_id: Some(project_id),
                    status: status.clone(),
                    priority: priority.clone(),
                    module_id,
                    label: label.clone(),
                    workable: if *workable { Some(true) } else { None },
                    limit: *limit,
                    offset: None,
                },
            )?;

            if json {
                print_json(&issues);
            } else if issues.is_empty() {
                println!("No issues found.");
            } else {
                println!("{} issue(s):\n", issues.len());
                for i in &issues {
                    let labels = if i.labels.is_empty() {
                        String::new()
                    } else {
                        format!(" [{}]", i.labels.join(", "))
                    };
                    let module = if let Some(mid) = i.module_id {
                        match queries::get_module_name(&conn, mid) {
                            Ok(name) => format!(" ({name})"),
                            Err(_) => String::new(),
                        }
                    } else {
                        String::new()
                    };
                    println!(
                        "  {:<8} {} | {} | {}{}{}",
                        i.identifier,
                        fmt_status(&i.status),
                        fmt_priority(&i.priority),
                        i.title,
                        labels,
                        module
                    );
                }
            }
        }

        IssueAction::Get { identifier } => {
            let conn = pool.read()?;
            let id = queries::resolve_identifier(&conn, identifier)?;
            let issue = queries::get_issue(&conn, id)?;

            if json {
                print_json(&issue);
            } else {
                println!("{} - {}", issue.identifier, issue.title);
                println!("  Status:   {}", issue.status);
                println!("  Priority: {}", issue.priority);
                if !issue.labels.is_empty() {
                    println!("  Labels:   {}", issue.labels.join(", "));
                }
                if let Some(mid) = issue.module_id
                    && let Ok(name) = queries::get_module_name(&conn, mid) {
                        println!("  Module:   {name}");
                    }
                if !issue.blocks.is_empty() {
                    println!("  Blocks:   {}", issue.blocks.join(", "));
                }
                if !issue.blocked_by.is_empty() {
                    println!("  Blocked:  {}", issue.blocked_by.join(", "));
                }
                if !issue.relates_to.is_empty() {
                    println!("  Relates:  {}", issue.relates_to.join(", "));
                }
                if !issue.description.is_empty() {
                    println!();
                    println!("{}", issue.description);
                }
            }
        }

        IssueAction::Create {
            project,
            title,
            description,
            status,
            priority,
            module,
            labels,
        } => {
            let conn = pool.write()?;
            let project_id = queries::resolve_project_identifier(&conn, project)?;

            let module_id = if let Some(name) = module {
                Some(queries::resolve_module_name(&conn, project_id, name)?)
            } else {
                None
            };

            let label_list: Vec<String> = labels
                .as_deref()
                .map(|s| {
                    s.split(',')
                        .map(|l| l.trim().to_string())
                        .filter(|l| !l.is_empty())
                        .collect()
                })
                .unwrap_or_default();

            let issue = queries::create_issue(
                &conn,
                &CreateIssue {
                    project_id,
                    title: title.clone(),
                    description: description.clone(),
                    status: status.clone(),
                    priority: priority.clone(),
                    module_id,
                    start_date: None,
                    target_date: None,
                    labels: label_list,
                },
            )?;

            if json {
                print_json(&issue);
            } else {
                println!("Created {}: {}", issue.identifier, issue.title);
            }
        }

        IssueAction::Update {
            identifier,
            title,
            description,
            status,
            priority,
            module,
            labels,
        } => {
            let conn = pool.write()?;
            let id = queries::resolve_identifier(&conn, identifier)?;

            let module_id = if let Some(name) = module {
                let issue = queries::get_issue(&conn, id)?;
                let project_id = issue.project_id;
                Some(queries::resolve_module_name(&conn, project_id, name)?)
            } else {
                None
            };

            let label_list = labels.as_deref().map(|s| {
                s.split(',')
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect()
            });

            let issue = queries::update_issue(
                &conn,
                id,
                &UpdateIssue {
                    title: title.clone(),
                    description: description.clone(),
                    status: status.clone(),
                    priority: priority.clone(),
                    module_id,
                    sort_order: None,
                    start_date: None,
                    target_date: None,
                    labels: label_list,
                },
            )?;

            if json {
                print_json(&issue);
            } else {
                println!("Updated {}: {}", issue.identifier, issue.title);
                println!("  Status:   {}", issue.status);
                println!("  Priority: {}", issue.priority);
            }
        }
    }
    Ok(())
}

// ── Project ──────────────────────────────────────────────────

fn project(
    pool: &DbPool,
    action: &ProjectAction,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        ProjectAction::List => {
            let conn = pool.read()?;
            let projects = queries::list_projects(&conn)?;

            if json {
                print_json(&projects);
            } else if projects.is_empty() {
                println!("No projects.");
            } else {
                println!("{} project(s):\n", projects.len());
                for p in &projects {
                    let desc = if p.description.is_empty() {
                        String::new()
                    } else {
                        format!(" - {}", p.description.lines().next().unwrap_or(""))
                    };
                    println!("  {:<5} {}{}", p.identifier, p.name, desc);
                }
            }
        }

        ProjectAction::Get { identifier } => {
            let conn = pool.read()?;
            let id = queries::resolve_project_identifier(&conn, identifier)?;
            let project = queries::get_project(&conn, id)?;

            if json {
                print_json(&project);
            } else {
                println!("{} - {}", project.identifier, project.name);
                if !project.description.is_empty() {
                    println!();
                    println!("{}", project.description);
                }
            }
        }

        ProjectAction::Create {
            name,
            identifier,
            description,
        } => {
            let conn = pool.write()?;
            let project = queries::create_project(
                &conn,
                &CreateProject {
                    name: name.clone(),
                    identifier: identifier.clone(),
                    description: description.clone(),
                    emoji: None,
                    lead_user_id: None,
                },
            )?;

            if json {
                print_json(&project);
            } else {
                println!("Created project {} ({})", project.name, project.identifier);
            }
        }

        ProjectAction::Update {
            identifier,
            name,
            description,
        } => {
            let conn = pool.write()?;
            let id = queries::resolve_project_identifier(&conn, identifier)?;
            let project = queries::update_project(
                &conn,
                id,
                &UpdateProject {
                    name: name.clone(),
                    identifier: None,
                    description: description.clone(),
                    emoji: None,
                    lead_user_id: None,
                },
            )?;

            if json {
                print_json(&project);
            } else {
                println!("Updated project {} ({})", project.name, project.identifier);
            }
        }
    }
    Ok(())
}

// ── Page ─────────────────────────────────────────────────────

fn page(pool: &DbPool, action: &PageAction, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        PageAction::List { project, folder } => {
            let conn = pool.read()?;
            let project_id = if let Some(ident) = project {
                Some(queries::resolve_project_identifier(&conn, ident)?)
            } else {
                None
            };

            let folder_id = if let (Some(pid), Some(fname)) = (project_id, folder) {
                Some(queries::resolve_folder_name(&conn, pid, fname)?)
            } else {
                None
            };

            let pages = queries::list_pages(&conn, project_id, folder_id)?;

            if json {
                print_json(&pages);
            } else if pages.is_empty() {
                println!("No pages found.");
            } else {
                println!("{} page(s):\n", pages.len());
                for p in &pages {
                    let preview = if p.content.is_empty() {
                        "(empty)".to_string()
                    } else {
                        let first_line = p.content.lines().next().unwrap_or("");
                        if first_line.len() > 60 {
                            format!("{}...", &first_line[..60])
                        } else {
                            first_line.to_string()
                        }
                    };
                    println!("  {:<12} {} - {}", p.identifier, p.title, preview);
                }
            }
        }

        PageAction::Get { identifier } => {
            let conn = pool.read()?;
            let id = queries::resolve_page_identifier(&conn, identifier)?;
            let page = queries::get_page(&conn, id)?;

            if json {
                print_json(&page);
            } else {
                println!("{} - {}", page.identifier, page.title);
                if !page.content.is_empty() {
                    println!();
                    println!("{}", page.content);
                }
            }
        }

        PageAction::Create {
            title,
            project,
            folder,
            content,
        } => {
            let conn = pool.write()?;
            let project_id = if let Some(ident) = project {
                Some(queries::resolve_project_identifier(&conn, ident)?)
            } else {
                None
            };

            let folder_id = if let (Some(pid), Some(fname)) = (project_id, folder) {
                Some(queries::resolve_folder_name(&conn, pid, fname)?)
            } else {
                None
            };

            let page = queries::create_page(
                &conn,
                &CreatePage {
                    project_id,
                    folder_id,
                    title: title.clone(),
                    content: content.clone(),
                },
            )?;

            if json {
                print_json(&page);
            } else {
                println!("Created page {}: {}", page.identifier, page.title);
            }
        }

        PageAction::Update {
            identifier,
            title,
            content,
            folder,
        } => {
            let conn = pool.write()?;
            let id = queries::resolve_page_identifier(&conn, identifier)?;

            let folder_id = if let Some(fname) = folder {
                let page = queries::get_page(&conn, id)?;
                if let Some(pid) = page.project_id {
                    Some(Some(queries::resolve_folder_name(&conn, pid, fname)?))
                } else {
                    return Err("cannot set folder on workspace page".into());
                }
            } else {
                None
            };

            let page = queries::update_page(
                &conn,
                id,
                &UpdatePage {
                    title: title.clone(),
                    content: content.clone(),
                    folder_id,
                    sort_order: None,
                },
            )?;

            if json {
                print_json(&page);
            } else {
                println!("Updated page {}: {}", page.identifier, page.title);
            }
        }
    }
    Ok(())
}

// ── Search ───────────────────────────────────────────────────

fn search(
    pool: &DbPool,
    query: &str,
    project: Option<&str>,
    limit: Option<i64>,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let conn = pool.read()?;
    let project_id = if let Some(ident) = project {
        Some(queries::resolve_project_identifier(&conn, ident)?)
    } else {
        None
    };

    let results = queries::search(
        &conn,
        &SearchQuery {
            query: query.to_string(),
            project_id,
            limit,
        },
    )?;

    if json {
        print_json(&results);
    } else if results.is_empty() {
        println!("No results found.");
    } else {
        println!("{} result(s):\n", results.len());
        for r in &results {
            let ident = r.identifier.as_deref().unwrap_or("?");
            println!("  {:<12} [{}] {}", ident, r.result_type, r.title);
            if !r.snippet.is_empty() {
                // Clean up snippet for terminal display
                let snippet = r.snippet.replace("**", "").replace('\n', " ");
                let snippet = if snippet.len() > 80 {
                    format!("{}...", &snippet[..80])
                } else {
                    snippet
                };
                println!("              {}", snippet);
            }
        }
    }
    Ok(())
}

// ── Comment ──────────────────────────────────────────────────

fn comment(
    pool: &DbPool,
    action: &CommentAction,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        CommentAction::List { identifier } => {
            let conn = pool.read()?;
            let id = queries::resolve_identifier(&conn, identifier)?;
            let comments = queries::comments::list_comments(&conn, id)?;

            if json {
                print_json(&comments);
            } else if comments.is_empty() {
                println!("No comments on {}.", identifier);
            } else {
                println!("{} comment(s) on {}:\n", comments.len(), identifier);
                for c in &comments {
                    println!(
                        "  {} ({}) - {}:",
                        c.author_display_name, c.author, c.created_at
                    );
                    for line in c.content.lines() {
                        println!("    {line}");
                    }
                    println!();
                }
            }
        }

        CommentAction::Add {
            identifier,
            content,
            user,
        } => {
            let conn = pool.write()?;
            let issue_id = queries::resolve_identifier(&conn, identifier)?;

            // Resolve user: either explicit --user or fall back to first admin
            let user_id = if let Some(username) = user {
                let u = queries::users::get_user_by_username(&conn, username)?;
                u.id
            } else {
                // Fall back to first admin user
                let users = queries::users::list_users(&conn)?;
                users
                    .iter()
                    .find(|u| u.is_admin && !u.is_bot)
                    .or_else(|| users.first())
                    .map(|u| u.id)
                    .ok_or_else(|| {
                        LificError::NotFound("no users exist; create a user first".into())
                    })?
            };

            let comment = queries::comments::create_comment(&conn, issue_id, user_id, content)?;

            if json {
                print_json(&comment);
            } else {
                println!("Added comment to {} by {}:", identifier, comment.author);
                println!("  {}", comment.content);
            }
        }
    }
    Ok(())
}

// ── Module ───────────────────────────────────────────────────

fn module(
    pool: &DbPool,
    action: &ModuleAction,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        ModuleAction::List { project } => {
            let conn = pool.read()?;
            let project_id = queries::resolve_project_identifier(&conn, project)?;
            let modules = queries::list_modules(&conn, project_id)?;

            if json {
                print_json(&modules);
            } else if modules.is_empty() {
                println!("No modules in {}.", project);
            } else {
                println!("{} module(s) in {}:\n", modules.len(), project);
                for m in &modules {
                    let desc = if m.description.is_empty() {
                        String::new()
                    } else {
                        format!(" - {}", m.description.lines().next().unwrap_or(""))
                    };
                    println!("  {:<20} [{}]{}", m.name, m.status, desc);
                }
            }
        }

        ModuleAction::Create {
            project,
            name,
            description,
            status,
        } => {
            let conn = pool.write()?;
            let project_id = queries::resolve_project_identifier(&conn, project)?;
            let module = queries::create_module(
                &conn,
                &CreateModule {
                    project_id,
                    name: name.clone(),
                    description: description.clone(),
                    status: status.clone(),
                },
            )?;

            if json {
                print_json(&module);
            } else {
                println!(
                    "Created module '{}' [{}] in {}",
                    module.name, module.status, project
                );
            }
        }

        ModuleAction::Update {
            project,
            name,
            new_name,
            description,
            status,
        } => {
            let conn = pool.write()?;
            let project_id = queries::resolve_project_identifier(&conn, project)?;
            let module_id = queries::resolve_module_name(&conn, project_id, name)?;
            let module = queries::update_module(
                &conn,
                module_id,
                &UpdateModule {
                    name: new_name.clone(),
                    description: description.clone(),
                    status: status.clone(),
                },
            )?;

            if json {
                print_json(&module);
            } else {
                println!("Updated module '{}' [{}]", module.name, module.status);
            }
        }

        ModuleAction::Delete { project, name } => {
            let conn = pool.write()?;
            let project_id = queries::resolve_project_identifier(&conn, project)?;
            let module_id = queries::resolve_module_name(&conn, project_id, name)?;
            queries::delete_module(&conn, module_id)?;

            if json {
                println!("{{\"deleted\": true, \"name\": {:?}}}", name);
            } else {
                println!("Deleted module '{}'", name);
            }
        }
    }
    Ok(())
}

// ── Label ────────────────────────────────────────────────────

fn label(
    pool: &DbPool,
    action: &LabelAction,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        LabelAction::List { project } => {
            let conn = pool.read()?;
            let project_id = queries::resolve_project_identifier(&conn, project)?;
            let labels = queries::list_labels(&conn, project_id)?;

            if json {
                print_json(&labels);
            } else if labels.is_empty() {
                println!("No labels in {}.", project);
            } else {
                println!("{} label(s) in {}:\n", labels.len(), project);
                for l in &labels {
                    println!("  {} ({})", l.name, l.color);
                }
            }
        }

        LabelAction::Create {
            project,
            name,
            color,
        } => {
            let conn = pool.write()?;
            let project_id = queries::resolve_project_identifier(&conn, project)?;
            let label = queries::create_label(
                &conn,
                &CreateLabel {
                    project_id,
                    name: name.clone(),
                    color: color.clone(),
                },
            )?;

            if json {
                print_json(&label);
            } else {
                println!("Created label '{}' ({})", label.name, label.color);
            }
        }

        LabelAction::Update {
            project,
            name,
            new_name,
            color,
        } => {
            let conn = pool.write()?;
            let project_id = queries::resolve_project_identifier(&conn, project)?;
            let label_id = queries::resolve_label_name(&conn, project_id, name)?;
            let label = queries::update_label(
                &conn,
                label_id,
                &UpdateLabel {
                    name: new_name.clone(),
                    color: color.clone(),
                },
            )?;

            if json {
                print_json(&label);
            } else {
                println!("Updated label '{}' ({})", label.name, label.color);
            }
        }

        LabelAction::Delete { project, name } => {
            let conn = pool.write()?;
            let project_id = queries::resolve_project_identifier(&conn, project)?;
            let label_id = queries::resolve_label_name(&conn, project_id, name)?;
            queries::delete_label(&conn, label_id)?;

            if json {
                println!("{{\"deleted\": true, \"name\": {:?}}}", name);
            } else {
                println!("Deleted label '{}'", name);
            }
        }
    }
    Ok(())
}

// ── Folder ───────────────────────────────────────────────────

fn folder(
    pool: &DbPool,
    action: &FolderAction,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        FolderAction::List { project } => {
            let conn = pool.read()?;
            let project_id = queries::resolve_project_identifier(&conn, project)?;
            let folders = queries::list_folders(&conn, project_id)?;

            if json {
                print_json(&folders);
            } else if folders.is_empty() {
                println!("No folders in {}.", project);
            } else {
                println!("{} folder(s) in {}:\n", folders.len(), project);
                for f in &folders {
                    println!("  {}", f.name);
                }
            }
        }

        FolderAction::Create { project, name } => {
            let conn = pool.write()?;
            let project_id = queries::resolve_project_identifier(&conn, project)?;
            let folder = queries::create_folder(
                &conn,
                &CreateFolder {
                    project_id,
                    parent_id: None,
                    name: name.clone(),
                },
            )?;

            if json {
                print_json(&folder);
            } else {
                println!("Created folder '{}'", folder.name);
            }
        }

        FolderAction::Update {
            project,
            name,
            new_name,
        } => {
            let conn = pool.write()?;
            let project_id = queries::resolve_project_identifier(&conn, project)?;
            let folder_id = queries::resolve_folder_name(&conn, project_id, name)?;
            let folder = queries::update_folder(
                &conn,
                folder_id,
                &UpdateFolder {
                    name: Some(new_name.clone()),
                },
            )?;

            if json {
                print_json(&folder);
            } else {
                println!("Renamed folder '{}' -> '{}'", name, folder.name);
            }
        }

        FolderAction::Delete { project, name } => {
            let conn = pool.write()?;
            let project_id = queries::resolve_project_identifier(&conn, project)?;
            let folder_id = queries::resolve_folder_name(&conn, project_id, name)?;
            queries::delete_folder(&conn, folder_id)?;

            if json {
                println!("{{\"deleted\": true, \"name\": {:?}}}", name);
            } else {
                println!("Deleted folder '{}'", name);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::db;
    use crate::db::queries;

    use super::*;

    fn test_pool() -> DbPool {
        db::open_memory().expect("test db")
    }

    fn seed_project(pool: &DbPool, ident: &str) {
        let conn = pool.write().unwrap();
        queries::create_project(
            &conn,
            &CreateProject {
                name: format!("Project {ident}"),
                identifier: ident.into(),
                description: String::new(),
                emoji: None,
                lead_user_id: None,
            },
        )
        .unwrap();
    }

    fn seed_issue(pool: &DbPool, project_ident: &str, title: &str) {
        let conn = pool.write().unwrap();
        let pid = queries::resolve_project_identifier(&conn, project_ident).unwrap();
        queries::create_issue(
            &conn,
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
            },
        )
        .unwrap();
    }

    fn seed_user(pool: &DbPool) {
        let conn = pool.write().unwrap();
        queries::users::create_user(
            &conn,
            &CreateUser {
                username: "testuser".into(),
                email: "test@test.com".into(),
                password: "testpass123".into(),
                display_name: Some("Test User".into()),
                is_admin: true,
                is_bot: false,
            },
        )
        .unwrap();
    }

    #[test]
    fn exec_project_create_and_list() {
        let pool = test_pool();
        let cmd = Command::Project {
            action: ProjectAction::Create {
                name: "Test".into(),
                identifier: "TST".into(),
                description: "A test".into(),
            },
        };
        run(&pool, &cmd, false).unwrap();

        // Verify it was created
        let conn = pool.read().unwrap();
        let projects = queries::list_projects(&conn).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].identifier, "TST");
    }

    #[test]
    fn exec_project_list_json() {
        let pool = test_pool();
        seed_project(&pool, "LIF");
        let cmd = Command::Project {
            action: ProjectAction::List,
        };
        // Should not panic
        run(&pool, &cmd, true).unwrap();
    }

    #[test]
    fn exec_issue_create_and_get() {
        let pool = test_pool();
        seed_project(&pool, "TST");

        let cmd = Command::Issue {
            action: IssueAction::Create {
                project: "TST".into(),
                title: "Fix the bug".into(),
                description: "It's broken".into(),
                status: "todo".into(),
                priority: "high".into(),
                module: None,
                labels: None,
            },
        };
        run(&pool, &cmd, false).unwrap();

        // Get it
        let cmd = Command::Issue {
            action: IssueAction::Get {
                identifier: "TST-1".into(),
            },
        };
        run(&pool, &cmd, false).unwrap();
    }

    #[test]
    fn exec_issue_update() {
        let pool = test_pool();
        seed_project(&pool, "TST");
        seed_issue(&pool, "TST", "Original");

        let cmd = Command::Issue {
            action: IssueAction::Update {
                identifier: "TST-1".into(),
                title: Some("Updated".into()),
                description: None,
                status: Some("active".into()),
                priority: None,
                module: None,
                labels: None,
            },
        };
        run(&pool, &cmd, false).unwrap();

        let conn = pool.read().unwrap();
        let id = queries::resolve_identifier(&conn, "TST-1").unwrap();
        let issue = queries::get_issue(&conn, id).unwrap();
        assert_eq!(issue.title, "Updated");
        assert_eq!(issue.status, "active");
    }

    #[test]
    fn exec_issue_list_with_filters() {
        let pool = test_pool();
        seed_project(&pool, "TST");
        {
            let conn = pool.write().unwrap();
            let pid = queries::resolve_project_identifier(&conn, "TST").unwrap();
            queries::create_issue(
                &conn,
                &CreateIssue {
                    project_id: pid,
                    title: "Active one".into(),
                    description: String::new(),
                    status: "active".into(),
                    priority: "high".into(),
                    module_id: None,
                    start_date: None,
                    target_date: None,
                    labels: vec![],
                },
            )
            .unwrap();
            queries::create_issue(
                &conn,
                &CreateIssue {
                    project_id: pid,
                    title: "Done one".into(),
                    description: String::new(),
                    status: "done".into(),
                    priority: "low".into(),
                    module_id: None,
                    start_date: None,
                    target_date: None,
                    labels: vec![],
                },
            )
            .unwrap();
        }

        let cmd = Command::Issue {
            action: IssueAction::List {
                project: "TST".into(),
                status: Some("active".into()),
                priority: None,
                module: None,
                label: None,
                workable: false,
                limit: None,
            },
        };
        run(&pool, &cmd, false).unwrap();
    }

    #[test]
    fn exec_search() {
        let pool = test_pool();
        seed_project(&pool, "TST");
        seed_issue(&pool, "TST", "Implement authentication");

        let cmd = Command::Search {
            query: "auth".into(),
            project: Some("TST".into()),
            limit: None,
        };
        run(&pool, &cmd, false).unwrap();
    }

    #[test]
    fn exec_page_create_and_get() {
        let pool = test_pool();
        seed_project(&pool, "TST");

        let cmd = Command::Page {
            action: PageAction::Create {
                title: "Design Doc".into(),
                project: Some("TST".into()),
                folder: None,
                content: "# Architecture\n\nOverview".into(),
            },
        };
        run(&pool, &cmd, false).unwrap();

        let cmd = Command::Page {
            action: PageAction::Get {
                identifier: "TST-DOC-1".into(),
            },
        };
        run(&pool, &cmd, false).unwrap();
    }

    #[test]
    fn exec_export_project_writes_files() {
        let pool = test_pool();
        seed_project(&pool, "TST");
        seed_issue(&pool, "TST", "Export this issue");

        let tmp = std::env::temp_dir().join(format!("lific-export-test-{}", std::process::id()));
        if tmp.exists() {
            std::fs::remove_dir_all(&tmp).unwrap();
        }

        let cmd = Command::Export {
            action: ExportAction::Project {
                project: "TST".into(),
                output: tmp.clone(),
            },
        };
        run(&pool, &cmd, false).unwrap();

        let issue_path = tmp.join("TST/issues/tst-1-export-this-issue.md");
        assert!(issue_path.exists());
        let content = std::fs::read_to_string(issue_path).unwrap();
        assert!(content.contains("identifier: TST-1"));

        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[test]
    fn exec_comment_add_and_list() {
        let pool = test_pool();
        seed_project(&pool, "TST");
        seed_issue(&pool, "TST", "Test issue");
        seed_user(&pool);

        let cmd = Command::Comment {
            action: CommentAction::Add {
                identifier: "TST-1".into(),
                content: "Looking into this".into(),
                user: Some("testuser".into()),
            },
        };
        run(&pool, &cmd, false).unwrap();

        let cmd = Command::Comment {
            action: CommentAction::List {
                identifier: "TST-1".into(),
            },
        };
        run(&pool, &cmd, false).unwrap();
    }

    #[test]
    fn exec_module_crud() {
        let pool = test_pool();
        seed_project(&pool, "TST");

        // Create
        let cmd = Command::Module {
            action: ModuleAction::Create {
                project: "TST".into(),
                name: "Core".into(),
                description: "The core".into(),
                status: "active".into(),
            },
        };
        run(&pool, &cmd, false).unwrap();

        // List
        let cmd = Command::Module {
            action: ModuleAction::List {
                project: "TST".into(),
            },
        };
        run(&pool, &cmd, false).unwrap();

        // Update
        let cmd = Command::Module {
            action: ModuleAction::Update {
                project: "TST".into(),
                name: "Core".into(),
                new_name: Some("Core DB".into()),
                description: None,
                status: Some("done".into()),
            },
        };
        run(&pool, &cmd, false).unwrap();

        // Delete
        let cmd = Command::Module {
            action: ModuleAction::Delete {
                project: "TST".into(),
                name: "Core DB".into(),
            },
        };
        run(&pool, &cmd, false).unwrap();
    }

    #[test]
    fn exec_label_crud() {
        let pool = test_pool();
        seed_project(&pool, "TST");

        // Create
        let cmd = Command::Label {
            action: LabelAction::Create {
                project: "TST".into(),
                name: "bug".into(),
                color: "#EF4444".into(),
            },
        };
        run(&pool, &cmd, false).unwrap();

        // List
        let cmd = Command::Label {
            action: LabelAction::List {
                project: "TST".into(),
            },
        };
        run(&pool, &cmd, false).unwrap();

        // Update
        let cmd = Command::Label {
            action: LabelAction::Update {
                project: "TST".into(),
                name: "bug".into(),
                new_name: Some("defect".into()),
                color: None,
            },
        };
        run(&pool, &cmd, false).unwrap();

        // Delete
        let cmd = Command::Label {
            action: LabelAction::Delete {
                project: "TST".into(),
                name: "defect".into(),
            },
        };
        run(&pool, &cmd, false).unwrap();
    }

    #[test]
    fn exec_folder_crud() {
        let pool = test_pool();
        seed_project(&pool, "TST");

        // Create
        let cmd = Command::Folder {
            action: FolderAction::Create {
                project: "TST".into(),
                name: "Docs".into(),
            },
        };
        run(&pool, &cmd, false).unwrap();

        // List
        let cmd = Command::Folder {
            action: FolderAction::List {
                project: "TST".into(),
            },
        };
        run(&pool, &cmd, false).unwrap();

        // Update
        let cmd = Command::Folder {
            action: FolderAction::Update {
                project: "TST".into(),
                name: "Docs".into(),
                new_name: "Documentation".into(),
            },
        };
        run(&pool, &cmd, false).unwrap();

        // Delete
        let cmd = Command::Folder {
            action: FolderAction::Delete {
                project: "TST".into(),
                name: "Documentation".into(),
            },
        };
        run(&pool, &cmd, false).unwrap();
    }

    #[test]
    fn exec_issue_create_with_labels() {
        let pool = test_pool();
        seed_project(&pool, "TST");

        // Create labels first
        {
            let conn = pool.write().unwrap();
            let pid = queries::resolve_project_identifier(&conn, "TST").unwrap();
            queries::create_label(
                &conn,
                &CreateLabel {
                    project_id: pid,
                    name: "bug".into(),
                    color: "#EF4444".into(),
                },
            )
            .unwrap();
            queries::create_label(
                &conn,
                &CreateLabel {
                    project_id: pid,
                    name: "urgent".into(),
                    color: "#F59E0B".into(),
                },
            )
            .unwrap();
        }

        let cmd = Command::Issue {
            action: IssueAction::Create {
                project: "TST".into(),
                title: "Labeled issue".into(),
                description: String::new(),
                status: "backlog".into(),
                priority: "none".into(),
                module: None,
                labels: Some("bug,urgent".into()),
            },
        };
        run(&pool, &cmd, false).unwrap();

        let conn = pool.read().unwrap();
        let id = queries::resolve_identifier(&conn, "TST-1").unwrap();
        let issue = queries::get_issue(&conn, id).unwrap();
        assert_eq!(issue.labels.len(), 2);
        assert!(issue.labels.contains(&"bug".to_string()));
        assert!(issue.labels.contains(&"urgent".to_string()));
    }

    #[test]
    fn exec_json_output_parses() {
        let pool = test_pool();
        seed_project(&pool, "TST");
        seed_issue(&pool, "TST", "JSON test");

        // This should produce valid JSON — we just verify no panic
        let cmd = Command::Issue {
            action: IssueAction::Get {
                identifier: "TST-1".into(),
            },
        };
        run(&pool, &cmd, true).unwrap();
    }

    #[test]
    fn exec_project_get() {
        let pool = test_pool();
        seed_project(&pool, "TST");

        let cmd = Command::Project {
            action: ProjectAction::Get {
                identifier: "TST".into(),
            },
        };
        run(&pool, &cmd, false).unwrap();
    }

    #[test]
    fn exec_not_found_errors() {
        let pool = test_pool();

        let cmd = Command::Issue {
            action: IssueAction::Get {
                identifier: "NOPE-1".into(),
            },
        };
        assert!(run(&pool, &cmd, false).is_err());
    }
}
