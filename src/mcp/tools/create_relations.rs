//! LIF-482: relations named when an issue is created.
//!
//! `create_issue` takes `blocks`, `blocked_by` and `relates_to` on the single
//! form and on each batch item. References to existing issues are resolved,
//! and their projects role-checked, before the write transaction opens, the
//! same order `link_issues` uses. The relations are then written in the
//! transaction that creates the issues, so a bad reference creates nothing.
//! In a batch a reference may also name another item, by index (`2` or
//! `issues[2]`) or by its title, so a burst of related issues can link itself
//! in one call.

use std::collections::BTreeSet;
use std::fmt::{self, Write as _};

use crate::db::queries;
use crate::error::LificError;

use super::{IssueLinkContext, issue_reference};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Blocks,
    BlockedBy,
    RelatesTo,
}

impl Kind {
    const ALL: [Kind; 3] = [Kind::Blocks, Kind::BlockedBy, Kind::RelatesTo];

    fn field(self) -> &'static str {
        match self {
            Kind::Blocks => "blocks",
            Kind::BlockedBy => "blocked_by",
            Kind::RelatesTo => "relates_to",
        }
    }
}

/// The other end of a relation.
#[derive(Debug)]
pub(super) enum End {
    Existing {
        id: i64,
        project_id: i64,
        identifier: String,
    },
    /// Another item of the same batch, by index.
    Item(usize),
}

#[derive(Debug)]
pub(super) struct Planned {
    kind: Kind,
    end: End,
}

/// The three relation lists of one create, as sent.
#[derive(Clone, Copy)]
pub(super) struct Requested<'a> {
    pub blocks: Option<&'a [String]>,
    pub blocked_by: Option<&'a [String]>,
    pub relates_to: Option<&'a [String]>,
}

impl Requested<'_> {
    fn list(&self, kind: Kind) -> &[String] {
        match kind {
            Kind::Blocks => self.blocks,
            Kind::BlockedBy => self.blocked_by,
            Kind::RelatesTo => self.relates_to,
        }
        .unwrap_or_default()
    }
}

/// The batch an item belongs to: every item's title, and the item's index.
pub(super) struct Batch<'a> {
    pub titles: &'a [&'a str],
    pub own: usize,
}

/// Resolve every reference in `requested`. Errors name the reference as
/// `blocks[0]`, the path the caller sent it under.
pub(super) fn plan(
    conn: &rusqlite::Connection,
    requested: Requested<'_>,
    batch: Option<&Batch<'_>>,
) -> Result<Vec<Planned>, LificError> {
    Kind::ALL
        .into_iter()
        .flat_map(|kind| {
            requested
                .list(kind)
                .iter()
                .enumerate()
                .map(move |(index, reference)| (kind, index, reference))
        })
        .map(|(kind, index, reference)| {
            resolve(conn, reference, batch)
                .map(|end| Planned { kind, end })
                .map_err(|error| at(format!("{}[{index}]", kind.field()), error))
        })
        .collect()
}

fn resolve(
    conn: &rusqlite::Connection,
    reference: &str,
    batch: Option<&Batch<'_>>,
) -> Result<End, LificError> {
    let reference = reference.trim();
    let self_link = || LificError::BadRequest("an issue cannot be linked to itself".into());
    if let Some(batch) = batch {
        if let Some(index) = item_index(reference) {
            if index >= batch.titles.len() {
                return Err(LificError::BadRequest(format!(
                    "this batch has no issues[{index}]"
                )));
            }
            if index == batch.own {
                return Err(self_link());
            }
            return Ok(End::Item(index));
        }
        let titled = |title: &&str| title.trim().eq_ignore_ascii_case(reference);
        let others: Vec<usize> = (batch.titles.iter().enumerate())
            .filter(|&(index, title)| index != batch.own && titled(title))
            .map(|(index, _)| index)
            .collect();
        match others.as_slice() {
            [] if titled(&batch.titles[batch.own]) => return Err(self_link()),
            [] => {}
            [index] => return Ok(End::Item(*index)),
            several => {
                let names: Vec<String> = several.iter().map(|i| format!("issues[{i}]")).collect();
                return Err(LificError::BadRequest(format!(
                    "\"{reference}\" is the title of {}; name one by index",
                    names.join(" and ")
                )));
            }
        }
    }
    let id = queries::resolve_identifier(conn, reference).map_err(|error| match error {
        LificError::BadRequest(_) if batch.is_some() => LificError::BadRequest(format!(
            "\"{reference}\" is not an issue ID, or another item's title or index"
        )),
        other => other,
    })?;
    let issue = queries::get_issue(conn, id)?;
    Ok(End::Existing {
        id: issue.id,
        project_id: issue.project_id,
        identifier: issue.identifier,
    })
}

/// `2` or `issues[2]`.
fn item_index(reference: &str) -> Option<usize> {
    let digits = reference
        .strip_prefix("issues[")
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(reference);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn at(path: String, error: LificError) -> LificError {
    let at = |message: String| format!("{path}: {message}");
    match error {
        LificError::NotFound(message) => LificError::NotFound(at(message)),
        LificError::BadRequest(message) => LificError::BadRequest(at(message)),
        other => other,
    }
}

/// Existing issues the planned relations touch, as `(project_id, issue_id)`:
/// their projects need the Maintainer check, and their views a realtime event.
pub(super) fn existing<'a>(planned: impl IntoIterator<Item = &'a Planned>) -> BTreeSet<(i64, i64)> {
    planned
        .into_iter()
        .filter_map(|planned| match planned.end {
            End::Existing { id, project_id, .. } => Some((project_id, id)),
            End::Item(_) => None,
        })
        .collect()
}

/// Write the planned relations for the new issue `issue_id`. `batch_ids` maps
/// item indexes to the issues the batch created.
pub(super) fn apply(
    conn: &rusqlite::Connection,
    issue_id: i64,
    planned: &[Planned],
    batch_ids: &[i64],
) -> Result<(), LificError> {
    planned.iter().try_for_each(|planned| {
        let other = match planned.end {
            End::Existing { id, .. } => id,
            End::Item(index) => batch_ids[index],
        };
        let (source, target, relation) = match planned.kind {
            Kind::Blocks => (issue_id, other, "blocks"),
            Kind::BlockedBy => (other, issue_id, "blocks"),
            Kind::RelatesTo => (issue_id, other, "relates_to"),
        };
        queries::link_issues(conn, source, target, relation)
    })
}

/// `blocks TST-1, TST-2; relates_to TST-4`, or nothing when no relation was
/// asked for. `batch_identifiers` maps item indexes to identifiers.
pub(super) fn render(
    output: &mut String,
    context: Option<&IssueLinkContext>,
    planned: &[Planned],
    batch_identifiers: &[String],
) -> fmt::Result {
    let mut first_kind = true;
    for kind in Kind::ALL {
        let mut ends = planned
            .iter()
            .filter(|planned| planned.kind == kind)
            .map(|planned| match &planned.end {
                End::Existing { identifier, .. } => identifier.as_str(),
                End::Item(index) => batch_identifiers[*index].as_str(),
            })
            .peekable();
        if ends.peek().is_none() {
            continue;
        }
        let separator = if first_kind { "" } else { "; " };
        first_kind = false;
        write!(output, "{separator}{}", kind.field())?;
        for (index, identifier) in ends.enumerate() {
            let separator = if index == 0 { " " } else { ", " };
            write!(
                output,
                "{separator}{}",
                issue_reference(context, identifier)
            )?;
        }
    }
    Ok(())
}
