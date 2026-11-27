//! Per-page metadata for the web Pages tab: a plain-text excerpt, a word
//! count and the incoming references a page collects from the rest of its
//! project.
//!
//! Everything here is a pure function over text. The query that feeds it
//! (`db::queries::page_meta`) reads the rows; this module decides what they
//! mean. Nothing here is a full Markdown parser: it recognises the block and
//! inline shapes a page body actually uses, well enough to show a reader the
//! first sentence of a page, and treats everything else as text.
//!
//! The reference grammar is hand-scanned rather than compiled, for the reason
//! `issue_refs` gives: `regex` is not a dependency of this crate. Each text is
//! still read once, and every page's references come out of that one pass.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

/// Longest excerpt, in Unicode scalar values, including the ellipsis that
/// marks a cut.
pub const EXCERPT_MAX_CHARS: usize = 220;

/// Most referring issues and pages listed for one page.
pub const LINKED_FROM_MAX: usize = 20;

const ELLIPSIS: char = '…';

// ── Block structure ─────────────────────────────────────────────

/// An open fenced code block: its fence character and run length.
struct Fence {
    ch: u8,
    len: usize,
}

/// The fence a line opens, if it opens one: up to three spaces, then three
/// or more backticks or tildes. A backtick fence's info string may not hold
/// a backtick, or the line is inline code instead.
fn fence_open(line: &str) -> Option<Fence> {
    let rest = line.trim_start_matches(' ');
    if line.len() - rest.len() > 3 {
        return None;
    }
    let ch = *rest.as_bytes().first()?;
    if ch != b'`' && ch != b'~' {
        return None;
    }
    let len = rest.bytes().take_while(|&b| b == ch).count();
    if len < 3 || (ch == b'`' && rest[len..].contains('`')) {
        return None;
    }
    Some(Fence { ch, len })
}

/// Whether a line closes `fence`: the same character, at least as many, and
/// nothing after but whitespace.
fn fence_closes(line: &str, fence: &Fence) -> bool {
    let rest = line.trim_start_matches(' ');
    if line.len() - rest.len() > 3 {
        return false;
    }
    let len = rest.bytes().take_while(|&b| b == fence.ch).count();
    len >= fence.len && rest[len..].trim().is_empty()
}

/// Index of the first body line after YAML front matter, or 0 when the body
/// does not open with a closed `---` block.
fn front_matter_end(lines: &[&str]) -> usize {
    if lines.first().map(|line| line.trim_end()) != Some("---") {
        return 0;
    }
    lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, line)| matches!(line.trim_end(), "---" | "..."))
        .map_or(0, |(index, _)| index + 1)
}

/// Strip blockquote markers (`>` with an optional following space), however
/// deeply nested.
fn strip_quote(mut line: &str) -> &str {
    loop {
        let rest = line.trim_start_matches(' ');
        if line.len() - rest.len() > 3 {
            return line;
        }
        match rest.strip_prefix('>') {
            Some(inner) => line = inner.strip_prefix(' ').unwrap_or(inner),
            None => return line,
        }
    }
}

