// LIF-268 / LIF-418: shared pending-upload state for every markdown composer.
//
// `compose.ts` owns the pure helpers (markdownFor, insertSnippetAt,
// filesFromClipboard/Drop). This module owns the stateful layer every composer
// shares, so drag, paste, the Attach button and the new-issue form can never
// fork behaviour: a reactive list of outstanding uploads, each rendered as a
// chip by `PendingUploads.svelte`.
//
// LIF-418 reworked the transfer itself. It used to be strictly sequential over
// fetch, with a spinner and no idea how far along anything was. Now:
//
//   * up to MAX_IN_FLIGHT transfers run at once (see queue.ts), so a six
//     screenshot drop starts immediately instead of trickling;
//   * every chip carries byte-level progress from XMLHttpRequest, because
//     fetch still cannot report upload progress;
//   * every chip can be canceled mid-flight, and every failure can be retried
//     in place;
//   * an image that is enormous, or close to the instance's byte cap, pauses
//     on the chip and offers a resize first (see downscale.ts).
//
// One upload moves through: (offer) -> queued -> uploading -> success (chip
// drops, markdown lands at the caret) or error (chip turns red with the server
// reason plus Retry / Dismiss). Image files carry a `previewUrl` object URL
// that is revoked the moment the chip leaves, so a long-lived composer never
// leaks blob URLs.
//
// Because it uses runes it lives in a `.svelte.ts` module.

import {
  uploadAttachmentWithProgress,
  type AttachmentEntity,
  type UploadHandle,
} from "../api";
import { markdownFor } from "./compose";
import { createConcurrencyQueue } from "./queue";
import {
  decideDownscale,
  downscaleImage,
  parseUploadCap,
  readImageFacts,
  DEFAULT_UPLOAD_CAP_BYTES,
  type DownscaleOffer,
} from "./downscale";

/** Transfers allowed at once per composer. Browsers cap sockets per origin
 *  anyway; three is enough to feel instant without making per-file progress
 *  meaningless. */
export const MAX_IN_FLIGHT = 3;

export type UploadStatus = "offer" | "queued" | "uploading" | "error";

export interface PendingUpload {
  /** Stable client id - chips key on this so retry/cancel target one row. */
  readonly id: number;
  /** The bytes we will actually send. Replaced in place when the user takes
   *  the resize offer. */
  file: File;
  filename: string;
  size: number;
  readonly isImage: boolean;
  /** Object URL for an image preview thumbnail; null for non-images. Revoked
   *  on removal (and when a resize replaces the file). */
  previewUrl: string | null;
  status: UploadStatus;
  /** Server-supplied reason, present only in the error state. */
  error: string | null;
  /** Bytes sent so far, and the total we expect to send. */
  loaded: number;
  total: number;
  /** Present only in the `offer` state: the resize we are proposing. */
  offer: DownscaleOffer | null;
}

export type DownscaleChoice = "resize" | "original";

// ── Session-wide facts ───────────────────────────────────────
//
// Two things outlive any single composer: the instance's byte cap (learned
// from a rejection) and the user's answer to the resize question. Both live in
// sessionStorage so they survive an in-tab navigation but never leak into the
// next day's session. Storage can throw (private mode, disabled cookies), so
// every access is guarded and falls back to module state.

const CAP_KEY = "lific_upload_cap";
const CHOICE_KEY = "lific_downscale_choice";

let capMemo: number | null = null;
let choiceMemo: DownscaleChoice | null = null;

function session(): Storage | null {
  try {
    return typeof sessionStorage === "undefined" ? null : sessionStorage;
  } catch {
    return null;
  }
}

/** The byte cap we believe this instance enforces. */
export function uploadCap(): number {
  if (capMemo != null) return capMemo;
  const raw = session()?.getItem(CAP_KEY);
  const parsed = raw ? Number(raw) : NaN;
  capMemo = Number.isFinite(parsed) && parsed > 0 ? parsed : DEFAULT_UPLOAD_CAP_BYTES;
  return capMemo;
}

