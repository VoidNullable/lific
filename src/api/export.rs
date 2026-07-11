use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::IntoResponse;

use crate::authz;
use crate::db::DbPool;
use crate::db::models::{AuthUser, Role};
use crate::error::LificError;

use super::with_read;

#[derive(serde::Deserialize)]
pub(super) struct ProjectExportQuery {
    pub format: Option<String>,
}

fn content_disposition(filename: &str) -> Result<HeaderValue, LificError> {
    HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
        .map_err(|e| LificError::Internal(format!("invalid content-disposition header: {e}")))
}

pub(super) async fn export_issue(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path(identifier): Path<String>,
) -> Result<impl IntoResponse, LificError> {
    let project_id = with_read(&db, |conn| {
        let id = crate::db::queries::resolve_identifier(conn, &identifier)?;
        Ok(crate::db::queries::get_issue(conn, id)?.project_id)
    })?;
    authz::require_role(&db, &auth_user, project_id, Role::Viewer)?;
    let bundle = with_read(&db, |conn| crate::export::export_issue(conn, &identifier))?;
    let file = bundle
        .files
        .into_iter()
        .next()
        .ok_or_else(|| LificError::Internal("issue export produced no files".into()))?;
    let filename = file
        .path
        .rsplit('/')
        .next()
        .unwrap_or("issue.md")
        .to_string();

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/markdown; charset=utf-8"),
    );
    headers.insert(header::CONTENT_DISPOSITION, content_disposition(&filename)?);
    Ok((headers, file.content))
}

pub(super) async fn export_page(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path(identifier): Path<String>,
) -> Result<impl IntoResponse, LificError> {
    let project_id = with_read(&db, |conn| {
        let id = crate::db::queries::resolve_page_identifier(conn, &identifier)?;
        Ok(crate::db::queries::get_page(conn, id)?.project_id)
    })?;
    match project_id {
        Some(pid) => authz::require_role(&db, &auth_user, pid, Role::Viewer)?,
        None => authz::require_workspace_admin(&db, &auth_user)?,
    }
    let bundle = with_read(&db, |conn| crate::export::export_page(conn, &identifier))?;
    let file = bundle
        .files
        .into_iter()
        .next()
        .ok_or_else(|| LificError::Internal("page export produced no files".into()))?;
    let filename = file
        .path
        .rsplit('/')
        .next()
        .unwrap_or("page.md")
        .to_string();

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/markdown; charset=utf-8"),
    );
    headers.insert(header::CONTENT_DISPOSITION, content_disposition(&filename)?);
    Ok((headers, file.content))
}

pub(super) async fn export_project(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path(identifier): Path<String>,
    Query(q): Query<ProjectExportQuery>,
) -> Result<impl IntoResponse, LificError> {
    let project_id = with_read(&db, |conn| {
        crate::db::queries::resolve_project_identifier(conn, &identifier)
    })?;
    authz::require_role(&db, &auth_user, project_id, Role::Viewer)?;
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
