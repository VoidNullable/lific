use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::IntoResponse;
use axum::Extension;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::sync::OwnedSemaphorePermit;

use crate::authz;
use crate::db::models::Role;
use crate::db::DbPool;
use crate::error::LificError;

use super::with_read;

const EXPORT_STREAM_CHUNK_BYTES: usize = 64 * 1024;
const EXPORT_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const EXPORT_STREAM_MAX_DURATION: Duration = Duration::from_secs(30 * 60);

#[derive(serde::Deserialize)]
pub(super) struct ExportQuery {
    pub format: Option<String>,
}

struct PreparedExport {
    temp_dir: tempfile::TempDir,
    path: std::path::PathBuf,
    content_type: HeaderValue,
    download_name: Option<String>,
}

impl PreparedExport {
    fn json(bundle: &crate::export::ExportBundle) -> Result<Self, LificError> {
        let temp_dir = export_temp_dir()?;
        let path = temp_dir.path().join("export.json");
        crate::export::bundle_to_json_file(bundle, &path)?;
        Ok(Self {
            temp_dir,
            path,
            content_type: HeaderValue::from_static("application/json"),
            download_name: None,
        })
    }

    fn markdown(
        bundle: crate::export::ExportBundle,
        fallback_name: &str,
    ) -> Result<Self, LificError> {
        let file = bundle.files.into_iter().next().ok_or_else(|| {
            LificError::Internal(format!("export produced no files for {fallback_name}"))
        })?;
        let download_name = file
            .path
            .rsplit('/')
            .next()
            .unwrap_or(fallback_name)
            .to_string();
        let temp_dir = export_temp_dir()?;
        let path = temp_dir.path().join("export.md");
        std::fs::write(&path, file.content)
            .map_err(|error| LificError::Internal(format!("write export file: {error}")))?;
        Ok(Self {
            temp_dir,
            path,
            content_type: HeaderValue::from_static("text/markdown; charset=utf-8"),
            download_name: Some(download_name),
        })
    }

    fn zip(bundle: &crate::export::ExportBundle) -> Result<Self, LificError> {
        let download_name = format!("{}-export.zip", bundle.root.to_ascii_lowercase());
        let temp_dir = export_temp_dir()?;
        let path = temp_dir.path().join("export.zip");
        crate::export::bundle_to_zip_file(bundle, &path)?;
        Ok(Self {
            temp_dir,
            path,
            content_type: HeaderValue::from_static("application/zip"),
            download_name: Some(download_name),
        })
    }
}

fn export_temp_dir() -> Result<tempfile::TempDir, LificError> {
    tempfile::tempdir()
        .map_err(|error| LificError::Internal(format!("create export temp dir: {error}")))
}

async fn blocking<T>(
    operation: impl FnOnce() -> Result<T, LificError> + Send + 'static,
) -> Result<T, LificError>
where
    T: Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| LificError::Internal(format!("export worker failed: {error}")))?
}

#[cfg(test)]
struct ExportTestGate {
    started: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}

#[cfg(test)]
impl ExportTestGate {
    fn new(
        started: tokio::sync::oneshot::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    ) -> Self {
        Self {
            started: std::sync::Mutex::new(Some(started)),
            release: std::sync::Mutex::new(release),
        }
    }

    fn wait(&self) {
        let Some(started) = self.started.lock().unwrap().take() else {
            return;
        };
        let _ = started.send(());
        self.release.lock().unwrap().recv().unwrap();
    }
}

#[cfg(test)]
tokio::task_local! {
    static EXPORT_TEST_GATE: std::sync::Arc<ExportTestGate>;
}

async fn blocking_export<T>(
    permit: OwnedSemaphorePermit,
    operation: impl FnOnce() -> Result<T, LificError> + Send + 'static,
) -> Result<(T, OwnedSemaphorePermit), LificError>
where
    T: Send + 'static,
{
    #[cfg(test)]
    let gate = EXPORT_TEST_GATE.try_with(std::sync::Arc::clone).ok();
    blocking(move || {
        #[cfg(test)]
        if let Some(gate) = gate {
            gate.wait();
        }
        Ok((operation()?, permit))
    })
    .await
}

