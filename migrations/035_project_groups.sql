-- Per-user grouping of projects in the sidebar.
--
-- Deliberately not called a folder: `folders` (001_initial.sql) is a
-- project-scoped, nestable container for pages. Two concepts sharing that
-- word would make every `grep folder` return unrelated hits.
--
-- Per-user rather than instance-wide because project visibility is per-user
-- through project_members once authz_enforced is on (authz::can_view_project),
-- so a shared group would render as an empty group for anyone without access
-- to the projects inside it.

CREATE TABLE IF NOT EXISTS project_groups (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name       TEXT    NOT NULL,
    sort_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT    NOT NULL DEFAULT (datetime('now')),
    UNIQUE (user_id, name)
);

-- A project belongs to at most one group per user. That invariant spans rows
-- in different groups, so it lives in the query layer (assign_project clears
-- any existing membership inside one SAVEPOINT) rather than in a constraint,
-- matching this codebase's preference for query-layer invariants over exotic
-- constraints (see the 029_saved_views.sql header).
CREATE TABLE IF NOT EXISTS project_group_items (
    group_id   INTEGER NOT NULL REFERENCES project_groups(id) ON DELETE CASCADE,
    project_id INTEGER NOT NULL REFERENCES projects(id)       ON DELETE CASCADE,
    PRIMARY KEY (group_id, project_id)
);

-- assign_project deletes by project_id across all of one user's groups.
CREATE INDEX IF NOT EXISTS idx_project_group_items_project
    ON project_group_items(project_id);
