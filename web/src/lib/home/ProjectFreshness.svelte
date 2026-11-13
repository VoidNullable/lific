<script lang="ts">
  // LIF-507: which projects with open work are being looked after. A bar per
  // project, as long as its open count and colored by freshness: days since
  // the project's last recorded change, in four bands.
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

  const CAP = 8;
  let expanded = $state(false);

  const BANDS = [
    { label: "Fresh", detail: "under a week", color: "var(--heat-2)" },
    { label: "Aging", detail: "1 to 3 weeks", color: "var(--heat-3)" },
    { label: "Stale", detail: "3 to 6 weeks", color: "var(--heat-5)" },
    { label: "Neglected", detail: "6 weeks+", color: "var(--heat-6)" },
  ];

  function band(days: number | null) {
    if (days === null || days >= 45) return BANDS[3];
    if (days >= 21) return BANDS[2];
    if (days >= 7) return BANDS[1];
    return BANDS[0];
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
  let max = $derived(Math.max(1, ...rows.map((r) => r.open)));
  let shown = $derived(expanded ? rows : rows.slice(0, CAP));
</script>

{#if rows.length > 0}
  <div class="flex flex-wrap items-center gap-x-5 gap-y-1 mb-3">
    {#each BANDS as b (b.label)}
      <span class="flex items-center gap-2 text-caption text-[var(--text-muted)]" title="Last change {b.detail} ago">
        <span class="size-3 shrink-0" style="background: {b.color}"></span>
        {b.label}
      </span>
    {/each}
  </div>
  <div class="flex flex-col border-t border-[var(--border)]">
    {#each shown as row (row.project.id)}
      {@const b = band(row.days)}
      <button
        class="freshness-row items-center gap-3 py-2 text-left border-b border-[var(--border)] hover:bg-[var(--chart-hover)] transition-colors"
        onclick={() => navigate(`/${row.project.identifier}/overview`)}
        title="{row.project.name}: {row.open} open, last change {row.days === null ? 'never' : `${ageLabel(row.days)} ago`}"
      >
        <span class="flex items-center gap-2.5 min-w-0">
          {#if row.project.emoji}
            <ProjectIcon value={row.project.emoji} size={15} />
          {:else}
            <span
              class="size-5 rounded-md bg-[var(--bg-subtle)] flex items-center justify-center
                     text-micro font-semibold text-[var(--text-muted)] shrink-0"
            >
              {row.project.identifier.slice(0, 2)}
            </span>
          {/if}
          <span class="truncate text-body-sm text-[var(--text)]">{row.project.name}</span>
        </span>
        <span class="flex items-center gap-2 min-w-0">
          <span class="block h-3.5" style="width: {(row.open / max) * 100}%; min-width: 3px; background: {b.color}"></span>
          <span class="shrink-0 text-caption tabular-nums text-[var(--text)]">{row.open}</span>
        </span>
        <span class="text-caption tabular-nums text-[var(--text-muted)] text-right">
          {row.days === null ? "never" : ageLabel(row.days)}
        </span>
      </button>
    {/each}
  </div>
  {#if rows.length > CAP}
    <button
      class="mt-2 text-caption text-[var(--text-muted)] hover:text-[var(--text)] transition-colors"
      onclick={() => (expanded = !expanded)}
    >
      {expanded ? "Show fewer" : `Show all ${rows.length}`}
    </button>
  {/if}
{:else}
  <p class="text-body-sm text-[var(--text-muted)]">No project has open work.</p>
{/if}

<style>
  .freshness-row {
    display: grid;
    grid-template-columns: minmax(7rem, 11rem) minmax(0, 1fr) 3.5rem;
  }
</style>
