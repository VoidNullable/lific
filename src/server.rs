//! HTTP server construction and startup.
//!
//! [`build_app`] is the single place the production router is assembled:
//! REST API, MCP (authed and the optional authless path token), OAuth, the
//! embedded frontend, and every middleware layer and extension they depend
//! on. [`run`] is `lific start`: guard rails, background tasks, bind, serve.
//!
//! Anything that wants to exercise the real request path (notably `lific
//! doctor`) calls `build_app` rather than hand-assembling a lookalike router
//! that can drift away from what production actually runs.

use std::{net::SocketAddr, sync::Arc};

use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{HeaderName, HeaderValue, Method, StatusCode, header},
    middleware,
    response::IntoResponse,
    routing::{any, get},
};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager,
    tower::{StreamableHttpServerConfig, StreamableHttpService},
};
use rust_embed::Embed;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{Any, CorsLayer};
use tracing::{info, warn};

use api_keys_simplified::ApiKeyManagerV0;

use crate::config::{self, Config};
use crate::{
    actor, api, auth, backup, db, links, mcp, oauth, ratelimit, realtime, resolve_caller, storage,
};

/// Embedded frontend assets compiled from web/dist/.
/// Falls back gracefully if dist/ doesn't exist (e.g. dev builds without frontend).
#[derive(Embed)]
#[folder = "web/dist/"]
#[allow(dead_code)]
struct WebAssets;

/// Serve an embedded static file, or fall back to index.html for SPA routing.
async fn serve_frontend(uri: axum::http::Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');

    // Try the exact path first (e.g. assets/index-abc.js)
    if let Some(file) = WebAssets::get(path) {
        let mime = mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string();
        // Vite emits content-hashed filenames under assets/ (e.g.
        // index-xkSiPCqs.js), so those are safe to cache forever — a new
        // build changes the hash and thus the URL. Everything else
        // (index.html, favicon) stays uncached so a redeploy is picked up
        // immediately.
        let cache_control = if path.starts_with("assets/") {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        return (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, mime),
                (header::CACHE_CONTROL, cache_control.to_string()),
            ],
            file.data.to_vec(),
        )
            .into_response();
    }

    // SPA fallback: serve index.html for all unmatched routes. Same
    // no-cache as the exact-file branch above: this IS index.html, so a
    // cached copy pins the browser to the previous build's asset URLs and a
    // redeploy is invisible until a hard refresh.
    match WebAssets::get("index.html") {
        Some(file) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "text/html".to_string()),
                (header::CACHE_CONTROL, "no-cache".to_string()),
            ],
            file.data.to_vec(),
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            "Frontend not built. Run: cd web && bun run build",
        )
            .into_response(),
    }
}

/// How far this instance can be reached from, as configured.
///
/// LIF-406: the guard rails around login-free mode and single-user web
/// auto-login used to ask only about the bind host. A loopback bind sitting
/// behind a public reverse proxy (Tailscale Funnel, nginx, Cloudflare) passed
/// that check while being reachable from the open internet, which is exactly
/// the deployment the guard rails exist for. The bind host and the advertised
/// `public_url` are both part of the answer, so both live here and every
/// caller asks the same question of the same type.
///
/// Layered as an axum extension by [`build_app`] so the runtime settings
/// update can re-run the startup guard before it persists anything.
#[derive(Debug, Clone)]
pub struct Reachability {
    /// `[server] host`: the address the listener binds.
    pub host: String,
    /// `[server] public_url`: the address the instance advertises, if any.
    pub public_url: Option<String>,
}

impl Reachability {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            host: cfg.server.host.clone(),
            public_url: cfg.server.public_url.clone(),
        }
    }

    /// Why this instance is reachable from beyond the local machine, or
    /// `None` when it genuinely is not: a loopback bind whose `public_url` is
    /// absent, loopback, or on a private network.
    ///
    /// The string is a clause ("[server] host (0.0.0.0) is not loopback"), so
    /// each caller can wrap it in the refusal that fits its surface.
    ///
    /// Conservative in both directions: an unparseable `public_url`, or one
    /// whose host cannot be positively classified as local, counts as
    /// publicly reachable. Guessing wrong in the other direction hands an
    /// admin session to the internet.
    pub fn public_exposure(&self) -> Option<String> {
        if !config::is_localhost_host(&self.host) {
            return Some(format!("[server] host ({}) is not loopback", self.host));
        }
        let url = self
            .public_url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())?;
        match public_url_host(url) {
            Some(host) if is_private_host(&host) => None,
            Some(host) => Some(format!(
                "[server] public_url ({url}) points at {host}, which is not a \
                 loopback or private-network address"
            )),
            None => Some(format!(
                "[server] public_url ({url}) has no host that can be read as \
                 loopback or private"
            )),
        }
    }
}

/// The host component of a configured `public_url`, if it has one. A value
/// with no authority (`example.com`, `/lific`) yields `None`, which callers
/// treat as "cannot be assumed local".
fn public_url_host(url: &str) -> Option<String> {
    let uri = url.parse::<axum::http::Uri>().ok()?;
    uri.authority().map(|a| a.host().to_string())
}

/// Whether a hostname names something only reachable from this machine or
/// from a private network.
///
/// Positive identification only: loopback and the RFC 1918 / ULA /
/// link-local ranges, plus the hostname suffixes reserved for local naming
/// (`localhost`, mDNS `.local`, `.internal`, `.home.arpa`, `.lan`). Anything
/// else, a public DNS name or a routable address, is not private. Note that
/// a tailnet name such as `magi.tailb93ac8.ts.net` is a public DNS name and
/// is treated as such: it is precisely the Funnel case LIF-406 is about.
fn is_private_host(host: &str) -> bool {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    if config::is_localhost_host(host) {
        return true;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return is_private_ip(ip);
    }
    let lower = host.to_ascii_lowercase();
    [".localhost", ".local", ".internal", ".home.arpa", ".lan"]
        .iter()
        .any(|suffix| lower.ends_with(suffix))
}

