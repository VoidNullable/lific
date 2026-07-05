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
  import type { Label, Module } from "../api";
  import {
    Plus, Search, ChevronDown, Signal,
    List as ListIcon, LayoutGrid, SlidersHorizontal, HelpCircle,
    ArrowDown, ArrowUp, Hash, Clock, History, Check, Zap, PenLine,
    SlidersVertical, Rows3, Layers,
  } from "lucide-svelte";
  import Tooltip from "../Tooltip.svelte";
  import Breadcrumbs from "../Breadcrumbs.svelte";
  import StatusIcon from "../StatusIcon.svelte";
  import Skeleton from "../Skeleton.svelte";
  import FilterModal from "./FilterModal.svelte";
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
    labels,
    modules,
    priorityCssColor,
    searchInputEl = $bindable(),
    onOpenSearch,
    onMaybeCollapseSearch,
    onQuickCreate,
    canEdit = true,
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
    /** Label + module lists feed the filter modal's Label / Module sections. */
    labels: Label[];
    modules: Module[];
    priorityCssColor: (p: string) => string;
    /** The search <input> DOM ref the parent focuses on `/` and openSearch. */
    searchInputEl: HTMLInputElement | null;
    onOpenSearch: () => void;
    onMaybeCollapseSearch: () => void;
    onQuickCreate: () => void;
    /** LIF-234: when false (a viewer on this project, enforcement on), the
     *  "New issue" primary action is hidden — creation is maintainer-gated. */
    canEdit?: boolean;
  } = $props();

  // Count of active filters — drives the badge on the Filter button. Derived
  // so the topbar re-renders when filters change without manual subscription.
  let filterCount = $derived(view.activeFilterCount());
</script>

