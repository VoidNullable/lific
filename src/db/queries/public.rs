//! LIF-465 / LIF-471: the anonymous read model for a published project.
//!
//! Publication is a predicate inside every function here, not a gate a caller
//! passes once. Each read resolves the project by identifier with
//! `is_public = 1` and then checks that whatever it is about to hand out
//! belongs to that project, all inside the caller's read snapshot, so no
//! already-resolved id can skip the check and unpublishing takes effect on the
//! next read.
//!
//! LIF-471 changed what the public surface *serves*: the private JSON shapes
//! ([`Project`], [`Issue`], [`Page`], [`Comment`], [`Module`], [`Label`],
//! [`Folder`], [`Attachment`], [`IndexSnapshot`], [`ChangesPage`]) rather than
//! a parallel set of allowlisted DTOs, so the public web view can run the same
//! components a signed-in reader uses. What keeps the boundary is the scrub
//! step every value passes through before it leaves this module:
//!
//! * a project's `lead_user_id` is dropped;
//! * an issue's `source` (import provenance) is dropped, and every relation
//!   identifier naming an issue outside this project is filtered out, so a
//!   public issue never names a private one;
//! * a comment keeps its `author_display_name` (attribution is part of what
//!   publishing means) but loses `user_id` and the login `author`;
//! * an attachment loses `uploader_id`; `sha256` is never serialized anyway;
//! * `/changes` drops every comment row, live or tombstone, since comment
//!   rows carry a `user_id` and the replica never needed them;
//! * the index cursor is the highest seq among the project's own rows rather
//!   than the instance-wide counter, so a stranger polling the snapshot cannot
//!   watch private projects being written to.
//!
//! Reachable: the published project's identity, its structure (modules,
//! labels, folders), its live issues and pages, their live comments, and
//! attachments linked to those. Not reachable, by construction: plans, the
//! audit log, member rosters, users, saved views, search, sync events, and
//! every write.

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::models::{
    Attachment, AttachmentEntity, Change, ChangesPage, Comment, Folder, IndexSnapshot, Issue,
    Label, Module, Page, Project,
};
use crate::error::LificError;

use super::comments::{CommentCursor, CommentPage, CommentParent};

/// The published project named by `identifier`, or `None`.
///
/// `None` covers "no such project", "private project" and "just unpublished"
/// alike; the caller must not be able to tell them apart, which is why this is
/// an `Option` rather than distinguishable errors. `projects.identifier` is
/// NOCASE (migration 039), so `/public/lif` and `/public/LIF` are one project.
pub fn public_project(conn: &Connection, identifier: &str) -> Result<Option<Project>, LificError> {
    let id: Option<i64> = conn
        .prepare_cached("SELECT id FROM projects WHERE identifier = ?1 AND is_public = 1")?
        .query_row(params![identifier], |row| row.get(0))
        .optional()?;
    let Some(id) = id else {
        return Ok(None);
    };
    let mut project = super::get_project(conn, id)?;
    project.lead_user_id = None;
    Ok(Some(project))
}

// ── Scrubbing ────────────────────────────────────────────────

/// True when `identifier` (`LIF-42`) names an issue in `project` (`LIF`).
fn names_project(project: &str, identifier: &str) -> bool {
    identifier
        .rsplit_once('-')
        .is_some_and(|(prefix, sequence)| {
            prefix.eq_ignore_ascii_case(project)
                && !sequence.is_empty()
                && sequence.bytes().all(|b| b.is_ascii_digit())
        })
}

/// Strip what a public reader may not learn from an issue. Relations to
/// issues in other projects are removed rather than the whole list, so the
/// in-project graph a reader is entitled to stays intact.
pub fn scrub_issue(project: &Project, issue: &mut Issue) {
    issue.source = None;
    for relations in [
        &mut issue.blocks,
        &mut issue.blocked_by,
        &mut issue.relates_to,
        &mut issue.duplicates,
        &mut issue.duplicated_by,
    ] {
        relations.retain(|identifier| names_project(&project.identifier, identifier));
    }
}

