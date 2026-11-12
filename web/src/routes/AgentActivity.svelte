<script lang="ts">
  // LIF-543: Agent activity. What connected tools (bot users) have been
  // reading and changing: a stacked chart of actions per bucket, the agent
  // roster with live status, and a recent timeline. Everything comes from
  // one read, `getAgentActivity`, for the chosen window (1h, 24h, 7d).
  //
  // Reads emit no realtime events, so the page also polls every 30 seconds;
  // mutation events refresh it sooner, debounced.

  import { getContext, onMount } from "svelte";
  import { Settings as SettingsIcon } from "lucide-svelte";
  import { getAgentActivity, type AgentActivity, type AgentWindow } from "../lib/api";
  import { startAutoRefresh } from "../lib/autoRefresh.svelte";
  import { navLink } from "../lib/navLink";
  import ErrorState from "../lib/ErrorState.svelte";
  import Skeleton from "../lib/Skeleton.svelte";
  import Mascot from "../lib/Mascot.svelte";
  import ActivityChart from "../lib/agents/ActivityChart.svelte";
  import AgentList from "../lib/agents/AgentList.svelte";
  import RecentFeed from "../lib/agents/RecentFeed.svelte";
  import {
    WINDOWS,
    WINDOW_PHRASE,
    WINDOW_SUBTITLE,
    isAgentWindow,
    totalOf,
  } from "../lib/agents/activity";

  let { navigate }: { navigate: (path: string) => void } = $props();

  const topbarCtx = getContext<{
    set: (s: import("svelte").Snippet | undefined) => void;
  } | undefined>("lific:topbar");

  $effect(() => {
    topbarCtx?.set(topbarContent);
    return () => topbarCtx?.set(undefined);
  });

  const WINDOW_KEY = "lific:agent-activity-window";

  function storedWindow(): AgentWindow {
    try {
      const v = localStorage.getItem(WINDOW_KEY);
      return isAgentWindow(v) ? v : "1h";
    } catch {
      return "1h";
    }
  }

  let win = $state<AgentWindow>(storedWindow());
  let data = $state<AgentActivity | null>(null);
  let loading = $state(true);
  /** A window switch is in flight; the previous window stays on screen, dimmed. */
  let switching = $state(false);
  let error = $state("");

  // Every load takes a ticket; only the latest one may write state, so a
  // slow response for an old window (or an older refresh) is dropped.
  let ticket = 0;

  async function load(mode: "initial" | "switch" | "refresh") {
    const mine = ++ticket;
    const requested = win;
    if (mode === "initial") {
      loading = true;
      error = "";
    } else if (mode === "switch") {
      switching = true;
    }

    const res = await getAgentActivity(requested);
    if (mine !== ticket) return;

    switching = false;
    loading = false;
    if (res.ok) {
      data = res.data;
      error = "";
    } else if (mode !== "refresh" || !data) {
      // A failed background refresh keeps what is on screen.
      data = null;
      error = res.error;
    }
  }

  function pickWindow(w: AgentWindow) {
    if (w === win) return;
    win = w;
    try {
      localStorage.setItem(WINDOW_KEY, w);
    } catch {
      // Private mode or storage disabled: the choice just won't persist.
    }
    void load(data ? "switch" : "initial");
  }

  onMount(() => {
    void load("initial");
  });

  $effect(() =>
    startAutoRefresh({
      refresh: () => load("refresh"),
      isBusy: () => loading || switching,
      intervalMs: 30_000,
      realtimeDebounceMs: 1_000,
      realtimeMaxWaitMs: 5_000,
      shouldRefresh: (event) =>
        event.type === "resync.required" ||
        event.type.startsWith("issue.") ||
        event.type.startsWith("project."),
    }),
  );

  let total = $derived(data ? totalOf(data.totals) : 0);
  let busyAgents = $derived(data ? data.agents.filter((a) => totalOf(a.counts) > 0).length : 0);
</script>

