//! LIF-477: module, label and folder names resolve regardless of case, and a
//! miss names the valid options, through the MCP tools agents actually call.

use super::tests::{mcp, seed_project};
use super::*;
use rmcp::handler::server::wrapper::Parameters;

fn seed_names(m: &LificMcp) {
    seed_project(m, "Lookup", "LKP");
    for (resource_type, name) in [
        ("module", "Backend"),
        ("module", "Web UI"),
        ("module", "Web"),
        ("module", "web"),
        ("label", "bug"),
        ("label", "Feature"),
        ("folder", "Design Notes"),
    ] {
        let created = m.manage_resource(Parameters(ManageResourceInput {
            resource_type: resource_type.into(),
            action: "create".into(),
            project: Some("LKP".into()),
            name: Some(name.into()),
            ..Default::default()
        }));
        assert!(created.starts_with("Created"), "got: {created}");
    }
}

fn issue_in(m: &LificMcp, identifier: &str) -> models::Issue {
    m.read(|conn| queries::get_issue(conn, queries::resolve_identifier(conn, identifier)?))
        .unwrap()
}

fn module_of(m: &LificMcp, issue: &models::Issue) -> String {
    m.read(|conn| queries::get_module_name(conn, issue.module_id.unwrap()))
        .unwrap()
}

#[test]
fn wrong_case_module_and_labels_resolve_to_the_stored_names() {
    let (m, _guard) = mcp();
    seed_names(&m);
    let created = m.create_issue(Parameters(CreateIssueInput {
        project: Some("LKP".into()),
        title: "Cased".into(),
        module: Some("backend".into()),
        labels: Some(vec!["BUG".into(), "feature".into()]),
        ..Default::default()
    }));
    assert!(created.starts_with("Created"), "got: {created}");
    let issue = issue_in(&m, "LKP-1");
    assert_eq!(module_of(&m, &issue), "Backend");
    let mut labels = issue.labels;
    labels.sort();
    assert_eq!(labels, vec!["Feature".to_string(), "bug".to_string()]);

    let listed = m.list_issues(Parameters(ListIssuesInput {
        project: Some("LKP".into()),
        label: Some("Bug".into()),
        ..Default::default()
    }));
    assert!(listed.contains("Cased"), "got: {listed}");
}

#[test]
fn exact_case_wins_when_names_differ_only_by_case() {
    let (m, _guard) = mcp();
    seed_names(&m);
    for (n, module) in ["Web", "web"].into_iter().enumerate() {
        let created = m.create_issue(Parameters(CreateIssueInput {
            project: Some("LKP".into()),
            title: format!("Issue {n}"),
            module: Some(module.into()),
            ..Default::default()
        }));
        assert!(created.starts_with("Created"), "got: {created}");
        let issue = issue_in(&m, &format!("LKP-{}", n + 1));
        assert_eq!(module_of(&m, &issue), module);
    }

    let ambiguous = m.create_issue(Parameters(CreateIssueInput {
        project: Some("LKP".into()),
        title: "Ambiguous".into(),
        module: Some("WEB".into()),
        ..Default::default()
    }));
    assert!(
        ambiguous.contains("matches more than one module by case ('Web', 'web')"),
        "got: {ambiguous}"
    );
}

#[test]
fn a_missing_module_names_the_valid_ones_closest_first() {
    let (m, _guard) = mcp();
    seed_names(&m);
    let result = m.create_issue(Parameters(CreateIssueInput {
        project: Some("LKP".into()),
        title: "Missing".into(),
        module: Some("Frontend".into()),
        ..Default::default()
    }));
    assert!(
        result.contains(
            "module 'Frontend' not found in project; valid modules: 'Backend', 'Web', 'web', 'Web UI'"
        ),
        "got: {result}"
    );
}

#[test]
fn a_missing_folder_or_label_names_the_valid_ones() {
    let (m, _guard) = mcp();
    seed_names(&m);
    let page = m.create_page(Parameters(CreatePageInput {
        project: Some("LKP".into()),
        title: "Doc".into(),
        folder: Some("Design".into()),
        ..Default::default()
    }));
    assert!(
        page.contains("folder 'Design' not found in project; valid folders: 'Design Notes'"),
        "got: {page}"
    );

    let deleted = m.delete(Parameters(DeleteInput {
        identifier: "bugs".into(),
        project: Some("LKP".into()),
        resource_type: "label".into(),
    }));
    assert!(
        deleted.contains("label 'bugs' not found in project; valid labels: 'bug', 'Feature'"),
        "got: {deleted}"
    );
}

#[test]
fn a_wrong_case_folder_resolves_for_a_page() {
    let (m, _guard) = mcp();
    seed_names(&m);
    let page = m.create_page(Parameters(CreatePageInput {
        project: Some("LKP".into()),
        title: "Doc".into(),
        folder: Some("design notes".into()),
        ..Default::default()
    }));
    assert!(page.starts_with("Created"), "got: {page}");
    let folder = m
        .read(|conn| {
            let page =
                queries::get_page(conn, queries::resolve_page_identifier(conn, "LKP-DOC-1")?)?;
            queries::get_folder_name(conn, page.folder_id.expect("page has a folder"))
        })
        .unwrap();
    assert_eq!(folder, "Design Notes");
}
