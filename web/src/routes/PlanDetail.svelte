<script lang="ts">
  // LIF-177 — Plan detail, brought up to Issue/Page parity. Uses the shared
  // DocumentDetail shell (editable title, metadata sidebar, Activity timeline,
  // delete kebab, chrome) with the step tree injected as the custom body.
  // Each step now has an expandable markdown description editor.

  import {
    getPlan,
    updatePlan,
    deletePlan,
    addPlanStep,
    updatePlanStep,
    deletePlanStep,
    listPlanActivity,
    type Plan,
    type PlanStep,
    type Activity,
    type Issue,
  } from "../lib/api";
  import DocumentDetail from "../lib/DocumentDetail.svelte";
  import IssuePickerModal from "../lib/IssuePickerModal.svelte";
  import Markdown from "../lib/Markdown.svelte";
  import { startAutoRefresh } from "../lib/autoRefresh.svelte";
  import { formatDate } from "../lib/format";
  import { recordRecent } from "../lib/home/recents"; // LIF-237
  import { openPeek } from "../lib/issues/peek.svelte"; // LIF-248
  import { projectRole, loadProjectRole } from "../lib/projectRole.svelte"; // LIF-234
  import {
    Check,
    Plus,
    X,
    Trash2,
    ChevronRight,
    ChevronDown,
    ArrowUpRight,
  } from "lucide-svelte";

  let {
    navigate,
    projectIdentifier,
    planId,
  }: {
    navigate: (path: string) => void;
    projectIdentifier: string;
    planId: number;
  } = $props();

  let plan = $state<Plan | null>(null);

  // LIF-234: plans are content — step edits, title, and delete are
  // maintainer-gated. A viewer sees the plan read-only.
  const canEdit = $derived(projectRole.canEdit);

  let activity = $state<Activity[]>([]);
  let loading = $state(true);
  let error = $state("");
  let notice = $state("");
  let saving = $state(false);
  let lastSaved = $state<string | null>(null);

  // Inline UI state.
  let mutating = $state(false);
  let addingChildOf = $state<number | null>(null); // step id, -1 = root
  let childTitle = $state("");
  let editingTitleOf = $state<number | null>(null);
  let titleDraft = $state("");
  let editingDescOf = $state<number | null>(null);
  let descDraft = $state("");
  let collapsed = $state<Set<number>>(new Set());
  let statusOpen = $state(false);

  // Issue-picker modal (LIF-202): replaces the old window.prompt flow for
  // both anchoring a plan and linking a step to an issue.
  let pickerOpen = $state(false);
  // What the next selection should do — "anchor" mutates the plan, while a
  // step id links that step.
  let pickerTarget = $state<{ kind: "anchor" } | { kind: "step"; stepId: number } | null>(null);
  let pickerTitle = $state("Link an issue");
  let pickerCurrent = $state<string | null>(null);
  let pickerAllowClear = $state(false);

  const STATUSES = ["active", "done", "archived"];

  $effect(() => {
    const id = planId;
    load(id);
  });

  $effect(() =>
    startAutoRefresh({
      refresh: reload,
      isBusy: () =>
        mutating ||
        statusOpen ||
        pickerOpen ||
        addingChildOf !== null ||
        editingTitleOf !== null ||
        editingDescOf !== null,
      shouldRefresh: (event) =>
        event.type === "resync.required" ||
        (event.type.startsWith("project.") &&
          typeof event.project_id === "number" &&
          event.project_id === plan?.project_id),
    }),
  );

  async function load(id: number) {
    loading = true;
    error = "";
    const res = await getPlan(id);
    if (!res.ok) { error = res.error; loading = false; return; }
    plan = res.data;
    loadProjectRole(plan.project_id); // LIF-234
    recordRecent({ type: "plan", routeId: String(plan.id), identifier: plan.identifier, title: plan.title, project: projectIdentifier }); // LIF-237
    const act = await listPlanActivity(id);
    if (act.ok) activity = act.data.items;
    loading = false;
  }

  async function reload() {
    if (!plan) return;
    const [p, a] = await Promise.all([getPlan(plan.id), listPlanActivity(plan.id)]);
    if (p.ok) plan = p.data;
    if (a.ok) activity = a.data.items;
  }

  async function refreshActivity() {
    if (!plan) return;
    const a = await listPlanActivity(plan.id);
    if (a.ok) activity = a.data.items;
  }

  function flash(msg: string) {
    notice = msg;
    if (msg) setTimeout(() => (notice = ""), 4000);
  }

  function markSaved() {
    lastSaved = new Date().toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  }

  // ── Plan-level ──

  async function saveTitle(next: string) {
    if (!plan || next === plan.title) return;
    saving = true;
    const res = await updatePlan(plan.id, { title: next });
    saving = false;
    if (res.ok) { plan = res.data; markSaved(); refreshActivity(); }
  }

  async function setStatus(status: string) {
    statusOpen = false;
    if (!plan || status === plan.status) return;
    mutating = true;
    const res = await updatePlan(plan.id, { status });
    mutating = false;
    if (res.ok) { plan = res.data; refreshActivity(); }
  }

  function setAnchor() {
    if (!plan) return;
    pickerTarget = { kind: "anchor" };
    pickerTitle = "Set anchor issue";
    pickerCurrent = plan.anchor_identifier ?? null;
    pickerAllowClear = true;
    pickerOpen = true;
  }

  async function applyAnchor(issueId: number | null) {
    if (!plan) return;
    mutating = true;
    const res = await updatePlan(plan.id, { issue_id: issueId });
    mutating = false;
    if (res.ok) plan = res.data;
    else error = res.error;
    refreshActivity();
  }

  async function removePlan(): Promise<boolean> {
    if (!plan) return false;
    const res = await deletePlan(plan.id);
    if (res.ok) { navigate(`/${projectIdentifier}/plans`); return true; }
    error = res.error;
    return false;
  }

  // ── Step-level ──

  async function toggleDone(step: PlanStep) {
    if (!plan) return;
    mutating = true;
    const res = await updatePlanStep(plan.id, step.id, { done: !step.done });
    mutating = false;
    if (res.ok) {
      plan = res.data.plan;
      const eff = res.data.effect;
      if (eff?.issue_status_changed && eff.issue_identifier) {
        flash(`${eff.issue_identifier} marked done`);
      }
      refreshActivity();
    } else error = res.error;
  }

  function startAddChild(stepId: number) {
    addingChildOf = stepId;
    childTitle = "";
  }
  async function commitAddChild() {
    if (!plan || addingChildOf === null || !childTitle.trim()) { addingChildOf = null; return; }
    mutating = true;
    const parent = addingChildOf === -1 ? undefined : addingChildOf;
    const res = await addPlanStep(plan.id, { parent_step_id: parent, title: childTitle.trim() });
    mutating = false;
    addingChildOf = null;
    if (res.ok) { plan = res.data; refreshActivity(); }
    else error = res.error;
  }

  function startEditTitle(step: PlanStep) {
    editingTitleOf = step.id;
    titleDraft = step.title;
  }
  async function commitEditTitle() {
    if (!plan || editingTitleOf === null || !titleDraft.trim()) { editingTitleOf = null; return; }
    mutating = true;
    const res = await updatePlanStep(plan.id, editingTitleOf, { title: titleDraft.trim() });
    mutating = false;
    editingTitleOf = null;
    if (res.ok) { plan = res.data.plan; refreshActivity(); }
    else error = res.error;
  }

  function startEditDesc(step: PlanStep) {
    editingDescOf = step.id;
    descDraft = step.description;
    expand(step.id);
  }
  async function commitEditDesc() {
    if (!plan || editingDescOf === null) { editingDescOf = null; return; }
    const id = editingDescOf;
    mutating = true;
    const res = await updatePlanStep(plan.id, id, { description: descDraft });
    mutating = false;
    editingDescOf = null;
    if (res.ok) { plan = res.data.plan; refreshActivity(); }
    else error = res.error;
  }
  function cancelEditDesc() {
    editingDescOf = null;
  }

  async function removeStep(step: PlanStep) {
    if (!plan) return;
    mutating = true;
    const res = await deletePlanStep(plan.id, step.id);
    mutating = false;
    if (res.ok) { plan = res.data; refreshActivity(); }
    else error = res.error;
  }

  async function detachIssue(step: PlanStep) {
    if (!plan) return;
    mutating = true;
    const res = await updatePlanStep(plan.id, step.id, { issue_id: null });
    mutating = false;
    if (res.ok) { plan = res.data.plan; refreshActivity(); }
  }
  function attachIssue(step: PlanStep) {
    if (!plan) return;
    pickerTarget = { kind: "step", stepId: step.id };
    pickerTitle = "Link an issue to this step";
    pickerCurrent = step.issue_identifier ?? null;
    pickerAllowClear = false;
    pickerOpen = true;
  }

  async function linkStepIssue(stepId: number, issueId: number) {
    if (!plan) return;
    mutating = true;
    const res = await updatePlanStep(plan.id, stepId, { issue_id: issueId });
    mutating = false;
    if (res.ok) { plan = res.data.plan; refreshActivity(); }
    else error = res.error;
  }

  // Dispatch a picker selection to whichever target opened it.
  function onPickerSelect(issue: Issue) {
    const t = pickerTarget;
    pickerTarget = null;
    if (!t) return;
    if (t.kind === "anchor") void applyAnchor(issue.id);
    else void linkStepIssue(t.stepId, issue.id);
  }

  function onPickerClear() {
    const t = pickerTarget;
    pickerTarget = null;
    if (t?.kind === "anchor") void applyAnchor(null);
  }

  // ── Expand/collapse ──

  function isExpanded(step: PlanStep): boolean {
    // Steps with content/children default open; user can collapse.
    if (collapsed.has(step.id)) return false;
    return true;
  }
  function toggleExpand(id: number) {
    const next = new Set(collapsed);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    collapsed = next;
  }
  function expand(id: number) {
    if (collapsed.has(id)) {
      const next = new Set(collapsed);
      next.delete(id);
      collapsed = next;
    }
  }

  function provenance(step: PlanStep): { text: string; tone: string } | null {
    if (!step.issue_identifier) return null;
    if (step.done && step.issue_status === "done") return { text: `via ${step.issue_identifier}`, tone: "muted" };
    if (!step.done && step.reopened_via_issue_at) return { text: `reopened — ${step.issue_identifier} reopened`, tone: "warn" };
    return { text: `${step.issue_identifier}: ${step.issue_status ?? "?"}`, tone: "link" };
  }

  // LIF-248: shared by the provenance chip and the anchor-issue chip below —
  // shift-click peeks instead of navigating, mirroring IssueDetail's
  // relation chips and Markdown.svelte's identifier links.
  function openIssueChip(e: MouseEvent, identifier: string) {
    if (e.shiftKey) {
      e.preventDefault();
      openPeek(identifier);
      return;
    }
    navigate(`/${projectIdentifier}/issues/${identifier}`);
  }

  let progress = $derived(plan && plan.step_count > 0 ? plan.done_count / plan.step_count : 0);