fn is_private_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        std::net::IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            v6.is_loopback()
                // Unique local addresses, fc00::/7.
                || (first & 0xfe00) == 0xfc00
                // Link-local unicast, fe80::/10.
                || (first & 0xffc0) == 0xfe80
        }
    }
}

/// Assemble the full production router.
///
/// Everything a request can hit lives here: the REST API, the authed `/mcp`
/// endpoint, the optional authless `/mcp/<token>` escape hatch, the OAuth
/// routes, and the embedded frontend fallback, wrapped in the middleware and
/// extensions they expect (auth, rate limiters, attachment storage, realtime
/// hub, CORS, body limit, compression).
///
/// Side-effect free apart from a DB read to resolve the authless MCP identity:
/// background tasks (backups, attachment GC) are [`run`]'s business, so tests
/// and diagnostics can build the app without spawning anything.
#[cfg_attr(not(test), allow(dead_code))]
pub fn build_app(
    cfg: &Config,
    pool: db::DbPool,
    manager: ApiKeyManagerV0,
    realtime: realtime::RealtimeHub,
    trusted_proxies: Arc<[ratelimit::IpNetwork]>,
) -> Router {
    build_app_with_store(
        cfg,
        pool,
        manager,
        realtime,
        trusted_proxies,
        storage::AttachmentStore::from_db_path(&cfg.database.path),
    )
}

