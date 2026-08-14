//! The client matrix for `lific connect`.
//!
//! One canonical [`ServerConfig`] describes the Lific MCP server (name, remote
//! or stdio transport). A table of [`ClientSpec`] entries "compiles" that
//! canonical config into each client's native on-disk schema. Adding support
//! for a new client is exactly one table row: its id, display name, the
//! global/project config paths (computed from an injected base so tests never
//! touch the real `$HOME`), a file format, and a mapper function that turns the
//! `ServerConfig` into the JSON/TOML/YAML value that client expects.
//!
//! Schemas are verified against each client's official docs (July 2026). The
//! per-client gotchas — `servers` vs `mcpServers`, `httpUrl`/`serverUrl`/`url`,
//! Codex's env-var token, Goose's YAML `extensions` — all live here so the
//! writer and orchestration layers stay format-agnostic.

use std::path::PathBuf;

/// The transport half of the canonical server config.
#[derive(Debug, Clone)]
pub enum Transport {
    /// Remote streamable-HTTP MCP server reached over the network.
    Remote {
        /// The MCP endpoint URL (e.g. `http://127.0.0.1:3456/mcp`).
        url: String,
        /// The bearer API key presented in the `Authorization` header.
        key: String,
    },
    /// Local stdio server: the client spawns `lific --db <db> mcp` itself.
    Stdio {
        /// Absolute path to the SQLite database the spawned server should open.
        db_path: String,
        /// The agent credential carried by this stdio session (LIFIC-18). Written
        /// into the client config's env field as `LIFIC_TOKEN` so the spawned
        /// server resolves the caller as the bound agent. `None` for a plain
        /// (operator) stdio config with no agent identity.
        token: Option<String>,
    },
    /// Remote streamable-HTTP server reached over the network, written WITHOUT
    /// any `Authorization` header — the client's native MCP OAuth flow (DCR +
    /// authorization-code) takes over and obtains its own token. No key is
    /// minted for this transport (LIF-259 `--oauth`).
    OauthRemote {
        /// The MCP endpoint URL (e.g. `http://127.0.0.1:3456/mcp`).
        url: String,
    },
}

/// The canonical, client-agnostic description of the Lific MCP server. Every
/// client mapper consumes exactly this.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// The server key under the client's top-level map. Always `"lific"`.
    pub name: String,
    pub transport: Transport,
}

impl ServerConfig {
    pub fn remote(url: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            name: "lific".into(),
            transport: Transport::Remote {
                url: url.into(),
                key: key.into(),
            },
        }
    }

    pub fn stdio(db_path: impl Into<String>) -> Self {
        Self {
            name: "lific".into(),
            transport: Transport::Stdio {
                db_path: db_path.into(),
                token: None,
            },
        }
    }

    /// A stdio server carrying an agent credential (LIFIC-18). The token is
    /// written into the client config's env field as `LIFIC_TOKEN`.
    pub fn stdio_with_token(db_path: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            name: "lific".into(),
            transport: Transport::Stdio {
                db_path: db_path.into(),
                token: Some(token.into()),
            },
        }
    }

    /// A remote server whose config carries no key — the client authenticates
    /// via its native MCP OAuth flow (LIF-259 `--oauth`).
    pub fn oauth_remote(url: impl Into<String>) -> Self {
        Self {
            name: "lific".into(),
            transport: Transport::OauthRemote { url: url.into() },
        }
    }
}

/// Whether a client can drive Lific's MCP OAuth flow from a config file that
/// carries only a URL (no `Authorization` header), and — when it can — the
/// post-connect command that kicks off/completes that flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OauthSupport {
    /// The client completes OAuth (DCR + auth-code) itself once it sees a
    /// header-less remote entry. `hint` is the command/instruction to surface.
    Capable { hint: &'static str },
    /// The client cannot OAuth from the config-file form we write. `reason` is
    /// shown to the user so the skip isn't silent.
    Unsupported { reason: &'static str },
}

/// On-disk file format for a client's config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Json,
    Toml,
    Yaml,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Json => "json",
            Format::Toml => "toml",
            Format::Yaml => "yaml",
        }
    }
}

/// The scope a config write targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The user-global config in the home directory.
    Global,
    /// The per-project config relative to the project root (cwd).
    Project,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Global => "global",
            Scope::Project => "project",
        }
    }
}

/// Bases from which every config path is computed. Injected so tests can point
/// at temp dirs and never read or write the real user home / cwd. Production
/// fills these from `dirs::home_dir()` and the current working directory.
#[derive(Debug, Clone)]
pub struct PathBase {
    pub home: PathBuf,
    pub project: PathBuf,
    /// The platform whose path conventions apply. Lets tests exercise the
    /// macOS `Library/Application Support` and Windows `%APPDATA%` branches on
    /// a Linux CI box. Production passes the real host OS.
    pub os: Os,
    /// Value of `%APPDATA%` on Windows (ignored on other OSes). Injected so the
    /// Windows branch is testable without a real Windows environment.
    pub appdata: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    Mac,
    Windows,
}

impl Os {
    /// The current host OS, mapped to our three-way enum. Anything that isn't
    /// macOS or Windows is treated as Linux (the XDG-style default), which is
    /// correct for the BSDs and other Unixes Lific might run on.
    pub fn host() -> Os {
        match std::env::consts::OS {
            "macos" => Os::Mac,
            "windows" => Os::Windows,
            _ => Os::Linux,
        }
    }
}

