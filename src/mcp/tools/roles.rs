//! GitHub #87: who works on a project, and what is in it, for the MCP project
//! listing, plus the list parameters (`statuses`, `roles`, `show_members`)
//! that pick what is shown.

use std::fmt::{self, Display};

use crate::db::models;

/// Roles in the order a roster prints them: whoever answers for the project
/// first.
pub(super) const ROSTER_ORDER: [models::Role; 3] = [
    models::Role::Lead,
    models::Role::Maintainer,
    models::Role::Viewer,
];

/// Statuses in the order a project row counts them: work in hand first.
pub(super) const COUNT_ORDER: [models::Status; 5] = [
    models::Status::Active,
    models::Status::Todo,
    models::Status::Backlog,
    models::Status::Done,
    models::Status::Cancelled,
];

/// Names a roster prints per role before summarizing the rest as `+N more`,
/// so a large team doesn't flood every project row.
const ROSTER_NAME_CAP: usize = 5;

/// On a single-person instance the owner answers for every project, including
/// ones created without a lead (CLI, imports). Credit an admin caller without
/// a membership row as lead, so `members: ["me"]` finds their own work.
pub(super) fn credit_solo_owner(
    rosters: &mut std::collections::HashMap<i64, Vec<models::MemberWithUser>>,
    project_ids: impl IntoIterator<Item = i64>,
    caller: Option<&models::AuthUser>,
) {
    let Some(caller) = caller.filter(|caller| caller.is_admin) else {
        return;
    };
    for project_id in project_ids {
        let roster = rosters.entry(project_id).or_default();
        if !roster.iter().any(|member| member.user_id == caller.id) {
            roster.push(models::MemberWithUser {
                project_id,
                user_id: caller.id,
                role: models::Role::Lead,
                created_at: String::new(),
                username: caller.username.clone(),
                display_name: caller.display_name.clone(),
            });
        }
    }
}

/// A list value trimmed and stripped of one pair of wrapping quotes, as the
/// other MCP arguments are (`'alice'` and `"todo"` arrive from some clients).
fn unquoted(raw: &str) -> &str {
    let trimmed = raw.trim();
    super::super::arguments::unquote_if_wrapped(trimmed).unwrap_or(trimmed)
}

/// Parse a list parameter whose values are one of `all` (lowercase wire
/// names), or the keyword `"all"` for every one of them. Values are trimmed and
/// matched case-insensitively; `None` (parameter omitted) stays `None`.
pub(super) fn parse_list<T: Copy + PartialEq + Display>(
    values: Option<&[String]>,
    all: &[T],
    kind: &str,
) -> Result<Option<Vec<T>>, String> {
    let Some(values) = values else {
        return Ok(None);
    };
    let mut parsed = Vec::new();
    for raw in values {
        let value = unquoted(raw).to_ascii_lowercase();
        if value == "all" {
            return Ok(Some(all.to_vec()));
        }
        let Some(found) = all.iter().find(|candidate| candidate.to_string() == value) else {
            let accepted = all.iter().map(ToString::to_string).collect::<Vec<_>>();
            return Err(format!(
                "invalid {kind} '{}'. Use {}, or all.",
                unquoted(raw),
                accepted.join(", ")
            ));
        };
        if !parsed.contains(found) {
            parsed.push(*found);
        }
    }
    Ok(Some(parsed))
}

/// A project row's roster and the caller's own role, appended to the row as
/// ` | lead @alice · maintainers @bob, @carol · 12 viewers | you: maintainer`.
/// By default leads and maintainers are named, up to [`ROSTER_NAME_CAP`] each,
/// and viewers counted.
pub(super) struct ProjectRoster<'a> {
    pub(super) members: &'a [models::MemberWithUser],
    pub(super) caller: Option<&'a models::AuthUser>,
    /// `show_members`: `None` is the default above, `[]` counts every role,
    /// and a listed role is named in full while the others keep the default.
    pub(super) show: Option<&'a [models::Role]>,
}

/// How a roster prints one role's holders.
#[derive(Clone, Copy, PartialEq)]
enum Listing {
    Count,
    Capped,
    Full,
}

