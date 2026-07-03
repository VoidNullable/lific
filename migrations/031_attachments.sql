-- LIF-262: attachments — image + file uploads on issues, comments, and pages.
--
-- Storage decision (Option B, content-addressed sidecar): the raw bytes live
-- on disk at `<data_dir>/attachments/<sha256>`, never in SQLite. This table
-- holds only metadata, so the DB stays small and the backup set is
-- "binary + database + attachments dir" (see src/backup.rs). Deduplication is
-- implicit: two uploads of identical bytes share one `sha256` file, but each
-- upload still gets its own `attachments` row (distinct filename / uploader /
-- timestamp), so a delete of one never orphans the other's link.
--
-- `attachment_links` is the many-to-many join between an attachment and the
-- entity that references it (an issue/page description or a comment). A single
-- attachment can be linked from several entities (e.g. the same screenshot
-- pasted into two comments). Link rows are removed when their owning entity is
-- deleted (explicit cascade below for issues/pages/comments), and an
-- attachment with zero links older than a grace window is swept by the orphan
-- GC (see src/db/queries/attachments.rs).

CREATE TABLE IF NOT EXISTS attachments (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    sha256      TEXT NOT NULL,
    filename    TEXT NOT NULL,
    mime        TEXT NOT NULL,
    size_bytes  INTEGER NOT NULL,
    uploader_id INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Fast "does any row still reference this content hash?" check, used by the
-- GC before it deletes a sidecar file (bytes are shared across rows).
CREATE INDEX IF NOT EXISTS idx_attachments_sha256 ON attachments(sha256);

CREATE TABLE IF NOT EXISTS attachment_links (
    attachment_id INTEGER NOT NULL REFERENCES attachments(id) ON DELETE CASCADE,
    entity_type   TEXT NOT NULL CHECK (entity_type IN ('issue','page','comment')),
    entity_id     INTEGER NOT NULL,
    created_at    TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (attachment_id, entity_type, entity_id)
);

-- "What is attached to this entity?" (detail-view attachment section) and
-- "how many links does this attachment still have?" (orphan GC) both read via
-- this index.
CREATE INDEX IF NOT EXISTS idx_attachment_links_entity
    ON attachment_links(entity_type, entity_id);
CREATE INDEX IF NOT EXISTS idx_attachment_links_attachment
    ON attachment_links(attachment_id);

-- Explicit link cascade on entity deletion. SQLite can't declare an FK from
-- attachment_links to three different parent tables via one column, so the
-- cascade is done with triggers instead: deleting an issue/page/comment drops
-- its link rows (the attachment row itself survives until the GC collects it
-- if it has no remaining links).
CREATE TRIGGER IF NOT EXISTS trg_attachment_links_issue_delete
AFTER DELETE ON issues
BEGIN
    DELETE FROM attachment_links
    WHERE entity_type = 'issue' AND entity_id = OLD.id;
END;

CREATE TRIGGER IF NOT EXISTS trg_attachment_links_page_delete
AFTER DELETE ON pages
BEGIN
    DELETE FROM attachment_links
    WHERE entity_type = 'page' AND entity_id = OLD.id;
END;

CREATE TRIGGER IF NOT EXISTS trg_attachment_links_comment_delete
AFTER DELETE ON comments
BEGIN
    DELETE FROM attachment_links
    WHERE entity_type = 'comment' AND entity_id = OLD.id;
END;
