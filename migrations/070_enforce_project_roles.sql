-- LIF-518: every instance enforces project roles; the legacy mode is gone.
--
-- An instance upgraded from before project membership existed ran with
-- `instance_settings.authz_enforced = 0`: anyone who could sign in read and
-- wrote every project, and roles only mattered for lead-level actions. From
-- 3.0 roles are checked everywhere. So that the access people actually used
-- survives, an instance still in that mode is seeded from evidence first:
--
--   * a project's lead (`projects.lead_user_id`) holds `lead`, even if an
--     earlier membership row said otherwise;
--   * anyone who wrote in a project (issues, pages, comments, plans, per the
--     audit log and comment authorship) becomes a `maintainer`. Work done
--     through a connected tool counts for the human who owns it.
--
-- Other existing membership rows are left exactly as they are, and admins
-- need none. Every active user who could reach a project before and now
-- holds no role in it is recorded in `authz_upgrade_losses`, which the server
-- logs once at startup and `lific doctor` reports until access is restored
-- (`lific member add`) or the user is removed. An instance that already
-- enforced roles, every fresh install since 2.0, is left as it was and
-- records nothing.

CREATE TABLE authz_upgrade_losses (
    user_id     INTEGER NOT NULL REFERENCES users(id)    ON DELETE CASCADE,
    project_id  INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    recorded_at TEXT    NOT NULL DEFAULT (datetime('now')),
    -- Set once a server start has logged this row, so the log is not repeated.
    logged_at   TEXT,
    PRIMARY KEY (user_id, project_id)
);

-- One row when this instance is in legacy mode, none otherwise. A missing
-- settings row reads as legacy, as the old code default did.
CREATE TEMP TABLE _authz_legacy AS
SELECT 1 AS legacy
WHERE NOT EXISTS (
    SELECT 1 FROM instance_settings WHERE id = 1 AND authz_enforced = 1
);

-- Who acts for whom: a connected tool (bot) counts as its owner.
CREATE TEMP TABLE _authz_principal AS
SELECT id,
       CASE WHEN is_bot = 1 AND owner_id IS NOT NULL THEN owner_id ELSE id END AS principal
FROM users;

INSERT INTO project_members (project_id, user_id, role)
SELECT p.id, p.lead_user_id, 'lead'
FROM projects p
JOIN users u ON u.id = p.lead_user_id
JOIN _authz_legacy
WHERE true
ON CONFLICT (project_id, user_id) DO UPDATE SET role = 'lead';

CREATE TEMP TABLE _authz_evidence AS
SELECT actor_user_id AS user_id, project_id
FROM audit_log
WHERE actor_user_id IS NOT NULL
  AND project_id IS NOT NULL
  AND entity_type IN ('issue', 'page', 'comment', 'plan', 'plan_step')
UNION
SELECT c.user_id, i.project_id
FROM comments c JOIN issues i ON i.id = c.issue_id
WHERE c.user_id IS NOT NULL
UNION
SELECT c.user_id, pg.project_id
FROM comments c JOIN pages pg ON pg.id = c.page_id
WHERE c.user_id IS NOT NULL AND pg.project_id IS NOT NULL;

INSERT OR IGNORE INTO project_members (project_id, user_id, role)
SELECT DISTINCT p.id, u.id, 'maintainer'
FROM _authz_evidence e
JOIN _authz_principal m ON m.id = e.user_id
JOIN users u ON u.id = m.principal
JOIN projects p ON p.id = e.project_id
JOIN _authz_legacy
WHERE u.is_admin = 0;

INSERT OR IGNORE INTO authz_upgrade_losses (user_id, project_id)
SELECT u.id, p.id
FROM users u
CROSS JOIN projects p
JOIN _authz_legacy
WHERE u.is_admin = 0
  AND u.is_active = 1
  AND (u.is_bot = 0 OR u.owner_id IS NULL)
  AND NOT EXISTS (
      SELECT 1 FROM project_members pm
      WHERE pm.project_id = p.id AND pm.user_id = u.id
  );

DROP TABLE _authz_evidence;
DROP TABLE _authz_principal;
DROP TABLE _authz_legacy;

ALTER TABLE instance_settings DROP COLUMN authz_enforced;
