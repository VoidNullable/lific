//! Issue list and board screens share the same browser read model and controls.
//!
//! Mount with [`screen`] after the session and sync bridges. Mutations use the
//! existing JSON endpoints; private project rows come from the shared replica.

use topcoat::{
    context::Cx,
    view::{BoxView, ViewExt, view},
};

pub(crate) const SCRIPT_PATH: &str = "/__topcoat-issue-list.js";
pub(crate) const SCRIPT: &str = include_str!("assets/issue-list.js");
pub(crate) const STYLESHEET_PATH: &str = "/__topcoat-issue-list.css";
pub(crate) const STYLESHEET: &str = include_str!("assets/issue-list.css");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layout {
    List,
    Board,
}

impl Layout {
    fn name(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Board => "board",
        }
    }
}

/// A missing project identifier renders the workspace's permission-filtered set.
pub(crate) fn screen<'a>(
    cx: &'a Cx,
    project_identifier: Option<&str>,
    layout: Layout,
) -> BoxView<'a> {
    let identifier = project_identifier.unwrap_or_default().to_owned();
    let title = match (project_identifier, layout) {
        (Some(project), Layout::List) => format!("{project} issues"),
        (Some(project), Layout::Board) => format!("{project} board"),
        (None, Layout::List) => "Workspace issues".to_owned(),
        (None, Layout::Board) => "Workspace board".to_owned(),
    };
    view! { cx =>
        <section class="tc-issues" data-topcoat-issue-list=""
            data-project-identifier=(identifier) data-layout=(layout.name())
            aria-label=(title.clone()) aria-busy="true">
            <h1>(title)</h1>
            <div class="tc-issues__controls" data-issues-controls=""></div>
            <div class="tc-issues__feedback" data-issues-feedback="">
                <p role="status" aria-live="polite">"Loading issues…"</p>
            </div>
            <div class="tc-issues__content" data-issues-content=""></div>
            <aside class="tc-issues__bulk" data-issues-bulk="" aria-label="Selected issue actions" hidden="hidden"></aside>
            <dialog class="tc-issues__peek" data-issues-peek="" aria-label="Issue preview"></dialog>
        </section>
    }.boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn issue_list_screen_escapes_identifiers_and_announces_cold_loading() {
        let cx = Cx::default();
        let html = screen(&cx, Some("ENG\" <script>"), Layout::List)
            .single()
            .await
            .unwrap()
            .render(&cx);
        assert!(html.contains("&quot;"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script> issues</h1>"));
        assert!(html.contains("aria-busy=\"true\""));
        assert!(html.contains("role=\"status\""));
        assert!(html.contains("Loading issues"));
    }

    #[tokio::test]
    async fn issue_list_workspace_board_has_no_project_assumption_or_mutation_markup() {
        let cx = Cx::default();
        let html = screen(&cx, None, Layout::Board)
            .single()
            .await
            .unwrap()
            .render(&cx);
        assert!(html.contains("Workspace board"));
        assert!(html.contains("data-project-identifier=\"\""));
        assert!(html.contains("data-layout=\"board\""));
        assert!(!html.contains("data-create-issue"));
        assert!(!html.contains("data-select"));
        assert!(html.contains("aria-label=\"Issue preview\""));
    }
}
