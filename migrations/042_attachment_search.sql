-- LIF-418: make attachments searchable by filename and, for text uploads, by
-- their contents.
--
-- The house FTS pattern (001's `search_index`, extended by 034 for comments)
-- is a plain fts5 table kept in sync by AFTER INSERT/UPDATE/DELETE triggers on
-- the source table, plus a one-time backfill in the migration itself for rows
-- that predate the triggers. This mirrors that shape exactly, with one
-- deliberate difference: attachments get their own index rather than more rows
-- in `search_index`.
--
-- Why a separate table: `search_index`'s columns are fixed at 001 (title,
-- body, entity_type, entity_id, project_id) and an attachment has no
-- project_id of its own — it belongs to whichever entities happen to link it,
-- which can be several, in several projects, and can change after upload.
-- Storing a snapshot of that in an FTS column would go stale the moment a link
-- moves. `attachments_fts` therefore carries only what is intrinsic to the
-- blob (filename + extracted text), and `db::queries::search` resolves the
-- project and the linked entity at query time from `attachment_links`.
--
-- `extracted_text` is populated by the upload path for `text/*` uploads up to
-- 512 KiB (see `api::attachments`) and by the startup backfill in
-- `storage::backfill_attachment_text` for rows uploaded before this migration.
-- Everything else keeps the empty string and is findable by filename alone.

CREATE VIRTUAL TABLE IF NOT EXISTS attachments_fts USING fts5(
    filename,
    extracted_text,
    attachment_id,   -- references attachments.id
    tokenize='porter unicode61'
);

-- ── Backfill rows that predate the triggers ──────────────────────────────
-- Filename only; the text of existing `text/*` uploads is filled in on the
-- next server start (the bytes live on disk, not in SQLite, so a migration
-- cannot read them).
INSERT INTO attachments_fts(filename, extracted_text, attachment_id)
SELECT a.filename, '', a.id
FROM attachments a
WHERE NOT EXISTS (
    SELECT 1 FROM attachments_fts f WHERE f.attachment_id = a.id
);

-- ── Keep the index in sync on attachment writes ──────────────────────────

CREATE TRIGGER IF NOT EXISTS attachments_fts_ai AFTER INSERT ON attachments BEGIN
    INSERT INTO attachments_fts(filename, extracted_text, attachment_id)
    VALUES (NEW.filename, '', NEW.id);
END;

-- Unlike the comment triggers this UPDATEs in place instead of
-- delete-then-insert: the extracted text is not derivable from the
-- `attachments` row (it comes from the blob on disk), so a delete+insert would
-- silently drop it. Only the filename can change on an existing row.
CREATE TRIGGER IF NOT EXISTS attachments_fts_au AFTER UPDATE ON attachments BEGIN
    UPDATE attachments_fts
    SET filename = NEW.filename
    WHERE attachment_id = OLD.id;
END;

CREATE TRIGGER IF NOT EXISTS attachments_fts_ad AFTER DELETE ON attachments BEGIN
    DELETE FROM attachments_fts WHERE attachment_id = OLD.id;
END;