/// The text of an ATX heading line (`# Title`, `## Title ##`).
fn atx_heading(trimmed: &str) -> Option<&str> {
    let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &trimmed[hashes..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let text = rest.trim();
    let closed = text.trim_end_matches('#');
    if closed.is_empty() || closed.ends_with([' ', '\t']) {
        return Some(closed.trim_end());
    }
    Some(text)
}

/// A setext underline: a run of `=` or of `-` and nothing else.
fn is_setext_underline(trimmed: &str) -> bool {
    !trimmed.is_empty()
        && (trimmed.bytes().all(|b| b == b'=') || trimmed.bytes().all(|b| b == b'-'))
}

/// A horizontal rule: three or more of one of `-`, `*`, `_`, spaces allowed.
fn is_thematic_break(trimmed: &str) -> bool {
    let Some(first) = trimmed.chars().next() else {
        return false;
    };
    if !matches!(first, '-' | '*' | '_') {
        return false;
    }
    let mut count = 0;
    for ch in trimmed.chars() {
        if ch == first {
            count += 1;
        } else if !ch.is_whitespace() {
            return false;
        }
    }
    count >= 3
}

/// The text of a list item, marker and task box removed: `- `, `* `, `+ `,
/// `1. ` or `1) `, then an optional `[ ] ` / `[x] `.
fn list_item(line: &str) -> Option<&str> {
    let rest = line.trim_start();
    let bytes = rest.as_bytes();
    let marker = match bytes.first()? {
        b'-' | b'*' | b'+' => 1,
        b'0'..=b'9' => {
            let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
            if digits > 9 || !matches!(bytes.get(digits), Some(b'.' | b')')) {
                return None;
            }
            digits + 1
        }
        _ => return None,
    };
    let after = &rest[marker..];
    if !after.is_empty() && !after.starts_with([' ', '\t']) {
        return None;
    }
    let text = after.trim_start();
    for box_ in ["[ ]", "[x]", "[X]"] {
        if let Some(tail) = text.strip_prefix(box_)
            && (tail.is_empty() || tail.starts_with([' ', '\t']))
        {
            return Some(tail.trim_start());
        }
    }
    Some(text)
}

/// Code indented by four spaces or a tab.
fn is_indented_code(line: &str) -> bool {
    line.starts_with('\t') || line.starts_with("    ")
}

/// A line that is nothing but HTML tags, such as `<div align="center">` or
/// `</details>`.
fn is_html_line(trimmed: &str) -> bool {
    let chars: Vec<char> = trimmed.chars().collect();
    let mut index = 0;
    let mut saw_tag = false;
    while index < chars.len() {
        if chars[index].is_whitespace() {
            index += 1;
            continue;
        }
        match html_tag_end(&chars, index) {
            Some(end) => {
                saw_tag = true;
                index = end;
            }
            None => return false,
        }
    }
    saw_tag
}

// ── Inline text ─────────────────────────────────────────────────

/// Where an HTML tag starting at `start` ends (one past its `>`): `<name`,
/// `</name`, then attributes up to `>`. An autolink (`<https://...>`) is not
/// a tag, because its scheme runs into a `:`.
fn html_tag_end(chars: &[char], start: usize) -> Option<usize> {
    if chars.get(start) != Some(&'<') {
        return None;
    }
    let mut index = start + 1;
    if chars.get(index) == Some(&'/') {
        index += 1;
    }
    if !chars.get(index).is_some_and(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    while chars
        .get(index)
        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '-')
    {
        index += 1;
    }
    match chars.get(index) {
        Some('>') => Some(index + 1),
        Some(c) if c.is_whitespace() || *c == '/' => chars[index..]
            .iter()
            .position(|&c| c == '>')
            .map(|offset| index + offset + 1),
        _ => None,
    }
}

/// An autolink starting at `start`: `<scheme:...>` or `<user@host>`. Returns
/// where it ends and the address inside.
fn autolink_at(chars: &[char], start: usize) -> Option<(usize, String)> {
    if chars.get(start) != Some(&'<') {
        return None;
    }
    let close = chars[start + 1..].iter().position(|&c| c == '>')? + start + 1;
    let inner: String = chars[start + 1..close].iter().collect();
    if inner.is_empty() || inner.chars().any(|c| c.is_whitespace() || c == '<') {
        return None;
    }
    let is_uri = inner.split_once(':').is_some_and(|(scheme, _)| {
        (2..=32).contains(&scheme.len())
            && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
    });
    let is_email = inner
        .split_once('@')
        .is_some_and(|(user, host)| !user.is_empty() && host.contains('.'));
    (is_uri || is_email).then_some((close + 1, inner))
}

/// The bracketed part of a link starting at `start` (`[`): returns where the
/// whole link ends and the range of its text. Handles `[text](url)` (with
/// nested parentheses in the url) and `[text][ref]`; a bare `[text]` is not a
/// link.
fn link_at(chars: &[char], start: usize) -> Option<(usize, std::ops::Range<usize>)> {
    let close = matching(chars, start, '[', ']')?;
    let text = start + 1..close;
    match chars.get(close + 1) {
        Some('(') => matching(chars, close + 1, '(', ')').map(|end| (end + 1, text)),
        Some('[') => matching(chars, close + 1, '[', ']').map(|end| (end + 1, text)),
        _ => None,
    }
}

/// Index of the bracket matching the `open` at `start`, honouring nesting
/// and backslash escapes.
fn matching(chars: &[char], start: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0usize;
    let mut index = start;
    while index < chars.len() {
        let ch = chars[index];
        if ch == '\\' {
            index += 2;
            continue;
        }
        if ch == open {
            depth += 1;
        } else if ch == close {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
        index += 1;
    }
    None
}

/// Reduce inline Markdown to the text a reader sees: link text kept, images
/// dropped, autolinks unwrapped, code spans unquoted, emphasis markers and
/// inline tags removed, escapes resolved, whitespace collapsed.
fn reduce_inline(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    reduce_into(&chars, &mut out);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn reduce_into(chars: &[char], out: &mut String) {
    let at = |index: usize| chars.get(index).copied();
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        match ch {
            '\\' if at(index + 1).is_some_and(|c| c.is_ascii_punctuation()) => {
                out.push(chars[index + 1]);
                index += 2;
            }
            '`' => {
                let run = chars[index..].iter().take_while(|&&c| c == '`').count();
                match code_span_close(chars, index + run, run) {
                    Some(close) => {
                        let code: String = chars[index + run..close].iter().collect();
                        let trimmed = if code.len() > 2
                            && code.starts_with(' ')
                            && code.ends_with(' ')
                            && !code.trim().is_empty()
                        {
                            &code[1..code.len() - 1]
                        } else {
                            code.as_str()
                        };
                        out.push_str(trimmed);
                        index = close + run;
                    }
                    None => {
                        out.extend(std::iter::repeat_n('`', run));
                        index += run;
                    }
                }
            }
            '!' if at(index + 1) == Some('[') => match link_at(chars, index + 1) {
                Some((end, _)) => index = end,
                None => {
                    out.push('!');
                    index += 1;
                }
            },
            '[' => match link_at(chars, index) {
                Some((end, text)) => {
                    reduce_into(&chars[text], out);
                    index = end;
                }
                None => {
                    out.push('[');
                    index += 1;
                }
            },
            '<' => {
                if let Some((end, address)) = autolink_at(chars, index) {
                    out.push_str(&address);
                    index = end;
                } else if let Some(end) = html_tag_end(chars, index) {
                    index = end;
                } else {
                    out.push('<');
                    index += 1;
                }
            }
            '*' => {
                // A lone `*` between spaces is arithmetic, not emphasis.
                let before = index.checked_sub(1).and_then(at);
                let after = at(index + 1);
                if before.is_none_or(char::is_whitespace) && after.is_none_or(char::is_whitespace) {
                    out.push('*');
                }
                index += 1;
            }
            '_' => {
                // Intraword underscores (`snake_case`) are not emphasis.
                let before = index.checked_sub(1).and_then(at);
                let after = at(index + 1);
                if before.is_some_and(char::is_alphanumeric)
                    && after.is_some_and(char::is_alphanumeric)
                {
                    out.push('_');
                }
                index += 1;
            }
            '~' if at(index + 1) == Some('~') => index += 2,
            _ => {
                out.push(ch);
                index += 1;
            }
        }
    }
}

/// Where a code span opened by `run` backticks closes: the next run of
/// exactly that many.
fn code_span_close(chars: &[char], from: usize, run: usize) -> Option<usize> {
    let mut index = from;
    while index < chars.len() {
        if chars[index] == '`' {
            let len = chars[index..].iter().take_while(|&&c| c == '`').count();
            if len == run {
                return Some(index);
            }
            index += len;
        } else {
            index += 1;
        }
    }
    None
}

/// Cut `text` to [`EXCERPT_MAX_CHARS`] at the last word boundary that fits,
/// marking the cut with an ellipsis.
fn truncate_at_word(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= EXCERPT_MAX_CHARS {
        return text.to_string();
    }
    let budget = EXCERPT_MAX_CHARS - 1;
    let head: String = chars[..budget].iter().collect();
    let cut = if chars[budget].is_whitespace() {
        head.as_str()
    } else {
        match head.rfind(char::is_whitespace) {
            Some(position) if position > 0 => &head[..position],
            _ => head.as_str(),
        }
    };
    let cut = cut.trim_end().trim_end_matches([',', ';', ':']).trim_end();
    format!("{cut}{ELLIPSIS}")
}

// ── Excerpt ─────────────────────────────────────────────────────

/// What the line being read belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    None,
    Paragraph,
    ListItem,
}

/// Collects blocks in order of preference: the first prose paragraph wins;
/// without one, the items of the first list; without those, the first
/// heading.
#[derive(Default)]
struct ExcerptBuilder<'a> {
    lines: Vec<&'a str>,
    list_items: Vec<String>,
    list_closed: bool,
    heading: Option<String>,
}

