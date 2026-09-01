//! LIF-5: `POST /api/git-hook` — close the issues a batch of commit messages
//! says they close.
//!
//! The consumer is CI: a workflow that has just seen a push hands over the
//! messages of the commits in it and gets back what was closed and what was
//! not. The grammar is [`crate::issue_refs`], shared verbatim with `lific
//! git-hook` so a hook and a pipeline agree on what "closes LIF-42" means.
//!
//! ## Nothing here is an existence oracle
//!
//! A commit message is written by whoever pushed, and the identifiers in it
//! are guesses as far as this endpoint is concerned. So an identifier that
//! names an issue in a project the caller cannot see is reported *identically*
//! to one that names nothing at all: `{"reason": "not found"}`. `forbidden` is
//! reserved for a project the caller can already read but may not write, where
//! the existence of the issue is not news to them.
//!
//! ## Closing goes through the ordinary update path
//!
//! The write below is the body of `api::issues::update_issue`, not a bespoke
//! `UPDATE`: `queries::update_issue` inside `db.transaction`, with the
//! Maintainer role re-read on the writing connection, attachment links
//! reconciled, and the realtime event stamped with the row's new `seq`. That
//! is what makes a hook-driven close indistinguishable from a human one in
//! `status_transitions`, the audit log, and every connected client.

use axum::{
    Extension,
    extract::{Json, State},
};
use serde::Deserialize;
use serde_json::json;

use crate::authz;
use crate::db::{DbPool, models::*};
use crate::error::LificError;
use crate::issue_refs;
use crate::realtime::{RealtimeEvent, RealtimeHub};

use super::with_read;

/// The identifier names nothing the caller can see. Deliberately the same
/// answer for "no such issue" and "an issue in a project you cannot read".
const NOT_FOUND: &str = "not found";
/// The caller can read the project but is not a Maintainer of it.
const FORBIDDEN: &str = "forbidden";
/// The issue is already `done` or `cancelled`, so there is nothing to do.
const ALREADY_CLOSED: &str = "already closed";

#[derive(Debug, Deserialize)]
pub(super) struct GitHookRequest {
    /// The commit messages to scan, in whatever order the caller has them.
    #[serde(default)]
    messages: Vec<String>,
    /// Report what would happen and write nothing.
    #[serde(default)]
    dry_run: bool,
}

/// One identifier that was not closed, and why.
fn skipped(identifier: &str, reason: &str) -> serde_json::Value {
    json!({ "identifier": identifier, "reason": reason })
}

/// The response document. `dry_run` renames the acted-upon list to
/// `would_close`, so a caller that forgot which mode it asked for cannot
/// mistake a rehearsal for a write.
fn document(
    dry_run: bool,
    acted: Vec<String>,
    skipped: Vec<serde_json::Value>,
) -> serde_json::Value {
    let key = if dry_run { "would_close" } else { "closed" };
    json!({ key: acted, "skipped": skipped })
}

