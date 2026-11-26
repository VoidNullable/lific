<script lang="ts">
  // The project overview: how the project is doing and what to do next, in
  // the same visual language as Home. Identity up top, a row of pulse
  // numbers over the status mix, then three bands of two columns: the work
  // that matters now beside the open-work trend, the project's structure
  // (modules, plans, pages), and what people and agents have been doing.
  // Everything that changes the project rather than describing it lives on
  // the Settings tab (ProjectSettings.svelte).
  import {
    listProjects,
    listIssues,
    listLabels,
    listModules,
    listPlans,
    listPages,
    listProjectActivity,
    listProjectMembers,
    listProjectRelations,
    getIssueCounts,
    getHomeOverview,
    getInsights,
    downloadProjectExport,
    updateProject,
    type Activity,
    type HomeOverview,
    type InsightsPayload,
    type Issue,
    type IssueStatusCounts,
    type Label,
    type Module,
    type Page,
    type Plan,
    type Project,
    type ProjectMember,
    type ProjectRelation,
  } from "../lib/api";
  import { getContext } from "svelte";
  import {
    ArrowRight, Bot, Check, Download, FileText, History, Layers, ListChecks,
    Pencil, Pin, Plus, Globe, Lock,
  } from "lucide-svelte";
  import ProjectTopbar from "../lib/overview/ProjectTopbar.svelte";
  import StatusStrip from "../lib/overview/StatusStrip.svelte";
  import WeekBars from "../lib/overview/WeekBars.svelte";
  import { byDay, digest, type ActivityLine } from "../lib/overview/activity";
  import OpenTrend from "../lib/home/OpenTrend.svelte";
  import HeatBar from "../lib/home/HeatBar.svelte";
  import { recordProjectVisit } from "../lib/home/lastVisit";
  import ProjectIcon from "../lib/ProjectIcon.svelte";
  import IconPicker from "../lib/IconPicker.svelte";
  import ProgressRing from "../lib/ProgressRing.svelte";
  import StatusIcon from "../lib/StatusIcon.svelte";
  import PriorityIcon from "../lib/PriorityIcon.svelte";
  import AssigneeMarker from "../lib/issues/AssigneeMarker.svelte";
  import PersonAvatar from "../lib/issues/PersonAvatar.svelte";
  import CopyIdButton from "../lib/CopyIdButton.svelte";
  import SubTabs from "../lib/SubTabs.svelte";
  import TimeAgo from "../lib/TimeAgo.svelte";
  import ErrorState from "../lib/ErrorState.svelte";
  import Skeleton from "../lib/Skeleton.svelte";
  import Mascot from "../lib/Mascot.svelte";
  import { ageLabel, daysSince, heat, importanceScore } from "../lib/issues/importance";
  import { loadListState, saveListState, type PersistedListState } from "../lib/issues/persistence";
  import { saveSubTab } from "../lib/subtab";
  import { STATUS_UNRESOLVED } from "../lib/issues/grouping";
  import { localDay, waitState } from "../lib/issues/waits";
  import { safeLabelColor } from "../lib/labelColors";
  import { navLink } from "../lib/navLink";
  import { projectRole, loadProjectRole } from "../lib/projectRole.svelte";
  import { isSignedOut } from "../lib/signedOut";
  import SignInButton from "../lib/SignInButton.svelte";
  import { startAutoRefresh } from "../lib/autoRefresh.svelte";
  import { toast } from "../lib/toast/toast.svelte";

  const topbarCtx = getContext<{
    set: (s: import("svelte").Snippet | undefined) => void;
  } | undefined>("lific:topbar");
  $effect(() => {
    topbarCtx?.set(topbarContent);
    return () => topbarCtx?.set(undefined);
  });

  let {
    navigate,
    projectIdentifier,
    onProjectChange,
  }: {
    navigate: (path: string) => void;
    projectIdentifier: string;
    onProjectChange?: () => void;
  } = $props();

  let project = $state<Project | null>(null);
  let loading = $state(true);
  let error = $state("");

  let counts = $state<IssueStatusCounts | null>(null);
  let issues = $state<Issue[]>([]);
  let labels = $state<Label[]>([]);
  let modules = $state<Module[]>([]);
  let plans = $state<Plan[]>([]);
  let pages = $state<Page[]>([]);
  let activity = $state<Activity[]>([]);
  let members = $state<ProjectMember[]>([]);
  let relations = $state<ProjectRelation[]>([]);
  let overview = $state<HomeOverview | null>(null);
  let insights = $state<InsightsPayload | null>(null);
  /** When this visit to the project is measured from (null: first visit). */
  let visitSince = $state<number | null>(null);

  $effect(() => {
    const id = projectIdentifier;
    load(id);
  });

  const NO_ACCOUNT = { ok: false as const, error: "Sign in to see this.", status: 401 };

  async function load(ident: string, initial = true) {
    if (initial) {
      loading = true;
      error = "";
    }
    const projRes = await listProjects();
    if (!projRes.ok) {
      if (initial) { error = projRes.error; loading = false; }
      return;
    }
    const found = projRes.data.find((p) => p.identifier === ident);
    if (!found) {
      if (initial) { error = `Project ${ident} not found`; loading = false; }
      return;
    }
    if (ident !== projectIdentifier) return;
    if (initial) {
      loadProjectRole(found.id);
      visitSince = recordProjectVisit(found.id);
    }
    const since = visitSince === null ? null : new Date(visitSince).toISOString();

    const [countsRes, issuesRes, labelsRes, modulesRes, plansRes, pagesRes, actRes, membersRes, overviewRes, insightsRes, relationsRes] =
      await Promise.all([
        getIssueCounts(found.id),
        listIssues({ project_id: found.id, limit: 1000 }),
        listLabels(found.id),
        listModules(found.id),
        listPlans(found.id, "active"),
        listPages(found.id, undefined, undefined, undefined, { order_by: "updated", order: "desc" }),
        listProjectActivity(found.id, 60),
        // LIF-583: the member list and the per-reader digest need an
        // account; a signed-out reader's overview goes without them.
        signedOut ? Promise.resolve(NO_ACCOUNT) : listProjectMembers(found.id),
        signedOut ? Promise.resolve(NO_ACCOUNT) : getHomeOverview(since, found.id),
        getInsights(found.id, 12),
        listProjectRelations(found.id),
      ]);
    // A slow load for a project the user already left must not paint over
    // the one they are looking at now.
    if (ident !== projectIdentifier) return;
    project = found;
    if (countsRes.ok) counts = countsRes.data;
    if (issuesRes.ok) issues = issuesRes.data;
    if (labelsRes.ok) labels = labelsRes.data;
    if (modulesRes.ok) modules = modulesRes.data;
    if (plansRes.ok) plans = plansRes.data;
    if (pagesRes.ok) pages = pagesRes.data;
    if (actRes.ok) activity = actRes.data.items;
    members = membersRes.ok ? membersRes.data : [];
    overview = overviewRes.ok ? overviewRes.data : null;
    insights = insightsRes.ok ? insightsRes.data : null;
    relations = relationsRes.ok ? relationsRes.data : [];
    if (initial) loading = false;
  }

  $effect(() =>
    startAutoRefresh({
      refresh: () => load(projectIdentifier, false),
      isBusy: () => loading || editingName || editingDesc,
      realtimeDebounceMs: 750,
      realtimeMaxWaitMs: 5_000,
      shouldRefresh: (event) =>
        event.type === "resync.required" ||
        (typeof event.project_id === "number" && event.project_id === project?.id),
    }),
  );

  const canManage = $derived(projectRole.canManage);
  // LIF-583: read once; App remounts the routes when this changes.
  const signedOut = isSignedOut();

  // ── Identity: inline name and description edits ───────────────
  let editingName = $state(false);
  let draftName = $state("");
  let editingDesc = $state(false);
  let draftDesc = $state("");
  let savedAt = $state(0);

  async function saveField(field: "name" | "description" | "emoji", value: string | null) {
    if (!project) return;
    const res = await updateProject(project.id, { [field]: value });
    if (!res.ok) {
      toast(`Couldn't save changes: ${res.error}`, { kind: "error" });
      return;
    }
    project = res.data;
    onProjectChange?.();
    savedAt = Date.now();
    window.setTimeout(() => { if (Date.now() - savedAt >= 1900) savedAt = 0; }, 2000);
  }
  function commitName() {
    editingName = false;
    const v = draftName.trim();
    if (project && v && v !== project.name) saveField("name", v);
  }
  function commitDesc() {
    editingDesc = false;
    const v = draftDesc.trim();
    if (project && v !== project.description) saveField("description", v);
  }

  // ── Derived views of the issue set ────────────────────────────
  const OPEN = new Set(["backlog", "todo", "active"]);
  const openIssues = $derived(issues.filter((i) => OPEN.has(i.status)));
  const byId = $derived(new Map(issues.map((i) => [i.id, i])));
  /** Blockers of each issue, by the blocked issue's id. Only edges inside
   *  the project: the list endpoint doesn't carry relations. */
  const blockersOf = $derived.by(() => {
    const m = new Map<number, number[]>();
    for (const r of relations) {
      if (r.relation_type !== "blocks") continue;
      m.set(r.target_id, [...(m.get(r.target_id) ?? []), r.source_id]);
    }
    return m;
  });
  const today = localDay();

  /** Why an open issue cannot move: an open blocker or a wait that holds.
   *  A blocker holds until it is done, as on the server. */
  function holdOf(issue: Issue): { blockers: Issue[]; waiting: boolean } {
    const blockers = (blockersOf.get(issue.id) ?? [])
      .map((id) => byId.get(id))
      .filter((b): b is Issue => !!b && b.status !== "done");
    const waiting = (issue.waits ?? []).some((w) => waitState(w, today) === "holding");
    return { blockers, waiting };
  }

  type WorkTab = "attention" | "progress" | "ready" | "person" | "blocked";
  let workTab = $state<WorkTab>("attention");
  let workExpanded = $state(false);
  const WORK_CAP = 8;

  const work = $derived.by(() => {
    const now = Date.now();
    const scored = openIssues
      .map((issue) => ({ issue, score: importanceScore(issue, now), hold: holdOf(issue) }))
      .sort((a, b) => b.score - a.score);
    const held = (h: { blockers: Issue[]; waiting: boolean }) => h.blockers.length > 0 || h.waiting;
    return {
      attention: scored,
      progress: scored.filter((s) => s.issue.status === "active"),
      ready: scored.filter((s) => !s.issue.needs_human && s.issue.status !== "active" && !held(s.hold)),
      person: scored.filter((s) => !!s.issue.needs_human),
      blocked: scored.filter((s) => held(s.hold)),
    };
  });

  const workTabs = $derived([
    { id: "attention", label: "Needs attention", count: work.attention.length },
    { id: "progress", label: "In progress", count: work.progress.length },
    { id: "ready", label: "Ready for agents", count: work.ready.length },
    { id: "person", label: "For a person", count: work.person.length },
    { id: "blocked", label: "Blocked", count: work.blocked.length },
  ]);
  const workRows = $derived(work[workTab]);

  const WORK_NOTES: Record<WorkTab, string> = {
    attention: "Open work ranked by priority, age and time since it last moved.",
    progress: "Active right now, the longest untouched first.",
    ready: "Unassigned and unblocked: what an agent can pick up next.",
    person: "Assigned to someone or marked for any person.",
    blocked: "Held by an open blocker or a wait that hasn't come due.",
  };

  let workSection = $state<HTMLElement | null>(null);
  function showWork(tab: WorkTab) {
    workTab = tab;
    workExpanded = false;
    workSection?.scrollIntoView({ behavior: "smooth", block: "start" });
  }

  // ── Pulse numbers ─────────────────────────────────────────────
  const weekDelta = $derived.by(() => {
    const t = overview?.open_trend ?? [];
    if (t.length < 8) return null;
    return t[t.length - 1].count - t[t.length - 8].count;
  });
  const closedWeeks = $derived(insights?.closed_per_week ?? []);
  const closedRecent = $derived(closedWeeks.slice(-4).reduce((n, w) => n + w.count, 0));
  const longestIdle = $derived(
    work.progress.reduce((m, s) => Math.max(m, daysSince(s.issue.updated_at)), 0),
  );
  const total = $derived(counts?.total ?? 0);
  const blank = $derived(total === 0 && modules.length === 0 && plans.length === 0 && pages.length === 0);
  const done = $derived(counts?.done ?? 0);
  const completion = $derived(total > 0 ? done / total : 0);

  // ── Modules ───────────────────────────────────────────────────
  const MODULE_CAP = 7;
  const moduleRows = $derived.by(() => {
    const tally = new Map<number, { total: number; done: number; open: number; active: number }>();
    for (const i of issues) {
      if (i.module_id === null || i.status === "cancelled") continue;
      const t = tally.get(i.module_id) ?? { total: 0, done: 0, open: 0, active: 0 };
      t.total++;
      if (i.status === "done") t.done++;
      else t.open++;
      if (i.status === "active") t.active++;
      tally.set(i.module_id, t);
    }
    const rank = (m: Module) => (m.status === "done" || m.status === "cancelled" ? 1 : 0);
    return modules
      .map((m) => ({ module: m, ...(tally.get(m.id) ?? { total: 0, done: 0, open: 0, active: 0 }) }))
      .sort((a, b) => rank(a.module) - rank(b.module) || b.open - a.open || a.module.name.localeCompare(b.module.name));
  });

  // ── Pages: pinned first, then the most recently touched ────────
  const PAGE_CAP = 6;
  const pageRows = $derived.by(() => {
    const pinned = pages.filter((p) => p.pinned);
    const rest = pages.filter((p) => !p.pinned && p.status !== "archived");
    return [...pinned, ...rest].slice(0, PAGE_CAP);
  });

  // ── People ────────────────────────────────────────────────────
  const lead = $derived(
    project?.lead_user_id == null ? null : (members.find((m) => m.user_id === project!.lead_user_id) ?? null),
  );
  const actors = $derived((insights?.top_actors ?? []).slice(0, 6));
  const actorMax = $derived(Math.max(1, ...actors.map((a) => a.actions)));

  // ── Labels by open use ────────────────────────────────────────
  const labelRows = $derived.by(() => {
    const open = new Map<string, number>();
    for (const i of openIssues) for (const l of i.labels) open.set(l, (open.get(l) ?? 0) + 1);
    return labels
      .map((l) => ({ label: l, open: open.get(l.name) ?? 0 }))
      .sort((a, b) => b.open - a.open || a.label.name.localeCompare(b.label.name));
  });

  // ── Activity ──────────────────────────────────────────────────
  const ACTIVITY_CAP = 12;
  // The feed comes in write order; imported or backfilled rows can carry
  // older timestamps, and a day list should read in time order.
  const lines = $derived(
    digest([...activity].sort((a, b) => b.ts.localeCompare(a.ts) || b.id - a.id)).slice(0, ACTIVITY_CAP),
  );
  const days = $derived(byDay(lines));
  const lastActive = $derived(activity[0]?.ts ?? null);

  /** Titles for the identifiers activity lines name. */
  const titles = $derived.by(() => {
    const m = new Map<string, string>();
    for (const i of issues) m.set(i.identifier, i.title);
    for (const p of pages) m.set(p.identifier, p.title);
    for (const p of plans) m.set(p.identifier, p.title);
    return m;
  });

  function lineDest(line: ActivityLine): string | null {
    const a = line.source;
    const ident = projectIdentifier;
    if (a.entity_type === "plan" || a.entity_type === "plan_step") {
      const plan = plans.find((p) => p.identifier === a.entity_label);
      if (plan) return `/${ident}/plans/${plan.id}`;
      return a.entity_type === "plan" ? `/${ident}/plans/${a.entity_id}` : `/${ident}/plans`;
    }
    if (a.issue_id !== null && a.entity_label && a.entity_type !== "page") {
      return a.entity_type === "issue" || a.entity_type === "comment"
        ? `/${ident}/issues/${a.entity_label}`
        : null;
    }
    if (a.entity_type === "page") return `/${ident}/pages/${a.entity_id}`;
    if (a.page_id !== null) return `/${ident}/pages/${a.page_id}`;
    if (a.entity_type === "module" && a.action !== "delete") return `/${ident}/modules/${a.entity_id}`;
    return null;
  }

  const sinceLine = $derived.by(() => {
    const s = overview?.since;
    if (visitSince === null || !s) return null;
    const when = relative(visitSince);
    const parts: string[] = [];
    const agents = [s.agents_closed && `closed ${s.agents_closed}`, s.agents_opened && `opened ${s.agents_opened}`].filter(Boolean);
    const people = [s.people_closed && `closed ${s.people_closed}`, s.people_opened && `opened ${s.people_opened}`].filter(Boolean);
    if (agents.length) parts.push(`agents ${agents.join(" and ")}`);
    if (people.length) parts.push(`people ${people.join(" and ")}`);
    if (parts.length === 0) return `Quiet since you were last here, ${when}.`;
    return `Since you were last here (${when}): ${parts.join("; ")}.`;
  });

  function relative(ts: number): string {
    const mins = Math.floor((Date.now() - ts) / 60000);
    if (mins < 60) return `${Math.max(1, mins)}m ago`;
    const hrs = Math.floor(mins / 60);
    if (hrs < 24) return `${hrs}h ago`;
    const d = Math.floor(hrs / 24);
    return d === 1 ? "yesterday" : `${d} days ago`;
  }

  // ── Leaving for the issue list with a filter ──────────────────
  function openInList(filters: Partial<PersistedListState>) {
    if (!project) return;
    const s = loadListState(projectIdentifier);
    saveListState(projectIdentifier, {
      ...s,
      filterStatus: [],
      filterPriority: [],
      filterLabel: [],
      filterModule: [],
      filterAssignee: [],
      searchQuery: "",
      ...filters,
    });
    // The issue list remembers its All/Open/Closed slice; a filtered jump
    // should land on everything the filter admits.
    saveSubTab("issues", String(project.id), "all");
    navigate(`/${projectIdentifier}/issues`);
  }

  let exporting = $state(false);
  async function exportProject() {
    if (!project || exporting) return;
    exporting = true;
    const res = await downloadProjectExport(project.identifier);
    if (!res.ok) toast(`Couldn't export: ${res.error}`, { kind: "error" });
    exporting = false;
  }

  function createdLabel(iso: string): string {
    const d = new Date(iso.replace(" ", "T") + "Z");
    return d.toLocaleDateString("en-US", { month: "short", year: "numeric" });
  }
