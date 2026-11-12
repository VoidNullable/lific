//! LIF-563: `list_issues`, `search`, `list_resources` and `get_briefing`
//! return `structuredContent` that matches the `outputSchema` they declare,
//! next to their unchanged text. The text itself is held by every existing
//! test of these tools; these check the structure and its schema.

use super::tests::{mcp, seed_issue, seed_project};
use super::*;
use rmcp::handler::server::{tool::IntoCallToolResult, wrapper::Parameters};
use serde_json::{Value, json};

const STRUCTURED_TOOLS: [&str; 4] = ["list_issues", "search", "list_resources", "get_briefing"];

/// The reply's structured content, validated against the schema the tool
/// declares on the live surface.
fn checked(m: &LificMcp, tool: &str, reply: &StructuredReply) -> Value {
    let schema = m
        .tool_output_schema(tool)
        .unwrap_or_else(|| panic!("{tool} declares no output schema"));
    let value = reply
        .structured
        .clone()
        .unwrap_or_else(|| panic!("{tool} returned no structured content: {}", reply.text));
    outputs::validate(&schema, &value, tool);
    value
}

fn linked<T>(f: impl FnOnce() -> T) -> T {
    let context = crate::links::IssueLinkContext::parse("https://tracker.example").unwrap();
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(crate::mcp::with_request_context(
            None,
            Some(context),
            || async { f() },
        ))
}

fn resources(m: &LificMcp, resource_type: &str, project: Option<&str>) -> StructuredReply {
    m.list_resources(Parameters(ListResourcesInput {
        resource_type: resource_type.into(),
        project: project.map(Into::into),
        ..Default::default()
    }))
}

#[test]
fn only_the_four_scripted_tools_declare_an_output_schema() {
    let (m, _guard) = mcp();
    for (name, _) in m.list_tool_schemas() {
        let schema = m.tool_output_schema(&name);
        if STRUCTURED_TOOLS.contains(&name.as_str()) {
            let schema = schema.unwrap_or_else(|| panic!("{name} has no output schema"));
            assert_eq!(schema["type"], "object", "{name}: {schema}");
            // Compacted like the input schemas: no draft URL or type name.
            assert!(schema.get("$schema").is_none(), "{name}: {schema}");
            assert!(schema.get("title").is_none(), "{name}: {schema}");
        } else {
            assert!(schema.is_none(), "{name} declares an output schema");
        }
    }
}

#[test]
fn a_structured_reply_keeps_its_text_as_the_only_content_block() {
    let (m, _guard) = mcp();
    seed_project(&m, "Structured", "STR");
    seed_issue(&m, "STR", "Wire shape");
    let reply = m.list_issues(Parameters(ListIssuesInput {
        project: Some("STR".into()),
        ..Default::default()
    }));
    let text = reply.text.clone();
    let structured = reply.structured.clone();
    let Ok(rmcp::model::CallToolResponse::Complete(result)) = reply.into_call_tool_result() else {
        panic!("a structured reply is a complete result");
    };
    assert_eq!(result.content.len(), 1);
    assert_eq!(result.content[0].as_text().unwrap().text, text);
    assert_eq!(result.structured_content, structured);
    assert!(structured.is_some());
}

#[test]
fn list_issues_rows_carry_identifier_title_status_priority_and_link() {
    let (m, _guard) = mcp();
    seed_project(&m, "Structured", "STR");
    for title in ["First", "Second", "Third"] {
        seed_issue(&m, "STR", title);
    }
    let reply = linked(|| {
        m.list_issues(Parameters(ListIssuesInput {
            project: Some("STR".into()),
            order_by: Some("created".into()),
            order: Some("asc".into()),
            limit: Some(2),
            ..Default::default()
        }))
    });
    let value = checked(&m, "list_issues", &reply);
    assert_eq!(
        value,
        json!({
            "issues": [
                {"identifier": "STR-1", "title": "First", "status": "backlog", "priority": "none",
                 "url": "https://tracker.example/STR/issues/STR-1"},
                {"identifier": "STR-2", "title": "Second", "status": "backlog", "priority": "none",
                 "url": "https://tracker.example/STR/issues/STR-2"},
            ],
            "next_offset": 2,
        }),
        "{}",
        reply.text
    );
    // The structure links what the text links, and pages where it pages.
    assert!(
        reply
            .text
            .contains("(https://tracker.example/STR/issues/STR-2)")
    );
    assert!(reply.text.contains("offset=2"));

    // Without a link context the text has no links, so neither do the rows.
    let plain = m.list_issues(Parameters(ListIssuesInput {
        project: Some("STR".into()),
        ..Default::default()
    }));
    let value = checked(&m, "list_issues", &plain);
    assert_eq!(value["issues"].as_array().unwrap().len(), 3);
    assert!(value["issues"][0].get("url").is_none(), "{value}");
    assert!(value.get("next_offset").is_none(), "{value}");
}

