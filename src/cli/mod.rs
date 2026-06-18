pub mod exec;

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

    /// Generate a default lific.toml config file
    Init,

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
        assert!(matches!(cli.command, Command::Init));
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
                    },
            } => {
                assert_eq!(name, Some("Acme Eng".into()));
                assert_eq!(signups, Some(false));
                assert_eq!(signup_domains, Some("acme.com,sub.acme.com".into()));
                assert_eq!(session_days, Some(14));
                assert_eq!(login_message, Some("Ask #it for access".into()));
            }
            _ => panic!("expected Instance Set"),
        }
    }

    #[test]
    fn parse_key_create() {
        let cli = Cli::try_parse_from(["lific", "key", "create", "--name", "test-key"]).unwrap();
        match cli.command {
            Command::Key {
                action: KeyAction::Create { name, user },
            } => {
                assert_eq!(name, "test-key");
                assert!(user.is_none());
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
                action: KeyAction::Create { name, user },
            } => {
                assert_eq!(name, "my-key");
                assert_eq!(user, Some("blake".into()));
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