pub(crate) fn build_app_with_store(
    cfg: &Config,
    pool: db::DbPool,
    manager: ApiKeyManagerV0,
    realtime: realtime::RealtimeHub,
    trusted_proxies: Arc<[ratelimit::IpNetwork]>,
    attachment_store: storage::AttachmentStore,
) -> Router {
    // Auth state for middleware. When no public_url is configured the
    // issuer is derived from the bind address — but 0.0.0.0/:: are
    // bind-any addresses, not dialable URLs. They leak into
    // user-facing links (OAuth metadata, device verification_uri), so
    // map them to loopback.
    let issuer = cfg.server.public_url.clone().unwrap_or_else(|| {
        let host = match cfg.server.host.as_str() {
            "0.0.0.0" | "::" | "[::]" => "127.0.0.1",
            h => h,
        };
        format!("http://{}:{}", crate::display_host(host), cfg.server.port)
    });

    let manager_ext = Arc::new(manager.clone());

    let auth_state = auth::AuthState {
        db: pool.clone(),
        manager,
        public_url: issuer.clone(),
        required: cfg.auth.required,
    };

    // MCP StreamableHTTP service
    let db_for_mcp = pool.clone();
    let realtime_for_mcp = realtime.clone();
    let mut mcp_allowed_hosts: Vec<String> =
        vec!["localhost".into(), "127.0.0.1".into(), "::1".into()];

    // If public_url is set, allow its hostname through the DNS rebinding check
    // so reverse proxies (Tailscale funnel, nginx, etc.) can forward requests.
    if let Some(ref url) = cfg.server.public_url
        && let Ok(parsed) = url.parse::<axum::http::Uri>()
        && let Some(authority) = parsed.authority()
    {
        let host: String = authority.host().to_string();
        mcp_allowed_hosts.push(host);
    }

    let mcp_config = StreamableHttpServerConfig::default()
        .with_stateful_mode(false)
        .with_json_response(true)
        .with_allowed_hosts(mcp_allowed_hosts.clone());

    let mcp_service = StreamableHttpService::new(
        move || {
            Ok(mcp::LificMcp::with_realtime(
                db_for_mcp.clone(),
                realtime_for_mcp.clone(),
            ))
        },
        Arc::new(LocalSessionManager::default()),
        mcp_config,
    );

    // Login rate limiter: 5 attempts per 15 minutes per identity
    let login_limiter = Arc::new(ratelimit::RateLimiter::new(
        5,
        std::time::Duration::from_secs(15 * 60),
    ));

    // LIF-262: attachment storage + upload guards. Bytes live in a
    // sidecar dir next to the database (content-addressed); the upload
    // route is rate-limited per user (30 uploads / 10 min).
    let attachment_config = api::AttachmentConfig::default();
    let attachment_upload_limiter = Arc::new(api::AttachmentUploadLimiter(
        ratelimit::RateLimiter::new(30, std::time::Duration::from_secs(10 * 60)),
    ));

    // Routes behind auth: REST API + MCP
    let mcp_public_url = cfg.server.public_url.clone();
    let mcp_allowed_hosts_for_links = mcp_allowed_hosts.clone();
    let authed_routes = api::router(pool.clone(), &cfg.server.cors_origins)
        .route(
            "/mcp",
            any(move |request: Request<Body>| async move {
                // Extract the authenticated user (set by auth middleware)
                // and store it for MCP tools to read. Serialized to prevent
                // concurrent requests from overwriting each other's identity.
                let auth_user = request
                    .extensions()
                    .get::<Option<db::models::AuthUser>>()
                    .cloned()
                    .flatten();

                let issue_links = links::IssueLinkContext::for_http_request(
                    mcp_public_url.as_deref(),
                    request
                        .headers()
                        .get(header::HOST)
                        .and_then(|value| value.to_str().ok()),
                    &mcp_allowed_hosts_for_links,
                );

                mcp::with_request_context(auth_user, issue_links, || async {
                    mcp_service.handle(request).await.into_response()
                })
                .await
            }),
        )
        .layer(axum::Extension(realtime.clone()))
        .layer(axum::Extension(login_limiter))
        .layer(axum::Extension(trusted_proxies.clone()))
        .layer(axum::Extension(attachment_store.clone()))
        .layer(axum::Extension(attachment_config))
        .layer(axum::Extension(attachment_upload_limiter))
        .layer(axum::Extension(crate::config::AuthConfig::from_server(
            &cfg.auth,
            cfg.server.public_url.as_deref(),
        )))
        // LIF-406: the settings update path re-runs the startup reachability
        // guard before it may turn web auto-login on.
        .layer(axum::Extension(Reachability::from_config(cfg)))
        .layer(axum::Extension(manager_ext))
        .layer(middleware::from_fn_with_state(
            auth_state,
            auth_middleware_wrapper,
        ));

    // OAuth client registration rate limiter: 10 clients per IP per hour.
    // /oauth/register is unauthenticated per RFC 7591; without this anyone
    // can flood the server with throwaway clients (LIF-64).
    let oauth_register_limiter = Arc::new(ratelimit::RateLimiter::new(
        10,
        std::time::Duration::from_secs(60 * 60),
    ));

    let oauth_state = oauth::OAuthState {
        db: pool.clone(),
        issuer,
        // LIF-287: an explicit public_url is advertised as-is; only a
        // bind-derived issuer may be replaced per-request by an
        // allowlisted Host header.
        issuer_is_explicit: cfg.server.public_url.is_some(),
        allowed_hosts: mcp_allowed_hosts.clone().into(),
        register_limiter: oauth_register_limiter,
        trusted_proxies: trusted_proxies.clone(),
    };

    // Optional authless MCP escape hatch at /mcp/<token> (see the
    // mcp_path_token config docs). Resolved identity for attribution:
    // the configured username, else the first admin, else anonymous.
    let authless_mcp_router: Option<Router> = cfg
        .server
        .mcp_path_token
        .clone()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .map(|token| {
            let authless_user: Option<db::models::AuthUser> = {
                match pool.read() {
                    Ok(conn) => match cfg.server.mcp_path_user.as_deref() {
                        Some(uname) => db::queries::users::get_user_by_username(&conn, uname)
                            .ok()
                            .map(|u| db::models::AuthUser {
                                id: u.id,
                                username: u.username,
                                display_name: u.display_name,
                                is_admin: u.is_admin,
                            }),
                        // LIFIC-8: the "no credential → first admin"
                        // fallback is consolidated in `resolve_caller`.
                        None => {
                            resolve_caller::resolve_caller_conn(&conn, None, actor::Transport::Mcp)
                                .ok()
                                .flatten()
                                .map(|i| i.user)
                        }
                    },
                    Err(_) => None,
                }
            };
            info!(
                acting_as = authless_user
                    .as_ref()
                    .map_or("<anonymous>", |u| u.username.as_str()),
                "authless MCP endpoint enabled at /mcp/<token>"
            );
            build_authless_mcp_router(
                pool.clone(),
                &token,
                authless_user,
                mcp_allowed_hosts.clone(),
                cfg.server.public_url.clone(),
                realtime.clone(),
            )
        });

    let app = authed_routes.merge(oauth::router(oauth_state));
    let app = match authless_mcp_router {
        Some(r) => app.merge(r),
        None => app,
    };
    // LIF-465: the anonymous project view. Merged here, *after* the auth
    // middleware has been layered onto `authed_routes`, so it is genuinely
    // outside that middleware rather than carved out of it by path. Nothing in
    // `api::public` can see an identity extension, an auth config or the API
    // key manager, because none of them are layered onto this router. Only
    // published projects are reachable through it (the flag is checked in the
    // SQL of every read), and only with `GET`.
    let app = app.merge(api::public::router(pool, attachment_store, trusted_proxies));
    app.fallback(get(serve_frontend))
        // Top-level CORS layer.
        //
        // This wraps EVERYTHING (REST API, /mcp, OAuth, frontend). Two
        // things matter here:
        //
        // 1. `CorsLayer` intercepts OPTIONS preflight requests and
        //    short-circuits them with a 204 — they never reach the auth
        //    middleware. Without this, browser MCP clients like Claude
        //    Web get their preflight rejected with 401 and the actual
        //    POST is never sent.
        //
        // 2. We expose MCP-specific headers (`mcp-session-id`,
        //    `www-authenticate`) and accept the request headers MCP
        //    clients send (`mcp-protocol-version`, `mcp-session-id`,
        //    `last-event-id` for SSE resumption).
        //
        // The internal CORS layer inside `api::router()` still runs for
        // /api/* but is effectively shadowed by this outer one.
        .layer(build_global_cors(&cfg.server.cors_origins))
        .layer(axum::extract::DefaultBodyLimit::max(2 * 1024 * 1024)) // 2 MB
        // Gzip/brotli compression for text responses. The embedded
        // frontend ships a ~1 MB JS bundle that was previously served
        // raw — uncompressed it took ~6-9s to transfer over the
        // tailnet, blocking first paint (and everything behind it).
        // CompressionLayer's DefaultPredicate already skips SSE
        // (text/event-stream — so MCP streaming is untouched), gRPC,
        // already-compressed images, and bodies under 32 bytes.
        .layer(CompressionLayer::new())
}

