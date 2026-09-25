-- LIF-480: page content history, so get_page(since_seq=N) can return only
-- what changed since the agent's last read.
--
-- The audit log already keeps old and new page content, but not the seq
-- each version had, and it is pruned by age. It cannot answer "what was this
-- page's content at seq N", which is the question since_seq asks.
--
-- Each row is one content version and the seq at which it became current,
-- so the content as of any seq N is the newest row with seq <= N. The
-- current content is stored too: without it, the start of the newest
-- version is unknown. seq is instance-wide (migration 045), so a seq read
-- from any entity is a valid point in time for this lookup.
--
-- Bounded per page: the newest 50 versions, dropping older ones once the
-- retained content passes 2,000,000 characters. That keeps a full 50-edit
-- history for pages under 40,000 characters and about 13 versions of the
-- largest page observed in practice (145,000 characters). The newest row is
-- always kept.
--
-- Deleted with the page (ON DELETE CASCADE); a soft-deleted page keeps its
-- history until it is purged. Project archives do not carry this table:
-- seq values are instance-scoped, so they would be meaningless on the
-- importing instance, and archive import suspends triggers, so an imported
-- page starts its history at its first edit.

CREATE TABLE IF NOT EXISTS page_revisions (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    page_id INTEGER NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    seq     INTEGER NOT NULL,
    content TEXT    NOT NULL,
    size    INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_page_revisions_page_seq ON page_revisions(page_id, seq);

-- Existing pages start their history at their current version.
INSERT INTO page_revisions (page_id, seq, content, size)
SELECT id, seq, content, length(content) FROM pages
 WHERE seq IS NOT NULL
   AND NOT EXISTS (SELECT 1 FROM page_revisions r WHERE r.page_id = pages.id);

-- Fires on the seq stamp itself (the nested UPDATE in stamp_pages_ai and
-- stamp_pages_au), where NEW carries both the final content and the new
-- seq, so it does not depend on trigger order. A write that leaves the
-- content alone (title, status, pin, move, soft delete) adds no row.
CREATE TRIGGER IF NOT EXISTS page_revisions_record AFTER UPDATE OF seq ON pages
WHEN NEW.seq IS NOT NULL
 AND NEW.seq IS NOT OLD.seq
 AND NEW.content IS NOT (
     SELECT content FROM page_revisions
      WHERE page_id = NEW.id
      ORDER BY seq DESC LIMIT 1
 )
BEGIN
    INSERT INTO page_revisions (page_id, seq, content, size)
    VALUES (NEW.id, NEW.seq, NEW.content, length(NEW.content));
END;

CREATE TRIGGER IF NOT EXISTS page_revisions_prune AFTER INSERT ON page_revisions
BEGIN
    DELETE FROM page_revisions
     WHERE page_id = NEW.page_id
       AND id NOT IN (
           SELECT id FROM (
               SELECT id,
                      ROW_NUMBER() OVER newest AS position,
                      SUM(size) OVER newest AS retained
                 FROM page_revisions
                WHERE page_id = NEW.page_id
               WINDOW newest AS (ORDER BY seq DESC)
           )
            WHERE position = 1 OR (position <= 50 AND retained <= 2000000)
       );
END;
