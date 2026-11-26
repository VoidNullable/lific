//! Who may publish a project (LIF-465). Moved here from the retired public
//! API module when LIF-583 made publication mean "readable at its normal links".

use crate::api::test_helpers::*;
use crate::db::models::*;
use axum::http::StatusCode;

/// Publication is off for every project the moment it is created. There
/// is no inherited value, no "public because it has no members".
#[tokio::test]
async fn a_new_project_is_private() {
    let app = test_app();
    let (_, project) = seed_project(&app).await;
    assert_eq!(project["is_public"], false);
}

/// Legacy mode (`authz_enforced` off) is the default on every existing
/// instance, and its `require_role` treats Viewer and Maintainer as
/// unconditional allows. Only the Lead branch denies anybody, so this is
/// the mode where a wrong gate would be invisible.
#[tokio::test]
async fn only_the_lead_or_an_admin_may_publish_in_legacy_mode() {
    let (db, admin, lead, regular, project_id) = setup_lead_test();
    assert!(!crate::authz::authz_enforced(&db).unwrap());

    let publish = serde_json::json!({ "is_public": true });

    let response = json_put(
        &app_as_user(db.clone(), &regular),
        &format!("/api/projects/{project_id}"),
        publish.clone(),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "a non-lead must not be able to publish a project"
    );
    assert!(!is_public(&db, project_id));

    let response = json_put(
        &app_as_user(db.clone(), &lead),
        &format!("/api/projects/{project_id}"),
        publish.clone(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(is_public(&db, project_id));

    // And an admin can put it back.
    let response = json_put(
        &app_as_user(db.clone(), &admin),
        &format!("/api/projects/{project_id}"),
        serde_json::json!({ "is_public": false }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!is_public(&db, project_id));
}

/// Enforced mode: the roles below Lead exist and are denied by name.
#[tokio::test]
async fn maintainers_and_viewers_may_not_publish_in_enforced_mode() {
    let (db, _admin, lead, maintainer, viewer, non_member, project_id) = setup_membership_test();

    for user in [&maintainer, &viewer, &non_member] {
        let response = json_put(
            &app_as_user(db.clone(), user),
            &format!("/api/projects/{project_id}"),
            serde_json::json!({ "is_public": true }),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "{} must not be able to publish",
            user.username
        );
        assert!(!is_public(&db, project_id));
    }

    let response = json_put(
        &app_as_user(db.clone(), &lead),
        &format!("/api/projects/{project_id}"),
        serde_json::json!({ "is_public": true }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(is_public(&db, project_id));
}

/// A connected tool acts with its owner's permissions and no more: the
/// existing bot policy, applied to the one setting where exceeding it
/// would put private issues on the internet. An agent is exactly the
/// caller most likely to be talked into trying (prompt injection), so the
/// answer has to come from the owner's role rather than from the bot's.
#[tokio::test]
async fn a_bot_publishes_only_what_its_owner_could() {
    let (db, _admin, lead, regular, project_id) = setup_lead_test();

    let (lead_bot, regular_bot) = {
        let conn = db.write().unwrap();
        (
            crate::db::queries::users::create_bot_user(
                &conn,
                lead.id,
                "lead-bot",
                "Lead's tool",
                Some("tool-a"),
            )
            .unwrap(),
            crate::db::queries::users::create_bot_user(
                &conn,
                regular.id,
                "regular-bot",
                "Regular's tool",
                Some("tool-b"),
            )
            .unwrap(),
        )
    };

    let response = json_put(
        &app_as_user(db.clone(), &regular_bot),
        &format!("/api/projects/{project_id}"),
        serde_json::json!({ "is_public": true }),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "a bot owned by a non-lead must not publish"
    );
    assert!(!is_public(&db, project_id));

    let response = json_put(
        &app_as_user(db.clone(), &lead_bot),
        &format!("/api/projects/{project_id}"),
        serde_json::json!({ "is_public": true }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(is_public(&db, project_id));
}

/// Publication is only ever changed by a request that says so. A rename
/// must not carry an implicit `is_public: false` and silently take a
/// published project down (nor the reverse).
#[tokio::test]
async fn an_unrelated_project_edit_leaves_publication_alone() {
    let (db, _admin, lead, _regular, project_id) = setup_lead_test();
    let app = app_as_user(db.clone(), &lead);

    json_put(
        &app,
        &format!("/api/projects/{project_id}"),
        serde_json::json!({ "is_public": true }),
    )
    .await;
    assert!(is_public(&db, project_id));

    let response = json_put(
        &app,
        &format!("/api/projects/{project_id}"),
        serde_json::json!({ "name": "Renamed" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = parse_json(response).await;
    assert_eq!(body["name"], "Renamed");
    assert_eq!(body["is_public"], true);
    assert!(is_public(&db, project_id));
}

/// Flipping the flag is the most consequential project edit there is, so
/// migration 051 records it. Without the row, an instance owner has no
/// way to answer "when did this go public, and who did it".
#[tokio::test]
async fn publishing_is_written_to_the_audit_log() {
    let (db, _admin, lead, _regular, project_id) = setup_lead_test();
    let app = app_as_user(db.clone(), &lead);

    json_put(
        &app,
        &format!("/api/projects/{project_id}"),
        serde_json::json!({ "is_public": true }),
    )
    .await;
    json_put(
        &app,
        &format!("/api/projects/{project_id}"),
        serde_json::json!({ "is_public": false }),
    )
    .await;
    // A no-op re-publish writes nothing: the trigger's WHEN guard.
    json_put(
        &app,
        &format!("/api/projects/{project_id}"),
        serde_json::json!({ "is_public": false }),
    )
    .await;

    let conn = db.read().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT old_value, new_value FROM audit_log
              WHERE entity_type = 'project' AND field = 'is_public'
           ORDER BY id",
        )
        .unwrap();
    let rows: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(
        rows,
        vec![
            ("private".to_string(), "public".to_string()),
            ("public".to_string(), "private".to_string()),
        ]
    );
}

fn is_public(db: &crate::db::DbPool, project_id: i64) -> bool {
    let conn = db.read().unwrap();
    crate::db::queries::get_project(&conn, project_id)
        .unwrap()
        .is_public
}

/// `setup_lead_test`'s users are `User`s; a bot created by
/// `create_bot_user` is one too, so `app_as_user` works for both. This
/// exists only to make that explicit for a reader.
#[allow(dead_code)]
fn _bots_are_users(user: &User) -> bool {
    user.is_bot
}
