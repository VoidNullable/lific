<script lang="ts">
  import {
    listIssues,
    listProjects,
    listModules,
    listLabels,
    updateIssue,
    type Issue,
    type Project,
    type Module,
    type Label,
  } from "../lib/api";
  import { Plus, Search, CircleCheckBig, CircleX } from "lucide-svelte";

  let {
    navigate,
    projectIdentifier,
  }: {
    navigate: (path: string) => void;
    projectIdentifier: string;
  } = $props();

  let project = $state<Project | null>(null);
  let issues = $state<Issue[]>([]);
  let modules = $state<Module[]>([]);
  let labels = $state<Label[]>([]);
  let loading = $state(true);
  let error = $state("");

  // Filters
  let filterStatus = $state<string>("");
  let filterPriority = $state<string>("");
  let filterLabel = $state<string>("");
  let filterModule = $state<string>("");
  let searchQuery = $state("");

  const STATUSES = ["backlog", "todo", "active", "done", "cancelled"];
  const PRIORITIES = ["urgent", "high", "medium", "low", "none"];

  // Re-run when the project prop changes (read it synchronously so Svelte tracks it)
  $effect(() => {
    const id = projectIdentifier;
    // Reset filters when switching projects
    filterStatus = "";
    filterPriority = "";
    filterLabel = "";
    filterModule = "";
    searchQuery = "";
    loadProject(id);
  });

  // Reload issues when filters change
  $effect(() => {
    // Reference the filter values to create dependency
    filterStatus;
    filterPriority;
    filterLabel;
    filterModule;
    if (project) {
      loadIssues();
    }
  });

  async function loadProject(identifier: string) {
    loading = true;
    error = "";
    const projRes = await listProjects();
    if (!projRes.ok) {
      error = projRes.error;
      loading = false;
      return;
    }

    const found = projRes.data.find(
      (p: Project) => p.identifier.toLowerCase() === identifier.toLowerCase()
    );
    if (!found) {
      error = `Project ${identifier} not found`;
      loading = false;
      return;
    }
    project = found;

    // Load modules, labels, and issues in parallel
    const [modRes, lblRes] = await Promise.all([
      listModules(found.id),
      listLabels(found.id),
    ]);

    if (modRes.ok) modules = modRes.data;
    if (lblRes.ok) labels = lblRes.data;

    await loadIssues();
    loading = false;
  }

  async function loadIssues() {
    if (!project) return;

    const filters: Record<string, unknown> = {
      project_id: project.id,
      limit: 200,
    };
    if (filterStatus) filters.status = filterStatus;
    if (filterPriority) filters.priority = filterPriority;
    if (filterLabel) filters.label = filterLabel;
    if (filterModule) {
      const mod = modules.find((m) => m.name === filterModule);
      if (mod) filters.module_id = mod.id;
    }

    const res = await listIssues(filters);
    if (res.ok) {
      issues = res.data;
    }
  }

  // Client-side search filter
  let filteredIssues = $derived(
    searchQuery
      ? issues.filter(
          (i) =>
            i.title.toLowerCase().includes(searchQuery.toLowerCase()) ||
            i.identifier.toLowerCase().includes(searchQuery.toLowerCase())
        )
      : issues
  );

  // Group issues by status for the list view
  let groupedByStatus = $derived.by(() => {
    if (filterStatus) return null; // Don't group when filtered to single status
    const groups: Record<string, Issue[]> = {};
    for (const status of STATUSES) {
      const matching = filteredIssues.filter((i) => i.status === status);
      if (matching.length > 0) groups[status] = matching;
    }
    return groups;
  });

  // Quick status update
  async function cycleStatus(issue: Issue, e: Event) {
    e.stopPropagation();
    const idx = STATUSES.indexOf(issue.status);
    const next = STATUSES[(idx + 1) % STATUSES.length];
    const res = await updateIssue(issue.id, { status: next });
    if (res.ok) {
      issue.status = next;
      issues = [...issues]; // trigger reactivity
    }
  }

  function hasActiveFilters(): boolean {
    return !!(filterStatus || filterPriority || filterLabel || filterModule);
  }

  function clearFilters() {
    filterStatus = "";
    filterPriority = "";
    filterLabel = "";
    filterModule = "";
    searchQuery = "";
  }

  function formatRelativeDate(iso: string): string {
    const d = new Date(iso + "Z");
    const now = new Date();
    const diffMs = now.getTime() - d.getTime();
    const diffMins = Math.floor(diffMs / 60000);
    const diffHrs = Math.floor(diffMs / 3600000);
    const diffDays = Math.floor(diffMs / 86400000);

    if (diffMins < 1) return "just now";
    if (diffMins < 60) return `${diffMins}m ago`;
    if (diffHrs < 24) return `${diffHrs}h ago`;
    if (diffDays < 7) return `${diffDays}d ago`;
    return d.toLocaleDateString("en-US", { month: "short", day: "numeric" });
  }
</script>

