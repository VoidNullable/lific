//! Search fallback (LIF-476) through the MCP `search` tool.

use super::tests::{mcp, seed_issue, seed_project};
use super::*;
use rmcp::handler::server::wrapper::Parameters;

fn search(m: &LificMcp, query: &str) -> String {
    m.search(Parameters(SearchInput {
        query: query.into(),
        ..Default::default()
    }))
}

#[test]
fn search_labels_partial_matches_when_no_result_has_every_word() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");
    seed_issue(&m, "TST", "Parser rejects empty input");
    seed_issue(&m, "TST", "Search ranking ignores empty titles");

    let result = search(&m, "search ranking empty zeppelin");

    let mut lines = result.lines();
    assert_eq!(
        lines.next(),
        Some("No result contains every word; showing partial matches ranked by relevance."),
        "got: {result}"
    );
    assert_eq!(lines.next(), Some("2 results:"), "got: {result}");
    let first = lines.next().unwrap_or_default();
    assert!(
        first.contains("TST-2"),
        "best partial match first: {result}"
    );
}

#[test]
fn search_with_a_full_match_renders_as_before() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");
    seed_issue(&m, "TST", "Search ranking ignores empty titles");
    seed_issue(&m, "TST", "Search is slow");

    let result = search(&m, "search ranking");

    assert!(result.starts_with("1 results:"), "got: {result}");
    assert!(!result.contains("partial"), "got: {result}");
}

// ── Similar open issues after create_issue (LIF-477) ─────────

fn create(m: &LificMcp, title: &str, status: Option<&str>, description: Option<&str>) -> String {
    m.create_issue(Parameters(CreateIssueInput {
        project: Some("TST".into()),
        title: title.into(),
        status: status.map(Into::into),
        description: description.map(Into::into),
        ..Default::default()
    }))
}

#[test]
fn create_issue_lists_open_issues_that_share_its_wording() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");
    seed_issue(&m, "TST", "Search ranking ignores empty titles");
    seed_issue(&m, "TST", "Billing export");

    let result = create(&m, "Search ranking breaks on long queries", None, None);

    assert_eq!(
        result,
        "Created TST-3: Search ranking breaks on long queries\n\
         Similar open issues:\n\
         - TST-1 (backlog) Search ranking ignores empty titles"
    );
}

#[test]
fn create_issue_never_lists_closed_issues() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");
    create(
        &m,
        "Search ranking ignores empty titles",
        Some("done"),
        None,
    );
    create(&m, "Search ranking drops accents", Some("cancelled"), None);

    let result = create(&m, "Search ranking breaks on long queries", None, None);

    assert!(!result.contains("Similar"), "got: {result}");
}

#[test]
fn create_issue_never_lists_the_issue_it_just_created() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");

    let result = create(&m, "Search ranking breaks on long queries", None, None);

    assert_eq!(
        result,
        "Created TST-1: Search ranking breaks on long queries"
    );
}

#[test]
fn create_issue_ignores_overlap_in_stopwords_or_a_single_description_word() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");
    seed_issue(&m, "TST", "Add the export button");
    create(
        &m,
        "Billing totals",
        None,
        Some("Unrelated, but mentions search and ranking in passing."),
    );

    let stopwords = create(&m, "Add the dark theme", None, None);
    assert!(!stopwords.contains("Similar"), "got: {stopwords}");

    let description_only = create(&m, "Search ranking breaks", None, None);
    assert!(
        !description_only.contains("Similar"),
        "a match only in another issue's description is not a duplicate: {description_only}"
    );
}

#[test]
fn create_issue_lists_at_most_three_similar_issues() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");
    for suffix in ["one", "two", "three", "four"] {
        seed_issue(&m, "TST", &format!("Search ranking regression {suffix}"));
    }

    let result = create(&m, "Search ranking regression five", None, None);

    assert_eq!(result.matches("\n- ").count(), 3, "got: {result}");
}
