//! HTTP transport for data-oriented CLI commands.
//!
//! The SQL executor and this module intentionally share the `Command` enum:
//! command parsing and output selection stay transport-independent while each
//! backend owns only identifier resolution and I/O.

use std::{
    borrow::Cow,
    collections::HashMap,
    fs,
    io::Write,
    net::IpAddr,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Result, anyhow, bail};
use reqwest::{
    Client, Method, RequestBuilder,
    header::{CONTENT_DISPOSITION, CONTENT_TYPE, HeaderMap},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::db::models;
use crate::links::{IssueLinkContext, MarkdownReference, ResourceUrl};

use super::{
    Command, CommentAction, ExportAction, FolderAction, IssueAction, LabelAction, ModuleAction,
    PageAction, ProjectAction, owned_labels, render,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const ERROR_BODY_LIMIT: usize = 64 * 1024;
type QueryParam<'a> = (&'a str, Cow<'a, str>);

#[derive(Debug)]
struct ResolvedResource {
    id: i64,
    identifier: String,
}

/// Deserialize a response into the model a command renders, or `None` when
/// the server sent something this binary does not recognize.
fn decode<T: DeserializeOwned>(value: &Value) -> Option<T> {
    serde_json::from_value(value.clone()).ok()
}

pub async fn run(
    command: &Command,
    base_url: &str,
    api_key: Option<&str>,
    json_output: bool,
) -> Result<()> {
    let backend = HttpBackend::new(base_url, api_key)?;
    let link_output = if json_output {
        IssueLinkOutput::Url
    } else {
        IssueLinkOutput::Markdown
    };
    let output = backend.execute(command, link_output).await?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        print!("{}", backend.human(command, &output).await);
    }
    Ok(())
}

struct HttpBackend {
    client: Client,
    base_url: String,
    link_context: IssueLinkContext,
    api_key: Option<String>,
}

impl HttpBackend {
    fn new(base_url: &str, api_key: Option<&str>) -> Result<Self> {
        let base_url = base_url.trim().trim_end_matches('/');
        if base_url.is_empty() {
            bail!("HTTP backend requires a non-empty server URL (use --url or LIFIC_URL)");
        }
        let parsed = reqwest::Url::parse(base_url)
            .map_err(|error| anyhow!("invalid HTTP backend URL '{base_url}': {error}"))?;
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            bail!("HTTP backend URL must use http:// or https://");
        }
        let link_context = IssueLinkContext::parse(base_url).ok_or_else(|| {
            anyhow!("HTTP backend URL must not contain credentials, a query, or a fragment")
        })?;
        if parsed.scheme() == "http"
            && api_key.is_some()
            && parsed
                .host_str()
                .is_some_and(|host| !is_loopback_host(host))
        {
            bail!(
                "refusing to send bearer credentials over plaintext http to {}",
                parsed.host_str().unwrap_or_default()
            );
        }
        if parsed.scheme() == "http" && parsed.host_str().is_some_and(|host| !is_loopback_host(host)) {
            eprintln!(
                "warning: connecting over unencrypted http to {}",
                parsed.host_str().unwrap_or_default()
            );
        }
        Ok(Self {
            client: Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            base_url: base_url.to_owned(),
            link_context,
            api_key: api_key.map(str::to_owned),
        })
    }

    async fn execute(&self, command: &Command, output: IssueLinkOutput) -> Result<Value> {
        match command {
            Command::Issue { action } => self.issue(action).await.map(|value| {
                linked_resources(value, &self.link_context, output, ResourceKind::Issue)
            }),
            Command::Project { action } => self.project(action).await.map(|value| {
                linked_resources(value, &self.link_context, output, ResourceKind::Project)
            }),
            Command::Page { action } => self.page(action).await.map(|value| {
                linked_resources(value, &self.link_context, output, ResourceKind::Page)
            }),
            Command::Export { action } => self.export(action).await,
            Command::Search {
                query,
                project,
                limit,
            } => {
                let project_id = match project {
                    Some(project) => Some(self.project_id(project).await?),
                    None => None,
                };
                let params = [
                    Some(("query", Cow::Borrowed(query.as_str()))),
                    project_id.map(|id| ("project_id", Cow::Owned(id.to_string()))),
                    limit.map(|value| ("limit", Cow::Owned(value.to_string()))),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
                self.get_json("/api/search", &params).await.map(|value| {
                    linked_resources(value, &self.link_context, output, ResourceKind::Search)
                })
            }
            Command::Comment { action } => self.comment(action).await.map(|(value, identifier)| {
                linked_comments(value, &self.link_context, output, &identifier)
            }),
            Command::Module { action } => self.module(action).await.map(|(value, project)| {
                linked_modules(value, &self.link_context, output, &project)
            }),
            Command::Label { action } => self.label(action).await,
            Command::Folder { action } => self.folder(action).await,
            _ => bail!("the HTTP backend does not support this command yet"),
        }
    }

    /// Render a response the way the SQL backend renders it (LIF-373).
    ///
    /// Falls back to pretty JSON when the payload does not deserialize into
    /// the model the command expects, so a CLI pointed at a server of another
    /// version still prints something rather than failing at the last step.
    async fn human(&self, command: &Command, value: &Value) -> String {
        match self.render(command, value).await {
            Some(text) => text,
            None => format!("{}\n", pretty(value)),
        }
    }

    async fn render(&self, command: &Command, value: &Value) -> Option<String> {
        Some(match command {
            Command::Issue { action } => match action {
                IssueAction::List { .. } => {
                    let issues: Vec<models::Issue> = decode(value)?;
                    let names = self.module_names(&issues).await;
                    render::issue_list(&issues, &|id| names.get(&id).cloned())
                }
                IssueAction::Get { .. } => {
                    let issue: models::Issue = decode(value)?;
                    let names = self.module_names(std::slice::from_ref(&issue)).await;
                    render::issue_detail(&issue, &|id| names.get(&id).cloned())
                }
                IssueAction::Create { .. } => {
                    let issue: models::Issue = decode(value)?;
                    render::issue_created(&issue)
                }
                IssueAction::Update { .. } => {
                    let issue: models::Issue = decode(value)?;
                    render::issue_updated(&issue)
                }
            },
            Command::Project { action } => match action {
                ProjectAction::List => {
                    let projects: Vec<models::Project> = decode(value)?;
                    render::project_list(&projects)
                }
                ProjectAction::Get { .. } => {
                    let project: models::Project = decode(value)?;
                    render::project_detail(&project)
                }
                ProjectAction::Create { .. } => {
                    let project: models::Project = decode(value)?;
                    render::project_created(&project)
                }
                ProjectAction::Update { .. } => {
                    let project: models::Project = decode(value)?;
                    render::project_updated(&project)
                }
            },
            Command::Page { action } => match action {
                PageAction::List { .. } => {
                    let pages: Vec<models::Page> = decode(value)?;
                    render::page_list(&pages)
                }
                PageAction::Get { .. } => {
                    let page: models::Page = decode(value)?;
                    render::page_detail(&page)
                }
                PageAction::Create { .. } => {
                    let page: models::Page = decode(value)?;
                    render::page_created(&page)
                }
                PageAction::Update { .. } => {
                    let page: models::Page = decode(value)?;
                    render::page_updated(&page)
                }
            },
            Command::Search { .. } => {
                let results: Vec<models::SearchResult> = decode(value)?;
                render::search_results(&results)
            }
            Command::Comment { action } => match action {
                CommentAction::List { identifier } => {
                    let comments: Vec<models::Comment> = decode(value)?;
                    render::comment_list(&comments, identifier)
                }
                CommentAction::Add { identifier, .. } => {
                    let comment: models::Comment = decode(value)?;
                    render::comment_added(&comment, identifier)
                }
            },
            Command::Module { action } => match action {
                ModuleAction::List { project } => {
                    let modules: Vec<models::Module> = decode(value)?;
                    render::module_list(&modules, project)
                }
                ModuleAction::Create { project, .. } => {
                    let module: models::Module = decode(value)?;
                    render::module_created(&module, project)
                }
                ModuleAction::Update { .. } => {
                    let module: models::Module = decode(value)?;
                    render::module_updated(&module)
                }
                ModuleAction::Delete { name, .. } => render::module_deleted(name),
            },
            Command::Label { action } => match action {
                LabelAction::List { project } => {
                    let labels: Vec<models::Label> = decode(value)?;
                    render::label_list(&labels, project)
                }
                LabelAction::Create { .. } => {
                    let label: models::Label = decode(value)?;
                    render::label_created(&label)
                }
                LabelAction::Update { .. } => {
                    let label: models::Label = decode(value)?;
                    render::label_updated(&label)
                }
                LabelAction::Delete { name, .. } => render::label_deleted(name),
            },
            Command::Folder { action } => match action {
                FolderAction::List { project } => {
                    let folders: Vec<models::Folder> = decode(value)?;
                    render::folder_list(&folders, project)
                }
                FolderAction::Create { .. } => {
                    let folder: models::Folder = decode(value)?;
                    render::folder_created(&folder)
                }
                FolderAction::Update { name, .. } => {
                    let folder: models::Folder = decode(value)?;
                    render::folder_updated(name, &folder)
                }
                FolderAction::Delete { name, .. } => render::folder_deleted(name),
            },
            Command::Export { action } => {
                let output = match action {
                    ExportAction::Issue { output, .. }
                    | ExportAction::Page { output, .. }
                    | ExportAction::Project { output, .. } => output,
                };
                let written: Vec<PathBuf> = decode::<Vec<String>>(value)?
                    .into_iter()
                    .map(PathBuf::from)
                    .collect();
                render::export_written(&written, output)
            }
            _ => return None,
        })
    }

    /// Module names for a rendered issue list, keyed by id. The SQL backend
    /// reads these from the database; over HTTP one `/api/modules` call per
    /// project covers the whole list. Failures are swallowed, so an
    /// unresolvable module renders as no module, matching the SQL backend.
    async fn module_names(&self, issues: &[models::Issue]) -> HashMap<i64, String> {
        let mut projects = issues
            .iter()
            .filter(|issue| issue.module_id.is_some())
            .map(|issue| issue.project_id)
            .collect::<Vec<_>>();
        projects.sort_unstable();
        projects.dedup();

        let mut names = HashMap::new();
        for project_id in projects {
            let params = [("project_id", Cow::Owned(project_id.to_string()))];
            if let Ok(value) = self.get_json("/api/modules", &params).await
                && let Some(modules) = decode::<Vec<models::Module>>(&value)
            {
                names.extend(modules.into_iter().map(|module| (module.id, module.name)));
            }
        }
        names
    }

    async fn issue(&self, action: &IssueAction) -> Result<Value> {
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
                let project_id = self.project_id(project).await?;
                let module_id = match module {
                    Some(module) => Some(self.module_id(project_id, module).await?),
                    None => None,
                };
                let params = [
                    Some(("project_id", Cow::Owned(project_id.to_string()))),
                    status
                        .as_deref()
                        .map(|value| ("status", Cow::Borrowed(value))),
                    priority
                        .as_deref()
                        .map(|value| ("priority", Cow::Borrowed(value))),
                    module_id.map(|id| ("module_id", Cow::Owned(id.to_string()))),
                    label
                        .as_deref()
                        .map(|value| ("label", Cow::Borrowed(value))),
                    workable.then_some(("workable", Cow::Borrowed("true"))),
                    limit.map(|value| ("limit", Cow::Owned(value.to_string()))),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
                self.get_json("/api/issues", &params).await
            }
            IssueAction::Get { identifier } => {
                self.get_json(&format!("/api/issues/resolve/{}", segment(identifier)), &[])
                    .await
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
                let project_id = self.project_id(project).await?;
                let module_id = match module {
                    Some(module) => Some(self.module_id(project_id, module).await?),
                    None => None,
                };
                let body = models::CreateIssue {
                    project_id,
                    title: title.clone(),
                    description: description.clone(),
                    status: status.parse().map_err(|e: String| anyhow!(e))?,
                    priority: priority.parse().map_err(|e: String| anyhow!(e))?,
                    module_id,
                    start_date: None,
                    target_date: None,
                    labels: owned_labels(labels.as_deref()).unwrap_or_default(),
                    source: None,
                };
                self.send_json(Method::POST, "/api/issues", &body).await
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
                let id = self.issue_id(identifier).await?;
                let module_id = match module {
                    Some(module) => {
                        let project_id = self.issue_project_id(id).await?;
                        Some(self.module_id(project_id, module).await?)
                    }
                    None => None,
                };
                let body = models::UpdateIssue {
                    title: title.clone(),
                    description: description.clone(),
                    status: models::Status::parse_opt(status.as_deref()).map_err(|e| anyhow!(e))?,
                    priority: models::Priority::parse_opt(priority.as_deref())
                        .map_err(|e| anyhow!(e))?,
                    // LIF-145: module_id is tristate; the CLI only sets or
                    // skips (no clear), so map Some(id) -> Some(Some(id)).
                    module_id: module_id.map(Some),
                    sort_order: None,
                    start_date: None,
                    target_date: None,
                    labels: owned_labels(labels.as_deref()),
                };
                self.send_json(Method::PUT, &format!("/api/issues/{id}"), &body)
                    .await
            }
        }
    }

    async fn project(&self, action: &ProjectAction) -> Result<Value> {
        match action {
            ProjectAction::List => self.get_json("/api/projects", &[]).await,
            ProjectAction::Get { identifier } => {
                let id = self.project_id(identifier).await?;
                self.get_json(&format!("/api/projects/{id}"), &[]).await
            }
            ProjectAction::Create {
                name,
                identifier,
                description,
            } => {
                self.send_json(
                    Method::POST,
                    "/api/projects",
                    &models::CreateProject {
                        name: name.clone(),
                        identifier: identifier.clone(),
                        description: description.clone(),
                        emoji: None,
                        lead_user_id: None,
                    },
                )
                .await
            }
            ProjectAction::Update {
                identifier,
                name,
                description,
            } => {
                let id = self.project_id(identifier).await?;
                let body = models::UpdateProject {
                    name: name.clone(),
                    identifier: None,
                    description: description.clone(),
                    emoji: None,
                    lead_user_id: None,
                };
                self.send_json(Method::PUT, &format!("/api/projects/{id}"), &body)
                    .await
            }
        }
    }

    async fn page(&self, action: &PageAction) -> Result<Value> {
        match action {
            PageAction::List {
                project,
                folder,
                label,
            } => {
                let (project_id, folder_id) = self
                    .page_scope(project.as_deref(), folder.as_deref())
                    .await?;
                let params = [
                    project_id.map(|id| ("project_id", Cow::Owned(id.to_string()))),
                    folder_id.map(|id| ("folder_id", Cow::Owned(id.to_string()))),
                    label
                        .as_deref()
                        .map(|value| ("label", Cow::Borrowed(value))),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
                self.get_json("/api/pages", &params).await
            }
            PageAction::Get { identifier } => {
                let id = self.page_id(identifier).await?;
                self.get_json(&format!("/api/pages/{id}"), &[]).await
            }
            PageAction::Create {
                title,
                project,
                folder,
                content,
                labels,
            } => {
                let (project_id, folder_id) = self
                    .page_scope(project.as_deref(), folder.as_deref())
                    .await?;
                self.send_json(
                    Method::POST,
                    "/api/pages",
                    &models::CreatePage {
                        project_id,
                        folder_id,
                        title: title.clone(),
                        content: content.clone(),
                        status: "draft".into(),
                        labels: owned_labels(labels.as_deref()).unwrap_or_default(),
                    },
                )
                .await
            }
            PageAction::Update {
                identifier,
                title,
                content,
                folder,
                labels,
            } => {
                let id = self.page_id(identifier).await?;
                let folder_id = match folder {
                    Some(folder) => {
                        let page = self.get_json(&format!("/api/pages/{id}"), &[]).await?;
                        let project_id = page["project_id"]
                            .as_i64()
                            .ok_or_else(|| anyhow!("cannot set folder on workspace page"))?;
                        Some(self.folder_id(project_id, folder).await?)
                    }
                    None => None,
                };
                let body = models::UpdatePage {
                    title: title.clone(),
                    content: content.clone(),
                    folder_id: folder_id.map(Some),
                    sort_order: None,
                    status: None,
                    pinned: None,
                    labels: owned_labels(labels.as_deref()),
                };
                self.send_json(Method::PUT, &format!("/api/pages/{id}"), &body)
                    .await
            }
        }
    }

    async fn comment(&self, action: &CommentAction) -> Result<(Value, String)> {
        match action {
            CommentAction::List { identifier } => {
                let issue = self.issue_identity(identifier).await?;
                self.get_json(&format!("/api/issues/{}/comments", issue.id), &[])
                    .await
                    .map(|value| (value, issue.identifier))
            }
            CommentAction::Add {
                identifier,
                content,
                user,
            } => {
                user.as_ref().map_or(Ok(()), |_| {
                    Err(anyhow!(
                        "--user cannot be used with the HTTP backend; the server uses the API credential's user"
                    ))
                })?;
                let issue = self.issue_identity(identifier).await?;
                self.send_json(
                    Method::POST,
                    &format!("/api/issues/{}/comments", issue.id),
                    &models::CreateComment {
                        content: content.clone(),
                    },
                )
                .await
                .map(|value| (value, issue.identifier))
            }
        }
    }

    async fn module(&self, action: &ModuleAction) -> Result<(Value, String)> {
        match action {
            ModuleAction::List { project } => {
                let project = self.project_identity(project).await?;
                self.get_json(
                    "/api/modules",
                    &[("project_id", Cow::Owned(project.id.to_string()))],
                )
                .await
                .map(|value| (value, project.identifier))
            }
            ModuleAction::Create {
                project,
                name,
                description,
                status,
            } => {
                let project = self.project_identity(project).await?;
                self.send_json(
                    Method::POST,
                    "/api/modules",
                    &models::CreateModule {
                        project_id: project.id,
                        name: name.clone(),
                        description: description.clone(),
                        status: status.clone(),
                        emoji: None,
                    },
                )
                .await
                .map(|value| (value, project.identifier))
            }
            ModuleAction::Update {
                project,
                name,
                new_name,
                description,
                status,
            } => {
                let project = self.project_identity(project).await?;
                let id = self.module_id(project.id, name).await?;
                let body = models::UpdateModule {
                    name: new_name.clone(),
                    description: description.clone(),
                    status: status.clone(),
                    emoji: None,
                };
                self.send_json(Method::PUT, &format!("/api/modules/{id}"), &body)
                    .await
                    .map(|value| (value, project.identifier))
            }
            ModuleAction::Delete { project, name } => {
                let project = self.project_identity(project).await?;
                let id = self.module_id(project.id, name).await?;
                self.send_delete(&format!("/api/modules/{id}"), name)
                    .await
                    .map(|value| (value, project.identifier))
            }
        }
    }

    async fn label(&self, action: &LabelAction) -> Result<Value> {
        match action {
            LabelAction::List { project } => {
                let id = self.project_id(project).await?;
                self.get_json("/api/labels", &[("project_id", Cow::Owned(id.to_string()))])
                    .await
            }
            LabelAction::Create {
                project,
                name,
                color,
            } => {
                let project_id = self.project_id(project).await?;
                self.send_json(
                    Method::POST,
                    "/api/labels",
                    &models::CreateLabel {
                        project_id,
                        name: name.clone(),
                        color: color.clone(),
                    },
                )
                .await
            }
            LabelAction::Update {
                project,
                name,
                new_name,
                color,
            } => {
                let id = self.label_id(self.project_id(project).await?, name).await?;
                let body = models::UpdateLabel {
                    name: new_name.clone(),
                    color: color.clone(),
                };
                self.send_json(Method::PUT, &format!("/api/labels/{id}"), &body)
                    .await
            }
            LabelAction::Delete { project, name } => {
                let id = self.label_id(self.project_id(project).await?, name).await?;
                self.send_delete(&format!("/api/labels/{id}"), name).await
            }
        }
    }

    async fn folder(&self, action: &FolderAction) -> Result<Value> {
        match action {
            FolderAction::List { project } => {
                let id = self.project_id(project).await?;
                self.get_json(
                    "/api/folders",
                    &[("project_id", Cow::Owned(id.to_string()))],
                )
                .await
            }
            FolderAction::Create { project, name } => {
                let project_id = self.project_id(project).await?;
                self.send_json(
                    Method::POST,
                    "/api/folders",
                    &models::CreateFolder {
                        project_id,
                        parent_id: None,
                        name: name.clone(),
                    },
                )
                .await
            }
            FolderAction::Update {
                project,
                name,
                new_name,
            } => {
                let id = self
                    .folder_id(self.project_id(project).await?, name)
                    .await?;
                self.send_json(
                    Method::PUT,
                    &format!("/api/folders/{id}"),
                    &models::UpdateFolder {
                        name: Some(new_name.clone()),
                    },
                )
                .await
            }
            FolderAction::Delete { project, name } => {
                let id = self
                    .folder_id(self.project_id(project).await?, name)
                    .await?;
                self.send_delete(&format!("/api/folders/{id}"), name).await
            }
        }
    }

    /// Write a remote export to disk in the layout a direct-SQL export
    /// produces, so `--backend http` and the default backend leave the same
    /// tree behind (LIF-341).
    ///
    /// A single issue or page is fetched as its `ExportBundle` (`format=json`)
    /// and handed to `write_bundle_to_directory`, the very writer the SQL
    /// backend calls, so the file lands at its nested `PROJ/issues/....md`
    /// path instead of as a bare basename in the output directory. A project
    /// export stays one ZIP on the wire and is unpacked here; the archive's
    /// entry names are the bundle paths, so the result is the same either way.
    ///
    /// The returned array of written paths is what both the JSON output and
    /// the shared renderer key off, matching the SQL backend's own.
    async fn export(&self, action: &ExportAction) -> Result<Value> {
        let (path, output, kind) = match action {
            ExportAction::Issue { identifier, output } => (
                format!("/api/export/issues/{}", segment(identifier)),
                output,
                ExportShape::Bundle,
            ),
            ExportAction::Page { identifier, output } => (
                format!("/api/export/pages/{}", segment(identifier)),
                output,
                ExportShape::Bundle,
            ),
            ExportAction::Project { project, output } => (
                format!("/api/export/projects/{}", segment(project)),
                output,
                ExportShape::Archive,
            ),
        };
        let params: &[QueryParam<'_>] = match kind {
            ExportShape::Bundle => &[("format", Cow::Borrowed("json"))],
            ExportShape::Archive => &[],
        };

        let response = self
            .send(self.request_builder(Method::GET, &path).query(params))
            .await?;
        let filename = export_filename(response.headers());
        let json_body = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("application/json"));
        let body = response.bytes().await?;

        let written = match kind {
            ExportShape::Archive => crate::export::unpack_zip_to_directory(&body, output)?,
            // A server too old to answer `format=json` ignores the parameter
            // and sends the markdown file itself. Save it under the name the
            // server chose rather than failing outright, the same courtesy
            // `human` extends to unrecognized payloads.
            ExportShape::Bundle if !json_body => {
                let filename = filename.unwrap_or_else(|| "export.bin".into());
                fs::create_dir_all(output)?;
                let path = output.join(&filename);
                fs::File::create(&path)?.write_all(&body)?;
                vec![path]
            }
            ExportShape::Bundle => {
                let bundle: crate::export::ExportBundle = serde_json::from_slice(&body)?;
                crate::export::write_bundle_to_directory(&bundle, output)?
            }
        };

        Ok(json!(
            written
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
        ))
    }

    async fn project_id(&self, identifier: &str) -> Result<i64> {
        self.project_identity(identifier)
            .await
            .map(|project| project.id)
    }

    async fn project_identity(&self, identifier: &str) -> Result<ResolvedResource> {
        self.resolve_resource("/api/projects", "identifier", identifier, "project", &[])
            .await
    }

    async fn page_scope(
        &self,
        project: Option<&str>,
        folder: Option<&str>,
    ) -> Result<(Option<i64>, Option<i64>)> {
        let project_id = match project {
            Some(project) => Some(self.project_id(project).await?),
            None => None,
        };
        let folder_id = match project_id.zip(folder) {
            Some((project_id, folder)) => Some(self.folder_id(project_id, folder).await?),
            None => None,
        };
        Ok((project_id, folder_id))
    }

    async fn issue_id(&self, identifier: &str) -> Result<i64> {
        self.issue_identity(identifier).await.map(|issue| issue.id)
    }

    async fn issue_identity(&self, identifier: &str) -> Result<ResolvedResource> {
        let value = self
            .get_json(&format!("/api/issues/resolve/{}", segment(identifier)), &[])
            .await?;
        resource_from_object(value, "identifier", identifier, "issue")
    }

    async fn issue_project_id(&self, id: i64) -> Result<i64> {
        self.get_json(&format!("/api/issues/{id}"), &[]).await?["project_id"]
            .as_i64()
            .ok_or_else(|| anyhow!("issue {id} response had no project id"))
    }

    async fn page_id(&self, identifier: &str) -> Result<i64> {
        self.get_json(&format!("/api/pages/resolve/{}", segment(identifier)), &[])
            .await?["id"]
            .as_i64()
            .ok_or_else(|| anyhow!("page '{identifier}' response had no id"))
    }

    async fn module_id(&self, project_id: i64, name: &str) -> Result<i64> {
        self.project_resource_id("/api/modules", project_id, name, "module")
            .await
    }

    async fn label_id(&self, project_id: i64, name: &str) -> Result<i64> {
        self.project_resource_id("/api/labels", project_id, name, "label")
            .await
    }

    async fn folder_id(&self, project_id: i64, name: &str) -> Result<i64> {
        self.project_resource_id("/api/folders", project_id, name, "folder")
            .await
    }

    async fn project_resource_id(
        &self,
        path: &str,
        project_id: i64,
        name: &str,
        kind: &str,
    ) -> Result<i64> {
        let params = [("project_id", Cow::Owned(project_id.to_string()))];
        self.resolve_id(path, "name", name, kind, &params).await
    }

    async fn resolve_id(
        &self,
        path: &str,
        key: &str,
        expected: &str,
        kind: &str,
        params: &[QueryParam<'_>],
    ) -> Result<i64> {
        self.resolve_resource(path, key, expected, kind, params)
            .await
            .map(|resource| resource.id)
    }

    async fn resolve_resource(
        &self,
        path: &str,
        key: &str,
        expected: &str,
        kind: &str,
        params: &[QueryParam<'_>],
    ) -> Result<ResolvedResource> {
        find_resource(self.get_json(path, params).await?, key, expected, kind)
    }

    async fn get_json(&self, path: &str, params: &[QueryParam<'_>]) -> Result<Value> {
        let response = self
            .send(self.request_builder(Method::GET, path).query(params))
            .await?;
        Ok(response.json().await?)
    }

    async fn send_json<T: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        body: &T,
    ) -> Result<Value> {
        Ok(self
            .send(self.request_builder(method, path).json(body))
            .await?
            .json()
            .await?)
    }

    /// Delete a resource and answer with the shape the SQL backend prints
    /// (LIF-341). The server replies `204 No Content`, so the name the user
    /// asked us to delete is the only thing left to report, and both backends
    /// now report it the same way.
    async fn send_delete(&self, path: &str, name: &str) -> Result<Value> {
        self.send(self.request_builder(Method::DELETE, path))
            .await?;
        Ok(serde_json::to_value(render::Deleted::named(name))?)
    }

    async fn send(&self, request: RequestBuilder) -> Result<reqwest::Response> {
        let response = request.send().await?;
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        let message = read_error_body(response).await.unwrap_or_default();
        bail!(
            "HTTP backend request failed ({status}): {}",
            sanitize_error_detail(&error_detail(&message))
        );
    }

    fn request_builder(&self, method: Method, path: &str) -> RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        let request = self.client.request(method, url);
        match self.api_key.as_deref() {
            Some(key) => request.bearer_auth(key),
            None => request,
        }
    }
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn find_resource(value: Value, key: &str, expected: &str, kind: &str) -> Result<ResolvedResource> {
    match value {
        Value::Array(items) => items.into_iter().find_map(|item| match item {
            Value::Object(object)
                if object
                    .get(key)
                    .and_then(Value::as_str)
                    .is_some_and(|value| value.eq_ignore_ascii_case(expected)) =>
            {
                object.get("id").and_then(Value::as_i64).map(|_| object)
            }
            _ => None,
        }),
        _ => None,
    }
    .ok_or_else(|| anyhow!("{kind} '{expected}' not found"))
    .and_then(|object| resource_from_map(object, key, expected, kind))
}

fn resource_from_object(
    value: Value,
    key: &str,
    expected: &str,
    kind: &str,
) -> Result<ResolvedResource> {
    match value {
        Value::Object(object) => resource_from_map(object, key, expected, kind),
        _ => Err(anyhow!("{kind} '{expected}' response was not an object")),
    }
}

fn resource_from_map(
    mut object: serde_json::Map<String, Value>,
    key: &str,
    expected: &str,
    kind: &str,
) -> Result<ResolvedResource> {
    let id = object
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("{kind} '{expected}' response had no id"))?;
    match object.remove(key) {
        Some(Value::String(identifier)) => Ok(ResolvedResource { id, identifier }),
        _ => Err(anyhow!("{kind} '{expected}' response had no {key}")),
    }
}

fn segment(value: &str) -> String {
    urlencoding::encode(value).into_owned()
}

#[must_use]
fn safe_filename(value: &str) -> Option<String> {
    Path::new(value)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| *name != "." && *name != "..")
        .map(str::to_owned)
}

