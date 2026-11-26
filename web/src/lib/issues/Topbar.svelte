<script lang="ts">
  // Issue list/board topbar. Extracted from IssueList.svelte (LIF-99 Phase
  // 3c) — the largest single template chunk (~600 lines). Layout: left zone
  // (breadcrumb + view switcher + status tallies), filter cluster, right
  // zone (display / search / keyboard help / primary action).
  //
  // Takes the shared IssueListState (`view`) so filters / sort / display /
  // popover state are read and mutated directly on it. Data-derived inputs
  // (option lists, tallies, count label) and the few component-owned bits
  // (navigate, searchInputEl, inline-create trigger) come in as props.
  import type { IssueAssignee, Label, Module } from "../api";
  import {
    Plus, Search, ChevronDown, Signal,
    List as ListIcon, LayoutGrid, SlidersHorizontal, HelpCircle,
    ArrowDown, ArrowUp, Hash, Clock, History, Check, Zap, PenLine,
    SlidersVertical, Rows3, Layers, MoreHorizontal,
    Square, SquareMinus, SquareCheck, X,
  } from "lucide-svelte";
  import Tooltip from "../Tooltip.svelte";
  import SignInButton from "../SignInButton.svelte";
  import { isSignedOut } from "../signedOut";
  import SubTabs, { type SubTab } from "../SubTabs.svelte";
  import Breadcrumbs from "../Breadcrumbs.svelte";
  import StatusIcon from "../StatusIcon.svelte";
  import FilterModal from "./FilterModal.svelte";
  import FilterChips from "./FilterChips.svelte";
  import SavedViews from "./SavedViews.svelte";
  import type { SortField } from "./sort";
  import type { GroupBy, Density, LaneBy } from "./grouping";
  import type { IssueListState } from "./state.svelte";
  import { toggleShortcutHelp } from "../shortcutHelpState.svelte";

  let {
    view,
    projectIdentifier,
    layout,
    navigate,
    statusCounts,
    countsLoading = false,
    countLabel,
    changedCount = 0,
    onCycleChanged = () => {},
    labels,
    modules,
    assignees = [],
    priorityCssColor,
    searchInputEl = $bindable(),
    onOpenSearch,
    onMaybeCollapseSearch,
    onQuickCreate,
    canEdit = true,
    selectableCount = 0,
    selectedCount = 0,
    selectionBusy = false,
    onSelectAll = () => {},
    onClearSelection = () => {},
    boardLeading,
  }: {
    view: IssueListState;
    projectIdentifier: string;
    layout: "list" | "board";
    navigate: (path: string) => void;
    statusCounts: { status: string; count: number }[];
    /** LIF-246: true until the counts fetch resolves — swaps the tally
     *  cluster for skeleton chips of the same width instead of the bare
     *  gap that used to sit there for that one frame. */
    countsLoading?: boolean;
    countLabel: string;
    /** LIF-153: how many currently-listed rows changed since the snapshot
     *  taken on the user's last visit. Drives the "N updated" chip; 0 hides
     *  it entirely. */
    changedCount?: number;
    /** Moves keyboard focus to the next changed row (and marks it seen). */
    onCycleChanged?: () => void;
    /** Label + module lists feed the filter modal's Label / Module sections. */
    labels: Label[];
    modules: Module[];
    /** LIF-147: people named on this project's issues, for the filter. */
    assignees?: IssueAssignee[];
    priorityCssColor: (p: string) => string;
    /** The search <input> DOM ref the parent focuses on `/` and openSearch. */
    searchInputEl: HTMLInputElement | null;
    onOpenSearch: () => void;
    onMaybeCollapseSearch: () => void;
    onQuickCreate: () => void;
    /** LIF-234: when false (a viewer on this project, enforcement on), the
     *  "New issue" primary action is hidden — creation is maintainer-gated. */
    canEdit?: boolean;
    /** LIF-149 select-all, relocated into the sub-tab strip. How many rows
     *  the list currently shows (what "Select all" would select), how many
     *  of them are selected, and whether a bulk action is in flight. */
    selectableCount?: number;
    selectedCount?: number;
    selectionBusy?: boolean;
    onSelectAll?: () => void;
    onClearSelection?: () => void;
    /** Board only: the column-visibility pills, rendered at the start of
     *  the toolbar tier so the board body doesn't spend a row on them. */
    boardLeading?: import("svelte").Snippet;
  } = $props();

  // Tri-state for the select-all toggle. "all" flips to clear on click;
  // "none" and "some" both select everything visible, matching the
  // indeterminate-checkbox convention it replaces.
  let selectionState = $derived<"none" | "some" | "all">(
    selectedCount === 0 ? "none"
      : selectedCount >= selectableCount ? "all"
      : "some",
  );

  // Count of active filters — drives the badge on the Filter button. Derived
  // so the topbar re-renders when filters change without manual subscription.
  let filterCount = $derived(view.activeFilterCount());

  // LIF-153: the chip's number is capped so its box has a known maximum
  // width below sm, where the row's whole slack is ~58px. Three glyphs is
  // the ceiling, and "99+" reads as "a lot" just as well as "137" does.
  let changedLabel = $derived(changedCount > 99 ? "99+" : String(changedCount));

  // LIF-308: status counts are server truth (rather than the capped row
  // fetch), so All/Open/Closed stay accurate even for large projects. While
  // they load, omit the numbers instead of presenting a misleading zero.
  let issueSubTabs = $derived.by<SubTab[]>(() => {
    if (countsLoading) {
      return [
        { id: "all", label: "All", count: null },
        { id: "recent", label: "Recent" },
        { id: "open", label: "Open", count: null },
        { id: "closed", label: "Closed", count: null },
      ];
    }

    const countFor = (status: string) =>
      statusCounts.find((entry) => entry.status === status)?.count ?? 0;
    return [
      { id: "all", label: "All", count: statusCounts.reduce((sum, entry) => sum + entry.count, 0) },
      { id: "recent", label: "Recent" },
      { id: "open", label: "Open", count: countFor("backlog") + countFor("todo") + countFor("active") },
      { id: "closed", label: "Closed", count: countFor("done") + countFor("cancelled") },
    ];
  });

  let sortLabel = $derived(
    view.sortField === "age"
      ? "Age"
      : view.sortField === "updated"
        ? "Updated"
        : view.sortField === "number"
          ? "Issue #"
          : "Priority",
  );

  // Compact controls extend through small tablets (LIF-400), below `md`.
  // The panel is a disclosure region, not a menu. Its contents are composite
  // widgets in their own right (SavedViews owns a menu with rename/create
  // forms; Sort and Display each own their own popover), and `role="menu"`
  // would promise arrow-key traversal over a flat list of menuitems that
  // this panel does not and should not have. So the trigger advertises
  // `aria-expanded` + `aria-controls` and nothing more, which is what the
  // interaction actually is: a button that reveals a chunk of the toolbar.
  const OVERFLOW_PANEL_ID = "issue-view-options";
  const FOCUSABLE_SELECTOR =
    'a[href], button:not([disabled]), input:not([disabled]), ' +
    'select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

  let overflowTriggerEl = $state<HTMLButtonElement | null>(null);
  let overflowPanelEl = $state<HTMLDivElement | null>(null);
  let overflowWasOpen = false;

  /** True only while the sub-`md` layout is live: at `md` and up the trigger
   *  is `hidden`, so there is nothing to focus into or back out of. */
  function overflowIsMobile(): boolean {
    return !!overflowTriggerEl && overflowTriggerEl.offsetParent !== null;
  }

  // Focus follows the disclosure both ways. Opening moves focus to the first
  // control in the panel; closing puts it back on the trigger, whichever way
  // the close came (the panel's own trigger, Escape or an outside click,
  // both of which are handled on `window` in IssueList). Restoring is
  // skipped when something else has already claimed focus, so clicking
  // straight from the open panel onto another control doesn't yank it back.
  $effect(() => {
    const open = view.overflowOpen;
    if (open === overflowWasOpen) return;
    overflowWasOpen = open;
    if (!overflowIsMobile()) return;

    if (open) {
      // The panel is revealed by class, not by `{#if}`, so wait a frame for
      // it to be laid out before asking it for a focusable child.
      requestAnimationFrame(() => {
        if (!view.overflowOpen) return;
        overflowPanelEl?.querySelector<HTMLElement>(FOCUSABLE_SELECTOR)?.focus();
      });
      return;
    }

    const active = document.activeElement;
    const cameFromPanel =
      active === null ||
      active === document.body ||
      (active instanceof Node && !!overflowPanelEl?.contains(active));
    if (cameFromPanel) overflowTriggerEl?.focus();
  });
