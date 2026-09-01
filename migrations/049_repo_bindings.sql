-- LIF-448: repo → project bindings.
--
-- A binding is the join between "the checkout an agent is standing in" and
-- "the Lific project that checkout files issues against". It is deliberately
-- a row of its own rather than a column on `projects`, because one project
-- can be reached from several checkouts (a clone, a worktree, a fork) and the
-- set of things that identify the same repository is open-ended.
--
-- ── Two tables, not one ────────────────────────────────────────────────
-- `repo_bindings` is the binding itself: it points at a project and nothing
-- else. `repo_identities` holds the aliases that resolve TO that binding —
-- one row per (kind, value), where kind is 'remote' (a normalized remote URL)
-- or 'root' (an absolute worktree root path). Aliases accumulate: the second
-- clone of the same repo adds a 'root' row to the binding it already resolved
-- through its remote, instead of creating a rival binding.
--
-- `UNIQUE(kind, value)` is global, not per-binding. An alias names exactly one
-- repository, so it may belong to exactly one binding; a second binding trying
-- to claim it is a mistake the query layer surfaces as a conflict rather than
-- a silent last-writer-wins. It also makes merging two bindings safe: moving
-- every identity across can never collide, because a collision would mean the
-- same alias was already claimed twice.
--
-- ── created_by is provenance, not authority ────────────────────────────
-- Whoever created the binding is recorded so the trail reads sensibly, but it
-- confers no control: any caller allowed to manage the project can repoint,
-- merge or delete the binding. No FK, so the row survives the user's deletion,
-- same reasoning as `audit_log.actor_user_id`.

CREATE TABLE repo_bindings (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id  INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    created_by  INTEGER               -- provenance only; confers no control
);
CREATE INDEX idx_repo_bindings_project ON repo_bindings(project_id);

CREATE TABLE repo_identities (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    binding_id    INTEGER NOT NULL REFERENCES repo_bindings(id) ON DELETE CASCADE,
    kind          TEXT    NOT NULL CHECK(kind IN ('remote','root')),
    value         TEXT    NOT NULL,
    first_seen_at TEXT    NOT NULL DEFAULT (datetime('now')),
    UNIQUE(kind, value)
);
CREATE INDEX idx_repo_identities_binding ON repo_identities(binding_id);

-- ════════════════════════════════════════════════════════════════════
-- Audit (migration 018 pattern, as extended by 028)
--
-- Attribution reads `_actor_state` exactly as every other audit trigger
-- does, so all write surfaces are covered without the query layer inserting
-- anything by hand. `entity_label` snapshots a human identifier: the
-- project's identifier for a binding, the alias itself for an identity.
--
-- Both labels are resolved with a subquery, so a CASCADE delete (dropping a
-- project, or a binding taking its identities with it) records the row with a
-- NULL label rather than failing: by the time the child trigger fires, the
-- parent is already gone. That matches how `audit_issues_delete` behaves when
-- a project is deleted out from under it.
--
-- Only `project_id` is auditable on a binding — a repoint is the one update
-- that means anything, and 018's rule is that timestamps are never audited.
-- Old and new are recorded as project IDENTIFIERS, not ids, following 018's
-- "resolved names, not ids" rule for reference columns.
-- ════════════════════════════════════════════════════════════════════

CREATE TRIGGER IF NOT EXISTS audit_repo_bindings_insert AFTER INSERT ON repo_bindings BEGIN
    INSERT INTO audit_log (actor_user_id, transport, entity_type, entity_id, entity_label,
                           project_id, action, new_value)
    VALUES (
        (SELECT user_id FROM _actor_state WHERE id = 1),
        COALESCE((SELECT transport FROM _actor_state WHERE id = 1), 'system'),
        'repo_binding', NEW.id,
        (SELECT identifier FROM projects WHERE id = NEW.project_id),
        NEW.project_id, 'create',
        (SELECT identifier FROM projects WHERE id = NEW.project_id)
    );
END;

CREATE TRIGGER IF NOT EXISTS audit_repo_bindings_delete AFTER DELETE ON repo_bindings BEGIN
    INSERT INTO audit_log (actor_user_id, transport, entity_type, entity_id, entity_label,
                           project_id, action, old_value)
    VALUES (
        (SELECT user_id FROM _actor_state WHERE id = 1),
        COALESCE((SELECT transport FROM _actor_state WHERE id = 1), 'system'),
        'repo_binding', OLD.id,
        (SELECT identifier FROM projects WHERE id = OLD.project_id),
        OLD.project_id, 'delete',
        (SELECT identifier FROM projects WHERE id = OLD.project_id)
    );
END;

CREATE TRIGGER IF NOT EXISTS audit_repo_bindings_update AFTER UPDATE OF project_id ON repo_bindings
WHEN OLD.project_id IS NOT NEW.project_id
BEGIN
    INSERT INTO audit_log (actor_user_id, transport, entity_type, entity_id, entity_label,
                           project_id, action, field, old_value, new_value)
    VALUES (
        (SELECT user_id FROM _actor_state WHERE id = 1),
        COALESCE((SELECT transport FROM _actor_state WHERE id = 1), 'system'),
        'repo_binding', NEW.id,
        (SELECT identifier FROM projects WHERE id = NEW.project_id),
        NEW.project_id, 'update', 'project',
        (SELECT identifier FROM projects WHERE id = OLD.project_id),
        (SELECT identifier FROM projects WHERE id = NEW.project_id)
    );
END;

CREATE TRIGGER IF NOT EXISTS audit_repo_identities_insert AFTER INSERT ON repo_identities BEGIN
    INSERT INTO audit_log (actor_user_id, transport, entity_type, entity_id, entity_label,
                           project_id, action, new_value)
    VALUES (
        (SELECT user_id FROM _actor_state WHERE id = 1),
        COALESCE((SELECT transport FROM _actor_state WHERE id = 1), 'system'),
        'repo_identity', NEW.id, NEW.value,
        (SELECT project_id FROM repo_bindings WHERE id = NEW.binding_id),
        'create', NEW.kind
    );
END;

CREATE TRIGGER IF NOT EXISTS audit_repo_identities_delete AFTER DELETE ON repo_identities BEGIN
    INSERT INTO audit_log (actor_user_id, transport, entity_type, entity_id, entity_label,
                           project_id, action, old_value)
    VALUES (
        (SELECT user_id FROM _actor_state WHERE id = 1),
        COALESCE((SELECT transport FROM _actor_state WHERE id = 1), 'system'),
        'repo_identity', OLD.id, OLD.value,
        (SELECT project_id FROM repo_bindings WHERE id = OLD.binding_id),
        'delete', OLD.kind
    );
END;