</script>

<DocumentDetail
  {navigate}
  {loading}
  {error}
  deleteNounLabel="plan"
  onRetry={() => load(planId)}
  identifier={plan?.identifier ?? `PLAN-${planId}`}
  backRoute={`/${projectIdentifier}/plans`}
  backLabel="Plans"
  breadcrumbSegments={[
    { label: projectIdentifier, href: `#/${projectIdentifier}/overview`, mono: true, hideBelowSm: true },
    { label: "Plans", href: `#/${projectIdentifier}/plans` },
    { label: plan?.title || plan?.identifier || `PLAN-${planId}` },
  ]}
  editable={canEdit}
  title={plan?.title ?? ""}
  titleSize="md"
  onSaveTitle={saveTitle}
  body=""
  onSaveBody={() => {}}
  {saving}
  {lastSaved}
  deleteNoun="plan"
  deleteLabel={plan?.identifier ?? ""}
  onDelete={removePlan}
  {activity}
  {bodyContent}
  {sidebar}
  layout="two-column"
/>

{#if plan}
  <IssuePickerModal
    bind:open={pickerOpen}
    projectId={plan.project_id}
    {projectIdentifier}
    title={pickerTitle}
    currentIdentifier={pickerCurrent}
    allowClear={pickerAllowClear}
    onSelect={onPickerSelect}
    onClear={onPickerClear}
  />
{/if}

{#snippet bodyContent()}
  {#if plan}
    {#if notice}
      <div class="mb-3 text-body-sm text-[var(--accent)] bg-[var(--accent-subtle)] rounded-md px-3 py-1.5">
        {notice}
      </div>
    {/if}

    <div class="flex flex-col gap-0.5 mt-2">
      {#each plan.steps as step (step.id)}
        {@render stepNode(step, 0)}
      {/each}
    </div>

    {#if canEdit}
      {#if addingChildOf === -1}
        <div class="flex items-center gap-2 py-1 mt-2 pl-1">
          <input
            class="flex-1 bg-transparent outline-none text-body text-[var(--text)] border-b border-[var(--accent)]"
            placeholder="Step title…"
            bind:value={childTitle}
            autofocus
            onkeydown={(e) => { if (e.key === "Enter") commitAddChild(); if (e.key === "Escape") addingChildOf = null; }}
            onblur={commitAddChild}
          />
        </div>
      {:else}
        <button
          class="mt-3 flex items-center gap-1.5 text-body-sm text-[var(--text-muted)] hover:text-[var(--text)]"
          onclick={() => startAddChild(-1)}
        >
          <Plus size={14} /> Add step
        </button>
      {/if}
    {/if}
  {/if}
{/snippet}

{#snippet stepNode(step: PlanStep, depth: number)}
  {@const prov = provenance(step)}
  {@const expanded = isExpanded(step)}
  {@const hasBody = step.description.trim().length > 0}
  <div class="flex flex-col">
    <div class="group flex items-start gap-2 py-1 rounded-md hover:bg-[var(--bg-subtle)]" style="padding-left: {depth * 1.5}rem">
      <!-- caret -->
      <button
        class="mt-0.5 size-4 shrink-0 flex items-center justify-center text-[var(--text-faint)] hover:text-[var(--text)]"
        onclick={() => toggleExpand(step.id)}
        title={expanded ? "Collapse" : "Expand"}
      >
        {#if expanded}<ChevronDown size={13} />{:else}<ChevronRight size={13} />{/if}
      </button>

      <!-- checkbox (LIF-234: static for viewers — done state still shows) -->
      <button
        class="mt-0.5 size-4 shrink-0 rounded border flex items-center justify-center transition-colors
               {step.done
                 ? 'bg-[var(--accent)] border-[var(--accent)] text-[var(--accent-text)]'
                 : 'border-[var(--border-strong)]'}
               {canEdit && !step.done ? 'hover:border-[var(--accent)]' : ''}
               {canEdit ? '' : 'cursor-default'}"
        onclick={() => { if (canEdit) toggleDone(step); }}
        title={canEdit ? (step.done ? "Mark not done" : "Mark done") : (step.done ? "Done" : "Not done")}
      >
        {#if step.done}<Check size={11} />{/if}
      </button>

      <!-- title + meta -->
      <div class="flex-1 min-w-0">
        <div class="flex items-center gap-2">
          {#if editingTitleOf === step.id}
            <input
              class="flex-1 bg-transparent outline-none text-body text-[var(--text)] border-b border-[var(--accent)]"
              bind:value={titleDraft}
              autofocus
              onkeydown={(e) => { if (e.key === "Enter") commitEditTitle(); if (e.key === "Escape") editingTitleOf = null; }}
              onblur={commitEditTitle}
            />
          {:else}
            <button
              class="text-left text-body truncate {step.done ? 'text-[var(--text-faint)] line-through' : 'text-[var(--text)]'} {canEdit ? '' : 'cursor-default'}"
              ondblclick={() => { if (canEdit) startEditTitle(step); }}
              title={canEdit ? "Double-click to rename" : undefined}
            >
              {step.title}
            </button>
          {/if}

          {#if prov}
            <button
              class="shrink-0 text-micro font-mono px-1.5 py-0.5 rounded inline-flex items-center gap-1
                     {prov.tone === 'warn'
                       ? 'text-[var(--warning)] bg-[var(--warning-bg)]'
                       : prov.tone === 'muted'
                         ? 'text-[var(--text-faint)] bg-[var(--bg-subtle)]'
                         : 'text-[var(--accent)] bg-[var(--accent-subtle)]'}"
              title="Shift-click to preview"
              onclick={(e) => step.issue_identifier && openIssueChip(e, step.issue_identifier)}
            >
              {prov.text}<ArrowUpRight size={10} />
            </button>
          {/if}

          <!-- row actions (LIF-234: hidden for viewers) -->
          {#if canEdit}
          <!-- LIF-275: pointer-coarse keeps step actions reachable on touch. -->
          <div class="ml-auto flex items-center gap-0.5 opacity-0 group-hover:opacity-100 pointer-coarse:opacity-100 transition-opacity shrink-0">
            {#if step.issue_identifier}
              <button class="p-1 rounded text-[var(--text-faint)] hover:text-[var(--text)] text-micro" title="Detach issue" onclick={() => detachIssue(step)}>unlink</button>
            {:else}
              <button class="p-1 rounded text-[var(--text-faint)] hover:text-[var(--text)] text-micro" title="Link an issue" onclick={() => attachIssue(step)}>link</button>
            {/if}
            <button class="p-1 rounded text-[var(--text-faint)] hover:text-[var(--text)]" title="Add sub-step" onclick={() => startAddChild(step.id)}><Plus size={13} /></button>
            <button class="p-1 rounded text-[var(--text-faint)] hover:text-[var(--error)]" title="Delete step" onclick={() => removeStep(step)}><X size={13} /></button>
          </div>
          {/if}
        </div>

        {#if expanded}
          <!-- description body -->
          <div class="mt-1 mb-1">
            {#if !canEdit}
              <!-- LIF-234: viewer — render markdown read-only, no edit CTA. -->
              {#if hasBody}
                <div class="prose-step"><Markdown content={step.description} /></div>
              {/if}
            {:else if editingDescOf === step.id}
              <textarea
                class="w-full bg-transparent outline-none text-body-sm leading-relaxed text-[var(--text)]
                       border border-[var(--border)] rounded-md p-2 resize-y min-h-[80px]"
                bind:value={descDraft}
                autofocus
                placeholder="Describe this step… (markdown supported)"
                onkeydown={(e) => { if (e.key === 'Escape') cancelEditDesc(); if ((e.ctrlKey || e.metaKey) && e.key === 's') { e.preventDefault(); commitEditDesc(); } }}
              ></textarea>
              <div class="flex items-center gap-2 mt-1">
                <button class="text-caption font-medium text-[var(--accent-text)] bg-[var(--accent)] px-2 py-1 rounded-md hover:bg-[var(--accent-hover)]" onclick={commitEditDesc}>Save</button>
                <button class="text-caption text-[var(--text-muted)] px-2 py-1 rounded-md hover:bg-[var(--bg-subtle)]" onclick={cancelEditDesc}>Cancel</button>
                <span class="text-micro text-[var(--text-faint)] ml-auto">Markdown · Esc to cancel · ⌘S to save</span>
              </div>
            {:else if hasBody}
              <button class="block w-full text-left prose-step" onclick={() => startEditDesc(step)} title="Click to edit">
                <Markdown content={step.description} />
              </button>
            {:else}
              <button class="text-body-sm italic text-[var(--text-faint)] hover:text-[var(--text-muted)]" onclick={() => startEditDesc(step)}>
                Add details…
              </button>
            {/if}
          </div>
        {/if}

        {#if addingChildOf === step.id}
          <div class="flex items-center gap-2 py-1">
            <input
              class="flex-1 bg-transparent outline-none text-body text-[var(--text)] border-b border-[var(--accent)]"
              placeholder="Sub-step title…"
              bind:value={childTitle}
              autofocus
              onkeydown={(e) => { if (e.key === "Enter") commitAddChild(); if (e.key === "Escape") addingChildOf = null; }}
              onblur={commitAddChild}
            />
          </div>
        {/if}
      </div>
    </div>

    {#if expanded}
      {#each step.children as child (child.id)}
        {@render stepNode(child, depth + 1)}
      {/each}
    {/if}
  </div>
{/snippet}

{#snippet sidebar()}
  {#if plan}
    <div class="issue-meta-aside">
      <!-- Status (LIF-234: read-only for viewers) -->
      <div class="issue-meta-field">
        <p class="issue-meta-field-label">Status</p>
        <div class="relative">
          <button
            class="flex items-center gap-2 text-body-sm rounded-md px-2 py-1 -mx-2 w-full text-left {canEdit ? 'hover:bg-[var(--bg-subtle)]' : 'cursor-default'}"
            onclick={(e) => { if (!canEdit) return; e.stopPropagation(); statusOpen = !statusOpen; }}
          >
            <span class="size-2 rounded-full {plan.status === 'active' ? 'bg-[var(--accent)]' : plan.status === 'done' ? 'bg-[var(--success)]' : 'bg-[var(--text-faint)]'}"></span>
            <span class="capitalize text-[var(--text)]">{plan.status}</span>
          </button>
          {#if statusOpen && canEdit}
            <div class="absolute left-0 top-full mt-1 z-20 w-[160px] bg-[var(--surface)] border border-[var(--border)] rounded-md shadow-lg py-1"
                 role="presentation" onclick={(e) => e.stopPropagation()} onkeydown={(e) => e.stopPropagation()}>
              {#each STATUSES as s}
                <button
                  class="w-full flex items-center gap-2 px-3 py-1.5 text-left text-body-sm capitalize
                         {s === plan.status ? 'text-[var(--accent)] bg-[var(--accent-subtle)]' : 'text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
                  onclick={() => setStatus(s)}
                >
                  {s}
                </button>
              {/each}
            </div>
          {/if}
        </div>
      </div>

      <!-- Progress -->
      <div class="issue-meta-field">
        <p class="issue-meta-field-label">Progress</p>
        <div class="flex items-center gap-2">
          <div class="flex-1 h-1.5 rounded-full bg-[var(--bg-subtle)] overflow-hidden">
            <div class="h-full bg-[var(--accent)] rounded-full transition-all" style="width: {progress * 100}%"></div>
          </div>
          <span class="text-caption text-[var(--text-muted)] tabular-nums">{plan.done_count}/{plan.step_count}</span>
        </div>
      </div>

      <!-- Anchor issue -->
      <div class="issue-meta-field">
        <p class="issue-meta-field-label">Anchor issue</p>
        <div class="flex items-center gap-1.5 -mx-2 px-2">
          {#if plan.anchor_identifier}
            <button class="text-body-sm font-mono text-[var(--accent)] hover:underline flex items-center gap-1"
                    title="Shift-click to preview"
                    onclick={(e) => plan?.anchor_identifier && openIssueChip(e, plan.anchor_identifier)}>
              {plan.anchor_identifier}<ArrowUpRight size={12} />
            </button>
            {#if canEdit}
              <button class="ml-auto text-[var(--text-faint)] hover:text-[var(--text)] text-caption" onclick={setAnchor}>change</button>
            {/if}
          {:else if canEdit}
            <button class="text-body-sm text-[var(--text-faint)] hover:text-[var(--text)]" onclick={setAnchor}>Set anchor…</button>
          {:else}
            <span class="text-body-sm text-[var(--text-faint)]">None</span>
          {/if}
        </div>
      </div>

      <div class="border-t border-[var(--border)] -mx-5 px-5 py-0 my-1"></div>

      <div class="issue-meta-dates">
        <div class="issue-meta-field">
          <p class="issue-meta-field-label">Created</p>
          <p class="text-body-sm text-[var(--text-muted)] leading-snug m-0">{formatDate(plan.created_at)}</p>
        </div>
        <div class="issue-meta-field">
          <p class="issue-meta-field-label">Updated</p>
          <p class="text-body-sm text-[var(--text-muted)] leading-snug m-0">{formatDate(plan.updated_at)}</p>
        </div>
      </div>
    </div>
  {/if}
{/snippet}

<svelte:window onclick={() => (statusOpen = false)} />
