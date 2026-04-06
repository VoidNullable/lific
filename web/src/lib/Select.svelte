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
  }: {
    options: Option[];
    value?: string | number | null;
    placeholder?: string;
    size?: "sm" | "md";
    class?: string;
    renderOption?: import("svelte").Snippet<[Option, boolean]>;
    renderSelected?: import("svelte").Snippet<[Option]>;
  } = $props();

  let sm = $derived(size === "sm");

  let open = $state(false);
  let triggerEl = $state<HTMLButtonElement | null>(null);

  let selected = $derived(options.find((o) => o.value === value));

  function select(opt: Option) {
    value = opt.value;
    open = false;
  }

  function toggle(e: Event) {
    e.stopPropagation();
    open = !open;
  }

  function handleWindowClick() {
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

<svelte:window onclick={handleWindowClick} />

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
        class="{sm ? 'text-[0.8125rem]' : 'text-[0.9375rem]'}
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
      class="absolute left-0 top-full mt-1 z-30 min-w-full w-max
             bg-[var(--surface)] border border-[var(--border)]
             rounded-lg shadow-lg py-1.5 max-h-[min(360px,_50vh)] overflow-y-auto"
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
                class="{sm ? 'text-[0.8125rem]' : 'text-[0.875rem]'}
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
