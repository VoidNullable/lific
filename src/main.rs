mod api;
mod auth;
mod backup;
mod cli;
mod config;
mod db;
mod error;
mod mcp;
mod oauth;
mod ratelimit;

use std::sync::Arc;

use axum::{
    body::Body,
    extract::Request,
    http::{StatusCode, header},
    middleware,
    response::IntoResponse,
    routing::{any, get},
};
use clap::Parser;
use cli::{Cli, Command, KeyAction, UserAction};
use config::Config;
use rmcp::{
    ServiceExt,
    transport::streamable_http_server::{
        session::local::LocalSessionManager,
        tower::{StreamableHttpServerConfig, StreamableHttpService},
    },
};
use rust_embed::Embed;
use tracing::info;

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
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, mime)],
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

    // Load config (CLI flags override config values)
    let mut cfg = Config::load(cli.config.as_deref());

    // CLI overrides
    if let Some(ref db) = cli.db {
        cfg.database.path = db.clone();
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

        Command::Key { action } => {
            let pool = db::open(&cfg.database.path)?;
            let manager =
                auth::create_key_manager().map_err(|e| format!("key manager init failed: {e}"))?;

            match action {
                KeyAction::Create { name, user } => {
                    let key = auth::create_api_key(&pool, &manager, &name)?;

                    // If --user was provided, assign the key to that user
                    if let Some(ref username) = user {
                        let conn = pool.read()?;
                        let u = db::queries::users::get_user_by_username(&conn, username)?;
                        drop(conn);
                        let conn = pool.write()?;
                        db::queries::users::assign_key_to_user(&conn, &name, u.id)?;
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
                KeyAction::List => {
                    let keys = auth::list_api_keys(&pool)?;
                    if keys.is_empty() {
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
                    println!("Revoked key: {name}");
                }
                KeyAction::Rotate { name } => {
                    let key = auth::rotate_api_key(&pool, &manager, &name)?;
                    println!();
                    println!("  Key rotated: {name}");
                    println!();
                    println!("  {key}");
                    println!();
                    println!("  Save this key now. It will never be shown again.");
                    println!();
                }
                KeyAction::Assign { name, user } => {
                    let conn = pool.read()?;
                    let u = db::queries::users::get_user_by_username(&conn, &user)?;
                    drop(conn);
                    let conn = pool.write()?;
                    db::queries::users::assign_key_to_user(&conn, &name, u.id)?;
                    println!("Assigned key '{name}' to user '{user}'");
                }
            }
            return Ok(());
        }

        Command::User { action } => {
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

                    let role = if user.is_admin { " (admin)" } else { "" };
                    println!("User created: {}{role}", user.username);
                    println!("  email: {}", user.email);
                    println!("  display_name: {}", user.display_name);
                }
                UserAction::List => {
                    let conn = pool.read()?;
                    let users = db::queries::users::list_users(&conn)?;

                    if users.is_empty() {
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
                    println!("Promoted '{username}' to admin.");
                }
                UserAction::Demote { username } => {
                    let conn = pool.write()?;
                    db::queries::users::set_admin(&conn, &username, false)?;
                    println!("Demoted '{username}' from admin.");
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

            // Auth state for middleware
            let issuer = cfg
                .server
                .public_url
                .clone()
                .unwrap_or_else(|| format!("http://{}:{}", cfg.server.host, cfg.server.port));

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
            let mcp_config = StreamableHttpServerConfig::default()
                .with_stateful_mode(false)
                .with_json_response(true);

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
                .layer(axum::Extension(cfg.auth.clone()))
                .layer(axum::Extension(manager_ext))
                .layer(middleware::from_fn_with_state(
                    auth_state,
                    auth_middleware_wrapper,
                ));

            let oauth_state = oauth::OAuthState {
                db: pool.clone(),
                issuer,
            };

            let app = authed_routes
                .merge(oauth::router(oauth_state))
                .fallback(get(serve_frontend))
                .layer(axum::extract::DefaultBodyLimit::max(2 * 1024 * 1024)); // 2 MB

            let addr = format!("{}:{}", cfg.server.host, cfg.server.port);
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            info!(addr = %addr, "lific server started (REST + MCP + OAuth at /mcp)");

            let shutdown_pool = pool.clone();
            let server =
                axum::serve(listener, app).with_graceful_shutdown(shutdown_signal(shutdown_pool));
            server.await?;
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
    }

    Ok(())
}

/// Wrapper that skips auth for /api/health
async fn auth_middleware_wrapper(
    state: axum::extract::State<auth::AuthState>,
    request: Request<Body>,
    next: middleware::Next,
) -> axum::response::Response {
    let path = request.uri().path();
    if path == "/api/health"
        || path == "/api/auth/signup"
        || path == "/api/auth/login"
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
