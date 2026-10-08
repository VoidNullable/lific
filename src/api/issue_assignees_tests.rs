//! LIF-147 / LIF-506: assignment over REST and the attention listing.

use axum::http::StatusCode;

use crate::api::test_helpers::*;

async fn issue(
    app: &axum::Router,
    project_id: i64,
    title: &str,
    assignees: serde_json::Value,
) -> serde_json::Value {
    let resp = json_post(
        app,
        "/api/issues",
        serde_json::json!({
            "project_id": project_id, "title": title, "status": "todo", "assignees": assignees
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    parse_json(resp).await
}

fn usernames(issue: &serde_json::Value) -> Vec<String> {
    issue["assignees"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|a| a["username"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn assignment_round_trips_and_filters_over_rest() {
    let (db, _admin, lead, maintainer, viewer, _outsider, project_id) = setup_membership_test();
    let app = app_as_user(db.clone(), &maintainer);

    let mine = issue(&app, project_id, "Mine", serde_json::json!(["me"])).await;
    assert_eq!(mine["needs_human"], true);
    assert_eq!(usernames(&mine), ["maintainer"]);

    let anyone = issue(&app, project_id, "Anyone", serde_json::json!(["human"])).await;
    assert_eq!(anyone["needs_human"], true);
    assert!(anyone.get("assignees").is_none(), "{anyone}");

    let free = issue(&app, project_id, "Free", serde_json::json!(null)).await;
    assert_eq!(free["needs_human"], false);

    let id = free["id"].as_i64().unwrap();
    let updated = parse_json(
        json_put(
            &app,
            &format!("/api/issues/{id}"),
            serde_json::json!({"assignees": ["lead", "@Viewer"]}),
        )
        .await,
    )
    .await;
    assert_eq!(usernames(&updated), ["lead", "viewer"]);

    let list = |filter: &'static str| {
        let app = app.clone();
        async move {
            let rows = parse_json(
                json_get(
                    &app,
                    &format!("/api/issues?project_id={project_id}&assignee={filter}"),
                )
                .await,
            )
            .await;
            rows.as_array()
                .unwrap()
                .iter()
                .map(|row| row["title"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(list("me").await, ["Mine"]);
    assert_eq!(list("lead").await, ["Free"]);
    let mut human = list("human").await;
    human.sort();
    assert_eq!(human, ["Anyone", "Free", "Mine"]);
    assert!(list("none").await.is_empty());

    // Clearing hands it back to agents.
    let cleared = parse_json(
        json_put(
            &app,
            &format!("/api/issues/{id}"),
            serde_json::json!({"assignees": []}),
        )
        .await,
    )
    .await;
    assert_eq!(cleared["needs_human"], false);
    assert_eq!(list("none").await, ["Free"]);

    // The web read model carries it.
    let index =
        parse_json(json_get(&app, &format!("/api/projects/{project_id}/index")).await).await;
    let rows = index["issues"].as_array().unwrap();
    let mine_row = rows.iter().find(|row| row["title"] == "Mine").unwrap();
    assert_eq!(mine_row["needs_human"], true);
    assert_eq!(usernames(mine_row), ["maintainer"]);

    let _ = (lead, viewer);
}

#[tokio::test]
async fn invalid_assignments_are_client_errors() {
    let (db, _admin, _lead, maintainer, _viewer, outsider, project_id) = setup_membership_test();
    let app = app_as_user(db.clone(), &maintainer);
    let target = issue(&app, project_id, "Target", serde_json::json!(null)).await;
    let uri = format!("/api/issues/{}", target["id"]);
    for (body, needle) in [
        (serde_json::json!(["non_member"]), "not a member"),
        (serde_json::json!(["human", "lead"]), "not both"),
        (serde_json::json!(["ghost"]), "no active user named 'ghost'"),
    ] {
        let resp = json_put(&app, &uri, serde_json::json!({ "assignees": body })).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let text = parse_json(resp).await.to_string();
        assert!(text.contains(needle), "{text}");
    }
    let _ = outsider;
}

#[tokio::test]
async fn viewers_cannot_assign() {
    let (db, _admin, _lead, maintainer, viewer, _outsider, project_id) = setup_membership_test();
    let target = issue(
        &app_as_user(db.clone(), &maintainer),
        project_id,
        "Target",
        serde_json::json!(null),
    )
    .await;
    let resp = json_put(
        &app_as_user(db.clone(), &viewer),
        &format!("/api/issues/{}", target["id"]),
        serde_json::json!({"assignees": ["me"]}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn attention_lists_what_waits_on_the_caller_in_visible_projects() {
    let (db, admin, lead, maintainer, viewer, _outsider, project_id) = setup_membership_test();
    let app = app_as_user(db.clone(), &maintainer);
    issue(&app, project_id, "Assigned", serde_json::json!(["viewer"])).await;
    issue(&app, project_id, "Anyone", serde_json::json!(["human"])).await;
    let done = issue(&app, project_id, "Done", serde_json::json!(["viewer"])).await;
    json_put(
        &app,
        &format!("/api/issues/{}", done["id"]),
        serde_json::json!({"status": "done"}),
    )
    .await;
    let waiting = issue(&app, project_id, "Waits", serde_json::json!(null)).await;
    let resp = json_post(
        &app,
        &format!("/api/issues/{}/waits", waiting["id"]),
        serde_json::json!({"user": "viewer"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // A project the viewer cannot see, with an issue for any human.
    let admin_app = app_as_user(db.clone(), &admin);
    let hidden = parse_json(
        json_post(
            &admin_app,
            "/api/projects",
            serde_json::json!({"name": "Hidden", "identifier": "HID"}),
        )
        .await,
    )
    .await;
    issue(
        &admin_app,
        hidden["id"].as_i64().unwrap(),
        "Secret",
        serde_json::json!(["human"]),
    )
    .await;

    let titles = |group: &serde_json::Value| -> Vec<String> {
        group
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["title"].as_str().unwrap().to_string())
            .collect()
    };
    let attention =
        parse_json(json_get(&app_as_user(db.clone(), &viewer), "/api/issues/attention").await)
            .await;
    assert_eq!(titles(&attention["assigned"]), ["Assigned"]);
    assert_eq!(titles(&attention["human"]), ["Anyone"]);
    assert_eq!(titles(&attention["waiting"]), ["Waits"]);

    let mut admin_human =
        titles(&parse_json(json_get(&admin_app, "/api/issues/attention").await).await["human"]);
    admin_human.sort();
    assert_eq!(admin_human, ["Anyone", "Secret"]);
    let _ = lead;
}
