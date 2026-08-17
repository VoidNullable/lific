// LIF-418: unified-diff parsing for the .patch/.diff attachment viewer.
//
// Small on purpose. This parses what `git format-patch`, `git diff`, and
// `diff -u` emit, well enough to color the lines and count the damage. It is
// not a patch applier and it does not need to round-trip: the raw text is
// always one copy button away.

export type DiffLineKind = "add" | "del" | "context" | "hunk" | "meta";

export interface DiffLine {
  kind: DiffLineKind;
  /** Line content with the leading +/-/space marker removed (kept for hunk
   *  headers and metadata, which have no marker). */
  text: string;
  /** 1-based line number in the pre-image, or null for added lines. */
  oldNo: number | null;
  /** 1-based line number in the post-image, or null for removed lines. */
  newNo: number | null;
}

export interface DiffFile {
  /** Path as it appears in the `---` header, with the a/ prefix stripped. */
  oldPath: string;
  /** Path as it appears in the `+++` header, with the b/ prefix stripped. */
  newPath: string;
  /** What to show in the file header: "path" normally, "old → new" on rename. */
  display: string;
  additions: number;
  deletions: number;
  /** Git reported the contents as binary, so there are no hunk lines. */
  binary: boolean;
  lines: DiffLine[];
}

export interface ParsedDiff {
  files: DiffFile[];
  additions: number;
  deletions: number;
}

const HUNK_RE = /^@@+ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;

/** Strip the conventional `a/` / `b/` prefix git puts on diff paths, leaving
 *  `/dev/null` alone (it marks an add or a delete). */
function cleanPath(raw: string): string {
  const path = raw.trim().replace(/\t.*$/, "");
  if (path === "/dev/null") return path;
  return path.replace(/^[ab]\//, "");
}

function displayFor(oldPath: string, newPath: string): string {
  if (oldPath === "/dev/null") return newPath;
  if (newPath === "/dev/null") return oldPath;
  if (oldPath && newPath && oldPath !== newPath) return `${oldPath} -> ${newPath}`;
  return newPath || oldPath || "(unknown file)";
}

function emptyFile(): DiffFile {
  return {
    oldPath: "",
    newPath: "",
    display: "(unknown file)",
    additions: 0,
    deletions: 0,
    binary: false,
    lines: [],
  };
}

/**
 * Parse a unified diff into per-file line lists.
 *
 * Text before the first file header (a `git format-patch` cover letter, a
 * commit message, an email preamble) is discarded: it is prose, not a diff,
 * and the raw copy button still hands over the original bytes.
 */
export function parseUnifiedDiff(text: string): ParsedDiff {
  const files: DiffFile[] = [];
  let current: DiffFile | null = null;
  let oldNo = 0;
  let newNo = 0;

  /** Open a new file record. Callers assign the result to `current`
   *  themselves rather than having this do it, so the narrowing below stays
   *  visible to the type checker. */
  const startFile = (): DiffFile => {
    const file = emptyFile();
    files.push(file);
    oldNo = 0;
    newNo = 0;
    return file;
  };

  const lines = text.split("\n");
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i].replace(/\r$/, "");

    if (line.startsWith("diff --git ") || line.startsWith("diff -")) {
      const file = startFile();
      current = file;
      file.lines.push({ kind: "meta", text: line, oldNo: null, newNo: null });
      // `diff --git a/x b/y` already names both sides; the ---/+++ headers
      // that usually follow will overwrite these, but a pure-mode-change or
      // binary diff has none.
      const m = line.match(/^diff --git (\S+) (\S+)$/);
      if (m) {
        file.oldPath = cleanPath(m[1]);
        file.newPath = cleanPath(m[2]);
        file.display = displayFor(file.oldPath, file.newPath);
      }
      continue;
    }

    if (line.startsWith("--- ") && lines[i + 1]?.startsWith("+++ ")) {
      // A bare `diff -u` has no `diff --git` line, so the ---/+++ pair is the
      // only file boundary available. Only open a new file when the current
      // one already has hunks (otherwise this is the header of the file the
      // `diff --git` line just opened).
      let file: DiffFile;
      if (current && current.lines.every((l) => l.kind === "meta")) {
        file = current;
      } else {
        file = startFile();
        current = file;
      }
      file.oldPath = cleanPath(line.slice(4));
      file.newPath = cleanPath(lines[i + 1].slice(4));
      file.display = displayFor(file.oldPath, file.newPath);
      file.lines.push({ kind: "meta", text: line, oldNo: null, newNo: null });
      file.lines.push({ kind: "meta", text: lines[i + 1], oldNo: null, newNo: null });
      i += 1;
      continue;
    }

    if (!current) {
      // Preamble before any file header.
      if (!HUNK_RE.test(line)) continue;
      current = startFile();
    }
    const file = current;

    const hunk = line.match(HUNK_RE);
    if (hunk) {
      oldNo = Number(hunk[1]);
      newNo = Number(hunk[3]);
      file.lines.push({ kind: "hunk", text: line, oldNo: null, newNo: null });
      continue;
    }

    if (/^(index |old mode |new mode |new file mode |deleted file mode |similarity index |rename |copy |GIT binary patch|Binary files )/.test(line)) {
      if (line.startsWith("GIT binary patch") || line.startsWith("Binary files")) {
        file.binary = true;
      }
      file.lines.push({ kind: "meta", text: line, oldNo: null, newNo: null });
      continue;
    }

    if (line.startsWith("+")) {
      file.lines.push({ kind: "add", text: line.slice(1), oldNo: null, newNo });
      file.additions += 1;
      newNo += 1;
      continue;
    }
    if (line.startsWith("-")) {
      file.lines.push({ kind: "del", text: line.slice(1), oldNo, newNo: null });
      file.deletions += 1;
      oldNo += 1;
      continue;
    }
    if (line.startsWith("\\")) {
      // "\ No newline at end of file"
      file.lines.push({ kind: "meta", text: line, oldNo: null, newNo: null });
      continue;
    }
    if (line.startsWith(" ") || line === "") {
      // A trailing empty line at the very end of the file is the split
      // artifact of a final newline, not a context line.
      if (line === "" && i === lines.length - 1) continue;
      file.lines.push({ kind: "context", text: line.slice(1), oldNo, newNo });
      oldNo += 1;
      newNo += 1;
      continue;
    }

    // Anything else between hunks (git trailers, `-- ` signature) is metadata.
    file.lines.push({ kind: "meta", text: line, oldNo: null, newNo: null });
  }

  return {
    files,
    additions: files.reduce((n, f) => n + f.additions, 0),
    deletions: files.reduce((n, f) => n + f.deletions, 0),
  };
}

/** "3 files changed, +42 -7" — the one-line header above the diff. */
export function summarizeDiff(diff: ParsedDiff): string {
  const count = diff.files.length;
  const noun = count === 1 ? "file" : "files";
  return `${count} ${noun} changed, +${diff.additions} -${diff.deletions}`;
}

/** Cheap sniff used by the dispatcher when a .txt attachment might actually be
 *  a patch. Requires a hunk header, which prose almost never contains. */
export function looksLikeDiff(text: string): boolean {
  return /^@@+ -\d+(?:,\d+)? \+\d+(?:,\d+)? @@/m.test(text);
}
