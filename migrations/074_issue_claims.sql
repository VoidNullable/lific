-- LIF-525: a worker holds an issue for a limited time.
--
-- One row per issue at most. The holder is an account plus an optional
-- worker id (one MCP session or process under that account); a NULL worker
-- is a claim tied to the account alone. A row whose expires_at has passed is
-- simply ignored by every read: nothing sweeps expired rows, and the next
-- claim overwrites one.
--
-- renewed_at is the last claim or re-claim (the heartbeat), which the stale
-- checks count as activity on the issue. claimed_at stays at the first claim
-- for as long as the same holder keeps renewing.
--
-- Claims are live coordination state, not history: they are not audited and
-- project archives leave them out.

CREATE TABLE IF NOT EXISTS issue_claims (
    issue_id    INTEGER PRIMARY KEY REFERENCES issues(id) ON DELETE CASCADE,
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    worker      TEXT,
    claimed_at  TEXT    NOT NULL,   -- 'YYYY-MM-DD HH:MM:SS' UTC
    renewed_at  TEXT    NOT NULL,
    expires_at  TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_issue_claims_expires ON issue_claims(expires_at);

-- Closing or deleting an issue ends its claim, whatever path wrote it.
CREATE TRIGGER IF NOT EXISTS issue_claims_end_on_close
AFTER UPDATE OF status, deleted_at ON issues
WHEN NEW.status IN ('done', 'cancelled') OR NEW.deleted_at IS NOT NULL
BEGIN
    DELETE FROM issue_claims WHERE issue_id = NEW.id;
END;
