use std::fmt::{self, Display, Formatter};

use reqwest::Url;

pub(crate) fn parse_http_authority(host_header: &str) -> Option<axum::http::uri::Authority> {
    let authority = host_header.parse::<axum::http::uri::Authority>().ok()?;
    if authority.as_str() != host_header {
        return None;
    }
    if authority.as_str().contains('@') {
        return None;
    }
    Some(authority)
}

pub(crate) fn authority_is_allowlisted(
    authority: &axum::http::uri::Authority,
    allowed_hosts: &[String],
) -> bool {
    let host = authority
        .host()
        .trim_start_matches('[')
        .trim_end_matches(']');
    allowed_hosts
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(host))
}

#[derive(Debug, Clone)]
pub(crate) struct IssueLinkContext {
    base_url: Url,
}

#[derive(Clone, Copy)]
enum ResourcePath<'a> {
    Issue {
        project: &'a str,
        identifier: &'a str,
    },
    Project(&'a str),
    Page {
        project: &'a str,
        id: i64,
    },
    Plan {
        project: &'a str,
        id: i64,
    },
    Module {
        project: &'a str,
        id: i64,
    },
}

#[derive(Clone, Copy)]
pub(crate) struct ResourceUrl<'a> {
    base_url: &'a str,
    path: ResourcePath<'a>,
    comment_id: Option<i64>,
}

impl Display for ResourceUrl<'_> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.base_url)?;
        if !self.base_url.ends_with('/') {
            formatter.write_str("/")?;
        }
        match self.path {
            ResourcePath::Issue {
                project,
                identifier,
            } => write!(formatter, "{project}/issues/{identifier}")?,
            ResourcePath::Project(project) => write!(formatter, "{project}/overview")?,
            ResourcePath::Page { project, id } => write!(formatter, "{project}/pages/{id}")?,
            ResourcePath::Plan { project, id } => write!(formatter, "{project}/plans/{id}")?,
            ResourcePath::Module { project, id } => write!(formatter, "{project}/modules/{id}")?,
        }
        if let Some(comment_id) = self.comment_id {
            write!(formatter, "#comment-{comment_id}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum LinkLabel<'a> {
    Text(&'a str),
    Escaped(&'a str),
    Comment(i64),
}

impl LinkLabel<'_> {
    fn fmt(self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(label) => formatter.write_str(label),
            Self::Escaped(label) => {
                for character in label.chars() {
                    if matches!(character, '\\' | '[' | ']') {
                        formatter.write_str("\\")?;
                    }
                    write!(formatter, "{character}")?;
                }
                Ok(())
            }
            Self::Comment(comment_id) => write!(formatter, "comment #{comment_id}"),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct MarkdownReference<'a> {
    label: LinkLabel<'a>,
    url: Option<ResourceUrl<'a>>,
}

impl<'a> MarkdownReference<'a> {
    pub(crate) fn plain(label: &'a str) -> Self {
        Self {
            label: LinkLabel::Text(label),
            url: None,
        }
    }

    pub(crate) fn linked(label: &'a str, url: ResourceUrl<'a>) -> Self {
        Self {
            label: LinkLabel::Text(label),
            url: Some(url),
        }
    }
}

impl Display for MarkdownReference<'_> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        if let Some(url) = self.url {
            formatter.write_str("[")?;
            self.label.fmt(formatter)?;
            write!(formatter, "]({url})")
        } else {
            match self.label {
                LinkLabel::Escaped(label) => formatter.write_str(label),
                label => label.fmt(formatter),
            }
        }
    }
}

impl IssueLinkContext {
    #[must_use]
    pub(crate) fn parse(base_url: &str) -> Option<Self> {
        let base_url = Url::parse(base_url).ok()?;
        valid_base_url(&base_url).then_some(Self { base_url })
    }

