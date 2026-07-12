// Shared view/interaction state for the issue list + board (LIF-99 Phase 3).
//
// This is a Svelte 5 `$state` class holding the UI control state that is
// otherwise scattered across IssueList.svelte and shared between the topbar,
// the keyboard handler, and the rows. It deliberately does NOT own the data
// layer (issues, project, modules, labels, loading, fetches, auto-refresh) —
// that stays in the component, which constructs one instance of this class
// and passes it to the extracted sub-components.
//
// Phase 3a (filters / sort / display / popovers) lands first; selection /
// focus / dropdown state migrates in a follow-up so each step stays
// build-verifiable.

import type { SortField, SortDir } from "./sort";
import { defaultSortDir } from "./sort";
import type { GroupBy, Density, LaneBy } from "./grouping";
import {
  loadListState,
  saveCollapsedGroups,
  loadCollapsedGroups,
  loadHiddenStatuses,
  saveHiddenStatuses,
  loadLaneBy,
  saveLaneBy,
  loadCollapsedLanes,
  saveCollapsedLanes,
  loadCollapsedColumns,
  saveCollapsedColumns,
} from "./persistence";
import { loadSubTab, saveSubTab } from "../subtab";
import { updateIssue, type Issue, type Module } from "../api";
import { toast } from "../toast/toast.svelte";

// LIF-308: issue-list content slices are deliberately separate from the
// persisted filter/sort/group state below. A tab narrows the current result;
// it must never overwrite the user's underlying view preferences.
export type IssueSubTab = "all" | "recent" | "open" | "closed";
export const ISSUE_SUB_TAB_IDS = ["all", "recent", "open", "closed"] as const;

function isIssueSubTab(id: string): id is IssueSubTab {
  return ISSUE_SUB_TAB_IDS.includes(id as IssueSubTab);
}

// ── LIF-243: undo layer for status/priority/module mutations ────────────
//
// These are plain exported functions rather than IssueListState methods —
// they're used from IssueList's row/keyboard/board handlers *and* from
// IssueDetail's sidebar, which doesn't have an IssueListState instance at
// all. Keeping them here (rather than inline in each call site) is what
// makes the toast text, the "what changed" detection, and the undo
// semantics consistent across every entry point instead of drifting.
//
// Every patch/prevPatch closure captures only primitive values (ids,
// plain objects) — never a component's local state — so an Undo button
// keeps working correctly even after the user has navigated to a
// different route and the component that created the toast is long gone.
// `onApplied` is the one place a caller's local state gets touched; if
// that component has since unmounted, calling its setter is a harmless
// no-op (nothing is listening to render it anymore).

export function capitalize(s: string): string {
  return s.length ? s[0].toUpperCase() + s.slice(1) : s;
}

/** Human summary of the single field that actually changed between `patch`
 *  and `prevPatch` — status wins if present (it's the field every entry
 *  point can produce), then module, then priority. Falls back to a generic
 *  "updated" for shapes we don't specifically describe (e.g. a same-value
 *  resend). */
export function describeIssueChange(
  patch: Record<string, unknown>,
  prevPatch: Record<string, unknown>,
  modules: Module[],
): string {
  if ("status" in patch && patch.status !== prevPatch.status) {
    return `→ ${capitalize(String(patch.status))}`;
  }
  if ("module_id" in patch && patch.module_id !== prevPatch.module_id) {
    const id = patch.module_id as number | null;
    const name = id == null ? "No module" : modules.find((m) => m.id === id)?.name ?? "a module";
    return `→ ${name}`;
  }
  if ("priority" in patch && patch.priority !== prevPatch.priority) {
    return `→ ${capitalize(String(patch.priority))} priority`;
  }
  return "updated";
}

/** Build the inverse of `patch` from an issue's current field values —
 *  only for the keys `patch` actually touches, so the resulting undo call
 *  is a minimal, field-for-field opposite. */
export function prevPatchFor(issue: Issue, patch: Record<string, unknown>): Record<string, unknown> {
  const prev: Record<string, unknown> = {};
  if ("status" in patch) prev.status = issue.status;
  if ("priority" in patch) prev.priority = issue.priority;
  if ("module_id" in patch) prev.module_id = issue.module_id;
  return prev;
}

/** Apply `patch` to one issue, then offer a single-shot Undo that re-applies
 *  `prevPatch` via the same `updateIssue` call. Returns whether the forward
 *  mutation succeeded so callers keep their existing failure branching
 *  (e.g. IssueList's board-drop rollback-by-reload). Errors route through
 *  an error toast either way — including for call sites that previously
 *  failed silently. */
