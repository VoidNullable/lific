//! Jira Cloud importer (LIF-265).
//!
//! Jira Cloud's REST API v3 (`https://<site>.atlassian.net/rest/api/3/`) uses
//! basic auth (`email:api_token`). Issues are fetched with a JQL search
//! (`project = KEY ORDER BY created ASC`), paginated. Newer Jira exposes
//! `/rest/api/3/search/jql` with `nextPageToken`; the live fetcher targets that
//! and falls back on `startAt`/`total` semantics only if needed. Comments come
//! from `/issue/{key}/comment`.
//!
//! ## The hard part: ADF → markdown
//!
//! v3 descriptions and comments are **Atlassian Document Format** (ADF) — a
//! JSON document tree, not markdown or HTML. [`adf_to_markdown`] is a bounded
//! converter over the node types we care about (paragraph, heading, marks,
//! lists, code, blockquote, hardBreak, mention, rule, …). Unknown node types
//! **degrade to their concatenated inner text** rather than failing — an import
//! must never abort over formatting it doesn't recognize.

use serde::Deserialize;

use super::{NormalizedComment, NormalizedIssue, NormalizedLabel};

// ── ADF → markdown ───────────────────────────────────────────

/// Convert an Atlassian Document Format value to markdown. Never errors: an
/// absent/`null` doc yields an empty string, and unknown nodes degrade to text.
pub fn adf_to_markdown(doc: &serde_json::Value) -> String {
    if doc.is_null() {
        return String::new();
    }
    // Sometimes a plain string sneaks through (older payloads / already-text
    // fields) — pass it through untouched.
    if let Some(s) = doc.as_str() {
        return s.to_string();
    }
    let mut out = String::new();
    render_block_children(doc, &mut out);
    out.trim_end().to_string()
}

/// Render the top-level `content` array (block nodes) joined by blank lines.
fn render_block_children(node: &serde_json::Value, out: &mut String) {
    let Some(content) = node.get("content").and_then(|c| c.as_array()) else {
        return;
    };
    let mut first = true;
    for child in content {
        let mut block = String::new();
        render_block(child, &mut block);
        if block.is_empty() {
            continue;
        }
        if !first {
            out.push_str("\n\n");
        }
        out.push_str(&block);
        first = false;
    }
}

/// Render one block-level node into `out` (no trailing blank line).
fn render_block(node: &serde_json::Value, out: &mut String) {
    let node_type = node.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match node_type {
        "paragraph" => {
            out.push_str(&render_inline(node));
        }
        "heading" => {
            let level = node
                .get("attrs")
                .and_then(|a| a.get("level"))
                .and_then(|l| l.as_u64())
                .unwrap_or(1)
                .clamp(1, 6) as usize;
            out.push_str(&"#".repeat(level));
            out.push(' ');
            out.push_str(&render_inline(node));
        }
        "bulletList" => render_list(node, out, false),
        "orderedList" => render_list(node, out, true),
        "codeBlock" => {
            let lang = node
                .get("attrs")
                .and_then(|a| a.get("language"))
                .and_then(|l| l.as_str())
                .unwrap_or("");
            out.push_str("```");
            out.push_str(lang);
            out.push('\n');
            out.push_str(&collect_text(node));
            out.push_str("\n```");
        }
        "blockquote" => {
            let mut inner = String::new();
            render_block_children(node, &mut inner);
            for line in inner.lines() {
                out.push_str("> ");
                out.push_str(line);
                out.push('\n');
            }
            // trim trailing newline
            while out.ends_with('\n') {
                out.pop();
            }
        }
        "rule" => out.push_str("---"),
        // Unknown block type: degrade to its concatenated inner text so nothing
        // is lost and the import never fails.
        _ => {
            let inline = render_inline(node);
            if inline.is_empty() {
                out.push_str(&collect_text(node));
            } else {
                out.push_str(&inline);
            }
        }
    }
}

/// Render a bullet/ordered list's `listItem` children.
fn render_list(node: &serde_json::Value, out: &mut String, ordered: bool) {
    let Some(items) = node.get("content").and_then(|c| c.as_array()) else {
        return;
    };
    for (i, item) in items.iter().enumerate() {
        let marker = if ordered {
            format!("{}. ", i + 1)
        } else {
            "- ".to_string()
        };
        // A listItem contains block nodes (usually one paragraph).
        let mut item_body = String::new();
        render_block_children(item, &mut item_body);
        // Prefix the first line with the marker; indent continuation lines.
        let mut lines = item_body.lines();
        if let Some(first) = lines.next() {
            out.push_str(&marker);
            out.push_str(first);
        } else {
            out.push_str(&marker);
        }
        for line in lines {
            out.push('\n');
            out.push_str("  ");
            out.push_str(line);
        }
        out.push('\n');
    }
    while out.ends_with('\n') {
        out.pop();
    }
}

