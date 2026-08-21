use axum::{
    Extension,
    extract::{Json, Path, Query, State},
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::authz;
use crate::db::{DbPool, models::*};
use crate::error::LificError;
use crate::realtime::{RealtimeEvent, RealtimeHub};

use super::{
    filter_visible, require_project_delete, require_project_lead, require_user, with_read,
    with_write,
};

pub(super) async fn list_projects(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
) -> Result<Json<Vec<Project>>, LificError> {
    // Cross-project list (LIF-197 scope item 2): filter, don't deny.
    let visible = authz::visible_project_ids(&db, &identity)?;
    let projects = with_read(&db, crate::db::queries::list_projects)?;
    Ok(Json(filter_visible(projects, &visible, |p| Some(p.id))))
}

pub(super) async fn get_project(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(id): Path<i64>,
) -> Result<Json<Project>, LificError> {
    let project = with_read(&db, |conn| crate::db::queries::get_project(conn, id))?;
    authz::require_role(&db, &identity, project.id, Role::Viewer)?;
    Ok(Json(project))
}

/// POST /api/projects.
///
/// `create_project` upserts a `lead` membership for whoever `lead_user_id`
/// names (LIF-195), so naming somebody else is a lasting access grant, exactly
/// as `PUT /api/projects/{id}` is. It is a grant on a project that is empty at
/// the moment it is made, but the project does not stay empty, and the
/// membership does not expire, so it is gated on the same terms as every other
/// grant: a browser session authenticated in the last 15 minutes.
///
/// Naming *yourself*, or naming nobody (the default), grants nothing that the
/// act of creating the project did not already grant, so neither needs
/// recency. That keeps `lific connect`-style API-key automation creating its
/// own projects exactly as before.
///
/// "Yourself" means the **effective** user, not `identity.user`. A bot's
/// permissions are its owner's ([`crate::authz::effective_user`]), so a
/// connected tool creating a project leads it as its owner, which is what the
/// project would resolve to anyway. That also closes the alternative reading:
/// a bot cannot use "my owner is not literally me" to smuggle in a grant,
/// because naming the owner *is* naming itself here.
pub(super) async fn create_project(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    headers: axum::http::HeaderMap,
    Json(mut input): Json<CreateProject>,
) -> Result<Json<Project>, LificError> {
    let caller = super::require_user(&identity)?;
    // Only parsed when the body asks for a lead that might not be the caller;
    // resolving "might not be" needs the effective user, which needs the
    // transaction, so the token is captured here and judged there.
    let session_token = crate::auth::recent_session_token(&headers).ok();

    let project = db.transaction(|tx| {
        let fresh = crate::auth::fresh_caller(tx, caller.id)?;
        let effective =
            crate::authz::effective_user(tx, &Some(crate::auth::fresh_auth_user(&fresh)))
                .ok_or_else(|| LificError::Forbidden("authentication required".into()))?;

        match input.lead_user_id {
            // LIF-102 fix #1: no lead supplied means the creator leads it.
            // Without this, `require_project_lead` rejects everyone but admins
            // and the project is unowned.
            None => input.lead_user_id = Some(effective.id),
            // Naming yourself grants nothing new.
            Some(id) if id == effective.id => {}
            // Naming anybody else does, so it needs a recent human sign-in.
            Some(_) => {
                let token = session_token.as_deref().ok_or_else(|| {
                    LificError::Forbidden("recent authentication required".into())
                })?;
                let session_user = crate::auth::revalidate_recent_session(tx, token, caller.id)?;
                // A session belongs to a human, so the effective user is that
                // human; assert it rather than assume it.
                if session_user.id != effective.id {
                    return Err(LificError::Forbidden(
                        "recent authentication required".into(),
                    ));
                }
            }
        }

        crate::db::queries::create_project(tx, &input)
    })?;
    realtime.send(RealtimeEvent::ProjectCreated {
        project_id: project.id,
    });
    Ok(Json(project))
}

pub(super) async fn update_project(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Path(id): Path<i64>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    headers: axum::http::HeaderMap,
    Json(input): Json<UpdateProject>,
) -> Result<Json<Project>, LificError> {
    require_project_lead(&db, &identity, id)?;

    // Naming a lead is a membership grant in disguise: `update_project` upserts
    // a `lead` row for whoever is named (LIF-195), which is the same access
    // expansion `POST /api/projects/{id}/members` performs and must carry the
    // same rule. Renames, descriptions, emoji and *clearing* the lead are not
    // expansions and stay ungated.
    let grants_lead = matches!(input.lead_user_id, Some(Some(_)));
    let recent = if grants_lead {
        let granter = super::require_user(&identity)?;
        Some((crate::auth::recent_session_token(&headers)?, granter.id))
    } else {
        None
    };

    let project = db.transaction(|tx| {
        // When this grants a lead membership the gate re-runs against the
        // freshly read session user, so a lead revoked since the request
        // arrived cannot hand the role to anyone.
        let gate_identity = match &recent {
            Some((token, granter_id)) => {
                let fresh = crate::auth::revalidate_recent_session(tx, token, *granter_id)?;
                Some(crate::auth::fresh_identity(
                    &fresh,
                    crate::actor::Transport::Web,
                ))
            }
            // Not a grant (a rename, or clearing the lead). No recency, but
            // still not the middleware's snapshot: the caller is re-read here
            // so a demoted admin cannot edit on the strength of a stale
            // `is_admin`.
            None => {
                let caller = super::require_user(&identity)?;
                let fresh = crate::auth::fresh_caller(tx, caller.id)?;
                Some(crate::auth::fresh_identity(
                    &fresh,
                    crate::actor::Transport::Web,
                ))
            }
        };
        crate::authz::require_role_conn(tx, &gate_identity, id, Role::Lead)?;
        crate::db::queries::update_project(tx, id, &input)
    })?;
    realtime.send(RealtimeEvent::ProjectUpdated {
        project_id: project.id,
    });
    Ok(Json(project))
}

/// PUT /api/projects/reorder — persist the sidebar order (LIF-233). Takes the
/// full id list top-to-bottom; the server reindexes `sort_order`. Gated only on
/// being authenticated (order is instance-wide, not a privileged project edit),
/// so any logged-in user can rearrange — unlike `update_project`, which is
/// lead/admin-only.
pub(super) async fn reorder_projects(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Json(input): Json<ReorderProjects>,
) -> Result<Json<Vec<Project>>, LificError> {
    require_user(&identity)?;
    let projects = with_write(&db, |conn| {
        crate::db::queries::reorder_projects(conn, &input.ids)
    })?;
    realtime.send(RealtimeEvent::ProjectsReordered);
    Ok(Json(projects))
}

pub(super) async fn delete_project_handler(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Path(id): Path<i64>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
) -> Result<Json<serde_json::Value>, LificError> {
    // Preflight on a read connection so an unauthorized caller never reaches
    // the writer; re-run authoritatively inside the transaction below.
    require_project_delete(&db, &identity, id)?;
    let caller = super::require_user(&identity)?;
    let (project, audience) = db.transaction(|tx| {
        // Deleting a project destroys everything in it, so the decision is
        // made from state read here rather than from the snapshot the
        // middleware attached before the request was routed.
        let fresh = crate::auth::fresh_caller(tx, caller.id)?;
        let fresh_identity = Some(crate::auth::fresh_identity(
            &fresh,
            crate::actor::Transport::Web,
        ));
        crate::authz::require_project_delete_role_conn(tx, &fresh_identity, id)?;
        crate::db::queries::delete_project_with_audience(tx, id)
    })?;
    let event = RealtimeEvent::ProjectDeleted {
        project_id: project.id,
    };
    match audience {
        Some(user_ids) => realtime.send_to_users(event, user_ids),
        None => realtime.send(event),
    }
    Ok(Json(serde_json::json!({"deleted": true})))
}

/// Per-status issue counts + total for the topbar (LIF-161). Separate from
/// the list endpoint because that one is limit-capped — counting its rows
/// client-side silently undercounts past the cap.
pub(super) async fn issue_counts(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(project_id): Path<i64>,
) -> Result<Json<IssueStatusCounts>, LificError> {
    authz::require_role(&db, &identity, project_id, Role::Viewer)?;
    with_read(&db, |conn| {
        crate::db::queries::count_issues_by_status(conn, project_id)
    })
    .map(Json)
}

#[derive(serde::Deserialize)]
pub(super) struct BoardQuery {
    #[serde(default = "default_group_by")]
    group_by: String,
}

fn default_group_by() -> String {
    "status".to_string()
}

pub(super) async fn get_board(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(project_id): Path<i64>,
    Query(q): Query<BoardQuery>,
) -> Result<Json<serde_json::Value>, LificError> {
    authz::require_role(&db, &identity, project_id, Role::Viewer)?;
    let issues = with_read(&db, |conn| {
        crate::db::queries::list_issues(
            conn,
            &ListIssuesQuery {
                project_id: Some(project_id),
                limit: Some(500),
                ..Default::default()
            },
        )
    })?;

    let module_names: std::collections::HashMap<i64, String> = if q.group_by == "module" {
        with_read(&db, |conn| {
            crate::db::queries::list_modules(conn, project_id)
        })
        .unwrap_or_default()
        .into_iter()
        .map(|m| (m.id, m.name))
        .collect()
    } else {
        std::collections::HashMap::new()
    };

    let mut board: std::collections::BTreeMap<String, Vec<&Issue>> =
        std::collections::BTreeMap::new();
    for issue in &issues {
        let key = match q.group_by.as_str() {
            "priority" => issue.priority.to_string(),
            "module" => issue
                .module_id
                .and_then(|m| module_names.get(&m).cloned())
                .unwrap_or("unassigned".into()),
            _ => issue.status.to_string(),
        };
        board.entry(key).or_default().push(issue);
    }

    Ok(Json(serde_json::json!(board)))
}

// ── GitHub import (LIF-264, web surface) ─────────────────────

/// Request body for `POST /api/projects/{id}/import/github`.
///
/// The web Import panel posts repo + token + mapping here. `dry_run` drives the
/// preview step (counts only, no writes). Only GitHub is exposed on the web;
/// Linear/Jira are CLI-only per LIF-265.
#[derive(serde::Deserialize)]
pub(super) struct GithubImportRequest {
    /// Source repo as `owner/name`.
    repo: String,
    /// Optional GitHub token. Public repos work without one (subject to the
    /// anon rate limit).
    #[serde(default)]
    token: Option<String>,
    /// open / closed / all. Defaults to all.
    #[serde(default = "default_import_state")]
    state: String,
    /// Lific status for open issues.
    #[serde(default = "default_map_open")]
    map_open: String,
    /// Lific status for closed issues.
    #[serde(default = "default_map_closed")]
    map_closed: String,
    /// Preview only — count, write nothing.
    #[serde(default)]
    dry_run: bool,
}

// Keep a small global ceiling while allowing unrelated projects to import in
// parallel. The per-project gate below is the important isolation boundary:
// one expensive import cannot make another import for the same project race
// its writes or consume unbounded resources.
const GITHUB_IMPORT_GLOBAL_LIMIT: usize = 4;
static GITHUB_IMPORT_SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
static GITHUB_IMPORT_PROJECT_SLOTS: OnceLock<Mutex<HashMap<i64, Weak<Semaphore>>>> =
    OnceLock::new();

fn github_import_permits(
    project_id: i64,
) -> Result<(OwnedSemaphorePermit, OwnedSemaphorePermit), LificError> {
    let project_slot = {
        let slots = GITHUB_IMPORT_PROJECT_SLOTS.get_or_init(|| Mutex::new(HashMap::new()));
        let mut slots = slots
            .lock()
            .map_err(|_| LificError::Internal("GitHub import gate poisoned".into()))?;
        slots.retain(|_, slot| slot.strong_count() > 0);
        match slots.entry(project_id) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                if let Some(slot) = entry.get().upgrade() {
                    slot
                } else {
                    let slot = Arc::new(Semaphore::new(1));
                    entry.insert(Arc::downgrade(&slot));
                    slot
                }
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                let slot = Arc::new(Semaphore::new(1));
                entry.insert(Arc::downgrade(&slot));
                slot
            }
        }
    };
    let project_permit = project_slot.try_acquire_owned().map_err(|_| {
        LificError::Conflict("a GitHub import is already running for this project".into())
    })?;
    let global_permit = GITHUB_IMPORT_SLOTS
        .get_or_init(|| Arc::new(Semaphore::new(GITHUB_IMPORT_GLOBAL_LIMIT)))
        .clone()
        .try_acquire_owned()
        .map_err(|_| LificError::Conflict("too many GitHub imports are already running".into()))?;
    Ok((global_permit, project_permit))
}

