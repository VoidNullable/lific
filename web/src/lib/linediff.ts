// LIF-158 - line-level diff for multi-line value changes.
//
// Pure, dependency-free LCS diff over lines. The activity feed used to show a
// whole old value in red and a whole new value in green, which made a one-word
// edit to a long description unreadable. This produces the usual added /
// removed / context sequence in old-to-new order instead, plus a helper that
// folds long unchanged runs away.

export type DiffKind = "context" | "added" | "removed";

export type DiffLine = {
  kind: DiffKind;
  text: string;
};

/** A run of unchanged lines collapsed away by `foldContext`. */
export type DiffFold = {
  kind: "fold";
  count: number;
};

export type DiffRow = DiffLine | DiffFold;

/**
 * Largest document, in lines, we are willing to diff. LCS is O(n*m) in time
 * and memory, so anything past this bails out and the caller falls back to
 * whole-value rendering rather than freezing the tab.
 */
export const MAX_DIFF_LINES = 3000;

/** Unchanged lines kept on each side of a change before folding kicks in. */
export const DEFAULT_CONTEXT_LINES = 3;

/** A fold is only worth drawing when it hides at least this many lines. */
const MIN_FOLD_LINES = 2;

/**
 * Split text into diffable lines. Line endings are normalized, a single
 * trailing newline is dropped (it terminates the last line rather than
 * starting an empty one), and empty text yields no lines at all so a
 * creation reads as pure additions.
 */
function splitLines(text: string): string[] {
  if (text === "") return [];
  const normalized = text.replace(/\r\n?/g, "\n").replace(/\n$/, "");
  if (normalized === "") return [""];
  return normalized.split("\n");
}

function asLines(texts: string[], kind: DiffKind): DiffLine[] {
  return texts.map((text) => ({ kind, text }));
}

/**
 * Diff two texts line by line.
 *
 * Returns the lines in old-to-new order: at each change point the removed
 * lines come first, then the added ones. Returns `null` when either side
 * exceeds `MAX_DIFF_LINES`, which is the caller's cue to fall back to
 * rendering both values whole.
 */
export function diffLines(oldText: string, newText: string): DiffLine[] | null {
  const oldLines = splitLines(oldText);
  const newLines = splitLines(newText);
  if (oldLines.length > MAX_DIFF_LINES || newLines.length > MAX_DIFF_LINES) {
    return null;
  }
  if (oldLines.length === 0) return asLines(newLines, "added");
  if (newLines.length === 0) return asLines(oldLines, "removed");

  // Shared head and tail are trivially context, and trimming them keeps the
  // quadratic part of the work proportional to the size of the edit.
  let head = 0;
  while (
    head < oldLines.length &&
    head < newLines.length &&
    oldLines[head] === newLines[head]
  ) {
    head++;
  }
  let oldEnd = oldLines.length;
  let newEnd = newLines.length;
  while (
    oldEnd > head &&
    newEnd > head &&
    oldLines[oldEnd - 1] === newLines[newEnd - 1]
  ) {
    oldEnd--;
    newEnd--;
  }

  return [
    ...asLines(oldLines.slice(0, head), "context"),
    ...lcsDiff(oldLines.slice(head, oldEnd), newLines.slice(head, newEnd)),
    ...asLines(oldLines.slice(oldEnd), "context"),
  ];
}

/** Classic LCS table plus a forward walk over it. */
function lcsDiff(oldLines: string[], newLines: string[]): DiffLine[] {
  const n = oldLines.length;
  const m = newLines.length;
  if (n === 0) return asLines(newLines, "added");
  if (m === 0) return asLines(oldLines, "removed");

  // table[i][j] = length of the LCS of oldLines[i..] and newLines[j..],
  // flattened into one typed array of stride (m + 1).
  const stride = m + 1;
  const table = new Int32Array((n + 1) * stride);
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      table[i * stride + j] =
        oldLines[i] === newLines[j]
          ? table[(i + 1) * stride + j + 1] + 1
          : Math.max(table[(i + 1) * stride + j], table[i * stride + j + 1]);
    }
  }

  const rows: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (oldLines[i] === newLines[j]) {
      rows.push({ kind: "context", text: oldLines[i] });
      i++;
      j++;
    } else if (table[(i + 1) * stride + j] >= table[i * stride + j + 1]) {
      rows.push({ kind: "removed", text: oldLines[i] });
      i++;
    } else {
      rows.push({ kind: "added", text: newLines[j] });
      j++;
    }
  }
  while (i < n) rows.push({ kind: "removed", text: oldLines[i++] });
  while (j < m) rows.push({ kind: "added", text: newLines[j++] });
  return rows;
}

/** True when a diff contains no additions or removals at all. */
export function isUnchanged(lines: DiffLine[]): boolean {
  return lines.every((line) => line.kind === "context");
}

/**
 * Collapse unchanged runs, keeping `contextLines` of them on each side of a
 * change. Anything hidden becomes a single `fold` row carrying its line count.
 */
export function foldContext(
  lines: DiffLine[],
  contextLines = DEFAULT_CONTEXT_LINES,
): DiffRow[] {
  const keep = new Array<boolean>(lines.length).fill(false);
  for (let i = 0; i < lines.length; i++) {
    if (lines[i].kind === "context") continue;
    keep[i] = true;
    for (let d = 1; d <= contextLines; d++) {
      if (i - d >= 0) keep[i - d] = true;
      if (i + d < lines.length) keep[i + d] = true;
    }
  }

  const rows: DiffRow[] = [];
  let run = 0;
  let runStart = 0;
  const flush = () => {
    if (run === 0) return;
    if (run >= MIN_FOLD_LINES) {
      rows.push({ kind: "fold", count: run });
    } else {
      // Too short to be worth hiding: show the lines instead of a divider
      // that saves nothing.
      rows.push(...lines.slice(runStart, runStart + run));
    }
    run = 0;
  };
  for (let i = 0; i < lines.length; i++) {
    if (keep[i]) {
      flush();
      rows.push(lines[i]);
    } else {
      if (run === 0) runStart = i;
      run++;
    }
  }
  flush();
  return rows;
}
