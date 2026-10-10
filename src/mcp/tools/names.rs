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

/// Entity suffixes following `&` that an HTML-escaping client produces for text.
const ENTITIES: [(&str, char); 6] = [
    ("amp;", '&'),
    ("lt;", '<'),
    ("gt;", '>'),
    ("quot;", '"'),
    ("#39;", '\''),
    ("#x27;", '\''),
];

/// Decode [`ENTITIES`] in one pass (so `&amp;lt;` becomes `&lt;`, not `<`).
/// `None` when the name contains none of them.
pub(super) fn decode_html_entities(name: &str) -> Option<String> {
    let mut replacements = name.match_indices('&').filter_map(|(start, _)| {
        let tail = &name[start + 1..];
        ENTITIES.iter().find_map(|(entity, ch)| {
            tail.starts_with(entity)
                .then_some((start, entity.len() + 1, *ch))
        })
    });
    let first = replacements.next()?;
    // Allocate only when an entity matches. Decoding cannot expand the input.
    let (mut decoded, end) = std::iter::once(first).chain(replacements).fold(
        (String::with_capacity(name.len()), 0),
        |(mut decoded, end), (start, len, ch)| {
            decoded.push_str(&name[end..start]);
            decoded.push(ch);
            (decoded, start + len)
        },
    );
    decoded.push_str(&name[end..]);
    Some(decoded)
}

/// Try the original name, then the name without one outer quote pair, then
/// their HTML-decoded forms in that order. Only a NotFound triggers a retry;
/// a final miss reports the original name.
fn with_decoded_retry<T>(
    name: &str,
    lookup: impl Fn(&str) -> Result<T, LificError>,
) -> Result<T, LificError> {
    let original_message = match lookup(name) {
        Err(LificError::NotFound(message)) => message,
        other => return other,
    };
    let unquoted = crate::mcp::arguments::unquote_if_wrapped(name);
    let decoded = std::iter::once(name)
        .chain(unquoted)
        .filter_map(decode_html_entities)
        .map(|candidate| lookup(&candidate));
    unquoted
        .into_iter()
        .map(&lookup)
        .chain(decoded)
        .find(|result| !matches!(result, Err(LificError::NotFound(_))))
        .unwrap_or_else(|| Err(LificError::NotFound(original_message)))
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
    use super::{decode_html_entities, with_decoded_retry};
    use crate::error::LificError;
    use proptest::prelude::*;
    use std::cell::RefCell;

    proptest! {
        #[test]
        fn retry_stops_at_first_success_or_other_error(stem in "[a-z雪🙂]{0,12}") {
            ['\'', '"'].into_iter()
                .flat_map(|quote| (0..4).map(move |stop| (quote, stop)))
                .flat_map(|(quote, stop)| [false, true].map(|fail| (quote, stop, fail)))
                .try_for_each(|(quote, stop, fail)| {
                    let candidates = [
                        format!("{quote}{stem}&amp;y{quote}"),
                        format!("{stem}&amp;y"),
                        format!("{quote}{stem}&y{quote}"),
                        format!("{stem}&y"),
                    ];
                    let attempts = RefCell::new(Vec::new());
                    let result = with_decoded_retry(&candidates[0], |name| {
                        attempts.borrow_mut().push(name.to_owned());
                        if name != candidates[stop] {
                            return Err(LificError::NotFound(name.to_owned()));
                        }
                        if fail {
                            Err(LificError::Forbidden(name.to_owned()))
                        } else {
                            Ok(name.to_owned())
                        }
                    });
                    prop_assert_eq!(attempts.into_inner(), candidates[..=stop].to_vec());
                    match result {
                        Ok(name) => {
                            prop_assert!(!fail);
                            prop_assert_eq!(&name, &candidates[stop]);
                        }
                        Err(LificError::Forbidden(name)) => {
                            prop_assert!(fail);
                            prop_assert_eq!(&name, &candidates[stop]);
                        }
                        other => prop_assert!(false, "unexpected result: {:?}", other),
                    }
                    Ok(())
                })?;
        }

        #[test]
        fn html_decoding_preserves_fragment_boundaries(
            fragments in prop::collection::vec(prop::sample::select(vec![
                ("&amp;", "&"), ("&lt;", "<"), ("&gt;", ">"),
                ("&quot;", "\""), ("&#39;", "'"), ("&#x27;", "'"),
                ("&amp;lt;", "&lt;"), ("&amp;amp;", "&amp;"),
                ("&", "&"), ("&nbsp;", "&nbsp;"), ("&AMP;", "&AMP;"),
                ("plain", "plain"), ("雪🙂", "雪🙂"), ("", ""),
            ]), 0..64)
        ) {
            let encoded: String = fragments.iter().map(|(encoded, _)| *encoded).collect();
            let expected = fragments.iter().any(|(encoded, decoded)| encoded != decoded)
                .then(|| fragments.iter().map(|(_, decoded)| *decoded).collect::<String>());
            prop_assert_eq!(decode_html_entities(&encoded), expected);
        }

        #[test]
        fn quoted_literal_names_precede_decoded_matches(stem in "[a-z]{0,12}") {
            for quote in ['\'', '"'] {
                let candidates = [
                    format!("{quote}{stem}&amp;y{quote}"),
                    format!("{stem}&amp;y"),
                    format!("{quote}{stem}&y{quote}"),
                    format!("{stem}&y"),
                ];
                // Exhaust every combination of existing names in precedence order.
                for present in 0_u8..16 {
                    let result = with_decoded_retry(&candidates[0], |name| {
                        candidates.iter().enumerate()
                            .find(|(index, candidate)| present & (1 << index) != 0 && candidate.as_str() == name)
                            .map(|(index, _)| index)
                            .ok_or_else(|| LificError::NotFound(name.into()))
                    });
                    match (0..4).find(|index| present & (1 << index) != 0) {
                        Some(expected) => prop_assert_eq!(result.unwrap(), expected, "presence mask: {}", present),
                        None => match result {
                            Err(LificError::NotFound(name)) => prop_assert_eq!(name, candidates[0].as_str()),
                            other => prop_assert!(false, "expected NotFound, got {:?}", other),
                        },
                    }
                }
            }
        }
    }

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