#[test]
fn list_issues_empty_replies_and_errors() {
    let (m, _guard) = mcp();
    // No projects at all: the onboarding nudge, still a valid empty list.
    let nudge = m.list_issues(Parameters(ListIssuesInput::default()));
    assert_eq!(nudge, NO_PROJECTS_NUDGE);
    assert_eq!(checked(&m, "list_issues", &nudge), json!({"issues": []}));

    seed_project(&m, "Structured", "STR");
    let empty = m.list_issues(Parameters(ListIssuesInput {
        project: Some("STR".into()),
        ..Default::default()
    }));
    assert_eq!(empty, "No issues found.");
    assert_eq!(checked(&m, "list_issues", &empty), json!({"issues": []}));

    // A failure is error text alone, with nothing to validate.
    let failed = m.list_issues(Parameters(ListIssuesInput {
        project: Some("NOPE".into()),
        ..Default::default()
    }));
    assert!(failed.starts_with("Error: "), "{failed}");
    assert!(failed.structured.is_none());
}

#[test]
fn search_hits_carry_type_id_identifier_title_and_link() {
    let (m, _guard) = mcp();
    seed_project(&m, "Structured", "STR");
    seed_issue(&m, "STR", "Searchable issue");
    let page = m.create_page(Parameters(CreatePageInput {
        project: Some("STR".into()),
        title: "Searchable page".into(),
        content: Some("Body".into()),
        ..Default::default()
    }));
    assert!(page.starts_with("Created"), "{page}");
    let comment = m.add_comment(Parameters(AddCommentInput {
        identifier: "STR-1".into(),
        content: "A searchable remark".into(),
    }));
    assert!(!comment.starts_with("Error"), "{comment}");

    let reply = linked(|| {
        m.search(Parameters(SearchInput {
            query: "searchable".into(),
            ..Default::default()
        }))
    });
    let value = checked(&m, "search", &reply);
    let results = value["results"].as_array().unwrap();
    assert_eq!(results.len(), 3, "{}", reply.text);
    let of = |kind: &str| {
        results
            .iter()
            .find(|hit| hit["type"] == kind)
            .unwrap_or_else(|| panic!("no {kind} hit: {value}"))
    };
    let issue = of("issue");
    assert_eq!(issue["identifier"], "STR-1");
    assert_eq!(issue["title"], "Searchable issue");
    assert_eq!(issue["url"], "https://tracker.example/STR/issues/STR-1");
    let page = of("page");
    assert_eq!(page["title"], "Searchable page");
    assert!(
        page["url"]
            .as_str()
            .unwrap()
            .starts_with("https://tracker.example/STR/pages/"),
        "{page}"
    );
    // A comment names the issue it is on, and links to itself there.
    let comment = of("comment");
    assert_eq!(comment["identifier"], "STR-1");
    assert_eq!(
        comment["url"],
        format!(
            "https://tracker.example/STR/issues/STR-1#comment-{}",
            comment["id"]
        )
    );
    for hit in results {
        let url = hit["url"].as_str().unwrap();
        assert!(
            reply.text.contains(&format!("({url})")),
            "{url} not in text"
        );
    }
}

#[test]
fn search_empty_replies_and_paging() {
    let (m, _guard) = mcp();
    let nudge = m.search(Parameters(SearchInput {
        query: "anything".into(),
        ..Default::default()
    }));
    assert_eq!(nudge, NO_PROJECTS_NUDGE);
    assert_eq!(checked(&m, "search", &nudge), json!({"results": []}));

    seed_project(&m, "Structured", "STR");
    let none = m.search(Parameters(SearchInput {
        query: "absent".into(),
        ..Default::default()
    }));
    assert_eq!(none, "No results found.");
    assert_eq!(checked(&m, "search", &none), json!({"results": []}));

    for title in ["Paged one", "Paged two", "Paged three"] {
        seed_issue(&m, "STR", title);
    }
    let page = m.search(Parameters(SearchInput {
        query: "paged".into(),
        limit: Some(2),
        ..Default::default()
    }));
    let value = checked(&m, "search", &page);
    assert_eq!(value["results"].as_array().unwrap().len(), 2);
    assert_eq!(value["next_offset"], 2, "{}", page.text);
    assert!(page.text.contains("offset=2"));
}

