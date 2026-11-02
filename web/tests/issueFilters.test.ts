import { describe, expect, test } from "bun:test";
import {
  applyIssueFilters,
  statusFilterSet,
  toFilterList,
  toggleFilterValue,
  type IssueFilters,
} from "../src/lib/issues/filters";
import { STATUS_UNRESOLVED } from "../src/lib/issues/grouping";

type Row = {
  id: string;
  status: string;
  priority: string;
  labels: string[];
  module_id: number | null;
  needs_human: boolean;
  assignees: { user_id: number; username: string }[];
};

function row(id: string, over: Partial<Row> = {}): Row {
  return {
    id,
    status: "todo",
    priority: "medium",
    labels: [],
    module_id: null,
    needs_human: false,
    assignees: [],
    ...over,
  };
}

const none: IssueFilters = { status: [], priority: [], label: [], module: [], assignee: [] };
const modules = [
  { id: 1, name: "Web" },
  { id: 2, name: "API" },
];
const me = { id: 7, username: "alice" };

const rows = [
  row("urgent-web", { priority: "urgent", module_id: 1, labels: ["bug"] }),
  row("high-api", { priority: "high", module_id: 2, labels: ["ux"], status: "active" }),
  row("low-done", { priority: "low", status: "done", labels: ["bug", "ux"] }),
  row("mine", { needs_human: true, assignees: [{ user_id: 7, username: "alice" }] }),
  row("human", { needs_human: true, status: "cancelled" }),
];

const ids = (filters: Partial<IssueFilters>) =>
  applyIssueFilters(rows, { ...none, ...filters }, modules, me).map((r) => r.id);

describe("multi-select issue filters", () => {
  test("no selection keeps every issue", () => {
    expect(ids({})).toEqual(rows.map((r) => r.id));
  });

  test("several priorities match any of them", () => {
    expect(ids({ priority: ["urgent", "high"] })).toEqual(["urgent-web", "high-api"]);
  });

  test("several statuses match any of them, and Unresolved expands", () => {
    expect(ids({ status: ["done", "cancelled"] })).toEqual(["low-done", "human"]);
    expect(ids({ status: [STATUS_UNRESOLVED, "done"] })).toEqual([
      "urgent-web",
      "high-api",
      "low-done",
      "mine",
    ]);
  });

  test("labels match an issue carrying any selected label", () => {
    expect(ids({ label: ["ux"] })).toEqual(["high-api", "low-done"]);
    expect(ids({ label: ["bug", "ux"] })).toEqual(["urgent-web", "high-api", "low-done"]);
  });

  test("modules match by name, and an unknown name means no module", () => {
    expect(ids({ module: ["Web", "API"] })).toEqual(["urgent-web", "high-api"]);
    expect(ids({ module: ["Gone"] })).toEqual(["low-done", "mine", "human"]);
  });

  test("assignee values match any of them", () => {
    expect(ids({ assignee: ["me", "human"] })).toEqual(["mine", "human"]);
  });

  test("different filters all have to match", () => {
    expect(ids({ priority: ["urgent", "high"], label: ["ux"] })).toEqual(["high-api"]);
  });
});

describe("filter values", () => {
  test("legacy single strings and lists both read as lists", () => {
    expect(toFilterList("")).toEqual([]);
    expect(toFilterList(undefined)).toEqual([]);
    expect(toFilterList("urgent")).toEqual(["urgent"]);
    expect(toFilterList(["urgent", "", 3, "high", "urgent"])).toEqual(["urgent", "high"]);
  });

  test("toggling adds and removes one value", () => {
    expect(toggleFilterValue(["urgent"], "high")).toEqual(["urgent", "high"]);
    expect(toggleFilterValue(["urgent", "high"], "urgent")).toEqual(["high"]);
  });

  test("the status set is null without a filter", () => {
    expect(statusFilterSet([])).toBeNull();
    expect([...statusFilterSet([STATUS_UNRESOLVED])!].sort()).toEqual(["active", "backlog", "todo"]);
  });
});
