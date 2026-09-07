//! LIF-419: a recovery that commits while a socket is already open must stop
//! that socket's *next delivery*, not merely its next periodic session check.
//!
//! Authorization on this path starts before a commit and can finish after it.
//! Every delivery decision (`forward_event`, `send_activity_baseline`,
//! `replay_for_client`) crosses at least one `.await` between "is this session
//! still live?" and "write the bytes", so `lock_down_account` can land in the
//! gap. These tests therefore drive the real `serve_socket` over a real
//! loopback WebSocket and revoke *without* calling `RealtimeHub::revoke_user`:
//! that is the cross-process case (a CLI `lific user set-password`, another
//! server process) where no in-process broadcast exists to close the socket,
//! and where the only thing standing between the revoked client and live data
//! is the revalidation on the delivery path itself.
//!
//! The harness never holds a database lock across an await: every write is a
//! synchronous helper that takes the writer guard and drops it before
//! returning.

use super::*;
use axum::extract::{Query, State, ws::WebSocketUpgrade};
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use tokio::net::{TcpSocket, TcpStream};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as ClientMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

type TestSocket = WebSocketStream<TcpStream>;

const BASELINE_REQUEST: &str = r#"{"type":"activity.baseline.request"}"#;
/// Any real frame arrives in milliseconds on loopback; this only exists so a
/// regression hangs the one test rather than the whole suite.
const FRAME_TIMEOUT: Duration = Duration::from_secs(10);
/// Pinned on both ends of every test connection so the amount of replay that
/// can sit in flight is a known, small number rather than whatever the kernel
/// autotunes to (this host allows a 32 MiB receive buffer). That is what makes
/// the mid-replay barrier below deterministic instead of a race.
const SOCKET_BUFFER_BYTES: u32 = 64 * 1024;
const REPLAY_FRAME_BYTES: usize = 128 * 1024;
/// 32 x 128 KiB = 4 MiB of replay against a quarter megabyte of buffer, so the
/// server is guaranteed to park inside a `send` long before the last frame.
const REPLAY_FRAMES: i64 = 32;

// ── harness ──────────────────────────────────────────────────

#[derive(Clone)]
struct WsState {
    db: crate::db::DbPool,
    hub: RealtimeHub,
}

#[derive(serde::Deserialize)]
struct TokenQuery {
    token: String,
}

/// The test route. It wires the production `serve_socket` exactly as
/// `api::events_ws` does, differing only in how the session token arrives, so
/// what these tests exercise is the shipped loop rather than a stand-in.
async fn upgrade(
    State(state): State<WsState>,
    Query(query): Query<TokenQuery>,
    ws: WebSocketUpgrade,
) -> axum::response::Response {
    let WsState { db, hub } = state;
    let user = session_user(&db, &query.token)
        .expect("session lookup")
        .expect("the test connects with a live session");
    let permit = hub.try_acquire_socket(user.id).expect("socket slot");
    ws.on_upgrade(move |socket| serve_socket(socket, hub, db, query.token, user, permit))
}

struct Recovery {
    db: crate::db::DbPool,
    hub: RealtimeHub,
    address: SocketAddr,
    project_id: i64,
    human_id: i64,
    human_token: String,
    bot_token: String,
    bystander_token: String,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Recovery {
    fn drop(&mut self) {
        // Nothing outlives the test: the listener and its socket tasks die
        // with this handle, so no server survives the process.
        self.server.abort();
    }
}

/// Not async: everything here is synchronous, including `tokio::spawn`, which
/// only needs a runtime in scope rather than an await.
fn fixture() -> Recovery {
    let db = crate::db::open_memory().expect("test db");
    let hub = RealtimeHub::new();

    // Scoped so the writer guard is released before the first await.
    let (project_id, human_id, human_token, bot_token, bystander_token) = {
        let conn = db.write().expect("write connection");
        let human = user(&conn, "human");
        // A connected-tool bot the human owns. `lock_down_account` severs the
        // owner's credentials *and* every bot's, so this session is collateral
        // of the same commit.
        let bot = crate::db::queries::users::create_bot_user(
            &conn,
            human.id,
            "human-bot",
            "Human's tool",
            Some("tool"),
        )
        .expect("bot user");
        let bystander = user(&conn, "bystander");
        let project = crate::db::queries::create_project(
            &conn,
            &crate::db::models::CreateProject {
                name: "Realtime".into(),
                identifier: "RT".into(),
                description: String::new(),
                emoji: None,
                lead_user_id: None,
            },
        )
        .expect("project");
        (
            project.id,
            human.id,
            session(&conn, human.id),
            session(&conn, bot.id),
            session(&conn, bystander.id),
        )
    };

    let listener = TcpSocket::new_v4().expect("listener socket");
    listener
        .set_send_buffer_size(SOCKET_BUFFER_BYTES)
        .expect("listener send buffer");
    listener
        .bind("127.0.0.1:0".parse().expect("loopback address"))
        .expect("bind");
    let listener = listener.listen(64).expect("listen");
    let address = listener.local_addr().expect("bound address");
    let app = axum::Router::new()
        .route("/ws", get(upgrade))
        .with_state(WsState {
            db: db.clone(),
            hub: hub.clone(),
        });
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("test server");
    });

    Recovery {
        db,
        hub,
        address,
        project_id,
        human_id,
        human_token,
        bot_token,
        bystander_token,
        server,
    }
}

