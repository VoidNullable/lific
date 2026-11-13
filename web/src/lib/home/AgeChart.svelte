<script lang="ts">
  // LIF-507: open work by how long it has been open. Each bucket stacks its
  // issues by priority; the slim green bar beside it is the share an agent
  // can pick up right now (unassigned, unblocked, not already active).
  // Drawn like the agent activity chart: pixel coordinates against the
  // measured width, square bars, a real count axis, one tooltip on hover.
  import type { AgeBucket, AgeBucketKey } from "../api";
  import { countTicks } from "../agents/activity";

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

  // Low and no priority share a color: apart they are two greys nobody can
  // tell apart.
  const SEGMENTS = [
    { key: "urgent", label: "Urgent", color: "var(--error)" },
    { key: "high", label: "High", color: "var(--warn)" },
    { key: "medium", label: "Medium", color: "color-mix(in oklab, var(--text-muted) 70%, transparent)" },
    { key: "low", label: "Low or none", color: "color-mix(in oklab, var(--text-faint) 42%, transparent)" },
  ] as const;
  const READY = "var(--success)";

  function value(b: AgeBucket, key: (typeof SEGMENTS)[number]["key"]): number {
    return key === "low" ? b.low + b.none : b[key];
  }

  const H = 264;
  const PAD_L = 38;
  const PAD_R = 4;
  const PAD_T = 22;
  const PAD_B = 46;
  const plotH = H - PAD_T - PAD_B;

  let width = $state(640);
  let plotW = $derived(Math.max(0, width - PAD_L - PAD_R));
  let n = $derived(buckets.length);
  let slot = $derived(n > 0 ? plotW / n : plotW);
  let barW = $derived(Math.max(10, Math.min(64, slot * 0.46)));
  let readyW = $derived(Math.max(4, Math.round(barW * 0.18)));
  let groupW = $derived(barW + 3 + readyW);
  let short = $derived(slot < 104);

  let ticks = $derived(countTicks(Math.max(0, ...buckets.map((b) => b.total))));
  let axisMax = $derived(ticks[ticks.length - 1] || 1);
  let quiet = $derived(buckets.every((b) => b.total === 0));

  function yAt(v: number): number {
    return PAD_T + plotH - (v / axisMax) * plotH;
  }
  function left(i: number): number {
    return PAD_L + slot * i + (slot - groupW) / 2;
  }

  function segments(b: AgeBucket) {
    let acc = 0;
    const out: { key: string; color: string; y: number; h: number }[] = [];
    for (const s of SEGMENTS) {
      const v = value(b, s.key);
      if (v <= 0) continue;
      const y0 = yAt(acc);
      acc += v;
      const y1 = yAt(acc);
      out.push({ key: s.key, color: s.color, y: y1, h: y0 - y1 });
    }
    return out;
  }

  let hover = $state<number | null>(null);

  let summary = $derived(
    "Open issues by age: " +
      buckets.map((b) => `${LABEL[b.key]} ${b.total}, ${b.agent_ready} ready for agents`).join("; "),
  );
</script>