fn default_import_state() -> String {
    "all".to_string()
}
fn default_map_open() -> String {
    "backlog".to_string()
}
fn default_map_closed() -> String {
    "done".to_string()
}

/// POST /api/projects/{id}/import/github — run (or preview) a GitHub import
/// into this project.
///
/// Synchronous for v1: the request blocks until the import completes and
/// returns the [`crate::import::ImportSummary`]. The fetch + DB work runs in a
/// `spawn_blocking` task because the importer uses the blocking reqwest client.
/// Progress is a spinner on the client; a real dry-run preview precedes the
/// write so the operator sees counts first. Gated on project-lead (same bar as
/// editing project structure).
///
/// The actual collect/apply is delegated to [`import_github_with`], which takes
/// the fetcher as a parameter so tests can stub the network entirely.
pub(super) async fn import_github(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Path(project_id): Path<i64>,
    Json(req): Json<GithubImportRequest>,
) -> Result<Json<crate::import::ImportSummary>, LificError> {
    require_project_lead(&db, &identity, project_id)?;
    // Move both permits into the blocking closure. `spawn_blocking` cannot
    // stop a running blocking task when this request is cancelled; keeping
    // the permits in that closure prevents a cancelled request from releasing
    // admission while its network/DB work is still running.
    let (global_permit, project_permit) = github_import_permits(project_id)?;

    // Resolve the import-bot owner from the authenticated user (the bot is
    // owned by whoever ran the import), so audit provenance is correct. On a
    // dry run we skip bot creation entirely.
    let owner_id = identity.as_ref().map(|i| i.user.id);
    let dry_run = req.dry_run;

    let db2 = db.clone();
    let summary = tokio::task::spawn_blocking(move || {
        let _global_permit = global_permit;
        let _project_permit = project_permit;
        run_github_import_blocking(&db2, project_id, owner_id, &req)
    })
    .await
    .map_err(|e| LificError::Internal(format!("import task failed: {e}")))??;

    if !dry_run {
        realtime.send(RealtimeEvent::ProjectUpdated { project_id });
    }

    Ok(Json(summary))
}

