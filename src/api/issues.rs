use axum::{
    Extension,
    extract::{Json, Path, Query, State},
};

use crate::authz;
use crate::db::{DbPool, models::*};
use crate::error::LificError;
use crate::realtime::{RealtimeEvent, RealtimeHub};

use super::{filter_visible, with_read, with_write};

pub(super) async fn list_issues(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Query(q): Query<ListIssuesQuery>,
) -> Result<Json<Vec<Issue>>, LificError> {
    if let Some(pid) = q.project_id {
        authz::require_role(&db, &identity, pid, Role::Viewer)?;
        return with_read(&db, |conn| crate::db::queries::list_issues(conn, &q)).map(Json);
    }
    // Cross-project list: filter instead of denying (LIF-197 scope item 2).
    let visible = authz::visible_project_ids(&db, &identity)?;
    let issues = with_read(&db, |conn| crate::db::queries::list_issues(conn, &q))?;
    Ok(Json(filter_visible(issues, &visible, |i| {
        Some(i.project_id)
    })))
}

pub(super) async fn get_issue(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(id): Path<i64>,
) -> Result<Json<Issue>, LificError> {
    let issue = with_read(&db, |conn| crate::db::queries::get_issue(conn, id))?;
    authz::require_role(&db, &identity, issue.project_id, Role::Viewer)?;
    Ok(Json(issue))
}

pub(super) async fn resolve_issue(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(identifier): Path<String>,
) -> Result<Json<Issue>, LificError> {
    let issue = with_read(&db, |conn| {
        let id = crate::db::queries::resolve_identifier(conn, &identifier)?;
        crate::db::queries::get_issue(conn, id)
    })?;
    authz::require_role(&db, &identity, issue.project_id, Role::Viewer)?;
    Ok(Json(issue))
}

pub(super) async fn create_issue(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Json(input): Json<CreateIssue>,
) -> Result<Json<Issue>, LificError> {
    authz::require_role(&db, &identity, input.project_id, Role::Maintainer)?;
    let issue = with_write(&db, |conn| {
        let issue = crate::db::queries::create_issue(conn, &input)?;
        // LIF-262: link any attachments the description references.
        crate::db::queries::attachments::sync_links(
            conn,
            AttachmentEntity::Issue,
            issue.id,
            &issue.description,
        )?;
        Ok(issue)
    })?;
    realtime.send(RealtimeEvent::IssueCreated {
        project_id: issue.project_id,
        issue_id: issue.id,
    });
    Ok(Json(issue))
}

pub(super) async fn update_issue(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(id): Path<i64>,
    Json(input): Json<UpdateIssue>,
) -> Result<Json<Issue>, LificError> {
    let project_id = with_read(&db, |conn| crate::db::queries::get_issue(conn, id))?.project_id;
    authz::require_role(&db, &identity, project_id, Role::Maintainer)?;
    let issue = with_write(&db, |conn| {
        let issue = crate::db::queries::update_issue(conn, id, &input)?;
        // LIF-262: re-scan the (possibly edited) description and reconcile links.
        crate::db::queries::attachments::sync_links(
            conn,
            AttachmentEntity::Issue,
            issue.id,
            &issue.description,
        )?;
        Ok(issue)
    })?;
    realtime.send(RealtimeEvent::IssueUpdated {
        project_id: issue.project_id,
        issue_id: issue.id,
    });
    Ok(Json(issue))
}

