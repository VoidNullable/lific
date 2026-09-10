// LIF-248 — right-click context menu state.
//
// A module singleton (mirrors toast/toast.svelte.ts and issues/peek.svelte.ts)
// rather than a prop threaded through every surface that wants a context
// menu: IssueRow, IssueCard, and Markdown.svelte's identifier links all
// call `openContextMenu(x, y, items)` directly. Only one menu can ever be
// open at a time (right-clicking a second target just replaces the first
// menu's position/items in place — see ContextMenu.svelte's window
// `contextmenu` listener), so this is a single slot, not a stack.

import type { Icon } from "lucide-svelte";

export interface ContextMenuItem {
  label: string;
  /** Lucide icon component, rendered at 14px — same vocabulary as every
   *  other menu/row icon in the app. Optional so a future text-only item
   *  isn't forced to pick one. Typed as the lucide `Icon` base class so any
   *  named icon (PanelRight, ExternalLink, ...) assigns cleanly; lucide-svelte
   *  v1 icons are legacy class components, not Svelte 5 `Component` functions. */
  icon?: typeof Icon;
  action?: () => void;
  /** Render navigation as a real link. Modified clicks retain browser behavior. */
  href?: string;
  disabled?: boolean;
}

class ContextMenuState {
  open = $state(false);
  x = $state(0);
  y = $state(0);
  items = $state<ContextMenuItem[]>([]);
  generation = $state(0);
}

export const contextMenuState = new ContextMenuState();
let returnTarget: HTMLElement | null = null;
let fallbackTarget: HTMLElement | null = null;

function restoreFocus(target: HTMLElement | null): boolean {
  if (!target?.isConnected || target.closest('[inert]')) return false;
  const temporaryTabindex = !target.hasAttribute('tabindex') && target.tabIndex < 0;
  if (temporaryTabindex) target.setAttribute('tabindex', '-1');
  target.focus({ preventScroll: true });
  const focused = document.activeElement === target;
  if (temporaryTabindex) {
    // Removing tabindex while a generic row is focused blurs it in Chromium.
    if (focused) target.addEventListener('blur', () => target.removeAttribute('tabindex'), { once: true });
    else target.removeAttribute('tabindex');
  }
  return focused;
}

/** Open the menu at viewport coordinates `x, y` (typically `e.clientX` /
 *  `e.clientY` from the triggering `contextmenu` event) with `items`.
 *  Callers are responsible for `e.preventDefault()` (suppress the native
 *  menu) and `e.stopPropagation()` (so ContextMenu.svelte's own
 *  outside-right-click-closes listener, which lives on `window`, doesn't
 *  immediately close the menu this call just opened — see that
 *  component's contextmenu listener for the full reasoning). */
export function openContextMenu(x: number, y: number, items: ContextMenuItem[], trigger?: HTMLElement | null): void {
  const active = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  // Reopening from within the menu must not save an item that is about to disappear.
  if (!active?.closest('[data-context-menu]')) fallbackTarget = active;
  returnTarget = trigger ?? fallbackTarget;
  contextMenuState.x = x;
  contextMenuState.y = y;
  contextMenuState.items = items;
  contextMenuState.open = true;
  contextMenuState.generation += 1;
}

export function closeContextMenu(restore = true): void {
  if (!contextMenuState.open) return;
  contextMenuState.open = false;
  // Synchronous by design: an action may now focus a dialog or rename field.
  if (restore && !restoreFocus(returnTarget)) restoreFocus(fallbackTarget);
  returnTarget = null;
  fallbackTarget = null;
}
