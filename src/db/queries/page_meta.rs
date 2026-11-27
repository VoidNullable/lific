//! Reads behind `GET /api/projects/{id}/pages/meta`: what the Pages tab
//! shows about each page that the sync rows leave out. Who wrote it, how
//! long it is, how it opens, and what else in the project points at it.
//!
//! Every query is per project, never per page: one read of the pages, one
//! grouped read of their audit rows, and one read each of the issue,
//! page and comment texts that may refer to them. The text work itself is
//! in [`crate::page_meta`].

use std::collections::HashMap;

use rusqlite::{Connection, params};
use serde::Serialize;

use crate::error::LificError;
use crate::page_meta::{
    LinkCounts, PageKeys, Source, SourceKind, excerpt, tally_incoming, word_count,
};

/// The account behind an audit row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PageActor {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub is_bot: bool,
    /// The connected tool a bot stands for; NULL for people and for bots
    /// created before tool ids existed.
    pub tool_id: Option<String>,
}

/// One page's metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PageMeta {
    pub id: i64,
    pub excerpt: String,
    pub words: usize,
    pub created_by: Option<PageActor>,
    pub updated_by: Option<PageActor>,
    pub links: LinkCounts,
    pub linked_from: Vec<String>,
}

/// The endpoint's response body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PageMetaList {
    pub pages: Vec<PageMeta>,
}

struct PageRow {
    id: i64,
    sequence: Option<i64>,
    content: String,
    updated_at: String,
}

/// Metadata for every live page of `project_id`, archived and conventions
/// pages included, in id order. A project that does not exist has none.
pub fn project_page_meta(conn: &Connection, project_id: i64) -> Result<PageMetaList, LificError> {
    // One snapshot for all the reads below, as the index does.
    let tx = conn.unchecked_transaction()?;
    let identifier: Option<String> = tx
        .query_row(
            "SELECT identifier FROM projects WHERE id = ?1",
            params![project_id],
            |row| row.get(0),
        )
        .map(Some)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?;
    let Some(identifier) = identifier else {
        return Ok(PageMetaList { pages: Vec::new() });
    };

    let pages = live_pages(&tx, project_id)?;
    let mut actors = page_actors(&tx, project_id)?;
    let issues = issue_texts(&tx, project_id)?;
    let comments = comment_texts(&tx, project_id)?;

    let keys = PageKeys {
        by_sequence: pages
            .iter()
            .filter_map(|page| page.sequence.map(|sequence| (sequence, page.id)))
            .collect(),
        ids: pages.iter().map(|page| page.id).collect(),
    };

    // Every referring text, most recently updated first, so `linked_from`
    // lists the freshest references first.
    let mut sources: Vec<(&str, Source<'_>)> = Vec::new();
    for issue in &issues {
        sources.push((
            issue.updated_at.as_str(),
            Source {
                kind: SourceKind::Issue,
                own_page: None,
                label: Some(format!("{identifier}-{}", issue.sequence)),
                texts: vec![issue.title.as_str(), issue.description.as_str()],
            },
        ));
    }
    for page in &pages {
        sources.push((
            page.updated_at.as_str(),
            Source {
                kind: SourceKind::Page,
                own_page: Some(page.id),
                label: page
                    .sequence
                    .map(|sequence| format!("{identifier}-DOC-{sequence}")),
                texts: vec![page.content.as_str()],
            },
        ));
    }
    for comment in &comments {
        sources.push((
            comment.updated_at.as_str(),
            Source {
                kind: SourceKind::Comment,
                own_page: comment.page_id,
                label: None,
                texts: vec![comment.content.as_str()],
            },
        ));
    }
    sources.sort_by(|a, b| b.0.cmp(a.0));
    let sources: Vec<Source<'_>> = sources.into_iter().map(|(_, source)| source).collect();
    let mut incoming = tally_incoming(&sources, &identifier, &keys);

    let pages = pages
        .iter()
        .map(|page| {
            let (created_by, updated_by) = actors.remove(&page.id).unwrap_or((None, None));
            let incoming = incoming.remove(&page.id).unwrap_or_default();
            PageMeta {
                id: page.id,
                excerpt: excerpt(&page.content),
                words: word_count(&page.content),
                created_by,
                updated_by,
                links: incoming.links,
                linked_from: incoming.linked_from,
            }
        })
        .collect();
    Ok(PageMetaList { pages })
}

fn live_pages(conn: &Connection, project_id: i64) -> Result<Vec<PageRow>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, sequence, content, updated_at FROM pages
          WHERE project_id = ?1 AND deleted_at IS NULL
          ORDER BY id",
    )?;
    let rows = stmt.query_map(params![project_id], |row| {
        Ok(PageRow {
            id: row.get(0)?,
            sequence: row.get(1)?,
            content: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            updated_at: row.get(3)?,
        })
    })?;
    rows.collect::<Result<_, _>>().map_err(Into::into)
}

