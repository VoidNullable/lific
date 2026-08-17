// LIF-418: alt text for freshly inserted image references.
//
// An upload inserts `![name.png](/api/attachments/12)` at the caret. The
// filename is a placeholder, not a description: screen readers announce
// "Screenshot 2026-08-17 at 14.02.11.png" and nobody is better off. After a
// successful image upload the pending chip offers a one-line input, and what
// gets typed there is spliced into the alt slot of that reference by the pure
// function below.
//
// Pure on purpose: the composer owns the draft string, this file owns the
// splice, and neither needs the other's state to be tested.

/**
 * Make a raw input string safe to sit inside `![...]`.
 *
 * Newlines would break the reference across lines and unbalanced brackets
 * would terminate the alt early, turning the rest of the description into
 * literal text next to a broken image. Both are folded rather than rejected so
 * a paste of multi-line text still produces something usable.
 */
export function sanitizeAltText(raw: string): string {
  return raw
    .replace(/[\r\n\t]+/g, " ")
    .replace(/[[\]]/g, "")
    .replace(/\s{2,}/g, " ")
    .trim();
}

/** Span of the alt text inside the first image reference at or after `offset`,
 *  as `[start, end)` indices into `markdown`. Null when there is none. */
export function findAltSpan(
  markdown: string,
  offset: number,
): { start: number; end: number } | null {
  const from = Math.max(0, Math.min(offset, markdown.length));
  // Search forward from the insertion point first, then fall back to the whole
  // document: the composer may have re-wrapped or the caret may have moved
  // between the upload finishing and the description being typed.
  const at = locate(markdown, from) ?? (from > 0 ? locate(markdown, 0) : null);
  return at;
}

function locate(markdown: string, from: number): { start: number; end: number } | null {
  let cursor = from;
  for (;;) {
    const bang = markdown.indexOf("![", cursor);
    if (bang === -1) return null;
    const close = markdown.indexOf("]", bang + 2);
    // A reference needs a closing bracket immediately followed by "(" to be an
    // image; anything else is prose that merely contains "![".
    if (close !== -1 && markdown[close + 1] === "(") {
      return { start: bang + 2, end: close };
    }
    cursor = bang + 2;
  }
}

/**
 * Replace the alt text of the image reference nearest `offset`.
 *
 * `offset` is where the upload inserted its snippet. An empty (or
 * whitespace-only) `alt` is a no-op so "press Enter on a blank field" reads as
 * skip, not as "erase the filename".
 */
export function applyAltText(markdown: string, offset: number, alt: string): string {
  const clean = sanitizeAltText(alt);
  if (!clean) return markdown;
  const span = findAltSpan(markdown, offset);
  if (!span) return markdown;
  return markdown.slice(0, span.start) + clean + markdown.slice(span.end);
}
