//! LIF-542: `GET /api/agents/activity`, the Agent activity page's data.
//! Cross-project and filtered to what the caller can see, like
//! `/api/home/overview`; the counting lives in `db::queries::agents`.

use axum::{
    Extension,
    extract::{Json, Query, State},
};

use crate::authz;
use crate::db::DbPool;
use crate::db::queries::agents::{ActivityInput, AgentActivity, Window, activity};
use crate::error::LificError;

use super::with_read;

#[derive(Debug, serde::Deserialize)]
pub(super) struct AgentActivityQuery {
    /// `1h` (default), `24h` or `7d`.
    pub window: Option<String>,
    /// The caller's offset from UTC in minutes, east positive. Defaults to 0.
    pub tz: Option<i32>,
}

/// GET /api/agents/activity?window=...&tz=...
pub(super) async fn agent_activity(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Query(q): Query<AgentActivityQuery>,
) -> Result<Json<AgentActivity>, LificError> {
    let user = super::require_user(&identity)?;
    let window = Window::parse(q.window.as_deref())?;
    let visible = authz::visible_project_ids(&db, &identity)?;
    // LIF-541: reads are buffered; write out the pending ones so the page
    // includes the last few seconds. A failed flush only costs freshness.
    if let Err(error) = db.flush_agent_reads() {
        tracing::warn!(error = %error, "agent read flush before activity failed");
    }
    with_read(&db, |conn| {
        let caller_id = authz::effective_user(conn, &Some(user)).map(|u| u.id);
        activity(
            conn,
            &ActivityInput {
                visible: visible.as_ref(),
                caller_id,
                window,
                tz_minutes: q.tz.unwrap_or(0),
                now: chrono::Utc::now().naive_utc(),
            },
        )
    })
    .map(Json)
}

#[cfg(test)]
mod tests {
    use crate::api::test_helpers::*;
    use crate::db::agent_reads::{AgentRead, now_ts};
    use axum::http::StatusCode;

    #[tokio::test]
    async fn activity_returns_the_requested_window() {
        let app = test_app();
        let body =
            parse_json(json_get(&app, "/api/agents/activity?window=24h&tz=-300").await).await;
        assert_eq!(body["window"], "24h");
        assert_eq!(body["bucket_seconds"], 3600);
        assert_eq!(body["buckets"].as_array().unwrap().len(), 24);
        assert!(body["agents"].as_array().unwrap().is_empty());

        let body = parse_json(json_get(&app, "/api/agents/activity").await).await;
        assert_eq!(body["window"], "1h");
        assert_eq!(body["buckets"].as_array().unwrap().len(), 12);
    }

    #[tokio::test]
    async fn activity_rejects_an_unknown_window() {
        let app = test_app();
        let resp = json_get(&app, "/api/agents/activity?window=30d").await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    /// A bot's write and its buffered read both show, the read without
    /// waiting for the background flush, and a project the caller cannot
    /// see contributes nothing.
    #[tokio::test]
    async fn activity_counts_a_bots_actions_only_for_callers_who_can_see_them() {
        let (db, _admin, lead, _maintainer, _viewer, non_member, project_id) =
            setup_membership_test();
        let bot = {
            let conn = db.write().unwrap();
            let bot = crate::db::queries::users::create_bot_user(
                &conn,
                lead.id,
                "lead-agent",
                "Lead Agent",
                None,
            )
            .unwrap();
            crate::db::queries::members::upsert_member(
                &conn,
                project_id,
                bot.id,
                crate::db::models::Role::Maintainer,
            )
            .unwrap();
            bot
        };
        {
            // The test router has no auth middleware to stamp the actor, so
            // attribute the write to the bot the way the triggers read it.
            let conn = db.write().unwrap();
            conn.execute(
                "UPDATE _actor_state SET user_id = ?1, transport = 'mcp'",
                [bot.id],
            )
            .unwrap();
            crate::db::queries::create_issue(
                &conn,
                &crate::db::models::CreateIssue {
                    project_id,
                    title: "Agent work".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        db.record_agent_read(AgentRead {
            ts: now_ts(),
            user_id: bot.id,
            tool: "get_issue",
            project_id: Some(project_id),
            entity_type: Some("issue"),
            entity_id: Some(1),
            entity_label: Some("MEM-1".into()),
        });

        let lead_app = app_as_user(db.clone(), &lead);
        let body = parse_json(json_get(&lead_app, "/api/agents/activity").await).await;
        assert_eq!(body["totals"]["create"], 1, "body: {body}");
        assert_eq!(body["totals"]["read"], 1, "body: {body}");
        assert_eq!(body["active_count"], 1);
        let agent = &body["agents"][0];
        assert_eq!(agent["user_id"], bot.id);
        assert_eq!(agent["status"], "active");
        assert_eq!(agent["owner"]["user_id"], lead.id);
        assert_eq!(body["recent"][0]["agent"]["user_id"], bot.id);
        assert_eq!(body["recent"].as_array().unwrap().len(), 2);

        let outsider = app_as_user(db, &non_member);
        let body = parse_json(json_get(&outsider, "/api/agents/activity").await).await;
        assert_eq!(body["totals"]["create"], 0);
        assert_eq!(body["totals"]["read"], 0);
        assert!(body["agents"].as_array().unwrap().is_empty());
        assert!(body["recent"].as_array().unwrap().is_empty());
    }
}
