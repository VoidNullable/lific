//! LIF-449: the REST surface over the repo → project bindings stored by
//! LIF-448 (`db::queries::repo_bindings`, migration 049). Design doctrine:
//! LIF-DOC-27.
//!
//! The single rule everything here follows: **an identity alias is a
//! selector, never proof of ownership.** The root OID of a public repository
//! is public, and any string at all can be asserted as a remote. So presenting
//! an alias buys the caller nothing:
//!
//! * Mutations (`bind`, `delete`, `merge`) are Lead-or-admin on the project
//!   that is (or becomes) bound, checked once on a read connection to keep an
//!   unauthorized caller off the writer and again *inside* the write
//!   transaction against a freshly read caller, the same shape
//!   `api::members` uses.
//! * `resolve` is visibility-filtered and answers with a constant shape. A
//!   match the caller cannot see is byte-identical to no match at all, so the
//!   endpoint is not an existence oracle for other people's projects.
//! * `bind` never names the project an alias already belongs to. Learning
//!   where an alias points is `resolve`'s job, and `resolve` is filtered.
//! * Aliases are never accumulated automatically. `bind` adds the ones the
//!   caller presented because the caller explicitly asked to, and that request
//!   is authorized; nothing here widens a binding as a side effect of a read.

use std::collections::{BTreeSet, HashSet};
use std::sync::LazyLock;
use std::time::Duration;

use axum::{
    Extension,
    extract::{Json, Path, State},
};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::json;

use crate::authz;
use crate::db::queries::repo_bindings::{self as bindings, Resolution};
use crate::db::{DbPool, models::*, queries};
use crate::error::LificError;
use crate::ratelimit::RateLimiter;
use crate::resolve_caller::ResolvedIdentity;

use super::with_read;

/// Repo-binding mutations, capped per authenticated user.
///
/// The login/signup limiter is injected from `server.rs` as an axum
/// `Extension`, because its window is operator-configurable. This one is not:
/// it is a fixed policy ceiling on a low-frequency administrative action, so
/// it lives module-locally and the router signature stays exactly as it is.
/// Nothing outside this module needs to see it.
static BIND_LIMITER: LazyLock<RateLimiter> =
    LazyLock::new(|| RateLimiter::new(30, Duration::from_secs(3600)));

/// The refusal a `bind`/`delete`/`merge` gets once a caller has spent their
/// hour's budget. Same shape the auth endpoints use for their limiters.
fn check_mutation_rate(user_id: i64) -> Result<(), LificError> {
    let key = format!("bind:{user_id}");
    if BIND_LIMITER.check(&key) {
        return Ok(());
    }
    let retry = BIND_LIMITER.retry_after(&key);
    Err(LificError::BadRequest(
        crate::ratelimit::retry_after_message("too many repo binding changes", retry),
    ))
}

/// The one thing `bind` says about an alias somebody else owns. Constant on
/// purpose: naming the other project would turn an unauthenticated-ish guess
/// into a cross-project read, and `resolve` already answers that question
/// under a visibility filter.
const ALREADY_BOUND: &str = "one or more aliases are already bound";

/// The one thing the binding-scoped routes say about a binding the caller may
/// not touch. Identical for "no such binding" and "bound to a project you
/// cannot see", so the pair cannot be used to probe for either.
const BINDING_NOT_FOUND: &str = "repo binding not found";

fn binding_not_found() -> LificError {
    LificError::NotFound(BINDING_NOT_FOUND.into())
}

// ── Request bodies ───────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub(super) struct AliasInput {
    kind: String,
    value: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct ResolveRequest {
    #[serde(default)]
    aliases: Vec<AliasInput>,
}

