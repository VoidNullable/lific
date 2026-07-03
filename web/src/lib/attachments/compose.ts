// LIF-262: shared attachment-upload composer logic for every markdown input
// (the issue/page description editor and every comment box). Keeps the upload
// → toast → markdown-insertion flow in one place so drag-drop, paste, and the
// "+ Attach" button all behave identically.

import { uploadAttachment, type AttachmentEntity, type UploadResponse } from "../api";
import { toast } from "../toast/toast.svelte";

/** The markdown snippet to insert for a finished upload: an image embed for
 *  images, a link chip for everything else. */
export function markdownFor(up: UploadResponse): string {
  const isImage = up.mime.startsWith("image/");
  if (isImage) {
    return `![${up.filename}](${up.url})`;
  }
  return `[${up.filename}](${up.url})`;
}

/** Insert `snippet` into `textarea` at the caret, returning the new full text
 *  and the caret position after the insertion. Surrounds the snippet with
 *  spacing so it never fuses onto adjacent words. */
export function insertAtCaret(
  textarea: HTMLTextAreaElement,
  current: string,
  snippet: string,
): { text: string; caret: number } {
  const start = textarea.selectionStart ?? current.length;
  const end = textarea.selectionEnd ?? current.length;
  const before = current.slice(0, start);
  const after = current.slice(end);
  // Add a leading newline if we're mid-line so an image embed lands on its own
  // block; keep inline otherwise.
  const needsLeadingBreak = before.length > 0 && !before.endsWith("\n");
  const prefix = needsLeadingBreak ? "\n" : "";
  const insertion = `${prefix}${snippet}\n`;
  const text = before + insertion + after;
  const caret = before.length + insertion.length;
  return { text, caret };
}

export interface UploadHooks {
  /** The entity this upload should link to immediately, when known (detail
   *  views pass this; the new-issue form has no id yet and omits it, relying on
   *  re-scan-on-save instead). */
  link?: { entity_type: AttachmentEntity; entity_id: number };
  /** Called with the markdown snippet to insert on success. */
  onInsert: (snippet: string) => void;
  /** Toggle a busy indicator while uploads are in flight. */
  onBusy?: (busy: boolean) => void;
}

/** Filter a candidate file list down to plausible uploads and run them
 *  sequentially, inserting each on success and toasting the exact reason on
 *  failure. Sequential (not parallel) so multiple pasted images insert in a
 *  predictable order and don't race the caret. */
export async function uploadFiles(files: File[], hooks: UploadHooks): Promise<void> {
  if (files.length === 0) return;
  hooks.onBusy?.(true);
  try {
    for (const file of files) {
      const result = await uploadAttachment(file, hooks.link);
      if (result.ok) {
        hooks.onInsert(markdownFor(result.data));
      } else {
        toast(`Couldn't upload ${file.name}: ${result.error}`, { kind: "error" });
      }
    }
  } finally {
    hooks.onBusy?.(false);
  }
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
