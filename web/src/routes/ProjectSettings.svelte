<script lang="ts">
  // LIF-187: Project Overview. Deliberately calm + high-signal: an
  // inline-editable identity hero, ONE importance-ranked "Needs attention"
  // list, a recent-activity feed, and a gated danger zone. No KPI cards,
  // no priority breakdown, no recently-updated list — signal over volume.
  import {
    listProjects,
    listIssues,
    listProjectActivity,
    listUsers,
    getIssueCounts,
    updateProject,
    deleteProject,
    downloadProjectExport,
    type Project,
    type Issue,
    type Activity,
    type IssueStatusCounts,
    type UserSummary,
  } from "../lib/api";
  import IconPicker from "../lib/IconPicker.svelte";
  import LabelManager from "../lib/LabelManager.svelte";
  import ProjectMembers from "../lib/ProjectMembers.svelte";
  import ImportPanel from "../lib/ImportPanel.svelte";
  import { loadListState, saveListState } from "../lib/issues/persistence";
  import ProjectIcon from "../lib/ProjectIcon.svelte";
  import ProgressRing from "../lib/ProgressRing.svelte";
  import StatusIcon from "../lib/StatusIcon.svelte";
  import PriorityIcon from "../lib/PriorityIcon.svelte";
  import { formatDate } from "../lib/format";
  import TimeAgo from "../lib/TimeAgo.svelte";
  import {
    ChevronRight, Download, Pencil, Copy, Check, ArrowRight, History,
    AlertTriangle, ChevronDown,
  } from "lucide-svelte";
  import ErrorState from "../lib/ErrorState.svelte";
  import Skeleton from "../lib/Skeleton.svelte";
  import { getContext } from "svelte";
  // LIF-234: role-aware affordance gating. `canManage` = lead/admin (or
  // enforcement off) — settings edits, danger zone, members, and import are
  // all lead-level. `canEdit` = maintainer/admin — label management.
  import { projectRole, loadProjectRole } from "../lib/projectRole.svelte";

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
  let activity = $state<Activity[]>([]);
  let users = $state<UserSummary[]>([]);

  // Inline-edit drafts
  let editingName = $state(false);
  let draftName = $state("");
  let editingDesc = $state(false);
  let draftDesc = $state("");
  let savedAt = $state(0); // ms timestamp of last successful field save
  let copied = $state(false);

  // Danger zone
  let dangerOpen = $state(false);
  let newIdent = $state("");
  let identError = $state("");
  let renaming = $state(false);
  let leadValue = $state("");
  let lastLead = $state<number | null>(null);
  let showDeleteSection = $state(false);
  let deleteConfirmText = $state("");
  let deleting = $state(false);
  let deleteError = $state("");
  let exportError = $state("");
  let exporting = $state(false);

  $effect(() => {
    const id = projectIdentifier;
    loadAll(id);
  });

  async function loadAll(ident: string) {
    loading = true;
    error = "";
    const projRes = await listProjects();
    if (!projRes.ok) { error = projRes.error; loading = false; return; }
    const found = projRes.data.find((p: Project) => p.identifier === ident);
    if (!found) { error = `Project ${ident} not found`; loading = false; return; }
    project = found;
    // LIF-234: prime the shared role store for this project so `canManage`/
    // `canEdit` below reflect the correct answer (Layout also primes it, but
    // this makes the route self-sufficient and dedupes via the store cache).
    loadProjectRole(found.id);
    leadValue = found.lead_user_id == null ? "" : String(found.lead_user_id);
    lastLead = found.lead_user_id;

    const [countsRes, issuesRes, actRes, usersRes] = await Promise.all([
      getIssueCounts(found.id),
      listIssues({ project_id: found.id, limit: 1000 }),
      listProjectActivity(found.id, 14),
      listUsers(),
    ]);
    if (countsRes.ok) counts = countsRes.data;
    if (issuesRes.ok) issues = issuesRes.data;
    if (actRes.ok) activity = actRes.data.items;
    if (usersRes.ok) users = usersRes.data;
    loading = false;
  }

  // ── Field-level autosave (no Save button) ────────────
  async function saveField(field: string, value: unknown) {
    if (!project) return;
    const res = await updateProject(project.id, { [field]: value });
    if (res.ok) {
      project = res.data;
      onProjectChange?.();
      savedAt = Date.now();
      window.setTimeout(() => { if (Date.now() - savedAt >= 1900) savedAt = 0; }, 2000);
    } else {
      error = res.error;
    }
  }

  function startEditName() {
    if (!project) return;
    draftName = project.name;
    editingName = true;
  }
  function commitName() {
    editingName = false;
    const v = draftName.trim();
    if (project && v && v !== project.name) saveField("name", v);
  }
  function startEditDesc() {
    if (!project) return;
    draftDesc = project.description;
    editingDesc = true;
  }
  function commitDesc() {
    editingDesc = false;
    const v = draftDesc.trim();
    if (project && v !== project.description) saveField("description", v);
  }

  // Lead change (danger zone). Autosaves on select; guarded so the initial
  // hydrate doesn't fire a write.
  $effect(() => {
    const v = leadValue;
    if (!project) return;
    const next = v === "" ? null : Number(v);
    if (next !== lastLead) {
      lastLead = next;
      saveField("lead_user_id", next);
    }
  });

  async function copyIdentifier() {
    if (!project) return;
    try {
      await navigator.clipboard.writeText(project.identifier);
      copied = true;
      window.setTimeout(() => { copied = false; }, 1500);
    } catch { /* clipboard blocked */ }
  }

  // ── Importance heuristic ─────────────────────────────
  // score = (priorityWeight + age*0.5 + staleness*0.6) * statusMultiplier,
  // over OPEN issues only. Cheap, O(n), and honest: an old urgent todo that
  // hasn't moved floats to the top. We never show the number — only the
  // cause (priority + an age/idle cue).
  const PRIORITY_WEIGHT: Record<string, number> = { urgent: 100, high: 55, medium: 25, low: 10, none: 4 };
  const STATUS_MULT: Record<string, number> = { todo: 1.25, active: 1.15, backlog: 1.0 };

  function daysSince(iso: string): number {
    const t = new Date(iso + "Z").getTime();
    if (Number.isNaN(t)) return 0;
    return Math.max(0, Math.floor((Date.now() - t) / 86400000));
  }
  function score(i: Issue): number {
    const pw = PRIORITY_WEIGHT[i.priority] ?? 4;
    const sm = STATUS_MULT[i.status] ?? 1;
    return (pw + daysSince(i.created_at) * 0.5 + daysSince(i.updated_at) * 0.6) * sm;
  }
  function ageLabel(days: number): string {
    if (days >= 60) return `${Math.round(days / 30)}mo`;
    if (days >= 1) return `${days}d`;
    return "today";
  }

  const openIssues = $derived(
    issues.filter((i) => i.status === "backlog" || i.status === "todo" || i.status === "active"),
  );
  const ranked = $derived.by(() =>
    [...openIssues].sort((a, b) => score(b) - score(a)),
  );
  const attention = $derived(ranked.slice(0, 6));
  const moreCount = $derived(Math.max(0, openIssues.length - attention.length));

  const total = $derived(counts?.total ?? 0);
  const completion = $derived(total > 0 ? (counts?.done ?? 0) / total : 0);

  // LIF-234: lead/admin (or enforcement off) may edit project identity,
  // manage members, import, and use the danger zone. A non-lead sees a
  // read-only overview.
  const canManage = $derived(projectRole.canManage);

  function gotoOpenIssues() {
    navigate(`/${projectIdentifier}/issues`);
  }

  // #1: open the issue list pre-filtered to a label. The list hydrates its
  // filters from persisted per-project state on mount, so we stamp the label
  // filter there (merging with whatever else is stored) before navigating.
  function openLabelInIssues(name: string) {
    const s = loadListState(projectIdentifier);
    saveListState(projectIdentifier, { ...s, filterLabel: name });
    navigate(`/${projectIdentifier}/issues`);
  }

  function actorName(a: Activity): string {
    return a.actor_display_name || a.actor_username || (a.actor_is_bot ? "a bot" : "system");
  }
  function activityText(a: Activity): string {
    const verb = a.action === "create" ? "created"
      : a.action === "delete" ? "deleted"
      : a.action === "update" ? "updated" : a.action;
    return `${verb} ${a.entity_label ? `${a.entity_type} ${a.entity_label}` : a.entity_type}`;
  }

  // ── Danger zone actions ──────────────────────────────
  let leadOptions = $derived([
    { value: "", label: "No lead" },
    ...users.map((u) => ({ value: String(u.id), label: u.display_name || u.username })),
  ]);

  async function renameIdentifier() {
    if (!project) return;
    const nid = newIdent.trim().toUpperCase();
    if (!nid || nid === project.identifier) return;
    renaming = true; identError = "";
    const res = await updateProject(project.id, { identifier: nid });
    if (res.ok) {
      project = res.data;
      onProjectChange?.();
      navigate(`/${res.data.identifier}/overview`);
    } else {
      identError = res.error;
      renaming = false;
    }
  }

  let deleteReady = $derived(project != null && deleteConfirmText === project.identifier);
  async function handleDelete() {
    if (!project || !deleteReady) return;
    deleting = true; deleteError = "";
    const res = await deleteProject(project.id);
    if (res.ok) navigate("/settings");
    else { deleteError = res.error; deleting = false; }
  }

  async function exportProject() {
    if (!project || exporting) return;
    exporting = true; exportError = "";
    const res = await downloadProjectExport(project.identifier);
    if (!res.ok) exportError = res.error;
    exporting = false;
  }
