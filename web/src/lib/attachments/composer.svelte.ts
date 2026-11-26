// LIF-418: the one code path that turns a textarea into an attachment-enabled
// composer.
//
// Three surfaces need identical behaviour - the issue/page body editor
// (EditableMarkdown), the comment boxes (Comments, new and edit) and the
// new-issue form (IssueNew). Before this they each hand-wired their own paste
// handler, file input and controller, which is exactly how IssueNew ended up
// with no attachment support at all.
//
// A composer hands over two accessors to its textarea (the element, and a
// read/write pair for its draft) and gets the whole behaviour set: drag,
// paste, capture, the Attach button, the pending strip, the big-paste offer
// and the alt-text prompt. Layout stays with the caller, matching the
// layout-prop composition the rest of the codebase uses; `AttachComposer.svelte`
// renders the shared chrome around whatever markup the caller passes.

import type { AttachmentEntity } from "../api";
import { importImageAttachment } from "../api";
import {
  createUploadController,
  type ComposerText,
  type UploadController,
  type UploadSource,
} from "./uploads.svelte";
import { filesFromClipboard, insertSnippetAt, markdownFor } from "./compose";
import { describePaste, isBigPaste, pasteFileFrom } from "./bigPaste";
import { imageUrlFromPaste, replacePastedUrl } from "./imageUrl";

export type { ComposerText, UploadSource };

/** Anything with a caret: a textarea, or the body editor's adapter. */
export interface CaretTarget {
  selectionStart: number | null;
  selectionEnd: number | null;
  focus(): void;
}

export interface ComposerAttachmentsOptions {
  /** The live textarea (or editor), or null before mount. */
  el: () => CaretTarget | null;
  /** Read and write the composer's draft. The same object is handed to the
   *  upload controller, so the alt-text prompt rewrites the very reference
   *  this module inserted. */
  text: ComposerText;
  /** Entity to link finished uploads to, when it already exists. */
  link?: () => { entity_type: AttachmentEntity; entity_id: number } | null | undefined;
}

export interface PendingPaste {
  /** The intercepted clipboard text, held until the user answers. */
  text: string;
  /** "412 lines" / "8,204 characters", for the offer copy. */
  summary: string;
}

export interface ComposerAttachments {
  readonly imagePaste: { url: string; busy: boolean; error: string | null } | null;
  importImage: () => void;
  keepImageUrl: () => void;
  readonly uploads: UploadController;
  /** Non-null while a big paste is waiting on Enter (attach) or Esc (inline). */
  readonly pendingPaste: PendingPaste | null;
  /** Queue files for upload. `source` decides whether the annotate prompt is
   *  offered first, so drops, pastes and camera shots all say where they came
   *  from. */
  enqueue: (files: File[], source?: UploadSource) => void;
  /** Insert a snippet at the textarea's caret with the same spacing rules as
   *  a finished upload. */
  insert: (snippet: string) => void;
  /** `onpaste` handler. Consumes file pastes and big text pastes. */
  handlePaste: (e: ClipboardEvent) => void;
  /** `onkeydown` handler. Returns true when it consumed the key, so the host
   *  can bail before its own Enter/Escape handling runs. */
  handleKeydown: (e: KeyboardEvent) => boolean;
  /** Resolve a pending paste as a `text/plain` attachment. */
  attachPaste: () => void;
  /** Resolve a pending paste by inserting it at the caret after all. */
  pasteInline: () => void;
  /** Wire the hidden `<input type="file">` AttachComposer renders. */
  bindFileInput: (el: HTMLInputElement | null) => void;
  /** Open the file picker (the Attach button). */
  openFilePicker: () => void;
  /** `onchange` handler for the hidden file input. */
  handleFilePicked: (e: Event) => void;
  /** Abort in-flight uploads and drop all state. Call from onDestroy. */
  destroy: () => void;
}

