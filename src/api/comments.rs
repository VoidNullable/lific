use axum::{
    Extension,
    extract::{Json, Path, State},
};

use crate::db::{DbPool, models::*};
use crate::error::LificError;

use super::{with_read, with_write};

pub(super) async fn list_comments(
    State(db): State<DbPool>,
    Path(issue_id): Path<i64>,
) -> Result<Json<Vec<Comment>>, LificError> {
    with_read(&db, |conn| {
        crate::db::queries::comments::list_comments(conn, issue_id)
    })
    .map(Json)
}

pub(super) async fn create_comment(
    State(db): State<DbPool>,
    Path(issue_id): Path<i64>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<CreateComment>,
) -> Result<Json<Comment>, LificError> {
    let user = auth_user
        .ok_or_else(|| LificError::BadRequest("authentication required to comment".into()))?;

    with_write(&db, |conn| {
        crate::db::queries::comments::create_comment(conn, issue_id, user.id, &input.content)
    })
    .map(Json)
}

pub(super) async fn update_comment_handler(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<UpdateComment>,
) -> Result<Json<Comment>, LificError> {
    let user = auth_user.ok_or_else(|| LificError::BadRequest("authentication required".into()))?;

    // Check ownership: only the author or an admin can edit
    let existing = with_read(&db, |conn| {
        crate::db::queries::comments::get_comment(conn, id)
    })?;
    if existing.user_id != user.id && !user.is_admin {
        return Err(LificError::BadRequest(
            "you can only edit your own comments".into(),
        ));
    }

    with_write(&db, |conn| {
        crate::db::queries::comments::update_comment(conn, id, &input.content)
    })
    .map(Json)
}

pub(super) async fn delete_comment_handler(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Extension(auth_user): Extension<Option<AuthUser>>,
) -> Result<Json<serde_json::Value>, LificError> {
    let user = auth_user.ok_or_else(|| LificError::BadRequest("authentication required".into()))?;

    // Check ownership: only the author or an admin can delete
    let existing = with_read(&db, |conn| {
        crate::db::queries::comments::get_comment(conn, id)
    })?;
    if existing.user_id != user.id && !user.is_admin {
        return Err(LificError::BadRequest(
            "you can only delete your own comments".into(),
        ));
    }

    with_write(&db, |conn| {
        crate::db::queries::comments::delete_comment(conn, id)
    })?;
    Ok(Json(serde_json::json!({"deleted": true})))
}