impl<'a> ExcerptBuilder<'a> {
    /// Finish the open block. Returns the excerpt once a paragraph with
    /// visible text has been read.
    fn end(&mut self, block: Block) -> Option<String> {
        let text = reduce_inline(&self.lines.join(" "));
        self.lines.clear();
        match block {
            Block::Paragraph if !text.is_empty() => return Some(text),
            Block::ListItem if !self.list_closed && !text.is_empty() => self.list_items.push(text),
            _ => {}
        }
        None
    }

    /// A block that is not a list item began, so the first list is over.
    fn close_list(&mut self) {
        if !self.list_items.is_empty() {
            self.list_closed = true;
        }
    }

    fn heading(&mut self, text: &str) {
        self.close_list();
        if self.heading.is_none() {
            let text = reduce_inline(text);
            if !text.is_empty() {
                self.heading = Some(text);
            }
        }
    }

    fn fallback(self) -> String {
        if !self.list_items.is_empty() {
            let mut joined = String::new();
            for item in &self.list_items {
                if !joined.is_empty() {
                    let ends_sentence = joined.ends_with(['.', '!', '?', ';', ':', ',']);
                    joined.push_str(if ends_sentence { " " } else { "; " });
                }
                joined.push_str(item);
            }
            return joined;
        }
        self.heading.unwrap_or_default()
    }
}

