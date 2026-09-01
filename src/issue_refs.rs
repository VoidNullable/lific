//! LIF-5: the closing-reference grammar a commit message uses to close an
//! issue, e.g. `closes LIF-42`.
//!
//! Two consumers read this: `POST /api/git-hook` (CI posts the messages of a
//! push) and `lific git-hook` (a local hook, or `--range` over a commit
//! range). They must agree exactly on what counts as a reference, so the
//! grammar lives here, above both, rather than in either.
//!
//! The grammar, as a regex, is:
//!
//! ```text
//! (?i)\b(closes?|closed|fix(es|ed)?|resolve[sd]?)\s+([A-Za-z][A-Za-z0-9]*-\d+)\b
//! ```
//!
//! It is hand-scanned rather than compiled, because `regex` is not a
//! dependency of this crate and one pattern this small does not justify
//! becoming one. The scanner is a faithful, non-backtracking equivalent: the
//! identifier's `[A-Za-z0-9]*` run can never cross the `-` it is looking for,
//! so there is exactly one way to match at any given position and nothing to
//! backtrack over.
//!
//! Deliberately *not* matched: a bare mention. "see LIF-42" or "part of
//! LIF-42" leave the issue alone, because a commit that talks about an issue
//! is not a commit that finishes it.

/// The closing keywords, longest-first within each family so the scanner
/// finds `closed` before `close` and does not stop at the shorter prefix.
const KEYWORDS: [&str; 9] = [
    "closes", "closed", "close", "fixes", "fixed", "fix", "resolves", "resolved", "resolve",
];

/// Regex `\w`: the character class both word boundaries in the pattern are
/// defined against.
fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Whether a match may start at `index` — regex's leading `\b`.
fn at_word_boundary(bytes: &[u8], index: usize) -> bool {
    index == 0 || !is_word_byte(bytes[index - 1])
}

/// The index just past a closing keyword starting at `index`, if one is there.
fn keyword_at(bytes: &[u8], index: usize) -> Option<usize> {
    KEYWORDS.iter().find_map(|keyword| {
        let end = index + keyword.len();
        let candidate = bytes.get(index..end)?;
        candidate
            .eq_ignore_ascii_case(keyword.as_bytes())
            .then_some(end)
    })
}

/// The issue identifier starting at `index` (`[A-Za-z][A-Za-z0-9]*-\d+\b`),
/// plus the index just past it.
fn identifier_at(bytes: &[u8], index: usize) -> Option<(&str, usize)> {
    if !bytes.get(index)?.is_ascii_alphabetic() {
        return None;
    }
    let mut cursor = index + 1;
    while bytes.get(cursor).is_some_and(u8::is_ascii_alphanumeric) {
        cursor += 1;
    }
    if bytes.get(cursor) != Some(&b'-') {
        return None;
    }
    cursor += 1;
    let digits_start = cursor;
    while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
        cursor += 1;
    }
    if cursor == digits_start {
        return None;
    }
    // The trailing `\b`: `LIF-42a` is not a reference to LIF-42.
    if bytes.get(cursor).copied().is_some_and(is_word_byte) {
        return None;
    }
    // Every byte consumed above is ASCII, so this slice is on a char boundary.
    let text = std::str::from_utf8(&bytes[index..cursor]).ok()?;
    Some((text, cursor))
}

/// Every issue identifier `text` closes, uppercased, deduplicated, in the
/// order they first appear.
///
/// Identifiers are uppercased on extraction because `fixes lif-42` and
/// `Fixes LIF-42` name the same issue, and the canonical spelling of an
/// identifier is upper case everywhere else in Lific.
#[must_use]
pub fn closing_references(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut found: Vec<String> = Vec::new();
    let mut index = 0usize;

    while index < bytes.len() {
        if !at_word_boundary(bytes, index) {
            index += 1;
            continue;
        }
        let Some(after_keyword) = keyword_at(bytes, index) else {
            index += 1;
            continue;
        };

        // `\s+` — at least one space, newline or tab between the two halves.
        let mut cursor = after_keyword;
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if cursor == after_keyword {
            index += 1;
            continue;
        }

        match identifier_at(bytes, cursor) {
            Some((identifier, end)) => {
                let identifier = identifier.to_ascii_uppercase();
                if !found.contains(&identifier) {
                    found.push(identifier);
                }
                index = end;
            }
            // Resume past the keyword, never inside it: `closes closes LIF-1`
            // still finds LIF-1.
            None => index = after_keyword,
        }
    }

    found
}

