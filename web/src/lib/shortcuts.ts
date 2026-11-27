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
import { shortcutHelpState } from "./shortcutHelpState.svelte";
import { contextMenuState } from "./contextMenuState.svelte"; // LIF-248
import { mobileNavState } from "./mobileNavState.svelte";

export type ShortcutScope = "global" | "list" | "board" | "pages" | "inbox" | "detail" | "peek" | "palette" | "editor";

export interface ShortcutEntry {
  /** Display form. Space-separated tokens render as separate <kbd> chips,
   *  e.g. "J ↓" → two chips, "⌘ K" → two chips. */
  keys: string;
  label: string;
  scope: ShortcutScope;
}

export const SCOPE_ORDER: ShortcutScope[] = ["global", "list", "board", "pages", "inbox", "detail", "peek", "palette", "editor"];

export const SCOPE_LABEL: Record<ShortcutScope, string> = {
  global: "Global",
  list: "Issue list",
  board: "Board",
  pages: "Pages",
  inbox: "Inbox",
  detail: "Issue and page view",
  peek: "Peek panel",
  palette: "Command palette",
  editor: "Markdown editor",
};

export const SHORTCUTS: ShortcutEntry[] = [
  // ── Global (Layout.svelte) ───────────────────────────────
  { keys: "⌘ K", label: "Open command palette", scope: "global" },
  { keys: "⌘ P", label: "Open command palette", scope: "global" },
  { keys: "⌘ \\", label: "Collapse or expand the sidebar", scope: "global" },
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
  { keys: "Ctrl/⌘ A", label: "Select all visible issues", scope: "list" },
  { keys: "⇧ J/K", label: "Extend selection", scope: "list" },
  { keys: "S", label: "Open the status picker", scope: "list" },
  { keys: "⇧ S", label: "Cycle status", scope: "list" },
  { keys: "P", label: "Open the priority picker", scope: "list" },
  { keys: "⇧ P", label: "Cycle priority", scope: "list" },
  { keys: "M", label: "Open the module picker", scope: "list" },
  { keys: "C", label: "New issue", scope: "list" },
  { keys: "Enter", label: "In the composer: create and start the next one", scope: "list" },
  { keys: "⌘ Enter", label: "In the composer: create and open it", scope: "list" },
  { keys: "F", label: "Open filters", scope: "list" },
  { keys: "/", label: "Focus search", scope: "list" },
  { keys: "Esc", label: "Clear focus / selection / close a popover", scope: "list" },

  // ── Board (IssueList.svelte, layout="board") ─────────────
  { keys: "Click", label: "Open card", scope: "board" },
  { keys: "⌘/Ctrl Click", label: "Peek card", scope: "board" },
  { keys: "Drag", label: "Move between columns and lanes", scope: "board" },
  { keys: "C", label: "New issue", scope: "board" },
  { keys: "F", label: "Open filters", scope: "board" },
  { keys: "/", label: "Focus search", scope: "board" },

  // ── Pages (PageList.svelte onKeydown) ────────────────────
  { keys: "J ↓", label: "Next page", scope: "pages" },
  { keys: "K ↑", label: "Previous page", scope: "pages" },
  { keys: "Enter", label: "Open the focused page", scope: "pages" },
  { keys: "Space", label: "Peek the focused page (without the reading pane)", scope: "pages" },
  { keys: "P", label: "Pin or unpin", scope: "pages" },
  { keys: "X", label: "Select or deselect", scope: "pages" },
  { keys: "Ctrl/⌘ A", label: "Select every page shown", scope: "pages" },
  { keys: "C", label: "New page", scope: "pages" },
  { keys: "F", label: "Open filters", scope: "pages" },
  { keys: "/", label: "Focus search", scope: "pages" },
  { keys: "Right-click", label: "Page actions (press and hold on touch)", scope: "pages" },
  { keys: "Esc", label: "Close a popover, clear the selection or the preview", scope: "pages" },

  // ── Issue and page view (DocumentDetail.svelte, IssueDetail.svelte) ──
  { keys: "E", label: "Edit the description", scope: "detail" },
  { keys: "C", label: "Write a comment", scope: "detail" },
  { keys: "S", label: "Change status (issues)", scope: "detail" },
  { keys: "P", label: "Change priority (issues)", scope: "detail" },
  { keys: "M", label: "Change module (issues)", scope: "detail" },
  { keys: "L", label: "Edit labels (issues)", scope: "detail" },
  { keys: "D", label: "Set the due date (issues)", scope: "detail" },
  { keys: "Esc", label: "Back to the list", scope: "detail" },

  // ── Peek panel (PeekPanel.svelte) ─────────────────────────
  { keys: "Esc", label: "Close preview", scope: "peek" },

  // ── Command palette (CommandPalette.svelte) ──────────────
  { keys: "↓ ↑", label: "Move selection", scope: "palette" },
  { keys: "Enter", label: "Open / run", scope: "palette" },
  { keys: "⌘ Enter", label: "Open in a new tab", scope: "palette" },
  { keys: "⌫", label: "Step back out of a submenu", scope: "palette" },
  { keys: "Esc", label: "Close", scope: "palette" },

  // ── Markdown editor (editor/cm/index.ts) ──
  { keys: "⌘ B", label: "Bold", scope: "editor" },
  { keys: "⌘ I", label: "Italic", scope: "editor" },
  { keys: "⌘ E", label: "Inline code", scope: "editor" },
  { keys: "⌘ ⇧ X", label: "Strikethrough", scope: "editor" },
  { keys: "⌘ ⇧ K", label: "Insert link", scope: "editor" },
  { keys: "⌘ Enter", label: "Tick or untick the task on this line", scope: "editor" },
  // ── Inbox (Inbox.svelte onKeydown) ──────────────────────
  { keys: "J ↓", label: "Next issue", scope: "inbox" },
  { keys: "K ↑", label: "Previous issue", scope: "inbox" },
  { keys: "Enter O", label: "Open the issue", scope: "inbox" },
  { keys: "U", label: "Mark read or unread", scope: "inbox" },
  { keys: "E", label: "Mark done", scope: "inbox" },
  { keys: "L", label: "Put off until tomorrow", scope: "inbox" },
  { keys: "X", label: "Select or deselect", scope: "inbox" },
  { keys: "⇧ A", label: "Mark everything shown as read", scope: "inbox" },
  { keys: "1 … 6", label: "Switch tab", scope: "inbox" },
  { keys: "/", label: "Search the inbox", scope: "inbox" },
  { keys: "/", label: "Insert a block (heading, list, table…)", scope: "editor" },
  { keys: "Tab ⇧ Tab", label: "Indent or outdent a list item", scope: "editor" },
  { keys: "⌘ S", label: "Save now (saves on its own too)", scope: "editor" },
  { keys: "Esc", label: "Leave the text", scope: "editor" },
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
    contextMenuState.open ||
    mobileNavState.open
  );
}