/// The first real prose of a page body, as plain text of at most
/// [`EXCERPT_MAX_CHARS`] characters. Headings, code, tables, rules, HTML and
/// front matter are skipped; list and quote markers are stripped. A body
/// with no prose falls back to its first list, then its first heading, then
/// the empty string.
pub fn excerpt(body: &str) -> String {
    truncate_at_word(&excerpt_text(body))
}

fn excerpt_text(body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let mut builder = ExcerptBuilder::default();
    let mut block = Block::None;
    let mut fence: Option<Fence> = None;
    let mut in_comment = false;

    macro_rules! end_block {
        () => {{
            if let Some(text) = builder.end(block) {
                return text;
            }
            block = Block::None;
        }};
    }

    for raw in &lines[front_matter_end(&lines)..] {
        if let Some(open) = &fence {
            if fence_closes(raw, open) {
                fence = None;
            }
            continue;
        }
        if in_comment {
            in_comment = !raw.contains("-->");
            continue;
        }
        let line = strip_quote(raw);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            end_block!();
            continue;
        }
        if let Some(open) = fence_open(line) {
            end_block!();
            builder.close_list();
            fence = Some(open);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("<!--") {
            end_block!();
            in_comment = !rest.contains("-->");
            continue;
        }
        if is_html_line(trimmed) {
            end_block!();
            continue;
        }
        if let Some(text) = atx_heading(trimmed) {
            end_block!();
            builder.heading(text);
            continue;
        }
        if block == Block::Paragraph && is_setext_underline(trimmed) {
            // The paragraph above was a heading all along.
            let text = builder.lines.join(" ");
            builder.lines.clear();
            block = Block::None;
            builder.heading(&text);
            continue;
        }
        if is_thematic_break(trimmed) || trimmed.starts_with('|') {
            end_block!();
            builder.close_list();
            continue;
        }
        if let Some(text) = list_item(line) {
            if let Some(text) = builder.end(block) {
                return text;
            }
            block = Block::ListItem;
            builder.lines.push(text);
            continue;
        }
        if block == Block::None && is_indented_code(line) {
            builder.close_list();
            continue;
        }
        if block == Block::None {
            block = Block::Paragraph;
        }
        builder.lines.push(trimmed);
    }
    if let Some(text) = builder.end(block) {
        return text;
    }
    builder.fallback()
}

