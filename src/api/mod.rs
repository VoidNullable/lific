use axum::{
    Router,
    extract::{Json, Path, Query, State},
    routing::{delete, get, post, put},
};
use tower_http::cors::CorsLayer;

use crate::db::{DbPool, models::*, queries};
use crate::error::LificError;

/// Build the full API router.
pub fn router(db: DbPool) -> Router {
    Router::new()
        // Projects
        .route("/api/projects", get(list_projects).post(create_project))
        .route(
            "/api/projects/{id}",
            get(get_project)
                .put(update_project)
                .delete(delete_project_handler),
        )
        // Issues
        .route("/api/issues", get(list_issues).post(create_issue))
        .route(
            "/api/issues/{id}",
            get(get_issue)
                .put(update_issue)
                .delete(delete_issue_handler),
        )
        .route("/api/issues/resolve/{identifier}", get(resolve_issue))
        // Issue relations
        .route("/api/issues/link", post(link_issues))
        .route("/api/issues/unlink", post(unlink_issues))
        // Modules
        .route("/api/modules", get(list_modules).post(create_module))
        .route(
            "/api/modules/{id}",
            put(update_module).delete(delete_module_handler),
        )
        // Labels
        .route("/api/labels", get(list_labels).post(create_label))
        .route("/api/labels/{id}", delete(delete_label_handler))
        // Pages
        .route("/api/pages", get(list_pages_handler).post(create_page))
        .route(
            "/api/pages/{id}",
            get(get_page).put(update_page).delete(delete_page_handler),
        )
        // Folders
        .route(
            "/api/folders",
            get(list_folders_handler).post(create_folder),
        )
        .route("/api/folders/{id}", delete(delete_folder_handler))
        // Search
        .route("/api/search", get(search))
        // Board view
        .route("/api/projects/{id}/board", get(get_board))
        // Health
        .route("/api/health", get(health))
        .layer(CorsLayer::permissive())
        .with_state(db)
}

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

async fn health() -> &'static str {
    "ok"
}

async fn list_projects(State(db): State<DbPool>) -> Result<Json<Vec<Project>>, LificError> {
    with_read(&db, queries::list_projects).map(Json)
}

async fn get_project(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
) -> Result<Json<Project>, LificError> {
    with_read(&db, |conn| queries::get_project(conn, id)).map(Json)
}

async fn create_project(
    State(db): State<DbPool>,
    Json(input): Json<CreateProject>,
) -> Result<Json<Project>, LificError> {
    with_write(&db, |conn| queries::create_project(conn, &input)).map(Json)
}

async fn update_project(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Json(input): Json<UpdateProject>,
) -> Result<Json<Project>, LificError> {
    with_write(&db, |conn| queries::update_project(conn, id, &input)).map(Json)
}