impl ProjectRoster<'_> {
    fn listing(&self, role: models::Role) -> Listing {
        let default = match role {
            models::Role::Viewer => Listing::Count,
            models::Role::Lead | models::Role::Maintainer => Listing::Capped,
        };
        match self.show {
            None => default,
            Some([]) => Listing::Count,
            Some(roles) if roles.contains(&role) => Listing::Full,
            Some(_) => default,
        }
    }
}

impl Display for ProjectRoster<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(" | ")?;
        if self.members.is_empty() {
            formatter.write_str("no members")?;
        }
        let mut separator = "";
        for role in ROSTER_ORDER {
            let holders: Vec<&models::MemberWithUser> = self
                .members
                .iter()
                .filter(|member| member.role == role)
                .collect();
            if holders.is_empty() {
                continue;
            }
            let label = match holders.len() {
                1 => role.as_str().to_owned(),
                _ => format!("{role}s"),
            };
            formatter.write_str(separator)?;
            separator = " · ";
            let listing = self.listing(role);
            if listing == Listing::Count {
                write!(formatter, "{} {label}", holders.len())?;
                continue;
            }
            let cap = match listing {
                Listing::Capped => ROSTER_NAME_CAP,
                Listing::Count | Listing::Full => holders.len(),
            };
            write!(formatter, "{label} ")?;
            for (index, member) in holders.iter().take(cap).enumerate() {
                if index > 0 {
                    formatter.write_str(", ")?;
                }
                write!(formatter, "@{}", member.username)?;
            }
            if holders.len() > cap {
                write!(formatter, " +{} more", holders.len() - cap)?;
            }
        }
        let Some(caller) = self.caller else {
            return Ok(());
        };
        match self
            .members
            .iter()
            .find(|member| member.user_id == caller.id)
        {
            Some(member) => write!(formatter, " | you: {}", member.role),
            None if caller.is_admin => formatter.write_str(" | you: admin"),
            None => Ok(()),
        }
    }
}

/// A project row's per-status issue counts, appended as
/// ` | 2 active · 5 todo · 9 backlog`. Zero counts are left out; a project
/// without issues says `no issues`, one whose issues all fall outside
/// `statuses` prints the chosen statuses as `0 active`, and an empty
/// `statuses` prints nothing at all.
pub(super) struct ProjectStatusCounts<'a> {
    pub(super) counts: Option<&'a models::IssueStatusCounts>,
    pub(super) statuses: &'a [models::Status],
}

impl Display for ProjectStatusCounts<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.statuses.is_empty() {
            return Ok(());
        }
        let mut parts = COUNT_ORDER
            .into_iter()
            .filter(|status| self.statuses.contains(status))
            .filter_map(|status| {
                let n = self.counts?.get(status);
                (n > 0).then_some((n, status))
            });
        let Some((n, status)) = parts.next() else {
            // Issues exist, just none in the chosen statuses: say which were
            // counted, so `0 active` is not mistaken for an empty project.
            if self.counts.is_some_and(|counts| counts.total > 0) {
                let mut chosen = COUNT_ORDER
                    .into_iter()
                    .filter(|status| self.statuses.contains(status));
                if let Some(first) = chosen.next() {
                    write!(formatter, " | 0 {first}")?;
                }
                return chosen.try_for_each(|status| write!(formatter, " · 0 {status}"));
            }
            return formatter.write_str(" | no issues");
        };
        write!(formatter, " | {n} {status}")?;
        parts.try_for_each(|(n, status)| write!(formatter, " · {n} {status}"))
    }
}

/// `members` + `roles`: the users whose projects a listing keeps, and the
/// roles that count. `roles` alone means `members: ["me"]`.
pub(super) struct MemberFilter {
    /// `(user id, label)`, where the label is `you` or `@username`.
    users: Vec<(i64, String)>,
    roles: Vec<models::Role>,
}