async fn single_file_response(
    bundle: crate::export::ExportBundle,
    format: &str,
    fallback_name: &'static str,
    permit: OwnedSemaphorePermit,
) -> Result<axum::response::Response, LificError> {
    let (prepared, permit) = match format {
        "json" => blocking_export(permit, move || PreparedExport::json(&bundle)).await?,
        "markdown" => {
            blocking_export(permit, move || {
                PreparedExport::markdown(bundle, fallback_name)
            })
            .await?
        }
        _ => unreachable!("format was validated before export"),
    };
    stream_response(prepared, permit).await
}

async fn stream_response(
    prepared: PreparedExport,
    permit: OwnedSemaphorePermit,
) -> Result<axum::response::Response, LificError> {
    stream_response_with_timeouts(
        prepared,
        permit,
        EXPORT_STREAM_IDLE_TIMEOUT,
        EXPORT_STREAM_MAX_DURATION,
    )
    .await
}

async fn stream_response_with_timeouts(
    prepared: PreparedExport,
    permit: OwnedSemaphorePermit,
    idle_timeout: Duration,
    max_duration: Duration,
) -> Result<axum::response::Response, LificError> {
    let mut file = tokio::fs::File::open(&prepared.path)
        .await
        .map_err(|error| LificError::Internal(format!("open export file: {error}")))?;
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    let (terminal_sender, terminal_receiver) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _temp_dir = prepared.temp_dir;
        let _permit = permit;
        let mut buffer = vec![0; EXPORT_STREAM_CHUNK_BYTES];
        let result = tokio::time::timeout(max_duration, async {
            loop {
                match file.read(&mut buffer).await {
                    Ok(0) => return Ok(()),
                    Ok(read) => {
                        let chunk = Bytes::copy_from_slice(&buffer[..read]);
                        match tokio::time::timeout(idle_timeout, sender.send(chunk)).await {
                            Ok(Ok(())) => {}
                            Ok(Err(_)) => return Ok(()),
                            Err(_) => {
                                return Err(std::io::Error::new(
                                    std::io::ErrorKind::TimedOut,
                                    "export stream idle timeout",
                                ));
                            }
                        }
                    }
                    Err(error) => return Err(error),
                }
            }
        })
        .await;
        let result = result.unwrap_or_else(|_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "export stream deadline exceeded",
            ))
        });
        let _ = terminal_sender.send(result);
    });
    let body = stream_body(receiver, terminal_receiver);
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, prepared.content_type);
    if let Some(filename) = prepared.download_name {
        headers.insert(header::CONTENT_DISPOSITION, content_disposition(&filename)?);
    }
    Ok((headers, body).into_response())
}

fn stream_body(
    receiver: tokio::sync::mpsc::Receiver<Bytes>,
    terminal: tokio::sync::oneshot::Receiver<std::io::Result<()>>,
) -> Body {
    Body::from_stream(futures_util::stream::unfold(
        (receiver, Some(terminal)),
        |(mut receiver, mut terminal)| async move {
            if let Some(chunk) = receiver.recv().await {
                return Some((Ok::<_, std::io::Error>(chunk), (receiver, terminal)));
            }
            let result = terminal.take()?.await.unwrap_or_else(|_| {
                Err(std::io::Error::other(
                    "export stream task ended without a result",
                ))
            });
            result.err().map(|error| (Err(error), (receiver, terminal)))
        },
    ))
}

fn content_disposition(filename: &str) -> Result<HeaderValue, LificError> {
    HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
        .map_err(|e| LificError::Internal(format!("invalid content-disposition header: {e}")))
}

