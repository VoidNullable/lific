// Formatting commands for the editor. The string math is shared with the old
// textarea toolbar (../../markdownFormat.ts); here each result is applied as
// the smallest set of edits, so undo, remote merges and the caret all see a
// precise change instead of a whole-document replacement.

import { EditorSelection, type ChangeSpec } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { indentLess, indentMore } from "@codemirror/commands";
import {
  insertLink,
  toggleCodeBlock,
  toggleInlineWrap,
  toggleLinePrefix,
  type EditorState as FormatState,
  type InlineMarker,
  type LinePrefixKind,
  type TransformResult,
} from "../../markdownFormat";
import { diffText } from "../textDiff";

export type FormatAction =
  | "bold"
  | "italic"
  | "strike"
  | "code"
  | "link"
  | "h1"
  | "h2"
  | "h3"
  | "bullet"
  | "numbered"
  | "checklist"
  | "quote"
  | "codeblock";

function apply(view: EditorView, fn: (s: FormatState) => TransformResult, userEvent: string) {
  const sel = view.state.selection.main;
  const text = view.state.doc.toString();
  const result = fn({ text, selectionStart: sel.from, selectionEnd: sel.to });
  view.dispatch({
    changes: diffText(text, result.text),
    selection: EditorSelection.range(result.selectionStart, result.selectionEnd),
    userEvent,
    scrollIntoView: true,
  });
  view.focus();
  return true;
}

const inline = (marker: InlineMarker) => (view: EditorView) =>
  apply(view, (s) => toggleInlineWrap(s, marker), "input.format");

const prefix = (kind: LinePrefixKind) => (view: EditorView) =>
  apply(view, (s) => toggleLinePrefix(s, kind), "input.format");

/** Set every selected line to heading `level`, or back to text when it
 *  already is one. */
export function setHeading(view: EditorView, level: number): boolean {
  const { state } = view;
  const sel = state.selection.main;
  const first = state.doc.lineAt(sel.from);
  const last = state.doc.lineAt(sel.to);
  const marker = "#".repeat(level) + " ";
  const lines = [];
  for (let n = first.number; n <= last.number; n++) lines.push(state.doc.line(n));
  const all = lines.every((l) => l.text.startsWith(marker));
  const changes: ChangeSpec[] = lines.map((l) => {
    const existing = /^#{1,6} /.exec(l.text)?.[0].length ?? 0;
    return { from: l.from, to: l.from + existing, insert: all ? "" : marker };
  });
  view.dispatch({ changes, userEvent: "input.format", scrollIntoView: true });
  view.focus();
  return true;
}

export const formatCommands: Record<FormatAction, (view: EditorView) => boolean> = {
  bold: inline("**"),
  italic: inline("*"),
  strike: inline("~~"),
  code: inline("`"),
  link: (view) => apply(view, insertLink, "input.format"),
  h1: (view) => setHeading(view, 1),
  h2: (view) => setHeading(view, 2),
  h3: (view) => setHeading(view, 3),
  bullet: prefix("bullet"),
  numbered: prefix("numbered"),
  checklist: prefix("checklist"),
  quote: prefix("quote"),
  codeblock: (view) => apply(view, toggleCodeBlock, "input.format"),
};

/** Which inline formats wrap the selection right now, for the toolbar's
 *  pressed state. */
export function activeFormats(view: EditorView): Set<FormatAction> {
  const out = new Set<FormatAction>();
  const { state } = view;
  const sel = state.selection.main;
  const line = state.doc.lineAt(sel.from).text;
  const h = /^(#{1,3}) /.exec(line);
  if (h) out.add(`h${h[1].length}` as FormatAction);
  if (/^\s*> /.test(line)) out.add("quote");
  if (/^\s*[-*+] \[[ xX]\] /.test(line)) out.add("checklist");
  else if (/^\s*[-*+] /.test(line)) out.add("bullet");
  if (/^\s*\d+[.)] /.test(line)) out.add("numbered");
  const before = state.sliceDoc(Math.max(0, sel.from - 2), sel.from);
  const after = state.sliceDoc(sel.to, sel.to + 2);
  const inner = state.sliceDoc(sel.from, sel.to);
  const wrapped = (m: string) =>
    (before.endsWith(m) && after.startsWith(m)) ||
    (inner.length >= m.length * 2 && inner.startsWith(m) && inner.endsWith(m));
  if (wrapped("**")) out.add("bold");
  else if (wrapped("*")) out.add("italic");
  if (wrapped("~~")) out.add("strike");
  if (wrapped("`")) out.add("code");
  return out;
}

const LIST_LINE_RE = /^\s*(?:[-*+]|\d+[.)])\s/;

/** Tab inside a list item nests it; elsewhere Tab is left to the browser so
 *  keyboard users can still leave the editor. */
export function indentListItem(view: EditorView): boolean {
  const line = view.state.doc.lineAt(view.state.selection.main.from);
  if (!LIST_LINE_RE.test(line.text)) return false;
  return indentMore(view);
}

export function outdentListItem(view: EditorView): boolean {
  const line = view.state.doc.lineAt(view.state.selection.main.from);
  if (!LIST_LINE_RE.test(line.text)) return false;
  return indentLess(view);
}

const URL_RE = /^https?:\/\/\S+$/;

/** Pasting a URL over selected text turns the text into a link. */
export function pasteUrlAsLink(view: EditorView, pasted: string): boolean {
  const sel = view.state.selection.main;
  const url = pasted.trim();
  if (sel.empty || !URL_RE.test(url)) return false;
  const text = view.state.sliceDoc(sel.from, sel.to);
  if (text.includes("\n") || URL_RE.test(text.trim())) return false;
  view.dispatch({
    changes: { from: sel.from, to: sel.to, insert: `[${text}](${url})` },
    selection: { anchor: sel.from + text.length + url.length + 4 },
    userEvent: "input.paste",
  });
  return true;
}

/** Replace [from, to) with `text`, putting the caret at `caret` within it
 *  (or at its end). Used by the slash menu to drop in a block. */
export function insertBlock(
  view: EditorView,
  from: number,
  to: number,
  text: string,
  caret = text.length,
  selectLength = 0,
) {
  view.dispatch({
    changes: { from, to, insert: text },
    selection: EditorSelection.range(from + caret, from + caret + selectLength),
    userEvent: "input.complete",
    scrollIntoView: true,
  });
}
