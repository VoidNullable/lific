<script lang="ts">
  // A small column chart of issues per week, for the overview's pulse
  // numbers. The latest week is drawn solid, earlier ones lighter, and each
  // column names its week and count on hover.
  import type { WeekPoint } from "../api";

  let {
    points,
    label,
    color = "var(--success)",
  }: { points: WeekPoint[]; label: string; color?: string } = $props();

  let max = $derived(Math.max(1, ...points.map((p) => p.count)));

  function week(start: string): string {
    const d = new Date(`${start}T00:00:00`);
    return Number.isNaN(d.getTime())
      ? start
      : `Week of ${d.toLocaleDateString("en-US", { month: "short", day: "numeric" })}`;
  }
</script>

<div class="flex items-end gap-[3px] h-7" role="img" aria-label={label}>
  {#each points as p, i (p.week_start)}
    <span
      class="flex-1 min-w-[3px] rounded-[2px] week-bar"
      style="height: {p.count === 0 ? 2 : Math.max(3, (p.count / max) * 28)}px;
             background: {p.count === 0 ? 'var(--chart-tile)' : color};
             opacity: {i === points.length - 1 ? 1 : 0.45}"
      title="{week(p.week_start)}: {p.count}"
    ></span>
  {/each}
</div>

<style>
  .week-bar {
    transition: height 380ms var(--ease-out-expo, cubic-bezier(0.16, 1, 0.3, 1));
  }
  @media (prefers-reduced-motion: reduce) {
    .week-bar {
      transition: none;
    }
  }
</style>
