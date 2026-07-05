<script lang="ts">
  import {
    listIssues,
    listProjects,
    listModules,
    listLabels,
    updateIssue,
    createIssue,
    getIssueCounts,
    type IssueStatusCounts,
    type Issue,
    type Project,
    type Module,
    type Label,
  } from "../lib/api";
  import { Plus, ChevronRight, Layers, PanelLeftClose, PanelLeftOpen } from "lucide-svelte";
  import Tooltip from "../lib/Tooltip.svelte";
  import PriorityIcon from "../lib/PriorityIcon.svelte";
  import StatusIcon from "../lib/StatusIcon.svelte";
  import ProjectIcon from "../lib/ProjectIcon.svelte";
  import Mascot from "../lib/Mascot.svelte";
  import ErrorState from "../lib/ErrorState.svelte";
  import Skeleton from "../lib/Skeleton.svelte";
  import { dndzone, type DndEvent } from "svelte-dnd-action";
  import { flip } from "svelte/animate";
  import { slide } from "svelte/transition";
  import { motionReduced } from "../lib/theme";
  import { getContext } from "svelte";
  import { startAutoRefresh } from "../lib/autoRefresh.svelte";
  import { compareIssues as compareIssuesPure } from "../lib/issues/sort";
  import { computeSearchResult, RESULT_CAP } from "../lib/issues/search";
  import {
    STATUSES, PRIORITIES, buildGroups, STATUS_UNRESOLVED, isUnresolved,
    buildLanes, laneKeyForIssue,
  } from "../lib/issues/grouping";
  import { saveListState, saveLayout } from "../lib/issues/persistence";
  import IssueCard from "../lib/issues/IssueCard.svelte";
  import BulkActionBar, {
    type BulkMenu,
  } from "../lib/issues/BulkActionBar.svelte";
  import RightSidebar from "../lib/issues/RightSidebar.svelte";
  import IssueRow from "../lib/issues/IssueRow.svelte";
  import Topbar from "../lib/issues/Topbar.svelte";
  import { peekState, openPeek, registerPeekSync } from "../lib/issues/peek.svelte"; // LIF-244 / LIF-248
  import {
    IssueListState,
    updateIssueWithUndo,
    bulkUpdateIssuesWithUndo,
    prevPatchFor,
  } from "../lib/issues/state.svelte"; // LIF-243: undo layer
  import { scheduleDelete, hasPendingDeletes } from "../lib/issues/deferredDelete.svelte"; // LIF-283
  import { shortcutsSuppressed } from "../lib/shortcuts"; // LIF-245
  import { shortcutHelpState } from "../lib/shortcutHelp.svelte"; // LIF-245
  import { commandPaletteState } from "../lib/commandPaletteState.svelte"; // LIF-245
  import { projectRole, loadProjectRole } from "../lib/projectRole.svelte"; // LIF-234

  const topbarCtx = getContext<{
    set: (s: import("svelte").Snippet | undefined) => void;
  } | undefined>("lific:topbar");

  // Register our toolbar with Layout's chrome topbar slot. Clear it on
  // unmount so the next route doesn't inherit our content.
  $effect(() => {
    topbarCtx?.set(topbarContent);
    return () => topbarCtx?.set(undefined);
  });

  let {
    navigate,
    projectIdentifier,
    layout = "list",
  }: {
    navigate: (path: string) => void;
    projectIdentifier: string;
    /** Render mode. Both modes share data loading, filters, sort, and
     *  topbar; only the body below the chrome differs. */
    layout?: "list" | "board";
  } = $props();

  let project = $state<Project | null>(null);
  // `issues` is the FULL, unfiltered project set (capped at 1000). Status /
  // priority / label / module filters are applied client-side downstream
  // (LIF-222 perf), so this array always reflects the whole project — which
  // is exactly what the right sidebar's project-wide breakdowns (LIF-186)
  // need, no separate unfiltered fetch required.
  let issues = $state<Issue[]>([]);
  // LIF-161: true per-status tallies from the server. The fetched `issues`
  // array is limit-capped, so its length is NOT a reliable count — this is.
  let issueCounts = $state<IssueStatusCounts | null>(null);
  let modules = $state<Module[]>([]);
  let labels = $state<Label[]>([]);
  let loading = $state(true);
  let error = $state("");

  // LIF-99 Phase 3: shared view/interaction state lives in a $state class.
  // The component still owns the data layer (issues, project, fetches).
  const view = new IssueListState();

  // LIF-234: content mutation gate. When false (a viewer, enforcement on),
  // hide create affordances, disable board drag, and drop the inline
  // status/priority/bulk controls — the server denies these anyway, this
  // just stops offering them. True while enforcement is off / for admins /
  // maintainer+.
  const canEdit = $derived(projectRole.canEdit);

  // LIF-246: shared duration for every animate:flip / dndzone
  // flipDurationMs in this component (list-row reorder, board card
  // drag-reorder/status-move, collapsed-column drop rail). Checked fresh
  // at the moment each flip/drag fires (not memoized) so a live toggle of
  // the appearance system's motion preference takes effect on the very
  // next reorder — same call-fresh pattern as Toaster/PeekPanel's
  // enterParams()/panelInParams().
  function flipMs(): number {
    return motionReduced() ? 0 : 150;
  }

  // LIF-246: group expand/collapse. `slide` animates height/padding (not
  // transform), so it's containing-block-safe by construction — no risk
  // to any fixed-position Select/Tooltip content nested in the rows it
  // reveals (see the KNOWN TRAP note on transform-holding containers).
  function slideParams() {
    return motionReduced() ? { duration: 0 } : { duration: 150 };
  }

  function priorityCssColor(p: string): string {
    switch (p) {
      case "urgent": return "var(--error)";
      case "high": return "var(--warn)";
      case "medium": return "var(--accent)";
      case "low": return "var(--text-muted)";
      case "none": return "var(--text-faint)";
      default: return "var(--text-faint)";
    }
  }

  // LIF-186: project-wide breakdowns for the right sidebar. Computed from the
  // unfiltered `issues` so the distribution and per-module counts reflect the
  // whole project, not the currently filtered view.
  let sidebarStats = $derived.by(() => {
    const prio: Record<string, number> = { urgent: 0, high: 0, medium: 0, low: 0, none: 0 };
    const byModule = new Map<number, number>();
    let noModule = 0;
    for (const i of issues) {
      prio[i.priority] = (prio[i.priority] ?? 0) + 1;
      if (i.module_id == null) noModule++;
      else byModule.set(i.module_id, (byModule.get(i.module_id) ?? 0) + 1);
    }
    return {
      prio,
      byModule,
      noModule,
      total: issueCounts?.total ?? issues.length,
      active: issueCounts?.active ?? 0,
    };
  });

  // ── Persisted list/board view state ──────────────────
  // Filters, search, and sort are remembered per-project so navigating
  // away (e.g. into an issue detail) and back doesn't reset the view.
  // Layout (list vs board) is remembered too so IssueDetail's back arrow
  // knows where to send the user. The view state itself lives on `view`;
  // these effects drive its hydrate/snapshot against localStorage.

  // Re-run when the project prop changes (read it synchronously so Svelte tracks it)
  $effect(() => {
    const id = projectIdentifier;
    view.hydrated = false;
    // view.hydrate loads filters/sort/display + collapsed/hidden sets, and
    // flips view.hydrated so the persist effect can start.
    view.hydrate(id);
    loadProject(id);
  });

  // Remember which layout the user is on so IssueDetail's back arrow
  // returns to the right route (/board vs /issues).
  $effect(() => {
    saveLayout(projectIdentifier, layout);
  });

  // Persist filter/sort/search state on change. Gated on view.hydrated
  // to avoid clobbering storage with defaults during the hydrate pass.
  $effect(() => {
    const id = projectIdentifier;
    const snapshot = view.snapshot();
    if (!view.hydrated) return;
    saveListState(id, snapshot);
  });

  // NOTE: filters no longer trigger a fetch. They're applied client-side in
  // `controlFilteredIssues` (LIF-222 perf), so changing a filter is a pure
  // in-memory recompute — instant, no round-trip. The full set is (re)loaded
  // on mount/navigation and by the 15s auto-refresh poll.

  async function loadProject(identifier: string) {
    loading = true;
    error = "";
    // Don't let the previous project's tallies linger while we fetch.
    issueCounts = null;
    // LIF-245: a stale keyboard focus/lastFocusedId from the previous
    // project could otherwise "resurrect" onto a same-numbered issue id in
    // the new project once its issues load (see the flatIssues-relocate
    // effect below) — ids are global, so this is astronomically unlikely
    // in practice, but free to rule out entirely on a project switch.
    view.focusedIndex = -1;
    lastFocusedId = null;
    const projRes = await listProjects();
    if (!projRes.ok) {
      error = projRes.error;
      loading = false;
      return;
    }

    const found = projRes.data.find(
      (p: Project) => p.identifier.toLowerCase() === identifier.toLowerCase()
    );
    if (!found) {
      error = `Project ${identifier} not found`;
      loading = false;
      return;
    }
    project = found;
    loadProjectRole(found.id); // LIF-234: prime role gating for this project

    // Load modules, labels, and issues in parallel
    const [modRes, lblRes] = await Promise.all([
      listModules(found.id),
      listLabels(found.id),
    ]);

    if (modRes.ok) modules = modRes.data;
    if (lblRes.ok) labels = lblRes.data;

    await loadIssues();
    loading = false;
  }

  async function loadIssues() {
    if (!project) return;

    // LIF-222 perf: always fetch the FULL, unfiltered issue set. Status /
    // priority / label / module filtering is applied client-side in the
    // derived pipeline (see `controlFilteredIssues`), exactly like search —
    // so toggling a filter is instant with zero network. The server fetch
    // only runs on mount and the 15s poll.
    //
    // LIF-161: bounded at 1000 so a huge project can't pull megabytes of
    // descriptions per poll; the topbar tallies come from the counts
    // endpoint, not from this fetch. Counts ride along so the topbar
    // converges with the rows.
    const [res, countsRes] = await Promise.all([
      listIssues({ project_id: project.id, limit: 1000 }),
      getIssueCounts(project.id),
    ]);
    if (res.ok) {
      issues = res.data;
    }
    if (countsRes.ok) {
      issueCounts = countsRes.data;
    }
  }

  // ── LIF-129: auto-refresh ────────────────────────────
  // Background poll (15s) + revalidate on tab focus so the list/board
  // converges on server state after out-of-band changes (MCP agent, API,
  // another tab). Both modes share this one loop — it's the same dataset.
  //
  // `mutationsInFlight` pauses refresh while a write is pending so a poll
  // can't land stale server data on top of an optimistic change that the
  // PUT hasn't acknowledged yet (the card-snaps-back race). `dragActive`
  // covers the drag itself. Open popovers / inline create / a focused
  // search box also veto a tick so we never yank UI out from under input.
  let mutationsInFlight = $state(0);
  let dragActive = $state(false);

  async function trackMutation<T>(p: Promise<T>): Promise<T> {
    mutationsInFlight++;
    try {
      return await p;
    } finally {
      mutationsInFlight--;
    }
  }

  function autoRefreshBusy(): boolean {
    return (
      // A mount/navigation load is already in flight — don't stack another
      // full fetch on top of it (matters a lot on a high-latency link).
      loading ||
      dragActive ||
      mutationsInFlight > 0 ||
      view.sortOpen ||
      view.displayOpen ||
      view.newMenuOpen ||
      view.filterOpen ||
      view.lanesOpen ||
      peekState.open ||
      // LIF-245: a poll landing under the command palette or the shortcut
      // help overlay wouldn't corrupt anything visible (both are opaque
      // modals over the list), but it would still burn a network round
      // trip and reset keyboard focus/selection pointlessly the moment
      // either closes — same reasoning as the peek-open veto above.
      commandPaletteState.open ||
      shortcutHelpState.open ||
      inlineCreateActive ||
      view.statusDropdownId !== null ||
      view.priorityDropdownId !== null ||
      view.moduleDropdownId !== null ||
      // LIF-149: a poll mustn't shuffle rows mid-selection or land stale
      // data on top of an in-flight bulk write.
      view.selectedIds.size > 0 ||
      bulkBusy ||
      // LIF-283: a deferred delete has optimistically removed rows the server
      // still has; a poll would resurrect them until the commit fires.
      hasPendingDeletes() ||
      // Don't refetch while the user is typing in the search box.
      (view.searchExpanded && document.activeElement === searchInputEl)
    );
  }

  // Refresh just the issue rows. Modules/labels feed the filter dropdowns
  // and change rarely; a full project reload on every tick would be
  // wasteful and could flash the loading spinner, so we only re-pull
  // issues here. New modules/labels reconcile on the next mount/navigation.
  async function refreshIssues() {
    if (!project) return;
    await loadIssues();
  }

  $effect(() =>
    startAutoRefresh({
      refresh: refreshIssues,
      isBusy: autoRefreshBusy,
      intervalMs: 15_000,
    }),
  );

  // LIF-222 perf: status / priority / label / module filtering applied
  // client-side over the full in-memory set. This used to be a server fetch
  // per filter change (2-3 round-trips of up to 1000 full records each);
  // doing it in-memory makes filter toggles instant and lets optimistic row
  // edits (status/priority cycle, bulk) re-derive without a refetch. Module
  // is matched by name → id via the loaded `modules` so the stored filter
  // value (a name) still works.
  let controlFilteredIssues = $derived.by(() => {
    let out = issues;
    if (view.filterStatus === STATUS_UNRESOLVED) {
      // "Unresolved" group: everything not in a terminal state.
      out = out.filter((i) => isUnresolved(i.status));
    } else if (view.filterStatus) {
      out = out.filter((i) => i.status === view.filterStatus);
    }
    if (view.filterPriority) out = out.filter((i) => i.priority === view.filterPriority);
    if (view.filterLabel) out = out.filter((i) => i.labels.includes(view.filterLabel));
    if (view.filterModule) {
      const mod = modules.find((m) => m.name === view.filterModule);
      const mid = mod ? mod.id : null;
      out = out.filter((i) => i.module_id === mid);
    }
    return out;
  });

  // LIF-119: fuzzy full-text search. The scoring/ranking lives in
  // lib/issues/search.ts; we wrap it in one $derived so downstream code can
  // read `filteredIssues` and `issueSearchScores` as projections of the
  // single result (avoids writing $state from inside a $derived). Fed the
  // control-filtered set so search composes with the active filters.
  let searchResult = $derived(computeSearchResult(view.searchQuery, controlFilteredIssues));

  let filteredIssues = $derived(searchResult.issues);
  let issueSearchScores = $derived(searchResult.scores);

  // Sort, display (group/density), collapsed groups, and the filter/sort
  // helpers now live on `view` (lib/issues/state.svelte.ts).

  // Sort applied to filtered issues. We make a fresh array so we don't
  // mutate the underlying `issues` state in place. The comparator is the
  // pure compareIssues from lib/issues/sort.ts, fed the current search
  // query + score map so relevance ordering still wins during search.
  let sortedIssues = $derived(
    [...filteredIssues].sort((a, b) =>
      compareIssuesPure(a, b, {
        searchQuery: view.searchQuery,
        scores: issueSearchScores,
        sortField: view.sortField,
        sortDir: view.sortDir,
      }),
    ),
  );

  // LIF-191: generalized grouping for the list view (logic in
  // lib/issues/grouping.ts). Returns ordered groups for the active
  // `groupBy`, or null when the view should render flat.
  let groups = $derived(
    buildGroups({
      sortedIssues,
      modules,
      groupBy: view.groupBy,
      searchQuery: view.searchQuery,
      filterStatus: view.filterStatus,
    }),
  );

  // ── LIF-161: topbar tallies ──────────────────────────
  // Per-status counts for the cluster next to the breadcrumb. Server truth,
  // independent of the (capped) list fetch and of any active filters.
  let statusCounts = $derived.by(() => {
    const c = issueCounts;
    if (!c) return [];
    return STATUSES.map((s) => ({
      status: s,
      count: c[s as keyof IssueStatusCounts],
    }));
  });

  // The number beside "Issues"/"Board". Shows the true total; when the view
  // is narrowed (filters or search) it becomes "shown of total" so the two
  // numbers can't be mistaken for each other.
  let countLabel = $derived.by(() => {
    if (!issueCounts) return loading ? "" : String(filteredIssues.length);
    const total = issueCounts.total;
    const narrowed = view.hasActiveFilters() || !!view.searchQuery.trim();
    return narrowed && filteredIssues.length !== total
      ? `${filteredIssues.length} of ${total}`
      : String(total);
  });

  // ── Topbar UI state ─────────────────────────────────
  // Filters, sort, group/density, and the popover flags now live on `view`.
  // searchInputEl stays here — it's a DOM-element ref the component owns.
  let searchInputEl = $state<HTMLInputElement | null>(null);
  // hintsOpen / displayOpen / sortOpen / newMenuOpen and hiddenStatuses now
  // live on `view`. view.hydrate() also loads hiddenStatuses per project.

  // ── Board view: drag-and-drop state ──────────────────
  // svelte-dnd-action needs each zone to own a writable items array
  // that it can mutate during consider/finalize. We sync that from the
  // sorted-issues derived value (so filters + sort feed into it), and
  // svelte-dnd-action takes over during the drag lifecycle.
  //
  // LIF-241: with swimlanes, a "zone" is (lane, status) rather than just
  // status, so the map is keyed by a composite string. NO_LANE is the key
  // used when laneBy === "none" (today's flat single-lane board).
  const NO_LANE = "__all__";
  function cellKey(laneKey: string, status: string): string {
    return `${laneKey}::${status}`;
  }

  let boardLanes = $derived(
    buildLanes({ sortedIssues, modules, laneBy: view.laneBy }),
  );

  // Statuses that render as columns — respects the single-status filter and
  // the "Columns" visibility pills. Shared by every lane's column row (and
  // the single implicit lane when swimlanes are off).
  let visibleStatuses = $derived(
    STATUSES.filter(
      (s) => (!view.filterStatus || view.filterStatus === s) && !view.hiddenStatuses.has(s),
    ),
  );

  let boardItems = $state<Record<string, Issue[]>>({});

  $effect(() => {
    if (layout !== "board") return;
    const next: Record<string, Issue[]> = {};
    if (boardLanes) {
      for (const lane of boardLanes) {
        for (const s of STATUSES) {
          next[cellKey(lane.key, s)] = lane.issues.filter((i) => i.status === s);
        }
      }
    } else {
      for (const s of STATUSES) {
        next[cellKey(NO_LANE, s)] = sortedIssues.filter((i) => i.status === s);
      }
    }
    boardItems = next;
  });

  function handleConsider(
    laneKey: string,
    status: string,
    e: CustomEvent<DndEvent<Issue>>,
  ) {
    // A drag is in progress — veto auto-refresh until finalize.
    dragActive = true;
    boardItems[cellKey(laneKey, status)] = e.detail.items as Issue[];
  }

  async function handleFinalize(
    laneKey: string,
    status: string,
    e: CustomEvent<DndEvent<Issue>>,
  ) {
    const newItems = e.detail.items as Issue[];
    boardItems[cellKey(laneKey, status)] = newItems;

    // Find the issue that landed in this cell with a stale status and/or
    // lane value — that's a cross-column and/or cross-lane drop. The
    // dragged issue object is untouched until the PUT below resolves, so
    // comparing its (still-old) status/lane to this cell's is the same
    // "what changed" trick for both dimensions at once. There can only
    // ever be one such item per finalize (a single drag op).
    const moved = newItems.find((i) => {
      if (i.status !== status) return true;
      if (view.laneBy !== "none" && laneKeyForIssue(i, view.laneBy) !== laneKey) return true;
      return false;
    });

    if (!moved) {
      dragActive = false;
      return;
    }

    // Build the update payload from the *destination cell*, not from a
    // diff — always sending both the status and (when lanes are on) the
    // lane field in one PUT, even if one of them didn't actually change.
    // Simpler than conditionally including fields, and idempotent (a
    // same-lane cross-status drop just resends the current module_id/
    // priority, which is a no-op server-side).
    const update: Record<string, unknown> = { status };
    if (view.laneBy === "module") {
      update.module_id = laneKey === "none" ? null : Number(laneKey);
    } else if (view.laneBy === "priority") {
      update.priority = laneKey;
    }

    // LIF-243: capture the pre-drop fields for Undo before anything else
    // touches `moved` — it still carries the stale (pre-drop) values here,
    // mirroring `update`'s shape field-for-field.
    const prevPatch: Record<string, unknown> = { status: moved.status };
    if ("module_id" in update) prevPatch.module_id = moved.module_id;
    if ("priority" in update) prevPatch.priority = moved.priority;

    // Optimistic: stamp both fields onto the master issues list so
    // sortedIssues/boardLanes/the cell all stay coherent until the API
    // resolves.
    const idx = issues.findIndex((i) => i.id === moved.id);
    if (idx >= 0) {
      issues = issues.map((i) =>
        i.id === moved.id ? { ...i, ...(update as Partial<Issue>) } : i,
      );
    }

    const movedId = moved.id;
    const movedIdentifier = moved.identifier;

    // trackMutation keeps auto-refresh paused until the PUT resolves;
    // clear dragActive only after, so no poll lands between drop and ack.
    // updateIssueWithUndo shows the success/Undo toast (or an error toast
    // on failure, replacing the previous silent failure path) and, on
    // Undo, re-stamps `prevPatch` back onto `issues` via onApplied.
    const ok = await trackMutation(
      updateIssueWithUndo({
        id: movedId,
        identifier: movedIdentifier,
        patch: update,
        prevPatch,
        modules,
        onApplied: (patch) => {
          issues = issues.map((i) =>
            i.id === movedId ? { ...i, ...(patch as Partial<Issue>) } : i,
          );
        },
      }),
    );
    dragActive = false;
    if (!ok) {
      // Rollback by re-fetching. Simpler than trying to undo the local
      // mutation surgically — drop failures should be rare. This also
      // covers the two-field case: a partial server failure still leaves
      // the client fully consistent because the reload replaces both
      // fields from server truth, not just the one that failed.
      await loadIssues();
    }
  }

  function openSearch() {
    view.searchExpanded = true;
    requestAnimationFrame(() => searchInputEl?.focus());
  }
  function maybeCollapseSearch() {
    // Collapse back to icon if user blurred an empty input.
    if (!view.searchQuery) view.searchExpanded = false;
  }

  // ── Keyboard navigation ──────────────────────────────
  // focusedIndex, the selection set, and the inline status/priority dropdown
  // ids now live on `view`. The inline-create form vars + DOM refs stay here.
  let inlineCreateActive = $state(false);
  let inlineCreateStatus = $state("backlog");
  let inlineCreateStatusOpen = $state(false);
  let inlineCreateTitle = $state("");
  let inlineCreateSaving = $state(false);
  let inlineCreateTitleEl = $state<HTMLInputElement | null>(null);
  let listEl = $state<HTMLDivElement | null>(null);

  // ── LIF-149: multi-select + bulk actions ─────────────
  // Selection (view.selectedIds) is ephemeral. `x` toggles the focused row,
  // shift+click / shift+j/k extend, ctrl/cmd+click toggles, Esc clears. The
  // selection mutators live here because they read the `flatIssues` derived.
  // Which action-bar menu is open (popovers open upward from the bar).
  let bulkMenu = $state<BulkMenu>(null);
  let bulkBusy = $state(false);

  function toggleSelect(id: number, idx: number) {
    const next = new Set(view.selectedIds);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    view.selectedIds = next;
    view.lastSelectedIdx = idx;
  }

  function rangeSelect(idx: number) {
    if (view.lastSelectedIdx < 0 || view.lastSelectedIdx >= flatIssues.length) {
      toggleSelect(flatIssues[idx].id, idx);
      return;
    }
    const [a, b] =
      view.lastSelectedIdx < idx
        ? [view.lastSelectedIdx, idx]
        : [idx, view.lastSelectedIdx];
    const next = new Set(view.selectedIds);
    for (let i = a; i <= b; i++) next.add(flatIssues[i].id);
    view.selectedIds = next;
    view.lastSelectedIdx = idx;
  }

  function clearSelection() {
    view.clearSelection();
    bulkMenu = null;
  }

  // Prune selection to rows that still exist — filters, search, and the
  // background poll can all remove rows out from under a selection. Only
  // writes when something actually fell out, so the effect settles.
  $effect(() => {
    const visible = new Set(flatIssues.map((i) => i.id));
    if ([...view.selectedIds].some((id) => !visible.has(id))) {
      view.selectedIds = new Set(
        [...view.selectedIds].filter((id) => visible.has(id)),
      );
    }
  });

  /** Apply the same field update to every selected issue. Optimistic:
   *  stamps the change locally on success; converges via reload if any
   *  PUT fails (rare — same tradeoff as the board's drop handler).
   *  LIF-243: each target's prior value is captured before the mutation so
   *  the resulting toast's Undo restores every issue to *its own* prior
   *  status/priority/module rather than a single blanket value. */
  async function bulkUpdate(input: Record<string, unknown>) {
    if (bulkBusy || view.selectedIds.size === 0) return;
    bulkBusy = true;
    bulkMenu = null;
    skipFocusReset = true;
    const targets = issues
      .filter((i) => view.selectedIds.has(i.id))
      .map((i) => ({ id: i.id, identifier: i.identifier, prevPatch: prevPatchFor(i, input) }));
    const { failedIds } = await trackMutation(
      bulkUpdateIssuesWithUndo({
        targets,
        patch: input,
        modules,
        onApplied: (patches) => {
          issues = issues.map((i) =>
            patches.has(i.id) ? { ...i, ...(patches.get(i.id) as Partial<Issue>) } : i,
          );
        },
      }),
    );
    bulkBusy = false;
    if (failedIds.size > 0) {
      await loadIssues();
    }
  }

  /** Add one label to every selected issue (union — issues that already
   *  carry it are skipped, not toggled, so the action is idempotent). */
  async function bulkAddLabel(name: string) {
    if (bulkBusy || view.selectedIds.size === 0) return;
    bulkBusy = true;
    bulkMenu = null;
    skipFocusReset = true;
    const targets = issues.filter(
      (i) => view.selectedIds.has(i.id) && !i.labels.includes(name),
    );
    const results = await Promise.all(
      targets.map((i) =>
        trackMutation(updateIssue(i.id, { labels: [...i.labels, name] })),
      ),
    );
    bulkBusy = false;
    if (results.some((r) => !r.ok)) {
      await loadIssues();
    } else {
      const targetIds = new Set(targets.map((t) => t.id));
      issues = issues.map((i) =>
        targetIds.has(i.id) ? { ...i, labels: [...i.labels, name] } : i,
      );
    }
  }

  async function bulkDelete() {
    if (bulkBusy || view.selectedIds.size === 0) return;
    // LIF-283: deferred delete. Capture the exact rows (with their current
    // array positions) BEFORE removing them, so Undo can splice them back in
    // place rather than reload and jump scroll/selection. The confirm popover
    // in BulkActionBar has already gated this — Undo is the second safety net.
    bulkMenu = null;
    const ids = new Set(view.selectedIds);
    const removed = issues
      .map((issue, index) => ({ issue, index }))
      .filter(({ issue }) => ids.has(issue.id));
    if (removed.length === 0) return;

    // Optimistic local removal + clear selection.
    issues = issues.filter((i) => !ids.has(i.id));
    clearSelection();

    scheduleDelete(
      removed.map((r) => r.issue),
      {
        onRestore: () => {
          // Reinsert each captured row at its original index (ascending, so
          // earlier splices don't shift later indices). Falls back to append
          // if the array has since changed length underneath us.
          const next = [...issues];
          for (const { issue, index } of removed) {
            if (index <= next.length) next.splice(index, 0, issue);
            else next.push(issue);
          }
          issues = next;
        },
        onCommit: () => {
          // Reconcile server-side counts after the real delete lands.
          void loadIssues();
        },
      },
    );
  }

  // Status picker keyboard index now lives on view.inlineCreateStatusIdx
  // (shared by inline create and row dropdowns).

  // Debounce: prevent key-repeat from spamming actions
  let statusUpdating = false;
  let lastKeyAction = 0;
  const KEY_DEBOUNCE = 150; // ms — blocks repeat-fire for held keys

  function canFireKey(): boolean {
    const now = Date.now();
    if (now - lastKeyAction < KEY_DEBOUNCE) return false;
    lastKeyAction = now;
    return true;
  }

  // Mouse suppression after keyboard use
  let keyboardActiveUntil = 0;
  let lastMouseX = 0;
  let lastMouseY = 0;
  const KEYBOARD_COOLDOWN = 750; // ms
  const MOUSE_MOVE_THRESHOLD = 8; // px

  function markKeyboardActive() {
    keyboardActiveUntil = Date.now() + KEYBOARD_COOLDOWN;
  }

  function handleMouseMove(e: MouseEvent) {
    lastMouseX = e.clientX;
    lastMouseY = e.clientY;
  }

  function shouldAcceptMouse(e: MouseEvent): boolean {
    if (Date.now() < keyboardActiveUntil) {
      // Only accept if the mouse has moved meaningfully
      const dx = e.clientX - lastMouseX;
      const dy = e.clientY - lastMouseY;
      if (Math.abs(dx) + Math.abs(dy) < MOUSE_MOVE_THRESHOLD) return false;
    }
    return true;
  }

  // Flat ordered list for keyboard indexing (matches render order).
  // Collapsed groups contribute no rows, so they're excluded — keyboard
  // nav and selection indices stay aligned with what's on screen.
  let flatIssues = $derived.by(() => {
    if (groups) {
      const flat: Issue[] = [];
      for (const g of groups) {
        if (view.isGroupCollapsed(g.key)) continue;
        flat.push(...g.issues);
      }
      return flat;
    }
    return sortedIssues;
  });

  // ── LIF-245: keyboard focus survives a list refetch ──────────────────
  // `flatIssues` is a fresh array every time `issues` changes reference —
  // which happens on every 15s auto-refresh poll, not just on a genuine
  // reorder — so naively resetting `focusedIndex` here would drop keyboard
  // focus out from under the user on every poll tick, even when the exact
  // same issue is still sitting right there. Instead: remember which
  // *issue* (by id) was focused, and when the list changes shape, try to
  // relocate it by id in the new flatIssues before falling back to -1.
  //
  // `skipFocusReset` stays for the handful of call sites (s/p/status-picker
  // mutations) that already relocate focus manually and don't want this
  // effect to race them — those keep working unchanged; this effect's
  // fallback logic below is what upgrades every OTHER case (auto-refresh
  // poll, filter/search/sort/group changes) from "always drop focus" to
  // "keep it if the issue is still visible."
  let skipFocusReset = false;
  let lastFocusedId: number | null = null;
  $effect(() => {
    const flat = flatIssues;
    if (skipFocusReset) {
      skipFocusReset = false;
    } else if (lastFocusedId !== null) {
      view.focusedIndex = flat.findIndex((i) => i.id === lastFocusedId);
    } else {
      view.focusedIndex = -1;
    }
  });

  // Keep `lastFocusedId` in sync with whatever `focusedIndex` currently
  // points at (from ANY source — keyboard nav, mouse hover, the relocation
  // above, or a manual set inside a mutation handler). Plain reads of
  // `view.focusedIndex` and `flatIssues` here are enough to track it
  // reactively without every call site needing to remember to update it.
  $effect(() => {
    const idx = view.focusedIndex;
    const flat = flatIssues;
    lastFocusedId = idx >= 0 && idx < flat.length ? flat[idx].id : null;
  });

  // Scroll focused row into view — only when driven by keyboard
  let scrollOnFocus = false;

  $effect(() => {
    if (view.focusedIndex < 0 || !listEl || !scrollOnFocus) {
      scrollOnFocus = false;
      return;
    }
    scrollOnFocus = false;
    const row = listEl.querySelector(`[data-issue-index="${view.focusedIndex}"]`) as HTMLElement | null;
    if (!row) return;

    requestAnimationFrame(() => {
      const listRect = listEl!.getBoundingClientRect();
      const rowRect = row.getBoundingClientRect();

      const stickyHeader = listEl!.querySelector(".sticky") as HTMLElement | null;
      const headerHeight = stickyHeader ? stickyHeader.offsetHeight : 0;

      const visibleTop = listRect.top + headerHeight;
      const visibleBottom = listRect.bottom;
      const pad = 4;

      if (rowRect.top < visibleTop + pad) {
        listEl!.scrollTop -= (visibleTop + pad - rowRect.top);
      } else if (rowRect.bottom > visibleBottom - pad) {
        listEl!.scrollTop += (rowRect.bottom - visibleBottom + pad);
      }
    });
  });

  function handleKeydown(e: KeyboardEvent) {
    // LIF-245: single shared guard — typing in a field, the peek panel,
    // the command palette, or the shortcut help overlay all own their own
    // keyboard input, so list shortcuts (j/k nav, x select, c create,
    // s/p/m pickers) must not fire on the row behind them. Replaces the
    // old `peekState.open` early-return + a separate `isInputFocused()`
    // check further down — both folded into one predicate in
    // lib/shortcuts.ts so every handler in the app agrees on the rule.
    if (shortcutsSuppressed()) return;

    // Row picker keyboard navigation: status / priority / module dropdown,
    // or the literal inline-create status picker. Only one can ever be
    // open at a time (opening one closes the others — see
    // toggleStatusDropdown/togglePriorityDropdown/toggleModuleDropdown
    // below), so branching on which id is non-null is unambiguous.
    if (
      inlineCreateStatusOpen ||
      view.statusDropdownId !== null ||
      view.priorityDropdownId !== null ||
      view.moduleDropdownId !== null
    ) {
      const moduleOptionIds: (number | null)[] = [null, ...modules.map((m) => m.id)];
      const pickingStatus = inlineCreateStatusOpen || view.statusDropdownId !== null;
      const pickingPriority = !pickingStatus && view.priorityDropdownId !== null;
      const pickingModule = !pickingStatus && !pickingPriority && view.moduleDropdownId !== null;
      const optionCount = pickingStatus
        ? STATUSES.length
        : pickingPriority
          ? PRIORITIES.length
          : moduleOptionIds.length;

      if (e.key === "ArrowDown" || e.key === "j") {
        e.preventDefault();
        if (pickingStatus) view.inlineCreateStatusIdx = Math.min(view.inlineCreateStatusIdx + 1, optionCount - 1);
        else if (pickingPriority) view.priorityPickerIdx = Math.min(view.priorityPickerIdx + 1, optionCount - 1);
        else if (pickingModule) view.modulePickerIdx = Math.min(view.modulePickerIdx + 1, optionCount - 1);
        return;
      }
      if (e.key === "ArrowUp" || e.key === "k") {
        e.preventDefault();
        if (pickingStatus) view.inlineCreateStatusIdx = Math.max(view.inlineCreateStatusIdx - 1, 0);
        else if (pickingPriority) view.priorityPickerIdx = Math.max(view.priorityPickerIdx - 1, 0);
        else if (pickingModule) view.modulePickerIdx = Math.max(view.modulePickerIdx - 1, 0);
        return;
      }
      if (e.key === "Enter") {
        e.preventDefault();
        if (inlineCreateStatusOpen) {
          // Inline create: pick status, move to title
          inlineCreateStatus = STATUSES[view.inlineCreateStatusIdx];
          inlineCreateStatusOpen = false;
          requestAnimationFrame(() => inlineCreateTitleEl?.focus());
        } else if (view.statusDropdownId !== null) {
          const target = issues.find((i) => i.id === view.statusDropdownId);
          if (target) pickRowStatus(target, STATUSES[view.inlineCreateStatusIdx]);
        } else if (view.priorityDropdownId !== null) {
          const target = issues.find((i) => i.id === view.priorityDropdownId);
          if (target) pickRowPriority(target, PRIORITIES[view.priorityPickerIdx]);
        } else if (view.moduleDropdownId !== null) {
          const target = issues.find((i) => i.id === view.moduleDropdownId);
          if (target) pickRowModule(target, moduleOptionIds[view.modulePickerIdx]);
        }
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        if (inlineCreateStatusOpen) {
          inlineCreateStatusOpen = false;
          requestAnimationFrame(() => inlineCreateTitleEl?.focus());
        } else {
          view.statusDropdownId = null;
          view.priorityDropdownId = null;
          view.moduleDropdownId = null;
        }
        return;
      }
      return; // Swallow all other keys while a picker is open
    }

    // ── LIF-245: keyboard row-focus + row-mutation shortcuts are scoped to
    // list mode. Board cards have no visual "focused" state (drag-and-drop
    // + click-to-open/peek are its interaction model — see IssueCard.svelte)
    // so before this gate, j/k/Enter/x/s/p/m/space silently acted on
    // whatever `flatIssues[focusedIndex]` happened to be while looking at
    // the board — invisibly, and occasionally on a stale index left over
    // from a prior list session (e.g. Enter would navigate to a random
    // issue). `c` (new issue) and `/` (search) aren't row-scoped, so they
    // stay available in both layouts, same as before.
    const listOnly = layout === "list";

    switch (e.key) {
      case "ArrowDown":
      case "j":
      case "J": {
        if (!listOnly) break;
        e.preventDefault();
        if (!canFireKey()) break;
        markKeyboardActive();
        scrollOnFocus = true;
        const prevDown = view.focusedIndex;
        view.focusedIndex = Math.min(view.focusedIndex + 1, flatIssues.length - 1);
        // Shift extends the selection across the rows the cursor sweeps.
        if (e.shiftKey && view.focusedIndex >= 0) {
          const next = new Set(view.selectedIds);
          if (prevDown >= 0 && flatIssues[prevDown]) next.add(flatIssues[prevDown].id);
          if (flatIssues[view.focusedIndex]) next.add(flatIssues[view.focusedIndex].id);
          view.selectedIds = next;
          view.lastSelectedIdx = view.focusedIndex;
        }
        break;
      }
      case "ArrowUp":
      case "k":
      case "K": {
        if (!listOnly) break;
        e.preventDefault();
        if (!canFireKey()) break;
        markKeyboardActive();
        scrollOnFocus = true;
        const prevUp = view.focusedIndex;
        view.focusedIndex = Math.max(view.focusedIndex - 1, 0);
        if (e.shiftKey && view.focusedIndex >= 0) {
          const next = new Set(view.selectedIds);
          if (prevUp >= 0 && flatIssues[prevUp]) next.add(flatIssues[prevUp].id);
          if (flatIssues[view.focusedIndex]) next.add(flatIssues[view.focusedIndex].id);
          view.selectedIds = next;
          view.lastSelectedIdx = view.focusedIndex;
        }
        break;
      }
      case "Home":
        if (!listOnly || flatIssues.length === 0) break;
        e.preventDefault();
        markKeyboardActive();
        scrollOnFocus = true;
        view.focusedIndex = 0;
        break;
      case "End":
        if (!listOnly || flatIssues.length === 0) break;
        e.preventDefault();
        markKeyboardActive();
        scrollOnFocus = true;
        view.focusedIndex = flatIssues.length - 1;
        break;
      case "x":
        // Toggle selection on the focused row (LIF-149). LIF-234: selection
        // only drives bulk mutations, so it's a no-op for viewers.
        if (!canEdit) break;
        if (listOnly && view.focusedIndex >= 0 && view.focusedIndex < flatIssues.length) {
          e.preventDefault();
          toggleSelect(flatIssues[view.focusedIndex].id, view.focusedIndex);
        }
        break;
      case "Enter":
        if (listOnly && view.focusedIndex >= 0 && view.focusedIndex < flatIssues.length) {
          e.preventDefault();
          navigate(`/${projectIdentifier}/issues/${flatIssues[view.focusedIndex].identifier}`);
        }
        break;
      case " ":
        // LIF-245: space opens the peek panel on the focused row (mirrors
        // the row's hover peek button). preventDefault so the page
        // doesn't scroll — but only once a row is actually focused, so an
        // idle space press (before any j/k) still scrolls normally.
        if (listOnly && view.focusedIndex >= 0 && view.focusedIndex < flatIssues.length) {
          e.preventDefault();
          peekIssue(flatIssues[view.focusedIndex]);
        }
        break;
      case "c":
        if (!canEdit) break; // LIF-234: creation is maintainer-gated
        e.preventDefault();
        inlineCreateActive = true;
        inlineCreateStatus = "backlog";
        inlineCreateStatusOpen = true;
        view.inlineCreateStatusIdx = 0;
        inlineCreateTitle = "";
        break;
      case "/":
        // Expand the topbar search and focus it.
        e.preventDefault();
        openSearch();
        break;
      case "s":
        // LIF-245: opens the status picker popover (same UI the row's
        // click trigger opens) rather than cycling — see the s/p decision
        // in the report. Cycling lives on shift+S below.
        // LIF-234: read-only for viewers.
        if (!canEdit) break;
        if (listOnly && view.focusedIndex >= 0 && view.focusedIndex < flatIssues.length) {
          e.preventDefault();
          toggleStatusDropdown(flatIssues[view.focusedIndex]);
        }
        break;
      case "S":
        if (!canEdit) break; // LIF-234
        // Fast-path: cycle status without opening the picker. Previously
        // bound to plain `s`; shift+S was an unbound no-op before this
        // change (the old switch only matched lowercase "s"), so this is
        // a pure addition, not a behavior change for existing users.
        if (listOnly && view.focusedIndex >= 0 && view.focusedIndex < flatIssues.length && !statusUpdating && canFireKey()) {
          e.preventDefault();
          const focusedIssue = flatIssues[view.focusedIndex];
          const focusedId = focusedIssue.id;
          const sIdx = STATUSES.indexOf(focusedIssue.status);
          const nextStatus = STATUSES[(sIdx + 1) % STATUSES.length];
          const prevStatus = focusedIssue.status;
          skipFocusReset = true;
          statusUpdating = true;
          trackMutation(
            updateIssueWithUndo({
              id: focusedIssue.id,
              identifier: focusedIssue.identifier,
              patch: { status: nextStatus },
              prevPatch: { status: prevStatus },
              modules,
              onApplied: (patch) => {
                issues = issues.map((i) =>
                  i.id === focusedId ? { ...i, ...(patch as Partial<Issue>) } : i,
                );
                const newIdx = flatIssues.findIndex((i) => i.id === focusedId);
                if (newIdx >= 0) {
                  scrollOnFocus = true;
                  view.focusedIndex = newIdx;
                }
              },
            }),
          ).then(() => {
            statusUpdating = false;
          });
        }
        break;
      case "p":
        // LIF-245: opens the priority picker popover — mirrors `s`.
        // LIF-234: read-only for viewers.
        if (!canEdit) break;
        if (listOnly && view.focusedIndex >= 0 && view.focusedIndex < flatIssues.length) {
          e.preventDefault();
          togglePriorityDropdown(flatIssues[view.focusedIndex]);
        }
        break;
      case "P":
        if (!canEdit) break; // LIF-234
        // Fast-path: cycle priority (previously plain `p`; shift+P was an
        // unbound no-op before this change — see the `S` case comment).
        if (listOnly && view.focusedIndex >= 0 && view.focusedIndex < flatIssues.length && canFireKey()) {
          e.preventDefault();
          const pIssue = flatIssues[view.focusedIndex];
          const pId = pIssue.id;
          const pIdx = PRIORITIES.indexOf(pIssue.priority);
          const nextP = PRIORITIES[(pIdx + 1) % PRIORITIES.length];
          const prevP = pIssue.priority;
          skipFocusReset = true;
          trackMutation(
            updateIssueWithUndo({
              id: pIssue.id,
              identifier: pIssue.identifier,
              patch: { priority: nextP },
              prevPatch: { priority: prevP },
              modules,
              onApplied: (patch) => {
                issues = issues.map((i) =>
                  i.id === pId ? { ...i, ...(patch as Partial<Issue>) } : i,
                );
                const newIdx = flatIssues.findIndex((i) => i.id === pId);
                if (newIdx >= 0) { scrollOnFocus = true; view.focusedIndex = newIdx; }
              },
            }),
          );
        }
        break;
      case "m":
        // LIF-245: opens the module picker popover — mirrors `s`/`p`. No
        // prior binding existed for module, so there's no cycle fast-path
        // to preserve (module sets aren't a small fixed enum like status/
        // priority, so cycling wouldn't be a great fit anyway).
        // LIF-234: read-only for viewers.
        if (!canEdit) break;
        if (listOnly && view.focusedIndex >= 0 && view.focusedIndex < flatIssues.length) {
          e.preventDefault();
          toggleModuleDropdown(flatIssues[view.focusedIndex]);
        }
        break;
      case "Escape":
        if (view.newMenuOpen) {
          view.newMenuOpen = false;
        } else if (view.displayOpen) {
          view.displayOpen = false;
        } else if (view.lanesOpen) {
          view.lanesOpen = false;
        } else if (view.sortOpen) {
          view.sortOpen = false;
        } else if (view.filterOpen) {
          view.filterOpen = false;
        } else if (bulkMenu !== null) {
          bulkMenu = null;
        } else if (view.moduleDropdownId !== null) {
          view.moduleDropdownId = null;
        } else if (view.priorityDropdownId !== null) {
          view.priorityDropdownId = null;
        } else if (view.statusDropdownId !== null) {
          view.statusDropdownId = null;
        } else if (view.selectedIds.size > 0) {
          clearSelection();
        } else if (inlineCreateActive) {
          inlineCreateActive = false;
          inlineCreateStatusOpen = false;
          inlineCreateTitle = "";
        } else {
          view.focusedIndex = -1;
        }
        break;
    }
  }

  // Empty-state CTA: open the inline quick-create row (mirrors the `c`
  // shortcut) and drop focus straight into the title input.
  function startInlineCreateFromEmpty() {
    if (!canEdit) return; // LIF-234: creation is maintainer-gated
    inlineCreateActive = true;
    inlineCreateStatus = "backlog";
    inlineCreateStatusOpen = false;
    inlineCreateTitle = "";
    requestAnimationFrame(() => inlineCreateTitleEl?.focus());
  }

  async function submitInlineCreate() {
    if (!project || !inlineCreateTitle.trim() || inlineCreateSaving) return;
    inlineCreateSaving = true;
    const res = await createIssue({
      project_id: project.id,
      title: inlineCreateTitle.trim(),
      status: inlineCreateStatus,
    });
    inlineCreateSaving = false;
    if (res.ok) {
      inlineCreateActive = false;
      inlineCreateTitle = "";
      navigate(`/${projectIdentifier}/issues/${res.data.identifier}`);
    }
  }

  // ── Row interaction handlers (fed to <IssueRow> as callbacks) ─────────
  // The row component is presentational; these own the shared-state writes
  // and the optimistic issue updates that used to live inline in the row.
  function openIssue(issue: Issue) {
    navigate(`/${projectIdentifier}/issues/${issue.identifier}`);
  }
  // LIF-244: peek panel. openPeek() is the module-level singleton entry
  // point (lib/issues/peek.svelte.ts) — PeekPanel itself is mounted once
  // below and reads that same store.
  function peekIssue(issue: Issue) {
    openPeek(issue.identifier);
  }
  // Mirrors every other row/keyboard/board handler's onApplied: stamp the
  // patch onto the master `issues` array so sortedIssues/groups/boardLanes
  // (and thus the row/card behind the peek's scrim) reflect a mutation
  // made from inside the peek panel, without a refetch.
  function onPeekIssueChanged(id: number, patch: Record<string, unknown>) {
    issues = issues.map((i) => (i.id === id ? { ...i, ...(patch as Partial<Issue>) } : i));
  }
  // LIF-248: PeekPanel is now mounted globally in Layout.svelte, not here —
  // register our sync callback with the peek.svelte.ts singleton instead
  // of passing it as a prop. Unregisters on unmount/navigation so a peek
  // mutation made after this list is gone can't call into a stale closure.
  $effect(() => registerPeekSync(onPeekIssueChanged));
  function onMouseEnterRow(e: MouseEvent, idx: number) {
    if (shouldAcceptMouse(e)) view.focusedIndex = idx;
  }
  function toggleStatusDropdown(issue: Issue) {
    view.priorityDropdownId = null;
    view.moduleDropdownId = null;
    if (view.statusDropdownId === issue.id) {
      view.statusDropdownId = null;
    } else {
      view.statusDropdownId = issue.id;
      view.inlineCreateStatusIdx = Math.max(0, STATUSES.indexOf(issue.status));
    }
  }
  function togglePriorityDropdown(issue: Issue) {
    view.statusDropdownId = null;
    view.moduleDropdownId = null;
    if (view.priorityDropdownId === issue.id) {
      view.priorityDropdownId = null;
    } else {
      view.priorityDropdownId = issue.id;
      view.priorityPickerIdx = Math.max(0, PRIORITIES.indexOf(issue.priority));
    }
  }
  // LIF-245: mirrors toggleStatusDropdown/togglePriorityDropdown. Index 0
  // of the picker is always "No module"; index n+1 is `modules[n]` — see
  // IssueRow's rendering and the moduleOptionIds array in handleKeydown.
  function toggleModuleDropdown(issue: Issue) {
    view.statusDropdownId = null;
    view.priorityDropdownId = null;
    if (view.moduleDropdownId === issue.id) {
      view.moduleDropdownId = null;
    } else {
      view.moduleDropdownId = issue.id;
      const idx = issue.module_id == null ? -1 : modules.findIndex((m) => m.id === issue.module_id);
      view.modulePickerIdx = Math.max(0, idx + 1);
    }
  }
  function pickRowStatus(issue: Issue, status: string) {
    view.statusDropdownId = null;
    if (status === issue.status) return;
    skipFocusReset = true;
    const issueId = issue.id;
    trackMutation(
      updateIssueWithUndo({
        id: issueId,
        identifier: issue.identifier,
        patch: { status },
        prevPatch: { status: issue.status },
        modules,
        onApplied: (patch) => {
          issues = issues.map((i) =>
            i.id === issueId ? { ...i, ...(patch as Partial<Issue>) } : i,
          );
        },
      }),
    );
  }
  function pickRowPriority(issue: Issue, priority: string) {
    view.priorityDropdownId = null;
    if (priority === issue.priority) return;
    skipFocusReset = true;
    const issueId = issue.id;
    trackMutation(
      updateIssueWithUndo({
        id: issueId,
        identifier: issue.identifier,
        patch: { priority },
        prevPatch: { priority: issue.priority },
        modules,
        onApplied: (patch) => {
          issues = issues.map((i) =>
            i.id === issueId ? { ...i, ...(patch as Partial<Issue>) } : i,
          );
        },
      }),
    );
  }
  function pickRowModule(issue: Issue, moduleId: number | null) {
    view.moduleDropdownId = null;
    if (moduleId === issue.module_id) return;
    skipFocusReset = true;
    const issueId = issue.id;
    trackMutation(
      updateIssueWithUndo({
        id: issueId,
        identifier: issue.identifier,
        patch: { module_id: moduleId },
        prevPatch: { module_id: issue.module_id },
        modules,
        onApplied: (patch) => {
          issues = issues.map((i) =>
            i.id === issueId ? { ...i, ...(patch as Partial<Issue>) } : i,
          );
        },
      }),
    );
  }

