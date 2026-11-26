//! LIF-583: a published project read at its ordinary routes, over the
//! production middleware stack.
//!
//! Three callers matter: a visitor with no credential at all, a signed-in
//! account that is not a member of anything, and the owner. The visitor and
//! the stranger read the published project; neither writes to it; neither
//! learns anything about the private one, including that it exists.

use super::*;
use crate::db::models::*;
use axum::body::Body;
use http_body_util::BodyExt;
use tower::ServiceExt;

struct Deployed {
    app: Router,
    _store_guard: tempfile::TempDir,
    published: i64,
    private: i64,
    issue: i64,
    private_issue: i64,
    page: i64,
    plan: i64,
    attachment: i64,
    owner_session: String,
    owner_key: String,
    stranger_session: String,
    reader_session: String,
}

/// Enforced authorization, auth required. `PUB` is published and holds an
/// issue with an assignee, a comment, a page, a plan whose step links an
/// issue in `PRIV`, and an attachment. `PRIV` holds a secret.
fn deploy() -> Deployed {
    let pool = db::open_memory().expect("test db");
    let tmp = tempfile::tempdir().expect("attachment tempdir");
    let store = storage::AttachmentStore::new(tmp.path().to_path_buf());

    let seeded = {
        let conn = pool.write().unwrap();
        db::queries::settings::update(
            &conn,
            db::queries::settings::InstanceSettingsPatch {
                authz_enforced: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        conn.execute(
            "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot)
             VALUES ('owner', 'owner@test.local', 'x', 'Owner', 1, 0),
                    ('stranger', 'stranger@test.local', 'x', 'Stranger', 0, 0),
                    ('reader', 'reader@test.local', 'x', 'Reader', 0, 0)",
            [],
        )
        .unwrap();
        let project = |name: &str, identifier: &str| {
            let project = db::queries::create_project(
                &conn,
                &CreateProject {
                    name: name.into(),
                    identifier: identifier.into(),
                    ..Default::default()
                },
            )
            .unwrap();
            db::queries::members::upsert_member(&conn, project.id, 1, Role::Lead).unwrap();
            project.id
        };
        let published = project("Published", "PUB");
        let private = project("Private", "PRIV");
        conn.execute(
            "UPDATE projects SET is_public = 1 WHERE id = ?1",
            [published],
        )
        .unwrap();

        let issue = db::queries::create_issue(
            &conn,
            &CreateIssue {
                project_id: published,
                title: "Public issue".into(),
                description: "A body anyone may read".into(),
                assignees: Some(vec!["owner".into()]),
                ..Default::default()
            },
        )
        .unwrap();
        let private_issue = db::queries::create_issue(
            &conn,
            &CreateIssue {
                project_id: private,
                title: "Private issue".into(),
                description: "classified".into(),
                ..Default::default()
            },
        )
        .unwrap();
        db::queries::comments::create_comment_with_mentions(
            &conn,
            db::queries::comments::CommentParent::Issue(issue.id),
            None,
            CommentActor {
                user_id: 1,
                is_admin: true,
            },
            AttachmentActor::TrustedLocal,
            "A public comment",
            false,
        )
        .unwrap();
        let page = db::queries::create_page(
            &conn,
            &serde_json::from_value(serde_json::json!({
                "project_id": published,
                "title": "Public page",
                "content": "Readable notes",
            }))
            .unwrap(),
        )
        .unwrap();
        let plan = db::queries::plans::create_plan(
            &conn,
            &serde_json::from_value(serde_json::json!({
                "project_id": published,
                "title": "Public plan",
                "steps": [
                    { "title": "Readable step", "issue_id": issue.id },
                    { "title": "Step on a private issue", "issue_id": private_issue.id },
                ],
            }))
            .unwrap(),
        )
        .unwrap();

        let bytes = b"public attachment bytes".to_vec();
        let sha = store.write(&bytes).unwrap();
        let attachment = db::queries::attachments::create_attachment(
            &conn,
            &sha,
            "spec.pdf",
            "application/pdf",
            bytes.len() as i64,
            None,
        )
        .unwrap();
        db::queries::attachments::link_attachment(
            &conn,
            attachment.id,
            AttachmentEntity::Issue,
            issue.id,
        )
        .unwrap();

        let owner_session = db::queries::users::create_session(&conn, 1, None)
            .unwrap()
            .token;
        let stranger_session = db::queries::users::create_session(&conn, 2, None)
            .unwrap()
            .token;

        // Private history that touches the published issue: a private plan
        // whose step links it, and a relation from it into PRIV.
        db::queries::plans::create_plan(
            &conn,
            &serde_json::from_value(serde_json::json!({
                "project_id": private,
                "title": "PRIVATE PLAN TITLE",
                "steps": [{ "title": "PRIVATE STEP TITLE", "issue_id": issue.id }],
            }))
            .unwrap(),
        )
        .unwrap();
        db::queries::link_issues(&conn, issue.id, private_issue.id, "blocks").unwrap();
        // A Viewer of PUB who is not a member of PRIV.
        db::queries::members::upsert_member(&conn, published, 3, Role::Viewer).unwrap();
        let reader_session = db::queries::users::create_session(&conn, 3, None)
            .unwrap()
            .token;
        (
            published,
            private,
            issue.id,
            private_issue.id,
            page.id,
            plan.id,
            attachment.id,
            owner_session,
            stranger_session,
            reader_session,
        )
    };
    let owner_key = crate::auth::create_api_key(&pool, "owner-key", Some(1)).unwrap();

    let mut cfg = Config::default();
    cfg.auth.required = true;
    cfg.server.host = "127.0.0.1".into();
    let trusted_proxies =
        Arc::<[ratelimit::IpNetwork]>::from(cfg.server.trusted_proxy_ranges().expect("proxies"));
    let app = build_app_with_store(
        &cfg,
        pool,
        realtime::RealtimeHub::new(),
        trusted_proxies,
        store,
    );

    let (
        published,
        private,
        issue,
        private_issue,
        page,
        plan,
        attachment,
        owner_session,
        stranger_session,
        reader_session,
    ) = seeded;
    Deployed {
        app,
        _store_guard: tmp,
        published,
        private,
        issue,
        private_issue,
        page,
        plan,
        attachment,
        owner_session,
        owner_key,
        stranger_session,
        reader_session,
    }
}

impl Deployed {
    /// Every anonymous route, filled in with the published project's ids.
    fn published_reads(&self) -> Vec<String> {
        let (p, i, g, n, a) = (
            self.published,
            self.issue,
            self.page,
            self.plan,
            self.attachment,
        );
        vec![
            "/api/projects".into(),
            format!("/api/projects/{p}"),
            format!("/api/projects/{p}/index"),
            format!("/api/projects/{p}/changes"),
            format!("/api/projects/{p}/board"),
            format!("/api/projects/{p}/issue-counts"),
            format!("/api/projects/{p}/relations"),
            format!("/api/projects/{p}/activity"),
            format!("/api/projects/{p}/activity/actors"),
            format!("/api/projects/{p}/insights"),
            format!("/api/projects/{p}/my-role"),
            format!("/api/projects/{p}/attachments"),
            format!("/api/issues?project_id={p}"),
            format!("/api/issues/{i}"),
            "/api/issues/resolve/PUB-1".into(),
            format!("/api/issues/{i}/comments"),
            format!("/api/issues/{i}/activity"),
            format!("/api/issues/{i}/waits"),
            format!("/api/pages?project_id={p}"),
            format!("/api/pages/{g}"),
            "/api/pages/resolve/PUB-DOC-1".into(),
            format!("/api/pages/{g}/comments"),
            format!("/api/pages/{g}/activity"),
            format!("/api/plans?project_id={p}"),
            format!("/api/plans/{n}"),
            "/api/plans/resolve/PUB-PLAN-1".into(),
            format!("/api/plans/{n}/activity"),
            format!("/api/modules?project_id={p}"),
            format!("/api/labels?project_id={p}"),
            format!("/api/folders?project_id={p}"),
            format!("/api/attachments?entity_type=issue&entity_id={i}"),
            format!("/api/attachments/{a}"),
            format!("/api/attachments/{a}/links"),
            "/api/search?query=issue".into(),
            "/api/clock".into(),
        ]
    }
}

/// Deliberately sends no `Authorization` header and no cookie.
async fn anonymous(app: &Router, method: &str, uri: &str) -> Response {
    send(app, method, uri, None, None).await
}

async fn bearer(app: &Router, token: &str, method: &str, uri: &str) -> Response {
    send(app, method, uri, Some(token), Some(serde_json::json!({}))).await
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<serde_json::Value>,
) -> Response {
    let mut request = axum::http::Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let body = match body {
        Some(json) if method != "GET" => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(json.to_string())
        }
        _ => Body::empty(),
    };
    app.clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap()
}

async fn body_string(response: Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).to_string()
}

