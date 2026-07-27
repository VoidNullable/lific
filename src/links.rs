use reqwest::Url;

#[derive(Debug, Clone)]
pub(crate) struct IssueLinkContext {
    base_url: Url,
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
                let host_header = host_header?.trim();
                let authority = host_header.parse::<axum::http::uri::Authority>().ok()?;
                if authority.as_str() != host_header {
                    return None;
                }
                let host = authority
                    .host()
                    .trim_start_matches('[')
                    .trim_end_matches(']');
                allowed_hosts
                    .iter()
                    .any(|allowed| allowed.eq_ignore_ascii_case(host))
                    .then(|| Self::parse(&format!("http://{host_header}")))
                    .flatten()
            }
        }
    }

    #[must_use]
    pub(crate) fn issue_markdown(&self, identifier: &str) -> String {
        self.issue_url(identifier).map_or_else(
            || identifier.to_owned(),
            |url| format!("[{identifier}]({url})"),
        )
    }

    #[must_use]
    pub(crate) fn issue_url(&self, identifier: &str) -> Option<String> {
        let (project, sequence) = identifier.rsplit_once('-')?;
        if !valid_issue_identifier(project, sequence) {
            return None;
        }
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .ok()?
            .push(project)
            .push("issues")
            .push(identifier);
        Some(url.to_string())
    }

    #[must_use]
    pub(crate) fn project_markdown(&self, identifier: &str) -> String {
        self.project_url(identifier).map_or_else(
            || identifier.to_owned(),
            |url| format!("[{identifier}]({url})"),
        )
    }

    #[must_use]
    pub(crate) fn project_url(&self, identifier: &str) -> Option<String> {
        valid_project_identifier(identifier).then(|| self.path_url([identifier, "overview"]))
    }

    #[must_use]
    pub(crate) fn page_markdown(&self, identifier: &str, page_id: i64) -> String {
        self.page_url(identifier, page_id).map_or_else(
            || identifier.to_owned(),
            |url| format!("[{identifier}]({url})"),
        )
    }

    #[must_use]
    pub(crate) fn page_url(&self, identifier: &str, page_id: i64) -> Option<String> {
        let (project, sequence) = identifier.split_once("-DOC-")?;
        (valid_project_identifier(project)
            && sequence.chars().all(|c| c.is_ascii_digit())
            && !sequence.is_empty()
            && page_id > 0)
            .then(|| self.path_url([project, "pages", &page_id.to_string()]))
    }

    #[must_use]
    pub(crate) fn plan_markdown(&self, identifier: &str, plan_id: i64) -> String {
        self.plan_url(identifier, plan_id).map_or_else(
            || identifier.to_owned(),
            |url| format!("[{identifier}]({url})"),
        )
    }

    #[must_use]
    pub(crate) fn plan_url(&self, identifier: &str, plan_id: i64) -> Option<String> {
        let (project, suffix) = identifier.split_once("-PLAN-")?;
        (valid_project_identifier(project)
            && suffix.chars().all(|c| c.is_ascii_digit())
            && !suffix.is_empty()
            && plan_id > 0)
            .then(|| self.path_url([project, "plans", &plan_id.to_string()]))
    }

    #[must_use]
    pub(crate) fn module_markdown(&self, project: &str, module_id: i64, label: &str) -> String {
        self.module_url(project, module_id).map_or_else(
            || label.to_owned(),
            |url| format!("[{label}]({url})"),
        )
    }

    #[must_use]
    pub(crate) fn module_url(&self, project: &str, module_id: i64) -> Option<String> {
        (valid_project_identifier(project) && module_id > 0)
            .then(|| self.path_url([project, "modules", &module_id.to_string()]))
    }

    #[must_use]
    pub(crate) fn issue_comment_markdown(
        &self,
        identifier: &str,
        comment_id: i64,
    ) -> String {
        self.issue_comment_url(identifier, comment_id).map_or_else(
            || format!("comment #{comment_id}"),
            |url| format!("[comment #{comment_id}]({url})"),
        )
    }

    #[must_use]
    pub(crate) fn issue_comment_url(&self, identifier: &str, comment_id: i64) -> Option<String> {
        if comment_id <= 0 {
            return None;
        }
        Some(format!(
            "{}#comment-{comment_id}",
            self.issue_url(identifier)?
        ))
    }

    #[must_use]
    pub(crate) fn page_comment_markdown(
        &self,
        identifier: &str,
        page_id: i64,
        comment_id: i64,
    ) -> String {
        self.page_comment_url(identifier, page_id, comment_id)
            .map_or_else(
                || format!("comment #{comment_id}"),
                |url| format!("[comment #{comment_id}]({url})"),
            )
    }

    #[must_use]
    pub(crate) fn page_comment_url(
        &self,
        identifier: &str,
        page_id: i64,
        comment_id: i64,
    ) -> Option<String> {
        if comment_id <= 0 {
            return None;
        }
        Some(format!(
            "{}#comment-{comment_id}",
            self.page_url(identifier, page_id)?
        ))
    }

    fn path_url<const N: usize>(&self, segments: [&str; N]) -> String {
        let mut url = self.base_url.clone();
        if let Ok(mut path) = url.path_segments_mut() {
            path.extend(segments);
        }
        url.to_string()
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
    project != "DOC"
        && valid_project_identifier(project)
        && !sequence.is_empty()
        && sequence.chars().all(|c| c.is_ascii_digit())
}