/** Record a cap discovered in a rejection message. */
export function rememberUploadCap(bytes: number): void {
  if (!Number.isFinite(bytes) || bytes <= 0) return;
  capMemo = bytes;
  try {
    session()?.setItem(CAP_KEY, String(bytes));
  } catch {
    // Memo alone is fine.
  }
}

/** The resize answer the user already gave this session, if any. */
export function downscaleChoice(): DownscaleChoice | null {
  if (choiceMemo) return choiceMemo;
  const raw = session()?.getItem(CHOICE_KEY);
  if (raw === "resize" || raw === "original") {
    choiceMemo = raw;
    return raw;
  }
  return null;
}

export function rememberDownscaleChoice(choice: DownscaleChoice): void {
  choiceMemo = choice;
  try {
    session()?.setItem(CHOICE_KEY, choice);
  } catch {
    // Memo alone is fine.
  }
}

/** Test/teardown hook: forget the cap and the remembered resize answer. */
export function resetUploadSession(): void {
  capMemo = null;
  choiceMemo = null;
  try {
    session()?.removeItem(CAP_KEY);
    session()?.removeItem(CHOICE_KEY);
  } catch {
    // Nothing to clear.
  }
}

// ── Controller ───────────────────────────────────────────────

export interface UploadControllerOptions {
  /** Entity to link finished uploads to, when the parent id is already known
   *  (detail views). Omitted for not-yet-created entities (the new-issue form,
   *  a new comment) which rely on the server re-scanning the saved body. A
   *  getter so the composer can pass a value that becomes known after mount. */
  link?: () => { entity_type: AttachmentEntity; entity_id: number } | null | undefined;
  /** Insert the finished markdown reference at the caret. */
  onInsert: (snippet: string) => void;
}

export interface UploadController {
  /** Reactive list of outstanding uploads for the strip to render. */
  readonly items: PendingUpload[];
  /** True while at least one transfer is queued or in flight. */
  readonly busy: boolean;
  /** True while anything is still going to happen, including a chip waiting
   *  on the user's resize answer. Save buttons gate on this. Failed chips do
   *  NOT count: a permanent rejection must never wedge the Save button. */
  readonly pending: boolean;
  /** Queue files for upload; images get a preview thumbnail immediately. */
  enqueue: (files: File[]) => void;
  /** Re-attempt a failed upload in place. */
  retry: (id: number) => void;
  /** Abort a queued or in-flight upload and drop its chip. */
  cancel: (id: number) => void;
  /** Drop a chip (typically a failed one) and revoke its preview URL. */
  dismiss: (id: number) => void;
  /** Take the resize offer on one chip, and remember that answer. */
  acceptDownscale: (id: number) => void;
  /** Decline the resize offer on one chip, and remember that answer. */
  keepOriginal: (id: number) => void;
  /** Abort everything and revoke every outstanding object URL. Call on
   *  composer teardown so navigating away leaves nothing running. */
  destroy: () => void;
}

interface Transfer {
  canceled: boolean;
  handle: UploadHandle | null;
}

