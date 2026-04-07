use axum::{
    Extension,
    extract::{Json, Path, Query, State},
};

use crate::db::{DbPool, models::*};
use crate::error::LificError;

use super::{require_admin, require_project_lead, with_read, with_write};

pub(super) async fn list_projects(
    State(db): State<DbPool>,
) -> Result<Json<Vec<Project>>, LificError> {
    with_read(&db, crate::db::queries::list_projects).map(Json)
}

pub(super) async fn get_project(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
) -> Result<Json<Project>, LificError> {
    with_read(&db, |conn| crate::db::queries::get_project(conn, id)).map(Json)
}

pub(super) async fn create_project(
    State(db): State<DbPool>,
    Json(input): Json<CreateProject>,
) -> Result<Json<Project>, LificError> {
    with_write(&db, |conn| crate::db::queries::create_project(conn, &input)).map(Json)
}

pub(super) async fn update_project(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<UpdateProject>,
) -> Result<Json<Project>, LificError> {
    require_project_lead(&db, &auth_user, id)?;
    with_write(&db, |conn| {
        crate::db::queries::update_project(conn, id, &input)
    })
    .map(Json)
}

pub(super) async fn delete_project_handler(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Extension(auth_user): Extension<Option<AuthUser>>,
) -> Result<Json<serde_json::Value>, LificError> {
    require_admin(&auth_user)?;
    with_write(&db, |conn| crate::db::queries::delete_project(conn, id))?;
    Ok(Json(serde_json::json!({"deleted": true})))
}

#[derive(serde::Deserialize)]
pub(super) struct BoardQuery {
    #[serde(default = "default_group_by")]
    group_by: String,
}

fn default_group_by() -> String {
    "status".to_string()
}

pub(super) async fn get_board(
    State(db): State<DbPool>,
    Path(project_id): Path<i64>,
    Query(q): Query<BoardQuery>,
) -> Result<Json<serde_json::Value>, LificError> {
    let issues = with_read(&db, |conn| {
        crate::db::queries::list_issues(
            conn,
            &ListIssuesQuery {
                project_id: Some(project_id),
                status: None,
                priority: None,
                module_id: None,
                label: None,
                workable: None,
                limit: Some(500),
                offset: None,
            },
        )
    })?;

    let module_names: std::collections::HashMap<i64, String> = if q.group_by == "module" {
        with_read(&db, |conn| {
            crate::db::queries::list_modules(conn, project_id)
        })
        .unwrap_or_default()
        .into_iter()
        .map(|m| (m.id, m.name))
        .collect()
    } else {
        std::collections::HashMap::new()
    };

    let mut board: std::collections::BTreeMap<String, Vec<&Issue>> =
        std::collections::BTreeMap::new();
    for issue in &issues {
        let key = match q.group_by.as_str() {
            "priority" => issue.priority.clone(),
            "module" => issue
                .module_id
                .and_then(|m| module_names.get(&m).cloned())
                .unwrap_or("unassigned".into()),
            _ => issue.status.clone(),
        };
        board.entry(key).or_default().push(issue);
    }

    Ok(Json(serde_json::json!(board)))
}

#[cfg(test)]
mod tests {
    use crate::api::test_helpers::*;
    use crate::db::models::*;
    use axum::Extension;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn project_crud_lifecycle() {
        let app = test_app();

        // Create
        let (id, project) = seed_project(&app).await;
        assert_eq!(project["identifier"], "TST");

        // Get
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/projects/{id}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // List
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/projects")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let list: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(list.len(), 1);

        // Update
        let update = serde_json::json!({"name": "Renamed"});
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/projects/{id}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&update).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let updated: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(updated["name"], "Renamed");
        assert_eq!(updated["identifier"], "TST"); // unchanged

        // Delete
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/projects/{id}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Verify gone
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/projects/{id}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_nonexistent_project_returns_404() {
        let app = test_app();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/projects/99999")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn board_groups_by_status() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;

        for (title, status) in [("A", "todo"), ("B", "active"), ("C", "todo")] {
            let body = serde_json::json!({
                "project_id": project_id,
                "title": title,
                "status": status
            });
            app.clone()
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
        }

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/projects/{project_id}/board"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let board: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(board["todo"].as_array().unwrap().len(), 2);
        assert_eq!(board["active"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn board_groups_by_module_resolves_names() {
        let db = crate::db::open_memory().expect("test db");
        let app = crate::api::router(db.clone(), &[])
            .layer(Extension(crate::config::AuthConfig { allow_signup: true }));
        let (project_id, _) = seed_project(&app).await;

        // Create a module via direct DB access
        let conn = db.read().unwrap();
        crate::db::queries::create_module(
            &conn,
            &CreateModule {
                project_id,
                name: "Backend".into(),
                description: String::new(),
                status: "active".into(),
            },
        )
        .unwrap();
        let modules = crate::db::queries::list_modules(&conn, project_id).unwrap();
        let module_id = modules[0].id;
        drop(conn);

        // Create issues: one with module, one without
        for (title, mid) in [("With mod", Some(module_id)), ("No mod", None)] {
            let mut body = serde_json::json!({
                "project_id": project_id,
                "title": title,
            });
            if let Some(m) = mid {
                body["module_id"] = serde_json::json!(m);
            }
            app.clone()
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
        }

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/projects/{project_id}/board?group_by=module"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let board: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            board.get("has_module").is_none(),
            "should not use 'has_module' as key"
        );
        assert_eq!(board["Backend"].as_array().unwrap().len(), 1);
        assert_eq!(board["unassigned"].as_array().unwrap().len(), 1);
    }

    // ── Project lead permission tests ────────────────────────

    #[tokio::test]
    async fn project_lead_can_update_own_project() {
        let (db, _, lead, _, project_id) = setup_lead_test();
        let app = app_as_user(db, &lead);

        let update = serde_json::json!({"name": "Renamed by lead"});
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/projects/{project_id}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&update).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        assert_eq!(data["name"], "Renamed by lead");
    }

    #[tokio::test]
    async fn admin_can_update_any_project() {
        let (db, admin, _, _, project_id) = setup_lead_test();
        let app = app_as_user(db, &admin);

        let update = serde_json::json!({"name": "Renamed by admin"});
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/projects/{project_id}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&update).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn regular_user_cannot_update_project() {
        let (db, _, _, regular, project_id) = setup_lead_test();
        let app = app_as_user(db, &regular);

        let update = serde_json::json!({"name": "Hijacked"});
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/projects/{project_id}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&update).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn only_admin_can_delete_project() {
        let (db, admin, lead, _, project_id) = setup_lead_test();

        // Lead cannot delete
        let lead_app = app_as_user(db.clone(), &lead);
        let resp = lead_app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/projects/{project_id}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        // Admin can delete
        let admin_app = app_as_user(db, &admin);
        let resp = admin_app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/projects/{project_id}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
