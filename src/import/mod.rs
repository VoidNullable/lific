//! Issue import (LIF-264 / LIF-265).
//!
//! Pulls issues from external trackers — GitHub, Linear, Jira — into a Lific
//! project. Three sources share one spine so behavior is uniform:
//!
//! - **Idempotency.** Every imported issue records a stable `source` marker
//!   (`github:owner/name#12`, `linear:ENG-3`, `jira:site:KEY-7`). A partial
//!   unique index (`migrations/033_import_source.sql`) makes re-running a
//!   no-op: an issue whose source already exists is skipped, so an interrupted
//!   import resumes cleanly and a completed one never duplicates.
//! - **Provenance.** Comments are attributed to a dedicated *import bot*
//!   identity (created exactly like `lific connect` bots, owned by a human) so
//!   the audit log shows the import as its own actor. The original author +
//!   timestamp are preserved in a body prefix.
//! - **Labels.** Missing labels are created, preserving the source's color
//!   where the API provides one.
//! - **Dry run.** Counting is separated from writing: a dry run reports how
//!   many issues / comments / labels *would* be created and touches nothing.
//!
//! ## Fetch abstraction
//!
//! Network I/O is behind traits ([`github::GithubFetcher`],
//! [`linear::LinearFetcher`], [`jira::JiraFetcher`]) so the mapping logic —
//! the interesting, bug-prone part — is pure and unit-tested against fixture
//! JSON with no live network. The live implementations wrap the blocking
//! `reqwest` client already in the dependency tree (used by `lific login` /
//! `doctor`); no new HTTP stack is added.
//!
//! ## Module layout
//!
//! - this module — shared core: import bot, label ensure-with-color, the
//!   source-marker dedupe insert, [`ImportSummary`], and the generic
//!   [`apply_issue`] that turns a normalized [`NormalizedIssue`] into DB rows.
//! - [`github`] / [`linear`] / [`jira`] — per-source fetch + map to
//!   [`NormalizedIssue`].

pub mod github;

use crate::db::DbPool;
use crate::db::models::{CreateIssue, CreateLabel};
use crate::db::queries;
use crate::error::LificError;