#[tokio::test]
async fn a_visitor_reads_every_part_of_a_published_project() {
    let d = deploy();
    for uri in d.published_reads() {
        let response = anonymous(&d.app, "GET", &uri).await;
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        let headers = response.headers();
        for (name, value) in [
            ("cache-control", "no-store, no-cache, must-revalidate"),
            ("x-content-type-options", "nosniff"),
            ("referrer-policy", "no-referrer"),
            ("x-frame-options", "DENY"),
            ("cross-origin-resource-policy", "same-origin"),
        ] {
            assert_eq!(
                headers.get(name).and_then(|v| v.to_str().ok()),
                Some(value),
                "{uri}: {name}"
            );
        }
    }
}

#[tokio::test]
async fn a_visitor_sees_only_published_projects_in_listings() {
    let d = deploy();
    let projects = body_string(anonymous(&d.app, "GET", "/api/projects").await).await;
    assert!(projects.contains("\"PUB\""), "{projects}");
    assert!(!projects.contains("PRIV"), "{projects}");
    for uri in [
        "/api/issues".to_string(),
        "/api/pages".to_string(),
        "/api/plans".to_string(),
        "/api/search?query=classified".to_string(),
        "/api/search?query=issue".to_string(),
    ] {
        let body = body_string(anonymous(&d.app, "GET", &uri).await).await;
        assert!(!body.contains("classified"), "{uri}: {body}");
        assert!(!body.contains("Private issue"), "{uri}: {body}");
    }
}

