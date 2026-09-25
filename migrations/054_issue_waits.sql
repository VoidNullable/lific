-- LIF-484: an issue can wait on a person or on a window of days, not only on
-- another issue.
--
-- `issue_relations` can only point at an issue, so "blocked until the owner
-- decides" or "the filing office said 2 to 5 business days" had nowhere to
-- live except prose. A wait is the blocker for those cases:
--
--   * kind 'user': held until someone clears it. `user_id` names the account
--     being waited on; `earliest`/`latest` are NULL.
--   * kind 'date': held before `earliest`. From `earliest` through `latest`
--     it is due to check (no longer blocking); after `latest` it is overdue.
--     A single day stores earliest = latest. Dates are plain YYYY-MM-DD and
--     are compared against the server's local calendar day at read time, so
--     nothing here goes stale at midnight.
--
-- ── Clearing deletes the row ──────────────────────────────────────────
-- Clearing a wait is the same act as unlinking a relation, and relations
-- are deleted rows. The history is not lost: the audit triggers below write
-- an `unwait` entry with the full description before the row goes. Keeping
-- cleared rows with a `cleared_at` would put a second copy of that history
-- here and a `cleared_at IS NULL` predicate on every read, workable filter
-- and archive scope, for no reader that needs it.
--
-- ── Soft delete ───────────────────────────────────────────────────────
-- Like labels and relations, waits survive an issue's tombstone so a
-- restore brings them back; every read joins the live issue. The purge's
-- physical DELETE cascades them away, and the audit trigger stays silent
-- for that cascade because the issue is no longer live.
--
-- ── Users ─────────────────────────────────────────────────────────────
-- Deleting the account being waited on deletes the wait (there is no one
-- left to clear it); deleting the author only forgets who wrote it.

CREATE TABLE IF NOT EXISTS issue_waits (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_id    INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    kind        TEXT    NOT NULL CHECK (kind IN ('user', 'date')),
    user_id     INTEGER REFERENCES users(id) ON DELETE CASCADE,
    earliest    TEXT,
    latest      TEXT,
    note        TEXT    NOT NULL DEFAULT '',
    created_by  INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    CHECK (
        (kind = 'user' AND user_id IS NOT NULL AND earliest IS NULL AND latest IS NULL)
        OR (kind = 'date' AND user_id IS NULL AND earliest IS NOT NULL
            AND latest IS NOT NULL AND latest >= earliest)
    )
);

CREATE INDEX IF NOT EXISTS idx_issue_waits_issue ON issue_waits(issue_id);

-- One wait per person, and one per exact window, on any issue. A second
-- "wait on @blake" with a different note is the same blocker, not a new one.
CREATE UNIQUE INDEX IF NOT EXISTS idx_issue_waits_user
    ON issue_waits(issue_id, user_id) WHERE kind = 'user';
CREATE UNIQUE INDEX IF NOT EXISTS idx_issue_waits_date
    ON issue_waits(issue_id, earliest, latest) WHERE kind = 'date';

-- ── Sync: a wait is activity on its issue ─────────────────────────────
-- Same shape as 045's label bumps: the issue's seq and updated_at advance,
-- so `/changes` re-delivers the issue row carrying its new waits.

CREATE TRIGGER IF NOT EXISTS issue_waits_bump_ai
AFTER INSERT ON issue_waits
BEGIN
    UPDATE sync_seq SET value = value + 1 WHERE id = 1;
    UPDATE issues
       SET updated_at = datetime('now'),
           seq = (SELECT value FROM sync_seq WHERE id = 1)
     WHERE id = NEW.issue_id;
END;

CREATE TRIGGER IF NOT EXISTS issue_waits_bump_ad
AFTER DELETE ON issue_waits
BEGIN
    UPDATE sync_seq SET value = value + 1 WHERE id = 1;
    UPDATE issues
       SET updated_at = datetime('now'),
           seq = (SELECT value FROM sync_seq WHERE id = 1)
     WHERE id = OLD.issue_id;
END;

-- ── Audit: 'wait' when added, 'unwait' when cleared ───────────────────
-- Recorded against the waiting issue; `field` is the kind and the value is
-- the rendered blocker ("@blake: note", "2026-09-28..2026-09-29: note").

CREATE TRIGGER IF NOT EXISTS audit_issue_waits_add AFTER INSERT ON issue_waits BEGIN
    INSERT INTO audit_log (actor_user_id, transport, entity_type, entity_id, entity_label,
                           project_id, issue_id, action, field, new_value)
    VALUES (
        (SELECT user_id FROM _actor_state WHERE id = 1),
        COALESCE((SELECT transport FROM _actor_state WHERE id = 1), 'system'),
        'issue', NEW.issue_id,
        (SELECT p.identifier || '-' || i.sequence FROM issues i JOIN projects p ON p.id = i.project_id WHERE i.id = NEW.issue_id),
        (SELECT project_id FROM issues WHERE id = NEW.issue_id),
        NEW.issue_id, 'wait', NEW.kind,
        CASE NEW.kind
            WHEN 'user' THEN '@' || COALESCE((SELECT username FROM users WHERE id = NEW.user_id), 'deleted user')
            ELSE CASE WHEN NEW.earliest = NEW.latest THEN NEW.earliest
                      ELSE NEW.earliest || '..' || NEW.latest END
        END || CASE WHEN NEW.note <> '' THEN ': ' || NEW.note ELSE '' END
    );
END;

CREATE TRIGGER IF NOT EXISTS audit_issue_waits_clear AFTER DELETE ON issue_waits
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
        OLD.issue_id, 'unwait', OLD.kind,
        CASE OLD.kind
            WHEN 'user' THEN '@' || COALESCE((SELECT username FROM users WHERE id = OLD.user_id), 'deleted user')
            ELSE CASE WHEN OLD.earliest = OLD.latest THEN OLD.earliest
                      ELSE OLD.earliest || '..' || OLD.latest END
        END || CASE WHEN OLD.note <> '' THEN ': ' || OLD.note ELSE '' END
    );
END;