</script>

{#if loading}
  <!-- LIF-281: structural skeleton replacing the bare centered spinner.
       Mirrors the loaded overview's default-visible frame — the same
       max-w-[840px] … gap-10 wrapper, the identity hero (icon + name/desc +
       completion ring), and the "Needs attention" heading + card list — so
       the first heading lands at the same y-position and nothing snaps when
       data arrives. Conditional/role-gated sections (members, import,
       danger zone, recent activity) are intentionally omitted; they don't
       render by default. -->
  <div class="h-full flex flex-col">
    <div class="flex-1 overflow-y-auto">
      <div class="max-w-[840px] mx-auto px-6 py-8 flex flex-col gap-10">
        <!-- Identity hero -->
        <section class="flex items-start gap-4">
          <Skeleton variant="block" class="size-12 rounded-xl shrink-0" />
          <div class="flex-1 min-w-0 flex flex-col gap-2.5">
            <Skeleton variant="bar" class="h-8 w-64" />
            <Skeleton variant="bar" class="h-4 w-full max-w-[46ch]" />
            <Skeleton variant="bar" class="h-3 w-40" />
          </div>
          <Skeleton variant="circle" class="size-[52px] shrink-0" />
        </section>

        <!-- Needs attention -->
        <section>
          <div class="flex items-baseline justify-between mb-3">
            <Skeleton variant="bar" class="h-3.5 w-32" />
          </div>
          <div class="flex flex-col rounded-xl bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)] overflow-hidden">
            {#each [0, 1, 2, 3] as row (row)}
              <div class="flex items-center gap-3 pl-4 pr-3 py-2.5 {row > 0 ? 'border-t border-[var(--border)]' : ''}">
                <Skeleton variant="circle" class="size-[15px]" />
                <Skeleton variant="bar" class="h-3 w-[58px] shrink-0" />
                <Skeleton variant="bar" class="h-3.5 flex-1 max-w-[320px]" />
                <Skeleton variant="bar" class="h-3 w-12 shrink-0" />
              </div>
            {/each}
          </div>
        </section>
      </div>
    </div>
  </div>
{:else if !project}
  <ErrorState title="Couldn't load this project" message={error}>
    <button
      class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)] px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
      onclick={() => loadAll(projectIdentifier)}
    >
      Try again
    </button>
    <button
      class="text-body-sm text-[var(--text-muted)] border border-[var(--border)] px-3 py-1.5 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
      onclick={() => navigate("/settings")}
    >
      Back to home
    </button>
  </ErrorState>
{:else}
  <div class="h-full flex flex-col">
    <div class="flex-1 overflow-y-auto">
      <div class="max-w-[840px] mx-auto px-6 py-8 flex flex-col gap-10">

        <!-- ── IDENTITY HERO (inline-editable) ──────────── -->
        <!-- LIF-234: identity edits (name/desc/icon → updateProject) are
             lead-gated. A non-lead sees the same hero, read-only: the icon
             isn't a picker, and name/description aren't click-to-edit. -->
        <section class="flex items-start gap-4">
          <div class="shrink-0">
            {#if canManage}
              <IconPicker value={project.emoji ?? ""} onchange={(v) => saveField("emoji", v || null)} />
            {:else}
              <div class="size-12 grid place-items-center rounded-xl bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)]">
                {#if project.emoji}
                  <ProjectIcon value={project.emoji} size={22} />
                {:else}
                  <span class="text-body-sm font-mono font-semibold text-[var(--text-muted)]">{project.identifier.slice(0, 2)}</span>
                {/if}
              </div>
            {/if}
          </div>
          <div class="flex-1 min-w-0">
            <!-- Name -->
            <div class="flex items-center gap-2 flex-wrap">
              {#if !canManage}
                <h1 class="text-display font-display tracking-tight text-[var(--text)] -my-0.5 px-0.5">{project.name}</h1>
              {:else if editingName}
                <!-- svelte-ignore a11y_autofocus -->
                <input
                  bind:value={draftName}
                  class="text-display font-display tracking-tight bg-transparent border-none outline-none
                         text-[var(--text)] w-full max-w-[28ch] -my-0.5 focus-visible:ring-2
                         focus-visible:ring-[var(--accent)] rounded px-0.5"
                  autofocus
                  onblur={commitName}
                  onkeydown={(e) => { if (e.key === 'Enter') commitName(); if (e.key === 'Escape') editingName = false; }}
                />
              {:else}
                <button
                  class="group flex items-center gap-2 text-display font-display tracking-tight
                         text-[var(--text)] -my-0.5 rounded px-0.5 cursor-text hover:bg-[var(--bg-subtle)] transition-colors"
                  onclick={startEditName}
                >
                  {project.name}
                  <Pencil size={14} class="text-[var(--text-faint)] opacity-0 group-hover:opacity-100 transition-opacity" />
                </button>
              {/if}
              <button
                class="group inline-flex items-center gap-1 text-micro font-mono font-semibold
                       px-1.5 py-0.5 rounded border border-[var(--border)] text-[var(--text-muted)]
                       hover:border-[var(--accent)] hover:text-[var(--accent)] transition-colors"
                onclick={copyIdentifier}
                title="Copy identifier"
              >
                {project.identifier}
                {#if copied}<Check size={11} />{:else}<Copy size={11} class="opacity-0 group-hover:opacity-100 transition-opacity" />{/if}
              </button>
              {#if savedAt}
                <span class="inline-flex items-center gap-1 text-micro text-[var(--success)]" aria-live="polite">
                  <Check size={11} /> Saved
                </span>
              {/if}
              {#if !canManage && projectRole.enforced}
                <!-- LIF-234: tell a non-lead why the identity + settings are
                     read-only, in the existing badge vocabulary. -->
                <span class="inline-flex items-center gap-1 text-micro font-medium px-1.5 py-0.5 rounded-full
                             text-[var(--text-muted)] bg-[var(--bg-subtle)]"
                      title="Only a project lead or admin can change project settings.">
                  Read-only
                </span>
              {/if}
            </div>

            <!-- Description -->
            {#if !canManage}
              {#if project.description}
                <p class="text-body text-[var(--text-muted)] leading-relaxed mt-1.5 max-w-[60ch]">{project.description}</p>
              {/if}
            {:else if editingDesc}
              <!-- svelte-ignore a11y_autofocus -->
              <textarea
                bind:value={draftDesc}
                rows="2"
                class="mt-2 w-full text-body bg-transparent border border-[var(--border)] rounded-md px-2.5 py-1.5
                       text-[var(--text)] outline-none resize-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
                placeholder="Describe this project…"
                autofocus
                onblur={commitDesc}
                onkeydown={(e) => { if (e.key === 'Escape') editingDesc = false; if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) commitDesc(); }}
              ></textarea>
            {:else}
              <button class="group block text-left mt-1.5 max-w-[60ch] rounded px-0.5 cursor-text hover:bg-[var(--bg-subtle)] transition-colors" onclick={startEditDesc}>
                {#if project.description}
                  <span class="text-body text-[var(--text-muted)] leading-relaxed">{project.description}</span>
                {:else}
                  <span class="text-body-sm text-[var(--text-faint)] italic">Add a description…</span>
                {/if}
                <Pencil size={12} class="inline ml-1 text-[var(--text-faint)] opacity-0 group-hover:opacity-100 transition-opacity align-baseline" />
              </button>
            {/if}

            <div class="flex items-center gap-3 mt-2.5 text-caption text-[var(--text-faint)] tabular-nums">
              <span>Created {formatDate(project.created_at)}</span>
              {#if activity[0]}<span>·</span><span>Active <TimeAgo date={activity[0].ts} /></span>{/if}
            </div>
          </div>

          <!-- Completion: the one health stat worth keeping. -->
          {#if total > 0}
            <div class="shrink-0 flex flex-col items-center gap-1 pt-0.5">
              <ProgressRing value={completion} size={52} stroke={5} color="var(--success)" />
              <span class="text-micro text-[var(--text-faint)] tabular-nums">{counts?.done ?? 0}/{total} done</span>
            </div>
          {/if}
        </section>

        <!-- ── NEEDS ATTENTION (the star) ───────────────── -->
        <section>
          <div class="flex items-baseline justify-between mb-3">
            <h2 class="text-body-sm font-semibold text-[var(--text)]">Needs attention</h2>
            {#if moreCount > 0}
              <button class="text-caption text-[var(--text-muted)] hover:text-[var(--text)] transition-colors flex items-center gap-1" onclick={gotoOpenIssues}>
                +{moreCount} more open <ArrowRight size={11} />
              </button>
            {/if}
          </div>

          {#if attention.length === 0}
            <div class="flex items-center gap-3 py-6 px-1 text-[var(--text-muted)]">
              <span class="grid place-items-center size-8 rounded-full bg-[var(--success-bg)] text-[var(--success)]"><Check size={16} /></span>
              <div>
                <p class="text-body text-[var(--text)]">Nothing needs attention</p>
                <p class="text-body-sm text-[var(--text-faint)]">Everything open is fresh and on track.</p>
              </div>
            </div>
          {:else}
            <div class="flex flex-col rounded-xl bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)] overflow-hidden">
              {#each attention as issue, idx (issue.id)}
                {@const age = daysSince(issue.created_at)}
                {@const idle = daysSince(issue.updated_at)}
                {@const heat = issue.priority === 'urgent'
                  ? 'var(--error)'
                  : (issue.priority === 'high' || idle >= 14)
                    ? 'var(--warn)'
                    : 'var(--text-faint)'}
                <button
                  class="group relative flex items-center gap-3 pl-4 pr-3 py-2.5 text-left
                         hover:bg-[var(--bg-subtle)] transition-colors
                         {idx > 0 ? 'border-t border-[var(--border)]' : ''}"
                  onclick={() => navigate(`/${projectIdentifier}/issues/${issue.identifier}`)}
                >
                  <!-- heat edge -->
                  <span class="absolute left-0 top-0 bottom-0 w-[3px]" style="background: {heat}"></span>
                  <PriorityIcon priority={issue.priority} size={15} />
                  <span class="text-micro font-mono text-[var(--text-faint)] shrink-0 tabular-nums w-[58px]">{issue.identifier}</span>
                  <span class="text-body text-[var(--text)] truncate flex-1">{issue.title}</span>
                  <div class="shrink-0 flex items-center gap-2 text-micro tabular-nums">
                    <span class="text-[var(--text-faint)]">open {ageLabel(age)}</span>
                    {#if idle >= 14}
                      <span class="px-1.5 py-0.5 rounded-full text-[var(--warn)] bg-[color-mix(in_oklab,var(--warn)_14%,transparent)]">idle {ageLabel(idle)}</span>
                    {/if}
                    <StatusIcon status={issue.status} size={14} />
                  </div>
                </button>
              {/each}
            </div>
          {/if}
        </section>

        <!-- ── LABELS (management) ──────────────────────── -->
        <!-- LIF-234: label management is a maintainer-level structure edit —
             a viewer sees labels read-only. -->
        <LabelManager
          projectId={project.id}
          {issues}
          canEdit={projectRole.canEdit}
          onChange={() => loadAll(projectIdentifier)}
          onOpenLabel={openLabelInIssues}
        />

        <!-- ── MEMBERS (LIF-200) ────────────────────────── -->
        <!-- LIF-234: members management is a lead-level operation. Hidden
             entirely for non-leads (its own read-only-notice path was for the
             flag-off era; now the whole panel only shows when manageable). -->
        {#if canManage}
          <ProjectMembers projectId={project.id} />

          <!-- ── IMPORT (LIF-264) ─────────────────────────── -->
          <ImportPanel projectId={project.id} onImported={() => loadAll(projectIdentifier)} />
        {/if}

        <!-- ── RECENT ACTIVITY ──────────────────────────── -->
        {#if activity.length > 0}
          <section>
            <div class="flex items-baseline justify-between mb-3">
              <h2 class="text-body-sm font-semibold text-[var(--text)]">Recent activity</h2>
              <button class="text-caption text-[var(--text-muted)] hover:text-[var(--text)] transition-colors flex items-center gap-1" onclick={() => navigate(`/${projectIdentifier}/activity`)}>
                <History size={11} /> Full log
              </button>
            </div>
            <div class="flex flex-col gap-2.5">
              {#each activity.slice(0, 8) as a (a.id)}
                <div class="flex items-start gap-2.5 text-body-sm leading-snug">
                  <span class="size-1.5 rounded-full bg-[var(--text-faint)] mt-1.5 shrink-0"></span>
                  <p class="text-[var(--text-muted)]">
                    <span class="font-medium text-[var(--text)]">{actorName(a)}</span>
                    {activityText(a)}
                    <span class="text-[var(--text-faint)] tabular-nums">· <TimeAgo date={a.ts} /></span>
                  </p>
                </div>
              {/each}
            </div>
          </section>
        {/if}

        <!-- ── DANGER ZONE ──────────────────────────────── -->
        <!-- LIF-234: lead/admin-only. Hidden entirely for everyone else. -->
        {#if canManage}
        <section class="rounded-xl border border-[var(--error)]/40 overflow-hidden mt-2"
                 style="border-color: color-mix(in oklab, var(--error) 35%, transparent)">
          <button
            class="w-full flex items-center gap-2 px-4 py-3 text-left hover:bg-[var(--error-bg)] transition-colors"
            onclick={() => { dangerOpen = !dangerOpen; }}
          >
            <AlertTriangle size={15} class="text-[var(--error)]" />
            <span class="text-body font-semibold text-[var(--error)] flex-1">Danger zone</span>
            <ChevronDown size={15} class="text-[var(--error)] transition-transform {dangerOpen ? 'rotate-180' : ''}" />
          </button>

          {#if dangerOpen}
            <div class="border-t px-4 py-4 flex flex-col gap-5"
                 style="border-color: color-mix(in oklab, var(--error) 25%, transparent)">

              <!-- Project lead -->
              <div class="flex items-center gap-3">
                <div class="flex-1">
                  <p class="text-body-sm font-medium text-[var(--text)]">Project lead</p>
                  <p class="text-caption text-[var(--text-muted)]">Who owns this project.</p>
                </div>
                <select
                  bind:value={leadValue}
                  class="text-body-sm rounded-md border border-[var(--border)] bg-[var(--surface)]
                         text-[var(--text)] px-2.5 py-1.5 outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
                >
                  {#each leadOptions as o}
                    <option value={o.value}>{o.label}</option>
                  {/each}
                </select>
              </div>

              <div class="h-px" style="background: color-mix(in oklab, var(--error) 18%, transparent)"></div>

              <!-- Change identifier -->
              <div>
                <p class="text-body-sm font-medium text-[var(--text)]">Change identifier</p>
                <p class="text-caption text-[var(--text-muted)] mt-0.5 mb-2 leading-relaxed">
                  Re-keys every issue, page, and plan. Existing references to <span class="font-mono">{project.identifier}-NNN</span>
                  written inside other issues/pages will no longer resolve. This cannot be undone automatically.
                </p>
                <div class="flex items-center gap-2">
                  <input
                    bind:value={newIdent}
                    placeholder={project.identifier}
                    class="w-[120px] px-2.5 py-1.5 text-body-sm font-mono uppercase rounded-md
                           border border-[var(--border)] bg-[var(--surface)] text-[var(--text)]
                           outline-none focus-visible:ring-2 focus-visible:ring-[var(--error)]"
                  />
                  <button
                    class="text-body-sm font-medium text-[var(--error-text)] bg-[var(--error)] px-3 py-1.5 rounded-md
                           hover:opacity-90 transition-opacity disabled:opacity-40 disabled:cursor-not-allowed"
                    disabled={renaming || !newIdent.trim() || newIdent.trim().toUpperCase() === project.identifier}
                    onclick={renameIdentifier}
                  >
                    {renaming ? "Renaming…" : "Rename"}
                  </button>
                </div>
                {#if identError}<p class="text-caption text-[var(--error)] mt-1.5">{identError}</p>{/if}
              </div>

              <div class="h-px" style="background: color-mix(in oklab, var(--error) 18%, transparent)"></div>

              <!-- Delete -->
              <div>
                <p class="text-body-sm font-medium text-[var(--text)]">Delete project</p>
                {#if !showDeleteSection}
                  <button
                    class="mt-2 text-body-sm text-[var(--error)] border border-[var(--error)] px-3 py-1.5 rounded-md hover:bg-[var(--error-bg)] transition-colors"
                    onclick={() => { showDeleteSection = true; }}
                  >
                    Delete this project
                  </button>
                {:else}
                  <p class="text-caption text-[var(--text-muted)] mt-1 mb-2">
                    Permanently deletes the project and all <strong>{total}</strong> issue{total !== 1 ? 's' : ''}, modules, labels, folders, pages, and plans.
                    Type <strong class="font-mono">{project.identifier}</strong> to confirm.
                  </p>
                  <div class="flex items-center gap-2">
                    <input
                      bind:value={deleteConfirmText}
                      placeholder={project.identifier}
                      class="w-[140px] px-2.5 py-1.5 text-body-sm font-mono rounded-md border border-[var(--error)]
                             bg-[var(--surface)] text-[var(--text)] outline-none focus-visible:ring-2 focus-visible:ring-[var(--error)]"
                    />
                    <button
                      class="text-body-sm font-medium text-[var(--error-text)] bg-[var(--error)] px-3 py-1.5 rounded-md
                             hover:opacity-90 transition-opacity disabled:opacity-40 disabled:cursor-not-allowed"
                      disabled={!deleteReady || deleting}
                      onclick={handleDelete}
                    >
                      {deleting ? "Deleting…" : "Delete permanently"}
                    </button>
                    <button class="text-body-sm text-[var(--text-muted)] px-2 py-1.5 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
                            onclick={() => { showDeleteSection = false; deleteConfirmText = ''; deleteError = ''; }}>
                      Cancel
                    </button>
                  </div>
                  {#if deleteError}<p class="text-caption text-[var(--error)] mt-1.5">{deleteError}</p>{/if}
                {/if}
              </div>
            </div>
          {/if}
        </section>
        {/if}

        <div class="h-2"></div>
      </div>
    </div>
  </div>
{/if}

{#snippet topbarContent()}
  {#if project}
    <div class="flex items-center gap-3 px-6 py-2 w-full">
      <div class="flex items-center gap-1.5 shrink-0">
        <button
          class="text-body-sm font-mono font-medium text-[var(--text-muted)] hover:text-[var(--text)] transition-colors"
          onclick={() => navigate(`/${project!.identifier}/issues`)}
        >
          {project.identifier}
        </button>
        <ChevronRight size={12} class="text-[var(--text-faint)]" />
        <span class="text-body-sm font-medium text-[var(--text)]">Overview</span>
      </div>
      <div class="ml-auto flex items-center gap-2 shrink-0">
        {#if exportError}
          <span class="text-body-sm text-[var(--error)] max-w-[min(280px,30vw)] truncate" title={exportError}>{exportError}</span>
        {/if}
        <button class="toolbar-pill" onclick={exportProject} disabled={exporting}>
          <Download size={14} />
          {exporting ? "Exporting…" : "Export"}
        </button>
      </div>
    </div>
  {/if}
{/snippet}