/// [`closing_references`] over several messages at once, deduplicated across
/// all of them. A push that mentions the same issue in three commits closes it
/// once.
#[must_use]
pub fn closing_references_in<S: AsRef<str>>(messages: &[S]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for message in messages {
        for identifier in closing_references(message.as_ref()) {
            if !found.contains(&identifier) {
                found.push(identifier);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs(text: &str) -> Vec<String> {
        closing_references(text)
    }

    #[test]
    fn every_keyword_in_the_grammar_closes() {
        for keyword in [
            "close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves", "resolved",
        ] {
            assert_eq!(
                refs(&format!("{keyword} LIF-42")),
                vec!["LIF-42"],
                "'{keyword}' must be a closing keyword"
            );
        }
    }

    #[test]
    fn keywords_are_case_insensitive() {
        for keyword in ["CLOSES", "Closes", "cLoSeS", "FIXES", "Resolved"] {
            assert_eq!(refs(&format!("{keyword} LIF-42")), vec!["LIF-42"]);
        }
    }

    #[test]
    fn identifiers_are_uppercased_on_extraction() {
        assert_eq!(refs("Fixes lif-42"), vec!["LIF-42"]);
        assert_eq!(refs("closes Lif-7"), vec!["LIF-7"]);
    }

    #[test]
    fn one_message_can_close_several_issues() {
        assert_eq!(
            refs("Closes LIF-1 and fixes LIF-2; also resolved LIF-3"),
            vec!["LIF-1", "LIF-2", "LIF-3"]
        );
    }

    #[test]
    fn repeated_references_are_deduplicated_in_first_seen_order() {
        assert_eq!(
            refs("closes LIF-2, fixes LIF-1, closes lif-2"),
            vec!["LIF-2", "LIF-1"]
        );
    }

    #[test]
    fn a_mention_without_a_keyword_is_not_a_closing_reference() {
        for text in [
            "see LIF-42",
            "part of LIF-42",
            "LIF-42",
            "related to LIF-42 for context",
            "reverts LIF-42",
        ] {
            assert!(refs(text).is_empty(), "'{text}' must not close anything");
        }
    }

    #[test]
    fn punctuation_ends_an_identifier() {
        assert_eq!(refs("closes LIF-42."), vec!["LIF-42"]);
        assert_eq!(refs("(fixes LIF-42)"), vec!["LIF-42"]);
        assert_eq!(refs("closes LIF-42, at last"), vec!["LIF-42"]);
        assert_eq!(refs("closes LIF-42\n"), vec!["LIF-42"]);
    }

    #[test]
    fn a_keyword_needs_its_own_word_boundary() {
        assert!(refs("uncloses LIF-42").is_empty());
        assert!(refs("prefixes LIF-42").is_empty());
        assert!(refs("closesLIF-42").is_empty());
    }

    #[test]
    fn a_trailing_word_character_disqualifies_the_identifier() {
        assert!(refs("closes LIF-42a").is_empty());
        assert!(refs("closes LIF-42_").is_empty());
    }

    #[test]
    fn a_malformed_identifier_does_not_swallow_the_next_reference() {
        assert_eq!(refs("closes closes LIF-1"), vec!["LIF-1"]);
        assert_eq!(refs("fixes nothing, fixes LIF-9"), vec!["LIF-9"]);
    }

    #[test]
    fn the_separator_may_be_any_whitespace_including_a_newline() {
        assert_eq!(refs("closes\nLIF-42"), vec!["LIF-42"]);
        assert_eq!(refs("closes   \t LIF-42"), vec!["LIF-42"]);
    }

    #[test]
    fn identifiers_may_carry_digits_before_the_dash() {
        assert_eq!(refs("closes AB1C-3"), vec!["AB1C-3"]);
        assert!(
            refs("closes 1LIF-3").is_empty(),
            "an identifier starts with a letter"
        );
    }

    #[test]
    fn non_ascii_text_around_a_reference_is_handled_without_panicking() {
        assert_eq!(refs("dépôt: closes LIF-42 ✅"), vec!["LIF-42"]);
        assert!(refs("naïve LIF-42").is_empty());
    }

    #[test]
    fn several_messages_deduplicate_against_each_other() {
        let messages = [
            "Add the thing\n\nCloses LIF-1",
            "Fix the other thing\n\nFixes LIF-2",
            "Follow-up\n\ncloses lif-1",
        ];
        assert_eq!(closing_references_in(&messages), vec!["LIF-1", "LIF-2"]);
    }

    #[test]
    fn an_empty_input_finds_nothing() {
        assert!(closing_references("").is_empty());
        assert!(closing_references_in::<&str>(&[]).is_empty());
    }
}