/// How a client's mapper emitted the config, plus where in the file it lives.
///
/// The `top_key` is the map the server entry lives under (e.g. `mcpServers`,
/// `servers`, `mcp`, `context_servers`, `extensions`). For Codex the value is
/// the whole `[mcp_servers.lific]` sub-table addressed by a dotted key.
#[derive(Default)]
pub struct CompiledEntry {
    /// The server name this entry is keyed under within `top_key` (always
    /// `"lific"`, taken from the canonical [`ServerConfig::name`]).
    pub name: String,
    /// The top-level object key the server entry is nested under. For JSON/YAML
    /// this is a single key. For Codex TOML it's the dotted table path
    /// `mcp_servers.lific` handled specially by the TOML writer.
    pub top_key: String,
    /// The server entry value (already keyed by name under `top_key` by the
    /// writer, so this is just the inner object for that one server).
    pub value: serde_json::Value,
    /// Human-readable notes to surface after writing (e.g. Codex's env-var
    /// export hint, or "remote via mcp-remote shim"). Empty when nothing to say.
    pub notes: Vec<String>,
}

/// A single client in the connection matrix.
pub struct ClientSpec {
    /// Stable identifier used on the CLI (`--client <id>`).
    pub id: &'static str,
    /// Human-facing display name. Matches the web UI's Connected Tools display
    /// map where the tool overlaps (LIF-259) so a CLI-connected bot and a
    /// web-connected bot are indistinguishable on that page.
    pub display: &'static str,
    /// Whether this client can drive Lific's MCP OAuth flow from a header-less
    /// config, and the post-connect auth hint (LIF-259 `--oauth`).
    pub oauth: OauthSupport,
    pub format: Format,
    /// The env-field name this client uses for a stdio command's environment
    /// (LIFIC-18): `environment` for OpenCode, `env` for Claude Code and Codex,
    /// per those tools' config schemas. `None` when the client's stdio entry
    /// cannot carry an env map (the token is then simply not written).
    pub stdio_env_key: Option<&'static str>,
    /// Compute the global-scope config path for this OS, or `None` if the
    /// client has no global config (rare).
    global_path: fn(&PathBase) -> Option<PathBuf>,
    /// Compute the project-scope config path, or `None` if the client has no
    /// project-scoped config (e.g. Claude Desktop, Zed, Windsurf, Goose).
    project_path: fn(&PathBase) -> Option<PathBuf>,
    /// Compile the canonical config into this client's native schema.
    compile: fn(&ServerConfig) -> CompiledEntry,
    /// Extra filesystem hints that, when present, indicate this client is
    /// installed (used for detection alongside the config path itself).
    detect_extra: fn(&PathBase, Scope) -> Vec<PathBuf>,
}

impl ClientSpec {
    pub fn path_for(&self, base: &PathBase, scope: Scope) -> Option<PathBuf> {
        match scope {
            Scope::Global => (self.global_path)(base),
            Scope::Project => (self.project_path)(base),
        }
    }

    pub fn compile(&self, cfg: &ServerConfig) -> CompiledEntry {
        let mut entry = (self.compile)(cfg);
        // The mappers don't set `name` themselves; inject the canonical server
        // name here so there's one source of truth (always `"lific"`).
        entry.name = cfg.name.clone();
        // LIFIC-18: for a stdio transport carrying an agent token, write the
        // token into the client's env field as LIFIC_TOKEN so the spawned
        // server resolves the caller as the bound agent. Clients that can't
        // carry an env map get the token silently dropped (i.e. write nothing).
        if let (Transport::Stdio { token: Some(tok), .. }, Some(env_key)) =
            (&cfg.transport, self.stdio_env_key)
        {
            entry.value[env_key] = serde_json::json!({ "LIFIC_TOKEN": tok });
        }
        entry
    }

    /// Whether this client appears installed for the given scope: its config
    /// file exists, or one of its extra marker paths exists.
    pub fn detected(&self, base: &PathBase, scope: Scope) -> bool {
        if let Some(p) = self.path_for(base, scope)
            && p.exists()
        {
            return true;
        }
        (self.detect_extra)(base, scope).iter().any(|p| p.exists())
    }
}

// ── Path helpers ─────────────────────────────────────────────

/// `~/.config/<rest>` on Linux/macOS; `%APPDATA%\<rest>` on Windows.
fn config_dir(base: &PathBase, rest: &[&str]) -> PathBuf {
    match base.os {
        Os::Windows => {
            let mut p = base
                .appdata
                .clone()
                .unwrap_or_else(|| base.home.join("AppData").join("Roaming"));
            for r in rest {
                p = p.join(r);
            }
            p
        }
        _ => {
            let mut p = base.home.join(".config");
            for r in rest {
                p = p.join(r);
            }
            p
        }
    }
}

/// `~/<rest>` on every OS (dotfile in the home dir).
fn home_dot(base: &PathBase, rest: &[&str]) -> PathBuf {
    let mut p = base.home.clone();
    for r in rest {
        p = p.join(r);
    }
    p
}

/// `<project>/<rest>`.
fn project_rel(base: &PathBase, rest: &[&str]) -> PathBuf {
    let mut p = base.project.clone();
    for r in rest {
        p = p.join(r);
    }
    p
}

fn no_extra(_: &PathBase, _: Scope) -> Vec<PathBuf> {
    Vec::new()
}

// ── Schema builders ──────────────────────────────────────────

fn bearer(key: &str) -> String {
    format!("Bearer {key}")
}

fn headers_obj(key: &str) -> serde_json::Value {
    serde_json::json!({ "Authorization": bearer(key) })
}

