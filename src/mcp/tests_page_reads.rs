//! Bounded page reads (LIF-479).

use rmcp::handler::server::wrapper::Parameters;

use super::super::page_reads::{Chars, PAGE_READ_BUDGET, headings};
use super::super::schemas::*;
use super::LificMcp;
use super::tests::{mcp, seed_project};
use crate::db::queries;

fn seed_page(m: &LificMcp, project: &str, content: &str) -> String {
    let result = m.create_page(Parameters(CreatePageInput {
        project: Some(project.into()),
        title: "Working notes".into(),
        content: Some(content.into()),
        ..Default::default()
    }));
    assert!(result.starts_with("Created "), "got: {result}");
    format!("{project}-DOC-1")
}

fn read(m: &LificMcp, identifier: &str, section: Option<&str>, outline: bool) -> String {
    m.get_page(Parameters(GetPageInput {
        identifier: identifier.into(),
        section: section.map(Into::into),
        outline: outline.then_some(true),
    }))
}

fn page_seq(m: &LificMcp, identifier: &str) -> i64 {
    m.read(|conn| {
        let id = queries::resolve_page_identifier(conn, identifier)?;
        Ok(queries::get_page(conn, id)?.seq)
    })
    .unwrap()
}

const NESTED: &str = "Intro line.\n\n## Alpha\nalpha body\n\n### Alpha one\nfirst child\n\n### Alpha two\nsecond child\n\n## Beta\nbeta body\n";

#[test]
fn a_page_within_the_budget_reads_exactly_as_before() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "SML");
    // Exactly at the budget still counts as within it.
    let content = format!("## Heading\n{}", "x".repeat(PAGE_READ_BUDGET - 11));
    assert_eq!(content.chars().count(), PAGE_READ_BUDGET);
    let identifier = seed_page(&m, "SML", &content);
    let page = m
        .read(|conn| {
            let id = queries::resolve_page_identifier(conn, &identifier)?;
            queries::get_page(conn, id)
        })
        .unwrap();

    let expected = format!(
        "SML-DOC-1 — Working notes\nStatus: draft | Folder: none\nCreated: {} | Updated: {}\n\n{content}\n",
        page.created_at, page.updated_at
    );
    assert_eq!(read(&m, &identifier, None, false), expected);
}

#[test]
fn section_returns_one_heading_and_nothing_after_it() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "SEC");
    let identifier = seed_page(&m, "SEC", NESTED);

    let result = read(&m, &identifier, Some("Beta"), false);

    assert!(result.contains("Section: ## Beta ("), "got: {result}");
    assert!(result.ends_with("## Beta\nbeta body\n"), "got: {result}");
    assert!(!result.contains("alpha body"), "got: {result}");
    assert!(!result.contains("Intro line"), "got: {result}");
}

#[test]
fn section_includes_nested_subsections_until_a_sibling_heading() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "NST");
    let identifier = seed_page(&m, "NST", NESTED);

    let alpha = read(&m, &identifier, Some("alpha"), false);
    assert!(alpha.contains("alpha body"), "got: {alpha}");
    assert!(alpha.contains("first child"), "got: {alpha}");
    assert!(alpha.contains("second child"), "got: {alpha}");
    assert!(!alpha.contains("beta body"), "got: {alpha}");

    let first = read(&m, &identifier, Some("Alpha one"), false);
    assert!(
        first.ends_with("### Alpha one\nfirst child\n"),
        "got: {first}"
    );
    assert!(!first.contains("second child"), "got: {first}");
}

#[test]
fn section_matches_an_anchor_or_the_heading_text_in_any_case() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "ANC");
    let identifier = seed_page(
        &m,
        "ANC",
        "## Current state (v2)\nnow\n\n## History\nthen\n",
    );

    for query in [
        "current-state-v2",
        "#current-state-v2",
        "CURRENT STATE (V2)",
        "## Current state (v2)",
    ] {
        let result = read(&m, &identifier, Some(query), false);
        assert!(
            result.ends_with("## Current state (v2)\nnow\n"),
            "{query}: {result}"
        );
    }
}

#[test]
fn an_ambiguous_section_lists_each_candidate_with_its_anchor() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "AMB");
    let identifier = seed_page(
        &m,
        "AMB",
        "## Week 1\n### Notes\nmonday\n\n## Week 2\n### Notes\ntuesday\n",
    );

    let result = read(&m, &identifier, Some("Notes"), false);
    assert!(
        result.starts_with("Error: 'Notes' matches 2 headings"),
        "got: {result}"
    );
    assert!(
        result.contains("- ### Notes [notes] under \"Week 1\""),
        "got: {result}"
    );
    assert!(
        result.contains("- ### Notes [notes-1] under \"Week 2\""),
        "got: {result}"
    );
    assert!(
        !result.contains("monday"),
        "no content on ambiguity: {result}"
    );

    let second = read(&m, &identifier, Some("notes-1"), false);
    assert!(second.ends_with("### Notes\ntuesday\n"), "got: {second}");
}

#[test]
fn an_unknown_section_is_an_error_that_points_at_the_outline() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "UNK");
    let identifier = seed_page(&m, "UNK", NESTED);

    let result = read(&m, &identifier, Some("Gamma"), false);
    assert!(result.starts_with("Error: no heading"), "got: {result}");
    assert!(result.contains("outline=true"), "got: {result}");
}