pub(super) async fn export_issue(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(identifier): Path<String>,
    Query(q): Query<ExportQuery>,
) -> Result<impl IntoResponse, LificError> {
    if let Some(format) = q.format.as_deref()
        && !matches!(format, "json" | "markdown")
    {
        return Err(LificError::BadRequest(
            "invalid export format. Expected 'markdown' or 'json'".into(),
        ));
    }
    let project_id = with_read(&db, |conn| {
        let id = crate::db::queries::resolve_identifier(conn, &identifier)?;
        crate::db::queries::issue_project_id(conn, id)
    })?;
    authz::require_role(&db, &identity, project_id, Role::Viewer)?;
    let slot = db.acquire_export_slot()?;
    let (bundle, slot) = blocking_export(slot, move || {
        with_read(&db, |conn| crate::export::export_issue(conn, &identifier))
    })
    .await?;
    single_file_response(
        bundle,
        q.format.as_deref().unwrap_or("markdown"),
        "issue.md",
        slot,
    )
    .await
}

pub(super) async fn export_page(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(identifier): Path<String>,
    Query(q): Query<ExportQuery>,
) -> Result<impl IntoResponse, LificError> {
    if let Some(format) = q.format.as_deref()
        && !matches!(format, "json" | "markdown")
    {
        return Err(LificError::BadRequest(
            "invalid export format. Expected 'markdown' or 'json'".into(),
        ));
    }
    let project_id = with_read(&db, |conn| {
        let id = crate::db::queries::resolve_page_identifier(conn, &identifier)?;
        crate::db::queries::page_project_id(conn, id)
    })?;
    match project_id {
        Some(pid) => authz::require_role(&db, &identity, pid, Role::Viewer)?,
        None => authz::require_workspace_admin(&db, &identity)?,
    }
    let slot = db.acquire_export_slot()?;
    let (bundle, slot) = blocking_export(slot, move || {
        with_read(&db, |conn| crate::export::export_page(conn, &identifier))
    })
    .await?;
    single_file_response(
        bundle,
        q.format.as_deref().unwrap_or("markdown"),
        "page.md",
        slot,
    )
    .await
}