<div class="relative flex items-center gap-2 sm:gap-3 px-3 sm:px-6 py-2 w-full">

  <!-- ── LEFT ZONE: scope + view switcher ───────────────────── -->
  <div class="flex items-center gap-2 sm:gap-3 shrink-0 min-w-0">
    <!-- Breadcrumb (LIF-286: shared component). The project segment + its
         separator collapse below sm (the mobile header already shows the
         app name); the page label stays. -->
    <Breadcrumbs
      segments={[
        { label: projectIdentifier, href: `#/${projectIdentifier}/overview`, mono: true, hideBelowSm: true },
        { label: layout === "board" ? "Board" : "Issues" },
      ]}
    />

    <!-- View switcher pill. Anchored directly after the breadcrumb so the
         toggle never shifts when the per-status tallies (which arrive a
         frame later, after the counts fetch) render in beside it. -->
    <div
      class="flex items-center gap-0.5 p-0.5 rounded-md bg-[var(--bg)]
             shadow-[inset_0_1px_2px_rgba(0,0,0,0.10)]"
    >
      <button
        class="flex items-center gap-1 px-2 py-0.5 rounded
               text-caption font-medium transition
               {layout === 'list'
          ? 'bg-[var(--surface)] text-[var(--text)] shadow-[0_1px_2px_rgba(0,0,0,0.16),0_1px_1px_rgba(0,0,0,0.10)]'
          : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
        aria-pressed={layout === "list"}
        onclick={() => navigate(`/${projectIdentifier}/issues`)}
      >
        <ListIcon size={11} class="shrink-0" />
        List
      </button>
      <button
        class="flex items-center gap-1 px-2 py-0.5 rounded
               text-caption font-medium transition
               {layout === 'board'
          ? 'bg-[var(--surface)] text-[var(--text)] shadow-[0_1px_2px_rgba(0,0,0,0.16),0_1px_1px_rgba(0,0,0,0.10)]'
          : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
        aria-pressed={layout === "board"}
        onclick={() => navigate(`/${projectIdentifier}/board`)}
      >
        <LayoutGrid size={11} class="shrink-0" />
        Board
      </button>
    </div>

    <!-- LIF-161: per-status tallies (server truth, immune to the list fetch
         cap). Clicking one toggles the matching status filter. Gated on at
         least one non-zero tally. -->
    {#if countsLoading}
      <!-- LIF-281: parity with the real tally cluster below — same
           container (gap-0.5) and same per-chip box (h-6, px-1.5, icon +
           count), so the tallies don't shift when the counts fetch lands. -->
      <div class="hidden md:flex items-center gap-0.5">
        {#each [0, 1, 2] as i (i)}
          <div class="h-6 flex items-center gap-1 px-1.5 rounded">
            <Skeleton variant="circle" class="size-3 shrink-0" />
            <Skeleton variant="bar" class="h-2.5 w-3" />
          </div>
        {/each}
      </div>
    {:else if statusCounts.some((s) => s.count > 0)}
      <div class="hidden md:flex items-center gap-0.5">
        {#each statusCounts as { status, count } (status)}
          {#if count > 0}
            <Tooltip
              content={`${count} ${status}${view.filterStatus === status ? "  ·  click to clear" : ""}`}
              placement="bottom"
            >
              <button
                class="h-6 flex items-center gap-1 px-1.5 rounded
                       text-micro font-medium tabular-nums
                       transition-colors
                       {view.filterStatus === status
                  ? 'bg-[var(--bg-subtle)] text-[var(--text)]'
                  : 'text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
                onclick={() => view.toggleStatusFilter(status)}
              >
                <StatusIcon {status} size={12} />
                {count}
              </button>
            </Tooltip>
          {/if}
        {/each}
      </div>
    {/if}
  </div>

  <!-- Separator -->
  <div class="w-px h-4 bg-[var(--border)]"></div>

  <!-- ── FILTER (unified popover; LIF-222) ────────────────────
       Replaces the previous row of up to four inline <Select>
       triggers (Status / Priority / Labels / Modules) plus a
       standalone Clear button. The popover stacks all four
       sections vertically with section labels matching the
       LIF-DOC-14 §7 popover language, and the trigger carries
       a small accent badge with the count of active filters. -->
  <div class="relative">
    <Tooltip content={view.filterOpen ? null : "Filter"} placement="bottom">
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
        }}
      >
        <SlidersVertical size={12} class="shrink-0" />
        <span>Filter</span>
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

  </div>

  <!-- Full filter modal (LIF-222 follow-up). Lives outside the trigger
       wrapper but is fixed-positioned, so DOM placement is irrelevant. -->
  <FilterModal {view} {labels} {modules} {priorityCssColor} />

  <!-- ── RIGHT ZONE: display / search / help / primary action ── -->
  <div class="ml-auto flex items-center gap-0.5 shrink-0">

    <!-- Issue count. Reserved min-width so the brief load frame can't reflow.
         Hidden below sm to save horizontal room on phones. -->
    <span
      class="hidden sm:inline mr-1.5 min-w-[2ch] text-right text-micro tabular-nums
             font-medium text-[var(--text-faint)]"
    >
      {countLabel}
    </span>
    <div class="hidden sm:block w-px h-4 bg-[var(--border)] mr-1"></div>

    <!-- LIF-242: saved views. Self-contained — hides itself when /api/me
         fails (logged out / OAuth-token-only / legacy key). -->
    <SavedViews {view} {projectIdentifier} {layout} {navigate} />

    <!-- Sort button + popover. -->
    <div class="relative">
      <Tooltip
        content={view.sortOpen
          ? null
          : `Sort: ${view.sortField === "age" ? "Age" : view.sortField === "updated" ? "Updated" : view.sortField === "number" ? "Issue #" : "Priority"} ${view.sortDir === "asc" ? "ascending" : "descending"}`}
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
          {#if view.sortDir === "asc"}
            <ArrowUp size={12} class="shrink-0" />
          {:else}
            <ArrowDown size={12} class="shrink-0" />
          {/if}
          <span class="hidden sm:inline">
            {view.sortField === "age"
              ? "Age"
              : view.sortField === "updated"
                ? "Updated"
                : view.sortField === "number"
                  ? "Issue #"
                  : "Priority"}
          </span>
        </button>
      </Tooltip>
      {#if view.sortOpen}
        <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
        <div
          class="absolute right-0 top-full mt-1.5 z-30 w-[220px]
                 bg-[var(--surface)] border border-[var(--border)]
                 rounded-lg shadow-lg py-1.5 text-body-sm"
          onclick={(e) => e.stopPropagation()}
        >
          <div class="px-3 pt-1 pb-1.5 text-[var(--text-faint)]
                      text-micro uppercase tracking-widest
                      font-semibold">
            Sort by
          </div>
          {#snippet sortRow(field: SortField, label: string, Icon: typeof Hash)}
            {@const active = view.sortField === field}
            <button
              class="w-full flex items-center justify-between gap-2
                     px-3 py-1.5 text-left transition-colors
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
                <span class="text-[var(--accent)] flex items-center">
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
          <div class="px-3 pt-2 pb-1 mt-1 text-micro
                      text-[var(--text-faint)] border-t
                      border-[var(--border)] leading-snug">
            Click the active row to flip direction.
          </div>
        </div>
      {/if}
    </div>

    <!-- LIF-241: Swimlane picker. Board view only — splits the board into
         horizontal bands (module / priority) on top of the status columns. -->
    {#if layout === "board"}
    <div class="relative">
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
          <span class="hidden sm:inline">
            {view.laneBy === "none" ? "Lanes" : view.laneBy === "module" ? "Module" : "Priority"}
          </span>
        </button>
      </Tooltip>
      {#if view.lanesOpen}
        <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
        <div
          class="absolute right-0 top-full mt-1.5 z-30 w-[188px]
                 bg-[var(--surface)] border border-[var(--border)]
                 rounded-lg shadow-lg py-1.5 text-body-sm"
          onclick={(e) => e.stopPropagation()}
        >
          <div class="px-3 pt-1 pb-1.5 text-[var(--text-faint)] text-micro uppercase tracking-widest font-semibold">
            Group rows by
          </div>
          {#snippet laneRow(val: LaneBy, label: string, Icon: typeof Layers | null)}
            {@const active = view.laneBy === val}
            <button
              class="w-full flex items-center justify-between gap-2 px-3 py-1.5 text-left transition-colors
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
          <div class="px-3 pt-2 pb-1 mt-1 text-micro
                      text-[var(--text-faint)] border-t
                      border-[var(--border)] leading-snug">
            Rows always show, even at zero — drag a card in to assign it.
          </div>
        </div>
      {/if}
    </div>
    {/if}

    <!-- LIF-191: Display options — group-by + density. List view only. -->
    {#if layout !== "board"}
    <div class="relative">
      <Tooltip content={view.displayOpen ? null : "Display options"} placement="bottom">
        <button
          class="size-7 flex items-center justify-center rounded-md
                 text-[var(--text-muted)] hover:text-[var(--text)]
                 hover:bg-[var(--bg-subtle)] transition-colors
                 {view.displayOpen ? 'text-[var(--text)] bg-[var(--bg-subtle)]' : ''}"
          onclick={(e) => { e.stopPropagation(); view.displayOpen = !view.displayOpen; view.sortOpen = false; view.newMenuOpen = false; }}
        >
          <SlidersHorizontal size={14} />
        </button>
      </Tooltip>
      {#if view.displayOpen}
        <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
        <div
          class="absolute right-0 top-full mt-1.5 z-30 w-[224px]
                 bg-[var(--surface)] border border-[var(--border)]
                 rounded-lg shadow-lg py-1.5 text-body-sm"
          onclick={(e) => e.stopPropagation()}
        >
          <div class="px-3 pt-1 pb-1.5 text-[var(--text-faint)] text-micro uppercase tracking-widest font-semibold">
            Group by
          </div>
          {#each [["status", "Status"], ["priority", "Priority"], ["module", "Module"], ["none", "None"]] as [val, label]}
            <button
              class="w-full flex items-center justify-between gap-2 px-3 py-1.5 text-left transition-colors
                     {view.groupBy === val
                ? 'text-[var(--text)] bg-[var(--bg-subtle)] font-medium'
                : 'text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
              onclick={() => { view.groupBy = val as GroupBy; }}
            >
              {label}
              {#if view.groupBy === val}<Check size={13} class="text-[var(--accent)]" />{/if}
            </button>
          {/each}

          <div class="px-3 pt-2.5 pb-1.5 mt-1 text-[var(--text-faint)] text-micro uppercase tracking-widest font-semibold border-t border-[var(--border)]">
            Density
          </div>
          {#each [["compact", "Compact"], ["comfortable", "Comfortable"]] as [val, label]}
            <button
              class="w-full flex items-center justify-between gap-2 px-3 py-1.5 text-left transition-colors
                     {view.density === val
                ? 'text-[var(--text)] bg-[var(--bg-subtle)] font-medium'
                : 'text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
              onclick={() => { view.density = val as Density; }}
            >
              {label}
              {#if view.density === val}<Check size={13} class="text-[var(--accent)]" />{/if}
            </button>
          {/each}
        </div>
      {/if}
    </div>
    {/if}

    <!-- Search: collapsed to icon, expands inline on click or `/`.
         Below sm the expanded field can't join the row — the row is already
         full of shrink-0 controls, and adding a fixed-width input pushes the
         row's min width past the viewport (LIF-271: the whole list appeared
         "zoomed out"). Instead it overlays the entire topbar row. -->
    {#if view.searchExpanded}
      <div
        class="max-sm:absolute max-sm:inset-0 max-sm:z-20 max-sm:bg-[var(--chrome)]
               max-sm:flex max-sm:items-center max-sm:px-3 max-sm:py-1.5"
      >
        <div class="relative w-full sm:w-auto">
          <div class="absolute left-2 top-1/2 -translate-y-1/2 pointer-events-none text-[var(--text-faint)]">
            <Search size={12} />
          </div>
          <!-- svelte-ignore a11y_autofocus -->
          <input
            type="text"
            placeholder="Search issues..."
            enterkeyhint="search"
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
            class="w-full sm:w-[200px] pl-7 pr-2 py-1 text-body-sm rounded-md
                   border border-[var(--border)] bg-[var(--surface)]
                   text-[var(--text)] placeholder:text-[var(--text-faint)]
                   focus:border-[var(--accent)]
                   focus:shadow-[0_0_0_3px_var(--accent-subtle)]
                   outline-none transition-colors"
          />
        </div>
      </div>
    {:else}
      <Tooltip content="Search  ·  /" placement="bottom">
        <button
          class="size-7 flex items-center justify-center rounded-md
                 text-[var(--text-muted)] hover:text-[var(--text)]
                 hover:bg-[var(--bg-subtle)] transition-colors"
          onclick={(e) => { e.stopPropagation(); onOpenSearch(); }}
        >
          <Search size={14} />
        </button>
      </Tooltip>
    {/if}

    <!-- LIF-245: opens the shared Shortcut Help overlay (Layout.svelte),
         sourced from the lib/shortcuts.ts registry — no longer a
         topbar-local popover with its own hand-maintained list. Hidden
         below md since there's no keyboard to shortcut with on touch. -->
    <div class="relative hidden md:block">
      <Tooltip content="Shortcuts  ·  ?" placement="bottom">
        <button
          class="size-7 flex items-center justify-center rounded-md
                 text-[var(--text-muted)] hover:text-[var(--text)]
                 hover:bg-[var(--bg-subtle)] transition-colors"
          onclick={(e) => { e.stopPropagation(); toggleShortcutHelp(); }}
        >
          <HelpCircle size={14} />
        </button>
      </Tooltip>
    </div>

    <!-- Separator -->
    {#if canEdit}
    <div class="w-px h-4 bg-[var(--border)] mx-1.5"></div>

    <!-- Primary action: New issue. Split button — main segment opens the
         inline quick-create row; the caret reveals alternative paths.
         Hidden for viewers (LIF-234) — creation is maintainer-gated. -->
    <div class="relative">
      <div
        class="flex items-stretch h-7 rounded-md overflow-hidden shadow-sm
               focus-within:ring-2 focus-within:ring-[var(--btn-success)]
               focus-within:ring-offset-1
               focus-within:ring-offset-[var(--chrome)]"
      >
        <!-- Main segment: quick-create -->
        <button
          class="group flex items-center gap-1.5 px-2 sm:pl-2.5 sm:pr-2
                 text-body-sm font-medium text-[var(--btn-success-text)]
                 bg-[var(--btn-success)] hover:bg-[var(--btn-success-hover)]
                 transition-colors focus:outline-none
                 motion-safe:active:scale-[0.97]"
          onclick={(e) => {
            e.stopPropagation();
            view.newMenuOpen = false;
            onQuickCreate();
          }}
        >
          <Plus
            size={14}
            class="motion-safe:transition-transform
                   motion-safe:group-hover:rotate-90"
          />
          <span class="hidden sm:inline">New</span>
          <kbd
            class="hidden sm:grid ml-0.5 place-items-center min-w-[1.05rem] h-[1.05rem]
                   rounded bg-white/20 font-mono text-micro leading-none"
          >
            C
          </kbd>
        </button>
        <div class="w-px bg-white/25"></div>
        <!-- Caret segment: alternative create paths -->
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
          }}
        >
          <ChevronDown
            size={14}
            class="motion-safe:transition-transform {view.newMenuOpen
              ? 'rotate-180'
              : ''}"
          />
        </button>
      </div>

      {#if view.newMenuOpen}
        <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
        <div
          role="menu"
          tabindex="-1"
          class="absolute right-0 top-full mt-1.5 z-30 w-[208px]
                 bg-[var(--surface)] border border-[var(--border)]
                 rounded-lg shadow-lg py-1.5"
          onclick={(e) => e.stopPropagation()}
        >
          <button
            role="menuitem"
            class="w-full flex items-center gap-2.5 px-3 py-1.5 text-left
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
            class="w-full flex items-center gap-2.5 px-3 py-1.5 text-left
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

          <div class="my-1 h-px bg-[var(--border)]"></div>
          <div
            class="px-3 pb-1 pt-0.5 text-micro uppercase tracking-widest
                   font-semibold text-[var(--text-faint)]"
          >
            New in status
          </div>
          {#each ["backlog", "todo", "active"] as s}
            <button
              role="menuitem"
              class="w-full flex items-center gap-2.5 px-3 py-1.5 text-left
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
    {/if}
  </div>
</div>
