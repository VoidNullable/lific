use crate::db::DbPool;
use crate::db::models::*;
use crate::db::queries;
use crate::error::LificError;
use crate::links::IssueLinkContext;

use super::render;
use super::weblinks::{self, IssueLinkOutput, ResourceKind};
use super::*;

/// Whoever runs `lific` against the database file directly.
///
/// This backend has no authentication, and needs none: opening the SQLite file
/// for writing already grants every row. So the operator is a *trusted local
/// operator* for authorization purposes ([`AttachmentActor::TrustedLocal`]),
/// and separately resolves to a real user only for the things that genuinely
/// need one — a comment's author, a new project's lead.
///
/// Only **active humans** are candidates. A bot is somebody's tool, not a
/// person who can be handed a project to lead or credited with a comment, and
/// a deactivated account is one an administrator has deliberately taken out of
/// circulation — resurrecting either by position in a list is how an
/// unattended CLI ends up attributing work to an account nobody is watching.
///
/// Among those, the established local-CLI operator wins: the first active
/// human administrator. With no administrator at all, a lone active human is
/// unambiguous and is used. Several equally plausible non-administrators is
/// genuinely ambiguous, and the guess is refused out loud rather than resolved
/// by row order.
///
/// `None` means the database has no active human at all. That is a real state
/// on a freshly initialized database, handled explicitly at each call site
/// rather than by inventing a user: `comment add` refuses (a comment must have
/// an author), `project create` proceeds leaderless (a project need not have
/// one, and the first administrator created can reach it anyway).
fn effective_operator(conn: &rusqlite::Connection) -> Result<Option<CommentActor>, LificError> {
    let users = queries::users::list_users(conn)?;
    let humans: Vec<_> = users.iter().filter(|u| u.is_active && !u.is_bot).collect();

    let actor = |u: &crate::db::models::User| CommentActor {
        user_id: u.id,
        is_admin: u.is_admin,
    };

    if let Some(admin) = humans.iter().find(|u| u.is_admin) {
        return Ok(Some(actor(admin)));
    }
    match humans.as_slice() {
        [] => Ok(None),
        [only] => Ok(Some(actor(only))),
        several => Err(LificError::BadRequest(format!(
            "cannot infer which user to act as: {} active non-admin users and no admin. \
             Pass --user, or promote one of them to admin",
            several.len()
        ))),
    }
}

fn require_operator(conn: &rusqlite::Connection) -> Result<CommentActor, LificError> {
    effective_operator(conn)?
        .ok_or_else(|| LificError::NotFound("no users exist; create a user first".into()))
}

/// Run a CLI CRUD command against the database.
/// Returns Ok(()) on success, printing output to stdout.
///
/// `links` is the web UI this instance is reachable at, from
/// `server.public_url`. It is `None` when that is unset, and then JSON output
/// simply carries no `web_url`: this backend talks to a database file, not to
/// a server, so there is no origin to infer and fabricating one would hand out
/// links that go nowhere.
pub fn run(
    pool: &DbPool,
    command: &Command,
    json: bool,
    links: Option<&IssueLinkContext>,
) -> Result<(), Box<dyn std::error::Error>> {
    let out = Output { json, links };
    match command {
        Command::Issue { action } => issue(pool, action, out),
        Command::Project { action } => project(pool, action, out),
        Command::Page { action } => page(pool, action, out),
        Command::Export { action } => export(pool, action, json),
        Command::Search {
            query,
            project,
            limit,
        } => search(pool, query, project.as_deref(), *limit, out),
        Command::Comment { action } => comment(pool, action, out),
        Command::Module { action } => module(pool, action, out),
        Command::Label { action } => label(pool, action, json),
        Command::Folder { action } => folder(pool, action, json),
        Command::Bind { project, create } => Ok(super::bind::run_sql(
            pool,
            project.as_deref(),
            *create,
            json,
        )?),
        Command::GitHook { range, dry_run } => Ok(super::git_hook::run_sql(
            pool,
            range.as_deref(),
            *dry_run,
            json,
        )?),
        _ => unreachable!(
            "non-CRUD commands are dispatched by main.rs to their own modules \
             (cli::instance, cli::key, cli::user, cli::member, server::run, ...)"
        ),
    }
}

