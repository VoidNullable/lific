//! Route selection and reusable application chrome for the Topcoat migration.
//! This module renders named placeholders; domain screens can replace the
//! content slot without changing the public/private navigation boundary.

use topcoat::{
    context::Cx,
    view::{BoxView, ViewExt, view},
};

#[path = "topcoat_shell/context.rs"]
pub(super) mod context;

pub(crate) const STYLESHEET: &str = include_str!("assets/topcoat-shell.css");
pub(crate) const STYLESHEET_PATH: &str = "/__topcoat-shell.css";
pub(crate) const ROUTE_SCRIPT: &str = include_str!("assets/topcoat-shell.js");
pub(crate) const ROUTE_SCRIPT_PATH: &str = "/__topcoat-shell.js";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layout {
    Auth,
    Private,
    Public,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Page<'a> {
    Login,
    Signup,
    Home,
    Settings,
    InstanceSettings,
    ProjectNew,
    ProjectImport,
    Overview,
    Issues,
    Board,
    Graph,
    IssueNew,
    IssueDetail(&'a str),
    Pages,
    Record(&'a str),
    Files,
    Modules,
    ModuleDetail(&'a str),
    Plans,
    PlanDetail(&'a str),
    Activity,
    Insights,
    NotFound,
}

impl<'a> Page<'a> {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Login => "Log in",
            Self::Signup => "Sign up",
            Self::Home => "Home",
            Self::Settings => "Settings",
            Self::InstanceSettings => "Instance settings",
            Self::ProjectNew => "New project",
            Self::ProjectImport => "Import project",
            Self::Overview => "Overview",
            Self::Issues => "Issues",
            Self::Board => "Board",
            Self::Graph => "Graph",
            Self::IssueNew => "New issue",
            Self::IssueDetail(_) => "Issue detail",
            Self::Pages => "Pages",
            Self::Record(_) => "Page detail",
            Self::Files => "Files",
            Self::Modules => "Modules",
            Self::ModuleDetail(_) => "Module detail",
            Self::Plans => "Plans",
            Self::PlanDetail(_) => "Plan detail",
            Self::Activity => "Activity",
            Self::Insights => "Insights",
            Self::NotFound => "Page not found",
        }
    }

    pub(crate) fn resource(self) -> Option<&'a str> {
        match self {
            Self::IssueDetail(id)
            | Self::Record(id)
            | Self::ModuleDetail(id)
            | Self::PlanDetail(id) => Some(id),
            _ => None,
        }
    }

    fn public(self) -> bool {
        matches!(
            self,
            Self::Issues | Self::Board | Self::IssueDetail(_) | Self::Pages | Self::Record(_)
        )
    }

    fn public_navigation(self) -> bool {
        matches!(
            self,
            Self::Issues | Self::IssueDetail(_) | Self::Pages | Self::Record(_)
        )
    }

    fn navigation_page(self) -> Self {
        match self {
            Self::Board | Self::IssueNew | Self::IssueDetail(_) => Self::Issues,
            Self::Record(_) => Self::Pages,
            Self::ModuleDetail(_) => Self::Modules,
            Self::PlanDetail(_) => Self::Plans,
            _ => self,
        }
    }
}

/// The query and fragment are retained verbatim for later domain handlers.
/// Layout selection describes presentation, never authorization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedRoute<'a> {
    pub(crate) layout: Layout,
    pub(crate) page: Page<'a>,
    pub(crate) project: Option<&'a str>,
    pub(crate) query: &'a str,
    pub(crate) fragment: &'a str,
    /// Canonical destination for the two existing short public link forms.
    pub(crate) redirect: Option<String>,
}

impl<'a> ParsedRoute<'a> {
    /// Accepts direct paths and legacy browser hash routes. HTTP handlers
    /// receive only path/query; the browser owns restoration of hash routes.
    pub(crate) fn parse(input: &'a str) -> Self {
        let input = if let Some((_, hash)) = input.split_once('#')
            && hash.starts_with('/')
        {
            hash
        } else {
            input
        };
        let (input, fragment) = input.split_once('#').unwrap_or((input, ""));
        let (path, query) = input.split_once('?').unwrap_or((input, ""));
        let mut route = Self {
            layout: Layout::Private,
            page: Page::NotFound,
            project: None,
            query,
            fragment,
            redirect: None,
        };

        route.page = match path {
            "/login" => {
                route.layout = Layout::Auth;
                Page::Login
            }
            "/signup" => {
                route.layout = Layout::Auth;
                Page::Signup
            }
            "/" => Page::Home,
            "/settings" => Page::Settings,
            "/settings/instance" => Page::InstanceSettings,
            "/projects/new" => Page::ProjectNew,
            "/projects/import" => Page::ProjectImport,
            _ => return route.parse_project(path),
        };
        route
    }