fn export_filename(headers: &HeaderMap) -> Option<String> {
    headers
        .get(CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .map(content_disposition::parse_content_disposition)
        .and_then(|header| header.params.get("filename").cloned())
        .and_then(|filename| safe_filename(&filename))
}

fn error_detail(message: &str) -> String {
    serde_json::from_str::<Value>(message)
        .ok()
        .and_then(|value| value["error"].as_str().map(str::to_owned))
        .unwrap_or_else(|| message.to_owned())
}

async fn read_error_body(mut response: reqwest::Response) -> Result<String, reqwest::Error> {
    let mut body = Vec::with_capacity(ERROR_BODY_LIMIT);
    while body.len() < ERROR_BODY_LIMIT {
        let Some(chunk) = response.chunk().await? else {
            break;
        };
        let remaining = ERROR_BODY_LIMIT - body.len();
        body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

fn sanitize_error_detail(detail: &str) -> String {
    detail
        .chars()
        .map(|character| {
            if character.is_ascii_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

/// How the server delivers an export, and so how this backend turns it back
/// into the on-disk layout both backends share (LIF-341).
#[derive(Clone, Copy)]
enum ExportShape {
    /// One `ExportBundle` as JSON, written through the shared writer.
    Bundle,
    /// One ZIP whose entry names are the bundle's paths, unpacked locally.
    Archive,
}

#[derive(Clone, Copy)]
enum IssueLinkOutput {
    Url,
    Markdown,
}

#[derive(Clone, Copy)]
enum ResourceKind {
    Issue,
    Project,
    Page,
    Search,
}

#[derive(Clone, Copy)]
enum CommentLocation {
    Issue,
    Page(i64),
}

impl CommentLocation {
    fn url<'a>(
        self,
        context: &'a IssueLinkContext,
        identifier: &'a str,
        comment_id: i64,
    ) -> Option<ResourceUrl<'a>> {
        match self {
            Self::Issue => context.issue_comment_url(identifier, comment_id),
            Self::Page(page_id) => context.page_comment_url(identifier, page_id, comment_id),
        }
    }

    fn markdown<'a>(
        self,
        context: &'a IssueLinkContext,
        identifier: &'a str,
        comment_id: i64,
    ) -> MarkdownReference<'a> {
        match self {
            Self::Issue => context.issue_comment_markdown(identifier, comment_id),
            Self::Page(page_id) => context.page_comment_markdown(identifier, page_id, comment_id),
        }
    }
}

fn linked_resources(
    value: Value,
    context: &IssueLinkContext,
    output: IssueLinkOutput,
    kind: ResourceKind,
) -> Value {
    map_output_objects(value, |object| {
        linked_resource(object, context, output, kind)
    })
}

fn map_output_objects(
    value: Value,
    mut map: impl FnMut(serde_json::Map<String, Value>) -> serde_json::Map<String, Value>,
) -> Value {
    fn map_object(
        value: Value,
        map: &mut impl FnMut(serde_json::Map<String, Value>) -> serde_json::Map<String, Value>,
    ) -> Value {
        match value {
            Value::Object(object) => Value::Object(map(object)),
            value => value,
        }
    }

    match value {
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|value| map_object(value, &mut map))
                .collect(),
        ),
        value => map_object(value, &mut map),
    }
}

