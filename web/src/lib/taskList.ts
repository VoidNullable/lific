// LIF-502: toggling GFM task-list items (`- [ ] option`) straight from a
// rendered body, without opening the editor.
//
// The rendered checkboxes come out of marked in render order, and the only
// thing the DOM can tell us is "the Nth checkbox". Mapping that back to a
// byte in the source has to agree with marked exactly, including everything
// marked refuses to treat as a task: `- [ ]` inside fenced or indented code,
// inside inline code, in a paragraph, and so on. A hand-written scanner would
// have to re-implement CommonMark container rules to get that right, so this
// module asks marked itself:
//
//   1. `taskStates` lexes the body with the renderer's options and collects
//      every `checkbox` token in the order the renderer emits them.
//   2. To find the source position of task N, each `[ ]`/`[x]` candidate in
//      the source is flipped and the body re-lexed. The real marker is the
//      one whose flip changes task N and nothing else. Candidates that look
//      like a list item's first marker are tried first, starting from the Nth,
//      so the usual case costs one extra lex.
//
// The body is normalized the way Markdown.svelte and marked normalize it
// before lexing (legacy escaped newlines, then CR/CRLF), and positions are
// mapped back so the edit lands on the original string, line endings intact.

import { marked, type Token } from "marked";

/** Markdown.svelte renders `content` with literal `\n` sequences turned into
 *  newlines (LIF-10's escaped-newline bug). Shared here so the renderer and
 *  the task mapping can never disagree about what was rendered. */
export function normalizeEscapedNewlines(source: string): string {
  return source.replace(/\\n/g, "\n");
}

interface Normalized {
  text: string;
  /** `origin[i]` is the index in the original string of `text[i]`. */
  origin: number[];
}

/** Replace every match of `re` with `"\n"`, recording where each output
 *  character came from. A replacement maps to the start of its match. */
function replaceWithNewline(input: Normalized, re: RegExp): Normalized {
  let text = "";
  const origin: number[] = [];
  let last = 0;
  for (const m of input.text.matchAll(re)) {
    const at = m.index;
    text += input.text.slice(last, at) + "\n";
    for (let i = last; i < at; i++) origin.push(input.origin[i]);
    origin.push(input.origin[at]);
    last = at + m[0].length;
  }
  text += input.text.slice(last);
  for (let i = last; i < input.text.length; i++) origin.push(input.origin[i]);
  return { text, origin };
}

function normalize(source: string): Normalized {
  const identity: Normalized = {
    text: source,
    origin: Array.from({ length: source.length }, (_, i) => i),
  };
  // Same order as rendering: Markdown.svelte first, then marked's lexer.
  return replaceWithNewline(replaceWithNewline(identity, /\\n/g), /\r\n|\r/g);
}

/** The options Markdown.svelte renders with. `marked.lexer` replaces rather
 *  than merges its options, so the defaults are spread in explicitly. */
function lex(text: string): Token[] {
  return marked.lexer(text, { ...marked.defaults, gfm: true, breaks: true });
}

function statesOf(text: string): boolean[] {
  const states: boolean[] = [];
  marked.walkTokens(lex(text), (token) => {
    if (token.type === "checkbox") states.push(!!token.checked);
  });
  return states;
}

/** Checked state of every task-list checkbox the renderer would draw for
 *  `source`, in render order. */
export function taskStates(source: string): boolean[] {
  return statesOf(normalize(source).text);
}

const MARKER_RE = /\[[ xX]\]/g;
// Everything before a list item's first marker on its line: container
// indentation, blockquote markers and one or more bullets (`-`, `*`, `+`,
// `1.`, `1)`), each followed by whitespace. Only a ranking hint; every
// candidate is verified against the lexer before it is used.
const ITEM_PREFIX_RE = /^(?:[ \t>]*(?:[*+-]|\d{1,9}[.)])[ \t]+)+$/;

function flipAt(text: string, at: number): string {
  const next = text[at + 1] === " " ? "x" : " ";
  return text.slice(0, at + 1) + next + text.slice(at + 2);
}

/** Index in `text` of the `[` that opens task `index`'s marker, or -1. */
function locate(text: string, index: number, states: boolean[]): number {
  const expected = states.slice();
  expected[index] = !expected[index];
  const verifies = (at: number) => {
    const after = statesOf(flipAt(text, at));
    return after.length === expected.length && after.every((s, i) => s === expected[i]);
  };

  const listed: number[] = [];
  const other: number[] = [];
  for (const m of text.matchAll(MARKER_RE)) {
    const lineStart = text.lastIndexOf("\n", m.index - 1) + 1;
    (ITEM_PREFIX_RE.test(text.slice(lineStart, m.index)) ? listed : other).push(m.index);
  }
  // Every real task marker is a listed candidate, in source order, so task N
  // sits at listed[N] or later (code blocks only add candidates before it).
  const order = [...listed.slice(index), ...listed.slice(0, index), ...other];
  for (const at of order) if (verifies(at)) return at;
  return -1;
}

/** `source` with task `index` set to `checked`. Returns `source` itself when
 *  the item already has that state, and null when the index is out of range
 *  or the marker cannot be located (the caller should not save then). */
export function setTaskChecked(source: string, index: number, checked: boolean): string | null {
  const norm = normalize(source);
  const states = statesOf(norm.text);
  if (!Number.isInteger(index) || index < 0 || index >= states.length) return null;
  if (states[index] === checked) return source;
  const at = locate(norm.text, index, states);
  if (at < 0) return null;
  // The character between the brackets is never part of a normalized
  // sequence, so it maps one to one onto the original string.
  const inner = norm.origin[at + 1];
  return source.slice(0, inner) + (checked ? "x" : " ") + source.slice(inner + 1);
}

/** `source` with task `index` flipped, or null (see `setTaskChecked`). */
export function toggleTask(source: string, index: number): string | null {
  const states = taskStates(source);
  if (index < 0 || index >= states.length) return null;
  return setTaskChecked(source, index, !states[index]);
}
