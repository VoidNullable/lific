import { describe, expect, test } from "bun:test";
import { ChangeSet, Text } from "@codemirror/state";
import { applyEdits, diffText } from "../src/lib/editor/textDiff";
import {
  BodySync,
  changeSetBetween,
  type BodySaveResult,
  type SyncStatus,
} from "../src/lib/editor/bodySync";

describe("diffText", () => {
  const cases: [string, string][] = [
    ["", "hello"],
    ["hello", ""],
    ["same", "same"],
    ["a\nb\nc", "a\nB\nc"],
    ["a\nb\nc", "a\nc"],
    ["a\nc", "a\nb\nc"],
    ["a\nb", "a\nb\n"],
    ["a\nb\n", "a\nb"],
    ["one\ntwo\nthree\nfour\nfive", "ONE\ntwo\nthree\nfour\nFIVE"],
    ["x\ny\nz", "q\nx\ny\nz\nr"],
    ["# Title\n\nBody text here.\n\n- [ ] task", "# Title\n\nBody text there.\n\n- [x] task\n- [ ] more"],
    ["\n\n\n", "\n"],
    ["line\n", "\nline"],
  ];
  test.each(cases)("turns %j into %j", (a, b) => {
    expect(applyEdits(a, diffText(a, b))).toBe(b);
  });

  test("keeps two distant edits separate", () => {
    const a = ["alpha", "beta", "gamma", "delta", "epsilon"].join("\n");
    const b = ["alpha!", "beta", "gamma", "delta", "epsilon!"].join("\n");
    const edits = diffText(a, b);
    expect(edits).toHaveLength(2);
    expect(edits.every((e) => e.insert === "!" && e.from === e.to)).toBe(true);
  });

  test("random edits round-trip", () => {
    let seed = 7;
    const rand = (n: number) => {
      seed = (seed * 1103515245 + 12345) & 0x7fffffff;
      return seed % n;
    };
    const alphabet = "ab\n c";
    for (let round = 0; round < 400; round++) {
      let a = "";
      for (let i = rand(30); i > 0; i--) a += alphabet[rand(alphabet.length)];
      let b = a;
      for (let k = rand(4); k >= 0; k--) {
        const at = rand(b.length + 1);
        const del = rand(4);
        let ins = "";
        for (let i = rand(4); i > 0; i--) ins += alphabet[rand(alphabet.length)];
        b = b.slice(0, at) + ins + b.slice(at + del);
      }
      const edits = diffText(a, b);
      expect(applyEdits(a, edits)).toBe(b);
      for (let i = 1; i < edits.length; i++) expect(edits[i].from).toBeGreaterThan(edits[i - 1].to);
    }
  });
});

function applyTo(doc: string, changes: ChangeSet): string {
  return changes.apply(Text.of(doc.split("\n"))).toString();
}

/** A stand-in editor: a string the sync applies changes to, plus a save
 *  queue the test answers by hand and timers it fires by hand. */
function realHarness(initial: string, seq: number | null = 1) {
  let doc = initial;
  const saves: { text: string; expected: number | null; resolve: (r: BodySaveResult) => void }[] =
    [];
  const statuses: SyncStatus[] = [];
  const timers: (() => void)[] = [];
  let merged = 0;
  let undoable = 0;
  const sync = new BodySync(
    {
      doc: () => doc,
      apply: (changes, u) => {
        doc = applyTo(doc, changes);
        if (u) undoable++;
      },
      save: (text, expected) =>
        new Promise<BodySaveResult>((resolve) => saves.push({ text, expected, resolve })),
      onStatus: (s) => statuses.push(s),
      onMerged: () => merged++,
      setTimer: (fn) => {
        timers.push(fn);
        return timers.length - 1;
      },
      clearTimer: (hd) => {
        timers[hd as number] = () => {};
      },
    },
    initial,
    seq,
  );
  return {
    sync,
    saves,
    statuses,
    get doc() {
      return doc;
    },
    get merged() {
      return merged;
    },
    get undoable() {
      return undoable;
    },
    type(next: string) {
      const changes = changeSetBetween(doc, next);
      doc = next;
      sync.local(changes);
    },
    tick() {
      const run = timers.splice(0);
      for (const fn of run) fn();
    },
  };
}

const settle = () => new Promise((r) => setTimeout(r, 0));

