<script lang="ts">
  // LIF-245 — Shortcut Help overlay. Renders straight from the
  // lib/shortcuts.ts registry, grouped by scope, so it can't drift from
  // what the handlers actually bind. Mounted once in Layout.svelte
  // (alongside CommandPalette) so it's reachable from anywhere: the global
  // "?" keydown binding, the sidebar footer's "?" button, and the issue
  // list topbar's Shortcuts button all call the same open function.
  //
  // Styling deliberately mirrors CommandPalette.svelte's modal (same
  // scrim, corner radius, shadow, border) so the two overlays read as one
  // family rather than two different modal languages.
  import { shortcutHelpState, closeShortcutHelp } from "./shortcutHelpState.svelte";
  import { SHORTCUTS, SCOPE_ORDER, SCOPE_LABEL } from "./shortcuts";
  import { X } from "lucide-svelte";

  let grouped = $derived(
    SCOPE_ORDER.map((scope) => ({
      scope,
      label: SCOPE_LABEL[scope],
      entries: SHORTCUTS.filter((s) => s.scope === scope),
    })).filter((g) => g.entries.length > 0),
  );

  // Esc closes. Guarded on `shortcutHelpState.open` the same way
  // PeekPanel/CommandPalette guard their own Esc handlers, so this
  // permanently-mounted listener is a no-op while the overlay is closed.
  function handleKeydown(e: KeyboardEvent) {
    if (!shortcutHelpState.open) return;
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      closeShortcutHelp();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if shortcutHelpState.open}
  <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
  <div
    class="fixed inset-0 z-[100] bg-black/25 flex items-end justify-center
           sm:items-start sm:pt-[10dvh] sm:px-4"
    onclick={closeShortcutHelp}
  >
    <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
    <!-- LIF-227: bottom sheet on phones. Read-only reference content, no
         input, so nothing here fights the keyboard. -->
    <div
      class="w-full max-h-[85dvh] flex flex-col bg-[var(--surface)]
             border-t border-[var(--border)] rounded-t-2xl
             shadow-[0_-8px_48px_rgba(0,0,0,0.28)] overflow-hidden
             sm:max-w-[560px] sm:max-h-[76dvh] sm:border sm:rounded-xl
             sm:shadow-[0_16px_48px_rgba(0,0,0,0.28)]"
      onclick={(e) => e.stopPropagation()}
      role="dialog"
      aria-modal="true"
      aria-label="Keyboard shortcuts"
    >
      <div class="sm:hidden flex justify-center pt-2 pb-0.5 shrink-0">
        <div class="h-1 w-9 rounded-full bg-[var(--border)]"></div>
      </div>
      <div class="flex items-center gap-2.5 px-4 py-3 border-b border-[var(--border)] shrink-0">
        <span class="flex-1 text-body-lg font-medium text-[var(--text)]">
          Keyboard shortcuts
        </span>
        <kbd
          class="px-1.5 py-0.5 rounded border border-[var(--border)]
                 bg-[var(--bg-subtle)] text-[var(--text-faint)]
                 font-mono text-micro leading-none shrink-0"
        >
          esc
        </kbd>
        <button
          class="size-11 sm:size-6 flex items-center justify-center rounded-md
                 text-[var(--text-faint)] hover:text-[var(--text)]
                 hover:bg-[var(--bg-subtle)] transition-colors"
          aria-label="Close"
          onclick={closeShortcutHelp}
        >
          <X size={14} />
        </button>
      </div>

      <div
        class="flex-1 min-h-0 overflow-y-auto overscroll-contain p-4
               grid sm:grid-cols-2 gap-x-6 gap-y-5
               pb-[max(1rem,env(safe-area-inset-bottom))] sm:pb-4"
      >
        {#each grouped as group (group.scope)}
          <div>
            <div
              class="text-micro font-semibold uppercase tracking-widest
                     text-[var(--text-faint)] mb-2"
            >
              {group.label}
            </div>
            <ul class="space-y-1.5">
              {#each group.entries as entry (entry.scope + entry.label + entry.keys)}
                <li class="flex items-center justify-between gap-3 text-body-sm">
                  <span class="text-[var(--text-muted)]">{entry.label}</span>
                  <span class="flex items-center gap-1 shrink-0">
                    {#each entry.keys.split(" ") as chip (chip)}
                      <kbd
                        class="px-1.5 py-0.5 rounded border border-[var(--border)]
                               bg-[var(--bg-subtle)] text-[var(--text)]
                               font-mono text-micro leading-none"
                      >
                        {chip}
                      </kbd>
                    {/each}
                  </span>
                </li>
              {/each}
            </ul>
          </div>
        {/each}
      </div>
    </div>
  </div>
{/if}
