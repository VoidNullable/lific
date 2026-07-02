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
  import { Check, Signal, Layers, PanelRight, ExternalLink } from "lucide-svelte";
  import StatusIcon from "../StatusIcon.svelte";
  import PriorityIcon from "../PriorityIcon.svelte";
  import ProjectIcon from "../ProjectIcon.svelte";
  import Tooltip from "../Tooltip.svelte";
  import { formatRelative } from "../format";
  import { STATUSES, PRIORITIES, descriptionPreview } from "./grouping";
  import { openContextMenu } from "../contextMenu.svelte"; // LIF-248
  import { projectCodeOf } from "../references"; // LIF-248

  let {
    issue,
    idx,
    isLast = false,
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
    moduleOpen,
    statusPickerIdx,
    priorityPickerIdx,
    modulePickerIdx,
    onOpen,
    onPeek,
    onRangeSelect,
    onToggleSelect,
    onMouseEnterRow,
    onToggleStatusDropdown,
    onTogglePriorityDropdown,
    onToggleModuleDropdown,
    onPickStatus,
    onPickPriority,
    onPickModule,
    onHoverStatusOption,
    onHoverPriorityOption,
    onHoverModuleOption,
  }: {
    issue: Issue;
    idx: number;
    /** LIF-246: last row in its group/list — drives the border like the
     *  old `last:border-b-0` did. Explicit now (not CSS `:last-child`)
     *  because each row is wrapped in its own animate:flip div for the
     *  reorder glide, which would make `:last-child` match every row
     *  (each is the sole child of its own wrapper). */
    isLast?: boolean;
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
    /** This row's inline module dropdown is open (LIF-245). */
    moduleOpen: boolean;
    /** Highlighted index within the open status picker. */
    statusPickerIdx: number;
    /** Highlighted index within the open priority picker (LIF-245). */
    priorityPickerIdx: number;
    /** Highlighted index within the open module picker — 0 is "No module",
     *  n+1 is `modules[n]` (LIF-245). */
    modulePickerIdx: number;
    onOpen: (issue: Issue) => void;
    /** LIF-244: opens the peek panel on this issue (hover affordance —
     *  mod-click stays reserved for ctrl/cmd-toggle-select on rows, see
     *  the row's own onclick below). */
    onPeek: (issue: Issue) => void;
    onRangeSelect: (idx: number) => void;
    onToggleSelect: (id: number, idx: number) => void;
    onMouseEnterRow: (e: MouseEvent, idx: number) => void;
    onToggleStatusDropdown: (issue: Issue) => void;
    onTogglePriorityDropdown: (issue: Issue) => void;
    onToggleModuleDropdown: (issue: Issue) => void;
    onPickStatus: (issue: Issue, status: string) => void;
    onPickPriority: (issue: Issue, priority: string) => void;
    onPickModule: (issue: Issue, moduleId: number | null) => void;
    onHoverStatusOption: (si: number) => void;
    onHoverPriorityOption: (pi: number) => void;
    onHoverModuleOption: (mi: number) => void;
  } = $props();

  const mod = $derived(
    issue.module_id == null
      ? undefined
      : modules.find((m) => m.id === issue.module_id),
  );

  // LIF-248: right-click → preview / open-in-new-tab. A separate event
  // from the row's own `onclick` below, so it can't touch selection
  // (shift-click stays range-select, ctrl/cmd-click stays multi-select —
  // both untouched here) or navigation.
  function handleContextMenu(e: MouseEvent) {
    e.preventDefault();
    e.stopPropagation();
    openContextMenu(e.clientX, e.clientY, [
      { label: "Open preview", icon: PanelRight, action: () => onPeek(issue) },
      {
        label: "Open in new tab",
        icon: ExternalLink,
        action: () =>
          window.open(
            `${location.origin}/#/${projectCodeOf(issue.identifier)}/issues/${issue.identifier}`,
            "_blank",
            "noopener",
          ),
      },
    ]);
  }
</script>

<div
  class="w-full flex items-center gap-2 sm:gap-3 px-3 sm:px-6 text-left
         {density === 'comfortable' ? 'py-3' : 'py-2.5'}
         {isLast ? '' : 'border-b border-[var(--border)]'}
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
  oncontextmenu={handleContextMenu}
