// Live preview: the Markdown source is the document, and decorations make it
// read like the rendered page. Syntax marks (`**`, `#`, `>`, link targets)
// are hidden until the selection reaches the element they belong to, so the
// person edits the real text without ever switching modes. Images, task
// checkboxes, rules, tables, diagrams and HTML blocks are drawn as widgets
// over their source.
//
// Decorations never change the text. Everything here is a view over ranges of
// the document, which is what keeps stored bodies byte-for-byte what was
// typed (agents edit them with exact-match find and replace).

import {
  EditorState,
  Facet,
  StateEffect,
  StateField,
  type Extension,
  type Range,
} from "@codemirror/state";
import { Decoration, EditorView, WidgetType, type DecorationSet } from "@codemirror/view";
import { ensureSyntaxTree, syntaxTree } from "@codemirror/language";
import type { SyntaxNode } from "@lezer/common";
import { IDENTIFIER_RE } from "../../references";
import { splitImageAlt } from "../../imageSize";

export interface PreviewServices {
  /** Where an image reference should load from in the editor. */
  imageSrc(url: string, width: number | null): { src: string; fallback: string | null };
  /** Open the image whose source starts at `pos` full screen. */
  openImage(view: EditorView, pos: number): void;
  /** Render a Markdown block (a table, a diagram, raw HTML) into `dom`
   *  exactly as the read view would. Returns a cleanup. */
  renderBlock(dom: HTMLElement, source: string): () => void;
}

export const previewServices = Facet.define<PreviewServices, PreviewServices | null>({
  combine: (values) => values[0] ?? null,
});

const setFocused = StateEffect.define<boolean>();

/** Whether the editor has focus. Without focus nothing is revealed, so an
 *  editor nobody is typing in reads exactly like the rendered page. */
const focused = StateField.define<boolean>({
  create: () => false,
  update(value, tr) {
    for (const e of tr.effects) if (e.is(setFocused)) return e.value;
    return value;
  },
});


const ATTACHMENT_RE = /^(?:https?:\/\/[^/]+)?\/api\/attachments\/(\d+)\/?$/;

// ── widgets ────────────────────────────────────────────────

class ImageWidget extends WidgetType {
  constructor(
    readonly url: string,
    readonly alt: string,
    readonly width: number | null,
    readonly title: string,
    readonly block: boolean,
    readonly revealed: boolean,
  ) {
    super();
  }

  eq(other: ImageWidget) {
    return (
      other.url === this.url &&
      other.alt === this.alt &&
      other.width === this.width &&
      other.title === this.title &&
      other.block === this.block &&
      other.revealed === this.revealed
    );
  }

  toDOM(view: EditorView) {
    const wrap = document.createElement("span");
    wrap.className = "cm-md-image" + (this.block ? " cm-md-image--block" : "");
    if (this.revealed) wrap.classList.add("cm-md-image--revealed");
    const img = document.createElement("img");
    img.alt = this.alt;
    img.draggable = false;
    const services = view.state.facet(previewServices);
    const { src, fallback } = services?.imageSrc(this.url, this.width) ?? {
      src: this.url,
      fallback: null,
    };
    img.src = src;
    if (fallback) {
      img.addEventListener(
        "error",
        () => {
          if (img.src !== fallback) img.src = fallback;
        },
        { once: true },
      );
    }
    if (this.width) img.style.maxWidth = `min(100%, ${this.width}px)`;
    wrap.appendChild(img);
    if (this.title && this.block) {
      const caption = document.createElement("span");
      caption.className = "cm-md-image__caption";
      caption.textContent = this.title;
      wrap.appendChild(caption);
    }
    const expand = document.createElement("button");
    expand.type = "button";
    expand.className = "cm-md-image__open";
    expand.title = "View full size";
    expand.setAttribute("aria-label", "View full size");
    expand.innerHTML =
      '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 3h6v6"/><path d="M9 21H3v-6"/><path d="m21 3-7 7"/><path d="m3 21 7-7"/></svg>';
    expand.addEventListener("mousedown", (e) => e.preventDefault());
    expand.addEventListener("click", (e) => {
      e.preventDefault();
      e.stopPropagation();
      const pos = view.posAtDOM(wrap);
      services?.openImage(view, pos);
    });
    wrap.appendChild(expand);
    return wrap;
  }