// ── Word count ──────────────────────────────────────────────────

/// Words in a page body: whitespace-separated tokens holding at least one
/// letter or digit, outside fenced code blocks. Markdown punctuation on its
/// own (`-`, `#`, `|`, `---`) is not a word.
pub fn word_count(body: &str) -> usize {
    let mut fence: Option<Fence> = None;
    let mut words = 0;
    for line in body.lines() {
        if let Some(open) = &fence {
            if fence_closes(line, open) {
                fence = None;
            }
            continue;
        }
        if let Some(open) = fence_open(line) {
            fence = Some(open);
            continue;
        }
        words += line
            .split_whitespace()
            .filter(|token| token.chars().any(char::is_alphanumeric))
            .count();
    }
    words
}

// ── References ──────────────────────────────────────────────────

/// One reference to a page found in text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PageRef {
    /// `LIF-DOC-7`: the page's sequence number in its project.
    Sequence(i64),
    /// `/LIF/pages/17`: the page's database id, as in a web link.
    Id(i64),
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The digits at the start of `bytes`, as a number. None when there are
/// none, or too many to be an id.
fn leading_number(bytes: &[u8]) -> Option<i64> {
    let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0 || digits > 18 {
        return None;
    }
    std::str::from_utf8(&bytes[..digits]).ok()?.parse().ok()
}

fn strip_prefix_ignore_case<'b>(bytes: &'b [u8], prefix: &[u8]) -> Option<&'b [u8]> {
    (bytes.len() >= prefix.len() && bytes[..prefix.len()].eq_ignore_ascii_case(prefix))
        .then(|| &bytes[prefix.len()..])
}

/// Every page reference to `project` in `text`, in order of appearance:
/// `{PROJECT}-DOC-{n}` (any case, starting at a word boundary, never a prefix
/// of a longer number) and the web path `/{PROJECT}/pages/{id}`, which also
/// covers `#/{PROJECT}/pages/{id}` and full URLs.
pub fn scan_page_refs(text: &str, project: &str) -> Vec<PageRef> {
    let bytes = text.as_bytes();
    let project = project.as_bytes();
    let mut refs = Vec::new();
    if project.is_empty() || bytes.len() < project.len() {
        return refs;
    }
    for start in 0..=bytes.len() - project.len() {
        let Some(rest) = strip_prefix_ignore_case(&bytes[start..], project) else {
            continue;
        };
        let before = start.checked_sub(1).map(|index| bytes[index]);
        if before.is_none_or(|b| !is_word_byte(b))
            && let Some(number) = strip_prefix_ignore_case(rest, b"-DOC-").and_then(leading_number)
        {
            refs.push(PageRef::Sequence(number));
        } else if before == Some(b'/')
            && let Some(number) =
                strip_prefix_ignore_case(rest, b"/pages/").and_then(leading_number)
        {
            refs.push(PageRef::Id(number));
        }
    }
    refs
}

/// How to turn a [`PageRef`] into a live page of the project.
#[derive(Debug, Default)]
pub struct PageKeys {
    /// Sequence number to page id.
    pub by_sequence: HashMap<i64, i64>,
    /// Ids of the project's live pages.
    pub ids: HashSet<i64>,
}

impl PageKeys {
    fn resolve(&self, reference: PageRef) -> Option<i64> {
        match reference {
            PageRef::Sequence(sequence) => self.by_sequence.get(&sequence).copied(),
            PageRef::Id(id) => self.ids.contains(&id).then_some(id),
        }
    }
}

/// What kind of entity holds a referring text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Issue,
    Page,
    Comment,
}

/// One entity whose text may refer to pages.
#[derive(Debug)]
pub struct Source<'a> {
    pub kind: SourceKind,
    /// The page this text belongs to: a page's own content, or a comment on
    /// a page. Its references to that page are not incoming links.
    pub own_page: Option<i64>,
    /// The identifier listed in `linked_from` (`LIF-12`, `LIF-DOC-3`). None
    /// for comments, which are counted but not listed.
    pub label: Option<String>,
    pub texts: Vec<&'a str>,
}

