<script lang="ts">
  // LIF-186 right sidebar, reworked as a project "pulse": completion, the
  // status mix, what is still open by priority and module, labels, and what
  // moved most recently. Every breakdown is project-wide (the parent passes
  // the unfiltered set) and every row doubles as a filter toggle, so the
  // panel is both a summary and the fastest way to narrow the list.
  import type { Issue, Label, Module } from "../api";
  import { Layers, PanelRightClose, PanelRightOpen, X } from "lucide-svelte";
  import PriorityIcon from "../PriorityIcon.svelte";
  import ProjectIcon from "../ProjectIcon.svelte";
  import ProgressRing from "../ProgressRing.svelte";
  import StatusIcon, { statusCssColor } from "../StatusIcon.svelte";
  import TimeAgo from "../TimeAgo.svelte";
  import Tooltip from "../Tooltip.svelte";
  import { safeLabelColor } from "../labelColors";
  import { PRIORITIES, STATUSES } from "./grouping";
  import type { IssueListState } from "./state.svelte";
  import {
    loadRightSidebarCollapsed,
    saveRightSidebarCollapsed,
  } from "./rightSidebarPrefs";

  let {
    issues,
    modules,
    labels,
    view,
    onPeek,
  }: {
    issues: Issue[];
    modules: Module[];
    labels: Label[];
    view: IssueListState;
    onPeek: (issue: Issue) => void;
  } = $props();

  let collapsed = $state(loadRightSidebarCollapsed());

  function toggleCollapsed() {
    collapsed = !collapsed;
    saveRightSidebarCollapsed(collapsed);
  }

  const OPEN = new Set(["backlog", "todo", "active"]);

  let stats = $derived.by(() => {
    const status: Record<string, number> = Object.fromEntries(STATUSES.map((s) => [s, 0]));
    const openPrio: Record<string, number> = Object.fromEntries(PRIORITIES.map((p) => [p, 0]));
    const mod = new Map<number, { open: number; done: number; total: number }>();
    const label = new Map<string, number>();
    for (const i of issues) {
      status[i.status] = (status[i.status] ?? 0) + 1;
      const open = OPEN.has(i.status);
      if (open) openPrio[i.priority] = (openPrio[i.priority] ?? 0) + 1;
      if (i.module_id != null) {
        const m = mod.get(i.module_id) ?? { open: 0, done: 0, total: 0 };
        m.total++;
        if (open) m.open++;
        if (i.status === "done") m.done++;
        mod.set(i.module_id, m);
      }
      for (const l of i.labels) label.set(l, (label.get(l) ?? 0) + 1);
    }
    const total = issues.length;
    const open = status.backlog + status.todo + status.active;
    // Cancelled work is neither done nor owed, so it leaves the denominator.
    const countable = total - status.cancelled;
    return {
      status,
      openPrio,
      mod,
      label,
      total,
      open,
      done: status.done,
      completion: countable > 0 ? status.done / countable : 0,
      maxPrio: Math.max(1, ...Object.values(openPrio)),
    };
  });

  let recent = $derived(
    [...issues].sort((a, b) => b.updated_at.localeCompare(a.updated_at)).slice(0, 5),
  );

  let usedLabels = $derived(
    labels
      .filter((l) => (stats.label.get(l.name) ?? 0) > 0)
      .sort((a, b) => (stats.label.get(b.name) ?? 0) - (stats.label.get(a.name) ?? 0)),
  );

  const rowBase =
    "w-full flex items-center gap-2.5 px-2 h-8 rounded-md text-left text-body-sm transition-colors";
  const rowIdle =
    "text-[var(--text-muted)] hover:bg-[var(--surface)] hover:text-[var(--text)]";
  const rowOn =
    "bg-[var(--surface)] text-[var(--text)] font-medium shadow-[0_1px_2px_rgba(0,0,0,0.08),inset_0_0_0_1px_var(--border)]";
</script>

