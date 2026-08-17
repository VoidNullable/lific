// LIF-418: the one code path that turns a textarea into an attachment-enabled
// composer.
//
// Three surfaces need identical behaviour - the issue/page body editor
// (EditableMarkdown), the comment boxes (Comments, new and edit) and the
// new-issue form (IssueNew). Before this they each hand-wired their own paste
// handler, file input and controller, which is exactly how IssueNew ended up
// with no attachment support at all.
//
// A composer hands over three accessors to its textarea (the element, its
// current text, and how to write text plus caret back) and gets the whole
// behaviour set: drag, paste, the Attach button, the pending strip, and the
// big-paste offer. Layout stays with the caller, matching the layout-prop
// composition the rest of the codebase uses; `AttachComposer.svelte` renders
// the shared chrome around whatever markup the caller passes.

import type { AttachmentEntity } from "../api";
import { createUploadController, type UploadController } from "./uploads.svelte";
import { filesFromClipboard, insertSnippetAt } from "./compose";
import { describePaste, isBigPaste, pasteFileFrom } from "./bigPaste";

export interface ComposerAttachmentsOptions {
  /** The live textarea, or null before mount / while the editor is closed. */
  el: () => HTMLTextAreaElement | null;
  /** The composer's current text. */
  text: () => string;
  /** Write new text back, and where the caret should land. */
  apply: (text: string, caret: number) => void;
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
  readonly uploads: UploadController;
  /** Non-null while a big paste is waiting on Enter (attach) or Esc (inline). */
  readonly pendingPaste: PendingPaste | null;
  enqueue: (files: File[]) => void;
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
  let fileInput: HTMLInputElement | null = null;

  /** Splice `insert` in over the current selection, verbatim (no markdown
   *  spacing). Used by the inline branch of the big-paste offer. */
  function spliceAtCaret(insert: string) {
    const el = opts.el();
    const current = opts.text();
    const start = el?.selectionStart ?? current.length;
    const end = el?.selectionEnd ?? start;
    const next = current.slice(0, start) + insert + current.slice(end);
    opts.apply(next, start + insert.length);
  }

  function insertMarkdown(snippet: string) {
    const el = opts.el();
    const current = opts.text();
    const { text, caret } = insertSnippetAt(
      current,
      el?.selectionStart ?? current.length,
      el?.selectionEnd ?? current.length,
      snippet,
    );
    opts.apply(text, caret);
  }

  const uploads = createUploadController({
    link: opts.link,
    onInsert: insertMarkdown,
  });

  function enqueue(files: File[]) {
    if (files.length > 0) uploads.enqueue(files);
  }

  function handlePaste(e: ClipboardEvent) {
    const files = filesFromClipboard(e);
    if (files.length > 0) {
      e.preventDefault();
      uploads.enqueue(files);
      return;
    }
    const text = e.clipboardData?.getData("text/plain") ?? "";
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
    uploads.enqueue([pasteFileFrom(held.text)]);
    opts.el()?.focus();
  }

  function pasteInline() {
    const held = pendingPaste;
    if (!held) return;
    pendingPaste = null;
    spliceAtCaret(held.text);
  }

  function handleKeydown(e: KeyboardEvent): boolean {
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
      uploads.enqueue(Array.from(input.files));
      input.value = "";
    }
  }

  function destroy() {
    pendingPaste = null;
    uploads.destroy();
  }

  return {
    get uploads() {
      return uploads;
    },
    get pendingPaste() {
      return pendingPaste;
    },
    enqueue,
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
