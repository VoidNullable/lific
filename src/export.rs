use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{self, Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::db::{
    models::Comment, models::Folder, models::Issue, models::Page, models::Project, queries,
};
use crate::error::LificError;

/// `Deserialize` matters as much as `Serialize` here: the CLI's HTTP backend
/// asks the server for the bundle and hands it straight to
/// [`write_bundle_to_directory`], the same writer the SQL backend uses, so a
/// remote export lands on disk exactly like a local one (LIF-341).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportFile {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportBundle {
    pub root: String,
    pub files: Vec<ExportFile>,
}

/// Export limits are on the uncompressed representation. They bound both
/// query result memory and the amount of data a single request can serialize.
pub const MAX_EXPORT_FILES: usize = 500;
pub const MAX_EXPORT_COMMENTS: i64 = 500;
pub const MAX_EXPORT_FILE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_EXPORT_TOTAL_BYTES: usize = 128 * 1024 * 1024;
const MAX_EXPORT_METADATA_ITEMS: i64 = 5_000;
// A JSON string can encode one source byte as a six-byte `\u00xx` escape.
// Keep that representation bounded without rejecting a bundle ZIP accepts.
const MAX_EXPORT_JSON_BYTES: usize = MAX_EXPORT_TOTAL_BYTES * 6 + 64 * 1024;

fn ensure_text_size(label: &str, value: &str) -> Result<(), LificError> {
    if value.len() > MAX_EXPORT_FILE_BYTES {
        return Err(LificError::BadRequest(format!(
            "{label} exceeds the {} byte limit",
            MAX_EXPORT_FILE_BYTES
        )));
    }
    Ok(())
}

fn ensure_comment_sizes(conn: &Connection, issue_id: i64) -> Result<(), LificError> {
    // SQLite's length(TEXT) reports characters, not UTF-8 bytes.  Use the
    // BLOB form for the limit that protects the rendered file allocation.
    let (too_large, total_bytes): (i64, i64) = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM comments WHERE issue_id = ?1 AND length(CAST(content AS BLOB)) > ?2),
                COALESCE(SUM(length(CAST(content AS BLOB))), 0)
           FROM comments WHERE issue_id = ?1",
        rusqlite::params![issue_id, MAX_EXPORT_FILE_BYTES],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if too_large != 0 {
        return Err(LificError::BadRequest(format!(
            "an issue comment exceeds the {} byte export limit",
            MAX_EXPORT_FILE_BYTES
        )));
    }
    if total_bytes > MAX_EXPORT_FILE_BYTES as i64 {
        return Err(LificError::BadRequest(format!(
            "issue comments exceed the {} byte aggregate export limit",
            MAX_EXPORT_FILE_BYTES
        )));
    }
    Ok(())
}

