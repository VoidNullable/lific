// LIF-418: big-paste interception.
//
// Dropping a 400-line log into a comment box buries the actual comment and
// makes the thread unreadable. The composer notices a paste that big and
// offers, without blocking a single keystroke, to send it as a text attachment
// instead. Enter attaches, Escape pastes inline, and nothing is lost either
// way.
//
// Everything here is pure so the thresholds and the generated filename can be
// unit tested; the toast lives in AttachComposer.svelte.

/** Line count above which we offer to attach instead of paste. */
export const BIG_PASTE_LINE_LIMIT = 60;

/** Character count above which we offer to attach instead of paste. */
export const BIG_PASTE_CHAR_LIMIT = 6000;

/** Lines in a pasted blob. An empty string is zero lines, not one. */
export function countLines(text: string): number {
  if (text.length === 0) return 0;
  return text.split("\n").length;
}

/** True when a paste is big enough to be worth offering as a file. Both
 *  thresholds are strict: exactly 60 lines or exactly 6000 characters still
 *  pastes inline without a prompt. */
export function isBigPaste(text: string): boolean {
  if (text.length > BIG_PASTE_CHAR_LIMIT) return true;
  return countLines(text) > BIG_PASTE_LINE_LIMIT;
}

function pad(n: number, width = 2): string {
  return String(n).padStart(width, "0");
}

/** `paste-YYYYMMDD-HHMM.txt`, in the user's local time so the name matches the
 *  clock they were looking at when they pasted. */
export function pasteFilename(at: Date): string {
  const stamp =
    `${at.getFullYear()}${pad(at.getMonth() + 1)}${pad(at.getDate())}` +
    `-${pad(at.getHours())}${pad(at.getMinutes())}`;
  return `paste-${stamp}.txt`;
}

/** Wrap pasted text as an uploadable `text/plain` file. */
export function pasteFileFrom(text: string, at: Date = new Date()): File {
  return new File([text], pasteFilename(at), {
    type: "text/plain",
    lastModified: at.getTime(),
  });
}

/** Short human summary for the offer toast ("412 lines", "8,204 characters"). */
export function describePaste(text: string): string {
  const lines = countLines(text);
  if (lines > BIG_PASTE_LINE_LIMIT) return `${lines.toLocaleString()} lines`;
  return `${text.length.toLocaleString()} characters`;
}
