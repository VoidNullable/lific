//! LIF-439: the delta-sync read surface — `GET /api/projects/{id}/changes`
//! and `GET /api/projects/{id}/index`.
//!
//! Both are Viewer-gated project-scoped reads, shaped exactly like
//! `api::insights`: authorize, clamp, hand off to the query layer. All of the
//! interesting behaviour (the merged seq ordering, tombstones, and the
//! cursor-before-lists rule the bootstrap's correctness rests on) lives in
//! `db::queries::changes`.
//!
//! This is a web-client surface. There are no MCP tools for it: an agent
//! reads issues through the existing tools and has no replica to reconcile.

use axum::{
    Extension,
    extract::{Json, Path, Query, State},
};

use crate::authz;
use crate::db::queries::changes::{
    clamp_changes_limit, get_index, get_project_index, list_changes,
};
use crate::db::{
    DbPool,
    models::{ChangesPage, IndexSnapshot},
};
use crate::error::LificError;

use super::with_read;

#[derive(Debug, serde::Deserialize)]
pub(super) struct ChangesQuery {
    /// Resume point: only rows with `seq > since` are returned. Absent or 0
    /// means everything, which works but is what `/index` is for.
    pub since: Option<i64>,
    /// Page size. Defaults to 5,000 and is clamped to `1..=50,000` — see
    /// `queries::changes::clamp_changes_limit`.
    pub limit: Option<i64>,
}

/// GET /api/projects/{id}/changes?since=N&limit=M
pub(super) async fn project_changes(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(id): Path<i64>,
    Query(q): Query<ChangesQuery>,
) -> Result<Json<ChangesPage>, LificError> {
    authz::require_read(&db, &identity, id)?;
    let limit = clamp_changes_limit(q.limit);
    let since = q.since.unwrap_or(0);
    with_read(&db, |conn| list_changes(conn, id, since, limit)).map(Json)
}

/// GET /api/projects/{id}/index
pub(super) async fn project_index(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(id): Path<i64>,
) -> Result<Json<IndexSnapshot>, LificError> {
    authz::require_read(&db, &identity, id)?;
    // An anonymous reader gets the project's own watermark rather than the
    // instance-wide counter, so polling a published project cannot be used to
    // watch how often private projects are written to.
    if identity.is_none() {
        return with_read(&db, |conn| {
            let tx = conn.unchecked_transaction()?;
            get_project_index(&tx, id)
        })
        .map(Json);
    }
    with_read(&db, |conn| get_index(conn, id)).map(Json)
}

#[cfg(test)]
mod tests {
    use crate::api::test_helpers::*;
    use axum::http::StatusCode;

    async fn seed_issue(app: &axum::Router, project_id: i64, title: &str) -> serde_json::Value {
        parse_json(
            json_post(
                app,
                "/api/issues",
                serde_json::json!({ "project_id": project_id, "title": title }),
            )
            .await,
        )
        .await
    }

    #[tokio::test]
    async fn index_returns_live_rows_and_a_cursor() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;
        let live = seed_issue(&app, project_id, "live").await;
        let doomed = seed_issue(&app, project_id, "doomed").await;
        json_post(
            &app,
            "/api/pages",
            serde_json::json!({ "project_id": project_id, "title": "Design" }),
        )
        .await;
        json_delete(
            &app,
            &format!("/api/issues/{}", doomed["id"].as_i64().unwrap()),
        )
        .await;