export async function updateIssueWithUndo(opts: {
  id: number;
  identifier: string;
  patch: Record<string, unknown>;
  prevPatch: Record<string, unknown>;
  modules: Module[];
  /** Sync the caller's local issue list after the forward mutation AND
   *  after a successful undo (called again with `prevPatch` then). */
  onApplied?: (patch: Record<string, unknown>) => void;
}): Promise<boolean> {
  const res = await updateIssue(opts.id, opts.patch);
  if (!res.ok) {
    toast(`Couldn't update ${opts.identifier}: ${res.error}`, { kind: "error" });
    return false;
  }
  opts.onApplied?.(opts.patch);
  toast(`${opts.identifier} ${describeIssueChange(opts.patch, opts.prevPatch, opts.modules)}`, {
    kind: "success",
    action: {
      label: "Undo",
      fn: async () => {
        const undoRes = await updateIssue(opts.id, opts.prevPatch);
        if (undoRes.ok) {
          opts.onApplied?.(opts.prevPatch);
          toast(`Restored ${opts.identifier}`, { kind: "info", duration: 3000 });
        } else {
          toast(`Couldn't undo ${opts.identifier}: ${undoRes.error}`, { kind: "error" });
        }
      },
    },
  });
  return true;
}

/** Bulk variant for BulkActionBar's Status/Priority/Module actions: applies
 *  `patch` to every target, then offers one Undo that restores each issue
 *  to *its own* captured prior value (not a blanket revert to a single
 *  prior state — selections are usually mixed). Both directions use
 *  Promise.allSettled and report partial failure honestly rather than
 *  implying an all-or-nothing result. */
export async function bulkUpdateIssuesWithUndo(opts: {
  targets: { id: number; identifier: string; prevPatch: Record<string, unknown> }[];
  patch: Record<string, unknown>;
  modules: Module[];
  onApplied?: (patches: Map<number, Record<string, unknown>>) => void;
}): Promise<{ okIds: Set<number>; failedIds: Set<number> }> {
  const results = await Promise.allSettled(
    opts.targets.map((t) => updateIssue(t.id, opts.patch)),
  );
  const okIds = new Set<number>();
  const failedIds = new Set<number>();
  results.forEach((r, i) => {
    const t = opts.targets[i];
    if (r.status === "fulfilled" && r.value.ok) okIds.add(t.id);
    else failedIds.add(t.id);
  });

  if (okIds.size > 0) {
    const applied = new Map<number, Record<string, unknown>>();
    for (const id of okIds) applied.set(id, opts.patch);
    opts.onApplied?.(applied);
  }

  if (failedIds.size > 0) {
    toast(
      okIds.size > 0
        ? `Updated ${okIds.size} of ${opts.targets.length} issues`
        : `Couldn't update ${opts.targets.length} issue${opts.targets.length === 1 ? "" : "s"}`,
      { kind: "error" },
    );
  }

  if (okIds.size > 0) {
    const okTargets = opts.targets.filter((t) => okIds.has(t.id));
    const label =
      okTargets.length === 1
        ? okTargets[0].identifier
        : `${okTargets.length} issue${okTargets.length === 1 ? "" : "s"}`;
    toast(`${label} ${describeIssueChange(opts.patch, okTargets[0].prevPatch, opts.modules)}`, {
      kind: "success",
      action: {
        label: "Undo",
        fn: async () => {
          const undoResults = await Promise.allSettled(
            okTargets.map((t) => updateIssue(t.id, t.prevPatch)),
          );
          const restored = new Map<number, Record<string, unknown>>();
          let failCount = 0;
          undoResults.forEach((r, i) => {
            if (r.status === "fulfilled" && r.value.ok) {
              restored.set(okTargets[i].id, okTargets[i].prevPatch);
            } else {
              failCount++;
            }
          });
          if (restored.size > 0) opts.onApplied?.(restored);
          if (failCount > 0) {
            toast(`Restored ${restored.size} of ${okTargets.length}`, { kind: "error" });
          } else {
            toast(`Restored ${restored.size} issue${restored.size === 1 ? "" : "s"}`, {
              kind: "info",
              duration: 3000,
            });
          }
        },
      },
    });
  }

  return { okIds, failedIds };
}

export class IssueListState {
  // ── LIF-308: issue-list content slice ──
  issueSubTab = $state<IssueSubTab>("all");
  /** Numeric project id used by the shared sub-tab localStorage convention.
   *  Kept separately from the issue-view state key, which predates LIF-308
   *  and is keyed by project identifier. */
  private issueSubTabProjectId = $state<string | null>(null);

  // ── Filters ──
  filterStatus = $state("");
  filterPriority = $state("");
  filterLabel = $state("");
  filterModule = $state("");
  searchQuery = $state("");

  // ── Sort ──
  sortField = $state<SortField>("priority");
  sortDir = $state<SortDir>("asc"); // default: urgent first

