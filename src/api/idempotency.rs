//! `Idempotency-Key` on the REST create routes (LIF-526).
//!
//! A create that carries the header records the key with its response body
//! inside the write's own transaction (see `db::queries::op_keys`). A retry
//! with the same key and body gets that stored body back, marked with
//! `Idempotent-Replayed: true`, and writes nothing. The same key with a
//! different body is refused with 409.
//!
//! The replay check runs inside the create's transaction, after the same
//! authorization a fresh create needs: a caller who lost access gets that
//! refusal, not the stored body, and a retry racing its own original waits
//! for it and replays it.

use axum::{
    Json,
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::db::queries::op_keys::{self, OpKey, Stored, Written};
use crate::error::LificError;

/// The request header that carries the key.
pub(crate) const KEY_HEADER: &str = "idempotency-key";

/// The response header that marks a replayed result.
pub(crate) const REPLAYED_HEADER: &str = "idempotent-replayed";

/// Listed in `/api/instance`'s `capabilities` by a server that honors
/// [`KEY_HEADER`]. A client resends a lost create only to such a server: an
/// older one ignores the header and would write the create twice.
pub(crate) const CAPABILITY: &str = "idempotency-key";

/// Read the key from `headers` and bind it to the caller and the request.
/// `None` when the request carried no key.
pub(super) fn key(
    identity: &Option<crate::resolve_caller::ResolvedIdentity>,
    headers: &HeaderMap,
    operation: &'static str,
    request: &impl Serialize,
) -> Result<Option<OpKey>, LificError> {
    let key = headers
        .get(KEY_HEADER)
        .map(|value| {
            value
                .to_str()
                .map_err(|_| LificError::BadRequest("Idempotency-Key must be visible ASCII".into()))
        })
        .transpose()?;
    let user_id = identity.as_ref().map(|identity| identity.user.id);
    OpKey::new("Idempotency-Key", user_id, key, operation, request)
}

/// What an earlier call with this key left, if any. Call inside the create's
/// transaction, after its authorization checks and before it writes.
pub(super) fn replay(
    conn: &rusqlite::Connection,
    key: Option<&OpKey>,
) -> Result<Option<Stored>, LificError> {
    op_keys::replay_opt(conn, key)
}

/// Record `key` with `body` as its result, on the create's transaction.
pub(super) fn record(
    conn: &rusqlite::Connection,
    key: Option<&OpKey>,
    body: &impl Serialize,
) -> Result<(), LificError> {
    op_keys::record_opt(conn, key, || {
        serde_json::to_string(body)
            .map_err(|error| LificError::Internal(format!("storing an idempotent result: {error}")))
    })
}

/// A create's response: the new entity, or the stored body of the call that
/// first used this key.
impl<T: Serialize> IntoResponse for Written<T> {
    fn into_response(self) -> Response {
        let replayed = (
            header::HeaderName::from_static(REPLAYED_HEADER),
            HeaderValue::from_static("true"),
        );
        match self {
            Self::Fresh(value) => Json(value).into_response(),
            Self::Replayed(Stored::Result(body)) => (
                [
                    (
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("application/json"),
                    ),
                    replayed,
                ],
                body,
            )
                .into_response(),
            Self::Replayed(Stored::TooLarge) => {
                let mut response = LificError::Conflict(
                    "this Idempotency-Key already created a record, so nothing new was written; \
                     its response was too large to keep, so read the record directly"
                        .into(),
                )
                .into_response();
                response.headers_mut().insert(replayed.0, replayed.1);
                response
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::api::test_helpers::*;
    use axum::Router;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    async fn post_keyed(
        app: &Router,
        uri: &str,
        key: &str,
        body: Value,
    ) -> (StatusCode, bool, Value) {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", key)
                    .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let replayed = response
            .headers()
            .get(super::REPLAYED_HEADER)
            .is_some_and(|value| value == "true");
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, replayed, serde_json::from_slice(&bytes).unwrap())
    }

    async fn body_of(response: axum::response::Response) -> Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn listed(app: &Router, uri: &str) -> usize {
        body_of(json_get(app, uri).await)
            .await
            .as_array()
            .map_or(0, Vec::len)
    }

    #[tokio::test]
    async fn a_retried_issue_create_replays_the_first_body_and_writes_once() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;
        let body = json!({"project_id": project_id, "title": "Once"});

        let (status, replayed, first) = post_keyed(&app, "/api/issues", "k1", body.clone()).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!replayed);
        let (status, replayed, again) = post_keyed(&app, "/api/issues", "k1", body).await;

        assert_eq!(status, StatusCode::OK);
        assert!(replayed, "a replay is marked with Idempotent-Replayed");
        assert_eq!(again, first);
        assert_eq!(
            listed(&app, &format!("/api/issues?project_id={project_id}")).await,
            1
        );
    }

    #[tokio::test]
    async fn the_same_key_with_a_different_body_is_a_conflict_and_writes_nothing() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;
        post_keyed(
            &app,
            "/api/issues",
            "k1",
            json!({"project_id": project_id, "title": "First"}),
        )
        .await;

        let (status, _, error) = post_keyed(
            &app,
            "/api/issues",
            "k1",
            json!({"project_id": project_id, "title": "Second"}),
        )
        .await;

        assert_eq!(status, StatusCode::CONFLICT);
        assert!(
            error["error"]
                .as_str()
                .is_some_and(|message| message.contains("Idempotency-Key")),
            "got: {error}"
        );
        assert_eq!(
            listed(&app, &format!("/api/issues?project_id={project_id}")).await,
            1
        );
    }

    #[tokio::test]
    async fn a_failed_create_leaves_its_key_free_for_a_corrected_retry() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;
        let (status, _, _) = post_keyed(
            &app,
            "/api/issues",
            "k1",
            json!({"project_id": project_id, "title": "Bad", "module_id": 999_999}),
        )
        .await;
        assert!(!status.is_success(), "got {status}");

        let (status, replayed, _) = post_keyed(
            &app,
            "/api/issues",
            "k1",
            json!({"project_id": project_id, "title": "Good"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!replayed);
    }

    #[tokio::test]
    async fn comment_page_and_plan_creates_replay_on_retry() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;
        let issue = body_of(
            json_post(
                &app,
                "/api/issues",
                json!({"project_id": project_id, "title": "Host"}),
            )
            .await,
        )
        .await;
        let issue_id = issue["id"].as_i64().unwrap();

        let creates = [
            (
                format!("/api/issues/{issue_id}/comments"),
                json!({"content": "Hello"}),
                format!("/api/issues/{issue_id}/comments"),
            ),
            (
                "/api/pages".to_string(),
                json!({"project_id": project_id, "title": "Notes"}),
                format!("/api/pages?project_id={project_id}"),
            ),
            (
                "/api/plans".to_string(),
                json!({"project_id": project_id, "title": "Plan", "steps": [{"title": "Go"}]}),
                format!("/api/plans?project_id={project_id}"),
            ),
        ];
        for (uri, body, list) in creates {
            let (status, _, first) = post_keyed(&app, &uri, &uri, body.clone()).await;
            assert_eq!(status, StatusCode::OK, "{uri}: {first}");
            let (status, replayed, again) = post_keyed(&app, &uri, &uri, body).await;
            assert_eq!(status, StatusCode::OK, "{uri}");
            assert!(replayed, "{uri}");
            assert_eq!(again, first, "{uri}");
            assert_eq!(listed(&app, &list).await, 1, "{uri}");
        }
    }

    /// A retry sent while the original still writes waits for it and gets
    /// the replay, not a conflict. The test holds the writer so both requests
    /// are inside their handlers before either can write.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_racing_requests_with_one_key_both_get_the_same_issue() {
        let (db, _admin, _lead, maintainer, _viewer, _outsider, project_id) =
            setup_membership_test();
        let app = app_as_user(db.clone(), &maintainer);
        let body = json!({"project_id": project_id, "title": "Raced"});
        let (held, holding) = std::sync::mpsc::channel();
        let holder = std::thread::spawn({
            let db = db.clone();
            move || {
                let _writer = db.write().unwrap();
                held.send(()).unwrap();
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
        });
        holding.recv().unwrap();
        let requests: Vec<_> = (0..2)
            .map(|_| {
                let (app, body) = (app.clone(), body.clone());
                tokio::spawn(async move { post_keyed(&app, "/api/issues", "race", body).await })
            })
            .collect();
        let mut answers = Vec::new();
        for request in requests {
            answers.push(request.await.unwrap());
        }
        holder.join().unwrap();

        for (status, _, body) in &answers {
            assert_eq!(*status, StatusCode::OK, "{body}");
        }
        assert_eq!(answers[0].2, answers[1].2);
        assert_eq!(
            answers.iter().filter(|(_, replayed, _)| *replayed).count(),
            1
        );
        assert_eq!(
            listed(&app, &format!("/api/issues?project_id={project_id}")).await,
            1
        );
    }

    /// A replay returns what was created, so it needs the permission a fresh
    /// create needs: a member removed since gets the refusal, never the
    /// stored body (for a comment, its full text).
    #[tokio::test]
    async fn a_member_removed_since_the_first_request_gets_no_replay() {
        let (db, _admin, _lead, maintainer, _viewer, _outsider, project_id) =
            setup_membership_test();
        let app = app_as_user(db.clone(), &maintainer);
        let issue = json!({"project_id": project_id, "title": "Host"});
        let (status, _, created) = post_keyed(&app, "/api/issues", "issue", issue.clone()).await;
        assert_eq!(status, StatusCode::OK, "{created}");
        let comments = format!("/api/issues/{}/comments", created["id"]);
        let comment = json!({"content": "Secret notes"});
        let (status, _, _) = post_keyed(&app, &comments, "comment", comment.clone()).await;
        assert_eq!(status, StatusCode::OK);
        crate::db::queries::members::remove_member(&db.write().unwrap(), project_id, maintainer.id)
            .unwrap();

        for (uri, key, body) in [
            ("/api/issues", "issue", issue),
            (comments.as_str(), "comment", comment),
        ] {
            let (status, replayed, answer) = post_keyed(&app, uri, key, body).await;
            assert!(status.is_client_error(), "{uri}: {status} {answer}");
            assert!(!replayed, "{uri}");
            assert!(
                !answer.to_string().contains("Secret notes"),
                "{uri}: {answer}"
            );
        }
    }
}
