//! LIF-563: structured results for the tools scripts compose calls from.
//!
//! `list_issues`, `search`, `list_resources` and `get_briefing` keep their
//! text exactly as it was and also return `structuredContent`, declared by an
//! `outputSchema`, so a host calling tools from code reads fields instead of
//! parsing lines. Each tool builds its structure from the same rows its text
//! renders, in the same function.
//!
//! Every connected agent pays for these schemas on each `tools/list`, so the
//! shapes hold only what a script needs to make the next call (identifiers,
//! ids, titles, status, links) and carry no field descriptions. The types use
//! plain comments, not doc comments: schemars would publish a doc comment as
//! a `description`.

use std::sync::Arc;

use rmcp::{
    handler::server::tool::IntoCallToolResult,
    model::{CallToolResponse, CallToolResult, ContentBlock, JsonObject},
    schemars::{self, JsonSchema},
};
use serde::Serialize;

/// The output schema a `#[tool(output_schema = ...)]` attribute publishes.
/// `LificMcp::shared_tool_router` compacts it like the input schemas.
pub(crate) fn schema<T: JsonSchema + 'static>() -> Arc<JsonObject> {
    rmcp::handler::server::common::schema_for_output::<T>()
}

/// A tool reply with text and, on success, its structured form. Failures
/// carry only the `Error: ` text, so `mark_tool_error` still flags them and
/// no result that fails the declared schema is ever sent.
#[derive(Debug)]
pub(crate) struct StructuredReply {
    pub(crate) text: String,
    pub(crate) structured: Option<serde_json::Value>,
}

impl StructuredReply {
    pub(crate) fn new<T: Serialize>(result: Result<(String, T), String>) -> Self {
        match result {
            // A render failure is already error text; it gets no structure.
            Ok((text, _)) if text.starts_with(super::TOOL_ERROR_PREFIX) => Self {
                text,
                structured: None,
            },
            Ok((text, value)) => match serde_json::to_value(value) {
                Ok(value) => Self {
                    text,
                    structured: Some(value),
                },
                Err(error) => Self::error(format!("failed to encode structured result: {error}")),
            },
            Err(error) => Self::error(error),
        }
    }

    fn error(error: String) -> Self {
        Self {
            text: format!("{}{error}", super::TOOL_ERROR_PREFIX),
            structured: None,
        }
    }
}

impl IntoCallToolResult for StructuredReply {
    fn into_call_tool_result(self) -> Result<CallToolResponse, rmcp::ErrorData> {
        let mut result = CallToolResult::success(vec![ContentBlock::text(self.text)]);
        result.structured_content = self.structured;
        Ok(result.into())
    }
}

// Tests read a reply as its text, exactly as they read the `String` these
// tools returned before.
#[cfg(test)]
impl std::ops::Deref for StructuredReply {
    type Target = str;

    fn deref(&self) -> &str {
        &self.text
    }
}

#[cfg(test)]
impl std::fmt::Display for StructuredReply {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.text)
    }
}

#[cfg(test)]
impl PartialEq<&str> for StructuredReply {
    fn eq(&self, other: &&str) -> bool {
        self.text == *other
    }
}

#[cfg(test)]
impl PartialEq<str> for StructuredReply {
    fn eq(&self, other: &str) -> bool {
        self.text == other
    }
}

#[cfg(test)]
impl PartialEq<String> for StructuredReply {
    fn eq(&self, other: &String) -> bool {
        &self.text == other
    }
}

// `list_issues`: one row per issue line, and the offset of the next page when
// the text ends with a paging hint.
#[derive(Debug, Default, Serialize, JsonSchema)]
pub(crate) struct IssueList {
    pub(crate) issues: Vec<IssueRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) next_offset: Option<i64>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(inline)]
pub(crate) struct IssueRow {
    pub(crate) identifier: String,
    pub(crate) title: String,
    pub(crate) status: String,
    pub(crate) priority: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) url: Option<String>,
}

// `search`: `type` is issue, page, plan, comment or attachment; `id` is that
// entity's own id; `identifier` is the hit's, or for a comment or attachment
// the issue or page it belongs to.
#[derive(Debug, Default, Serialize, JsonSchema)]
pub(crate) struct SearchResults {
    pub(crate) results: Vec<SearchHit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) next_offset: Option<i64>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(inline)]
pub(crate) struct SearchHit {
    #[serde(rename = "type")]
    pub(crate) kind: String,
    pub(crate) id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) identifier: Option<String>,
    pub(crate) title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) url: Option<String>,
}

// `list_resources`: projects, modules, labels and folders have a `name`;
// issues, pages and plans a `title`.
#[derive(Debug, Default, Serialize, JsonSchema)]
pub(crate) struct ResourceList {
    pub(crate) resource_type: String,
    pub(crate) items: Vec<ResourceRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) next_offset: Option<i64>,
}

#[derive(Debug, Default, Serialize, JsonSchema)]
#[schemars(inline)]
pub(crate) struct ResourceRow {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) identifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) url: Option<String>,
}

// `get_briefing`: the sections in text order, each keyed by a stable name
// (human_queue, changes, plans, due, blocked, workable, pages, active, stale,
// triage) with its full count and the items its lines show.
#[derive(Debug, Default, Serialize, JsonSchema)]
pub(crate) struct Briefing {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) project: Option<String>,
    pub(crate) sections: Vec<BriefingSection>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(inline)]
pub(crate) struct BriefingSection {
    pub(crate) section: String,
    pub(crate) total: i64,
    pub(crate) items: Vec<BriefingItem>,
}

// `kind` is issue, page, plan, module or comment. A comment names the issue
// or page it is on.
#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
#[schemars(inline)]
pub(crate) struct BriefingItem {
    pub(crate) kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) identifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) url: Option<String>,
}

/// A JSON Schema check for the keywords these output schemas use (`type`,
/// `properties`, `required`, `items`). Any other keyword fails the check
/// rather than being skipped, so a schema that grows one (a `$ref`, an
/// `anyOf`) cannot pass untested.
#[cfg(test)]
pub(crate) fn validate(schema: &serde_json::Value, value: &serde_json::Value, path: &str) {
    use serde_json::Value;
    let schema = schema.as_object().expect("a schema is an object");
    for keyword in schema.keys() {
        assert!(
            matches!(
                keyword.as_str(),
                "type" | "properties" | "required" | "items"
            ),
            "{path}: the test validator does not know schema keyword '{keyword}'"
        );
    }
    if let Some(expected) = schema.get("type").and_then(Value::as_str) {
        let matches = match expected {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "integer" => value.is_i64() || value.is_u64(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            other => panic!("{path}: unknown type '{other}'"),
        };
        assert!(matches, "{path}: expected {expected}, got {value}");
    }
    if let Some(object) = value.as_object() {
        let properties = schema.get("properties").and_then(Value::as_object);
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for name in required {
                let name = name.as_str().expect("required names are strings");
                assert!(
                    object.contains_key(name),
                    "{path}: missing required '{name}' in {value}"
                );
            }
        }
        for (name, field) in object {
            match properties.and_then(|properties| properties.get(name)) {
                Some(field_schema) => validate(field_schema, field, &format!("{path}.{name}")),
                // Stricter than JSON Schema, which allows undeclared fields:
                // a field the schema does not name means the two drifted.
                None => panic!("{path}: '{name}' is not declared in the schema"),
            }
        }
    }
    if let (Some(items), Some(array)) = (schema.get("items"), value.as_array()) {
        for (index, item) in array.iter().enumerate() {
            validate(items, item, &format!("{path}[{index}]"));
        }
    }
}
