<script lang="ts">
  // LIF-237 — Home: the landing dashboard at "/".
  //
  // LIF-507 rebuilt the body around two lanes of work: what an agent can
  // pick up (unassigned, unblocked, ranked by the same importance score as
  // the project overview) and what only a person can do (the LIF-506
  // attention groups). Above them, open work by how long it has been
  // sitting, a quiet 90-day open count, and which projects are going stale.
  // The aggregates come from one cross-project read, `getHomeOverview`, so
  // they are not truncated by the list endpoint's row cap.

  import {
    me,
    listProjects,
    listIssues,
    listAllPages,
    listProjectActivity,
    getAttention,
    getHomeOverview,
    type Attention,
    type AuthUser,
    type HomeOverview,
    type Project,
    type Issue,
    type Page,
    type Activity,
  } from "../lib/api";
  import {
    selectActivityRate,
    type ActivityCountsReader,
  } from "../lib/activityRate";
  import StatusIcon from "../lib/StatusIcon.svelte";
  import PriorityIcon from "../lib/PriorityIcon.svelte";
  import Mascot from "../lib/Mascot.svelte";
  import ErrorState from "../lib/ErrorState.svelte";
  import Skeleton from "../lib/Skeleton.svelte";
  import AgeChart from "../lib/home/AgeChart.svelte";
  import OpenTrend from "../lib/home/OpenTrend.svelte";
  import ProjectFreshness from "../lib/home/ProjectFreshness.svelte";
  import DoneHeatmap from "../lib/home/DoneHeatmap.svelte";
  import HeatBar from "../lib/home/HeatBar.svelte";
  import { recordHomeVisit } from "../lib/home/lastVisit";
  import { ageLabel, daysSince, heat, importanceScore } from "../lib/issues/importance";
  import {
    Plus,
    Command,
    FileText,
    UserCheck,
    User,
    Hourglass,
    Bot,
  } from "lucide-svelte";
  import { projectCodeOf } from "../lib/references";
  import { getContext } from "svelte";
  import { startAutoRefresh } from "../lib/autoRefresh.svelte";

  const topbarCtx = getContext<{
    set: (s: import("svelte").Snippet | undefined) => void;
  } | undefined>("lific:topbar");

  $effect(() => {
    topbarCtx?.set(topbarContent);
    return () => topbarCtx?.set(undefined);
  });

  let {
    navigate,
    realtimeActivityCounts,
    realtimeActivityReady,
    realtimeActivityRevision,
  }: {
    navigate: (path: string) => void;
    realtimeActivityCounts: ActivityCountsReader;
    realtimeActivityReady: boolean;
    realtimeActivityRevision: number;
  } = $props();

  // Measured once per mount, so auto-refreshes keep comparing against the
  // same "last here".
  const visitSince = recordHomeVisit();
  const sinceIso = visitSince === null ? null : new Date(visitSince).toISOString();

  let user = $state<AuthUser | null>(null);
  let projects = $state<Project[]>([]);
  /** LIF-506: what needs the signed-in person. Null until loaded or when
   *  the request fails, which hides the lane rather than the dashboard. */
  let attention = $state<Attention | null>(null);
  let overview = $state<HomeOverview | null>(null);
  /** Unassigned, workable issues across projects, ranked client-side. */
  let agentPool = $state<Issue[]>([]);
  let allPages = $state<Page[]>([]);
  let activityItems = $state<Activity[]>([]);
  let activityNow = $state(Date.now());
  let loading = $state(true);
  let error = $state("");

  // The shortest displayed window is one second, so the shared 30-second
  // relative-time clock is too coarse. Keep the finer timer local to Home.
  $effect(() => {
    const interval = setInterval(() => {
      activityNow = Date.now();
    }, 1_000);
    return () => clearInterval(interval);
  });

  $effect(() => {
    loadData();
  });

  $effect(() =>
    startAutoRefresh({
      refresh: () => loadData(false),
      isBusy: () => loading,
      realtimeDebounceMs: 750,
      realtimeMaxWaitMs: 5_000,
      shouldRefresh: (event) =>
        event.type === "resync.required" ||
        event.type.startsWith("project.") ||
        event.type.startsWith("issue."),
    }),
  );

  async function loadData(initial = true) {
    if (initial) {
      loading = true;
      error = "";
    }

    const [meRes, projRes] = await Promise.all([me(), listProjects()]);
    if (!meRes.ok) {
      if (initial) {
        error = meRes.error;
        loading = false;
      }
      return;
    }
    error = "";
    user = meRes.data;
    if (projRes.ok) projects = projRes.data;

    // The pool is ordered by priority server-side, so the 500-row cap can
    // only ever drop the least pressing issues before ranking.
    const [attentionRes, overviewRes, poolRes, pagesRes] = await Promise.all([
      getAttention(),
      getHomeOverview(sinceIso),
      listIssues({ workable: true, assignee: "none", order_by: "priority", limit: 500 }),
      listAllPages(),
    ]);
    attention = attentionRes.ok ? attentionRes.data : null;
    overview = overviewRes.ok ? overviewRes.data : null;
    agentPool = poolRes.ok ? poolRes.data : [];
    allPages = pagesRes.ok ? pagesRes.data : [];

    // There is no cross-project activity feed: merge the three most
    // recently active projects' feeds client-side.
    const feeds = await Promise.all(
      activeProjectIds.slice(0, 3).map((pid) => listProjectActivity(pid, 8, 0)),
    );
    const combined = feeds.flatMap((f) => (f.ok ? f.data.items : []));
    combined.sort((a, b) => b.ts.localeCompare(a.ts));
    activityItems = combined.slice(0, 8);

    if (initial) loading = false;
  }

  /** Visible projects, most recently active first. */
  let activeProjectIds = $derived.by(() => {
    const last = new Map((overview?.projects ?? []).map((p) => [p.project_id, p.last_activity ?? ""]));
    return [...projects]
      .sort((a, b) =>
        (last.get(b.id) || b.updated_at).localeCompare(last.get(a.id) || a.updated_at),
      )
      .map((p) => p.id);
  });

  function projectIdent(id: number | null): string | null {
    if (id === null) return null;
    return projects.find((p) => p.id === id)?.identifier ?? null;
  }

  function issueProjectIdent(issue: Issue): string {
    return projectIdent(issue.project_id) ?? projectCodeOf(issue.identifier);
  }

  function openIssue(issue: Issue) {
    navigate(`/${issueProjectIdent(issue)}/issues/${issue.identifier}`);
  }

  // ── Agent queue ──────────────────────────────────────────────

  const QUEUE_CAP = 7;

  let agentQueue = $derived.by(() => {
    const now = Date.now();
    return agentPool
      .filter((i) => i.status !== "active" && !i.needs_human)
      .map((issue) => ({ issue, score: importanceScore(issue, now) }))
      .sort((a, b) => b.score - a.score);
  });

  // ── Only you (LIF-506) ───────────────────────────────────────
  //
  // Open issues assigned to you, marked for any person, or with a wait on
  // you, across every project you can see. Empty groups are hidden. An
  // issue can show in "Waiting on you" and in one of the other two.

  const ATTENTION_CAP = 4;
  let attentionExpanded = $state<Set<string>>(new Set());

  let attentionGroups = $derived.by(() => {
    if (!attention) return [];
    return [
      { key: "assigned", label: "Assigned to you", icon: UserCheck, items: attention.assigned },
      { key: "human", label: "Marked for a person", icon: User, items: attention.human },
      { key: "waiting", label: "Waiting on you", icon: Hourglass, items: attention.waiting },
    ].filter((g) => g.items.length > 0);
  });
  let attentionIssues = $derived(attentionGroups.flatMap((g) => g.items));
  let attentionTotal = $derived(new Set(attentionIssues.map((i) => i.id)).size);

  function toggleAttentionGroup(key: string) {
    const next = new Set(attentionExpanded);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    attentionExpanded = next;
  }

  // ── Since you were last here ─────────────────────────────────

  function serverTime(ts: string): number {
    return new Date(ts.replace(" ", "T") + "Z").getTime();
  }

  let sinceLine = $derived.by(() => {
    if (visitSince === null || !overview?.since) return null;
    const s = overview.since;
    const fresh = new Set(
      attentionIssues.filter((i) => serverTime(i.updated_at) > visitSince).map((i) => i.id),
    ).size;
    const parts: string[] = [];
    const did = [
      s.agents_closed ? `closed ${s.agents_closed}` : "",
      s.agents_opened ? `opened ${s.agents_opened}` : "",
    ].filter(Boolean);
    if (did.length > 0) parts.push(`agents ${did.join(" and ")}.`);
    if (fresh > 0) {
      parts.push(fresh === 1 ? "1 new thing needs you." : `${fresh} new things need you.`);
    }
    const when = relative(visitSince);
    if (parts.length === 0) return `Quiet since you were last here (${when}).`;
    return `Since you were last here (${when}): ${parts.join(" ")}`;
  });

  function relative(ts: number): string {
    const mins = Math.floor((Date.now() - ts) / 60000);
    if (mins < 60) return `${Math.max(1, mins)}m ago`;
    const hrs = Math.floor(mins / 60);
    if (hrs < 24) return `${hrs}h ago`;
    const days = Math.floor(hrs / 24);
    return days === 1 ? "yesterday" : `${days} days ago`;
  }

  /** Net change in open issues over the last week of the trend. */
  let weekDelta = $derived.by(() => {
    const t = overview?.open_trend ?? [];
    if (t.length < 8) return 0;
    return t[t.length - 1].count - t[t.length - 8].count;
  });

  // ── Pinned pages ──────────────────────────────────────────────
  //
  // See api.ts's `listAllPages` doc comment for why this is one
  // cross-project call filtered client-side.

  let pinnedPages = $derived.by(() =>
    allPages
      .filter((p) => p.pinned && p.project_id !== null)
      .sort((a, b) => b.updated_at.localeCompare(a.updated_at))
      .slice(0, 6),
  );

  // ── Activity digest ──────────────────────────────────────────

  function activityVerb(a: Activity): string {
    switch (a.action) {
      case "create":
        return a.entity_type === "comment" ? "commented on" : `created ${a.entity_type}`;
      case "delete":
        return a.entity_type === "comment" ? "deleted a comment on" : `deleted ${a.entity_type}`;
      case "update":
        return a.entity_type === "comment"
          ? "edited a comment on"
          : a.field
            ? `changed ${a.field} on`
            : "updated";
      case "attach":
        return "labeled";
      case "detach":
        return "unlabeled";
      case "link":
        return "linked";
      case "unlink":
        return "unlinked";
      case "assign":
        return "assigned";
      case "unassign":
        return "unassigned";
      case "wait":
        return "set a wait on";
      case "unwait":
        return "cleared a wait on";
      default:
        return a.action;
    }
  }

  function activityActor(a: Activity): string {
    return a.actor_display_name || a.actor_username || "system";
  }

  let activityRate = $derived.by(() => {
    void realtimeActivityRevision;
    return selectActivityRate(realtimeActivityCounts(activityNow));
  });

  function activityDest(a: Activity): string | null {
    const ident = projectIdent(a.project_id);
    if (!ident) return null;
    switch (a.entity_type) {
      case "issue":
        return a.entity_label ? `/${ident}/issues/${a.entity_label}` : null;
      case "page":
        return `/${ident}/pages/${a.entity_id}`;
      case "comment":
        if (a.issue_id !== null && a.entity_label) return `/${ident}/issues/${a.entity_label}`;
        if (a.page_id !== null) return `/${ident}/pages/${a.page_id}`;
        return null;
      default:
        return null;
    }
  }

  // ── Greeting + quick actions ────────────────────────────────────

  function greetingText(h: number): string {
    if (h < 5) return "Good night";
    if (h < 12) return "Good morning";
    if (h < 17) return "Good afternoon";
    if (h < 21) return "Good evening";
    return "Good night";
  }

  const nowHour = new Date().getHours();
  const greeting = greetingText(nowHour);
  const todayLabel = new Date().toLocaleDateString("en-US", {
    weekday: "long",
    month: "long",
    day: "numeric",
  });

  // "New issue" lands in the most recently active project.
  let quickIssueProject = $derived(projectIdent(activeProjectIds[0] ?? null));

  function openPalette() {
    // The palette listens for cmd/ctrl+K globally (CommandPalette.svelte,
    // mounted once in Layout) — dispatch the same chord rather than
    // threading a ref through Layout for a single button.
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "k", metaKey: true, bubbles: true }));
  }