/// The stdio command args shared by clients that take a `command`+`args` pair:
/// `["--db", <db>, "mcp"]`.
fn stdio_args(db_path: &str) -> serde_json::Value {
    serde_json::json!(["--db", db_path, "mcp"])
}

// ── The matrix ───────────────────────────────────────────────

/// The full client matrix. Order is the display order in interactive mode.
pub fn all_clients() -> Vec<ClientSpec> {
    vec![
        // ── opencode ──────────────────────────────────────────
        ClientSpec {
            id: "opencode",
            display: "OpenCode",
            oauth: OauthSupport::Capable {
                hint: "opencode mcp auth lific",
            },
            format: Format::Json,
            stdio_env_key: Some("environment"),
            global_path: |b| Some(config_dir(b, &["opencode", "opencode.json"])),
            project_path: |b| Some(project_rel(b, &["opencode.json"])),
            detect_extra: no_extra,
            compile: |c| match &c.transport {
                Transport::Remote { url, key } => CompiledEntry {
                    name: String::new(),
                    top_key: "mcp".into(),
                    value: serde_json::json!({
                        "type": "remote",
                        "url": url,
                        "headers": headers_obj(key),
                        "enabled": true,
                    }),
                    notes: vec![],
                },
                // OAuth: a bare remote entry with no headers. OpenCode's
                // automatic MCP OAuth detection kicks in (verified: it completes
                // discovery + DCR against Lific with just this).
                Transport::OauthRemote { url } => CompiledEntry {
                    name: String::new(),
                    top_key: "mcp".into(),
                    value: serde_json::json!({
                        "type": "remote",
                        "url": url,
                        "enabled": true,
                    }),
                    notes: vec![],
                },
                Transport::Stdio { db_path, .. } => CompiledEntry {
                    name: String::new(),
                    top_key: "mcp".into(),
                    value: serde_json::json!({
                        "type": "local",
                        "command": ["lific", "--db", db_path, "mcp"],
                        "enabled": true,
                    }),
                    notes: vec![],
                },
            },
        },
        // ── claude-code ───────────────────────────────────────
        ClientSpec {
            id: "claude-code",
            display: "Claude Code",
            oauth: OauthSupport::Capable {
                hint: "claude mcp login lific  (or /mcp inside a session)",
            },
            format: Format::Json,
            stdio_env_key: Some("env"),
            global_path: |b| Some(home_dot(b, &[".claude.json"])),
            project_path: |b| Some(project_rel(b, &[".mcp.json"])),
            detect_extra: no_extra,
            compile: |c| match &c.transport {
                Transport::Remote { url, key } => CompiledEntry {
                    name: String::new(),
                    top_key: "mcpServers".into(),
                    value: serde_json::json!({
                        "type": "http",
                        "url": url,
                        "headers": headers_obj(key),
                    }),
                    notes: vec![],
                },
                Transport::OauthRemote { url } => CompiledEntry {
                    name: String::new(),
                    top_key: "mcpServers".into(),
                    value: serde_json::json!({
                        "type": "http",
                        "url": url,
                    }),
                    notes: vec![],
                },
                Transport::Stdio { db_path, .. } => CompiledEntry {
                    name: String::new(),
                    top_key: "mcpServers".into(),
                    value: serde_json::json!({
                        "type": "stdio",
                        "command": "lific",
                        "args": stdio_args(db_path),
                    }),
                    notes: vec![],
                },
            },
        },
        // ── claude-desktop (global only) ──────────────────────
        ClientSpec {
            id: "claude-desktop",
            display: "Claude Desktop",
            // Its config-file form uses the mcp-remote npx shim; the native
            // OAuth path is the Custom Connectors UI, not a header-less config.
            oauth: OauthSupport::Unsupported {
                reason: "Claude Desktop's OAuth path is the Settings > Connectors UI, not a \
                         config-file entry — add Lific there instead.",
            },
            format: Format::Json,
            stdio_env_key: None,
            global_path: |b| {
                Some(match b.os {
                    Os::Mac => home_dot(
                        b,
                        &[
                            "Library",
                            "Application Support",
                            "Claude",
                            "claude_desktop_config.json",
                        ],
                    ),
                    Os::Windows => {
                        config_dir(b, &["Claude", "claude_desktop_config.json"])
                    }
                    Os::Linux => config_dir(b, &["Claude", "claude_desktop_config.json"]),
                })
            },
            project_path: |_| None,
            detect_extra: no_extra,
            compile: |c| match &c.transport {
                Transport::Remote { url, key } => CompiledEntry {
                    name: String::new(),
                    top_key: "mcpServers".into(),
                    // Claude Desktop has no native remote MCP support: bridge it
                    // through the mcp-remote npx shim, passing the bearer header.
                    value: serde_json::json!({
                        "command": "npx",
                        "args": [
                            "-y",
                            "mcp-remote",
                            url,
                            "--header",
                            format!("Authorization: {}", bearer(key)),
                        ],
                    }),
                    notes: vec![
                        "Claude Desktop reaches the remote server via the mcp-remote npx shim; \
                         npx installs it on first run."
                            .into(),
                    ],
                },
                // Unreachable in `--oauth` mode (claude-desktop is skipped as
                // OAuth-incapable before compile), but the match must be total:
                // fall back to the header-less mcp-remote shim.
                Transport::OauthRemote { url } => CompiledEntry {
                    name: String::new(),
                    top_key: "mcpServers".into(),
                    value: serde_json::json!({
                        "command": "npx",
                        "args": ["-y", "mcp-remote", url],
                    }),
                    notes: vec![],
                },
                Transport::Stdio { db_path, .. } => CompiledEntry {
                    name: String::new(),
                    top_key: "mcpServers".into(),
                    value: serde_json::json!({
                        "command": "lific",
                        "args": stdio_args(db_path),
                    }),
                    notes: vec![],
                },
            },
        },
        // ── cursor ────────────────────────────────────────────
        ClientSpec {
            id: "cursor",
            display: "Cursor",
            oauth: OauthSupport::Capable {
                hint: "Cursor will prompt to authorize in-app on first connect",
            },
            format: Format::Json,
            stdio_env_key: None,
            global_path: |b| Some(home_dot(b, &[".cursor", "mcp.json"])),
            project_path: |b| Some(project_rel(b, &[".cursor", "mcp.json"])),
            detect_extra: |b, scope| match scope {
                Scope::Global => vec![home_dot(b, &[".cursor"])],
                Scope::Project => vec![project_rel(b, &[".cursor"])],
            },
            compile: |c| match &c.transport {
                Transport::Remote { url, key } => CompiledEntry {
                    name: String::new(),
                    top_key: "mcpServers".into(),
                    value: serde_json::json!({
                        "url": url,
                        "headers": headers_obj(key),
                    }),
                    notes: vec![],
                },
                Transport::OauthRemote { url } => CompiledEntry {
                    name: String::new(),
                    top_key: "mcpServers".into(),
                    value: serde_json::json!({
                        "url": url,
                    }),
                    notes: vec![],
                },
                Transport::Stdio { db_path, .. } => CompiledEntry {
                    name: String::new(),
                    top_key: "mcpServers".into(),
                    value: serde_json::json!({
                        "command": "lific",
                        "args": stdio_args(db_path),
                    }),
                    notes: vec![],
                },
            },
        },
        // ── vscode ────────────────────────────────────────────
        ClientSpec {
            id: "vscode",
            display: "VS Code",
            oauth: OauthSupport::Capable {
                hint: "VS Code starts the browser OAuth flow on first connect",
            },
            format: Format::Json,
            stdio_env_key: None,
            global_path: |b| {
                Some(match b.os {
                    Os::Mac => home_dot(
                        b,
                        &["Library", "Application Support", "Code", "User", "mcp.json"],
                    ),
                    Os::Windows => config_dir(b, &["Code", "User", "mcp.json"]),
                    Os::Linux => config_dir(b, &["Code", "User", "mcp.json"]),
                })
            },
            project_path: |b| Some(project_rel(b, &[".vscode", "mcp.json"])),
            detect_extra: |b, scope| match scope {
                Scope::Project => vec![project_rel(b, &[".vscode"])],
                Scope::Global => vec![],
            },
            compile: |c| match &c.transport {
                // VS Code's top-level key is `servers`, NOT `mcpServers`.
                Transport::Remote { url, key } => CompiledEntry {
                    name: String::new(),
                    top_key: "servers".into(),
                    value: serde_json::json!({
                        "type": "http",
                        "url": url,
                        "headers": headers_obj(key),
                    }),
                    notes: vec![],
                },
                Transport::OauthRemote { url } => CompiledEntry {
                    name: String::new(),
                    top_key: "servers".into(),
                    value: serde_json::json!({
                        "type": "http",
                        "url": url,
                    }),
                    notes: vec![],
                },
                Transport::Stdio { db_path, .. } => CompiledEntry {
                    name: String::new(),
                    top_key: "servers".into(),
                    value: serde_json::json!({
                        "type": "stdio",
                        "command": "lific",
                        "args": stdio_args(db_path),
                    }),
                    notes: vec![],
                },
            },
        },
        // ── codex (TOML) ──────────────────────────────────────
        ClientSpec {
            id: "codex",
            display: "Codex",
            oauth: OauthSupport::Capable {
                hint: "codex mcp login lific",
            },
            format: Format::Toml,
            stdio_env_key: Some("env"),
            global_path: |b| Some(home_dot(b, &[".codex", "config.toml"])),
            project_path: |b| Some(project_rel(b, &[".codex", "config.toml"])),
            detect_extra: |b, scope| match scope {
                Scope::Global => vec![home_dot(b, &[".codex"])],
                Scope::Project => vec![project_rel(b, &[".codex"])],
            },
            compile: |c| match &c.transport {
                // Codex reads the bearer token from an env var, never inline.
                Transport::Remote { url, .. } => CompiledEntry {
                    name: String::new(),
                    top_key: "mcp_servers.lific".into(),
                    value: serde_json::json!({
                        "url": url,
                        "bearer_token_env_var": "LIFIC_API_KEY",
                    }),
                    notes: vec![
                        "Codex reads the key from the LIFIC_API_KEY environment variable — \
                         export it before launching Codex (see the hint below)."
                            .into(),
                    ],
                },
                // OAuth: url only — no bearer_token_env_var, so Codex runs its
                // own login flow rather than reading a key from the env.
                Transport::OauthRemote { url } => CompiledEntry {
                    name: String::new(),
                    top_key: "mcp_servers.lific".into(),
                    value: serde_json::json!({
                        "url": url,
                    }),
                    notes: vec![],
                },
                Transport::Stdio { db_path, .. } => CompiledEntry {
                    name: String::new(),
                    top_key: "mcp_servers.lific".into(),
                    value: serde_json::json!({
                        "command": "lific",
                        "args": stdio_args(db_path),
                    }),
                    notes: vec![],
                },
            },
        },
        // ── zed (global only) ─────────────────────────────────
        ClientSpec {
            id: "zed",
            display: "Zed",
            oauth: OauthSupport::Capable {
                hint: "Zed runs the OAuth flow automatically when no header is set",
            },
            format: Format::Json,
            stdio_env_key: None,
            global_path: |b| Some(config_dir(b, &["zed", "settings.json"])),
            project_path: |_| None,
            detect_extra: |b, scope| match scope {
                Scope::Global => vec![config_dir(b, &["zed"])],
                Scope::Project => vec![],
            },
            compile: |c| match &c.transport {
                Transport::Remote { url, key } => CompiledEntry {
                    name: String::new(),
                    top_key: "context_servers".into(),
                    value: serde_json::json!({
                        "url": url,
                        "headers": headers_obj(key),
                    }),
                    notes: vec![],
                },
                Transport::OauthRemote { url } => CompiledEntry {
                    name: String::new(),
                    top_key: "context_servers".into(),
                    value: serde_json::json!({
                        "url": url,
                    }),
                    notes: vec![],
                },
                Transport::Stdio { db_path, .. } => CompiledEntry {
                    name: String::new(),
                    top_key: "context_servers".into(),
                    value: serde_json::json!({
                        "command": "lific",
                        "args": stdio_args(db_path),
                    }),
                    notes: vec![],
                },
            },
        },
        // ── gemini ────────────────────────────────────────────
        ClientSpec {
            id: "gemini",
            display: "Gemini CLI",
            oauth: OauthSupport::Capable {
                hint: "run /mcp auth lific inside Gemini CLI",
            },
            format: Format::Json,
            stdio_env_key: None,
            global_path: |b| Some(home_dot(b, &[".gemini", "settings.json"])),
            project_path: |b| Some(project_rel(b, &[".gemini", "settings.json"])),
            detect_extra: |b, scope| match scope {
                Scope::Global => vec![home_dot(b, &[".gemini"])],
                Scope::Project => vec![project_rel(b, &[".gemini"])],
            },
            compile: |c| match &c.transport {
                // Gemini's remote streamable-HTTP field is `httpUrl`, NOT `url`.
                Transport::Remote { url, key } => CompiledEntry {
                    name: String::new(),
                    top_key: "mcpServers".into(),
                    value: serde_json::json!({
                        "httpUrl": url,
                        "headers": headers_obj(key),
                    }),
                    notes: vec![],
                },
                Transport::OauthRemote { url } => CompiledEntry {
                    name: String::new(),
                    top_key: "mcpServers".into(),
                    value: serde_json::json!({
                        "httpUrl": url,
                    }),
                    notes: vec![],
                },
                Transport::Stdio { db_path, .. } => CompiledEntry {
                    name: String::new(),
                    top_key: "mcpServers".into(),
                    value: serde_json::json!({
                        "command": "lific",
                        "args": stdio_args(db_path),
                    }),
                    notes: vec![],
                },
            },
        },
        // ── windsurf (global only) ────────────────────────────
        ClientSpec {
            id: "windsurf",
            display: "Windsurf",
            oauth: OauthSupport::Capable {
                hint: "Windsurf prompts to authorize in-app on first connect",
            },
            format: Format::Json,
            stdio_env_key: None,
            global_path: |b| Some(home_dot(b, &[".codeium", "windsurf", "mcp_config.json"])),
            project_path: |_| None,
            detect_extra: |b, scope| match scope {
                Scope::Global => vec![home_dot(b, &[".codeium", "windsurf"])],
                Scope::Project => vec![],
            },
            compile: |c| match &c.transport {
                // Windsurf's remote field is `serverUrl`, NOT `url`.
                Transport::Remote { url, key } => CompiledEntry {
                    name: String::new(),
                    top_key: "mcpServers".into(),
                    value: serde_json::json!({
                        "serverUrl": url,
                        "headers": headers_obj(key),
                    }),
                    notes: vec![],
                },
                Transport::OauthRemote { url } => CompiledEntry {
                    name: String::new(),
                    top_key: "mcpServers".into(),
                    value: serde_json::json!({
                        "serverUrl": url,
                    }),
                    notes: vec![],
                },
                Transport::Stdio { db_path, .. } => CompiledEntry {
                    name: String::new(),
                    top_key: "mcpServers".into(),
                    value: serde_json::json!({
                        "command": "lific",
                        "args": stdio_args(db_path),
                    }),
                    notes: vec![],
                },
            },
        },
        // ── goose (YAML, global only) ─────────────────────────
        ClientSpec {
            id: "goose",
            display: "Goose",
            // Unconfirmed OAuth support — don't write a header-less config we
            // can't vouch for; steer the user to the default key mode.
            oauth: OauthSupport::Unsupported {
                reason: "Goose's MCP OAuth support is unconfirmed — use the default key mode \
                         (drop --oauth) to connect it.",
            },
            format: Format::Yaml,
            stdio_env_key: None,
            global_path: |b| Some(config_dir(b, &["goose", "config.yaml"])),
            project_path: |_| None,
            detect_extra: |b, scope| match scope {
                Scope::Global => vec![config_dir(b, &["goose"])],
                Scope::Project => vec![],
            },
            compile: |c| match &c.transport {
                Transport::Remote { url, key } => CompiledEntry {
                    name: String::new(),
                    top_key: "extensions".into(),
                    value: serde_json::json!({
                        "name": "lific",
                        "type": "streamable_http",
                        "uri": url,
                        "headers": headers_obj(key),
                        "enabled": true,
                        "timeout": 300,
                    }),
                    notes: vec![
                        "Goose config is YAML; existing comments in config.yaml are not preserved \
                         across this edit."
                            .into(),
                    ],
                },
                // Unreachable in `--oauth` mode (goose is skipped as
                // OAuth-incapable before compile), but the match must be total.
                Transport::OauthRemote { url } => CompiledEntry {
                    name: String::new(),
                    top_key: "extensions".into(),
                    value: serde_json::json!({
                        "name": "lific",
                        "type": "streamable_http",
                        "uri": url,
                        "enabled": true,
                        "timeout": 300,
                    }),
                    notes: vec![],
                },
                Transport::Stdio { db_path, .. } => CompiledEntry {
                    name: String::new(),
                    top_key: "extensions".into(),
                    value: serde_json::json!({
                        "name": "lific",
                        "type": "stdio",
                        "cmd": "lific",
                        "args": stdio_args(db_path),
                        "enabled": true,
                        "timeout": 300,
                    }),
                    notes: vec![
                        "Goose config is YAML; existing comments in config.yaml are not preserved \
                         across this edit."
                            .into(),
                    ],
                },
            },
        },
        // ── crush ─────────────────────────────────────────────
        ClientSpec {
            id: "crush",
            display: "Crush",
            oauth: OauthSupport::Unsupported {
                reason: "Crush's MCP OAuth support is unconfirmed — use the default key mode \
                         (drop --oauth) to connect it.",
            },
            format: Format::Json,
            stdio_env_key: None,
            global_path: |b| Some(config_dir(b, &["crush", "crush.json"])),
            project_path: |b| Some(project_rel(b, &["crush.json"])),
            detect_extra: |b, scope| match scope {
                Scope::Global => vec![config_dir(b, &["crush"])],
                Scope::Project => vec![],
            },
            compile: |c| match &c.transport {
                Transport::Remote { url, key } => CompiledEntry {
                    name: String::new(),
                    top_key: "mcp".into(),
                    value: serde_json::json!({
                        "type": "http",
                        "url": url,
                        "headers": headers_obj(key),
                    }),
                    notes: vec![],
                },
                // Unreachable in `--oauth` mode (crush is skipped as
                // OAuth-incapable before compile), but the match must be total.
                Transport::OauthRemote { url } => CompiledEntry {
                    name: String::new(),
                    top_key: "mcp".into(),
                    value: serde_json::json!({
                        "type": "http",
                        "url": url,
                    }),
                    notes: vec![],
                },
                Transport::Stdio { db_path, .. } => CompiledEntry {
                    name: String::new(),
                    top_key: "mcp".into(),
                    value: serde_json::json!({
                        "type": "stdio",
                        "command": "lific",
                        "args": stdio_args(db_path),
                    }),
                    notes: vec![],
                },
            },
        },
    ]
}

