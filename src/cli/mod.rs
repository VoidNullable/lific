pub mod agents_md;
pub mod connect;
pub mod credentials;
pub mod doctor;
pub mod exec;
pub mod import;
pub mod login;
pub mod service;
pub mod term;
pub mod ui;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "lific",
    version,
    about = "Local-first, lightweight issue tracker"
)]
pub struct Cli {
    /// Path to config file (default: auto-discover lific.toml)
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    /// Path to the SQLite database file (overrides config)
    #[arg(long, global = true)]
    pub db: Option<PathBuf>,

    /// Output as JSON (for scripting/agent consumption)
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Start the HTTP API + MCP server
    Start {
        /// Port to listen on (overrides config)
        #[arg(short, long)]
        port: Option<u16>,

        /// Host to bind to (overrides config)
        #[arg(long)]
        host: Option<String>,
    },

    /// Run MCP server over stdio (for AI assistants)
    Mcp,

    /// Sign in to a Lific server via the OAuth 2.0 device flow (RFC 8628).
    ///
    /// Prints a verification URL and short code; approve it on any device
    /// (phone, browser) while this command polls until you do. The resulting
    /// OAuth token is stored in your OS keyring (or a 0600 file fallback) so
    /// later commands can reuse it.
    ///
    /// Agent/CI friendly: `--non-interactive` prints the code + device_code as
    /// JSON and exits immediately; complete the login later (after a human
    /// approves) with `lific login --complete <device_code>`.
    Login {
        /// Base URL of the server (default: server.public_url, else
        /// http://127.0.0.1:<port>).
        #[arg(long)]
        url: Option<String>,

        /// Print the device code as JSON and exit without polling (Stripe-style
        /// two-step flow). Also implied when stdin is not a TTY.
        #[arg(long = "non-interactive")]
        non_interactive: bool,

        /// Resume a previously started login by polling for this device_code
        /// until it is approved/denied/expired, then store the token.
        #[arg(long)]
        complete: Option<String>,

        /// Human-friendly label for this login, shown on the approval page.
        #[arg(long)]
        label: Option<String>,

        /// Don't persist the token (print it instead). Useful for one-off /
        /// scripted use where storage isn't wanted.
        #[arg(long = "no-store")]
        no_store: bool,
    },

    /// Sign out: delete the stored credential for a server and best-effort
    /// revoke it server-side.
    Logout {
        /// Base URL of the server (default: server.public_url, else
        /// http://127.0.0.1:<port>).
        #[arg(long)]
        url: Option<String>,
    },

    /// Diagnose the local setup (config, database, backups, running server).
    ///
    /// Runs a series of checks and prints a green/yellow/red status per check
    /// (pass/warn/fail), like `gh auth status` or `claude doctor`. Exits 0 when
    /// nothing failed, 1 otherwise — so agents and CI can gate on it. Safe to
    /// run whether or not a server is up; server-dependent checks are skipped
    /// (not failed) when nothing is listening.
    Doctor {
        /// API key to test an authorized MCP round-trip. Falls back to the
        /// LIFIC_API_KEY environment variable. Without a key, doctor still
        /// verifies that auth is enforced and discovery is advertised.
        #[arg(long, env = "LIFIC_API_KEY")]
        key: Option<String>,
    },

    /// Set up a ready-to-use Lific instance.
    ///
    /// One command, whole story: writes lific.toml (kept if already present),
    /// creates the database, prints your initial API key, installs a
    /// background service (systemd user unit on Linux, LaunchAgent on macOS)
    /// and starts it, then waits until the server answers. Re-running is safe
    /// and repairs whatever is missing. Use `--no-service` if you'd rather run
    /// `lific start` in the foreground yourself.
    ///
    /// By default the instance lives in your OS's standard locations (config
    /// in ~/.config/lific/, data in ~/.local/share/lific/ on Linux; macOS and
    /// Windows equivalents) so running init from any directory targets the
    /// same instance. Use `--here` for a directory-local instance
    /// (./lific.toml + ./lific.db, the pre-2.1 behavior), or `--config
    /// <path>` to root it at an explicit config file. If the current
    /// directory already has a lific.toml, init keeps operating on it.
    Init {
        /// Only write config and create the database; don't install or start
        /// a background service.
        #[arg(long = "no-service")]
        no_service: bool,

        /// Create the instance in the current directory (./lific.toml,
        /// ./lific.db) instead of the OS config/data directories.
        /// Conflicts with --config (which already names an exact location;
        /// enforced at run time because --config is a global arg).
        #[arg(long)]
        here: bool,
    },