/// Where a command's output goes, and what it may be enriched with.
///
/// Only the JSON side is enriched. Human output already renders identifiers
/// the way this backend has always rendered them, and the HTTP backend's
/// markdown mode exists to make identifiers clickable in an agent's transcript
/// — a different job from "machine output carries a link".
#[derive(Clone, Copy)]
struct Output<'a> {
    json: bool,
    links: Option<&'a IssueLinkContext>,
}

impl Output<'_> {
    /// Serialize, enrich, print. Split from the three `enrich_*` helpers so the
    /// enrichment can be asserted against the HTTP backend's without capturing
    /// stdout.
    fn emit(self, value: serde_json::Value) {
        println!("{}", serde_json::to_string_pretty(&value).unwrap());
    }

    /// Serialization of our own models cannot fail.
    fn value<T: serde::Serialize>(value: &T) -> serde_json::Value {
        serde_json::to_value(value).unwrap()
    }

    fn enrich_resources(self, value: serde_json::Value, kind: ResourceKind) -> serde_json::Value {
        match self.links {
            Some(context) => weblinks::linked_resources(value, context, IssueLinkOutput::Url, kind),
            None => value,
        }
    }

    fn json_resources<T: serde::Serialize>(self, value: &T, kind: ResourceKind) {
        self.emit(self.enrich_resources(Self::value(value), kind));
    }

    fn enrich_comments(self, value: serde_json::Value, identifier: &str) -> serde_json::Value {
        match self.links {
            Some(context) => {
                weblinks::linked_comments(value, context, IssueLinkOutput::Url, identifier)
            }
            None => value,
        }
    }

    fn json_comments<T: serde::Serialize>(self, value: &T, identifier: &str) {
        self.emit(self.enrich_comments(Self::value(value), identifier));
    }

    fn enrich_modules(self, value: serde_json::Value, project: &str) -> serde_json::Value {
        match self.links {
            Some(context) => {
                weblinks::linked_modules(value, context, IssueLinkOutput::Url, project)
            }
            None => value,
        }
    }

    fn json_modules<T: serde::Serialize>(self, value: &T, project: &str) {
        self.emit(self.enrich_modules(Self::value(value), project));
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
    println!("{}", term::json_string(val).unwrap());
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
    out: Output<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let json = out.json;
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
                out.json_resources(&issues, ResourceKind::Issue);
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
                out.json_resources(&issue, ResourceKind::Issue);
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
                    // LIF-409: the description's attachment references are
                    // linked by `create_issue` itself. A direct-SQL caller is
                    // past every gate already, so every reference that names a
                    // real attachment is honoured.
                    attachments: AttachmentActor::TrustedLocal,
                    ..Default::default()
                },
            )?;
            drop(conn);

            if json {
                out.json_resources(&issue, ResourceKind::Issue);
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
                    // LIF-409: see `issue create`. An edit that drops a
                    // reference drops its link, same as every other backend.
                    attachments: AttachmentActor::TrustedLocal,
                    ..Default::default()
                },
            )?;
            drop(conn);

            if json {
                out.json_resources(&issue, ResourceKind::Issue);
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
    out: Output<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let json = out.json;
    match action {
        ProjectAction::List => {
            let conn = pool.read()?;
            let projects = queries::list_projects(&conn)?;

            if json {
                out.json_resources(&projects, ResourceKind::Project);
            } else {
                print!("{}", render::project_list(&projects));
            }
        }

        ProjectAction::Get { identifier } => {
            let conn = pool.read()?;
            let id = queries::resolve_project_identifier(&conn, identifier)?;
            let project = queries::get_project(&conn, id)?;

            if json {
                out.json_resources(&project, ResourceKind::Project);
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
            // LIF-409: match `POST /api/projects`, where the creator leads the
            // project it just made and gets the matching `lead` membership
            // row. Without this the CLI produced an unowned project that only
            // an administrator could administer — the LIF-102 bug, still alive
            // on this backend long after REST was fixed.
            //
            // The creator here is the effective local operator. On a database
            // with no users at all there is nobody to name, and the project is
            // created leaderless rather than pointing at an invented id.
            let lead_user_id = effective_operator(&conn)?.map(|operator| operator.user_id);
            let project = queries::create_project(
                &conn,
                &CreateProject {
                    name: name.clone(),
                    identifier: identifier.clone(),
                    description: description.clone(),
                    lead_user_id,
                    ..Default::default()
                },
            )?;
            drop(conn);

            if json {
                out.json_resources(&project, ResourceKind::Project);
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
            drop(conn);

            if json {
                out.json_resources(&project, ResourceKind::Project);
            } else {
                print!("{}", render::project_updated(&project));
            }
        }
    }
    Ok(())
}

// ── Page ─────────────────────────────────────────────────────

fn page(
    pool: &DbPool,
    action: &PageAction,
    out: Output<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let json = out.json;
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
                out.json_resources(&pages, ResourceKind::Page);
            } else {
                print!("{}", render::page_list(&pages));
            }
        }

        PageAction::Get { identifier } => {
            let conn = pool.read()?;
            let id = queries::resolve_page_identifier(&conn, identifier)?;
            let page = queries::get_page(&conn, id)?;

            if json {
                out.json_resources(&page, ResourceKind::Page);
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
                    // LIF-409: see `issue create`.
                    attachments: AttachmentActor::TrustedLocal,
                    ..Default::default()
                },
            )?;
            drop(conn);

            if json {
                out.json_resources(&page, ResourceKind::Page);
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
                    // LIF-409: see `issue update`.
                    attachments: AttachmentActor::TrustedLocal,
                    ..Default::default()
                },
            )?;
            drop(conn);

            if json {
                out.json_resources(&page, ResourceKind::Page);
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
    out: Output<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let json = out.json;
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
        out.json_resources(&results, ResourceKind::Search);
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
    out: Output<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let json = out.json;
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
                out.json_comments(&comments, identifier);
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
            let parent = queries::comments::CommentParent::Issue(issue_id);

            // The comment's *author*: either explicit --user, or the effective
            // local operator. A comment cannot exist without one, so an empty
            // user table is an error here rather than a silent fallback.
            let author = if let Some(username) = user {
                let u = queries::users::get_user_by_username(&conn, username)?;
                CommentActor {
                    user_id: u.id,
                    is_admin: u.is_admin,
                }
            } else {
                require_operator(&conn)?
            };

            // LIF-409: the shared create path, so a CLI comment resolves its
            // @mentions and links its attachment references exactly as a
            // comment posted through REST or MCP does. It self-transacts, so
            // the comment, its mentions and its links land together on this
            // connection, which has no transaction of its own.
            //
            // Authorization stays the trusted local operator's, not the named
            // author's: `--user` says who wrote it, not whose permissions the
            // body's references borrow.
            let project_id = parent.project_id(&conn)?;
            let member_scoped = crate::authz::authz_enforced_conn(&conn)?;
            let comment = queries::comments::create_comment_with_mentions(
                &conn,
                parent,
                project_id,
                author,
                AttachmentActor::TrustedLocal,
                content,
                member_scoped,
            )?;
            drop(conn);

            if json {
                out.json_comments(&comment, identifier);
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
    out: Output<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let json = out.json;
    match action {
        ModuleAction::List { project } => {
            let conn = pool.read()?;
            let project_id = queries::resolve_project_identifier(&conn, project)?;
            let modules = queries::list_modules(&conn, project_id)?;

            if json {
                out.json_modules(&modules, project);
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
            drop(conn);

            if json {
                out.json_modules(&module, project);
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
            drop(conn);

            if json {
                out.json_modules(&module, project);
            } else {
                print!("{}", render::module_updated(&module));
            }
        }

        ModuleAction::Delete { project, name } => {
            let conn = pool.write()?;
            let project_id = queries::resolve_project_identifier(&conn, project)?;
            let module_id = queries::resolve_module_name(&conn, project_id, name)?;
            queries::delete_module(&conn, module_id)?;
            drop(conn);

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
            drop(conn);

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
            drop(conn);

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
            drop(conn);

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
            drop(conn);

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
            drop(conn);

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
            drop(conn);

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
        run(&pool, &cmd, false, None).unwrap();

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
        run(&pool, &cmd, true, None).unwrap();
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
        run(&pool, &cmd, false, None).unwrap();

        // Get it
        let cmd = Command::Issue {
            action: IssueAction::Get {
                identifier: "TST-1".into(),
            },
        };
        run(&pool, &cmd, false, None).unwrap();
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
        run(&pool, &cmd, false, None).unwrap();

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
        run(&pool, &cmd, false, None).unwrap();
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
        run(&pool, &cmd, false, None).unwrap();
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
        run(&pool, &cmd, false, None).unwrap();

        let cmd = Command::Page {
            action: PageAction::Get {
                identifier: "TST-DOC-1".into(),
            },
        };
        run(&pool, &cmd, false, None).unwrap();
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
            None,
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
            None,
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
        run(&pool, &cmd, false, None).unwrap();

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
        run(&pool, &cmd, false, None).unwrap();

        let cmd = Command::Comment {
            action: CommentAction::List {
                identifier: "TST-1".into(),
                limit: queries::DEFAULT_PAGE_LIMIT,
                offset: 0,
                order: "desc".into(),
            },
        };
        run(&pool, &cmd, false, None).unwrap();
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
                rusqlite::params![
                    format!("2026-01-01 00:{:02}:{:02}", i / 60, i % 60),
                    comment.id
                ],
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
        run(&pool, &cmd, false, None).unwrap();

        // List
        let cmd = Command::Module {
            action: ModuleAction::List {
                project: "TST".into(),
            },
        };
        run(&pool, &cmd, false, None).unwrap();

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
        run(&pool, &cmd, false, None).unwrap();

        // Delete
        let cmd = Command::Module {
            action: ModuleAction::Delete {
                project: "TST".into(),
                name: "Core DB".into(),
            },
        };
        run(&pool, &cmd, false, None).unwrap();
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
        run(&pool, &cmd, false, None).unwrap();

        // List
        let cmd = Command::Label {
            action: LabelAction::List {
                project: "TST".into(),
            },
        };
        run(&pool, &cmd, false, None).unwrap();

        // Update
        let cmd = Command::Label {
            action: LabelAction::Update {
                project: "TST".into(),
                name: "bug".into(),
                new_name: Some("defect".into()),
                color: None,
            },
        };
        run(&pool, &cmd, false, None).unwrap();

        // Delete
        let cmd = Command::Label {
            action: LabelAction::Delete {
                project: "TST".into(),
                name: "defect".into(),
            },
        };
        run(&pool, &cmd, false, None).unwrap();
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
        run(&pool, &cmd, false, None).unwrap();

        // List
        let cmd = Command::Folder {
            action: FolderAction::List {
                project: "TST".into(),
            },
        };
        run(&pool, &cmd, false, None).unwrap();

        // Update
        let cmd = Command::Folder {
            action: FolderAction::Update {
                project: "TST".into(),
                name: "Docs".into(),
                new_name: "Documentation".into(),
            },
        };
        run(&pool, &cmd, false, None).unwrap();

        // Delete
        let cmd = Command::Folder {
            action: FolderAction::Delete {
                project: "TST".into(),
                name: "Documentation".into(),
            },
        };
        run(&pool, &cmd, false, None).unwrap();
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
        run(&pool, &cmd, false, None).unwrap();

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
        run(&pool, &cmd, true, None).unwrap();
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
        run(&pool, &cmd, false, None).unwrap();
    }

    #[test]
    fn exec_not_found_errors() {
        let pool = test_pool();

        let cmd = Command::Issue {
            action: IssueAction::Get {
                identifier: "NOPE-1".into(),
            },
        };
        assert!(run(&pool, &cmd, false, None).is_err());
    }

    // ── LIF-409 ──────────────────────────────────────────────

    /// An attachment uploaded by `uploader` (or by nobody, when `None`).
    fn seed_attachment(pool: &DbPool, label: &str, uploader: Option<i64>) -> i64 {
        let conn = pool.write().unwrap();
        queries::attachments::create_attachment(
            &conn,
            &crate::storage::AttachmentStore::hash_bytes(label.as_bytes()),
            &format!("{label}.txt"),
            "text/plain",
            label.len() as i64,
            uploader,
        )
        .unwrap()
        .id
    }

    fn linked_attachment_ids(pool: &DbPool, entity: AttachmentEntity, entity_id: i64) -> Vec<i64> {
        let conn = pool.read().unwrap();
        queries::attachments::list_for_entity(&conn, entity, entity_id)
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect()
    }

    fn body(attachment_id: i64) -> String {
        format!("see ![shot](/api/attachments/{attachment_id})")
    }

    /// The bug LIF-409 names: the direct-SQL backend wrote issue bodies
    /// without ever reconciling the attachments they referenced, so a file
    /// embedded via the CLI stayed unlinked and the orphan sweep eventually
    /// deleted it out from under a live document.
    #[test]
    fn cli_issue_create_and_update_reconcile_attachment_links() {
        let pool = test_pool();
        seed_project(&pool, "TST");
        let first = seed_attachment(&pool, "first", None);
        let second = seed_attachment(&pool, "second", None);

        run(
            &pool,
            &Command::Issue {
                action: IssueAction::Create {
                    project: "TST".into(),
                    title: "With an attachment".into(),
                    description: body(first),
                    status: "todo".into(),
                    priority: "none".into(),
                    module: None,
                    labels: None,
                },
            },
            false,
            None,
        )
        .unwrap();

        let issue_id = {
            let conn = pool.read().unwrap();
            queries::resolve_identifier(&conn, "TST-1").unwrap()
        };
        assert_eq!(
            linked_attachment_ids(&pool, AttachmentEntity::Issue, issue_id),
            vec![first],
            "a CLI-created issue must link what its description references"
        );

        // Editing to a different attachment drops the old link and adds the
        // new one: re-scan on save, same as REST and MCP.
        run(
            &pool,
            &Command::Issue {
                action: IssueAction::Update {
                    identifier: "TST-1".into(),
                    title: None,
                    description: Some(body(second)),
                    status: None,
                    priority: None,
                    module: None,
                    labels: None,
                },
            },
            false,
            None,
        )
        .unwrap();
        assert_eq!(
            linked_attachment_ids(&pool, AttachmentEntity::Issue, issue_id),
            vec![second]
        );

        // An update that does not touch the description leaves the link alone
        // rather than reconciling against an empty body.
        run(
            &pool,
            &Command::Issue {
                action: IssueAction::Update {
                    identifier: "TST-1".into(),
                    title: Some("Renamed".into()),
                    description: None,
                    status: None,
                    priority: None,
                    module: None,
                    labels: None,
                },
            },
            false,
            None,
        )
        .unwrap();
        assert_eq!(
            linked_attachment_ids(&pool, AttachmentEntity::Issue, issue_id),
            vec![second]
        );
    }

    #[test]
    fn cli_page_create_and_update_reconcile_attachment_links() {
        let pool = test_pool();
        seed_project(&pool, "TST");
        let diagram = seed_attachment(&pool, "diagram", None);

        run(
            &pool,
            &Command::Page {
                action: PageAction::Create {
                    title: "Design".into(),
                    project: Some("TST".into()),
                    folder: None,
                    content: body(diagram),
                    labels: None,
                },
            },
            false,
            None,
        )
        .unwrap();

        let page_id = {
            let conn = pool.read().unwrap();
            queries::resolve_page_identifier(&conn, "TST-DOC-1").unwrap()
        };
        assert_eq!(
            linked_attachment_ids(&pool, AttachmentEntity::Page, page_id),
            vec![diagram]
        );

        run(
            &pool,
            &Command::Page {
                action: PageAction::Update {
                    identifier: "TST-DOC-1".into(),
                    title: None,
                    content: Some("the diagram is gone".into()),
                    folder: None,
                    labels: None,
                },
            },
            false,
            None,
        )
        .unwrap();
        assert!(
            linked_attachment_ids(&pool, AttachmentEntity::Page, page_id).is_empty(),
            "dropping the reference must drop the link"
        );
    }

    /// `comment add` used to call bare `create_comment`, so a CLI comment
    /// resolved no `@mentions` and linked no attachments while the identical
    /// comment posted through REST did both. It now goes through the shared
    /// path, and this asserts the parity directly.
    #[test]
    fn cli_comment_add_matches_the_shared_mention_and_attachment_path() {
        let pool = test_pool();
        seed_project(&pool, "TST");
        seed_issue(&pool, "TST", "Test issue");
        seed_user(&pool);
        let shot = seed_attachment(&pool, "shot", None);

        run(
            &pool,
            &Command::Comment {
                action: CommentAction::Add {
                    identifier: "TST-1".into(),
                    content: format!("@testuser look at {}", body(shot)),
                    user: Some("testuser".into()),
                },
            },
            false,
            None,
        )
        .unwrap();

        let conn = pool.read().unwrap();
        let issue_id = queries::resolve_identifier(&conn, "TST-1").unwrap();
        let comments = queries::comments::list_comments(
            &conn,
            queries::comments::CommentParent::Issue(issue_id),
            None,
            None,
        )
        .unwrap();
        assert_eq!(comments.len(), 1);
        let comment = &comments[0];
        let author_id = queries::users::get_user_by_username(&conn, "testuser")
            .unwrap()
            .id;
        assert_eq!(comment.user_id, author_id, "--user names the author");
        assert_eq!(
            queries::comments::list_mention_user_ids(&conn, comment.id).unwrap(),
            vec![author_id],
            "a CLI comment resolves its mentions like every other backend"
        );
        drop(conn);
        assert_eq!(
            linked_attachment_ids(&pool, AttachmentEntity::Comment, comment.id),
            vec![shot]
        );
    }

    /// The trusted-local operator is past every gate the ownership policy
    /// could apply, because they can write the link row by hand. An
    /// authenticated caller in the same position is not, and the same
    /// attachment stays unlinked for them.
    #[test]
    fn a_foreign_attachment_links_for_the_local_operator_and_not_for_a_stranger() {
        let pool = test_pool();
        seed_project(&pool, "TST");
        seed_user(&pool);
        let owner_id = {
            let conn = pool.read().unwrap();
            queries::users::list_users(&conn).unwrap()[0].id
        };
        let foreign = seed_attachment(&pool, "foreign", Some(owner_id));

        // A stranger's authenticated write: refused, silently, as designed.
        let stranger = AttachmentActor::Authenticated(CommentActor {
            user_id: owner_id + 1,
            is_admin: false,
        });
        let stranger_issue = {
            let conn = pool.write().unwrap();
            let project_id = queries::resolve_project_identifier(&conn, "TST").unwrap();
            queries::create_issue(
                &conn,
                &CreateIssue {
                    project_id,
                    title: "stranger".into(),
                    description: body(foreign),
                    attachments: stranger,
                    ..Default::default()
                },
            )
            .unwrap()
            .id
        };
        assert!(
            linked_attachment_ids(&pool, AttachmentEntity::Issue, stranger_issue).is_empty(),
            "another user's unlinked upload must not follow a reference into a stranger's issue"
        );

        // The same body through the CLI: linked.
        run(
            &pool,
            &Command::Issue {
                action: IssueAction::Create {
                    project: "TST".into(),
                    title: "operator".into(),
                    description: body(foreign),
                    status: "todo".into(),
                    priority: "none".into(),
                    module: None,
                    labels: None,
                },
            },
            false,
            None,
        )
        .unwrap();
        let operator_issue = {
            let conn = pool.read().unwrap();
            queries::resolve_identifier(&conn, "TST-2").unwrap()
        };
        assert_eq!(
            linked_attachment_ids(&pool, AttachmentEntity::Issue, operator_issue),
            vec![foreign]
        );
    }

    /// LIF-102's fix, finally applied to this backend: the creator leads the
    /// project it just made, with the matching membership row, exactly as
    /// `POST /api/projects` does. Otherwise the CLI leaves behind a project
    /// `require_project_lead` rejects everyone but administrators for.
    #[test]
    fn cli_project_create_leads_with_the_effective_operator() {
        let pool = test_pool();
        seed_user(&pool);

        run(
            &pool,
            &Command::Project {
                action: ProjectAction::Create {
                    name: "Test".into(),
                    identifier: "TST".into(),
                    description: String::new(),
                },
            },
            false,
            None,
        )
        .unwrap();

        let conn = pool.read().unwrap();
        let admin_id = queries::users::get_user_by_username(&conn, "testuser")
            .unwrap()
            .id;
        let project = queries::get_project(
            &conn,
            queries::resolve_project_identifier(&conn, "TST").unwrap(),
        )
        .unwrap();
        assert_eq!(project.lead_user_id, Some(admin_id));
        assert_eq!(
            queries::members::get_member_role(&conn, project.id, admin_id).unwrap(),
            Some(Role::Lead),
            "the lead pointer and the membership row are one fact, written together"
        );
    }

    fn seed_named_user(pool: &DbPool, username: &str, is_admin: bool, is_bot: bool) -> i64 {
        let conn = pool.write().unwrap();
        queries::users::create_user(
            &conn,
            &CreateUser {
                username: username.into(),
                email: format!("{username}@test.com"),
                password: "testpass123".into(),
                display_name: Some(username.into()),
                is_admin,
                is_bot,
            },
        )
        .unwrap()
        .id
    }

    fn operator_of(pool: &DbPool) -> Result<Option<CommentActor>, LificError> {
        let conn = pool.read().unwrap();
        effective_operator(&conn)
    }

    /// A bot is somebody's tool, not a person: it must never be handed a
    /// project to lead or credited with a comment just because it sorts first.
    /// A deactivated account is one an administrator took out of circulation,
    /// and resurrecting it by row order is the same mistake.
    #[test]
    fn the_local_operator_is_never_a_bot_or_a_deactivated_account() {
        let pool = test_pool();
        seed_named_user(&pool, "botuser", true, true);
        assert_eq!(
            operator_of(&pool).unwrap(),
            None,
            "an admin bot is still a tool, not somebody to act as"
        );

        // `retired` sorts before `keeper`, so only the is_active filter can
        // keep the deactivated account from winning.
        let retired = seed_named_user(&pool, "retired", true, false);
        let keeper = seed_named_user(&pool, "keeper", true, false);
        {
            let conn = pool.write().unwrap();
            queries::users::set_active(&conn, retired, false).unwrap();
        }
        assert_eq!(
            operator_of(&pool).unwrap().map(|a| a.user_id),
            Some(keeper),
            "a deactivated admin is out of circulation and stays out"
        );
    }

    /// With no administrator at all, one active human is unambiguous. Several
    /// are not, and the guess is refused out loud rather than settled by row
    /// order — picking one would silently attribute work to whoever happened
    /// to sign up first.
    #[test]
    fn a_sole_human_acts_but_an_ambiguous_pair_refuses() {
        let pool = test_pool();
        let only = seed_named_user(&pool, "solo", false, false);
        assert_eq!(
            operator_of(&pool).unwrap().map(|a| a.user_id),
            Some(only),
            "one active human is unambiguous"
        );

        seed_named_user(&pool, "second", false, false);
        let error = operator_of(&pool).unwrap_err();
        assert!(
            error.to_string().contains("cannot infer which user"),
            "got: {error}"
        );

        // An admin resolves it: the established operator wins outright.
        let admin = seed_named_user(&pool, "boss", true, false);
        assert_eq!(operator_of(&pool).unwrap().map(|a| a.user_id), Some(admin));
    }

    /// A freshly initialized database has no users at all. That is a real
    /// state, not an error, and it must not be papered over by naming an
    /// arbitrary id: the project is simply created leaderless.
    #[test]
    fn cli_project_create_without_users_is_leaderless_rather_than_arbitrary() {
        let pool = test_pool();

        run(
            &pool,
            &Command::Project {
                action: ProjectAction::Create {
                    name: "Test".into(),
                    identifier: "TST".into(),
                    description: String::new(),
                },
            },
            false,
            None,
        )
        .unwrap();

        let conn = pool.read().unwrap();
        let project = queries::get_project(
            &conn,
            queries::resolve_project_identifier(&conn, "TST").unwrap(),
        )
        .unwrap();
        assert_eq!(project.lead_user_id, None);
        assert!(
            queries::members::list_members(&conn, project.id)
                .unwrap()
                .is_empty()
        );
    }

    /// `comment add` on a database with no users refuses rather than picking
    /// somebody: a comment must have an author.
    #[test]
    fn cli_comment_add_without_users_refuses() {
        let pool = test_pool();
        seed_project(&pool, "TST");
        seed_issue(&pool, "TST", "Test issue");

        let error = run(
            &pool,
            &Command::Comment {
                action: CommentAction::Add {
                    identifier: "TST-1".into(),
                    content: "orphan".into(),
                    user: None,
                },
            },
            false,
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("no users exist"), "got: {error}");
    }

    /// The same object, the same configured base URL, the same `web_url` on
    /// either backend. The SQL backend used to emit none at all, which made
    /// `lific issue get --json` answer differently depending on how it reached
    /// the data.
    #[test]
    fn sql_json_web_url_matches_the_http_backend() {
        let pool = test_pool();
        seed_project(&pool, "TST");
        seed_issue(&pool, "TST", "Linkable");

        let context = IssueLinkContext::parse("https://tracker.example/lific").unwrap();
        let issue = {
            let conn = pool.read().unwrap();
            let id = queries::resolve_identifier(&conn, "TST-1").unwrap();
            queries::get_issue(&conn, id).unwrap()
        };
        let value = Output::value(&issue);

        let sql = Output {
            json: true,
            links: Some(&context),
        }
        .enrich_resources(value.clone(), ResourceKind::Issue);
        // Exactly what `HttpBackend::execute` does to an issue response.
        let http = weblinks::linked_resources(
            value.clone(),
            &context,
            IssueLinkOutput::Url,
            ResourceKind::Issue,
        );

        assert_eq!(sql, http);
        assert_eq!(
            sql["web_url"],
            "https://tracker.example/lific/TST/issues/TST-1"
        );

        // No configured public URL means no link, rather than one built on a
        // guessed origin.
        let unlinked = Output {
            json: true,
            links: None,
        }
        .enrich_resources(value, ResourceKind::Issue);
        assert!(unlinked.get("web_url").is_none());
    }
}
