//! LIF-524: each project's conventions page over MCP: created with the
//! project, delivered first by the briefing, marked in reads and listings,
//! boosted modestly in search, and never called anything but conventions.

use super::briefing::{BUDGET_CHARS, CONVENTIONS_CHARS};
use super::tests::mcp;
use super::*;
use crate::conventions::Template;
use rmcp::handler::server::wrapper::Parameters;

fn create_project(m: &LificMcp, ident: &str, conventions: Option<&str>) -> String {
    m.manage_resource(Parameters(ManageResourceInput {
        resource_type: "project".into(),
        action: "create".into(),
        name: Some(format!("Project {ident}")),
        identifier: Some(ident.into()),
        conventions: conventions.map(Into::into),
        ..Default::default()
    }))
}

fn update_conventions(m: &LificMcp, ident: &str, conventions: &str) -> String {
    m.manage_resource(Parameters(ManageResourceInput {
        resource_type: "project".into(),
        action: "update".into(),
        project: Some(ident.into()),
        conventions: Some(conventions.into()),
        ..Default::default()
    }))
}

/// A project made the way pre-conventions Lific made them: no page, no row.
fn legacy_project(m: &LificMcp, ident: &str) -> i64 {
    m.write(|conn| {
        Ok(queries::create_project(
            conn,
            &models::CreateProject {
                name: format!("Legacy {ident}"),
                identifier: ident.into(),
                ..Default::default()
            },
        )?
        .id)
    })
    .unwrap()
}

fn project_id(m: &LificMcp, ident: &str) -> i64 {
    m.read(|conn| queries::resolve_project_identifier(conn, ident))
        .unwrap()
}

fn conventions(m: &LificMcp, ident: &str) -> Option<models::Page> {
    let pid = project_id(m, ident);
    m.read(|conn| queries::conventions_page(conn, pid)).unwrap()
}

fn set_content(m: &LificMcp, identifier: &str, content: &str) {
    let out = m.update_page(Parameters(UpdatePageInput {
        identifier: identifier.into(),
        content: Some(content.into()),
        ..Default::default()
    }));
    assert!(!out.starts_with("Error"), "got: {out}");
}

fn briefing(m: &LificMcp, ident: &str) -> String {
    m.get_briefing(Parameters(GetBriefingInput {
        project: Some(ident.into()),
        ..Default::default()
    }))
    .text
}

fn create_page(m: &LificMcp, ident: &str, title: &str, conventions: bool) -> String {
    m.create_page(Parameters(CreatePageInput {
        project: Some(ident.into()),
        title: title.into(),
        content: Some("Close issues with a comment.".into()),
        conventions: conventions.then_some(true),
        ..Default::default()
    }))
}

fn delete_page(m: &LificMcp, identifier: &str) {
    let out = m.delete(Parameters(DeleteInput {
        resource_type: "page".into(),
        identifier: identifier.into(),
        ..Default::default()
    }));
    assert!(!out.starts_with("Error"), "got: {out}");
}

const HINT: &str = "No conventions page yet.";

/// The briefing's text and its structured response, for absence checks.
fn briefing_everywhere(m: &LificMcp, ident: &str) -> String {
    let out = m.get_briefing(Parameters(GetBriefingInput {
        project: Some(ident.into()),
        ..Default::default()
    }));
    format!("{}\n{}", out.text, out.structured.unwrap())
}

#[test]
fn creating_a_project_with_each_template_starts_its_conventions_page() {
    let (m, _guard) = mcp();
    for (ident, value, template) in [
        ("AIM", Some("ai_managed"), Template::AiManaged),
        ("HMG", Some("human_managed"), Template::HumanManaged),
        ("HOW", Some("human_owned"), Template::HumanOwned),
        ("EMP", Some("empty"), Template::Empty),
        ("DEF", None, Template::Empty),
    ] {
        let out = create_project(&m, ident, value);
        assert!(out.starts_with("Created project"), "got: {out}");
        assert!(
            out.contains(&format!("Conventions page {ident}-DOC-1")),
            "{out}"
        );
        let page = conventions(&m, ident).expect("conventions page");
        assert_eq!(page.content, template.content(), "{ident}");
        assert_eq!(page.title, crate::conventions::PAGE_TITLE);
        assert!(page.conventions && page.conventions_enabled);
    }
}

