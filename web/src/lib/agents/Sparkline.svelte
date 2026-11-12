<script lang="ts">
  // LIF-543: tiny per-agent trend of total actions per bucket. Straight
  // segments rather than the smoothed curve in insights/curve.ts, because a
  // spline overshoots below zero on spiky counts. An all-zero series draws
  // a flat baseline in the border color.

  let {
    series,
    label,
    class: className = "",
  }: { series: number[]; label: string; class?: string } = $props();

  const W = 120;
  const H = 32;
  const PAD = 2;

  let max = $derived(Math.max(0, ...series));
  let points = $derived.by(() => {
    const n = series.length;
    if (n === 0) return [];
    return series.map((v, i) => ({
      x: n === 1 ? W / 2 : PAD + ((W - PAD * 2) * i) / (n - 1),
      y: max === 0 ? H - PAD : H - PAD - (v / max) * (H - PAD * 2),
    }));
  });
  let line = $derived(points.map((p, i) => `${i === 0 ? "M" : "L"} ${p.x} ${p.y}`).join(" "));
  let area = $derived(
    points.length > 1
      ? `${line} L ${points[points.length - 1].x} ${H - PAD} L ${points[0].x} ${H - PAD} Z`
      : "",
  );
</script>

<svg
  viewBox="0 0 {W} {H}"
  preserveAspectRatio="none"
  class="block {className}"
  role="img"
  aria-label={label}
>
  {#if max > 0}
    <path d={area} fill="var(--success)" opacity="0.14" />
    <path
      d={line}
      fill="none"
      stroke="var(--success)"
      stroke-width="1.5"
      stroke-linejoin="round"
      stroke-linecap="round"
      vector-effect="non-scaling-stroke"
    />
  {:else}
    <line
      x1={PAD}
      x2={W - PAD}
      y1={H - PAD}
      y2={H - PAD}
      stroke="var(--border)"
      stroke-width="1.5"
      vector-effect="non-scaling-stroke"
    />
  {/if}
</svg>
