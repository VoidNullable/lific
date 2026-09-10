//! LIF-465 / LIF-471: the anonymous read surface for a published project.
//!
//! A router of its own rather than routes added to [`super::router`], because
//! the separation is the boundary:
//!
//! * it is merged *after* `server::build_app` layers the auth middleware onto
//!   the authenticated router, so it is outside that middleware rather than
//!   carved out of it by path;
//! * it has no identity extension at all, so a handler here cannot consult
//!   `authz.rs` even by accident. That rules out the two shortcuts the
//!   contract forbids: turning auth off, and impersonating a Viewer;
//! * it registers only `get`, so every other method is a 405 from axum's own
//!   method router rather than from a dozen handlers remembering to check.
//!
//! ## The mirror rule (LIF-471)
//!
//! Every route here is a private route with `/api` replaced by
//! `/public/api/projects/{project}`, answering with the same JSON shape and
//! the same paging headers, minus what [`crate::db::queries::public`] scrubs.
//! That is what lets the web client run its ordinary components against this
//! surface with one path rewrite and no bearer token:
//!
//! | private                          | public                                                  |
//! |----------------------------------|---------------------------------------------------------|
//! | `GET /api/projects/{id}`         | `GET /public/api/projects/{project}`                    |
//! | `GET /api/projects/{id}/index`   | `GET /public/api/projects/{project}/index`              |
//! | `GET /api/projects/{id}/changes` | `GET /public/api/projects/{project}/changes`            |
//! | `GET /api/modules?project_id=`   | `GET /public/api/projects/{project}/modules`            |
//! | `GET /api/labels?project_id=`    | `GET /public/api/projects/{project}/labels`             |
//! | `GET /api/folders?project_id=`   | `GET /public/api/projects/{project}/folders`            |
//! | `GET /api/issues/resolve/{ident}`| `GET /public/api/projects/{project}/issues/resolve/{ident}` |
//! | `GET /api/issues/{id}`           | `GET /public/api/projects/{project}/issues/{id}`        |
//! | `GET /api/issues/{id}/comments`  | `GET /public/api/projects/{project}/issues/{id}/comments` |
//! | `GET /api/pages/{id}`            | `GET /public/api/projects/{project}/pages/{id}`         |
//! | `GET /api/pages/{id}/comments`   | `GET /public/api/projects/{project}/pages/{id}/comments` |
//! | `GET /api/attachments?...`       | `GET /public/api/projects/{project}/attachments?...`    |
//! | `GET /api/attachments/{id}`      | `GET /public/api/projects/{project}/attachments/{id}`   |
//! | `.../{id}/thumbnail`, `/preview` | same, under the project                                  |
//!
//! Authorization is the `is_public = 1` predicate plus project ownership
//! inside every query in [`crate::db::queries::public`], evaluated in one read
//! snapshot per request. Each read re-evaluates it, so unpublishing closes all
//! of them on the next request.
//!
//! Caching is off everywhere: a published project can be unpublished, and a
//! cached response would outlive that decision.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    RequestExt, Router,
    body::Body,
    extract::{ConnectInfo, Extension, Path, Query, State},
    http::{HeaderMap, HeaderValue, Request, Response, StatusCode, header},
    middleware::{self, Next},
    response::IntoResponse,
    routing::get,
};
use serde::Deserialize;
use tokio::io::AsyncReadExt;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::db::models::{
    Attachment, AttachmentEntity, ChangesPage, Comment, Folder, IndexSnapshot, Issue, Label,
    Module, Page, Project,
};
use crate::db::queries::comments::CommentParent;
use crate::db::{DbPool, queries, queries::public as q};
use crate::error::LificError;
use crate::ratelimit::{self, IpNetwork, RateLimiter};
use crate::storage::{self, AttachmentStore};

use super::comments::{ListCommentsQuery, paging_headers};
use super::with_read;

/// The single answer for "no such published thing here".
///
/// A private project, a nonexistent one, one unpublished a second ago, an
/// issue in the trash and an attachment belonging to another project all
/// produce this, byte for byte. Anything more specific is an oracle.
const NOT_FOUND: &str = "not found";

fn not_found() -> LificError {
    LificError::NotFound(NOT_FOUND.into())
}

// ── Anonymous load bounds ────────────────────────────────────
//
// These are the only routes with no credential behind them, so they carry two
// limits that fail differently: a per-IP rate limit stops one host looping,
// and a concurrency permit stops many hosts occupying every read connection
// and starving the authenticated app.

/// Public reads allowed per client IP per minute. A person opening the list,
/// an issue, its comments and a body of images stays well under it.
const PUBLIC_READS_PER_MINUTE: usize = 240;

/// Public requests that may be in flight at once, across every visitor.
///
/// Below the read-pool size on purpose, so anonymous traffic cannot crowd the
/// authenticated instance out of the database. Exceeding it is a fast 503 with
/// a `Retry-After`, not a queue.
const PUBLIC_CONCURRENCY: usize = 4;

/// Largest attachment a public thumbnail or preview will be *derived* from.
/// The download streams, but a thumbnail decodes the whole image and a
/// preview parses the whole archive in memory; four anonymous requests for a
/// 500 MB import must not be able to do that at once. A thumbnail already on
/// disk is served whatever the original's size.
const PUBLIC_DERIVE_MAX_BYTES: i64 = 32 * 1024 * 1024;

/// Bytes read from disk per chunk when streaming an attachment. The download
/// is streamed rather than buffered because an imported attachment can be
/// hundreds of megabytes and these callers are anonymous and concurrent; this
/// is the only allocation the size of the file affects.
const DOWNLOAD_CHUNK_BYTES: usize = 64 * 1024;

pub struct PublicReadLimiter(pub RateLimiter);

pub struct PublicConcurrency(pub Arc<Semaphore>);

/// Build the anonymous router.
///
/// `store` is passed in rather than derived from the config so the public
/// download path shares the instance the authenticated one uses, including its
/// operation lock. `trusted_proxies` is the list the authenticated router
/// rate-limits with, and matters here for the same reason: behind a proxy
/// every request arrives from one address, and in front of one a
/// client-supplied `X-Forwarded-For` would mint a fresh bucket per request.
pub fn router(db: DbPool, store: AttachmentStore, trusted_proxies: Arc<[IpNetwork]>) -> Router {
    router_with_bounds(
        db,
        store,
        trusted_proxies,
        Arc::new(PublicReadLimiter(RateLimiter::new(
            PUBLIC_READS_PER_MINUTE,
            std::time::Duration::from_secs(60),
        ))),
        Arc::new(PublicConcurrency(Arc::new(Semaphore::new(
            PUBLIC_CONCURRENCY,
        )))),
    )
}