pub(super) async fn delete_issue_handler(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, LificError> {
    let project_id = with_read(&db, |conn| crate::db::queries::get_issue(conn, id))?.project_id;
    authz::require_role(&db, &identity, project_id, Role::Maintainer)?;
    let issue = with_write(&db, |conn| {
        let issue = crate::db::queries::get_issue(conn, id)?;
        crate::db::queries::delete_issue(conn, id)?;
        Ok(issue)
    })?;
    realtime.send(RealtimeEvent::IssueDeleted {
        project_id: issue.project_id,
        issue_id: issue.id,
    });
    Ok(Json(serde_json::json!({"deleted": true})))
}

/// LIF-363: every relation edge inside one project, in one round trip. Feeds
/// the dependency-graph view; the client filters to `blocks` edges itself so
/// a future view mode (e.g. relates_to clusters) needs no new endpoint.
pub(super) async fn project_relations(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<ProjectRelation>>, LificError> {
    authz::require_role(&db, &identity, id, Role::Viewer)?;
    with_read(&db, |conn| {
        crate::db::queries::list_project_relations(conn, id)
    })
    .map(Json)
}

#[derive(serde::Deserialize)]
pub(super) struct LinkRequest {
    source: String,
    target: String,
    relation_type: String,
}

#[derive(serde::Deserialize)]
pub(super) struct UnlinkRequest {
    source: String,
    target: String,
}

pub(super) async fn link_issues(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Json(input): Json<LinkRequest>,
) -> Result<Json<serde_json::Value>, LificError> {
    let (source, target) = with_read(&db, |conn| {
        let source_id = crate::db::queries::resolve_identifier(conn, &input.source)?;
        let target_id = crate::db::queries::resolve_identifier(conn, &input.target)?;
        Ok((
            crate::db::queries::get_issue(conn, source_id)?,
            crate::db::queries::get_issue(conn, target_id)?,
        ))
    })?;
    // Cross-project relation: the caller must be a Maintainer on BOTH sides
    // (LIF-197 scope item 3), even when source and target share a project.
    authz::require_role(&db, &identity, source.project_id, Role::Maintainer)?;
    authz::require_role(&db, &identity, target.project_id, Role::Maintainer)?;

    with_write(&db, |conn| {
        crate::db::queries::link_issues(conn, source.id, target.id, &input.relation_type)
    })?;
    realtime.send(RealtimeEvent::IssueLinked {
        project_id: source.project_id,
        issue_id: source.id,
    });
    realtime.send(RealtimeEvent::IssueLinked {
        project_id: target.project_id,
        issue_id: target.id,
    });
    Ok(Json(serde_json::json!({"linked": true})))
}

pub(super) async fn unlink_issues(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Json(input): Json<UnlinkRequest>,
) -> Result<Json<serde_json::Value>, LificError> {
    let (source, target) = with_read(&db, |conn| {
        let source_id = crate::db::queries::resolve_identifier(conn, &input.source)?;
        let target_id = crate::db::queries::resolve_identifier(conn, &input.target)?;
        Ok((
            crate::db::queries::get_issue(conn, source_id)?,
            crate::db::queries::get_issue(conn, target_id)?,
        ))
    })?;
    authz::require_role(&db, &identity, source.project_id, Role::Maintainer)?;
    authz::require_role(&db, &identity, target.project_id, Role::Maintainer)?;

    with_write(&db, |conn| {
        crate::db::queries::unlink_issues(conn, source.id, target.id)
    })?;
    realtime.send(RealtimeEvent::IssueUnlinked {
        project_id: source.project_id,
        issue_id: source.id,
    });
    realtime.send(RealtimeEvent::IssueUnlinked {
        project_id: target.project_id,
        issue_id: target.id,
    });
    Ok(Json(serde_json::json!({"unlinked": true})))
}

#[cfg(test)]
mod tests {
    use crate::api::test_helpers::*;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn issue_create_emits_realtime_event() {
        let test = test_app_with_realtime();
        let (project_id, _) = seed_project(&test.app).await;
        let mut events = test.realtime.subscribe();
        let body = serde_json::json!({
            "project_id": project_id,
            "title": "Fresh event",
        });

        let resp = json_post(&test.app, "/api/issues", body).await;

        assert_eq!(resp.status(), StatusCode::OK);
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();
        let axum::extract::ws::Message::Text(text) = event.message else {
            panic!("expected text realtime event");
        };
        let event: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(event["type"], "issue.created");
        assert_eq!(event["project_id"], project_id);
    }

    #[tokio::test]
    async fn issue_crud_lifecycle() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;

        // Create issue
        let body = serde_json::json!({
            "project_id": project_id,
            "title": "Fix the bug",
            "status": "todo",
            "priority": "high"
        });
        let resp = app
            .clone()
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
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let issue: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let issue_id = issue["id"].as_i64().unwrap();
        assert_eq!(issue["identifier"], "TST-1");
        assert_eq!(issue["priority"], "high");

        // List with filter
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/issues?project_id={project_id}&status=todo"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let list: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(list.len(), 1);

        // Update
        let update = serde_json::json!({"status": "active"});
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/issues/{issue_id}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&update).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let updated: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(updated["status"], "active");

        // Resolve by identifier
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/issues/resolve/TST-1")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Delete
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/issues/{issue_id}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// LIF-363: the graph endpoint returns every edge whose endpoints both
    /// live in the project — all relation types, both chain links — and
    /// excludes cross-project edges entirely (a node for the far endpoint
    /// wouldn't exist in a project-scoped graph).
    #[tokio::test]
    async fn project_relations_returns_in_project_edges_only() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;

        for title in ["A", "B", "C"] {
            let resp = json_post(
                &app,
                "/api/issues",
                serde_json::json!({"project_id": project_id, "title": title}),
            )
            .await;
            assert_eq!(resp.status(), StatusCode::OK);
        }
        // A blocks B, B blocks C (the acceptance-criteria chain), plus one
        // relates_to edge to prove type passthrough.
        for (source, target, rel) in [
            ("TST-1", "TST-2", "blocks"),
            ("TST-2", "TST-3", "blocks"),
            ("TST-1", "TST-3", "relates_to"),
        ] {
            let resp = json_post(
                &app,
                "/api/issues/link",
                serde_json::json!({"source": source, "target": target, "relation_type": rel}),
            )
            .await;
            assert_eq!(resp.status(), StatusCode::OK);
        }

        // A second project with a cross-project link back into TST: the edge
        // must not appear in either project's graph.
        let resp = json_post(
            &app,
            "/api/projects",
            serde_json::json!({"name": "Other", "identifier": "OTH"}),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let other: serde_json::Value = parse_json(resp).await;
        let other_id = other["id"].as_i64().unwrap();
        let resp = json_post(
            &app,
            "/api/issues",
            serde_json::json!({"project_id": other_id, "title": "Outsider"}),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = json_post(
            &app,
            "/api/issues/link",
            serde_json::json!({"source": "OTH-1", "target": "TST-1", "relation_type": "blocks"}),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = json_get(&app, &format!("/api/projects/{project_id}/relations")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let edges: serde_json::Value = parse_json(resp).await;
        let edges = edges.as_array().unwrap();
        assert_eq!(edges.len(), 3);
        let as_tuples: Vec<(String, String, String)> = edges
            .iter()
            .map(|e| {
                (
                    e["source_identifier"].as_str().unwrap().to_string(),
                    e["target_identifier"].as_str().unwrap().to_string(),
                    e["relation_type"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        for expected in [
            ("TST-1", "TST-2", "blocks"),
            ("TST-2", "TST-3", "blocks"),
            ("TST-1", "TST-3", "relates_to"),
        ] {
            assert!(
                as_tuples.contains(&(
                    expected.0.to_string(),
                    expected.1.to_string(),
                    expected.2.to_string()
                )),
                "missing edge {expected:?} in {as_tuples:?}"
            );
        }
        // Numeric ids come along for O(1) node lookup client-side.
        assert!(edges.iter().all(|e| e["source_id"].is_i64() && e["target_id"].is_i64()));

        let resp = json_get(&app, &format!("/api/projects/{other_id}/relations")).await;
        let edges: serde_json::Value = parse_json(resp).await;
        assert_eq!(edges.as_array().unwrap().len(), 0);
    }

    /// An empty project graphs to an empty edge list, not an error.
    #[tokio::test]
    async fn project_relations_empty_project() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;
        let resp = json_get(&app, &format!("/api/projects/{project_id}/relations")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let edges: serde_json::Value = parse_json(resp).await;
        assert_eq!(edges.as_array().unwrap().len(), 0);
    }

    /// LIF-385: `status` and `priority` are enums, so a value outside the set
    /// is refused by the extractor. Before, it travelled all the way to
    /// SQLite's CHECK constraint and came back as a 500.
    #[tokio::test]
    async fn out_of_set_status_and_priority_are_refused() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;

        for body in [
            serde_json::json!({"project_id": project_id, "title": "T", "status": "shipped"}),
            serde_json::json!({"project_id": project_id, "title": "T", "priority": "critical"}),
        ] {
            let resp = app
                .clone()
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
            assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        }

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/issues?project_id={project_id}&status=shipped"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
