//! Agent-input tolerance for the MCP tools: camelCase edit keys (LIF-472)
//! and HTML-entity-escaped names (LIF-473).

use super::tests::{comment_id_from, mcp, seed_issue, seed_project};
use super::*;
use rmcp::handler::server::wrapper::Parameters;
use serde_json::json;

/// Deserialize exactly as the MCP transport does, from the raw JSON object.
fn parse<T: serde::de::DeserializeOwned>(arguments: serde_json::Value) -> T {
    serde_json::from_value(arguments).expect("arguments deserialize")
}

// ── LIF-472: camelCase oldString/newString/replaceAll ────────

#[test]
fn edit_issue_accepts_camel_case_keys() {
    let (m, _guard) = mcp();
    seed_project(&m, "Camel", "CML");
    m.create_issue(Parameters(CreateIssueInput {
        project: Some("CML".into()),
        title: "Camel".into(),
        description: Some("one fish, one fish".into()),
        ..Default::default()
    }));

    let edited = m.edit_issue(Parameters(parse(json!({
        "identifier": "CML-1",
        "oldString": "one",
        "newString": "two",
        "replaceAll": true,
    }))));
    assert!(!edited.starts_with("Error"), "got: {edited}");
    let issue = m
        .read(|conn| queries::get_issue(conn, queries::resolve_identifier(conn, "CML-1")?))
        .unwrap();
    assert_eq!(issue.description, "two fish, two fish");
}

#[test]
fn edit_page_accepts_camel_case_keys() {
    let (m, _guard) = mcp();
    seed_project(&m, "Camel", "CML");
    m.create_page(Parameters(CreatePageInput {
        project: Some("CML".into()),
        title: "Notes".into(),
        content: Some("draft text".into()),
        ..Default::default()
    }));

    let edited = m.edit_page(Parameters(parse(json!({
        "identifier": "CML-DOC-1",
        "oldString": "draft",
        "newString": "final",
    }))));
    assert!(!edited.starts_with("Error"), "got: {edited}");
    let page = m
        .read(|conn| queries::get_page(conn, queries::resolve_page_identifier(conn, "CML-DOC-1")?))
        .unwrap();
    assert_eq!(page.content, "final text");
}

#[test]
fn edit_comment_accepts_camel_case_keys() {
    let (m, _guard) = mcp();
    seed_project(&m, "Camel", "CML");
    seed_issue(&m, "CML", "Commented");
    let added = m.add_comment(Parameters(AddCommentInput {
        identifier: "CML-1".into(),
        content: "a b a".into(),
    }));
    let cid = comment_id_from(&added);

    let edited = m.edit_comment(Parameters(parse(json!({
        "comment_id": cid,
        "oldString": "a",
        "newString": "c",
        "replaceAll": true,
    }))));
    assert!(!edited.starts_with("Error"), "got: {edited}");
    let body = m
        .read(|conn| queries::comments::get_comment(conn, cid))
        .unwrap()
        .content;
    assert_eq!(body, "c b c");
}

#[test]
fn edit_plan_step_accepts_camel_case_keys() {
    let (m, _guard) = mcp();
    seed_project(&m, "Camel", "CML");
    let created = m.create_plan(Parameters(CreatePlanInput {
        project: Some("CML".into()),
        title: "Plan".into(),
        steps: Some(vec![PlanStepInput {
            title: "Write the draft".into(),
            ..Default::default()
        }]),
        ..Default::default()
    }));
    let step_id: i64 = created
        .split_once(" Write the draft")
        .and_then(|(head, _)| head.rsplit_once('#'))
        .and_then(|(_, id)| id.parse().ok())
        .unwrap_or_else(|| panic!("no step id in: {created}"));

    let edited = m.edit_plan_step(Parameters(parse(json!({
        "plan": "CML-PLAN-1",
        "step_id": step_id,
        "field": "title",
        "oldString": "draft",
        "newString": "final",
    }))));
    assert!(edited.contains("Edited step"), "got: {edited}");
    let plan = m.get_plan(Parameters(GetPlanInput {
        plan: "CML-PLAN-1".into(),
    }));
    assert!(plan.contains("Write the final"), "got: {plan}");
}

