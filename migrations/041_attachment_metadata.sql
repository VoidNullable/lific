-- LIF-418: richer attachment metadata for the media viewer.
--
-- Three nullable columns, all populated opportunistically rather than
-- required:
--
-- * `width` / `height` are the decoded pixel dimensions of a raster image
--   (png/jpeg/gif/webp), recorded at upload time. NULL for every other type,
--   and NULL for rasters uploaded before this migration, so readers must
--   treat "no dimensions" as normal rather than as corruption. The frontend
--   uses them to reserve layout space before the bytes arrive, which is what
--   stops an image grid from reflowing as it loads.
--
-- * `alt_text` is the caller-supplied accessibility description, set through
--   `PATCH /api/attachments/{id}`. NULL means "never described"; the empty
--   string is normalized to NULL by the handler so there is exactly one
--   representation of "no alt text".
--
-- Nothing derived from the bytes themselves lives here beyond the dimensions.
-- Thumbnails stay on disk next to the blobs (`attachments/thumbs/<sha>.webp`)
-- and are regenerated on demand, so they need no schema of their own.

ALTER TABLE attachments ADD COLUMN width INTEGER;
ALTER TABLE attachments ADD COLUMN height INTEGER;
ALTER TABLE attachments ADD COLUMN alt_text TEXT;
