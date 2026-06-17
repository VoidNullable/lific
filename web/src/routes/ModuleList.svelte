<script lang="ts">
  // LIF-121 — Modules list view.
  //
  // Mirrors the shape of PageList: a top-level browsing surface within
  // a project. Modules are grouped by their lifecycle status (active /
  // planned / paused / done / cancelled / backlog) so the user can see
  // what's in flight vs. parked at a glance. Within each group, modules
  // are alpha-sorted (backend already does this in list_modules).
  //
  // Read this with PageList.svelte side-by-side — the routing
  // surfaces and topbar pattern are intentionally parallel.

  import {
    listModules,
    listIssues,
    listProjects,
    createModule,
    type Module,
    type Issue,
    type Project,
  } from "../lib/api";
  import { Layers, Plus, ChevronRight, CircleDot, Pause, CircleCheck, CircleX, CircleDashed, Circle } from "lucide-svelte";
  import ProjectIcon from "../lib/ProjectIcon.svelte";
  import IconPicker from "../lib/IconPicker.svelte";
  import ProgressRing from "../lib/ProgressRing.svelte";
  import Mascot from "../lib/Mascot.svelte";
  import ErrorState from "../lib/ErrorState.svelte";
  import { getContext } from "svelte";

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
  }: {
    navigate: (path: string) => void;
    projectIdentifier: string;
  } = $props();

  let project = $state<Project | null>(null);
  let modules = $state<Module[]>([]);
  let issues = $state<Issue[]>([]);
  let loading = $state(true);
  let error = $state("");

  // Inline create. Live alongside the existing modules in the list —
  // pressing the toolbar "+ Module" button opens a row at the top of
  // the Active section (the most common destination for a newly
  // declared module).
  let creating = $state(false);
  let createName = $state("");
  let createEmoji = $state("");
  let createError = $state("");
  let createSaving = $state(false);
  // Ref to the inline-create card so a blur that lands on the icon picker
  // (also inside the card) doesn't trip the empty-name auto-cancel.
  let createRowEl = $state<HTMLElement | null>(null);

  // Display order across status groups. Matches the natural lifecycle
  // (active → planned → paused → backlog → done → cancelled) so users
  // looking at "what's happening now" see it at the top.
  const STATUS_ORDER = ["active", "planned", "paused", "backlog", "done", "cancelled"];

  const STATUS_LABEL: Record<string, string> = {
    active: "Active",
    planned: "Planned",
    paused: "Paused",
    backlog: "Backlog",
    done: "Done",
    cancelled: "Cancelled",
  };

  $effect(() => {
    const id = projectIdentifier;
    loadData(id);
  });

  async function loadData(ident: string) {
    loading = true;
    error = "";
    creating = false;

    const projRes = await listProjects();
    if (!projRes.ok) { error = projRes.error; loading = false; return; }
    const found = projRes.data.find((p) => p.identifier === ident);
    if (!found) { error = `Project ${ident} not found`; loading = false; return; }
    project = found;

    // Pull modules + issues in parallel. Issues feed the per-module
    // counts; we deliberately fetch them all rather than calling per
    // module so a project with 30 modules doesn't cost 30 round trips.
    const [modRes, issueRes] = await Promise.all([
      listModules(found.id),
      listIssues({ project_id: found.id, limit: 1000 }),
    ]);
    if (modRes.ok) modules = modRes.data;
    if (issueRes.ok) issues = issueRes.data;

    loading = false;
  }

  // Modules grouped by status in display order. Each entry is non-empty;
  // empty groups are dropped so the list doesn't look like a settings
  // page full of section headers.
  let grouped = $derived.by(() => {
    const groups: { status: string; mods: Module[] }[] = [];
    for (const s of STATUS_ORDER) {
      const matching = modules
        .filter((m) => m.status === s)
        .sort((a, b) => a.name.localeCompare(b.name));
      if (matching.length > 0) groups.push({ status: s, mods: matching });
    }
    // Surface unknown statuses (forward-compat) at the end.
    const known = new Set(STATUS_ORDER);
    const leftover = modules
      .filter((m) => !known.has(m.status))
      .sort((a, b) => a.name.localeCompare(b.name));
    if (leftover.length > 0) groups.push({ status: "other", mods: leftover });
    return groups;
  });

  function issueCount(moduleId: number): number {
    return issues.filter((i) => i.module_id === moduleId).length;
  }

  // Completion for a single module. Numerator = done-status issues only;
  // denominator = every issue assigned to the module (cancelled counts in
  // the total, per the chosen metric). frac is 0..1, guarded against /0.
  function moduleProgress(moduleId: number): { done: number; total: number; frac: number } {
    const mine = issues.filter((i) => i.module_id === moduleId);
    const done = mine.filter((i) => i.status === "done").length;
    const total = mine.length;
    return { done, total, frac: total > 0 ? done / total : 0 };
  }

  // Portfolio rollup across every module-assigned issue — drives the hero
  // gauge. "In flight" = modules whose lifecycle status is active.
  let portfolio = $derived.by(() => {
    const assigned = issues.filter((i) => i.module_id != null);
    const total = assigned.length;
    const done = assigned.filter((i) => i.status === "done").length;
    const inFlight = modules.filter((m) => m.status === "active").length;
    return {
      total,
      done,
      frac: total > 0 ? done / total : 0,
      inFlight,
      moduleCount: modules.length,
    };
  });

  // Cheap markdown stripper: takes the first non-empty non-heading line
  // and clears the obvious inline markers. Same heuristic as PageList's
  // contentPreview so the visual texture matches.
  function descriptionPreview(content: string): string {
    if (!content.trim()) return "";
    const lines = content.split("\n").filter((l) => l.trim() && !l.startsWith("#"));
    return (lines[0] ?? "").replace(/[*_`\[\]]/g, "").trim().slice(0, 120);
  }

  // ── Inline create ─────────────────────────────────────

  function startCreate() {
    creating = true;
    createName = "";
    createEmoji = "";
    createError = "";
  }

  function cancelCreate() {
    creating = false;
    createName = "";
    createEmoji = "";
    createError = "";
  }

  // Cancel only when the focus is leaving the create card entirely AND
  // nothing's been entered. Clicking the icon picker keeps focus inside
  // the card, so it won't cancel.
  function handleCreateBlur(e: FocusEvent) {
    const next = e.relatedTarget as Node | null;
    if (next && createRowEl?.contains(next)) return;
    if (!createName.trim() && !createEmoji) cancelCreate();
  }

  async function commitCreate() {
    if (!project) return;
    const name = createName.trim();
    if (!name) { cancelCreate(); return; }
    createSaving = true;
    createError = "";
    const res = await createModule({
      project_id: project.id,
      name,
      status: "active",
      ...(createEmoji ? { emoji: createEmoji } : {}),
    });
    createSaving = false;
    if (res.ok) {
      modules = [...modules, res.data];
      creating = false;
      createName = "";
      createEmoji = "";
      navigate(`/${projectIdentifier}/modules/${res.data.id}`);
    } else {
      createError = res.error;
    }
  }
</script>

{#snippet topbarContent()}
  <div class="flex items-center gap-3 px-6 py-2 w-full">
    <!-- Breadcrumb: Project > Modules -->
    <div class="flex items-center gap-1.5 shrink-0">
      <button
        class="text-[0.8125rem] font-mono font-medium text-[var(--text-muted)]
               hover:text-[var(--text)] transition-colors"
        onclick={() => navigate(`/${projectIdentifier}/overview`)}
      >
        {projectIdentifier}
      </button>
      <ChevronRight size={12} class="text-[var(--text-faint)]" />
      <span class="text-[0.8125rem] font-medium text-[var(--text)]">
        Modules
      </span>
      {#if !loading}
        <span
          class="ml-1 text-[0.6875rem] text-[var(--text-faint)] font-medium
                 tabular-nums"
        >
          {modules.length}
        </span>
      {/if}
    </div>

    <!-- Right zone: action -->
    <div class="ml-auto flex items-center gap-1.5 shrink-0">
      <button
        class="flex items-center gap-1 text-[0.8125rem] font-medium
               text-[var(--btn-success-text)] bg-[var(--btn-success)]
               px-2.5 py-1 rounded-md hover:bg-[var(--btn-success-hover)]
               transition-colors focus:outline-none
               motion-safe:active:scale-[0.97]"
        onclick={startCreate}
      >
        <Plus size={14} />
        Module
      </button>
    </div>
  </div>
{/snippet}

<div class="h-full flex flex-col">
  <div class="flex-1 overflow-y-auto">
    {#if loading}
      <div class="flex items-center justify-center py-20">
        <div
          class="size-6 rounded-full border-2 border-[var(--border)]
                 border-t-[var(--accent)] animate-spin"
        ></div>
      </div>
    {:else if error}
      <ErrorState title="Couldn't load modules" message={error}>
        <button
          class="text-[0.8125rem] font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)] px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
          onclick={() => loadData(projectIdentifier)}
        >
          Try again
        </button>
        <button
          class="text-[0.8125rem] text-[var(--text-muted)] border border-[var(--border)] px-3 py-1.5 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
          onclick={() => navigate(`/${projectIdentifier}/overview`)}
        >
          Project overview
        </button>
      </ErrorState>
    {:else if modules.length === 0 && !creating}
      <!-- Empty state — mascot + charming copy + green CTA, matching the
           issue-list empty state vocabulary. -->
      <div class="flex flex-col items-center py-20 gap-4 px-6 max-w-[480px] mx-auto text-center">
        <Mascot src="/LizzySleep2.png" nativeW={1000} nativeH={420} scale={0.25} />
        <div class="flex flex-col items-center gap-1.5">
          <p class="text-[1.0625rem] font-medium text-[var(--text)]">No moving parts yet</p>
          <p class="text-[0.8125rem] text-[var(--text-muted)] leading-relaxed">
            Modules gather related issues into a single arc of work: a feature,
            a release, an effort. Spin one up to start organizing.
          </p>
        </div>
        <button
          class="flex items-center gap-1.5 mt-1 text-[0.8125rem] font-medium
                 text-[var(--btn-success-text)] bg-[var(--btn-success)]
                 px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)]
                 transition-colors"
          onclick={startCreate}
        >
          <Plus size={15} />
          Create a module
        </button>
      </div>
    {:else}
      <div class="max-w-[1100px] mx-auto px-6 py-6">
        <!-- Portfolio hero: aggregate completion gauge + headline tallies.
             The dashboard "moment" unique to the Modules surface. -->
        <div
          class="mb-7 rounded-xl bg-[var(--surface)] p-5
                 shadow-[0_1px_2px_rgba(0,0,0,0.06)]
                 flex items-center gap-6 flex-wrap"
        >
          <ProgressRing
            value={portfolio.frac}
            size={116}
            stroke={9}
            color="var(--success)"
          />
          <div class="grid grid-cols-2 sm:grid-cols-4 gap-x-8 gap-y-3 flex-1 min-w-[240px]">
            {@render heroStat(portfolio.moduleCount, "Modules")}
            {@render heroStat(portfolio.inFlight, "In flight")}
            {@render heroStat(portfolio.total, "Issues")}
            {@render heroStat(portfolio.done, "Completed")}
          </div>
        </div>

        <!-- Inline-create row. Lives outside any status group because the
             user hasn't picked a status yet — defaults to Active on commit. -->
        {#if creating}
          <div
            bind:this={createRowEl}
            class="mb-6 flex items-center gap-3 p-3 rounded-xl
                   border-l-2 border-l-[var(--btn-success)]
                   bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)]"
          >
            <IconPicker
              value={createEmoji}
              onchange={(v) => { createEmoji = v; }}
            />
            <div class="flex-1 min-w-0">
              <!-- svelte-ignore a11y_autofocus -->
              <input
                type="text"
                bind:value={createName}
                class="w-full text-[0.9375rem] font-medium bg-transparent
                       border-none outline-none text-[var(--text)]
                       placeholder:text-[var(--text-faint)]"
                placeholder="Module name (e.g. Q1 Launch, Auth, Search rework)"
                autofocus
                onkeydown={(e) => {
                  if (e.key === "Enter") commitCreate();
                  if (e.key === "Escape") cancelCreate();
                }}
                onblur={handleCreateBlur}
              />
              {#if createError}
                <p class="text-[0.75rem] text-[var(--error)] mt-1">{createError}</p>
              {/if}
              <p class="text-[0.6875rem] text-[var(--text-faint)] mt-1">
                Enter to create · Esc to cancel · status defaults to Active
              </p>
            </div>
            {#if createSaving}
              <div class="text-[0.75rem] text-[var(--text-faint)] mt-1">Saving...</div>
            {/if}
          </div>
        {/if}

        {#each grouped as group (group.status)}
          {@const isActive = group.status === "active"}
          <section class="mb-8 last:mb-0">
            <!-- Group header. Same uppercase-tracking treatment used by
                 IssueList's status group headers and the sidebar section
                 labels for visual continuity. -->
            <div class="flex items-center gap-2 mb-3 px-1">
              {@render statusIcon(group.status, 13)}
              <h2
                class="text-[0.6875rem] font-semibold uppercase tracking-widest
                       text-[var(--text-muted)]"
              >
                {STATUS_LABEL[group.status] ?? group.status}
              </h2>
              <span class="text-[0.6875rem] text-[var(--text-faint)] tabular-nums">
                {group.mods.length}
              </span>
            </div>

            <!-- Bento grid of ring-tiles. The Active group reads as the
                 focal lane: larger rings, two-up at most so each tile has
                 room; other lifecycle groups pack three-up. -->
            <div
              class="grid grid-cols-1 sm:grid-cols-2 gap-3
                     {isActive ? '' : 'lg:grid-cols-3'}"
            >
              {#each group.mods as mod (mod.id)}
                {@const prog = moduleProgress(mod.id)}
                {@const preview = descriptionPreview(mod.description)}
                {@const ringSize = isActive ? 60 : 48}
                <button
                  class="group text-left rounded-xl bg-[var(--surface)] p-4
                         shadow-[0_1px_2px_rgba(0,0,0,0.06)]
                         hover:shadow-[0_6px_16px_rgba(0,0,0,0.10)]
                         transition-all motion-safe:hover:-translate-y-0.5"
                  onclick={() =>
                    navigate(`/${projectIdentifier}/modules/${mod.id}`)}
                >
                  <div class="flex items-start gap-3.5">
                    <ProgressRing
                      value={prog.frac}
                      size={ringSize}
                      stroke={isActive ? 5 : 4}
                      color="var(--success)"
                    >
                      {#snippet label()}
                        {#if prog.total > 0}
                          <span
                            class="font-semibold tabular-nums text-[var(--text)] leading-none"
                            style="font-size: {Math.round(ringSize * 0.26)}px;"
                          >
                            {Math.round(prog.frac * 100)}<span class="text-[0.7em] text-[var(--text-muted)]">%</span>
                          </span>
                        {:else if mod.emoji}
                          <ProjectIcon value={mod.emoji} size={isActive ? 22 : 18} class="text-[var(--text-faint)]" />
                        {:else}
                          <Layers size={isActive ? 20 : 16} class="text-[var(--text-faint)]" />
                        {/if}
                      {/snippet}
                    </ProgressRing>

                    <div class="flex-1 min-w-0">
                      <div class="flex items-center gap-1.5">
                        {#if mod.emoji}
                          <span class="shrink-0 text-[var(--text-muted)]">
                            <ProjectIcon value={mod.emoji} size={15} />
                          </span>
                        {/if}
                        <span class="text-[0.9375rem] font-medium text-[var(--text)] truncate">
                          {mod.name}
                        </span>
                      </div>
                      <p class="text-[0.75rem] text-[var(--text-muted)] tabular-nums mt-1">
                        {#if prog.total > 0}
                          {prog.done}/{prog.total} done
                        {:else}
                          No issues yet
                        {/if}
                      </p>
                      {#if preview}
                        <p class="text-[0.8125rem] text-[var(--text-faint)] line-clamp-2 mt-1.5 leading-snug">
                          {preview}
                        </p>
                      {/if}
                    </div>
                  </div>
                </button>
              {/each}
            </div>
          </section>
        {/each}
      </div>
    {/if}
  </div>
</div>

<!--
  Status icon snippet. The vocabulary is shared with IssueList /
  IssueDetail's status icons — same shapes for done/cancelled/active
  so modules and issues read as part of the same lifecycle language,
  with planned/paused added for module-specific states.
-->
<!-- Portfolio hero stat: big number over an uppercase label. -->
{#snippet heroStat(value: number, label: string)}
  <div>
    <p class="text-[1.375rem] font-display tracking-tight tabular-nums text-[var(--text)] leading-none">
      {value}
    </p>
    <p class="text-[0.625rem] font-semibold uppercase tracking-widest text-[var(--text-faint)] mt-1">
      {label}
    </p>
  </div>
{/snippet}

{#snippet statusIcon(status: string, size: number)}
  {#if status === "active"}
    <CircleDot {size} class="text-[var(--accent)]" />
  {:else if status === "planned"}
    <Circle {size} class="text-[var(--text-muted)]" />
  {:else if status === "paused"}
    <Pause {size} class="text-[var(--text-muted)]" />
  {:else if status === "done"}
    <CircleCheck {size} class="text-[var(--success)]" />
  {:else if status === "cancelled"}
    <CircleX {size} class="text-[var(--text-faint)]" />
  {:else if status === "backlog"}
    <CircleDashed {size} class="text-[var(--text-faint)]" />
  {:else}
    <Circle {size} class="text-[var(--text-faint)]" />
  {/if}
{/snippet}
