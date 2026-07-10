//! LIF-240: `GET /api/projects/{id}/insights` — the Insights tab's single
//! read endpoint. All aggregation happens in SQL (`db::queries::insights`);
//! this module is just the Viewer-gated HTTP wrapper, mirroring the shape
//! of `api::activity`'s project-scoped read handlers.

use axum::{
    Extension,
    extract::{Json, Path, Query, State},
};

use crate::authz;
use crate::db::queries::insights::{clamp_weeks, get_insights};
use crate::db::{
    DbPool,
    models::{AuthUser, InsightsPayload, Role},
};
use crate::error::LificError;

use super::with_read;

#[derive(Debug, serde::Deserialize)]
pub(super) struct InsightsQuery {
    /// Trend-line window in weeks. Clamped to 1..=52, defaults to 12 —
    /// see `queries::insights::clamp_weeks`.
    pub weeks: Option<i64>,
}

/// GET /api/projects/{id}/insights?weeks=N
pub(super) async fn project_insights(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Path(id): Path<i64>,
    Query(q): Query<InsightsQuery>,
) -> Result<Json<InsightsPayload>, LificError> {
    authz::require_role(&db, &auth_user, id, Role::Viewer)?;
    let weeks = clamp_weeks(q.weeks);
    with_read(&db, |conn| get_insights(conn, id, weeks)).map(Json)
}

#[cfg(test)]
mod tests {
    use crate::api::test_helpers::*;
    use axum::http::StatusCode;

    #[tokio::test]
    async fn insights_returns_full_shape_for_a_seeded_project() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;

        json_post(
            &app,
            "/api/issues",
            serde_json::json!({ "project_id": project_id, "title": "Seed 1", "priority": "high" }),
        )
        .await;
        let issue = parse_json(
            json_post(
                &app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id, "title": "Seed 2" }),
            )
            .await,
        )
        .await;
        let issue_id = issue["id"].as_i64().unwrap();
        json_put(
            &app,
            &format!("/api/issues/{issue_id}"),
            serde_json::json!({ "status": "done" }),
        )
        .await;

        let resp = json_get(
            &app,
            &format!("/api/projects/{project_id}/insights?weeks=6"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = parse_json(resp).await;

        assert_eq!(body["weeks"], 6);
        assert_eq!(body["created_per_week"].as_array().unwrap().len(), 6);
        assert_eq!(body["closed_per_week"].as_array().unwrap().len(), 6);
        assert_eq!(body["status_counts"]["total"], 2);
        assert_eq!(body["status_counts"]["done"], 1);
        assert_eq!(body["priority_counts"]["total"], 2);
        assert_eq!(body["priority_counts"]["high"], 1);
        assert!(!body["module_counts"].as_array().unwrap().is_empty());
        assert!(body["top_actors"].is_array());

        // Trend-line dense buckets: every point has both keys.
        let first_point = &body["created_per_week"][0];
        assert!(first_point["week_start"].is_string());
        assert!(first_point["count"].is_i64());
    }

    #[tokio::test]
    async fn insights_defaults_weeks_to_twelve_and_clamps_out_of_range() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;

        let resp = json_get(&app, &format!("/api/projects/{project_id}/insights")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = parse_json(resp).await;
        assert_eq!(body["weeks"], 12);
        assert_eq!(body["created_per_week"].as_array().unwrap().len(), 12);

        let resp = json_get(
            &app,
            &format!("/api/projects/{project_id}/insights?weeks=999"),
        )
        .await;
        let body = parse_json(resp).await;
        assert_eq!(body["weeks"], 52);
    }
}

/// LIF-197/LIF-201: Viewer-gated like every sibling read handler in
/// `api::activity`; a non-member with the flag on must get 403, a viewer
/// must get 200.
#[cfg(test)]
mod authz_gating_tests {
    use crate::api::test_helpers::*;
    use axum::http::StatusCode;

    #[tokio::test]
    async fn insights_denies_non_member_allows_viewer() {
        let (db, _admin, lead, _maintainer, viewer, non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db.clone(), &lead);
        json_post(
            &lead_app,
            "/api/issues",
            serde_json::json!({ "project_id": project_id, "title": "Gated" }),
        )
        .await;

        let non_member_app = app_as_user(db.clone(), &non_member);
        assert_eq!(
            json_get(
                &non_member_app,
                &format!("/api/projects/{project_id}/insights")
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );

        let viewer_app = app_as_user(db, &viewer);
        let resp = json_get(&viewer_app, &format!("/api/projects/{project_id}/insights")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = parse_json(resp).await;
        assert_eq!(body["status_counts"]["total"], 1);
    }
}
