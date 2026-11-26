//! LIF-507: `GET /api/home/overview`, the Home dashboard's aggregates.
//! Cross-project and filtered to what the caller can see, like
//! `/api/issues/attention`; the counting lives in `db::queries::home`.

use axum::{
    Extension,
    extract::{Json, Query, State},
};

use crate::authz;
use crate::db::DbPool;
use crate::db::queries::home::{HomeOverview, OverviewInput, overview};
use crate::error::LificError;

use super::with_read;

#[derive(Debug, serde::Deserialize)]
pub(super) struct HomeQuery {
    /// Summarize what was opened and closed after this ISO 8601 instant.
    pub since: Option<String>,
    /// The caller's offset from UTC in minutes, east positive. Defaults to 0.
    pub tz: Option<i32>,
    /// Narrow every aggregate to one project, for the project overview. A
    /// project the caller cannot see counts as empty, like any other.
    pub project_id: Option<i64>,
}

/// GET /api/home/overview?since=...&tz=...&project_id=...
pub(super) async fn home_overview(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Query(q): Query<HomeQuery>,
) -> Result<Json<HomeOverview>, LificError> {
    let user = super::require_user(&identity)?;
    let mut visible = authz::visible_project_ids(&db, &identity)?;
    if let Some(pid) = q.project_id {
        let allowed = visible.as_ref().is_none_or(|set| set.contains(&pid));
        visible = Some(if allowed {
            std::collections::HashSet::from([pid])
        } else {
            std::collections::HashSet::new()
        });
    }
    with_read(&db, |conn| {
        let user_id = authz::effective_user(conn, &Some(user)).map(|u| u.id);
        overview(
            conn,
            &OverviewInput {
                visible: visible.as_ref(),
                user_id,
                since: q.since.as_deref(),
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
    use axum::http::StatusCode;

    #[tokio::test]
    async fn overview_returns_every_section() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;
        json_post(
            &app,
            "/api/issues",
            serde_json::json!({ "project_id": project_id, "title": "Open", "priority": "urgent" }),
        )
        .await;

        let resp = json_get(
            &app,
            "/api/home/overview?since=2000-01-01T00:00:00Z&tz=-300",
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = parse_json(resp).await;
        assert_eq!(body["open_total"], 1);
        assert_eq!(body["age_buckets"].as_array().unwrap().len(), 5);
        assert_eq!(body["age_buckets"][0]["urgent"], 1);
        assert_eq!(body["age_buckets"][0]["agent_ready"], 1);
        assert_eq!(body["open_trend"].as_array().unwrap().len(), 91);
        assert_eq!(body["projects"][0]["project_id"], project_id);
        assert_eq!(body["since"]["people_opened"], 1);
        assert_eq!(body["my_done"].as_array().unwrap().len(), 84);
    }

    #[tokio::test]
    async fn overview_narrows_to_one_project() {
        let app = test_app();
        let (first, _) = seed_project(&app).await;
        let second = parse_json(
            json_post(
                &app,
                "/api/projects",
                serde_json::json!({ "name": "Second", "identifier": "SEC" }),
            )
            .await,
        )
        .await["id"]
            .as_i64()
            .unwrap();
        for (project_id, title) in [(first, "One"), (second, "Two"), (second, "Three")] {
            json_post(
                &app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id, "title": title }),
            )
            .await;
        }

        let all = parse_json(json_get(&app, "/api/home/overview").await).await;
        assert_eq!(all["open_total"], 3);

        let body =
            parse_json(json_get(&app, &format!("/api/home/overview?project_id={second}")).await)
                .await;
        assert_eq!(body["open_total"], 2);
        let pulses = body["projects"].as_array().unwrap();
        assert_eq!(pulses.len(), 1);
        assert_eq!(pulses[0]["project_id"], second);
        assert_eq!(
            body["open_trend"].as_array().unwrap().last().unwrap()["count"],
            2
        );
    }

    #[tokio::test]
    async fn overview_of_a_hidden_project_is_empty() {
        let (db, _admin, lead, _maintainer, _viewer, non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db.clone(), &lead);
        json_post(
            &lead_app,
            "/api/issues",
            serde_json::json!({ "project_id": project_id, "title": "Members only" }),
        )
        .await;
        let path = format!("/api/home/overview?project_id={project_id}");
        let lead_body = parse_json(json_get(&lead_app, &path).await).await;
        assert_eq!(lead_body["open_total"], 1);

        let outsider = app_as_user(db, &non_member);
        let body = parse_json(json_get(&outsider, &path).await).await;
        assert_eq!(body["open_total"], 0);
        assert!(body["projects"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn overview_rejects_a_malformed_since() {
        let app = test_app();
        let resp = json_get(&app, "/api/home/overview?since=soon").await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn overview_counts_only_projects_the_caller_can_see() {
        let (db, _admin, lead, _maintainer, _viewer, non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db.clone(), &lead);
        json_post(
            &lead_app,
            "/api/issues",
            serde_json::json!({ "project_id": project_id, "title": "Members only" }),
        )
        .await;

        let lead_body = parse_json(json_get(&lead_app, "/api/home/overview").await).await;
        assert_eq!(lead_body["open_total"], 1);

        let outsider = app_as_user(db, &non_member);
        let body = parse_json(json_get(&outsider, "/api/home/overview").await).await;
        assert_eq!(body["open_total"], 0);
        assert!(body["projects"].as_array().unwrap().is_empty());
        assert!(body["since"].is_null());
    }
}
