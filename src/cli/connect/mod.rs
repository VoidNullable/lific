//! LIF-249: `lific connect` — write MCP config into AI clients from the CLI.
//!
//! The flagship onboarding command. It replaces the copy-a-snippet web page as
//! the primary path: pick clients (interactively or via `--client`), mint (or
//! reuse) an API key, and write correct MCP config directly into each client's
//! native config file — merging non-destructively.
//!
//! Structure:
//! - [`clients`] — the canonical server config + per-client schema/path matrix.
//! - [`writer`]  — format-native, merge-preserving JSON/TOML/YAML writers.
//! - this module — orchestration: CLI args → detection → key minting → writes →
//!   output, plus the optional AGENTS.md step (LIF-251).
//!
//! ## Key-minting & authz semantics (investigated, LIF-249)
//!
//! An API key with `user_id = NULL` (an "unassigned" key) behaves very
//! differently under the two authz modes (see `src/authz.rs`):
//! - **Legacy mode (default, `authz_enforced = false`):** an unassigned key
//!   resolves to `AuthUser = None`, which `require_role` passes unconditionally
//!   at Viewer/Maintainer. It can read and write everything — exactly like
//!   `lific start`'s first-run "default" key. Fine for a local single-user box.
//! - **Enforced mode (`authz_enforced = true`):** `None` is default-denied at
//!   every level and `visible_project_ids` returns the empty set — an
//!   unassigned key would **see nothing**. Shipping one there is a setup bug.
//!
//! Therefore `connect` prefers a **bot identity owned by a human** (parity with
//! the web UI's Connected Tools): the bot inherits its owner's role, so it works
//! under both modes. It only falls back to a plain unassigned key on a truly
//! fresh install (zero human users) — where enforcement can't be on yet (it
//! takes an admin to enable). If humans exist but none can be chosen
//! unambiguously and no `--user` was given, we surface guidance rather than mint
//! a key that might see nothing.
//!
//! **Per-tool identities (LIF-259).** `connect` mints ONE bot + key PER SELECTED
//! CLIENT, named after the tool the way the web UI's Connected Tools page does
//! (`{tool}-{owner.username}`, e.g. `opencode-blake`). This means the audit log
//! attributes each change to the specific harness ("OpenCode changed status"),
//! and CLI-connected tools show up on that page indistinguishable from
//! web-connected ones. On a fresh install (zero human users) it mints one plain
//! unassigned key per tool named just `{tool}` — still per-tool attribution in
//! the key name even without a human owner. `--key <k>` uses that one key
//! verbatim for every client (no minting); `--dry-run` uses a placeholder.
//!
//! **`--oauth` mode (LIF-259).** Writes the remote config WITHOUT any
//! `Authorization` header (URL only) so the client's native MCP OAuth flow takes
//! over. Mints nothing — no bot, no key. Only OAuth-capable clients are written;
//! the rest are surfaced as skipped outcomes with an explanatory note. Conflicts
//! with `--stdio` and `--key`.

pub mod clients;
pub mod writer;

use std::io::IsTerminal;
use std::path::PathBuf;

use crate::config::Config;
use crate::db::DbPool;

use clients::{ClientSpec, OauthSupport, Os, PathBase, Scope, ServerConfig};

/// Parsed, validated arguments for a `connect` run. Built from the CLI enum in
/// `cli/mod.rs` so the heavy lifting here is testable without clap.
pub struct ConnectArgs {
    pub clients: Vec<String>,
    pub scope: Scope,
    pub stdio: bool,
    /// Write header-less remote config and let the client's native MCP OAuth
    /// flow authenticate. Mints nothing. Conflicts with `--stdio`/`--key`.
    pub oauth: bool,
    pub url: Option<String>,
    pub key: Option<String>,
    pub user: Option<String>,
    pub yes: bool,
    pub dry_run: bool,
    pub skip_agents: bool,
}

/// The outcome for a single client write, for both human and JSON output.
#[derive(Debug, Default)]
pub struct ClientOutcome {
    pub id: String,
    pub display: String,
    pub format: String,
    pub path: Option<PathBuf>,
    pub action: Option<String>,
    pub notes: Vec<String>,
    pub error: Option<String>,
    pub manual_snippet: Option<String>,
    /// The full file body, for `--dry-run` display.
    pub dry_run_contents: Option<String>,
    /// The API key written into THIS client's config (LIF-259 per-tool keys).
    /// `None` for stdio, `--oauth`, and skipped clients.
    pub key: Option<String>,
    /// The post-connect auth command to run for this client under `--oauth`.
    pub auth_hint: Option<String>,
}

/// How this run's per-tool keys were obtained (all share one origin).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOrigin {
    /// Supplied by the user via `--key`.
    Provided,
    /// Minted as a bot identity owned by a human user.
    Bot,
    /// Minted as a plain unassigned key (fresh install, zero users).
    Unassigned,
}

/// The full result of a run, returned so `main` (and tests) can render it.
///
/// Per-client keys now live on each [`ClientOutcome::key`] (LIF-259). The
/// run-level `key_origin` records how those keys were obtained (all clients in a
/// run share the same origin: provided / bot / unassigned).
#[derive(Debug)]
pub struct ConnectResult {
    pub outcomes: Vec<ClientOutcome>,
    pub key_origin: Option<KeyOrigin>,
    pub agents_md: Option<AgentsMdOutcome>,
    pub dry_run: bool,
    pub stdio: bool,
    /// True when this run wrote header-less OAuth configs (`--oauth`).
    pub oauth: bool,
    pub url: String,
}

#[derive(Debug)]
pub struct AgentsMdOutcome {
    pub path: PathBuf,
    pub action: String,
}

/// Build the production [`PathBase`] from the real environment.
///
/// A `LIFIC_CONNECT_HOME` override is honored for the home dir. It exists for
/// smoke-testing so a manual run can be pointed at a scratch dir instead of the
/// operator's real `~/.config` — documented as test-only.
pub fn production_base() -> Result<PathBase, String> {
    let home = std::env::var_os("LIFIC_CONNECT_HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .ok_or_else(|| "could not determine home directory".to_string())?;
    let project = std::env::current_dir().map_err(|e| format!("cannot read cwd: {e}"))?;
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from);
    Ok(PathBase {
        home,
        project,
        os: Os::host(),
        appdata,
    })
}

