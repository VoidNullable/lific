<script module lang="ts">
  // Shape of the project-wide breakdown the parent computes from the
  // unfiltered issue set. Exported so the parent can type its derived.
  export interface SidebarStats {
    prio: Record<string, number>;
    byModule: Map<number, number>;
    noModule: number;
    total: number;
    active: number;
  }
</script>

<script lang="ts">
  // LIF-186 persistent right sidebar — project-wide issue context.
  // Extracted from IssueList.svelte (LIF-99). Always-on on lg+; mirrors the
  // Pages sidebar. Breakdowns come from the parent's unfiltered allIssues,
  // and every row is a one-click filter shortcut back into the parent's
  // filter state (via the toggle callbacks).
  import type { Module } from "../api";
  import { Layers, PanelRightClose, PanelRightOpen } from "lucide-svelte";
  import PriorityIcon from "../PriorityIcon.svelte";
  import ProjectIcon from "../ProjectIcon.svelte";
  import { PRIORITIES } from "./grouping";
  import {
    loadRightSidebarCollapsed,
    saveRightSidebarCollapsed,
  } from "./rightSidebarPrefs";

  let {
    stats,
    modules,
    filterPriority,
    filterModule,
    onTogglePriority,
    onToggleModule,
  }: {
    stats: SidebarStats;
    modules: Module[];
    filterPriority: string;
    filterModule: string;
    onTogglePriority: (priority: string) => void;
    onToggleModule: (name: string) => void;
  } = $props();

  // Collapse is entirely this module's concern: the parent's layout is a
  // plain flex row, so swapping the 244px aside for a slim edge tab lets the
  // content column reclaim the width with no parent changes. State persists
  // across visits (mirrors the left nav's LIF-360 behavior).
  let collapsed = $state(loadRightSidebarCollapsed());

  function toggleCollapsed() {
    collapsed = !collapsed;
    saveRightSidebarCollapsed(collapsed);
  }
</script>

{#if collapsed}
  <!-- Collapsed: the only way back, so it stays visible at the right edge.
       Mirrors the left nav's expand control (LIF-360) in styling; the
       rotated label names what returns, so the tab isn't a bare icon. -->
  <button
    class="hidden lg:flex flex-col items-center gap-2 w-7 shrink-0 py-3
           border-l border-[var(--border)] bg-[var(--bg-subtle)]
           text-[var(--text-faint)] hover:text-[var(--text)]
           hover:bg-[var(--surface)] transition-colors"
    onclick={toggleCollapsed}
    title="Expand issues sidebar"
    aria-label="Expand issues sidebar"
  >
    <PanelRightOpen size={15} />
    <span
      class="text-micro font-semibold uppercase tracking-widest
             [writing-mode:vertical-rl]"
    >
      Stats
    </span>
  </button>
{:else}
<aside
  class="hidden lg:flex flex-col w-[244px] shrink-0 overflow-y-auto
         border-l border-[var(--border)] bg-[var(--bg-subtle)] px-4 py-5"
>
  <!-- Collapse control, right-aligned above the summary so it sits nearest
       the screen edge it folds toward — mirroring the left nav's control
       placement beside the brand. -->
  <div class="flex items-center justify-end mb-3">
    <button
      class="size-6 grid place-items-center rounded-md
             text-[var(--text-faint)] hover:text-[var(--text)]
             hover:bg-[var(--surface)] transition-colors"
      onclick={toggleCollapsed}
      title="Collapse sidebar"
      aria-label="Collapse sidebar"
    >
      <PanelRightClose size={15} />
    </button>
  </div>

  <!-- Summary -->
  <div class="grid grid-cols-2 gap-3 mb-5">
    <div>
      <p class="text-title font-display tracking-tight tabular-nums text-[var(--text)] leading-none">
        {stats.total}
      </p>
      <p class="text-micro font-semibold uppercase tracking-widest text-[var(--text-faint)] mt-1">
        Issues
      </p>
    </div>
    <div>
      <p class="text-title font-display tracking-tight tabular-nums text-[var(--text)] leading-none">
        {stats.active}
      </p>
      <p class="text-micro font-semibold uppercase tracking-widest text-[var(--text-faint)] mt-1">
        Active
      </p>
    </div>
  </div>

  <!-- Priority breakdown — not surfaced anywhere else in the view; each row
       toggles the Priority filter. -->
  <p class="text-micro font-semibold uppercase tracking-widest text-[var(--text-faint)] mb-2 px-1">
    Priority
  </p>
  <div class="flex flex-col gap-0.5 mb-5">
    {#each PRIORITIES as p}
      {#if stats.prio[p] > 0}
        <button
          class="flex items-center gap-2 px-2 py-1.5 rounded-md text-left text-body-sm
                 transition-colors
                 {filterPriority === p
            ? 'bg-[var(--surface)] text-[var(--text)] shadow-[0_1px_2px_rgba(0,0,0,0.06)] font-medium'
            : 'text-[var(--text-muted)] hover:bg-[var(--surface)] hover:text-[var(--text)]'}"
          onclick={() => onTogglePriority(p)}
        >
          <PriorityIcon priority={p} size={14} />
          <span class="flex-1 capitalize">{p}</span>
          <span class="tabular-nums text-micro text-[var(--text-faint)]">
            {stats.prio[p]}
          </span>
        </button>
      {/if}
    {/each}
  </div>

  {#if modules.length > 0}
    <div class="h-px bg-[var(--border)] -mx-4 mb-4"></div>
    <!-- Module navigator — parallel to the Pages folder navigator; click to
         focus a module's issues. -->
    <p class="text-micro font-semibold uppercase tracking-widest text-[var(--text-faint)] mb-2 px-1">
      Modules
    </p>
    <div class="flex flex-col gap-0.5">
      {#each modules as m (m.id)}
        <button
          class="flex items-center gap-2 px-2 py-1.5 rounded-md text-left text-body-sm
                 transition-colors
                 {filterModule === m.name
            ? 'bg-[var(--surface)] text-[var(--text)] shadow-[0_1px_2px_rgba(0,0,0,0.06)] font-medium'
            : 'text-[var(--text-muted)] hover:bg-[var(--surface)] hover:text-[var(--text)]'}"
          onclick={() => onToggleModule(m.name)}
        >
          {#if m.emoji}
            <span class="shrink-0 text-[var(--text-faint)]"><ProjectIcon value={m.emoji} size={14} /></span>
          {:else}
            <Layers size={14} class="shrink-0 text-[var(--text-faint)]" />
          {/if}
          <span class="flex-1 truncate">{m.name}</span>
          <span class="tabular-nums text-micro text-[var(--text-faint)]">
            {stats.byModule.get(m.id) ?? 0}
          </span>
        </button>
      {/each}
    </div>
  {/if}
</aside>
{/if}
