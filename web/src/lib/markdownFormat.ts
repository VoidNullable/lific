// LIF-282 — pure, testable markdown-formatting transforms for the editor.
//
// Every function here takes an immutable "editor state" — the full text plus
// the current selection range — and returns the same shape describing the
// text and selection AFTER the transform. No DOM, no side effects: the
// EditableMarkdown component owns the textarea and is responsible for applying
// a result (ideally via document.execCommand('insertText') so native undo
// keeps working, falling back to value assignment). Keeping the string math
// here means it can be unit-tested without a browser and the component stays
// lean.
//
// Two families of transform:
//
//   1. Inline wraps (bold **, italic *, inline code `) — wrap the selection;
//      if it's already wrapped (markers inside OR immediately surrounding the
//      selection), unwrap instead so the buttons/shortcuts toggle. With an
//      empty selection, insert the marker pair and drop the caret between.
//
//   2. Line-prefix toggles (heading, bullet, numbered, checklist, quote) —
//      apply/remove a leading marker on every line the selection touches.
//      When every touched line already has the prefix we strip it; otherwise
//      we add it to the lines that lack it. Numbered lists renumber 1..n.
//
// Plus two one-offs: a fenced code block and a link insertion (both of which
// need bespoke caret handling described at their definitions).

/** The full editing state a transform reads and rewrites. `selectionStart`
 *  and `selectionEnd` are byte offsets into `text` (start ≤ end). */
export interface EditorState {
  text: string;
  selectionStart: number;
  selectionEnd: number;
}

/** The result of a transform: the new full text and where the selection
 *  should land afterwards. Shape-identical to EditorState so results can be
 *  fed back in for chained transforms in tests. */
export type TransformResult = EditorState;

/** Inline markers we support toggling. */
export type InlineMarker = "**" | "*" | "_" | "`";

/** Line-prefix kinds we support toggling. */
export type LinePrefixKind =
  "heading" | "bullet" | "numbered" | "checklist" | "quote";

// ── Inline wrap toggle ────────────────────────────────────

/**
 * Toggle an inline wrap (`**`, `*`, `_`, or `` ` ``) around the selection.
 *
 * - Empty selection → insert `marker + marker` and place the caret between
 *   them so the user can type inside.
 * - Selection already surrounded by the marker (either the markers sit just
 *   OUTSIDE the selection in `text`, or they're the first/last chars INSIDE
 *   the selection) → unwrap, keeping the inner text selected.
 * - Otherwise → wrap the selection, keeping the (now inner) text selected.
 */
export function toggleInlineWrap(
  state: EditorState,
  marker: InlineMarker,
): TransformResult {
  const { text, selectionStart: start, selectionEnd: end } = state;
  const m = marker;
  const mLen = m.length;

  // Empty selection: insert the pair, caret between.
  if (start === end) {
    const next = text.slice(0, start) + m + m + text.slice(start);
    const caret = start + mLen;
    return { text: next, selectionStart: caret, selectionEnd: caret };
  }

  const selected = text.slice(start, end);

  // Case A: markers are just OUTSIDE the selection in the surrounding text.
  const before = text.slice(Math.max(0, start - mLen), start);
  const after = text.slice(end, end + mLen);
  if (before === m && after === m) {
    const next =
      text.slice(0, start - mLen) + selected + text.slice(end + mLen);
    return {
      text: next,
      selectionStart: start - mLen,
      selectionEnd: end - mLen,
    };
  }

  // Case B: markers are the first/last chars INSIDE the selection.
  if (
    selected.length >= mLen * 2 &&
    selected.startsWith(m) &&
    selected.endsWith(m)
  ) {
    const inner = selected.slice(mLen, selected.length - mLen);
    const next = text.slice(0, start) + inner + text.slice(end);
    return {
      text: next,
      selectionStart: start,
      selectionEnd: start + inner.length,
    };
  }

  // Otherwise: wrap. Keep the inner text selected.
  const next = text.slice(0, start) + m + selected + m + text.slice(end);
  return {
    text: next,
    selectionStart: start + mLen,
    selectionEnd: end + mLen,
  };
}

