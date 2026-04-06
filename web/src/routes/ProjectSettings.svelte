<script lang="ts">
  import {
    listProjects,
    listIssues,
    updateProject,
    deleteProject,
    type Project,
  } from "../lib/api";
  import ProjectForm from "../lib/ProjectForm.svelte";
  import { ChevronRight } from "lucide-svelte";

  let {
    navigate,
    projectIdentifier,
    onProjectChange,
  }: {
    navigate: (path: string) => void;
    projectIdentifier: string;
    onProjectChange?: () => void;
  } = $props();

  let project = $state<Project | null>(null);
  let loading = $state(true);
  let error = $state("");

  // Edit fields
  let name = $state("");
  let identifier = $state("");
  let description = $state("");
  let emoji = $state("");
  let leadUserId = $state<number | null>(null);
  let saving = $state(false);
  let saveSuccess = $state(false);

  // Delete
  let issueCount = $state(0);
  let showDeleteSection = $state(false);
  let deleteConfirmText = $state("");
  let deleting = $state(false);
  let deleteError = $state("");

  $effect(() => {
    const id = projectIdentifier;
    loadProject(id);
  });

  async function loadProject(ident: string) {
    loading = true;
    error = "";
    const projRes = await listProjects();
    if (!projRes.ok) {
      error = projRes.error;
      loading = false;
      return;
    }
    const found = projRes.data.find((p: Project) => p.identifier === ident);
    if (!found) {
      error = `Project ${ident} not found`;
      loading = false;
      return;
    }
    project = found;
    name = found.name;
    identifier = found.identifier;
    description = found.description;
    emoji = found.emoji ?? "";
    leadUserId = found.lead_user_id;

    // Fetch issue count for the delete warning
    const allRes = await listIssues({ project_id: found.id, limit: 9999 });
    if (allRes.ok) issueCount = allRes.data.length;

    loading = false;
  }

  let hasChanges = $derived(
    project != null && (
      name.trim() !== project.name ||
      identifier.trim().toUpperCase() !== project.identifier ||
      description.trim() !== project.description ||
      (emoji.trim() || "") !== (project.emoji ?? "") ||
      leadUserId !== project.lead_user_id
    )
  );

  async function saveChanges() {
    if (!project || !hasChanges) return;
    saving = true;
    saveSuccess = false;
    error = "";

    const input: Record<string, unknown> = {};
    if (name.trim() !== project.name) input.name = name.trim();
    if (identifier.trim().toUpperCase() !== project.identifier) {
      input.identifier = identifier.trim().toUpperCase();
    }
    if (description.trim() !== project.description) input.description = description.trim();
    const newEmoji = emoji.trim() || undefined;
    if (newEmoji !== (project.emoji ?? undefined)) input.emoji = newEmoji ?? "";
    if (leadUserId !== project.lead_user_id) input.lead_user_id = leadUserId;

    const res = await updateProject(project.id, input);
    if (res.ok) {
      project = res.data;
      saveSuccess = true;
      onProjectChange?.();
      if (res.data.identifier !== projectIdentifier) {
        navigate(`/${res.data.identifier}/settings`);
      }
      setTimeout(() => { saveSuccess = false; }, 2000);
    } else {
      error = res.error;
    }
    saving = false;
  }

  let deleteReady = $derived(
    project != null && deleteConfirmText === project.identifier
  );

  async function handleDelete() {
    if (!project || !deleteReady) return;
    deleting = true;
    deleteError = "";

    const res = await deleteProject(project.id);
    if (res.ok) {
      navigate("/settings");
    } else {
      deleteError = res.error;
      deleting = false;
    }
  }
