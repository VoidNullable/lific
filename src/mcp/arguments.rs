//! LIF-474: say which parameter an agent meant when it sends one that does
//! not exist.
//!
//! Every MCP input struct denies unknown fields, so a misspelled optional
//! parameter (`comments` for `include_comments`) fails instead of being
//! dropped while the tool answers a different question. Serde's own message
//! lists every valid field but buries the likely one; this rewrites it to
//! lead with the closest match. The error stays `-32602 Invalid params`,
//! which is exactly what it is, and the list of valid names comes from serde,
//! so it can never disagree with what the deserializer accepts.

use rmcp::ErrorData;

/// Rewrite an rmcp parameter-deserialization error that is about an unknown
/// field. Every other error passes through unchanged.
pub(crate) fn explain_unknown_parameter(
    tool: &str,
    top_level: Option<&[String]>,
    error: ErrorData,
) -> ErrorData {
    match describe(tool, top_level, &error.message) {
        Some(message) => ErrorData::invalid_params(message, error.data),
        None => error,
    }
}

fn describe(tool: &str, top_level: Option<&[String]>, message: &str) -> Option<String> {
    const PREFIX: &str = "unknown field `";
    let start = message.find(PREFIX)? + PREFIX.len();
    let rest = &message[start..];
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
    let nested = top_level.is_some_and(|top| {
        let mut top: Vec<&str> = top.iter().map(String::as_str).collect();
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
    fn rewrites_only_unknown_field_errors() {
        let top = vec!["identifier".to_string(), "include_comments".to_string()];
        let rewritten = describe(
            "get_issue",
            Some(&top),
            "failed to deserialize parameters: unknown field `comments`, expected `identifier` or `include_comments`",
        )
        .unwrap();
        assert_eq!(
            rewritten,
            "Unknown parameter `comments` in get_issue. Did you mean `include_comments`? \
             Valid parameters: `identifier`, `include_comments`."
        );
        assert!(
            describe(
                "get_issue",
                Some(&top),
                "failed to deserialize parameters: missing field `identifier`"
            )
            .is_none()
        );
    }

    #[test]
    fn hides_aliases_and_names_nested_objects() {
        let rewritten = describe(
            "create_plan",
            Some(&["project".into(), "title".into(), "steps".into()]),
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
            "unknown field `old`, expected one of `identifier`, `old_string`, `oldString`",
        )
        .unwrap();
        assert!(!aliased.contains("oldString"), "{aliased}");
        assert!(aliased.contains("Did you mean `old_string`?"), "{aliased}");
    }
}
