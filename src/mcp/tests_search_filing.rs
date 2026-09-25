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

// ── Batch create_issue (LIF-478) ─────────────────────────────

fn item(title: &str) -> CreateIssueItem {
    CreateIssueItem {
        title: title.into(),
        ..Default::default()
    }
}

fn create_batch(m: &LificMcp, issues: Vec<CreateIssueItem>) -> String {
    m.create_issue(Parameters(CreateIssueInput {
        project: Some("TST".into()),
        issues: Some(issues),
        ..Default::default()
    }))
}

fn issue_count(m: &LificMcp) -> i64 {
    m.read(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM issues", [], |row| row.get(0))?))
        .unwrap()
}

#[test]
fn create_issue_batch_creates_every_item_and_returns_each_identifier() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");

    let result = create_batch(
        &m,
        vec![
            item("Billing export"),
            CreateIssueItem {
                status: Some("todo".into()),
                priority: Some("high".into()),
                ..item("Dark theme toggle")
            },
            item("Keyboard shortcuts overlay"),
        ],
    );

    assert_eq!(
        result,
        "Created 3 issues:\n\
         - TST-1: Billing export\n\
         - TST-2: Dark theme toggle\n\
         - TST-3: Keyboard shortcuts overlay"
    );
    let second = m
        .read(|conn| queries::get_issue(conn, queries::resolve_identifier(conn, "TST-2")?))
        .unwrap();
    assert_eq!(second.status.as_str(), "todo");
    assert_eq!(second.priority.as_str(), "high");
}

#[test]
fn create_issue_batch_with_an_invalid_item_creates_nothing_and_names_it() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");

    let unknown_module = create_batch(
        &m,
        vec![
            item("Billing export"),
            CreateIssueItem {
                module: Some("Nonexistent".into()),
                ..item("Dark theme toggle")
            },
            item("Keyboard shortcuts overlay"),
        ],
    );
    assert!(
        unknown_module.starts_with("Error: ") && unknown_module.contains("issues[1]"),
        "got: {unknown_module}"
    );

    let bad_status = create_batch(
        &m,
        vec![
            item("Billing export"),
            item("Dark theme toggle"),
            CreateIssueItem {
                status: Some("finished".into()),
                ..item("Keyboard shortcuts overlay")
            },
        ],
    );
    assert!(bad_status.contains("issues[2]"), "got: {bad_status}");

    let missing_title = create_batch(&m, vec![item("Billing export"), item("  ")]);
    assert!(
        missing_title.contains("issues[1]") && missing_title.contains("title is required"),
        "got: {missing_title}"
    );

    assert_eq!(issue_count(&m), 0, "a failed batch creates no issue at all");
}

#[test]
fn create_issue_batch_refuses_top_level_fields_and_empty_batches() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");

    let mixed = m.create_issue(Parameters(CreateIssueInput {
        project: Some("TST".into()),
        title: "Stray title".into(),
        issues: Some(vec![item("Billing export")]),
        ..Default::default()
    }));
    assert!(mixed.contains("only project applies"), "got: {mixed}");

    let empty = create_batch(&m, Vec::new());
    assert!(empty.starts_with("Error: "), "got: {empty}");

    let neither = m.create_issue(Parameters(CreateIssueInput {
        project: Some("TST".into()),
        ..Default::default()
    }));
    assert!(neither.contains("title is required"), "got: {neither}");

    assert_eq!(issue_count(&m), 0);
}

#[test]
fn create_issue_batch_hints_existing_duplicates_but_not_its_own_items() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");
    seed_issue(&m, "TST", "Search ranking ignores empty titles");

    let result = create_batch(
        &m,
        vec![
            item("Search ranking breaks on long queries"),
            item("Search ranking drops accents"),
        ],
    );

    assert_eq!(
        result,
        "Created 2 issues:\n\
         - TST-2: Search ranking breaks on long queries (similar open: TST-1)\n\
         - TST-3: Search ranking drops accents (similar open: TST-1)"
    );
}

#[test]
fn create_issue_input_accepts_a_titleless_batch_and_rejects_unknown_item_keys() {
    let batch: CreateIssueInput = serde_json::from_value(serde_json::json!({
        "project": "TST",
        "issues": [{ "title": "One" }, { "title": "Two", "status": "todo" }]
    }))
    .unwrap();
    assert!(batch.title.is_empty());
    assert_eq!(batch.issues.map(|issues| issues.len()), Some(2));

    let unknown = serde_json::from_value::<CreateIssueInput>(serde_json::json!({
        "issues": [{ "title": "One", "project": "TST" }]
    }));
    assert!(unknown.is_err(), "a per-item project is not a thing");
}