  ignoreEvent(e: Event) {
    // Clicks on the expand button stay with the button; everything else
    // reaches the editor so a click places the caret beside the image.
    return e.target instanceof Element && !!e.target.closest(".cm-md-image__open");
  }
}

class CheckboxWidget extends WidgetType {
  constructor(readonly checked: boolean) {
    super();
  }
  eq(other: CheckboxWidget) {
    return other.checked === this.checked;
  }
  toDOM(view: EditorView) {
    const box = document.createElement("input");
    box.type = "checkbox";
    box.className = "cm-md-task";
    box.checked = this.checked;
    box.setAttribute("aria-label", this.checked ? "Mark as not done" : "Mark as done");
    box.addEventListener("mousedown", (e) => e.preventDefault());
    box.addEventListener("click", (e) => {
      e.preventDefault();
      const pos = view.posAtDOM(box);
      toggleTaskAt(view, pos);
    });
    return box;
  }
  ignoreEvent() {
    return true;
  }
}

/** Flip the task marker that starts at `pos` (`[ ]` ↔ `[x]`). */
export function toggleTaskAt(view: EditorView, pos: number): boolean {
  const text = view.state.sliceDoc(pos, pos + 3);
  if (!/^\[[ xX]\]$/.test(text)) return false;
  const checked = text[1] !== " ";
  view.dispatch({
    changes: { from: pos + 1, to: pos + 2, insert: checked ? " " : "x" },
    userEvent: "input.task",
  });
  return true;
}

class BulletWidget extends WidgetType {
  constructor(readonly depth: number) {
    super();
  }
  eq(other: BulletWidget) {
    return other.depth === this.depth;
  }
  toDOM() {
    const span = document.createElement("span");
    span.className = "cm-md-bullet";
    span.textContent = this.depth % 3 === 0 ? "•" : this.depth % 3 === 1 ? "◦" : "▪";
    return span;
  }
}

class RuleWidget extends WidgetType {
  eq() {
    return true;
  }
  toDOM() {
    const hr = document.createElement("span");
    hr.className = "cm-md-hr";
    return hr;
  }
}

class FenceLabelWidget extends WidgetType {
  constructor(readonly lang: string) {
    super();
  }
  eq(other: FenceLabelWidget) {
    return other.lang === this.lang;
  }
  toDOM() {
    const span = document.createElement("span");
    span.className = "cm-md-fence-label";
    span.textContent = this.lang;
    return span;
  }
}

/** A block drawn by the read view's renderer: tables, diagrams, raw HTML.
 *  Clicking it puts the caret in its source. */
class RenderedBlockWidget extends WidgetType {
  private cleanup: (() => void) | null = null;
  constructor(
    readonly source: string,
    readonly kind: string,
  ) {
    super();
  }
  eq(other: RenderedBlockWidget) {
    return other.source === this.source && other.kind === this.kind;
  }
  get estimatedHeight() {
    return this.kind === "mermaid" ? 240 : Math.max(32, this.source.split("\n").length * 30);
  }
  toDOM(view: EditorView) {
    const wrap = document.createElement("div");
    wrap.className = `cm-md-block cm-md-block--${this.kind}`;
    const services = view.state.facet(previewServices);
    if (services) this.cleanup = services.renderBlock(wrap, this.source);
    else {
      const pre = document.createElement("pre");
      pre.textContent = this.source;
      wrap.appendChild(pre);
    }
    wrap.addEventListener("mousedown", (e) => {
      const target = e.target as Element | null;
      // Links and controls inside the rendered block keep working.
      if (target?.closest("a, button, input, summary")) return;
      e.preventDefault();
      const pos = view.posAtDOM(wrap);
      view.focus();
      view.dispatch({ selection: { anchor: pos }, scrollIntoView: false });
    });
    return wrap;
  }
  destroy() {
    this.cleanup?.();
    this.cleanup = null;
  }
  ignoreEvent() {
    return true;
  }
}

