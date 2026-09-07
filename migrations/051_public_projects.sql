-- LIF-465: per-project anonymous publication flag.
--
-- One column, default 0. Publication is a property of the project, not a
-- secret handed out per visitor: `is_public = 1` means "anyone who can reach
-- this instance may read this project's current issues at /public/<IDENT>",
-- and nothing else. There is no token, no expiry and no per-reader state,
-- because the contract (LIF-DOC-30) calls this genuinely public rather than
-- an unguessable-link mechanism.
--
-- ── Why a column and not a table ──────────────────────────────────────
-- A publication row would imply per-publication settings (an audience, a
-- scope, an expiry) that deliberately do not exist. The flag is one bit of
-- project configuration alongside `lead_user_id` and `sort_order`, edited
-- through the same Lead-gated `PUT /api/projects/{id}` those go through, and
-- unpublishing must leave nothing behind: flipping the bit back to 0 closes
-- every public read immediately, and republishing later resumes the SAME
-- address because the address is derived from the identifier, not minted.
--
-- ── DEFAULT 0 is the security property ────────────────────────────────
-- Every project that exists when this migration runs, and every project
-- created after it, is private until a human with Lead (or admin) rights
-- says otherwise in the UI, where the exposure warning lives. There is no
-- backfill, no "public by default for projects without members" shortcut,
-- and no other write path in the codebase sets this to 1.

ALTER TABLE projects ADD COLUMN is_public INTEGER NOT NULL DEFAULT 0;

-- The public read path resolves a project by identifier and immediately
-- requires publication, so the index carries both halves of that predicate.
-- Partial on `is_public = 1`: published projects are the rare case, and no
-- query ever asks this index for a private one.
CREATE INDEX IF NOT EXISTS idx_projects_public
    ON projects(identifier) WHERE is_public = 1;

-- ════════════════════════════════════════════════════════════════════
-- Audit
--
-- Publishing a project exposes every current issue description, comment and
-- linked attachment in it to the open internet. That is the single most
-- consequential project setting there is, so it gets its own audit row with
-- the same `_actor_state` attribution every other trigger uses (018).
--
-- A separate trigger rather than another arm bolted onto
-- `audit_projects_update` (018, recreated verbatim by 039): that trigger is
-- rewritten wholesale whenever the projects table is rebuilt, and adding an
-- arm to it here would mean re-emitting all of it, which is exactly the kind
-- of copy the 039 rebuild already had to make once. `AFTER UPDATE OF
-- is_public` fires only for statements that name the column, and the WHEN
-- guard drops the no-op re-publish of an already published project.
--
-- old/new are recorded as the words a reader wants ('public' / 'private'),
-- not as 0/1, following 018's "resolved names, not ids" rule.
-- ════════════════════════════════════════════════════════════════════

CREATE TRIGGER IF NOT EXISTS audit_projects_publication
AFTER UPDATE OF is_public ON projects
WHEN OLD.is_public IS NOT NEW.is_public
BEGIN
    INSERT INTO audit_log (actor_user_id, transport, entity_type, entity_id, entity_label,
                           project_id, action, field, old_value, new_value)
    VALUES (
        (SELECT user_id FROM _actor_state WHERE id = 1),
        COALESCE((SELECT transport FROM _actor_state WHERE id = 1), 'system'),
        'project', NEW.id, NEW.identifier, NEW.id, 'update', 'is_public',
        CASE WHEN OLD.is_public = 1 THEN 'public' ELSE 'private' END,
        CASE WHEN NEW.is_public = 1 THEN 'public' ELSE 'private' END
    );
END;
