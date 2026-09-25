//! LIF-475: a project export over MCP returns the markdown itself.
//!
//! The bundle's file paths only mean something to the CLI that writes them to
//! disk; an agent cannot open them. So a project export hands back the
//! documents, one issue or page each (every one opens with frontmatter naming
//! its identifier), a page of documents at a time with the totals and the
//! offset to continue from.

use std::fmt::Write as _;

use crate::db::queries;
use crate::export::ExportBundle;

pub(super) const DEFAULT_DOCUMENTS: i64 = 20;
pub(super) const MAX_DOCUMENTS: i64 = 100;

/// A page also stops early once this much markdown is in it, so a limit of
/// 100 long documents cannot flood the agent's context. The first document
/// is always included, however long, or a large one could never be read.
const PAGE_BUDGET_BYTES: usize = 256 * 1024;

pub(super) fn render_project_page(
    bundle: &ExportBundle,
    offset: Option<i64>,
    limit: Option<i64>,
) -> String {
    let (limit, offset) = queries::page_with(limit, offset, DEFAULT_DOCUMENTS, MAX_DOCUMENTS);
    let total = bundle.files.len();
    let issue_prefix = format!("{}/issues/", bundle.root);
    let issues = bundle
        .files
        .iter()
        .filter(|file| file.path.starts_with(&issue_prefix))
        .count();
    let pages = total - issues;
    let start = usize::try_from(offset).map_or(total, |offset| offset.min(total));
    let limit = usize::try_from(limit).unwrap_or(1);

    let mut documents = String::new();
    let mut shown = 0;
    let mut budget_limited = false;
    for file in bundle.files.iter().skip(start).take(limit) {
        if shown > 0 && documents.len() + 1 + file.content.len() > PAGE_BUDGET_BYTES {
            budget_limited = true;
            break;
        }
        if shown > 0 {
            documents.push('\n');
        }
        documents.push_str(&file.content);
        shown += 1;
    }

    let root = &bundle.root;
    let mut out = format!(
        "Project {root} export: {issues} issue(s) and {pages} page(s), {total} document(s)."
    );
    if shown == 0 {
        if total > 0 {
            let _ = write!(out, " Nothing at offset={offset}.");
        }
        return out;
    }
    let _ = write!(
        out,
        " Documents {}-{} follow.\n\n{documents}",
        start + 1,
        start + shown
    );
    let remaining = total - start - shown;
    if remaining > 0 {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        let reason = if budget_limited {
            " (this page stopped at the response size budget)"
        } else {
            ""
        };
        let _ = write!(
            out,
            "\n... {remaining} more document(s){reason}; call export with identifier=\"{root}\" and offset={}",
            start + shown
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::ExportFile;

    fn bundle(sizes: &[usize]) -> ExportBundle {
        ExportBundle {
            root: "BIG".into(),
            files: sizes
                .iter()
                .enumerate()
                .map(|(i, size)| ExportFile {
                    path: format!("BIG/issues/big-{i}.md"),
                    content: format!("doc{i}:{}\n", "x".repeat(*size)),
                })
                .collect(),
        }
    }

    #[test]
    fn a_page_stops_at_the_size_budget_but_always_shows_one_document() {
        let rendered = render_project_page(&bundle(&[200_000, 200_000, 10]), None, None);
        assert!(rendered.contains("doc0:"), "{}", &rendered[..200]);
        assert!(!rendered.contains("doc1:"));
        assert!(
            rendered.ends_with(
                "2 more document(s) (this page stopped at the response size budget); \
                 call export with identifier=\"BIG\" and offset=1"
            ),
            "{}",
            &rendered[rendered.len() - 200..]
        );

        let oversized = render_project_page(&bundle(&[400_000]), None, None);
        assert!(oversized.contains("doc0:"));
        assert!(!oversized.contains("more document(s)"));
    }

    #[test]
    fn an_offset_past_the_end_says_so() {
        let rendered = render_project_page(&bundle(&[1, 1]), Some(5), None);
        assert_eq!(
            rendered,
            "Project BIG export: 2 issue(s) and 0 page(s), 2 document(s). Nothing at offset=5."
        );
    }
}
