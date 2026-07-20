use axum::{
    Extension,
    extract::{Json, Path, Query, State},
};

use crate::authz;
use crate::db::{DbPool, models::*};
use crate::error::LificError;
use crate::realtime::{RealtimeEvent, RealtimeHub};

use super::{filter_visible, with_read, with_write};

/// Gate a page mutation/read by its `project_id`: project-scoped pages check
/// `min` role on the project; workspace-level pages (`project_id = None`,
/// the only entity besides itself that can be project-less — plans always
/// require a project) fall back to admin-only once enforcement is on
/// (design decision #10).
fn require_page_role(
    db: &DbPool,
    auth_user: &Option<AuthUser>,
    project_id: Option<i64>,
    min: Role,
) -> Result<(), LificError> {
    match project_id {
        Some(pid) => authz::require_role(db, auth_user, pid, min),
        None => authz::require_workspace_admin(db, auth_user),
    }
}

#[derive(serde::Deserialize)]
pub(super) struct PageQuery {
    project_id: Option<i64>,
    folder_id: Option<i64>,
    /// LIF-105: filter pages by label name. Mirrors `?label=` on the
    /// issue list endpoint.
    label: Option<String>,
    /// LIF-112: filter pages by lifecycle status. Mirrors `?status=` on
    /// the issue list endpoint.
    status: Option<String>,
    /// Sort column: sort_order (default), title, status, created, updated.
    /// Whitelisted in `list_pages`.
    order_by: Option<String>,
    /// Sort direction: asc (default) or desc.
    order: Option<String>,
    /// Maximum number of pages to return (clamped by `list_pages`).
    limit: Option<i64>,
    /// Number of matching pages to skip before returning results.
    offset: Option<i64>,
}

pub(super) async fn list_pages_handler(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Query(q): Query<PageQuery>,
) -> Result<Json<Vec<Page>>, LificError> {
    if let Some(pid) = q.project_id {
        authz::require_role(&db, &auth_user, pid, Role::Viewer)?;
        return with_read(&db, |conn| {
            crate::db::queries::list_pages(
                conn,
                q.project_id,
                q.folder_id,
                q.label.as_deref(),
                q.status.as_deref(),
                q.order_by.as_deref(),
                q.order.as_deref(),
                q.limit,
                q.offset,
            )
        })
        .map(Json);
    }
    // Cross-project list (LIF-197 scope item 2): filter, don't deny. A
    // workspace page (project_id None) is excluded for any non-admin once
    // enforcement is on — see `filter_visible`'s doc comment.
    let visible = authz::visible_project_ids(&db, &auth_user)?;
    let pages = with_read(&db, |conn| {
        crate::db::queries::list_pages(
            conn,
            q.project_id,
            q.folder_id,
            q.label.as_deref(),
            q.status.as_deref(),
            q.order_by.as_deref(),
            q.order.as_deref(),
            q.limit,
            q.offset,
        )
    })?;
    Ok(Json(filter_visible(pages, &visible, |p| p.project_id)))
}

pub(super) async fn get_page(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path(id): Path<i64>,
) -> Result<Json<Page>, LificError> {
    let page = with_read(&db, |conn| crate::db::queries::get_page(conn, id))?;
    require_page_role(&db, &auth_user, page.project_id, Role::Viewer)?;
    Ok(Json(page))
}

pub(super) async fn resolve_page(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path(identifier): Path<String>,
) -> Result<Json<Page>, LificError> {
    let page = with_read(&db, |conn| {
        let id = crate::db::queries::resolve_page_identifier(conn, &identifier)?;
        crate::db::queries::get_page(conn, id)
    })?;
    require_page_role(&db, &auth_user, page.project_id, Role::Viewer)?;
    Ok(Json(page))
}

pub(super) async fn create_page(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<CreatePage>,
) -> Result<Json<Page>, LificError> {
    require_page_role(&db, &auth_user, input.project_id, Role::Maintainer)?;
    let page = with_write(&db, |conn| {
        let page = crate::db::queries::create_page(conn, &input)?;
        // LIF-262: link any attachments the content references.
        super::attachments::sync_links(conn, AttachmentEntity::Page, page.id, &page.content)?;
        Ok(page)
    })?;
    if let Some(project_id) = page.project_id {
        realtime.send(RealtimeEvent::ProjectUpdated { project_id });
    }
    Ok(Json(page))
}

