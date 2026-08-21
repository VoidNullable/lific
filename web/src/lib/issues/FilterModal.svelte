<script lang="ts">
  // Full filter modal (LIF-222 follow-up). Replaces the cramped 280px filter
  // popover with a roomy centered dialog so each status / priority can carry a
  // one-line description, making the vocabulary self-documenting. Chrome and
  // interaction model mirror IssuePickerModal / CommandPalette (dimmed
  // backdrop, centered card, Esc to close, click-outside to dismiss) so the
  // modals feel like one family.
  //
  // Reads + mutates the shared IssueListState (`view`) directly, same as the
  // old inline popover did; data-derived inputs (labels, modules) come in as
  // props.
  import { X, Check, Layers, CircleDotDashed } from "lucide-svelte";
  import StatusIcon from "../StatusIcon.svelte";
  import PriorityIcon from "../PriorityIcon.svelte";
  import {
    STATUSES,
    PRIORITIES,
    STATUS_UNRESOLVED,
    STATUS_DESCRIPTIONS,
    UNRESOLVED_DESCRIPTION,
    PRIORITY_DESCRIPTIONS,
  } from "./grouping";
  import type { IssueListState } from "./state.svelte";
  import type { Label, Module } from "../api";
  import { safeLabelColor } from "../labelColors";

  let {
    view,
    labels,
    modules,
    priorityCssColor,
  }: {
    view: IssueListState;
    labels: Label[];
    modules: Module[];
    priorityCssColor: (p: string) => string;
  } = $props();

  let filterCount = $derived(view.activeFilterCount());

  function close() {
    view.filterOpen = false;
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      close();
    }
  }
</script>

