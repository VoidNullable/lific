// The Markdown editor, assembled. This module and everything it imports load
// on demand (see ../loadEditor.ts), so people who only read never download
// CodeMirror.

import {
  Annotation,
  EditorSelection,
  EditorState,
  Prec,
  Transaction,
  type ChangeSet,
} from "@codemirror/state";
import {
  EditorView,
  drawSelection,
  dropCursor,
  keymap,
  placeholder as placeholderExt,
  type ViewUpdate,
} from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, redo } from "@codemirror/commands";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { completionKeymap } from "@codemirror/autocomplete";
import { insertSnippetAt } from "../../attachments/compose";
import { diffText } from "../textDiff";
import { livePreview, toggleTaskAt, type PreviewServices } from "./livePreview";
import { editorTheme } from "./theme";
import { editorCompletions } from "./completions";
import {
  activeFormats,
  formatCommands,
  indentListItem,
  outdentListItem,
  pasteUrlAsLink,
  type FormatAction,
} from "./commands";

export { BodySync, type BodySaveResult, type SyncStatus } from "../bodySync";
export { activeFormats, formatCommands, type FormatAction };
export { toggleTaskAt } from "./livePreview";
export { linkAt, type LinkAt } from "./links";
export type { PreviewServices } from "./livePreview";

/** Marks transactions that carry someone else's edits into the editor. */
const remoteEdit = Annotation.define<boolean>();

export interface EditorHooks {
  /** The person edited the text. */
  onChange?: (changes: ChangeSet, update: ViewUpdate) => void;
  /** Selection or focus moved (drives the floating toolbar). */
  onSelection?: (view: EditorView) => void;
  onFocus?: (focused: boolean) => void;
  /** Ctrl/⌘+S. */
  onSaveNow?: () => void;
  /** Escape with nothing else to close. The editor has already blurred. */
  onEscape?: () => void;
  /** Return true when the paste was consumed (files, a big paste offer). */
  onPaste?: (e: ClipboardEvent) => boolean;
  /** Return true when the keydown was consumed before the editor saw it. */
  onKeydown?: (e: KeyboardEvent) => boolean;
  /** Files dropped into the text; the caret is already at the drop point. */
  onDropFiles?: (files: File[]) => void;
  /** Open the file picker (the block menu's "Image or file"). */
  onAttach?: () => void;
  /** A link or reference was opened with ⌘/Ctrl+click. */
  onOpenLink?: (href: string, kind: "url" | "ref") => void;
}

export interface EditorOptions {
  doc: string;
  placeholder?: string;
  services: PreviewServices;
  hooks: EditorHooks;
  /** Accessible name for the text area. */
  label?: string;
}

export interface MarkdownEditorHandle {
  view: EditorView;
  /** Current text. */
  text(): string;
  /** Apply edits from elsewhere. Not reported through `onChange`. */
  applyRemote(changes: ChangeSet, undoable: boolean): void;
  /** Replace the whole text with `next` as the smallest edit, placing the
   *  caret at `caret` when given (the attachment composer's write path). */
  setText(next: string, caret?: number | null): void;
  /** Insert a Markdown snippet (an upload, a panel insert) at the caret
   *  with the composer's spacing rules. */
  insertSnippet(snippet: string): void;
  /** What the attachment composer reads as the caret. */
  caret(): { selectionStart: number; selectionEnd: number; focus(): void };
  focus(where?: "end" | "keep"): void;
  destroy(): void;
}

/** ⌘/Ctrl+Enter ticks or unticks the task on the caret's line. */
function toggleTaskOnCaretLine(view: EditorView): boolean {
  const line = view.state.doc.lineAt(view.state.selection.main.head);
  const m = /^\s*(?:>\s*)*(?:[-*+]|\d+[.)])\s+(?=\[[ xX]\])/.exec(line.text);
  return m ? toggleTaskAt(view, line.from + m[0].length) : false;
}

function filesFrom(e: DragEvent): File[] {
  return Array.from(e.dataTransfer?.files ?? []);
}