#[test]
fn an_unknown_or_none_conventions_value_is_refused_on_create() {
    let (m, _guard) = mcp();
    for value in ["agents", "none"] {
        let out = create_project(&m, "BAD", Some(value));
        assert!(out.starts_with("Error"), "{value}: {out}");
        assert!(out.contains("ai_managed"), "{out}");
    }
}

#[test]
fn a_briefing_prints_a_non_empty_conventions_page_first() {
    let (m, _guard) = mcp();
    create_project(&m, "BRC", Some("ai_managed"));
    // An issue for a person puts the human queue in the briefing too.
    let filed = m.create_issue(Parameters(CreateIssueInput {
        project: Some("BRC".into()),
        title: "Pick a hosting plan".into(),
        assignees: Some(vec!["human".into()]),
        ..Default::default()
    }));
    assert!(filed.starts_with("Created"), "{filed}");

    let out = briefing(&m, "BRC");
    let conventions_at = out
        .find("\n## Conventions for BRC (BRC-DOC-1)")
        .unwrap_or_else(|| panic!("{out}"));
    let queue_at = out.find("\n## Human queue").expect("human queue");
    assert!(conventions_at < queue_at, "{out}");
    assert!(
        out.contains("The rules for working with this project's issues, pages and plans."),
        "{out}"
    );
    // The page's own headings sit below the briefing's.
    assert!(out.contains("\n### Filing\n"), "{out}");
    assert!(out.contains("One issue, one change."), "{out}");
    assert!(!out.contains(HINT), "{out}");

    let structured = m
        .get_briefing(Parameters(GetBriefingInput {
            project: Some("BRC".into()),
            ..Default::default()
        }))
        .structured
        .unwrap();
    assert_eq!(structured["sections"][0]["section"], "conventions");
    assert_eq!(
        structured["sections"][0]["items"][0]["identifier"],
        "BRC-DOC-1"
    );
}

#[test]
fn an_empty_off_or_deleted_conventions_page_puts_nothing_in_the_briefing() {
    let (m, _guard) = mcp();
    create_project(&m, "OFF", None);
    let empty = briefing_everywhere(&m, "OFF");
    assert!(!empty.contains("OFF-DOC-1"), "{empty}");
    assert!(!empty.contains("Conventions"), "{empty}");
    assert!(!empty.contains(HINT), "{empty}");
    // Asked for by name, it is still listed like any page.
    let named = m.get_briefing(Parameters(GetBriefingInput {
        project: Some("OFF".into()),
        pages: Some(vec!["OFF-DOC-1".into()]),
        ..Default::default()
    }));
    assert!(
        named.text.contains("OFF-DOC-1 Conventions"),
        "{}",
        named.text
    );

    set_content(&m, "OFF-DOC-1", "Close issues with a comment.");
    assert!(briefing(&m, "OFF").contains("## Conventions for OFF"));

    let page = conventions(&m, "OFF").unwrap();
    m.write(|conn| {
        queries::update_page(
            conn,
            page.id,
            &models::UpdatePage {
                conventions_enabled: Some(false),
                ..Default::default()
            },
        )
    })
    .unwrap();
    let off = briefing_everywhere(&m, "OFF");
    assert!(!off.contains("OFF-DOC-1"), "{off}");
    assert!(!off.contains("Conventions"), "{off}");
    assert!(!off.contains("Close issues with a comment."), "{off}");

    delete_page(&m, "OFF-DOC-1");
    let deleted = briefing_everywhere(&m, "OFF");
    assert!(!deleted.contains("OFF-DOC-1"), "{deleted}");
    assert!(
        !deleted.contains(HINT),
        "a deleted page is a choice: {deleted}"
    );
}