    fn parse_project(mut self, path: &'a str) -> Self {
        let Some(path) = path.strip_prefix('/') else {
            return self;
        };
        let (public, path) = match path.strip_prefix("public/") {
            Some(path) => (true, path),
            None => (false, path),
        };
        let (project, rest) = path.split_once('/').unwrap_or((path, ""));
        if !project_identifier(project) {
            return self;
        }
        self.project = Some(project);
        if public {
            self.layout = Layout::Public;
            // Preserve published project and legacy issue deep links.
            let destination = if rest.is_empty() {
                Some(format!("/public/{project}/issues"))
            } else if issue_identifier(rest) {
                Some(format!("/public/{project}/issues/{rest}"))
            } else {
                None
            };
            if let Some(mut destination) = destination {
                if !self.query.is_empty() {
                    destination.push('?');
                    destination.push_str(self.query);
                }
                if !self.fragment.is_empty() {
                    destination.push('#');
                    destination.push_str(self.fragment);
                }
                self.redirect = Some(destination);
                return self;
            }
        }

        let (section, target) = rest.split_once('/').unwrap_or((rest, ""));
        let section = section.to_ascii_lowercase();
        self.page = match (section.as_str(), target) {
            ("overview" | "settings", "") if !rest.ends_with('/') => Page::Overview,
            ("issues", "") if !rest.ends_with('/') => Page::Issues,
            ("board", "") if !rest.ends_with('/') => Page::Board,
            ("graph", "") if !rest.ends_with('/') => Page::Graph,
            ("issues", value) if value.eq_ignore_ascii_case("new") => Page::IssueNew,
            ("issues", id) if issue_identifier(id) => Page::IssueDetail(id),
            ("pages", "") if !rest.ends_with('/') => Page::Pages,
            ("pages", id) if digits(id) => Page::Record(id),
            ("files", "") if !rest.ends_with('/') => Page::Files,
            ("modules", "") if !rest.ends_with('/') => Page::Modules,
            ("modules", id) if digits(id) => Page::ModuleDetail(id),
            ("plans", "") if !rest.ends_with('/') => Page::Plans,
            ("plans", id) if digits(id) => Page::PlanDetail(id),
            ("activity", "") if !rest.ends_with('/') => Page::Activity,
            ("insights", "") if !rest.ends_with('/') => Page::Insights,
            _ => Page::NotFound,
        };
        if public && !self.page.public() {
            self.page = Page::NotFound;
        }
        self
    }

    fn project_href(&self, destination: &str) -> String {
        let project = self.project.unwrap_or_default();
        if self.layout == Layout::Public {
            format!("/public/{project}/{destination}")
        } else {
            format!("/{project}/{destination}")
        }
    }
}

