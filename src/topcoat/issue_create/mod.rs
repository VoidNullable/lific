//! Topcoat issue creation using the shared project, session and attachment APIs.

use topcoat::{
    context::Cx,
    view::{BoxView, ViewExt, view},
};

pub(crate) const SCRIPT_PATH: &str = "/__topcoat-issue-create.js";
pub(crate) const SCRIPT: &str = include_str!("assets/issue-create.js");
pub(crate) const STYLESHEET_PATH: &str = "/__topcoat-issue-create.css";
pub(crate) const STYLESHEET: &str = include_str!("assets/issue-create.css");

pub(crate) fn screen<'a>(cx: &'a Cx, project_identifier: &str) -> BoxView<'a> {
    let identifier = project_identifier.to_owned();
    let issues_href = format!("/{project_identifier}/issues");
    view! { cx =>
        <section class="tc-issue-create" data-topcoat-issue-create=""
            data-project-identifier=(identifier) aria-busy="true" aria-label="Create issue">
            <div data-issue-create-loading="" role="status" aria-live="polite">"Loading project…"</div>
            <div data-issue-create-denied="" hidden="hidden">
                <h1>"You can't create issues here"</h1>
                <p>"Only project maintainers, leads and admins can create issues."</p>
                <a data-issue-create-back="" href=(issues_href.as_str())>"Back to issues"</a>
            </div>
            <form data-issue-create-form="" hidden="hidden">
                <header class="tc-issue-create__header">
                    <a data-issue-create-back="" href=(issues_href.as_str())>"‹ Issues"</a>
                    <span aria-hidden="true">"/"</span><h1>"New issue"</h1>
                    <p data-issue-create-error="" role="alert" aria-live="assertive"></p>
                    <button type="button" data-issue-create-discard="">"Discard"</button>
                    <button type="submit" data-issue-create-submit="" disabled="disabled">"Create issue"</button>
                </header>
                <div class="tc-issue-create__body">
                    <main>
                        <label class="tc-issue-create__title-label" for="tc-issue-create-title">"Issue title"</label>
                        <input id="tc-issue-create-title" name="title" data-issue-create-title="" autofocus="autofocus" required="required" maxlength="500" placeholder="Issue title">
                        <label for="tc-issue-create-description">"Description"</label>
                        <textarea id="tc-issue-create-description" data-issue-create-description="" placeholder="Add a description… (markdown supported)" rows="8"></textarea>
                        <div class="tc-issue-create__attachments">
                            <label>"Attach files "<input type="file" multiple="multiple" data-issue-create-files=""></label>
                            <span data-issue-create-upload-status="" role="status" aria-live="polite">"Markdown · drag, paste or attach files"</span>
                            <ul data-issue-create-uploads="" aria-label="Pending uploads"></ul>
                        </div>
                    </main>
                    <aside aria-label="Issue properties">
                        <label for="tc-issue-create-status">"Status"</label>
                        <select id="tc-issue-create-status" data-issue-create-status="">
                            <option value="backlog">"Backlog"</option><option value="todo">"Todo"</option>
                            <option value="active">"Active"</option><option value="done">"Done"</option>
                            <option value="cancelled">"Cancelled"</option>
                        </select>
                        <label for="tc-issue-create-priority">"Priority"</label>
                        <select id="tc-issue-create-priority" data-issue-create-priority="">
                            <option value="urgent">"Urgent"</option><option value="high">"High"</option>
                            <option value="medium">"Medium"</option><option value="low">"Low"</option>
                            <option value="none">"No priority"</option>
                        </select>
                        <label for="tc-issue-create-module">"Module"</label>
                        <select id="tc-issue-create-module" data-issue-create-module=""><option value="">"None"</option></select>
                        <fieldset><legend>"Labels"</legend>
                            <div data-issue-create-labels=""></div>
                            <div class="tc-issue-create__label-create">
                                <input type="text" data-issue-create-label-name="" placeholder="Create a label">
                                <input type="color" data-issue-create-label-color="" value="#6b7280" aria-label="New label color">
                                <button type="button" data-issue-create-label-submit="">"Create label"</button>
                            </div>
                        </fieldset>
                    </aside>
                </div>
            </form>
            <div data-issue-create-load-error="" role="alert">
                <p data-issue-create-load-message=""></p>
                <button type="button" data-issue-create-retry="">"Try again"</button>
                <a data-issue-create-back="">"Back to issues"</a>
            </div>
        </section>
    }.boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn issue_create_screen_escapes_identifier_and_starts_loading() {
        let cx = Cx::default();
        let html = screen(&cx, "ENG\" <script>")
            .single()
            .await
            .unwrap()
            .render(&cx);
        assert!(
            html.contains("data-project-identifier=\"ENG&amp;quot;")
                || html.contains("data-project-identifier=\"ENG&quot;")
        );
        assert!(html.contains("aria-busy=\"true\""));
        assert!(html.contains("data-issue-create-form"));
        assert!(html.contains("data-issue-create-files"));
    }
}