</script>

<!-- Two tiers. The first says where you are and holds the two things you
     reach for most (search and New); the second is the toolbar for shaping
     the list. A third appears only while filters are active and spells them
     out as removable chips. -->
<div class="relative w-full flex flex-col">

  <!-- ── TIER 1: identity, view switch, search, primary action ── -->
  <div class="relative flex items-center gap-2 md:gap-4 px-3 md:px-6 h-12 md:h-[3.25rem]">
    <div class="hidden md:flex items-baseline gap-2 min-w-0 shrink-0">
      <a
        href="#/{projectIdentifier}/overview"
        class="font-mono text-caption text-[var(--text-faint)] hover:text-[var(--text)] transition-colors"
      >
        {projectIdentifier}
      </a>
      <span class="text-[var(--text-faint)] opacity-60" aria-hidden="true">/</span>
      <h1 class="font-display text-heading font-medium tracking-tight text-[var(--text)] leading-none">
        {layout === "board" ? "Board" : "Issues"}
      </h1>
      <span class="min-w-[2ch] text-caption tabular-nums text-[var(--text-faint)]">{countLabel}</span>
    </div>

    <!-- View switcher. Icon-only below md (the labels stay in the
         accessibility tree via sr-only). -->
    <div
      class="flex items-center gap-0.5 p-[3px] rounded-lg bg-[var(--bg)]
             shadow-[inset_0_1px_2px_rgba(0,0,0,0.10)]"
    >
      {#each [["list", "List", `/${projectIdentifier}/issues`], ["board", "Board", `/${projectIdentifier}/board`]] as [mode, label, href] (mode)}
        <button
          class="flex items-center gap-1.5 h-7 px-2 md:px-2.5 rounded-md
                 text-body-sm font-medium transition-[background,color,box-shadow] duration-150
                 {layout === mode
            ? 'bg-[var(--surface)] text-[var(--text)] shadow-[0_1px_2px_rgba(0,0,0,0.16),0_1px_1px_rgba(0,0,0,0.10)]'
            : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
          aria-pressed={layout === mode}
          onclick={() => navigate(href)}
        >
          {#if mode === "list"}
            <ListIcon size={13} class="shrink-0" />
          {:else}
            <LayoutGrid size={13} class="shrink-0" />
          {/if}
          <span class="sr-only md:not-sr-only">{label}</span>
        </button>
      {/each}
    </div>

    <div class="flex-1"></div>

    <!-- LIF-153: "changed since your last visit" chip. List only: its whole
         payload is moving row focus, which the board doesn't have. -->
    {#if layout === "list" && changedCount > 0}
      <Tooltip
        content={`${changedCount} updated since you last looked  ·  click to step through`}
        placement="bottom"
      >
        <button
          class="h-7 flex items-center gap-1.5 px-2 md:px-2.5 rounded-full
                 text-caption font-medium tabular-nums
                 text-[var(--accent)] bg-[var(--accent-subtle)]
                 hover:bg-[var(--accent)] hover:text-[var(--accent-text)]
                 transition-colors"
          aria-label={`${changedCount} updated since you last looked. Focus the next one.`}
          onclick={(e) => {
            e.stopPropagation();
            view.overflowOpen = false;
            onCycleChanged();
          }}
        >
          <span class="relative flex size-1.5 shrink-0" aria-hidden="true">
            <span class="changed-ping absolute inset-0 rounded-full bg-current"></span>
            <span class="relative size-1.5 rounded-full bg-current"></span>
          </span>
          <span>{changedLabel}</span>
          <span class="hidden md:inline">updated</span>
        </button>
      </Tooltip>
    {/if}

    <!-- Search. A real field from md up (no more hunting for the icon);
         below md an icon that opens the field over the whole tier. -->
    {#if !view.searchExpanded}
      <button
        aria-label="Search issues"
        class="md:hidden touch-target size-8 flex items-center justify-center rounded-md
               text-[var(--text-muted)] hover:text-[var(--text)]
               hover:bg-[var(--bg-subtle)] transition-colors"
        onclick={(e) => { e.stopPropagation(); view.overflowOpen = false; onOpenSearch(); }}
      >
        <Search size={15} />
      </button>
    {/if}
    <div
      class="{view.searchExpanded ? 'max-md:flex' : 'max-md:hidden'}
             max-md:absolute max-md:inset-0 max-md:z-20 max-md:bg-[var(--chrome)]
             max-md:items-center max-md:px-3"
    >
      <div class="relative w-full md:w-auto group/search">
        <div class="absolute left-2.5 top-1/2 -translate-y-1/2 pointer-events-none text-[var(--text-faint)]
                    group-focus-within/search:text-[var(--accent)] transition-colors">
          <Search size={13} />
        </div>
        <input
          type="text"
          placeholder="Search issues..."
          enterkeyhint="search"
          aria-label="Search issues"
          bind:this={searchInputEl}
          bind:value={view.searchQuery}
          onblur={onMaybeCollapseSearch}
          onkeydown={(e) => {
            if (e.key === "Escape") {
              e.preventDefault();
              view.searchQuery = "";
              view.searchExpanded = false;
              (e.currentTarget as HTMLInputElement).blur();
            }
          }}
          class="peer w-full h-8 md:w-[220px] md:focus:w-[300px] pl-8 pr-8 text-body-sm rounded-lg
                 border border-[var(--border)] bg-[var(--bg)]
                 text-[var(--text)] placeholder:text-[var(--text-faint)]
                 hover:border-[var(--text-faint)]
                 focus:border-[var(--accent)] focus:bg-[var(--surface)]
                 focus:shadow-[0_0_0_3px_var(--accent-subtle)]
                 outline-none transition-[width,border-color,box-shadow,background] duration-200
                 {view.searchQuery ? 'md:w-[300px]' : ''}"
        />
        {#if view.searchQuery}
          <button
            class="absolute right-1.5 top-1/2 -translate-y-1/2 size-5 grid place-items-center rounded
                   text-[var(--text-faint)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]"
            aria-label="Clear search"
            onmousedown={(e) => e.preventDefault()}
            onclick={(e) => { e.stopPropagation(); view.searchQuery = ""; searchInputEl?.focus(); }}
          >
            <X size={12} />
          </button>
        {:else}
          <kbd
            class="hidden md:grid absolute right-2 top-1/2 -translate-y-1/2 pointer-events-none
                   place-items-center min-w-[1.15rem] h-[1.15rem] px-1 rounded
                   border border-[var(--border)] font-mono text-micro text-[var(--text-faint)]
                   peer-focus:opacity-0 transition-opacity"
          >/</kbd>
        {/if}
      </div>
    </div>

    <!-- LIF-245: shared Shortcut Help overlay. Hidden below md (no keyboard). -->
    <div class="relative hidden md:block">
      <Tooltip content="Shortcuts  ·  ?" placement="bottom">
        <button
          class="size-8 flex items-center justify-center rounded-md
                 text-[var(--text-muted)] hover:text-[var(--text)]
                 hover:bg-[var(--bg-subtle)] transition-colors"
          aria-label="Keyboard shortcuts"
          onclick={(e) => { e.stopPropagation(); toggleShortcutHelp(); }}
        >
          <HelpCircle size={15} />
        </button>
      </Tooltip>
    </div>

    <!-- Primary action: New issue. Split button: the main segment opens the
         inline composer; the caret reveals the other ways in. Hidden for
         viewers (LIF-234). -->
    {#if canEdit}
    <div class="relative">
      <div
        class="flex items-stretch h-8 rounded-lg overflow-hidden
               shadow-[0_1px_2px_rgba(0,0,0,0.18),inset_0_1px_0_rgba(255,255,255,0.18)]
               focus-within:ring-2 focus-within:ring-[var(--btn-success)]
               focus-within:ring-offset-1
               focus-within:ring-offset-[var(--chrome)]"
      >
        <button
          class="group flex items-center gap-1.5 px-2.5 md:pl-3 md:pr-2.5
                 text-body-sm font-medium text-[var(--btn-success-text)]
                 bg-[var(--btn-success)] hover:bg-[var(--btn-success-hover)]
                 transition-colors focus:outline-none
                 motion-safe:active:scale-[0.97]"
          aria-label="New issue"
          onclick={(e) => {
            e.stopPropagation();
            view.newMenuOpen = false;
            view.overflowOpen = false;
            onQuickCreate();
          }}
        >
          <Plus
            size={15}
            class="motion-safe:transition-transform motion-safe:duration-300
                   motion-safe:group-hover:rotate-90"
          />
          <span class="hidden md:inline">New issue</span>
          <kbd
            class="hidden md:grid ml-0.5 place-items-center min-w-[1.1rem] h-[1.1rem]
                   rounded bg-black/10 font-mono text-micro leading-none"
          >
            C
          </kbd>
        </button>
        <div class="w-px bg-black/15"></div>
        <button
          class="flex items-center justify-center px-1.5
                 text-[var(--btn-success-text)] bg-[var(--btn-success)]
                 hover:bg-[var(--btn-success-hover)] transition-colors
                 focus:outline-none motion-safe:active:scale-[0.97]"
          aria-label="More create options"
          aria-haspopup="menu"
          aria-expanded={view.newMenuOpen}
          onclick={(e) => {
            e.stopPropagation();
            view.newMenuOpen = !view.newMenuOpen;
            view.sortOpen = false;
            view.displayOpen = false;
            view.lanesOpen = false;
            view.overflowOpen = false;
          }}
        >
          <ChevronDown
            size={14}
            class="motion-safe:transition-transform {view.newMenuOpen ? 'rotate-180' : ''}"
          />
        </button>
      </div>

      {#if view.newMenuOpen}
        <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
        <div
          role="menu"
          tabindex="-1"
          class="popover-in origin-top-right absolute right-0 top-full mt-1.5 z-30 w-[220px]
                 bg-[var(--surface)] border border-[var(--border)]
                 rounded-xl shadow-lg p-1"
          onclick={(e) => e.stopPropagation()}
        >
          <button
            role="menuitem"
            class="w-full flex items-center gap-2.5 px-2.5 h-8 rounded-md text-left
                   text-body-sm text-[var(--text)]
                   hover:bg-[var(--bg-subtle)] transition-colors"
            onclick={() => {
              view.newMenuOpen = false;
              onQuickCreate();
            }}
          >
            <Zap size={14} class="text-[var(--success)]" />
            <span class="flex-1">Quick create</span>
            <kbd
              class="px-1.5 py-0.5 rounded border border-[var(--border)]
                     bg-[var(--bg-subtle)] text-[var(--text)] font-mono
                     text-micro leading-none shrink-0"
            >
              C
            </kbd>
          </button>
          <button
            role="menuitem"
            class="w-full flex items-center gap-2.5 px-2.5 h-8 rounded-md text-left
                   text-body-sm text-[var(--text)]
                   hover:bg-[var(--bg-subtle)] transition-colors"
            onclick={() => {
              view.newMenuOpen = false;
              navigate(`/${projectIdentifier}/issues/new`);
            }}
          >
            <PenLine size={14} class="text-[var(--text-muted)]" />
            <span class="flex-1">Open full editor</span>
          </button>

          <div class="my-1 mx-1 h-px bg-[var(--border)]"></div>
          <div
            class="px-2.5 pb-1 pt-1 text-micro uppercase tracking-widest
                   font-semibold text-[var(--text-faint)]"
          >
            New in status
          </div>
          {#each ["backlog", "todo", "active"] as s}
            <button
              role="menuitem"
              class="w-full flex items-center gap-2.5 px-2.5 h-8 rounded-md text-left
                     text-body-sm capitalize text-[var(--text)]
                     hover:bg-[var(--bg-subtle)] transition-colors"
              onclick={() => {
                view.newMenuOpen = false;
                navigate(`/${projectIdentifier}/issues/new?status=${s}`);
              }}
            >
              <StatusIcon status={s} size={14} />
              <span class="flex-1">{s}</span>
            </button>
          {/each}
        </div>
      {/if}
    </div>
    {:else if isSignedOut()}
      <!-- LIF-583: where New issue would be, for a signed-out reader. -->
      <SignInButton />
    {/if}
  </div>

  <!-- ── TIER 2: slices (list) or columns (board), then the view tools ── -->
  <div class="flex items-stretch gap-3 px-3 md:px-6 h-11 border-b border-[var(--border)]">
    <div class="min-w-0 flex-1 flex items-end">
      {#if layout === "list"}
        <SubTabs
          tabs={issueSubTabs}
          active={view.issueSubTab}
          onselect={(id) => view.selectIssueSubTab(id)}
          border={false}
        />
      {:else if boardLeading}
        <div class="self-center min-w-0 overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
          {@render boardLeading()}
        </div>
      {/if}
    </div>

    <div class="shrink-0 flex items-center gap-1">
      <!-- Unified filter (LIF-222). The chips tier below says what's on. -->
      <Tooltip content={view.filterOpen ? null : "Filter  ·  F"} placement="bottom">
        <button
          class="h-7 flex items-center gap-1.5 px-2 rounded-md
                 text-caption font-medium transition-colors
                 hover:bg-[var(--bg-subtle)]
                 {view.filterOpen || filterCount > 0
                   ? 'text-[var(--text)] bg-[var(--bg-subtle)]'
                   : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
          onclick={(e) => {
            e.stopPropagation();
            view.filterOpen = !view.filterOpen;
            view.sortOpen = false;
            view.displayOpen = false;
            view.lanesOpen = false;
            view.newMenuOpen = false;
            view.overflowOpen = false;
          }}
        >
          <SlidersVertical size={13} class="shrink-0" />
          <span class="sr-only sm:not-sr-only">Filter</span>
          {#if filterCount > 0}
            <span
              class="grid place-items-center min-w-[1.05rem] h-[1.05rem] px-1
                     rounded-full bg-[var(--accent)] text-[var(--accent-text)]
                     font-mono text-micro leading-none tabular-nums"
            >
              {filterCount}
            </span>
          {/if}
        </button>
      </Tooltip>

      <FilterModal {view} {labels} {modules} {assignees} {priorityCssColor} />

      <!-- LIF-350 / LIF-400: below md, saved views, sort and display fold
           behind one button; at md and up both wrappers are
           `display: contents`, so the controls sit in this row directly. -->
      <div class="max-md:relative md:contents">
        <button
          class="md:hidden touch-target size-7 flex items-center justify-center rounded-md
                 text-[var(--text-muted)] hover:text-[var(--text)]
                 hover:bg-[var(--bg-subtle)] transition-colors
                 {view.overflowOpen ? 'text-[var(--text)] bg-[var(--bg-subtle)]' : ''}"
          aria-label="View options"
          aria-expanded={view.overflowOpen}
          aria-controls={OVERFLOW_PANEL_ID}
          bind:this={overflowTriggerEl}
          onclick={(e) => {
            e.stopPropagation();
            view.overflowOpen = !view.overflowOpen;
            view.sortOpen = false;
            view.displayOpen = false;
            view.lanesOpen = false;
            view.filterOpen = false;
            view.newMenuOpen = false;
          }}
        >
          <MoreHorizontal size={14} />
        </button>
        <div
          id={OVERFLOW_PANEL_ID}
          bind:this={overflowPanelEl}
          class="md:contents max-md:absolute max-md:right-0 max-md:top-full max-md:mt-1.5
                 max-md:z-30 max-md:w-[212px] max-md:flex-col max-md:gap-0.5 max-md:p-1
                 max-md:rounded-xl max-md:border max-md:border-[var(--border)]
                 max-md:bg-[var(--surface)] max-md:shadow-lg
                 {view.overflowOpen ? 'max-md:flex' : 'max-md:hidden'}"
        >

    <SavedViews {view} {projectIdentifier} {layout} {navigate} />

    <div class="relative max-md:[&>span]:w-full">
      <Tooltip
        content={view.sortOpen ? null : `Sort: ${sortLabel} ${view.sortDir === "asc" ? "ascending" : "descending"}`}
        placement="bottom"
      >
        <button
          class="h-7 flex items-center gap-1 px-2 rounded-md
                 text-caption font-medium
                 text-[var(--text-muted)] hover:text-[var(--text)]
                 hover:bg-[var(--bg-subtle)] transition-colors
                 {view.sortOpen ? 'text-[var(--text)] bg-[var(--bg-subtle)]' : ''}"
          onclick={(e) => {
            e.stopPropagation();
            view.sortOpen = !view.sortOpen;
            view.displayOpen = false;
            view.lanesOpen = false;
          }}
        >
          <span class="flex motion-safe:transition-transform motion-safe:duration-200 {view.sortDir === 'asc' ? '' : 'rotate-180'}">
            <ArrowUp size={12} class="shrink-0" />
          </span>
          <span class="md:hidden">Sort:</span>
          <span>{sortLabel}</span>
        </button>
      </Tooltip>
      {#if view.sortOpen}
        <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
        <div
          class="popover-in origin-top-right absolute right-0 top-full mt-1.5 z-30 w-[220px]
                 max-md:right-auto max-md:left-0
                 bg-[var(--surface)] border border-[var(--border)]
                 rounded-xl shadow-lg p-1 text-body-sm"
          onclick={(e) => e.stopPropagation()}
        >
          <div class="px-2.5 pt-1.5 pb-1 text-[var(--text-faint)]
                      text-micro uppercase tracking-widest font-semibold">
            Sort by
          </div>
          {#snippet sortRow(field: SortField, label: string, Icon: typeof Hash)}
            {@const active = view.sortField === field}
            <button
              class="w-full flex items-center justify-between gap-2
                     px-2.5 h-8 rounded-md text-left transition-colors
                     {active
                ? 'text-[var(--text)] bg-[var(--bg-subtle)] font-medium'
                : 'text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
              onclick={() => view.selectSort(field)}
            >
              <span class="flex items-center gap-2">
                <Icon size={13} class="shrink-0" />
                {label}
              </span>
              {#if active}
                <span class="text-[var(--accent)] flex items-center gap-1 text-micro font-medium">
                  {view.sortDir === "asc" ? "Asc" : "Desc"}
                  {#if view.sortDir === "asc"}
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
          <div class="px-2.5 pt-2 pb-1.5 mt-1 text-micro
                      text-[var(--text-faint)] border-t
                      border-[var(--border)] leading-snug">
            Click the active row to flip direction.
          </div>
        </div>
      {/if}
    </div>

    <!-- LIF-241: swimlane picker. Board only. -->
    {#if layout === "board"}
    <div class="relative max-md:[&>span]:w-full">
      <Tooltip content={view.lanesOpen ? null : "Swimlanes"} placement="bottom">
        <button
          class="h-7 flex items-center gap-1 px-2 rounded-md
                 text-caption font-medium
                 text-[var(--text-muted)] hover:text-[var(--text)]
                 hover:bg-[var(--bg-subtle)] transition-colors
                 {view.lanesOpen || view.laneBy !== 'none' ? 'text-[var(--text)] bg-[var(--bg-subtle)]' : ''}"
          onclick={(e) => {
            e.stopPropagation();
            view.lanesOpen = !view.lanesOpen;
            view.sortOpen = false;
            view.newMenuOpen = false;
            view.filterOpen = false;
          }}
        >
          <Rows3 size={12} class="shrink-0" />
          <span>
            {view.laneBy === "none" ? "Lanes" : view.laneBy === "module" ? "Module" : "Priority"}
          </span>
        </button>
      </Tooltip>
      {#if view.lanesOpen}
        <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
        <div
          class="popover-in origin-top-right absolute right-0 top-full mt-1.5 z-30 w-[200px]
                 max-md:right-auto max-md:left-0
                 bg-[var(--surface)] border border-[var(--border)]
                 rounded-xl shadow-lg p-1 text-body-sm"
          onclick={(e) => e.stopPropagation()}
        >
          <div class="px-2.5 pt-1.5 pb-1 text-[var(--text-faint)] text-micro uppercase tracking-widest font-semibold">
            Group rows by
          </div>
          {#snippet laneRow(val: LaneBy, label: string, Icon: typeof Layers | null)}
            {@const active = view.laneBy === val}
            <button
              class="w-full flex items-center justify-between gap-2 px-2.5 h-8 rounded-md text-left transition-colors
                     {active
                ? 'text-[var(--text)] bg-[var(--bg-subtle)] font-medium'
                : 'text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
              onclick={() => { view.setLaneBy(projectIdentifier, val); }}
            >
              <span class="flex items-center gap-2">
                {#if Icon}<Icon size={13} class="shrink-0" />{:else}<span class="size-[13px] shrink-0"></span>{/if}
                {label}
              </span>
              {#if active}<Check size={13} class="text-[var(--accent)]" />{/if}
            </button>
          {/snippet}
          {@render laneRow("none", "None", null)}
          {@render laneRow("module", "Module", Layers)}
          {@render laneRow("priority", "Priority", Signal)}
          <div class="px-2.5 pt-2 pb-1.5 mt-1 text-micro
                      text-[var(--text-faint)] border-t
                      border-[var(--border)] leading-snug">
            Rows always show, even at zero. Drag a card in to assign it.
          </div>
        </div>
      {/if}
    </div>
    {/if}

    <!-- LIF-191: display options (group-by + density). List only. -->
    {#if layout !== "board"}
    <div class="relative max-md:[&>span]:w-full">
      <Tooltip content={view.displayOpen ? null : "Display options"} placement="bottom">
        <button
          class="touch-target h-7 flex items-center gap-1.5 px-2 rounded-md
                 max-md:w-full max-md:justify-start
                 text-caption font-medium
                 text-[var(--text-muted)] hover:text-[var(--text)]
                 hover:bg-[var(--bg-subtle)] transition-colors
                 {view.displayOpen ? 'text-[var(--text)] bg-[var(--bg-subtle)]' : ''}"
          onclick={(e) => { e.stopPropagation(); view.displayOpen = !view.displayOpen; view.sortOpen = false; view.newMenuOpen = false; }}
        >
          <SlidersHorizontal size={13} class="shrink-0" />
          <span>Display</span>
        </button>
      </Tooltip>
      {#if view.displayOpen}
        <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
        <div
          class="popover-in origin-top-right absolute right-0 top-full mt-1.5 z-30 w-[260px]
                 max-md:right-auto max-md:left-0
                 bg-[var(--surface)] border border-[var(--border)]
                 rounded-xl shadow-lg p-3 text-body-sm"
          onclick={(e) => e.stopPropagation()}
        >
          <div class="pb-2 text-[var(--text-faint)] text-micro uppercase tracking-widest font-semibold">
            Group by
          </div>
          <div class="grid grid-cols-2 gap-1">
            {#each [["status", "Status"], ["priority", "Priority"], ["module", "Module"], ["none", "None"]] as [val, label]}
              <button
                class="h-8 flex items-center justify-center gap-1.5 rounded-md border transition-colors
                       {view.groupBy === val
                  ? 'border-[var(--accent)] text-[var(--text)] bg-[var(--accent-subtle)] font-medium'
                  : 'border-[var(--border)] text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
                aria-pressed={view.groupBy === val}
                onclick={() => { view.groupBy = val as GroupBy; }}
              >
                {label}
              </button>
            {/each}
          </div>

          <div class="pt-3.5 pb-2 text-[var(--text-faint)] text-micro uppercase tracking-widest font-semibold">
            Density
          </div>
          <div class="grid grid-cols-2 gap-1">
            {#each [["compact", "Compact"], ["comfortable", "Comfortable"]] as [val, label]}
              <button
                class="flex flex-col items-center gap-1.5 py-2 rounded-md border transition-colors
                       {view.density === val
                  ? 'border-[var(--accent)] text-[var(--text)] bg-[var(--accent-subtle)] font-medium'
                  : 'border-[var(--border)] text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
                aria-pressed={view.density === val}
                onclick={() => { view.density = val as Density; }}
              >
                <!-- A tiny picture of the rows, so the choice is visible. -->
                <span class="flex flex-col w-10 {val === 'compact' ? 'gap-[3px]' : 'gap-[6px]'}" aria-hidden="true">
                  {#each [0, 1, 2] as n (n)}
                    <span class="h-[3px] rounded-full bg-current opacity-50 {n === 1 ? 'w-3/4' : 'w-full'}"></span>
                  {/each}
                </span>
                {label}
              </button>
            {/each}
          </div>
        </div>
      {/if}
    </div>
    {/if}
        </div>
      </div>

      <!-- Select all (LIF-149). Desktop only: below sm rows are tapped one
           at a time and the bulk bar handles the rest. -->
      {#if layout === "list" && canEdit && selectableCount > 0}
        <div class="hidden sm:block w-px h-4 bg-[var(--border)] mx-1"></div>
        <Tooltip
          content={selectionState === "all"
            ? "Clear selection  ·  Esc"
            : "Select all visible issues  ·  Ctrl+A"}
          placement="bottom"
        >
          <button
            type="button"
            aria-pressed={selectionState === "all"}
            class="hidden sm:flex h-7 items-center gap-1.5 px-2 rounded-md
                   text-caption font-medium transition-colors
                   disabled:opacity-50 disabled:cursor-not-allowed
                   {selectionState === 'none'
              ? 'text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'
              : 'text-[var(--text)] bg-[var(--bg-subtle)]'}"
            disabled={selectionBusy}
            onclick={() => selectionState === "all" ? onClearSelection() : onSelectAll()}
          >
            {#if selectionState === "all"}
              <SquareCheck size={14} class="shrink-0 text-[var(--accent)]" />
            {:else if selectionState === "some"}
              <SquareMinus size={14} class="shrink-0 text-[var(--accent)]" />
            {:else}
              <Square size={14} class="shrink-0" />
            {/if}
            <span>{selectionState === "all" ? "All selected" : "Select all"}</span>
            <span class="text-micro tabular-nums font-normal
                         {selectionState === 'none' ? 'text-[var(--text-faint)]' : 'text-[var(--text-muted)]'}">
              {selectionState === "some" ? `${selectedCount}/${selectableCount}` : selectableCount}
            </span>
          </button>
        </Tooltip>
      {/if}
    </div>
  </div>

  <!-- ── TIER 3: what the filters are doing, only while they do something ── -->
  {#if view.hasActiveFilters()}
    <div class="tier-in px-3 md:px-6 py-2 border-b border-[var(--border)]">
      <FilterChips {view} {labels} {modules} includeSearch={false} />
    </div>
  {/if}
</div>

<style>
  @keyframes ping {
    75%,
    100% {
      transform: scale(2.4);
      opacity: 0;
    }
  }
  .changed-ping {
    animation: ping 1.6s cubic-bezier(0, 0, 0.2, 1) 3;
  }
</style>
