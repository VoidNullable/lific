use axum::{
    Extension,
    extract::{Json, Path, Query, State},
};

use crate::authz;
use crate::db::{DbPool, models::*};
use crate::error::LificError;
use crate::realtime::{RealtimeEvent, RealtimeHub};

use super::{require_structure_role, with_read, with_write};

// ── Module endpoints ─────────────────────────────────────────

#[derive(serde::Deserialize)]
pub(super) struct ModuleQuery {
    project_id: i64,
}

pub(super) async fn list_modules(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Query(q): Query<ModuleQuery>,
) -> Result<Json<Vec<Module>>, LificError> {
    authz::require_role(&db, &auth_user, q.project_id, Role::Viewer)?;
    with_read(&db, |conn| {
        crate::db::queries::list_modules(conn, q.project_id)
    })
    .map(Json)
}

pub(super) async fn get_module(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path(id): Path<i64>,
) -> Result<Json<Module>, LificError> {
    let module = with_read(&db, |conn| crate::db::queries::get_module(conn, id))?;
    authz::require_role(&db, &auth_user, module.project_id, Role::Viewer)?;
    Ok(Json(module))
}

pub(super) async fn create_module(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<CreateModule>,
) -> Result<Json<Module>, LificError> {
    require_structure_role(&db, &auth_user, input.project_id)?;
    let module = with_write(&db, |conn| crate::db::queries::create_module(conn, &input))?;
    realtime.send(RealtimeEvent::ProjectUpdated { project_id: module.project_id });
    Ok(Json(module))
}

pub(super) async fn update_module(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Path(id): Path<i64>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<UpdateModule>,
) -> Result<Json<Module>, LificError> {
    let project_id = with_read(&db, |conn| {
        crate::db::queries::get_resource_project_id(conn, "modules", id)
    })?;
    require_structure_role(&db, &auth_user, project_id)?;
    let module = with_write(&db, |conn| {
        crate::db::queries::update_module(conn, id, &input)
    })?;
    realtime.send(RealtimeEvent::ProjectUpdated { project_id });
    Ok(Json(module))
}

pub(super) async fn delete_module_handler(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Path(id): Path<i64>,
    Extension(auth_user): Extension<Option<AuthUser>>,
) -> Result<Json<serde_json::Value>, LificError> {
    let project_id = with_read(&db, |conn| {
        crate::db::queries::get_resource_project_id(conn, "modules", id)
    })?;
    require_structure_role(&db, &auth_user, project_id)?;
    with_write(&db, |conn| crate::db::queries::delete_module(conn, id))?;
    realtime.send(RealtimeEvent::ProjectUpdated { project_id });
    Ok(Json(serde_json::json!({"deleted": true})))
}

// ── Label endpoints ──────────────────────────────────────────

#[derive(serde::Deserialize)]
pub(super) struct LabelQuery {
    project_id: i64,
}

pub(super) async fn list_labels(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Query(q): Query<LabelQuery>,
) -> Result<Json<Vec<Label>>, LificError> {
    authz::require_role(&db, &auth_user, q.project_id, Role::Viewer)?;
    with_read(&db, |conn| {
        crate::db::queries::list_labels(conn, q.project_id)
    })
    .map(Json)
}

pub(super) async fn create_label(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<CreateLabel>,
) -> Result<Json<Label>, LificError> {
    require_structure_role(&db, &auth_user, input.project_id)?;
    let label = with_write(&db, |conn| crate::db::queries::create_label(conn, &input))?;
    realtime.send(RealtimeEvent::ProjectUpdated { project_id: input.project_id });
    Ok(Json(label))
}

pub(super) async fn update_label_handler(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Path(id): Path<i64>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<UpdateLabel>,
) -> Result<Json<Label>, LificError> {
    let project_id = with_read(&db, |conn| {
        crate::db::queries::get_resource_project_id(conn, "labels", id)
    })?;
    require_structure_role(&db, &auth_user, project_id)?;
    let label = with_write(&db, |conn| {
        crate::db::queries::update_label(conn, id, &input)
    })?;
    realtime.send(RealtimeEvent::ProjectUpdated { project_id });
    Ok(Json(label))
}

pub(super) async fn delete_label_handler(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Path(id): Path<i64>,
    Extension(auth_user): Extension<Option<AuthUser>>,
) -> Result<Json<serde_json::Value>, LificError> {
    let project_id = with_read(&db, |conn| {
        crate::db::queries::get_resource_project_id(conn, "labels", id)
    })?;
    require_structure_role(&db, &auth_user, project_id)?;
    with_write(&db, |conn| crate::db::queries::delete_label(conn, id))?;
    realtime.send(RealtimeEvent::ProjectUpdated { project_id });
    Ok(Json(serde_json::json!({"deleted": true})))
}

#[derive(serde::Deserialize)]
pub(super) struct MergeLabel {
    /// Target label id the source is folded into.
    into: i64,
}

pub(super) async fn merge_label_handler(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
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
    require_structure_role(&db, &auth_user, source_project)?;
    let label = with_write(&db, |conn| {
        crate::db::queries::merge_label(conn, id, input.into)
    })?;
    realtime.send(RealtimeEvent::ProjectUpdated { project_id: source_project });
    Ok(Json(label))
}

