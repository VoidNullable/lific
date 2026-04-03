mod api;
mod auth;
mod backup;
mod cli;
mod config;
mod db;
mod error;
mod import;
mod mcp;

use std::sync::Arc;

use axum::{body::Body, extract::Request, middleware, response::IntoResponse, routing::any};
use clap::Parser;
use cli::{Cli, Command, KeyAction};
use config::Config;
use rmcp::{
    ServiceExt,
    transport::streamable_http_server::{
        session::local::LocalSessionManager,
        tower::{StreamableHttpServerConfig, StreamableHttpService},
    },
};
use tracing::info;

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
                KeyAction::Create { name } => {
                    let key = auth::create_api_key(&pool, &manager, &name)?;
                    println!();
                    println!("  API Key created: {name}");
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
            let auth_state = auth::AuthState {
                db: pool.clone(),
                manager,
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

            // Build router with auth middleware on all routes except /api/health
            let app = api::router(pool.clone())
                .route(
                    "/mcp",
                    any(move |request: Request<Body>| async move {
                        mcp_service.handle(request).await.into_response()
                    }),
                )
                .layer(middleware::from_fn_with_state(
                    auth_state,
                    auth_middleware_wrapper,
                ));

            let addr = format!("{}:{}", cfg.server.host, cfg.server.port);
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            info!(addr = %addr, "lific server started (REST + MCP at /mcp)");

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

        Command::ImportPlane {
            url,
            api_key,
            workspace,
            skip,
        } => {
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| format!("lific={}", cfg.log.level).into()),
                )
                .init();

            let pool = db::open(&cfg.database.path)?;
            info!(path = %cfg.database.path.display(), "database ready");

            import::run_api_import(&pool, &url, &api_key, &workspace, &skip).await?;
        }

        Command::ImportFile { file, skip } => {
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| format!("lific={}", cfg.log.level).into()),
                )
                .init();

            let pool = db::open(&cfg.database.path)?;
            info!(path = %cfg.database.path.display(), "database ready");

            import::run_import(&pool, &file, &skip)?;
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
    if request.uri().path() == "/api/health" {
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
