use axum::{
    Extension,
    extract::{Json, Path, State},
};

use crate::authz;
use crate::db::queries::comments::CommentParent;
use crate::db::{DbPool, models::*};
use crate::error::LificError;

use super::{with_read, with_write};

/// LIF-197: comments are gated at `Viewer` — anyone who can see a project
/// can read and post comments on its issues/pages (the actual auth-required
/// check for *who* the comment is attributed to is separate, below).
/// Workspace-level pages (`project_id = None`) fall back to admin-only.
fn require_comment_viewer(
    db: &DbPool,
    auth_user: &Option<AuthUser>,
    project_id: Option<i64>,
) -> Result<(), LificError> {
    match project_id {
        Some(pid) => authz::require_role(db, auth_user, pid, Role::Viewer),
        None => authz::require_workspace_admin(db, auth_user),
    }
}

pub(super) async fn list_comments(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path(issue_id): Path<i64>,
) -> Result<Json<Vec<Comment>>, LificError> {
    let project_id = with_read(&db, |conn| crate::db::queries::get_issue(conn, issue_id))?
        .project_id;
    require_comment_viewer(&db, &auth_user, Some(project_id))?;
    with_read(&db, |conn| {
        crate::db::queries::comments::list_comments(conn, CommentParent::Issue(issue_id), None, None)
    })
    .map(Json)
}

pub(super) async fn create_comment(
    State(db): State<DbPool>,
    Path(issue_id): Path<i64>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<CreateComment>,
) -> Result<Json<Comment>, LificError> {
    let project_id = with_read(&db, |conn| crate::db::queries::get_issue(conn, issue_id))?
        .project_id;
    require_comment_viewer(&db, &auth_user, Some(project_id))?;

    let user = auth_user
        .ok_or_else(|| LificError::BadRequest("authentication required to comment".into()))?;

    with_write(&db, |conn| {
        crate::db::queries::comments::create_comment(
            conn,
            CommentParent::Issue(issue_id),
            user.id,
            &input.content,
        )
    })
    .map(Json)
}

pub(super) async fn list_page_comments(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path(page_id): Path<i64>,
) -> Result<Json<Vec<Comment>>, LificError> {
    let project_id = with_read(&db, |conn| crate::db::queries::get_page(conn, page_id))?
        .project_id;
    require_comment_viewer(&db, &auth_user, project_id)?;
    with_read(&db, |conn| {
        crate::db::queries::comments::list_comments(conn, CommentParent::Page(page_id), None, None)
    })
    .map(Json)
}

