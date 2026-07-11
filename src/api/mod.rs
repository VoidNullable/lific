mod activity;
mod attachments;
mod auth;
mod comments;
mod export;
mod insights;
mod issues;
mod members;
mod pages;
mod plans;
mod projects;
mod resources;
mod views;

use axum::{
    Router,
    extract::{DefaultBodyLimit, Extension, Json, Query, State, ws::WebSocketUpgrade},
    http::{HeaderMap, header},
    response::IntoResponse,
    routing::{delete, get, patch, post, put},
};
use tower_http::cors::{self, CorsLayer};

/// Transport-level body-size ceiling for the multipart upload route only. The
/// per-instance `AttachmentConfig.max_bytes` (default 10 MB) is the real limit
/// the handler enforces with an exact byte count and a friendly error; this
/// ceiling just has to be comfortably above it so the raised limit isn't the
/// thing that rejects a legitimate upload before the handler can. It overrides
/// the global 2 MB `DefaultBodyLimit` (main.rs) for this route alone —
/// everything else stays capped at 2 MB.
const UPLOAD_BODY_LIMIT: usize = 64 * 1024 * 1024;

use crate::db::{DbPool, models::*, queries};
use crate::error::LificError;

pub use attachments::{AttachmentConfig, AttachmentUploadLimiter};

/// Build the full API router.
pub fn router(db: DbPool, cors_origins: &[String]) -> Router {
    let cors = if cors_origins.is_empty() {
        CorsLayer::new().allow_origin(cors::Any)
    } else {
        let origins: Vec<axum::http::HeaderValue> =
            cors_origins.iter().filter_map(|o| o.parse().ok()).collect();
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
        .route("/api/issues/{id}/activity", get(activity::issue_activity))
        .route("/api/pages/{id}/activity", get(activity::page_activity))
        .route(
            "/api/projects/{id}/activity",
            get(activity::project_activity),
        )
        .route(
            "/api/projects/{id}/activity/actors",
            get(activity::project_activity_actors),
        )
        // Insights (per-project analytics tab — LIF-240)
        .route(
            "/api/projects/{id}/insights",
            get(insights::project_insights),
        )
        .route("/api/export/issues/{identifier}", get(export::export_issue))
        .route("/api/export/pages/{identifier}", get(export::export_page))
        .route(
            "/api/export/projects/{identifier}",
            get(export::export_project),
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
        .route("/api/events/ws", get(events_ws))
        // Search
        .route("/api/search", get(search))
        // Board view
        .route("/api/projects/{id}/board", get(projects::get_board))
        // Membership management (LIF-199) — lead-gated, web/REST only per
        // design LIF-DOC-7 decision #14 (no MCP tools).
        .route(
            "/api/projects/{id}/members",
            get(members::list_project_members).post(members::add_project_member),
        )
        .route(
            "/api/projects/{id}/members/{user_id}",
            patch(members::update_project_member).delete(members::remove_project_member),
        )
        // The caller's own effective role on a project (LIF-234) — Viewer-gated,
        // so any member can learn their role to drive role-aware UI affordances
        // without reading the full roster or the admin-only instance settings.
        .route("/api/projects/{id}/my-role", get(members::my_project_role))
        // @mention autocomplete candidates (LIF-263) — Viewer-gated,
        // member-scoped when authz enforcement is on.
        .route(
            "/api/projects/{id}/mention-candidates",
            get(comments::mention_candidates),
        )
        // Per-status issue counts (topbar tallies — LIF-161)
        .route(
            "/api/projects/{id}/issue-counts",
            get(projects::issue_counts),
        )
        // GitHub import (LIF-264) — lead-gated. dry_run in the body drives the
        // preview step.
        .route(
            "/api/projects/{id}/import/github",
            post(projects::import_github),
        )
        // Saved views (LIF-242) — Viewer-gated + strict per-user ownership
        // enforced in the query layer (see src/api/views.rs doc comment).
        .route(
            "/api/projects/{id}/views",
            get(views::list_views).post(views::create_view),
        )
        .route(
            "/api/projects/{id}/views/{view_id}",
            patch(views::update_view).delete(views::delete_view),
        )
        // Attachments (LIF-262) — image + file uploads on issues, comments,
        // and pages. The upload route carries its own larger DefaultBodyLimit
        // (overriding the global 2 MB) so multipart uploads up to the
        // configured max aren't rejected at the transport layer.
        .route(
            "/api/attachments",
            get(attachments::list_entity_attachments)
                .post(attachments::upload_attachment)
                .layer(DefaultBodyLimit::max(UPLOAD_BODY_LIMIT)),
        )
        .route(
            "/api/attachments/{id}",
            get(attachments::download_attachment).delete(attachments::delete_attachment),
        )
        // Health
        .route("/api/health", get(health))
        .layer(
            cors.allow_methods([
                    axum::http::Method::GET,
                    axum::http::Method::POST,
                axum::http::Method::PATCH,
                    axum::http::Method::PUT,
                    axum::http::Method::DELETE,
                ])
                .allow_headers([
                    axum::http::header::CONTENT_TYPE,
                    axum::http::header::AUTHORIZATION,
                ]),
        )
        .with_state(db)
        .layer(Extension(cors_origins.to_vec()))
}

async fn events_ws(
    State(db): State<DbPool>,
    Extension(realtime): Extension<crate::realtime::RealtimeHub>,
    Extension(allowed_origins): Extension<Vec<String>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, LificError> {
    let session_token = validate_websocket_request(&db, &headers, &allowed_origins)?;
    Ok(ws.on_upgrade(move |socket| {
        crate::realtime::serve_socket(socket, realtime, db, session_token)
    }))
}

fn validate_websocket_request(
    db: &DbPool,
    headers: &HeaderMap,
    allowed_origins: &[String],
) -> Result<String, LificError> {
    match (
        websocket_origin_allowed(headers, allowed_origins),
        websocket_session_token(headers),
    ) {
        (false, _) => Err(LificError::Forbidden("websocket origin not allowed".into())),
        (true, None) => Err(LificError::Forbidden("authentication required".into())),
        (true, Some(token)) => with_read(db, |conn| {
            match crate::db::queries::users::validate_session(conn, token) {
                Ok(_) => Ok(token.to_string()),
                Err(LificError::BadRequest(message))
                    if message == crate::db::queries::users::INVALID_SESSION_MESSAGE =>
                {
                    Err(LificError::Forbidden(
                        crate::db::queries::users::INVALID_SESSION_MESSAGE.into(),
                    ))
                }
                Err(error) => Err(error),
            }
        }),
    }
}

fn websocket_origin_allowed(headers: &HeaderMap, allowed_origins: &[String]) -> bool {
    match headers.get(header::ORIGIN).map(|value| value.to_str()) {
        None => true,
        Some(Ok(origin)) => {
            allowed_origins.iter().any(|allowed| allowed == origin)
                || headers
                .get(header::HOST)
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|host| {
                        websocket_same_origin(origin, host, websocket_request_scheme(headers))
                    })
        }
        Some(Err(_)) => false,
    }
}

fn websocket_request_scheme(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            headers
                .get("forwarded")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| {
                    value.split(';').find_map(|part| {
                        part.trim()
                            .strip_prefix("proto=")
                            .map(str::trim)
                            .filter(|proto| !proto.is_empty())
                    })
                })
        })
}

