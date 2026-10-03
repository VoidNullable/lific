//! Editable issue description surface; writes are dispatched through the route.

use topcoat::{
    context::Cx,
    view::{BoxView, ViewExt, view},
};

pub(crate) const SCRIPT_PATH: &str = "/__topcoat-issue-editor.js";
pub(crate) const SCRIPT: &str = include_str!("assets/editor.js");
pub(crate) const STYLESHEET_PATH: &str = "/__topcoat-issue-editor.css";
pub(crate) const STYLESHEET: &str = include_str!("assets/editor.css");

/// Render the stable host used by the issue-detail route's editor initializer.
pub(crate) fn editor(cx: &Cx) -> BoxView<'_> {
    view! { cx =>
        <section class="tc-issue-editor" data-topcoat-issue-editor="" hidden="hidden">
            <div class="tc-issue-editor__toolbar" role="toolbar" aria-label="Description">
                <button class="tc-button" type="button" data-editor-edit="" hidden="hidden">"Edit"</button>
                <button class="tc-button" type="button" data-editor-preview-toggle="" aria-pressed="false">"Preview"</button>
                <button class="tc-button" type="button" data-editor-save="" disabled="disabled">"Save"</button>
                <button class="tc-button" type="button" data-editor-cancel="" hidden="hidden">"Cancel"</button>
            </div>
            <section class="tc-issue-editor__attachments" data-editor-attachments="" aria-label="Description attachments">
                <form data-attachment-upload="">
                    <label>"Attach files "<input type="file" multiple="multiple" data-attachment-files="" /></label>
                    <button class="tc-button" type="submit">"Upload"</button>
                    <button class="tc-button" type="button" data-attachment-cancel="" hidden="hidden">"Cancel"</button>
                    <progress data-attachment-progress="" max="1" value="0" hidden="hidden" aria-label="Upload progress"></progress>
                    <p data-attachment-status="" role="status"></p>
                </form>
            </section>
            <p class="tc-issue-editor__status" data-editor-status="" role="status" aria-live="polite">"Saved"</p>
            <p class="tc-issue-editor__error" data-editor-error="" role="alert" hidden="hidden"></p>
            <section class="tc-issue-editor__conflict" data-editor-conflict="" hidden="hidden" aria-live="assertive">
                <h3>"Description changed on the server"</h3>
                <p data-editor-conflict-message=""></p>
                <pre data-editor-server-value=""></pre>
            </section>
            <textarea class="tc-issue-editor__input" data-editor-input="" aria-label="Issue description"
                placeholder="Add a description... (markdown supported)" disabled="disabled"></textarea>
            <article class="tc-issue-editor__preview" data-editor-preview="" hidden="hidden"></article>
        </section>
    }.boxed()
}
