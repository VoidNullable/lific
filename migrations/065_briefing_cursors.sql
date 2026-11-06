-- LIF-490: where each caller's last briefing of a project left off, so
-- `get_briefing(since='last')` can resume without the agent carrying a cursor.
--
-- The cursor is a position in the audit log (`audit_log.id`), not a
-- timestamp. The audit log is the one change sequence that records every
-- kind of change the briefing reports (issue created, status moved, page
-- edited, comment added) together with who made it, and its AUTOINCREMENT id
-- is strictly increasing and never reused, even after retention prunes old
-- rows. `sync_seq` (migration 045) stamps a row's latest state instead, so it
-- cannot tell a new issue from an edited one.
--
-- `user_id` is the resolved caller: a bot key keeps its own cursor, separate
-- from its owner's, and a credential-less local session resolves to the first
-- admin like every other identity check.

CREATE TABLE IF NOT EXISTS briefing_cursors (
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    audit_id   INTEGER NOT NULL,
    briefed_at TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (user_id, project_id)
);

CREATE INDEX IF NOT EXISTS idx_briefing_cursors_project ON briefing_cursors(project_id);