/// Compute the default MCP URL for remote configs.
///
/// Prefer `server.public_url` (with `/mcp` appended if it isn't already there).
/// Otherwise `http://127.0.0.1:{port}/mcp` — never `0.0.0.0`, which is a bind
/// address, not something a client can dial.
pub fn default_url(cfg: &Config) -> String {
    if let Some(pu) = cfg.server.public_url.as_deref() {
        let trimmed = pu.trim().trim_end_matches('/');
        if trimmed.ends_with("/mcp") {
            return trimmed.to_string();
        }
        return format!("{trimmed}/mcp");
    }
    format!("http://127.0.0.1:{}/mcp", cfg.server.port)
}

/// Absolute DB path for stdio configs (canonicalized when the file exists, else
/// made absolute against cwd so the spawned server opens the right file).
pub fn absolute_db_path(cfg: &Config) -> String {
    let p = &cfg.database.path;
    if let Ok(canon) = std::fs::canonicalize(p) {
        return canon.display().to_string();
    }
    if p.is_absolute() {
        return p.display().to_string();
    }
    match std::env::current_dir() {
        Ok(cwd) => cwd.join(p).display().to_string(),
        Err(_) => p.display().to_string(),
    }
}

/// Build the canonical [`ServerConfig`] for one client. `key` is that client's
/// own key (ignored for stdio and oauth transports).
fn build_server_config(args: &ConnectArgs, cfg: &Config, key: &str) -> ServerConfig {
    if args.stdio {
        ServerConfig::stdio(absolute_db_path(cfg))
    } else if args.oauth {
        let url = args.url.clone().unwrap_or_else(|| default_url(cfg));
        ServerConfig::oauth_remote(url)
    } else {
        let url = args.url.clone().unwrap_or_else(|| default_url(cfg));
        ServerConfig::remote(url, key)
    }
}

// ── Client selection ─────────────────────────────────────────

/// Resolve the list of client ids to write. Explicit `--client` wins (each is
/// validated). With none given and a TTY, run the interactive picker. With none
/// and no TTY, refuse — naming the flags a non-interactive caller must pass.
///
/// Factored to take an injected `stdin_tty` and a picker closure so the refusal
/// branch is unit-testable (mirrors `term::confirm_inner`).
pub fn resolve_clients_inner(
    requested: &[String],
    stdin_tty: bool,
    base: &PathBase,
    scope: Scope,
    picker: impl FnOnce(&[DetectedClient]) -> Result<Vec<String>, String>,
) -> Result<Vec<String>, String> {
    if !requested.is_empty() {
        for id in requested {
            if clients::find_client(id).is_none() {
                return Err(format!(
                    "unknown client '{id}'. Known clients: {}",
                    clients::all_client_ids().join(", ")
                ));
            }
        }
        // De-dup while preserving order.
        let mut seen = std::collections::HashSet::new();
        return Ok(requested
            .iter()
            .filter(|id| seen.insert((*id).clone()))
            .cloned()
            .collect());
    }

    if !stdin_tty {
        return Err(format!(
            "no client selected and stdin is not a TTY. Pass --client <id> (repeatable) to choose \
             clients, and --yes to skip prompts. Known clients: {}",
            clients::all_client_ids().join(", ")
        ));
    }

    let detected = detect_clients(base, scope);
    picker(&detected)
}

/// A client and whether it was detected in the given scope.
#[derive(Debug, Clone)]
pub struct DetectedClient {
    pub id: String,
    pub display: String,
    pub detected: bool,
}

/// Probe the filesystem for every client's config presence in `scope`.
pub fn detect_clients(base: &PathBase, scope: Scope) -> Vec<DetectedClient> {
    clients::all_clients()
        .iter()
        .map(|c| DetectedClient {
            id: c.id.to_string(),
            display: c.display.to_string(),
            detected: c.detected(base, scope),
        })
        .collect()
}

/// The default interactive picker: a real arrow-key multiselect (space to
/// toggle, enter to confirm). Detected clients are listed first and
/// preselected; the rest follow so an undetected client can still be chosen.
fn interactive_picker(detected: &[DetectedClient]) -> Result<Vec<String>, String> {
    let any_installed = detected.iter().any(|c| c.detected);
    let mut ordered: Vec<&DetectedClient> = detected.iter().filter(|c| c.detected).collect();
    ordered.extend(detected.iter().filter(|c| !c.detected));

    let mut prompt = cliclack::multiselect(if any_installed {
        "Which clients should connect to Lific?"
    } else {
        "No installed clients detected in this scope — pick any to configure anyway:"
    })
    .required(true);
    for c in &ordered {
        prompt = prompt.item(
            c.id.clone(),
            &c.display,
            if c.detected { "detected" } else { "" },
        );
    }
    let initial: Vec<String> = ordered
        .iter()
        .filter(|c| c.detected)
        .map(|c| c.id.clone())
        .collect();
    if !initial.is_empty() {
        prompt = prompt.initial_values(initial);
    }
    prompt.interact().map_err(|e| {
        if e.kind() == std::io::ErrorKind::Interrupted {
            "cancelled".to_string()
        } else {
            format!("selection failed: {e}")
        }
    })
}

// ── Key minting ──────────────────────────────────────────────

/// How this run mints per-tool keys, resolved once up front (owner selection is
/// a run-wide decision) and then applied per selected client.
#[derive(Debug)]
enum KeySource {
    /// `--key <k>`: use this verbatim for every client, mint nothing.
    Provided(String),
    /// Humans exist: mint a bot `{tool}-{owner}` owned by `owner_id` per tool.
    Bot { owner_id: i64 },
    /// Fresh install (zero humans): mint a plain unassigned key named `{tool}`.
    FreshInstall,
}

impl KeySource {
    fn origin(&self) -> KeyOrigin {
        match self {
            KeySource::Provided(_) => KeyOrigin::Provided,
            KeySource::Bot { .. } => KeyOrigin::Bot,
            KeySource::FreshInstall => KeyOrigin::Unassigned,
        }
    }
}