fn linked_resource(
    object: serde_json::Map<String, Value>,
    context: &IssueLinkContext,
    output: IssueLinkOutput,
    kind: ResourceKind,
) -> serde_json::Map<String, Value> {
    let linked_field = object
        .get("identifier")
        .and_then(Value::as_str)
        .and_then(|identifier| {
            resource_url(&object, context, identifier, kind).map(|url| match output {
                IssueLinkOutput::Url => ("web_url", url.to_string()),
                IssueLinkOutput::Markdown => (
                    "identifier",
                    MarkdownReference::linked(identifier, url).to_string(),
                ),
            })
        });
    match linked_field {
        Some((field, value)) => with_string_field(object, field, value),
        None => object,
    }
}

fn resource_url<'a>(
    object: &serde_json::Map<String, Value>,
    context: &'a IssueLinkContext,
    identifier: &'a str,
    kind: ResourceKind,
) -> Option<ResourceUrl<'a>> {
    let id = object.get("id").and_then(Value::as_i64);
    match kind {
        ResourceKind::Issue => context.issue_url(identifier),
        ResourceKind::Project => context.project_url(identifier),
        ResourceKind::Page => context.page_url(identifier, id?),
        ResourceKind::Search => match object.get("result_type").and_then(Value::as_str)? {
            "issue" => context.issue_url(identifier),
            "page" => context.page_url(identifier, id?),
            "comment" => object
                .get("parent_page_id")
                .and_then(Value::as_i64)
                .map_or(CommentLocation::Issue, CommentLocation::Page)
                .url(context, identifier, id?),
            _ => None,
        },
    }
}

