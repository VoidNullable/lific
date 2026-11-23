// Pure logic for the Inbox: one deduplicated feed with reasons, read state,
// "later" (a holding date wait), grouping, sorting and filtering. Kept free
// of Svelte so it can be tested directly.

import type { Attention, Issue, IssueWait } from "../api";
import { PRIORITY_RANK } from "../issues/sort";
import { waitState } from "../issues/waits";
import { serverTime } from "../workspace/issues";

export type Reason = "assigned" | "human" | "waiting";

export const REASONS: Reason[] = ["assigned", "human", "waiting"];

export const REASON_LABEL: Record<Reason, string> = {
  assigned: "Assigned to you",
  human: "For anyone",
  waiting: "Waiting on you",
};

/** Short form for row chips. */
export const REASON_CHIP: Record<Reason, string> = {
  assigned: "You",
  human: "Anyone",
  waiting: "Waiting",
};

export interface InboxEntry {
  issue: Issue;
  reasons: Reason[];
  /** A date wait that still holds: the issue was put off until later. */
  later: IssueWait | null;
  /** An inbox "later" wait whose day has come: the issue is back. */
  returned: IssueWait | null;
}

/** The note an inbox "later" writes on its date wait, so the inbox can tell
 *  its own waits from ones people set on the issue for other reasons. */
export const LATER_NOTE = "Put off from the inbox";

export function isInboxLater(wait: IssueWait): boolean {
  return wait.kind === "date" && wait.note === LATER_NOTE;
}

/** The date wait holding this issue back today, if any. */
export function holdingDateWait(issue: Issue, today: string | null): IssueWait | null {
  for (const w of issue.waits ?? []) {
    if (w.kind === "date" && waitState(w, today) === "holding") return w;
  }
  return null;
}

/** An inbox "later" wait that is no longer holding. */
export function returnedLater(issue: Issue, today: string | null): IssueWait | null {
  for (const w of issue.waits ?? []) {
    if (isInboxLater(w) && waitState(w, today) !== "holding") return w;
  }
  return null;
}

/** The waits worth a chip: an inbox "later" that came back is shown as
 *  "Back" by the inbox, not as an overdue wait. */
export function chipWaits(issue: Issue, today: string | null): IssueWait[] {
  return (issue.waits ?? []).filter((w) => !(isInboxLater(w) && waitState(w, today) !== "holding"));
}

/** Merge the three attention groups into one entry per issue, newest first.
 *  An issue named in several groups carries every reason once. */
export function buildEntries(attention: Attention, today: string | null): InboxEntry[] {
  const byId = new Map<number, InboxEntry>();
  for (const reason of REASONS) {
    for (const issue of attention[reason]) {
      const entry = byId.get(issue.id);
      if (entry) {
        if (!entry.reasons.includes(reason)) entry.reasons.push(reason);
      } else {
        byId.set(issue.id, {
          issue,
          reasons: [reason],
          later: holdingDateWait(issue, today),
          returned: returnedLater(issue, today),
        });
      }
    }
  }
  return [...byId.values()].sort(
    (a, b) => serverTime(b.issue.updated_at) - serverTime(a.issue.updated_at),
  );
}

/** True when any attention group came back at the server's per-group cap. */
export function anyCapped(attention: Attention, limit: number): boolean {
  return REASONS.some((r) => attention[r].length >= limit);
}

// ── Read state ─────────────────────────────────────────────────────────
// Per issue: the `updated_at` the person last read it at. An issue is
// unread when it has changed since. A device with no record seeds from the
// first load, so the first visit never says "everything is unread".

export interface ReadState {
  seeded: boolean;
  items: Record<number, string>;
}

export function emptyReadState(): ReadState {
  return { seeded: false, items: {} };
}

export function isUnread(state: ReadState, issue: Issue): boolean {
  if (!state.seeded) return false;
  return state.items[issue.id] !== issue.updated_at;
}

