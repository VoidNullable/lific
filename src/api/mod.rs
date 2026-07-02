mod activity;
mod auth;
mod comments;
mod export;
mod issues;
mod pages;
mod plans;
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
        // Public instance metadata for the auth screen (unauthenticated).
        .route("/api/instance", get(auth::instance_info))
        // Admin-only instance settings (authenticated; admin enforced in handler).
        .route(
            "/api/instance/settings",
            get(auth::instance_settings_get).patch(auth::instance_settings_patch),
        )
        // Auth
        .route("/api/auth/signup", post(auth::auth_signup))
        .route("/api/auth/login", post(auth::auth_login))
        // Single-user mode: mint an admin session without a password when the
        // instance flag is set. Public (the carve-out in auth_middleware_wrapper
        // must include this path) and default-deny when disabled. LIF-215.
        .route("/api/auth/auto-login", post(auth::auth_auto_login))
        .route("/api/auth/logout", post(auth::auth_logout))
        .route("/api/auth/me", get(auth::auth_me).patch(auth::update_me))
        .route("/api/auth/me/password", post(auth::change_password))
        .route("/api/auth/me/sessions", delete(auth::revoke_all_sessions))
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
            "/api/pages/{page_id}/comments",
            get(comments::list_page_comments).post(comments::create_page_comment),
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
        // Reorder must be registered before the `{id}` route so the static
        // segment wins the match (axum/matchit prioritises static over param,
        // but keeping it adjacent makes the intent obvious). LIF-233.
        .route("/api/projects/reorder", put(projects::reorder_projects))
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
        // Activity (audit log read surface — LIF-156)
        .route(
            "/api/issues/{id}/activity",
            get(activity::issue_activity),
        )
        .route("/api/pages/{id}/activity", get(activity::page_activity))
        .route(
            "/api/projects/{id}/activity",
            get(activity::project_activity),
        )
        .route(
            "/api/projects/{id}/activity/actors",
            get(activity::project_activity_actors),
        )
        .route("/api/export/issues/{identifier}", get(export::export_issue))
        .route("/api/export/pages/{identifier}", get(export::export_page))
        .route("/api/export/projects/{identifier}", get(export::export_project))
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
            get(resources::get_module)
                .put(resources::update_module)
                .delete(resources::delete_module_handler),
        )
        // Labels
        .route(
            "/api/labels",
            get(resources::list_labels).post(resources::create_label),
        )
        .route(
            "/api/labels/{id}",
            put(resources::update_label_handler).delete(resources::delete_label_handler),
        )
        .route(
            "/api/labels/{id}/merge",
            post(resources::merge_label_handler),
        )
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
        // Plans (LIF-172)
        .route(
            "/api/plans",
            get(plans::list_plans).post(plans::create_plan),
        )
        .route(
            "/api/plans/{id}",
            get(plans::get_plan)
                .put(plans::update_plan)
                .delete(plans::delete_plan_handler),
        )
        .route("/api/plans/resolve/{identifier}", get(plans::resolve_plan))
        .route("/api/plans/{id}/activity", get(activity::plan_activity))
        .route("/api/plans/{id}/steps", post(plans::add_step))
        .route(
            "/api/plans/{plan_id}/steps/{step_id}",
            put(plans::update_step).delete(plans::delete_step_handler),
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
        // Per-status issue counts (topbar tallies — LIF-161)
        .route(
            "/api/projects/{id}/issue-counts",
            get(projects::issue_counts),
        )
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
///
/// LIF-102: when `project.lead_user_id IS NULL`, only admins can edit. This
/// prevents the previous behavior where `Some(user.id) == None` was always
/// false and thus locked out every non-admin user. New projects default the
/// lead to the creator (see `create_project`), and the 011 migration backfills
/// existing unowned projects, so this branch should be rare in practice.
///
/// LIF-196: thin wrapper over `authz::require_role(.., Role::Lead)`, the
/// single enforcement primitive shared with MCP. Kept as its own function so
/// existing call sites (`api/resources.rs`, `api/projects.rs`) don't churn.
/// The behavior above is unchanged while the `authz_enforced` instance
/// setting is off (today's default); see `src/authz.rs` for the full mode
/// split.
fn require_project_lead(
    db: &DbPool,
    auth_user: &Option<AuthUser>,
    project_id: i64,
) -> Result<(), LificError> {
    crate::authz::require_role(db, auth_user, project_id, Role::Lead)
}

/// Require any authenticated user (LIF-233). Used for low-stakes, instance-wide
/// actions like sidebar project ordering, which shouldn't be gated behind
/// per-project lead/admin rights the way structural project edits are.
/// Default-deny: returns Forbidden when auth_user is None.
fn require_authenticated(auth_user: &Option<AuthUser>) -> Result<(), LificError> {
    match auth_user {
        Some(_) => Ok(()),
        None => Err(LificError::Forbidden("authentication required".into())),
    }
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
        // Insert a real admin row so FK constraints (e.g. projects.lead_user_id
        // now defaults to the creator — see LIF-102) pass. Direct SQL skips
        // argon2 hashing, keeping the test fixture cheap.
        let admin_id = {
            let conn = db.write().unwrap();
            conn.execute(
                "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot)
                 VALUES ('test-admin', 'admin@test.local', 'x', 'Test Admin', 1, 0)",
                [],
            )
            .expect("seed test admin");
            conn.last_insert_rowid()
        };
        super::router(db, &[])
            .layer(Extension(crate::config::AuthConfig { allow_signup: true, secure_cookies: false }))
            .layer(Extension(Some(AuthUser {
                id: admin_id,
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

    pub async fn json_get(app: &Router, uri: &str) -> axum::response::Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(uri)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    pub async fn json_put(
        app: &Router,
        uri: &str,
        body: serde_json::Value,
    ) -> axum::response::Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    pub async fn json_patch(
        app: &Router,
        uri: &str,
        body: serde_json::Value,
    ) -> axum::response::Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
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
            .layer(Extension(crate::config::AuthConfig { allow_signup: true, secure_cookies: false }))
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
