-- Imported comments have inert attribution instead of a destination user.
-- The migration runner preserves existing triggers around this table rebuild.
CREATE TABLE comments_archive_rebuild (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_id INTEGER REFERENCES issues(id) ON DELETE CASCADE,
    page_id INTEGER REFERENCES pages(id) ON DELETE CASCADE,
    user_id INTEGER REFERENCES users(id),
    content TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    seq INTEGER,
    deleted_at TEXT,
    imported_author TEXT,
    CHECK ((issue_id IS NOT NULL) <> (page_id IS NOT NULL)),
    CHECK (user_id IS NOT NULL OR imported_author IS NOT NULL)
);
INSERT INTO comments_archive_rebuild
    (id, issue_id, page_id, user_id, content, created_at, updated_at, seq, deleted_at)
SELECT id, issue_id, page_id, user_id, content, created_at, updated_at, seq, deleted_at FROM comments;
-- Preserve deleted IDs too: audit history can still refer to them.
INSERT INTO sqlite_sequence (name, seq)
SELECT 'comments_archive_rebuild', seq FROM sqlite_sequence
WHERE name = 'comments'
  AND NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'comments_archive_rebuild');
UPDATE sqlite_sequence
SET seq = max(seq, COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'comments'), 0))
WHERE name = 'comments_archive_rebuild';
DROP TABLE comments;
ALTER TABLE comments_archive_rebuild RENAME TO comments;
CREATE INDEX idx_comments_issue ON comments(issue_id);
CREATE INDEX idx_comments_page ON comments(page_id);
CREATE INDEX idx_comments_user ON comments(user_id);
CREATE INDEX idx_comments_seq ON comments(seq DESC);
CREATE INDEX idx_comments_deleted_at ON comments(deleted_at) WHERE deleted_at IS NOT NULL;
CREATE INDEX idx_comments_issue_live ON comments(issue_id) WHERE deleted_at IS NULL;
CREATE INDEX idx_comments_page_live ON comments(page_id) WHERE deleted_at IS NULL;

ALTER TABLE attachments ADD COLUMN imported_author TEXT;
ALTER TABLE audit_log ADD COLUMN imported_author TEXT;
ALTER TABLE audit_log ADD COLUMN imported_source TEXT;
ALTER TABLE status_transitions ADD COLUMN imported_author TEXT;
ALTER TABLE status_transitions ADD COLUMN imported_source TEXT;

-- Source IDs here are provenance, never foreign keys into this instance.
CREATE TABLE project_archive_provenance (
    project_id INTEGER PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    format_version INTEGER NOT NULL,
    source_project_id INTEGER NOT NULL,
    exported_at TEXT NOT NULL,
    imported_at TEXT NOT NULL DEFAULT (datetime('now')),
    external_references TEXT NOT NULL
);
