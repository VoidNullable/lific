-- Add legacy Hangul Word Processor files to the attachment MIME allowlist.
-- The attachments table is rebuilt because SQLite cannot alter its CHECK
-- constraint in place. Preserve attachment rows and their content hashes,
-- and every column the table has gained since it was created, including
-- imported_author from migration 050.
DROP TRIGGER IF EXISTS attachments_fts_ai;
DROP TRIGGER IF EXISTS attachments_fts_au;
DROP TRIGGER IF EXISTS attachments_fts_ad;

CREATE TABLE attachments_new (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    sha256      TEXT NOT NULL
                CHECK (length(sha256) = 64 AND sha256 NOT GLOB '*[^0-9a-f]*'),
    filename    TEXT NOT NULL,
    mime        TEXT NOT NULL CHECK (mime IN (
        'image/png',
        'image/jpeg',
        'image/gif',
        'image/webp',
        'image/svg+xml',
        'application/pdf',
        'text/plain',
        'application/zip',
        'video/mp4',
        'video/webm',
        'audio/webm',
        'audio/ogg',
        'audio/mpeg',
        'application/vnd.sqlite3',
        'application/x-hwp'
    )),
    size_bytes  INTEGER NOT NULL CHECK (size_bytes >= 0),
    uploader_id INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    width       INTEGER,
    height      INTEGER,
    alt_text    TEXT,
    imported_author TEXT
);

INSERT INTO attachments_new
    (id, sha256, filename, mime, size_bytes, uploader_id, created_at,
     width, height, alt_text, imported_author)
SELECT id, sha256, filename, mime, size_bytes, uploader_id, created_at,
       width, height, alt_text, imported_author
FROM attachments;

DROP TABLE attachments;
ALTER TABLE attachments_new RENAME TO attachments;

CREATE INDEX idx_attachments_sha256 ON attachments(sha256);

CREATE TRIGGER attachments_fts_ai AFTER INSERT ON attachments BEGIN
    INSERT INTO attachments_fts(filename, extracted_text, attachment_id)
    VALUES (NEW.filename, '', NEW.id);
END;

CREATE TRIGGER attachments_fts_au AFTER UPDATE ON attachments BEGIN
    UPDATE attachments_fts
    SET filename = NEW.filename
    WHERE attachment_id = OLD.id;
END;

CREATE TRIGGER attachments_fts_ad AFTER DELETE ON attachments BEGIN
    DELETE FROM attachments_fts WHERE attachment_id = OLD.id;
END;
