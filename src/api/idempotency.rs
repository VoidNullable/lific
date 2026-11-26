//! `Idempotency-Key` on the REST create routes (LIF-526).
//!
//! A create that carries the header records the key with its response body
//! inside the write's own transaction (see `db::queries::op_keys`). A retry
//! with the same key and body gets that stored body back, marked with
//! `Idempotent-Replayed: true`, and writes nothing. The same key with a
//! different body is refused with 409.

use axum::{
    Json,
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::db::DbPool;
use crate::db::queries::op_keys::{self, OpKey};
use crate::error::LificError;

/// The request header that carries the key.
pub(crate) const KEY_HEADER: &str = "idempotency-key";

/// The response header that marks a replayed result.
pub(crate) const REPLAYED_HEADER: &str = "idempotent-replayed";

/// What a create does with its key: write (recording the key when there is
/// one), or answer with the body an earlier call with this key committed.
pub(super) enum Bound {
    Write(Option<OpKey>),
    Replay(String),
}

/// Read the key from `headers`, bind it to the caller and the request, and
/// look for an earlier call that used it.
pub(super) fn bind(
    db: &DbPool,
    identity: &Option<crate::resolve_caller::ResolvedIdentity>,
    headers: &HeaderMap,
    operation: &'static str,
    request: &impl Serialize,
) -> Result<Bound, LificError> {
    let key = headers
        .get(KEY_HEADER)
        .map(|value| {
            value
                .to_str()
                .map_err(|_| LificError::BadRequest("Idempotency-Key must be visible ASCII".into()))
        })
        .transpose()?;
    let user_id = identity.as_ref().map(|identity| identity.user.id);
    let Some(key) = OpKey::new("Idempotency-Key", user_id, key, operation, request)? else {
        return Ok(Bound::Write(None));
    };
    let conn = db.read()?;
    Ok(match op_keys::replay(&conn, &key)? {
        Some(body) => Bound::Replay(body),
        None => Bound::Write(Some(key)),
    })
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
pub(super) enum Created<T> {
    Fresh(T),
    Replayed(String),
}

impl<T: Serialize> IntoResponse for Created<T> {
    fn into_response(self) -> Response {
        match self {
            Self::Fresh(value) => Json(value).into_response(),
            Self::Replayed(body) => (
                [
                    (
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("application/json"),
                    ),
                    (
                        header::HeaderName::from_static(REPLAYED_HEADER),
                        HeaderValue::from_static("true"),
                    ),
                ],
                body,
            )
                .into_response(),
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
}