// ── decoration building ────────────────────────────────────

const hidden = Decoration.replace({});
const syntaxMark = Decoration.mark({ class: "cm-md-syntax" });

function lineDeco(cls: string) {
  return Decoration.line({ class: cls });
}

interface Builder {
  add(from: number, to: number, deco: Decoration): void;
}

/** Whether any selection range touches [from, to]. */
function touches(state: EditorState, isFocused: boolean, from: number, to: number): boolean {
  if (!isFocused) return false;
  for (const r of state.selection.ranges) {
    if (r.from <= to && r.to >= from) return true;
  }
  return false;
}

/** Whether the selection touches any of the lines [from, to] spans. */
function linesTouch(state: EditorState, isFocused: boolean, from: number, to: number): boolean {
  const start = state.doc.lineAt(from).from;
  const end = state.doc.lineAt(to).to;
  return touches(state, isFocused, start, end);
}

function listDepth(node: SyntaxNode): number {
  let depth = 0;
  for (let p = node.parent; p; p = p.parent) {
    if (p.name === "BulletList" || p.name === "OrderedList") depth++;
  }
  return Math.max(0, depth - 1);
}

function child(node: SyntaxNode, name: string): SyntaxNode | null {
  return node.getChild(name);
}

/** Ranges where identifiers must not be decorated. */
const NO_REF_NODES = new Set([
  "InlineCode",
  "FencedCode",
  "CodeBlock",
  "Link",
  "Image",
  "URL",
  "Autolink",
  "HTMLBlock",
  "HTMLTag",
  "Table",
]);

