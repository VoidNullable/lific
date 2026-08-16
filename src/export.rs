use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
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

pub fn export_issue(conn: &Connection, identifier: &str) -> Result<ExportBundle, LificError> {
    let issue_id = queries::resolve_identifier(conn, identifier)?;
    let issue = queries::get_issue(conn, issue_id)?;
    let project = queries::get_project(conn, issue.project_id)?;
    let comments = queries::comments::list_comments(
        conn,
        queries::comments::CommentParent::Issue(issue.id),
        None,
        None,
    )?;
    let path = format!(
        "{}/issues/{}.md",
        project.identifier,
        slugged_issue_name(&issue)
    );
    Ok(ExportBundle {
        root: project.identifier.clone(),
        files: vec![ExportFile {
            path,
            content: render_issue_markdown(conn, &project, &issue, &comments)?,
        }],
    })
}

pub fn export_page(conn: &Connection, identifier: &str) -> Result<ExportBundle, LificError> {
    let page_id = queries::resolve_page_identifier(conn, identifier)?;
    let page = queries::get_page(conn, page_id)?;
    let (project, root) = match page.project_id {
        Some(project_id) => {
            let project = queries::get_project(conn, project_id)?;
            (Some(project.clone()), project.identifier)
        }
        None => (None, "workspace".to_string()),
    };
    let folders = match page.project_id {
        Some(project_id) => queries::list_folders(conn, project_id)?,
        None => Vec::new(),
    };
    let path = build_page_path(&root, &page, &folders);
    Ok(ExportBundle {
        root,
        files: vec![ExportFile {
            path,
            content: render_page_markdown(project.as_ref(), &page),
        }],
    })
}

pub fn export_project(conn: &Connection, identifier: &str) -> Result<ExportBundle, LificError> {
    let project_id = queries::resolve_project_identifier(conn, identifier)?;
    let project = queries::get_project(conn, project_id)?;
    let folders = queries::list_folders(conn, project.id)?;
    let issues = queries::list_issues(
        conn,
        &crate::db::models::ListIssuesQuery {
            project_id: Some(project.id),
            limit: Some(10_000),
            ..Default::default()
        },
    )?;
    let pages = queries::list_pages(conn, Some(project.id), None, None, None, None, None, None, None)?;

    let mut files = Vec::new();
    for issue in issues {
        let comments = queries::comments::list_comments(
            conn,
            queries::comments::CommentParent::Issue(issue.id),
            None,
            None,
        )?;
        files.push(ExportFile {
            path: format!(
                "{}/issues/{}.md",
                project.identifier,
                slugged_issue_name(&issue)
            ),
            content: render_issue_markdown(conn, &project, &issue, &comments)?,
        });
    }
    for page in pages {
        files.push(ExportFile {
            path: build_page_path(&project.identifier, &page, &folders),
            content: render_page_markdown(Some(&project), &page),
        });
    }

    Ok(ExportBundle {
        root: project.identifier.clone(),
        files,
    })
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

pub fn bundle_to_zip(bundle: &ExportBundle) -> Result<Vec<u8>, LificError> {
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
}
