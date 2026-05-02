use std::collections::HashMap;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::Serialize;

use crate::db::{
    models::Comment, models::Folder, models::Issue, models::Page, models::Project, queries,
};
use crate::error::LificError;

#[derive(Debug, Clone, Serialize)]
pub struct ExportFile {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportBundle {
    pub root: String,
    pub files: Vec<ExportFile>,
}

pub fn export_issue(conn: &Connection, identifier: &str) -> Result<ExportBundle, LificError> {
    let issue_id = queries::resolve_identifier(conn, identifier)?;
    let issue = queries::get_issue(conn, issue_id)?;
    let project = queries::get_project(conn, issue.project_id)?;
    let comments = queries::comments::list_comments(conn, issue.id)?;
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
            status: None,
            priority: None,
            module_id: None,
            label: None,
            workable: None,
            limit: Some(10_000),
            offset: None,
        },
    )?;
    let pages = queries::list_pages(conn, Some(project.id), None)?;

    let mut files = Vec::new();
    for issue in issues {
        let comments = queries::comments::list_comments(conn, issue.id)?;
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

pub fn write_bundle_to_directory(
    bundle: &ExportBundle,
    target_dir: &Path,
) -> Result<Vec<PathBuf>, LificError> {
    let mut written = Vec::new();
    for file in &bundle.files {
        let full_path = target_dir.join(&file.path);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent).map_err(io_error)?;
        }
        std::fs::write(&full_path, &file.content).map_err(io_error)?;
        written.push(full_path);
    }
    Ok(written)
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
        status: &'a str,
        priority: &'a str,
        module: Option<String>,
        labels: &'a [String],
        blocks: &'a [String],
        blocked_by: &'a [String],
        relates_to: &'a [String],
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
            status: &issue.status,
            priority: &issue.priority,
            module,
            labels: &issue.labels,
            blocks: &issue.blocks,
            blocked_by: &issue.blocked_by,
            relates_to: &issue.relates_to,
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
    use crate::db::models::{CreateFolder, CreateIssue, CreatePage, CreateProject};
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
                description: String::new(),
                emoji: None,
                lead_user_id: None,
            },
        )
        .unwrap();
        let issue = queries::create_issue(
            &conn,
            &CreateIssue {
                project_id: project.id,
                title: "Ship export".into(),
                description: "Need markdown output".into(),
                status: "todo".into(),
                priority: "high".into(),
                module_id: None,
                start_date: None,
                target_date: None,
                labels: vec!["feature".into()],
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
        queries::comments::create_comment(&conn, issue.id, user.id, "First exported comment")
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
}
