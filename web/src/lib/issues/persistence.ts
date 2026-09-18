// Per-project persistence for the issue list/board view. Extracted from
// IssueList.svelte (LIF-99) so every localStorage key and its
// (de)serialization lives in one place rather than scattered across the
// component's effects.
//
// Seven independent slices are persisted, each namespaced by project id:
//   - view state  (filters, search, sort, group, density)  lific:list:state:<id>
//   - layout      (list vs board, for IssueDetail's back arrow) lific:list:layout:<id>
//   - collapsed groups                                      lific:list:collapsed:<id>
//   - board hidden status columns                           lific:board:hidden-statuses:<id>
//   - board swimlane dimension (LIF-241)                    lific:board:lanes:<id>
//   - board collapsed lanes (LIF-241)                       lific:board:collapsed-lanes:<id>
//   - board collapsed status columns (LIF-241)              lific:board:collapsed-columns:<id>
//
// Every accessor swallows storage errors (private mode / quota) and falls
// back to a sensible default, matching the component's prior behavior.

import type { SortField, SortDir } from "./sort";
import type { GroupBy, Density, LaneBy } from "./grouping";

export type PersistedListState = {
  filterStatus?: string;
  filterPriority?: string;
  filterLabel?: string;
  filterModule?: string;
  searchQuery?: string;
  sortField?: SortField;
  sortDir?: SortDir;
  groupBy?: GroupBy;
  density?: Density;
};

const stateKey = (id: string) => `lific:list:state:${id}`;
const layoutKey = (id: string) => `lific:list:layout:${id}`;
const collapsedKey = (id: string) => `lific:list:collapsed:${id}`;
const hiddenStatusesKey = (id: string) => `lific:board:hidden-statuses:${id}`;
const laneByKey = (id: string) => `lific:board:lanes:${id}`;
const collapsedLanesKey = (id: string) => `lific:board:collapsed-lanes:${id}`;
const collapsedColumnsKey = (id: string) =>
  `lific:board:collapsed-columns:${id}`;

// ── View state (filters / search / sort / group / density) ──

/** Read the persisted view state for a project. Returns {} when nothing is
 *  stored or storage is unavailable, so callers can spread defaults over it. */
export function loadListState(id: string): PersistedListState {
  try {
    const raw = localStorage.getItem(stateKey(id));
    if (raw) return JSON.parse(raw) as PersistedListState;
  } catch {
    // ignore
  }
  return {};
}

/** Persist the view state for a project. Silently no-ops on storage failure. */
export function saveListState(id: string, snapshot: PersistedListState): void {
  try {
    localStorage.setItem(stateKey(id), JSON.stringify(snapshot));
  } catch {
    // ignore
  }
}

// ── Layout (list vs board) ──

/** Layout the project's issue list was last using. Drives IssueDetail's
 *  back arrow and the synthesized back entry for deep-linked issue views
 *  (App.svelte, LIF-434). */
export function loadLayout(id: string): "board" | "list" {
  try {
    const raw = localStorage.getItem(layoutKey(id));
    if (raw === "board" || raw === "list") return raw;
  } catch {
    // ignore
  }
  return "list";
}

export function saveLayout(id: string, layout: string): void {
  try {
    localStorage.setItem(layoutKey(id), layout);
  } catch {
    // ignore
  }
}

// ── Collapsed group keys ──

export function loadCollapsedGroups(id: string): Set<string> {
  try {
    const raw = localStorage.getItem(collapsedKey(id));
    return raw ? new Set(JSON.parse(raw) as string[]) : new Set();
  } catch {
    return new Set();
  }
}

export function saveCollapsedGroups(id: string, keys: Set<string>): void {
  try {
    localStorage.setItem(collapsedKey(id), JSON.stringify([...keys]));
  } catch {
    // ignore
  }
}

// ── Board hidden status columns ──

export function loadHiddenStatuses(id: string): Set<string> {
  try {
    const raw = localStorage.getItem(hiddenStatusesKey(id));
    return raw ? new Set(JSON.parse(raw) as string[]) : new Set();
  } catch {
    return new Set();
  }
}

export function saveHiddenStatuses(id: string, statuses: Set<string>): void {
  try {
    localStorage.setItem(hiddenStatusesKey(id), JSON.stringify([...statuses]));
  } catch {
    // localStorage can fail in private mode / quota — silently degrade to
    // in-memory state, which is fine for the rest of the session.
  }
}

// ── Board swimlanes (LIF-241) ──

export function loadLaneBy(id: string): LaneBy {
  try {
    const raw = localStorage.getItem(laneByKey(id));
    if (raw === "module" || raw === "priority" || raw === "none") return raw;
  } catch {
    // ignore
  }
  return "none";
}

export function saveLaneBy(id: string, laneBy: LaneBy): void {
  try {
    localStorage.setItem(laneByKey(id), laneBy);
  } catch {
    // ignore
  }
}

export function loadCollapsedLanes(id: string): Set<string> {
  try {
    const raw = localStorage.getItem(collapsedLanesKey(id));
    return raw ? new Set(JSON.parse(raw) as string[]) : new Set();
  } catch {
    return new Set();
  }
}

export function saveCollapsedLanes(id: string, keys: Set<string>): void {
  try {
    localStorage.setItem(collapsedLanesKey(id), JSON.stringify([...keys]));
  } catch {
    // ignore
  }
}

// ── Board collapsed status columns (LIF-241) ──
// Distinct from hiddenStatuses above: hidden columns disappear entirely
// (the "Columns" visibility pills); collapsed columns stay on the board as
// a slim drop-target rail. Collapse is board-wide per status, independent
// of whichever lane it's rendered under.

export function loadCollapsedColumns(id: string): Set<string> {
  try {
    const raw = localStorage.getItem(collapsedColumnsKey(id));
    return raw ? new Set(JSON.parse(raw) as string[]) : new Set();
  } catch {
    return new Set();
  }
}

export function saveCollapsedColumns(id: string, statuses: Set<string>): void {
  try {
    localStorage.setItem(
      collapsedColumnsKey(id),
      JSON.stringify([...statuses]),
    );
  } catch {
    // ignore
  }
}
