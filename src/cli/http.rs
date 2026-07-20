//! HTTP transport for data-oriented CLI commands.
//!
//! The SQL executor and this module intentionally share the `Command` enum:
//! command parsing and output selection stay transport-independent while each
//! backend owns only identifier resolution and I/O.

use std::{borrow::Cow, fs, io::Write, net::IpAddr, path::Path, time::Duration};

use anyhow::{Result, anyhow, bail};
use reqwest::{
    Client, Method, RequestBuilder, StatusCode,
    header::{CONTENT_DISPOSITION, HeaderMap},
};
use serde::Serialize;
use serde_json::{Value, json};

use super::{
    Command, CommentAction, ExportAction, FolderAction, IssueAction, LabelAction, ModuleAction,
    PageAction, ProjectAction, borrowed_labels,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const ERROR_BODY_LIMIT: usize = 64 * 1024;
type QueryParam<'a> = (&'a str, Cow<'a, str>);

#[derive(Serialize)]
struct IssueCreate<'a> {
    project_id: i64,
    title: &'a str,
    description: &'a str,
    status: &'a str,
    priority: &'a str,
    module_id: Option<i64>,
    start_date: Option<&'a str>,
    target_date: Option<&'a str>,
    labels: &'a [&'a str],
    source: Option<&'a str>,
}

#[derive(Serialize)]
struct IssueUpdate<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    priority: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    module_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    labels: Option<&'a [&'a str]>,
}

#[derive(Serialize)]
struct ProjectCreate<'a> {
    name: &'a str,
    identifier: &'a str,
    description: &'a str,
    emoji: Option<&'a str>,
    lead_user_id: Option<i64>,
}

#[derive(Serialize)]
struct ProjectUpdate<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
}

#[derive(Serialize)]
struct PageCreate<'a> {
    project_id: Option<i64>,
    folder_id: Option<i64>,
    title: &'a str,
    content: &'a str,
    status: &'static str,
    labels: &'a [&'a str],
}

#[derive(Serialize)]
struct PageUpdate<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    folder_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    labels: Option<&'a [&'a str]>,
}

#[derive(Serialize)]
struct CommentCreate<'a> {
    content: &'a str,
}

#[derive(Serialize)]
struct ModuleCreate<'a> {
    project_id: i64,
    name: &'a str,
    description: &'a str,
    status: &'a str,
    emoji: Option<&'a str>,
}

#[derive(Serialize)]
struct ModuleUpdate<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<&'a str>,
}

#[derive(Serialize)]
struct LabelCreate<'a> {
    project_id: i64,
    name: &'a str,
    color: &'a str,
}

#[derive(Serialize)]
struct LabelUpdate<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    color: Option<&'a str>,
}

#[derive(Serialize)]
struct FolderCreate<'a> {
    project_id: i64,
    parent_id: Option<i64>,
    name: &'a str,
}

#[derive(Serialize)]
struct FolderUpdate<'a> {
    name: &'a str,
}

pub async fn run(
    command: &Command,
    base_url: &str,
    api_key: Option<&str>,
    json_output: bool,
) -> Result<()> {
    let backend = HttpBackend::new(base_url, api_key)?;
    let output = backend.execute(command).await?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        print_human(&output);
    }
    Ok(())
}

