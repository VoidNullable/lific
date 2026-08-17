// LIF-418: ANSI escape-code rendering for the text attachment viewer.
//
// Build logs, `cargo test` output, and CI dumps get attached constantly and
// they are full of SGR escapes. Rendering them raw shows `[0;32m` noise;
// stripping them loses the one thing that makes a 4000-line log readable. So
// we parse the subset of ANSI that actually appears in tool output (SGR:
// colors + bold/dim/italic/underline) into styled spans, and drop everything
// else on the floor rather than trying to emulate a terminal.
//
// Deliberately NOT supported (silently stripped): cursor movement, erase,
// scroll regions, OSC hyperlinks/titles, and the raw control characters that
// would let a log rewrite the page. A viewer is not a terminal.

/** Resolved visual state for a run of text. */
export interface AnsiStyle {
  /** CSS color for the foreground, or undefined for "inherit". */
  fg?: string;
  /** CSS color for the background, or undefined for "none". */
  bg?: string;
  bold?: boolean;
  dim?: boolean;
  italic?: boolean;
  underline?: boolean;
  /** SGR 7: swap fg/bg at render time. */
  inverse?: boolean;
  /** SGR 9: struck through. */
  strike?: boolean;
}

export interface AnsiSpan {
  text: string;
  style: AnsiStyle;
}

/** The 16 base colors, as CSS custom properties so the viewer stylesheet can
 *  tune them per theme (a pure `#000` foreground is unreadable on a dark
 *  surface, and a pure `#fff` one is unreadable on a light surface). */
const BASE_COLORS = [
  "var(--ansi-black)",
  "var(--ansi-red)",
  "var(--ansi-green)",
  "var(--ansi-yellow)",
  "var(--ansi-blue)",
  "var(--ansi-magenta)",
  "var(--ansi-cyan)",
  "var(--ansi-white)",
  "var(--ansi-bright-black)",
  "var(--ansi-bright-red)",
  "var(--ansi-bright-green)",
  "var(--ansi-bright-yellow)",
  "var(--ansi-bright-blue)",
  "var(--ansi-bright-magenta)",
  "var(--ansi-bright-cyan)",
  "var(--ansi-bright-white)",
];

/** xterm 256-color cube / grayscale ramp, for indexes 16..255. */
function color256(index: number): string {
  if (index < 16) return BASE_COLORS[index];
  if (index < 232) {
    const n = index - 16;
    const levels = [0, 95, 135, 175, 215, 255];
    const r = levels[Math.floor(n / 36) % 6];
    const g = levels[Math.floor(n / 6) % 6];
    const b = levels[n % 6];
    return rgb(r, g, b);
  }
  const gray = 8 + (index - 232) * 10;
  return rgb(gray, gray, gray);
}

function rgb(r: number, g: number, b: number): string {
  const hex = (n: number) => Math.max(0, Math.min(255, n)).toString(16).padStart(2, "0");
  return `#${hex(r)}${hex(g)}${hex(b)}`;
}

/** Matches every escape sequence we care to recognize:
 *   - CSI ... final-byte   (SGR when the final byte is `m`, otherwise dropped)
 *   - OSC ... BEL | ST     (window titles, hyperlinks — dropped whole)
 *   - a lone ESC + single byte (charset selects etc. — dropped)
 *  The `\u001b` may also arrive as the literal two-character sequence some
 *  logs are stored with, which callers normalize before calling in. */
// eslint-disable-next-line no-control-regex
const ESCAPE_RE = /\u001b(?:\[([0-9;:?]*)([ -/]*[@-~])|\][^\u0007\u001b]*(?:\u0007|\u001b\\)|[@-Z\\-_])/g;

function applySgr(style: AnsiStyle, params: number[]): AnsiStyle {
  let next: AnsiStyle = { ...style };
  for (let i = 0; i < params.length; i++) {
    const code = params[i];
    if (code === 0) {
      next = {};
    } else if (code === 1) next.bold = true;
    else if (code === 2) next.dim = true;
    else if (code === 3) next.italic = true;
    else if (code === 4) next.underline = true;
    else if (code === 7) next.inverse = true;
    else if (code === 9) next.strike = true;
    else if (code === 21 || code === 22) {
      next.bold = false;
      next.dim = false;
    } else if (code === 23) next.italic = false;
    else if (code === 24) next.underline = false;
    else if (code === 27) next.inverse = false;
    else if (code === 29) next.strike = false;
    else if (code >= 30 && code <= 37) next.fg = BASE_COLORS[code - 30];
    else if (code === 39) next.fg = undefined;
    else if (code >= 40 && code <= 47) next.bg = BASE_COLORS[code - 40];
    else if (code === 49) next.bg = undefined;
    else if (code >= 90 && code <= 97) next.fg = BASE_COLORS[code - 90 + 8];
    else if (code >= 100 && code <= 107) next.bg = BASE_COLORS[code - 100 + 8];
    else if (code === 38 || code === 48) {
      // Extended color: `38;5;N` (256) or `38;2;R;G;B` (truecolor).
      const mode = params[i + 1];
      let color: string | undefined;
      if (mode === 5 && params.length > i + 2) {
        color = color256(params[i + 2]);
        i += 2;
      } else if (mode === 2 && params.length > i + 4) {
        color = rgb(params[i + 2], params[i + 3], params[i + 4]);
        i += 4;
      } else {
        // Malformed extended color: swallow the introducer and move on.
        i += 1;
        continue;
      }
      if (code === 38) next.fg = color;
      else next.bg = color;
    }
    // Everything else (blink, font selection, overline, ...) is ignored.
  }
  return next;
}

