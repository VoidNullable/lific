<script lang="ts">
  // LIF-507: open work by how long it has been open. Each bucket stacks its
  // issues by priority; the slim green rail beside it is the share an agent
  // can pick up right now (unassigned, unblocked, not already active).
  import type { AgeBucket, AgeBucketKey } from "../api";

  let { buckets }: { buckets: AgeBucket[] } = $props();

  const LABEL: Record<AgeBucketKey, string> = {
    week: "This week",
    month: "1 to 4 weeks",
    quarter: "1 to 3 months",
    half: "3 to 6 months",
    older: "6 months+",
  };
  const SHORT: Record<AgeBucketKey, string> = {
    week: "< 1 wk",
    month: "1-4 wk",
    quarter: "1-3 mo",
    half: "3-6 mo",
    older: "6 mo+",
  };

  const SEGMENTS = [
    { key: "urgent", label: "Urgent", color: "var(--error)" },
    { key: "high", label: "High", color: "var(--warn)" },
    { key: "medium", label: "Medium", color: "color-mix(in oklab, var(--text-muted) 60%, transparent)" },
    { key: "low", label: "Low", color: "color-mix(in oklab, var(--text-faint) 38%, transparent)" },
    { key: "none", label: "No priority", color: "color-mix(in oklab, var(--text-faint) 20%, transparent)" },
  ] as const;

  const PLOT_H = 136;

  let max = $derived(Math.max(1, ...buckets.map((b) => b.total)));
  let hover = $state<number | null>(null);

  function px(n: number): number {
    return n === 0 ? 0 : Math.max(2, Math.round((n / max) * PLOT_H));
  }
</script>

<div class="flex items-end gap-3 sm:gap-5 pt-6" role="img" aria-label="Open issues by age and priority">
  {#each buckets as bucket, i (bucket.key)}
    <div
      class="relative flex-1 min-w-0 flex flex-col items-center"
      role="presentation"
      onmouseenter={() => (hover = i)}
      onmouseleave={() => (hover = null)}
    >
      {#if hover === i && bucket.total > 0}
        <div
          class="absolute bottom-full mb-2 z-10 pointer-events-none px-2.5 py-2 rounded-md whitespace-nowrap
                 bg-[var(--surface)] border border-[var(--border)] shadow-[0_4px_12px_rgba(0,0,0,0.18)]"
        >
          <p class="text-caption font-medium text-[var(--text)] mb-1">{LABEL[bucket.key]}</p>
          {#each SEGMENTS as seg (seg.key)}
            {#if bucket[seg.key] > 0}
              <p class="flex items-center gap-1.5 text-micro text-[var(--text-muted)] m-0">
                <span class="size-2 rounded-sm shrink-0" style="background: {seg.color}"></span>
                {seg.label}
                <span class="ml-auto pl-3 tabular-nums text-[var(--text)]">{bucket[seg.key]}</span>
              </p>
            {/if}
          {/each}
          <p class="flex items-center gap-1.5 text-micro text-[var(--text-muted)] m-0 mt-1 pt-1 border-t border-[var(--border)]">
            <span class="size-2 rounded-sm shrink-0 bg-[var(--success)]"></span>
            Ready for agents
            <span class="ml-auto pl-3 tabular-nums text-[var(--text)]">{bucket.agent_ready}</span>
          </p>
          <p class="flex items-center gap-1.5 text-micro text-[var(--text-muted)] m-0">
            <span class="size-2 shrink-0"></span>
            Needs a person
            <span class="ml-auto pl-3 tabular-nums text-[var(--text)]">{bucket.needs_human}</span>
          </p>
        </div>
      {/if}

      <span class="text-caption tabular-nums text-[var(--text-muted)] mb-1.5">{bucket.total}</span>
      <div class="flex items-end gap-1 w-full max-w-[72px]" style="height: {PLOT_H}px">
        <div class="flex-1 flex flex-col-reverse gap-px rounded-md overflow-hidden {bucket.total === 0 ? 'h-0.5 bg-[var(--border)]' : ''}">
          {#each SEGMENTS as seg (seg.key)}
            {#if bucket[seg.key] > 0}
              <span class="block w-full" style="height: {px(bucket[seg.key])}px; background: {seg.color}"></span>
            {/if}
          {/each}
        </div>
        <span
          class="w-1 rounded-full bg-[var(--success)] shrink-0"
          style="height: {px(bucket.agent_ready)}px"
          title="{bucket.agent_ready} ready for agents"
        ></span>
      </div>
      <span class="mt-2 text-caption text-[var(--text)] text-center truncate max-w-full">
        <span class="sm:hidden">{SHORT[bucket.key]}</span>
        <span class="hidden sm:inline">{LABEL[bucket.key]}</span>
      </span>
      <span class="text-micro text-[var(--success)] tabular-nums whitespace-nowrap">
        {bucket.agent_ready}<span class="hidden sm:inline">{" for agents"}</span>
      </span>
    </div>
  {/each}
</div>

<div class="flex flex-wrap items-center gap-x-4 gap-y-1 mt-4">
  {#each SEGMENTS.slice(0, 3) as seg (seg.key)}
    <span class="flex items-center gap-1.5 text-micro text-[var(--text-muted)]">
      <span class="size-2 rounded-sm" style="background: {seg.color}"></span>{seg.label}
    </span>
  {/each}
  <span class="flex items-center gap-1.5 text-micro text-[var(--text-muted)]">
    <span class="size-2 rounded-sm" style="background: {SEGMENTS[3].color}"></span>Low or none
  </span>
  <span class="flex items-center gap-1.5 text-micro text-[var(--text-muted)]">
    <span class="w-1 h-2.5 rounded-full bg-[var(--success)]"></span>Ready for agents
  </span>
</div>