  // ── Display (group + density) ──
  groupBy = $state<GroupBy>("status");
  density = $state<Density>("compact");
  // Collapsed group keys, namespaced `${groupBy}:${groupKey}` so the same
  // header collapsed under one grouping doesn't hide a same-named one under
  // another. Persisted per project.
  collapsedGroups = $state<Set<string>>(new Set());

  // ── Board: per-status column visibility ──
  hiddenStatuses = $state<Set<string>>(new Set());

  // ── Board: swimlanes (LIF-241) ──
  // Which dimension splits the board into horizontal bands, on top of the
  // status columns. "none" = today's flat board (single implicit lane).
  laneBy = $state<LaneBy>("none");
  /** Collapsed lane keys (module id / "none" / priority name). Unlike
   *  collapsedGroups these aren't namespaced by laneBy — switching lanes
   *  clears the set's *meaning* anyway (a module-id key means nothing under
   *  priority lanes), so the component just treats the current laneBy's
   *  keys as the ones that matter and stale keys from a prior laneBy are
   *  harmless (never matched, quietly forgotten on next save). */
  collapsedLanes = $state<Set<string>>(new Set());
  /** Collapsed status columns — board-wide, independent of lane. A
   *  collapsed column shrinks to a slim drop-target rail. */
  collapsedColumns = $state<Set<string>>(new Set());

  // ── Topbar popovers (only one open at a time, but tracked separately so
  //    the global click/Escape handlers can close whichever is open) ──
  searchExpanded = $state(false);
  displayOpen = $state(false);
  sortOpen = $state(false);
  newMenuOpen = $state(false);
  /** Unified filter popover (LIF-222). Replaces the previous row of four
   *  inline `<Select>` filter triggers. */
  filterOpen = $state(false);
  /** Swimlane-picker popover (LIF-241). Board mode only. */
  lanesOpen = $state(false);

  // ── Row interaction: keyboard focus, multi-select, inline dropdowns ──
  // Shared between the keyboard handler, the bulk handlers, and IssueRow.
  // The selection mutators (toggle/range/clear) stay in the component because
  // they depend on its `flatIssues` derived; they write these fields.
  focusedIndex = $state(-1);
  selectedIds = $state<Set<number>>(new Set());
  lastSelectedIdx = $state(-1);
  /** Issue id whose inline status picker is open (or null). */
  statusDropdownId = $state<number | null>(null);
  /** Issue id whose inline priority picker is open (or null). */
  priorityDropdownId = $state<number | null>(null);
  /** Issue id whose inline module picker is open (or null). LIF-245: the
   *  row-level module picker, opened via click or the `m` shortcut —
   *  mirrors status/priority. */
  moduleDropdownId = $state<number | null>(null);
  /** Highlighted index within an open status picker (shared by inline-create
   *  and row dropdowns). Kept under the original name to limit churn. */
  inlineCreateStatusIdx = $state(0);
  /** Highlighted index within an open priority picker (row dropdown only —
   *  there's no inline-create equivalent for priority). LIF-245. */
  priorityPickerIdx = $state(0);
  /** Highlighted index within an open module picker. Index 0 is always
   *  "No module"; index n+1 is `modules[n]`. LIF-245. */
  modulePickerIdx = $state(0);

  /** True once a hydrate pass has run, so the persist effect doesn't clobber
   *  storage with defaults before the stored values are loaded. */
  hydrated = $state(false);

  clearSelection(): void {
    this.selectedIds = new Set();
    this.lastSelectedIdx = -1;
  }

  // ── Filter helpers ──
  hasActiveFilters(): boolean {
    return !!(
      this.filterStatus ||
      this.filterPriority ||
      this.filterLabel ||
      this.filterModule
    );
  }

  clearFilters(): void {
    this.filterStatus = "";
    this.filterPriority = "";
    this.filterLabel = "";
    this.filterModule = "";
    this.searchQuery = "";
  }

  togglePriorityFilter(p: string): void {
    this.filterPriority = this.filterPriority === p ? "" : p;
  }

  toggleModuleFilter(name: string): void {
    this.filterModule = this.filterModule === name ? "" : name;
  }

  toggleStatusFilter(status: string): void {
    this.filterStatus = this.filterStatus === status ? "" : status;
  }

  toggleLabelFilter(name: string): void {
    this.filterLabel = this.filterLabel === name ? "" : name;
  }

  // ── Sort helper ──
  /** Select a sort field (default direction) or, if already active, flip
   *  direction. Mirrors the spreadsheet-column pattern. */
  selectSort(field: SortField): void {
    if (this.sortField === field) {
      this.sortDir = this.sortDir === "asc" ? "desc" : "asc";
    } else {
      this.sortField = field;
      this.sortDir = defaultSortDir(field);
    }
  }

  // ── Group collapse helpers (persisted) ──
  private groupCollapseKey(key: string): string {
    return `${this.groupBy}:${key}`;
  }