#[cfg(test)]
mod tests {
    use crate::api::test_helpers::*;
    use crate::db::models::*;
    use axum::Extension;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    /// Set up a test app with a user, project, and issue pre-seeded.
    /// Returns (app_with_user_extension, issue_id, user_id).
    fn setup_comment_test() -> (axum::Router, i64, i64) {
        let db = crate::db::open_memory().expect("test db");
        let conn = db.write().unwrap();

        let user = crate::db::queries::users::create_user(
            &conn,
            &CreateUser {
                username: "commenter".into(),
                email: "c@test.com".into(),
                password: "testpassword1".into(),
                display_name: Some("Commenter".into()),
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap();

        let project = crate::db::queries::create_project(
            &conn,
            &CreateProject {
                name: "Test".into(),
                identifier: "TST".into(),
                description: String::new(),
                emoji: None,
                lead_user_id: None,
            },
        )
        .unwrap();

        let issue = crate::db::queries::create_issue(
            &conn,
            &CreateIssue {
                project_id: project.id,
                title: "Comment test issue".into(),
                description: String::new(),
                status: "todo".into(),
                priority: "medium".into(),
                module_id: None,
                start_date: None,
                target_date: None,
                labels: vec![],
            },
        )
        .unwrap();

        drop(conn);

        let app = crate::api::router(db)
            .layer(Extension(crate::config::AuthConfig { allow_signup: true }))
            .layer(Extension(Some(AuthUser {
                id: user.id,
                username: user.username.clone(),
                display_name: user.display_name.clone(),
                is_admin: user.is_admin,
            })));

        (app, issue.id, user.id)
    }

    #[tokio::test]
    async fn comment_create_and_list() {
        let (app, issue_id, _) = setup_comment_test();

        // Create a comment
        let body = serde_json::json!({"content": "Hello from test"});
        let resp = json_post(&app, &format!("/api/issues/{issue_id}/comments"), body).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        assert_eq!(data["content"], "Hello from test");
        assert_eq!(data["author"], "commenter");

        // Create another
        let body = serde_json::json!({"content": "Second comment"});
        json_post(&app, &format!("/api/issues/{issue_id}/comments"), body).await;

        // List
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/issues/{issue_id}/comments"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        let comments = data.as_array().unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0]["content"], "Hello from test");
        assert_eq!(comments[1]["content"], "Second comment");
    }

    #[tokio::test]
    async fn comment_edit_own() {
        let (app, issue_id, _) = setup_comment_test();

        let body = serde_json::json!({"content": "Original"});
        let resp = json_post(&app, &format!("/api/issues/{issue_id}/comments"), body).await;
        let data = parse_json(resp).await;
        let comment_id = data["id"].as_i64().unwrap();

        // Edit it
        let body = serde_json::json!({"content": "Edited"});
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/comments/{comment_id}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        assert_eq!(data["content"], "Edited");
    }

    #[tokio::test]
    async fn comment_delete_own() {
        let (app, issue_id, _) = setup_comment_test();

        let body = serde_json::json!({"content": "Delete me"});
        let resp = json_post(&app, &format!("/api/issues/{issue_id}/comments"), body).await;
        let data = parse_json(resp).await;
        let comment_id = data["id"].as_i64().unwrap();

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/comments/{comment_id}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn comment_edit_other_rejected() {
        let db = crate::db::open_memory().expect("test db");
        let conn = db.write().unwrap();

        let owner = crate::db::queries::users::create_user(
            &conn,
            &CreateUser {
                username: "owner".into(),
                email: "owner@test.com".into(),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap();

        let other = crate::db::queries::users::create_user(
            &conn,
            &CreateUser {
                username: "other".into(),
                email: "other@test.com".into(),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap();

        let project = crate::db::queries::create_project(
            &conn,
            &CreateProject {
                name: "Test".into(),
                identifier: "TST".into(),
                description: String::new(),
                emoji: None,
                lead_user_id: None,
            },
        )
        .unwrap();

        let issue = crate::db::queries::create_issue(
            &conn,
            &CreateIssue {
                project_id: project.id,
                title: "Test".into(),
                description: String::new(),
                status: "todo".into(),
                priority: "medium".into(),
                module_id: None,
                start_date: None,
                target_date: None,
                labels: vec![],
            },
        )
        .unwrap();

        // Owner creates a comment
        let comment =
            crate::db::queries::comments::create_comment(&conn, issue.id, owner.id, "Mine")
                .unwrap();
        drop(conn);

        // Build app as "other" (non-owner, non-admin)
        let app = crate::api::router(db)
            .layer(Extension(crate::config::AuthConfig { allow_signup: true }))
            .layer(Extension(Some(AuthUser {
                id: other.id,
                username: other.username,
                display_name: other.display_name,
                is_admin: false,
            })));

        // Try to edit owner's comment
        let body = serde_json::json!({"content": "Hijacked"});
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/comments/{}", comment.id))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // Try to delete owner's comment
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/comments/{}", comment.id))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn comment_admin_can_delete_others() {
        let db = crate::db::open_memory().expect("test db");
        let conn = db.write().unwrap();

        let regular = crate::db::queries::users::create_user(
            &conn,
            &CreateUser {
                username: "regular".into(),
                email: "reg@test.com".into(),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap();

        let admin = crate::db::queries::users::create_user(
            &conn,
            &CreateUser {
                username: "admin".into(),
                email: "admin@test.com".into(),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: true,
                is_bot: false,
            },
        )
        .unwrap();

        let project = crate::db::queries::create_project(
            &conn,
            &CreateProject {
                name: "Test".into(),
                identifier: "TST".into(),
                description: String::new(),
                emoji: None,
                lead_user_id: None,
            },
        )
        .unwrap();

        let issue = crate::db::queries::create_issue(
            &conn,
            &CreateIssue {
                project_id: project.id,
                title: "Test".into(),
                description: String::new(),
                status: "todo".into(),
                priority: "medium".into(),
                module_id: None,
                start_date: None,
                target_date: None,
                labels: vec![],
            },
        )
        .unwrap();

        let comment = crate::db::queries::comments::create_comment(
            &conn,
            issue.id,
            regular.id,
            "Regular user's comment",
        )
        .unwrap();
        drop(conn);

        // Build app as admin
        let app = crate::api::router(db)
            .layer(Extension(crate::config::AuthConfig { allow_signup: true }))
            .layer(Extension(Some(AuthUser {
                id: admin.id,
                username: admin.username,
                display_name: admin.display_name,
                is_admin: true,
            })));

        // Admin can delete regular user's comment
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/comments/{}", comment.id))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
