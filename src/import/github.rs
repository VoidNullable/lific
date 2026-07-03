//! GitHub Issues importer (LIF-264).
//!
//! Pulls issues from a repo via the REST API (`/repos/{owner}/{repo}/issues`),
//! mapping each to a [`NormalizedIssue`]. Two things the endpoint does that we
//! handle explicitly:
//!
//! - It returns **pull requests too** — every PR is also an "issue" with a
//!   `pull_request` key. We filter those out (counted in the summary).
//! - It **paginates** via the `Link` header, and enforces a 5000 req/hr limit
//!   surfaced in `X-RateLimit-*`. The live fetcher walks pages and backs off
//!   when the remaining budget hits zero.
//!
//! The mapping ([`map_issue`], [`map_status`], [`map_priority`]) is pure and
//! tested against fixture JSON; the network lives behind [`GithubFetcher`].

use serde::Deserialize;

use super::{NormalizedComment, NormalizedIssue, NormalizedLabel, StatusMap};

/// One issue as returned by the GitHub REST API (fields we use).
#[derive(Debug, Clone, Deserialize)]
pub struct GithubIssue {
    pub number: i64,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    /// "open" or "closed".
    pub state: String,
    #[serde(default)]
    pub labels: Vec<GithubLabel>,
    #[serde(default)]
    pub assignees: Vec<GithubUser>,
    #[serde(default)]
    pub milestone: Option<serde_json::Value>,
    /// Present iff this "issue" is actually a pull request.
    #[serde(default)]
    pub pull_request: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GithubLabel {
    pub name: String,
    /// 6-hex without a leading `#`.
    #[serde(default)]
    pub color: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GithubUser {
    pub login: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GithubComment {
    #[serde(default)]
    pub user: Option<GithubUser>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

/// Which issues to fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateFilter {
    Open,
    Closed,
    All,
}

impl StateFilter {
    pub fn parse(s: &str) -> Result<StateFilter, String> {
        match s {
            "open" => Ok(StateFilter::Open),
            "closed" => Ok(StateFilter::Closed),
            "all" => Ok(StateFilter::All),
            other => Err(format!("invalid --state '{other}' (open|closed|all)")),
        }
    }
    pub fn as_query(&self) -> &'static str {
        match self {
            StateFilter::Open => "open",
            StateFilter::Closed => "closed",
            StateFilter::All => "all",
        }
    }
}

/// True if this entry is a pull request masquerading as an issue.
pub fn is_pull_request(issue: &GithubIssue) -> bool {
    issue.pull_request.is_some()
}

/// Map GitHub's `state` ("open"/"closed") to a Lific status via `map`.
pub fn map_status(state: &str, map: &StatusMap) -> String {
    match state {
        "closed" => map.closed.clone(),
        _ => map.open.clone(),
    }
}

/// GitHub has no issue priority; every imported issue is `none`. Kept as a
/// function for symmetry with the other importers and future label-driven
/// priority inference.
pub fn map_priority(_issue: &GithubIssue) -> String {
    "none".to_string()
}

/// Normalize a GitHub label's color: the API returns 6 hex digits with no `#`
/// (e.g. `d73a4a`). Prepend `#`. Empty/malformed → `None` (default color).
fn normalize_color(raw: Option<&str>) -> Option<String> {
    let c = raw?.trim();
    if c.len() == 6 && c.chars().all(|ch| ch.is_ascii_hexdigit()) {
        Some(format!("#{c}"))
    } else {
        None
    }
}

/// Map one GitHub issue + its comments to a [`NormalizedIssue`].
/// `slug` is `owner/name`, used to build the `github:owner/name#N` source.
pub fn map_issue(
    slug: &str,
    issue: &GithubIssue,
    comments: &[GithubComment],
    map: &StatusMap,
) -> NormalizedIssue {
    let labels = issue
        .labels
        .iter()
        .map(|l| NormalizedLabel {
            name: l.name.clone(),
            color: normalize_color(l.color.as_deref()),
        })
        .collect();

    let mapped_comments = comments
        .iter()
        .map(|c| NormalizedComment {
            author: c
                .user
                .as_ref()
                .map(|u| u.login.clone())
                .unwrap_or_else(|| "ghost".to_string()),
            created_at: c.created_at.clone(),
            body: c.body.clone().unwrap_or_default(),
        })
        .collect();

    NormalizedIssue {
        source: format!("github:{slug}#{}", issue.number),
        title: issue.title.clone(),
        description: issue.body.clone().unwrap_or_default(),
        status: map_status(&issue.state, map),
        priority: map_priority(issue),
        labels,
        comments: mapped_comments,
    }
}

/// Abstraction over the network so mapping stays testable. The live impl walks
/// pages; a fake returns canned pages.
pub trait GithubFetcher {
    /// Fetch the issues page. `page` is 1-indexed. Returns `(issues,
    /// has_next_page)`. Only real issues (PRs may be included — the caller
    /// filters).
    fn fetch_issues_page(
        &self,
        page: u32,
        state: StateFilter,
    ) -> Result<(Vec<GithubIssue>, bool), String>;

    /// Fetch all comments for one issue number.
    fn fetch_comments(&self, issue_number: i64) -> Result<Vec<GithubComment>, String>;
}

/// Walk all pages of a [`GithubFetcher`], filtering out PRs, and normalize
/// every issue (fetching its comments). Pure over the fetcher — a fake fetcher
/// drives the whole pipeline in tests with no network.
pub fn collect(
    fetcher: &dyn GithubFetcher,
    slug: &str,
    state: StateFilter,
    map: &StatusMap,
) -> Result<super::FetchedIssues, String> {
    let mut out = super::FetchedIssues::default();
    let mut page = 1u32;
    loop {
        let (issues, has_next) = fetcher.fetch_issues_page(page, state)?;
        for issue in &issues {
            if is_pull_request(issue) {
                out.skipped_non_issues += 1;
                continue;
            }
            out.skipped_assignees += issue.assignees.len();
            if issue.milestone.is_some() {
                out.skipped_other += 1;
            }
            let comments = fetcher.fetch_comments(issue.number)?;
            out.issues.push(map_issue(slug, issue, &comments, map));
        }
        if !has_next {
            break;
        }
        page += 1;
        // Safety valve: GitHub caps at ~10k results; 400 pages of 30 is plenty.
        if page > 400 {
            break;
        }
    }
    Ok(out)
}

/// Parse the RFC 5988 `Link` header to decide whether a next page exists.
/// GitHub returns `<...&page=2>; rel="next", <...&page=9>; rel="last"`.
pub fn has_next_page(link_header: Option<&str>) -> bool {
    match link_header {
        Some(h) => h.split(',').any(|part| part.contains("rel=\"next\"")),
        None => false,
    }
}

/// Live fetcher over the blocking reqwest client already in the tree.
pub struct LiveGithub {
    client: reqwest::blocking::Client,
    owner: String,
    repo: String,
    token: Option<String>,
}

impl LiveGithub {
    pub fn new(owner: &str, repo: &str, token: Option<String>) -> Result<LiveGithub, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("lific-import/1.0")
            .build()
            .map_err(|e| format!("http client init failed: {e}"))?;
        Ok(LiveGithub {
            client,
            owner: owner.to_string(),
            repo: repo.to_string(),
            token,
        })
    }

    fn get(&self, url: &str) -> Result<reqwest::blocking::Response, String> {
        let mut req = self
            .client
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(t) = &self.token {
            req = req.header("Authorization", format!("Bearer {t}"));
        }
        let resp = req.send().map_err(|e| format!("request failed: {e}"))?;
        // Respect the rate-limit budget: if we're out, surface a clear error
        // rather than hammering. GitHub sets X-RateLimit-Remaining: 0 and a
        // reset epoch when throttled (HTTP 403).
        if resp.status().as_u16() == 403 {
            if let Some(rem) = resp
                .headers()
                .get("x-ratelimit-remaining")
                .and_then(|v| v.to_str().ok())
                && rem == "0"
            {
                let reset = resp
                    .headers()
                    .get("x-ratelimit-reset")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("soon");
                return Err(format!(
                    "GitHub rate limit exhausted; resets at epoch {reset}. \
                     Provide a token (--token / GITHUB_TOKEN) for a higher budget."
                ));
            }
            return Err("GitHub returned 403 (forbidden) — check token permissions".into());
        }
        if resp.status().as_u16() == 401 {
            return Err("GitHub authentication failed — check your token".into());
        }
        if resp.status().as_u16() == 404 {
            return Err(format!(
                "repo {}/{} not found (or private and token lacks access)",
                self.owner, self.repo
            ));
        }
        if !resp.status().is_success() {
            return Err(format!("GitHub returned HTTP {}", resp.status()));
        }
        Ok(resp)
    }
}

impl GithubFetcher for LiveGithub {
    fn fetch_issues_page(
        &self,
        page: u32,
        state: StateFilter,
    ) -> Result<(Vec<GithubIssue>, bool), String> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/issues?state={}&per_page=100&page={page}",
            self.owner,
            self.repo,
            state.as_query()
        );
        let resp = self.get(&url)?;
        let link = resp
            .headers()
            .get("link")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let issues: Vec<GithubIssue> = resp
            .json()
            .map_err(|e| format!("failed to parse issues JSON: {e}"))?;
        Ok((issues, has_next_page(link.as_deref())))
    }

    fn fetch_comments(&self, issue_number: i64) -> Result<Vec<GithubComment>, String> {
        let mut all = Vec::new();
        let mut page = 1u32;
        loop {
            let url = format!(
                "https://api.github.com/repos/{}/{}/issues/{issue_number}/comments?per_page=100&page={page}",
                self.owner, self.repo
            );
            let resp = self.get(&url)?;
            let link = resp
                .headers()
                .get("link")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let batch: Vec<GithubComment> = resp
                .json()
                .map_err(|e| format!("failed to parse comments JSON: {e}"))?;
            all.extend(batch);
            if !has_next_page(link.as_deref()) {
                break;
            }
            page += 1;
            if page > 100 {
                break;
            }
        }
        Ok(all)
    }
}