describe("BodySync", () => {
  test("saves after a pause with the seq it last synced with", async () => {
    const h = realHarness("hello", 4);
    h.type("hello world");
    expect(h.sync.currentStatus).toBe("pending");
    expect(h.saves).toHaveLength(0);
    h.tick();
    expect(h.saves).toHaveLength(1);
    expect(h.saves[0]).toMatchObject({ text: "hello world", expected: 4 });
    h.saves[0].resolve({ ok: true, body: "hello world", seq: 9 });
    await settle();
    expect(h.sync.currentStatus).toBe("saved");
    expect(h.sync.dirty).toBe(false);
    h.type("hello world!");
    void h.sync.flush();
    expect(h.saves[1].expected).toBe(9);
  });

  test("applies a remote change to a clean editor as edits", () => {
    const h = realHarness("one\ntwo\nthree", 1);
    h.sync.remote("one\nTWO\nthree", 2);
    expect(h.doc).toBe("one\nTWO\nthree");
    expect(h.sync.dirty).toBe(false);
    expect(h.undoable).toBe(0);
  });

  test("ignores a remote echo of its own save and stale refreshes", async () => {
    const h = realHarness("a", 1);
    h.type("ab");
    const flushing = h.sync.flush();
    h.sync.remote("ab", 2); // the route's refresh lands while the save is open
    h.saves[0].resolve({ ok: true, body: "ab", seq: 2 });
    await flushing;
    h.sync.remote("a", 1); // an old refresh finishing late
    expect(h.doc).toBe("ab");
    expect(h.sync.currentStatus).toBe("saved");
  });

  test("merges a remote edit elsewhere into unsaved typing", () => {
    const h = realHarness("first line\nsecond line\nthird line", 1);
    h.type("first line, edited\nsecond line\nthird line");
    h.sync.remote("first line\nsecond line\nthird line by an agent", 2);
    expect(h.doc).toBe("first line, edited\nsecond line\nthird line by an agent");
    expect(h.merged).toBe(1);
    expect(h.sync.currentStatus).toBe("pending");
  });

  test("a merged save sends the merged text against the new seq", async () => {
    const h = realHarness("a\nb\nc", 1);
    h.type("a!\nb\nc");
    h.sync.remote("a\nb\nc?", 5);
    void h.sync.flush();
    expect(h.saves[0]).toMatchObject({ text: "a!\nb\nc?", expected: 5 });
  });

  test("overlapping edits stop and report a conflict", () => {
    const h = realHarness("status: draft", 1);
    h.type("status: ready");
    h.sync.remote("status: shipped", 2);
    expect(h.doc).toBe("status: ready");
    expect(h.sync.currentStatus).toBe("conflict");
    expect(h.sync.conflictText).toBe("status: shipped");
  });

  test("a 409 on save merges the newer body and saves again", async () => {
    const h = realHarness("a\nb\nc", 1);
    h.type("a\nb\nc\nd");
    const flushing = h.sync.flush();
    h.saves[0].resolve({ ok: false, conflict: { body: "A\nb\nc", seq: 3 } });
    await flushing;
    expect(h.doc).toBe("A\nb\nc\nd");
    expect(h.saves[1]).toMatchObject({ text: "A\nb\nc\nd", expected: 3 });
  });

  test("keeping mine overwrites; taking theirs replaces the text undoably", async () => {
    const mine = realHarness("x = 1", 1);
    mine.type("x = 2");
    mine.sync.remote("x = 3", 2);
    mine.sync.resolveConflict("mine");
    await settle();
    expect(mine.saves[0]).toMatchObject({ text: "x = 2", expected: 2 });

    const theirs = realHarness("x = 1", 1);
    theirs.type("x = 2");
    theirs.sync.remote("x = 3", 2);
    theirs.sync.resolveConflict("theirs");
    expect(theirs.doc).toBe("x = 3");
    expect(theirs.undoable).toBe(1);
    expect(theirs.sync.dirty).toBe(false);
    expect(theirs.sync.currentStatus).toBe("saved");
  });

  test("a failed save keeps the edits and retries", async () => {
    const h = realHarness("a", 1);
    h.type("ab");
    const first = h.sync.flush();
    h.saves[0].resolve({ ok: false, error: "offline" });
    await first;
    expect(h.sync.currentStatus).toBe("error");
    expect(h.sync.dirty).toBe(true);
    h.tick(); // the retry timer
    expect(h.saves[1]).toMatchObject({ text: "ab", expected: 1 });
  });

  test("typing during a save is sent afterwards", async () => {
    const h = realHarness("a", 1);
    h.type("ab");
    const first = h.sync.flush();
    h.type("abc");
    h.saves[0].resolve({ ok: true, body: "ab", seq: 2 });
    await first;
    expect(h.sync.currentStatus).toBe("pending");
    void h.sync.flush();
    expect(h.saves[1]).toMatchObject({ text: "abc", expected: 2 });
  });
});
