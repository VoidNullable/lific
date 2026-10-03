//! Project and workspace page routes.

use topcoat::{
    context::Cx,
    view::{BoxView, ViewExt, view},
};

pub(crate) const SCRIPT_PATH: &str = "/__topcoat-pages.js";
pub(crate) const SCRIPT: &str = concat!(
    include_str!("../attachments/assets/attachments.js"),
    include_str!("assets/pages.js")
);
pub(crate) const STYLESHEET_PATH: &str = "/__topcoat-pages.css";
pub(crate) const STYLESHEET: &str = concat!(
    include_str!("../attachments/assets/attachments.css"),
    include_str!("assets/pages.css")
);

pub(crate) fn list<'a>(cx: &'a Cx, project: &'a str, public: bool) -> BoxView<'a> {
    let scope = if public { "public" } else { "private" };
    view! { cx =>
        <section class="tc-pages" data-topcoat-pages="list" data-project-identifier=(project)
            data-page-scope=(scope) aria-busy="true">
            <header class="tc-pages__heading"><h1>"Pages"</h1>
                <button class="tc-button" type="button" data-page-create="" hidden="hidden">"New page"</button>
                <details data-page-create-presets="" hidden="hidden">
                    <summary class="tc-button">"New page as…"</summary>
                    <div><button class="tc-button" type="button" data-page-create-preset="draft">"Draft"</button>
                        <button class="tc-button" type="button" data-page-create-preset="active">"Active"</button>
                        <button class="tc-button" type="button" data-page-create-preset="complete">"Complete"</button></div>
                </details>
            </header>
            <p class="tc-pages__status" data-pages-status="" role="status" aria-live="polite">"Loading pages…"</p>
            <div class="tc-pages__error" data-pages-error="" role="alert" hidden="hidden"></div>
            <form class="tc-pages__filters" data-pages-filters="">
                <label>"Search "<input type="search" data-pages-search="" /></label>
                <label>"Lifecycle "<select data-pages-status-filter="">
                    <option value="__active" selected="selected">"Active"</option><option value="">"All statuses"</option><option>"draft"</option><option>"active"</option>
                    <option>"complete"</option><option>"archived"</option>
                </select></label>
                <label>"Folder "<select data-pages-folder=""><option value="">"All folders"</option></select></label>
                <label>"Label "<select data-pages-label-filter=""><option value="">"All labels"</option></select></label>
                <button class="tc-button" type="button" data-pages-folder-create="" hidden="hidden">"New folder"</button>
            </form>
            <nav class="tc-pages__tabs" data-pages-tabs="" aria-label="Page views">
                <button class="tc-button" type="button" data-pages-tab="browse" aria-current="page">"Browse"</button>
                <button class="tc-button" type="button" data-pages-tab="recent">"Recent"</button>
                <button class="tc-button" type="button" data-pages-tab="drafts">"Drafts"</button>
                <button class="tc-button" type="button" data-pages-tab="archived">"Archived"</button>
            </nav>
            <ul class="tc-pages__folder-tree" data-pages-folder-tree=""></ul>
            <div data-pages-content=""></div>
            <dialog data-pages-create-dialog="">
                <form method="dialog" data-pages-create-form="">
                    <h2>"New page"</h2><label>"Title "<input name="title" required="" maxlength="200" /></label>
                    <label>"Status "<select name="status"><option value="draft">"Draft"</option><option value="active">"Active"</option><option value="complete">"Complete"</option></select></label>
                    <label>"Folder "<select name="folder_id"><option value="">"No folder"</option></select></label>
                    <div class="tc-pages__dialog-actions"><button class="tc-button" value="cancel">"Cancel"</button>
                        <button class="tc-button tc-button--primary" type="submit" data-pages-create-submit="">"Create"</button></div>
                </form>
            </dialog>
            <dialog data-pages-peek-dialog="">
                <h2 data-pages-peek-title=""></h2><article data-pages-peek-content=""></article>
                <a class="tc-button" data-pages-peek-open="">"Open page"</a>
                <button class="tc-button" type="button" data-pages-peek-close="">"Close"</button>
            </dialog>
        </section>
    }.boxed()
}

