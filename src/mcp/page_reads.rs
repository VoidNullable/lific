//! Bounded page reads for `get_page` (LIF-479).
//!
//! Agents keep long-lived "current state" pages, and a real one reached
//! 145,000 characters. Harnesses truncate tool output silently, so a plain
//! read of such a page loses its tail without the agent noticing. Above
//! [`PAGE_READ_BUDGET`], `get_page` therefore returns the outline plus the
//! first part of the body and says how to read one section at a time.

use std::fmt::{self, Display, Write as _};

/// The most page content one `get_page` response carries, in characters.
///
/// OpenCode truncates tool output near 50 KB and Claude Code near 25,000
/// tokens. 25,000 tokens of English markdown is roughly 90,000 characters,
/// so 50 KB is the limit that binds. 30,000 characters is at most 30 KB of
/// ASCII, which leaves room under 50 KB for the header, the outline, JSON
/// escaping and multi-byte text. Page writes warn above this size (LIF-481).
pub(crate) const PAGE_READ_BUDGET: usize = 30_000;

/// Share of the budget the outline may take in an oversized read; the rest
/// goes to the start of the body.
const FALLBACK_OUTLINE_BUDGET: usize = PAGE_READ_BUDGET / 3;

/// One ATX heading and the byte range of its section: the heading line and
/// everything under it until the next heading of the same or a higher level.
pub(crate) struct Heading {
    pub(crate) level: usize,
    pub(crate) text: String,
    /// GitHub-style anchor slug, made unique with `-1`, `-2` suffixes.
    pub(crate) anchor: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
    /// Another heading has the same text (case-insensitively), so only the
    /// anchor selects this one.
    pub(crate) duplicate: bool,
}

/// Character count with thousands separators, for sizes agents compare.
pub(crate) struct Chars(pub(crate) usize);

impl Display for Chars {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let digits = self.0.to_string();
        for (index, digit) in digits.chars().enumerate() {
            if index > 0 && (digits.len() - index).is_multiple_of(3) {
                f.write_char(',')?;
            }
            f.write_char(digit)?;
        }
        Ok(())
    }
}

pub(crate) fn char_len(text: &str) -> usize {
    text.chars().count()
}

