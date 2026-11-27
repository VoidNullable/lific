<script lang="ts">
  // The Pages topbar, in the issue list's two tiers. The first holds search
  // and New page; the second says what the list shows (the folder pane's
  // pick, a drawer button below lg) and holds the tools that shape it
  // (Filter, Display). A third tier spells out the active filters as
  // removable chips.
  import {
    Archive,
    ChevronDown,
    Files,
    FolderOpen,
    HelpCircle,
    PanelRight,
    ScrollText,
    Plus,
    Search,
    SlidersHorizontal,
    SlidersVertical,
    X,
    Check,
  } from "lucide-svelte";
  import type { Label, PageMetaPerson } from "../api";
  import Tooltip from "../Tooltip.svelte";
  import SignInButton from "../SignInButton.svelte";
  import { isSignedOut } from "../signedOut";
  import { safeLabelColor } from "../labelColors";
  import { toggleShortcutHelp } from "../shortcutHelpState.svelte";
  import EditorMark from "./EditorMark.svelte";
  import {
    filterCount,
    LIVE_STATUSES,
    personLabel,
    STATUS_LABEL,
    type FolderScope,
    type PageGroup,
    type PageSort,
  } from "./list";
  import type { PageListView } from "./state.svelte";

  let {
    view,
    projectIdentifier,
    count,
    scope,
    labels,
    editors,
    changedCount = 0,
    onCycleChanged,
    canEdit,
    scopeLabel,
    onNewPage,
    onOpenFolders,
    onOpenPulse,
    searchInputEl = $bindable(),
  }: {
    view: PageListView;
    projectIdentifier: string;
    /** Pages the list shows with the current filters. */
    count: number | null;
    scope: FolderScope;
    labels: Label[];
    /** Accounts that last edited a page here, for the filter. */
    editors: PageMetaPerson[];
    changedCount?: number;
    onCycleChanged: () => void;
    canEdit: boolean;
    /** What the list shows: All pages, a folder's path, Archived. */
    scopeLabel: string;
    onNewPage: () => void;
    onOpenFolders: () => void;
    onOpenPulse: () => void;
    searchInputEl?: HTMLInputElement | null;
  } = $props();

  // Conventions is one page, not a list: nothing to filter or arrange.
  const shapeable = $derived(scope !== "conventions");
  const archived = $derived(scope === "archived");
  // Archived pages all share one status, so the status filter has no say.
  const nFilters = $derived(filterCount(archived ? { ...view.filters, statuses: [] } : view.filters));
  const SORT_LABEL: Record<PageSort, string> = {
    updated: "Last edited",
    created: "Created",
    title: "Title",
    status: "Status",
  };
  const GROUP_LABEL: Record<PageGroup, string> = {
    folder: "Folder",
    edited: "Day edited",
    label: "Label",
    status: "Status",
    none: "No grouping",
  };

  function editorLabel(v: string): string {
    if (v === "people") return "People";
    if (v === "agents") return "Agents";
    const id = Number(v.slice(2));
    const p = editors.find((e) => e.id === id);
    return p ? personLabel(p) : "Someone";
  }

  const toolBtn =
    "h-7 flex items-center gap-1.5 px-2 rounded-md text-caption font-medium transition-colors hover:bg-[var(--bg-subtle)]";
  const option = (on: boolean) =>
    `h-7 px-2.5 rounded-md text-caption font-medium border transition-colors ${
      on
        ? "border-[var(--accent)] bg-[var(--accent-subtle)] text-[var(--text)]"
        : "border-[var(--border)] text-[var(--text-muted)] hover:text-[var(--text)] hover:border-[var(--text-faint)]"
    }`;
</script>