/// [`router`] with the two bounds supplied, so a test can hold the same
/// limiter and semaphore the middleware uses.
fn router_with_bounds(
    db: DbPool,
    store: AttachmentStore,
    trusted_proxies: Arc<[IpNetwork]>,
    limiter: Arc<PublicReadLimiter>,
    concurrency: Arc<PublicConcurrency>,
) -> Router {
    const P: &str = "/public/api/projects/{project}";
    Router::new()
        .route(P, get(get_project))
        .route(&format!("{P}/index"), get(get_index))
        .route(&format!("{P}/changes"), get(get_changes))
        .route(&format!("{P}/modules"), get(list_modules))
        .route(&format!("{P}/labels"), get(list_labels))
        .route(&format!("{P}/folders"), get(list_folders))
        .route(
            &format!("{P}/issues/resolve/{{identifier}}"),
            get(resolve_issue),
        )
        .route(&format!("{P}/issues/{{id}}"), get(get_issue))
        .route(
            &format!("{P}/issues/{{id}}/comments"),
            get(list_issue_comments),
        )
        .route(&format!("{P}/pages/{{id}}"), get(get_page))
        .route(
            &format!("{P}/pages/{{id}}/comments"),
            get(list_page_comments),
        )
        .route(&format!("{P}/attachments"), get(list_entity_attachments))
        .route(&format!("{P}/attachments/{{id}}"), get(download_attachment))
        .route(
            &format!("{P}/attachments/{{id}}/thumbnail"),
            get(attachment_thumbnail),
        )
        .route(
            &format!("{P}/attachments/{{id}}/preview"),
            get(attachment_preview),
        )
        // Anything else under the prefix is the same JSON 404 as a private
        // project, with the same headers. Without this the SPA fallback would
        // answer an unknown public API path with 200 and a page of HTML.
        .route("/public/api/{*rest}", axum::routing::any(unknown))
        // axum's layer order is the reverse of reading order: the last layer
        // added is outermost. Bounds are added first so they sit inside the
        // headers layer, and a 429 or 503 they short-circuit with still leaves
        // through it. A refusal is a public response too.
        .layer(middleware::from_fn(enforce_load_bounds))
        .layer(middleware::from_fn(no_store_headers))
        .layer(Extension(limiter))
        .layer(Extension(concurrency))
        .layer(Extension(trusted_proxies))
        .layer(Extension(store))
        .with_state(db)
}
/// Stamp every public response, including 404s, 405s and errors, with the
/// caching and browser-hardening headers.
///
/// A layer rather than per handler so an error return or a method rejection
/// cannot skip it. `insert` overwrites: the attachment download would
/// otherwise keep an `immutable` year-long cache for a file that can be
/// unpublished tomorrow.
async fn no_store_headers(request: Request<Body>, next: Next) -> Response<Body> {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    for (name, value) in [
        (header::CACHE_CONTROL, "no-store, no-cache, must-revalidate"),
        (header::PRAGMA, "no-cache"),
        (header::EXPIRES, "0"),
        // A public body can carry links a reader clicks; the destination must
        // not learn which instance, let alone which issue, they came from.
        (header::REFERRER_POLICY, "no-referrer"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::X_FRAME_OPTIONS, "DENY"),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    headers.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
    response
}

/// Refuse a public request that is over the per-IP rate or would exceed the
/// concurrency budget, before it reaches a database connection.
///
/// The permit is held in a local across `next.run`, so it covers the whole
/// handler. It is then handed to the response so it also covers a streaming
/// body, which is polled long after this function returns: the download
/// handler moves it into the stream, and for a buffered response it is dropped
/// here, which is correct because the body is already in memory.
///
/// Both extensions are read as `Option` so a router assembled without them is
/// unbounded rather than broken; `router` above always installs both.
async fn enforce_load_bounds(mut request: Request<Body>, next: Next) -> Response<Body> {
    // Through the extractor, not `extensions().get()`: a test router supplies
    // the peer with `MockConnectInfo`, which stores itself rather than a
    // `ConnectInfo`, and only the extractor looks for both.
    let peer = request
        .extract_parts::<ConnectInfo<SocketAddr>>()
        .await
        .ok()
        .map(|ConnectInfo(addr)| addr);

    if let Some(limiter) = request
        .extensions()
        .get::<Arc<PublicReadLimiter>>()
        .cloned()
    {
        let key = format!("public:{}", public_client_ip(peer, &request));
        if !limiter.0.check(&key) {
            return LificError::TooManyRequests(
                "too many requests; slow down and try again shortly".into(),
            )
            .into_response();
        }
    }

    let permit = match request
        .extensions()
        .get::<Arc<PublicConcurrency>>()
        .cloned()
    {
        Some(concurrency) => match Arc::clone(&concurrency.0).try_acquire_owned() {
            Ok(permit) => Some(permit),
            Err(_) => {
                return LificError::Unavailable("public view is busy".into()).into_response();
            }
        },
        None => None,
    };

    // The handler extracts what it needs and drops the request before it runs,
    // so a permit parked only in the extensions would be released immediately.
    // `held` stays in this frame across `next.run`, and the handler gets a
    // clone of the same `Arc`: a streaming body can take ownership out of it,
    // and anything else leaves it here to be dropped when this returns.
    let held = permit.map(|permit| HeldPermit(Arc::new(std::sync::Mutex::new(Some(permit)))));
    if let Some(held) = held.clone() {
        request.extensions_mut().insert(held);
    }
    let response = next.run(request).await;
    drop(held);
    response
}

/// The concurrency permit for the request in flight, offered to a handler that
/// returns a streaming body so the permit outlives this middleware.
///
/// A handler that does not take it leaves the permit here, and it is released
/// when the request extensions are dropped after the response is built, which
/// is the right moment for a buffered body.
#[derive(Clone)]
struct HeldPermit(Arc<std::sync::Mutex<Option<OwnedSemaphorePermit>>>);

impl HeldPermit {
    fn take(&self) -> Option<OwnedSemaphorePermit> {
        self.0.lock().ok().and_then(|mut slot| slot.take())
    }
}

/// The rate-limit key: the client IP as [`ratelimit::client_ip`] resolves it,
/// which believes a forwarding header only when the peer is a trusted proxy.
/// A request with no peer keys on one shared bucket, so the failure direction
/// is "shared limit", not "no limit".
fn public_client_ip(peer: Option<SocketAddr>, request: &Request<Body>) -> String {
    let Some(peer) = peer else {
        return "unknown".into();
    };
    let empty: Arc<[IpNetwork]> = Arc::from(Vec::new());
    let trusted = request
        .extensions()
        .get::<Arc<[IpNetwork]>>()
        .cloned()
        .unwrap_or(empty);
    let headers: &HeaderMap = request.headers();
    ratelimit::client_ip(peer.ip(), headers, &trusted)
}

// ── Handlers ─────────────────────────────────────────────────

/// The catch-all for the prefix. A path that names no route here is a 404,
/// never a 405 and never the SPA's `index.html`.
async fn unknown() -> LificError {
    not_found()
}
//
// Every handler resolves the published project first, inside one read
// snapshot, and does everything else against that `Project` in the same
// snapshot. A private or missing project is `not_found()` before any other
// table is touched.

/// Run `f` against the published project named in the path, in one snapshot.
fn with_public<T>(
    db: &DbPool,
    identifier: &str,
    f: impl FnOnce(&rusqlite::Connection, &Project) -> Result<T, LificError>,
) -> Result<T, LificError> {
    with_read(db, |conn| {
        let tx = conn.unchecked_transaction()?;
        let project = q::public_project(&tx, identifier)?.ok_or_else(not_found)?;
        f(&tx, &project)
    })
}

type PathProject = Path<String>;

/// A numeric row id from the path. Anything else is a 404 rather than axum's
/// 400: the answer to "is there anything public at this address" is no.
fn row_id(raw: &str) -> Result<i64, LificError> {
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(not_found());
    }
    raw.parse().map_err(|_| not_found())
}

/// `GET /public/api/projects/{project}`
async fn get_project(
    State(db): State<DbPool>,
    Path(project): PathProject,
) -> Result<axum::Json<Project>, LificError> {
    let project = with_public(&db, &project, |_, project| Ok(project.clone()))?;
    Ok(axum::Json(project))
}

/// `GET /public/api/projects/{project}/index`
async fn get_index(
    State(db): State<DbPool>,
    Path(project): PathProject,
) -> Result<axum::Json<IndexSnapshot>, LificError> {
    let snapshot = with_public(&db, &project, |conn, project| {
        q::public_index(conn, project)
    })?;
    Ok(axum::Json(snapshot))
}

/// `?since=&limit=`, mirroring `api::sync::ChangesQuery`.
#[derive(Debug, Default, Deserialize)]
struct ChangesQuery {
    since: Option<i64>,
    limit: Option<i64>,
}

/// `GET /public/api/projects/{project}/changes`
async fn get_changes(
    State(db): State<DbPool>,
    Path(project): PathProject,
    Query(query): Query<ChangesQuery>,
) -> Result<axum::Json<ChangesPage>, LificError> {
    let since = query.since.unwrap_or(0).max(0);
    let limit = queries::changes::clamp_changes_limit(query.limit);
    let page = with_public(&db, &project, |conn, project| {
        q::public_changes(conn, project, since, limit)
    })?;
    Ok(axum::Json(page))
}

/// `GET /public/api/projects/{project}/modules`
async fn list_modules(
    State(db): State<DbPool>,
    Path(project): PathProject,
) -> Result<axum::Json<Vec<Module>>, LificError> {
    Ok(axum::Json(with_public(&db, &project, |conn, project| {
        q::public_modules(conn, project)
    })?))
}

/// `GET /public/api/projects/{project}/labels`
async fn list_labels(
    State(db): State<DbPool>,
    Path(project): PathProject,
) -> Result<axum::Json<Vec<Label>>, LificError> {
    Ok(axum::Json(with_public(&db, &project, |conn, project| {
        q::public_labels(conn, project)
    })?))
}

/// `GET /public/api/projects/{project}/folders`
async fn list_folders(
    State(db): State<DbPool>,
    Path(project): PathProject,
) -> Result<axum::Json<Vec<Folder>>, LificError> {
    Ok(axum::Json(with_public(&db, &project, |conn, project| {
        q::public_folders(conn, project)
    })?))
}

/// `GET /public/api/projects/{project}/issues/resolve/{identifier}`
async fn resolve_issue(
    State(db): State<DbPool>,
    Path((project, identifier)): Path<(String, String)>,
) -> Result<axum::Json<Issue>, LificError> {
    let issue = with_public(&db, &project, |conn, project| {
        q::public_issue_by_identifier(conn, project, &identifier)?.ok_or_else(not_found)
    })?;
    Ok(axum::Json(issue))
}

/// `GET /public/api/projects/{project}/issues/{id}`
async fn get_issue(
    State(db): State<DbPool>,
    Path((project, id)): Path<(String, String)>,
) -> Result<axum::Json<Issue>, LificError> {
    let id = row_id(&id)?;
    let issue = with_public(&db, &project, |conn, project| {
        q::public_issue(conn, project, id)?.ok_or_else(not_found)
    })?;
    Ok(axum::Json(issue))
}

/// `GET /public/api/projects/{project}/pages/{id}`
async fn get_page(
    State(db): State<DbPool>,
    Path((project, id)): Path<(String, String)>,
) -> Result<axum::Json<Page>, LificError> {
    let id = row_id(&id)?;
    let page = with_public(&db, &project, |conn, project| {
        q::public_page(conn, project, id)?.ok_or_else(not_found)
    })?;
    Ok(axum::Json(page))
}

/// Shared by the issue and page comment routes. Same query contract and the
/// same paging headers as the private route, so the client's comment thread
/// does not need to know which surface it is reading from.
fn list_comments_for(
    db: &DbPool,
    project: &str,
    parent: CommentParent,
    query: &ListCommentsQuery,
) -> Result<(HeaderMap, axum::Json<Vec<Comment>>), LificError> {
    let cursor = query.cursor()?;
    let (limit, offset) = queries::page(query.limit, query.offset);
    let page = with_public(db, project, |conn, project| {
        if !q::public_parent_exists(conn, project, parent)? {
            return Err(not_found());
        }
        // `author` is accepted and ignored: filtering by username would
        // confirm which usernames exist.
        q::public_comments(
            conn,
            parent,
            query.order.as_deref(),
            Some(limit),
            Some(offset),
            cursor.as_ref(),
        )
    })?;
    let headers = paging_headers(
        &page,
        cursor.is_some(),
        query.order.as_deref() == Some("desc"),
    );
    Ok((headers, axum::Json(page.items)))
}