/// An unpublished project answers a visitor exactly as a project that was
/// never created does, at every kind of route.
#[tokio::test]
async fn an_unpublished_project_reads_like_a_missing_one() {
    let d = deploy();
    let pairs = [
        (
            format!("/api/projects/{}", d.private),
            "/api/projects/987654".to_string(),
        ),
        (
            format!("/api/projects/{}/index", d.private),
            "/api/projects/987654/index".to_string(),
        ),
        (
            format!("/api/issues/{}", d.private_issue),
            "/api/issues/987654".to_string(),
        ),
        (
            "/api/issues/resolve/PRIV-1".to_string(),
            "/api/issues/resolve/NOPE-1".to_string(),
        ),
        (
            format!("/api/issues/{}/comments", d.private_issue),
            "/api/issues/987654/comments".to_string(),
        ),
    ];
    for (hidden, missing) in pairs {
        let hidden_response = anonymous(&d.app, "GET", &hidden).await;
        let missing_response = anonymous(&d.app, "GET", &missing).await;
        assert_eq!(hidden_response.status(), StatusCode::NOT_FOUND, "{hidden}");
        assert_eq!(
            missing_response.status(),
            StatusCode::NOT_FOUND,
            "{missing}"
        );
        assert_eq!(
            body_string(hidden_response).await,
            body_string(missing_response).await,
            "{hidden} vs {missing}"
        );
    }
}