/// What should happen to one referenced identifier.
enum Verdict {
    Close(i64),
    Skip(&'static str),
}

/// Decide the fate of `identifier` without writing anything.
fn verdict(
    db: &DbPool,
    identity: &Option<crate::resolve_caller::ResolvedIdentity>,
    caller: &crate::resolve_caller::ResolvedIdentity,
    identifier: &str,
) -> Result<Verdict, LificError> {
    let issue = match with_read(db, |conn| {
        let id = crate::db::queries::resolve_identifier(conn, identifier)?;
        crate::db::queries::get_issue(conn, id)
    }) {
        Ok(issue) => issue,
        // A malformed identifier cannot come out of `issue_refs`, but a
        // NotFound absolutely can, and it is the caller's problem to see as a
        // skip rather than a failed request.
        Err(LificError::NotFound(_) | LificError::BadRequest(_)) => {
            return Ok(Verdict::Skip(NOT_FOUND));
        }
        Err(other) => return Err(other),
    };

    if !authz::can_view_project(db, caller, issue.project_id)? {
        return Ok(Verdict::Skip(NOT_FOUND));
    }
    if authz::require_role(db, identity, issue.project_id, Role::Maintainer).is_err() {
        return Ok(Verdict::Skip(FORBIDDEN));
    }
    if issue.status.is_closed() {
        return Ok(Verdict::Skip(ALREADY_CLOSED));
    }
    Ok(Verdict::Close(issue.id))
}

/// Mark one issue `done`, exactly the way `PUT /api/issues/{id}` does.
fn close_issue(
    db: &DbPool,
    realtime: &RealtimeHub,
    identity: &Option<crate::resolve_caller::ResolvedIdentity>,
    user: &AuthUser,
    id: i64,
) -> Result<(), LificError> {
    let issue = db.transaction(|conn| {
        let issue = crate::db::queries::update_issue(
            conn,
            id,
            &UpdateIssue {
                status: Some(Status::Done),
                ..Default::default()
            },
        )?;
        // The gate ran on a read connection before this write began; re-run it
        // here against the issue's project as it stands inside the
        // transaction, so a revocation cannot slip in between.
        authz::require_role_conn(conn, identity, issue.project_id, Role::Maintainer)?;
        super::attachments::sync_links_scoped(
            conn,
            AttachmentEntity::Issue,
            issue.id,
            &issue.description,
            user,
            Some(issue.project_id),
        )?;
        Ok(issue)
    })?;
    realtime.send_with_seq(
        RealtimeEvent::IssueUpdated {
            project_id: issue.project_id,
            issue_id: issue.id,
        },
        issue.seq,
    );
    Ok(())
}

pub(super) async fn git_hook(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Json(input): Json<GitHookRequest>,
) -> Result<Json<serde_json::Value>, LificError> {
    let user = super::require_user(&identity)?;
    let caller = identity
        .clone()
        .ok_or_else(|| LificError::Forbidden("authentication required".into()))?;

    // Deliberate ceilings, named in the refusal (the PR #40 convention): a CI
    // push has no business carrying more, and each reference below costs
    // resolution plus authorization queries.
    const MAX_MESSAGES: usize = 500;
    const MAX_REFERENCES: usize = 500;
    if input.messages.len() > MAX_MESSAGES {
        return Err(LificError::BadRequest(format!(
            "too many messages in one request (max {MAX_MESSAGES})"
        )));
    }
    let references = issue_refs::closing_references_in(&input.messages);
    if references.len() > MAX_REFERENCES {
        return Err(LificError::BadRequest(format!(
            "too many issue references in one request (max {MAX_REFERENCES})"
        )));
    }

    let mut acted = Vec::new();
    let mut skips = Vec::new();

    for identifier in references {
        match verdict(&db, &identity, &caller, &identifier)? {
            Verdict::Skip(reason) => skips.push(skipped(&identifier, reason)),
            Verdict::Close(id) => {
                if !input.dry_run {
                    close_issue(&db, &realtime, &identity, &user, id)?;
                }
                acted.push(identifier);
            }
        }
    }

    Ok(Json(document(input.dry_run, acted, skips)))
}

#[cfg(test)]
mod tests {
    use crate::api::test_helpers::*;
    use crate::db::DbPool;
    use crate::db::models::*;
    use crate::db::queries;
    use axum::http::StatusCode;
    use http_body_util::BodyExt;

    /// Raw response bytes, so the no-oracle test can compare two answers byte
    /// for byte rather than through a lossy `serde_json::Value` round trip.
    async fn raw_body(resp: axum::response::Response) -> Vec<u8> {
        resp.into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec()
    }

    async fn hook(app: &axum::Router, body: serde_json::Value) -> serde_json::Value {
        let resp = json_post(app, "/api/git-hook", body).await;
        assert_eq!(resp.status(), StatusCode::OK);
        parse_json(resp).await
    }

    fn status_transition_count(db: &DbPool, issue_id: i64) -> i64 {
        let conn = db.read().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM status_transitions WHERE issue_id = ?1",
            [issue_id],
            |row| row.get(0),
        )
        .unwrap()
    }

