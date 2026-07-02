// LIF-242: saved views — named filter/group/sort/display presets per
// project, personal to each user. This module owns:
//
//   - the `ViewConfig` shape: the portable, savable subset of the state
//     `IssueListState` + persistence.ts already track per project
//   - snapshotting live state into a config (`buildConfig`) and applying a
//     config back onto live state (`applyConfig`) — the latter writes
//     through the exact setters/localStorage helpers persistence.ts already
//     uses for manual changes, so everything downstream (the board's
//     swimlane picker, IssueDetail's back-arrow layout memory) reacts
//     identically to a saved view being applied vs. the user changing
//     things by hand
//   - `configsDiffer`, which drives the "this view has unsaved changes" dot
//   - `shouldAutoApplyDefault`, the once-per-project-per-browser-tab-session
//     gate for auto-applying the user's default view on first visit
//
// Deliberately excludes ephemeral UI state from `ViewConfig` — selection,
// keyboard focus, which popover is open, which group/lane headers are
// collapsed — the same split `IssueListState` itself already draws between
// "view" state and "interaction" state.

import type { IssueListState } from "./state.svelte";
import type { SortField, SortDir } from "./sort";
import type { GroupBy, Density, LaneBy } from "./grouping";
import { saveListState, saveLayout, saveHiddenStatuses } from "./persistence";
import {
  listSavedViews,
  createSavedView,
  updateSavedView,
  deleteSavedView,
  type SavedView,
} from "../api";

export type Layout = "list" | "board";

export interface ViewConfig {
  /** Bumped only on a breaking shape change. `parseConfig` tolerates
   *  missing/foreign fields regardless, so older saved views never hard-fail
   *  to apply — this is just a marker for future migrations, unused today. */
  version: 1;
  layout: Layout;
  filterStatus: string;
  filterPriority: string;
  filterLabel: string;
  filterModule: string;
  searchQuery: string;
  sortField: SortField;
  sortDir: SortDir;
  groupBy: GroupBy;
  density: Density;
  /** Board swimlane dimension (LIF-241). Meaningless in list mode but kept
   *  in the config anyway — switching a saved board view back and forth
   *  from list mode shouldn't lose the lane choice. */
  laneBy: LaneBy;
  /** Board hidden status columns — unlike collapsed lanes/columns (pure UI
   *  chrome), which statuses are hidden meaningfully changes what's visible,
   *  so it travels with the view the same way a filter does. */
  hiddenStatuses: string[];
}

const FIELDS_COMPARED: (keyof ViewConfig)[] = [
  "filterStatus",
  "filterPriority",
  "filterLabel",
  "filterModule",
  "searchQuery",
  "sortField",
  "sortDir",
  "groupBy",
  "density",
  "laneBy",
];

/** Snapshot the current view into a config, ready to `JSON.stringify` into
 *  a saved view's `config` column. */
export function buildConfig(view: IssueListState, layout: Layout): ViewConfig {
  return {
    version: 1,
    layout,
    filterStatus: view.filterStatus,
    filterPriority: view.filterPriority,
    filterLabel: view.filterLabel,
    filterModule: view.filterModule,
    searchQuery: view.searchQuery,
    sortField: view.sortField,
    sortDir: view.sortDir,
    groupBy: view.groupBy,
    density: view.density,
    laneBy: view.laneBy,
    hiddenStatuses: [...view.hiddenStatuses],
  };
}

/** Parse a saved view's opaque `config` string back into a `ViewConfig`,
 *  tolerating missing/malformed fields (falls back to the same defaults
 *  `IssueListState` itself uses) so a view saved before a field existed —
 *  or a hand-edited row — never crashes on apply. Returns `null` only when
 *  the string isn't parseable JSON at all (shouldn't happen — the backend
 *  validates that on write — but a defensive parse costs nothing). */
export function parseConfig(raw: string): ViewConfig | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (!parsed || typeof parsed !== "object") return null;
  const p = parsed as Record<string, unknown>;
  const str = (v: unknown, fallback: string) => (typeof v === "string" ? v : fallback);
  return {
    version: 1,
    layout: p.layout === "board" ? "board" : "list",
    filterStatus: str(p.filterStatus, ""),
    filterPriority: str(p.filterPriority, ""),
    filterLabel: str(p.filterLabel, ""),
    filterModule: str(p.filterModule, ""),
    searchQuery: str(p.searchQuery, ""),
    sortField: (["priority", "age", "number", "updated"] as const).includes(p.sortField as SortField)
      ? (p.sortField as SortField)
      : "priority",
    sortDir: p.sortDir === "desc" ? "desc" : "asc",
    groupBy: (["status", "priority", "module", "none"] as const).includes(p.groupBy as GroupBy)
      ? (p.groupBy as GroupBy)
      : "status",
    density: p.density === "comfortable" ? "comfortable" : "compact",
    laneBy: p.laneBy === "module" || p.laneBy === "priority" ? p.laneBy : "none",
    hiddenStatuses: Array.isArray(p.hiddenStatuses)
      ? p.hiddenStatuses.filter((s): s is string => typeof s === "string")
      : [],
  };
}

