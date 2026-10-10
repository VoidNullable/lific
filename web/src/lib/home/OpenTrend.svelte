<script lang="ts">
  // LIF-507: open issue count at the end of each day. A gentle line, not a
  // scoreboard: in a tracker that captures every idea, a rising count is
  // normal, so the chart only answers "is the pile growing or shrinking".
  import type { DayCount } from "../api";
  import { smoothAreaPath, smoothPath, type Point } from "../insights/curve";

  let { points }: { points: DayCount[] } = $props();

  const VB_W = 300;
  const VB_H = 72;
  const PAD_Y = 6;

  let lo = $derived(Math.min(...points.map((p) => p.count)));
  let hi = $derived(Math.max(...points.map((p) => p.count)));

  let pts = $derived<Point[]>(
    points.map((p, i) => {
      const span = Math.max(1, hi - lo);
      return {
        x: points.length <= 1 ? VB_W / 2 : (VB_W * i) / (points.length - 1),
        y: PAD_Y + (VB_H - 2 * PAD_Y) * (1 - (p.count - lo) / span),
      };
    }),
  );

  function label(iso: string | undefined): string {
    if (!iso) return "";
    return new Date(`${iso}T00:00:00`).toLocaleDateString("en-US", { month: "short", day: "numeric" });
  }

  let hover = $state<number | null>(null);

  function onMove(e: PointerEvent) {
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const t = (e.clientX - rect.left) / rect.width;
    hover = Math.min(points.length - 1, Math.max(0, Math.round(t * (points.length - 1))));
  }
</script>

{#if points.length > 1}
  <div
    class="relative"
    role="presentation"
    onpointermove={onMove}
    onpointerleave={() => (hover = null)}
  >
    <svg viewBox="0 0 {VB_W} {VB_H}" preserveAspectRatio="none" class="w-full h-[72px] block" role="img" aria-label="Open issues over the last 90 days">
      <path d={smoothAreaPath(pts, VB_H)} fill="var(--success)" opacity="0.1" />
      <path
        d={smoothPath(pts)}
        fill="none"
        stroke="var(--success)"
        stroke-width="1.75"
        vector-effect="non-scaling-stroke"
        stroke-linecap="round"
      />
      {#if hover !== null}
        <line x1={pts[hover].x} x2={pts[hover].x} y1="0" y2={VB_H} stroke="var(--text-faint)" stroke-dasharray="2 3" vector-effect="non-scaling-stroke" />
      {/if}
    </svg>
    {#if hover !== null}
      <div
        class="absolute -top-1 -translate-y-full -translate-x-1/2 pointer-events-none px-2 py-1 rounded-md whitespace-nowrap
               bg-[var(--surface)] border border-[var(--border)] shadow-[0_4px_12px_rgba(0,0,0,0.18)] text-micro"
        style="left: {Math.min(90, Math.max(10, (pts[hover].x / VB_W) * 100))}%"
      >
        <span class="text-[var(--text-muted)]">{label(points[hover].date)}</span>
        <span class="ml-1.5 font-medium tabular-nums text-[var(--text)]">{points[hover].count} open</span>
      </div>
    {/if}
  </div>
  <div class="flex justify-between mt-1 text-micro text-[var(--text-faint)]">
    <span>{label(points[0]?.date)}</span>
    <span>Today</span>
  </div>
{/if}
