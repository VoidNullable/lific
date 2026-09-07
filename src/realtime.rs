use axum::extract::ws::{Message, WebSocket};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::{self, Duration, Instant};
use tracing::{trace, warn};

const EVENT_BUFFER: usize = 256;
/// The realtime protocol accepts only heartbeats and bounded
/// `activity.baseline.request` messages from clients. These small limits leave
/// room for control frames while bounding tungstenite's pre-handler buffers
/// well below its large defaults.
pub(crate) const MAX_CLIENT_MESSAGE_BYTES: usize = 16 * 1024;
pub(crate) const MAX_CLIENT_FRAME_BYTES: usize = 4 * 1024;
const ACTIVITY_BASELINE_CACHE_TTL: Duration = Duration::from_secs(60);
const CLIENT_MESSAGE_WINDOW: Duration = Duration::from_secs(10);
pub(crate) const MAX_CLIENT_MESSAGES_PER_WINDOW: usize = 64;
const CLIENT_PROGRESS_TIMEOUT: Duration = Duration::from_secs(120);
/// How often the server sends a WebSocket ping. Every conforming client answers
/// pings automatically at the protocol layer, so this keeps liveness detection
/// entirely inside the WebSocket spec: a passive client that never sends an
/// application message still proves it is alive, and only a peer that answers
/// nothing at all trips `CLIENT_PROGRESS_TIMEOUT`. Four pings fit inside the
/// timeout, so a single lost pong cannot disconnect a healthy client.
const SERVER_PING_INTERVAL: Duration = Duration::from_secs(30);
/// Bound on a single outbound send. `send().await` on a peer that has stopped
/// reading blocks once the kernel and TLS buffers fill, which would pin the
/// socket's task and its `SocketPermit` for as long as the peer cares to stall.
/// On timeout the socket is dropped instead, releasing both.
const SOCKET_SEND_TIMEOUT: Duration = Duration::from_secs(5);
// Retire idle revoked sockets too. Protected deliveries revalidate separately,
// so cross-process recovery does not wait for this timer to stop data access.
const SESSION_REVALIDATE_INTERVAL: Duration = Duration::from_secs(60);
/// Per-user cap on concurrent event sockets. Generous for real browser tabs,
/// but stops one authenticated client from accumulating unbounded server
/// tasks + broadcast receivers.
pub(crate) const MAX_SOCKETS_PER_USER: usize = 16;
/// Instance-wide cap on concurrent event sockets. The per-user cap alone only
/// bounds a single account, so `accounts * MAX_SOCKETS_PER_USER` sockets can
/// still exhaust file descriptors and task slots. This bounds the total.
/// Generous enough that a real instance never reaches it: 1024 sockets is 64
/// fully saturated users.
pub(crate) const MAX_SOCKETS_TOTAL: usize = 1024;
/// The instance cap must leave room for at least one fully saturated user, and
/// the socket-cap tests fill the instance budget a whole user at a time.
const _: () = assert!(
    MAX_SOCKETS_TOTAL >= MAX_SOCKETS_PER_USER
        && MAX_SOCKETS_TOTAL.is_multiple_of(MAX_SOCKETS_PER_USER)
);
/// LIF-440: how many recent seq-bearing events one project keeps around for
/// replay. The broadcast channel is the live path and forgets an event the
/// moment every live socket has seen it; this ring is what a socket that was
/// *not* live gets to read on reconnect. 1024 is roughly an hour of a busy
/// project's writes and costs a few hundred kilobytes per project at the
/// serialized sizes these envelopes have (ids only, no bodies).
pub(crate) const RING_CAPACITY: usize = 1024;
/// LIF-440: how long a buffered event stays replayable. Past this a
/// reconnecting client is told to `sync_required` and backfill from
/// `/api/projects/{id}/changes` instead, which is both cheaper and more
/// correct than keeping an unbounded tail in memory for a laptop that was
/// asleep all weekend. Enforced lazily on publish and on resume, so no
/// background task exists to leak.
pub(crate) const RING_MAX_AGE: Duration = Duration::from_secs(300);

#[derive(Debug, Default)]
struct SocketCounts {
    per_user: HashMap<i64, usize>,
    total: usize,
}

/// One buffered event, kept only long enough to answer a resume.
#[derive(Debug, Clone)]
struct BufferedEvent {
    seq: i64,
    published_at: Instant,
    /// The exact frame the live path sent, so a replayed event and a live one
    /// are byte-identical and a client cannot tell them apart.
    message: Message,
}

/// One project's replay ring (LIF-440).
///
/// `latest_seq` and `evicted` are what make a *negative* answer possible: the
/// events themselves can all be gone, and the ring still knows whether their
/// absence means "nothing happened" or "something happened and you missed it".
#[derive(Debug, Default)]
struct ProjectRing {
    events: VecDeque<BufferedEvent>,
    /// Highest seq ever published for this project, retained past eviction.
    latest_seq: i64,
    /// Whether anything has ever been dropped from `events`. While this is
    /// false the ring holds every event the project has ever published, so it
    /// covers every cursor, including a cursor far below the oldest seq
    /// present (which is common: seq is instance-wide, so a project's first
    /// event can carry a large seq).
    evicted: bool,
}

impl ProjectRing {
    /// Drop everything older than [`RING_MAX_AGE`]. Entries are pushed in
    /// publish order, so the front is always the oldest and this stops at the
    /// first entry still inside the window.
    fn expire(&mut self, now: Instant) {
        while self.events.front().is_some_and(|oldest| {
            now.saturating_duration_since(oldest.published_at) >= RING_MAX_AGE
        }) {
            self.events.pop_front();
            self.evicted = true;
        }
    }

    /// Can this ring account for everything published after `cursor`?
    fn covers(&self, cursor: i64) -> bool {
        if !self.evicted {
            return true;
        }
        match self.events.front() {
            // The oldest surviving event is the first one after the cursor (or
            // earlier), so nothing in between was lost.
            Some(oldest) => oldest.seq <= cursor.saturating_add(1),
            // Everything aged out. The client is only safe if it had already
            // seen everything this project ever published.
            None => self.latest_seq <= cursor,
        }
    }
}

/// What a resuming client is owed.
#[derive(Debug, PartialEq, Eq)]
enum ResumeOutcome {
    /// Every buffered event after the client's cursor, in publish order.
    Replay(Vec<Message>),
    /// The gap is wider than the buffer; the client must backfill from
    /// `/api/projects/{id}/changes`.
    SyncRequired,
}

/// Per-project rings of recently published seq-bearing events (LIF-440).
#[derive(Debug, Default)]
struct ReplayBuffer {
    projects: HashMap<i64, ProjectRing>,
}

impl ReplayBuffer {
    fn record(&mut self, project_id: i64, seq: i64, message: Message, now: Instant) {
        let ring = self.projects.entry(project_id).or_default();
        ring.expire(now);
        ring.latest_seq = ring.latest_seq.max(seq);
        ring.events.push_back(BufferedEvent {
            seq,
            published_at: now,
            message,
        });
        while ring.events.len() > RING_CAPACITY {
            ring.events.pop_front();
            ring.evicted = true;
        }
    }

    fn resume(&mut self, project_id: i64, cursor: i64, now: Instant) -> ResumeOutcome {
        let Some(ring) = self.projects.get_mut(&project_id) else {
            // Nothing seq-bearing has ever been published for this project on
            // this process, so there is nothing the client can have missed
            // that the buffer would know about.
            return ResumeOutcome::Replay(Vec::new());
        };
        ring.expire(now);
        if !ring.covers(cursor) {
            return ResumeOutcome::SyncRequired;
        }
        ResumeOutcome::Replay(
            ring.events
                .iter()
                .filter(|buffered| buffered.seq > cursor)
                .map(|buffered| buffered.message.clone())
                .collect(),
        )
    }
}

#[derive(Debug, Clone)]
pub struct RealtimeHub {
    tx: broadcast::Sender<RealtimeMessage>,
    revocations: broadcast::Sender<i64>,
    connections: Arc<Mutex<SocketCounts>>,
    /// LIF-440. A `std::sync::Mutex` rather than an async one on purpose:
    /// every critical section is a `VecDeque` push or a bounded scan, the
    /// publisher side (`send_message`) is a synchronous function called from
    /// request handlers, and nothing awaits while holding it.
    replay: Arc<Mutex<ReplayBuffer>>,
}

impl RealtimeHub {
    pub fn new() -> Self {
        Self::with_capacity(EVENT_BUFFER)
    }

