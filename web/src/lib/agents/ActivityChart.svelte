<script lang="ts">
  // LIF-543: agent actions per bucket, stacked Read / Edit / Create / Close.
  // Drawn in pixel coordinates against the measured width, so the plot keeps
  // a fixed height at any width and its text never scales with it. A
  // positioned-div overlay (one invisible column per bucket) handles hover.

  import type { AgentActivityBucket, AgentWindow } from "../api";
  import {
    SERIES,
    WINDOW_PHRASE,
    bucketLabel,
    bucketRange,
    countTicks,
    labelIndices,
    totalOf,
  } from "./activity";

  let {
    buckets,
    win,
    bucketSeconds,
  }: {
    buckets: AgentActivityBucket[];
    win: AgentWindow;
    bucketSeconds: number;
  } = $props();

  const H = 236;
  const PAD_L = 34;
  const PAD_R = 4;
  const PAD_T = 8;
  const PAD_B = 28;
  const plotH = H - PAD_T - PAD_B;

  let width = $state(720);
  let plotW = $derived(Math.max(0, width - PAD_L - PAD_R));

  let n = $derived(buckets.length);
  let totals = $derived(buckets.map((b) => totalOf(b)));
  let domainMax = $derived(Math.max(0, ...totals));
  let ticks = $derived(countTicks(domainMax));
  let axisMax = $derived(ticks[ticks.length - 1] || 1);
  let quiet = $derived(domainMax === 0);

  let slot = $derived(n > 0 ? plotW / n : plotW);
  let barW = $derived(Math.max(4, Math.min(72, slot * 0.72)));

  function slotCenter(i: number): number {
    return PAD_L + slot * i + slot / 2;
  }
  function yAt(v: number): number {
    return PAD_T + plotH - (v / axisMax) * plotH;
  }

  /** Stacked segments for one bucket, bottom up, zero-height ones dropped. */
  function segments(b: AgentActivityBucket) {
    let acc = 0;
    const out: { key: string; color: string; y: number; h: number; top: boolean }[] = [];
    for (const s of SERIES) {
      const v = b[s.key];
      if (v <= 0) continue;
      const y0 = yAt(acc);
      acc += v;
      const y1 = yAt(acc);
      out.push({ key: s.key, color: s.color, y: y1, h: y0 - y1, top: false });
    }
    if (out.length) out[out.length - 1].top = true;
    return out;
  }

  let labelled = $derived(new Set(labelIndices(n, win === "24h" ? 8 : 12)));
  let hover = $state<number | null>(null);

  let summary = $derived(
    `Agent actions per ${win === "7d" ? "day" : win === "24h" ? "hour" : "5 minutes"}, ` +
      SERIES.map((s) => `${buckets.reduce((sum, b) => sum + b[s.key], 0)} ${s.label.toLowerCase()}`).join(", "),
  );
</script>

<div class="flex flex-wrap items-center gap-x-5 gap-y-1 mb-4">
  {#each SERIES as s (s.key)}
    <span class="flex items-center gap-2 text-[0.9rem] text-[var(--aa-text-2)]">
      <span class="size-3 rounded-[3px] shrink-0" style="background: {s.color}"></span>
      {s.label}
    </span>
  {/each}
</div>

<div class="relative select-none" bind:clientWidth={width} style="height: {H}px">
  <svg width={width} height={H} class="block" role="img" aria-label={summary}>
    {#each ticks as t (t)}
      <line
        x1={PAD_L}
        x2={width - PAD_R}
        y1={yAt(t) + 0.5}
        y2={yAt(t) + 0.5}
        stroke={t === 0 ? "var(--aa-axis)" : "var(--aa-grid)"}
        stroke-width="1"
      />
      <text x={PAD_L - 10} y={yAt(t) + 4} text-anchor="end" fill="var(--aa-text-2)" font-size="13">
        {t}
      </text>
    {/each}

    <line x1={PAD_L + 0.5} x2={PAD_L + 0.5} y1={PAD_T} y2={PAD_T + plotH} stroke="var(--aa-axis)" stroke-width="1" />

    {#each buckets as b, i (b.start)}
      {#if labelled.has(i)}
        <text x={slotCenter(i)} y={H - 6} text-anchor="middle" fill="var(--aa-text-2)" font-size="13">
          {bucketLabel(b.start, win)}
        </text>
      {/if}
    {/each}

    {#if !quiet}
      {#if hover !== null}
        <rect
          x={PAD_L + slot * hover + 1}
          y={PAD_T}
          width={Math.max(0, slot - 2)}
          height={plotH}
          rx="4"
          fill="var(--aa-hover)"
        />
      {/if}
      {#each buckets as b, i (b.start)}
        <g opacity={hover === null || hover === i ? 1 : 0.5}>
          {#each segments(b) as seg (seg.key)}
            <rect
              x={slotCenter(i) - barW / 2}
              y={seg.y}
              width={barW}
              height={Math.max(0, seg.h)}
              rx="0"
              fill={seg.color}
            />
          {/each}
        </g>
      {/each}
    {/if}
  </svg>

  {#if quiet}
    <div
      class="absolute flex items-center justify-center pointer-events-none"
      style="left: {PAD_L}px; right: {PAD_R}px; top: {PAD_T}px; height: {plotH}px;"
    >
      <p class="text-[0.9rem] text-[var(--aa-text-2)]">No agent activity in {WINDOW_PHRASE[win]}.</p>
    </div>
  {:else}
    <div class="absolute flex" style="left: {PAD_L}px; right: {PAD_R}px; top: 0; bottom: 0;">
      {#each buckets as b, i (b.start)}
        <div
          class="flex-1 h-full cursor-default"
          role="presentation"
          onmouseenter={() => (hover = i)}
          onmouseleave={() => (hover = null)}
        ></div>
      {/each}
    </div>

    {#if hover !== null && buckets[hover]}
      {@const b = buckets[hover]}
      {@const x = slotCenter(hover)}
      <div
        class="absolute z-10 pointer-events-none px-3 py-2 rounded-lg whitespace-nowrap
               bg-[var(--aa-pop)] border border-[var(--aa-line)] shadow-[0_8px_24px_rgba(0,0,0,0.28)]"
        style="top: 0; {x > width * 0.7
          ? `right: ${width - x + barW / 2 + 8}px`
          : `left: ${x + barW / 2 + 8}px`}"
      >
        <p class="text-[0.8rem] font-medium text-[var(--aa-text)] mb-1">
          {bucketRange(b.start, win, bucketSeconds, hover === n - 1)}
        </p>
        {#each [...SERIES].reverse() as s (s.key)}
          <p class="flex items-center gap-2 text-[0.8rem] text-[var(--aa-text-2)] m-0 leading-6">
            <span class="size-2.5 rounded-[2px] shrink-0" style="background: {s.color}"></span>
            {s.label}
            <span class="ml-auto pl-6 tabular-nums text-[var(--aa-text)]">{b[s.key]}</span>
          </p>
        {/each}
        <p class="flex items-center gap-2 text-[0.8rem] text-[var(--aa-text-2)] m-0 mt-1 pt-1 border-t border-[var(--aa-line)] leading-6">
          Total
          <span class="ml-auto pl-6 tabular-nums font-semibold text-[var(--aa-text)]">{totals[hover]}</span>
        </p>
      </div>
    {/if}
  {/if}
</div>