#[test]
fn an_oversized_conventions_page_is_capped_with_a_get_page_pointer() {
    let (m, _guard) = mcp();
    create_project(&m, "BIG", None);
    let rule = "- Every issue names the change that closes it and the proof.\n";
    let content = rule.repeat(CONVENTIONS_CHARS / rule.len() * 2);
    set_content(&m, "BIG-DOC-1", &content);

    let out = briefing(&m, "BIG");
    assert!(out.contains("## Conventions for BIG"), "{out}");
    let total = content.trim().chars().count();
    assert!(
        out.contains(&format!(
            " of {total} characters. Read the rest with get_page(identifier='BIG-DOC-1').)"
        )),
        "{out}"
    );
    // Cut at a line break, inside the cap.
    let shown = out.matches("closes it and the proof.").count();
    assert!(
        shown * rule.len() <= CONVENTIONS_CHARS,
        "{shown} rules shown"
    );
    assert!(shown > 0);
    assert!(out.chars().count() <= BUDGET_CHARS + CONVENTIONS_CHARS + 400);
}

#[test]
fn a_project_without_a_page_gets_a_hint_until_it_has_one_or_dismisses_it() {
    let (m, _guard) = mcp();
    legacy_project(&m, "OLD");
    let out = briefing(&m, "OLD");
    assert!(
        out.contains(
            "\nNo conventions page yet. Ask the user how agents should work here, then add one \
             with manage_resource, or set conventions='none' to stop this reminder.\n"
        ),
        "{out}"
    );
    assert_eq!(out.matches(HINT).count(), 1);

    let created = update_conventions(&m, "OLD", "human_owned");
    assert!(
        created.contains("Created conventions page OLD-DOC-1"),
        "{created}"
    );
    assert_eq!(
        conventions(&m, "OLD").unwrap().content,
        Template::HumanOwned.content()
    );
    let after = briefing(&m, "OLD");
    assert!(!after.contains(HINT), "{after}");
    assert!(after.contains("## Conventions for OLD"), "{after}");

    let again = update_conventions(&m, "OLD", "ai_managed");
    assert!(again.starts_with("Error"), "{again}");
    assert!(again.contains("OLD-DOC-1"), "{again}");

    legacy_project(&m, "DIS");
    let dismissed = update_conventions(&m, "DIS", "none");
    assert!(dismissed.contains("no longer suggest"), "{dismissed}");
    assert!(!briefing(&m, "DIS").contains(HINT));
    assert!(conventions(&m, "DIS").is_none());
}

#[test]
fn create_page_makes_a_conventions_page_only_when_the_project_has_none() {
    let (m, _guard) = mcp();
    create_project(&m, "CPG", None);

    let refused = create_page(&m, "CPG", "Second rules", true);
    assert!(refused.starts_with("Error"), "{refused}");
    assert!(refused.contains("CPG-DOC-1"), "{refused}");

    delete_page(&m, "CPG-DOC-1");
    let created = create_page(&m, "CPG", "House rules", true);
    assert!(created.contains("conventions page"), "{created}");
    let page = conventions(&m, "CPG").unwrap();
    assert_eq!(page.title, "House rules");
    assert_eq!(page.identifier, "CPG-DOC-2");

    // With a replacement made, restoring the old one brings it back as an
    // ordinary page.
    let old = m
        .read(|conn| {
            Ok(conn.query_row(
                "SELECT id FROM pages WHERE title = 'Conventions'",
                [],
                |row| row.get::<_, i64>(0),
            )?)
        })
        .unwrap();
    let restored = m.write(|conn| queries::restore_page(conn, old)).unwrap();
    assert!(!restored.conventions);
    assert_eq!(conventions(&m, "CPG").unwrap().identifier, "CPG-DOC-2");

    let plain = create_page(&m, "CPG", "Notes", false);
    assert!(!plain.contains("conventions page"), "{plain}");

    let workspace = m.create_page(Parameters(CreatePageInput {
        title: "Workspace rules".into(),
        conventions: Some(true),
        ..Default::default()
    }));
    assert!(workspace.starts_with("Error"), "{workspace}");
}

