//! Relations named at creation (LIF-482) through the MCP `create_issue` tool.

use super::tests::{mcp, seed_issue, seed_project, setup_membership_mcp};
use super::*;
use rmcp::handler::server::wrapper::Parameters;

fn strings(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| (*id).to_string()).collect()
}

/// Every relation in the database, as `(source, type, target)` identifiers.
fn relations(m: &LificMcp) -> Vec<(String, String, String)> {
    m.read(|conn| {
        let mut statement = conn.prepare(
            "SELECT sp.identifier || '-' || s.sequence AS source, r.relation_type,
                    tp.identifier || '-' || t.sequence AS target
             FROM issue_relations r
             JOIN issues s ON s.id = r.source_id
             JOIN projects sp ON sp.id = s.project_id
             JOIN issues t ON t.id = r.target_id
             JOIN projects tp ON tp.id = t.project_id
             ORDER BY source, r.relation_type, target",
        )?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
    .unwrap()
}

fn relation(source: &str, kind: &str, target: &str) -> (String, String, String) {
    (source.into(), kind.into(), target.into())
}

fn issue_count(m: &LificMcp) -> i64 {
    m.read(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM issues", [], |row| row.get(0))?))
        .unwrap()
}

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

#[test]
fn create_issue_that_blocks_an_existing_issue_writes_the_relation() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");
    seed_issue(&m, "TST", "Ship the importer");

    let result = m.create_issue(Parameters(CreateIssueInput {
        project: Some("TST".into()),
        title: "Parse the legacy format".into(),
        blocks: Some(strings(&["TST-1"])),
        ..Default::default()
    }));

    assert_eq!(
        result,
        "Created TST-2: Parse the legacy format\nLinked: blocks TST-1"
    );
    assert_eq!(relations(&m), [relation("TST-2", "blocks", "TST-1")]);
    let blocked = m
        .read(|conn| queries::get_issue(conn, queries::resolve_identifier(conn, "TST-1")?))
        .unwrap();
    assert_eq!(
        blocked.blocked_by,
        ["TST-2"],
        "the existing issue now shows the new one as its blocker"
    );
}

#[test]
fn create_issue_writes_blocked_by_and_relates_to_in_their_directions() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");
    seed_issue(&m, "TST", "Design the schema");
    seed_issue(&m, "TST", "Billing export");
    seed_issue(&m, "TST", "Audit log retention");

    let result = m.create_issue(Parameters(CreateIssueInput {
        project: Some("TST".into()),
        title: "Write the migration".into(),
        blocked_by: Some(strings(&["TST-1"])),
        relates_to: Some(strings(&["TST-2", "TST-3"])),
        ..Default::default()
    }));

    assert_eq!(
        result,
        "Created TST-4: Write the migration\nLinked: blocked_by TST-1; relates_to TST-2, TST-3"
    );
    assert_eq!(
        relations(&m),
        [
            relation("TST-1", "blocks", "TST-4"),
            relation("TST-4", "relates_to", "TST-2"),
            relation("TST-4", "relates_to", "TST-3"),
        ]
    );
}

#[test]
fn create_issue_with_an_unknown_relation_creates_nothing_and_names_the_reference() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");
    seed_issue(&m, "TST", "Ship the importer");

    let missing = m.create_issue(Parameters(CreateIssueInput {
        project: Some("TST".into()),
        title: "Parse the legacy format".into(),
        blocks: Some(strings(&["TST-1", "TST-99"])),
        ..Default::default()
    }));

    assert_eq!(
        missing,
        "Error: Not found: blocks[1]: issue TST-99 not found"
    );
    assert_eq!(issue_count(&m), 1, "the new issue was not created");
    assert!(relations(&m).is_empty());
}

#[test]
fn create_issue_does_not_offer_an_issue_it_just_linked_as_a_duplicate() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");
    seed_issue(&m, "TST", "Search ranking ignores empty titles");

    let result = m.create_issue(Parameters(CreateIssueInput {
        project: Some("TST".into()),
        title: "Search ranking breaks on long queries".into(),
        relates_to: Some(strings(&["TST-1"])),
        ..Default::default()
    }));

    assert_eq!(
        result,
        "Created TST-2: Search ranking breaks on long queries\nLinked: relates_to TST-1"
    );
}

#[test]
fn create_issue_batch_links_items_to_each_other_by_index_and_title() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");
    seed_issue(&m, "TST", "Release 3.0");

    let result = create_batch(
        &m,
        vec![
            CreateIssueItem {
                blocks: Some(strings(&["issues[1]", "TST-1"])),
                ..item("Design the schema")
            },
            CreateIssueItem {
                blocks: Some(strings(&["TST-1"])),
                relates_to: Some(strings(&["write the docs"])),
                ..item("Write the migration")
            },
            CreateIssueItem {
                blocked_by: Some(strings(&["0"])),
                ..item("Write the docs")
            },
        ],
    );

    assert_eq!(
        result,
        "Created 3 issues:\n\
         - TST-2: Design the schema (blocks TST-3, TST-1)\n\
         - TST-3: Write the migration (blocks TST-1; relates_to TST-4)\n\
         - TST-4: Write the docs (blocked_by TST-2)"
    );
    assert_eq!(
        relations(&m),
        [
            relation("TST-2", "blocks", "TST-1"),
            relation("TST-2", "blocks", "TST-3"),
            relation("TST-2", "blocks", "TST-4"),
            relation("TST-3", "blocks", "TST-1"),
            relation("TST-3", "relates_to", "TST-4"),
        ]
    );
}