/// Resolve the run-wide key source: `--key` short-circuits; otherwise pick an
/// owner (explicit `--user`, the sole human, or the sole admin — else require
/// `--user`). See the module docs for the authz rationale.
fn resolve_key_source(args: &ConnectArgs, pool: &DbPool) -> Result<KeySource, String> {
    if let Some(k) = &args.key {
        return Ok(KeySource::Provided(k.clone()));
    }
    match choose_owner(pool, args.user.as_deref())? {
        OwnerChoice::User(owner_id) => Ok(KeySource::Bot { owner_id }),
        OwnerChoice::FreshInstall => Ok(KeySource::FreshInstall),
    }
}

/// Mint (or rotate) the key for one specific tool under `source`.
///
/// - **Bot:** find-or-create the bot `{tool}-{owner}` (web-UI convention), with
///   the tool's display name, mint-or-rotate a key named after the bot, and
///   assign the key to the bot. The bot's `owner_id` points at the human, so
///   authz resolves bot → owner (src/authz.rs).
/// - **FreshInstall:** mint-or-rotate a plain unassigned key named just `{tool}`.
/// - **Provided:** the verbatim `--key` (no DB writes).
///
/// Returns the plaintext key for that tool.
fn mint_for_tool(
    source: &KeySource,
    spec: &ClientSpec,
    pool: &DbPool,
    manager: &api_keys_simplified::ApiKeyManagerV0,
) -> Result<String, String> {
    match source {
        KeySource::Provided(k) => Ok(k.clone()),
        KeySource::Bot { owner_id } => {
            // Bot username = `{tool}-{owner.username}`, matching the web UI's
            // Connected Tools (src/api/auth.rs create_bot) so a CLI-connected
            // bot is indistinguishable from a web-connected one.
            let owner_username = {
                let conn = pool.read().map_err(|e| e.to_string())?;
                crate::db::queries::users::get_user_by_id(&conn, *owner_id)
                    .map_err(|e| e.to_string())?
                    .username
            };
            let bot_username = format!("{}-{}", spec.id, owner_username);
            let bot_id = {
                let conn = pool.write().map_err(|e| e.to_string())?;
                match crate::db::queries::users::find_bot_by_username(&conn, &bot_username)
                    .map_err(|e| e.to_string())?
                {
                    Some(existing) => existing.id,
                    None => crate::db::queries::users::create_bot_user(
                        &conn,
                        *owner_id,
                        &bot_username,
                        spec.display,
                    )
                    .map_err(|e| e.to_string())?
                    .id,
                }
            };
            let key = mint_or_rotate(pool, manager, &bot_username)?;
            {
                let conn = pool.write().map_err(|e| e.to_string())?;
                crate::db::queries::users::assign_key_to_user(&conn, &bot_username, bot_id)
                    .map_err(|e| e.to_string())?;
            }
            Ok(key)
        }
        KeySource::FreshInstall => {
            // Zero human users: enforcement can't be on (needs an admin to
            // enable), so a plain unassigned key behaves like `lific start`'s
            // first-run default key. Named just `{tool}` — per-tool attribution
            // in the key name even without a human owner.
            mint_or_rotate(pool, manager, spec.id)
        }
    }
}

/// Create a key named `name`, or — if an active key with that name already
/// exists (a previous `connect` run) — rotate it instead so re-running
/// `connect` (e.g. to add another client later) always succeeds with a fresh
/// plaintext. Rotation preserves any existing user binding.
fn mint_or_rotate(
    pool: &DbPool,
    manager: &api_keys_simplified::ApiKeyManagerV0,
    name: &str,
) -> Result<String, String> {
    let active_exists = {
        let conn = pool.read().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT COUNT(*) > 0 FROM api_keys WHERE name = ?1 AND revoked = 0",
            rusqlite::params![name],
            |row| row.get::<_, bool>(0),
        )
        .unwrap_or(false)
    };
    if active_exists {
        crate::auth::rotate_api_key(pool, manager, name).map_err(|e| e.to_string())
    } else {
        crate::auth::create_api_key(pool, manager, name).map_err(|e| e.to_string())
    }
}

enum OwnerChoice {
    User(i64),
    FreshInstall,
}

fn choose_owner(pool: &DbPool, requested_user: Option<&str>) -> Result<OwnerChoice, String> {
    let conn = pool.read().map_err(|e| e.to_string())?;

    if let Some(username) = requested_user {
        let u = crate::db::queries::users::get_user_by_username(&conn, username)
            .map_err(|_| format!("user '{username}' not found"))?;
        return Ok(OwnerChoice::User(u.id));
    }

    let users = crate::db::queries::users::list_users(&conn).map_err(|e| e.to_string())?;
    let humans: Vec<_> = users.iter().filter(|u| !u.is_bot).collect();

    match humans.len() {
        0 => Ok(OwnerChoice::FreshInstall),
        1 => Ok(OwnerChoice::User(humans[0].id)),
        _ => {
            // Prefer a single admin if there's exactly one; otherwise require
            // an explicit choice rather than guessing (and risk a key that sees
            // nothing under enforcement, or is owned by the wrong person).
            let admins: Vec<_> = humans.iter().filter(|u| u.is_admin).collect();
            if admins.len() == 1 {
                Ok(OwnerChoice::User(admins[0].id))
            } else {
                Err(
                    "multiple users exist — pass --user <username> to choose which user owns the \
                     connection's API key (it inherits that user's project access)."
                        .into(),
                )
            }
        }
    }
}

// ── The run ──────────────────────────────────────────────────

