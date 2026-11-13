import { describe, expect, test } from "bun:test";
import {
  FUZZY_MIN_TERM,
  LOCAL_HIT_SERVER_THRESHOLD,
  LOCAL_SCORE_CEIL,
  LOCAL_SCORE_FLOOR,
  CURRENT_PROJECT_REF_SCORE,
  EXACT_REF_SCORE,
  PREFIX_MIN_TERM,
  QUALITY,
  SERVER_SCORE_MAX,
  boundedEditDistance,
  dedupeByIdentifier,
  dedupeByKey,
  highlightSegments,
  isStaleSearch,
  localScoreToPaletteScore,
  matchQuality,
  matchRanges,
  parseRefQuery,
  projectCatalogChanged,
  preserveSelection,
  refIdentifier,
  refNumber,
  scoreDoc,
  searchLocalDocs,
  searchLocalDocsPerKind,
  tokenize,
  type PaletteDoc,
} from "../src/lib/paletteSearch";

let nextId = 1;

function doc(over: Partial<PaletteDoc> = {}): PaletteDoc {
  const id = over.id ?? nextId++;
  return {
    kind: "issue",
    id,
    identifier: `LIF-${id}`,
    title: "Untitled",
    labels: [],
    preview: "",
    updated_at: "2026-01-01T00:00:00Z",
    ...over,
  };
}

describe("tokenize", () => {
  test("splits on whitespace and lowercases", () => {
    expect(tokenize("  Warm  Read Model ")).toEqual(["warm", "read", "model"]);
  });

  test("keeps punctuation attached so a reference stays one term", () => {
    expect(tokenize("LIF-445")).toEqual(["lif-445"]);
  });

  test("an empty query yields no terms", () => {
    expect(tokenize("   ")).toEqual([]);
  });
});

describe("project catalog freshness", () => {
  const cached = [
    { id: 1, identifier: "LIF" },
    { id: 2, identifier: "OPS" },
  ];

  test("keeps metadata for the same project identities regardless of order", () => {
    expect(projectCatalogChanged(cached, [...cached].reverse())).toBe(false);
  });

  test("invalidates metadata when projects are added, removed, or renamed", () => {
    expect(projectCatalogChanged(cached, [...cached, { id: 3, identifier: "WEB" }])).toBe(true);
    expect(projectCatalogChanged(cached, [cached[0]])).toBe(true);
    expect(projectCatalogChanged(cached, [{ ...cached[0], identifier: "LIF2" }, cached[1]])).toBe(
      true,
    );
  });
});

describe("matchQuality", () => {
  test("an exact field beats a prefix beats a word prefix beats a substring", () => {
    expect(matchQuality("palette", "palette")).toBe(QUALITY.exact);
    expect(matchQuality("pal", "palette search")).toBe(QUALITY.prefix);
    expect(matchQuality("sea", "palette search")).toBe(QUALITY.wordPrefix);
    expect(matchQuality("ear", "palette search")).toBe(QUALITY.substring);
  });

  test(`prefix credit needs ${PREFIX_MIN_TERM} characters`, () => {
    expect(matchQuality("p", "palette")).toBe(QUALITY.substring);
    expect(matchQuality("pa", "palette")).toBe(QUALITY.prefix);
  });

  test("a one-character term still matches exactly", () => {
    expect(matchQuality("p", "p")).toBe(QUALITY.exact);
  });

  test(`typo tolerance starts at ${FUZZY_MIN_TERM} characters`, () => {
    // 4 chars, one substitution.
    expect(matchQuality("wark", "warm read model")).toBe(QUALITY.fuzzy);
    // 3 chars: "bug" must not reach "big", or short queries match everything.
    expect(matchQuality("bug", "big picture")).toBe(0);
  });

  test("tolerates one typo per five characters", () => {
    expect(matchQuality("comand", "command palette")).toBe(QUALITY.fuzzy);
    expect(matchQuality("paltte", "command palette")).toBe(QUALITY.fuzzy);
    // Two edits in a six-character term is past the 0.2 budget.
    expect(matchQuality("cxmxnd", "command palette")).toBe(0);
  });

  test("fuzzy can be refused, for numeric fields", () => {
    expect(matchQuality("4451", "445", false)).toBe(0);
    expect(matchQuality("4451", "445", true)).toBe(QUALITY.fuzzy);
  });

  test("no match is zero, and empty inputs are zero", () => {
    expect(matchQuality("zzz", "palette")).toBe(0);
    expect(matchQuality("", "palette")).toBe(0);
    expect(matchQuality("palette", "")).toBe(0);
  });
});