#[test]
fn list_resources_rows_per_resource_type() {
    let (m, _guard) = mcp();
    seed_project(&m, "Structured", "STR");
    seed_issue(&m, "STR", "Listed issue");
    for (resource_type, name) in [("module", "Core"), ("label", "bug"), ("folder", "Notes")] {
        let created = m.manage_resource(Parameters(ManageResourceInput {
            resource_type: resource_type.into(),
            action: "create".into(),
            project: Some("STR".into()),
            name: Some(name.into()),
            ..Default::default()
        }));
        assert!(created.starts_with("Created"), "{created}");
    }
    let page = m.create_page(Parameters(CreatePageInput {
        project: Some("STR".into()),
        title: "Listed page".into(),
        ..Default::default()
    }));
    assert!(page.starts_with("Created"), "{page}");
    let plan = m.create_plan(Parameters(CreatePlanInput {
        project: Some("STR".into()),
        title: "Listed plan".into(),
        ..Default::default()
    }));
    assert!(plan.starts_with("Created"), "{plan}");

    let listed = |resource_type: &str| {
        let reply = linked(|| resources(&m, resource_type, Some("STR")));
        let value = checked(&m, "list_resources", &reply);
        assert_eq!(value["resource_type"], resource_type);
        let items = value["items"].as_array().unwrap().clone();
        assert_eq!(items.len(), 1, "{resource_type}: {value}");
        // Every link in the structure is one the text shows.
        if let Some(url) = items[0]["url"].as_str() {
            assert!(reply.text.contains(&format!("({url})")), "{url}");
        }
        items[0].clone()
    };

    let project = listed("project");
    assert_eq!(project["identifier"], "STR");
    assert_eq!(project["name"], "Structured");
    assert_eq!(project["url"], "https://tracker.example/STR/overview");
    let issue = listed("issue");
    assert_eq!(issue["identifier"], "STR-1");
    assert_eq!(issue["title"], "Listed issue");
    assert_eq!(issue["status"], "backlog");
    assert_eq!(issue["url"], "https://tracker.example/STR/issues/STR-1");
    let page = listed("page");
    assert_eq!(page["title"], "Listed page");
    assert!(page["identifier"].as_str().unwrap().starts_with("STR-DOC-"));
    let plan = listed("plan");
    assert_eq!(plan["title"], "Listed plan");
    assert_eq!(plan["identifier"], "STR-PLAN-1");
    assert!(plan["url"].is_string(), "{plan}");
    let module = listed("module");
    assert_eq!(module["name"], "Core");
    assert!(
        module["id"].is_i64() && module["url"].is_string(),
        "{module}"
    );
    let label = listed("label");
    assert_eq!(label["name"], "bug");
    assert!(label.get("url").is_none(), "{label}");
    let folder = listed("folder");
    assert_eq!(folder["name"], "Notes");
    assert!(folder["id"].is_i64(), "{folder}");
}

#[test]
fn list_resources_empty_replies_and_errors() {
    let (m, _guard) = mcp();
    let nudge = resources(&m, "project", None);
    assert_eq!(nudge, NO_PROJECTS_NUDGE);
    assert_eq!(
        checked(&m, "list_resources", &nudge),
        json!({"resource_type": "project", "items": []})
    );

    seed_project(&m, "Structured", "STR");
    for (resource_type, text) in [
        ("plan", "No plans found."),
        ("page", "No pages found."),
        ("issue", "0 issues (use list_issues for filtering):\n"),
        ("module", "0 modules:\n"),
        ("label", "0 labels:\n"),
        ("folder", "0 folders:\n"),
    ] {
        let reply = resources(&m, resource_type, Some("STR"));
        assert_eq!(reply, text);
        assert_eq!(
            checked(&m, "list_resources", &reply),
            json!({"resource_type": resource_type, "items": []})
        );
    }
    // An unknown type answers with guidance, not an error, so it carries an
    // empty list too.
    let unknown = resources(&m, "widget", Some("STR"));
    assert!(unknown.starts_with("Unknown type 'widget'"), "{unknown}");
    assert_eq!(
        checked(&m, "list_resources", &unknown),
        json!({"resource_type": "widget", "items": []})
    );

    let failed = resources(&m, "module", Some("NOPE"));
    assert!(failed.starts_with("Error: "), "{failed}");
    assert!(failed.structured.is_none());
}

