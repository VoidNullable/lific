// LIF-237 — localStorage-backed "recently viewed" for the Home dashboard.
//
// Recorded from the three detail routes (IssueDetail, PageDetail,
// PlanDetail) on load; read back by Home. Deliberately client-only, no
// server round trip — this is genuinely per-browser/per-device, the same
// contract as e.g. an editor's "recent files" list. Swallows every storage
// failure (private mode / quota) since recents are a nicety, never
// load-bearing.

export type RecentType = "issue" | "page" | "plan";

export interface RecentEntry {
  type: RecentType;
  /** The routing param for this item's detail route: an issue's full
   *  identifier ("LIF-42", since /issues/:identifier resolves by string),
   *  but a page/plan's numeric id-as-string (since /pages/:id and
   *  /plans/:id are numeric routes — their "LIF-DOC-3" style identifier
   *  isn't a valid route param). */
  routeId: string;
  /** Human-facing identifier shown in the UI (e.g. "LIF-DOC-3"). */
  identifier: string;
  title: string;
  /** Owning project's identifier (e.g. "LIF") — drives the route + chip. */
  project: string;
  /** Epoch ms of the most recent visit. */
  ts: number;
}

const KEY = "lific_recents";
const CAP = 15;

export function getRecents(): RecentEntry[] {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? (parsed as RecentEntry[]) : [];
  } catch {
    return [];
  }
}

/** Record (or bump) a visit. Dedupes on (type, routeId) — revisiting an
 *  item moves it to the front rather than duplicating — and caps the list
 *  at CAP so it never grows unbounded across a long session. */
export function recordRecent(entry: Omit<RecentEntry, "ts">): void {
  try {
    const rest = getRecents().filter(
      (e) => !(e.type === entry.type && e.routeId === entry.routeId),
    );
    const next: RecentEntry[] = [{ ...entry, ts: Date.now() }, ...rest].slice(
      0,
      CAP,
    );
    localStorage.setItem(KEY, JSON.stringify(next));
  } catch {
    // ignore — private mode / quota
  }
}
