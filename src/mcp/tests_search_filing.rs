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
