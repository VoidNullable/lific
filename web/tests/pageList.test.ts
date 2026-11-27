import { describe, expect, test } from "bun:test";
import type { Folder, PageMeta } from "../src/lib/api";
import {
  applyFilters,
  bucketByDay,
  dayBucket,
  describeLength,
  describeLinks,
  descendantIds,
  draftAge,
  excerptOf,
  folderPathLabel,
  folderStats,
  groupPages,
  inScope,
  orderedFolders,
  quietPages,
  sortPages,
  type ListPage,
} from "../src/lib/pages/list";
import { parseView } from "../src/lib/pages/state.svelte";

const folder = (id: number, name: string, parent_id: number | null = null): Folder => ({
  id,
  name,
  parent_id,
  project_id: 1,
  sort_order: 0,
});

let seq = 0;
const page = (over: Partial<ListPage> = {}): ListPage => {
  seq += 1;
  return {
    id: seq,
    identifier: `LIF-DOC-${seq}`,
    title: `Page ${seq}`,
    status: "active",
    folder_id: null,
    pinned: false,
    created_at: "2026-11-01T10:00:00",
    updated_at: "2026-11-01T10:00:00",
    preview: "",
    labels: [],
    ...over,
  };
};

const meta = (id: number, over: Partial<PageMeta> = {}): PageMeta => ({
  id,
  excerpt: "",
  words: 0,
  created_by: null,
  updated_by: null,
  links: { issues: 0, pages: 0, comments: 0 },
  linked_from: [],
  ...over,
});

const person = (id: number, is_bot = false) => ({ id, username: `u${id}`, display_name: null, is_bot, tool_id: null });

const folders = [folder(1, "Architecture"), folder(2, "MCP", 1), folder(3, "Design"), folder(4, "Api", 1)];

describe("folders", () => {
  test("ordered depth-first with siblings by name", () => {
    expect(orderedFolders(folders).map((n) => `${n.depth}:${n.folder.name}`)).toEqual([
      "0:Architecture",
      "1:Api",
      "1:MCP",
      "0:Design",
    ]);
  });

  test("a folder whose parent is missing becomes a root", () => {
    expect(orderedFolders([folder(9, "Orphan", 99)]).map((n) => n.depth)).toEqual([0]);
  });

  test("descendants include the folder and everything under it", () => {
    expect([...descendantIds(folders, 1)].sort()).toEqual([1, 2, 4]);
    expect([...descendantIds(folders, 3)]).toEqual([3]);
  });

  test("path labels read root first", () => {
    expect(folderPathLabel(folders, 2)).toBe("Architecture / MCP");
  });

  test("stats count direct pages and roll totals up to ancestors", () => {
    const rows = [
      page({ folder_id: 2, updated_at: "2026-11-03T00:00:00" }),
      page({ folder_id: 1, updated_at: "2026-11-02T00:00:00" }),
      page({ folder_id: null }),
    ];
    const s = folderStats(rows, folders);
    expect(s.get(1)).toEqual({ direct: 1, total: 2, updated: "2026-11-03T00:00:00" });
    expect(s.get(2)).toEqual({ direct: 1, total: 1, updated: "2026-11-03T00:00:00" });
    expect(s.get(3)).toEqual({ direct: 0, total: 0, updated: null });
  });

  test("scope: a folder includes its subfolders", () => {
    expect(inScope(page({ folder_id: 2 }), 1, folders)).toBe(true);
    expect(inScope(page({ folder_id: 3 }), 1, folders)).toBe(false);
    expect(inScope(page({ folder_id: null }), 1, folders)).toBe(false);
    expect(inScope(page({ folder_id: 3 }), "all", folders)).toBe(true);
  });
});

describe("filters", () => {
  test("status, label, editor and unlinked narrow the list", () => {
    const a = page({ status: "draft", labels: ["spec"] });
    const b = page({ status: "active" });
    const c = page({ status: "complete", labels: ["spec"] });
    const m = new Map([
      [a.id, meta(a.id, { updated_by: person(1, true) })],
      [b.id, meta(b.id, { updated_by: person(2), links: { issues: 1, pages: 0, comments: 0 } })],
      [c.id, meta(c.id, { updated_by: person(2) })],
    ]);
    const rows = [a, b, c];
    const ids = (f: Parameters<typeof applyFilters>[1]) => applyFilters(rows, f, m).map((p) => p.id);
    const none = { statuses: [], label: "", editor: "", unlinked: false } as const;
    expect(ids({ ...none, statuses: ["draft", "complete"] })).toEqual([a.id, c.id]);
    expect(ids({ ...none, label: "spec" })).toEqual([a.id, c.id]);
    expect(ids({ ...none, editor: "agents" })).toEqual([a.id]);
    expect(ids({ ...none, editor: "people" })).toEqual([b.id, c.id]);
    expect(ids({ ...none, editor: "u:2" })).toEqual([b.id, c.id]);
    expect(ids({ ...none, unlinked: true })).toEqual([a.id, c.id]);
  });

  test("unlinked hides pages whose links have not loaded", () => {
    const a = page();
    expect(applyFilters([a], { statuses: [], label: "", editor: "", unlinked: true }, new Map())).toEqual([]);
  });
});