    #[must_use]
    pub(crate) fn for_http_request(
        public_url: Option<&str>,
        host_header: Option<&str>,
        allowed_hosts: &[String],
    ) -> Option<Self> {
        match public_url {
            Some(public_url) => Self::parse(public_url),
            None => {
                let authority = parse_http_authority(host_header?)?;
                if !authority_is_allowlisted(&authority, allowed_hosts) {
                    return None;
                }
                Self::parse(&format!("http://{authority}"))
            }
        }
    }

    #[must_use]
    pub(crate) fn issue_markdown<'a>(&'a self, identifier: &'a str) -> MarkdownReference<'a> {
        MarkdownReference {
            label: LinkLabel::Text(identifier),
            url: self.issue_url(identifier),
        }
    }

    #[must_use]
    pub(crate) fn issue_url<'a>(&'a self, identifier: &'a str) -> Option<ResourceUrl<'a>> {
        let (project, sequence) = identifier.rsplit_once('-')?;
        if !valid_issue_identifier(project, sequence) {
            return None;
        }
        Some(self.url(ResourcePath::Issue {
            project,
            identifier,
        }))
    }

    #[must_use]
    pub(crate) fn project_markdown<'a>(&'a self, identifier: &'a str) -> MarkdownReference<'a> {
        MarkdownReference {
            label: LinkLabel::Text(identifier),
            url: self.project_url(identifier),
        }
    }

    #[must_use]
    pub(crate) fn project_url<'a>(&'a self, identifier: &'a str) -> Option<ResourceUrl<'a>> {
        valid_project_identifier(identifier).then(|| self.url(ResourcePath::Project(identifier)))
    }

    #[must_use]
    pub(crate) fn page_markdown<'a>(
        &'a self,
        identifier: &'a str,
        page_id: i64,
    ) -> MarkdownReference<'a> {
        MarkdownReference {
            label: LinkLabel::Text(identifier),
            url: self.page_url(identifier, page_id),
        }
    }

    #[must_use]
    pub(crate) fn page_url<'a>(
        &'a self,
        identifier: &'a str,
        page_id: i64,
    ) -> Option<ResourceUrl<'a>> {
        let project = valid_page_identifier(identifier)?;
        (page_id > 0).then(|| {
            self.url(ResourcePath::Page {
                project,
                id: page_id,
            })
        })
    }

    #[must_use]
    pub(crate) fn plan_markdown<'a>(
        &'a self,
        identifier: &'a str,
        plan_id: i64,
    ) -> MarkdownReference<'a> {
        MarkdownReference {
            label: LinkLabel::Text(identifier),
            url: self.plan_url(identifier, plan_id),
        }
    }

    #[must_use]
    pub(crate) fn plan_url<'a>(
        &'a self,
        identifier: &'a str,
        plan_id: i64,
    ) -> Option<ResourceUrl<'a>> {
        let (project, suffix) = identifier.split_once("-PLAN-")?;
        (valid_project_identifier(project)
            && suffix.chars().all(|c| c.is_ascii_digit())
            && !suffix.is_empty()
            && plan_id > 0)
            .then(|| {
                self.url(ResourcePath::Plan {
                    project,
                    id: plan_id,
                })
            })
    }

    #[must_use]
    pub(crate) fn module_markdown<'a>(
        &'a self,
        project: &'a str,
        module_id: i64,
        label: &'a str,
    ) -> MarkdownReference<'a> {
        MarkdownReference {
            label: LinkLabel::Escaped(label),
            url: self.module_url(project, module_id),
        }
    }

    #[must_use]
    pub(crate) fn module_url<'a>(
        &'a self,
        project: &'a str,
        module_id: i64,
    ) -> Option<ResourceUrl<'a>> {
        (valid_project_identifier(project) && module_id > 0).then(|| {
            self.url(ResourcePath::Module {
                project,
                id: module_id,
            })
        })
    }

    #[must_use]
    pub(crate) fn issue_comment_markdown<'a>(
        &'a self,
        identifier: &'a str,
        comment_id: i64,
    ) -> MarkdownReference<'a> {
        MarkdownReference {
            label: LinkLabel::Comment(comment_id),
            url: self.issue_comment_url(identifier, comment_id),
        }
    }

    #[must_use]
    pub(crate) fn issue_comment_url<'a>(
        &'a self,
        identifier: &'a str,
        comment_id: i64,
    ) -> Option<ResourceUrl<'a>> {
        (comment_id > 0)
            .then(|| self.issue_url(identifier))
            .flatten()
            .map(|url| url.with_comment(comment_id))
    }

    #[must_use]
    pub(crate) fn page_comment_markdown<'a>(
        &'a self,
        identifier: &'a str,
        page_id: i64,
        comment_id: i64,
    ) -> MarkdownReference<'a> {
        MarkdownReference {
            label: LinkLabel::Comment(comment_id),
            url: self.page_comment_url(identifier, page_id, comment_id),
        }
    }

    #[must_use]
    pub(crate) fn page_comment_url<'a>(
        &'a self,
        identifier: &'a str,
        page_id: i64,
        comment_id: i64,
    ) -> Option<ResourceUrl<'a>> {
        (comment_id > 0)
            .then(|| self.page_url(identifier, page_id))
            .flatten()
            .map(|url| url.with_comment(comment_id))
    }

    fn url<'a>(&'a self, path: ResourcePath<'a>) -> ResourceUrl<'a> {
        ResourceUrl {
            base_url: self.base_url.as_str(),
            path,
            comment_id: None,
        }
    }
}