/// Execute a `connect` run: select clients, then per client mint/reuse its own
/// key (LIF-259) and render or write its config; finally (optionally) update
/// AGENTS.md. Pure enough to test end-to-end against a temp base + in-memory DB.
pub fn run(
    args: &ConnectArgs,
    cfg: &Config,
    pool: &DbPool,
    base: &PathBase,
) -> Result<ConnectResult, String> {
    // Flag conflicts: --oauth mints nothing and writes header-less config, so a
    // stdio transport or an explicit key make no sense together with it.
    if args.oauth && args.stdio {
        return Err(
            "--oauth and --stdio are mutually exclusive: --oauth writes a header-less remote \
             config for the client's native OAuth flow, while --stdio writes a local spawn with \
             no network auth at all."
                .into(),
        );
    }
    if args.oauth && args.key.is_some() {
        return Err(
            "--oauth and --key are mutually exclusive: --oauth writes no key (the client \
             obtains its own token via OAuth), so passing --key contradicts it."
                .into(),
        );
    }

    let stdin_tty = std::io::stdin().is_terminal();
    let selected = resolve_clients_inner(
        &args.clients,
        stdin_tty,
        base,
        args.scope,
        interactive_picker,
    )?;

    // Resolve how per-tool keys are minted (once — owner selection is run-wide).
    // Not needed for stdio (no key) or --oauth (mints nothing), and skipped in
    // dry-run so a preview never touches the DB.
    let needs_minting = !args.stdio && !args.oauth && !args.dry_run;
    let key_source = if needs_minting {
        Some(resolve_key_source(args, pool)?)
    } else if args.dry_run && !args.stdio && !args.oauth {
        // Dry-run still reports an origin so output matches a real run's shape.
        Some(KeySource::Provided(
            "lific_sk-live-DRYRUN000000000000000000000000".to_string(),
        ))
    } else {
        None
    };
    let key_origin = key_source.as_ref().map(|s| s.origin());

    let manager = if needs_minting {
        Some(
            crate::auth::create_key_manager()
                .map_err(|e| format!("key manager init failed: {e}"))?,
        )
    } else {
        None
    };

    let outcomes = write_all_clients(
        &selected,
        args,
        cfg,
        pool,
        base,
        key_source.as_ref(),
        manager.as_ref(),
    )?;

    // AGENTS.md (LIF-251).
    let agents_md = maybe_write_agents_md(args, base, stdin_tty)?;

    // A representative URL/db-path for the run-level summary.
    let url = if args.stdio {
        absolute_db_path(cfg)
    } else {
        args.url.clone().unwrap_or_else(|| default_url(cfg))
    };

    Ok(ConnectResult {
        outcomes,
        key_origin,
        agents_md,
        dry_run: args.dry_run,
        stdio: args.stdio,
        oauth: args.oauth,
        url,
    })
}

/// Write (or render, under `--dry-run`) every selected client, minting each
/// client's own key as it goes (LIF-259). Errors only on a key-minting failure;
/// per-client write failures and skips are recorded as outcomes so the run
/// continues.
fn write_all_clients(
    selected: &[String],
    args: &ConnectArgs,
    cfg: &Config,
    pool: &DbPool,
    base: &PathBase,
    key_source: Option<&KeySource>,
    manager: Option<&api_keys_simplified::ApiKeyManagerV0>,
) -> Result<Vec<ClientOutcome>, String> {
    let mut outcomes = Vec::new();
    for id in selected {
        let Some(spec) = clients::find_client(id) else {
            continue;
        };

        // --oauth: skip OAuth-incapable clients with an explanatory note rather
        // than silently dropping them (LIF-259).
        if args.oauth
            && let OauthSupport::Unsupported { reason } = spec.oauth
        {
            outcomes.push(ClientOutcome {
                id: id.clone(),
                display: spec.display.to_string(),
                format: spec.format.as_str().to_string(),
                error: Some(format!("{} does not support --oauth; skipped", spec.display)),
                notes: vec![reason.to_string()],
                ..Default::default()
            });
            continue;
        }

        let Some(path) = spec.path_for(base, args.scope) else {
            outcomes.push(ClientOutcome {
                id: id.clone(),
                display: spec.display.to_string(),
                format: spec.format.as_str().to_string(),
                error: Some(format!(
                    "{} has no {}-scope config; skipped",
                    spec.display,
                    args.scope.as_str()
                )),
                ..Default::default()
            });
            continue;
        };

        // Mint this client's own key (per-tool). Only when a real remote write
        // with minting is happening; stdio/oauth/dry-run supply their own.
        let this_key = match (key_source, manager) {
            (Some(source), Some(mgr)) => match mint_for_tool(source, &spec, pool, mgr) {
                Ok(k) => Some(k),
                Err(e) => {
                    // Minting failed for this tool — record and keep going.
                    outcomes.push(ClientOutcome {
                        id: id.clone(),
                        display: spec.display.to_string(),
                        format: spec.format.as_str().to_string(),
                        path: Some(path),
                        error: Some(format!("key minting failed: {e}")),
                        ..Default::default()
                    });
                    continue;
                }
            },
            // Dry-run placeholder (Provided) with no manager, or provided --key.
            _ => match key_source {
                Some(KeySource::Provided(k)) => Some(k.clone()),
                _ => None,
            },
        };

        let server = build_server_config(args, cfg, this_key.as_deref().unwrap_or(""));
        let entry = spec.compile(&server);

        // The per-client key we surface: none for stdio/oauth (no header key).
        let out_key = if args.stdio || args.oauth {
            None
        } else {
            this_key.clone()
        };
        let auth_hint = if args.oauth {
            match spec.oauth {
                OauthSupport::Capable { hint } => Some(hint.to_string()),
                OauthSupport::Unsupported { .. } => None,
            }
        } else {
            None
        };

        if args.dry_run {
            match writer::render(&path, spec.format, &entry) {
                Ok(rendered) => outcomes.push(ClientOutcome {
                    id: id.clone(),
                    display: spec.display.to_string(),
                    format: spec.format.as_str().to_string(),
                    path: Some(path),
                    action: Some(rendered.action.as_str().to_string()),
                    notes: entry.notes.clone(),
                    dry_run_contents: Some(rendered.contents),
                    key: out_key,
                    auth_hint,
                    ..Default::default()
                }),
                Err(e) => outcomes.push(ClientOutcome {
                    id: id.clone(),
                    display: spec.display.to_string(),
                    format: spec.format.as_str().to_string(),
                    path: Some(path),
                    notes: entry.notes.clone(),
                    error: Some(e.message.clone()),
                    manual_snippet: e.manual_snippet,
                    ..Default::default()
                }),
            }
        } else {
            match writer::write(&path, spec.format, &entry) {
                Ok(action) => outcomes.push(ClientOutcome {
                    id: id.clone(),
                    display: spec.display.to_string(),
                    format: spec.format.as_str().to_string(),
                    path: Some(path),
                    action: Some(action.as_str().to_string()),
                    notes: entry.notes.clone(),
                    key: out_key,
                    auth_hint,
                    ..Default::default()
                }),
                Err(e) => outcomes.push(ClientOutcome {
                    id: id.clone(),
                    display: spec.display.to_string(),
                    format: spec.format.as_str().to_string(),
                    path: Some(path),
                    notes: entry.notes.clone(),
                    error: Some(e.message.clone()),
                    manual_snippet: e.manual_snippet,
                    ..Default::default()
                }),
            }
        }
    }
    Ok(outcomes)
}

