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
        since_seq: None,
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

// ── LIF-480: get_page(since_seq) ──

fn read_since(m: &LificMcp, identifier: &str, since_seq: i64) -> String {
    m.get_page(Parameters(GetPageInput {
        identifier: identifier.into(),
        since_seq: Some(since_seq),
        ..Default::default()
    }))
}

fn edit(m: &LificMcp, identifier: &str, old: &str, new: &str) {
    let result = m.edit_page(Parameters(EditPageInput {
        identifier: identifier.into(),
        old_string: old.into(),
        new_string: new.into(),
        ..Default::default()
    }));
    assert!(result.starts_with("Edited "), "got: {result}");
}

fn revision_count(m: &LificMcp, identifier: &str) -> i64 {
    m.read(|conn| {
        let id = queries::resolve_page_identifier(conn, identifier)?;
        Ok(conn.query_row(
            "SELECT count(*) FROM page_revisions WHERE page_id = ?1",
            [id],
            |row| row.get(0),
        )?)
    })
    .unwrap()
}

fn numbered_lines(count: usize) -> String {
    let mut lines = String::new();
    for n in 1..=count {
        lines.push_str(&format!("line {n}\n"));
    }
    lines
}

#[test]
fn since_seq_returns_only_the_changed_hunks_and_the_current_seq() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "DIF");
    let identifier = seed_page(&m, "DIF", &numbered_lines(30));
    let created = page_seq(&m, &identifier);
    edit(&m, &identifier, "line 5\n", "line five\n");
    let first = page_seq(&m, &identifier);
    edit(&m, &identifier, "line 25\n", "line twenty-five\n");
    let now = page_seq(&m, &identifier);
    assert!(created < first && first < now);

    let result = read_since(&m, &identifier, first);

    assert!(
        result.contains(&format!(
            "Content changes since seq {first} (now seq {now}):"
        )),
        "got: {result}"
    );
    assert!(
        result.contains("\n-line 25\n+line twenty-five\n"),
        "got: {result}"
    );
    assert!(result.contains("@@ -23,5 +23,5 @@"), "got: {result}");
    assert!(
        result.contains(" line 23\n"),
        "two lines of context: {result}"
    );
    assert!(!result.contains("line 22\n"), "no more context: {result}");
    assert!(
        !result.contains("line five"),
        "the earlier edit is not repeated: {result}"
    );
    assert!(
        !result.contains("line 15"),
        "unchanged content stays out: {result}"
    );

    // From the page's first version, both edits show.
    let both = read_since(&m, &identifier, created);
    assert!(both.contains("-line 5\n+line five\n"), "got: {both}");
    assert!(
        both.contains("-line 25\n+line twenty-five\n"),
        "got: {both}"
    );
}

#[test]
fn since_seq_between_versions_diffs_from_the_version_current_then() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "MID");
    seed_project(&m, "Other", "OTH");
    let identifier = seed_page(&m, "MID", &numbered_lines(10));
    edit(&m, &identifier, "line 2\n", "line two\n");
    // Seq is instance-wide: a seq read from another entity is a point in
    // time between this page's versions.
    let elsewhere = seed_page(&m, "OTH", "unrelated");
    let between = page_seq(&m, &elsewhere);
    edit(&m, &identifier, "line 8\n", "line eight\n");

    let result = read_since(&m, &identifier, between);
    assert!(result.contains("+line eight"), "got: {result}");
    assert!(!result.contains("line two"), "got: {result}");
}

#[test]
fn since_seq_reports_unchanged_content_when_only_metadata_moved() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "SAM");
    let identifier = seed_page(&m, "SAM", "body\n");
    let before = page_seq(&m, &identifier);
    m.update_page(Parameters(UpdatePageInput {
        identifier: identifier.clone(),
        title: Some("Renamed".into()),
        status: Some("active".into()),
        ..Default::default()
    }));
    let now = page_seq(&m, &identifier);
    assert!(now > before);

    let result = read_since(&m, &identifier, before);
    assert!(
        result.ends_with(&format!(
            "\nContent unchanged since seq {before} (now seq {now}).\n"
        )),
        "got: {result}"
    );
    assert_eq!(
        revision_count(&m, &identifier),
        1,
        "no row without a content change"
    );
}

#[test]
fn an_unknown_since_seq_falls_back_to_the_full_page_with_a_note() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "FBK");
    let identifier = seed_page(&m, "FBK", "first\n");
    let created = page_seq(&m, &identifier);
    edit(&m, &identifier, "first", "second");

    for unknown in [created - 1, 1_000_000] {
        let result = read_since(&m, &identifier, unknown);
        assert!(
            result.contains(&format!(
                "Note: seq {unknown} is outside this page's recorded history"
            )),
            "got: {result}"
        );
        assert!(
            result.ends_with("follows.\n\nsecond\n\n"),
            "the normal response follows: {result}"
        );
    }
}

#[test]
fn since_seq_cannot_be_combined_with_a_section_or_outline() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "CMB");
    let identifier = seed_page(&m, "CMB", NESTED);
    let result = m.get_page(Parameters(GetPageInput {
        identifier,
        section: Some("Beta".into()),
        since_seq: Some(1),
        ..Default::default()
    }));
    assert!(
        result.starts_with("Error: since_seq cannot be combined"),
        "got: {result}"
    );
}

#[test]
fn page_history_keeps_the_latest_fifty_versions() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "CAP");
    let identifier = seed_page(&m, "CAP", "version 0\n");
    let created = page_seq(&m, &identifier);
    for version in 1..=60 {
        edit(
            &m,
            &identifier,
            &format!("version {}\n", version - 1),
            &format!("version {version}\n"),
        );
    }
    assert_eq!(revision_count(&m, &identifier), 50);
    assert!(read_since(&m, &identifier, created).contains("outside this page's recorded history"));
}

#[test]
fn page_history_drops_old_versions_past_the_size_cap() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "SZC");
    let big = "x".repeat(400_000);
    let identifier = seed_page(&m, "SZC", &format!("v0\n{big}"));
    for version in 1..=6 {
        edit(
            &m,
            &identifier,
            &format!("v{}\n", version - 1),
            &format!("v{version}\n"),
        );
    }
    // Each version is just over 400,000 characters, so the newest four fit
    // in 2,000,000 and the older three are dropped.
    assert_eq!(revision_count(&m, &identifier), 4);
}

#[test]
fn page_history_is_deleted_with_the_page() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "DEL");
    let identifier = seed_page(&m, "DEL", "one\n");
    edit(&m, &identifier, "one", "two");
    assert_eq!(revision_count(&m, &identifier), 2);
    let id = m
        .read(|conn| queries::resolve_page_identifier(conn, &identifier))
        .unwrap();

    let conn = m.db.write().unwrap();
    conn.execute("DELETE FROM pages WHERE id = ?1", [id])
        .unwrap();
    let left: i64 = conn
        .query_row(
            "SELECT count(*) FROM page_revisions WHERE page_id = ?1",
            [id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(left, 0);
}
