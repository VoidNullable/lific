//! LIF-262: attachment metadata + link bookkeeping.
//!
//! Pure data access over the `attachments` / `attachment_links` tables
//! (migration 031). No file I/O and no authorization live here — the API
//! layer (`api::attachments`) owns byte storage (`crate::storage`) and the
//! project-role gate. Bytes are content-addressed on disk; a row here just
//! records the metadata and the `sha256` that points at the blob.

use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, named_params, params, types::Value};

use crate::db::models::{
    Attachment, AttachmentEntity, CommentActor, LinkedEntity, PendingOrphan, ProjectAttachment,
    ProjectAttachmentPage, ProjectAttachmentQuery,
};
use crate::error::LificError;

/// Insert a new attachment metadata row and return it. The caller has already
/// written the bytes to the content-addressed store and computed `sha256`.
pub fn create_attachment(
    conn: &Connection,
    sha256: &str,
    filename: &str,
    mime: &str,
    size_bytes: i64,
    uploader_id: Option<i64>,
) -> Result<Attachment, LificError> {
    conn.execute(
        "INSERT INTO attachments (sha256, filename, mime, size_bytes, uploader_id)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![sha256, filename, mime, size_bytes, uploader_id],
    )?;
    get_attachment(conn, conn.last_insert_rowid())
}

/// The column list every attachment read shares, in the order
/// [`row_to_attachment`] expects. Kept in one place so adding a column is a
/// single edit rather than a hunt through four query strings.
const ATTACHMENT_COLUMNS: &str =
    "id, sha256, filename, mime, size_bytes, uploader_id, created_at, width, height, alt_text";

/// Fetch one attachment by id. `NotFound` when it doesn't exist.
pub fn get_attachment(conn: &Connection, id: i64) -> Result<Attachment, LificError> {
    conn.prepare_cached(&format!(
        "SELECT {ATTACHMENT_COLUMNS} FROM attachments WHERE id = ?1"
    ))?
    .query_row(params![id], row_to_attachment)
    .optional()?
    .ok_or_else(|| LificError::NotFound(format!("attachment {id} not found")))
}

/// [`ATTACHMENT_COLUMNS`] with a table alias prefix, for joined queries where
/// bare column names would be ambiguous. Keeps every read on the one shared
/// column list so [`row_to_attachment`] never sees a short row (the exact
/// failure mode that broke `list_for_project` when columns were added).
fn prefixed_attachment_columns(alias: &str) -> String {
    ATTACHMENT_COLUMNS
        .split(", ")
        .map(|c| format!("{alias}.{c}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn row_to_attachment(row: &rusqlite::Row) -> rusqlite::Result<Attachment> {
    let mime: String = row.get(3)?;
    let width: Option<i64> = row.get(7)?;
    let height: Option<i64> = row.get(8)?;
    Ok(Attachment {
        id: row.get(0)?,
        sha256: row.get(1)?,
        filename: row.get(2)?,
        has_thumbnail: crate::storage::expects_thumbnail(&mime, width, height),
        mime,
        size_bytes: row.get(4)?,
        uploader_id: row.get(5)?,
        created_at: row.get(6)?,
        width,
        height,
        alt_text: row.get(9)?,
    })
}

/// Record the decoded pixel dimensions of a raster upload (LIF-418). Called
/// immediately after `create_attachment`, inside the same write transaction,
/// once the bytes have been decoded.
pub fn set_dimensions(
    conn: &Connection,
    id: i64,
    width: i64,
    height: i64,
) -> Result<(), LificError> {
    conn.execute(
        "UPDATE attachments SET width = ?2, height = ?3 WHERE id = ?1",
        params![id, width, height],
    )?;
    Ok(())
}

/// Set (or clear, with `None`) an attachment's accessibility description and
/// return the updated row.
pub fn update_alt_text(
    conn: &Connection,
    id: i64,
    alt_text: Option<&str>,
) -> Result<Attachment, LificError> {
    let changed = conn.execute(
        "UPDATE attachments SET alt_text = ?2 WHERE id = ?1",
        params![id, alt_text],
    )?;
    if changed == 0 {
        return Err(LificError::NotFound(format!("attachment {id} not found")));
    }
    get_attachment(conn, id)
}

/// Every OTHER attachment row pointing at the same bytes. Uploading the same
/// screenshot into two projects produces two rows over one blob, and this is
/// how `GET /api/attachments/{id}/links` finds the twin so a caller can see
/// the file is already in the tracker before uploading a third copy.
pub fn duplicates_of(conn: &Connection, id: i64, sha256: &str) -> Result<Vec<Attachment>, LificError> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {ATTACHMENT_COLUMNS} FROM attachments
         WHERE sha256 = ?1 AND id != ?2 ORDER BY id"
    ))?;
    let rows = stmt.query_map(params![sha256, id], row_to_attachment)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// The raw `(entity_type, entity_id)` link rows for one attachment, in a
