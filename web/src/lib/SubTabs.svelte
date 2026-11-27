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
    border = true,
  }: {
    tabs: SubTab[];
    active: string;
    onselect: (id: string) => void;
    /** Optional control pinned to the strip's right edge (e.g. the issue
     *  list's select-all toggle). Rendered outside the tablist semantics so
     *  it is not announced as a tab. */
    trailing?: import("svelte").Snippet;
    /** Draw the strip's own bottom rule. Off when a parent row already
     *  draws one and the tabs sit on it. */
    border?: boolean;
  } = $props();

  // With the scrollbar hidden, a strip wider than its column would just cut
  // the last tab off. Fade whichever edge has more tabs past it instead.
  let strip = $state<HTMLDivElement | null>(null);
  let fadeStart = $state(false);
  let fadeEnd = $state(false);
  function measure() {
    if (!strip) return;
    fadeStart = strip.scrollLeft > 1;
    fadeEnd = strip.scrollLeft + strip.clientWidth < strip.scrollWidth - 1;
  }
  $effect(() => {
    if (!strip) return;
    const ro = new ResizeObserver(measure);
    ro.observe(strip);
    return () => ro.disconnect();
  });
  // Keep the chosen tab in view, also when it is picked from elsewhere.
  $effect(() => {
    const id = active;
    if (!strip) return;
    const tab = strip.querySelector<HTMLElement>(`[data-tab-id="${CSS.escape(id)}"]`);
    if (!tab) return;
    const left = tab.getBoundingClientRect().left - strip.getBoundingClientRect().left + strip.scrollLeft;
    if (left < strip.scrollLeft) strip.scrollTo({ left: left - 24 });
    else if (left + tab.offsetWidth > strip.scrollLeft + strip.clientWidth)
      strip.scrollTo({ left: left + tab.offsetWidth - strip.clientWidth + 24 });
    measure();
  });
  const FADE = "2.5rem";
  const mask = $derived(
    fadeStart || fadeEnd
      ? `linear-gradient(to right, ${fadeStart ? `transparent, #000 ${FADE}` : "#000"}, ${fadeEnd ? `#000 calc(100% - ${FADE}), transparent` : "#000"})`
      : "",
  );
</script>

<!-- The scroller's `-mb-px` lets the active underline sit on the strip's
     bottom rule (or the parent's, with border off) without the tabs
     overflowing the scroller, which would reserve a scrollbar gutter.
     Scrollbars stay hidden; the strip pans by touch/wheel. The trailing
     control sits outside the scroller so it never scrolls or fades away. -->
<div class="min-w-0 flex items-center {border ? 'border-b border-[var(--border)]' : ''}">
  <div
    bind:this={strip}
    onscroll={measure}
    class="-mb-px min-w-0 flex-1 overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
    style:mask-image={mask || null}
    style:-webkit-mask-image={mask || null}
  >
  <div class="flex items-center gap-5 w-max" role="tablist">
  {#each tabs as tab (tab.id)}
    <button
      role="tab"
      data-tab-id={tab.id}
      aria-selected={active === tab.id}
      class="relative shrink-0 flex items-baseline gap-1.5 px-0.5 pb-2 pt-1
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
