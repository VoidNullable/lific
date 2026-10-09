-- Archived projects: a project taken out of the live database and kept as
-- one compressed file in `<data dir>/archived-projects/`, tracked here so an
-- admin can bring it back.
--
-- The file is a portable project archive, the same format `lific
-- project-archive export` writes, so it also imports on another instance.
-- `restore` holds what that format leaves out on purpose but this instance
-- can put back: the lead, the memberships, publication, and the local
-- accounts behind comments, attachments and history. Only an unarchive
-- reads it, so it is JSON rather than tables.
--
-- The listing columns (name, counts, ...) are copied out at archive time so
-- the settings page never has to open an archive to describe it.

CREATE TABLE IF NOT EXISTS archived_projects (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    identifier   TEXT    NOT NULL,
    name         TEXT    NOT NULL,
    description  TEXT    NOT NULL DEFAULT '',
    emoji        TEXT,
    file_name    TEXT    NOT NULL UNIQUE,
    sha256       TEXT    NOT NULL,
    size_bytes   INTEGER NOT NULL,
    issue_count  INTEGER NOT NULL DEFAULT 0,
    page_count   INTEGER NOT NULL DEFAULT 0,
    restore      TEXT    NOT NULL DEFAULT '{}',
    archived_by  INTEGER REFERENCES users(id) ON DELETE SET NULL,
    archived_at  TEXT    NOT NULL DEFAULT (datetime('now'))
);
