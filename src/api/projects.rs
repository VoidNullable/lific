use axum::{
    Extension,
    extract::{Json, Path, Query, State},
};

use crate::db::{DbPool, models::*};
use crate::error::LificError;

use super::{require_admin, require_authenticated, require_project_lead, with_read, with_write};

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
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(mut input): Json<CreateProject>,
) -> Result<Json<Project>, LificError> {
    // LIF-102 fix #1: if no lead was supplied, default to the authenticated
    // creator. This prevents the "unowned project" trap where require_project_lead
    // rejects everyone except admins.
    if input.lead_user_id.is_none()
        && let Some(user) = &auth_user
    {
        input.lead_user_id = Some(user.id);
    }
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

/// PUT /api/projects/reorder — persist the sidebar order (LIF-233). Takes the
/// full id list top-to-bottom; the server reindexes `sort_order`. Gated only on
/// being authenticated (order is instance-wide, not a privileged project edit),
/// so any logged-in user can rearrange — unlike `update_project`, which is
/// lead/admin-only.
pub(super) async fn reorder_projects(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<ReorderProjects>,
) -> Result<Json<Vec<Project>>, LificError> {
    require_authenticated(&auth_user)?;
    with_write(&db, |conn| {
        crate::db::queries::reorder_projects(conn, &input.ids)
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

/// Per-status issue counts + total for the topbar (LIF-161). Separate from
/// the list endpoint because that one is limit-capped — counting its rows
/// client-side silently undercounts past the cap.
pub(super) async fn issue_counts(
    State(db): State<DbPool>,
    Path(project_id): Path<i64>,
) -> Result<Json<IssueStatusCounts>, LificError> {
    with_read(&db, |conn| {
        crate::db::queries::count_issues_by_status(conn, project_id)
    })
    .map(Json)
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
                limit: Some(500),
                ..Default::default()
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
    async fn issue_counts_returns_per_status_tallies_and_total() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;

        for (title, status) in [
            ("A", "todo"),
            ("B", "active"),
            ("C", "todo"),
            ("D", "done"),
        ] {
            let body = serde_json::json!({
                "project_id": project_id,
                "title": title,
                "status": status
            });
            json_post(&app, "/api/issues", body).await;
        }

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/projects/{project_id}/issue-counts"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let counts = parse_json(resp).await;
        assert_eq!(counts["backlog"], 0);
        assert_eq!(counts["todo"], 2);
        assert_eq!(counts["active"], 1);
        assert_eq!(counts["done"], 1);
        assert_eq!(counts["cancelled"], 0);
        assert_eq!(counts["total"], 4);
    }

    #[tokio::test]
    async fn board_groups_by_module_resolves_names() {
        let db = crate::db::open_memory().expect("test db");
        // Seed a real admin so create_project's lead-defaulting (LIF-102)
        // can FK to a valid user row.
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
        let app = crate::api::router(db.clone(), &[])
            .layer(Extension(crate::config::AuthConfig { allow_signup: true, secure_cookies: false }))
            .layer(Extension(Some(AuthUser {
                id: admin_id,
                username: "test-admin".into(),
                display_name: "Test Admin".into(),
                is_admin: true,
            })));
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
                emoji: None,
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

    // ── LIF-102: project edit blocked when project has no lead ────────────
    //
    // The previous behavior compared `Some(user.id)` to `project.lead_user_id`,
    // which was `None`, so every non-admin user was rejected forever. The fix
    // is two-part: default the creator as lead on create (so the unowned state
    // is uncommon), and explicitly route the `None` case to admin-only access.

    /// Create a project with `lead_user_id = NULL` via direct DB access,
    /// bypassing the API's default-creator-as-lead behavior.
    fn seed_unowned_project(db: &crate::db::DbPool) -> i64 {
        let conn = db.write().unwrap();
        crate::db::queries::create_project(
            &conn,
            &CreateProject {
                name: "Unowned".into(),
                identifier: "UNO".into(),
                description: String::new(),
                emoji: None,
                lead_user_id: None,
            },
        )
        .unwrap()
        .id
    }

    #[tokio::test]
    async fn non_admin_cannot_edit_unowned_project() {
        let (db, _, _, regular, _) = setup_lead_test();
        let project_id = seed_unowned_project(&db);
        let app = app_as_user(db, &regular);

        let update = serde_json::json!({"name": "Sneaky rename"});
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
        let data = parse_json(resp).await;
        // Distinct message tells the user *why* they can't edit: no lead exists.
        assert!(
            data["error"]
                .as_str()
                .unwrap_or("")
                .contains("no lead"),
            "expected 'no lead' in error, got: {}",
            data["error"]
        );
    }

    #[tokio::test]
    async fn admin_can_edit_unowned_project() {
        let (db, admin, _, _, _) = setup_lead_test();
        let project_id = seed_unowned_project(&db);
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
        let data = parse_json(resp).await;
        assert_eq!(data["name"], "Renamed by admin");
    }

    // ── LIF-103: tristate clear via HTTP ─────────────────────────────────
    //
    // The model now distinguishes "field absent" from "field explicitly null"
    // so clients can wipe emoji/lead back to NULL. Before the fix, both
    // shapes collapsed to None and the update path skipped the column.

    #[tokio::test]
    async fn update_with_null_emoji_clears_emoji() {
        let (db, admin, _, _, _) = setup_lead_test();
        let app = app_as_user(db.clone(), &admin);

        // Seed a project with an emoji set.
        let project = {
            let conn = db.write().unwrap();
            crate::db::queries::create_project(
                &conn,
                &CreateProject {
                    name: "With Emoji".into(),
                    identifier: "EMJ".into(),
                    description: String::new(),
                    emoji: Some("🧪".into()),
                    lead_user_id: Some(admin.id),
                },
            )
            .unwrap()
        };
        assert_eq!(project.emoji.as_deref(), Some("🧪"));

        // PUT with explicit null.
        let update = serde_json::json!({"emoji": null});
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/projects/{}", project.id))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&update).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        assert!(data["emoji"].is_null(), "expected null emoji, got: {}", data["emoji"]);
    }

    #[tokio::test]
    async fn update_with_null_lead_clears_lead() {
        let (db, admin, lead, _, project_id) = setup_lead_test();
        // setup_lead_test creates project with lead set.
        let app = app_as_user(db.clone(), &admin); // admin can edit any project

        // Sanity check: lead is set.
        let pre: serde_json::Value = {
            let conn = db.read().unwrap();
            let p = crate::db::queries::get_project(&conn, project_id).unwrap();
            serde_json::to_value(&p).unwrap()
        };
        assert_eq!(pre["lead_user_id"].as_i64(), Some(lead.id));

        let update = serde_json::json!({"lead_user_id": null});
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
        assert!(
            data["lead_user_id"].is_null(),
            "expected null lead_user_id, got: {}",
            data["lead_user_id"]
        );
    }

    #[tokio::test]
    async fn update_with_empty_body_changes_nothing() {
        let (db, admin, lead, _, project_id) = setup_lead_test();
        let app = app_as_user(db, &admin);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/projects/{project_id}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(b"{}".to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        // Project from setup_lead_test has name "Lead Test", identifier "LDT",
        // lead set, no emoji.
        assert_eq!(data["name"], "Lead Test");
        assert_eq!(data["identifier"], "LDT");
        assert_eq!(data["lead_user_id"].as_i64(), Some(lead.id));
        assert!(data["emoji"].is_null());
    }

    #[tokio::test]
    async fn update_lead_to_nonexistent_user_returns_400() {
        let (db, admin, _, _, project_id) = setup_lead_test();
        let app = app_as_user(db, &admin);

        let update = serde_json::json!({"lead_user_id": 99999});
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
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let data = parse_json(resp).await;
        assert!(
            data["error"].as_str().unwrap_or("").contains("not found"),
            "expected 'not found' in error, got: {}",
            data["error"]
        );
    }

    // ── LIF-233: sidebar reordering ──────────────────────────

    /// POST a project with an explicit identifier; return its id.
    async fn seed_named_project(app: &axum::Router, name: &str, ident: &str) -> i64 {
        let resp = json_post(
            app,
            "/api/projects",
            serde_json::json!({ "name": name, "identifier": ident }),
        )
        .await;
        parse_json(resp).await["id"].as_i64().unwrap()
    }

    async fn list_project_names(app: &axum::Router) -> Vec<String> {
        let resp = json_get(app, "/api/projects").await;
        parse_json(resp)
            .await
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap().to_string())
            .collect()
    }

    #[tokio::test]
    async fn reorder_persists_new_order() {
        let app = test_app();
        let a = seed_named_project(&app, "Alpha", "AAA").await;
        let b = seed_named_project(&app, "Beta", "BBB").await;
        let c = seed_named_project(&app, "Gamma", "GGG").await;

        // Default order is alphabetical.
        assert_eq!(list_project_names(&app).await, ["Alpha", "Beta", "Gamma"]);

        // Reorder: Gamma, Alpha, Beta.
        let resp = json_put(
            &app,
            "/api/projects/reorder",
            serde_json::json!({ "ids": [c, a, b] }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        // Response echoes the new order.
        let returned: Vec<String> = parse_json(resp)
            .await
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(returned, ["Gamma", "Alpha", "Beta"]);
        // And a fresh GET reflects it.
        assert_eq!(list_project_names(&app).await, ["Gamma", "Alpha", "Beta"]);
    }

    #[tokio::test]
    async fn reorder_allowed_for_non_lead_user() {
        // Unlike update_project (lead/admin-only), reordering is open to any
        // authenticated user since sidebar order is instance-wide chrome.
        let (db, _, _, regular, _) = setup_lead_test();
        let app = app_as_user(db, &regular);

        // setup_lead_test already created project "LDT"; add a second.
        let ldt = {
            // resolve LDT's id from the list
            let names = json_get(&app, "/api/projects").await;
            parse_json(names).await[0]["id"].as_i64().unwrap()
        };
        let second = seed_named_project(&app, "Second", "SEC").await;

        let resp = json_put(
            &app,
            "/api/projects/reorder",
            serde_json::json!({ "ids": [second, ldt] }),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "a regular (non-lead) user should be allowed to reorder"
        );
        assert_eq!(list_project_names(&app).await, ["Second", "Lead Test"]);
    }

    #[tokio::test]
    async fn reorder_with_unknown_id_returns_400() {
        let app = test_app();
        let a = seed_named_project(&app, "Alpha", "AAA").await;
        let resp = json_put(
            &app,
            "/api/projects/reorder",
            serde_json::json!({ "ids": [a, 99999] }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_project_defaults_lead_to_creator() {
        // setup_lead_test gives us a real lead user we can authenticate as.
        let (db, _, lead, _, _) = setup_lead_test();
        let app = app_as_user(db, &lead);

        let body = serde_json::json!({
            "name": "My Project",
            "identifier": "MINE",
            "description": ""
            // intentionally no lead_user_id
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/projects")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        assert_eq!(
            data["lead_user_id"].as_i64(),
            Some(lead.id),
            "expected lead defaulted to creator, got: {}",
            data["lead_user_id"]
        );

        // And the creator can subsequently edit it (the whole point — no more trap).
        let pid = data["id"].as_i64().unwrap();
        let update = serde_json::json!({"name": "Renamed by creator"});
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/projects/{pid}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&update).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
