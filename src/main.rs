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
mod dump;
mod error;
mod export;
mod import;
mod links;
mod mcp;
mod oauth;
mod ratelimit;
mod resolve_caller;
mod realtime;
mod server;
mod storage;

use clap::{CommandFactory, Parser};
use cli::{
    BackendKind, Cli, Command, ServiceAction,
};
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
use rmcp::ServiceExt;
use tracing::info;

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

    // Load config (CLI flags override config values). A malformed config is
    // fatal: booting on defaults would silently widen the instance.
    let mut cfg = Config::load(cli.config.as_deref())?;

    // CLI overrides
    if let Some(ref db) = cli.db {
        cfg.database.path = db.clone();
    }

    if cli.backend == BackendKind::Http {
        if !is_crud_command(&cli.command) {
            return Err(
                "the HTTP backend currently supports data commands: issue, project, page, export, search, comment, module, label, and folder"
                    .into(),
            );
        }
        let url = http_backend_url(
            cli.url.as_deref(),
            cfg.server.public_url.as_deref(),
            &cfg,
        );
        let api_key = cli::resolve_http_credential(
            cli.api_key.as_deref(),
            || cli::credentials::load(&url),
        )?;
        let json = cli::term::wants_json(cli.json);
        return cli::http::run(&cli.command, &url, api_key.as_deref(), json)
            .await
            .map_err(Into::into);
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
        Command::Init {
            no_service,
            here,
            name,
            auth_mode,
            password,
        } => {
            // LIF-292: init/service must honor --config; they take the raw
            // flag (not the pre-loaded cfg) because init may need to CREATE
            // the file at that path and then reload anchored to it.
            return cmd_init(
                cli.config.as_deref(),
                cli.db.as_deref(),
                cli.json,
                no_service,
                here,
                name,
                auth_mode,
                password,
            )
            .await;
        }

        Command::Service { action } => {
            return cmd_service(&cfg, cli.config.as_deref(), cli.json, &action);
        }

        Command::Dump { out } => {
            let json = cli::term::wants_json(cli.json);
            let result = dump::run_dump(&cfg.database.path, out.as_deref())
                .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?;
            let m = &result.manifest;
            if json {
                let out_json = serde_json::json!({
                    "archive": result.archive_path.display().to_string(),
                    "lific_version": m.lific_version,
                    "schema_version": m.schema_version,
                    "created_at": m.created_at,
                    "db_size_bytes": m.db_size_bytes,
                    "attachment_count": m.attachment_count,
                    "attachment_bytes": m.attachment_bytes,
                });
                println!("{}", serde_json::to_string_pretty(&out_json)?);
            } else {
                use cli::ui;
                ui::step(format!(
                    "Wrote backup archive {}",
                    ui::command(result.archive_path.display())
                ));
                ui::info(ui::dim(format!(
                    "lific {} · schema v{} · db {} bytes · {} attachments ({} bytes)",
                    m.lific_version,
                    m.schema_version,
                    m.db_size_bytes,
                    m.attachment_count,
                    m.attachment_bytes
                )));
            }
            return Ok(());
        }

        Command::Restore { archive, force } => {
            let json = cli::term::wants_json(cli.json);
            // Best-effort warning: a hot WAL suggests the server is still up.
            if dump::server_maybe_running(&cfg.database.path) {
                eprintln!(
                    "warning: a hot -wal file is present next to {} — is the server still \
                     running? Stop it before restoring.",
                    cfg.database.path.display()
                );
            }
            let result = dump::run_restore(&archive, &cfg.database.path, force)
                .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?;
            let m = &result.manifest;
            if json {
                let out_json = serde_json::json!({
                    "restored_to": result.db_path.display().to_string(),
                    "lific_version": m.lific_version,
                    "schema_version": m.schema_version,
                    "created_at": m.created_at,
                    "attachment_count": result.attachment_count,
                    "moved_existing_to": result
                        .moved_existing_to
                        .as_ref()
                        .map(|p| p.display().to_string()),
                });
                println!("{}", serde_json::to_string_pretty(&out_json)?);
            } else {
                use cli::ui;
                ui::intro("lific restore");
                ui::step(format!("Restored from {}", ui::command(archive.display())));
                ui::info(ui::dim(format!(
                    "database {} · from lific {} · schema v{} · {} attachments",
                    result.db_path.display(),
                    m.lific_version,
                    m.schema_version,
                    result.attachment_count
                )));
                if let Some(moved) = &result.moved_existing_to {
                    ui::warn(format!(
                        "previous database moved aside to {}",
                        moved.display()
                    ));
                }
                ui::outro("Start the server; any pending migrations will apply on startup.");
            }
            return Ok(());
        }

        Command::Instance { action } => {
            return cli::instance::run(&cfg, action, cli.json);
        }

        Command::Key { action } => {
            return cli::key::run(&cfg, action, cli.json);
        }

        Command::User { action } => {
            return cli::user::run(&cfg, action, cli.json);
        }

        Command::Member { action } => {
            return cli::member::run(&cfg, action, cli.json);
        }

        Command::Start { port, host } => {
            if let Some(p) = port {
                cfg.server.port = p;
            }
            if let Some(h) = host {
                cfg.server.host = h;
            }

            server::run(&cfg).await?;
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
            // Refuse to conjure a fresh database in whatever directory this
            // happens to run from — connect targets an EXISTING instance.
            cli::connect::ensure_instance_exists(&cfg)?;
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
            if !json {
                cli::ui::intro("lific connect");
                // Say WHICH instance up front: the url clients will dial and
                // the database keys are minted in. Running from the wrong
                // directory must be obvious here, not after the writes.
                cli::ui::info(format!(
                    "Instance: {} {}",
                    cli::ui::command(cli::connect::target_url(&args, &cfg)),
                    cli::ui::dim(format!(
                        "(keys minted in {})",
                        cli::connect::absolute_db_path(&cfg)
                    ))
                ));
            }
            let result = match cli::connect::run(&args, &cfg, &pool, &base) {
                Ok(r) => r,
                Err(e) => {
                    // Close the clack session cleanly instead of leaving a
                    // dangling gutter, then surface the error normally.
                    if !json {
                        cli::ui::outro_cancel(&e);
                        std::process::exit(1);
                    }
                    return Err(e.into());
                }
            };
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

        Command::Import { action } => {
            let json = cli::term::wants_json(cli.json);
            // The importers use blocking reqwest + polling loops; run them off
            // the async runtime so `reqwest::blocking` doesn't panic (same
            // pattern as `login`).
            let cfg_clone = cfg.clone();
            tokio::task::spawn_blocking(move || {
                cli::import::run(&cfg_clone, &action, json).map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?
            .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
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

            // LIFIC-18: a stdio agent carries its identity in LIFIC_TOKEN. Read
            // it at startup, validate it, and resolve the caller as that agent
            // for the whole session. A missing/unbound token runs as the
            // operator with a stderr warning (MCP stdio has no transport auth;
            // the launch boundary is the trust). A PRESENT-but-invalid token is
            // a hard error: a revoked or mistyped agent credential must not
            // silently fall back to higher-privilege operator access (PR #23
            // review).
            let manager = auth::create_key_manager()?;
            let token_user = match auth::resolve_stdio_token(&pool, &manager) {
                Ok(Some(user)) => Some(user),
                Ok(None) => {
                    // Absent or valid-but-unbound (e.g. a fresh-install
                    // unassigned key): run as the operator, with a warning.
                    eprintln!(
                        "LIFIC_TOKEN not set or unbound — this session runs as the operator, \
                         not a connected agent.\n\
                         Run `lific connect` to bind this session to an agent identity."
                    );
                    None
                }
                Err(e) => {
                    return Err(format!(
                        "LIFIC_TOKEN is set but invalid ({e}); refusing to start. A revoked \
                         or mistyped agent credential must not fall back to operator access. \
                         Re-run `lific connect` to mint a fresh token, or unset LIFIC_TOKEN \
                         to run as the operator."
                    )
                    .into());
                }
            };

            let server = mcp::LificMcp::new(pool);
            // LIFIC-18: bind the resolved agent (or operator) as this stdio
            // session's identity for the whole process lifetime.
            mcp::set_stdio_user(token_user.clone());
            let transport = rmcp::transport::io::stdio();

            info!("lific MCP server started (stdio)");
            let handle = server.serve(transport).await?;
            if let Some(u) = &token_user {
                info!(user = %u.username, "stdio session bound to agent");
            }
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

/// The locally dialable base URL for this instance (bind-any hosts map to
/// loopback, same rule as the OAuth issuer derivation in `start`).
fn local_url(cfg: &Config) -> String {
    let host = match cfg.server.host.as_str() {
        "0.0.0.0" | "::" | "[::]" => "127.0.0.1",
        h => h,
    };
    format!("http://{}:{}", host, cfg.server.port)
}

fn http_backend_url(cli_url: Option<&str>, public_url: Option<&str>, cfg: &Config) -> String {
    cli_url
        .or(public_url)
        .map(str::to_owned)
        .unwrap_or_else(|| local_url(cfg))
}
/// Poll `<base>/api/health` until it answers 200 or the deadline passes.
async fn wait_healthy(base_url: &str, timeout: std::time::Duration) -> bool {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let url = format!("{base_url}/api/health");
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if let Ok(resp) = client.get(&url).send().await
            && resp.status().is_success()
        {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    false
}

/// `lific init`: everything needed to go from nothing to a running, reachable
/// instance in one command — config, database, initial API key, and a
/// background service that survives reboot. Idempotent: re-running repairs
/// whatever is missing and never overwrites existing config or keys.
/// LIF-295: where `lific init` roots the instance.
///
/// Returns `(config_path, default_db_path)`; `default_db_path` is `Some`
/// only for the OS-dirs layout, where the generated config must carry an
/// explicit absolute `database.path` (config dir and data dir differ).
///
/// - `--config <p>` → root at `p`, relative db beside it.
/// - `--here`, or a `lific.toml` already in the cwd (repairing an existing
///   directory-local instance must win over silently starting a second
///   instance in the OS dirs), or unresolvable platform dirs → cwd layout.
/// - otherwise → OS config dir + OS data dir (`Config::os_default_instance`).
fn resolve_init_target(
    config_flag: Option<&std::path::Path>,
    here: bool,
    cwd_config_exists: bool,
    os_default: Option<(std::path::PathBuf, std::path::PathBuf)>,
) -> (std::path::PathBuf, Option<std::path::PathBuf>) {
    if let Some(p) = config_flag {
        return (p.to_path_buf(), None);
    }
    if here || cwd_config_exists {
        return (std::path::PathBuf::from("lific.toml"), None);
    }
    match os_default {
        Some((config, db)) => (config, Some(db)),
        None => (std::path::PathBuf::from("lific.toml"), None),
    }
}

/// Load the config file `init` operates on, applying the optional `--db`
/// override on top. Shared by the initial load and the post-auth-mode reload
/// (LIFIC-25), so the override logic lives in exactly one place. A malformed
/// config file is fatal, matching `Config::load`'s contract everywhere else.
fn load_config_for_init(
    config_path: &std::path::Path,
    db_flag: Option<&std::path::Path>,
) -> Result<Config, config::ConfigError> {
    let mut cfg = Config::load(Some(config_path))?;
    if let Some(db) = db_flag {
        cfg.database.path = db.to_path_buf();
    }
    Ok(cfg)
}

/// Resolve the auth mode the operator chose at `init` (LIFIC-25). Honors an
/// explicit `--auth-mode` flag (non-interactive); otherwise, on a TTY, shows
/// the interactive menu. Refuses (rather than hangs) off a TTY, matching
/// `prompt_text`/`confirm`, and names the bypass flag.
fn resolve_auth_mode(flag: &Option<String>) -> Result<config::AuthMode, Box<dyn std::error::Error>> {
    if let Some(value) = flag {
        return config::AuthMode::parse(value).ok_or_else(|| {
            format!("invalid --auth-mode '{value}': expected login-free or passwords").into()
        });
    }
    if !cli::term::stdin_is_tty() {
        return Err(
            "auth-mode selection requires a terminal; re-run with --auth-mode login-free|passwords"
                .into(),
        );
    }
    let mut prompt = cliclack::Select::new("How do you want to sign in?");
    prompt = prompt
        .item(
            config::AuthMode::LoginFree,
            "Login-free",
            "no password; your browser signs you in; binds to 127.0.0.1",
        )
        .item(
            config::AuthMode::Passwords,
            "Passwords",
            "set a password and sign in on the web",
        );
    let mode = prompt.interact().map_err(|e| -> Box<dyn std::error::Error> {
        if e.kind() == std::io::ErrorKind::Interrupted {
            "cancelled".into()
        } else {
            format!("auth-mode selection failed: {e}").into()
        }
    })?;
    if mode == config::AuthMode::LoginFree
        && !cli::term::confirm(
            &format!("{}\n\nProceed?", config::login_free_caution()),
            "--auth-mode login-free",
        )?
    {
        return Err("cancelled".into());
    }
    Ok(mode)
}

/// Prompt for the operator's password in `--auth-mode passwords`. Masked on a
/// TTY; read-a-line when piped (so scripts can supply it), matching the `user
/// create` flow.
fn prompt_password_for_auth_mode() -> Result<String, Box<dyn std::error::Error>> {
    if cli::term::stdin_is_tty() {
        Ok(cliclack::password("Operator password").interact()?)
    } else {
        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf)?;
        Ok(buf.trim().to_string())
    }
}

// clap can't express the --config conflict, and init threads many small flags;
// the repo tolerates this for command handlers (see cli/import.rs).
#[allow(clippy::too_many_arguments)]
async fn cmd_init(
    config_flag: Option<&std::path::Path>,
    db_flag: Option<&std::path::Path>,
    json_flag: bool,
    no_service: bool,
    here: bool,
    name: Option<String>,
    auth_mode_flag: Option<String>,
    password_flag: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    use cli::ui;
    // clap can't express this conflict: --config is a global arg on the
    // top-level Cli, out of the subcommand's conflicts_with reach.
    if here && config_flag.is_some() {
        return Err("--here conflicts with --config — pick one location".into());
    }
    let json = cli::term::wants_json(json_flag);
    if !json {
        ui::intro("lific init");
    }
    // LIF-292 + LIF-295: the instance roots wherever the config file lives —
    // an explicit --config, the cwd (--here / existing ./lific.toml), or the
    // OS-standard config+data dirs by default.
    let (config_path, default_db) = resolve_init_target(
        config_flag,
        here,
        std::path::Path::new("lific.toml").exists(),
        Config::os_default_instance(),
    );
    let created_config = if config_path.exists() {
        false
    } else {
        if let Some(parent) = config_path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let toml = match &default_db {
            Some(db) => Config::default_toml_with_db(db),
            None => Config::default_toml(),
        };
        std::fs::write(&config_path, toml)?;
        true
    };

    // (Re)load from the file init actually operates on, so a relative
    // database.path anchors to the config's own directory — the same
    // resolution the installed service (WorkingDirectory = that directory)
    // applies at runtime. The pre-dispatch Config::load can't have done
    // this when the file didn't exist yet. Applied again after the auth-mode
    // edit rewrites the file (LIFIC-25).
    let mut cfg = load_config_for_init(&config_path, db_flag)?;

    // Create + migrate the database and seed instance settings now, while the
    // instance has zero users — this is the moment the authz-enforced default
    // is decided. The data dir may not exist yet under the OS-dirs layout
    // (LIF-295: db lives in ~/.local/share/lific/, not beside the config).
    if let Some(parent) = cfg.database.path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let pool = db::open(&cfg.database.path)?;
    {
        let conn = pool.write()?;
        db::queries::settings::ensure(&conn, cfg.auth.allow_signup)?;
    }

    // LIFIC-25: on a fresh install (no human operator yet) the operator picks
    // an auth mode — login-free or passwords. Resolve it (flag, or an
    // interactive TTY menu), persist the choice to the config file + database,
    // and create the first admin in that mode. An existing instance with users
    // skips all of this entirely.
    let created_admin = if !auth::has_human_operator(&pool) {
        let mode = resolve_auth_mode(&auth_mode_flag)?;

        // Persist the choice into the config file, editing it in place (the
        // change set `[auth] required` and `[server] host`; every other section
        // and setting survives). Reload cfg so downstream (local_url, JSON,
        // service plan) reflects required/host.
        let existing = std::fs::read_to_string(&config_path).unwrap_or_default();
        let new_toml = Config::apply_auth_mode(&existing, mode.required(), mode.host())?;
        std::fs::write(&config_path, new_toml)?;
        cfg = load_config_for_init(&config_path, db_flag)?;

        let op_name = match name {
            Some(n) => n,
            None => cli::term::prompt_text("What's your name?", "--name")
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?,
        };

        // Write web_auto_login to the DB beside the admin (it lives in the
        // database, not the config). On for login-free so the browser signs the
        // operator in; off for password mode.
        let conn = pool.write()?;
        db::queries::settings::update(
            &conn,
            db::queries::settings::InstanceSettingsPatch {
                web_auto_login: Some(mode.web_auto_login()),
                ..Default::default()
            },
        )?;

        let admin = if mode.passwordless() {
            db::queries::users::create_passwordless_admin(&conn, &op_name)?
        } else {
            let pw = match &password_flag {
                Some(p) => p.clone(),
                None => prompt_password_for_auth_mode()?,
            };
            db::queries::users::create_first_admin_with_password(&conn, &op_name, &pw)?
        };
        info!(operator = %admin.username, mode = mode.as_str(), "created first human admin");
        Some(admin)
    } else {
        None
    };

    // Mint the initial API key HERE, in the operator's terminal. Once the
    // server runs as a background service, its stdout goes to the journal
    // where nobody would see a printed key. LIFIC-9: once a human admin exists
    // we stop auto-minting the unbound "default" key — the operator is a real
    // user now, and keys are minted on demand via `lific key create`.
    let new_key = if auth::should_mint_initial_key(&pool) {
        let manager =
            auth::create_key_manager().map_err(|e| format!("key manager init failed: {e}"))?;
        Some(auth::create_api_key(&pool, &manager, "default")?)
    } else {
        None
    };
    // Release the CLI's DB handles before the service process opens the file.
    drop(pool);

    // Background service: the README's 60-second setup has to end with a
    // server that is still alive tomorrow, not a process tied to a terminal.
    let url = local_url(&cfg);
    let mut service_report = None;
    let mut service_error = None;
    let mut healthy = false;
    if !no_service {
        match cli::service::detect() {
            Some(mgr) => {
                let plan = cli::service::ServicePlan::for_config_file(&config_path)?;
                match cli::service::install(mgr, &plan) {
                    Ok(report) => {
                        healthy = wait_healthy(&url, std::time::Duration::from_secs(15)).await;
                        // A 200 alone can lie (another process may own the
                        // port while our unit crash-loops on AddrInUse), and
                        // silence alone is ambiguous. Cross-check the unit's
                        // own active state to say something precise.
                        let active =
                            cli::service::status(mgr).map(|s| s.active).unwrap_or(false);
                        match (healthy, active) {
                            (true, true) => {}
                            (true, false) => {
                                healthy = false;
                                service_error = Some(format!(
                                    "something is answering at {url}, but it isn't the \
                                     installed service — another server is likely already \
                                     using the port. Check: {}",
                                    cli::service::logs_hint(mgr)
                                ));
                            }
                            (false, false) => {
                                service_error = Some(format!(
                                    "the service failed to stay running — most often the \
                                     port is already in use. Check: {}",
                                    cli::service::logs_hint(mgr)
                                ));
                            }
                            (false, true) => {
                                service_error = Some(format!(
                                    "the service is running but didn't answer at {url} \
                                     within 15s. Check: {}",
                                    cli::service::logs_hint(mgr)
                                ));
                            }
                        }
                        service_report = Some((mgr, report));
                    }
                    Err(e) => service_error = Some(e),
                }
            }
            None => {
                service_error = Some(
                    "no supported service manager found (needs a systemd user session on \
                     Linux, or launchd on macOS)"
                        .to_string(),
                )
            }
        }
    }

    if json {
        let out = serde_json::json!({
            "config": { "path": config_path.display().to_string(), "created": created_config },
            "database": cfg.database.path.display().to_string(),
            "key": new_key,
            "admin": created_admin.as_ref().map(|a| serde_json::json!({
                "id": a.id,
                "username": a.username,
                "display_name": a.display_name,
                "is_admin": a.is_admin,
            })),
            "url": url,
            "service": {
                "requested": !no_service,
                "installed": service_report.as_ref().map(|(_, r)| serde_json::to_value(r).unwrap_or_default()),
                "healthy": healthy,
                "error": service_error,
            },
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    if created_config {
        ui::step(format!("Created {}", config_path.display()));
    } else {
        ui::step(format!("Using existing {}", config_path.display()));
    }
    ui::step(format!("Database ready {}", ui::dim(cfg.database.path.display())));

    if let Some(ref admin) = created_admin {
        ui::step(format!(
            "First operator {} created — passwordless mode is on",
            ui::command(&admin.display_name)
        ));
    }

    if let Some(ref key) = new_key {
        ui::note(
            "Initial API key — save it now, it will not be shown again",
            format!("{key}\n\nUse it as: Authorization: Bearer <key>"),
        );
    }

    if let Some((mgr, ref report)) = service_report {
        ui::step(format!(
            "Service installed — {} {}",
            report.manager,
            ui::dim(&report.definition)
        ));
        if report.linger == Some(false) {
            ui::warn(
                "`loginctl enable-linger` didn't succeed — the service will stop when you \
                 log out. Run it manually to fix that.",
            );
        }
        if healthy {
            ui::step(format!("Lific is running at {}", ui::command(&url)));
        } else if let Some(ref e) = service_error {
            ui::warn(e);
        } else {
            ui::warn(format!(
                "service started but the server didn't answer at {url} within 15s — check \
                 logs: {}",
                cli::service::logs_hint(mgr)
            ));
        }
    } else if no_service {
        ui::info(format!(
            "Service install skipped (--no-service). Run the server with {}",
            ui::command("lific start")
        ));
    } else if let Some(e) = service_error {
        ui::warn(format!("couldn't install a background service: {e}"));
        ui::info(format!(
            "run the server in the foreground instead: {}",
            ui::command("lific start")
        ));
    }

    ui::note(
        "Next steps",
        format!(
            "1. Open {url} and create your account\n2. {}\n3. {}   {}",
            ui::command("lific user promote --username <you>"),
            ui::command("lific connect"),
            ui::dim("# wire up your AI tools"),
        ),
    );

    let mut outro_msg = format!("Verify anytime with {}", ui::command("lific doctor"));
    if service_report.is_some() {
        outro_msg.push_str(&format!(
            " · manage the service with {}",
            ui::command("lific service status|restart|stop|uninstall")
        ));
    }
    ui::outro(outro_msg);
    Ok(())
}

/// `lific service <action>`: manage the background service `init` installs.
fn cmd_service(
    cfg: &Config,
    config_flag: Option<&std::path::Path>,
    json_flag: bool,
    action: &ServiceAction,
) -> Result<(), Box<dyn std::error::Error>> {
    use cli::ui;
    let json = cli::term::wants_json(json_flag);
    let Some(mgr) = cli::service::detect() else {
        return Err("no supported service manager found (needs a systemd user session on \
                    Linux, or launchd on macOS)"
            .into());
    };
    match action {
        ServiceAction::Install => {
            // LIF-292: honor --config; the unit is rendered around this
            // exact file. Without the flag, discover the instance the same
            // way Config::load does (cwd → user config dir → system config
            // dir, LIF-295) so a bare install finds the OS-dirs instance
            // that a bare `lific init` created.
            let config_path: std::path::PathBuf = match config_flag {
                Some(p) => p.to_path_buf(),
                None => Config::discover_path()
                    .unwrap_or_else(|| std::path::PathBuf::from("lific.toml")),
            };
            if !config_path.exists() {
                return Err(format!(
                    "config not found at {} — run `lific init` first (or point --config at an \
                     existing lific.toml)",
                    config_path.display()
                )
                .into());
            }
            let plan = cli::service::ServicePlan::for_config_file(&config_path)?;
            let report = cli::service::install(mgr, &plan)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                ui::intro("lific service install");
                ui::step(format!(
                    "Service installed and started — {} {}",
                    report.manager,
                    ui::dim(&report.definition)
                ));
                if report.linger == Some(false) {
                    ui::warn(
                        "`loginctl enable-linger` didn't succeed — the service will stop \
                         when you log out. Run it manually to fix that.",
                    );
                }
                ui::outro(format!("Logs: {}", ui::command(cli::service::logs_hint(mgr))));
            }
        }
        ServiceAction::Uninstall => {
            let removed = cli::service::uninstall(mgr)?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({ "uninstalled": true, "definition": removed })
                );
            } else {
                ui::intro("lific service uninstall");
                ui::step(format!("Service stopped and uninstalled {}", ui::dim(&removed)));
                ui::outro(format!("Reinstall anytime with {}", ui::command("lific service install")));
            }
        }
        ServiceAction::Status => {
            let s = cli::service::status(mgr)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&s)?);
            } else if s.active {
                ui::step(format!(
                    "Service is running ({}) — {}",
                    s.manager,
                    ui::command(local_url(cfg))
                ));
            } else if s.installed {
                ui::error(format!(
                    "Service is installed but NOT running ({}). Start it: {}",
                    s.manager,
                    ui::command("lific service restart")
                ));
            } else {
                ui::error(format!(
                    "Service is not installed. Install it: {}",
                    ui::command("lific service install")
                ));
            }
            if !(s.installed && s.active) {
                std::process::exit(1);
            }
        }
        ServiceAction::Stop => {
            cli::service::stop(mgr)?;
            if json {
                println!("{}", serde_json::json!({ "stopped": true }));
            } else {
                ui::step(format!(
                    "Service stopped {}",
                    ui::dim("(still installed; it returns on reboot or `lific service restart`)")
                ));
            }
        }
        ServiceAction::Restart => {
            cli::service::restart(mgr)?;
            if json {
                println!("{}", serde_json::json!({ "restarted": true }));
            } else {
                ui::step(format!("Service restarted — {}", ui::command(local_url(cfg))));
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod init_target_tests {
    use super::{auth, cmd_init, resolve_init_target, Config};
    use crate::db;
    use std::path::{Path, PathBuf};

    fn os_default() -> Option<(PathBuf, PathBuf)> {
        Some((
            PathBuf::from("/home/u/.config/lific/lific.toml"),
            PathBuf::from("/home/u/.local/share/lific/lific.db"),
        ))
    }

    // LIF-295: bare init targets the OS dirs, with an explicit db path so the
    // generated config can split config dir from data dir.
    #[test]
    fn bare_init_targets_os_dirs() {
        let (config, db) = resolve_init_target(None, false, false, os_default());
        assert_eq!(config, Path::new("/home/u/.config/lific/lific.toml"));
        assert_eq!(db.as_deref(), Some(Path::new("/home/u/.local/share/lific/lific.db")));
    }

    #[test]
    fn here_flag_forces_cwd_layout() {
        let (config, db) = resolve_init_target(None, true, false, os_default());
        assert_eq!(config, Path::new("lific.toml"));
        assert_eq!(db, None, "cwd layout keeps the relative default db");
    }

    // Repairing an existing directory-local instance must win over creating
    // a second instance in the OS dirs.
    #[test]
    fn existing_cwd_config_wins_over_os_dirs() {
        let (config, db) = resolve_init_target(None, false, true, os_default());
        assert_eq!(config, Path::new("lific.toml"));
        assert_eq!(db, None);
    }

    #[test]
    fn explicit_config_flag_wins_over_everything() {
        let (config, db) = resolve_init_target(
            Some(Path::new("/srv/lific/lific.toml")),
            false,
            true,
            os_default(),
        );
        assert_eq!(config, Path::new("/srv/lific/lific.toml"));
        assert_eq!(db, None);
    }

    #[test]
    fn unresolvable_platform_dirs_fall_back_to_cwd() {
        let (config, db) = resolve_init_target(None, false, false, None);
        assert_eq!(config, Path::new("lific.toml"));
        assert_eq!(db, None);
    }

    // The --here / --config conflict is enforced in cmd_init (clap can't
    // express it: --config is a global arg). The guard runs before any
    // filesystem access, so calling it here is side-effect free.
    #[tokio::test]
    async fn init_rejects_here_with_config() {
        let err = cmd_init(
            Some(Path::new("/tmp/nonexistent/lific.toml")),
            None,
            true, // json
            true, // no_service
            true, // here
            Some("test".into()),
            None, // auth_mode
            None, // password
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("--here conflicts with --config"));
    }

    // A temp dir that self-destructs, so cmd_init's filesystem writes stay out
    // of the repo tree and don't collide across tests.
    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "lific-init-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Run `lific init --config <dir>/lific.toml --no-service` for the operator
    /// `name` and assert on the DB state it wrote (stdout isn't a TTY under the
    /// test harness, so we can't capture cmd_init's printed JSON — instead we
    /// re-open the database and read back the shared facts).
    async fn run_init(
        dir: &TempDir,
        name: Option<&str>,
        auth_mode: Option<&str>,
        password: Option<&str>,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        let config_path = dir.path().join("lific.toml");
        cmd_init(
            Some(&config_path),
            None,
            true,  // json
            true,  // no_service
            false, // here
            name.map(str::to_string),
            auth_mode.map(str::to_string),
            password.map(str::to_string),
        )
        .await?;
        let cfg = Config::load(Some(&config_path))?;
        let pool = db::open(&cfg.database.path)?;
        let conn = pool.read().unwrap();
        let admin = crate::db::queries::users::first_admin(&conn)?;
        let settings = crate::db::queries::settings::get(&conn).ok();
        Ok(serde_json::json!({
            "admin": admin.as_ref().map(|a| a.username.clone()),
            "admin_display": admin.as_ref().map(|a| a.display_name.clone()),
            "keys": auth::has_any_keys(&pool),
            "host": cfg.server.host,
            "required": cfg.auth.required,
            "web_auto_login": settings.map(|s| s.web_auto_login),
        }))
    }

    // LIFIC-9: a fresh install (no humans) creates the first passwordless admin
    // when given `--name` non-interactively (login-free mode).
    #[tokio::test]
    async fn init_fresh_install_creates_first_admin_with_name() {
        let dir = TempDir::new();
        let out = run_init(&dir, Some("Blake Alston"), Some("login-free"), None)
            .await
            .unwrap();
        assert_eq!(out["admin"], serde_json::json!("blake-alston"));
    }

    // LIFIC-9: once a human admin exists, init skips minting the unbound
    // "default" key (passwordless mode) — no key is auto-generated.
    #[tokio::test]
    async fn init_fresh_install_skips_default_key_when_admin_created() {
        let dir = TempDir::new();
        let out = run_init(&dir, Some("Blake"), Some("login-free"), None)
            .await
            .unwrap();
        assert_eq!(out["admin"], serde_json::json!("blake"));
        assert_eq!(
            out["keys"], serde_json::json!(false),
            "a human operator exists, so no unbound default key is minted"
        );
    }

    // LIFIC-9: re-running init on an existing instance (admins already exist)
    // skips creation — idempotent, existing setup untouched.
    #[tokio::test]
    async fn init_existing_install_skips_admin_creation() {
        let dir = TempDir::new();
        let first = run_init(&dir, Some("Blake"), Some("login-free"), None)
            .await
            .unwrap();
        assert_eq!(first["admin"], serde_json::json!("blake"));

        // Second run with a different name must NOT create a second admin.
        let second = run_init(&dir, Some("Someone Else"), Some("passwords"), Some("hunter22!"))
            .await
            .unwrap();
        assert_eq!(
            second["admin"], serde_json::json!("blake"),
            "existing instance keeps its first admin"
        );
    }

    // LIFIC-25: login-free mode writes required=false, host=127.0.0.1,
    // web_auto_login=true, and a passwordless admin.
    #[tokio::test]
    async fn init_login_free_wires_config_db_and_passwordless_admin() {
        let dir = TempDir::new();
        let out = run_init(&dir, Some("Blake"), Some("login-free"), None)
            .await
            .unwrap();
        assert_eq!(out["admin_display"], serde_json::json!("Blake"));
        assert_eq!(out["required"], serde_json::json!(false));
        assert_eq!(out["host"], serde_json::json!("127.0.0.1"));
        assert_eq!(out["web_auto_login"], serde_json::json!(true));
    }

    // LIFIC-25: password mode writes required=true, leaves host unchanged,
    // web_auto_login=false, and creates an admin with the chosen password.
    #[tokio::test]
    async fn init_passwords_wires_config_db_and_passworded_admin() {
        let dir = TempDir::new();
        let out = run_init(&dir, Some("Blake"), Some("passwords"), Some("hunter22!"))
            .await
            .unwrap();
        assert_eq!(out["required"], serde_json::json!(true));
        // host is left at its default (0.0.0.0) — password mode never binds loopback.
        assert_eq!(out["host"], serde_json::json!("0.0.0.0"));
        assert_eq!(out["web_auto_login"], serde_json::json!(false));
        // Passworded admin can sign in.
        assert_eq!(out["admin"], serde_json::json!("blake"));
    }

    // LIFIC-25: an invalid --auth-mode is rejected.
    #[tokio::test]
    async fn init_rejects_invalid_auth_mode() {
        let dir = TempDir::new();
        let config_path = dir.path().join("lific.toml");
        let err = cmd_init(
            Some(&config_path),
            None,
            true, // json
            true, // no_service
            false,
            Some("Blake".to_string()),
            Some("bogus".to_string()),
            None,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("invalid --auth-mode"));
    }
}

#[cfg(test)]
mod http_backend_url_tests {
    use super::{http_backend_url, Config};

    #[test]
    fn maps_bind_any_hosts_to_loopback() {
        let mut cfg = Config::default();
        cfg.server.host = "0.0.0.0".into();
        cfg.server.port = 4567;

        assert_eq!(
            http_backend_url(None, None, &cfg),
            "http://127.0.0.1:4567"
        );
    }

    #[test]
    fn preserves_explicit_cli_and_public_urls() {
        let mut cfg = Config::default();
        cfg.server.public_url = Some("https://public.example.test".into());

        assert_eq!(
            http_backend_url(None, cfg.server.public_url.as_deref(), &cfg),
            "https://public.example.test"
        );
        assert_eq!(
            http_backend_url(
                Some("https://cli.example.test"),
                cfg.server.public_url.as_deref(),
                &cfg,
            ),
            "https://cli.example.test"
        );
    }
}