#[test]
fn outline_lists_headings_with_levels_section_sizes_and_the_seq() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "OUT");
    let identifier = seed_page(&m, "OUT", NESTED);
    let seq = page_seq(&m, &identifier);

    let result = read(&m, &identifier, None, true);

    let size = |heading: &str, until: Option<&str>| {
        let start = NESTED.find(heading).unwrap();
        let end = until.map_or(NESTED.len(), |next| NESTED.find(next).unwrap());
        NESTED[start..end].chars().count()
    };
    let expected = format!(
        "\nOutline (seq {seq}, {} chars; sizes include subsections):\n## Alpha ({})\n  ### Alpha one ({})\n  ### Alpha two ({})\n## Beta ({})\n",
        NESTED.chars().count(),
        size("## Alpha", Some("## Beta")),
        size("### Alpha one", Some("### Alpha two")),
        size("### Alpha two", Some("## Beta")),
        size("## Beta", None),
    );
    assert!(result.ends_with(&expected), "got: {result}");
    assert!(!result.contains("alpha body"), "outline only: {result}");
}

#[test]
fn sizes_count_characters_not_bytes() {
    let content = "## Café\néé\n";
    let parsed = headings(content);
    assert_eq!(parsed.len(), 1);
    let section = &content[parsed[0].start..parsed[0].end];
    assert_eq!(section.chars().count(), 11);
    assert_eq!(Chars(1_234_567).to_string(), "1,234,567");
}

#[test]
fn headings_inside_fenced_code_blocks_are_not_headings() {
    let content = "## Real\n```bash\n# a shell comment\n```\n~~~~\n## also code\n~~~\n## still code\n~~~~\n    # indented code\n### After #\n# C#\n";
    let parsed = headings(content);
    let texts: Vec<&str> = parsed.iter().map(|h| h.text.as_str()).collect();
    assert_eq!(texts, ["Real", "After", "C#"]);
    // The fenced lines belong to the section they sit in.
    assert!(content[parsed[0].start..parsed[0].end].contains("# a shell comment"));
}

#[test]
fn an_oversized_page_returns_its_outline_and_opening_with_the_section_call() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "BIG");
    let mut content = String::from("Opening paragraph.\n\n");
    for week in 1..=40 {
        content.push_str(&format!("## Week {week}\n"));
        for line in 0..100 {
            content.push_str(&format!("week {week} entry {line}\n"));
        }
        content.push('\n');
    }
    assert!(content.chars().count() > 2 * PAGE_READ_BUDGET);
    let identifier = seed_page(&m, "BIG", &content);
    let seq = page_seq(&m, &identifier);

    let result = read(&m, &identifier, None, false);

    assert!(
        result.contains(&format!(
            "chars, over the {}-char read budget",
            Chars(PAGE_READ_BUDGET)
        )),
        "got: {result}"
    );
    assert!(
        result.contains("get_page(identifier=\"BIG-DOC-1\", section=\"<heading text or anchor>\")"),
        "got: {result}"
    );
    assert!(
        result.contains(&format!("Outline (seq {seq};")),
        "got: {result}"
    );
    assert!(result.contains("\n## Week 40 ("), "full outline: {result}");
    assert!(
        result.contains("\nStart of the page:\nOpening paragraph."),
        "got: {result}"
    );
    assert!(!result.contains("week 40 entry 99"), "the tail stays out");
    assert!(result.contains("[Truncated at "), "got: {result}");
    assert!(
        result.chars().count() < PAGE_READ_BUDGET + 1_000,
        "{} chars",
        result.chars().count()
    );

    // And the tail is one section read away.
    let tail = read(&m, &identifier, Some("Week 40"), false);
    assert!(tail.ends_with("week 40 entry 99\n"), "got: {tail}");
}

#[test]
fn an_oversized_page_without_headings_says_it_cannot_be_sectioned() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "FLT");
    let identifier = seed_page(&m, "FLT", &"flat line\n".repeat(PAGE_READ_BUDGET / 5));

    let result = read(&m, &identifier, None, false);
    assert!(result.contains("has no headings"), "got: {result}");
    assert!(!result.contains("Outline"), "got: {result}");
    assert!(result.contains("[Truncated at "), "got: {result}");
}

#[test]
fn an_oversized_section_is_itself_outlined() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "BSC");
    let mut content = String::from("## Log\n");
    for day in 1..=3 {
        content.push_str(&format!(
            "### Day {day}\n{}\n",
            "entry\n".repeat(PAGE_READ_BUDGET / 5)
        ));
    }
    let identifier = seed_page(&m, "BSC", &content);

    let result = read(&m, &identifier, Some("Log"), false);
    assert!(result.contains("This section is "), "got: {result}");
    assert!(result.contains("\n  ### Day 3 ("), "got: {result}");
    assert!(result.chars().count() < PAGE_READ_BUDGET + 1_000);

    let outline = read(&m, &identifier, Some("Log"), true);
    assert!(outline.contains("\n  ### Day 2 ("), "got: {outline}");
    assert!(!outline.contains("entry"), "got: {outline}");
}

#[test]
fn an_oversized_page_with_a_huge_outline_drops_deeper_headings_first() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "DEP");
    let mut content = String::new();
    for part in 1..=300 {
        content.push_str(&format!("## Part {part}\n"));
        for sub in 1..=5 {
            content.push_str(&format!("### Part {part} detail {sub}\nsome text\n"));
        }
    }
    let identifier = seed_page(&m, "DEP", &content);

    let result = read(&m, &identifier, None, false);
    assert!(
        result.contains("Deeper headings are omitted"),
        "got: {result}"
    );
    assert!(result.contains("\n## Part 300 ("), "got: {result}");
    assert!(!result.contains("  ### Part"), "got: {result}");
    assert!(result.chars().count() < PAGE_READ_BUDGET + 1_000);

    let outline = read(&m, &identifier, None, true);
    assert!(
        outline.contains("  ### Part 300 detail 5 ("),
        "got: {outline}"
    );
}
