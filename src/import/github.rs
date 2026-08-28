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
use serde::de::DeserializeOwned;
use std::io::Read;

use super::{NormalizedComment, NormalizedIssue, NormalizedLabel, StatusMap};
use crate::db::models::{Priority, Status};

pub const MAX_GITHUB_ISSUES: usize = 10_000;
pub const MAX_GITHUB_COMMENTS_PER_ISSUE: usize = 1_000;
pub const MAX_GITHUB_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_GITHUB_NORMALIZED_BYTES: usize = 128 * 1024 * 1024;

/// Which deliberate ceiling an import ran into.
///
/// Each variant carries the numbers that produced it rather than a
/// pre-rendered sentence, so callers (and tests) can branch on *which* limit
/// fired without pattern-matching prose. `Display` renders the operator-facing
/// message, which is the only place the wording lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GithubLimit {
    /// The repository has more issues than Lific will import.
    Issues { max: usize },
    /// One issue carries more comments than Lific will import.
    CommentsPerIssue { issue_number: i64, max: usize },
    /// A single GitHub response body was larger than we will buffer.
    ResponseBytes { label: &'static str, max: usize },
    /// A single page returned more items than GitHub's own `per_page` allows.
    PageItems { label: &'static str, max: usize },
    /// The normalized issues retained in memory outgrew the import budget.
    NormalizedBytes { max: usize },
}

impl std::fmt::Display for GithubLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GithubLimit::Issues { max } => {
                write!(f, "GitHub repository has too many issues (more than {max})")
            }
            GithubLimit::CommentsPerIssue { issue_number, max } => write!(
                f,
                "GitHub issue #{issue_number} has too many comments (more than {max})"
            ),
            GithubLimit::ResponseBytes { label, max } => {
                write!(f, "GitHub {label} response exceeds the {max} byte limit")
            }
            GithubLimit::PageItems { label, max } => write!(
                f,
                "GitHub {label} response exceeds the per-page limit of {max} items"
            ),
            GithubLimit::NormalizedBytes { max } => write!(
                f,
                "GitHub import normalized data exceeds the {max} byte limit"
            ),
        }
    }
}

/// Why a GitHub import stopped.
///
/// The split exists so the HTTP layer can answer correctly. A [`Limit`] is a
/// deliberate refusal: the repository is bigger than Lific will import, the
/// caller asked for something we will not do, and no amount of retrying
/// changes that. Collapsing it into a `String` meant the import endpoint
/// answered `500 internal server error`, which reads as a Lific bug and hides
/// the one fact the operator needs. [`Upstream`] (GitHub or the network) and
/// [`Internal`] (an allocation or arithmetic failure on our side) stay
/// server-side faults.
///
/// [`Limit`]: GithubImportError::Limit
/// [`Upstream`]: GithubImportError::Upstream
/// [`Internal`]: GithubImportError::Internal
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GithubImportError {
    /// A resource ceiling was hit on purpose. Client-facing, actionable.
    #[error("{0}")]
    Limit(GithubLimit),
    /// GitHub was unreachable, refused us, or sent something unparseable.
    #[error("{0}")]
    Upstream(String),
    /// Lific-side failure while assembling the import.
    #[error("{0}")]
    Internal(String),
}

impl GithubImportError {
    /// The limit this error reports, if it reports one.
    pub fn limit(&self) -> Option<GithubLimit> {
        match self {
            GithubImportError::Limit(limit) => Some(*limit),
            _ => None,
        }
    }
}

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
pub fn map_status(state: &str, map: &StatusMap) -> Status {
    match state {
        "closed" => map.closed,
        _ => map.open,
    }
}

/// GitHub has no issue priority; every imported issue is `none`. Kept as a
/// function for symmetry with the other importers and future label-driven
/// priority inference.
pub fn map_priority(_issue: &GithubIssue) -> Priority {
    Priority::None
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
    ) -> Result<(Vec<GithubIssue>, bool), GithubImportError>;

    /// Fetch all comments for one issue number.
    fn fetch_comments(&self, issue_number: i64) -> Result<Vec<GithubComment>, GithubImportError>;
}

