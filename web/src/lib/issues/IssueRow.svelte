<script lang="ts">
  // List-view issue row. Extracted from IssueList.svelte (LIF-99) — the
  // single biggest chunk of the monolith.
  //
  // The row is deliberately "dumb": it reads the current selection / focus /
  // open-dropdown state as plain props and emits every mutation as a
  // callback. The parent owns the shared state machine (selection set,
  // focused index, which dropdown is open, optimistic issue updates). A
  // later refactor (LIF-99 Phase 3b) collapses these props into a shared
  // state instance; keeping them explicit here first makes the seam
  // reviewable and build-verifiable.
  import type { Issue, Label, Module } from "../api";
  import { Check, Signal, Layers } from "lucide-svelte";
  import StatusIcon from "../StatusIcon.svelte";
  import PriorityIcon from "../PriorityIcon.svelte";
  import ProjectIcon from "../ProjectIcon.svelte";
  import Tooltip from "../Tooltip.svelte";
  import { formatRelative } from "../format";
  import { STATUSES, PRIORITIES, descriptionPreview } from "./grouping";

  let {
    issue,
    idx,
    labels,
    modules,
    density,
    groupBy,
    isFocused,
    isSelected,
    selectionActive,
    hitSnippet,
    statusOpen,
    priorityOpen,
    statusPickerIdx,
    onOpen,
    onRangeSelect,
    onToggleSelect,
    onMouseEnterRow,
    onToggleStatusDropdown,
    onTogglePriorityDropdown,
    onPickStatus,
    onPickPriority,
    onHoverStatusOption,
  }: {
    issue: Issue;
    idx: number;
    labels: Label[];
    modules: Module[];
    density: "compact" | "comfortable";
    groupBy: "status" | "priority" | "module" | "none";
    /** Keyboard-focused row. */
    isFocused: boolean;
    /** Row is in the multi-select set. */
    isSelected: boolean;
    /** Any selection exists (keeps checkboxes visible across all rows). */
    selectionActive: boolean;
    /** Search snippet to show under the title, if this row matched on body. */
    hitSnippet: string | null;
    /** This row's inline status dropdown is open. */
    statusOpen: boolean;
    /** This row's inline priority dropdown is open. */
    priorityOpen: boolean;
    /** Highlighted index within the open status picker. */
    statusPickerIdx: number;
    onOpen: (issue: Issue) => void;
    onRangeSelect: (idx: number) => void;
    onToggleSelect: (id: number, idx: number) => void;
    onMouseEnterRow: (e: MouseEvent, idx: number) => void;
    onToggleStatusDropdown: (issue: Issue) => void;
    onTogglePriorityDropdown: (issue: Issue) => void;
    onPickStatus: (issue: Issue, status: string) => void;
    onPickPriority: (issue: Issue, priority: string) => void;
    onHoverStatusOption: (si: number) => void;
  } = $props();

  const mod = $derived(
    issue.module_id == null
      ? undefined
      : modules.find((m) => m.id === issue.module_id),
  );
</script>

<div
  class="w-full flex items-center gap-3 px-6 text-left
         {density === 'comfortable' ? 'py-3' : 'py-2.5'}
         border-b border-[var(--border)] last:border-b-0
         border-l-2 transition-colors group cursor-pointer
         {isFocused ? 'border-l-[var(--accent)]' : 'border-l-transparent'}
         {isSelected || isFocused
    ? 'bg-[var(--accent-subtle)]'
    : 'hover:bg-[var(--bg-subtle)]'}"
  data-issue-index={idx}
  role="button"
  tabindex="-1"
  onclick={(e) => {
    // LIF-149: shift-click extends a range, ctrl/cmd-click toggles —
    // plain click still opens the issue.
    if (e.shiftKey) {
      e.preventDefault();
      onRangeSelect(idx);
      return;
    }
    if (e.ctrlKey || e.metaKey) {
      e.preventDefault();
      onToggleSelect(issue.id, idx);
      return;
    }
    onOpen(issue);
  }}
  onmousedown={(e) => {
    // Shift-click means "extend selection" — suppress the native
    // text-selection sweep it would otherwise trigger.
    if (e.shiftKey) e.preventDefault();
  }}
  onmouseenter={(e) => onMouseEnterRow(e, idx)}
