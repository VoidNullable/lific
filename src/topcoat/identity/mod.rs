//! Sign-in, account preferences, and instance administration.

use topcoat::{
    context::Cx,
    view::{BoxView, ViewExt, view},
};

pub(crate) const SCRIPT_PATH: &str = "/__topcoat-identity.js";
pub(crate) const SCRIPT: &str = include_str!("assets/identity.js");
pub(crate) const STYLESHEET_PATH: &str = "/__topcoat-identity.css";
pub(crate) const STYLESHEET: &str = include_str!("assets/identity.css");

pub(crate) fn login(cx: &Cx) -> BoxView<'_> {
    screen(cx, "login")
}

pub(crate) fn signup(cx: &Cx) -> BoxView<'_> {
    screen(cx, "signup")
}

pub(crate) fn settings(cx: &Cx) -> BoxView<'_> {
    screen(cx, "settings")
}

pub(crate) fn instance_settings(cx: &Cx) -> BoxView<'_> {
    screen(cx, "instance")
}

fn screen<'a>(cx: &'a Cx, mode: &str) -> BoxView<'a> {
    let mode = mode.to_owned();
    view! { cx =>
        <section class="tc-identity" data-topcoat-identity=(mode.as_str())
            aria-busy="true" aria-live="polite">
            <p class="tc-identity__status" data-identity-status="" role="status">"Loading…"</p>
            <div data-identity-content=""></div>
        </section>
    }
    .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn identity_routes_render_an_accessible_mount_for_each_mode() {
        let cx = Cx::default();
        for mode in ["login", "signup", "settings", "instance"] {
            let html = screen(&cx, mode).single().await.unwrap().render(&cx);
            assert!(html.contains(&format!("data-topcoat-identity=\"{mode}\"")));
            assert!(html.contains("data-identity-content"));
            assert!(html.contains("data-identity-status"));
            assert!(html.contains("aria-busy=\"true\""));
        }
    }

    #[test]
    fn identity_scripts_and_stylesheets_are_embedded() {
        assert!(SCRIPT.contains("LificTopcoatIdentity"));
        assert!(STYLESHEET.contains(".tc-identity"));
        assert_eq!(SCRIPT_PATH, "/__topcoat-identity.js");
        assert_eq!(STYLESHEET_PATH, "/__topcoat-identity.css");
    }
}
