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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parse_start_defaults() {
        let cli = Cli::try_parse_from(["lific", "start"]).unwrap();
        assert!(cli.config.is_none());
        assert!(cli.db.is_none());
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
    fn parse_key_create() {
        let cli = Cli::try_parse_from(["lific", "key", "create", "--name", "test-key"]).unwrap();
        match cli.command {
            Command::Key {
                action: KeyAction::Create { name },
            } => assert_eq!(name, "test-key"),
            _ => panic!("expected Key Create"),
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
    fn parse_global_config_flag() {
        let cli = Cli::try_parse_from(["lific", "--config", "/etc/lific.toml", "start"]).unwrap();
        assert_eq!(cli.config, Some(PathBuf::from("/etc/lific.toml")));
    }

    #[test]
    fn missing_subcommand_errors() {
        assert!(Cli::try_parse_from(["lific"]).is_err());
    }
}