impl ResourceUrl<'_> {
    fn with_comment(mut self, comment_id: i64) -> Self {
        self.comment_id = Some(comment_id);
        self
    }
}

fn valid_base_url(base_url: &Url) -> bool {
    matches!(base_url.scheme(), "http" | "https")
        && base_url.has_authority()
        && base_url.username().is_empty()
        && base_url.password().is_none()
        && base_url.query().is_none()
        && base_url.fragment().is_none()
}

fn valid_issue_identifier(project: &str, sequence: &str) -> bool {
    valid_project_identifier(project)
        && !sequence.is_empty()
        && sequence.chars().all(|c| c.is_ascii_digit())
}

fn valid_page_identifier(identifier: &str) -> Option<&str> {
    let (project, sequence) = identifier
        .strip_prefix("DOC-")
        .map(|sequence| ("DOC", sequence))
        .or_else(|| identifier.split_once("-DOC-"))?;
    ((project == "DOC" || valid_project_identifier(project))
        && !sequence.is_empty()
        && sequence.chars().all(|c| c.is_ascii_digit()))
    .then_some(project)
}

fn valid_project_identifier(project: &str) -> bool {
    project != "DOC"
        && project.len() <= 5
        && project
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase())
        && project
            .chars()
            .skip(1)
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::IssueLinkContext;

    #[test]
    fn issue_markdown_preserves_base_path() {
        for base_url in [
            "https://tracker.example/lific",
            "https://tracker.example/lific/",
        ] {
            let context = IssueLinkContext::parse(base_url).unwrap();
            assert_eq!(
                context.issue_markdown("LIF-42").to_string(),
                "[LIF-42](https://tracker.example/lific/LIF/issues/LIF-42)"
            );
        }
    }

    #[test]
    fn issue_markdown_keeps_malformed_identifiers_plain() {
        let context = IssueLinkContext::parse("https://tracker.example").unwrap();

        for identifier in ["DOC-1", "lif-1", "LIF", "LIF-nope", "TOOLONG-1"] {
            assert_eq!(context.issue_markdown(identifier).to_string(), identifier);
        }
    }

    #[test]
    fn resource_markdown_uses_detail_routes() {
        let context = IssueLinkContext::parse("https://tracker.example/lific").unwrap();

        assert_eq!(
            context.project_markdown("LIF").to_string(),
            "[LIF](https://tracker.example/lific/LIF/overview)"
        );
        assert_eq!(
            context.page_markdown("LIF-DOC-3", 17).to_string(),
            "[LIF-DOC-3](https://tracker.example/lific/LIF/pages/17)"
        );
        assert_eq!(
            context.page_markdown("DOC-3", 18).to_string(),
            "[DOC-3](https://tracker.example/lific/DOC/pages/18)"
        );
        assert_eq!(
            context.plan_markdown("LIF-PLAN-4", 19).to_string(),
            "[LIF-PLAN-4](https://tracker.example/lific/LIF/plans/19)"
        );
        assert_eq!(
            context.module_markdown("LIF", 23, "Backend").to_string(),
            "[Backend](https://tracker.example/lific/LIF/modules/23)"
        );
    }

    #[test]
    fn reserved_doc_identifier_only_links_workspace_pages() {
        let context = IssueLinkContext::parse("https://tracker.example/lific").unwrap();

        assert_eq!(context.project_markdown("DOC").to_string(), "DOC");
        assert_eq!(
            context.plan_markdown("DOC-PLAN-1", 19).to_string(),
            "DOC-PLAN-1"
        );
        assert_eq!(
            context.module_markdown("DOC", 23, "Backend").to_string(),
            "Backend"
        );
        assert_eq!(
            context.page_markdown("DOC-3", 18).to_string(),
            "[DOC-3](https://tracker.example/lific/DOC/pages/18)"
        );
    }

    #[test]
    fn module_markdown_escapes_link_label_delimiters() {
        let context = IssueLinkContext::parse("https://tracker.example/lific").unwrap();

        assert_eq!(
            context
                .module_markdown("LIF", 23, r"API [v2]\stable")
                .to_string(),
            r"[API \[v2\]\\stable](https://tracker.example/lific/LIF/modules/23)"
        );
    }

    #[test]
    fn comment_markdown_points_at_comment_anchor() {
        let context = IssueLinkContext::parse("https://tracker.example/lific").unwrap();

        assert_eq!(
            context.issue_comment_markdown("LIF-42", 7).to_string(),
            "[comment #7](https://tracker.example/lific/LIF/issues/LIF-42#comment-7)"
        );
        assert_eq!(
            context
                .page_comment_markdown("LIF-DOC-3", 17, 8)
                .to_string(),
            "[comment #8](https://tracker.example/lific/LIF/pages/17#comment-8)"
        );
        assert_eq!(
            context.page_comment_markdown("DOC-3", 18, 9).to_string(),
            "[comment #9](https://tracker.example/lific/DOC/pages/18#comment-9)"
        );
    }

    #[test]
    fn parse_rejects_ambiguous_or_credential_bearing_bases() {
        for base_url in [
            "file:///tmp/lific",
            "https://user:password@tracker.example",
            "https://tracker.example?tenant=one",
            "https://tracker.example#fragment",
        ] {
            assert!(IssueLinkContext::parse(base_url).is_none(), "{base_url}");
        }
    }

    #[test]
    fn http_request_origin_prefers_public_url_and_falls_back_to_allowlisted_host() {
        let allowed_hosts = vec!["localhost".into(), "tracker.example".into()];
        let public = IssueLinkContext::for_http_request(
            Some("https://tracker.example/lific"),
            Some("localhost:3456"),
            &allowed_hosts,
        )
        .unwrap();
        assert_eq!(
            public.issue_markdown("LIF-1").to_string(),
            "[LIF-1](https://tracker.example/lific/LIF/issues/LIF-1)"
        );

        let direct =
            IssueLinkContext::for_http_request(None, Some("localhost:3456"), &allowed_hosts)
                .unwrap();
        assert_eq!(
            direct.issue_markdown("LIF-1").to_string(),
            "[LIF-1](http://localhost:3456/LIF/issues/LIF-1)"
        );
        assert!(
            IssueLinkContext::for_http_request(None, Some("spoofed.example"), &allowed_hosts,)
                .is_none()
        );
        assert!(
            IssueLinkContext::for_http_request(None, Some(" localhost:3456 "), &allowed_hosts,)
                .is_none()
        );
        assert!(
            IssueLinkContext::for_http_request(
                None,
                Some("evil.example@localhost:3456"),
                &allowed_hosts,
            )
            .is_none()
        );
    }
}