impl MemberFilter {
    /// `None` when neither parameter was passed: nothing to filter by.
    /// `caller` is the gates' effective user, so a bot's "me" is its owner.
    pub(super) fn parse(
        conn: &rusqlite::Connection,
        members: Option<&[String]>,
        roles: Option<&[String]>,
        caller: Option<&models::AuthUser>,
    ) -> Result<Option<Self>, String> {
        let roles = parse_list(roles, &ROSTER_ORDER, "role")?;
        let me = [String::from("me")];
        let members = match (members, &roles) {
            (Some(members), _) => members,
            (None, Some(_)) => &me[..],
            (None, None) => return Ok(None),
        };
        if members.is_empty() {
            return Err("members must name at least one user, or \"me\"".into());
        }
        let mut users: Vec<(i64, String)> = Vec::new();
        for raw in members {
            let name = unquoted(raw);
            let user = if name.eq_ignore_ascii_case("me") {
                let caller = caller
                    .ok_or_else(|| String::from("members \"me\" needs a caller bound to a user"))?;
                (caller.id, String::from("you"))
            } else if name.eq_ignore_ascii_case("all") {
                return Err("'all' is not allowed in members; name the users, or \"me\"".into());
            } else {
                let (id, username) = crate::db::queries::waits::resolve_user(conn, name)
                    .map_err(super::super::sanitize_error)?;
                (id, format!("@{username}"))
            };
            if !users.iter().any(|(id, _)| *id == user.0) {
                users.push(user);
            }
        }
        Ok(Some(Self {
            users,
            roles: roles.unwrap_or_else(|| ROSTER_ORDER.to_vec()),
        }))
    }

    /// Whether any filtered user holds one of the filtered roles in `roster`.
    pub(super) fn matches(&self, roster: &[models::MemberWithUser]) -> bool {
        roster.iter().any(|member| {
            self.roles.contains(&member.role)
                && self.users.iter().any(|(id, _)| *id == member.user_id)
        })
    }

    /// The reply when no visible project matches.
    pub(super) fn no_projects(&self) -> String {
        format!(
            "No projects where {} a role (among projects you can see).",
            self.subject()
        )
    }

    /// The filtered users' matching roles in `roster`, as `(label, role)`.
    fn holdings<'a>(
        &'a self,
        roster: &'a [models::MemberWithUser],
    ) -> impl Iterator<Item = (&'a str, models::Role)> + 'a {
        roster.iter().filter_map(|member| {
            let (_, label) = self.users.iter().find(|(id, _)| *id == member.user_id)?;
            self.roles
                .contains(&member.role)
                .then_some((label.as_str(), member.role))
        })
    }

    /// `you`, `@bob`, or `you, @bob`, followed by the matching verb.
    pub(super) fn subject(&self) -> String {
        let names = self
            .users
            .iter()
            .map(|(_, label)| label.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let verb = match self.users.as_slice() {
            [(_, label)] if label != "you" => "has",
            _ => "have",
        };
        format!("{names} {verb}")
    }
}

/// The header of a cross-project issue listing: which role the filtered users
/// hold in each listed project, so an agent can weigh the rows by it.
/// `Your roles: APP lead · WEB viewer` when the filter is just the caller,
/// `Roles: APP @bob lead, @carol viewer · WEB @bob maintainer` otherwise.
pub(super) struct RolesHeader<'a> {
    pub(super) filter: &'a MemberFilter,
    /// `(project identifier, roster)` for each listed project, in order.
    pub(super) projects: &'a [(&'a str, &'a [models::MemberWithUser])],
}

impl Display for RolesHeader<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let me_only = matches!(self.filter.users.as_slice(), [(_, label)] if label == "you");
        formatter.write_str(if me_only { "Your roles: " } else { "Roles: " })?;
        for (index, (identifier, roster)) in self.projects.iter().enumerate() {
            if index > 0 {
                formatter.write_str(" · ")?;
            }
            write!(formatter, "{identifier} ")?;
            let holdings: Vec<(&str, models::Role)> = self.filter.holdings(roster).collect();
            for (position, (label, role)) in holdings.iter().take(ROSTER_NAME_CAP).enumerate() {
                if position > 0 {
                    formatter.write_str(", ")?;
                }
                if me_only {
                    write!(formatter, "{role}")?;
                } else {
                    write!(formatter, "{label} {role}")?;
                }
            }
            if holdings.len() > ROSTER_NAME_CAP {
                write!(formatter, " +{} more", holdings.len() - ROSTER_NAME_CAP)?;
            }
        }
        Ok(())
    }
}
