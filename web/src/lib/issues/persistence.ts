// Per-project persistence for the issue list/board view. Extracted from
// IssueList.svelte (LIF-99) so every localStorage key and its
// (de)serialization lives in one place rather than scattered across the
// component's effects.
//
// Four independent slices are persisted, each namespaced by project id:
//   - view state  (filters, search, sort, group, density)  lific:list:state:<id>
//   - layout      (list vs board, for IssueDetail's back arrow) lific:list:layout:<id>
//   - collapsed groups                                      lific:list:collapsed:<id>
//   - board hidden status columns                           lific:board:hidden-statuses:<id>
//
// Every accessor swallows storage errors (private mode / quota) and falls
// back to a sensible default, matching the component's prior behavior.

import type { SortField, SortDir } from "./sort";
import type { GroupBy, Density } from "./grouping";

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