#[test]
fn restoring_a_deleted_conventions_page_with_no_replacement_keeps_its_role() {
    let (m, _guard) = mcp();
    create_project(&m, "RST", None);
    let id = conventions(&m, "RST").unwrap().id;
    delete_page(&m, "RST-DOC-1");
    assert!(conventions(&m, "RST").is_none());
    let restored = m.write(|conn| queries::restore_page(conn, id)).unwrap();
    assert!(restored.conventions);
    assert_eq!(conventions(&m, "RST").unwrap().id, id);
}

#[test]
fn get_page_and_page_listings_mark_the_conventions_page() {
    let (m, _guard) = mcp();
    create_project(&m, "MRK", Some("human_managed"));
    create_page(&m, "MRK", "Notes", false);

    let page = m.get_page(Parameters(GetPageInput {
        identifier: "MRK-DOC-1".into(),
        ..Default::default()
    }));
    assert!(
        page.contains("This project's conventions page: the rules for working with its issues, pages and plans."),
        "{page}"
    );
    let other = m.get_page(Parameters(GetPageInput {
        identifier: "MRK-DOC-2".into(),
        ..Default::default()
    }));
    assert!(!other.contains("conventions page"), "{other}");

    let listing = m.list_resources(Parameters(ListResourcesInput {
        resource_type: "page".into(),
        project: Some("MRK".into()),
        ..Default::default()
    }));
    let row = listing
        .text
        .lines()
        .find(|line| line.contains("MRK-DOC-1"))
        .unwrap();
    assert!(row.contains("(this project's conventions page)"), "{row}");
    let notes = listing
        .text
        .lines()
        .find(|line| line.contains("MRK-DOC-2"))
        .unwrap();
    assert!(!notes.contains("conventions"), "{notes}");
    let items = &listing.structured.unwrap()["items"];
    let flagged: Vec<_> = items
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["conventions"] == true)
        .map(|item| item["identifier"].clone())
        .collect();
    assert_eq!(flagged, ["MRK-DOC-1"]);
}