/// Keep the display name, drop the account behind it.
pub fn scrub_comment(comment: &mut Comment) {
    comment.user_id = 0;
    comment.author = String::new();
}

pub fn scrub_attachment(attachment: &mut Attachment) {
    attachment.uploader_id = None;
}

// ── Structure ────────────────────────────────────────────────

pub fn public_modules(conn: &Connection, project: &Project) -> Result<Vec<Module>, LificError> {
    super::list_modules(conn, project.id)
}

pub fn public_labels(conn: &Connection, project: &Project) -> Result<Vec<Label>, LificError> {
    super::list_labels(conn, project.id)
}

pub fn public_folders(conn: &Connection, project: &Project) -> Result<Vec<Folder>, LificError> {
    super::list_folders(conn, project.id)
}

// ── Sync snapshot and deltas ─────────────────────────────────

/// The cold-start snapshot for a published project.
///
/// The private [`super::changes::get_index`] reads the instance-wide counter
/// as its cursor, which is exactly right inside the app and exactly wrong
/// here: it would let anyone measure every project's write rate by polling.
/// Inside one read snapshot the highest seq stamped on any of this project's
/// own rows (issues, pages and their comments, live or tombstoned) is an
/// equally correct resume point: anything written later has a higher seq, and
/// it only moves when this project moves. Read before the rows, like the
/// private one, so a write racing the snapshot is re-delivered rather than
/// skipped.
pub fn public_index(conn: &Connection, project: &Project) -> Result<IndexSnapshot, LificError> {
    debug_assert!(
        !conn.is_autocommit(),
        "public_index needs a read snapshot so its cursor is exact"
    );
    let cursor: i64 = conn
        .prepare_cached(
            "SELECT COALESCE(MAX(seq), 0) FROM (
                 SELECT MAX(seq) AS seq FROM issues WHERE project_id = ?1
                 UNION ALL
                 SELECT MAX(seq) FROM pages WHERE project_id = ?1
                 UNION ALL
                 SELECT MAX(c.seq) FROM comments c
                   JOIN issues i ON i.id = c.issue_id
                  WHERE i.project_id = ?1
                 UNION ALL
                 SELECT MAX(c.seq) FROM comments c
                   JOIN pages pg ON pg.id = c.page_id
                  WHERE pg.project_id = ?1
             )",
        )?
        .query_row(params![project.id], |row| row.get(0))?;
    let (issues, pages) = super::changes::index_rows(conn, project.id)?;
    Ok(IndexSnapshot {
        cursor,
        issues,
        pages,
    })
}

/// One page of the project's delta stream with every comment row removed.
///
/// The cursor and `has_more` are taken from the unfiltered page, so a page
/// made entirely of comment rows still advances the client instead of
/// stalling it on the same `since` forever.
pub fn public_changes(
    conn: &Connection,
    project: &Project,
    since: i64,
    limit: i64,
) -> Result<ChangesPage, LificError> {
    let mut page = super::changes::list_changes(conn, project.id, since, limit)?;
    page.changes.retain(|change| match change {
        Change::Issue(_) | Change::Page(_) => true,
        Change::Comment(_) => false,
        Change::Tombstone(tombstone) => tombstone.kind != crate::db::models::ChangeKind::Comment,
    });
    Ok(page)
}

// ── Issues ───────────────────────────────────────────────────

