mod auth;
mod comments;
mod issues;
mod pages;
mod projects;
mod resources;

use axum::{
    Router,
    extract::{Json, Query, State},
    routing::{delete, get, post, put},
};
use tower_http::cors::{self, CorsLayer};

use crate::db::{DbPool, models::*, queries};
use crate::error::LificError;

/// Build the full API router.
pub fn router(db: DbPool, cors_origins: &[String]) -> Router {
    let cors = if cors_origins.is_empty() {
        CorsLayer::new().allow_origin(cors::Any)
    } else {
        let origins: Vec<axum::http::HeaderValue> = cors_origins
            .iter()
            .filter_map(|o| o.parse().ok())
            .collect();
        CorsLayer::new().allow_origin(origins)
    };

    Router::new()
        // Auth
        .route("/api/auth/signup", post(auth::auth_signup))
        .route("/api/auth/login", post(auth::auth_login))
        .route("/api/auth/logout", post(auth::auth_logout))
        .route("/api/auth/me", get(auth::auth_me))
        .route(
            "/api/auth/keys",
            get(auth::list_keys).post(auth::create_key),
        )
        .route("/api/auth/keys/{id}", delete(auth::revoke_key))
        // Connected tools (bots)
        .route(
            "/api/auth/bots",
            get(auth::list_bots).post(auth::create_bot),
        )
        .route("/api/auth/bots/{id}/disconnect", post(auth::disconnect_bot))
        .route("/api/auth/bots/{id}", delete(auth::delete_bot))
        // Comments
        .route(
            "/api/issues/{issue_id}/comments",
            get(comments::list_comments).post(comments::create_comment),
        )
        .route(
            "/api/comments/{id}",
            put(comments::update_comment_handler).delete(comments::delete_comment_handler),
        )
        // Projects
        .route(
            "/api/projects",
            get(projects::list_projects).post(projects::create_project),
        )
        .route(
            "/api/projects/{id}",
            get(projects::get_project)
                .put(projects::update_project)
                .delete(projects::delete_project_handler),
        )
        // Issues
        .route(
            "/api/issues",
            get(issues::list_issues).post(issues::create_issue),
        )
        .route(
            "/api/issues/{id}",
            get(issues::get_issue)
                .put(issues::update_issue)
                .delete(issues::delete_issue_handler),
        )
        .route(
            "/api/issues/resolve/{identifier}",
            get(issues::resolve_issue),
        )
        // Issue relations
        .route("/api/issues/link", post(issues::link_issues))
        .route("/api/issues/unlink", post(issues::unlink_issues))
        // Modules
        .route(
            "/api/modules",
            get(resources::list_modules).post(resources::create_module),
        )
        .route(
            "/api/modules/{id}",
            put(resources::update_module).delete(resources::delete_module_handler),
        )
        // Labels
        .route(
            "/api/labels",
            get(resources::list_labels).post(resources::create_label),
        )
        .route("/api/labels/{id}", delete(resources::delete_label_handler))
        // Pages
        .route(
            "/api/pages",
            get(pages::list_pages_handler).post(pages::create_page),
        )
        .route(
            "/api/pages/{id}",
            get(pages::get_page)
                .put(pages::update_page)
                .delete(pages::delete_page_handler),
        )
        // Folders
        .route(
            "/api/folders",
            get(resources::list_folders_handler).post(resources::create_folder),
        )
        .route(
            "/api/folders/{id}",
            delete(resources::delete_folder_handler),
        )
        // Users (for dropdowns)
        .route("/api/users", get(auth::list_users))
        // Search
        .route("/api/search", get(search))
        // Board view
        .route("/api/projects/{id}/board", get(projects::get_board))
        // Health
        .route("/api/health", get(health))
        .layer(
            cors.allow_methods([
                    axum::http::Method::GET,
                    axum::http::Method::POST,
                    axum::http::Method::PUT,
                    axum::http::Method::DELETE,
                ])
                .allow_headers([
                    axum::http::header::CONTENT_TYPE,
                    axum::http::header::AUTHORIZATION,
                ]),
        )
        .with_state(db)
}

// ── Shared helpers ───────────────────────────────────────────

/// Execute a read-only operation against the read pool.
fn with_read<F, T>(db: &DbPool, f: F) -> Result<T, LificError>
where
    F: FnOnce(&rusqlite::Connection) -> Result<T, LificError>,
{
    let conn = db.read()?;
    f(&conn)
}

/// Execute a write operation against the exclusive write connection.
fn with_write<F, T>(db: &DbPool, f: F) -> Result<T, LificError>
where
    F: FnOnce(&rusqlite::Connection) -> Result<T, LificError>,
{
    let conn = db.write()?;
    f(&conn)
}

