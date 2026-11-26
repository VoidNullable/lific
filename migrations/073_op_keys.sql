-- Idempotency keys for create calls: `op_key` on the MCP create tools and the
-- `Idempotency-Key` header on the matching REST routes.
--
-- A row is written inside the create's own transaction, so a key exists
-- exactly when its write committed. A retry with the same key and the same
-- request gets `result` back instead of writing again; the same key with a
-- different request is refused. `request_hash` is the SHA-256 of the
-- operation name and the request as the server parsed it. Keys live 24 hours
-- and the retention sweep deletes older ones. `result` is NULL when the
-- result was too large to keep: the key still blocks a second write.
--
-- `user_id` is NULL for calls with no authenticated user (auth off), which
-- the unique index folds to one shared scope.
CREATE TABLE IF NOT EXISTS op_keys (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id      INTEGER REFERENCES users(id) ON DELETE CASCADE,
    op_key       TEXT    NOT NULL,
    operation    TEXT    NOT NULL,
    request_hash TEXT    NOT NULL,
    result       TEXT,
    created_at   TEXT    NOT NULL DEFAULT (datetime('now'))
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_op_keys_scope ON op_keys(IFNULL(user_id, 0), op_key);
CREATE INDEX IF NOT EXISTS idx_op_keys_created ON op_keys(created_at);
