//! Module metadata, live issue counts, and issue assignment.

use topcoat::{
    context::Cx,
    view::{BoxView, ViewExt, view},
};

pub(crate) const SCRIPT_PATH: &str = "/__topcoat-modules.js";
pub(crate) const SCRIPT: &str = concat!(
    include_str!("../issue_detail/editor/assets/editor.js"),
    include_str!("../plans/assets/picker.js"),
    include_str!("assets/modules.js")
);
pub(crate) const STYLESHEET_PATH: &str = "/__topcoat-modules.css";
pub(crate) const STYLESHEET: &str = concat!(
    include_str!("../plans/assets/picker.css"),
    include_str!("assets/modules.css")
);

pub(crate) fn list<'a>(cx: &'a Cx, project: &str) -> BoxView<'a> {
    screen(cx, project, None)
}

pub(crate) fn detail<'a>(cx: &'a Cx, project: &str, module_id: i64) -> BoxView<'a> {
    screen(cx, project, Some(module_id))
}

fn screen<'a>(cx: &'a Cx, project: &str, module_id: Option<i64>) -> BoxView<'a> {
    let project = project.to_owned();
    let mode = if module_id.is_some() {
        "detail"
    } else {
        "list"
    };
    let module_id = module_id.map(|id| id.to_string());
    view! { cx =>
        <section class="tc-modules" data-topcoat-modules=(mode)
            data-project-identifier=(project.as_str()) data-module-id=(module_id.as_deref()) aria-busy="true">
            <p data-modules-status="" role="status" aria-live="polite">"Loading modules…"</p>
            <div data-modules-error="" role="alert" hidden="hidden"></div>
            <div data-modules-content=""></div>
        </section>
    }.boxed()
}