/// A source-agnostic issue ready to be written into Lific. Every importer maps
/// its native representation to this shape; the shared core does the rest.
#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedIssue {
    /// Stable idempotency marker, e.g. `github:owner/name#12`.
    pub source: String,
    pub title: String,
    /// Markdown body (already converted for Jira's ADF).
    pub description: String,
    /// Mapped Lific status: backlog / todo / active / done / cancelled.
    pub status: String,
    /// Mapped Lific priority: urgent / high / medium / low / none.
    pub priority: String,
    /// Labels to attach, with the color the source reported (hex like
    /// `#EF4444`) or `None` to use the Lific default.
    pub labels: Vec<NormalizedLabel>,
    /// Comments to import, in chronological order.
    pub comments: Vec<NormalizedComment>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedLabel {
    pub name: String,
    /// Hex color (`#RRGGBB`) if the source provided one.
    pub color: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedComment {
    /// Original author handle/name (without `@`).
    pub author: String,
    /// ISO-8601 timestamp of the original comment, if known.
    pub created_at: Option<String>,
    /// Comment body markdown.
    pub body: String,
}

impl NormalizedComment {
    /// Render the stored comment body with a provenance prefix so the original
    /// author + time survive the import (the DB comment is authored by the
    /// import bot). Shape: `Originally by @alice on 2024-01-02:\n\n<body>`.
    pub fn body_with_attribution(&self) -> String {
        let when = self
            .created_at
            .as_deref()
            .map(|t| format!(" on {t}"))
            .unwrap_or_default();
        format!("_Originally by @{}{}_\n\n{}", self.author, when, self.body)
    }
}

/// What a source produced after normalization but before any DB writes. Lets
/// the summary count labels/comments the same way whether we're doing a dry
/// run or a real import.
#[derive(Debug, Default, Clone)]
pub struct FetchedIssues {
    pub issues: Vec<NormalizedIssue>,
    /// Count of entries the source returned that were skipped before
    /// normalization (PRs on GitHub's issues endpoint, etc.).
    pub skipped_non_issues: usize,
    /// Assignees seen and skipped (we don't map external users).
    pub skipped_assignees: usize,
    /// Other skipped facets (milestone/sprint/epic/estimate/…), counted for
    /// the summary so the operator knows what didn't come across.
    pub skipped_other: usize,
}

/// Outcome of an import (or a dry-run preview).
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct ImportSummary {
    /// True if nothing was written.
    pub dry_run: bool,
    /// Issues newly created this run.
    pub issues_created: usize,
    /// Issues skipped because their `source` already existed (idempotency).
    pub issues_skipped_existing: usize,
    /// Comments created (0 in a dry run — reported as `comments_planned`).
    pub comments_created: usize,
    /// Labels created (0 in a dry run — reported as `labels_planned`).
    pub labels_created: usize,
    /// For a dry run: comments that would be created.
    pub comments_planned: usize,
    /// For a dry run: distinct new labels that would be created.
    pub labels_planned: usize,
    /// Non-issue rows filtered out (GitHub PRs).
    pub skipped_non_issues: usize,
    /// Assignees dropped (we don't map external identities).
    pub skipped_assignees: usize,
    /// Milestone/sprint/epic/estimate/etc. dropped.
    pub skipped_other: usize,
}

/// Find-or-create the import bot for a source, owned by a human user, mirroring
/// `lific connect`'s bot-identity convention (`src/cli/connect/mod.rs`): the
/// bot's `owner_id` points at a human so authz resolves bot → owner under both
/// legacy and enforced modes. Bot username is `import-{source}-{owner}`.
///
/// Returns the bot user id, used as the comment author for provenance.
pub fn ensure_import_bot(
    pool: &DbPool,
    owner_id: i64,
    source_slug: &str,
    display: &str,
) -> Result<i64, LificError> {
    let owner_username = {
        let conn = pool.read()?;
        queries::users::get_user_by_id(&conn, owner_id)?.username
    };
    let bot_username = format!("import-{source_slug}-{owner_username}");
    let conn = pool.write()?;
    if let Some(existing) = queries::users::find_bot_by_username(&conn, &bot_username)? {
        return Ok(existing.id);
    }
    let bot = queries::users::create_bot_user(&conn, owner_id, &bot_username, display)?;
    Ok(bot.id)
}

/// Resolve the human who will own the import bot: explicit username, else the
/// sole human, else the sole admin — else an error asking for `--user`. Same
/// policy as `connect`'s `choose_owner`. Returns `None` only on a truly fresh
/// install (zero humans), where comments are attributed to the first admin if
/// one exists, or skipped entirely.
pub fn resolve_owner(pool: &DbPool, requested: Option<&str>) -> Result<Option<i64>, LificError> {
    let conn = pool.read()?;
    if let Some(username) = requested {
        let u = queries::users::get_user_by_username(&conn, username)?;
        return Ok(Some(u.id));
    }
    let users = queries::users::list_users(&conn)?;
    let humans: Vec<_> = users.iter().filter(|u| !u.is_bot).collect();
    match humans.len() {
        0 => Ok(None),
        1 => Ok(Some(humans[0].id)),
        _ => {
            let admins: Vec<_> = humans.iter().filter(|u| u.is_admin).collect();
            if admins.len() == 1 {
                Ok(Some(admins[0].id))
            } else {
                Err(LificError::BadRequest(
                    "multiple users exist — pass --user <username> to choose who owns the \
                     import bot (imported comments are attributed to it)."
                        .into(),
                ))
            }
        }
    }
}

/// True if an issue with this `source` marker already exists (idempotency).
pub fn source_exists(conn: &rusqlite::Connection, source: &str) -> Result<bool, LificError> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM issues WHERE source = ?1)",
        rusqlite::params![source],
        |row| row.get(0),
    )?;
    Ok(exists)
}

