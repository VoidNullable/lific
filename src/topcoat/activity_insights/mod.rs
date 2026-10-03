//! Project audit history, aggregate insights, and dependency exploration.

use topcoat::{
    context::Cx,
    view::{BoxView, ViewExt, view},
};

pub(crate) const SCRIPT_PATH: &str = "/__topcoat-activity-insights.js";
pub(crate) const SCRIPT: &str = concat!(
    include_str!("assets/model.js"),
    include_str!("assets/routes.js")
);
pub(crate) const STYLESHEET_PATH: &str = "/__topcoat-activity-insights.css";
pub(crate) const STYLESHEET: &str = include_str!("assets/routes.css");

pub(crate) fn activity<'a>(cx: &'a Cx, project: &str) -> BoxView<'a> {
    screen(cx, project, "activity")
}

pub(crate) fn insights<'a>(cx: &'a Cx, project: &str) -> BoxView<'a> {
    screen(cx, project, "insights")
}

pub(crate) fn graph<'a>(cx: &'a Cx, project: &str) -> BoxView<'a> {
    screen(cx, project, "graph")
}

fn screen<'a>(cx: &'a Cx, project: &str, mode: &'static str) -> BoxView<'a> {
    let project = project.to_owned();
    view! { cx =>
        <section class="tc-analytics" data-topcoat-analytics=(mode)
            data-project-identifier=(project.as_str()) aria-busy="true">
            <p data-analytics-status="" role="status" aria-live="polite">"Loading project data…"</p>
            <div data-analytics-error="" role="alert" hidden="hidden"></div>
            <div data-analytics-content=""></div>
        </section>
    }
    .boxed()
}
