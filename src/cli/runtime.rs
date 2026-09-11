//! Command execution policy derived from the parsed CLI command.
//!
//! Keeping these decisions together prevents the top-level runner from having
//! several independent command lists that can drift when a new subcommand is
//! added.

use super::{Command, ServiceAction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandKind {
    Data,
    Doctor,
    Completion,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DatabaseRequirement {
    None,
    Existing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackendSupport {
    LocalOnly,
    LocalAndHttp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SigpipePolicy {
    Restore,
    KeepIgnored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CommandPlan {
    kind: CommandKind,
    database: DatabaseRequirement,
    backend: BackendSupport,
    sigpipe: SigpipePolicy,
}

impl CommandPlan {
    #[must_use]
    pub(crate) const fn is_data(self) -> bool {
        matches!(self.kind, CommandKind::Data)
    }

    #[must_use]
    pub(crate) const fn is_doctor(self) -> bool {
        matches!(self.kind, CommandKind::Doctor)
    }

    #[must_use]
    pub(crate) const fn is_completion(self) -> bool {
        matches!(self.kind, CommandKind::Completion)
    }

    #[must_use]
    pub(crate) const fn requires_existing_database(self) -> bool {
        matches!(self.database, DatabaseRequirement::Existing)
    }

    #[must_use]
    pub(crate) const fn supports_http(self) -> bool {
        matches!(self.backend, BackendSupport::LocalAndHttp)
    }

    #[must_use]
    pub(crate) const fn restores_sigpipe(self) -> bool {
        matches!(self.sigpipe, SigpipePolicy::Restore)
    }
}

#[must_use]
pub(crate) fn plan(command: &Command) -> CommandPlan {
    let data = CommandPlan {
        kind: CommandKind::Data,
        database: DatabaseRequirement::Existing,
        backend: BackendSupport::LocalAndHttp,
        sigpipe: SigpipePolicy::Restore,
    };

    match command {
        Command::Issue { .. }
        | Command::Project { .. }
        | Command::Page { .. }
        | Command::Export { .. }
        | Command::Search { .. }
        | Command::Comment { .. }
        | Command::Module { .. }
        | Command::Label { .. }
        | Command::Folder { .. }
        | Command::Bind { .. }
        | Command::GitHook { .. } => data,
        Command::Doctor { .. } => CommandPlan {
            kind: CommandKind::Doctor,
            database: DatabaseRequirement::None,
            backend: BackendSupport::LocalOnly,
            sigpipe: SigpipePolicy::Restore,
        },
        Command::Completion { .. } => CommandPlan {
            kind: CommandKind::Completion,
            database: DatabaseRequirement::None,
            backend: BackendSupport::LocalOnly,
            sigpipe: SigpipePolicy::Restore,
        },
        Command::Init { .. }
        | Command::Login { .. }
        | Command::Logout { .. }
        | Command::Connect { .. }
        | Command::AgentsMd { .. }
        | Command::Import { .. } => no_database(),
        Command::Mcp {
            remote, instances, ..
        } => CommandPlan {
            kind: CommandKind::Other,
            database: if *remote || instances.is_some() {
                DatabaseRequirement::None
            } else {
                DatabaseRequirement::Existing
            },
            backend: BackendSupport::LocalOnly,
            sigpipe: SigpipePolicy::KeepIgnored,
        },
        Command::Start {
            init_if_missing, ..
        } => CommandPlan {
            kind: CommandKind::Other,
            database: if *init_if_missing {
                DatabaseRequirement::None
            } else {
                DatabaseRequirement::Existing
            },
            backend: BackendSupport::LocalOnly,
            sigpipe: SigpipePolicy::KeepIgnored,
        },
        Command::Service { action } => CommandPlan {
            kind: CommandKind::Other,
            database: if matches!(action, ServiceAction::Install) {
                DatabaseRequirement::Existing
            } else {
                DatabaseRequirement::None
            },
            backend: BackendSupport::LocalOnly,
            sigpipe: SigpipePolicy::Restore,
        },
        Command::ProjectArchive { .. }
        | Command::Dump { .. }
        | Command::Restore { .. }
        | Command::Instance { .. }
        | Command::Key { .. }
        | Command::User { .. }
        | Command::Member { .. } => existing_local(),
    }
}

const fn no_database() -> CommandPlan {
    CommandPlan {
        kind: CommandKind::Other,
        database: DatabaseRequirement::None,
        backend: BackendSupport::LocalOnly,
        sigpipe: SigpipePolicy::Restore,
    }
}

const fn existing_local() -> CommandPlan {
    CommandPlan {
        kind: CommandKind::Other,
        database: DatabaseRequirement::Existing,
        backend: BackendSupport::LocalOnly,
        sigpipe: SigpipePolicy::Restore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_commands_share_local_and_http_policy() {
        let plan = plan(&Command::Bind {
            project: None,
            create: false,
        });
        assert!(plan.is_data());
        assert!(plan.requires_existing_database());
        assert!(plan.supports_http());
        assert!(plan.restores_sigpipe());
    }

    #[test]
    fn service_database_requirement_depends_on_action() {
        assert!(
            plan(&Command::Service {
                action: ServiceAction::Install,
            })
            .requires_existing_database()
        );
        assert!(
            !plan(&Command::Service {
                action: ServiceAction::Status,
            })
            .requires_existing_database()
        );
    }

    #[test]
    fn remote_mcp_and_first_boot_skip_database_guard() {
        assert!(
            !plan(&Command::Mcp {
                remote: true,
                url: None,
                instances: None,
            })
            .requires_existing_database()
        );
        assert!(
            !plan(&Command::Start {
                port: None,
                host: None,
                init_if_missing: true,
            })
            .requires_existing_database()
        );
    }
}
