<script lang="ts">
  // LIF-507: a small, honest flex. Issues you or your agents moved to done,
  // one square per day, weeks as columns starting on Monday.
  import type { DayCount } from "../api";

  let { days }: { days: DayCount[] } = $props();

  function weekday(iso: string): number {
    // Monday = 0.
    return (new Date(`${iso}T00:00:00`).getDay() + 6) % 7;
  }

  let columns = $derived.by(() => {
    if (days.length === 0) return [];
    const cells: (DayCount | null)[] = [...Array(weekday(days[0].date)).fill(null), ...days];
    const cols: (DayCount | null)[][] = [];
    for (let i = 0; i < cells.length; i += 7) cols.push(cells.slice(i, i + 7));
    return cols;
  });

  let thisMonth = $derived.by(() => {
    const last = days[days.length - 1]?.date.slice(0, 7);
    return days.filter((d) => d.date.startsWith(last ?? "")).reduce((n, d) => n + d.count, 0);
  });

  function fill(count: number): string {
    if (count === 0) return "color-mix(in oklab, var(--text-faint) 14%, transparent)";
    if (count === 1) return "color-mix(in oklab, var(--success) 35%, transparent)";
    if (count <= 3) return "color-mix(in oklab, var(--success) 60%, transparent)";
    if (count <= 6) return "color-mix(in oklab, var(--success) 82%, transparent)";
    return "var(--success)";
  }

  function label(d: DayCount): string {
    const date = new Date(`${d.date}T00:00:00`).toLocaleDateString("en-US", {
      weekday: "short",
      month: "short",
      day: "numeric",
    });
    return `${date}: ${d.count} done`;
  }
</script>

<div>
  <div class="flex items-baseline justify-between gap-2 mb-2.5">
    <h2 class="text-body-sm font-semibold text-[var(--text)]">Your last 12 weeks</h2>
    <span class="text-micro text-[var(--text-faint)] tabular-nums">{thisMonth} done this month</span>
  </div>
  <div class="flex gap-[3px]" role="img" aria-label="Issues you and your agents finished each day">
    {#each columns as col, ci (ci)}
      <div class="flex-1 flex flex-col gap-[3px]">
        {#each col as cell, ri (ri)}
          {#if cell}
            <span class="block aspect-square rounded-[2px]" style="background: {fill(cell.count)}" title={label(cell)}></span>
          {:else}
            <span class="block aspect-square"></span>
          {/if}
        {/each}
      </div>
    {/each}
  </div>
</div>