export function createUploadController(opts: UploadControllerOptions): UploadController {
  let seq = 0;
  const items = $state<PendingUpload[]>([]);
  const queue = createConcurrencyQueue(MAX_IN_FLIGHT);
  // Kept outside $state: aborting is imperative plumbing, not rendered.
  const transfers = new Map<number, Transfer>();

  function get(id: number): PendingUpload | null {
    return items.find((it) => it.id === id) ?? null;
  }

  function revokePreview(item: PendingUpload) {
    if (item.previewUrl) URL.revokeObjectURL(item.previewUrl);
  }

  function remove(id: number) {
    const i = items.findIndex((it) => it.id === id);
    if (i === -1) return;
    revokePreview(items[i]);
    items.splice(i, 1);
    transfers.delete(id);
  }

  /** Kick off the transfer for one chip. Admission to the queue is immediate;
   *  the chip sits in `queued` until a slot frees up. */
  function start(id: number) {
    const item = get(id);
    if (!item) return;
    item.status = "queued";
    item.error = null;
    item.loaded = 0;
    item.total = item.file.size;

    const transfer: Transfer = { canceled: false, handle: null };
    transfers.set(id, transfer);

    void queue.add(async () => {
      if (transfer.canceled) return;
      const queued = get(id);
      if (!queued) return;
      queued.status = "uploading";

      const handle = uploadAttachmentWithProgress(queued.file, {
        link: opts.link?.() ?? null,
        onProgress: (p) => {
          const live = get(id);
          if (!live) return;
          live.loaded = p.loaded;
          live.total = p.total || live.size;
        },
      });
      transfer.handle = handle;
      if (transfer.canceled) handle.abort();

      const outcome = await handle.result;
      transfers.delete(id);
      const settled = get(id);
      if (!settled) return;

      if (outcome.ok) {
        opts.onInsert(markdownFor(outcome.data));
        remove(id);
        return;
      }
      if (outcome.canceled) {
        remove(id);
        return;
      }
      // A rejection usually carries the instance's real cap; learn it so the
      // next oversize image gets the resize offer instead of a round trip.
      const cap = parseUploadCap(outcome.error);
      if (cap) rememberUploadCap(cap);
      settled.status = "error";
      settled.error = outcome.error;
    });
  }

  /** Swap a chip's file for a resized copy, then upload it. */
  async function resizeThenStart(id: number, offer: DownscaleOffer) {
    const item = get(id);
    if (!item) return;
    const resized = await downscaleImage(item.file, offer);
    const live = get(id);
    if (!live) return;
    if (resized !== live.file) {
      revokePreview(live);
      live.file = resized;
      live.size = resized.size;
      live.previewUrl = URL.createObjectURL(resized);
    }
    live.offer = null;
    start(id);
  }

  /** Measure an image and decide whether to ask before uploading. */
  async function prepare(id: number) {
    const item = get(id);
    if (!item) return;
    if (!item.isImage) {
      start(id);
      return;
    }
    const facts = await readImageFacts(item.file);
    if (!get(id)) return; // dismissed while decoding
    const offer = facts ? decideDownscale(facts, uploadCap()) : null;
    if (!offer) {
      start(id);
      return;
    }
    const remembered = downscaleChoice();
    if (remembered === "resize") {
      await resizeThenStart(id, offer);
      return;
    }
    if (remembered === "original") {
      start(id);
      return;
    }
    const waiting = get(id);
    if (!waiting) return;
    waiting.offer = offer;
    waiting.status = "offer";
  }

  function enqueue(files: File[]) {
    for (const file of files) {
      const isImage = file.type.startsWith("image/");
      const id = ++seq;
      items.push({
        id,
        file,
        filename: file.name,
        size: file.size,
        isImage,
        previewUrl: isImage ? URL.createObjectURL(file) : null,
        status: "queued",
        error: null,
        loaded: 0,
        total: file.size,
        offer: null,
      });
      void prepare(id);
    }
  }

  function retry(id: number) {
    if (!get(id)) return;
    start(id);
  }

  function cancel(id: number) {
    const transfer = transfers.get(id);
    if (transfer) {
      transfer.canceled = true;
      transfer.handle?.abort();
    }
    remove(id);
  }

  function acceptDownscale(id: number) {
    const item = get(id);
    if (!item || !item.offer) return;
    rememberDownscaleChoice("resize");
    const offer = item.offer;
    item.status = "queued";
    void resizeThenStart(id, offer);
  }

  function keepOriginal(id: number) {
    const item = get(id);
    if (!item) return;
    rememberDownscaleChoice("original");
    item.offer = null;
    start(id);
  }

  function destroy() {
    for (const transfer of transfers.values()) {
      transfer.canceled = true;
      transfer.handle?.abort();
    }
    transfers.clear();
    for (const it of items) revokePreview(it);
    items.splice(0, items.length);
  }

  return {
    get items() {
      return items;
    },
    get busy() {
      return items.some((it) => it.status === "queued" || it.status === "uploading");
    },
    get pending() {
      return items.some((it) => it.status !== "error");
    },
    enqueue,
    retry,
    cancel,
    dismiss: remove,
    acceptDownscale,
    keepOriginal,
    destroy,
  };
}