pub(super) async fn create_page_comment(
    State(db): State<DbPool>,
    Path(page_id): Path<i64>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<CreateComment>,
) -> Result<Json<Comment>, LificError> {
    let project_id = with_read(&db, |conn| crate::db::queries::get_page(conn, page_id))?
        .project_id;
    require_comment_viewer(&db, &auth_user, project_id)?;

    let user = auth_user
        .ok_or_else(|| LificError::BadRequest("authentication required to comment".into()))?;

    with_write(&db, |conn| {
        crate::db::queries::comments::create_comment(
            conn,
            CommentParent::Page(page_id),
            user.id,
            &input.content,
        )
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
                source: None,
            },
        )
        .unwrap();

        drop(conn);

        let app = crate::api::router(db, &[])
            .layer(Extension(crate::config::AuthConfig { allow_signup: true, secure_cookies: false }))
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

        // Scope the write guard tightly so it cannot be held across the
        // awaits below (clippy::await_holding_lock).
        let (other, comment_id) = {
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
                    source: None,
                },
            )
            .unwrap();
            let comment = crate::db::queries::comments::create_comment(
                &conn,
                crate::db::queries::comments::CommentParent::Issue(issue.id),
                owner.id,
                "Mine",
            )
            .unwrap();
            (other, comment.id)
        };

        // Build app as "other" (non-owner, non-admin)
        let app = crate::api::router(db, &[])
            .layer(Extension(crate::config::AuthConfig { allow_signup: true, secure_cookies: false }))
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
                    .uri(format!("/api/comments/{comment_id}"))
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
                    .uri(format!("/api/comments/{comment_id}"))
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

        // Scope the write guard tightly so it cannot be held across the
        // awaits below (clippy::await_holding_lock).
        let (admin, comment_id) = {
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
                    source: None,
                },
            )
            .unwrap();
            let comment = crate::db::queries::comments::create_comment(
                &conn,
                crate::db::queries::comments::CommentParent::Issue(issue.id),
                regular.id,
                "Regular user's comment",
            )
            .unwrap();
            (admin, comment.id)
        };

        // Build app as admin
        let app = crate::api::router(db, &[])
            .layer(Extension(crate::config::AuthConfig { allow_signup: true, secure_cookies: false }))
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
                    .uri(format!("/api/comments/{comment_id}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── LIF-106: page comments ─────────────────────────────────────────────

    /// Set up a test app with a user, project, and page pre-seeded.
    /// Returns (app, page_id, user_id).
    fn setup_page_comment_test() -> (axum::Router, i64, i64) {
        let db = crate::db::open_memory().expect("test db");

        let (user, page_id) = {
            let conn = db.write().unwrap();

            let user = crate::db::queries::users::create_user(
                &conn,
                &CreateUser {
                    username: "pagecommenter".into(),
                    email: "pc@test.com".into(),
                    password: "testpassword1".into(),
                    display_name: Some("Page Commenter".into()),
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

            let page = crate::db::queries::create_page(
                &conn,
                &CreatePage {
                    project_id: Some(project.id),
                    folder_id: None,
                    title: "Doc 1".into(),
                    content: "Body".into(),
                    status: "draft".into(),
                    labels: vec![],
                },
            )
            .unwrap();

            (user, page.id)
        };

        let app = crate::api::router(db, &[])
            .layer(Extension(crate::config::AuthConfig { allow_signup: true, secure_cookies: false }))
            .layer(Extension(Some(AuthUser {
                id: user.id,
                username: user.username.clone(),
                display_name: user.display_name.clone(),
                is_admin: user.is_admin,
            })));

        (app, page_id, user.id)
    }

    #[tokio::test]
    async fn page_comment_create_and_list() {
        let (app, page_id, _) = setup_page_comment_test();

        let body = serde_json::json!({"content": "Comment on the page"});
        let resp = json_post(&app, &format!("/api/pages/{page_id}/comments"), body).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        assert_eq!(data["content"], "Comment on the page");
        assert_eq!(data["author"], "pagecommenter");
        assert_eq!(data["page_id"].as_i64(), Some(page_id));
        // issue_id is serialized as null for page comments
        assert!(data["issue_id"].is_null());

        // Second comment
        let body = serde_json::json!({"content": "Another"});
        json_post(&app, &format!("/api/pages/{page_id}/comments"), body).await;

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/pages/{page_id}/comments"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        let comments = data.as_array().unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0]["content"], "Comment on the page");
        assert_eq!(comments[1]["content"], "Another");
    }

    #[tokio::test]
    async fn page_comment_edit_and_delete_own() {
        let (app, page_id, _) = setup_page_comment_test();

        let body = serde_json::json!({"content": "Original"});
        let resp = json_post(&app, &format!("/api/pages/{page_id}/comments"), body).await;
        let data = parse_json(resp).await;
        let comment_id = data["id"].as_i64().unwrap();

        // Edit via shared /api/comments/{id} endpoint — parent-agnostic.
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

        // Delete via shared endpoint
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
    async fn page_comment_edit_other_rejected() {
        let db = crate::db::open_memory().expect("test db");

        let (other, comment_id) = {
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
            let page = crate::db::queries::create_page(
                &conn,
                &CreatePage {
                    project_id: Some(project.id),
                    folder_id: None,
                    title: "Page".into(),
                    content: String::new(),
                    status: "draft".into(),
                    labels: vec![],
                },
            )
            .unwrap();
            let comment = crate::db::queries::comments::create_comment(
                &conn,
                crate::db::queries::comments::CommentParent::Page(page.id),
                owner.id,
                "Owner's page comment",
            )
            .unwrap();
            (other, comment.id)
        };

        let app = crate::api::router(db, &[])
            .layer(Extension(crate::config::AuthConfig { allow_signup: true, secure_cookies: false }))
            .layer(Extension(Some(AuthUser {
                id: other.id,
                username: other.username,
                display_name: other.display_name,
                is_admin: false,
            })));

        // Try to edit owner's page comment as a non-owner, non-admin user
        let body = serde_json::json!({"content": "Hijacked"});
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
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn page_comment_admin_can_delete_others() {
        let db = crate::db::open_memory().expect("test db");

        let (admin, comment_id) = {
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
            let page = crate::db::queries::create_page(
                &conn,
                &CreatePage {
                    project_id: Some(project.id),
                    folder_id: None,
                    title: "Page".into(),
                    content: String::new(),
                    status: "draft".into(),
                    labels: vec![],
                },
            )
            .unwrap();
            let comment = crate::db::queries::comments::create_comment(
                &conn,
                crate::db::queries::comments::CommentParent::Page(page.id),
                regular.id,
                "Regular's page comment",
            )
            .unwrap();
            (admin, comment.id)
        };

        let app = crate::api::router(db, &[])
            .layer(Extension(crate::config::AuthConfig { allow_signup: true, secure_cookies: false }))
            .layer(Extension(Some(AuthUser {
                id: admin.id,
                username: admin.username,
                display_name: admin.display_name,
                is_admin: true,
            })));

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
    async fn page_comments_dont_leak_into_issue_thread() {
        // Both a page and an issue under the same project; comments on each
        // must not appear in the other's list.
        let db = crate::db::open_memory().expect("test db");

        let (user, issue_id, page_id) = {
            let conn = db.write().unwrap();
            let user = crate::db::queries::users::create_user(
                &conn,
                &CreateUser {
                    username: "u".into(),
                    email: "u@test.com".into(),
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
                    name: "T".into(),
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
                    title: "i".into(),
                    description: String::new(),
                    status: "todo".into(),
                    priority: "medium".into(),
                    module_id: None,
                    start_date: None,
                    target_date: None,
                    labels: vec![],
                    source: None,
                },
            )
            .unwrap();
            let page = crate::db::queries::create_page(
                &conn,
                &CreatePage {
                    project_id: Some(project.id),
                    folder_id: None,
                    title: "p".into(),
                    content: String::new(),
                    status: "draft".into(),
                    labels: vec![],
                },
            )
            .unwrap();
            (user, issue.id, page.id)
        };

        let app = crate::api::router(db, &[])
            .layer(Extension(crate::config::AuthConfig { allow_signup: true, secure_cookies: false }))
            .layer(Extension(Some(AuthUser {
                id: user.id,
                username: user.username,
                display_name: user.display_name,
                is_admin: false,
            })));

        // Post one comment to the issue and one to the page.
        json_post(
            &app,
            &format!("/api/issues/{issue_id}/comments"),
            serde_json::json!({"content": "issue-only"}),
        )
        .await;
        json_post(
            &app,
            &format!("/api/pages/{page_id}/comments"),
            serde_json::json!({"content": "page-only"}),
        )
        .await;

        // Issue endpoint sees only the issue comment.
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
        let data = parse_json(resp).await;
        let comments = data.as_array().unwrap();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0]["content"], "issue-only");

        // Page endpoint sees only the page comment.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/pages/{page_id}/comments"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let data = parse_json(resp).await;
        let comments = data.as_array().unwrap();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0]["content"], "page-only");
    }
}
