//! HTTP surface of archived projects, through the real router and the real
//! authentication middleware.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use crate::api::test_helpers::{
    test_attachment_store, test_peer, with_attachment_layers_store, with_client_ip_test_layers,
};
use crate::db::DbPool;
use crate::db::models::{CreateUser, User};
use crate::storage::AttachmentStore;

struct Instance {
    db: DbPool,
    app: Router,
    store: AttachmentStore,
    _store_guard: tempfile::TempDir,
}

impl Instance {
    fn new() -> Self {
        let db = crate::db::open_memory().expect("test db");
        let (store, guard) = test_attachment_store();
        let auth_state = crate::auth::AuthState {
            db: db.clone(),
            public_url: "https://archive.test".into(),
            required: true,
        };
        let app = with_client_ip_test_layers(
            with_attachment_layers_store(crate::api::router(db.clone(), &[]), store.clone()),
            test_peer(),
        )
        .layer(axum::Extension(crate::realtime::RealtimeHub::new()))
        .layer(axum::Extension(crate::config::AuthConfig {
            allow_signup: false,
            required: true,
            secure_cookies: false,
        }))
        .layer(axum::middleware::from_fn_with_state(
            auth_state,
            crate::auth::require_api_key,
        ));
        Self {
            db,
            app,
            store,
            _store_guard: guard,
        }
    }

    fn user(&self, username: &str, is_admin: bool) -> User {
        let conn = self.db.write().unwrap();
        crate::db::queries::users::create_user(
            &conn,
            &CreateUser {
                username: username.into(),
                email: format!("{username}@archive.test"),
                password: "testpassword1".into(),
                display_name: None,
                is_admin,
                is_bot: false,
            },
        )
        .unwrap()
    }

    fn session(&self, user_id: i64) -> String {
        let conn = self.db.write().unwrap();
        crate::db::queries::users::create_session(&conn, user_id, None)
            .unwrap()
            .token
    }

    fn enforce_authz(&self, on: bool) {
        let conn = self.db.write().unwrap();
        crate::db::queries::settings::update(
            &conn,
            crate::db::queries::settings::InstanceSettingsPatch {
                authz_enforced: Some(on),
                ..Default::default()
            },
        )
        .unwrap();
    }

    fn project(&self, members: &[(i64, &str)]) -> i64 {
        let conn = self.db.write().unwrap();
        conn.execute_batch(
            "INSERT INTO projects(id,name,identifier,is_public) VALUES(7,'Shelf','SHF',1);
             INSERT INTO issues(id,project_id,sequence,title) VALUES(70,7,1,'Kept');",
        )
        .unwrap();
        for (user, role) in members {
            conn.execute(
                "INSERT INTO project_members(project_id,user_id,role) VALUES (7,?1,?2)",
                rusqlite::params![user, role],
            )
            .unwrap();
        }
        7
    }

    fn count(&self, sql: &str) -> i64 {
        self.db
            .read()
            .unwrap()
            .query_row(sql, [], |r| r.get(0))
            .unwrap()
    }

    async fn send(&self, method: &str, uri: &str, token: &str) -> (StatusCode, serde_json::Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }
}

#[tokio::test]
async fn a_lead_archives_and_only_an_admin_lists_and_unarchives() {
    let instance = Instance::new();
    instance.enforce_authz(true);
    let admin = instance.user("admin", true);
    let lead = instance.user("lead", false);
    let project = instance.project(&[(lead.id, "lead")]);
    let lead_token = instance.session(lead.id);
    let admin_token = instance.session(admin.id);

    let (status, body) = instance
        .send(
            "POST",
            &format!("/api/projects/{project}/archive"),
            &lead_token,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["identifier"], "SHF");
    assert_eq!(body["issue_count"], 1);
    assert_eq!(body["was_public"], true);
    assert_eq!(body["file_present"], true);
    let archive_id = body["id"].as_i64().unwrap();
    assert_eq!(instance.count("SELECT count(*) FROM projects"), 0);
    assert!(
        instance
            .store
            .archived_dir()
            .join(body["file_name"].as_str().unwrap())
            .is_file()
    );

    let (status, _) = instance
        .send("GET", "/api/archived-projects", &lead_token)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = instance
        .send(
            "POST",
            &format!("/api/archived-projects/{archive_id}/unarchive"),
            &lead_token,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(instance.count("SELECT count(*) FROM archived_projects"), 1);

    let (status, body) = instance
        .send("GET", "/api/archived-projects", &admin_token)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["archived_by_name"], "lead");

    let (status, body) = instance
        .send(
            "POST",
            &format!("/api/archived-projects/{archive_id}/unarchive"),
            &admin_token,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["identifier"], "SHF");
    assert_eq!(body["was_public"], true);
    let restored = body["project_id"].as_i64().unwrap();

    // The lead gets their project back, private.
    let (status, body) = instance
        .send("GET", &format!("/api/projects/{restored}"), &lead_token)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["is_public"], false);
    let (_, body) = instance
        .send("GET", "/api/archived-projects", &admin_token)
        .await;
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn a_viewer_cannot_archive() {
    let instance = Instance::new();
    instance.enforce_authz(true);
    let viewer = instance.user("viewer", false);
    let project = instance.project(&[(viewer.id, "viewer")]);
    let token = instance.session(viewer.id);

    let (status, _) = instance
        .send("POST", &format!("/api/projects/{project}/archive"), &token)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(instance.count("SELECT count(*) FROM projects"), 1);
    assert_eq!(instance.count("SELECT count(*) FROM archived_projects"), 0);
}

#[tokio::test]
async fn without_enforcement_only_an_admin_archives() {
    let instance = Instance::new();
    instance.enforce_authz(false);
    let admin = instance.user("admin", true);
    let user = instance.user("someone", false);
    let project = instance.project(&[]);

    let (status, _) = instance
        .send(
            "POST",
            &format!("/api/projects/{project}/archive"),
            &instance.session(user.id),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(instance.count("SELECT count(*) FROM projects"), 1);

    let (status, _) = instance
        .send(
            "POST",
            &format!("/api/projects/{project}/archive"),
            &instance.session(admin.id),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(instance.count("SELECT count(*) FROM projects"), 0);
}

#[tokio::test]
async fn unarchiving_into_a_taken_identifier_is_a_conflict() {
    let instance = Instance::new();
    let admin = instance.user("admin", true);
    let project = instance.project(&[]);
    let token = instance.session(admin.id);
    let (_, body) = instance
        .send("POST", &format!("/api/projects/{project}/archive"), &token)
        .await;
    let archive_id = body["id"].as_i64().unwrap();
    {
        let conn = instance.db.write().unwrap();
        conn.execute(
            "INSERT INTO projects(name,identifier) VALUES ('New','SHF')",
            [],
        )
        .unwrap();
    }
    let (status, body) = instance
        .send(
            "POST",
            &format!("/api/archived-projects/{archive_id}/unarchive"),
            &token,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(instance.count("SELECT count(*) FROM archived_projects"), 1);
}