pub(super) async fn update_page(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path(id): Path<i64>,
    Json(input): Json<UpdatePage>,
) -> Result<Json<Page>, LificError> {
    let project_id = with_read(&db, |conn| crate::db::queries::get_page(conn, id))?.project_id;
    require_page_role(&db, &auth_user, project_id, Role::Maintainer)?;
    let page = with_write(&db, |conn| {
        let page = crate::db::queries::update_page(conn, id, &input)?;
        // LIF-262: re-scan the (possibly edited) content and reconcile links.
        super::attachments::sync_links(conn, AttachmentEntity::Page, page.id, &page.content)?;
        Ok(page)
    })?;
    if let Some(project_id) = page.project_id {
        realtime.send(RealtimeEvent::ProjectUpdated { project_id });
    }
    Ok(Json(page))
}

pub(super) async fn delete_page_handler(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, LificError> {
    let project_id = with_read(&db, |conn| crate::db::queries::get_page(conn, id))?.project_id;
    require_page_role(&db, &auth_user, project_id, Role::Maintainer)?;
    with_write(&db, |conn| crate::db::queries::delete_page(conn, id))?;
    if let Some(project_id) = project_id {
        realtime.send(RealtimeEvent::ProjectUpdated { project_id });
    }
    Ok(Json(serde_json::json!({"deleted": true})))
}

#[cfg(test)]
mod tests {
    use crate::api::test_helpers::*;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// Seed a page-friendly project plus two labels, return (project_id).
    async fn seed_project_with_labels(app: &axum::Router) -> i64 {
        let (project_id, _) = seed_project(app).await;
        for (name, color) in [("design", "#22C55E"), ("draft", "#F59E0B")] {
            json_post(
                app,
                "/api/labels",
                serde_json::json!({
                    "project_id": project_id,
                    "name": name,
                    "color": color,
                }),
            )
            .await;
        }
        project_id
    }

    #[tokio::test]
    async fn create_page_accepts_labels_and_returns_them() {
        let app = test_app();
        let pid = seed_project_with_labels(&app).await;

        let resp = json_post(
            &app,
            "/api/pages",
            serde_json::json!({
                "project_id": pid,
                "title": "Spec",
                "labels": ["design"],
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let page = parse_json(resp).await;
        assert_eq!(page["labels"], serde_json::json!(["design"]));
    }

    #[tokio::test]
    async fn update_page_replaces_labels() {
        // PUT /api/pages/{id} with labels = [...] should replace the
        // attached set wholesale (delete-all + insert-by-name), matching
        // the `update_issue` behavior the frontend already relies on.
        let app = test_app();
        let pid = seed_project_with_labels(&app).await;

        let created = parse_json(
            json_post(
                &app,
                "/api/pages",
                serde_json::json!({
                    "project_id": pid,
                    "title": "Spec",
                    "labels": ["design"],
                }),
            )
            .await,
        )
        .await;
        let id = created["id"].as_i64().unwrap();

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/pages/{id}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_vec(&serde_json::json!({ "labels": ["draft"] })).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let updated = parse_json(resp).await;
        assert_eq!(updated["labels"], serde_json::json!(["draft"]));
    }

    #[tokio::test]
    async fn list_pages_supports_label_filter() {
        let app = test_app();
        let pid = seed_project_with_labels(&app).await;

        json_post(
            &app,
            "/api/pages",
            serde_json::json!({
                "project_id": pid,
                "title": "Designy",
                "labels": ["design"],
            }),
        )
        .await;
        json_post(
            &app,
            "/api/pages",
            serde_json::json!({
                "project_id": pid,
                "title": "Plain",
            }),
        )
        .await;

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/pages?project_id={pid}&label=design"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let list: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["title"], "Designy");
    }

    #[tokio::test]
    async fn list_pages_supports_status_filter() {
        // LIF-112: mirrors the issues status-filter test. Create one
        // draft (default) and one archived page, then verify ?status=
        // narrows the list.
        let app = test_app();
        let (pid, _) = seed_project(&app).await;

        json_post(
            &app,
            "/api/pages",
            serde_json::json!({
                "project_id": pid,
                "title": "Drafty",
            }),
        )
        .await;
        json_post(
            &app,
            "/api/pages",
            serde_json::json!({
                "project_id": pid,
                "title": "Archived doc",
                "status": "archived",
            }),
        )
        .await;

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/pages?project_id={pid}&status=archived"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let list: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["title"], "Archived doc");
        assert_eq!(list[0]["status"], "archived");
    }

    #[tokio::test]
    async fn list_pages_orders_and_paginates_results() {
        let app = test_app();
        let (pid, _) = seed_project(&app).await;

        for title in ["Delta", "Alpha", "Charlie", "Bravo"] {
            json_post(
                &app,
                "/api/pages",
                serde_json::json!({
                    "project_id": pid,
                    "title": title,
                }),
            )
            .await;
        }

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/pages?project_id={pid}&order_by=title&order=asc&limit=2&offset=1"
                    ))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let list: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
        let titles: Vec<&str> = list
            .iter()
            .map(|page| page["title"].as_str().unwrap())
            .collect();
        assert_eq!(titles, ["Bravo", "Charlie"]);
    }

    #[tokio::test]
    async fn create_page_defaults_status_to_draft() {
        let app = test_app();
        let (pid, _) = seed_project(&app).await;

        let resp = json_post(
            &app,
            "/api/pages",
            serde_json::json!({
                "project_id": pid,
                "title": "Fresh",
            }),
        )
        .await;
        let page = parse_json(resp).await;
        assert_eq!(page["status"], "draft");
    }

    #[tokio::test]
    async fn get_page_includes_labels() {
        let app = test_app();
        let pid = seed_project_with_labels(&app).await;

        let created = parse_json(
            json_post(
                &app,
                "/api/pages",
                serde_json::json!({
                    "project_id": pid,
                    "title": "Spec",
                    "labels": ["design", "draft"],
                }),
            )
            .await,
        )
        .await;
        let id = created["id"].as_i64().unwrap();

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/pages/{id}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let page = parse_json(resp).await;
        let labels = page["labels"].as_array().unwrap();
        assert_eq!(labels.len(), 2);
    }

    #[tokio::test]
    async fn resolve_page_by_project_identifier_returns_full_page() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;
        let created = parse_json(
            json_post(
                &app,
                "/api/pages",
                serde_json::json!({ "project_id": project_id, "title": "Project spec" }),
            )
            .await,
        )
        .await;

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/pages/resolve/TST-DOC-1")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let page = parse_json(response).await;
        assert_eq!(page["id"], created["id"]);
        assert_eq!(page["identifier"], "TST-DOC-1");
        assert_eq!(page["title"], "Project spec");
    }

    #[tokio::test]
    async fn resolve_page_by_workspace_identifier_returns_full_page() {
        let app = test_app();
        let created = parse_json(
            json_post(
                &app,
                "/api/pages",
                serde_json::json!({ "title": "Workspace guide" }),
            )
            .await,
        )
        .await;

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/pages/resolve/DOC-1")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let page = parse_json(response).await;
        assert_eq!(page["id"], created["id"]);
        assert_eq!(page["identifier"], "DOC-1");
        assert!(page["project_id"].is_null());
    }

    #[tokio::test]
    async fn resolve_page_returns_not_found_for_unknown_identifier() {
        let app = test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/pages/resolve/TST-DOC-999")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn resolve_page_allows_viewer_and_denies_non_member_when_enforced() {
        let (db, _admin, lead, _maintainer, viewer, non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db.clone(), &lead);
        let page = parse_json(
            json_post(
                &lead_app,
                "/api/pages",
                serde_json::json!({ "project_id": project_id, "title": "Members only" }),
            )
            .await,
        )
        .await;
        let identifier = page["identifier"].as_str().unwrap();

        let viewer_app = app_as_user(db.clone(), &viewer);
        let response = viewer_app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/pages/resolve/{identifier}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let non_member_app = app_as_user(db, &non_member);
        let response = non_member_app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/pages/resolve/{identifier}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
