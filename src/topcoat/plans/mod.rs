//! Plan lists, nested step editing, and issue links through the existing REST API.

use topcoat::{
    context::Cx,
    view::{BoxView, ViewExt, view},
};

pub(crate) const SCRIPT_PATH: &str = "/__topcoat-plans.js";
pub(crate) const SCRIPT: &str = concat!(
    include_str!("../issue_detail/editor/assets/editor.js"),
    include_str!("assets/picker.js"),
    include_str!("assets/plans.js")
);
pub(crate) const STYLESHEET_PATH: &str = "/__topcoat-plans.css";
pub(crate) const STYLESHEET: &str = concat!(
    include_str!("assets/picker.css"),
    include_str!("assets/plans.css")
);

pub(crate) fn list<'a>(cx: &'a Cx, project: &str) -> BoxView<'a> {
    screen(cx, project, None)
}

pub(crate) fn detail<'a>(cx: &'a Cx, project: &str, plan_id: i64) -> BoxView<'a> {
    screen(cx, project, Some(plan_id))
}

fn screen<'a>(cx: &'a Cx, project: &str, plan_id: Option<i64>) -> BoxView<'a> {
    let project = project.to_owned();
    let mode = if plan_id.is_some() { "detail" } else { "list" };
    let plan_id = plan_id.map(|id| id.to_string());
    view! { cx =>
        <section class="tc-plans" data-topcoat-plans=(mode)
            data-project-identifier=(project.as_str()) data-plan-id=(plan_id.as_deref()) aria-busy="true">
            <p data-plans-status="" role="status" aria-live="polite">"Loading plans…"</p>
            <div data-plans-error="" role="alert" hidden="hidden"></div>
            <div data-plans-content=""></div>
        </section>
    }.boxed()
}
