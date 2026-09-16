use super::*;
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

/// Use the real JSON-line codec and serving loop so an unknown method must
/// survive deserialization, receive a wire response, and leave a usable session.
struct Session {
    input: WriteHalf<DuplexStream>,
    output: Lines<BufReader<ReadHalf<DuplexStream>>>,
    server: JoinHandle<()>,
}

impl Session {
    fn start() -> Self {
        let (client, server) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(server);
        let transport = PreInitGuard::new(AsyncRwTransport::new_server(reader, writer));
        let service = crate::mcp::LificMcp::for_stdio(crate::db::open_memory().unwrap(), None);
        let server = tokio::spawn(async move {
            service
                .serve(transport)
                .await
                .expect("initialize succeeds")
                .waiting()
                .await
                .expect("session closes cleanly");
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

    async fn request(&mut self, id: Value, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        let line = timeout(DEADLINE, self.output.next_line())
            .await
            .expect("server responds without closing stdin")
            .expect("read succeeds")
            .expect("server does not close stdout");
        let response: Value = serde_json::from_str(&line).expect("JSON-RPC response");
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], id);
        response
    }

    async fn initialize(&mut self) {
        let response = self
            .request(
                json!("initialize"),
                "initialize",
                json!({
                    "protocolVersion": "2026-07-28",
                    "capabilities": {},
                    "clientInfo": {"name": "discovery-regression", "version": "1.0"}
                }),
            )
            .await;
        assert_eq!(response["result"]["serverInfo"]["name"], "lific");
        assert_eq!(response["result"]["protocolVersion"], "2025-03-26");
        self.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await;
    }

    async fn assert_tools_available(&mut self) {
        let response = self.request(json!("tools"), "tools/list", json!({})).await;
        let tools = response["result"]["tools"].as_array().expect("tool list");
        assert!(tools.iter().any(|tool| tool["name"] == "list_resources"));
    }

    async fn finish(mut self) {
        self.input.shutdown().await.unwrap();
        timeout(DEADLINE, self.server)
            .await
            .expect("server stops on EOF")
            .expect("server task succeeds");
    }
}

#[tokio::test]
async fn discovery_rejection_allows_fallback_and_subsequent_requests() {
    let mut session = Session::start();
    for id in [json!(1), json!("discovery-probe")] {
        let response = session.request(id, "server/discover", json!({})).await;
        assert_eq!(response["error"]["code"], -32601);
    }
    session.initialize().await;
    session.assert_tools_available().await;

    // After initialization, rmcp resumes responsibility for unknown methods
    // and the session remains usable after its rejection too.
    let response = session
        .request(json!("after-init"), "server/discover", json!({}))
        .await;
    assert_eq!(response["error"]["code"], -32601);
    let response = session.request(json!("ping"), "ping", json!({})).await;
    assert_eq!(response["result"], json!({}));
    session.finish().await;
}

#[tokio::test]
async fn ordinary_initialization_still_lists_tools() {
    let mut session = Session::start();
    session.initialize().await;
    session.assert_tools_available().await;
    session.finish().await;
}

#[tokio::test]
async fn preinit_ping_and_ignored_traffic_preserve_the_handshake() {
    let mut session = Session::start();
    let response = session.request(json!(1), "ping", json!({})).await;
    assert_eq!(response["result"], json!({}));
    session
        .send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .await;
    session
        .send(json!({"jsonrpc": "2.0", "id": "unsolicited", "result": {}}))
        .await;
    session
        .send(
            json!({"jsonrpc": "2.0", "id": "unsolicited-error", "error": {
                "code": -32601, "message": "Method not found"
            }}),
        )
        .await;
    let response = session.request(json!(2), "tools/list", json!({})).await;
    assert_eq!(response["error"]["code"], -32601);
    let response = session.request(json!(3), "ping", json!({})).await;
    assert_eq!(response["result"], json!({}));
    session.initialize().await;
    session.assert_tools_available().await;
    session.finish().await;
}

#[tokio::test]
async fn failed_rejection_write_ends_the_handshake_without_waiting_for_more_input() {
    let (mut input, reader) = tokio::io::duplex(4096);
    let (output, writer) = tokio::io::duplex(4096);
    drop(output);
    let mut transport = PreInitGuard::new(AsyncRwTransport::new_server(reader, writer));
    input
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"server/discover\",\"params\":{}}\n")
        .await
        .unwrap();

    // Keep input open: ignoring BrokenPipe would hang waiting for another
    // request from a client that never received the discovery response.
    assert!(
        timeout(DEADLINE, transport.receive())
            .await
            .expect("failed output must stop the handshake")
            .is_none()
    );
    drop(input);
}

#[tokio::test]
async fn eof_before_initialize_ends_the_guard() {
    let (input, reader) = tokio::io::duplex(4096);
    drop(input);
    let mut transport = PreInitGuard::new(AsyncRwTransport::new_server(reader, tokio::io::sink()));
    assert!(
        timeout(DEADLINE, transport.receive())
            .await
            .expect("EOF must stop the guard")
            .is_none()
    );
}
