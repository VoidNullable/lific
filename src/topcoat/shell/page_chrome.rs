//! Reusable breadcrumbs, sub-tabs, and route-scoped trailing actions.

use topcoat::{
    context::Cx,
    view::{BoxView, ViewExt, view},
};

use super::{ParsedRoute, context::PageMetadata};

pub(crate) const STYLESHEET: &str = include_str!("assets/page-chrome.css");
pub(crate) const SCRIPT: &str = include_str!("assets/page-chrome.js");
pub(crate) const SCRIPT_PATH: &str = "/__topcoat-page-chrome.js";
pub(crate) const STYLESHEET_PATH: &str = "/__topcoat-page-chrome.css";

#[derive(Debug, Clone, PartialEq, Eq)]
struct BreadcrumbView {
    label: String,
    href: Option<String>,
    current: bool,
    copy_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActionView {
    id: String,
    label: String,
    shortcut: Option<String>,
    href: Option<String>,
    history: Option<&'static str>,
    command: Option<&'static str>,
}

pub(crate) fn page_chrome<'a>(
    cx: &'a Cx,
    route: &ParsedRoute<'_>,
    view_id: &str,
    project_id: Option<i64>,
    metadata: &PageMetadata,
) -> BoxView<'a> {
    let breadcrumb_count = metadata.breadcrumbs.len();
    let breadcrumbs = metadata
        .breadcrumbs
        .iter()
        .enumerate()
        .map(|(index, crumb)| BreadcrumbView {
            label: crumb.label.clone(),
            href: crumb
                .href
                .as_deref()
                .filter(|href| breadcrumb_is_in_scope(route, href))
                .map(str::to_owned),
            current: index + 1 == breadcrumb_count,
            copy_text: copyable_identifier(route, &crumb.label).map(str::to_owned),
        })
        .collect::<Vec<_>>();
    let tabs = metadata.tabs.clone();
    let actions = metadata
        .trailing_actions
        .iter()
        .map(|action| {
            let (href, history, command) = match &action.behavior {
                super::context::PageActionBehavior::Navigate(request) => (
                    Some(request.href.clone()),
                    Some(match request.history {
                        super::context::HistoryMode::Push => "push",
                        super::context::HistoryMode::Replace => "replace",
                    }),
                    None,
                ),
                super::context::PageActionBehavior::Dispatch(command) => {
                    (None, None, Some(command_name(command)))
                }
            };
            ActionView {
                id: action.id.clone(),
                label: action.label.clone(),
                shortcut: action.shortcut.clone(),
                href,
                history,
                command,
            }
        })
        .collect::<Vec<_>>();
    let project_id = project_id.map(|id| id.to_string());
    let view_id = view_id.to_owned();

    view! { cx =>
        <section class="tc-page-chrome" data-page-view=(view_id.as_str())>
            <div class="tc-page-chrome__row">
                <nav class="tc-page-chrome__breadcrumb" aria-label="Breadcrumb">
                    <ol>
                        for crumb in breadcrumbs {
                            <li class="tc-page-chrome__crumb">
                                if let Some(href) = crumb.href.as_deref() {
                                    <a href=(href) title=(crumb.label.as_str())>(crumb.label.as_str())</a>
                                } else {
                                    <span title=(crumb.label.as_str())
                                        aria-current=(crumb.current.then_some("page"))>
                                        (crumb.label.as_str())
                                    </span>
                                }
                                if let Some(value) = crumb.copy_text.as_deref() {
                                    <button type="button" class="tc-page-chrome__copy"
                                        data-copy-identifier=(value) aria-label="Copy identifier"
                                        title="Copy identifier">"Copy"</button>
                                }
                            </li>
                        }
                    </ol>
                </nav>
                <div class="tc-page-chrome__actions" data-page-actions="">
                    for action in actions {
                        if let Some(href) = action.href.as_deref() {
                            <a class="tc-page-chrome__action" href=(href)
                                data-page-action-id=(action.id.as_str())
                                data-history-mode=(action.history.unwrap_or("push"))>
                                (action.label.as_str())
                                if let Some(shortcut) = action.shortcut.as_deref() {
                                    <kbd>(shortcut)</kbd>
                                }
                            </a>
                        } else {
                            <button type="button" class="tc-page-chrome__action"
                                data-page-action-id=(action.id.as_str())
                                data-page-action-command=(action.command.unwrap_or(""))>
                                (action.label.as_str())
                                if let Some(shortcut) = action.shortcut.as_deref() {
                                    <kbd>(shortcut)</kbd>
                                }
                            </button>
                        }
                    }
                </div>
            </div>
            if !tabs.is_empty() {
                <nav class="tc-page-chrome__tabs" role="tablist" aria-label="Page sections"
                    data-subtabs="" data-view=(view_id.as_str()) data-project-id=(project_id.as_deref())>
                    for tab in tabs {
                        <button type="button" role="tab" class="tc-page-chrome__tab"
                            data-subtab-id=(tab.id.as_str()) aria-selected=(tab.selected)
                            tabindex=(if tab.selected { "0" } else { "-1" })>
                            <span>(tab.label.as_str())</span>
                            if let Some(count) = tab.count {
                                <span class="tc-page-chrome__count" aria-label=(format!("{count} items"))>
                                    (count)
                                </span>
                            }
                        </button>
                    }
                </nav>
            }
        </section>
    }
    .boxed()
}