/// Ensure a label exists in the project, creating it with the given color when
/// missing. Returns `true` if a new label was created. An existing label's
/// color is left untouched (the operator may have customized it).
pub fn ensure_label(
    conn: &rusqlite::Connection,
    project_id: i64,
    label: &NormalizedLabel,
) -> Result<bool, LificError> {
    if queries::resolve_label_name(conn, project_id, &label.name).is_ok() {
        return Ok(false);
    }
    let color = label
        .color
        .clone()
        .unwrap_or_else(|| "#6B7280".to_string());
    queries::create_label(
        conn,
        &CreateLabel {
            project_id,
            name: label.name.clone(),
            color,
        },
    )?;
    Ok(true)
}

/// The write side: import one normalized issue into `project_id`, attributing
/// comments to `bot_id` (when present). Idempotent — returns `Skipped` if the
/// source marker already exists. Non-dry-run only; callers gate on dry_run.
pub enum ApplyOutcome {
    /// Newly created; carries how many comments/labels were created.
    Created {
        comments_created: usize,
        labels_created: usize,
    },
    /// Already imported (source marker present); nothing written.
    Skipped,
}

/// Apply one normalized issue. Ensures labels first (so the issue's label
/// attach resolves), inserts the issue with its source marker, then creates
/// each comment authored by the import bot. Wrapped in a savepoint so a partial
/// failure doesn't leave a half-imported issue.
pub fn apply_issue(
    pool: &DbPool,
    project_id: i64,
    bot_id: Option<i64>,
    issue: &NormalizedIssue,
) -> Result<ApplyOutcome, LificError> {
    let conn = pool.write()?;
    if source_exists(&conn, &issue.source)? {
        return Ok(ApplyOutcome::Skipped);
    }

    let mut labels_created = 0usize;
    let mut comments_created = 0usize;

    queries::savepoint(&conn, "import_issue", || {
        for label in &issue.labels {
            if ensure_label(&conn, project_id, label)? {
                labels_created += 1;
            }
        }

        let created = queries::create_issue(
            &conn,
            &CreateIssue {
                project_id,
                title: issue.title.clone(),
                description: issue.description.clone(),
                status: issue.status.clone(),
                priority: issue.priority.clone(),
                module_id: None,
                start_date: None,
                target_date: None,
                labels: issue.labels.iter().map(|l| l.name.clone()).collect(),
                source: Some(issue.source.clone()),
            },
        )?;

        if let Some(bot) = bot_id {
            for comment in &issue.comments {
                queries::comments::create_comment(
                    &conn,
                    queries::comments::CommentParent::Issue(created.id),
                    bot,
                    &comment.body_with_attribution(),
                )?;
                comments_created += 1;
            }
        }
        Ok(())
    })?;

    Ok(ApplyOutcome::Created {
        comments_created,
        labels_created,
    })
}

/// Build the full summary for a set of fetched issues, either applying them
/// (dry_run = false) or just counting (dry_run = true). Shared by CLI + web so
/// a preview and a real run report the same shape.
pub fn run_import(
    pool: &DbPool,
    project_id: i64,
    bot_id: Option<i64>,
    fetched: &FetchedIssues,
    dry_run: bool,
) -> Result<ImportSummary, LificError> {
    let mut summary = ImportSummary {
        dry_run,
        skipped_non_issues: fetched.skipped_non_issues,
        skipped_assignees: fetched.skipped_assignees,
        skipped_other: fetched.skipped_other,
        ..Default::default()
    };

    if dry_run {
        // Count what a real run would do without writing. A label is "planned"
        // once across the whole batch even if several issues carry it and it
        // doesn't already exist.
        let conn = pool.read()?;
        let mut planned_labels: std::collections::HashSet<String> = std::collections::HashSet::new();
        for issue in &fetched.issues {
            if source_exists(&conn, &issue.source)? {
                summary.issues_skipped_existing += 1;
                continue;
            }
            summary.issues_created += 1;
            summary.comments_planned += issue.comments.len();
            for label in &issue.labels {
                if queries::resolve_label_name(&conn, project_id, &label.name).is_err() {
                    planned_labels.insert(label.name.clone());
                }
            }
        }
        summary.labels_planned = planned_labels.len();
        return Ok(summary);
    }

    for issue in &fetched.issues {
        match apply_issue(pool, project_id, bot_id, issue)? {
            ApplyOutcome::Created {
                comments_created,
                labels_created,
            } => {
                summary.issues_created += 1;
                summary.comments_created += comments_created;
                summary.labels_created += labels_created;
            }
            ApplyOutcome::Skipped => summary.issues_skipped_existing += 1,
        }
    }
    Ok(summary)
}