#[derive(Debug, Deserialize)]
pub(super) struct BindRequest {
    project: String,
    #[serde(default)]
    aliases: Vec<AliasInput>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MergeRequest {
    from: i64,
    into: i64,
}

/// The `(kind, value)` pairs the query layer takes, deduplicated.
///
/// A caller presenting the same alias twice means the same thing as
/// presenting it once, and letting the duplicate through would collide with
/// `UNIQUE(kind, value)` on the second insert and report a conflict against
/// the caller's own binding.
fn alias_pairs(aliases: &[AliasInput]) -> Vec<(&str, &str)> {
    let mut seen = HashSet::new();
    aliases
        .iter()
        .map(|alias| (alias.kind.as_str(), alias.value.as_str()))
        .filter(|pair| seen.insert(*pair))
        .collect()
}

// ── Visibility ───────────────────────────────────────────────

/// Whether `project_id` is inside the caller's cross-project read filter.
/// `None` is the unrestricted answer `authz::visible_project_ids` returns for
/// an admin, or whenever enforcement is off.
fn project_visible(visible: &Option<HashSet<i64>>, project_id: i64) -> bool {
    visible.as_ref().is_none_or(|ids| ids.contains(&project_id))
}

/// `{ id, identifier, name }` for one project — the only project fields
/// `resolve` ever discloses, and only for a project the caller can see.
fn project_summary(conn: &Connection, project_id: i64) -> Result<serde_json::Value, LificError> {
    let project = queries::get_project(conn, project_id)?;
    Ok(json!({
        "id": project.id,
        "identifier": project.identifier,
        "name": project.name,
    }))
}

/// The project a binding points at, or the constant 404 when the binding does
/// not exist *or* its project is invisible. The two cases deliberately
/// produce the identical error.
fn visible_binding_project(
    conn: &Connection,
    visible: &Option<HashSet<i64>>,
    binding_id: i64,
) -> Result<i64, LificError> {
    let binding = bindings::get(conn, binding_id)?.ok_or_else(binding_not_found)?;
    if !project_visible(visible, binding.project_id) {
        return Err(binding_not_found());
    }
    Ok(binding.project_id)
}

/// A binding plus the aliases that resolve to it — the shape `bind`, `merge`
/// and the per-project listing all return.
fn binding_with_identities(
    conn: &Connection,
    binding: RepoBinding,
) -> Result<serde_json::Value, LificError> {
    let identities = bindings::list_identities(conn, binding.id)?;
    Ok(json!({ "binding": binding, "identities": identities }))
}

// ── POST /api/repos/resolve ──────────────────────────────────

/// What a checkout's aliases point at, filtered to what the caller may see.
///
/// Open to any authenticated user, because the aliases are the caller's own
/// checkout and the answer is confined to projects they already have read
/// access to. Three constant shapes, and only three:
///
/// ```json
/// {"resolution": "none"}
/// {"resolution": "one", "project": {...}, "binding_id": 7}
/// {"resolution": "conflict", "projects": [{...}, {...}]}
/// ```
///
/// The filter is applied to the *bindings*, then the shape is decided from
/// what survives. A conflict spanning a visible and an invisible project is
/// reported as `one`; a match in a project the caller cannot see is reported
/// as `none`, byte for byte identical to an alias nobody has ever bound.
/// Every conflict entry carries its `binding_id` so a caller who can see both
/// sides has what `POST /api/repos/merge` needs.
pub(super) async fn resolve_repo(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<ResolvedIdentity>>,
    Json(input): Json<ResolveRequest>,
) -> Result<Json<serde_json::Value>, LificError> {
    super::require_user(&identity)?;
    let visible = authz::visible_project_ids(&db, &identity)?;
    let pairs = alias_pairs(&input.aliases);

    with_read(&db, |conn| {
        let found = match bindings::resolve(conn, &pairs)? {
            Resolution::None => Vec::new(),
            Resolution::One(binding) => vec![binding],
            Resolution::Conflict(list) => list,
        };
        let visible_bindings: Vec<RepoBinding> = found
            .into_iter()
            .filter(|binding| project_visible(&visible, binding.project_id))
            .collect();

        match visible_bindings.split_first() {
            None => Ok(json!({ "resolution": "none" })),
            Some((only, [])) => Ok(json!({
                "resolution": "one",
                "project": project_summary(conn, only.project_id)?,
                "binding_id": only.id,
            })),
            Some(_) => {
                let mut projects = Vec::with_capacity(visible_bindings.len());
                for binding in &visible_bindings {
                    let mut summary = project_summary(conn, binding.project_id)?;
                    summary["binding_id"] = json!(binding.id);
                    projects.push(summary);
                }
                Ok(json!({ "resolution": "conflict", "projects": projects }))
            }
        }
    })
    .map(Json)
}

// ── POST /api/repos/bind ─────────────────────────────────────

/// Decide what the presented aliases mean for `project_id`, and do it.
///
/// Each alias is resolved on its own, so the three cases stay distinguishable:
/// unclaimed, claimed by a binding of this project, claimed by a binding of
/// some other project. The last one refuses with the constant message and
/// never says whose.
///
/// Extending an existing binding with the aliases the caller just presented is
/// not the automatic accumulation LIF-DOC-27 rules out: the caller asked for
/// exactly this, and the request is Lead-gated.
fn bind_in_transaction(
    conn: &Connection,
    project_id: i64,
    created_by: i64,
    pairs: &[(&str, &str)],
) -> Result<RepoBinding, LificError> {
    let mut ours: BTreeSet<i64> = BTreeSet::new();
    let mut unclaimed: Vec<(&str, &str)> = Vec::new();

    for pair in pairs {
        match bindings::resolve(conn, std::slice::from_ref(pair))? {
            Resolution::None => unclaimed.push(*pair),
            Resolution::One(binding) => {
                if binding.project_id != project_id {
                    return Err(LificError::Conflict(ALREADY_BOUND.into()));
                }
                ours.insert(binding.id);
            }
            // Unreachable for a single `(kind, value)`, which `UNIQUE` pins to
            // at most one binding. Treated as the ordinary refusal rather than
            // an internal error: whatever it is, it is not something this
            // request can safely extend.
            Resolution::Conflict(_) => return Err(LificError::Conflict(ALREADY_BOUND.into())),
        }
    }

    // The aliases span two bindings of this same project. Both are the
    // caller's, but which one to extend is not ours to guess — that is what
    // `POST /api/repos/merge` is for.
    if ours.len() > 1 {
        return Err(LificError::Conflict(ALREADY_BOUND.into()));
    }

    match ours.first().copied() {
        Some(binding_id) => {
            for (kind, value) in &unclaimed {
                bindings::add_identity(conn, binding_id, kind, value)?;
            }
            bindings::get(conn, binding_id)?.ok_or_else(binding_not_found)
        }
        None => bindings::create_binding(conn, project_id, Some(created_by), pairs),
    }
}

/// Point a repository at a project. Lead-or-admin on that project.
///
/// Idempotent: re-presenting aliases that already belong to this project's
/// binding returns that binding untouched.
pub(super) async fn bind_repo(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<ResolvedIdentity>>,
    Json(input): Json<BindRequest>,
) -> Result<Json<serde_json::Value>, LificError> {
    let caller = super::require_user(&identity)?;
    check_mutation_rate(caller.id)?;

    if input.aliases.is_empty() {
        return Err(LificError::BadRequest(
            "at least one alias is required to bind a repository".into(),
        ));
    }
    let pairs = alias_pairs(&input.aliases);

    let project_id = with_read(&db, |conn| {
        queries::resolve_project_identifier(conn, &input.project)
    })?;
    // Cheap pre-check on a read connection so an unauthorized caller never
    // reaches the writer; re-run authoritatively inside the transaction.
    authz::require_role(&db, &identity, project_id, Role::Lead)?;

    db.transaction(|tx| {
        let fresh = crate::auth::fresh_caller(tx, caller.id)?;
        let fresh_identity = Some(crate::auth::fresh_identity(
            &fresh,
            crate::actor::Transport::Web,
        ));
        authz::require_role_conn(tx, &fresh_identity, project_id, Role::Lead)?;
        let binding = bind_in_transaction(tx, project_id, caller.id, &pairs)?;
        binding_with_identities(tx, binding)
    })
    .map(Json)
}

// ── GET /api/projects/{id}/bindings ──────────────────────────

/// Every binding pointing at this project, with its aliases. Viewer-gated,
/// the same bar as reading the project's issues: if you can read the project
/// you can see which checkouts file against it.
pub(super) async fn list_project_bindings(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<ResolvedIdentity>>,
    Path(project_id): Path<i64>,
) -> Result<Json<Vec<serde_json::Value>>, LificError> {
    authz::require_role(&db, &identity, project_id, Role::Viewer)?;
    with_read(&db, |conn| {
        let found = bindings::list_for_project(conn, project_id)?;
        let mut out = Vec::with_capacity(found.len());
        for binding in found {
            out.push(binding_with_identities(conn, binding)?);
        }
        Ok(out)
    })
    .map(Json)
}

// ── DELETE /api/repos/bindings/{id} ──────────────────────────

/// Unbind a repository. Lead-or-admin on the project it points at.
///
/// This is also the reclaim path: an instance admin passes `require_role` by
/// the admin short-circuit, so a binding made by somebody who has since left
/// can always be cleared without touching the database by hand.
pub(super) async fn delete_repo_binding(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<ResolvedIdentity>>,
    Path(binding_id): Path<i64>,
) -> Result<Json<serde_json::Value>, LificError> {
    let caller = super::require_user(&identity)?;
    check_mutation_rate(caller.id)?;

    // 404 before 403: a binding in a project the caller cannot see must be
    // indistinguishable from one that does not exist.
    let visible = authz::visible_project_ids(&db, &identity)?;
    let project_id = with_read(&db, |conn| {
        visible_binding_project(conn, &visible, binding_id)
    })?;
    authz::require_role(&db, &identity, project_id, Role::Lead)?;

    db.transaction(|tx| {
        let fresh = crate::auth::fresh_caller(tx, caller.id)?;
        let fresh_identity = Some(crate::auth::fresh_identity(
            &fresh,
            crate::actor::Transport::Web,
        ));
        let current = bindings::get(tx, binding_id)?.ok_or_else(binding_not_found)?;
        authz::require_role_conn(tx, &fresh_identity, current.project_id, Role::Lead)?;
        bindings::delete_binding(tx, binding_id)?;
        Ok(())
    })?;

    Ok(Json(json!({ "deleted": true })))
}

// ── POST /api/repos/merge ────────────────────────────────────

/// Fold one binding into another, moving its aliases across.
///
/// Lead-or-admin on **both** bound projects. Requiring it only on the
/// destination would make this an expropriation primitive: any lead could
/// pull another project's repository over to their own by naming it as the
/// source.
pub(super) async fn merge_repo_bindings(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<ResolvedIdentity>>,
    Json(input): Json<MergeRequest>,
) -> Result<Json<serde_json::Value>, LificError> {
    let caller = super::require_user(&identity)?;
    check_mutation_rate(caller.id)?;

    let visible = authz::visible_project_ids(&db, &identity)?;
    let (from_project, into_project) = with_read(&db, |conn| {
        Ok((
            visible_binding_project(conn, &visible, input.from)?,
            visible_binding_project(conn, &visible, input.into)?,
        ))
    })?;
    for project_id in [from_project, into_project] {
        authz::require_role(&db, &identity, project_id, Role::Lead)?;
    }

    db.transaction(|tx| {
        let fresh = crate::auth::fresh_caller(tx, caller.id)?;
        let fresh_identity = Some(crate::auth::fresh_identity(
            &fresh,
            crate::actor::Transport::Web,
        ));
        for binding_id in [input.from, input.into] {
            let current = bindings::get(tx, binding_id)?.ok_or_else(binding_not_found)?;
            authz::require_role_conn(tx, &fresh_identity, current.project_id, Role::Lead)?;
        }
        bindings::merge(tx, input.from, input.into)?;
        let surviving = bindings::get(tx, input.into)?.ok_or_else(binding_not_found)?;
        binding_with_identities(tx, surviving)
    })
    .map(Json)
}

#[cfg(test)]
mod tests {
    use crate::api::test_helpers::*;
    use crate::db::DbPool;
    use crate::db::models::{CreateProject, Role, User};
    use crate::db::queries;
    use axum::http::StatusCode;
    use http_body_util::BodyExt;
    use rusqlite::{Connection, params};
    use std::sync::atomic::{AtomicI64, Ordering};

