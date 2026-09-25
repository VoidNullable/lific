//! LIF-484: user and date blockers over MCP.
//!
//! No new tools. A wait is a `blocks` link whose source is a person or a
//! window of days instead of an issue, so it rides `link_issues` (`user`, or
//! `from`/`until`, in place of `source`) and `unlink_issues` (`user`, or
//! `from`). Reads surface waits on `get_issue`, `list_issues` and
//! `get_board`; see [`WaitTokens`] and [`WaitLines`] for the two shapes.

use std::fmt::{self, Display, Write as _};

use crate::db::models::{self, CreateWait, IssueWait, WaitKind, WaitState};
use crate::db::queries::{self, waits};

use super::LificMcp;
use super::schemas::{LinkIssuesInput, UnlinkIssuesInput};

/// Whether a link/unlink call is about a wait rather than an issue pair.
pub(super) fn link_is_wait(input: &LinkIssuesInput) -> bool {
    [&input.user, &input.from, &input.until]
        .iter()
        .any(|value| value.as_deref().is_some_and(|v| !v.trim().is_empty()))
}

pub(super) fn unlink_is_wait(input: &UnlinkIssuesInput) -> bool {
    [&input.user, &input.from]
        .iter()
        .any(|value| value.as_deref().is_some_and(|v| !v.trim().is_empty()))
}

/// The compact list/board form, appended to an issue line:
/// ` waiting_on:@blake`, ` waiting_until:2026-09-28..29`,
/// ` due_since:2026-09-28`, ` overdue_since:2026-09-30`. Notes are left to
/// `get_issue`: a board full of them would cost more than it tells.
pub(super) struct WaitTokens<'a>(pub &'a [IssueWait]);

impl Display for WaitTokens<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|wait| {
            let earliest = wait.earliest.as_deref().unwrap_or("");
            let latest = wait.latest.as_deref().unwrap_or("");
            match (wait.kind, wait.state) {
                (WaitKind::User, _) => write!(formatter, " waiting_on:{}", waits::describe(wait)),
                (_, WaitState::Holding) => write!(
                    formatter,
                    " waiting_until:{}",
                    waits::format_window(earliest, latest)
                ),
                (_, WaitState::Due) => write!(formatter, " due_since:{earliest}"),
                (_, WaitState::Overdue) => {
                    write!(formatter, " overdue_since:{}", waits::day_after(latest))
                }
            }
        })
    }
}

/// One wait as a sentence, for `get_issue` and write confirmations:
///
/// * `Waiting on @blake (decide the schema)`
/// * `Waiting until 2026-09-28..29 (state filing office)`
/// * `Due to check since 2026-09-28, expected by 2026-09-29 (...)`
/// * `Overdue since 2026-09-30, expected 2026-09-28..29 (...)`
pub(super) struct WaitLine<'a>(pub &'a IssueWait);

impl Display for WaitLine<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let wait = self.0;
        let earliest = wait.earliest.as_deref().unwrap_or("");
        let latest = wait.latest.as_deref().unwrap_or("");
        let window = waits::format_window(earliest, latest);
        match (wait.kind, wait.state) {
            (WaitKind::User, _) => write!(formatter, "Waiting on {}", waits::describe(wait))?,
            (_, WaitState::Holding) => write!(formatter, "Waiting until {window}")?,
            (_, WaitState::Due) if earliest == latest => {
                write!(formatter, "Due to check since {earliest}")?
            }
            (_, WaitState::Due) => write!(
                formatter,
                "Due to check since {earliest}, expected by {latest}"
            )?,
            (_, WaitState::Overdue) => write!(
                formatter,
                "Overdue since {}, expected {window}",
                waits::day_after(latest)
            )?,
        }
        if !wait.note.is_empty() {
            write!(formatter, " ({})", wait.note)?;
        }
        Ok(())
    }
}

/// Every wait on its own line, for `get_issue`.
pub(super) struct WaitLines<'a>(pub &'a [IssueWait]);

impl Display for WaitLines<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0
            .iter()
            .try_for_each(|wait| writeln!(formatter, "{}", WaitLine(wait)))
    }
}

