//! Mobile shell presentation. Catalog and identity arrive through LIF-223's
//! immutable context; route navigation is emitted to the shell adapter.

use topcoat::{
    context::Cx,
    view::{BoxView, ViewExt, view},
};

use super::{
    Page,
    context::{ProjectCatalogSnapshot, ShellContext, ShellPrincipal},
};

pub(crate) const SCRIPT: &str = include_str!("assets/mobile.js");
pub(crate) const SCRIPT_PATH: &str = "/__topcoat-mobile.js";
pub(crate) const STYLESHEET: &str = include_str!("assets/mobile.css");
pub(crate) const STYLESHEET_PATH: &str = "/__topcoat-mobile.css";

fn catalog_json(catalog: Option<&ProjectCatalogSnapshot>) -> String {
    let Some(catalog) = catalog else {
        return "{\"generation\":0,\"projects\":[],\"groups\":[]}".to_owned();
    };
    serde_json::json!({
        "generation": catalog.generation,
        "projects": catalog.projects.iter().map(|project| serde_json::json!({
            "id": project.id, "identifier": project.identifier,
            "name": project.name, "emoji": project.emoji,
        })).collect::<Vec<_>>(),
        "groups": catalog.groups.iter().map(|group| serde_json::json!({
            "id": group.id, "name": group.name,
            "sort_order": group.sort_order, "project_ids": group.project_ids.as_ref(),
        })).collect::<Vec<_>>(),
    })
    .to_string()
}

/// Compose this once in the shell header. Its fixed panel remains outside
/// the document's inert background; the browser adapter owns modal state.
/// `lificMobileNavigation.navigateTo` emits a bubbling `lific:navigate`
/// event with the frozen `{href, history: 'push' | 'replace'}` payload after
/// unwinding drawer entries. Mobile never mutates application route history.
pub(crate) fn mobile_navigation<'a>(cx: &'a Cx, context: &ShellContext<'_>) -> BoxView<'a> {
    let (public_project, catalog, account_name) = match context.principal() {
        ShellPrincipal::Auth => return view! { cx => <span hidden="hidden"></span> }.boxed(),
        ShellPrincipal::Private { catalog, user, .. } => (
            None,
            catalog_json(catalog.as_deref()),
            user.map(|user| {
                if user.display_name.is_empty() {
                    user.username.clone()
                } else {
                    user.display_name.clone()
                }
            }),
        ),
        ShellPrincipal::Public { project } => {
            (Some((*project).to_owned()), catalog_json(None), None)
        }
    };
    let private = public_project.is_none();
    let active_project = context.route().project.map(str::to_owned);
    let active_page = match context.route().page {
        Page::Overview => "overview",
        Page::Issues | Page::Board | Page::IssueNew | Page::IssueDetail(_) => "issues",
        Page::Pages | Page::Record(_) => "pages",
        Page::Modules | Page::ModuleDetail(_) => "modules",
        Page::Plans | Page::PlanDetail(_) => "plans",
        Page::Graph => "graph",
        Page::Files => "files",
        Page::Activity => "activity",
        Page::Insights => "insights",
        _ => "",
    };
    view! { cx =>
        <button type="button" class="tc-mobile-trigger" data-mobile-open=""
            aria-label="Open navigation" aria-haspopup="dialog"
            aria-controls="tc-mobile-navigation" aria-expanded="false">"Navigation"</button>
        <div class="tc-mobile" id="tc-mobile-navigation" data-mobile-navigation=""
            data-mobile-catalog=(catalog) data-mobile-public-project=(public_project)
            data-mobile-active-project=(active_project) data-mobile-active-page=(active_page)
            role="dialog" aria-label="Navigation" aria-modal="true" aria-hidden="true"
            inert="inert" tabindex="-1" data-open="false" data-level="root">
            <div class="tc-mobile__pane tc-mobile__root" data-mobile-root="" aria-hidden="true" inert="inert">
                <header class="tc-mobile__header">
                    <span class="tc-mobile__brand">"Lific"</span>
                    <button type="button" data-mobile-close="" aria-label="Close navigation">"Close"</button>
                </header>
                <nav class="tc-mobile__scroll" aria-label="Projects">
                    if private {
                        <a href="/" data-mobile-destination="">"Home"</a>
                    }
                    <h2>"Projects"</h2>
                    <div data-mobile-project-list=""></div>
                    <p class="tc-mobile__hint" data-mobile-empty="" hidden="hidden">"No projects available."</p>
                    if private {
                        <a href="/projects/new" data-mobile-destination="">"New project"</a>
                        <a href="/projects/import" data-mobile-destination="">"Import project"</a>
                    }
                </nav>
                <footer class="tc-mobile__footer">
                    if private {
                        <a href="/settings" data-mobile-destination="">(account_name.unwrap_or_else(|| "Settings".to_owned()))</a>
                    } else {
                        <span class="tc-mobile__hint">"Public project · Read only"</span>
                    }
                </footer>
            </div>
            <div class="tc-mobile__pane tc-mobile__project" data-mobile-project="" aria-hidden="true" inert="inert">
                <header class="tc-mobile__header">
                    <button type="button" data-mobile-back="">"Projects"</button>
                    <button type="button" data-mobile-close="" aria-label="Close navigation">"Close"</button>
                </header>
                <div class="tc-mobile__project-heading">
                    <h2 data-mobile-project-name="">"Project"</h2>
                    <p class="tc-mobile__hint" data-mobile-project-identifier=""></p>
                </div>
                <nav class="tc-mobile__scroll" data-mobile-destinations="" aria-label="Project destinations"></nav>
                <p class="tc-mobile__unavailable" data-mobile-unavailable="" hidden="hidden">"This project is no longer available."</p>
            </div>
        </div>
    }.boxed()
}

#[cfg(test)]
mod tests {
    use super::super::ParsedRoute;
    use super::*;

    #[tokio::test]
    async fn mobile_public_markup_has_no_private_catalog_or_account_routes() {
        let cx = Cx::default();
        let route = ParsedRoute::parse("/public/LIF/issues");
        let context = ShellContext::public(&route).unwrap();
        let html = mobile_navigation(&cx, &context)
            .single()
            .await
            .unwrap()
            .render(&cx);
        assert!(html.contains("data-mobile-public-project=\"LIF\""));
        assert!(html.contains("role=\"dialog\""));
        assert!(html.contains("aria-controls=\"tc-mobile-navigation\""));
        assert!(html.contains("aria-hidden=\"true\" inert=\"inert\""));
        for href in ["/settings", "/projects/new", "/projects/import"] {
            assert!(!html.contains(href), "{href}");
        }
    }

    #[tokio::test]
    async fn mobile_auth_context_does_not_render_navigation() {
        let cx = Cx::default();
        let route = ParsedRoute::parse("/login");
        let context = ShellContext::auth(&route).unwrap();
        let html = mobile_navigation(&cx, &context)
            .single()
            .await
            .unwrap()
            .render(&cx);
        assert!(!html.contains("data-mobile-navigation"));
    }
}