export function createComposerAttachments(
  opts: ComposerAttachmentsOptions,
): ComposerAttachments {
  let pendingPaste = $state<PendingPaste | null>(null);
  let imagePaste = $state<{ url: string; offset: number; busy: boolean; error: string | null } | null>(null);
  let imageRequest: AbortController | null = null;
  let fileInput: HTMLInputElement | null = null;

  /** Splice `insert` in over the current selection, verbatim (no markdown
   *  spacing). Used by the inline branch of the big-paste offer. */
  function spliceAtCaret(insert: string) {
    const el = opts.el();
    const current = opts.text.read();
    const start = el?.selectionStart ?? current.length;
    const end = el?.selectionEnd ?? start;
    const next = current.slice(0, start) + insert + current.slice(end);
    opts.text.write(next, start + insert.length);
  }

  function insertMarkdown(snippet: string) {
    const el = opts.el();
    const current = opts.text.read();
    const { text, caret } = insertSnippetAt(
      current,
      el?.selectionStart ?? current.length,
      el?.selectionEnd ?? current.length,
      snippet,
    );
    opts.text.write(text, caret);
  }

  const uploads = createUploadController({
    externalPending: () => imagePaste?.busy ?? false,
    link: opts.link,
    onInsert: insertMarkdown,
    text: opts.text,
  });

  function enqueue(files: File[], source: UploadSource = "picker") {
    if (files.length > 0) uploads.enqueue(files, source);
  }

  function handlePaste(e: ClipboardEvent) {
    const files = filesFromClipboard(e);
    if (files.length > 0) {
      e.preventDefault();
      uploads.enqueue(files, "paste");
      return;
    }
    const text = e.clipboardData?.getData("text/plain") ?? "";
    const imageUrl = imageUrlFromPaste(text);
    if (imageUrl && !imagePaste?.busy) {
      e.preventDefault();
      const offset = opts.el()?.selectionStart ?? opts.text.read().length;
      spliceAtCaret(text);
      imagePaste = { url: imageUrl, offset: offset + text.indexOf(imageUrl), busy: false, error: null };
      return;
    }
    if (!isBigPaste(text)) return;
    // Hold the text rather than dropping it in. Nothing is lost either way:
    // both answers put the content somewhere, and the textarea keeps focus so
    // typing carries on while the offer sits there.
    e.preventDefault();
    pendingPaste = { text, summary: describePaste(text) };
  }

  function attachPaste() {
    const held = pendingPaste;
    if (!held) return;
    pendingPaste = null;
    // Plain text, so the annotate prompt passes it straight through; the
    // source is still honest about where the bytes came from.
    uploads.enqueue([pasteFileFrom(held.text)], "paste");
    opts.el()?.focus();
  }

  function pasteInline() {
    const held = pendingPaste;
    if (!held) return;
    pendingPaste = null;
    spliceAtCaret(held.text);
  }

  function handleKeydown(e: KeyboardEvent): boolean {
    if (imagePaste && !imagePaste.busy && (e.key === "Enter" || e.key === "Escape")) {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Enter") void importImage();
      else imagePaste = null;
      return true;
    }
    if (!pendingPaste) return false;
    if (e.key === "Enter") {
      e.preventDefault();
      attachPaste();
      return true;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      // Escape also closes the body editor and the comment editor, so stop it
      // here: answering the offer should not throw away the draft too.
      e.stopPropagation();
      pasteInline();
      return true;
    }
    return false;
  }

  function bindFileInput(el: HTMLInputElement | null) {
    fileInput = el;
  }

  function openFilePicker() {
    fileInput?.click();
  }

  function handleFilePicked(e: Event) {
    const input = e.target as HTMLInputElement;
    if (input.files && input.files.length > 0) {
      uploads.enqueue(Array.from(input.files), "picker");
      input.value = "";
    }
  }

  async function importImage() {
    const held = imagePaste;
    if (!held || held.busy) return;
    held.busy = true;
    held.error = null;
    const request = new AbortController();
    imageRequest = request;
    const result = await importImageAttachment(held.url, opts.link?.() ?? null, request.signal);
    if (request.signal.aborted) return;
    imageRequest = null;
    if (!result.ok) {
      held.busy = false;
      held.error = result.error;
      return;
    }
    const snippet = markdownFor(result.data, result.data.filename);
    const replacement = replacePastedUrl(opts.text.read(), held.url, held.offset, snippet);
    if (replacement) opts.text.write(replacement.text, replacement.caret);
    imagePaste = null;
  }

  function destroy() {
    imageRequest?.abort();
    imagePaste = null;
    pendingPaste = null;
    uploads.destroy();
  }

  return {
    get imagePaste() { return imagePaste; },
    importImage: () => { void importImage(); },
    keepImageUrl: () => { imagePaste = null; opts.el()?.focus(); },
    get uploads() {
      return uploads;
    },
    get pendingPaste() {
      return pendingPaste;
    },
    enqueue,
    insert: insertMarkdown,
    handlePaste,
    handleKeydown,
    attachPaste,
    pasteInline,
    bindFileInput,
    openFilePicker,
    handleFilePicked,
    destroy,
  };
}