{#snippet topbarContent()}
  <div class="flex items-center gap-3 px-6 py-2 w-full">
    <span class="text-body-sm font-medium text-[var(--text)]">Agent activity</span>
  </div>
{/snippet}

{#snippet sectionHeading(title: string, count?: number)}
  <div class="flex items-center gap-2.5 mb-4">
    <h2 class="text-[1.35rem] font-semibold tracking-tight text-[var(--aa-text)]">{title}</h2>
    {#if count !== undefined}
      <span class="text-[0.8rem] font-medium tabular-nums text-[var(--aa-text)] bg-[var(--aa-tile)] min-w-6 h-6 px-2 rounded-full inline-flex items-center justify-center">
        {count}
      </span>
    {/if}
  </div>
{/snippet}

<div class="agent-activity h-full overflow-y-auto">
  <div class="max-w-[1320px] mx-auto px-6 md:px-10 pt-7 pb-12">
    <!-- ── HEADER + WINDOW ──────────────────────────────── -->
    <header class="flex flex-wrap items-start justify-between gap-4 pb-5 border-b border-[var(--aa-line)]">
      <div class="min-w-0">
        <h1 class="text-[1.9rem] font-semibold tracking-tight leading-tight text-[var(--aa-text)]">Agent activity</h1>
        <p class="text-[0.95rem] text-[var(--aa-text-2)] mt-1">
          {WINDOW_SUBTITLE[win]}{#if data && !loading && total > 0}<span class="text-[var(--aa-text-3)]">
              &nbsp;·&nbsp;{total} {total === 1 ? "action" : "actions"} by {busyAgents}
              {busyAgents === 1 ? "agent" : "agents"}</span
            >{/if}
        </p>
      </div>
      <div class="inline-flex rounded-lg border border-[var(--aa-line-strong)] overflow-hidden" role="group" aria-label="Time window">
        {#each WINDOWS as w, i (w)}
          <button
            class="px-4 py-1.5 text-[0.95rem] font-medium tabular-nums transition-colors
                   {i > 0 ? 'border-l border-[var(--aa-line-strong)]' : ''}
                   {win === w
              ? 'bg-[var(--aa-primary)] text-white'
              : 'text-[var(--aa-text-2)] hover:text-[var(--aa-text)] hover:bg-[var(--aa-hover)]'}"
            aria-pressed={win === w}
            onclick={() => pickWindow(w)}
          >
            {w}
          </button>
        {/each}
      </div>
    </header>

    {#if loading}
      <div class="py-6 border-b border-[var(--aa-line)]">
        <div class="flex gap-5 mb-4">
          {#each [0, 1, 2, 3] as k (k)}
            <Skeleton variant="bar" class="h-3.5 w-14" />
          {/each}
        </div>
        <Skeleton variant="block" class="h-[236px] w-full rounded-lg" />
      </div>
      <div class="pt-6 flex flex-col gap-5">
        {#each [0, 1, 2] as row (row)}
          <div class="flex items-center gap-4">
            <Skeleton variant="circle" class="size-11 rounded-xl" />
            <div class="flex flex-col gap-2 flex-1">
              <Skeleton variant="bar" class="h-3.5 w-32" />
              <Skeleton variant="bar" class="h-3 w-20" />
            </div>
            <Skeleton variant="bar" class="h-8 w-72" />
          </div>
        {/each}
      </div>
    {:else if error || !data}
      <div class="pt-10">
        <ErrorState title="Couldn't load agent activity" message={error}>
          <button
            class="text-[0.9rem] font-medium text-white bg-[var(--aa-primary)] px-3.5 py-1.5 rounded-md hover:opacity-90 transition-opacity"
            onclick={() => load("initial")}
          >
            Try again
          </button>
        </ErrorState>
      </div>
    {:else if data.agents.length === 0}
      <section class="py-16 flex flex-col items-center text-center gap-3" data-testid="agents-empty">
        <Mascot src="/LizzySleep2.png" nativeW={1000} nativeH={420} scale={0.18} />
        <h2 class="text-[1.35rem] font-semibold text-[var(--aa-text)]">No agents yet</h2>
        <p class="text-[0.95rem] text-[var(--aa-text-2)] max-w-[46ch] leading-relaxed">
          Agents show up here once a connected tool reads or changes something in Lific.
          Connect one in Settings, under Connected tools.
        </p>
        <a
          href="#/settings"
          use:navLink={navigate}
          class="mt-1 inline-flex items-center gap-1.5 text-[0.9rem] text-[var(--aa-text)]
                 border border-[var(--aa-line-strong)] px-3.5 py-1.5 rounded-md hover:bg-[var(--aa-hover)] transition-colors"
        >
          <SettingsIcon size={15} /> Open Settings
        </a>
      </section>
    {:else}
      <div class="transition-opacity {switching ? 'opacity-60' : ''}" aria-busy={switching}>
        <!-- ── CHART ─────────────────────────────────────── -->
        <section class="py-6 border-b border-[var(--aa-line)] min-w-0">
          <ActivityChart buckets={data.buckets} win={data.window} bucketSeconds={data.bucket_seconds} />
        </section>

        <!-- ── AGENTS + RECENT ───────────────────────────── -->
        <div class="grid grid-cols-1 xl:grid-cols-[minmax(0,1fr)_minmax(300px,380px)]">
          <section class="pt-6 xl:pr-8 min-w-0" data-testid="agent-list">
            {@render sectionHeading("Agents", data.agents.length)}
            <AgentList agents={data.agents} win={data.window} />
          </section>

          <section
            class="pt-6 xl:pl-8 xl:border-l border-[var(--aa-line)] min-w-0 max-xl:mt-6"
            data-testid="agent-recent"
          >
            {@render sectionHeading("Recent")}
            {#if data.recent.length === 0}
              <p class="text-[0.95rem] text-[var(--aa-text-2)]">Nothing to list from {WINDOW_PHRASE[data.window]}.</p>
            {:else}
              <RecentFeed items={data.recent} {navigate} />
            {/if}
          </section>
        </div>
      </div>
    {/if}
  </div>
</div>

<style>
  /* Page-local palette, deliberately not built from the app's theme tokens:
     the page follows its own design until the app-wide tokens are reworked.
     Close is amber so it never reads as a shade of Edit's blue. */
  .agent-activity {
    --aa-text: #161a18;
    --aa-text-2: #5d6762;
    --aa-text-3: #9aa39e;
    --aa-line: rgba(0, 0, 0, 0.09);
    --aa-line-strong: rgba(0, 0, 0, 0.14);
    --aa-grid: rgba(0, 0, 0, 0.06);
    --aa-axis: rgba(0, 0, 0, 0.2);
    --aa-tile: rgba(0, 0, 0, 0.055);
    --aa-hover: rgba(0, 0, 0, 0.035);
    --aa-pop: #ffffff;
    --aa-primary: #4f5ae0;
    --aa-link: #4652d6;
    --aa-active: #16a34a;
    --aa-idle: #d97706;
    --agent-read: #aab5af;
    --agent-edit: #4f5ee8;
    --agent-create: #22b55a;
    --agent-close: #ee8a0b;
  }
  :global(.dark) .agent-activity {
    --aa-text: #eef1ef;
    --aa-text-2: #a3aca7;
    --aa-text-3: #6c7570;
    --aa-line: rgba(255, 255, 255, 0.09);
    --aa-line-strong: rgba(255, 255, 255, 0.14);
    --aa-grid: rgba(255, 255, 255, 0.07);
    --aa-axis: rgba(255, 255, 255, 0.22);
    --aa-tile: rgba(255, 255, 255, 0.075);
    --aa-hover: rgba(255, 255, 255, 0.045);
    --aa-pop: #1d2220;
    --aa-primary: #4f5ae0;
    --aa-link: #8f98ff;
    --aa-active: #4ade80;
    --aa-idle: #f5c542;
    --agent-read: #6e7d76;
    --agent-edit: #5b6cf0;
    --agent-create: #5ee07a;
    --agent-close: #f5a524;
  }
</style>