#[test]
fn get_briefing_sections_list_the_items_their_lines_show() {
    let (m, _guard) = mcp();
    seed_project(&m, "Structured", "STR");
    for title in ["Pick me", "Or me"] {
        seed_issue(&m, "STR", title);
    }
    let page = m.create_page(Parameters(CreatePageInput {
        project: Some("STR".into()),
        title: "Canon".into(),
        ..Default::default()
    }));
    assert!(page.starts_with("Created"), "{page}");

    let reply = linked(|| {
        m.get_briefing(Parameters(GetBriefingInput {
            project: Some("STR".into()),
            ..Default::default()
        }))
    });
    let value = checked(&m, "get_briefing", &reply);
    assert_eq!(value["project"], "STR");
    let section = |key: &str| {
        value["sections"]
            .as_array()
            .unwrap()
            .iter()
            .find(|section| section["section"] == key)
            .unwrap_or_else(|| panic!("no {key} section: {value}"))
            .clone()
    };
    let workable = section("workable");
    assert_eq!(workable["total"], 2);
    let titles: Vec<&str> = workable["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            assert_eq!(item["kind"], "issue");
            assert_eq!(item["status"], "backlog");
            item["title"].as_str().unwrap()
        })
        .collect();
    assert_eq!(titles.len(), 2);
    assert!(titles.contains(&"Pick me") && titles.contains(&"Or me"));
    let pages = section("pages");
    assert_eq!(pages["items"][0]["kind"], "page");
    assert_eq!(pages["items"][0]["title"], "Canon");
    for section in value["sections"].as_array().unwrap() {
        for item in section["items"].as_array().unwrap() {
            if let Some(url) = item["url"].as_str() {
                assert!(reply.text.contains(&format!("({url})")), "{url}");
            }
        }
    }

    // A named page that does not resolve is still an item, by identifier.
    let missing = m.get_briefing(Parameters(GetBriefingInput {
        project: Some("STR".into()),
        pages: Some(vec!["STR-DOC-404".into()]),
        ..Default::default()
    }));
    let value = checked(&m, "get_briefing", &missing);
    let pages = value["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|section| section["section"] == "pages")
        .unwrap();
    assert_eq!(
        pages["items"],
        json!([{"kind": "page", "identifier": "STR-DOC-404"}])
    );
}

#[test]
fn get_briefing_empty_replies_and_errors() {
    let (m, _guard) = mcp();
    let nudge = m.get_briefing(Parameters(GetBriefingInput::default()));
    assert_eq!(nudge, NO_PROJECTS_NUDGE);
    assert_eq!(checked(&m, "get_briefing", &nudge), json!({"sections": []}));

    seed_project(&m, "Structured", "STR");
    let quiet = m.get_briefing(Parameters(GetBriefingInput {
        project: Some("STR".into()),
        ..Default::default()
    }));
    assert!(
        quiet.contains("Nothing is planned, active, blocked or workable."),
        "{quiet}"
    );
    assert_eq!(
        checked(&m, "get_briefing", &quiet),
        json!({"project": "STR", "sections": []})
    );

    let failed = m.get_briefing(Parameters(GetBriefingInput {
        project: Some("NOPE".into()),
        ..Default::default()
    }));
    assert!(failed.starts_with("Error: "), "{failed}");
    assert!(failed.structured.is_none());
}

#[test]
fn the_test_validator_rejects_what_a_schema_does_not_allow() {
    let schema = json!({
        "type": "object",
        "properties": {
            "items": {"type": "array", "items": {
                "type": "object",
                "properties": {"id": {"type": "integer"}},
                "required": ["id"],
            }},
        },
        "required": ["items"],
    });
    let rejects = |schema: &Value, value: Value| {
        std::panic::catch_unwind(|| outputs::validate(schema, &value, "t")).is_err()
    };
    assert!(!rejects(&schema, json!({"items": [{"id": 1}]})));
    assert!(rejects(&schema, json!({})), "missing required field");
    assert!(
        rejects(&schema, json!({"items": [{"id": "1"}]})),
        "wrong type"
    );
    assert!(
        rejects(&schema, json!({"items": [{}]})),
        "missing nested field"
    );
    assert!(
        rejects(&schema, json!({"items": [], "extra": true})),
        "undeclared field"
    );
    assert!(
        rejects(&json!({"$ref": "#/$defs/X"}), json!({})),
        "unknown keyword"
    );
}
