// Two completion sources share the editor's popup:
//
//   - The block menu, opened by "/" at the start of a line or after a space:
//     headings, lists, a checklist, quote, code, table, diagram, divider, and
//     attaching a file. Each entry writes plain Markdown.
//   - Issue references: "#words" searches issues by title, "LIF-" or "lif-4"
//     narrows to a project; accepting inserts the canonical identifier.

import {
  autocompletion,
  type Completion,
  type CompletionContext,
  type CompletionResult,
} from "@codemirror/autocomplete";
import type { EditorView } from "@codemirror/view";
import { findTrigger, searchSuggestions } from "../../references";
import { insertBlock } from "./commands";

export interface CompletionHooks {
  /** Open the file picker (the block menu's "Image or file"). */
  attach?: () => void;
}

interface BlockCommand {
  label: string;
  keywords: string;
  icon: string;
  detail?: string;
  /** Text that replaces the "/query". `‸` marks the caret; `⁅…⁆` marks
   *  text to leave selected instead. */
  template?: string;
  run?: (view: EditorView, from: number, to: number) => void;
}

function blockCommands(hooks: CompletionHooks): BlockCommand[] {
  const list: BlockCommand[] = [
    { label: "Heading 1", keywords: "h1 title heading", icon: "H1", template: "# ‸" },
    { label: "Heading 2", keywords: "h2 heading subtitle", icon: "H2", template: "## ‸" },
    { label: "Heading 3", keywords: "h3 heading", icon: "H3", template: "### ‸" },
    { label: "Bulleted list", keywords: "bullet ul unordered list", icon: "•", template: "- ‸" },
    { label: "Numbered list", keywords: "ol ordered numbered list", icon: "1.", template: "1. ‸" },
    { label: "Checklist", keywords: "todo task checkbox check list", icon: "☐", template: "- [ ] ‸" },
    { label: "Quote", keywords: "blockquote quote", icon: "❝", template: "> ‸" },
    { label: "Code block", keywords: "code fence pre snippet", icon: "{}", template: "```‸\n\n```" },
    {
      label: "Table",
      keywords: "table grid columns",
      icon: "▦",
      template: "| ⁅Column⁆ | Column |\n| --- | --- |\n|  |  |",
    },
    {
      label: "Diagram",
      keywords: "mermaid diagram chart flow graph",
      icon: "◇",
      detail: "Mermaid",
      template: "```mermaid\nflowchart LR\n  ⁅A⁆ --> B\n```",
    },
    { label: "Divider", keywords: "hr rule divider line separator", icon: "—", template: "---\n‸" },
  ];
  if (hooks.attach) {
    const attach = hooks.attach;
    list.push({
      label: "Image or file",
      keywords: "image picture photo file upload attach screenshot",
      icon: "⬆",
      run: (view, from, to) => {
        view.dispatch({ changes: { from, to, insert: "" }, selection: { anchor: from } });
        attach();
      },
    });
  }
  return list;
}

function applyTemplate(view: EditorView, from: number, to: number, template: string) {
  const open = template.indexOf("⁅");
  if (open >= 0) {
    const close = template.indexOf("⁆", open);
    const text = template.replace("⁅", "").replace("⁆", "");
    insertBlock(view, from, to, text, open, close - open - 1);
    return;
  }
  const caret = template.indexOf("‸");
  const text = template.replace("‸", "");
  insertBlock(view, from, to, text, caret >= 0 ? caret : text.length);
}

function iconFor(completion: Completion & { icon?: string }): Node | null {
  if (!completion.icon) return null;
  const span = document.createElement("span");
  span.className = "cm-md-cmd-icon";
  span.textContent = completion.icon;
  return span;
}

const SLASH_RE = /(?:^|\s)\/([\w-]*)$/;

function slashSource(hooks: CompletionHooks) {
  return (ctx: CompletionContext): CompletionResult | null => {
    const line = ctx.state.doc.lineAt(ctx.pos);
    const before = line.text.slice(0, ctx.pos - line.from);
    const m = SLASH_RE.exec(before);
    if (!m) return null;
    // Not inside code.
    const lineText = line.text.trimStart();
    if (lineText.startsWith("```") || lineText.startsWith("    ")) return null;
    const slashAt = ctx.pos - m[1].length - 1;
    const query = m[1].toLowerCase();
    const commands = blockCommands(hooks).filter(
      (c) => !query || c.label.toLowerCase().includes(query) || c.keywords.includes(query),
    );
    if (commands.length === 0) return null;
    return {
      from: slashAt,
      to: ctx.pos,
      filter: false,
      options: commands.map((c, i) => ({
        label: c.label,
        detail: c.detail,
        icon: c.icon,
        boost: -i,
        apply: (view: EditorView, _completion: Completion, from: number, to: number) => {
          // Blocks start on their own line: a command typed mid-line breaks
          // the line first.
          const lineStart = view.state.doc.lineAt(from).from;
          const leading = view.state.sliceDoc(lineStart, from).trim() ? "\n" : "";
          if (c.run) return c.run(view, from, to);
          applyTemplate(view, from, to, leading + (c.template ?? ""));
        },
      })) as Completion[],
    };
  };
}

async function referenceSource(ctx: CompletionContext): Promise<CompletionResult | null> {
  const line = ctx.state.doc.lineAt(ctx.pos);
  const before = line.text.slice(0, ctx.pos - line.from);
  const trig = findTrigger(before, before.length);
  if (!trig) return null;
  const from = line.from + trig.start;
  const hits = await searchSuggestions(trig);
  if (ctx.aborted) return null;
  if (hits.length === 0) return null;
  return {
    from,
    to: ctx.pos,
    filter: false,
    options: hits.map((hit, i) => ({
      label: hit.title || hit.identifier,
      detail: hit.identifier,
      boost: -i,
      apply: `${hit.identifier} `,
    })),
  };
}

export function editorCompletions(hooks: CompletionHooks) {
  return autocompletion({
    override: [slashSource(hooks), referenceSource],
    icons: false,
    addToOptions: [{ render: iconFor as (c: Completion) => Node | null, position: 20 }],
    activateOnTypingDelay: 120,
    closeOnBlur: true,
    tooltipClass: () => "cm-md-completions",
  });
}