/// `lific start`: bring up the HTTP server for `cfg` and serve until a
/// shutdown signal arrives.
pub async fn run(cfg: &Config) -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| format!("lific={}", cfg.log.level).into()),
        )
        .with_writer(crate::cli::term::sanitized_stderr())
        .init();

    // Parse trusted proxy CIDRs once at startup. Invalid entries must
    // stop the server rather than quietly disabling the trust boundary
    // around client-IP rate-limit and audit keys.
    let trusted_proxies = Arc::<[ratelimit::IpNetwork]>::from(
        cfg.server
            .trusted_proxy_ranges()
            .map_err(|error| format!("invalid server.trusted_proxies: {error}"))?,
    );

    // LIF-294: guard rails for auth-optional mode. Refuse outright
    // when the instance says it's publicly reachable; shout otherwise
    // (the default bind is 0.0.0.0 — the whole LAN can reach it).
    //
    // LIF-406: "publicly reachable" is [`Reachability`]'s question, not a
    // bind-host string comparison, so a loopback bind behind a public reverse
    // proxy is refused here too rather than sailing through.
    let reachability = Reachability::from_config(cfg);
    if !cfg.auth.required {
        if let Some(exposure) = reachability.public_exposure() {
            return Err(format!(
                "refusing to start: [auth] required = false (login-free mode) while \
                 {exposure}. Anyone who can reach this instance can administer it. \
                 Re-enable auth, bind to 127.0.0.1, or remove the public URL."
            )
            .into());
        }
        warn!(
            host = %cfg.server.host,
            "{} ([auth] required = false)",
            config::login_free_caution()
        );
    }

    let pool = db::open(&cfg.database.path)?;
    info!(path = %cfg.database.path.display(), "database ready");

    // Seed the instance-settings row on first run, taking the initial
    // signup policy from TOML. Once seeded, the DB row is authoritative
    // and admins edit it live via the UI/CLI (LIF-210).
    {
        let conn = pool.write()?;
        db::queries::settings::ensure(&conn, cfg.auth.allow_signup)?;

        // LIF-215: single-user web auto-login hands an admin session to
        // anyone who can load the page. It shares login-free mode's
        // threat model, so it gets the same guard rail (PR #23 review):
        // refuse outright when the instance is publicly reachable — the
        // [auth] required check above doesn't see this DB flag, and a
        // stale or toggled flag must not turn a reachable instance into
        // passwordless admin. On a genuinely local instance with an https
        // public_url on a private network, keep the loud warning.
        let auto_login = db::queries::settings::get(&conn)?.web_auto_login;
        drop(conn);
        if auto_login && let Some(exposure) = reachability.public_exposure() {
            return Err(format!(
                "refusing to start: single-user web auto-login is enabled while \
                 {exposure}. Anyone who can reach this instance gets an admin \
                 session without a password. Disable web auto-login in the \
                 instance settings, bind to 127.0.0.1, or remove the public URL."
            )
            .into());
        }
        if auto_login
            && cfg
                .server
                .public_url
                .as_deref()
                .is_some_and(|u| u.trim().to_ascii_lowercase().starts_with("https://"))
        {
            warn!(
                "web_auto_login is ENABLED while public_url is https — anyone who can \
                 reach this instance gets an admin session without a password. Only \
                 enable single-user mode on a private/local instance."
            );
        }
    }

    // Key manager for auth
    let manager =
        auth::create_key_manager().map_err(|e| format!("key manager init failed: {e}"))?;

    // Auto-generate a key if none exist and no human operator exists
    // yet (LIFIC-9: once a human exists we stop auto-minting the
    // unbound "default" key — keys are minted on demand). The three
    // branches are mutually exclusive by construction:
    //   1. empty bootstrap (no human, no keys) → mint the default key
    //   2. human present, still no keys → passwordless mode
    //   3. keys exist → plain count
    if auth::should_mint_initial_key(&pool) {
        let key = auth::create_api_key(&pool, &manager, "default", None)?;
        info!("no API keys found, auto-generated initial key");
        print_initial_key(&key);
    } else if !auth::has_any_keys(&pool) {
        // A human operator exists (should_mint was false for lack of
        // keys alone) but no key has been created yet: keys are minted
        // on demand via `lific key create`.
        info!(
            "human operator present — passwordless mode; mint keys on demand with `lific key create`"
        );
    } else {
        let count = auth::list_api_keys(&pool)?
            .iter()
            .filter(|k| !k.revoked)
            .count();
        info!(active_keys = count, "API key auth enabled");
    }

    // Start backup task
    if cfg.backup.enabled {
        let pool_arc = Arc::new(pool.clone());
        backup::start_backup_task(pool_arc, cfg.database.path.clone(), cfg.backup.clone());
        info!(
            dir = %cfg.backup_dir().display(),
            interval = %format!("{}m", cfg.backup.interval_minutes),
            retain = cfg.backup.retain,
            "automatic backups enabled"
        );
    }

    // Empty the trash on a schedule (LIF-438). A delete is a tombstone now, so
    // something has to collect them once they are past the recovery window.
    crate::retention::start_trash_purge_task(pool.clone(), cfg.retention.trash_days);

    // Sweep abandoned (unlinked) attachments hourly. Share the store with the
    // request router so its operation lock covers both upload and GC paths.
    let attachment_store = storage::AttachmentStore::from_db_path(&cfg.database.path);
    let gc_attachment_store = attachment_store.clone();
    storage::start_gc_task(pool.clone(), gc_attachment_store);

    let app = build_app_with_store(
        cfg,
        pool.clone(),
        manager,
        realtime::RealtimeHub::new(),
        trusted_proxies,
        attachment_store,
    );

    let addr = format!("{}:{}", cfg.server.host, cfg.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!(addr = %addr, "lific server started (REST + MCP + OAuth at /mcp)");

    let shutdown_pool = pool.clone();
    let server = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal(shutdown_pool));
    server.await?;
    Ok(())
}

