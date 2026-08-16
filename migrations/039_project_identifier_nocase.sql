-- LIF-348: make `projects.identifier` case-insensitive.
--
-- Project identifiers were the only case-SENSITIVE resolver left in the
-- system (modules, folders, usernames and emails are all NOCASE), so
-- `get_issue("lif-42")` failed while `LIF-42` worked, across MCP, REST and
-- CLI. The cause is the column declaration in 001: `identifier TEXT NOT NULL
-- UNIQUE` with the default BINARY collation.
--
-- SQLite cannot ALTER a column's collating sequence in place, so the table is
-- rebuilt. Putting `COLLATE NOCASE` on the column (rather than a per-callsite
-- `COLLATE NOCASE` in each query) makes every bare `identifier = ?` comparison
-- case-insensitive at once — resolve_project_identifier, and the project half
-- of issue (`LIF-42`) and page (`LIF-DOC-3`) identifier resolution — and makes
-- the UNIQUE index case-insensitive too, so `abc` can no longer be created
-- alongside `ABC`.
--
-- Foreign keys: unlike 012 (comments, which nothing references), a dozen
-- tables reference projects(id) ON DELETE CASCADE. Under `PRAGMA
-- foreign_keys=ON`, `DROP TABLE projects` performs an implicit DELETE FROM
-- that would cascade every issue, page and plan out of existence. The pragma
-- is a no-op inside a transaction, so migrate.rs runs this migration with
-- foreign keys disabled around the savepoint (FK_REBUILD_MIGRATIONS) and
-- verifies with PRAGMA foreign_key_check afterwards. This is the standard
-- SQLite table-rebuild procedure. Row ids are preserved verbatim, so every
-- child reference stays valid.
--
-- Duplicate handling: an existing database could hold both `ABC` and `abc`,
-- which the new UNIQUE index would reject. Resolve it deterministically
-- instead of failing the migration (this runs at startup — it must not be able
-- to abort). The oldest row (lowest id) in each case-insensitive group keeps
-- its identifier untouched; every later collider is renamed to a synthetic
-- `<letter><k>` name, where `k` is the collider's 1-based rank across all
-- colliders ordered by id.
--
-- Why that cannot collide, which a mnemonic `base || rn` scheme could not
-- guarantee (`ABCDE`/`abcde` and `ABCDF`/`abcdf` both truncate to `ABCD2`):
--   * against other generated names — every candidate is one letter followed
--     by the decimal `k` (with the last-resort form additionally ending in
--     `Z`). Two candidates are equal only if they share a letter and a digit
--     string, i.e. only if they came from the same `k`, and `k` is unique per
--     collider. The `Z<k>Z` last resort ends in a letter, so it can never
--     equal a `<letter><k>` form, whose final character is always a digit.
--   * against surviving identifiers — each candidate is tested with a
--     case-insensitive lookup against every identifier in the old table (a
--     superset of the ones that survive), walking P/Q/R/X/Y/Z until one is
--     free. Exhausting all six needs six specific projects to already exist.
-- Length: `k` counts case-duplicate rows, realistically 1, so `P1` sits well
-- inside the 5-character grammar `validate_identifier` enforces.
--
-- Expected to be a no-op on every real database.

CREATE TABLE projects_new (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    name         TEXT    NOT NULL,
    identifier   TEXT    NOT NULL COLLATE NOCASE UNIQUE,
    description  TEXT    NOT NULL DEFAULT '',
    emoji        TEXT,
    created_at   TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at   TEXT    NOT NULL DEFAULT (datetime('now')),
    lead_user_id INTEGER REFERENCES users(id) ON DELETE SET NULL,
    sort_order   INTEGER NOT NULL DEFAULT 0
);

WITH ranked AS (
    SELECT p.id, p.name, p.identifier, p.description, p.emoji,
           p.created_at, p.updated_at, p.lead_user_id, p.sort_order,
           ROW_NUMBER() OVER (PARTITION BY lower(p.identifier) ORDER BY p.id) AS rn
    FROM projects p
),
colliders AS (
    SELECT r.id, ROW_NUMBER() OVER (ORDER BY r.id) AS k
    FROM ranked r
    WHERE r.rn > 1
),
renamed AS (
    SELECT c.id,
           CASE
               WHEN NOT EXISTS (SELECT 1 FROM projects t
                                 WHERE lower(t.identifier) = lower('P' || c.k)) THEN 'P' || c.k
               WHEN NOT EXISTS (SELECT 1 FROM projects t
                                 WHERE lower(t.identifier) = lower('Q' || c.k)) THEN 'Q' || c.k
               WHEN NOT EXISTS (SELECT 1 FROM projects t
                                 WHERE lower(t.identifier) = lower('R' || c.k)) THEN 'R' || c.k
               WHEN NOT EXISTS (SELECT 1 FROM projects t
                                 WHERE lower(t.identifier) = lower('X' || c.k)) THEN 'X' || c.k
               WHEN NOT EXISTS (SELECT 1 FROM projects t
                                 WHERE lower(t.identifier) = lower('Y' || c.k)) THEN 'Y' || c.k
               WHEN NOT EXISTS (SELECT 1 FROM projects t
                                 WHERE lower(t.identifier) = lower('Z' || c.k)) THEN 'Z' || c.k
               ELSE 'Z' || c.k || 'Z'
           END AS identifier
    FROM colliders c
)
INSERT INTO projects_new
    (id, name, identifier, description, emoji, created_at, updated_at, lead_user_id, sort_order)