/// The blocking body of [`import_github`], factored out so it runs off the
/// async runtime (blocking reqwest) and so tests can call the injectable
/// [`import_github_with`] variant directly.
fn run_github_import_blocking(
    db: &DbPool,
    project_id: i64,
    owner_id: Option<i64>,
    req: &GithubImportRequest,
) -> Result<crate::import::ImportSummary, LificError> {
    let (owner, name) =
        crate::import::github::parse_repo(&req.repo).map_err(LificError::BadRequest)?;
    let state =
        crate::import::github::StateFilter::parse(&req.state).map_err(LificError::BadRequest)?;
    let fetcher = crate::import::github::LiveGithub::new(&owner, &name, req.token.clone())
        .map_err(LificError::Internal)?;
    let slug = format!("{owner}/{name}");
    import_github_with(db, project_id, owner_id, &fetcher, &slug, state, req)
}

/// Core import logic with the fetcher injected. `owner_id` is the human who
/// owns the import bot (comments are attributed to it); `None` (fresh install /
/// dry run) skips comment attribution. Shared by the live handler and tests.
pub(super) fn import_github_with(
    db: &DbPool,
    project_id: i64,
    owner_id: Option<i64>,
    fetcher: &dyn crate::import::github::GithubFetcher,
    slug: &str,
    state: crate::import::github::StateFilter,
    req: &GithubImportRequest,
) -> Result<crate::import::ImportSummary, LificError> {
    // LIF-385: `map_open` / `map_closed` arrive as free text from the web
    // Import panel; reject a bad one with 400 up front instead of letting every
    // insert fail against the status CHECK constraint.
    let status_map = crate::import::StatusMap {
        open: req.map_open.parse().map_err(LificError::BadRequest)?,
        closed: req.map_closed.parse().map_err(LificError::BadRequest)?,
    };
    let fetched = crate::import::github::collect(fetcher, slug, state, &status_map)
        .map_err(LificError::Internal)?;

    // A dry run never mints a bot or writes; a real run resolves/creates the
    // import bot owned by the requester.
    let bot = if req.dry_run {
        None
    } else {
        match owner_id {
            Some(owner) => Some(crate::import::ensure_import_bot(
                db,
                owner,
                "github",
                "GitHub Import",
            )?),
            None => None,
        }
    };

    crate::import::run_import(db, project_id, bot, &fetched, req.dry_run)
}

