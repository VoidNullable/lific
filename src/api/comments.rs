use axum::{
    Extension,
    extract::{Json, Path, State},
};

use crate::authz;
use crate::db::queries::comments::{self, CommentParent};
use crate::db::{DbPool, models::*};
use crate::error::LificError;

use super::{with_read, with_write};

/// LIF-263: build the visible mention-candidate set for a comment's project,
/// then create the comment and record its resolved mentions in one write.
///
/// The `authz_enforced` flag is read *before* the write lock is taken (it
/// opens its own read connection) to avoid nesting the pool's guards.
fn create_comment_with_mentions(
    db: &DbPool,
    parent: CommentParent,
    project_id: Option<i64>,
    user_id: i64,
    content: &str,
) -> Result<Comment, LificError> {
    let member_scoped = authz::authz_enforced(db)?;
    with_write(db, |conn| {
        let candidates = comments::mention_candidates(conn, project_id, member_scoped)?;
        let comment = comments::create_comment(conn, parent, user_id, content)?;
        comments::sync_mentions(conn, comment.id, &comment.content, &candidates)?;
        // LIF-262: link attachments referenced in the comment body.
        super::attachments::sync_links(conn, AttachmentEntity::Comment, comment.id, &comment.content)?;
        Ok(comment)
    })
}

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

    create_comment_with_mentions(
        &db,
        CommentParent::Issue(issue_id),
        Some(project_id),
        user.id,
        &input.content,
    )
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

    create_comment_with_mentions(
        &db,
        CommentParent::Page(page_id),
        project_id,
        user.id,
        &input.content,
    )
    .map(Json)
}

