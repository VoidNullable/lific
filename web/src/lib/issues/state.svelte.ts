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
import type { GroupBy, Density } from "./grouping";
import {
  loadListState,
  saveCollapsedGroups,
  loadCollapsedGroups,
  loadHiddenStatuses,
  saveHiddenStatuses,
} from "./persistence";

export class IssueListState {
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

  // ── Topbar popovers (only one open at a time, but tracked separately so
  //    the global click/Escape handlers can close whichever is open) ──
  searchExpanded = $state(false);
  hintsOpen = $state(false);
  displayOpen = $state(false);
  sortOpen = $state(false);
  newMenuOpen = $state(false);
  /** Unified filter popover (LIF-222). Replaces the previous row of four
   *  inline `<Select>` filter triggers. */
  filterOpen = $state(false);

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
  /** Highlighted index within an open status picker (shared by inline-create
   *  and row dropdowns). Kept under the original name to limit churn. */
  inlineCreateStatusIdx = $state(0);

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

  // ── Popover helpers ──
  /** Close every topbar popover. Used by the global click + Escape paths. */
  closePopovers(): void {
    this.hintsOpen = false;
    this.displayOpen = false;
    this.sortOpen = false;
    this.newMenuOpen = false;
    this.filterOpen = false;
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
