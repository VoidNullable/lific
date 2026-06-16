ALTER TABLE pages ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
CREATE INDEX idx_pages_pinned ON pages(pinned);
