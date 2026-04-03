-- Add project-scoped sequence numbers to pages for human identifiers.
-- Pages get identifiers like LIF-DOC-1, PRO-DOC-3.
-- Workspace-level pages (project_id IS NULL) use sequence alone.

ALTER TABLE pages ADD COLUMN sequence INTEGER;

-- Backfill existing pages with sequential numbers per project
UPDATE pages SET sequence = (
    SELECT COUNT(*) FROM pages p2
    WHERE p2.project_id IS pages.project_id
      AND p2.id <= pages.id
);

-- Enforce uniqueness going forward
CREATE UNIQUE INDEX IF NOT EXISTS idx_pages_project_sequence
    ON pages(project_id, sequence);