/// Parses an ATX heading line (already stripped of up to three spaces of
/// indentation) into its level and text.
fn atx_heading(line: &str) -> Option<(usize, String)> {
    let level = line.bytes().take_while(|b| *b == b'#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &line[level..];
    if !(rest.is_empty() || rest.starts_with([' ', '\t'])) {
        return None;
    }
    let mut text = rest.trim();
    // An optional closing sequence of `#`s, only when set off by a space
    // (so `C#` keeps its `#`).
    let unclosed = text.trim_end_matches('#');
    if unclosed.is_empty() || unclosed.ends_with([' ', '\t']) {
        text = unclosed.trim_end();
    }
    (!text.is_empty()).then(|| (level, text.to_string()))
}

fn slug(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

/// Every ATX heading outside fenced code blocks, in document order.
pub(crate) fn headings(content: &str) -> Vec<Heading> {
    let mut found: Vec<Heading> = Vec::new();
    let mut fence: Option<(u8, usize)> = None;
    let mut offset = 0;
    for raw in content.split_inclusive('\n') {
        let start = offset;
        offset += raw.len();
        let line = raw.trim_end_matches(['\n', '\r']);
        let body = line.trim_start_matches(' ');
        if line.len() - body.len() > 3 {
            continue;
        }
        let marker = body.bytes().next().filter(|b| *b == b'`' || *b == b'~');
        let run = marker.map_or(0, |m| body.bytes().take_while(|b| *b == m).count());
        if let Some((open, open_run)) = fence {
            if marker == Some(open) && run >= open_run && body[run..].trim().is_empty() {
                fence = None;
            }
            continue;
        }
        // A backtick fence's info string cannot itself contain a backtick.
        let opens = run >= 3 && !(marker == Some(b'`') && body[run..].contains('`'));
        if let Some(m) = marker.filter(|_| opens) {
            fence = Some((m, run));
            continue;
        }
        if let Some((level, text)) = atx_heading(body) {
            found.push(Heading {
                level,
                text,
                anchor: String::new(),
                start,
                end: content.len(),
                duplicate: false,
            });
        }
    }

    for index in 0..found.len() {
        let level = found[index].level;
        if let Some(next) = found[index + 1..].iter().find(|h| h.level <= level) {
            found[index].end = next.start;
        }
    }

    let mut used = std::collections::HashSet::new();
    let mut texts = std::collections::HashMap::<String, usize>::new();
    for heading in &mut found {
        let base = slug(&heading.text);
        let mut anchor = base.clone();
        let mut suffix = 0;
        while !used.insert(anchor.clone()) {
            suffix += 1;
            anchor = format!("{base}-{suffix}");
        }
        heading.anchor = anchor;
        *texts.entry(heading.text.to_lowercase()).or_default() += 1;
    }
    for heading in &mut found {
        heading.duplicate = texts[&heading.text.to_lowercase()] > 1;
    }
    found
}

enum SectionLookup<'a> {
    Found(&'a Heading),
    Ambiguous(Vec<&'a Heading>),
    Missing,
}

/// An exact anchor wins (anchors are unique); otherwise the heading text,
/// case-insensitively. Leading `#`s in the query are ignored, so
/// `## Current state` and `#current-state` both work.
fn find_section<'a>(headings: &'a [Heading], query: &str) -> SectionLookup<'a> {
    let query = query.trim().trim_start_matches('#').trim();
    if let Some(heading) = headings.iter().find(|h| h.anchor == query) {
        return SectionLookup::Found(heading);
    }
    let wanted = query.to_lowercase();
    let mut matches: Vec<&Heading> = headings
        .iter()
        .filter(|h| h.text.to_lowercase() == wanted)
        .collect();
    match matches.len() {
        0 => SectionLookup::Missing,
        1 => SectionLookup::Found(matches.remove(0)),
        _ => SectionLookup::Ambiguous(matches),
    }
}

fn heading_line(out: &mut String, heading: &Heading) -> fmt::Result {
    write!(out, "{} {}", "#".repeat(heading.level), heading.text)?;
    if heading.duplicate {
        write!(out, " [{}]", heading.anchor)?;
    }
    Ok(())
}

/// The outline lines for headings up to `max_level`, indented relative to
/// the shallowest heading, each with its section size in characters.
fn outline_lines(content: &str, headings: &[Heading], max_level: usize) -> String {
    let top = headings.iter().map(|h| h.level).min().unwrap_or(1);
    let mut out = String::new();
    for heading in headings.iter().filter(|h| h.level <= max_level) {
        let _ = write!(out, "{}", "  ".repeat(heading.level - top));
        let _ = heading_line(&mut out, heading);
        let _ = writeln!(
            out,
            " ({})",
            Chars(char_len(&content[heading.start..heading.end]))
        );
    }
    out
}

fn write_outline(out: &mut String, content: &str, seq: i64) -> fmt::Result {
    let headings = headings(content);
    writeln!(
        out,
        "\nOutline (seq {seq}, {} chars; sizes include subsections):",
        Chars(char_len(content))
    )?;
    if headings.is_empty() {
        return writeln!(out, "No headings.");
    }
    out.push_str(&outline_lines(content, &headings, 6));
    Ok(())
}

/// The first `max_chars` characters of `body`, cut back to a line break when
/// one falls in the second half so the excerpt does not end mid-line.
fn prefix(body: &str, max_chars: usize) -> &str {
    let cut = body
        .char_indices()
        .nth(max_chars)
        .map_or(body.len(), |(index, _)| index);
    let at_line = body[..cut]
        .rfind('\n')
        .filter(|newline| *newline >= cut / 2)
        .map_or(cut, |newline| newline + 1);
    &body[..at_line]
}

/// An oversized page or section: the outline (trimmed to its share of the
/// budget) and the start of the body, with the call that reads the rest.
fn write_oversized(
    out: &mut String,
    identifier: &str,
    noun: &str,
    body: &str,
    seq: i64,
) -> fmt::Result {
    let total = char_len(body);
    let headings = headings(body);
    let mut max_level = 6;
    let mut outline = outline_lines(body, &headings, max_level);
    while max_level > 1 && char_len(&outline) > FALLBACK_OUTLINE_BUDGET {
        max_level -= 1;
        outline = outline_lines(body, &headings, max_level);
    }
    let cut_short = char_len(&outline) > FALLBACK_OUTLINE_BUDGET;
    if cut_short {
        let keep = prefix(&outline, FALLBACK_OUTLINE_BUDGET).len();
        outline.truncate(keep);
    }
    let outline_note = if cut_short {
        " The outline is cut short; outline=true lists it all."
    } else if headings.iter().any(|h| h.level > max_level) {
        " Deeper headings are omitted; outline=true lists them all."
    } else {
        ""
    };
    let shown = prefix(body, PAGE_READ_BUDGET.saturating_sub(char_len(&outline)));
    let shown_chars = char_len(shown);

    writeln!(
        out,
        "\nThis {noun} is {} chars, over the {}-char read budget, so only its outline and first {} chars follow.",
        Chars(total),
        Chars(PAGE_READ_BUDGET),
        Chars(shown_chars)
    )?;
    if headings.is_empty() {
        writeln!(
            out,
            "It has no headings, so it cannot be read by section. Split it into smaller pages."
        )?;
    } else {
        writeln!(
            out,
            "Read the rest one section at a time: get_page(identifier=\"{identifier}\", section=\"<heading text or anchor>\").{outline_note}"
        )?;
        writeln!(out, "\nOutline (seq {seq}; sizes include subsections):")?;
        out.push_str(&outline);
    }
    writeln!(out, "\nStart of the {noun}:\n{shown}")?;
    writeln!(
        out,
        "[Truncated at {} of {} chars.]",
        Chars(shown_chars),
        Chars(total)
    )
}

/// Everything `get_page` prints after the page header.
///
/// A plain read of a page within the budget is the content exactly as it
/// has always been rendered, so small pages are unchanged byte for byte.
pub(crate) fn page_body(
    identifier: &str,
    content: &str,
    seq: i64,
    section: Option<&str>,
    outline: bool,
) -> Result<String, String> {
    let mut out = String::new();
    let rendered = match section {
        Some(query) => {
            let headings = headings(content);
            let heading = select_section(&headings, identifier, content, query)?;
            write_section(&mut out, identifier, content, seq, heading, outline)
        }
        None if outline => write_outline(&mut out, content, seq),
        None if char_len(content) > PAGE_READ_BUDGET => {
            write_oversized(&mut out, identifier, "page", content, seq)
        }
        None if content.is_empty() => Ok(()),
        None => writeln!(out, "\n{content}"),
    };
    rendered.map_err(|error| format!("failed to format response: {error}"))?;
    Ok(out)
}

/// The heading `query` names, or an error that lists what an agent can pass
/// instead: every candidate's unique anchor when the text is ambiguous.
fn select_section<'a>(
    headings: &'a [Heading],
    identifier: &str,
    content: &str,
    query: &str,
) -> Result<&'a Heading, String> {
    match find_section(headings, query) {
        SectionLookup::Found(heading) => Ok(heading),
        SectionLookup::Missing => Err(format!(
            "no heading in {identifier} matches '{query}'. Pass outline=true to list its headings."
        )),
        SectionLookup::Ambiguous(candidates) => {
            let mut message = format!(
                "'{query}' matches {} headings in {identifier}; pass one anchor as section:",
                candidates.len()
            );
            for candidate in candidates {
                let _ = write!(
                    message,
                    "\n- {} {} [{}]",
                    "#".repeat(candidate.level),
                    candidate.text,
                    candidate.anchor
                );
                let parent = headings
                    .iter()
                    .rev()
                    .find(|h| h.start < candidate.start && h.level < candidate.level);
                if let Some(parent) = parent {
                    let _ = write!(message, " under \"{}\"", parent.text);
                }
                let _ = write!(
                    message,
                    " ({} chars)",
                    Chars(char_len(&content[candidate.start..candidate.end]))
                );
            }
            Err(message)
        }
    }
}