/** Seed on first sight; afterwards drop records for issues that left. */
export function reconcileRead(state: ReadState, issues: Issue[]): ReadState {
  if (!state.seeded) {
    return { seeded: true, items: Object.fromEntries(issues.map((i) => [i.id, i.updated_at])) };
  }
  const live = new Set(issues.map((i) => i.id));
  const items: Record<number, string> = {};
  let dropped = false;
  for (const [id, stamp] of Object.entries(state.items)) {
    if (live.has(Number(id))) items[Number(id)] = stamp;
    else dropped = true;
  }
  return dropped ? { seeded: true, items } : state;
}

export function markRead(state: ReadState, issues: Issue[]): ReadState {
  const items = { ...state.items };
  let changed = false;
  for (const i of issues) {
    if (items[i.id] !== i.updated_at) {
      items[i.id] = i.updated_at;
      changed = true;
    }
  }
  return changed ? { seeded: true, items } : state;
}

export function markUnread(state: ReadState, issues: Issue[]): ReadState {
  const items = { ...state.items };
  for (const i of issues) delete items[i.id];
  return { seeded: true, items };
}

// ── Filter, sort, group ────────────────────────────────────────────────

export type InboxTab = "all" | "unread" | Reason | "later";
export type SortBy = "priority" | "newest" | "oldest";
export type GroupBy = "project" | "priority" | "reason" | "updated" | "none";

export interface InboxFilter {
  tab: InboxTab;
  projects: number[];
  query: string;
  /** Only urgent and high priority. */
  pressing: boolean;
  /** Only issues with an overdue wait. */
  overdue: boolean;
}

export function isPressing(issue: Issue): boolean {
  return issue.priority === "urgent" || issue.priority === "high";
}

export function hasOverdueWait(issue: Issue, today: string | null): boolean {
  return chipWaits(issue, today).some((w) => waitState(w, today) === "overdue");
}

export function matchesQuery(entry: InboxEntry, query: string, projectName?: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  const hay = [
    entry.issue.identifier,
    entry.issue.title,
    projectName ?? "",
    ...entry.issue.labels,
  ]
    .join(" ")
    .toLowerCase();
  return q.split(/\s+/).every((word) => hay.includes(word));
}

/** Everything but the tab: the tab counts are computed over this. */
export function applyBaseFilter(
  entries: InboxEntry[],
  filter: InboxFilter,
  today: string | null,
  projectName: (id: number) => string | undefined,
): InboxEntry[] {
  return entries.filter(
    (e) =>
      (filter.projects.length === 0 || filter.projects.includes(e.issue.project_id)) &&
      (!filter.pressing || isPressing(e.issue)) &&
      (!filter.overdue || hasOverdueWait(e.issue, today)) &&
      matchesQuery(e, filter.query, projectName(e.issue.project_id)),
  );
}

/** "Later" entries only show under their own tab. */
export function inTab(entry: InboxEntry, tab: InboxTab, unread: boolean): boolean {
  if (tab === "later") return entry.later !== null;
  if (entry.later) return false;
  if (tab === "all") return true;
  if (tab === "unread") return unread;
  return entry.reasons.includes(tab);
}

export function sortEntries(entries: InboxEntry[], sort: SortBy): InboxEntry[] {
  const out = [...entries];
  const time = (e: InboxEntry) => serverTime(e.issue.updated_at);
  if (sort === "newest") out.sort((a, b) => time(b) - time(a));
  else if (sort === "oldest") out.sort((a, b) => time(a) - time(b));
  else {
    out.sort((a, b) => {
      const pa = PRIORITY_RANK[a.issue.priority] ?? PRIORITY_RANK.none;
      const pb = PRIORITY_RANK[b.issue.priority] ?? PRIORITY_RANK.none;
      return pa !== pb ? pa - pb : time(b) - time(a);
    });
  }
  return out;
}