fn websocket_same_origin(origin: &str, host: &str, request_scheme: Option<&str>) -> bool {
    let Some(request_scheme) = request_scheme else {
        return false;
    };
    let origin = origin.parse::<axum::http::Uri>().ok();
    let host = host.parse::<axum::http::uri::Authority>().ok();

    match (
        origin.as_ref().and_then(axum::http::Uri::scheme_str),
        origin.as_ref().and_then(axum::http::Uri::authority),
        host.as_ref(),
    ) {
        (Some(scheme), Some(origin_authority), Some(host_authority))
            if scheme.eq_ignore_ascii_case(request_scheme) =>
        {
            websocket_default_port(scheme).is_some_and(|default_port| {
                let origin_port = origin_authority.port_u16().unwrap_or(default_port);
                let host_port = host_authority.port_u16().unwrap_or(default_port);
                origin_authority
                    .host()
                    .eq_ignore_ascii_case(host_authority.host())
                    && origin_port == host_port
            })
        }
        _ => false,
    }
}

fn websocket_default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    }
}

fn websocket_session_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookie| {
            cookie.split(';').find_map(|part| {
                part.trim()
                    .strip_prefix("lific_token=")
                    .map(str::trim)
                    .filter(|token| token.starts_with("lific_sess_"))
            })
        })
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

/// LIF-197: thin wrapper over `authz::require_structure_role` for the
/// module/label/folder ("structure") endpoints. See that function's doc
/// comment for why it can't just be `require_role(.., Maintainer)`.
fn require_structure_role(
    db: &DbPool,
    auth_user: &Option<AuthUser>,
    project_id: i64,
) -> Result<(), LificError> {
    crate::authz::require_structure_role(db, auth_user, project_id)
}

/// LIF-197: thin wrapper over `authz::require_project_delete_role`, used by
/// `DELETE /api/projects/{id}`. See that function's doc comment for why the
/// legacy branch reproduces `require_admin` exactly rather than delegating
/// to `require_role(.., Lead)`.
fn require_project_delete(
    db: &DbPool,
    auth_user: &Option<AuthUser>,
    project_id: i64,
) -> Result<(), LificError> {
    crate::authz::require_project_delete_role(db, auth_user, project_id)
}