/// stable order.
pub fn links_for_attachment(
    conn: &Connection,
    attachment_id: i64,
) -> Result<Vec<(String, i64)>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT entity_type, entity_id FROM attachment_links
         WHERE attachment_id = ?1 ORDER BY entity_type, entity_id",
    )?;
    let rows = stmt.query_map(params![attachment_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// Delete an attachment row by id. Its `attachment_links` rows cascade via the
/// FK. Returns whether a row was removed.
pub fn delete_attachment(conn: &Connection, id: i64) -> Result<bool, LificError> {
    let changed = conn.execute("DELETE FROM attachments WHERE id = ?1", params![id])?;
    Ok(changed > 0)
}

/// How many `attachments` rows still reference a given content hash. Used by
/// the orphan GC to decide whether the sidecar blob can be removed: bytes are
/// shared across rows, so a file is only deletable when this hits zero.
pub fn count_rows_for_sha(conn: &Connection, sha256: &str) -> Result<i64, LificError> {
    conn.query_row(
        "SELECT COUNT(*) FROM attachments WHERE sha256 = ?1",
        params![sha256],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

// ── Links ────────────────────────────────────────────────────

/// Record that `entity` references `attachment_id`. Idempotent (the composite
/// PK makes a repeat a no-op via ON CONFLICT).
pub fn link_attachment(
    conn: &Connection,
    attachment_id: i64,
    entity: AttachmentEntity,
    entity_id: i64,
) -> Result<(), LificError> {
    conn.execute(
        "INSERT INTO attachment_links (attachment_id, entity_type, entity_id)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(attachment_id, entity_type, entity_id) DO NOTHING",
        params![attachment_id, entity.as_str(), entity_id],
    )?;
    Ok(())
}

/// Remove one link. Silent when the link doesn't exist.
pub fn unlink_attachment(
    conn: &Connection,
    attachment_id: i64,
    entity: AttachmentEntity,
    entity_id: i64,
) -> Result<(), LificError> {
    conn.execute(
        "DELETE FROM attachment_links
         WHERE attachment_id = ?1 AND entity_type = ?2 AND entity_id = ?3",
        params![attachment_id, entity.as_str(), entity_id],
    )?;
    Ok(())
}

/// List the attachments linked to a given entity, newest-linked last (stable
/// display order for the detail-view "Attachments (n)" section).
pub fn list_for_entity(
    conn: &Connection,
    entity: AttachmentEntity,
    entity_id: i64,
) -> Result<Vec<Attachment>, LificError> {
    let columns = prefixed_attachment_columns("a");
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {columns}
         FROM attachments a
         JOIN attachment_links l ON l.attachment_id = a.id
         WHERE l.entity_type = ?1 AND l.entity_id = ?2
         ORDER BY l.created_at, a.id"
    ))?;
    let rows = stmt.query_map(params![entity.as_str(), entity_id], row_to_attachment)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// Every entity an attachment is linked to, in link order. Unknown
/// `entity_type` values (impossible under the table's CHECK, but cheap to be
/// defensive about) are skipped rather than erroring.
pub fn links_for(
    conn: &Connection,
    attachment_id: i64,
) -> Result<Vec<(AttachmentEntity, i64)>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT entity_type, entity_id FROM attachment_links
         WHERE attachment_id = ?1
         ORDER BY created_at, entity_id",
    )?;
    let rows = stmt.query_map(params![attachment_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut links = Vec::new();
    for row in rows {
        let (entity_type, entity_id) = row?;
        if let Ok(entity) = entity_type.parse::<AttachmentEntity>() {
            links.push((entity, entity_id));
        }
    }
    Ok(links)
}

/// Every attachment linked to any entity owned by `project_id`, deduplicated
/// (one attachment can be linked into several entities in the same project).
/// Comment links resolve through the comment's parent issue or page, matching
/// how `attachment_allowed_for_project` scopes an attachment to a project.
pub fn list_for_project(conn: &Connection, project_id: i64) -> Result<Vec<Attachment>, LificError> {
    let columns = prefixed_attachment_columns("a");
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT DISTINCT {columns}
         FROM attachments a
         JOIN attachment_links l ON l.attachment_id = a.id
         LEFT JOIN issues i ON l.entity_type = 'issue' AND i.id = l.entity_id
         LEFT JOIN pages p ON l.entity_type = 'page' AND p.id = l.entity_id
         LEFT JOIN comments c ON l.entity_type = 'comment' AND c.id = l.entity_id
         LEFT JOIN issues ci ON ci.id = c.issue_id
         LEFT JOIN pages cp ON cp.id = c.page_id
         WHERE ?1 IN (i.project_id, p.project_id, ci.project_id, cp.project_id)
         ORDER BY a.id"
    ))?;
    let rows = stmt.query_map(params![project_id], row_to_attachment)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// Replace an entity's link set to exactly the given attachment ids. Adds
/// missing links, removes ones no longer referenced. Called after an
/// issue/page description or a comment is saved, with the ids parsed out of the
/// markdown (`/api/attachments/{id}` references). This is the "re-scan on save"
/// mechanism: the source of truth for which attachments an entity uses is the
/// entity's own text, and this reconciles the join table to match.
pub fn sync_entity_links(
    conn: &Connection,
    entity: AttachmentEntity,
    entity_id: i64,
    referenced_ids: &[i64],
    user_id: i64,
    is_admin: bool,
    project_id: Option<i64>,
) -> Result<(), LificError> {
    // Current links for this entity.
    let mut stmt = conn.prepare_cached(
        "SELECT attachment_id FROM attachment_links WHERE entity_type = ?1 AND entity_id = ?2",
    )?;
    let current: Vec<i64> = stmt
        .query_map(params![entity.as_str(), entity_id], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;

    // Remove links whose attachment id is no longer referenced.
    for existing in &current {
        if !referenced_ids.contains(existing) {
            unlink_attachment(conn, *existing, entity, entity_id)?;
        }
    }
    // Add newly-referenced links (skip ids that don't correspond to a real
    // attachment row — a stale/typo reference in the text shouldn't create a
    // dangling link).
    for id in referenced_ids {
        if !current.contains(id)
            && attachment_allowed_for_entity(conn, *id, user_id, is_admin, project_id)?
        {
            link_attachment(conn, *id, entity, entity_id)?;
        }
    }
    Ok(())
}

/// An attachment may be introduced into a document only by its uploader (or
/// an administrator), or when it is already linked to another entity in the
/// same project. This prevents a user who can edit one document from guessing
/// another user's unlinked attachment id and importing it into that document.
fn attachment_allowed_for_entity(
    conn: &Connection,
    attachment_id: i64,
    user_id: i64,
    is_admin: bool,
    project_id: Option<i64>,
) -> Result<bool, LificError> {
    if is_admin {
        return attachment_exists(conn, attachment_id);
    }
    let owned: Option<Option<i64>> = conn
        .query_row(
            "SELECT uploader_id FROM attachments WHERE id = ?1",
            params![attachment_id],
            |row| row.get(0),
        )
        .optional()?;
    if owned == Some(Some(user_id)) {
        return Ok(true);
    }
    let Some(project_id) = project_id else {
        return Ok(false);
    };
    let linked: Option<i64> = conn
        .query_row(
            "SELECT 1
             FROM attachment_links l
             WHERE l.attachment_id = ?1
               AND (
                 (l.entity_type = 'issue' AND EXISTS
                    (SELECT 1 FROM issues i WHERE i.id = l.entity_id AND i.project_id = ?2))
                 OR (l.entity_type = 'page' AND EXISTS
                    (SELECT 1 FROM pages p WHERE p.id = l.entity_id AND p.project_id = ?2))
                 OR (l.entity_type = 'comment' AND EXISTS
                    (SELECT 1 FROM comments c
                     LEFT JOIN issues i ON i.id = c.issue_id
                     LEFT JOIN pages p ON p.id = c.page_id
                     WHERE c.id = l.entity_id
                       AND (i.project_id = ?2 OR p.project_id = ?2)))
               )
             LIMIT 1",
            params![attachment_id, project_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(linked.is_some())
}

fn attachment_exists(conn: &Connection, id: i64) -> Result<bool, LificError> {
    let exists: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM attachments WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(exists.is_some())
}

/// Whether a caller may introduce an attachment link into `project_id`.
/// Uploaders and administrators may place their attachment anywhere they can
/// edit; other callers may only reuse attachments already linked in the same
/// project.
fn attachment_allowed_for_project(
    conn: &Connection,
    attachment_id: i64,
    actor: CommentActor,
    project_id: Option<i64>,
) -> Result<bool, LificError> {
    Ok(conn.query_row(
        "SELECT EXISTS (
             SELECT 1
             FROM attachments a
             WHERE a.id = :attachment_id
               AND (
                   :is_admin
                   OR a.uploader_id = :actor_id
                   OR EXISTS (
                       SELECT 1
                       FROM attachment_links l
                       LEFT JOIN issues i
                         ON l.entity_type = 'issue' AND i.id = l.entity_id
                       LEFT JOIN pages p
                         ON l.entity_type = 'page' AND p.id = l.entity_id
                       LEFT JOIN comments c
                         ON l.entity_type = 'comment' AND c.id = l.entity_id
                       LEFT JOIN issues ci ON ci.id = c.issue_id
                       LEFT JOIN pages cp ON cp.id = c.page_id
                       WHERE l.attachment_id = a.id
                         AND :project_id IN (
                             i.project_id,
                             p.project_id,
                             ci.project_id,
                             cp.project_id
                         )
                   )
               )
         )",
        named_params! {
            ":attachment_id": attachment_id,
            ":actor_id": actor.user_id,
            ":is_admin": actor.is_admin,
            ":project_id": project_id,
        },
        |row| row.get(0),
    )?)
}

// ── Orphan GC ────────────────────────────────────────────────

/// One collectable orphan: an attachment row with zero links, older than the
/// grace window. Carries `sha256` so the sweep can decide whether to delete the
/// sidecar file (only when no OTHER row shares the hash).
#[derive(Debug, Clone)]
pub struct OrphanAttachment {
    pub id: i64,
    pub sha256: String,
}

/// Find attachments with no links whose `created_at` is older than
/// `grace_seconds` ago. The grace window keeps a just-uploaded attachment
/// (linked a moment later, once its markdown is saved) from being swept out
/// from under an in-progress compose.
pub fn find_orphans(
    conn: &Connection,
    grace_seconds: i64,
) -> Result<Vec<OrphanAttachment>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT a.id, a.sha256
         FROM attachments a
         LEFT JOIN attachment_links l ON l.attachment_id = a.id
         WHERE l.attachment_id IS NULL
           AND a.created_at < datetime('now', ?1)",
    )?;
    // A negative grace means "collect everything, even brand-new" (used by
    // tests) — expressed as a positive offset into the future so the
    // `created_at < cutoff` comparison passes for just-created rows. SQLite's
    // datetime modifier needs an explicit sign, and `--1` is invalid, so build
    // the sign ourselves rather than interpolating a bare negative.
    let modifier = if grace_seconds >= 0 {
        format!("-{grace_seconds} seconds")
    } else {
        format!("+{} seconds", -grace_seconds)
    };
    let rows = stmt.query_map(params![modifier], |row| {
        Ok(OrphanAttachment {
            id: row.get(0)?,
            sha256: row.get(1)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

// ── Project files manager (LIF-418) ──────────────────────────
//
// The per-project Files view answers two questions the detail-view section
// can't: "what is attached anywhere in this project?" and "what did we upload
// that is about to be swept?". Both are pure reads assembled here; the API
// layer owns the Viewer gate.

/// Coarse MIME buckets the files manager filters and iconifies by.
///
/// One SQL expression is the single source of truth: the same CASE both
/// filters rows and labels them in the response, so a filter chip can never
/// select a class the returned rows disagree with.
const MIME_CLASS_SQL: &str = "CASE
        WHEN a.mime LIKE 'image/%' THEN 'image'
        WHEN a.mime LIKE 'video/%' THEN 'video'
        WHEN a.mime LIKE 'audio/%' THEN 'audio'
        WHEN a.mime LIKE 'text/%' THEN 'text'
        WHEN a.mime = 'application/pdf' THEN 'pdf'
        WHEN a.mime IN (
            'application/zip', 'application/gzip', 'application/x-tar',
            'application/x-7z-compressed', 'application/x-bzip2',
            'application/vnd.rar', 'application/x-rar-compressed'
        ) THEN 'archive'
        ELSE 'other'
    END";

/// The accepted `mime_class` filter values, in the order the UI shows them.
pub const MIME_CLASSES: &[&str] = &["image", "video", "audio", "text", "pdf", "archive", "other"];

/// `EXISTS` fragment: does `a` (an `attachments` row) have at least one link
/// reaching `:project_id`? A comment link resolves through the comment's
/// parent issue/page, mirroring `attachment_allowed_for_project`.
fn project_link_exists(filter_entity_type: bool) -> String {
    let entity_clause = if filter_entity_type {
        "AND l.entity_type = :entity_type"
    } else {
        ""
    };
    format!(
        "EXISTS (
             SELECT 1
             FROM attachment_links l
             LEFT JOIN issues   i   ON l.entity_type = 'issue'   AND i.id   = l.entity_id
             LEFT JOIN pages    pg  ON l.entity_type = 'page'    AND pg.id  = l.entity_id
             LEFT JOIN comments c   ON l.entity_type = 'comment' AND c.id   = l.entity_id
             LEFT JOIN issues   ci  ON ci.id  = c.issue_id
             LEFT JOIN pages    cpg ON cpg.id = c.page_id
             WHERE l.attachment_id = a.id
               {entity_clause}
               AND :project_id IN (i.project_id, pg.project_id, ci.project_id, cpg.project_id)
         )"
    )
}

/// `ORDER BY` fragment for the listing. Fixed strings only — never
/// interpolated user input.
fn listing_order_clause(sort: Option<&str>, order: Option<&str>) -> Result<String, LificError> {
    let (column, default_desc) = match sort {
        None | Some("created_at") => ("a.created_at", true),
        Some("size") => ("a.size_bytes", true),
        Some("filename") => ("a.filename COLLATE NOCASE", false),
        Some(other) => {
            return Err(LificError::BadRequest(format!(
                "invalid sort '{other}'. Use created_at, size, or filename."
            )));
        }
    };
    let descending = match order {
        None => default_desc,
        Some("desc") => true,
        Some("asc") => false,
        Some(other) => {
            return Err(LificError::BadRequest(format!(
                "invalid order '{other}'. Use asc or desc."
            )));
        }
    };
    // `a.id` is the stable tiebreak so paging can't repeat or skip a row when
    // two files share a timestamp, size, or name.
    Ok(format!(
        "ORDER BY {column} {}, a.id DESC",
        if descending { "DESC" } else { "ASC" }
    ))
}

type NamedParams = Vec<(&'static str, Box<dyn rusqlite::ToSql>)>;

/// Bind a `NamedParams` for one `query_map` call.
fn bind(params: &NamedParams) -> Vec<(&str, &dyn rusqlite::ToSql)> {
    params.iter().map(|(k, v)| (*k, v.as_ref())).collect()
}

/// Every attachment linked to any entity in `project_id`, one page at a time,
/// with the aggregate count + byte total for the whole filtered set.
pub fn list_project_attachments(
    conn: &Connection,
    project_id: i64,
    query: &ProjectAttachmentQuery,
) -> Result<ProjectAttachmentPage, LificError> {
    // Validate the enum-ish filters up front so a typo errors instead of
    // silently widening the result set.
    if let Some(class) = query.mime_class.as_deref()
        && !MIME_CLASSES.contains(&class)
    {
        return Err(LificError::BadRequest(format!(
            "invalid mime_class '{class}'. Use one of: {}.",
            MIME_CLASSES.join(", ")
        )));
    }
    if let Some(entity_type) = query.entity_type.as_deref() {
        entity_type
            .parse::<AttachmentEntity>()
            .map_err(LificError::BadRequest)?;
    }
    let order_clause = listing_order_clause(query.sort.as_deref(), query.order.as_deref())?;

    let (limit, offset) = super::page(query.limit, query.offset);
    let fetch = super::over_fetch(limit);

    let mut conditions = vec![project_link_exists(query.entity_type.is_some())];
    if query.mime_class.is_some() {
        conditions.push(format!("({MIME_CLASS_SQL}) = :mime_class"));
    }
    if query.uploader.is_some() {
        conditions.push("lower(u.username) = lower(:uploader)".to_string());
    }
    let where_clause = conditions.join(" AND ");

    let base_params = || -> NamedParams {
        let mut params: NamedParams = vec![(":project_id", Box::new(project_id))];
        if let Some(entity_type) = query.entity_type.clone() {
            params.push((":entity_type", Box::new(entity_type)));
        }
        if let Some(class) = query.mime_class.clone() {
            params.push((":mime_class", Box::new(class)));
        }
        if let Some(uploader) = query.uploader.clone() {
            params.push((":uploader", Box::new(uploader)));
        }
        params
    };

    let sql = format!(
        "SELECT a.id, a.filename, a.mime, ({MIME_CLASS_SQL}) AS mime_class, a.size_bytes,
                a.uploader_id, u.username, u.display_name, a.created_at
         FROM attachments a
         LEFT JOIN users u ON u.id = a.uploader_id
         WHERE {where_clause}
         {order_clause}
         LIMIT :limit OFFSET :offset"
    );
    let mut params = base_params();
    params.push((":limit", Box::new(fetch)));
    params.push((":offset", Box::new(offset)));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(bind(&params).as_slice(), |row| {
        Ok(ProjectAttachment {
            id: row.get(0)?,
            filename: row.get(1)?,
            mime: row.get(2)?,
            mime_class: row.get(3)?,
            size_bytes: row.get(4)?,
            uploader_id: row.get(5)?,
            uploader: row.get(6)?,
            uploader_display_name: row.get(7)?,
            created_at: row.get(8)?,
            entities: Vec::new(),
        })
    })?;
    let page = super::Page::from_over_fetch(rows.collect::<Result<Vec<_>, _>>()?, limit);
    let mut items = page.items;

    // Aggregate header: the whole filtered set, not this page.
    let totals_sql = format!(
        "SELECT COUNT(*), COALESCE(SUM(a.size_bytes), 0)
         FROM attachments a
         LEFT JOIN users u ON u.id = a.uploader_id
         WHERE {where_clause}"
    );
    let totals_params = base_params();
    let (total_count, total_bytes): (i64, i64) = conn.query_row(
        &totals_sql,
        bind(&totals_params).as_slice(),
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    let ids: Vec<i64> = items.iter().map(|item| item.id).collect();
    let mut entities = linked_entities_in_project(conn, project_id, &ids)?;
    for item in &mut items {
        item.entities = entities.remove(&item.id).unwrap_or_default();
    }

    Ok(ProjectAttachmentPage {
        items,
        has_more: page.has_more,
        total_count,
        total_bytes,
    })
}

/// Resolve, per attachment id, the entities *in this project* that reference
/// it — identifier and title included, so a row can render navigable chips
/// without a second round trip.
pub fn linked_entities_in_project(
    conn: &Connection,
    project_id: i64,
    attachment_ids: &[i64],
) -> Result<std::collections::HashMap<i64, Vec<LinkedEntity>>, LificError> {
    let mut out: std::collections::HashMap<i64, Vec<LinkedEntity>> =
        std::collections::HashMap::new();
    if attachment_ids.is_empty() {
        return Ok(out);
    }
    // Interpolating the ids is safe: they are i64s we just read out of the
    // database, never caller-supplied text.
    let id_list = attachment_ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");

    let sql = format!(
        "        SELECT l.attachment_id, l.entity_type, l.entity_id,
                pi.identifier,  i.sequence,   i.title,
                pp.identifier,  pg.sequence,  pg.title,
                c.issue_id, c.page_id,
                cip.identifier, ci.sequence,  ci.title,
                cpp.identifier, cpg.sequence, cpg.title,
                pg.id, cpg.id
         FROM attachment_links l
         LEFT JOIN issues   i   ON l.entity_type = 'issue'   AND i.id   = l.entity_id
         LEFT JOIN projects pi  ON pi.id  = i.project_id
         LEFT JOIN pages    pg  ON l.entity_type = 'page'    AND pg.id  = l.entity_id
         LEFT JOIN projects pp  ON pp.id  = pg.project_id
         LEFT JOIN comments c   ON l.entity_type = 'comment' AND c.id   = l.entity_id
         LEFT JOIN issues   ci  ON ci.id  = c.issue_id
         LEFT JOIN projects cip ON cip.id = ci.project_id
         LEFT JOIN pages    cpg ON cpg.id = c.page_id
         LEFT JOIN projects cpp ON cpp.id = cpg.project_id
         WHERE l.attachment_id IN ({id_list})
           AND ?1 IN (i.project_id, pg.project_id, ci.project_id, cpg.project_id)
         ORDER BY l.attachment_id, l.entity_type, l.entity_id"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![project_id], |row| {
        let attachment_id: i64 = row.get(0)?;
        let entity_type: String = row.get(1)?;
        let entity_id: i64 = row.get(2)?;
        let issue_project: Option<String> = row.get(3)?;
        let issue_sequence: Option<i64> = row.get(4)?;
        let issue_title: Option<String> = row.get(5)?;
        let page_project: Option<String> = row.get(6)?;
        let page_sequence: Option<i64> = row.get(7)?;
        let page_title: Option<String> = row.get(8)?;
        let comment_issue_id: Option<i64> = row.get(9)?;
        let comment_page_id: Option<i64> = row.get(10)?;
        let comment_issue_project: Option<String> = row.get(11)?;
        let comment_issue_sequence: Option<i64> = row.get(12)?;
        let comment_issue_title: Option<String> = row.get(13)?;
        let comment_page_project: Option<String> = row.get(14)?;
        let comment_page_sequence: Option<i64> = row.get(15)?;
        let comment_page_title: Option<String> = row.get(16)?;
        let page_row_id: Option<i64> = row.get(17)?;
        let comment_page_row_id: Option<i64> = row.get(18)?;

        // A comment carries no identifier or title of its own; it renders as a
        // reference to the issue/page the thread lives on, which is where a
        // click should land.
        let (identifier, title, page) = match entity_type.as_str() {
            "issue" => (
                issue_identifier(issue_project.as_deref(), issue_sequence),
                issue_title.unwrap_or_default(),
                None,
            ),
            "page" => (
                page_identifier(page_project.as_deref(), page_sequence),
                page_title.unwrap_or_default(),
                page_row_id,
            ),
            "comment" if comment_issue_id.is_some() => (
                issue_identifier(comment_issue_project.as_deref(), comment_issue_sequence),
                comment_issue_title.unwrap_or_default(),
                None,
            ),
            "comment" if comment_page_id.is_some() => (
                page_identifier(comment_page_project.as_deref(), comment_page_sequence),
                comment_page_title.unwrap_or_default(),
                comment_page_row_id,
            ),
            _ => (None, String::new(), None),
        };
        Ok((
            attachment_id,
            LinkedEntity {
                entity_type,
                entity_id,
                identifier,
                title,
                page_id: page,
            },
        ))
    })?;

    for row in rows {
        let (attachment_id, entity) = row?;
        out.entry(attachment_id).or_default().push(entity);
    }
    Ok(out)
}

/// `PRJ-42`, or `None` when either half is missing.
pub(crate) fn issue_identifier(project: Option<&str>, sequence: Option<i64>) -> Option<String> {
    match (project, sequence) {
        (Some(project), Some(sequence)) => Some(format!("{project}-{sequence}")),
        _ => None,
    }
}

/// `PRJ-DOC-7`, or the workspace-level `DOC-7` when the page has no project.
pub(crate) fn page_identifier(project: Option<&str>, sequence: Option<i64>) -> Option<String> {
    match (project, sequence) {
        (Some(project), Some(sequence)) => Some(format!("{project}-DOC-{sequence}")),
        (None, Some(sequence)) => Some(format!("DOC-{sequence}")),
        _ => None,
    }
}

/// Uploads by this project's members that carry no links at all and are
/// therefore queued for the orphan sweeper.
///
/// "Orphan" is exactly [`find_orphans`]'s definition (an `attachments` row with
/// no `attachment_links`), minus the grace-window cutoff: the point of this
/// listing is to show the files *before* they vanish, with the countdown, so
/// it deliberately includes rows still inside the window and reports how long
/// each has left. A row with `seconds_until_sweep = 0` is one the sweeper would
/// take on its next pass.
///
/// Scoped by uploader membership because an unlinked attachment belongs to no
/// project yet — the uploader is the only association it has.
pub fn list_project_orphans(
    conn: &Connection,
    project_id: i64,
    grace_seconds: i64,
) -> Result<Vec<PendingOrphan>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT a.id, a.filename, a.mime, a.size_bytes, a.uploader_id, u.username, a.created_at,
                MAX(
                    CAST(strftime('%s', 'now') AS INTEGER)
                        - CAST(strftime('%s', a.created_at) AS INTEGER),
                    0
                ) AS age_seconds
         FROM attachments a
         JOIN project_members m ON m.user_id = a.uploader_id AND m.project_id = ?1
         LEFT JOIN users u ON u.id = a.uploader_id
         LEFT JOIN attachment_links l ON l.attachment_id = a.id
         WHERE l.attachment_id IS NULL
         ORDER BY a.created_at ASC, a.id ASC",
    )?;
    let rows = stmt.query_map(params![project_id], |row| {
        let age_seconds: i64 = row.get(7)?;
        Ok(PendingOrphan {
            id: row.get(0)?,
            filename: row.get(1)?,
            mime: row.get(2)?,
            size_bytes: row.get(3)?,
            uploader_id: row.get(4)?,
            uploader: row.get(5)?,
            uploaded_at: row.get(6)?,
            age_seconds,
            seconds_until_sweep: (grace_seconds - age_seconds).max(0),
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// Where a search hit on an attachment should send the caller: the project it
/// belongs to and the entity that references it.
#[derive(Debug, Clone)]
pub struct AttachmentLinkTarget {
    pub project_id: Option<i64>,
    pub identifier: Option<String>,
    /// Set when the link resolves to a page (directly or through a page
    /// comment), so a renderer can build a page link rather than an issue one.
    pub page_id: Option<i64>,
}

/// The project an `attachment_links` row resolves to, in terms of the alias
/// set both [`primary_link`] and [`visible_link_scope_sql`] join with. Shared
/// so the "does a visible link exist" test and the "which link do we render"
/// choice can never disagree about what a link's project is.
pub const LINK_PROJECT: &str =
    "COALESCE(i.project_id, pg.project_id, ci.project_id, cpg.project_id)";

/// The joins that resolve an `attachment_links` row (`l`) to its entity, and
/// through that to `LINK_PROJECT`.
const LINK_ENTITY_JOINS: &str =
    "LEFT JOIN issues   i   ON l.entity_type = 'issue'   AND i.id   = l.entity_id
         LEFT JOIN pages    pg  ON l.entity_type = 'page'    AND pg.id  = l.entity_id
         LEFT JOIN comments c   ON l.entity_type = 'comment' AND c.id   = l.entity_id
         LEFT JOIN issues   ci  ON ci.id  = c.issue_id
         LEFT JOIN pages    cpg ON cpg.id = c.page_id";

/// The scope test for one link row: inside `project_id` when the caller
/// narrowed to a project, and inside `visible` when the caller is restricted.
fn link_scope_predicate(
    project_id: Option<i64>,
    visible: Option<&HashSet<i64>>,
) -> (String, Vec<Value>) {
    let mut clauses = Vec::new();
    let mut params = Vec::new();
    if let Some(project_id) = project_id {
        clauses.push(format!("{LINK_PROJECT} = ?"));
        params.push(Value::Integer(project_id));
    }
    let (visibility, ids) = super::project_visibility_sql(LINK_PROJECT, visible);
    clauses.push(visibility);
    params.extend(ids.into_iter().map(Value::Integer));
    (clauses.join(" AND "), params)
}

/// `EXISTS (…)` SQL asserting that the attachment identified by
/// `attachment_column` in the enclosing query has at least one link the caller
/// is allowed to see, plus the values to bind.
///
/// Search splices this into its `WHERE` clause so scope and visibility are
/// settled before `LIMIT`. An attachment with no qualifying link is not a hit
/// at all, which also keeps never-linked uploads out of results.
pub fn visible_link_scope_sql(
    attachment_column: &str,
    project_id: Option<i64>,
    visible: Option<&HashSet<i64>>,
) -> (String, Vec<Value>) {
    let (predicate, params) = link_scope_predicate(project_id, visible);
    let sql = format!(
        "EXISTS (
             SELECT 1
             FROM attachment_links l
             {LINK_ENTITY_JOINS}
             WHERE l.attachment_id = {attachment_column}
               AND {predicate}
         )"
    );
    (sql, params)
}

/// The single entity a search hit should point at, preferring a link inside
/// `prefer_project` when the attachment is used in several places. `None` for
/// an attachment with no links at all — those never surface in search, since
/// there is no project to authorize them against.
pub fn primary_link(
    conn: &Connection,
    attachment_id: i64,
    prefer_project: Option<i64>,
) -> Result<Option<AttachmentLinkTarget>, LificError> {
    select_link(conn, attachment_id, prefer_project, "1=1", Vec::new())
}

/// [`primary_link`] restricted to links the caller may see.
///
/// The difference matters for a file shared between a visible and a hidden
/// project: preferring a visible link is not enough, because the fallback
/// would render the hidden project's identifier and leak both the entity's
/// existence and its title. Here `project_id` and `visible` are hard filters
/// on the candidate set, so an out-of-scope link can never be chosen — it can
/// only make the hit disappear, which is what the same filters already did in
/// SQL when the row was selected.
pub fn visible_primary_link(
    conn: &Connection,
    attachment_id: i64,
    project_id: Option<i64>,
    visible: Option<&HashSet<i64>>,
) -> Result<Option<AttachmentLinkTarget>, LificError> {
    if visible.is_none() {
        // Nothing is hidden from this caller, so there is nothing to filter
        // out: the plain preference resolution already lands on a
        // `project_id` link when one exists, and the row would not have been
        // selected unless one did.
        return primary_link(conn, attachment_id, project_id);
    }
    let (predicate, params) = link_scope_predicate(project_id, visible);
    select_link(conn, attachment_id, project_id, &predicate, params)
}

fn select_link(
    conn: &Connection,
    attachment_id: i64,
    prefer_project: Option<i64>,
    predicate: &str,
    predicate_params: Vec<Value>,
) -> Result<Option<AttachmentLinkTarget>, LificError> {
    let sql = format!(
        "SELECT {LINK_PROJECT} AS project_id,
                l.entity_type,
                pi.identifier,  i.sequence,
                pp.identifier,  pg.sequence,  pg.id,
                c.issue_id, c.page_id,
                cip.identifier, ci.sequence,
                cpp.identifier, cpg.sequence, cpg.id
         FROM attachment_links l
         LEFT JOIN issues   i   ON l.entity_type = 'issue'   AND i.id   = l.entity_id
         LEFT JOIN projects pi  ON pi.id  = i.project_id
         LEFT JOIN pages    pg  ON l.entity_type = 'page'    AND pg.id  = l.entity_id
         LEFT JOIN projects pp  ON pp.id  = pg.project_id
         LEFT JOIN comments c   ON l.entity_type = 'comment' AND c.id   = l.entity_id
         LEFT JOIN issues   ci  ON ci.id  = c.issue_id
         LEFT JOIN projects cip ON cip.id = ci.project_id
         LEFT JOIN pages    cpg ON cpg.id = c.page_id
         LEFT JOIN projects cpp ON cpp.id = cpg.project_id
         WHERE l.attachment_id = ?
           AND {predicate}
         ORDER BY
             CASE WHEN {LINK_PROJECT} = ? THEN 0 ELSE 1 END,
             l.entity_type, l.entity_id
         LIMIT 1"
    );
    // Every placeholder is positional and bound in SQL text order: the
    // attachment, then the scope predicate's ids, then the preferred project.
    let mut values = vec![Value::Integer(attachment_id)];
    values.extend(predicate_params);
    values.push(prefer_project.map_or(Value::Null, Value::Integer));

    let mut stmt = conn.prepare_cached(&sql)?;
    let target = stmt
        .query_row(rusqlite::params_from_iter(values), |row| {
            let project_id: Option<i64> = row.get(0)?;
            let entity_type: String = row.get(1)?;
            let issue_project: Option<String> = row.get(2)?;
            let issue_sequence: Option<i64> = row.get(3)?;
            let page_project: Option<String> = row.get(4)?;
            let page_sequence: Option<i64> = row.get(5)?;
            let page_id: Option<i64> = row.get(6)?;
            let comment_issue_id: Option<i64> = row.get(7)?;
            let comment_page_id: Option<i64> = row.get(8)?;
            let comment_issue_project: Option<String> = row.get(9)?;
            let comment_issue_sequence: Option<i64> = row.get(10)?;
            let comment_page_project: Option<String> = row.get(11)?;
            let comment_page_sequence: Option<i64> = row.get(12)?;
            let comment_page_row_id: Option<i64> = row.get(13)?;

            let (identifier, page) = match entity_type.as_str() {
                "issue" => (
                    issue_identifier(issue_project.as_deref(), issue_sequence),
                    None,
                ),
                "page" => (
                    page_identifier(page_project.as_deref(), page_sequence),
                    page_id,
                ),
                "comment" if comment_issue_id.is_some() => (
                    issue_identifier(comment_issue_project.as_deref(), comment_issue_sequence),
                    None,
                ),
                "comment" if comment_page_id.is_some() => (
                    page_identifier(comment_page_project.as_deref(), comment_page_sequence),
                    comment_page_row_id,
                ),
                _ => (None, None),
            };
            Ok(AttachmentLinkTarget {
                project_id,
                identifier,
                page_id: page,
            })
        })
        .optional()?;
    Ok(target)
}

// ── Attachment search index (LIF-418, migration 042) ─────────

/// Upper bound on the text we pull into the search index for one attachment.
/// Filenames are always indexed; contents only for `text/*` uploads at or
/// under this size, so a 40 MB log can't bloat the FTS table.
pub const MAX_EXTRACT_BYTES: i64 = 512 * 1024;

/// Whether an upload's contents should be extracted into `attachments_fts`.
pub fn is_extractable(mime: &str, size_bytes: i64) -> bool {
    mime.starts_with("text/") && size_bytes <= MAX_EXTRACT_BYTES
}

/// Store an attachment's extracted text in the FTS index. The row itself is
/// created by migration 042's insert trigger, so this only fills the column
/// in; a missing row (an attachment deleted mid-flight) is a silent no-op.
pub fn set_extracted_text(
    conn: &Connection,
    attachment_id: i64,
    text: &str,
) -> Result<(), LificError> {
    conn.execute(
        "UPDATE attachments_fts SET extracted_text = ?1 WHERE attachment_id = ?2",
        params![text, attachment_id],
    )?;
    Ok(())
}

/// Text attachments whose contents are not in the index yet: uploads that
/// predate migration 042, or ones whose extraction failed. Returns
/// `(id, sha256)` so the caller can read the blob and index it.
///
/// Cheap by construction: bounded by the number of `text/*` rows, and the
/// `NOT EXISTS` short-circuits on the first indexed row per attachment.
pub fn unindexed_text_attachments(conn: &Connection) -> Result<Vec<(i64, String)>, LificError> {
    let mut stmt = conn.prepare(
        "SELECT a.id, a.sha256
         FROM attachments a
         WHERE a.mime LIKE 'text/%'
           AND a.size_bytes <= ?1
           AND NOT EXISTS (
               SELECT 1 FROM attachments_fts f
               WHERE f.attachment_id = a.id AND f.extracted_text <> ''
           )
         ORDER BY a.id",
    )?;
    let rows = stmt.query_map(params![MAX_EXTRACT_BYTES], |row| {
        Ok((row.get(0)?, row.get(1)?))
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// Parse `/api/attachments/{id}` references out of a markdown body, returning
/// the distinct attachment ids it mentions. Matches both image embeds
/// (`![alt](/api/attachments/12)`) and link chips (`[file](/api/attachments/12)`),
/// plus a bare occurrence of the path, so re-scan-on-save catches every form
/// the composer can insert.
pub fn parse_referenced_ids(markdown: &str) -> Vec<i64> {
    let mut ids = Vec::new();
    let needle = "/api/attachments/";
    let bytes = markdown.as_bytes();
    let mut search_from = 0;
    while let Some(rel) = markdown[search_from..].find(needle) {
        let start = search_from + rel + needle.len();
        // Consume the run of ASCII digits following the path.
        let mut end = start;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end > start
            && let Ok(id) = markdown[start..end].parse::<i64>()
            && !ids.contains(&id)
        {
            ids.push(id);
        }
        search_from = end.max(search_from + rel + needle.len());
    }
    ids
}

/// Re-scan an entity's markdown body for `/api/attachments/{id}` references
/// and reconcile the link table to match (LIF-262 "re-scan on save"). Called
/// from every issue/page/comment create+update path — REST handlers and MCP
/// tools alike (LIF-369) — inside their write txn. The entity's own text is
/// the source of truth for which attachments it uses; this makes the join
/// table agree.
pub fn sync_links(
    conn: &Connection,
    entity: AttachmentEntity,
    entity_id: i64,
    markdown: &str,
    user_id: i64,
    is_admin: bool,
    project_id: Option<i64>,
) -> Result<(), LificError> {
    let ids = parse_referenced_ids(markdown);
    sync_entity_links(conn, entity, entity_id, &ids, user_id, is_admin, project_id)
}

/// Reconcile attachment links without allowing references to import an
/// attachment from another project or another user's unlinked upload.
pub fn sync_links_scoped(
    conn: &Connection,
    entity: AttachmentEntity,
    entity_id: i64,
    markdown: &str,
    actor: CommentActor,
    project_id: Option<i64>,
) -> Result<(), LificError> {
    let mut allowed = Vec::new();
    for attachment_id in parse_referenced_ids(markdown) {
        if attachment_allowed_for_project(conn, attachment_id, actor, project_id)? {
            allowed.push(attachment_id);
        }
    }
    sync_entity_links(
        conn,
        entity,
        entity_id,
        &allowed,
        actor.user_id,
        actor.is_admin,
        project_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::CreateProject;
    use crate::db::{self, queries};

    fn test_db() -> db::DbPool {
        db::open_memory().expect("test db")
    }

    fn seed_user(conn: &Connection, username: &str) -> i64 {
        conn.execute(
            "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot)
             VALUES (?1, ?2, 'x', ?1, 0, 0)",
            params![username, format!("{username}@test.local")],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn seed_issue(conn: &Connection) -> i64 {
        let project = queries::create_project(
            conn,
            &CreateProject {
                name: "Att".into(),
                identifier: "ATT".into(),
                ..Default::default()
            },
        )
        .unwrap();
        queries::create_issue(
            conn,
            &crate::db::models::CreateIssue {
                project_id: project.id,
                title: "i".into(),
                description: String::new(),
                status: crate::db::models::Status::Todo,
                priority: crate::db::models::Priority::Medium,
                module_id: None,
                start_date: None,
                target_date: None,
                labels: vec![],
                source: None,
            },
        )
        .unwrap()
        .id
    }

    #[test]
    fn create_get_delete_roundtrip() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let uploader = seed_user(&conn, "up");
        let att = create_attachment(&conn, "abc123", "shot.png", "image/png", 42, Some(uploader))
            .unwrap();
        assert_eq!(att.filename, "shot.png");
        assert_eq!(att.size_bytes, 42);

        let fetched = get_attachment(&conn, att.id).unwrap();
        assert_eq!(fetched.sha256, "abc123");

        assert!(delete_attachment(&conn, att.id).unwrap());
        assert!(get_attachment(&conn, att.id).is_err());
    }

    #[test]
    fn attachment_scope_uses_actor_identity_and_admin_status() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let uploader_id = seed_user(&conn, "uploader");
        let attachment = create_attachment(
            &conn,
            "owned",
            "owned.txt",
            "text/plain",
            1,
            Some(uploader_id),
        )
        .unwrap();

        let uploader = CommentActor {
            user_id: uploader_id,
            is_admin: false,
        };
        let stranger = CommentActor {
            user_id: uploader_id + 1,
            is_admin: false,
        };
        let admin = CommentActor {
            user_id: uploader_id + 1,
            is_admin: true,
        };

        assert!(attachment_allowed_for_project(&conn, attachment.id, uploader, None).unwrap());
        assert!(!attachment_allowed_for_project(&conn, attachment.id, stranger, None).unwrap());
        assert!(attachment_allowed_for_project(&conn, attachment.id, admin, None).unwrap());
        assert!(!attachment_allowed_for_project(&conn, i64::MAX, admin, None).unwrap());
    }

    #[test]
    fn linking_is_idempotent_and_unlink_clears_it() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let issue = seed_issue(&conn);
        let att = create_attachment(&conn, "h1", "a.pdf", "application/pdf", 10, None).unwrap();

        assert!(
            list_for_entity(&conn, AttachmentEntity::Issue, issue)
                .unwrap()
                .is_empty()
        );
        link_attachment(&conn, att.id, AttachmentEntity::Issue, issue).unwrap();
        link_attachment(&conn, att.id, AttachmentEntity::Issue, issue).unwrap(); // idempotent

        let listed = list_for_entity(&conn, AttachmentEntity::Issue, issue).unwrap();
        assert_eq!(
            listed.len(),
            1,
            "the second link must not duplicate the row"
        );
        assert_eq!(listed[0].id, att.id);

        unlink_attachment(&conn, att.id, AttachmentEntity::Issue, issue).unwrap();
        assert!(
            list_for_entity(&conn, AttachmentEntity::Issue, issue)
                .unwrap()
                .is_empty()
        );
        // With no links left the attachment is collectable by the orphan GC.
        assert_eq!(find_orphans(&conn, -1).unwrap().len(), 1);
    }

    #[test]
    fn dedup_count_rows_for_sha() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        create_attachment(&conn, "same", "a.png", "image/png", 1, None).unwrap();
        create_attachment(&conn, "same", "b.png", "image/png", 1, None).unwrap();
        assert_eq!(count_rows_for_sha(&conn, "same").unwrap(), 2);
    }

    #[test]
    fn sync_entity_links_reconciles() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let issue = seed_issue(&conn);
        let a = create_attachment(&conn, "a", "a.png", "image/png", 1, None).unwrap();
        let b = create_attachment(&conn, "b", "b.png", "image/png", 1, None).unwrap();
        let c = create_attachment(&conn, "c", "c.png", "image/png", 1, None).unwrap();

        // Start linking a + b.
        sync_entity_links(&conn, AttachmentEntity::Issue, issue, &[a.id, b.id], 0, true, Some(1))
            .unwrap();
        let ids: Vec<i64> = list_for_entity(&conn, AttachmentEntity::Issue, issue)
            .unwrap()
            .into_iter()
            .map(|x| x.id)
            .collect();
        assert_eq!(ids, vec![a.id, b.id]);

        // Re-sync to b + c: a is unlinked, c is added.
        sync_entity_links(&conn, AttachmentEntity::Issue, issue, &[b.id, c.id], 0, true, Some(1))
            .unwrap();
        let mut ids: Vec<i64> = list_for_entity(&conn, AttachmentEntity::Issue, issue)
            .unwrap()
            .into_iter()
            .map(|x| x.id)
            .collect();
        ids.sort();
        let mut want = vec![b.id, c.id];
        want.sort();
        assert_eq!(ids, want);
    }

    #[test]
    fn sync_ignores_nonexistent_ids() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let issue = seed_issue(&conn);
        // 99999 doesn't exist — must not create a dangling link.
        sync_entity_links(&conn, AttachmentEntity::Issue, issue, &[99999], 0, true, Some(1))
            .unwrap();
        assert!(
            list_for_entity(&conn, AttachmentEntity::Issue, issue)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn sync_does_not_import_unlinked_attachment_from_another_user() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let owner = seed_user(&conn, "owner");
        let editor = seed_user(&conn, "editor");
        let issue = seed_issue(&conn);
        let att = create_attachment(&conn, "foreign", "f.png", "image/png", 1, Some(owner))
            .unwrap();

        sync_entity_links(
            &conn,
            AttachmentEntity::Issue,
            issue,
            &[att.id],
            editor,
            false,
            Some(1),
        )
        .unwrap();
        assert!(list_for_entity(&conn, AttachmentEntity::Issue, issue)
            .unwrap()
            .is_empty());

        // Once the owner has placed the attachment in the same project, a
        // maintainer editing another entity in that project may reference it.
        link_attachment(&conn, att.id, AttachmentEntity::Issue, issue).unwrap();
        let second = queries::create_issue(
            &conn,
            &crate::db::models::CreateIssue {
                project_id: queries::get_issue(&conn, issue).unwrap().project_id,
                title: "second".into(),
                description: String::new(),
                status: crate::db::models::Status::Todo,
                priority: crate::db::models::Priority::Medium,
                module_id: None,
                start_date: None,
                target_date: None,
                labels: vec![],
                source: None,
            },
        )
        .unwrap()
        .id;
        sync_entity_links(
            &conn,
            AttachmentEntity::Issue,
            second,
            &[att.id],
            editor,
            false,
            Some(1),
        )
        .unwrap();
        assert_eq!(
            list_for_entity(&conn, AttachmentEntity::Issue, second)
                .unwrap()
                .len(),
            1
        );
    }

    /// A reference set can be part authorized and part not, and the two
    /// outcomes are decided one attachment at a time. Committed, that is the
    /// documented filtering: the allowed reference links, the unauthorized one
    /// is dropped in silence. Aborted, it is all or nothing — the caller must
    /// never be able to observe the allowed half of a save whose gate went on
    /// to deny, which is why the filter and the link writes run on the
    /// connection that owns the transaction rather than one of their own.
    #[test]
    fn a_partly_authorized_reference_set_commits_or_rolls_back_whole() {
        let pool = test_db();
        let (issue, project_id, editor, mine, theirs) = {
            let conn = pool.write().unwrap();
            let owner = seed_user(&conn, "owner");
            let editor = seed_user(&conn, "editor");
            let issue = seed_issue(&conn);
            let project_id = queries::get_issue(&conn, issue).unwrap().project_id;
            let mine =
                create_attachment(&conn, "mine", "m.png", "image/png", 1, Some(editor)).unwrap();
            let theirs =
                create_attachment(&conn, "theirs", "t.png", "image/png", 1, Some(owner)).unwrap();
            (issue, project_id, editor, mine.id, theirs.id)
        };

        // Abort after the set is reconciled, standing in for the in-transaction
        // gate denying: nothing at all is left behind, not even the half that
        // was allowed.
        let denied: Result<(), LificError> = pool.transaction(|conn| {
            sync_entity_links(
                conn,
                AttachmentEntity::Issue,
                issue,
                &[mine, theirs],
                editor,
                false,
                Some(project_id),
            )?;
            Err(LificError::Forbidden("revoked mid-save".into()))
        });
        assert!(matches!(denied, Err(LificError::Forbidden(_))));
        {
            let conn = pool.read().unwrap();
            assert!(
                list_for_entity(&conn, AttachmentEntity::Issue, issue)
                    .unwrap()
                    .is_empty(),
                "a denied save must not commit the references it had already allowed"
            );
        }

        // The same set, committed: filtering semantics are unchanged.
        pool.transaction(|conn| {
            sync_entity_links(
                conn,
                AttachmentEntity::Issue,
                issue,
                &[mine, theirs],
                editor,
                false,
                Some(project_id),
            )
        })
        .unwrap();
        let conn = pool.read().unwrap();
        let linked: Vec<i64> = list_for_entity(&conn, AttachmentEntity::Issue, issue)
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(
            linked,
            vec![mine],
            "the uploader's own reference links; another user's unlinked upload does not"
        );
    }

    #[test]
    fn find_orphans_respects_grace_window() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let issue = seed_issue(&conn);
        let linked = create_attachment(&conn, "l", "l.png", "image/png", 1, None).unwrap();
        link_attachment(&conn, linked.id, AttachmentEntity::Issue, issue).unwrap();
        let orphan = create_attachment(&conn, "o", "o.png", "image/png", 1, None).unwrap();

        // Grace of 1 hour: the just-created orphan is too new to collect.
        assert!(find_orphans(&conn, 3600).unwrap().is_empty());

        // Grace of 0 (or negative): the unlinked orphan surfaces; the linked
        // one never does.
        let found = find_orphans(&conn, -1).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, orphan.id);
    }

    #[test]
    fn link_cascades_on_entity_delete_via_trigger() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let issue = seed_issue(&conn);
        let att = create_attachment(&conn, "h", "a.png", "image/png", 1, None).unwrap();
        link_attachment(&conn, att.id, AttachmentEntity::Issue, issue).unwrap();
        assert_eq!(
            list_for_entity(&conn, AttachmentEntity::Issue, issue)
                .unwrap()
                .len(),
            1
        );

        queries::delete_issue(&conn, issue).unwrap();
        // Trigger drops the link; the attachment row itself survives (GC's job).
        assert!(
            list_for_entity(&conn, AttachmentEntity::Issue, issue)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            find_orphans(&conn, -1).unwrap().len(),
            1,
            "the now-unlinked attachment is collectable"
        );
        assert!(get_attachment(&conn, att.id).is_ok());
    }

    // ── Project files manager (LIF-418) ──────────────────────

    fn seed_named_project(conn: &Connection, ident: &str) -> i64 {
        queries::create_project(
            conn,
            &CreateProject {
                name: format!("Project {ident}"),
                identifier: ident.into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id
    }

    fn seed_issue_in(conn: &Connection, project_id: i64, title: &str) -> i64 {
        queries::create_issue(
            conn,
            &crate::db::models::CreateIssue {
                project_id,
                title: title.into(),
                ..Default::default()
            },
        )
        .unwrap()
        .id
    }

    /// Attach `filename` (of `mime`) to an issue in `project_id`, returning the
    /// attachment id.
    fn attach_to_issue(
        conn: &Connection,
        issue_id: i64,
        sha: &str,
        filename: &str,
        mime: &str,
        size: i64,
        uploader: Option<i64>,
    ) -> i64 {
        let att = create_attachment(conn, sha, filename, mime, size, uploader).unwrap();
        link_attachment(conn, att.id, AttachmentEntity::Issue, issue_id).unwrap();
        att.id
    }

    #[test]
    fn project_listing_returns_linked_files_with_aggregate_totals() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_named_project(&conn, "FIL");
        let issue = seed_issue_in(&conn, project, "Bug with a screenshot");
        let uploader = seed_user(&conn, "uploader");
        attach_to_issue(&conn, issue, "s1", "shot.png", "image/png", 100, Some(uploader));
        attach_to_issue(&conn, issue, "s2", "trace.log", "text/plain", 250, Some(uploader));
        // An unlinked upload is NOT part of the project listing.
        create_attachment(&conn, "s3", "stray.png", "image/png", 999, Some(uploader)).unwrap();

        let page =
            list_project_attachments(&conn, project, &ProjectAttachmentQuery::default()).unwrap();
        assert_eq!(page.total_count, 2);
        assert_eq!(page.total_bytes, 350, "bytes cover the filtered set");
        assert!(!page.has_more);
        assert_eq!(page.items.len(), 2);

        let names: Vec<&str> = page.items.iter().map(|a| a.filename.as_str()).collect();
        assert!(names.contains(&"shot.png") && names.contains(&"trace.log"));
        let shot = page
            .items
            .iter()
            .find(|a| a.filename == "shot.png")
            .unwrap();
        assert_eq!(shot.mime_class, "image");
        assert_eq!(shot.uploader.as_deref(), Some("uploader"));
        assert_eq!(shot.entities.len(), 1);
        assert_eq!(shot.entities[0].entity_type, "issue");
        assert_eq!(shot.entities[0].identifier.as_deref(), Some("FIL-1"));
        assert_eq!(shot.entities[0].title, "Bug with a screenshot");
    }

    #[test]
    fn project_listing_hides_files_from_other_projects() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let mine = seed_named_project(&conn, "MIN");
        let theirs = seed_named_project(&conn, "THR");
        let my_issue = seed_issue_in(&conn, mine, "mine");
        let their_issue = seed_issue_in(&conn, theirs, "theirs");
        attach_to_issue(&conn, my_issue, "a", "mine.png", "image/png", 1, None);
        attach_to_issue(&conn, their_issue, "b", "theirs.png", "image/png", 1, None);

        let page =
            list_project_attachments(&conn, mine, &ProjectAttachmentQuery::default()).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].filename, "mine.png");
    }

    #[test]
    fn project_listing_filters_by_mime_class_uploader_and_entity_type() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_named_project(&conn, "FLT");
        let issue = seed_issue_in(&conn, project, "target");
        let alice = seed_user(&conn, "alice");
        let bob = seed_user(&conn, "bob");
        attach_to_issue(&conn, issue, "a", "a.png", "image/png", 10, Some(alice));
        attach_to_issue(&conn, issue, "b", "b.pdf", "application/pdf", 20, Some(bob));

        // A page-linked file, so the entity_type filter has something to
        // exclude.
        let page_row = queries::pages::create_page(
            &conn,
            &crate::db::models::CreatePage {
                project_id: Some(project),
                title: "Design".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let on_page = create_attachment(&conn, "c", "c.zip", "application/zip", 30, Some(alice))
            .unwrap();
        link_attachment(&conn, on_page.id, AttachmentEntity::Page, page_row.id).unwrap();

        let by_class = list_project_attachments(
            &conn,
            project,
            &ProjectAttachmentQuery {
                mime_class: Some("image".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(by_class.total_count, 1);
        assert_eq!(by_class.items[0].filename, "a.png");

        let by_uploader = list_project_attachments(
            &conn,
            project,
            &ProjectAttachmentQuery {
                // Case-insensitive on purpose: the chip shows the username as
                // stored, the URL may not.
                uploader: Some("ALICE".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(by_uploader.total_count, 2);

        let by_entity = list_project_attachments(
            &conn,
            project,
            &ProjectAttachmentQuery {
                entity_type: Some("page".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(by_entity.total_count, 1);
        assert_eq!(by_entity.items[0].filename, "c.zip");
        assert_eq!(by_entity.items[0].mime_class, "archive");
        assert_eq!(
            by_entity.items[0].entities[0].identifier.as_deref(),
            Some("FLT-DOC-1")
        );
        assert_eq!(
            by_entity.items[0].entities[0].page_id,
            Some(page_row.id),
            "a page link carries the numeric id the web route needs"
        );
    }

    #[test]
    fn project_listing_sorts_and_pages() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_named_project(&conn, "SRT");
        let issue = seed_issue_in(&conn, project, "target");
        attach_to_issue(&conn, issue, "a", "big.bin", "application/octet-stream", 900, None);
        attach_to_issue(&conn, issue, "b", "mid.png", "image/png", 500, None);
        attach_to_issue(&conn, issue, "c", "small.txt", "text/plain", 10, None);

        let by_size = list_project_attachments(
            &conn,
            project,
            &ProjectAttachmentQuery {
                sort: Some("size".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let sizes: Vec<i64> = by_size.items.iter().map(|a| a.size_bytes).collect();
        assert_eq!(sizes, vec![900, 500, 10], "size sorts largest first");
        assert_eq!(by_size.items[0].mime_class, "other");

        let by_name = list_project_attachments(
            &conn,
            project,
            &ProjectAttachmentQuery {
                sort: Some("filename".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let names: Vec<&str> = by_name.items.iter().map(|a| a.filename.as_str()).collect();
        assert_eq!(names, vec!["big.bin", "mid.png", "small.txt"]);

        // Paging: the aggregate header still describes the whole set.
        let first = list_project_attachments(
            &conn,
            project,
            &ProjectAttachmentQuery {
                sort: Some("size".into()),
                limit: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(first.items.len(), 2);
        assert!(first.has_more);
        assert_eq!(first.total_count, 3);
        assert_eq!(first.total_bytes, 1410);

        let second = list_project_attachments(
            &conn,
            project,
            &ProjectAttachmentQuery {
                sort: Some("size".into()),
                limit: Some(2),
                offset: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(second.items.len(), 1);
        assert!(!second.has_more);
        assert_eq!(second.items[0].filename, "small.txt");
    }

    #[test]
    fn project_listing_rejects_unknown_filters() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_named_project(&conn, "BAD");

        for query in [
            ProjectAttachmentQuery {
                mime_class: Some("hologram".into()),
                ..Default::default()
            },
            ProjectAttachmentQuery {
                entity_type: Some("widget".into()),
                ..Default::default()
            },
            ProjectAttachmentQuery {
                sort: Some("colour".into()),
                ..Default::default()
            },
            ProjectAttachmentQuery {
                order: Some("sideways".into()),
                ..Default::default()
            },
        ] {
            assert!(
                list_project_attachments(&conn, project, &query).is_err(),
                "unknown filter value must error: {query:?}"
            );
        }
    }

    #[test]
    fn comment_linked_file_resolves_to_its_parent_entity() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_named_project(&conn, "CMT");
        let issue = seed_issue_in(&conn, project, "Thread parent");
        let author = seed_user(&conn, "author");
        let comment = queries::comments::create_comment(
            &conn,
            queries::comments::CommentParent::Issue(issue),
            author,
            "see the log",
        )
        .unwrap();
        let att = create_attachment(&conn, "z", "log.txt", "text/plain", 5, Some(author)).unwrap();
        link_attachment(&conn, att.id, AttachmentEntity::Comment, comment.id).unwrap();

        let page =
            list_project_attachments(&conn, project, &ProjectAttachmentQuery::default()).unwrap();
        assert_eq!(page.items.len(), 1);
        let entity = &page.items[0].entities[0];
        assert_eq!(entity.entity_type, "comment");
        assert_eq!(entity.entity_id, comment.id);
        assert_eq!(
            entity.identifier.as_deref(),
            Some("CMT-1"),
            "a comment link points at the issue the thread lives on"
        );
        assert_eq!(entity.title, "Thread parent");

        // The same resolution drives search hits.
        let target = primary_link(&conn, att.id, Some(project)).unwrap().unwrap();
        assert_eq!(target.project_id, Some(project));
        assert_eq!(target.identifier.as_deref(), Some("CMT-1"));
        assert_eq!(target.page_id, None);
    }

    #[test]
    fn primary_link_prefers_the_searched_project() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let first = seed_named_project(&conn, "ONE");
        let second = seed_named_project(&conn, "TWO");
        let first_issue = seed_issue_in(&conn, first, "one");
        let second_issue = seed_issue_in(&conn, second, "two");
        let att = create_attachment(&conn, "s", "shared.png", "image/png", 1, None).unwrap();
        link_attachment(&conn, att.id, AttachmentEntity::Issue, first_issue).unwrap();
        link_attachment(&conn, att.id, AttachmentEntity::Issue, second_issue).unwrap();

        assert_eq!(
            primary_link(&conn, att.id, Some(second))
                .unwrap()
                .unwrap()
                .identifier
                .as_deref(),
            Some("TWO-1")
        );
        // No link at all: nothing to point at.
        let unlinked = create_attachment(&conn, "u", "u.png", "image/png", 1, None).unwrap();
        assert!(primary_link(&conn, unlinked.id, None).unwrap().is_none());
    }

    #[test]
    fn project_orphans_list_members_uploads_with_a_countdown() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let project = seed_named_project(&conn, "ORP");
        let issue = seed_issue_in(&conn, project, "target");
        let member = seed_user(&conn, "member");
        let stranger = seed_user(&conn, "stranger");
        queries::members::upsert_member(
            &conn,
            project,
            member,
            crate::db::models::Role::Maintainer,
        )
        .unwrap();

        // Linked: never an orphan.
        attach_to_issue(&conn, issue, "a", "used.png", "image/png", 10, Some(member));
        // Unlinked, by a member: the row this endpoint exists for.
        let pending =
            create_attachment(&conn, "b", "draft.png", "image/png", 20, Some(member)).unwrap();
        // Unlinked, by a non-member: not this project's business.
        create_attachment(&conn, "c", "other.png", "image/png", 30, Some(stranger)).unwrap();

        let orphans = list_project_orphans(&conn, project, 24 * 60 * 60).unwrap();
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].id, pending.id);
        assert_eq!(orphans[0].filename, "draft.png");
        assert_eq!(orphans[0].uploader.as_deref(), Some("member"));
        assert!(
            orphans[0].seconds_until_sweep > 23 * 60 * 60,
            "a fresh upload has nearly the whole grace window left, got {}",
            orphans[0].seconds_until_sweep
        );

        // Past the window the countdown floors at zero rather than going
        // negative: the sweeper takes it on its next pass.
        let due = list_project_orphans(&conn, project, 0).unwrap();
        assert_eq!(due[0].seconds_until_sweep, 0);
    }

    // ── Attachment text extraction (LIF-418) ─────────────────

    #[test]
    fn only_small_text_uploads_are_extractable() {
        assert!(is_extractable("text/plain", 1024));
        assert!(is_extractable("text/plain", MAX_EXTRACT_BYTES));
        assert!(!is_extractable("text/plain", MAX_EXTRACT_BYTES + 1));
        assert!(!is_extractable("image/png", 10));
        assert!(!is_extractable("application/pdf", 10));
    }

    #[test]
    fn extracted_text_is_indexed_once_and_not_reoffered() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let text = create_attachment(&conn, "t1", "notes.txt", "text/plain", 12, None).unwrap();
        create_attachment(&conn, "i1", "shot.png", "image/png", 12, None).unwrap();

        // The insert trigger indexed the filename but no contents yet, so the
        // text row is the only backfill candidate.
        let pending = unindexed_text_attachments(&conn).unwrap();
        assert_eq!(pending, vec![(text.id, "t1".to_string())]);

        set_extracted_text(&conn, text.id, "the quokka migration notes").unwrap();
        assert!(
            unindexed_text_attachments(&conn).unwrap().is_empty(),
            "a second backfill pass must find nothing to do"
        );
    }

    // ── parse_referenced_ids ─────────────────────────────────

    #[test]
    fn parse_ids_from_image_and_link_forms() {
        let md = "text ![alt](/api/attachments/12) more [file.pdf](/api/attachments/7) \
                  and bare /api/attachments/12 again and /api/attachments/99";
        let ids = parse_referenced_ids(md);
        assert_eq!(ids, vec![12, 7, 99]); // distinct, in first-seen order
    }

    // ── LIF-418 metadata ─────────────────────────────────────

    #[test]
    fn dimensions_drive_the_derived_thumbnail_flag() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let att = create_attachment(&conn, "dim", "big.png", "image/png", 1, None).unwrap();
        assert_eq!(att.width, None);
        assert!(
            !att.has_thumbnail,
            "an image with no recorded size offers no thumbnail"
        );

        set_dimensions(&conn, att.id, 1600, 900).unwrap();
        let att = get_attachment(&conn, att.id).unwrap();
        assert_eq!((att.width, att.height), (Some(1600), Some(900)));
        assert!(att.has_thumbnail);

        // A raster that already fits the thumbnail box needs none.
        let small = create_attachment(&conn, "small", "s.png", "image/png", 1, None).unwrap();
        set_dimensions(&conn, small.id, 100, 100).unwrap();
        assert!(!get_attachment(&conn, small.id).unwrap().has_thumbnail);

        // Nor does a non-raster, whatever dimensions someone recorded.
        let pdf = create_attachment(&conn, "pdf", "a.pdf", "application/pdf", 1, None).unwrap();
        set_dimensions(&conn, pdf.id, 2000, 2000).unwrap();
        assert!(!get_attachment(&conn, pdf.id).unwrap().has_thumbnail);
    }

    #[test]
    fn alt_text_round_trips_and_clears() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let att = create_attachment(&conn, "alt", "a.png", "image/png", 1, None).unwrap();
        assert_eq!(att.alt_text, None);

        let updated = update_alt_text(&conn, att.id, Some("a red square")).unwrap();
        assert_eq!(updated.alt_text.as_deref(), Some("a red square"));

        let cleared = update_alt_text(&conn, att.id, None).unwrap();
        assert_eq!(cleared.alt_text, None);

        assert!(update_alt_text(&conn, 99999, Some("nope")).is_err());
    }

    #[test]
    fn duplicates_are_the_other_rows_over_the_same_bytes() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let a = create_attachment(&conn, "shared", "a.png", "image/png", 1, None).unwrap();
        let b = create_attachment(&conn, "shared", "b.png", "image/png", 1, None).unwrap();
        let other = create_attachment(&conn, "alone", "c.png", "image/png", 1, None).unwrap();

        let dupes = duplicates_of(&conn, a.id, "shared").unwrap();
        assert_eq!(dupes.len(), 1);
        assert_eq!(dupes[0].id, b.id);
        assert_eq!(dupes[0].filename, "b.png");
        assert!(duplicates_of(&conn, other.id, "alone").unwrap().is_empty());
    }

    #[test]
    fn links_for_attachment_lists_every_referencing_entity() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let issue = seed_issue(&conn);
        let att = create_attachment(&conn, "l", "a.png", "image/png", 1, None).unwrap();
        assert!(links_for_attachment(&conn, att.id).unwrap().is_empty());

        link_attachment(&conn, att.id, AttachmentEntity::Issue, issue).unwrap();
        link_attachment(&conn, att.id, AttachmentEntity::Comment, 7).unwrap();
        assert_eq!(
            links_for_attachment(&conn, att.id).unwrap(),
            vec![("comment".to_string(), 7), ("issue".to_string(), issue)]
        );
    }

    #[test]
    fn parse_ids_empty_when_none() {
        assert!(parse_referenced_ids("no attachments here").is_empty());
        assert!(parse_referenced_ids("/api/attachments/ trailing slash no id").is_empty());
    }
}
