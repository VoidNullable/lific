<script lang="ts">
  // Every issue in the project as one bar, split by status, with a legend
  // that opens the issue list filtered to the status it names.
  import type { IssueStatusCounts } from "../api";
  import StatusIcon, { statusCssColor } from "../StatusIcon.svelte";
  import { STATUSES, statusLabel } from "../issues/grouping";

  let {
    counts,
    onpick,
  }: {
    counts: IssueStatusCounts;
    onpick: (status: string) => void;
  } = $props();

  let hovered = $state<string | null>(null);
  let total = $derived(counts.total);
  const count = (s: string) => (counts as unknown as Record<string, number>)[s] ?? 0;
</script>

<div class="flex flex-col gap-3">
  <div
    class="flex h-2.5 w-full overflow-hidden rounded-full bg-[var(--chart-tile)] gap-[2px]"
    role="img"
    aria-label={STATUSES.map((s) => `${statusLabel(s)} ${count(s)}`).join(", ")}
  >
    {#each STATUSES as s (s)}
      {#if count(s) > 0}
        <div
          class="strip-seg h-full"
          style="flex-grow: {count(s)}; background: {statusCssColor(s)};
                 opacity: {hovered && hovered !== s ? 0.3 : s === 'cancelled' ? 0.45 : 1}"
        ></div>
      {/if}
    {/each}
  </div>
  <div class="flex flex-wrap items-center gap-x-1 gap-y-1 -ml-2">
    {#each STATUSES as s (s)}
      {@const n = count(s)}
      <button
        class="group flex items-center gap-1.5 px-2 py-1 rounded-md text-body-sm transition-colors
               hover:bg-[var(--chart-hover)] disabled:opacity-50 disabled:hover:bg-transparent"
        disabled={n === 0}
        onmouseenter={() => (hovered = s)}
        onmouseleave={() => (hovered = null)}
        onfocus={() => (hovered = s)}
        onblur={() => (hovered = null)}
        onclick={() => onpick(s)}
        aria-label={`${statusLabel(s)}: ${n} ${n === 1 ? "issue" : "issues"}. Show them in the issue list`}
      >
        <StatusIcon status={s} size={13} />
        <span class="text-[var(--text-muted)] group-hover:text-[var(--text)] transition-colors">{statusLabel(s)}</span>
        <span class="tabular-nums text-[var(--text)] font-medium">{n}</span>
        {#if total > 0 && n > 0}
          <span class="tabular-nums text-caption text-[var(--text-faint)]">{Math.round((n / total) * 100)}%</span>
        {/if}
      </button>
    {/each}
  </div>
</div>

<style>
  .strip-seg {
    transition:
      flex-grow 420ms var(--ease-out-expo, cubic-bezier(0.16, 1, 0.3, 1)),
      opacity 160ms ease;
  }
  @media (prefers-reduced-motion: reduce) {
    .strip-seg {
      transition: none;
    }
  }
</style>
