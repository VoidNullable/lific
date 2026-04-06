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
  import { ArrowLeft, Plus, X, Check } from "lucide-svelte";

  let {
    navigate,
    projectIdentifier,
  }: {
    navigate: (path: string) => void;
    projectIdentifier: string;
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
  let status = $state("backlog");
  let priority = $state("none");
  let moduleId = $state<number | null>(null);
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
    <!-- Top bar -->
    <div
      class="shrink-0 flex items-center gap-3 px-6 py-2.5
             border-b border-[var(--border)] bg-[var(--surface)]"
    >
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

      <div class="ml-auto flex items-center gap-2">
        {#if error}
          <span class="text-[0.8125rem] text-[var(--error)]">{error}</span>
        {/if}
        <button
          class="text-[0.8125rem] text-[var(--text-muted)] px-3 py-1.5
                 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
          onclick={discard}
        >
          Discard
        </button>
        <button
          class="text-[0.8125rem] font-medium text-[var(--accent-text)]
                 bg-[var(--accent)] px-3 py-1.5 rounded-md
                 hover:bg-[var(--accent-hover)] transition-colors
                 disabled:opacity-40 disabled:cursor-not-allowed"
          disabled={!canSave || saving}
          onclick={save}
        >
          {saving ? "Creating..." : "Create issue"}
        </button>
      </div>
    </div>

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

        <!-- Sidebar -->
        <aside
          class="w-[220px] shrink-0 border-l border-[var(--border)]
                 py-6 px-5 space-y-5"
        >
          <!-- Status -->
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
              <span class="size-2.5 rounded-full {statusDotClass(status)}"></span>
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
                    <span class="size-2 rounded-full {statusDotClass(s.value)}"></span>
                    {s.label}
                  </button>
                {/each}
              </div>
            {/if}
          </div>

          <!-- Priority -->
          {@render sidebarField("Priority")}
          <div class="relative -mt-4">
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
              <span class="text-[var(--text)] {priorityTextClass(priority)}">
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
                    {p.label}
                  </button>
                {/each}
              </div>
            {/if}
          </div>

          <!-- Module -->
          {#if modules.length > 0}
            {@render sidebarField("Module")}
            <div class="relative -mt-4">
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
                <span class="text-[var(--text)] {moduleId ? '' : 'text-[var(--text-faint)]'}">
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
          {/if}

          <!-- Labels -->
          {#if labels.length > 0}
            {@render sidebarField("Labels")}
            <div class="relative -mt-4">
              <div class="flex flex-wrap gap-1.5 items-center">
                {#if selectedLabels.length > 0}
                  {#each selectedLabels as lbl}
                    {@const labelObj = labels.find((l) => l.name === lbl)}
                    <span
                      class="inline-flex items-center gap-1 text-[0.75rem]
                             font-medium px-2 py-0.5 rounded-full border"
                      style={labelObj
                        ? `color: ${labelObj.color}; border-color: ${labelObj.color}40; background: ${labelObj.color}10;`
                        : ""}
                    >
                      {lbl}
                      <button
                        class="size-3 rounded-full hover:bg-[var(--bg-subtle)]
                               inline-flex items-center justify-center opacity-60
                               hover:opacity-100 transition-opacity"
                        onclick={(e) => { e.stopPropagation(); toggleLabel(lbl); }}
                        title="Remove label"
                      >
                        <X size={10} />
                      </button>
                    </span>
                  {/each}
                {:else}
                  <span class="text-[0.8125rem] text-[var(--text-faint)]">None</span>
                {/if}

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
              </div>

              {#if labelsOpen}
                <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
                <div
                  class="absolute left-0 top-full mt-1 z-20 w-[180px]
                         bg-[var(--surface)] border border-[var(--border)]
                         rounded-md shadow-lg py-1"
                  onclick={(e) => e.stopPropagation()}
                >
                  {#each labels as label}
                    {@const isAttached = selectedLabels.includes(label.name)}
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
                </div>
              {/if}
            </div>
          {/if}
        </aside>
      </div>
    </div>
  </div>
{/if}

{#snippet sidebarField(label: string)}
  <p
    class="text-[0.6875rem] font-semibold uppercase tracking-widest
           text-[var(--text-faint)] mb-1"
  >
    {label}
  </p>
{/snippet}

<script lang="ts" module>
  function statusDotClass(s: string): string {
    switch (s) {
      case "backlog": return "bg-[var(--text-faint)]";
      case "todo": return "bg-[var(--text-muted)]";
      case "active": return "bg-[var(--accent)]";
      case "done": return "bg-[var(--success)]";
      case "cancelled": return "bg-[var(--text-faint)]";
      default: return "bg-[var(--text-faint)]";
    }
  }

  function priorityTextClass(p: string): string {
    switch (p) {
      case "urgent": return "text-[var(--error)]";
      case "high": return "text-orange-500";
      case "medium": return "text-[var(--accent)]";
      default: return "";
    }
  }
</script>
