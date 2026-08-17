// LIF-262 / LIF-418: pure helpers shared by every markdown composer (the
// issue/page description editor, every comment box, and the new-issue form).
//
// The stateful half - queueing, progress, retries, the downscale offer - lives
// in `uploads.svelte.ts`, and the wiring that turns a textarea into an
// attachment-enabled composer lives in `composer.svelte.ts`. This file stays
// free of runes and of the DOM wherever possible so it can be unit tested.

import type { UploadResponse } from "../api";

/** The markdown snippet to insert for a finished upload: an image embed for
 *  images, a link chip for everything else. */
export function markdownFor(up: UploadResponse): string {
  const isImage = up.mime.startsWith("image/");
  if (isImage) {
    return `![${up.filename}](${up.url})`;
  }
  return `[${up.filename}](${up.url})`;
}

export interface Insertion {
  text: string;
  caret: number;
}

/** Pure insertion math: splice `snippet` into `current` over the selection
 *  [start, end), returning the new text and where the caret lands.
 *
 *  A leading newline is added when the caret sits mid-line so an image embed
 *  starts its own block, and a trailing newline always follows so the next
 *  upload in a multi-file drop does not fuse onto this one. Out-of-range or
 *  reversed selections are clamped rather than throwing, because a detached
 *  textarea reports nonsense offsets. */
export function insertSnippetAt(
  current: string,
  selectionStart: number,
  selectionEnd: number,
  snippet: string,
): Insertion {
  const len = current.length;
  const rawStart = Number.isFinite(selectionStart) ? selectionStart : len;
  const rawEnd = Number.isFinite(selectionEnd) ? selectionEnd : rawStart;
  const start = Math.min(Math.max(0, Math.min(rawStart, rawEnd)), len);
  const end = Math.min(Math.max(start, Math.max(rawStart, rawEnd)), len);

  const before = current.slice(0, start);
  const after = current.slice(end);
  const needsLeadingBreak = before.length > 0 && !before.endsWith("\n");
  const insertion = `${needsLeadingBreak ? "\n" : ""}${snippet}\n`;
  return { text: before + insertion + after, caret: before.length + insertion.length };
}

/** Insert `snippet` into `textarea` at the caret. Thin wrapper over
 *  `insertSnippetAt` for call sites that already hold the element. */
export function insertAtCaret(
  textarea: HTMLTextAreaElement,
  current: string,
  snippet: string,
): Insertion {
  return insertSnippetAt(
    current,
    textarea.selectionStart ?? current.length,
    textarea.selectionEnd ?? current.length,
    snippet,
  );
}

/** Pull File objects out of a clipboard paste (images copied from the OS
 *  screenshot tool arrive as `items`, not `files`). Returns [] when the paste
 *  is plain text, so the caller can let the default paste proceed. */
export function filesFromClipboard(e: ClipboardEvent): File[] {
  const dt = e.clipboardData;
  if (!dt) return [];
  const out: File[] = [];
  for (const item of Array.from(dt.items)) {
    if (item.kind === "file") {
      const f = item.getAsFile();
      if (f) out.push(f);
    }
  }
  return out;
}

/** Pull File objects out of a drag-and-drop event. */
export function filesFromDrop(e: DragEvent): File[] {
  const dt = e.dataTransfer;
  if (!dt) return [];
  return Array.from(dt.files);
}