fn user(conn: &rusqlite::Connection, username: &str) -> crate::db::models::User {
    crate::db::queries::users::create_user(
        conn,
        &crate::db::models::CreateUser {
            username: username.into(),
            email: format!("{username}@test.local"),
            password: "testpassword1".into(),
            display_name: None,
            is_admin: false,
            is_bot: false,
        },
    )
    .expect("user")
}

fn session(conn: &rusqlite::Connection, user_id: i64) -> String {
    crate::db::queries::users::create_session(conn, user_id, None)
        .expect("session")
        .token
}

impl Recovery {
    async fn connect(&self, token: &str) -> TestSocket {
        let client = TcpSocket::new_v4().expect("client socket");
        client
            .set_recv_buffer_size(SOCKET_BUFFER_BYTES)
            .expect("client receive buffer");
        let stream = client.connect(self.address).await.expect("connect");
        let request = format!("ws://{}/ws?token={token}", self.address)
            .into_client_request()
            .expect("client request");
        let (socket, _) = tokio_tungstenite::client_async(request, stream)
            .await
            .expect("websocket handshake");
        socket
    }

    /// Connect and wait for one answered request. That answer is proof of two
    /// things the tests depend on: `serve_socket` subscribes to the hub before
    /// it ever reads a client frame, so the socket cannot miss an event
    /// published after this returns, and the client's activity baseline is now
    /// cached server-side.
    async fn primed(&self, token: &str) -> TestSocket {
        let mut socket = self.connect(token).await;
        send(&mut socket, BASELINE_REQUEST).await;
        assert_eq!(
            next_frame(&mut socket).await.expect("baseline answer")["type"],
            "activity.baseline"
        );
        socket
    }

    fn publish(&self, seq: i64) {
        self.hub.send_with_seq(
            RealtimeEvent::IssueUpdated {
                project_id: self.project_id,
                issue_id: seq,
            },
            seq,
        );
    }

    /// Commit the recovery the way another process would: sever every
    /// credential of the human and of the bots they own, and tell the hub
    /// nothing. `revoke_user` is deliberately never called, so the only thing
    /// that can close these sockets is the delivery path revalidating.
    fn lock_down(&self) {
        let conn = self.db.write().expect("write connection");
        crate::db::queries::users::lock_down_account(&conn, self.human_id).expect("lock down");
    }

    /// Fill one project's ring with frames too large for the pinned socket
    /// buffers to swallow whole. The server therefore parks inside a replay
    /// `send` until the client reads, and that park is the barrier the
    /// mid-replay test uses to commit a recovery *between* two frames without
    /// any test hook in production code.
    fn buffer_padded_replay(&self) {
        let mut replay = self.hub.replay.lock().expect("replay buffer lock");
        let now = Instant::now();
        for seq in 1..=REPLAY_FRAMES {
            let frame = serde_json::json!({
                "type": "issue.updated",
                "project_id": self.project_id,
                "issue_id": seq,
                "seq": seq,
                "pad": "x".repeat(REPLAY_FRAME_BYTES),
            })
            .to_string();
            replay.record(self.project_id, seq, Message::Text(frame.into()), now);
        }
    }
}

async fn send(socket: &mut TestSocket, request: &str) {
    socket
        .send(ClientMessage::Text(request.into()))
        .await
        .expect("client send");
}

