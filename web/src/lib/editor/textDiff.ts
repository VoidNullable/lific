// Minimal text diff for the body editor: turns "the body was A, now it is B"
// into the few edits that make that change, so a body changed elsewhere can be
// applied to an open editor as a transaction (the caret, the selection and the
// undo history survive) and merged with what the person is typing.
//
// Lines are matched first (an LCS over the lines that differ), then each
// changed run of lines is trimmed to the characters that actually differ. Two
// separate edits far apart therefore stay two small edits instead of one
// replacement spanning everything between them, which matters when merging:
// a local edit inside that span would otherwise collide with it.

export interface TextEdit {
  /** Start offset in the old text. */
  from: number;
  /** End offset in the old text (exclusive). */
  to: number;
  /** Text that replaces [from, to). */
  insert: string;
}

/** LCS table cells we allow before falling back to one coarse edit. */
const MAX_CELLS = 2_000_000;

/** Offsets where each line starts, plus a final entry at `text.length`. */
function lineStarts(lines: string[]): number[] {
  const starts = [0];
  for (const line of lines) starts.push(starts[starts.length - 1] + line.length + 1);
  return starts;
}

/** One edit that trims the shared prefix and suffix of a and b. */
function trimmedEdit(a: string, b: string, offset: number): TextEdit | null {
  if (a === b) return null;
  let start = 0;
  const max = Math.min(a.length, b.length);
  while (start < max && a.charCodeAt(start) === b.charCodeAt(start)) start++;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a.charCodeAt(endA - 1) === b.charCodeAt(endB - 1)) {
    endA--;
    endB--;
  }
  return { from: offset + start, to: offset + endA, insert: b.slice(start, endB) };
}

/** The edits that turn `a` into `b`, in ascending order, non-overlapping. */
export function diffText(a: string, b: string): TextEdit[] {
  if (a === b) return [];
  const la = a.split("\n");
  const lb = b.split("\n");
  // Shared leading and trailing lines never take part in the table.
  let head = 0;
  while (head < la.length && head < lb.length && la[head] === lb[head]) head++;
  let tailA = la.length;
  let tailB = lb.length;
  while (tailA > head && tailB > head && la[tailA - 1] === lb[tailB - 1]) {
    tailA--;
    tailB--;
  }
  const startsA = lineStarts(la);
  const startsB = lineStarts(lb);

  const n = tailA - head;
  const m = tailB - head;
  // One block of change (or too big a table): trimming the shared prefix
  // and suffix of the whole text gives the single smallest edit.
  const coarse = () => {
    const edit = trimmedEdit(a, b, 0);
    return edit ? [edit] : [];
  };
  if (n === 0 || m === 0 || (n + 1) * (m + 1) > MAX_CELLS) return coarse();

  // Classic LCS over the middle lines, filled from the end so the walk below
  // can go forwards.
  const width = m + 1;
  const table = new Int32Array((n + 1) * width);
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      table[i * width + j] =
        la[head + i] === lb[head + j]
          ? table[(i + 1) * width + j + 1] + 1
          : Math.max(table[(i + 1) * width + j], table[i * width + j + 1]);
    }
  }

  const edits: TextEdit[] = [];
  let i = 0;
  let j = 0;
  // Collect runs of unmatched lines; each run becomes one edit over its
  // characters (newline included where the run touches a following line).
  let runA = -1;
  let runB = -1;
  const closeRun = () => {
    if (runA < 0) return;
    const fromLineA = head + runA;
    const toLineA = head + i;
    const fromLineB = head + runB;
    const toLineB = head + j;
    // Include the newline that ends the run, so whole lines are added or
    // removed with their separator. When a run reaches the end of the text
    // there is no trailing newline, so include the one before it instead.
    let fromA = startsA[fromLineA];
    let toA = startsA[toLineA];
    let fromB = startsB[fromLineB];
    let toB = startsB[toLineB];
    if (toA > a.length || toB > b.length) {
      fromA = Math.max(0, fromA - 1);
      fromB = Math.max(0, fromB - 1);
      toA = Math.min(toA, a.length);
      toB = Math.min(toB, b.length);
    }
    const edit = trimmedEdit(a.slice(fromA, toA), b.slice(fromB, toB), fromA);
    if (edit) edits.push(edit);
    runA = -1;
    runB = -1;
  };
  while (i < n || j < m) {
    if (i < n && j < m && la[head + i] === lb[head + j]) {
      closeRun();
      i++;
      j++;
      continue;
    }
    if (runA < 0) {
      runA = i;
      runB = j;
    }
    if (j >= m || (i < n && table[(i + 1) * width + j] >= table[i * width + j + 1])) i++;
    else j++;
  }
  closeRun();
  return mergeTouching(edits, a);
}

/** Runs that ended up touching (an edit's end equals the next one's start)
 *  are fused, so the result is strictly non-overlapping. */
function mergeTouching(edits: TextEdit[], a: string): TextEdit[] {
  const out: TextEdit[] = [];
  for (const edit of edits) {
    const last = out[out.length - 1];
    if (last && edit.from <= last.to) {
      last.insert += a.slice(last.to, edit.from) + edit.insert;
      last.to = Math.max(last.to, edit.to);
    } else {
      out.push({ ...edit });
    }
  }
  return out;
}

/** Apply edits produced by `diffText` to `a`. */
export function applyEdits(a: string, edits: TextEdit[]): string {
  let out = "";
  let pos = 0;
  for (const e of edits) {
    out += a.slice(pos, e.from) + e.insert;
    pos = e.to;
  }
  return out + a.slice(pos);
}