struct HttpBackend {
    client: Client,
    base_url: String,
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
        if parsed.scheme() == "http"
            && parsed
                .host_str()
                .is_some_and(|host| !is_loopback_host(host))
        {
            eprintln!(
                "warning: sending credentials over unencrypted http to {}",
                parsed.host_str().unwrap_or_default()
            );
        }
        Ok(Self {
            client: Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            base_url: base_url.to_owned(),
            api_key: api_key.map(str::to_owned),
        })
    }

    async fn execute(&self, command: &Command) -> Result<Value> {
        match command {
            Command::Issue { action } => self.issue(action).await,
            Command::Project { action } => self.project(action).await,
            Command::Page { action } => self.page(action).await,
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
                self.get_json("/api/search", &params).await
            }
            Command::Comment { action } => self.comment(action).await,
            Command::Module { action } => self.module(action).await,
            Command::Label { action } => self.label(action).await,
            Command::Folder { action } => self.folder(action).await,
            _ => bail!("the HTTP backend does not support this command yet"),
        }
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
                let labels = borrowed_labels(labels.as_deref()).unwrap_or_default();
                let body = IssueCreate {
                    project_id,
                    title,
                    description,
                    status,
                    priority,
                    module_id,
                    start_date: None,
                    target_date: None,
                    labels: &labels,
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
                let labels = borrowed_labels(labels.as_deref());
                let body = IssueUpdate {
                    title: title.as_deref(),
                    description: description.as_deref(),
                    status: status.as_deref(),
                    priority: priority.as_deref(),
                    module_id,
                    labels: labels.as_deref(),
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
                    &ProjectCreate {
                        name,
                        identifier,
                        description,
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
                let body = ProjectUpdate {
                    name: name.as_deref(),
                    description: description.as_deref(),
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
                let labels = borrowed_labels(labels.as_deref()).unwrap_or_default();
                self.send_json(
                    Method::POST,
                    "/api/pages",
                    &PageCreate {
                        project_id,
                        folder_id,
                        title,
                        content,
                        status: "draft",
                        labels: &labels,
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
                let labels = borrowed_labels(labels.as_deref());
                let body = PageUpdate {
                    title: title.as_deref(),
                    content: content.as_deref(),
                    folder_id,
                    labels: labels.as_deref(),
                };
                self.send_json(Method::PUT, &format!("/api/pages/{id}"), &body)
                    .await
            }
        }
    }

    async fn comment(&self, action: &CommentAction) -> Result<Value> {
        match action {
            CommentAction::List { identifier } => {
                let id = self.issue_id(identifier).await?;
                self.get_json(&format!("/api/issues/{id}/comments"), &[])
                    .await
            }
            CommentAction::Add {
                identifier,
                content,
                user,
            } => {
                if user.is_some() {
                    bail!(
                        "--user cannot be used with the HTTP backend; the server uses the API credential's user"
                    );
                }
                let id = self.issue_id(identifier).await?;
                self.send_json(
                    Method::POST,
                    &format!("/api/issues/{id}/comments"),
                    &CommentCreate { content },
                )
                .await
            }
        }
    }

    async fn module(&self, action: &ModuleAction) -> Result<Value> {
        match action {
            ModuleAction::List { project } => {
                let id = self.project_id(project).await?;
                self.get_json(
                    "/api/modules",
                    &[("project_id", Cow::Owned(id.to_string()))],
                )
                .await
            }
            ModuleAction::Create {
                project,
                name,
                description,
                status,
            } => {
                let project_id = self.project_id(project).await?;
                self.send_json(
                    Method::POST,
                    "/api/modules",
                    &ModuleCreate {
                        project_id,
                        name,
                        description,
                        status,
                        emoji: None,
                    },
                )
                .await
            }
            ModuleAction::Update {
                project,
                name,
                new_name,
                description,
                status,
            } => {
                let project_id = self.project_id(project).await?;
                let id = self.module_id(project_id, name).await?;
                let body = ModuleUpdate {
                    name: new_name.as_deref(),
                    description: description.as_deref(),
                    status: status.as_deref(),
                };
                self.send_json(Method::PUT, &format!("/api/modules/{id}"), &body)
                    .await
            }
            ModuleAction::Delete { project, name } => {
                let id = self
                    .module_id(self.project_id(project).await?, name)
                    .await?;
                self.send_empty(Method::DELETE, &format!("/api/modules/{id}"))
                    .await
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
                    &LabelCreate {
                        project_id,
                        name,
                        color,
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
                let body = LabelUpdate {
                    name: new_name.as_deref(),
                    color: color.as_deref(),
                };
                self.send_json(Method::PUT, &format!("/api/labels/{id}"), &body)
                    .await
            }
            LabelAction::Delete { project, name } => {
                let id = self.label_id(self.project_id(project).await?, name).await?;
                self.send_empty(Method::DELETE, &format!("/api/labels/{id}"))
                    .await
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
                    &FolderCreate {
                        project_id,
                        parent_id: None,
                        name,
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
                    &FolderUpdate { name: new_name },
                )
                .await
            }
            FolderAction::Delete { project, name } => {
                let id = self
                    .folder_id(self.project_id(project).await?, name)
                    .await?;
                self.send_empty(Method::DELETE, &format!("/api/folders/{id}"))
                    .await
            }
        }
    }

    async fn export(&self, action: &ExportAction) -> Result<Value> {
        let (path, output) = match action {
            ExportAction::Issue { identifier, output } => (
                format!("/api/export/issues/{}", segment(identifier)),
                output,
            ),
            ExportAction::Page { identifier, output } => {
                (format!("/api/export/pages/{}", segment(identifier)), output)
            }
            ExportAction::Project { project, output } => {
                (format!("/api/export/projects/{}", segment(project)), output)
            }
        };
        let mut response = self.send(self.request_builder(Method::GET, &path)).await?;
        let filename = export_filename(response.headers()).unwrap_or_else(|| "export.bin".into());
        fs::create_dir_all(output)?;
        let path = output.join(&filename);
        let mut file = fs::File::create(&path)?;
        while let Some(chunk) = response.chunk().await? {
            file.write_all(&chunk)?;
        }
        Ok(json!([path.display().to_string()]))
    }

    async fn project_id(&self, identifier: &str) -> Result<i64> {
        self.resolve_id("/api/projects", "identifier", identifier, "project", &[])
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
        self.get_json(&format!("/api/issues/resolve/{}", segment(identifier)), &[])
            .await?["id"]
            .as_i64()
            .ok_or_else(|| anyhow!("issue '{identifier}' response had no id"))
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

    async fn send_empty(&self, method: Method, path: &str) -> Result<Value> {
        let response = self.send(self.request_builder(method, path)).await?;
        if response.status() == StatusCode::NO_CONTENT {
            Ok(json!({"deleted": true}))
        } else {
            Ok(response.json().await?)
        }
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

fn find_resource(value: Value, key: &str, expected: &str, kind: &str) -> Result<i64> {
    value
        .as_array()
        .and_then(|items| {
            items.iter().find(|item| {
                item[key]
                    .as_str()
                    .is_some_and(|value| value.eq_ignore_ascii_case(expected))
            })
        })
        .and_then(|item| item["id"].as_i64())
        .ok_or_else(|| anyhow!("{kind} '{expected}' not found"))
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

fn print_human(value: &Value) {
    match value {
        Value::Array(items) => println!("{} item(s):\n{}", items.len(), pretty(value)),
        _ => println!("{}", pretty(value)),
    }
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc};

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
        header::{CONTENT_DISPOSITION, HeaderMap},
    };
    use serde_json::json;
    use tokio::{net::TcpListener, sync::Mutex, task::JoinHandle};

    use crate::{
        api::test_helpers::{
            json_post, parse_json, seed_project, test_peer, with_attachment_layers,
            with_client_ip_test_layers,
        },
        cli::{Command, ExportAction, IssueAction, PageAction, ProjectAction, split_csv},
        config::AuthConfig,
        db::models::AuthUser,
        realtime::RealtimeHub,
    };

    use super::{
        ERROR_BODY_LIMIT, HttpBackend, IssueCreate, IssueUpdate, PageCreate, ProjectCreate,
        error_detail, export_filename, find_resource, is_loopback_host, safe_filename,
        sanitize_error_detail, segment,
    };

    type CapturedRequest = Arc<Mutex<Option<(String, Option<String>)>>>;

    struct RealApiFixture {
        url: String,
        project_page_identifier: String,
        workspace_page_identifier: String,
        issue_identifier: String,
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
            with_attachment_layers(crate::api::router(db, &[])),
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
        let workspace_page = parse_json(
            json_post(&app, "/api/pages", json!({"title": "Workspace page"})).await,
        )
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
    fn identifies_loopback_hosts_for_plaintext_warning() {
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("LOCALHOST"));
        assert!(is_loopback_host("127.1.2.3"));
        assert!(is_loopback_host("::1"));
        assert!(!is_loopback_host("0.0.0.0"));
        assert!(!is_loopback_host("tracker.example"));
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
            .execute(&Command::Search {
                query: "term".into(),
                project: None,
                limit: Some(7),
            })
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
    async fn executes_page_list_with_project_and_folder_resolution() {
        let router = Router::new().route("/api/{*path}", any(page_scope_request));
        let (url, server) = spawn_server(router).await;
        let backend = HttpBackend::new(&url, None).unwrap();

        let output = backend
            .execute(&Command::Page {
                action: PageAction::List {
                    project: Some("LIF".into()),
                    folder: Some("Docs".into()),
                    label: None,
                },
            })
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
            .execute(&Command::Project {
                action: ProjectAction::List,
            })
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

        assert!(error
            .to_string()
            .starts_with("HTTP backend request failed (302 Found):"));
        assert!(captured.lock().await.is_none());
        server.abort();
    }

    #[tokio::test]
    async fn writes_http_export_response_using_server_filename() {
        let router = Router::new().route("/api/export/issues/{identifier}", get(export_response));
        let (url, server) = spawn_server(router).await;
        let backend = HttpBackend::new(&url, None).unwrap();
        let output_dir =
            std::env::temp_dir().join(format!("lific-http-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&output_dir);

        let output = backend
            .execute(&Command::Export {
                action: ExportAction::Issue {
                    identifier: "LIF-1".into(),
                    output: PathBuf::from(&output_dir),
                },
            })
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(output_dir.join("report.txt")).unwrap(),
            "export contents"
        );
        assert_eq!(output, json!([output_dir.join("report.txt").display().to_string()]));
        std::fs::remove_dir_all(output_dir).unwrap();
        server.abort();
    }

    #[tokio::test]
    async fn gets_project_page_over_http_against_real_api_router() {
        let fixture = spawn_real_api_server().await;
        let backend = HttpBackend::new(&fixture.url, None).unwrap();

        let page = backend
            .execute(&Command::Page {
                action: PageAction::Get {
                    identifier: fixture.project_page_identifier,
                },
            })
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
            .execute(&Command::Page {
                action: PageAction::Get {
                    identifier: fixture.workspace_page_identifier,
                },
            })
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
            .execute(&Command::Issue {
                action: IssueAction::Get {
                    identifier: fixture.issue_identifier,
                },
            })
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
            .execute(&Command::Page {
                action: PageAction::Get {
                    identifier: "TST-DOC-\u{1b}[31m".into(),
                },
            })
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
        assert_eq!(
            find_resource(resources, "name", "backend", "module").unwrap(),
            4
        );
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

    #[test]
    fn preserves_optional_json_fields_only_when_set() {
        let body = serde_json::to_value(IssueUpdate {
            title: Some("Updated"),
            description: None,
            status: None,
            priority: None,
            module_id: None,
            labels: None,
        })
        .unwrap();
        assert_eq!(body["title"], "Updated");
        assert!(body.get("description").is_none());
    }

    #[test]
    fn preserves_empty_optional_json_values() {
        let body = serde_json::to_value(IssueUpdate {
            title: None,
            description: Some(""),
            status: None,
            priority: None,
            module_id: None,
            labels: None,
        })
        .unwrap();
        assert_eq!(body["description"], "");
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
        let labels = vec!["bug"];
        let payload = serde_json::to_value(IssueCreate {
            project_id: 7,
            title: "Broken link",
            description: "Details",
            status: "backlog",
            priority: "high",
            module_id: None,
            start_date: None,
            target_date: None,
            labels: &labels,
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
        let payload = serde_json::to_value(ProjectCreate {
            name: "Docs",
            identifier: "DOC",
            description: "Reference material",
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
        let labels = Vec::new();
        let payload = serde_json::to_value(PageCreate {
            project_id: None,
            folder_id: None,
            title: "Runbook",
            content: "# Steps",
            status: "draft",
            labels: &labels,
        })
        .unwrap();
        assert!(payload["project_id"].is_null());
        assert!(payload["folder_id"].is_null());
        assert_eq!(payload["status"], "draft");
        assert_eq!(payload["labels"], json!([]));
    }

    #[test]
    fn builds_page_create_payload_with_project_folder_and_labels() {
        let labels = vec!["ops", "ship"];
        let payload = serde_json::to_value(PageCreate {
            project_id: Some(3),
            folder_id: Some(8),
            title: "Release",
            content: "Notes",
            status: "draft",
            labels: &labels,
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
            .execute(&Command::Start {
                port: None,
                host: None,
            })
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "the HTTP backend does not support this command yet"
        );
    }
}
