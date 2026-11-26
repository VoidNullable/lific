-- Instance identity belongs to the database, not an origin or a project archive.
-- Restoring the database preserves it; applying migrations again never replaces it.
CREATE TABLE IF NOT EXISTS database_identity (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    value TEXT NOT NULL
);
INSERT OR IGNORE INTO database_identity (id, value)
VALUES (1, lower(hex(randomblob(16))));
