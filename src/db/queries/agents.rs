//! LIF-542: the Agent activity page's aggregates, in one read.
//!
//! An agent is a bot account (`users.is_bot = 1`); a person's own MCP key is
//! a person. Each agent action is one of four kinds:
//!
//! * **read**: a row in `agent_reads` (LIF-541).
//! * **close**: an audit row moving an issue's status to done or cancelled.
//! * **create**: an audit `create` of an issue, page, plan, module, project,
//!   label or folder.
//! * **edit**: every other audit row.
//!
//! One tool call writes several audit rows that share actor, entity and
//! second (an update touching three fields is three rows), so audit rows are
//! collapsed to distinct `(actor, entity_type, entity_id, ts, kind)` before
//! they are counted.
//!
//! Buckets follow the caller's local clock: the client sends its offset from
//! UTC and bucket boundaries are aligned in that zone, oldest first, the last
//! one holding `now`. Visibility follows `/api/home/overview`, with one
//! addition: rows that name no project (workspace search, project lists) are
//! shown to unrestricted callers and to the owner of the bot that made them.
//!
//! Every query is bounded by the 30-day roster horizon or the (at most
//! 8-day) window and runs off the `ts` indexes.

use std::collections::{HashMap, HashSet};

use chrono::{Duration, NaiveDateTime, Timelike};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::error::LificError;

/// Widest real UTC offset is +14:00, so this bounds a client's `tz`.
const MAX_TZ_MINUTES: i32 = 14 * 60;
/// How far back an agent's last action keeps it on the roster.
pub const ROSTER_DAYS: i64 = 30;
/// Most rows in `recent`.
pub const RECENT_LIMIT: usize = 30;
/// Last action under this many minutes ago: active.
const ACTIVE_MINUTES: i64 = 10;
/// Last action under this many minutes ago: idle. Older: inactive.
const IDLE_MINUTES: i64 = 60;

const TS_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

/// The audit-row classification, as SQL over an `audit_log a`.
const KIND_SQL: &str = "CASE
        WHEN a.entity_type = 'issue' AND a.field = 'status'
             AND a.new_value IN ('done', 'cancelled') THEN 'close'
        WHEN a.action = 'create'
             AND a.entity_type IN ('issue', 'page', 'plan', 'module', 'project', 'label', 'folder')
             THEN 'create'
        ELSE 'edit'
    END";

/// The page's three windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    Hour,
    Day,
    Week,
}

