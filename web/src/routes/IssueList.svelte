<script lang="ts">
  import {
    listIssues,
    listProjects,
    listModules,
    listLabels,
    updateIssue,
    createIssue,
    type Issue,
    type Project,
    type Module,
    type Label,
  } from "../lib/api";
  import {
    Plus, Search, ChevronRight, CircleCheckBig, CircleX, X,
    Circle, CircleDot, CircleDashed, Layers, SignalHigh, SignalMedium, SignalLow, Signal, AlertTriangle,
    List as ListIcon, LayoutGrid, SlidersHorizontal, HelpCircle,
    ArrowDownUp, ArrowDown, ArrowUp, Hash, Clock, History,
  } from "lucide-svelte";
  import Select from "../lib/Select.svelte";
  import Tooltip from "../lib/Tooltip.svelte";
  import { dndzone, type DndEvent } from "svelte-dnd-action";
  import { flip } from "svelte/animate";
  import { getContext } from "svelte";
  import { fuzzyMatch, buildSnippet } from "../lib/fuzzy";

  // LIF-119: search tuning, kept identical to the page list (LIF-118) so
  // the two list views feel consistent. See web/src/lib/fuzzy.ts for the
  // scorer and the rationale on each constant.
  const SCORE_THRESHOLD = 0.25;
  const RESULT_CAP = 50;
  const CONTENT_SCAN_MAX = 4000;
  const CONTENT_WEIGHT = 0.6;
  const IDENTIFIER_WEIGHT = 0.9;

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
  let modules = $state<Module[]>([]);
  let labels = $state<Label[]>([]);
  let loading = $state(true);
  let error = $state("");

  // Filters
  let filterStatus = $state<string>("");
  let filterPriority = $state<string>("");
  let filterLabel = $state<string>("");
  let filterModule = $state<string>("");
  let searchQuery = $state("");

  const STATUSES = ["backlog", "todo", "active", "done", "cancelled"];
  const PRIORITIES = ["urgent", "high", "medium", "low", "none"];

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

  // CSS variable value for a status — used in both snippets
  function statusCssColor(s: string): string {
    switch (s) {
      case "backlog": return "var(--text-faint)";
      case "todo": return "var(--text-muted)";
      case "active": return "var(--accent)";
      case "done": return "var(--success)";
      case "cancelled": return "var(--text-faint)";
      default: return "var(--text-faint)";
    }
  }

  function priorityCssColor(p: string): string {
    switch (p) {
      case "urgent": return "var(--error)";
      case "high": return "#f97316";
      case "medium": return "var(--accent)";
      case "low": return "var(--text-muted)";
      case "none": return "var(--text-faint)";
      default: return "var(--text-faint)";
    }
  }

  // ── Persisted list/board view state ──────────────────
  // Filters, search, and sort are remembered per-project so navigating
  // away (e.g. into an issue detail) and back doesn't reset the view.
  // Layout (list vs board) is remembered too so IssueDetail's back arrow
  // knows where to send the user.
  function storageKeyForState(id: string) {
    return `lific:list:state:${id}`;
  }
  function storageKeyForLayout(id: string) {
    return `lific:list:layout:${id}`;
  }

  type PersistedListState = {
    filterStatus?: string;
    filterPriority?: string;
    filterLabel?: string;
    filterModule?: string;
    searchQuery?: string;
    sortField?: SortField;
    sortDir?: SortDir;
  };

  let stateHydrated = $state(false);

  // Re-run when the project prop changes (read it synchronously so Svelte tracks it)
  $effect(() => {
    const id = projectIdentifier;
    stateHydrated = false;
    // Hydrate filters/sort/search from localStorage (per-project) so going
    // back from an issue detail preserves the view. Fall back to empty
    // defaults if nothing is stored or storage is unavailable.
    let s: PersistedListState = {};
    try {
      const raw = localStorage.getItem(storageKeyForState(id));
      if (raw) s = JSON.parse(raw) as PersistedListState;
    } catch {
      // ignore
    }
    filterStatus = s.filterStatus ?? "";
    filterPriority = s.filterPriority ?? "";
    filterLabel = s.filterLabel ?? "";
    filterModule = s.filterModule ?? "";
    searchQuery = s.searchQuery ?? "";
    if (s.sortField) sortField = s.sortField;
    if (s.sortDir) sortDir = s.sortDir;
    stateHydrated = true;
    loadProject(id);
  });

  // Remember which layout the user is on so IssueDetail's back arrow
  // returns to the right route (/board vs /issues).
  $effect(() => {
    const id = projectIdentifier;
    const l = layout;
    try {
      localStorage.setItem(storageKeyForLayout(id), l);
    } catch {
      // ignore
    }
  });

  // Persist filter/sort/search state on change. Gated on stateHydrated
  // to avoid clobbering storage with defaults during the hydrate pass.
  $effect(() => {
    const id = projectIdentifier;
    const snapshot: PersistedListState = {
      filterStatus,
      filterPriority,
      filterLabel,
      filterModule,
      searchQuery,
      sortField,
      sortDir,
    };
    if (!stateHydrated) return;
    try {
      localStorage.setItem(storageKeyForState(id), JSON.stringify(snapshot));
    } catch {
      // ignore
    }
  });

  // Reload issues when filters change
  $effect(() => {
    // Reference the filter values to create dependency
    filterStatus;
    filterPriority;
    filterLabel;
    filterModule;
    if (project) {
      loadIssues();
    }
  });

  async function loadProject(identifier: string) {
    loading = true;
    error = "";
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
      limit: 200,
    };
    if (filterStatus) filters.status = filterStatus;
    if (filterPriority) filters.priority = filterPriority;
    if (filterLabel) filters.label = filterLabel;
    if (filterModule) {
      const mod = modules.find((m) => m.name === filterModule);
      if (mod) filters.module_id = mod.id;
    }

    const res = await listIssues(filters);
    if (res.ok) {
      issues = res.data;
    }
  }

  // LIF-119: fuzzy full-text search across title, identifier, and
  // description. We compute the filtered set AND the per-issue score map
  // in a single derived so we never have to write a $state from inside
  // a $derived (Svelte's state_unsafe_mutation guard). Downstream code
  // reads `filteredIssues` and `issueSearchScores` as derived projections
  // of this one result.
  interface SearchHit {
    score: number;
    snippet: string | null;
  }

  let searchResult = $derived.by<{
    issues: Issue[];
    scores: Map<number, SearchHit>;
  }>(() => {
    const q = searchQuery.trim();
    if (!q) return { issues, scores: new Map() };

    const scores = new Map<number, SearchHit>();
    const hits: Array<{ issue: Issue; score: number }> = [];

    for (const issue of issues) {
      const titleHit = fuzzyMatch(q, issue.title);
      const idHit = fuzzyMatch(q, issue.identifier);
      const body = issue.description.slice(0, CONTENT_SCAN_MAX);
      const descHit = fuzzyMatch(q, body);

      const titleScore = titleHit?.score ?? 0;
      const idScore = (idHit?.score ?? 0) * IDENTIFIER_WEIGHT;
      const descScore = (descHit?.score ?? 0) * CONTENT_WEIGHT;

      const best = Math.max(titleScore, idScore, descScore);
      if (best < SCORE_THRESHOLD) continue;

      const snippet =
        descHit && descScore === best && best > 0
          ? buildSnippet(body, descHit.matchStart, descHit.matchEnd)
          : null;

      scores.set(issue.id, { score: best, snippet });
      hits.push({ issue, score: best });
    }

    hits.sort((a, b) => b.score - a.score);
    const capped = hits.slice(0, RESULT_CAP);

    // Drop scores for issues that fell off the result cap so the
    // comparator doesn't grant relevance ordering to invisible rows.
    const capIds = new Set(capped.map((h) => h.issue.id));
    for (const id of [...scores.keys()]) {
      if (!capIds.has(id)) scores.delete(id);
    }

    return { issues: capped.map((h) => h.issue), scores };
  });

  let filteredIssues = $derived(searchResult.issues);
  let issueSearchScores = $derived(searchResult.scores);

  // ── Sort ────────────────────────────────────────────
  // Topbar-controlled ordering. Applied after filter, before grouping —
  // so the sort is honored both inside each status group AND in the
  // flat list. `sortDir` is interpreted per field:
  //   priority asc  = urgent first (lowest rank number)
  //   priority desc = none first
  //   age      asc  = oldest first
  //   age      desc = newest first
  //   number   asc  = smallest issue # first
  //   number   desc = largest issue # first
  type SortField = "priority" | "age" | "number" | "updated";
  type SortDir = "asc" | "desc";
  let sortField = $state<SortField>("priority");
  let sortDir = $state<SortDir>("asc"); // default: urgent first

  const PRIORITY_RANK: Record<string, number> = {
    urgent: 0,
    high: 1,
    medium: 2,
    low: 3,
    none: 4,
  };

  function compareIssues(a: Issue, b: Issue): number {
    // LIF-119: when search is active, relevance wins over the user's
    // chosen sort field. Otherwise priority/age/number drives the
    // ordering as before.
    if (searchQuery.trim() && issueSearchScores.size > 0) {
      const sa = issueSearchScores.get(a.id)?.score ?? 0;
      const sb = issueSearchScores.get(b.id)?.score ?? 0;
      if (sa !== sb) return sb - sa;
      // Tie-break by identifier so the order is stable across keystrokes.
      return a.identifier.localeCompare(b.identifier);
    }

    let r = 0;
    switch (sortField) {
      case "priority":
        r = (PRIORITY_RANK[a.priority] ?? 99)
          - (PRIORITY_RANK[b.priority] ?? 99);
        // Tie-break: newest first within the same priority so urgent
        // issues from today float above urgents from last month.
        if (r === 0) r = b.created_at.localeCompare(a.created_at);
        break;
      case "age":
        r = a.created_at.localeCompare(b.created_at);
        break;
      case "updated":
        r = a.updated_at.localeCompare(b.updated_at);
        break;
      case "number":
        r = a.sequence - b.sequence;
        break;
    }
    return sortDir === "asc" ? r : -r;
  }

  // Sort applied to filtered issues. We make a fresh array so we don't
  // mutate the underlying `issues` state in place.
  let sortedIssues = $derived(
    [...filteredIssues].sort(compareIssues)
  );

  /** Clicking a field selects it (with default asc) or, if already
   *  selected, toggles direction. Matches the spreadsheet-column pattern
   *  users already expect. */
  function selectSort(field: SortField) {
    if (sortField === field) {
      sortDir = sortDir === "asc" ? "desc" : "asc";
    } else {
      sortField = field;
      // "updated" means "last activity"; newest-first is the natural default.
      sortDir = field === "updated" ? "desc" : "asc";
    }
  }

  // Group issues by status for the list view
  let groupedByStatus = $derived.by(() => {
    if (filterStatus) return null; // Don't group when filtered to single status
    const groups: Record<string, Issue[]> = {};
    for (const status of STATUSES) {
      const matching = sortedIssues.filter((i) => i.status === status);
      if (matching.length > 0) groups[status] = matching;
    }
    return groups;
  });

  function hasActiveFilters(): boolean {
    return !!(filterStatus || filterPriority || filterLabel || filterModule);
  }

  function clearFilters() {
    filterStatus = "";
    filterPriority = "";
    filterLabel = "";
    filterModule = "";
    searchQuery = "";
  }

  // ── Topbar UI state ─────────────────────────────────
  // Search collapses to an icon by default; expands inline on click or `/`.
  let searchExpanded = $state(false);
  let searchInputEl = $state<HTMLInputElement | null>(null);
  // Keyboard cheatsheet popover.
  let hintsOpen = $state(false);
  // Display options popover (Group/Density). Wired in sub-phase 1b.
  let displayOpen = $state(false);
  // Sort popover (field + direction).
  let sortOpen = $state(false);

  // ── Board view: per-status column visibility ─────────
  // Users can hide columns they don't care about in their workflow
  // (e.g. "Cancelled" once a project is mid-flight). Stored per-project
  // in localStorage so the choice persists across reloads.
  let hiddenStatuses = $state<Set<string>>(new Set());

  function storageKeyForHidden(id: string) {
    return `lific:board:hidden-statuses:${id}`;
  }

  function toggleStatusVisibility(status: string) {
    const next = new Set(hiddenStatuses);
    if (next.has(status)) next.delete(status);
    else next.add(status);
    hiddenStatuses = next;
    try {
      localStorage.setItem(
        storageKeyForHidden(projectIdentifier),
        JSON.stringify([...next]),
      );
    } catch {
      // localStorage can fail in private mode / quota — silently degrade
      // to in-memory state, which is fine for the rest of the session.
    }
  }

  // Re-hydrate hidden-statuses when the active project changes. Each
  // project owns its own visibility state.
  $effect(() => {
    const id = projectIdentifier;
    try {
      const raw = localStorage.getItem(storageKeyForHidden(id));
      hiddenStatuses = raw ? new Set(JSON.parse(raw) as string[]) : new Set();
    } catch {
      hiddenStatuses = new Set();
    }
  });

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

    if (!moved) return;

    // Optimistic: stamp the new status onto the master issues list so
    // sortedIssues and the cell stay coherent until the API resolves.
    const idx = issues.findIndex((i) => i.id === moved.id);
    if (idx >= 0) {
      issues = issues.map((i) =>
        i.id === moved.id ? { ...i, status } : i,
      );
    }

    const res = await updateIssue(moved.id, { status });
    if (!res.ok) {
      // Rollback by re-fetching. Simpler than trying to undo the local
      // mutation surgically — drop failures should be rare.
      await loadIssues();
    }
  }

  function openSearch() {
    searchExpanded = true;
    requestAnimationFrame(() => searchInputEl?.focus());
  }
  function maybeCollapseSearch() {
    // Collapse back to icon if user blurred an empty input.
    if (!searchQuery) searchExpanded = false;
  }

  // ── Keyboard navigation ──────────────────────────────
  let focusedIndex = $state(-1);
  let inlineCreateActive = $state(false);
  let inlineCreateStatus = $state("backlog");
  let inlineCreateStatusOpen = $state(false);
  let inlineCreateTitle = $state("");
  let inlineCreateSaving = $state(false);
  let inlineCreateTitleEl = $state<HTMLInputElement | null>(null);
  let listEl = $state<HTMLDivElement | null>(null);

  // Status dropdown on existing issue rows
  let statusDropdownId = $state<number | null>(null);

  // Status picker keyboard index (shared by inline create and row dropdowns)
  let inlineCreateStatusIdx = $state(0);

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

  // Flat ordered list for keyboard indexing (matches render order)
  let flatIssues = $derived.by(() => {
    if (groupedByStatus && !filterStatus) {
      const flat: Issue[] = [];
      for (const status of STATUSES) {
        const group = groupedByStatus[status];
        if (group) flat.push(...group);
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
      focusedIndex = -1;
    }
  });

  // Scroll focused row into view — only when driven by keyboard
  let scrollOnFocus = false;

  $effect(() => {
    if (focusedIndex < 0 || !listEl || !scrollOnFocus) {
      scrollOnFocus = false;
      return;
    }
    scrollOnFocus = false;
    const row = listEl.querySelector(`[data-issue-index="${focusedIndex}"]`) as HTMLElement | null;
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
    if (inlineCreateStatusOpen || statusDropdownId !== null) {
      if (e.key === "ArrowDown" || e.key === "j") {
        e.preventDefault();
        inlineCreateStatusIdx = Math.min(inlineCreateStatusIdx + 1, STATUSES.length - 1);
        return;
      }
      if (e.key === "ArrowUp" || e.key === "k") {
        e.preventDefault();
        inlineCreateStatusIdx = Math.max(inlineCreateStatusIdx - 1, 0);
        return;
      }
      if (e.key === "Enter") {
        e.preventDefault();
        const picked = STATUSES[inlineCreateStatusIdx];
        if (inlineCreateStatusOpen) {
          // Inline create: pick status, move to title
          inlineCreateStatus = picked;
          inlineCreateStatusOpen = false;
          requestAnimationFrame(() => inlineCreateTitleEl?.focus());
        } else if (statusDropdownId !== null) {
          // Existing issue row: set status
          const target = issues.find((i) => i.id === statusDropdownId);
          if (target && picked !== target.status) {
            skipFocusReset = true;
            updateIssue(target.id, { status: picked }).then((res) => {
              if (res.ok) {
                target.status = picked;
                issues = [...issues];
              }
            });
          }
          statusDropdownId = null;
        }
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        if (inlineCreateStatusOpen) {
          inlineCreateStatusOpen = false;
          requestAnimationFrame(() => inlineCreateTitleEl?.focus());
        } else {
          statusDropdownId = null;
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
        e.preventDefault();
        if (!canFireKey()) break;
        markKeyboardActive();
        scrollOnFocus = true;
        focusedIndex = Math.min(focusedIndex + 1, flatIssues.length - 1);
        break;
      case "ArrowUp":
      case "k":
        e.preventDefault();
        if (!canFireKey()) break;
        markKeyboardActive();
        scrollOnFocus = true;
        focusedIndex = Math.max(focusedIndex - 1, 0);
        break;
      case "Enter":
        if (focusedIndex >= 0 && focusedIndex < flatIssues.length) {
          e.preventDefault();
          navigate(`/${projectIdentifier}/issues/${flatIssues[focusedIndex].identifier}`);
        }
        break;
      case "c":
        e.preventDefault();
        inlineCreateActive = true;
        inlineCreateStatus = "backlog";
        inlineCreateStatusOpen = true;
        inlineCreateStatusIdx = 0;
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
        hintsOpen = !hintsOpen;
        break;
      case "s":
        if (focusedIndex >= 0 && focusedIndex < flatIssues.length && !statusUpdating && canFireKey()) {
          e.preventDefault();
          const focusedIssue = flatIssues[focusedIndex];
          const focusedId = focusedIssue.id;
          const sIdx = STATUSES.indexOf(focusedIssue.status);
          const nextStatus = STATUSES[(sIdx + 1) % STATUSES.length];
          skipFocusReset = true;
          statusUpdating = true;
          updateIssue(focusedIssue.id, { status: nextStatus }).then((res) => {
            statusUpdating = false;
            if (res.ok) {
              focusedIssue.status = nextStatus;
              issues = [...issues];
              const newIdx = flatIssues.findIndex((i) => i.id === focusedId);
              if (newIdx >= 0) {
                scrollOnFocus = true;
                focusedIndex = newIdx;
              }
            }
          });
        }
        break;
      case "Escape":
        if (hintsOpen) {
          hintsOpen = false;
        } else if (displayOpen) {
          displayOpen = false;
        } else if (sortOpen) {
          sortOpen = false;
        } else if (statusDropdownId !== null) {
          statusDropdownId = null;
        } else if (inlineCreateActive) {
          inlineCreateActive = false;
          inlineCreateStatusOpen = false;
          inlineCreateTitle = "";
        } else {
          focusedIndex = -1;
        }
        break;
    }
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

  function formatRelativeDate(iso: string): string {
    const d = new Date(iso + "Z");
    const now = new Date();
    const diffMs = now.getTime() - d.getTime();
    const diffMins = Math.floor(diffMs / 60000);
    const diffHrs = Math.floor(diffMs / 3600000);
    const diffDays = Math.floor(diffMs / 86400000);

    if (diffMins < 1) return "just now";
    if (diffMins < 60) return `${diffMins}m ago`;
    if (diffHrs < 24) return `${diffHrs}h ago`;
    if (diffDays < 7) return `${diffDays}d ago`;
    return d.toLocaleDateString("en-US", { month: "short", day: "numeric" });
  }
</script>

<svelte:window
  onkeydown={handleKeydown}
  onmousemove={handleMouseMove}
  onclick={() => {
    statusDropdownId = null;
    inlineCreateStatusOpen = false;
    hintsOpen = false;
    displayOpen = false;
    sortOpen = false;
  }}
/>

<!-- Register topbar with Layout (chrome area above the inset panel).
     Layout: left zone (scope: breadcrumb + view switcher) — filter cluster —
     right zone (display / search / keyboard help / primary action). -->
{#snippet topbarContent()}
  <div class="flex items-center gap-3 px-6 py-2 w-full">

    <!-- ── LEFT ZONE: scope + view switcher ───────────────────── -->
    <div class="flex items-center gap-3 shrink-0">
      <!-- Breadcrumb -->
      <div class="flex items-center gap-1.5">
        <button
          class="text-[0.8125rem] font-mono font-medium text-[var(--text-muted)]
                 hover:text-[var(--text)] transition-colors"
          onclick={() => navigate(`/${projectIdentifier}/settings`)}
        >
          {projectIdentifier}
        </button>
        <ChevronRight size={12} class="text-[var(--text-faint)]" />
        <span class="text-[0.8125rem] font-medium text-[var(--text)]">
          Issues
        </span>
        {#if !loading}
          <span
            class="ml-1 text-[0.6875rem] text-[var(--text-faint)] font-medium
                   tabular-nums"
          >
            {filteredIssues.length}
          </span>
        {/if}
      </div>

      <!-- View switcher pill. Routes to `/{project}/issues` for list mode,
           `/{project}/board` for board mode. Active state derives from the
           `layout` prop, which is set by App's route parser. -->
      <div
        class="flex items-center gap-0.5 p-0.5 rounded-md
               bg-[var(--bg-subtle)] border border-[var(--border)]"
      >
        <button
          class="flex items-center gap-1 px-2 py-0.5 rounded
                 text-[0.75rem] font-medium transition-colors
                 {layout === 'list'
            ? 'bg-[var(--chrome)] text-[var(--text)] shadow-[0_1px_2px_rgba(0,0,0,0.08)]'
            : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
          aria-pressed={layout === "list"}
          onclick={() => navigate(`/${projectIdentifier}/issues`)}
        >
          <ListIcon size={11} class="shrink-0" />
          List
        </button>
        <button
          class="flex items-center gap-1 px-2 py-0.5 rounded
                 text-[0.75rem] font-medium transition-colors
                 {layout === 'board'
            ? 'bg-[var(--chrome)] text-[var(--text)] shadow-[0_1px_2px_rgba(0,0,0,0.08)]'
            : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
          aria-pressed={layout === "board"}
          onclick={() => navigate(`/${projectIdentifier}/board`)}
        >
          <LayoutGrid size={11} class="shrink-0" />
          Board
        </button>
      </div>
    </div>

    <!-- Separator -->
    <div class="w-px h-4 bg-[var(--border)]"></div>

    <!-- ── FILTERS (sub-phase 1b will replace with `+ Filter` + chips) ── -->
    <div class="flex items-center gap-1.5">
      <!-- Status -->
      <Select options={statusOptions} bind:value={filterStatus} placeholder="Status" size="sm" class="w-auto">
        {#snippet renderSelected(opt)}
          <span class="flex items-center gap-1.5 text-[0.8125rem]">
            {#if opt.value}
              {@render statusIcon(String(opt.value), 13)}
              <span class="text-[var(--text)] capitalize">{opt.label}</span>
            {:else}
              <span class="text-[var(--text-muted)]">{opt.label}</span>
            {/if}
          </span>
        {/snippet}
        {#snippet renderOption(opt, isSelected)}
          <span class="flex items-center gap-2 text-[0.8125rem] {isSelected ? 'font-medium' : ''}">
            {#if opt.value}
              {@render statusIcon(String(opt.value), 14)}
              <span class="{isSelected ? 'text-[var(--accent)]' : 'text-[var(--text)]'} capitalize">{opt.label}</span>
            {:else}
              <span class="text-[var(--text-muted)]">{opt.label}</span>
            {/if}
          </span>
        {/snippet}
      </Select>

      <!-- Priority -->
      <Select options={priorityOptions} bind:value={filterPriority} placeholder="Priority" size="sm" class="w-auto">
        {#snippet renderSelected(opt)}
          <span class="flex items-center gap-1.5 text-[0.8125rem]">
            {#if opt.value}
              {@render priorityIcon(String(opt.value), 13)}
              <span class="capitalize" style="color: {priorityCssColor(String(opt.value))}">{opt.label}</span>
            {:else}
              <span class="text-[var(--text-muted)]">{opt.label}</span>
            {/if}
          </span>
        {/snippet}
        {#snippet renderOption(opt, isSelected)}
          <span class="flex items-center gap-2 text-[0.8125rem] {isSelected ? 'font-medium' : ''}">
            {#if opt.value}
              {@render priorityIcon(String(opt.value), 14)}
              <span class="{isSelected ? 'text-[var(--accent)]' : 'text-[var(--text)]'} capitalize">{opt.label}</span>
            {:else}
              <span class="text-[var(--text-muted)]">{opt.label}</span>
            {/if}
          </span>
        {/snippet}
      </Select>

      <!-- Labels -->
      {#if labels.length > 0}
        <Select options={labelOptions} bind:value={filterLabel} placeholder="Label" size="sm" class="w-auto">
          {#snippet renderSelected(opt)}
            <span class="flex items-center gap-1.5 text-[0.8125rem]">
              {#if opt.value && opt.color}
                <span class="size-2.5 rounded-full shrink-0" style="background: {opt.color}"></span>
                <span class="text-[var(--text)]">{opt.label}</span>
              {:else}
                <span class="text-[var(--text-muted)]">{opt.label}</span>
              {/if}
            </span>
          {/snippet}
          {#snippet renderOption(opt, isSelected)}
            <span class="flex items-center gap-2 text-[0.8125rem] {isSelected ? 'font-medium' : ''}">
              {#if opt.value && opt.color}
                <span class="size-2.5 rounded-full shrink-0" style="background: {opt.color}"></span>
                <span class="{isSelected ? 'text-[var(--accent)]' : 'text-[var(--text)]'}">{opt.label}</span>
              {:else}
                <span class="text-[var(--text-muted)]">{opt.label}</span>
              {/if}
            </span>
          {/snippet}
        </Select>
      {/if}

      <!-- Modules -->
      {#if modules.length > 0}
        <Select options={moduleOptions} bind:value={filterModule} placeholder="Module" size="sm" class="w-auto">
          {#snippet renderSelected(opt)}
            <span class="flex items-center gap-1.5 text-[0.8125rem]">
              {#if opt.value}
                <Layers size={13} class="shrink-0 text-[var(--text-muted)]" />
                <span class="text-[var(--text)]">{opt.label}</span>
              {:else}
                <span class="text-[var(--text-muted)]">{opt.label}</span>
              {/if}
            </span>
          {/snippet}
          {#snippet renderOption(opt, isSelected)}
            <span class="flex items-center gap-2 text-[0.8125rem] {isSelected ? 'font-medium' : ''}">
              {#if opt.value}
                <Layers size={14} class="shrink-0 text-[var(--text-muted)]" />
                <span class="{isSelected ? 'text-[var(--accent)]' : 'text-[var(--text)]'}">{opt.label}</span>
              {:else}
                <span class="text-[var(--text-muted)]">{opt.label}</span>
              {/if}
            </span>
          {/snippet}
        </Select>
      {/if}

      {#if hasActiveFilters()}
        <button
          class="flex items-center gap-1 text-[0.75rem] text-[var(--text-muted)]
                 hover:text-[var(--text)] px-1.5 py-1 rounded-md
                 hover:bg-[var(--bg-subtle)] transition-colors"
          onclick={clearFilters}
          title="Clear all filters"
        >
          <X size={12} />
          Clear
        </button>
      {/if}
    </div>

    <!-- ── RIGHT ZONE: display / search / help / primary action ── -->
    <div class="ml-auto flex items-center gap-0.5 shrink-0">

      <!-- Sort button + popover. Shows current field + direction; clicking
           a row selects it (default asc) or, if already active, toggles
           direction. Mirrors the spreadsheet-column sort pattern users
           already know from any data tool. -->
      <div class="relative">
        <Tooltip
          content={sortOpen
            ? null
            : `Sort: ${sortField === "age" ? "Age" : sortField === "updated" ? "Updated" : sortField === "number" ? "Issue #" : "Priority"} ${sortDir === "asc" ? "ascending" : "descending"}`}
          placement="bottom"
        >
          <button
            class="h-7 flex items-center gap-1 px-2 rounded-md
                   text-[0.75rem] font-medium
                   text-[var(--text-muted)] hover:text-[var(--text)]
                   hover:bg-[var(--bg-subtle)] transition-colors
                   {sortOpen ? 'text-[var(--text)] bg-[var(--bg-subtle)]' : ''}"
            onclick={(e) => {
              e.stopPropagation();
              sortOpen = !sortOpen;
              displayOpen = false;
              hintsOpen = false;
            }}
          >
            {#if sortDir === "asc"}
              <ArrowUp size={12} class="shrink-0" />
            {:else}
              <ArrowDown size={12} class="shrink-0" />
            {/if}
            <span>
              {sortField === "age"
                ? "Age"
                : sortField === "updated"
                  ? "Updated"
                  : sortField === "number"
                    ? "Issue #"
                    : "Priority"}
            </span>
          </button>
        </Tooltip>
        {#if sortOpen}
          <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
          <div
            class="absolute right-0 top-full mt-1.5 z-30 w-[220px]
                   bg-[var(--surface)] border border-[var(--border)]
                   rounded-lg shadow-lg py-1.5 text-[0.8125rem]"
            onclick={(e) => e.stopPropagation()}
          >
            <div class="px-3 pt-1 pb-1.5 text-[var(--text-faint)]
                        text-[0.6875rem] uppercase tracking-widest
                        font-semibold">
              Sort by
            </div>
            {#snippet sortRow(field: SortField, label: string, Icon: typeof Hash)}
              {@const active = sortField === field}
              <button
                class="w-full flex items-center justify-between gap-2
                       px-3 py-1.5 text-left transition-colors
                       {active
                  ? 'text-[var(--text)] bg-[var(--bg-subtle)] font-medium'
                  : 'text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
                onclick={() => selectSort(field)}
              >
                <span class="flex items-center gap-2">
                  <Icon size={13} class="shrink-0" />
                  {label}
                </span>
                {#if active}
                  <span class="text-[var(--accent)] flex items-center">
                    {#if sortDir === "asc"}
                      <ArrowUp size={13} />
                    {:else}
                      <ArrowDown size={13} />
                    {/if}
                  </span>
                {/if}
              </button>
            {/snippet}
            {@render sortRow("priority", "Priority", Signal)}
            {@render sortRow("age", "Age", Clock)}
            {@render sortRow("updated", "Updated", History)}
            {@render sortRow("number", "Issue number", Hash)}
            <div class="px-3 pt-2 pb-1 mt-1 text-[0.6875rem]
                        text-[var(--text-faint)] border-t
                        border-[var(--border)] leading-snug">
              Click the active row to flip direction.
            </div>
          </div>
        {/if}
      </div>

      <!-- Display options button — HIDDEN for v1.4 (LIF-104).
           The popover only ever held a "coming soon" placeholder, which read
           as a broken control to users. Grouping/density is now tracked as a
           feature; drop this {#if false} to re-enable the button when it ships.
           Kept (not deleted) so the scaffolding is here for that work. -->
      {#if false}
      <div class="relative">
        <Tooltip content={displayOpen ? null : "Display options"} placement="bottom">
          <button
            class="size-7 flex items-center justify-center rounded-md
                   text-[var(--text-muted)] hover:text-[var(--text)]
                   hover:bg-[var(--bg-subtle)] transition-colors
                   {displayOpen ? 'text-[var(--text)] bg-[var(--bg-subtle)]' : ''}"
            onclick={(e) => { e.stopPropagation(); displayOpen = !displayOpen; sortOpen = false; hintsOpen = false; }}
          >
            <SlidersHorizontal size={14} />
          </button>
        </Tooltip>
        {#if displayOpen}
          <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
          <div
            class="absolute right-0 top-full mt-1.5 z-30 w-[240px]
                   bg-[var(--surface)] border border-[var(--border)]
                   rounded-lg shadow-lg p-3 text-[0.8125rem]"
            onclick={(e) => e.stopPropagation()}
          >
            <div class="text-[var(--text-faint)] text-[0.6875rem]
                        uppercase tracking-widest font-semibold mb-2">
              Display
            </div>
            <div class="text-[var(--text-muted)]">
              Grouping &amp; density land here next pass.
            </div>
          </div>
        {/if}
      </div>
      {/if}

      <!-- Search: collapsed to icon, expands inline on click or `/`. -->
      {#if searchExpanded}
        <div class="relative">
          <div class="absolute left-2 top-1/2 -translate-y-1/2 pointer-events-none text-[var(--text-faint)]">
            <Search size={12} />
          </div>
          <!-- svelte-ignore a11y_autofocus -->
          <input
            type="text"
            placeholder="Search issues..."
            bind:this={searchInputEl}
            bind:value={searchQuery}
            onblur={maybeCollapseSearch}
            onkeydown={(e) => {
              if (e.key === "Escape") {
                e.preventDefault();
                searchQuery = "";
                searchExpanded = false;
                (e.currentTarget as HTMLInputElement).blur();
              }
            }}
            class="w-[200px] pl-7 pr-2 py-1 text-[0.8125rem] rounded-md
                   border border-[var(--border)] bg-[var(--surface)]
                   text-[var(--text)] placeholder:text-[var(--text-faint)]
                   focus:border-[var(--accent)]
                   focus:shadow-[0_0_0_3px_var(--accent-subtle)]
                   outline-none transition-colors"
          />
        </div>
      {:else}
        <Tooltip content="Search  ·  /" placement="bottom">
          <button
            class="size-7 flex items-center justify-center rounded-md
                   text-[var(--text-muted)] hover:text-[var(--text)]
                   hover:bg-[var(--bg-subtle)] transition-colors"
            onclick={(e) => { e.stopPropagation(); openSearch(); }}
          >
            <Search size={14} />
          </button>
        </Tooltip>
      {/if}

      <!-- Keyboard cheatsheet popover. -->
      <div class="relative">
        <Tooltip content={hintsOpen ? null : "Shortcuts  ·  ?"} placement="bottom">
          <button
            class="size-7 flex items-center justify-center rounded-md
                   text-[var(--text-muted)] hover:text-[var(--text)]
                   hover:bg-[var(--bg-subtle)] transition-colors
                   {hintsOpen ? 'text-[var(--text)] bg-[var(--bg-subtle)]' : ''}"
            onclick={(e) => { e.stopPropagation(); hintsOpen = !hintsOpen; displayOpen = false; }}
          >
            <HelpCircle size={14} />
          </button>
        </Tooltip>
        {#if hintsOpen}
          <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
          <div
            class="absolute right-0 top-full mt-1.5 z-30 w-[240px]
                   bg-[var(--surface)] border border-[var(--border)]
                   rounded-lg shadow-lg p-3"
            onclick={(e) => e.stopPropagation()}
          >
            <div class="text-[var(--text-faint)] text-[0.6875rem]
                        uppercase tracking-widest font-semibold mb-2">
              Keyboard
            </div>
            <ul class="space-y-1.5 text-[0.8125rem]">
              {#each [
                ["C", "New issue"],
                ["S", "Cycle status"],
                ["↑ ↓ / J K", "Navigate"],
                ["Enter", "Open"],
                ["/", "Search"],
                ["?", "Show this"],
                ["Esc", "Clear / close"],
              ] as [keys, label]}
                <li class="flex items-center justify-between gap-3">
                  <span class="text-[var(--text-muted)]">{label}</span>
                  <kbd class="px-1.5 py-0.5 rounded
                              border border-[var(--border)]
                              bg-[var(--bg-subtle)]
                              text-[var(--text)]
                              font-mono text-[0.6875rem] leading-none
                              shrink-0">
                    {keys}
                  </kbd>
                </li>
              {/each}
            </ul>
          </div>
        {/if}
      </div>

      <!-- Separator -->
      <div class="w-px h-4 bg-[var(--border)] mx-1.5"></div>

      <!-- Primary action: New issue. -->
      <button
        class="flex items-center gap-1 text-[0.8125rem] font-medium
               text-[var(--accent-text)] bg-[var(--accent)] px-2.5 py-1
               rounded-md hover:bg-[var(--accent-hover)] transition-colors"
        onclick={() => navigate(`/${projectIdentifier}/issues/new`)}
      >
        <Plus size={14} />
        New
      </button>
    </div>
  </div>
{/snippet}

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
        class="text-[0.6875rem] font-semibold uppercase tracking-widest
               text-[var(--text-faint)]"
      >
        Columns
      </span>
      <div
        class="flex items-center gap-0.5 p-0.5 rounded-md
               bg-[var(--bg-subtle)] border border-[var(--border)]"
      >
        {#each STATUSES as status (status)}
          {@const visible = !hiddenStatuses.has(status)}
          {@const count = sortedIssues.filter((i) => i.status === status).length}
          <Tooltip
            content={`${visible ? "Hide" : "Show"} ${status[0].toUpperCase() + status.slice(1)}`}
            placement="bottom"
          >
            <button
              class="flex items-center gap-1.5 px-2 py-1 rounded
                     text-[0.75rem] font-medium transition-colors
                     {visible
                ? 'bg-[var(--chrome)] text-[var(--text)] shadow-[0_1px_2px_rgba(0,0,0,0.08)]'
                : 'text-[var(--text-faint)] hover:text-[var(--text-muted)]'}"
              aria-pressed={visible}
              onclick={() => toggleStatusVisibility(status)}
            >
              {@render statusIcon(status, 12)}
              <span class="capitalize">{status}</span>
              <span
                class="tabular-nums text-[0.6875rem]
                       {visible
                  ? 'text-[var(--text-faint)]'
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
      <div class="flex-1 flex items-center justify-center">
        <p class="text-[var(--error)] text-[0.875rem]">{error}</p>
      </div>
    {:else}
      {#each STATUSES as status (status)}
        {@const colIssues = columnItems[status] ?? []}
        {#if (!filterStatus || filterStatus === status) && !hiddenStatuses.has(status)}
          <div
            class="w-[300px] shrink-0 flex flex-col h-full
                   border-r border-[var(--border)] last:border-r-0"
          >
            <!-- Column header. Sticky-like: not scrollable with cards. -->
            <div
              class="shrink-0 flex items-center gap-2 px-3 py-2.5
                     border-b border-[var(--border)]"
            >
              {@render statusIcon(status, 14)}
              <span
                class="text-[0.75rem] font-semibold uppercase tracking-widest
                       text-[var(--text-muted)]"
              >
                {status}
              </span>
              <span class="text-[0.75rem] text-[var(--text-faint)] tabular-nums">
                {colIssues.length}
              </span>
              <div class="flex-1"></div>
              <Tooltip content="New {status} issue" placement="bottom">
                <button
                  class="size-5 flex items-center justify-center rounded
                         text-[var(--text-faint)] hover:text-[var(--accent)]
                         hover:bg-[var(--bg-subtle)] transition-colors"
                  onclick={() => navigate(`/${projectIdentifier}/issues/new`)}
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
                <!-- svelte-ignore a11y_no_static_element_interactions a11y_no_noninteractive_element_interactions a11y_click_events_have_key_events -->
                <article
                  animate:flip={{ duration: 150 }}
                  class="bg-[var(--surface)] border border-[var(--border)]
                         rounded-md p-2.5 cursor-grab active:cursor-grabbing
                         hover:border-[var(--text-faint)]
                         transition-colors group"
                  tabindex="0"
                  onclick={() =>
                    navigate(`/${projectIdentifier}/issues/${issue.identifier}`)}
                >
                  <!-- Top row: identifier + priority -->
                  <div class="flex items-center gap-2 mb-1.5">
                    <span
                      class="text-[0.6875rem] font-mono
                             text-[var(--text-faint)]"
                    >
                      {issue.identifier}
                    </span>
                    <div class="flex-1"></div>
                    {#if issue.priority !== "none"}
                      <Tooltip
                        content={issue.priority[0].toUpperCase() +
                          issue.priority.slice(1)}
                      >
                        {@render priorityIcon(issue.priority, 14)}
                      </Tooltip>
                    {/if}
                  </div>

                  <!-- Title -->
                  <h3
                    class="text-[0.8125rem] text-[var(--text)] leading-snug
                           line-clamp-3
                           {issue.status === 'done' ||
                           issue.status === 'cancelled'
                      ? 'line-through text-[var(--text-muted)]'
                      : ''}"
                  >
                    {issue.title}
                  </h3>

                  <!-- Bottom: labels + updated time -->
                  {#if issue.labels.length > 0 || issue.updated_at}
                    <div
                      class="flex items-center gap-1.5 mt-2 flex-wrap"
                    >
                      {#each issue.labels.slice(0, 3) as lbl}
                        {@const labelObj = labels.find(
                          (l) => l.name === lbl
                        )}
                        <span
                          class="text-[0.625rem] font-medium px-1.5 py-0.5
                                 rounded-full border border-[var(--border)]"
                          style={labelObj
                            ? `color: ${labelObj.color}; border-color: ${labelObj.color}40;`
                            : ""}
                        >
                          {lbl}
                        </span>
                      {/each}
                      {#if issue.labels.length > 3}
                        <span class="text-[0.625rem] text-[var(--text-faint)]">
                          +{issue.labels.length - 3}
                        </span>
                      {/if}
                      <div class="flex-1"></div>
                      <span
                        class="text-[0.625rem] text-[var(--text-faint)]
                               tabular-nums"
                      >
                        {formatRelativeDate(issue.updated_at)}
                      </span>
                    </div>
                  {/if}
                </article>
              {/each}
              </div>
              {#if colIssues.length === 0}
                <!-- Visual-only empty placeholder. pointer-events-none so
                     it never intercepts drop hits on the zone above. -->
                <div
                  class="pointer-events-none text-center py-6
                         text-[0.75rem] text-[var(--text-faint)]"
                >
                  No issues
                </div>
              {/if}
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
            {@render statusIcon(inlineCreateStatus, 16)}
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
                         text-[0.8125rem] transition-colors capitalize
                         {si === inlineCreateStatusIdx
                    ? 'text-[var(--accent)] bg-[var(--accent-subtle)] font-medium'
                    : 'text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
                  onclick={() => {
                    inlineCreateStatus = s;
                    inlineCreateStatusOpen = false;
                    requestAnimationFrame(() => inlineCreateTitleEl?.focus());
                  }}
                  onmouseenter={() => { inlineCreateStatusIdx = si; }}
                >
                  {@render statusIcon(s, 14)}
                  {s}
                </button>
              {/each}
            </div>
          {/if}
        </div>

        <span class="text-[0.8125rem] text-[var(--text-faint)] font-mono shrink-0 w-[72px]">
          {projectIdentifier}-...
        </span>
        <!-- svelte-ignore a11y_autofocus -->
        <input
          type="text"
          bind:this={inlineCreateTitleEl}
          bind:value={inlineCreateTitle}
          class="flex-1 text-[0.875rem] bg-transparent text-[var(--text)]
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
          <span class="text-[0.75rem] text-[var(--text-faint)]">Creating...</span>
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
      <div class="flex items-center justify-center py-20">
        <p class="text-[var(--error)] text-[0.875rem]">{error}</p>
      </div>
    {:else if filteredIssues.length === 0}
      <div class="flex flex-col items-center justify-center py-20 gap-2">
        <p class="text-[var(--text-muted)] text-[0.9375rem]">
          {hasActiveFilters() || searchQuery ? "No issues match your filters" : "No issues yet"}
        </p>
        {#if hasActiveFilters() || searchQuery}
          <button
            class="text-[0.8125rem] text-[var(--accent)]
                   hover:underline transition-colors"
            onclick={clearFilters}
          >
            Clear filters
          </button>
        {/if}
      </div>
    {:else if searchQuery.trim()}
      <!-- LIF-119: search-mode flat ranked list. Bypasses grouping —
           when hunting for an issue by name or content, the status
           buckets are just noise. Ordering is by relevance score
           (set up in compareIssues). -->
      {#if sortedIssues.length === RESULT_CAP}
        <div class="text-[0.6875rem] text-[var(--text-faint)] uppercase tracking-widest font-semibold px-6 py-2 border-b border-[var(--border)] bg-[var(--surface)]">
          Top {RESULT_CAP} matches — narrow the query for fewer results
        </div>
      {/if}
      {#each sortedIssues as issue, i (issue.id)}
        {@render issueRow(issue, i)}
      {/each}
    {:else if groupedByStatus && !filterStatus}
      <!-- Grouped view -->
      {@const _groups = Object.entries(groupedByStatus)}
      {#each _groups as [status, statusIssues], _gi (status)}
        {@const groupOffset = _groups.slice(0, _gi).reduce((n, [, g]) => n + g.length, 0)}
        <div class="border-b border-[var(--border)] last:border-b-0">
          <div
            class="sticky top-0 z-10 flex items-center gap-2 px-6 py-2
                   bg-[var(--surface)] border-b border-[var(--border)]"
          >
            <span class="inline-flex items-center gap-1.5">
              {@render statusIcon(status, 14)}
              <span
                class="text-[0.75rem] font-semibold uppercase tracking-widest
                       text-[var(--text-muted)]"
              >
                {status}
              </span>
            </span>
            <span class="text-[0.75rem] text-[var(--text-faint)]">
              {statusIssues.length}
            </span>
          </div>
          {#each statusIssues as issue, si (issue.id)}
            {@render issueRow(issue, groupOffset + si)}
          {/each}
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
</div>
{/if}

{#snippet issueRow(issue: Issue, idx: number)}
  {@const isFocused = idx === focusedIndex}
  {@const hitSnippet = issueSearchScores.get(issue.id)?.snippet ?? null}
  <div
    class="w-full flex items-center gap-3 px-6 py-2.5 text-left
           border-b border-[var(--border)] last:border-b-0
           border-l-2 transition-colors group cursor-pointer
           {isFocused
      ? 'border-l-[var(--accent)] bg-[var(--accent-subtle)]'
      : 'border-l-transparent hover:bg-[var(--bg-subtle)]'}"
    data-issue-index={idx}
    role="button"
    tabindex="-1"
    onclick={() => navigate(`/${projectIdentifier}/issues/${issue.identifier}`)}
    onmouseenter={(e) => { if (shouldAcceptMouse(e)) focusedIndex = idx; }}
  >
    <!-- Status indicator (clickable to pick).
         Tooltip suppressed while the status picker is open for this row,
         otherwise it'd hover-fight with the dropdown. -->
    <div class="relative shrink-0">
      <Tooltip
        content={statusDropdownId === issue.id
          ? null
          : issue.status[0].toUpperCase() + issue.status.slice(1)}
      >
        <button
          class="size-4 flex items-center justify-center transition-colors
                 hover:text-[var(--accent)]"
          onclick={(e) => {
            e.stopPropagation();
            if (statusDropdownId === issue.id) {
              statusDropdownId = null;
            } else {
              statusDropdownId = issue.id;
              inlineCreateStatusIdx = Math.max(0, STATUSES.indexOf(issue.status));
            }
          }}
        >
          {@render statusIcon(issue.status, 16)}
        </button>
      </Tooltip>
      {#if statusDropdownId === issue.id}
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
                     text-[0.8125rem] transition-colors capitalize
                     {si === inlineCreateStatusIdx
                ? 'text-[var(--accent)] bg-[var(--accent-subtle)] font-medium'
                : 'text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
              onclick={() => {
                statusDropdownId = null;
                if (s !== issue.status) {
                  skipFocusReset = true;
                  updateIssue(issue.id, { status: s }).then((res) => {
                    if (res.ok) {
                      issue.status = s;
                      issues = [...issues];
                    }
                  });
                }
              }}
              onmouseenter={() => { inlineCreateStatusIdx = si; }}
            >
              {@render statusIcon(s, 14)}
              {s}
            </button>
          {/each}
        </div>
      {/if}
    </div>

    <!-- Identifier -->
    <span
      class="text-[0.8125rem] text-[var(--text-faint)] font-mono shrink-0 w-[72px]"
    >
      {issue.identifier}
    </span>

    <!-- Title (and, in search mode, an optional content snippet below
         it when the description was the reason this issue surfaced).
         The column flexes vertically to stack the two lines while the
         outer row stays items-center, so icons remain vertically
         aligned to the title column as a whole. -->
    <div class="flex-1 min-w-0 flex flex-col gap-0.5">
      <span
        class="text-[0.875rem] text-[var(--text)] truncate
               {issue.status === 'done' || issue.status === 'cancelled'
          ? 'line-through text-[var(--text-muted)]'
          : ''}"
      >
        {issue.title}
      </span>
      {#if hitSnippet}
        <span class="text-[0.75rem] text-[var(--text-muted)] truncate">
          {hitSnippet}
        </span>
      {/if}
    </div>

    <!-- Labels -->
    {#if issue.labels.length > 0}
      <div class="flex items-center gap-1 shrink-0">
        {#each issue.labels.slice(0, 2) as lbl}
          {@const labelObj = labels.find((l) => l.name === lbl)}
          <span
            class="text-[0.6875rem] font-medium px-1.5 py-0.5 rounded-full
                   border border-[var(--border)]"
            style={labelObj ? `color: ${labelObj.color}; border-color: ${labelObj.color}40;` : ""}
          >
            {lbl}
          </span>
        {/each}
        {#if issue.labels.length > 2}
          <span class="text-[0.6875rem] text-[var(--text-faint)]">
            +{issue.labels.length - 2}
          </span>
        {/if}
      </div>
    {/if}

    <!-- Priority icon. Matches the icon used in the filter dropdown so the
         row and the filter chip vocabulary stay consistent. -->
    <span class="shrink-0 w-9 flex items-center justify-end">
      {#if issue.priority !== "none"}
        <Tooltip
          content={issue.priority[0].toUpperCase() + issue.priority.slice(1)}
        >
          {@render priorityIcon(issue.priority, 28)}
        </Tooltip>
      {/if}
    </span>

    <!-- Updated time -->
    <span class="text-[0.75rem] text-[var(--text-faint)] shrink-0 w-[60px] text-right">
      {formatRelativeDate(issue.updated_at)}
    </span>
  </div>
{/snippet}

{#snippet statusIcon(status: string, size: number)}
  {#if status === "done"}
    <CircleCheckBig {size} style="color: {statusCssColor(status)}" />
  {:else if status === "cancelled"}
    <CircleX {size} style="color: {statusCssColor(status)}" />
  {:else if status === "active"}
    <CircleDot {size} style="color: {statusCssColor(status)}" />
  {:else if status === "backlog"}
    <CircleDashed {size} style="color: {statusCssColor(status)}" />
  {:else}
    <Circle {size} style="color: {statusCssColor(status)}" />
  {/if}
{/snippet}

{#snippet priorityIcon(priority: string, size: number)}
  {#if priority === "urgent"}
    <AlertTriangle {size} style="color: {priorityCssColor(priority)}" />
  {:else if priority === "high"}
    <SignalHigh {size} style="color: {priorityCssColor(priority)}" />
  {:else if priority === "medium"}
    <SignalMedium {size} style="color: {priorityCssColor(priority)}" />
  {:else if priority === "low"}
    <SignalLow {size} style="color: {priorityCssColor(priority)}" />
  {:else}
    <Signal {size} style="color: {priorityCssColor(priority)}" />
  {/if}
{/snippet}


