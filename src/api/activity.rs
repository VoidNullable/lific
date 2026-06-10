//! LIF-156: REST read surface for the audit log (captured in migration
//! 018's triggers — see queries::activity for the read layer).

use axum::extract::{Json, Path, Query, State};

use crate::db::queries::activity::{ActivityScope, actor_stats, list_activity};
use crate::db::{
    DbPool,
    models::{ActivityFeed, ActorStat},
};
use crate::error::LificError;

use super::with_read;

#[derive(Debug, serde::Deserialize)]
pub(super) struct ActivityQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// GET /api/issues/{id}/activity — the issue's own edits plus its
/// comments, label attach/detach, and relation link/unlink events.
pub(super) async fn issue_activity(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Query(q): Query<ActivityQuery>,
) -> Result<Json<ActivityFeed>, LificError> {
    with_read(&db, |conn| {
        list_activity(conn, ActivityScope::Issue(id), q.limit, q.offset)
    })
    .map(Json)
}

/// GET /api/pages/{id}/activity
pub(super) async fn page_activity(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Query(q): Query<ActivityQuery>,
) -> Result<Json<ActivityFeed>, LificError> {
    with_read(&db, |conn| {
        list_activity(conn, ActivityScope::Page(id), q.limit, q.offset)
    })
    .map(Json)
}

/// GET /api/projects/{id}/activity — everything in the project, newest
/// first: issues, pages, comments, modules, labels, folders.
pub(super) async fn project_activity(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Query(q): Query<ActivityQuery>,
) -> Result<Json<ActivityFeed>, LificError> {
    with_read(&db, |conn| {
        list_activity(conn, ActivityScope::Project(id), q.limit, q.offset)
    })
    .map(Json)
}

/// GET /api/projects/{id}/activity/actors — per-actor rollup, most
/// active first (LIF-158: actor rail + expanded-entry stats).
pub(super) async fn project_activity_actors(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<ActorStat>>, LificError> {
    with_read(&db, |conn| actor_stats(conn, id)).map(Json)
}

#[cfg(test)]
mod tests {
    use crate::api::test_helpers::*;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn get_json(app: &axum::Router, uri: &str) -> serde_json::Value {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn issue_activity_records_authenticated_web_actor() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;

        let issue = parse_json(
            json_post(
                &app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id, "title": "Watch me" }),
            )
            .await,
        )
        .await;
        let issue_id = issue["id"].as_i64().unwrap();

        // Mutate via the API: status + priority.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/issues/{issue_id}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_vec(&serde_json::json!({
                            "status": "active", "priority": "high"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        let feed = get_json(&app, &format!("/api/issues/{issue_id}/activity")).await;
        let items = feed["items"].as_array().unwrap();

        let status_row = items
            .iter()
            .find(|a| a["field"] == "status")
            .expect("status change audited");
        assert_eq!(status_row["old_value"], "backlog");
        assert_eq!(status_row["new_value"], "active");
        assert_eq!(status_row["action"], "update");

        assert!(items.iter().any(|a| a["field"] == "priority"));
        assert!(items.iter().any(|a| a["action"] == "create"));
        assert_eq!(feed["has_more"], false);
    }

    #[tokio::test]
    async fn project_activity_pages_with_has_more() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;

        for i in 0..5 {
            json_post(
                &app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id, "title": format!("i{i}") }),
            )
            .await;
        }

        let feed = get_json(
            &app,
            &format!("/api/projects/{project_id}/activity?limit=3"),
        )
        .await;
        assert_eq!(feed["items"].as_array().unwrap().len(), 3);
        assert_eq!(feed["has_more"], true);

        let rest = get_json(
            &app,
            &format!("/api/projects/{project_id}/activity?limit=50&offset=3"),
        )
        .await;
        // 6 total (project create + 5 issues) minus the 3 already seen.
        assert_eq!(rest["items"].as_array().unwrap().len(), 3);
        assert_eq!(rest["has_more"], false);
    }

    #[tokio::test]
    async fn page_activity_tracks_content_edits() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;

        let page = parse_json(
            json_post(
                &app,
                "/api/pages",
                serde_json::json!({ "project_id": project_id, "title": "Doc" }),
            )
            .await,
        )
        .await;
        let page_id = page["id"].as_i64().unwrap();

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/pages/{page_id}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_vec(&serde_json::json!({ "content": "# hello" })).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        let feed = get_json(&app, &format!("/api/pages/{page_id}/activity")).await;
        let items = feed["items"].as_array().unwrap();
        let edit = items
            .iter()
            .find(|a| a["field"] == "content")
            .expect("content edit audited");
        assert_eq!(edit["new_value"], "# hello");
        assert_eq!(edit["entity_label"], "TST-DOC-1");
    }

    #[tokio::test]
    async fn actors_endpoint_returns_rollup() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;
        for i in 0..3 {
            json_post(
                &app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id, "title": format!("i{i}") }),
            )
            .await;
        }

        let actors = get_json(&app, &format!("/api/projects/{project_id}/activity/actors")).await;
        let arr = actors.as_array().unwrap();
        assert_eq!(arr.len(), 1, "test app writes are unscoped → one bucket");
        assert!(arr[0]["actions"].as_i64().unwrap() >= 4); // project + 3 issues
        assert!(arr[0]["last_ts"].is_string());
        assert_eq!(arr[0]["top_transport"], "system");
    }
}