impl Window {
    /// `1h`, `24h` or `7d`; anything else is a bad request.
    pub fn parse(window: Option<&str>) -> Result<Self, LificError> {
        match window.unwrap_or("1h") {
            "1h" => Ok(Self::Hour),
            "24h" => Ok(Self::Day),
            "7d" => Ok(Self::Week),
            other => Err(LificError::BadRequest(format!(
                "invalid window '{other}'; use 1h, 24h or 7d"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Hour => "1h",
            Self::Day => "24h",
            Self::Week => "7d",
        }
    }

    fn buckets(self) -> usize {
        match self {
            Self::Hour => 12,
            Self::Day => 24,
            Self::Week => 7,
        }
    }

    fn bucket_seconds(self) -> i64 {
        match self {
            Self::Hour => 300,
            Self::Day => 3600,
            Self::Week => 86_400,
        }
    }

    /// Start of the local bucket holding `local`.
    fn floor(self, local: NaiveDateTime) -> NaiveDateTime {
        let date = local.date();
        match self {
            Self::Hour => date
                .and_hms_opt(local.hour(), local.minute() - local.minute() % 5, 0)
                .unwrap(),
            Self::Day => date.and_hms_opt(local.hour(), 0, 0).unwrap(),
            Self::Week => date.and_hms_opt(0, 0, 0).unwrap(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    Read,
    Edit,
    Create,
    Close,
}

impl Kind {
    fn parse(kind: &str) -> Self {
        match kind {
            "close" => Self::Close,
            "create" => Self::Create,
            "read" => Self::Read,
            _ => Self::Edit,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Edit => "edit",
            Self::Create => "create",
            Self::Close => "close",
        }
    }
}

#[derive(Debug, Default, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct Counts {
    pub read: i64,
    pub edit: i64,
    pub create: i64,
    pub close: i64,
}

impl Counts {
    fn add(&mut self, kind: Kind, n: i64) {
        match kind {
            Kind::Read => self.read += n,
            Kind::Edit => self.edit += n,
            Kind::Create => self.create += n,
            Kind::Close => self.close += n,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct AgentActivity {
    pub window: &'static str,
    /// Server time, RFC 3339 UTC.
    pub now: String,
    pub bucket_seconds: i64,
    /// Oldest first.
    pub buckets: Vec<Bucket>,
    pub totals: Counts,
    /// Agents whose status is `active`.
    pub active_count: usize,
    pub agents: Vec<AgentSummary>,
    /// Newest first, at most [`RECENT_LIMIT`].
    pub recent: Vec<RecentAction>,
}

#[derive(Debug, Serialize)]
pub struct Bucket {
    /// RFC 3339 UTC.
    pub start: String,
    #[serde(flatten)]
    pub counts: Counts,
}

#[derive(Debug, Serialize)]
pub struct AgentSummary {
    pub user_id: i64,
    pub username: String,
    pub display_name: String,
    pub owner: Option<Owner>,
    /// `active`, `idle` or `inactive`, from `last_seen` alone.
    pub status: &'static str,
    /// Most recent visible action within the roster horizon, RFC 3339 UTC.
    pub last_seen: Option<String>,
    pub counts: Counts,
    /// Total actions per bucket.
    pub series: Vec<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Owner {
    pub user_id: i64,
    pub username: String,
    pub display_name: String,
}

#[derive(Debug, Serialize)]
pub struct RecentAction {
    pub at: String,
    pub kind: &'static str,
    pub agent: AgentRef,
    pub entity_type: String,
    pub identifier: Option<String>,
    pub title: Option<String>,
    pub detail: Option<String>,
    /// App hash route without `#`, or `None` when the entity is gone.
    pub route: Option<String>,
    /// Reads only: the MCP tool.
    pub tool: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AgentRef {
    pub user_id: i64,
    pub display_name: String,
}

pub struct ActivityInput<'a> {
    /// The caller's project filter (`None` = every project).
    pub visible: Option<&'a HashSet<i64>>,
    /// The caller as the gates see them (a bot's owner), for owned bots and
    /// project-less rows.
    pub caller_id: Option<i64>,
    pub window: Window,
    /// The caller's offset from UTC in minutes, east positive.
    pub tz_minutes: i32,
    /// Current UTC time, injected so tests can pin it.
    pub now: NaiveDateTime,
}

struct Bot {
    username: String,
    display_name: String,
    owner_id: Option<i64>,
    is_active: bool,
    owner: Option<Owner>,
}

/// Stored timestamps are `YYYY-MM-DD HH:MM:SS` (UTC), sometimes with
/// fractional seconds.
fn parse_ts(ts: &str) -> Option<NaiveDateTime> {
    let head = ts.get(..19).unwrap_or(ts).replace('T', " ");
    NaiveDateTime::parse_from_str(&head, TS_FORMAT).ok()
}

fn rfc3339(at: NaiveDateTime) -> String {
    at.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn stored_to_rfc3339(ts: &str) -> String {
    parse_ts(ts).map_or_else(|| ts.to_string(), rfc3339)
}

pub fn activity(conn: &Connection, input: &ActivityInput) -> Result<AgentActivity, LificError> {
    let window = input.window;
    let tz = Duration::minutes(input.tz_minutes.clamp(-MAX_TZ_MINUTES, MAX_TZ_MINUTES) as i64);
    let now = input.now.with_nanosecond(0).unwrap_or(input.now);
    let size = window.bucket_seconds();
    let count = window.buckets();
    let last_local = window.floor(now + tz);
    let first = last_local - Duration::seconds(size * (count as i64 - 1)) - tz;
    let start_text = first.format(TS_FORMAT).to_string();
    let now_text = now.format(TS_FORMAT).to_string();
    let horizon_text = (now - Duration::days(ROSTER_DAYS))
        .format(TS_FORMAT)
        .to_string();
    let start_epoch = first.and_utc().timestamp();

    let bots = bots(conn)?;
    let visible = |project_id: Option<i64>, actor: i64| match (input.visible, project_id) {
        (None, _) => true,
        (Some(set), Some(pid)) => set.contains(&pid),
        (Some(_), None) => {
            input.caller_id.is_some()
                && bots.get(&actor).and_then(|bot| bot.owner_id) == input.caller_id
        }
    };

    // Last visible action per bot, within the roster horizon.
    let mut last_seen: HashMap<i64, String> = HashMap::new();
    for sql in [
        "SELECT a.actor_user_id, a.project_id, MAX(a.ts) FROM audit_log a
           JOIN users u ON u.id = a.actor_user_id AND u.is_bot = 1
          WHERE a.ts >= ?1 AND a.ts <= ?2
          GROUP BY a.actor_user_id, a.project_id",
        "SELECT r.user_id, r.project_id, MAX(r.ts) FROM agent_reads r
           JOIN users u ON u.id = r.user_id AND u.is_bot = 1
          WHERE r.ts >= ?1 AND r.ts <= ?2
          GROUP BY r.user_id, r.project_id",
    ] {
        let mut stmt = conn.prepare_cached(sql)?;
        let rows = stmt.query_map(params![horizon_text, now_text], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (actor, project_id, ts) = row?;
            if !visible(project_id, actor) {
                continue;
            }
            let entry = last_seen.entry(actor).or_default();
            if ts > *entry {
                *entry = ts;
            }
        }
    }

    // Per bot, per bucket, per kind, inside the window.
    let mut per_agent: HashMap<i64, Vec<Counts>> = HashMap::new();
    let audit_sql = format!(
        "SELECT actor_user_id, project_id, bucket, kind, COUNT(*) FROM (
             SELECT DISTINCT a.actor_user_id, a.project_id, a.entity_type, a.entity_id, a.ts,
                    (CAST(strftime('%s', a.ts) AS INTEGER) - ?3) / ?4 AS bucket,
                    {KIND_SQL} AS kind
               FROM audit_log a
               JOIN users u ON u.id = a.actor_user_id AND u.is_bot = 1
              WHERE a.ts >= ?1 AND a.ts <= ?2
         )
         GROUP BY actor_user_id, project_id, bucket, kind"
    );
    let reads_sql = "SELECT r.user_id, r.project_id,
                            (CAST(strftime('%s', r.ts) AS INTEGER) - ?3) / ?4 AS bucket,
                            'read', COUNT(*)
                       FROM agent_reads r
                       JOIN users u ON u.id = r.user_id AND u.is_bot = 1
                      WHERE r.ts >= ?1 AND r.ts <= ?2
                      GROUP BY r.user_id, r.project_id, bucket";
    for sql in [audit_sql.as_str(), reads_sql] {
        let mut stmt = conn.prepare_cached(sql)?;
        let rows = stmt.query_map(params![start_text, now_text, start_epoch, size], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?;
        for row in rows {
            let (actor, project_id, bucket, kind, n) = row?;
            if !visible(project_id, actor) {
                continue;
            }
            let Some(bucket) = bucket else { continue };
            let index = usize::try_from(bucket).unwrap_or(0).min(count - 1);
            per_agent
                .entry(actor)
                .or_insert_with(|| vec![Counts::default(); count])[index]
                .add(Kind::parse(&kind), n);
        }
    }

    let mut buckets: Vec<Bucket> = (0..count)
        .map(|i| Bucket {
            start: rfc3339(first + Duration::seconds(size * i as i64)),
            counts: Counts::default(),
        })
        .collect();
    let mut totals = Counts::default();
    for series in per_agent.values() {
        for (bucket, counts) in buckets.iter_mut().zip(series) {
            for kind in [Kind::Read, Kind::Edit, Kind::Create, Kind::Close] {
                let n = kind_count(counts, kind);
                bucket.counts.add(kind, n);
                totals.add(kind, n);
            }
        }
    }

    let mut agents: Vec<AgentSummary> = bots
        .iter()
        .filter(|(id, bot)| {
            last_seen.contains_key(id)
                || (bot.is_active && input.caller_id.is_some() && bot.owner_id == input.caller_id)
        })
        .map(|(&id, bot)| {
            let seen = last_seen.get(&id).and_then(|ts| parse_ts(ts));
            let series = per_agent.get(&id);
            let mut counts = Counts::default();
            for bucket in series.into_iter().flatten() {
                for kind in [Kind::Read, Kind::Edit, Kind::Create, Kind::Close] {
                    counts.add(kind, kind_count(bucket, kind));
                }
            }
            AgentSummary {
                user_id: id,
                username: bot.username.clone(),
                display_name: bot.display_name.clone(),
                owner: bot.owner.clone(),
                status: status(seen, now),
                last_seen: seen.map(rfc3339),
                counts,
                series: series.map_or_else(
                    || vec![0; count],
                    |series| {
                        series
                            .iter()
                            .map(|c| c.read + c.edit + c.create + c.close)
                            .collect()
                    },
                ),
            }
        })
        .collect();
    agents.sort_by(|a, b| {
        status_rank(a.status)
            .cmp(&status_rank(b.status))
            .then_with(|| b.last_seen.cmp(&a.last_seen))
            .then_with(|| a.display_name.cmp(&b.display_name))
            .then_with(|| a.user_id.cmp(&b.user_id))
    });
    let active_count = agents.iter().filter(|a| a.status == "active").count();

    let recent = recent(conn, &bots, &visible, &start_text, &now_text)?;

    Ok(AgentActivity {
        window: window.as_str(),
        now: rfc3339(now),
        bucket_seconds: size,
        buckets,
        totals,
        active_count,
        agents,
        recent,
    })
}

fn kind_count(counts: &Counts, kind: Kind) -> i64 {
    match kind {
        Kind::Read => counts.read,
        Kind::Edit => counts.edit,
        Kind::Create => counts.create,
        Kind::Close => counts.close,
    }
}

fn status(last_seen: Option<NaiveDateTime>, now: NaiveDateTime) -> &'static str {
    match last_seen.map(|seen| now - seen) {
        Some(age) if age < Duration::minutes(ACTIVE_MINUTES) => "active",
        Some(age) if age < Duration::minutes(IDLE_MINUTES) => "idle",
        _ => "inactive",
    }
}

fn status_rank(status: &str) -> u8 {
    match status {
        "active" => 0,
        "idle" => 1,
        _ => 2,
    }
}

fn bots(conn: &Connection) -> Result<HashMap<i64, Bot>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT b.id, b.username, b.display_name, b.owner_id, b.is_active,
                o.id, o.username, o.display_name
           FROM users b LEFT JOIN users o ON o.id = b.owner_id
          WHERE b.is_bot = 1",
    )?;
    let rows = stmt.query_map([], |row| {
        let owner = match row.get::<_, Option<i64>>(5)? {
            Some(user_id) => Some(Owner {
                user_id,
                username: row.get(6)?,
                display_name: row.get(7)?,
            }),
            None => None,
        };
        Ok((
            row.get::<_, i64>(0)?,
            Bot {
                username: row.get(1)?,
                display_name: row.get(2)?,
                owner_id: row.get(3)?,
                is_active: row.get(4)?,
                owner,
            },
        ))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// A recent action before its entity is looked up.
struct RawRecent {
    ts: String,
    kind: Kind,
    actor: i64,
    entity_type: String,
    entity_id: i64,
    label: Option<String>,
    project_id: Option<i64>,
    issue_id: Option<i64>,
    page_id: Option<i64>,
    action: Option<String>,
    field: Option<String>,
    old_value: Option<String>,
    new_value: Option<String>,
    tool: Option<String>,
}

fn recent(
    conn: &Connection,
    bots: &HashMap<i64, Bot>,
    visible: &dyn Fn(Option<i64>, i64) -> bool,
    start: &str,
    now: &str,
) -> Result<Vec<RecentAction>, LificError> {
    let mut raw: Vec<RawRecent> = Vec::new();

    // Audit actions, newest first, collapsed exactly as they are counted.
    let audit_sql = format!(
        "SELECT a.actor_user_id, a.ts, a.entity_type, a.entity_id, a.entity_label,
                a.project_id, a.issue_id, a.page_id, a.action, a.field,
                a.old_value, a.new_value, {KIND_SQL}
           FROM audit_log a
           JOIN users u ON u.id = a.actor_user_id AND u.is_bot = 1
          WHERE a.ts >= ?1 AND a.ts <= ?2
          ORDER BY a.ts DESC, a.id DESC"
    );
    let mut stmt = conn.prepare_cached(&audit_sql)?;
    let mut rows = stmt.query(params![start, now])?;
    let mut seen: HashSet<(i64, String, i64, String, Kind)> = HashSet::new();
    let mut audit = 0;
    while audit < RECENT_LIMIT {
        let Some(row) = rows.next()? else { break };
        let actor: i64 = row.get(0)?;
        let project_id: Option<i64> = row.get(5)?;
        if !visible(project_id, actor) {
            continue;
        }
        let ts: String = row.get(1)?;
        let entity_type: String = row.get(2)?;
        let entity_id: i64 = row.get(3)?;
        let kind = Kind::parse(&row.get::<_, String>(12)?);
        if !seen.insert((actor, entity_type.clone(), entity_id, ts.clone(), kind)) {
            continue;
        }
        raw.push(RawRecent {
            ts,
            kind,
            actor,
            entity_type,
            entity_id,
            label: row.get(4)?,
            project_id,
            issue_id: row.get(6)?,
            page_id: row.get(7)?,
            action: row.get(8)?,
            field: row.get(9)?,
            old_value: row.get(10)?,
            new_value: row.get(11)?,
            tool: None,
        });
        audit += 1;
    }
    drop(rows);

    // Reads of a single entity; list and search reads are only counted.
    let mut stmt = conn.prepare_cached(
        "SELECT r.user_id, r.ts, r.entity_type, r.entity_id, r.entity_label,
                r.project_id, r.tool
           FROM agent_reads r
           JOIN users u ON u.id = r.user_id AND u.is_bot = 1
          WHERE r.ts >= ?1 AND r.ts <= ?2
            AND r.entity_type IS NOT NULL AND r.entity_id IS NOT NULL
          ORDER BY r.ts DESC, r.id DESC",
    )?;
    let mut rows = stmt.query(params![start, now])?;
    let mut reads = 0;
    while reads < RECENT_LIMIT {
        let Some(row) = rows.next()? else { break };
        let actor: i64 = row.get(0)?;
        let project_id: Option<i64> = row.get(5)?;
        if !visible(project_id, actor) {
            continue;
        }
        raw.push(RawRecent {
            ts: row.get(1)?,
            kind: Kind::Read,
            actor,
            entity_type: row.get(2)?,
            entity_id: row.get(3)?,
            label: row.get(4)?,
            project_id,
            issue_id: None,
            page_id: None,
            action: None,
            field: None,
            old_value: None,
            new_value: None,
            tool: row.get(6)?,
        });
        reads += 1;
    }
    drop(rows);

    // Stable, so equal timestamps keep audit actions ahead of reads.
    raw.sort_by(|a, b| b.ts.cmp(&a.ts));
    raw.truncate(RECENT_LIMIT);
    raw.into_iter()
        .map(|item| describe(conn, bots, item))
        .collect()
}

/// What a recent row is about, before the current entity is looked up.
struct Target {
    entity_type: String,
    entity_id: Option<i64>,
    detail: Option<String>,
    /// Identifier from the audit snapshot, used when the entity is gone.
    fallback_identifier: Option<String>,
    /// Title from the audit snapshot, used when the entity is gone.
    fallback_title: Option<String>,
}

fn target(conn: &Connection, item: &RawRecent) -> Result<Target, LificError> {
    if item.kind == Kind::Read {
        return Ok(Target {
            entity_type: item.entity_type.clone(),
            entity_id: Some(item.entity_id),
            detail: None,
            fallback_identifier: match item.entity_type.as_str() {
                "attachment" => None,
                _ => item.label.clone(),
            },
            fallback_title: match item.entity_type.as_str() {
                "attachment" => item.label.clone(),
                _ => None,
            },
        });
    }
    let action = item.action.as_deref().unwrap_or_default();
    Ok(match item.entity_type.as_str() {
        // Comments belong to their issue or page.
        "comment" => {
            let (entity_type, entity_id) = match (item.issue_id, item.page_id) {
                (Some(id), _) => ("issue", Some(id)),
                (None, Some(id)) => ("page", Some(id)),
                (None, None) => ("issue", None),
            };
            Target {
                entity_type: entity_type.into(),
                entity_id,
                detail: Some("comment".into()),
                fallback_identifier: item.label.clone(),
                fallback_title: None,
            }
        }
        // Plan steps belong to their plan; the row's label is the plan's.
        "plan_step" => {
            let plan_id = conn
                .query_row(
                    "SELECT plan_id FROM plan_steps WHERE id = ?1",
                    params![item.entity_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?;
            let plan_id = match plan_id {
                Some(id) => Some(id),
                None => match item.label.as_deref() {
                    Some(label) => super::plans::resolve_plan_identifier(conn, label).ok(),
                    None => None,
                },
            };
            Target {
                entity_type: "plan".into(),
                entity_id: plan_id,
                detail: Some("step".into()),
                fallback_identifier: item.label.clone(),
                fallback_title: None,
            }
        }
        "issue" | "page" | "plan" | "module" | "project" | "label" | "folder" => {
            let detail = match action {
                "create" => None,
                "update" => item.field.clone(),
                "link" | "unlink" => Some("link".into()),
                "attach" | "detach" => Some(item.field.clone().unwrap_or_else(|| "label".into())),
                "wait" | "unwait" => Some("wait".into()),
                "assign" | "unassign" => Some("assignee".into()),
                other => Some(other.to_string()),
            };
            let named = matches!(item.entity_type.as_str(), "module" | "label" | "folder");
            let fallback_title = match (action, item.field.as_deref()) {
                ("create", _) => item.new_value.clone(),
                ("delete", _) => item.old_value.clone(),
                ("update", Some("title" | "name")) => item.new_value.clone(),
                _ if named => item.label.clone(),
                _ => None,
            };
            Target {
                entity_type: item.entity_type.clone(),
                entity_id: Some(item.entity_id),
                detail,
                fallback_identifier: if named { None } else { item.label.clone() },
                fallback_title,
            }
        }
        // Membership, repository bindings and the like: report against the
        // project they changed, named by what changed.
        other => match item.project_id {
            Some(project_id) => Target {
                entity_type: "project".into(),
                entity_id: Some(project_id),
                detail: Some(other.to_string()),
                fallback_identifier: None,
                fallback_title: None,
            },
            None => Target {
                entity_type: other.to_string(),
                entity_id: None,
                detail: Some(action.to_string()).filter(|a| !a.is_empty()),
                fallback_identifier: None,
                fallback_title: item.label.clone(),
            },
        },
    })
}

/// The entity as it stands now.
struct Current {
    identifier: Option<String>,
    title: String,
    route: Option<String>,
}

/// Routes mirror `crate::links`' resource paths, without the base URL.
fn current(
    conn: &Connection,
    entity_type: &str,
    entity_id: i64,
) -> Result<Option<Current>, LificError> {
    let found = match entity_type {
        "issue" => conn
            .query_row(
                "SELECT i.title, p.identifier || '-' || i.sequence, p.identifier,
                        i.deleted_at IS NULL
                   FROM issues i JOIN projects p ON p.id = i.project_id
                  WHERE i.id = ?1",
                params![entity_id],
                |row| {
                    let identifier: String = row.get(1)?;
                    let project: String = row.get(2)?;
                    let live: bool = row.get(3)?;
                    Ok(Current {
                        route: live.then(|| format!("/{project}/issues/{identifier}")),
                        identifier: Some(identifier),
                        title: row.get(0)?,
                    })
                },
            )
            .optional()?,
        "page" => conn
            .query_row(
                "SELECT pg.title, p.identifier, pg.sequence, pg.deleted_at IS NULL
                   FROM pages pg LEFT JOIN projects p ON p.id = pg.project_id
                  WHERE pg.id = ?1",
                params![entity_id],
                |row| {
                    let project: Option<String> = row.get(1)?;
                    let sequence: i64 = row.get(2)?;
                    let live: bool = row.get(3)?;
                    Ok(Current {
                        identifier: Some(match &project {
                            Some(project) => format!("{project}-DOC-{sequence}"),
                            None => format!("DOC-{sequence}"),
                        }),
                        // Workspace pages have no project route.
                        route: project
                            .filter(|_| live)
                            .map(|project| format!("/{project}/pages/{entity_id}")),
                        title: row.get(0)?,
                    })
                },
            )
            .optional()?,
        "plan" => conn
            .query_row(
                "SELECT pl.title, p.identifier, pl.sequence
                   FROM plans pl JOIN projects p ON p.id = pl.project_id
                  WHERE pl.id = ?1",
                params![entity_id],
                |row| {
                    let project: String = row.get(1)?;
                    let sequence: i64 = row.get(2)?;
                    Ok(Current {
                        identifier: Some(format!("{project}-PLAN-{sequence}")),
                        route: Some(format!("/{project}/plans/{entity_id}")),
                        title: row.get(0)?,
                    })
                },
            )
            .optional()?,
        "module" => conn
            .query_row(
                "SELECT m.name, p.identifier
                   FROM modules m JOIN projects p ON p.id = m.project_id
                  WHERE m.id = ?1",
                params![entity_id],
                |row| {
                    let project: String = row.get(1)?;
                    Ok(Current {
                        identifier: None,
                        route: Some(format!("/{project}/modules/{entity_id}")),
                        title: row.get(0)?,
                    })
                },
            )
            .optional()?,
        "project" => conn
            .query_row(
                "SELECT name, identifier FROM projects WHERE id = ?1",
                params![entity_id],
                |row| {
                    let project: String = row.get(1)?;
                    Ok(Current {
                        route: Some(format!("/{project}/overview")),
                        identifier: Some(project),
                        title: row.get(0)?,
                    })
                },
            )
            .optional()?,
        "label" | "folder" | "attachment" => {
            let sql = match entity_type {
                "label" => "SELECT name FROM labels WHERE id = ?1",
                "folder" => "SELECT name FROM folders WHERE id = ?1",
                _ => "SELECT filename FROM attachments WHERE id = ?1",
            };
            conn.query_row(sql, params![entity_id], |row| {
                Ok(Current {
                    identifier: None,
                    route: None,
                    title: row.get(0)?,
                })
            })
            .optional()?
        }
        _ => None,
    };
    Ok(found)
}

fn describe(
    conn: &Connection,
    bots: &HashMap<i64, Bot>,
    item: RawRecent,
) -> Result<RecentAction, LificError> {
    let target = target(conn, &item)?;
    let now = match target.entity_id {
        Some(id) => current(conn, &target.entity_type, id)?,
        None => None,
    };
    let (identifier, title, route) = match now {
        Some(current) => (
            current.identifier.or(target.fallback_identifier),
            Some(current.title),
            current.route,
        ),
        None => (target.fallback_identifier, target.fallback_title, None),
    };
    Ok(RecentAction {
        at: stored_to_rfc3339(&item.ts),
        kind: item.kind.as_str(),
        agent: AgentRef {
            user_id: item.actor,
            display_name: bots
                .get(&item.actor)
                .map(|bot| bot.display_name.clone())
                .unwrap_or_default(),
        },
        entity_type: target.entity_type,
        identifier,
        title,
        detail: target.detail,
        route,
        tool: item.tool,
    })
}

#[cfg(test)]
mod tests;
