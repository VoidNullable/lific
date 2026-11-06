import { describe, expect, test } from "bun:test";
import { parseStoredDiff, storedDiffStats } from "../src/lib/linediff";

// Shape produced by the server (similar's unified diff, no file header).
const TWO_HUNKS = `@@ -4,7 +4,7 @@
 line 4
 line 5
 line 6
-line 7
+line seven
 line 8
 line 9
 line 10
@@ -147,7 +147,6 @@
 line 147
 line 148
 line 149
-line 150
 line 151
 line 152
 line 153
`;

describe("stored page diffs", () => {
  test("hunk headers become folds counting the skipped lines", () => {
    const rows = parseStoredDiff(TWO_HUNKS);
    expect(rows[0]).toEqual({ kind: "fold", count: 3 });
    expect(rows).toContainEqual({ kind: "removed", text: "line 7" });
    expect(rows).toContainEqual({ kind: "added", text: "line seven" });
    // Lines 11 through 146 sit between the hunks.
    expect(rows).toContainEqual({ kind: "fold", count: 136 });
    expect(rows.at(-1)).toEqual({ kind: "context", text: "line 153" });
  });

  test("a hunk at the top of the page adds no fold", () => {
    const rows = parseStoredDiff("@@ -0,0 +1,2 @@\n+# Title\n+body\n");
    expect(rows).toEqual([
      { kind: "added", text: "# Title" },
      { kind: "added", text: "body" },
    ]);
  });

  test("the missing-newline marker is not rendered", () => {
    const rows = parseStoredDiff("@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n");
    expect(rows).toEqual([
      { kind: "removed", text: "old" },
      { kind: "added", text: "new" },
    ]);
  });

  test("stats count added and removed lines", () => {
    expect(storedDiffStats(TWO_HUNKS)).toEqual({ added: 1, removed: 2 });
  });
});