describe("boundedEditDistance", () => {
  test("measures edits up to the budget", () => {
    expect(boundedEditDistance("kitten", "kitten", 3)).toBe(0);
    expect(boundedEditDistance("kitten", "sitten", 3)).toBe(1);
    expect(boundedEditDistance("kitten", "sitting", 3)).toBe(3);
  });

  test("abandons once the budget cannot be met", () => {
    expect(boundedEditDistance("kitten", "sitting", 1)).toBeGreaterThan(1);
    expect(boundedEditDistance("abc", "abcdefghij", 2)).toBeGreaterThan(2);
  });
});

describe("refNumber", () => {
  test("takes the trailing number of a reference", () => {
    expect(refNumber("LIF-445")).toBe("445");
    expect(refNumber("LIF-DOC-3")).toBe("3");
    expect(refNumber("LIF")).toBe("");
  });
});

describe("scoreDoc weighting", () => {
  test("a title hit outranks the same hit in a label or a preview", () => {
    const title = doc({ title: "Palette search" });
    const label = doc({ title: "Something else", labels: ["palette"] });
    const preview = doc({ title: "Something else", preview: "the palette is warm" });

    const t = scoreDoc(["palette"], title);
    const l = scoreDoc(["palette"], label);
    const p = scoreDoc(["palette"], preview);

    expect(t).toBeGreaterThan(l);
    expect(l).toBeGreaterThan(p);
    expect(p).toBeGreaterThan(0);
  });

  test("an exact reference scores the reference weight, not the title weight", () => {
    const d = doc({ id: 445, identifier: "LIF-445", title: "Warm the palette" });
    // ref weight 4 / max weight 5.
    expect(scoreDoc(["lif-445"], d)).toBeCloseTo(4 / 5, 10);
  });

  test("a bare number matches the reference's number field", () => {
    const d = doc({ id: 445, identifier: "LIF-445", title: "Warm the palette" });
    // number weight 4 / max weight 5.
    expect(scoreDoc(["445"], d)).toBeCloseTo(4 / 5, 10);
    expect(scoreDoc(["446"], d)).toBe(0);
  });

  test("an exact title match is the ceiling", () => {
    expect(scoreDoc(["palette"], doc({ title: "Palette" }))).toBe(1);
  });

  test("no terms scores nothing", () => {
    expect(scoreDoc([], doc({ title: "Palette" }))).toBe(0);
  });
});

describe("scoreDoc multi-term AND", () => {
  test("every term must land somewhere", () => {
    const d = doc({ title: "Warm read model", labels: ["sync"] });
    expect(scoreDoc(["warm", "sync"], d)).toBeGreaterThan(0);
    expect(scoreDoc(["warm", "kanban"], d)).toBe(0);
  });

  test("terms may match different fields", () => {
    const d = doc({
      id: 445,
      identifier: "LIF-445",
      title: "Warm the palette",
      labels: ["frontend"],
      preview: "local search over the read model",
    });
    expect(scoreDoc(["445", "frontend", "local"], d)).toBeGreaterThan(0);
  });

  test("adding a term narrows rather than widens", () => {
    const docs = [
      doc({ title: "Warm read model", labels: ["sync"] }),
      doc({ title: "Warm the palette", labels: ["frontend"] }),
    ];
    expect(searchLocalDocs("warm", docs)).toHaveLength(2);
    expect(searchLocalDocs("warm sync", docs)).toHaveLength(1);
  });
});

