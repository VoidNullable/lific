// LIF-244 — issue peek panel state.
//
// A module singleton (mirrors toast/toast.svelte.ts) rather than a prop
// threaded through IssueList: any row/card, at any nesting depth, can call
// `openPeek(identifier)` without the parent wiring a callback down through
// IssueRow/IssueCard/BulkActionBar/etc. It also means a future "space to
// peek" keyboard shortcut (LIF-244's follow-up wave) can import and call
// `openPeek` directly instead of threading a new prop through the keyboard
// handler.
//
// LIF-248: the panel itself (PeekPanel.svelte) is now mounted once in
// Layout.svelte — globally, on every authenticated route, not just
// IssueList — and reads this store directly, same shape as Toaster.svelte
// reading toastStore. Only one peek can be open at a time (it's a preview
// of a single issue), so `identifier` is a single nullable field, not a
// stack.

class PeekState {
  open = $state(false);
  identifier = $state<string | null>(null);
}

export const peekState = new PeekState();

// ── LIF-248: mutation sync registration ──────────────────────────────
//
// A mutation applied *from inside the peek* (status/priority/module/title
// edits, or their Undo) needs to reach whichever list/board is showing
// that same issue behind the scrim, so the row/card updates live instead
// of going stale until the next poll. When the panel lived only inside
// IssueList, that was a plain `onIssueChanged` prop. Now that PeekPanel is
// mounted once in Layout — outside every route's component tree — there's
// no prop path down to it anymore, so the route that cares registers a
// callback here instead and unregisters on unmount.
//
// Only IssueList registers today: it's the one surface holding a client-
// side cache (`issues`) that a background poll doesn't immediately
// refresh. Every other route that can now open peek (IssueDetail's
// relation chips, PlanDetail's step/anchor chips, any Markdown link) reads
// its own data fresh on its own load, so a peek mutation there needs no
// broadcast — the route just isn't registered, and `notifyPeekSync` is a
// silent no-op. At most one registrant makes sense (peek shows one issue
// preview over one screen at a time), so this is a single slot, not a
// list of subscribers.
type PeekSyncFn = (id: number, patch: Record<string, unknown>) => void;

let syncFn: PeekSyncFn | null = null;

/** Register `fn` to receive every mutation applied from the peek panel.
 *  Returns an unregister function — call it from the same effect's
 *  cleanup so a route that stops caring (unmount, navigation) can't leak
 *  a stale closure that a later peek mutation would call into a dead
 *  component. */
export function registerPeekSync(fn: PeekSyncFn): () => void {
  syncFn = fn;
  return () => {
    if (syncFn === fn) syncFn = null;
  };
}

/** Called by PeekPanel after every successful mutation. Forwards to
 *  whatever registered (if anything) — see the module doc above. */
export function notifyPeekSync(id: number, patch: Record<string, unknown>): void {
  syncFn?.(id, patch);
}

/** Open the peek panel on `identifier`. If the panel is already open on a
 *  different issue, this just swaps the identifier — PeekPanel's own
 *  effect re-fetches and re-renders in place without a close/reopen
 *  animation (the `open` flag, which drives the mount transition, doesn't
 *  change). */
export function openPeek(identifier: string): void {
  peekState.identifier = identifier;
  peekState.open = true;
}

/** Close the panel. `identifier` is deliberately left set so the close
 *  transition doesn't blank the content mid-animation (PeekPanel keeps
 *  rendering the last-loaded issue while it slides out). */
export function closePeek(): void {
  peekState.open = false;
}
