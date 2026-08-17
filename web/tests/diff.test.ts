import { describe, expect, test } from "bun:test";
import {
  looksLikeDiff,
  parseUnifiedDiff,
  summarizeDiff,
} from "../src/lib/attachments/viewers/diff";

const GIT_DIFF = `diff --git a/src/main.rs b/src/main.rs
index 1234567..89abcde 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -10,7 +10,8 @@ fn main() {
     let args = Args::parse();
-    println!("hello");
+    println!("hello, world");
+    println!("extra");
     run(args);
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@ -1,3 +1,2 @@
 # Lific
-old line
 tail
`;

describe("unified diff parsing", () => {
  test("splits files and counts each side", () => {
    const parsed = parseUnifiedDiff(GIT_DIFF);
    expect(parsed.files).toHaveLength(2);
    expect(parsed.files[0].display).toBe("src/main.rs");
    expect(parsed.files[0].additions).toBe(2);
    expect(parsed.files[0].deletions).toBe(1);
    expect(parsed.files[1].display).toBe("README.md");
    expect(parsed.files[1].additions).toBe(0);
    expect(parsed.files[1].deletions).toBe(1);
    expect(parsed.additions).toBe(2);
    expect(parsed.deletions).toBe(2);
  });

  test("numbers lines from the hunk header, per side", () => {
    const parsed = parseUnifiedDiff(GIT_DIFF);
    const body = parsed.files[0].lines.filter((l) => l.kind !== "meta" && l.kind !== "hunk");
    expect(body.map((l) => [l.kind, l.oldNo, l.newNo])).toEqual([
      ["context", 10, 10],
      ["del", 11, null],
      ["add", null, 11],
      ["add", null, 12],
      ["context", 12, 13],
    ]);
  });

  test("keeps the +/- marker out of the line text", () => {
    const parsed = parseUnifiedDiff(GIT_DIFF);
    const added = parsed.files[0].lines.find((l) => l.kind === "add");
    expect(added?.text).toBe('    println!("hello, world");');
  });

  test("summarizes with correct pluralization", () => {
    expect(summarizeDiff(parseUnifiedDiff(GIT_DIFF))).toBe("2 files changed, +2 -2");
    const single = parseUnifiedDiff(`--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n`);
    expect(summarizeDiff(single)).toBe("1 file changed, +1 -1");
  });

  test("handles a bare diff -u with no git header", () => {
    const parsed = parseUnifiedDiff(
      `--- one.txt\t2026-01-01\n+++ two.txt\t2026-01-02\n@@ -1,2 +1,2 @@\n keep\n-drop\n+take\n`,
    );
    expect(parsed.files).toHaveLength(1);
    expect(parsed.files[0].display).toBe("one.txt -> two.txt");
    expect(parsed.files[0].additions).toBe(1);
  });

  test("names new and deleted files from the surviving side", () => {
    const added = parseUnifiedDiff(
      `diff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+hi\n`,
    );
    expect(added.files[0].display).toBe("new.txt");
    const removed = parseUnifiedDiff(
      `diff --git a/gone.txt b/gone.txt\ndeleted file mode 100644\n--- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n`,
    );
    expect(removed.files[0].display).toBe("gone.txt");
  });

  test("flags binary files and gives them no hunk lines", () => {
    const parsed = parseUnifiedDiff(
      `diff --git a/logo.png b/logo.png\nindex 000..111 100644\nBinary files a/logo.png and b/logo.png differ\n`,
    );
    expect(parsed.files[0].binary).toBe(true);
    expect(parsed.files[0].additions).toBe(0);
  });

  test("discards a format-patch preamble", () => {
    const parsed = parseUnifiedDiff(
      `From abc Mon Sep 17 00:00:00 2001\nSubject: [PATCH] fix\n\nA commit message.\n\ndiff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-x\n+y\n`,
    );
    expect(parsed.files).toHaveLength(1);
    expect(parsed.files[0].display).toBe("a.txt");
  });

  test("an empty or prose-only document parses to nothing", () => {
    expect(parseUnifiedDiff("").files).toHaveLength(0);
    expect(parseUnifiedDiff("just some notes\n").files).toHaveLength(0);
  });

  test("sniffs a diff by its hunk header", () => {
    expect(looksLikeDiff(GIT_DIFF)).toBe(true);
    expect(looksLikeDiff("no hunks here @@ nope")).toBe(false);
  });
});
