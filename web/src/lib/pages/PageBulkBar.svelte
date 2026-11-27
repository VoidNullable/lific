<script lang="ts">
  // What to do with the selected pages: move them, change their status, add
  // a label, pin them, export them as Markdown.
  import { fly } from "svelte/transition";
  import { X } from "lucide-svelte";
  import type { Folder, Label } from "../api";
  import { safeLabelColor } from "../labelColors";
  import { orderedFolders, STATUS_LABEL } from "./list";
  import { motionReduced } from "../theme";

  let {
    count,
    busy,
    folders,
    labels,
    onMove,
    onStatus,
    onLabel,
    onPin,
    onExport,
    onClear,
  }: {
    count: number;
    busy: boolean;
    folders: Folder[];
    labels: Label[];
    onMove: (folderId: number | null) => void;
    onStatus: (status: string) => void;
    onLabel: (name: string) => void;
    onPin: (pinned: boolean) => void;
    onExport: () => void;
    onClear: () => void;
  } = $props();

  let menu = $state<"move" | "status" | "label" | null>(null);
  const nodes = $derived(orderedFolders(folders));

  const trigger = (on: boolean) =>
    `text-body-sm px-2 py-1 rounded-md transition-colors disabled:opacity-50 ${
      on ? "text-[var(--text)] bg-[var(--bg-subtle)]" : "text-[var(--text-muted)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]"
    }`;
  const pop =
    "absolute bottom-full mb-1.5 left-0 bg-[var(--surface)] border border-[var(--border)] rounded-lg shadow-lg py-1.5 max-h-[40vh] overflow-y-auto";
  const item = "w-full flex items-center gap-2 px-3 py-1.5 text-left text-body-sm text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors";

  function run(fn: () => void) {
    menu = null;
    fn();
  }
</script>

<svelte:window onclick={() => (menu = null)} />

<div class="fixed z-40 inset-x-3 bottom-[max(1rem,env(safe-area-inset-bottom))] sm:inset-x-auto sm:bottom-6 sm:left-1/2 sm:-translate-x-1/2">
  <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
  <div
    class="flex flex-wrap items-center justify-center gap-0.5 gap-y-1 pl-3 pr-1.5 py-1.5
           bg-[var(--surface)] border border-[var(--border)] rounded-xl shadow-[0_8px_28px_rgba(0,0,0,0.18)]"
    in:fly={{ y: 8, duration: motionReduced() ? 0 : 180 }}
    onclick={(e) => e.stopPropagation()}
    data-testid="pages-bulk-bar"
  >
    <span class="text-body-sm font-medium text-[var(--text)] tabular-nums pr-1">{count} selected</span>
    {#if busy}<span class="text-caption text-[var(--text-faint)] animate-pulse pr-1">Working...</span>{/if}
    <div class="w-px h-4 bg-[var(--border)] mx-1"></div>

    <div class="relative">
      <button class={trigger(menu === "move")} disabled={busy} onclick={() => (menu = menu === "move" ? null : "move")}>Move</button>
      {#if menu === "move"}
        <div class="{pop} w-[220px]">
          <button class="{item} text-[var(--text-muted)]" onclick={() => run(() => onMove(null))}>Unfiled</button>
          {#each nodes as { folder, depth } (folder.id)}
            <button class={item} style="padding-left: {12 + depth * 12}px" onclick={() => run(() => onMove(folder.id))}>
              <span class="truncate">{folder.name}</span>
            </button>
          {/each}
        </div>
      {/if}
    </div>

    <div class="relative">
      <button class={trigger(menu === "status")} disabled={busy} onclick={() => (menu = menu === "status" ? null : "status")}>Status</button>
      {#if menu === "status"}
        <div class="{pop} w-[160px]">
          {#each ["draft", "active", "complete", "archived"] as s (s)}
            <button class={item} onclick={() => run(() => onStatus(s))}>{s === "archived" ? "Archive" : STATUS_LABEL[s]}</button>
          {/each}
        </div>
      {/if}
    </div>

    {#if labels.length > 0}
      <div class="relative">
        <button class={trigger(menu === "label")} disabled={busy} onclick={() => (menu = menu === "label" ? null : "label")}>Label</button>
        {#if menu === "label"}
          <div class="{pop} w-[200px]">
            {#each labels as l (l.id)}
              <button class={item} onclick={() => run(() => onLabel(l.name))}>
                <span class="size-2.5 rounded-full shrink-0" style="background: {safeLabelColor(l.color)}"></span>
                <span class="truncate">{l.name}</span>
              </button>
            {/each}
          </div>
        {/if}
      </div>
    {/if}

    <button class={trigger(false)} disabled={busy} onclick={() => onPin(true)}>Pin</button>
    <button class={trigger(false)} disabled={busy} onclick={() => onPin(false)}>Unpin</button>
    <button class={trigger(false)} disabled={busy} onclick={onExport}>Export</button>

    <div class="w-px h-4 bg-[var(--border)] mx-1"></div>
    <button
      class="size-7 grid place-items-center rounded-md text-[var(--text-faint)] hover:text-[var(--text)] hover:bg-[var(--bg-subtle)]"
      aria-label="Clear selection"
      onclick={onClear}
    >
      <X size={14} />
    </button>
  </div>
</div>