</script>

{#snippet topbarContent()}
  <div class="flex items-center gap-3 px-6 py-2 w-full">
    <span class="text-body-sm font-medium text-[var(--text)]">Home</span>
  </div>
{/snippet}

{#snippet sectionHeading(title: string, count?: number, note?: string)}
  <div class="mb-4">
    <h2 class="flex items-center gap-2.5 text-[1.35rem] font-semibold tracking-tight leading-tight text-[var(--text)]">
      {title}
      {#if count !== undefined}
        <span
          class="text-[0.8rem] font-medium tabular-nums text-[var(--text)] bg-[var(--chart-tile)]
                 min-w-6 h-6 px-2 rounded-full inline-flex items-center justify-center"
        >
          {count}
        </span>
      {/if}
    </h2>
    {#if note}
      <p class="text-[0.95rem] text-[var(--text-muted)] mt-1">{note}</p>
    {/if}
  </div>
{/snippet}

<div class="home h-full overflow-y-auto">
  <div class="max-w-[1320px] mx-auto px-6 md:px-10 pt-7 pb-12">
    {#if loading}
      <!-- LIF-281: content-shaped skeleton mirroring the loaded layout:
           the header, the charts row, then the two lanes. -->
      <div class="flex flex-wrap items-start justify-between gap-4 pb-5 border-b border-[var(--border)]">
        <div class="flex flex-col gap-2.5">
          <Skeleton variant="bar" class="h-7 w-64" />
          <Skeleton variant="bar" class="h-3.5 w-40" />
        </div>
        <div class="flex items-center gap-2">
          <Skeleton variant="bar" class="h-8 w-28 rounded-md" />
          <Skeleton variant="bar" class="h-8 w-24 rounded-md" />
        </div>
      </div>
      <div class="py-6 border-b border-[var(--border)]">
        <Skeleton variant="bar" class="h-5 w-48 mb-5" />
        <Skeleton variant="block" class="h-[264px] w-full rounded-lg" />
      </div>
      <div class="pt-6 flex flex-col gap-4">
        <Skeleton variant="bar" class="h-5 w-40 mb-1" />
        {#each [0, 1, 2, 3] as row (row)}
          <div class="flex items-center gap-3">
            <Skeleton variant="bar" class="h-3 w-16 shrink-0" />
            <Skeleton variant="bar" class="h-3 flex-1 max-w-[360px]" />
          </div>
        {/each}
      </div>
    {:else if error}
      <ErrorState title="Couldn't load your dashboard" message={error}>
        <button
          class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)] px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
          onclick={() => loadData()}
        >
          Try again
        </button>
      </ErrorState>
    {:else}
      <!-- ── HEADER + QUICK ACTIONS ───────────────────────── -->
      <header class="flex flex-wrap items-start justify-between gap-4 pb-5 border-b border-[var(--border)]">
        <div class="min-w-0">
          <h1 class="text-[1.9rem] font-semibold tracking-tight leading-tight text-[var(--text)]">
            {greeting}{user ? `, ${user.display_name || user.username}` : ""}
          </h1>
          <p class="text-[0.95rem] text-[var(--text-muted)] mt-1">{todayLabel}</p>
          {#if sinceLine}
            <p class="text-[0.95rem] text-[var(--text-muted)] mt-0.5" data-testid="since-line">{sinceLine}</p>
          {/if}
        </div>

        <div class="flex items-center gap-2">
          {#if quickIssueProject}
            <button
              class="flex items-center gap-1.5 text-body-sm font-medium
                     text-[var(--btn-success-text)] bg-[var(--btn-success)]
                     px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
              onclick={() => navigate(`/${quickIssueProject}/issues/new`)}
            >
              <Plus size={14} /> New issue
            </button>
          {/if}
          <button
            class="flex items-center gap-1.5 text-body-sm text-[var(--text-muted)]
                   hover:text-[var(--text)] border border-[var(--border)]
                   px-3 py-1.5 rounded-md hover:bg-[var(--chart-hover)] transition-colors"
            onclick={openPalette}
            title="Open command palette"
          >
            <Command size={13} />
            Jump to…
            <kbd class="font-mono text-micro leading-none text-[var(--text-faint)]
                        border border-[var(--border)] rounded px-1 py-0.5 ml-0.5">⌘K</kbd>
          </button>
        </div>
      </header>

      <!-- ── OPEN WORK: AGE AND TREND ──────────────────────── -->
      {#if overview}
        {@const busyProjects = overview.projects.filter((p) => p.open > 0).length}
        <div class="home-split border-b border-[var(--border)]">
          <section class="py-6 min-w-0" data-testid="open-by-age">
            {@render sectionHeading(
              "Open work by age",
              undefined,
              `${overview.open_total} open across ${busyProjects} ${busyProjects === 1 ? "project" : "projects"}, ` +
                `${weekDelta > 0 ? `+${weekDelta}` : weekDelta === 0 ? "no change" : weekDelta} this week`,
            )}
            <AgeChart buckets={overview.age_buckets} />
          </section>
          <section class="home-aside py-6 min-w-0">
            {@render sectionHeading(
              "Open over 90 days",
              undefined,
              `${overview.open_trend[overview.open_trend.length - 1]?.count ?? 0} open now`,
            )}
            <OpenTrend points={overview.open_trend} />
          </section>
        </div>
      {/if}

      <!-- ── THE TWO LANES ─────────────────────────────────── -->
      <div class="home-split border-b border-[var(--border)]">
        <section class="py-6 min-w-0" data-testid="agent-queue">
          {@render sectionHeading(
            "Agent queue",
            agentQueue.length,
            "Unassigned and unblocked, ranked by priority, age and idle time.",
          )}
          {#if agentQueue.length === 0}
            <div class="flex flex-col items-center justify-center py-10 gap-3">
              <Mascot src="/LizzySleep2.png" nativeW={1000} nativeH={420} scale={0.16} />
              <p class="text-[0.95rem] text-[var(--text-muted)]">Nothing is waiting for an agent.</p>
            </div>
          {:else}
            <ol class="flex flex-col border-t border-[var(--border)]">
              {#each agentQueue.slice(0, QUEUE_CAP) as { issue, score }, rank (issue.id)}
                {@const idle = daysSince(issue.updated_at)}
                <li>
                  <button
                    class="w-full flex items-center gap-3 px-1 py-2.5 text-left border-b border-[var(--border)]
                           hover:bg-[var(--chart-hover)] transition-colors"
                    data-identifier={issue.identifier}
                    onclick={() => openIssue(issue)}
                  >
                    <span class="w-4 shrink-0 text-caption tabular-nums text-[var(--text-faint)] text-right">{rank + 1}</span>
                    <span class="w-14 shrink-0 hidden sm:block"><HeatBar value={heat(score)} /></span>
                    <span class="text-caption font-mono text-[var(--text-faint)] w-[64px] shrink-0 truncate">
                      {issue.identifier}
                    </span>
                    <span class="text-body-sm text-[var(--text)] truncate flex-1">{issue.title}</span>
                    <PriorityIcon priority={issue.priority} size={15} />
                    <span class="shrink-0 hidden md:flex items-center gap-1.5 text-micro tabular-nums">
                      <span class="w-[52px] text-right text-[var(--text-faint)]">{ageLabel(daysSince(issue.created_at))} old</span>
                      {#if idle >= 14}
                        <span class="px-1.5 py-0.5 rounded-full text-[var(--warn)] bg-[color-mix(in_oklab,var(--warn)_14%,transparent)]">
                          idle {ageLabel(idle)}
                        </span>
                      {:else}
                        <span class="px-1.5 py-0.5 text-[var(--text-faint)]">idle {ageLabel(idle)}</span>
                      {/if}
                    </span>
                  </button>
                </li>
              {/each}
            </ol>
            {#if agentQueue.length > QUEUE_CAP}
              <p class="pt-2.5 text-caption text-[var(--text-muted)]">
                {agentQueue.length - QUEUE_CAP} more ready after these.
              </p>
            {/if}
          {/if}
        </section>

        <section class="home-aside py-6 min-w-0" data-testid="needs-you">
          {@render sectionHeading(
            "Only you",
            attention ? attentionTotal : undefined,
            "Assigned to you, marked for a person, or waiting on you.",
          )}
          {#if !attention}
            <p class="text-[0.95rem] text-[var(--text-muted)]">Couldn't load what needs you.</p>
          {:else if attentionGroups.length === 0}
            <p class="text-[0.95rem] text-[var(--text-muted)]">Nothing is assigned to you or waiting on you.</p>
          {:else}
            <div class="flex flex-col gap-5">
              {#each attentionGroups as group (group.key)}
                {@const expanded = attentionExpanded.has(group.key)}
                {@const shown = expanded ? group.items : group.items.slice(0, ATTENTION_CAP)}
                <div data-attention-group={group.key}>
                  <div class="flex items-center gap-2 pb-1.5 text-caption font-medium text-[var(--text-muted)]">
                    <group.icon size={13} class="shrink-0 text-[var(--text-faint)]" />
                    <span>{group.label}</span>
                    <span class="text-[var(--text-faint)] tabular-nums">{group.items.length}</span>
                  </div>
                  <div class="flex flex-col border-t border-[var(--border)]">
                    {#each shown as issue (issue.id)}
                      <button
                        class="w-full flex items-center gap-2.5 px-1 py-2 text-left border-b border-[var(--border)]
                               hover:bg-[var(--chart-hover)] transition-colors"
                        data-identifier={issue.identifier}
                        onclick={() => openIssue(issue)}
                      >
                        <StatusIcon status={issue.status} size={14} />
                        <span class="text-caption font-mono text-[var(--text-faint)] w-[64px] shrink-0 truncate">
                          {issue.identifier}
                        </span>
                        <span class="text-body-sm text-[var(--text)] truncate flex-1">{issue.title}</span>
                        <PriorityIcon priority={issue.priority} size={15} />
                      </button>
                    {/each}
                  </div>
                  {#if group.items.length > ATTENTION_CAP}
                    <button
                      class="mt-1.5 text-caption text-[var(--text-muted)] hover:text-[var(--text)] transition-colors"
                      onclick={() => toggleAttentionGroup(group.key)}
                    >
                      {expanded ? "Show fewer" : `Show all ${group.items.length}`}
                    </button>
                  {/if}
                </div>
              {/each}
            </div>
          {/if}
        </section>
      </div>

      <!-- ── FRESHNESS, YOUR WEEKS, ACTIVITY, PINS ─────────── -->
      <div class="home-split">
        <section class="pt-6 min-w-0" data-testid="project-freshness">
          {@render sectionHeading("Project freshness", undefined, "Open work per project, colored by its last change.")}
          {#if overview}
            <ProjectFreshness pulses={overview.projects} {projects} {navigate} />
          {/if}
        </section>

        <div class="home-aside pt-6 max-xl:mt-6 min-w-0 flex flex-col gap-8">
          {#if overview}
            <section>
              {@render sectionHeading("Your last 12 weeks", undefined, "Issues you and your agents finished.")}
              <DoneHeatmap days={overview.my_done} />
            </section>
          {/if}
          <section>
            {@render sectionHeading(
              "Recent activity",
              undefined,
              realtimeActivityReady ? `${activityRate.value} ${activityRate.unit}` : undefined,
            )}
            {#if activityItems.length === 0}
              <p class="text-[0.95rem] text-[var(--text-muted)]">No recent activity.</p>
            {:else}
              <ol class="flex flex-col">
                {#each activityItems as a (a.id)}
                  {@const dest = activityDest(a)}
                  {@const ident = projectIdent(a.project_id)}
                  <li>
                    <button
                      type="button"
                      disabled={!dest}
                      class="w-full flex items-start gap-3 px-1 py-2 rounded-md text-body-sm leading-snug text-left
                             {dest ? 'hover:bg-[var(--chart-hover)]' : ''}
                             transition-colors disabled:cursor-default min-w-0"
                      onclick={() => dest && navigate(dest)}
                    >
                      <span class="size-7 shrink-0 rounded-lg bg-[var(--chart-tile)] flex items-center justify-center text-[var(--text-muted)]">
                        {#if a.actor_is_bot}
                          <Bot size={14} />
                        {:else}
                          <User size={14} />
                        {/if}
                      </span>
                      <span class="flex-1 min-w-0 pt-1 text-[var(--text-muted)]">
                        <span class="font-medium text-[var(--text)]">{activityActor(a)}</span>
                        {activityVerb(a)}
                        {#if ident}
                          <span class="font-mono text-caption text-[var(--accent)]">
                            {a.entity_label ?? `${ident} #${a.entity_id}`}
                          </span>
                        {/if}
                      </span>
                    </button>
                  </li>
                {/each}
              </ol>
            {/if}
          </section>

          {#if pinnedPages.length > 0}
            <section>
              {@render sectionHeading("Pinned pages", pinnedPages.length)}
              <div class="flex flex-col border-t border-[var(--border)]">
                {#each pinnedPages as page (page.id)}
                  {@const ident = projectIdent(page.project_id)}
                  {#if ident}
                    <button
                      class="w-full flex items-center gap-2.5 px-1 py-2 text-left border-b border-[var(--border)]
                             hover:bg-[var(--chart-hover)] transition-colors"
                      onclick={() => navigate(`/${ident}/pages/${page.id}`)}
                    >
                      <FileText size={14} class="shrink-0 text-[var(--text-faint)]" />
                      <span class="flex-1 min-w-0 text-body-sm text-[var(--text)] truncate">{page.title}</span>
                      <span class="shrink-0 text-micro text-[var(--text-faint)] font-mono">{ident}</span>
                    </button>
                  {/if}
                {/each}
              </div>
            </section>
          {/if}
        </div>
      </div>
    {/if}
  </div>
</div>

<style>
  /* Chart ink shared by the Home charts, in the spirit of the agent activity
     page but built from the theme so accents and light/dark still apply. */
  .home {
    --chart-grid: color-mix(in oklab, var(--text) 7%, transparent);
    --chart-axis: color-mix(in oklab, var(--text) 22%, transparent);
    --chart-hover: color-mix(in oklab, var(--text) 4.5%, transparent);
    --chart-tile: color-mix(in oklab, var(--text) 7.5%, transparent);
  }
  /* Two columns split by a hairline on wide screens; stacked, the second
     column gets a rule above it instead. */
  .home-split {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
  }
  .home-aside {
    border-top: 1px solid var(--border);
  }
  @media (min-width: 1280px) {
    .home-split {
      grid-template-columns: minmax(0, 1fr) minmax(320px, 400px);
    }
    .home-split > :first-child {
      padding-right: 2rem;
    }
    .home-aside {
      border-top: 0;
      border-left: 1px solid var(--border);
      padding-left: 2rem;
    }
  }
</style>
