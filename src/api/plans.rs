use axum::{
    Extension,
    extract::{Json, Path, Query, State},
};

use crate::authz;
use crate::db::queries::plans::{self, StepDoneEffect};
use crate::db::{DbPool, models::*};
use crate::error::LificError;
use crate::realtime::{RealtimeEvent, RealtimeHub};

use super::{filter_visible, with_read, with_write};

pub(super) async fn list_plans(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Query(q): Query<ListPlansQuery>,
) -> Result<Json<Vec<Plan>>, LificError> {
    if let Some(pid) = q.project_id {
        authz::require_role(&db, &auth_user, pid, Role::Viewer)?;
        return with_read(&db, |conn| plans::list_plans(conn, &q)).map(Json);
    }
    // Cross-project list (LIF-197 scope item 2): filter, don't deny.
    let visible = authz::visible_project_ids(&db, &auth_user)?;
    let list = with_read(&db, |conn| plans::list_plans(conn, &q))?;
    Ok(Json(filter_visible(list, &visible, |p| Some(p.project_id))))
}

pub(super) async fn get_plan(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path(id): Path<i64>,
) -> Result<Json<Plan>, LificError> {
    let plan = with_read(&db, |conn| plans::get_plan(conn, id))?;
    authz::require_role(&db, &auth_user, plan.project_id, Role::Viewer)?;
    Ok(Json(plan))
}

pub(super) async fn resolve_plan(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path(identifier): Path<String>,
) -> Result<Json<Plan>, LificError> {
    let plan = with_read(&db, |conn| {
        let id = plans::resolve_plan_identifier(conn, &identifier)?;
        plans::get_plan(conn, id)
    })?;
    authz::require_role(&db, &auth_user, plan.project_id, Role::Viewer)?;
    Ok(Json(plan))
}

pub(super) async fn create_plan(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<CreatePlan>,
) -> Result<Json<Plan>, LificError> {
    authz::require_role(&db, &auth_user, input.project_id, Role::Maintainer)?;
    let plan = with_write(&db, |conn| plans::create_plan(conn, &input))?;
    realtime.send(RealtimeEvent::ProjectUpdated { project_id: plan.project_id });
    Ok(Json(plan))
}

pub(super) async fn update_plan(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path(id): Path<i64>,
    Json(input): Json<UpdatePlan>,
) -> Result<Json<Plan>, LificError> {
    let project_id = with_read(&db, |conn| plans::get_plan(conn, id))?.project_id;
    authz::require_role(&db, &auth_user, project_id, Role::Maintainer)?;
    let plan = with_write(&db, |conn| plans::update_plan(conn, id, &input))?;
    realtime.send(RealtimeEvent::ProjectUpdated { project_id });
    Ok(Json(plan))
}

