//! LIF-156: audit-log read surface.
//!
//! Capture happens entirely in SQL (migration 018 triggers) — this module
//! only reads. Every query LEFT JOINs `users` so actor display data rides
//! along, degrading gracefully when the actor's account is gone.

use rusqlite::Connection;

use crate::db::models::{Activity, ActivityFeed};
use crate::error::LificError;

/// Which slice of the audit log to read.
///
/// `Issue`/`Page` use the denormalized parent columns, so an issue's feed
/// naturally includes its comments, label attach/detach, and relation
/// events — not just edits to the issue row itself.
#[derive(Debug, Clone, Copy)]
pub enum ActivityScope {
    Issue(i64),
    Page(i64),
    Project(i64),
}

const MAX_LIMIT: i64 = 200;
const DEFAULT_LIMIT: i64 = 50;

/// List activity newest-first. `limit` is clamped to 1..=200 (default 50).
/// Fetches limit+1 rows internally to compute `has_more` without a COUNT.
pub fn list_activity(
    conn: &Connection,
    scope: ActivityScope,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<ActivityFeed, LificError> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = offset.unwrap_or(0).max(0);

    let (where_clause, scope_id) = match scope {
        ActivityScope::Issue(id) => ("a.issue_id = ?1", id),
        ActivityScope::Page(id) => ("a.page_id = ?1", id),
        ActivityScope::Project(id) => ("a.project_id = ?1", id),
    };

    let sql = format!(
        "SELECT a.id, a.ts, a.actor_user_id, u.username, u.display_name,
                COALESCE(u.is_bot, 0), a.transport, a.entity_type, a.entity_id,
                a.entity_label, a.project_id, a.issue_id, a.page_id,
                a.action, a.field, a.old_value, a.new_value
         FROM audit_log a
         LEFT JOIN users u ON u.id = a.actor_user_id
         WHERE {where_clause}
         ORDER BY a.id DESC
         LIMIT ?2 OFFSET ?3"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params![scope_id, limit + 1, offset],
        row_to_activity,
    )?;
    let mut items: Vec<Activity> = rows.collect::<Result<Vec<_>, _>>()?;

    let has_more = items.len() as i64 > limit;
    items.truncate(limit as usize);

    Ok(ActivityFeed { items, has_more })
}

