//! LIF-474: say which parameter an agent meant when it sends one that does
//! not exist.
//!
//! Every MCP input struct denies unknown fields, so a misspelled optional
//! parameter (`comments` for `include_comments`) fails instead of being
//! dropped while the tool answers a different question. Serde's own message
//! lists every valid field but buries the likely one; this rewrites it to
//! lead with the closest match. rmcp reports the failure as a tool result with
//! `isError: true`, so the agent reads it as text, and the list of valid names
//! comes from serde, so it can never disagree with what the deserializer
//! accepts.

use rmcp::model::{CallToolResult, ContentBlock};

/// The prefix rmcp puts on every argument deserialization failure.
const DESERIALIZE_PREFIX: &str = "failed to deserialize parameters:";

/// Remove one matching pair of quote characters around an MCP string value.
/// Some clients include the source-language string delimiters in the value
/// itself instead of sending only the string contents.
pub(super) fn unquote(value: String) -> String {
    match unquote_if_wrapped(&value) {
        Some(unquoted) => unquoted.to_owned(),
        None => value,
    }
}

/// Return the value inside one matching pair of outer quote characters.
pub(super) fn unquote_if_wrapped(value: &str) -> Option<&str> {
    let quote = match value.chars().next()? {
        quote @ ('\'' | '"') => quote,
        _ => return None,
    };
    value.strip_prefix(quote)?.strip_suffix(quote)
}

/// The published top-level parameter names of one tool, taken from its
/// `input_schema.properties`. This is the contract the error rewrite relies
/// on, made a type instead of a comment: aliases (LIF-472's `oldString`)
/// never appear here because the schema advertises snake_case only, and
/// [`ToolParameters::from_schema`] is the one producer, so the list cannot
/// drift from what the deserializer accepts.
pub(crate) struct ToolParameters(Vec<String>);

impl ToolParameters {
    pub(crate) fn from_schema(input_schema: &serde_json::Map<String, serde_json::Value>) -> Self {
        let names = input_schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .map(|properties| properties.keys().cloned().collect())
            .unwrap_or_default();
        Self(names)
    }

    fn names(&self) -> &[String] {
        &self.0
    }
}

/// Rewrite rmcp's failed tool result for a parameter-deserialization error
/// about an unknown or missing field. Every other result passes through
/// unchanged. `provided` is the top-level argument names the call sent.
pub(crate) fn explain_unknown_parameter(
    tool: &str,
    params: Option<ToolParameters>,
    provided: &[String],
    result: CallToolResult,
) -> CallToolResult {
    if result.is_error != Some(true) || result.content.len() != 1 {
        return result;
    }
    let rewritten = result.content[0]
        .as_text()
        .filter(|text| text.text.starts_with(DESERIALIZE_PREFIX))
        .and_then(|text| describe(tool, params.as_ref(), provided, &text.text));
    match rewritten {
        Some(message) => CallToolResult::error(vec![ContentBlock::text(message)]),
        None => result,
    }
}

fn describe(
    tool: &str,
    params: Option<&ToolParameters>,
    provided: &[String],
    message: &str,
) -> Option<String> {
    const UNKNOWN: &str = "unknown field `";
    const MISSING: &str = "missing field `";
    // Serde names one field per message: either an unknown one (with the
    // full expected list attached) or the first required one the payload
    // lacks. The missing-field shape is how a required parameter arrives
    // when a client dropped or renamed it, so it gets the same treatment:
    // name the field, list the real ones.
    let Some(at) = message.find(UNKNOWN) else {
        let at = message.find(MISSING)?;
        let name = message[at + MISSING.len()..].strip_suffix('`')?;
        return Some(describe_missing(tool, params?.names(), provided, name));
    };
    let rest = &message[at + UNKNOWN.len()..];
    let (unknown, expected) = match rest.rfind("`, expected ") {
        Some(at) => (&rest[..at], &rest[at + "`, expected ".len()..]),
        None => (rest.strip_suffix("`, there are no fields")?, ""),
    };
    // Serde lists aliases too (LIF-472's `oldString`); advertise only the
    // published snake_case names.
    let valid: Vec<&str> = expected
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|name| !name.chars().any(|c| c.is_ascii_uppercase()))
        .collect();

    // A nested object (a plan step) has its own fields; say so rather than
    // claim they are the tool's parameters.
    let nested = params.is_some_and(|top| {
        let mut top: Vec<&str> = top.names().iter().map(String::as_str).collect();
        let mut listed = valid.clone();
        top.sort_unstable();
        listed.sort_unstable();
        top != listed
    });
    let (noun, place) = if nested {
        ("field", format!("a nested {tool} object"))
    } else {
        ("parameter", tool.to_owned())
    };

    let mut out = format!("Unknown {noun} `{unknown}` in {place}.");
    if let Some(suggestion) = closest(unknown, &valid) {
        out.push_str(&format!(" Did you mean `{suggestion}`?"));
    }
    if valid.is_empty() {
        out.push_str(" It takes no parameters.");
    } else {
        let listed: Vec<String> = valid.iter().map(|name| format!("`{name}`")).collect();
        out.push_str(&format!(" Valid {noun}s: {}.", listed.join(", ")));
    }
    Some(out)
}

