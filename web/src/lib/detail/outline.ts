// Reading aids for a Markdown body: its outline (the headings, for the
// "On this page" list) and its length.

export interface OutlineHeading {
  /** 1 to 6, as written. */
  level: number;
  /** The heading text with inline Markdown markers stripped. */
  text: string;
  /** 1-based line number of the heading's text in the body. */
  line: number;
  /** Position among every heading in the body, whatever its level. */
  index: number;
}

const FENCE = /^\s{0,3}(`{3,}|~{3,})/;
const ATX = /^\s{0,3}(#{1,6})(?:[ \t]+(.*?))?(?:[ \t]+#+)?[ \t]*$/;
const SETEXT_1 = /^\s{0,3}=+[ \t]*$/;
const SETEXT_2 = /^\s{0,3}-+[ \t]*$/;
// Lines that cannot be a setext heading's text.
const NOT_PARAGRAPH = /^\s{0,3}([-*+]|\d+[.)])[ \t]|^\s{0,3}(>|#|\||<)|^\s{4}|^\s*$/;

function plain(text: string): string {
  return text
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/[*_~`]/g, "")
    .trim();
}

/** Every heading in `markdown`, in order, skipping fenced code. */
export function outlineOf(markdown: string): OutlineHeading[] {
  const lines = markdown.split("\n");
  const out: OutlineHeading[] = [];
  let fence: string | null = null;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const f = FENCE.exec(line);
    if (fence) {
      if (f && f[1][0] === fence[0] && f[1].length >= fence.length) fence = null;
      continue;
    }
    if (f) {
      fence = f[1];
      continue;
    }
    const atx = ATX.exec(line);
    if (atx) {
      const text = plain(atx[2] ?? "");
      if (text) out.push({ level: atx[1].length, text, line: i + 1, index: out.length });
      continue;
    }
    const next = lines[i + 1];
    if (next !== undefined && !NOT_PARAGRAPH.test(line) && (i === 0 || /^\s*$/.test(lines[i - 1]))) {
      const level = SETEXT_1.test(next) ? 1 : SETEXT_2.test(next) ? 2 : 0;
      if (level) {
        const text = plain(line);
        if (text) out.push({ level, text, line: i + 1, index: out.length });
        i += 1;
      }
    }
  }
  return out;
}

/** The headings worth listing: the top three levels present, and only
 *  when there are enough of them to need a map. */
export function tableOfContents(headings: OutlineHeading[], min = 3): OutlineHeading[] {
  if (headings.length < min) return [];
  const top = Math.min(...headings.map((h) => h.level));
  return headings.filter((h) => h.level <= top + 2);
}

/** Words in a Markdown body, code and link targets excluded. */
export function wordCount(markdown: string): number {
  const text = markdown
    .replace(/```[\s\S]*?```/g, " ")
    .replace(/\]\([^)]*\)/g, "]")
    .replace(/[#>*_~`|[\]()-]/g, " ");
  const words = text.match(/[\p{L}\p{N}][\p{L}\p{N}'’]*/gu);
  return words ? words.length : 0;
}

/** Minutes to read at 230 words a minute, at least one. */
export function readingMinutes(words: number): number {
  return Math.max(1, Math.round(words / 230));
}
