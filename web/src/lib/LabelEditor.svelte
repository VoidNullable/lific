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
  //
  // Inline creation (label management): when an `onCreate` callback is
  // provided, the picker grows a filter/create input. Typing a name that
  // doesn't exist yet reveals a color row + Create button, so a user can
  // mint a label from inside an issue without leaving for project settings.

  import { Plus, X, Check } from "lucide-svelte";
  import type { Label } from "./api";
  import { colorForName } from "./labelColors";
  import ColorPicker from "./ColorPicker.svelte";

  let {
    attached,
    all,
    editable = true,
    onToggle,
    open = $bindable(false),
    onOpen,
    onCreate,
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
    /** When provided, the picker offers inline label creation. Returns true
     *  on success (parent persists + attaches + refreshes `all`). */
    onCreate?: (name: string, color: string) => Promise<boolean>;
    emptyText?: string;
    emptyItalic?: boolean;
    hideEmptyWhenEditable?: boolean;
    popoverWidth?: string;
    emptyPickerText?: string;
  } = $props();

  let query = $state("");
  let pickedColor = $state<string | null>(null);
  let creating = $state(false);
  let inputEl = $state<HTMLInputElement | null>(null);

  // Effective create color: the user's explicit pick, else a stable color
  // derived from the typed name.
  let createColor = $derived(pickedColor ?? colorForName(query.trim() || "label"));

  let trimmed = $derived(query.trim());
  let filtered = $derived(
    trimmed
      ? all.filter((l) => l.name.toLowerCase().includes(trimmed.toLowerCase()))
      : all,
  );
  let exactMatch = $derived(
    all.some((l) => l.name.toLowerCase() === trimmed.toLowerCase()),
  );
  let canCreate = $derived(!!onCreate && trimmed.length > 0 && !exactMatch);

  function handleWindowClick() {
    open = false;
  }

  function toggleOpen(e: MouseEvent) {
    e.stopPropagation();
    const next = !open;
    open = next;
    if (next) {
      query = "";
      pickedColor = null;
      onOpen?.();
      // Focus the filter/create input on open (when creation is enabled).
      if (onCreate) requestAnimationFrame(() => inputEl?.focus());
    }
  }

  async function doCreate() {
    if (!onCreate || !canCreate || creating) return;
    creating = true;
    const ok = await onCreate(trimmed, createColor);
    creating = false;
    if (ok) {
      query = "";
      pickedColor = null;
      requestAnimationFrame(() => inputEl?.focus());
    }
  }

  function onInputKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      if (canCreate) {
        doCreate();
      } else if (filtered.length === 1) {
        onToggle(filtered[0].name);
      }
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      open = false;
    }
  }
</script>

<svelte:window onclick={handleWindowClick} />

<div class="relative">
  <div class="flex flex-wrap gap-1.5 items-center">
    {#if attached.length > 0}
      {#each attached as name}
        {@const obj = all.find((l) => l.name === name)}
        <span
          class="inline-flex items-center gap-1 text-caption
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
      <span class="text-body-sm text-[var(--text-faint)] {emptyItalic ? 'italic' : ''}">
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
      class="absolute left-0 top-full mt-1 z-20 {onCreate ? 'w-[240px]' : popoverWidth}
             bg-[var(--surface)] border border-[var(--border)]
             rounded-md shadow-lg py-1"
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => e.stopPropagation()}
    >
      {#if onCreate}
        <!-- Filter / create input -->
        <div class="px-2 pt-1 pb-1.5">
          <input
            bind:this={inputEl}
            bind:value={query}
            type="text"
            placeholder="Filter or create…"
            class="w-full px-2 py-1 text-body-sm rounded
                   border border-[var(--border)] bg-[var(--bg)]
                   text-[var(--text)] placeholder:text-[var(--text-faint)]
                   outline-none focus:border-[var(--accent)]"
            onkeydown={onInputKeydown}
          />
        </div>
      {/if}

      <div class="max-h-[220px] overflow-y-auto">
        {#if all.length === 0 && !canCreate}
          <div class="px-3 py-2 text-body-sm text-[var(--text-faint)]">
            {emptyPickerText}
          </div>
        {:else}
          {#each filtered as label (label.id)}
            {@const isAttached = attached.includes(label.name)}
            <button
              class="w-full flex items-center gap-2 px-3 py-1.5 text-left
                     text-body-sm transition-colors hover:bg-[var(--bg-subtle)]"
              onclick={() => onToggle(label.name)}
            >
              <span
                class="size-2.5 rounded-full shrink-0"
                style="background: {label.color};"
              ></span>
              <span class="flex-1 min-w-0 truncate {isAttached ? 'font-medium' : ''}">
                {label.name}
              </span>
              {#if isAttached}
                <Check size={14} class="text-[var(--accent)] shrink-0" />
              {/if}
            </button>
          {/each}
          {#if onCreate && filtered.length === 0 && !canCreate}
            <div class="px-3 py-2 text-body-sm text-[var(--text-faint)]">
              {trimmed ? "No match" : emptyPickerText}
            </div>
          {/if}
        {/if}
      </div>

      {#if canCreate}
        <!-- Create section: compact color picker + Create button -->
        <div class="border-t border-[var(--border)] mt-1 pt-2 px-2.5 pb-2 flex items-center gap-2">
          <ColorPicker value={createColor} onChange={(c) => { pickedColor = c; }} />
          <button
            class="flex-1 flex items-center justify-center gap-1.5 px-2 py-1.5 rounded
                   text-body-sm font-medium text-[var(--btn-success-text)]
                   bg-[var(--btn-success)] hover:bg-[var(--btn-success-hover)]
                   transition-colors disabled:opacity-50"
            disabled={creating}
            onclick={doCreate}
          >
            {creating ? "Creating…" : `Create “${trimmed}”`}
          </button>
        </div>
      {/if}
    </div>
  {/if}
</div>
