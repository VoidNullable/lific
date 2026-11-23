<script lang="ts">
  // The active filters, spelled out. The Filter button only carried a count,
  // so a narrowed list never said *what* was hiding the rest. One chip per
  // filtered dimension (plus the search query): the body reopens the filter
  // dialog, the × drops that dimension. Used under the toolbar and in the
  // filtered-empty state, so both say the same thing the same way.
  import type { Label, Module } from "../api";
  import { Search, X, Layers, User, Tag } from "lucide-svelte";
  import StatusIcon from "../StatusIcon.svelte";
  import PriorityIcon from "../PriorityIcon.svelte";
  import { safeLabelColor } from "../labelColors";
  import { capitalize, type IssueListState } from "./state.svelte";
  import {
    ASSIGNEE_FILTER_HUMAN,
    ASSIGNEE_FILTER_ME,
    ASSIGNEE_FILTER_NONE,
    filterUsername,
  } from "./assignees";

  let {
    view,
    labels,
    modules,
    size = "sm",
    includeSearch = true,
  }: {
    view: IssueListState;
    labels: Label[];
    modules: Module[];
    size?: "sm" | "md";
    /** Off under the toolbar, where the search field already shows it. */
    includeSearch?: boolean;
  } = $props();

  function assigneeName(v: string): string {
    if (v === ASSIGNEE_FILTER_ME) return "Me";
    if (v === ASSIGNEE_FILTER_HUMAN) return "Needs a person";
    if (v === ASSIGNEE_FILTER_NONE) return "Any agent";
    const u = filterUsername(v);
    return u ? `@${u}` : v;
  }

  function list(values: string[], fmt: (v: string) => string = capitalize): string {
    if (values.length <= 2) return values.map(fmt).join(" or ");
    return `${fmt(values[0])} +${values.length - 1}`;
  }

  function edit(e: MouseEvent) {
    e.stopPropagation();
    view.filterOpen = true;
  }

  let query = $derived(includeSearch ? view.searchQuery.trim() : "");
  let any = $derived(view.hasActiveFilters() || !!query);
  const h = $derived(size === "md" ? "h-8 text-body-sm" : "h-7 text-caption");
</script>

{#snippet chip(name: string, value: string, clear: () => void, icon: import("svelte").Snippet)}
  <span
    class="chip inline-flex items-stretch {h} rounded-full overflow-hidden
           bg-[var(--surface)] border border-[var(--border)]
           shadow-[0_1px_1px_rgba(0,0,0,0.04)]"
  >
    <button
      class="flex items-center gap-1.5 pl-2.5 pr-1.5 hover:bg-[var(--bg-subtle)] transition-colors"
      onclick={edit}
      title="Edit filters"
    >
      {@render icon()}
      <span class="text-[var(--text-faint)]">{name}</span>
      <span class="font-medium text-[var(--text)] max-w-[220px] truncate">{value}</span>
    </button>
    <button
      class="flex items-center px-1.5 text-[var(--text-faint)]
             hover:text-[var(--error)] hover:bg-[var(--bg-subtle)] transition-colors"
      aria-label="Remove {name.toLowerCase()} filter"
      onclick={(e) => { e.stopPropagation(); clear(); }}
    >
      <X size={13} />
    </button>
  </span>
{/snippet}

{#if any}
  <div class="flex flex-wrap items-center gap-1.5">
    {#if query}
      {#snippet searchIcon()}<Search size={12} class="text-[var(--text-faint)]" />{/snippet}
      {@render chip("Matches", `“${query}”`, () => (view.searchQuery = ""), searchIcon)}
    {/if}
    {#if view.filterStatus.length}
      {#snippet statusIcon()}
        <span class="flex -space-x-1">
          {#each view.filterStatus.slice(0, 3) as s (s)}<StatusIcon status={s} size={12} />{/each}
        </span>
      {/snippet}
      {@render chip("Status", list(view.filterStatus), () => (view.filterStatus = []), statusIcon)}
    {/if}
    {#if view.filterPriority.length}
      {#snippet prioIcon()}<PriorityIcon priority={view.filterPriority[0]} size={12} />{/snippet}
      {@render chip("Priority", list(view.filterPriority), () => (view.filterPriority = []), prioIcon)}
    {/if}
    {#if view.filterLabel.length}
      {#snippet labelIcon()}
        {@const first = labels.find((l) => l.name === view.filterLabel[0])}
        {#if first}
          <span class="size-2 rounded-full" style="background: {safeLabelColor(first.color)}"></span>
        {:else}
          <Tag size={12} class="text-[var(--text-faint)]" />
        {/if}
      {/snippet}
      {@render chip("Label", list(view.filterLabel, (v) => v), () => (view.filterLabel = []), labelIcon)}
    {/if}
    {#if view.filterModule.length}
      {#snippet modIcon()}<Layers size={12} class="text-[var(--text-faint)]" />{/snippet}
      {@render chip(
        "Module",
        list(view.filterModule, (v) => modules.find((m) => m.name === v)?.name ?? v),
        () => (view.filterModule = []),
        modIcon,
      )}
    {/if}
    {#if view.filterAssignee.length}
      {#snippet userIcon()}<User size={12} class="text-[var(--text-faint)]" />{/snippet}
      {@render chip("Assignee", list(view.filterAssignee, assigneeName), () => (view.filterAssignee = []), userIcon)}
    {/if}
    <button
      class="{h} px-2 rounded-full font-medium text-[var(--text-muted)]
             hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
      onclick={(e) => { e.stopPropagation(); view.clearFilters(); }}
    >
      Clear all
    </button>
  </div>
{/if}
