<script lang="ts">
  // LIF-173 — Plans list view. A first-class project surface alongside
  // Issues / Board / Modules / Pages. Plans are persisted step trees;
  // this lists them grouped by status (active / done / archived).

  import {
    listPlans,
    listProjects,
    createPlan,
    type Plan,
    type Project,
  } from "../lib/api";
  import { startAutoRefresh } from "../lib/autoRefresh.svelte";
  import { ListChecks, Plus, ChevronRight } from "lucide-svelte";
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
  let plans = $state<Plan[]>([]);
  let loading = $state(true);
  let error = $state("");

  let creating = $state(false);
  let createTitle = $state("");
  let createSaving = $state(false);

  const STATUS_ORDER = ["active", "done", "archived"];
  const STATUS_LABEL: Record<string, string> = {
    active: "Active",
    done: "Done",
    archived: "Archived",
  };

  $effect(() => {
    const id = projectIdentifier;
    loadData(id);
  });

  $effect(() =>
    startAutoRefresh({
      refresh: reload,
      isBusy: () => creating || createSaving,
      intervalMs: 15_000,
    }),
  );

  async function loadData(ident: string) {
    loading = true;
    error = "";
    const projRes = await listProjects();
    if (!projRes.ok) { error = projRes.error; loading = false; return; }
    const found = projRes.data.find((p) => p.identifier === ident);
    if (!found) { error = `Project ${ident} not found`; loading = false; return; }
    project = found;
    await reload();
    loading = false;
  }

  async function reload() {
    if (!project) return;
    const res = await listPlans(project.id);
    if (res.ok) plans = res.data;
  }

  let grouped = $derived.by(() => {
    const groups: { status: string; items: Plan[] }[] = [];
    for (const s of STATUS_ORDER) {
      const matching = plans.filter((p) => p.status === s);
      if (matching.length > 0) groups.push({ status: s, items: matching });
    }
    return groups;
  });

  function startCreate() {
    creating = true;
    createTitle = "";
  }

  async function commitCreate() {
    if (!project || !createTitle.trim()) { creating = false; return; }
    createSaving = true;
    const res = await createPlan({ project_id: project.id, title: createTitle.trim() });
    createSaving = false;
    if (res.ok) {
      creating = false;
      navigate(`/${projectIdentifier}/plans/${res.data.id}`);
    } else {
      error = res.error;
    }
  }
</script>

