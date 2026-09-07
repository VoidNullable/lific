//! LIF-465: the anonymous read surface for a published project.
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
//!   method router rather than from five handlers remembering to check.
//!
//! Authorization is the `is_public = 1` predicate inside every statement in
//! [`crate::db::queries::public`]. Each read re-evaluates it, so unpublishing
//! closes all of them on the next request.
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
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::db::{DbPool, queries::public as q};
use crate::error::LificError;
use crate::ratelimit::{self, IpNetwork, RateLimiter};
use crate::storage::{self, AttachmentStore};

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

// ── Response envelopes ───────────────────────────────────────
//
// The payload types are the allowlists in `db::queries::public`; these only
// name the shape of each response.

#[derive(Debug, Serialize)]
struct ProjectResponse {
    project: q::PublicProject,
}

#[derive(Debug, Serialize)]
struct IssueListResponse {
    project: q::PublicProject,
    issues: Vec<q::PublicIssue>,
    /// Echoed so a paging client can see that its `limit` was clamped.
    limit: i64,
    offset: i64,
    has_more: bool,
}

#[derive(Debug, Serialize)]
struct CommentListResponse {
    comments: Vec<q::PublicComment>,
    /// Live comments on the issue, so a client knows what paging will reach.
    total: i64,
    limit: i64,
    offset: i64,
    has_more: bool,
}

