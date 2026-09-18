// Issue-list grouping + density. Extracted from IssueList.svelte (LIF-99).
//
// LIF-191: the list can group issues by status / priority / module, and the
// "Display" popover toggles density. The status/priority orderings are the
// canonical ones used across the issue list (filters, group buckets,
// keyboard status/priority cycling), so they live here as the single source
// of truth and are imported back into the component.

import type { Issue, Module } from "../api";

/** Canonical status order (backlog → cancelled). */
export const STATUSES = ["backlog", "todo", "active", "done", "cancelled"];
/** Canonical priority order (urgent → none). */
export const PRIORITIES = ["urgent", "high", "medium", "low", "none"];

/** Terminal statuses — work that's left the board (no longer actionable). */
export const TERMINAL_STATUSES = ["done", "cancelled"];

/** Sentinel `filterStatus` value for the "Unresolved" status-group filter:
 *  everything that isn't in a terminal state (backlog + todo + active). Uses
 *  an `@`-prefix so it can never collide with a real status string. */
export const STATUS_UNRESOLVED = "@unresolved";

/** True when an issue is unresolved (not done/cancelled). */
export function isUnresolved(status: string): boolean {
  return !TERMINAL_STATUSES.includes(status);
}

/** One-line descriptions of each status, surfaced in the filter modal so the
 *  vocabulary is self-documenting. Single source of truth. */
export const STATUS_DESCRIPTIONS: Record<string, string> = {
  backlog: "Captured, not yet planned.",
  todo: "Planned and ready to start.",
  active: "In progress right now.",
  done: "Completed and shipped.",
  cancelled: "Abandoned — won't be done.",
};

/** Description of the "Unresolved" status-group filter. */
export const UNRESOLVED_DESCRIPTION =
  "All open work — backlog, todo, and active.";

/** One-line descriptions of each priority level. */
export const PRIORITY_DESCRIPTIONS: Record<string, string> = {
  urgent: "Drop everything.",
  high: "Important — do soon.",
  medium: "Normal priority.",
  low: "Nice to have, no rush.",
  none: "No priority set.",
};

export type GroupBy = "status" | "priority" | "module" | "none";
export type Density = "compact" | "comfortable";

export type IssueGroup = {
  key: string;
  label: string;
  kind: GroupBy;
  module?: Module;
  issues: Issue[];
};

/** First non-heading line of a description, for the Comfortable density
 *  preview. Cheap markdown strip, capped at 160 chars. */
export function descriptionPreview(content: string): string {
  if (!content) return "";
  const lines = content
    .split("\n")
    .filter((l) => l.trim() && !l.startsWith("#"));
  return (lines[0] ?? "")
    .replace(/[*_`>[\]]/g, "")
    .trim()
    .slice(0, 160);
}

/** LIF-191: build ordered groups for the active `groupBy`, or null when the
 *  view should render flat — search mode, groupBy="none", or status-grouping
 *  under a single status filter (where buckets would be pointless).
 *
 *  Pure: the caller passes the already-sorted issues plus the current
 *  search/filter/grouping context and the module list. Empty buckets are
 *  omitted; the module grouping appends a "No module" bucket last. */
export function buildGroups(opts: {
  sortedIssues: Issue[];
  modules: Module[];
  groupBy: GroupBy;
  searchQuery: string;
  filterStatus: string;
}): IssueGroup[] | null {
  const { sortedIssues, modules, groupBy, searchQuery, filterStatus } = opts;

  if (searchQuery.trim()) return null;
  if (groupBy === "none") return null;
  // A single literal status filter makes status buckets pointless (one
  // bucket). The "Unresolved" group filter still spans backlog/todo/active,
  // so status grouping stays meaningful there — don't suppress it.
  if (
    groupBy === "status" &&
    filterStatus &&
    filterStatus !== STATUS_UNRESOLVED
  )
    return null;

  const out: IssueGroup[] = [];
  if (groupBy === "status") {
    for (const s of STATUSES) {
      const items = sortedIssues.filter((i) => i.status === s);
      if (items.length)
        out.push({ key: s, label: s, kind: "status", issues: items });
    }
  } else if (groupBy === "priority") {
    for (const p of PRIORITIES) {
      const items = sortedIssues.filter((i) => i.priority === p);
      if (items.length)
        out.push({ key: p, label: p, kind: "priority", issues: items });
    }
  } else if (groupBy === "module") {
    for (const m of modules) {
      const items = sortedIssues.filter((i) => i.module_id === m.id);
      if (items.length)
        out.push({
          key: String(m.id),
          label: m.name,
          kind: "module",
          module: m,
          issues: items,
        });
    }
    const none = sortedIssues.filter((i) => i.module_id == null);
    if (none.length)
      out.push({
        key: "none",
        label: "No module",
        kind: "module",
        issues: none,
      });
  }
  return out;
}

// ── LIF-241: board swimlanes ────────────────────────────────────────────
// Lanes are a board-only concept (see IssueList.svelte's board mode) and
// deliberately differ from buildGroups above in one key way: a swimlane is
// also a drag-and-drop target, so an empty "Design" lane must still render
// as a row you can drop a card into to assign it that module/priority.
// buildGroups omits empty buckets (they're just headers in a flat list);
// buildLanes never does — every module and every priority always gets a
// row, count 0 included.

export type LaneBy = "none" | "module" | "priority";

export type Lane = {
  key: string;
  label: string;
  kind: LaneBy;
  module?: Module;
  priority?: string;
  issues: Issue[];
};

/** Build swimlane rows for the board. Returns null for laneBy === "none",
 *  meaning the board renders as a single implicit lane (today's behavior). */
export function buildLanes(opts: {
  sortedIssues: Issue[];
  modules: Module[];
  laneBy: LaneBy;
}): Lane[] | null {
  const { sortedIssues, modules, laneBy } = opts;
  if (laneBy === "none") return null;

  if (laneBy === "priority") {
    return PRIORITIES.map((p) => ({
      key: p,
      label: p,
      kind: "priority" as const,
      priority: p,
      issues: sortedIssues.filter((i) => i.priority === p),
    }));
  }

  // module
  const lanes: Lane[] = modules.map((m) => ({
    key: String(m.id),
    label: m.name,
    kind: "module" as const,
    module: m,
    issues: sortedIssues.filter((i) => i.module_id === m.id),
  }));
  lanes.push({
    key: "none",
    label: "No module",
    kind: "module",
    issues: sortedIssues.filter((i) => i.module_id == null),
  });
  return lanes;
}

/** The lane key a given issue currently belongs to, under `laneBy` — the
 *  inverse of the bucketing in `buildLanes`. The board's dnd finalize
 *  handler uses this to detect a cross-lane drop: the dragged issue object
 *  is stale (its module_id/priority isn't updated until the PUT resolves),
 *  so comparing its lane key to the drop zone's lane key is the same trick
 *  IssueList already uses for cross-column status drops. */
export function laneKeyForIssue(issue: Issue, laneBy: LaneBy): string {
  if (laneBy === "priority") return issue.priority;
  if (laneBy === "module")
    return issue.module_id == null ? "none" : String(issue.module_id);
  return "";
}