type ActorPair = (Option<PageActor>, Option<PageActor>);

/// For each live page, the actor of its earliest and of its latest audit
/// row. A row with no actor, or one whose user is gone, gives None.
fn page_actors(conn: &Connection, project_id: i64) -> Result<HashMap<i64, ActorPair>, LificError> {
    let mut stmt = conn.prepare_cached(
        "WITH ranked AS (
             SELECT a.entity_id, a.actor_user_id,
                    ROW_NUMBER() OVER (PARTITION BY a.entity_id ORDER BY a.ts ASC, a.id ASC)
                        AS first_rank,
                    ROW_NUMBER() OVER (PARTITION BY a.entity_id ORDER BY a.ts DESC, a.id DESC)
                        AS last_rank
               FROM audit_log a
              WHERE a.entity_type = 'page'
                AND a.entity_id IN (SELECT id FROM pages
                                     WHERE project_id = ?1 AND deleted_at IS NULL)
         )
         SELECT r.entity_id, r.first_rank = 1, r.last_rank = 1,
                u.id, u.username, u.display_name, u.is_bot, u.tool_id
           FROM ranked r
           LEFT JOIN users u ON u.id = r.actor_user_id
          WHERE r.first_rank = 1 OR r.last_rank = 1",
    )?;
    let rows = stmt.query_map(params![project_id], |row| {
        let actor = match row.get::<_, Option<i64>>(3)? {
            Some(id) => Some(PageActor {
                id,
                username: row.get(4)?,
                display_name: row.get(5)?,
                is_bot: row.get(6)?,
                tool_id: row.get(7)?,
            }),
            None => None,
        };
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, bool>(1)?,
            row.get::<_, bool>(2)?,
            actor,
        ))
    })?;
    let mut actors: HashMap<i64, ActorPair> = HashMap::new();
    for row in rows {
        let (page, first, last, actor) = row?;
        let entry = actors.entry(page).or_default();
        if first {
            entry.0 = actor.clone();
        }
        if last {
            entry.1 = actor;
        }
    }
    Ok(actors)
}

struct IssueText {
    sequence: i64,
    title: String,
    description: String,
    updated_at: String,
}

fn issue_texts(conn: &Connection, project_id: i64) -> Result<Vec<IssueText>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT sequence, title, description, updated_at FROM issues
          WHERE project_id = ?1 AND deleted_at IS NULL",
    )?;
    let rows = stmt.query_map(params![project_id], |row| {
        Ok(IssueText {
            sequence: row.get(0)?,
            title: row.get(1)?,
            description: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            updated_at: row.get(3)?,
        })
    })?;
    rows.collect::<Result<_, _>>().map_err(Into::into)
}

struct CommentText {
    page_id: Option<i64>,
    content: String,
    updated_at: String,
}