/// Every route outside the read allowlist keeps answering 401 to a visitor,
/// writes included, whichever project they name.
#[tokio::test]
async fn every_write_and_every_member_only_read_is_401_signed_out() {
    let d = deploy();
    let (p, i) = (d.published, d.issue);
    for (method, uri) in [
        ("POST", "/api/issues".to_string()),
        ("PUT", format!("/api/issues/{i}")),
        ("DELETE", format!("/api/issues/{i}")),
        ("POST", format!("/api/issues/{i}/comments")),
        ("POST", format!("/api/issues/{i}/waits")),
        ("POST", "/api/pages".to_string()),
        ("PUT", format!("/api/pages/{}", d.page)),
        ("POST", "/api/plans".to_string()),
        ("PUT", format!("/api/projects/{p}")),
        ("DELETE", format!("/api/projects/{p}")),
        ("POST", "/api/attachments".to_string()),
        ("POST", "/api/labels".to_string()),
        ("GET", format!("/api/projects/{p}/members")),
        ("GET", format!("/api/projects/{p}/mention-candidates")),
        ("GET", format!("/api/projects/{p}/views")),
        ("GET", "/api/export/projects/PUB".to_string()),
        ("GET", "/api/users".to_string()),
        ("GET", "/api/auth/me".to_string()),
        ("GET", "/api/instance/settings".to_string()),
        ("GET", "/api/home/overview".to_string()),
    ] {
        let response = anonymous(&d.app, method, &uri).await;
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {uri}"
        );
    }
}

/// A signed-in account with no role on the published project reads it, and
/// every write to it is a 403.
#[tokio::test]
async fn a_signed_in_non_member_reads_but_cannot_write() {
    let d = deploy();
    let token = d.stranger_session.clone();
    for uri in d.published_reads() {
        let response = bearer(&d.app, &token, "GET", &uri).await;
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
    }
    let (p, i) = (d.published, d.issue);
    for (method, uri, body) in [
        (
            "POST",
            "/api/issues".to_string(),
            serde_json::json!({ "project_id": p, "title": "x" }),
        ),
        (
            "PUT",
            format!("/api/issues/{i}"),
            serde_json::json!({ "title": "x" }),
        ),
        ("DELETE", format!("/api/issues/{i}"), serde_json::json!({})),
        (
            "POST",
            format!("/api/issues/{i}/comments"),
            serde_json::json!({ "content": "x" }),
        ),
        (
            "POST",
            "/api/pages".to_string(),
            serde_json::json!({ "project_id": p, "title": "x" }),
        ),
        (
            "PUT",
            format!("/api/projects/{p}"),
            serde_json::json!({ "name": "x" }),
        ),
        (
            "POST",
            "/api/labels".to_string(),
            serde_json::json!({ "project_id": p, "name": "x", "color": "#000000" }),
        ),
    ] {
        let response = send(&d.app, method, &uri, Some(&token), Some(body)).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {uri}");
    }
    // The roster stays with the members.
    let members = bearer(&d.app, &token, "GET", &format!("/api/projects/{p}/members")).await;
    assert_eq!(members.status(), StatusCode::FORBIDDEN);
    // And the private project stays private.
    let hidden = bearer(
        &d.app,
        &token,
        "GET",
        &format!("/api/issues/{}", d.private_issue),
    )
    .await;
    assert!(matches!(
        hidden.status(),
        StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
    ));
}

/// Nothing a visitor can fetch carries an email address, a session token or
/// an API key.
#[tokio::test]
async fn no_anonymous_response_carries_an_email_or_a_token() {
    let d = deploy();
    for uri in d.published_reads() {
        let body = body_string(anonymous(&d.app, "GET", &uri).await).await;
        for secret in [
            "@test.local",
            d.owner_session.as_str(),
            d.stranger_session.as_str(),
            d.owner_key.as_str(),
            "lific_",
        ] {
            assert!(!body.contains(secret), "{uri} leaks {secret}: {body}");
        }
    }
}

/// A published plan does not name the private issue one of its steps links.
#[tokio::test]
async fn a_published_plan_hides_steps_linked_into_a_private_project() {
    let d = deploy();
    let plan = body_string(anonymous(&d.app, "GET", &format!("/api/plans/{}", d.plan)).await).await;
    assert!(plan.contains("PUB-1"), "{plan}");
    assert!(plan.contains("Step on a private issue"), "{plan}");
    assert!(!plan.contains("PRIV-1"), "{plan}");
}