<div class="h-full flex flex-col">
  <!-- Header -->
  <div class="shrink-0 border-b border-[var(--border)] bg-[var(--surface)]">
    <div class="flex items-center justify-between px-6 py-3">
      <div class="flex items-center gap-3">
        <h1 class="text-[1.125rem] font-semibold text-[var(--text)] tracking-tight">
          Issues
        </h1>
        {#if !loading}
          <span class="text-[0.8125rem] text-[var(--text-faint)]">
            {filteredIssues.length}
          </span>
        {/if}
      </div>

      <div class="flex items-center gap-2">
        <!-- Create -->
        <button
          class="flex items-center gap-1.5 text-[0.8125rem] font-medium
                 text-[var(--accent-text)] bg-[var(--accent)] px-2.5 py-1.5
                 rounded-md hover:bg-[var(--accent-hover)] transition-colors"
          onclick={() => navigate(`/${projectIdentifier}/issues/new`)}
        >
          <Plus size={14} />
          New
        </button>

        <!-- Search -->
        <div class="relative">
          <div class="absolute left-2.5 top-1/2 -translate-y-1/2 pointer-events-none text-[var(--text-faint)]">
            <Search size={14} />
          </div>
        <input
          type="text"
          placeholder="Search issues..."
          bind:value={searchQuery}
          class="w-[200px] pl-8 pr-3 py-1.5 text-[0.8125rem] rounded-md
                 border border-[var(--border)] bg-[var(--bg)]
                 text-[var(--text)] placeholder:text-[var(--text-faint)]
                 focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_var(--accent-subtle)]
                 transition-all"
        />
        </div>
      </div>
    </div>

    <!-- Filter bar -->
    <div class="flex items-center gap-2 px-6 pb-3 flex-wrap">
      <!-- Status filter -->
      <select
        bind:value={filterStatus}
        class="text-[0.8125rem] rounded-md border border-[var(--border)]
               bg-[var(--surface)] text-[var(--text)] px-2.5 py-1
               focus:border-[var(--accent)] focus:outline-none
               {filterStatus ? 'border-[var(--accent)] text-[var(--accent)]' : ''}"
      >
        <option value="">All statuses</option>
        {#each STATUSES as status}
          <option value={status}>{status}</option>
        {/each}
      </select>

      <!-- Priority filter -->
      <select
        bind:value={filterPriority}
        class="text-[0.8125rem] rounded-md border border-[var(--border)]
               bg-[var(--surface)] text-[var(--text)] px-2.5 py-1
               focus:border-[var(--accent)] focus:outline-none
               {filterPriority ? 'border-[var(--accent)] text-[var(--accent)]' : ''}"
      >
        <option value="">All priorities</option>
        {#each PRIORITIES as priority}
          <option value={priority}>{priority}</option>
        {/each}
      </select>

      <!-- Label filter -->
      {#if labels.length > 0}
        <select
          bind:value={filterLabel}
          class="text-[0.8125rem] rounded-md border border-[var(--border)]
                 bg-[var(--surface)] text-[var(--text)] px-2.5 py-1
                 focus:border-[var(--accent)] focus:outline-none
                 {filterLabel ? 'border-[var(--accent)] text-[var(--accent)]' : ''}"
        >
          <option value="">All labels</option>
          {#each labels as label}
            <option value={label.name}>{label.name}</option>
          {/each}
        </select>
      {/if}

      <!-- Module filter -->
      {#if modules.length > 0}
        <select
          bind:value={filterModule}
          class="text-[0.8125rem] rounded-md border border-[var(--border)]
                 bg-[var(--surface)] text-[var(--text)] px-2.5 py-1
                 focus:border-[var(--accent)] focus:outline-none
                 {filterModule ? 'border-[var(--accent)] text-[var(--accent)]' : ''}"
        >
          <option value="">All modules</option>
          {#each modules as mod}
            <option value={mod.name}>{mod.name}</option>
          {/each}
        </select>
      {/if}

      <!-- Clear filters -->
      {#if hasActiveFilters()}
        <button
          class="text-[0.8125rem] text-[var(--accent)] px-2 py-1
                 rounded-md hover:bg-[var(--accent-subtle)] transition-colors"
          onclick={clearFilters}
        >
          Clear filters
        </button>
      {/if}
    </div>
  </div>

  <!-- Issue list -->
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
    {:else if filteredIssues.length === 0}
      <div class="flex flex-col items-center justify-center py-20 gap-2">
        <p class="text-[var(--text-muted)] text-[0.9375rem]">
          {hasActiveFilters() || searchQuery ? "No issues match your filters" : "No issues yet"}
        </p>
        {#if hasActiveFilters() || searchQuery}
          <button
            class="text-[0.8125rem] text-[var(--accent)]
                   hover:underline transition-colors"
            onclick={clearFilters}
          >
            Clear filters
          </button>
        {/if}
      </div>
    {:else if groupedByStatus && !filterStatus}
      <!-- Grouped view -->
      {#each Object.entries(groupedByStatus) as [status, statusIssues] (status)}
        <div class="border-b border-[var(--border)] last:border-b-0">
          <div
            class="sticky top-0 z-10 flex items-center gap-2 px-6 py-2
                   bg-[var(--bg)] border-b border-[var(--border)]"
          >
            <span class="inline-flex items-center gap-1.5">
              <span class="size-2 rounded-full {statusColor(status)}"></span>
              <span
                class="text-[0.75rem] font-semibold uppercase tracking-widest
                       text-[var(--text-muted)]"
              >
                {status}
              </span>
            </span>
            <span class="text-[0.75rem] text-[var(--text-faint)]">
              {statusIssues.length}
            </span>
          </div>
          {#each statusIssues as issue (issue.id)}
            {@render issueRow(issue)}
          {/each}
        </div>
      {/each}
    {:else}
      <!-- Flat list -->
      {#each filteredIssues as issue (issue.id)}
        {@render issueRow(issue)}
      {/each}
    {/if}
  </div>
</div>

{#snippet issueRow(issue: Issue)}
  <div
    class="w-full flex items-center gap-3 px-6 py-2.5 text-left
           border-b border-[var(--border)] last:border-b-0
           hover:bg-[var(--bg-subtle)] transition-colors group cursor-pointer"
    role="button"
    tabindex="0"
    onclick={() => navigate(`/${projectIdentifier}/issues/${issue.identifier}`)}
    onkeydown={(e) => { if (e.key === "Enter") navigate(`/${projectIdentifier}/issues/${issue.identifier}`); }}
  >
    <!-- Status indicator (clickable to cycle) -->
    <button
      class="size-4 shrink-0 transition-colors flex items-center justify-center
             {issue.status === 'done' || issue.status === 'cancelled'
        ? 'border-0'
        : 'rounded-full border-2 ' + statusBorderColor(issue.status)}
             hover:text-[var(--accent)]"
      title="Status: {issue.status} (click to cycle)"
      onclick={(e) => cycleStatus(issue, e)}
    >
      {#if issue.status === "done"}
        <CircleCheckBig size={16} class="text-[var(--success)]" />
      {:else if issue.status === "cancelled"}
        <CircleX size={16} class="text-[var(--text-faint)]" />
      {/if}
    </button>

    <!-- Identifier -->
    <span
      class="text-[0.8125rem] text-[var(--text-faint)] font-mono shrink-0 w-[72px]"
    >
      {issue.identifier}
    </span>

    <!-- Title -->
    <span
      class="flex-1 text-[0.875rem] text-[var(--text)] truncate
             {issue.status === 'done' || issue.status === 'cancelled'
        ? 'line-through text-[var(--text-muted)]'
        : ''}"
    >
      {issue.title}
    </span>

    <!-- Labels -->
    {#if issue.labels.length > 0}
      <div class="flex items-center gap-1 shrink-0">
        {#each issue.labels.slice(0, 2) as lbl}
          {@const labelObj = labels.find((l) => l.name === lbl)}
          <span
            class="text-[0.6875rem] font-medium px-1.5 py-0.5 rounded-full
                   border border-[var(--border)]"
            style={labelObj ? `color: ${labelObj.color}; border-color: ${labelObj.color}40;` : ""}
          >
            {lbl}
          </span>
        {/each}
        {#if issue.labels.length > 2}
          <span class="text-[0.6875rem] text-[var(--text-faint)]">
            +{issue.labels.length - 2}
          </span>
        {/if}
      </div>
    {/if}

    <!-- Priority badge -->
    <span
      class="text-[0.6875rem] font-medium shrink-0 w-[52px] text-right
             {priorityColor(issue.priority)}"
    >
      {#if issue.priority !== "none"}
        {issue.priority}
      {/if}
    </span>

    <!-- Updated time -->
    <span class="text-[0.75rem] text-[var(--text-faint)] shrink-0 w-[60px] text-right">
      {formatRelativeDate(issue.updated_at)}
    </span>
  </div>
{/snippet}

<script lang="ts" module>
  function statusColor(status: string): string {
    switch (status) {
      case "backlog": return "bg-[var(--text-faint)]";
      case "todo": return "bg-[var(--text-muted)]";
      case "active": return "bg-[var(--accent)]";
      case "done": return "bg-[var(--success)]";
      case "cancelled": return "bg-[var(--text-faint)]";
      default: return "bg-[var(--text-faint)]";
    }
  }

  function statusBorderColor(status: string): string {
    switch (status) {
      case "backlog": return "border-[var(--text-faint)]";
      case "todo": return "border-[var(--text-muted)]";
      case "active": return "border-[var(--accent)]";
      case "done": return "border-[var(--success)]";
      case "cancelled": return "border-[var(--text-faint)]";
      default: return "border-[var(--text-faint)]";
    }
  }

  function priorityColor(priority: string): string {
    switch (priority) {
      case "urgent": return "text-[var(--error)]";
      case "high": return "text-orange-500";
      case "medium": return "text-[var(--accent)]";
      case "low": return "text-[var(--text-muted)]";
      default: return "text-[var(--text-faint)]";
    }
  }
</script>