    /// The mutation limiter is a process-wide static keyed on `bind:{user_id}`
    /// (deliberately, so it survives across requests), and the test binary runs
    /// every test in one process. Handing each fixture its own block of user
    /// ids keeps one test's budget out of another's, in any order, at any
    /// parallelism.
    static NEXT_ID_BLOCK: AtomicI64 = AtomicI64::new(0);

    struct Fixture {
        db: DbPool,
        /// Instance admin, member of nothing.
        admin: User,
        /// Lead of project A only.
        lead: User,
        /// Maintainer of project A. Can read it, cannot bind.
        maintainer: User,
        /// A bot owned by `maintainer`, so it inherits exactly that authority.
        bot: User,
        /// Member of nothing at all.
        outsider: User,
        /// Lead of both projects.
        both: User,
        /// Project A ("APR").
        a: i64,
        /// Project B ("BPR").
        b: i64,
    }

    fn seed_user(conn: &Connection, id: i64, username: &str, is_admin: bool) -> User {
        conn.execute(
            "INSERT INTO users (id, username, email, password_hash, display_name, is_admin, is_bot)
             VALUES (?1, ?2, ?3, 'x', ?2, ?4, 0)",
            params![id, username, format!("{username}@test.local"), is_admin],
        )
        .expect("seed user");
        queries::users::get_user_by_id(conn, id).expect("read seeded user")
    }