/// `?limit=&offset=`. Both optional, both clamped rather than rejected: a 400
/// on a hand-typed `?limit=99999` would teach a stranger where the ceiling is
/// and gain nothing.
#[derive(Debug, Default, Deserialize)]
struct PageQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Debug, Serialize)]
struct IssueDetailResponse {
    project: q::PublicProject,
    #[serde(flatten)]
    issue: q::PublicIssueDetail,
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
    Router::new()
        .route("/public/api/projects/{project}", get(get_project))
        .route("/public/api/projects/{project}/issues", get(list_issues))
        .route(
            "/public/api/projects/{project}/issues/{issue}",
            get(get_issue),
        )
        .route(
            "/public/api/projects/{project}/issues/{issue}/comments",
            get(list_comments),
        )
        .route(
            "/public/api/projects/{project}/attachments/{id}",
            get(download_attachment),
        )
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

// ── Identifier handling ──────────────────────────────────────

/// Split `LIF-42` into its sequence, but only if it names `project`.
///
/// This is what makes a guessed cross-project identifier inert: the path
/// already says which project is being read, so an identifier naming a
/// different one is rejected here rather than resolved and then filtered.
/// Page identifiers (`DEMO-DOC-3`) fail as a side effect, since the prefix
/// works out to `DEMO-DOC`.
fn split_issue_identifier(project: &str, identifier: &str) -> Option<i64> {
    let (prefix, sequence) = identifier.rsplit_once('-')?;
    if !prefix.eq_ignore_ascii_case(project) {
        return None;
    }
    // `i64::from_str` alone accepts a leading sign, which is not an identifier.
    if sequence.is_empty() || !sequence.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    sequence.parse::<i64>().ok()
}

// ── Handlers ─────────────────────────────────────────────────

/// `GET /public/api/projects/{project}`
async fn get_project(
    State(db): State<DbPool>,
    Path(project): Path<String>,
) -> Result<axum::Json<ProjectResponse>, LificError> {
    let project =
        with_read(&db, |conn| q::get_public_project(conn, &project))?.ok_or_else(not_found)?;
    Ok(axum::Json(ProjectResponse { project }))
}

/// `GET /public/api/projects/{project}/issues`
async fn list_issues(
    State(db): State<DbPool>,
    Path(project): Path<String>,
    Query(page): Query<PageQuery>,
) -> Result<axum::Json<IssueListResponse>, LificError> {
    let limit = page
        .limit
        .unwrap_or(q::PUBLIC_ISSUE_PAGE)
        .clamp(1, q::PUBLIC_ISSUE_PAGE);
    let offset = page.offset.unwrap_or(0).max(0);
    let (project, issues, has_more) = with_read(&db, |conn| {
        let tx = conn.unchecked_transaction()?;
        let Some(project) = q::get_public_project(&tx, &project)? else {
            return Err(not_found());
        };
        let (issues, has_more) = q::list_public_issues(&tx, &project.identifier, limit, offset)?;
        Ok((project, issues, has_more))
    })?;
    Ok(axum::Json(IssueListResponse {
        project,
        issues,
        limit,
        offset,
        has_more,
    }))
}

/// `GET /public/api/projects/{project}/issues/{issue}`
async fn get_issue(
    State(db): State<DbPool>,
    Path((project, issue)): Path<(String, String)>,
) -> Result<axum::Json<IssueDetailResponse>, LificError> {
    let sequence = split_issue_identifier(&project, &issue).ok_or_else(not_found)?;
    let (project, issue) = with_read(&db, |conn| {
        let tx = conn.unchecked_transaction()?;
        let Some(project) = q::get_public_project(&tx, &project)? else {
            return Err(not_found());
        };
        let Some(issue) = q::get_public_issue(&tx, &project.identifier, sequence)? else {
            return Err(not_found());
        };
        Ok((project, issue))
    })?;
    Ok(axum::Json(IssueDetailResponse { project, issue }))
}

/// `GET /public/api/projects/{project}/issues/{issue}/comments`
async fn list_comments(
    State(db): State<DbPool>,
    Path((project, issue)): Path<(String, String)>,
    Query(page): Query<PageQuery>,
) -> Result<axum::Json<CommentListResponse>, LificError> {
    let sequence = split_issue_identifier(&project, &issue).ok_or_else(not_found)?;
    let limit = page
        .limit
        .unwrap_or(q::PUBLIC_COMMENT_PAGE)
        .clamp(1, q::PUBLIC_COMMENT_PAGE);
    let offset = page.offset.unwrap_or(0).max(0);
    let page = with_read(&db, |conn| {
        let tx = conn.unchecked_transaction()?;
        if !q::public_issue_exists(&tx, &project, sequence)? {
            return Err(not_found());
        }
        q::list_public_comments(&tx, &project, sequence, limit, offset)
    })?;
    Ok(axum::Json(CommentListResponse {
        comments: page.comments,
        total: page.total,
        limit,
        offset,
        has_more: page.has_more,
    }))
}

/// `GET /public/api/projects/{project}/attachments/{id}`
///
/// Re-authorized from scratch against the project in the path: an id lifted
/// from a private project, belonging to a page, orphaned, or whose issue was
/// deleted a moment ago all answer with the same 404 as one that never existed.
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
    Path((project, id)): Path<(String, i64)>,
) -> Result<Response<Body>, LificError> {
    let blob = with_read(&db, |conn| q::get_public_attachment(conn, &project, id))?
        .ok_or_else(not_found)?;

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

    /// A published `PUB` project and a private `PRIV` project, each with one
    /// live issue, one live comment, and one deleted issue carrying its own
    /// comment. Attachments are added per test so each one names the linkage
    /// it is actually about.
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
                    ..Default::default()
                },
            )
            .unwrap()
            .id
        }

        fn seed_comment(&self, issue_id: i64, content: &str) -> i64 {
            let conn = self.db.write().unwrap();
            crate::db::queries::comments::create_comment_with_mentions(
                &conn,
                crate::db::queries::comments::CommentParent::Issue(issue_id),
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

        fn seed_page(&self, project_id: i64) -> i64 {
            let conn = self.db.write().unwrap();
            crate::db::queries::create_page(
                &conn,
                &CreatePage {
                    project_id: Some(project_id),
                    title: "Private page".into(),
                    content: "secret".into(),
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
                None,
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

        /// A comment written straight to the table, bypassing the authenticated
        /// size validation, to stand in for legacy or imported data.
        fn seed_oversized_comment(&self, issue_id: i64, bytes: usize) -> i64 {
            let conn = self.db.write().unwrap();
            conn.execute(
                "INSERT INTO comments (issue_id, user_id, content) VALUES (?1, 1, ?2)",
                params![issue_id, "z".repeat(bytes)],
            )
            .unwrap();
            conn.last_insert_rowid()
        }

        fn set_description(&self, issue_id: i64, description: &str) {
            let conn = self.db.write().unwrap();
            conn.execute(
                "UPDATE issues SET description = ?1 WHERE id = ?2",
                params![description, issue_id],
            )
            .unwrap();
        }

        fn add_label(&self, project_id: i64, issue_id: i64, name: &str) {
            let conn = self.db.write().unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO labels (project_id, name) VALUES (?1, ?2)",
                params![project_id, name],
            )
            .unwrap();
            let label_id: i64 = conn
                .query_row(
                    "SELECT id FROM labels WHERE project_id = ?1 AND name = ?2",
                    params![project_id, name],
                    |row| row.get(0),
                )
                .unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO issue_labels (issue_id, label_id) VALUES (?1, ?2)",
                params![issue_id, label_id],
            )
            .unwrap();
        }

        /// SQL statements the public query module has issued on this thread.
        /// See `db::queries::public::probe` for why the tally exists and why
        /// it is thread-local.
        fn statements_run(&self) -> usize {
            crate::db::queries::public::probe::count()
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

    #[tokio::test]
    async fn public_comments_remain_readable_when_parent_text_exceeds_its_budget() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let issue = f.seed_issue(project, "Issue", "Body");
        f.seed_comment(issue, "Readable comment");
        let huge = "x".repeat(q::PUBLIC_PAGE_BYTES);
        f.set_description(issue, &huge);
        f.db.write()
            .unwrap()
            .execute(
                "UPDATE projects SET description=?1 WHERE id=?2",
                params![huge, project],
            )
            .unwrap();
        let response = f
            .get("/public/api/projects/PUB/issues/PUB-1/comments")
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            body_json(response).await["comments"][0]["content"],
            "Readable comment"
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

    // ── The happy path ───────────────────────────────────────

    #[tokio::test]
    async fn an_anonymous_visitor_reads_a_published_project() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let issue = f.seed_issue(project, "Public issue", "The body");
        f.seed_comment(issue, "A public comment");

        let response = f.get("/public/api/projects/PUB").await;
        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert_eq!(json["project"]["identifier"], "PUB");

        let response = f.get("/public/api/projects/PUB/issues").await;
        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert_eq!(json["issues"][0]["identifier"], "PUB-1");
        assert_eq!(json["issues"][0]["title"], "Public issue");
        assert_eq!(json["has_more"], false);

        let response = f.get("/public/api/projects/PUB/issues/PUB-1").await;
        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert_eq!(json["description"], "The body");

        let response = f
            .get("/public/api/projects/PUB/issues/PUB-1/comments")
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert_eq!(json["comments"][0]["content"], "A public comment");
        assert_eq!(json["has_more"], false);
    }

    /// The identifier column is NOCASE everywhere else in Lific; a shared
    /// link that lost its capitals must still resolve.
    #[tokio::test]
    async fn the_project_identifier_is_case_insensitive() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        f.seed_issue(project, "Public issue", "");
        assert_eq!(
            f.get("/public/api/projects/pub/issues/pub-1")
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
        f.seed_issue(project, "Private issue", "secret body");

        for (hidden, missing) in [
            ("/public/api/projects/PRIV", "/public/api/projects/NOPE"),
            (
                "/public/api/projects/PRIV/issues",
                "/public/api/projects/NOPE/issues",
            ),
            (
                "/public/api/projects/PRIV/issues/PRIV-1",
                "/public/api/projects/NOPE/issues/NOPE-1",
            ),
            (
                "/public/api/projects/PRIV/issues/PRIV-1/comments",
                "/public/api/projects/NOPE/issues/NOPE-1/comments",
            ),
        ] {
            let hidden_response = f.get(hidden).await;
            let missing_response = f.get(missing).await;
            assert_eq!(hidden_response.status(), StatusCode::NOT_FOUND, "{hidden}");
            assert_eq!(
                missing_response.status(),
                StatusCode::NOT_FOUND,
                "{missing}"
            );
            assert_eq!(
                body_string(hidden_response).await,
                body_string(missing_response).await,
                "{hidden} must answer identically to {missing}"
            );
        }
    }

    /// A published project must not become a lens onto its neighbours. The
    /// identifier in the path is matched against the identifier in the issue
    /// reference before any row is read.
    #[tokio::test]
    async fn a_cross_project_issue_identifier_resolves_to_nothing() {
        let f = fixture();
        let public = f.seed_project("PUB", true);
        let private = f.seed_project("PRIV", false);
        f.seed_issue(public, "Public issue", "");
        let secret = f.seed_issue(private, "Private issue", "secret body");
        f.seed_comment(secret, "secret comment");

        for uri in [
            "/public/api/projects/PUB/issues/PRIV-1",
            "/public/api/projects/PUB/issues/PRIV-1/comments",
            // A page identifier, which splits to the prefix `PUB-DOC`.
            "/public/api/projects/PUB/issues/PUB-DOC-1",
            // Nonsense that must not reach SQLite as a sequence.
            "/public/api/projects/PUB/issues/PUB-+1",
            "/public/api/projects/PUB/issues/PUB-1x",
            "/public/api/projects/PUB/issues/PUB-",
            "/public/api/projects/PUB/issues/1",
        ] {
            let response = f.get(uri).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
            assert!(
                !body_string(response).await.contains("secret"),
                "{uri} leaked private content"
            );
        }

        // And the second published project cannot be reached through the
        // first even when both are public: the sequence is scoped by the
        // identifier bound in the same statement.
        let other = f.seed_project("PUB2", true);
        f.seed_issue(other, "Other public issue", "");
        let json = body_json(f.get("/public/api/projects/PUB/issues/PUB-1").await).await;
        assert_eq!(json["title"], "Public issue");
    }

    /// A tombstoned issue is gone from the public view immediately, and so is
    /// its comment thread. The trash is not a public archive.
    #[tokio::test]
    async fn deleted_issues_and_comments_are_not_public() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let live = f.seed_issue(project, "Live issue", "");
        f.seed_comment(live, "live comment");
        let doomed = f.seed_issue(project, "Doomed issue", "about to go");
        f.seed_comment(doomed, "doomed comment");
        f.delete_issue(doomed);

        let json = body_json(f.get("/public/api/projects/PUB/issues").await).await;
        let titles: Vec<&str> = json["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["title"].as_str().unwrap())
            .collect();
        assert_eq!(titles, vec!["Live issue"]);

        assert_eq!(
            f.get("/public/api/projects/PUB/issues/PUB-2")
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            f.get("/public/api/projects/PUB/issues/PUB-2/comments")
                .await
                .status(),
            StatusCode::NOT_FOUND
        );

        // A comment deleted on its own drops out while its issue stays.
        let stray = f.seed_comment(live, "retracted");
        {
            let conn = f.db.write().unwrap();
            crate::db::queries::comments::delete_comment(&conn, stray).unwrap();
        }
        let json = body_json(
            f.get("/public/api/projects/PUB/issues/PUB-1/comments")
                .await,
        )
        .await;
        assert_eq!(json["comments"].as_array().unwrap().len(), 1);
        assert_eq!(json["comments"][0]["content"], "live comment");
    }

    /// Unpublishing closes the detail and download paths that were open a
    /// moment ago. The address itself survives, so republishing resumes it.
    #[tokio::test]
    async fn unpublishing_closes_every_open_path() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let issue = f.seed_issue(project, "Public issue", "");
        f.seed_comment(issue, "hello");
        let attachment = f.seed_attachment(AttachmentEntity::Issue, issue, "spec.pdf");

        let paths = [
            "/public/api/projects/PUB".to_string(),
            "/public/api/projects/PUB/issues".to_string(),
            "/public/api/projects/PUB/issues/PUB-1".to_string(),
            "/public/api/projects/PUB/issues/PUB-1/comments".to_string(),
            format!("/public/api/projects/PUB/attachments/{attachment}"),
        ];
        for path in &paths {
            assert_eq!(f.get(path).await.status(), StatusCode::OK, "{path}");
        }

        f.unpublish("PUB");
        for path in &paths {
            assert_eq!(
                f.get(path).await.status(),
                StatusCode::NOT_FOUND,
                "{path} stayed open after unpublish"
            );
        }

        // Republishing resumes the same stable address; no new URL is minted.
        {
            let conn = f.db.write().unwrap();
            conn.execute(
                "UPDATE projects SET is_public = 1 WHERE identifier = 'PUB'",
                [],
            )
            .unwrap();
        }
        for path in &paths {
            assert_eq!(f.get(path).await.status(), StatusCode::OK, "{path}");
        }
    }

    // ── Attachments ──────────────────────────────────────────

    #[tokio::test]
    async fn attachments_linked_to_public_issues_and_comments_download() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let issue = f.seed_issue(project, "Public issue", "");
        let comment = f.seed_comment(issue, "see attached");
        let on_issue = f.seed_attachment(AttachmentEntity::Issue, issue, "issue.pdf");
        let on_comment = f.seed_attachment(AttachmentEntity::Comment, comment, "comment.pdf");

        for id in [on_issue, on_comment] {
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

        // The metadata rides along with the entities that carry them.
        let json = body_json(f.get("/public/api/projects/PUB/issues/PUB-1").await).await;
        assert_eq!(json["attachments"][0]["id"], on_issue);
        assert!(
            json["attachments"][0].get("sha256").is_none(),
            "the content address is not public metadata"
        );
        let json = body_json(
            f.get("/public/api/projects/PUB/issues/PUB-1/comments")
                .await,
        )
        .await;
        assert_eq!(json["comments"][0]["attachments"][0]["id"], on_comment);
    }

    /// The attachment id space is global and countable, so the download has
    /// to re-derive publication from the link graph on every request. Each
    /// case here is an id an anonymous visitor could reach by typing.
    #[tokio::test]
    async fn attachments_outside_the_published_issue_graph_are_refused() {
        let f = fixture();
        let public = f.seed_project("PUB", true);
        let private = f.seed_project("PRIV", false);
        let public_issue = f.seed_issue(public, "Public issue", "");
        let private_issue = f.seed_issue(private, "Private issue", "");
        let private_comment = f.seed_comment(private_issue, "private");
        let page = f.seed_page(public);
        let doomed = f.seed_issue(public, "Doomed", "");
        let doomed_comment = f.seed_comment(doomed, "going away");

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
                "a page in the published project",
                f.seed_attachment(AttachmentEntity::Page, page, "page.pdf"),
            ),
            ("an orphan with no links", f.seed_orphan_attachment()),
        ];

        // Tombstoning the parent must close the download without anything
        // unlinking the attachment first.
        let on_doomed = f.seed_attachment(AttachmentEntity::Issue, doomed, "doomed.pdf");
        let on_doomed_comment =
            f.seed_attachment(AttachmentEntity::Comment, doomed_comment, "doomedc.pdf");
        f.delete_issue(doomed);

        for (what, id) in cases
            .into_iter()
            .chain([
                ("a deleted parent issue", on_doomed),
                ("a comment on a deleted issue", on_doomed_comment),
            ])
            .chain([("an id that does not exist", 99_999)])
        {
            let response = f
                .get(&format!("/public/api/projects/PUB/attachments/{id}"))
                .await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{what}");
        }

        // The one that must still work, so the test above is not passing by
        // refusing everything.
        let allowed = f.seed_attachment(AttachmentEntity::Issue, public_issue, "ok.pdf");
        assert_eq!(
            f.get(&format!("/public/api/projects/PUB/attachments/{allowed}"))
                .await
                .status(),
            StatusCode::OK
        );
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

    // ── Methods and headers ──────────────────────────────────

    /// Nothing on this surface writes. The routing table only knows `get`,
    /// so every other verb is refused before a handler exists to refuse it.
    #[tokio::test]
    async fn every_mutating_method_is_refused() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let issue = f.seed_issue(project, "Public issue", "");
        f.seed_comment(issue, "hello");
        let attachment = f.seed_attachment(AttachmentEntity::Issue, issue, "spec.pdf");

        let paths = [
            "/public/api/projects/PUB".to_string(),
            "/public/api/projects/PUB/issues".to_string(),
            "/public/api/projects/PUB/issues/PUB-1".to_string(),
            "/public/api/projects/PUB/issues/PUB-1/comments".to_string(),
            format!("/public/api/projects/PUB/attachments/{attachment}"),
        ];
        for path in &paths {
            for method in ["POST", "PUT", "PATCH", "DELETE"] {
                let response = f.request(method, path).await;
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
            ("/public/api/projects/PUB/issues", "GET"),
            ("/public/api/projects/PUB/issues/PUB-1", "GET"),
            // a 404
            ("/public/api/projects/NOPE", "GET"),
            // a 405
            ("/public/api/projects/PUB/issues", "POST"),
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

    /// The contract excludes account metadata, rosters and history. This
    /// walks every public response and fails on any of their field names or
    /// values, which is cheaper to keep honest than a per-DTO review.
    #[tokio::test]
    async fn public_json_carries_no_identity_or_history() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let issue = f.seed_issue(project, "Public issue", "body");
        f.seed_comment(issue, "a comment");
        f.seed_attachment(AttachmentEntity::Issue, issue, "spec.pdf");

        let forbidden = [
            "owner@test.local",
            "password_hash",
            "user_id",
            "uploader_id",
            "author",
            "username",
            "is_admin",
            "email",
            "sha256",
            "audit",
            "lead_user_id",
            "seq",
        ];
        for path in [
            "/public/api/projects/PUB",
            "/public/api/projects/PUB/issues",
            "/public/api/projects/PUB/issues/PUB-1",
            "/public/api/projects/PUB/issues/PUB-1/comments",
        ] {
            let body = body_string(f.get(path).await).await;
            for needle in forbidden {
                assert!(
                    !body.contains(needle),
                    "{path} exposed {needle:?} in: {body}"
                );
            }
        }
    }

    // ── Comment paging ──────────────────────────────────────

    /// A thread longer than one page is fully readable by paging, not
    /// truncated with an apology. `total` is what lets a client say so.
    #[tokio::test]
    async fn a_long_comment_thread_is_paged_not_truncated() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let issue = f.seed_issue(project, "Chatty", "");
        for n in 0..(q::PUBLIC_COMMENT_PAGE + 7) {
            f.seed_comment(issue, &format!("comment {n}"));
        }

        let mut seen: Vec<String> = Vec::new();
        let mut offset = 0;
        loop {
            let json = body_json(
                f.get(&format!(
                    "/public/api/projects/PUB/issues/PUB-1/comments?offset={offset}"
                ))
                .await,
            )
            .await;
            assert_eq!(json["total"], q::PUBLIC_COMMENT_PAGE + 7);
            let page = json["comments"].as_array().unwrap().clone();
            for comment in &page {
                seen.push(comment["content"].as_str().unwrap().to_string());
            }
            if !json["has_more"].as_bool().unwrap() {
                break;
            }
            offset += page.len() as i64;
            assert!(offset < 1000, "comment paging did not terminate");
        }

        let expected: Vec<String> = (0..(q::PUBLIC_COMMENT_PAGE + 7))
            .map(|n| format!("comment {n}"))
            .collect();
        assert_eq!(seen, expected);
    }

    #[tokio::test]
    async fn comment_page_size_is_clamped() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let issue = f.seed_issue(project, "Chatty", "");
        for n in 0..3 {
            f.seed_comment(issue, &format!("comment {n}"));
        }
        for (query, limit) in [
            ("?limit=99999", q::PUBLIC_COMMENT_PAGE),
            ("?limit=0", 1),
            ("?offset=-4", q::PUBLIC_COMMENT_PAGE),
        ] {
            let json = body_json(
                f.get(&format!(
                    "/public/api/projects/PUB/issues/PUB-1/comments{query}"
                ))
                .await,
            )
            .await;
            assert_eq!(json["limit"], limit, "{query}");
            assert!(json["offset"].as_i64().unwrap() >= 0, "{query}");
        }
    }

    // ── Byte budgets ────────────────────────────────────────

    /// A page is bounded in bytes as well as rows, and the bound is applied
    /// from a conservative JSON preflight, so oversized text is never read.
    #[tokio::test]
    async fn a_page_stops_at_the_byte_budget_and_says_so() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let issue = f.seed_issue(project, "Chatty", "");
        // Two rows exceed the conservative six-bytes-per-input-byte budget.
        let big = q::PUBLIC_PAGE_BYTES / 12 + 1024;
        f.seed_oversized_comment(issue, big);
        f.seed_oversized_comment(issue, big);
        f.seed_comment(issue, "small");

        let json = body_json(
            f.get("/public/api/projects/PUB/issues/PUB-1/comments")
                .await,
        )
        .await;
        assert_eq!(
            json["comments"].as_array().unwrap().len(),
            1,
            "the page stops before the byte budget is exceeded"
        );
        assert_eq!(json["has_more"], true, "the rest must remain reachable");
        assert_eq!(json["total"], 3);

        // And the rest is genuinely reachable from the reported offset.
        let json = body_json(
            f.get("/public/api/projects/PUB/issues/PUB-1/comments?offset=1")
                .await,
        )
        .await;
        assert_eq!(json["comments"].as_array().unwrap().len(), 2);
        assert_eq!(json["has_more"], false);
    }

    /// A single row past the whole budget (legacy or imported data) is a
    /// visible error, never a silently empty page a client would ask for
    /// forever.
    #[tokio::test]
    async fn one_oversized_row_is_refused_with_a_visible_error() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let issue = f.seed_issue(project, "Chatty", "");
        f.seed_oversized_comment(issue, q::PUBLIC_PAGE_BYTES + 1024);

        let response = f
            .get("/public/api/projects/PUB/issues/PUB-1/comments")
            .await;
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let body = body_string(response).await;
        assert!(body.contains("public response"), "{body}");
    }

    /// The same rule on an issue body, which has no page to stop short of.
    #[tokio::test]
    async fn an_oversized_issue_description_is_refused() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let issue = f.seed_issue(project, "Huge", "");
        f.set_description(issue, &"y".repeat(q::PUBLIC_PAGE_BYTES + 1024));

        let response = f.get("/public/api/projects/PUB/issues/PUB-1").await;
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        // The list still works: one unreadable body does not close the project.
        assert_eq!(
            f.get("/public/api/projects/PUB/issues").await.status(),
            StatusCode::OK
        );
    }

    // ── Pagination (Sol, low) ────────────────────────────────

    /// The bug this replaced: `has_more` was `count >= limit`, so a project
    /// holding exactly one page of issues advertised a next page that was
    /// empty. The flag is now derived from a row fetched past the end, so the
    /// boundary case is the interesting one and it is checked exactly.
    #[tokio::test]
    async fn a_project_holding_exactly_one_page_does_not_claim_a_next_page() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        for n in 0..5 {
            f.seed_issue(project, &format!("Issue {n}"), "");
        }

        let json = body_json(f.get("/public/api/projects/PUB/issues?limit=5").await).await;
        assert_eq!(json["issues"].as_array().unwrap().len(), 5);
        assert_eq!(json["has_more"], false, "exactly a full page is not 'more'");

        let json = body_json(f.get("/public/api/projects/PUB/issues?limit=4").await).await;
        assert_eq!(json["issues"].as_array().unwrap().len(), 4);
        assert_eq!(json["has_more"], true);
    }

    /// Walking the pages returns every issue exactly once, in sequence order,
    /// and stops on its own.
    #[tokio::test]
    async fn paging_walks_the_whole_project_without_gaps_or_repeats() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        for n in 0..23 {
            f.seed_issue(project, &format!("Issue {n}"), "");
        }

        let mut seen: Vec<String> = Vec::new();
        let mut offset = 0;
        loop {
            let json = body_json(
                f.get(&format!(
                    "/public/api/projects/PUB/issues?limit=10&offset={offset}"
                ))
                .await,
            )
            .await;
            for issue in json["issues"].as_array().unwrap() {
                seen.push(issue["identifier"].as_str().unwrap().to_string());
            }
            assert_eq!(json["offset"], offset);
            assert_eq!(json["limit"], 10);
            if !json["has_more"].as_bool().unwrap() {
                break;
            }
            offset += 10;
            assert!(offset < 1000, "paging did not terminate");
        }

        let expected: Vec<String> = (1..=23).map(|n| format!("PUB-{n}")).collect();
        assert_eq!(seen, expected);
    }

    /// A hand-typed page size is clamped, never rejected: a `400` on a public
    /// endpoint would tell a stranger where the ceiling is and gain nothing.
    /// The echoed `limit` is how a client sees that it was clamped.
    #[tokio::test]
    async fn page_size_is_clamped_rather_than_rejected() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        for n in 0..3 {
            f.seed_issue(project, &format!("Issue {n}"), "");
        }

        for (query, expected_limit, expected_offset) in [
            ("?limit=99999", q::PUBLIC_ISSUE_PAGE, 0),
            ("?limit=0", 1, 0),
            ("?limit=-5", 1, 0),
            ("?offset=-5", q::PUBLIC_ISSUE_PAGE, 0),
            ("", q::PUBLIC_ISSUE_PAGE, 0),
        ] {
            let response = f
                .get(&format!("/public/api/projects/PUB/issues{query}"))
                .await;
            assert_eq!(response.status(), StatusCode::OK, "{query}");
            let json = body_json(response).await;
            assert_eq!(json["limit"], expected_limit, "{query}");
            assert_eq!(json["offset"], expected_offset, "{query}");
        }

        // Past the end is an empty page, not an error.
        let json = body_json(f.get("/public/api/projects/PUB/issues?offset=500").await).await;
        assert_eq!(json["issues"].as_array().unwrap().len(), 0);
        assert_eq!(json["has_more"], false);
    }

    // ── Query count (Sol, medium) ────────────────────────────

    /// The list used to issue one label query per issue, so a page cost 101
    /// statements. This counts what SQLite actually prepared and asserts the
    /// cost is flat in the page size: a real regression guard, not a comment
    /// claiming the batching exists.
    #[tokio::test]
    async fn a_list_read_costs_a_fixed_number_of_statements() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        for n in 0..40 {
            let issue = f.seed_issue(project, &format!("Issue {n}"), "");
            f.add_label(project, issue, &format!("label-{}", n % 4));
        }

        let before = f.statements_run();
        let json = body_json(f.get("/public/api/projects/PUB/issues?limit=40").await).await;
        let cost_40 = f.statements_run() - before;

        // Labels genuinely came back, so the cheap version is not cheap by
        // being wrong.
        assert_eq!(json["issues"][0]["labels"][0], "label-0");
        assert_eq!(json["issues"][5]["labels"][0], "label-1");

        // The same read over a page of 4.
        let before = f.statements_run();
        let _ = f.get("/public/api/projects/PUB/issues?limit=4").await;
        let cost_4 = f.statements_run() - before;

        assert_eq!(
            cost_40, cost_4,
            "a 40-issue page cost {cost_40} statements and a 4-issue page {cost_4}; \
             the label lookup is per-issue again"
        );
        assert!(
            cost_40 <= 7,
            "a list read should be a handful of statements, took {cost_40}"
        );
    }

    /// Same guarantee for a comment thread, whose attachments were also a
    /// query per row.
    #[tokio::test]
    async fn a_comment_read_costs_a_fixed_number_of_statements() {
        let f = fixture();
        let project = f.seed_project("PUB", true);
        let issue = f.seed_issue(project, "Chatty", "");
        for n in 0..30 {
            let comment = f.seed_comment(issue, &format!("comment {n}"));
            f.seed_attachment(AttachmentEntity::Comment, comment, &format!("f{n}.pdf"));
        }

        let before = f.statements_run();
        let json = body_json(
            f.get("/public/api/projects/PUB/issues/PUB-1/comments?limit=30")
                .await,
        )
        .await;
        let cost_30 = f.statements_run() - before;
        assert_eq!(json["comments"].as_array().unwrap().len(), 30);
        assert!(
            json["comments"][0]["attachments"][0]["id"].is_i64(),
            "attachments must still be attached to the right comment"
        );

        let before = f.statements_run();
        let _ = f
            .get("/public/api/projects/PUB/issues/PUB-1/comments?limit=3")
            .await;
        let cost_3 = f.statements_run() - before;
        assert_eq!(
            cost_30, cost_3,
            "a 30-comment page cost {cost_30} statements and a 3-comment page {cost_3}; \
             the attachment lookup is per-comment again"
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

    #[test]
    fn issue_identifiers_are_only_accepted_for_their_own_project() {
        assert_eq!(split_issue_identifier("PUB", "PUB-1"), Some(1));
        assert_eq!(split_issue_identifier("PUB", "pub-42"), Some(42));
        assert_eq!(split_issue_identifier("PUB", "OTHER-1"), None);
        assert_eq!(split_issue_identifier("PUB", "PUB-DOC-1"), None);
        assert_eq!(split_issue_identifier("PUB", "PUB-0x1"), None);
        assert_eq!(split_issue_identifier("PUB", "PUB--1"), None);
        assert_eq!(split_issue_identifier("PUB", "PUB-"), None);
        assert_eq!(split_issue_identifier("PUB", "1"), None);
        assert_eq!(split_issue_identifier("PUB", ""), None);
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