/// Decide whether and how to touch AGENTS.md for this run.
///
/// Only in project scope, or when cwd looks like a project (has `.git`).
/// `--skip-agents` opts out silently. In interactive mode we'd ask; here the
/// consent model is: with `--yes` (or `--skip-agents`) the decision is explicit,
/// so in project scope with `--yes` we write it. Without a TTY and without
/// `--yes`, we skip (don't hang, don't surprise-write).
fn maybe_write_agents_md(
    args: &ConnectArgs,
    base: &PathBase,
    stdin_tty: bool,
) -> Result<Option<AgentsMdOutcome>, String> {
    if args.skip_agents {
        return Ok(None);
    }
    if args.dry_run {
        return Ok(None);
    }

    let looks_like_project =
        args.scope == Scope::Project || base.project.join(".git").exists();
    if !looks_like_project {
        return Ok(None);
    }

    // Consent: explicit --yes writes; interactive TTY asks; otherwise skip.
    let consented = if args.yes {
        true
    } else if stdin_tty {
        cliclack::confirm(
            "Write a Lific block into ./AGENTS.md so agents in this repo know about it?",
        )
        .initial_value(true)
        .interact()
        .unwrap_or(false)
    } else {
        false
    };
    if !consented {
        return Ok(None);
    }

    let path = base.project.join("AGENTS.md");
    let action = crate::cli::agents_md::write(&path, None)
        .map_err(|e| format!("AGENTS.md update failed: {e}"))?;
    Ok(Some(AgentsMdOutcome {
        path,
        action: action.as_str().to_string(),
    }))
}

// ── Output rendering ─────────────────────────────────────────

/// Render a run result to stdout, honoring `json`.
pub fn print_result(result: &ConnectResult, json: bool) {
    if json {
        print_json(result);
    } else {
        print_human(result);
    }
}

