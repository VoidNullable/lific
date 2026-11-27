-- Sidebar folders nest, and folders and projects share one personal order.
--
-- A folder (still the `project_groups` table, see 035 for the naming) gains
-- an optional parent. Depth and cycles are enforced in the query layer
-- (queries::project_groups), like the one-folder-per-project rule. SET NULL
-- rather than CASCADE: the query layer moves a deleted folder's contents to
-- its parent first, and anything it somehow missed lands at the top level
-- instead of vanishing.
ALTER TABLE project_groups ADD COLUMN parent_id INTEGER
    REFERENCES project_groups(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_project_groups_parent
    ON project_groups(user_id, parent_id);

-- Order inside any container (the top level or a folder) compares a folder's
-- `project_groups.sort_order` with a project's `user_project_order.sort_order`.
-- Writes renumber both in depth-first order of the whole tree. Until then,
-- keep today's layout, where every folder sat above the loose projects:
-- shift each user's folder ranks below zero, under every project rank.
UPDATE project_groups
SET sort_order = sort_order - (
    SELECT MAX(g.sort_order) + 1 FROM project_groups g
    WHERE g.user_id = project_groups.user_id
);