/// Walk all pages of a [`GithubFetcher`], filtering out PRs, and normalize
/// every issue (fetching its comments). Pure over the fetcher — a fake fetcher
/// drives the whole pipeline in tests with no network.
pub fn collect(
    fetcher: &dyn GithubFetcher,
    slug: &str,
    state: StateFilter,
    map: &StatusMap,
) -> Result<super::FetchedIssues, GithubImportError> {
    let mut out = super::FetchedIssues::default();
    let mut total_normalized_bytes = 0usize;
    let mut page = 1u32;
    loop {
        let (issues, has_next) = fetcher.fetch_issues_page(page, state)?;
        for issue in &issues {
            if is_pull_request(issue) {
                out.skipped_non_issues += 1;
                continue;
            }
            if out.issues.len() >= MAX_GITHUB_ISSUES {
                return Err(GithubImportError::Limit(GithubLimit::Issues {
                    max: MAX_GITHUB_ISSUES,
                }));
            }
            out.skipped_assignees += issue.assignees.len();
            if issue.milestone.is_some() {
                out.skipped_other += 1;
            }
            let comments = fetcher.fetch_comments(issue.number)?;
            if comments.len() > MAX_GITHUB_COMMENTS_PER_ISSUE {
                return Err(GithubImportError::Limit(GithubLimit::CommentsPerIssue {
                    issue_number: issue.number,
                    max: MAX_GITHUB_COMMENTS_PER_ISSUE,
                }));
            }
            let normalized = map_issue(slug, issue, &comments, map);
            let previous_capacity = out.issues.capacity();
            out.issues.try_reserve(1).map_err(|_| {
                GithubImportError::Internal(
                    "GitHub import normalized allocation failed".to_string(),
                )
            })?;
            let issue_slots = out
                .issues
                .capacity()
                .saturating_sub(previous_capacity)
                .saturating_mul(std::mem::size_of::<NormalizedIssue>());
            total_normalized_bytes = total_normalized_bytes
                .checked_add(issue_slots)
                .and_then(|bytes| bytes.checked_add(normalized_retained_bytes(&normalized)))
                .ok_or_else(|| {
                    GithubImportError::Internal("GitHub import content size overflow".to_string())
                })?;
            if total_normalized_bytes > MAX_GITHUB_NORMALIZED_BYTES {
                return Err(GithubImportError::Limit(GithubLimit::NormalizedBytes {
                    max: MAX_GITHUB_NORMALIZED_BYTES,
                }));
            }
            out.issues.push(normalized);
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

fn normalized_retained_bytes(issue: &NormalizedIssue) -> usize {
    let label_bytes = issue.labels.iter().fold(
        issue
            .labels
            .capacity()
            .saturating_mul(std::mem::size_of::<NormalizedLabel>()),
        |bytes, label| {
            bytes
                .saturating_add(label.name.capacity())
                .saturating_add(label.color.as_ref().map_or(0, String::capacity))
        },
    );
    let comment_bytes = issue.comments.iter().fold(
        issue
            .comments
            .capacity()
            .saturating_mul(std::mem::size_of::<NormalizedComment>()),
        |bytes, comment| {
            bytes
                .saturating_add(comment.author.capacity())
                .saturating_add(comment.body.capacity())
                .saturating_add(comment.created_at.as_ref().map_or(0, String::capacity))
        },
    );

    issue
        .source
        .capacity()
        .saturating_add(issue.title.capacity())
        .saturating_add(issue.description.capacity())
        .saturating_add(label_bytes)
        .saturating_add(comment_bytes)
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
    pub fn new(
        owner: &str,
        repo: &str,
        token: Option<String>,
    ) -> Result<LiveGithub, GithubImportError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("lific-import/1.0")
            .build()
            .map_err(|e| GithubImportError::Internal(format!("http client init failed: {e}")))?;
        Ok(LiveGithub {
            client,
            owner: owner.to_string(),
            repo: repo.to_string(),
            token,
        })
    }

    fn get(&self, url: &str) -> Result<reqwest::blocking::Response, GithubImportError> {
        let mut req = self
            .client
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(t) = &self.token {
            req = req.header("Authorization", format!("Bearer {t}"));
        }
        let resp = req
            .send()
            .map_err(|e| GithubImportError::Upstream(format!("request failed: {e}")))?;
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
                return Err(GithubImportError::Upstream(format!(
                    "GitHub rate limit exhausted; resets at epoch {reset}. \
                     Provide a token (--token / GITHUB_TOKEN) for a higher budget."
                )));
            }
            return Err(GithubImportError::Upstream(
                "GitHub returned 403 (forbidden) — check token permissions".into(),
            ));
        }
        if resp.status().as_u16() == 401 {
            return Err(GithubImportError::Upstream(
                "GitHub authentication failed — check your token".into(),
            ));
        }
        if resp.status().as_u16() == 404 {
            return Err(GithubImportError::Upstream(format!(
                "repo {}/{} not found (or private and token lacks access)",
                self.owner, self.repo
            )));
        }
        if !resp.status().is_success() {
            return Err(GithubImportError::Upstream(format!(
                "GitHub returned HTTP {}",
                resp.status()
            )));
        }
        Ok(resp)
    }

    fn json_bounded<T: DeserializeOwned>(
        resp: reqwest::blocking::Response,
        label: &'static str,
    ) -> Result<T, GithubImportError> {
        let mut bytes = Vec::new();
        let mut reader = resp.take((MAX_GITHUB_RESPONSE_BYTES + 1) as u64);
        reader.read_to_end(&mut bytes).map_err(|e| {
            GithubImportError::Upstream(format!("failed to read GitHub {label} response: {e}"))
        })?;
        if bytes.len() > MAX_GITHUB_RESPONSE_BYTES {
            return Err(GithubImportError::Limit(GithubLimit::ResponseBytes {
                label,
                max: MAX_GITHUB_RESPONSE_BYTES,
            }));
        }
        serde_json::from_slice(&bytes).map_err(|e| {
            GithubImportError::Upstream(format!("failed to parse GitHub {label} JSON: {e}"))
        })
    }
}