fn linked_comments(
    value: Value,
    context: &IssueLinkContext,
    output: IssueLinkOutput,
    identifier: &str,
) -> Value {
    map_output_objects(value, |object| {
        let linked_field = object
            .get("id")
            .and_then(Value::as_i64)
            .and_then(|comment_id| {
                let location = object
                    .get("page_id")
                    .and_then(Value::as_i64)
                    .map_or(CommentLocation::Issue, CommentLocation::Page);
                location
                    .url(context, identifier, comment_id)
                    .map(|url| match output {
                        IssueLinkOutput::Url => ("web_url", url.to_string()),
                        IssueLinkOutput::Markdown => (
                            "comment",
                            location
                                .markdown(context, identifier, comment_id)
                                .to_string(),
                        ),
                    })
            });
        match linked_field {
            Some((field, value)) => with_string_field(object, field, value),
            None => object,
        }
    })
}

fn linked_modules(
    value: Value,
    context: &IssueLinkContext,
    output: IssueLinkOutput,
    project: &str,
) -> Value {
    map_output_objects(value, |object| {
        let linked_field = object
            .get("id")
            .and_then(Value::as_i64)
            .zip(object.get("name").and_then(Value::as_str))
            .and_then(|(module_id, name)| {
                context
                    .module_url(project, module_id)
                    .map(|url| match output {
                        IssueLinkOutput::Url => ("web_url", url.to_string()),
                        IssueLinkOutput::Markdown => (
                            "name",
                            context
                                .module_markdown(project, module_id, name)
                                .to_string(),
                        ),
                    })
            });
        match linked_field {
            Some((field, value)) => with_string_field(object, field, value),
            None => object,
        }
    })
}