fn ensure_issue_metadata_size(conn: &Connection, issue_id: i64) -> Result<(), LificError> {
    let (items, bytes): (i64, i64) = conn.query_row(
        "SELECT
            (SELECT COUNT(*) FROM issue_labels WHERE issue_id = ?1) +
            (SELECT COUNT(*) FROM issue_relations WHERE source_id = ?1 OR target_id = ?1),
            (SELECT COALESCE(SUM(length(CAST(l.name AS BLOB))), 0)
               FROM issue_labels il JOIN labels l ON l.id = il.label_id WHERE il.issue_id = ?1) +
            (SELECT COALESCE(SUM(length(CAST(p.identifier AS BLOB)) + 20), 0)
               FROM issue_relations ir
               JOIN issues other ON other.id = CASE WHEN ir.source_id = ?1 THEN ir.target_id ELSE ir.source_id END
               JOIN projects p ON p.id = other.project_id
              WHERE ir.source_id = ?1 OR ir.target_id = ?1) +
            (SELECT COALESCE(SUM(length(CAST(COALESCE(u.display_name, u.username) AS BLOB))), 0)
               FROM comments c JOIN users u ON u.id = c.user_id WHERE c.issue_id = ?1) +
            (SELECT COALESCE(length(CAST(m.name AS BLOB)), 0)
               FROM issues i LEFT JOIN modules m ON m.id = i.module_id WHERE i.id = ?1)",
        [issue_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    ensure_metadata_size(items, bytes)
}

fn ensure_page_metadata_size(conn: &Connection, page_id: i64) -> Result<(), LificError> {
    let (items, bytes): (i64, i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(length(CAST(l.name AS BLOB))), 0)
           FROM page_labels pl JOIN labels l ON l.id = pl.label_id
          WHERE pl.page_id = ?1",
        [page_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    ensure_metadata_size(items, bytes)
}

fn bounded_folders(conn: &Connection, project_id: i64) -> Result<Vec<Folder>, LificError> {
    let (items, bytes, oversized): (i64, i64, i64) = conn.query_row(
        "SELECT COUNT(*),
                COALESCE(SUM(length(CAST(name AS BLOB))), 0),
                EXISTS(SELECT 1 FROM folders WHERE project_id = ?1 AND length(CAST(name AS BLOB)) > ?2)
           FROM folders WHERE project_id = ?1",
        rusqlite::params![project_id, MAX_EXPORT_FILE_BYTES],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    ensure_metadata_size(items, bytes)?;
    if oversized != 0 {
        return Err(LificError::BadRequest(
            "export folder name exceeds export limit".into(),
        ));
    }
    queries::list_folders(conn, project_id)
}

fn ensure_metadata_size(items: i64, bytes: i64) -> Result<(), LificError> {
    if items > MAX_EXPORT_METADATA_ITEMS {
        return Err(LificError::BadRequest(format!(
            "export contains too many metadata entries ({items} > {MAX_EXPORT_METADATA_ITEMS})"
        )));
    }
    if bytes > MAX_EXPORT_TOTAL_BYTES as i64 {
        return Err(LificError::BadRequest(format!(
            "export metadata exceeds the {} byte total limit",
            MAX_EXPORT_TOTAL_BYTES
        )));
    }
    Ok(())
}

fn bounded_project(conn: &Connection, project_id: i64) -> Result<Project, LificError> {
    let oversized: i64 = conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM projects WHERE id = ?1 AND (
                 length(CAST(name AS BLOB)) > ?2 OR
                 length(CAST(identifier AS BLOB)) > ?2 OR
                 length(CAST(description AS BLOB)) > ?2 OR
                 length(CAST(COALESCE(emoji, '') AS BLOB)) > ?2
             )
         )",
        rusqlite::params![project_id, MAX_EXPORT_FILE_BYTES],
        |row| row.get(0),
    )?;
    if oversized != 0 {
        return Err(LificError::BadRequest(
            "project metadata exceeds export limit".into(),
        ));
    }
    queries::get_project(conn, project_id)
}

struct ExportBudget {
    files: usize,
    bytes: usize,
}

impl ExportBudget {
    fn new(root: &str) -> Result<Self, LificError> {
        ensure_text_size("export root", root)?;
        Ok(Self {
            files: 0,
            bytes: root.len(),
        })
    }

    fn add(&mut self, file: &ExportFile) -> Result<(), LificError> {
        self.files += 1;
        if self.files > MAX_EXPORT_FILES {
            return Err(LificError::BadRequest(format!(
                "export contains too many files ({} > {})",
                self.files, MAX_EXPORT_FILES
            )));
        }
        let bytes = file
            .path
            .len()
            .checked_add(file.content.len())
            .ok_or_else(|| LificError::BadRequest("export size overflow".into()))?;
        if bytes > MAX_EXPORT_FILE_BYTES {
            return Err(LificError::BadRequest(format!(
                "export file {} exceeds the {} byte limit",
                file.path, MAX_EXPORT_FILE_BYTES
            )));
        }
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or_else(|| LificError::BadRequest("export size overflow".into()))?;
        if self.bytes > MAX_EXPORT_TOTAL_BYTES {
            return Err(LificError::BadRequest(format!(
                "export exceeds the {} byte total limit",
                MAX_EXPORT_TOTAL_BYTES
            )));
        }
        Ok(())
    }
}

struct BundleBuilder {
    root: String,
    files: Vec<ExportFile>,
    budget: ExportBudget,
}

impl BundleBuilder {
    fn new(root: String) -> Result<Self, LificError> {
        let budget = ExportBudget::new(&root)?;
        Ok(Self {
            root,
            files: Vec::new(),
            budget,
        })
    }

    fn push(&mut self, path: String, content: String) -> Result<(), LificError> {
        let file = ExportFile { path, content };
        self.budget.add(&file)?;
        self.files.push(file);
        Ok(())
    }

    fn finish(self) -> ExportBundle {
        ExportBundle {
            root: self.root,
            files: self.files,
        }
    }
}

fn validate_bundle(bundle: &ExportBundle) -> Result<(), LificError> {
    let mut budget = ExportBudget::new(&bundle.root)?;
    for file in &bundle.files {
        budget.add(file)?;
    }
    Ok(())
}

fn bounded_issue_comments(
    conn: &Connection,
    issue_id: i64,
) -> Result<Vec<Comment>, LificError> {
    let parent = queries::comments::CommentParent::Issue(issue_id);
    ensure_comment_sizes(conn, issue_id)?;
    if queries::comments::count_comments(conn, parent, None)? > MAX_EXPORT_COMMENTS {
        return Err(LificError::BadRequest(format!(
            "issue has more than {MAX_EXPORT_COMMENTS} comments; export it in smaller slices"
        )));
    }
    queries::comments::list_comments_paginated(
        conn,
        parent,
        None,
        None,
        Some(MAX_EXPORT_COMMENTS),
        None,
    )
}

pub fn export_issue(conn: &Connection, identifier: &str) -> Result<ExportBundle, LificError> {
    let transaction = conn.unchecked_transaction()?;
    let bundle = export_issue_snapshot(&transaction, identifier)?;
    transaction.commit()?;
    Ok(bundle)
}

fn export_issue_snapshot(
    conn: &Connection,
    identifier: &str,
) -> Result<ExportBundle, LificError> {
    let issue_id = queries::resolve_identifier(conn, identifier)?;
    let oversized: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM issues WHERE id = ?1 AND (
             length(CAST(title AS BLOB)) > ?2 OR
             length(CAST(description AS BLOB)) > ?2 OR
             length(CAST(COALESCE(source, '') AS BLOB)) > ?2
         ))",
        rusqlite::params![issue_id, MAX_EXPORT_FILE_BYTES],
        |row| row.get(0),
    )?;
    if oversized != 0 {
        return Err(LificError::BadRequest("issue content exceeds export limit".into()));
    }
    ensure_issue_metadata_size(conn, issue_id)?;
    let project_id = conn.query_row(
        "SELECT project_id FROM issues WHERE id = ?1",
        [issue_id],
        |row| row.get(0),
    )?;
    let project = bounded_project(conn, project_id)?;
    let issue = queries::get_issue(conn, issue_id)?;
    ensure_text_size("issue title", &issue.title)?;
    ensure_text_size("issue description", &issue.description)?;
    let comments = bounded_issue_comments(conn, issue.id)?;
    let path = format!(
        "{}/issues/{}.md",
        project.identifier,
        slugged_issue_name(&issue)
    );
    let mut bundle = BundleBuilder::new(project.identifier.clone())?;
    bundle.push(
        path,
        render_issue_markdown(conn, &project, &issue, &comments)?,
    )?;
    Ok(bundle.finish())
}

pub fn export_page(conn: &Connection, identifier: &str) -> Result<ExportBundle, LificError> {
    let transaction = conn.unchecked_transaction()?;
    let bundle = export_page_snapshot(&transaction, identifier)?;
    transaction.commit()?;
    Ok(bundle)
}

fn export_page_snapshot(
    conn: &Connection,
    identifier: &str,
) -> Result<ExportBundle, LificError> {
    let page_id = queries::resolve_page_identifier(conn, identifier)?;
    let oversized: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pages WHERE id = ?1 AND (length(CAST(title AS BLOB)) > ?2 OR length(CAST(content AS BLOB)) > ?2))",
        rusqlite::params![page_id, MAX_EXPORT_FILE_BYTES],
        |row| row.get(0),
    )?;
    if oversized != 0 {
        return Err(LificError::BadRequest("page content exceeds export limit".into()));
    }
    ensure_page_metadata_size(conn, page_id)?;
    let project_id = conn.query_row(
        "SELECT project_id FROM pages WHERE id = ?1",
        [page_id],
        |row| row.get::<_, Option<i64>>(0),
    )?;
    let project = project_id
        .map(|project_id| bounded_project(conn, project_id))
        .transpose()?;
    let page = queries::get_page(conn, page_id)?;
    ensure_text_size("page title", &page.title)?;
    ensure_text_size("page content", &page.content)?;
    let root = project
        .as_ref()
        .map_or_else(|| "workspace".to_string(), |project| project.identifier.clone());
    let folders = project_id
        .map(|project_id| bounded_folders(conn, project_id))
        .transpose()?
        .unwrap_or_default();
    let path = build_page_path(&root, &page, &folders);
    let mut bundle = BundleBuilder::new(root)?;
    bundle.push(path, render_page_markdown(project.as_ref(), &page))?;
    Ok(bundle.finish())
}