/// Hits for `query` in project `ident`, best first, as identifiers.
fn search_ids(m: &LificMcp, ident: &str, query: &str) -> Vec<String> {
    m.search(Parameters(SearchInput {
        query: query.into(),
        project: Some(ident.into()),
        ..Default::default()
    }))
    .structured
    .unwrap()["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hit| hit["identifier"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Plain BM25 order for `query` in a project, without the conventions boost.
fn unboosted_ids(m: &LificMcp, ident: &str, query: &str) -> Vec<String> {
    let pid = project_id(m, ident);
    m.read(|conn| {
        let mut stmt = conn.prepare(
            "SELECT CASE s.entity_type WHEN 'page' THEN p.identifier || '-DOC-' || pg.sequence
                                       ELSE p.identifier || '-' || i.sequence END
               FROM search_index s
               JOIN projects p ON p.id = s.project_id
               LEFT JOIN issues i ON s.entity_type = 'issue' AND i.id = s.entity_id
               LEFT JOIN pages pg ON s.entity_type = 'page' AND pg.id = s.entity_id
              WHERE search_index MATCH ?1 AND s.project_id = ?2
              ORDER BY rank",
        )?;
        let expression =
            queries::fts_expression(query, queries::Terms::All, "{title body}").unwrap();
        let rows = stmt.query_map(rusqlite::params![expression, pid], |row| row.get(0))?;
        Ok(rows.collect::<Result<Vec<String>, _>>()?)
    })
    .unwrap()
}

fn issue_with(m: &LificMcp, ident: &str, title: &str, description: &str) {
    let out = m.create_issue(Parameters(CreateIssueInput {
        project: Some(ident.into()),
        title: title.into(),
        description: Some(description.into()),
        ..Default::default()
    }));
    assert!(out.starts_with("Created"), "{out}");
}

/// A project with ordinary issues around, so term rarity means something.
fn search_project(m: &LificMcp, ident: &str) {
    create_project(m, ident, Some("ai_managed"));
    for n in 0..20 {
        issue_with(
            m,
            ident,
            &format!("Filler work item {n}"),
            "Some ordinary words about the product and the people who use it.",
        );
    }
}

#[test]
fn search_ranks_the_conventions_page_first_when_it_covers_the_query() {
    let (m, _guard) = mcp();
    search_project(&m, "SRC");
    // An ordinary issue that mentions the rule in passing. Plain BM25 puts
    // it ahead of the longer conventions page; the boost turns that around.
    issue_with(
        &m,
        "SRC",
        "Move the docs site to the new host",
        "The docs site moves to the new host once DNS is switched over. Until the DNS \
         change lands this waits on the networking issue, so it gets a blocker link to \
         it. After the move, check every redirect from the old paths, rebuild the search \
         index, confirm the TLS certificate renews on its own, update the address in the \
         README and the footer, and post the new address where the old one was shared.",
    );
    let unboosted = unboosted_ids(&m, "SRC", "blocker link");
    assert_eq!(
        unboosted,
        ["SRC-21", "SRC-DOC-1"],
        "the boost is what decides"
    );

    let hits = search_ids(&m, "SRC", "blocker link");
    assert_eq!(hits, ["SRC-DOC-1", "SRC-21"]);
    // A term only the conventions page uses.
    assert_eq!(search_ids(&m, "SRC", "credential"), ["SRC-DOC-1"]);
}

#[test]
fn search_keeps_the_conventions_page_below_an_issue_it_does_not_cover() {
    let (m, _guard) = mcp();
    search_project(&m, "TTL");
    issue_with(
        &m,
        "TTL",
        "Login timeout on Safari",
        "Sessions expire after a minute.",
    );
    issue_with(
        &m,
        "TTL",
        "Blocker link for the deploy",
        "Add the blocker link.",
    );

    // An unrelated issue title finds that issue, not the conventions page.
    let unrelated = search_ids(&m, "TTL", "login timeout on safari");
    assert_eq!(
        unrelated.first().map(String::as_str),
        Some("TTL-21"),
        "{unrelated:?}"
    );
    assert!(
        !unrelated.iter().any(|hit| hit == "TTL-DOC-1"),
        "{unrelated:?}"
    );

    // Modest, not a pin: an issue about exactly the query stays on top.
    assert_eq!(
        search_ids(&m, "TTL", "blocker link"),
        ["TTL-22", "TTL-DOC-1"]
    );

    // Turned off, the page gets no boost at all.
    let page = conventions(&m, "TTL").unwrap();
    m.write(|conn| {
        queries::update_page(
            conn,
            page.id,
            &models::UpdatePage {
                conventions_enabled: Some(false),
                ..Default::default()
            },
        )
    })
    .unwrap();
    assert_eq!(
        search_ids(&m, "TTL", "blocker link"),
        unboosted_ids(&m, "TTL", "blocker link")
    );
}

#[test]
fn nothing_an_agent_reads_calls_the_page_anything_but_conventions() {
    let (m, _guard) = mcp();
    create_project(&m, "WRD", Some("ai_managed"));
    legacy_project(&m, "WRL");

    let mut surface = vec![crate::mcp::SERVER_INSTRUCTIONS.to_string()];
    for tool in m.tool_router.list_all() {
        surface.push(tool.description.unwrap_or_default().to_string());
        surface.push(serde_json::to_string(&tool.input_schema).unwrap());
    }
    surface.push(briefing(&m, "WRD"));
    surface.push(briefing(&m, "WRL"));
    surface.push(m.get_page(Parameters(GetPageInput {
        identifier: "WRD-DOC-1".into(),
        ..Default::default()
    })));
    for template in Template::ALL {
        surface.push(template.content().to_string());
        surface.push(template.summary().to_string());
    }
    for text in &surface {
        assert!(!text.to_lowercase().contains("agents.md"), "{text}");
    }
    let manage = m
        .tool_router
        .list_all()
        .into_iter()
        .find(|tool| tool.name == "manage_resource")
        .unwrap();
    assert!(
        manage
            .description
            .unwrap()
            .contains("ask the user how it will work with agents"),
    );
    assert!(crate::mcp::SERVER_INSTRUCTIONS.contains("conventions page"));
}
