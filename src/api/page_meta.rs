//! `GET /api/projects/{id}/pages/meta`: per-page metadata for the web Pages
//! tab (excerpt, word count, author and last editor, incoming references),
//! which the sync rows do not carry.
//!
//! Authorized exactly like `GET /api/projects/{id}/index`: a Viewer-gated
//! project read that a published project also answers anonymously. The work
//! is in `db::queries::page_meta` and `crate::page_meta`.

use axum::{
    Extension,
    extract::{Json, Path, State},
};

use crate::authz;
use crate::db::DbPool;
use crate::db::queries::page_meta::{PageMetaList, project_page_meta};
use crate::error::LificError;

use super::with_read;

/// GET /api/projects/{id}/pages/meta
pub(super) async fn project_pages_meta(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(id): Path<i64>,
) -> Result<Json<PageMetaList>, LificError> {
    authz::require_read(&db, &identity, id)?;
    with_read(&db, |conn| project_page_meta(conn, id)).map(Json)
}

#[cfg(test)]
mod tests {
    use crate::api::test_helpers::*;
    use axum::http::StatusCode;

    #[tokio::test]
    async fn the_owner_reads_every_live_page_with_its_metadata() {
        let (db, admin, _lead, _regular, _project) = setup_lead_test();
        // `app_as_user` skips the auth middleware that scopes the audit
        // actor, so scope it here the way `api::members` tests do.
        let actor = crate::actor::ActorCtx {
            user_id: Some(admin.id),
            transport: crate::actor::Transport::Web,
        };
        crate::actor::scope(actor, owner_reads_page_meta(db, admin)).await;
    }

    async fn owner_reads_page_meta(db: crate::db::DbPool, admin: crate::db::models::User) {
        let app = app_as_user(db, &admin);
        let (project_id, _) = seed_project(&app).await;
        let target = parse_json(
            json_post(
                &app,
                "/api/pages",
                serde_json::json!({
                    "project_id": project_id,
                    "title": "Storage",
                    "content": "# Storage\n\nLific keeps everything in one **SQLite** file.\n\n```\nnot counted\n```",
                }),
            )
            .await,
        )
        .await;
        let target_id = target["id"].as_i64().unwrap();
        let reference = target["identifier"].as_str().unwrap().to_string();
        let archived = parse_json(
            json_post(
                &app,
                "/api/pages",
                serde_json::json!({
                    "project_id": project_id,
                    "title": "Old",
                    "status": "archived",
                    "content": format!("Superseded by {reference}."),
                }),
            )
            .await,
        )
        .await;
        json_post(
            &app,
            "/api/issues",
            serde_json::json!({
                "project_id": project_id,
                "title": format!("Read {}", reference.to_lowercase()),
            }),
        )
        .await;

        let response = json_get(&app, &format!("/api/projects/{project_id}/pages/meta")).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = parse_json(response).await;
        let pages = body["pages"].as_array().unwrap();
        // The conventions page every new project gets, the target, and the
        // archived page.
        assert_eq!(
            pages.len(),
            3,
            "conventions and archived pages are included"
        );
        let meta = pages.iter().find(|p| p["id"] == target_id).unwrap();
        assert_eq!(
            meta["excerpt"],
            "Lific keeps everything in one SQLite file."
        );
        assert_eq!(meta["words"], 8);
        assert_eq!(
            meta["links"],
            serde_json::json!({ "issues": 1, "pages": 1, "comments": 0 })
        );
        let mut linked_from: Vec<&str> = meta["linked_from"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        linked_from.sort_unstable();
        assert_eq!(
            linked_from,
            vec![
                "TST-1".to_string(),
                archived["identifier"].as_str().unwrap().to_string()
            ]
        );
        for field in ["created_by", "updated_by"] {
            let actor = &meta[field];
            assert_eq!(actor["id"], admin.id, "{field}: {actor}");
            assert_eq!(actor["username"], "admin");
            assert!(actor["display_name"].is_string());
            assert_eq!(actor["is_bot"], false);
            assert!(actor.get("tool_id").is_some());
            assert!(actor.get("email").is_none());
        }
        let old = pages.iter().find(|p| p["id"] == archived["id"]).unwrap();
        assert_eq!(
            old["links"],
            serde_json::json!({ "issues": 0, "pages": 0, "comments": 0 })
        );
        assert_eq!(old["linked_from"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn page_meta_denies_a_non_member_and_allows_a_viewer() {
        let (db, _admin, lead, _maintainer, viewer, non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db.clone(), &lead);
        json_post(
            &lead_app,
            "/api/pages",
            serde_json::json!({ "project_id": project_id, "title": "Gated" }),
        )
        .await;
        let uri = format!("/api/projects/{project_id}/pages/meta");
        let index = format!("/api/projects/{project_id}/index");

        let non_member_app = app_as_user(db.clone(), &non_member);
        assert_eq!(
            json_get(&non_member_app, &uri).await.status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            json_get(&non_member_app, &index).await.status(),
            StatusCode::FORBIDDEN
        );

        let viewer_app = app_as_user(db, &viewer);
        let body = parse_json(json_get(&viewer_app, &uri).await).await;
        assert_eq!(body["pages"].as_array().unwrap().len(), 1);
    }
}