    fn seed_bot(conn: &Connection, id: i64, username: &str, owner_id: i64) -> User {
        conn.execute(
            "INSERT INTO users (id, username, email, password_hash, display_name, is_admin, is_bot, owner_id)
             VALUES (?1, ?2, ?3, 'x', ?2, 0, 1, ?4)",
            params![id, username, format!("{username}@bot.local"), owner_id],
        )
        .expect("seed bot");
        queries::users::get_user_by_id(conn, id).expect("read seeded bot")
    }

    fn fixture() -> Fixture {
        let db = crate::db::open_memory().expect("test db");
        let base = 100_000 + NEXT_ID_BLOCK.fetch_add(1, Ordering::Relaxed) * 1_000;

        let (admin, lead, maintainer, bot, outsider, both, a, b) = {
            let conn = db.write().expect("test writer");
            queries::settings::update(
                &conn,
                queries::settings::InstanceSettingsPatch {
                    authz_enforced: Some(true),
                    ..Default::default()
                },
            )
            .expect("enable enforcement");

            let admin = seed_user(&conn, base + 1, "rb-admin", true);
            let lead = seed_user(&conn, base + 2, "rb-lead", false);
            let maintainer = seed_user(&conn, base + 3, "rb-maintainer", false);
            let outsider = seed_user(&conn, base + 4, "rb-outsider", false);
            let both = seed_user(&conn, base + 5, "rb-both", false);
            let b_lead = seed_user(&conn, base + 6, "rb-b-lead", false);
            let bot = seed_bot(&conn, base + 7, "rb-bot", maintainer.id);

            let a = queries::create_project(
                &conn,
                &CreateProject {
                    name: "Project A".into(),
                    identifier: "APR".into(),
                    lead_user_id: Some(lead.id),
                    ..Default::default()
                },
            )
            .expect("project A")
            .id;
            let b = queries::create_project(
                &conn,
                &CreateProject {
                    name: "Project B".into(),
                    identifier: "BPR".into(),
                    lead_user_id: Some(b_lead.id),
                    ..Default::default()
                },
            )
            .expect("project B")
            .id;

            queries::members::upsert_member(&conn, a, maintainer.id, Role::Maintainer).unwrap();
            queries::members::upsert_member(&conn, a, both.id, Role::Lead).unwrap();
            queries::members::upsert_member(&conn, b, both.id, Role::Lead).unwrap();

            (admin, lead, maintainer, bot, outsider, both, a, b)
        };

        Fixture {
            db,
            admin,
            lead,
            maintainer,
            bot,
            outsider,
            both,
            a,
            b,
        }
    }