fn with_string_field(
    mut object: serde_json::Map<String, Value>,
    field: &str,
    value: String,
) -> serde_json::Map<String, Value> {
    object.insert(field.into(), Value::String(value));
    object
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        sync::Arc,
    };

    use tempfile::TempDir;

    use axum::{
        Extension, Json, Router,
        body::Body,
        extract::{Request, State},
        http::{HeaderValue, StatusCode},
        response::Response,
        routing::{any, get},
    };
    use reqwest::{
        Method,
        header::{CONTENT_DISPOSITION, CONTENT_TYPE, HeaderMap},
    };
    use serde_json::json;
    use tokio::{net::TcpListener, sync::Mutex, task::JoinHandle};

    use crate::{
        api::test_helpers::{
            json_post, parse_json, seed_project, test_peer, with_attachment_layers,
            with_client_ip_test_layers,
        },
        cli::{
            Command, CommentAction, ExportAction, FolderAction, IssueAction, LabelAction,
            ModuleAction, PageAction, ProjectAction, split_csv,
        },
        config::AuthConfig,
        db::models::AuthUser,
        realtime::RealtimeHub,
    };

    use super::{
        ERROR_BODY_LIMIT, HttpBackend, IssueLinkOutput, ResourceKind, decode, error_detail,
        export_filename, find_resource, is_loopback_host, linked_comments, linked_modules,
        linked_resources, models, render, resource_from_object, resource_url, safe_filename,
        sanitize_error_detail, segment,
    };
    use crate::links::IssueLinkContext;

    type CapturedRequest = Arc<Mutex<Option<(String, Option<String>)>>>;

    struct RealApiFixture {
        url: String,
        project_page_identifier: String,
        workspace_page_identifier: String,
        issue_identifier: String,
        /// The very database the router serves. Cloning the pool lets a test
        /// run the SQL backend over the same rows the HTTP backend just read,
        /// which is what makes a real parity assertion possible (LIF-341).
        db: crate::db::DbPool,
        server: JoinHandle<()>,
    }

    async fn spawn_server(router: Router) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (format!("http://{address}"), task)
    }

    async fn spawn_real_api_server() -> RealApiFixture {
        let db = crate::db::open_memory().expect("test db");
        let admin_id = {
            let conn = db.write().unwrap();
            conn.execute(
                "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot)
                 VALUES ('test-admin', 'admin@test.local', 'x', 'Test Admin', 1, 0)",
                [],
            )
            .unwrap();
            conn.last_insert_rowid()
        };
        let app = with_client_ip_test_layers(
            with_attachment_layers(crate::api::router(db.clone(), &[])),
            test_peer(),
        )
        .layer(Extension(RealtimeHub::new()))
        .layer(Extension(AuthConfig {
            allow_signup: true,
            required: true,
            secure_cookies: false,
        }))
        .layer(Extension(Some(AuthUser {
            id: admin_id,
            username: "test-admin".into(),
            display_name: "Test Admin".into(),
            is_admin: true,
        })))
            .layer(Extension(Some(crate::resolve_caller::ResolvedIdentity {
                user: AuthUser {
                    id: admin_id,
                    username: "test-admin".into(),
                    display_name: "Test Admin".into(),
                    is_admin: true,
                },
                transport: crate::actor::Transport::Web,
            })))
            .layer(Extension(Some(crate::resolve_caller::ResolvedIdentity {
                user: AuthUser {
                    id: admin_id,
                    username: "test-admin".into(),
                    display_name: "Test Admin".into(),
                    is_admin: true,
                },
                transport: crate::actor::Transport::Web,
            })));
        let (project_id, _) = seed_project(&app).await;
        let project_page = parse_json(
            json_post(
                &app,
                "/api/pages",
                json!({"project_id": project_id, "title": "Project page"}),
            )
            .await,
        )
        .await;
        let workspace_page =
            parse_json(json_post(&app, "/api/pages", json!({"title": "Workspace page"})).await)
                .await;
        let issue = parse_json(
            json_post(
                &app,
                "/api/issues",
                json!({"project_id": project_id, "title": "Test issue"}),
            )
            .await,
        )
        .await;
        let (url, server) = spawn_server(app).await;

        RealApiFixture {
            url,
            project_page_identifier: project_page["identifier"].as_str().unwrap().to_owned(),
            workspace_page_identifier: workspace_page["identifier"].as_str().unwrap().to_owned(),
            issue_identifier: issue["identifier"].as_str().unwrap().to_owned(),
            db,
            server,
        }
    }

    async fn capture_request(
        State(captured): State<CapturedRequest>,
        request: Request,
    ) -> Json<serde_json::Value> {
        let authorization = request
            .headers()
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        *captured.lock().await = Some((request.uri().to_string(), authorization));
        Json(json!([{"id": 1}]))
    }

    async fn failed_request() -> (StatusCode, Json<serde_json::Value>) {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "request rejected"})),
        )
    }

    async fn oversized_failed_request() -> Response {
        let mut response = Response::new(Body::from("x".repeat(ERROR_BODY_LIMIT + 1)));
        *response.status_mut() = StatusCode::BAD_REQUEST;
        response
    }

    async fn redirect_response() -> Response {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::FOUND;
        response.headers_mut().insert(
            axum::http::header::LOCATION,
            HeaderValue::from_static("/api/projects"),
        );
        response
    }

    async fn export_response() -> Response {
        let mut response = Response::new(Body::from("export contents"));
        response.headers_mut().insert(
            CONTENT_DISPOSITION,
            HeaderValue::from_static("attachment; filename=report.txt"),
        );
        response
    }

    async fn page_scope_request(request: Request) -> Json<serde_json::Value> {
        let value = match request.uri().path() {
            "/api/projects" => json!([{"id": 3, "identifier": "LIF"}]),
            "/api/folders" => json!([{"id": 8, "name": "Docs"}]),
            "/api/pages" => json!([{"id": 9, "title": "Release notes"}]),
            _ => json!([]),
        };
        Json(value)
    }

    async fn issue_response() -> Json<serde_json::Value> {
        Json(json!({
            "id": 42,
            "identifier": "LIF-42",
            "title": "Construct links with HTTP results"
        }))
    }

    #[test]
    fn rejects_non_http_backend_urls() {
        let error = match HttpBackend::new("file://invalid", None) {
            Ok(_) => panic!("file URL should be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("must use http:// or https://"));
    }

    #[test]
    fn rejects_empty_backend_urls() {
        let error = match HttpBackend::new("///", None) {
            Ok(_) => panic!("an empty URL should be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("non-empty server URL"));
    }

    #[test]
    fn rejects_malformed_backend_urls() {
        let error = match HttpBackend::new("not a URL", None) {
            Ok(_) => panic!("a malformed URL should be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("invalid HTTP backend URL"));
    }

    #[test]
    fn trims_trailing_backend_url_slashes() {
        let backend = HttpBackend::new("https://tracker.invalid///", None).unwrap();
        assert_eq!(backend.base_url, "https://tracker.invalid");
    }

    #[test]
    fn trims_backend_url_whitespace_before_trailing_slashes() {
        let backend = HttpBackend::new("  https://tracker.invalid///  ", None).unwrap();
        assert_eq!(backend.base_url, "https://tracker.invalid");
    }

    #[test]
    fn normalized_backend_url_constructs_links() {
        let backend = HttpBackend::new("  https://tracker.invalid/lific/  ", None).unwrap();

        assert_eq!(
            backend.link_context.issue_markdown("LIF-42").to_string(),
            "[LIF-42](https://tracker.invalid/lific/LIF/issues/LIF-42)"
        );
    }

    #[test]
    fn rejects_ambiguous_backend_urls() {
        for base_url in [
            "https://user:password@tracker.invalid",
            "https://tracker.invalid?tenant=one",
            "https://tracker.invalid#fragment",
        ] {
            assert!(HttpBackend::new(base_url, None).is_err(), "{base_url}");
        }
    }

    #[test]
    fn identifies_loopback_hosts_for_plaintext_warning() {
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("LOCALHOST"));
        assert!(is_loopback_host("127.1.2.3"));
        assert!(is_loopback_host("::1"));
        assert!(!is_loopback_host("0.0.0.0"));
        assert!(!is_loopback_host("tracker.example"));
    }

    #[test]
    fn refuses_plaintext_remote_bearer_transport() {
        let error = match HttpBackend::new("http://tracker.example", Some("secret-key")) {
            Ok(_) => panic!("remote plaintext bearer transport must be rejected"),
            Err(error) => error,
        };
        let message = error.to_string();
        assert!(message.contains("plaintext http"));
        assert!(!message.contains("secret-key"));
        assert!(HttpBackend::new("http://127.0.0.1:3456", Some("secret-key")).is_ok());
        assert!(HttpBackend::new("http://tracker.example", None).is_ok());
    }

    #[test]
    fn builds_authenticated_json_request() {
        let backend = HttpBackend::new("https://tracker.invalid", Some("key-value")).unwrap();
        let body = json!({"title": "A test"});
        let request = backend
            .request_builder(Method::POST, "/api/issues")
            .json(&body)
            .build()
            .unwrap();

        assert_eq!(request.url().as_str(), "https://tracker.invalid/api/issues");
        assert_eq!(
            request.headers().get("authorization").unwrap(),
            "Bearer key-value"
        );
        assert_eq!(
            request.headers().get("content-type").unwrap(),
            "application/json"
        );
        let body = request.body().and_then(reqwest::Body::as_bytes).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(body).unwrap()["title"],
            "A test"
        );
    }

    #[test]
    fn builds_request_without_auth_when_key_is_absent() {
        let backend = HttpBackend::new("https://tracker.invalid", None).unwrap();
        let request = backend
            .request_builder(Method::GET, "/api/projects")
            .build()
            .unwrap();

        assert!(request.headers().get("authorization").is_none());
        assert!(request.headers().get("content-type").is_none());
        assert_eq!(request.url().path(), "/api/projects");
    }

    #[test]
    fn encodes_query_parameters_in_request_builder() {
        let backend = HttpBackend::new("https://tracker.invalid", None).unwrap();
        let params = [("search term", "a/b".to_owned())];
        let request = backend
            .request_builder(Method::GET, "/api/search")
            .query(&params)
            .build()
            .unwrap();

        assert_eq!(
            request.url().query_pairs().collect::<Vec<_>>(),
            vec![("search term".into(), "a/b".into())]
        );
    }

    #[test]
    fn preserves_request_methods_and_paths() {
        let backend = HttpBackend::new("https://tracker.invalid", None).unwrap();
        let request = backend
            .request_builder(Method::DELETE, "/api/labels/4")
            .build()
            .unwrap();

        assert_eq!(request.method(), Method::DELETE);
        assert_eq!(request.url().path(), "/api/labels/4");
    }

    #[tokio::test]
    async fn executes_search_over_http_with_auth_and_query() {
        let captured = Arc::new(Mutex::new(None));
        let router = Router::new()
            .route("/api/search", any(capture_request))
            .with_state(captured.clone());
        let (url, server) = spawn_server(router).await;
        let backend = HttpBackend::new(&url, Some("test-key")).unwrap();

        let output = backend
            .execute(
                &Command::Search {
                    query: "term".into(),
                    project: None,
                    limit: Some(7),
                },
                IssueLinkOutput::Url,
            )
            .await
            .unwrap();

        assert_eq!(output, json!([{"id": 1}]));
        assert_eq!(
            captured.lock().await.as_ref(),
            Some(&(
                "/api/search?query=term&limit=7".into(),
                Some("Bearer test-key".into())
            ))
        );
        server.abort();
    }

    #[tokio::test]
    async fn execute_constructs_linked_issue_results() {
        let router = Router::new().route("/api/issues/resolve/{identifier}", get(issue_response));
        let (url, server) = spawn_server(router).await;
        let backend = HttpBackend::new(&url, None).unwrap();

        let output = backend
            .execute(
                &Command::Issue {
                    action: IssueAction::Get {
                        identifier: "LIF-42".into(),
                    },
                },
                IssueLinkOutput::Url,
            )
            .await
            .unwrap();

        assert_eq!(output["web_url"], format!("{url}/LIF/issues/LIF-42"));
        server.abort();
    }

    #[tokio::test]
    async fn executes_page_list_with_project_and_folder_resolution() {
        let router = Router::new().route("/api/{*path}", any(page_scope_request));
        let (url, server) = spawn_server(router).await;
        let backend = HttpBackend::new(&url, None).unwrap();

        let output = backend
            .execute(
                &Command::Page {
                    action: PageAction::List {
                        project: Some("LIF".into()),
                        folder: Some("Docs".into()),
                        label: None,
                    },
                },
                IssueLinkOutput::Url,
            )
            .await
            .unwrap();

        assert_eq!(output, json!([{"id": 9, "title": "Release notes"}]));
        server.abort();
    }

    #[tokio::test]
    async fn reports_http_error_details_from_server_responses() {
        let router = Router::new().route("/api/projects", get(failed_request));
        let (url, server) = spawn_server(router).await;
        let backend = HttpBackend::new(&url, None).unwrap();

        let error = backend
            .execute(
                &Command::Project {
                    action: ProjectAction::List,
                },
                IssueLinkOutput::Url,
            )
            .await
            .unwrap_err();

        assert_eq!(
            error.to_string(),
            "HTTP backend request failed (400 Bad Request): request rejected"
        );
        server.abort();
    }

    #[tokio::test]
    async fn bounds_http_error_response_bodies() {
        let router = Router::new().route("/api/projects", get(oversized_failed_request));
        let (url, server) = spawn_server(router).await;
        let backend = HttpBackend::new(&url, None).unwrap();

        let error = backend.get_json("/api/projects", &[]).await.unwrap_err();
        let prefix = "HTTP backend request failed (400 Bad Request): ";
        assert_eq!(error.to_string().len(), prefix.len() + ERROR_BODY_LIMIT);

        server.abort();
    }

    #[tokio::test]
    async fn does_not_follow_http_redirects() {
        let captured = Arc::new(Mutex::new(None));
        let router = Router::new()
            .route("/api/redirect", get(redirect_response))
            .route("/api/projects", any(capture_request))
            .with_state(captured.clone());
        let (url, server) = spawn_server(router).await;
        let backend = HttpBackend::new(&url, Some("test-key")).unwrap();

        let error = backend.get_json("/api/redirect", &[]).await.unwrap_err();

        assert!(
            error
                .to_string()
                .starts_with("HTTP backend request failed (302 Found):")
        );
        assert!(captured.lock().await.is_none());
        server.abort();
    }

    /// Unique per call: tests share a process, so a fixed directory name
    /// would have parallel tests writing over each other. The guard removes
    /// the directory when it drops, so hold it for as long as the export
    /// under test needs the path.
    fn scratch_dir(label: &str) -> TempDir {
        tempfile::Builder::new()
            .prefix(&format!("lific-{label}-"))
            .tempdir()
            .unwrap()
    }

    /// Every file under `root` as (path relative to `root`, contents),
    /// sorted, so two export directories can be compared as values.
    fn tree(root: &Path) -> Vec<(String, String)> {
        fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
            for entry in std::fs::read_dir(dir)
                .unwrap_or_else(|error| panic!("reading {}: {error}", dir.display()))
            {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, root, out);
                } else {
                    out.push((
                        relative(&path, root),
                        std::fs::read_to_string(&path).unwrap(),
                    ));
                }
            }
        }
        let mut out = Vec::new();
        walk(root, root, &mut out);
        out.sort();
        out
    }

    fn relative(path: &Path, root: &Path) -> String {
        path.strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// Export the same thing through both backends and require the results to
    /// be indistinguishable: same files, same relative paths, same contents,
    /// and the same list of written paths reported back (LIF-341).
    async fn assert_export_parity(
        fixture: &RealApiFixture,
        backend: &HttpBackend,
        label: &str,
        action: impl Fn(PathBuf) -> ExportAction,
    ) {
        let remote_tmp = scratch_dir(&format!("export-http-{label}"));
        let local_tmp = scratch_dir(&format!("export-sql-{label}"));
        let remote_dir = remote_tmp.path().to_path_buf();
        let local_dir = local_tmp.path().to_path_buf();

        let reported = backend
            .execute(
                &Command::Export {
                    action: action(remote_dir.clone()),
                },
                IssueLinkOutput::Url,
            )
            .await
            .unwrap();
        crate::cli::exec::run(
            &fixture.db,
            &Command::Export {
                action: action(local_dir.clone()),
            },
            false,
        )
        .unwrap();

        let remote = tree(&remote_dir);
        assert_eq!(
            remote,
            tree(&local_dir),
            "the {label} export differed between backends"
        );
        assert!(!remote.is_empty(), "the {label} export wrote nothing");
        assert!(
            remote.iter().all(|(path, _)| path.ends_with(".md")),
            "the {label} export left something other than markdown: {remote:?}"
        );

        // The paths the command reports are exactly the files it wrote, which
        // is what `--json` prints and what the shared renderer lists.
        let mut written: Vec<String> = decode::<Vec<String>>(&reported)
            .expect("export reports an array of paths")
            .iter()
            .map(|path| relative(Path::new(path), &remote_dir))
            .collect();
        written.sort();
        assert_eq!(
            written,
            remote
                .iter()
                .map(|(path, _)| path.clone())
                .collect::<Vec<_>>(),
            "the {label} export reported paths it did not write"
        );
    }

    /// LIF-341: `lific export` leaves the same tree on disk whichever backend
    /// ran it. A project still crosses the wire as one ZIP, so this also
    /// covers the HTTP backend unpacking that archive into the individual
    /// markdown files the SQL backend writes directly.
    #[tokio::test]
    async fn writes_remote_exports_into_the_same_tree_as_the_sql_backend() {
        let fixture = spawn_real_api_server().await;
        let backend = HttpBackend::new(&fixture.url, None).unwrap();

        // A page filed in a folder: its export path is nested below the
        // folder, so a backend that only knew the file's basename could not
        // reproduce it.
        let project_id = backend.project_id("TST").await.unwrap();
        let folder = backend
            .send_json(
                Method::POST,
                "/api/folders",
                &models::CreateFolder {
                    project_id,
                    parent_id: None,
                    name: "Design notes".into(),
                },
            )
            .await
            .unwrap();
        let filed_page = backend
            .send_json(
                Method::POST,
                "/api/pages",
                &json!({
                    "project_id": project_id,
                    "folder_id": folder["id"],
                    "title": "Filed page",
                    "content": "Body of the filed page"
                }),
            )
            .await
            .unwrap();
        let filed_page = filed_page["identifier"].as_str().unwrap().to_owned();

        let issue = fixture.issue_identifier.clone();
        assert_export_parity(&fixture, &backend, "issue", |output| ExportAction::Issue {
            identifier: issue.clone(),
            output,
        })
        .await;

        assert_export_parity(&fixture, &backend, "filed-page", |output| {
            ExportAction::Page {
                identifier: filed_page.clone(),
                output,
            }
        })
        .await;

        let workspace_page = fixture.workspace_page_identifier.clone();
        assert_export_parity(&fixture, &backend, "workspace-page", |output| {
            ExportAction::Page {
                identifier: workspace_page.clone(),
                output,
            }
        })
        .await;

        assert_export_parity(&fixture, &backend, "project", |output| {
            ExportAction::Project {
                project: "TST".into(),
                output,
            }
        })
        .await;

        fixture.server.abort();
    }

    /// The archive is unpacked, not saved: a served ZIP becomes the files it
    /// contains, at the paths its entries name, and no `.zip` is left behind.
    #[tokio::test]
    async fn unpacks_a_served_project_archive_into_its_files() {
        let bundle = crate::export::ExportBundle {
            root: "TST".into(),
            files: vec![
                crate::export::ExportFile {
                    path: "TST/issues/tst-1-first.md".into(),
                    content: "# First".into(),
                },
                crate::export::ExportFile {
                    path: "TST/pages/design-notes/tst-doc-1-filed.md".into(),
                    content: "# Filed".into(),
                },
            ],
        };
        let archive = crate::export::bundle_to_zip(&bundle).unwrap();
        let router = Router::new().route(
            "/api/export/projects/{identifier}",
            get(move || {
                let archive = archive.clone();
                async move {
                    let mut response = Response::new(Body::from(archive));
                    response
                        .headers_mut()
                        .insert(CONTENT_TYPE, HeaderValue::from_static("application/zip"));
                    response.headers_mut().insert(
                        CONTENT_DISPOSITION,
                        HeaderValue::from_static("attachment; filename=tst-export.zip"),
                    );
                    response
                }
            }),
        );
        let (url, server) = spawn_server(router).await;
        let backend = HttpBackend::new(&url, None).unwrap();
        let output_tmp = scratch_dir("export-archive");
        let output_dir = output_tmp.path().to_path_buf();

        let reported = backend
            .execute(
                &Command::Export {
                    action: ExportAction::Project {
                        project: "TST".into(),
                        output: output_dir.clone(),
                    },
                },
                IssueLinkOutput::Url,
            )
            .await
            .unwrap();

        assert_eq!(
            tree(&output_dir),
            vec![
                ("TST/issues/tst-1-first.md".to_owned(), "# First".to_owned()),
                (
                    "TST/pages/design-notes/tst-doc-1-filed.md".to_owned(),
                    "# Filed".to_owned()
                ),
            ]
        );
        assert_eq!(
            reported,
            json!([
                output_dir
                    .join("TST/issues/tst-1-first.md")
                    .display()
                    .to_string(),
                output_dir
                    .join("TST/pages/design-notes/tst-doc-1-filed.md")
                    .display()
                    .to_string(),
            ])
        );

        server.abort();
    }

    /// LIF-341: every list command prints the same thing through either
    /// backend, not just `issue list`. Each of these goes out over HTTP and
    /// is compared against the shared renderer fed straight from the
    /// database, so a response this binary cannot decode (which would quietly
    /// fall back to a JSON dump) fails the test instead of shipping.
    #[tokio::test]
    async fn renders_every_list_command_identically_to_the_sql_backend() {
        let fixture = spawn_real_api_server().await;
        let backend = HttpBackend::new(&fixture.url, None).unwrap();
        let project_id = backend.project_id("TST").await.unwrap();

        backend
            .send_json(
                Method::POST,
                "/api/modules",
                &models::CreateModule {
                    project_id,
                    name: "Core".into(),
                    description: "The engine".into(),
                    status: "active".into(),
                    emoji: None,
                },
            )
            .await
            .unwrap();
        backend
            .send_json(
                Method::POST,
                "/api/labels",
                &models::CreateLabel {
                    project_id,
                    name: "bug".into(),
                    color: "#ff0000".into(),
                },
            )
            .await
            .unwrap();
        backend
            .send_json(
                Method::POST,
                "/api/folders",
                &models::CreateFolder {
                    project_id,
                    parent_id: None,
                    name: "Design notes".into(),
                },
            )
            .await
            .unwrap();
        let issue = fixture.issue_identifier.clone();
        backend
            .execute(
                &Command::Comment {
                    action: CommentAction::Add {
                        identifier: issue.clone(),
                        content: "A remark\nover two lines".into(),
                        user: None,
                    },
                },
                IssueLinkOutput::Url,
            )
            .await
            .unwrap();

        let commands = [
            Command::Project {
                action: ProjectAction::List,
            },
            Command::Project {
                action: ProjectAction::Get {
                    identifier: "TST".into(),
                },
            },
            Command::Page {
                action: PageAction::List {
                    project: Some("TST".into()),
                    folder: None,
                    label: None,
                },
            },
            Command::Page {
                action: PageAction::Get {
                    identifier: fixture.project_page_identifier.clone(),
                },
            },
            Command::Search {
                query: "page".into(),
                project: None,
                limit: None,
            },
            Command::Comment {
                action: CommentAction::List {
                    identifier: issue.clone(),
                },
            },
            Command::Module {
                action: ModuleAction::List {
                    project: "TST".into(),
                },
            },
            Command::Label {
                action: LabelAction::List {
                    project: "TST".into(),
                },
            },
            Command::Folder {
                action: FolderAction::List {
                    project: "TST".into(),
                },
            },
        ];

        // Every HTTP round trip happens first, so the database connection
        // below is never held across an await.
        let mut remote = Vec::new();
        for command in &commands {
            let value = backend
                .execute(command, IssueLinkOutput::Url)
                .await
                .unwrap();
            remote.push(backend.human(command, &value).await);
        }

        let conn = fixture.db.read().unwrap();
        use crate::db::queries;
        let issue_id = queries::resolve_identifier(&conn, &issue).unwrap();
        let page_id =
            queries::resolve_page_identifier(&conn, &fixture.project_page_identifier).unwrap();
        let local = [
            render::project_list(&queries::list_projects(&conn).unwrap()),
            render::project_detail(&queries::get_project(&conn, project_id).unwrap()),
            render::page_list(
                &queries::list_pages(
                    &conn,
                    Some(project_id),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap(),
            ),
            render::page_detail(&queries::get_page(&conn, page_id).unwrap()),
            render::search_results(
                &queries::search(
                    &conn,
                    &models::SearchQuery {
                        query: "page".into(),
                        ..Default::default()
                    },
                )
                .unwrap(),
            ),
            render::comment_list(
                &queries::comments::list_comments(
                    &conn,
                    queries::comments::CommentParent::Issue(issue_id),
                    None,
                    None,
                )
                .unwrap(),
                &issue,
            ),
            render::module_list(&queries::list_modules(&conn, project_id).unwrap(), "TST"),
            render::label_list(&queries::list_labels(&conn, project_id).unwrap(), "TST"),
            render::folder_list(&queries::list_folders(&conn, project_id).unwrap(), "TST"),
        ];

        // `Command` is not `Debug`, so name the cases for the failure message.
        let names = [
            "project list",
            "project get",
            "page list",
            "page get",
            "search",
            "comment list",
            "module list",
            "label list",
            "folder list",
        ];
        assert_eq!(names.len(), commands.len());
        for ((name, remote), local) in names.iter().zip(&remote).zip(&local) {
            assert_eq!(remote, local, "backends disagreed on `{name}`");
            assert!(
                !remote.starts_with('{') && !remote.starts_with('['),
                "`{name}` fell back to a JSON dump: {remote}"
            );
        }

        fixture.server.abort();
    }

    /// LIF-341: a delete reports the same JSON either way. The SQL backend
    /// prints `render::Deleted`; the HTTP backend gets back only a `204 No
    /// Content` and used to answer with a nameless `{"deleted": true}` of its
    /// own, so it now builds that same value.
    #[tokio::test]
    async fn reports_deletes_the_way_the_sql_backend_does() {
        let fixture = spawn_real_api_server().await;
        let backend = HttpBackend::new(&fixture.url, None).unwrap();
        let project_id = backend.project_id("TST").await.unwrap();
        backend
            .send_json(
                Method::POST,
                "/api/labels",
                &models::CreateLabel {
                    project_id,
                    name: "chore".into(),
                    color: "#ffffff".into(),
                },
            )
            .await
            .unwrap();

        let deleted = backend
            .execute(
                &Command::Label {
                    action: LabelAction::Delete {
                        project: "TST".into(),
                        name: "chore".into(),
                    },
                },
                IssueLinkOutput::Url,
            )
            .await
            .unwrap();

        // The SQL backend prints exactly `render::Deleted::named(name)`.
        assert_eq!(
            deleted,
            serde_json::to_value(render::Deleted::named("chore")).unwrap()
        );
        assert_eq!(deleted, json!({"deleted": true, "name": "chore"}));
        fixture.server.abort();
    }

    /// A server too old to answer `format=json` sends the markdown file
    /// itself. That still produces a file rather than an error, the same
    /// courtesy `human` extends to payloads it cannot decode.
    #[tokio::test]
    async fn falls_back_to_the_server_filename_when_no_bundle_comes_back() {
        let router = Router::new().route("/api/export/issues/{identifier}", get(export_response));
        let (url, server) = spawn_server(router).await;
        let backend = HttpBackend::new(&url, None).unwrap();
        let output_tmp = scratch_dir("export-legacy");
        let output_dir = output_tmp.path().to_path_buf();

        let output = backend
            .execute(
                &Command::Export {
                    action: ExportAction::Issue {
                        identifier: "LIF-1".into(),
                        output: PathBuf::from(&output_dir),
                    },
                },
                IssueLinkOutput::Url,
            )
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(output_dir.join("report.txt")).unwrap(),
            "export contents"
        );
        assert_eq!(
            output,
            json!([output_dir.join("report.txt").display().to_string()])
        );
        server.abort();
    }

    #[tokio::test]
    async fn gets_project_page_over_http_against_real_api_router() {
        let fixture = spawn_real_api_server().await;
        let backend = HttpBackend::new(&fixture.url, None).unwrap();

        let page = backend
            .execute(
                &Command::Page {
                    action: PageAction::Get {
                        identifier: fixture.project_page_identifier,
                    },
                },
                IssueLinkOutput::Url,
            )
            .await
            .unwrap();

        assert_eq!(page["title"], "Project page");
        assert_eq!(page["identifier"], "TST-DOC-1");
        fixture.server.abort();
    }

    #[tokio::test]
    async fn gets_workspace_page_over_http_against_real_api_router() {
        let fixture = spawn_real_api_server().await;
        let backend = HttpBackend::new(&fixture.url, None).unwrap();

        let page = backend
            .execute(
                &Command::Page {
                    action: PageAction::Get {
                        identifier: fixture.workspace_page_identifier,
                    },
                },
                IssueLinkOutput::Url,
            )
            .await
            .unwrap();

        assert_eq!(page["title"], "Workspace page");
        assert_eq!(page["identifier"], "DOC-1");
        fixture.server.abort();
    }

    #[tokio::test]
    async fn gets_issue_over_http_against_real_api_router() {
        let fixture = spawn_real_api_server().await;
        let backend = HttpBackend::new(&fixture.url, None).unwrap();

        let issue = backend
            .execute(
                &Command::Issue {
                    action: IssueAction::Get {
                        identifier: fixture.issue_identifier,
                    },
                },
                IssueLinkOutput::Url,
            )
            .await
            .unwrap();

        assert_eq!(issue["title"], "Test issue");
        assert_eq!(issue["identifier"], "TST-1");
        fixture.server.abort();
    }

    #[tokio::test]
    async fn sanitizes_real_api_error_details() {
        let fixture = spawn_real_api_server().await;
        let backend = HttpBackend::new(&fixture.url, None).unwrap();

        let error = backend
            .execute(
                &Command::Page {
                    action: PageAction::Get {
                        identifier: "TST-DOC-\u{1b}[31m".into(),
                    },
                },
                IssueLinkOutput::Url,
            )
            .await
            .unwrap_err();
        let error = error.to_string();

        assert!(error.starts_with(
            "HTTP backend request failed (400 Bad Request): invalid page identifier: TST-DOC- [31m"
        ));
        assert!(!error.chars().any(|character| character.is_ascii_control()));
        fixture.server.abort();
    }

    #[test]
    fn encodes_identifier_path_segments() {
        assert_eq!(segment("DOC 1/2"), "DOC%201%2F2");
    }

    #[test]
    fn reduces_export_paths_to_safe_basenames() {
        assert_eq!(
            safe_filename("../outside/report.txt"),
            Some("report.txt".to_owned())
        );
        assert_eq!(
            safe_filename("/absolute/report.txt"),
            Some("report.txt".to_owned())
        );
        assert_eq!(safe_filename(".."), None);
    }

    #[test]
    fn parses_content_disposition_filenames() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_DISPOSITION,
            "attachment; filename=\"report.zip\"".parse().unwrap(),
        );
        assert_eq!(export_filename(&headers), Some("report.zip".to_owned()));
    }

    #[test]
    fn parses_encoded_content_disposition_filenames() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_DISPOSITION,
            "attachment; filename*=UTF-8''report%20final.zip"
                .parse()
                .unwrap(),
        );

        assert_eq!(
            export_filename(&headers),
            Some("report final.zip".to_owned())
        );
    }

    #[test]
    fn splits_csv_values_and_discards_empty_items() {
        assert_eq!(
            split_csv(" bug, ,urgent ,").collect::<Vec<_>>(),
            vec!["bug", "urgent"]
        );
        assert!(split_csv("").next().is_none());
    }

    #[test]
    fn resolves_resources_case_insensitively() {
        let resources = json!([
            {"id": 4, "name": "Backend"},
            {"id": 9, "name": "Docs"}
        ]);
        let resource = find_resource(resources, "name", "backend", "module").unwrap();
        assert_eq!(resource.id, 4);
        assert_eq!(resource.identifier, "Backend");
    }

    #[test]
    fn preserves_canonical_identity_from_resolved_object() {
        let resource = resource_from_object(
            json!({"id": 42, "identifier": "LIF-42"}),
            "identifier",
            "lif-042",
            "issue",
        )
        .unwrap();

        assert_eq!(resource.id, 42);
        assert_eq!(resource.identifier, "LIF-42");
    }

    #[test]
    fn ignores_resources_with_missing_ids() {
        let resources = json!([
            {"name": "Backend"},
            {"id": 9, "name": "Docs"}
        ]);
        let error = find_resource(resources, "name", "backend", "module").unwrap_err();
        assert_eq!(error.to_string(), "module 'backend' not found");
    }

    #[test]
    fn reports_missing_resource_name() {
        let error = find_resource(json!([]), "name", "missing", "folder").unwrap_err();
        assert_eq!(error.to_string(), "folder 'missing' not found");
    }

    #[test]
    fn reports_missing_resource_key() {
        let error = find_resource(
            json!([{"id": 2, "title": "Docs"}]),
            "name",
            "Docs",
            "folder",
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "folder 'Docs' not found");
    }

    fn issue_update() -> models::UpdateIssue {
        models::UpdateIssue {
            title: None,
            description: None,
            status: None,
            priority: None,
            module_id: None,
            sort_order: None,
            start_date: None,
            target_date: None,
            labels: None,
        }
    }

    #[test]
    fn preserves_optional_json_fields_only_when_set() {
        let body = serde_json::to_value(models::UpdateIssue {
            title: Some("Updated".into()),
            ..issue_update()
        })
        .unwrap();
        assert_eq!(body["title"], "Updated");
        assert!(body.get("description").is_none());
    }

    #[test]
    fn preserves_empty_optional_json_values() {
        let body = serde_json::to_value(models::UpdateIssue {
            description: Some(String::new()),
            ..issue_update()
        })
        .unwrap();
        assert_eq!(body["description"], "");
    }

    /// LIF-374: the HTTP backend used to declare its own `IssueUpdate` with a
    /// flat `Option<i64>` module id, which cannot express "clear the module"
    /// — `null` and "absent" collapsed into the same payload. Sending
    /// `models::UpdateIssue` keeps all three states distinguishable the way
    /// the server's `deserialize_nullable` reads them.
    #[test]
    fn distinguishes_absent_cleared_and_assigned_module_ids() {
        let absent = serde_json::to_value(issue_update()).unwrap();
        let cleared = serde_json::to_value(models::UpdateIssue {
            module_id: Some(None),
            ..issue_update()
        })
        .unwrap();
        let assigned = serde_json::to_value(models::UpdateIssue {
            module_id: Some(Some(7)),
            ..issue_update()
        })
        .unwrap();

        assert!(absent.get("module_id").is_none());
        assert_eq!(cleared["module_id"], serde_json::Value::Null);
        assert_eq!(assigned["module_id"], 7);
    }

    /// The flags `lific project update` exposes must reach the server
    /// unchanged, and the fields it does not expose must stay absent rather
    /// than being sent as nulls that would clear them (LIF-374: the old
    /// shadow struct carried only 2 of the 5 fields).
    #[test]
    fn sends_only_the_project_fields_the_cli_sets() {
        let body = serde_json::to_value(models::UpdateProject {
            name: Some("Docs".into()),
            identifier: None,
            description: Some("Reference material".into()),
            emoji: None,
            lead_user_id: None,
        })
        .unwrap();

        assert_eq!(body["name"], "Docs");
        assert_eq!(body["description"], "Reference material");
        assert!(body.get("identifier").is_none());
        assert!(body.get("emoji").is_none());
        assert!(body.get("lead_user_id").is_none());
    }

    #[test]
    fn extracts_structured_server_error_messages() {
        assert_eq!(
            error_detail(r#"{"error":"access denied"}"#),
            "access denied"
        );
        assert_eq!(error_detail("connection refused"), "connection refused");
        assert_eq!(
            error_detail(r#"{"message":"access denied"}"#),
            r#"{"message":"access denied"}"#
        );
    }

    #[test]
    fn sanitizes_ascii_control_characters_in_error_details() {
        assert_eq!(
            sanitize_error_detail("access\u{1b}[31m denied\n\t"),
            "access [31m denied  "
        );
    }

    #[test]
    fn builds_issue_create_payload_with_nullable_fields() {
        let payload = serde_json::to_value(models::CreateIssue {
            project_id: 7,
            title: "Broken link".into(),
            description: "Details".into(),
            status: models::Status::Backlog,
            priority: models::Priority::High,
            module_id: None,
            start_date: None,
            target_date: None,
            labels: vec!["bug".into()],
            source: None,
        })
        .unwrap();
        assert_eq!(payload["project_id"], 7);
        assert_eq!(payload["title"], "Broken link");
        assert_eq!(payload["labels"], json!(["bug"]));
        assert!(payload["module_id"].is_null());
        assert!(payload["source"].is_null());
    }

    #[test]
    fn builds_project_create_payload_with_server_defaults() {
        let payload = serde_json::to_value(models::CreateProject {
            name: "Docs".into(),
            identifier: "DOC".into(),
            description: "Reference material".into(),
            emoji: None,
            lead_user_id: None,
        })
        .unwrap();
        assert_eq!(payload["name"], "Docs");
        assert_eq!(payload["identifier"], "DOC");
        assert_eq!(payload["description"], "Reference material");
        assert!(payload["emoji"].is_null());
        assert!(payload["lead_user_id"].is_null());
    }

    #[test]
    fn builds_page_create_payload_for_workspace_pages() {
        let payload = serde_json::to_value(models::CreatePage {
            project_id: None,
            folder_id: None,
            title: "Runbook".into(),
            content: "# Steps".into(),
            status: "draft".into(),
            labels: Vec::new(),
        })
        .unwrap();
        assert!(payload["project_id"].is_null());
        assert!(payload["folder_id"].is_null());
        assert_eq!(payload["status"], "draft");
        assert_eq!(payload["labels"], json!([]));
    }

    #[test]
    fn builds_page_create_payload_with_project_folder_and_labels() {
        let payload = serde_json::to_value(models::CreatePage {
            project_id: Some(3),
            folder_id: Some(8),
            title: "Release".into(),
            content: "Notes".into(),
            status: "draft".into(),
            labels: vec!["ops".into(), "ship".into()],
        })
        .unwrap();
        assert_eq!(payload["project_id"], 3);
        assert_eq!(payload["folder_id"], 8);
        assert_eq!(payload["labels"], json!(["ops", "ship"]));
    }

    #[tokio::test]
    async fn rejects_commands_outside_http_data_scope() {
        let backend = HttpBackend::new("https://tracker.invalid", None).unwrap();
        let error = backend
            .execute(
                &Command::Start {
                    port: None,
                    host: None,
                },
                IssueLinkOutput::Url,
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "the HTTP backend does not support this command yet"
        );
    }

    #[test]
    fn constructs_http_resource_results_with_links() {
        let context = IssueLinkContext::parse("https://tracker.example/lific").unwrap();
        let issues = linked_resources(
            json!([{"identifier": "LIF-42", "title": "Fix"}]),
            &context,
            IssueLinkOutput::Url,
            ResourceKind::Issue,
        );
        let pages = linked_resources(
            json!([{"identifier": "LIF-DOC-3", "id": 17, "title": "Page"}]),
            &context,
            IssueLinkOutput::Url,
            ResourceKind::Page,
        );
        let projects = linked_resources(
            json!([
            {"identifier": "LIF", "id": 23, "name": "Project"},
            {"identifier": "OPS", "name": "Project without id"}
            ]),
            &context,
            IssueLinkOutput::Url,
            ResourceKind::Project,
        );

        assert_eq!(
            issues[0]["web_url"],
            "https://tracker.example/lific/LIF/issues/LIF-42"
        );
        assert_eq!(
            pages[0]["web_url"],
            "https://tracker.example/lific/LIF/pages/17"
        );
        assert_eq!(
            projects[0]["web_url"],
            "https://tracker.example/lific/LIF/overview"
        );
        assert_eq!(
            projects[1]["web_url"],
            "https://tracker.example/lific/OPS/overview"
        );
        assert_eq!(
            resource_url(
                pages[0].as_object().unwrap(),
                &context,
                "LIF-DOC-3",
                ResourceKind::Page,
            )
            .map(|url| url.to_string()),
            Some("https://tracker.example/lific/LIF/pages/17".into())
        );
        let markdown = linked_resources(
            json!([
                {"identifier": "LIF-42", "title": "Fix"},
                {"identifier": "LIF-43", "title": "Another fix"}
            ]),
            &context,
            IssueLinkOutput::Markdown,
            ResourceKind::Issue,
        );
        assert_eq!(
            markdown[0]["identifier"],
            "[LIF-42](https://tracker.example/lific/LIF/issues/LIF-42)"
        );
        assert_eq!(
            markdown[1]["identifier"],
            "[LIF-43](https://tracker.example/lific/LIF/issues/LIF-43)"
        );
    }

    #[test]
    fn constructs_http_search_comments_with_comment_anchors() {
        let context = IssueLinkContext::parse("https://tracker.example/lific").unwrap();
        let value = serde_json::to_value([
            crate::db::models::SearchResult {
                result_type: "comment".into(),
                id: 7,
                identifier: Some("LIF-42".into()),
                title: "Fix".into(),
                snippet: "Issue comment".into(),
                project_id: Some(1),
                parent_page_id: None,
            },
            crate::db::models::SearchResult {
                result_type: "comment".into(),
                id: 8,
                identifier: Some("LIF-DOC-3".into()),
                title: "Page".into(),
                snippet: "Page comment".into(),
                project_id: Some(1),
                parent_page_id: Some(17),
            },
        ])
        .unwrap();
        let markdown = linked_resources(
            value.clone(),
            &context,
            IssueLinkOutput::Markdown,
            ResourceKind::Search,
        );
        let value = linked_resources(value, &context, IssueLinkOutput::Url, ResourceKind::Search);

        assert_eq!(
            [value[0]["web_url"].as_str(), value[1]["web_url"].as_str()],
            [
                Some("https://tracker.example/lific/LIF/issues/LIF-42#comment-7"),
                Some("https://tracker.example/lific/LIF/pages/17#comment-8")
            ]
        );
        assert_eq!(
            [
                markdown[0]["identifier"].as_str(),
                markdown[1]["identifier"].as_str()
            ],
            [
                Some("[LIF-42](https://tracker.example/lific/LIF/issues/LIF-42#comment-7)"),
                Some("[LIF-DOC-3](https://tracker.example/lific/LIF/pages/17#comment-8)")
            ]
        );
    }

    #[test]
    fn constructs_http_comment_results_with_issue_anchor() {
        let context = IssueLinkContext::parse("https://tracker.example/lific").unwrap();
        let value = linked_comments(
            json!([{
                "id": 7,
                "content": "Looks good",
                "author_record": {"id": 99, "name": "Ada"}
            }]),
            &context,
            IssueLinkOutput::Markdown,
            "LIF-42",
        );

        assert_eq!(
            value[0]["comment"],
            "[comment #7](https://tracker.example/lific/LIF/issues/LIF-42#comment-7)"
        );
        assert!(value[0]["author_record"].get("comment").is_none());
    }

    #[test]
    fn constructs_http_comment_results_with_page_anchor() {
        let context = IssueLinkContext::parse("https://tracker.example/lific").unwrap();
        let value = linked_comments(
            json!([{
                "id": 8,
                "page_id": 17,
                "content": "Page comment"
            }]),
            &context,
            IssueLinkOutput::Markdown,
            "LIF-DOC-3",
        );

        assert_eq!(
            value[0]["comment"],
            "[comment #8](https://tracker.example/lific/LIF/pages/17#comment-8)"
        );
    }

    /// LIF-373: `lific issue list` must print the same thing whether it read
    /// the database directly or went over HTTP. Same data, one renderer,
    /// byte-identical output.
    #[tokio::test]
    async fn renders_issue_lists_identically_to_the_sql_backend() {
        let fixture = spawn_real_api_server().await;
        let backend = HttpBackend::new(&fixture.url, None).unwrap();
        let command = Command::Issue {
            action: IssueAction::List {
                project: "TST".into(),
                status: None,
                priority: None,
                module: None,
                label: None,
                workable: false,
                limit: None,
            },
        };

        // A module-assigned issue as well: the SQL backend reads the module
        // name from the database, the HTTP backend has to go fetch it.
        let project_id = backend.project_id("TST").await.unwrap();
        let module = backend
            .send_json(
                Method::POST,
                "/api/modules",
                &models::CreateModule {
                    project_id,
                    name: "Core".into(),
                    description: String::new(),
                    status: "active".into(),
                    emoji: None,
                },
            )
            .await
            .unwrap();
        backend
            .send_json(
                Method::POST,
                "/api/issues",
                &models::CreateIssue {
                    project_id,
                    title: "Modular issue".into(),
                    description: String::new(),
                    status: models::Status::Active,
                    priority: models::Priority::High,
                    module_id: module["id"].as_i64(),
                    start_date: None,
                    target_date: None,
                    labels: Vec::new(),
                    source: None,
                },
            )
            .await
            .unwrap();

        let value = backend.execute(&command, IssueLinkOutput::Url).await.unwrap();
        let remote = backend.human(&command, &value).await;

        let pool = crate::db::open_memory().expect("test db");
        {
            let conn = pool.write().unwrap();
            let project = crate::db::queries::create_project(
                &conn,
                &models::CreateProject {
                    name: "Test Project".into(),
                    identifier: "TST".into(),
                    description: "integration test project".into(),
                    emoji: None,
                    lead_user_id: None,
                },
            )
            .unwrap();
            crate::db::queries::create_issue(
                &conn,
                &models::CreateIssue {
                    project_id: project.id,
                    title: "Test issue".into(),
                    description: String::new(),
                    status: models::Status::Backlog,
                    priority: models::Priority::None,
                    module_id: None,
                    start_date: None,
                    target_date: None,
                    labels: Vec::new(),
                    source: None,
                },
            )
            .unwrap();
            let module = crate::db::queries::create_module(
                &conn,
                &models::CreateModule {
                    project_id: project.id,
                    name: "Core".into(),
                    description: String::new(),
                    status: "active".into(),
                    emoji: None,
                },
            )
            .unwrap();
            crate::db::queries::create_issue(
                &conn,
                &models::CreateIssue {
                    project_id: project.id,
                    title: "Modular issue".into(),
                    description: String::new(),
                    status: models::Status::Active,
                    priority: models::Priority::High,
                    module_id: Some(module.id),
                    start_date: None,
                    target_date: None,
                    labels: Vec::new(),
                    source: None,
                },
            )
            .unwrap();
        }
        let conn = pool.read().unwrap();
        let issues = crate::db::queries::list_issues(
            &conn,
            &models::ListIssuesQuery {
                project_id: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
        let module_name = |id: i64| crate::db::queries::get_module_name(&conn, id).ok();
        let local = render::issue_list(&issues, &module_name);

        assert_eq!(remote, local);
        assert!(remote.contains("TST-2"), "{remote}");
        assert!(remote.contains("(Core)"), "{remote}");
        fixture.server.abort();
    }

    /// A server that answers with a shape this binary does not know (an older
    /// or newer release) still prints its payload instead of failing at the
    /// last step.
    #[tokio::test]
    async fn falls_back_to_json_for_unrecognized_responses() {
        let backend = HttpBackend::new("https://tracker.invalid", None).unwrap();

        let rendered = backend
            .human(
                &Command::Project {
                    action: ProjectAction::List,
                },
                &json!({"unexpected": true}),
            )
            .await;

        assert_eq!(rendered, "{\n  \"unexpected\": true\n}\n");
    }

    #[test]
    fn constructs_http_module_results_with_project_route() {
        let context = IssueLinkContext::parse("https://tracker.example/lific").unwrap();
        let value = linked_modules(
            json!([{
                "id": 23,
                "project_id": 1,
                "name": "Backend [internal]",
                "owner": {"id": 99, "name": "Ada"}
            }]),
            &context,
            IssueLinkOutput::Markdown,
            "LIF",
        );

        assert_eq!(
            value[0]["name"],
            "[Backend \\[internal\\]](https://tracker.example/lific/LIF/modules/23)"
        );
        assert_eq!(value[0]["owner"]["name"], "Ada");
        assert!(value[0]["owner"].get("web_url").is_none());
    }
}
