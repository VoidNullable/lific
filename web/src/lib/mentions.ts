// LIF-263: @mention parsing + chip rendering shared between the comment
// composer (autocomplete) and the Markdown renderer (chip display).
//
// A mention is `@` at a word boundary followed by a run of username chars
// (`[A-Za-z0-9_-]`). This mirrors the server-side extractor in
// `db::queries::comments::extract_mention_usernames` so the client and
// server agree on what a token is — the server is authoritative for which
// tokens *resolve* to a real user; the client only needs the same token
// shape to render chips and drive autocomplete.

/** A user who can be mentioned. Mirrors the API `MentionCandidate`. */
export interface MentionUser {
  user_id: number;
  username: string;
  display_name: string;
}

// The `@` must be preceded by nothing (start) or a non-username,
// non-`@` character. `(^|[^\w@-])` captures that boundary so `foo@bar`
// (email) and `a@b` (mid-word) never match. Username chars are
// `[A-Za-z0-9_-]`.
export const MENTION_RE = /(^|[^\w@-])@([A-Za-z0-9_-]+)/g;

/**
 * Extract the distinct `@username` tokens from a body, lowercased and
 * de-duplicated. Order is first-seen. Pure text — no DOM.
 */
export function extractMentions(body: string): string[] {
  const out: string[] = [];
  const seen = new Set<string>();
  for (const m of body.matchAll(MENTION_RE)) {
    const key = m[2].toLowerCase();
    if (!seen.has(key)) {
      seen.add(key);
      out.push(key);
    }
  }
  return out;
}

/** Escape a string for safe insertion into HTML text/attributes. */
function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/**
 * Rewrite `@username` tokens in an HTML string into mention chips.
 *
 * `known` maps lowercased username → display name; only tokens present in
 * the map become chips (matching the server's "only visible members
 * resolve" rule). Unknown tokens stay literal text. The chip shows the
 * display name and carries the raw username in a title + data attribute.
 *
 * This is applied to already-rendered, tag-aware HTML the same way
 * `linkIdentifiers` is: callers skip runs inside <a>/<code>/<pre>.
 */
export function linkMentionsInText(
  text: string,
  known: Map<string, string>,
): string {
  return text.replace(MENTION_RE, (full, pre: string, name: string) => {
    const display = known.get(name.toLowerCase());
    if (display === undefined) return full; // unresolved — leave literal
    const safeName = escapeHtml(name);
    const safeDisplay = escapeHtml(display);
    return `${pre}<span class="mention-chip" data-mention="${safeName}" title="@${safeName}">@${safeDisplay}</span>`;
  });
}
