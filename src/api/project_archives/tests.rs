//! LIF-467 backend tests.
//!
//! Everything credential-shaped runs against the real production stack: the
//! actual `api::router`, wrapped in the actual `require_api_key` middleware,
//! reached over real requests. Layering a `ResolvedIdentity` extension by hand
//! would prove nothing about a surface whose entire job is refusing every
//! credential except one.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use rusqlite::params;
use tower::ServiceExt;

use crate::api::test_helpers::{
    test_attachment_store, test_peer, with_attachment_layers_store, with_client_ip_test_layers,
};
use crate::db::DbPool;
use crate::db::models::{CreateUser, Role, User};
use crate::error::LificError;
use crate::project_archive::Limits;
use crate::realtime::{RealtimeEvent, RealtimeHub};
use crate::storage::AttachmentStore;

const BOUNDARY: &str = "lific467boundary";

// Fixture
struct Instance {
    db: DbPool,
    app: Router,
    store: AttachmentStore,
    realtime: RealtimeHub,
    manager: api_keys_simplified::ApiKeyManagerV0,
    _store_guard: tempfile::TempDir,
}

impl Instance {
    fn new() -> Self {
        Self::with_auth(true)
    }

    fn with_auth(required: bool) -> Self {
        let db = crate::db::open_memory().expect("test db");
        let (store, guard) = test_attachment_store();
        let realtime = RealtimeHub::new();
        let manager = crate::auth::create_key_manager().unwrap();
        let auth_state = crate::auth::AuthState {
            db: db.clone(),
            manager: manager.clone(),
            public_url: "https://archive.test".into(),
            required,
        };
        let app = with_client_ip_test_layers(
            with_attachment_layers_store(crate::api::router(db.clone(), &[]), store.clone()),
            test_peer(),
        )
        .layer(axum::Extension(realtime.clone()))
        .layer(axum::Extension(crate::config::AuthConfig {
            allow_signup: false,
            required,
            secure_cookies: false,
        }))
        .layer(axum::Extension(Arc::new(manager.clone())))
        .layer(axum::middleware::from_fn_with_state(
            auth_state,
            crate::auth::require_api_key,
        ));
        Self {
            db,
            app,
            store,
            realtime,
            manager,
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

    fn api_key(&self, name: &str, owner: Option<i64>) -> String {
        crate::auth::create_api_key(&self.db, &self.manager, name, owner).unwrap()
    }

    fn oauth_token(&self, suffix: &str, user_id: i64) -> String {
        let token = format!("lific_at_{suffix}");
        let hash = crate::auth::sha256_hex(token.as_bytes());
        let expires = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        let conn = self.db.write().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO oauth_clients (client_id, client_name, redirect_uris)
             VALUES ('archive-client', 'Archive', '[\"http://localhost\"]')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO oauth_tokens (access_token, client_id, expires_at, scope, user_id)
             VALUES (?1, 'archive-client', ?2, 'mcp', ?3)",
            params![hash, expires, user_id],
        )
        .unwrap();
        token
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

    fn project_count(&self) -> i64 {
        self.try_project_count().unwrap()
    }

    /// Fallible, for polling while a writer holds an immediate transaction:
    /// a shared-cache in-memory reader answers "schema is locked" then.
    fn try_project_count(&self) -> Option<i64> {
        let conn = self.db.read().ok()?;
        conn.query_row("SELECT count(*) FROM projects", [], |r| r.get(0))
            .ok()
    }

    async fn get(&self, uri: &str, token: Option<&str>) -> axum::response::Response {
        let mut builder = Request::builder().uri(uri);
        if let Some(token) = token {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        self.app
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    /// Download an archive, drain it, and wait for its slot to come back.
    /// A streamed download holds the instance's single archive slot until
    /// its body is finished, so a test that fires several in a row has to
    /// let each one land or the next is legitimately answered 429.
    async fn export_status(&self, identifier: &str, user_id: i64) -> StatusCode {
        let token = self.session(user_id);
        let response = self
            .get(&format!("/api/project-archives/{identifier}"), Some(&token))
            .await;
        let status = response.status();
        let _ = response.into_body().collect().await;
        self.settle().await;
        status
    }

    async fn settle(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if self.db.acquire_archive_slot().is_ok() {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("a finished archive request releases its slot");
    }

    async fn upload(&self, token: Option<&str>, body: Vec<u8>) -> axum::response::Response {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/api/project-archives")
            .header(
                "content-type",
                format!("multipart/form-data; boundary={BOUNDARY}"),
            );
        if let Some(token) = token {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        self.app
            .clone()
            .oneshot(builder.body(Body::from(body)).unwrap())
            .await
            .unwrap()
    }
}

/// A single `archive` part, the shape the contract requires.
fn archive_body(bytes: &[u8]) -> Vec<u8> {
    multipart(&[("archive", "project.tar.gz", bytes)])
}

fn multipart(parts: &[(&str, &str, &[u8])]) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, filename, bytes) in parts {
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n\
                 Content-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    body
}

async fn json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

/// A project with the whole shape an archive has to carry: an attachment
/// referenced from a description, a soft-deleted issue, a page and a comment.
/// Published, so the "a public source imports as private" rule has something
/// to be true about.
fn seed_project(instance: &Instance, lead: i64) -> i64 {
    let sha = instance.store.write(b"archived attachment bytes").unwrap();
    let conn = instance.db.write().unwrap();
    conn.execute_batch(
        "INSERT INTO projects(id,name,identifier,is_public) VALUES(7,'Portable','POR',1);
         INSERT INTO issues(id,project_id,sequence,title) VALUES(70,7,1,'Live issue'),(71,7,2,'Gone issue');
         INSERT INTO pages(id,project_id,sequence,title,content) VALUES(80,7,1,'Guide','body');
         INSERT INTO comments(id,issue_id,content,imported_author) VALUES(90,70,'a comment','Someone (imported)');
         UPDATE issues SET deleted_at='2025-02-03 00:00:00' WHERE id=71;",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO attachments(id,sha256,filename,mime,size_bytes) VALUES(95,?1,'note.txt','text/plain',25)",
        [&sha],
    )
    .unwrap();
    conn.execute_batch(
        "INSERT INTO attachment_links VALUES(95,'issue',70,'2025-01-01 00:00:00');
         UPDATE issues SET description='[note](/api/attachments/95)' WHERE id=70;",
    )
    .unwrap();
    conn.execute("UPDATE projects SET lead_user_id = ?1 WHERE id = 7", [lead])
        .unwrap();
    7
}

// Credential policy
#[tokio::test]
async fn only_a_browser_session_reaches_the_archive_surface() {
    let instance = Instance::new();
    let admin = instance.user("admin", true);
    let bot = {
        let conn = instance.db.write().unwrap();
        crate::db::queries::users::create_bot_user(&conn, admin.id, "tool-admin", "Tool", None)
            .unwrap()
    };

    let admin_key = instance.api_key("admin-key", Some(admin.id));
    let operator_key = instance.api_key("operator", None);
    let admin_oauth = instance.oauth_token("admin", admin.id);
    let bot_session = instance.session(bot.id);

    // No credential at all on an instance that requires one.
    assert_eq!(
        instance.get("/api/project-archives", None).await.status(),
        StatusCode::UNAUTHORIZED
    );

    for (label, token) in [
        ("an API key bound to the admin", admin_key.as_str()),
        ("the unbound operator key", operator_key.as_str()),
        ("an OAuth connector token", admin_oauth.as_str()),
        ("a bot's own session", bot_session.as_str()),
    ] {
        let response = instance.get("/api/project-archives", Some(token)).await;
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "{label} must not read the archive surface"
        );
        let response = instance.upload(Some(token), archive_body(b"junk")).await;
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "{label} must not import"
        );
    }

    let session = instance.session(admin.id);
    assert_eq!(
        instance
            .get("/api/project-archives", Some(&session))
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(instance.project_count(), 0);
}

/// With `[auth] required = false` the middleware hands every credential-less
/// request the first admin. That fallback must not become an archive
/// credential: there is no human behind it.
#[tokio::test]
async fn the_authentication_disabled_fallback_admin_is_not_an_archive_credential() {
    let instance = Instance::with_auth(false);
    instance.user("admin", true);

    let response = instance.get("/api/project-archives", None).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = instance.upload(None, archive_body(b"junk")).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(instance.project_count(), 0);
}

#[tokio::test]
async fn capabilities_publish_the_web_profile_and_the_callers_own_import_right() {
    let instance = Instance::new();
    let admin = instance.user("admin", true);
    let regular = instance.user("regular", false);

    let body = json(
        instance
            .get("/api/project-archives", Some(&instance.session(admin.id)))
            .await,
    )
    .await;
    assert_eq!(body["can_import"], true);
    assert_eq!(body["max_upload_bytes"], 134_217_728u64);
    assert_eq!(body["max_expanded_bytes"], 268_435_456u64);
    assert_eq!(body["max_metadata_bytes"], 16_777_216u64);
    assert_eq!(body["max_blob_bytes"], 67_108_864u64);
    assert_eq!(body["max_blob_total_bytes"], 201_326_592u64);
    assert_eq!(body["max_rows"], 50_000);
    assert_eq!(body["max_blobs"], 2_000);

    let body = json(
        instance
            .get("/api/project-archives", Some(&instance.session(regular.id)))
            .await,
    )
    .await;
    assert_eq!(body["can_import"], false);
}

#[tokio::test]
async fn a_regular_user_cannot_import_with_authorization_enforcement_either_way() {
    for enforced in [false, true] {
        let instance = Instance::new();
        instance.enforce_authz(enforced);
        let regular = instance.user("regular", false);
        let session = instance.session(regular.id);

        let body = json(instance.get("/api/project-archives", Some(&session)).await).await;
        assert_eq!(body["can_import"], false, "enforced={enforced}");

        let response = instance.upload(Some(&session), archive_body(b"junk")).await;
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "enforced={enforced}"
        );
        assert_eq!(instance.project_count(), 0);
    }
}

/// An admin demoted after the request was authenticated, but before the
/// writer transaction runs, must not import. The in-transaction hook is what
/// decides, so that is what this asserts.
#[tokio::test]
async fn demotion_between_the_gate_and_the_write_blocks_the_import() {
    let instance = Instance::new();
    let admin = instance.user("admin", true);
    let session = instance.session(admin.id);
    {
        let conn = instance.db.read().unwrap();
        super::authorize_import(&conn, &session, admin.id).expect("an admin passes");
    }
    {
        let conn = instance.db.write().unwrap();
        conn.execute("UPDATE users SET is_admin = 0 WHERE id = ?1", [admin.id])
            .unwrap();
    }
    let conn = instance.db.read().unwrap();
    assert!(matches!(
        super::authorize_import(&conn, &session, admin.id),
        Err(LificError::Forbidden(_))
    ));
    drop(conn);

    let response = instance.upload(Some(&session), archive_body(b"junk")).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// Signing out revokes the archive surface immediately, including for a
/// request already in flight: the snapshot hook re-reads the session row.
#[tokio::test]
async fn logging_out_between_the_gate_and_the_snapshot_blocks_the_export() {
    let instance = Instance::new();
    let admin = instance.user("admin", true);
    let session = instance.session(admin.id);
    let project_id = seed_project(&instance, admin.id);
    {
        let conn = instance.db.read().unwrap();
        super::authorize_export(&conn, &session, admin.id, project_id).expect("an admin passes");
    }
    {
        let conn = instance.db.write().unwrap();
        conn.execute("DELETE FROM sessions", []).unwrap();
    }
    let conn = instance.db.read().unwrap();
    assert!(matches!(
        super::authorize_export(&conn, &session, admin.id, project_id),
        Err(LificError::Forbidden(_))
    ));
    drop(conn);

    // Over the wire the middleware gets there first and answers 401. The
    // point of the assertion above is that the hook refuses independently,
    // so a session revoked after the middleware ran still stops the read.
    assert_eq!(
        instance
            .get("/api/project-archives/POR", Some(&session))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

/// A session that names somebody other than the identity the middleware
/// resolved is refused, which is what closes the swap between the two reads.
#[tokio::test]
async fn a_session_belonging_to_another_account_is_refused() {
    let instance = Instance::new();
    let admin = instance.user("admin", true);
    let stranger = instance.user("stranger", false);
    let token = instance.session(admin.id);
    let conn = instance.db.read().unwrap();
    assert!(matches!(
        super::authorize_import(&conn, &token, stranger.id),
        Err(LificError::Forbidden(_))
    ));
}

// Export authorization
#[tokio::test]
async fn export_admits_only_the_lead_and_instance_admins() {
    let instance = Instance::new();
    instance.enforce_authz(true);
    let admin = instance.user("admin", true);
    let lead = instance.user("lead", false);
    let maintainer = instance.user("maintainer", false);
    let viewer = instance.user("viewer", false);
    let outsider = instance.user("outsider", false);
    let project_id = seed_project(&instance, lead.id);
    {
        let conn = instance.db.write().unwrap();
        crate::db::queries::members::upsert_member(&conn, project_id, lead.id, Role::Lead).unwrap();
        crate::db::queries::members::upsert_member(
            &conn,
            project_id,
            maintainer.id,
            Role::Maintainer,
        )
        .unwrap();
        crate::db::queries::members::upsert_member(&conn, project_id, viewer.id, Role::Viewer)
            .unwrap();
    }

    for denied in [&maintainer, &viewer, &outsider] {
        assert_eq!(
            instance.export_status("POR", denied.id).await,
            StatusCode::FORBIDDEN,
            "{} must not export the project's history",
            denied.username
        );
    }

    for allowed in [&lead, &admin] {
        assert_eq!(
            instance.export_status("POR", allowed.id).await,
            StatusCode::OK,
            "{} may export",
            allowed.username
        );
    }
}

/// With enforcement off the classic `lead_user_id` pointer is still the gate,
/// exactly as `require_project_lead` has always read it.
#[tokio::test]
async fn the_legacy_lead_gate_still_decides_when_enforcement_is_off() {
    let instance = Instance::new();
    let admin = instance.user("admin", true);
    let lead = instance.user("lead", false);
    let outsider = instance.user("outsider", false);
    seed_project(&instance, lead.id);

    assert_eq!(
        instance.export_status("POR", outsider.id).await,
        StatusCode::FORBIDDEN
    );
    for allowed in [&lead, &admin] {
        assert_eq!(
            instance.export_status("POR", allowed.id).await,
            StatusCode::OK,
            "{}",
            allowed.username
        );
    }
}

// Round trip
#[tokio::test]
async fn a_downloaded_archive_imports_into_a_second_instance_with_its_graph_intact() {
    let source = Instance::new();
    let lead = source.user("lead", false);
    seed_project(&source, lead.id);

    let response = source
        .get("/api/project-archives/POR", Some(&source.session(lead.id)))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[axum::http::header::CONTENT_TYPE],
        "application/gzip"
    );
    assert_eq!(
        response.headers()[axum::http::header::CONTENT_DISPOSITION],
        "attachment; filename=\"POR.lific.tar.gz\""
    );
    assert_eq!(
        response.headers()[axum::http::header::CACHE_CONTROL],
        "no-store"
    );
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert_eq!(
        response.headers()["content-security-policy"],
        "default-src 'none'; sandbox"
    );
    let archive = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&archive[..2], b"\x1f\x8b", "a gzip member");

    let destination = Instance::new();
    let admin = destination.user("admin", true);
    let mut events = destination.realtime.subscribe();

    let response = destination
        .upload(Some(&destination.session(admin.id)), archive_body(&archive))
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json(response).await;

    let project_id = body["project"]["id"].as_i64().unwrap();
    assert_eq!(body["project"]["identifier"], "POR");
    assert_eq!(body["project"]["is_public"], false);
    assert_eq!(body["report"]["project"], "POR");
    assert_eq!(body["report"]["rows"]["issues"], 2);
    assert_eq!(body["report"]["rows"]["pages"], 1);
    assert_eq!(body["report"]["rows"]["comments"], 1);
    assert_eq!(body["report"]["blobs"], 1);
    assert!(body["report"]["external_references"].is_array());
    assert!(body["report"]["external_reference_count"].is_number());

    assert!(matches!(
        events.try_recv().unwrap().event,
        RealtimeEvent::ProjectUpdated { project_id: id } if id == project_id
    ));

    let conn = destination.db.read().unwrap();
    let (identifier, is_public, lead_user): (String, bool, Option<i64>) = conn
        .query_row(
            "SELECT identifier, is_public, lead_user_id FROM projects WHERE id = ?1",
            [project_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(identifier, "POR");
    assert!(!is_public, "a published source imports as private");
    assert_eq!(lead_user, Some(admin.id));

    // Tombstones travel; the attachment link and its rewritten reference do
    // too, and the blob really landed in the destination store.
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM issues WHERE project_id=?1 AND deleted_at IS NOT NULL",
            [project_id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    let (attachment, sha): (i64, String) = conn
        .query_row("SELECT id, sha256 FROM attachments", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    let description: String = conn
        .query_row(
            "SELECT description FROM issues WHERE project_id=?1 AND sequence=1",
            [project_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        description,
        format!("[note](/api/attachments/{attachment})")
    );
    assert_eq!(
        destination.store.read(&sha).unwrap(),
        b"archived attachment bytes"
    );

    // The lead grant is audited as this admin acting through the browser, not
    // as the CLI's actorless local grant.
    let (transport, actor): (String, Option<i64>) = conn
        .query_row(
            "SELECT transport, actor_user_id FROM audit_log
             WHERE transport <> 'imported' ORDER BY id DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(transport, "web");
    assert_eq!(actor, Some(admin.id));
}

#[tokio::test]
async fn importing_the_same_archive_twice_changes_nothing_the_second_time() {
    let source = Instance::new();
    let lead = source.user("lead", false);
    seed_project(&source, lead.id);
    let archive = source
        .get("/api/project-archives/POR", Some(&source.session(lead.id)))
        .await
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();

    let destination = Instance::new();
    let admin = destination.user("admin", true);
    let session = destination.session(admin.id);
    assert_eq!(
        destination
            .upload(Some(&session), archive_body(&archive))
            .await
            .status(),
        StatusCode::CREATED
    );
    let before = destination.project_count();

    let response = destination
        .upload(Some(&session), archive_body(&archive))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(
        json(response).await["error"]
            .as_str()
            .unwrap()
            .contains("already exists")
    );
    assert_eq!(destination.project_count(), before);
}

// Malformed and hostile uploads
#[tokio::test]
async fn a_corrupt_archive_is_refused_and_writes_nothing() {
    let instance = Instance::new();
    let admin = instance.user("admin", true);
    let session = instance.session(admin.id);

    for body in [
        archive_body(b""),
        archive_body(b"not a gzip stream at all"),
        archive_body(&[
            0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0xff,
        ]),
    ] {
        let response = instance.upload(Some(&session), body).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    assert_eq!(instance.project_count(), 0);
}

#[tokio::test]
async fn the_upload_takes_exactly_one_field_named_archive() {
    let instance = Instance::new();
    let admin = instance.user("admin", true);
    let session = instance.session(admin.id);

    let rejected = [
        // A field naming a destination owner: the caller does not get to
        // choose who the project is handed to.
        multipart(&[("owner", "x", b"admin"), ("archive", "a.tar.gz", b"junk")]),
        // A field naming a path: nothing about the destination is caller
        // controlled either.
        multipart(&[
            ("path", "x", b"/etc/lific"),
            ("archive", "a.tar.gz", b"junk"),
        ]),
        // Two archives.
        multipart(&[
            ("archive", "a.tar.gz", b"junk"),
            ("archive", "b.tar.gz", b"junk"),
        ]),
        // Nothing at all.
        multipart(&[]),
        // The right bytes under the wrong name.
        multipart(&[("file", "a.tar.gz", b"junk")]),
    ];
    for body in rejected {
        let response = instance.upload(Some(&session), body).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    assert_eq!(instance.project_count(), 0);
}

#[tokio::test]
async fn an_upload_past_the_ceiling_is_refused_while_it_streams() {
    let instance = Instance::new();
    let admin = instance.user("admin", true);
    let session = instance.session(admin.id);
    let body = archive_body(&vec![0u8; 4096]);

    let response = super::TEST_WEB_LIMITS
        .scope(
            Limits {
                max_compressed: 1024,
                ..Limits::WEB
            },
            instance.upload(Some(&session), body),
        )
        .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(instance.project_count(), 0);
}

// Slot lifetime
#[tokio::test]
async fn the_archive_slot_is_instance_wide_and_separate_from_export_slots() {
    let instance = Instance::new();
    let held = instance.db.acquire_archive_slot().unwrap();
    assert!(matches!(
        instance.db.acquire_archive_slot(),
        Err(LificError::TooManyRequests(_))
    ));
    // Ordinary exports are untouched by an archive in flight.
    let _first = instance.db.acquire_export_slot().unwrap();
    let _second = instance.db.acquire_export_slot().unwrap();
    drop(held);
    let _again = instance.db.acquire_archive_slot().unwrap();
}

#[tokio::test]
async fn a_busy_instance_refuses_a_second_archive_request_before_reading_its_body() {
    let instance = Instance::new();
    let admin = instance.user("admin", true);
    let lead = instance.user("lead", false);
    seed_project(&instance, lead.id);
    let held = instance.db.acquire_archive_slot().unwrap();

    let response = instance
        .upload(
            Some(&instance.session(admin.id)),
            archive_body(&vec![0u8; 4096]),
        )
        .await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let response = instance
        .get(
            "/api/project-archives/POR",
            Some(&instance.session(lead.id)),
        )
        .await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    drop(held);
}

/// A client that gives up mid-import must not hand its slot to the next
/// request while the worker is still holding the writer.
#[tokio::test]
async fn a_cancelled_request_keeps_its_archive_slot_until_the_worker_stops() {
    let instance = Instance::new();
    let slot = instance.db.acquire_archive_slot().unwrap();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let request = tokio::spawn(crate::api::export::blocking_export(slot, move || {
        let _ = started_tx.send(());
        release_rx.recv().unwrap();
        Ok(())
    }));

    started_rx.await.unwrap();
    request.abort();
    assert!(
        instance.db.acquire_archive_slot().is_err(),
        "an aborted request must not release the slot early"
    );

    release_tx.send(()).unwrap();
    let released = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if instance.db.acquire_archive_slot().is_ok() {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(
        released.is_ok(),
        "the worker releases the slot when it stops"
    );
}

// Limits reach the HTTP surface
/// The published numbers and the profile the handlers actually run under are
/// the same values, so the contract cannot drift from the enforcement.
#[test]
fn the_published_limits_are_the_profile_the_handlers_use() {
    assert_eq!(Limits::WEB.max_compressed, 134_217_728);
    assert_eq!(Limits::WEB.max_expanded, 268_435_456);
    assert_eq!(Limits::WEB.max_metadata, 16_777_216);
    assert_eq!(Limits::WEB.max_blob, 67_108_864);
    assert_eq!(Limits::WEB.max_rows, 50_000);
    assert_eq!(Limits::WEB.max_blobs, 2_000);
    // The web profile must never be able to build an archive it would then
    // refuse to read back.
    const {
        assert!(Limits::WEB.max_blob_total + Limits::WEB.max_metadata < Limits::WEB.max_expanded);
    }
}

// Resource ceilings over the wire
/// Every budget the web profile publishes answers 413 at the router, and a
/// broken archive still answers 400. Proven with kilobyte profiles, so none of
/// this needs a fixture anywhere near the real limits.
#[tokio::test]
async fn every_web_budget_answers_413_from_the_router() {
    let source = Instance::new();
    let lead = source.user("lead", false);
    seed_project(&source, lead.id);
    let archive = source
        .get("/api/project-archives/POR", Some(&source.session(lead.id)))
        .await
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();

    let budgets = [
        (
            "manifest",
            Limits {
                max_metadata: 16,
                ..Limits::WEB
            },
        ),
        (
            "decompressed archive",
            Limits {
                max_expanded: 64,
                ..Limits::WEB
            },
        ),
        (
            "combined blob bytes",
            Limits {
                max_blob_total: 4,
                ..Limits::WEB
            },
        ),
        (
            "one blob",
            Limits {
                max_blob: 4,
                ..Limits::WEB
            },
        ),
        (
            "rows",
            Limits {
                max_rows: 2,
                ..Limits::WEB
            },
        ),
        (
            "blob count",
            Limits {
                max_blobs: 0,
                ..Limits::WEB
            },
        ),
        (
            "compressed archive",
            Limits {
                max_compressed: 8,
                ..Limits::WEB
            },
        ),
    ];

    for (label, limits) in budgets {
        let destination = Instance::new();
        let admin = destination.user("admin", true);
        let session = destination.session(admin.id);
        let response = super::TEST_WEB_LIMITS
            .scope(
                limits,
                destination.upload(Some(&session), archive_body(&archive)),
            )
            .await;
        assert_eq!(
            response.status(),
            StatusCode::PAYLOAD_TOO_LARGE,
            "{label} must be a resource answer"
        );
        assert_eq!(destination.project_count(), 0, "{label}");
    }
}

/// A hostile archive must not be able to choose its own status code by
/// writing budget-shaped text into its data.
#[tokio::test]
async fn a_broken_archive_stays_a_400_whatever_it_claims_about_limits() {
    let instance = Instance::new();
    let admin = instance.user("admin", true);
    let session = instance.session(admin.id);

    for body in [
        archive_body(b"too many rows"),
        archive_body(b"Payload too large: project archive: too many list entries"),
        archive_body(&[
            0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0xff,
        ]),
    ] {
        assert_eq!(
            instance.upload(Some(&session), body).await.status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(instance.project_count(), 0);
}

/// A capabilities response and the profile the upload actually runs under are
/// read from the same place, so a client cannot be told one number and judged
/// by another.
#[tokio::test]
async fn capabilities_and_enforcement_read_the_same_profile() {
    let instance = Instance::new();
    let admin = instance.user("admin", true);
    let session = instance.session(admin.id);
    let limits = Limits {
        max_compressed: 1024,
        ..Limits::WEB
    };

    let body = super::TEST_WEB_LIMITS
        .scope(limits, async {
            json(instance.get("/api/project-archives", Some(&session)).await).await
        })
        .await;
    assert_eq!(body["max_upload_bytes"], 1024);

    let response = super::TEST_WEB_LIMITS
        .scope(
            limits,
            instance.upload(Some(&session), archive_body(&vec![0u8; 4096])),
        )
        .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

// Identifier reassignment
/// The download names the project the snapshot exported, not the label the
/// request happened to be written with, because the two can stop agreeing.
#[tokio::test]
async fn the_download_is_named_by_the_project_the_snapshot_exported() {
    let instance = Instance::new();
    let admin = instance.user("admin", true);
    let lead = instance.user("lead", false);
    seed_project(&instance, lead.id);

    let response = instance
        .get(
            "/api/project-archives/POR",
            Some(&instance.session(admin.id)),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[axum::http::header::CONTENT_DISPOSITION],
        "attachment; filename=\"POR.lific.tar.gz\""
    );
    let _ = response.into_body().collect().await;
    instance.settle().await;
}

/// Handing an authorized project's identifier to a project the caller cannot
/// see must never leak that project's bytes.
#[tokio::test]
async fn reassigning_an_identifier_does_not_expose_another_project() {
    let instance = Instance::new();
    instance.enforce_authz(true);
    let lead = instance.user("lead", false);
    let project_id = seed_project(&instance, lead.id);
    {
        let conn = instance.db.write().unwrap();
        crate::db::queries::members::upsert_member(&conn, project_id, lead.id, Role::Lead).unwrap();
        conn.execute_batch(
            "INSERT INTO projects(id,name,identifier) VALUES(8,'Secret','SEC');
             INSERT INTO issues(id,project_id,sequence,title) VALUES(99,8,1,'PRIVATE ISSUE');",
        )
        .unwrap();
        // The label the lead knows now belongs to a project they cannot see.
        conn.execute_batch(
            "UPDATE projects SET identifier='OLD' WHERE id=7;
             UPDATE projects SET identifier='POR' WHERE id=8;",
        )
        .unwrap();
    }

    // Asking for the label resolves the other project, and the gate refuses.
    assert_eq!(
        instance.export_status("POR", lead.id).await,
        StatusCode::FORBIDDEN
    );

    // Asking for their own project still works, and carries only their bytes.
    let response = instance
        .get(
            "/api/project-archives/OLD",
            Some(&instance.session(lead.id)),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[axum::http::header::CONTENT_DISPOSITION],
        "attachment; filename=\"OLD.lific.tar.gz\""
    );
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    instance.settle().await;
    assert!(!String::from_utf8_lossy(&bytes).contains("PRIVATE ISSUE"));
}

// Cancellation after the worker starts

/// A client that disconnects after the worker is running has already handed
/// the instance a committed project. Abandoning the import there would leave
/// the caller with a project they were told did not exist.
#[tokio::test]
async fn a_cancelled_request_still_commits_and_announces_the_import_once() {
    let source = Instance::new();
    let lead = source.user("lead", false);
    seed_project(&source, lead.id);
    let archive = source
        .get("/api/project-archives/POR", Some(&source.session(lead.id)))
        .await
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();

    let destination = Instance::new();
    let admin = destination.user("admin", true);
    let session = destination.session(admin.id);
    let mut events = destination.realtime.subscribe();

    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let gate = Arc::new(crate::api::export::ExportTestGate::new(
        started_tx, release_rx,
    ));
    let app = destination.app.clone();
    let request = Request::builder()
        .method("POST")
        .uri("/api/project-archives")
        .header("authorization", format!("Bearer {session}"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(archive_body(&archive)))
        .unwrap();
    let task = tokio::spawn(
        crate::api::export::EXPORT_TEST_GATE.scope(gate, async move { app.oneshot(request).await }),
    );

    // The worker is on its blocking thread and has not imported anything yet.
    started_rx.await.unwrap();
    task.abort();
    assert!(
        destination.db.acquire_archive_slot().is_err(),
        "the slot is held across the commit, not released with the request"
    );
    assert_eq!(destination.project_count(), 0);

    release_tx.send(()).unwrap();
    let committed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if destination.try_project_count() == Some(1) {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(committed.is_ok(), "the abandoned import still commits");

    destination.settle().await;
    let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("the commit is announced even though nobody is listening on the request")
        .unwrap();
    assert!(matches!(event.event, RealtimeEvent::ProjectUpdated { .. }));
    assert!(
        events.try_recv().is_err(),
        "exactly one announcement per import"
    );
}

#[tokio::test]
async fn a_failed_import_announces_nothing() {
    let instance = Instance::new();
    let admin = instance.user("admin", true);
    let session = instance.session(admin.id);
    let mut events = instance.realtime.subscribe();

    for body in [
        archive_body(b"not a gzip stream"),
        multipart(&[("owner", "x", b"admin")]),
    ] {
        assert!(
            instance
                .upload(Some(&session), body)
                .await
                .status()
                .is_client_error()
        );
    }
    assert_eq!(instance.project_count(), 0);
    assert!(
        events.try_recv().is_err(),
        "a refused import publishes nothing"
    );
}
