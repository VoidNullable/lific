use axum::extract::ws::{Message, WebSocket};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::{self, Duration};
use tracing::{trace, warn};

const EVENT_BUFFER: usize = 256;
const SESSION_REVALIDATE_INTERVAL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone)]
pub struct RealtimeHub {
    tx: broadcast::Sender<RealtimeMessage>,
}

impl RealtimeHub {
    pub fn new() -> Self {
        Self::with_capacity(EVENT_BUFFER)
    }

    fn with_capacity(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RealtimeMessage> {
        self.tx.subscribe()
    }

    pub fn send(&self, event: RealtimeEvent) {
        self.send_message(event, RealtimeAudience::Event);
    }

    pub fn send_to_users(&self, event: RealtimeEvent, user_ids: Vec<i64>) {
        self.send_message(event, RealtimeAudience::Users(user_ids));
    }

    fn send_message(&self, event: RealtimeEvent, audience: RealtimeAudience) {
        match self.tx.receiver_count() {
            0 => trace!("dropped realtime event because no receivers are subscribed"),
            _ => match serde_json::to_string(&event) {
                Ok(json) => {
                    let message = RealtimeMessage {
                        event,
                        message: Message::Text(json.into()),
                        audience,
                    };
                    if self.tx.send(message).is_err() {
                        trace!("dropped realtime event because no receivers are subscribed");
                    }
                }
                Err(_) => warn!("failed to serialize realtime event"),
            },
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RealtimeEvent {
    #[serde(rename = "resync.required")]
    ResyncRequired,
    #[serde(rename = "project.created")]
    ProjectCreated { project_id: i64 },
    #[serde(rename = "project.updated")]
    ProjectUpdated { project_id: i64 },
    #[serde(rename = "project.deleted")]
    ProjectDeleted { project_id: i64 },
    #[serde(rename = "projects.reordered")]
    ProjectsReordered,
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
}

pub async fn serve_socket(
    mut socket: WebSocket,
    hub: RealtimeHub,
    db: crate::db::DbPool,
    session_token: String,
) {
    let mut auth_user = match session_user(&db, &session_token) {
        Ok(Some(user)) => user,
        Ok(None) => {
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
        Err(e) => {
            warn!(error = %e, "websocket session lookup failed");
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
    };
    let mut visible_projects = visible_projects_for(&db, &auth_user);
    let mut rx = hub.subscribe();
    let mut revalidate = time::interval(SESSION_REVALIDATE_INTERVAL);
    revalidate.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

    while let SocketFlow::Open = tokio::select! {
        _ = revalidate.tick() => {
            let flow = revalidate_session(&mut socket, &db, &session_token, &mut auth_user).await;
            if flow == SocketFlow::Open {
                visible_projects = visible_projects_for(&db, &auth_user);
            }
            flow
        },
        event = rx.recv() => forward_event(&mut socket, &db, &auth_user, &mut visible_projects, event).await,
        message = socket.recv() => handle_client_message(&mut socket, message).await,
    } {}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SocketFlow {
    Open,
    Close,
}

impl SocketFlow {
    fn from_send(result: Result<(), axum::Error>) -> Self {
        match result {
            Ok(()) => Self::Open,
            Err(_) => Self::Close,
        }
    }
}

async fn revalidate_session(
    socket: &mut WebSocket,
    db: &crate::db::DbPool,
    session_token: &str,
    auth_user: &mut crate::db::models::AuthUser,
) -> SocketFlow {
    match session_state(db, session_token) {
        SessionState::Valid(user) => {
            *auth_user = user;
            SocketFlow::Open
        }
        SessionState::Invalid => {
            let _ = socket.send(Message::Close(None)).await;
            SocketFlow::Close
        }
        SessionState::Error(error) => {
            warn!(error = %error, "websocket session revalidation failed");
            let _ = socket.send(Message::Close(None)).await;
            SocketFlow::Close
        }
    }
}

async fn forward_event(
    socket: &mut WebSocket,
    db: &crate::db::DbPool,
    auth_user: &crate::db::models::AuthUser,
    visible_projects: &mut Option<HashSet<i64>>,
    event: Result<RealtimeMessage, RecvError>,
) -> SocketFlow {
    match event {
        Ok(message) => match event_flow(db, auth_user, &message) {
            EventFlow::Forward => {
                if let Some(project_id) = message.event.project_id() {
                    if matches!(message.event, RealtimeEvent::ProjectDeleted { .. }) {
                        if let Some(projects) = visible_projects {
                            projects.remove(&project_id);
                        }
                    } else if let Some(projects) = visible_projects {
                        projects.insert(project_id);
                    }
                }
                SocketFlow::from_send(socket.send(message.message).await)
            }
            EventFlow::Drop => {
                let revoked = matches!(message.event, RealtimeEvent::ProjectUpdated { .. })
                    && message
                        .event
                        .project_id()
                        .is_some_and(|project_id| {
                            visible_projects
                                .as_mut()
                                .is_some_and(|projects| projects.remove(&project_id))
                        });
                if revoked {
                    send_event(socket, &RealtimeEvent::ResyncRequired).await
                } else {
                    SocketFlow::Open
                }
            }
        },
        Err(RecvError::Lagged(dropped)) => {
            warn!(
                dropped,
                "realtime websocket lagged; asking client to resync"
            );
            send_event(socket, &RealtimeEvent::ResyncRequired).await
        }
        Err(RecvError::Closed) => SocketFlow::Close,
    }
}

async fn handle_client_message(
    socket: &mut WebSocket,
    message: Option<Result<Message, axum::Error>>,
) -> SocketFlow {
    match message {
        Some(Ok(Message::Ping(payload))) => {
            SocketFlow::from_send(socket.send(Message::Pong(payload)).await)
        }
        Some(Ok(Message::Close(_))) | Some(Err(_)) | None => SocketFlow::Close,
        Some(Ok(Message::Pong(_))) | Some(Ok(_)) => SocketFlow::Open,
    }
}

async fn send_event(socket: &mut WebSocket, event: &RealtimeEvent) -> SocketFlow {
    match serde_json::to_string(event) {
        Ok(json) => SocketFlow::from_send(socket.send(Message::Text(json.into())).await),
        Err(_) => {
            warn!("failed to serialize realtime event");
            SocketFlow::from_send(socket.send(Message::Close(None)).await)
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

fn visible_projects_for(
    db: &crate::db::DbPool,
    auth_user: &crate::db::models::AuthUser,
) -> Option<HashSet<i64>> {
    crate::authz::visible_project_ids(db, &Some(auth_user.clone()))
        .ok()
        .flatten()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EventVisibility {
    Visible,
    Hidden,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EventFlow {
    Forward,
    Drop,
}

fn event_flow(
    db: &crate::db::DbPool,
    auth_user: &crate::db::models::AuthUser,
    message: &RealtimeMessage,
) -> EventFlow {
    match visible_to(db, auth_user, message) {
        EventVisibility::Visible => EventFlow::Forward,
        EventVisibility::Hidden => EventFlow::Drop,
    }
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
            Some(project_id) => match crate::authz::can_view_project(db, auth_user, project_id) {
                Ok(true) => EventVisibility::Visible,
                Ok(false) | Err(_) => EventVisibility::Hidden,
            },
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
            | Self::IssueUnlinked { project_id, .. } => Some(*project_id),
            Self::ResyncRequired | Self::ProjectsReordered => None,
        }
    }
}

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

    fn event_json(message: Message) -> serde_json::Value {
        match message {
            Message::Text(text) => serde_json::from_str(&text).unwrap(),
            other => panic!("expected text event, got {other:?}"),
        }
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