/// `GET /public/api/projects/{project}/issues/{id}/comments`
async fn list_issue_comments(
    State(db): State<DbPool>,
    Path((project, id)): Path<(String, String)>,
    Query(query): Query<ListCommentsQuery>,
) -> Result<(HeaderMap, axum::Json<Vec<Comment>>), LificError> {
    list_comments_for(&db, &project, CommentParent::Issue(row_id(&id)?), &query)
}

/// `GET /public/api/projects/{project}/pages/{id}/comments`
async fn list_page_comments(
    State(db): State<DbPool>,
    Path((project, id)): Path<(String, String)>,
    Query(query): Query<ListCommentsQuery>,
) -> Result<(HeaderMap, axum::Json<Vec<Comment>>), LificError> {
    list_comments_for(&db, &project, CommentParent::Page(row_id(&id)?), &query)
}

/// `?entity_type=issue&entity_id=42`, mirroring the private route.
#[derive(Debug, Deserialize)]
struct ListForEntityQuery {
    entity_type: String,
    entity_id: i64,
}

/// `GET /public/api/projects/{project}/attachments`
async fn list_entity_attachments(
    State(db): State<DbPool>,
    Path(project): PathProject,
    Query(query): Query<ListForEntityQuery>,
) -> Result<axum::Json<Vec<Attachment>>, LificError> {
    // An unknown entity type is a 404 here, not a 400: the answer to "is there
    // anything public at this address" is no, and nothing more.
    let entity: AttachmentEntity = query.entity_type.parse().map_err(|_| not_found())?;
    let items = with_public(&db, &project, |conn, project| {
        q::public_entity_attachments(conn, project, entity, query.entity_id)?.ok_or_else(not_found)
    })?;
    Ok(axum::Json(items))
}

/// The attachment named in the path, if the project in the path publishes it.
fn public_blob(db: &DbPool, project: &str, id: &str) -> Result<Attachment, LificError> {
    let id = row_id(id)?;
    with_public(db, project, |conn, project| {
        q::public_attachment(conn, project, id)?.ok_or_else(not_found)
    })
}

/// `GET /public/api/projects/{project}/attachments/{id}`
///
/// Re-authorized from scratch against the project in the path: an id lifted
/// from a private project, orphaned, or whose parent was deleted a moment ago
/// all answer with the same 404 as one that never existed.
///
/// Headers follow the authenticated download's rules, which matter more here:
/// `nosniff` so a browser cannot re-guess a hostile file into something
/// scriptable, an `attachment` disposition for everything that is not a plain
/// raster or media container (SVG is a document that can run script, so it is
/// served as `application/octet-stream`), and a `default-src 'none'; sandbox`
/// CSP as the backstop.
///
/// The bounded producer holds its permit until completion, disconnect or
/// timeout, including when the client stops polling the response body.
async fn download_attachment(
    State(db): State<DbPool>,
    Extension(store): Extension<AttachmentStore>,
    permit: Option<Extension<HeldPermit>>,
    Path((project, id)): Path<(String, String)>,
) -> Result<Response<Body>, LificError> {
    let blob = public_blob(&db, &project, &id)?;

    // The store lock is held only to open the handle. Streaming under it would
    // block a dump or restore for as long as a reader takes; the fd stays
    // valid once opened.
    let (file, len) = store
        .try_with_lock(|store| store.open_blob(&blob.sha256))?
        .ok_or_else(AttachmentStore::busy_error)?;

    let inline_safe = storage::is_inline_safe_mime(&blob.mime);
    let content_type =
        if blob.mime == "image/svg+xml" || !storage::ALLOWED_MIMES.contains(&blob.mime.as_str()) {
            "application/octet-stream"
        } else {
            &blob.mime
        };
    let disposition = if inline_safe {
        format!("inline; filename=\"{}\"", header_safe(&blob.filename))
    } else {
        format!("attachment; filename=\"{}\"", header_safe(&blob.filename))
    };

    let held = permit.and_then(|Extension(held)| held.take());
    let body = download_body(
        file,
        held,
        std::time::Duration::from_secs(15),
        std::time::Duration::from_secs(5 * 60),
    );

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, len)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; sandbox",
        )
        .header(header::CONTENT_DISPOSITION, disposition)
        .body(body)
        .map_err(|e| LificError::Internal(format!("build response: {e}")))
        .map(IntoResponse::into_response)
}

/// `GET /public/api/projects/{project}/attachments/{id}/thumbnail`
///
/// Same lazy generation as the private route; same 404 for "not a raster" as
/// for "not public", so the response never says which.
async fn attachment_thumbnail(
    State(db): State<DbPool>,
    Extension(store): Extension<AttachmentStore>,
    Path((project, id)): Path<(String, String)>,
) -> Result<Response<Body>, LificError> {
    let blob = public_blob(&db, &project, &id)?;
    if !storage::is_raster_mime(&blob.mime) {
        return Err(not_found());
    }
    let thumb = match store.read_thumb(&blob.sha256)? {
        Some(bytes) => bytes,
        None => {
            if blob.size_bytes > PUBLIC_DERIVE_MAX_BYTES {
                return Err(not_found());
            }
            let source = store.read(&blob.sha256)?;
            match storage::generate_thumbnail(&source) {
                Ok(Some(bytes)) => {
                    if let Err(e) = store.write_thumb(&blob.sha256, &bytes) {
                        tracing::warn!(error = %e, "failed to cache public attachment thumbnail");
                    }
                    bytes
                }
                Ok(None) | Err(_) => return Err(not_found()),
            }
        }
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/webp")
        .header(header::CONTENT_LENGTH, thumb.len())
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; sandbox",
        )
        .header(
            header::CONTENT_DISPOSITION,
            format!("inline; filename=\"{}.webp\"", header_safe(&blob.filename)),
        )
        .body(Body::from(thumb))
        .map_err(|e| LificError::Internal(format!("build response: {e}")))
}

/// `GET /public/api/projects/{project}/attachments/{id}/preview`
async fn attachment_preview(
    State(db): State<DbPool>,
    Extension(store): Extension<AttachmentStore>,
    Path((project, id)): Path<(String, String)>,
) -> Result<axum::Json<crate::preview::Preview>, LificError> {
    let blob = public_blob(&db, &project, &id)?;
    if blob.size_bytes > PUBLIC_DERIVE_MAX_BYTES {
        return Err(not_found());
    }
    let bytes = store.read(&blob.sha256)?;
    Ok(axum::Json(crate::preview::preview_bytes(&bytes)?))
}

fn download_body(
    file: std::fs::File,
    permit: Option<OwnedSemaphorePermit>,
    idle_timeout: std::time::Duration,
    max_duration: std::time::Duration,
) -> Body {
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    let (terminal_sender, terminal_receiver) = tokio::sync::oneshot::channel();
    // A producer deadline runs even when the client stops polling the body.
    tokio::spawn(async move {
        let _permit = permit;
        let mut file = tokio::fs::File::from_std(file);
        let mut buffer = vec![0; DOWNLOAD_CHUNK_BYTES];
        let result = tokio::time::timeout(max_duration, async {
            loop {
                let read = file.read(&mut buffer).await?;
                if read == 0 {
                    return Ok(());
                }
                let chunk = axum::body::Bytes::copy_from_slice(&buffer[..read]);
                match tokio::time::timeout(idle_timeout, sender.send(chunk)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) => return Ok(()),
                    Err(_) => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "public download idle timeout",
                        ));
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "public download deadline exceeded",
            ))
        });
        let _ = terminal_sender.send(result);
    });
    super::export::stream_body(receiver, terminal_receiver)
}