fn project_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn digits(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn issue_identifier(value: &str) -> bool {
    value
        .rsplit_once('-')
        .is_some_and(|(project, number)| project_identifier(project) && digits(number))
}

const PROJECT_DESTINATIONS: &[(Page<'static>, &str)] = &[
    (Page::Overview, "overview"),
    (Page::Issues, "issues"),
    (Page::Board, "board"),
    (Page::Graph, "graph"),
    (Page::Modules, "modules"),
    (Page::Pages, "pages"),
    (Page::Files, "files"),
    (Page::Plans, "plans"),
    (Page::Activity, "activity"),
    (Page::Insights, "insights"),
];

struct NavigationLink {
    href: String,
    label: &'static str,
    active: bool,
}

fn navigation<'a>(cx: &'a Cx, route: &ParsedRoute<'_>, label: &'static str) -> BoxView<'a> {
    let private = route.layout == Layout::Private;
    let mut links = Vec::new();
    if let Some(project) = route.project {
        for &(page, slug) in PROJECT_DESTINATIONS {
            if private || page.public_navigation() {
                links.push(NavigationLink {
                    href: route.project_href(slug),
                    label: page.title(),
                    active: route.page.navigation_page() == page,
                });
            }
        }
        let project = project.to_owned();
        view! { cx =>
            <nav class="tc-shell__navigation" aria-label=(label)>
                if private {
                    <a href="/">"Home"</a>
                    <a href="/projects/new">"New project"</a>
                    <a href="/projects/import">"Import project"</a>
                }
                <h2 class="tc-shell__project">(project)</h2>
                for link in links {
                    <a href=(link.href) aria-current=(link.active.then_some("page"))>(link.label)</a>
                }
                if private {
                    <a href="/settings">"Settings"</a>
                    <a href="/settings/instance">"Instance settings"</a>
                }
            </nav>
        }
        .boxed()
    } else {
        if private {
            links.extend([
                NavigationLink {
                    href: "/".to_owned(),
                    label: "Home",
                    active: route.page == Page::Home,
                },
                NavigationLink {
                    href: "/projects/new".to_owned(),
                    label: "New project",
                    active: route.page == Page::ProjectNew,
                },
                NavigationLink {
                    href: "/projects/import".to_owned(),
                    label: "Import project",
                    active: route.page == Page::ProjectImport,
                },
            ]);
        }
        view! { cx =>
            <nav class="tc-shell__navigation" aria-label=(label)>
                if private {
                    for link in links {
                        <a href=(link.href) aria-current=(link.active.then_some("page"))>(link.label)</a>
                    }
                    <a href="/settings">"Settings"</a>
                    <a href="/settings/instance">"Instance settings"</a>
                    <p class="tc-shell__hint">"Choose a project from Home."</p>
                }
            </nav>
        }
        .boxed()
    }
}

/// Reusable chrome around an independently owned route screen.
/// Public navigation never links to private project or creation routes.
pub(crate) fn shell<'a>(cx: &'a Cx, route: &ParsedRoute<'_>, content: BoxView<'a>) -> BoxView<'a> {
    let is_public = route.layout == Layout::Public;
    let is_auth = route.layout == Layout::Auth;
    let layout_name = match route.layout {
        Layout::Auth => "auth",
        Layout::Private => "private",
        Layout::Public => "public",
    };
    let brand_href = if is_public {
        route.project_href("issues")
    } else {
        "/".to_owned()
    };
    let auth_link = route.page == Page::Login;
    let signup_link = route.page == Page::Signup;
    let mobile_navigation = navigation(cx, route, "Mobile navigation");
    let desktop_navigation = navigation(cx, route, "Desktop navigation");
    view! { cx =>
        <div class="tc-shell" data-layout=(layout_name)>
            <a class="tc-shell__skip" href="#main-content">"Skip to content"</a>
            <header class="tc-shell__header">
                <a class="tc-shell__brand" href=(brand_href)>"Lific"</a>
                if is_public {
                    <span class="tc-shell__hint">"Public project · Read only"</span>
                }
                if is_auth {
                    <nav class="tc-shell__auth-links" aria-label="Account">
                        <a href="/login" aria-current=(auth_link.then_some("page"))>"Log in"</a>
                        <a href="/signup" aria-current=(signup_link.then_some("page"))>"Sign up"</a>
                    </nav>
                } else {
                    <button type="button" class="tc-shell__sidebar-toggle" data-sidebar-toggle=""
                        aria-label="Collapse sidebar" aria-expanded="true" aria-controls="tc-sidebar">
                        "Collapse sidebar"
                    </button>
                    <details class="tc-shell__mobile">
                        <summary>"Navigation"</summary>
                        (mobile_navigation)
                    </details>
                }
            </header>
            <div class="tc-shell__body">
                if !is_auth {
                    <span class="tc-shell__sidebar-probe" data-sidebar-probe="" aria-hidden="true"></span>
                    <aside class="tc-shell__desktop" id="tc-sidebar">
                        (desktop_navigation)
                        <div class="tc-shell__sidebar-resize" data-sidebar-resize="" role="separator"
                            tabindex="0" aria-label="Resize sidebar" aria-orientation="vertical"
                            aria-controls="tc-sidebar"
                            aria-valuemin="180" aria-valuemax="400" aria-valuenow="230"
                            title="Use Left and Right arrows to resize. Double click to reset."></div>
                    </aside>
                }
                <main class="tc-shell__main" id="main-content" tabindex="-1">(content)</main>
            </div>
        </div>
    }
    .boxed()
}