        let resp = json_get(&app, &format!("/api/projects/{project_id}/index")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = parse_json(resp).await;

        let issues = body["issues"].as_array().unwrap();
        assert_eq!(issues.len(), 1, "the tombstoned issue must not appear");
        assert_eq!(issues[0]["id"], live["id"]);
        assert_eq!(issues[0]["kind"], "issue");
        assert_eq!(issues[0]["deleted"], false);
        assert!(issues[0].get("description").is_none());

        let pages = body["pages"].as_array().unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0]["identifier"], "TST-DOC-1");
        assert_eq!(pages[0]["conventions"], true);
        assert_eq!(pages[1]["identifier"], "TST-DOC-2");
        assert_eq!(pages[1]["conventions"], false);
        assert!(pages[1].get("content").is_none());

        let cursor = body["cursor"].as_i64().unwrap();
        assert!(
            issues
                .iter()
                .chain(pages.iter())
                .all(|row| row["seq"].as_i64().unwrap() <= cursor),
            "cursor must be at or above every row it shipped"
        );
    }

    /// The endpoint's core contract: hand it the cursor from a previous
    /// bootstrap and it reports exactly what happened since, deletions
    /// included.
    #[tokio::test]
    async fn changes_backfills_creates_updates_and_deletes_since_a_cursor() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;
        let untouched = seed_issue(&app, project_id, "untouched").await;
        let edited = seed_issue(&app, project_id, "edited").await;
        let doomed = seed_issue(&app, project_id, "doomed").await;

        let cursor = parse_json(json_get(&app, &format!("/api/projects/{project_id}/index")).await)
            .await["cursor"]
            .as_i64()
            .unwrap();

        json_put(
            &app,
            &format!("/api/issues/{}", edited["id"].as_i64().unwrap()),
            serde_json::json!({ "title": "edited twice" }),
        )
        .await;
        json_delete(
            &app,
            &format!("/api/issues/{}", doomed["id"].as_i64().unwrap()),
        )
        .await;

        let body = parse_json(
            json_get(
                &app,
                &format!("/api/projects/{project_id}/changes?since={cursor}"),
            )
            .await,
        )
        .await;
        let changes = body["changes"].as_array().unwrap();
        assert_eq!(changes.len(), 2, "only the two touched rows");
        assert!(!body["has_more"].as_bool().unwrap());

        let seqs: Vec<i64> = changes.iter().map(|c| c["seq"].as_i64().unwrap()).collect();
        let mut sorted = seqs.clone();
        sorted.sort_unstable();
        assert_eq!(seqs, sorted, "ascending by seq");
        assert_eq!(body["cursor"].as_i64().unwrap(), *seqs.last().unwrap());

        let edited_row = changes
            .iter()
            .find(|c| c["id"] == edited["id"])
            .expect("the edited issue");
        assert_eq!(edited_row["deleted"], false);
        assert_eq!(edited_row["title"], "edited twice");

        let tombstone = changes
            .iter()
            .find(|c| c["id"] == doomed["id"])
            .expect("the tombstone");
        assert_eq!(tombstone["deleted"], true);
        assert_eq!(tombstone["kind"], "issue");
        assert!(tombstone.get("title").is_none(), "tombstones carry no body");

        assert!(
            !changes.iter().any(|c| c["id"] == untouched["id"]),
            "an untouched row must not be re-delivered"
        );
    }

    #[tokio::test]
    async fn changes_includes_comments_scoped_to_the_project() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;
        let issue = seed_issue(&app, project_id, "commented").await;
        let issue_id = issue["id"].as_i64().unwrap();
        json_post(
            &app,
            &format!("/api/issues/{issue_id}/comments"),
            serde_json::json!({ "content": "hello there" }),
        )
        .await;

        let body =
            parse_json(json_get(&app, &format!("/api/projects/{project_id}/changes")).await).await;
        let comments: Vec<&serde_json::Value> = body["changes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["kind"] == "comment")
            .collect();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0]["issue_id"], issue_id);
        assert_eq!(comments[0]["username"], "test-admin");
        assert!(
            comments[0].get("content").is_none(),
            "comment bodies load per-detail"
        );
    }

    #[tokio::test]
    async fn changes_paginate_without_gaps_or_duplicates() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;
        for n in 0..5 {
            seed_issue(&app, project_id, &format!("issue {n}")).await;
        }

        let first = parse_json(
            json_get(&app, &format!("/api/projects/{project_id}/changes?limit=2")).await,
        )
        .await;
        assert_eq!(first["changes"].as_array().unwrap().len(), 2);
        assert!(first["has_more"].as_bool().unwrap());
        assert_eq!(
            first["cursor"].as_i64().unwrap(),
            first["changes"][1]["seq"].as_i64().unwrap()
        );

        let mut ids: Vec<String> = first["changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| format!("{}:{}", c["kind"], c["id"]))
            .collect();
        let mut cursor = first["cursor"].as_i64().unwrap();
        loop {
            let next = parse_json(
                json_get(
                    &app,
                    &format!("/api/projects/{project_id}/changes?since={cursor}&limit=2"),
                )
                .await,
            )
            .await;
            ids.extend(
                next["changes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| format!("{}:{}", c["kind"], c["id"])),
            );
            cursor = next["cursor"].as_i64().unwrap();
            if !next["has_more"].as_bool().unwrap() {
                break;
            }
        }

        // Five issues and the conventions page the project starts with.
        assert_eq!(ids.len(), 6, "every row arrived exactly once");
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), 6);
    }

    /// An oversized `limit` must not let a caller dump the instance in one
    /// request, and an absent one must still return a full page.
    #[tokio::test]
    async fn changes_clamps_limit_and_applies_a_default() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;
        for n in 0..3 {
            seed_issue(&app, project_id, &format!("issue {n}")).await;
        }

        for uri in [
            format!("/api/projects/{project_id}/changes?limit=999999"),
            format!("/api/projects/{project_id}/changes"),
            format!("/api/projects/{project_id}/changes?limit=-1"),
        ] {
            let resp = json_get(&app, &uri).await;
            assert_eq!(resp.status(), StatusCode::OK, "{uri}");
            let body = parse_json(resp).await;
            let count = body["changes"].as_array().unwrap().len();
            // Three issues plus the project's conventions page.
            assert!(count <= 4, "{uri} returned {count}");
        }

        // `limit=-1` must floor at 1 rather than becoming SQLite's "no limit".
        let clamped = parse_json(
            json_get(
                &app,
                &format!("/api/projects/{project_id}/changes?limit=-1"),
            )
            .await,
        )
        .await;
        assert_eq!(clamped["changes"].as_array().unwrap().len(), 1);
        assert!(clamped["has_more"].as_bool().unwrap());
    }

    /// A row created after a bootstrap's cursor must reappear in the next
    /// `/changes` call — the guarantee that makes the cursor-before-lists
    /// ordering in `queries::changes::get_index` worth having.
    #[tokio::test]
    async fn a_row_created_after_the_index_cursor_arrives_in_the_next_delta() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;
        seed_issue(&app, project_id, "before").await;

        let cursor = parse_json(json_get(&app, &format!("/api/projects/{project_id}/index")).await)
            .await["cursor"]
            .as_i64()
            .unwrap();
        let after = seed_issue(&app, project_id, "after").await;

        let body = parse_json(
            json_get(
                &app,
                &format!("/api/projects/{project_id}/changes?since={cursor}"),
            )
            .await,
        )
        .await;
        let changes = body["changes"].as_array().unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0]["id"], after["id"]);
        assert!(changes[0]["seq"].as_i64().unwrap() > cursor);
    }
}

/// LIF-197/LIF-201: Viewer-gated exactly like every sibling project-scoped
/// read (`api::insights`, `api::activity`), so a non-member gets 403 and a
/// viewer gets 200.
#[cfg(test)]
mod authz_gating_tests {
    use crate::api::test_helpers::*;
    use axum::http::StatusCode;

    #[tokio::test]
    async fn changes_and_index_deny_non_member_allow_viewer() {
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
        for uri in [
            format!("/api/projects/{project_id}/changes"),
            format!("/api/projects/{project_id}/index"),
        ] {
            assert_eq!(
                json_get(&non_member_app, &uri).await.status(),
                StatusCode::FORBIDDEN,
                "{uri}"
            );
        }

        let viewer_app = app_as_user(db, &viewer);
        let changes =
            parse_json(json_get(&viewer_app, &format!("/api/projects/{project_id}/changes")).await)
                .await;
        assert_eq!(changes["changes"].as_array().unwrap().len(), 1);

        let index =
            parse_json(json_get(&viewer_app, &format!("/api/projects/{project_id}/index")).await)
                .await;
        assert_eq!(index["issues"].as_array().unwrap().len(), 1);
    }
}