async fn delete_project_handler(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, LificError> {
    with_write(&db, |conn| queries::delete_project(conn, id))?;
    Ok(Json(serde_json::json!({"deleted": true})))
}

async fn list_issues(
    State(db): State<DbPool>,
    Query(q): Query<ListIssuesQuery>,
) -> Result<Json<Vec<Issue>>, LificError> {
    with_read(&db, |conn| queries::list_issues(conn, &q)).map(Json)
}

async fn get_issue(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
) -> Result<Json<Issue>, LificError> {
    with_read(&db, |conn| queries::get_issue(conn, id)).map(Json)
}

async fn resolve_issue(
    State(db): State<DbPool>,
    Path(identifier): Path<String>,
) -> Result<Json<Issue>, LificError> {
    with_read(&db, |conn| {
        let id = queries::resolve_identifier(conn, &identifier)?;
        queries::get_issue(conn, id)
    })
    .map(Json)
}

async fn create_issue(
    State(db): State<DbPool>,
    Json(input): Json<CreateIssue>,
) -> Result<Json<Issue>, LificError> {
    with_write(&db, |conn| queries::create_issue(conn, &input)).map(Json)
}

async fn update_issue(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Json(input): Json<UpdateIssue>,
) -> Result<Json<Issue>, LificError> {
    with_write(&db, |conn| queries::update_issue(conn, id, &input)).map(Json)
}

async fn delete_issue_handler(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, LificError> {
    with_write(&db, |conn| queries::delete_issue(conn, id))?;
    Ok(Json(serde_json::json!({"deleted": true})))
}

#[derive(serde::Deserialize)]
struct LinkRequest {
    source: String,
    target: String,
    relation_type: String,
}

#[derive(serde::Deserialize)]
struct UnlinkRequest {
    source: String,
    target: String,
}

async fn link_issues(
    State(db): State<DbPool>,
    Json(input): Json<LinkRequest>,
) -> Result<Json<serde_json::Value>, LificError> {
    with_write(&db, |conn| {
        let source_id = queries::resolve_identifier(conn, &input.source)?;
        let target_id = queries::resolve_identifier(conn, &input.target)?;
        queries::link_issues(conn, source_id, target_id, &input.relation_type)
    })?;
    Ok(Json(serde_json::json!({"linked": true})))
}

async fn unlink_issues(
    State(db): State<DbPool>,
    Json(input): Json<UnlinkRequest>,
) -> Result<Json<serde_json::Value>, LificError> {
    with_write(&db, |conn| {
        let source_id = queries::resolve_identifier(conn, &input.source)?;
        let target_id = queries::resolve_identifier(conn, &input.target)?;
        queries::unlink_issues(conn, source_id, target_id)
    })?;
    Ok(Json(serde_json::json!({"unlinked": true})))
}

#[derive(serde::Deserialize)]
struct ModuleQuery {
    project_id: i64,
}

async fn list_modules(
    State(db): State<DbPool>,
    Query(q): Query<ModuleQuery>,
) -> Result<Json<Vec<Module>>, LificError> {
    with_read(&db, |conn| queries::list_modules(conn, q.project_id)).map(Json)
}

async fn create_module(
    State(db): State<DbPool>,
    Json(input): Json<CreateModule>,
) -> Result<Json<Module>, LificError> {
    with_write(&db, |conn| queries::create_module(conn, &input)).map(Json)
}

async fn update_module(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Json(input): Json<UpdateModule>,
) -> Result<Json<Module>, LificError> {
    with_write(&db, |conn| queries::update_module(conn, id, &input)).map(Json)
}

async fn delete_module_handler(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, LificError> {
    with_write(&db, |conn| queries::delete_module(conn, id))?;
    Ok(Json(serde_json::json!({"deleted": true})))
}

#[derive(serde::Deserialize)]
struct LabelQuery {
    project_id: i64,
}

async fn list_labels(
    State(db): State<DbPool>,
    Query(q): Query<LabelQuery>,
) -> Result<Json<Vec<Label>>, LificError> {
    with_read(&db, |conn| queries::list_labels(conn, q.project_id)).map(Json)
}

async fn create_label(
    State(db): State<DbPool>,
    Json(input): Json<CreateLabel>,
) -> Result<Json<Label>, LificError> {
    with_write(&db, |conn| queries::create_label(conn, &input)).map(Json)
}

async fn delete_label_handler(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, LificError> {
    with_write(&db, |conn| queries::delete_label(conn, id))?;
    Ok(Json(serde_json::json!({"deleted": true})))
}

#[derive(serde::Deserialize)]
struct PageQuery {
    project_id: Option<i64>,
    folder_id: Option<i64>,
}

async fn list_pages_handler(
    State(db): State<DbPool>,
    Query(q): Query<PageQuery>,
) -> Result<Json<Vec<Page>>, LificError> {
    with_read(&db, |conn| {
        queries::list_pages(conn, q.project_id, q.folder_id)
    })
    .map(Json)
}

async fn get_page(State(db): State<DbPool>, Path(id): Path<i64>) -> Result<Json<Page>, LificError> {
    with_read(&db, |conn| queries::get_page(conn, id)).map(Json)
}

async fn create_page(
    State(db): State<DbPool>,
    Json(input): Json<CreatePage>,
) -> Result<Json<Page>, LificError> {
    with_write(&db, |conn| queries::create_page(conn, &input)).map(Json)
}

async fn update_page(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Json(input): Json<UpdatePage>,
) -> Result<Json<Page>, LificError> {
    with_write(&db, |conn| queries::update_page(conn, id, &input)).map(Json)
}

async fn delete_page_handler(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, LificError> {
    with_write(&db, |conn| queries::delete_page(conn, id))?;
    Ok(Json(serde_json::json!({"deleted": true})))
}

#[derive(serde::Deserialize)]
struct FolderQuery {
    project_id: i64,
}

async fn list_folders_handler(
    State(db): State<DbPool>,
    Query(q): Query<FolderQuery>,
) -> Result<Json<Vec<Folder>>, LificError> {
    with_read(&db, |conn| queries::list_folders(conn, q.project_id)).map(Json)
}

async fn create_folder(
    State(db): State<DbPool>,
    Json(input): Json<CreateFolder>,
) -> Result<Json<Folder>, LificError> {
    with_write(&db, |conn| queries::create_folder(conn, &input)).map(Json)
}

async fn delete_folder_handler(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, LificError> {
    with_write(&db, |conn| queries::delete_folder(conn, id))?;
    Ok(Json(serde_json::json!({"deleted": true})))
}

async fn search(
    State(db): State<DbPool>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<Vec<SearchResult>>, LificError> {
    with_read(&db, |conn| queries::search(conn, &q)).map(Json)
}

#[derive(serde::Deserialize)]
struct BoardQuery {
    #[serde(default = "default_group_by")]
    group_by: String,
}

fn default_group_by() -> String {
    "status".to_string()
}

async fn get_board(
    State(db): State<DbPool>,
    Path(project_id): Path<i64>,
    Query(q): Query<BoardQuery>,
) -> Result<Json<serde_json::Value>, LificError> {
    let issues = with_read(&db, |conn| {
        queries::list_issues(
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

    let mut board: std::collections::BTreeMap<String, Vec<&Issue>> =
        std::collections::BTreeMap::new();
    for issue in &issues {
        let key = match q.group_by.as_str() {
            "priority" => issue.priority.clone(),
            "module" => issue
                .module_id
                .map(|_| "has_module".to_string())
                .unwrap_or("unassigned".to_string()),
            _ => issue.status.clone(),
        };
        board.entry(key).or_default().push(issue);
    }

    Ok(Json(serde_json::json!(board)))
}