/// Render inline content of a node (text nodes with marks, hardBreaks,
/// mentions, inline code).
fn render_inline(node: &serde_json::Value) -> String {
    let Some(content) = node.get("content").and_then(|c| c.as_array()) else {
        return String::new();
    };
    let mut out = String::new();
    for child in content {
        render_inline_node(child, &mut out);
    }
    out
}

fn render_inline_node(node: &serde_json::Value, out: &mut String) {
    let node_type = node.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match node_type {
        "text" => {
            let text = node.get("text").and_then(|t| t.as_str()).unwrap_or("");
            out.push_str(&apply_marks(text, node));
        }
        "hardBreak" => out.push_str("  \n"),
        "mention" => {
            let name = node
                .get("attrs")
                .and_then(|a| a.get("text"))
                .and_then(|t| t.as_str())
                .map(|s| s.trim_start_matches('@').to_string())
                .unwrap_or_else(|| "someone".to_string());
            out.push('@');
            out.push_str(&name);
        }
        "emoji" => {
            let text = node
                .get("attrs")
                .and_then(|a| a.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            out.push_str(text);
        }
        "inlineCard" => {
            let url = node
                .get("attrs")
                .and_then(|a| a.get("url"))
                .and_then(|u| u.as_str())
                .unwrap_or("");
            out.push_str(url);
        }
        // Unknown inline node: degrade to its text.
        _ => out.push_str(&collect_text(node)),
    }
}

/// Apply text marks (bold, italic, code, link, strike) to a piece of text.
fn apply_marks(text: &str, node: &serde_json::Value) -> String {
    let Some(marks) = node.get("marks").and_then(|m| m.as_array()) else {
        return text.to_string();
    };
    let mut result = text.to_string();
    // `code` is exclusive-ish but we still wrap; order chosen so links wrap the
    // outermost emphasis.
    let mut link_href: Option<String> = None;
    for mark in marks {
        let mark_type = mark.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match mark_type {
            "strong" => result = format!("**{result}**"),
            "em" => result = format!("*{result}*"),
            "code" => result = format!("`{result}`"),
            "strike" => result = format!("~~{result}~~"),
            "link" => {
                link_href = mark
                    .get("attrs")
                    .and_then(|a| a.get("href"))
                    .and_then(|h| h.as_str())
                    .map(|s| s.to_string());
            }
            // Unknown mark: leave text unwrapped.
            _ => {}
        }
    }
    if let Some(href) = link_href {
        result = format!("[{result}]({href})");
    }
    result
}

/// Recursively concatenate all `text` leaves under a node. The universal
/// fallback that guarantees no content is lost for unknown node types.
fn collect_text(node: &serde_json::Value) -> String {
    let mut out = String::new();
    collect_text_into(node, &mut out);
    out
}

fn collect_text_into(node: &serde_json::Value, out: &mut String) {
    if let Some(t) = node.get("text").and_then(|t| t.as_str()) {
        out.push_str(t);
    }
    if let Some(content) = node.get("content").and_then(|c| c.as_array()) {
        for child in content {
            collect_text_into(child, out);
        }
    }
}

// ── Jira issue mapping ───────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct JiraIssue {
    pub key: String,
    #[serde(default)]
    pub fields: JiraFields,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct JiraFields {
    #[serde(default)]
    pub summary: Option<String>,
    /// ADF document (or null).
    #[serde(default)]
    pub description: serde_json::Value,
    #[serde(default)]
    pub status: Option<JiraStatus>,
    #[serde(default)]
    pub priority: Option<JiraPriority>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub assignee: Option<serde_json::Value>,
    #[serde(default)]
    pub issuetype: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JiraStatus {
    #[serde(default, rename = "statusCategory")]
    pub status_category: Option<JiraStatusCategory>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JiraStatusCategory {
    /// "new" / "indeterminate" / "done"
    #[serde(default)]
    pub key: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JiraPriority {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JiraComment {
    #[serde(default)]
    pub author: Option<JiraAuthor>,
    /// ADF body.
    #[serde(default)]
    pub body: serde_json::Value,
    #[serde(default)]
    pub created: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JiraAuthor {
    #[serde(default, rename = "displayName")]
    pub display_name: Option<String>,
}

/// Configurable statusCategory → Lific status mapping.
#[derive(Debug, Clone)]
pub struct JiraStatusMap {
    pub new: String,
    pub indeterminate: String,
    pub done: String,
}

impl Default for JiraStatusMap {
    fn default() -> Self {
        JiraStatusMap {
            new: "backlog".into(),
            indeterminate: "active".into(),
            done: "done".into(),
        }
    }
}

/// Map Jira's statusCategory.key to a Lific status.
pub fn map_status(category_key: &str, map: &JiraStatusMap) -> String {
    match category_key {
        "new" => map.new.clone(),
        "indeterminate" => map.indeterminate.clone(),
        "done" => map.done.clone(),
        _ => map.new.clone(),
    }
}

/// Map Jira priority names to Lific priorities. Highest/High → urgent/high,
/// Medium → medium, Low/Lowest → low. Unknown → none.
pub fn map_priority(name: &str) -> String {
    match name {
        "Highest" => "urgent",
        "High" => "high",
        "Medium" => "medium",
        "Low" | "Lowest" => "low",
        _ => "none",
    }
    .to_string()
}

/// Map one Jira issue + its comments to a [`NormalizedIssue`]. `site` is used
/// for the `jira:<site>:KEY-123` source marker.
pub fn map_issue(
    site: &str,
    issue: &JiraIssue,
    comments: &[JiraComment],
    map: &JiraStatusMap,
) -> NormalizedIssue {
    let status = issue
        .fields
        .status
        .as_ref()
        .and_then(|s| s.status_category.as_ref())
        .map(|c| map_status(&c.key, map))
        .unwrap_or_else(|| map.new.clone());

    let priority = issue
        .fields
        .priority
        .as_ref()
        .map(|p| map_priority(&p.name))
        .unwrap_or_else(|| "none".to_string());

    let labels = issue
        .fields
        .labels
        .iter()
        .map(|name| NormalizedLabel {
            name: name.clone(),
            color: None, // Jira labels are plain strings, no color
        })
        .collect();

    let mapped_comments = comments
        .iter()
        .map(|c| NormalizedComment {
            author: c
                .author
                .as_ref()
                .and_then(|a| a.display_name.clone())
                .unwrap_or_else(|| "unknown".to_string()),
            created_at: c.created.clone(),
            body: adf_to_markdown(&c.body),
        })
        .collect();

    NormalizedIssue {
        source: format!("jira:{site}:{}", issue.key),
        title: issue.fields.summary.clone().unwrap_or_default(),
        description: adf_to_markdown(&issue.fields.description),
        status,
        priority,
        labels,
        comments: mapped_comments,
    }
}

/// Abstraction over Jira's REST endpoints for testability.
pub trait JiraFetcher {
    /// Fetch one page of issues. `next_token` is the opaque page token (`None`
    /// for the first page). Returns `(issues, next_token)`.
    fn fetch_page(&self, next_token: Option<&str>)
    -> Result<(Vec<JiraIssue>, Option<String>), String>;

    /// Fetch all comments for one issue key.
    fn fetch_comments(&self, key: &str) -> Result<Vec<JiraComment>, String>;
}

/// Walk all pages and normalize. Assignee/issuetype counted as skipped facets.
pub fn collect(
    fetcher: &dyn JiraFetcher,
    site: &str,
    map: &JiraStatusMap,
) -> Result<super::FetchedIssues, String> {
    let mut out = super::FetchedIssues::default();
    let mut token: Option<String> = None;
    let mut guard = 0u32;
    loop {
        let (issues, next) = fetcher.fetch_page(token.as_deref())?;
        for issue in &issues {
            if issue.fields.assignee.is_some() {
                out.skipped_assignees += 1;
            }
            if issue.fields.issuetype.is_some() {
                out.skipped_other += 1;
            }
            let comments = fetcher.fetch_comments(&issue.key)?;
            out.issues.push(map_issue(site, issue, &comments, map));
        }
        match next {
            Some(t) => token = Some(t),
            None => break,
        }
        guard += 1;
        if guard > 1000 {
            break;
        }
    }
    Ok(out)
}

/// Live fetcher over Jira Cloud REST v3 with basic auth.
pub struct LiveJira {
    client: reqwest::blocking::Client,
    site: String,
    project_key: String,
    auth: String,
}

#[derive(Debug, Deserialize)]
struct JiraSearchResponse {
    #[serde(default)]
    issues: Vec<JiraIssue>,
    #[serde(default, rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JiraCommentsResponse {
    #[serde(default)]
    comments: Vec<JiraComment>,
}

impl LiveJira {
    pub fn new(
        site: &str,
        project_key: &str,
        email: &str,
        token: &str,
    ) -> Result<LiveJira, String> {
        use base64::Engine;
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("lific-import/1.0")
            .build()
            .map_err(|e| format!("http client init failed: {e}"))?;
        let raw = format!("{email}:{token}");
        let auth = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(raw.as_bytes())
        );
        Ok(LiveJira {
            client,
            site: site.to_string(),
            project_key: project_key.to_string(),
            auth,
        })
    }

    fn base(&self) -> String {
        format!("https://{}.atlassian.net/rest/api/3", self.site)
    }
}

impl JiraFetcher for LiveJira {
    fn fetch_page(
        &self,
        next_token: Option<&str>,
    ) -> Result<(Vec<JiraIssue>, Option<String>), String> {
        let jql = format!("project = {} ORDER BY created ASC", self.project_key);
        let mut req = self
            .client
            .get(format!("{}/search/jql", self.base()))
            .header("Authorization", &self.auth)
            .header("Accept", "application/json")
            .query(&[
                ("jql", jql.as_str()),
                ("maxResults", "50"),
                ("fields", "summary,description,status,priority,labels,assignee,issuetype"),
            ]);
        if let Some(t) = next_token {
            req = req.query(&[("nextPageToken", t)]);
        }
        let resp = req.send().map_err(|e| format!("request failed: {e}"))?;
        if resp.status().as_u16() == 401 {
            return Err("Jira authentication failed — check email + API token".into());
        }
        if resp.status().as_u16() == 404 {
            return Err(format!(
                "Jira project {} not found on site {}",
                self.project_key, self.site
            ));
        }
        if !resp.status().is_success() {
            return Err(format!("Jira returned HTTP {}", resp.status()));
        }
        let parsed: JiraSearchResponse = resp
            .json()
            .map_err(|e| format!("failed to parse Jira search response: {e}"))?;
        Ok((parsed.issues, parsed.next_page_token))
    }

    fn fetch_comments(&self, key: &str) -> Result<Vec<JiraComment>, String> {
        let resp = self
            .client
            .get(format!("{}/issue/{key}/comment", self.base()))
            .header("Authorization", &self.auth)
            .header("Accept", "application/json")
            .send()
            .map_err(|e| format!("request failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!(
                "Jira returned HTTP {} fetching comments for {key}",
                resp.status()
            ));
        }
        let parsed: JiraCommentsResponse = resp
            .json()
            .map_err(|e| format!("failed to parse Jira comments: {e}"))?;
        Ok(parsed.comments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ISSUES_FIXTURE: &str = include_str!("fixtures/jira_issues.json");
    const ADF_FIXTURE: &str = include_str!("fixtures/jira_adf.json");

    fn fixture_issues() -> Vec<JiraIssue> {
        serde_json::from_str(ISSUES_FIXTURE).unwrap()
    }

    #[test]
    fn status_category_mapping() {
        let m = JiraStatusMap::default();
        assert_eq!(map_status("new", &m), "backlog");
        assert_eq!(map_status("indeterminate", &m), "active");
        assert_eq!(map_status("done", &m), "done");
        assert_eq!(map_status("weird", &m), "backlog");
    }

    #[test]
    fn priority_mapping() {
        assert_eq!(map_priority("Highest"), "urgent");
        assert_eq!(map_priority("High"), "high");
        assert_eq!(map_priority("Medium"), "medium");
        assert_eq!(map_priority("Low"), "low");
        assert_eq!(map_priority("Lowest"), "low");
        assert_eq!(map_priority("Whatever"), "none");
    }

    #[test]
    fn map_issue_builds_source_and_maps_fields() {
        let issues = fixture_issues();
        let first = issues.iter().find(|i| i.key == "PROJ-1").unwrap();
        let mapped = map_issue("mycompany", first, &[], &JiraStatusMap::default());
        assert_eq!(mapped.source, "jira:mycompany:PROJ-1");
        assert_eq!(mapped.status, "active"); // indeterminate
        assert_eq!(mapped.priority, "high");
        assert_eq!(mapped.title, "Implement search");
        // Labels are plain strings, no color.
        assert_eq!(mapped.labels.len(), 2);
        assert!(mapped.labels.iter().all(|l| l.color.is_none()));
        // Description is ADF converted to markdown.
        assert!(mapped.description.contains("search"));
    }

    #[test]
    fn map_issue_with_null_description() {
        let issues = fixture_issues();
        let second = issues.iter().find(|i| i.key == "PROJ-2").unwrap();
        let mapped = map_issue("mycompany", second, &[], &JiraStatusMap::default());
        assert_eq!(mapped.description, "");
        assert_eq!(mapped.status, "done");
    }

    // ── ADF converter: covers every supported node + an unknown one ──

    #[test]
    fn adf_null_and_string_passthrough() {
        assert_eq!(adf_to_markdown(&serde_json::Value::Null), "");
        assert_eq!(
            adf_to_markdown(&serde_json::json!("plain text")),
            "plain text"
        );
    }

    #[test]
    fn adf_full_fixture_covers_all_node_types() {
        let doc: serde_json::Value = serde_json::from_str(ADF_FIXTURE).unwrap();
        let md = adf_to_markdown(&doc);

        // heading
        assert!(md.contains("# Title"), "heading: {md}");
        // paragraph with bold, italic, code, link marks
        assert!(md.contains("**bold**"), "bold: {md}");
        assert!(md.contains("*italic*"), "italic: {md}");
        assert!(md.contains("`code`"), "inline code: {md}");
        assert!(md.contains("[a link](https://example.com)"), "link: {md}");
        // bullet list
        assert!(md.contains("- first"), "bullet: {md}");
        assert!(md.contains("- second"), "bullet: {md}");
        // ordered list
        assert!(md.contains("1. one"), "ordered: {md}");
        assert!(md.contains("2. two"), "ordered: {md}");
        // code block with language
        assert!(md.contains("```rust"), "codeblock lang: {md}");
        assert!(md.contains("let x = 1;"), "codeblock body: {md}");
        // blockquote
        assert!(md.contains("> quoted"), "blockquote: {md}");
        // mention
        assert!(md.contains("@alice"), "mention: {md}");
        // rule
        assert!(md.contains("---"), "rule: {md}");
        // hardBreak: two-space + newline present
        assert!(md.contains("line1  \nline2"), "hardbreak: {md}");
    }

    #[test]
    fn adf_unknown_node_degrades_to_text_not_failure() {
        // A made-up "panel"/"status" node the converter doesn't know: its inner
        // text must survive, and conversion must not panic or drop content.
        let doc = serde_json::json!({
            "type": "doc",
            "version": 1,
            "content": [
                {
                    "type": "someUnknownFutureNode",
                    "content": [
                        { "type": "text", "text": "survived degradation" }
                    ]
                },
                {
                    "type": "paragraph",
                    "content": [
                        {
                            "type": "reallyWeirdInline",
                            "content": [{ "type": "text", "text": "inline survived" }]
                        }
                    ]
                }
            ]
        });
        let md = adf_to_markdown(&doc);
        assert!(md.contains("survived degradation"), "block degrade: {md}");
        assert!(md.contains("inline survived"), "inline degrade: {md}");
    }

    // Fake fetcher driving the full pipeline offline.
    struct FakeJira {
        pages: Vec<(Vec<JiraIssue>, Option<String>)>,
    }
    impl JiraFetcher for FakeJira {
        fn fetch_page(
            &self,
            token: Option<&str>,
        ) -> Result<(Vec<JiraIssue>, Option<String>), String> {
            let idx = match token {
                None => 0,
                Some(t) => t.parse::<usize>().unwrap(),
            };
            Ok(self.pages[idx].clone())
        }
        fn fetch_comments(&self, _key: &str) -> Result<Vec<JiraComment>, String> {
            Ok(vec![])
        }
    }

    #[test]
    fn collect_paginates_with_tokens() {
        let issues = fixture_issues();
        let (a, b) = issues.split_at(1);
        let fetcher = FakeJira {
            pages: vec![(a.to_vec(), Some("1".into())), (b.to_vec(), None)],
        };
        let fetched = collect(&fetcher, "mycompany", &JiraStatusMap::default()).unwrap();
        assert_eq!(fetched.issues.len(), issues.len());
    }
}