pub(super) async fn export_project(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(identifier): Path<String>,
    Query(q): Query<ExportQuery>,
) -> Result<impl IntoResponse, LificError> {
    if let Some(format) = q.format.as_deref()
        && !matches!(format, "json" | "zip")
    {
        return Err(LificError::BadRequest(
            "invalid export format. Expected 'zip' or 'json'".into(),
        ));
    }
    let project_id = with_read(&db, |conn| {
        crate::db::queries::resolve_project_identifier(conn, &identifier)
    })?;
    authz::require_role(&db, &identity, project_id, Role::Viewer)?;
    let slot = db.acquire_export_slot()?;
    let format = q.format.unwrap_or_else(|| "zip".into());
    let (bundle, slot) = blocking_export(slot, move || {
        with_read(&db, |conn| crate::export::export_project(conn, &identifier))
    })
    .await?;
    let (prepared, slot) = match format.as_str() {
        "json" => blocking_export(slot, move || PreparedExport::json(&bundle)).await?,
        "zip" => blocking_export(slot, move || PreparedExport::zip(&bundle)).await?,
        _ => unreachable!("format was validated before export"),
    };
    stream_response(prepared, slot).await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use axum::body::Bytes;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::{
        blocking_export, stream_response, ExportTestGate, PreparedExport, EXPORT_TEST_GATE,
    };
    use crate::api::test_helpers::{
        json_post, parse_json, seed_project, setup_membership_test, test_app,
    };
    use crate::error::LificError;

    #[tokio::test]
    async fn export_issue_returns_markdown_attachment() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;
        let created = parse_json(
            json_post(
                &app,
                "/api/issues",
                serde_json::json!({
                    "project_id": project_id,
                    "title": "Export me",
                    "description": "Body"
                }),
            )
            .await,
        )
        .await;

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/export/issues/{}",
                        created["identifier"].as_str().unwrap()
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()[axum::http::header::CONTENT_TYPE],
            "text/markdown; charset=utf-8"
        );
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("identifier: TST-1"));
        assert!(body.contains("# Export me"));
    }

    /// LIF-341: the CLI's HTTP backend asks for the bundle rather than the
    /// rendered file, because the bundle carries the path the file belongs
    /// at. Without it a remote export could only drop a bare basename into
    /// the output directory while a local one nested it under the project.
    #[tokio::test]
    async fn export_issue_returns_the_whole_bundle_as_json() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;
        let created = parse_json(
            json_post(
                &app,
                "/api/issues",
                serde_json::json!({
                    "project_id": project_id,
                    "title": "Export me",
                    "description": "Body"
                }),
            )
            .await,
        )
        .await;

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/export/issues/{}?format=json",
                        created["identifier"].as_str().unwrap()
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bundle = parse_json(resp).await;
        assert_eq!(bundle["root"], "TST");
        assert_eq!(bundle["files"][0]["path"], "TST/issues/tst-1-export-me.md");
        assert!(bundle["files"][0]["content"]
            .as_str()
            .unwrap()
            .contains("# Export me"));
    }

    #[tokio::test]
    async fn export_page_returns_the_whole_bundle_as_json() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;
        let created = parse_json(
            json_post(
                &app,
                "/api/pages",
                serde_json::json!({"project_id": project_id, "title": "Bundle page"}),
            )
            .await,
        )
        .await;

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/export/pages/{}?format=json",
                        created["identifier"].as_str().unwrap()
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bundle = parse_json(resp).await;
        assert_eq!(
            bundle["files"][0]["path"],
            "TST/pages/tst-doc-1-bundle-page.md"
        );
    }

    #[tokio::test]
    async fn export_rejects_a_format_it_does_not_render() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;
        let created = parse_json(
            json_post(
                &app,
                "/api/issues",
                serde_json::json!({"project_id": project_id, "title": "Export me"}),
            )
            .await,
        )
        .await;

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/export/issues/{}?format=pdf",
                        created["identifier"].as_str().unwrap()
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn export_project_returns_zip_attachment() {
        let app = test_app();
        let (project_id, project) = seed_project(&app).await;
        json_post(
            &app,
            "/api/issues",
            serde_json::json!({
                "project_id": project_id,
                "title": "Export project"
            }),
        )
        .await;

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/export/projects/{}",
                        project["identifier"].as_str().unwrap()
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()[axum::http::header::CONTENT_TYPE],
            "application/zip"
        );
        assert!(resp
            .headers()
            .get(axum::http::header::CONTENT_LENGTH)
            .is_none());
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert!(!body.is_empty());
        assert_eq!(&body[..2], b"PK");
    }

    #[tokio::test]
    async fn stream_error_follows_the_queued_chunk() {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let (terminal_sender, terminal_receiver) = tokio::sync::oneshot::channel();
        sender.send(Bytes::from_static(b"chunk")).await.unwrap();
        terminal_sender
            .send(Err(std::io::Error::other("read failed")))
            .unwrap();
        drop(sender);

        let error = super::stream_body(receiver, terminal_receiver)
            .collect()
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "read failed");
    }

    #[tokio::test]
    async fn export_slots_are_held_until_response_bodies_are_dropped() {
        let app = test_app();
        let (project_id, project) = seed_project(&app).await;
        json_post(
            &app,
            "/api/issues",
            serde_json::json!({
                "project_id": project_id,
                "title": "Large export",
                "description": "x".repeat(3 * super::EXPORT_STREAM_CHUNK_BYTES)
            }),
        )
        .await;
        let identifier = project["identifier"].as_str().unwrap();
        let request = || {
            Request::builder()
                .uri(format!("/api/export/projects/{identifier}?format=json"))
                .body(axum::body::Body::empty())
                .unwrap()
        };

        let first = app.clone().oneshot(request()).await.unwrap();
        let second = app.clone().oneshot(request()).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(second.status(), StatusCode::OK);

        let blocked = app.clone().oneshot(request()).await.unwrap();
        assert_eq!(blocked.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(blocked.headers()[axum::http::header::RETRY_AFTER], "30");

        drop(first);
        drop(second);
        let retried = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let response = app.clone().oneshot(request()).await.unwrap();
                if response.status() == StatusCode::OK {
                    break response;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("dropped responses should release their export slots");
        assert_eq!(retried.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn denied_exports_do_not_compete_for_slots() {
        let (db, _, _, _, _, non_member, _) = setup_membership_test();
        let _first = db.acquire_export_slot().unwrap();
        let _second = db.acquire_export_slot().unwrap();
        let identity = crate::resolve_caller::ResolvedIdentity {
            user: crate::db::models::AuthUser {
                id: non_member.id,
                username: non_member.username,
                display_name: non_member.display_name,
                is_admin: non_member.is_admin,
            },
            transport: crate::actor::Transport::Web,
        };

        let result = super::export_project(
            axum::extract::State(db),
            axum::Extension(Some(identity)),
            axum::extract::Path("MEM".into()),
            axum::extract::Query(super::ExportQuery { format: None }),
        )
        .await;

        assert!(matches!(result, Err(LificError::Forbidden(_))));
    }

    #[tokio::test]
    async fn issue_authorization_does_not_materialize_metadata_before_capacity() {
        let (db, _, _, _, viewer, _, project_id) = setup_membership_test();
        let issue = {
            let conn = db.write().unwrap();
            let issue = crate::db::queries::create_issue(
                &conn,
                &crate::db::models::CreateIssue {
                    project_id,
                    title: "Bound authorization".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            conn.execute(
                "INSERT INTO labels (project_id, name) VALUES (?1, X'80')",
                [project_id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO issue_labels (issue_id, label_id) VALUES (?1, last_insert_rowid())",
                [issue.id],
            )
            .unwrap();
            issue
        };
        let _first = db.acquire_export_slot().unwrap();
        let _second = db.acquire_export_slot().unwrap();

        let result = super::export_issue(
            axum::extract::State(db),
            axum::Extension(Some(crate::resolve_caller::ResolvedIdentity {
                user: crate::db::models::AuthUser {
                    id: viewer.id,
                    username: viewer.username,
                    display_name: viewer.display_name,
                    is_admin: viewer.is_admin,
                },
                transport: crate::actor::Transport::Web,
            })),
            axum::extract::Path(issue.identifier),
            axum::extract::Query(super::ExportQuery { format: None }),
        )
        .await;

        assert!(matches!(result, Err(LificError::TooManyRequests(_))));
    }

    #[tokio::test]
    async fn page_authorization_does_not_materialize_metadata_before_capacity() {
        let (db, _, _, _, viewer, _, project_id) = setup_membership_test();
        let page = {
            let conn = db.write().unwrap();
            let page = crate::db::queries::create_page(
                &conn,
                &crate::db::models::CreatePage {
                    project_id: Some(project_id),
                    title: "Bound authorization".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            conn.execute(
                "INSERT INTO labels (project_id, name) VALUES (?1, X'80')",
                [project_id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO page_labels (page_id, label_id) VALUES (?1, last_insert_rowid())",
                [page.id],
            )
            .unwrap();
            page
        };
        let _first = db.acquire_export_slot().unwrap();
        let _second = db.acquire_export_slot().unwrap();

        let result = super::export_page(
            axum::extract::State(db),
            axum::Extension(Some(crate::resolve_caller::ResolvedIdentity {
                user: crate::db::models::AuthUser {
                    id: viewer.id,
                    username: viewer.username,
                    display_name: viewer.display_name,
                    is_admin: viewer.is_admin,
                },
                transport: crate::actor::Transport::Web,
            })),
            axum::extract::Path(page.identifier),
            axum::extract::Query(super::ExportQuery { format: None }),
        )
        .await;

        assert!(matches!(result, Err(LificError::TooManyRequests(_))));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn project_export_leaves_the_runtime_responsive() {
        let app = test_app();
        let (_, project) = seed_project(&app).await;
        let identifier = project["identifier"].as_str().unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let gate = Arc::new(ExportTestGate::new(started_tx, release_rx));
        let export = app.clone().oneshot(
            Request::builder()
                .uri(format!("/api/export/projects/{identifier}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        );
        let health = async {
            started_rx.await.unwrap();
            let response = app
                .oneshot(
                    Request::builder()
                        .uri("/api/health")
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            release_tx.send(()).unwrap();
        };

        let (response, ()) = tokio::time::timeout(
            Duration::from_secs(1),
            EXPORT_TEST_GATE.scope(gate, async { tokio::join!(export, health) }),
        )
        .await
        .expect("export worker blocked the async runtime");
        assert_eq!(response.unwrap().status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn cancelled_request_keeps_its_slot_until_blocking_work_stops() {
        let semaphore = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let request = tokio::spawn(blocking_export(permit, move || {
            let _ = started_tx.send(());
            release_rx.recv().unwrap();
            Ok(())
        }));

        started_rx.await.unwrap();
        request.abort();
        assert!(semaphore.clone().try_acquire_owned().is_err());

        release_tx.send(()).unwrap();
        let _permit = tokio::time::timeout(Duration::from_secs(1), semaphore.acquire_owned())
            .await
            .expect("blocking worker should release its slot")
            .unwrap();
    }

    #[tokio::test]
    async fn dropping_a_response_closes_its_file_before_temp_cleanup() {
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path().to_path_buf();
        let path = temp_path.join("export.md");
        std::fs::write(&path, "body").unwrap();
        let semaphore = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let response = stream_response(
            PreparedExport {
                temp_dir,
                path,
                content_type: axum::http::HeaderValue::from_static("text/markdown"),
                download_name: None,
            },
            permit,
        )
        .await
        .unwrap();

        drop(response);
        let _permit = tokio::time::timeout(Duration::from_secs(1), semaphore.acquire_owned())
            .await
            .expect("dropping the response should release its export slot")
            .unwrap();
        assert!(!temp_path.exists());
    }

    #[tokio::test]
    async fn stalled_response_releases_its_slot_and_temp_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path().to_path_buf();
        let path = temp_path.join("export.md");
        std::fs::write(&path, vec![0; 256 * 1024]).unwrap();
        let semaphore = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let response = super::stream_response_with_timeouts(
            PreparedExport {
                temp_dir,
                path,
                content_type: axum::http::HeaderValue::from_static("text/markdown"),
                download_name: None,
            },
            permit,
            Duration::from_millis(20),
            Duration::from_secs(1),
        )
        .await
        .unwrap();

        let _permit = tokio::time::timeout(Duration::from_secs(1), semaphore.acquire_owned())
            .await
            .expect("stalled response should release its export slot")
            .unwrap();
        assert!(!temp_path.exists());
        assert!(response.into_body().collect().await.is_err());
    }

    #[tokio::test]
    async fn trickle_response_cannot_hold_an_export_slot_indefinitely() {
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path().to_path_buf();
        let path = temp_path.join("export.md");
        std::fs::write(&path, vec![0; 256 * 1024]).unwrap();
        let semaphore = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let response = super::stream_response_with_timeouts(
            PreparedExport {
                temp_dir,
                path,
                content_type: axum::http::HeaderValue::from_static("text/markdown"),
                download_name: None,
            },
            permit,
            Duration::from_secs(5),
            Duration::from_millis(200),
        )
        .await
        .unwrap();

        let mut body = response.into_body();
        assert!(body.frame().await.unwrap().unwrap().is_data());
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(body.frame().await.unwrap().unwrap().is_data());

        let _permit = tokio::time::timeout(Duration::from_secs(1), semaphore.acquire_owned())
            .await
            .expect("stream deadline should release the export slot")
            .unwrap();
        assert!(!temp_path.exists());
        assert!(body.collect().await.is_err());
    }
}