#[cfg(test)]
mod tests {
    use super::{GithubImportRequest, github_import_permits, import_github_with};
    use crate::api::test_helpers::*;
    use crate::db::models::*;
    use axum::Extension;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn github_import_gate_is_per_project_and_survives_task_abort() {
        let project_id = i64::MIN + 1;
        let (global, project) = github_import_permits(project_id).unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let task = tokio::task::spawn_blocking(move || {
            // A blocking task keeps running after JoinHandle::abort(). The
            // permits must therefore live in this closure, not in the request
            // future that spawned it.
            let _global = global;
            let _project = project;
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv().unwrap();
        task.abort();
        assert!(
            github_import_permits(project_id).is_err(),
            "same project must remain closed while aborted blocking work runs"
        );

        // A different project is admitted while the first one is occupied.
        let (_other_global, _other_project) = github_import_permits(project_id + 1).unwrap();
        release_tx.send(()).unwrap();
        let _ = task.await;
    }

    #[tokio::test]
    async fn project_crud_lifecycle() {
        let app = test_app();

        // Create
        let (id, project) = seed_project(&app).await;
        assert_eq!(project["identifier"], "TST");

        // Get
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/projects/{id}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // List
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/projects")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let list: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(list.len(), 1);

        // Update
        let update = serde_json::json!({"name": "Renamed"});
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/projects/{id}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&update).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let updated: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(updated["name"], "Renamed");
        assert_eq!(updated["identifier"], "TST"); // unchanged

        // Delete
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/projects/{id}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Verify gone
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/projects/{id}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_nonexistent_project_returns_404() {
        let app = test_app();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/projects/99999")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn board_groups_by_status() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;

        for (title, status) in [("A", "todo"), ("B", "active"), ("C", "todo")] {
            let body = serde_json::json!({
                "project_id": project_id,
                "title": title,
                "status": status
            });
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/issues")
                        .header("content-type", "application/json")
                        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                        .unwrap(),
                )
                .await
                .unwrap();
        }

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/projects/{project_id}/board"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let board: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(board["todo"].as_array().unwrap().len(), 2);
        assert_eq!(board["active"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn issue_counts_returns_per_status_tallies_and_total() {
        let app = test_app();
        let (project_id, _) = seed_project(&app).await;

        for (title, status) in [("A", "todo"), ("B", "active"), ("C", "todo"), ("D", "done")] {
            let body = serde_json::json!({
                "project_id": project_id,
                "title": title,
                "status": status
            });
            json_post(&app, "/api/issues", body).await;
        }

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/projects/{project_id}/issue-counts"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let counts = parse_json(resp).await;
        assert_eq!(counts["backlog"], 0);
        assert_eq!(counts["todo"], 2);
        assert_eq!(counts["active"], 1);
        assert_eq!(counts["done"], 1);
        assert_eq!(counts["cancelled"], 0);
        assert_eq!(counts["total"], 4);
    }

    #[tokio::test]
    async fn board_groups_by_module_resolves_names() {
        let db = crate::db::open_memory().expect("test db");
        // Seed a real admin so create_project's lead-defaulting (LIF-102)
        // can FK to a valid user row.
        let admin_id = {
            let conn = db.write().unwrap();
            conn.execute(
                "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot)
                 VALUES ('test-admin', 'admin@test.local', 'x', 'Test Admin', 1, 0)",
                [],
            )
            .unwrap();
            conn.last_insert_rowid()
        };
        let app = crate::api::router(db.clone(), &[])
            .layer(Extension(crate::realtime::RealtimeHub::new()))
            .layer(Extension(crate::config::AuthConfig {
                allow_signup: true,
                required: true,
                secure_cookies: false,
            }))
            .layer(Extension(Some(AuthUser {
                id: admin_id,
                username: "test-admin".into(),
                display_name: "Test Admin".into(),
                is_admin: true,
            })))
            .layer(Extension(Some(crate::resolve_caller::ResolvedIdentity {
                user: AuthUser {
                    id: admin_id,
                    username: "test-admin".into(),
                    display_name: "Test Admin".into(),
                    is_admin: true,
                },
                transport: crate::actor::Transport::Web,
            })))
            .layer(Extension(Some(crate::resolve_caller::ResolvedIdentity {
                user: AuthUser {
                    id: admin_id,
                    username: "test-admin".into(),
                    display_name: "Test Admin".into(),
                    is_admin: true,
                },
                transport: crate::actor::Transport::Web,
            })));
        let (project_id, _) = seed_project(&app).await;

        // Create a module via direct DB access
        let conn = db.read().unwrap();
        crate::db::queries::create_module(
            &conn,
            &CreateModule {
                project_id,
                name: "Backend".into(),
                description: String::new(),
                status: "active".into(),
                emoji: None,
            },
        )
        .unwrap();
        let modules = crate::db::queries::list_modules(&conn, project_id).unwrap();
        let module_id = modules[0].id;
        drop(conn);

        // Create issues: one with module, one without
        for (title, mid) in [("With mod", Some(module_id)), ("No mod", None)] {
            let mut body = serde_json::json!({
                "project_id": project_id,
                "title": title,
            });
            if let Some(m) = mid {
                body["module_id"] = serde_json::json!(m);
            }
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/issues")
                        .header("content-type", "application/json")
                        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                        .unwrap(),
                )
                .await
                .unwrap();
        }

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/projects/{project_id}/board?group_by=module"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let board: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            board.get("has_module").is_none(),
            "should not use 'has_module' as key"
        );
        assert_eq!(board["Backend"].as_array().unwrap().len(), 1);
        assert_eq!(board["unassigned"].as_array().unwrap().len(), 1);
    }

    /// LIF-364 (dr.leech's report): with `authz_enforced` ON, an instance
    /// admin must see every project in `GET /api/projects` — including ones
    /// they are not a member of — while a plain non-member sees none. This
    /// pins the full REST stack (identity extension → visible_project_ids →
    /// filter_visible), not just the authz unit.
    #[tokio::test]
    async fn enforced_admin_sees_all_projects_non_member_sees_none() {
        let (db, admin, _lead, _maint, _viewer, non_member, project_id) =
            setup_membership_test();

        let resp = json_get(&app_as_user(db.clone(), &admin), "/api/projects").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let projects = parse_json(resp).await;
        let ids: Vec<i64> = projects
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].as_i64().unwrap())
            .collect();
        assert!(
            ids.contains(&project_id),
            "admin (non-member) must see the project, got {ids:?}"
        );

        let resp = json_get(&app_as_user(db, &non_member), "/api/projects").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let projects = parse_json(resp).await;
        assert_eq!(
            projects.as_array().unwrap().len(),
            0,
            "plain non-member must see no projects under enforcement"
        );
    }

