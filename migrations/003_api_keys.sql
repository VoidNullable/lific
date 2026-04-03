-- API key authentication.
-- Only hashes are stored, never plaintext keys.

CREATE TABLE IF NOT EXISTS api_keys (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT    NOT NULL UNIQUE,
    key_hash    TEXT    NOT NULL,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    expires_at  TEXT,
    revoked     INTEGER NOT NULL DEFAULT 0
);
