-- Widen the kind constraint while preserving ids, sync positions and attribution.
-- The runner suspends and restores triggers around the rebuild.
CREATE TABLE comments_decision_rebuild (
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
    kind TEXT NOT NULL DEFAULT 'comment' CHECK (kind IN ('comment', 'verification', 'decision')),
    CHECK ((issue_id IS NOT NULL) <> (page_id IS NOT NULL)),
    CHECK (user_id IS NOT NULL OR imported_author IS NOT NULL)
);
INSERT INTO comments_decision_rebuild
    (id, issue_id, page_id, user_id, content, created_at, updated_at, seq, deleted_at, imported_author, kind)
SELECT id, issue_id, page_id, user_id, content, created_at, updated_at, seq, deleted_at, imported_author, kind FROM comments;
INSERT INTO sqlite_sequence (name, seq)
SELECT 'comments_decision_rebuild', seq FROM sqlite_sequence
WHERE name = 'comments'
  AND NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'comments_decision_rebuild');
UPDATE sqlite_sequence
SET seq = max(seq, COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'comments'), 0))
WHERE name = 'comments_decision_rebuild';
DROP TABLE comments;
ALTER TABLE comments_decision_rebuild RENAME TO comments;
CREATE INDEX idx_comments_issue ON comments(issue_id);
CREATE INDEX idx_comments_page ON comments(page_id);
CREATE INDEX idx_comments_user ON comments(user_id);
CREATE INDEX idx_comments_seq ON comments(seq DESC);
CREATE INDEX idx_comments_deleted_at ON comments(deleted_at) WHERE deleted_at IS NOT NULL;
CREATE INDEX idx_comments_issue_live ON comments(issue_id) WHERE deleted_at IS NULL;
CREATE INDEX idx_comments_page_live ON comments(page_id) WHERE deleted_at IS NULL;
CREATE INDEX idx_comments_decisions ON comments(issue_id, created_at, id)
    WHERE kind = 'decision' AND deleted_at IS NULL;