// ── Line-prefix helpers ───────────────────────────────────

/** Expand a selection to cover the full lines it touches. Returns the
 *  line-start of the first touched line and the line-end (exclusive of the
 *  trailing "\n") of the last touched line. */
function selectedLineBounds(
  text: string,
  start: number,
  end: number,
): { lineStart: number; lineEnd: number } {
  // Walk back to the start of the first touched line.
  let lineStart = start;
  while (lineStart > 0 && text[lineStart - 1] !== "\n") lineStart -= 1;
  // Walk forward to the end of the last touched line. When the selection is
  // collapsed at a line boundary (end === lineStart of a line) we still want
  // that single line.
  let lineEnd = end;
  // If the selection ends exactly at a line start (right after a "\n") and is
  // not empty, don't pull in the following line.
  if (lineEnd > start && lineEnd > 0 && text[lineEnd - 1] === "\n") {
    lineEnd -= 1;
  }
  while (lineEnd < text.length && text[lineEnd] !== "\n") lineEnd += 1;
  return { lineStart, lineEnd };
}

/** Regex matching an existing prefix of the given kind at line start, so we
 *  can detect + strip it. Heading matches any level (#..######). Numbered
 *  matches `<n>. `. Checklist must be tested before bullet since it starts
 *  with `- `. */