</script>

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
      onclick={() => navigate("/settings")}
    >
      Back
    </button>
  </div>
{:else}
  <div class="h-full flex flex-col">
    <!-- Toolbar -->
    <div
      class="shrink-0 flex items-center gap-3 px-6 py-2.5
             border-b border-[var(--border)] bg-[var(--surface)]"
    >
      <!-- Breadcrumb -->
      <div class="flex items-center gap-1.5 shrink-0">
        <button
          class="text-[0.8125rem] font-mono font-medium text-[var(--text-muted)]
                 hover:text-[var(--text)] transition-colors"
          onclick={() => navigate(`/${project!.identifier}/issues`)}
        >
          {project!.identifier}
        </button>
        <ChevronRight size={12} class="text-[var(--text-faint)]" />
        <span class="text-[0.8125rem] font-medium text-[var(--text)]">
          Settings
        </span>
      </div>

      <div class="ml-auto flex items-center gap-2">
        {#if error}
          <span class="text-[0.8125rem] text-[var(--error)] max-w-[min(280px,40vw)] truncate" title={error}>
            {error}
          </span>
        {/if}
        {#if saveSuccess}
          <span class="text-[0.8125rem] text-[var(--success)]">Saved</span>
        {/if}
        <button
          class="text-[0.8125rem] font-medium text-[var(--accent-text)]
                 bg-[var(--accent)] px-3 py-1 rounded-md
                 hover:bg-[var(--accent-hover)] transition-colors
                 disabled:opacity-40 disabled:cursor-not-allowed"
          disabled={!hasChanges || saving}
          onclick={saveChanges}
        >
          {saving ? "Saving..." : "Save changes"}
        </button>
      </div>
    </div>

    <!-- Content -->
    <div class="flex-1 overflow-y-auto">
      <ProjectForm
        bind:name
        bind:identifier
        bind:description
        bind:emoji
        bind:leadUserId
        mode="edit"
      />

      <!-- Danger zone -->
      <div class="max-w-[560px] mx-auto px-6 pb-16">
        <div class="border-t border-[var(--border)] pt-8">
          <h2 class="text-[1rem] font-semibold text-[var(--error)] mb-1">
            Danger zone
          </h2>
          <p class="text-[0.8125rem] text-[var(--text-muted)] mb-4">
            Irreversible actions that permanently destroy data.
          </p>

          {#if !showDeleteSection}
            <button
              class="text-[0.8125rem] text-[var(--error)] border border-[var(--error)]
                     px-4 py-2 rounded-md hover:bg-[var(--error-bg)] transition-colors"
              onclick={() => { showDeleteSection = true; }}
            >
              Delete this project
            </button>
          {:else}
            <div
              class="border border-[var(--error)] rounded-md p-5 bg-[var(--error-bg)]"
            >
              <h3 class="text-[0.9375rem] font-semibold text-[var(--error)] mb-2">
                Delete {project.name}
              </h3>
              <p class="text-[0.8125rem] text-[var(--text)] mb-1">
                This will permanently delete:
              </p>
              <ul class="text-[0.8125rem] text-[var(--text)] mb-4 list-disc pl-5 space-y-0.5">
                <li>The project <strong>{project.name}</strong> ({project.identifier})</li>
                <li>All <strong>{issueCount}</strong> issue{issueCount !== 1 ? "s" : ""} and their comments</li>
                <li>All modules, labels, and folders</li>
                <li>All pages within this project</li>
              </ul>
              <p class="text-[0.8125rem] text-[var(--text)] mb-3">
                Type <strong class="font-mono">{project.identifier}</strong> to confirm:
              </p>
              <input
                type="text"
                bind:value={deleteConfirmText}
                class="w-full px-3 py-2 text-[0.875rem] font-mono rounded-md
                       border border-[var(--error)] bg-[var(--surface)]
                       text-[var(--text)] mb-3
                       focus:shadow-[0_0_0_3px_var(--error-bg)]"
                placeholder={project.identifier}
              />
              {#if deleteError}
                <p class="text-[0.8125rem] text-[var(--error)] mb-3">{deleteError}</p>
              {/if}
              <div class="flex items-center gap-3">
                <button
                  class="text-[0.875rem] font-medium text-white
                         bg-[var(--error)] px-4 py-2 rounded-md
                         hover:opacity-90 transition-opacity
                         disabled:opacity-40 disabled:cursor-not-allowed"
                  disabled={!deleteReady || deleting}
                  onclick={handleDelete}
                >
                  {deleting ? "Deleting..." : "Permanently delete project"}
                </button>
                <button
                  class="text-[0.8125rem] text-[var(--text-muted)] px-3 py-2
                         rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
                  onclick={() => { showDeleteSection = false; deleteConfirmText = ""; deleteError = ""; }}
                >
                  Cancel
                </button>
              </div>
            </div>
          {/if}
        </div>
      </div>
    </div>
  </div>
{/if}
