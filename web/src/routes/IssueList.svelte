<script lang="ts">
  import {
    listIssues,
    listProjects,
    listModules,
    listLabels,
    updateIssue,
    createIssue,
    deleteIssue,
    getIssueCounts,
    type IssueStatusCounts,
    type Issue,
    type Project,
    type Module,
    type Label,
  } from "../lib/api";
  import { Plus, ChevronRight, Layers } from "lucide-svelte";
  import Tooltip from "../lib/Tooltip.svelte";
  import PriorityIcon from "../lib/PriorityIcon.svelte";
  import StatusIcon from "../lib/StatusIcon.svelte";
  import ProjectIcon from "../lib/ProjectIcon.svelte";
  import Mascot from "../lib/Mascot.svelte";
  import ErrorState from "../lib/ErrorState.svelte";
  import { dndzone, type DndEvent } from "svelte-dnd-action";
  import { flip } from "svelte/animate";
  import { getContext } from "svelte";
  import { formatRelative } from "../lib/format";
  import { startAutoRefresh } from "../lib/autoRefresh.svelte";
  import { compareIssues as compareIssuesPure } from "../lib/issues/sort";
  import { computeSearchResult, RESULT_CAP } from "../lib/issues/search";
  import { STATUSES, PRIORITIES, buildGroups } from "../lib/issues/grouping";
  import { saveListState, saveLayout } from "../lib/issues/persistence";
  import IssueCard from "../lib/issues/IssueCard.svelte";
  import BulkActionBar, {
    type BulkMenu,
  } from "../lib/issues/BulkActionBar.svelte";
  import RightSidebar from "../lib/issues/RightSidebar.svelte";
  import IssueRow from "../lib/issues/IssueRow.svelte";
  import Topbar from "../lib/issues/Topbar.svelte";
  import { IssueListState } from "../lib/issues/state.svelte";

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
  let issues = $state<Issue[]>([]);
  // LIF-186: an unfiltered copy of the project's issues, used purely for the
  // right sidebar's project-wide breakdowns (priority distribution, per-module
  // counts). `issues` is server-FILTERED, so it can't answer "how many issues
  // does each module have across the whole project" once a filter is active.
  // When no filter is active this just mirrors `issues` (no extra fetch).
  let allIssues = $state<Issue[]>([]);
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

  let statusOptions = $derived([
    { value: "", label: "Status" },
    ...STATUSES.map((s) => ({ value: s, label: s })),
  ]);
  let priorityOptions = $derived([
    { value: "", label: "Priority" },
    ...PRIORITIES.map((p) => ({ value: p, label: p })),
  ]);
  let labelOptions = $derived([
    { value: "", label: "Label" },
    ...labels.map((l) => ({ value: l.name, label: l.name, color: l.color })),
  ]);
  let moduleOptions = $derived([
    { value: "", label: "Module" },
    ...modules.map((m) => ({ value: m.name, label: m.name })),
  ]);

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
  // unfiltered `allIssues` so the distribution and per-module counts reflect
  // the whole project, not the currently filtered view.
  let sidebarStats = $derived.by(() => {
    const prio: Record<string, number> = { urgent: 0, high: 0, medium: 0, low: 0, none: 0 };
    const byModule = new Map<number, number>();
    let noModule = 0;
    for (const i of allIssues) {
      prio[i.priority] = (prio[i.priority] ?? 0) + 1;
      if (i.module_id == null) noModule++;
      else byModule.set(i.module_id, (byModule.get(i.module_id) ?? 0) + 1);
    }
    return {
      prio,
      byModule,
      noModule,
      total: issueCounts?.total ?? allIssues.length,
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

  // Reload issues when filters change
  $effect(() => {
    // Reference the filter values to create dependency
    view.filterStatus;
    view.filterPriority;
    view.filterLabel;
    view.filterModule;
    if (project) {
      loadIssues();
    }
  });

  async function loadProject(identifier: string) {
    loading = true;
    error = "";
    // Don't let the previous project's tallies linger while we fetch.
    issueCounts = null;
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

    const filters: Record<string, unknown> = {
      project_id: project.id,
      // LIF-161: was 200, which silently truncated both the list and the
      // topbar count once a project outgrew it. Still bounded so a huge
      // project can't pull megabytes of descriptions per poll; the topbar
      // tallies come from the counts endpoint, not from this fetch.
      limit: 1000,
    };
    if (view.filterStatus) filters.status = view.filterStatus;
    if (view.filterPriority) filters.priority = view.filterPriority;
    if (view.filterLabel) filters.label = view.filterLabel;
    if (view.filterModule) {
      const mod = modules.find((m) => m.name === view.filterModule);
      if (mod) filters.module_id = mod.id;
    }

    // Counts ride along with every issue fetch (initial load, filter
    // change, 15s poll) so the topbar tallies converge with the rows.
    // LIF-186: when a filter is active we ALSO pull an unfiltered set for the
    // sidebar's project-wide breakdowns; with no filter the filtered fetch is
    // already the full set, so we skip the extra round-trip.
    const anyFilter = !!(view.filterStatus || view.filterPriority || view.filterLabel || view.filterModule);
    const reqs: Promise<unknown>[] = [listIssues(filters), getIssueCounts(project.id)];
    if (anyFilter) reqs.push(listIssues({ project_id: project.id, limit: 1000 }));
    const [res, countsRes, allRes] = (await Promise.all(reqs)) as [
      Awaited<ReturnType<typeof listIssues>>,
      Awaited<ReturnType<typeof getIssueCounts>>,
      Awaited<ReturnType<typeof listIssues>> | undefined,
    ];
    if (res.ok) {
      issues = res.data;
    }
    if (countsRes.ok) {
      issueCounts = countsRes.data;
    }
    if (anyFilter) {
      if (allRes && allRes.ok) allIssues = allRes.data;
    } else if (res.ok) {
      allIssues = res.data;
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
      view.hintsOpen ||
      view.displayOpen ||
      view.newMenuOpen ||
      inlineCreateActive ||
      view.statusDropdownId !== null ||
      view.priorityDropdownId !== null ||
      // LIF-149: a poll mustn't shuffle rows mid-selection or land stale
      // data on top of an in-flight bulk write.
      view.selectedIds.size > 0 ||
      bulkBusy ||
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

  // LIF-119: fuzzy full-text search. The scoring/ranking lives in
  // lib/issues/search.ts; we wrap it in one $derived so downstream code can
  // read `filteredIssues` and `issueSearchScores` as projections of the
  // single result (avoids writing $state from inside a $derived).
  let searchResult = $derived(computeSearchResult(view.searchQuery, issues));

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
  let columnItems = $state<Record<string, Issue[]>>({});

  $effect(() => {
    if (layout !== "board") return;
    const next: Record<string, Issue[]> = {};
    for (const s of STATUSES) {
      next[s] = sortedIssues.filter((i) => i.status === s);
    }
    columnItems = next;
  });

  function handleConsider(status: string, e: CustomEvent<DndEvent<Issue>>) {
    // A drag is in progress — veto auto-refresh until finalize.
    dragActive = true;
    columnItems[status] = e.detail.items as Issue[];
  }

  async function handleFinalize(
    status: string,
    e: CustomEvent<DndEvent<Issue>>,
  ) {
    const newItems = e.detail.items as Issue[];

    // Find any issue that landed in this column with a different status
    // — that's a cross-column drop and needs persisting. There can only
    // ever be one such item per finalize (a single drag op).
    const moved = newItems.find((i) => i.status !== status);
    columnItems[status] = newItems;

    if (!moved) {
      dragActive = false;
      return;
    }

    // Optimistic: stamp the new status onto the master issues list so
    // sortedIssues and the cell stay coherent until the API resolves.
    const idx = issues.findIndex((i) => i.id === moved.id);
    if (idx >= 0) {
      issues = issues.map((i) =>
        i.id === moved.id ? { ...i, status } : i,
      );
    }

    // trackMutation keeps auto-refresh paused until the PUT resolves;
    // clear dragActive only after, so no poll lands between drop and ack.
    const res = await trackMutation(updateIssue(moved.id, { status }));
    dragActive = false;
    if (!res.ok) {
      // Rollback by re-fetching. Simpler than trying to undo the local
      // mutation surgically — drop failures should be rare.
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
   *  PUT fails (rare — same tradeoff as the board's drop handler). */
  async function bulkUpdate(input: Record<string, unknown>) {
    if (bulkBusy || view.selectedIds.size === 0) return;
    bulkBusy = true;
    bulkMenu = null;
    skipFocusReset = true;
    const ids = [...view.selectedIds];
    const results = await Promise.all(
      ids.map((id) => trackMutation(updateIssue(id, input))),
    );
    bulkBusy = false;
    if (results.some((r) => !r.ok)) {
      await loadIssues();
    } else {
      issues = issues.map((i) =>
        view.selectedIds.has(i.id) ? { ...i, ...(input as Partial<Issue>) } : i,
      );
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
    bulkBusy = true;
    bulkMenu = null;
    const ids = [...view.selectedIds];
    await Promise.all(ids.map((id) => trackMutation(deleteIssue(id))));
    bulkBusy = false;
    clearSelection();
    await loadIssues();
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

  // Reset focus when issues change — but not from a status cycle
  let skipFocusReset = false;
  $effect(() => {
    flatIssues;
    if (skipFocusReset) {
      skipFocusReset = false;
    } else {
      view.focusedIndex = -1;
    }
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

  function isInputFocused(): boolean {
    const el = document.activeElement;
    if (!el) return false;
    const tag = el.tagName;
    return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || (el as HTMLElement).isContentEditable;
  }

  function handleKeydown(e: KeyboardEvent) {
    // Status picker keyboard navigation (inline create or row dropdown)
    if (inlineCreateStatusOpen || view.statusDropdownId !== null) {
      if (e.key === "ArrowDown" || e.key === "j") {
        e.preventDefault();
        view.inlineCreateStatusIdx = Math.min(view.inlineCreateStatusIdx + 1, STATUSES.length - 1);
        return;
      }
      if (e.key === "ArrowUp" || e.key === "k") {
        e.preventDefault();
        view.inlineCreateStatusIdx = Math.max(view.inlineCreateStatusIdx - 1, 0);
        return;
      }
      if (e.key === "Enter") {
        e.preventDefault();
        const picked = STATUSES[view.inlineCreateStatusIdx];
        if (inlineCreateStatusOpen) {
          // Inline create: pick status, move to title
          inlineCreateStatus = picked;
          inlineCreateStatusOpen = false;
          requestAnimationFrame(() => inlineCreateTitleEl?.focus());
        } else if (view.statusDropdownId !== null) {
          // Existing issue row: set status
          const target = issues.find((i) => i.id === view.statusDropdownId);
          if (target && picked !== target.status) {
            skipFocusReset = true;
            trackMutation(updateIssue(target.id, { status: picked })).then((res) => {
              if (res.ok) {
                target.status = picked;
                issues = [...issues];
              }
            });
          }
          view.statusDropdownId = null;
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
        }
        return;
      }
      return; // Swallow all other keys while picker is open
    }

    // Don't intercept when typing in inputs
    if (isInputFocused()) return;

    switch (e.key) {
      case "ArrowDown":
      case "j":
      case "J": {
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
      case "x":
        // Toggle selection on the focused row (LIF-149).
        if (view.focusedIndex >= 0 && view.focusedIndex < flatIssues.length) {
          e.preventDefault();
          toggleSelect(flatIssues[view.focusedIndex].id, view.focusedIndex);
        }
        break;
      case "Enter":
        if (view.focusedIndex >= 0 && view.focusedIndex < flatIssues.length) {
          e.preventDefault();
          navigate(`/${projectIdentifier}/issues/${flatIssues[view.focusedIndex].identifier}`);
        }
        break;
      case "c":
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
      case "?":
        // Toggle the keyboard cheatsheet popover.
        e.preventDefault();
        view.hintsOpen = !view.hintsOpen;
        break;
      case "s":
        if (view.focusedIndex >= 0 && view.focusedIndex < flatIssues.length && !statusUpdating && canFireKey()) {
          e.preventDefault();
          const focusedIssue = flatIssues[view.focusedIndex];
          const focusedId = focusedIssue.id;
          const sIdx = STATUSES.indexOf(focusedIssue.status);
          const nextStatus = STATUSES[(sIdx + 1) % STATUSES.length];
          skipFocusReset = true;
          statusUpdating = true;
          trackMutation(updateIssue(focusedIssue.id, { status: nextStatus })).then((res) => {
            statusUpdating = false;
            if (res.ok) {
              focusedIssue.status = nextStatus;
              issues = [...issues];
              const newIdx = flatIssues.findIndex((i) => i.id === focusedId);
              if (newIdx >= 0) {
                scrollOnFocus = true;
                view.focusedIndex = newIdx;
              }
            }
          });
        }
        break;
      case "p":
        // LIF-191: cycle priority on the focused row (mirrors `s` for status).
        if (view.focusedIndex >= 0 && view.focusedIndex < flatIssues.length && canFireKey()) {
          e.preventDefault();
          const pIssue = flatIssues[view.focusedIndex];
          const pId = pIssue.id;
          const pIdx = PRIORITIES.indexOf(pIssue.priority);
          const nextP = PRIORITIES[(pIdx + 1) % PRIORITIES.length];
          skipFocusReset = true;
          trackMutation(updateIssue(pIssue.id, { priority: nextP })).then((res) => {
            if (res.ok) {
              pIssue.priority = nextP;
              issues = [...issues];
              const newIdx = flatIssues.findIndex((i) => i.id === pId);
              if (newIdx >= 0) { scrollOnFocus = true; view.focusedIndex = newIdx; }
            }
          });
        }
        break;
      case "Escape":
        if (view.newMenuOpen) {
          view.newMenuOpen = false;
        } else if (view.hintsOpen) {
          view.hintsOpen = false;
        } else if (view.displayOpen) {
          view.displayOpen = false;
        } else if (view.sortOpen) {
          view.sortOpen = false;
        } else if (bulkMenu !== null) {
          bulkMenu = null;
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
  function onMouseEnterRow(e: MouseEvent, idx: number) {
    if (shouldAcceptMouse(e)) view.focusedIndex = idx;
  }
  function toggleStatusDropdown(issue: Issue) {
    if (view.statusDropdownId === issue.id) {
      view.statusDropdownId = null;
    } else {
      view.statusDropdownId = issue.id;
      view.inlineCreateStatusIdx = Math.max(0, STATUSES.indexOf(issue.status));
    }
  }
  function togglePriorityDropdown(issue: Issue) {
    view.statusDropdownId = null;
    view.priorityDropdownId = view.priorityDropdownId === issue.id ? null : issue.id;
  }
  function pickRowStatus(issue: Issue, status: string) {
    view.statusDropdownId = null;
    if (status === issue.status) return;
    skipFocusReset = true;
    updateIssue(issue.id, { status }).then((res) => {
      if (res.ok) {
        issue.status = status;
        issues = [...issues];
      }
    });
  }
  function pickRowPriority(issue: Issue, priority: string) {
    view.priorityDropdownId = null;
    if (priority === issue.priority) return;
    skipFocusReset = true;
    updateIssue(issue.id, { priority }).then((res) => {
      if (res.ok) {
        issue.priority = priority;
        issues = [...issues];
      }
    });
  }

</script>

<svelte:window
  onkeydown={handleKeydown}
  onmousemove={handleMouseMove}
  onclick={() => {
    view.statusDropdownId = null;
    view.priorityDropdownId = null;
    inlineCreateStatusOpen = false;
    view.hintsOpen = false;
    view.displayOpen = false;
    view.sortOpen = false;
    view.newMenuOpen = false;
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
    {statusCounts}
    {countLabel}
    {statusOptions}
    {priorityOptions}
    {labelOptions}
    {moduleOptions}
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

    <!-- Columns -->
    <div class="flex-1 flex overflow-x-auto overflow-y-hidden min-h-0">
    {#if loading}
      <div class="flex-1 flex items-center justify-center">
        <div
          class="size-6 rounded-full border-2 border-[var(--border)]
                 border-t-[var(--accent)] animate-spin"
        ></div>
      </div>
    {:else if error}
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
    {:else}
      {#each STATUSES as status (status)}
        {@const colIssues = columnItems[status] ?? []}
        {#if (!view.filterStatus || view.filterStatus === status) && !view.hiddenStatuses.has(status)}
          <div
            class="w-[300px] shrink-0 flex flex-col h-full
                   border-r border-[var(--border)] last:border-r-0"
          >
            <!-- Column header. Sticky-like: not scrollable with cards. -->
            <div
              class="shrink-0 flex items-center gap-2 px-3 py-2.5
                     border-b border-[var(--border)]"
            >
              <StatusIcon status={status} size={14} />
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
                   item dragged from any column drops into any other. -->
              <div
                class="flex flex-col gap-2 flex-1 min-h-[40px]"
                use:dndzone={{
                  items: colIssues,
                  flipDurationMs: 150,
                  type: "lific-issues",
                  dropTargetStyle: {
                    outline: "2px dashed var(--accent)",
                    outlineOffset: "-4px",
                    borderRadius: "8px",
                  },
                }}
                onconsider={(e) => handleConsider(status, e as CustomEvent<DndEvent<Issue>>)}
                onfinalize={(e) => handleFinalize(status, e as CustomEvent<DndEvent<Issue>>)}
              >
              {#each colIssues as issue (issue.id)}
                <!-- Wrapper carries animate:flip (svelte-dnd-action animates
                     each direct zone child) and is the draggable item; the
                     visual card lives in IssueCard. -->
                <div animate:flip={{ duration: 150 }}>
                  <IssueCard
                    {issue}
                    {labels}
                    onOpen={(i) =>
                      navigate(`/${projectIdentifier}/issues/${i.identifier}`)}
                  />
                </div>
              {/each}
              </div>
            </div>
          </div>
        {/if}
      {/each}
    {/if}
    </div>
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
      <div class="flex items-center justify-center py-20">
        <div
          class="size-6 rounded-full border-2 border-[var(--border)]
                 border-t-[var(--accent)] animate-spin"
        ></div>
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
          <p class="text-[var(--text-muted)] text-[0.9375rem]">
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
            <p class="text-[var(--text)] text-[1.0625rem] font-medium">
              All quiet here
            </p>
            <p class="text-[var(--text-muted)] text-body">
              No work on the board. Time for a nap… or a fresh idea.
            </p>
          </div>
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
        {@render issueRow(issue, i)}
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
            {#each g.issues as issue, si (issue.id)}
              {@render issueRow(issue, groupOffset + si)}
            {/each}
          {/if}
        </div>
      {/each}
    {:else}
      <!-- Flat list (active when a single status filter is applied, so
           grouping is skipped). Honors the same sort as grouped view. -->
      {#each sortedIssues as issue, i (issue.id)}
        {@render issueRow(issue, i)}
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
      come from the unfiltered `allIssues`, and every row is a one-click
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

{#snippet issueRow(issue: Issue, idx: number)}
  <IssueRow
    {issue}
    {idx}
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
    statusPickerIdx={view.inlineCreateStatusIdx}
    onOpen={openIssue}
    onRangeSelect={rangeSelect}
    onToggleSelect={toggleSelect}
    {onMouseEnterRow}
    onToggleStatusDropdown={toggleStatusDropdown}
    onTogglePriorityDropdown={togglePriorityDropdown}
    onPickStatus={pickRowStatus}
    onPickPriority={pickRowPriority}
    onHoverStatusOption={(si) => { view.inlineCreateStatusIdx = si; }}
  />
{/snippet}




