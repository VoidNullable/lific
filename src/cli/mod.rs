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

    /// Import projects, issues, and data from Plane
    ImportPlane {
        /// Plane API base URL (e.g. http://localhost:8585)
        #[arg(long, env = "PLANE_BASE_URL")]
        url: String,

        /// Plane API key
        #[arg(long, env = "PLANE_API_KEY")]
        api_key: String,

        /// Plane workspace slug
        #[arg(long, env = "PLANE_WORKSPACE_SLUG")]
        workspace: String,

        /// Skip projects with these identifiers (comma-separated)
        #[arg(long, value_delimiter = ',')]
        skip: Vec<String>,
    },

    /// Import from a pre-exported Plane JSON file (advanced)
    ImportFile {
        /// Path to the JSON export file
        file: PathBuf,

        /// Skip projects with these identifiers (comma-separated)
        #[arg(long, value_delimiter = ',')]
        skip: Vec<String>,
    },

    /// Manage API keys
    Key {
        #[command(subcommand)]
        action: KeyAction,
    },
}

#[derive(Subcommand)]
pub enum KeyAction {
    /// Create a new API key
    Create {
        /// Name for this key (e.g. "claude", "opencode", "personal")
        #[arg(short, long)]
        name: String,
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
