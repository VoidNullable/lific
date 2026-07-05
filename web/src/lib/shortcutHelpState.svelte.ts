// LIF-245 — Shortcut Help overlay state. Mirrors peek.svelte.ts: a module
// singleton so the "?" affordance (sidebar footer, issue-list topbar, and
// the global keydown binding in Layout.svelte) can all open the same
// overlay without prop-threading, and lib/shortcuts.ts's
// `shortcutsSuppressed()` can read whether it's open without importing the
// component itself.

class ShortcutHelpState {
  open = $state(false);
}

export const shortcutHelpState = new ShortcutHelpState();

export function openShortcutHelp(): void {
  shortcutHelpState.open = true;
}

export function closeShortcutHelp(): void {
  shortcutHelpState.open = false;
}

export function toggleShortcutHelp(): void {
  shortcutHelpState.open = !shortcutHelpState.open;
}
