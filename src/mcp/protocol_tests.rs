//! Protocol compatibility over stdio: a 2025-03-26 client that starts with
//! `initialize`, and a 2026-07-28 client that starts with `server/discover`
//! and carries its version in every request's `_meta`. Both run the real
//! JSON-line codec and serving loop, the same path `lific mcp` serves.

use rmcp::ServiceExt;
use rmcp::transport::async_rw::AsyncRwTransport;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
};
use tokio::task::JoinHandle;
use tokio::time::timeout;

const DEADLINE: Duration = Duration::from_secs(5);

/// The per-request metadata a 2026-07-28 client sends instead of a handshake.
pub(crate) fn modern_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "modern-client", "version": "1.0"},
        "io.modelcontextprotocol/clientCapabilities": {}
    })
}

struct Session {
    input: WriteHalf<DuplexStream>,
    output: Lines<BufReader<ReadHalf<DuplexStream>>>,
    server: JoinHandle<()>,
}

impl Session {
    fn start() -> Self {
        let pool = crate::db::open_memory().unwrap();
        {
            let conn = pool.write().unwrap();
            crate::db::queries::users::create_passwordless_admin(&conn, "Operator").unwrap();
            crate::db::queries::create_project(
                &conn,
                &crate::db::models::CreateProject {
                    name: "Protocol project".into(),
                    identifier: "PRO".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        let (client, server) = tokio::io::duplex(1 << 16);
        let (reader, writer) = tokio::io::split(server);
        let service = crate::mcp::LificMcp::for_stdio(pool, None);
        let server = tokio::spawn(async move {
            service
                .serve(AsyncRwTransport::new_server(reader, writer))
                .await
                .expect("the session starts")
                .waiting()
                .await
                .expect("the session closes cleanly");
        });
        let (reader, input) = tokio::io::split(client);
        Self {
            input,
            output: BufReader::new(reader).lines(),
            server,
        }
    }

    async fn send(&mut self, message: Value) {
        let line = format!("{message}\n");
        timeout(DEADLINE, self.input.write_all(line.as_bytes()))
            .await
            .expect("write completes")
            .expect("write succeeds");
    }

    async fn request(&mut self, id: i64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        let line = timeout(DEADLINE, self.output.next_line())
            .await
            .expect("the server answers instead of hanging")
            .expect("read succeeds")
            .expect("the server keeps stdout open");
        let response: Value = serde_json::from_str(&line).expect("JSON-RPC response");
        assert_eq!(response["id"], id);
        response
    }

    async fn finish(mut self) {
        self.input.shutdown().await.unwrap();
        let _ = timeout(DEADLINE, self.server)
            .await
            .expect("the server stops on EOF");
    }
}

fn assert_lists_tools(response: &Value) {
    let tools = response["result"]["tools"].as_array().expect("tool list");
    assert!(
        tools.iter().any(|tool| tool["name"] == "list_resources"),
        "{response}"
    );
}

fn assert_lists_project(response: &Value) {
    assert_ne!(response["result"]["isError"], true, "{response}");
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .expect("text result");
    assert!(text.contains("Protocol project"), "{text}");
}

#[tokio::test]
async fn a_2025_03_26_client_initializes_lists_and_calls() {
    let mut session = Session::start();
    let init = session
        .request(
            1,
            "initialize",
            json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "legacy-client", "version": "1.0"}
            }),
        )
        .await;
    assert_eq!(init["result"]["protocolVersion"], "2025-03-26", "{init}");
    assert_eq!(init["result"]["serverInfo"]["name"], "lific");
    session
        .send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .await;

    let list = session.request(2, "tools/list", json!({})).await;
    assert_lists_tools(&list);
    let call = session
        .request(
            3,
            "tools/call",
            json!({"name": "list_resources", "arguments": {"resource_type": "project"}}),
        )
        .await;
    assert_lists_project(&call);
    // A legacy session keeps its historical result shape.
    assert!(call["result"].get("resultType").is_none(), "{call}");
    session.finish().await;
}