SELECT r.id,
       r.name,
       COALESCE(n.identifier, r.identifier),
       r.description, r.emoji, r.created_at, r.updated_at, r.lead_user_id, r.sort_order
FROM ranked r
LEFT JOIN renamed n ON n.id = r.id;

DROP TABLE projects;
ALTER TABLE projects_new RENAME TO projects;

-- Triggers live on the table, so DROP TABLE took all four with it. Recreated
-- verbatim from 001 (projects_updated) and 018 (the audit trio).

CREATE TRIGGER projects_updated AFTER UPDATE ON projects BEGIN
    UPDATE projects SET updated_at = datetime('now') WHERE id = NEW.id;
END;

CREATE TRIGGER audit_projects_insert AFTER INSERT ON projects BEGIN
    INSERT INTO audit_log (actor_user_id, transport, entity_type, entity_id, entity_label,
                           project_id, action, new_value)
    VALUES (
        (SELECT user_id FROM _actor_state WHERE id = 1),
        COALESCE((SELECT transport FROM _actor_state WHERE id = 1), 'system'),
        'project', NEW.id, NEW.identifier, NEW.id, 'create', NEW.name
    );
END;

CREATE TRIGGER audit_projects_delete AFTER DELETE ON projects BEGIN
    INSERT INTO audit_log (actor_user_id, transport, entity_type, entity_id, entity_label,
                           project_id, action, old_value)
    VALUES (
        (SELECT user_id FROM _actor_state WHERE id = 1),
        COALESCE((SELECT transport FROM _actor_state WHERE id = 1), 'system'),
        'project', OLD.id, OLD.identifier, OLD.id, 'delete', OLD.name
    );
END;

CREATE TRIGGER audit_projects_update AFTER UPDATE ON projects BEGIN
    INSERT INTO audit_log (actor_user_id, transport, entity_type, entity_id, entity_label,
                           project_id, action, field, old_value, new_value)
    SELECT (SELECT user_id FROM _actor_state WHERE id = 1),
           COALESCE((SELECT transport FROM _actor_state WHERE id = 1), 'system'),
           'project', NEW.id, NEW.identifier, NEW.id, 'update', 'name', OLD.name, NEW.name
    WHERE OLD.name IS NOT NEW.name;

    INSERT INTO audit_log (actor_user_id, transport, entity_type, entity_id, entity_label,
                           project_id, action, field, old_value, new_value)
    SELECT (SELECT user_id FROM _actor_state WHERE id = 1),
           COALESCE((SELECT transport FROM _actor_state WHERE id = 1), 'system'),
           'project', NEW.id, NEW.identifier, NEW.id, 'update', 'identifier', OLD.identifier, NEW.identifier
    WHERE OLD.identifier IS NOT NEW.identifier;

    INSERT INTO audit_log (actor_user_id, transport, entity_type, entity_id, entity_label,
                           project_id, action, field, old_value, new_value)
    SELECT (SELECT user_id FROM _actor_state WHERE id = 1),
           COALESCE((SELECT transport FROM _actor_state WHERE id = 1), 'system'),
           'project', NEW.id, NEW.identifier, NEW.id, 'update', 'description', OLD.description, NEW.description
    WHERE OLD.description IS NOT NEW.description;

    INSERT INTO audit_log (actor_user_id, transport, entity_type, entity_id, entity_label,
                           project_id, action, field, old_value, new_value)
    SELECT (SELECT user_id FROM _actor_state WHERE id = 1),
           COALESCE((SELECT transport FROM _actor_state WHERE id = 1), 'system'),
           'project', NEW.id, NEW.identifier, NEW.id, 'update', 'emoji', OLD.emoji, NEW.emoji
    WHERE OLD.emoji IS NOT NEW.emoji;

    INSERT INTO audit_log (actor_user_id, transport, entity_type, entity_id, entity_label,
                           project_id, action, field, old_value, new_value)
    SELECT (SELECT user_id FROM _actor_state WHERE id = 1),
           COALESCE((SELECT transport FROM _actor_state WHERE id = 1), 'system'),
           'project', NEW.id, NEW.identifier, NEW.id, 'update', 'lead',
           (SELECT username FROM users WHERE id = OLD.lead_user_id),
           (SELECT username FROM users WHERE id = NEW.lead_user_id)
    WHERE OLD.lead_user_id IS NOT NEW.lead_user_id;
END;