    /// Manage the background service that `lific init` installs.
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },

    /// Write a single self-contained backup archive of the whole data set.
    ///
    /// Produces `lific_YYYYMMDD_HHMMSS.tar.gz` (gitea-dump style) containing a
    /// consistent snapshot of the database, every attachment blob, and a
    /// `manifest.json`. Safe to run while the server is running (the DB is
    /// snapshotted via `VACUUM INTO`, no writer lock is held). The archive is
    /// chmod 0600. Point external harnesses (restic/borg/cron) at the output,
    /// or call this as a pre-backup hook.
    Dump {
        /// Output target: a file path, or a directory (the default filename is
        /// used inside it). Defaults to the current working directory.
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Restore the data set from a `lific dump` archive.
    ///
    /// STOP THE SERVER FIRST — restoring under a live server corrupts state.
    /// Validates the archive (manifest + db present, no path traversal in
    /// attachment entries, schema not newer than this binary), then stages the
    /// extraction in a temp dir and moves it into place, so a failure leaves
    /// the original data dir untouched. Refuses to overwrite an existing
    /// database unless `--force`; with `--force` the current db is moved aside
    /// to `lific.db.pre-restore-<timestamp>` rather than deleted. After a
    /// restore, start the server — any pending migrations apply on startup.
    Restore {
        /// Path to the `.tar.gz` archive produced by `lific dump`.
        archive: PathBuf,

        /// Overwrite an existing database (moving the current one aside).
        #[arg(long)]
        force: bool,
    },

    /// Write Lific's MCP config into your AI clients (the fastest way to
    /// connect an editor/agent to this Lific instance).
    ///
    /// With no --client and an interactive terminal, probes for installed
    /// clients and lets you pick. Non-interactively you must name at least one
    /// --client and pass --yes. One bot + API key is minted PER selected client
    /// (named after the tool, matching the web UI's Connected Tools) so the
    /// audit log attributes changes to the specific harness. --key reuses one
    /// key verbatim for all clients; --oauth mints nothing and writes
    /// header-less config for the client's native OAuth flow.
    Connect {
        /// Client id to configure (repeatable). Known ids: opencode,
        /// claude-code, claude-desktop, cursor, vscode, codex, zed, gemini,
        /// windsurf, goose, crush.
        #[arg(long = "client")]
        clients: Vec<String>,

        /// Scope to write: global (user home, default) or project (this repo).
        #[arg(long, default_value = "global")]
        scope: String,

        /// Write the local stdio form (`lific --db <db> mcp`) instead of a
        /// remote HTTP server. No API key is needed.
        #[arg(long)]
        stdio: bool,

        /// Write a header-less remote config and let the client's native MCP
        /// OAuth flow authenticate. Mints no key or bot. Conflicts with --stdio
        /// and --key. OAuth-incapable clients are skipped with a note.
        #[arg(long)]
        oauth: bool,

        /// Override the MCP URL (default: server.public_url, else
        /// http://127.0.0.1:<port>/mcp).
        #[arg(long)]
        url: Option<String>,

        /// Use this API key verbatim instead of minting one.
        #[arg(long)]
        key: Option<String>,

        /// User who should own the minted connection key (it inherits their
        /// project access under authorization enforcement).
        #[arg(long)]
        user: Option<String>,

        /// Proceed without interactive prompts (required non-interactively).
        #[arg(long)]
        yes: bool,

        /// Print what would be written without touching any files.
        #[arg(long = "dry-run")]
        dry_run: bool,

        /// Don't write/update ./AGENTS.md.
        #[arg(long = "skip-agents")]
        skip_agents: bool,
    },

    /// Write or update the Lific block in a project's AGENTS.md so agents in
    /// that repo know it uses Lific for issue tracking (LIF-251).
    ///
    /// Idempotent: re-running replaces the marker-delimited block in place and
    /// preserves everything around it. Creates AGENTS.md if missing.
    AgentsMd {
        /// Path to the AGENTS.md file (default: ./AGENTS.md).
        #[arg(long)]
        path: Option<PathBuf>,

        /// Project identifier to bake into the CLI examples (e.g. LIF). If
        /// omitted, a generic placeholder is used with a discovery note.
        #[arg(long)]
        project: Option<String>,
    },

    /// Generate shell completions (e.g. `lific completion fish | source`)
    Completion {
        /// Shell to generate completions for (bash, zsh, fish, powershell, elvish)
        shell: clap_complete::Shell,
    },

    /// Inspect instance-wide settings and state (admin/operator scope)
    Instance {
        #[command(subcommand)]
        action: InstanceAction,
    },

    /// Manage API keys
    Key {
        #[command(subcommand)]
        action: KeyAction,
    },

    /// Manage user accounts
    User {
        #[command(subcommand)]
        action: UserAction,
    },

    /// Manage project membership (who can see and edit which project).
    ///
    /// With authorization enforcement on (`lific instance set
    /// --authz-enforced true`, the default on fresh installs), users only see
    /// projects they are members of — a freshly created user sees nothing
    /// until granted access here (or via the web UI's project members page).
    Member {
        #[command(subcommand)]
        action: MemberAction,
    },

    /// Manage issues
    Issue {
        #[command(subcommand)]
        action: IssueAction,
    },

    /// Manage projects
    Project {
        #[command(subcommand)]
        action: ProjectAction,
    },

    /// Manage pages (documentation)
    Page {
        #[command(subcommand)]
        action: PageAction,
    },

    /// Export issues and pages as markdown
    Export {
        #[command(subcommand)]
        action: ExportAction,
    },

    /// Search issues and pages
    Search {
        /// Search query text
        query: String,

        /// Filter to a specific project (identifier, e.g. LIF)
        #[arg(short, long)]
        project: Option<String>,

        /// Max results (default 20)
        #[arg(short, long)]
        limit: Option<i64>,
    },

    /// Manage comments on issues
    Comment {
        #[command(subcommand)]
        action: CommentAction,
    },

    /// Manage modules
    Module {
        #[command(subcommand)]
        action: ModuleAction,
    },

    /// Manage labels
    Label {
        #[command(subcommand)]
        action: LabelAction,
    },

    /// Manage folders
    Folder {
        #[command(subcommand)]
        action: FolderAction,
    },

    /// Import issues from an external tracker (GitHub, Linear, Jira).
    ///
    /// Each importer records a stable `source` marker per issue so re-running
    /// is idempotent: already-imported issues are skipped, never duplicated.
    /// Imported comments are attributed to a dedicated import bot (owned by a
    /// human, like `lific connect` bots) so the audit log shows provenance.
    /// `--dry-run` reports counts and writes nothing.
    Import {
        #[command(subcommand)]
        action: ImportAction,
    },
}

// ── Service ──────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum ServiceAction {
    /// Install the background service for the instance in the current
    /// directory (or --config) and start it. Safe to re-run; rewrites the
    /// definition to match this binary and config.
    Install,

    /// Stop the service and remove its definition.
    Uninstall,

    /// Show whether the service is installed and running.
    Status,

    /// Stop the running service (keeps it installed; it returns on reboot).
    Stop,

    /// Restart the service (or start it if stopped).
    Restart,
}

// ── Import ───────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum ImportAction {
    /// Import GitHub repo issues (filters out pull requests).
    Github {
        /// Source repo as owner/name (e.g. octocat/hello).
        #[arg(long)]
        repo: String,

        /// Destination Lific project identifier (e.g. APP).
        #[arg(long)]
        project: String,

        /// Which issues to import: open, closed, or all.
        #[arg(long, default_value = "all")]
        state: String,

        /// GitHub token (falls back to GITHUB_TOKEN). Optional for public
        /// repos, but strongly recommended to avoid the 60 req/hr anon limit.
        #[arg(long, env = "GITHUB_TOKEN")]
        token: Option<String>,

        /// Lific status for open issues.
        #[arg(long = "map-open", default_value = "backlog")]
        map_open: String,

        /// Lific status for closed issues.
        #[arg(long = "map-closed", default_value = "done")]
        map_closed: String,

        /// Username who should own the import bot (defaults to the sole human
        /// / sole admin; required when several users exist).
        #[arg(long)]
        user: Option<String>,

        /// Report what would be imported without writing anything.
        #[arg(long = "dry-run")]
        dry_run: bool,
    },

    /// Import Linear team issues.
    Linear {
        /// Linear team key (e.g. ENG).
        #[arg(long)]
        team: String,

        /// Destination Lific project identifier.
        #[arg(long)]
        project: String,

        /// Linear personal API key (falls back to LINEAR_API_KEY).
        #[arg(long, env = "LINEAR_API_KEY")]
        token: Option<String>,

        /// Username who should own the import bot.
        #[arg(long)]
        user: Option<String>,

        /// Report what would be imported without writing anything.
        #[arg(long = "dry-run")]
        dry_run: bool,
    },

    /// Import Jira Cloud project issues.
    Jira {
        /// Jira site slug (the `<site>` in <site>.atlassian.net).
        #[arg(long)]
        site: String,

        /// Jira project key (e.g. PROJ).
        #[arg(long = "jira-project")]
        jira_project: String,

        /// Destination Lific project identifier.
        #[arg(long)]
        project: String,

        /// Jira account email (falls back to JIRA_EMAIL).
        #[arg(long, env = "JIRA_EMAIL")]
        email: Option<String>,

        /// Jira API token (falls back to JIRA_API_TOKEN).
        #[arg(long, env = "JIRA_API_TOKEN")]
        token: Option<String>,

        /// Username who should own the import bot.
        #[arg(long)]
        user: Option<String>,

        /// Report what would be imported without writing anything.
        #[arg(long = "dry-run")]
        dry_run: bool,
    },
}

