-- LIF-196: runtime toggle for project-scoped default-deny authorization
-- (epic LIF-194, design LIF-DOC-7).
--
-- Off by default so every existing instance keeps today's exact behavior
-- (any authenticated request, and any anonymous-with-valid-key request,
-- can read and mutate project content; only project-lead/admin actions are
-- gated) until an admin explicitly opts in — after verifying `project_members`
-- rows exist for everyone who needs access (LIF-195 backfill only seeds
-- `lead` rows from `projects.lead_user_id`).
ALTER TABLE instance_settings ADD COLUMN authz_enforced INTEGER NOT NULL DEFAULT 0;