/// Look up a client spec by id.
pub fn find_client(id: &str) -> Option<ClientSpec> {
    all_clients().into_iter().find(|c| c.id == id)
}

/// All client ids, for CLI help and validation error messages.
pub fn all_client_ids() -> Vec<&'static str> {
    all_clients().iter().map(|c| c.id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linux_base() -> PathBase {
        PathBase {
            home: PathBuf::from("/home/tester"),
            project: PathBuf::from("/proj"),
            os: Os::Linux,
            appdata: None,
        }
    }

    fn remote_cfg() -> ServerConfig {
        ServerConfig::remote("http://127.0.0.1:3456/mcp", "lific_sk-live-KEY")
    }

    fn stdio_cfg() -> ServerConfig {
        ServerConfig::stdio("/abs/lific.db")
    }

    fn stdio_token_cfg() -> ServerConfig {
        ServerConfig::stdio_with_token("/abs/lific.db", "lific_sk-live-AGENTTOKEN")
    }

    fn oauth_cfg() -> ServerConfig {
        ServerConfig::oauth_remote("http://127.0.0.1:3456/mcp")
    }

    #[test]
    fn opencode_oauth_has_url_but_no_headers() {
        let e = find_client("opencode").unwrap().compile(&oauth_cfg());
        assert_eq!(e.value["url"], "http://127.0.0.1:3456/mcp");
        assert!(
            e.value.get("headers").is_none(),
            "oauth opencode entry must have no headers"
        );
        assert_eq!(e.value["type"], "remote");
    }

    #[test]
    fn codex_oauth_has_url_and_no_bearer_env_var() {
        let e = find_client("codex").unwrap().compile(&oauth_cfg());
        assert_eq!(e.value["url"], "http://127.0.0.1:3456/mcp");
        assert!(
            e.value.get("bearer_token_env_var").is_none(),
            "oauth codex entry must not set bearer_token_env_var"
        );
    }

    #[test]
    fn oauth_capable_clients_have_hints_incapable_have_reasons() {
        let capable = [
            "opencode",
            "claude-code",
            "cursor",
            "vscode",
            "codex",
            "zed",
            "gemini",
            "windsurf",
        ];
        for id in capable {
            match find_client(id).unwrap().oauth {
                OauthSupport::Capable { hint } => assert!(!hint.is_empty(), "{id} needs a hint"),
                OauthSupport::Unsupported { .. } => panic!("{id} should be OAuth-capable"),
            }
        }
        for id in ["claude-desktop", "goose", "crush"] {
            match find_client(id).unwrap().oauth {
                OauthSupport::Unsupported { reason } => {
                    assert!(!reason.is_empty(), "{id} needs a reason")
                }
                OauthSupport::Capable { .. } => panic!("{id} should not be OAuth-capable"),
            }
        }
    }

    #[test]
    fn every_client_has_a_global_path_or_documents_why_not() {
        let base = linux_base();
        for c in all_clients() {
            // Every client in our matrix has a global path.
            assert!(
                c.path_for(&base, Scope::Global).is_some(),
                "{} must have a global path",
                c.id
            );
        }
    }

    #[test]
    fn opencode_project_path_is_bare_opencode_json() {
        let base = linux_base();
        let oc = find_client("opencode").unwrap();
        assert_eq!(
            oc.path_for(&base, Scope::Project).unwrap(),
            PathBuf::from("/proj/opencode.json")
        );
        assert_eq!(
            oc.path_for(&base, Scope::Global).unwrap(),
            PathBuf::from("/home/tester/.config/opencode/opencode.json")
        );
    }

    #[test]
    fn opencode_remote_uses_type_remote_and_headers() {
        let e = find_client("opencode").unwrap().compile(&remote_cfg());
        assert_eq!(e.top_key, "mcp");
        assert_eq!(e.value["type"], "remote");
        assert_eq!(e.value["url"], "http://127.0.0.1:3456/mcp");
        assert_eq!(e.value["headers"]["Authorization"], "Bearer lific_sk-live-KEY");
        assert_eq!(e.value["enabled"], true);
    }

    #[test]
    fn opencode_stdio_uses_type_local_and_command_array() {
        let e = find_client("opencode").unwrap().compile(&stdio_cfg());
        assert_eq!(e.value["type"], "local");
        assert_eq!(
            e.value["command"],
            serde_json::json!(["lific", "--db", "/abs/lific.db", "mcp"])
        );
    }

    // ── LIFIC-18: stdio agent token → env field ────────────────────────────

    #[test]
    fn opencode_stdio_token_writes_into_environment_field() {
        let e = find_client("opencode")
            .unwrap()
            .compile(&stdio_token_cfg());
        assert_eq!(e.value["type"], "local");
        // Command stays unchanged.
        assert_eq!(
            e.value["command"],
            serde_json::json!(["lific", "--db", "/abs/lific.db", "mcp"])
        );
        // opencode names its env field `environment`.
        assert_eq!(e.value["environment"]["LIFIC_TOKEN"], "lific_sk-live-AGENTTOKEN");
    }

    #[test]
    fn claude_code_stdio_token_writes_into_env_field() {
        let e = find_client("claude-code")
            .unwrap()
            .compile(&stdio_token_cfg());
        assert_eq!(e.value["type"], "stdio");
        assert_eq!(e.value["env"]["LIFIC_TOKEN"], "lific_sk-live-AGENTTOKEN");
    }

    #[test]
    fn codex_stdio_token_writes_into_env_field() {
        let e = find_client("codex").unwrap().compile(&stdio_token_cfg());
        assert_eq!(e.value["env"]["LIFIC_TOKEN"], "lific_sk-live-AGENTTOKEN");
    }

    #[test]
    fn stdio_without_token_writes_no_env_entry() {
        // A plain stdio config (operator, no agent) must not invent an env map.
        for id in ["opencode", "claude-code", "codex"] {
            let e = find_client(id).unwrap().compile(&stdio_cfg());
            assert!(
                e.value.get("environment").is_none() && e.value.get("env").is_none(),
                "{id} plain stdio must not write an env field"
            );
        }
    }

    #[test]
    fn stdio_token_for_env_incapable_client_is_dropped() {
        // Clients with no documented env field (well, cursor here as stand-in)
        // must not write the token into an invented key.
        let e = find_client("cursor").unwrap().compile(&stdio_token_cfg());
        assert!(e.value.get("env").is_none());
        assert!(e.value.get("environment").is_none());
        // The stdio command still stands.
        assert_eq!(e.value["command"], "lific");
    }

    #[test]
    fn connect_stdio_env_uses_per_client_env_key() {
        // opencode=environment, claude-code & codex=env — the three named in
        // the spec. Others are None.
        let spec_env = |id: &str| find_client(id).unwrap().stdio_env_key;
        assert_eq!(spec_env("opencode"), Some("environment"));
        assert_eq!(spec_env("claude-code"), Some("env"));
        assert_eq!(spec_env("codex"), Some("env"));
        for id in [
            "claude-desktop",
            "cursor",
            "vscode",
            "zed",
            "gemini",
            "windsurf",
            "goose",
            "crush",
        ] {
            assert_eq!(spec_env(id), None, "{id} has no documented stdio env field");
        }
    }

    #[test]
    fn claude_code_paths_and_http_type() {
        let base = linux_base();
        let c = find_client("claude-code").unwrap();
        assert_eq!(
            c.path_for(&base, Scope::Global).unwrap(),
            PathBuf::from("/home/tester/.claude.json")
        );
        assert_eq!(
            c.path_for(&base, Scope::Project).unwrap(),
            PathBuf::from("/proj/.mcp.json")
        );
        let e = c.compile(&remote_cfg());
        assert_eq!(e.top_key, "mcpServers");
        assert_eq!(e.value["type"], "http");
    }

    #[test]
    fn claude_desktop_remote_uses_mcp_remote_shim() {
        let e = find_client("claude-desktop").unwrap().compile(&remote_cfg());
        assert_eq!(e.value["command"], "npx");
        let args = e.value["args"].as_array().unwrap();
        assert!(args.iter().any(|a| a == "mcp-remote"));
        assert!(
            args.iter()
                .any(|a| a.as_str() == Some("Authorization: Bearer lific_sk-live-KEY"))
        );
        assert!(!e.notes.is_empty(), "shim note should be present");
    }

    #[test]
    fn claude_desktop_has_no_project_path() {
        let base = linux_base();
        assert!(
            find_client("claude-desktop")
                .unwrap()
                .path_for(&base, Scope::Project)
                .is_none()
        );
    }

    #[test]
    fn claude_desktop_mac_path_is_application_support() {
        let base = PathBase {
            home: PathBuf::from("/Users/tester"),
            project: PathBuf::from("/proj"),
            os: Os::Mac,
            appdata: None,
        };
        assert_eq!(
            find_client("claude-desktop")
                .unwrap()
                .path_for(&base, Scope::Global)
                .unwrap(),
            PathBuf::from(
                "/Users/tester/Library/Application Support/Claude/claude_desktop_config.json"
            )
        );
    }

    #[test]
    fn claude_desktop_windows_path_uses_appdata() {
        let base = PathBase {
            home: PathBuf::from("C:\\Users\\tester"),
            project: PathBuf::from("C:\\proj"),
            os: Os::Windows,
            appdata: Some(PathBuf::from("C:\\Users\\tester\\AppData\\Roaming")),
        };
        assert_eq!(
            find_client("claude-desktop")
                .unwrap()
                .path_for(&base, Scope::Global)
                .unwrap(),
            PathBuf::from(
                "C:\\Users\\tester\\AppData\\Roaming/Claude/claude_desktop_config.json"
            )
        );
    }

    #[test]
    fn vscode_uses_servers_key_not_mcpservers() {
        let e = find_client("vscode").unwrap().compile(&remote_cfg());
        assert_eq!(e.top_key, "servers");
        assert_eq!(e.value["type"], "http");
    }

    #[test]
    fn vscode_mac_global_path_is_application_support() {
        let base = PathBase {
            home: PathBuf::from("/Users/tester"),
            project: PathBuf::from("/proj"),
            os: Os::Mac,
            appdata: None,
        };
        assert_eq!(
            find_client("vscode")
                .unwrap()
                .path_for(&base, Scope::Global)
                .unwrap(),
            PathBuf::from("/Users/tester/Library/Application Support/Code/User/mcp.json")
        );
    }

    #[test]
    fn cursor_remote_uses_bare_url_and_headers() {
        let e = find_client("cursor").unwrap().compile(&remote_cfg());
        assert_eq!(e.top_key, "mcpServers");
        assert_eq!(e.value["url"], "http://127.0.0.1:3456/mcp");
        assert!(e.value.get("type").is_none());
    }

    #[test]
    fn gemini_remote_uses_httpurl_not_url() {
        let e = find_client("gemini").unwrap().compile(&remote_cfg());
        assert_eq!(e.value["httpUrl"], "http://127.0.0.1:3456/mcp");
        assert!(
            e.value.get("url").is_none(),
            "gemini must not use the `url` key"
        );
    }

    #[test]
    fn windsurf_remote_uses_serverurl_not_url() {
        let e = find_client("windsurf").unwrap().compile(&remote_cfg());
        assert_eq!(e.value["serverUrl"], "http://127.0.0.1:3456/mcp");
        assert!(
            e.value.get("url").is_none(),
            "windsurf must not use the `url` key"
        );
    }

    #[test]
    fn windsurf_has_no_project_path() {
        let base = linux_base();
        assert!(
            find_client("windsurf")
                .unwrap()
                .path_for(&base, Scope::Project)
                .is_none()
        );
    }

    #[test]
    fn codex_remote_uses_env_var_and_dotted_key() {
        let e = find_client("codex").unwrap().compile(&remote_cfg());
        assert_eq!(e.top_key, "mcp_servers.lific");
        assert_eq!(e.value["url"], "http://127.0.0.1:3456/mcp");
        assert_eq!(e.value["bearer_token_env_var"], "LIFIC_API_KEY");
        // The key itself is NEVER written inline for Codex.
        assert!(
            !e.value.to_string().contains("lific_sk-live-KEY"),
            "codex must not inline the bearer key"
        );
        assert!(!e.notes.is_empty());
    }

    #[test]
    fn zed_uses_context_servers_key() {
        let e = find_client("zed").unwrap().compile(&remote_cfg());
        assert_eq!(e.top_key, "context_servers");
    }

    #[test]
    fn goose_remote_uses_streamable_http_and_uri() {
        let e = find_client("goose").unwrap().compile(&remote_cfg());
        assert_eq!(e.top_key, "extensions");
        assert_eq!(e.value["type"], "streamable_http");
        assert_eq!(e.value["uri"], "http://127.0.0.1:3456/mcp");
        assert_eq!(e.value["enabled"], true);
        assert_eq!(e.value["timeout"], 300);
    }

    #[test]
    fn crush_remote_uses_mcp_key_and_http_type() {
        let base = linux_base();
        let c = find_client("crush").unwrap();
        assert_eq!(
            c.path_for(&base, Scope::Project).unwrap(),
            PathBuf::from("/proj/crush.json")
        );
        let e = c.compile(&remote_cfg());
        assert_eq!(e.top_key, "mcp");
        assert_eq!(e.value["type"], "http");
    }

    #[test]
    fn find_client_unknown_is_none() {
        assert!(find_client("notaclient").is_none());
    }

    #[test]
    fn all_client_ids_are_unique() {
        let ids = all_client_ids();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids.len(), sorted.len(), "client ids must be unique");
    }
}
