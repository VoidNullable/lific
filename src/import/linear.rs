//! Linear importer (LIF-265).
//!
//! Linear's API is GraphQL (`https://api.linear.app/graphql`), authenticated by
//! a personal API key in the `Authorization` header. We query a team's issues
//! with nested label + comment connections, paginating by cursor
//! (`first`/`after` + `pageInfo{hasNextPage endCursor}`), ~1500 req/hr.
//!
//! Descriptions and comments are already markdown — imported verbatim. The
//! mapping ([`map_issue`], [`map_state_type`], [`map_priority`]) is pure and
//! fixture-tested; the network lives behind [`LinearFetcher`].

use serde::Deserialize;

use super::{NormalizedComment, NormalizedIssue, NormalizedLabel};

/// The GraphQL response shape we deserialize (a subset of Linear's schema).
#[derive(Debug, Clone, Deserialize)]
pub struct LinearIssue {
    /// Human identifier like `ENG-123` — used for the source marker.
    pub identifier: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    /// 0..4 — Linear's priority scale.
    #[serde(default)]
    pub priority: f64,
    #[serde(default)]
    pub state: Option<LinearState>,
    #[serde(default)]
    pub assignee: Option<LinearUser>,
    #[serde(default)]
    pub estimate: Option<f64>,
    #[serde(default)]
    pub labels: LinearLabelConnection,
    #[serde(default)]
    pub comments: LinearCommentConnection,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LinearState {
    /// backlog / triage / unstarted / started / completed / canceled
    #[serde(rename = "type")]
    pub type_: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LinearUser {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, rename = "displayName")]
    pub display_name: Option<String>,
}

