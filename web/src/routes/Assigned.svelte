<script lang="ts">
  // LIF-540: every open issue that names the signed-in person, across the
  // projects they can see, grouped by status. One request per open status
  // with `assignee=me`, so done and cancelled work never uses up the row cap.
  import { getContext } from "svelte";
  import { UserCheck } from "lucide-svelte";
  import { listIssues, listProjects, type Issue, type Project } from "../lib/api";
  import ErrorState from "../lib/ErrorState.svelte";
  import Mascot from "../lib/Mascot.svelte";
  import StatusIcon from "../lib/StatusIcon.svelte";
  import { startAutoRefresh } from "../lib/autoRefresh.svelte";
  import IssueGroup from "../lib/workspace/IssueGroup.svelte";
  import WorkspaceSkeleton from "../lib/workspace/WorkspaceSkeleton.svelte";
  import { OPEN_STATUS_ORDER, groupByOpenStatus, uniqueIssueCount } from "../lib/workspace/issues";

  let { navigate }: { navigate: (path: string) => void } = $props();

  const topbarCtx = getContext<{
    set: (s: import("svelte").Snippet | undefined) => void;
  } | undefined>("lific:topbar");

  $effect(() => {
    topbarCtx?.set(topbarContent);
    return () => topbarCtx?.set(undefined);
  });

  /** The list endpoint's maximum page size. */
  const ROW_LIMIT = 500;

  let issues = $state<Issue[]>([]);
  /** Statuses whose request came back full, so more may exist. */
  let truncated = $state<Set<string>>(new Set());
  let projects = $state<Project[]>([]);
  let loading = $state(true);
  let error = $state("");
  let loaded = false;
  let collapsed = $state<Set<string>>(new Set());

  $effect(() => {
    load();
  });

  $effect(() =>
    startAutoRefresh({
      refresh: () => load(false),
      isBusy: () => loading,
      realtimeDebounceMs: 750,
      realtimeMaxWaitMs: 5_000,
      shouldRefresh: (event) =>
        event.type === "resync.required" ||
        event.type.startsWith("project.") ||
        event.type.startsWith("issue."),
    }),
  );

  async function load(initial = true) {
    if (initial) {
      loading = true;
      error = "";
    }
    const [projectsRes, ...results] = await Promise.all([
      listProjects(),
      ...OPEN_STATUS_ORDER.map((status) =>
        listIssues({ assignee: "me", status, order_by: "updated", order: "desc", limit: ROW_LIMIT }),
      ),
    ]);
    if (projectsRes.ok) projects = projectsRes.data;
    const failed = results.find((r) => !r.ok);
    if (failed && !failed.ok) {
      if (initial || !loaded) error = failed.error;
    } else {
      issues = results.flatMap((r) => (r.ok ? r.data : []));
      truncated = new Set(
        OPEN_STATUS_ORDER.filter((_, i) => {
          const r = results[i];
          return r.ok && r.data.length >= ROW_LIMIT;
        }),
      );
      loaded = true;
      error = "";
    }
    if (initial) loading = false;
  }

  let groups = $derived(groupByOpenStatus(issues));
  let total = $derived(uniqueIssueCount(groups));
  let projectNames = $derived(new Map(projects.map((p) => [p.id, p.name])));

  function toggle(key: string) {
    const next = new Set(collapsed);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    collapsed = next;
  }
</script>

{#snippet capNote()}
  Showing the {ROW_LIMIT} most recently updated.
{/snippet}

{#snippet topbarContent()}
  <div class="flex items-center gap-2 px-6 py-2 w-full">
    <UserCheck size={14} class="shrink-0 text-[var(--text-faint)]" />
    <span class="text-body-sm font-medium text-[var(--text)]">Assigned to me</span>
  </div>
{/snippet}

<div class="h-full flex flex-col">
  <div class="flex-1 overflow-y-auto">
    {#if loading}
      <div class="max-w-[960px] mx-auto px-6 md:px-8 py-8 md:py-10">
        <WorkspaceSkeleton groups={[2, 4, 3]} />
      </div>
    {:else if error}
      <ErrorState title="Couldn't load your assigned issues" message={error}>
        <button
          class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)] px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors"
          onclick={() => load()}
        >
          Try again
        </button>
      </ErrorState>
    {:else}
      <div class="max-w-[960px] mx-auto px-6 md:px-8 py-8 md:py-10">
        <header class="mb-6">
          <h1 class="font-display text-title tracking-tight text-[var(--text)] leading-none flex items-baseline gap-2.5">
            Assigned to me
            {#if total > 0}
              <span class="text-body-sm font-sans tracking-normal text-[var(--text-faint)] tabular-nums" data-testid="assigned-total">
                {total}
              </span>
            {/if}
          </h1>
          <p class="text-body-sm text-[var(--text-muted)] mt-1.5">
            Open issues that name you, across every project you can see.
          </p>
        </header>

        {#if groups.length === 0}
          <div class="flex flex-col items-center justify-center py-16 gap-3">
            <Mascot src="/LizzySleep2.png" nativeW={1000} nativeH={420} scale={0.16} />
            <p class="text-body-sm text-[var(--text-muted)]">No open issues name you.</p>
          </div>
        {:else}
          <div class="flex flex-col gap-5">
            {#each groups as group (group.status)}
              {#snippet icon()}
                <StatusIcon status={group.status} size={13} />
              {/snippet}
              <IssueGroup
                key={group.status}
                label={group.label}
                issues={group.items}
                {icon}
                collapsed={collapsed.has(group.status)}
                onToggle={() => toggle(group.status)}
                projectName={(issue) => projectNames.get(issue.project_id)}
                {navigate}
                footer={truncated.has(group.status) ? capNote : undefined}
              />
            {/each}
          </div>
        {/if}
      </div>
    {/if}
  </div>
</div>
