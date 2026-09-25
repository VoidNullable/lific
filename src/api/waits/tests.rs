use axum::http::StatusCode;

use crate::api::test_helpers::*;

async fn issue(app: &axum::Router, project_id: i64, title: &str) -> serde_json::Value {
    parse_json(
        json_post(
            app,
            "/api/issues",
            serde_json::json!({"project_id": project_id, "title": title, "status": "todo"}),
        )
        .await,
    )
    .await
}

#[tokio::test]
async fn waits_are_added_listed_and_cleared_over_rest() {
    let rt = test_app_with_realtime();
    let app = rt.app.clone();
    let (project_id, _) = seed_project(&app).await;
    let target = issue(&app, project_id, "Waiting").await;
    let id = target["id"].as_i64().unwrap();
    let mut rx = rt.realtime.subscribe();

    let user = json_post(
        &app,
        &format!("/api/issues/{id}/waits"),
        serde_json::json!({"user": "test-admin", "note": "real-device check"}),
    )
    .await;
    assert_eq!(user.status(), StatusCode::OK);
    let user = parse_json(user).await;
    assert_eq!(user["kind"], "user");
    assert_eq!(user["username"], "test-admin");
    assert_eq!(user["display_name"], "Test Admin");
    assert_eq!(user["state"], "holding");
    let event = rx.try_recv().expect("issue.updated");
    assert!(matches!(
        event.event,
        crate::realtime::RealtimeEvent::IssueUpdated { issue_id, .. } if issue_id == id
    ));

    let date = parse_json(
        json_post(
            &app,
            &format!("/api/issues/{id}/waits"),
            serde_json::json!({"from": "2099-01-02", "until": "2099-01-05", "note": "office"}),
        )
        .await,
    )
    .await;
    assert_eq!(date["kind"], "date");
    assert_eq!(date["earliest"], "2099-01-02");
    assert_eq!(date["latest"], "2099-01-05");
    assert_eq!(date["state"], "holding");

    let listed = parse_json(json_get(&app, &format!("/api/issues/{id}/waits")).await).await;
    assert_eq!(listed.as_array().unwrap().len(), 2);
    let detail = parse_json(json_get(&app, &format!("/api/issues/{id}")).await).await;
    assert_eq!(detail["waits"].as_array().unwrap().len(), 2);
    let list = parse_json(
        json_get(
            &app,
            &format!("/api/issues?project_id={project_id}&workable=true"),
        )
        .await,
    )
    .await;
    assert!(list.as_array().unwrap().is_empty(), "{list}");

    // The web read model sees the waits on the issue row.
    let index =
        parse_json(json_get(&app, &format!("/api/projects/{project_id}/index")).await).await;
    assert_eq!(index["issues"][0]["waits"].as_array().unwrap().len(), 2);

    for wait in [&user, &date] {
        let resp = json_delete(
            &app,
            &format!("/api/issues/{id}/waits/{}", wait["id"].as_i64().unwrap()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }
    let list = parse_json(
        json_get(
            &app,
            &format!("/api/issues?project_id={project_id}&workable=true"),
        )
        .await,
    )
    .await;
    assert_eq!(list.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn invalid_waits_are_refused_with_client_errors() {
    let app = test_app();
    let (project_id, _) = seed_project(&app).await;
    let target = issue(&app, project_id, "Waiting").await;
    let other = issue(&app, project_id, "Other").await;
    let id = target["id"].as_i64().unwrap();
    let uri = format!("/api/issues/{id}/waits");

    for (body, status, needle) in [
        (
            serde_json::json!({"user": "ghost"}),
            StatusCode::BAD_REQUEST,
            "no active user named 'ghost'",
        ),
        (
            serde_json::json!({"from": "2026-09-28", "until": "2026-09-01"}),
            StatusCode::BAD_REQUEST,
            "is before from",
        ),
        (serde_json::json!({}), StatusCode::BAD_REQUEST, "pass user"),
    ] {
        let resp = json_post(&app, &uri, body).await;
        assert_eq!(resp.status(), status);
        let text = parse_json(resp).await.to_string();
        assert!(text.contains(needle), "{text}");
    }

    let first = json_post(&app, &uri, serde_json::json!({"user": "test-admin"})).await;
    assert_eq!(first.status(), StatusCode::OK);
    let wait_id = parse_json(first).await["id"].as_i64().unwrap();
    let again = json_post(&app, &uri, serde_json::json!({"user": "test-admin"})).await;
    assert_eq!(again.status(), StatusCode::CONFLICT);

    // A wait cannot be cleared through a different issue's path.
    let other_id = other["id"].as_i64().unwrap();
    let resp = json_delete(&app, &format!("/api/issues/{other_id}/waits/{wait_id}")).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let resp = json_get(&app, "/api/issues/9999/waits").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn wait_routes_are_gated_viewer_to_read_and_maintainer_to_write() {
    let (db, _admin, lead, maintainer, viewer, non_member, project_id) = setup_membership_test();
    let lead_app = app_as_user(db.clone(), &lead);
    let target = issue(&lead_app, project_id, "Gate").await;
    let id = target["id"].as_i64().unwrap();
    let uri = format!("/api/issues/{id}/waits");
    let body = serde_json::json!({"user": "lead"});

    let viewer_app = app_as_user(db.clone(), &viewer);
    let outsider = app_as_user(db.clone(), &non_member);
    assert_eq!(
        json_post(&viewer_app, &uri, body.clone()).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        json_post(&outsider, &uri, body.clone()).await.status(),
        StatusCode::FORBIDDEN
    );

    let maintainer_app = app_as_user(db.clone(), &maintainer);
    let added = json_post(&maintainer_app, &uri, body).await;
    assert_eq!(added.status(), StatusCode::OK);
    let wait_id = parse_json(added).await["id"].as_i64().unwrap();

    assert_eq!(json_get(&viewer_app, &uri).await.status(), StatusCode::OK);
    assert_eq!(
        json_get(&outsider, &uri).await.status(),
        StatusCode::FORBIDDEN
    );

    let clear = format!("{uri}/{wait_id}");
    assert_eq!(
        json_delete(&viewer_app, &clear).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        json_delete(&outsider, &clear).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        json_delete(&maintainer_app, &clear).await.status(),
        StatusCode::OK
    );

    // Both writes are in the issue's history.
    let conn = db.read().unwrap();
    let audited: i64 = conn
        .query_row(
            "SELECT count(*) FROM audit_log WHERE issue_id = ?1 AND action IN ('wait', 'unwait')",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(audited, 2);
}
