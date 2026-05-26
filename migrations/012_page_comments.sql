-- LIF-106: extend `comments` to support page comments alongside issue comments.
--
-- SQLite can't ALTER COLUMN to drop NOT NULL, so we rebuild the table:
--   * issue_id becomes nullable
--   * page_id is added (nullable, FK to pages with cascade)
--   * CHECK ensures exactly one of (issue_id, page_id) is set
--   * existing rows are migrated with page_id = NULL (all current comments are issue comments)
--
-- Nothing references `comments` via FK, so no foreign_keys=OFF dance is required;
-- the migration runs inside a SAVEPOINT (see migrate.rs).

CREATE TABLE comments_new (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_id   INTEGER REFERENCES issues(id) ON DELETE CASCADE,
    page_id    INTEGER REFERENCES pages(id)  ON DELETE CASCADE,
    user_id    INTEGER NOT NULL REFERENCES users(id),
    content    TEXT    NOT NULL,
    created_at TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT    NOT NULL DEFAULT (datetime('now')),
    -- exactly one parent must be set
    CHECK ((issue_id IS NOT NULL) <> (page_id IS NOT NULL))
);

INSERT INTO comments_new (id, issue_id, page_id, user_id, content, created_at, updated_at)
    SELECT id, issue_id, NULL, user_id, content, created_at, updated_at FROM comments;

DROP TABLE comments;
ALTER TABLE comments_new RENAME TO comments;

CREATE INDEX idx_comments_issue ON comments(issue_id);
CREATE INDEX idx_comments_page  ON comments(page_id);
CREATE INDEX idx_comments_user  ON comments(user_id);