impl LinearUser {
    fn handle(&self) -> String {
        self.display_name
            .clone()
            .or_else(|| self.name.clone())
            .unwrap_or_else(|| "unknown".to_string())
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LinearLabelConnection {
    #[serde(default)]
    pub nodes: Vec<LinearLabel>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LinearLabel {
    pub name: String,
    #[serde(default)]
    pub color: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LinearCommentConnection {
    #[serde(default)]
    pub nodes: Vec<LinearComment>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LinearComment {
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default, rename = "createdAt")]
    pub created_at: Option<String>,
    #[serde(default)]
    pub user: Option<LinearUser>,
}

/// Configurable state.type → Lific status mapping.
#[derive(Debug, Clone)]
pub struct LinearStatusMap {
    pub backlog: String,
    pub unstarted: String,
    pub started: String,
    pub completed: String,
    pub canceled: String,
}

impl Default for LinearStatusMap {
    fn default() -> Self {
        LinearStatusMap {
            backlog: "backlog".into(),
            unstarted: "todo".into(),
            started: "active".into(),
            completed: "done".into(),
            canceled: "cancelled".into(),
        }
    }
}

/// Map Linear's `state.type` to a Lific status. `backlog`/`triage` → backlog,
/// `unstarted` → todo, `started` → active, `completed` → done, `canceled` →
/// cancelled. Unknown types fall back to backlog.
pub fn map_state_type(type_: &str, map: &LinearStatusMap) -> String {
    match type_ {
        "backlog" | "triage" => map.backlog.clone(),
        "unstarted" => map.unstarted.clone(),
        "started" => map.started.clone(),
        "completed" => map.completed.clone(),
        "canceled" | "cancelled" => map.canceled.clone(),
        _ => map.backlog.clone(),
    }
}

/// Map Linear's numeric priority (0 none, 1 urgent, 2 high, 3 medium, 4 low) to
/// a Lific priority string.
pub fn map_priority(priority: f64) -> String {
    match priority.round() as i64 {
        1 => "urgent",
        2 => "high",
        3 => "medium",
        4 => "low",
        _ => "none",
    }
    .to_string()
}

/// Map one Linear issue to a [`NormalizedIssue`]. `team` is the team key used
/// for the `linear:TEAM-123` source marker; Linear's `identifier` already
/// carries it, so we trust `identifier` directly.
pub fn map_issue(issue: &LinearIssue, map: &LinearStatusMap) -> NormalizedIssue {
    let status = issue
        .state
        .as_ref()
        .map(|s| map_state_type(&s.type_, map))
        .unwrap_or_else(|| map.backlog.clone());

    let labels = issue
        .labels
        .nodes
        .iter()
        .map(|l| NormalizedLabel {
            name: l.name.clone(),
            color: l.color.clone(),
        })
        .collect();

    let comments = issue
        .comments
        .nodes
        .iter()
        .map(|c| NormalizedComment {
            author: c.user.as_ref().map(|u| u.handle()).unwrap_or_else(|| "unknown".into()),
            created_at: c.created_at.clone(),
            body: c.body.clone().unwrap_or_default(),
        })
        .collect();

    NormalizedIssue {
        source: format!("linear:{}", issue.identifier),
        title: issue.title.clone(),
        description: issue.description.clone().unwrap_or_default(),
        status,
        priority: map_priority(issue.priority),
        labels,
        comments,
    }
}

/// Abstraction over Linear's GraphQL endpoint for testability. Returns one page
/// of issues plus the next cursor (`None` when exhausted).
pub trait LinearFetcher {
    fn fetch_page(&self, after: Option<&str>) -> Result<(Vec<LinearIssue>, Option<String>), String>;
}

/// Walk all cursor pages and normalize. Assignee/estimate are counted as
/// skipped facets for the summary.
pub fn collect(
    fetcher: &dyn LinearFetcher,
    map: &LinearStatusMap,
) -> Result<super::FetchedIssues, String> {
    let mut out = super::FetchedIssues::default();
    let mut cursor: Option<String> = None;
    let mut guard = 0u32;
    loop {
        let (issues, next) = fetcher.fetch_page(cursor.as_deref())?;
        for issue in &issues {
            if issue.assignee.is_some() {
                out.skipped_assignees += 1;
            }
            if issue.estimate.is_some() {
                out.skipped_other += 1;
            }
            out.issues.push(map_issue(issue, map));
        }
        match next {
            Some(c) => cursor = Some(c),
            None => break,
        }
        guard += 1;
        if guard > 1000 {
            break;
        }
    }
    Ok(out)
}

/// The GraphQL query used by the live fetcher. `first: 50` batches with nested
/// label/comment connections to stay well under the ~1500 req/hr budget.
pub const ISSUES_QUERY: &str = r#"
query Issues($team: String!, $after: String) {
  issues(
    first: 50
    after: $after
    filter: { team: { key: { eq: $team } } }
  ) {
    pageInfo { hasNextPage endCursor }
    nodes {
      identifier
      title
      description
      priority
      state { type }
      assignee { name displayName }
      estimate
      labels { nodes { name color } }
      comments { nodes { body createdAt user { name displayName } } }
    }
  }
}
"#;

/// Live GraphQL fetcher over the blocking reqwest client.
pub struct LiveLinear {
    client: reqwest::blocking::Client,
    token: String,
    team: String,
}

#[derive(Debug, Deserialize)]
struct GraphqlEnvelope {
    #[serde(default)]
    data: Option<GraphqlData>,
    #[serde(default)]
    errors: Option<Vec<GraphqlError>>,
}

#[derive(Debug, Deserialize)]
struct GraphqlData {
    issues: IssuesConnection,
}

#[derive(Debug, Deserialize)]
struct IssuesConnection {
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
    nodes: Vec<LinearIssue>,
}

#[derive(Debug, Deserialize)]
struct PageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
    #[serde(rename = "endCursor")]
    end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GraphqlError {
    message: String,
}

impl LiveLinear {
    pub fn new(team: &str, token: String) -> Result<LiveLinear, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("lific-import/1.0")
            .build()
            .map_err(|e| format!("http client init failed: {e}"))?;
        Ok(LiveLinear {
            client,
            token,
            team: team.to_string(),
        })
    }
}

impl LinearFetcher for LiveLinear {
    fn fetch_page(&self, after: Option<&str>) -> Result<(Vec<LinearIssue>, Option<String>), String> {
        let body = serde_json::json!({
            "query": ISSUES_QUERY,
            "variables": { "team": self.team, "after": after },
        });
        let resp = self
            .client
            .post("https://api.linear.app/graphql")
            .header("Authorization", &self.token)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .map_err(|e| format!("request failed: {e}"))?;
        if resp.status().as_u16() == 401 || resp.status().as_u16() == 400 {
            return Err("Linear authentication failed — check your API key".into());
        }
        if !resp.status().is_success() {
            return Err(format!("Linear returned HTTP {}", resp.status()));
        }
        let env: GraphqlEnvelope = resp
            .json()
            .map_err(|e| format!("failed to parse Linear response: {e}"))?;
        if let Some(errs) = env.errors
            && !errs.is_empty()
        {
            return Err(format!(
                "Linear GraphQL error: {}",
                errs.into_iter()
                    .map(|e| e.message)
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        let data = env
            .data
            .ok_or_else(|| "Linear response had no data".to_string())?;
        let next = if data.issues.page_info.has_next_page {
            data.issues.page_info.end_cursor
        } else {
            None
        };
        Ok((data.issues.nodes, next))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("fixtures/linear_issues.json");

    fn fixture() -> Vec<LinearIssue> {
        serde_json::from_str(FIXTURE).unwrap()
    }

    #[test]
    fn state_type_mapping_default() {
        let m = LinearStatusMap::default();
        assert_eq!(map_state_type("backlog", &m), "backlog");
        assert_eq!(map_state_type("triage", &m), "backlog");
        assert_eq!(map_state_type("unstarted", &m), "todo");
        assert_eq!(map_state_type("started", &m), "active");
        assert_eq!(map_state_type("completed", &m), "done");
        assert_eq!(map_state_type("canceled", &m), "cancelled");
        assert_eq!(map_state_type("weird", &m), "backlog");
    }

    #[test]
    fn state_type_mapping_custom() {
        let m = LinearStatusMap {
            started: "todo".into(),
            ..LinearStatusMap::default()
        };
        assert_eq!(map_state_type("started", &m), "todo");
    }

    #[test]
    fn priority_mapping() {
        assert_eq!(map_priority(0.0), "none");
        assert_eq!(map_priority(1.0), "urgent");
        assert_eq!(map_priority(2.0), "high");
        assert_eq!(map_priority(3.0), "medium");
        assert_eq!(map_priority(4.0), "low");
    }

    #[test]
    fn map_issue_uses_identifier_as_source() {
        let issues = fixture();
        let first = &issues[0];
        let mapped = map_issue(first, &LinearStatusMap::default());
        assert_eq!(mapped.source, "linear:ENG-1");
        assert_eq!(mapped.status, "active"); // state.type = started
        assert_eq!(mapped.priority, "urgent"); // priority 1
        assert_eq!(mapped.labels.len(), 1);
        assert_eq!(mapped.labels[0].name, "Bug");
        assert_eq!(mapped.labels[0].color.as_deref(), Some("#e11d48"));
        assert_eq!(mapped.comments.len(), 1);
        assert_eq!(mapped.comments[0].author, "Alice");
    }

    #[test]
    fn descriptions_are_verbatim_markdown() {
        let issues = fixture();
        let mapped = map_issue(&issues[0], &LinearStatusMap::default());
        assert!(mapped.description.contains("## Steps"));
        assert!(mapped.description.contains("- reproduce"));
    }

    struct FakeLinear {
        pages: Vec<(Vec<LinearIssue>, Option<String>)>,
    }
    impl LinearFetcher for FakeLinear {
        fn fetch_page(
            &self,
            after: Option<&str>,
        ) -> Result<(Vec<LinearIssue>, Option<String>), String> {
            // Page 0 has after=None; subsequent pages keyed by cursor value.
            let idx = match after {
                None => 0,
                Some(c) => c.parse::<usize>().unwrap(),
            };
            Ok(self.pages[idx].clone())
        }
    }

    #[test]
    fn collect_walks_cursor_pages() {
        let issues = fixture();
        let (a, b) = issues.split_at(1);
        let fetcher = FakeLinear {
            pages: vec![
                (a.to_vec(), Some("1".into())),
                (b.to_vec(), None),
            ],
        };
        let fetched = collect(&fetcher, &LinearStatusMap::default()).unwrap();
        assert_eq!(fetched.issues.len(), issues.len());
        // Fixture: first issue has an assignee.
        assert!(fetched.skipped_assignees >= 1);
    }
}