impl GithubFetcher for LiveGithub {
    fn fetch_issues_page(
        &self,
        page: u32,
        state: StateFilter,
    ) -> Result<(Vec<GithubIssue>, bool), GithubImportError> {
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
        let issues: Vec<GithubIssue> = Self::json_bounded(resp, "issues")?;
        if issues.len() > 100 {
            return Err(GithubImportError::Limit(GithubLimit::PageItems {
                label: "issues",
                max: 100,
            }));
        }
        Ok((issues, has_next_page(link.as_deref())))
    }

    fn fetch_comments(&self, issue_number: i64) -> Result<Vec<GithubComment>, GithubImportError> {
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
            let batch: Vec<GithubComment> = Self::json_bounded(resp, "comments")?;
            if batch.len() > 100 {
                return Err(GithubImportError::Limit(GithubLimit::PageItems {
                    label: "comments",
                    max: 100,
                }));
            }
            if all.len() + batch.len() > MAX_GITHUB_COMMENTS_PER_ISSUE {
                return Err(GithubImportError::Limit(GithubLimit::CommentsPerIssue {
                    issue_number,
                    max: MAX_GITHUB_COMMENTS_PER_ISSUE,
                }));
            }
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
        assert_eq!(map_status("open", &d), Status::Backlog);
        assert_eq!(map_status("closed", &d), Status::Done);
        let custom = StatusMap {
            open: Status::Todo,
            closed: Status::Cancelled,
        };
        assert_eq!(map_status("open", &custom), Status::Todo);
        assert_eq!(map_status("closed", &custom), Status::Cancelled);
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
        assert_eq!(mapped.status, Status::Backlog);
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
        assert_eq!(mapped.status, Status::Done);
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
        comment_fetches: std::cell::Cell<usize>,
        /// When set, `fetch_comments` fails with this instead of returning
        /// `comments`, standing in for GitHub or the network dropping out.
        comment_failure: Option<GithubImportError>,
    }

    impl FakeGithub {
        fn new(pages: Vec<Vec<GithubIssue>>, comments: Vec<GithubComment>) -> FakeGithub {
            FakeGithub {
                pages,
                comments,
                comment_fetches: std::cell::Cell::new(0),
                comment_failure: None,
            }
        }
    }

    impl GithubFetcher for FakeGithub {
        fn fetch_issues_page(
            &self,
            page: u32,
            _state: StateFilter,
        ) -> Result<(Vec<GithubIssue>, bool), GithubImportError> {
            let idx = (page - 1) as usize;
            let issues = self.pages.get(idx).cloned().unwrap_or_default();
            let has_next = idx + 1 < self.pages.len();
            Ok((issues, has_next))
        }
        fn fetch_comments(&self, _n: i64) -> Result<Vec<GithubComment>, GithubImportError> {
            self.comment_fetches.set(self.comment_fetches.get() + 1);
            match &self.comment_failure {
                Some(error) => Err(error.clone()),
                None => Ok(self.comments.clone()),
            }
        }
    }

    fn collect_with(
        fetcher: &FakeGithub,
    ) -> Result<super::super::FetchedIssues, GithubImportError> {
        collect(
            fetcher,
            "octocat/hello",
            StateFilter::All,
            &StatusMap::default(),
        )
    }

    #[test]
    fn collect_walks_pages_and_filters_prs() {
        let all = fixture_issues();
        // Split fixture across two pages to exercise pagination.
        let (p1, p2) = all.split_at(2);
        let fetcher = FakeGithub::new(
            vec![p1.to_vec(), p2.to_vec()],
            vec![GithubComment {
                user: Some(GithubUser {
                    login: "octocat".into(),
                }),
                body: Some("a comment".into()),
                created_at: Some("2024-01-01T00:00:00Z".into()),
            }],
        );
        let fetched = collect_with(&fetcher).unwrap();
        // Fixture: 3 real issues + 1 PR.
        assert_eq!(fetched.issues.len(), 3);
        assert_eq!(fetched.skipped_non_issues, 1);
        // Each real issue got one comment from the fake.
        assert!(fetched.issues.iter().all(|i| i.comments.len() == 1));
        assert_eq!(fetched.issues[0].comments[0].author, "octocat");
    }

