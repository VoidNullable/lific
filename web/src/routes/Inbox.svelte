<script lang="ts">
  // LIF-539: Inbox, everything that needs the signed-in person across the
  // projects they can see. The page is a list of sections; today every
  // section is one of the LIF-506 attention groups, and notifications
  // (LIF-528) can join as another kind of section.
  import { getContext } from "svelte";
  import { Inbox as InboxIcon, UserCheck, User, Hourglass } from "lucide-svelte";
  import { getAttention, listProjects, type Attention, type Issue, type Project } from "../lib/api";
  import ErrorState from "../lib/ErrorState.svelte";
  import Mascot from "../lib/Mascot.svelte";
  import { startAutoRefresh } from "../lib/autoRefresh.svelte";
  import IssueGroup from "../lib/workspace/IssueGroup.svelte";
  import WorkspaceSkeleton from "../lib/workspace/WorkspaceSkeleton.svelte";
  import { newSince, uniqueIssueCount } from "../lib/workspace/issues";
  import { INBOX_VISIT_KEY, recordVisit } from "../lib/workspace/lastVisit";

  let { navigate }: { navigate: (path: string) => void } = $props();

  const topbarCtx = getContext<{
    set: (s: import("svelte").Snippet | undefined) => void;
  } | undefined>("lific:topbar");

  $effect(() => {
    topbarCtx?.set(topbarContent);
    return () => topbarCtx?.set(undefined);
  });

  // Measured once per mount, so refreshes keep comparing against the same
  // previous visit.
  const visitSince = recordVisit(INBOX_VISIT_KEY);

  let attention = $state<Attention | null>(null);
  let projects = $state<Project[]>([]);
  let loading = $state(true);
  let error = $state("");
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
    const [attentionRes, projectsRes] = await Promise.all([getAttention(), listProjects()]);
    if (projectsRes.ok) projects = projectsRes.data;
    if (attentionRes.ok) {
      attention = attentionRes.data;
      error = "";
    } else if (initial || !attention) {
      error = attentionRes.error;
    }
    if (initial) loading = false;
  }

  /** The attention endpoint returns at most this many issues per group. */
  const ATTENTION_LIMIT = 50;

  type Section = {
    key: string;
    label: string;
    icon: typeof UserCheck;
    items: Issue[];
  };

  let sections = $derived.by((): Section[] => {
    if (!attention) return [];
    return [
      { key: "assigned", label: "Assigned to you", icon: UserCheck, items: attention.assigned },
      { key: "human", label: "Marked for a person", icon: User, items: attention.human },
      { key: "waiting", label: "Waiting on you", icon: Hourglass, items: attention.waiting },
    ].filter((s) => s.items.length > 0);
  });

  let total = $derived(uniqueIssueCount(sections));
  let fresh = $derived(newSince(sections.flatMap((s) => s.items), visitSince));

  let projectNames = $derived(new Map(projects.map((p) => [p.id, p.name])));

  function toggle(key: string) {
    const next = new Set(collapsed);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    collapsed = next;
  }
</script>

{#snippet capNote()}
  Showing the {ATTENTION_LIMIT} most recently updated.
{/snippet}

{#snippet topbarContent()}
  <div class="flex items-center gap-2 px-6 py-2 w-full">
    <InboxIcon size={14} class="shrink-0 text-[var(--text-faint)]" />
    <span class="text-body-sm font-medium text-[var(--text)]">Inbox</span>
  </div>
{/snippet}

<div class="h-full flex flex-col">
  <div class="flex-1 overflow-y-auto">
    {#if loading}
      <div class="max-w-[960px] mx-auto px-6 md:px-8 py-8 md:py-10">
        <WorkspaceSkeleton groups={[3, 2, 2]} />
      </div>
    {:else if error}
      <ErrorState title="Couldn't load your inbox" message={error}>
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
            Inbox
            {#if total > 0}
              <span class="text-body-sm font-sans tracking-normal text-[var(--text-faint)] tabular-nums" data-testid="inbox-total">
                {total}
              </span>
            {/if}
          </h1>
          <p class="text-body-sm text-[var(--text-muted)] mt-1.5">
            {#if fresh.size > 0}
              {fresh.size === 1 ? "1 issue changed" : `${fresh.size} issues changed`} since your last visit.
            {:else}
              Issues assigned to you, marked for a person, or waiting on you.
            {/if}
          </p>
        </header>

        {#if sections.length === 0}
          <div class="flex flex-col items-center justify-center py-16 gap-3">
            <Mascot src="/LizzySleep2.png" nativeW={1000} nativeH={420} scale={0.16} />
            <p class="text-body-sm text-[var(--text-muted)]">Nothing needs you right now.</p>
          </div>
        {:else}
          <div class="flex flex-col gap-5">
            {#each sections as section (section.key)}
              {#snippet icon()}
                <section.icon size={13} />
              {/snippet}
              <IssueGroup
                key={section.key}
                label={section.label}
                issues={section.items}
                {icon}
                collapsed={collapsed.has(section.key)}
                onToggle={() => toggle(section.key)}
                projectName={(issue) => projectNames.get(issue.project_id)}
                isNew={(issue) => fresh.has(issue.id)}
                {navigate}
                footer={section.items.length >= ATTENTION_LIMIT ? capNote : undefined}
              />
            {/each}
          </div>
        {/if}
      </div>
    {/if}
  </div>
</div>