/// GET /api/projects/{id}/mention-candidates — the users who may be
/// `@`-mentioned in this project's comments. Viewer-gated (same as reading
/// any project data); a non-member is denied when enforcement is on, and the
/// candidate list itself is member-scoped in that mode so it never leaks a
/// user who can't see the project. Powers the composer autocomplete.
pub(super) async fn mention_candidates(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path(project_id): Path<i64>,
) -> Result<Json<Vec<MentionCandidate>>, LificError> {
    authz::require_role(&db, &auth_user, project_id, Role::Viewer)?;
    let member_scoped = authz::authz_enforced(&db)?;
    with_read(&db, |conn| {
        comments::mention_candidates(conn, Some(project_id), member_scoped)
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

    // LIF-263: recompute the mention set against the parent project's
    // visible members. Resolve the project from the comment's parent
    // (issue_id XOR page_id) — a page comment may have a NULL project
    // (workspace page), which `mention_candidates` handles.
    let project_id = with_read(&db, |conn| resolve_comment_project(conn, &existing))?;
    let member_scoped = authz::authz_enforced(&db)?;

    with_write(&db, |conn| {
        let candidates = comments::mention_candidates(conn, project_id, member_scoped)?;
        let comment = comments::update_comment(conn, id, &input.content)?;
        comments::sync_mentions(conn, comment.id, &comment.content, &candidates)?;
        // LIF-262: re-scan the edited comment and reconcile links.
        super::attachments::sync_links(conn, AttachmentEntity::Comment, comment.id, &comment.content)?;
        Ok(comment)
    })
    .map(Json)
}

/// Resolve the project a comment belongs to, via its issue or page parent.
fn resolve_comment_project(
    conn: &rusqlite::Connection,
    comment: &Comment,
) -> Result<Option<i64>, LificError> {
    if let Some(issue_id) = comment.issue_id {
        Ok(Some(crate::db::queries::get_issue(conn, issue_id)?.project_id))
    } else if let Some(page_id) = comment.page_id {
        Ok(crate::db::queries::get_page(conn, page_id)?.project_id)
    } else {
        Ok(None)
    }
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

    // ── LIF-263: @mentions ──────────────────────────────────────────

    /// Read the recorded mention user_ids for a comment straight from the DB.
    fn mention_ids(db: &crate::db::DbPool, comment_id: i64) -> Vec<i64> {
        let conn = db.read().unwrap();
        crate::db::queries::comments::list_mention_user_ids(&conn, comment_id).unwrap()
    }

    #[tokio::test]
    async fn creating_a_comment_records_mentions_and_activity() {
        let db = crate::db::open_memory().expect("test db");
        let (author, mentioned, issue_id, project_id) = {
            let conn = db.write().unwrap();
            let author = crate::db::queries::users::create_user(
                &conn,
                &CreateUser {
                    username: "author".into(),
                    email: "author@test.com".into(),
                    password: "testpassword1".into(),
                    display_name: Some("Author".into()),
                    is_admin: false,
                    is_bot: false,
                },
            )
            .unwrap();
            let mentioned = crate::db::queries::users::create_user(
                &conn,
                &CreateUser {
                    username: "ada".into(),
                    email: "ada@test.com".into(),
                    password: "testpassword1".into(),
                    display_name: Some("Ada L".into()),
                    is_admin: false,
                    is_bot: false,
                },
            )
            .unwrap();
            let project = crate::db::queries::create_project(
                &conn,
                &CreateProject {
                    name: "Mentions".into(),
                    identifier: "MEN".into(),
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
                    title: "Mention target".into(),
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
            (author, mentioned, issue.id, project.id)
        };

        let app = app_as_user(db.clone(), &author);

        // Post a comment mentioning @ada plus an unmatched @ghost.
        let resp = json_post(
            &app,
            &format!("/api/issues/{issue_id}/comments"),
            serde_json::json!({"content": "hey @ada and @ghost, look here"}),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        let comment_id = data["id"].as_i64().unwrap();
        // Body is stored verbatim, tokens intact.
        assert!(data["content"].as_str().unwrap().contains("@ada"));
        assert!(data["content"].as_str().unwrap().contains("@ghost"));

        // Only @ada resolved.
        assert_eq!(mention_ids(&db, comment_id), vec![mentioned.id]);

        // The project activity feed carries a "mention" row for ada.
        let feed = parse_json(
            json_get(&app, &format!("/api/projects/{project_id}/activity?limit=100")).await,
        )
        .await;
        let items = feed["items"].as_array().unwrap();
        assert!(
            items.iter().any(|a| a["action"] == "mention" && a["new_value"] == "ada"),
            "expected a mention activity row: {items:#?}"
        );
    }

    #[tokio::test]
    async fn editing_a_comment_recomputes_mentions() {
        let db = crate::db::open_memory().expect("test db");
        let (author, ada, bob, issue_id2) = {
            let conn = db.write().unwrap();
            let author = crate::db::queries::users::create_user(
                &conn,
                &CreateUser {
                    username: "author".into(),
                    email: "author@test.com".into(),
                    password: "testpassword1".into(),
                    display_name: None,
                    is_admin: false,
                    is_bot: false,
                },
            )
            .unwrap();
            let ada = crate::db::queries::users::create_user(
                &conn,
                &CreateUser {
                    username: "ada".into(),
                    email: "ada@test.com".into(),
                    password: "testpassword1".into(),
                    display_name: None,
                    is_admin: false,
                    is_bot: false,
                },
            )
            .unwrap();
            let bob = crate::db::queries::users::create_user(
                &conn,
                &CreateUser {
                    username: "bob".into(),
                    email: "bob@test.com".into(),
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
                    name: "Edit".into(),
                    identifier: "EDT".into(),
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
                    title: "Edit target".into(),
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
            (author, ada, bob, issue.id)
        };

        let app = app_as_user(db.clone(), &author);

        let resp = json_post(
            &app,
            &format!("/api/issues/{issue_id2}/comments"),
            serde_json::json!({"content": "ping @ada"}),
        )
        .await;
        let comment_id = parse_json(resp).await["id"].as_i64().unwrap();
        assert_eq!(mention_ids(&db, comment_id), vec![ada.id]);

        // Edit to mention bob instead.
        let resp = json_put(
            &app,
            &format!("/api/comments/{comment_id}"),
            serde_json::json!({"content": "actually @bob"}),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(mention_ids(&db, comment_id), vec![bob.id]);

        // Edit to mention nobody.
        json_put(
            &app,
            &format!("/api/comments/{comment_id}"),
            serde_json::json!({"content": "never mind"}),
        )
        .await;
        assert!(mention_ids(&db, comment_id).is_empty());
    }

    #[tokio::test]
    async fn mention_candidates_endpoint_open_lists_all_non_bots() {
        let (app, _issue_id, _) = setup_comment_test();
        // setup_comment_test creates exactly one project (id 1) in a fresh
        // in-memory DB. Flag is OFF (default), so all non-bot users list.
        let resp = json_get(&app, "/api/projects/1/mention-candidates").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let cands = parse_json(resp).await;
        let arr = cands.as_array().unwrap();
        assert!(arr.iter().any(|c| c["username"] == "commenter"));
    }

    #[tokio::test]
    async fn authz_scoping_excludes_non_member_from_candidates_and_resolution() {
        // Enforcement ON: only members are candidates, and a mention of a
        // non-member never resolves.
        let (db, _admin, lead, _maintainer, _viewer, non_member, project_id) =
            setup_membership_test();

        // An issue in the enforced project.
        let issue_id = {
            let conn = db.write().unwrap();
            crate::db::queries::create_issue(
                &conn,
                &CreateIssue {
                    project_id,
                    title: "Enforced".into(),
                    description: String::new(),
                    status: "todo".into(),
                    priority: "medium".into(),
                    module_id: None,
                    start_date: None,
                    target_date: None,
                    labels: vec![],
                },
            )
            .unwrap()
            .id
        };

        let lead_app = app_as_user(db.clone(), &lead);

        // Candidates: the non_member (not a project member) must be absent;
        // the viewer/maintainer/lead members present.
        let resp = json_get(&lead_app, &format!("/api/projects/{project_id}/mention-candidates")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let cands = parse_json(resp).await;
        let arr = cands.as_array().unwrap();
        assert!(arr.iter().any(|c| c["username"] == "lead"));
        assert!(arr.iter().any(|c| c["username"] == "viewer"));
        assert!(
            !arr.iter().any(|c| c["username"] == "non_member"),
            "non-member must not be a candidate: {arr:#?}"
        );

        // Resolution: the lead comments mentioning the non_member — it must
        // NOT resolve (stays literal), while a real member (viewer) does.
        let resp = json_post(
            &lead_app,
            &format!("/api/issues/{issue_id}/comments"),
            serde_json::json!({"content": "@non_member and @viewer"}),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let comment_id = parse_json(resp).await["id"].as_i64().unwrap();

        let resolved = mention_ids(&db, comment_id);
        assert!(!resolved.contains(&non_member.id), "non-member must not resolve");
        // viewer resolves.
        let viewer_id = {
            let conn = db.read().unwrap();
            crate::db::queries::users::get_user_by_username(&conn, "viewer").unwrap().id
        };
        assert_eq!(resolved, vec![viewer_id]);
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
