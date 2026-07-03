-- LIF-146: index comment threads in the FTS `search_index` so decisions made
-- in comments are discoverable via /api/search, the MCP search tool, and the
-- web UI.
--
-- There is ONE `comments` table (issue comments from 006, extended to also
-- carry page comments in 012 via a nullable page_id + CHECK exactly-one-parent).
-- So a single set of triggers covers both issue and page comments; the row's
-- own issue_id/page_id tells search.rs which parent to link back to.
--
-- The `search_index` FTS columns are fixed at 001 (title, body, entity_type,
-- entity_id, project_id). We index each comment as:
--   entity_type = 'comment'
--   entity_id   = comments.id
--   body        = comments.content   (the searchable text)
--   title       = ''                 (comments have no title of their own)
--   project_id  = the parent issue/page's project_id
-- search.rs joins `comments` back to issues/pages to recover the parent
-- identifier a hit should navigate to.

-- ── Backfill existing comments (triggers only fire on future writes) ──────
INSERT INTO search_index(title, body, entity_type, entity_id, project_id)
SELECT '', c.content, 'comment', c.id,
       COALESCE(i.project_id, pg.project_id)
FROM comments c
LEFT JOIN issues i ON c.issue_id = i.id
LEFT JOIN pages  pg ON c.page_id  = pg.id;

-- ── Keep the FTS index in sync on comment writes ─────────────────────────

CREATE TRIGGER IF NOT EXISTS comments_search_ai AFTER INSERT ON comments BEGIN
    INSERT INTO search_index(title, body, entity_type, entity_id, project_id)
    SELECT '', NEW.content, 'comment', NEW.id,
           COALESCE(
               (SELECT project_id FROM issues WHERE id = NEW.issue_id),
               (SELECT project_id FROM pages  WHERE id = NEW.page_id)
           );
END;

CREATE TRIGGER IF NOT EXISTS comments_search_au AFTER UPDATE ON comments BEGIN
    DELETE FROM search_index WHERE entity_type = 'comment' AND entity_id = OLD.id;
    INSERT INTO search_index(title, body, entity_type, entity_id, project_id)
    SELECT '', NEW.content, 'comment', NEW.id,
           COALESCE(
               (SELECT project_id FROM issues WHERE id = NEW.issue_id),
               (SELECT project_id FROM pages  WHERE id = NEW.page_id)
           );
END;

CREATE TRIGGER IF NOT EXISTS comments_search_ad AFTER DELETE ON comments BEGIN
    DELETE FROM search_index WHERE entity_type = 'comment' AND entity_id = OLD.id;
END;