describe("sort and group", () => {
  test("sorts by edit time, title (natural) and status", () => {
    const a = page({ title: "Page 10", status: "complete", updated_at: "2026-11-02T00:00:00" });
    const b = page({ title: "page 9", status: "draft", updated_at: "2026-11-03T00:00:00" });
    expect(sortPages([a, b], "updated").map((p) => p.id)).toEqual([b.id, a.id]);
    expect(sortPages([a, b], "title").map((p) => p.id)).toEqual([b.id, a.id]);
    expect(sortPages([a, b], "status").map((p) => p.id)).toEqual([b.id, a.id]);
  });

  test("folder groups follow the tree, unfiled first, empty folders left out", () => {
    const rows = [page({ folder_id: 2 }), page({ folder_id: null }), page({ folder_id: 3 })];
    const groups = groupPages(rows, "folder", { folders });
    expect(groups.map((g) => g.label)).toEqual(["Unfiled", "Architecture / MCP", "Design"]);
  });

  test("the scope's own folder shows even when empty", () => {
    const groups = groupPages([page({ folder_id: 2 })], "folder", { folders, scopeFolder: 1 });
    expect(groups.map((g) => [g.label, g.pages.length, g.depth])).toEqual([
      ["Architecture", 0, 0],
      ["Architecture / MCP", 1, 1],
    ]);
  });

  test("edited groups bucket by day, newest first", () => {
    const now = new Date(2026, 10, 26, 15, 0);
    const today = page({ updated_at: new Date(2026, 10, 26, 9).toISOString().replace("Z", "") });
    const old = page({ updated_at: "2026-01-01T00:00:00" });
    const groups = groupPages([old, today], "edited", { folders, now });
    expect(groups.map((g) => [g.label, g.pages.map((p) => p.id)])).toEqual([
      ["Today", [today.id]],
      ["Older", [old.id]],
    ]);
  });

  test("label groups list a page under each label, then No label", () => {
    const a = page({ labels: ["b", "a"] });
    const b = page();
    const groups = groupPages([a, b], "label", { folders });
    expect(groups.map((g) => [g.label, g.pages.map((p) => p.id)])).toEqual([
      ["a", [a.id]],
      ["b", [a.id]],
      ["No label", [b.id]],
    ]);
  });
});

describe("time", () => {
  const now = new Date(2026, 10, 26, 15, 0);
  const local = (d: number, h = 12) => new Date(2026, 10, d, h).toISOString().replace("Z", "");

  test("day buckets follow the local calendar", () => {
    expect(dayBucket(local(26, 1), now)).toBe("Today");
    expect(dayBucket(local(25, 23), now)).toBe("Yesterday");
    expect(dayBucket(local(21), now)).toBe("Earlier this week");
    expect(dayBucket(local(2), now)).toBe("Earlier this month");
    expect(dayBucket("2026-01-01T00:00:00", now)).toBe("Older");
  });

  test("buckets keep their order and drop empty ones", () => {
    const rows = [{ updated_at: local(26) }, { updated_at: "2026-01-01T00:00:00" }, { updated_at: local(26, 2) }];
    expect(bucketByDay(rows, now).map((b) => [b.bucket, b.rows.length])).toEqual([
      ["Today", 2],
      ["Older", 1],
    ]);
  });

  test("draft age and quiet pages", () => {
    expect(draftAge(local(26, 9), now)).toBe("Started today");
    expect(draftAge(local(14), now)).toBe("Draft for 12 days");
    const old = page({ status: "active", updated_at: "2026-08-01T00:00:00" });
    const draft = page({ status: "draft", updated_at: "2026-08-01T00:00:00" });
    const fresh = page({ status: "active", updated_at: local(25) });
    expect(quietPages([old, draft, fresh], now).map((p) => p.id)).toEqual([old.id]);
  });
});

describe("text", () => {
  test("the server excerpt wins once loaded, even when empty", () => {
    expect(excerptOf({ preview: "Some text" }, meta(1, { excerpt: "Real prose" }))).toBe("Real prose");
    expect(excerptOf({ preview: "Some text" }, meta(1, { excerpt: "" }))).toBe("");
  });

  test("before it loads, a heading line shows nothing and list markers drop", () => {
    expect(excerptOf({ preview: "## Why" }, undefined)).toBe("");
    expect(excerptOf({ preview: "```ts" }, undefined)).toBe("");
    expect(excerptOf({ preview: "- [x] Ship *it*" }, undefined)).toBe("Ship it");
  });

  test("link and length descriptions", () => {
    expect(describeLinks(meta(1, { links: { issues: 2, pages: 1, comments: 0 } }))).toBe("Linked from 2 issues and 1 page");
    expect(describeLinks(meta(1, { links: { issues: 1, pages: 0, comments: 3 } }))).toBe("Linked from 1 issue and 3 comments");
    expect(describeLinks(meta(1))).toBe("");
    expect(describeLength(412)).toBe("412 words · 2 min read");
    expect(describeLength(0)).toBe("");
  });

});

describe("stored view", () => {
  test("keeps known values and drops the rest", () => {
    const v = parseView(
      JSON.stringify({
        scope: 4,
        filters: { statuses: ["draft", "archived", 3], label: "spec", editor: "u:7", unlinked: true },
        sort: "title",
        group: "bogus",
        excerpts: false,
      }),
    );
    expect(v).toEqual({
      scope: 4,
      filters: { statuses: ["draft"], label: "spec", editor: "u:7", unlinked: true },
      sort: "title",
      group: "folder",
      excerpts: false,
      pane: true,
    });
  });

  test("archived and conventions scopes survive; the old unfiled scope does not", () => {
    expect(parseView(JSON.stringify({ scope: "archived" })).scope).toBe("archived");
    expect(parseView(JSON.stringify({ scope: "conventions" })).scope).toBe("conventions");
    expect(parseView(JSON.stringify({ scope: "unfiled" })).scope).toBe("all");
  });

  test("garbage falls back to the defaults", () => {
    expect(parseView("{nope").group).toBe("folder");
    expect(parseView(JSON.stringify({ filters: { editor: "drop table" } })).filters.editor).toBe("");
  });
});
