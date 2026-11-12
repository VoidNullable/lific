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

{#snippet cardHeading(title: string, count?: number)}
  <div class="flex items-center gap-2 px-4 py-3 border-b border-[var(--border)]">
    <h2 class="text-body-sm font-semibold text-[var(--text)]">{title}</h2>
    {#if count !== undefined}
      <span
        class="text-micro tabular-nums text-[var(--text-muted)] bg-[var(--bg-subtle)] px-1.5 py-px rounded-full"
      >
        {count}
      </span>
    {/if}
  </div>
{/snippet}

<div class="h-full flex flex-col">
  <div class="flex-1 overflow-y-auto">
    <div class="agent-activity max-w-[1280px] mx-auto px-6 md:px-8 py-8 md:py-10 flex flex-col gap-6">
      <!-- ── HEADER + WINDOW ──────────────────────────────── -->
      <header class="flex flex-wrap items-start justify-between gap-4">
        <div class="min-w-0">
          <h1 class="font-display text-title tracking-tight text-[var(--text)] leading-none">Agent activity</h1>
          <p class="text-body-sm text-[var(--text-muted)] mt-1.5">{WINDOW_SUBTITLE[win]}</p>
        </div>
        <div
          class="inline-flex p-0.5 rounded-lg bg-[var(--bg)] shadow-[inset_0_1px_2px_rgba(0,0,0,0.10)]"
          role="group"
          aria-label="Time window"
        >
          {#each WINDOWS as w (w)}
            <button
              class="px-3 py-1 rounded-md text-caption font-medium tabular-nums transition
                     {win === w
                ? 'bg-[var(--surface)] text-[var(--text)] shadow-[0_1px_2px_rgba(0,0,0,0.12)]'
                : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
              aria-pressed={win === w}
              onclick={() => pickWindow(w)}
            >
              {w}
            </button>
          {/each}
        </div>
      </header>

      {#if loading}
        <div class="rounded-xl bg-[var(--surface)] border border-[var(--border)] p-5">
          <div class="flex gap-4 mb-4">
            {#each [0, 1, 2, 3] as k (k)}
              <Skeleton variant="bar" class="h-3 w-14" />
            {/each}
          </div>
          <Skeleton variant="block" class="h-[200px] w-full rounded-lg" />
        </div>
        <div class="grid grid-cols-1 lg:grid-cols-[minmax(0,1fr)_340px] gap-6 items-start">
          <div class="rounded-xl bg-[var(--surface)] border border-[var(--border)] overflow-hidden">
            <div class="px-4 py-3 border-b border-[var(--border)]">
              <Skeleton variant="bar" class="h-3.5 w-20" />
            </div>
            {#each [0, 1, 2] as row (row)}
              <div class="flex items-center gap-3 px-4 py-4 border-b border-[var(--border)] last:border-b-0">
                <Skeleton variant="circle" class="size-9 rounded-lg" />
                <div class="flex flex-col gap-2 flex-1">
                  <Skeleton variant="bar" class="h-3 w-28" />
                  <Skeleton variant="bar" class="h-3 w-16" />
                </div>
                <Skeleton variant="bar" class="h-6 w-28 hidden sm:block" />
                <Skeleton variant="bar" class="h-6 w-48" />
              </div>
            {/each}
          </div>
          <div class="rounded-xl bg-[var(--surface)] border border-[var(--border)] p-4 flex flex-col gap-4">
            <Skeleton variant="bar" class="h-3.5 w-16" />
            {#each [0, 1, 2, 3] as row (row)}
              <div class="flex gap-3">
                <Skeleton variant="circle" class="size-7 rounded-lg" />
                <div class="flex flex-col gap-2 flex-1">
                  <Skeleton variant="bar" class="h-3 w-32" />
                  <Skeleton variant="bar" class="h-3 w-full" />
                </div>
              </div>
            {/each}
          </div>
        </div>
      {:else if error || !data}
        <ErrorState title="Couldn't load agent activity" message={error}>
          <button
            class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)] px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
            onclick={() => load("initial")}
          >
            Try again
          </button>
        </ErrorState>
      {:else if data.agents.length === 0}
        <section
          class="rounded-xl bg-[var(--surface)] border border-[var(--border)] px-6 py-14
                 flex flex-col items-center text-center gap-3"
          data-testid="agents-empty"
        >
          <Mascot src="/LizzySleep2.png" nativeW={1000} nativeH={420} scale={0.18} />
          <h2 class="font-display text-heading text-[var(--text)]">No agents yet</h2>
          <p class="text-body text-[var(--text-muted)] max-w-[46ch] leading-relaxed">
            Agents show up here once a connected tool reads or changes something in Lific.
            Connect one in Settings, under Connected tools.
          </p>
          <a
            href="#/settings"
            use:navLink={navigate}
            class="mt-1 inline-flex items-center gap-1.5 text-body-sm text-[var(--text-muted)]
                   hover:text-[var(--text)] border border-[var(--border)] px-3 py-1.5 rounded-md
                   hover:bg-[var(--bg-subtle)] transition-colors"
          >
            <SettingsIcon size={14} /> Open Settings
          </a>
        </section>
      {:else}
        <div class="flex flex-col gap-6 transition-opacity {switching ? 'opacity-60' : ''}" aria-busy={switching}>
          <!-- ── CHART ─────────────────────────────────────── -->
          <section class="rounded-xl bg-[var(--surface)] border border-[var(--border)] p-5 min-w-0">
            <div class="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1 mb-3">
              <h2 class="text-body-sm font-semibold text-[var(--text)]">Actions</h2>
              <p class="text-caption text-[var(--text-faint)] tabular-nums">
                {#if total === 0}
                  None in {WINDOW_PHRASE[data.window]}
                {:else}
                  {total} {total === 1 ? "action" : "actions"} by {busyAgents}
                  {busyAgents === 1 ? "agent" : "agents"}
                {/if}
              </p>
            </div>
            <ActivityChart buckets={data.buckets} win={data.window} bucketSeconds={data.bucket_seconds} />
          </section>

          <!-- ── AGENTS + RECENT ───────────────────────────── -->
          <div class="grid grid-cols-1 lg:grid-cols-[minmax(0,1fr)_340px] gap-6 items-start">
            <section
              class="rounded-xl bg-[var(--surface)] border border-[var(--border)] overflow-hidden min-w-0"
              data-testid="agent-list"
            >
              {@render cardHeading("Agents", data.agents.length)}
              <AgentList agents={data.agents} win={data.window} />
            </section>

            <section
              class="rounded-xl bg-[var(--surface)] border border-[var(--border)] overflow-hidden min-w-0"
              data-testid="agent-recent"
            >
              {@render cardHeading("Recent")}
              {#if data.recent.length === 0}
                <p class="px-4 py-4 text-body-sm text-[var(--text-muted)]">
                  Nothing to list from {WINDOW_PHRASE[data.window]}.
                </p>
              {:else}
                <div class="p-2">
                  <RecentFeed items={data.recent} {navigate} />
                </div>
              {/if}
            </section>
          </div>
        </div>
      {/if}
    </div>
  </div>
</div>

<style>
  /* Series colors for the chart, legend and timeline icons. Deliberately
     not derived from --accent, which follows the user's accent theme:
     Read is a quiet neutral (it dominates the counts), Edit blue, Create
     green, Close amber so it never reads as a shade of Edit. */
  .agent-activity {
    --agent-read: #a3aea8;
    --agent-edit: #3b6fd8;
    --agent-create: #1f9d55;
    --agent-close: #d98a0b;
  }
  :global(.dark) .agent-activity {
    --agent-read: #6f7b75;
    --agent-edit: #6b9bf0;
    --agent-create: #4ade80;
    --agent-close: #f5b53d;
  }
</style>