    fn alias(kind: &str, value: &str) -> serde_json::Value {
        serde_json::json!({ "kind": kind, "value": value })
    }

    async fn bind(
        app: &axum::Router,
        project: &str,
        aliases: Vec<serde_json::Value>,
    ) -> axum::response::Response {
        json_post(
            app,
            "/api/repos/bind",
            serde_json::json!({ "project": project, "aliases": aliases }),
        )
        .await
    }

    async fn resolve(
        app: &axum::Router,
        aliases: Vec<serde_json::Value>,
    ) -> axum::response::Response {
        json_post(
            app,
            "/api/repos/resolve",
            serde_json::json!({ "aliases": aliases }),
        )
        .await
    }

    async fn raw_body(resp: axum::response::Response) -> Vec<u8> {
        resp.into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec()
    }

    // ── resolve ──────────────────────────────────────────────

    #[tokio::test]
    async fn resolve_reports_none_for_aliases_nobody_has_bound() {
        let f = fixture();
        let app = app_as_user(f.db, &f.lead);

        let resp = resolve(&app, vec![alias("root", "/mnt/dev/unbound")]).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(parse_json(resp).await["resolution"], "none");
    }

    #[tokio::test]
    async fn resolve_reports_the_binding_when_its_project_is_visible() {
        let f = fixture();
        let app = app_as_user(f.db, &f.lead);
        let created = parse_json(bind(&app, "APR", vec![alias("remote", "github.com/v/a")]).await)
            .await["binding"]["id"]
            .as_i64()
            .unwrap();

        let body = parse_json(resolve(&app, vec![alias("remote", "github.com/v/a")]).await).await;

        assert_eq!(body["resolution"], "one");
        assert_eq!(body["project"]["identifier"], "APR");
        assert_eq!(body["binding_id"], created);
    }