fn print_json(result: &ConnectResult) {
    // LIF-259: keys are per-client now. Each client carries its own `key`
    // (null for stdio/oauth/skipped) and, under --oauth, its `auth_hint`.
    let clients: Vec<serde_json::Value> = result
        .outcomes
        .iter()
        .map(|o| {
            serde_json::json!({
                "id": o.id,
                "format": o.format,
                "path": o.path.as_ref().map(|p| p.display().to_string()),
                "action": o.action,
                "notes": o.notes,
                "error": o.error,
                "manual_snippet": o.manual_snippet,
                "contents": o.dry_run_contents,
                "key": o.key,
                "auth_hint": o.auth_hint,
            })
        })
        .collect();
    let out = serde_json::json!({
        "clients": clients,
        // Top-level key is always null now — keys live per-client above.
        "key": serde_json::Value::Null,
        "dry_run": result.dry_run,
        "stdio": result.stdio,
        "oauth": result.oauth,
        "url": result.url,
        "agents_md": result.agents_md.as_ref().map(|a| serde_json::json!({
            "path": a.path.display().to_string(),
            "action": a.action,
        })),
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
}

fn print_human(result: &ConnectResult) {
    use crate::cli::ui;

    if result.dry_run {
        ui::info("Dry run — no files were written.");
    }
    for o in &result.outcomes {
        match (&o.action, &o.error) {
            (Some(action), _) => {
                let path = o
                    .path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                ui::step(format!("{} — {action} {}", o.display, ui::dim(&path)));
            }
            (None, Some(err)) => {
                ui::warn(format!("{} — skipped: {err}", o.display));
                if let Some(snippet) = &o.manual_snippet {
                    ui::note(format!("{} — merge this in manually", o.display), snippet);
                }
            }
            (None, None) => {}
        }
        for note in &o.notes {
            ui::info(note);
        }
        // LIF-259: this tool's own key right under its line.
        if let Some(key) = &o.key {
            // Codex reads its key from an env var — show the export for it.
            let body = if o.id == "codex" {
                format!("{key}\n\nexport LIFIC_API_KEY=\"{key}\"")
            } else {
                key.clone()
            };
            ui::note(format!("{} API key", o.display), body);
        }
        // --oauth: the client's native auth command instead of a key.
        if let Some(hint) = &o.auth_hint {
            ui::info(format!("Next: {}", ui::command(hint)));
        }
        if result.dry_run
            && let Some(contents) = &o.dry_run_contents
        {
            let path = o
                .path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            ui::note(path, contents.trim_end());
        }
    }

    if let Some(a) = &result.agents_md {
        ui::step(format!(
            "AGENTS.md {} {}",
            a.action,
            ui::dim(a.path.display())
        ));
    }

    // One consolidated warning when real keys were written (LIF-259).
    let wrote_any_key = result.outcomes.iter().any(|o| o.key.is_some());
    if wrote_any_key {
        match result.key_origin {
            Some(KeyOrigin::Provided) => {}
            _ => {
                ui::warn("Save the key(s) above now. They will never be shown again.");
            }
        }
        if let Some(KeyOrigin::Unassigned) = result.key_origin {
            ui::info(
                "Unassigned keys — full access on this local instance. Create a user and \
                 re-run --user <name> if you enable project authorization.",
            );
        }
    }

    ui::outro("Restart your client(s) to pick up the new MCP server.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn cfg_with_port(port: u16) -> Config {
        let mut c = Config::default();
        c.server.port = port;
        c
    }

    fn base(dir: &std::path::Path) -> PathBase {
        PathBase {
            home: dir.join("home"),
            project: dir.join("proj"),
            os: Os::Linux,
            appdata: None,
        }
    }

    fn tmp() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "lific-connect-run-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    // ── default_url ──────────────────────────────────────────

    #[test]
    fn default_url_uses_loopback_and_port_not_bind_host() {
        let c = cfg_with_port(9999);
        assert_eq!(default_url(&c), "http://127.0.0.1:9999/mcp");
    }

    #[test]
    fn default_url_prefers_public_url_and_appends_mcp() {
        let mut c = Config::default();
        c.server.public_url = Some("https://lific.example.com".into());
        assert_eq!(default_url(&c), "https://lific.example.com/mcp");
    }

    #[test]
    fn default_url_public_url_already_has_mcp() {
        let mut c = Config::default();
        c.server.public_url = Some("https://lific.example.com/mcp".into());
        assert_eq!(default_url(&c), "https://lific.example.com/mcp");
    }

    // ── resolve_clients_inner ────────────────────────────────

    fn no_picker(_: &[DetectedClient]) -> Result<Vec<String>, String> {
        panic!("picker must not be called when --client is given or stdin is not a TTY");
    }

    #[test]
    fn resolve_explicit_clients_validates_and_dedups() {
        let dir = tmp();
        let b = base(&dir);
        let got = resolve_clients_inner(
            &["opencode".into(), "codex".into(), "opencode".into()],
            true,
            &b,
            Scope::Global,
            no_picker,
        )
        .unwrap();
        assert_eq!(got, vec!["opencode".to_string(), "codex".to_string()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_unknown_client_errors() {
        let dir = tmp();
        let b = base(&dir);
        let err = resolve_clients_inner(
            &["nope".into()],
            true,
            &b,
            Scope::Global,
            no_picker,
        )
        .unwrap_err();
        assert!(err.contains("unknown client"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_no_client_non_tty_refuses_naming_flags() {
        let dir = tmp();
        let b = base(&dir);
        let err = resolve_clients_inner(&[], false, &b, Scope::Global, no_picker).unwrap_err();
        assert!(err.contains("--client"), "must name --client: {err}");
        assert!(err.contains("--yes"), "must name --yes: {err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_no_client_tty_calls_picker() {
        let dir = tmp();
        let b = base(&dir);
        let got = resolve_clients_inner(&[], true, &b, Scope::Global, |_| {
            Ok(vec!["cursor".into()])
        })
        .unwrap();
        assert_eq!(got, vec!["cursor".to_string()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── detection ────────────────────────────────────────────

    #[test]
    fn detect_finds_only_present_clients() {
        let dir = tmp();
        let b = base(&dir);
        // Create ~/.cursor/ and ~/.codex/config.toml in the injected home.
        std::fs::create_dir_all(b.home.join(".cursor")).unwrap();
        std::fs::create_dir_all(b.home.join(".codex")).unwrap();
        std::fs::write(b.home.join(".codex").join("config.toml"), "").unwrap();

        let detected = detect_clients(&b, Scope::Global);
        let by_id = |id: &str| detected.iter().find(|c| c.id == id).unwrap().detected;
        assert!(by_id("cursor"), "cursor should be detected");
        assert!(by_id("codex"), "codex should be detected");
        assert!(!by_id("gemini"), "gemini should not be detected");
        assert!(!by_id("windsurf"), "windsurf should not be detected");
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── end-to-end run ───────────────────────────────────────

    fn args(clients: &[&str], scope: Scope) -> ConnectArgs {
        ConnectArgs {
            clients: clients.iter().map(|s| s.to_string()).collect(),
            scope,
            stdio: false,
            oauth: false,
            url: Some("http://127.0.0.1:3456/mcp".into()),
            key: Some("lific_sk-live-TESTKEY".into()),
            user: None,
            yes: true,
            dry_run: false,
            skip_agents: true,
        }
    }

    /// A single user for tests that exercise per-tool bot minting.
    fn seed_user(pool: &DbPool, username: &str, admin: bool) -> i64 {
        let conn = pool.write().unwrap();
        crate::db::queries::users::create_user(
            &conn,
            &crate::db::models::CreateUser {
                username: username.into(),
                email: format!("{username}@test.com"),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: admin,
                is_bot: false,
            },
        )
        .unwrap()
        .id
    }

    #[test]
    fn run_writes_project_scope_configs_and_skips_no_project_clients() {
        let dir = tmp();
        let b = base(&dir);
        let pool = db::open_memory().unwrap();
        let cfg = Config::default();
        // goose has no project path → should be skipped with a warning.
        let a = args(&["opencode", "codex", "goose"], Scope::Project);
        let result = run(&a, &cfg, &pool, &b).unwrap();

        let oc = result.outcomes.iter().find(|o| o.id == "opencode").unwrap();
        assert_eq!(oc.action.as_deref(), Some("created"));
        assert_eq!(
            oc.key.as_deref(),
            Some("lific_sk-live-TESTKEY"),
            "provided --key is used verbatim per client"
        );
        assert!(b.project.join("opencode.json").exists());

        let cx = result.outcomes.iter().find(|o| o.id == "codex").unwrap();
        assert_eq!(cx.action.as_deref(), Some("created"));
        assert_eq!(cx.key.as_deref(), Some("lific_sk-live-TESTKEY"));
        assert!(b.project.join(".codex/config.toml").exists());

        let goose = result.outcomes.iter().find(|o| o.id == "goose").unwrap();
        assert!(goose.action.is_none());
        assert!(
            goose.error.as_ref().unwrap().contains("project"),
            "goose skip should mention project scope"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_stdio_writes_absolute_db_and_no_key() {
        let dir = tmp();
        let b = base(&dir);
        let pool = db::open_memory().unwrap();
        let mut cfg = Config::default();
        cfg.database.path = dir.join("mydb.db");
        let mut a = args(&["opencode"], Scope::Project);
        a.stdio = true;
        a.key = None;

        let result = run(&a, &cfg, &pool, &b).unwrap();
        // stdio needs no key: no per-client key on any outcome.
        assert!(
            result.outcomes.iter().all(|o| o.key.is_none()),
            "stdio needs no key"
        );

        let written =
            std::fs::read_to_string(b.project.join("opencode.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(v["mcp"]["lific"]["type"], "local");
        let cmd = v["mcp"]["lific"]["command"].as_array().unwrap();
        // The db path is absolute.
        let db_arg = cmd[2].as_str().unwrap();
        assert!(
            std::path::Path::new(db_arg).is_absolute(),
            "stdio db path must be absolute, got {db_arg}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_dry_run_writes_nothing_but_returns_contents() {
        let dir = tmp();
        let b = base(&dir);
        let pool = db::open_memory().unwrap();
        let cfg = Config::default();
        let mut a = args(&["cursor"], Scope::Global);
        a.dry_run = true;

        let result = run(&a, &cfg, &pool, &b).unwrap();
        let oc = &result.outcomes[0];
        assert!(oc.dry_run_contents.is_some());
        // Nothing on disk.
        assert!(!b.home.join(".cursor/mcp.json").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── per-tool key minting (LIF-259) ───────────────────────

    /// Count active (unrevoked) keys with a given name.
    fn active_key_count(pool: &DbPool, name: &str) -> i64 {
        let conn = pool.read().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM api_keys WHERE name = ?1 AND revoked = 0",
            rusqlite::params![name],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn resolve_key_source_provided_is_verbatim() {
        let pool = db::open_memory().unwrap();
        let a = args(&["opencode"], Scope::Global); // provides --key
        match resolve_key_source(&a, &pool).unwrap() {
            KeySource::Provided(k) => assert_eq!(k, "lific_sk-live-TESTKEY"),
            _ => panic!("expected Provided"),
        }
    }

    #[test]
    fn provided_key_is_used_verbatim_for_all_clients() {
        let dir = tmp();
        let b = base(&dir);
        let pool = db::open_memory().unwrap();
        let cfg = Config::default();
        // Two clients, one shared --key: both get the SAME verbatim key.
        let a = args(&["opencode", "cursor"], Scope::Global);
        let result = run(&a, &cfg, &pool, &b).unwrap();
        for id in ["opencode", "cursor"] {
            let oc = result.outcomes.iter().find(|o| o.id == id).unwrap();
            assert_eq!(oc.key.as_deref(), Some("lific_sk-live-TESTKEY"));
        }
        // Nothing was minted (no bots, no keys in the DB).
        let conn = pool.read().unwrap();
        let key_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM api_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(key_count, 0, "--key mints nothing");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fresh_install_mints_per_tool_unassigned_keys() {
        let dir = tmp();
        let b = base(&dir);
        let pool = db::open_memory().unwrap(); // zero users
        let cfg = Config::default();
        let mut a = args(&["opencode", "cursor"], Scope::Global);
        a.key = None; // force minting

        let result = run(&a, &cfg, &pool, &b).unwrap();
        assert_eq!(result.key_origin, Some(KeyOrigin::Unassigned));

        let oc = result.outcomes.iter().find(|o| o.id == "opencode").unwrap();
        let cur = result.outcomes.iter().find(|o| o.id == "cursor").unwrap();
        let ock = oc.key.clone().unwrap();
        let curk = cur.key.clone().unwrap();
        assert!(ock.starts_with("lific_sk-live-"));
        assert!(curk.starts_with("lific_sk-live-"));
        assert_ne!(ock, curk, "each tool gets a distinct key");

        // Keys are named just after the tool, and unassigned (user_id NULL).
        let conn = pool.read().unwrap();
        for name in ["opencode", "cursor"] {
            let uid: Option<i64> = conn
                .query_row(
                    "SELECT user_id FROM api_keys WHERE name = ?1",
                    rusqlite::params![name],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(uid, None, "{name} key must be unassigned");
        }
        // Each config file contains its own key.
        let oc_body = std::fs::read_to_string(
            b.home.join(".config/opencode/opencode.json"),
        )
        .unwrap();
        assert!(oc_body.contains(&ock));
        let cur_body = std::fs::read_to_string(b.home.join(".cursor/mcp.json")).unwrap();
        assert!(cur_body.contains(&curk));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn single_user_mints_per_tool_bots_owned_by_them() {
        let dir = tmp();
        let b = base(&dir);
        let pool = db::open_memory().unwrap();
        let owner_id = seed_user(&pool, "solo", true);
        let cfg = Config::default();
        let mut a = args(&["opencode", "cursor"], Scope::Global);
        a.key = None;

        let result = run(&a, &cfg, &pool, &b).unwrap();
        assert_eq!(result.key_origin, Some(KeyOrigin::Bot));

        let ock = result
            .outcomes
            .iter()
            .find(|o| o.id == "opencode")
            .unwrap()
            .key
            .clone()
            .unwrap();
        let curk = result
            .outcomes
            .iter()
            .find(|o| o.id == "cursor")
            .unwrap()
            .key
            .clone()
            .unwrap();
        assert_ne!(ock, curk, "each tool gets a distinct plaintext");

        let conn = pool.read().unwrap();
        // Two bots: `opencode-solo` and `cursor-solo`, correct display names,
        // owned by the human.
        for (username, display) in [("opencode-solo", "OpenCode"), ("cursor-solo", "Cursor")] {
            let (is_bot, owner, dn): (bool, Option<i64>, String) = conn
                .query_row(
                    "SELECT is_bot, owner_id, display_name FROM users WHERE username = ?1",
                    rusqlite::params![username],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            assert!(is_bot, "{username} must be a bot");
            assert_eq!(owner, Some(owner_id), "{username} must be owned by solo");
            assert_eq!(dn, display, "{username} display name must match web UI");
        }
        // One active key per bot, each assigned to the right bot.
        assert_eq!(active_key_count(&pool, "opencode-solo"), 1);
        assert_eq!(active_key_count(&pool, "cursor-solo"), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rerun_rotates_both_tool_keys_without_error() {
        let dir = tmp();
        let b = base(&dir);
        let pool = db::open_memory().unwrap();
        seed_user(&pool, "solo", true);
        let cfg = Config::default();
        let mut a = args(&["opencode", "cursor"], Scope::Global);
        a.key = None;

        let first = run(&a, &cfg, &pool, &b).unwrap();
        let ock1 = first
            .outcomes
            .iter()
            .find(|o| o.id == "opencode")
            .unwrap()
            .key
            .clone()
            .unwrap();

        // Re-run: must succeed and rotate (fresh plaintext), still one active
        // key per bot.
        let second = run(&a, &cfg, &pool, &b).unwrap();
        let ock2 = second
            .outcomes
            .iter()
            .find(|o| o.id == "opencode")
            .unwrap()
            .key
            .clone()
            .unwrap();
        assert_ne!(ock1, ock2, "re-run must rotate the opencode key");
        assert_eq!(active_key_count(&pool, "opencode-solo"), 1);
        assert_eq!(active_key_count(&pool, "cursor-solo"), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn explicit_user_owns_the_bots() {
        let pool = db::open_memory().unwrap();
        let alice = seed_user(&pool, "alice", false);
        let _bob = seed_user(&pool, "bob", false);

        let mut a = args(&["opencode"], Scope::Global);
        a.key = None;
        a.user = Some("alice".into());

        let dir = tmp();
        let b = base(&dir);
        let cfg = Config::default();
        let result = run(&a, &cfg, &pool, &b).unwrap();
        assert_eq!(result.key_origin, Some(KeyOrigin::Bot));

        let conn = pool.read().unwrap();
        let owner: Option<i64> = conn
            .query_row(
                "SELECT owner_id FROM users WHERE username = 'opencode-alice'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(owner, Some(alice), "explicit --user must own the bot");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn multiple_users_no_user_flag_errors_with_guidance() {
        let pool = db::open_memory().unwrap();
        seed_user(&pool, "a", false);
        seed_user(&pool, "b", false);
        let mut a = args(&["opencode"], Scope::Global);
        a.key = None;
        let err = resolve_key_source(&a, &pool).unwrap_err();
        assert!(err.contains("--user"), "must guide toward --user: {err}");
    }

    // ── --oauth mode (LIF-259) ───────────────────────────────

    #[test]
    fn oauth_writes_headerless_opencode_and_mints_nothing() {
        let dir = tmp();
        let b = base(&dir);
        let pool = db::open_memory().unwrap();
        seed_user(&pool, "solo", true); // a human exists — must still not mint
        let cfg = Config::default();
        let mut a = args(&["opencode"], Scope::Global);
        a.key = None;
        a.oauth = true;

        let result = run(&a, &cfg, &pool, &b).unwrap();
        assert!(result.oauth);

        let oc = result.outcomes.iter().find(|o| o.id == "opencode").unwrap();
        assert_eq!(oc.action.as_deref(), Some("created"));
        assert!(oc.key.is_none(), "oauth writes no key");
        assert_eq!(oc.auth_hint.as_deref(), Some("opencode mcp auth lific"));

        // The written config has a url and NO headers key at all.
        let body =
            std::fs::read_to_string(b.home.join(".config/opencode/opencode.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["mcp"]["lific"]["url"], "http://127.0.0.1:3456/mcp");
        assert!(
            v["mcp"]["lific"].get("headers").is_none(),
            "oauth opencode config must have no headers key: {body}"
        );

        // Zero keys and zero bots created.
        let conn = pool.read().unwrap();
        let keys: i64 = conn
            .query_row("SELECT COUNT(*) FROM api_keys", [], |r| r.get(0))
            .unwrap();
        let bots: i64 = conn
            .query_row("SELECT COUNT(*) FROM users WHERE is_bot = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(keys, 0, "oauth mints no keys");
        assert_eq!(bots, 0, "oauth creates no bots");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn oauth_codex_toml_has_url_and_no_bearer_env_var() {
        let dir = tmp();
        let b = base(&dir);
        let pool = db::open_memory().unwrap();
        let cfg = Config::default();
        let mut a = args(&["codex"], Scope::Global);
        a.key = None;
        a.oauth = true;

        run(&a, &cfg, &pool, &b).unwrap();
        let body = std::fs::read_to_string(b.home.join(".codex/config.toml")).unwrap();
        let doc: toml_edit::DocumentMut = body.parse().unwrap();
        assert_eq!(
            doc["mcp_servers"]["lific"]["url"].as_str(),
            Some("http://127.0.0.1:3456/mcp")
        );
        assert!(
            doc["mcp_servers"]["lific"].get("bearer_token_env_var").is_none(),
            "oauth codex must not set bearer_token_env_var: {body}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn oauth_skips_incapable_clients_with_note() {
        let dir = tmp();
        let b = base(&dir);
        let pool = db::open_memory().unwrap();
        let cfg = Config::default();
        let mut a = args(&["opencode", "goose", "claude-desktop"], Scope::Global);
        a.key = None;
        a.oauth = true;

        let result = run(&a, &cfg, &pool, &b).unwrap();
        for id in ["goose", "claude-desktop"] {
            let oc = result.outcomes.iter().find(|o| o.id == id).unwrap();
            assert!(oc.action.is_none(), "{id} must be skipped, not written");
            assert!(
                oc.error.as_ref().unwrap().contains("does not support --oauth"),
                "{id} skip must explain why"
            );
            assert!(!oc.notes.is_empty(), "{id} must carry an explanatory note");
        }
        // opencode still went through.
        let oc = result.outcomes.iter().find(|o| o.id == "opencode").unwrap();
        assert_eq!(oc.action.as_deref(), Some("created"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn oauth_and_stdio_conflict_errors() {
        let dir = tmp();
        let b = base(&dir);
        let pool = db::open_memory().unwrap();
        let cfg = Config::default();
        let mut a = args(&["opencode"], Scope::Global);
        a.key = None;
        a.oauth = true;
        a.stdio = true;
        let err = run(&a, &cfg, &pool, &b).unwrap_err();
        assert!(err.contains("--oauth") && err.contains("--stdio"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn oauth_and_key_conflict_errors() {
        let dir = tmp();
        let b = base(&dir);
        let pool = db::open_memory().unwrap();
        let cfg = Config::default();
        let mut a = args(&["opencode"], Scope::Global); // provides --key
        a.oauth = true;
        let err = run(&a, &cfg, &pool, &b).unwrap_err();
        assert!(err.contains("--oauth") && err.contains("--key"));
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── AGENTS.md integration ────────────────────────────────

    #[test]
    fn run_writes_agents_md_in_project_scope_when_yes() {
        let dir = tmp();
        let b = base(&dir);
        std::fs::create_dir_all(&b.project).unwrap();
        let pool = db::open_memory().unwrap();
        let cfg = Config::default();
        let mut a = args(&["opencode"], Scope::Project);
        a.skip_agents = false; // allow it

        let result = run(&a, &cfg, &pool, &b).unwrap();
        assert!(result.agents_md.is_some());
        assert!(b.project.join("AGENTS.md").exists());
        let content = std::fs::read_to_string(b.project.join("AGENTS.md")).unwrap();
        assert!(content.contains("lific:begin"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn run_skip_agents_writes_no_agents_md() {
        let dir = tmp();
        let b = base(&dir);
        std::fs::create_dir_all(&b.project).unwrap();
        let pool = db::open_memory().unwrap();
        let cfg = Config::default();
        let a = args(&["opencode"], Scope::Project); // skip_agents = true
        let result = run(&a, &cfg, &pool, &b).unwrap();
        assert!(result.agents_md.is_none());
        assert!(!b.project.join("AGENTS.md").exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