</script>

<svelte:window
  onkeydown={handleKeydown}
  onmousemove={handleMouseMove}
  onclick={() => {
    view.statusDropdownId = null;
    view.priorityDropdownId = null;
    view.moduleDropdownId = null;
    inlineCreateStatusOpen = false;
    view.displayOpen = false;
    view.sortOpen = false;
    view.newMenuOpen = false;
    view.filterOpen = false;
    view.lanesOpen = false;
    bulkMenu = null;
  }}
/>

<!-- Register topbar with Layout (chrome area above the inset panel).
     Layout: left zone (scope: breadcrumb + view switcher) — filter cluster —
     right zone (display / search / keyboard help / primary action). -->
{#snippet topbarContent()}
  <Topbar
    {view}
    {projectIdentifier}
    {layout}
    {navigate}
    {canEdit}
    {statusCounts}
    countsLoading={issueCounts === null}
    {countLabel}
    {labels}
    {modules}
    {priorityCssColor}
    bind:searchInputEl
    onOpenSearch={openSearch}
    onMaybeCollapseSearch={maybeCollapseSearch}
    onQuickCreate={startInlineCreateFromEmpty}
  />
{/snippet}

<div class="h-full flex">
 <div class="flex-1 min-w-0">
{#if layout === "board"}
  <!-- ── BOARD LAYOUT ──────────────────────────────────────────
       Horizontally-scrolling kanban. Columns are statuses (filterable
       to one via the Status filter, or toggled in/out via the visibility
       pills below). Within a column, cards honor the active sort.
       v1 = click-card-to-open; drag-and-drop tracked in LIF-100. -->
  <div class="h-full flex flex-col">
    <!-- Status visibility pills. Click a pill to hide/show that column.
         Visible columns sit "raised" (chrome bg + soft shadow); hidden
         ones sit recessed against the bg-subtle track, like the
         unselected segment of an iOS-style control. -->
    <div
      class="shrink-0 flex items-center gap-3 px-6 pt-3 pb-2"
    >
      <span
        class="text-micro font-semibold uppercase tracking-widest
               text-[var(--text-faint)]"
      >
        Columns
      </span>
      <div
        class="flex items-center gap-0.5 p-0.5 rounded-md
               bg-[var(--bg-subtle)] border border-[var(--border)]"
      >
        {#each STATUSES as status (status)}
          {@const visible = !view.hiddenStatuses.has(status)}
          {@const count = sortedIssues.filter((i) => i.status === status).length}
          <Tooltip
            content={`${visible ? "Hide" : "Show"} ${status[0].toUpperCase() + status.slice(1)}`}
            placement="bottom"
          >
            <button
              class="flex items-center gap-1.5 px-2 py-1 rounded
                     text-caption font-medium transition-colors
                     {visible
                ? 'bg-[var(--chrome)] text-[var(--text)] shadow-[0_1px_2px_rgba(0,0,0,0.08)]'
                : 'text-[var(--text-faint)] hover:text-[var(--text-muted)]'}"
              aria-pressed={visible}
              onclick={() => view.toggleStatusVisibility(projectIdentifier, status)}
            >
              <StatusIcon status={status} size={12} />
              <span class="capitalize">{status}</span>
              <span
                class="tabular-nums text-micro
                       {visible
                  ? 'text-[var(--text-muted)]'
                  : 'text-[var(--text-faint)]'}"
              >
                {count}
              </span>
            </button>
          </Tooltip>
        {/each}
      </div>
    </div>

    <!-- Board body -->
    {#if loading}
      <!-- LIF-281: board skeleton with shape+position parity to the loaded
           flat board. Column widths now match the loaded snap panels
           (w-[85vw] md:w-[300px]) instead of a fixed w-[300px] that
           overflowed + snapped on phones, and the outer scroller mirrors
           the loaded flat-board container (h-full flex overflow-x-auto).
           Column header (px-3 py-2.5 border-b) and cards (p-2.5, mb-1.5 top
           row, mt-2 bottom row) copy the real column + IssueCard paddings. -->
      <div class="relative flex-1 min-h-0">
        <div class="h-full flex overflow-x-auto overflow-y-hidden">
          {#each [0, 1, 2] as col (col)}
            <div
              class="relative shrink-0 snap-start flex flex-col
                     border-r border-[var(--border)] last:border-r-0
                     w-[85vw] md:w-[300px] h-full"
            >
              <!-- Column header — mirrors the real header at px-3 py-2.5. -->
              <div
                class="shrink-0 flex items-center gap-2 px-3 py-2.5
                       border-b border-[var(--border)]"
              >
                <Skeleton variant="circle" class="size-3.5 shrink-0" />
                <Skeleton variant="bar" class="h-2.5 w-16" />
                <Skeleton variant="bar" class="h-2.5 w-4 shrink-0" />
              </div>
              <!-- Cards container — matches the real p-2 / gap-2 flow. -->
              <div class="flex-1 overflow-y-hidden p-2 min-h-0 flex flex-col gap-2">
                {#each [0, 1, 2] as card (card)}
                  <!-- Card — IssueCard paddings: p-2.5, mb-1.5 top row,
                       mt-2 bottom row. -->
                  <div class="rounded-md border border-[var(--border)] bg-[var(--surface)] p-2.5">
                    <div class="flex items-center gap-2 mb-1.5">
                      <Skeleton variant="bar" class="h-2.5 w-12" />
                      <div class="flex-1"></div>
                      <Skeleton variant="circle" class="size-3.5 shrink-0" />
                    </div>
                    <Skeleton variant="bar" class="h-3 w-full" />
                    <Skeleton variant="bar" class="h-3 w-2/3 mt-1" />
                    <div class="flex items-center mt-2">
                      <div class="flex-1"></div>
                      <Skeleton variant="bar" class="h-2.5 w-10 shrink-0" />
                    </div>
                  </div>
                {/each}
              </div>
            </div>
          {/each}
        </div>
      </div>
    {:else if error}
      <div class="flex-1 flex items-center justify-center">
        <ErrorState title="Couldn't load this board" message={error}>
          <button
            class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)] px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
            onclick={() => loadProject(projectIdentifier)}
          >
            Try again
          </button>
          <button
            class="text-body-sm text-[var(--text-muted)] border border-[var(--border)] px-3 py-1.5 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
            onclick={() => navigate(`/${projectIdentifier}/overview`)}
          >
            Project overview
          </button>
        </ErrorState>
      </div>
    {:else}
      <!-- One column: a status cell within a given lane (or NO_LANE when
           swimlanes are off). Shared by the lane rows below and the
           single-lane fallback so the two render paths can't drift.
           LIF-225/mobile: below md, columns are 85vw scroll-snap panels
           with a subtle edge-fade hint on the scroller; at sm+ they're a
           fixed 300px in a plain horizontally-scrolling row. -->
      {#snippet column(laneKey: string, status: string)}
        {@const colIssues = boardItems[cellKey(laneKey, status)] ?? []}
        {@const collapsed = view.isColumnCollapsed(status)}
        <div
          class="relative shrink-0 snap-start flex flex-col
                 border-r border-[var(--border)] last:border-r-0
                 {collapsed ? 'w-10' : 'w-[85vw] md:w-[300px]'}
                 {boardLanes ? 'h-[380px]' : 'h-full'}"
        >
          {#if collapsed}
            <!-- Collapsed rail: slim drop target. The dndzone IS the
                 visible-sized element (so svelte-dnd-action's dashed-outline
                 drop-target style shows on the real rail the user sees, not
                 an invisible helper); its items render at ~0 size so they
                 don't visually leak, while the chrome overlay on top is
                 pointer-events-none so drags pass through to the zone. -->
            <div
              class="h-full w-full overflow-hidden"
              use:dndzone={{
                items: colIssues,
                flipDurationMs: flipMs(),
                type: "lific-issues",
                dragDisabled: !canEdit,
                dropTargetStyle: {
                  outline: "2px dashed var(--accent)",
                  outlineOffset: "-4px",
                  borderRadius: "8px",
                },
              }}
              onconsider={(e) => handleConsider(laneKey, status, e as CustomEvent<DndEvent<Issue>>)}
              onfinalize={(e) => handleFinalize(laneKey, status, e as CustomEvent<DndEvent<Issue>>)}
            >
              {#each colIssues as issue (issue.id)}
                <div animate:flip={{ duration: flipMs() }} class="h-px opacity-0"></div>
              {/each}
            </div>
            <div class="pointer-events-none absolute inset-0 flex flex-col items-center pt-2.5 pb-3 gap-2">
              <Tooltip content="Expand {status} column" placement="right">
                <button
                  class="pointer-events-auto size-5 flex items-center justify-center rounded
                         text-[var(--text-faint)] hover:text-[var(--text)]
                         hover:bg-[var(--bg-subtle)] transition-colors"
                  onclick={() => view.toggleColumnCollapsed(projectIdentifier, status)}
                >
                  <PanelLeftOpen size={12} />
                </button>
              </Tooltip>
              <StatusIcon {status} size={13} />
              <span
                class="flex-1 text-caption font-semibold uppercase tracking-widest text-[var(--text-muted)]"
                style="writing-mode: vertical-rl; transform: rotate(180deg);"
              >
                {status}
              </span>
              <span class="text-micro text-[var(--text-faint)] tabular-nums">{colIssues.length}</span>
            </div>
          {:else}
            <!-- Column header. Sticky-like: not scrollable with cards. -->
            <div
              class="shrink-0 flex items-center gap-2 px-3 py-2.5
                     border-b border-[var(--border)]"
            >
              <StatusIcon {status} size={14} />
              <span
                class="text-caption font-semibold uppercase tracking-widest
                       text-[var(--text-muted)]"
              >
                {status}
              </span>
              <span class="text-caption text-[var(--text-faint)] tabular-nums">
                {colIssues.length}
              </span>
              <div class="flex-1"></div>
              <Tooltip content="Collapse column" placement="bottom">
                <button
                  class="size-5 flex items-center justify-center rounded
                         text-[var(--text-faint)] hover:text-[var(--text)]
                         hover:bg-[var(--bg-subtle)] transition-colors"
                  onclick={() => view.toggleColumnCollapsed(projectIdentifier, status)}
                >
                  <PanelLeftClose size={12} />
                </button>
              </Tooltip>
              {#if canEdit}
                <Tooltip content="New {status} issue" placement="bottom">
                  <button
                    class="size-5 flex items-center justify-center rounded
                           text-[var(--text-faint)] hover:text-[var(--accent)]
                           hover:bg-[var(--bg-subtle)] transition-colors"
                    onclick={() =>
                      navigate(`/${projectIdentifier}/issues/new?status=${status}`)}
                  >
                    <Plus size={12} />
                  </button>
                </Tooltip>
              {/if}
            </div>

            <!-- Cards container wraps the dndzone so we can render the
                 empty-state placeholder as a sibling (svelte-dnd-action
                 treats every direct child of a dndzone as an item, so
                 non-item children would break drag accounting). -->
            <div class="flex-1 overflow-y-auto p-2 min-h-0 flex flex-col">
              {#if colIssues.length === 0}
                <!-- Visual-only empty placeholder, pinned to the top of the
                     column. pointer-events-none so it never intercepts drop
                     hits on the zone below. Kept as a sibling of the dndzone
                     (svelte-dnd-action treats every direct child of a zone
                     as an item). -->
                <div
                  class="pointer-events-none flex flex-col items-center
                         gap-1.5 pt-4 pb-2"
                >
                  <Mascot src="/LizzySleep2.png" nativeW={1000} nativeH={420} scale={0.1} />
                  <span class="text-caption text-[var(--text-faint)]">
                    All quiet
                  </span>
                </div>
              {/if}
              <!-- Drop zone. All zones share `type: "lific-issues"` so an
                   item dragged from any (lane, status) cell drops into any
                   other — cross-column, cross-lane, or both at once. -->
              <div
                class="flex flex-col gap-2 flex-1 min-h-[40px]"
                use:dndzone={{
                  items: colIssues,
                  flipDurationMs: flipMs(),
                  type: "lific-issues",
                  dragDisabled: !canEdit,
                  dropTargetStyle: {
                    outline: "2px dashed var(--accent)",
                    outlineOffset: "-4px",
                    borderRadius: "8px",
                  },
                }}
                onconsider={(e) => handleConsider(laneKey, status, e as CustomEvent<DndEvent<Issue>>)}
                onfinalize={(e) => handleFinalize(laneKey, status, e as CustomEvent<DndEvent<Issue>>)}
              >
              {#each colIssues as issue (issue.id)}
                <!-- Wrapper carries animate:flip (svelte-dnd-action animates
                     each direct zone child) and is the draggable item; the
                     visual card lives in IssueCard. -->
                <div animate:flip={{ duration: flipMs() }}>
                  <IssueCard
                    {issue}
                    {labels}
                    onOpen={(i) =>
                      navigate(`/${projectIdentifier}/issues/${i.identifier}`)}
                    onPeek={peekIssue}
                  />
                </div>
              {/each}
              </div>
            </div>
          {/if}
        </div>
      {/snippet}

      {#if boardLanes}
        <!-- Swimlanes on: vertically-stacked bands, each its own
             horizontally-scrolling column row (mobile: each lane gets its
             own snap-scroll row, stacked one after another — no nested
             horizontal scrollers competing for the same gesture). -->
        <div class="flex-1 min-h-0 overflow-y-auto overflow-x-hidden">
          {#each boardLanes as lane (lane.key)}
            {@const laneCollapsed = view.isLaneCollapsed(lane.key)}
            <div class="border-b border-[var(--border)] last:border-b-0">
              <!-- Lane header -->
              <button
                class="w-full flex items-center gap-2 px-4 py-2 sticky top-0 z-10
                       bg-[var(--bg)] border-b border-[var(--border)]
                       hover:bg-[var(--bg-subtle)] transition-colors text-left"
                onclick={() => view.toggleLaneCollapsed(projectIdentifier, lane.key)}
                aria-expanded={!laneCollapsed}
              >
                <ChevronRight
                  size={13}
                  class="shrink-0 text-[var(--text-faint)] transition-transform
                         {laneCollapsed ? '' : 'rotate-90'}"
                />
                {#if lane.kind === "module"}
                  <Layers size={13} class="shrink-0 text-[var(--text-faint)]" />
                {:else if lane.kind === "priority"}
                  <PriorityIcon priority={lane.priority ?? "none"} size={13} />
                {/if}
                <span class="text-caption font-semibold uppercase tracking-widest text-[var(--text-muted)] truncate">
                  {lane.label}
                </span>
                <span class="text-caption text-[var(--text-faint)] tabular-nums">
                  {lane.issues.length}
                </span>
              </button>
              {#if !laneCollapsed}
                <div class="relative">
                  <div
                    class="flex overflow-x-auto pb-3
                           max-md:snap-x max-md:snap-mandatory max-md:scroll-smooth
                           max-md:[mask-image:linear-gradient(to_right,transparent,black_16px,black_calc(100%-16px),transparent)]"
                  >
                    {#each visibleStatuses as status (status)}
                      {@render column(lane.key, status)}
                    {/each}
                  </div>
                </div>
              {/if}
            </div>
          {/each}
        </div>
      {:else}
        <!-- Swimlanes off: today's flat board, filling the remaining height. -->
        <div class="relative flex-1 min-h-0">
          <div
            class="h-full flex overflow-x-auto overflow-y-hidden
                   max-md:snap-x max-md:snap-mandatory max-md:scroll-smooth
                   max-md:[mask-image:linear-gradient(to_right,transparent,black_16px,black_calc(100%-16px),transparent)]"
          >
            {#each visibleStatuses as status (status)}
              {@render column(NO_LANE, status)}
            {/each}
          </div>
        </div>
      {/if}
    {/if}
  </div>
{:else}

<div class="h-full flex flex-col">
  <!-- Inline create row (sticky above scrollable list) -->
  {#if inlineCreateActive}
      <div
        class="shrink-0 flex items-center gap-3 px-6 py-2.5
               border-b border-[var(--border)] border-l-2 border-l-[var(--accent)]
               bg-[var(--accent-subtle)]"
      >
        <!-- Status picker -->
        <div class="relative shrink-0">
          <button
            class="size-4 flex items-center justify-center transition-colors
                   hover:text-[var(--accent)]"
            title="Set status"
            onclick={(e) => { e.stopPropagation(); inlineCreateStatusOpen = !inlineCreateStatusOpen; }}
          >
            <StatusIcon status={inlineCreateStatus} size={16} />
          </button>
          {#if inlineCreateStatusOpen}
            <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
            <div
              class="absolute left-0 top-full mt-1.5 z-30 w-[160px]
                     bg-[var(--surface)] border border-[var(--border)]
                     rounded-lg shadow-lg py-1.5"
              onclick={(e) => e.stopPropagation()}
            >
              {#each STATUSES as s, si}
                <button
                  class="w-full flex items-center gap-2 px-3 py-1.5 text-left
                         text-body-sm transition-colors capitalize
                         {si === view.inlineCreateStatusIdx
                    ? 'text-[var(--accent)] bg-[var(--accent-subtle)] font-medium'
                    : 'text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
                  onclick={() => {
                    inlineCreateStatus = s;
                    inlineCreateStatusOpen = false;
                    requestAnimationFrame(() => inlineCreateTitleEl?.focus());
                  }}
                  onmouseenter={() => { view.inlineCreateStatusIdx = si; }}
                >
                  <StatusIcon status={s} size={14} />
                  {s}
                </button>
              {/each}
            </div>
          {/if}
        </div>

        <span class="text-body-sm text-[var(--text-faint)] font-mono shrink-0 w-[72px]">
          {projectIdentifier}-...
        </span>
        <!-- svelte-ignore a11y_autofocus -->
        <input
          type="text"
          bind:this={inlineCreateTitleEl}
          bind:value={inlineCreateTitle}
          class="flex-1 text-body bg-transparent text-[var(--text)]
                 placeholder:text-[var(--text-faint)] outline-none border-none"
          placeholder="Issue title..."
          autofocus={!inlineCreateStatusOpen}
          disabled={inlineCreateSaving}
          onkeydown={(e) => {
            if (e.key === "Enter" && inlineCreateTitle.trim()) {
              e.preventDefault();
              submitInlineCreate();
            }
            if (e.key === "Escape") {
              e.preventDefault();
              e.stopPropagation();
              inlineCreateActive = false;
              inlineCreateStatusOpen = false;
              inlineCreateTitle = "";
            }
          }}
          onblur={() => {
            // Small delay to allow clicking the status picker without closing
            setTimeout(() => {
              if (!inlineCreateTitle.trim() && !inlineCreateStatusOpen) {
                inlineCreateActive = false;
                inlineCreateTitle = "";
              }
            }, 150);
          }}
        />
        {#if inlineCreateSaving}
          <span class="text-caption text-[var(--text-faint)]">Creating...</span>
        {/if}
      </div>
  {/if}

  <!-- Issue list -->
  <div class="flex-1 overflow-y-auto" bind:this={listEl}>
    {#if loading}
      <!-- LIF-281: grouped-list skeleton with shape+position parity to the
           loaded state. The default view (groupBy="status", density=
           "compact") renders status group headers with rows beneath them,
           so the skeleton mirrors BOTH: sticky group-header bars (matching
           the real header's px-6 py-2 / border-b markup) interleaved with
           row skeletons whose container metrics are copied verbatim from
           IssueRow.svelte at compact density (gap-2 sm:gap-3, px-3 sm:px-6,
           py-2.5, border-b border-l-2) — leading size-4 checkbox slot,
           size-4 status circle, w-[52px] sm:w-[72px] identifier, flex-1
           title, and the sm+-only w-[60px] trailing time — so nothing
           snaps when real rows land. -->
      <div>
        {#each [4, 3] as rowCount, gi (gi)}
          <div class="border-b border-[var(--border)] last:border-b-0">
            <!-- Group header — mirrors the real button at IssueList's
                 grouped view: sticky, px-6 py-2, border-b. -->
            <div
              class="w-full sticky top-0 z-10 flex items-center gap-2 px-6 py-2
                     bg-[var(--surface)] border-b border-[var(--border)]"
            >
              <Skeleton variant="bar" class="size-3.5 shrink-0" />
              <Skeleton variant="circle" class="size-3.5 shrink-0" />
              <Skeleton variant="bar" class="h-2.5 w-16" />
              <Skeleton variant="bar" class="h-2.5 w-4 shrink-0" />
            </div>
            {#each Array(rowCount) as _, i (i)}
              <!-- Row — container classes copied from IssueRow.svelte
                   (compact: py-2.5) so height/padding match exactly. The
                   transparent left border keeps the row's horizontal
                   metrics identical to the loaded border-l-2 rows. -->
              <div
                class="w-full flex items-center gap-2 sm:gap-3 px-3 sm:px-6 py-2.5
                       border-b border-[var(--border)] last:border-b-0
                       border-l-2 border-l-transparent"
              >
                <!-- Selection checkbox slot (size-4, always reserved). -->
                <Skeleton variant="bar" class="size-4 shrink-0 rounded" />
                <!-- Status icon. -->
                <Skeleton variant="circle" class="size-4 shrink-0" />
                <!-- Identifier. -->
                <Skeleton variant="bar" class="h-3 w-[52px] sm:w-[72px] shrink-0" />
                <!-- Title (flexes like the loaded title column). -->
                <Skeleton variant="bar" class="h-3.5 flex-1 min-w-0 max-w-[420px]" />
                <!-- Updated time — hidden below sm, w-[60px], like the row. -->
                <Skeleton variant="bar" class="hidden sm:block h-3 w-[60px] shrink-0" />
              </div>
            {/each}
          </div>
        {/each}
      </div>
    {:else if error}
      <ErrorState title="Couldn't load issues" message={error}>
        <button
          class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)] px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
          onclick={() => loadProject(projectIdentifier)}
        >
          Try again
        </button>
        <button
          class="text-body-sm text-[var(--text-muted)] border border-[var(--border)] px-3 py-1.5 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
          onclick={() => navigate(`/${projectIdentifier}/overview`)}
        >
          Project overview
        </button>
      </ErrorState>
    {:else if filteredIssues.length === 0}
      {#if view.hasActiveFilters() || view.searchQuery}
        <!-- Filtered-empty: work exists, it's just hidden behind a
             filter/search, so we keep the recovery affordance. -->
        <div class="flex flex-col items-center justify-center py-20 gap-3">
          <Mascot src="/LizzySleep2.png" nativeW={1000} nativeH={420} scale={0.16} />
          <p class="text-[var(--text-muted)] text-body-lg">
            No issues match your filters
          </p>
          <button
            class="text-body-sm text-[var(--accent)]
                   hover:underline transition-colors"
          onclick={() => view.clearFilters()}
          >
            Clear filters
          </button>
        </div>
      {:else if !inlineCreateActive}
        <!-- Truly-empty: nothing to do. Hidden while the inline create
             row is open so the mascot doesn't fight the input. -->
        <div class="flex flex-col items-center justify-center py-20 gap-4">
          <Mascot src="/LizzySleep2.png" nativeW={1000} nativeH={420} scale={0.25} />
          <div class="flex flex-col items-center gap-1.5 text-center">
            <p class="text-[var(--text)] text-heading font-medium">
              All quiet here
            </p>
            <p class="text-[var(--text-muted)] text-body">
              No work on the board. Time for a nap… or a fresh idea.
            </p>
          </div>
          {#if canEdit}
            <button
              class="flex items-center gap-1.5 mt-1 text-body-sm font-medium
                     text-[var(--btn-success-text)] bg-[var(--btn-success)]
                     px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)]
                     transition-colors"
              onclick={startInlineCreateFromEmpty}
            >
              <Plus size={15} />
              Create an issue
            </button>
          {/if}
        </div>
      {/if}
    {:else if view.searchQuery.trim()}
      <!-- LIF-119: search-mode flat ranked list. Bypasses grouping —
           when hunting for an issue by name or content, the status
           buckets are just noise. Ordering is by relevance score
           (set up in compareIssues). -->
      {#if sortedIssues.length === RESULT_CAP}
        <div class="text-micro text-[var(--text-faint)] uppercase tracking-widest font-semibold px-6 py-2 border-b border-[var(--border)] bg-[var(--surface)]">
          Top {RESULT_CAP} matches — narrow the query for fewer results
        </div>
      {/if}
      {#each sortedIssues as issue, i (issue.id)}
        <div animate:flip={{ duration: flipMs() }}>
          {@render issueRow(issue, i, i === sortedIssues.length - 1)}
        </div>
      {/each}
    {:else if groups}
      <!-- LIF-191: grouped view (group-by status / priority / module).
           Offsets only count NON-collapsed preceding groups so keyboard
           focus indices line up with flatIssues. -->
      {#each groups as g, _gi (g.key)}
        {@const collapsed = view.isGroupCollapsed(g.key)}
        {@const groupOffset = groups.slice(0, _gi).reduce((n, gg) => n + (view.isGroupCollapsed(gg.key) ? 0 : gg.issues.length), 0)}
        <div class="border-b border-[var(--border)] last:border-b-0">
          <button
            class="w-full sticky top-0 z-10 flex items-center gap-2 px-6 py-2
                   bg-[var(--surface)] border-b border-[var(--border)]
                   hover:bg-[var(--bg-subtle)] transition-colors text-left"
            onclick={() => view.toggleGroupCollapsed(projectIdentifier, g.key)}
          >
            <ChevronRight
              size={13}
              class="shrink-0 text-[var(--text-faint)] transition-transform {collapsed ? '' : 'rotate-90'}"
            />
            {#if g.kind === "status"}
              <StatusIcon status={g.key} size={14} />
            {:else if g.kind === "priority"}
              <PriorityIcon priority={g.key} size={14} />
            {:else if g.kind === "module"}
              {#if g.module?.emoji}
                <ProjectIcon value={g.module.emoji} size={14} class="text-[var(--text-muted)]" />
              {:else}
                <Layers size={14} class="text-[var(--text-faint)]" />
              {/if}
            {/if}
            <span class="text-caption font-semibold uppercase tracking-widest text-[var(--text-muted)] truncate">
              {g.label}
            </span>
            <span class="text-caption text-[var(--text-faint)] tabular-nums">{g.issues.length}</span>
          </button>
          {#if !collapsed}
            <div transition:slide={slideParams()}>
              {#each g.issues as issue, si (issue.id)}
                <div animate:flip={{ duration: flipMs() }}>
                  {@render issueRow(issue, groupOffset + si, si === g.issues.length - 1)}
                </div>
              {/each}
            </div>
          {/if}
        </div>
      {/each}
    {:else}
      <!-- Flat list (active when a single status filter is applied, so
           grouping is skipped). Honors the same sort as grouped view. -->
      {#each sortedIssues as issue, i (issue.id)}
        <div animate:flip={{ duration: flipMs() }}>
          {@render issueRow(issue, i, i === sortedIssues.length - 1)}
        </div>
      {/each}
    {/if}
  </div>

  <!-- LIF-149: floating bulk-action bar (component in lib/issues). Appears
       while anything is selected. bulkMenu is bound so the parent's Escape
       handler and outside-click can close the open menu. -->
  {#if view.selectedIds.size > 0}
    <BulkActionBar
      selectedCount={view.selectedIds.size}
      {bulkBusy}
      bind:bulkMenu
      {modules}
      {labels}
      onUpdate={bulkUpdate}
      onAddLabel={bulkAddLabel}
      onDelete={bulkDelete}
      onClear={clearSelection}
    />
  {/if}
</div>
{/if}
 </div>

 <!-- LIF-186: persistent right sidebar — project-wide issue context.
      Always-on (no toggle) on lg+; mirrors the Pages sidebar. Breakdowns
       come from the unfiltered `issues`, and every row is a one-click
      filter shortcut into the existing filter state. -->
 {#if layout !== "board" && !loading && !error}
   <RightSidebar
      stats={sidebarStats}
      {modules}
      filterPriority={view.filterPriority}
      filterModule={view.filterModule}
      onTogglePriority={(p) => view.togglePriorityFilter(p)}
     onToggleModule={(name) => view.toggleModuleFilter(name)}
   />
 {/if}
</div>

{#snippet issueRow(issue: Issue, idx: number, isLast: boolean = false)}
  <IssueRow
    {issue}
    {idx}
    {isLast}
    editable={canEdit}
    {labels}
    {modules}
    density={view.density}
    groupBy={view.groupBy}
    isFocused={idx === view.focusedIndex}
    isSelected={view.selectedIds.has(issue.id)}
    selectionActive={view.selectedIds.size > 0}
    hitSnippet={issueSearchScores.get(issue.id)?.snippet ?? null}
    statusOpen={view.statusDropdownId === issue.id}
    priorityOpen={view.priorityDropdownId === issue.id}
    moduleOpen={view.moduleDropdownId === issue.id}
    statusPickerIdx={view.inlineCreateStatusIdx}
    priorityPickerIdx={view.priorityPickerIdx}
    modulePickerIdx={view.modulePickerIdx}
    onOpen={openIssue}
    onPeek={peekIssue}
    onRangeSelect={rangeSelect}
    onToggleSelect={toggleSelect}
    {onMouseEnterRow}
    onToggleStatusDropdown={toggleStatusDropdown}
    onTogglePriorityDropdown={togglePriorityDropdown}
    onToggleModuleDropdown={toggleModuleDropdown}
    onPickStatus={pickRowStatus}
    onPickPriority={pickRowPriority}
    onPickModule={pickRowModule}
    onHoverStatusOption={(si) => { view.inlineCreateStatusIdx = si; }}
    onHoverPriorityOption={(pi) => { view.priorityPickerIdx = pi; }}
    onHoverModuleOption={(mi) => { view.modulePickerIdx = mi; }}
  />
{/snippet}



