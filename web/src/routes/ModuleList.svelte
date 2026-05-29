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
  import Tooltip from "../lib/Tooltip.svelte";
  import ProjectIcon from "../lib/ProjectIcon.svelte";
  import IconPicker from "../lib/IconPicker.svelte";
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
        onclick={() => navigate(`/${projectIdentifier}/settings`)}
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
               text-[var(--accent-text)] bg-[var(--accent)] px-2.5 py-1
               rounded-md hover:bg-[var(--accent-hover)] transition-colors"
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
      <div class="flex items-center justify-center py-20">
        <p class="text-[var(--error)] text-[0.875rem]">{error}</p>
      </div>
    {:else if modules.length === 0 && !creating}
      <!-- Empty state. Same vocabulary as the PageList empty state so the
           first-time-into-a-tab feel is consistent. -->
      <div class="flex flex-col items-center py-20 gap-3 px-6 max-w-[480px] mx-auto text-center">
        <Layers size={32} class="text-[var(--text-faint)]" />
        <p class="text-[0.9375rem] text-[var(--text-muted)]">No modules yet</p>
        <p class="text-[0.8125rem] text-[var(--text-faint)] leading-relaxed">
          Modules group related issues into a single arc of work — a feature,
          a release, an effort. Create one to start organizing.
        </p>
        <button
          class="text-[0.8125rem] text-[var(--accent)] hover:underline mt-2"
          onclick={startCreate}
        >
          Create the first module
        </button>
      </div>
    {:else}
      <div class="max-w-[860px] mx-auto px-6 py-6">
        <!-- Inline-create row. Lives outside any status group because the
             user hasn't picked a status yet — defaults to Active on commit. -->
        {#if creating}
          <div
            bind:this={createRowEl}
            class="mb-6 flex items-center gap-3 p-3 rounded-md border
                   border-[var(--accent)] bg-[var(--accent-subtle)]"
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

            <div class="flex flex-col gap-1">
              {#each group.mods as mod (mod.id)}
                {@const count = issueCount(mod.id)}
                {@const preview = descriptionPreview(mod.description)}
                <button
                  class="text-left rounded-md border border-[var(--border)]
                         bg-[var(--surface)] px-4 py-3
                         hover:border-[var(--text-faint)]
                         hover:shadow-[0_1px_2px_rgba(0,0,0,0.04)]
                         transition-all"
                  onclick={() =>
                    navigate(`/${projectIdentifier}/modules/${mod.id}`)}
                >
                  <div class="flex items-start gap-3">
                    {#if mod.emoji}
                      <span class="shrink-0 mt-0.5 size-[18px] flex items-center justify-center text-[var(--text-muted)]">
                        <ProjectIcon value={mod.emoji} size={18} />
                      </span>
                    {:else}
                      <Layers size={18} class="shrink-0 text-[var(--text-faint)] mt-0.5" />
                    {/if}
                    <div class="flex-1 min-w-0">
                      <div class="flex items-center gap-2">
                        <span class="text-[0.9375rem] font-medium text-[var(--text)] truncate">
                          {mod.name}
                        </span>
                      </div>
                      {#if preview}
                        <p class="text-[0.8125rem] text-[var(--text-muted)] truncate mt-0.5">
                          {preview}
                        </p>
                      {/if}
                    </div>
                    <div class="shrink-0 flex flex-col items-end gap-0.5 pl-3">
                      <Tooltip
                        content={count === 1 ? "1 issue" : `${count} issues`}
                        placement="left"
                      >
                        <span
                          class="text-[0.75rem] font-medium text-[var(--text-muted)]
                                 tabular-nums bg-[var(--bg-subtle)]
                                 px-2 py-0.5 rounded-full"
                        >
                          {count}
                        </span>
                      </Tooltip>
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
