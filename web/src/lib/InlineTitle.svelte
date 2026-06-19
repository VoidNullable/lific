<script lang="ts">
  // LIF-123 — click-to-edit document title, shared by IssueDetail,
  // PageDetail and ModuleDetail. Owns its own draft/editing state and
  // commits via `onSave` (which only fires when the trimmed value
  // actually changed). Enter / blur / Ctrl+S commit; Esc cancels.
  //
  // `size` controls type scale + bottom margin to match each surface:
  //   md → 1.5rem  (issue)
  //   lg → 1.75rem (page / module)

  let {
    value,
    editable = true,
    size = "md",
    onSave,
  }: {
    value: string;
    editable?: boolean;
    size?: "md" | "lg";
    onSave: (next: string) => Promise<void> | void;
  } = $props();

  // Full class strings so Tailwind's scanner keeps them.
  const sizeClass = $derived(
    size === "lg" ? "text-display mb-3" : "text-title mb-4",
  );

  let editing = $state(false);
  let draft = $state("");

  function start() {
    if (!editable) return;
    draft = value;
    editing = true;
  }

  async function commit() {
    editing = false;
    const trimmed = draft.trim();
    if (trimmed && trimmed !== value) {
      await onSave(trimmed);
    }
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      commit();
    } else if (e.key === "Escape") {
      e.preventDefault();
      editing = false;
    } else if ((e.ctrlKey || e.metaKey) && e.key === "s") {
      e.preventDefault();
      commit();
    }
  }
</script>

{#if editing}
  <!-- svelte-ignore a11y_autofocus -->
  <input
    type="text"
    bind:value={draft}
    class="w-full {sizeClass} font-display tracking-tight
           bg-transparent border-0 border-b-2 border-solid
           border-b-[var(--accent)] outline-none
           text-[var(--text)] py-1"
    onblur={commit}
    onkeydown={onKey}
    autofocus
  />
{:else if editable}
  <button
    type="button"
    class="{sizeClass} font-display tracking-tight text-[var(--text)]
           py-1 rounded transition-colors w-full text-left
           bg-transparent border-0 cursor-text hover:bg-[var(--bg-subtle)]"
    onclick={start}
    onkeydown={(e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        start();
      }
    }}
  >
    {value}
  </button>
{:else}
  <h1
    class="{sizeClass} font-display tracking-tight text-[var(--text)] py-1"
  >
    {value}
  </h1>
{/if}
