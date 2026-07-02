<script lang="ts">
  // Compact color picker (label management #2). A single swatch trigger that
  // opens a popover with the named palette plus a free `#hex` field — far
  // calmer than painting all 12 swatches inline on every row, and capable of
  // brand colors the palette doesn't include. Controlled: `value` in,
  // `onChange` out; open state is internal with outside-click + Esc close.
  import { Check } from "lucide-svelte";
  import {
    LABEL_PALETTE,
    colorName,
    isValidHex,
    normalizeHex,
  } from "./labelColors";

  let {
    value,
    onChange,
    size = 20,
    align = "left",
  }: {
    value: string;
    onChange: (hex: string) => void;
    /** Trigger swatch diameter in px. */
    size?: number;
    /** Popover horizontal anchor. */
    align?: "left" | "right";
  } = $props();

  let open = $state(false);
  let hexDraft = $state("");
  let hexBad = $state(false);

  function toggle(e: MouseEvent) {
    e.stopPropagation();
    open = !open;
    if (open) {
      hexDraft = value.replace(/^#/, "");
      hexBad = false;
    }
  }

  function pick(hex: string) {
    onChange(hex);
    open = false;
  }

  function commitHex() {
    if (!isValidHex(hexDraft)) {
      hexBad = true;
      return;
    }
    onChange(normalizeHex(hexDraft));
    open = false;
  }
</script>

<svelte:window onclick={() => (open = false)} />

<div class="relative inline-flex">
  <button
    type="button"
    class="rounded-full shrink-0 border border-black/10 dark:border-white/15
           shadow-sm transition-transform hover:scale-105
           focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
    style="width: {size}px; height: {size}px; background: {value}"
    aria-label="Color: {colorName(value)}. Click to change."
    title="{colorName(value)} · {value}"
    onclick={toggle}
  ></button>

  {#if open}
    <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
    <div
      class="absolute top-full mt-1.5 z-40 w-[208px] p-2.5
             bg-[var(--surface)] border border-[var(--border)]
             rounded-lg shadow-lg {align === 'right' ? 'right-0' : 'left-0'}"
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => e.stopPropagation()}
    >
      <div class="grid grid-cols-6 gap-1.5 mb-2.5">
        {#each LABEL_PALETTE as c (c.value)}
          {@const selected = c.value.toLowerCase() === value.toLowerCase()}
          <button
            type="button"
            class="size-6 rounded-full grid place-items-center transition-transform
                   hover:scale-110
                   {selected ? 'ring-2 ring-offset-1 ring-offset-[var(--surface)] ring-[var(--text)]' : ''}"
            style="background: {c.value}"
            aria-label={c.name}
            title={c.name}
            onclick={() => pick(c.value)}
          >
            {#if selected}<Check size={13} class="text-white drop-shadow" />{/if}
          </button>
        {/each}
      </div>

      <!-- Custom hex -->
      <div class="flex items-center gap-1.5">
        <span class="text-body-sm text-[var(--text-faint)] font-mono">#</span>
        <input
          bind:value={hexDraft}
          spellcheck="false"
          maxlength="7"
          placeholder="hex"
          class="flex-1 min-w-0 px-1.5 py-1 text-body-sm font-mono rounded
                 border bg-[var(--bg)] text-[var(--text)] outline-none
                 focus:border-[var(--accent)]
                 {hexBad ? 'border-[var(--error)]' : 'border-[var(--border)]'}"
          oninput={() => (hexBad = false)}
          onkeydown={(e) => {
            if (e.key === 'Enter') { e.preventDefault(); commitHex(); }
            if (e.key === 'Escape') { e.preventDefault(); open = false; }
          }}
        />
        <button
          type="button"
          class="px-2 py-1 text-body-sm font-medium rounded
                 bg-[var(--bg-subtle)] text-[var(--text)]
                 hover:bg-[var(--border)] transition-colors"
          onclick={commitHex}
        >
          Set
        </button>
      </div>
    </div>
  {/if}
</div>