/// Live comments on the project's live issues and pages.
fn comment_texts(conn: &Connection, project_id: i64) -> Result<Vec<CommentText>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT c.page_id, c.content, c.updated_at FROM comments c
          WHERE c.deleted_at IS NULL
            AND (c.issue_id IN (SELECT id FROM issues
                                 WHERE project_id = ?1 AND deleted_at IS NULL)
              OR c.page_id IN (SELECT id FROM pages
                                WHERE project_id = ?1 AND deleted_at IS NULL))",
    )?;
    let rows = stmt.query_map(params![project_id], |row| {
        Ok(CommentText {
            page_id: row.get(0)?,
            content: row.get(1)?,
            updated_at: row.get(2)?,
        })
    })?;
    rows.collect::<Result<_, _>>().map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::{ActorCtx, Transport};
    use crate::db::models::*;
    use crate::db::queries::{self, users};

    fn act_as(conn: &Connection, user_id: Option<i64>) {
        crate::actor::stamp(
            conn,
            &ActorCtx {
                user_id,
                transport: Transport::Web,
            },
        );
    }

    fn user(conn: &Connection, name: &str, is_bot: bool) -> i64 {
        users::create_user(
            conn,
            &CreateUser {
                username: name.into(),
                email: format!("{name}@test.local"),
                password: "testpassword1".into(),
                display_name: Some(format!("{name} display")),
                is_admin: false,
                is_bot,
            },
        )
        .unwrap()
        .id
    }

    fn project(conn: &Connection, identifier: &str) -> i64 {
        queries::create_project(
            conn,
            &CreateProject {
                name: identifier.into(),
                identifier: identifier.into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id
    }

    fn page(conn: &Connection, project_id: i64, title: &str, content: &str) -> Page {
        queries::create_page(
            conn,
            &CreatePage {
                project_id: Some(project_id),
                title: title.into(),
                content: content.into(),
                ..Default::default()
            },
        )
        .unwrap()
    }

    fn issue(conn: &Connection, project_id: i64, title: &str, description: &str) -> Issue {
        queries::create_issue(
            conn,
            &CreateIssue {
                project_id,
                title: title.into(),
                description: description.into(),
                ..Default::default()
            },
        )
        .unwrap()
    }

    fn comment(
        conn: &Connection,
        issue_id: Option<i64>,
        page_id: Option<i64>,
        content: &str,
    ) -> i64 {
        conn.execute(
            "INSERT INTO comments (issue_id, page_id, imported_author, content)
             VALUES (?1, ?2, 'tester', ?3)",
            params![issue_id, page_id, content],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn meta_of(list: &PageMetaList, id: i64) -> &PageMeta {
        list.pages.iter().find(|meta| meta.id == id).unwrap()
    }

    #[test]
    fn created_by_and_updated_by_follow_the_audit_log() {
        let db = crate::db::open_memory().unwrap();
        let conn = db.write().unwrap();
        let alice = user(&conn, "alice", false);
        let bot = user(&conn, "bot", true);
        let project_id = project(&conn, "MET");

        act_as(&conn, Some(alice));
        let written = page(&conn, project_id, "Doc", "Original text.");
        let untouched = page(&conn, project_id, "Other", "");
        act_as(&conn, Some(bot));
        queries::update_page(
            &conn,
            written.id,
            &UpdatePage {
                content: Some("Rewritten text.".into()),
                ..Default::default()
            },
        )
        .unwrap();

        let list = project_page_meta(&conn, project_id).unwrap();
        let meta = meta_of(&list, written.id);
        let created = meta.created_by.as_ref().unwrap();
        assert_eq!(created.id, alice);
        assert_eq!(created.username, "alice");
        assert_eq!(created.display_name, "alice display");
        assert!(!created.is_bot);
        let updated = meta.updated_by.as_ref().unwrap();
        assert_eq!(updated.id, bot);
        assert!(updated.is_bot);
        assert_eq!(updated.tool_id, None);
        assert_eq!(meta.excerpt, "Rewritten text.");
        assert_eq!(meta.words, 2);

        let other = meta_of(&list, untouched.id);
        assert_eq!(other.created_by.as_ref().unwrap().id, alice);
        assert_eq!(other.updated_by.as_ref().unwrap().id, alice);
    }

    #[test]
    fn an_unattributed_or_vanished_actor_is_null() {
        let db = crate::db::open_memory().unwrap();
        let conn = db.write().unwrap();
        let project_id = project(&conn, "MET");
        act_as(&conn, None);
        let system = page(&conn, project_id, "System", "");
        act_as(&conn, Some(987_654));
        let ghost = page(&conn, project_id, "Ghost", "");

        let list = project_page_meta(&conn, project_id).unwrap();
        for id in [system.id, ghost.id] {
            let meta = meta_of(&list, id);
            assert_eq!(meta.created_by, None);
            assert_eq!(meta.updated_by, None);
        }
    }

    #[test]
    fn links_count_live_referrers_in_the_same_project_only() {
        let db = crate::db::open_memory().unwrap();
        let conn = db.write().unwrap();
        let project_id = project(&conn, "MET");
        let other_project = project(&conn, "OTH");
        let target = page(&conn, project_id, "Target", "Self link MET-DOC-1.");
        assert_eq!(target.identifier, "MET-DOC-1");

        let live_issue = issue(
            &conn,
            project_id,
            "Mentions met-doc-1",
            "and MET-DOC-1 again",
        );
        let dead_issue = issue(&conn, project_id, "Also MET-DOC-1", "");
        queries::delete_issue(&conn, dead_issue.id).unwrap();
        issue(&conn, other_project, "Foreign MET-DOC-1", "");
        issue(&conn, project_id, "Near miss MET-DOC-10", "");

        let linking = page(
            &conn,
            project_id,
            "Linking",
            &format!("See [target](/MET/pages/{})", target.id),
        );
        let dead_page = page(&conn, project_id, "Dead", "MET-DOC-1");
        queries::delete_page(&conn, dead_page.id).unwrap();

        comment(&conn, Some(live_issue.id), None, "comment on MET-DOC-1");
        let dead_comment = comment(&conn, Some(live_issue.id), None, "gone MET-DOC-1");
        queries::comments::delete_comment(&conn, dead_comment).unwrap();
        comment(
            &conn,
            None,
            Some(target.id),
            "a comment on the page itself, MET-DOC-1",
        );
        comment(
            &conn,
            None,
            Some(linking.id),
            "comment on another page: MET-DOC-1",
        );

        let list = project_page_meta(&conn, project_id).unwrap();
        assert_eq!(list.pages.len(), 2, "the deleted page is left out");
        let meta = meta_of(&list, target.id);
        assert_eq!(
            meta.links,
            LinkCounts {
                issues: 1,
                pages: 1,
                comments: 2
            }
        );
        let mut linked_from = meta.linked_from.clone();
        linked_from.sort();
        assert_eq!(
            linked_from,
            vec!["MET-1".to_string(), "MET-DOC-2".to_string()]
        );
        assert_eq!(meta_of(&list, linking.id).links, LinkCounts::default());
    }

    #[test]
    fn linked_from_lists_the_most_recently_updated_first() {
        let db = crate::db::open_memory().unwrap();
        let conn = db.write().unwrap();
        let project_id = project(&conn, "MET");
        let target = page(&conn, project_id, "Target", "");
        let older = issue(&conn, project_id, "MET-DOC-1 older", "");
        let newer = issue(&conn, project_id, "MET-DOC-1 newer", "");
        // Moving `seq` as well keeps the bump triggers from overwriting the
        // backdated `updated_at`.
        for (id, at) in [
            (older.id, "2026-01-01 00:00:00"),
            (newer.id, "2026-06-01 00:00:00"),
        ] {
            conn.execute(
                "UPDATE issues SET updated_at = ?2, seq = seq + 1000000 WHERE id = ?1",
                params![id, at],
            )
            .unwrap();
        }

        let list = project_page_meta(&conn, project_id).unwrap();
        assert_eq!(
            meta_of(&list, target.id).linked_from,
            vec![newer.identifier, older.identifier]
        );
    }

    #[test]
    fn a_missing_project_has_no_pages() {
        let db = crate::db::open_memory().unwrap();
        let conn = db.write().unwrap();
        assert_eq!(project_page_meta(&conn, 987_654).unwrap().pages, Vec::new());
    }
}
