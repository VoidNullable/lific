-- LIF-524: each project's conventions page, the rules agents and people
-- follow when working in its tracker.
--
-- One row per project that has made a choice about conventions. `page_id` is
-- the conventions page itself: an ordinary page that this row marks, so the
-- page keeps its editor, history, comments and archive handling. A project
-- with no row has never had a conventions page, and only those projects get
-- the briefing's hint to create one. A row with a NULL `page_id` means the
-- page was purged from the trash or the hint was dismissed; either way the
-- hint stays quiet.
--
-- The primary key keeps it to one conventions page per project. Pointing the
-- row at a new page (`create_page(conventions: true)` after a delete) leaves
-- the old tombstone an ordinary page, so restoring it cannot make two.
--
-- `enabled` is the page view's toggle: off keeps the text but takes the page
-- out of the briefing.

CREATE TABLE IF NOT EXISTS project_conventions (
    project_id INTEGER PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    page_id    INTEGER UNIQUE REFERENCES pages(id) ON DELETE SET NULL,
    enabled    INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1))
);
