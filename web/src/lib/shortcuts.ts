// LIF-245 — single source of truth for every keyboard shortcut in the app.
//
// Two jobs:
//   1. `SHORTCUTS` is the data the Shortcut Help overlay (ShortcutHelp.svelte)
//      renders from, grouped by `scope`. Keeping the list here — rather than
//      hand-rolled markup duplicated inside a popover — means the overlay
//      can't silently drift from what the handlers actually bind. When you
//      add or change a binding in IssueList.svelte / Layout.svelte /
//      CommandPalette.svelte, update the matching entry here too.
//   2. `shortcutsSuppressed()` centralizes "should a shortcut fire right
//      now" — focus is in a text field, or a modal/overlay that owns its
//      own keyboard input (peek panel, command palette) is open. Handlers
//      that used to roll their own ad hoc guard (IssueList's
//      `isInputFocused`, DocumentDetail's `inField`, the peek-open
//      early-return) call this instead so the rule can't drift between
//      call sites either.

import { peekState } from "./issues/peek.svelte";
import { commandPaletteState } from "./commandPaletteState.svelte";
import { shortcutHelpState } from "./shortcutHelp.svelte";
import { contextMenuState } from "./contextMenu.svelte"; // LIF-248

export type ShortcutScope = "global" | "list" | "board" | "peek" | "palette";

export interface ShortcutEntry {
  /** Display form. Space-separated tokens render as separate <kbd> chips,
   *  e.g. "J ↓" → two chips, "⌘ K" → two chips. */
  keys: string;
  label: string;
  scope: ShortcutScope;
}

export const SCOPE_ORDER: ShortcutScope[] = ["global", "list", "board", "peek", "palette"];

export const SCOPE_LABEL: Record<ShortcutScope, string> = {
  global: "Global",
  list: "Issue list",
  board: "Board",
  peek: "Peek panel",
  palette: "Command palette",
};

export const SHORTCUTS: ShortcutEntry[] = [
  // ── Global (Layout.svelte) ───────────────────────────────
  { keys: "⌘ K", label: "Open command palette", scope: "global" },
  { keys: "⌘ P", label: "Open command palette", scope: "global" },
  { keys: "?", label: "Show this shortcut list", scope: "global" },
  { keys: "Esc", label: "Close the open dialog", scope: "global" },

  // ── Issue list (IssueList.svelte handleKeydown) ──────────
  { keys: "J ↓", label: "Move focus down", scope: "list" },
  { keys: "K ↑", label: "Move focus up", scope: "list" },
  { keys: "Home", label: "Focus the first row", scope: "list" },
  { keys: "End", label: "Focus the last row", scope: "list" },
  { keys: "Enter", label: "Open the focused issue", scope: "list" },
  { keys: "Space", label: "Peek the focused issue", scope: "list" },
  { keys: "X", label: "Toggle selection on the focused row", scope: "list" },
  { keys: "⇧ J/K", label: "Extend selection", scope: "list" },
  { keys: "S", label: "Open the status picker", scope: "list" },
  { keys: "⇧ S", label: "Cycle status", scope: "list" },
  { keys: "P", label: "Open the priority picker", scope: "list" },
  { keys: "⇧ P", label: "Cycle priority", scope: "list" },
  { keys: "M", label: "Open the module picker", scope: "list" },
  { keys: "C", label: "New issue", scope: "list" },
  { keys: "/", label: "Focus search", scope: "list" },
  { keys: "Esc", label: "Clear focus / selection / close a popover", scope: "list" },

  // ── Board (IssueList.svelte, layout="board") ─────────────
  { keys: "Click", label: "Open card", scope: "board" },
  { keys: "⌘/Ctrl Click", label: "Peek card", scope: "board" },
  { keys: "Drag", label: "Move between columns and lanes", scope: "board" },
  { keys: "C", label: "New issue", scope: "board" },
  { keys: "/", label: "Focus search", scope: "board" },

  // ── Peek panel (PeekPanel.svelte) ─────────────────────────
  { keys: "Esc", label: "Close preview", scope: "peek" },

  // ── Command palette (CommandPalette.svelte) ──────────────
  { keys: "↓ ↑", label: "Move selection", scope: "palette" },
  { keys: "Enter", label: "Open / run", scope: "palette" },
  { keys: "⌫", label: "Step back out of a submenu", scope: "palette" },
  { keys: "Esc", label: "Close", scope: "palette" },
];

/** True when focus is in something that owns its own text input — a
 *  shortcut must not hijack a keystroke meant for the field. Shared by
 *  every keydown handler in the app (previously duplicated per-route). */
export function isTypingContext(el: Element | null = document.activeElement): boolean {
  if (!el) return false;
  const tag = el.tagName;
  return (
    tag === "INPUT" ||
    tag === "TEXTAREA" ||
    tag === "SELECT" ||
    (el as HTMLElement).isContentEditable
  );
}

/** True when no list/board/global shortcut should fire: typing in a field,
 *  or a modal that owns its own keyboard handling is open (the peek panel,
 *  the command palette, the shortcut help overlay, or — LIF-248 — the
 *  right-click context menu, whose own arrow/Enter/Esc handling would
 *  otherwise double-fire alongside a list's j/k row navigation). Deliberately
 *  NOT used by the "?" toggle handler in Layout.svelte — that one needs to
 *  keep firing while the help overlay is already open (so a second "?"
 *  press can close it) and checks the typing/peek/palette/context-menu
 *  conditions directly instead. */
export function shortcutsSuppressed(): boolean {
  return (
    isTypingContext() ||
    peekState.open ||
    commandPaletteState.open ||
    shortcutHelpState.open ||
    contextMenuState.open
  );
}