</script>

{#snippet topbarContent()}
  <ProjectTopbar identifier={projectIdentifier} current="overview" {navigate}>
    {#snippet actions()}
      {#if signedOut}
        <!-- LIF-583: export and creation need an account. -->
        <SignInButton />
      {:else}
        <button class="toolbar-pill" onclick={exportProject} disabled={exporting || !project}
                title="Download every issue and page as Markdown">
          <Download size={14} />
          <span class="hidden sm:inline">{exporting ? "Exporting…" : "Export"}</span>
        </button>
        {#if projectRole.canEdit}
          <a
            href="#/{projectIdentifier}/issues/new"
            use:navLink={navigate}
            class="flex items-center gap-1.5 h-8 px-3 rounded-md text-body-sm font-medium
                   text-[var(--btn-success-text)] bg-[var(--btn-success)] hover:bg-[var(--btn-success-hover)] transition-colors"
          >
            <Plus size={14} /> <span class="hidden sm:inline">New issue</span>
          </a>
        {/if}
      {/if}
    {/snippet}
  </ProjectTopbar>
{/snippet}

{#snippet heading(title: string, note?: string, link?: { href: string; label: string })}
  <div class="flex items-start justify-between gap-4 mb-4">
    <div class="min-w-0">
      <h2 class="text-[1.35rem] font-semibold tracking-tight leading-tight text-[var(--text)]">{title}</h2>
      {#if note}<p class="text-[0.95rem] text-[var(--text-muted)] mt-1">{note}</p>{/if}
    </div>
    {#if link}
      <a
        href="#{link.href}"
        use:navLink={navigate}
        class="shrink-0 mt-1 inline-flex items-center gap-1 text-caption text-[var(--text-muted)]
               hover:text-[var(--text)] transition-colors"
      >
        {link.label} <ArrowRight size={12} />
      </a>
    {/if}
  </div>
{/snippet}

{#snippet actorMark(isBot: boolean, userId: number | null, username: string | null, displayName: string | null)}
  {#if isBot}
    <span class="size-[22px] shrink-0 rounded-full grid place-items-center bg-[var(--chart-tile)] text-[var(--text-muted)]" title="Agent">
      <Bot size={12} />
    </span>
  {:else if userId !== null && username}
    <PersonAvatar person={{ user_id: userId, username, display_name: displayName }} size="sm" />
  {:else}
    <!-- No account: Lific itself (migrations, imports, scheduled work). -->
    <img src="/logo.webp" alt="" class="size-[22px] shrink-0 rounded-md" />
  {/if}
{/snippet}

{#snippet stat(label: string, value: string | number, onclick: () => void, sub?: import("svelte").Snippet, testid?: string)}
  <button
    class="stat group text-left px-4 py-3.5 rounded-lg transition-colors hover:bg-[var(--chart-hover)] min-w-0"
    data-testid={testid}
    {onclick}
  >
    <span class="block text-caption text-[var(--text-muted)] group-hover:text-[var(--text)] transition-colors">{label}</span>
    <span class="block text-[2rem] leading-tight font-semibold tracking-tight tabular-nums text-[var(--text)] mt-0.5">{value}</span>
    {#if sub}<span class="block mt-1 min-h-7 text-caption text-[var(--text-faint)]">{@render sub()}</span>{/if}
  </button>
{/snippet}

<div class="overview h-full overflow-y-auto">
  <div class="max-w-[1320px] mx-auto px-4 md:px-10 pt-7 pb-14">
    {#if loading}
      <div class="flex items-start gap-5 pb-6 border-b border-[var(--border)]">
        <Skeleton variant="block" class="size-14 rounded-2xl shrink-0" />
        <div class="flex-1 flex flex-col gap-2.5">
          <Skeleton variant="bar" class="h-8 w-56" />
          <Skeleton variant="bar" class="h-4 w-full max-w-[52ch]" />
          <Skeleton variant="bar" class="h-3.5 w-72" />
        </div>
      </div>
      <div class="py-6 border-b border-[var(--border)] grid grid-cols-2 md:grid-cols-5 gap-2">
        {#each [0, 1, 2, 3, 4] as i (i)}
          <div class="px-4 py-3.5 flex flex-col gap-2">
            <Skeleton variant="bar" class="h-3 w-20" />
            <Skeleton variant="bar" class="h-8 w-14" />
          </div>
        {/each}
      </div>
      <div class="pt-6 flex flex-col gap-4">
        <Skeleton variant="bar" class="h-6 w-48 mb-1" />
        {#each [0, 1, 2, 3, 4] as row (row)}
          <div class="flex items-center gap-3">
            <Skeleton variant="bar" class="h-3 w-16 shrink-0" />
            <Skeleton variant="bar" class="h-3 flex-1 max-w-[420px]" />
          </div>
        {/each}
      </div>
    {:else if !project}
      <ErrorState title="Couldn't load this project" message={error}>
        <button
          class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)] px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
          onclick={() => load(projectIdentifier)}
        >
          Try again
        </button>
        <button
          class="text-body-sm text-[var(--text-muted)] border border-[var(--border)] px-3 py-1.5 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
          onclick={() => navigate("/")}
        >
          Back to home
        </button>
      </ErrorState>
    {:else}
      <!-- ── IDENTITY ─────────────────────────────────────────── -->
      <header class="flex flex-wrap items-start gap-x-5 gap-y-4 pb-6 border-b border-[var(--border)]">
        <div class="shrink-0 hero-icon">
          {#if canManage}
            <IconPicker value={project.emoji ?? ""} onchange={(v) => saveField("emoji", v || null)} />
          {:else}
            <div class="size-12 grid place-items-center rounded-xl bg-[var(--chart-tile)]">
              {#if project.emoji}
                <ProjectIcon value={project.emoji} size={22} />
              {:else}
                <span class="text-body-sm font-mono font-semibold text-[var(--text-muted)]">{project.identifier.slice(0, 2)}</span>
              {/if}
            </div>
          {/if}
        </div>

        <div class="flex-1 min-w-[240px]">
          <div class="flex items-center gap-2.5 flex-wrap">
            {#if canManage && editingName}
              <!-- svelte-ignore a11y_autofocus -->
              <input
                bind:value={draftName}
                aria-label="Project name"
                class="text-[1.9rem] font-semibold tracking-tight leading-tight bg-transparent outline-none
                       text-[var(--text)] w-full max-w-[24ch] rounded px-1 -mx-1
                       focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
                autofocus
                onblur={commitName}
                onkeydown={(e) => {
                  if (e.key === "Enter") commitName();
                  if (e.key === "Escape") editingName = false;
                }}
              />
            {:else if canManage}
              <button
                class="group flex items-center gap-2 text-[1.9rem] font-semibold tracking-tight leading-tight
                       text-[var(--text)] rounded px-1 -mx-1 cursor-text hover:bg-[var(--chart-hover)] transition-colors"
                title="Rename"
                onclick={() => { draftName = project!.name; editingName = true; }}
              >
                <h1>{project.name}</h1>
                <Pencil size={15} class="text-[var(--text-faint)] opacity-0 group-hover:opacity-100 pointer-coarse:opacity-100 transition-opacity" />
              </button>
            {:else}
              <h1 class="text-[1.9rem] font-semibold tracking-tight leading-tight text-[var(--text)]">{project.name}</h1>
            {/if}
            <CopyIdButton
              value={project.identifier}
              label={project.identifier}
              iconSize={11}
              iconClass="opacity-0 group-hover:opacity-100 pointer-coarse:opacity-100 transition-opacity"
              class="group inline-flex items-center gap-1 text-caption font-mono font-semibold
                     px-1.5 py-0.5 rounded-md bg-[var(--chart-tile)] text-[var(--text-muted)]
                     hover:text-[var(--accent)] transition-colors"
            >
              {project.identifier}
            </CopyIdButton>
            <span
              class="inline-flex items-center gap-1 text-caption px-2 py-0.5 rounded-full
                     {project.is_public ? 'text-[var(--success)] bg-[var(--success-bg)]' : 'text-[var(--text-muted)] bg-[var(--chart-tile)]'}"
              title={project.is_public ? "Anyone can read this project, with no account" : "Only people with access can see this project"}
            >
              {#if project.is_public}<Globe size={11} /> Public{:else}<Lock size={11} /> Private{/if}
            </span>
            {#if savedAt}
              <span class="inline-flex items-center gap-1 text-caption text-[var(--success)]" aria-live="polite">
                <Check size={12} /> Saved
              </span>
            {/if}
          </div>

          {#if canManage && editingDesc}
            <!-- svelte-ignore a11y_autofocus -->
            <textarea
              bind:value={draftDesc}
              aria-label="Project description"
              rows="2"
              class="mt-2 w-full max-w-[72ch] text-[0.95rem] bg-transparent border border-[var(--border)] rounded-md px-2.5 py-1.5
                     text-[var(--text)] outline-none resize-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
              placeholder="What is this project for?"
              autofocus
              onblur={commitDesc}
              onkeydown={(e) => {
                if (e.key === "Escape") editingDesc = false;
                if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) commitDesc();
              }}
            ></textarea>
          {:else if canManage}
            <button
              class="group block text-left mt-1.5 max-w-[72ch] rounded px-1 -mx-1 cursor-text hover:bg-[var(--chart-hover)] transition-colors"
              onclick={() => { draftDesc = project!.description; editingDesc = true; }}
            >
              {#if project.description}
                <span class="text-[0.95rem] text-[var(--text-muted)] leading-relaxed">{project.description}</span>
              {:else}
                <span class="text-[0.95rem] text-[var(--text-faint)]">Add a description so people know what this project is for</span>
              {/if}
              <Pencil size={12} class="inline ml-1 text-[var(--text-faint)] opacity-0 group-hover:opacity-100 pointer-coarse:opacity-100 transition-opacity align-baseline" />
            </button>
          {:else if project.description}
            <p class="text-[0.95rem] text-[var(--text-muted)] leading-relaxed mt-1.5 max-w-[72ch]">{project.description}</p>
          {/if}

          <div class="flex flex-wrap items-center gap-x-4 gap-y-1.5 mt-3 text-body-sm text-[var(--text-muted)]">
            {#if lead}
              <span class="inline-flex items-center gap-1.5">
                <PersonAvatar person={lead} size="xs" />
                <span><span class="text-[var(--text)]">{lead.display_name || lead.username}</span> leads</span>
              </span>
            {/if}
            {#if members.length > 0}
              <a href="#/{projectIdentifier}/settings" use:navLink={navigate}
                 class="hover:text-[var(--text)] transition-colors tabular-nums">
                {members.length} {members.length === 1 ? "member" : "members"}
              </a>
            {/if}
            <span>Since {createdLabel(project.created_at)}</span>
            {#if lastActive}
              <span>Active <TimeAgo date={lastActive} /></span>
            {/if}
          </div>
          {#if sinceLine}
            <p class="mt-1.5 text-body-sm text-[var(--text-muted)]" data-testid="project-since-line">{sinceLine}</p>
          {/if}
        </div>

        {#if total > 0}
          <div class="shrink-0 flex items-center gap-3 self-center">
            <ProgressRing value={completion} size={64} stroke={6} color="var(--success)" />
            <div class="text-body-sm leading-snug">
              <p class="text-[var(--text)] font-medium tabular-nums">{done} of {total} done</p>
              <p class="text-[var(--text-faint)] tabular-nums">{counts?.cancelled ?? 0} cancelled</p>
            </div>
          </div>
        {/if}
      </header>

      {#if total === 0}
        <!-- ── EMPTY PROJECT ────────────────────────────────── -->
        <section class="flex flex-col items-center text-center py-16 gap-4">
          <Mascot src="/LizzyWriting.png" nativeW={603} nativeH={597} scale={0.3} />
          <div class="flex flex-col gap-1.5 max-w-[46ch]">
            <p class="text-heading font-medium text-[var(--text)]">A fresh project</p>
            <p class="text-[0.95rem] text-[var(--text-muted)] leading-relaxed">
              Once it has issues, this page shows what needs attention, how the work is trending,
              and what people and agents are doing.
            </p>
          </div>
          {#if projectRole.canEdit}
          <div class="flex flex-wrap justify-center gap-2 mt-1">
            <a href="#/{projectIdentifier}/issues/new" use:navLink={navigate}
               class="flex items-center gap-1.5 text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)]
                      px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors">
              <Plus size={14} /> Create the first issue
            </a>
            <a href="#/{projectIdentifier}/modules" use:navLink={navigate}
               class="flex items-center gap-1.5 text-body-sm text-[var(--text-muted)] border border-[var(--border)]
                      px-3 py-1.5 rounded-md hover:bg-[var(--chart-hover)] hover:text-[var(--text)] transition-colors">
              <Layers size={14} /> Set up modules
            </a>
            <a href="#/{projectIdentifier}/pages" use:navLink={navigate}
               class="flex items-center gap-1.5 text-body-sm text-[var(--text-muted)] border border-[var(--border)]
                      px-3 py-1.5 rounded-md hover:bg-[var(--chart-hover)] hover:text-[var(--text)] transition-colors">
              <FileText size={14} /> Write a page
            </a>
          </div>
          {/if}
        </section>
      {:else}
        <!-- ── PULSE ──────────────────────────────────────────── -->
        <section class="py-5 border-b border-[var(--border)]" aria-label="Project pulse">
          <div class="pulse-grid -mx-4">
            {#snippet openSub()}
              {#if weekDelta === null}
                &nbsp;
              {:else if weekDelta === 0}
                No change this week
              {:else}
                <span class={weekDelta > 0 ? "text-[var(--warn-text)]" : "text-[var(--success)]"}>
                  {weekDelta > 0 ? `+${weekDelta}` : weekDelta}
                </span> this week
              {/if}
            {/snippet}
            {#snippet progressSub()}
              {work.progress.length === 0 ? "Nothing active" : `Longest untouched ${ageLabel(longestIdle)}`}
            {/snippet}
            {#snippet closedSub()}
              {#if closedWeeks.length > 0}
                <WeekBars points={closedWeeks} label="Issues closed per week, last 12 weeks" />
              {:else}
                &nbsp;
              {/if}
            {/snippet}
            {#snippet readySub()}Unassigned and unblocked{/snippet}
            {#snippet personSub()}Assigned or marked for a person{/snippet}
            {@render stat("Open", openIssues.length, () => openInList({ filterStatus: [STATUS_UNRESOLVED] }), openSub, "pulse-open")}
            {@render stat("In progress", work.progress.length, () => showWork("progress"), progressSub, "pulse-progress")}
            {@render stat("Closed in 4 weeks", closedRecent, () => openInList({ filterStatus: ["done"] }), closedSub, "pulse-closed")}
            {@render stat("Ready for agents", work.ready.length, () => showWork("ready"), readySub, "pulse-ready")}
            {@render stat("For a person", work.person.length, () => showWork("person"), personSub, "pulse-person")}
          </div>
          {#if counts}
            <div class="mt-4">
              <StatusStrip {counts} onpick={(s) => openInList({ filterStatus: [s] })} />
            </div>
          {/if}
        </section>

        <!-- ── WORK + TREND ───────────────────────────────────── -->
        <div class="ov-split border-b border-[var(--border)]">
          <section class="py-6 min-w-0 scroll-mt-4" bind:this={workSection} data-testid="overview-work">
            {@render heading("Work", WORK_NOTES[workTab])}
            <SubTabs tabs={workTabs} active={workTab} onselect={(id) => { workTab = id as WorkTab; workExpanded = false; }} />
            {#if workRows.length === 0}
              <div class="flex items-center gap-3 py-8 px-1 text-[var(--text-muted)]">
                <span class="grid place-items-center size-8 rounded-full bg-[var(--success-bg)] text-[var(--success)]"><Check size={16} /></span>
                <p class="text-[0.95rem]">
                  {workTab === "blocked" ? "Nothing is blocked." :
                   workTab === "progress" ? "Nothing is in progress right now." :
                   workTab === "person" ? "Nothing is waiting on a person." :
                   workTab === "ready" ? "Nothing is ready for an agent." : "Nothing is open."}
                </p>
              </div>
            {:else}
              {@const shown = workExpanded ? workRows.slice(0, 40) : workRows.slice(0, WORK_CAP)}
              <ol class="flex flex-col">
                {#each shown as { issue, score, hold }, rank (issue.id)}
                  {@const idle = daysSince(issue.updated_at)}
                  <li>
                    <a
                      href="#/{projectIdentifier}/issues/{issue.identifier}"
                      use:navLink={navigate}
                      data-identifier={issue.identifier}
                      class="work-row w-full flex items-center gap-3 px-1 py-2.5 text-left border-b border-[var(--border)]
                             hover:bg-[var(--chart-hover)] transition-colors"
                    >
                      <span class="w-5 shrink-0 text-caption tabular-nums text-[var(--text-faint)] text-right">{rank + 1}</span>
                      <span class="w-12 shrink-0 hidden sm:block"><HeatBar value={heat(score)} /></span>
                      <StatusIcon status={issue.status} size={14} />
                      <span class="text-caption font-mono text-[var(--text-faint)] w-[62px] shrink-0 truncate">{issue.identifier}</span>
                      <span class="flex-1 min-w-0">
                        <span class="block text-body-sm text-[var(--text)] truncate">{issue.title}</span>
                        {#if workTab === "blocked"}
                          <span class="block text-caption text-[var(--text-muted)] truncate">
                            {#if hold.blockers.length > 0}
                              Blocked by {hold.blockers.map((b) => b.identifier).join(", ")}{hold.waiting ? ", and waiting" : ""}
                            {:else}
                              Waiting
                            {/if}
                          </span>
                        {/if}
                      </span>
                      <AssigneeMarker {issue} />
                      <PriorityIcon priority={issue.priority} size={15} />
                      <span class="shrink-0 hidden md:flex items-center justify-end gap-1.5 w-[118px] text-micro tabular-nums">
                        <span class="text-[var(--text-faint)]">{ageLabel(daysSince(issue.created_at))} old</span>
                        {#if idle >= 14}
                          <span class="px-1.5 py-0.5 rounded-full text-[var(--warn-text)] bg-[color-mix(in_oklab,var(--warn)_14%,transparent)]">idle {ageLabel(idle)}</span>
                        {:else}
                          <span class="px-1.5 py-0.5 text-[var(--text-faint)]">idle {ageLabel(idle)}</span>
                        {/if}
                      </span>
                    </a>
                  </li>
                {/each}
              </ol>
              <div class="flex items-center justify-between gap-3 pt-2.5 text-caption">
                {#if workRows.length > WORK_CAP}
                  <button class="text-[var(--text-muted)] hover:text-[var(--text)] transition-colors"
                          onclick={() => (workExpanded = !workExpanded)}>
                    {workExpanded ? "Show fewer" : `Show ${Math.min(workRows.length, 40) - WORK_CAP} more`}
                  </button>
                {:else}
                  <span></span>
                {/if}
                <button class="inline-flex items-center gap-1 text-[var(--text-muted)] hover:text-[var(--text)] transition-colors"
                        onclick={() => openInList({ filterStatus: [STATUS_UNRESOLVED] })}>
                  All open issues <ArrowRight size={12} />
                </button>
              </div>
            {/if}
          </section>

          <section class="ov-aside py-6 min-w-0 flex flex-col gap-8" data-testid="overview-trend">
            {#if overview}
              <div>
                {@render heading("Open over 90 days", `${overview.open_total} open now`)}
                <OpenTrend points={overview.open_trend} />
              </div>
            {/if}
            {#if insights}
              {@const created = insights.created_per_week.slice(-4).reduce((n, w) => n + w.count, 0)}
              <div>
                {@render heading("Last 4 weeks", undefined, { href: `/${projectIdentifier}/insights`, label: "Insights" })}
                <dl class="grid grid-cols-2 gap-3">
                  <div class="rounded-lg bg-[var(--chart-tile)] px-3.5 py-3">
                    <dt class="text-caption text-[var(--text-muted)]">Opened</dt>
                    <dd class="text-[1.5rem] font-semibold tabular-nums text-[var(--text)] leading-tight">{created}</dd>
                  </div>
                  <div class="rounded-lg bg-[var(--chart-tile)] px-3.5 py-3">
                    <dt class="text-caption text-[var(--text-muted)]">Closed</dt>
                    <dd class="text-[1.5rem] font-semibold tabular-nums text-[var(--text)] leading-tight">{closedRecent}</dd>
                  </div>
                </dl>
                <p class="mt-2.5 text-body-sm text-[var(--text-muted)]">
                  {#if closedRecent > created}
                    Closing faster than it fills up.
                  {:else if closedRecent === created}
                    Holding steady.
                  {:else}
                    Filling up faster than it closes.
                  {/if}
                </p>
              </div>
            {/if}
          </section>
        </div>
      {/if}

      <!-- A project with nothing in it yet stops at the empty state above,
           which already offers the first steps. -->
      {#if !blank}
      <!-- ── STRUCTURE: MODULES, PLANS, PAGES ─────────────────── -->
      <div class="ov-split border-b border-[var(--border)]">
        <section class="py-6 min-w-0" data-testid="overview-modules">
          {@render heading("Modules", modules.length ? "Progress by area, the busiest first." : undefined, { href: `/${projectIdentifier}/modules`, label: "All modules" })}
          {#if moduleRows.length === 0}
            <p class="text-[0.95rem] text-[var(--text-muted)]">
              No modules yet. Modules group issues by area so progress shows per part of the project.
            </p>
          {:else}
            <ul class="flex flex-col">
              {#each moduleRows.slice(0, MODULE_CAP) as row (row.module.id)}
                {@const pct = row.total > 0 ? row.done / row.total : 0}
                <li>
                  <a
                    href="#/{projectIdentifier}/modules/{row.module.id}"
                    use:navLink={navigate}
                    class="grid grid-cols-[20px_minmax(0,1fr)_auto] sm:grid-cols-[20px_minmax(7rem,1fr)_minmax(8rem,18rem)_auto] items-center gap-x-3 gap-y-1.5
                           px-1 py-2.5 border-b border-[var(--border)] hover:bg-[var(--chart-hover)] transition-colors"
                  >
                    <span class="grid place-items-center text-[var(--text-muted)]">
                      {#if row.module.emoji}<ProjectIcon value={row.module.emoji} size={15} />{:else}<Layers size={15} />{/if}
                    </span>
                    <span class="min-w-0 flex items-center gap-2">
                      <span class="text-body-sm text-[var(--text)] truncate">{row.module.name}</span>
                      {#if row.module.status !== "active"}
                        <span class="shrink-0 text-micro capitalize text-[var(--text-faint)] px-1.5 py-0.5 rounded bg-[var(--chart-tile)]">{row.module.status}</span>
                      {/if}
                    </span>
                    <span class="col-start-2 col-span-2 sm:col-start-auto sm:col-span-1 flex items-center gap-2.5 min-w-0">
                      <span class="relative flex-1 h-1.5 rounded-full bg-[var(--chart-tile)] overflow-hidden">
                        <span class="meter absolute inset-y-0 left-0 rounded-full bg-[var(--success)]" style="width: {pct * 100}%"></span>
                      </span>
                      <span class="w-9 text-right text-caption tabular-nums text-[var(--text-muted)]">{Math.round(pct * 100)}%</span>
                    </span>
                    <span class="row-start-1 col-start-3 sm:row-start-auto sm:col-start-auto sm:w-[124px] whitespace-nowrap text-right text-caption tabular-nums text-[var(--text-faint)]">
                      {#if row.active > 0}<span class="text-[var(--accent)]">{row.active} active</span> · {/if}{row.open} open
                    </span>
                  </a>
                </li>
              {/each}
            </ul>
            {#if moduleRows.length > MODULE_CAP}
              <p class="pt-2.5 text-caption text-[var(--text-muted)]">{moduleRows.length - MODULE_CAP} more in Modules.</p>
            {/if}
          {/if}
        </section>

        <div class="ov-aside py-6 min-w-0 flex flex-col gap-8">
          <section data-testid="overview-plans">
            {@render heading("Plans", undefined, { href: `/${projectIdentifier}/plans`, label: "All plans" })}
            {#if plans.length === 0}
              <p class="text-[0.95rem] text-[var(--text-muted)]">No plans in progress.</p>
            {:else}
              <ul class="flex flex-col">
                {#each plans.slice(0, 4) as plan (plan.id)}
                  {@const pct = plan.step_count > 0 ? plan.done_count / plan.step_count : 0}
                  <li>
                    <a href="#/{projectIdentifier}/plans/{plan.id}" use:navLink={navigate}
                       class="flex items-center gap-3 px-1 py-2.5 border-b border-[var(--border)] hover:bg-[var(--chart-hover)] transition-colors">
                      <ProgressRing value={pct} size={22} stroke={3} color="var(--accent)" showValue={false} />
                      <span class="flex-1 min-w-0">
                        <span class="block text-body-sm text-[var(--text)] truncate">{plan.title}</span>
                        <span class="block text-caption text-[var(--text-faint)] tabular-nums">
                          {plan.done_count} of {plan.step_count} steps · <TimeAgo date={plan.updated_at} />
                        </span>
                      </span>
                      <ListChecks size={14} class="shrink-0 text-[var(--text-faint)]" />
                    </a>
                  </li>
                {/each}
              </ul>
            {/if}
          </section>

          <section data-testid="overview-pages">
            {@render heading(pageRows.some((p) => p.pinned) ? "Pinned and recent pages" : "Recent pages", undefined, { href: `/${projectIdentifier}/pages`, label: "All pages" })}
            {#if pageRows.length === 0}
              <p class="text-[0.95rem] text-[var(--text-muted)]">No pages yet. Pages hold the docs and decisions behind the work.</p>
            {:else}
              <ul class="flex flex-col">
                {#each pageRows as page (page.id)}
                  <li>
                    <a href="#/{projectIdentifier}/pages/{page.id}" use:navLink={navigate}
                       class="flex items-center gap-2.5 px-1 py-2 border-b border-[var(--border)] hover:bg-[var(--chart-hover)] transition-colors">
                      {#if page.pinned}
                        <Pin size={14} class="shrink-0 text-[var(--accent)]" aria-label="Pinned" />
                      {:else}
                        <FileText size={14} class="shrink-0 text-[var(--text-faint)]" />
                      {/if}
                      <span class="flex-1 min-w-0 text-body-sm text-[var(--text)] truncate">{page.title}</span>
                      <span class="shrink-0 text-micro text-[var(--text-faint)] tabular-nums"><TimeAgo date={page.updated_at} /></span>
                    </a>
                  </li>
                {/each}
              </ul>
            {/if}
          </section>
        </div>
      </div>

      <!-- ── ACTIVITY + PEOPLE ──────────────────────────────────── -->
      <div class="ov-split">
        <section class="pt-6 min-w-0" data-testid="overview-activity">
          {@render heading("Recent activity", undefined, { href: `/${projectIdentifier}/activity`, label: "Full log" })}
          {#if days.length === 0}
            <p class="text-[0.95rem] text-[var(--text-muted)]">No activity yet.</p>
          {:else}
            <div class="flex flex-col gap-5">
              {#each days as day (day.key)}
                <div>
                  <p class="text-caption font-medium text-[var(--text-faint)] mb-1">{day.label}</p>
                  <ol class="flex flex-col">
                    {#each day.lines as line (line.id)}
                      {@const dest = lineDest(line)}
                      {@const title = line.target ? titles.get(line.target) : undefined}
                      <li>
                        {#if dest}
                          <a href="#{dest}" use:navLink={navigate}
                             class="flex items-start gap-3 px-1 py-2 rounded-md text-body-sm leading-snug hover:bg-[var(--chart-hover)] transition-colors">
                            {@render activityLine(line, title)}
                          </a>
                        {:else}
                          <div class="flex items-start gap-3 px-1 py-2 rounded-md text-body-sm leading-snug">
                            {@render activityLine(line, title)}
                          </div>
                        {/if}
                      </li>
                    {/each}
                  </ol>
                </div>
              {/each}
            </div>
          {/if}
        </section>

        {#snippet activityLine(line: ActivityLine, title: string | undefined)}
                          <span class="size-7 shrink-0 grid place-items-center">
                            {@render actorMark(line.isBot, line.source.actor_user_id, line.source.actor_username, line.source.actor_display_name)}
                          </span>
                          <span class="flex-1 min-w-0 pt-1">
                            <span class="text-[var(--text-muted)]">
                              <span class="font-medium text-[var(--text)]">{line.actor}</span>
                              {line.verb}
                              {#if line.target}
                                <span class="font-mono text-caption text-[var(--accent)]">{line.target}</span>
                              {/if}
                              {#if line.detail}<span class="text-[var(--text)]">{line.detail}</span>{/if}
                              {#if line.count > 1}
                                <span class="text-caption text-[var(--text-faint)]">and {line.count - 1} more {line.count === 2 ? "change" : "changes"}</span>
                              {/if}
                            </span>
                            {#if title}
                              <span class="block text-caption text-[var(--text-faint)] truncate mt-0.5">{title}</span>
                            {/if}
                            {#if line.quote}
                              <span class="block text-caption text-[var(--text-muted)] mt-1 pl-2 border-l-2 border-[var(--border)] line-clamp-2">{line.quote}</span>
                            {/if}
                          </span>
                          <span class="shrink-0 pt-1 text-micro text-[var(--text-faint)] tabular-nums"><TimeAgo date={line.ts} /></span>
        {/snippet}

        <div class="ov-aside pt-6 max-xl:mt-6 min-w-0 flex flex-col gap-8">
          <section data-testid="overview-people">
            {@render heading("Who's been working", "The last 12 weeks.")}
            {#if actors.length === 0}
              <p class="text-[0.95rem] text-[var(--text-muted)]">Nobody yet.</p>
            {:else}
              <ul class="flex flex-col gap-1">
                {#each actors as a (a.actor_user_id ?? -1)}
                  {@const name = a.display_name || a.username || "Lific"}
                  <li class="flex items-center gap-3 px-1 py-1.5">
                    {@render actorMark(a.is_bot, a.actor_user_id, a.username, a.display_name)}
                    <span class="w-28 sm:w-36 shrink-0 min-w-0">
                      <span class="block text-body-sm text-[var(--text)] truncate">{name}</span>
                      <span class="block text-micro text-[var(--text-faint)]"><TimeAgo date={a.last_ts} /></span>
                    </span>
                    <span class="relative flex-1 h-1.5 rounded-full bg-[var(--chart-tile)] overflow-hidden">
                      <span class="meter absolute inset-y-0 left-0 rounded-full {a.is_bot ? 'bg-[var(--accent)]' : 'bg-[var(--text-muted)]'}"
                            style="width: {(a.actions / actorMax) * 100}%"></span>
                    </span>
                    <span class="w-10 text-right text-caption tabular-nums text-[var(--text-muted)]">{a.actions}</span>
                  </li>
                {/each}
              </ul>
            {/if}
          </section>

          {#if labelRows.length > 0}
            <section data-testid="overview-labels">
              {@render heading("Labels", "Open issues per label.", canManage ? { href: `/${projectIdentifier}/settings?section=labels`, label: "Manage" } : undefined)}
              <div class="flex flex-wrap gap-1.5">
                {#each labelRows.slice(0, 16) as { label, open } (label.id)}
                  <button
                    class="inline-flex items-center gap-1.5 h-7 pl-2 pr-2.5 rounded-full text-body-sm
                           bg-[var(--chart-tile)] hover:bg-[color-mix(in_oklab,var(--text)_12%,transparent)] transition-colors
                           {open === 0 ? 'opacity-60' : ''}"
                    onclick={() => openInList({ filterLabel: [label.name], filterStatus: [STATUS_UNRESOLVED] })}
                    title={`Open issues labeled ${label.name}`}
                  >
                    <span class="size-2 rounded-full" style="background: {safeLabelColor(label.color)}"></span>
                    <span class="text-[var(--text)]">{label.name}</span>
                    <span class="tabular-nums text-caption text-[var(--text-faint)]">{open}</span>
                  </button>
                {/each}
              </div>
            </section>
          {/if}

          <section>
            <a
              href="#/{projectIdentifier}/activity"
              use:navLink={navigate}
              class="inline-flex items-center gap-1.5 text-body-sm text-[var(--text-muted)] hover:text-[var(--text)] transition-colors"
            >
              <History size={14} /> Every change, with diffs, is in the activity log
            </a>
          </section>
        </div>
      </div>
      {/if}
    {/if}
  </div>
</div>

<style>
  .overview {
    --chart-grid: color-mix(in oklab, var(--text) 7%, transparent);
    --chart-axis: color-mix(in oklab, var(--text) 22%, transparent);
    --chart-hover: color-mix(in oklab, var(--text) 4.5%, transparent);
    --chart-tile: color-mix(in oklab, var(--text) 7.5%, transparent);
  }
  .pulse-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 0.25rem;
  }
  @media (min-width: 768px) {
    .pulse-grid {
      grid-template-columns: repeat(5, minmax(0, 1fr));
    }
  }
  .meter {
    transition: width 480ms var(--ease-out-expo, cubic-bezier(0.16, 1, 0.3, 1));
  }
  .work-row {
    transition: background-color 120ms ease;
  }
  .ov-split {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
  }
  .ov-aside {
    border-top: 1px solid var(--border);
  }
  /* Stacked under the main column, the aside's sections sit side by side
     where there is room rather than stretching a small chart edge to edge. */
  @media (min-width: 768px) and (max-width: 1279px) {
    .ov-aside {
      display: grid;
      grid-template-columns: repeat(2, minmax(0, 1fr));
      column-gap: 2rem;
      align-items: start;
    }
  }
  @media (min-width: 1280px) {
    .ov-split {
      grid-template-columns: minmax(0, 1fr) minmax(320px, 400px);
    }
    .ov-split > :first-child {
      padding-right: 2rem;
    }
    .ov-aside {
      border-top: 0;
      border-left: 1px solid var(--border);
      padding-left: 2rem;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .meter {
      transition: none;
    }
  }
</style>