/// First slice: identifies the destination without pretending a domain
/// screen has been migrated. Later ticket owners supply their own content.
pub(crate) fn placeholder<'a>(cx: &'a Cx, route: &ParsedRoute<'_>) -> BoxView<'a> {
    let title = route.page.title();
    let resource = route.page.resource().map(str::to_owned);
    let not_found = route.page == Page::NotFound;
    view! { cx =>
        <section class="tc-shell__placeholder" aria-labelledby="route-heading">
            <h1 id="route-heading">(title)</h1>
            if let Some(id) = resource {
                <p>(id)</p>
            }
            if not_found {
                <p>"This address does not match an available page."</p>
            } else {
                <p>"This page is not available yet."</p>
            }
        </section>
    }
    .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_route_parity_preserves_each_private_family() {
        let cases = [
            ("/login", Layout::Auth, Page::Login),
            ("/signup", Layout::Auth, Page::Signup),
            ("/", Layout::Private, Page::Home),
            ("/settings", Layout::Private, Page::Settings),
            (
                "/settings/instance",
                Layout::Private,
                Page::InstanceSettings,
            ),
            ("/projects/new", Layout::Private, Page::ProjectNew),
            ("/projects/import", Layout::Private, Page::ProjectImport),
            ("/LIF/overview", Layout::Private, Page::Overview),
            ("/LIF/settings", Layout::Private, Page::Overview),
            ("/LIF/issues", Layout::Private, Page::Issues),
            ("/LIF/board", Layout::Private, Page::Board),
            ("/LIF/graph", Layout::Private, Page::Graph),
            ("/LIF/issues/new", Layout::Private, Page::IssueNew),
            (
                "/LIF/issues/LIF-205",
                Layout::Private,
                Page::IssueDetail("LIF-205"),
            ),
            ("/LIF/pages", Layout::Private, Page::Pages),
            ("/LIF/pages/7", Layout::Private, Page::Record("7")),
            ("/LIF/files", Layout::Private, Page::Files),
            ("/LIF/modules", Layout::Private, Page::Modules),
            ("/LIF/modules/8", Layout::Private, Page::ModuleDetail("8")),
            ("/LIF/plans", Layout::Private, Page::Plans),
            ("/LIF/plans/9", Layout::Private, Page::PlanDetail("9")),
            ("/LIF/activity", Layout::Private, Page::Activity),
            ("/LIF/insights", Layout::Private, Page::Insights),
        ];
        for (path, layout, page) in cases {
            let route = ParsedRoute::parse(path);
            assert_eq!((route.layout, route.page), (layout, page), "{path}");
            assert!(route.redirect.is_none(), "{path}");
        }
        assert_eq!(
            ParsedRoute::parse("/lif/ISSUES/Lif-1").page,
            Page::IssueDetail("Lif-1")
        );
    }

    #[test]
    fn shell_public_routes_limit_destinations_and_keep_aliases() {
        for path in [
            "/public/LIF/issues",
            "/public/LIF/board",
            "/public/LIF/issues/LIF-42",
            "/public/LIF/pages",
            "/public/LIF/pages/7",
        ] {
            let route = ParsedRoute::parse(path);
            assert_eq!(route.layout, Layout::Public, "{path}");
            assert!(route.page.public(), "{path}");
        }
        for suffix in [
            "overview",
            "settings",
            "graph",
            "files",
            "modules",
            "plans",
            "activity",
            "insights",
            "issues/new",
        ] {
            let path = format!("/public/LIF/{suffix}");
            let route = ParsedRoute::parse(&path);
            assert_eq!(route.layout, Layout::Public, "{path}");
            assert_eq!(route.page, Page::NotFound, "{path}");
        }
        for path in ["/public/LIF", "/public/LIF/"] {
            assert_eq!(
                ParsedRoute::parse(path).redirect.as_deref(),
                Some("/public/LIF/issues")
            );
        }
        assert_eq!(
            ParsedRoute::parse("/public/LIF/LIF-42?view=all#comment-123")
                .redirect
                .as_deref(),
            Some("/public/LIF/issues/LIF-42?view=all#comment-123")
        );
    }

    #[test]
    fn shell_deep_links_keep_query_fragment_and_reject_malformed_paths() {
        let route = ParsedRoute::parse("/#/LIF/issues/new?module=7&status=todo#comment-123");
        assert_eq!(route.page, Page::IssueNew);
        assert_eq!(route.project, Some("LIF"));
        assert_eq!(route.query, "module=7&status=todo");
        assert_eq!(route.fragment, "comment-123");
        for path in [
            "/LIF/issues/",
            "/LIF/issues/LIF-no",
            "/LIF/pages/-1",
            "/LIF/pages/7/extra",
            "/1LIF/issues",
            "/LIF//issues",
            "/unknown",
        ] {
            assert_eq!(ParsedRoute::parse(path).page, Page::NotFound, "{path}");
        }
    }

    #[tokio::test]
    async fn shell_navigation_has_keyboard_semantics_and_active_parent() {
        let cx = Cx::default();
        let route = ParsedRoute::parse("/LIF/issues/LIF-205");
        let html = shell(&cx, &route, placeholder(&cx, &route))
            .single()
            .await
            .unwrap()
            .render(&cx);
        assert!(html.contains("href=\"#main-content\""));
        assert!(html.contains("id=\"main-content\" tabindex=\"-1\""));
        assert!(html.contains("<summary>Navigation</summary>"));
        assert!(html.contains("aria-label=\"Desktop navigation\""));
        assert!(html.contains("aria-label=\"Mobile navigation\""));
        assert!(html.contains("href=\"/LIF/issues\" aria-current=\"page\""));
        assert!(html.contains("Issue detail"));
        assert!(html.contains("LIF-205"));
    }

    #[tokio::test]
    async fn shell_public_and_auth_chrome_exclude_private_navigation() {
        let cx = Cx::default();
        let route = ParsedRoute::parse("/public/LIF/pages/7");
        let html = shell(&cx, &route, placeholder(&cx, &route))
            .single()
            .await
            .unwrap()
            .render(&cx);
        assert!(html.contains("href=\"/public/LIF/pages\" aria-current=\"page\""));
        assert!(html.contains("Read only"));
        for private in [
            "href=\"/settings",
            "href=\"/projects/",
            "href=\"/LIF/",
            "/public/LIF/modules",
            "/public/LIF/files",
            "/public/LIF/overview",
        ] {
            assert!(!html.contains(private), "{private}");
        }
        let route = ParsedRoute::parse("/login");
        let html = shell(&cx, &route, placeholder(&cx, &route))
            .single()
            .await
            .unwrap()
            .render(&cx);
        assert!(html.contains("aria-label=\"Account\""));
        assert!(!html.contains("Desktop navigation"));
        assert!(!html.contains("Mobile navigation"));
    }

    #[tokio::test]
    async fn shell_public_board_uses_issue_navigation_entry() {
        let cx = Cx::default();
        let route = ParsedRoute::parse("/public/LIF/board");
        assert_eq!(route.layout, Layout::Public);
        assert_eq!(route.page, Page::Board);
        let html = shell(&cx, &route, placeholder(&cx, &route))
            .single()
            .await
            .unwrap()
            .render(&cx);
        assert!(html.contains("href=\"/public/LIF/issues\" aria-current=\"page\""));
        assert!(!html.contains("href=\"/public/LIF/board\""));
    }

    #[tokio::test]
    async fn shell_sidebar_resize_and_collapse_are_keyboard_accessible() {
        let cx = Cx::default();
        for path in ["/LIF/issues", "/public/LIF/issues"] {
            let route = ParsedRoute::parse(path);
            let html = shell(&cx, &route, placeholder(&cx, &route))
                .single()
                .await
                .unwrap()
                .render(&cx);
            assert!(html.contains("aria-label=\"Collapse sidebar\""), "{path}");
            assert!(html.contains("aria-controls=\"tc-sidebar\""), "{path}");
            assert!(html.contains("id=\"tc-sidebar\""), "{path}");
            assert!(html.contains("role=\"separator\""), "{path}");
            assert!(html.contains("aria-orientation=\"vertical\""), "{path}");
            assert!(html.contains("aria-label=\"Resize sidebar\""), "{path}");
            assert!(html.contains("tabindex=\"0\""), "{path}");
        }
        let route = ParsedRoute::parse("/login");
        let html = shell(&cx, &route, placeholder(&cx, &route))
            .single()
            .await
            .unwrap()
            .render(&cx);
        assert!(!html.contains("data-sidebar-toggle"));
        assert!(!html.contains("data-sidebar-resize"));
    }
}