#[tokio::test]
async fn a_2026_07_28_client_discovers_lists_and_calls_without_initialize() {
    let mut session = Session::start();
    let discover = session
        .request(1, "server/discover", json!({"_meta": modern_meta()}))
        .await;
    let versions = discover["result"]["supportedVersions"]
        .as_array()
        .expect("supported versions");
    assert!(versions.contains(&json!("2026-07-28")), "{discover}");
    assert!(versions.contains(&json!("2025-03-26")), "{discover}");
    assert_eq!(
        discover["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "lific"
    );

    let list = session
        .request(2, "tools/list", json!({"_meta": modern_meta()}))
        .await;
    assert_lists_tools(&list);
    let call = session
        .request(
            3,
            "tools/call",
            json!({
                "_meta": modern_meta(),
                "name": "list_resources",
                "arguments": {"resource_type": "project"}
            }),
        )
        .await;
    assert_lists_project(&call);
    assert_eq!(call["result"]["resultType"], "complete", "{call}");
    session.finish().await;
}

/// VoidNullable/lific#57: a discovery probe used to end the stdio session
/// with no reply, so the client read EOF instead of something it could fall
/// back from. A probe without the metadata the server needs still gets an
/// answer, and the session then initializes normally.
#[tokio::test]
async fn a_bare_discovery_probe_is_answered_and_the_session_survives() {
    let mut session = Session::start();
    let probe = session.request(1, "server/discover", json!({})).await;
    assert!(
        probe.get("result").is_some() || probe.get("error").is_some(),
        "{probe}"
    );
    let init = session
        .request(
            2,
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "fallback-client", "version": "1.0"}
            }),
        )
        .await;
    assert_eq!(init["result"]["serverInfo"]["name"], "lific", "{init}");
    session
        .send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .await;
    let list = session.request(3, "tools/list", json!({})).await;
    assert_lists_tools(&list);
    session.finish().await;
}

/// A failed tool reports `isError: true` with its `Error: ` text unchanged,
/// and a successful one reports `isError: false`.
#[tokio::test]
async fn tool_failures_set_is_error_and_successes_do_not() {
    let mut session = Session::start();
    let meta = modern_meta();
    let missing = session
        .request(
            1,
            "tools/call",
            json!({"_meta": meta, "name": "get_issue", "arguments": {"identifier": "PRO-999"}}),
        )
        .await;
    assert_eq!(missing["result"]["isError"], true, "{missing}");
    let text = missing["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("Error: "), "{text}");
    assert!(text.contains("PRO-999"), "{text}");

    let listed = session
        .request(
            2,
            "tools/call",
            json!({"_meta": meta, "name": "list_resources", "arguments": {"resource_type": "project"}}),
        )
        .await;
    assert_eq!(listed["result"]["isError"], false, "{listed}");
    session.finish().await;
}

async fn initialized(session: &mut Session) -> Value {
    let init = session
        .request(
            1,
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "resource-client", "version": "1.0"}
            }),
        )
        .await;
    session
        .send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .await;
    init
}

#[tokio::test]
async fn issues_and_pages_are_readable_as_resources() {
    let mut session = Session::start();
    let init = initialized(&mut session).await;
    assert!(
        init["result"]["capabilities"]["resources"].is_object(),
        "{init}"
    );

    let templates = session
        .request(2, "resources/templates/list", json!({}))
        .await;
    let uris: Vec<&str> = templates["result"]["resourceTemplates"]
        .as_array()
        .expect("templates")
        .iter()
        .map(|template| template["uriTemplate"].as_str().unwrap())
        .collect();
    assert_eq!(
        uris,
        [
            "lific://{project}/issues/{identifier}",
            "lific://{project}/pages/{identifier}"
        ]
    );
    let listed = session.request(3, "resources/list", json!({})).await;
    assert_eq!(listed["result"]["resources"], json!([]), "{listed}");

    let created = session
        .request(
            4,
            "tools/call",
            json!({"name": "create_issue", "arguments": {"project": "PRO", "title": "Readable"}}),
        )
        .await;
    assert_ne!(created["result"]["isError"], true, "{created}");
    let tool = session
        .request(
            5,
            "tools/call",
            json!({"name": "get_issue", "arguments": {"identifier": "PRO-1"}}),
        )
        .await;
    let read = session
        .request(
            6,
            "resources/read",
            json!({"uri": "lific://PRO/issues/PRO-1"}),
        )
        .await;
    let contents = &read["result"]["contents"][0];
    assert_eq!(contents["uri"], "lific://PRO/issues/PRO-1", "{read}");
    assert_eq!(contents["mimeType"], "text/markdown");
    assert_eq!(contents["text"], tool["result"]["content"][0]["text"]);

    let page = session
        .request(
            7,
            "tools/call",
            json!({"name": "create_page", "arguments": {"project": "PRO", "title": "Guide", "content": "Read me."}}),
        )
        .await;
    assert_ne!(page["result"]["isError"], true, "{page}");
    let read = session
        .request(
            8,
            "resources/read",
            json!({"uri": "lific://PRO/pages/PRO-DOC-1"}),
        )
        .await;
    let text = read["result"]["contents"][0]["text"].as_str().unwrap();
    assert!(text.contains("Read me."), "{read}");

    let missing = session
        .request(
            9,
            "resources/read",
            json!({"uri": "lific://PRO/issues/PRO-99"}),
        )
        .await;
    assert_eq!(missing["error"]["code"], -32002, "{missing}");
    let foreign = session
        .request(
            10,
            "resources/read",
            json!({"uri": "lific://PRO/issues/OPS-1"}),
        )
        .await;
    assert_eq!(foreign["error"]["code"], -32002, "{foreign}");
    session.finish().await;
}
