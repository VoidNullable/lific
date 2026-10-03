//! Anonymous, read-only public project route family.

use topcoat::{
    context::Cx,
    view::{BoxView, ViewExt, view},
};

pub(crate) const SCRIPT_PATH: &str = "/__topcoat-public.js";
pub(crate) const SCRIPT: &str = concat!(
    include_str!("../attachments/assets/attachments.js"),
    include_str!("assets/public.js")
);
pub(crate) const STYLESHEET_PATH: &str = "/__topcoat-public.css";
pub(crate) const STYLESHEET: &str = include_str!("assets/public.css");

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Route {
    Redirect(String),
    Issues { project: String },
    Board { project: String },
    IssueDetail { project: String, identifier: String },
    Pages { project: String },
    PageDetail { project: String, page_id: i64 },
}

fn valid_project(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z' | b'a'..=b'z'))
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_issue_identifier(value: &str) -> bool {
    let Some((project, sequence)) = value.rsplit_once('-') else {
        return false;
    };
    valid_project(project)
        && !sequence.is_empty()
        && sequence.bytes().all(|byte| byte.is_ascii_digit())
}

/// Resolve only the public routes supported by the existing web router.
/// Unsupported public paths return `None` and remain not-found routes.
pub(crate) fn resolve(path: &str) -> Option<Route> {
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let rest = path.strip_prefix("/public/")?;
    let mut parts = rest.split('/');
    let project = parts.next()?;
    if !valid_project(project) {
        return None;
    }
    let project = project.to_owned();
    let remaining = parts.collect::<Vec<_>>();

    match remaining.as_slice() {
        [] | [""] => Some(Route::Redirect(format!("/public/{project}/issues"))),
        [legacy] if valid_issue_identifier(legacy) => Some(Route::Redirect(format!(
            "/public/{project}/issues/{legacy}"
        ))),
        [section] if section.eq_ignore_ascii_case("issues") => Some(Route::Issues { project }),
        [section] if section.eq_ignore_ascii_case("board") => Some(Route::Board { project }),
        [section, identifier]
            if section.eq_ignore_ascii_case("issues") && valid_issue_identifier(identifier) =>
        {
            Some(Route::IssueDetail {
                project,
                identifier: (*identifier).to_owned(),
            })
        }
        [section] if section.eq_ignore_ascii_case("pages") => Some(Route::Pages { project }),
        [section, id]
            if section.eq_ignore_ascii_case("pages")
                && !id.is_empty()
                && id.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            Some(Route::PageDetail {
                project,
                page_id: id.parse().ok()?,
            })
        }
        _ => None,
    }
}

pub(crate) fn screen<'a>(cx: &'a Cx, route: Route) -> Option<BoxView<'a>> {
    let (project, kind, identifier) = match route {
        Route::Issues { project } => (project, "issues", None),
        Route::Board { project } => (project, "board", None),
        Route::IssueDetail {
            project,
            identifier,
        } => (project, "issue-detail", Some(identifier)),
        Route::Pages { project } => (project, "pages", None),
        Route::PageDetail { project, page_id } => {
            (project, "page-detail", Some(page_id.to_string()))
        }
        Route::Redirect(_) => return None,
    };
    let public_project = project.to_ascii_uppercase();
    let project_href = format!("/public/{project}/issues");
    let heading = match kind {
        "issues" => "Issues",
        "board" => "Board",
        "issue-detail" => "Issue",
        "pages" => "Pages",
        _ => "Page",
    };
    Some(view! { cx =>
        <section class="tc-public" data-topcoat-public=(kind)
            data-public-project=(public_project.as_str()) data-public-identifier=(identifier.as_deref())
            aria-busy="true" aria-readonly="true">
            <header class="tc-public__header">
                <a href=(project_href.as_str()) aria-label="Public project issues">(project.as_str())</a>
                <h1>(heading)</h1>
                <span class="tc-public__badge">"Public · read only"</span>
            </header>
            <p data-public-status="" role="status" aria-live="polite">"Loading…"</p>
            <div data-public-error="" role="alert" hidden="hidden"></div>
            <section data-public-content="" hidden="hidden"></section>
        </section>
    }.boxed())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_routes_match_the_five_supported_families_and_two_redirects() {
        assert_eq!(
            resolve("/public/ENG"),
            Some(Route::Redirect("/public/ENG/issues".into()))
        );
        assert_eq!(
            resolve("/public/ENG/"),
            Some(Route::Redirect("/public/ENG/issues".into()))
        );
        assert_eq!(
            resolve("/public/ENG/ENG-42?focus=1"),
            Some(Route::Redirect("/public/ENG/issues/ENG-42".into()))
        );
        assert_eq!(
            resolve("/public/ENG/issues"),
            Some(Route::Issues {
                project: "ENG".into()
            })
        );
        assert_eq!(resolve("/public/ENG/ISSUES"), resolve("/public/ENG/issues"));
        assert_eq!(
            resolve("/public/ENG/board"),
            Some(Route::Board {
                project: "ENG".into()
            })
        );
        assert_eq!(resolve("/public/ENG/BOARD"), resolve("/public/ENG/board"));
        assert_eq!(
            resolve("/public/ENG/issues/ENG-42"),
            Some(Route::IssueDetail {
                project: "ENG".into(),
                identifier: "ENG-42".into()
            })
        );
        assert_eq!(
            resolve("/public/ENG/pages"),
            Some(Route::Pages {
                project: "ENG".into()
            })
        );
        assert_eq!(
            resolve("/public/ENG/pages/42"),
            Some(Route::PageDetail {
                project: "ENG".into(),
                page_id: 42
            })
        );
        assert_eq!(
            resolve("/public/ENG/PAGES/42"),
            resolve("/public/ENG/pages/42")
        );
    }

    #[test]
    fn unsupported_public_routes_do_not_fall_through_to_private_routes() {
        for path in [
            "/public/ENG/overview",
            "/public/ENG/settings",
            "/public/ENG/files",
            "/public/ENG/modules",
            "/public/ENG/issues/new",
            "/public/ENG/pages/not-a-number",
            "/ENG/issues/ENG-42",
        ] {
            assert_eq!(resolve(path), None, "{path}");
        }
    }

    #[tokio::test]
    async fn public_mounts_have_only_read_only_route_markers() {
        let cx = Cx::default();
        for route in [
            Route::Issues {
                project: "ENG".into(),
            },
            Route::Board {
                project: "ENG".into(),
            },
            Route::IssueDetail {
                project: "ENG".into(),
                identifier: "ENG-42".into(),
            },
            Route::Pages {
                project: "ENG".into(),
            },
            Route::PageDetail {
                project: "ENG".into(),
                page_id: 42,
            },
        ] {
            let html = screen(&cx, route)
                .unwrap()
                .single()
                .await
                .unwrap()
                .render(&cx);
            assert!(html.contains("data-topcoat-public="));
            assert!(html.contains("data-public-project=\"ENG\""));
            assert!(html.contains("aria-readonly=\"true\""));
            assert!(!html.contains("<form"));
            assert!(!html.contains("<input"));
            assert!(!html.contains("<button"));
        }
    }

    #[test]
    fn public_assets_embed_scoped_attachment_reads() {
        assert_eq!(SCRIPT_PATH, "/__topcoat-public.js");
        assert_eq!(STYLESHEET_PATH, "/__topcoat-public.css");
        assert!(SCRIPT.contains("LificTopcoatAttachments"));
        assert!(SCRIPT.contains("LificTopcoatPublic"));
        assert!(STYLESHEET.contains(".tc-public"));
    }
}
