-- LIF-263: @mentions in comments.
--
-- A comment body can reference other users as `@username` tokens. The
-- tokens are stored verbatim in `comments.content`; this table records the
-- *resolved* set — which real users a given comment actually mentions —
-- computed server-side against the visible-member set at create/edit time
-- (see `db::queries::comments::sync_mentions`). Unmatched `@foo` never lands
-- here; it stays literal prose.
--
-- ── Recompute semantics ────────────────────────────────────────────────
-- On comment edit the whole set is recomputed: the query layer deletes the
-- comment's rows and re-inserts the current matches. That means an insert
-- trigger firing on a *re-added* mention is normal and expected; the audit
-- row it writes is the "mentioned @user" activity event for that edit.

CREATE TABLE IF NOT EXISTS comment_mentions (
    comment_id INTEGER NOT NULL REFERENCES comments(id) ON DELETE CASCADE,
    user_id    INTEGER NOT NULL REFERENCES users(id)    ON DELETE CASCADE,
    created_at TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (comment_id, user_id)
);

CREATE INDEX IF NOT EXISTS idx_comment_mentions_user ON comment_mentions(user_id);

-- ════════════════════════════════════════════════════════════════════
-- Activity: a "mention" event when a mention row is written. The event
-- lives on the parent issue/page feed (same denormalization the comment
-- triggers in migration 018 use), attributed to whoever is stamped on
-- `_actor_state`. `entity_label` snapshots the parent's identifier so the
-- feed still renders after the comment is gone; `new_value` snapshots the
-- mentioned user's username.
-- ════════════════════════════════════════════════════════════════════

CREATE TRIGGER IF NOT EXISTS audit_comment_mentions_insert AFTER INSERT ON comment_mentions BEGIN
    INSERT INTO audit_log (actor_user_id, transport, entity_type, entity_id, entity_label,
                           project_id, issue_id, page_id, action, new_value)
    SELECT
        (SELECT user_id FROM _actor_state WHERE id = 1),
        COALESCE((SELECT transport FROM _actor_state WHERE id = 1), 'system'),
        'comment', c.id,
        COALESCE(
            (SELECT p.identifier || '-' || i.sequence FROM issues i JOIN projects p ON p.id = i.project_id WHERE i.id = c.issue_id),
            (SELECT CASE WHEN pg.project_id IS NULL THEN 'DOC-' || pg.sequence
                         ELSE pr.identifier || '-DOC-' || pg.sequence END
             FROM pages pg LEFT JOIN projects pr ON pr.id = pg.project_id WHERE pg.id = c.page_id)
        ),
        COALESCE((SELECT project_id FROM issues WHERE id = c.issue_id),
                 (SELECT project_id FROM pages WHERE id = c.page_id)),
        c.issue_id, c.page_id, 'mention',
        (SELECT username FROM users WHERE id = NEW.user_id)
    FROM comments c
    WHERE c.id = NEW.comment_id;
END;