<div class="flex flex-wrap items-center gap-x-5 gap-y-1 mb-3">
  {#each SEGMENTS as s (s.key)}
    <span class="flex items-center gap-2 text-caption text-[var(--text-muted)]">
      <span class="size-3 shrink-0" style="background: {s.color}"></span>
      {s.label}
    </span>
  {/each}
  <span class="flex items-center gap-2 text-caption text-[var(--text-muted)]">
    <span class="w-1.5 h-3 shrink-0" style="background: {READY}"></span>
    Ready for agents
  </span>
</div>

<div class="relative select-none" bind:clientWidth={width} style="height: {H}px">
  <svg width={width} height={H} class="block" role="img" aria-label={summary}>
    {#each ticks as t (t)}
      <line
        x1={PAD_L}
        x2={width - PAD_R}
        y1={yAt(t) + 0.5}
        y2={yAt(t) + 0.5}
        stroke={t === 0 ? "var(--chart-axis)" : "var(--chart-grid)"}
        stroke-width="1"
      />
      <text x={PAD_L - 10} y={yAt(t) + 4} text-anchor="end" fill="var(--text-muted)" font-size="12">{t}</text>
    {/each}

    {#if hover !== null && !quiet}
      <rect x={PAD_L + slot * hover + 1} y={PAD_T - 20} width={Math.max(0, slot - 2)} height={plotH + 20} fill="var(--chart-hover)" />
    {/if}

    {#each buckets as b, i (b.key)}
      {@const x = left(i)}
      <g opacity={hover === null || hover === i ? 1 : 0.55}>
        {#each segments(b) as seg (seg.key)}
          <rect x={x} y={seg.y} width={barW} height={Math.max(0, seg.h)} fill={seg.color} />
        {/each}
        {#if b.agent_ready > 0}
          <rect
            x={x + barW + 3}
            y={yAt(b.agent_ready)}
            width={readyW}
            height={yAt(0) - yAt(b.agent_ready)}
            fill={READY}
          />
        {/if}
        <text x={x + barW / 2} y={yAt(b.total) - 7} text-anchor="middle" fill="var(--text)" font-size="12" font-weight="600">
          {b.total}
        </text>
      </g>
      <text x={PAD_L + slot * i + slot / 2} y={H - 26} text-anchor="middle" fill="var(--text)" font-size="13">
        {short ? SHORT[b.key] : LABEL[b.key]}
      </text>
      <text x={PAD_L + slot * i + slot / 2} y={H - 8} text-anchor="middle" fill={READY} font-size="12">
        {b.agent_ready}{short ? "" : " for agents"}
      </text>
    {/each}
  </svg>

  <div class="absolute flex" style="left: {PAD_L}px; right: {PAD_R}px; top: 0; bottom: 0;">
    {#each buckets as b, i (b.key)}
      <div
        class="flex-1 h-full cursor-default"
        role="presentation"
        onmouseenter={() => (hover = i)}
        onmouseleave={() => (hover = null)}
      ></div>
    {/each}
  </div>

  {#if hover !== null && buckets[hover] && buckets[hover].total > 0}
    {@const b = buckets[hover]}
    {@const x = PAD_L + slot * hover + slot / 2}
    <div
      class="absolute z-10 pointer-events-none px-3 py-2 rounded-lg whitespace-nowrap
             bg-[var(--surface)] border border-[var(--border)] shadow-[0_8px_24px_rgba(0,0,0,0.28)]"
      style="top: {PAD_T}px; {x > width * 0.6
        ? `right: ${width - x + groupW / 2 + 10}px`
        : `left: ${x + groupW / 2 + 10}px`}"
    >
      <p class="text-caption font-medium text-[var(--text)] mb-1">Open {LABEL[b.key].toLowerCase()}</p>
      {#each SEGMENTS as s (s.key)}
        <p class="flex items-center gap-2 text-caption text-[var(--text-muted)] m-0 leading-6">
          <span class="size-2.5 shrink-0" style="background: {s.color}"></span>
          {s.label}
          <span class="ml-auto pl-6 tabular-nums text-[var(--text)]">{value(b, s.key)}</span>
        </p>
      {/each}
      <p class="flex items-center gap-2 text-caption text-[var(--text-muted)] m-0 mt-1 pt-1 border-t border-[var(--border)] leading-6">
        <span class="size-2.5 shrink-0" style="background: {READY}"></span>
        Ready for agents
        <span class="ml-auto pl-6 tabular-nums text-[var(--text)]">{b.agent_ready}</span>
      </p>
      <p class="flex items-center gap-2 text-caption text-[var(--text-muted)] m-0 leading-6">
        <span class="size-2.5 shrink-0"></span>
        Needs a person
        <span class="ml-auto pl-6 tabular-nums text-[var(--text)]">{b.needs_human}</span>
      </p>
    </div>
  {/if}
</div>