describe("searchLocalDocs", () => {
  test("ranks stronger matches first", () => {
    const docs = [
      doc({ title: "Unrelated", preview: "mentions the palette in passing" }),
      doc({ title: "Palette search" }),
      doc({ title: "Rebuild the palette" }),
    ];
    const titles = searchLocalDocs("palette", docs).map((h) => h.doc.title);
    expect(titles).toEqual(["Palette search", "Rebuild the palette", "Unrelated"]);
  });

  test("breaks ties on updated_at, newest first", () => {
    const older = doc({ title: "Palette", updated_at: "2026-01-01T00:00:00Z" });
    const newer = doc({ title: "Palette", updated_at: "2026-06-01T00:00:00Z" });
    const hits = searchLocalDocs("palette", [older, newer]);
    expect(hits.map((h) => h.doc.id)).toEqual([newer.id, older.id]);
    expect(hits[0].score).toBe(hits[1].score);
  });

  test("finds a typo'd title and a prefix alike", () => {
    const docs = [doc({ title: "Command palette" })];
    expect(searchLocalDocs("comm", docs)).toHaveLength(1);
    expect(searchLocalDocs("palete", docs)).toHaveLength(1);
    expect(searchLocalDocs("kanban", docs)).toHaveLength(0);
  });

  test("searches issues and pages together", () => {
    const docs: PaletteDoc[] = [
      doc({ kind: "issue", title: "Palette search" }),
      doc({ kind: "page", identifier: "LIF-DOC-3", title: "Palette design" }),
    ];
    expect(searchLocalDocs("palette", docs).map((h) => h.doc.kind).sort()).toEqual([
      "issue",
      "page",
    ]);
  });

  test("honours the limit and returns nothing for an empty query", () => {
    const docs = Array.from({ length: 30 }, () => doc({ title: "Palette" }));
    expect(searchLocalDocs("palette", docs, 8)).toHaveLength(8);
    expect(searchLocalDocs("palette", docs, 0)).toHaveLength(30);
    expect(searchLocalDocs("  ", docs)).toHaveLength(0);
  });
});

describe("score bands", () => {
  test("every local hit outranks every server hit", () => {
    expect(LOCAL_SCORE_FLOOR).toBeGreaterThan(SERVER_SCORE_MAX);
    expect(localScoreToPaletteScore(Number.MIN_VALUE)).toBeGreaterThan(SERVER_SCORE_MAX);
  });

  test("no local hit reaches the identifier fast path or an exact project", () => {
    // identifierHits score 3, exact catalog project match 2.6.
    expect(LOCAL_SCORE_CEIL).toBeLessThan(2.6);
    expect(localScoreToPaletteScore(1)).toBe(LOCAL_SCORE_CEIL);
  });

  test("the current project's reference outranks every other result", () => {
    expect(CURRENT_PROJECT_REF_SCORE).toBeGreaterThan(EXACT_REF_SCORE);
    expect(EXACT_REF_SCORE).toBeGreaterThan(LOCAL_SCORE_CEIL);
    expect(EXACT_REF_SCORE).toBeGreaterThan(2.6);
  });

  test("the mapping is monotonic and clamped", () => {
    expect(localScoreToPaletteScore(0.5)).toBeGreaterThan(localScoreToPaletteScore(0.25));
    expect(localScoreToPaletteScore(-1)).toBe(LOCAL_SCORE_FLOOR);
    expect(localScoreToPaletteScore(4)).toBe(LOCAL_SCORE_CEIL);
  });

  test("the server is consulted only when the local answer is thin", () => {
    const docs = Array.from({ length: 6 }, () => doc({ title: "Palette" }));
    const many = searchLocalDocs("palette", docs);
    const few = searchLocalDocs("palette", docs.slice(0, 2));
    expect(many.length < LOCAL_HIT_SERVER_THRESHOLD).toBe(false);
    expect(few.length < LOCAL_HIT_SERVER_THRESHOLD).toBe(true);
  });
});