// ── Issue ────────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum IssueAction {
    /// List issues in a project
    List {
        /// Project identifier (e.g. LIF)
        #[arg(short, long)]
        project: String,

        /// Filter by status: backlog, todo, active, done, cancelled
        #[arg(short, long)]
        status: Option<String>,

        /// Filter by priority: urgent, high, medium, low, none
        #[arg(long)]
        priority: Option<String>,

        /// Filter by module name
        #[arg(short, long)]
        module: Option<String>,

        /// Filter by label name
        #[arg(short, long)]
        label: Option<String>,

        /// Only show workable issues (no unresolved blockers)
        #[arg(short, long)]
        workable: bool,

        /// Max results (default 50)
        #[arg(long)]
        limit: Option<i64>,
    },

    /// Get a single issue by identifier (e.g. LIF-42)
    Get {
        /// Issue identifier (e.g. LIF-42)
        identifier: String,
    },

    /// Create a new issue
    Create {
        /// Project identifier (e.g. LIF)
        #[arg(short, long)]
        project: String,

        /// Issue title
        #[arg(short, long)]
        title: String,

        /// Issue description (markdown)
        #[arg(short, long, default_value = "")]
        description: String,

        /// Status: backlog, todo, active, done, cancelled
        #[arg(short, long, default_value = "backlog")]
        status: String,

        /// Priority: urgent, high, medium, low, none
        #[arg(long, default_value = "none")]
        priority: String,

        /// Module name to assign to
        #[arg(short, long)]
        module: Option<String>,

        /// Labels to attach (comma-separated)
        #[arg(short, long)]
        labels: Option<String>,
    },

    /// Update an existing issue
    Update {
        /// Issue identifier (e.g. LIF-42)
        identifier: String,

        /// New title
        #[arg(short, long)]
        title: Option<String>,

        /// New description
        #[arg(short, long)]
        description: Option<String>,

        /// New status
        #[arg(short, long)]
        status: Option<String>,

        /// New priority
        #[arg(long)]
        priority: Option<String>,

        /// New module name
        #[arg(short, long)]
        module: Option<String>,

        /// Replace labels (comma-separated)
        #[arg(short, long)]
        labels: Option<String>,
    },
}

// ── Project ──────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum ProjectAction {
    /// List all projects
    List,

    /// Get a single project by identifier
    Get {
        /// Project identifier (e.g. LIF)
        identifier: String,
    },

    /// Create a new project
    Create {
        /// Project name
        #[arg(short, long)]
        name: String,

        /// Short identifier (max 5 chars, e.g. LIF)
        #[arg(short, long)]
        identifier: String,

        /// Description
        #[arg(short, long, default_value = "")]
        description: String,
    },

    /// Update an existing project
    Update {
        /// Project identifier (e.g. LIF)
        identifier: String,

        /// New name
        #[arg(short, long)]
        name: Option<String>,

        /// New description
        #[arg(short, long)]
        description: Option<String>,
    },
}

// ── Page ─────────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum PageAction {
    /// List pages in a project (or workspace pages if no project)
    List {
        /// Project identifier (e.g. LIF). Omit for workspace pages.
        #[arg(short, long)]
        project: Option<String>,

        /// Filter by folder name
        #[arg(short, long)]
        folder: Option<String>,

        /// Filter by label name (LIF-105)
        #[arg(short, long)]
        label: Option<String>,
    },

    /// Get a single page by identifier (e.g. LIF-DOC-1)
    Get {
        /// Page identifier (e.g. LIF-DOC-1 or DOC-1)
        identifier: String,
    },

    /// Create a new page
    Create {
        /// Page title
        #[arg(short, long)]
        title: String,

        /// Project identifier (e.g. LIF). Omit for workspace page.
        #[arg(short, long)]
        project: Option<String>,

        /// Folder name to place page in
        #[arg(short, long)]
        folder: Option<String>,

        /// Page content (markdown)
        #[arg(short, long, default_value = "")]
        content: String,

        /// Labels to attach (comma-separated). Ignored for workspace pages.
        #[arg(short, long)]
        labels: Option<String>,
    },

    /// Update an existing page
    Update {
        /// Page identifier (e.g. LIF-DOC-1)
        identifier: String,

        /// New title
        #[arg(short, long)]
        title: Option<String>,

        /// New content
        #[arg(short, long)]
        content: Option<String>,

        /// Move to folder name
        #[arg(short, long)]
        folder: Option<String>,

        /// Replace labels (comma-separated). Ignored for workspace pages.
        #[arg(short, long)]
        labels: Option<String>,
    },
}

// ── Export ───────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum ExportAction {
    /// Export a single issue to a target directory
    Issue {
        /// Issue identifier (e.g. LIF-42)
        identifier: String,

        /// Output directory for exported files
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Export a single page to a target directory
    Page {
        /// Page identifier (e.g. LIF-DOC-1)
        identifier: String,

        /// Output directory for exported files
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Export a whole project to a target directory
    Project {
        /// Project identifier (e.g. LIF)
        project: String,

        /// Output directory for exported files
        #[arg(short, long)]
        output: PathBuf,
    },
}

// ── Comment ──────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum CommentAction {
    /// List comments on an issue
    List {
        /// Issue identifier (e.g. LIF-42)
        identifier: String,
    },

    /// Add a comment to an issue
    Add {
        /// Issue identifier (e.g. LIF-42)
        identifier: String,

        /// Comment content (markdown)
        #[arg(short, long)]
        content: String,

        /// Username of the comment author (defaults to first admin user)
        #[arg(short, long)]
        user: Option<String>,
    },
}

// ── Module ───────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum ModuleAction {
    /// List modules in a project
    List {
        /// Project identifier (e.g. LIF)
        #[arg(short, long)]
        project: String,
    },

    /// Create a new module
    Create {
        /// Project identifier (e.g. LIF)
        #[arg(short, long)]
        project: String,

        /// Module name
        #[arg(short, long)]
        name: String,

        /// Description
        #[arg(short, long, default_value = "")]
        description: String,

        /// Status: backlog, planned, active, paused, done, cancelled
        #[arg(short, long, default_value = "active")]
        status: String,
    },

    /// Update a module
    Update {
        /// Project identifier (e.g. LIF)
        #[arg(short, long)]
        project: String,

        /// Current module name
        #[arg(short, long)]
        name: String,

        /// New name
        #[arg(long)]
        new_name: Option<String>,

        /// New description
        #[arg(short, long)]
        description: Option<String>,

        /// New status
        #[arg(short, long)]
        status: Option<String>,
    },

    /// Delete a module
    Delete {
        /// Project identifier (e.g. LIF)
        #[arg(short, long)]
        project: String,

        /// Module name
        #[arg(short, long)]
        name: String,
    },
}