>
  <!-- Selection checkbox (LIF-149). Space is always reserved so rows never
       shift; the box is invisible until hover or until a selection exists
       anywhere, then stays visible for the session of that selection. -->
  <button
    class="size-4 shrink-0 rounded border flex items-center justify-center
           transition
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

  <!-- Identifier. Narrower on mobile (the project prefix is implied by
       context) so the title gets more room. -->
  <span class="text-body-sm text-[var(--text-faint)] font-mono shrink-0 w-[52px] sm:w-[72px] truncate">
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

  <!-- Labels. Hidden below sm — secondary metadata that would otherwise
       crush the title on a phone (LIF-229). -->
  {#if issue.labels.length > 0}
    <div class="hidden sm:flex items-center gap-1 shrink-0">
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

  <!-- LIF-191/245: module — click (or the `m` shortcut on the focused row)
       to pick in place, mirrors the status/priority pickers. Previously a
       read-only chip that hid itself when grouped by module (redundant
       with the group header); now that it's an editable control that hide
       would also make `m` and the popover invisible mid-group, so it
       always renders (still hover-revealed when unset, like priority's
       "none" affordance). -->
  <div class="relative shrink-0 hidden sm:block">
    <Tooltip content={moduleOpen ? null : mod ? mod.name : "Set module"}>
      <button
        class="max-w-[130px] flex items-center gap-1 text-micro
               text-[var(--text-muted)] transition-opacity hover:text-[var(--text)]
               {mod ? '' : 'opacity-0 group-hover:opacity-100'}"
        onclick={(e) => {
          e.stopPropagation();
          onToggleModuleDropdown(issue);
        }}
      >
        {#if mod?.emoji}
          <ProjectIcon value={mod.emoji} size={12} />
        {:else}
          <Layers size={11} class="text-[var(--text-faint)]" />
        {/if}
        <span class="truncate">{mod ? mod.name : "Module"}</span>
      </button>
    </Tooltip>
    {#if moduleOpen}
      <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
      <div
        class="absolute left-0 top-full mt-1.5 z-30 w-[180px] max-h-[240px]
               overflow-y-auto bg-[var(--surface)] border border-[var(--border)]
               rounded-lg shadow-lg py-1.5"
        onclick={(e) => e.stopPropagation()}
      >
        <button
          class="w-full flex items-center gap-2 px-3 py-1.5 text-left
                 text-body-sm transition-colors
                 {modulePickerIdx === 0
            ? 'text-[var(--accent)] bg-[var(--accent-subtle)] font-medium'
            : 'text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
          onclick={() => onPickModule(issue, null)}
          onmouseenter={() => onHoverModuleOption(0)}
        >
          <Layers size={13} class="text-[var(--text-faint)]" />
          No module
        </button>
        {#each modules as m, mi}
          <button
            class="w-full flex items-center gap-2 px-3 py-1.5 text-left
                   text-body-sm transition-colors truncate
                   {mi + 1 === modulePickerIdx
              ? 'text-[var(--accent)] bg-[var(--accent-subtle)] font-medium'
              : 'text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
            onclick={() => onPickModule(issue, m.id)}
            onmouseenter={() => onHoverModuleOption(mi + 1)}
          >
            {#if m.emoji}
              <ProjectIcon value={m.emoji} size={13} />
            {:else}
              <Layers size={13} class="text-[var(--text-faint)]" />
            {/if}
            <span class="truncate">{m.name}</span>
          </button>
        {/each}
      </div>
    {/if}
  </div>

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
        {#each PRIORITIES as p, pi}
          <button
            class="w-full flex items-center gap-2 px-3 py-1.5 text-left
                   text-body-sm capitalize transition-colors
                   {pi === priorityPickerIdx
              ? 'text-[var(--accent)] bg-[var(--accent-subtle)] font-medium'
              : 'text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
            onclick={() => onPickPriority(issue, p)}
            onmouseenter={() => onHoverPriorityOption(pi)}
          >
            <span class="w-4 flex justify-center"><PriorityIcon priority={p} size={15} /></span>
            {p}
          </button>
        {/each}
      </div>
    {/if}
  </div>

  <!-- LIF-244: peek affordance. Hover-only trigger for the slide-over
       preview — mirrors the checkbox/priority hover pattern above, but
       `[@media(hover:hover)]` (rather than plain `group-hover`) also
       fully removes it on touch (no hover capability), where a phantom
       hover-in-waiting affordance would just be a dead tap target. -->
  <Tooltip content="Peek">
    <button
      class="hidden shrink-0 size-6 items-center justify-center rounded
             text-[var(--text-faint)] hover:text-[var(--accent)]
             hover:bg-[var(--bg-subtle)] transition-colors
             [@media(hover:hover)]:flex [@media(hover:hover)]:opacity-0
             [@media(hover:hover)]:group-hover:opacity-100"
      onclick={(e) => {
        e.stopPropagation();
        onPeek(issue);
      }}
    >
      <PanelRight size={13} />
    </button>
  </Tooltip>

  <!-- Updated time. Hidden below sm to give the title room (LIF-229). -->
  <span class="hidden sm:block text-caption text-[var(--text-faint)] shrink-0 w-[60px] text-right">
    {formatRelative(issue.updated_at)}
  </span>
</div>