fn write_section(
    out: &mut String,
    identifier: &str,
    content: &str,
    seq: i64,
    heading: &Heading,
    outline: bool,
) -> fmt::Result {
    let body = &content[heading.start..heading.end];
    write!(out, "Section: ")?;
    heading_line(out, heading)?;
    writeln!(
        out,
        " ({} of {} chars, seq {seq})",
        Chars(char_len(body)),
        Chars(char_len(content))
    )?;
    if outline {
        write_outline(out, body, seq)
    } else if char_len(body) > PAGE_READ_BUDGET {
        write_oversized(out, identifier, "section", body, seq)
    } else {
        writeln!(out, "\n{}", body.trim_end())
    }
}

/// What `get_page(since_seq=...)` prints after the page header (LIF-480):
/// a unified diff of the content between `since` and now, with two lines of
/// context around each change. `before` is `None` when `since` is outside
/// the page's recorded history; the full read follows a note saying so.
pub(crate) fn page_changes(
    identifier: &str,
    before: Option<&str>,
    content: &str,
    since: i64,
    seq: i64,
) -> Result<String, String> {
    let Some(before) = before else {
        let body = page_body(identifier, content, seq, None, false)?;
        return Ok(format!(
            "Note: seq {since} is outside this page's recorded history (its latest 50 versions), so the full page follows.\n{body}"
        ));
    };
    let mut out = String::new();
    let rendered = if before == content {
        writeln!(
            out,
            "\nContent unchanged since seq {since} (now seq {seq})."
        )
    } else {
        write_diff(&mut out, before, content, since, seq)
    };
    rendered.map_err(|error| format!("failed to format response: {error}"))?;
    Ok(out)
}

fn write_diff(out: &mut String, before: &str, after: &str, since: i64, seq: i64) -> fmt::Result {
    let diff = similar::TextDiff::from_lines(before, after);
    let hunks = diff.unified_diff().context_radius(2).to_string();
    let total = char_len(&hunks);
    let shown = prefix(&hunks, PAGE_READ_BUDGET);
    // A fence longer than any backtick run in the diff, since page content
    // often carries its own code fences.
    let longest = shown.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    writeln!(
        out,
        "\nContent changes since seq {since} (now seq {seq}):\n{fence}diff\n{shown}{fence}"
    )?;
    if shown.len() < hunks.len() {
        writeln!(
            out,
            "[Diff truncated at {} of {} chars. Read the changed sections with section= instead.]",
            Chars(char_len(shown)),
            Chars(total)
        )?;
    }
    Ok(())
}
