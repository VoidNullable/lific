<script lang="ts">
  import { ChevronDown, Check } from "lucide-svelte";

  type Option = { value: string | number | null; label: string; [key: string]: unknown };

  let {
    options,
    value = $bindable(null),
    placeholder = "Select...",
    size = "md",
    class: className = "",
    renderOption,
    renderSelected,
    onchange,
  }: {
    options: Option[];
    value?: string | number | null;
    placeholder?: string;
    size?: "sm" | "md";
    class?: string;
    renderOption?: import("svelte").Snippet<[Option, boolean]>;
    renderSelected?: import("svelte").Snippet<[Option]>;
    /** Fired with the newly-picked option, in addition to the `bind:value`
     *  update — handy for per-row selects (e.g. a list loop) where a plain
     *  two-way binding can't carry which row changed. */
    onchange?: (opt: Option) => void;
  } = $props();

  let sm = $derived(size === "sm");

  let open = $state(false);
  let triggerEl = $state<HTMLButtonElement | null>(null);
  let menuEl = $state<HTMLDivElement | null>(null);

  // The menu is position:fixed with viewport coordinates (same technique as
  // Tooltip.svelte) so it escapes `overflow: hidden` ancestors — e.g. the
  // rounded settings cards (ProjectMembers, LabelManager) clip any
  // absolute-positioned child to the card.
  let menuPos = $state({ top: 0, left: 0, minWidth: 0 });

  function seedMenuPos() {
    if (!triggerEl) return;
    const t = triggerEl.getBoundingClientRect();
    menuPos = { top: t.bottom + 4, left: t.left, minWidth: t.width };
  }

  // Refine once the menu has a real size: flip above the trigger when it
  // would run off the bottom of the viewport, clamp to the right edge.
  $effect(() => {
    if (!open || !menuEl || !triggerEl) return;
    const t = triggerEl.getBoundingClientRect();
    const m = menuEl.getBoundingClientRect();
    let top = t.bottom + 4;
    if (top + m.height > window.innerHeight - 8 && t.top - m.height - 4 >= 8) {
      top = t.top - m.height - 4;
    }
    const left = Math.max(8, Math.min(t.left, window.innerWidth - m.width - 8));
    menuPos = { top, left, minWidth: t.width };
  });

  let selected = $derived(options.find((o) => o.value === value));

  function select(opt: Option) {
    value = opt.value;
    open = false;
    onchange?.(opt);
  }

  function toggle(e: Event) {
    e.stopPropagation();
    if (!open) seedMenuPos();
    open = !open;
  }

  function handleWindowClick() {
    open = false;
  }

  // A fixed-position menu doesn't travel with its trigger, so close it when
  // anything scrolls or the window resizes — except scrolls happening inside
  // the menu itself (it has its own overflow-y).
  function handleScrollOrResize(e: Event) {
    if (!open) return;
    if (e.type === "scroll" && menuEl && e.target instanceof Node && menuEl.contains(e.target)) {
      return;
    }
    open = false;
  }

  function handleKeydown(e: KeyboardEvent) {
    if (!open) {
      if (e.key === "Enter" || e.key === " " || e.key === "ArrowDown") {
        e.preventDefault();
        open = true;
      }
      return;
    }

    if (e.key === "Escape") {
      open = false;
      triggerEl?.focus();
      return;
    }

    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const idx = options.findIndex((o) => o.value === value);
      const next =
        e.key === "ArrowDown"
          ? Math.min(idx + 1, options.length - 1)
          : Math.max(idx - 1, 0);
      value = options[next].value;
    }

    if (e.key === "Enter") {
      open = false;
      triggerEl?.focus();
    }
  }
</script>

<svelte:window
  onclick={handleWindowClick}
  onscrollcapture={handleScrollOrResize}
  onresize={handleScrollOrResize}
/>

<div class="relative {className}">
  <button
    bind:this={triggerEl}
    class="w-full flex items-center justify-between gap-1.5 rounded-md
           text-left border transition-colors outline-none
           hover:border-[var(--text-faint)]
           focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_var(--accent-subtle)]
           {sm
             ? 'px-2 py-1 border-[var(--border)] bg-[var(--surface)]'
             : 'px-3 py-2.5 border-[var(--border)] bg-[var(--bg-subtle)]'}
           {open ? 'border-[var(--accent)] shadow-[0_0_0_3px_var(--accent-subtle)]' : ''}"
    onclick={toggle}
    onkeydown={handleKeydown}
    type="button"
  >
    {#if selected && renderSelected}
      {@render renderSelected(selected)}
    {:else}
      <span
        class="{sm ? 'text-body-sm' : 'text-body-lg'}
               {selected ? 'text-[var(--text)]' : 'text-[var(--text-muted)]'}"
      >
        {selected?.label ?? placeholder}
      </span>
    {/if}
    <ChevronDown
      size={sm ? 12 : 14}
      class="shrink-0 text-[var(--text-faint)] transition-transform
             {open ? 'rotate-180' : ''}"
    />
  </button>

  {#if open}
    <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
    <div
      bind:this={menuEl}
      class="fixed z-50 w-max
             bg-[var(--surface)] border border-[var(--border)]
             rounded-lg shadow-lg py-1.5 max-h-[min(360px,_50vh)] overflow-y-auto"
      style="top: {menuPos.top}px; left: {menuPos.left}px; min-width: {menuPos.minWidth}px;"
      onclick={(e) => e.stopPropagation()}
    >
      {#each options as opt (opt.value)}
        {@const isSelected = opt.value === value}
        <button
          class="w-full flex items-center gap-2 text-left transition-colors
                 {sm ? 'px-2.5 py-1.5' : 'px-3 py-2'}
                 {isSelected
            ? 'bg-[var(--accent-subtle)]'
            : 'hover:bg-[var(--bg-subtle)]'}"
          onclick={() => select(opt)}
        >
          <div class="flex-1 min-w-0">
            {#if renderOption}
              {@render renderOption(opt, isSelected)}
            {:else}
              <span
                class="{sm ? 'text-body-sm' : 'text-body'}
                       {isSelected ? 'text-[var(--accent)] font-medium' : 'text-[var(--text)]'}"
              >
                {opt.label}
              </span>
            {/if}
          </div>
          {#if isSelected}
            <Check size={sm ? 12 : 14} class="shrink-0 text-[var(--accent)]" />
          {/if}
        </button>
      {/each}
    </div>
  {/if}
</div>
