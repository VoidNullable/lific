use axum::extract::{Json, Path, Query, State};

use crate::db::{DbPool, models::*};
use crate::error::LificError;

use super::{with_read, with_write};

#[derive(serde::Deserialize)]
pub(super) struct PageQuery {
    project_id: Option<i64>,
    folder_id: Option<i64>,
    /// LIF-105: filter pages by label name. Mirrors `?label=` on the
    /// issue list endpoint.
    label: Option<String>,
}

pub(super) async fn list_pages_handler(
    State(db): State<DbPool>,
    Query(q): Query<PageQuery>,
) -> Result<Json<Vec<Page>>, LificError> {
    with_read(&db, |conn| {
        crate::db::queries::list_pages(conn, q.project_id, q.folder_id, q.label.as_deref())
    })
    .map(Json)
}

pub(super) async fn get_page(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
) -> Result<Json<Page>, LificError> {
    with_read(&db, |conn| crate::db::queries::get_page(conn, id)).map(Json)
}

pub(super) async fn create_page(
    State(db): State<DbPool>,
    Json(input): Json<CreatePage>,
) -> Result<Json<Page>, LificError> {
    with_write(&db, |conn| crate::db::queries::create_page(conn, &input)).map(Json)
}

pub(super) async fn update_page(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Json(input): Json<UpdatePage>,
) -> Result<Json<Page>, LificError> {
    with_write(&db, |conn| {
        crate::db::queries::update_page(conn, id, &input)
    })
    .map(Json)
}

pub(super) async fn delete_page_handler(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, LificError> {
    with_write(&db, |conn| crate::db::queries::delete_page(conn, id))?;
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
}
