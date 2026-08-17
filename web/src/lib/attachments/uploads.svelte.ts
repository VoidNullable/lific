// LIF-268: shared pending-upload state for every markdown composer.
//
// LIF-262 landed the mechanical upload path (drag / paste / button →
// uploadAttachment → insert markdown → toast on failure) in `compose.ts`.
// That file stays the source of truth for the *pure* helpers (markdownFor,
// insertAtCaret, filesFromClipboard/Drop). This module adds the *stateful*
// layer both composers share so they never fork behaviour: a reactive list of
// in-flight uploads, each rendered as a chip in `PendingUploads.svelte`.
//
// A single upload moves through: uploading → (success → inserted, chip drops)
// or (error → chip turns red with the server reason + retry/dismiss). Image
// files carry a `previewUrl` (object URL) revoked the moment the chip leaves,
// so a long-lived composer never leaks blob URLs.
//
// The controller is instantiated once per composer via `createUploadController`
// and exposes an imperative surface (`enqueue`, `retry`, `dismiss`) plus the
// reactive `items` array the strip renders. Because it uses runes it lives in a
// `.svelte.ts` module.

import { uploadAttachment, type AttachmentEntity } from "../api";
import { markdownFor } from "./compose";
import { maybeAnnotate, type UploadSource } from "./annotateFlow";
import { applyAltText } from "./altText";

export type { UploadSource };

// LIF-418 adds a third resting state. `alt` is a *settled, successful* upload
// whose chip lingers only to offer a one-line description for the image's alt
// text. It is not busy, nothing is in flight, and ignoring it costs nothing.
export type UploadStatus = "uploading" | "error" | "alt";

export interface PendingUpload {
  /** Stable client id — chips key on this so retry/dismiss target one row. */
  readonly id: number;
  readonly file: File;
  readonly filename: string;
  readonly size: number;
  readonly isImage: boolean;
  /** Object URL for an image preview thumbnail; null for non-images. Revoked
   *  on removal. */
  readonly previewUrl: string | null;
  status: UploadStatus;
  /** Server-supplied reason, present only in the error state. */
  error: string | null;
  /** LIF-418: the markdown that was inserted on success, kept so the alt-text
   *  prompt can find its own reference again in the composer's draft. */
  snippet: string | null;
}

export interface UploadControllerOptions {
  /** Entity to link finished uploads to, when the parent id is already known
   *  (detail views). Omitted for not-yet-created entities (new-issue form,
   *  new comment) which rely on server re-scan of the saved body. A getter so
   *  the composer can pass a value that becomes known after mount. */
  link?: () => { entity_type: AttachmentEntity; entity_id: number } | null | undefined;
  /** Insert the finished markdown reference at the caret. */
  onInsert: (snippet: string) => void;
  /**
   * LIF-418: read/write access to the composer's draft.
   *
   * Supplying it turns on the alt-text prompt: after an image lands, its chip
   * stays put with a one-line input, and what gets typed is spliced into the
   * alt slot of the reference that was just inserted. Omit it and image
   * uploads behave exactly as before (chip disappears on success).
   */
  text?: {
    read: () => string;
    write: (next: string) => void;
  };
}

export interface UploadController {
  /** Reactive list of in-flight / failed uploads for the strip to render. */
  readonly items: PendingUpload[];
  /** True while at least one upload is in flight (drives busy affordances). */
  readonly busy: boolean;
  /** Queue files for upload; images get a preview thumbnail immediately.
   *  `source` decides whether the annotate prompt is offered first. */
  enqueue: (files: File[], source?: UploadSource) => void;
  /** Re-attempt a failed upload in place. */
  retry: (id: number) => void;
  /** Write `alt` into the alt slot of this upload's inserted reference, then
   *  drop the chip. Empty input is treated as a skip. */
  applyAlt: (id: number, alt: string) => void;
  /** Drop a chip (typically a failed one) and revoke its preview URL. */
  dismiss: (id: number) => void;
  /** Revoke every outstanding object URL. Call on composer teardown. */
  destroy: () => void;
}

export function createUploadController(opts: UploadControllerOptions): UploadController {
  let seq = 0;
  const items = $state<PendingUpload[]>([]);

  function indexOf(id: number): number {
    return items.findIndex((it) => it.id === id);
  }

  function revoke(item: PendingUpload) {
    if (item.previewUrl) URL.revokeObjectURL(item.previewUrl);
  }

  async function run(item: PendingUpload) {
    const link = opts.link?.() ?? undefined;
    const result = await uploadAttachment(item.file, link ?? undefined);
    const i = indexOf(item.id);
    if (i === -1) return; // dismissed mid-flight
    if (result.ok) {
      const snippet = markdownFor(result.data);
      opts.onInsert(snippet);
      if (result.data.mime.startsWith("image/") && opts.text) {
        // Hold the chip open for a description instead of vanishing. The
        // upload is already done and linked; this is purely an offer.
        items[i].status = "alt";
        items[i].snippet = snippet;
      } else {
        revoke(items[i]);
        items.splice(i, 1);
      }
    } else {
      items[i].status = "error";
      items[i].error = result.error;
    }
  }

  function enqueue(files: File[], source: UploadSource = "picker") {
    void begin(files, source);
  }

  // Split out because annotation is async: a pasted screenshot may sit in the
  // prompt (or the editor) for a while before anything is uploaded. Nothing is
  // in flight during that window, so no chip is shown for it either.
  async function begin(files: File[], source: UploadSource) {
    const prepared = await maybeAnnotate(files, source);
    for (const file of prepared) {
      const isImage = file.type.startsWith("image/");
      const item: PendingUpload = {
        id: ++seq,
        file,
        filename: file.name,
        size: file.size,
        isImage,
        previewUrl: isImage ? URL.createObjectURL(file) : null,
        status: "uploading",
        error: null,
        snippet: null,
      };
      items.push(item);
      void run(item);
    }
  }

  function applyAlt(id: number, alt: string) {
    const i = indexOf(id);
    if (i === -1) return;
    const item = items[i];
    const source = opts.text;
    if (source && item.snippet) {
      const current = source.read();
      // Locate the reference we inserted; if the draft moved on and it is gone,
      // fall back to a whole-document search from zero rather than rewriting
      // whatever happens to sit at the old offset.
      const offset = current.indexOf(item.snippet);
      const next = applyAltText(current, offset === -1 ? 0 : offset, alt);
      if (next !== current) source.write(next);
    }
    dismiss(id);
  }

  function retry(id: number) {
    const i = indexOf(id);
    if (i === -1) return;
    items[i].status = "uploading";
    items[i].error = null;
    void run(items[i]);
  }

  function dismiss(id: number) {
    const i = indexOf(id);
    if (i === -1) return;
    revoke(items[i]);
    items.splice(i, 1);
  }

  function destroy() {
    for (const it of items) revoke(it);
    items.splice(0, items.length);
  }

  return {
    get items() {
      return items;
    },
    get busy() {
      return items.some((it) => it.status === "uploading");
    },
    enqueue,
    retry,
    applyAlt,
    dismiss,
    destroy,
  };
}