// ── Label ────────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum LabelAction {
    /// List labels in a project
    List {
        /// Project identifier (e.g. LIF)
        #[arg(short, long)]
        project: String,
    },

    /// Create a new label
    Create {
        /// Project identifier (e.g. LIF)
        #[arg(short, long)]
        project: String,

        /// Label name
        #[arg(short, long)]
        name: String,

        /// Color hex (e.g. #EF4444)
        #[arg(short, long, default_value = "#6B7280")]
        color: String,
    },

    /// Update a label
    Update {
        /// Project identifier (e.g. LIF)
        #[arg(short, long)]
        project: String,

        /// Current label name
        #[arg(short, long)]
        name: String,

        /// New name
        #[arg(long)]
        new_name: Option<String>,

        /// New color hex
        #[arg(short, long)]
        color: Option<String>,
    },

    /// Delete a label
    Delete {
        /// Project identifier (e.g. LIF)
        #[arg(short, long)]
        project: String,

        /// Label name
        #[arg(short, long)]
        name: String,
    },
}

// ── Folder ───────────────────────────────────────────────────

#[derive(Subcommand)]
pub enum FolderAction {
    /// List folders in a project
    List {
        /// Project identifier (e.g. LIF)
        #[arg(short, long)]
        project: String,
    },

    /// Create a new folder
    Create {
        /// Project identifier (e.g. LIF)
        #[arg(short, long)]
        project: String,

        /// Folder name
        #[arg(short, long)]
        name: String,
    },

    /// Update a folder
    Update {
        /// Project identifier (e.g. LIF)
        #[arg(short, long)]
        project: String,

        /// Current folder name
        #[arg(short, long)]
        name: String,

        /// New name
        #[arg(long)]
        new_name: String,
    },

    /// Delete a folder
    Delete {
        /// Project identifier (e.g. LIF)
        #[arg(short, long)]
        project: String,

        /// Folder name
        #[arg(short, long)]
        name: String,
    },
}

// ── Instance ─────────────────────────────────────────────────
//
// Instance-scoped settings and state, kept distinct from per-user `user`
// management and from the project/content commands. Today it is read-only
// (`info`); runtime-editable settings (signup toggle, instance name) land
// with the DB-backed instance-settings store.

#[derive(Subcommand)]
pub enum InstanceAction {
    /// Show instance settings and state
    Info,

    /// Change instance settings (only the flags you pass are updated)
    Set {
        /// Instance name (pass "" to clear and fall back to the host)
        #[arg(long)]
        name: Option<String>,

        /// Open or close self-service signup
        #[arg(long)]
        signups: Option<bool>,

        /// Comma-separated email domains allowed to self-register
        /// (e.g. "acme.com,sub.acme.com"; pass "" to allow any)
        #[arg(long = "signup-domains")]
        signup_domains: Option<String>,

        /// How long a login session stays valid, in days (1-365)
        #[arg(long = "session-days")]
        session_days: Option<i64>,

        /// Short message shown on the auth screen (pass "" to clear)
        #[arg(long = "login-message")]
        login_message: Option<String>,

        /// Single-user mode: auto-sign-in the web UI as the admin (no login
        /// screen). Browser-only; REST/MCP still need tokens. Dangerous if the
        /// instance is publicly reachable.
        #[arg(long = "auto-login")]
        auto_login: Option<bool>,

        /// Enable project-scoped default-deny authorization (LIF-194). When
        /// true, viewer/maintainer/lead project membership is enforced on every
        /// REST/MCP call, including reads. Fresh installs (created 2.1+) seed
        /// this ON; upgrades keep it OFF until you set it. Unbound API keys
        /// (from `lific start`/`lific key create`/`connect`) are operator-
        /// trusted and bypass this by design — audit them with `lific key list`.
        #[arg(long = "authz-enforced")]
        authz_enforced: Option<bool>,
    },
}

#[derive(Subcommand)]
pub enum KeyAction {
    /// Create a new API key
    Create {
        /// Name for this key (e.g. "claude", "opencode", "personal")
        #[arg(short, long)]
        name: String,

        /// Username to assign this key to
        #[arg(short, long)]
        user: Option<String>,

        /// Expiry as an ISO 8601 date (2026-12-31) or datetime
        /// (2026-12-31T23:59:59Z). After this instant the key stops
        /// authenticating. Omit for a key that never expires.
        #[arg(short, long)]
        expires: Option<String>,
    },

    /// Assign an existing API key to a user
    Assign {
        /// Name of the API key
        #[arg(short, long)]
        name: String,

        /// Username to assign the key to
        #[arg(short, long)]
        user: String,
    },

    /// List all API keys (never shows the key itself)
    List,

    /// Revoke an API key by name
    Revoke {
        /// Name of the key to revoke
        #[arg(short, long)]
        name: String,
    },

    /// Rotate an API key (revoke old, generate new)
    Rotate {
        /// Name of the key to rotate
        #[arg(short, long)]
        name: String,
    },
}

#[derive(Subcommand)]
pub enum MemberAction {
    /// List a project's members and their roles
    List {
        /// Project identifier (e.g. LIF)
        #[arg(short, long)]
        project: String,
    },

    /// Grant a user access to a project (or to every project with --all)
    Add {
        /// Project identifier (e.g. LIF). Required unless --all is given.
        #[arg(short, long, conflicts_with = "all", required_unless_present = "all")]
        project: Option<String>,

        /// Username to grant access to
        #[arg(short, long)]
        user: String,

        /// Role: viewer, maintainer, or lead
        #[arg(short, long, default_value = "viewer")]
        role: String,

        /// Grant access to every existing project (projects where the user
        /// is already a member are skipped, their role untouched)
        #[arg(long)]
        all: bool,
    },

    /// Change an existing member's role
    Role {
        /// Project identifier (e.g. LIF)
        #[arg(short, long)]
        project: String,

        /// Username of the member
        #[arg(short, long)]
        user: String,

        /// New role: viewer, maintainer, or lead
        #[arg(short, long)]
        role: String,
    },

    /// Remove a user's access to a project
    Remove {
        /// Project identifier (e.g. LIF)
        #[arg(short, long)]
        project: String,

        /// Username of the member
        #[arg(short, long)]
        user: String,
    },
}

#[derive(Subcommand)]
pub enum UserAction {
    /// Create a new user account
    Create {
        /// Username (unique, case-insensitive)
        #[arg(short, long)]
        username: String,

        /// Email address (unique)
        #[arg(short, long)]
        email: String,

        /// Password (prompted interactively if omitted)
        #[arg(short, long)]
        password: Option<String>,

        /// Grant admin privileges
        #[arg(long)]
        admin: bool,

        /// Mark as a bot account
        #[arg(long)]
        bot: bool,
    },

    /// List all user accounts
    List,

    /// Set a user's password (operator password reset — no current password
    /// needed; shell access to the server is the trust boundary). Invalidates
    /// all of the user's sessions, same as a self-service password change.
    SetPassword {
        /// Username whose password to set
        #[arg(short, long)]
        username: String,

        /// New password (prompted interactively if omitted)
        #[arg(short, long)]
        password: Option<String>,
    },