pub(super) async fn delete_plan_handler(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, LificError> {
    let project_id = with_read(&db, |conn| plans::get_plan(conn, id))?.project_id;
    authz::require_role(&db, &auth_user, project_id, Role::Maintainer)?;
    with_write(&db, |conn| plans::delete_plan(conn, id))?;
    realtime.send(RealtimeEvent::ProjectUpdated { project_id });
    Ok(Json(serde_json::json!({"deleted": true})))
}

#[derive(serde::Deserialize)]
pub(super) struct AddStepRequest {
    pub parent_step_id: Option<i64>,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub issue_id: Option<i64>,
}

pub(super) async fn add_step(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path(plan_id): Path<i64>,
    Json(input): Json<AddStepRequest>,
) -> Result<Json<Plan>, LificError> {
    let project_id = with_read(&db, |conn| plans::get_plan(conn, plan_id))?.project_id;
    authz::require_role(&db, &auth_user, project_id, Role::Maintainer)?;
    let plan = with_write(&db, |conn| {
        plans::add_step(
            conn,
            plan_id,
            input.parent_step_id,
            &input.title,
            &input.description,
            input.issue_id,
        )?;
        plans::get_plan(conn, plan_id)
    })?;
    realtime.send(RealtimeEvent::ProjectUpdated { project_id });
    Ok(Json(plan))
}

#[derive(serde::Deserialize)]
pub(super) struct UpdateStepRequest {
    pub title: Option<String>,
    pub description: Option<String>,
    pub done: Option<bool>,
    /// Tristate issue link: absent = no change, null = detach, id = attach.
    #[serde(default, deserialize_with = "crate::db::models::deserialize_nullable")]
    pub issue_id: Option<Option<i64>>,
    pub move_parent_step_id: Option<i64>,
    pub move_to_root: Option<bool>,
    pub move_position: Option<i64>,
}

#[derive(serde::Serialize)]
pub(super) struct StepUpdateResponse {
    pub plan: Plan,
    /// Set when the request toggled `done`, so the UI can surface the
    /// issue side effect (e.g. "LIF-42 marked done").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect: Option<StepDoneEffect>,
}

pub(super) async fn update_step(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path((plan_id, step_id)): Path<(i64, i64)>,
    Json(input): Json<UpdateStepRequest>,
) -> Result<Json<StepUpdateResponse>, LificError> {
    let project_id = with_read(&db, |conn| plans::get_plan(conn, plan_id))?.project_id;
    authz::require_role(&db, &auth_user, project_id, Role::Maintainer)?;
    let (resp, issue_event) = with_write(&db, |conn| {
        plans::assert_step_in_plan(conn, plan_id, step_id)?;
        if let Some(ref t) = input.title {
            plans::set_step_title(conn, step_id, t)?;
        }
        if let Some(ref d) = input.description {
            plans::set_step_description(conn, step_id, d)?;
        }
        if let Some(issue) = input.issue_id {
            plans::set_step_issue(conn, step_id, issue)?;
        }
        let mut effect = None;
        if let Some(done) = input.done {
            effect = Some(plans::set_step_done(conn, step_id, done)?);
        }
        if input.move_to_root.unwrap_or(false)
            || input.move_parent_step_id.is_some()
            || input.move_position.is_some()
        {
            let new_parent = if input.move_to_root.unwrap_or(false) {
                None
            } else if let Some(p) = input.move_parent_step_id {
                Some(p)
            } else {
                plans::step_parent(conn, step_id)?
            };
            plans::move_step(conn, step_id, new_parent, input.move_position)?;
        }
        let plan = plans::get_plan(conn, plan_id)?;
        let issue_event = if effect
            .as_ref()
            .is_some_and(|effect| effect.issue_status_changed)
        {
            plans::step_issue_id(conn, step_id)?
                .map(|issue_id| {
                    crate::db::queries::get_issue(conn, issue_id)
                        .map(|issue| (issue.project_id, issue.id))
                })
                .transpose()?
        } else {
            None
        };
        Ok((StepUpdateResponse { plan, effect }, issue_event))
    })?;
    realtime.send(RealtimeEvent::ProjectUpdated { project_id });
    if let Some((issue_project_id, issue_id)) = issue_event {
        realtime.send(RealtimeEvent::IssueUpdated {
            project_id: issue_project_id,
            issue_id,
        });
    }
    Ok(Json(resp))
}

pub(super) async fn delete_step_handler(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path((plan_id, step_id)): Path<(i64, i64)>,
) -> Result<Json<Plan>, LificError> {
    let project_id = with_read(&db, |conn| plans::get_plan(conn, plan_id))?.project_id;
    authz::require_role(&db, &auth_user, project_id, Role::Maintainer)?;
    let plan = with_write(&db, |conn| {
        plans::assert_step_in_plan(conn, plan_id, step_id)?;
        plans::delete_step(conn, step_id)?;
        plans::get_plan(conn, plan_id)
    })?;
    realtime.send(RealtimeEvent::ProjectUpdated { project_id });
    Ok(Json(plan))
}

#[cfg(test)]
mod tests {
    use crate::api::test_helpers::*;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn body_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn plan_crud_and_step_cascade() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;

        // Seed an issue to mirror.
        let issue = json_post(
            &app,
            "/api/issues",
            serde_json::json!({"project_id": project_id, "title": "Work", "status": "todo"}),
        )
        .await;
        let issue = body_json(issue).await;
        let issue_id = issue["id"].as_i64().unwrap();

        // Create a plan with one issue-linked step.
        let resp = json_post(
            &app,
            "/api/plans",
            serde_json::json!({
                "project_id": project_id,
                "title": "Ship it",
                "steps": [{"title": "mirror", "issue_id": issue_id}]
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let plan = body_json(resp).await;
        let plan_id = plan["id"].as_i64().unwrap();
        assert_eq!(plan["identifier"], "TST-PLAN-1");
        let step_id = plan["steps"][0]["id"].as_i64().unwrap();

        // Mark the step done → issue should close, effect reported.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/plans/{plan_id}/steps/{step_id}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_vec(&serde_json::json!({"done": true})).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let out = body_json(resp).await;
        assert_eq!(out["effect"]["issue_status_changed"], true);
        assert_eq!(out["plan"]["steps"][0]["done"], true);

        // List filtered by status.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/plans?project_id={project_id}&status=active"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let list = body_json(resp).await;
        assert_eq!(list.as_array().unwrap().len(), 1);

        // Delete.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/plans/{plan_id}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn completing_cross_project_linked_step_emits_issue_updated_for_issue_project() {
        let test = test_app_with_realtime();
        let (plan_project_id, _) = seed_project(&test.app).await;
        let issue_project = body_json(
            json_post(
                &test.app,
                "/api/projects",
                serde_json::json!({
                    "name": "Issue Project",
                    "identifier": "ISS",
                }),
            )
            .await,
        )
        .await;
        let issue_project_id = issue_project["id"].as_i64().unwrap();
        let issue = body_json(
            json_post(
                &test.app,
                "/api/issues",
                serde_json::json!({
                    "project_id": issue_project_id,
                    "title": "Cross-project issue",
                }),
            )
            .await,
        )
        .await;
        let issue_id = issue["id"].as_i64().unwrap();
        let plan = body_json(
            json_post(
                &test.app,
                "/api/plans",
                serde_json::json!({
                    "project_id": plan_project_id,
                    "title": "Cross-project plan",
                    "steps": [{"title": "Complete linked issue", "issue_id": issue_id}],
                }),
            )
            .await,
        )
        .await;
        let plan_id = plan["id"].as_i64().unwrap();
        let step_id = plan["steps"][0]["id"].as_i64().unwrap();
        let mut events = test.realtime.subscribe();

        let resp = test
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/plans/{plan_id}/steps/{step_id}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_vec(&serde_json::json!({"done": true})).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let project_event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();
        let axum::extract::ws::Message::Text(project_text) = project_event.message else {
            panic!("expected text realtime event");
        };
        let project_event: serde_json::Value = serde_json::from_str(&project_text).unwrap();
        assert_eq!(project_event["type"], "project.updated");
        assert_eq!(project_event["project_id"], plan_project_id);

        let issue_event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();
        let axum::extract::ws::Message::Text(issue_text) = issue_event.message else {
            panic!("expected text realtime event");
        };
        let issue_event: serde_json::Value = serde_json::from_str(&issue_text).unwrap();
        assert_eq!(issue_event["type"], "issue.updated");
        assert_eq!(issue_event["project_id"], issue_project_id);
        assert_eq!(issue_event["issue_id"], issue_id);
    }

    #[tokio::test]
    async fn step_description_edit_and_plan_activity() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;

        let plan = body_json(
            json_post(
                &app,
                "/api/plans",
                serde_json::json!({
                    "project_id": project_id,
                    "title": "Doc plan",
                    "steps": [{"title": "step"}]
                }),
            )
            .await,
        )
        .await;
        let plan_id = plan["id"].as_i64().unwrap();
        let step_id = plan["steps"][0]["id"].as_i64().unwrap();

        // Set the step's description (the previously-missing capability).
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/plans/{plan_id}/steps/{step_id}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_vec(&serde_json::json!({"description": "the body"}))
                            .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let out = body_json(resp).await;
        assert_eq!(out["plan"]["steps"][0]["description"], "the body");

        // Plan activity feed includes plan + step rows.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/plans/{plan_id}/activity"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let feed = body_json(resp).await;
        let items = feed["items"].as_array().unwrap();
        assert!(
            items
                .iter()
                .any(|a| a["entity_type"] == "plan" && a["action"] == "create")
        );
        assert!(
            items
                .iter()
                .any(|a| a["entity_type"] == "plan_step" && a["field"] == "description"),
            "step description edit must show in plan activity: {feed}"
        );
    }
}