function build(state: EditorState, isFocused: boolean): DecorationSet {
  const ranges: Range<Decoration>[] = [];
  const b: Builder = {
    add(from, to, deco) {
      if (from > to) return;
      ranges.push(deco.range(from, to));
    },
  };
  const doc = state.doc;
  const tree = ensureSyntaxTree(state, doc.length, 200) ?? syntaxTree(state);
  const skipRefs: [number, number][] = [];
  const codeRanges: [number, number][] = [];

  tree.iterate({
    enter: (ref) => {
      const node = ref.node;
      const name = node.name;
      if (NO_REF_NODES.has(name)) skipRefs.push([node.from, node.to]);

      // Headings
      const atx = /^ATXHeading(\d)$/.exec(name);
      const setext = /^SetextHeading(\d)$/.exec(name);
      if (atx || setext) {
        const level = Number((atx ?? setext)![1]);
        const firstLine = doc.lineAt(node.from);
        b.add(firstLine.from, firstLine.from, lineDeco(`cm-md-h cm-md-h${level}`));
        if (setext) {
          const last = doc.lineAt(node.to);
          if (last.number !== firstLine.number) {
            b.add(last.from, last.from, lineDeco("cm-md-setext-mark"));
          }
        }
        const active = linesTouch(state, isFocused, node.from, node.to);
        for (let m = node.firstChild; m; m = m.nextSibling) {
          if (m.name !== "HeaderMark") continue;
          if (atx && m.from === node.from) {
            // `## ` with its space; a closing `##` run is hidden the same way.
            const end = doc.sliceString(m.to, m.to + 1) === " " ? m.to + 1 : m.to;
            if (active) b.add(m.from, end, syntaxMark);
            else b.add(m.from, end, hidden);
          } else if (active) b.add(m.from, m.to, syntaxMark);
          else if (atx) b.add(m.from, m.to, hidden);
          else b.add(m.from, m.to, Decoration.mark({ class: "cm-md-syntax cm-md-setext-rule" }));
        }
        return;
      }

      switch (name) {
        case "Emphasis":
        case "StrongEmphasis":
        case "Strikethrough":
        case "InlineCode": {
          const cls =
            name === "Emphasis"
              ? "cm-md-em"
              : name === "StrongEmphasis"
                ? "cm-md-strong"
                : name === "Strikethrough"
                  ? "cm-md-strike"
                  : "cm-md-code";
          b.add(node.from, node.to, Decoration.mark({ class: cls }));
          const active = touches(state, isFocused, node.from, node.to);
          const markName =
            name === "Strikethrough"
              ? "StrikethroughMark"
              : name === "InlineCode"
                ? "CodeMark"
                : "EmphasisMark";
          for (let m = node.firstChild; m; m = m.nextSibling) {
            if (m.name !== markName) continue;
            b.add(m.from, m.to, active ? syntaxMark : hidden);
          }
          return name === "InlineCode" ? false : undefined;
        }

        case "Link": {
          const marks = node.getChildren("LinkMark");
          const url = child(node, "URL");
          const href = url ? doc.sliceString(url.from, url.to) : "";
          const active = touches(state, isFocused, node.from, node.to);
          const open = marks[0];
          const close = marks.find((m) => doc.sliceString(m.from, m.to) === "]");
          if (!open || !close) return;
          const attachment = ATTACHMENT_RE.test(href);
          b.add(
            open.to,
            close.from,
            Decoration.mark({
              class: attachment ? "cm-md-link cm-md-attachment" : "cm-md-link",
              attributes: { "data-href": href },
            }),
          );
          if (active) {
            b.add(open.from, open.to, syntaxMark);
            b.add(close.from, node.to, Decoration.mark({ class: "cm-md-syntax cm-md-url" }));
          } else {
            b.add(open.from, open.to, hidden);
            b.add(close.from, node.to, hidden);
          }
          return false;
        }

        case "Autolink": {
          const url = child(node, "URL");
          if (!url) return false;
          const href = doc.sliceString(url.from, url.to);
          b.add(url.from, url.to, Decoration.mark({ class: "cm-md-link", attributes: { "data-href": href } }));
          const active = touches(state, isFocused, node.from, node.to);
          b.add(node.from, url.from, active ? syntaxMark : hidden);
          b.add(url.to, node.to, active ? syntaxMark : hidden);
          return false;
        }

        case "URL": {
          // A bare URL (GFM autolink). URLs inside links and images are
          // handled with their parent.
          const href = doc.sliceString(node.from, node.to);
          b.add(node.from, node.to, Decoration.mark({ class: "cm-md-link", attributes: { "data-href": href } }));
          return false;
        }

        case "Image": {
          const marks = node.getChildren("LinkMark");
          const url = child(node, "URL");
          const titleNode = child(node, "LinkTitle");
          const close = marks.find((m) => doc.sliceString(m.from, m.to) === "]");
          if (!url || !close) return false;
          const rawAlt = doc.sliceString(node.from + 2, close.from);
          const { alt, width } = splitImageAlt(rawAlt);
          const title = titleNode ? doc.sliceString(titleNode.from + 1, titleNode.to - 1) : "";
          const line = doc.lineAt(node.from);
          const block = line.text.trim() === doc.sliceString(node.from, node.to).trim();
          const active = touches(state, isFocused, node.from, node.to);
          const src = doc.sliceString(url.from, url.to);
          if (active) {
            b.add(node.from, node.to, Decoration.mark({ class: "cm-md-syntax cm-md-image-src" }));
            b.add(
              node.to,
              node.to,
              Decoration.widget({
                widget: new ImageWidget(src, alt, width, title, block, true),
                side: 1,
              }),
            );
          } else {
            if (block) b.add(line.from, line.from, lineDeco("cm-md-image-line"));
            b.add(
              node.from,
              node.to,
              Decoration.replace({ widget: new ImageWidget(src, alt, width, title, block, false) }),
            );
          }
          return false;
        }

        case "Task": {
          const marker = child(node, "TaskMarker");
          if (!marker) return;
          const text = doc.sliceString(marker.from, marker.to);
          const checked = text[1] !== " ";
          if (checked) {
            const line = doc.lineAt(node.from);
            b.add(line.from, line.from, lineDeco("cm-md-task-done"));
          }
          // The checkbox stays a checkbox unless the caret is inside `[ ]`.
          const inside =
            isFocused &&
            state.selection.ranges.some((r) => r.from > marker.from && r.to < marker.to);
          if (!inside) {
            const end = doc.sliceString(marker.to, marker.to + 1) === " " ? marker.to + 1 : marker.to;
            b.add(marker.from, end, Decoration.replace({ widget: new CheckboxWidget(checked) }));
          }
          return;
        }

        case "ListMark": {
          const item = node.parent;
          const list = item?.parent;
          if (!item || !list) return;
          const line = doc.lineAt(node.from);
          const active = touches(state, isFocused, line.from, line.to);
          const isTask = !!item.getChild("Task");
          const after = doc.sliceString(node.to, node.to + 1) === " " ? node.to + 1 : node.to;
          if (list.name === "BulletList") {
            if (isTask) {
              if (!active) b.add(node.from, after, hidden);
              else b.add(node.from, node.to, syntaxMark);
            } else if (!active) {
              b.add(node.from, node.to, Decoration.replace({ widget: new BulletWidget(listDepth(node)) }));
            } else b.add(node.from, node.to, syntaxMark);
          } else {
            b.add(node.from, node.to, Decoration.mark({ class: "cm-md-list-num" }));
          }
          return;
        }

        case "Blockquote": {
          for (let pos = node.from; pos <= node.to; ) {
            const line = doc.lineAt(pos);
            b.add(line.from, line.from, lineDeco("cm-md-quote"));
            pos = line.to + 1;
          }
          return;
        }

        case "QuoteMark": {
          const line = doc.lineAt(node.from);
          const active = touches(state, isFocused, line.from, line.to);
          const end = doc.sliceString(node.to, node.to + 1) === " " ? node.to + 1 : node.to;
          b.add(node.from, end, active ? syntaxMark : hidden);
          return;
        }

        case "HorizontalRule": {
          const line = doc.lineAt(node.from);
          const active = touches(state, isFocused, line.from, line.to);
          if (active) b.add(node.from, node.to, syntaxMark);
          else b.add(node.from, node.to, Decoration.replace({ widget: new RuleWidget() }));
          return;
        }

        case "FencedCode":
        case "CodeBlock": {
          codeRanges.push([node.from, node.to]);
          const first = doc.lineAt(node.from);
          const last = doc.lineAt(node.to);
          const active = linesTouch(state, isFocused, node.from, node.to);
          const info = child(node, "CodeInfo");
          const lang = info ? doc.sliceString(info.from, info.to).trim().toLowerCase() : "";
          if (name === "FencedCode" && lang === "mermaid" && !active) {
            b.add(
              first.from,
              last.to,
              Decoration.replace({
                widget: new RenderedBlockWidget(doc.sliceString(first.from, last.to), "mermaid"),
                block: true,
              }),
            );
            return false;
          }
          for (let n = first.number; n <= last.number; n++) {
            const line = doc.line(n);
            let cls = "cm-md-codeblock";
            if (n === first.number) cls += " cm-md-codeblock-first";
            if (n === last.number) cls += " cm-md-codeblock-last";
            b.add(line.from, line.from, lineDeco(cls));
          }
          if (name === "FencedCode") {
            const marks = node.getChildren("CodeMark");
            const open = marks[0];
            const close = marks.length > 1 ? marks[marks.length - 1] : null;
            if (open) {
              const openEnd = doc.lineAt(open.from).to;
              if (active) b.add(open.from, openEnd, syntaxMark);
              else if (lang)
                b.add(open.from, openEnd, Decoration.replace({ widget: new FenceLabelWidget(lang) }));
              else b.add(open.from, openEnd, hidden);
            }
            if (close) b.add(close.from, close.to, active ? syntaxMark : hidden);
          }
          return false;
        }

        case "Table":
        case "HTMLBlock": {
          const first = doc.lineAt(node.from);
          const last = doc.lineAt(node.to);
          const active = linesTouch(state, isFocused, node.from, node.to);
          if (!active) {
            b.add(
              first.from,
              last.to,
              Decoration.replace({
                widget: new RenderedBlockWidget(
                  doc.sliceString(first.from, last.to),
                  name === "Table" ? "table" : "html",
                ),
                block: true,
              }),
            );
          } else {
            for (let n = first.number; n <= last.number; n++) {
              b.add(doc.line(n).from, doc.line(n).from, lineDeco("cm-md-source"));
            }
          }
          return false;
        }
      }
      return undefined;
    },
  });

  // Identifier references (LIF-42, LIF-DOC-3) outside code and links.
  addReferences(state, b, skipRefs);
  addBlankLines(state, isFocused, b, codeRanges);

  return Decoration.set(ranges, true);
}