/// Incoming reference counts, one per referring entity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LinkCounts {
    pub issues: u32,
    pub pages: u32,
    pub comments: u32,
}

/// What one page collects from the rest of its project.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Incoming {
    pub links: LinkCounts,
    /// Referring issue and page identifiers, in source order, at most
    /// [`LINKED_FROM_MAX`].
    pub linked_from: Vec<String>,
}

/// Tally every page's incoming references from `sources`, which the caller
/// orders most recently updated first so `linked_from` comes out that way.
/// Each source counts once per page however often it mentions it.
pub fn tally_incoming(
    sources: &[Source<'_>],
    project: &str,
    keys: &PageKeys,
) -> HashMap<i64, Incoming> {
    let mut incoming: HashMap<i64, Incoming> = HashMap::new();
    for source in sources {
        let mut targets: Vec<i64> = Vec::new();
        for text in &source.texts {
            for reference in scan_page_refs(text, project) {
                if let Some(page) = keys.resolve(reference)
                    && Some(page) != source.own_page
                    && !targets.contains(&page)
                {
                    targets.push(page);
                }
            }
        }
        for page in targets {
            let entry = incoming.entry(page).or_default();
            match source.kind {
                SourceKind::Issue => entry.links.issues += 1,
                SourceKind::Page => entry.links.pages += 1,
                SourceKind::Comment => entry.links.comments += 1,
            }
            if let Some(label) = &source.label
                && entry.linked_from.len() < LINKED_FROM_MAX
                && !entry.linked_from.contains(label)
            {
                entry.linked_from.push(label.clone());
            }
        }
    }
    incoming
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── excerpt ──

    #[test]
    fn excerpt_takes_the_first_paragraph_after_headings() {
        let body = "# Storage\n\n## Why\n\nLific keeps everything in one SQLite file\nso a backup is a copy.\n\nSecond paragraph.";
        assert_eq!(
            excerpt(body),
            "Lific keeps everything in one SQLite file so a backup is a copy."
        );
    }

    #[test]
    fn excerpt_skips_fenced_code_of_both_kinds() {
        let body = "```rust\nfn main() {}\n\nlet x = 1;\n```\n~~~~\nplain tilde block\n~~~~\nAfter the code.";
        assert_eq!(excerpt(body), "After the code.");
    }

    #[test]
    fn excerpt_does_not_close_a_fence_on_a_shorter_run() {
        let body = "````\n```\nstill code\n````\nProse.";
        assert_eq!(excerpt(body), "Prose.");
    }

    #[test]
    fn excerpt_skips_indented_code_tables_rules_and_html() {
        let body = "    indented code\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n***\n\n<!-- a\nmultiline comment -->\n<div align=\"center\">\n</div>\n\nThe real text.";
        assert_eq!(excerpt(body), "The real text.");
    }

    #[test]
    fn excerpt_skips_yaml_front_matter() {
        let body = "---\ntitle: Notes\ntags: [a, b]\n---\n\nBody text here.";
        assert_eq!(excerpt(body), "Body text here.");
    }

    #[test]
    fn excerpt_treats_a_setext_heading_as_a_heading() {
        let body = "Overview\n========\n\nSection\n-------\nThe summary line.";
        assert_eq!(excerpt(body), "The summary line.");
    }

    #[test]
    fn excerpt_prefers_prose_over_an_earlier_list() {
        let body = "- first item\n- second item\n\nA paragraph after the list.";
        assert_eq!(excerpt(body), "A paragraph after the list.");
    }

    #[test]
    fn excerpt_falls_back_to_list_text_with_markers_and_task_boxes_stripped() {
        let body = "# Todo\n\n- [ ] buy milk\n- [x] write tests.\n* plain star\n1. numbered\n2) also numbered";
        assert_eq!(
            excerpt(body),
            "buy milk; write tests. plain star; numbered; also numbered"
        );
    }

    #[test]
    fn excerpt_uses_only_the_first_list() {
        let body = "- one\n- two\n\n## Later\n\n- three";
        assert_eq!(excerpt(body), "one; two");
    }

    #[test]
    fn excerpt_strips_blockquote_markers() {
        let body = "> Quoted wisdom\n> on two lines.";
        assert_eq!(excerpt(body), "Quoted wisdom on two lines.");
    }

    #[test]
    fn excerpt_reduces_links_images_and_autolinks() {
        let body = "See [the docs](https://lific.dev/docs (v2)) and ![logo](logo.png)[ref text][1] at <https://lific.dev> or <me@example.com>.";
        assert_eq!(
            excerpt(body),
            "See the docs and ref text at https://lific.dev or me@example.com."
        );
    }

    #[test]
    fn excerpt_strips_emphasis_and_keeps_code_text() {
        let body =
            "This is **bold**, *em*, _under_, ~~gone~~ and `a*b_c` with snake_case and 2 * 3.";
        assert_eq!(
            excerpt(body),
            "This is bold, em, under, gone and a*b_c with snake_case and 2 * 3."
        );
    }

    #[test]
    fn excerpt_resolves_escapes_and_strips_inline_tags() {
        let body = r"A literal \*star\* and <kbd>Ctrl</kbd> key.";
        assert_eq!(excerpt(body), "A literal *star* and Ctrl key.");
    }

    #[test]
    fn excerpt_truncates_at_a_word_boundary_with_an_ellipsis() {
        let body = "word ".repeat(100);
        let result = excerpt(&body);
        assert!(result.ends_with('…'), "{result}");
        assert!(result.chars().count() <= EXCERPT_MAX_CHARS);
        // 44 words fill 219 characters exactly, so the cut lands on a space.
        assert_eq!(result.trim_end_matches('…'), "word ".repeat(44).trim_end());
        // Here the limit falls inside the 28th word, which is dropped whole.
        let cut = excerpt(&"abcdefg ".repeat(40));
        assert_eq!(cut, format!("{}…", "abcdefg ".repeat(27).trim_end()));
    }

    #[test]
    fn excerpt_counts_unicode_scalars_not_bytes() {
        let body = "é".repeat(219);
        assert_eq!(excerpt(&body), body);
        let long = "é".repeat(300);
        let cut = excerpt(&long);
        assert_eq!(cut.chars().count(), EXCERPT_MAX_CHARS);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn excerpt_leaves_text_at_the_limit_alone() {
        let body = "a".repeat(EXCERPT_MAX_CHARS);
        assert_eq!(excerpt(&body), body);
    }

    #[test]
    fn excerpt_of_an_empty_or_code_only_body_is_empty() {
        assert_eq!(excerpt(""), "");
        assert_eq!(excerpt("\n\n  \n"), "");
        assert_eq!(excerpt("```\ncode only\n```"), "");
        assert_eq!(excerpt("---\n\n***"), "");
    }

    #[test]
    fn excerpt_of_a_headings_only_body_is_its_first_heading() {
        assert_eq!(excerpt("# Design *notes* #\n\n## Later"), "Design notes");
    }

    #[test]
    fn excerpt_does_not_treat_a_hash_word_as_a_heading() {
        assert_eq!(
            excerpt("#hashtag starts the line"),
            "#hashtag starts the line"
        );
    }

    // ── word count ──

    #[test]
    fn word_count_counts_prose_words() {
        assert_eq!(word_count("One two  three\nfour"), 4);
        assert_eq!(word_count(""), 0);
    }

    #[test]
    fn word_count_skips_fenced_code_and_its_fences() {
        let body = "Before code.\n```rust\nlet a = b + c;\n```\nAfter.";
        assert_eq!(word_count(body), 3);
    }

    #[test]
    fn word_count_ignores_punctuation_only_tokens() {
        let body = "# Title\n\n- item one\n> quoted\n---\n| a | b |\n**bold** text";
        assert_eq!(word_count(body), 8);
    }

    // ── references ──

    #[test]
    fn scan_finds_identifiers_case_insensitively() {
        assert_eq!(
            scan_page_refs("see LIF-DOC-7 and lif-doc-8, Lif-Doc-9.", "LIF"),
            vec![
                PageRef::Sequence(7),
                PageRef::Sequence(8),
                PageRef::Sequence(9)
            ]
        );
    }

    #[test]
    fn scan_does_not_match_a_longer_number() {
        assert_eq!(
            scan_page_refs("LIF-DOC-70", "LIF"),
            vec![PageRef::Sequence(70)]
        );
        assert!(!scan_page_refs("LIF-DOC-70", "LIF").contains(&PageRef::Sequence(7)));
    }

    #[test]
    fn scan_needs_a_word_boundary_before_the_identifier() {
        assert!(scan_page_refs("XLIF-DOC-7 LIF_LIF-DOC-8", "LIF").is_empty());
        assert_eq!(
            scan_page_refs("(LIF-DOC-7)", "LIF"),
            vec![PageRef::Sequence(7)]
        );
    }

    #[test]
    fn scan_ignores_other_projects_and_issue_identifiers() {
        assert!(scan_page_refs("PRO-DOC-1 LIF-12 LIF-DOC- LIF2-DOC-3", "LIF").is_empty());
    }

    #[test]
    fn scan_finds_web_link_paths() {
        assert_eq!(
            scan_page_refs(
                "[x](https://host/lific/LIF/pages/17) #/LIF/pages/18 /lif/pages/19x",
                "LIF"
            ),
            vec![PageRef::Id(17), PageRef::Id(18), PageRef::Id(19)]
        );
        assert!(scan_page_refs("LIF/pages/17 /LIF/page/17", "LIF").is_empty());
    }

    fn keys() -> PageKeys {
        PageKeys {
            by_sequence: HashMap::from([(1, 100), (2, 200)]),
            ids: HashSet::from([100, 200]),
        }
    }

    #[test]
    fn tally_counts_each_entity_once_per_page() {
        let sources = vec![Source {
            kind: SourceKind::Issue,
            own_page: None,
            label: Some("LIF-5".into()),
            texts: vec!["LIF-DOC-1 twice: LIF-DOC-1", "and /LIF/pages/100"],
        }];
        let incoming = tally_incoming(&sources, "LIF", &keys());
        assert_eq!(
            incoming[&100].links,
            LinkCounts {
                issues: 1,
                pages: 0,
                comments: 0
            }
        );
        assert_eq!(incoming[&100].linked_from, vec!["LIF-5"]);
    }

    #[test]
    fn tally_ignores_self_references_and_unknown_pages() {
        let sources = vec![
            Source {
                kind: SourceKind::Page,
                own_page: Some(100),
                label: Some("LIF-DOC-1".into()),
                texts: vec!["I am LIF-DOC-1, see LIF-DOC-2 and LIF-DOC-99"],
            },
            Source {
                kind: SourceKind::Comment,
                own_page: Some(200),
                label: None,
                texts: vec!["this page, LIF-DOC-2, links LIF-DOC-1"],
            },
        ];
        let incoming = tally_incoming(&sources, "LIF", &keys());
        assert_eq!(
            incoming[&200].links,
            LinkCounts {
                issues: 0,
                pages: 1,
                comments: 0
            }
        );
        assert_eq!(incoming[&200].linked_from, vec!["LIF-DOC-1"]);
        assert_eq!(
            incoming[&100].links,
            LinkCounts {
                issues: 0,
                pages: 0,
                comments: 1
            }
        );
        assert!(incoming[&100].linked_from.is_empty());
        assert_eq!(incoming.len(), 2);
    }

    #[test]
    fn tally_keeps_source_order_and_caps_linked_from() {
        let labels: Vec<String> = (1..=25).map(|n| format!("LIF-{n}")).collect();
        let sources: Vec<Source> = labels
            .iter()
            .map(|label| Source {
                kind: SourceKind::Issue,
                own_page: None,
                label: Some(label.clone()),
                texts: vec!["LIF-DOC-2"],
            })
            .collect();
        let incoming = tally_incoming(&sources, "LIF", &keys());
        assert_eq!(incoming[&200].links.issues, 25);
        assert_eq!(incoming[&200].linked_from.len(), LINKED_FROM_MAX);
        assert_eq!(incoming[&200].linked_from[0], "LIF-1");
        assert_eq!(incoming[&200].linked_from[19], "LIF-20");
    }
}