{#snippet topbarContent()}
  <div class="flex items-center gap-3 px-6 py-2 w-full">
    <div class="flex items-center gap-1.5 shrink-0">
      <button
        class="text-body-sm font-mono font-medium text-[var(--text-muted)]
               hover:text-[var(--text)] transition-colors"
        onclick={() => navigate(`/${projectIdentifier}/overview`)}
      >
        {projectIdentifier}
      </button>
      <ChevronRight size={12} class="text-[var(--text-faint)]" />
      <span class="text-body-sm font-medium text-[var(--text)]">Plans</span>
      {#if !loading}
        <span class="ml-1 text-micro text-[var(--text-faint)] font-medium tabular-nums">
          {plans.length}
        </span>
      {/if}
    </div>
    <div class="ml-auto flex items-center gap-1.5 shrink-0">
      <button
        class="flex items-center gap-1 text-body-sm font-medium
               text-[var(--btn-success-text)] bg-[var(--btn-success)]
               px-2.5 py-1 rounded-md hover:bg-[var(--btn-success-hover)]
               transition-colors focus:outline-none
               motion-safe:active:scale-[0.97]"
        onclick={startCreate}
      >
        <Plus size={14} />
        Plan
      </button>
    </div>
  </div>
{/snippet}

<div class="h-full flex flex-col">
  <div class="flex-1 overflow-y-auto">
    {#if loading}
      <div class="flex items-center justify-center py-20">
        <div class="size-6 rounded-full border-2 border-[var(--border)] border-t-[var(--accent)] animate-spin"></div>
      </div>
    {:else if error}
      <ErrorState title="Couldn't load plans" message={error}>
        <button
          class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)] px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
          onclick={() => loadData(projectIdentifier)}
        >
          Try again
        </button>
        <button
          class="text-body-sm text-[var(--text-muted)] border border-[var(--border)] px-3 py-1.5 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
          onclick={() => navigate(`/${projectIdentifier}/overview`)}
        >
          Project overview
        </button>
      </ErrorState>
    {:else}
      <div class="max-w-[860px] mx-auto px-6 py-6">
        {#if creating}
          <div class="mb-6 flex items-center gap-3 p-3 rounded-xl border-l-2 border-l-[var(--btn-success)] bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)]">
            <ListChecks size={16} class="text-[var(--btn-success)]" />
            <input
              class="flex-1 bg-transparent outline-none text-body text-[var(--text)]"
              placeholder="Plan title…"
              bind:value={createTitle}
              autofocus
              onkeydown={(e) => {
                if (e.key === "Enter") commitCreate();
                if (e.key === "Escape") creating = false;
              }}
            />
            <button
              class="text-body-sm font-medium text-[var(--btn-success)] hover:underline disabled:opacity-50"
              disabled={createSaving || !createTitle.trim()}
              onclick={commitCreate}
            >
              Create
            </button>
          </div>
        {/if}

        {#if plans.length === 0 && !creating}
          <div class="flex flex-col items-center py-16 gap-4 px-6 max-w-[480px] mx-auto text-center">
            <Mascot src="/LizzyWriting.png" nativeW={567} nativeH={562} />
            <div class="flex flex-col items-center gap-1.5">
              <p class="text-[1.0625rem] font-medium text-[var(--text)]">The drawing board's empty</p>
              <p class="text-body-sm text-[var(--text-muted)] leading-relaxed">
                A plan breaks a goal into a tree of steps that survives across
                sessions. Steps can mirror issues, so closing an issue checks
                off its step.
              </p>
            </div>
            <button
              class="flex items-center gap-1.5 mt-1 text-body-sm font-medium
                     text-[var(--btn-success-text)] bg-[var(--btn-success)]
                     px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)]
                     transition-colors"
              onclick={startCreate}
            >
              <Plus size={15} />
              Create a plan
            </button>
          </div>
        {:else}
          {#each grouped as group (group.status)}
            <div class="mb-6">
              <h2 class="text-micro font-semibold uppercase tracking-wide text-[var(--text-faint)] mb-2">
                {STATUS_LABEL[group.status] ?? group.status}
                <span class="ml-1 tabular-nums">{group.items.length}</span>
              </h2>
              <div class="flex flex-col gap-2">
                {#each group.items as plan (plan.id)}
                  {@const frac = plan.step_count > 0 ? plan.done_count / plan.step_count : 0}
                  <button
                    class="group flex items-center gap-3.5 p-3 rounded-xl bg-[var(--surface)]
                           shadow-[0_1px_2px_rgba(0,0,0,0.06)]
                           hover:shadow-[0_6px_16px_rgba(0,0,0,0.10)]
                           transition-all motion-safe:hover:-translate-y-0.5 text-left"
                    onclick={() => navigate(`/${projectIdentifier}/plans/${plan.id}`)}
                  >
                    <ProgressRing value={frac} size={40} stroke={4} color="var(--success)">
                      {#snippet label()}
                        {#if plan.step_count > 0}
                          <span class="text-micro font-semibold tabular-nums text-[var(--text)] leading-none">
                            {Math.round(frac * 100)}
                          </span>
                        {:else}
                          <ListChecks size={15} class="text-[var(--text-faint)]" />
                        {/if}
                      {/snippet}
                    </ProgressRing>
                    <div class="flex-1 min-w-0">
                      <div class="text-body text-[var(--text)] truncate">{plan.title}</div>
                      <div class="text-caption text-[var(--text-faint)] font-mono">
                        {plan.identifier}{plan.anchor_identifier ? ` · anchor ${plan.anchor_identifier}` : ""}
                      </div>
                    </div>
                    <div class="text-caption text-[var(--text-muted)] tabular-nums shrink-0">
                      {plan.done_count}/{plan.step_count}
                    </div>
                  </button>
                {/each}
              </div>
            </div>
          {/each}
        {/if}
      </div>
    {/if}
  </div>
</div>