    /// Create one issue through the API and hand back its numeric id.
    async fn seed_issue(app: &axum::Router, project_id: i64, title: &str) -> i64 {
        let resp = json_post(
            app,
            "/api/issues",
            serde_json::json!({"project_id": project_id, "title": title}),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        parse_json(resp).await["id"].as_i64().unwrap()
    }

    #[tokio::test]
    async fn a_closing_message_closes_the_issue_and_records_the_transition() {
        let test = test_app_with_realtime();
        let (project_id, _) = seed_project(&test.app).await;
        let id = seed_issue(&test.app, project_id, "Gets closed").await;

        let body = hook(
            &test.app,
            serde_json::json!({"messages": ["Do the thing\n\nCloses TST-1"]}),
        )
        .await;

        assert_eq!(body["closed"], serde_json::json!(["TST-1"]));
        assert_eq!(body["skipped"], serde_json::json!([]));

        let issue = parse_json(json_get(&test.app, &format!("/api/issues/{id}")).await).await;
        assert_eq!(issue["status"], "done");
    }

    #[tokio::test]
    async fn closing_through_the_hook_writes_a_status_transition_row() {
        let db = crate::db::open_memory().expect("test db");
        let user = queries::users::create_user(
            &db.write().unwrap(),
            &CreateUser {
                username: "hook-admin".into(),
                email: "hook-admin@test.local".into(),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: true,
                is_bot: false,
            },
        )
        .unwrap();
        let app = app_as_user(db.clone(), &user);
        let (project_id, _) = seed_project(&app).await;
        let id = seed_issue(&app, project_id, "Transitions").await;

        let before = status_transition_count(&db, id);
        hook(&app, serde_json::json!({"messages": ["fixes TST-1"]})).await;
        assert_eq!(
            status_transition_count(&db, id),
            before + 1,
            "the close must travel the ordinary update path, which is what feeds status_transitions"
        );
    }

    #[tokio::test]
    async fn dry_run_reports_what_would_close_and_writes_nothing() {
        let test = test_app_with_realtime();
        let (project_id, _) = seed_project(&test.app).await;
        let id = seed_issue(&test.app, project_id, "Rehearsal").await;

        let body = hook(
            &test.app,
            serde_json::json!({"messages": ["closes TST-1"], "dry_run": true}),
        )
        .await;

        assert_eq!(body["would_close"], serde_json::json!(["TST-1"]));
        assert!(
            body.get("closed").is_none(),
            "a rehearsal must not claim to have closed anything: {body}"
        );
        let issue = parse_json(json_get(&test.app, &format!("/api/issues/{id}")).await).await;
        assert_eq!(issue["status"], "backlog", "dry run wrote to the database");
    }

    #[tokio::test]
    async fn an_unknown_identifier_is_skipped_as_not_found() {
        let app = test_app();
        seed_project(&app).await;

        let body = hook(&app, serde_json::json!({"messages": ["closes TST-99"]})).await;

        assert_eq!(body["closed"], serde_json::json!([]));
        assert_eq!(
            body["skipped"],
            serde_json::json!([{"identifier": "TST-99", "reason": "not found"}])
        );
    }

    #[tokio::test]
    async fn an_already_closed_issue_is_skipped_rather_than_reclosed() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;
        let id = seed_issue(&app, project_id, "Done already").await;
        json_put(
            &app,
            &format!("/api/issues/{id}"),
            serde_json::json!({"status": "done"}),
        )
        .await;

        let body = hook(&app, serde_json::json!({"messages": ["closes TST-1"]})).await;
        assert_eq!(
            body["skipped"],
            serde_json::json!([{"identifier": "TST-1", "reason": "already closed"}])
        );

        // `cancelled` is the other terminal state and reads the same way.
        let other = seed_issue(&app, project_id, "Cancelled").await;
        json_put(
            &app,
            &format!("/api/issues/{other}"),
            serde_json::json!({"status": "cancelled"}),
        )
        .await;
        let body = hook(&app, serde_json::json!({"messages": ["fixes TST-2"]})).await;
        assert_eq!(
            body["skipped"],
            serde_json::json!([{"identifier": "TST-2", "reason": "already closed"}])
        );
    }