/// Split an `owner/name` slug, validating shape.
pub fn parse_repo(slug: &str) -> Result<(String, String), String> {
    let parts: Vec<&str> = slug.splitn(2, '/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(format!("invalid --repo '{slug}', expected owner/name"));
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("fixtures/github_issues.json");

    fn fixture_issues() -> Vec<GithubIssue> {
        serde_json::from_str(FIXTURE).unwrap()
    }

    #[test]
    fn parse_repo_valid_and_invalid() {
        assert_eq!(
            parse_repo("octocat/hello").unwrap(),
            ("octocat".into(), "hello".into())
        );
        assert!(parse_repo("noslash").is_err());
        assert!(parse_repo("/name").is_err());
        assert!(parse_repo("owner/").is_err());
    }

    #[test]
    fn pull_requests_are_detected_and_filtered() {
        let issues = fixture_issues();
        let prs: Vec<_> = issues.iter().filter(|i| is_pull_request(i)).collect();
        assert_eq!(prs.len(), 1, "fixture has exactly one PR");
        assert_eq!(prs[0].number, 102);
    }

    #[test]
    fn status_mapping_default_and_custom() {
        let d = StatusMap::default();
        assert_eq!(map_status("open", &d), "backlog");
        assert_eq!(map_status("closed", &d), "done");
        let custom = StatusMap {
            open: "todo".into(),
            closed: "cancelled".into(),
        };
        assert_eq!(map_status("open", &custom), "todo");
        assert_eq!(map_status("closed", &custom), "cancelled");
    }

    #[test]
    fn color_normalization() {
        assert_eq!(normalize_color(Some("d73a4a")).as_deref(), Some("#d73a4a"));
        assert_eq!(normalize_color(Some("")), None);
        assert_eq!(normalize_color(Some("notahex")), None);
        assert_eq!(normalize_color(None), None);
    }

    #[test]
    fn map_issue_produces_source_and_labels() {
        let issues = fixture_issues();
        let open = issues.iter().find(|i| i.number == 100).unwrap();
        let mapped = map_issue("octocat/hello", open, &[], &StatusMap::default());
        assert_eq!(mapped.source, "github:octocat/hello#100");
        assert_eq!(mapped.status, "backlog");
        assert_eq!(mapped.title, "Bug: crash on startup");
        assert!(mapped.description.contains("crashes"));
        // Two labels, one with a color.
        assert_eq!(mapped.labels.len(), 2);
        let bug = mapped.labels.iter().find(|l| l.name == "bug").unwrap();
        assert_eq!(bug.color.as_deref(), Some("#d73a4a"));
    }

    #[test]
    fn map_closed_issue() {
        let issues = fixture_issues();
        let closed = issues.iter().find(|i| i.number == 101).unwrap();
        let mapped = map_issue("octocat/hello", closed, &[], &StatusMap::default());
        assert_eq!(mapped.status, "done");
    }

    #[test]
    fn link_header_next_detection() {
        assert!(has_next_page(Some(
            "<https://api.github.com/x?page=2>; rel=\"next\", <https://api.github.com/x?page=9>; rel=\"last\""
        )));
        assert!(!has_next_page(Some(
            "<https://api.github.com/x?page=1>; rel=\"prev\""
        )));
        assert!(!has_next_page(None));
    }

    // A fake fetcher driving the full pagination + PR-filter pipeline offline.
    struct FakeGithub {
        pages: Vec<Vec<GithubIssue>>,
        comments: Vec<GithubComment>,
    }
    impl GithubFetcher for FakeGithub {
        fn fetch_issues_page(
            &self,
            page: u32,
            _state: StateFilter,
        ) -> Result<(Vec<GithubIssue>, bool), String> {
            let idx = (page - 1) as usize;
            let issues = self.pages.get(idx).cloned().unwrap_or_default();
            let has_next = idx + 1 < self.pages.len();
            Ok((issues, has_next))
        }
        fn fetch_comments(&self, _n: i64) -> Result<Vec<GithubComment>, String> {
            Ok(self.comments.clone())
        }
    }

    #[test]
    fn collect_walks_pages_and_filters_prs() {
        let all = fixture_issues();
        // Split fixture across two pages to exercise pagination.
        let (p1, p2) = all.split_at(2);
        let fetcher = FakeGithub {
            pages: vec![p1.to_vec(), p2.to_vec()],
            comments: vec![GithubComment {
                user: Some(GithubUser {
                    login: "octocat".into(),
                }),
                body: Some("a comment".into()),
                created_at: Some("2024-01-01T00:00:00Z".into()),
            }],
        };
        let fetched =
            collect(&fetcher, "octocat/hello", StateFilter::All, &StatusMap::default()).unwrap();
        // Fixture: 3 real issues + 1 PR.
        assert_eq!(fetched.issues.len(), 3);
        assert_eq!(fetched.skipped_non_issues, 1);
        // Each real issue got one comment from the fake.
        assert!(fetched.issues.iter().all(|i| i.comments.len() == 1));
        assert_eq!(fetched.issues[0].comments[0].author, "octocat");
    }
}
