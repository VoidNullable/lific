<script lang="ts">
  import {
    listProjects,
    listModules,
    listLabels,
    createIssue,
    type Project,
    type Module,
    type Label,
  } from "../lib/api";
  import { ArrowLeft } from "lucide-svelte";
  import LabelEditor from "../lib/LabelEditor.svelte";
  import PriorityIcon from "../lib/PriorityIcon.svelte";
  import StatusIcon from "../lib/StatusIcon.svelte";
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
    defaultModuleId = null,
    defaultStatus = null,
  }: {
    navigate: (path: string) => void;
    projectIdentifier: string;
    /** LIF-121: pre-fill the module assignment from a query param so
     *  "+ Issue" on a module page lands here with that module already
     *  selected. */
    defaultModuleId?: number | null;
    /** Pre-fill status from ?status= so the board column "+" creates
     *  an issue in that column instead of silently defaulting to
     *  backlog. Validated by App's route parser. */
    defaultStatus?: string | null;
  } = $props();

  let project = $state<Project | null>(null);
  let modules = $state<Module[]>([]);
  let labels = $state<Label[]>([]);
  let loading = $state(true);
  let error = $state("");
  let saving = $state(false);

  // Draft fields
  let title = $state("");
  let description = $state("");
  let status = $state(defaultStatus ?? "backlog");
  let priority = $state("none");
  let moduleId = $state<number | null>(defaultModuleId);
  let selectedLabels = $state<string[]>([]);

  // Dropdown states
  let statusOpen = $state(false);
  let priorityOpen = $state(false);
  let moduleOpen = $state(false);
  let labelsOpen = $state(false);

  // Auto-resize
  let descriptionEl = $state<HTMLTextAreaElement | null>(null);

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
    const id = projectIdentifier;
    loadProject(id);
  });

  async function loadProject(identifier: string) {
    loading = true;
    const projRes = await listProjects();
    if (!projRes.ok) {
      error = projRes.error;
      loading = false;
      return;
    }
    const found = projRes.data.find((p: Project) => p.identifier === identifier);
    if (!found) {
      error = `Project ${identifier} not found`;
      loading = false;
      return;
    }
    project = found;

    const [modRes, lblRes] = await Promise.all([
      listModules(found.id),
      listLabels(found.id),
    ]);
    if (modRes.ok) modules = modRes.data;
    if (lblRes.ok) labels = lblRes.data;
    loading = false;
  }

  function handleWindowClick() {
    statusOpen = false;
    priorityOpen = false;
    moduleOpen = false;
    labelsOpen = false;
  }

  let canSave = $derived(title.trim().length > 0);

  async function save() {
    if (!project || !canSave) return;
    saving = true;
    error = "";

    const res = await createIssue({
      project_id: project.id,
      title: title.trim(),
      description: description,
      status,
      priority,
      module_id: moduleId ?? undefined,
      labels: selectedLabels.length > 0 ? selectedLabels : undefined,
    });

    if (res.ok) {
      navigate(`/${projectIdentifier}/issues/${res.data.identifier}`);
    } else {
      error = res.error;
      saving = false;
    }
  }

  function discard() {
    navigate(`/${projectIdentifier}/issues`);
  }

  function autoResize() {
    const el = descriptionEl;
    if (!el) return;
    el.style.height = "0";
    el.style.height = el.scrollHeight + "px";
  }

  function moduleName(id: number | null): string {
    if (!id) return "None";
    return modules.find((m) => m.id === id)?.name ?? "Unknown";
  }

  function toggleLabel(name: string) {
    const idx = selectedLabels.indexOf(name);
    if (idx >= 0) {
      selectedLabels = selectedLabels.filter((l) => l !== name);
    } else {
      selectedLabels = [...selectedLabels, name];
    }
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
{:else if !project}
  <div class="h-full flex flex-col items-center justify-center gap-3">
    <p class="text-[var(--error)] text-[0.875rem]">{error}</p>
    <button
      class="text-[0.8125rem] text-[var(--accent)] hover:underline"
      onclick={() => navigate(`/${projectIdentifier}/issues`)}
    >
      Back to issues
    </button>
  </div>
{:else}
  <div class="h-full flex flex-col">
    <!-- Content -->
    <div class="flex-1 overflow-y-auto">
      <div class="max-w-[960px] mx-auto flex gap-0 min-h-full">
        <!-- Main column -->
        <div class="flex-1 min-w-0 px-8 py-6">
          <!-- Title -->
          <input
            type="text"
            bind:value={title}
            class="w-full text-[1.5rem] font-display tracking-tight
                   bg-transparent border-none outline-none
                   text-[var(--text)] py-1 mb-4
                   placeholder:text-[var(--text-faint)]"
            placeholder="Issue title"
            autofocus
          />

          <!-- Description -->
          <section class="mb-8">
            <textarea
              bind:value={description}
              bind:this={descriptionEl}
              class="w-full text-[0.875rem] leading-[1.7] text-[var(--text)]
                     bg-transparent border-none outline-none resize-none
                     p-0 m-0 font-[var(--font-body)] min-h-[120px]"
              placeholder="Add a description... (markdown supported)"
              oninput={autoResize}
            ></textarea>
          </section>
        </div>

        <!-- Sidebar. Same issue-meta-* spacing system as IssueDetail so
             the field rhythm matches the detail page exactly (LIF-126). -->
        <aside
          class="w-[220px] shrink-0 border-l border-[var(--border)] py-6 px-5"
        >
          <div class="issue-meta-aside">
            <!-- Status -->
            <div class="issue-meta-field">
              {@render sidebarField("Status")}
              <div class="relative">
                <button
                  class="flex items-center gap-2 text-[0.8125rem] rounded-md
                         px-2 py-1 -mx-2 transition-colors w-full text-left
                         hover:bg-[var(--bg-subtle)] cursor-pointer"
                  onclick={(e) => {
                    e.stopPropagation();
                    statusOpen = !statusOpen;
                    priorityOpen = false;
                    moduleOpen = false;
                    labelsOpen = false;
                  }}
                >
                  <StatusIcon {status} size={14} />
                  <span class="capitalize text-[var(--text)]">{status}</span>
                </button>
                {#if statusOpen}
                  <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
                  <div
                    class="absolute left-0 top-full mt-1 z-20 w-[180px]
                           bg-[var(--surface)] border border-[var(--border)]
                           rounded-md shadow-lg py-1"
                    onclick={(e) => e.stopPropagation()}
                  >
                    {#each STATUSES as s}
                      <button
                        class="w-full flex items-center gap-2 px-3 py-1.5 text-left
                               text-[0.8125rem] transition-colors
                               {s.value === status
                          ? 'text-[var(--accent)] bg-[var(--accent-subtle)]'
                          : 'text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
                        onclick={() => { status = s.value; statusOpen = false; }}
                      >
                        <StatusIcon status={s.value} size={14} />
                        {s.label}
                      </button>
                    {/each}
                  </div>
                {/if}
              </div>
            </div>

            <!-- Priority -->
            <div class="issue-meta-field">
              {@render sidebarField("Priority")}
              <div class="relative">
                <button
                  class="flex items-center gap-2 text-[0.8125rem] rounded-md
                         px-2 py-1 -mx-2 transition-colors w-full text-left
                         hover:bg-[var(--bg-subtle)] cursor-pointer"
                  onclick={(e) => {
                    e.stopPropagation();
                    priorityOpen = !priorityOpen;
                    statusOpen = false;
                    moduleOpen = false;
                    labelsOpen = false;
                  }}
                >
                  <PriorityIcon {priority} />
                  <span class={priorityTextClass(priority)}>
                    {priority === "none" ? "No priority" : priority.charAt(0).toUpperCase() + priority.slice(1)}
                  </span>
                </button>
                {#if priorityOpen}
                  <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
                  <div
                    class="absolute left-0 top-full mt-1 z-20 w-[180px]
                           bg-[var(--surface)] border border-[var(--border)]
                           rounded-md shadow-lg py-1"
                    onclick={(e) => e.stopPropagation()}
                  >
                    {#each PRIORITIES as p}
                      <button
                        class="w-full flex items-center gap-2 px-3 py-1.5 text-left
                               text-[0.8125rem] transition-colors
                               {p.value === priority
                          ? 'text-[var(--accent)] bg-[var(--accent-subtle)]'
                          : 'text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
                        onclick={() => { priority = p.value; priorityOpen = false; }}
                      >
                        <PriorityIcon priority={p.value} />
                        {p.label}
                      </button>
                    {/each}
                  </div>
                {/if}
              </div>
            </div>

            <!-- Module -->
            {#if modules.length > 0}
              <div class="issue-meta-field">
                {@render sidebarField("Module")}
                <div class="relative">
                  <button
                    class="flex items-center gap-2 text-[0.8125rem] rounded-md
                           px-2 py-1 -mx-2 transition-colors w-full text-left
                           hover:bg-[var(--bg-subtle)] cursor-pointer"
                    onclick={(e) => {
                      e.stopPropagation();
                      moduleOpen = !moduleOpen;
                      statusOpen = false;
                      priorityOpen = false;
                      labelsOpen = false;
                    }}
                  >
                    <span class={moduleId ? "text-[var(--text)]" : "text-[var(--text-faint)]"}>
                      {moduleName(moduleId)}
                    </span>
                  </button>
                  {#if moduleOpen}
                    <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
                    <div
                      class="absolute left-0 top-full mt-1 z-20 w-[180px]
                             bg-[var(--surface)] border border-[var(--border)]
                             rounded-md shadow-lg py-1"
                      onclick={(e) => e.stopPropagation()}
                    >
                      <button
                        class="w-full px-3 py-1.5 text-left text-[0.8125rem]
                               text-[var(--text-faint)] hover:bg-[var(--bg-subtle)]
                               transition-colors"
                        onclick={() => { moduleId = null; moduleOpen = false; }}
                      >
                        None
                      </button>
                      {#each modules as mod}
                        <button
                          class="w-full px-3 py-1.5 text-left text-[0.8125rem]
                                 transition-colors
                                 {mod.id === moduleId
                            ? 'text-[var(--accent)] bg-[var(--accent-subtle)]'
                            : 'text-[var(--text)] hover:bg-[var(--bg-subtle)]'}"
                          onclick={() => { moduleId = mod.id; moduleOpen = false; }}
                        >
                          {mod.name}
                        </button>
                      {/each}
                    </div>
                  {/if}
                </div>
              </div>
            {/if}

            <!-- Labels (shared with IssueDetail via LabelEditor) -->
            {#if labels.length > 0}
              <div class="issue-meta-field">
                {@render sidebarField("Labels")}
                <LabelEditor
                  attached={selectedLabels}
                  all={labels}
                  onToggle={toggleLabel}
                  bind:open={labelsOpen}
                  onOpen={() => {
                    statusOpen = false;
                    priorityOpen = false;
                    moduleOpen = false;
                  }}
                />
              </div>
            {/if}
          </div>
        </aside>
      </div>
    </div>
  </div>
{/if}

{#snippet topbarContent()}
  <div class="flex items-center gap-3 px-6 py-2 w-full">
    <div class="flex items-center gap-1.5 shrink-0">
      <button
        class="flex items-center gap-1.5 text-[0.8125rem] text-[var(--text-muted)]
               hover:text-[var(--text)] transition-colors rounded px-1.5 py-0.5
               hover:bg-[var(--bg-subtle)]"
        onclick={discard}
      >
        <ArrowLeft size={14} />
        Issues
      </button>
      <span class="text-[var(--text-faint)]">/</span>
      <span class="text-[0.8125rem] text-[var(--text-muted)]">
        New issue
      </span>
    </div>

    <div class="ml-auto flex items-center gap-2 shrink-0">
      {#if error}
        <span class="text-[0.8125rem] text-[var(--error)]">{error}</span>
      {/if}
      <button
        class="text-[0.8125rem] text-[var(--text-muted)] px-2.5 py-1
               rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
        onclick={discard}
      >
        Discard
      </button>
      <button
        class="text-[0.8125rem] font-medium text-[var(--accent-text)]
               bg-[var(--accent)] px-2.5 py-1 rounded-md
               hover:bg-[var(--accent-hover)] transition-colors
               disabled:opacity-40 disabled:cursor-not-allowed"
        disabled={!canSave || saving}
        onclick={save}
      >
        {saving ? "Creating..." : "Create issue"}
      </button>
    </div>
  </div>
{/snippet}

{#snippet sidebarField(label: string)}
  <p class="issue-meta-field-label">{label}</p>
{/snippet}

<script lang="ts" module>
  function priorityTextClass(p: string): string {
    switch (p) {
      case "urgent": return "text-[var(--error)]";
      case "high": return "text-[var(--warn)]";
      case "medium": return "text-[var(--accent)]";
      default: return "text-[var(--text)]";
    }
  }
</script>