    #[tokio::test]
    async fn a_visible_project_the_caller_cannot_write_is_forbidden() {
        let (db, _admin, lead, _maintainer, viewer, _non_member, project_id) =
            setup_membership_test();
        let lead_app = app_as_user(db.clone(), &lead);
        let id = seed_issue(&lead_app, project_id, "Viewer may look only").await;

        let viewer_app = app_as_user(db.clone(), &viewer);
        let body = hook(
            &viewer_app,
            serde_json::json!({"messages": ["closes MEM-1"]}),
        )
        .await;

        assert_eq!(body["closed"], serde_json::json!([]));
        assert_eq!(
            body["skipped"],
            serde_json::json!([{"identifier": "MEM-1", "reason": "forbidden"}]),
            "a viewer can already see the issue, so hiding it behind 'not found' would only confuse"
        );
        let issue = parse_json(json_get(&lead_app, &format!("/api/issues/{id}")).await).await;
        assert_eq!(issue["status"], "backlog");
    }

    /// The no-oracle property, proven the only way it can be: two instances
    /// that differ *only* in whether the referenced issue exists, asked the
    /// identical question by an identical outsider, must answer with identical
    /// bytes. Anything less and the endpoint tells a stranger which
    /// identifiers are real.
    #[tokio::test]
    async fn an_invisible_issue_and_a_nonexistent_one_answer_byte_for_byte_alike() {
        async fn answer(seed_the_issue: bool) -> Vec<u8> {
            let (db, _admin, lead, _maintainer, _viewer, non_member, project_id) =
                setup_membership_test();
            if seed_the_issue {
                let lead_app = app_as_user(db.clone(), &lead);
                seed_issue(&lead_app, project_id, "Invisible").await;
            }
            let outsider = app_as_user(db, &non_member);
            let resp = json_post(
                &outsider,
                "/api/git-hook",
                serde_json::json!({"messages": ["closes MEM-1"]}),
            )
            .await;
            assert_eq!(resp.status(), StatusCode::OK);
            raw_body(resp).await
        }

        let real_but_invisible = answer(true).await;
        let never_existed = answer(false).await;

        assert_eq!(
            String::from_utf8_lossy(&real_but_invisible),
            String::from_utf8_lossy(&never_existed),
            "an issue the caller cannot see must be indistinguishable from one that does not exist"
        );
        let body: serde_json::Value = serde_json::from_slice(&real_but_invisible).unwrap();
        assert_eq!(
            body["skipped"],
            serde_json::json!([{"identifier": "MEM-1", "reason": "not found"}])
        );
    }

    #[tokio::test]
    async fn references_are_deduplicated_across_every_message_in_the_batch() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;
        seed_issue(&app, project_id, "One").await;
        seed_issue(&app, project_id, "Two").await;

        let body = hook(
            &app,
            serde_json::json!({"messages": [
                "First\n\nCloses TST-1",
                "Second\n\nfixes TST-2 and closes tst-1",
            ]}),
        )
        .await;

        assert_eq!(body["closed"], serde_json::json!(["TST-1", "TST-2"]));
    }

    #[tokio::test]
    async fn a_message_with_no_closing_keyword_changes_nothing() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;
        let id = seed_issue(&app, project_id, "Merely mentioned").await;

        let body = hook(
            &app,
            serde_json::json!({"messages": ["Refactor the parser (see TST-1)"]}),
        )
        .await;

        assert_eq!(body["closed"], serde_json::json!([]));
        assert_eq!(body["skipped"], serde_json::json!([]));
        let issue = parse_json(json_get(&app, &format!("/api/issues/{id}")).await).await;
        assert_eq!(issue["status"], "backlog");
    }

    #[tokio::test]
    async fn an_empty_batch_is_an_empty_answer() {
        let app = test_app();
        let body = hook(&app, serde_json::json!({"messages": []})).await;
        assert_eq!(
            body,
            serde_json::json!({"closed": [], "skipped": []}),
            "a push with nothing to close is a success, not an error"
        );
    }
}
