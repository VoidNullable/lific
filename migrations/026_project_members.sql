-- LIF-195: project membership — the (user, role) source of truth for
-- project-scoped authorization (epic LIF-194). Data model only: no
-- enforcement lives in this migration or the query layer that reads it.
--
-- `projects.lead_user_id` (migration 008) remains the denormalized "primary
-- lead" pointer used by the current LIF-102 access check; the query layer
-- keeps both consistent on write (see db::queries::projects::create_project
-- / update_project). This table is additive: a project can have exactly one
-- `lead_user_id`, but any number of 'lead' rows here (e.g. after the lead
-- changes hands — the old lead keeps their membership row).

CREATE TABLE IF NOT EXISTS project_members (
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    user_id    INTEGER NOT NULL REFERENCES users(id)    ON DELETE CASCADE,
    role       TEXT NOT NULL CHECK (role IN ('lead','maintainer','viewer')),
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (project_id, user_id)
);

CREATE INDEX IF NOT EXISTS idx_project_members_user ON project_members(user_id);

-- Backfill: every project that already has a lead_user_id gets a 'lead'
-- membership row for that user. INSERT OR IGNORE makes this idempotent
-- (harmless if re-run), and the JOIN against users skips any lead_user_id
-- that's dangling (shouldn't happen given the FK, but cheap insurance).
INSERT OR IGNORE INTO project_members (project_id, user_id, role)
SELECT p.id, p.lead_user_id, 'lead'
FROM projects p
JOIN users u ON u.id = p.lead_user_id
WHERE p.lead_user_id IS NOT NULL;