    #[test]
    fn collect_rejects_comment_blowup() {
        let issue = fixture_issues().into_iter().next().unwrap();
        let number = issue.number;
        let fetcher = FakeGithub::new(
            vec![vec![issue]],
            vec![
                GithubComment {
                    user: None,
                    body: Some("x".into()),
                    created_at: None,
                };
                MAX_GITHUB_COMMENTS_PER_ISSUE + 1
            ],
        );
        let error = collect_with(&fetcher).unwrap_err();
        assert_eq!(
            error.limit(),
            Some(GithubLimit::CommentsPerIssue {
                issue_number: number,
                max: MAX_GITHUB_COMMENTS_PER_ISSUE,
            })
        );
    }

    #[test]
    fn collect_rejects_issue_limit_before_fetching_comments() {
        let issue = fixture_issues().into_iter().next().unwrap();
        let fetcher = FakeGithub::new(vec![vec![issue; MAX_GITHUB_ISSUES + 1]], Vec::new());
        let error = collect_with(&fetcher).unwrap_err();

        assert_eq!(
            error.limit(),
            Some(GithubLimit::Issues {
                max: MAX_GITHUB_ISSUES
            })
        );
        assert_eq!(fetcher.comment_fetches.get(), MAX_GITHUB_ISSUES);
    }

    /// The normalized-size ceiling is about retained memory, not issue count,
    /// so it has to fire on a handful of very large issues too.
    #[test]
    fn collect_rejects_normalized_size_blowup() {
        let mut issue = fixture_issues().into_iter().next().unwrap();
        // ~4 MiB of description each; 32 of them clear the 128 MiB budget
        // while staying far under MAX_GITHUB_ISSUES.
        issue.body = Some("x".repeat(4 * 1024 * 1024));
        let fetcher = FakeGithub::new(vec![vec![issue; 64]], Vec::new());

        let error = collect_with(&fetcher).unwrap_err();
        assert_eq!(
            error.limit(),
            Some(GithubLimit::NormalizedBytes {
                max: MAX_GITHUB_NORMALIZED_BYTES
            })
        );
    }

    /// A limit is a refusal we chose; GitHub failing is not. They must not
    /// collapse into the same variant, because the HTTP layer answers them
    /// with different statuses.
    #[test]
    fn upstream_failures_are_not_limits() {
        let issue = fixture_issues().into_iter().next().unwrap();
        let mut fetcher = FakeGithub::new(vec![vec![issue]], Vec::new());
        fetcher.comment_failure = Some(GithubImportError::Upstream(
            "request failed: connection reset".into(),
        ));

        let error = collect_with(&fetcher).unwrap_err();
        assert_eq!(
            error.limit(),
            None,
            "a network drop is not a resource limit"
        );
        assert!(matches!(error, GithubImportError::Upstream(_)));
    }

    /// `Display` is the only place the operator-facing wording lives; keep it
    /// pointing at the number that triggered the refusal.
    #[test]
    fn limits_render_the_ceiling_they_hit() {
        assert_eq!(
            GithubLimit::Issues { max: 10 }.to_string(),
            "GitHub repository has too many issues (more than 10)"
        );
        assert_eq!(
            GithubLimit::CommentsPerIssue {
                issue_number: 42,
                max: 7
            }
            .to_string(),
            "GitHub issue #42 has too many comments (more than 7)"
        );
        assert_eq!(
            GithubLimit::NormalizedBytes { max: 128 }.to_string(),
            "GitHub import normalized data exceeds the 128 byte limit"
        );
        assert_eq!(
            GithubLimit::ResponseBytes {
                label: "comments",
                max: 8
            }
            .to_string(),
            "GitHub comments response exceeds the 8 byte limit"
        );
        assert_eq!(
            GithubLimit::PageItems {
                label: "issues",
                max: 100
            }
            .to_string(),
            "GitHub issues response exceeds the per-page limit of 100 items"
        );
    }

    #[test]
    fn normalized_size_counts_empty_comment_allocations() {
        let issue = fixture_issues().into_iter().next().unwrap();
        let comments = vec![
            GithubComment {
                user: None,
                body: None,
                created_at: None,
            };
            MAX_GITHUB_COMMENTS_PER_ISSUE
        ];
        let normalized = map_issue("octocat/hello", &issue, &comments, &StatusMap::default());

        assert!(
            normalized_retained_bytes(&normalized)
                >= comments.len() * std::mem::size_of::<super::super::NormalizedComment>()
        );
    }
}
