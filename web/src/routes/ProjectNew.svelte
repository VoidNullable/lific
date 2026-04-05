<script lang="ts">
  import { createProject } from "../lib/api";

  let { navigate }: { navigate: (path: string) => void } = $props();

  let name = $state("");
  let identifier = $state("");
  let description = $state("");
  let emoji = $state("");
  let saving = $state(false);
  let error = $state("");
  let identifierTouched = $state(false);

  // Auto-generate identifier from name until the user manually edits it
  $effect(() => {
    if (!identifierTouched && name) {
      identifier = name
        .toUpperCase()
        .replace(/[^A-Z0-9]+/g, "")
        .slice(0, 5);
    }
  });

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
      <svg class="size-3.5" viewBox="0 0 16 16" fill="currentColor">
        <path fill-rule="evenodd" d="M7.78 12.53a.75.75 0 0 1-1.06 0L2.47 8.28a.75.75 0 0 1 0-1.06l4.25-4.25a.75.75 0 0 1 1.06 1.06L4.81 7h7.44a.75.75 0 0 1 0 1.5H4.81l2.97 2.97a.75.75 0 0 1 0 1.06Z" clip-rule="evenodd" />
      </svg>
      Back
    </button>

    <span class="text-[var(--text-faint)]">/</span>
    <span class="text-[0.8125rem] text-[var(--text-muted)]">New project</span>

    <div class="ml-auto flex items-center gap-2">
      {#if error}
        <span class="text-[0.8125rem] text-[var(--error)]">{error}</span>
      {/if}
      <button
        class="text-[0.8125rem] text-[var(--text-muted)] px-3 py-1.5
               rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
        onclick={() => navigate("/settings")}
      >
        Cancel
      </button>
      <button
        class="text-[0.8125rem] font-medium text-[var(--accent-text)]
               bg-[var(--accent)] px-3 py-1.5 rounded-md
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
    <div class="max-w-[480px] mx-auto py-10 px-6 space-y-6">
      <div>
        <label class="block text-[0.8125rem] font-medium text-[var(--text)] mb-1.5">
          Project name
        </label>
        <input
          type="text"
          bind:value={name}
          class="w-full px-3 py-2 text-[0.875rem] rounded-md
                 border border-[var(--border)] bg-[var(--surface)]
                 text-[var(--text)]"
          placeholder="My Project"
          autofocus
        />
      </div>

      <div>
        <label class="block text-[0.8125rem] font-medium text-[var(--text)] mb-1.5">
          Identifier
        </label>
        <input
          type="text"
          bind:value={identifier}
          oninput={() => { identifierTouched = true; }}
          class="w-full px-3 py-2 text-[0.875rem] rounded-md font-mono uppercase
                 border border-[var(--border)] bg-[var(--surface)]
                 text-[var(--text)]"
          placeholder="PRO"
        />
        <p class="text-[0.75rem] text-[var(--text-faint)] mt-1">
          Short uppercase code used in issue identifiers (e.g. {identifier || "PRO"}-1)
        </p>
      </div>

      <div>
        <label class="block text-[0.8125rem] font-medium text-[var(--text)] mb-1.5">
          Description <span class="text-[var(--text-faint)] font-normal">(optional)</span>
        </label>
        <textarea
          bind:value={description}
          class="w-full px-3 py-2 text-[0.875rem] rounded-md min-h-[80px]
                 border border-[var(--border)] bg-[var(--surface)]
                 text-[var(--text)] resize-y"
          placeholder="What is this project about?"
        ></textarea>
      </div>

      <div>
        <label class="block text-[0.8125rem] font-medium text-[var(--text)] mb-1.5">
          Emoji <span class="text-[var(--text-faint)] font-normal">(optional)</span>
        </label>
        <input
          type="text"
          bind:value={emoji}
          class="w-[80px] px-3 py-2 text-[1rem] rounded-md text-center
                 border border-[var(--border)] bg-[var(--surface)]"
          placeholder="📦"
          maxlength="2"
        />
      </div>
    </div>
  </div>
</div>