function styleIsEqual(a: AnsiStyle, b: AnsiStyle): boolean {
  return (
    a.fg === b.fg &&
    a.bg === b.bg &&
    !!a.bold === !!b.bold &&
    !!a.dim === !!b.dim &&
    !!a.italic === !!b.italic &&
    !!a.underline === !!b.underline &&
    !!a.inverse === !!b.inverse &&
    !!a.strike === !!b.strike
  );
}

/** True when the string carries at least one escape introducer, so callers can
 *  skip the whole span machinery for the common plain-text file. */
export function hasAnsi(text: string): boolean {
  return text.includes("\u001b");
}

/**
 * Split one line into styled spans. `initial` carries the style left open by
 * the previous line (SGR state spans newlines in real terminal output), and
 * the trailing style is returned so the caller can thread it forward.
 */
export function ansiLineToSpans(
  line: string,
  initial: AnsiStyle = {},
): { spans: AnsiSpan[]; trailing: AnsiStyle } {
  const spans: AnsiSpan[] = [];
  let style = initial;
  let cursor = 0;

  const push = (text: string) => {
    if (!text) return;
    const last = spans[spans.length - 1];
    if (last && styleIsEqual(last.style, style)) last.text += text;
    else spans.push({ text, style: { ...style } });
  };

  ESCAPE_RE.lastIndex = 0;
  let match: RegExpExecArray | null;
  while ((match = ESCAPE_RE.exec(line)) !== null) {
    push(line.slice(cursor, match.index));
    cursor = match.index + match[0].length;
    const [, params, final] = match;
    if (final === "m" && params !== undefined) {
      // `:` is the ITU sub-parameter separator (used by some underline
      // extensions); flattening it to `;` is close enough for our subset.
      const codes = params
        .replace(/:/g, ";")
        .split(";")
        .map((p) => (p === "" ? 0 : Number(p)))
        .filter((n) => Number.isFinite(n));
      style = applySgr(style, codes);
    }
    // Non-SGR sequences are dropped entirely (already skipped by `cursor`).
  }
  push(line.slice(cursor));

  // A line that was nothing but escapes still needs one (empty) span so the
  // renderer emits a row rather than collapsing it away.
  if (spans.length === 0) spans.push({ text: "", style: { ...style } });
  return { spans, trailing: style };
}

/**
 * Convert a whole document into per-line spans, threading SGR state across
 * line boundaries. Returns one entry per input line.
 */
export function ansiToSpans(text: string): AnsiSpan[][] {
  const out: AnsiSpan[][] = [];
  let style: AnsiStyle = {};
  for (const line of text.split("\n")) {
    const { spans, trailing } = ansiLineToSpans(line, style);
    out.push(spans);
    style = trailing;
  }
  return out;
}

/** Drop every escape sequence, leaving readable text. Used for find-in-file
 *  matching and for the copy-to-clipboard payload, so a search for "error"
 *  hits a line whose color changes mid-word. */
export function stripAnsi(text: string): string {
  ESCAPE_RE.lastIndex = 0;
  return text.replace(ESCAPE_RE, "");
}

/** Inline CSS for a span. Kept next to the parser so the mapping from ANSI
 *  semantics to CSS lives in one place. */
export function ansiStyleToCss(style: AnsiStyle): string {
  const fg = style.inverse ? (style.bg ?? "var(--surface)") : style.fg;
  const bg = style.inverse ? (style.fg ?? "var(--text)") : style.bg;
  const parts: string[] = [];
  if (fg) parts.push(`color:${fg}`);
  if (bg) parts.push(`background:${bg}`);
  if (style.bold) parts.push("font-weight:600");
  if (style.dim) parts.push("opacity:0.65");
  if (style.italic) parts.push("font-style:italic");
  const decorations: string[] = [];
  if (style.underline) decorations.push("underline");
  if (style.strike) decorations.push("line-through");
  if (decorations.length) parts.push(`text-decoration:${decorations.join(" ")}`);
  return parts.join(";");
}