/// The owner still sees everything, through a key as through a session.
#[tokio::test]
async fn the_owner_is_unaffected() {
    let d = deploy();
    for token in [&d.owner_key, &d.owner_session] {
        let plan =
            body_string(bearer(&d.app, token, "GET", &format!("/api/plans/{}", d.plan)).await)
                .await;
        assert!(plan.contains("PRIV-1"), "{plan}");
        let response = bearer(
            &d.app,
            token,
            "GET",
            &format!("/api/issues/{}", d.private_issue),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }
}

#[tokio::test]
async fn anonymous_reads_are_rate_limited_and_signed_in_reads_are_not() {
    let d = deploy();
    let mut refused = None;
    for _ in 0..(crate::api::anonymous::READS_PER_MINUTE + 50) {
        let response = anonymous(&d.app, "GET", "/api/projects").await;
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            refused = Some(response);
            break;
        }
    }
    let refused = refused.expect("every anonymous read in a row past the limit was served");
    assert!(refused.headers().get(header::RETRY_AFTER).is_some());
    assert_eq!(
        refused
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store, no-cache, must-revalidate"),
        "a refusal leaves through the same hardening"
    );
    let signed_in = bearer(&d.app, &d.stranger_session, "GET", "/api/projects").await;
    assert_eq!(signed_in.status(), StatusCode::OK);
}

#[tokio::test]
async fn an_anonymous_download_is_bounded_and_sandboxed() {
    let d = deploy();
    let response = anonymous(&d.app, "GET", &format!("/api/attachments/{}", d.attachment)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers();
    assert_eq!(
        headers.get(header::CONTENT_SECURITY_POLICY).unwrap(),
        "default-src 'none'; sandbox"
    );
    assert_eq!(
        headers.get(header::CACHE_CONTROL).unwrap(),
        "no-store, no-cache, must-revalidate"
    );
    assert!(
        headers
            .get(header::CONTENT_DISPOSITION)
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("attachment;"),
        "a PDF downloads rather than opening"
    );
    assert_eq!(body_string(response).await, "public attachment bytes");
}

#[tokio::test]
async fn an_authenticated_attachment_download_uses_the_scoped_store() {
    let d = deploy();
    let response = d
        .app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/api/attachments/{}", d.attachment))
                .header("cookie", format!("lific_token={}", d.owner_session))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_string(response).await, "public attachment bytes");
}