pub(crate) fn detail<'a>(cx: &'a Cx, project: &'a str, page_id: i64, public: bool) -> BoxView<'a> {
    let scope = if public { "public" } else { "private" };
    view! { cx =>
        <section class="tc-pages tc-page-detail" data-topcoat-pages="detail"
            data-project-identifier=(project) data-page-id=(page_id.to_string()) data-page-scope=(scope)
            aria-busy="true">
            <p class="tc-pages__status" data-page-status-message="" role="status" aria-live="polite">"Loading page…"</p>
            <div class="tc-pages__error" data-page-error="" role="alert" hidden="hidden"></div>
            <article data-page-content="" hidden="hidden">
                <nav aria-label="Breadcrumb"><a href=(format!("/{}/pages", if public { format!("public/{project}") } else { project.to_owned() }))>"Pages"</a><span data-page-folder-crumb=""></span></nav>
                <header class="tc-page-detail__heading">
                    <input class="tc-page-detail__title" data-page-title="" aria-label="Page title" disabled="disabled" />
                    <div class="tc-page-detail__actions">
                        <select data-page-lifecycle="" aria-label="Page status" disabled="disabled">
                            <option value="draft">"Draft"</option><option value="active">"Active"</option>
                            <option value="complete">"Complete"</option><option value="archived">"Archived"</option>
                        </select>
                        <button class="tc-button" type="button" data-page-pin="" hidden="hidden">"Pin"</button>
                        <button class="tc-button" type="button" data-page-export="">"Export Markdown"</button>
                        <button class="tc-button" type="button" data-page-delete="" hidden="hidden">"Delete"</button>
                    </div>
                </header>
                <label class="tc-page-detail__folder" data-page-folder-control="" hidden="hidden">"Folder "
                    <select data-page-folder="" disabled="disabled"><option value="">"No folder"</option></select>
                </label>
                <div class="tc-page-detail__labels" data-page-labels=""></div>
                <section class="tc-page-editor" data-page-editor="">
                    <div class="tc-page-editor__toolbar"><button class="tc-button" type="button" data-page-edit="" hidden="hidden">"Edit"</button>
                        <button class="tc-button" type="button" data-page-preview="" aria-pressed="true">"Edit Markdown"</button>
                        <button class="tc-button" type="button" data-page-save="" disabled="disabled">"Save"</button>
                        <button class="tc-button" type="button" data-page-cancel="" hidden="hidden">"Cancel"</button></div>
                    <textarea data-page-body="" aria-label="Page content" placeholder="Write in Markdown…" disabled="disabled" hidden="hidden"></textarea>
                    <article data-page-preview-content=""></article>
                    <p data-page-save-status="" role="status" aria-live="polite"></p>
                </section>
                <section data-page-attachments="" class="tc-page-attachments">
                    <h2>"Attachments"</h2><ul data-page-attachment-list=""></ul>
                    <label data-page-attachment-upload="" hidden="hidden">"Upload files "<input data-page-files="" type="file" multiple="" /></label>
                    <p data-page-attachment-status="" role="status" aria-live="polite"></p>
                    <div data-page-attachment-viewer="" hidden="hidden"></div>
                </section>
                <section class="tc-page-comments" data-page-comments="">
                    <h2>"Comments"</h2><ol data-page-comment-list=""></ol>
                    <button class="tc-button" type="button" data-page-comments-older="" hidden="hidden">"Load older comments"</button>
                    <form data-page-comment-form=""><label>"Add a comment "<textarea name="content" required=""></textarea></label>
                        <label>"Attach files "<input data-page-comment-files="" type="file" multiple="" /></label>
                        <button class="tc-button tc-button--primary" type="submit">"Comment"</button></form>
                </section>
                <section class="tc-page-activity"><h2>"Activity"</h2><ol data-page-activity=""></ol></section>
            </article>
        </section>
    }.boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn page_list_exposes_scoped_filters_and_mutation_mounts() {
        let cx = Cx::default();
        let html = list(&cx, "LIF", false).single().await.unwrap().render(&cx);
        assert!(html.contains("data-topcoat-pages=\"list\""));
        assert!(html.contains("data-page-scope=\"private\""));
        assert!(html.contains("data-pages-search"));
        assert!(html.contains("data-pages-folder"));
        let public = list(&cx, "LIF", true).single().await.unwrap().render(&cx);
        assert!(public.contains("data-page-scope=\"public\""));
        assert!(public.contains("data-page-create=\"\" hidden=\"hidden\""));
    }

    #[tokio::test]
    async fn page_detail_mount_supports_readonly_public_and_explicit_body_saves() {
        let cx = Cx::default();
        let html = detail(&cx, "LIF", 17, true)
            .single()
            .await
            .unwrap()
            .render(&cx);
        assert!(html.contains("data-page-id=\"17\""));
        assert!(html.contains("data-page-scope=\"public\""));
        assert!(html.contains("data-page-save=\"\" disabled=\"disabled\""));
        assert!(html.contains("data-page-comment-form"));
        assert!(html.contains("data-page-activity"));
    }

    #[test]
    fn page_assets_are_the_registered_embedded_resources() {
        assert_eq!(SCRIPT_PATH, "/__topcoat-pages.js");
        assert_eq!(STYLESHEET_PATH, "/__topcoat-pages.css");
        assert!(SCRIPT.contains("LificTopcoatPages"));
        assert!(STYLESHEET.contains(".tc-pages"));
        assert!(STYLESHEET.contains(".tc-attachments"));
    }
}