/// Strip anything that could break out of a quoted `Content-Disposition`
/// filename. Same rule as `api::attachments`, restated so this module's header
/// handling can be audited without leaving the file.
fn header_safe(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control() && !matches!(c, '"' | '\\'))
        .take(200)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::*;
    use axum::extract::connect_info::MockConnectInfo;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use rusqlite::params;
    use tower::ServiceExt;

    struct Fixture {
        db: DbPool,
        app: Router,
        _store_guard: tempfile::TempDir,
        store: AttachmentStore,
        /// The router's own semaphore, so a test can observe permits rather
        /// than infer them from status codes.
        concurrency: Arc<PublicConcurrency>,
    }

    /// The peer every fixture request appears to come from. Inside the trusted
    /// range below, so the forwarding-header tests can exercise the path a
    /// reverse-proxy deployment actually takes.
    fn peer() -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], 4242))
    }

    fn trusted() -> Arc<[IpNetwork]> {
        Arc::from(
            ratelimit::parse_trusted_proxies(&["127.0.0.0/8".into()])
                .expect("test trusted proxy range"),
        )
    }

    fn fixture() -> Fixture {
        let db = crate::db::open_memory().expect("test db");
        let tmp = tempfile::tempdir().expect("attachment tempdir");
        let store = AttachmentStore::new(tmp.path().to_path_buf());
        {
            let conn = db.write().unwrap();
            conn.execute(
                "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot)
                 VALUES ('owner', 'owner@test.local', 'x', 'Owner', 1, 0)",
                [],
            )
            .unwrap();
        }
        // MockConnectInfo supplies the peer address `enforce_load_bounds`
        // keys the rate limit on; without it every fixture would share the
        // "unknown" bucket and the limit tests would interfere with the rest.
        let concurrency = Arc::new(PublicConcurrency(Arc::new(Semaphore::new(
            PUBLIC_CONCURRENCY,
        ))));
        let app = router_with_bounds(
            db.clone(),
            store.clone(),
            trusted(),
            Arc::new(PublicReadLimiter(RateLimiter::new(
                PUBLIC_READS_PER_MINUTE,
                std::time::Duration::from_secs(60),
            ))),
            Arc::clone(&concurrency),
        )
        .layer(MockConnectInfo(peer()));
        Fixture {
            db,
            app,
            _store_guard: tmp,
            store,
            concurrency,
        }
    }

    impl Fixture {
        fn seed_project(&self, identifier: &str, public: bool) -> i64 {
            let conn = self.db.write().unwrap();
            let project = crate::db::queries::create_project(
                &conn,
                &CreateProject {
                    name: format!("{identifier} project"),
                    identifier: identifier.into(),
                    lead_user_id: Some(1),
                    ..Default::default()
                },
            )
            .unwrap();
            if public {
                conn.execute(
                    "UPDATE projects SET is_public = 1 WHERE id = ?1",
                    [project.id],
                )
                .unwrap();
            }
            project.id
        }

        fn seed_issue(&self, project_id: i64, title: &str, description: &str) -> i64 {
            let conn = self.db.write().unwrap();
            crate::db::queries::create_issue(
                &conn,
                &CreateIssue {
                    project_id,
                    title: title.into(),
                    description: description.into(),
                    source: Some(format!("github:acme/secret#{title}")),
                    ..Default::default()
                },
            )
            .unwrap()
            .id
        }

        fn seed_comment(&self, parent: CommentParent, content: &str) -> i64 {
            let conn = self.db.write().unwrap();
            crate::db::queries::comments::create_comment_with_mentions(
                &conn,
                parent,
                None,
                CommentActor {
                    user_id: 1,
                    is_admin: true,
                },
                AttachmentActor::TrustedLocal,
                content,
                false,
            )
            .unwrap()
            .id
        }

        fn delete_issue(&self, issue_id: i64) {
            let conn = self.db.write().unwrap();
            crate::db::queries::delete_issue(&conn, issue_id).unwrap();
        }

        fn delete_page(&self, page_id: i64) {
            let conn = self.db.write().unwrap();
            crate::db::queries::delete_page(&conn, page_id).unwrap();
        }

        fn seed_page(&self, project_id: Option<i64>, title: &str, content: &str) -> i64 {
            let conn = self.db.write().unwrap();
            crate::db::queries::create_page(
                &conn,
                &CreatePage {
                    project_id,
                    title: title.into(),
                    content: content.into(),
                    ..Default::default()
                },
            )
            .unwrap()
            .id
        }

        /// Upload real bytes and link them to `entity`.
        fn seed_attachment(&self, entity: AttachmentEntity, entity_id: i64, name: &str) -> i64 {
            let bytes = format!("bytes for {name}").into_bytes();
            let sha = self.store.write(&bytes).unwrap();
            let conn = self.db.write().unwrap();
            let attachment = crate::db::queries::attachments::create_attachment(
                &conn,
                &sha,
                name,
                "application/pdf",
                bytes.len() as i64,
                Some(1),
            )
            .unwrap();
            crate::db::queries::attachments::link_attachment(
                &conn,
                attachment.id,
                entity,
                entity_id,
            )
            .unwrap();
            attachment.id
        }

        /// An attachment with real bytes and no link rows at all.
        fn seed_orphan_attachment(&self) -> i64 {
            let bytes = b"orphan bytes".to_vec();
            let sha = self.store.write(&bytes).unwrap();
            let conn = self.db.write().unwrap();
            crate::db::queries::attachments::create_attachment(
                &conn,
                &sha,
                "orphan.pdf",
                "application/pdf",
                bytes.len() as i64,
                None,
            )
            .unwrap()
            .id
        }

        fn unpublish(&self, identifier: &str) {
            let conn = self.db.write().unwrap();
            conn.execute(
                "UPDATE projects SET is_public = 0 WHERE identifier = ?1",
                [identifier],
            )
            .unwrap();
        }

        async fn get(&self, uri: &str) -> axum::response::Response {
            self.request("GET", uri).await
        }

        /// A request that arrives through the trusted proxy on behalf of
        /// `client`, the shape a Funnel/nginx deployment produces.
        async fn get_as(&self, client: &str, uri: &str) -> axum::response::Response {
            self.app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(uri)
                        .header("x-forwarded-for", client)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
        }

        async fn request(&self, method: &str, uri: &str) -> axum::response::Response {
            self.app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
        }
    }

    async fn body_string(response: axum::response::Response) -> String {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8_lossy(&bytes).to_string()
    }

    async fn body_json(response: axum::response::Response) -> serde_json::Value {
        serde_json::from_str(&body_string(response).await).unwrap()
    }

    /// Every public route, for the tests that walk the whole surface.
    fn all_routes(issue_id: i64, page_id: i64, attachment_id: i64) -> Vec<String> {
        vec![
            "/public/api/projects/PUB".into(),
            "/public/api/projects/PUB/index".into(),
            "/public/api/projects/PUB/changes".into(),
            "/public/api/projects/PUB/modules".into(),
            "/public/api/projects/PUB/labels".into(),
            "/public/api/projects/PUB/folders".into(),
            "/public/api/projects/PUB/issues/resolve/PUB-1".into(),
            format!("/public/api/projects/PUB/issues/{issue_id}"),
            format!("/public/api/projects/PUB/issues/{issue_id}/comments"),
            format!("/public/api/projects/PUB/pages/{page_id}"),
            format!("/public/api/projects/PUB/pages/{page_id}/comments"),
            format!("/public/api/projects/PUB/attachments?entity_type=issue&entity_id={issue_id}"),
            format!("/public/api/projects/PUB/attachments/{attachment_id}"),
            format!("/public/api/projects/PUB/attachments/{attachment_id}/preview"),
        ]
    }

    // ── The happy path ───────────────────────────────────────

    #[tokio::test]
    async fn an_anonymous_visitor_reads_a_published_project() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let issue = f.seed_issue(project, "Public issue", "The body");
        f.seed_comment(CommentParent::Issue(issue), "A public comment");
        let page = f.seed_page(Some(project), "Public page", "Page body");
        f.seed_comment(CommentParent::Page(page), "A page comment");

        let json = body_json(f.get("/public/api/projects/PUB").await).await;
        assert_eq!(json["identifier"], "PUB");
        assert_eq!(json["id"], project);
        assert_eq!(json["is_public"], true);

        let json = body_json(f.get("/public/api/projects/PUB/index").await).await;
        assert_eq!(json["issues"][0]["identifier"], "PUB-1");
        assert_eq!(json["pages"][0]["title"], "Public page");

        let json = body_json(f.get("/public/api/projects/PUB/issues/resolve/PUB-1").await).await;
        assert_eq!(json["description"], "The body");
        assert_eq!(json["id"], issue);

        let json = body_json(
            f.get(&format!("/public/api/projects/PUB/issues/{issue}"))
                .await,
        )
        .await;
        assert_eq!(json["title"], "Public issue");

        let response = f
            .get(&format!("/public/api/projects/PUB/issues/{issue}/comments"))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[super::super::comments::HAS_MORE_HEADER],
            "false"
        );
        let json = body_json(response).await;
        assert_eq!(json[0]["content"], "A public comment");
        assert_eq!(json[0]["author_display_name"], "Owner");

        let json = body_json(
            f.get(&format!("/public/api/projects/PUB/pages/{page}"))
                .await,
        )
        .await;
        assert_eq!(json["content"], "Page body");
        let json = body_json(
            f.get(&format!("/public/api/projects/PUB/pages/{page}/comments"))
                .await,
        )
        .await;
        assert_eq!(json[0]["content"], "A page comment");
    }

    /// The identifier column is NOCASE everywhere else in Lific; a shared
    /// link that lost its capitals must still resolve.
    #[tokio::test]
    async fn the_project_identifier_is_case_insensitive() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        f.seed_issue(project, "Public issue", "");
        assert_eq!(
            f.get("/public/api/projects/pub/issues/resolve/pub-1")
                .await
                .status(),
            StatusCode::OK
        );
    }

    // ── The boundary ─────────────────────────────────────────

    /// The core promise. A private project answers exactly as a project that
    /// does not exist does (same status, same body), so no stranger can use
    /// this surface to enumerate what an instance holds.
    #[tokio::test]
    async fn a_private_project_is_indistinguishable_from_a_missing_one() {
        let f = fixture();
        let project = f.seed_project("PRIV", false);
        let issue = f.seed_issue(project, "Private issue", "secret body");
        let page = f.seed_page(Some(project), "Private page", "secret");
        let attachment = f.seed_attachment(AttachmentEntity::Issue, issue, "secret.pdf");

        for hidden in all_routes(issue, page, attachment) {
            let hidden = hidden.replace("/PUB", "/PRIV");
            let missing = hidden.replace("/PRIV", "/NOPE");
            let a = f.get(&hidden).await;
            let b = f.get(&missing).await;
            assert_eq!(a.status(), StatusCode::NOT_FOUND, "{hidden}");
            assert_eq!(b.status(), StatusCode::NOT_FOUND, "{missing}");
            assert_eq!(
                body_string(a).await,
                body_string(b).await,
                "{hidden} must not read differently from {missing}"
            );
        }
    }

    /// Only the project in the path publishes anything. An id or identifier
    /// from another project, even a public one, is inert through this path.
    #[tokio::test]
    async fn another_projects_content_is_unreachable_through_this_path() {
        let f = fixture();
        let public = f.seed_project("PUB", true);
        let other = f.seed_project("OTHER", true);
        f.seed_issue(public, "Mine", "");
        let theirs = f.seed_issue(other, "Theirs", "");
        let their_page = f.seed_page(Some(other), "Their page", "");
        let their_comment = f.seed_comment(CommentParent::Issue(theirs), "theirs");
        let their_file = f.seed_attachment(AttachmentEntity::Issue, theirs, "theirs.pdf");
        let their_comment_file =
            f.seed_attachment(AttachmentEntity::Comment, their_comment, "theirsc.pdf");

        for path in [
            "/public/api/projects/PUB/issues/resolve/OTHER-1".to_string(),
            format!("/public/api/projects/PUB/issues/{theirs}"),
            format!("/public/api/projects/PUB/issues/{theirs}/comments"),
            format!("/public/api/projects/PUB/pages/{their_page}"),
            format!("/public/api/projects/PUB/pages/{their_page}/comments"),
            format!("/public/api/projects/PUB/attachments?entity_type=issue&entity_id={theirs}"),
            format!(
                "/public/api/projects/PUB/attachments?entity_type=comment&entity_id={their_comment}"
            ),
            format!("/public/api/projects/PUB/attachments/{their_file}"),
            format!("/public/api/projects/PUB/attachments/{their_comment_file}"),
        ] {
            assert_eq!(f.get(&path).await.status(), StatusCode::NOT_FOUND, "{path}");
        }
        // And the same rows are fine through their own project.
        assert_eq!(
            f.get(&format!("/public/api/projects/OTHER/issues/{theirs}"))
                .await
                .status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn deleted_content_is_not_public() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let live = f.seed_issue(project, "Live", "");
        let doomed = f.seed_issue(project, "Doomed", "");
        let doomed_comment = f.seed_comment(CommentParent::Issue(doomed), "going away");
        let doomed_file = f.seed_attachment(AttachmentEntity::Issue, doomed, "doomed.pdf");
        let doomed_comment_file =
            f.seed_attachment(AttachmentEntity::Comment, doomed_comment, "doomedc.pdf");
        let doomed_page = f.seed_page(Some(project), "Doomed page", "");
        let doomed_page_file = f.seed_attachment(AttachmentEntity::Page, doomed_page, "dp.pdf");
        // A deleted comment on a live issue.
        let gone_comment = f.seed_comment(CommentParent::Issue(live), "deleted");
        let gone_comment_file =
            f.seed_attachment(AttachmentEntity::Comment, gone_comment, "gone.pdf");
        {
            let conn = f.db.write().unwrap();
            crate::db::queries::comments::delete_comment(&conn, gone_comment).unwrap();
        }
        f.delete_issue(doomed);
        f.delete_page(doomed_page);

        let json = body_json(f.get("/public/api/projects/PUB/index").await).await;
        assert_eq!(json["issues"].as_array().unwrap().len(), 1);
        assert_eq!(json["pages"].as_array().unwrap().len(), 0);

        for path in [
            "/public/api/projects/PUB/issues/resolve/PUB-2".to_string(),
            format!("/public/api/projects/PUB/issues/{doomed}"),
            format!("/public/api/projects/PUB/issues/{doomed}/comments"),
            format!("/public/api/projects/PUB/pages/{doomed_page}"),
            format!("/public/api/projects/PUB/attachments/{doomed_file}"),
            format!("/public/api/projects/PUB/attachments/{doomed_comment_file}"),
            format!("/public/api/projects/PUB/attachments/{doomed_page_file}"),
            format!("/public/api/projects/PUB/attachments/{gone_comment_file}"),
            format!(
                "/public/api/projects/PUB/attachments?entity_type=comment&entity_id={gone_comment}"
            ),
        ] {
            assert_eq!(f.get(&path).await.status(), StatusCode::NOT_FOUND, "{path}");
        }
        let json = body_json(
            f.get(&format!("/public/api/projects/PUB/issues/{live}/comments"))
                .await,
        )
        .await;
        assert_eq!(
            json.as_array().unwrap().len(),
            0,
            "the deleted comment is gone"
        );
    }

    /// Publication is re-read on every request, including for URLs already in
    /// somebody's hands.
    #[tokio::test]
    async fn unpublishing_closes_every_open_path() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let issue = f.seed_issue(project, "Public issue", "");
        f.seed_comment(CommentParent::Issue(issue), "hello");
        let page = f.seed_page(Some(project), "Page", "");
        let attachment = f.seed_attachment(AttachmentEntity::Issue, issue, "spec.pdf");
        let routes = all_routes(issue, page, attachment);
        for path in &routes {
            assert_eq!(f.get(path).await.status(), StatusCode::OK, "{path}");
        }
        f.unpublish("PUB");
        for path in &routes {
            assert_eq!(f.get(path).await.status(), StatusCode::NOT_FOUND, "{path}");
        }
    }

    #[tokio::test]
    async fn workspace_pages_plans_and_history_are_not_on_this_surface() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let issue = f.seed_issue(project, "Public issue", "");
        let workspace_page = f.seed_page(None, "Workspace page", "not project content");
        for path in [
            format!("/public/api/projects/PUB/pages/{workspace_page}"),
            "/public/api/projects/PUB/plans".into(),
            "/public/api/projects/PUB/activity".into(),
            "/public/api/projects/PUB/members".into(),
            "/public/api/projects/PUB/my-role".into(),
            "/public/api/projects/PUB/views".into(),
            "/public/api/projects/PUB/mention-candidates".into(),
            format!("/public/api/projects/PUB/issues/{issue}/activity"),
            "/public/api/projects/PUB/attachments/orphans".into(),
            "/public/api/projects/PUB/issues/-1".into(),
            "/public/api/search?query=x".into(),
            "/public/api/projects".into(),
        ] {
            assert_eq!(f.get(&path).await.status(), StatusCode::NOT_FOUND, "{path}");
        }
    }

    // ── Attachments ──────────────────────────────────────────

    #[tokio::test]
    async fn attachments_linked_to_public_issues_comments_and_pages_download() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let issue = f.seed_issue(project, "Public issue", "");
        let comment = f.seed_comment(CommentParent::Issue(issue), "see attached");
        let page = f.seed_page(Some(project), "Page", "");
        let page_comment = f.seed_comment(CommentParent::Page(page), "see attached");
        let on_issue = f.seed_attachment(AttachmentEntity::Issue, issue, "issue.pdf");
        let on_comment = f.seed_attachment(AttachmentEntity::Comment, comment, "comment.pdf");
        let on_page = f.seed_attachment(AttachmentEntity::Page, page, "page.pdf");
        let on_page_comment =
            f.seed_attachment(AttachmentEntity::Comment, page_comment, "pagecomment.pdf");

        for id in [on_issue, on_comment, on_page, on_page_comment] {
            let response = f
                .get(&format!("/public/api/projects/PUB/attachments/{id}"))
                .await;
            assert_eq!(response.status(), StatusCode::OK);
            let headers = response.headers().clone();
            assert_eq!(
                headers.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(),
                "nosniff"
            );
            assert!(
                headers
                    .get(header::CONTENT_DISPOSITION)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("attachment;"),
                "a PDF must download, not render inline"
            );
            assert_eq!(
                headers.get(header::CONTENT_SECURITY_POLICY).unwrap(),
                "default-src 'none'; sandbox"
            );
            assert!(body_string(response).await.starts_with("bytes for"));
        }

        // The listing route is scrubbed and re-checks the entity.
        let json = body_json(
            f.get(&format!(
                "/public/api/projects/PUB/attachments?entity_type=page&entity_id={page}"
            ))
            .await,
        )
        .await;
        assert_eq!(json[0]["id"], on_page);
        assert!(
            json[0]["uploader_id"].is_null(),
            "uploader is account metadata"
        );
        assert!(json[0].get("sha256").is_none());
        assert_eq!(
            f.get("/public/api/projects/PUB/attachments?entity_type=bogus&entity_id=1")
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }

    /// Deriving a thumbnail or a preview reads the whole file into memory,
    /// which an anonymous caller must not be able to ask for at any size.
    #[tokio::test]
    async fn derived_views_are_refused_for_oversized_attachments() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let issue = f.seed_issue(project, "Public issue", "");
        let id = f.seed_attachment(AttachmentEntity::Issue, issue, "huge.png");
        f.db.write()
            .unwrap()
            .execute(
                "UPDATE attachments SET mime = 'image/png', size_bytes = ?1 WHERE id = ?2",
                params![PUBLIC_DERIVE_MAX_BYTES + 1, id],
            )
            .unwrap();
        for suffix in ["/thumbnail", "/preview"] {
            assert_eq!(
                f.get(&format!(
                    "/public/api/projects/PUB/attachments/{id}{suffix}"
                ))
                .await
                .status(),
                StatusCode::NOT_FOUND,
                "{suffix}"
            );
        }
        // The bytes themselves still stream.
        assert_eq!(
            f.get(&format!("/public/api/projects/PUB/attachments/{id}"))
                .await
                .status(),
            StatusCode::OK
        );
    }

    /// The attachment id space is global and countable, so the download has
    /// to re-derive publication from the link graph on every request.
    #[tokio::test]
    async fn attachments_outside_the_published_graph_are_refused() {
        let f = fixture();
        f.seed_project("PUB", true);
        let private = f.seed_project("PRIV", false);
        let private_issue = f.seed_issue(private, "Private issue", "");
        let private_comment = f.seed_comment(CommentParent::Issue(private_issue), "private");
        let private_page = f.seed_page(Some(private), "Private page", "");
        let workspace_page = f.seed_page(None, "Workspace page", "");

        let cases = [
            (
                "another project's issue",
                f.seed_attachment(AttachmentEntity::Issue, private_issue, "priv.pdf"),
            ),
            (
                "another project's comment",
                f.seed_attachment(AttachmentEntity::Comment, private_comment, "privc.pdf"),
            ),
            (
                "another project's page",
                f.seed_attachment(AttachmentEntity::Page, private_page, "privp.pdf"),
            ),
            (
                "a workspace page",
                f.seed_attachment(AttachmentEntity::Page, workspace_page, "ws.pdf"),
            ),
            ("an orphan with no links", f.seed_orphan_attachment()),
            ("an id that does not exist", 99_999),
        ];
        for (what, id) in cases {
            for suffix in ["", "/thumbnail", "/preview"] {
                let response = f
                    .get(&format!(
                        "/public/api/projects/PUB/attachments/{id}{suffix}"
                    ))
                    .await;
                assert_eq!(response.status(), StatusCode::NOT_FOUND, "{what}{suffix}");
            }
        }
    }

    /// One attachment, two links: the shared blob is reachable through the
    /// project that published it and unreachable through the path of a
    /// project that did not.
    #[tokio::test]
    async fn an_attachment_shared_across_projects_follows_the_path_it_is_asked_through() {
        let f = fixture();
        let public = f.seed_project("PUB", true);
        let private = f.seed_project("PRIV", false);
        let public_issue = f.seed_issue(public, "Public issue", "");
        let private_issue = f.seed_issue(private, "Private issue", "");

        let shared = f.seed_attachment(AttachmentEntity::Issue, private_issue, "shared.pdf");
        {
            let conn = f.db.write().unwrap();
            crate::db::queries::attachments::link_attachment(
                &conn,
                shared,
                AttachmentEntity::Issue,
                public_issue,
            )
            .unwrap();
        }

        assert_eq!(
            f.get(&format!("/public/api/projects/PUB/attachments/{shared}"))
                .await
                .status(),
            StatusCode::OK,
            "publishing an issue publishes the files its body embeds"
        );
        assert_eq!(
            f.get(&format!("/public/api/projects/PRIV/attachments/{shared}"))
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "the private project's path must not serve it"
        );
    }

    /// An SVG is a document that can run script, so it is never served in a
    /// way a browser will render in place.
    #[tokio::test]
    async fn an_unsafe_format_is_forced_to_download() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let issue = f.seed_issue(project, "Public issue", "");
        let id = {
            let bytes =
                br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#
                    .to_vec();
            let sha = f.store.write(&bytes).unwrap();
            let conn = f.db.write().unwrap();
            let attachment = crate::db::queries::attachments::create_attachment(
                &conn,
                &sha,
                "evil\";x=\".svg",
                "image/svg+xml",
                bytes.len() as i64,
                None,
            )
            .unwrap();
            crate::db::queries::attachments::link_attachment(
                &conn,
                attachment.id,
                AttachmentEntity::Issue,
                issue,
            )
            .unwrap();
            attachment.id
        };

        let response = f
            .get(&format!("/public/api/projects/PUB/attachments/{id}"))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        let headers = response.headers();
        assert_eq!(
            headers.get(header::CONTENT_TYPE).unwrap(),
            "application/octet-stream"
        );
        let disposition = headers
            .get(header::CONTENT_DISPOSITION)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(disposition.starts_with("attachment;"), "{disposition}");
        assert!(
            !disposition.contains("evil\";"),
            "the quote must be stripped, not passed through: {disposition}"
        );
    }

    #[tokio::test]
    async fn public_download_headers_bound_and_sanitize_legacy_filenames() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let issue = f.seed_issue(project, "Issue", "Body");
        let id = f.seed_attachment(AttachmentEntity::Issue, issue, "file.txt");
        f.db.write()
            .unwrap()
            .execute(
                "UPDATE attachments SET filename=?1 WHERE id=?2",
                params![format!("name\0{}\r\n", "z".repeat(200_000)), id],
            )
            .unwrap();
        let response = f
            .get(&format!("/public/api/projects/PUB/attachments/{id}"))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value = response.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap();
        assert!(value.len() < 1024);
        assert!(!value.chars().any(char::is_control));
        assert!(!body_string(response).await.is_empty());
    }

    // ── Methods and headers ──────────────────────────────────

    /// Nothing on this surface writes. The routing table only knows `get`,
    /// so every other verb is refused before a handler exists to refuse it.
    #[tokio::test]
    async fn every_mutating_method_is_refused() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let issue = f.seed_issue(project, "Public issue", "");
        f.seed_comment(CommentParent::Issue(issue), "hello");
        let page = f.seed_page(Some(project), "Page", "");
        let attachment = f.seed_attachment(AttachmentEntity::Issue, issue, "spec.pdf");

        for path in all_routes(issue, page, attachment) {
            for method in ["POST", "PUT", "PATCH", "DELETE"] {
                let response = f.request(method, &path).await;
                assert_eq!(
                    response.status(),
                    StatusCode::METHOD_NOT_ALLOWED,
                    "{method} {path}"
                );
            }
        }
    }

    /// Every response, including the ones nobody plans for, carries the
    /// no-store and browser-hardening headers. A cached public response
    /// would outlive the decision to unpublish.
    #[tokio::test]
    async fn every_response_is_uncacheable_and_hardened() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let issue = f.seed_issue(project, "Public issue", "");
        let attachment = f.seed_attachment(AttachmentEntity::Issue, issue, "spec.pdf");

        for (path, method) in [
            ("/public/api/projects/PUB", "GET"),
            ("/public/api/projects/PUB/index", "GET"),
            ("/public/api/projects/PUB/issues/resolve/PUB-1", "GET"),
            // a 404
            ("/public/api/projects/NOPE", "GET"),
            // a 405
            ("/public/api/projects/PUB/index", "POST"),
        ] {
            let response = f.request(method, path).await;
            let headers = response.headers();
            assert_eq!(
                headers.get(header::CACHE_CONTROL).unwrap(),
                "no-store, no-cache, must-revalidate",
                "{method} {path}"
            );
            assert_eq!(headers.get(header::PRAGMA).unwrap(), "no-cache");
            assert_eq!(headers.get(header::REFERRER_POLICY).unwrap(), "no-referrer");
            assert_eq!(
                headers.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(),
                "nosniff"
            );
            assert_eq!(headers.get(header::X_FRAME_OPTIONS).unwrap(), "DENY");
            assert_eq!(
                headers.get("cross-origin-resource-policy").unwrap(),
                "same-origin"
            );
        }

        // The attachment download would otherwise inherit its authenticated
        // sibling's year-long immutable cache.
        let response = f
            .get(&format!(
                "/public/api/projects/PUB/attachments/{attachment}"
            ))
            .await;
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store, no-cache, must-revalidate"
        );
    }

    // ── What must never appear in a public body ──────────────

    /// The shapes are the private ones, so the scrub is what stands between a
    /// reader and the account data those shapes normally carry. This walks
    /// every route and checks the values, not the field names.
    #[tokio::test]
    async fn public_json_carries_no_account_data_provenance_or_private_relations() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let private = f.seed_project("PRIV", false);
        let issue = f.seed_issue(project, "Public issue", "body");
        let theirs = f.seed_issue(private, "Their issue", "");
        {
            let conn = f.db.write().unwrap();
            crate::db::queries::link_issues(&conn, issue, theirs, "blocks").unwrap();
        }
        let comment = f.seed_comment(CommentParent::Issue(issue), "a comment");
        let page = f.seed_page(Some(project), "Page", "");
        let attachment = f.seed_attachment(AttachmentEntity::Issue, issue, "spec.pdf");
        f.seed_attachment(AttachmentEntity::Comment, comment, "c.pdf");

        let forbidden = [
            "owner@test.local",
            "password_hash",
            "\"owner\"",
            "acme/secret",
            "PRIV-1",
            "is_admin",
            "\"user_id\":1",
            "\"uploader_id\":1",
            "\"lead_user_id\":1",
            "sha256",
        ];
        for path in all_routes(issue, page, attachment) {
            let response = f.get(&path).await;
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            let body = body_string(response).await;
            for needle in forbidden {
                assert!(
                    !body.contains(needle),
                    "{path} exposed {needle:?} in: {body}"
                );
            }
        }

        let json = body_json(f.get("/public/api/projects/PUB").await).await;
        assert!(json["lead_user_id"].is_null());
        let json = body_json(f.get("/public/api/projects/PUB/issues/resolve/PUB-1").await).await;
        assert!(json.get("source").is_none());
        assert!(
            json.get("blocks").is_none(),
            "the only relation was private"
        );
        let json = body_json(
            f.get(&format!("/public/api/projects/PUB/issues/{issue}/comments"))
                .await,
        )
        .await;
        assert_eq!(json[0]["user_id"], 0);
        assert_eq!(json[0]["author"], "");
        assert_eq!(json[0]["author_display_name"], "Owner");
    }

    // ── Sync stream ──────────────────────────────────────────

    #[tokio::test]
    async fn the_change_stream_omits_comments_but_still_advances_past_them() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let issue = f.seed_issue(project, "Public issue", "");
        let json = body_json(f.get("/public/api/projects/PUB/index").await).await;
        let cursor = json["cursor"].as_i64().unwrap();
        assert_eq!(cursor, json["issues"][0]["seq"].as_i64().unwrap());

        f.seed_comment(CommentParent::Issue(issue), "one");
        f.seed_comment(CommentParent::Issue(issue), "two");
        let json = body_json(
            f.get(&format!(
                "/public/api/projects/PUB/changes?since={cursor}&limit=1"
            ))
            .await,
        )
        .await;
        assert_eq!(json["changes"].as_array().unwrap().len(), 0);
        assert!(json["cursor"].as_i64().unwrap() > cursor, "must not stall");
        assert_eq!(json["has_more"], true);

        // Tombstones for issues do ride the stream; comment tombstones do not.
        f.delete_issue(issue);
        let json = body_json(
            f.get(&format!("/public/api/projects/PUB/changes?since={cursor}"))
                .await,
        )
        .await;
        let kinds: Vec<&str> = json["changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|change| change["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, vec!["issue"]);
        assert_eq!(json["changes"][0]["deleted"], true);
    }

    // ── Comment paging ──────────────────────────────────────

    /// A long thread pages exactly as it does on the private route, headers
    /// included, so the shared comment component needs no special case.
    #[tokio::test]
    async fn comment_paging_mirrors_the_private_contract() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let issue = f.seed_issue(project, "Chatty", "");
        for n in 0..7 {
            f.seed_comment(CommentParent::Issue(issue), &format!("comment {n}"));
        }
        let response = f
            .get(&format!(
                "/public/api/projects/PUB/issues/{issue}/comments?limit=5"
            ))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        let headers = response.headers().clone();
        assert_eq!(headers[super::super::comments::HAS_MORE_HEADER], "true");
        assert_eq!(headers[super::super::comments::RETURNED_HEADER], "5");
        assert_eq!(headers[super::super::comments::NEXT_OFFSET_HEADER], "5");
        let json = body_json(response).await;
        assert_eq!(json.as_array().unwrap().len(), 5);

        let response = f
            .get(&format!(
                "/public/api/projects/PUB/issues/{issue}/comments?limit=5&offset=5"
            ))
            .await;
        assert_eq!(
            response.headers()[super::super::comments::HAS_MORE_HEADER],
            "false"
        );
        assert_eq!(body_json(response).await.as_array().unwrap().len(), 2);

        // The author filter is ignored rather than honoured: honouring it
        // would answer "does this username exist" one guess at a time.
        let json = body_json(
            f.get(&format!(
                "/public/api/projects/PUB/issues/{issue}/comments?author=owner&limit=100"
            ))
            .await,
        )
        .await;
        assert_eq!(json.as_array().unwrap().len(), 7);
        let json = body_json(
            f.get(&format!(
                "/public/api/projects/PUB/issues/{issue}/comments?author=nobody&limit=100"
            ))
            .await,
        )
        .await;
        assert_eq!(json.as_array().unwrap().len(), 7);

        // A half-supplied cursor is refused, exactly as it is privately.
        assert_eq!(
            f.get(&format!(
                "/public/api/projects/PUB/issues/{issue}/comments?before_id=1"
            ))
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    // ── Anonymous load bounds (Sol, medium) ──────────────────

    /// One host cannot walk the public API in an unbounded loop. The limiter
    /// keys on the resolved client IP, so this is also the proof that the
    /// keying works at all.
    #[tokio::test]
    async fn a_single_client_is_rate_limited() {
        let f = fixture();
        f.seed_project("PUB", true);

        let mut refused = 0;
        let mut allowed = 0;
        for _ in 0..(PUBLIC_READS_PER_MINUTE + 20) {
            match f.get("/public/api/projects/PUB").await.status() {
                StatusCode::OK => allowed += 1,
                StatusCode::TOO_MANY_REQUESTS => refused += 1,
                other => panic!("unexpected status {other}"),
            }
        }
        assert_eq!(allowed, PUBLIC_READS_PER_MINUTE);
        assert_eq!(refused, 20);
    }

    /// A refusal is still a public response: it must not leak out of the
    /// no-store/hardening envelope, and it must not have touched the database.
    #[tokio::test]
    async fn a_rate_limited_response_is_still_hardened() {
        let f = fixture();
        f.seed_project("PUB", true);
        for _ in 0..PUBLIC_READS_PER_MINUTE {
            let _ = f.get("/public/api/projects/PUB").await;
        }
        let response = f.get("/public/api/projects/PUB").await;
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let headers = response.headers();
        assert_eq!(
            headers.get(header::CACHE_CONTROL).unwrap(),
            "no-store, no-cache, must-revalidate"
        );
        assert_eq!(headers.get(header::X_FRAME_OPTIONS).unwrap(), "DENY");
        assert!(headers.get(header::RETRY_AFTER).is_some());
    }

    /// Behind a reverse proxy every request arrives from the proxy's address.
    /// Two readers must get two buckets, which means the forwarding header has
    /// to be honoured, but only because the peer is a configured trusted
    /// proxy. The spoofing half is covered by `ratelimit`'s own tests; this
    /// asserts the public router actually consults them.
    #[tokio::test]
    async fn separate_forwarded_clients_get_separate_budgets() {
        let f = fixture();
        f.seed_project("PUB", true);

        for _ in 0..PUBLIC_READS_PER_MINUTE {
            assert_eq!(
                f.get_as("203.0.113.7", "/public/api/projects/PUB")
                    .await
                    .status(),
                StatusCode::OK
            );
        }
        assert_eq!(
            f.get_as("203.0.113.7", "/public/api/projects/PUB")
                .await
                .status(),
            StatusCode::TOO_MANY_REQUESTS,
            "the exhausted reader is refused"
        );
        assert_eq!(
            f.get_as("203.0.113.8", "/public/api/projects/PUB")
                .await
                .status(),
            StatusCode::OK,
            "a different reader must not inherit somebody else's exhaustion"
        );
    }

    /// The gate refuses rather than queues, and the permit covers the handler
    /// while it runs. The first request parks inside the handler, so the
    /// second is refused *during* the first, not because the semaphore was
    /// drained beforehand.
    #[tokio::test]
    async fn a_request_in_flight_holds_its_permit() {
        let semaphore = Arc::new(Semaphore::new(1));
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel::<()>();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let entered = Arc::new(tokio::sync::Mutex::new(Some(entered_tx)));
        let release = Arc::new(tokio::sync::Mutex::new(Some(release_rx)));

        let app = Router::new()
            .route(
                "/probe",
                get(move || {
                    let entered = Arc::clone(&entered);
                    let release = Arc::clone(&release);
                    async move {
                        let first = entered.lock().await.take();
                        if let Some(tx) = first {
                            let _ = tx.send(());
                            let wait = release.lock().await.take();
                            if let Some(rx) = wait {
                                let _ = rx.await;
                            }
                        }
                        "ok"
                    }
                }),
            )
            // Same order as `router`: bounds inside, headers outside.
            .layer(middleware::from_fn(enforce_load_bounds))
            .layer(middleware::from_fn(no_store_headers))
            .layer(Extension(Arc::new(PublicConcurrency(Arc::clone(
                &semaphore,
            )))));

        let probe = |app: Router| {
            app.oneshot(
                Request::builder()
                    .uri("/probe")
                    .body(Body::empty())
                    .unwrap(),
            )
        };

        let first = tokio::spawn(probe(app.clone()));
        entered_rx.await.expect("first request reached the handler");

        let blocked = probe(app.clone()).await.unwrap();
        assert_eq!(
            blocked.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "a second request must be refused while the first is still running"
        );
        assert_eq!(blocked.headers().get(header::RETRY_AFTER).unwrap(), "2");
        assert_eq!(
            blocked.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store, no-cache, must-revalidate"
        );

        let _ = release_tx.send(());
        let done = first.await.unwrap().unwrap();
        assert_eq!(done.status(), StatusCode::OK);
        let _ = done.into_body().collect().await;

        // Released, so the surface reopens; the permit was not leaked.
        assert_eq!(probe(app.clone()).await.unwrap().status(), StatusCode::OK);
        assert_eq!(semaphore.available_permits(), 1);
    }

    /// An active producer retains its permit while the bounded channel is full.
    #[tokio::test]
    async fn a_stalled_large_download_holds_its_permit_until_production_ends() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let issue = f.seed_issue(project, "Public issue", "");
        let attachment = f.seed_attachment(AttachmentEntity::Issue, issue, "spec.pdf");
        let expected = vec![42; DOWNLOAD_CHUNK_BYTES * 4];
        let sha = f.store.write(&expected).unwrap();
        f.db.write()
            .unwrap()
            .execute(
                "UPDATE attachments SET sha256 = ?1, size_bytes = ?2 WHERE id = ?3",
                params![sha, expected.len() as i64, attachment],
            )
            .unwrap();

        let response = f
            .get(&format!(
                "/public/api/projects/PUB/attachments/{attachment}"
            ))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body();
        let first = body.frame().await.unwrap().unwrap().into_data().unwrap();
        assert_eq!(first.len(), DOWNLOAD_CHUNK_BYTES);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(
            f.concurrency.0.available_permits(),
            PUBLIC_CONCURRENCY - 1,
            "the permit must still be held while the producer is blocked"
        );

        let remaining = body.collect().await.unwrap().to_bytes();
        assert_eq!([first.as_ref(), remaining.as_ref()].concat(), expected);
        assert_eq!(
            f.concurrency.0.available_permits(),
            PUBLIC_CONCURRENCY,
            "the permit is released once the body is finished"
        );
    }

    fn test_download(size: usize, idle_ms: u64, max_ms: u64) -> (Body, Arc<Semaphore>) {
        let file = tempfile::tempfile().unwrap();
        file.set_len(size as u64).unwrap();
        let semaphore = Arc::new(Semaphore::new(1));
        let permit = Arc::clone(&semaphore).try_acquire_owned().unwrap();
        let body = download_body(
            file,
            Some(permit),
            std::time::Duration::from_millis(idle_ms),
            std::time::Duration::from_millis(max_ms),
        );
        (body, semaphore)
    }

    async fn wait_for_download_release(semaphore: &Arc<Semaphore>) {
        let permit = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            Arc::clone(semaphore).acquire_owned(),
        )
        .await
        .expect("producer must release its permit without body polling")
        .unwrap();
        drop(permit);
    }

    #[tokio::test]
    async fn an_unpolled_download_times_out_idle_and_reports_truncation() {
        let (mut body, semaphore) = test_download(DOWNLOAD_CHUNK_BYTES * 4, 20, 5_000);
        wait_for_download_release(&semaphore).await;
        let queued = body.frame().await.unwrap().unwrap().into_data().unwrap();
        assert_eq!(queued.len(), DOWNLOAD_CHUNK_BYTES);
        let error = body.frame().await.unwrap().unwrap_err();
        assert!(
            error.to_string().contains("public download idle timeout"),
            "{error}"
        );
        assert!(body.frame().await.is_none());
        assert!(
            body.frame().await.is_none(),
            "terminal polling must remain safe"
        );
    }

    #[tokio::test]
    async fn a_partly_read_download_hits_its_deadline_without_further_polling() {
        let (mut body, semaphore) = test_download(DOWNLOAD_CHUNK_BYTES * 100, 5_000, 200);
        for _ in 0..2 {
            let frame = body.frame().await.unwrap().unwrap().into_data().unwrap();
            assert_eq!(frame.len(), DOWNLOAD_CHUNK_BYTES);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(semaphore.available_permits(), 0);
        wait_for_download_release(&semaphore).await;
        let error = body.collect().await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("public download deadline exceeded"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_fully_buffered_download_releases_its_permit_before_body_consumption() {
        let (body, semaphore) = test_download(17, 5_000, 5_000);
        wait_for_download_release(&semaphore).await;
        assert_eq!(body.collect().await.unwrap().to_bytes().as_ref(), &[0; 17]);
    }

    #[tokio::test]
    async fn dropping_a_download_releases_its_producer_permit() {
        let (mut body, semaphore) = test_download(DOWNLOAD_CHUNK_BYTES * 4, 5_000, 5_000);
        assert!(body.frame().await.unwrap().unwrap().is_data());
        drop(body);
        wait_for_download_release(&semaphore).await;
    }
}

/// Who is allowed to *turn publication on*.
///
/// The read surface above has no identity at all; this is the other half of
/// the feature, and it is the half with an authorization question. It rides
/// the existing `PUT /api/projects/{id}` route rather than inventing one, so
/// the gate is `authz`'s Lead gate verbatim, the same one that already
/// guards renaming a project and naming its lead. These tests exist because
/// "reuses the existing gate" is a claim, and the consequence of it being
/// wrong is a project on the open internet.
#[cfg(test)]
mod publication_setting_tests {
    use crate::api::test_helpers::*;
    use crate::db::models::*;
    use axum::http::StatusCode;

    /// Publication is off for every project the moment it is created. There
    /// is no inherited value, no "public because it has no members".
    #[tokio::test]
    async fn a_new_project_is_private() {
        let app = test_app();
        let (_, project) = seed_project(&app).await;
        assert_eq!(project["is_public"], false);
    }

    /// Legacy mode (`authz_enforced` off) is the default on every existing
    /// instance, and its `require_role` treats Viewer and Maintainer as
    /// unconditional allows. Only the Lead branch denies anybody, so this is
    /// the mode where a wrong gate would be invisible.
    #[tokio::test]
    async fn only_the_lead_or_an_admin_may_publish_in_legacy_mode() {
        let (db, admin, lead, regular, project_id) = setup_lead_test();
        assert!(!crate::authz::authz_enforced(&db).unwrap());

        let publish = serde_json::json!({ "is_public": true });

        let response = json_put(
            &app_as_user(db.clone(), &regular),
            &format!("/api/projects/{project_id}"),
            publish.clone(),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "a non-lead must not be able to publish a project"
        );
        assert!(!is_public(&db, project_id));

        let response = json_put(
            &app_as_user(db.clone(), &lead),
            &format!("/api/projects/{project_id}"),
            publish.clone(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(is_public(&db, project_id));

        // And an admin can put it back.
        let response = json_put(
            &app_as_user(db.clone(), &admin),
            &format!("/api/projects/{project_id}"),
            serde_json::json!({ "is_public": false }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!is_public(&db, project_id));
    }

    /// Enforced mode: the roles below Lead exist and are denied by name.
    #[tokio::test]
    async fn maintainers_and_viewers_may_not_publish_in_enforced_mode() {
        let (db, _admin, lead, maintainer, viewer, non_member, project_id) =
            setup_membership_test();

        for user in [&maintainer, &viewer, &non_member] {
            let response = json_put(
                &app_as_user(db.clone(), user),
                &format!("/api/projects/{project_id}"),
                serde_json::json!({ "is_public": true }),
            )
            .await;
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "{} must not be able to publish",
                user.username
            );
            assert!(!is_public(&db, project_id));
        }

        let response = json_put(
            &app_as_user(db.clone(), &lead),
            &format!("/api/projects/{project_id}"),
            serde_json::json!({ "is_public": true }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(is_public(&db, project_id));
    }

    /// A connected tool acts with its owner's permissions and no more: the
    /// existing bot policy, applied to the one setting where exceeding it
    /// would put private issues on the internet. An agent is exactly the
    /// caller most likely to be talked into trying (prompt injection), so the
    /// answer has to come from the owner's role rather than from the bot's.
    #[tokio::test]
    async fn a_bot_publishes_only_what_its_owner_could() {
        let (db, _admin, lead, regular, project_id) = setup_lead_test();

        let (lead_bot, regular_bot) = {
            let conn = db.write().unwrap();
            (
                crate::db::queries::users::create_bot_user(
                    &conn,
                    lead.id,
                    "lead-bot",
                    "Lead's tool",
                    Some("tool-a"),
                )
                .unwrap(),
                crate::db::queries::users::create_bot_user(
                    &conn,
                    regular.id,
                    "regular-bot",
                    "Regular's tool",
                    Some("tool-b"),
                )
                .unwrap(),
            )
        };

        let response = json_put(
            &app_as_user(db.clone(), &regular_bot),
            &format!("/api/projects/{project_id}"),
            serde_json::json!({ "is_public": true }),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "a bot owned by a non-lead must not publish"
        );
        assert!(!is_public(&db, project_id));

        let response = json_put(
            &app_as_user(db.clone(), &lead_bot),
            &format!("/api/projects/{project_id}"),
            serde_json::json!({ "is_public": true }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(is_public(&db, project_id));
    }

    /// Publication is only ever changed by a request that says so. A rename
    /// must not carry an implicit `is_public: false` and silently take a
    /// published project down (nor the reverse).
    #[tokio::test]
    async fn an_unrelated_project_edit_leaves_publication_alone() {
        let (db, _admin, lead, _regular, project_id) = setup_lead_test();
        let app = app_as_user(db.clone(), &lead);

        json_put(
            &app,
            &format!("/api/projects/{project_id}"),
            serde_json::json!({ "is_public": true }),
        )
        .await;
        assert!(is_public(&db, project_id));

        let response = json_put(
            &app,
            &format!("/api/projects/{project_id}"),
            serde_json::json!({ "name": "Renamed" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = parse_json(response).await;
        assert_eq!(body["name"], "Renamed");
        assert_eq!(body["is_public"], true);
        assert!(is_public(&db, project_id));
    }

    /// Flipping the flag is the most consequential project edit there is, so
    /// migration 051 records it. Without the row, an instance owner has no
    /// way to answer "when did this go public, and who did it".
    #[tokio::test]
    async fn publishing_is_written_to_the_audit_log() {
        let (db, _admin, lead, _regular, project_id) = setup_lead_test();
        let app = app_as_user(db.clone(), &lead);

        json_put(
            &app,
            &format!("/api/projects/{project_id}"),
            serde_json::json!({ "is_public": true }),
        )
        .await;
        json_put(
            &app,
            &format!("/api/projects/{project_id}"),
            serde_json::json!({ "is_public": false }),
        )
        .await;
        // A no-op re-publish writes nothing: the trigger's WHEN guard.
        json_put(
            &app,
            &format!("/api/projects/{project_id}"),
            serde_json::json!({ "is_public": false }),
        )
        .await;

        let conn = db.read().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT old_value, new_value FROM audit_log
                  WHERE entity_type = 'project' AND field = 'is_public'
               ORDER BY id",
            )
            .unwrap();
        let rows: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            rows,
            vec![
                ("private".to_string(), "public".to_string()),
                ("public".to_string(), "private".to_string()),
            ]
        );
    }

    fn is_public(db: &crate::db::DbPool, project_id: i64) -> bool {
        let conn = db.read().unwrap();
        crate::db::queries::get_project(&conn, project_id)
            .unwrap()
            .is_public
    }

    /// `setup_lead_test`'s users are `User`s; a bot created by
    /// `create_bot_user` is one too, so `app_as_user` works for both. This
    /// exists only to make that explicit for a reader.
    #[allow(dead_code)]
    fn _bots_are_users(user: &User) -> bool {
        user.is_bot
    }
}