pub fn export_project(conn: &Connection, identifier: &str) -> Result<ExportBundle, LificError> {
    let transaction = conn.unchecked_transaction()?;
    let bundle = export_project_snapshot(&transaction, identifier)?;
    transaction.commit()?;
    Ok(bundle)
}

fn ensure_project_preflight(
    conn: &Connection,
    project_id: i64,
    max_total_bytes: i64,
) -> Result<(), LificError> {
    let (
        issue_count,
        page_count,
        comment_count,
        issue_bytes,
        page_bytes,
        comment_bytes,
        metadata_items,
        metadata_bytes,
    ): (i64, i64, i64, i64, i64, i64, i64, i64) = conn.query_row(
        "SELECT
            (SELECT COUNT(*) FROM issues WHERE project_id = ?1),
            (SELECT COUNT(*) FROM pages WHERE project_id = ?1),
            (SELECT COUNT(*) FROM comments c JOIN issues i ON i.id = c.issue_id WHERE i.project_id = ?1),
            (SELECT COALESCE(SUM(length(CAST(title AS BLOB)) + length(CAST(description AS BLOB)) + length(CAST(COALESCE(source, '') AS BLOB))), 0) FROM issues WHERE project_id = ?1),
            (SELECT COALESCE(SUM(length(CAST(title AS BLOB)) + length(CAST(content AS BLOB))), 0) FROM pages WHERE project_id = ?1),
            (SELECT COALESCE(SUM(length(CAST(c.content AS BLOB))), 0) FROM comments c JOIN issues i ON i.id = c.issue_id WHERE i.project_id = ?1),
            (SELECT COUNT(*) FROM issue_labels il JOIN issues i ON i.id = il.issue_id WHERE i.project_id = ?1) +
            (SELECT COUNT(*) FROM page_labels pl JOIN pages p ON p.id = pl.page_id WHERE p.project_id = ?1) +
            (SELECT COUNT(*) FROM folders WHERE project_id = ?1) +
            2 * (SELECT COUNT(*) FROM issue_relations ir
                   JOIN issues source ON source.id = ir.source_id
                   JOIN issues target ON target.id = ir.target_id
                  WHERE source.project_id = ?1 OR target.project_id = ?1),
            (SELECT COALESCE(SUM(length(CAST(l.name AS BLOB))), 0)
               FROM issue_labels il JOIN issues i ON i.id = il.issue_id JOIN labels l ON l.id = il.label_id
              WHERE i.project_id = ?1) +
            (SELECT COALESCE(SUM(length(CAST(l.name AS BLOB))), 0)
               FROM page_labels pl JOIN pages page ON page.id = pl.page_id JOIN labels l ON l.id = pl.label_id
              WHERE page.project_id = ?1) +
            (SELECT COALESCE(SUM(length(CAST(m.name AS BLOB))), 0)
               FROM issues i JOIN modules m ON m.id = i.module_id WHERE i.project_id = ?1) +
            (SELECT COALESCE(SUM(length(CAST(name AS BLOB))), 0)
               FROM folders WHERE project_id = ?1) +
            (SELECT COALESCE(SUM(length(CAST(COALESCE(u.display_name, u.username) AS BLOB))), 0)
               FROM comments c JOIN issues i ON i.id = c.issue_id JOIN users u ON u.id = c.user_id
              WHERE i.project_id = ?1) +
            2 * (SELECT COALESCE(SUM(length(CAST(source_project.identifier AS BLOB)) +
                                     length(CAST(target_project.identifier AS BLOB)) + 40), 0)
                   FROM issue_relations ir
                   JOIN issues source ON source.id = ir.source_id
                   JOIN projects source_project ON source_project.id = source.project_id
                   JOIN issues target ON target.id = ir.target_id
                   JOIN projects target_project ON target_project.id = target.project_id
                  WHERE source.project_id = ?1 OR target.project_id = ?1)",
        [project_id],
        |row| {
            Ok((
                row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?,
                row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?,
            ))
        },
    )?;
    if issue_count + page_count > MAX_EXPORT_FILES as i64 {
        return Err(LificError::BadRequest(format!(
            "project export contains too many files ({} > {})",
            issue_count + page_count,
            MAX_EXPORT_FILES
        )));
    }
    if comment_count > MAX_EXPORT_COMMENTS {
        return Err(LificError::BadRequest(format!(
            "project export contains too many comments ({} > {})",
            comment_count, MAX_EXPORT_COMMENTS
        )));
    }
    ensure_metadata_size(metadata_items, metadata_bytes)?;
    let source_bytes = issue_bytes
        .checked_add(page_bytes)
        .and_then(|bytes| bytes.checked_add(comment_bytes))
        .and_then(|bytes| bytes.checked_add(metadata_bytes))
        .ok_or_else(|| LificError::BadRequest("export size overflow".into()))?;
    if source_bytes > max_total_bytes {
        return Err(LificError::BadRequest(format!(
            "project source exceeds the {} byte total export limit",
            max_total_bytes
        )));
    }
    let oversized: i64 = conn.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM issues WHERE project_id = ?1
             AND (length(CAST(title AS BLOB)) > ?2 OR
                  length(CAST(description AS BLOB)) > ?2 OR
                  length(CAST(COALESCE(source, '') AS BLOB)) > ?2)
           UNION ALL
           SELECT 1 FROM pages WHERE project_id = ?1
             AND (length(CAST(title AS BLOB)) > ?2 OR length(CAST(content AS BLOB)) > ?2)
           UNION ALL
           SELECT 1 FROM comments c JOIN issues i ON i.id = c.issue_id
             WHERE i.project_id = ?1 AND length(CAST(c.content AS BLOB)) > ?2
           UNION ALL
           SELECT 1 FROM folders WHERE project_id = ?1 AND length(CAST(name AS BLOB)) > ?2
         )",
        rusqlite::params![project_id, MAX_EXPORT_FILE_BYTES],
        |row| row.get(0),
    )?;
    if oversized != 0 {
        return Err(LificError::BadRequest("project content exceeds export limit".into()));
    }
    Ok(())
}

