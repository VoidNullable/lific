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
    trailing,
  }: {
    tabs: SubTab[];
    active: string;
    onselect: (id: string) => void;
    /** Optional control pinned to the strip's right edge (e.g. the issue
     *  list's select-all toggle). Rendered outside the tablist semantics so
     *  it is not announced as a tab. */
    trailing?: import("svelte").Snippet;
  } = $props();
</script>

<!-- The tabs' `-mb-px` overlap makes this scroll container overflow by 1px
     vertically, which would otherwise reserve a 10px scrollbar gutter along
     the right edge (and misalign anything pinned there). Hide the strip's
     scrollbars; it still pans horizontally by touch/wheel. -->
<div
  class="flex items-center gap-5 border-b border-[var(--border)] overflow-x-auto
         [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
>
  <div class="flex items-center gap-5 shrink-0" role="tablist">
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
  {#if trailing}
    <!-- Tabs sit on pt-1/pb-2 above a 2px underline, so their text centre
         is ~2px above the row's centre. mb-1 nudges the trailing control up
         to meet it instead of hanging visibly low. -->
    <div class="ml-auto shrink-0 mb-1 pl-3">
      {@render trailing()}
    </div>
  {/if}
</div>
