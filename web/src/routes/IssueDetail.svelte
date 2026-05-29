<script lang="ts">
  import {
    resolveIssue,
    updateIssue,
    deleteIssue,
    downloadIssueExport,
    listModules,
    listLabels,
    listComments,
    createComment,
    type Issue,
    type Module,
    type Label,
    type Comment,
  } from "../lib/api";
  import DocumentDetail from "../lib/DocumentDetail.svelte";
  import LabelEditor from "../lib/LabelEditor.svelte";
  import { formatDate } from "../lib/format";
  import {
    CircleAlert, Circle, CircleDot, CircleDashed, CircleCheckBig, CircleX,
    ArrowUpRight,
  } from "lucide-svelte";

  function statusCssColor(s: string): string {
    switch (s) {
      case "backlog": return "var(--text-faint)";
      case "todo": return "var(--text-muted)";
      case "active": return "var(--accent)";
      case "done": return "var(--success)";
      case "cancelled": return "var(--text-faint)";
      default: return "var(--text-faint)";
    }
  }

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

    const [modRes, lblRes, cmtRes] = await Promise.all([
      listModules(issue.project_id),
      listLabels(issue.project_id),
      listComments(issue.id),
    ]);
    if (modRes.ok) modules = modRes.data;
    if (lblRes.ok) labels = lblRes.data;
    if (cmtRes.ok) comments = cmtRes.data;

    loading = false;
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

  async function setStatus(value: string) {
    statusOpen = false;
    if (issue && value !== issue.status) await saveField("status", value);
  }

  async function setPriority(value: string) {
    priorityOpen = false;
    if (issue && value !== issue.priority) await saveField("priority", value);
  }

  async function setModule(id: number | null) {
    moduleOpen = false;
    if (!issue) return;
    if (id !== issue.module_id) await saveField("module_id", id);
  }

  async function toggleLabel(name: string) {
    if (!issue) return;
    const current = [...issue.labels];
    const idx = current.indexOf(name);
    if (idx >= 0) current.splice(idx, 1);
    else current.push(name);
    await saveField("labels", current);
  }

  // ── Comments / export / delete ───────────────────────

  async function handleNewComment(content: string) {
    if (!issue) return null;
    const res = await createComment(issue.id, content);
    if (!res.ok) return null;
    comments = [...comments, res.data];
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
</script>

<svelte:window onclick={handleWindowClick} />

<DocumentDetail
  {navigate}
  {loading}
  {error}
  identifier={issue?.identifier ?? issueIdentifier}
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
  layout="two-column"
>
  {#snippet sidebar()}
    {#if issue}
      <div class="issue-meta-aside">
        <div class="issue-meta-field">
          {@render sidebarField("Status")}
          <div class="relative">
            <button
              class="flex items-center gap-2 text-[0.8125rem] rounded-md
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
              {@render statusIcon(issue.status, 14)}
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
                           text-[0.8125rem] transition-colors
                           {s.value === issue.status
                      ? 'text-[var(--accent)] bg-[var(--accent-subtle)]'
                      : 'text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
                    onclick={() => setStatus(s.value)}
                  >
                    {@render statusIcon(s.value, 14)}
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
              class="flex items-center gap-2 flex-nowrap text-[0.8125rem] rounded-md
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
              {@render priorityIcon(issue.priority)}
              <span class="text-[var(--text)] {priorityTextClass(issue.priority)}">
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
                           text-[0.8125rem] transition-colors
                           {p.value === issue.priority
                      ? 'text-[var(--accent)] bg-[var(--accent-subtle)]'
                      : 'text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
                    onclick={() => setPriority(p.value)}
                  >
                    {@render priorityIcon(p.value)}
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
                class="flex items-center gap-2 text-[0.8125rem] rounded-md
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
                <span class="text-[var(--text)] {issue.module_id ? '' : 'text-[var(--text-faint)]'}">
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
                  class="w-full px-3 py-1.5 text-left text-[0.8125rem]
                         text-[var(--text-faint)] hover:bg-[var(--bg-subtle)]
                         transition-colors"
                  onclick={() => setModule(null)}
                >
                  None
                </button>
                {#each modules as mod}
                  <button
                    class="w-full px-3 py-1.5 text-left text-[0.8125rem]
                           transition-colors
                           {mod.id === issue.module_id
                      ? 'text-[var(--accent)] bg-[var(--accent-subtle)]'
                      : 'text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
                    onclick={() => setModule(mod.id)}
                  >
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
                      class="text-[0.75rem] font-mono text-[var(--error)]
                             bg-[var(--error-bg)] px-1.5 py-0.5 rounded
                             hover:underline transition-colors"
                      onclick={() => navigate(`/${projectIdentifier}/issues/${rel}`)}
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
                      class="text-[0.75rem] font-mono text-[var(--accent)]
                             bg-[var(--accent-subtle)] px-1.5 py-0.5 rounded
                             hover:underline transition-colors"
                      onclick={() => navigate(`/${projectIdentifier}/issues/${rel}`)}
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
                      class="text-[0.75rem] font-mono text-[var(--text-muted)]
                             bg-[var(--bg-subtle)] px-1.5 py-0.5 rounded
                             hover:underline transition-colors"
                      onclick={() => navigate(`/${projectIdentifier}/issues/${rel}`)}
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
            <p class="text-[0.8125rem] text-[var(--text-muted)] leading-snug m-0">
              {formatDate(issue.created_at)}
            </p>
          </div>
          <div class="issue-meta-field">
            {@render sidebarField("Updated")}
            <p class="text-[0.8125rem] text-[var(--text-muted)] leading-snug m-0">
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

{#snippet priorityIcon(priority: string)}
  {#if priority === "urgent"}
    <CircleAlert size={14} class="text-[var(--error)]" />
  {:else if priority === "high"}
    <svg class="size-3.5 text-orange-500" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <line x1="5" y1="12" x2="19" y2="12"/><line x1="5" y1="6" x2="19" y2="6"/><line x1="5" y1="18" x2="19" y2="18"/>
    </svg>
  {:else if priority === "medium"}
    <svg class="size-3.5 text-[var(--accent)]" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <line x1="5" y1="9" x2="19" y2="9"/><line x1="5" y1="15" x2="19" y2="15"/>
    </svg>
  {:else if priority === "low"}
    <svg class="size-3.5 text-[var(--text-muted)]" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <line x1="5" y1="12" x2="19" y2="12"/>
    </svg>
  {:else}
    <span class="size-3.5"></span>
  {/if}
{/snippet}

{#snippet statusIcon(status: string, size: number)}
  {#if status === "done"}
    <CircleCheckBig {size} style="color: {statusCssColor(status)}" />
  {:else if status === "cancelled"}
    <CircleX {size} style="color: {statusCssColor(status)}" />
  {:else if status === "active"}
    <CircleDot {size} style="color: {statusCssColor(status)}" />
  {:else if status === "backlog"}
    <CircleDashed {size} style="color: {statusCssColor(status)}" />
  {:else}
    <Circle {size} style="color: {statusCssColor(status)}" />
  {/if}
{/snippet}

<script lang="ts" module>
  function priorityTextClass(priority: string): string {
    switch (priority) {
      case "urgent": return "text-[var(--error)]";
      case "high": return "text-orange-500";
      case "medium": return "text-[var(--accent)]";
      default: return "";
    }
  }
</script>