fn export_project_snapshot(
    conn: &Connection,
    identifier: &str,
) -> Result<ExportBundle, LificError> {
    let project_id = queries::resolve_project_identifier(conn, identifier)?;
    let project = bounded_project(conn, project_id)?;
    ensure_project_preflight(conn, project.id, MAX_EXPORT_TOTAL_BYTES as i64)?;
    let issue_ids = conn
        .prepare_cached("SELECT id FROM issues WHERE project_id = ?1 ORDER BY id")?
        .query_map([project.id], |row| row.get(0))?
        .collect::<Result<Vec<i64>, _>>()?;
    let page_ids = conn
        .prepare_cached("SELECT id FROM pages WHERE project_id = ?1 ORDER BY id")?
        .query_map([project.id], |row| row.get(0))?
        .collect::<Result<Vec<i64>, _>>()?;

    let mut bundle = BundleBuilder::new(project.identifier.clone())?;
    for issue_id in issue_ids {
        let issue = queries::get_issue(conn, issue_id)?;
        ensure_text_size("issue title", &issue.title)?;
        ensure_text_size("issue description", &issue.description)?;
        let comments = bounded_issue_comments(conn, issue.id)?;
        bundle.push(
            format!(
                "{}/issues/{}.md",
                project.identifier,
                slugged_issue_name(&issue)
            ),
            render_issue_markdown(conn, &project, &issue, &comments)?,
        )?;
    }
    let folders = queries::list_folders(conn, project.id)?;
    for page_id in page_ids {
        let page = queries::get_page(conn, page_id)?;
        ensure_text_size("page title", &page.title)?;
        ensure_text_size("page content", &page.content)?;
        bundle.push(
            build_page_path(&project.identifier, &page, &folders),
            render_page_markdown(Some(&project), &page),
        )?;
    }
    Ok(bundle.finish())
}

/// Write a bundle's files under `target_dir`, one file per
/// [`ExportFile::path`].
///
/// The paths are not trusted. A local SQL export builds them itself, but the
/// CLI's HTTP backend deserializes the very same bundle from a remote
/// server's JSON (LIF-341), so every path is checked here rather than at the
/// call sites: anything that could land outside `target_dir` (an absolute
/// path, a `..` segment, a platform prefix, a symlinked directory already
/// sitting in the output tree) is rejected instead of written.
pub fn write_bundle_to_directory(
    bundle: &ExportBundle,
    target_dir: &Path,
) -> Result<Vec<PathBuf>, LificError> {
    validate_bundle(bundle)?;
    let mut written = Vec::new();
    for file in &bundle.files {
        let full_path = prepare_output_path(target_dir, &file.path)?;
        std::fs::write(&full_path, &file.content).map_err(io_error)?;
        written.push(full_path);
    }
    Ok(written)
}

/// Unpack a project export archive into the tree
/// [`write_bundle_to_directory`] would have written (LIF-341).
///
/// Remote project exports stay a single ZIP on the wire, so the client is
/// what has to unpack them: the archive's entry names are exactly the
/// bundle's relative paths, so writing each entry under `target_dir` leaves
/// the same individual markdown files behind that a direct-SQL export does.
///
/// Entry names come off the network, so they are checked rather than
/// trusted. Anything that could climb out of `target_dir` (an absolute path,
/// a `..` segment, a Windows drive prefix, a symlink in the output tree) is
/// rejected instead of written, and the archive itself is capped so a hostile
/// server cannot fill the disk with a zip bomb.
pub fn unpack_zip_to_directory(
    archive: &[u8],
    target_dir: &Path,
) -> Result<Vec<PathBuf>, LificError> {
    unpack_zip_with_limits(archive, target_dir, &UnpackLimits::default())
}

/// How much of an archive we are willing to expand onto the caller's disk.
struct UnpackLimits {
    max_entries: usize,
    max_bytes: u64,
}

impl Default for UnpackLimits {
    fn default() -> Self {
        Self {
            max_entries: MAX_ARCHIVE_ENTRIES,
            max_bytes: MAX_ARCHIVE_BYTES,
        }
    }
}

/// A project export is a few thousand markdown files at the very outside.
const MAX_ARCHIVE_ENTRIES: usize = 10_000;

/// Total expanded bytes, not compressed bytes: the compressed size is what a
/// zip bomb makes small.
const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;

fn unpack_zip_with_limits(
    archive: &[u8],
    target_dir: &Path,
    limits: &UnpackLimits,
) -> Result<Vec<PathBuf>, LificError> {
    let mut zip = zip::ZipArchive::new(Cursor::new(archive)).map_err(zip_error)?;
    if zip.len() > limits.max_entries {
        return Err(LificError::BadRequest(format!(
            "export archive holds {} entries, more than the {} allowed",
            zip.len(),
            limits.max_entries
        )));
    }

    let mut written = Vec::new();
    let mut expanded: u64 = 0;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(zip_error)?;
        if entry.is_dir() {
            continue;
        }
        let full_path = prepare_output_path(target_dir, entry.name())?;

        // Read through a limited reader: `entry.size()` is the archive's own
        // claim about the entry, so it decides neither how much we allocate
        // nor how much we accept.
        let remaining = limits.max_bytes - expanded;
        let mut content = Vec::new();
        let read = entry
            .by_ref()
            .take(remaining.saturating_add(1))
            .read_to_end(&mut content)
            .map_err(io_error)? as u64;
        if read > remaining {
            return Err(LificError::BadRequest(format!(
                "export archive expands past the {} byte limit",
                limits.max_bytes
            )));
        }
        expanded += read;

        std::fs::write(&full_path, &content).map_err(io_error)?;
        written.push(full_path);
    }
    Ok(written)
}

/// Resolve an untrusted export path against `target_dir` and create the
/// directories leading to it, refusing anything that would write outside the
/// tree the user asked to export into.
///
/// Three checks, because the obvious one is not enough. [`contained_path`]
/// rejects the path lexically. Then every component that already exists is
/// tested for being a symlink, since a lexically contained path can still be
/// redirected by a link sitting in the output tree. Finally the created
/// parent is canonicalized and required to stay under the canonical
/// `target_dir`, which catches whatever the component walk did not.
fn prepare_output_path(target_dir: &Path, name: &str) -> Result<PathBuf, LificError> {
    let relative = contained_path(name)?;

    let mut current = target_dir.to_path_buf();
    for component in relative.components() {
        current.push(component);
        if let Ok(metadata) = std::fs::symlink_metadata(&current)
            && metadata.file_type().is_symlink()
        {
            return Err(LificError::BadRequest(format!(
                "export entry '{name}' would write through a symlink in the output directory"
            )));
        }
    }

    let full_path = target_dir.join(&relative);
    if let Some(parent) = full_path.parent() {
        std::fs::create_dir_all(parent).map_err(io_error)?;
        let canonical_root = std::fs::canonicalize(target_dir).map_err(io_error)?;
        let canonical_parent = std::fs::canonicalize(parent).map_err(io_error)?;
        if !canonical_parent.starts_with(&canonical_root) {
            return Err(LificError::BadRequest(format!(
                "export entry '{name}' would write outside the output directory"
            )));
        }
    }
    Ok(full_path)
}