/// Serde gives no list and no location for a missing field. A name the tool
/// does not take, or one the call already sent, was missing from a nested
/// object (a plan step, a batch issue), so listing the tool's parameters
/// would point the agent at the wrong place. A missing field is always a
/// real, required name, so there is nothing to suggest.
fn describe_missing(tool: &str, top_level: &[String], provided: &[String], name: &str) -> String {
    let is_parameter = top_level.iter().any(|parameter| parameter == name)
        && !provided.iter().any(|sent| sent == name);
    if !is_parameter {
        return format!("Missing field `{name}` in a nested {tool} object.");
    }
    let listed: Vec<String> = top_level.iter().map(|name| format!("`{name}`")).collect();
    format!(
        "Missing parameter `{name}` in {tool}. Valid parameters: {}.",
        listed.join(", ")
    )
}

/// Lowercase with separators removed, so `includeComments`,
/// `include-comments` and `include_comments` compare equal.
fn normalize(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '_' && *c != '-')
        .flat_map(char::to_lowercase)
        .collect()
}

/// The valid name the agent most plausibly meant: a spelling variant, then a
/// name that contains the key or is contained by it (`comments` in
/// `include_comments`), then a small edit distance. `None` when nothing is
/// close enough to be worth suggesting.
fn closest<'a>(unknown: &str, valid: &[&'a str]) -> Option<&'a str> {
    let key = normalize(unknown);
    if key.is_empty() {
        return None;
    }
    let normalized: Vec<(String, &str)> =
        valid.iter().map(|name| (normalize(name), *name)).collect();

    if let Some((_, name)) = normalized.iter().find(|(candidate, _)| *candidate == key) {
        return Some(name);
    }
    if key.chars().count() >= 3
        && let Some((_, name)) = normalized
            .iter()
            .filter(|(candidate, _)| candidate.contains(&key) || key.contains(candidate.as_str()))
            .min_by_key(|(candidate, _)| candidate.len().abs_diff(key.len()))
    {
        return Some(name);
    }
    let budget = (key.chars().count() / 3).max(2);
    normalized
        .iter()
        .map(|(candidate, name)| (levenshtein(&key, candidate), *name))
        .filter(|(distance, _)| *distance <= budget)
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, name)| name)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut current = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != *cb);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        previous = current;
    }
    previous[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn valid() -> Vec<&'static str> {
        vec!["identifier", "include_comments"]
    }

    #[test]
    fn suggests_the_parameter_a_shortened_name_belongs_to() {
        assert_eq!(closest("comments", &valid()), Some("include_comments"));
    }

    #[test]
    fn suggests_across_spelling_conventions() {
        assert_eq!(
            closest("includeComments", &valid()),
            Some("include_comments")
        );
        assert_eq!(closest("identifer", &valid()), Some("identifier"));
    }

    #[test]
    fn suggests_nothing_for_an_unrelated_name() {
        assert_eq!(closest("zzz", &valid()), None);
        assert_eq!(closest("", &valid()), None);
    }

    #[test]
    fn tool_parameters_come_from_schema_properties() {
        let schema = serde_json::json!({
            "properties": { "identifier": {}, "old_string": {} }
        });
        let params = ToolParameters::from_schema(schema.as_object().unwrap());
        assert_eq!(params.names(), ["identifier", "old_string"]);
        assert!(
            ToolParameters::from_schema(&serde_json::Map::new())
                .names()
                .is_empty()
        );
    }

    #[test]
    fn rewrites_unknown_and_missing_field_errors() {
        let top = vec!["identifier".to_string(), "include_comments".to_string()];
        let rewritten = describe(
            "get_issue",
            Some(&ToolParameters(top)),
            &[],
            "failed to deserialize parameters: unknown field `comments`, expected `identifier` or `include_comments`",
        )
        .unwrap();
        assert_eq!(
            rewritten,
            "Unknown parameter `comments` in get_issue. Did you mean `include_comments`? \
             Valid parameters: `identifier`, `include_comments`."
        );
        // A dropped required parameter names the field but carries no list;
        // the tool's schema properties stand in.
        let missing = describe(
            "list_resources",
            Some(&ToolParameters(vec![
                "resource_type".to_string(),
                "project".to_string(),
            ])),
            &["project".to_string()],
            "failed to deserialize parameters: missing field `resource_type`",
        )
        .unwrap();
        assert_eq!(
            missing,
            "Missing parameter `resource_type` in list_resources. \
             Valid parameters: `resource_type`, `project`."
        );
    }

    #[test]
    fn a_field_missing_from_a_nested_object_is_not_called_a_parameter() {
        let top = vec![
            "project".to_string(),
            "title".to_string(),
            "steps".to_string(),
        ];
        let message = "failed to deserialize parameters: missing field `title`";
        // The call sent `title`, so the missing one is a step's.
        let sent = vec!["title".to_string(), "steps".to_string()];
        assert_eq!(
            describe(
                "create_plan",
                Some(&ToolParameters(top.clone())),
                &sent,
                message
            )
            .unwrap(),
            "Missing field `title` in a nested create_plan object."
        );
        // A name the tool does not take at all is nested too.
        assert_eq!(
            describe(
                "create_plan",
                Some(&ToolParameters(top)),
                &[],
                "failed to deserialize parameters: missing field `text`"
            )
            .unwrap(),
            "Missing field `text` in a nested create_plan object."
        );
        // Without the tool's schema there is nothing true to add.
        assert!(describe("create_plan", None, &[], message).is_none());
    }

    #[test]
    fn hides_aliases_and_names_nested_objects() {
        let rewritten = describe(
            "create_plan",
            Some(&ToolParameters(vec!["project".into(), "title".into(), "steps".into()])),
            &[],
            "unknown field `name`, expected one of `title`, `description`, `issue`, `done`, `steps`",
        )
        .unwrap();
        assert!(
            rewritten.starts_with("Unknown field `name` in a nested create_plan object."),
            "{rewritten}"
        );
        let aliased = describe(
            "edit_page",
            None,
            &[],
            "unknown field `old`, expected one of `identifier`, `old_string`, `oldString`",
        )
        .unwrap();
        assert!(!aliased.contains("oldString"), "{aliased}");
        assert!(aliased.contains("Did you mean `old_string`?"), "{aliased}");
    }

    proptest! {
        #[test]
        fn removes_exactly_one_matching_quote_pair(value in any::<String>(), quote in prop_oneof![Just('\''), Just('"')]) {
            let wrapped = format!("{quote}{value}{quote}");
            prop_assert_eq!(unquote(wrapped), value);
        }

        #[test]
        fn leaves_unquoted_and_mismatched_values_unchanged(value in any::<String>(), quotes in prop_oneof![Just(('\'', '"')), Just(('"', '\''))]) {
            let plain = format!("prefix {value} suffix");
            prop_assert_eq!(unquote(plain.clone()), plain);

            let mixed = format!("{}{value}{}", quotes.0, quotes.1);
            prop_assert_eq!(unquote(mixed.clone()), mixed);
        }
    }
}
