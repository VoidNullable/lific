// LIF-543: pure helpers for the Agent activity page. Formatting, axis
// labels and status copy live here so they can be unit tested without a
// DOM (see web/tests/agentActivity.test.ts).

import type {
  AgentActionCounts,
  AgentActionKind,
  AgentRecentItem,
  AgentStatus,
  AgentWindow,
} from "../api";

export const WINDOWS: AgentWindow[] = ["1h", "24h", "7d"];

export const WINDOW_SUBTITLE: Record<AgentWindow, string> = {
  "1h": "Last hour",
  "24h": "Last 24 hours",
  "7d": "Last 7 days",
};

/** Phrase used inside sentences ("No agent activity in the last hour."). */
export const WINDOW_PHRASE: Record<AgentWindow, string> = {
  "1h": "the last hour",
  "24h": "the last 24 hours",
  "7d": "the last 7 days",
};

export function isAgentWindow(v: unknown): v is AgentWindow {
  return v === "1h" || v === "24h" || v === "7d";
}

/** Bottom to top in the stacked bars, left to right in the legend. Colors
 *  are CSS custom properties defined on the page wrapper (light and dark
 *  variants), with fallbacks so a component still renders on its own. */
export const SERIES: { key: AgentActionKind; label: string; color: string }[] = [
  { key: "read", label: "Read", color: "var(--agent-read, #a3aea8)" },
  { key: "edit", label: "Edit", color: "var(--agent-edit, #3b6fd8)" },
  { key: "create", label: "Create", color: "var(--agent-create, #1f9d55)" },
  { key: "close", label: "Close", color: "var(--agent-close, #d98a0b)" },
];

const COUNT_NOUN: Record<AgentActionKind, [string, string]> = {
  read: ["Read", "Reads"],
  edit: ["Edit", "Edits"],
  create: ["Create", "Creates"],
  close: ["Close", "Closes"],
};

/** "Read" for exactly one, "Reads" otherwise (zero included). */
export function countNoun(kind: AgentActionKind, n: number): string {
  return COUNT_NOUN[kind][n === 1 ? 0 : 1];
}

export function totalOf(c: AgentActionCounts): number {
  return c.read + c.edit + c.create + c.close;
}

// ── Axis and tooltip labels ─────────────────────────────────

function hhmm(d: Date, timeZone?: string): string {
  return d.toLocaleTimeString("en-GB", {
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
    timeZone,
  });
}

/** Short x-axis label for a bucket start: HH:MM for 1h and 24h, the
 *  weekday for 7d. Local time unless `timeZone` is given (tests). */
export function bucketLabel(startIso: string, window: AgentWindow, timeZone?: string): string {
  const d = new Date(startIso);
  if (Number.isNaN(d.getTime())) return "";
  if (window === "7d") return d.toLocaleDateString("en-US", { weekday: "short", timeZone });
  return hhmm(d, timeZone);
}

/** Tooltip heading for a bucket: a time range for 1h/24h ("10:05 to
 *  10:10"), the full day for 7d ("Thu, Nov 12"). The last bucket ends at
 *  `now`, so its range reads "to now". */
export function bucketRange(
  startIso: string,
  window: AgentWindow,
  bucketSeconds: number,
  isLast: boolean,
  timeZone?: string,
): string {
  const d = new Date(startIso);
  if (Number.isNaN(d.getTime())) return "";
  if (window === "7d") {
    const day = d.toLocaleDateString("en-US", { weekday: "short", month: "short", day: "numeric", timeZone });
    return isLast ? `${day} (today)` : day;
  }
  const end = isLast ? "now" : hhmm(new Date(d.getTime() + bucketSeconds * 1000), timeZone);
  return `${hhmm(d, timeZone)} to ${end}`;
}

/** Y-axis gridlines for integer counts: 0 up to the first multiple of a
 *  1/2/5 x 10^n step at or above `max`, about `target` intervals. Unlike
 *  insights/curve.ts niceTicks, every tick is a whole, round number
 *  (niceTicks turns a max of 50 into 0, 13, 25, 38, 50). */
