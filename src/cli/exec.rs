use crate::db::DbPool;
use crate::db::models::*;
use crate::db::queries;
use crate::error::LificError;

use super::render;
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
        _ => unreachable!(
            "non-CRUD commands are dispatched by main.rs to their own modules \
             (cli::instance, cli::key, cli::user, cli::member, server::run, ...)"
        ),
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
        print!("{}", render::export_written(&written, output));
    }
    Ok(())
}

// ── Helpers ──────────────────────────────────────────────────

fn print_json<T: serde::Serialize>(val: &T) {
    println!("{}", serde_json::to_string_pretty(val).unwrap());
}

fn page_folder_id(
    conn: &rusqlite::Connection,
    page_id: i64,
    name: &str,
) -> Result<i64, Box<dyn std::error::Error>> {
    let page = queries::get_page(conn, page_id)?;
    page.project_id
        .map(|project_id| queries::resolve_folder_name(conn, project_id, name))
        .transpose()?
        .ok_or_else(|| "cannot set folder on workspace page".into())
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

            let module_id = module
                .as_deref()
                .map(|name| queries::resolve_module_name(&conn, project_id, name))
                .transpose()?;

            let issues = queries::list_issues(
                &conn,
                &ListIssuesQuery {
                    project_id: Some(project_id),
                    status: Status::parse_opt(status.as_deref())?,
                    priority: Priority::parse_opt(priority.as_deref())?,
                    module_id,
                    label: label.clone(),
                    workable: if *workable { Some(true) } else { None },
                    limit: *limit,
                    ..Default::default()
                },
            )?;

            if json {
                print_json(&issues);
            } else {
                let module_name = |id: i64| queries::get_module_name(&conn, id).ok();
                print!("{}", render::issue_list(&issues, &module_name));
            }
        }

        IssueAction::Get { identifier } => {
            let conn = pool.read()?;
            let id = queries::resolve_identifier(&conn, identifier)?;
            let issue = queries::get_issue(&conn, id)?;

            if json {
                print_json(&issue);
            } else {
                let module_name = |id: i64| queries::get_module_name(&conn, id).ok();
                print!("{}", render::issue_detail(&issue, &module_name));
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

            let module_id = module
                .as_deref()
                .map(|name| queries::resolve_module_name(&conn, project_id, name))
                .transpose()?;

            let label_list = owned_labels(labels.as_deref()).unwrap_or_default();

            let issue = queries::create_issue(
                &conn,
                &CreateIssue {
                    project_id,
                    title: title.clone(),
                    description: description.clone(),
                    status: status.parse()?,
                    priority: priority.parse()?,
                    module_id,
                    labels: label_list,
                    ..Default::default()
                },
            )?;

            if json {
                print_json(&issue);
            } else {
                print!("{}", render::issue_created(&issue));
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

            let label_list = owned_labels(labels.as_deref());

            let issue = queries::update_issue(
                &conn,
                id,
                &UpdateIssue {
                    title: title.clone(),
                    description: description.clone(),
                    status: Status::parse_opt(status.as_deref())?,
                    priority: Priority::parse_opt(priority.as_deref())?,
                    // LIF-145: module_id is now tristate; the CLI only sets or
                    // skips (no clear), so map Some(id) -> Some(Some(id)).
                    module_id: module_id.map(Some),
                    labels: label_list,
                    ..Default::default()
                },
            )?;

            if json {
                print_json(&issue);
            } else {
                print!("{}", render::issue_updated(&issue));
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
            } else {
                print!("{}", render::project_list(&projects));
            }
        }

        ProjectAction::Get { identifier } => {
            let conn = pool.read()?;
            let id = queries::resolve_project_identifier(&conn, identifier)?;
            let project = queries::get_project(&conn, id)?;

            if json {
                print_json(&project);
            } else {
                print!("{}", render::project_detail(&project));
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
                    ..Default::default()
                },
            )?;

            if json {
                print_json(&project);
            } else {
                print!("{}", render::project_created(&project));
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
                    description: description.clone(),
                    ..Default::default()
                },
            )?;

            if json {
                print_json(&project);
            } else {
                print!("{}", render::project_updated(&project));
            }
        }
    }
    Ok(())
}

// ── Page ─────────────────────────────────────────────────────

