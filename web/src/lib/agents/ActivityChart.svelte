<script lang="ts">
  // LIF-543: agent actions per bucket, stacked Read / Edit / Create / Close.
  // Hand-rolled SVG like insights/TrendChart.svelte: a responsive viewBox
  // for the drawing and a positioned-div overlay (one invisible column per
  // bucket) for hover, with a small floating card for the counts.

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

  const VB_W = 680;
  const VB_H = 200;
  const PAD_L = 30;
  const PAD_R = 6;
  const PAD_T = 10;
  const PAD_B = 22;
  const plotW = VB_W - PAD_L - PAD_R;
  const plotH = VB_H - PAD_T - PAD_B;

  let n = $derived(buckets.length);
  let totals = $derived(buckets.map((b) => totalOf(b)));
  let domainMax = $derived(Math.max(0, ...totals));
  let ticks = $derived(countTicks(domainMax));
  let axisMax = $derived(ticks[ticks.length - 1] || 1);
  let quiet = $derived(domainMax === 0);

  let slot = $derived(n > 0 ? plotW / n : plotW);
  let barW = $derived(Math.min(44, slot * 0.62));

  function slotCenter(i: number): number {
    return PAD_L + slot * i + slot / 2;
  }
  function yAt(v: number): number {
    return PAD_T + plotH - (v / axisMax) * plotH;
  }

  /** Stacked segments for one bucket, bottom up, zero-height ones dropped. */
  function segments(b: AgentActivityBucket) {
    let acc = 0;
    const out: { key: string; color: string; y: number; h: number }[] = [];
    for (const s of SERIES) {
      const v = b[s.key];
      if (v <= 0) continue;
      const y0 = yAt(acc);
      acc += v;
      const y1 = yAt(acc);
      out.push({ key: s.key, color: s.color, y: y1, h: y0 - y1 });
    }
    return out;
  }

  let labelled = $derived(new Set(labelIndices(n, win === "7d" ? 7 : 8)));
  let hover = $state<number | null>(null);

  let summary = $derived(
    `Agent actions per ${win === "7d" ? "day" : win === "24h" ? "hour" : "5 minutes"}, ` +
      SERIES.map((s) => `${buckets.reduce((sum, b) => sum + b[s.key], 0)} ${s.label.toLowerCase()}`).join(", "),
  );
</script>

<div class="flex flex-wrap items-center gap-x-4 gap-y-1 mb-3">
  {#each SERIES as s (s.key)}
    <span class="flex items-center gap-1.5 text-caption text-[var(--text-muted)]">
      <span class="size-2.5 rounded-[3px] shrink-0" style="background: {s.color}"></span>
      {s.label}
    </span>
  {/each}
</div>

<div class="relative select-none">
  <svg viewBox="0 0 {VB_W} {VB_H}" class="w-full h-auto block" role="img" aria-label={summary}>
    {#each ticks as t (t)}
      <line
        x1={PAD_L}
        x2={VB_W - PAD_R}
        y1={yAt(t)}
        y2={yAt(t)}
        stroke="var(--border)"
        stroke-width="1"
        stroke-dasharray={t === 0 ? undefined : "2 3"}
      />
      <text x={PAD_L - 6} y={yAt(t) + 3} text-anchor="end" class="fill-[var(--text-faint)]" font-size="9">
        {t}
      </text>
    {/each}

    {#each buckets as b, i (b.start)}
      {#if labelled.has(i)}
        <text
          x={slotCenter(i)}
          y={VB_H - 6}
          text-anchor="middle"
          class="fill-[var(--text-faint)]"
          font-size="9"
        >
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
          rx="3"
          fill="var(--bg-subtle)"
          opacity="0.7"
        />
      {/if}
      {#each buckets as b, i (b.start)}
        <g opacity={hover === null || hover === i ? 1 : 0.55}>
          {#each segments(b) as seg (seg.key)}
            <rect
              x={slotCenter(i) - barW / 2}
              y={seg.y}
              width={barW}
              height={Math.max(0, seg.h - (seg.h > 2 ? 1 : 0))}
              rx="1.5"
              fill={seg.color}
            />
          {/each}
        </g>
      {/each}
    {/if}
  </svg>

  {#if quiet}
    <div
      class="absolute inset-0 flex items-center justify-center pointer-events-none"
      style="left: {(PAD_L / VB_W) * 100}%; bottom: {(PAD_B / VB_H) * 100}%;"
    >
      <p class="text-body-sm text-[var(--text-muted)] bg-[var(--surface)] px-3 py-1 rounded-md">
        No agent activity in {WINDOW_PHRASE[win]}.
      </p>
    </div>
  {:else}
    <div
      class="absolute inset-0 flex"
      style="left: {(PAD_L / VB_W) * 100}%; right: {(PAD_R / VB_W) * 100}%;"
    >
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
      <div
        class="absolute z-10 pointer-events-none px-2.5 py-2 rounded-md whitespace-nowrap
               bg-[var(--surface)] border border-[var(--border)]
               shadow-[0_4px_12px_rgba(0,0,0,0.18)] -translate-x-1/2"
        style="left: {Math.min(88, Math.max(12, (slotCenter(hover) / VB_W) * 100))}%; top: 2px;"
      >
        <p class="text-caption font-medium text-[var(--text)] mb-1">
          {bucketRange(b.start, win, bucketSeconds, hover === n - 1)}
        </p>
        {#each [...SERIES].reverse() as s (s.key)}
          <p class="flex items-center gap-1.5 text-micro text-[var(--text-muted)] m-0">
            <span class="size-2 rounded-sm shrink-0" style="background: {s.color}"></span>
            {s.label}
            <span class="ml-auto pl-4 tabular-nums text-[var(--text)]">{b[s.key]}</span>
          </p>
        {/each}
        <p class="flex items-center gap-1.5 text-micro text-[var(--text-muted)] m-0 mt-1 pt-1 border-t border-[var(--border)]">
          <span class="size-2 shrink-0"></span>
          Total
          <span class="ml-auto pl-4 tabular-nums font-medium text-[var(--text)]">{totals[hover]}</span>
        </p>
      </div>
    {/if}
  {/if}
</div>
