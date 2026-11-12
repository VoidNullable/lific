// Pure helpers shared by the workspace pages (Inbox, Assigned to me).

import type { Issue } from "../api";
import { PRIORITY_RANK } from "../issues/sort";

/** Open statuses in the order the Assigned page shows them. */
export const OPEN_STATUS_ORDER = ["active", "todo", "backlog"] as const;
export type OpenStatus = (typeof OPEN_STATUS_ORDER)[number];

export const STATUS_LABELS: Record<OpenStatus, string> = {
  active: "Active",
  todo: "Todo",
  backlog: "Backlog",
};

/** Epoch ms of a server timestamp ("YYYY-MM-DD HH:MM:SS", UTC). */
export function serverTime(ts: string): number {
  const iso = ts.includes("T") ? ts : ts.replace(" ", "T");
  return new Date(/[zZ]|[+-]\d\d:?\d\d$/.test(iso) ? iso : iso + "Z").getTime();
}

/** Urgent first, then most recently updated. */
export function compareByPriorityThenUpdated(a: Issue, b: Issue): number {
  const pa = PRIORITY_RANK[a.priority] ?? PRIORITY_RANK.none;
  const pb = PRIORITY_RANK[b.priority] ?? PRIORITY_RANK.none;
  if (pa !== pb) return pa - pb;
  return serverTime(b.updated_at) - serverTime(a.updated_at);
}

export interface StatusGroup {
  status: OpenStatus;
  label: string;
  items: Issue[];
}

/** Open issues grouped by status (active, todo, backlog), empty groups
 *  dropped, duplicates by id removed, each group sorted by priority then
 *  recency. Done and cancelled issues are left out. */
export function groupByOpenStatus(issues: Issue[]): StatusGroup[] {
  const seen = new Set<number>();
  const buckets = new Map<string, Issue[]>(OPEN_STATUS_ORDER.map((s) => [s, []]));
  for (const issue of issues) {
    const bucket = buckets.get(issue.status);
    if (!bucket || seen.has(issue.id)) continue;
    seen.add(issue.id);
    bucket.push(issue);
  }
  return OPEN_STATUS_ORDER.map((status) => ({
    status,
    label: STATUS_LABELS[status],
    items: [...buckets.get(status)!].sort(compareByPriorityThenUpdated),
  })).filter((g) => g.items.length > 0);
}

/** Distinct issues across groups, for a header count. */
export function uniqueIssueCount(groups: { items: Issue[] }[]): number {
  return new Set(groups.flatMap((g) => g.items.map((i) => i.id))).size;
}

/** Issues updated after `since` (epoch ms). None on a first visit. */
export function newSince(issues: Issue[], since: number | null): Set<number> {
  if (since === null) return new Set();
  return new Set(issues.filter((i) => serverTime(i.updated_at) > since).map((i) => i.id));
}