/// LIF-197: apply the `visible_project_ids` cross-project read filter to a
/// list of items. `None` (unrestricted — admin, or enforcement off) keeps
/// everything. `Some(ids)` keeps only items whose `project_id_of` result is
/// `Some(pid)` with `pid` in `ids` — a workspace-level item (`None`
/// project_id) is therefore excluded for any non-admin once enforcement is
/// on, matching design decision #10 (workspace pages are admin-only).
fn filter_visible<T>(
    items: Vec<T>,
    visible: &Option<std::collections::HashSet<i64>>,
    project_id_of: impl Fn(&T) -> Option<i64>,
) -> Vec<T> {
    match visible {
        None => items,
        Some(ids) => items
            .into_iter()
            .filter(|it| project_id_of(it).is_some_and(|pid| ids.contains(&pid)))
            .collect(),
    }
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
    axum::Extension(auth_user): axum::Extension<Option<AuthUser>>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<Vec<SearchResult>>, LificError> {
    // Cross-project read (LIF-197 scope item 2): non-visible projects are
    // simply absent from results, not an error — even when `q.project_id`
    // narrows the search to one project, since a non-member of that project
    // shouldn't be able to probe its existence via a 403 vs. empty-results
    // side channel here.
    let visible = crate::authz::visible_project_ids(&db, &auth_user)?;
    let results = with_read(&db, |conn| queries::search(conn, &q))?;
    Ok(Json(filter_visible(results, &visible, |r| r.project_id)))
}

// ── Shared test helpers ──────────────────────────────────────

#[cfg(test)]
pub(crate) mod test_helpers {
    use axum::http::Request;
    use axum::{Extension, Router};
    use http_body_util::BodyExt;
    use std::sync::Arc;
    use tower::ServiceExt;

    use crate::db::DbPool;
    use crate::db::models::*;

    pub struct RealtimeTestApp {
        pub app: Router,
        pub realtime: crate::realtime::RealtimeHub,
    }

    /// A unique tempdir-backed attachment store for a test app, plus the
    /// config + rate-limiter extensions the attachment routes need. Layered
    /// onto every test app so the attachment endpoints work in tests without a
    /// real data dir.
    pub fn test_attachment_store() -> crate::storage::AttachmentStore {
        let dir = std::env::temp_dir().join(format!(
            "lific_att_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        crate::storage::AttachmentStore::new(dir)
    }

    /// Layer the three attachment extensions onto a router under test.
    pub fn with_attachment_layers(router: Router) -> Router {
        with_attachment_layers_store(router, test_attachment_store())
    }

    pub fn with_attachment_layers_store(
        router: Router,
        store: crate::storage::AttachmentStore,
    ) -> Router {
        router
            .layer(Extension(store))
            .layer(Extension(super::AttachmentConfig::default()))
            .layer(Extension(Arc::new(super::AttachmentUploadLimiter(
                crate::ratelimit::RateLimiter::new(1000, std::time::Duration::from_secs(3600)),
            ))))
    }

    pub fn test_app() -> Router {
        test_app_with_realtime().app
    }

    /// Like [`test_app`] but with an explicit `[auth] required` value, for
    /// LIF-297's auth-optional web bootstrap tests.
    pub fn test_app_with_auth(required: bool) -> Router {
        test_app_with_realtime_and_auth(required).app
    }

    pub fn test_app_with_realtime() -> RealtimeTestApp {
        test_app_with_realtime_and_auth(true)
    }

    fn test_app_with_realtime_and_auth(required: bool) -> RealtimeTestApp {
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
        let realtime = crate::realtime::RealtimeHub::new();
        let app = with_attachment_layers(super::router(db, &[]))
            .layer(Extension(realtime.clone()))
            .layer(Extension(crate::config::AuthConfig {
                allow_signup: true,
                required,
                secure_cookies: false,
            }))
            .layer(Extension(Some(AuthUser {
                id: admin_id,
                username: "test-admin".into(),
                display_name: "Test Admin".into(),
                is_admin: true,
            })));
        RealtimeTestApp { app, realtime }
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

    pub async fn json_delete(app: &Router, uri: &str) -> axum::response::Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(uri)
                    .body(axum::body::Body::empty())
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
        with_attachment_layers(super::router(db, &[]))
            .layer(Extension(crate::realtime::RealtimeHub::new()))
            .layer(Extension(crate::config::AuthConfig {
                allow_signup: true,
                required: true,
                secure_cookies: false,
            }))
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

    /// LIF-197: set up a DB with `authz_enforced` ON, an admin, and a
    /// project with a lead/maintainer/viewer member plus a non-member —
    /// the fixture the LIF-197 flag-ON test matrix (and its LIF-198 MCP
    /// sibling) both need. Returns
    /// `(db, admin, lead, maintainer, viewer, non_member, project_id)`.
    pub fn setup_membership_test() -> (DbPool, User, User, User, User, User, i64) {
        let db = crate::db::open_memory().expect("test db");
        let conn = db.write().unwrap();

        crate::db::queries::settings::update(
            &conn,
            crate::db::queries::settings::InstanceSettingsPatch {
                authz_enforced: Some(true),
                ..Default::default()
            },
        )
        .unwrap();

        let mk_user = |username: &str, is_admin: bool| {
            crate::db::queries::users::create_user(
                &conn,
                &CreateUser {
                    username: username.into(),
                    email: format!("{username}@test.com"),
                    password: "testpassword1".into(),
                    display_name: None,
                    is_admin,
                    is_bot: false,
                },
            )
            .unwrap()
        };

        let admin = mk_user("admin", true);
        let lead = mk_user("lead", false);
        let maintainer = mk_user("maintainer", false);
        let viewer = mk_user("viewer", false);
        let non_member = mk_user("non_member", false);

        let project = crate::db::queries::create_project(
            &conn,
            &CreateProject {
                name: "Membership Test".into(),
                identifier: "MEM".into(),
                description: String::new(),
                emoji: None,
                lead_user_id: Some(lead.id),
            },
        )
        .unwrap();

        crate::db::queries::members::upsert_member(
            &conn,
            project.id,
            maintainer.id,
            Role::Maintainer,
        )
            .unwrap();
        crate::db::queries::members::upsert_member(&conn, project.id, viewer.id, Role::Viewer)
            .unwrap();

        drop(conn);
        (db, admin, lead, maintainer, viewer, non_member, project.id)
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

/// LIF-197: project-scoped authorization enforcement across every REST
/// handler. Flag-ON cases exercise the full viewer/maintainer/lead matrix;
/// the flag-OFF smoke test is the regression proof that today's behavior
/// (all 530 pre-existing tests, run flag-OFF by default) hasn't moved.
#[cfg(test)]
mod authz_gating_tests {
    use super::test_helpers::*;
    use crate::db::models::*;
    use axum::http::StatusCode;

    // ── Reads: single-resource Viewer gate ──────────────────────

    #[tokio::test]
    async fn issue_read_denies_non_member_allows_viewer() {
        let (db, _admin, lead, _maintainer, viewer, non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db.clone(), &lead);
        let issue = parse_json(
            json_post(
                &lead_app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id, "title": "Secret work" }),
            )
            .await,
        )
        .await;
        let issue_id = issue["id"].as_i64().unwrap();

        let non_member_app = app_as_user(db.clone(), &non_member);
        assert_eq!(
            json_get(&non_member_app, &format!("/api/issues/{issue_id}"))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );

        let viewer_app = app_as_user(db, &viewer);
        assert_eq!(
            json_get(&viewer_app, &format!("/api/issues/{issue_id}"))
                .await
                .status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn page_and_plan_reads_follow_the_same_viewer_gate() {
        let (db, _admin, lead, _maintainer, viewer, non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db.clone(), &lead);
        let page = parse_json(
            json_post(
                &lead_app,
                "/api/pages",
                serde_json::json!({ "project_id": project_id, "title": "Doc" }),
            )
            .await,
        )
        .await;
        let page_id = page["id"].as_i64().unwrap();
        let plan = parse_json(
            json_post(
                &lead_app,
                "/api/plans",
                serde_json::json!({ "project_id": project_id, "title": "Plan" }),
            )
            .await,
        )
        .await;
        let plan_id = plan["id"].as_i64().unwrap();

        let non_member_app = app_as_user(db.clone(), &non_member);
        assert_eq!(
            json_get(&non_member_app, &format!("/api/pages/{page_id}"))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            json_get(&non_member_app, &format!("/api/plans/{plan_id}"))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );

        let viewer_app = app_as_user(db, &viewer);
        assert_eq!(
            json_get(&viewer_app, &format!("/api/pages/{page_id}"))
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            json_get(&viewer_app, &format!("/api/plans/{plan_id}"))
                .await
                .status(),
            StatusCode::OK
        );
    }

    // ── Reads: cross-project list/search filter instead of denying ──

    #[tokio::test]
    async fn issue_cross_project_list_filters_instead_of_denying() {
        let (db, _admin, lead, _maintainer, viewer, non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db.clone(), &lead);
        json_post(
            &lead_app,
            "/api/issues",
            serde_json::json!({ "project_id": project_id, "title": "Members only" }),
        )
        .await;

        // No project_id filter → cross-project list. A non-member must get
        // 200 with an empty (filtered) result, never a 403.
        let non_member_app = app_as_user(db.clone(), &non_member);
        let resp = json_get(&non_member_app, "/api/issues").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(parse_json(resp).await.as_array().unwrap().len(), 0);

        let viewer_app = app_as_user(db, &viewer);
        let resp = json_get(&viewer_app, "/api/issues").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(parse_json(resp).await.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn search_filters_out_non_visible_projects() {
        let (db, _admin, lead, _maintainer, _viewer, non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db.clone(), &lead);
        json_post(
            &lead_app,
            "/api/issues",
            serde_json::json!({ "project_id": project_id, "title": "Unique searchable xyzzy" }),
        )
        .await;

        let non_member_app = app_as_user(db, &non_member);
        let resp = json_get(&non_member_app, "/api/search?query=xyzzy").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            parse_json(resp).await.as_array().unwrap().is_empty(),
            "non-member must not see search hits from a project they can't see"
        );
    }

    #[tokio::test]
    async fn project_list_filters_to_member_projects() {
        let (db, _admin, lead, _maintainer, _viewer, non_member, _project_id) =
            setup_membership_test();

        let non_member_app = app_as_user(db.clone(), &non_member);
        let list = parse_json(json_get(&non_member_app, "/api/projects").await).await;
        assert_eq!(list.as_array().unwrap().len(), 0);

        let lead_app = app_as_user(db, &lead);
        let list = parse_json(json_get(&lead_app, "/api/projects").await).await;
        assert_eq!(list.as_array().unwrap().len(), 1);
    }

    // ── Writes: content mutations gated at Maintainer ────────────

    #[tokio::test]
    async fn issue_create_gated_by_maintainer_role() {
        let (db, admin, lead, maintainer, viewer, non_member, project_id) = setup_membership_test();

        for (user, expect_ok) in [
            (&non_member, false),
            (&viewer, false),
            (&maintainer, true),
            (&lead, true),
            (&admin, true),
        ] {
            let app = app_as_user(db.clone(), user);
            let resp = json_post(
                &app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id, "title": format!("by {}", user.username) }),
            )
            .await;
            let expected = if expect_ok {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            };
            assert_eq!(
                resp.status(),
                expected,
                "{} create expected {expected}",
                user.username
            );
        }
    }

    #[tokio::test]
    async fn issue_update_and_delete_gated_by_maintainer_role() {
        let (db, _admin, lead, maintainer, viewer, non_member, project_id) =
            setup_membership_test();
        let maintainer_app = app_as_user(db.clone(), &maintainer);
        let issue = parse_json(
            json_post(
                &maintainer_app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id, "title": "Target" }),
            )
            .await,
        )
        .await;
        let issue_id = issue["id"].as_i64().unwrap();

        let viewer_app = app_as_user(db.clone(), &viewer);
        assert_eq!(
            json_put(
                &viewer_app,
                &format!("/api/issues/{issue_id}"),
                serde_json::json!({"title": "hijack"})
            )
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        let non_member_app = app_as_user(db.clone(), &non_member);
        assert_eq!(
            json_put(
                &non_member_app,
                &format!("/api/issues/{issue_id}"),
                serde_json::json!({"title": "hijack"})
            )
                .await
                .status(),
            StatusCode::FORBIDDEN
        );

        let lead_app = app_as_user(db.clone(), &lead);
        assert_eq!(
            json_put(
                &lead_app,
                &format!("/api/issues/{issue_id}"),
                serde_json::json!({"title": "renamed"})
            )
                .await
                .status(),
            StatusCode::OK
        );

        assert_eq!(
            json_delete(&viewer_app, &format!("/api/issues/{issue_id}"))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            json_delete(&lead_app, &format!("/api/issues/{issue_id}"))
                .await
                .status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn page_and_plan_writes_gated_by_maintainer_role() {
        let (db, _admin, lead, maintainer, viewer, non_member, project_id) =
            setup_membership_test();

        for (user, expect_ok) in [
            (&viewer, false),
            (&non_member, false),
            (&maintainer, true),
            (&lead, true),
        ] {
            let app = app_as_user(db.clone(), user);
            let resp = json_post(
                &app,
                "/api/pages",
                serde_json::json!({ "project_id": project_id, "title": format!("page by {}", user.username) }),
            )
            .await;
            let expected = if expect_ok {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            };
            assert_eq!(resp.status(), expected, "page create by {}", user.username);

            let resp = json_post(
                &app,
                "/api/plans",
                serde_json::json!({ "project_id": project_id, "title": format!("plan by {}", user.username) }),
            )
            .await;
            assert_eq!(resp.status(), expected, "plan create by {}", user.username);
        }
    }

    // ── Comments: Viewer can read + create; non-member cannot ───

    #[tokio::test]
    async fn comment_create_allows_viewer_denies_non_member() {
        let (db, _admin, lead, _maintainer, viewer, non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db.clone(), &lead);
        let issue = parse_json(
            json_post(
                &lead_app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id, "title": "Commentable" }),
            )
            .await,
        )
        .await;
        let issue_id = issue["id"].as_i64().unwrap();

        let viewer_app = app_as_user(db.clone(), &viewer);
        let resp = json_post(
            &viewer_app,
            &format!("/api/issues/{issue_id}/comments"),
            serde_json::json!({ "content": "viewers can comment" }),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "viewer must be allowed to comment"
        );

        let non_member_app = app_as_user(db, &non_member);
        let resp = json_post(
            &non_member_app,
            &format!("/api/issues/{issue_id}/comments"),
            serde_json::json!({ "content": "should not land" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    // ── Structure endpoints: loosened to Maintainer once enforced ──

    #[tokio::test]
    async fn structure_endpoints_viewer_denied_maintainer_allowed() {
        let (db, _admin, _lead, maintainer, viewer, non_member, project_id) =
            setup_membership_test();

        let viewer_app = app_as_user(db.clone(), &viewer);
        assert_eq!(
            json_post(
                &viewer_app,
                "/api/modules",
                serde_json::json!({"project_id": project_id, "name": "Nope"})
            )
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        let non_member_app = app_as_user(db.clone(), &non_member);
        assert_eq!(
            json_post(
                &non_member_app,
                "/api/labels",
                serde_json::json!({"project_id": project_id, "name": "nope"})
            )
                .await
                .status(),
            StatusCode::FORBIDDEN
        );

        let maintainer_app = app_as_user(db.clone(), &maintainer);
        assert_eq!(
            json_post(
                &maintainer_app,
                "/api/modules",
                serde_json::json!({"project_id": project_id, "name": "Backend"})
            )
                .await
                .status(),
            StatusCode::OK,
            "maintainer should manage structure once enforcement loosens the gate"
        );
        assert_eq!(
            json_post(
                &maintainer_app,
                "/api/folders",
                serde_json::json!({"project_id": project_id, "name": "Docs"})
            )
                .await
                .status(),
            StatusCode::OK
        );
    }

    // ── Project settings / delete: Lead ──────────────────────────

    #[tokio::test]
    async fn project_settings_update_maintainer_denied_lead_allowed() {
        let (db, _admin, lead, maintainer, _viewer, _non_member, project_id) =
            setup_membership_test();

        let maintainer_app = app_as_user(db.clone(), &maintainer);
        assert_eq!(
            json_put(
                &maintainer_app,
                &format!("/api/projects/{project_id}"),
                serde_json::json!({"name": "Nope"})
            )
                .await
                .status(),
            StatusCode::FORBIDDEN
        );

        let lead_app = app_as_user(db, &lead);
        assert_eq!(
            json_put(
                &lead_app,
                &format!("/api/projects/{project_id}"),
                serde_json::json!({"name": "Renamed"})
            )
                .await
                .status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn project_delete_maintainer_denied_lead_allowed_when_enforced() {
        // Design decision #6: deletion loosens Admin -> Lead once enforced.
        let (db, _admin, lead, maintainer, _viewer, _non_member, project_id) =
            setup_membership_test();

        let maintainer_app = app_as_user(db.clone(), &maintainer);
        assert_eq!(
            json_delete(&maintainer_app, &format!("/api/projects/{project_id}"))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );

        let lead_app = app_as_user(db, &lead);
        assert_eq!(
            json_delete(&lead_app, &format!("/api/projects/{project_id}"))
                .await
                .status(),
            StatusCode::OK
        );
    }

    // ── Cross-project relations: role required on BOTH sides ─────

    #[tokio::test]
    async fn relation_link_requires_maintainer_on_both_projects() {
        let (db, _admin, lead, maintainer, _viewer, _non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db.clone(), &lead);
        let issue_a = parse_json(
            json_post(
                &lead_app,
                "/api/issues",
                serde_json::json!({"project_id": project_id, "title": "A"}),
            )
            .await,
        )
        .await;

        let other_project_id = {
            let conn = db.write().unwrap();
            crate::db::queries::create_project(
                &conn,
                &CreateProject {
                    name: "Other".into(),
                    identifier: "OTH".into(),
                    description: String::new(),
                    emoji: None,
                    lead_user_id: Some(lead.id),
                },
            )
            .unwrap()
            .id
        };
        let issue_b = parse_json(
            json_post(
                &lead_app,
                "/api/issues",
                serde_json::json!({"project_id": other_project_id, "title": "B"}),
            )
            .await,
        )
        .await;

        let maintainer_app = app_as_user(db.clone(), &maintainer);
        let link_body = serde_json::json!({
            "source": issue_a["identifier"], "target": issue_b["identifier"], "relation_type": "relates_to"
        });
        assert_eq!(
            json_post(&maintainer_app, "/api/issues/link", link_body.clone())
                .await
                .status(),
            StatusCode::FORBIDDEN,
            "maintainer has no role on the target's project"
        );

        {
            let conn = db.write().unwrap();
            crate::db::queries::members::upsert_member(
                &conn,
                other_project_id,
                maintainer.id,
                Role::Maintainer,
            )
                .unwrap();
        }
        assert_eq!(
            json_post(&maintainer_app, "/api/issues/link", link_body)
                .await
                .status(),
            StatusCode::OK,
            "maintainer now has Maintainer on both sides"
        );
    }

    // ── Workspace-level (project-less) pages: admin-only ─────────

    #[tokio::test]
    async fn workspace_page_mutation_requires_admin() {
        let (db, admin, _lead, maintainer, _viewer, _non_member, _project_id) =
            setup_membership_test();

        let maintainer_app = app_as_user(db.clone(), &maintainer);
        assert_eq!(
            json_post(
                &maintainer_app,
                "/api/pages",
                serde_json::json!({"title": "Workspace doc"})
            )
                .await
                .status(),
            StatusCode::FORBIDDEN
        );

        let admin_app = app_as_user(db, &admin);
        assert_eq!(
            json_post(
                &admin_app,
                "/api/pages",
                serde_json::json!({"title": "Workspace doc"})
            )
            .await
            .status(),
            StatusCode::OK
        );
    }

    // ── Admin override: non-member admin reads/writes/manages members ──

    #[tokio::test]
    async fn admin_non_member_can_read_write_and_manage_members_across_the_board() {
        // LIF-201 gap: authz.rs's `enforced_admin_non_member_allowed_all_levels`
        // exercises the require_role primitive directly; this spot-checks the
        // same guarantee through the actual REST handlers the primitive
        // gates, on a project the admin holds no membership row on at all.
        let (db, admin, lead, _maintainer, _viewer, non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db.clone(), &lead);
        let issue = parse_json(
            json_post(
                &lead_app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id, "title": "Admin spot-check" }),
            )
            .await,
        )
        .await;
        let issue_id = issue["id"].as_i64().unwrap();

        let admin_app = app_as_user(db.clone(), &admin);

        // Read: project + issue, despite no membership row for admin.
        assert_eq!(
            json_get(&admin_app, &format!("/api/projects/{project_id}"))
                .await
                .status(),
            StatusCode::OK,
            "admin must read a project they're not a member of"
        );
        assert_eq!(
            json_get(&admin_app, &format!("/api/issues/{issue_id}"))
                .await
                .status(),
            StatusCode::OK
        );

        // Write: create + update.
        assert_eq!(
            json_post(
                &admin_app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id, "title": "by admin" })
            )
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            json_put(
                &admin_app,
                &format!("/api/issues/{issue_id}"),
                serde_json::json!({"title": "renamed by admin"})
            )
                .await
                .status(),
            StatusCode::OK
        );

        // Manage members: admin adds a member despite not being one itself
        // (also covered at the members-endpoint level by
        // `admin_can_manage_members_of_a_project_they_are_not_in` in
        // api/members.rs — kept here too as the "everywhere" spot-check).
        assert_eq!(
            json_post(
                &admin_app,
                &format!("/api/projects/{project_id}/members"),
                serde_json::json!({ "user_id": non_member.id, "role": "viewer" }),
            )
            .await
            .status(),
            StatusCode::OK
        );
    }

    // ── Token-backed lockout regression (the epic's landmine) ─────
    //
    // LIF-DOC-7 decision #9 / LIF-204: OAuth tokens resolve to a real
    // `AuthUser` via `require_api_key` (proven in isolation by
    // `auth.rs`'s `oauth_token_rest_request_resolves_to_correct_auth_user`).
    // Every test above this point proves the *role* matrix using
    // `app_as_user`, which injects `Extension<Option<AuthUser>>` directly —
    // it never exercises the real bearer-token → middleware → handler path.
    // This test closes that gap at the layer closest to production: the
    // actual `api::router` wrapped in the actual `require_api_key`
    // middleware (mirroring `main.rs`'s `authed_routes`), fed a real OAuth
    // bearer token. Flag ON: a token bound to a project member (maintainer)
    // must succeed on read AND write — the specific "member gets bricked by
    // default-deny" failure mode the design doc calls the lockout landmine.
    // A token bound to a non-member must still be denied on both.

    #[tokio::test]
    async fn oauth_token_backed_member_succeeds_non_member_denied_when_enforced() {
        use axum::http::{Request, StatusCode as SC};
        use rusqlite::params;
        use sha2::{Digest, Sha256};
        use tower::ServiceExt;

        let (db, _admin, _lead, maintainer, _viewer, non_member, project_id) =
            setup_membership_test();

        let issue_id = {
            let conn = db.write().unwrap();
            crate::db::queries::create_issue(
                &conn,
                &crate::db::models::CreateIssue {
                    project_id,
                    title: "Token-guarded".into(),
                    description: String::new(),
                    status: "todo".into(),
                    priority: "medium".into(),
                    module_id: None,
                    start_date: None,
                    target_date: None,
                    labels: vec![],
                    source: None,
                },
            )
            .unwrap()
            .id
        };

        fn insert_oauth_token(db: &crate::db::DbPool, suffix: &str, user_id: i64) -> String {
            let token = format!("lific_at_test-{suffix}");
            let hash: String = Sha256::digest(token.as_bytes())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let expires = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
            let client_id = format!("client-{suffix}");
            let conn = db.write().unwrap();
            conn.execute(
                "INSERT INTO oauth_clients (client_id, client_name, redirect_uris) VALUES (?1, 'Test', '[\"http://localhost\"]')",
                params![client_id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO oauth_tokens (access_token, client_id, expires_at, scope, user_id) VALUES (?1, ?2, ?3, 'mcp', ?4)",
                params![hash, client_id, expires, user_id],
            )
            .unwrap();
            token
        }

        let member_token = insert_oauth_token(&db, "member", maintainer.id);
        let outsider_token = insert_oauth_token(&db, "outsider", non_member.id);

        let auth_state = crate::auth::AuthState {
            db: db.clone(),
            manager: crate::auth::create_key_manager().unwrap(),
            public_url: "https://example.com".into(),
            required: true,
        };
        // The real request path: api::router behind the real require_api_key
        // middleware — not the app_as_user() Extension-injection shortcut.
        let app = crate::api::router(db.clone(), &[])
            .layer(axum::Extension(crate::realtime::RealtimeHub::new()))
            .layer(axum::Extension(crate::config::AuthConfig {
                allow_signup: true,
                required: true,
                secure_cookies: false,
            }))
            .layer(axum::middleware::from_fn_with_state(
                auth_state,
                crate::auth::require_api_key,
            ));

        async fn get_with_token(app: axum::Router, uri: String, token: &str) -> SC {
            app.oneshot(
                Request::builder()
                    .uri(uri)
                    .header("authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
        }

        async fn post_with_token(
            app: axum::Router,
            uri: String,
            body: serde_json::Value,
            token: &str,
        ) -> SC {
            app.oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
        }

        // Token-backed MEMBER succeeds on read + write.
        assert_eq!(
            get_with_token(
                app.clone(),
                format!("/api/issues/{issue_id}"),
                &member_token
            )
            .await,
            SC::OK,
            "token-backed member must be able to read"
        );
        assert_eq!(
            post_with_token(
                app.clone(),
                "/api/issues".into(),
                serde_json::json!({ "project_id": project_id, "title": "by token member" }),
                &member_token
            )
            .await,
            SC::OK,
            "token-backed member must be able to write"
        );

        // Token-backed NON-MEMBER is denied on both.
        assert_eq!(
            get_with_token(
                app.clone(),
                format!("/api/issues/{issue_id}"),
                &outsider_token
            )
            .await,
            SC::FORBIDDEN,
            "token-backed non-member must be denied on read"
        );
        assert_eq!(
            post_with_token(
                app.clone(),
                "/api/issues".into(),
                serde_json::json!({ "project_id": project_id, "title": "by token outsider" }),
                &outsider_token
            )
            .await,
            SC::FORBIDDEN,
            "token-backed non-member must be denied on write"
        );
    }

    // ── Flag OFF: byte-for-byte regression proof ──────────────────

    #[tokio::test]
    async fn flag_off_preserves_legacy_behavior() {
        // setup_lead_test seeds a DB with authz_enforced left at its default
        // (off) — a random authenticated non-member.
        let (db, admin, lead, regular, project_id) = setup_lead_test();
        let random_app = app_as_user(db.clone(), &regular);

        // Reads + content mutation stay open to any authenticated user.
        assert_eq!(
            json_get(&random_app, &format!("/api/projects/{project_id}"))
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            json_post(
                &random_app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id, "title": "Legacy open" })
            )
            .await
            .status(),
            StatusCode::OK,
            "content mutation must stay open when the flag is off"
        );

        // Structure endpoints stay lead-gated (not loosened to Maintainer).
        assert_eq!(
            json_post(
                &random_app,
                "/api/modules",
                serde_json::json!({"project_id": project_id, "name": "Nope"})
            )
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        let lead_app = app_as_user(db.clone(), &lead);
        assert_eq!(
            json_post(
                &lead_app,
                "/api/modules",
                serde_json::json!({"project_id": project_id, "name": "Yes"})
            )
                .await
                .status(),
            StatusCode::OK
        );

        // Project delete stays admin-only (lead denied, matching pre-LIF-194).
        assert_eq!(
            json_delete(&lead_app, &format!("/api/projects/{project_id}"))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        let admin_app = app_as_user(db, &admin);
        assert_eq!(
            json_delete(&admin_app, &format!("/api/projects/{project_id}"))
                .await
                .status(),
            StatusCode::OK
        );
    }

    // ── Runtime toggle via PATCH /api/instance/settings ───────────

    #[tokio::test]
    async fn toggling_authz_enforced_via_patch_changes_behavior_on_next_request() {
        let (db, admin, _lead, regular, project_id) = setup_lead_test();
        let admin_app = app_as_user(db.clone(), &admin);
        let regular_app = app_as_user(db.clone(), &regular);

        // Flag off (default): a non-member can read the project freely.
        assert_eq!(
            json_get(&regular_app, &format!("/api/projects/{project_id}"))
                .await
                .status(),
            StatusCode::OK
        );

        let resp = json_patch(
            &admin_app,
            "/api/instance/settings",
            serde_json::json!({"authz_enforced": true}),
        )
            .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(parse_json(resp).await["authz_enforced"], true);

        // Same connection, next request: the non-member is now denied — no
        // restart required (authz::authz_enforced reads the row live).
        assert_eq!(
            json_get(&regular_app, &format!("/api/projects/{project_id}"))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }

    fn websocket_session() -> (crate::db::DbPool, String) {
        let db = crate::db::open_memory().expect("test db");
        let token = {
            let conn = db.write().unwrap();
            let user = crate::db::queries::users::create_user(
                &conn,
                &crate::db::models::CreateUser {
                    username: "ws-user".into(),
                    email: "ws@test.local".into(),
                    password: "testpassword1".into(),
                    display_name: None,
                    is_admin: false,
                    is_bot: false,
                },
            )
            .unwrap();
            crate::db::queries::users::create_session(&conn, user.id, None)
                .unwrap()
                .token
        };
        (db, token)
    }

    #[test]
    fn websocket_session_token_reads_cookie_only() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer lific_sess_bearer".parse().unwrap(),
        );
        headers.insert(
            axum::http::header::COOKIE,
            "theme=dark; lific_token=lific_sess_cookie; other=1"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            super::websocket_session_token(&headers),
            Some("lific_sess_cookie")
        );
    }

    #[test]
    fn websocket_session_token_rejects_empty_credentials() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(axum::http::header::COOKIE, "lific_token=".parse().unwrap());
        assert_eq!(super::websocket_session_token(&headers), None);

        headers.insert(
            axum::http::header::COOKIE,
            "lific_token=   ".parse().unwrap(),
        );
        assert_eq!(super::websocket_session_token(&headers), None);
    }

    #[test]
    fn websocket_session_token_rejects_non_session_credentials() {
        for value in ["lific_sk_live_x", "lific_at_x", "garbage"] {
            let mut headers = axum::http::HeaderMap::new();
            headers.insert(
                axum::http::header::COOKIE,
                format!("lific_token={value}").parse().unwrap(),
            );
            assert_eq!(
                super::websocket_session_token(&headers),
                None,
                "websocket cookie auth must only accept session tokens"
            );
        }
    }

    #[tokio::test]
    async fn websocket_rejects_missing_or_invalid_session_cookie() {
        let (db, _) = websocket_session();

        let missing = axum::http::HeaderMap::new();
        assert!(matches!(
            super::validate_websocket_request(&db, &missing, &[]),
            Err(crate::error::LificError::Forbidden(_))
        ));

        let mut invalid = axum::http::HeaderMap::new();
        invalid.insert(
            axum::http::header::COOKIE,
            "lific_token=lific_sess_fake".parse().unwrap(),
        );
        assert!(matches!(
            super::validate_websocket_request(&db, &invalid, &[]),
            Err(crate::error::LificError::Forbidden(_))
        ));
    }

    #[tokio::test]
    async fn websocket_accepts_valid_session_cookie_before_upgrade() {
        let (db, token) = websocket_session();
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            format!("lific_token={token}").parse().unwrap(),
        );

        super::validate_websocket_request(&db, &headers, &[]).unwrap();
    }

    #[tokio::test]
    async fn websocket_origin_policy_uses_configured_origins() {
        let origins = vec!["https://app.example.test".to_string()];
        let (db, token) = websocket_session();

        let mut rejected = axum::http::HeaderMap::new();
        rejected.insert(
            axum::http::header::ORIGIN,
            "https://evil.example.test".parse().unwrap(),
        );
        rejected.insert(
            axum::http::header::COOKIE,
            format!("lific_token={token}").parse().unwrap(),
        );
        assert!(matches!(
            super::validate_websocket_request(&db, &rejected, &origins),
            Err(crate::error::LificError::Forbidden(_))
        ));

        let mut accepted = axum::http::HeaderMap::new();
        accepted.insert(
            axum::http::header::ORIGIN,
            "https://app.example.test".parse().unwrap(),
        );
        accepted.insert(
            axum::http::header::COOKIE,
            format!("lific_token={token}").parse().unwrap(),
        );
        super::validate_websocket_request(&db, &accepted, &origins).unwrap();
    }

    #[tokio::test]
    async fn websocket_origin_policy_rejects_cross_site_by_default() {
        let (db, token) = websocket_session();

        let mut rejected = axum::http::HeaderMap::new();
        rejected.insert(
            axum::http::header::ORIGIN,
            "https://evil.example.test".parse().unwrap(),
        );
        rejected.insert(
            axum::http::header::HOST,
            "app.example.test".parse().unwrap(),
        );
        rejected.insert(
            axum::http::header::COOKIE,
            format!("lific_token={token}").parse().unwrap(),
        );
        assert!(matches!(
            super::validate_websocket_request(&db, &rejected, &[]),
            Err(crate::error::LificError::Forbidden(_))
        ));

        let mut malformed = axum::http::HeaderMap::new();
        malformed.insert(
            axum::http::header::ORIGIN,
            axum::http::HeaderValue::from_bytes(b"\xff").unwrap(),
        );
        malformed.insert(
            axum::http::header::COOKIE,
            format!("lific_token={token}").parse().unwrap(),
        );
        assert!(matches!(
            super::validate_websocket_request(&db, &malformed, &[]),
            Err(crate::error::LificError::Forbidden(_))
        ));

        let mut same_origin = axum::http::HeaderMap::new();
        same_origin.insert(
            axum::http::header::ORIGIN,
            "https://app.example.test".parse().unwrap(),
        );
        same_origin.insert(
            axum::http::header::HOST,
            "app.example.test".parse().unwrap(),
        );
        same_origin.insert("x-forwarded-proto", "https".parse().unwrap());
        same_origin.insert(
            axum::http::header::COOKIE,
            format!("lific_token={token}").parse().unwrap(),
        );
        super::validate_websocket_request(&db, &same_origin, &[]).unwrap();

        same_origin.insert(
            axum::http::header::ORIGIN,
            "https://app.example.test".parse().unwrap(),
        );
        same_origin.insert(
            axum::http::header::HOST,
            "app.example.test:443".parse().unwrap(),
        );
        super::validate_websocket_request(&db, &same_origin, &[]).unwrap();

        same_origin.insert(
            axum::http::header::ORIGIN,
            "http://app.example.test:80".parse().unwrap(),
        );
        same_origin.insert(
            axum::http::header::HOST,
            "app.example.test".parse().unwrap(),
        );
        same_origin.insert("x-forwarded-proto", "http".parse().unwrap());
        super::validate_websocket_request(&db, &same_origin, &[]).unwrap();
    }
}
