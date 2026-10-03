//! Editable issue fields shared by the create and detail routes.

use topcoat::{
    context::Cx,
    view::{BoxView, ViewExt, view},
};

pub(crate) const SCRIPT_PATH: &str = "/__topcoat-issue-fields.js";
pub(crate) const SCRIPT: &str = include_str!("assets/fields.js");
pub(crate) const STYLESHEET_PATH: &str = "/__topcoat-issue-fields.css";
pub(crate) const STYLESHEET: &str = include_str!("assets/fields.css");

/// Renders a stable mount point. The browser hydrates it once the issue and
/// project metadata have loaded, keeping this shared shell independent of API
/// timing and stale route responses.
pub(crate) fn scaffold(cx: &Cx) -> BoxView<'_> {
    view! { cx =>
        <section class="tc-issue-fields" data-issue-fields="" aria-label="Issue fields">
            <label class="tc-issue-fields__title">"Title"
                <input data-field="title" name="title" maxlength="200" autocomplete="off" required="" />
            </label>
            <label>"Status"
                <select data-field="status" name="status">
                    <option value="backlog">"Backlog"</option>
                    <option value="todo">"Todo"</option>
                    <option value="active">"In progress"</option>
                    <option value="done">"Done"</option>
                    <option value="cancelled">"Cancelled"</option>
                </select>
            </label>
            <label>"Priority"
                <select data-field="priority" name="priority">
                    <option value="urgent">"Urgent"</option>
                    <option value="high">"High"</option>
                    <option value="medium">"Medium"</option>
                    <option value="low">"Low"</option>
                    <option value="none">"No priority"</option>
                </select>
            </label>
            <label>"Module"
                <select data-field="module_id" name="module_id"><option value="">"No module"</option></select>
            </label>
            <label>"Labels"
                <input data-field="labels" name="labels" autocomplete="off" aria-describedby="issue-label-help" />
            </label>
            <p id="issue-label-help" class="tc-issue-fields__hint">"Separate labels with commas."</p>
            <div class="tc-issue-fields__metadata" data-field-metadata="" hidden="hidden">
                <span data-field-created=""></span>
                <span data-field-updated=""></span>
            </div>
            <p class="tc-issue-fields__status" data-fields-status="" role="status" aria-live="polite"></p>
        </section>
    }.boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fields_scaffold_exposes_accessible_editable_controls_and_creation_defaults() {
        let cx = Cx::default();
        let html = scaffold(&cx).single().await.unwrap().render(&cx);
        assert!(html.contains("data-issue-fields=\"\""));
        assert!(html.contains("data-field=\"title\""));
        assert!(html.contains("data-field=\"status\""));
        assert!(html.contains("value=\"backlog\""));
        assert!(html.contains("data-field=\"priority\""));
        assert!(html.contains("value=\"none\""));
        assert!(html.contains("data-field=\"module_id\""));
        assert!(html.contains("data-field=\"labels\""));
        assert!(html.contains("aria-live=\"polite\""));
    }
}
