// What link, if any, sits at a position: for the "Open" popover shown while
// the caret is inside one.

import type { EditorState } from "@codemirror/state";
import { syntaxTree } from "@codemirror/language";
import { IDENTIFIER_RE } from "../../references";

export interface LinkAt {
  href: string;
  kind: "url" | "ref";
  from: number;
  to: number;
}

export function linkAt(state: EditorState, pos: number): LinkAt | null {
  const tree = syntaxTree(state);
  for (const side of [-1, 1] as const) {
    for (let node: ReturnType<typeof tree.resolveInner> | null = tree.resolveInner(pos, side); node; node = node.parent) {
      if (node.name === "Link" || node.name === "Autolink") {
        const url = node.getChild("URL");
        if (!url) return null;
        return { href: state.sliceDoc(url.from, url.to), kind: "url", from: node.from, to: node.to };
      }
      if (node.name === "URL" && node.parent?.name !== "Image") {
        return { href: state.sliceDoc(node.from, node.to), kind: "url", from: node.from, to: node.to };
      }
      if (node.name === "Image" || node.name === "InlineCode" || node.name === "FencedCode") return null;
    }
  }
  const line = state.doc.lineAt(pos);
  const re = new RegExp(IDENTIFIER_RE.source, "g");
  for (let m = re.exec(line.text); m; m = re.exec(line.text)) {
    const from = line.from + m.index;
    const to = from + m[0].length;
    if (pos >= from && pos <= to) return { href: m[0], kind: "ref", from, to };
  }
  return null;
}
