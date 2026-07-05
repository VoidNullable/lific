<script module lang="ts">
  // Exported from the module context so the parent can import the type
  // (instance-script exports aren't visible as module exports in Svelte 5).
  export type BulkMenu =
    | "status"
    | "priority"
    | "module"
    | "label"
    | "delete"
    | null;
</script>

<script lang="ts">
  // LIF-149 floating bulk-action bar. Extracted from IssueList.svelte
  // (LIF-99). Appears while issues are selected; menus open upward.
  //
  // `bulkMenu` is bindable because the parent also mutates it (Escape key,
  // outside-click on window), so the open menu must stay in sync both ways.
  // All the actual mutations live in the parent and arrive as callbacks.
  import type { Module, Label } from "../api";
  import { fly } from "svelte/transition";
  import { Trash2, X } from "lucide-svelte";
  import StatusIcon from "../StatusIcon.svelte";
  import PriorityIcon from "../PriorityIcon.svelte";
  import { STATUSES, PRIORITIES } from "./grouping";

  let {
    selectedCount,
    bulkBusy,
    bulkMenu = $bindable(),
    modules,
    labels,
    onUpdate,
    onAddLabel,
    onDelete,
    onClear,
  }: {
    selectedCount: number;
    bulkBusy: boolean;
    bulkMenu: BulkMenu;
    modules: Module[];
    labels: Label[];
    /** Apply a field update to every selected issue. */
    onUpdate: (input: Record<string, unknown>) => void;
    /** Add one label (by name) to every selected issue. */
    onAddLabel: (name: string) => void;
    /** Delete every selected issue. */
    onDelete: () => void;
    /** Clear the selection. */
    onClear: () => void;
  } = $props();
</script>

<!-- The outer div owns viewport centering; the inner div owns the entrance
     fly so the two transforms don't fight.
     LIF-276: below sm the bar pins to both edges (inset-x-3) instead of
     centering a min-content row — six inline controls exceed a 360px
     viewport. The row wraps if it still can't fit, and bottom offset
     respects the home-indicator safe area. -->
<div
  class="fixed z-40 inset-x-3 bottom-[max(1rem,env(safe-area-inset-bottom))]
         sm:inset-x-auto sm:bottom-6 sm:left-1/2 sm:-translate-x-1/2"