/// A live issue of the published project, by row id. `None` for an id in
/// another project, in the trash, or nonexistent: all the same to a reader.
pub fn public_issue(
    conn: &Connection,
    project: &Project,
    issue_id: i64,
) -> Result<Option<Issue>, LificError> {
    match super::get_issue(conn, issue_id) {
        Ok(mut issue) if issue.project_id == project.id => {
            scrub_issue(project, &mut issue);
            Ok(Some(issue))
        }
        Ok(_) | Err(LificError::NotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Resolve `LIF-42` within the published project. The prefix must name the
/// project in the path: a guessed cross-project identifier is inert here
/// rather than resolved and then filtered.
pub fn public_issue_by_identifier(
    conn: &Connection,
    project: &Project,
    identifier: &str,
) -> Result<Option<Issue>, LificError> {
    if !names_project(&project.identifier, identifier) {
        return Ok(None);
    }
    match super::resolve_identifier(conn, identifier) {
        Ok(id) => public_issue(conn, project, id),
        Err(LificError::NotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

// ── Pages ────────────────────────────────────────────────────

/// A live page of the published project, by row id. Workspace pages have no
/// project and are never public.
pub fn public_page(
    conn: &Connection,
    project: &Project,
    page_id: i64,
) -> Result<Option<Page>, LificError> {
    match super::get_page(conn, page_id) {
        Ok(page) if page.project_id == Some(project.id) => Ok(Some(page)),
        Ok(_) | Err(LificError::NotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

// ── Comments ─────────────────────────────────────────────────

/// Whether `parent` is a live issue or page of the published project.
pub fn public_parent_exists(
    conn: &Connection,
    project: &Project,
    parent: CommentParent,
) -> Result<bool, LificError> {
    Ok(match parent {
        CommentParent::Issue(id) => public_issue(conn, project, id)?.is_some(),
        CommentParent::Page(id) => public_page(conn, project, id)?.is_some(),
    })
}

/// One page of a live parent's live comments, scrubbed. The caller has
/// already established the parent with [`public_parent_exists`]; the paging
/// contract (limit, offset, keyset cursor, `has_more`) is the private one
/// minus the `author` filter, which would let a stranger confirm usernames
/// one guess at a time.
pub fn public_comments(
    conn: &Connection,
    parent: CommentParent,
    order: Option<&str>,
    limit: Option<i64>,
    offset: Option<i64>,
    before: Option<&CommentCursor>,
) -> Result<CommentPage, LificError> {
    let mut page =
        super::comments::list_comments_keyset(conn, parent, None, order, limit, offset, before)?;
    for comment in &mut page.items {
        scrub_comment(comment);
    }
    Ok(page)
}

// ── Attachments ──────────────────────────────────────────────

/// The attachments linked to a live entity of the published project. The
/// entity is re-checked here rather than trusted from the path.
pub fn public_entity_attachments(
    conn: &Connection,
    project: &Project,
    entity: AttachmentEntity,
    entity_id: i64,
) -> Result<Option<Vec<Attachment>>, LificError> {
    let live = match entity {
        AttachmentEntity::Issue => public_issue(conn, project, entity_id)?.is_some(),
        AttachmentEntity::Page => public_page(conn, project, entity_id)?.is_some(),
        AttachmentEntity::Comment => public_comment_is_live(conn, project, entity_id)?,
    };
    if !live {
        return Ok(None);
    }
    let mut items = super::attachments::list_for_entity(conn, entity, entity_id)?;
    for attachment in &mut items {
        scrub_attachment(attachment);
    }
    Ok(Some(items))
}

/// A live comment whose live parent belongs to the published project.
fn public_comment_is_live(
    conn: &Connection,
    project: &Project,
    comment_id: i64,
) -> Result<bool, LificError> {
    Ok(conn
        .prepare_cached(
            "SELECT 1
               FROM comments c
               LEFT JOIN issues i ON i.id = c.issue_id AND i.deleted_at IS NULL
               LEFT JOIN pages pg ON pg.id = c.page_id AND pg.deleted_at IS NULL
              WHERE c.id = ?1
                AND c.deleted_at IS NULL
                AND COALESCE(i.project_id, pg.project_id) = ?2",
        )?
        .query_row(params![comment_id, project.id], |_| Ok(()))
        .optional()?
        .is_some())
}

/// The attachment, if it is linked to a live issue, a live page, or a live
/// comment on either, inside the published project.
///
/// Re-derived from the link graph on every call: the attachment id space is
/// global and countable, so an id lifted from a private project, orphaned, or
/// whose parent was deleted a moment ago must answer `None` exactly as a
/// nonexistent one does. An attachment shared between a private and a
/// published project does match through the published path, correctly:
/// publishing an issue publishes the files its body embeds.
pub fn public_attachment(
    conn: &Connection,
    project: &Project,
    attachment_id: i64,
) -> Result<Option<Attachment>, LificError> {
    let reachable = conn
        .prepare_cached(
            "SELECT 1
               FROM attachment_links l
              WHERE l.attachment_id = ?1
                AND (
                     (l.entity_type = 'issue' AND EXISTS (
                          SELECT 1 FROM issues i
                           WHERE i.id = l.entity_id
                             AND i.deleted_at IS NULL
                             AND i.project_id = ?2))
                  OR (l.entity_type = 'page' AND EXISTS (
                          SELECT 1 FROM pages pg
                           WHERE pg.id = l.entity_id
                             AND pg.deleted_at IS NULL
                             AND pg.project_id = ?2))
                  OR (l.entity_type = 'comment' AND EXISTS (
                          SELECT 1 FROM comments c
                           LEFT JOIN issues i ON i.id = c.issue_id AND i.deleted_at IS NULL
                           LEFT JOIN pages pg ON pg.id = c.page_id AND pg.deleted_at IS NULL
                           WHERE c.id = l.entity_id
                             AND c.deleted_at IS NULL
                             AND COALESCE(i.project_id, pg.project_id) = ?2))
                )
              LIMIT 1",
        )?
        .query_row(params![attachment_id, project.id], |_| Ok(()))
        .optional()?
        .is_some();
    if !reachable {
        return Ok(None);
    }
    match super::attachments::get_attachment(conn, attachment_id) {
        Ok(mut attachment) => {
            scrub_attachment(&mut attachment);
            Ok(Some(attachment))
        }
        Err(LificError::NotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::*;

    fn fixture() -> (crate::db::DbPool, Project, Project) {
        let db = crate::db::open_memory().unwrap();
        let (public, private) = {
            let conn = db.write().unwrap();
            conn.execute(
                "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot)
                 VALUES ('owner', 'owner@test.local', 'x', 'Owner', 1, 0)",
                [],
            )
            .unwrap();
            let public = super::super::create_project(
                &conn,
                &CreateProject {
                    name: "Public".into(),
                    identifier: "PUB".into(),
                    lead_user_id: Some(1),
                    ..Default::default()
                },
            )
            .unwrap();
            conn.execute(
                "UPDATE projects SET is_public = 1 WHERE id = ?1",
                [public.id],
            )
            .unwrap();
            let private = super::super::create_project(
                &conn,
                &CreateProject {
                    name: "Private".into(),
                    identifier: "PRIV".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            (public, private)
        };
        (db, public, private)
    }

    fn issue(db: &crate::db::DbPool, project_id: i64, title: &str) -> Issue {
        let conn = db.write().unwrap();
        super::super::create_issue(
            &conn,
            &CreateIssue {
                project_id,
                title: title.into(),
                source: Some(format!("github:acme/private#{title}")),
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn a_private_project_resolves_to_nothing_and_a_public_one_hides_its_lead() {
        let (db, _, _) = fixture();
        let conn = db.read().unwrap();
        assert!(public_project(&conn, "PRIV").unwrap().is_none());
        assert!(public_project(&conn, "NOPE").unwrap().is_none());
        let project = public_project(&conn, "pub").unwrap().expect("published");
        assert_eq!(project.identifier, "PUB");
        assert_eq!(project.lead_user_id, None, "the lead is account metadata");
    }

    #[test]
    fn cross_project_relations_and_import_provenance_are_scrubbed() {
        let (db, public, private) = fixture();
        let mine = issue(&db, public.id, "Mine");
        let sibling = issue(&db, public.id, "Sibling");
        let theirs = issue(&db, private.id, "Theirs");
        {
            let conn = db.write().unwrap();
            super::super::link_issues(&conn, mine.id, sibling.id, "blocks").unwrap();
            super::super::link_issues(&conn, mine.id, theirs.id, "blocks").unwrap();
            super::super::link_issues(&conn, theirs.id, mine.id, "relates_to").unwrap();
        }
        let conn = db.read().unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        let project = public_project(&tx, "PUB").unwrap().unwrap();
        let got = public_issue_by_identifier(&tx, &project, "PUB-1")
            .unwrap()
            .expect("live issue");
        assert_eq!(got.blocks, vec!["PUB-2".to_string()]);
        assert!(got.relates_to.is_empty(), "PRIV-1 must not be named");
        assert_eq!(got.source, None);

        // Naming the other project's issue through this project's path is
        // inert, as is the other project's own path.
        assert!(
            public_issue_by_identifier(&tx, &project, "PRIV-1")
                .unwrap()
                .is_none()
        );
        assert!(public_issue(&tx, &project, theirs.id).unwrap().is_none());
    }

    #[test]
    fn the_index_cursor_is_project_local_and_changes_carry_no_comments() {
        let (db, public, private) = fixture();
        let mine = issue(&db, public.id, "Mine");
        {
            let conn = db.write().unwrap();
            super::super::comments::create_comment_with_mentions(
                &conn,
                CommentParent::Issue(mine.id),
                None,
                CommentActor {
                    user_id: 1,
                    is_admin: true,
                },
                AttachmentActor::TrustedLocal,
                "hello",
                false,
            )
            .unwrap();
        }
        // A write elsewhere advances the instance counter; the public cursor
        // must not follow it.
        let _ = issue(&db, private.id, "Theirs");

        let conn = db.read().unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        let project = public_project(&tx, "PUB").unwrap().unwrap();
        let index = public_index(&tx, &project).unwrap();
        let global = super::super::changes::index_cursor(&tx).unwrap();
        assert_eq!(index.issues.len(), 1);
        // The cursor covers the comment too (its own seq is above its
        // parent's), and nothing outside this project.
        assert!(index.cursor >= index.issues[0].seq);
        assert!(
            index.cursor < global,
            "the instance counter must stay private"
        );

        let changes = public_changes(&tx, &project, 0, 100).unwrap();
        assert!(
            changes
                .changes
                .iter()
                .all(|change| !matches!(change, Change::Comment(_))),
            "comment rows carry a user id"
        );
        // The cursor still moved past the comment that was dropped.
        assert_eq!(changes.cursor, index.cursor);
        // Nothing above the snapshot cursor: a client resuming from it is
        // quiet until this project changes again.
        let quiet = public_changes(&tx, &project, index.cursor, 100).unwrap();
        assert!(quiet.changes.is_empty());
        assert_eq!(quiet.cursor, index.cursor);
    }

    #[test]
    fn comments_keep_the_display_name_and_lose_the_account() {
        let (db, public, _) = fixture();
        let mine = issue(&db, public.id, "Mine");
        {
            let conn = db.write().unwrap();
            super::super::comments::create_comment_with_mentions(
                &conn,
                CommentParent::Issue(mine.id),
                None,
                CommentActor {
                    user_id: 1,
                    is_admin: true,
                },
                AttachmentActor::TrustedLocal,
                "hello",
                false,
            )
            .unwrap();
        }
        let conn = db.read().unwrap();
        let page =
            public_comments(&conn, CommentParent::Issue(mine.id), None, None, None, None).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].author_display_name, "Owner");
        assert_eq!(page.items[0].author, "");
        assert_eq!(page.items[0].user_id, 0);
    }

    #[test]
    fn names_project_is_strict() {
        assert!(names_project("LIF", "LIF-42"));
        assert!(names_project("lif", "LIF-42"));
        assert!(!names_project("LIF", "LIF-DOC-3"));
        assert!(!names_project("LIF", "LIFX-1"));
        assert!(!names_project("LIF", "LIF-"));
        assert!(!names_project("LIF", "LIF--1"));
        assert!(!names_project("LIF", "OTHER-1"));
    }
}