/// A status-mapping table shared by importers: maps a source's state token to a
/// Lific status, with per-flag overrides. `open`/`closed` naming echoes
/// GitHub's flags but the mechanism is generic.
#[derive(Debug, Clone)]
pub struct StatusMap {
    pub open: String,
    pub closed: String,
}

impl Default for StatusMap {
    fn default() -> Self {
        StatusMap {
            open: "backlog".into(),
            closed: "done".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn seed_project(pool: &DbPool, ident: &str) -> i64 {
        let conn = pool.write().unwrap();
        queries::create_project(
            &conn,
            &crate::db::models::CreateProject {
                name: format!("Project {ident}"),
                identifier: ident.into(),
                description: String::new(),
                emoji: None,
                lead_user_id: None,
            },
        )
        .unwrap()
        .id
    }

    fn seed_user(pool: &DbPool, username: &str, admin: bool) -> i64 {
        let conn = pool.write().unwrap();
        queries::users::create_user(
            &conn,
            &crate::db::models::CreateUser {
                username: username.into(),
                email: format!("{username}@test.com"),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: admin,
                is_bot: false,
            },
        )
        .unwrap()
        .id
    }

    fn norm(source: &str, title: &str) -> NormalizedIssue {
        NormalizedIssue {
            source: source.into(),
            title: title.into(),
            description: "body".into(),
            status: "backlog".into(),
            priority: "none".into(),
            labels: vec![],
            comments: vec![],
        }
    }

    #[test]
    fn attribution_prefix_includes_author_and_time() {
        let c = NormalizedComment {
            author: "octocat".into(),
            created_at: Some("2024-01-02T03:04:05Z".into()),
            body: "looks good".into(),
        };
        let out = c.body_with_attribution();
        assert!(out.contains("@octocat"));
        assert!(out.contains("2024-01-02T03:04:05Z"));
        assert!(out.ends_with("looks good"));
    }

    #[test]
    fn attribution_prefix_without_time() {
        let c = NormalizedComment {
            author: "bob".into(),
            created_at: None,
            body: "hi".into(),
        };
        let out = c.body_with_attribution();
        assert!(out.contains("@bob"));
        assert!(!out.contains(" on "));
    }

    #[test]
    fn resolve_owner_sole_human() {
        let pool = db::open_memory().unwrap();
        let id = seed_user(&pool, "solo", false);
        assert_eq!(resolve_owner(&pool, None).unwrap(), Some(id));
    }

    #[test]
    fn resolve_owner_fresh_install_is_none() {
        let pool = db::open_memory().unwrap();
        assert_eq!(resolve_owner(&pool, None).unwrap(), None);
    }

    #[test]
    fn resolve_owner_multiple_requires_user() {
        let pool = db::open_memory().unwrap();
        seed_user(&pool, "a", false);
        seed_user(&pool, "b", false);
        assert!(resolve_owner(&pool, None).is_err());
        // explicit choice works
        let a = {
            let conn = pool.read().unwrap();
            queries::users::get_user_by_username(&conn, "a").unwrap().id
        };
        assert_eq!(resolve_owner(&pool, Some("a")).unwrap(), Some(a));
    }

    #[test]
    fn resolve_owner_prefers_sole_admin() {
        let pool = db::open_memory().unwrap();
        seed_user(&pool, "user", false);
        let admin = seed_user(&pool, "boss", true);
        assert_eq!(resolve_owner(&pool, None).unwrap(), Some(admin));
    }

    #[test]
    fn import_bot_is_created_once_and_owned() {
        let pool = db::open_memory().unwrap();
        let owner = seed_user(&pool, "solo", true);
        let bot1 = ensure_import_bot(&pool, owner, "github", "GitHub Import").unwrap();
        let bot2 = ensure_import_bot(&pool, owner, "github", "GitHub Import").unwrap();
        assert_eq!(bot1, bot2, "second call reuses the bot");
        let conn = pool.read().unwrap();
        let (is_bot, owner_id): (bool, Option<i64>) = conn
            .query_row(
                "SELECT is_bot, owner_id FROM users WHERE id = ?1",
                rusqlite::params![bot1],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(is_bot);
        assert_eq!(owner_id, Some(owner));
    }

    #[test]
    fn apply_issue_creates_and_is_idempotent() {
        let pool = db::open_memory().unwrap();
        let pid = seed_project(&pool, "APP");
        let owner = seed_user(&pool, "solo", true);
        let bot = ensure_import_bot(&pool, owner, "github", "GitHub Import").unwrap();

        let mut issue = norm("github:o/n#1", "First");
        issue.labels = vec![NormalizedLabel {
            name: "bug".into(),
            color: Some("#EF4444".into()),
        }];
        issue.comments = vec![NormalizedComment {
            author: "octocat".into(),
            created_at: Some("2024-01-01".into()),
            body: "hi".into(),
        }];

        // First apply creates everything.
        match apply_issue(&pool, pid, Some(bot), &issue).unwrap() {
            ApplyOutcome::Created {
                comments_created,
                labels_created,
            } => {
                assert_eq!(comments_created, 1);
                assert_eq!(labels_created, 1);
            }
            ApplyOutcome::Skipped => panic!("first apply must create"),
        }

        // Second apply is a no-op (idempotent by source marker).
        assert!(matches!(
            apply_issue(&pool, pid, Some(bot), &issue).unwrap(),
            ApplyOutcome::Skipped
        ));

        // Exactly one issue, one comment.
        let conn = pool.read().unwrap();
        let issue_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM issues", [], |r| r.get(0))
            .unwrap();
        assert_eq!(issue_count, 1);
        let comment_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM comments", [], |r| r.get(0))
            .unwrap();
        assert_eq!(comment_count, 1);
    }

    #[test]
    fn dry_run_writes_nothing_and_counts() {
        let pool = db::open_memory().unwrap();
        let pid = seed_project(&pool, "APP");

        let mut a = norm("github:o/n#1", "A");
        a.labels = vec![NormalizedLabel {
            name: "bug".into(),
            color: None,
        }];
        a.comments = vec![NormalizedComment {
            author: "x".into(),
            created_at: None,
            body: "c".into(),
        }];
        let mut b = norm("github:o/n#2", "B");
        b.labels = vec![NormalizedLabel {
            name: "bug".into(),
            color: None,
        }]; // same label — planned once

        let fetched = FetchedIssues {
            issues: vec![a, b],
            skipped_non_issues: 3,
            skipped_assignees: 2,
            skipped_other: 1,
        };

        let summary = run_import(&pool, pid, None, &fetched, true).unwrap();
        assert!(summary.dry_run);
        assert_eq!(summary.issues_created, 2);
        assert_eq!(summary.comments_planned, 1);
        assert_eq!(summary.labels_planned, 1, "shared label counted once");
        assert_eq!(summary.skipped_non_issues, 3);
        assert_eq!(summary.skipped_assignees, 2);

        // Nothing on disk.
        let conn = pool.read().unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM issues", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "dry run must not write");
    }

    #[test]
    fn run_import_skips_already_imported() {
        let pool = db::open_memory().unwrap();
        let pid = seed_project(&pool, "APP");
        let fetched = FetchedIssues {
            issues: vec![norm("github:o/n#1", "A"), norm("github:o/n#2", "B")],
            ..Default::default()
        };
        // First run creates both.
        let s1 = run_import(&pool, pid, None, &fetched, false).unwrap();
        assert_eq!(s1.issues_created, 2);
        // Second run skips both.
        let s2 = run_import(&pool, pid, None, &fetched, false).unwrap();
        assert_eq!(s2.issues_created, 0);
        assert_eq!(s2.issues_skipped_existing, 2);
    }
}
