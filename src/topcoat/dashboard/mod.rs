//! Private Home/My Work and project overview. Browser requests use the shared
//! session boundary and existing REST endpoints; project administration is a
//! separate screen.

use topcoat::{
    context::Cx,
    view::{BoxView, ViewExt, view},
};

pub(crate) const SCRIPT_PATH: &str = "/__topcoat-dashboard.js";
pub(crate) const SCRIPT: &str = include_str!("assets/dashboard.js");
pub(crate) const STYLESHEET_PATH: &str = "/__topcoat-dashboard.css";
pub(crate) const STYLESHEET: &str = include_str!("assets/dashboard.css");
pub(crate) const MASCOT_PATH: &str = "/__topcoat-dashboard-mascot.png";
pub(crate) const MASCOT: &[u8] = include_bytes!("assets/sleeping-lizzy.png");

enum Screen {
    Home,
    Overview(String),
}

pub(crate) fn home(cx: &Cx) -> BoxView<'_> {
    dashboard(cx, Screen::Home)
}

pub(crate) fn overview<'a>(cx: &'a Cx, identifier: &str) -> BoxView<'a> {
    dashboard(cx, Screen::Overview(identifier.to_owned()))
}

fn dashboard(cx: &Cx, screen: Screen) -> BoxView<'_> {
    let (mode, title, section, identifier) = match screen {
        Screen::Home => ("home", "Home", "My active issues", None),
        Screen::Overview(identifier) => (
            "overview",
            "Project overview",
            "Needs attention",
            Some(identifier),
        ),
    };
    view! { cx =>
        <section class="tc-dashboard" data-topcoat-dashboard=(mode)
            data-project-identifier=(identifier.as_deref()) aria-busy="true">
            <p data-dashboard-status="" role="status" aria-live="polite">"Loading your dashboard…"</p>
            <div data-dashboard-errors="" role="status" aria-live="polite"></div>
            <div data-dashboard-content="">
                <header class="tc-dashboard__hero"><h1>(title)</h1></header>
                <h2>(section)</h2>
                <div class="tc-dashboard__skeleton" aria-hidden="true">
                    <div class="tc-dashboard__skeleton-card"></div>
                    <div class="tc-dashboard__skeleton-card"></div>
                </div>
            </div>
        </section>
    }.boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn home_has_accessible_loading_frame_and_runtime_mount() {
        let cx = Cx::default();
        let html = home(&cx).single().await.unwrap().render(&cx);
        assert!(html.contains("data-topcoat-dashboard=\"home\""));
        assert!(html.contains("aria-busy=\"true\""));
        assert!(html.contains("My active issues"));
        assert!(html.contains("data-dashboard-content"));
        assert!(html.contains("data-dashboard-status"));
    }

    #[tokio::test]
    async fn overview_carries_deep_link_identifier_without_rendering_administration() {
        let cx = Cx::default();
        let html = overview(&cx, "LIF").single().await.unwrap().render(&cx);
        assert!(html.contains("data-topcoat-dashboard=\"overview\""));
        assert!(html.contains("data-project-identifier=\"LIF\""));
        assert!(html.contains("Needs attention"));
        assert!(!html.contains("Delete project"));
        assert!(!html.contains("Publish"));
    }
}