>
  <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
  <div
    class="flex flex-wrap items-center justify-center gap-0.5 gap-y-1 pl-3 pr-1.5 py-1.5
           bg-[var(--surface)] border border-[var(--border)]
           rounded-xl shadow-[0_8px_28px_rgba(0,0,0,0.18)]"
    in:fly={{ y: 8, duration: 180 }}
    onclick={(e) => e.stopPropagation()}
  >
    <span class="text-body-sm font-medium text-[var(--text)] tabular-nums pr-1">
      {selectedCount} selected
    </span>
    {#if bulkBusy}
      <span class="text-caption text-[var(--text-faint)] animate-pulse pr-1">
        Applying...
      </span>
    {/if}
    <div class="w-px h-4 bg-[var(--border)] mx-1"></div>

    {#snippet bulkTrigger(menu: "status" | "priority" | "module" | "label", label: string)}
      <button
        class="text-body-sm px-2 py-1 rounded-md transition-colors
               disabled:opacity-50 disabled:cursor-not-allowed
               {bulkMenu === menu
          ? 'text-[var(--text)] bg-[var(--bg-subtle)]'
          : 'text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
        disabled={bulkBusy}
        onclick={(e) => {
          e.stopPropagation();
          bulkMenu = bulkMenu === menu ? null : menu;
        }}
      >
        {label}
      </button>
    {/snippet}

    <!-- Status -->
    <div class="relative">
      {@render bulkTrigger("status", "Status")}
      {#if bulkMenu === "status"}
        <div
          class="absolute bottom-full mb-1.5 left-0 w-[160px]
                 bg-[var(--surface)] border border-[var(--border)]
                 rounded-lg shadow-lg py-1.5"
        >
          {#each STATUSES as s}
            <button
              class="w-full flex items-center gap-2 px-3 py-1.5 text-left
                     text-body-sm text-[var(--text)] capitalize
                     hover:bg-[var(--bg-subtle)] transition-colors"
              onclick={() => onUpdate({ status: s })}
            >
              <StatusIcon status={s} size={14} />
              {s}
            </button>
          {/each}
        </div>
      {/if}
    </div>

    <!-- Priority -->
    <div class="relative">
      {@render bulkTrigger("priority", "Priority")}
      {#if bulkMenu === "priority"}
        <div
          class="absolute bottom-full mb-1.5 left-0 w-[160px]
                 bg-[var(--surface)] border border-[var(--border)]
                 rounded-lg shadow-lg py-1.5"
        >
          {#each PRIORITIES as p}
            <button
              class="w-full flex items-center gap-2 px-3 py-1.5 text-left
                     text-body-sm text-[var(--text)] capitalize
                     hover:bg-[var(--bg-subtle)] transition-colors"
              onclick={() => onUpdate({ priority: p })}
            >
              <PriorityIcon priority={p} size={14} />
              {p}
            </button>
          {/each}
        </div>
      {/if}
    </div>

    <!-- Module (only when the project has modules) -->
    {#if modules.length > 0}
      <div class="relative">
        {@render bulkTrigger("module", "Module")}
        {#if bulkMenu === "module"}
          <div
            class="absolute bottom-full mb-1.5 left-0 w-[180px]
                   bg-[var(--surface)] border border-[var(--border)]
                   rounded-lg shadow-lg py-1.5 max-h-[40vh] overflow-y-auto"
          >
            <button
              class="w-full px-3 py-1.5 text-left text-body-sm
                     text-[var(--text-faint)] hover:bg-[var(--bg-subtle)]
                     transition-colors"
              onclick={() => onUpdate({ module_id: null })}
            >
              None
            </button>
            {#each modules as mod (mod.id)}
              <button
                class="w-full px-3 py-1.5 text-left text-body-sm
                       text-[var(--text)] hover:bg-[var(--bg-subtle)]
                       transition-colors truncate"
                onclick={() => onUpdate({ module_id: mod.id })}
              >
                {mod.name}
              </button>
            {/each}
          </div>
        {/if}
      </div>
    {/if}

    <!-- Add label (only when the project has labels) -->
    {#if labels.length > 0}
      <div class="relative">
        {@render bulkTrigger("label", "Label")}
        {#if bulkMenu === "label"}
          <div
            class="absolute bottom-full mb-1.5 left-0 w-[180px]
                   bg-[var(--surface)] border border-[var(--border)]
                   rounded-lg shadow-lg py-1.5 max-h-[40vh] overflow-y-auto"
          >
            {#each labels as lbl (lbl.id)}
              <button
                class="w-full flex items-center gap-2 px-3 py-1.5 text-left
                       text-body-sm text-[var(--text)]
                       hover:bg-[var(--bg-subtle)] transition-colors"
                onclick={() => onAddLabel(lbl.name)}
              >
                <span
                  class="size-2.5 rounded-full shrink-0"
                  style="background: {lbl.color}"
                ></span>
                <span class="truncate">{lbl.name}</span>
              </button>
            {/each}
          </div>
        {/if}
      </div>
    {/if}

    <div class="w-px h-4 bg-[var(--border)] mx-1"></div>

    <!-- Delete (confirm popover) -->
    <div class="relative">
      <button
        class="size-7 flex items-center justify-center rounded-md
               text-[var(--error)] hover:bg-[var(--error-bg)]
               transition-colors disabled:opacity-50"
        title="Delete selected"
        disabled={bulkBusy}
        onclick={(e) => {
          e.stopPropagation();
          bulkMenu = bulkMenu === "delete" ? null : "delete";
        }}
      >
        <Trash2 size={14} />
      </button>
      {#if bulkMenu === "delete"}
        <div
          class="absolute bottom-full mb-1.5 right-0 w-[240px]
                 bg-[var(--surface)] border border-[var(--border)]
                 rounded-lg shadow-lg p-3"
        >
          <p class="text-body-sm font-medium text-[var(--text)] mb-1">
            Delete {selectedCount} issue{selectedCount === 1 ? "" : "s"}?
          </p>
          <p class="text-caption text-[var(--text-muted)] mb-3">
            This can't be undone.
          </p>
          <div class="flex items-center gap-2">
            <button
              class="text-body-sm font-medium text-[var(--error-text)]
                     bg-[var(--error)] px-3 py-1.5 rounded-md
                     hover:opacity-90 transition-opacity
                     disabled:opacity-50 disabled:cursor-not-allowed"
              disabled={bulkBusy}
              onclick={onDelete}
            >
              {bulkBusy ? "Deleting..." : "Delete"}
            </button>
            <button
              class="text-body-sm text-[var(--text-muted)] px-3 py-1.5
                     rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
              onclick={() => { bulkMenu = null; }}
            >
              Cancel
            </button>
          </div>
        </div>
      {/if}
    </div>

    <!-- Clear -->
    <button
      class="size-7 flex items-center justify-center rounded-md
             text-[var(--text-muted)] hover:text-[var(--text)]
             hover:bg-[var(--bg-subtle)] transition-colors"
      title="Clear selection  ·  Esc"
      onclick={onClear}
    >
      <X size={14} />
    </button>
  </div>
</div>
