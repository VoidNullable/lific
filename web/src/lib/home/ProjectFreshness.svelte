<script lang="ts">
  // LIF-507: which projects with open work are being looked after. Freshness
  // is days since the project's last recorded change, in four bands.
  import type { Project, ProjectPulse } from "../api";
  import ProjectIcon from "../ProjectIcon.svelte";
  import { daysSince, ageLabel } from "../issues/importance";

  let {
    pulses,
    projects,
    navigate,
  }: {
    pulses: ProjectPulse[];
    projects: Project[];
    navigate: (path: string) => void;
  } = $props();

  const CAP = 5;
  let expanded = $state(false);

  function band(days: number | null): { label: string; color: string } {
    if (days === null || days >= 45) return { label: "neglected", color: "var(--heat-6)" };
    if (days >= 21) return { label: "stale", color: "var(--heat-5)" };
    if (days >= 7) return { label: "aging", color: "var(--heat-3)" };
    return { label: "fresh", color: "var(--heat-2)" };
  }

  let rows = $derived.by(() => {
    const byId = new Map(projects.map((p) => [p.id, p]));
    return pulses
      .filter((p) => p.open > 0 && byId.has(p.project_id))
      .map((p) => ({
        project: byId.get(p.project_id)!,
        open: p.open,
        days: p.last_activity ? daysSince(p.last_activity) : null,
      }))
      .sort((a, b) => (a.days ?? Infinity) - (b.days ?? Infinity));
  });
  let shown = $derived(expanded ? rows : rows.slice(0, CAP));
</script>

{#if rows.length > 0}
  <div class="flex flex-col">
    {#each shown as row (row.project.id)}
      {@const b = band(row.days)}
      <button
        class="flex items-center gap-2.5 px-2 py-1.5 -mx-2 rounded-md text-left hover:bg-[var(--bg-subtle)] transition-colors"
        onclick={() => navigate(`/${row.project.identifier}/overview`)}
      >
        {#if row.project.emoji}
          <ProjectIcon value={row.project.emoji} size={14} />
        {:else}
          <span
            class="size-5 rounded-md border border-[var(--border)] bg-[var(--bg-subtle)] flex items-center justify-center
                   text-micro font-semibold text-[var(--text-muted)] shrink-0"
          >
            {row.project.identifier.slice(0, 2)}
          </span>
        {/if}
        <span class="flex-1 min-w-0 truncate text-body-sm text-[var(--text)]">{row.project.name}</span>
        <span class="shrink-0 text-micro tabular-nums text-[var(--text-faint)]" title="Open issues">{row.open} open</span>
        <span class="shrink-0 w-[92px] flex items-center gap-1.5 text-micro text-[var(--text-muted)]">
          <span class="size-1.5 rounded-full shrink-0" style="background: {b.color}"></span>
          {b.label}
          <span class="ml-auto tabular-nums text-[var(--text-faint)]">{row.days === null ? "" : ageLabel(row.days)}</span>
        </span>
      </button>
    {/each}
    {#if rows.length > CAP}
      <button
        class="self-start mt-1 text-caption text-[var(--text-muted)] hover:text-[var(--text)] transition-colors"
        onclick={() => (expanded = !expanded)}
      >
        {expanded ? "Show fewer" : `Show all ${rows.length}`}
      </button>
    {/if}
  </div>
{:else}
  <p class="text-body-sm text-[var(--text-muted)]">No project has open work.</p>
{/if}