describe("parseRefQuery", () => {
  test("a bare or hashed number leaves the project to the caller", () => {
    expect(parseRefQuery("34")).toEqual({ kind: "issue", project: null, n: 34 });
    expect(parseRefQuery(" #34 ")).toEqual({ kind: "issue", project: null, n: 34 });
  });

  test("doc n names a page in the current project", () => {
    expect(parseRefQuery("doc 3")).toEqual({ kind: "page", project: null, n: 3 });
    expect(parseRefQuery("DOC-3")).toEqual({ kind: "page", project: null, n: 3 });
  });

  test("qualified issue references in every spelling", () => {
    for (const q of ["FIC34", "fic 34", "FIC-34", "fic-034"]) {
      expect(parseRefQuery(q)).toEqual({ kind: "issue", project: expect.stringMatching(/^fic$/i), n: 34 });
    }
  });

  test("qualified page references in every spelling", () => {
    for (const q of ["lif doc 3", "LIF-DOC-3", "lifdoc3"]) {
      expect(parseRefQuery(q)).toEqual({ kind: "page", project: expect.stringMatching(/^lif$/i), n: 3 });
    }
  });

  test("ordinary text is not a reference", () => {
    for (const q of ["", "status", "fix 34 bugs", "34 bugs", "#", "3.4"]) {
      expect(parseRefQuery(q)).toBeNull();
    }
  });

  test("refIdentifier builds the canonical identifier", () => {
    expect(refIdentifier("FIC", { kind: "issue", project: null, n: 34 })).toBe("FIC-34");
    expect(refIdentifier("LIF", { kind: "page", project: "lif", n: 3 })).toBe("LIF-DOC-3");
  });
});

describe("dedupeByIdentifier", () => {
  test("drops server hits the local pass already produced", () => {
    const server = [
      { identifier: "LIF-1", title: "one" },
      { identifier: "LIF-2", title: "two" },
    ];
    expect(dedupeByIdentifier(server, ["LIF-1"]).map((h) => h.identifier)).toEqual([
      "LIF-2",
    ]);
  });

  test("compares identifiers case-insensitively", () => {
    const server = [{ identifier: "lif-1", title: "one" }];
    expect(dedupeByIdentifier(server, ["LIF-1"])).toHaveLength(0);
  });

  test("collapses duplicates inside the server list too", () => {
    const server = [
      { identifier: "LIF-1", title: "one" },
      { identifier: "LIF-1", title: "one again" },
    ];
    expect(dedupeByIdentifier(server, [])).toHaveLength(1);
  });

  test("keeps hits with no identifier, and ignores empty taken entries", () => {
    const server = [{ identifier: null, title: "a page" }];
    expect(dedupeByIdentifier(server, [undefined, null, ""])).toHaveLength(1);
  });

  test("preserves the server's own order", () => {
    const server = [
      { identifier: "LIF-9", title: "nine" },
      { identifier: "LIF-3", title: "three" },
      { identifier: "LIF-7", title: "seven" },
    ];
    expect(dedupeByIdentifier(server, ["LIF-3"]).map((h) => h.identifier)).toEqual([
      "LIF-9",
      "LIF-7",
    ]);
  });
});

// The palette's publish step, reproduced: sort by score, dedupe on the
// `{#each}` key, then cap per kind. Kept in lockstep with
// CommandPalette.svelte's `publish()` / `resultKey()`.
interface Row {
  kind: "issue" | "page" | "project";
  title: string;
  identifier?: string;
  route: string;
  score: number;
}

const rowKey = (r: Row) => r.route + (r.identifier ?? r.title);

function publish(merged: Row[], groupCap = 8): Row[] {
  const sorted = [...merged].sort((a, b) => b.score - a.score);
  const counts = new Map<string, number>();
  return dedupeByKey(sorted, rowKey).filter((r) => {
    const c = counts.get(r.kind) ?? 0;
    if (c >= groupCap) return false;
    counts.set(r.kind, c + 1);
    return true;
  });
}

describe("dedupeByKey", () => {
  test("keeps the first occurrence, in order", () => {
    const items = ["a", "b", "a", "c", "b"];
    expect(dedupeByKey(items, (s) => s)).toEqual(["a", "b", "c"]);
  });

  test("an empty list stays empty", () => {
    expect(dedupeByKey([], (s: string) => s)).toEqual([]);
  });
});

