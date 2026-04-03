-- Lific initial schema
-- String enums, integer PKs, no UUID indirection.

CREATE TABLE IF NOT EXISTS projects (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT    NOT NULL,
    identifier  TEXT    NOT NULL UNIQUE,
    description TEXT    NOT NULL DEFAULT '',
    emoji       TEXT,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS modules (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id  INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name        TEXT    NOT NULL,
    description TEXT    NOT NULL DEFAULT '',
    status      TEXT    NOT NULL DEFAULT 'active'
                        CHECK(status IN ('backlog','planned','active','paused','done','cancelled')),
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS labels (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id  INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name        TEXT    NOT NULL,
    color       TEXT    NOT NULL DEFAULT '#6B7280',
    UNIQUE(project_id, name)
);

CREATE TABLE IF NOT EXISTS issues (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id  INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    sequence    INTEGER NOT NULL,
    title       TEXT    NOT NULL,
    description TEXT    NOT NULL DEFAULT '',
    status      TEXT    NOT NULL DEFAULT 'backlog'
                        CHECK(status IN ('backlog','todo','active','done','cancelled')),
    priority    TEXT    NOT NULL DEFAULT 'none'
                        CHECK(priority IN ('urgent','high','medium','low','none')),
    module_id   INTEGER REFERENCES modules(id) ON DELETE SET NULL,
    sort_order  REAL    NOT NULL DEFAULT 0,
    start_date  TEXT,
    target_date TEXT,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    UNIQUE(project_id, sequence)
);

CREATE TABLE IF NOT EXISTS issue_labels (
    issue_id    INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    label_id    INTEGER NOT NULL REFERENCES labels(id) ON DELETE CASCADE,
    PRIMARY KEY (issue_id, label_id)
);

CREATE TABLE IF NOT EXISTS issue_relations (
    source_id      INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    target_id      INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    relation_type  TEXT    NOT NULL
                   CHECK(relation_type IN ('blocks','relates_to','duplicate')),
    PRIMARY KEY (source_id, target_id, relation_type)
);

CREATE TABLE IF NOT EXISTS folders (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id  INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    parent_id   INTEGER REFERENCES folders(id) ON DELETE CASCADE,
    name        TEXT    NOT NULL,
    sort_order  REAL    NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS pages (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id  INTEGER REFERENCES projects(id) ON DELETE CASCADE,
    folder_id   INTEGER REFERENCES folders(id) ON DELETE SET NULL,
    title       TEXT    NOT NULL,
    content     TEXT    NOT NULL DEFAULT '',
    sort_order  REAL    NOT NULL DEFAULT 0,
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS page_issue_links (
    page_id     INTEGER NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    issue_id    INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    PRIMARY KEY (page_id, issue_id)
);

-- Indexes for common query patterns
CREATE INDEX IF NOT EXISTS idx_issues_project     ON issues(project_id);
CREATE INDEX IF NOT EXISTS idx_issues_status      ON issues(project_id, status);
CREATE INDEX IF NOT EXISTS idx_issues_priority    ON issues(project_id, priority);
CREATE INDEX IF NOT EXISTS idx_issues_module      ON issues(module_id);
CREATE INDEX IF NOT EXISTS idx_modules_project    ON modules(project_id);
CREATE INDEX IF NOT EXISTS idx_labels_project     ON labels(project_id);
CREATE INDEX IF NOT EXISTS idx_pages_project      ON pages(project_id);
CREATE INDEX IF NOT EXISTS idx_pages_folder       ON pages(folder_id);
CREATE INDEX IF NOT EXISTS idx_folders_project    ON folders(project_id);
CREATE INDEX IF NOT EXISTS idx_relations_source   ON issue_relations(source_id);
CREATE INDEX IF NOT EXISTS idx_relations_target   ON issue_relations(target_id);

-- FTS5 for full-text search across issues and pages
CREATE VIRTUAL TABLE IF NOT EXISTS search_index USING fts5(
    title,
    body,
    entity_type,   -- 'issue' or 'page'
    entity_id,     -- references issues.id or pages.id
    project_id,
    tokenize='porter unicode61'
);

-- Triggers to keep FTS index in sync

CREATE TRIGGER IF NOT EXISTS issues_ai AFTER INSERT ON issues BEGIN
    INSERT INTO search_index(title, body, entity_type, entity_id, project_id)
    VALUES (NEW.title, NEW.description, 'issue', NEW.id, NEW.project_id);
END;

CREATE TRIGGER IF NOT EXISTS issues_au AFTER UPDATE ON issues BEGIN
    DELETE FROM search_index WHERE entity_type = 'issue' AND entity_id = OLD.id;
    INSERT INTO search_index(title, body, entity_type, entity_id, project_id)
    VALUES (NEW.title, NEW.description, 'issue', NEW.id, NEW.project_id);
END;

CREATE TRIGGER IF NOT EXISTS issues_ad AFTER DELETE ON issues BEGIN
    DELETE FROM search_index WHERE entity_type = 'issue' AND entity_id = OLD.id;
END;

CREATE TRIGGER IF NOT EXISTS pages_ai AFTER INSERT ON pages BEGIN
    INSERT INTO search_index(title, body, entity_type, entity_id, project_id)
    VALUES (NEW.title, NEW.content, 'page', NEW.id, NEW.project_id);
END;

CREATE TRIGGER IF NOT EXISTS pages_au AFTER UPDATE ON pages BEGIN
    DELETE FROM search_index WHERE entity_type = 'page' AND entity_id = OLD.id;
    INSERT INTO search_index(title, body, entity_type, entity_id, project_id)
    VALUES (NEW.title, NEW.content, 'page', NEW.id, NEW.project_id);
END;

CREATE TRIGGER IF NOT EXISTS pages_ad AFTER DELETE ON pages BEGIN
    DELETE FROM search_index WHERE entity_type = 'page' AND entity_id = OLD.id;
END;

-- Auto-update updated_at timestamps

CREATE TRIGGER IF NOT EXISTS projects_updated AFTER UPDATE ON projects BEGIN
    UPDATE projects SET updated_at = datetime('now') WHERE id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS issues_updated AFTER UPDATE ON issues BEGIN
    UPDATE issues SET updated_at = datetime('now') WHERE id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS modules_updated AFTER UPDATE ON modules BEGIN
    UPDATE modules SET updated_at = datetime('now') WHERE id = NEW.id;
END;

CREATE TRIGGER IF NOT EXISTS pages_updated AFTER UPDATE ON pages BEGIN
    UPDATE pages SET updated_at = datetime('now') WHERE id = NEW.id;
END;
