use super::*;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

struct Client {
    app: Router,
    path: String,
    token: Option<String>,
}

impl Client {
    async fn call(&self, name: &str, arguments: Value) -> String {
        let mut request = Request::builder()
            .method(Method::POST)
            .uri(&self.path)
            .header("host", "localhost")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2025-06-18");
        if let Some(token) = &self.token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let response = self
            .app
            .clone()
            .oneshot(
                request
                    .body(Body::from(
                        json!({
                            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                            "params": {"name": name, "arguments": arguments}
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(value.get("error").is_none(), "{value}");
        assert_ne!(value["result"]["isError"], true, "{value}");
        value["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_owned()
    }
}

async fn call_pair(clients: &[Client; 2], name: &str, arguments: [Value; 2]) -> [String; 2] {
    let [alice_args, bob_args] = arguments;
    let (alice, bob) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(
            clients[0].call(name, alice_args),
            clients[1].call(name, bob_args)
        )
    })
    .await
    .expect("both HTTP MCP calls must reach tool dispatch concurrently and finish");
    [alice, bob]
}

// Four workers leave room for the HTTP tasks as well as two admitted tools.
// The barrier is inside LificMcp::call_tool, after admission: serializing the
// handlers (or admitting just one tool) makes this fail at the bounded timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_mcp_tools_overlap_without_crossing_identity_or_audit_actor() {
    let pool = db::open_memory().unwrap();
    let users = {
        let conn = pool.write().unwrap();
        db::queries::settings::ensure(&conn, false).unwrap();
        db::queries::users::create_passwordless_admin(&conn, "Operator").unwrap();
        ["alice", "bob"].map(|name| {
            let user = db::queries::users::create_user(
                &conn,
                &db::models::CreateUser {
                    username: name.into(),
                    email: format!("{name}@example.test"),
                    password: "testpassword1".into(),
                    display_name: None,
                    is_admin: false,
                    is_bot: false,
                },
            )
            .unwrap();
            db::queries::create_project(
                &conn,
                &db::models::CreateProject {
                    name: format!("{name} private project"),
                    identifier: name.to_ascii_uppercase(),
                    lead_user_id: Some(user.id),
                    ..Default::default()
                },
            )
            .unwrap();
            db::models::AuthUser {
                id: user.id,
                username: user.username,
                display_name: user.display_name,
                is_admin: false,
            }
        })
    };
    let tokens = users
        .each_ref()
        .map(|user| auth::create_api_key(&pool, &user.username, Some(user.id)).unwrap());
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let mut cfg = Config::default();
    cfg.auth.required = true;
    let scratch = tempfile::tempdir().unwrap();
    let app = build_app_with_store(
        &cfg,
        pool.clone(),
        realtime::RealtimeHub::new(),
        Arc::from([]),
        storage::AttachmentStore::new(scratch.path().join("attachments")),
    )
    .layer(axum::Extension(barrier.clone()));
    let authenticated = tokens.map(|token| Client {
        app: app.clone(),
        path: "/mcp".into(),
        token: Some(token),
    });
    let authless = users.each_ref().map(|user| Client {
        app: build_authless_mcp_router(
            pool.clone(),
            &user.username,
            Some(user.clone()),
            vec!["localhost".into()],
            None,
            realtime::RealtimeHub::new(),
        )
        .layer(axum::Extension(barrier.clone())),
        path: format!("/mcp/{}", user.username),
        token: None,
    });

    for (route, clients) in [("authenticated", authenticated), ("authless", authless)] {
        let visible = call_pair(
            &clients,
            "list_resources",
            std::array::from_fn(|_| json!({"resource_type": "project"})),
        )
        .await;
        for (index, text) in visible.iter().enumerate() {
            assert!(
                text.contains(&format!("{} private project", users[index].username)),
                "{text}"
            );
            assert!(
                !text.contains(&format!("{} private project", users[1 - index].username)),
                "{text}"
            );
        }
        let created = call_pair(
            &clients,
            "create_issue",
            users.each_ref().map(|user| {
                json!({
                    "project": user.username.to_ascii_uppercase(),
                    "title": format!("{route} {} write", user.username)
                })
            }),
        )
        .await;
        for (user, text) in users.iter().zip(created) {
            assert!(text.starts_with("Created"), "{text}");
            let (actor, transport): (i64, String) = pool
                .read()
                .unwrap()
                .query_row(
                    "SELECT actor_user_id, transport FROM audit_log
                     WHERE entity_type = 'issue' AND action = 'create' AND new_value = ?1",
                    [format!("{route} {} write", user.username)],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(actor, user.id);
            assert_eq!(transport, "mcp");
        }
    }
}
