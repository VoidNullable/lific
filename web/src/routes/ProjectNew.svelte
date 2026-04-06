<script lang="ts">
  import { createProject } from "../lib/api";
  import ProjectForm from "../lib/ProjectForm.svelte";
  import { ArrowLeft } from "lucide-svelte";

  let { navigate }: { navigate: (path: string) => void } = $props();

  let name = $state("");
  let identifier = $state("");
  let description = $state("");
  let emoji = $state("");
  let leadUserId = $state<number | null>(null);
  let saving = $state(false);
  let error = $state("");

  let canSave = $derived(name.trim().length > 0 && identifier.trim().length > 0);

  async function save() {
    if (!canSave) return;
    saving = true;
    error = "";

    const res = await createProject({
      name: name.trim(),
      identifier: identifier.trim().toUpperCase(),
      description: description.trim() || undefined,
      emoji: emoji.trim() || undefined,
      lead_user_id: leadUserId ?? undefined,
    });

    if (res.ok) {
      navigate(`/${res.data.identifier}/issues`);
    } else {
      error = res.error;
      saving = false;
    }
  }
</script>

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
      onclick={() => navigate("/settings")}
    >
      <ArrowLeft size={14} />
      Back
    </button>

    <span class="text-[0.8125rem] font-medium text-[var(--text)]">New project</span>

    <div class="ml-auto flex items-center gap-2">
      {#if error}
        <span class="text-[0.8125rem] text-[var(--error)] max-w-[min(280px,40vw)] truncate" title={error}>
          {error}
        </span>
      {/if}
      <button
        class="text-[0.8125rem] text-[var(--text-muted)] px-3 py-1
               rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
        onclick={() => navigate("/settings")}
      >
        Cancel
      </button>
      <button
        class="text-[0.8125rem] font-medium text-[var(--accent-text)]
               bg-[var(--accent)] px-3 py-1 rounded-md
               hover:bg-[var(--accent-hover)] transition-colors
               disabled:opacity-40 disabled:cursor-not-allowed"
        disabled={!canSave || saving}
        onclick={save}
      >
        {saving ? "Creating..." : "Create project"}
      </button>
    </div>
  </div>

  <!-- Form -->
  <div class="flex-1 overflow-y-auto">
    <ProjectForm
      bind:name
      bind:identifier
      bind:description
      bind:emoji
      bind:leadUserId
      mode="create"
    />
  </div>
</div>
