<script lang="ts">
  // LIF-123 — label chips + add/remove picker, shared by IssueDetail
  // (sidebar field) and PageDetail (inline strip below the title). The
  // chip rendering and picker popover are identical across both; only
  // the empty-state copy, popover width, and whether the empty text
  // shows while editable differ — all surfaced as props.
  //
  // `open` is bindable so a parent with sibling dropdowns (the issue
  // sidebar) can coordinate them: it binds `open` and uses `onOpen` to
  // close its other menus when this one opens. Standalone consumers
  // (the page) can ignore both and rely on the internal outside-click
  // close.

  import { Plus, X, Check } from "lucide-svelte";
  import type { Label } from "./api";

  let {
    attached,
    all,
    editable = true,
    onToggle,
    open = $bindable(false),
    onOpen,
    emptyText = "None",
    emptyItalic = false,
    hideEmptyWhenEditable = false,
    popoverWidth = "w-[180px]",
    emptyPickerText = "No labels defined",
  }: {
    /** Names of labels currently attached to the object. */
    attached: string[];
    /** All labels available in the project. */
    all: Label[];
    editable?: boolean;
    onToggle: (name: string) => void;
    open?: boolean;
    onOpen?: () => void;
    emptyText?: string;
    emptyItalic?: boolean;
    hideEmptyWhenEditable?: boolean;
    popoverWidth?: string;
    emptyPickerText?: string;
  } = $props();

  function handleWindowClick() {
    open = false;
  }

  function toggleOpen(e: MouseEvent) {
    e.stopPropagation();
    const next = !open;
    open = next;
    if (next) onOpen?.();
  }
</script>

<svelte:window onclick={handleWindowClick} />

<div class="relative">
  <div class="flex flex-wrap gap-1.5 items-center">
    {#if attached.length > 0}
      {#each attached as name}
        {@const obj = all.find((l) => l.name === name)}
        <span
          class="inline-flex items-center gap-1 text-[0.75rem]
                 font-medium px-2 py-0.5 rounded-full border"
          style={obj
            ? `color: ${obj.color}; border-color: ${obj.color}40; background: ${obj.color}10;`
            : ""}
        >
          {name}
          {#if editable}
            <button
              class="size-3 rounded-full hover:bg-[var(--bg-subtle)]
                     inline-flex items-center justify-center opacity-60
                     hover:opacity-100 transition-opacity"
              onclick={(e) => { e.stopPropagation(); onToggle(name); }}
              title="Remove label"
            >
              <X size={10} />
            </button>
          {/if}
        </span>
      {/each}
    {:else if !(editable && hideEmptyWhenEditable)}
      <span class="text-[0.8125rem] text-[var(--text-faint)] {emptyItalic ? 'italic' : ''}">
        {emptyText}
      </span>
    {/if}

    {#if editable}
      <button
        class="size-5 rounded border border-dashed border-[var(--border)]
               text-[var(--text-faint)] hover:border-[var(--accent)]
               hover:text-[var(--accent)] flex items-center justify-center
               transition-colors"
        title="Add label"
        onclick={toggleOpen}
      >
        <Plus size={12} />
      </button>
    {/if}
  </div>

  {#if open}
    <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
    <div
      class="absolute left-0 top-full mt-1 z-20 {popoverWidth}
             bg-[var(--surface)] border border-[var(--border)]
             rounded-md shadow-lg py-1"
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => e.stopPropagation()}
    >
      {#if all.length === 0}
        <div class="px-3 py-2 text-[0.8125rem] text-[var(--text-faint)]">
          {emptyPickerText}
        </div>
      {:else}
        {#each all as label}
          {@const isAttached = attached.includes(label.name)}
          <button
            class="w-full flex items-center gap-2 px-3 py-1.5 text-left
                   text-[0.8125rem] transition-colors hover:bg-[var(--bg-subtle)]"
            onclick={() => onToggle(label.name)}
          >
            <span
              class="size-2.5 rounded-full shrink-0"
              style="background: {label.color};"
            ></span>
            <span class="flex-1 {isAttached ? 'font-medium' : ''}">
              {label.name}
            </span>
            {#if isAttached}
              <Check size={14} class="text-[var(--accent)]" />
            {/if}
          </button>
        {/each}
      {/if}
    </div>
  {/if}
</div>