    pub(crate) fn with_capacity(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        let (revocations, _) = broadcast::channel(capacity);
        Self {
            tx,
            revocations,
            connections: Arc::new(Mutex::new(SocketCounts::default())),
            replay: Arc::new(Mutex::new(ReplayBuffer::default())),
        }
    }

    /// Claim a connection slot for `user_id`, or `None` when the user already
    /// has `MAX_SOCKETS_PER_USER` live sockets or the instance already has
    /// `MAX_SOCKETS_TOTAL`. The returned permit releases the slot on drop, so a
    /// slot can never leak past its socket task.
    pub(crate) fn try_acquire_socket(&self, user_id: i64) -> Option<SocketPermit> {
        let mut connections = self.connections.lock().expect("connections lock poisoned");
        if connections.total >= MAX_SOCKETS_TOTAL {
            return None;
        }
        let count = connections.per_user.entry(user_id).or_insert(0);
        if *count >= MAX_SOCKETS_PER_USER {
            return None;
        }
        *count += 1;
        connections.total += 1;
        drop(connections);
        Some(SocketPermit {
            connections: Arc::clone(&self.connections),
            user_id,
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RealtimeMessage> {
        self.tx.subscribe()
    }

    pub fn send(&self, event: RealtimeEvent) {
        self.send_message(event, None, RealtimeAudience::Event);
    }

    /// Publish an event stamped with the `seq` of the row the mutation wrote
    /// (LIF-440). Only these events are replayable: `seq` is what lets a
    /// reconnecting client say "everything after C" and what lets it discard
    /// the duplicates a resume can produce. Seq-less events stay advisory —
    /// a client that sees one refreshes whatever view it names.
    pub fn send_with_seq(&self, event: RealtimeEvent, seq: i64) {
        self.send_message(event, Some(seq), RealtimeAudience::Event);
    }

    pub fn send_to_users(&self, event: RealtimeEvent, user_ids: Vec<i64>) {
        self.send_message(event, None, RealtimeAudience::Users(user_ids));
    }

    /// Immediately terminate every live socket for a user after account
    /// recovery. The normal periodic session check remains as a defense in
    /// depth for revocations made by other processes.
    pub fn revoke_user(&self, user_id: i64) {
        let _ = self.revocations.send(user_id);
    }

    /// Subscribe to the revocation stream, for tests that assert who was told.
    #[cfg(test)]
    pub(crate) fn subscribe_revocations(&self) -> broadcast::Receiver<i64> {
        self.revocations.subscribe()
    }

    fn send_message(&self, event: RealtimeEvent, seq: Option<i64>, audience: RealtimeAudience) {
        // A replayable event is project-scoped, seq-stamped and addressed to
        // everyone who can see the project. Anything else is advisory and
        // lives only as long as the live fan-out.
        let replayable = match (event.project_id(), seq, &audience) {
            (Some(project_id), Some(seq), RealtimeAudience::Event) => Some((project_id, seq)),
            _ => None,
        };
        if replayable.is_none() && self.tx.receiver_count() == 0 {
            trace!("dropped realtime event because no receivers are subscribed");
            return;
        }

        let Ok(json) = serde_json::to_string(&EventEnvelope { event: &event, seq }) else {
            warn!("failed to serialize realtime event");
            return;
        };
        let frame = Message::Text(json.into());

        // Buffer before the live fan-out, and regardless of who is listening:
        // "no receivers right now" is precisely the case a resume exists to
        // repair, so bailing out on an empty subscriber list would leave the
        // ring empty exactly when it matters.
        if let Some((project_id, seq)) = replayable {
            self.replay
                .lock()
                .expect("replay buffer lock poisoned")
                .record(project_id, seq, frame.clone(), Instant::now());
        }

        if self.tx.receiver_count() == 0 {
            trace!("dropped realtime event because no receivers are subscribed");
            return;
        }
        let message = RealtimeMessage {
            event,
            message: frame,
            audience,
        };
        if self.tx.send(message).is_err() {
            trace!("dropped realtime event because no receivers are subscribed");
        }
    }

    fn resume(&self, project_id: i64, cursor: i64, now: Instant) -> ResumeOutcome {
        self.replay
            .lock()
            .expect("replay buffer lock poisoned")
            .resume(project_id, cursor, now)
    }
}

/// The wire form of a realtime event: the event's own internally tagged
/// fields, plus the optional `seq` LIF-440 added. Kept as a separate
/// serialize-only shape so [`RealtimeEvent`] stays a plain enum and the
/// dozens of emit sites that have no seq to give need no change.
#[derive(Serialize)]
struct EventEnvelope<'a> {
    #[serde(flatten)]
    event: &'a RealtimeEvent,
    #[serde(skip_serializing_if = "Option::is_none")]
    seq: Option<i64>,
}

/// RAII guard for one live socket's slot in the per-user and instance-wide
/// connection counts.
#[must_use = "dropping the permit releases the socket slot"]
pub(crate) struct SocketPermit {
    connections: Arc<Mutex<SocketCounts>>,
    user_id: i64,
}

impl Drop for SocketPermit {
    fn drop(&mut self) {
        let mut connections = self.connections.lock().expect("connections lock poisoned");
        if let Some(count) = connections.per_user.get_mut(&self.user_id) {
            *count -= 1;
            if *count == 0 {
                connections.per_user.remove(&self.user_id);
            }
            connections.total = connections.total.saturating_sub(1);
        }
    }
}

#[derive(Debug, Clone)]
enum RealtimeAudience {
    Event,
    Users(Vec<i64>),
}

#[derive(Debug, Clone)]
pub struct RealtimeMessage {
    pub event: RealtimeEvent,
    pub message: Message,
    audience: RealtimeAudience,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum RealtimeRequest {
    #[serde(rename = "activity.baseline.request")]
    ActivityBaselineRequest,
    #[serde(rename = "heartbeat")]
    Heartbeat,
    /// LIF-440. A reconnecting client's opening move: "I have applied
    /// everything up to `cursor` in this project, give me the rest."
    #[serde(rename = "resume")]
    Resume { project_id: i64, cursor: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RealtimeEvent {
    #[serde(rename = "resync.required")]
    ResyncRequired,
    /// LIF-440: the answer to a `resume` the replay buffer can no longer
    /// cover. Unlike `resync.required` this is project-scoped and actionable:
    /// the client backfills from `/api/projects/{id}/changes` past its cursor
    /// rather than refetching everything it holds. Only ever sent directly to
    /// the socket that asked; it is never broadcast.
    #[serde(rename = "sync_required")]
    SyncRequired { project_id: i64 },
    #[serde(rename = "project.created")]
    ProjectCreated { project_id: i64 },
    #[serde(rename = "project.updated")]
    ProjectUpdated { project_id: i64 },
    #[serde(rename = "project.deleted")]
    ProjectDeleted { project_id: i64 },
    #[serde(rename = "projects.reordered")]
    ProjectsReordered,
    /// Addressed to the owning user through `send_to_users` rather than
    /// broadcast, since groups are per-user and every other client would only
    /// be told that someone reorganized their own sidebar.
    ///
    /// That is an audience, not a secret: `visible_to` lets any admin see
    /// every `Users(..)`-addressed event, the same superuser rule
    /// `can_view_project` and `visible_project_ids` follow. The payload
    /// carries no group data, and an admin's client refetches only its own
    /// groups, so what leaks is the bare fact that a user changed theirs.
    #[serde(rename = "project_groups.changed")]
    ProjectGroupsChanged,
    #[serde(rename = "issue.created")]
    IssueCreated { project_id: i64, issue_id: i64 },
    #[serde(rename = "issue.updated")]
    IssueUpdated { project_id: i64, issue_id: i64 },
    #[serde(rename = "issue.deleted")]
    IssueDeleted { project_id: i64, issue_id: i64 },
    #[serde(rename = "issue.linked")]
    IssueLinked { project_id: i64, issue_id: i64 },
    #[serde(rename = "issue.unlinked")]
    IssueUnlinked { project_id: i64, issue_id: i64 },
    #[serde(rename = "activity.baseline")]
    ActivityBaseline { day_count: i64 },
}

pub async fn serve_socket(
    mut socket: WebSocket,
    hub: RealtimeHub,
    db: crate::db::DbPool,
    session_token: String,
    mut auth_user: crate::db::models::AuthUser,
    _permit: SocketPermit,
) {
    let mut visible_projects = visible_projects_for(&db, &auth_user).await;
    let connected_at = Instant::now();
    let mut client = ClientState::new(connected_at);
    let mut rx = hub.subscribe();
    let mut revocations = hub.revocations.subscribe();
    let mut revalidate = time::interval_at(
        connected_at + SESSION_REVALIDATE_INTERVAL,
        SESSION_REVALIDATE_INTERVAL,
    );
    revalidate.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    let mut ping = time::interval_at(connected_at + SERVER_PING_INTERVAL, SERVER_PING_INTERVAL);
    ping.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

    loop {
        let progress_deadline = client.progress_deadline();
        let input = next_socket_input!(
            time::sleep_until(progress_deadline),
            revocations.recv(),
            revalidate.tick(),
            ping.tick(),
            rx.recv(),
            socket.recv(),
        );
        let flow = match input {
            SocketInput::Revalidate => {
                let flow =
                    revalidate_session(&mut socket, &db, &session_token, &mut auth_user).await;
                if flow == SocketFlow::Open {
                    visible_projects = visible_projects_for(&db, &auth_user).await;
                    client.invalidate_activity_baseline();
                }
                flow
            }
            SocketInput::Revocation(revoked) => match revocation_flow(revoked, auth_user.id) {
                RevocationFlow::Ignore => SocketFlow::Open,
                RevocationFlow::Revalidate => {
                    let flow =
                        revalidate_session(&mut socket, &db, &session_token, &mut auth_user).await;
                    if flow == SocketFlow::Open {
                        visible_projects = visible_projects_for(&db, &auth_user).await;
                        client.invalidate_activity_baseline();
                    }
                    flow
                }
                RevocationFlow::Close => close_socket(&mut socket).await,
            },
            SocketInput::Ping => send_bounded(&mut socket, Message::Ping(Vec::new().into())).await,
            SocketInput::Event(event) => {
                forward_event(
                    &mut socket,
                    &db,
                    &session_token,
                    &mut auth_user,
                    &mut visible_projects,
                    &mut client,
                    event,
                )
                .await
            }
            SocketInput::Message(message) => {
                handle_client_message(
                    &mut socket,
                    &hub,
                    &db,
                    &session_token,
                    &mut auth_user,
                    &mut client,
                    message,
                )
                .await
            }
            SocketInput::ProgressDeadline => close_socket(&mut socket).await,
        };
        if flow == SocketFlow::Close {
            break;
        }
    }
}

enum SocketInput {
    Revalidate,
    Revocation(Result<i64, RecvError>),
    Ping,
    Event(Result<RealtimeMessage, RecvError>),
    Message(Option<Result<Message, axum::Error>>),
    ProgressDeadline,
}

/// Pick the next thing the socket loop should act on, in priority order.
///
/// `biased` makes this an ordering policy, not a fair race, and the order is
/// the security-relevant part of the loop, so it lives in one macro that
/// production and tests both invoke. A test-only copy of the arm order would
/// pin the copy, not the thing that runs.
///
/// The policy, most urgent first:
///
/// 1. **Progress deadline.** A socket that has stopped making progress is
///    closed before anything else is attempted on it.
/// 2. **Revocation.** Access has just been taken away, so this is the other
///    fail-closed arm and nothing that merely serves the client may come
///    first. It sits *above* interval revalidation deliberately: a revocation
///    broadcast is already the answer, and making it wait behind an unrelated
///    revalidate would put a database round trip between a recovery and the
///    socket it is meant to close.
/// 3. **Interval revalidation**, the slow-path version of the same check.
/// 4. **Ping**, then **queued events**, then **client frames**: everything
///    that serves the connection rather than ending it. A socket with a busy
///    event stream must not be able to starve arms 1 to 3.
///
/// Each argument is a future; the payload types are exactly what the
/// corresponding [`SocketInput`] variant carries.
macro_rules! next_socket_input {
    (
        $progress:expr,
        $revocations:expr,
        $revalidate:expr,
        $ping:expr,
        $events:expr,
        $socket:expr $(,)?
    ) => {
        tokio::select! {
            biased;
            _ = $progress => $crate::realtime::SocketInput::ProgressDeadline,
            revoked = $revocations => $crate::realtime::SocketInput::Revocation(revoked),
            _ = $revalidate => $crate::realtime::SocketInput::Revalidate,
            _ = $ping => $crate::realtime::SocketInput::Ping,
            event = $events => $crate::realtime::SocketInput::Event(event),
            message = $socket => $crate::realtime::SocketInput::Message(message),
        }
    };
}
use next_socket_input;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RevocationFlow {
    Ignore,
    Revalidate,
    Close,
}

fn revocation_flow(revoked: Result<i64, RecvError>, user_id: i64) -> RevocationFlow {
    match revoked {
        Ok(id) if id != user_id => RevocationFlow::Ignore,
        Ok(_) | Err(RecvError::Closed) => RevocationFlow::Close,
        Err(RecvError::Lagged(_)) => RevocationFlow::Revalidate,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SocketFlow {
    Open,
    Close,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ClientAdmission {
    Accepted,
    RateLimited,
}

struct FixedWindowRateLimit {
    window_started: Instant,
    messages: usize,
}

impl FixedWindowRateLimit {
    fn new(now: Instant) -> Self {
        Self {
            window_started: now,
            messages: 0,
        }
    }

    #[must_use]
    fn admit(&mut self, now: Instant) -> ClientAdmission {
        if now.duration_since(self.window_started) >= CLIENT_MESSAGE_WINDOW {
            self.window_started = now;
            self.messages = 0;
        }
        self.messages += 1;
        if self.messages <= MAX_CLIENT_MESSAGES_PER_WINDOW {
            ClientAdmission::Accepted
        } else {
            ClientAdmission::RateLimited
        }
    }
}

struct CachedActivityBaseline {
    loaded_at: Instant,
    event: RealtimeEvent,
}

struct ClientState {
    rate_limit: FixedWindowRateLimit,
    progress_deadline: Instant,
    activity_baseline: Option<CachedActivityBaseline>,
}

impl ClientState {
    fn new(now: Instant) -> Self {
        Self {
            rate_limit: FixedWindowRateLimit::new(now),
            progress_deadline: now + CLIENT_PROGRESS_TIMEOUT,
            activity_baseline: None,
        }
    }

    fn progress_deadline(&self) -> Instant {
        self.progress_deadline
    }

    #[must_use]
    fn admit_message(&mut self, now: Instant) -> ClientAdmission {
        self.rate_limit.admit(now)
    }

    fn record_progress(&mut self, now: Instant) {
        self.progress_deadline = now + CLIENT_PROGRESS_TIMEOUT;
    }

    fn cached_activity_baseline(&self, now: Instant) -> Option<RealtimeEvent> {
        self.activity_baseline
            .as_ref()
            .filter(|cached| now.duration_since(cached.loaded_at) < ACTIVITY_BASELINE_CACHE_TTL)
            .map(|cached| cached.event.clone())
    }

    fn cache_activity_baseline(&mut self, now: Instant, event: RealtimeEvent) {
        self.activity_baseline = Some(CachedActivityBaseline {
            loaded_at: now,
            event,
        });
    }

    fn invalidate_activity_baseline(&mut self) {
        self.activity_baseline = None;
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ClientAction {
    Send(Message),
    ActivityBaseline,
    /// LIF-440: replay this project's buffered events past `cursor`.
    Resume {
        project_id: i64,
        cursor: i64,
    },
    Heartbeat,
    /// A reply to one of the server's own pings. Every conforming WebSocket
    /// client sends these without any application-level cooperation, which is
    /// what lets a passive client stay connected past `CLIENT_PROGRESS_TIMEOUT`.
    Pong,
    Close,
}

fn client_action(message: Message) -> ClientAction {
    match message {
        Message::Ping(payload) => ClientAction::Send(Message::Pong(payload)),
        Message::Pong(_) => ClientAction::Pong,
        Message::Text(text) => match serde_json::from_str::<RealtimeRequest>(&text) {
            Ok(RealtimeRequest::ActivityBaselineRequest) => ClientAction::ActivityBaseline,
            Ok(RealtimeRequest::Resume { project_id, cursor }) => {
                ClientAction::Resume { project_id, cursor }
            }
            Ok(RealtimeRequest::Heartbeat) => ClientAction::Heartbeat,
            Err(_) => ClientAction::Close,
        },
        Message::Binary(_) | Message::Close(_) => ClientAction::Close,
    }
}

impl SocketFlow {
    fn from_send(result: Result<(), axum::Error>) -> Self {
        match result {
            Ok(()) => Self::Open,
            Err(_) => Self::Close,
        }
    }
}

/// Send one message under `SOCKET_SEND_TIMEOUT`. Returning `Close` on timeout
/// makes `serve_socket` break its loop, which drops the socket and the
/// `SocketPermit` with it.
async fn send_bounded(socket: &mut WebSocket, message: Message) -> SocketFlow {
    bounded_send(socket.send(message)).await
}

async fn bounded_send<F>(send: F) -> SocketFlow
where
    F: std::future::Future<Output = Result<(), axum::Error>>,
{
    match time::timeout(SOCKET_SEND_TIMEOUT, send).await {
        Ok(result) => SocketFlow::from_send(result),
        Err(_) => {
            warn!("realtime websocket send timed out; dropping the socket");
            SocketFlow::Close
        }
    }
}

/// Best-effort courtesy close frame. Bounded for the same reason as every other
/// send: the socket is going away regardless, and a stalled peer must not be
/// able to delay that.
async fn send_close_frame(socket: &mut WebSocket) {
    let _ = time::timeout(SOCKET_SEND_TIMEOUT, socket.send(Message::Close(None))).await;
}

async fn revalidate_session(
    socket: &mut WebSocket,
    db: &crate::db::DbPool,
    session_token: &str,
    auth_user: &mut crate::db::models::AuthUser,
) -> SocketFlow {
    let db = db.clone();
    let session_token = session_token.to_owned();
    let state = tokio::task::spawn_blocking(move || session_state(&db, &session_token))
        .await
        .unwrap_or_else(|error| {
            SessionState::Error(crate::error::LificError::Internal(format!(
                "websocket session task failed: {error}"
            )))
        });
    match state {
        SessionState::Valid(user) => {
            *auth_user = user;
            SocketFlow::Open
        }
        SessionState::Invalid => {
            send_close_frame(socket).await;
            SocketFlow::Close
        }
        SessionState::Error(error) => {
            warn!(error = %error, "websocket session revalidation failed");
            send_close_frame(socket).await;
            SocketFlow::Close
        }
    }
}

async fn forward_event(
    socket: &mut WebSocket,
    db: &crate::db::DbPool,
    session_token: &str,
    auth_user: &mut crate::db::models::AuthUser,
    visible_projects: &mut Option<HashSet<i64>>,
    client: &mut ClientState,
    event: Result<RealtimeMessage, RecvError>,
) -> SocketFlow {
    if revalidate_session(socket, db, session_token, auth_user).await == SocketFlow::Close {
        return SocketFlow::Close;
    }
    match event {
        Ok(message) => {
            let visibility_db = db.clone();
            let visibility_user = auth_user.clone();
            let visibility_message = message.clone();
            let visibility = tokio::task::spawn_blocking(move || {
                visible_to(&visibility_db, &visibility_user, &visibility_message)
            })
            .await
            .unwrap_or_else(|error| {
                warn!(error = %error, "websocket visibility task failed");
                EventVisibility::Hidden
            });
            match visibility {
                EventVisibility::Visible => {
                    if let Some(project_id) = message.event.project_id() {
                        if matches!(message.event, RealtimeEvent::ProjectDeleted { .. }) {
                            if let Some(projects) = visible_projects {
                                projects.remove(&project_id);
                            }
                        } else if let Some(projects) = visible_projects {
                            projects.insert(project_id);
                        }
                    }
                    send_bounded(socket, message.message).await
                }
                EventVisibility::Hidden => {
                    let revoked = matches!(message.event, RealtimeEvent::ProjectUpdated { .. })
                        && message.event.project_id().is_some_and(|project_id| {
                            visible_projects
                                .as_mut()
                                .is_some_and(|projects| projects.remove(&project_id))
                        });
                    if revoked {
                        send_resync(socket, client).await
                    } else {
                        SocketFlow::Open
                    }
                }
            }
        }
        Err(RecvError::Lagged(dropped)) => {
            warn!(
                dropped,
                "realtime websocket lagged; asking client to resync"
            );
            send_resync(socket, client).await
        }
        Err(RecvError::Closed) => SocketFlow::Close,
    }
}

async fn handle_client_message(
    socket: &mut WebSocket,
    hub: &RealtimeHub,
    db: &crate::db::DbPool,
    session_token: &str,
    auth_user: &mut crate::db::models::AuthUser,
    client: &mut ClientState,
    message: Option<Result<Message, axum::Error>>,
) -> SocketFlow {
    match message {
        Some(Ok(Message::Close(_))) | Some(Err(_)) | None => SocketFlow::Close,
        Some(Ok(message)) => {
            let now = Instant::now();
            if client.admit_message(now) == ClientAdmission::RateLimited {
                return close_socket(socket).await;
            }

            match client_action(message) {
                ClientAction::Send(message) => send_bounded(socket, message).await,
                ClientAction::ActivityBaseline => {
                    client.record_progress(now);
                    send_activity_baseline(socket, db, session_token, auth_user, client).await
                }
                ClientAction::Resume { project_id, cursor } => {
                    client.record_progress(now);
                    replay_for_client(
                        socket,
                        hub,
                        db,
                        session_token,
                        auth_user,
                        project_id,
                        cursor,
                    )
                    .await
                }
                ClientAction::Heartbeat | ClientAction::Pong => {
                    client.record_progress(now);
                    SocketFlow::Open
                }
                ClientAction::Close => close_socket(socket).await,
            }
        }
    }
}

/// Answer a client's `resume` frame (LIF-440).
///
/// Two outcomes, and only two: either every seq-bearing event this project
/// published after `cursor` goes out in publish order, or the client is told
/// `sync_required` and backfills from `/api/projects/{id}/changes`. There is
/// no third "here is some of it" outcome, because a partial replay is exactly
/// the silent gap the whole mechanism exists to prevent.
///
/// **Duplicates are fine, gaps are not.** The socket subscribed to the live
/// broadcast before it ever read this frame, so an event published between
/// those two moments is delivered twice: once from the ring, once live. That
/// is deliberate. The alternative — hold live events back until the replay is
/// flushed — buys nothing, because the client's cursor discipline ("ignore
/// anything at or below the cursor I have already applied") makes a duplicate
/// a no-op, while any scheme that risks dropping an event is unrecoverable.
///
/// Credentials and project access are checked before each frame, since recovery
/// or membership changes can commit while an earlier network send awaits. A
/// client that cannot see the project gets `sync_required`, which tells it
/// nothing it could not learn by calling `/changes` and being refused.
async fn replay_for_client(
    socket: &mut WebSocket,
    hub: &RealtimeHub,
    db: &crate::db::DbPool,
    session_token: &str,
    auth_user: &mut crate::db::models::AuthUser,
    project_id: i64,
    cursor: i64,
) -> SocketFlow {
    if revalidate_session(socket, db, session_token, auth_user).await == SocketFlow::Close {
        return SocketFlow::Close;
    }
    if !project_visible(db, auth_user, project_id).await {
        return send_event(socket, &RealtimeEvent::SyncRequired { project_id }).await;
    }
    match hub.resume(project_id, cursor, Instant::now()) {
        ResumeOutcome::SyncRequired => {
            send_event(socket, &RealtimeEvent::SyncRequired { project_id }).await
        }
        ResumeOutcome::Replay(messages) => {
            for message in messages {
                if revalidate_session(socket, db, session_token, auth_user).await
                    == SocketFlow::Close
                {
                    return SocketFlow::Close;
                }
                if !project_visible(db, auth_user, project_id).await {
                    return send_event(socket, &RealtimeEvent::SyncRequired { project_id }).await;
                }
                if send_bounded(socket, message).await == SocketFlow::Close {
                    return SocketFlow::Close;
                }
            }
            SocketFlow::Open
        }
    }
}

async fn project_visible(
    db: &crate::db::DbPool,
    auth_user: &crate::db::models::AuthUser,
    project_id: i64,
) -> bool {
    let db = db.clone();
    let auth_user = auth_user.clone();
    tokio::task::spawn_blocking(move || {
        let identity = crate::resolve_caller::ResolvedIdentity {
            user: auth_user,
            transport: crate::actor::Transport::Web,
        };
        crate::authz::can_view_project(&db, &identity, project_id).unwrap_or(false)
    })
    .await
    .unwrap_or_else(|error| {
        warn!(error = %error, "websocket replay visibility task failed");
        false
    })
}

async fn send_activity_baseline(
    socket: &mut WebSocket,
    db: &crate::db::DbPool,
    session_token: &str,
    auth_user: &mut crate::db::models::AuthUser,
    client: &mut ClientState,
) -> SocketFlow {
    let was_admin = auth_user.is_admin;
    if revalidate_session(socket, db, session_token, auth_user).await == SocketFlow::Close {
        return SocketFlow::Close;
    }
    if was_admin != auth_user.is_admin {
        client.invalidate_activity_baseline();
    }
    let now = Instant::now();
    let baseline = match client.cached_activity_baseline(now) {
        Some(event) => Ok(event),
        None => {
            let baseline_db = db.clone();
            let baseline_user = auth_user.clone();
            tokio::task::spawn_blocking(move || activity_baseline(&baseline_db, &baseline_user))
                .await
                .unwrap_or_else(|error| {
                    Err(crate::error::LificError::Internal(format!(
                        "websocket baseline task failed: {error}"
                    )))
                })
                .inspect(|event| client.cache_activity_baseline(now, event.clone()))
        }
    };
    match baseline_response(baseline) {
        RealtimeEvent::ResyncRequired => send_resync(socket, client).await,
        event => send_event(socket, &event).await,
    }
}

/// Send `resync.required`, dropping the cached activity baseline first.
///
/// A resync tells the client every cached view it holds is stale, and the
/// server's own `ACTIVITY_BASELINE_CACHE_TTL` cache is one of those views. Left
/// alone, the client's very next `activity.baseline.request` would be answered
/// from a snapshot taken up to 60 seconds before the event that forced the
/// resync, so the resync would hand back the same stale number it was meant to
/// correct. Every resync path routes through here for that reason.
async fn send_resync(socket: &mut WebSocket, client: &mut ClientState) -> SocketFlow {
    client.invalidate_activity_baseline();
    send_event(socket, &RealtimeEvent::ResyncRequired).await
}

fn baseline_response(baseline: Result<RealtimeEvent, crate::error::LificError>) -> RealtimeEvent {
    match baseline {
        Ok(event) => event,
        Err(error) => {
            warn!(error = %error, "failed to load websocket activity baseline");
            RealtimeEvent::ResyncRequired
        }
    }
}

async fn close_socket(socket: &mut WebSocket) -> SocketFlow {
    send_close_frame(socket).await;
    SocketFlow::Close
}

fn activity_baseline(
    db: &crate::db::DbPool,
    auth_user: &crate::db::models::AuthUser,
) -> Result<RealtimeEvent, crate::error::LificError> {
    let identity = crate::resolve_caller::ResolvedIdentity {
        user: auth_user.clone(),
        transport: crate::actor::Transport::Web,
    };
    let visible_projects = crate::authz::visible_project_ids(db, &Some(identity))?;
    let conn = db.read()?;
    let day_count = crate::db::queries::activity::activity_count(&conn, visible_projects.as_ref())?;
    Ok(RealtimeEvent::ActivityBaseline { day_count })
}

async fn send_event(socket: &mut WebSocket, event: &RealtimeEvent) -> SocketFlow {
    match serde_json::to_string(event) {
        Ok(json) => send_bounded(socket, Message::Text(json.into())).await,
        Err(_) => {
            warn!("failed to serialize realtime event");
            close_socket(socket).await
        }
    }
}

enum SessionState {
    Valid(crate::db::models::AuthUser),
    Invalid,
    Error(crate::error::LificError),
}

fn session_state(db: &crate::db::DbPool, token: &str) -> SessionState {
    match session_user(db, token) {
        Ok(Some(user)) => SessionState::Valid(user),
        Ok(None) => SessionState::Invalid,
        Err(error) => SessionState::Error(error),
    }
}

fn session_user(
    db: &crate::db::DbPool,
    token: &str,
) -> Result<Option<crate::db::models::AuthUser>, crate::error::LificError> {
    let conn = db.read()?;
    match crate::db::queries::users::validate_session(&conn, token) {
        Ok(user) => Ok(Some(crate::db::models::AuthUser {
            id: user.id,
            username: user.username,
            display_name: user.display_name,
            is_admin: user.is_admin,
        })),
        Err(crate::error::LificError::BadRequest(message))
            if message == crate::db::queries::users::INVALID_SESSION_MESSAGE =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

async fn visible_projects_for(
    db: &crate::db::DbPool,
    auth_user: &crate::db::models::AuthUser,
) -> Option<HashSet<i64>> {
    let db = db.clone();
    let auth_user = auth_user.clone();
    tokio::task::spawn_blocking(move || query_visible_projects(&db, &auth_user))
        .await
        .unwrap_or_else(|error| {
            warn!(error = %error, "websocket project visibility task failed");
            None
        })
}

fn query_visible_projects(
    db: &crate::db::DbPool,
    auth_user: &crate::db::models::AuthUser,
) -> Option<HashSet<i64>> {
    let identity = crate::resolve_caller::ResolvedIdentity {
        user: auth_user.clone(),
        transport: crate::actor::Transport::Web,
    };
    crate::authz::visible_project_ids(db, &Some(identity))
        .ok()
        .flatten()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EventVisibility {
    Visible,
    Hidden,
}

fn visible_to(
    db: &crate::db::DbPool,
    auth_user: &crate::db::models::AuthUser,
    message: &RealtimeMessage,
) -> EventVisibility {
    match &message.audience {
        RealtimeAudience::Users(user_ids) => {
            if auth_user.is_admin || user_ids.contains(&auth_user.id) {
                EventVisibility::Visible
            } else {
                EventVisibility::Hidden
            }
        }
        RealtimeAudience::Event => match message.event.project_id() {
            Some(project_id) => {
                let identity = crate::resolve_caller::ResolvedIdentity {
                    user: auth_user.clone(),
                    transport: crate::actor::Transport::Web,
                };
                match crate::authz::can_view_project(db, &identity, project_id) {
                    Ok(true) => EventVisibility::Visible,
                    Ok(false) | Err(_) => EventVisibility::Hidden,
                }
            }
            None => EventVisibility::Visible,
        },
    }
}

impl RealtimeEvent {
    fn project_id(&self) -> Option<i64> {
        match self {
            Self::ProjectCreated { project_id }
            | Self::ProjectUpdated { project_id }
            | Self::ProjectDeleted { project_id }
            | Self::IssueCreated { project_id, .. }
            | Self::IssueUpdated { project_id, .. }
            | Self::IssueDeleted { project_id, .. }
            | Self::IssueLinked { project_id, .. }
            | Self::IssueUnlinked { project_id, .. }
            | Self::SyncRequired { project_id } => Some(*project_id),
            Self::ResyncRequired
            | Self::ProjectsReordered
            | Self::ProjectGroupsChanged
            | Self::ActivityBaseline { .. } => None,
        }
    }
}

#[cfg(test)]
mod recovery_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_serializes_with_dotted_type() {
        let event = RealtimeEvent::IssueUpdated {
            project_id: 7,
            issue_id: 42,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "issue.updated");
        assert_eq!(json["project_id"], 7);
        assert_eq!(json["issue_id"], 42);
    }

    #[test]
    fn activity_baseline_serializes_with_day_count() {
        let event = RealtimeEvent::ActivityBaseline { day_count: 123 };
        let json = serde_json::to_value(&event).unwrap();

        assert_eq!(json["type"], "activity.baseline");
        assert_eq!(json["day_count"], 123);
    }

    #[test]
    fn activity_baseline_rechecks_current_project_visibility() {
        let (db, auth_user, project_id, _) = visibility_fixture(true);
        {
            let conn = db.write().unwrap();
            conn.execute("UPDATE audit_log SET ts = datetime('now', '-25 hours')", [])
                .unwrap();
            crate::db::queries::create_issue(
                &conn,
                &crate::db::models::CreateIssue {
                    project_id,
                    title: "Visible activity".into(),
                    description: String::new(),
                    status: crate::db::models::Status::Backlog,
                    priority: crate::db::models::Priority::None,
                    module_id: None,
                    start_date: None,
                    target_date: None,
                    labels: vec![],
                    source: None,
                    ..Default::default()
                },
            )
            .unwrap();
        }

        assert_eq!(
            activity_baseline(&db, &auth_user).unwrap(),
            RealtimeEvent::ActivityBaseline { day_count: 1 }
        );

        {
            let conn = db.write().unwrap();
            crate::db::queries::members::remove_member(&conn, project_id, auth_user.id).unwrap();
        }

        assert_eq!(
            activity_baseline(&db, &auth_user).unwrap(),
            RealtimeEvent::ActivityBaseline { day_count: 0 }
        );
    }

    #[tokio::test]
    async fn lagged_receiver_requests_resync() {
        let hub = RealtimeHub::with_capacity(1);
        let mut rx = hub.subscribe();

        hub.send(RealtimeEvent::ProjectUpdated { project_id: 1 });
        hub.send(RealtimeEvent::ProjectUpdated { project_id: 2 });

        assert!(matches!(rx.recv().await, Err(RecvError::Lagged(1))));
        assert_eq!(
            event_json(rx.recv().await.unwrap().message)["project_id"],
            2
        );
    }

    // ── LIF-440: replay ring, resume, sync_required ──────────────

    /// Publish with a seq, exactly as a mutation handler does. The hub has no
    /// subscribers in these tests on purpose: the ring must fill anyway, since
    /// "nobody was listening" is the case a resume exists to repair.
    fn publish(hub: &RealtimeHub, project_id: i64, issue_id: i64, seq: i64) {
        hub.send_with_seq(
            RealtimeEvent::IssueUpdated {
                project_id,
                issue_id,
            },
            seq,
        );
    }

    fn replayed_seqs(outcome: ResumeOutcome) -> Vec<i64> {
        match outcome {
            ResumeOutcome::Replay(messages) => messages
                .into_iter()
                .map(|message| event_json(message)["seq"].as_i64().unwrap())
                .collect(),
            ResumeOutcome::SyncRequired => panic!("expected a replay, got sync_required"),
        }
    }

    #[test]
    fn published_events_carry_the_seq_of_the_row_they_describe() {
        let hub = RealtimeHub::new();
        let mut rx = hub.subscribe();

        publish(&hub, 1, 42, 17);

        let json = event_json(rx.try_recv().unwrap().message);
        assert_eq!(json["type"], "issue.updated");
        assert_eq!(json["project_id"], 1);
        assert_eq!(json["issue_id"], 42);
        assert_eq!(json["seq"], 17);
    }

    /// A seq-less event is advisory: it goes out on the wire without a `seq`
    /// key at all, and it never enters the replay ring.
    #[test]
    fn seq_less_events_omit_seq_and_are_not_buffered() {
        let hub = RealtimeHub::new();
        let mut rx = hub.subscribe();

        hub.send(RealtimeEvent::ProjectUpdated { project_id: 1 });

        let json = event_json(rx.try_recv().unwrap().message);
        assert_eq!(json["type"], "project.updated");
        assert!(json.get("seq").is_none(), "advisory events carry no seq");
        assert_eq!(
            replayed_seqs(hub.resume(1, 0, Instant::now())),
            Vec::<i64>::new()
        );
    }

    /// Events addressed to specific users are never replayed: the ring answers
    /// a project resume, and a per-user audience is not a project fact.
    #[test]
    fn user_addressed_events_are_not_buffered() {
        let hub = RealtimeHub::new();
        hub.send_to_users(RealtimeEvent::ProjectUpdated { project_id: 1 }, vec![7]);
        assert_eq!(
            replayed_seqs(hub.resume(1, 0, Instant::now())),
            Vec::<i64>::new()
        );
    }

    #[test]
    fn resume_replays_exactly_the_tail_after_the_cursor_in_order() {
        let hub = RealtimeHub::new();
        for seq in 1..=6 {
            publish(&hub, 1, seq, seq);
        }

        assert_eq!(
            replayed_seqs(hub.resume(1, 3, Instant::now())),
            vec![4, 5, 6]
        );
    }

    /// The cursor is inclusive of what the client already applied, so resuming
    /// at the newest seq replays nothing, and resuming from scratch replays
    /// everything the ring still holds.
    #[test]
    fn resume_boundaries_replay_nothing_and_everything() {
        let hub = RealtimeHub::new();
        for seq in 1..=3 {
            publish(&hub, 1, seq, seq);
        }

        assert_eq!(
            replayed_seqs(hub.resume(1, 3, Instant::now())),
            Vec::<i64>::new()
        );
        assert_eq!(
            replayed_seqs(hub.resume(1, 0, Instant::now())),
            vec![1, 2, 3]
        );
    }

    /// A project this process has published nothing for cannot have a gap, so
    /// the client is told to carry on rather than to resync.
    #[test]
    fn resume_for_a_silent_project_replays_nothing() {
        let hub = RealtimeHub::new();
        assert_eq!(
            replayed_seqs(hub.resume(404, 99, Instant::now())),
            Vec::<i64>::new()
        );
    }

    /// Instance-wide seq means a project's first event can carry a large seq
    /// while the ring has still never dropped anything. That ring covers every
    /// cursor, including zero, and must not force a needless full sync.
    #[test]
    fn an_unevicted_ring_covers_a_cursor_below_its_oldest_seq() {
        let hub = RealtimeHub::new();
        publish(&hub, 1, 1, 5_000);
        publish(&hub, 1, 2, 5_001);

        assert_eq!(
            replayed_seqs(hub.resume(1, 0, Instant::now())),
            vec![5_000, 5_001]
        );
    }

    #[test]
    fn a_cursor_evicted_by_ring_capacity_requires_a_full_sync() {
        let hub = RealtimeHub::new();
        let published = RING_CAPACITY as i64 + 10;
        for seq in 1..=published {
            publish(&hub, 1, seq, seq);
        }

        // The oldest event still buffered is well past seq 1, so the events
        // between are unrecoverable from memory.
        assert_eq!(
            hub.resume(1, 1, Instant::now()),
            ResumeOutcome::SyncRequired
        );
        // A cursor inside the surviving window is still served.
        assert_eq!(
            replayed_seqs(hub.resume(1, published - 2, Instant::now())),
            vec![published - 1, published]
        );
    }

    /// Age eviction, driven by an injected timestamp rather than a sleep.
    #[test]
    fn events_older_than_the_max_age_are_dropped_and_force_a_sync() {
        let hub = RealtimeHub::new();
        let published_at = Instant::now();
        for seq in 1..=3 {
            publish(&hub, 1, seq, seq);
        }

        // Still inside the window: everything replays.
        assert_eq!(
            replayed_seqs(hub.resume(1, 0, published_at + RING_MAX_AGE - Duration::from_secs(1))),
            vec![1, 2, 3]
        );

        // Past it: the entries are gone, and a cursor that predates them can
        // no longer be served from memory.
        let expired = published_at + RING_MAX_AGE + Duration::from_secs(1);
        assert_eq!(hub.resume(1, 0, expired), ResumeOutcome::SyncRequired);
        // A client that had already applied everything needs nothing, so the
        // same emptied ring answers it without a resync.
        assert_eq!(replayed_seqs(hub.resume(1, 3, expired)), Vec::<i64>::new());
    }

    /// Age eviction also runs on publish, so a long-quiet project does not
    /// carry a stale tail into its next burst of activity.
    #[test]
    fn publishing_expires_the_entries_that_aged_out_while_the_project_was_quiet() {
        let hub = RealtimeHub::new();
        publish(&hub, 1, 1, 1);

        // Nothing is published for longer than the max age, then one event is.
        // Asserted on the ring itself rather than through `resume`, because
        // `resume` expires too and would pass either way.
        let mut buffer = hub.replay.lock().unwrap();
        let now = Instant::now() + RING_MAX_AGE + Duration::from_secs(1);
        buffer.record(1, 2, Message::Text(r#"{"seq":2}"#.into()), now);

        let ring = &buffer.projects[&1];
        assert_eq!(
            ring.events
                .iter()
                .map(|event| event.seq)
                .collect::<Vec<_>>(),
            vec![2],
            "the aged-out entry must not survive the next publish"
        );
        assert!(ring.evicted, "dropping it is recorded as a coverage gap");
    }

    #[test]
    fn replay_rings_are_scoped_to_one_project() {
        let hub = RealtimeHub::new();
        publish(&hub, 1, 10, 1);
        publish(&hub, 2, 20, 2);
        publish(&hub, 1, 11, 3);

        assert_eq!(replayed_seqs(hub.resume(1, 0, Instant::now())), vec![1, 3]);
        assert_eq!(replayed_seqs(hub.resume(2, 0, Instant::now())), vec![2]);
        // Project 2's ring saturating does nothing to project 1's coverage.
        for seq in 100..=(RING_CAPACITY as i64 + 200) {
            publish(&hub, 2, seq, seq);
        }
        assert_eq!(replayed_seqs(hub.resume(1, 0, Instant::now())), vec![1, 3]);
    }

    /// A replayed frame is byte-identical to the live one, so a client cannot
    /// tell them apart and duplicates at the resume boundary are inert.
    #[test]
    fn replayed_frames_are_the_frames_the_live_path_sent() {
        let hub = RealtimeHub::new();
        let mut rx = hub.subscribe();
        publish(&hub, 1, 42, 9);

        let live = rx.try_recv().unwrap().message;
        let ResumeOutcome::Replay(replayed) = hub.resume(1, 8, Instant::now()) else {
            panic!("expected a replay");
        };
        assert_eq!(replayed, vec![live]);
    }

    #[test]
    fn resume_frames_parse_into_a_replay_action() {
        assert_eq!(
            client_action(Message::Text(
                r#"{"type":"resume","project_id":7,"cursor":42}"#.into()
            )),
            ClientAction::Resume {
                project_id: 7,
                cursor: 42
            }
        );
        // A malformed resume is still a protocol violation, same as any other
        // unrecognized frame.
        assert_eq!(
            client_action(Message::Text(r#"{"type":"resume"}"#.into())),
            ClientAction::Close
        );
    }

    #[test]
    fn sync_required_serializes_with_its_project() {
        let json = serde_json::to_value(RealtimeEvent::SyncRequired { project_id: 7 }).unwrap();
        assert_eq!(json["type"], "sync_required");
        assert_eq!(json["project_id"], 7);
    }

    fn event_json(message: Message) -> serde_json::Value {
        match message {
            Message::Text(text) => serde_json::from_str(&text).unwrap(),
            other => panic!("expected text event, got {other:?}"),
        }
    }

    #[test]
    fn socket_slots_are_capped_per_user_and_released_on_drop() {
        let hub = RealtimeHub::new();
        let mut slots: Vec<SocketPermit> = (0..MAX_SOCKETS_PER_USER)
            .map(|_| hub.try_acquire_socket(7).expect("slot under the cap"))
            .collect();

        // A different user is unaffected by user 7's saturation.
        assert!(hub.try_acquire_socket(8).is_some());
        // User 7 is at the cap.
        assert!(hub.try_acquire_socket(7).is_none());

        // Dropping one slot frees exactly one.
        drop(slots.pop());
        assert!(hub.try_acquire_socket(7).is_some());
    }

    #[test]
    fn socket_slots_are_capped_instance_wide_across_users() {
        let hub = RealtimeHub::new();
        let users = MAX_SOCKETS_TOTAL / MAX_SOCKETS_PER_USER;
        let mut slots: Vec<SocketPermit> = (0..users as i64)
            .flat_map(|user_id| (0..MAX_SOCKETS_PER_USER).map(move |_| (user_id, ())))
            .map(|(user_id, ())| {
                hub.try_acquire_socket(user_id)
                    .expect("slot under both caps")
            })
            .collect();
        assert_eq!(slots.len(), MAX_SOCKETS_TOTAL);

        // A brand new user is under the per-user cap yet still refused: the
        // instance-wide budget is what is exhausted.
        assert!(hub.try_acquire_socket(9_999).is_none());

        // And the global count is released by the same RAII drop.
        slots.pop();
        assert!(hub.try_acquire_socket(9_999).is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn a_send_that_never_completes_closes_the_socket() {
        // A peer that stops reading leaves `send()` pending forever. The bound
        // turns that into a close, which drops the socket and its permit.
        assert_eq!(
            bounded_send(std::future::pending::<Result<(), axum::Error>>()).await,
            SocketFlow::Close
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_send_that_completes_in_time_keeps_the_socket_open() {
        assert_eq!(
            bounded_send(std::future::ready(Ok(()))).await,
            SocketFlow::Open
        );
    }

    #[test]
    fn server_pings_fit_inside_the_progress_timeout() {
        // A healthy client must survive a lost pong, so more than one ping has
        // to land inside the timeout window.
        assert!(SERVER_PING_INTERVAL * 2 < CLIENT_PROGRESS_TIMEOUT);
    }

    #[test]
    fn client_data_limits_are_pinned() {
        assert_eq!(MAX_CLIENT_FRAME_BYTES, 4 * 1024);
        assert_eq!(MAX_CLIENT_MESSAGE_BYTES, 16 * 1024);
    }

    #[test]
    fn client_actions_cover_every_supported_message_kind() {
        assert_eq!(
            client_action(Message::Ping(vec![1, 2, 3].into())),
            ClientAction::Send(Message::Pong(vec![1, 2, 3].into()))
        );
        assert_eq!(
            client_action(Message::Pong(Vec::new().into())),
            ClientAction::Pong
        );
        assert_eq!(
            client_action(Message::Text(r#"{"type":"heartbeat"}"#.into())),
            ClientAction::Heartbeat
        );
        assert_eq!(
            client_action(Message::Text(
                r#"{"type":"activity.baseline.request"}"#.into()
            )),
            ClientAction::ActivityBaseline
        );
        assert_eq!(
            client_action(Message::Text(r#"{"type":"unknown"}"#.into())),
            ClientAction::Close
        );
        assert_eq!(
            client_action(Message::Binary(Vec::new().into())),
            ClientAction::Close
        );
        assert_eq!(client_action(Message::Close(None)), ClientAction::Close);
    }

    #[test]
    fn fixed_window_rate_limit_resets_at_the_window_boundary() {
        let started = Instant::now();
        let mut limit = FixedWindowRateLimit::new(started);

        for _ in 0..MAX_CLIENT_MESSAGES_PER_WINDOW {
            assert_eq!(limit.admit(started), ClientAdmission::Accepted);
        }
        assert_eq!(limit.admit(started), ClientAdmission::RateLimited);
        assert_eq!(
            limit.admit(started + CLIENT_MESSAGE_WINDOW),
            ClientAdmission::Accepted
        );
    }

    #[test]
    fn rate_admission_does_not_extend_the_progress_deadline() {
        let started = Instant::now();
        let mut client = ClientState::new(started);
        let received_at = started + Duration::from_secs(30);

        assert_eq!(client.admit_message(received_at), ClientAdmission::Accepted);
        assert_eq!(
            client.progress_deadline(),
            started + CLIENT_PROGRESS_TIMEOUT
        );
    }

    #[test]
    fn application_message_extends_the_progress_deadline() {
        let started = Instant::now();
        let mut client = ClientState::new(started);
        let received_at = started + Duration::from_secs(30);

        client.record_progress(received_at);
        assert_eq!(
            client.progress_deadline(),
            received_at + CLIENT_PROGRESS_TIMEOUT
        );
    }

    #[test]
    fn activity_baseline_cache_expires_at_its_ttl_boundary() {
        let loaded_at = Instant::now();
        let mut client = ClientState::new(loaded_at);
        let event = RealtimeEvent::ActivityBaseline { day_count: 7 };
        client.cache_activity_baseline(loaded_at, event.clone());

        assert_eq!(
            client.cached_activity_baseline(
                loaded_at + ACTIVITY_BASELINE_CACHE_TTL - Duration::from_nanos(1)
            ),
            Some(event)
        );
        assert_eq!(
            client.cached_activity_baseline(loaded_at + ACTIVITY_BASELINE_CACHE_TTL),
            None
        );
    }

    #[test]
    fn activity_baseline_cache_can_be_invalidated_after_revalidation() {
        let loaded_at = Instant::now();
        let mut client = ClientState::new(loaded_at);
        client.cache_activity_baseline(loaded_at, RealtimeEvent::ActivityBaseline { day_count: 7 });

        client.invalidate_activity_baseline();

        assert_eq!(client.cached_activity_baseline(loaded_at), None);
    }

    #[test]
    fn baseline_errors_request_a_client_resync() {
        assert_eq!(
            baseline_response(Err(crate::error::LificError::Internal("test".into()))),
            RealtimeEvent::ResyncRequired
        );
    }

    #[test]
    fn revoke_user_broadcasts_immediately_to_socket_tasks() {
        let hub = RealtimeHub::new();
        let mut rx = hub.revocations.subscribe();
        hub.revoke_user(42);
        assert_eq!(rx.try_recv().unwrap(), 42);
    }

    #[test]
    fn revocation_receiver_lag_revalidates_and_closure_fails_closed() {
        assert_eq!(
            revocation_flow(Err(RecvError::Lagged(1)), 42),
            RevocationFlow::Revalidate
        );
        assert_eq!(
            revocation_flow(Err(RecvError::Closed), 42),
            RevocationFlow::Close
        );
        assert_eq!(revocation_flow(Ok(7), 42), RevocationFlow::Ignore);
        assert_eq!(revocation_flow(Ok(42), 42), RevocationFlow::Close);
    }

    /// One recovery must reach every socket the account has open, and no
    /// socket belonging to anyone else. The hub is a fan-out broadcast, so
    /// this is about the subscription, not the flow decision above.
    #[test]
    fn one_revocation_reaches_every_socket_the_account_has_open() {
        let hub = RealtimeHub::new();
        let mut first = hub.revocations.subscribe();
        let mut second = hub.revocations.subscribe();

        hub.revoke_user(42);

        assert_eq!(
            revocation_flow(Ok(first.try_recv().unwrap()), 42),
            RevocationFlow::Close
        );
        assert_eq!(
            revocation_flow(Ok(second.try_recv().unwrap()), 42),
            RevocationFlow::Close
        );
        // The same message, judged by a socket belonging to someone else.
        assert_eq!(revocation_flow(Ok(42), 7), RevocationFlow::Ignore);
    }

    /// A revocation is dispatched as an ordinary [`SocketInput`], so once its
    /// arm is selected it runs through the same loop body as pings, events and
    /// client frames, and its three outcomes are the same `SocketFlow` values
    /// every other arm produces. Losing that would mean losing the send
    /// budget, the progress timeout and the bounded `close_socket` the rest of
    /// the loop depends on. (Its *priority* among the arms is a separate
    /// question, pinned by
    /// `the_socket_loop_takes_inputs_in_the_documented_priority_order`.)
    ///
    /// There is no harness for driving a live `axum::extract::ws::WebSocket`
    /// in-process, so the socket half of the loop is pinned by construction
    /// rather than by assertion: `revocation_flow` returns `SocketFlow`-shaped
    /// decisions only, and the `Close` arm has nowhere to go but
    /// `close_socket`.
    #[test]
    fn revocation_outcomes_are_the_same_three_the_loop_already_handles() {
        for (input, flow) in [
            (Ok(42), RevocationFlow::Close),
            (Ok(7), RevocationFlow::Ignore),
            (Err(RecvError::Lagged(3)), RevocationFlow::Revalidate),
            (Err(RecvError::Closed), RevocationFlow::Close),
        ] {
            assert_eq!(revocation_flow(input, 42), flow);
        }
    }

    /// The loop's priority order, exercised through the very macro the loop
    /// invokes. Each arm is handed either a ready future or one that never
    /// completes, so which variant comes out is decided purely by the `biased`
    /// ordering inside `next_socket_input!` and nothing else.
    ///
    /// This is the only place that ordering is checked, and it checks the
    /// production definition rather than a restatement of it: change the arm
    /// order in the macro and these assertions fail.
    #[tokio::test]
    async fn the_socket_loop_takes_inputs_in_the_documented_priority_order() {
        use std::future::{pending, ready};

        // Payload types are the real ones each `SocketInput` variant carries.
        macro_rules! revocation {
            (ready) => {
                ready(Ok::<i64, RecvError>(7))
            };
            (never) => {
                pending::<Result<i64, RecvError>>()
            };
        }
        macro_rules! event {
            (ready) => {
                ready(Err::<RealtimeMessage, RecvError>(RecvError::Closed))
            };
            (never) => {
                pending::<Result<RealtimeMessage, RecvError>>()
            };
        }
        macro_rules! frame {
            (ready) => {
                ready(None::<Result<Message, axum::Error>>)
            };
            (never) => {
                pending::<Option<Result<Message, axum::Error>>>()
            };
        }

        // Everything ready at once: the progress deadline wins outright.
        let input = next_socket_input!(
            ready(()),
            revocation!(ready),
            ready(()),
            ready(()),
            event!(ready),
            frame!(ready),
        );
        assert!(
            matches!(input, SocketInput::ProgressDeadline),
            "a stalled socket is closed before anything else is attempted"
        );

        // Revocation outranks revalidation, ping, events and client frames.
        let input = next_socket_input!(
            pending::<()>(),
            revocation!(ready),
            ready(()),
            ready(()),
            event!(ready),
            frame!(ready),
        );
        assert!(
            matches!(input, SocketInput::Revocation(Ok(7))),
            "a ready revocation must not wait behind a revalidate DB round trip, a ping, a queued event or a client frame"
        );

        // Then the slow-path revalidation.
        let input = next_socket_input!(
            pending::<()>(),
            revocation!(never),
            ready(()),
            ready(()),
            event!(ready),
            frame!(ready),
        );
        assert!(matches!(input, SocketInput::Revalidate));

        // Then the arms that merely serve the connection, in order.
        let input = next_socket_input!(
            pending::<()>(),
            revocation!(never),
            pending::<()>(),
            ready(()),
            event!(ready),
            frame!(ready),
        );
        assert!(matches!(input, SocketInput::Ping));

        let input = next_socket_input!(
            pending::<()>(),
            revocation!(never),
            pending::<()>(),
            pending::<()>(),
            event!(ready),
            frame!(ready),
        );
        assert!(matches!(input, SocketInput::Event(Err(RecvError::Closed))));

        let input = next_socket_input!(
            pending::<()>(),
            revocation!(never),
            pending::<()>(),
            pending::<()>(),
            event!(never),
            frame!(ready),
        );
        assert!(matches!(input, SocketInput::Message(None)));
    }

    #[test]
    fn project_event_is_visible_to_project_viewer() {
        let (db, auth_user, project_id, _) = visibility_fixture(true);
        let event = RealtimeEvent::IssueUpdated {
            project_id,
            issue_id: 42,
        };

        assert_eq!(
            visible_to(&db, &auth_user, &event_message(event)),
            EventVisibility::Visible
        );
    }

    #[test]
    fn project_event_is_hidden_from_non_member_when_authz_is_enforced() {
        let (db, auth_user, project_id, _) = visibility_fixture(false);
        let event = RealtimeEvent::IssueUpdated {
            project_id,
            issue_id: 42,
        };

        assert_eq!(
            visible_to(&db, &auth_user, &event_message(event)),
            EventVisibility::Hidden
        );
    }

    #[test]
    fn deleted_project_snapshot_is_visible_after_project_is_deleted() {
        let (db, auth_user, project_id, _) = visibility_fixture(true);
        {
            let conn = db.write().unwrap();
            crate::db::queries::delete_project(&conn, project_id).unwrap();
        }

        let message = RealtimeMessage {
            event: RealtimeEvent::ProjectDeleted { project_id },
            message: Message::Text("{}".into()),
            audience: RealtimeAudience::Users(vec![auth_user.id]),
        };

        assert_eq!(
            visible_to(&db, &auth_user, &message),
            EventVisibility::Visible
        );
    }

    fn event_message(event: RealtimeEvent) -> RealtimeMessage {
        RealtimeMessage {
            event,
            message: Message::Text("{}".into()),
            audience: RealtimeAudience::Event,
        }
    }

    fn visibility_fixture(
        member: bool,
    ) -> (crate::db::DbPool, crate::db::models::AuthUser, i64, String) {
        let db = crate::db::open_memory().unwrap();
        let (auth_user, project_id, token) = {
            let conn = db.write().unwrap();
            crate::db::queries::settings::update(
                &conn,
                crate::db::queries::settings::InstanceSettingsPatch {
                    authz_enforced: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
            let user = crate::db::queries::users::create_user(
                &conn,
                &crate::db::models::CreateUser {
                    username: "viewer".into(),
                    email: "viewer@example.test".into(),
                    password: "password".into(),
                    display_name: Some("Viewer".into()),
                    is_admin: false,
                    is_bot: false,
                },
            )
            .unwrap();
            let project = crate::db::queries::create_project(
                &conn,
                &crate::db::models::CreateProject {
                    name: "Visible".into(),
                    identifier: "VIS".into(),
                    description: String::new(),
                    emoji: None,
                    lead_user_id: None,
                },
            )
            .unwrap();
            if member {
                crate::db::queries::members::upsert_member(
                    &conn,
                    project.id,
                    user.id,
                    crate::db::models::Role::Viewer,
                )
                .unwrap();
            }
            let token = crate::db::queries::users::create_session(&conn, user.id, None)
                .unwrap()
                .token;
            (
                crate::db::models::AuthUser {
                    id: user.id,
                    username: user.username,
                    display_name: user.display_name,
                    is_admin: user.is_admin,
                },
                project.id,
                token,
            )
        };
        (db, auth_user, project_id, token)
    }
}
