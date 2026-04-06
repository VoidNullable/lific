<script lang="ts">
  import {
    listProjects,
    listIssues,
    listUsers,
    updateProject,
    deleteProject,
    type Project,
    type UserSummary,
  } from "../lib/api";
  import IconPicker from "../lib/IconPicker.svelte";
  import Select from "../lib/Select.svelte";
  import { ArrowLeft } from "lucide-svelte";

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
  let users = $state<UserSummary[]>([]);

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

    // Load users for lead dropdown
    const usersRes = await listUsers();
    if (usersRes.ok) users = usersRes.data;

    // Fetch issue count for the delete warning
    const issueRes = await listIssues({ project_id: found.id, limit: 1 });
    if (issueRes.ok) {
      // The API doesn't return a total count, so fetch with high limit
      const allRes = await listIssues({ project_id: found.id, limit: 9999 });
      if (allRes.ok) issueCount = allRes.data.length;
    }

    loading = false;
  }

  let userOptions = $derived([
    { value: null, label: "No lead" },
    ...users.map((u) => ({
      value: u.id,
      label: u.display_name || u.username,
      username: u.username,
      is_admin: u.is_admin,
      created_at: u.created_at,
    })),
  ]);

  function formatMemberSince(iso: string): string {
    const d = new Date(iso + "Z");
    return d.toLocaleDateString("en-US", { month: "short", year: "numeric" });
  }

  function userInitials(name: string): string {
    return name.split(/[\s_-]+/).slice(0, 2).map((w) => w[0]?.toUpperCase() ?? "").join("");
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
      // If identifier changed, update the URL
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
    <!-- Top bar -->
    <div
      class="shrink-0 flex items-center gap-3 px-6 py-2.5
             border-b border-[var(--border)] bg-[var(--surface)]"
    >
      <button
        class="flex items-center gap-1.5 text-[0.8125rem] text-[var(--text-muted)]
               hover:text-[var(--text)] transition-colors rounded px-1.5 py-0.5
               hover:bg-[var(--bg-subtle)]"
        onclick={() => navigate(`/${project!.identifier}/issues`)}
      >
        <ArrowLeft size={14} />
        {project!.name}
      </button>

      <span class="text-[var(--text-faint)]">/</span>
      <span class="text-[0.8125rem] text-[var(--text-muted)]">Project settings</span>
    </div>

    <!-- Content -->
    <div class="flex-1 overflow-y-auto">
      <div class="max-w-[520px] mx-auto py-10 px-6">
        <h1 class="font-display text-[1.375rem] text-[var(--text)] mb-8">
          Project settings
        </h1>

        <!-- Edit form -->
        <div class="space-y-5 mb-10">
          <div>
            <label class="block text-[0.8125rem] font-medium text-[var(--text)] mb-1.5">
              Name
            </label>
            <input
              type="text"
              bind:value={name}
              class="w-full px-3 py-2 text-[0.875rem] rounded-md
                     border border-[var(--border)] bg-[var(--surface)]
                     text-[var(--text)]"
            />
          </div>

          <div>
            <label class="block text-[0.8125rem] font-medium text-[var(--text)] mb-1.5">
              Identifier
            </label>
            <input
              type="text"
              bind:value={identifier}
              maxlength="5"
              class="w-[120px] px-3 py-2 text-[0.875rem] rounded-md font-mono uppercase
                     border border-[var(--border)] bg-[var(--surface)]
                     text-[var(--text)]"
            />
            <p class="text-[0.75rem] text-[var(--text-faint)] mt-1">
              Changing this will update all issue identifiers.
            </p>
          </div>

          <div>
            <label class="block text-[0.8125rem] font-medium text-[var(--text)] mb-1.5">
              Description
            </label>
            <textarea
              bind:value={description}
              class="w-full px-3 py-2 text-[0.875rem] rounded-md min-h-[80px]
                     border border-[var(--border)] bg-[var(--surface)]
                     text-[var(--text)] resize-y"
            ></textarea>
          </div>

          <div class="flex gap-4 items-start">
            <div class="flex-1">
              <label class="block text-[0.8125rem] font-medium text-[var(--text)] mb-1.5">
                Lead
              </label>
              <Select
                options={userOptions}
                bind:value={leadUserId}
                placeholder="No lead"
                renderSelected={selectedUser}
                renderOption={userOption}
              />
            </div>
            <div>
              <label class="block text-[0.8125rem] font-medium text-[var(--text)] mb-1.5">
                Icon
              </label>
              <IconPicker value={emoji} onchange={(v) => { emoji = v; }} />
            </div>
          </div>

          <div class="flex items-center gap-3 pt-2">
            <button
              class="text-[0.875rem] font-medium text-[var(--accent-text)]
                     bg-[var(--accent)] px-4 py-2 rounded-md
                     hover:bg-[var(--accent-hover)] transition-colors
                     disabled:opacity-40 disabled:cursor-not-allowed"
              disabled={!hasChanges || saving}
              onclick={saveChanges}
            >
              {saving ? "Saving..." : "Save changes"}
            </button>
            {#if saveSuccess}
              <span class="text-[0.8125rem] text-[var(--success)]">Saved</span>
            {/if}
            {#if error}
              <span class="text-[0.8125rem] text-[var(--error)]">{error}</span>
            {/if}
          </div>
        </div>

        <!-- Danger zone -->
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

{#snippet selectedUser(opt: { value: string | number | null; label: string; [key: string]: unknown })}
  <div class="flex items-center gap-2">
    {#if opt.value !== null}
      <div
        class="size-5 rounded-full bg-[var(--accent)] text-[var(--accent-text)]
               flex items-center justify-center text-[0.5625rem] font-semibold shrink-0"
      >
        {userInitials(opt.label)}
      </div>
    {/if}
    <span class="text-[0.875rem] text-[var(--text)]">{opt.label}</span>
  </div>
{/snippet}

{#snippet userOption(opt: { value: string | number | null; label: string; [key: string]: unknown }, isSelected: boolean)}
  {#if opt.value === null}
    <span class="text-[0.875rem] text-[var(--text-faint)]">{opt.label}</span>
  {:else}
    <div class="flex items-center gap-2.5">
      <div
        class="size-7 rounded-full flex items-center justify-center
               text-[0.625rem] font-semibold shrink-0
               {isSelected
          ? 'bg-[var(--accent)] text-[var(--accent-text)]'
          : 'bg-[var(--bg-subtle)] text-[var(--text-muted)]'}"
      >
        {userInitials(opt.label)}
      </div>
      <div class="min-w-0">
        <div class="flex items-center gap-1.5">
          <span
            class="text-[0.875rem] truncate
                   {isSelected ? 'text-[var(--accent)] font-medium' : 'text-[var(--text)]'}"
          >
            {opt.label}
          </span>
          {#if opt.is_admin}
            <span
              class="text-[0.625rem] font-semibold uppercase tracking-wide
                     px-1 py-0.5 rounded bg-[var(--accent-subtle)] text-[var(--accent)]"
            >
              Admin
            </span>
          {/if}
        </div>
        <span class="text-[0.75rem] text-[var(--text-faint)]">
          Member since {formatMemberSince(opt.created_at as string)}
        </span>
      </div>
    </div>
  {/if}
{/snippet}
