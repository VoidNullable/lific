//! LIF-487: progress through the markdown task list in an issue description.
//!
//! Agents write acceptance criteria as task lists (`- [ ] tests pass`), and
//! nothing could read them back except by rereading the whole description.
//! This counts them so the read tools can show progress in a few tokens.

/// Checked and total task items in a markdown body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checklist {
    pub done: usize,
    pub total: usize,
}

impl Checklist {
    /// Task items still unchecked.
    pub fn open(self) -> usize {
        self.total - self.done
    }
}

/// Count the GFM task items in `text`, or `None` when it has none.
///
/// A task item is a list item (`-`, `*`, `+`, `1.` or `1)`) whose content
/// starts with `[ ]`, `[x]` or `[X]` followed by whitespace or the end of the
/// line. Nested items count like top-level ones. Lines inside fenced code
/// blocks (``` or ~~~) are skipped: a task list quoted as an example is not
/// the issue's own checklist.
pub fn checklist(text: &str) -> Option<Checklist> {
    let mut fence: Option<(char, usize)> = None;
    let mut done = 0;
    let mut total = 0;
    for line in text.lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if let Some((marker, length)) = fence_marker(trimmed).filter(|_| indent < 4) {
            match fence {
                None => fence = Some((marker, length)),
                // A closing fence repeats the opening character at least as
                // many times and carries nothing else.
                Some((open, open_length))
                    if marker == open
                        && length >= open_length
                        && trimmed[length..].trim().is_empty() =>
                {
                    fence = None
                }
                Some(_) => {}
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        if let Some(checked) = task_state(trimmed) {
            total += 1;
            done += usize::from(checked);
        }
    }
    (total > 0).then_some(Checklist { done, total })
}

/// The fence character and run length when `line` opens or closes a fence.
fn fence_marker(line: &str) -> Option<(char, usize)> {
    let marker = line.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let length = line.chars().take_while(|c| *c == marker).count();
    (length >= 3).then_some((marker, length))
}

/// `Some(checked)` when `line` (already left-trimmed) is a task item.
fn task_state(line: &str) -> Option<bool> {
    let rest = match line.as_bytes().first()? {
        b'-' | b'*' | b'+' => &line[1..],
        b'0'..=b'9' => {
            let digits = line.bytes().take_while(u8::is_ascii_digit).count();
            let rest = &line[digits..];
            rest.strip_prefix('.').or_else(|| rest.strip_prefix(')'))?
        }
        _ => return None,
    };
    let content = rest.trim_start_matches([' ', '\t']);
    if content.len() == rest.len() {
        return None;
    }
    let checked = match content.get(..3)? {
        "[ ]" => false,
        "[x]" | "[X]" => true,
        _ => return None,
    };
    match content[3..].chars().next() {
        None => Some(checked),
        Some(c) if c.is_whitespace() => Some(checked),
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(text: &str) -> Option<(usize, usize)> {
        checklist(text).map(|c| (c.done, c.total))
    }

    #[test]
    fn counts_checked_and_unchecked_items_in_every_bullet_style() {
        let text = "Done when:\n- [x] parser\n* [ ] tests\n+ [X] docs\n1. [ ] review\n2) [x] ship\n  - [ ] nested";
        assert_eq!(counts(text), Some((3, 6)));
    }

    #[test]
    fn a_body_without_task_items_has_no_checklist() {
        assert_eq!(counts(""), None);
        assert_eq!(
            counts("- plain bullet\n- [link](x)\n[ ] not a list item"),
            None
        );
        assert_eq!(counts("-[ ] no space\n- [x]done\n- [y] other"), None);
    }

    #[test]
    fn items_inside_fenced_code_blocks_are_ignored() {
        let text = "- [ ] real\n```markdown\n- [x] example\n- [ ] example\n```\n~~~\n- [x] tilde\n~~~\n- [x] also real";
        assert_eq!(counts(text), Some((1, 2)));
    }

    #[test]
    fn a_fence_closes_only_on_its_own_marker_and_length() {
        // The ``` inside the ```` block and the ~~~ inside the ``` block do
        // not close them, so both example items stay excluded.
        let text = "````\n```\n- [ ] quoted\n````\n```\n~~~\n- [x] quoted\n```\n- [ ] real";
        assert_eq!(counts(text), Some((0, 1)));
    }

    #[test]
    fn an_unclosed_fence_hides_everything_after_it() {
        assert_eq!(counts("- [x] before\n```\n- [ ] after"), Some((1, 1)));
    }
}