{#snippet sectionHead(title: string, active: boolean, clear: () => void)}
  <div class="flex items-center justify-between h-6 mb-1.5 px-2">
    <h3 class="text-micro font-semibold uppercase tracking-widest text-[var(--text-faint)]">
      {title}
    </h3>
    {#if active}
      <button
        class="flex items-center gap-1 text-micro font-medium text-[var(--accent)]
               hover:underline underline-offset-2"
        onclick={clear}
      >
        <X size={11} /> Clear
      </button>
    {/if}
  </div>
{/snippet}

{#if collapsed}
  <button
    class="hidden xl:flex flex-col items-center gap-2 w-8 shrink-0 py-4
           border-l border-[var(--border)] bg-[var(--bg-subtle)]
           text-[var(--text-faint)] hover:text-[var(--text)]
           hover:bg-[var(--surface)] transition-colors"
    onclick={toggleCollapsed}
    title="Expand issues sidebar"
    aria-label="Expand issues sidebar"
  >
    <PanelRightOpen size={15} />
    <span class="text-micro font-semibold uppercase tracking-widest [writing-mode:vertical-rl]">
      Pulse
    </span>
  </button>
{:else}
<aside
  class="pulse hidden xl:flex flex-col gap-7 w-[296px] shrink-0 overflow-y-auto
         border-l border-[var(--border)] bg-[var(--bg-subtle)] px-4 pt-4 pb-8"
  aria-label="Project pulse"
>
  <!-- Completion header -->
  <div class="flex items-start gap-4 px-2">
    <ProgressRing value={stats.completion} size={60} stroke={5} />
    <div class="min-w-0 flex-1 pt-1">
      <p class="font-display text-title tracking-tight leading-none tabular-nums text-[var(--text)]">
        {stats.open}
        <span class="text-body-sm font-body tracking-normal text-[var(--text-muted)]">open</span>
      </p>
      <p class="mt-1.5 text-caption text-[var(--text-faint)] tabular-nums">
        {stats.done} of {stats.total - stats.status.cancelled} done
      </p>
      {#if stats.status.active > 0}
        <p class="mt-0.5 flex items-center gap-1.5 text-caption text-[var(--accent)] tabular-nums">
          <span class="size-1.5 rounded-full bg-current"></span>
          {stats.status.active} in progress
        </p>
      {/if}
    </div>
    <button
      class="-mr-1 size-7 shrink-0 grid place-items-center rounded-md
             text-[var(--text-faint)] hover:text-[var(--text)]
             hover:bg-[var(--surface)] transition-colors"
      onclick={toggleCollapsed}
      title="Collapse sidebar"
      aria-label="Collapse sidebar"
    >
      <PanelRightClose size={15} />
    </button>
  </div>

  <!-- Status mix: one stacked bar, then the legend as filter toggles. These
       replaced the cryptic icon tallies that used to crowd the topbar. -->
  <section>
    {@render sectionHead("Status", view.filterStatus.length > 0, () => (view.filterStatus = []))}
    {#if stats.total > 0}
      <div class="mx-2 mb-2.5 flex h-1.5 rounded-full overflow-hidden bg-[var(--border)] gap-px">
        {#each STATUSES as s (s)}
          {#if stats.status[s] > 0}
            <div
              class="pulse-seg h-full"
              style="flex-grow: {stats.status[s]}; background: {statusCssColor(s)};
                     opacity: {s === 'cancelled' ? 0.4 : view.filterStatus.length && !view.filterStatus.includes(s) ? 0.35 : 1}"
            ></div>
          {/if}
        {/each}
      </div>
    {/if}
    <div class="flex flex-col gap-0.5">
      {#each STATUSES as s (s)}
        {@const on = view.filterStatus.includes(s)}
        <button
          class="{rowBase} {on ? rowOn : rowIdle}"
          aria-pressed={on}
          aria-label="Filter by status {s} ({stats.status[s]})"
          onclick={() => view.toggleStatusFilter(s)}
        >
          <StatusIcon status={s} size={14} />
          <span class="flex-1 capitalize">{s}</span>
          <span class="tabular-nums text-caption {stats.status[s] ? 'text-[var(--text-muted)]' : 'text-[var(--text-faint)] opacity-60'}">
            {stats.status[s]}
          </span>
        </button>
      {/each}
    </div>
  </section>

  <!-- Open work by priority, as meters so the shape reads at a glance. -->
  <section>
    {@render sectionHead("Open by priority", view.filterPriority.length > 0, () => (view.filterPriority = []))}
    <div class="flex flex-col gap-0.5">
      {#each PRIORITIES as p (p)}
        {@const n = stats.openPrio[p]}
        {@const on = view.filterPriority.includes(p)}
        {#if n > 0 || on}
          <button
            class="{rowBase} {on ? rowOn : rowIdle}"
            aria-pressed={on}
            aria-label="Filter by priority {p} ({n} open)"
            onclick={() => view.togglePriorityFilter(p)}
          >
            <span class="w-3.5 flex justify-center"><PriorityIcon priority={p} size={14} /></span>
            <span class="w-[3.75rem] capitalize">{p}</span>
            <span class="flex-1 h-1 rounded-full bg-[var(--border)] overflow-hidden">
              <span
                class="pulse-meter block h-full rounded-full"
                style="width: {(n / stats.maxPrio) * 100}%; background: {p === 'urgent'
                  ? 'var(--error)'
                  : p === 'high'
                    ? 'var(--warn)'
                    : p === 'medium'
                      ? 'var(--accent)'
                      : 'var(--text-faint)'}"
              ></span>
            </span>
            <span class="w-5 text-right tabular-nums text-caption text-[var(--text-muted)]">{n}</span>
          </button>
        {/if}
      {/each}
    </div>
  </section>

  {#if modules.length > 0}
    <section>
      {@render sectionHead("Modules", view.filterModule.length > 0, () => (view.filterModule = []))}
      <div class="flex flex-col gap-0.5">
        {#each modules as m (m.id)}
          {@const s = stats.mod.get(m.id) ?? { open: 0, done: 0, total: 0 }}
          {@const on = view.filterModule.includes(m.name)}
          <div class="[&>span]:w-full">
          <Tooltip content={s.total ? `${s.done} of ${s.total} done · ${s.open} open` : "No issues yet"} placement="left">
            <button
              class="w-full flex items-center gap-2.5 px-2 py-1.5 rounded-md text-left text-body-sm
                     transition-colors {on ? rowOn : rowIdle}"
              aria-pressed={on}
              aria-label="Filter by module {m.name} ({s.open} open)"
              onclick={() => view.toggleModuleFilter(m.name)}
            >
              <span class="shrink-0 text-[var(--text-faint)]">
                {#if m.emoji}<ProjectIcon value={m.emoji} size={14} />{:else}<Layers size={14} />{/if}
              </span>
              <span class="flex-1 min-w-0 flex flex-col gap-1">
                <span class="truncate leading-tight">{m.name}</span>
                <span class="h-[3px] rounded-full bg-[var(--border)] overflow-hidden">
                  <span
                    class="pulse-meter block h-full rounded-full bg-[var(--success)]"
                    style="width: {s.total ? (s.done / s.total) * 100 : 0}%"
                  ></span>
                </span>
              </span>
              <span class="w-5 text-right tabular-nums text-caption text-[var(--text-muted)]">{s.open}</span>
            </button>
          </Tooltip>
          </div>
        {/each}
      </div>
    </section>
  {/if}

  {#if usedLabels.length > 0}
    <section>
      {@render sectionHead("Labels", view.filterLabel.length > 0, () => (view.filterLabel = []))}
      <div class="flex flex-wrap gap-1.5 px-2">
        {#each usedLabels as l (l.id)}
          {@const on = view.filterLabel.includes(l.name)}
          <button
            class="inline-flex items-center gap-1.5 h-6 pl-2 pr-1.5 rounded-full text-caption
                   border transition-colors
                   {on
              ? 'border-transparent text-[var(--text)] font-medium'
              : 'border-[var(--border)] text-[var(--text-muted)] hover:text-[var(--text)] hover:border-[var(--text-faint)]'}"
            style={on ? `background: color-mix(in srgb, ${safeLabelColor(l.color)} 22%, transparent)` : ""}
            aria-pressed={on}
            aria-label="Filter by label {l.name} ({stats.label.get(l.name)})"
            onclick={() => view.toggleLabelFilter(l.name)}
          >
            <span class="size-2 rounded-full shrink-0" style="background: {safeLabelColor(l.color)}"></span>
            {l.name}
            <span class="tabular-nums text-micro text-[var(--text-faint)]">{stats.label.get(l.name)}</span>
          </button>
        {/each}
      </div>
    </section>
  {/if}

  {#if recent.length > 0}
    <section>
      {@render sectionHead("Recently updated", false, () => {})}
      <div class="flex flex-col gap-0.5">
        {#each recent as issue (issue.id)}
          <button
            class="w-full flex items-start gap-2.5 px-2 py-1.5 rounded-md text-left
                   text-[var(--text-muted)] hover:bg-[var(--surface)] hover:text-[var(--text)] transition-colors"
            onclick={() => onPeek(issue)}
            title="Preview {issue.identifier}"
          >
            <span class="mt-0.5"><StatusIcon status={issue.status} size={13} /></span>
            <span class="flex-1 min-w-0">
              <span class="block text-body-sm leading-snug line-clamp-2">{issue.title}</span>
              <span class="block mt-0.5 text-micro text-[var(--text-faint)]">
                <span class="font-mono">{issue.identifier}</span> · <TimeAgo date={issue.updated_at} />
              </span>
            </span>
          </button>
        {/each}
      </div>
    </section>
  {/if}
</aside>
{/if}

<style>
  .pulse-seg,
  .pulse-meter {
    transition:
      flex-grow 400ms var(--ease-out-expo),
      width 400ms var(--ease-out-expo),
      opacity 200ms ease;
  }
</style>
