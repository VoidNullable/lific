<script lang="ts">
  import {
    resolveIssue,
    updateIssue,
    deleteIssue,
    listModules,
    listLabels,
    listComments,
    createComment,
    type Issue,
    type Module,
    type Label,
    type Comment,
  } from "../lib/api";
  import Markdown from "../lib/Markdown.svelte";
  import {
    ArrowLeft, Ellipsis, Trash2, Plus, X, Check,
    CircleCheck, CircleX, CircleAlert,
    Circle, CircleDot, CircleDashed, CircleCheckBig,
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

  let issue = $state<Issue | null>(null);
  let modules = $state<Module[]>([]);
  let labels = $state<Label[]>([]);
  let comments = $state<Comment[]>([]);
  let loading = $state(true);
  let error = $state("");

  // Editing states
  let editingTitle = $state(false);
  let editingDescription = $state(false);
  let draftTitle = $state("");
  let draftDescription = $state("");
  let descriptionEl = $state<HTMLTextAreaElement | null>(null);
  let descriptionWrapperEl = $state<HTMLElement | null>(null);
  let preEditHeight = $state<number | null>(null);

  // Dropdown states
  let statusOpen = $state(false);
  let priorityOpen = $state(false);
  let moduleOpen = $state(false);
  let labelsOpen = $state(false);

  // Comment
  let commentDraft = $state("");
  let commentSubmitting = $state(false);

  // Save indicator
  let saving = $state(false);
  let lastSaved = $state<string | null>(null);

  // Kebab menu
  let menuOpen = $state(false);
  let confirmingDelete = $state(false);
  let deleting = $state(false);

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
    // Reset all editing state when switching issues
    editingTitle = false;
    editingDescription = false;
    preEditHeight = null;
    menuOpen = false;
    confirmingDelete = false;
    statusOpen = false;
    priorityOpen = false;
    moduleOpen = false;
    labelsOpen = false;
    commentDraft = "";
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

    // Load metadata and comments in parallel
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

  // Close all dropdowns on outside click
  function handleWindowClick() {
    statusOpen = false;
    priorityOpen = false;
    moduleOpen = false;
    labelsOpen = false;
    menuOpen = false;
    confirmingDelete = false;
  }

  // ── Delete ────────────────────────────────────────────

  async function confirmDelete() {
    if (!issue) return;
    deleting = true;
    const res = await deleteIssue(issue.id);
    if (res.ok) {
      navigate(`/${projectIdentifier}/issues`);
    } else {
      deleting = false;
      confirmingDelete = false;
      menuOpen = false;
    }
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

  // ── Title editing ────────────────────────────────────

  function startEditTitle() {
    if (!editable || !issue) return;
    draftTitle = issue.title;
    editingTitle = true;
  }

  async function commitTitle() {
    if (!issue) return;
    editingTitle = false;
    const trimmed = draftTitle.trim();
    if (trimmed && trimmed !== issue.title) {
      await saveField("title", trimmed);
    }
  }

  function cancelTitle() {
    editingTitle = false;
  }

  // ── Description editing ──────────────────────────────

  function startEditDescription() {
    if (!editable || !issue) return;
    // Capture the rendered content height before swapping to textarea.
    // This becomes the minimum height for the entire editing session so
    // the shorter raw-markdown text never collapses the container.
    if (descriptionWrapperEl) {
      preEditHeight = descriptionWrapperEl.offsetHeight;
    }
    draftDescription = issue.description;
    editingDescription = true;
    requestAnimationFrame(() => autoResize());
  }

  function autoResize() {
    const el = descriptionEl;
    if (!el) return;
    el.style.height = "0";
    const floor = preEditHeight ?? 0;
    el.style.height = Math.max(el.scrollHeight, floor) + "px";
  }

  async function commitDescription() {
    if (!issue) return;
    editingDescription = false;
    preEditHeight = null;
    if (draftDescription !== issue.description) {
      await saveField("description", draftDescription);
    }
  }

  function cancelDescription() {
    editingDescription = false;
    preEditHeight = null;
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
    const current = issue.module_id;
    if (id !== current) await saveField("module_id", id);
  }

  async function toggleLabel(name: string) {
    if (!issue) return;
    const current = [...issue.labels];
    const idx = current.indexOf(name);
    if (idx >= 0) {
      current.splice(idx, 1);
    } else {
      current.push(name);
    }
    await saveField("labels", current);
  }

  // ── Comments ─────────────────────────────────────────

  async function submitComment() {
    if (!issue || !commentDraft.trim()) return;
    commentSubmitting = true;
    const res = await createComment(issue.id, commentDraft.trim());
    if (res.ok) {
      comments = [...comments, res.data];
      commentDraft = "";
    }
    commentSubmitting = false;
  }

  // ── Helpers ──────────────────────────────────────────

  function formatDate(iso: string): string {
    const d = new Date(iso + "Z");
    return d.toLocaleDateString("en-US", {
      month: "short",
      day: "numeric",
      year: "numeric",
      hour: "numeric",
      minute: "2-digit",
    });
  }

  function formatRelative(iso: string): string {
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

  function moduleName(id: number | null): string {
    if (!id) return "None";
    return modules.find((m) => m.id === id)?.name ?? "Unknown";
  }

  function initials(name: string): string {
    return name
      .split(/[\s_-]+/)
      .slice(0, 2)
      .map((w) => w[0]?.toUpperCase() ?? "")
      .join("");
  }
</script>

<svelte:window onclick={handleWindowClick} />

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
      onclick={() => navigate(`/${projectIdentifier}/issues`)}
    >
      Back to issues
    </button>
  </div>
{:else if issue}
  <div class="h-full flex flex-col">
    <!-- Top bar -->
    <div
      class="shrink-0 flex items-center gap-3 px-6 py-2.5
             border-b border-[var(--border)] bg-[var(--surface)]"
    >
      <button
        class="flex items-center gap-1.5 text-[0.8125rem] text-[var(--text-muted)]
               hover:text-[var(--text)] transition-colors rounded px-1.5 py-0.5
               hover:bg-[var(--bg-subtle)]"
        onclick={() => navigate(`/${projectIdentifier}/issues`)}
      >
        <ArrowLeft size={14} />
        Issues
      </button>

      <span class="text-[var(--text-faint)]">/</span>

      <span class="text-[0.8125rem] font-mono text-[var(--text-muted)]">
        {issue.identifier}
      </span>

      <!-- Save indicator -->
      <div class="ml-auto text-[0.75rem] text-[var(--text-faint)]">
        {#if saving}
          <span class="animate-pulse">Saving...</span>
        {:else if lastSaved}
          Saved at {lastSaved}
        {/if}
      </div>
    </div>

    <!-- Content -->
    <div class="flex-1 overflow-y-auto">
      <div class="max-w-[1120px] mx-auto flex gap-0 min-h-full">
        <!-- Main column -->
        <div class="flex-1 min-w-0 px-8 py-6">
          <!-- Title -->
          {#if editingTitle}
            <!-- svelte-ignore a11y_autofocus -->
            <input
              type="text"
              bind:value={draftTitle}
              class="w-full text-[1.5rem] font-display tracking-tight
                     bg-transparent border-none outline-none
                     text-[var(--text)] py-1 mb-4
                     border-b-2 border-b-[var(--accent)]
                     focus:border-b-[var(--accent)]"
              onblur={commitTitle}
              onkeydown={(e) => {
                if (e.key === "Enter") commitTitle();
                if (e.key === "Escape") cancelTitle();
              }}
              autofocus
            />
          {:else if editable}
            <button
              type="button"
              class="text-[1.5rem] font-display font-normal tracking-tight text-[var(--text)]
                     py-1 mb-4 rounded transition-colors w-full text-left
                     bg-transparent border-0 p-0 cursor-text hover:bg-[var(--bg-subtle)]"
              onclick={startEditTitle}
              onkeydown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  startEditTitle();
                }
              }}
            >
              {issue.title}
            </button>
          {:else}
            <h1
              class="text-[1.5rem] font-display tracking-tight text-[var(--text)] py-1 mb-4"
            >
              {issue.title}
            </h1>
          {/if}

          <!-- Description -->
          <section
            class="mb-8"
            style={preEditHeight != null ? `min-height: ${preEditHeight}px;` : ""}
            bind:this={descriptionWrapperEl}
          >
            {#if editingDescription}
              <div>
                <!-- svelte-ignore a11y_autofocus -->
                <textarea
                  bind:value={draftDescription}
                  bind:this={descriptionEl}
                  class="w-full text-[0.875rem] leading-[1.7] text-[var(--text)]
                         bg-transparent border-none outline-none resize-none
                         p-0 m-0 font-[var(--font-body)]"
                  style={preEditHeight != null ? `height: ${preEditHeight}px;` : ""}
                  placeholder="Add a description... (markdown supported)"
                  onkeydown={(e) => {
                    if (e.key === "Escape") cancelDescription();
                  }}
                  oninput={autoResize}
                  autofocus
                ></textarea>
                <div class="flex items-center gap-2 mt-3 pt-3 border-t border-[var(--border)]">
                  <button
                    class="text-[0.8125rem] font-medium text-[var(--accent-text)]
                           bg-[var(--accent)] px-3 py-1.5 rounded-md
                           hover:bg-[var(--accent-hover)] transition-colors"
                    onclick={commitDescription}
                  >
                    Save
                  </button>
                  <button
                    class="text-[0.8125rem] text-[var(--text-muted)] px-3 py-1.5
                           rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
                    onclick={cancelDescription}
                  >
                    Cancel
                  </button>
                  <span class="text-[0.75rem] text-[var(--text-faint)] ml-auto">
                    Markdown · Esc to cancel
                  </span>
                </div>
              </div>
            {:else}
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <div
                class="transition-colors min-h-[60px]
                       {editable ? 'cursor-text hover:bg-[var(--bg-subtle)] rounded-md' : ''}"
                onclick={startEditDescription}
                onkeydown={(e) => { if (e.key === "Enter") startEditDescription(); }}
              >
                {#if issue.description.trim()}
                  <Markdown content={issue.description} />
                {:else}
                  <p
                    class="text-[0.875rem] text-[var(--text-faint)] italic py-2"
                  >
                    {editable ? "Click to add a description..." : "No description"}
                  </p>
                {/if}
              </div>
            {/if}
          </section>

          <!-- Comments -->
          <section>
            <h2
              class="text-[0.8125rem] font-semibold uppercase tracking-widest
                     text-[var(--text-faint)] mb-4"
            >
              Comments
              {#if comments.length > 0}
                <span class="font-normal ml-1">{comments.length}</span>
              {/if}
            </h2>

            {#if comments.length === 0}
              <p class="text-[0.875rem] text-[var(--text-faint)] mb-6">
                No comments yet.
              </p>
            {:else}
              <div class="space-y-0 mb-6">
                {#each comments as comment (comment.id)}
                  <div
                    class="flex gap-3 py-4
                           border-t border-[var(--border)] first:border-t-0"
                  >
                    <!-- Avatar -->
                    <div
                      class="size-7 rounded-full bg-[var(--accent-subtle)]
                             text-[var(--accent)] flex items-center justify-center
                             text-[0.625rem] font-semibold shrink-0 mt-0.5"
                    >
                      {initials(comment.author_display_name || comment.author)}
                    </div>

                    <div class="flex-1 min-w-0">
                      <div class="flex items-baseline gap-2 mb-1">
                        <span class="text-[0.8125rem] font-medium text-[var(--text)]">
                          {comment.author_display_name || comment.author}
                        </span>
                        <span
                          class="text-[0.75rem] text-[var(--text-faint)]"
                          title={formatDate(comment.created_at)}
                        >
                          {formatRelative(comment.created_at)}
                        </span>
                      </div>
                      <Markdown
                        content={comment.content}
                        class="text-sm"
                      />
                    </div>
                  </div>
                {/each}
              </div>
            {/if}

            <!-- New comment -->
            {#if editable}
              <div class="border-t border-[var(--border)] pt-4">
                <textarea
                  bind:value={commentDraft}
                  class="w-full min-h-[80px] text-[0.875rem] leading-relaxed
                         bg-[var(--surface)] border border-[var(--border)]
                         rounded-md p-3 text-[var(--text)]
                         resize-y outline-none placeholder:text-[var(--text-faint)]
                         focus:border-[var(--accent)]
                         focus:shadow-[0_0_0_3px_var(--accent-subtle)]"
                  placeholder="Leave a comment... (markdown supported)"
                  onkeydown={(e) => {
                    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) submitComment();
                  }}
                ></textarea>
                <div class="flex items-center justify-between mt-2">
                  <span class="text-[0.75rem] text-[var(--text-faint)]">
                    Ctrl+Enter to submit
                  </span>
                  <button
                    class="text-[0.8125rem] font-medium text-[var(--accent-text)]
                           bg-[var(--accent)] px-3 py-1.5 rounded-md
                           hover:bg-[var(--accent-hover)] transition-colors
                           disabled:opacity-50 disabled:cursor-not-allowed"
                    disabled={!commentDraft.trim() || commentSubmitting}
                    onclick={submitComment}
                  >
                    {commentSubmitting ? "Posting..." : "Comment"}
                  </button>
                </div>
              </div>
            {/if}
          </section>
        </div>

        <!-- Sidebar: issue-meta-* classes in app.css (explicit gaps; compact joins avoid stray flex whitespace nodes) -->
        <aside
          class="w-[220px] shrink-0 border-l border-[var(--border)] py-6 px-5"
        >
          <!-- Manage -->
          {#if editable}
            <div class="flex items-center justify-between mb-5">
              {@render sidebarField("Manage")}
              <div class="relative">
                <button
                  class="flex items-center gap-1 text-[0.75rem] text-[var(--text-faint)]
                         hover:text-[var(--text)] transition-colors rounded px-1.5 py-0.5
                         hover:bg-[var(--bg-subtle)]"
                  onclick={(e) => {
                    e.stopPropagation();
                    if (confirmingDelete) {
                      confirmingDelete = false;
                      menuOpen = false;
                    } else {
                      menuOpen = !menuOpen;
                    }
                  }}
                >
              <Ellipsis size={14} />
                </button>

                {#if menuOpen && !confirmingDelete}
                  <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
                  <div
                    class="absolute right-0 top-full mt-1 z-20 w-[180px]
                           bg-[var(--surface)] border border-[var(--border)]
                           rounded-md shadow-lg py-1"
                    onclick={(e) => e.stopPropagation()}
                  >
                    <button
                      class="w-full flex items-center gap-2 px-3 py-1.5 text-left
                             text-[0.8125rem] text-[var(--error)]
                             hover:bg-[var(--error-bg)] transition-colors"
                      onclick={() => { confirmingDelete = true; }}
                    >
                      <Trash2 size={14} />
                      Delete issue
                    </button>
                  </div>
                {/if}

                {#if confirmingDelete}
                  <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
                  <div
                    class="absolute right-0 top-full mt-1 z-20 w-[240px]
                           bg-[var(--surface)] border border-[var(--border)]
                           rounded-md shadow-lg p-3"
                    onclick={(e) => e.stopPropagation()}
                  >
                    <p class="text-[0.8125rem] text-[var(--text)] mb-1 font-medium">
                      Delete {issue.identifier}?
                    </p>
                    <p class="text-[0.75rem] text-[var(--text-muted)] mb-3">
                      This can't be undone.
                    </p>
                    <div class="flex items-center gap-2">
                      <button
                        class="text-[0.8125rem] font-medium text-white
                               bg-[var(--error)] px-3 py-1.5 rounded-md
                               hover:opacity-90 transition-opacity
                               disabled:opacity-50 disabled:cursor-not-allowed"
                        disabled={deleting}
                        onclick={confirmDelete}
                      >
                        {deleting ? "Deleting..." : "Delete"}
                      </button>
                      <button
                        class="text-[0.8125rem] text-[var(--text-muted)] px-3 py-1.5
                               rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
                        onclick={() => { confirmingDelete = false; menuOpen = false; }}
                      >
                        Cancel
                      </button>
                    </div>
                  </div>
                {/if}
              </div>
            </div>
          {/if}

          <div class="issue-meta-aside"><div class="issue-meta-field">{@render sidebarField("Status")}<div class="relative">
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
          </div></div><div class="issue-meta-field">{@render sidebarField("Priority")}<div class="relative">
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
          </div></div><div class="issue-meta-field">{@render sidebarField("Module")}<div class="relative">
            <button
              class="flex items-center gap-2 text-[0.8125rem] rounded-md
                     px-2 py-1 -mx-2 transition-colors w-full text-left
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
          </div></div><div class="issue-meta-field">{@render sidebarField("Labels")}<div class="relative">
            <div class="flex flex-wrap gap-1.5 items-center">
              {#if issue.labels.length > 0}
                {#each issue.labels as lbl}
                  {@const labelObj = labels.find((l) => l.name === lbl)}
                  <span
                    class="inline-flex items-center gap-1 text-[0.75rem]
                           font-medium px-2 py-0.5 rounded-full border"
                    style={labelObj
                      ? `color: ${labelObj.color}; border-color: ${labelObj.color}40; background: ${labelObj.color}10;`
                      : ""}
                  >
                    {lbl}
                    {#if editable}
                      <button
                        class="size-3 rounded-full hover:bg-[var(--bg-subtle)]
                               inline-flex items-center justify-center opacity-60
                               hover:opacity-100 transition-opacity"
                        onclick={(e) => { e.stopPropagation(); toggleLabel(lbl); }}
                        title="Remove label"
                      >
                        <X size={10} />
                      </button>
                    {/if}
                  </span>
                {/each}
              {:else}
                <span class="text-[0.8125rem] text-[var(--text-faint)]">None</span>
              {/if}

              {#if editable}
                <button
                  class="size-5 rounded border border-dashed border-[var(--border)]
                         text-[var(--text-faint)] hover:border-[var(--accent)]
                         hover:text-[var(--accent)] flex items-center justify-center
                         transition-colors"
                  title="Add label"
                  onclick={(e) => {
                    e.stopPropagation();
                    labelsOpen = !labelsOpen;
                    statusOpen = false;
                    priorityOpen = false;
                    moduleOpen = false;
                  }}
                >
                  <Plus size={12} />
                </button>
              {/if}
            </div>

            {#if labelsOpen}
              <div
                class="absolute left-0 top-full mt-1 z-20 w-[180px]
                       bg-[var(--surface)] border border-[var(--border)]
                       rounded-md shadow-lg py-1"
                role="presentation"
                onclick={(e) => e.stopPropagation()}
                onkeydown={(e) => e.stopPropagation()}
              >
                {#if labels.length === 0}
                  <div class="px-3 py-2 text-[0.8125rem] text-[var(--text-faint)]">
                    No labels defined
                  </div>
                {:else}
                  {#each labels as label}
                    {@const isAttached = issue.labels.includes(label.name)}
                    <button
                      class="w-full flex items-center gap-2 px-3 py-1.5 text-left
                             text-[0.8125rem] transition-colors
                             hover:bg-[var(--bg-subtle)]"
                      onclick={() => toggleLabel(label.name)}
                    >
                      <span
                        class="size-2.5 rounded-full shrink-0"
                        style="background: {label.color};"
                      ></span>
                      <span class="flex-1 {isAttached ? 'font-medium' : ''}">
                        {label.name}
                      </span>
                      {#if isAttached}
                        <Check size={14} class="text-[var(--accent)]" />
                      {/if}
                    </button>
                  {/each}
                {/if}
              </div>
            {/if}
          </div></div><div class="border-t border-[var(--border)] -mx-5 px-5 py-0 my-1"></div>{#if (issue.blocks && issue.blocks.length > 0) || (issue.blocked_by && issue.blocked_by.length > 0) || (issue.relates_to && issue.relates_to.length > 0)}
            <div class="issue-meta-relations">
              {#if issue.blocked_by && issue.blocked_by.length > 0}
                <div class="issue-meta-field">{@render sidebarField("Blocked by")}<div class="flex flex-wrap gap-1.5">
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
                  </div></div>
              {/if}
              {#if issue.blocks && issue.blocks.length > 0}
                <div class="issue-meta-field">{@render sidebarField("Blocks")}<div class="flex flex-wrap gap-1.5">
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
                  </div></div>
              {/if}
              {#if issue.relates_to && issue.relates_to.length > 0}
                <div class="issue-meta-field">{@render sidebarField("Related")}<div class="flex flex-wrap gap-1.5">
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
                  </div></div>
              {/if}
            </div>

            <div class="border-t border-[var(--border)] -mx-5 px-5 py-0 my-1"></div>
          {/if}<div class="issue-meta-dates">
            <div class="issue-meta-field">{@render sidebarField("Created")}<p
              class="text-[0.8125rem] text-[var(--text-muted)] leading-snug m-0"
            >
              {formatDate(issue.created_at)}
            </p></div><div class="issue-meta-field">{@render sidebarField("Updated")}<p
              class="text-[0.8125rem] text-[var(--text-muted)] leading-snug m-0"
            >
              {formatDate(issue.updated_at)}
            </p></div>
          </div></div>
        </aside>
      </div>
    </div>
  </div>
{/if}

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
