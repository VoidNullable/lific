use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::IntoResponse;

use crate::authz;
use crate::db::DbPool;
use crate::db::models::Role;
use crate::error::LificError;

use super::with_read;

/// `?format=json` returns the raw [`crate::export::ExportBundle`] instead of
/// a file body. The CLI's HTTP backend uses it for single-resource exports so
/// it can write the bundle through the same writer the SQL backend uses, and
/// so land the file at the same nested path rather than a bare basename
/// (LIF-341). Project exports keep `zip` as their default.
#[derive(serde::Deserialize)]
pub(super) struct ExportQuery {
    pub format: Option<String>,
}

/// A single-resource export renders one file; `format=json` hands back the
/// whole bundle so the caller knows the path it belongs at.
fn single_file_response(
    bundle: crate::export::ExportBundle,
    format: Option<&str>,
    fallback_name: &str,
) -> Result<axum::response::Response, LificError> {
    match format.unwrap_or("markdown") {
        "json" => Ok(axum::Json(bundle).into_response()),
        "markdown" => {
            let file = bundle.files.into_iter().next().ok_or_else(|| {
                LificError::Internal(format!("export produced no files for {fallback_name}"))
            })?;
            let filename = file.path.rsplit('/').next().unwrap_or(fallback_name);
            let mut headers = HeaderMap::new();
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/markdown; charset=utf-8"),
            );
            headers.insert(header::CONTENT_DISPOSITION, content_disposition(filename)?);
            Ok((headers, file.content).into_response())
        }
        _ => Err(LificError::BadRequest(
            "invalid export format. Expected 'markdown' or 'json'".into(),
        )),
    }
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
    let project_id = with_read(&db, |conn| {
        let id = crate::db::queries::resolve_identifier(conn, &identifier)?;
        Ok(crate::db::queries::get_issue(conn, id)?.project_id)
    })?;
    authz::require_role(&db, &identity, project_id, Role::Viewer)?;
    let bundle = with_read(&db, |conn| crate::export::export_issue(conn, &identifier))?;
    single_file_response(bundle, q.format.as_deref(), "issue.md")
}

pub(super) async fn export_page(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(identifier): Path<String>,
    Query(q): Query<ExportQuery>,
) -> Result<impl IntoResponse, LificError> {
    let project_id = with_read(&db, |conn| {
        let id = crate::db::queries::resolve_page_identifier(conn, &identifier)?;
        Ok(crate::db::queries::get_page(conn, id)?.project_id)
    })?;
    match project_id {
        Some(pid) => authz::require_role(&db, &identity, pid, Role::Viewer)?,
        None => authz::require_workspace_admin(&db, &identity)?,
    }
    let bundle = with_read(&db, |conn| crate::export::export_page(conn, &identifier))?;
    single_file_response(bundle, q.format.as_deref(), "page.md")
}

pub(super) async fn export_project(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(identifier): Path<String>,
    Query(q): Query<ExportQuery>,
) -> Result<impl IntoResponse, LificError> {
    let project_id = with_read(&db, |conn| {
        crate::db::queries::resolve_project_identifier(conn, &identifier)
    })?;
    authz::require_role(&db, &identity, project_id, Role::Viewer)?;
    let format = q.format.as_deref().unwrap_or("zip");
    let bundle = with_read(&db, |conn| crate::export::export_project(conn, &identifier))?;

    match format {
        "json" => Ok(axum::Json(bundle).into_response()),
        "zip" => {
            let filename = format!("{}-export.zip", bundle.root.to_ascii_lowercase());
            let bytes = crate::export::bundle_to_zip(&bundle)?;
            let mut headers = HeaderMap::new();
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/zip"),
            );
            headers.insert(header::CONTENT_DISPOSITION, content_disposition(&filename)?);
            Ok((headers, bytes).into_response())
        }
        _ => Err(LificError::BadRequest(
            "invalid export format. Expected 'zip' or 'json'".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::api::test_helpers::{json_post, parse_json, seed_project, test_app};

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
        assert!(
            bundle["files"][0]["content"]
                .as_str()
                .unwrap()
                .contains("# Export me")
        );
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
    }
}