#[cfg(test)]
thread_local! {
    /// Test seam: runs after the read-side gate and before the write
    /// transaction opens, which is exactly the window a revocation must not
    /// be able to exploit.
    pub(super) static BEFORE_WAIT_WRITE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

fn before_wait_write() {
    #[cfg(test)]
    if let Some(hook) = BEFORE_WAIT_WRITE.with(|cell| cell.borrow_mut().take()) {
        hook();
    }
}

/// Re-assert Maintainer on the waiting issue's project against the
/// connection that is about to write, inside its transaction, and return the
/// caller's user id for `created_by`. The gate in [`LificMcp::waiting_issue`]
/// ran on a read connection earlier; a membership revoked since then is
/// visible here and refuses the write. Same shape as REST's
/// `require_role_conn` recheck in `api::waits`.
fn authorize_wait_write(
    conn: &rusqlite::Connection,
    issue_id: i64,
) -> Result<Option<i64>, crate::error::LificError> {
    let project_id = queries::issue_project_id(conn, issue_id)?;
    let identity = crate::resolve_caller::resolve_caller_conn(
        conn,
        super::current_auth_user(),
        crate::actor::Transport::Mcp,
    )?;
    crate::authz::require_role_conn(conn, &identity, project_id, models::Role::Maintainer)?;
    Ok(identity.map(|identity| identity.user.id))
}

impl LificMcp {
    /// The waiting issue, resolved and authorized as a Maintainer write.
    fn waiting_issue(&self, identifier: &str) -> Result<models::Issue, String> {
        let issue = self.read(|conn| {
            let id = queries::resolve_identifier(conn, identifier)?;
            queries::get_issue(conn, id)
        })?;
        crate::authz::require_role(
            &self.db,
            &super::current_identity(&self.db),
            issue.project_id,
            models::Role::Maintainer,
        )
        .map_err(|error| error.to_string())?;
        Ok(issue)
    }

    fn emit_wait_change(&self, issue_id: i64) {
        // The wait's triggers advanced the issue's seq; advertise that seq so
        // a replica pulls the re-delivered row.
        if let Ok((project_id, seq)) = self.read(|conn| {
            Ok((
                queries::issue_project_id(conn, issue_id)?,
                queries::issue_seq(conn, issue_id)?,
            ))
        }) {
            self.emit_with_seq(
                crate::realtime::RealtimeEvent::IssueUpdated {
                    project_id,
                    issue_id,
                },
                seq,
            );
        }
    }

    /// `link_issues` with `user` or `from`/`until`: `target` starts waiting.
    pub(super) fn link_wait(&self, input: &LinkIssuesInput) -> Result<String, String> {
        if input.relation_type != "blocks" {
            return Err(format!(
                "user and from/until make a blocks link; relation_type '{}' needs a source issue",
                input.relation_type
            ));
        }
        if !input.source.trim().is_empty() {
            return Err(
                "pass source (an issue) or user/from (a wait), not both; the wait is on target"
                    .into(),
            );
        }
        let issue = self.waiting_issue(&input.target)?;
        before_wait_write();
        let wait = self.transaction(|conn| {
            let created_by = authorize_wait_write(conn, issue.id)?;
            waits::add_wait(
                conn,
                issue.id,
                &CreateWait {
                    user: input.user.clone(),
                    from: input.from.clone(),
                    until: input.until.clone(),
                    note: input.note.clone(),
                },
                created_by,
            )
        })?;
        self.emit_wait_change(issue.id);
        Ok(format!("{}: {}", issue.identifier, WaitLine(&wait)))
    }

    /// `unlink_issues` with `user` or `from`: clear `target`'s wait.
    pub(super) fn unlink_wait(&self, input: &UnlinkIssuesInput) -> Result<String, String> {
        if !input.source.trim().is_empty() {
            return Err(
                "pass source (an issue) or user/from (a wait), not both; the wait is on target"
                    .into(),
            );
        }
        let user = input.user.as_deref().filter(|v| !v.trim().is_empty());
        let from = input.from.as_deref().filter(|v| !v.trim().is_empty());
        let issue = self.waiting_issue(&input.target)?;
        before_wait_write();
        let cleared = self.transaction(|conn| {
            authorize_wait_write(conn, issue.id)?;
            match (user, from) {
                (Some(_), Some(_)) => Err(crate::error::LificError::BadRequest(
                    "clear one wait at a time: pass user or from".into(),
                )),
                (Some(user), None) => Ok(vec![waits::clear_user_wait(conn, issue.id, user)?]),
                (None, Some(from)) => waits::clear_date_waits(conn, issue.id, from),
                (None, None) => Err(crate::error::LificError::BadRequest(
                    "pass user or from to clear a wait".into(),
                )),
            }
        })?;
        self.emit_wait_change(issue.id);
        let mut out = format!("Cleared {}'s wait", issue.identifier);
        if cleared.len() > 1 {
            out.push('s');
        }
        out.push_str(" on ");
        for (index, wait) in cleared.iter().enumerate() {
            if index > 0 {
                out.push_str(", ");
            }
            let _ = write!(out, "{}", waits::describe(wait));
        }
        Ok(out)
    }
}