    /// Promote a user to admin
    Promote {
        /// Username to promote
        #[arg(short, long)]
        username: String,
    },

    /// Demote an admin to regular user
    Demote {
        /// Username to demote
        #[arg(short, long)]
        username: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parse_start_defaults() {
        let cli = Cli::try_parse_from(["lific", "start"]).unwrap();
        assert!(cli.config.is_none());
        assert!(cli.db.is_none());
        assert!(!cli.json);
        match cli.command {
            Command::Start { port, host } => {
                assert!(port.is_none());
                assert!(host.is_none());
            }
            _ => panic!("expected Start"),
        }
    }

    #[test]
    fn parse_start_with_overrides() {
        let cli = Cli::try_parse_from([
            "lific",
            "--db",
            "/tmp/test.db",
            "start",
            "--port",
            "8080",
            "--host",
            "127.0.0.1",
        ])
        .unwrap();
        assert_eq!(cli.db, Some(PathBuf::from("/tmp/test.db")));
        match cli.command {
            Command::Start { port, host } => {
                assert_eq!(port, Some(8080));
                assert_eq!(host, Some("127.0.0.1".into()));
            }
            _ => panic!("expected Start"),
        }
    }

    #[test]
    fn parse_mcp() {
        let cli = Cli::try_parse_from(["lific", "mcp"]).unwrap();
        assert!(matches!(cli.command, Command::Mcp));
    }

    #[test]
    fn parse_init() {
        let cli = Cli::try_parse_from(["lific", "init"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Init {
                no_service: false,
                here: false
            }
        ));
    }

    #[test]
    fn parse_init_no_service() {
        let cli = Cli::try_parse_from(["lific", "init", "--no-service"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Init {
                no_service: true,
                here: false
            }
        ));
    }

    // LIF-295: --here forces the directory-local layout. (Its conflict with
    // the global --config is enforced at run time in cmd_init, out of clap's
    // reach — covered by main.rs's init_target_tests.)
    #[test]
    fn parse_init_here() {
        let cli = Cli::try_parse_from(["lific", "init", "--here"]).unwrap();
        assert!(matches!(cli.command, Command::Init { here: true, .. }));
    }

    #[test]
    fn parse_service_actions() {
        for (arg, want) in [
            ("install", "Install"),
            ("uninstall", "Uninstall"),
            ("status", "Status"),
            ("stop", "Stop"),
            ("restart", "Restart"),
        ] {
            let cli = Cli::try_parse_from(["lific", "service", arg]).unwrap();
            let Command::Service { action } = cli.command else {
                panic!("expected Service for {arg}");
            };
            let got = match action {
                ServiceAction::Install => "Install",
                ServiceAction::Uninstall => "Uninstall",
                ServiceAction::Status => "Status",
                ServiceAction::Stop => "Stop",
                ServiceAction::Restart => "Restart",
            };
            assert_eq!(got, want);
        }
    }

    #[test]
    fn service_requires_an_action() {
        assert!(Cli::try_parse_from(["lific", "service"]).is_err());
    }

    #[test]
    fn parse_dump_defaults() {
        let cli = Cli::try_parse_from(["lific", "dump"]).unwrap();
        match cli.command {
            Command::Dump { out } => assert!(out.is_none()),
            _ => panic!("expected Dump"),
        }
    }

    #[test]
    fn parse_dump_with_out() {
        let cli = Cli::try_parse_from(["lific", "dump", "--out", "/tmp/backups"]).unwrap();
        match cli.command {
            Command::Dump { out } => assert_eq!(out, Some(PathBuf::from("/tmp/backups"))),
            _ => panic!("expected Dump"),
        }
    }

    #[test]
    fn parse_restore_defaults() {
        let cli = Cli::try_parse_from(["lific", "restore", "/tmp/b.tar.gz"]).unwrap();
        match cli.command {
            Command::Restore { archive, force } => {
                assert_eq!(archive, PathBuf::from("/tmp/b.tar.gz"));
                assert!(!force);
            }
            _ => panic!("expected Restore"),
        }
    }

    #[test]
    fn parse_restore_force() {
        let cli =
            Cli::try_parse_from(["lific", "restore", "/tmp/b.tar.gz", "--force"]).unwrap();
        match cli.command {
            Command::Restore { archive, force } => {
                assert_eq!(archive, PathBuf::from("/tmp/b.tar.gz"));
                assert!(force);
            }
            _ => panic!("expected Restore"),
        }
    }

    #[test]
    fn parse_doctor_no_key() {
        // A bare `lific doctor` parses; key is absent unless LIFIC_API_KEY is
        // set in the environment (clap env fallback). Guard against a stray
        // env var polluting the assertion.
        let cli = Cli::try_parse_from(["lific", "doctor"]).unwrap();
        match cli.command {
            Command::Doctor { key } => {
                assert_eq!(key, std::env::var("LIFIC_API_KEY").ok());
            }
            _ => panic!("expected Doctor"),
        }
    }

    // ── login / logout parse tests ───────────────────────────

    #[test]
    fn parse_login_defaults() {
        let cli = Cli::try_parse_from(["lific", "login"]).unwrap();
        match cli.command {
            Command::Login {
                url,
                non_interactive,
                complete,
                label,
                no_store,
            } => {
                assert!(url.is_none());
                assert!(!non_interactive);
                assert!(complete.is_none());
                assert!(label.is_none());
                assert!(!no_store);
            }
            _ => panic!("expected Login"),
        }
    }

    #[test]
    fn parse_login_all_flags() {
        let cli = Cli::try_parse_from([
            "lific",
            "login",
            "--url",
            "http://127.0.0.1:3998",
            "--non-interactive",
            "--label",
            "my-laptop",
            "--no-store",
        ])
        .unwrap();
        match cli.command {
            Command::Login {
                url,
                non_interactive,
                complete,
                label,
                no_store,
            } => {
                assert_eq!(url, Some("http://127.0.0.1:3998".into()));
                assert!(non_interactive);
                assert!(complete.is_none());
                assert_eq!(label, Some("my-laptop".into()));
                assert!(no_store);
            }
            _ => panic!("expected Login"),
        }
    }

    #[test]
    fn parse_login_complete() {
        let cli = Cli::try_parse_from([
            "lific", "login", "--complete", "abc123def", "--url", "http://h:1",
        ])
        .unwrap();
        match cli.command {
            Command::Login {
                complete, url, ..
            } => {
                assert_eq!(complete, Some("abc123def".into()));
                assert_eq!(url, Some("http://h:1".into()));
            }
            _ => panic!("expected Login"),
        }
    }

    #[test]
    fn parse_logout_defaults() {
        let cli = Cli::try_parse_from(["lific", "logout"]).unwrap();
        match cli.command {
            Command::Logout { url } => assert!(url.is_none()),
            _ => panic!("expected Logout"),
        }
    }

    #[test]
    fn parse_logout_with_url() {
        let cli = Cli::try_parse_from(["lific", "logout", "--url", "https://lific.example"]).unwrap();
        match cli.command {
            Command::Logout { url } => assert_eq!(url, Some("https://lific.example".into())),
            _ => panic!("expected Logout"),
        }
    }

    #[test]
    fn parse_doctor_with_key_flag() {
        let cli = Cli::try_parse_from(["lific", "doctor", "--key", "lific_sk-live-abc"]).unwrap();
        match cli.command {
            Command::Doctor { key } => assert_eq!(key, Some("lific_sk-live-abc".into())),
            _ => panic!("expected Doctor"),
        }
    }

    // ── connect / agents-md parse tests ──────────────────────

    #[test]
    fn parse_connect_defaults() {
        let cli = Cli::try_parse_from(["lific", "connect"]).unwrap();
        match cli.command {
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
                assert!(clients.is_empty());
                assert_eq!(scope, "global");
                assert!(!stdio);
                assert!(!oauth);
                assert!(url.is_none());
                assert!(key.is_none());
                assert!(user.is_none());
                assert!(!yes);
                assert!(!dry_run);
                assert!(!skip_agents);
            }
            _ => panic!("expected Connect"),
        }
    }

