import { describe, expect, test } from "bun:test";
import type { Attention, Issue, IssueWait } from "../src/lib/api";
import {
  addDays,
  applyBaseFilter,
  buildEntries,
  chipWaits,
  emptyReadState,
  hasOverdueWait,
  LATER_NOTE,
  groupEntries,
  inTab,
  isUnread,
  markRead,
  markUnread,
  nextMonday,
  reconcileRead,
  sortEntries,
  updatedBucket,
} from "../src/lib/inbox/inbox";

function issue(id: number, over: Partial<Issue> = {}): Issue {
  return {
    id,
    project_id: 1,
    sequence: id,
    identifier: `P-${id}`,
    title: `Issue ${id}`,
    description: "",
    status: "todo",
    priority: "none",
    module_id: null,
    sort_order: 0,
    start_date: null,
    target_date: null,
    created_at: "2026-11-01 10:00:00",
    updated_at: `2026-11-${String(10 + id).padStart(2, "0")} 10:00:00`,
    labels: [],
    ...over,
  };
}

function dateWait(earliest: string, latest = earliest): IssueWait {
  return { id: 9, issue_id: 1, kind: "date", earliest, latest, note: "", state: "holding", created_at: "" };
}

const filter = { tab: "all" as const, projects: [], query: "", pressing: false, overdue: false };

describe("buildEntries", () => {
  test("merges an issue named by several groups into one entry with every reason", () => {
    const a = issue(1);
    const att: Attention = { assigned: [a], human: [issue(2)], waiting: [a] };
    const entries = buildEntries(att, "2026-11-22");
    expect(entries.map((e) => e.issue.id)).toEqual([2, 1]);
    expect(entries.find((e) => e.issue.id === 1)!.reasons).toEqual(["assigned", "waiting"]);
  });

  test("an issue with a date wait still holding is put off until later", () => {
    const att: Attention = { assigned: [issue(1, { waits: [dateWait("2026-11-25")] })], human: [], waiting: [] };
    const [entry] = buildEntries(att, "2026-11-22");
    expect(entry.later?.earliest).toBe("2026-11-25");
    expect(inTab(entry, "all", false)).toBe(false);
    expect(inTab(entry, "later", false)).toBe(true);
    const [due] = buildEntries(att, "2026-11-25");
    expect(due.later).toBeNull();
  });

  test("an inbox later that came back is marked back, not overdue", () => {
    const later = { ...dateWait("2026-11-20"), note: LATER_NOTE };
    const att: Attention = { assigned: [issue(1, { waits: [later, { ...dateWait("2026-11-01"), id: 10 }] })], human: [], waiting: [] };
    const [entry] = buildEntries(att, "2026-11-22");
    expect(entry.later).toBeNull();
    expect(entry.returned?.id).toBe(later.id);
    expect(chipWaits(entry.issue, "2026-11-22")).toHaveLength(1);
    expect(hasOverdueWait(entry.issue, "2026-11-22")).toBe(true);
    const onlyLater = issue(2, { waits: [later] });
    expect(hasOverdueWait(onlyLater, "2026-11-22")).toBe(false);
  });
});

describe("read state", () => {
  test("a first load seeds everything as read, so nothing is unread", () => {
    const issues = [issue(1), issue(2)];
    const state = reconcileRead(emptyReadState(), issues);
    expect(issues.some((i) => isUnread(state, i))).toBe(false);
  });

  test("an issue that changed since it was read is unread until marked again", () => {
    const state = reconcileRead(emptyReadState(), [issue(1)]);
    const bumped = issue(1, { updated_at: "2026-11-30 09:00:00" });
    expect(isUnread(state, bumped)).toBe(true);
    expect(isUnread(markRead(state, [bumped]), bumped)).toBe(false);
    expect(isUnread(markUnread(state, [issue(1)]), issue(1))).toBe(true);
  });

  test("records for issues that left the inbox are dropped", () => {
    const state = reconcileRead(emptyReadState(), [issue(1), issue(2)]);
    expect(Object.keys(reconcileRead(state, [issue(2)]).items)).toEqual(["2"]);
  });
});

describe("filter, sort, group", () => {
  const att: Attention = {
    assigned: [issue(1, { priority: "low" }), issue(2, { priority: "urgent", project_id: 2 })],
    human: [issue(3, { priority: "high", title: "Rotate the key" })],
    waiting: [],
  };
  const entries = buildEntries(att, null);
  const name = (id: number) => (id === 1 ? "Alpha" : "Beta");

  test("search matches every word across title, identifier and project", () => {
    const hits = applyBaseFilter(entries, { ...filter, query: "rotate alpha" }, null, name);
    expect(hits.map((e) => e.issue.id)).toEqual([3]);
  });

  test("pressing keeps urgent and high", () => {
    const hits = applyBaseFilter(entries, { ...filter, pressing: true }, null, name);
    expect(hits.map((e) => e.issue.id).sort()).toEqual([2, 3]);
  });

  test("priority sort puts urgent first", () => {
    expect(sortEntries(entries, "priority").map((e) => e.issue.id)).toEqual([2, 3, 1]);
    expect(sortEntries(entries, "oldest").map((e) => e.issue.id)).toEqual([1, 2, 3]);
  });

  test("project groups lead with the project holding the most pressing work", () => {
    const groups = groupEntries(sortEntries(entries, "priority"), "project", name);
    expect(groups.map((g) => g.label)).toEqual(["Beta", "Alpha"]);
  });

  test("updated buckets", () => {
    const now = new Date("2026-11-22T15:00:00").getTime();
    const local = (d: Date) => d.toISOString().replace("T", " ").slice(0, 19);
    expect(updatedBucket(local(new Date("2026-11-22T12:00:00")), now).label).toBe("Today");
    expect(updatedBucket(local(new Date("2026-11-21T12:00:00")), now).label).toBe("Yesterday");
    expect(updatedBucket(local(new Date("2026-10-01T12:00:00")), now).label).toBe("Older");
  });
});

describe("days", () => {
  test("addDays crosses months", () => {
    expect(addDays("2026-11-30", 1)).toBe("2026-12-01");
  });
  test("nextMonday is strictly after the day", () => {
    expect(nextMonday("2026-11-22")).toBe("2026-11-23"); // Sunday
    expect(nextMonday("2026-11-23")).toBe("2026-11-30"); // Monday
  });
});