/// Print the one-time initial API key. No box-drawing: keys are longer than
/// any fixed-width frame (the old box rendered broken), and plain lines are
/// easier to copy.
fn print_initial_key(key: &str) {
    println!();
    println!("  Initial API key — save it now, it will not be shown again:");
    println!();
    println!("    {key}");
    println!();
    println!("  Use it as: Authorization: Bearer <key>");
    println!();
}

/// Build the top-level CORS layer applied to the entire app.
///
/// When `cors_origins` is empty, allows any origin (suitable for a local-first
/// tool exposed via Tailscale Funnel where the auth layer is the real gate).
/// Otherwise, allows only the listed origins.
///
/// Methods, request headers, and exposed response headers are all configured
/// for the union of REST + MCP needs. Notably we accept the MCP transport
/// headers (`mcp-protocol-version`, `mcp-session-id`, `last-event-id`) and
/// expose `mcp-session-id` and `www-authenticate` so MCP clients can read
/// the session id back and so 401 responses surface the resource metadata.
fn build_global_cors(cors_origins: &[String]) -> CorsLayer {
    let layer = CorsLayer::new()
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::ACCEPT,
            HeaderName::from_static("mcp-protocol-version"),
            HeaderName::from_static("mcp-session-id"),
            HeaderName::from_static("last-event-id"),
        ])
        .expose_headers([
            header::WWW_AUTHENTICATE,
            HeaderName::from_static("mcp-session-id"),
            // LIF-421: comment paging metadata. See `api::comments`.
            HeaderName::from_static(crate::api::comments::HAS_MORE_HEADER),
            HeaderName::from_static(crate::api::comments::NEXT_OFFSET_HEADER),
            HeaderName::from_static(crate::api::comments::RETURNED_HEADER),
            HeaderName::from_static(crate::api::comments::NEXT_CURSOR_AT_HEADER),
            HeaderName::from_static(crate::api::comments::NEXT_CURSOR_ID_HEADER),
        ])
        .max_age(std::time::Duration::from_secs(86400));

    if cors_origins.is_empty() {
        layer.allow_origin(Any)
    } else {
        let origins: Vec<HeaderValue> =
            cors_origins.iter().filter_map(|o| o.parse().ok()).collect();
        layer.allow_origin(origins)
    }
}

