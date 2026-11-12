//! LIF-541: what a read tool call read, collected while it runs and recorded
//! only once it has succeeded.
//!
//! Each read tool's inner function takes a [`ReadLog`] and notes its target
//! after the project or entity is resolved and authorized. The tool wrapper
//! hands the log and the result to [`LificMcp::finish_read`], which queues
//! one `agent_reads` row per target, and only when the call succeeded and
//! has an authenticated caller. A call that fails (unknown identifier,
//! forbidden) or notes no target records nothing. Rows are buffered on the
//! pool; see `crate::db::agent_reads`.

use super::LificMcp;
use crate::db::agent_reads::{AgentRead, now_ts};

/// The read targets of one tool call.
pub(crate) struct ReadLog {
    tool: &'static str,
    targets: Vec<ReadTarget>,
}

struct ReadTarget {
    project_id: Option<i64>,
    entity: Option<(&'static str, i64, String)>,
}

impl ReadLog {
    pub(crate) fn new(tool: &'static str) -> Self {
        Self {
            tool,
            targets: Vec::new(),
        }
    }

    /// A list or search read over one project, or over the whole workspace
    /// when `project_id` is `None`.
    pub(crate) fn scope(&mut self, project_id: Option<i64>) {
        self.targets.push(ReadTarget {
            project_id,
            entity: None,
        });
    }

    /// A read of one entity: `issue`, `page`, `plan`, `project` or
    /// `attachment`, with its human label (`LIF-42`, `LIF-DOC-3`, ...).
    pub(crate) fn entity(
        &mut self,
        project_id: Option<i64>,
        entity_type: &'static str,
        entity_id: i64,
        label: impl Into<String>,
    ) {
        self.targets.push(ReadTarget {
            project_id,
            entity: Some((entity_type, entity_id, label.into())),
        });
    }
}

impl LificMcp {
    /// Queue the log's rows when `succeeded` and the call has a caller.
    pub(crate) fn finish_read(&self, log: ReadLog, succeeded: bool) {
        if !succeeded || log.targets.is_empty() {
            return;
        }
        let Some(user) = super::current_auth_user() else {
            return;
        };
        let ts = now_ts();
        for target in log.targets {
            let (entity_type, entity_id, entity_label) = match target.entity {
                Some((kind, id, label)) => (Some(kind), Some(id), Some(label)),
                None => (None, None, None),
            };
            self.db.record_agent_read(AgentRead {
                ts: ts.clone(),
                user_id: user.id,
                tool: log.tool,
                project_id: target.project_id,
                entity_type,
                entity_id,
                entity_label,
            });
        }
    }

    /// The usual tool wrapper: record on success, render errors as text.
    pub(crate) fn logged_read(&self, log: ReadLog, result: Result<String, String>) -> String {
        self.finish_read(log, result.is_ok());
        result.unwrap_or_else(|error| format!("Error: {error}"))
    }

    /// [`Self::logged_read`] for a tool that also returns structured content
    /// (LIF-563).
    pub(crate) fn logged_structured<T: serde::Serialize>(
        &self,
        log: ReadLog,
        result: Result<(String, T), String>,
    ) -> super::outputs::StructuredReply {
        self.finish_read(log, result.is_ok());
        super::outputs::StructuredReply::new(result)
    }
}
