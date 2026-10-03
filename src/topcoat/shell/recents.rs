//! The active private project's server-backed sidebar recent resources.

use topcoat::{
    context::Cx,
    view::{BoxView, ViewExt, view},
};

use super::{
    Layout, Page, ParsedRoute,
    context::{ShellContext, ShellPrincipal},
};

pub(crate) const STYLESHEET: &str = include_str!("assets/recents.css");
pub(crate) const STYLESHEET_PATH: &str = "/__topcoat-recents.css";
pub(crate) const SCRIPT: &str = include_str!("assets/recents.js");
pub(crate) const SCRIPT_PATH: &str = "/__topcoat-recents.js";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Section {
    Issues,
    Modules,
    Pages,
    Plans,
}

impl Section {
    pub(crate) fn for_route(route: &ParsedRoute<'_>) -> Option<Self> {
        match (route.layout, route.page) {
            (Layout::Private, Page::Issues | Page::IssueNew | Page::IssueDetail(_)) => {
                Some(Self::Issues)
            }
            (Layout::Private, Page::Modules | Page::ModuleDetail(_)) => Some(Self::Modules),
            (Layout::Private, Page::Pages | Page::Record(_)) => Some(Self::Pages),
            (Layout::Private, Page::Plans | Page::PlanDetail(_)) => Some(Self::Plans),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Issues => "issues",
            Self::Modules => "modules",
            Self::Pages => "pages",
            Self::Plans => "plans",
        }
    }
}

/// Compose once in desktop navigation. The browser controller supplies rows
/// through the authenticated REST transport after the catalog resolves.
pub(crate) fn panel<'a>(cx: &'a Cx, context: &ShellContext<'_>) -> BoxView<'a> {
    match (
        context.principal(),
        Section::for_route(context.route()),
        context.route().project,
    ) {
        (ShellPrincipal::Private { active_project, .. }, Some(section), Some(identifier)) => {
            let section = section.as_str();
            let identifier = identifier.to_owned();
            let project_id = active_project
                .as_ref()
                .map(|project| project.project.id.to_string());
            let heading = format!("Recent {section}");
            view! { cx =>
                <section class="tc-recents" data-topcoat-recents=""
                    data-project-identifier=(identifier) data-project-id=(project_id)
                    data-recent-section=(section) aria-label="Recent resources" hidden="hidden">
                    <button type="button" class="tc-recents__heading" data-recents-toggle=""
                        aria-expanded="false" aria-controls="tc-sidebar-recents-list">
                        (heading)
                    </button>
                    <div id="tc-sidebar-recents-list" data-recents-content="" hidden="hidden" aria-busy="false">
                        <p class="tc-recents__status" data-recents-status="" role="status" aria-live="polite"></p>
                        <ul class="tc-recents__list" data-recents-list=""></ul>
                        <p class="tc-recents__error" data-recents-error="" role="status" aria-live="polite" hidden="hidden"></p>
                    </div>
                </section>
            }
            .boxed()
        }
        _ => view! { cx => }.boxed(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recents_follow_private_resource_families_without_board_or_public_panels() {
        for (path, expected) in [
            ("/LIF/issues", Some(Section::Issues)),
            ("/LIF/issues/new", Some(Section::Issues)),
            ("/LIF/issues/LIF-7", Some(Section::Issues)),
            ("/LIF/modules/8", Some(Section::Modules)),
            ("/LIF/pages/9", Some(Section::Pages)),
            ("/LIF/plans/10", Some(Section::Plans)),
            ("/LIF/board", None),
            ("/LIF/graph", None),
            ("/public/LIF/issues", None),
            ("/public/LIF/pages/9", None),
            ("/login", None),
        ] {
            assert_eq!(
                Section::for_route(&ParsedRoute::parse(path)),
                expected,
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn recents_private_panel_exposes_disclosure_and_live_loading_semantics() {
        let cx = Cx::default();
        let route = ParsedRoute::parse("/LIF/issues/LIF-7");
        let context = ShellContext::private(&route, None, None, None).unwrap();
        let html = panel(&cx, &context).single().await.unwrap().render(&cx);
        assert!(html.contains("Recent issues"));
        assert!(html.contains("data-project-identifier=\"LIF\""));
        assert!(html.contains("aria-expanded=\"false\""));
        assert!(html.contains("aria-controls=\"tc-sidebar-recents-list\""));
        assert!(html.contains("aria-busy=\"false\""));
        assert!(html.contains("aria-live=\"polite\""));
    }

    #[tokio::test]
    async fn recents_public_shell_has_no_private_panel_or_links() {
        let cx = Cx::default();
        let route = ParsedRoute::parse("/public/LIF/issues/LIF-7");
        let context = ShellContext::public(&route).unwrap();
        let html = panel(&cx, &context).single().await.unwrap().render(&cx);
        assert!(html.is_empty(), "{html}");
    }
}