/// Reduce an export path to a relative path that cannot escape the
/// directory it will be joined onto.
fn contained_path(name: &str) -> Result<PathBuf, LificError> {
    let mut contained = PathBuf::new();
    for component in Path::new(name).components() {
        match component {
            Component::Normal(segment) => contained.push(segment),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(LificError::BadRequest(format!(
                    "export entry '{name}' would write outside the output directory"
                )));
            }
        }
    }
    if contained.as_os_str().is_empty() {
        return Err(LificError::BadRequest(format!(
            "export entry '{name}' has no file name"
        )));
    }
    Ok(contained)
}

#[cfg(test)]
pub fn bundle_to_zip(bundle: &ExportBundle) -> Result<Vec<u8>, LificError> {
    validate_bundle(bundle)?;
    let mut cursor = Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(&mut cursor);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    for file in &bundle.files {
        zip.start_file(&file.path, options).map_err(zip_error)?;
        zip.write_all(file.content.as_bytes()).map_err(io_error)?;
    }
    zip.finish().map_err(zip_error)?;
    Ok(cursor.into_inner())
}
/// Write a validated bundle to a caller-owned temporary file.  Keeping the
/// archive on disk lets HTTP callers stream it instead of retaining another
/// complete compressed copy in the response body.
pub fn bundle_to_zip_file(bundle: &ExportBundle, path: &Path) -> Result<(), LificError> {
    validate_bundle(bundle)?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io_error)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for file in &bundle.files {
        zip.start_file(&file.path, options).map_err(zip_error)?;
        zip.write_all(file.content.as_bytes()).map_err(io_error)?;
    }
    zip.finish().map_err(zip_error)?;
    Ok(())
}

pub(crate) fn bundle_to_json_file(
    bundle: &ExportBundle,
    path: &Path,
) -> Result<(), LificError> {
    bundle_to_json_file_with_limit(bundle, path, MAX_EXPORT_JSON_BYTES)
}

fn bundle_to_json_file_with_limit(
    bundle: &ExportBundle,
    path: &Path,
    limit: usize,
) -> Result<(), LificError> {
    validate_bundle(bundle)?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io_error)?;
    serde_json::to_writer(LimitedWriter::new(file, limit), bundle).map_err(|error| {
        if error.io_error_kind() == Some(io::ErrorKind::FileTooLarge) {
            LificError::BadRequest(format!(
                "JSON export exceeds the {limit} byte encoded-output limit"
            ))
        } else {
            LificError::Internal(format!("serialize export JSON: {error}"))
        }
    })
}

struct LimitedWriter<W> {
    inner: W,
    remaining: usize,
}

impl<W> LimitedWriter<W> {
    fn new(inner: W, limit: usize) -> Self {
        Self {
            inner,
            remaining: limit,
        }
    }
}

impl<W: Write> Write for LimitedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.remaining {
            return Err(io::Error::from(io::ErrorKind::FileTooLarge));
        }
        let written = self.inner.write(bytes)?;
        self.remaining -= written;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn render_issue_markdown(
    conn: &Connection,
    project: &Project,
    issue: &Issue,
    comments: &[Comment],
) -> Result<String, LificError> {
    let module = issue
        .module_id
        .map(|id| queries::get_module_name(conn, id))
        .transpose()?;

    #[derive(Serialize)]
    struct IssueFrontmatter<'a> {
        identifier: &'a str,
        title: &'a str,
        project: &'a str,
        status: crate::db::models::Status,
        priority: crate::db::models::Priority,
        module: Option<String>,
        labels: &'a [String],
        blocks: &'a [String],
        blocked_by: &'a [String],
        relates_to: &'a [String],
        #[serde(skip_serializing_if = "<[String]>::is_empty")]
        duplicates: &'a [String],
        #[serde(skip_serializing_if = "<[String]>::is_empty")]
        duplicated_by: &'a [String],
        start_date: &'a Option<String>,
        target_date: &'a Option<String>,
        created_at: &'a str,
        updated_at: &'a str,
    }

    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(
        &serde_yaml::to_string(&IssueFrontmatter {
            identifier: &issue.identifier,
            title: &issue.title,
            project: &project.identifier,
            status: issue.status,
            priority: issue.priority,
            module,
            labels: &issue.labels,
            blocks: &issue.blocks,
            blocked_by: &issue.blocked_by,
            relates_to: &issue.relates_to,
            duplicates: &issue.duplicates,
            duplicated_by: &issue.duplicated_by,
            start_date: &issue.start_date,
            target_date: &issue.target_date,
            created_at: &issue.created_at,
            updated_at: &issue.updated_at,
        })
        .map_err(yaml_error)?,
    );
    out.push_str("---\n\n");
    out.push_str(&format!("# {}\n\n", issue.title));
    if !issue.description.trim().is_empty() {
        out.push_str(issue.description.trim_end());
        out.push('\n');
    }
    if !comments.is_empty() {
        out.push_str("\n## Comments\n\n");
        for comment in comments {
            out.push_str(&format!(
                "### {} ({})\n\n{}\n\n",
                comment.author_display_name,
                comment.created_at,
                comment.content.trim_end()
            ));
        }
    }
    Ok(out)
}

fn render_page_markdown(project: Option<&Project>, page: &Page) -> String {
    #[derive(Serialize)]
    struct PageFrontmatter<'a> {
        identifier: &'a str,
        title: &'a str,
        project: Option<&'a str>,
        created_at: &'a str,
        updated_at: &'a str,
    }

    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(
        &serde_yaml::to_string(&PageFrontmatter {
            identifier: &page.identifier,
            title: &page.title,
            project: project.map(|p| p.identifier.as_str()),
            created_at: &page.created_at,
            updated_at: &page.updated_at,
        })
        .expect("page frontmatter"),
    );
    out.push_str("---\n\n");
    out.push_str(&format!("# {}\n\n", page.title));
    if !page.content.trim().is_empty() {
        out.push_str(page.content.trim_end());
        out.push('\n');
    }
    out
}

fn build_page_path(root: &str, page: &Page, folders: &[Folder]) -> String {
    let mut parts = vec![root.to_string(), "pages".to_string()];
    if let Some(folder_id) = page.folder_id {
        parts.extend(folder_segments(folder_id, folders));
    }
    parts.push(format!(
        "{}.md",
        slugify(&format!("{}-{}", page.identifier, page.title))
    ));
    parts.join("/")
}

