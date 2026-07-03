-- LIF-264 / LIF-265: import provenance + idempotency marker.
--
-- Every issue created by an importer records its origin in `source`, a stable
-- string unique per external issue:
--   github:owner/name#NUMBER
--   linear:TEAM-123
--   jira:<site>:KEY-123
--
-- The partial UNIQUE index makes re-running an import a no-op: a second attempt
-- to insert the same source collides and the importer treats it as "already
-- imported, skip." NULL sources (every hand-created issue) are excluded from
-- the uniqueness constraint via the WHERE clause, so normal issue creation is
-- untouched.
ALTER TABLE issues ADD COLUMN source TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_issues_source
    ON issues(source) WHERE source IS NOT NULL;
