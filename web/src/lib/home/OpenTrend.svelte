<script lang="ts">
  // LIF-507: open issue count at the end of each day. In a tracker that
  // captures every idea a rising count is normal, so the chart only answers
  // "is the pile growing or shrinking". The axis spans the data's own range
  // and says so with its labels, which keeps small moves visible.
  import type { DayCount } from "../api";

  let { points }: { points: DayCount[] } = $props();

  const H = 232;
  const PAD_L = 38;
  const PAD_R = 6;
  const PAD_T = 8;
  const PAD_B = 24;
  const plotH = H - PAD_T - PAD_B;

  let width = $state(360);
  let plotW = $derived(Math.max(0, width - PAD_L - PAD_R));

  /** Three or four round gridline values covering [lo, hi]. */
  function rangeTicks(lo: number, hi: number): number[] {
    const span = Math.max(1, hi - lo);
    const raw = span / 3;
    const mag = 10 ** Math.floor(Math.log10(raw));
    const step = Math.max(1, [1, 2, 5, 10].map((m) => m * mag).find((s) => s >= raw) ?? 10 * mag);
    const start = Math.floor(lo / step) * step;
    const end = Math.max(start + step, Math.ceil(hi / step) * step);
    const out: number[] = [];
    for (let v = start; v <= end; v += step) out.push(v);
    return out;
  }

  let ticks = $derived(
    rangeTicks(Math.min(...points.map((p) => p.count)), Math.max(...points.map((p) => p.count))),
  );
  let lo = $derived(ticks[0]);
  let hi = $derived(ticks[ticks.length - 1]);

  function xAt(i: number): number {
    return PAD_L + (points.length <= 1 ? plotW / 2 : (plotW * i) / (points.length - 1));
  }
  function yAt(v: number): number {
    return PAD_T + plotH - ((v - lo) / Math.max(1, hi - lo)) * plotH;
  }

  let line = $derived(points.map((p, i) => `${i === 0 ? "M" : "L"}${xAt(i).toFixed(1)} ${yAt(p.count).toFixed(1)}`).join(" "));
  let area = $derived(
    points.length > 1 ? `${line} L${xAt(points.length - 1)} ${yAt(lo)} L${xAt(0)} ${yAt(lo)} Z` : "",
  );

  function label(iso: string | undefined): string {
    if (!iso) return "";
    return new Date(`${iso}T00:00:00`).toLocaleDateString("en-US", { month: "short", day: "numeric" });
  }

  let hover = $state<number | null>(null);

  function onMove(e: PointerEvent) {
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const t = (e.clientX - rect.left - PAD_L) / Math.max(1, plotW);
    hover = Math.min(points.length - 1, Math.max(0, Math.round(t * (points.length - 1))));
  }
</script>

{#if points.length > 1}
  <div
    class="relative select-none"
    role="presentation"
    bind:clientWidth={width}
    style="height: {H}px"
    onpointermove={onMove}
    onpointerleave={() => (hover = null)}
  >
    <svg width={width} height={H} class="block" role="img" aria-label="Open issues over the last 90 days, {points[0].count} to {points[points.length - 1].count}">
      {#each ticks as t (t)}
        <line x1={PAD_L} x2={width - PAD_R} y1={yAt(t) + 0.5} y2={yAt(t) + 0.5}
          stroke={t === lo ? "var(--chart-axis)" : "var(--chart-grid)"} stroke-width="1" />
        <text x={PAD_L - 10} y={yAt(t) + 4} text-anchor="end" fill="var(--text-muted)" font-size="12">{t}</text>
      {/each}
      <path d={area} fill="var(--success)" opacity="0.12" />
      <path d={line} fill="none" stroke="var(--success)" stroke-width="2" stroke-linejoin="round" />
      {#if hover !== null}
        <line x1={xAt(hover)} x2={xAt(hover)} y1={PAD_T} y2={PAD_T + plotH} stroke="var(--chart-axis)" />
        <rect x={xAt(hover) - 3.5} y={yAt(points[hover].count) - 3.5} width="7" height="7" fill="var(--success)" />
      {/if}
      <text x={PAD_L} y={H - 6} fill="var(--text-muted)" font-size="12">{label(points[0]?.date)}</text>
      <text x={width - PAD_R} y={H - 6} text-anchor="end" fill="var(--text-muted)" font-size="12">Today</text>
    </svg>
    {#if hover !== null}
      {@const x = xAt(hover)}
      <div
        class="absolute top-0 z-10 pointer-events-none px-2.5 py-1.5 rounded-lg whitespace-nowrap text-caption
               bg-[var(--surface)] border border-[var(--border)] shadow-[0_8px_24px_rgba(0,0,0,0.28)]"
        style={x > width * 0.6 ? `right: ${width - x + 10}px` : `left: ${x + 10}px`}
      >
        <span class="text-[var(--text-muted)]">{label(points[hover].date)}</span>
        <span class="ml-1.5 font-medium tabular-nums text-[var(--text)]">{points[hover].count} open</span>
      </div>
    {/if}
  </div>
{/if}