fn page(pool: &DbPool, action: &PageAction, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        PageAction::List {
            project,
            folder,
            label,
        } => {
            let conn = pool.read()?;
            let project_id = project
                .as_deref()
                .map(|ident| queries::resolve_project_identifier(&conn, ident))
                .transpose()?;

            let folder_id = project_id
                .zip(folder.as_deref())
                .map(|(project_id, name)| queries::resolve_folder_name(&conn, project_id, name))
                .transpose()?;

            let pages = queries::list_pages(
                &conn,
                project_id,
                folder_id,
                label.as_deref(),
                None,
                None,
                None,
                None,
                None,
            )?;

            if json {
                print_json(&pages);
            } else {
                print!("{}", render::page_list(&pages));
            }
        }

        PageAction::Get { identifier } => {
            let conn = pool.read()?;
            let id = queries::resolve_page_identifier(&conn, identifier)?;
            let page = queries::get_page(&conn, id)?;

            if json {
                print_json(&page);
            } else {
                print!("{}", render::page_detail(&page));
            }
        }

        PageAction::Create {
            title,
            project,
            folder,
            content,
            labels,
        } => {
            let conn = pool.write()?;
            let project_id = project
                .as_deref()
                .map(|ident| queries::resolve_project_identifier(&conn, ident))
                .transpose()?;

            let folder_id = project_id
                .zip(folder.as_deref())
                .map(|(project_id, name)| queries::resolve_folder_name(&conn, project_id, name))
                .transpose()?;

            // Same comma-split shape `issue create` uses, so users get
            // one mental model across both CLIs.
            let label_list = owned_labels(labels.as_deref()).unwrap_or_default();

            let page = queries::create_page(
                &conn,
                &CreatePage {
                    project_id,
                    folder_id,
                    title: title.clone(),
                    content: content.clone(),
                    labels: label_list,
                    ..Default::default()
                },
            )?;

            if json {
                print_json(&page);
            } else {
                print!("{}", render::page_created(&page));
            }
        }

        PageAction::Update {
            identifier,
            title,
            content,
            folder,
            labels,
        } => {
            let conn = pool.write()?;
            let id = queries::resolve_page_identifier(&conn, identifier)?;

            let folder_id = folder
                .as_deref()
                .map(|name| page_folder_id(&conn, id, name))
                .transpose()?
                .map(Some);

            let label_list = owned_labels(labels.as_deref());

            let page = queries::update_page(
                &conn,
                id,
                &UpdatePage {
                    title: title.clone(),
                    content: content.clone(),
                    folder_id,
                    labels: label_list,
                    ..Default::default()
                },
            )?;

            if json {
                print_json(&page);
            } else {
                print!("{}", render::page_updated(&page));
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
    let project_id = project
        .map(|ident| queries::resolve_project_identifier(&conn, ident))
        .transpose()?;

    let results = queries::search(
        &conn,
        &SearchQuery {
            query: query.to_string(),
            project_id,
            limit,
            ..Default::default()
        },
    )?;

    if json {
        print_json(&results);
    } else {
        print!("{}", render::search_results(&results));
    }
    Ok(())
}

// ── Comment ──────────────────────────────────────────────────

/// One page of an issue's comments, plus what lies past it.
///
/// `limit`/`offset` go through the shared clamp (1..=500, offset floored at
/// 0), and `list_comments_page` over-fetches one row inside the query, so
/// `has_more` stays right even at the cap. The direct-SQL backend therefore
/// always *knows* the answer and never returns
/// [`CommentContinuation::Unknown`].
fn comment_page(
    conn: &rusqlite::Connection,
    issue_id: i64,
    limit: i64,
    offset: i64,
    order: &str,
) -> Result<(Vec<Comment>, render::CommentContinuation), LificError> {
    let (limit, offset) = queries::page(Some(limit), Some(offset));
    let page = queries::comments::list_comments_page(
        conn,
        queries::comments::CommentParent::Issue(issue_id),
        None,
        Some(order),
        Some(limit),
        Some(offset),
    )?;
    let continuation = if page.has_more {
        render::CommentContinuation::Next(offset + page.items.len() as i64)
    } else {
        render::CommentContinuation::End
    };
    Ok((page.items, continuation))
}

fn comment(
    pool: &DbPool,
    action: &CommentAction,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        CommentAction::List {
            identifier,
            limit,
            offset,
            order,
        } => {
            let conn = pool.read()?;
            let id = queries::resolve_identifier(&conn, identifier)?;
            let (comments, continuation) = comment_page(&conn, id, *limit, *offset, order)?;

            if json {
                print_json(&comments);
            } else {
                print!(
                    "{}",
                    render::comment_list(&comments, identifier, continuation)
                );
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

            let comment = queries::comments::create_comment(
                &conn,
                queries::comments::CommentParent::Issue(issue_id),
                user_id,
                content,
            )?;

            if json {
                print_json(&comment);
            } else {
                print!("{}", render::comment_added(&comment, identifier));
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
            } else {
                print!("{}", render::module_list(&modules, project));
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
                    emoji: None,
                },
            )?;

            if json {
                print_json(&module);
            } else {
                print!("{}", render::module_created(&module, project));
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
                    ..Default::default()
                },
            )?;

            if json {
                print_json(&module);
            } else {
                print!("{}", render::module_updated(&module));
            }
        }

        ModuleAction::Delete { project, name } => {
            let conn = pool.write()?;
            let project_id = queries::resolve_project_identifier(&conn, project)?;
            let module_id = queries::resolve_module_name(&conn, project_id, name)?;
            queries::delete_module(&conn, module_id)?;

            if json {
                print_json(&render::Deleted::named(name));
            } else {
                print!("{}", render::module_deleted(name));
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
            } else {
                print!("{}", render::label_list(&labels, project));
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
                print!("{}", render::label_created(&label));
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
                print!("{}", render::label_updated(&label));
            }
        }

        LabelAction::Delete { project, name } => {
            let conn = pool.write()?;
            let project_id = queries::resolve_project_identifier(&conn, project)?;
            let label_id = queries::resolve_label_name(&conn, project_id, name)?;
            queries::delete_label(&conn, label_id)?;

            if json {
                print_json(&render::Deleted::named(name));
            } else {
                print!("{}", render::label_deleted(name));
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
            } else {
                print!("{}", render::folder_list(&folders, project));
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
                print!("{}", render::folder_created(&folder));
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
                print!("{}", render::folder_updated(name, &folder));
            }
        }

        FolderAction::Delete { project, name } => {
            let conn = pool.write()?;
            let project_id = queries::resolve_project_identifier(&conn, project)?;
            let folder_id = queries::resolve_folder_name(&conn, project_id, name)?;
            queries::delete_folder(&conn, folder_id)?;

            if json {
                print_json(&render::Deleted::named(name));
            } else {
                print!("{}", render::folder_deleted(name));
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
                ..Default::default()
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
                ..Default::default()
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
        assert_eq!(issue.status, Status::Active);
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
                    status: Status::Active,
                    priority: Priority::High,
                    ..Default::default()
                },
            )
            .unwrap();
            queries::create_issue(
                &conn,
                &CreateIssue {
                    project_id: pid,
                    title: "Done one".into(),
                    status: Status::Done,
                    priority: Priority::Low,
                    ..Default::default()
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
                labels: None,
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
    fn exec_workspace_page_folder_update_returns_clear_error() {
        let pool = test_pool();
        run(
            &pool,
            &Command::Page {
                action: PageAction::Create {
                    title: "Workspace doc".into(),
                    project: None,
                    folder: None,
                    content: String::new(),
                    labels: None,
                },
            },
            true,
        )
        .unwrap();

        let error = run(
            &pool,
            &Command::Page {
                action: PageAction::Update {
                    identifier: "DOC-1".into(),
                    title: None,
                    content: None,
                    folder: Some("folder".into()),
                    labels: None,
                },
            },
            true,
        )
        .unwrap_err();

        assert_eq!(error.to_string(), "cannot set folder on workspace page");
    }

    #[test]
    fn exec_export_project_writes_files() {
        let pool = test_pool();
        seed_project(&pool, "TST");
        seed_issue(&pool, "TST", "Export this issue");

        let guard = tempfile::tempdir().unwrap();
        // A subdirectory that does not exist yet: the export command creates
        // its own output directory.
        let tmp = guard.path().join("export");

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
                limit: queries::DEFAULT_PAGE_LIMIT,
                offset: 0,
                order: "desc".into(),
            },
        };
        run(&pool, &cmd, false).unwrap();
    }

    /// Seed `n` comments on TST-1 with distinct timestamps, so ordering
    /// assertions do not depend on `datetime('now')`'s one-second grain.
    fn seed_comment_trail(pool: &DbPool, n: i64) {
        let conn = pool.write().unwrap();
        let issue_id = queries::resolve_identifier(&conn, "TST-1").unwrap();
        let user_id = queries::users::list_users(&conn).unwrap()[0].id;
        for i in 1..=n {
            let comment = queries::comments::create_comment(
                &conn,
                queries::comments::CommentParent::Issue(issue_id),
                user_id,
                &format!("comment {i}"),
            )
            .unwrap();
            conn.execute(
                "UPDATE comments SET created_at = ?1 WHERE id = ?2",
                rusqlite::params![format!("2026-01-01 00:{:02}:{:02}", i / 60, i % 60), comment.id],
            )
            .unwrap();
        }
    }

    /// `comment list` is newest-first and bounded by default. An unbounded
    /// oldest-first dump is exactly what a long thread cannot afford, and
    /// the newest comment is the one a reader wants first.
    #[test]
    fn comment_list_defaults_to_the_newest_page() {
        let pool = test_pool();
        seed_project(&pool, "TST");
        seed_issue(&pool, "TST", "Chatty");
        seed_user(&pool);
        let total = queries::DEFAULT_PAGE_LIMIT + 3;
        seed_comment_trail(&pool, total);

        let conn = pool.read().unwrap();
        let issue_id = queries::resolve_identifier(&conn, "TST-1").unwrap();
        let (comments, continuation) =
            comment_page(&conn, issue_id, queries::DEFAULT_PAGE_LIMIT, 0, "desc").unwrap();

        assert_eq!(comments.len(), queries::DEFAULT_PAGE_LIMIT as usize);
        assert_eq!(comments[0].content, format!("comment {total}"));
        assert_eq!(
            continuation,
            render::CommentContinuation::Next(queries::DEFAULT_PAGE_LIMIT)
        );

        // The hint is what makes the truncation discoverable at all.
        let rendered = render::comment_list(&comments, "TST-1", continuation);
        assert!(
            rendered.contains(&format!(
                "More comments available. Next page: --offset {}",
                queries::DEFAULT_PAGE_LIMIT
            )),
            "got: {rendered}"
        );

        // The next page finishes the thread, so it carries no hint.
        let (tail, continuation) = comment_page(
            &conn,
            issue_id,
            queries::DEFAULT_PAGE_LIMIT,
            queries::DEFAULT_PAGE_LIMIT,
            "desc",
        )
        .unwrap();
        assert_eq!(tail.len(), 3);
        assert_eq!(tail.last().unwrap().content, "comment 1");
        assert_eq!(continuation, render::CommentContinuation::End);
        assert!(
            !render::comment_list(&tail, "TST-1", continuation).contains("More comments"),
            "a final page must not advertise another one"
        );
    }

    #[test]
    fn comment_list_honours_asc_and_floors_a_negative_offset() {
        let pool = test_pool();
        seed_project(&pool, "TST");
        seed_issue(&pool, "TST", "Chatty");
        seed_user(&pool);
        seed_comment_trail(&pool, 5);

        let conn = pool.read().unwrap();
        let issue_id = queries::resolve_identifier(&conn, "TST-1").unwrap();

        let (comments, _) = comment_page(&conn, issue_id, 10, 0, "asc").unwrap();
        assert_eq!(comments.len(), 5);
        assert_eq!(comments[0].content, "comment 1");

        // A negative offset floors at 0 rather than reaching SQL.
        let (comments, _) = comment_page(&conn, issue_id, 2, -10, "asc").unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].content, "comment 1");

        assert!(comment_page(&conn, issue_id, 2, 0, "sideways").is_err());
    }

    /// `--limit 100000` is clamped to the shared 500-row cap, and the page
    /// still knows a row sits past it.
    #[test]
    fn comment_list_clamps_limit_to_the_shared_cap() {
        let pool = test_pool();
        seed_project(&pool, "TST");
        seed_issue(&pool, "TST", "Chatty");
        seed_user(&pool);
        {
            let conn = pool.write().unwrap();
            let issue_id = queries::resolve_identifier(&conn, "TST-1").unwrap();
            let user_id = queries::users::list_users(&conn).unwrap()[0].id;
            // Inserted directly: this test is about the LIMIT arithmetic,
            // not about comment creation.
            for n in 0..=queries::MAX_PAGE_LIMIT {
                conn.execute(
                    "INSERT INTO comments (issue_id, user_id, content) VALUES (?1, ?2, ?3)",
                    rusqlite::params![issue_id, user_id, format!("comment {n}")],
                )
                .unwrap();
            }
        }

        let conn = pool.read().unwrap();
        let issue_id = queries::resolve_identifier(&conn, "TST-1").unwrap();
        let (comments, continuation) = comment_page(&conn, issue_id, 100_000, 0, "desc").unwrap();
        assert_eq!(comments.len() as i64, queries::MAX_PAGE_LIMIT);
        assert_eq!(
            continuation,
            render::CommentContinuation::Next(queries::MAX_PAGE_LIMIT)
        );
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