export interface EntryGroup {
  key: string;
  label: string;
  entries: InboxEntry[];
  /** The project id, for project groups (drives the icon). */
  projectId?: number;
  priority?: string;
  reason?: Reason;
}

const DAY = 86_400_000;

/** Which recency bucket an update falls in, relative to `now`. */
export function updatedBucket(updatedAt: string, now: number): { key: string; label: string } {
  const t = serverTime(updatedAt);
  const start = new Date(now);
  start.setHours(0, 0, 0, 0);
  const today = start.getTime();
  if (t >= today) return { key: "0", label: "Today" };
  if (t >= today - DAY) return { key: "1", label: "Yesterday" };
  if (t >= today - 6 * DAY) return { key: "2", label: "Earlier this week" };
  if (t >= today - 29 * DAY) return { key: "3", label: "This month" };
  return { key: "4", label: "Older" };
}

const PRIORITY_ORDER = ["urgent", "high", "medium", "low", "none"];

/** Group already-sorted entries; group order is fixed per mode, and the
 *  entries keep their sorted order inside each group. */
export function groupEntries(
  entries: InboxEntry[],
  by: GroupBy,
  projectName: (id: number) => string | undefined,
  now: number = Date.now(),
): EntryGroup[] {
  if (by === "none") return [{ key: "all", label: "Everything", entries }];
  const groups = new Map<string, EntryGroup>();
  const add = (key: string, make: () => Omit<EntryGroup, "entries">, e: InboxEntry) => {
    let g = groups.get(key);
    if (!g) {
      g = { ...make(), entries: [] };
      groups.set(key, g);
    }
    g.entries.push(e);
  };
  for (const e of entries) {
    if (by === "project") {
      const id = e.issue.project_id;
      add(String(id), () => ({
        key: String(id),
        label: projectName(id) ?? e.issue.identifier.split("-")[0],
        projectId: id,
      }), e);
    } else if (by === "priority") {
      const p = e.issue.priority;
      add(p, () => ({ key: p, label: p === "none" ? "No priority" : p[0].toUpperCase() + p.slice(1), priority: p }), e);
    } else if (by === "reason") {
      const r = e.reasons[0];
      add(r, () => ({ key: r, label: REASON_LABEL[r], reason: r }), e);
    } else {
      const b = updatedBucket(e.issue.updated_at, now);
      add(b.key, () => ({ key: b.key, label: b.label }), e);
    }
  }
  const list = [...groups.values()];
  if (by === "priority") list.sort((a, b) => PRIORITY_ORDER.indexOf(a.key) - PRIORITY_ORDER.indexOf(b.key));
  else if (by === "reason") list.sort((a, b) => REASONS.indexOf(a.reason!) - REASONS.indexOf(b.reason!));
  else if (by === "updated") list.sort((a, b) => a.key.localeCompare(b.key));
  else {
    // Projects: the one with the most pressing work first, then by size.
    const best = (g: EntryGroup) =>
      Math.min(...g.entries.map((e) => PRIORITY_RANK[e.issue.priority] ?? PRIORITY_RANK.none));
    list.sort((a, b) => best(a) - best(b) || b.entries.length - a.entries.length || a.label.localeCompare(b.label));
  }
  return list;
}

// ── Days, for "later" ──────────────────────────────────────────────────

/** `day` (YYYY-MM-DD) plus `n` days, calendar-safe. */
export function addDays(day: string, n: number): string {
  const [y, m, d] = day.split("-").map(Number);
  const date = new Date(Date.UTC(y, m - 1, d + n));
  return date.toISOString().slice(0, 10);
}

/** The next Monday strictly after `day`. */
export function nextMonday(day: string): string {
  const [y, m, d] = day.split("-").map(Number);
  const dow = new Date(Date.UTC(y, m - 1, d)).getUTCDay(); // 0 = Sunday
  const ahead = ((8 - dow) % 7) || 7;
  return addDays(day, ahead);
}
