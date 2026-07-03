mod actor;
mod api;
mod auth;
mod authz;
#[cfg(test)]
mod authz_coverage_tests;
mod backup;
mod cli;
mod config;
mod db;
mod error;
mod export;
mod mcp;
mod oauth;
mod ratelimit;
mod storage;

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{HeaderName, HeaderValue, Method, StatusCode, header},
    middleware,
    response::IntoResponse,
    routing::{any, get},
};
use tower_http::compression::CompressionLayer;
use tower_http::cors::{Any, CorsLayer};
use clap::{CommandFactory, Parser};
use cli::{Cli, Command, InstanceAction, KeyAction, UserAction};
use config::Config;

// Commands that operate directly on the database (no server required)
fn is_crud_command(cmd: &Command) -> bool {
    matches!(cmd,
        Command::Issue { .. } | Command::Project { .. } | Command::Page { .. } |
        Command::Export { .. } |
        Command::Search { .. } | Command::Comment { .. } | Command::Module { .. } |
        Command::Label { .. } | Command::Folder { .. }
    )
}
use rmcp::{
    ServiceExt,
    transport::streamable_http_server::{
        session::local::LocalSessionManager,
        tower::{StreamableHttpServerConfig, StreamableHttpService},
    },
};
use rust_embed::Embed;
use tracing::{info, warn};

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

    // SPA fallback: serve index.html for all unmatched routes
    match WebAssets::get("index.html") {
        Some(file) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html".to_string())],
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Rust ignores SIGPIPE process-wide, which makes println!/stdout writes
    // PANIC when piped into a closed reader (`lific completion fish | head`,
    // `lific issue list --json | head -1`). For data commands, restore the
    // default SIGPIPE disposition so the process exits quietly like every
    // other Unix CLI. The long-running servers (Start, Mcp) keep SIGPIPE
    // ignored — tokio socket writes rely on that to surface EPIPE as errors
    // instead of killing the process.
    #[cfg(unix)]
    if !matches!(cli.command, Command::Start { .. } | Command::Mcp) {
        // SAFETY: setting a signal disposition to SIG_DFL before any threads
        // depend on the ignored state; standard practice for CLI tools.
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        }
    }

    // Shell completions must work with no lific.toml present and touch no DB,
    // so handle them before loading config or opening the database.
    if let Command::Completion { shell } = cli.command {
        clap_complete::generate(shell, &mut Cli::command(), "lific", &mut std::io::stdout());
        return Ok(());
    }

    // Load config (CLI flags override config values)
    let mut cfg = Config::load(cli.config.as_deref());

    // CLI overrides
    if let Some(ref db) = cli.db {
        cfg.database.path = db.clone();
    }

    // Handle CRUD commands (direct database access, no server needed)
    if is_crud_command(&cli.command) {
        // LIF-155: CLI mutations run outside any request task — audit
        // them via the process-default transport.
        actor::set_default_transport(actor::Transport::Cli);
        let pool = db::open(&cfg.database.path)?;
        // clispec.dev: honor explicit --json, and auto-upgrade to JSON when
        // stdout is piped/redirected so scripts and agents get machine output.
        let json = cli::term::wants_json(cli.json);
        return cli::exec::run(&pool, &cli.command, json);
    }

    match cli.command {
        Command::Init => {
            let config_path = std::path::Path::new("lific.toml");
            if config_path.exists() {
                eprintln!("lific.toml already exists in current directory");
                std::process::exit(1);
            }
            std::fs::write(config_path, Config::default_toml())?;
            println!("Created lific.toml with default settings");
            return Ok(());
        }

        Command::Instance { action } => {
            let json = cli::term::wants_json(cli.json);
            let pool = db::open(&cfg.database.path)?;
            // Seed the settings row from TOML on first touch, then operate on
            // the DB store (authoritative).
            {
                let conn = pool.write()?;
                db::queries::settings::ensure(&conn, cfg.auth.allow_signup)?;
            }

            match action {
                InstanceAction::Set {
                    name,
                    signups,
                    signup_domains,
                    session_days,
                    login_message,
                    auto_login,
                    authz_enforced,
                } => {
                    let patch = db::queries::settings::InstanceSettingsPatch {
                        allow_signup: signups,
                        instance_name: name,
                        signup_email_domains: signup_domains.map(|csv| {
                            csv.split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect()
                        }),
                        session_lifetime_days: session_days,
                        login_message,
                        web_auto_login: auto_login,
                        authz_enforced,
                    };
                    let conn = pool.write()?;
                    db::queries::settings::update(&conn, patch)?;
                    drop(conn);
                    if !json {
                        println!("Updated instance settings.");
                    }
                    // Fall through to print current state below.
                }
                InstanceAction::Info => {}
            }

            let settings = {
                let conn = pool.read()?;
                db::queries::settings::get(&conn)?
            };
            let (total, admins) = {
                let conn = pool.read()?;
                let users = db::queries::users::list_users(&conn)?;
                let humans: Vec<_> = users.iter().filter(|u| !u.is_bot).collect();
                let admins = humans.iter().filter(|u| u.is_admin).count();
                (humans.len(), admins)
            };
            let version = env!("CARGO_PKG_VERSION");
            let domains = if settings.signup_email_domains.is_empty() {
                "(any)".to_string()
            } else {
                settings.signup_email_domains.join(", ")
            };

            if json {
                let out = serde_json::json!({
                    "version": version,
                    "database": cfg.database.path.display().to_string(),
                    "host": cfg.server.host,
                    "port": cfg.server.port,
                    "public_url": cfg.server.public_url,
                    "name": settings.instance_name,
                    "allow_signup": settings.allow_signup,
                    "signup_email_domains": settings.signup_email_domains,
                    "session_lifetime_days": settings.session_lifetime_days,
                    "login_message": settings.login_message,
                    "users": { "total": total, "admins": admins },
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                println!("Instance");
                println!("  name:          {}", settings.instance_name.as_deref().unwrap_or("(unnamed)"));
                println!("  version:       {version}");
                println!("  database:      {}", cfg.database.path.display());
                println!("  bind:          {}:{}", cfg.server.host, cfg.server.port);
                println!("  public url:    {}", cfg.server.public_url.as_deref().unwrap_or("(not set)"));
                println!("  signups:       {}", if settings.allow_signup { "open" } else { "closed" });
                println!("  signup domains:{domains}");
                println!("  session days:  {}", settings.session_lifetime_days);
                println!("  login message: {}", settings.login_message.as_deref().unwrap_or("(none)"));
                println!("  users:         {total} ({admins} admin)");
            }
            return Ok(());
        }

        Command::Key { action } => {
            let json = cli::term::wants_json(cli.json);
            let pool = db::open(&cfg.database.path)?;
            let manager =
                auth::create_key_manager().map_err(|e| format!("key manager init failed: {e}"))?;

            match action {
                KeyAction::Create { name, user } => {
                    let key = auth::create_api_key(&pool, &manager, &name)?;

                    // If --user was provided, assign the key to that user
                    let assigned = if let Some(ref username) = user {
                        let conn = pool.read()?;
                        let u = db::queries::users::get_user_by_username(&conn, username)?;
                        drop(conn);
                        let conn = pool.write()?;
                        db::queries::users::assign_key_to_user(&conn, &name, u.id)?;
                        Some(username.clone())
                    } else {
                        None
                    };

                    if json {
                        let out = serde_json::json!({
                            "name": name,
                            "key": key,
                            "user": assigned,
                        });
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    } else {
                        if let Some(ref username) = assigned {
                            println!();
                            println!("  API Key created: {name} (assigned to {username})");
                        } else {
                            println!();
                            println!("  API Key created: {name}");
                        }
                        println!();
                        println!("  {key}");
                        println!();
                        println!("  Save this key now. It will never be shown again.");
                        println!("  Use as: Authorization: Bearer {key}");
                        println!();
                    }
                }
                KeyAction::List => {
                    let keys = auth::list_api_keys(&pool)?;
                    if json {
                        let out: Vec<_> = keys
                            .iter()
                            .map(|k| {
                                serde_json::json!({
                                    "name": k.name,
                                    "revoked": k.revoked,
                                    "created_at": k.created_at,
                                    "expires_at": k.expires_at,
                                })
                            })
                            .collect();
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    } else if keys.is_empty() {
                        println!("No API keys configured.");
                    } else {
                        println!("{} API key(s):", keys.len());
                        for k in &keys {
                            let status = if k.revoked { "REVOKED" } else { "active" };
                            let expiry = k.expires_at.as_deref().unwrap_or("never");
                            println!(
                                "  {} | {} | created {} | expires {}",
                                k.name, status, k.created_at, expiry
                            );
                        }
                    }
                }
                KeyAction::Revoke { name } => {
                    auth::revoke_api_key(&pool, &name)?;
                    if json {
                        let out = serde_json::json!({ "revoked": name });
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    } else {
                        println!("Revoked key: {name}");
                    }
                }
                KeyAction::Rotate { name } => {
                    let key = auth::rotate_api_key(&pool, &manager, &name)?;
                    if json {
                        let out = serde_json::json!({ "name": name, "key": key });
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    } else {
                        println!();
                        println!("  Key rotated: {name}");
                        println!();
                        println!("  {key}");
                        println!();
                        println!("  Save this key now. It will never be shown again.");
                        println!();
                    }
                }
                KeyAction::Assign { name, user } => {
                    let conn = pool.read()?;
                    let u = db::queries::users::get_user_by_username(&conn, &user)?;
                    drop(conn);
                    let conn = pool.write()?;
                    db::queries::users::assign_key_to_user(&conn, &name, u.id)?;
                    if json {
                        let out = serde_json::json!({ "name": name, "user": user });
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    } else {
                        println!("Assigned key '{name}' to user '{user}'");
                    }
                }
            }
            return Ok(());
        }

        Command::User { action } => {
            let json = cli::term::wants_json(cli.json);
            let pool = db::open(&cfg.database.path)?;

            match action {
                UserAction::Create {
                    username,
                    email,
                    password,
                    admin,
                    bot,
                } => {
                    // Prompt for password if not provided
                    let pw = match password {
                        Some(p) => p,
                        None => {
                            eprint!("Password: ");
                            let mut buf = String::new();
                            std::io::stdin().read_line(&mut buf)?;
                            buf.trim().to_string()
                        }
                    };

                    let conn = pool.write()?;
                    let user = db::queries::users::create_user(
                        &conn,
                        &db::models::CreateUser {
                            username: username.clone(),
                            email: email.clone(),
                            password: pw,
                            display_name: None,
                            is_admin: admin,
                            is_bot: bot,
                        },
                    )?;

                    if json {
                        let out = serde_json::json!({
                            "username": user.username,
                            "email": user.email,
                            "display_name": user.display_name,
                            "is_admin": user.is_admin,
                            "is_bot": user.is_bot,
                        });
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    } else {
                        let role = if user.is_admin { " (admin)" } else { "" };
                        println!("User created: {}{role}", user.username);
                        println!("  email: {}", user.email);
                        println!("  display_name: {}", user.display_name);
                    }
                }
                UserAction::List => {
                    let conn = pool.read()?;
                    let users = db::queries::users::list_users(&conn)?;

                    if json {
                        let out: Vec<_> = users
                            .iter()
                            .map(|u| {
                                serde_json::json!({
                                    "id": u.id,
                                    "username": u.username,
                                    "email": u.email,
                                    "is_admin": u.is_admin,
                                    "is_bot": u.is_bot,
                                    "created_at": u.created_at,
                                })
                            })
                            .collect();
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    } else if users.is_empty() {
                        println!("No users.");
                    } else {
                        println!("{} user(s):", users.len());
                        for u in &users {
                            let flags = match (u.is_admin, u.is_bot) {
                                (true, true) => " [admin, bot]",
                                (true, false) => " [admin]",
                                (false, true) => " [bot]",
                                (false, false) => "",
                            };
                            println!(
                                "  {} | {} | {}{} | created {}",
                                u.id, u.username, u.email, flags, u.created_at
                            );
                        }
                    }
                }
                UserAction::Promote { username } => {
                    let conn = pool.write()?;
                    db::queries::users::set_admin(&conn, &username, true)?;
                    if json {
                        let out = serde_json::json!({ "promoted": username });
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    } else {
                        println!("Promoted '{username}' to admin.");
                    }
                }
                UserAction::Demote { username } => {
                    let conn = pool.write()?;
                    db::queries::users::set_admin(&conn, &username, false)?;
                    if json {
                        let out = serde_json::json!({ "demoted": username });
                        println!("{}", serde_json::to_string_pretty(&out)?);
                    } else {
                        println!("Demoted '{username}' from admin.");
                    }
                }
            }
            return Ok(());
        }

        Command::Start { port, host } => {
            if let Some(p) = port {
                cfg.server.port = p;
            }
            if let Some(h) = host {
                cfg.server.host = h;
            }

            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| format!("lific={}", cfg.log.level).into()),
                )
                .init();

            let pool = db::open(&cfg.database.path)?;
            info!(path = %cfg.database.path.display(), "database ready");

            // Seed the instance-settings row on first run, taking the initial
            // signup policy from TOML. Once seeded, the DB row is authoritative
            // and admins edit it live via the UI/CLI (LIF-210).
            {
                let conn = pool.write()?;
                db::queries::settings::ensure(&conn, cfg.auth.allow_signup)?;

                // LIF-215: single-user web auto-login hands an admin session to
                // anyone who can load the page. That's fine on a private/local
                // instance but dangerous when the server is publicly reachable.
                // Use an https public_url as the "exposed" heuristic and shout.
                if db::queries::settings::get(&conn)
                    .map(|s| s.web_auto_login)
                    .unwrap_or(false)
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

            // Auto-generate a key if none exist
            if !auth::has_any_keys(&pool) {
                let key = auth::create_api_key(&pool, &manager, "default")?;
                info!("no API keys found, auto-generated initial key");
                println!();
                println!("  ┌─────────────────────────────────────────────────────┐");
                println!("  │  No API keys found. Generated initial key:          │");
                println!("  │                                                     │");
                println!("  │  {key}");
                println!("  │                                                     │");
                println!("  │  Save this key now. It will never be shown again.   │");
                println!("  │  Use as: Authorization: Bearer <key>                │");
                println!("  └─────────────────────────────────────────────────────┘");
                println!();
            } else {
                let count = auth::list_api_keys(&pool)?
                    .iter()
                    .filter(|k| !k.revoked)
                    .count();
                info!(active_keys = count, "API key auth enabled");
            }

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
                format!("http://{}:{}", host, cfg.server.port)
            });

            let manager_ext = Arc::new(manager.clone());

            let auth_state = auth::AuthState {
                db: pool.clone(),
                manager,
                public_url: issuer.clone(),
            };

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

            // MCP StreamableHTTP service
            let db_for_mcp = pool.clone();
            let mut mcp_allowed_hosts: Vec<String> =
                vec!["localhost".into(), "127.0.0.1".into(), "::1".into()];

            // If public_url is set, allow its hostname through the DNS rebinding check
            // so reverse proxies (Tailscale funnel, nginx, etc.) can forward requests.
            if let Some(ref url) = cfg.server.public_url
                && let Ok(parsed) = url.parse::<axum::http::Uri>()
                    && let Some(authority) = parsed.authority() {
                        let host: String = authority.host().to_string();
                        mcp_allowed_hosts.push(host);
                    }

            let mcp_config = StreamableHttpServerConfig::default()
                .with_stateful_mode(false)
                .with_json_response(true)
                .with_allowed_hosts(mcp_allowed_hosts.clone());

            let mcp_service = StreamableHttpService::new(
                move || Ok(mcp::LificMcp::new(db_for_mcp.clone())),
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
            let attachment_store =
                storage::AttachmentStore::from_db_path(&cfg.database.path);
            let attachment_config = api::AttachmentConfig::default();
            let attachment_upload_limiter = Arc::new(api::AttachmentUploadLimiter(
                ratelimit::RateLimiter::new(30, std::time::Duration::from_secs(10 * 60)),
            ));
            // Sweep abandoned (unlinked) attachments hourly.
            storage::start_gc_task(pool.clone(), attachment_store.clone());

            // Routes behind auth: REST API + MCP
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

                        mcp::with_request_user(auth_user, || async {
                            mcp_service.handle(request).await.into_response()
                        })
                        .await
                    }),
                )
                .layer(axum::Extension(login_limiter))
                .layer(axum::Extension(attachment_store))
                .layer(axum::Extension(attachment_config))
                .layer(axum::Extension(attachment_upload_limiter))
                .layer(axum::Extension(crate::config::AuthConfig::from_server(
                    cfg.auth.allow_signup,
                    cfg.server.public_url.as_deref(),
                )))
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
                register_limiter: oauth_register_limiter,
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
                                None => db::queries::users::first_admin(&conn)
                                    .ok()
                                    .flatten(),
                            },
                            Err(_) => None,
                        }
                    };
                    info!(
                        acting_as = authless_user
                            .as_ref()
                            .map(|u| u.username.as_str())
                            .unwrap_or("<anonymous>"),
                        "authless MCP endpoint enabled at /mcp/<token>"
                    );
                    build_authless_mcp_router(
                        pool.clone(),
                        &token,
                        authless_user,
                        mcp_allowed_hosts.clone(),
                    )
                });

            let app = authed_routes.merge(oauth::router(oauth_state));
            let app = match authless_mcp_router {
                Some(r) => app.merge(r),
                None => app,
            };
            let app = app
                .fallback(get(serve_frontend))
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
                .layer(CompressionLayer::new());

            let addr = format!("{}:{}", cfg.server.host, cfg.server.port);
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            info!(addr = %addr, "lific server started (REST + MCP + OAuth at /mcp)");

            let shutdown_pool = pool.clone();
            let server =
                axum::serve(listener, app).with_graceful_shutdown(shutdown_signal(shutdown_pool));
            server.await?;
        }

        Command::Login {
            url,
            non_interactive,
            complete,
            label,
            no_store,
        } => {
            let json = cli::term::wants_json(cli.json);
            let args = cli::login::LoginArgs {
                url,
                non_interactive,
                complete,
                label,
                no_store,
            };
            // The login flow uses a blocking reqwest client and a polling loop
            // with sleeps; run it off the async runtime so `reqwest::blocking`
            // doesn't panic (dropping its runtime inside an async context) and
            // the sleeps don't stall the reactor.
            let cfg_clone = cfg.clone();
            tokio::task::spawn_blocking(move || cli::login::run_login(&args, &cfg_clone, json))
                .await
                .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            return Ok(());
        }

        Command::Logout { url } => {
            let json = cli::term::wants_json(cli.json);
            let cfg_clone = cfg.clone();
            tokio::task::spawn_blocking(move || {
                cli::login::run_logout(url.as_deref(), &cfg_clone, json)
            })
            .await
            .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?
            .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            return Ok(());
        }

        Command::Doctor { key } => {
            // Diagnostics only: no tracing subscriber (keep stdout clean for the
            // human table / JSON), and no DB open up front — the database check
            // opens it itself and reports failure as a check, rather than
            // aborting `doctor` before it can tell you why.
            let json = cli::term::wants_json(cli.json);
            cli::doctor::run(&cfg, cli.config.as_deref(), key, json)
                .await
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            return Ok(());
        }

        Command::Connect {
            clients,
            scope,
            stdio,
            oauth,
            url,
            key,
            user,
            yes,
            dry_run,
            skip_agents,
        } => {
            let json = cli::term::wants_json(cli.json);
            let scope = match scope.as_str() {
                "global" => cli::connect::clients::Scope::Global,
                "project" => cli::connect::clients::Scope::Project,
                other => {
                    return Err(format!(
                        "invalid --scope '{other}' (expected 'global' or 'project')"
                    )
                    .into());
                }
            };

            let base = cli::connect::production_base()?;
            let pool = db::open(&cfg.database.path)?;
            actor::set_default_transport(actor::Transport::Cli);

            let args = cli::connect::ConnectArgs {
                clients,
                scope,
                stdio,
                oauth,
                url,
                key,
                user,
                yes,
                dry_run,
                skip_agents,
            };
            let result = cli::connect::run(&args, &cfg, &pool, &base)?;
            cli::connect::print_result(&result, json);
            return Ok(());
        }

        Command::AgentsMd { path, project } => {
            let json = cli::term::wants_json(cli.json);
            let target = path.unwrap_or_else(|| std::path::PathBuf::from("AGENTS.md"));
            let action = cli::agents_md::write(&target, project.as_deref())?;
            if json {
                let out = serde_json::json!({
                    "path": target.display().to_string(),
                    "action": action.as_str(),
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                println!("AGENTS.md {}: {}", action.as_str(), target.display());
            }
            return Ok(());
        }

        Command::Mcp => {
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| format!("lific={}", cfg.log.level).into()),
                )
                .with_writer(std::io::stderr)
                .init();

            let pool = db::open(&cfg.database.path)?;
            info!(path = %cfg.database.path.display(), "database ready");

            let server = mcp::LificMcp::new(pool);
            let transport = rmcp::transport::io::stdio();

            info!("lific MCP server started (stdio)");
            let handle = server.serve(transport).await?;
            handle.waiting().await?;
        }

        // CRUD commands and Completion are handled before this match
        Command::Completion { .. } |
        Command::Issue { .. } | Command::Project { .. } | Command::Page { .. } |
        Command::Export { .. } |
        Command::Search { .. } | Command::Comment { .. } | Command::Module { .. } |
        Command::Label { .. } | Command::Folder { .. } => unreachable!(),
    }

    Ok(())
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
        ])
        .max_age(std::time::Duration::from_secs(86400));

    if cors_origins.is_empty() {
        layer.allow_origin(Any)
    } else {
        let origins: Vec<HeaderValue> = cors_origins
            .iter()
            .filter_map(|o| o.parse().ok())
            .collect();
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
) -> Router {
    let config = StreamableHttpServerConfig::default()
        .with_stateful_mode(false)
        .with_json_response(true)
        .with_allowed_hosts(allowed_hosts);
    let service = StreamableHttpService::new(
        move || Ok(mcp::LificMcp::new(pool.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    Router::new().route(
        &format!("/mcp/{token}"),
        any(move |request: Request<Body>| async move {
            mcp::with_request_user(user, || async {
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
    let path = request.uri().path();
    if path == "/api/health"
        || path == "/api/instance"
        || path == "/api/auth/signup"
        || path == "/api/auth/login"
        || path == "/api/auth/auto-login"
        || path.starts_with("/.well-known/")
        || path.starts_with("/oauth/")
        || path == "/register"
        || path == "/authorize"
        || path == "/token"
        || path == "/revoke"
    {
        return next.run(request).await;
    }
    auth::require_api_key(state, request, next).await
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
mod cors_tests {
    use super::*;
    use axum::routing::post;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

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
            .header("access-control-request-headers", "authorization,content-type")
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
        let router =
            build_authless_mcp_router(pool, token, None, vec!["localhost".into()]);

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
        let router =
            build_authless_mcp_router(pool, "the-right-token", None, vec!["localhost".into()]);

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