    #[tokio::test]
    async fn a_match_the_caller_cannot_see_is_byte_identical_to_no_match() {
        let f = fixture();
        let lead_app = app_as_user(f.db.clone(), &f.lead);
        bind(
            &lead_app,
            "APR",
            vec![alias("remote", "github.com/v/secret")],
        )
        .await;

        let outsider_app = app_as_user(f.db, &f.outsider);
        let hidden = resolve(&outsider_app, vec![alias("remote", "github.com/v/secret")]).await;
        let unknown = resolve(
            &outsider_app,
            vec![alias("remote", "github.com/v/never-bound")],
        )
        .await;

        assert_eq!(hidden.status(), unknown.status());
        assert_eq!(
            raw_body(hidden).await,
            raw_body(unknown).await,
            "a hidden match must not be distinguishable from no match at all"
        );
    }

    #[tokio::test]
    async fn resolve_reports_a_conflict_when_both_projects_are_visible() {
        let f = fixture();
        let both_app = app_as_user(f.db, &f.both);
        bind(&both_app, "APR", vec![alias("remote", "github.com/v/c")]).await;
        bind(&both_app, "BPR", vec![alias("root", "/mnt/dev/c")]).await;

        let body = parse_json(
            resolve(
                &both_app,
                vec![
                    alias("remote", "github.com/v/c"),
                    alias("root", "/mnt/dev/c"),
                ],
            )
            .await,
        )
        .await;

        assert_eq!(body["resolution"], "conflict");
        let identifiers: Vec<&str> = body["projects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["identifier"].as_str().unwrap())
            .collect();
        assert_eq!(identifiers, vec!["APR", "BPR"]);
    }

    #[tokio::test]
    async fn a_conflict_degrades_to_one_when_only_one_project_is_visible() {
        let f = fixture();
        let both_app = app_as_user(f.db.clone(), &f.both);
        bind(&both_app, "APR", vec![alias("remote", "github.com/v/d")]).await;
        bind(&both_app, "BPR", vec![alias("root", "/mnt/dev/d")]).await;

        // `lead` leads A and is a member of nothing else, so only one of the
        // two conflicting bindings exists as far as they are concerned.
        let lead_app = app_as_user(f.db, &f.lead);
        let body = parse_json(
            resolve(
                &lead_app,
                vec![
                    alias("remote", "github.com/v/d"),
                    alias("root", "/mnt/dev/d"),
                ],
            )
            .await,
        )
        .await;

        assert_eq!(body["resolution"], "one");
        assert_eq!(body["project"]["identifier"], "APR");
    }

    // ── bind ─────────────────────────────────────────────────

    #[tokio::test]
    async fn a_non_lead_member_cannot_bind() {
        let f = fixture();
        let app = app_as_user(f.db, &f.maintainer);

        let resp = bind(&app, "APR", vec![alias("root", "/mnt/dev/nope")]).await;

        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_bot_owned_by_a_plain_member_cannot_bind() {
        let f = fixture();
        // The bot's owner is a maintainer, and a bot never exceeds its owner.
        let app = app_as_user(f.db, &f.bot);

        let resp = bind(&app, "APR", vec![alias("root", "/mnt/dev/bot")]).await;

        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_lead_can_bind_a_repository_to_their_project() {
        let f = fixture();
        let app = app_as_user(f.db, &f.lead);

        let resp = bind(
            &app,
            "APR",
            vec![
                alias("remote", "github.com/v/lead"),
                alias("root", "/mnt/dev/lead"),
            ],
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = parse_json(resp).await;
        assert_eq!(body["binding"]["project_id"], f.a);
        let values: Vec<&str> = body["identities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["value"].as_str().unwrap())
            .collect();
        assert_eq!(values, vec!["github.com/v/lead", "/mnt/dev/lead"]);
    }

    #[tokio::test]
    async fn an_admin_can_bind_a_project_they_are_not_a_member_of() {
        let f = fixture();
        let app = app_as_user(f.db, &f.admin);

        let resp = bind(&app, "BPR", vec![alias("root", "/mnt/dev/admin")]).await;

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(parse_json(resp).await["binding"]["project_id"], f.b);
    }

    #[tokio::test]
    async fn re_binding_the_same_aliases_returns_the_same_binding() {
        let f = fixture();
        let app = app_as_user(f.db, &f.lead);
        let aliases = vec![alias("remote", "github.com/v/idem")];

        let first = parse_json(bind(&app, "APR", aliases.clone()).await).await;
        let second_resp = bind(&app, "APR", aliases).await;

        assert_eq!(second_resp.status(), StatusCode::OK);
        let second = parse_json(second_resp).await;
        assert_eq!(first["binding"]["id"], second["binding"]["id"]);
        assert_eq!(second["identities"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn binding_teaches_an_existing_binding_a_new_alias() {
        let f = fixture();
        let app = app_as_user(f.db, &f.lead);
        let first = parse_json(bind(&app, "APR", vec![alias("remote", "github.com/v/grow")]).await)
            .await["binding"]["id"]
            .as_i64()
            .unwrap();

        let resp = bind(
            &app,
            "APR",
            vec![
                alias("remote", "github.com/v/grow"),
                alias("root", "/mnt/dev/grow"),
            ],
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = parse_json(resp).await;
        assert_eq!(
            body["binding"]["id"], first,
            "the new alias joins the existing binding rather than making a second one"
        );
        assert_eq!(body["identities"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn an_alias_owned_by_another_project_is_a_constant_409() {
        let f = fixture();
        let both_app = app_as_user(f.db.clone(), &f.both);
        bind(
            &both_app,
            "BPR",
            vec![alias("remote", "github.com/v/taken")],
        )
        .await;

        let lead_app = app_as_user(f.db, &f.lead);
        let resp = bind(
            &lead_app,
            "APR",
            vec![alias("remote", "github.com/v/taken")],
        )
        .await;

        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = String::from_utf8(raw_body(resp).await).unwrap();
        assert_eq!(body, r#"{"error":"one or more aliases are already bound"}"#);
        assert!(
            !body.contains("BPR"),
            "the refusal must never name the other project: {body}"
        );
    }

    #[tokio::test]
    async fn binding_without_aliases_is_a_400() {
        let f = fixture();
        let app = app_as_user(f.db, &f.lead);

        let resp = bind(&app, "APR", vec![]).await;

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(
            parse_json(resp).await["error"]
                .as_str()
                .unwrap()
                .contains("at least one alias")
        );
    }

    // ── list ─────────────────────────────────────────────────

    #[tokio::test]
    async fn a_viewer_level_member_can_list_the_projects_bindings() {
        let f = fixture();
        let lead_app = app_as_user(f.db.clone(), &f.lead);
        bind(&lead_app, "APR", vec![alias("root", "/mnt/dev/list")]).await;

        let maintainer_app = app_as_user(f.db.clone(), &f.maintainer);
        let resp = json_get(&maintainer_app, &format!("/api/projects/{}/bindings", f.a)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = parse_json(resp).await;
        assert_eq!(body[0]["identities"][0]["value"], "/mnt/dev/list");

        let outsider_app = app_as_user(f.db, &f.outsider);
        assert_eq!(
            json_get(&outsider_app, &format!("/api/projects/{}/bindings", f.a))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }

    // ── delete ───────────────────────────────────────────────

    #[tokio::test]
    async fn a_lead_can_delete_their_projects_binding() {
        let f = fixture();
        let app = app_as_user(f.db, &f.lead);
        let id = parse_json(bind(&app, "APR", vec![alias("root", "/mnt/dev/del")]).await).await
            ["binding"]["id"]
            .as_i64()
            .unwrap();

        let resp = json_delete(&app, &format!("/api/repos/bindings/{id}")).await;
        assert_eq!(resp.status(), StatusCode::OK);

        assert_eq!(
            parse_json(resolve(&app, vec![alias("root", "/mnt/dev/del")]).await).await["resolution"],
            "none"
        );
    }

    #[tokio::test]
    async fn an_admin_can_reclaim_a_binding_they_did_not_make() {
        let f = fixture();
        let lead_app = app_as_user(f.db.clone(), &f.lead);
        let id = parse_json(bind(&lead_app, "APR", vec![alias("root", "/mnt/dev/rec")]).await)
            .await["binding"]["id"]
            .as_i64()
            .unwrap();

        let admin_app = app_as_user(f.db, &f.admin);
        let resp = json_delete(&admin_app, &format!("/api/repos/bindings/{id}")).await;

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn deleting_an_invisible_binding_is_refused_exactly_like_a_missing_one() {
        let f = fixture();
        let lead_app = app_as_user(f.db.clone(), &f.lead);
        let id = parse_json(bind(&lead_app, "APR", vec![alias("root", "/mnt/dev/inv")]).await)
            .await["binding"]["id"]
            .as_i64()
            .unwrap();

        let outsider_app = app_as_user(f.db, &f.outsider);
        let hidden = json_delete(&outsider_app, &format!("/api/repos/bindings/{id}")).await;
        let missing = json_delete(&outsider_app, "/api/repos/bindings/987654").await;

        assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
        assert_eq!(hidden.status(), missing.status());
        assert_eq!(
            raw_body(hidden).await,
            raw_body(missing).await,
            "an invisible binding and a nonexistent one must answer identically"
        );
    }

    // ── merge ────────────────────────────────────────────────

    #[tokio::test]
    async fn merging_requires_lead_on_both_projects() {
        let f = fixture();
        let both_app = app_as_user(f.db.clone(), &f.both);
        let from = parse_json(bind(&both_app, "BPR", vec![alias("root", "/mnt/dev/m-from")]).await)
            .await["binding"]["id"]
            .as_i64()
            .unwrap();
        let into = parse_json(
            bind(
                &both_app,
                "APR",
                vec![alias("remote", "github.com/v/m-into")],
            )
            .await,
        )
        .await["binding"]["id"]
            .as_i64()
            .unwrap();

        // `lead` leads A but not B, so the source binding is not theirs to move.
        let lead_app = app_as_user(f.db, &f.lead);
        let refused = json_post(
            &lead_app,
            "/api/repos/merge",
            serde_json::json!({ "from": from, "into": into }),
        )
        .await;
        let missing = json_post(
            &lead_app,
            "/api/repos/merge",
            serde_json::json!({ "from": 987_654, "into": into }),
        )
        .await;

        assert_eq!(refused.status(), StatusCode::NOT_FOUND);
        assert_eq!(refused.status(), missing.status());
        assert_eq!(raw_body(refused).await, raw_body(missing).await);
    }

    #[tokio::test]
    async fn merging_moves_the_aliases_onto_the_surviving_binding() {
        let f = fixture();
        let app = app_as_user(f.db, &f.both);
        let from = parse_json(bind(&app, "BPR", vec![alias("root", "/mnt/dev/mv")]).await).await
            ["binding"]["id"]
            .as_i64()
            .unwrap();
        let into = parse_json(bind(&app, "APR", vec![alias("remote", "github.com/v/mv")]).await)
            .await["binding"]["id"]
            .as_i64()
            .unwrap();

        let resp = json_post(
            &app,
            "/api/repos/merge",
            serde_json::json!({ "from": from, "into": into }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        // The conflict that motivated the merge now resolves to one binding.
        let body = parse_json(
            resolve(
                &app,
                vec![
                    alias("root", "/mnt/dev/mv"),
                    alias("remote", "github.com/v/mv"),
                ],
            )
            .await,
        )
        .await;
        assert_eq!(body["resolution"], "one");
        assert_eq!(body["binding_id"], into);
        assert_eq!(body["project"]["identifier"], "APR");
    }

    // ── rate limit ───────────────────────────────────────────

    #[tokio::test]
    async fn the_thirty_first_binding_change_in_an_hour_is_refused() {
        let f = fixture();
        let app = app_as_user(f.db, &f.lead);

        for n in 0..30 {
            let resp = bind(
                &app,
                "APR",
                vec![alias("root", &format!("/mnt/dev/rl-{n}"))],
            )
            .await;
            assert_eq!(resp.status(), StatusCode::OK, "attempt {n} must be allowed");
        }

        let resp = bind(&app, "APR", vec![alias("root", "/mnt/dev/rl-over")]).await;

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(
            parse_json(resp).await["error"]
                .as_str()
                .unwrap()
                .contains("too many repo binding changes")
        );
    }
}
