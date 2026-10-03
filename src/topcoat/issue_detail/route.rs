//! Issue-detail route composition and the browser-owned issue read model.

use topcoat::{
    context::Cx,
    view::{BoxView, ViewExt, view},
};

pub(crate) const SCRIPT_PATH: &str = "/__topcoat-issue-detail.js";
pub(crate) const SCRIPT: &str = include_str!("assets/route.js");
pub(crate) const STYLESHEET_PATH: &str = "/__topcoat-issue-detail.css";
pub(crate) const STYLESHEET: &str = include_str!("assets/route.css");

pub(crate) fn screen<'a>(
    cx: &'a Cx,
    project_identifier: &'a str,
    issue_identifier: &'a str,
    public_scope: bool,
) -> BoxView<'a> {
    let scope = if public_scope { "public" } else { "private" };
    view! { cx =>
        <section class="tc-issue-detail" data-topcoat-issue-detail=""
            data-project-identifier=(project_identifier) data-issue-identifier=(issue_identifier)
            data-issue-scope=(scope) aria-busy="true">
                <div class="tc-issue-detail__loading" data-detail-loading="">
                    <h1>"Loading issue…"</h1>
                    <p role="status" aria-live="polite">"Loading issue details…"</p>
                </div>
                <div class="tc-issue-detail__error" data-detail-error="" role="alert" hidden="hidden"></div>
                <div class="tc-issue-detail__content" data-detail-content="" hidden="hidden">
                    <header class="tc-issue-detail__heading">
                        <a class="tc-issue-detail__back" data-detail-back="" href=(format!("/{project_identifier}/issues"))>"← Issues"</a>
                        <p data-detail-identifier=""></p>
                        <h1 data-detail-title=""></h1>
                    </header>
                    <div class="tc-issue-detail__layout">
                        <div class="tc-issue-detail__main">
                            (super::fields::scaffold(cx))
                            (super::editor::editor(cx))
                        </div>
                        <aside class="tc-issue-detail__aside">
                            (super::collaboration::empty_panel(
                                cx,
                                super::RouteKey { issue_id: 0, generation: 0 },
                                super::Capabilities { edit: false, comment: false },
                            ))
                        </aside>
                    </div>
                </div>
            </section>
    }
    .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn issue_detail_route_has_empty_mount_points_and_scope_is_explicit() {
        let cx = Cx::default();
        for (public_scope, expected) in [(false, "private"), (true, "public")] {
            let html = screen(&cx, "ENG", "ENG-42", public_scope)
                .single()
                .await
                .unwrap()
                .render(&cx);
            assert!(html.contains("data-topcoat-issue-detail=\"\""));
            assert!(html.contains("data-project-identifier=\"ENG\""));
            assert!(html.contains("data-issue-identifier=\"ENG-42\""));
            assert!(html.contains(&format!("data-issue-scope=\"{expected}\"")));
            assert!(html.contains("data-issue-fields=\"\""));
            assert!(html.contains("data-topcoat-issue-editor=\"\""));
            assert!(html.contains("data-topcoat-collaboration=\"\""));
        }
    }
}