    // ── Project lead permission tests ────────────────────────

    #[tokio::test]
    async fn project_lead_can_update_own_project() {
        let (db, _, lead, _, project_id) = setup_lead_test();
        let app = app_as_user(db, &lead);

        let update = serde_json::json!({"name": "Renamed by lead"});
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/projects/{project_id}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&update).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        assert_eq!(data["name"], "Renamed by lead");
    }

    #[tokio::test]
    async fn admin_can_update_any_project() {
        let (db, admin, _, _, project_id) = setup_lead_test();
        let app = app_as_user(db, &admin);

        let update = serde_json::json!({"name": "Renamed by admin"});
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/projects/{project_id}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&update).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn regular_user_cannot_update_project() {
        let (db, _, _, regular, project_id) = setup_lead_test();
        let app = app_as_user(db, &regular);

        let update = serde_json::json!({"name": "Hijacked"});
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/projects/{project_id}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&update).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn only_admin_can_delete_project() {
        let (db, admin, lead, _, project_id) = setup_lead_test();

        // Lead cannot delete
        let lead_app = app_as_user(db.clone(), &lead);
        let resp = lead_app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/projects/{project_id}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        // Admin can delete
        let admin_app = app_as_user(db, &admin);
        let resp = admin_app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/projects/{project_id}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── LIF-102: project edit blocked when project has no lead ────────────
    //
    // The previous behavior compared `Some(user.id)` to `project.lead_user_id`,
    // which was `None`, so every non-admin user was rejected forever. The fix
    // is two-part: default the creator as lead on create (so the unowned state
    // is uncommon), and explicitly route the `None` case to admin-only access.

    /// Create a project with `lead_user_id = NULL` via direct DB access,
    /// bypassing the API's default-creator-as-lead behavior.
    fn seed_unowned_project(db: &crate::db::DbPool) -> i64 {
        let conn = db.write().unwrap();
        crate::db::queries::create_project(
            &conn,
            &CreateProject {
                name: "Unowned".into(),
                identifier: "UNO".into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id
    }

    #[tokio::test]
    async fn non_admin_cannot_edit_unowned_project() {
        let (db, _, _, regular, _) = setup_lead_test();
        let project_id = seed_unowned_project(&db);
        let app = app_as_user(db, &regular);

        let update = serde_json::json!({"name": "Sneaky rename"});
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/projects/{project_id}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&update).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let data = parse_json(resp).await;
        // Distinct message tells the user *why* they can't edit: no lead exists.
        assert!(
            data["error"].as_str().unwrap_or("").contains("no lead"),
            "expected 'no lead' in error, got: {}",
            data["error"]
        );
    }

    #[tokio::test]
    async fn admin_can_edit_unowned_project() {
        let (db, admin, _, _, _) = setup_lead_test();
        let project_id = seed_unowned_project(&db);
        let app = app_as_user(db, &admin);

        let update = serde_json::json!({"name": "Renamed by admin"});
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/projects/{project_id}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&update).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        assert_eq!(data["name"], "Renamed by admin");
    }

    // ── LIF-103: tristate clear via HTTP ─────────────────────────────────
    //
    // The model now distinguishes "field absent" from "field explicitly null"
    // so clients can wipe emoji/lead back to NULL. Before the fix, both
    // shapes collapsed to None and the update path skipped the column.

    #[tokio::test]
    async fn update_with_null_emoji_clears_emoji() {
        let (db, admin, _, _, _) = setup_lead_test();
        let app = app_as_user(db.clone(), &admin);

        // Seed a project with an emoji set.
        let project = {
            let conn = db.write().unwrap();
            crate::db::queries::create_project(
                &conn,
                &CreateProject {
                    name: "With Emoji".into(),
                    identifier: "EMJ".into(),
                    emoji: Some("🧪".into()),
                    lead_user_id: Some(admin.id),
                    ..Default::default()
                },
            )
            .unwrap()
        };
        assert_eq!(project.emoji.as_deref(), Some("🧪"));

        // PUT with explicit null.
        let update = serde_json::json!({"emoji": null});
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/projects/{}", project.id))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&update).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        assert!(
            data["emoji"].is_null(),
            "expected null emoji, got: {}",
            data["emoji"]
        );
    }

    #[tokio::test]
    async fn update_with_null_lead_clears_lead() {
        let (db, admin, lead, _, project_id) = setup_lead_test();
        // setup_lead_test creates project with lead set.
        let app = app_as_user(db.clone(), &admin); // admin can edit any project

        // Sanity check: lead is set.
        let pre: serde_json::Value = {
            let conn = db.read().unwrap();
            let p = crate::db::queries::get_project(&conn, project_id).unwrap();
            serde_json::to_value(&p).unwrap()
        };
        assert_eq!(pre["lead_user_id"].as_i64(), Some(lead.id));

        let update = serde_json::json!({"lead_user_id": null});
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/projects/{project_id}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&update).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        assert!(
            data["lead_user_id"].is_null(),
            "expected null lead_user_id, got: {}",
            data["lead_user_id"]
        );
    }

    #[tokio::test]
    async fn update_with_empty_body_changes_nothing() {
        let (db, admin, lead, _, project_id) = setup_lead_test();
        let app = app_as_user(db, &admin);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/projects/{project_id}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(b"{}".to_vec()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        // Project from setup_lead_test has name "Lead Test", identifier "LDT",
        // lead set, no emoji.
        assert_eq!(data["name"], "Lead Test");
        assert_eq!(data["identifier"], "LDT");
        assert_eq!(data["lead_user_id"].as_i64(), Some(lead.id));
        assert!(data["emoji"].is_null());
    }

    #[tokio::test]
    async fn update_lead_to_nonexistent_user_returns_400() {
        let (db, admin, _, _, project_id) = setup_lead_test();
        let app = app_as_user(db, &admin);

        let update = serde_json::json!({"lead_user_id": 99999});
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/projects/{project_id}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&update).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let data = parse_json(resp).await;
        assert!(
            data["error"].as_str().unwrap_or("").contains("not found"),
            "expected 'not found' in error, got: {}",
            data["error"]
        );
    }

    // ── LIF-233: sidebar reordering ──────────────────────────

    /// POST a project with an explicit identifier; return its id.
    async fn seed_named_project(app: &axum::Router, name: &str, ident: &str) -> i64 {
        let resp = json_post(
            app,
            "/api/projects",
            serde_json::json!({ "name": name, "identifier": ident }),
        )
        .await;
        parse_json(resp).await["id"].as_i64().unwrap()
    }

    async fn list_project_names(app: &axum::Router) -> Vec<String> {
        let resp = json_get(app, "/api/projects").await;
        parse_json(resp)
            .await
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap().to_string())
            .collect()
    }

    #[tokio::test]
    async fn reorder_persists_new_order() {
        let app = test_app();
        let a = seed_named_project(&app, "Alpha", "AAA").await;
        let b = seed_named_project(&app, "Beta", "BBB").await;
        let c = seed_named_project(&app, "Gamma", "GGG").await;

        // Default order is alphabetical.
        assert_eq!(list_project_names(&app).await, ["Alpha", "Beta", "Gamma"]);

        // Reorder: Gamma, Alpha, Beta.
        let resp = json_put(
            &app,
            "/api/projects/reorder",
            serde_json::json!({ "ids": [c, a, b] }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        // Response echoes the new order.
        let returned: Vec<String> = parse_json(resp)
            .await
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(returned, ["Gamma", "Alpha", "Beta"]);
        // And a fresh GET reflects it.
        assert_eq!(list_project_names(&app).await, ["Gamma", "Alpha", "Beta"]);
    }

    #[tokio::test]
    async fn reorder_allowed_for_non_lead_user() {
        // Unlike update_project (lead/admin-only), reordering is open to any
        // authenticated user since sidebar order is instance-wide chrome.
        let (db, _, _, regular, _) = setup_lead_test();
        let app = app_as_user(db, &regular);

        // setup_lead_test already created project "LDT"; add a second.
        let ldt = {
            // resolve LDT's id from the list
            let names = json_get(&app, "/api/projects").await;
            parse_json(names).await[0]["id"].as_i64().unwrap()
        };
        let second = seed_named_project(&app, "Second", "SEC").await;

        let resp = json_put(
            &app,
            "/api/projects/reorder",
            serde_json::json!({ "ids": [second, ldt] }),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "a regular (non-lead) user should be allowed to reorder"
        );
        assert_eq!(list_project_names(&app).await, ["Second", "Lead Test"]);
    }

    #[tokio::test]
    async fn reorder_with_unknown_id_returns_400() {
        let app = test_app();
        let a = seed_named_project(&app, "Alpha", "AAA").await;
        let resp = json_put(
            &app,
            "/api/projects/reorder",
            serde_json::json!({ "ids": [a, 99999] }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_project_defaults_lead_to_creator() {
        // setup_lead_test gives us a real lead user we can authenticate as.
        let (db, _, lead, _, _) = setup_lead_test();
        let app = app_as_user(db, &lead);

        let body = serde_json::json!({
            "name": "My Project",
            "identifier": "MINE",
            "description": ""
            // intentionally no lead_user_id
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/projects")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        assert_eq!(
            data["lead_user_id"].as_i64(),
            Some(lead.id),
            "expected lead defaulted to creator, got: {}",
            data["lead_user_id"]
        );

        // And the creator can subsequently edit it (the whole point — no more trap).
        let pid = data["id"].as_i64().unwrap();
        let update = serde_json::json!({"name": "Renamed by creator"});
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/projects/{pid}"))
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&update).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── GitHub import endpoint (LIF-264) ─────────────────────
    //
    // The HTTP handler wires a LiveGithub fetcher (real network), so we test
    // the injectable core `import_github_with` with a fake fetcher — exercising
    // the same collect → apply pipeline the endpoint uses, with zero network.

    use crate::db::DbPool;
    use crate::import::github::{
        GithubComment, GithubFetcher, GithubIssue, GithubUser, StateFilter,
    };

    fn import_pool() -> (DbPool, i64, i64) {
        let db = crate::db::open_memory().unwrap();
        let (pid, owner) = {
            let conn = db.write().unwrap();
            conn.execute(
                "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot)
                 VALUES ('boss', 'boss@test.local', 'x', 'Boss', 1, 0)",
                [],
            )
            .unwrap();
            let owner = conn.last_insert_rowid();
            let pid = crate::db::queries::create_project(
                &conn,
                &CreateProject {
                    name: "App".into(),
                    identifier: "APP".into(),
                    lead_user_id: Some(owner),
                    ..Default::default()
                },
            )
            .unwrap()
            .id;
            (pid, owner)
        };
        (db, pid, owner)
    }

    struct FakeGithub;
    impl GithubFetcher for FakeGithub {
        fn fetch_issues_page(
            &self,
            page: u32,
            _state: StateFilter,
        ) -> Result<(Vec<GithubIssue>, bool), String> {
            if page > 1 {
                return Ok((vec![], false));
            }
            let issues: Vec<GithubIssue> = serde_json::from_str(
                r#"[
                    {"number":1,"title":"Open one","body":"b","state":"open","labels":[{"name":"bug","color":"d73a4a"}],"assignees":[]},
                    {"number":2,"title":"Closed one","body":"","state":"closed","labels":[],"assignees":[]},
                    {"number":3,"title":"A PR","body":"","state":"open","labels":[],"assignees":[],"pull_request":{"url":"x"}}
                ]"#,
            )
            .unwrap();
            Ok((issues, false))
        }
        fn fetch_comments(&self, _n: i64) -> Result<Vec<GithubComment>, String> {
            Ok(vec![GithubComment {
                user: Some(GithubUser {
                    login: "octocat".into(),
                }),
                body: Some("nice".into()),
                created_at: Some("2024-01-01T00:00:00Z".into()),
            }])
        }
    }

    fn req(dry_run: bool) -> GithubImportRequest {
        GithubImportRequest {
            repo: "octocat/hello".into(),
            token: None,
            state: "all".into(),
            map_open: "backlog".into(),
            map_closed: "done".into(),
            dry_run,
        }
    }

    #[test]
    fn import_github_dry_run_counts_and_writes_nothing() {
        let (db, pid, owner) = import_pool();
        let summary = import_github_with(
            &db,
            pid,
            Some(owner),
            &FakeGithub,
            "octocat/hello",
            StateFilter::All,
            &req(true),
        )
        .unwrap();
        assert!(summary.dry_run);
        assert_eq!(summary.issues_created, 2, "PR filtered out");
        assert_eq!(summary.skipped_non_issues, 1);
        assert_eq!(summary.comments_planned, 2);
        assert_eq!(summary.labels_planned, 1);
        // Nothing written.
        let conn = db.read().unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM issues", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn import_github_real_run_writes_and_is_idempotent() {
        let (db, pid, owner) = import_pool();
        let s1 = import_github_with(
            &db,
            pid,
            Some(owner),
            &FakeGithub,
            "octocat/hello",
            StateFilter::All,
            &req(false),
        )
        .unwrap();
        assert_eq!(s1.issues_created, 2);
        assert_eq!(s1.comments_created, 2);
        assert_eq!(s1.labels_created, 1);

        // Re-run: idempotent no-op.
        let s2 = import_github_with(
            &db,
            pid,
            Some(owner),
            &FakeGithub,
            "octocat/hello",
            StateFilter::All,
            &req(false),
        )
        .unwrap();
        assert_eq!(s2.issues_created, 0);
        assert_eq!(s2.issues_skipped_existing, 2);

        // Verify status mapping + source markers landed.
        let conn = db.read().unwrap();
        let (open_status, open_src): (String, String) = conn
            .query_row(
                "SELECT status, source FROM issues WHERE source = 'github:octocat/hello#1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(open_status, "backlog");
        assert_eq!(open_src, "github:octocat/hello#1");
        let closed_status: String = conn
            .query_row(
                "SELECT status FROM issues WHERE source = 'github:octocat/hello#2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(closed_status, "done");
    }
}