export function createMarkdownEditor(parent: HTMLElement, opts: EditorOptions): MarkdownEditorHandle {
  const { hooks } = opts;

  const run = (action: FormatAction) => (view: EditorView) => formatCommands[action](view);

  const editorKeys = keymap.of([
    { key: "Mod-b", run: run("bold") },
    { key: "Mod-i", run: run("italic") },
    { key: "Mod-Shift-x", run: run("strike") },
    { key: "Mod-e", run: run("code") },
    { key: "Mod-Shift-k", run: run("link") },
    { key: "Mod-Alt-1", run: run("h1") },
    { key: "Mod-Alt-2", run: run("h2") },
    { key: "Mod-Alt-3", run: run("h3") },
    { key: "Mod-Shift-7", run: run("numbered") },
    { key: "Mod-Shift-8", run: run("bullet") },
    { key: "Mod-Shift-9", run: run("checklist") },
    {
      key: "Mod-s",
      preventDefault: true,
      run: () => {
        hooks.onSaveNow?.();
        return true;
      },
    },
    { key: "Tab", run: indentListItem, shift: outdentListItem },
    { key: "Mod-Enter", run: toggleTaskOnCaretLine },
    // CodeMirror binds this on macOS and Linux only; Windows gets Ctrl+Y.
    { key: "Mod-Shift-z", run: redo, preventDefault: true },
    {
      key: "Escape",
      run: (view) => {
        view.contentDOM.blur();
        hooks.onEscape?.();
        return true;
      },
    },
  ]);

  const domHandlers = EditorView.domEventHandlers({
    keydown(e, view) {
      if (hooks.onKeydown?.(e)) return true;
      // ⌘/Ctrl+Shift+K inserts a link. Stop it here: the command palette
      // listens for ⌘/Ctrl+K on the window and does not check Shift.
      if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === "k") {
        e.preventDefault();
        e.stopPropagation();
        formatCommands.link(view);
        return true;
      }
      return false;
    },
    paste(e, view) {
      if (hooks.onPaste?.(e) || e.defaultPrevented) return true;
      const text = e.clipboardData?.getData("text/plain") ?? "";
      if (pasteUrlAsLink(view, text)) {
        e.preventDefault();
        return true;
      }
      return false;
    },
    dragover(e) {
      if (e.dataTransfer?.types.includes("Files")) {
        e.preventDefault();
        e.dataTransfer.dropEffect = "copy";
      }
      return false;
    },
    drop(e, view) {
      const files = filesFrom(e);
      if (files.length === 0 || !hooks.onDropFiles) return false;
      e.preventDefault();
      const pos = view.posAtCoords({ x: e.clientX, y: e.clientY }) ?? view.state.doc.length;
      view.focus();
      view.dispatch({ selection: { anchor: pos } });
      hooks.onDropFiles(files);
      return true;
    },
    mousedown(e) {
      if (!(e.metaKey || e.ctrlKey) || !hooks.onOpenLink) return false;
      const target = (e.target as Element | null)?.closest<HTMLElement>("[data-href], [data-ident]");
      if (!target) return false;
      e.preventDefault();
      if (target.dataset.ident) hooks.onOpenLink(target.dataset.ident, "ref");
      else if (target.dataset.href) hooks.onOpenLink(target.dataset.href, "url");
      return true;
    },
  });

  const listener = EditorView.updateListener.of((update) => {
    if (update.docChanged) {
      const remote = update.transactions.some((tr) => tr.annotation(remoteEdit));
      if (!remote) hooks.onChange?.(update.changes, update);
      else {
        // A remote transaction can share an update with local ones only in
        // theory (each dispatch is its own update); report the local parts.
        for (const tr of update.transactions) {
          if (!tr.annotation(remoteEdit) && tr.docChanged) hooks.onChange?.(tr.changes, update);
        }
      }
    }
    if (update.selectionSet || update.focusChanged || update.docChanged || update.geometryChanged) {
      hooks.onSelection?.(update.view);
    }
    if (update.focusChanged) hooks.onFocus?.(update.view.hasFocus);
  });

  const state = EditorState.create({
    doc: opts.doc,
    extensions: [
      Prec.highest(domHandlers),
      Prec.high(keymap.of(completionKeymap)),
      Prec.high(editorKeys),
      history(),
      drawSelection(),
      dropCursor(),
      EditorView.lineWrapping,
      markdown({ base: markdownLanguage, addKeymap: true }),
      keymap.of([...defaultKeymap, ...historyKeymap]),
      editorCompletions({ attach: hooks.onAttach }),
      livePreview(opts.services),
      editorTheme,
      opts.placeholder ? placeholderExt(opts.placeholder) : [],
      EditorView.contentAttributes.of({
        "aria-label": opts.label ?? "Body",
        "aria-multiline": "true",
        spellcheck: "true",
        autocapitalize: "sentences",
      }),
      listener,
    ],
  });

  const view = new EditorView({ state, parent });

  function setText(next: string, caret?: number | null) {
    const current = view.state.doc.toString();
    const changes = diffText(current, next);
    if (changes.length === 0 && caret == null) return;
    view.dispatch({
      changes,
      selection: caret != null ? EditorSelection.cursor(Math.min(caret, next.length)) : undefined,
      userEvent: "input",
      scrollIntoView: caret != null,
    });
    if (caret != null) view.focus();
  }

  return {
    view,
    text: () => view.state.doc.toString(),
    applyRemote(changes, undoable) {
      view.dispatch({
        changes,
        annotations: [remoteEdit.of(true), Transaction.addToHistory.of(undoable)],
      });
    },
    setText,
    insertSnippet(snippet) {
      const sel = view.state.selection.main;
      const current = view.state.doc.toString();
      const { text, caret } = insertSnippetAt(current, sel.from, sel.to, snippet);
      setText(text, caret);
    },
    caret: () => ({
      selectionStart: view.state.selection.main.from,
      selectionEnd: view.state.selection.main.to,
      focus: () => view.focus(),
    }),
    focus(where = "keep") {
      view.focus();
      if (where === "end") {
        const end = view.state.doc.length;
        view.dispatch({ selection: { anchor: end }, scrollIntoView: true });
      }
    },
    destroy: () => view.destroy(),
  };
}
