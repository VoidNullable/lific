<script lang="ts">
  import {
    resolveIssue,
    updateIssue,
    deleteIssue,
    downloadIssueExport,
    listModules,
    listLabels,
    createLabel,
    listComments,
    createComment,
    listIssueActivity,
    type Issue,
    type Module,
    type Label,
    type Comment,
    type Activity,
  } from "../lib/api";
  import DocumentDetail from "../lib/DocumentDetail.svelte";
  import LabelEditor from "../lib/LabelEditor.svelte";
  import ProjectIcon from "../lib/ProjectIcon.svelte";
  import PriorityIcon from "../lib/PriorityIcon.svelte";
  import StatusIcon, { statusCssColor } from "../lib/StatusIcon.svelte";
  import { formatDate } from "../lib/format";
  import { recordRecent } from "../lib/home/recents"; // LIF-237
  import { updateIssueWithUndo } from "../lib/issues/state.svelte"; // LIF-243
  import { openPeek } from "../lib/issues/peek.svelte"; // LIF-248
  import { ArrowUpRight } from "lucide-svelte";

  let {
    navigate,
    projectIdentifier,
    issueIdentifier,
    editable = true,
  }: {
    navigate: (path: string) => void;
    projectIdentifier: string;
    issueIdentifier: string;
    editable?: boolean;
  } = $props();

  // Back-arrow destination mirrors whichever list layout the user was
  // last viewing for this project (set by IssueList). Falling back to
  // the flat issues list preserves prior behavior when nothing's stored.
  function storedLayout(): "board" | "list" {
    try {
      const raw = localStorage.getItem(`lific:list:layout:${projectIdentifier}`);
      if (raw === "board" || raw === "list") return raw;
    } catch {
      // ignore
    }
    return "list";
  }
  function backHref(): string {
    return storedLayout() === "board"
      ? `/${projectIdentifier}/board`
      : `/${projectIdentifier}/issues`;
  }
  function backText(): string {
    return storedLayout() === "board" ? "Board" : "Issues";
  }

  let issue = $state<Issue | null>(null);
  let modules = $state<Module[]>([]);
  let labels = $state<Label[]>([]);
  let comments = $state<Comment[]>([]);
  let activity = $state<Activity[]>([]);
  let loading = $state(true);
  let error = $state("");

  // Sidebar dropdown states (issue-specific; the body's read/edit mode
  // lives inside DocumentDetail).
  let statusOpen = $state(false);
  let priorityOpen = $state(false);
  let moduleOpen = $state(false);
  let labelsOpen = $state(false);

  // Save indicator
  let saving = $state(false);
  let lastSaved = $state<string | null>(null);

  // Export
  let exportError = $state("");
  let exporting = $state(false);

  const STATUSES = [
    { value: "backlog", label: "Backlog" },
    { value: "todo", label: "Todo" },
    { value: "active", label: "Active" },
    { value: "done", label: "Done" },
    { value: "cancelled", label: "Cancelled" },
  ];

  const PRIORITIES = [
    { value: "urgent", label: "Urgent" },
    { value: "high", label: "High" },
    { value: "medium", label: "Medium" },
    { value: "low", label: "Low" },
    { value: "none", label: "None" },
  ];

  $effect(() => {
    const id = issueIdentifier;
    statusOpen = false;
    priorityOpen = false;
    moduleOpen = false;
    labelsOpen = false;
    lastSaved = null;
    loadIssue(id);
  });

  async function loadIssue(identifier: string) {
    loading = true;
    error = "";
    const res = await resolveIssue(identifier);
    if (!res.ok) {
      error = res.error;
      loading = false;
      return;
    }
    issue = res.data;
    recordRecent({ type: "issue", routeId: issue.identifier, identifier: issue.identifier, title: issue.title, project: projectIdentifier }); // LIF-237

    const [modRes, lblRes, cmtRes, actRes] = await Promise.all([
      listModules(issue.project_id),
      listLabels(issue.project_id),
      listComments(issue.id),
      listIssueActivity(issue.id),
    ]);
    if (modRes.ok) modules = modRes.data;
    if (lblRes.ok) labels = lblRes.data;
    if (cmtRes.ok) comments = cmtRes.data;
    if (actRes.ok) activity = actRes.data.items;

    loading = false;
  }

  // Re-pull the timeline after any mutation so the user's own edit shows
  // up in Activity immediately (it was just audited server-side).
  async function refreshActivity() {
    if (!issue) return;
    const res = await listIssueActivity(issue.id);
    if (res.ok) activity = res.data.items;
  }

  // Close sidebar dropdowns on outside click. (LabelEditor + the topbar
  // DeleteMenu manage their own outside-click close.)
  function handleWindowClick() {
    statusOpen = false;
    priorityOpen = false;
    moduleOpen = false;
    labelsOpen = false;
  }

  function closeOtherDropdowns() {
    statusOpen = false;
    priorityOpen = false;
    moduleOpen = false;
  }

  // ── Save helpers ─────────────────────────────────────

  async function saveField(field: string, value: unknown) {
    if (!issue) return;
    saving = true;
    const res = await updateIssue(issue.id, { [field]: value });
    if (res.ok) {
      issue = res.data;
      lastSaved = new Date().toLocaleTimeString([], {
        hour: "2-digit",
        minute: "2-digit",
      });
      refreshActivity();
    }
    saving = false;
  }

  async function saveTitle(next: string) {
    await saveField("title", next);
  }

  async function saveDescription(next: string) {
    if (!issue) return;
    if (next !== issue.description) {
      await saveField("description", next);
    }
  }

  // ── Metadata updates ─────────────────────────────────
  // LIF-243: status/priority/module are one-click reversible, so they skip
  // the plain `saveField` path in favor of `saveFieldWithUndo`, which shows
  // a toast with a single-shot Undo. Title/description/labels stay on
  // `saveField` — editing text isn't a "one value flips to another" action
  // undo makes sense for.

  /** Shares saveField's saving/lastSaved/activity-refresh side effects, but
   *  routes the mutation through updateIssueWithUndo for the toast + Undo
   *  affordance. `onApplied` fires both after the forward save and — if the
   *  user clicks Undo later, possibly from a different route entirely —
   *  after the reverting save; guarding on `issue.id` keeps it a no-op if
   *  the sidebar has since loaded a different issue. */
  async function saveFieldWithUndo(
    patch: Record<string, unknown>,
    prevPatch: Record<string, unknown>,
  ) {
    if (!issue) return;
    const id = issue.id;
    const identifier = issue.identifier;
    saving = true;
    await updateIssueWithUndo({
      id,
      identifier,
      patch,
      prevPatch,
      modules,
      onApplied: (applied) => {
        if (issue && issue.id === id) {
          issue = { ...issue, ...(applied as Partial<Issue>) };
        }
        lastSaved = new Date().toLocaleTimeString([], {
          hour: "2-digit",
          minute: "2-digit",
        });
        refreshActivity();
      },
    });
    saving = false;
  }

  async function setStatus(value: string) {
    statusOpen = false;
    if (issue && value !== issue.status) {
      await saveFieldWithUndo({ status: value }, { status: issue.status });
    }
  }

  async function setPriority(value: string) {
    priorityOpen = false;
    if (issue && value !== issue.priority) {
      await saveFieldWithUndo({ priority: value }, { priority: issue.priority });
    }
  }

  async function setModule(id: number | null) {
    moduleOpen = false;
    if (!issue) return;
    if (id !== issue.module_id) {
      await saveFieldWithUndo({ module_id: id }, { module_id: issue.module_id });
    }
  }

  async function toggleLabel(name: string) {
    if (!issue) return;
    const current = [...issue.labels];
    const idx = current.indexOf(name);
    if (idx >= 0) current.splice(idx, 1);
    else current.push(name);
    await saveField("labels", current);
  }

  // Inline label creation from the picker (label management). Creates the
  // project label, folds it into the local `labels` list, and attaches it to
  // this issue. Returns success so LabelEditor can reset its create form.
  async function createLabelInline(name: string, color: string): Promise<boolean> {
    if (!issue) return false;
    const res = await createLabel({ project_id: issue.project_id, name, color });
    if (!res.ok) return false;
    labels = [...labels, res.data].sort((a, b) => a.name.localeCompare(b.name));
    await toggleLabel(res.data.name);
    return true;
  }

  // LIF-248: shared by every relation chip (blocked-by / blocks / related)
  // below — shift-click peeks instead of navigating, mirroring
  // Markdown.svelte's identifier links and IssueCard's shift-click.
  function openRelation(e: MouseEvent, rel: string) {
    if (e.shiftKey) {
      e.preventDefault();
      openPeek(rel);
      return;
    }
    navigate(`/${projectIdentifier}/issues/${rel}`);
  }

  // ── Comments / export / delete ───────────────────────

  async function handleNewComment(content: string) {
    if (!issue) return null;
    const res = await createComment(issue.id, content);
    if (!res.ok) return null;
    comments = [...comments, res.data];
    refreshActivity();
    return res.data;
  }

  async function exportMarkdown() {
    if (!issue || exporting) return;
    exporting = true;
    exportError = "";
    const res = await downloadIssueExport(issue.identifier);
    if (!res.ok) exportError = res.error;
    exporting = false;
  }

  async function handleDelete(): Promise<boolean> {
    if (!issue) return false;
    const res = await deleteIssue(issue.id);
    if (res.ok) {
      navigate(backHref());
      return true;
    }
    return false;
  }

  function moduleName(id: number | null): string {
    if (!id) return "None";
    return modules.find((m) => m.id === id)?.name ?? "Unknown";
  }

  // ── LIF-159: palette actions ─────────────────────────
  // Specialized commands for the issue view, surfaced through cmd+k /
  // ctrl+p. Derived so hints (current status/priority/…) stay live.
  let paletteActions = $derived.by<import("../lib/palette").PaletteAction[]>(() => {
    if (!issue) return [];
    const i = issue;
    return [
      {
        id: "set-status",
        title: "Set status…",
        hint: i.status,
        children: () =>
          STATUSES.map((s) => ({
            title: s.label,
            status: s.value,
            hint: s.value === i.status ? "current" : undefined,
            run: () => void setStatus(s.value),
          })),
      },
      {
        id: "set-priority",
        title: "Set priority…",
        hint: i.priority,
        children: () =>
          PRIORITIES.map((p) => ({
            title: p.label,
            priority: p.value,
            hint: p.value === i.priority ? "current" : undefined,
            run: () => void setPriority(p.value),
          })),
      },
      ...(modules.length > 0
        ? [
            {
              id: "set-module",
              title: "Set module…",
              hint: moduleName(i.module_id),
              children: () => [
                {
                  title: "None",
                  hint: i.module_id === null ? "current" : undefined,
                  run: () => void setModule(null),
                },
                ...modules.map((m) => ({
                  title: m.name,
                  hint: m.id === i.module_id ? "current" : undefined,
                  run: () => void setModule(m.id),
                })),
              ],
            },
          ]
        : []),
      ...(labels.length > 0
        ? [
            {
              id: "toggle-label",
              title: "Add or remove label…",
              hint: i.labels.length > 0 ? i.labels.join(", ") : undefined,
              children: () =>
                labels.map((l) => ({
                  title: l.name,
                  color: l.color,
                  hint: i.labels.includes(l.name) ? "remove" : "add",
                  run: () => void toggleLabel(l.name),
                })),
            },
          ]
        : []),
    ];
  });

  function moduleEmoji(id: number | null): string | null {
    if (!id) return null;
    return modules.find((m) => m.id === id)?.emoji ?? null;
  }
