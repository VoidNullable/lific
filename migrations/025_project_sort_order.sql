-- LIF-233: user-controllable ordering for projects in the sidebar.
-- Integer rank, reindexed sequentially on every reorder (see reorder_projects).
-- All existing rows default to 0; list_projects tie-breaks on name, so until
-- the user drags something the order is identical to the previous alphabetical
-- behaviour.
ALTER TABLE projects ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0;
