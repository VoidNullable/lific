use axum::{
    Extension,
    extract::{Json, Path, Query, State},
};
use rusqlite::Connection;

use crate::authz;
use crate::db::queries::ResourceTable;
use crate::db::{DbPool, models::*};
use crate::error::LificError;
use crate::realtime::{RealtimeEvent, RealtimeHub};

use super::{require_structure_role, with_read, with_write};

// ── Shared structure CRUD ────────────────────────────────────
//
// LIF-382: modules, labels and folders are one resource shape wearing three
// names. Each is a project-scoped row whose list/create/update/delete
// endpoints do the same four things: authorize against the owning project,
// read or write through the pool, and broadcast `ProjectUpdated`. That shape
// lives once, in the generic handlers below; a resource only supplies its
// query functions through `Structure`.
//
// Generic functions rather than a macro, deliberately: every line here stays
// ordinary Rust, so go-to-definition lands on a real `impl` block, types show
// up in hover and inlay hints, and a mistake in one resource is a normal
// trait mismatch pointing at the offending line instead of an error inside a
// macro expansion nobody can step through.

/// One project-scoped structure resource (a module, label, or folder).
pub(super) trait Structure: 'static {
    /// Row type handed back to the client.
    type Model: serde::Serialize + Send + 'static;
    /// Create payload, deserialized from the request body.
    type Create: serde::de::DeserializeOwned + Send + 'static;
    /// Update payload, deserialized from the request body.
    type Update: serde::de::DeserializeOwned + Send + 'static;

    /// Table an existing row lives in, used to resolve its project.
    const TABLE: ResourceTable;

    /// Project a create payload targets.
    fn create_project_id(input: &Self::Create) -> i64;

    fn list(conn: &Connection, project_id: i64) -> Result<Vec<Self::Model>, LificError>;
    fn create(conn: &Connection, input: &Self::Create) -> Result<Self::Model, LificError>;
    fn update(conn: &Connection, id: i64, input: &Self::Update)
    -> Result<Self::Model, LificError>;
    fn delete(conn: &Connection, id: i64) -> Result<(), LificError>;
}

pub(super) struct Modules;

impl Structure for Modules {
    type Model = Module;
    type Create = CreateModule;
    type Update = UpdateModule;

    const TABLE: ResourceTable = ResourceTable::Modules;

    fn create_project_id(input: &CreateModule) -> i64 {
        input.project_id
    }

    fn list(conn: &Connection, project_id: i64) -> Result<Vec<Module>, LificError> {
        crate::db::queries::list_modules(conn, project_id)
    }

    fn create(conn: &Connection, input: &CreateModule) -> Result<Module, LificError> {
        crate::db::queries::create_module(conn, input)
    }

    fn update(conn: &Connection, id: i64, input: &UpdateModule) -> Result<Module, LificError> {
        crate::db::queries::update_module(conn, id, input)
    }

    fn delete(conn: &Connection, id: i64) -> Result<(), LificError> {
        crate::db::queries::delete_module(conn, id)
    }
}

pub(super) struct Labels;

impl Structure for Labels {
    type Model = Label;
    type Create = CreateLabel;
    type Update = UpdateLabel;

    const TABLE: ResourceTable = ResourceTable::Labels;

    fn create_project_id(input: &CreateLabel) -> i64 {
        input.project_id
    }

    fn list(conn: &Connection, project_id: i64) -> Result<Vec<Label>, LificError> {
        crate::db::queries::list_labels(conn, project_id)
    }

    fn create(conn: &Connection, input: &CreateLabel) -> Result<Label, LificError> {
        crate::db::queries::create_label(conn, input)
    }

    fn update(conn: &Connection, id: i64, input: &UpdateLabel) -> Result<Label, LificError> {
        crate::db::queries::update_label(conn, id, input)
    }

    fn delete(conn: &Connection, id: i64) -> Result<(), LificError> {
        crate::db::queries::delete_label(conn, id)
    }
}

pub(super) struct Folders;

impl Structure for Folders {
    type Model = Folder;
    type Create = CreateFolder;
    type Update = UpdateFolder;

    const TABLE: ResourceTable = ResourceTable::Folders;

    fn create_project_id(input: &CreateFolder) -> i64 {
        input.project_id
    }

    fn list(conn: &Connection, project_id: i64) -> Result<Vec<Folder>, LificError> {
        crate::db::queries::list_folders(conn, project_id)
    }

