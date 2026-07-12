<script lang="ts">
  // Shared sub-tab strip for resource list views (LIF-305): Modules, Pages,
  // Plans. Each tab is a content slice of the same collection (e.g.
  // Active / Backlog / Archive), so it uses the underlined "sections of one
  // area" style from SettingsTabs rather than the segmented pill, which is
  // reserved for view-mode switches (list vs board).
  //
  // Counts are optional per tab: pass a number to render a muted tally
  // beside the label; omit (or pass null) for tabs where a count is
  // meaningless (e.g. a recency-capped "Recent" slice).
  export type SubTab = {
    id: string;
    label: string;
    count?: number | null;
  };

  let {
    tabs,
    active,
    onselect,
  }: {
    tabs: SubTab[];
    active: string;
    onselect: (id: string) => void;
  } = $props();
</script>

<div
  class="flex items-center gap-5 border-b border-[var(--border)] overflow-x-auto"
  role="tablist"
>
  {#each tabs as tab (tab.id)}
    <button
      role="tab"
      aria-selected={active === tab.id}
      class="relative -mb-px shrink-0 flex items-center gap-1.5 px-0.5 pb-2 pt-1
             text-body font-medium border-b-2 transition-colors
             {active === tab.id
        ? 'border-[var(--accent)] text-[var(--text)]'
        : 'border-transparent text-[var(--text-muted)] hover:text-[var(--text)]'}"
      onclick={() => onselect(tab.id)}
    >
      {tab.label}
      {#if tab.count != null}
        <span
          class="text-micro tabular-nums font-normal
                 {active === tab.id
            ? 'text-[var(--text-muted)]'
            : 'text-[var(--text-faint)]'}"
        >
          {tab.count}
        </span>
      {/if}
    </button>
  {/each}
</div>
