-- LIF-541: one row per successful MCP read tool call, so the Agent activity
-- page can show reads next to the writes audit_log already records.
--
-- Rows are buffered in memory and flushed in batches (see
-- src/db/agent_reads.rs), and a retention sweep deletes anything older than
-- 30 days. No foreign keys, for the same reason audit_log has none: the
-- history must not block deleting the users or entities it mentions.
CREATE TABLE IF NOT EXISTS agent_reads (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    ts           TEXT    NOT NULL,   -- 'YYYY-MM-DD HH:MM:SS' UTC, same format as audit_log.ts
    user_id      INTEGER NOT NULL,   -- caller users.id; no FK on purpose (like audit_log)
    tool         TEXT    NOT NULL,   -- MCP tool name
    project_id   INTEGER,            -- resolved project, NULL for workspace-wide reads
    entity_type  TEXT,               -- issue | page | plan | project | attachment; NULL for list/search
    entity_id    INTEGER,
    entity_label TEXT                -- 'LIF-42', 'LIF-DOC-3', 'LIF-PLAN-2', 'LIF'
);
CREATE INDEX IF NOT EXISTS idx_agent_reads_ts   ON agent_reads(ts);
CREATE INDEX IF NOT EXISTS idx_agent_reads_user ON agent_reads(user_id, ts);