fn breadcrumb_is_in_scope(route: &ParsedRoute<'_>, href: &str) -> bool {
    if route.layout != super::Layout::Public {
        return true;
    }
    let Some(project) = route.project else {
        return false;
    };
    let path = href.strip_prefix('#').unwrap_or(href);
    let prefix = format!("/public/{project}");
    path == prefix
        || path.strip_prefix(&prefix).is_some_and(|suffix| {
            suffix.starts_with('/') || suffix.starts_with('?') || suffix.starts_with('#')
        })
}

fn copyable_identifier<'a>(route: &ParsedRoute<'a>, label: &'a str) -> Option<&'a str> {
    route
        .project
        .filter(|project| project.eq_ignore_ascii_case(label))
        .or_else(|| route.page.resource().filter(|resource| *resource == label))
}

fn command_name(command: &super::context::PageActionCommand) -> &'static str {
    match command {
        super::context::PageActionCommand::SetIssueStatus { .. } => "set-issue-status",
        super::context::PageActionCommand::SetIssuePriority { .. } => "set-issue-priority",
        super::context::PageActionCommand::OpenPicker { .. } => "open-picker",
    }
}

fn route_family(path: &str) -> &'static str {
    let path = path.strip_prefix("/public").unwrap_or(path);
    match path.split('/').nth(2) {
        Some("issues" | "board") => "issues",
        Some("pages") => "pages",
        Some("modules") => "modules",
        Some("plans") => "plans",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_breadcrumbs_cannot_escape_the_selected_project_scope() {
        let route = ParsedRoute::parse("/public/LIF/issues/LIF-42");
        assert!(breadcrumb_is_in_scope(&route, "/public/LIF/issues"));
        assert!(breadcrumb_is_in_scope(
            &route,
            "#/public/LIF/issues?status=open"
        ));
        assert!(!breadcrumb_is_in_scope(&route, "/LIF/issues"));
        assert!(!breadcrumb_is_in_scope(&route, "/public/OTHER/issues"));
    }

    #[test]
    fn issue_and_board_routes_share_one_transition_family() {
        assert_eq!(route_family("/LIF/issues"), route_family("/LIF/board"));
        assert_ne!(route_family("/LIF/pages"), route_family("/LIF/board"));
    }

    #[test]
    fn project_and_resource_identifiers_remain_copyable() {
        let route = ParsedRoute::parse("/LIF/issues/LIF-42");
        assert_eq!(copyable_identifier(&route, "LIF"), Some("LIF"));
        assert_eq!(copyable_identifier(&route, "LIF-42"), Some("LIF-42"));
        assert_eq!(copyable_identifier(&route, "Issues"), None);
    }
}