</script>

<svelte:window onclick={handleWindowClick} />

<DocumentDetail
  {navigate}
  {loading}
  {error}
  deleteNounLabel="issue"
  onRetry={() => loadIssue(issueIdentifier)}
  identifier={issue?.identifier ?? issueIdentifier}
  attachEntity={issue ? { entity_type: "issue", entity_id: issue.id } : null}
  backRoute={backHref()}
  backLabel={backText()}
  {editable}
  title={issue?.title ?? ""}
  titleSize="md"
  onSaveTitle={saveTitle}
  body={issue?.description ?? ""}
  bodyPlaceholder="Add a description... (markdown supported)"
  bodyEmptyEditCta="Click to add a description..."
  bodyEmptyReadText="No description"
  bodyProseMinHeight="60px"
  onSaveBody={saveDescription}
  autofocusWhenEmpty
  {saving}
  {lastSaved}
  onExport={exportMarkdown}
  {exporting}
  {exportError}
  deleteNoun="issue"
  deleteLabel={issue?.identifier ?? ""}
  onDelete={handleDelete}
  {comments}
  onNewComment={handleNewComment}
  {activity}
  {paletteActions}
  layout="two-column"
>
  {#snippet breadcrumbExtra()}
    {#if issue}
      <span class="text-[var(--text-faint)]">/</span>
      <span class="flex items-center gap-1.5 text-body-sm">
        <StatusIcon status={issue.status} size={13} />
        <span class="capitalize" style="color: {statusCssColor(issue.status)}">
          {issue.status}
        </span>
      </span>
    {/if}
  {/snippet}

  {#snippet sidebar()}
    {#if issue}
      <div class="issue-meta-aside">
        <div class="issue-meta-field">
          {@render sidebarField("Status")}
          <div class="relative">
            <button
              class="flex items-center gap-2 text-body-sm rounded-md
                     px-2 py-1 -mx-2 transition-colors w-full text-left
                     {editable ? 'hover:bg-[var(--bg-subtle)] cursor-pointer' : 'cursor-default'}"
              onclick={(e) => {
                if (!editable) return;
                e.stopPropagation();
                statusOpen = !statusOpen;
                priorityOpen = false;
                moduleOpen = false;
                labelsOpen = false;
              }}
            >
              <StatusIcon status={issue.status} size={14} />
              <span class="capitalize text-[var(--text)]">{issue.status}</span>
            </button>
            {#if statusOpen}
              <div
                class="absolute left-0 top-full mt-1 z-20 w-[180px]
                       bg-[var(--surface)] border border-[var(--border)]
                       rounded-md shadow-lg py-1"
                role="presentation"
                onclick={(e) => e.stopPropagation()}
                onkeydown={(e) => e.stopPropagation()}
              >
                {#each STATUSES as s}
                  <button
                    class="w-full flex items-center gap-2 px-3 py-1.5 text-left
                           text-body-sm transition-colors
                           {s.value === issue.status
                      ? 'text-[var(--accent)] bg-[var(--accent-subtle)]'
                      : 'text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
                    onclick={() => setStatus(s.value)}
                  >
                    <StatusIcon status={s.value} size={14} />
                    {s.label}
                  </button>
                {/each}
              </div>
            {/if}
          </div>
        </div>

        <div class="issue-meta-field">
          {@render sidebarField("Priority")}
          <div class="relative">
            <button
              class="flex items-center gap-2 flex-nowrap text-body-sm rounded-md
                     px-2 py-1 -mx-2 transition-colors w-full text-left
                     {editable ? 'hover:bg-[var(--bg-subtle)] cursor-pointer' : 'cursor-default'}"
              onclick={(e) => {
                if (!editable) return;
                e.stopPropagation();
                priorityOpen = !priorityOpen;
                statusOpen = false;
                moduleOpen = false;
                labelsOpen = false;
              }}
            >
              <PriorityIcon priority={issue.priority} />
              <span class={priorityTextClass(issue.priority)}>
                {issue.priority === "none" ? "No priority" : issue.priority.charAt(0).toUpperCase() + issue.priority.slice(1)}
              </span>
            </button>
            {#if priorityOpen}
              <div
                class="absolute left-0 top-full mt-1 z-20 w-[180px]
                       bg-[var(--surface)] border border-[var(--border)]
                       rounded-md shadow-lg py-1"
                role="presentation"
                onclick={(e) => e.stopPropagation()}
                onkeydown={(e) => e.stopPropagation()}
              >
                {#each PRIORITIES as p}
                  <button
                    class="w-full flex items-center gap-2 px-3 py-1.5 text-left
                           text-body-sm transition-colors
                           {p.value === issue.priority
                      ? 'text-[var(--accent)] bg-[var(--accent-subtle)]'
                      : 'text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
                    onclick={() => setPriority(p.value)}
                  >
                    <PriorityIcon priority={p.value} />
                    {p.label}
                  </button>
                {/each}
              </div>
            {/if}
          </div>
        </div>

        <div class="issue-meta-field">
          {@render sidebarField("Module")}
          <div class="relative">
            <!-- Trigger opens the assignment dropdown; the arrow jumps to
                 the module's detail page (LIF-121). -->
            <div class="flex items-center -mx-2">
              <button
                class="flex items-center gap-2 text-body-sm rounded-md
                       px-2 py-1 transition-colors flex-1 text-left
                       {editable ? 'hover:bg-[var(--bg-subtle)] cursor-pointer' : 'cursor-default'}"
                onclick={(e) => {
                  if (!editable) return;
                  e.stopPropagation();
                  moduleOpen = !moduleOpen;
                  statusOpen = false;
                  priorityOpen = false;
                  labelsOpen = false;
                }}
              >
                {#if moduleEmoji(issue.module_id)}
                  <ProjectIcon value={moduleEmoji(issue.module_id)} size={14} class="text-[var(--text-muted)] shrink-0" />
                {/if}
                <span class={issue.module_id ? "text-[var(--text)]" : "text-[var(--text-faint)]"}>
                  {moduleName(issue.module_id)}
                </span>
              </button>
              {#if issue.module_id}
                {@const targetModuleId = issue.module_id}
                <button
                  class="size-6 flex items-center justify-center rounded
                         text-[var(--text-faint)] hover:text-[var(--accent)]
                         hover:bg-[var(--bg-subtle)] transition-colors shrink-0"
                  onclick={(e) => {
                    e.stopPropagation();
                    navigate(`/${projectIdentifier}/modules/${targetModuleId}`);
                  }}
                  title="Open module"
                >
                  <ArrowUpRight size={13} />
                </button>
              {/if}
            </div>
            {#if moduleOpen}
              <div
                class="absolute left-0 top-full mt-1 z-20 w-[180px]
                       bg-[var(--surface)] border border-[var(--border)]
                       rounded-md shadow-lg py-1"
                role="presentation"
                onclick={(e) => e.stopPropagation()}
                onkeydown={(e) => e.stopPropagation()}
              >
                <button
                  class="w-full px-3 py-1.5 text-left text-body-sm
                         text-[var(--text-faint)] hover:bg-[var(--bg-subtle)]
                         transition-colors"
                  onclick={() => setModule(null)}
                >
                  None
                </button>
                {#each modules as mod}
                  <button
                    class="w-full flex items-center gap-2 px-3 py-1.5 text-left
                           text-body-sm transition-colors
                           {mod.id === issue.module_id
                      ? 'text-[var(--accent)] bg-[var(--accent-subtle)]'
                      : 'text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
                    onclick={() => setModule(mod.id)}
                  >
                    {#if mod.emoji}
                      <ProjectIcon value={mod.emoji} size={14} class="shrink-0" />
                    {/if}
                    {mod.name}
                  </button>
                {/each}
              </div>
            {/if}
          </div>
        </div>

        <div class="issue-meta-field">
          {@render sidebarField("Labels")}
          <LabelEditor
            attached={issue.labels}
            all={labels}
            {editable}
            onToggle={toggleLabel}
            onCreate={editable ? createLabelInline : undefined}
            bind:open={labelsOpen}
            onOpen={closeOtherDropdowns}
          />
        </div>

        <div class="border-t border-[var(--border)] -mx-5 px-5 py-0 my-1"></div>

        {#if (issue.blocks && issue.blocks.length > 0) || (issue.blocked_by && issue.blocked_by.length > 0) || (issue.relates_to && issue.relates_to.length > 0)}
          <div class="issue-meta-relations">
            {#if issue.blocked_by && issue.blocked_by.length > 0}
              <div class="issue-meta-field">
                {@render sidebarField("Blocked by")}
                <div class="flex flex-wrap gap-1.5">
                  {#each issue.blocked_by as rel}
                    <button
                      class="text-caption font-mono text-[var(--error)]
                             bg-[var(--error-bg)] px-1.5 py-0.5 rounded
                             hover:underline transition-colors"
                      title="{rel}  ·  Shift-click to preview"
                      onclick={(e) => openRelation(e, rel)}
                    >
                      {rel}
                    </button>
                  {/each}
                </div>
              </div>
            {/if}
            {#if issue.blocks && issue.blocks.length > 0}
              <div class="issue-meta-field">
                {@render sidebarField("Blocks")}
                <div class="flex flex-wrap gap-1.5">
                  {#each issue.blocks as rel}
                    <button
                      class="text-caption font-mono text-[var(--accent)]
                             bg-[var(--accent-subtle)] px-1.5 py-0.5 rounded
                             hover:underline transition-colors"
                      title="{rel}  ·  Shift-click to preview"
                      onclick={(e) => openRelation(e, rel)}
                    >
                      {rel}
                    </button>
                  {/each}
                </div>
              </div>
            {/if}
            {#if issue.relates_to && issue.relates_to.length > 0}
              <div class="issue-meta-field">
                {@render sidebarField("Related")}
                <div class="flex flex-wrap gap-1.5">
                  {#each issue.relates_to as rel}
                    <button
                      class="text-caption font-mono text-[var(--text-muted)]
                             bg-[var(--bg-subtle)] px-1.5 py-0.5 rounded
                             hover:underline transition-colors"
                      title="{rel}  ·  Shift-click to preview"
                      onclick={(e) => openRelation(e, rel)}
                    >
                      {rel}
                    </button>
                  {/each}
                </div>
              </div>
            {/if}
          </div>

          <div class="border-t border-[var(--border)] -mx-5 px-5 py-0 my-1"></div>
        {/if}

        <div class="issue-meta-dates">
          <div class="issue-meta-field">
            {@render sidebarField("Created")}
            <p class="text-body-sm text-[var(--text-muted)] leading-snug m-0">
              {formatDate(issue.created_at)}
            </p>
          </div>
          <div class="issue-meta-field">
            {@render sidebarField("Updated")}
            <p class="text-body-sm text-[var(--text-muted)] leading-snug m-0">
              {formatDate(issue.updated_at)}
            </p>
          </div>
        </div>
      </div>
    {/if}
  {/snippet}
</DocumentDetail>

{#snippet sidebarField(label: string)}
  <p class="issue-meta-field-label">{label}</p>
{/snippet}

<script lang="ts" module>
  function priorityTextClass(priority: string): string {
    switch (priority) {
      case "urgent": return "text-[var(--error)]";
      case "high": return "text-[var(--warn)]";
      case "medium": return "text-[var(--accent)]";
      default: return "text-[var(--text)]";
    }
  }
</script>
