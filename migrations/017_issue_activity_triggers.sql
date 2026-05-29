-- LIF-116: keep issues.updated_at reflecting *activity*, not just edits to
-- the issue row itself. The base schema (001) only bumps updated_at on a
-- direct UPDATE of `issues`. Comments and label changes are activity on the
-- issue too, so sorting by "last activity" should account for them.
--
-- Comments live in a shared table (issue_id XOR page_id, see migration 012),
-- so the comment triggers are guarded to fire only for issue-attached rows.
-- issue_labels is issue-only, so its triggers are unguarded.

-- ── Comments → bump parent issue's updated_at ──────────────────────────

CREATE TRIGGER IF NOT EXISTS comments_bump_issue_ai
AFTER INSERT ON comments
WHEN NEW.issue_id IS NOT NULL
BEGIN
    UPDATE issues SET updated_at = datetime('now') WHERE id = NEW.issue_id;
END;

CREATE TRIGGER IF NOT EXISTS comments_bump_issue_au
AFTER UPDATE ON comments
WHEN NEW.issue_id IS NOT NULL
BEGIN
    UPDATE issues SET updated_at = datetime('now') WHERE id = NEW.issue_id;
END;

CREATE TRIGGER IF NOT EXISTS comments_bump_issue_ad
AFTER DELETE ON comments
WHEN OLD.issue_id IS NOT NULL
BEGIN
    UPDATE issues SET updated_at = datetime('now') WHERE id = OLD.issue_id;
END;

-- ── Label attach/detach → bump issue's updated_at ──────────────────────

CREATE TRIGGER IF NOT EXISTS issue_labels_bump_ai
AFTER INSERT ON issue_labels
BEGIN
    UPDATE issues SET updated_at = datetime('now') WHERE id = NEW.issue_id;
END;

CREATE TRIGGER IF NOT EXISTS issue_labels_bump_ad
AFTER DELETE ON issue_labels
BEGIN
    UPDATE issues SET updated_at = datetime('now') WHERE id = OLD.issue_id;
END;