function prefixMatcher(kind: LinePrefixKind): RegExp {
  switch (kind) {
    case "heading":
      return /^(#{1,6}) /;
    case "bullet":
      return /^[-*+] /;
    case "numbered":
      return /^\d+\. /;
    case "checklist":
      return /^[-*+] \[[ xX]\] /;
    case "quote":
      return /^> /;
  }
}

/** The literal prefix to add for a kind, given the 0-based line index (only
 *  numbered lists care about the index). */
function prefixFor(kind: LinePrefixKind, index: number): string {
  switch (kind) {
    case "heading":
      return "## ";
    case "bullet":
      return "- ";
    case "numbered":
      return `${index + 1}. `;
    case "checklist":
      return "- [ ] ";
    case "quote":
      return "> ";
  }
}

/**
 * Toggle a line prefix across every line the selection touches.
 *
 * If EVERY non-empty touched line already carries the prefix, we strip it from
 * all of them. Otherwise we add it to the lines that lack it (leaving lines
 * that already have a same-kind prefix as-is, except numbered lists which are
 * fully renumbered 1..n). The returned selection spans the rewritten block.
 */
export function toggleLinePrefix(
  state: EditorState,
  kind: LinePrefixKind,
): TransformResult {
  const { text, selectionStart: start, selectionEnd: end } = state;
  const { lineStart, lineEnd } = selectedLineBounds(text, start, end);
  const block = text.slice(lineStart, lineEnd);
  const lines = block.split("\n");
  const matcher = prefixMatcher(kind);

  // A line "counts" toward the all-prefixed check when it's non-empty.
  const meaningful = lines.filter((l) => l.trim().length > 0);
  const allPrefixed =
    meaningful.length > 0 && meaningful.every((l) => matcher.test(l));

  let out: string[];
  if (allPrefixed) {
    // Strip the prefix from every line that has it.
    out = lines.map((l) => l.replace(matcher, ""));
  } else {
    // Add the prefix. Numbered lists renumber across the whole block; other
    // kinds first strip any existing same-family prefix so we don't stack
    // (e.g. turning a bullet line into a checklist replaces "- " cleanly).
    let counter = 0;
    out = lines.map((l) => {
      // Skip pure-empty lines — prefixing a blank line is noise.
      if (l.trim().length === 0) return l;
      const stripped = stripAnyListPrefix(l, kind);
      const idx = counter;
      counter += 1;
      return prefixFor(kind, idx) + stripped;
    });
  }

  const nextBlock = out.join("\n");
  const next = text.slice(0, lineStart) + nextBlock + text.slice(lineEnd);
  return {
    text: next,
    selectionStart: lineStart,
    selectionEnd: lineStart + nextBlock.length,
  };
}

/** When adding a prefix, remove a pre-existing prefix of a compatible family
 *  so kinds cleanly replace one another instead of stacking. Heading and
 *  quote are independent markers, so they only strip their own kind; the list
 *  kinds (bullet / numbered / checklist) strip each other. */
function stripAnyListPrefix(line: string, kind: LinePrefixKind): string {
  if (kind === "heading") return line.replace(/^(#{1,6}) /, "");
  if (kind === "quote") return line.replace(/^> /, "");
  // List family: strip checklist first (superset of bullet), then numbered,
  // then bullet.
  return line
    .replace(/^[-*+] \[[ xX]\] /, "")
    .replace(/^\d+\. /, "")
    .replace(/^[-*+] /, "");
}

// ── Fenced code block ─────────────────────────────────────

/**
 * Wrap the selection (expanded to full lines) in triple-backtick fences on
 * their own lines. With an empty selection, inserts an empty fenced block and
 * places the caret on the blank middle line.
 */
export function toggleCodeBlock(state: EditorState): TransformResult {
  const { text, selectionStart: start, selectionEnd: end } = state;

  if (start === end) {
    // Empty: build a ```\n\n``` block; caret on the middle line.
    const needsLeadingBreak = start > 0 && text[start - 1] !== "\n";
    const lead = needsLeadingBreak ? "\n" : "";
    const insertion = `${lead}\`\`\`\n\n\`\`\`\n`;
    const next = text.slice(0, start) + insertion + text.slice(start);
    const caret = start + lead.length + 4; // after "```\n"
    return { text: next, selectionStart: caret, selectionEnd: caret };
  }

  const { lineStart, lineEnd } = selectedLineBounds(text, start, end);
  const block = text.slice(lineStart, lineEnd);
  const fenced = "```\n" + block + "\n```";
  const next = text.slice(0, lineStart) + fenced + text.slice(lineEnd);
  return {
    text: next,
    selectionStart: lineStart,
    selectionEnd: lineStart + fenced.length,
  };
}

// ── Link ──────────────────────────────────────────────────

/** The placeholder used for a link's URL when the user hasn't got one yet. */
export const LINK_URL_PLACEHOLDER = "url";
/** The placeholder used for a link's text when the selection is empty. */
export const LINK_TEXT_PLACEHOLDER = "text";

/**
 * Insert a markdown link.
 *
 * - With a selection → `[selection](url)` and select the `url` placeholder so
 *   the user can immediately paste/type the destination.
 * - With no selection → `[text](url)` and select the `text` placeholder.
 */
export function insertLink(state: EditorState): TransformResult {
  const { text, selectionStart: start, selectionEnd: end } = state;

  if (start === end) {
    const insertion = `[${LINK_TEXT_PLACEHOLDER}](${LINK_URL_PLACEHOLDER})`;
    const next = text.slice(0, start) + insertion + text.slice(start);
    const selStart = start + 1; // after "["
    const selEnd = selStart + LINK_TEXT_PLACEHOLDER.length;
    return { text: next, selectionStart: selStart, selectionEnd: selEnd };
  }

  const selected = text.slice(start, end);
  const insertion = `[${selected}](${LINK_URL_PLACEHOLDER})`;
  const next = text.slice(0, start) + insertion + text.slice(end);
  // Select the url placeholder: it sits after "[selected](".
  const selStart = start + 1 + selected.length + 2; // "[" + selected + "]("
  const selEnd = selStart + LINK_URL_PLACEHOLDER.length;
  return { text: next, selectionStart: selStart, selectionEnd: selEnd };
}
