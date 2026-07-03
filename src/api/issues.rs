use axum::{
    Extension,
    extract::{Json, Path, Query, State},
};

use crate::authz;
use crate::db::{DbPool, models::*};
use crate::error::LificError;

use super::{filter_visible, with_read, with_write};

pub(super) async fn list_issues(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Query(q): Query<ListIssuesQuery>,
) -> Result<Json<Vec<Issue>>, LificError> {
    if let Some(pid) = q.project_id {
        authz::require_role(&db, &auth_user, pid, Role::Viewer)?;
        return with_read(&db, |conn| crate::db::queries::list_issues(conn, &q)).map(Json);
    }
    // Cross-project list: filter instead of denying (LIF-197 scope item 2).
    let visible = authz::visible_project_ids(&db, &auth_user)?;
    let issues = with_read(&db, |conn| crate::db::queries::list_issues(conn, &q))?;
    Ok(Json(filter_visible(issues, &visible, |i| Some(i.project_id))))
}

pub(super) async fn get_issue(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path(id): Path<i64>,
) -> Result<Json<Issue>, LificError> {
    let issue = with_read(&db, |conn| crate::db::queries::get_issue(conn, id))?;
    authz::require_role(&db, &auth_user, issue.project_id, Role::Viewer)?;
    Ok(Json(issue))
}

pub(super) async fn resolve_issue(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path(identifier): Path<String>,
) -> Result<Json<Issue>, LificError> {
    let issue = with_read(&db, |conn| {
        let id = crate::db::queries::resolve_identifier(conn, &identifier)?;
        crate::db::queries::get_issue(conn, id)
    })?;
    authz::require_role(&db, &auth_user, issue.project_id, Role::Viewer)?;
    Ok(Json(issue))
}

pub(super) async fn create_issue(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<CreateIssue>,
) -> Result<Json<Issue>, LificError> {
    authz::require_role(&db, &auth_user, input.project_id, Role::Maintainer)?;
    let issue = with_write(&db, |conn| {
        let issue = crate::db::queries::create_issue(conn, &input)?;
        // LIF-262: link any attachments the description references.
        super::attachments::sync_links(
            conn,
            AttachmentEntity::Issue,
            issue.id,
            &issue.description,
        )?;
        Ok(issue)
    })?;
    Ok(Json(issue))
}

pub(super) async fn update_issue(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path(id): Path<i64>,
    Json(input): Json<UpdateIssue>,
) -> Result<Json<Issue>, LificError> {
    let project_id = with_read(&db, |conn| crate::db::queries::get_issue(conn, id))?.project_id;
    authz::require_role(&db, &auth_user, project_id, Role::Maintainer)?;
    let issue = with_write(&db, |conn| {
        let issue = crate::db::queries::update_issue(conn, id, &input)?;
        // LIF-262: re-scan the (possibly edited) description and reconcile links.
        super::attachments::sync_links(
            conn,
            AttachmentEntity::Issue,
            issue.id,
            &issue.description,
        )?;
        Ok(issue)
    })?;
    Ok(Json(issue))
}

pub(super) async fn delete_issue_handler(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, LificError> {
    let project_id = with_read(&db, |conn| crate::db::queries::get_issue(conn, id))?.project_id;
    authz::require_role(&db, &auth_user, project_id, Role::Maintainer)?;
    with_write(&db, |conn| crate::db::queries::delete_issue(conn, id))?;
    Ok(Json(serde_json::json!({"deleted": true})))
}

#[derive(serde::Deserialize)]
pub(super) struct LinkRequest {
    source: String,
    target: String,
    relation_type: String,
}

#[derive(serde::Deserialize)]
pub(super) struct UnlinkRequest {
    source: String,
    target: String,
}

pub(super) async fn link_issues(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<LinkRequest>,
) -> Result<Json<serde_json::Value>, LificError> {
    let (source_id, target_id) = with_read(&db, |conn| {
        let source_id = crate::db::queries::resolve_identifier(conn, &input.source)?;
        let target_id = crate::db::queries::resolve_identifier(conn, &input.target)?;
        Ok((source_id, target_id))
    })?;
    // Cross-project relation: the caller must be a Maintainer on BOTH sides
    // (LIF-197 scope item 3), even when source and target share a project.
    let source_project = with_read(&db, |conn| crate::db::queries::get_issue(conn, source_id))?
        .project_id;
    let target_project = with_read(&db, |conn| crate::db::queries::get_issue(conn, target_id))?
        .project_id;
    authz::require_role(&db, &auth_user, source_project, Role::Maintainer)?;
    authz::require_role(&db, &auth_user, target_project, Role::Maintainer)?;

    with_write(&db, |conn| {
        crate::db::queries::link_issues(conn, source_id, target_id, &input.relation_type)
    })?;
    Ok(Json(serde_json::json!({"linked": true})))
}

pub(super) async fn unlink_issues(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<UnlinkRequest>,
) -> Result<Json<serde_json::Value>, LificError> {
    let (source_id, target_id) = with_read(&db, |conn| {
        let source_id = crate::db::queries::resolve_identifier(conn, &input.source)?;
        let target_id = crate::db::queries::resolve_identifier(conn, &input.target)?;
        Ok((source_id, target_id))
    })?;
    let source_project = with_read(&db, |conn| crate::db::queries::get_issue(conn, source_id))?
        .project_id;
    let target_project = with_read(&db, |conn| crate::db::queries::get_issue(conn, target_id))?
        .project_id;
    authz::require_role(&db, &auth_user, source_project, Role::Maintainer)?;
    authz::require_role(&db, &auth_user, target_project, Role::Maintainer)?;

    with_write(&db, |conn| {
        crate::db::queries::unlink_issues(conn, source_id, target_id)
    })?;
    Ok(Json(serde_json::json!({"unlinked": true})))
}

#[cfg(test)]
mod tests {
    use crate::api::test_helpers::*;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn issue_crud_lifecycle() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;

        // Create issue
        let body = serde_json::json!({
            "project_id": project_id,
            "title": "Fix the bug",
            "status": "todo",
            "priority": "high"
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/issues")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let issue: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let issue_id = issue["id"].as_i64().unwrap();
        assert_eq!(issue["identifier"], "TST-1");
        assert_eq!(issue["priority"], "high");

        // List with filter
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/issues?project_id={project_id}&status=todo"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let list: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(list.len(), 1);

        // Update
        let update = serde_json::json!({"status": "active"});
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/issues/{issue_id}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&update).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let updated: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(updated["status"], "active");

        // Resolve by identifier
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/issues/resolve/TST-1")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Delete
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/issues/{issue_id}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