#[test]
fn create_issue_batch_refuses_bad_item_references_and_creates_nothing() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");

    let out_of_range = create_batch(
        &m,
        vec![
            item("Design the schema"),
            CreateIssueItem {
                blocks: Some(strings(&["issues[5]"])),
                ..item("Write the migration")
            },
        ],
    );
    assert_eq!(
        out_of_range,
        "Error: Bad request: issues[1]: blocks[0]: this batch has no issues[5]"
    );

    let itself = create_batch(
        &m,
        vec![CreateIssueItem {
            relates_to: Some(strings(&["Design the schema"])),
            ..item("Design the schema")
        }],
    );
    assert_eq!(
        itself,
        "Error: Bad request: issues[0]: relates_to[0]: an issue cannot be linked to itself"
    );

    let ambiguous = create_batch(
        &m,
        vec![
            item("Write the docs"),
            item("Write the docs"),
            CreateIssueItem {
                blocked_by: Some(strings(&["Write the docs"])),
                ..item("Publish")
            },
        ],
    );
    assert!(
        ambiguous.contains("issues[2]: blocked_by[0]")
            && ambiguous.contains("issues[0] and issues[1]"),
        "got: {ambiguous}"
    );

    let unknown_title = create_batch(
        &m,
        vec![CreateIssueItem {
            blocks: Some(strings(&["No such item"])),
            ..item("Design the schema")
        }],
    );
    assert_eq!(
        unknown_title,
        "Error: Bad request: issues[0]: blocks[0]: \"No such item\" is not an issue ID, or another item's title or index"
    );

    assert_eq!(issue_count(&m), 0);
    assert!(relations(&m).is_empty());
}

#[test]
fn create_issue_batch_refuses_relations_at_the_top_level() {
    let (m, _guard) = mcp();
    seed_project(&m, "Test", "TST");
    seed_issue(&m, "TST", "Release 3.0");

    let result = m.create_issue(Parameters(CreateIssueInput {
        project: Some("TST".into()),
        blocks: Some(strings(&["TST-1"])),
        issues: Some(vec![item("Design the schema")]),
        ..Default::default()
    }));

    assert!(result.contains("only project applies"), "got: {result}");
    assert_eq!(issue_count(&m), 1);
}

#[test]
fn create_issue_input_accepts_relation_lists_on_the_wire() {
    let single: CreateIssueInput = serde_json::from_value(serde_json::json!({
        "title": "One",
        "blocks": ["TST-1"],
        "blocked_by": ["TST-2"],
        "relates_to": ["TST-3"]
    }))
    .unwrap();
    assert_eq!(single.blocks, Some(strings(&["TST-1"])));
    assert_eq!(single.blocked_by, Some(strings(&["TST-2"])));
    assert_eq!(single.relates_to, Some(strings(&["TST-3"])));

    let batch: CreateIssueInput = serde_json::from_value(serde_json::json!({
        "issues": [{ "title": "One" }, { "title": "Two", "blocked_by": ["0"] }]
    }))
    .unwrap();
    let items = batch.issues.unwrap();
    assert_eq!(items[1].blocked_by, Some(strings(&["0"])));
}

/// Linking needs Maintainer on the other issue's project too, as
/// `link_issues` does. A Viewer there can read the issue but not link it.
#[test]
fn create_issue_refuses_a_relation_into_a_project_the_caller_cannot_maintain() {
    let (m, _admin, _lead, maintainer, _viewer, _non_member, _project_id, _guard) =
        setup_membership_mcp();
    m.write(|conn| {
        let other = queries::create_project(
            conn,
            &models::CreateProject {
                name: "Other".into(),
                identifier: "OTH".into(),
                ..Default::default()
            },
        )?;
        queries::members::upsert_member(conn, other.id, maintainer.id, models::Role::Viewer)?;
        queries::create_issue(
            conn,
            &models::CreateIssue {
                project_id: other.id,
                title: "Elsewhere".into(),
                ..Default::default()
            },
        )
    })
    .unwrap();

    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(crate::mcp::with_request_user(Some(maintainer), || async {
            m.create_issue(Parameters(CreateIssueInput {
                project: Some("MEM".into()),
                title: "Blocks something elsewhere".into(),
                blocks: Some(strings(&["OTH-1"])),
                ..Default::default()
            }))
        }));

    assert!(result.starts_with("Error: Forbidden"), "got: {result}");
    assert_eq!(issue_count(&m), 1, "only the seeded OTH-1 exists");
    assert!(relations(&m).is_empty());
}