// ── Folder endpoints ─────────────────────────────────────────

#[derive(serde::Deserialize)]
pub(super) struct FolderQuery {
    project_id: i64,
}

pub(super) async fn list_folders_handler(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Query(q): Query<FolderQuery>,
) -> Result<Json<Vec<Folder>>, LificError> {
    authz::require_role(&db, &auth_user, q.project_id, Role::Viewer)?;
    with_read(&db, |conn| {
        crate::db::queries::list_folders(conn, q.project_id)
    })
    .map(Json)
}

pub(super) async fn create_folder(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<CreateFolder>,
) -> Result<Json<Folder>, LificError> {
    require_structure_role(&db, &auth_user, input.project_id)?;
    let folder = with_write(&db, |conn| crate::db::queries::create_folder(conn, &input))?;
    realtime.send(RealtimeEvent::ProjectUpdated { project_id: input.project_id });
    Ok(Json(folder))
}

pub(super) async fn delete_folder_handler(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Path(id): Path<i64>,
    Extension(auth_user): Extension<Option<AuthUser>>,
) -> Result<Json<serde_json::Value>, LificError> {
    let project_id = with_read(&db, |conn| {
        crate::db::queries::get_resource_project_id(conn, "folders", id)
    })?;
    require_structure_role(&db, &auth_user, project_id)?;
    with_write(&db, |conn| crate::db::queries::delete_folder(conn, id))?;
    realtime.send(RealtimeEvent::ProjectUpdated { project_id });
    Ok(Json(serde_json::json!({"deleted": true})))
}

pub(super) async fn update_folder(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Path(id): Path<i64>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<UpdateFolder>,
) -> Result<Json<Folder>, LificError> {
    let project_id = with_read(&db, |conn| {
        crate::db::queries::get_resource_project_id(conn, "folders", id)
    })?;
    require_structure_role(&db, &auth_user, project_id)?;
    let folder = with_write(&db, |conn| {
        crate::db::queries::update_folder(conn, id, &input)
    })?;
    realtime.send(RealtimeEvent::ProjectUpdated { project_id });
    Ok(Json(folder))
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
    async fn lead_can_update_folder_and_regular_cannot() {
        let (db, _, lead, regular, project_id) = setup_lead_test();
        let lead_app = app_as_user(db.clone(), &lead);
        let created = parse_json(
            json_post(
                &lead_app,
                "/api/folders",
                serde_json::json!({ "project_id": project_id, "name": "Docs" }),
            )
            .await,
        )
        .await;
        let folder_id = created["id"].as_i64().unwrap();

        let updated = parse_json(
            json_put(
                &lead_app,
                &format!("/api/folders/{folder_id}"),
                serde_json::json!({ "name": "Documentation" }),
            )
            .await,
        )
        .await;
        assert_eq!(updated["name"], "Documentation");

        let regular_app = app_as_user(db, &regular);
        let response = json_put(
            &regular_app,
            &format!("/api/folders/{folder_id}"),
            serde_json::json!({ "name": "Nope" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn folder_update_allows_maintainer_and_denies_viewer_and_non_member_when_enforced() {
        let (db, _admin, _lead, maintainer, viewer, non_member, project_id) =
            setup_membership_test();
        let maintainer_app = app_as_user(db.clone(), &maintainer);
        let folder = parse_json(
            json_post(
                &maintainer_app,
                "/api/folders",
                serde_json::json!({ "project_id": project_id, "name": "Docs" }),
            )
            .await,
        )
        .await;
        let folder_id = folder["id"].as_i64().unwrap();

        let response = json_put(
            &maintainer_app,
            &format!("/api/folders/{folder_id}"),
            serde_json::json!({ "name": "Documentation" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(parse_json(response).await["name"], "Documentation");

        let viewer_app = app_as_user(db.clone(), &viewer);
        let response = json_put(
            &viewer_app,
            &format!("/api/folders/{folder_id}"),
            serde_json::json!({ "name": "Nope" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let non_member_app = app_as_user(db, &non_member);
        let response = json_put(
            &non_member_app,
            &format!("/api/folders/{folder_id}"),
            serde_json::json!({ "name": "Still nope" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn regular_cannot_create_folder() {
        let (db, _, _lead, regular, project_id) = setup_lead_test();
        let regular_app = app_as_user(db, &regular);
        let response = json_post(
            &regular_app,
            "/api/folders",
            serde_json::json!({ "project_id": project_id, "name": "Docs" }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn lead_can_merge_labels_and_regular_cannot() {
        let (db, _, lead, regular, project_id) = setup_lead_test();
        let lead_app = app_as_user(db.clone(), &lead);

        let mk = |name: &str| serde_json::json!({ "project_id": project_id, "name": name, "color": "#FF0000" });
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

        let list =
            parse_json(json_get(&lead_app, &format!("/api/labels?project_id={project_id}")).await)
                .await;
        let names: Vec<&str> = list
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["defect"]);
    }
}