describe("merged result set", () => {
  const exactRef: Row = {
    kind: "issue",
    title: "Warm the palette",
    identifier: "LIF-445",
    route: "/LIF/issues/LIF-445",
    score: 3,
  };
  const localSameIssue: Row = {
    kind: "issue",
    title: "Warm the palette",
    identifier: "LIF-445",
    route: "/LIF/issues/LIF-445",
    score: localScoreToPaletteScore(0.8),
  };
  const serverSameIssue: Row = {
    kind: "issue",
    title: "Warm the palette",
    identifier: "LIF-445",
    route: "/LIF/issues/LIF-445",
    score: 1,
  };

  test("the identifier fast path and the local hit never emit two rows", () => {
    const out = publish([exactRef, localSameIssue]);
    expect(out).toHaveLength(1);
    expect(new Set(out.map(rowKey)).size).toBe(out.length);
  });

  test("the exact reference is the row that survives", () => {
    expect(publish([localSameIssue, exactRef])[0].score).toBe(3);
    expect(publish([exactRef, localSameIssue])[0].score).toBe(3);
  });

  test("a server hit for a row already shown locally collapses too", () => {
    const out = publish([exactRef, localSameIssue, serverSameIssue]);
    expect(out).toHaveLength(1);
  });

  test("every key in a published set is unique", () => {
    const merged: Row[] = [
      exactRef,
      localSameIssue,
      serverSameIssue,
      { kind: "issue", title: "Other", identifier: "LIF-1", route: "/LIF/issues/LIF-1", score: 2 },
      { kind: "page", title: "Design", identifier: "LIF-DOC-3", route: "/LIF/pages/3", score: 2.4 },
      { kind: "page", title: "Design", identifier: "LIF-DOC-3", route: "/LIF/pages/3", score: 0.9 },
      { kind: "project", title: "Lific", identifier: "LIF", route: "/LIF/overview", score: 2.6 },
    ];
    const out = publish(merged);
    expect(new Set(out.map(rowKey)).size).toBe(out.length);
    expect(out).toHaveLength(4);
  });

  test("local rows still outrank server rows after deduping", () => {
    const local: Row = {
      kind: "issue",
      title: "Local",
      identifier: "LIF-2",
      route: "/LIF/issues/LIF-2",
      score: localScoreToPaletteScore(0.2),
    };
    const server: Row = {
      kind: "issue",
      title: "Server",
      identifier: "LIF-3",
      route: "/LIF/issues/LIF-3",
      score: SERVER_SCORE_MAX,
    };
    expect(publish([server, local]).map((r) => r.title)).toEqual(["Local", "Server"]);
  });
});

describe("searchLocalDocsPerKind", () => {
  test("a page survives a flood of stronger issue matches", () => {
    const issues = Array.from({ length: 40 }, () =>
      doc({ kind: "issue", title: "Palette work" }),
    );
    const page = doc({
      kind: "page",
      identifier: "LIF-DOC-3",
      title: "Notes mentioning palette somewhere",
    });

    // The old shared budget: pages never even get looked at.
    expect(searchLocalDocs("palette", [...issues, page], 8).some((h) => h.doc.kind === "page"))
      .toBe(false);

    const perKind = searchLocalDocsPerKind("palette", [...issues, page], 8);
    expect(perKind.some((h) => h.doc.id === page.id)).toBe(true);
  });

  test("caps each kind independently", () => {
    const docs = [
      ...Array.from({ length: 20 }, () => doc({ kind: "issue", title: "Palette" })),
      ...Array.from({ length: 20 }, () => doc({ kind: "page", title: "Palette" })),
    ];
    const hits = searchLocalDocsPerKind("palette", docs, 8);
    expect(hits.filter((h) => h.doc.kind === "issue")).toHaveLength(8);
    expect(hits.filter((h) => h.doc.kind === "page")).toHaveLength(8);
  });

  test("still ranks the combined set by score", () => {
    const docs = [
      doc({ kind: "issue", title: "Mentions palette midway" }),
      doc({ kind: "page", title: "Palette" }),
    ];
    expect(searchLocalDocsPerKind("palette", docs, 8)[0].doc.kind).toBe("page");
  });

  test("an empty query yields nothing", () => {
    expect(searchLocalDocsPerKind("  ", [doc({ title: "Palette" })])).toEqual([]);
  });
});

