<script lang="ts">
  // LIF-237 — Home: the "My Work" landing dashboard. The new root route
  // ("/"), replacing the old redirect-to-Settings default.
  //
  // Five sections, none of which needed a new backend endpoint except one
  // (see api.ts's `listAllPages` for why): a cross-project view of active
  // work, a client-side "recently viewed" trail, pinned pages, a compact
  // activity digest, and quick actions. Read the per-section comments below
  // for the specific API-shape decisions (pinned pages, activity digest).

  import {
    me,
    listProjects,
    listIssues,
    listAllPages,
    listProjectActivity,
    type AuthUser,
    type Project,
    type Issue,
    type Page,
    type Activity,
  } from "../lib/api";
  import { getRecents, type RecentEntry } from "../lib/home/recents";
  import {
    selectActivityRate,
    type ActivityCountsReader,
  } from "../lib/activityRate";
  import StatusIcon from "../lib/StatusIcon.svelte";
  import PriorityIcon from "../lib/PriorityIcon.svelte";
  import ProjectIcon from "../lib/ProjectIcon.svelte";
  import Mascot from "../lib/Mascot.svelte";
  import ErrorState from "../lib/ErrorState.svelte";
  import Skeleton from "../lib/Skeleton.svelte";
  import { PRIORITIES } from "../lib/issues/grouping";
  import {
    Plus,
    Command,
    Pin,
    History,
    FileText,
    ListChecks,
    ChevronRight,
    ArrowUpRight,
    CircleDot,
    Sunrise,
    Sun,
    Sunset,
    Moon,
  } from "lucide-svelte";
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
  }: {
    navigate: (path: string) => void;
    realtimeActivityCounts: ActivityCountsReader;
  } = $props();

  let user = $state<AuthUser | null>(null);
  let projects = $state<Project[]>([]);
  let myIssues = $state<Issue[]>([]);
  let allPages = $state<Page[]>([]);
  let activityItems = $state<Activity[]>([]);
  let activityNow = $state(Date.now());
  let recents = $state<RecentEntry[]>([]);
  let loading = $state(true);
  let error = $state("");

  // The projects backing the activity digest (see `topProjectIds` below),
  // reused as the destination for the "New issue" quick action so it lands
  // wherever the user has been most active rather than an arbitrary project.
  let digestProjectIds = $state<number[]>([]);

  // Cadence uses a one-second window, so it needs a finer-grained clock than
  // the shared 30-second relative-time heartbeat. Keep this timer local to
  // Home so other timestamp displays do not rerender every second.
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
    recents = getRecents();

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

    // Cross-project "my active issues": two calls (status has no OR filter
    // server-side) with no project_id, which the API filters to visible
    // projects for us (LIF-197). Capped generously — grouping/display
    // below applies its own per-project cap.
    const [activeRes, todoRes, pagesRes] = await Promise.all([
      listIssues({ status: "active", limit: 200 }),
      listIssues({ status: "todo", limit: 200 }),
      listAllPages(),
    ]);
    myIssues = [
      ...(activeRes.ok ? activeRes.data : []),
      ...(todoRes.ok ? todoRes.data : []),
    ];
    allPages = pagesRes.ok ? pagesRes.data : [];

    // Activity digest: there's no cross-project activity feed, so we
    // aggregate the 3 projects with the most recent active/todo issue
    // activity (falling back to the 3 most recently touched projects when
    // nobody has active work) and merge their per-project feeds
    // client-side — 3 requests, ~10 rows shown. A per-project count
    // summary would be cheaper but far less useful ("what changed" beats
    // "how much changed" for a landing page); this is the judgment call
    // called out in the task.
    digestProjectIds = topProjectIds(myIssues, projects);
    if (digestProjectIds.length > 0) {
      const feeds = await Promise.all(
        digestProjectIds.map((pid) => listProjectActivity(pid, 8, 0)),
      );
      const combined = feeds.flatMap((f) => (f.ok ? f.data.items : []));
      combined.sort((a, b) => b.ts.localeCompare(a.ts));
      activityItems = combined.slice(0, 10);
    } else {
      activityItems = [];
    }

    if (initial) loading = false;
  }

  function topProjectIds(issues: Issue[], projs: Project[]): number[] {
    const lastSeen = new Map<number, string>();
    for (const i of issues) {
      const prev = lastSeen.get(i.project_id);
      if (!prev || i.updated_at > prev) lastSeen.set(i.project_id, i.updated_at);
    }
    if (lastSeen.size > 0) {
      return [...lastSeen.entries()]
        .sort((a, b) => b[1].localeCompare(a[1]))
        .slice(0, 3)
        .map(([pid]) => pid);
    }
    return [...projs]
      .sort((a, b) => b.updated_at.localeCompare(a.updated_at))
      .slice(0, 3)
      .map((p) => p.id);
  }

  // ── My active issues, grouped by project ─────────────────────

  const STATUS_RANK: Record<string, number> = { active: 0, todo: 1 };
  const PRIORITY_RANK: Record<string, number> = Object.fromEntries(
    PRIORITIES.map((p, i) => [p, i]),
  );
  const GROUP_CAP = 6;

  function compareIssues(a: Issue, b: Issue): number {
    const s = (STATUS_RANK[a.status] ?? 9) - (STATUS_RANK[b.status] ?? 9);
    if (s !== 0) return s;
    const p = (PRIORITY_RANK[a.priority] ?? 9) - (PRIORITY_RANK[b.priority] ?? 9);
    if (p !== 0) return p;
    return b.updated_at.localeCompare(a.updated_at);
  }

  let issueGroups = $derived.by(() => {
    const byProject = new Map<number, Issue[]>();
    for (const i of myIssues) {
      const list = byProject.get(i.project_id);
      if (list) list.push(i);
      else byProject.set(i.project_id, [i]);
    }
    const groups: { project: Project; visible: Issue[]; total: number }[] = [];
    for (const [pid, list] of byProject) {
      const project = projects.find((p) => p.id === pid);
      if (!project) continue; // visible per authz but not in our project list yet — skip rather than crash
      list.sort(compareIssues);
      groups.push({ project, visible: list.slice(0, GROUP_CAP), total: list.length });
    }
    groups.sort((a, b) => b.total - a.total || a.project.name.localeCompare(b.project.name));
    return groups;
  });

  // ── Pinned pages ──────────────────────────────────────────────
  //
  // See api.ts's `listAllPages` doc comment for why this is one
  // cross-project call filtered client-side rather than N per-project
  // calls or a server-side ?pinned= filter (which doesn't exist yet).

  let pinnedPages = $derived.by(() =>
    allPages
      .filter((p) => p.pinned && p.project_id !== null)
      .sort((a, b) => b.updated_at.localeCompare(a.updated_at))
      .slice(0, 8),
  );

  function projectIdent(id: number | null): string | null {
    if (id === null) return null;
    return projects.find((p) => p.id === id)?.identifier ?? null;
  }

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
      default:
        return a.action;
    }
  }

  function activityActor(a: Activity): string {
    return a.actor_display_name || a.actor_username || "system";
  }

  let activityRate = $derived(selectActivityRate(realtimeActivityCounts(activityNow)));

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

  // ── Recently viewed ────────────────────────────────────────────

  function recentDest(e: RecentEntry): string {
    const seg = e.type === "issue" ? "issues" : e.type === "page" ? "pages" : "plans";
    return `/${e.project}/${seg}/${e.routeId}`;
  }

  function recentRelative(ts: number): string {
    const diffMs = Date.now() - ts;
    const diffMins = Math.floor(diffMs / 60000);
    const diffHrs = Math.floor(diffMs / 3600000);
    const diffDays = Math.floor(diffMs / 86400000);
    if (diffMins < 1) return "just now";
    if (diffMins < 60) return `${diffMins}m ago`;
    if (diffHrs < 24) return `${diffHrs}h ago`;
    if (diffDays < 7) return `${diffDays}d ago`;
    return new Date(ts).toLocaleDateString("en-US", { month: "short", day: "numeric" });
  }

  // ── Greeting + quick actions ────────────────────────────────────

  function greetingText(h: number): string {
    if (h < 5) return "Good night";
    if (h < 12) return "Good morning";
    if (h < 17) return "Good afternoon";
    if (h < 21) return "Good evening";
    return "Good night";
  }

  function greetingIcon(h: number) {
    if (h < 5) return Moon;
    if (h < 12) return Sunrise;
    if (h < 17) return Sun;
    if (h < 21) return Sunset;
    return Moon;
  }

  const nowHour = new Date().getHours();
  const greeting = greetingText(nowHour);
  const GreetIcon = greetingIcon(nowHour);
  const todayLabel = new Date().toLocaleDateString("en-US", {
    weekday: "long",
    month: "long",
    day: "numeric",
  });

  // "New issue" lands in whichever project the digest picked as most
  // active; falls back to the first visible project, and is hidden
  // entirely (see markup) when there are no projects at all.
  let quickIssueProject = $derived.by(() => {
    const pid = digestProjectIds[0];
    if (pid !== undefined) {
      const p = projects.find((pr) => pr.id === pid);
      if (p) return p.identifier;
    }
    return projects[0]?.identifier ?? null;
  });

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