/// Build the authless MCP router mounted at `/mcp/<token>`.
///
/// This endpoint deliberately bypasses the OAuth/API-key auth middleware: the
/// secret path segment IS the credential. It exists because claude.ai web's
/// OAuth connector flow is currently broken (it finishes the OAuth dance, gets
/// a token, then never sends the authenticated MCP request). An authless server
/// sidesteps that path entirely. Every request is run as a fixed identity
/// (`user`) so MCP tools that attribute actions to a user still work.
///
/// Security: anyone who learns the URL has full MCP access. The token must be
/// long and random, and only served over HTTPS.
fn build_authless_mcp_router(
    pool: db::DbPool,
    token: &str,
    user: Option<db::models::AuthUser>,
    allowed_hosts: Vec<String>,
    public_url: Option<String>,
    realtime: realtime::RealtimeHub,
) -> Router {
    let allowed_hosts_for_links = allowed_hosts.clone();
    let config = StreamableHttpServerConfig::default()
        .with_stateful_mode(false)
        .with_json_response(true)
        .with_allowed_hosts(allowed_hosts);
    let service = StreamableHttpService::new(
        move || Ok(mcp::LificMcp::with_realtime(pool.clone(), realtime.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    Router::new().route(
        &format!("/mcp/{token}"),
        any(move |request: Request<Body>| async move {
            let issue_links = links::IssueLinkContext::for_http_request(
                public_url.as_deref(),
                request
                    .headers()
                    .get(header::HOST)
                    .and_then(|value| value.to_str().ok()),
                &allowed_hosts_for_links,
            );
            mcp::with_request_context(user, issue_links, || async {
                service.handle(request).await.into_response()
            })
            .await
        }),
    )
}

/// Wrapper that skips auth for /api/health
async fn auth_middleware_wrapper(
    state: axum::extract::State<auth::AuthState>,
    request: Request<Body>,
    next: middleware::Next,
) -> axum::response::Response {
    if skips_auth_middleware(request.uri().path()) {
        return next.run(request).await;
    }
    auth::require_api_key(state, request, next).await
}

fn skips_auth_middleware(path: &str) -> bool {
    matches!(
        path,
        "/api/health"
            | "/api/instance"
            | "/api/auth/signup"
            | "/api/auth/login"
            | "/api/auth/auto-login"
            | "/api/events/ws"
            | "/register"
            | "/authorize"
            | "/token"
            | "/revoke"
    ) || path.starts_with("/.well-known/")
        || path.starts_with("/oauth/")
}

/// Wait for SIGINT/SIGTERM, then checkpoint WAL before shutting down.
async fn shutdown_signal(pool: db::DbPool) {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("shutdown signal received, checkpointing WAL...");
    backup::checkpoint_wal(&pool);
    info!("shutdown complete");
}

#[cfg(test)]
mod reachability_tests {
    use super::*;

    fn reach(host: &str, public_url: Option<&str>) -> Reachability {
        Reachability {
            host: host.to_string(),
            public_url: public_url.map(str::to_string),
        }
    }

    /// The setups LIF-406 must keep working: a genuinely local instance,
    /// with no public URL or one that names the same machine or a private
    /// network. These are the login-free installs the guard rails exist to
    /// permit.
    #[test]
    fn a_local_instance_is_not_exposed() {
        for (host, url) in [
            ("127.0.0.1", None),
            ("localhost", None),
            ("::1", None),
            ("127.0.0.1", Some("http://127.0.0.1:3456")),
            ("127.0.0.1", Some("http://localhost:3456")),
            ("127.0.0.1", Some("http://[::1]:3456")),
            ("127.0.0.1", Some("https://10.0.0.5")),
            ("127.0.0.1", Some("https://192.168.1.20:3456")),
            ("127.0.0.1", Some("https://nas.local")),
            ("127.0.0.1", Some("https://lific.home.arpa")),
            ("127.0.0.1", Some("   ")),
        ] {
            assert_eq!(
                reach(host, url).public_exposure(),
                None,
                "host={host} public_url={url:?} should count as local"
            );
        }
    }

    /// The bind-host half of the guard, unchanged from LIF-294.
    #[test]
    fn a_non_loopback_bind_is_exposed() {
        for host in ["0.0.0.0", "::", "192.168.1.10", "not-an-address"] {
            let exposure = reach(host, None).public_exposure().unwrap_or_default();
            assert!(
                exposure.contains("[server] host"),
                "host={host} should be refused on the bind: {exposure:?}"
            );
        }
    }

    /// The hole LIF-406 closes: production binds loopback and is published to
    /// the internet by Tailscale Funnel, which the old bind-host-only check
    /// waved through.
    #[test]
    fn a_loopback_bind_behind_a_public_url_is_exposed() {
        for url in [
            "https://magi.tailb93ac8.ts.net",
            "https://lific.example.com/lific",
            "http://203.0.113.10:3456",
            "https://[2606:4700:4700::1111]",
            // No authority to read, so it cannot be assumed local.
            "lific.example.com",
        ] {
            let exposure = reach("127.0.0.1", Some(url))
                .public_exposure()
                .unwrap_or_default();
            assert!(
                exposure.contains("public_url"),
                "public_url={url} should be refused: {exposure:?}"
            );
        }
    }

    #[test]
    fn from_config_reads_both_halves() {
        let mut cfg = Config::default();
        cfg.server.host = "127.0.0.1".into();
        cfg.server.public_url = Some("https://lific.example.com".into());
        let r = Reachability::from_config(&cfg);
        assert_eq!(r.host, "127.0.0.1");
        assert_eq!(r.public_url.as_deref(), Some("https://lific.example.com"));
        assert!(r.public_exposure().is_some());
    }
}

#[cfg(test)]
mod cors_tests {
    use super::*;
    use axum::routing::post;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[test]
    fn websocket_path_skips_header_auth_middleware() {
        assert!(skips_auth_middleware("/api/events/ws"));
        assert!(!skips_auth_middleware("/api/issues"));
    }

    /// Build a minimal /mcp router behind an auth gate identical in spirit to
    /// the real one (returns 401 if Authorization is missing), wrapped with
    /// our global CORS layer.
    fn app_with_cors(origins: &[String]) -> Router {
        let inner = Router::new().route(
            "/mcp",
            post(|headers: axum::http::HeaderMap| async move {
                if headers.get(header::AUTHORIZATION).is_none() {
                    return (StatusCode::UNAUTHORIZED, "missing auth").into_response();
                }
                (StatusCode::OK, "ok").into_response()
            }),
        );
        inner.layer(build_global_cors(origins))
    }

    /// A browser MCP client (Claude Web) issues a CORS preflight before the
    /// authenticated POST. That preflight must succeed WITHOUT any
    /// Authorization header — otherwise the browser blocks the real request
    /// and the user sees "Authorization with the MCP server failed".
    #[tokio::test]
    async fn cors_preflight_to_mcp_bypasses_auth() {
        let app = app_with_cors(&[]);

        let req = Request::builder()
            .method(Method::OPTIONS)
            .uri("/mcp")
            .header("origin", "https://claude.ai")
            .header("access-control-request-method", "POST")
            .header(
                "access-control-request-headers",
                "authorization,content-type",
            )
            .body(Body::empty())
            .unwrap();

        let res = app.oneshot(req).await.unwrap();

        // tower-http returns 200 OK for valid preflights (not 204, but either
        // is RFC-compliant). The critical thing is NOT 401.
        assert!(
            res.status().is_success(),
            "preflight should succeed without auth, got {}",
            res.status()
        );

        let headers = res.headers();
        assert_eq!(
            headers
                .get("access-control-allow-origin")
                .and_then(|v| v.to_str().ok()),
            Some("*"),
            "empty cors_origins should allow any origin"
        );

        let allow_methods = headers
            .get("access-control-allow-methods")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            allow_methods.contains("POST"),
            "POST must be in allowed methods, got: {allow_methods}"
        );
        assert!(
            allow_methods.contains("PATCH"),
            "PATCH must be in allowed methods, got: {allow_methods}"
        );

        let allow_headers = headers
            .get("access-control-allow-headers")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        assert!(
            allow_headers.contains("authorization"),
            "authorization must be allowed, got: {allow_headers}"
        );
        assert!(
            allow_headers.contains("mcp-session-id"),
            "mcp-session-id must be allowed, got: {allow_headers}"
        );
    }

    /// Real (post-preflight) requests still go through normal auth — CORS
    /// doesn't bypass the auth middleware for the actual call.
    #[tokio::test]
    async fn cors_does_not_bypass_auth_on_real_request() {
        let app = app_with_cors(&[]);

        let req = Request::builder()
            .method(Method::POST)
            .uri("/mcp")
            .header("origin", "https://claude.ai")
            .body(Body::empty())
            .unwrap();

        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    /// When configured with an explicit origin list, only those origins
    /// receive an Access-Control-Allow-Origin header echoing them back.
    #[tokio::test]
    async fn explicit_origins_are_allowlisted() {
        let app = app_with_cors(&["https://claude.ai".to_string()]);

        let req = Request::builder()
            .method(Method::OPTIONS)
            .uri("/mcp")
            .header("origin", "https://claude.ai")
            .header("access-control-request-method", "POST")
            .body(Body::empty())
            .unwrap();

        let res = app.oneshot(req).await.unwrap();
        assert!(res.status().is_success());
        assert_eq!(
            res.headers()
                .get("access-control-allow-origin")
                .and_then(|v| v.to_str().ok()),
            Some("https://claude.ai")
        );
    }

    /// MCP responses must expose the session id header so the client can
    /// read it back — without `Access-Control-Expose-Headers`, browser
    /// JS can't see custom response headers cross-origin.
    #[tokio::test]
    async fn mcp_session_id_is_exposed() {
        // We make a synthetic GET that returns 200 with a header. The
        // preflight response also carries the expose-headers field, so we
        // check it there for simplicity.
        let app = app_with_cors(&[]);

        let req = Request::builder()
            .method(Method::OPTIONS)
            .uri("/mcp")
            .header("origin", "https://claude.ai")
            .header("access-control-request-method", "POST")
            .body(Body::empty())
            .unwrap();

        let res = app.oneshot(req).await.unwrap();
        // Expose-Headers is sent on actual responses, not preflight, in tower-http.
        // So instead, fire a real (failing) request and check exposed headers.
        let _ = res.into_body().collect().await;

        let app = app_with_cors(&[]);
        let req = Request::builder()
            .method(Method::POST)
            .uri("/mcp")
            .header("origin", "https://claude.ai")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        let expose = res
            .headers()
            .get("access-control-expose-headers")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        assert!(
            expose.contains("mcp-session-id"),
            "mcp-session-id must be exposed, got: {expose}"
        );
        assert!(
            expose.contains("www-authenticate"),
            "www-authenticate must be exposed, got: {expose}"
        );
    }
}

/// LIF-465: the anonymous project view through the router `lific start`
/// builds. `api::public`'s own tests prove the handlers and the SQL; only the
/// assembled app can prove that mounting it did not put it behind the auth
/// middleware, or the authenticated API in front of it.
#[cfg(test)]
mod public_surface_tests {
    use super::*;
    use crate::db::models::*;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    struct Deployed {
        app: Router,
        _store_guard: tempfile::TempDir,
        attachment_id: i64,
        private_issue_id: i64,
    }

    /// Auth required, one published project with an issue, a comment and an
    /// attachment, and one private project holding a secret.
    fn deploy() -> Deployed {
        let pool = db::open_memory().expect("test db");
        let tmp = tempfile::tempdir().expect("attachment tempdir");
        let store = storage::AttachmentStore::new(tmp.path().to_path_buf());

        let (attachment_id, private_issue_id) = {
            let conn = pool.write().unwrap();
            conn.execute(
                "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot)
                 VALUES ('owner', 'owner@test.local', 'x', 'Owner', 1, 0)",
                [],
            )
            .unwrap();

            let published = db::queries::create_project(
                &conn,
                &CreateProject {
                    name: "Published".into(),
                    identifier: "PUB".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            conn.execute(
                "UPDATE projects SET is_public = 1 WHERE id = ?1",
                [published.id],
            )
            .unwrap();
            let private = db::queries::create_project(
                &conn,
                &CreateProject {
                    name: "Private".into(),
                    identifier: "PRIV".into(),
                    ..Default::default()
                },
            )
            .unwrap();

            let issue = db::queries::create_issue(
                &conn,
                &CreateIssue {
                    project_id: published.id,
                    title: "Public issue".into(),
                    description: "A body anyone may read".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            let secret = db::queries::create_issue(
                &conn,
                &CreateIssue {
                    project_id: private.id,
                    title: "Private issue".into(),
                    description: "classified".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            db::queries::comments::create_comment_with_mentions(
                &conn,
                db::queries::comments::CommentParent::Issue(issue.id),
                None,
                CommentActor {
                    user_id: 1,
                    is_admin: true,
                },
                AttachmentActor::TrustedLocal,
                "A public comment",
                false,
            )
            .unwrap();

            let bytes = b"public attachment bytes".to_vec();
            let sha = store.write(&bytes).unwrap();
            let attachment = db::queries::attachments::create_attachment(
                &conn,
                &sha,
                "spec.pdf",
                "application/pdf",
                bytes.len() as i64,
                None,
            )
            .unwrap();
            db::queries::attachments::link_attachment(
                &conn,
                attachment.id,
                AttachmentEntity::Issue,
                issue.id,
            )
            .unwrap();

            (attachment.id, secret.id)
        };

        let mut cfg = Config::default();
        cfg.auth.required = true;
        cfg.server.host = "127.0.0.1".into();
        let trusted_proxies = Arc::<[ratelimit::IpNetwork]>::from(
            cfg.server.trusted_proxy_ranges().expect("proxy ranges"),
        );
        let app = build_app_with_store(
            &cfg,
            pool,
            auth::create_key_manager().expect("key manager"),
            realtime::RealtimeHub::new(),
            trusted_proxies,
            store,
        );

        Deployed {
            app,
            _store_guard: tmp,
            attachment_id,
            private_issue_id,
        }
    }

    /// Deliberately sends no `Authorization` header and no cookie.
    async fn anonymous(app: &Router, method: &str, uri: &str) -> axum::response::Response {
        app.clone()
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

    async fn body_string(response: axum::response::Response) -> String {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8_lossy(&bytes).to_string()
    }

    /// A visitor with no account reads the list, the detail, the comments
    /// and the file, over the production middleware stack.
    #[tokio::test]
    async fn a_visitor_with_no_credentials_reads_a_published_project() {
        let d = deploy();
        for uri in [
            "/public/api/projects/PUB".to_string(),
            "/public/api/projects/PUB/index".to_string(),
            "/public/api/projects/PUB/issues/resolve/PUB-1".to_string(),
            format!("/public/api/projects/PUB/attachments/{}", d.attachment_id),
        ] {
            let response = anonymous(&d.app, "GET", &uri).await;
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "{uri} must be readable without any credential"
            );
            assert_eq!(
                response
                    .headers()
                    .get(header::CACHE_CONTROL)
                    .and_then(|v| v.to_str().ok()),
                Some("no-store, no-cache, must-revalidate"),
                "{uri} lost its no-store header somewhere in the stack"
            );
        }
    }

    /// Adding the public router loosened nothing: every ordinary API path
    /// still answers 401, including ones serving the same content.
    #[tokio::test]
    async fn the_ordinary_api_is_still_closed_to_anonymous_callers() {
        let d = deploy();
        for uri in [
            "/api/projects".to_string(),
            "/api/issues".to_string(),
            format!("/api/issues/{}", d.private_issue_id),
            "/api/issues/resolve/PUB-1".to_string(),
            "/api/pages".to_string(),
            "/api/search?q=classified".to_string(),
            "/api/auth/me".to_string(),
            "/api/users".to_string(),
            format!("/api/attachments/{}", d.attachment_id),
        ] {
            let response = anonymous(&d.app, "GET", &uri).await;
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{uri} must still require authentication"
            );
        }
    }

    /// A private project is not reachable through the public prefix, and says
    /// exactly what a project that was never created says.
    #[tokio::test]
    async fn a_private_project_is_not_reachable_through_the_public_prefix() {
        let d = deploy();
        let hidden = anonymous(&d.app, "GET", "/public/api/projects/PRIV/index").await;
        let missing = anonymous(&d.app, "GET", "/public/api/projects/NOPE/index").await;
        assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        let (hidden, missing) = (body_string(hidden).await, body_string(missing).await);
        assert_eq!(hidden, missing);
        assert!(!hidden.contains("classified"));
    }

    /// Nothing on the public prefix writes, in the assembled app as much as
    /// in the isolated router.
    #[tokio::test]
    async fn the_public_prefix_refuses_every_write() {
        let d = deploy();
        for method in ["POST", "PUT", "PATCH", "DELETE"] {
            let response = anonymous(&d.app, method, "/public/api/projects/PUB/index").await;
            assert_eq!(
                response.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} on the public prefix"
            );
        }
    }

    /// The load bounds are the only thing between an anonymous stranger and
    /// the read pool, so "wired in" is asserted where the wiring happens.
    #[tokio::test]
    async fn public_reads_are_rate_limited_in_the_assembled_app() {
        let d = deploy();
        let mut refused = 0;
        for _ in 0..300 {
            if anonymous(&d.app, "GET", "/public/api/projects/PUB")
                .await
                .status()
                == StatusCode::TOO_MANY_REQUESTS
            {
                refused += 1;
            }
        }
        assert!(
            refused > 0,
            "300 anonymous reads in a row were all served; no rate limit is wired in"
        );

        // The authenticated API is not collateral damage: it has its own
        // limits and must not share the public bucket.
        assert_eq!(
            anonymous(&d.app, "GET", "/api/projects").await.status(),
            StatusCode::UNAUTHORIZED,
            "the ordinary API must still answer 401, not the public 429"
        );
    }

    /// `/public/PUB` is the address a person shares. It is not an API route;
    /// it falls through to the SPA, which then renders the public view from
    /// the JSON endpoints above. The assertion is only that it is not a 401
    /// or a 404, the same treatment `/DEMO/issues` gets.
    #[tokio::test]
    async fn the_shareable_address_serves_the_app_without_a_login() {
        let d = deploy();
        let response = anonymous(&d.app, "GET", "/public/PUB").await;
        assert_ne!(response.status(), StatusCode::UNAUTHORIZED);
    }
}

#[cfg(test)]
mod authless_mcp_tests {
    use super::*;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn initialize_body() -> Body {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "1"}
            }
        });
        Body::from(serde_json::to_vec(&body).unwrap())
    }

    /// The whole point: a request to /mcp/<token> with NO Authorization header
    /// drives a full MCP `initialize` and returns 200. This is the path that
    /// works around claude.ai web's broken OAuth connector flow.
    #[tokio::test]
    async fn authless_path_serves_mcp_without_auth() {
        let pool = db::open_memory().unwrap();
        let token = "s3cret-authless-token-abcdef";
        let router = build_authless_mcp_router(
            pool,
            token,
            None,
            vec!["localhost".into()],
            None,
            realtime::RealtimeHub::new(),
        );

        let req = Request::builder()
            .method(Method::POST)
            .uri(format!("/mcp/{token}"))
            .header("host", "localhost")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(initialize_body())
            .unwrap();

        let res = router.oneshot(req).await.unwrap();
        assert_eq!(
            res.status(),
            StatusCode::OK,
            "authless MCP initialize must succeed without any auth header"
        );
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            val["result"]["serverInfo"].is_object(),
            "expected an initialize result, got: {val}"
        );
    }

    /// A wrong path token does not match the route at all (no secret leak,
    /// no MCP access) — it falls through to 404 in this isolated router.
    #[tokio::test]
    async fn wrong_path_token_does_not_match() {
        let pool = db::open_memory().unwrap();
        let router = build_authless_mcp_router(
            pool,
            "the-right-token",
            None,
            vec!["localhost".into()],
            None,
            realtime::RealtimeHub::new(),
        );

        let req = Request::builder()
            .method(Method::POST)
            .uri("/mcp/the-wrong-token")
            .header("host", "localhost")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(initialize_body())
            .unwrap();

        let res = router.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }
}