export function countTicks(max: number, target = 4): number[] {
  if (max <= 0) return [0, 1, 2, 3, 4];
  const raw = max / target;
  const mag = 10 ** Math.floor(Math.log10(raw));
  const step = Math.max(1, [1, 2, 5, 10].map((m) => m * mag).find((s) => s >= raw) ?? 10 * mag);
  const top = Math.ceil(max / step) * step;
  const out: number[] = [];
  for (let v = 0; v <= top; v += step) out.push(v);
  return out;
}

/** Which bucket indices get an x-axis label. Counts back from the newest
 *  bucket so "now" is always labeled, and keeps at most `maxLabels`. */
export function labelIndices(n: number, maxLabels = 8): number[] {
  if (n <= 0) return [];
  const stride = Math.max(1, Math.ceil(n / maxLabels));
  const out: number[] = [];
  for (let i = n - 1; i >= 0; i -= stride) out.unshift(i);
  return out;
}

// ── Status ──────────────────────────────────────────────────

export const STATUS_LABEL: Record<AgentStatus, string> = {
  active: "Active",
  idle: "Idle",
  inactive: "Inactive",
};

/** "just now", "38 min ago", "2 h ago", "3 days ago", then a date. */
export function lastSeenAgo(iso: string, nowMs: number): string {
  const t = new Date(iso).getTime();
  if (Number.isNaN(t)) return "";
  const mins = Math.floor((nowMs - t) / 60_000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins} min ago`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `${hrs} h ago`;
  const days = Math.floor(hrs / 24);
  if (days === 1) return "yesterday";
  if (days < 7) return `${days} days ago`;
  return new Date(t).toLocaleDateString("en-US", { month: "short", day: "numeric" });
}

/** Compact age for the Recent feed: "now", "2m", "38m", "3h", "2d", then a date. */
export function shortAgo(iso: string, nowMs: number): string {
  const t = new Date(iso).getTime();
  if (Number.isNaN(t)) return "";
  const mins = Math.floor((nowMs - t) / 60_000);
  if (mins < 1) return "now";
  if (mins < 60) return `${mins}m`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `${hrs}h`;
  const days = Math.floor(hrs / 24);
  if (days < 7) return `${days}d`;
  return new Date(t).toLocaleDateString("en-US", { month: "short", day: "numeric" });
}

/** Secondary line under the status pill. Active agents need none. */
export function statusDetail(status: AgentStatus, lastSeen: string | null, nowMs: number): string | null {
  if (status === "active") return null;
  if (lastSeen === null) return "No activity yet";
  return `Last seen ${lastSeenAgo(lastSeen, nowMs)}`;
}

/** "Blake's" for the owner line. */
export function possessive(name: string): string {
  return /s$/i.test(name) ? `${name}'` : `${name}'s`;
}

// ── Recent feed copy ────────────────────────────────────────

const NOUN: Record<string, string> = {
  page: "page",
  plan: "plan",
  module: "module",
  project: "project",
  label: "label",
  folder: "folder",
  attachment: "file",
};

/** The verb after the agent's name: "closed", "commented on", "read page",
 *  "created plan". Issues take no noun; the identifier says what it is. */
export function recentVerb(item: Pick<AgentRecentItem, "kind" | "entity_type" | "detail">): string {
  const noun = NOUN[item.entity_type];
  const withNoun = (verb: string) => (noun ? `${verb} ${noun}` : verb);
  switch (item.kind) {
    case "read":
      return withNoun("read");
    case "create":
      return withNoun("created");
    case "close":
      return "closed";
    case "edit":
      switch (item.detail) {
        case "comment":
          return withNoun("commented on");
        case "label":
          return withNoun("relabeled");
        case "link":
          return withNoun("linked");
        case "delete":
          return withNoun("deleted");
        case "status":
          return withNoun("changed status of");
        default:
          return withNoun("edited");
      }
  }
}

/** True when the window has no actions at all. */
export function isQuiet(totals: AgentActionCounts): boolean {
  return totalOf(totals) === 0;
}