<div class="h-full flex flex-col">
  <div class="flex-1 overflow-y-auto">
    {#if loading}
      <!-- LIF-281: mirrors the two-column dashboard shape (greeting +
           active-issue sections on the left, recents/pinned/activity rail
           on the right) instead of a centered spinner. Aligned to the loaded
           markup below: the greeting row now uses the same
           `justify-between … mb-8` as the real hero (with a right-side
           quick-actions stand-in), the main column carries the "My active
           issues" section header (`mb-3`) so the first section card lands at
           the same y-position as loaded, and the issue rows use `py-2` to
           match the real issue-row padding. -->
      <div class="max-w-[1280px] mx-auto px-6 md:px-8 py-8 md:py-10">
        <div class="flex flex-wrap items-start justify-between gap-4 mb-8">
          <div class="flex items-center gap-3">
            <Skeleton variant="circle" class="size-11 rounded-xl" />
            <div class="flex flex-col gap-2">
              <Skeleton variant="bar" class="h-5 w-48" />
              <Skeleton variant="bar" class="h-3 w-32" />
            </div>
          </div>
          <div class="flex items-center gap-2">
            <Skeleton variant="bar" class="h-8 w-28 rounded-md" />
            <Skeleton variant="bar" class="h-8 w-24 rounded-md" />
          </div>
        </div>
        <div class="flex flex-col lg:flex-row gap-8 items-start">
          <div class="flex-1 min-w-0 w-full">
            <!-- "My active issues" section header (matches loaded mb-3) -->
            <div class="flex items-center gap-2 mb-3">
              <Skeleton variant="bar" class="h-3 w-28" />
              <Skeleton variant="bar" class="h-3 w-4" />
            </div>
            <div class="flex flex-col gap-5">
              {#each [0, 1] as section (section)}
                <div class="rounded-xl bg-[var(--surface)] border border-[var(--border)] overflow-hidden">
                  <div class="flex items-center gap-2 px-4 py-2.5 border-b border-[var(--border)]">
                    <Skeleton variant="circle" class="size-5 rounded-md" />
                    <Skeleton variant="bar" class="h-3.5 w-32" />
                  </div>
                  {#each [0, 1, 2] as row (row)}
                    <div class="flex items-center gap-2.5 px-4 py-2 border-b border-[var(--border)] last:border-b-0">
                      <Skeleton variant="circle" class="size-3.5" />
                      <Skeleton variant="bar" class="h-3 w-16 shrink-0" />
                      <Skeleton variant="bar" class="h-3 flex-1 max-w-[260px]" />
                    </div>
                  {/each}
                </div>
              {/each}
            </div>
          </div>
          <aside class="w-full lg:w-[340px] shrink-0 flex flex-col gap-8">
            {#each [4, 3, 3] as rows, section (section)}
              <div class="flex flex-col gap-2.5">
                <Skeleton variant="bar" class="h-3 w-28" />
                <div class="flex flex-col gap-1.5">
                  {#each Array(rows) as _, row (row)}
                    <Skeleton variant="bar" class="h-3 w-full" />
                  {/each}
                </div>
              </div>
            {/each}
          </aside>
        </div>
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
      <div class="max-w-[1280px] mx-auto px-6 md:px-8 py-8 md:py-10">
        <!-- ── GREETING + QUICK ACTIONS ─────────────────────── -->
        <div class="flex flex-wrap items-start justify-between gap-4 mb-8">
          <div class="flex items-center gap-3">
            <span
              class="size-11 shrink-0 rounded-xl bg-[var(--accent-subtle)]
                     flex items-center justify-center text-[var(--accent)]"
            >
              <GreetIcon size={20} />
            </span>
            <div>
              <h1 class="font-display text-title tracking-tight text-[var(--text)] leading-none">
                {greeting}{user ? `, ${user.display_name || user.username}` : ""}
              </h1>
              <p class="text-body-sm text-[var(--text-muted)] mt-1">{todayLabel}</p>
            </div>
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
                     px-3 py-1.5 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
              onclick={openPalette}
              title="Open command palette"
            >
              <Command size={13} />
              Jump to…
              <kbd class="font-mono text-micro leading-none text-[var(--text-faint)]
                          border border-[var(--border)] rounded px-1 py-0.5 ml-0.5">⌘K</kbd>
            </button>
          </div>
        </div>

        <div class="flex flex-col lg:flex-row gap-8 items-start">
          <!-- ── MY ACTIVE ISSUES ────────────────────────────── -->
          <div class="flex-1 min-w-0 w-full">
            <div class="flex items-center gap-2 mb-3">
              <h2 class="text-micro font-semibold uppercase tracking-widest text-[var(--text-muted)]">
                My active issues
              </h2>
              <span class="text-micro text-[var(--text-faint)] tabular-nums">
                {myIssues.length}
              </span>
            </div>

            {#if issueGroups.length === 0}
              <div
                class="flex flex-col items-center justify-center py-16 gap-3 rounded-xl
                       bg-[var(--surface)] border border-[var(--border)]"
              >
                <Mascot src="/LizzySleep2.png" nativeW={1000} nativeH={420} scale={0.18} />
                <p class="text-body text-[var(--text)] font-medium">All quiet here</p>
                <p class="text-body-sm text-[var(--text-muted)] text-center max-w-[36ch]">
                  Nothing active or todo assigned to you across your projects
                  right now.
                </p>
              </div>
            {:else}
              <div class="flex flex-col gap-5">
                {#each issueGroups as group (group.project.id)}
                  <section
                    class="rounded-xl bg-[var(--surface)] border border-[var(--border)] overflow-hidden"
                  >
                    <button
                      class="w-full flex items-center gap-2 px-4 py-2.5 border-b border-[var(--border)]
                             text-left hover:bg-[var(--bg-subtle)] transition-colors"
                      onclick={() => navigate(`/${group.project.identifier}/overview`)}
                    >
                      {#if group.project.emoji}
                        <ProjectIcon value={group.project.emoji} size={15} />
                      {:else}
                        <span
                          class="size-5 rounded-md border border-[var(--border)] bg-[var(--bg-subtle)]
                                 flex items-center justify-center text-micro font-semibold
                                 text-[var(--text-muted)] shrink-0"
                        >
                          {group.project.identifier.slice(0, 2)}
                        </span>
                      {/if}
                      <span class="text-body-sm font-medium text-[var(--text)] truncate flex-1">
                        {group.project.name}
                      </span>
                      <span class="text-micro text-[var(--text-faint)] tabular-nums">
                        {group.total}
                      </span>
                    </button>

                    {#each group.visible as issue (issue.id)}
                      <button
                        class="w-full flex items-center gap-2.5 px-4 py-2 text-left
                               border-b border-[var(--border)] last:border-b-0
                               hover:bg-[var(--bg-subtle)] transition-colors"
                        onclick={() => navigate(`/${group.project.identifier}/issues/${issue.identifier}`)}
                      >
                        <StatusIcon status={issue.status} size={14} />
                        <span class="text-caption font-mono text-[var(--text-faint)] w-[64px] shrink-0 truncate">
                          {issue.identifier}
                        </span>
                        <span
                          class="text-body-sm text-[var(--text)] truncate flex-1"
                        >
                          {issue.title}
                        </span>
                        <PriorityIcon priority={issue.priority} size={15} />
                      </button>
                    {/each}

                    {#if group.total > group.visible.length}
                      <button
                        class="w-full flex items-center justify-center gap-1 px-4 py-2
                               text-caption text-[var(--text-muted)] hover:text-[var(--text)]
                               hover:bg-[var(--bg-subtle)] transition-colors"
                        onclick={() => navigate(`/${group.project.identifier}/issues`)}
                      >
                        View all {group.total} in {group.project.identifier}
                        <ArrowUpRight size={11} />
                      </button>
                    {/if}
                  </section>
                {/each}
              </div>
            {/if}
          </div>

          <!-- ── RIGHT RAIL ───────────────────────────────────── -->
          <aside class="w-full lg:w-[340px] shrink-0 flex flex-col gap-8 lg:sticky lg:top-6">
            <!-- Recently viewed -->
            {#if recents.length > 0}
              <section>
                <div class="flex items-center gap-2 mb-3">
                  <History size={12} class="text-[var(--text-faint)]" />
                  <h2 class="text-micro font-semibold uppercase tracking-widest text-[var(--text-muted)]">
                    Recently viewed
                  </h2>
                </div>
                <div class="flex flex-col gap-0.5">
                  {#each recents.slice(0, 8) as r (r.type + r.routeId)}
                    <button
                      class="w-full flex items-center gap-2 px-2.5 py-1.5 rounded-md text-left
                             hover:bg-[var(--bg-subtle)] transition-colors"
                      onclick={() => navigate(recentDest(r))}
                    >
                      {#if r.type === "issue"}
                        <CircleDot size={13} class="shrink-0 text-[var(--text-faint)]" />
                      {:else if r.type === "page"}
                        <FileText size={13} class="shrink-0 text-[var(--text-faint)]" />
                      {:else}
                        <ListChecks size={13} class="shrink-0 text-[var(--text-faint)]" />
                      {/if}
                      <span class="flex-1 min-w-0 text-body-sm text-[var(--text)] truncate">
                        {r.title}
                      </span>
                      <span class="shrink-0 text-micro text-[var(--text-faint)] font-mono">
                        {r.project}
                      </span>
                    </button>
                  {/each}
                </div>
              </section>
            {/if}

            <!-- Pinned pages -->
            {#if pinnedPages.length > 0}
              <section>
                <div class="flex items-center gap-2 mb-3">
                  <Pin size={12} class="text-[var(--text-faint)]" />
                  <h2 class="text-micro font-semibold uppercase tracking-widest text-[var(--text-muted)]">
                    Pinned pages
                  </h2>
                </div>
                <div class="flex flex-col gap-0.5">
                  {#each pinnedPages as page (page.id)}
                    {@const ident = projectIdent(page.project_id)}
                    {#if ident}
                      <button
                        class="w-full flex items-center gap-2 px-2.5 py-1.5 rounded-md text-left
                               hover:bg-[var(--bg-subtle)] transition-colors"
                        onclick={() => navigate(`/${ident}/pages/${page.id}`)}
                      >
                        <FileText size={13} class="shrink-0 text-[var(--text-faint)]" />
                        <span class="flex-1 min-w-0 text-body-sm text-[var(--text)] truncate">
                          {page.title}
                        </span>
                        <span class="shrink-0 text-micro text-[var(--text-faint)] font-mono">
                          {ident}
                        </span>
                      </button>
                    {/if}
                  {/each}
                </div>
              </section>
            {/if}

            <!-- Activity digest -->
            {#if activityItems.length > 0}
              <section>
                <div class="flex items-center justify-between mb-3">
                  <div class="flex items-center gap-2 min-w-0">
                    <ArrowUpRight size={12} class="text-[var(--text-faint)]" />
                    <h2 class="text-micro font-semibold uppercase tracking-widest text-[var(--text-muted)]">
                      Recent activity
                    </h2>
                  </div>
                  <span
                    class="text-micro text-[var(--text-faint)] tabular-nums text-right"
                    title="Live websocket activity observed this session; counts use 1-second, 1-minute, 1-hour, or 1-day windows"
                  >
                    {activityRate.value} {activityRate.unit}
                  </span>
                </div>
                <div class="flex flex-col gap-0.5">
                  {#each activityItems as a (a.id)}
                    {@const dest = activityDest(a)}
                    {@const ident = projectIdent(a.project_id)}
                    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
                    <div
                      class="flex items-start gap-2 px-2.5 py-1.5 rounded-md text-body-sm leading-snug
                             {dest ? 'cursor-pointer hover:bg-[var(--bg-subtle)]' : ''} transition-colors"
                      role={dest ? "button" : undefined}
                      tabindex={dest ? 0 : undefined}
                      onclick={() => dest && navigate(dest)}
                    >
                      <span class="flex-1 min-w-0 text-[var(--text-muted)]">
                        <span class="font-medium text-[var(--text)]">{activityActor(a)}</span>
                        {activityVerb(a)}
                        {#if ident}
                          <span class="font-mono text-caption text-[var(--accent)]">
                            {a.entity_label ?? `${ident} #${a.entity_id}`}
                          </span>
                        {/if}
                      </span>
                    </div>
                  {/each}
                </div>
              </section>
            {/if}
          </aside>
        </div>
      </div>
    {/if}
  </div>
</div>
