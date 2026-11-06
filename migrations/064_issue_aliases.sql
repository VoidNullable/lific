-- LIF-487: moving an issue to another project, keeping its old identifier.
--
-- An issue's identifier is its project's identifier plus its per-project
-- `sequence`, so a move gives it a new one. Every place that still says the
-- old one (descriptions, comments, commit messages, links) would go dead.
-- `issue_aliases` keeps each identifier an issue gave up, so `LIF-42` keeps
-- resolving to the issue that is now `FOO-7`.
--
-- ── Keys ──────────────────────────────────────────────────────────────
-- An alias is keyed by project id, not by the identifier text, so it follows
-- a project rename the same way the issue's own identifier does. A
-- (project, sequence) pair is never handed out twice: `create_issue` and
-- moves allocate past the highest alias as well as the highest issue, so an
-- alias can never collide with a live identifier.
--
-- ── Sync ──────────────────────────────────────────────────────────────
-- `seq` is drawn from the instance-wide `sync_seq` counter when the alias is
-- written. The `/changes` feed of the project the issue left reads alias
-- rows as issue tombstones at that seq, so a replica of the old project drops
-- the issue instead of keeping a stale copy forever.
--
-- ── Lifetime ──────────────────────────────────────────────────────────
-- Deleting the issue or the old project removes the alias with it.

CREATE TABLE IF NOT EXISTS issue_aliases (
    project_id  INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    sequence    INTEGER NOT NULL,
    issue_id    INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    seq         INTEGER NOT NULL,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (project_id, sequence)
);

CREATE INDEX IF NOT EXISTS idx_issue_aliases_issue ON issue_aliases(issue_id);
CREATE INDEX IF NOT EXISTS idx_issue_aliases_project_seq ON issue_aliases(project_id, seq);

-- A move is one UPDATE of `project_id` (with `sequence`). The 018 audit
-- trigger only watches named fields, so record the move itself: old and new
-- identifier, scoped to the issue and its new project.
CREATE TRIGGER IF NOT EXISTS audit_issues_move AFTER UPDATE OF project_id ON issues
WHEN NEW.project_id IS NOT OLD.project_id
BEGIN
    INSERT INTO audit_log (actor_user_id, transport, entity_type, entity_id, entity_label,
                           project_id, issue_id, action, field, old_value, new_value)
    VALUES (
        (SELECT user_id FROM _actor_state WHERE id = 1),
        COALESCE((SELECT transport FROM _actor_state WHERE id = 1), 'system'),
        'issue', NEW.id,
        (SELECT identifier FROM projects WHERE id = NEW.project_id) || '-' || NEW.sequence,
        NEW.project_id, NEW.id, 'update', 'project',
        (SELECT identifier FROM projects WHERE id = OLD.project_id) || '-' || OLD.sequence,
        (SELECT identifier FROM projects WHERE id = NEW.project_id) || '-' || NEW.sequence
    );
END;

-- Comments are indexed for search under their parent's project (034). The
-- issue's own row is re-indexed by 045's `issues_au`; its comments are not,
-- so search scoped to the new project would miss them. Carry them along.
CREATE TRIGGER IF NOT EXISTS issues_move_comment_search AFTER UPDATE OF project_id ON issues
WHEN NEW.project_id IS NOT OLD.project_id
BEGIN
    UPDATE search_index SET project_id = NEW.project_id
     WHERE entity_type = 'comment'
       AND entity_id IN (SELECT id FROM comments WHERE issue_id = NEW.id);
END;
