-- LIF-147: who an issue is for.
--
-- An issue is in one of three states:
--
--   * unassigned: no rows. Any agent may work it unless it is blocked,
--     done or cancelled. This is the default and the agents' pool.
--   * human: one row with user_id NULL. Some person must do it, no one in
--     particular.
--   * named people: one or more rows with a user_id, each a human account.
--
-- Human and named people are exclusive. The query layer replaces the whole
-- set on every write, so a NULL row and user rows never coexist; the
-- partial unique indexes below keep each state free of duplicates.
--
-- ── Soft delete ───────────────────────────────────────────────────────
-- Like labels and waits, rows survive an issue's tombstone so a restore
-- brings them back; every read joins the live issue. The purge's physical
-- DELETE cascades them away, and the audit trigger stays silent for that
-- cascade because the issue is no longer live.
--
-- ── Users ─────────────────────────────────────────────────────────────
-- Human accounts are deactivated rather than deleted, so a named assignee
-- normally outlives everything. If one is deleted anyway, an issue that
-- named only that person falls back to "human" instead of dropping into
-- the agents' pool: the work still needs a person.

CREATE TABLE IF NOT EXISTS issue_assignees (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_id    INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    user_id     INTEGER REFERENCES users(id) ON DELETE CASCADE,
    created_by  INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_issue_assignees_user ON issue_assignees(user_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_issue_assignees_named
    ON issue_assignees(issue_id, user_id) WHERE user_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_issue_assignees_human
    ON issue_assignees(issue_id) WHERE user_id IS NULL;

CREATE TRIGGER IF NOT EXISTS issue_assignees_keep_human_bd
BEFORE DELETE ON users
BEGIN
    INSERT OR IGNORE INTO issue_assignees (issue_id, user_id)
    SELECT a.issue_id, NULL FROM issue_assignees a
     WHERE a.user_id = OLD.id
       AND NOT EXISTS (
           SELECT 1 FROM issue_assignees other
            WHERE other.issue_id = a.issue_id
              AND other.user_id IS NOT NULL
              AND other.user_id <> OLD.id
       );
END;

-- ── Sync: an assignment is activity on its issue ──────────────────────
-- Same shape as 054's wait bumps.

CREATE TRIGGER IF NOT EXISTS issue_assignees_bump_ai
AFTER INSERT ON issue_assignees
BEGIN
    UPDATE sync_seq SET value = value + 1 WHERE id = 1;
    UPDATE issues
       SET updated_at = datetime('now'),
           seq = (SELECT value FROM sync_seq WHERE id = 1)
     WHERE id = NEW.issue_id;
END;

CREATE TRIGGER IF NOT EXISTS issue_assignees_bump_ad
AFTER DELETE ON issue_assignees
BEGIN
    UPDATE sync_seq SET value = value + 1 WHERE id = 1;
    UPDATE issues
       SET updated_at = datetime('now'),
           seq = (SELECT value FROM sync_seq WHERE id = 1)
     WHERE id = OLD.issue_id;
END;

-- ── Audit: 'assign' when added, 'unassign' when removed ───────────────
-- The value is '@username' for a named person and 'human' for the mark.

CREATE TRIGGER IF NOT EXISTS audit_issue_assignees_add AFTER INSERT ON issue_assignees BEGIN
    INSERT INTO audit_log (actor_user_id, transport, entity_type, entity_id, entity_label,
                           project_id, issue_id, action, field, new_value)
    VALUES (
        (SELECT user_id FROM _actor_state WHERE id = 1),
        COALESCE((SELECT transport FROM _actor_state WHERE id = 1), 'system'),
        'issue', NEW.issue_id,
        (SELECT p.identifier || '-' || i.sequence FROM issues i JOIN projects p ON p.id = i.project_id WHERE i.id = NEW.issue_id),
        (SELECT project_id FROM issues WHERE id = NEW.issue_id),
        NEW.issue_id, 'assign', 'assignee',
        CASE WHEN NEW.user_id IS NULL THEN 'human'
             ELSE '@' || COALESCE((SELECT username FROM users WHERE id = NEW.user_id), 'deleted user')
        END
    );
END;

CREATE TRIGGER IF NOT EXISTS audit_issue_assignees_remove AFTER DELETE ON issue_assignees
WHEN EXISTS (SELECT 1 FROM issues WHERE id = OLD.issue_id AND deleted_at IS NULL)
BEGIN
    INSERT INTO audit_log (actor_user_id, transport, entity_type, entity_id, entity_label,
                           project_id, issue_id, action, field, old_value)
    VALUES (
        (SELECT user_id FROM _actor_state WHERE id = 1),
        COALESCE((SELECT transport FROM _actor_state WHERE id = 1), 'system'),
        'issue', OLD.issue_id,
        (SELECT p.identifier || '-' || i.sequence FROM issues i JOIN projects p ON p.id = i.project_id WHERE i.id = OLD.issue_id),
        (SELECT project_id FROM issues WHERE id = OLD.issue_id),
        OLD.issue_id, 'unassign', 'assignee',
        CASE WHEN OLD.user_id IS NULL THEN 'human'
             ELSE '@' || COALESCE((SELECT username FROM users WHERE id = OLD.user_id), 'deleted user')
        END
    );
END;
