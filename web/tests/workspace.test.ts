import { describe, expect, test } from "bun:test";
import type { Issue } from "../src/lib/api";
import {
  compareByPriorityThenUpdated,
  groupByOpenStatus,
  newSince,
  serverTime,
  uniqueIssueCount,
} from "../src/lib/workspace/issues";
import { INBOX_VISIT_KEY, recordVisit, type VisitStorage } from "../src/lib/workspace/lastVisit";
import { VISIT_GAP_MS } from "../src/lib/home/lastVisit";

function issue(id: number, status: string, priority: string, updated_at: string): Issue {
  return {
    id,
    project_id: 1,
    sequence: id,
    identifier: `P-${id}`,
    title: `Issue ${id}`,
    description: "",
    status,
    priority,
    module_id: null,
    sort_order: 0,
    start_date: null,
    target_date: null,
    created_at: "2026-01-01 00:00:00",
    updated_at,
    labels: [],
  };
}

function memoryStorage(): VisitStorage & { data: Map<string, string> } {
  const data = new Map<string, string>();
  return {
    data,
    getItem: (k) => data.get(k) ?? null,
    setItem: (k, v) => void data.set(k, v),
  };
}

describe("serverTime", () => {
  test("reads SQLite timestamps as UTC", () => {
    expect(serverTime("2026-03-01 12:00:00")).toBe(Date.UTC(2026, 2, 1, 12));
    expect(serverTime("2026-03-01T12:00:00Z")).toBe(Date.UTC(2026, 2, 1, 12));
  });
});

describe("groupByOpenStatus", () => {
  test("orders groups active, todo, backlog and skips empty and closed", () => {
    const groups = groupByOpenStatus([
      issue(1, "backlog", "low", "2026-01-02 00:00:00"),
      issue(2, "done", "urgent", "2026-01-02 00:00:00"),
      issue(3, "active", "none", "2026-01-02 00:00:00"),
      issue(4, "cancelled", "high", "2026-01-02 00:00:00"),
    ]);
    expect(groups.map((g) => g.status)).toEqual(["active", "backlog"]);
    expect(groups.map((g) => g.label)).toEqual(["Active", "Backlog"]);
  });

  test("sorts by priority, then most recently updated", () => {
    const [todo] = groupByOpenStatus([
      issue(1, "todo", "low", "2026-01-05 00:00:00"),
      issue(2, "todo", "urgent", "2026-01-01 00:00:00"),
      issue(3, "todo", "high", "2026-01-02 00:00:00"),
      issue(4, "todo", "high", "2026-01-03 00:00:00"),
      issue(5, "todo", "none", "2026-01-09 00:00:00"),
    ]);
    expect(todo.items.map((i) => i.id)).toEqual([2, 4, 3, 1, 5]);
  });

  test("drops repeated issues", () => {
    const a = issue(1, "todo", "low", "2026-01-05 00:00:00");
    expect(groupByOpenStatus([a, a])[0].items).toHaveLength(1);
  });

  test("an unknown priority sorts with none", () => {
    const a = issue(1, "todo", "weird", "2026-01-05 00:00:00");
    const b = issue(2, "todo", "low", "2026-01-01 00:00:00");
    expect(compareByPriorityThenUpdated(a, b)).toBeGreaterThan(0);
  });
});

describe("uniqueIssueCount", () => {
  test("counts an issue in two groups once", () => {
    const shared = issue(1, "todo", "low", "2026-01-01 00:00:00");
    const other = issue(2, "todo", "low", "2026-01-01 00:00:00");
    expect(uniqueIssueCount([{ items: [shared, other] }, { items: [shared] }])).toBe(2);
  });
});

describe("newSince", () => {
  test("marks issues updated after the previous visit", () => {
    const since = Date.UTC(2026, 0, 2);
    const fresh = newSince(
      [issue(1, "todo", "low", "2026-01-01 00:00:00"), issue(2, "todo", "low", "2026-01-03 00:00:00")],
      since,
    );
    expect([...fresh]).toEqual([2]);
  });

  test("marks nothing on a first visit", () => {
    expect(newSince([issue(1, "todo", "low", "2026-01-03 00:00:00")], null).size).toBe(0);
  });
});

describe("recordVisit", () => {
  test("keeps each key's visit separate", () => {
    const storage = memoryStorage();
    const t0 = 1_000_000;
    expect(recordVisit(INBOX_VISIT_KEY, t0, storage)).toBeNull();
    expect(recordVisit("lific_other_visit", t0 + VISIT_GAP_MS * 3, storage)).toBeNull();
    // A later Inbox visit compares with the earlier Inbox view, untouched by the other key.
    expect(recordVisit(INBOX_VISIT_KEY, t0 + VISIT_GAP_MS * 2, storage)).toBe(t0);
  });

  test("views within the gap keep measuring from the same visit", () => {
    const storage = memoryStorage();
    recordVisit(INBOX_VISIT_KEY, 0, storage);
    const since = recordVisit(INBOX_VISIT_KEY, VISIT_GAP_MS + 1, storage);
    expect(since).toBe(0);
    expect(recordVisit(INBOX_VISIT_KEY, VISIT_GAP_MS + 2, storage)).toBe(0);
  });

  test("ignores a corrupt stored value", () => {
    const storage = memoryStorage();
    storage.setItem(INBOX_VISIT_KEY, "{not json");
    expect(recordVisit(INBOX_VISIT_KEY, 5, storage)).toBeNull();
  });

  test("without storage there is no previous visit", () => {
    expect(recordVisit(INBOX_VISIT_KEY, 5, null)).toBeNull();
  });
});