fn folder_segments(folder_id: i64, folders: &[Folder]) -> Vec<String> {
    let map: HashMap<i64, &Folder> = folders.iter().map(|folder| (folder.id, folder)).collect();
    let mut segments = Vec::new();
    let mut current = Some(folder_id);
    while let Some(id) = current {
        if let Some(folder) = map.get(&id) {
            segments.push(slugify(&folder.name));
            current = folder.parent_id;
        } else {
            break;
        }
    }
    segments.reverse();
    segments
}

fn slugged_issue_name(issue: &Issue) -> String {
    slugify(&format!("{}-{}", issue.identifier, issue.title))
}

fn slugify(input: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in input.chars() {
        let ch = ch.to_ascii_lowercase();
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn io_error(err: std::io::Error) -> LificError {
    LificError::Internal(format!("export io error: {err}"))
}

fn yaml_error(err: serde_yaml::Error) -> LificError {
    LificError::Internal(format!("export yaml error: {err}"))
}

fn zip_error(err: zip::result::ZipError) -> LificError {
    LificError::Internal(format!("export zip error: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{
        CreateFolder, CreateIssue, CreatePage, CreateProject, Priority, Status,
    };
    use crate::db::{open_memory, queries};

    #[test]
    fn project_export_writes_issue_and_nested_page_paths() {
        let db = open_memory().unwrap();
        let conn = db.write().unwrap();
        let project = queries::create_project(
            &conn,
            &CreateProject {
                name: "Export Test".into(),
                identifier: "EXP".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let issue = queries::create_issue(
            &conn,
            &CreateIssue {
                project_id: project.id,
                title: "Ship export".into(),
                description: "Need markdown output".into(),
                status: Status::Todo,
                priority: Priority::High,
                labels: vec!["feature".into()],
                ..Default::default()
            },
        )
        .unwrap();
        let user = queries::users::create_user(
            &conn,
            &crate::db::models::CreateUser {
                username: "tester".into(),
                email: "tester@example.com".into(),
                password: "password123".into(),
                display_name: Some("Tester".into()),
                is_admin: true,
                is_bot: false,
            },
        )
        .unwrap();
        queries::comments::create_comment(
            &conn,
            queries::comments::CommentParent::Issue(issue.id),
            user.id,
            "First exported comment",
        )
        .unwrap();
        let parent = queries::create_folder(
            &conn,
            &CreateFolder {
                project_id: project.id,
                parent_id: None,
                name: "Docs".into(),
            },
        )
        .unwrap();
        let child = queries::create_folder(
            &conn,
            &CreateFolder {
                project_id: project.id,
                parent_id: Some(parent.id),
                name: "Guides".into(),
            },
        )
        .unwrap();
        queries::create_page(
            &conn,
            &CreatePage {
                project_id: Some(project.id),
                folder_id: Some(child.id),
                title: "Getting Started".into(),
                content: "Welcome".into(),
                ..Default::default()
            },
        )
        .unwrap();

        let bundle = export_project(&conn, "EXP").unwrap();
        assert_eq!(bundle.root, "EXP");
        assert_eq!(bundle.files.len(), 2);
        assert!(bundle
            .files
            .iter()
            .any(|file| file.path.starts_with("EXP/issues/exp-1-ship-export")));
        assert!(bundle
            .files
            .iter()
            .any(|file| file.path == "EXP/pages/docs/guides/exp-doc-1-getting-started.md"));
        let issue_file = bundle
            .files
            .iter()
            .find(|file| file.path.contains("issues/"))
            .unwrap();
        assert!(issue_file.content.contains("identifier: EXP-1"));
        assert!(issue_file.content.contains("## Comments"));
        assert_eq!(issue_file.content.matches("First exported comment").count(), 1);
    }

    // LIF-136: duplicate relations must appear in exported frontmatter, both
    // the `duplicates` (source) and `duplicated_by` (target) directions.
    #[test]
    fn issue_export_includes_duplicate_relations() {
        let db = open_memory().unwrap();
        let conn = db.write().unwrap();
        let project = queries::create_project(
            &conn,
            &CreateProject {
                name: "Dup Test".into(),
                identifier: "DUP".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let mk = |title: &str| {
            queries::create_issue(
                &conn,
                &CreateIssue {
                    project_id: project.id,
                    title: title.into(),
                    status: Status::Todo,
                    ..Default::default()
                },
            )
            .unwrap()
        };
        let dup = mk("Duplicate");
        let canonical = mk("Canonical");
        queries::link_issues(&conn, dup.id, canonical.id, "duplicate").unwrap();

        // The single-issue export path populates relations via get_issue.
        let dup_bundle = export_issue(&conn, "DUP-1").unwrap();
        let dup_file = &dup_bundle.files[0];
        assert!(
            dup_file.content.contains("duplicates:") && dup_file.content.contains("DUP-2"),
            "source frontmatter should list duplicates: {}",
            dup_file.content
        );
        assert!(
            !dup_file.content.contains("duplicated_by:"),
            "source frontmatter should omit empty duplicated_by: {}",
            dup_file.content
        );

        let canonical_bundle = export_issue(&conn, "DUP-2").unwrap();
        let canonical_file = &canonical_bundle.files[0];
        assert!(
            canonical_file.content.contains("duplicated_by:")
                && canonical_file.content.contains("DUP-1"),
            "target frontmatter should list duplicated_by: {}",
            canonical_file.content
        );
    }

    /// LIF-341: unpacking the archive is how the CLI's HTTP backend lands a
    /// remote project export on disk, so it has to leave the same tree
    /// `write_bundle_to_directory` writes locally.
    #[test]
    fn unpacking_an_archive_matches_writing_the_bundle_directly() {
        let bundle = ExportBundle {
            root: "EXP".into(),
            files: vec![
                ExportFile {
                    path: "EXP/issues/exp-1-ship-export.md".into(),
                    content: "# Ship export\n".into(),
                },
                ExportFile {
                    path: "EXP/pages/docs/handbook/exp-doc-1-guide.md".into(),
                    content: "# Guide\n".into(),
                },
            ],
        };

        let direct_tmp = scratch_dir("bundle-direct");
        let unpacked_tmp = scratch_dir("bundle-unpacked");
        let direct_dir = direct_tmp.path().to_path_buf();
        let unpacked_dir = unpacked_tmp.path().to_path_buf();
        let direct = write_bundle_to_directory(&bundle, &direct_dir).unwrap();
        let unpacked =
            unpack_zip_to_directory(&bundle_to_zip(&bundle).unwrap(), &unpacked_dir).unwrap();

        // Same paths, in the same order, holding the same bytes.
        let relative = |paths: &[PathBuf], root: &Path| -> Vec<String> {
            paths
                .iter()
                .map(|path| {
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/")
                })
                .collect()
        };
        assert_eq!(
            relative(&unpacked, &unpacked_dir),
            relative(&direct, &direct_dir)
        );
        for (unpacked, direct) in unpacked.iter().zip(&direct) {
            assert_eq!(
                std::fs::read_to_string(unpacked).unwrap(),
                std::fs::read_to_string(direct).unwrap()
            );
        }
    }

    /// Entry names arrive over the network, so a hostile one must not be able
    /// to plant a file outside the directory the user asked to export into.
    #[test]
    fn refuses_archive_entries_that_climb_out_of_the_output_directory() {
        for escape in ["../escaped.md", "EXP/../../escaped.md", "/etc/escaped.md"] {
            let archive = bundle_to_zip(&ExportBundle {
                root: "EXP".into(),
                files: vec![ExportFile {
                    path: escape.into(),
                    content: "owned".into(),
                }],
            })
            .unwrap();

            let output = scratch_dir("bundle-escape");
            let error = unpack_zip_to_directory(&archive, output.path())
                .expect_err("'{escape}' should be rejected");
            assert!(
                error.to_string().contains("outside the output directory"),
                "unexpected error for '{escape}': {error}"
            );
        }
    }

    /// The CLI's HTTP backend hands `write_bundle_to_directory` a bundle it
    /// deserialized from a remote server, so the JSON path needs the same
    /// containment the archive path has: a hostile `path` must not plant a
    /// file next to the directory the user asked to export into.
    #[test]
    fn refuses_bundle_files_that_climb_out_of_the_output_directory() {
        for escape in ["../escaped.md", "EXP/../../escaped.md", "../../escaped.md"] {
            let root = scratch_dir("bundle-json-escape");
            let output = root.path().join("out");
            std::fs::create_dir_all(&output).unwrap();

            let error = write_bundle_to_directory(
                &ExportBundle {
                    root: "EXP".into(),
                    files: vec![ExportFile {
                        path: escape.into(),
                        content: "owned".into(),
                    }],
                },
                &output,
            )
            .expect_err("'{escape}' should be rejected");
            assert!(
                error.to_string().contains("outside the output directory"),
                "unexpected error for '{escape}': {error}"
            );
            assert!(
                !root.path().join("escaped.md").exists(),
                "'{escape}' wrote outside the output directory"
            );
            assert!(
                !root.path().parent().unwrap().join("escaped.md").exists(),
                "'{escape}' wrote outside the output directory"
            );
        }
    }

    /// An absolute path would ignore the output directory entirely, so it is
    /// rejected rather than silently rewritten.
    #[test]
    fn refuses_bundle_files_with_absolute_paths() {
        let output = scratch_dir("bundle-json-absolute");
        let error = write_bundle_to_directory(
            &ExportBundle {
                root: "EXP".into(),
                files: vec![ExportFile {
                    path: "/tmp/lific-absolute-escape.md".into(),
                    content: "owned".into(),
                }],
            },
            output.path(),
        )
        .expect_err("an absolute path should be rejected");
        assert!(
            error.to_string().contains("outside the output directory"),
            "unexpected error: {error}"
        );
        assert!(!Path::new("/tmp/lific-absolute-escape.md").exists());
    }

    /// Containment by string inspection alone is not enough: `EXP/evil.md` is
    /// a perfectly relative path, and still escapes if `EXP` is a symlink
    /// pointing somewhere else.
    #[cfg(unix)]
    #[test]
    fn refuses_bundle_files_that_write_through_a_symlink() {
        let root = scratch_dir("bundle-symlink");
        let output = root.path().join("out");
        let elsewhere = root.path().join("elsewhere");
        std::fs::create_dir_all(&output).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, output.join("EXP")).unwrap();

        let error = write_bundle_to_directory(
            &ExportBundle {
                root: "EXP".into(),
                files: vec![ExportFile {
                    path: "EXP/evil.md".into(),
                    content: "owned".into(),
                }],
            },
            &output,
        )
        .expect_err("a symlinked directory should be rejected");
        assert!(
            error.to_string().contains("symlink"),
            "unexpected error: {error}"
        );
        assert!(
            !elsewhere.join("evil.md").exists(),
            "the write followed the symlink out of the output directory"
        );
    }

    /// Same protection on the archive path, where the symlink can be planted
    /// by an earlier entry of the very same archive.
    #[cfg(unix)]
    #[test]
    fn refuses_archive_entries_that_write_through_a_symlink() {
        let root = scratch_dir("archive-symlink");
        let output = root.path().join("out");
        let elsewhere = root.path().join("elsewhere");
        std::fs::create_dir_all(&output).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, output.join("EXP")).unwrap();

        let archive = bundle_to_zip(&ExportBundle {
            root: "EXP".into(),
            files: vec![ExportFile {
                path: "EXP/evil.md".into(),
                content: "owned".into(),
            }],
        })
        .unwrap();

        let error = unpack_zip_to_directory(&archive, &output)
            .expect_err("a symlinked directory should be rejected");
        assert!(
            error.to_string().contains("symlink"),
            "unexpected error: {error}"
        );
        assert!(!elsewhere.join("evil.md").exists());
    }

    /// A server that answers an export with a million entries should not turn
    /// into a million files on the caller's disk.
    #[test]
    fn refuses_archives_with_too_many_entries() {
        let archive = bundle_to_zip(&ExportBundle {
            root: "EXP".into(),
            files: (0..4)
                .map(|index| ExportFile {
                    path: format!("EXP/issues/exp-{index}.md"),
                    content: "body".into(),
                })
                .collect(),
        })
        .unwrap();

        let output = scratch_dir("archive-entry-cap");
        let error = unpack_zip_with_limits(
            &archive,
            output.path(),
            &UnpackLimits {
                max_entries: 3,
                max_bytes: MAX_ARCHIVE_BYTES,
            },
        )
        .expect_err("an over-long archive should be rejected");
        assert!(
            error.to_string().contains("more than the 3 allowed"),
            "unexpected error: {error}"
        );
        assert!(!output.path().join("EXP").exists());
    }

    /// The compressed size says nothing about the expanded size, so the cap
    /// counts what actually lands on disk.
    #[test]
    fn refuses_archives_that_expand_past_the_size_limit() {
        let archive = bundle_to_zip(&ExportBundle {
            root: "EXP".into(),
            files: (0..3)
                .map(|index| ExportFile {
                    path: format!("EXP/issues/exp-{index}.md"),
                    content: "x".repeat(100),
                })
                .collect(),
        })
        .unwrap();

        let output = scratch_dir("archive-byte-cap");
        let error = unpack_zip_with_limits(
            &archive,
            output.path(),
            &UnpackLimits {
                max_entries: MAX_ARCHIVE_ENTRIES,
                max_bytes: 150,
            },
        )
        .expect_err("an over-large archive should be rejected");
        assert!(
            error.to_string().contains("past the 150 byte limit"),
            "unexpected error: {error}"
        );
        // The first entry fit under the cap; the second is what tripped it.
        assert!(output.path().join("EXP/issues/exp-0.md").exists());
        assert!(!output.path().join("EXP/issues/exp-2.md").exists());
    }

    /// Unique per call, so these tests can run beside each other. The guard
    /// removes the directory on Drop, unwinding included, so a failing
    /// assertion cannot leave scratch state behind.
    fn scratch_dir(label: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!("lific-{label}-"))
            .tempdir()
            .unwrap()
    }

    #[test]
    fn project_preflight_rejects_aggregate_source_bytes_before_materialization() {
        let db = open_memory().unwrap();
        let conn = db.write().unwrap();
        let project = queries::create_project(
            &conn,
            &CreateProject {
                name: "Source Bound".into(),
                identifier: "SRC".into(),
                ..Default::default()
            },
        )
        .unwrap();
        for title in ["First", "Second"] {
            queries::create_issue(
                &conn,
                &CreateIssue {
                    project_id: project.id,
                    title: title.into(),
                    description: "twenty source bytes!".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        }

        let error = ensure_project_preflight(&conn, project.id, 32).unwrap_err();
        assert!(error.to_string().contains("project source exceeds"));
    }

    #[test]
    fn project_export_rejects_too_many_comments_in_aggregate() {
        let db = open_memory().unwrap();
        let conn = db.write().unwrap();
        let project = queries::create_project(
            &conn,
            &CreateProject {
                name: "Comment Bound".into(),
                identifier: "CMT".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let user = queries::users::create_user(
            &conn,
            &crate::db::models::CreateUser {
                username: "commenter".into(),
                email: "commenter@example.com".into(),
                password: "password123".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap();
        let issues = ["First", "Second"].map(|title| {
            queries::create_issue(
                &conn,
                &CreateIssue {
                    project_id: project.id,
                    title: title.into(),
                    ..Default::default()
                },
            )
            .unwrap()
        });
        for issue in issues {
            for _ in 0..251 {
                queries::comments::create_comment(
                    &conn,
                    queries::comments::CommentParent::Issue(issue.id),
                    user.id,
                    "bounded",
                )
                .unwrap();
            }
        }

        let error = export_project(&conn, "CMT").unwrap_err();
        assert!(error.to_string().contains("too many comments"));
    }

    #[test]
    fn project_export_rejects_too_many_folders_before_loading_them() {
        let db = open_memory().unwrap();
        let conn = db.write().unwrap();
        let project = queries::create_project(
            &conn,
            &CreateProject {
                name: "Folder Bound".into(),
                identifier: "FLD".into(),
                ..Default::default()
            },
        )
        .unwrap();
        conn.execute(
            "WITH RECURSIVE sequence(value) AS (
                 SELECT 1 UNION ALL SELECT value + 1 FROM sequence WHERE value <= ?2
             )
             INSERT INTO folders (project_id, name)
             SELECT ?1, 'folder-' || value FROM sequence",
            rusqlite::params![project.id, MAX_EXPORT_METADATA_ITEMS],
        )
        .unwrap();

        let error = export_project(&conn, "FLD").unwrap_err();
        assert!(error.to_string().contains("too many metadata entries"));
    }

    #[test]
    fn export_file_limit_includes_its_path() {
        let bundle = ExportBundle {
            root: "TEST".into(),
            files: vec![ExportFile {
                path: "TEST/issues/large.md".into(),
                content: "x".repeat(MAX_EXPORT_FILE_BYTES),
            }],
        };

        let error = validate_bundle(&bundle).unwrap_err();
        assert!(error.to_string().contains("exceeds"));
    }

    #[test]
    fn json_export_bounds_the_encoded_output() {
        let bundle = ExportBundle {
            root: "TEST".into(),
            files: vec![ExportFile {
                path: "TEST/page.md".into(),
                content: "\0".repeat(16),
            }],
        };
        let output = scratch_dir("json-byte-cap");
        let error = bundle_to_json_file_with_limit(
            &bundle,
            &output.path().join("export.json"),
            64,
        )
        .unwrap_err();

        assert!(error.to_string().contains("encoded-output limit"));
    }

    #[test]
    fn json_and_zip_accept_the_same_control_heavy_bundle() {
        let bundle = ExportBundle {
            root: "TEST".into(),
            files: vec![ExportFile {
                path: "TEST/page.md".into(),
                content: "\0".repeat(1_024),
            }],
        };
        let output = scratch_dir("json-policy");

        bundle_to_zip(&bundle).unwrap();
        bundle_to_json_file(&bundle, &output.path().join("export.json")).unwrap();
    }

    #[test]
    fn project_export_bounds_metadata_before_materializing_it() {
        let db = open_memory().unwrap();
        let conn = db.write().unwrap();
        let project = queries::create_project(
            &conn,
            &CreateProject {
                name: "Metadata Bound".into(),
                identifier: "META".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let issue = queries::create_issue(
            &conn,
            &CreateIssue {
                project_id: project.id,
                title: "Bound labels".into(),
                ..Default::default()
            },
        )
        .unwrap();
        conn.execute(
            "WITH RECURSIVE sequence(value) AS (
                 SELECT 1 UNION ALL SELECT value + 1 FROM sequence WHERE value <= ?2
             )
             INSERT INTO labels (project_id, name)
             SELECT ?1, 'label-' || value FROM sequence",
            rusqlite::params![project.id, MAX_EXPORT_METADATA_ITEMS],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO issue_labels (issue_id, label_id)
             SELECT ?1, id FROM labels WHERE project_id = ?2",
            rusqlite::params![issue.id, project.id],
        )
        .unwrap();

        let error = export_project(&conn, "META").unwrap_err();
        assert!(error.to_string().contains("too many metadata entries"));
    }
}