#[test]
fn edit_inputs_still_accept_snake_case_keys() {
    let snake: EditIssueInput = parse(json!({
        "identifier": "CML-1",
        "old_string": "a",
        "new_string": "b",
        "replace_all": true,
    }));
    assert_eq!(
        (snake.old_string.as_str(), snake.new_string.as_str()),
        ("a", "b")
    );
    assert_eq!(snake.replace_all, Some(true));
}

// ── LIF-473: HTML-entity-escaped names ───────────────────────

/// A project holding a module, label and folder whose names need escaping,
/// plus a label that literally contains an entity.
fn seed_escapable_names(m: &LificMcp) {
    seed_project(m, "Names", "NAM");
    for (resource_type, name) in [
        ("module", "Infra & Ops"),
        ("module", "Spare"),
        ("label", "R&D"),
        ("label", "Q&A"),
        ("label", "Q&amp;A"),
        ("folder", "Specs <draft>"),
        ("folder", "Spare"),
    ] {
        let created = m.manage_resource(Parameters(ManageResourceInput {
            resource_type: resource_type.into(),
            action: "create".into(),
            project: Some("NAM".into()),
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

fn page_in(m: &LificMcp, identifier: &str) -> models::Page {
    m.read(|conn| queries::get_page(conn, queries::resolve_page_identifier(conn, identifier)?))
        .unwrap()
}

#[test]
fn issue_writes_resolve_escaped_module_and_label_names() {
    let (m, _guard) = mcp();
    seed_escapable_names(&m);
    let created = m.create_issue(Parameters(CreateIssueInput {
        project: Some("NAM".into()),
        title: "Escaped".into(),
        module: Some("Infra &amp; Ops".into()),
        labels: Some(vec!["R&amp;D".into()]),
        ..Default::default()
    }));
    assert!(created.starts_with("Created"), "got: {created}");
    let issue = issue_in(&m, "NAM-1");
    let module = m.read(|conn| queries::get_module_name(conn, issue.module_id.unwrap()));
    assert_eq!(module.unwrap(), "Infra & Ops");
    assert_eq!(issue.labels, vec!["R&D".to_string()]);

    m.update_issue(Parameters(UpdateIssueInput {
        identifier: "NAM-1".into(),
        module: Some("Spare".into()),
        labels: Some(vec![]),
        ..Default::default()
    }));
    let updated = m.update_issue(Parameters(UpdateIssueInput {
        identifier: "NAM-1".into(),
        module: Some("infra &amp; ops".into()),
        labels: Some(vec!["R&amp;D".into()]),
        ..Default::default()
    }));
    assert!(updated.starts_with("Updated"), "got: {updated}");
    let issue = issue_in(&m, "NAM-1");
    let module = m.read(|conn| queries::get_module_name(conn, issue.module_id.unwrap()));
    assert_eq!(module.unwrap(), "Infra & Ops");
    assert_eq!(issue.labels, vec!["R&D".to_string()]);
}

#[test]
fn issue_filters_resolve_escaped_module_and_label_names() {
    let (m, _guard) = mcp();
    seed_escapable_names(&m);
    m.create_issue(Parameters(CreateIssueInput {
        project: Some("NAM".into()),
        title: "Tagged".into(),
        module: Some("Infra & Ops".into()),
        labels: Some(vec!["R&D".into()]),
        ..Default::default()
    }));
    seed_issue(&m, "NAM", "Untagged");

    for listing in [
        m.list_issues(Parameters(ListIssuesInput {
            project: Some("NAM".into()),
            module: Some("Infra &amp; Ops".into()),
            ..Default::default()
        })),
        m.list_issues(Parameters(ListIssuesInput {
            project: Some("NAM".into()),
            label: Some("R&amp;D".into()),
            ..Default::default()
        })),
    ] {
        assert!(listing.contains("Tagged"), "got: {listing}");
        assert!(!listing.contains("Untagged"), "got: {listing}");
    }

    let bulk = m.bulk_update(Parameters(BulkUpdateInput {
        project: "NAM".into(),
        filter_module: Some("Infra &amp; Ops".into()),
        filter_label: Some("R&amp;D".into()),
        set_status: Some("done".into()),
        ..Default::default()
    }));
    assert_eq!(bulk, "Updated 1 issue(s)");
    assert_eq!(issue_in(&m, "NAM-1").status.as_str(), "done");

    let moved = m.bulk_update(Parameters(BulkUpdateInput {
        project: "NAM".into(),
        filter_status: Some("backlog".into()),
        set_module: Some("Infra &amp; Ops".into()),
        ..Default::default()
    }));
    assert_eq!(moved, "Updated 1 issue(s)");
    assert!(issue_in(&m, "NAM-2").module_id.is_some());
}

#[test]
fn page_writes_and_listings_resolve_escaped_folder_and_label_names() {
    let (m, _guard) = mcp();
    seed_escapable_names(&m);
    let created = m.create_page(Parameters(CreatePageInput {
        project: Some("NAM".into()),
        title: "Escaped page".into(),
        folder: Some("Specs &lt;draft&gt;".into()),
        labels: Some(vec!["R&amp;D".into()]),
        ..Default::default()
    }));
    assert!(created.starts_with("Created"), "got: {created}");
    let page = page_in(&m, "NAM-DOC-1");
    let specs = m
        .read(|conn| queries::resolve_folder_name(conn, project_id(&m), "Specs <draft>"))
        .unwrap();
    assert_eq!(page.folder_id, Some(specs));
    assert_eq!(page.labels, vec!["R&D".to_string()]);

    m.update_page(Parameters(UpdatePageInput {
        identifier: "NAM-DOC-1".into(),
        folder: Some("Spare".into()),
        labels: Some(vec![]),
        ..Default::default()
    }));
    let updated = m.update_page(Parameters(UpdatePageInput {
        identifier: "NAM-DOC-1".into(),
        folder: Some("Specs &lt;draft&gt;".into()),
        labels: Some(vec!["R&amp;D".into()]),
        ..Default::default()
    }));
    assert!(updated.starts_with("Updated"), "got: {updated}");
    let page = page_in(&m, "NAM-DOC-1");
    assert_eq!(page.folder_id, Some(specs));
    assert_eq!(page.labels, vec!["R&D".to_string()]);

    for (project, folder, label) in [
        (Some("NAM"), Some("Specs &lt;draft&gt;"), None),
        (Some("NAM"), None, Some("R&amp;D")),
    ] {
        let listing = m.list_resources(Parameters(ListResourcesInput {
            resource_type: "page".into(),
            project: project.map(Into::into),
            folder: folder.map(Into::into),
            label: label.map(Into::into),
            ..Default::default()
        }));
        assert!(listing.contains("Escaped page"), "got: {listing}");
    }
}

fn project_id(m: &LificMcp) -> i64 {
    m.read(|conn| queries::resolve_project_identifier(conn, "NAM"))
        .unwrap()
}

#[test]
fn manage_resource_updates_resolve_escaped_current_names() {
    let (m, _guard) = mcp();
    seed_escapable_names(&m);
    for (resource_type, current, renamed) in [
        ("module", "Infra &amp; Ops", "Platform"),
        ("label", "R&amp;D", "Research"),
        ("folder", "Specs &lt;draft&gt;", "Specs"),
    ] {
        let updated = m.manage_resource(Parameters(ManageResourceInput {
            resource_type: resource_type.into(),
            action: "update".into(),
            project: Some("NAM".into()),
            current_name: Some(current.into()),
            name: Some(renamed.into()),
            ..Default::default()
        }));
        assert!(updated.starts_with("Updated"), "{resource_type}: {updated}");
        assert!(updated.contains(renamed), "{resource_type}: {updated}");
    }
}

#[test]
fn delete_resolves_escaped_names() {
    let (m, _guard) = mcp();
    seed_escapable_names(&m);
    for (resource_type, name) in [
        ("module", "Infra &amp; Ops"),
        ("label", "R&amp;D"),
        ("folder", "Specs &lt;draft&gt;"),
    ] {
        let deleted = m.delete(Parameters(DeleteInput {
            resource_type: resource_type.into(),
            identifier: name.into(),
            project: Some("NAM".into()),
        }));
        assert!(deleted.starts_with("Deleted"), "{resource_type}: {deleted}");
    }
    let pid = project_id(&m);
    m.read(|conn| {
        assert!(queries::resolve_module_name(conn, pid, "Infra & Ops").is_err());
        assert!(queries::resolve_label_name(conn, pid, "R&D").is_err());
        assert!(queries::resolve_folder_name(conn, pid, "Specs <draft>").is_err());
        Ok(())
    })
    .unwrap();
}

#[test]
fn a_name_that_literally_contains_an_entity_matches_itself_first() {
    let (m, _guard) = mcp();
    seed_escapable_names(&m);
    // Both "Q&A" and the literal "Q&amp;A" exist; the literal wins.
    m.create_issue(Parameters(CreateIssueInput {
        project: Some("NAM".into()),
        title: "Literal".into(),
        labels: Some(vec!["Q&amp;A".into()]),
        ..Default::default()
    }));
    assert_eq!(issue_in(&m, "NAM-1").labels, vec!["Q&amp;A".to_string()]);

    let deleted = m.delete(Parameters(DeleteInput {
        resource_type: "label".into(),
        identifier: "Q&amp;A".into(),
        project: Some("NAM".into()),
    }));
    assert!(deleted.starts_with("Deleted"), "got: {deleted}");
    let pid = project_id(&m);
    m.read(|conn| {
        assert!(queries::resolve_label_name(conn, pid, "Q&amp;A").is_err());
        assert!(queries::resolve_label_name(conn, pid, "Q&A").is_ok());
        Ok(())
    })
    .unwrap();

    // With no literal match left, the same text now falls back to "Q&A".
    m.create_issue(Parameters(CreateIssueInput {
        project: Some("NAM".into()),
        title: "Decoded".into(),
        labels: Some(vec!["Q&A".into()]),
        ..Default::default()
    }));
    let listing = m.list_issues(Parameters(ListIssuesInput {
        project: Some("NAM".into()),
        label: Some("Q&amp;A".into()),
        ..Default::default()
    }));
    assert!(listing.contains("Decoded"), "got: {listing}");
    assert!(!listing.contains("Literal"), "got: {listing}");
}

#[test]
fn an_escaped_name_that_matches_nothing_reports_the_name_as_sent() {
    let (m, _guard) = mcp();
    seed_escapable_names(&m);
    let result = m.create_issue(Parameters(CreateIssueInput {
        project: Some("NAM".into()),
        title: "Missing".into(),
        module: Some("Nope &amp; Nada".into()),
        ..Default::default()
    }));
    assert!(
        result.contains("module 'Nope &amp; Nada' not found"),
        "got: {result}"
    );
}

// ── LIF-474: unknown parameters are rejected with a suggestion ──

/// A real JSON-line MCP session over an in-memory pipe, so arguments take the
/// exact path a client's do: rmcp's deserializer, then `call_tool`.
mod wire {
    use rmcp::ServiceExt;
    use rmcp::transport::async_rw::AsyncRwTransport;
    use serde_json::{Value, json};
    use std::time::Duration;
    use tokio::io::WriteHalf;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf};
    use tokio::time::timeout;

    const DEADLINE: Duration = Duration::from_secs(5);

    pub(super) struct Session {
        input: WriteHalf<DuplexStream>,
        output: Lines<BufReader<ReadHalf<DuplexStream>>>,
        next_id: i64,
    }

    impl Session {
        pub(super) async fn start(server: crate::mcp::LificMcp) -> Self {
            let (client, server_end) = tokio::io::duplex(1 << 16);
            let (reader, writer) = tokio::io::split(server_end);
            tokio::spawn(async move {
                if let Ok(running) = server
                    .serve(AsyncRwTransport::new_server(reader, writer))
                    .await
                {
                    let _ = running.waiting().await;
                }
            });
            let (reader, input) = tokio::io::split(client);
            let mut session = Self {
                input,
                output: BufReader::new(reader).lines(),
                next_id: 0,
            };
            let init = session
                .request(
                    "initialize",
                    json!({
                        "protocolVersion": "2025-03-26",
                        "capabilities": {},
                        "clientInfo": {"name": "input-hardening", "version": "1.0"}
                    }),
                )
                .await;
            assert_eq!(init["result"]["serverInfo"]["name"], "lific");
            session
                .send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
                .await;
            session
        }

        async fn send(&mut self, message: Value) {
            let line = format!("{message}\n");
            timeout(DEADLINE, self.input.write_all(line.as_bytes()))
                .await
                .expect("write completes")
                .expect("write succeeds");
        }

        async fn request(&mut self, method: &str, params: Value) -> Value {
            self.next_id += 1;
            let id = self.next_id;
            self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
                .await;
            let line = timeout(DEADLINE, self.output.next_line())
                .await
                .expect("server responds")
                .expect("read succeeds")
                .expect("server keeps stdout open");
            let response: Value = serde_json::from_str(&line).expect("JSON-RPC response");
            assert_eq!(response["id"], id);
            response
        }

        /// `Ok(text)` for a tool result, `Err(message)` for a JSON-RPC error.
        pub(super) async fn call(
            &mut self,
            tool: &str,
            arguments: Value,
        ) -> Result<String, String> {
            let response = self
                .request("tools/call", json!({"name": tool, "arguments": arguments}))
                .await;
            match response.get("error") {
                Some(error) => Err(error["message"].as_str().unwrap_or_default().to_owned()),
                None => Ok(response["result"]["content"][0]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()),
            }
        }
    }
}

#[tokio::test]
async fn a_misspelled_optional_parameter_names_the_valid_one() {
    let (m, _guard) = mcp();
    seed_project(&m, "Wire", "WIR");
    seed_issue(&m, "WIR", "Spelled");
    let mut session = wire::Session::start(m.clone()).await;

    let error = session
        .call(
            "get_issue",
            json!({"identifier": "WIR-1", "comments": "all"}),
        )
        .await
        .expect_err("an unknown parameter must be refused");
    assert!(error.contains("`comments`"), "got: {error}");
    assert!(
        error.contains("Did you mean `include_comments`?"),
        "got: {error}"
    );

    let read = session
        .call(
            "get_issue",
            json!({"identifier": "WIR-1", "include_comments": "all"}),
        )
        .await
        .expect("the valid spelling still works");
    assert!(read.contains("Spelled"), "got: {read}");
}

#[tokio::test]
async fn unknown_fields_in_nested_plan_steps_are_rejected() {
    let (m, _guard) = mcp();
    seed_project(&m, "Wire", "WIR");
    let mut session = wire::Session::start(m.clone()).await;

    let error = session
        .call(
            "create_plan",
            json!({"project": "WIR", "title": "Plan", "steps": [{"name": "Step"}]}),
        )
        .await
        .expect_err("a step with an unknown field must be refused");
    assert!(
        error.contains("Unknown field `name` in a nested create_plan object"),
        "got: {error}"
    );
    let plans = m.list_resources(Parameters(ListResourcesInput {
        resource_type: "plan".into(),
        project: Some("WIR".into()),
        ..Default::default()
    }));
    assert_eq!(plans, "No plans found.");
}

#[tokio::test]
async fn camel_case_edit_keys_survive_unknown_field_rejection() {
    let (m, _guard) = mcp();
    seed_project(&m, "Wire", "WIR");
    m.create_issue(Parameters(CreateIssueInput {
        project: Some("WIR".into()),
        title: "Aliased".into(),
        description: Some("before".into()),
        ..Default::default()
    }));
    let mut session = wire::Session::start(m.clone()).await;

    let edited = session
        .call(
            "edit_issue",
            json!({"identifier": "WIR-1", "oldString": "before", "newString": "after", "replaceAll": false}),
        )
        .await
        .expect("aliases are known fields");
    assert!(!edited.starts_with("Error"), "got: {edited}");
    let issue = m
        .read(|conn| queries::get_issue(conn, queries::resolve_identifier(conn, "WIR-1")?))
        .unwrap();
    assert_eq!(issue.description, "after");
}

/// The remote stdio proxy fills an omitted `project` from the repository
/// binding before the call leaves the machine. That injected key must be one
/// the receiving tool declares, or every bound call would now be refused.
#[test]
fn the_bound_project_proxy_only_injects_a_declared_parameter() {
    let db = crate::db::open_memory().expect("test db");
    let schemas = LificMcp::new(db).list_tool_schemas();
    let mut checked = 0;
    for (tool, schema) in &schemas {
        for resource_type in [None, Some("issue"), Some("plan"), Some("module")] {
            if !project_fallback_applies(tool, resource_type) {
                continue;
            }
            checked += 1;
            assert!(
                schema["properties"].get("project").is_some(),
                "{tool} receives an injected `project` it does not declare"
            );
        }
    }
    assert!(checked >= 5, "only {checked} injectable calls inspected");
}

#[test]
fn every_tool_input_rejects_unknown_fields() {
    let db = crate::db::open_memory().expect("test db");
    for (tool, schema) in LificMcp::new(db).list_tool_schemas() {
        assert_eq!(
            schema["additionalProperties"],
            json!(false),
            "{tool} would silently ignore a misspelled parameter"
        );
    }
}

// ── LIF-475: project export returns content, paged ───────────

#[tokio::test]
async fn project_export_pages_through_markdown_without_paths() {
    let (m, _guard) = mcp();
    seed_project(&m, "Export", "EXQ");
    for (title, body) in [
        ("First", "first body"),
        ("Second", "second body"),
        ("Third", "third body"),
    ] {
        m.create_issue(Parameters(CreateIssueInput {
            project: Some("EXQ".into()),
            title: title.into(),
            description: Some(body.into()),
            ..Default::default()
        }));
    }
    m.create_page(Parameters(CreatePageInput {
        project: Some("EXQ".into()),
        title: "Handbook".into(),
        content: Some("page body".into()),
        ..Default::default()
    }));

    let first = m
        .export(Parameters(ExportInput {
            identifier: "EXQ".into(),
            limit: Some(2),
            ..Default::default()
        }))
        .await;
    assert!(
        first.starts_with(
            "Project EXQ export: 3 issue(s) and 1 page(s), 4 document(s). Documents 1-2 follow."
        ),
        "got: {first}"
    );
    assert!(first.contains("identifier: EXQ-1"), "got: {first}");
    assert!(first.contains("first body") && first.contains("second body"));
    assert!(!first.contains("third body"), "got: {first}");
    assert!(
        first.ends_with("... 2 more document(s); call export with identifier=\"EXQ\" and offset=2"),
        "got: {first}"
    );

    let second = m
        .export(Parameters(ExportInput {
            identifier: "EXQ".into(),
            offset: Some(2),
            limit: Some(2),
        }))
        .await;
    assert!(second.contains("Documents 3-4 follow."), "got: {second}");
    assert!(second.contains("third body") && second.contains("page body"));
    assert!(!second.contains("more document(s)"), "got: {second}");

    for output in [&first, &second] {
        assert!(!output.contains(".md"), "a file path leaked: {output}");
        assert!(!output.contains("EXQ/"), "a file path leaked: {output}");
    }
}