{#snippet toggle(label: string, help: string, on: boolean, set: (on: boolean) => void)}
  <button
    type="button"
    role="switch"
    aria-checked={on}
    class="w-full flex items-center justify-between gap-3 text-left text-body-sm text-[var(--text)] rounded-md
           focus-visible:outline-none focus-visible:shadow-[0_0_0_3px_var(--accent-subtle)]"
    onclick={() => set(!on)}
  >
    <span>
      {label}
      {#if help}<span class="block text-caption text-[var(--text-muted)]">{help}</span>{/if}
    </span>
    <span class="relative inline-flex h-5 w-9 shrink-0 rounded-full transition-colors {on ? 'bg-[var(--accent)]' : 'bg-[var(--border)]'}">
      <span class="absolute top-0.5 left-0.5 size-4 rounded-full bg-[var(--surface)] shadow motion-safe:transition-transform {on ? 'translate-x-4' : 'translate-x-0'}"></span>
    </span>
  </button>
{/snippet}

<div class="relative w-full flex flex-col">
  <!-- ── Tier 1 ── -->
  <div class="relative flex items-center gap-2 md:gap-4 px-3 md:px-6 h-12 md:h-[3.25rem]">
    <div class="hidden md:flex items-baseline gap-2 min-w-0 shrink-0">
      <a
        href="#/{projectIdentifier}/overview"
        class="font-mono text-caption text-[var(--text-faint)] hover:text-[var(--text)] transition-colors"
      >
        {projectIdentifier}
      </a>
      <span class="text-[var(--text-faint)] opacity-60" aria-hidden="true">/</span>
      <h1 class="font-display text-heading font-medium tracking-tight text-[var(--text)] leading-none">Pages</h1>
      <span class="min-w-[2ch] text-caption tabular-nums text-[var(--text-faint)]" data-testid="pages-count">
        {count ?? ""}
      </span>
    </div>

    <h1 class="md:hidden font-display text-heading font-medium tracking-tight text-[var(--text)] leading-none">Pages</h1>

    <div class="flex-1"></div>

    {#if changedCount > 0}
      <Tooltip content={`${changedCount} updated since you last looked  ·  click to step through`} placement="bottom">
        <button
          class="h-7 flex items-center gap-1.5 px-2 md:px-2.5 rounded-full text-caption font-medium tabular-nums
                 text-[var(--accent)] bg-[var(--accent-subtle)]
                 hover:bg-[var(--accent)] hover:text-[var(--accent-text)] transition-colors"
          aria-label={`${changedCount} updated since you last looked. Focus the next one.`}
          data-testid="pages-changed-chip"
          onclick={(e) => { e.stopPropagation(); onCycleChanged(); }}
        >
          <span class="relative flex size-1.5 shrink-0" aria-hidden="true">
            <span class="changed-ping absolute inset-0 rounded-full bg-current"></span>
            <span class="relative size-1.5 rounded-full bg-current"></span>
          </span>
          <span>{changedCount > 99 ? "99+" : changedCount}</span>
          <span class="hidden md:inline">updated</span>
        </button>
      </Tooltip>
    {/if}

    {#if !view.searchOpen}
      <button
        aria-label="Search pages"
        class="md:hidden touch-target size-8 flex items-center justify-center rounded-md
               text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
        onclick={(e) => {
          e.stopPropagation();
          view.searchOpen = true;
          requestAnimationFrame(() => searchInputEl?.focus());
        }}
      >
        <Search size={15} />
      </button>
    {/if}
    <div
      class="{view.searchOpen ? 'max-md:flex' : 'max-md:hidden'}
             max-md:absolute max-md:inset-0 max-md:z-20 max-md:bg-[var(--chrome)] max-md:items-center max-md:px-3"
    >
      <div class="relative w-full md:w-auto group/search">
        <div class="absolute left-2.5 top-1/2 -translate-y-1/2 pointer-events-none text-[var(--text-faint)]
                    group-focus-within/search:text-[var(--accent)] transition-colors">
          <Search size={13} />
        </div>
        <input
          type="text"
          placeholder="Search pages..."
          enterkeyhint="search"
          aria-label="Search pages"
          bind:this={searchInputEl}
          bind:value={view.search}
          onblur={() => { if (!view.search) view.searchOpen = false; }}
          onkeydown={(e) => {
            if (e.key === "Escape") {
              e.preventDefault();
              view.search = "";
              view.searchOpen = false;
              (e.currentTarget as HTMLInputElement).blur();
            }
            if (e.key === "ArrowDown" || e.key === "Enter") {
              e.preventDefault();
              (e.currentTarget as HTMLInputElement).blur();
              window.dispatchEvent(new CustomEvent("lific:pages-search-done", { detail: e.key }));
            }
          }}
          class="peer w-full h-8 md:w-[220px] md:focus:w-[300px] pl-8 pr-8 text-body-sm rounded-lg
                 border border-[var(--border)] bg-[var(--bg)] text-[var(--text)] placeholder:text-[var(--text-faint)]
                 hover:border-[var(--text-faint)] focus:border-[var(--accent)] focus:bg-[var(--surface)]
                 focus:shadow-[0_0_0_3px_var(--accent-subtle)] outline-none
                 transition-[width,border-color,box-shadow,background] duration-200
                 {view.search ? 'md:w-[300px]' : ''}"
        />
        {#if view.search}
          <button
            class="absolute right-1.5 top-1/2 -translate-y-1/2 size-5 grid place-items-center rounded
                   text-[var(--text-faint)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]"
            aria-label="Clear search"
            onmousedown={(e) => e.preventDefault()}
            onclick={(e) => { e.stopPropagation(); view.search = ""; searchInputEl?.focus(); }}
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

    <div class="relative hidden md:block">
      <Tooltip content="Shortcuts  ·  ?" placement="bottom">
        <button
          class="size-8 flex items-center justify-center rounded-md text-[var(--text-muted)]
                 hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
          aria-label="Keyboard shortcuts"
          onclick={(e) => { e.stopPropagation(); toggleShortcutHelp(); }}
        >
          <HelpCircle size={15} />
        </button>
      </Tooltip>
    </div>

    {#if canEdit}
      <button
        class="group h-8 flex items-center gap-1.5 px-2.5 md:pl-3 md:pr-2.5 rounded-lg text-body-sm font-medium
               text-[var(--btn-success-text)] bg-[var(--btn-success)] hover:bg-[var(--btn-success-hover)]
               shadow-[0_1px_2px_rgba(0,0,0,0.18),inset_0_1px_0_rgba(255,255,255,0.18)] transition-colors
               focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--btn-success)] focus-visible:ring-offset-1
               focus-visible:ring-offset-[var(--chrome)] motion-safe:active:scale-[0.97]"
        aria-label="New page"
        onclick={(e) => { e.stopPropagation(); view.closePopovers(); onNewPage(); }}
      >
        <Plus size={15} class="motion-safe:transition-transform motion-safe:duration-300 motion-safe:group-hover:rotate-90" />
        <span class="hidden md:inline">New page</span>
        <kbd class="hidden md:grid ml-0.5 place-items-center min-w-[1.1rem] h-[1.1rem] rounded bg-black/10 font-mono text-micro leading-none">C</kbd>
      </button>
    {:else if isSignedOut()}
      <SignInButton />
    {/if}
  </div>

  <!-- ── Tier 2 ── -->
  <div class="flex items-stretch gap-3 px-3 md:px-6 h-11 border-b border-[var(--border)]">
    <!-- What the list shows. Below lg the folder pane is a drawer and this
         opens it. -->
    {#snippet scopeIcon()}
      {#if scope === "conventions"}
        <ScrollText size={15} class="shrink-0 text-[var(--text-muted)]" />
      {:else if scope === "archived"}
        <Archive size={15} class="shrink-0 text-[var(--text-muted)]" />
      {:else if typeof scope === "number"}
        <FolderOpen size={15} class="shrink-0 text-[var(--accent)]" />
      {:else}
        <Files size={15} class="shrink-0 text-[var(--text-muted)]" />
      {/if}
    {/snippet}
    <div class="min-w-0 flex-1 flex items-center" data-testid="pages-scope">
      <button
        class="lg:hidden min-w-0 h-8 -ml-2 flex items-center gap-1.5 px-2 rounded-md
               text-body-sm font-medium text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
        aria-label="Folders: {scopeLabel}"
        onclick={(e) => { e.stopPropagation(); onOpenFolders(); }}
      >
        {@render scopeIcon()}
        <span class="truncate">{scopeLabel}</span>
        <ChevronDown size={13} class="shrink-0 text-[var(--text-faint)]" />
      </button>
      <h2 class="max-lg:hidden min-w-0 flex items-center gap-2 text-body-sm font-medium text-[var(--text)]">
        {@render scopeIcon()}
        <span class="truncate">{scopeLabel}</span>
      </h2>
    </div>

    {#if shapeable}
      <div class="shrink-0 flex items-center gap-1">
        <div class="relative">
          <Tooltip content={view.filterOpen ? null : "Filter  ·  F"} placement="bottom">
            <button
              class="{toolBtn} {view.filterOpen || nFilters > 0
                ? 'text-[var(--text)] bg-[var(--bg-subtle)]'
                : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
              aria-haspopup="dialog"
              aria-expanded={view.filterOpen}
              onclick={(e) => {
                e.stopPropagation();
                const open = !view.filterOpen;
                view.closePopovers();
                view.filterOpen = open;
              }}
            >
              <SlidersVertical size={13} class="shrink-0" />
              <span class="sr-only sm:not-sr-only">Filter</span>
              {#if nFilters > 0}
                <span class="grid place-items-center min-w-[1.05rem] h-[1.05rem] px-1 rounded-full bg-[var(--accent)]
                             text-[var(--accent-text)] font-mono text-micro leading-none tabular-nums">{nFilters}</span>
              {/if}
            </button>
          </Tooltip>
          {#if view.filterOpen}
            <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
            <div
              role="dialog"
              aria-label="Filter pages"
              tabindex="-1"
              class="popover-in origin-top-right absolute right-0 top-full mt-1.5 z-30 w-[min(320px,calc(100vw-1.5rem))]
                     bg-[var(--surface)] border border-[var(--border)] rounded-xl shadow-lg p-3 flex flex-col gap-4"
              onclick={(e) => e.stopPropagation()}
              data-testid="pages-filter"
            >
              {#if !archived}
                <section>
                  <h3 class="pop-label">Status</h3>
                  <div class="mt-1.5 flex flex-wrap gap-1.5">
                    {#each LIVE_STATUSES as s (s)}
                      <button class={option(view.filters.statuses.includes(s))} aria-pressed={view.filters.statuses.includes(s)} onclick={() => view.toggleStatus(s)}>
                        {STATUS_LABEL[s]}
                      </button>
                    {/each}
                  </div>
                </section>
              {/if}
              {#if labels.length > 0}
                <section>
                  <h3 class="pop-label">Label</h3>
                  <div class="mt-1.5 flex flex-wrap gap-1.5 max-h-28 overflow-y-auto">
                    {#each labels as l (l.id)}
                      {@const on = view.filters.label === l.name}
                      <button
                        class="{option(on)} flex items-center gap-1.5"
                        aria-pressed={on}
                        onclick={() => (view.filters = { ...view.filters, label: on ? "" : l.name })}
                      >
                        <span class="size-2 rounded-full" style="background: {safeLabelColor(l.color)}"></span>{l.name}
                      </button>
                    {/each}
                  </div>
                </section>
              {/if}
              <section>
                <h3 class="pop-label">Last edited by</h3>
                <div class="mt-1.5 flex flex-wrap gap-1.5">
                  {#each [["", "Anyone"], ["people", "People"], ["agents", "Agents"]] as [v, label] (v)}
                    <button class={option(view.filters.editor === v)} aria-pressed={view.filters.editor === v} onclick={() => (view.filters = { ...view.filters, editor: v as "" })}>
                      {label}
                    </button>
                  {/each}
                </div>
                {#if editors.length > 0}
                  <div class="mt-1.5 flex flex-col max-h-36 overflow-y-auto -mx-1">
                    {#each editors as p (p.id)}
                      {@const v = `u:${p.id}` as const}
                      {@const on = view.filters.editor === v}
                      <button
                        class="flex items-center gap-2 h-8 px-1 rounded-md text-left text-body-sm hover:bg-[var(--bg-subtle)]
                               {on ? 'text-[var(--text)] font-medium' : 'text-[var(--text-muted)]'}"
                        aria-pressed={on}
                        onclick={() => (view.filters = { ...view.filters, editor: on ? "" : v })}
                      >
                        <EditorMark person={p} />
                        <span class="flex-1 truncate">{personLabel(p)}</span>
                        {#if on}<Check size={13} class="text-[var(--accent)]" />{/if}
                      </button>
                    {/each}
                  </div>
                {/if}
              </section>
              {@render toggle("Only pages nothing links to", "", view.filters.unlinked, (on) => (view.filters = { ...view.filters, unlinked: on }))}
              {#if nFilters > 0}
                <button class="self-start text-caption text-[var(--accent)] hover:underline" onclick={() => view.clearFilters()}>
                  Clear filters
                </button>
              {/if}
            </div>
          {/if}
        </div>

        <div class="relative">
          <Tooltip content={view.displayOpen ? null : "Display"} placement="bottom">
            <button
              class="{toolBtn} {view.displayOpen ? 'text-[var(--text)] bg-[var(--bg-subtle)]' : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
              aria-haspopup="dialog"
              aria-expanded={view.displayOpen}
              onclick={(e) => {
                e.stopPropagation();
                const open = !view.displayOpen;
                view.closePopovers();
                view.displayOpen = open;
              }}
            >
              <SlidersHorizontal size={13} class="shrink-0" />
              <span class="sr-only sm:not-sr-only">Display</span>
            </button>
          </Tooltip>
          {#if view.displayOpen}
            <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
            <div
              role="dialog"
              aria-label="Display options"
              tabindex="-1"
              class="popover-in origin-top-right absolute right-0 top-full mt-1.5 z-30 w-[min(300px,calc(100vw-1.5rem))]
                     bg-[var(--surface)] border border-[var(--border)] rounded-xl shadow-lg p-3 flex flex-col gap-4"
              onclick={(e) => e.stopPropagation()}
              data-testid="pages-display"
            >
              <section>
                <h3 class="pop-label">Group by</h3>
                <div class="mt-1.5 flex flex-wrap gap-1.5">
                  {#each ["folder", "edited", "label", "status", "none"] as const as g (g)}
                    <button class={option(view.group === g)} aria-pressed={view.group === g} onclick={() => (view.group = g)}>
                      {GROUP_LABEL[g]}
                    </button>
                  {/each}
                </div>
              </section>
              {#if view.group !== "edited"}
                <section>
                  <h3 class="pop-label">Sort by</h3>
                  <div class="mt-1.5 flex flex-wrap gap-1.5">
                    {#each ["updated", "created", "title", "status"] as const as s (s)}
                      <button class={option(view.sort === s)} aria-pressed={view.sort === s} onclick={() => (view.sort = s)}>
                        {SORT_LABEL[s]}
                      </button>
                    {/each}
                  </div>
                </section>
              {/if}
              {@render toggle("Show excerpts", "", view.excerpts, (on) => (view.excerpts = on))}
              <div class="hidden xl:block">
                {@render toggle("Reading pane", "Clicking a page shows it beside the list.", view.pane, (on) => (view.pane = on))}
              </div>
            </div>
          {/if}
        </div>

        <Tooltip content="Activity" placement="bottom">
          <button
            class="max-sm:hidden xl:hidden size-7 grid place-items-center rounded-md text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]"
            aria-label="Show recent activity"
            onclick={(e) => { e.stopPropagation(); onOpenPulse(); }}
          >
            <PanelRight size={14} />
          </button>
        </Tooltip>
      </div>
    {/if}
  </div>

  {#if shapeable && nFilters > 0}
    <div class="flex flex-wrap items-center gap-1.5 px-3 md:px-6 py-2 border-b border-[var(--border)]" data-testid="pages-filter-chips">
      {#snippet chip(text: string, clear: () => void, dot: string | null = null)}
        <span class="inline-flex items-center h-6 rounded-full border border-[var(--border)] bg-[var(--surface)] text-caption text-[var(--text)]">
          <button class="flex items-center gap-1.5 pl-2.5 pr-1.5 h-full" onclick={(e) => { e.stopPropagation(); view.closePopovers(); view.filterOpen = true; }}>
            {#if dot}<span class="size-2 rounded-full" style="background: {dot}"></span>{/if}
            {text}
          </button>
          <button class="h-full pr-2 pl-0.5 text-[var(--text-faint)] hover:text-[var(--text)]" aria-label="Remove filter: {text}" onclick={clear}>
            <X size={11} />
          </button>
        </span>
      {/snippet}
      {#if view.filters.statuses.length > 0 && !archived}
        {@render chip(view.filters.statuses.map((s) => STATUS_LABEL[s]).join(" or "), () => (view.filters = { ...view.filters, statuses: [] }))}
      {/if}
      {#if view.filters.label}
        {@const l = labels.find((x) => x.name === view.filters.label)}
        {@render chip(view.filters.label, () => (view.filters = { ...view.filters, label: "" }), l ? safeLabelColor(l.color) : null)}
      {/if}
      {#if view.filters.editor}
        {@render chip(`Edited by ${editorLabel(view.filters.editor)}`, () => (view.filters = { ...view.filters, editor: "" }))}
      {/if}
      {#if view.filters.unlinked}
        {@render chip("Nothing links here", () => (view.filters = { ...view.filters, unlinked: false }))}
      {/if}
      <button class="ml-1 text-caption text-[var(--text-muted)] hover:text-[var(--text)]" onclick={() => view.clearFilters()}>
        Clear
      </button>
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
  .pop-label {
    font-size: var(--text-micro);
    text-transform: uppercase;
    letter-spacing: 0.08em;
    font-weight: 600;
    color: var(--text-faint);
  }
</style>
