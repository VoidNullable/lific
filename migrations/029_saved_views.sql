-- LIF-242: saved views — named filter/group/sort presets per project,
-- personal to each user. `config` is an opaque JSON blob as far as the
-- backend is concerned (validated for size + JSON well-formedness only —
-- see db::queries::views::validate_config); the frontend owns its shape
-- (`ViewConfig` in web/src/lib/issues/views.ts) and can evolve it without a
-- migration.
--
-- `is_default` is per-(project, user): at most one row may have
-- is_default = 1 for a given (project_id, user_id) pair. This is enforced
-- in the query layer (create_view / update_view clear any existing default
-- before setting a new one inside the same SAVEPOINT) rather than a partial
-- unique index, matching the rest of this codebase's preference for
-- query-layer invariants over exotic constraints (see e.g. the last-lead
-- guard in db::queries::members).
--
-- UNIQUE(project_id, user_id, name) mirrors labels' per-project name
-- uniqueness: a user can't have two views with the same name on one
-- project, but different users (or the same user on a different project)
-- can reuse a name freely.

CREATE TABLE IF NOT EXISTS saved_views (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    user_id    INTEGER NOT NULL REFERENCES users(id)    ON DELETE CASCADE,
    name       TEXT NOT NULL,
    config     TEXT NOT NULL,
    is_default INTEGER NOT NULL DEFAULT 0 CHECK (is_default IN (0, 1)),
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (project_id, user_id, name)
);

CREATE INDEX IF NOT EXISTS idx_saved_views_project_user ON saved_views(project_id, user_id);