    #[test]
    fn parse_connect_oauth_flag() {
        let cli = Cli::try_parse_from([
            "lific", "connect", "--oauth", "--client", "opencode", "--yes",
        ])
        .unwrap();
        match cli.command {
            Command::Connect {
                oauth,
                clients,
                stdio,
                key,
                ..
            } => {
                assert!(oauth, "--oauth must parse");
                assert_eq!(clients, vec!["opencode".to_string()]);
                assert!(!stdio);
                assert!(key.is_none());
            }
            _ => panic!("expected Connect"),
        }
    }

    #[test]
    fn parse_connect_repeatable_client_and_flags() {
        let cli = Cli::try_parse_from([
            "lific", "connect",
            "--client", "opencode",
            "--client", "codex",
            "--scope", "project",
            "--stdio",
            "--url", "http://127.0.0.1:9000/mcp",
            "--key", "lific_sk-live-K",
            "--user", "blake",
            "--yes",
            "--dry-run",
            "--skip-agents",
        ])
        .unwrap();
        match cli.command {
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
                assert_eq!(clients, vec!["opencode".to_string(), "codex".to_string()]);
                assert_eq!(scope, "project");
                assert!(stdio);
                assert!(!oauth);
                assert_eq!(url, Some("http://127.0.0.1:9000/mcp".into()));
                assert_eq!(key, Some("lific_sk-live-K".into()));
                assert_eq!(user, Some("blake".into()));
                assert!(yes);
                assert!(dry_run);
                assert!(skip_agents);
            }
            _ => panic!("expected Connect"),
        }
    }

    #[test]
    fn parse_agents_md_defaults() {
        let cli = Cli::try_parse_from(["lific", "agents-md"]).unwrap();
        match cli.command {
            Command::AgentsMd { path, project } => {
                assert!(path.is_none());
                assert!(project.is_none());
            }
            _ => panic!("expected AgentsMd"),
        }
    }

    #[test]
    fn parse_agents_md_with_path_and_project() {
        let cli = Cli::try_parse_from([
            "lific", "agents-md", "--path", "docs/AGENTS.md", "--project", "LIF",
        ])
        .unwrap();
        match cli.command {
            Command::AgentsMd { path, project } => {
                assert_eq!(path, Some(PathBuf::from("docs/AGENTS.md")));
                assert_eq!(project, Some("LIF".into()));
            }
            _ => panic!("expected AgentsMd"),
        }
    }

    #[test]
    fn parse_completion_fish() {
        let cli = Cli::try_parse_from(["lific", "completion", "fish"]).unwrap();
        match cli.command {
            Command::Completion { shell } => {
                assert_eq!(shell, clap_complete::Shell::Fish);
            }
            _ => panic!("expected Completion"),
        }
    }

    #[test]
    fn parse_completion_bash() {
        let cli = Cli::try_parse_from(["lific", "completion", "bash"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Completion {
                shell: clap_complete::Shell::Bash,
            }
        ));
    }

    #[test]
    fn parse_completion_rejects_garbage() {
        assert!(Cli::try_parse_from(["lific", "completion", "notashell"]).is_err());
    }

    #[test]
    fn parse_instance_info() {
        let cli = Cli::try_parse_from(["lific", "instance", "info"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Instance {
                action: InstanceAction::Info,
            }
        ));
    }

    #[test]
    fn parse_instance_set() {
        let cli = Cli::try_parse_from([
            "lific", "instance", "set",
            "--name", "Acme Eng",
            "--signups", "false",
            "--signup-domains", "acme.com,sub.acme.com",
            "--session-days", "14",
            "--login-message", "Ask #it for access",
            "--auto-login", "true",
            "--authz-enforced", "true",
        ])
        .unwrap();
        match cli.command {
            Command::Instance {
                action:
                    InstanceAction::Set {
                        name,
                        signups,
                        signup_domains,
                        session_days,
                        login_message,
                        auto_login,
                        authz_enforced,
                    },
            } => {
                assert_eq!(name, Some("Acme Eng".into()));
                assert_eq!(signups, Some(false));
                assert_eq!(signup_domains, Some("acme.com,sub.acme.com".into()));
                assert_eq!(session_days, Some(14));
                assert_eq!(login_message, Some("Ask #it for access".into()));
                assert_eq!(auto_login, Some(true));
                assert_eq!(authz_enforced, Some(true));
            }
            _ => panic!("expected Instance Set"),
        }
    }

    #[test]
    fn parse_key_create() {
        let cli = Cli::try_parse_from(["lific", "key", "create", "--name", "test-key"]).unwrap();
        match cli.command {
            Command::Key {
                action: KeyAction::Create {
                    name,
                    user,
                    expires,
                },
            } => {
                assert_eq!(name, "test-key");
                assert!(user.is_none());
                assert!(expires.is_none());
            }
            _ => panic!("expected Key Create"),
        }
    }

    #[test]
    fn parse_key_create_with_user() {
        let cli = Cli::try_parse_from([
            "lific", "key", "create", "--name", "my-key", "--user", "blake",
        ])
        .unwrap();
        match cli.command {
            Command::Key {
                action: KeyAction::Create {
                    name,
                    user,
                    expires,
                },
            } => {
                assert_eq!(name, "my-key");
                assert_eq!(user, Some("blake".into()));
                assert!(expires.is_none());
            }
            _ => panic!("expected Key Create"),
        }
    }

    #[test]
    fn parse_key_create_with_expires() {
        let cli = Cli::try_parse_from([
            "lific",
            "key",
            "create",
            "--name",
            "temp-key",
            "--expires",
            "2026-12-31",
        ])
        .unwrap();
        match cli.command {
            Command::Key {
                action: KeyAction::Create {
                    name,
                    user,
                    expires,
                },
            } => {
                assert_eq!(name, "temp-key");
                assert!(user.is_none());
                assert_eq!(expires, Some("2026-12-31".into()));
            }
            _ => panic!("expected Key Create"),
        }
    }

    #[test]
    fn parse_key_assign() {
        let cli = Cli::try_parse_from([
            "lific", "key", "assign", "--name", "opencode", "--user", "blake",
        ])
        .unwrap();
        match cli.command {
            Command::Key {
                action: KeyAction::Assign { name, user },
            } => {
                assert_eq!(name, "opencode");
                assert_eq!(user, "blake");
            }
            _ => panic!("expected Key Assign"),
        }
    }

    #[test]
    fn parse_key_revoke() {
        let cli = Cli::try_parse_from(["lific", "key", "revoke", "--name", "old"]).unwrap();
        match cli.command {
            Command::Key {
                action: KeyAction::Revoke { name },
            } => assert_eq!(name, "old"),
            _ => panic!("expected Key Revoke"),
        }
    }

    #[test]
    fn parse_user_create() {
        let cli = Cli::try_parse_from([
            "lific",
            "user",
            "create",
            "--username",
            "blake",
            "--email",
            "b@test.com",
            "--password",
            "secret123",
            "--admin",
        ])
        .unwrap();
        match cli.command {
            Command::User {
                action:
                    UserAction::Create {
                        username,
                        email,
                        password,
                        admin,
                        bot,
                    },
            } => {
                assert_eq!(username, "blake");
                assert_eq!(email, "b@test.com");
                assert_eq!(password, Some("secret123".into()));
                assert!(admin);
                assert!(!bot);
            }
            _ => panic!("expected User Create"),
        }
    }

    #[test]
    fn parse_user_list() {
        let cli = Cli::try_parse_from(["lific", "user", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::User {
                action: UserAction::List,
            }
        ));
    }

    // ── LIF-291: user set-password ───────────────────────────

    #[test]
    fn parse_user_set_password() {
        let cli = Cli::try_parse_from([
            "lific", "user", "set-password", "--username", "blake",
        ])
        .unwrap();
        match cli.command {
            Command::User {
                action: UserAction::SetPassword { username, password },
            } => {
                assert_eq!(username, "blake");
                assert!(password.is_none(), "password omitted → interactive prompt");
            }
            _ => panic!("expected User SetPassword"),
        }
    }

    #[test]
    fn parse_user_set_password_with_password_flag() {
        let cli = Cli::try_parse_from([
            "lific", "user", "set-password", "-u", "blake", "-p", "newpass123",
        ])
        .unwrap();
        match cli.command {
            Command::User {
                action: UserAction::SetPassword { username, password },
            } => {
                assert_eq!(username, "blake");
                assert_eq!(password, Some("newpass123".into()));
            }
            _ => panic!("expected User SetPassword"),
        }
    }

    // ── LIF-290: member management ───────────────────────────

    #[test]
    fn parse_member_add_defaults_to_viewer() {
        let cli = Cli::try_parse_from([
            "lific", "member", "add", "--project", "LIF", "--user", "bob",
        ])
        .unwrap();
        match cli.command {
            Command::Member {
                action: MemberAction::Add { project, user, role, all },
            } => {
                assert_eq!(project, Some("LIF".into()));
                assert_eq!(user, "bob");
                assert_eq!(role, "viewer");
                assert!(!all);
            }
            _ => panic!("expected Member Add"),
        }
    }

    #[test]
    fn parse_member_add_all_projects() {
        let cli = Cli::try_parse_from([
            "lific", "member", "add", "--all", "--user", "bob", "--role", "maintainer",
        ])
        .unwrap();
        match cli.command {
            Command::Member {
                action: MemberAction::Add { project, user, role, all },
            } => {
                assert!(project.is_none());
                assert_eq!(user, "bob");
                assert_eq!(role, "maintainer");
                assert!(all);
            }
            _ => panic!("expected Member Add"),
        }
    }

    #[test]
    fn parse_member_add_requires_project_or_all() {
        assert!(
            Cli::try_parse_from(["lific", "member", "add", "--user", "bob"]).is_err(),
            "one of --project / --all is required"
        );
        assert!(
            Cli::try_parse_from([
                "lific", "member", "add", "--project", "LIF", "--all", "--user", "bob",
            ])
            .is_err(),
            "--project conflicts with --all"
        );
    }

    #[test]
    fn parse_member_list_role_remove() {
        assert!(matches!(
            Cli::try_parse_from(["lific", "member", "list", "--project", "LIF"])
                .unwrap()
                .command,
            Command::Member { action: MemberAction::List { .. } }
        ));
        match Cli::try_parse_from([
            "lific", "member", "role", "-p", "LIF", "-u", "bob", "-r", "lead",
        ])
        .unwrap()
        .command
        {
            Command::Member {
                action: MemberAction::Role { project, user, role },
            } => {
                assert_eq!((project.as_str(), user.as_str(), role.as_str()), ("LIF", "bob", "lead"));
            }
            _ => panic!("expected Member Role"),
        }
        assert!(matches!(
            Cli::try_parse_from(["lific", "member", "remove", "-p", "LIF", "-u", "bob"])
                .unwrap()
                .command,
            Command::Member { action: MemberAction::Remove { .. } }
        ));
    }

    #[test]
    fn parse_global_config_flag() {
        let cli = Cli::try_parse_from(["lific", "--config", "/etc/lific.toml", "start"]).unwrap();
        assert_eq!(cli.config, Some(PathBuf::from("/etc/lific.toml")));
    }

    #[test]
    fn missing_subcommand_errors() {
        assert!(Cli::try_parse_from(["lific"]).is_err());
    }

    // ── Issue CLI tests ──────────────────────────────────────

    #[test]
    fn parse_issue_list() {
        let cli = Cli::try_parse_from(["lific", "issue", "list", "--project", "LIF"]).unwrap();
        match cli.command {
            Command::Issue {
                action:
                    IssueAction::List {
                        project,
                        status,
                        priority,
                        module,
                        label,
                        workable,
                        limit,
                    },
            } => {
                assert_eq!(project, "LIF");
                assert!(status.is_none());
                assert!(priority.is_none());
                assert!(module.is_none());
                assert!(label.is_none());
                assert!(!workable);
                assert!(limit.is_none());
            }
            _ => panic!("expected Issue List"),
        }
    }

    #[test]
    fn parse_issue_list_with_filters() {
        let cli = Cli::try_parse_from([
            "lific",
            "issue",
            "list",
            "--project",
            "LIF",
            "--status",
            "active",
            "--priority",
            "urgent",
            "--workable",
            "--limit",
            "10",
        ])
        .unwrap();
        match cli.command {
            Command::Issue {
                action:
                    IssueAction::List {
                        project,
                        status,
                        priority,
                        workable,
                        limit,
                        ..
                    },
            } => {
                assert_eq!(project, "LIF");
                assert_eq!(status, Some("active".into()));
                assert_eq!(priority, Some("urgent".into()));
                assert!(workable);
                assert_eq!(limit, Some(10));
            }
            _ => panic!("expected Issue List"),
        }
    }

    #[test]
    fn parse_issue_get() {
        let cli = Cli::try_parse_from(["lific", "issue", "get", "LIF-42"]).unwrap();
        match cli.command {
            Command::Issue {
                action: IssueAction::Get { identifier },
            } => assert_eq!(identifier, "LIF-42"),
            _ => panic!("expected Issue Get"),
        }
    }

    #[test]
    fn parse_issue_create() {
        let cli = Cli::try_parse_from([
            "lific",
            "issue",
            "create",
            "--project",
            "LIF",
            "--title",
            "Fix bug",
            "--priority",
            "high",
            "--labels",
            "bug,urgent",
        ])
        .unwrap();
        match cli.command {
            Command::Issue {
                action:
                    IssueAction::Create {
                        project,
                        title,
                        priority,
                        labels,
                        status,
                        ..
                    },
            } => {
                assert_eq!(project, "LIF");
                assert_eq!(title, "Fix bug");
                assert_eq!(priority, "high");
                assert_eq!(status, "backlog");
                assert_eq!(labels, Some("bug,urgent".into()));
            }
            _ => panic!("expected Issue Create"),
        }
    }

    #[test]
    fn parse_issue_update() {
        let cli = Cli::try_parse_from(["lific", "issue", "update", "LIF-42", "--status", "done"])
            .unwrap();
        match cli.command {
            Command::Issue {
                action:
                    IssueAction::Update {
                        identifier,
                        status,
                        title,
                        ..
                    },
            } => {
                assert_eq!(identifier, "LIF-42");
                assert_eq!(status, Some("done".into()));
                assert!(title.is_none());
            }
            _ => panic!("expected Issue Update"),
        }
    }

    // ── Project CLI tests ────────────────────────────────────

    #[test]
    fn parse_project_list() {
        let cli = Cli::try_parse_from(["lific", "project", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Project {
                action: ProjectAction::List
            }
        ));
    }

    #[test]
    fn parse_project_create() {
        let cli = Cli::try_parse_from([
            "lific",
            "project",
            "create",
            "--name",
            "My Project",
            "--identifier",
            "MP",
        ])
        .unwrap();
        match cli.command {
            Command::Project {
                action:
                    ProjectAction::Create {
                        name,
                        identifier,
                        description,
                    },
            } => {
                assert_eq!(name, "My Project");
                assert_eq!(identifier, "MP");
                assert_eq!(description, "");
            }
            _ => panic!("expected Project Create"),
        }
    }

    // ── Search CLI test ──────────────────────────────────────

    #[test]
    fn parse_search() {
        let cli =
            Cli::try_parse_from(["lific", "search", "auth flow", "--project", "LIF"]).unwrap();
        match cli.command {
            Command::Search {
                query,
                project,
                limit,
            } => {
                assert_eq!(query, "auth flow");
                assert_eq!(project, Some("LIF".into()));
                assert!(limit.is_none());
            }
            _ => panic!("expected Search"),
        }
    }

    #[test]
    fn parse_export_project() {
        let cli =
            Cli::try_parse_from(["lific", "export", "project", "LIF", "--output", "/tmp/out"])
                .unwrap();
        match cli.command {
            Command::Export {
                action: ExportAction::Project { project, output },
            } => {
                assert_eq!(project, "LIF");
                assert_eq!(output, PathBuf::from("/tmp/out"));
            }
            _ => panic!("expected Export Project"),
        }
    }

    // ── Comment CLI tests ────────────────────────────────────

    #[test]
    fn parse_comment_list() {
        let cli = Cli::try_parse_from(["lific", "comment", "list", "LIF-42"]).unwrap();
        match cli.command {
            Command::Comment {
                action: CommentAction::List { identifier },
            } => assert_eq!(identifier, "LIF-42"),
            _ => panic!("expected Comment List"),
        }
    }

    #[test]
    fn parse_comment_add() {
        let cli = Cli::try_parse_from([
            "lific",
            "comment",
            "add",
            "LIF-42",
            "--content",
            "Looking into this",
        ])
        .unwrap();
        match cli.command {
            Command::Comment {
                action:
                    CommentAction::Add {
                        identifier,
                        content,
                        user,
                    },
            } => {
                assert_eq!(identifier, "LIF-42");
                assert_eq!(content, "Looking into this");
                assert!(user.is_none());
            }
            _ => panic!("expected Comment Add"),
        }
    }

    // ── Module CLI tests ─────────────────────────────────────

    #[test]
    fn parse_module_list() {
        let cli = Cli::try_parse_from(["lific", "module", "list", "--project", "LIF"]).unwrap();
        match cli.command {
            Command::Module {
                action: ModuleAction::List { project },
            } => assert_eq!(project, "LIF"),
            _ => panic!("expected Module List"),
        }
    }

    #[test]
    fn parse_module_create() {
        let cli = Cli::try_parse_from([
            "lific",
            "module",
            "create",
            "--project",
            "LIF",
            "--name",
            "Core",
        ])
        .unwrap();
        match cli.command {
            Command::Module {
                action:
                    ModuleAction::Create {
                        project,
                        name,
                        status,
                        ..
                    },
            } => {
                assert_eq!(project, "LIF");
                assert_eq!(name, "Core");
                assert_eq!(status, "active");
            }
            _ => panic!("expected Module Create"),
        }
    }

    // ── Label CLI tests ──────────────────────────────────────

    #[test]
    fn parse_label_create() {
        let cli = Cli::try_parse_from([
            "lific",
            "label",
            "create",
            "--project",
            "LIF",
            "--name",
            "bug",
            "--color",
            "#EF4444",
        ])
        .unwrap();
        match cli.command {
            Command::Label {
                action:
                    LabelAction::Create {
                        project,
                        name,
                        color,
                    },
            } => {
                assert_eq!(project, "LIF");
                assert_eq!(name, "bug");
                assert_eq!(color, "#EF4444");
            }
            _ => panic!("expected Label Create"),
        }
    }

    // ── Folder CLI tests ─────────────────────────────────────

    #[test]
    fn parse_folder_create() {
        let cli = Cli::try_parse_from([
            "lific",
            "folder",
            "create",
            "--project",
            "LIF",
            "--name",
            "Architecture",
        ])
        .unwrap();
        match cli.command {
            Command::Folder {
                action: FolderAction::Create { project, name },
            } => {
                assert_eq!(project, "LIF");
                assert_eq!(name, "Architecture");
            }
            _ => panic!("expected Folder Create"),
        }
    }

    // ── JSON flag test ───────────────────────────────────────

    #[test]
    fn parse_json_flag() {
        let cli = Cli::try_parse_from(["lific", "--json", "project", "list"]).unwrap();
        assert!(cli.json);
        assert!(matches!(
            cli.command,
            Command::Project {
                action: ProjectAction::List
            }
        ));
    }

    #[test]
    fn json_flag_defaults_to_false() {
        let cli = Cli::try_parse_from(["lific", "project", "list"]).unwrap();
        assert!(!cli.json);
    }
}
