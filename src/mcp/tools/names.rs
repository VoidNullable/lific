//! LIF-473: name lookups that tolerate a client's HTML escaping.
//!
//! Lific stores and returns names verbatim (the LIF-299 guards), but some
//! clients escape tool arguments on the way in, so `Infra & Ops` arrives as
//! `Infra &amp; Ops`. Every module, label and folder lookup an MCP tool makes
//! goes through here: the name as sent is tried first, so a name that
//! literally contains an entity still matches itself, and only a miss retries
//! with the common entities decoded.

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::queries;
use crate::error::LificError;

/// The entities an HTML-escaping client produces for text.
const ENTITIES: [(&str, char); 6] = [
    ("&amp;", '&'),
    ("&lt;", '<'),
    ("&gt;", '>'),
    ("&quot;", '"'),
    ("&#39;", '\''),
    ("&#x27;", '\''),
];

/// Decode [`ENTITIES`] in one pass (so `&amp;lt;` becomes `&lt;`, not `<`).
/// `None` when the name contains none of them.
pub(super) fn decode_html_entities(name: &str) -> Option<String> {
    let mut decoded = String::with_capacity(name.len());
    let mut rest = name;
    let mut changed = false;
    while let Some(at) = rest.find('&') {
        decoded.push_str(&rest[..at]);
        let tail = &rest[at..];
        match ENTITIES.iter().find(|(entity, _)| tail.starts_with(entity)) {
            Some((entity, ch)) => {
                decoded.push(*ch);
                rest = &tail[entity.len()..];
                changed = true;
            }
            None => {
                decoded.push('&');
                rest = &tail[1..];
            }
        }
    }
    decoded.push_str(rest);
    changed.then_some(decoded)
}

/// Run `lookup` on `name`, and on a miss once more on its decoded form. A
/// second miss reports the name as the caller sent it.
fn with_decoded_retry<T>(
    name: &str,
    lookup: impl Fn(&str) -> Result<T, LificError>,
) -> Result<T, LificError> {
    match lookup(name) {
        Err(LificError::NotFound(message)) => match decode_html_entities(name) {
            Some(decoded) => match lookup(&decoded) {
                Err(LificError::NotFound(_)) => Err(LificError::NotFound(message)),
                other => other,
            },
            None => Err(LificError::NotFound(message)),
        },
        other => other,
    }
}

pub(super) fn module_id(conn: &Connection, project_id: i64, name: &str) -> Result<i64, LificError> {
    with_decoded_retry(name, |name| {
        queries::resolve_module_name(conn, project_id, name)
    })
}

pub(super) fn folder_id(conn: &Connection, project_id: i64, name: &str) -> Result<i64, LificError> {
    with_decoded_retry(name, |name| {
        queries::resolve_folder_name(conn, project_id, name)
    })
}

pub(super) fn label_id(conn: &Connection, project_id: i64, name: &str) -> Result<i64, LificError> {
    with_decoded_retry(name, |name| {
        queries::resolve_label_name(conn, project_id, name)
    })
}

/// The label name to hand to the queries that attach or filter by name.
///
/// Those match `name = ?` exactly and treat an unknown name as no match, so
/// this keeps their comparison and only swaps in the decoded form when it
/// names a label and the name as sent does not.
pub(super) fn stored_label_name(
    conn: &Connection,
    project_id: i64,
    name: &str,
) -> Result<String, LificError> {
    let exists = |name: &str| -> Result<bool, LificError> {
        Ok(conn
            .query_row(
                "SELECT 1 FROM labels WHERE project_id = ?1 AND name = ?2",
                params![project_id, name],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    };
    if !exists(name)?
        && let Some(decoded) = decode_html_entities(name)
        && exists(&decoded)?
    {
        return Ok(decoded);
    }
    Ok(name.to_owned())
}

pub(super) fn stored_label_names(
    conn: &Connection,
    project_id: i64,
    names: &[String],
) -> Result<Vec<String>, LificError> {
    names
        .iter()
        .map(|name| stored_label_name(conn, project_id, name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::decode_html_entities;

    #[test]
    fn decodes_the_common_entities_once() {
        assert_eq!(
            decode_html_entities("Infra &amp; Ops").as_deref(),
            Some("Infra & Ops")
        );
        assert_eq!(
            decode_html_entities("&lt;b&gt; &quot;x&quot; it&#39;s it&#x27;s").as_deref(),
            Some("<b> \"x\" it's it's")
        );
        assert_eq!(decode_html_entities("&amp;lt;").as_deref(), Some("&lt;"));
    }

    #[test]
    fn leaves_names_without_entities_alone() {
        assert_eq!(decode_html_entities("R&D"), None);
        assert_eq!(decode_html_entities("A & B &nbsp;"), None);
        assert_eq!(decode_html_entities("plain"), None);
    }
}
