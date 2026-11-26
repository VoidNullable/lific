import { describe, expect, test } from "bun:test";
import { loadSnapshot, storeSnapshot, validSnapshot } from "../src/lib/sync/persistence";

const issue = {
  kind: "issue", id: 1, seq: 12, deleted: false, identifier: "APP-1",
  title: "Saved issue", status: "todo", priority: "none", module_id: null,
  sort_order: 0, start_date: null, target_date: null, created_at: "today",
  updated_at: "today", preview: "", labels: ["test"],
  needs_human: true, assignees: [{ user_id: 3, username: "person" }],
  waits: [{ id: 1, issue_id: 1, kind: "date", earliest: "2026-11-26",
    latest: "2026-11-27", note: "", state: "due", created_at: "today" }],
};
const page = {
  kind: "page", id: 2, seq: 5, deleted: false, identifier: "APP-DOC-2",
  title: "Saved page", status: "draft", folder_id: null, pinned: false,
  created_at: "today", updated_at: "today", preview: "", labels: [],
};
const snapshot = { version: 1, database_id: "database-a", cursor: 10, issues: [issue], pages: [page] };

describe("read-model disk validation", () => {
  test("accepts live summaries, including rows ahead of the snapshot cursor", () => {
    expect(validSnapshot(snapshot)).toBe(true);
    expect(validSnapshot({ ...snapshot, cursor: 0, issues: [], pages: [] })).toBe(true);
  });

  test("rejects incompatible versions, corrupt rows, and duplicate identities", () => {
    for (const invalid of [
      null, {}, { ...snapshot, version: 2 }, { ...snapshot, cursor: -1 },
      { ...snapshot, database_id: undefined }, { ...snapshot, database_id: "" },
      { ...snapshot, cursor: Infinity }, { ...snapshot, pages: null },
      { ...snapshot, issues: [issue, issue] },
      { ...snapshot, issues: [{ ...issue, deleted: true }] },
      { ...snapshot, issues: [{ ...issue, labels: [12] }] },
      { ...snapshot, issues: [{ ...issue, title: null }] },
      { ...snapshot, issues: [{ ...issue, assignees: [null] }] },
      { ...snapshot, issues: [{ ...issue, waits: [{}] }] },
      { ...snapshot, pages: [{ ...page, pinned: "yes" }] },
    ]) expect(validSnapshot(invalid)).toBe(false);
  });

  test("unavailable IndexedDB is a cache miss, not a bootstrap failure", async () => {
    expect(await loadSnapshot("private:1:1")).toEqual({ snapshot: null, epoch: null });
    expect(await storeSnapshot("private:1:1", { database_id: "database-a", cursor: 0, issues: [], pages: [] }, "")).toBe(false);
  });
});
