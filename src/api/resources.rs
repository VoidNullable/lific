use axum::{
    Extension,
    extract::{Json, Path, Query, State},
};

use crate::db::{DbPool, models::*};
use crate::error::LificError;

use super::{require_project_lead, with_read, with_write};

// ── Module endpoints ─────────────────────────────────────────

#[derive(serde::Deserialize)]
pub(super) struct ModuleQuery {
    project_id: i64,
}

pub(super) async fn list_modules(
    State(db): State<DbPool>,
    Query(q): Query<ModuleQuery>,
) -> Result<Json<Vec<Module>>, LificError> {
    with_read(&db, |conn| {
        crate::db::queries::list_modules(conn, q.project_id)
    })
    .map(Json)
}

pub(super) async fn create_module(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<CreateModule>,
) -> Result<Json<Module>, LificError> {
    require_project_lead(&db, &auth_user, input.project_id)?;
    with_write(&db, |conn| crate::db::queries::create_module(conn, &input)).map(Json)
}

pub(super) async fn update_module(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<UpdateModule>,
) -> Result<Json<Module>, LificError> {
    let project_id = with_read(&db, |conn| {
        crate::db::queries::get_resource_project_id(conn, "modules", id)
    })?;
    require_project_lead(&db, &auth_user, project_id)?;
    with_write(&db, |conn| {
        crate::db::queries::update_module(conn, id, &input)
    })
    .map(Json)
}

pub(super) async fn delete_module_handler(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Extension(auth_user): Extension<Option<AuthUser>>,
) -> Result<Json<serde_json::Value>, LificError> {
    let project_id = with_read(&db, |conn| {
        crate::db::queries::get_resource_project_id(conn, "modules", id)
    })?;
    require_project_lead(&db, &auth_user, project_id)?;
    with_write(&db, |conn| crate::db::queries::delete_module(conn, id))?;
    Ok(Json(serde_json::json!({"deleted": true})))
}

// ── Label endpoints ──────────────────────────────────────────

#[derive(serde::Deserialize)]
pub(super) struct LabelQuery {
    project_id: i64,
}

pub(super) async fn list_labels(
    State(db): State<DbPool>,
    Query(q): Query<LabelQuery>,
) -> Result<Json<Vec<Label>>, LificError> {
    with_read(&db, |conn| {
        crate::db::queries::list_labels(conn, q.project_id)
    })
    .map(Json)
}

pub(super) async fn create_label(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<CreateLabel>,
) -> Result<Json<Label>, LificError> {
    require_project_lead(&db, &auth_user, input.project_id)?;
    with_write(&db, |conn| crate::db::queries::create_label(conn, &input)).map(Json)
}

pub(super) async fn delete_label_handler(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Extension(auth_user): Extension<Option<AuthUser>>,
) -> Result<Json<serde_json::Value>, LificError> {
    let project_id = with_read(&db, |conn| {
        crate::db::queries::get_resource_project_id(conn, "labels", id)
    })?;
    require_project_lead(&db, &auth_user, project_id)?;
    with_write(&db, |conn| crate::db::queries::delete_label(conn, id))?;
    Ok(Json(serde_json::json!({"deleted": true})))
}

// ── Folder endpoints ─────────────────────────────────────────

#[derive(serde::Deserialize)]
pub(super) struct FolderQuery {
    project_id: i64,
}

pub(super) async fn list_folders_handler(
    State(db): State<DbPool>,
    Query(q): Query<FolderQuery>,
) -> Result<Json<Vec<Folder>>, LificError> {
    with_read(&db, |conn| {
        crate::db::queries::list_folders(conn, q.project_id)
    })
    .map(Json)
}

pub(super) async fn create_folder(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<CreateFolder>,
) -> Result<Json<Folder>, LificError> {
    require_project_lead(&db, &auth_user, input.project_id)?;
    with_write(&db, |conn| crate::db::queries::create_folder(conn, &input)).map(Json)
}

pub(super) async fn delete_folder_handler(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Extension(auth_user): Extension<Option<AuthUser>>,
) -> Result<Json<serde_json::Value>, LificError> {
    let project_id = with_read(&db, |conn| {
        crate::db::queries::get_resource_project_id(conn, "folders", id)
    })?;
    require_project_lead(&db, &auth_user, project_id)?;
    with_write(&db, |conn| crate::db::queries::delete_folder(conn, id))?;
    Ok(Json(serde_json::json!({"deleted": true})))
}

#[cfg(test)]
mod tests {
    use crate::api::test_helpers::*;
    use axum::http::StatusCode;

    #[tokio::test]
    async fn lead_can_manage_modules() {
        let (db, _, lead, regular, project_id) = setup_lead_test();

        // Lead can create a module
        let lead_app = app_as_user(db.clone(), &lead);
        let body = serde_json::json!({
            "project_id": project_id,
            "name": "Backend",
            "status": "active"
        });
        let resp = json_post(&lead_app, "/api/modules", body).await;
        assert_eq!(resp.status(), StatusCode::OK);

        // Regular user cannot create a module
        let reg_app = app_as_user(db, &regular);
        let body = serde_json::json!({
            "project_id": project_id,
            "name": "Forbidden Module",
            "status": "active"
        });
        let resp = json_post(&reg_app, "/api/modules", body).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn lead_can_manage_labels() {
        let (db, _, lead, regular, project_id) = setup_lead_test();

        // Lead can create a label
        let lead_app = app_as_user(db.clone(), &lead);
        let body = serde_json::json!({
            "project_id": project_id,
            "name": "bug",
            "color": "#FF0000"
        });
        let resp = json_post(&lead_app, "/api/labels", body).await;
        assert_eq!(resp.status(), StatusCode::OK);

        // Regular user cannot
        let reg_app = app_as_user(db, &regular);
        let body = serde_json::json!({
            "project_id": project_id,
            "name": "forbidden-label",
            "color": "#FF0000"
        });
        let resp = json_post(&reg_app, "/api/labels", body).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
