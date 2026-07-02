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

pub(super) async fn get_module(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
) -> Result<Json<Module>, LificError> {
    with_read(&db, |conn| crate::db::queries::get_module(conn, id)).map(Json)
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

pub(super) async fn update_label_handler(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<UpdateLabel>,
) -> Result<Json<Label>, LificError> {
    let project_id = with_read(&db, |conn| {
        crate::db::queries::get_resource_project_id(conn, "labels", id)
    })?;
    require_project_lead(&db, &auth_user, project_id)?;
    with_write(&db, |conn| crate::db::queries::update_label(conn, id, &input)).map(Json)
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

#[derive(serde::Deserialize)]
pub(super) struct MergeLabel {
    /// Target label id the source is folded into.
    into: i64,
}

pub(super) async fn merge_label_handler(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<MergeLabel>,
) -> Result<Json<Label>, LificError> {
    // Both labels must live in the same project, and the caller must lead it.
    let source_project = with_read(&db, |conn| {
        crate::db::queries::get_resource_project_id(conn, "labels", id)
    })?;
    let target_project = with_read(&db, |conn| {
        crate::db::queries::get_resource_project_id(conn, "labels", input.into)
    })?;
    if source_project != target_project {
        return Err(LificError::BadRequest(
            "cannot merge labels across projects".into(),
        ));
    }
    require_project_lead(&db, &auth_user, source_project)?;
    with_write(&db, |conn| {
        crate::db::queries::merge_label(conn, id, input.into)
    })
    .map(Json)
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

    #[tokio::test]
    async fn lead_can_update_label_and_regular_cannot() {
        let (db, _, lead, regular, project_id) = setup_lead_test();
        let lead_app = app_as_user(db.clone(), &lead);

        // Create a label to mutate.
        let resp = json_post(
            &lead_app,
            "/api/labels",
            serde_json::json!({ "project_id": project_id, "name": "bug", "color": "#FF0000" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let created = parse_json(resp).await;
        let label_id = created["id"].as_i64().unwrap();

        // Lead can rename + recolor it.
        let resp = json_put(
            &lead_app,
            &format!("/api/labels/{label_id}"),
            serde_json::json!({ "name": "defect", "color": "#00FF00" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let updated = parse_json(resp).await;
        assert_eq!(updated["name"], "defect");
        assert_eq!(updated["color"], "#00FF00");

        // Regular user cannot update it.
        let reg_app = app_as_user(db, &regular);
        let resp = json_put(
            &reg_app,
            &format!("/api/labels/{label_id}"),
            serde_json::json!({ "name": "nope" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn lead_can_merge_labels_and_regular_cannot() {
        let (db, _, lead, regular, project_id) = setup_lead_test();
        let lead_app = app_as_user(db.clone(), &lead);

        let mk = |name: &str| {
            serde_json::json!({ "project_id": project_id, "name": name, "color": "#FF0000" })
        };
        let a = parse_json(json_post(&lead_app, "/api/labels", mk("bug")).await).await;
        let b = parse_json(json_post(&lead_app, "/api/labels", mk("defect")).await).await;
        let a_id = a["id"].as_i64().unwrap();
        let b_id = b["id"].as_i64().unwrap();

        // Regular user cannot merge.
        let reg_app = app_as_user(db, &regular);
        let resp = json_post(
            &reg_app,
            &format!("/api/labels/{a_id}/merge"),
            serde_json::json!({ "into": b_id }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        // Lead merges A into B: response is the survivor, and A is gone.
        let resp = json_post(
            &lead_app,
            &format!("/api/labels/{a_id}/merge"),
            serde_json::json!({ "into": b_id }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(parse_json(resp).await["id"].as_i64().unwrap(), b_id);

        let list = parse_json(json_get(&lead_app, &format!("/api/labels?project_id={project_id}")).await).await;
        let names: Vec<&str> = list.as_array().unwrap().iter().map(|l| l["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["defect"]);
    }
}
