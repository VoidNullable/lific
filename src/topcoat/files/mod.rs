//! Project files browser and attachment management.

use topcoat::{
    context::Cx,
    view::{BoxView, ViewExt, view},
};

pub(crate) const SCRIPT_PATH: &str = "/__topcoat-files.js";
pub(crate) const SCRIPT: &str = concat!(
    include_str!("../attachments/assets/attachments.js"),
    include_str!("assets/files.js")
);
pub(crate) const STYLESHEET_PATH: &str = "/__topcoat-files.css";
pub(crate) const STYLESHEET: &str = include_str!("assets/files.css");

pub(crate) fn screen<'a>(cx: &'a Cx, project: &'a str) -> BoxView<'a> {
    view! { cx =>
        <section class="tc-files" data-topcoat-files="" data-project-identifier=(project)
            aria-busy="true">
            <header class="tc-files__header">
                <nav aria-label="Breadcrumb"><a href=(format!("/{project}/overview"))>"Project"</a><span>" / "</span></nav>
                <h1>"Files"</h1>
                <p data-files-status="" role="status" aria-live="polite">"Loading files…"</p>
            </header>
            <div data-files-error="" role="alert" hidden="hidden"></div>
            <div class="tc-files__filters" data-files-filters="">
                <fieldset><legend>"File type"</legend>
                    <button type="button" data-files-mime="" aria-pressed="true">"All"</button>
                    <button type="button" data-files-mime="image">"Images"</button>
                    <button type="button" data-files-mime="video">"Video"</button>
                    <button type="button" data-files-mime="audio">"Audio"</button>
                    <button type="button" data-files-mime="text">"Text"</button>
                    <button type="button" data-files-mime="pdf">"PDF"</button>
                    <button type="button" data-files-mime="archive">"Archives"</button>
                    <button type="button" data-files-mime="other">"Other"</button>
                </fieldset>
                <label>"Uploader "<select data-files-uploader=""><option value="">"All uploaders"</option></select></label>
                <label>"Sort "<select data-files-sort="">
                    <option value="created_at">"Newest"</option><option value="size">"Largest"</option><option value="filename">"Filename"</option>
                </select></label>
            </div>
            <p class="tc-files__totals"><span data-files-count="">"0 files"</span><span data-files-bytes="">"0 B"</span></p>
            <div data-files-list="" aria-live="polite"></div>
            <button class="tc-button" type="button" data-files-more="" hidden="hidden">"Load more"</button>
            <section class="tc-files__orphans">
                <button class="tc-files__orphans-toggle" type="button" data-files-orphans-toggle="" aria-expanded="false">
                    "Unlinked uploads "<span data-files-orphan-count="">""</span>
                </button>
                <p>"Unlinked uploads are removed by the cleanup sweep after their grace period."</p>
                <div data-files-orphans="" hidden="hidden"></div>
            </section>
            <dialog data-files-viewer="" aria-label="File preview">
                <h2 data-files-viewer-title=""></h2><p data-files-viewer-status="" role="status"></p>
                <div data-files-viewer-content=""></div>
                <a data-files-download="" download="download">"Download original"</a>
                <button class="tc-button" type="button" data-files-viewer-close="">"Close"</button>
            </dialog>
        </section>
    }.boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn files_mount_contains_project_scope_filters_pagination_and_orphan_inventory() {
        let cx = Cx::default();
        let html = screen(&cx, "ENG").single().await.unwrap().render(&cx);
        assert!(html.contains("data-topcoat-files=\"\""));
        assert!(html.contains("data-project-identifier=\"ENG\""));
        assert!(html.contains("data-files-mime=\"image\""));
        assert!(html.contains("data-files-uploader"));
        assert!(html.contains("data-files-sort"));
        assert!(html.contains("data-files-more"));
        assert!(html.contains("data-files-orphans-toggle"));
        assert!(html.contains("data-files-viewer"));
    }

    #[test]
    fn assets_embed_the_shared_attachment_stream_client() {
        assert_eq!(SCRIPT_PATH, "/__topcoat-files.js");
        assert_eq!(STYLESHEET_PATH, "/__topcoat-files.css");
        assert!(SCRIPT.contains("LificTopcoatAttachments"));
        assert!(SCRIPT.contains("LificTopcoatFiles"));
        assert!(STYLESHEET.contains(".tc-files"));
    }
}
