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
        && project.len() <= 5
        && project
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase())
        && project
            .chars()
            .skip(1)
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        && !sequence.is_empty()
        && sequence.chars().all(|c| c.is_ascii_digit())
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