    fn create(conn: &Connection, input: &CreateFolder) -> Result<Folder, LificError> {
        crate::db::queries::create_folder(conn, input)
    }

    fn update(conn: &Connection, id: i64, input: &UpdateFolder) -> Result<Folder, LificError> {
        crate::db::queries::update_folder(conn, id, input)
    }

    fn delete(conn: &Connection, id: i64) -> Result<(), LificError> {
        crate::db::queries::delete_folder(conn, id)
    }
}

/// Every structure listing is scoped to a single project.
#[derive(serde::Deserialize)]
pub(super) struct ProjectQuery {
    project_id: i64,
}

/// Resolve the project owning an existing structure row, and require the
/// caller may edit that project's structure. Returns the project id so the
/// caller can broadcast against it.
fn authorize_existing<R: Structure>(
    db: &DbPool,
    identity: &Option<crate::resolve_caller::ResolvedIdentity>,
    id: i64,
) -> Result<i64, LificError> {
    let project_id = with_read(db, |conn| {
        crate::db::queries::get_resource_project_id(conn, R::TABLE, id)
    })?;
    require_structure_role(db, identity, project_id)?;
    Ok(project_id)
}

pub(super) async fn list_structure<R: Structure>(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Query(q): Query<ProjectQuery>,
) -> Result<Json<Vec<R::Model>>, LificError> {
    authz::require_role(&db, &identity, q.project_id, Role::Viewer)?;
    with_read(&db, |conn| R::list(conn, q.project_id)).map(Json)
}

pub(super) async fn create_structure<R: Structure>(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Json(input): Json<R::Create>,
) -> Result<Json<R::Model>, LificError> {
    let project_id = R::create_project_id(&input);
    require_structure_role(&db, &identity, project_id)?;
    let created = with_write(&db, |conn| R::create(conn, &input))?;
    realtime.send(RealtimeEvent::ProjectUpdated { project_id });
    Ok(Json(created))
}

pub(super) async fn update_structure<R: Structure>(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Path(id): Path<i64>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Json(input): Json<R::Update>,
) -> Result<Json<R::Model>, LificError> {
    let project_id = authorize_existing::<R>(&db, &identity, id)?;
    let updated = with_write(&db, |conn| R::update(conn, id, &input))?;
    realtime.send(RealtimeEvent::ProjectUpdated { project_id });
    Ok(Json(updated))
}

pub(super) async fn delete_structure<R: Structure>(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Path(id): Path<i64>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
) -> Result<Json<serde_json::Value>, LificError> {
    let project_id = authorize_existing::<R>(&db, &identity, id)?;
    with_write(&db, |conn| R::delete(conn, id))?;
    realtime.send(RealtimeEvent::ProjectUpdated { project_id });
    Ok(Json(serde_json::json!({"deleted": true})))
}

// ── Module endpoints ─────────────────────────────────────────

pub(super) async fn get_module(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(id): Path<i64>,
) -> Result<Json<Module>, LificError> {
    let module = with_read(&db, |conn| crate::db::queries::get_module(conn, id))?;
    authz::require_role(&db, &identity, module.project_id, Role::Viewer)?;
    Ok(Json(module))
}

// ── Label endpoints ──────────────────────────────────────────

#[derive(serde::Deserialize)]
pub(super) struct MergeLabel {
    /// Target label id the source is folded into.
    into: i64,
}

pub(super) async fn merge_label_handler(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Path(id): Path<i64>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Json(input): Json<MergeLabel>,
) -> Result<Json<Label>, LificError> {
    // Both labels must live in the same project, and the caller must lead it.
    let source_project = with_read(&db, |conn| {
        crate::db::queries::get_resource_project_id(conn, Labels::TABLE, id)
    })?;
    let target_project = with_read(&db, |conn| {
        crate::db::queries::get_resource_project_id(conn, Labels::TABLE, input.into)
    })?;
    if source_project != target_project {
        return Err(LificError::BadRequest(
            "cannot merge labels across projects".into(),
        ));
    }
    require_structure_role(&db, &identity, source_project)?;
    let label = with_write(&db, |conn| {
        crate::db::queries::merge_label(conn, id, input.into)
    })?;
    realtime.send(RealtimeEvent::ProjectUpdated { project_id: source_project });
    Ok(Json(label))
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