describe("isStaleSearch", () => {
  const issued = { gen: 7, projectIdent: "LIF" };

  test("a response for the current generation and project is fresh", () => {
    expect(isStaleSearch(issued, { gen: 7, projectIdent: "LIF" })).toBe(false);
  });

  test("a newer keystroke, mode change or close invalidates it", () => {
    expect(isStaleSearch(issued, { gen: 8, projectIdent: "LIF" })).toBe(true);
  });

  test("navigating to another project invalidates it", () => {
    expect(isStaleSearch(issued, { gen: 7, projectIdent: "OMN" })).toBe(true);
  });

  test("leaving a project entirely invalidates it", () => {
    expect(isStaleSearch(issued, { gen: 7, projectIdent: null })).toBe(true);
    expect(isStaleSearch({ gen: 7, projectIdent: null }, { gen: 7, projectIdent: "LIF" })).toBe(
      true,
    );
  });

  test("two responses racing: only the last one issued may land", () => {
    // Both were dispatched, then a third keystroke bumped the counter.
    const first = { gen: 1, projectIdent: "LIF" };
    const second = { gen: 2, projectIdent: "LIF" };
    const now = { gen: 2, projectIdent: "LIF" };
    expect(isStaleSearch(first, now)).toBe(true);
    expect(isStaleSearch(second, now)).toBe(false);
  });
});

describe("preserveSelection", () => {
  test("follows the selected item when a merge reorders the list", () => {
    const before = ["n:a", "n:b", "n:c"];
    const after = ["n:x", "n:a", "n:b", "n:c"];
    expect(preserveSelection(before[1], after, 1)).toBe(2);
  });

  test("keeps the cursor put when the server appends below it", () => {
    const before = ["n:a", "n:b"];
    const after = ["n:a", "n:b", "n:server1", "n:server2"];
    expect(preserveSelection(before[0], after, 0)).toBe(0);
  });

  test("falls back to the clamped index when the item is gone", () => {
    expect(preserveSelection("n:gone", ["n:a", "n:b"], 5)).toBe(1);
    expect(preserveSelection("n:gone", ["n:a", "n:b"], 1)).toBe(1);
    expect(preserveSelection(null, ["n:a", "n:b"], 0)).toBe(0);
  });

  test("an empty list selects nothing", () => {
    expect(preserveSelection("n:a", [], 3)).toBe(0);
  });
});

describe("matchRanges", () => {
  test("marks every literal occurrence of each term, case-insensitively", () => {
    expect(matchRanges("retry", "Retry the retry limit")).toEqual([[0, 5], [10, 15]]);
    expect(matchRanges("add limits", "Add retry limits")).toEqual([[0, 3], [10, 16]]);
  });

  test("merges overlapping and adjacent term hits", () => {
    expect(matchRanges("web hook", "webhook sender")).toEqual([[0, 7]]);
  });

  test("a one-character term marks only its first occurrence", () => {
    expect(matchRanges("e", "Deep sleep")).toEqual([[1, 2]]);
  });

  test("falls back to subsequence positions for a one-word query", () => {
    expect(matchRanges("rtb", "Retry backoff")).toEqual([[0, 1], [2, 3], [6, 7]]);
  });

  test("highlights nothing for typo matches or empty input", () => {
    expect(matchRanges("retyr", "Retry")).toEqual([]);
    expect(matchRanges("", "Retry")).toEqual([]);
    expect(matchRanges("retry", "")).toEqual([]);
  });
});

describe("highlightSegments", () => {
  test("splits text into plain and matched runs", () => {
    expect(highlightSegments("Add retry limits", [[4, 9]])).toEqual([
      { text: "Add ", hit: false },
      { text: "retry", hit: true },
      { text: " limits", hit: false },
    ]);
  });

  test("returns the whole text as one plain run without ranges", () => {
    expect(highlightSegments("Retry", [])).toEqual([{ text: "Retry", hit: false }]);
  });
});
