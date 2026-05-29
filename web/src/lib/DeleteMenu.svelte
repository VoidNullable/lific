<script lang="ts">
  // LIF-123 — shared delete affordance: a kebab button that opens a
  // single "Delete {noun}" menu item, which in turn opens an inline
  // confirm popover. Self-contained: owns its open/confirm/deleting
  // state and its own outside-click close, so any detail page can drop
  // it into a topbar (or sidebar) without re-implementing the dance.
  //
  // `onDelete` should perform the deletion and navigate away on success.
  // It returns `true` on success (component is unmounting, no reset
  // needed) or `false` on failure (we reset so the user can retry).

  import { Ellipsis, Trash2 } from "lucide-svelte";

  let {
    noun,
    label,
    onDelete,
    confirmBody = "This can't be undone.",
    align = "right",
  }: {
    /** Object kind shown in the menu item, e.g. "issue" / "page". */
    noun: string;
    /** Identifier or name shown in the confirm ("Delete LIF-42?"). */
    label: string;
    onDelete: () => Promise<boolean>;
    confirmBody?: string;
    align?: "left" | "right";
  } = $props();

  let menuOpen = $state(false);
  let confirming = $state(false);
  let deleting = $state(false);

  function handleWindowClick() {
    menuOpen = false;
    confirming = false;
  }

  async function run() {
    deleting = true;
    const ok = await onDelete();
    if (!ok) {
      deleting = false;
      confirming = false;
      menuOpen = false;
    }
  }
</script>

<svelte:window onclick={handleWindowClick} />

<div class="relative">
  <button
    class="size-7 flex items-center justify-center rounded-md
           text-[var(--text-faint)] hover:text-[var(--text)]
           hover:bg-[var(--bg-subtle)] transition-colors"
    title="More actions"
    onclick={(e) => {
      e.stopPropagation();
      if (confirming) { confirming = false; menuOpen = false; }
      else { menuOpen = !menuOpen; }
    }}
  >
    <Ellipsis size={14} />
  </button>

  {#if menuOpen && !confirming}
    <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
    <div
      class="absolute {align === 'right' ? 'right-0' : 'left-0'} top-full mt-1.5 z-30 w-[180px]
             bg-[var(--surface)] border border-[var(--border)]
             rounded-md shadow-lg py-1"
      onclick={(e) => e.stopPropagation()}
    >
      <button
        class="w-full flex items-center gap-2 px-3 py-1.5 text-left
               text-[0.8125rem] text-[var(--error)]
               hover:bg-[var(--error-bg)] transition-colors"
        onclick={() => { confirming = true; }}
      >
        <Trash2 size={14} />
        Delete {noun}
      </button>
    </div>
  {/if}

  {#if confirming}
    <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
    <div
      class="absolute {align === 'right' ? 'right-0' : 'left-0'} top-full mt-1.5 z-30 w-[260px]
             bg-[var(--surface)] border border-[var(--border)]
             rounded-md shadow-lg p-3"
      onclick={(e) => e.stopPropagation()}
    >
      <p class="text-[0.8125rem] text-[var(--text)] mb-1 font-medium">
        Delete {label}?
      </p>
      <p class="text-[0.75rem] text-[var(--text-muted)] mb-3">
        {confirmBody}
      </p>
      <div class="flex items-center gap-2">
        <button
          class="text-[0.8125rem] font-medium text-white
                 bg-[var(--error)] px-3 py-1.5 rounded-md
                 hover:opacity-90 transition-opacity
                 disabled:opacity-50 disabled:cursor-not-allowed"
          disabled={deleting}
          onclick={run}
        >
          {deleting ? "Deleting..." : "Delete"}
        </button>
        <button
          class="text-[0.8125rem] text-[var(--text-muted)] px-3 py-1.5
                 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
          onclick={() => { confirming = false; menuOpen = false; }}
        >
          Cancel
        </button>
      </div>
    </div>
  {/if}
</div>