  isGroupCollapsed(key: string): boolean {
    return this.collapsedGroups.has(this.groupCollapseKey(key));
  }

  toggleGroupCollapsed(projectId: string, key: string): void {
    const k = this.groupCollapseKey(key);
    const next = new Set(this.collapsedGroups);
    if (next.has(k)) next.delete(k);
    else next.add(k);
    this.collapsedGroups = next;
    saveCollapsedGroups(projectId, next);
  }

  // ── Board column visibility (persisted) ──
  toggleStatusVisibility(projectId: string, status: string): void {
    const next = new Set(this.hiddenStatuses);
    if (next.has(status)) next.delete(status);
    else next.add(status);
    this.hiddenStatuses = next;
    saveHiddenStatuses(projectId, next);
  }

  // ── Board swimlanes (persisted, LIF-241) ──
  setLaneBy(projectId: string, laneBy: LaneBy): void {
    this.laneBy = laneBy;
    saveLaneBy(projectId, laneBy);
  }

  isLaneCollapsed(key: string): boolean {
    return this.collapsedLanes.has(key);
  }

  toggleLaneCollapsed(projectId: string, key: string): void {
    const next = new Set(this.collapsedLanes);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    this.collapsedLanes = next;
    saveCollapsedLanes(projectId, next);
  }

  // ── Board collapsed columns (persisted, LIF-241) ──
  isColumnCollapsed(status: string): boolean {
    return this.collapsedColumns.has(status);
  }

  toggleColumnCollapsed(projectId: string, status: string): void {
    const next = new Set(this.collapsedColumns);
    if (next.has(status)) next.delete(status);
    else next.add(status);
    this.collapsedColumns = next;
    saveCollapsedColumns(projectId, next);
  }

  // ── Popover helpers ──
  /** Close every topbar popover. Used by the global click + Escape paths. */
  closePopovers(): void {
    this.displayOpen = false;
    this.sortOpen = false;
    this.newMenuOpen = false;
    this.filterOpen = false;
    this.lanesOpen = false;
  }

  /** Count of active filters, for the topbar Filter button badge. */
  activeFilterCount(): number {
    let n = 0;
    if (this.filterStatus) n++;
    if (this.filterPriority) n++;
    if (this.filterLabel) n++;
    if (this.filterModule) n++;
    return n;
  }

  // ── LIF-308 sub-tab persistence ──
  /** Reset while a new project resolves so its previous tab cannot flash. */
  resetIssueSubTab(): void {
    this.issueSubTab = "all";
    this.issueSubTabProjectId = null;
  }

  /** Load a project's saved content slice without writing a default. */
  hydrateIssueSubTab(projectId: string): void {
    this.issueSubTabProjectId = projectId;
    this.issueSubTab = (loadSubTab("issues", projectId, ISSUE_SUB_TAB_IDS) ?? "all") as IssueSubTab;
  }

  /** Save only an explicit user selection; hydrate/reset never persist. */
  selectIssueSubTab(id: string): void {
    if (!isIssueSubTab(id)) return;
    this.issueSubTab = id;
    if (this.issueSubTabProjectId) {
      saveSubTab("issues", this.issueSubTabProjectId, id);
    }
  }

  // ── Persistence wiring ──
  /** Hydrate filters/sort/display + per-project collapsed/hidden sets from
   *  localStorage. Sets `hydrated` so the persist effect can start. */
  hydrate(projectId: string): void {
    const s = loadListState(projectId);
    this.filterStatus = s.filterStatus ?? "";
    this.filterPriority = s.filterPriority ?? "";
    this.filterLabel = s.filterLabel ?? "";
    this.filterModule = s.filterModule ?? "";
    this.searchQuery = s.searchQuery ?? "";
    if (s.sortField) this.sortField = s.sortField;
    if (s.sortDir) this.sortDir = s.sortDir;
    if (s.groupBy) this.groupBy = s.groupBy;
    if (s.density) this.density = s.density;
    this.collapsedGroups = loadCollapsedGroups(projectId);
    this.hiddenStatuses = loadHiddenStatuses(projectId);
    this.laneBy = loadLaneBy(projectId);
    this.collapsedLanes = loadCollapsedLanes(projectId);
    this.collapsedColumns = loadCollapsedColumns(projectId);
    this.hydrated = true;
  }

  /** Snapshot of the persisted view-state slice (for saveListState). */
  snapshot() {
    return {
      filterStatus: this.filterStatus,
      filterPriority: this.filterPriority,
      filterLabel: this.filterLabel,
      filterModule: this.filterModule,
      searchQuery: this.searchQuery,
      sortField: this.sortField,
      sortDir: this.sortDir,
      groupBy: this.groupBy,
      density: this.density,
    };
  }
}