/// Check if the authenticated user can manage a project (update settings, manage structure).
/// Returns Ok(()) if: user is admin, or user is project lead.
/// Default-deny: returns Forbidden when auth_user is None (OAuth tokens, legacy keys).
fn require_project_lead(
    db: &DbPool,
    auth_user: &Option<AuthUser>,
    project_id: i64,
) -> Result<(), LificError> {
    let Some(user) = auth_user else {
        return Err(LificError::Forbidden(
            "only the project lead or an admin can do this".into(),
        ));
    };
    if user.is_admin {
        return Ok(());
    }
    let project = with_read(db, |conn| queries::get_project(conn, project_id))?;
    if project.lead_user_id == Some(user.id) {
        return Ok(());
    }
    Err(LificError::Forbidden(
        "only the project lead or an admin can do this".into(),
    ))
}

/// Check if the authenticated user is an admin.
/// Default-deny: returns Forbidden when auth_user is None (OAuth tokens, legacy keys).
fn require_admin(auth_user: &Option<AuthUser>) -> Result<(), LificError> {
    match auth_user {
        Some(user) if user.is_admin => Ok(()),
        _ => Err(LificError::Forbidden("only an admin can do this".into())),
    }
}

// ── Cross-cutting endpoints ──────────────────────────────────

async fn health() -> &'static str {
    "ok"
}

async fn search(
    State(db): State<DbPool>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<Vec<SearchResult>>, LificError> {
    with_read(&db, |conn| queries::search(conn, &q)).map(Json)
}

// ── Shared test helpers ──────────────────────────────────────

#[cfg(test)]
pub(crate) mod test_helpers {
    use axum::http::Request;
    use axum::{Extension, Router};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::db::DbPool;
    use crate::db::models::*;

    pub fn test_app() -> Router {
        let db = crate::db::open_memory().expect("test db");
        super::router(db, &[])
            .layer(Extension(crate::config::AuthConfig { allow_signup: true }))
            .layer(Extension(Some(AuthUser {
                id: 0,
                username: "test-admin".into(),
                display_name: "Test Admin".into(),
                is_admin: true,
            })))
    }

    /// Seed a project and return its id.
    pub async fn seed_project(app: &Router) -> (i64, serde_json::Value) {
        let body = serde_json::json!({
            "name": "Test Project",
            "identifier": "TST",
            "description": "integration test project"
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
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let id = val["id"].as_i64().unwrap();
        (id, val)
    }

    pub async fn json_post(
        app: &Router,
        uri: &str,
        body: serde_json::Value,
    ) -> axum::response::Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    pub async fn parse_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// Build a test app authenticated as a specific user.
    pub fn app_as_user(db: DbPool, user: &User) -> Router {
        super::router(db, &[])
            .layer(Extension(crate::config::AuthConfig { allow_signup: true }))
            .layer(Extension(Some(AuthUser {
                id: user.id,
                username: user.username.clone(),
                display_name: user.display_name.clone(),
                is_admin: user.is_admin,
            })))
    }

    /// Set up a DB with an admin, a project lead, a regular user, and a project.
    pub fn setup_lead_test() -> (DbPool, User, User, User, i64) {
        let db = crate::db::open_memory().expect("test db");
        let conn = db.write().unwrap();

        let admin = crate::db::queries::users::create_user(
            &conn,
            &CreateUser {
                username: "admin".into(),
                email: "admin@test.com".into(),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: true,
                is_bot: false,
            },
        )
        .unwrap();

        let lead = crate::db::queries::users::create_user(
            &conn,
            &CreateUser {
                username: "lead".into(),
                email: "lead@test.com".into(),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap();

        let regular = crate::db::queries::users::create_user(
            &conn,
            &CreateUser {
                username: "regular".into(),
                email: "regular@test.com".into(),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap();

        let project = crate::db::queries::create_project(
            &conn,
            &CreateProject {
                name: "Lead Test".into(),
                identifier: "LDT".into(),
                description: String::new(),
                emoji: None,
                lead_user_id: Some(lead.id),
            },
        )
        .unwrap();

        drop(conn);
        (db, admin, lead, regular, project.id)
    }
}

#[cfg(test)]
mod tests {
    use super::test_helpers::*;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_returns_ok() {
        let app = test_app();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn search_returns_results() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;

        // Create an issue to search for
        let body = serde_json::json!({
            "project_id": project_id,
            "title": "Unique searchable title xyz"
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

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/search?query=searchable")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let results: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
        assert!(!results.is_empty());
    }
}