/// Per-actor rollup for a project: action counts, last-seen, and their
/// dominant transport. Ordered most-active-first. `actor_user_id` NULL
/// groups all system/unattributed writes into one row.
pub fn actor_stats(
    conn: &Connection,
    project_id: i64,
) -> Result<Vec<crate::db::models::ActorStat>, LificError> {
    let mut stmt = conn.prepare(
        "SELECT a.actor_user_id, u.username, u.display_name, COALESCE(u.is_bot, 0),
                COUNT(*) AS actions, MAX(a.ts) AS last_ts,
                (SELECT t.transport FROM audit_log t
                 WHERE t.project_id = a.project_id
                   AND t.actor_user_id IS a.actor_user_id
                 GROUP BY t.transport ORDER BY COUNT(*) DESC LIMIT 1) AS top_transport
         FROM audit_log a
         LEFT JOIN users u ON u.id = a.actor_user_id
         WHERE a.project_id = ?1
         GROUP BY a.actor_user_id
         ORDER BY actions DESC",
    )?;
    let rows = stmt.query_map([project_id], |row| {
        Ok(crate::db::models::ActorStat {
            actor_user_id: row.get(0)?,
            username: row.get(1)?,
            display_name: row.get(2)?,
            is_bot: row.get::<_, i64>(3)? != 0,
            actions: row.get(4)?,
            last_ts: row.get(5)?,
            top_transport: row.get(6)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(LificError::Database)
}

fn row_to_activity(row: &rusqlite::Row<'_>) -> Result<Activity, rusqlite::Error> {
    Ok(Activity {
        id: row.get(0)?,
        ts: row.get(1)?,
        actor_user_id: row.get(2)?,
        actor_username: row.get(3)?,
        actor_display_name: row.get(4)?,
        actor_is_bot: row.get::<_, i64>(5)? != 0,
        transport: row.get(6)?,
        entity_type: row.get(7)?,
        entity_id: row.get(8)?,
        entity_label: row.get(9)?,
        project_id: row.get(10)?,
        issue_id: row.get(11)?,
        page_id: row.get(12)?,
        action: row.get(13)?,
        field: row.get(14)?,
        old_value: row.get(15)?,
        new_value: row.get(16)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::{ActorCtx, Transport};
    use crate::db::models::*;
    use crate::db::queries;

    /// Seed a project and return (pool, project_id).
    fn seeded() -> (crate::db::DbPool, i64) {
        let pool = crate::db::open_memory().expect("test db");
        let project = {
            let conn = pool.write().unwrap();
            queries::create_project(
                &conn,
                &CreateProject {
                    name: "Audit Test".into(),
                    identifier: "AUD".into(),
                    description: String::new(),
                    emoji: None,
                    lead_user_id: None,
                },
            )
            .unwrap()
        };
        (pool, project.id)
    }

    fn new_issue(pid: i64, title: &str) -> CreateIssue {
        CreateIssue {
            project_id: pid,
            title: title.into(),
            description: String::new(),
            status: "backlog".into(),
            priority: "none".into(),
            module_id: None,
            start_date: None,
            target_date: None,
            labels: vec![],
        }
    }

    fn no_update() -> UpdateIssue {
        UpdateIssue {
            title: None,
            description: None,
            status: None,
            priority: None,
            module_id: None,
            sort_order: None,
            start_date: None,
            target_date: None,
            labels: None,
        }
    }

    fn seed_user(conn: &rusqlite::Connection, username: &str, is_bot: bool) -> i64 {
        conn.execute(
            "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot)
             VALUES (?1, ?2, 'x', ?1, 0, ?3)",
            rusqlite::params![username, format!("{username}@test.local"), is_bot],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn issue_feed(pool: &crate::db::DbPool, issue_id: i64) -> Vec<Activity> {
        let conn = pool.read().unwrap();
        list_activity(&conn, ActivityScope::Issue(issue_id), Some(100), None)
            .unwrap()
            .items
    }

    // ── Capture: per-field diffs ─────────────────────────

    #[test]
    fn issue_create_is_audited() {
        let (pool, pid) = seeded();
        let conn = pool.write().unwrap();
        let issue = queries::create_issue(&conn, &new_issue(pid, "First")).unwrap();
        drop(conn);

        let feed = issue_feed(&pool, issue.id);
        let create = feed.iter().find(|a| a.action == "create").unwrap();
        assert_eq!(create.entity_type, "issue");
        assert_eq!(create.entity_label.as_deref(), Some("AUD-1"));
        assert_eq!(create.new_value.as_deref(), Some("First"));
        assert_eq!(create.project_id, Some(pid));
    }

    #[test]
    fn issue_update_writes_one_row_per_changed_field() {
        let (pool, pid) = seeded();
        let conn = pool.write().unwrap();
        let issue = queries::create_issue(&conn, &new_issue(pid, "Original")).unwrap();

        queries::update_issue(
            &conn,
            issue.id,
            &UpdateIssue {
                title: Some("Renamed".into()),
                status: Some("active".into()),
                priority: Some("high".into()),
                ..no_update()
            },
        )
        .unwrap();
        drop(conn);

        let feed = issue_feed(&pool, issue.id);
        let updates: Vec<&Activity> = feed.iter().filter(|a| a.action == "update").collect();
        assert_eq!(updates.len(), 3, "one row per changed field: {updates:#?}");

        let status = updates
            .iter()
            .find(|a| a.field.as_deref() == Some("status"))
            .unwrap();
        assert_eq!(status.old_value.as_deref(), Some("backlog"));
        assert_eq!(status.new_value.as_deref(), Some("active"));

        let title = updates
            .iter()
            .find(|a| a.field.as_deref() == Some("title"))
            .unwrap();
        assert_eq!(title.old_value.as_deref(), Some("Original"));
        assert_eq!(title.new_value.as_deref(), Some("Renamed"));
    }

    #[test]
    fn unchanged_fields_produce_no_rows() {
        let (pool, pid) = seeded();
        let conn = pool.write().unwrap();
        let issue = queries::create_issue(&conn, &new_issue(pid, "Same")).unwrap();

        queries::update_issue(
            &conn,
            issue.id,
            &UpdateIssue {
                title: Some("Same".into()),
                ..no_update()
            },
        )
        .unwrap();
        drop(conn);

        let feed = issue_feed(&pool, issue.id);
        assert!(
            feed.iter().all(|a| a.action != "update"),
            "no-op update must not audit: {feed:#?}"
        );
    }

    #[test]
    fn module_change_records_names_not_ids() {
        let (pool, pid) = seeded();
        let conn = pool.write().unwrap();
        let module = queries::create_module(
            &conn,
            &CreateModule {
                project_id: pid,
                name: "Web UI".into(),
                description: String::new(),
                status: "active".into(),
                emoji: None,
            },
        )
        .unwrap();
        let issue = queries::create_issue(&conn, &new_issue(pid, "I")).unwrap();

        queries::update_issue(
            &conn,
            issue.id,
            &UpdateIssue {
                module_id: Some(module.id),
                ..no_update()
            },
        )
        .unwrap();
        drop(conn);

        let feed = issue_feed(&pool, issue.id);
        let row = feed
            .iter()
            .find(|a| a.field.as_deref() == Some("module"))
            .unwrap();
        assert_eq!(row.old_value, None);
        assert_eq!(row.new_value.as_deref(), Some("Web UI"));
    }

    #[test]
    fn issue_delete_snapshots_label_and_title() {
        let (pool, pid) = seeded();
        let conn = pool.write().unwrap();
        let issue = queries::create_issue(&conn, &new_issue(pid, "Doomed")).unwrap();
        queries::delete_issue(&conn, issue.id).unwrap();
        drop(conn);

        let feed = issue_feed(&pool, issue.id);
        let del = feed
            .iter()
            .find(|a| a.action == "delete" && a.entity_type == "issue")
            .unwrap();
        assert_eq!(del.entity_label.as_deref(), Some("AUD-1"));
        assert_eq!(del.old_value.as_deref(), Some("Doomed"));
    }

    // ── Capture: children land in the parent feed ────────

    #[test]
    fn comment_and_labels_appear_in_issue_feed() {
        let (pool, pid) = seeded();
        let conn = pool.write().unwrap();
        let user_id = seed_user(&conn, "carol", false);
        let issue = queries::create_issue(&conn, &new_issue(pid, "Busy")).unwrap();
        queries::comments::create_comment(
            &conn,
            queries::comments::CommentParent::Issue(issue.id),
            user_id,
            "hello world",
        )
        .unwrap();
        queries::create_label(
            &conn,
            &CreateLabel {
                project_id: pid,
                name: "bug".into(),
                color: "#6B7280".into(),
            },
        )
        .unwrap();
        queries::update_issue(
            &conn,
            issue.id,
            &UpdateIssue {
                labels: Some(vec!["bug".into()]),
                ..no_update()
            },
        )
        .unwrap();
        drop(conn);

        let feed = issue_feed(&pool, issue.id);
        let comment = feed
            .iter()
            .find(|a| a.entity_type == "comment" && a.action == "create")
            .expect("comment create in issue feed");
        assert_eq!(comment.new_value.as_deref(), Some("hello world"));
        assert_eq!(comment.entity_label.as_deref(), Some("AUD-1"));

        let attach = feed
            .iter()
            .find(|a| a.action == "attach")
            .expect("label attach in issue feed");
        assert_eq!(attach.new_value.as_deref(), Some("bug"));
    }

    #[test]
    fn relation_link_records_target_identifier() {
        let (pool, pid) = seeded();
        let conn = pool.write().unwrap();
        let a = queries::create_issue(&conn, &new_issue(pid, "A")).unwrap();
        let b = queries::create_issue(&conn, &new_issue(pid, "B")).unwrap();
        queries::link_issues(&conn, a.id, b.id, "blocks").unwrap();
        queries::unlink_issues(&conn, a.id, b.id).unwrap();
        drop(conn);

        let feed = issue_feed(&pool, a.id);
        let link = feed.iter().find(|x| x.action == "link").unwrap();
        assert_eq!(link.field.as_deref(), Some("blocks"));
        assert_eq!(link.new_value.as_deref(), Some("AUD-2"));
        let unlink = feed.iter().find(|x| x.action == "unlink").unwrap();
        assert_eq!(unlink.old_value.as_deref(), Some("AUD-2"));
    }

    // ── Capture: pages ───────────────────────────────────

    #[test]
    fn page_edits_are_audited_with_doc_identifier() {
        let (pool, pid) = seeded();
        let conn = pool.write().unwrap();
        let page = queries::create_page(
            &conn,
            &CreatePage {
                project_id: Some(pid),
                folder_id: None,
                title: "Notes".into(),
                content: String::new(),
                status: "draft".into(),
                labels: vec![],
            },
        )
        .unwrap();
        queries::update_page(
            &conn,
            page.id,
            &UpdatePage {
                title: None,
                content: Some("# new content".into()),
                folder_id: None,
                sort_order: None,
                status: None,
                labels: None,
            },
        )
        .unwrap();
        drop(conn);

        let conn = pool.read().unwrap();
        let feed = list_activity(&conn, ActivityScope::Page(page.id), None, None)
            .unwrap()
            .items;
        let edit = feed
            .iter()
            .find(|a| a.field.as_deref() == Some("content"))
            .unwrap();
        assert_eq!(edit.entity_label.as_deref(), Some("AUD-DOC-1"));
        assert_eq!(edit.new_value.as_deref(), Some("# new content"));
    }

    // ── Attribution ──────────────────────────────────────

    #[test]
    fn stamped_actor_is_attributed_and_joined() {
        let (pool, pid) = seeded();
        let conn = pool.write().unwrap();
        let bot_id = seed_user(&conn, "opencode-blake", true);

        crate::actor::stamp(
            &conn,
            &ActorCtx {
                user_id: Some(bot_id),
                transport: Transport::Mcp,
            },
        );
        let issue = queries::create_issue(&conn, &new_issue(pid, "By bot")).unwrap();
        drop(conn);

        let feed = issue_feed(&pool, issue.id);
        let create = feed.iter().find(|a| a.action == "create").unwrap();
        assert_eq!(create.actor_user_id, Some(bot_id));
        assert_eq!(create.actor_username.as_deref(), Some("opencode-blake"));
        assert!(create.actor_is_bot);
        assert_eq!(create.transport, "mcp");
    }

    #[test]
    fn unstamped_writes_attribute_to_system() {
        let (pool, pid) = seeded();
        let conn = pool.write().unwrap();
        let issue = queries::create_issue(&conn, &new_issue(pid, "Anon")).unwrap();
        drop(conn);

        let feed = issue_feed(&pool, issue.id);
        let create = feed.iter().find(|a| a.action == "create").unwrap();
        assert_eq!(create.actor_user_id, None);
        assert_eq!(create.transport, "system");
        assert!(!create.actor_is_bot);
    }

    #[tokio::test]
    async fn task_local_scope_flows_through_pool_write() {
        let (pool, pid) = seeded();
        let user_id = {
            let conn = pool.write().unwrap();
            seed_user(&conn, "dave", false)
        };

        // The full production path: actor scoped on the task, stamped by
        // DbPool::write() with no manual intervention.
        let issue = crate::actor::scope(
            ActorCtx {
                user_id: Some(user_id),
                transport: Transport::Web,
            },
            async {
                let conn = pool.write().unwrap();
                queries::create_issue(&conn, &new_issue(pid, "Via web")).unwrap()
            },
        )
        .await;

        let feed = issue_feed(&pool, issue.id);
        let create = feed.iter().find(|a| a.action == "create").unwrap();
        assert_eq!(create.actor_user_id, Some(user_id));
        assert_eq!(create.transport, "web");
        assert_eq!(create.actor_username.as_deref(), Some("dave"));
    }

    #[test]
    fn deleted_actor_degrades_gracefully() {
        let (pool, pid) = seeded();
        let conn = pool.write().unwrap();
        let uid = seed_user(&conn, "fleeting", false);
        crate::actor::stamp(
            &conn,
            &ActorCtx {
                user_id: Some(uid),
                transport: Transport::Api,
            },
        );
        let issue = queries::create_issue(&conn, &new_issue(pid, "Orphan")).unwrap();
        conn.execute("DELETE FROM users WHERE id = ?1", [uid]).unwrap();
        drop(conn);

        let feed = issue_feed(&pool, issue.id);
        let create = feed.iter().find(|a| a.action == "create").unwrap();
        assert_eq!(create.actor_user_id, Some(uid), "id survives user deletion");
        assert_eq!(create.actor_username, None, "join degrades to None");
    }

    // ── Transactionality ─────────────────────────────────

    #[test]
    fn rolled_back_writes_audit_nothing() {
        let (pool, pid) = seeded();
        let conn = pool.write().unwrap();

        let result: Result<(), crate::error::LificError> =
            queries::savepoint(&conn, "audit_rb_test", || {
                queries::create_issue(&conn, &new_issue(pid, "Ghost"))?;
                Err(crate::error::LificError::BadRequest("abort".into()))
            });
        assert!(result.is_err());
        drop(conn);

        let conn = pool.read().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM audit_log WHERE entity_type = 'issue' AND new_value = 'Ghost'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "rolled-back create must not be audited");
    }

    // ── Read surface ─────────────────────────────────────

    #[test]
    fn project_feed_spans_entities_and_pages_through_results() {
        let (pool, pid) = seeded();
        let conn = pool.write().unwrap();
        for i in 0..5 {
            queries::create_issue(&conn, &new_issue(pid, &format!("Issue {i}"))).unwrap();
        }
        queries::create_module(
            &conn,
            &CreateModule {
                project_id: pid,
                name: "M".into(),
                description: String::new(),
                status: "active".into(),
                emoji: None,
            },
        )
        .unwrap();
        drop(conn);

        let conn = pool.read().unwrap();
        // Full feed: project create + 5 issue creates + module create = 7
        let all = list_activity(&conn, ActivityScope::Project(pid), Some(100), None).unwrap();
        assert_eq!(all.items.len(), 7);
        assert!(!all.has_more);
        assert!(all.items.iter().any(|a| a.entity_type == "module"));
        assert!(all.items.iter().any(|a| a.entity_type == "project"));

        // Newest-first ordering.
        assert!(all.items.windows(2).all(|w| w[0].id > w[1].id));

        // Paging: 3 + has_more, then offset to the tail.
        let first = list_activity(&conn, ActivityScope::Project(pid), Some(3), None).unwrap();
        assert_eq!(first.items.len(), 3);
        assert!(first.has_more);
        let rest = list_activity(&conn, ActivityScope::Project(pid), Some(10), Some(3)).unwrap();
        assert_eq!(rest.items.len(), 4);
        assert!(!rest.has_more);
        assert!(first.items.last().unwrap().id > rest.items.first().unwrap().id);
    }

    #[test]
    fn actor_stats_rollup_counts_ranks_and_transports() {
        let (pool, pid) = seeded();
        let conn = pool.write().unwrap();
        let alice = seed_user(&conn, "alice", false);
        let bot = seed_user(&conn, "opencode-alice", true);

        crate::actor::stamp(
            &conn,
            &ActorCtx { user_id: Some(alice), transport: Transport::Web },
        );
        queries::create_issue(&conn, &new_issue(pid, "a1")).unwrap();
        queries::create_issue(&conn, &new_issue(pid, "a2")).unwrap();

        crate::actor::stamp(
            &conn,
            &ActorCtx { user_id: Some(bot), transport: Transport::Mcp },
        );
        queries::create_issue(&conn, &new_issue(pid, "b1")).unwrap();
        queries::create_issue(&conn, &new_issue(pid, "b2")).unwrap();
        queries::create_issue(&conn, &new_issue(pid, "b3")).unwrap();
        drop(conn);

        let conn = pool.read().unwrap();
        let stats = actor_stats(&conn, pid).unwrap();
        // bot (3) > alice (2) > system (1: the project create itself)
        assert_eq!(stats.len(), 3, "{stats:#?}");
        assert_eq!(stats[0].username.as_deref(), Some("opencode-alice"));
        assert_eq!(stats[0].actions, 3);
        assert!(stats[0].is_bot);
        assert_eq!(stats[0].top_transport, "mcp");
        assert_eq!(stats[1].username.as_deref(), Some("alice"));
        assert_eq!(stats[1].actions, 2);
        assert_eq!(stats[1].top_transport, "web");
        assert_eq!(stats[2].actor_user_id, None, "system bucket last");
        assert_eq!(stats[2].top_transport, "system");
    }
}
