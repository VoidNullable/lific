//! LIF-525: claims over REST, the workable listing and the Home overview.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use crate::api::test_helpers::*;

/// POST `uri` with an optional `X-Lific-Worker` header and JSON body.
async fn post(
    app: &Router,
    uri: &str,
    worker: Option<&str>,
    body: Option<serde_json::Value>,
) -> axum::response::Response {
    let mut request = Request::builder().method("POST").uri(uri);
    if let Some(worker) = worker {
        request = request.header("x-lific-worker", worker);
    }
    let body = match body {
        Some(body) => {
            request = request.header("content-type", "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    app.clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap()
}

async fn get_with(app: &Router, uri: &str, worker: &str) -> serde_json::Value {
    let request = Request::builder()
        .uri(uri)
        .header("x-lific-worker", worker)
        .body(Body::empty())
        .unwrap();
    parse_json(app.clone().oneshot(request).await.unwrap()).await
}

async fn issue(app: &Router, project_id: i64, title: &str, status: &str) -> i64 {
    let resp = json_post(
        app,
        "/api/issues",
        serde_json::json!({ "project_id": project_id, "title": title, "status": status }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    parse_json(resp).await["id"].as_i64().unwrap()
}

#[tokio::test]
async fn a_claim_refuses_other_workers_until_released() {
    let (db, _admin, lead, maintainer, viewer, _outsider, project_id) = setup_membership_test();
    let app = app_as_user(db.clone(), &maintainer);
    let id = issue(&app, project_id, "Contested", "todo").await;
    let claim = format!("/api/issues/{id}/claim");
    let release = format!("/api/issues/{id}/release");

    let resp = post(
        &app,
        &claim,
        Some("one"),
        Some(serde_json::json!({"minutes": 30})),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = parse_json(resp).await;
    assert_eq!(body["status"], "active", "a todo claim starts the work");
    assert_eq!(body["claim"]["username"], "maintainer");
    // A worker id in the header is the worker as sent.
    let worker = "one";
    assert_eq!(body["claim"]["worker"], worker);

    // Another session of the same account, and another account, are refused.
    let resp = post(&app, &claim, Some("two"), None).await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let error = parse_json(resp).await.to_string();
    assert!(
        error.contains(&format!("claimed by @maintainer (worker {worker})")),
        "{error}"
    );
    let lead_app = app_as_user(db.clone(), &lead);
    assert_eq!(
        post(&lead_app, &claim, None, None).await.status(),
        StatusCode::CONFLICT
    );
    // A viewer may not claim at all.
    let viewer_app = app_as_user(db.clone(), &viewer);
    assert_eq!(
        post(&viewer_app, &claim, None, None).await.status(),
        StatusCode::FORBIDDEN
    );

    // Workable from each session.
    let list = format!("/api/issues?project_id={project_id}&workable=true");
    assert_eq!(
        get_with(&app, &list, "one").await.as_array().unwrap().len(),
        1
    );
    assert!(
        get_with(&app, &list, "two")
            .await
            .as_array()
            .unwrap()
            .is_empty()
    );

    // Only the holder releases; then the other session takes it.
    assert_eq!(
        post(&app, &release, Some("two"), None).await.status(),
        StatusCode::CONFLICT
    );
    let resp = post(&app, &release, Some("one"), None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(parse_json(resp).await.get("claim").is_none());
    assert_eq!(
        post(&app, &claim, Some("two"), None).await.status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_header_worker_resumes_a_claim_and_bad_input_is_refused() {
    let (db, _admin, _lead, maintainer, _viewer, _outsider, project_id) = setup_membership_test();
    let app = app_as_user(db, &maintainer);
    let id = issue(&app, project_id, "Resumable", "active").await;
    let claim = format!("/api/issues/{id}/claim");
    // A session whose id is hashed reports its worker id; a later client
    // resumes by sending that id, and its workable list agrees.
    let request = Request::builder()
        .method("POST")
        .uri(&claim)
        .header("mcp-session-id", "an opaque session")
        .body(Body::empty())
        .unwrap();
    let first = parse_json(app.clone().oneshot(request).await.unwrap()).await;
    let worker = first["claim"]["worker"].as_str().unwrap().to_string();
    assert!(worker.starts_with("s-"), "{worker}");

    let resp = post(&app, &claim, Some(&worker), None).await;
    assert_eq!(resp.status(), StatusCode::OK, "the header names the holder");
    let renewed = parse_json(resp).await;
    assert_eq!(renewed["claim"]["claimed_at"], first["claim"]["claimed_at"]);
    let list = format!("/api/issues?project_id={project_id}&workable=true");
    assert_eq!(
        get_with(&app, &list, &worker)
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );

    for body in [
        serde_json::json!({"minutes": 0}),
        serde_json::json!({"minutes": 1441}),
        serde_json::json!({"worker": "w-1"}),
        serde_json::json!({"unknown": true}),
    ] {
        let resp = post(&app, &claim, None, Some(body.clone())).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{body}");
    }
    let release = format!("/api/issues/{id}/release");
    let resp = post(
        &app,
        &release,
        None,
        Some(serde_json::json!({"minutes": 5})),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let done = issue(&app, project_id, "Finished", "done").await;
    let resp = post(&app, &format!("/api/issues/{done}/claim"), None, None).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn home_lists_live_claims_and_leaves_them_out_of_the_agent_pool() {
    let (db, _admin, _lead, maintainer, _viewer, _outsider, project_id) = setup_membership_test();
    let app = app_as_user(db, &maintainer);
    let held = issue(&app, project_id, "Held", "todo").await;
    issue(&app, project_id, "Free", "todo").await;
    let ready = |body: &serde_json::Value| {
        body["age_buckets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|bucket| bucket["agent_ready"].as_i64().unwrap())
            .sum::<i64>()
    };
    let before = parse_json(json_get(&app, "/api/home/overview").await).await;
    assert_eq!(ready(&before), 2);
    assert!(before["claims"].as_array().unwrap().is_empty());

    // Claimed with the status left as todo, so only the claim takes it out.
    let resp = post(
        &app,
        &format!("/api/issues/{held}/claim"),
        Some("agent"),
        None,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    json_put(
        &app,
        &format!("/api/issues/{held}"),
        serde_json::json!({"status": "todo"}),
    )
    .await;

    let after = parse_json(json_get(&app, "/api/home/overview").await).await;
    assert_eq!(ready(&after), 1);
    let claims = after["claims"].as_array().unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0]["title"], "Held");
    assert_eq!(claims[0]["claim"]["username"], "maintainer");
}
