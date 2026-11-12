-- One-time upload URLs for MCP clients that cannot send a file inline.
--
-- `upload_attachment` with a `size` and no content issues a ticket; one PUT of
-- exactly that many bytes to /api/uploads/<token> creates the attachment as
-- the ticket's issuer and links it to the declared target. Only the SHA-256 of
-- the token is stored. Tickets live for minutes, are deleted when used, and
-- expired ones are swept whenever a new ticket is issued.
CREATE TABLE IF NOT EXISTS upload_tickets (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    token_hash  TEXT    NOT NULL UNIQUE,
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    filename    TEXT    NOT NULL,
    size_bytes  INTEGER NOT NULL,
    entity_type TEXT,
    entity_id   INTEGER,
    alt_text    TEXT,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    expires_at  TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_upload_tickets_expires ON upload_tickets(expires_at);
CREATE INDEX IF NOT EXISTS idx_upload_tickets_user ON upload_tickets(user_id);
