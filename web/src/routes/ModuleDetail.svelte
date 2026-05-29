<script lang="ts">
  // LIF-121 — Module detail view.
  //
  // Surfaces a single module as a focused workspace: editable name +
  // markdown description (via the shared EditableMarkdown), a status
  // dropdown driving the lifecycle column on the list view, and a list
  // of every issue assigned to this module. From here you can create
  // new issues that land pre-assigned to the module, click into any
  // existing one, or delete the module entirely.
  //
  // Reads as a sibling of PageDetail / IssueDetail. Topbar lives in
  // the chrome zone via the same context slot, sidebar carries the
  // metadata (status, dates, dangerous actions).

  import {
    getModule,
    updateModule,
    deleteModule,
    listIssues,
    type Module,
    type Issue,
  } from "../lib/api";
  import EditableMarkdown from "../lib/EditableMarkdown.svelte";
  import ModeToggle from "../lib/ModeToggle.svelte";
  import DeleteMenu from "../lib/DeleteMenu.svelte";
  import { formatDate } from "../lib/format";
  import {
    ArrowLeft, Plus, ChevronDown,
    CircleDot, Pause, CircleCheck, CircleX, CircleDashed, Circle,
    CircleAlert,
  } from "lucide-svelte";
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
    moduleId,
    editable = true,
  }: {
    navigate: (path: string) => void;
    projectIdentifier: string;
    moduleId: number;
    editable?: boolean;
  } = $props();

  let mod = $state<Module | null>(null);
  let issues = $state<Issue[]>([]);
  let loading = $state(true);
  let error = $state("");

  // Editing
  let editingName = $state(false);
  let draftName = $state("");
  let descriptionMode = $state<"read" | "edit">("read");
  let descriptionRef = $state<EditableMarkdown | null>(null);

  // Save indicator
  let saving = $state(false);
  let lastSaved = $state<string | null>(null);

  // Status dropdown
  let statusOpen = $state(false);

  const STATUSES: { value: string; label: string }[] = [
    { value: "active", label: "Active" },
    { value: "planned", label: "Planned" },
    { value: "paused", label: "Paused" },
    { value: "backlog", label: "Backlog" },
    { value: "done", label: "Done" },
    { value: "cancelled", label: "Cancelled" },
  ];

  // Status vocabulary for the issues list inside the module —
  // mirrors IssueDetail's STATUSES list so the icons/colors stay
  // consistent across surfaces.
  const ISSUE_STATUS_ORDER = ["backlog", "todo", "active", "done", "cancelled"];

  $effect(() => {
    const id = moduleId;
    // Reset volatile state when navigating between modules.
    editingName = false;
    descriptionMode = "read";
    statusOpen = false;
    lastSaved = null;
    loadModule(id);
  });

  async function loadModule(id: number) {
    loading = true;
    error = "";
    issues = [];

    const res = await getModule(id);
    if (!res.ok) { error = res.error; loading = false; return; }
    mod = res.data;

    // Pull the issues for this module. Using the existing module_id
    // filter on listIssues keeps the load efficient even for very
    // populous modules — server-side filter, no client trim.
    const issuesRes = await listIssues({ project_id: mod.project_id, module_id: mod.id, limit: 500 });
    if (issuesRes.ok) issues = issuesRes.data;

    loading = false;
  }

  function handleWindowClick() {
    statusOpen = false;
  }

  // ── Save helpers ─────────────────────────────────────

  async function saveField(field: string, value: unknown) {
    if (!mod) return;
    saving = true;
    const res = await updateModule(mod.id, { [field]: value });
    if (res.ok) {
      mod = res.data;
      lastSaved = new Date().toLocaleTimeString([], {
        hour: "2-digit",
        minute: "2-digit",
      });
    }
    saving = false;
  }

  // ── Name editing ─────────────────────────────────────

  function startEditName() {
    if (!editable || !mod) return;
    draftName = mod.name;
    editingName = true;
  }

  async function commitName() {
    if (!mod) return;
    editingName = false;
    const trimmed = draftName.trim();
    if (trimmed && trimmed !== mod.name) {
      await saveField("name", trimmed);
    }
  }

  // ── Description (delegated to EditableMarkdown) ──────

  async function saveDescription(next: string) {
    if (!mod) return;
    if (next !== mod.description) {
      await saveField("description", next);
    }
  }

  // ── Status ───────────────────────────────────────────

  async function setStatus(value: string) {
    statusOpen = false;
    if (mod && value !== mod.status) await saveField("status", value);
  }

  // ── Delete ───────────────────────────────────────────

  async function handleDelete(): Promise<boolean> {
    if (!mod) return false;
    const res = await deleteModule(mod.id);
    if (res.ok) {
      navigate(`/${projectIdentifier}/modules`);
      return true;
    }
    return false;
  }

  // ── Keyboard shortcuts ───────────────────────────────

  function handleKeydown(e: KeyboardEvent) {
    if (e.key !== "e" && e.key !== "E") return;
    if (!editable || !mod) return;
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    const el = document.activeElement;
    if (el) {
      const tag = el.tagName;
      if (
        tag === "INPUT" ||
        tag === "TEXTAREA" ||
        tag === "SELECT" ||
        (el as HTMLElement).isContentEditable
      ) {
        return;
      }
    }
    if (descriptionMode === "edit") return;
    e.preventDefault();
    descriptionRef?.focus();
  }

  function newIssueInModule() {
    if (!mod) return;
    navigate(`/${projectIdentifier}/issues/new?module=${mod.id}`);
  }

  function statusLabel(value: string): string {
    return STATUSES.find((s) => s.value === value)?.label ?? value;
  }

  // Issue rollup — by status, in lifecycle order. Drives the small
  // "{n} backlog · {n} active · {n} done" header above the issues
  // list, which is the at-a-glance health summary for the module.
  let issueStatusCounts = $derived.by(() => {
    const counts: Record<string, number> = {};
    for (const s of ISSUE_STATUS_ORDER) counts[s] = 0;
    for (const i of issues) {
      counts[i.status] = (counts[i.status] ?? 0) + 1;
    }
    return counts;
  });