fn valid_project_identifier(project: &str) -> bool {
    project.len() <= 5
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
        let context = IssueLinkContext::parse("https://tracker.example/lific").unwrap();

        assert_eq!(
            context.issue_markdown("LIF-42"),
            "[LIF-42](https://tracker.example/lific/LIF/issues/LIF-42)"
        );
    }

    #[test]
    fn issue_markdown_keeps_malformed_identifiers_plain() {
        let context = IssueLinkContext::parse("https://tracker.example").unwrap();

        for identifier in ["DOC-1", "lif-1", "LIF", "LIF-nope", "TOOLONG-1"] {
            assert_eq!(context.issue_markdown(identifier), identifier);
        }
    }

    #[test]
    fn resource_markdown_uses_detail_routes() {
        let context = IssueLinkContext::parse("https://tracker.example/lific").unwrap();

        assert_eq!(
            context.project_markdown("LIF"),
            "[LIF](https://tracker.example/lific/LIF/overview)"
        );
        assert_eq!(
            context.page_markdown("LIF-DOC-3", 17),
            "[LIF-DOC-3](https://tracker.example/lific/LIF/pages/17)"
        );
        assert_eq!(
            context.plan_markdown("LIF-PLAN-4", 19),
            "[LIF-PLAN-4](https://tracker.example/lific/LIF/plans/19)"
        );
        assert_eq!(
            context.module_markdown("LIF", 23, "Backend"),
            "[Backend](https://tracker.example/lific/LIF/modules/23)"
        );
    }

    #[test]
    fn comment_markdown_points_at_comment_anchor() {
        let context = IssueLinkContext::parse("https://tracker.example/lific").unwrap();

        assert_eq!(
            context.issue_comment_markdown("LIF-42", 7),
            "[comment #7](https://tracker.example/lific/LIF/issues/LIF-42#comment-7)"
        );
        assert_eq!(
            context.page_comment_markdown("LIF-DOC-3", 17, 8),
            "[comment #8](https://tracker.example/lific/LIF/pages/17#comment-8)"
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
            public.issue_markdown("LIF-1"),
            "[LIF-1](https://tracker.example/lific/LIF/issues/LIF-1)"
        );

        let direct =
            IssueLinkContext::for_http_request(None, Some("localhost:3456"), &allowed_hosts)
                .unwrap();
        assert_eq!(
            direct.issue_markdown("LIF-1"),
            "[LIF-1](http://localhost:3456/LIF/issues/LIF-1)"
        );
        assert!(
            IssueLinkContext::for_http_request(None, Some("spoofed.example"), &allowed_hosts,)
                .is_none()
        );
    }
}