>
  <!-- Selection checkbox (LIF-149). Space is always reserved so rows never
       shift; the box is invisible until hover or until a selection exists
       anywhere, then stays visible for the session of that selection. -->
  <button
    class="size-4 shrink-0 rounded border flex items-center justify-center
           transition-all
           {isSelected
      ? 'bg-[var(--accent)] border-[var(--accent)] text-[var(--accent-text)]'
      : 'border-[var(--border)] text-transparent hover:border-[var(--text-faint)]'}
           {isSelected || selectionActive
      ? 'opacity-100'
      : 'opacity-0 group-hover:opacity-100'}"
    title={isSelected ? "Deselect" : "Select  ·  X"}
    onclick={(e) => {
      e.stopPropagation();
      if (e.shiftKey) onRangeSelect(idx);
      else onToggleSelect(issue.id, idx);
    }}
  >
    <Check size={11} strokeWidth={3} />
  </button>

  <!-- Status indicator (clickable to pick). Tooltip suppressed while the
       status picker is open for this row, otherwise it'd hover-fight with
       the dropdown. -->
  <div class="relative shrink-0">
    <Tooltip
      content={statusOpen
        ? null
        : issue.status[0].toUpperCase() + issue.status.slice(1)}
    >
      <button
        class="size-4 flex items-center justify-center transition-colors
               hover:text-[var(--accent)]"
        onclick={(e) => {
          e.stopPropagation();
          onToggleStatusDropdown(issue);
        }}
      >
        <StatusIcon status={issue.status} size={16} />
      </button>
    </Tooltip>
    {#if statusOpen}
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
                   {si === statusPickerIdx
              ? 'text-[var(--accent)] bg-[var(--accent-subtle)] font-medium'
              : 'text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
            onclick={() => onPickStatus(issue, s)}
            onmouseenter={() => onHoverStatusOption(si)}
          >
            <StatusIcon status={s} size={14} />
            {s}
          </button>
        {/each}
      </div>
    {/if}
  </div>

  <!-- Identifier -->
  <span class="text-body-sm text-[var(--text-faint)] font-mono shrink-0 w-[72px]">
    {issue.identifier}
  </span>

  <!-- Title (and, in search mode, an optional content snippet below it when
       the description was the reason this issue surfaced). The column flexes
       vertically to stack the two lines while the outer row stays
       items-center, so icons remain aligned to the title column as a whole. -->
  <div class="flex-1 min-w-0 flex flex-col gap-0.5">
    <span
      class="text-body text-[var(--text)] truncate
             {issue.status === 'done' || issue.status === 'cancelled'
        ? 'line-through text-[var(--text-muted)]'
        : ''}"
    >
      {issue.title}
    </span>
    {#if hitSnippet}
      <span class="text-caption text-[var(--text-muted)] truncate">
        {hitSnippet}
      </span>
    {:else if density === "comfortable"}
      {@const prev = descriptionPreview(issue.description)}
      {#if prev}
        <span class="text-caption text-[var(--text-faint)] truncate">{prev}</span>
      {/if}
    {/if}
  </div>

  <!-- Labels -->
  {#if issue.labels.length > 0}
    <div class="flex items-center gap-1 shrink-0">
      {#each issue.labels.slice(0, 2) as lbl}
        {@const labelObj = labels.find((l) => l.name === lbl)}
        <span
          class="text-micro font-medium px-1.5 py-0.5 rounded-full
                 border border-[var(--border)]"
          style={labelObj ? `color: ${labelObj.color}; border-color: ${labelObj.color}40;` : ""}
        >
          {lbl}
        </span>
      {/each}
      {#if issue.labels.length > 2}
        <span class="text-micro text-[var(--text-faint)]">
          +{issue.labels.length - 2}
        </span>
      {/if}
    </div>
  {/if}

  <!-- LIF-191: module chip — which arc this issue belongs to. Hidden when
       already grouped by module (redundant). -->
  {#if issue.module_id != null && groupBy !== "module" && mod}
    <span class="shrink-0 inline-flex items-center gap-1 max-w-[130px] text-micro text-[var(--text-muted)]">
      {#if mod.emoji}
        <ProjectIcon value={mod.emoji} size={12} />
      {:else}
        <Layers size={11} class="text-[var(--text-faint)]" />
      {/if}
      <span class="truncate">{mod.name}</span>
    </span>
  {/if}

  <!-- LIF-191: priority — click to pick in place (mirrors the status
       picker). When 'none', a faint affordance appears on row hover. -->
  <div class="relative shrink-0 w-9 flex items-center justify-end">
    <Tooltip
      content={priorityOpen
        ? null
        : issue.priority === "none"
          ? "Set priority"
          : issue.priority[0].toUpperCase() + issue.priority.slice(1)}
    >
      <button
        class="size-6 flex items-center justify-end transition-opacity hover:opacity-100
               {issue.priority === 'none' ? 'opacity-0 group-hover:opacity-100' : ''}"
        onclick={(e) => {
          e.stopPropagation();
          onTogglePriorityDropdown(issue);
        }}
      >
        {#if issue.priority !== "none"}
          <PriorityIcon priority={issue.priority} size={21} />
        {:else}
          <Signal size={15} class="text-[var(--text-faint)]" />
        {/if}
      </button>
    </Tooltip>
    {#if priorityOpen}
      <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
      <div
        class="absolute right-0 top-full mt-1.5 z-30 w-[150px]
               bg-[var(--surface)] border border-[var(--border)]
               rounded-lg shadow-lg py-1.5"
        onclick={(e) => e.stopPropagation()}
      >
        {#each PRIORITIES as p}
          <button
            class="w-full flex items-center gap-2 px-3 py-1.5 text-left
                   text-body-sm capitalize transition-colors
                   {p === issue.priority
              ? 'text-[var(--accent)] bg-[var(--accent-subtle)] font-medium'
              : 'text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
            onclick={() => onPickPriority(issue, p)}
          >
            <span class="w-4 flex justify-center"><PriorityIcon priority={p} size={15} /></span>
            {p}
          </button>
        {/each}
      </div>
    {/if}
  </div>

  <!-- Updated time -->
  <span class="text-caption text-[var(--text-faint)] shrink-0 w-[60px] text-right">
    {formatRelative(issue.updated_at)}
  </span>
</div>