</script>

<svelte:window onclick={handleWindowClick} onkeydown={handleKeydown} />

{#snippet topbarContent()}
  {#if mod}
    <div class="flex items-center gap-3 px-6 py-2 w-full">
      <div class="flex items-center gap-1.5 shrink-0">
        <button
          class="flex items-center gap-1.5 text-[0.8125rem] text-[var(--text-muted)]
                 hover:text-[var(--text)] transition-colors rounded px-1.5 py-0.5
                 hover:bg-[var(--bg-subtle)]"
          onclick={() => navigate(`/${projectIdentifier}/modules`)}
        >
          <ArrowLeft size={14} />
          Modules
        </button>
        <span class="text-[var(--text-faint)]">/</span>
        <span class="text-[0.8125rem] font-medium text-[var(--text)] truncate max-w-[280px]">
          {mod.name}
        </span>
      </div>

      <div class="ml-auto flex items-center gap-2 shrink-0">
        {#if editable && mod.description.trim()}
          <ModeToggle
            mode={descriptionMode}
            size="sm"
            disabled={saving}
            onSelect={(next) => descriptionRef?.setMode(next)}
          />
        {/if}

        <span class="text-[0.75rem] text-[var(--text-faint)] min-w-[5rem] text-right">
          {#if saving}
            <span class="animate-pulse">Saving...</span>
          {:else if lastSaved}
            Saved at {lastSaved}
          {/if}
        </span>

        {#if editable}
          <button
            class="inline-flex items-center gap-1 text-[0.8125rem] font-medium
                   text-[var(--accent-text)] bg-[var(--accent)] px-2.5 py-1
                   rounded-md hover:bg-[var(--accent-hover)] transition-colors"
            onclick={newIssueInModule}
          >
            <Plus size={13} />
            Issue
          </button>

          <DeleteMenu
            noun="module"
            label={mod.name}
            confirmBody={issues.length === 0
              ? "This module is empty. It will be removed."
              : `${issues.length} issue${issues.length === 1 ? "" : "s"} will be unassigned from this module but not deleted.`}
            onDelete={handleDelete}
            align="right"
          />
        {/if}
      </div>
    </div>
  {/if}
{/snippet}

{#if loading}
  <div class="h-full flex items-center justify-center">
    <div
      class="size-6 rounded-full border-2 border-[var(--border)]
             border-t-[var(--accent)] animate-spin"
    ></div>
  </div>
{:else if error}
  <div class="h-full flex flex-col items-center justify-center gap-3">
    <p class="text-[var(--error)] text-[0.875rem]">{error}</p>
    <button
      class="text-[0.8125rem] text-[var(--accent)] hover:underline"
      onclick={() => navigate(`/${projectIdentifier}/modules`)}
    >
      Back to modules
    </button>
  </div>
{:else if mod}
  <div class="h-full flex flex-col">
    <div class="flex-1 overflow-y-auto">
      <div class="max-w-[1120px] mx-auto flex gap-0 min-h-full">
        <!-- Main column -->
        <div class="flex-1 min-w-0 px-8 py-6">
          <!-- Name -->
          {#if editingName}
            <!-- svelte-ignore a11y_autofocus -->
            <input
              type="text"
              bind:value={draftName}
              class="w-full text-[1.75rem] font-display tracking-tight
                     bg-transparent border-none outline-none
                     text-[var(--text)] py-1 mb-3"
              onblur={commitName}
              onkeydown={(e) => {
                if (e.key === "Enter") commitName();
                if (e.key === "Escape") { editingName = false; }
              }}
              autofocus
            />
          {:else if editable}
            <button
              type="button"
              class="text-[1.75rem] font-display tracking-tight text-[var(--text)]
                     py-1 mb-3 rounded transition-colors w-full text-left
                     bg-transparent border-0 p-0 cursor-text hover:bg-[var(--bg-subtle)]"
              onclick={startEditName}
              onkeydown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  startEditName();
                }
              }}
            >
              {mod.name}
            </button>
          {:else}
            <h1 class="text-[1.75rem] font-display tracking-tight text-[var(--text)] py-1 mb-3">
              {mod.name}
            </h1>
          {/if}

          <!-- Description -->
          <section class="mb-10">
            <EditableMarkdown
              bind:this={descriptionRef}
              bind:mode={descriptionMode}
              value={mod.description}
              {editable}
              {saving}
              placeholder="Describe this module... (markdown supported)"
              emptyEditCta="Click to describe this module..."
              emptyReadText="No description"
              proseMinHeight="60px"
              onSave={saveDescription}
            />
          </section>

          <!-- Issues section -->
          <section>
            <!-- Section header: count rollup + new-issue affordance. The
                 rollup is the at-a-glance health for the module —
                 "what's queued, what's in progress, what's done." -->
            <div class="flex items-baseline justify-between mb-3 pb-2 border-b border-[var(--border)]">
              <div class="flex items-baseline gap-2">
                <h2 class="text-[0.6875rem] font-semibold uppercase tracking-widest text-[var(--text-muted)]">
                  Issues
                </h2>
                <span class="text-[0.6875rem] text-[var(--text-faint)] tabular-nums">
                  {issues.length}
                </span>
              </div>
              <div class="flex items-center gap-3 text-[0.6875rem] text-[var(--text-faint)]">
                {#each ISSUE_STATUS_ORDER as s}
                  {#if issueStatusCounts[s] > 0}
                    <span class="flex items-center gap-1">
                      {@render issueStatusIcon(s, 11)}
                      <span class="tabular-nums">{issueStatusCounts[s]}</span>
                    </span>
                  {/if}
                {/each}
              </div>
            </div>

            {#if issues.length === 0}
              <div class="py-10 flex flex-col items-center gap-2">
                <p class="text-[0.875rem] text-[var(--text-faint)] italic">
                  No issues in this module yet
                </p>
                {#if editable}
                  <button
                    class="text-[0.8125rem] text-[var(--accent)] hover:underline"
                    onclick={newIssueInModule}
                  >
                    Add the first issue
                  </button>
                {/if}
              </div>
            {:else}
              <div class="flex flex-col -mx-2">
                {#each [...issues].sort((a, b) => {
                  // Group by lifecycle status (backlog → todo → active → done → cancelled),
                  // then by created_at desc within each group so recent
                  // arrivals float to the top of their status bucket.
                  const sa = ISSUE_STATUS_ORDER.indexOf(a.status);
                  const sb = ISSUE_STATUS_ORDER.indexOf(b.status);
                  if (sa !== sb) return sa - sb;
                  return b.created_at.localeCompare(a.created_at);
                }) as issue (issue.id)}
                  <button
                    class="flex items-center gap-3 px-2 py-2 -mx-0 rounded-md
                           text-left hover:bg-[var(--bg-subtle)]
                           transition-colors group"
                    onclick={() => navigate(`/${projectIdentifier}/issues/${issue.identifier}`)}
                  >
                    {@render issueStatusIcon(issue.status, 14)}
                    <span class="text-[0.75rem] font-mono text-[var(--text-faint)] shrink-0 tabular-nums w-[60px]">
                      {issue.identifier}
                    </span>
                    <span class="text-[0.875rem] text-[var(--text)] truncate flex-1">
                      {issue.title}
                    </span>
                    {#if issue.priority && issue.priority !== "none"}
                      {@render priorityIcon(issue.priority)}
                    {/if}
                  </button>
                {/each}
              </div>
            {/if}
          </section>
        </div>

        <!-- Sidebar -->
        <aside class="w-[220px] shrink-0 border-l border-[var(--border)] py-6 px-5">
          <!-- Status -->
          <div class="mb-5">
            <p class="text-[0.6875rem] font-semibold uppercase tracking-widest text-[var(--text-faint)] mb-2">
              Status
            </p>
            <div class="relative">
              <button
                class="flex items-center gap-2 text-[0.8125rem] rounded-md
                       px-2 py-1 -mx-2 transition-colors w-full text-left
                       {editable ? 'hover:bg-[var(--bg-subtle)] cursor-pointer' : 'cursor-default'}"
                onclick={(e) => {
                  if (!editable) return;
                  e.stopPropagation();
                  statusOpen = !statusOpen;
                }}
              >
                {@render moduleStatusIcon(mod.status, 14)}
                <span class="text-[var(--text)] flex-1">{statusLabel(mod.status)}</span>
                {#if editable}
                  <ChevronDown size={12} class="text-[var(--text-faint)]" />
                {/if}
              </button>
              {#if statusOpen}
                <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
                <div
                  class="absolute left-0 top-full mt-1 z-30 w-[180px]
                         bg-[var(--surface)] border border-[var(--border)]
                         rounded-md shadow-lg py-1"
                  onclick={(e) => e.stopPropagation()}
                >
                  {#each STATUSES as s}
                    <button
                      class="w-full flex items-center gap-2 px-3 py-1.5 text-left
                             text-[0.8125rem] transition-colors
                             {s.value === mod.status
                        ? 'text-[var(--accent)] bg-[var(--accent-subtle)]'
                        : 'text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
                      onclick={() => setStatus(s.value)}
                    >
                      {@render moduleStatusIcon(s.value, 14)}
                      {s.label}
                    </button>
                  {/each}
                </div>
              {/if}
            </div>
          </div>

          <div class="border-t border-[var(--border)] -mx-5 my-4"></div>

          <!-- Dates -->
          <div class="flex flex-col gap-4">
            <div>
              <p class="text-[0.6875rem] font-semibold uppercase tracking-widest text-[var(--text-faint)] mb-0.5">
                Created
              </p>
              <p class="text-[0.8125rem] text-[var(--text-muted)] leading-snug m-0">
                {formatDate(mod.created_at)}
              </p>
            </div>
            <div>
              <p class="text-[0.6875rem] font-semibold uppercase tracking-widest text-[var(--text-faint)] mb-0.5">
                Updated
              </p>
              <p class="text-[0.8125rem] text-[var(--text-muted)] leading-snug m-0">
                {formatDate(mod.updated_at)}
              </p>
            </div>
          </div>
        </aside>
      </div>
    </div>
  </div>
{/if}

{#snippet moduleStatusIcon(status: string, size: number)}
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

{#snippet issueStatusIcon(status: string, size: number)}
  {#if status === "done"}
    <CircleCheck {size} class="text-[var(--success)] shrink-0" />
  {:else if status === "cancelled"}
    <CircleX {size} class="text-[var(--text-faint)] shrink-0" />
  {:else if status === "active"}
    <CircleDot {size} class="text-[var(--accent)] shrink-0" />
  {:else if status === "backlog"}
    <CircleDashed {size} class="text-[var(--text-faint)] shrink-0" />
  {:else}
    <Circle {size} class="text-[var(--text-muted)] shrink-0" />
  {/if}
{/snippet}

{#snippet priorityIcon(priority: string)}
  {#if priority === "urgent"}
    <CircleAlert size={13} class="text-[var(--error)] shrink-0" />
  {:else if priority === "high"}
    <svg class="size-3 text-orange-500 shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <line x1="5" y1="12" x2="19" y2="12"/><line x1="5" y1="6" x2="19" y2="6"/><line x1="5" y1="18" x2="19" y2="18"/>
    </svg>
  {:else if priority === "medium"}
    <svg class="size-3 text-[var(--accent)] shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <line x1="5" y1="9" x2="19" y2="9"/><line x1="5" y1="15" x2="19" y2="15"/>
    </svg>
  {:else if priority === "low"}
    <svg class="size-3 text-[var(--text-muted)] shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <line x1="5" y1="12" x2="19" y2="12"/>
    </svg>
  {/if}
{/snippet}
