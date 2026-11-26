CREATE TABLE project_number_sequences (
    project_id INTEGER PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    issues INTEGER NOT NULL DEFAULT 0 CHECK(typeof(issues) = 'integer' AND issues >= 0),
    pages INTEGER NOT NULL DEFAULT 0 CHECK(typeof(pages) = 'integer' AND pages >= 0),
    plans INTEGER NOT NULL DEFAULT 0 CHECK(typeof(plans) = 'integer' AND plans >= 0)
);

CREATE TABLE workspace_page_sequence (
    id INTEGER PRIMARY KEY CHECK(id = 1),
    value INTEGER NOT NULL CHECK(typeof(value) = 'integer' AND value >= 0)
);

-- Tombstones and move aliases still own their numbers.
INSERT INTO project_number_sequences (project_id, issues, pages, plans)
SELECT p.id,
    MAX(
        COALESCE((SELECT MAX(sequence) FROM issues WHERE project_id = p.id), 0),
        COALESCE((SELECT MAX(sequence) FROM issue_aliases WHERE project_id = p.id), 0)
    ),
    COALESCE((SELECT MAX(sequence) FROM pages WHERE project_id = p.id), 0),
    COALESCE((SELECT MAX(sequence) FROM plans WHERE project_id = p.id), 0)
FROM projects p;

INSERT INTO workspace_page_sequence (id, value)
SELECT 1, COALESCE(MAX(sequence), 0) FROM pages WHERE project_id IS NULL;

-- Explicit sequence writes (including imports) must also advance the marks.
CREATE TRIGGER number_issues_insert AFTER INSERT ON issues BEGIN
    INSERT INTO project_number_sequences (project_id, issues) VALUES (NEW.project_id, NEW.sequence)
    ON CONFLICT(project_id) DO UPDATE SET issues = MAX(issues, excluded.issues);
END;
CREATE TRIGGER number_issues_update AFTER UPDATE OF project_id, sequence ON issues BEGIN
    INSERT INTO project_number_sequences (project_id, issues) VALUES (NEW.project_id, NEW.sequence)
    ON CONFLICT(project_id) DO UPDATE SET issues = MAX(issues, excluded.issues);
END;
CREATE TRIGGER number_aliases_insert AFTER INSERT ON issue_aliases BEGIN
    INSERT INTO project_number_sequences (project_id, issues) VALUES (NEW.project_id, NEW.sequence)
    ON CONFLICT(project_id) DO UPDATE SET issues = MAX(issues, excluded.issues);
END;
CREATE TRIGGER number_aliases_update AFTER UPDATE OF project_id, sequence ON issue_aliases BEGIN
    INSERT INTO project_number_sequences (project_id, issues) VALUES (NEW.project_id, NEW.sequence)
    ON CONFLICT(project_id) DO UPDATE SET issues = MAX(issues, excluded.issues);
END;
CREATE TRIGGER number_pages_insert AFTER INSERT ON pages WHEN NEW.sequence IS NOT NULL BEGIN
    INSERT INTO project_number_sequences (project_id, pages)
    SELECT NEW.project_id, NEW.sequence WHERE NEW.project_id IS NOT NULL
    ON CONFLICT(project_id) DO UPDATE SET pages = MAX(pages, excluded.pages);
    UPDATE workspace_page_sequence SET value = MAX(value, NEW.sequence) WHERE NEW.project_id IS NULL;
END;
CREATE TRIGGER number_pages_update AFTER UPDATE OF project_id, sequence ON pages WHEN NEW.sequence IS NOT NULL BEGIN
    INSERT INTO project_number_sequences (project_id, pages)
    SELECT NEW.project_id, NEW.sequence WHERE NEW.project_id IS NOT NULL
    ON CONFLICT(project_id) DO UPDATE SET pages = MAX(pages, excluded.pages);
    UPDATE workspace_page_sequence SET value = MAX(value, NEW.sequence) WHERE NEW.project_id IS NULL;
END;
CREATE TRIGGER number_plans_insert AFTER INSERT ON plans BEGIN
    INSERT INTO project_number_sequences (project_id, plans) VALUES (NEW.project_id, NEW.sequence)
    ON CONFLICT(project_id) DO UPDATE SET plans = MAX(plans, excluded.plans);
END;
CREATE TRIGGER number_plans_update AFTER UPDATE OF project_id, sequence ON plans BEGIN
    INSERT INTO project_number_sequences (project_id, plans) VALUES (NEW.project_id, NEW.sequence)
    ON CONFLICT(project_id) DO UPDATE SET plans = MAX(plans, excluded.plans);
END;