/** Blank lines between blocks are drawn shorter, close to the paragraph
 *  spacing of the rendered page, except the one the caret is on. */
function addBlankLines(
  state: EditorState,
  isFocused: boolean,
  b: Builder,
  code: [number, number][],
) {
  const doc = state.doc;
  let ci = 0;
  for (let n = 1; n <= doc.lines; n++) {
    const line = doc.line(n);
    if (line.length > 0 && line.text.trim() !== "") continue;
    while (ci < code.length && code[ci][1] < line.from) ci++;
    if (ci < code.length && code[ci][0] <= line.from && line.to <= code[ci][1]) continue;
    if (touches(state, isFocused, line.from, line.to)) continue;
    b.add(line.from, line.from, lineDeco("cm-md-blank"));
  }
}

function addReferences(state: EditorState, b: Builder, skip: [number, number][]) {
  const text = state.doc.toString();
  skip.sort((x, y) => x[0] - y[0]);
  const re = new RegExp(IDENTIFIER_RE.source, "g");
  let si = 0;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    const from = m.index;
    const to = from + m[0].length;
    while (si < skip.length && skip[si][1] < from) si++;
    let blocked = false;
    for (let k = si; k < skip.length && skip[k][0] <= to; k++) {
      if (skip[k][0] < to && skip[k][1] > from) {
        blocked = true;
        break;
      }
    }
    if (blocked) continue;
    const kind = m[2] === "DOC-" ? "page" : m[2] === "PLAN-" ? "plan" : "issue";
    b.add(
      from,
      to,
      Decoration.mark({
        class: "cm-md-ref",
        attributes: { "data-ident": m[0], "data-kind": kind },
      }),
    );
  }
}

const decorations = StateField.define<DecorationSet>({
  create: (state) => build(state, false),
  update(value, tr) {
    const isFocused = tr.state.field(focused);
    const focusChanged = tr.startState.field(focused) !== isFocused;
    if (tr.docChanged || focusChanged || tr.selection || syntaxTree(tr.state) !== syntaxTree(tr.startState)) {
      return build(tr.state, isFocused);
    }
    return value.map(tr.changes);
  },
  provide: (f) => EditorView.decorations.from(f),
});

/** Keeps `focused` in step with the editor's real focus. */
const focusTracker = EditorView.focusChangeEffect.of((_state, isFocused) => setFocused.of(isFocused));

export function livePreview(services: PreviewServices): Extension {
  return [previewServices.of(services), focused, focusTracker, decorations];
}
