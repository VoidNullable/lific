//! Project setup, transfer, and administration using the existing REST API.

use topcoat::{
    context::Cx,
    view::{BoxView, ViewExt, view},
};

pub(crate) const SCRIPT_PATH: &str = "/__topcoat-project-settings.js";
pub(crate) const SCRIPT: &str = include_str!("assets/project-settings.js");
pub(crate) const STYLESHEET_PATH: &str = "/__topcoat-project-settings.css";
pub(crate) const STYLESHEET: &str = include_str!("assets/project-settings.css");

pub(crate) fn new_project(cx: &Cx) -> BoxView<'_> {
    screen(cx, "new", "Create project", None)
}

pub(crate) fn archive_import(cx: &Cx) -> BoxView<'_> {
    screen(cx, "archive", "Import project archive", None)
}

pub(crate) fn administration<'a>(cx: &'a Cx, identifier: &str) -> BoxView<'a> {
    let identifier = identifier.to_owned();
    view! { cx =>
        <section class="tc-project-settings" data-topcoat-project-settings="settings"
            data-project-identifier=(identifier.as_str()) aria-busy="true" aria-label="Project administration">
            <p data-project-settings-status="" role="status" aria-live="polite">"Loading project access…"</p>
            <p data-project-settings-error="" role="alert"></p>
            <p data-project-settings-warning="" role="status" aria-live="polite"></p>
            <div data-project-settings-content=""></div>
        </section>
    }.boxed()
}

fn screen<'a>(cx: &'a Cx, mode: &str, title: &str, identifier: Option<String>) -> BoxView<'a> {
    let mode = mode.to_owned();
    let title = title.to_owned();
    view! { cx =>
        <section class="tc-project-settings" data-topcoat-project-settings=(mode.as_str())
            data-project-identifier=(identifier.as_deref()) aria-busy="true">
            <h1>(title.as_str())</h1>
            <p data-project-settings-status="" role="status" aria-live="polite">"Loading project access…"</p>
            <p data-project-settings-error="" role="alert"></p>
            <p data-project-settings-warning="" role="status" aria-live="polite"></p>
            <div data-project-settings-content=""></div>
        </section>
    }.boxed()
}