/** Apply a config onto live state, writing through the same
 *  setters/localStorage helpers a manual change would use. Returns the
 *  config's layout so the caller (Topbar owns `navigate`) can route to
 *  `/issues` or `/board` if it differs from the currently rendered layout —
 *  this module has no navigation concerns of its own. */
export function applyConfig(
  view: IssueListState,
  projectIdentifier: string,
  config: ViewConfig,
): Layout {
  view.filterStatus = config.filterStatus;
  view.filterPriority = config.filterPriority;
  view.filterLabel = config.filterLabel;
  view.filterModule = config.filterModule;
  view.searchQuery = config.searchQuery;
  view.sortField = config.sortField;
  view.sortDir = config.sortDir;
  view.groupBy = config.groupBy;
  view.density = config.density;
  view.setLaneBy(projectIdentifier, config.laneBy); // persists laneBy itself
  view.hiddenStatuses = new Set(config.hiddenStatuses);
  saveHiddenStatuses(projectIdentifier, view.hiddenStatuses);
  saveListState(projectIdentifier, view.snapshot());
  saveLayout(projectIdentifier, config.layout);
  return config.layout;
}

/** True when the live state (+ current layout) has drifted from a saved
 *  view's stored config — drives Topbar's "unsaved changes" dot next to the
 *  active view's name. `layout` participates: switching from list to board
 *  (or back) is itself a config change a saved view can capture, same as
 *  changing the sort field. */
export function configsDiffer(a: ViewConfig, b: ViewConfig): boolean {
  if (a.layout !== b.layout) return true;
  if (FIELDS_COMPARED.some((k) => a[k] !== b[k])) return true;
  const ah = [...a.hiddenStatuses].sort();
  const bh = [...b.hiddenStatuses].sort();
  return ah.length !== bh.length || ah.some((s, i) => s !== bh[i]);
}

// ── Backend round trips ───────────────────────────────────────────────
// Thin wrappers over ../api's raw HTTP calls: JSON-(de)serialize `config`
// so callers (SavedViews.svelte) work with typed `ViewConfig` objects, not
// strings.

export async function fetchViews(projectId: number) {
  return listSavedViews(projectId);
}

export async function saveNewView(
  projectId: number,
  name: string,
  config: ViewConfig,
  isDefault = false,
) {
  return createSavedView(projectId, {
    name,
    config: JSON.stringify(config),
    is_default: isDefault,
  });
}

export async function saveViewUpdate(
  projectId: number,
  viewId: number,
  patch: { name?: string; config?: ViewConfig; is_default?: boolean },
) {
  return updateSavedView(projectId, viewId, {
    name: patch.name,
    config: patch.config ? JSON.stringify(patch.config) : undefined,
    is_default: patch.is_default,
  });
}

export async function removeView(projectId: number, viewId: number) {
  return deleteSavedView(projectId, viewId);
}

export type { SavedView };

// ── Once-per-project-per-tab-session default-view auto-apply ───────────
// `IssueListState.hydrate()` (state.svelte.ts, outside this feature's
// reach this session) already loads whatever was in localStorage for the
// project when the route mounts — filters/sort/display + the per-project
// collapsed/hidden sets. `SavedViews.svelte` calls
// `shouldAutoApplyDefault(projectIdentifier)` once `view.hydrated` is true;
// if it returns `true`, it fetches the view list, looks for
// `is_default`, and if found applies it via `applyConfig` — overwriting
// whatever `hydrate()` just loaded from localStorage with the user's
// explicit default. The sessionStorage flag makes this a one-shot per
// (project, tab session): remounting Topbar (e.g. toggling list <-> board
// within the same project) never re-triggers it, so manual changes made
// later in the same session survive a later remount within that project.
const sessionCheckedKey = (projectId: string) => `lific:views:session-checked:${projectId}`;

export function shouldAutoApplyDefault(projectIdentifier: string): boolean {
  try {
    if (sessionStorage.getItem(sessionCheckedKey(projectIdentifier))) return false;
    sessionStorage.setItem(sessionCheckedKey(projectIdentifier), "1");
    return true;
  } catch {
    // Storage unavailable (private mode / quota) — never auto-apply rather
    // than risk retrying (and re-fetching) on every render.
    return false;
  }
}

// ── Active view tracking (which saved view, if any, is "live") ─────────
// Session-scoped (not persisted across browser restarts — a saved view is
// a preset to re-apply, not a sticky mode) so navigating between issue
// detail and the list within the same tab keeps showing the right name in
// the Topbar trigger without an extra round trip.
const activeViewKey = (projectId: string) => `lific:views:active:${projectId}`;

export function getActiveViewId(projectIdentifier: string): number | null {
  try {
    const raw = sessionStorage.getItem(activeViewKey(projectIdentifier));
    return raw ? Number(raw) : null;
  } catch {
    return null;
  }
}

export function setActiveViewId(projectIdentifier: string, id: number | null): void {
  try {
    if (id == null) sessionStorage.removeItem(activeViewKey(projectIdentifier));
    else sessionStorage.setItem(activeViewKey(projectIdentifier), String(id));
  } catch {
    // ignore
  }
}