{#if view.filterOpen}
  <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
  <div
    class="fixed inset-0 z-[100] bg-black/25 flex items-end justify-center
           sm:items-start sm:pt-[9dvh] sm:px-4"
    onclick={close}
    onkeydown={onKeydown}
  >
    <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
    <!-- LIF-227: a bottom sheet on phones. Unlike the search overlays this
         one has no text input to be buried by the keyboard, and its content
         is a stack of toggles — so anchoring it to the bottom puts every
         control inside thumb reach instead of stranding them at the top of
         a 6-inch screen. -->
    <div
      class="w-full max-h-[85dvh] flex flex-col
             bg-[var(--surface)] border-t border-[var(--border)]
             rounded-t-2xl shadow-[0_-8px_48px_rgba(0,0,0,0.28)] overflow-hidden
             sm:max-w-[640px] sm:max-h-[82dvh] sm:border sm:rounded-xl
             sm:shadow-[0_16px_48px_rgba(0,0,0,0.28)]"
      onclick={(e) => e.stopPropagation()}
    >
      <!-- Grab-handle affordance (phones only; the scrim is the close
           gesture, this just signals "sheet"). Mirrors PeekPanel. -->
      <div class="sm:hidden flex justify-center pt-2 pb-0.5 shrink-0">
        <div class="h-1 w-9 rounded-full bg-[var(--border)]"></div>
      </div>

      <!-- Header -->
      <div
        class="shrink-0 flex items-center gap-3 px-5 py-3.5 border-b border-[var(--border)]"
      >
        <h2 class="text-body-lg font-semibold text-[var(--text)]">Filters</h2>
        {#if filterCount > 0}
          <span
            class="grid place-items-center min-w-[1.05rem] h-[1.05rem] px-1
                   rounded-full bg-[var(--accent)] text-[var(--accent-text)]
                   font-mono text-micro leading-none tabular-nums"
          >
            {filterCount}
          </span>
          <button
            class="text-caption text-[var(--text-muted)] hover:text-[var(--text)]
                   transition-colors"
            onclick={() => view.clearFilters()}
          >
            Clear all
          </button>
        {/if}
        <button
          class="ml-auto size-11 sm:size-7 grid place-items-center rounded-md
                 text-[var(--text-muted)] hover:text-[var(--text)]
                 hover:bg-[var(--bg-subtle)] transition-colors"
          aria-label="Close filters"
          onclick={close}
        >
          <X size={15} />
        </button>
      </div>

      <!-- Body -->
      <div
        class="flex-1 min-h-0 overflow-y-auto overscroll-contain
               px-5 py-4 grid grid-cols-1 sm:grid-cols-2 gap-x-6 gap-y-5"
      >

        <!-- STATUS -->
        <section>
          <div class="px-1 pb-1.5 text-micro uppercase tracking-widest font-semibold text-[var(--text-faint)]">
            Status
          </div>
          <div class="flex flex-col gap-0.5">
            <!-- Any -->
            <button
              class="w-full flex items-start gap-2.5 px-2.5 py-2 rounded-md text-left transition-colors
                     {!view.filterStatus
                ? 'bg-[var(--accent-subtle)]'
                : 'hover:bg-[var(--bg-subtle)]'}"
              onclick={() => (view.filterStatus = "")}
            >
              <span class="size-4 mt-0.5 shrink-0"></span>
              <span class="flex-1 min-w-0">
                <span class="block text-body-sm font-medium text-[var(--text)]">Any</span>
                <span class="block text-caption text-[var(--text-faint)]">No status filter.</span>
              </span>
              {#if !view.filterStatus}
                <Check size={14} class="mt-0.5 shrink-0 text-[var(--accent)]" />
              {/if}
            </button>

            <!-- Unresolved (status group) -->
            <button
              class="w-full flex items-start gap-2.5 px-2.5 py-2 rounded-md text-left transition-colors
                     {view.filterStatus === STATUS_UNRESOLVED
                ? 'bg-[var(--accent-subtle)]'
                : 'hover:bg-[var(--bg-subtle)]'}"
              onclick={() => view.toggleStatusFilter(STATUS_UNRESOLVED)}
            >
              <CircleDotDashed size={16} class="mt-0.5 shrink-0 text-[var(--accent)]" />
              <span class="flex-1 min-w-0">
                <span class="block text-body-sm font-medium text-[var(--text)]">Unresolved</span>
                <span class="block text-caption text-[var(--text-faint)]">{UNRESOLVED_DESCRIPTION}</span>
              </span>
              {#if view.filterStatus === STATUS_UNRESOLVED}
                <Check size={14} class="mt-0.5 shrink-0 text-[var(--accent)]" />
              {/if}
            </button>

            {#each STATUSES as s (s)}
              {@const active = view.filterStatus === s}
              <button
                class="w-full flex items-start gap-2.5 px-2.5 py-2 rounded-md text-left transition-colors
                       {active ? 'bg-[var(--accent-subtle)]' : 'hover:bg-[var(--bg-subtle)]'}"
                onclick={() => view.toggleStatusFilter(s)}
              >
                <span class="mt-0.5 shrink-0"><StatusIcon status={s} size={16} /></span>
                <span class="flex-1 min-w-0">
                  <span class="block text-body-sm font-medium capitalize text-[var(--text)]">{s}</span>
                  <span class="block text-caption text-[var(--text-faint)]">{STATUS_DESCRIPTIONS[s] ?? ""}</span>
                </span>
                {#if active}
                  <Check size={14} class="mt-0.5 shrink-0 text-[var(--accent)]" />
                {/if}
              </button>
            {/each}
          </div>
        </section>

        <!-- PRIORITY -->
        <section>
          <div class="px-1 pb-1.5 text-micro uppercase tracking-widest font-semibold text-[var(--text-faint)]">
            Priority
          </div>
          <div class="flex flex-col gap-0.5">
            <button
              class="w-full flex items-start gap-2.5 px-2.5 py-2 rounded-md text-left transition-colors
                     {!view.filterPriority
                ? 'bg-[var(--accent-subtle)]'
                : 'hover:bg-[var(--bg-subtle)]'}"
              onclick={() => (view.filterPriority = "")}
            >
              <span class="size-4 mt-0.5 shrink-0"></span>
              <span class="flex-1 min-w-0">
                <span class="block text-body-sm font-medium text-[var(--text)]">Any</span>
                <span class="block text-caption text-[var(--text-faint)]">No priority filter.</span>
              </span>
              {#if !view.filterPriority}
                <Check size={14} class="mt-0.5 shrink-0 text-[var(--accent)]" />
              {/if}
            </button>

            {#each PRIORITIES as p (p)}
              {@const active = view.filterPriority === p}
              <button
                class="w-full flex items-start gap-2.5 px-2.5 py-2 rounded-md text-left transition-colors
                       {active ? 'bg-[var(--accent-subtle)]' : 'hover:bg-[var(--bg-subtle)]'}"
                onclick={() => view.togglePriorityFilter(p)}
              >
                <span class="mt-0.5 shrink-0"><PriorityIcon priority={p} size={16} /></span>
                <span class="flex-1 min-w-0">
                  <span
                    class="block text-body-sm font-medium capitalize"
                    style="color: {priorityCssColor(p)}"
                  >{p}</span>
                  <span class="block text-caption text-[var(--text-faint)]">{PRIORITY_DESCRIPTIONS[p] ?? ""}</span>
                </span>
                {#if active}
                  <Check size={14} class="mt-0.5 shrink-0 text-[var(--accent)]" />
                {/if}
              </button>
            {/each}
          </div>
        </section>

        <!-- LABEL -->
        {#if labels.length > 0}
          <section>
            <div class="px-1 pb-1.5 text-micro uppercase tracking-widest font-semibold text-[var(--text-faint)]">
              Label
            </div>
            <div class="flex flex-col gap-0.5">
              <button
                class="w-full flex items-center gap-2.5 px-2.5 py-2 rounded-md text-left transition-colors
                       {!view.filterLabel ? 'bg-[var(--accent-subtle)]' : 'hover:bg-[var(--bg-subtle)]'}"
                onclick={() => (view.filterLabel = "")}
              >
                <span class="size-2.5 shrink-0"></span>
                <span class="flex-1 text-body-sm font-medium text-[var(--text)]">Any</span>
                {#if !view.filterLabel}<Check size={14} class="shrink-0 text-[var(--accent)]" />{/if}
              </button>
              {#each labels as l (l.id)}
                {@const active = view.filterLabel === l.name}
                <button
                  class="w-full flex items-center gap-2.5 px-2.5 py-2 rounded-md text-left transition-colors
                         {active ? 'bg-[var(--accent-subtle)]' : 'hover:bg-[var(--bg-subtle)]'}"
                  onclick={() => view.toggleLabelFilter(l.name)}
                >
                  <span class="size-2.5 rounded-full shrink-0" style="background: {safeLabelColor(l.color)}"></span>
                  <span class="flex-1 min-w-0 truncate text-body-sm font-medium text-[var(--text)]">{l.name}</span>
                  {#if active}<Check size={14} class="shrink-0 text-[var(--accent)]" />{/if}
                </button>
              {/each}
            </div>
          </section>
        {/if}

        <!-- MODULE -->
        {#if modules.length > 0}
          <section>
            <div class="px-1 pb-1.5 text-micro uppercase tracking-widest font-semibold text-[var(--text-faint)]">
              Module
            </div>
            <div class="flex flex-col gap-0.5">
              <button
                class="w-full flex items-center gap-2.5 px-2.5 py-2 rounded-md text-left transition-colors
                       {!view.filterModule ? 'bg-[var(--accent-subtle)]' : 'hover:bg-[var(--bg-subtle)]'}"
                onclick={() => (view.filterModule = "")}
              >
                <span class="size-4 shrink-0"></span>
                <span class="flex-1 text-body-sm font-medium text-[var(--text)]">Any</span>
                {#if !view.filterModule}<Check size={14} class="shrink-0 text-[var(--accent)]" />{/if}
              </button>
              {#each modules as m (m.id)}
                {@const active = view.filterModule === m.name}
                <button
                  class="w-full flex items-start gap-2.5 px-2.5 py-2 rounded-md text-left transition-colors
                         {active ? 'bg-[var(--accent-subtle)]' : 'hover:bg-[var(--bg-subtle)]'}"
                  onclick={() => view.toggleModuleFilter(m.name)}
                >
                  <Layers size={15} class="mt-0.5 shrink-0 text-[var(--text-muted)]" />
                  <span class="flex-1 min-w-0">
                    <span class="block text-body-sm font-medium text-[var(--text)] truncate">{m.name}</span>
                    {#if m.description}
                      <span class="block text-caption text-[var(--text-faint)] truncate">{m.description}</span>
                    {/if}
                  </span>
                  {#if active}<Check size={14} class="mt-0.5 shrink-0 text-[var(--accent)]" />{/if}
                </button>
              {/each}
            </div>
          </section>
        {/if}
      </div>

      <!-- Footer -->
      <div
        class="shrink-0 flex items-center gap-3 px-5 py-2.5 border-t border-[var(--border)]
               text-micro text-[var(--text-faint)]
               pb-[max(0.625rem,env(safe-area-inset-bottom))] sm:pb-2.5"
      >
        <span class="inline-flex items-center gap-1">
          <kbd class="font-mono">esc</kbd> close
        </span>
        <span class="ml-auto">
          {filterCount > 0 ? `${filterCount} active` : "No filters applied"}
        </span>
      </div>
    </div>
  </div>
{/if}