/// Unpublishing closes the project to visitors on the very next request.
#[tokio::test]
async fn unpublishing_closes_the_project_at_once() {
    let d = deploy();
    let uri = format!("/api/issues/{}", d.issue);
    assert_eq!(
        anonymous(&d.app, "GET", &uri).await.status(),
        StatusCode::OK
    );
    let response = send(
        &d.app,
        "PUT",
        &format!("/api/projects/{}", d.published),
        Some(&d.owner_key),
        Some(serde_json::json!({ "is_public": false })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        anonymous(&d.app, "GET", &uri).await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn an_old_public_link_redirects_to_the_normal_one() {
    let d = deploy();
    for (old, new) in [
        ("/public/PUB/issues/PUB-1", "/PUB/issues/PUB-1"),
        ("/public/PUB", "/PUB/issues"),
        ("/public/PUB/", "/PUB/issues"),
        ("/public/PUB/PUB-1", "/PUB/issues/PUB-1"),
        ("/public/PUB/pages/PUB-DOC-1", "/PUB/pages/PUB-DOC-1"),
        ("/public/PUB/issues?status=todo", "/PUB/issues?status=todo"),
    ] {
        let response = anonymous(&d.app, "GET", old).await;
        assert_eq!(response.status(), StatusCode::MOVED_PERMANENTLY, "{old}");
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            new,
            "{old}"
        );
    }
    let gone = anonymous(&d.app, "GET", "/public/api/projects/PUB").await;
    assert_eq!(gone.status(), StatusCode::NOT_FOUND);
}

#[test]
fn a_legacy_public_path_never_redirects_off_site() {
    assert_eq!(legacy_public_location("//evil.example", None), None);
    assert_eq!(legacy_public_location("/public//evil.example", None), None);
    assert_eq!(legacy_public_location("/public/%2F%2Fevil", None), None);
    assert_eq!(legacy_public_location("/public/api/x", None), None);
    assert_eq!(legacy_public_location("/publicity", None), None);
}

/// A shared link to a published project opens the app with no sign-in step;
/// the app itself decides what to render from the API above.
#[tokio::test]
async fn a_project_link_serves_the_app_signed_out() {
    let d = deploy();
    for uri in ["/PUB/issues/PUB-1", "/PRIV/issues/PRIV-1", "/"] {
        let response = anonymous(&d.app, "GET", uri).await;
        assert_ne!(response.status(), StatusCode::UNAUTHORIZED, "{uri}");
    }
}

// ── Activity: only rows the reader may see (LIF-583 review) ─────────

/// Strings that only exist in the private project's history.
const PRIVATE_HISTORY: [&str; 4] = [
    "PRIVATE PLAN TITLE",
    "PRIVATE STEP TITLE",
    "PRIV-PLAN-1",
    "PRIV-1",
];

impl Deployed {
    fn activity_feeds(&self) -> [String; 3] {
        [
            format!("/api/issues/{}/activity?limit=200", self.issue),
            format!("/api/projects/{}/activity?limit=200", self.published),
            format!("/api/plans/{}/activity?limit=200", self.plan),
        ]
    }

    /// The feed as anonymous, signed-in non-member and PUB-only Viewer.
    async fn feeds_as_readers(&self, uri: &str) -> Vec<(&'static str, String)> {
        vec![
            (
                "anonymous",
                body_string(anonymous(&self.app, "GET", uri).await).await,
            ),
            (
                "non-member",
                body_string(bearer(&self.app, &self.stranger_session, "GET", uri).await).await,
            ),
            (
                "PUB viewer",
                body_string(bearer(&self.app, &self.reader_session, "GET", uri).await).await,
            ),
        ]
    }
}

/// A private plan whose step links a published issue, and a relation from
/// the published issue into the private project, both write rows the
/// published issue's feed would otherwise return.
#[tokio::test]
async fn activity_never_shows_another_projects_private_history() {
    let d = deploy();
    for uri in d.activity_feeds() {
        for (reader, body) in d.feeds_as_readers(&uri).await {
            assert!(body.starts_with("{\"items\""), "{reader} {uri}: {body}");
            for secret in PRIVATE_HISTORY {
                assert!(
                    !body.contains(secret),
                    "{reader} {uri} leaks {secret}: {body}"
                );
            }
        }
    }
    // The relation row itself is still there, with its target hidden.
    let feed =
        body_string(anonymous(&d.app, "GET", &format!("/api/issues/{}/activity", d.issue)).await)
            .await;
    let feed: serde_json::Value = serde_json::from_str(&feed).unwrap();
    let link = feed["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["action"] == "link")
        .expect("the relation row is kept");
    assert_eq!(
        link["new_value"],
        crate::db::queries::activity::HIDDEN_ISSUE,
        "{link}"
    );
    // A plan step in PUB linked to a PRIV issue keeps its row but loses the
    // private issue id.
    let plan =
        body_string(anonymous(&d.app, "GET", &format!("/api/plans/{}/activity", d.plan)).await)
            .await;
    let plan: serde_json::Value = serde_json::from_str(&plan).unwrap();
    assert!(
        plan["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["issue_id"] != d.private_issue),
        "{plan}"
    );
}

/// Someone who may read both projects sees all of it.
#[tokio::test]
async fn activity_is_whole_for_a_reader_of_both_projects() {
    let d = deploy();
    let body = body_string(
        bearer(
            &d.app,
            &d.owner_key,
            "GET",
            &format!("/api/issues/{}/activity?limit=200", d.issue),
        )
        .await,
    )
    .await;
    // The plan's own title is on the plan's feed, not the issue's.
    for secret in &PRIVATE_HISTORY[1..] {
        assert!(body.contains(secret), "owner misses {secret}: {body}");
    }
}

/// Membership rows say who belongs to the project; publishing it does not
/// publish that. Members still see them.
#[tokio::test]
async fn membership_activity_stays_with_members() {
    let d = deploy();
    let uri = format!("/api/projects/{}/activity?limit=200", d.published);
    for (reader, body) in d.feeds_as_readers(&uri).await {
        let has_member_rows = body.contains("\"entity_type\":\"member\"");
        assert_eq!(
            has_member_rows,
            reader == "PUB viewer",
            "{reader} membership rows: {body}"
        );
    }
}