fn resume_request(project_id: i64) -> String {
    format!(r#"{{"type":"resume","project_id":{project_id},"cursor":0}}"#)
}

/// The next application frame, or `None` once the server has closed the
/// socket. Protocol pings are skipped: either side may emit one at any time
/// and neither is a delivery.
async fn next_frame(socket: &mut TestSocket) -> Option<serde_json::Value> {
    loop {
        let received = tokio::time::timeout(FRAME_TIMEOUT, socket.next())
            .await
            .expect("the socket must produce a frame or a close within the timeout");
        match received {
            Some(Ok(ClientMessage::Text(text))) => {
                return Some(serde_json::from_str(&text).expect("json frame"));
            }
            Some(Ok(ClientMessage::Ping(_) | ClientMessage::Pong(_))) => continue,
            Some(Ok(ClientMessage::Close(_))) | Some(Err(_)) | None => return None,
            other => panic!("unexpected frame {other:?}"),
        }
    }
}

// ── tests ────────────────────────────────────────────────────

/// The live path. One recovery, no hub revocation: the human's own socket and
/// the socket of a bot they own are both closed instead of being handed the
/// next event, while a session the recovery did not touch keeps receiving.
#[tokio::test]
async fn a_recovery_closes_the_human_and_its_bot_before_the_next_live_event() {
    let recovery = fixture();
    let mut human = recovery.primed(&recovery.human_token).await;
    let mut bot = recovery.primed(&recovery.bot_token).await;
    let mut bystander = recovery.primed(&recovery.bystander_token).await;

    // All three are live deliveries before the recovery, so what changes
    // below is the recovery and nothing else.
    recovery.publish(1);
    assert_eq!(next_frame(&mut human).await.expect("live event")["seq"], 1);
    assert_eq!(next_frame(&mut bot).await.expect("live event")["seq"], 1);
    assert_eq!(
        next_frame(&mut bystander).await.expect("live event")["seq"],
        1
    );

    recovery.lock_down();
    recovery.publish(2);

    assert_eq!(
        next_frame(&mut human).await,
        None,
        "a severed human session must be closed, not handed the next event"
    );
    assert_eq!(
        next_frame(&mut bot).await,
        None,
        "a bot's session falls with its owner's, on the same delivery"
    );
    assert_eq!(
        next_frame(&mut bystander).await.expect("live event")["seq"],
        2,
        "an untouched session keeps receiving data"
    );
}

/// The cached path. A primed socket can be answered from `ClientState`'s
/// activity-baseline cache without reading the database at all, so the cache
/// is exactly where a revoked session would keep getting served if the
/// revalidation sat behind the cache lookup instead of in front of it.
#[tokio::test]
async fn a_recovery_closes_a_socket_the_baseline_cache_could_still_answer() {
    let recovery = fixture();
    let mut human = recovery.primed(&recovery.human_token).await;

    recovery.lock_down();
    send(&mut human, BASELINE_REQUEST).await;

    assert_eq!(
        next_frame(&mut human).await,
        None,
        "the cached baseline must not answer a severed session"
    );
}

/// The queued path. These events are buffered before the socket exists, so
/// the only way they can reach the client is a replay, and the replay is
/// requested after the recovery has committed.
#[tokio::test]
async fn a_recovery_closes_a_socket_before_its_queued_replay() {
    let recovery = fixture();
    for seq in 1..=3 {
        recovery.publish(seq);
    }
    let mut human = recovery.primed(&recovery.human_token).await;

    recovery.lock_down();
    send(&mut human, &resume_request(recovery.project_id)).await;

    assert_eq!(
        next_frame(&mut human).await,
        None,
        "a severed session must be closed rather than replayed to"
    );
}

/// The same path, gated *per frame*. The recovery commits after the replay has
/// already started and while the server is parked inside a send, which is the
/// case a single check at the top of the replay would miss entirely.
#[tokio::test]
async fn a_recovery_stops_a_replay_that_is_already_in_flight() {
    let recovery = fixture();
    recovery.buffer_padded_replay();
    let mut human = recovery.primed(&recovery.human_token).await;

    send(&mut human, &resume_request(recovery.project_id)).await;
    // Reading one frame proves the replay loop is running. The client then
    // stops reading, so the rest of the replay fills the pinned buffers and
    // the server parks mid-send until this test lets it continue.
    assert_eq!(
        next_frame(&mut human).await.expect("first replay frame")["seq"],
        1
    );

    recovery.lock_down();

    // Draining releases the parked send. The frame it was already committed to
    // may still land, but every frame after it is gated afresh, so the replay
    // cannot run to the end.
    let mut last = 1;
    while last < REPLAY_FRAMES {
        match next_frame(&mut human).await {
            Some(frame) => last = frame["seq"].as_i64().expect("seq"),
            None => break,
        }
    }
    assert!(
        last < REPLAY_FRAMES,
        "the replay must stop at the recovery, but it delivered {last} of {REPLAY_FRAMES} frames"
    );
}
