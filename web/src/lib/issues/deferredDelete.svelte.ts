// LIF-283 — deferred issue deletion with an Undo toast.
//
// Deletes are the one destructive action in the UI with no recovery. This
// module makes issue deletes *optimistic + deferred*: the caller removes the
// rows from its local state immediately (so the UI feels instant), we show a
// "Deleted N issue(s)" toast with an Undo action, and the actual DELETE API
// call(s) only fire once that toast closes without Undo being pressed.
//
//   - Undo pressed        → cancel, call `onRestore`, never touch the API.
//   - Toast expires/closed → commit: fire the DELETE(s); on failure, toast an
//                            error and call `onRestore` so nothing is lost.
//   - Page hide / nav away → flush synchronously via keepalive fetch so
//                            closing the tab can't silently cancel the delete.
//
// Only ONE batch is pending at a time. Scheduling a new batch first flushes
// (commits) any outstanding one — this keeps the mental model and the flush
// path simple, and matches how a user thinks ("I deleted these, then those").
//
// The module owns *timing and the API*; the caller owns *local state* (it did
// the optimistic removal and provides `onRestore` to put rows back). Restore
// callbacks capture plain issue objects, never live component state, so Undo
// keeps working even after the originating component has unmounted (same
// discipline as state.svelte.ts's update-undo layer).

import { deleteIssue, type Issue } from "../api";
import { toast } from "../toast/toast.svelte";

interface PendingBatch {
  /** The issues being deleted (captured objects — used for the count label
   *  and for the keepalive flush path, which needs their ids). */
  issues: Issue[];
  /** Put the rows back in the UI. Called on Undo and on API failure. */
  onRestore: () => void;
  /** Optional hook after a successful commit (e.g. reconcile counts). */
  onCommit?: () => void;
  /** True once the DELETE calls have been fired, so we never double-commit
   *  (timer vs. flush vs. a second schedule racing each other). */
  committed: boolean;
}

/** The single in-flight batch, or null. Module-level singleton so the
 *  pagehide/visibilitychange listeners (registered once) and every delete
 *  entry point share one pending slot. */
let pending: PendingBatch | null = null;

/** Reactive mirror of "is a batch pending" for the auto-refresh poll veto.
 *  A plain `$state` boolean the poll's `isBusy()` can read so a background
 *  refresh doesn't resurrect optimistically-removed rows before the commit
 *  lands. Kept in sync wherever `pending` is set/cleared. */
const pendingSignal = $state({ active: false });

/** Read by IssueList's `autoRefreshBusy()`. Truthy while a deferred delete
 *  is waiting to commit. */
export function hasPendingDeletes(): boolean {
  return pendingSignal.active;
}

function setPending(batch: PendingBatch | null): void {
  pending = batch;
  pendingSignal.active = batch !== null;
}

/** Fire the DELETE for one id via the normal authed JSON path (used when we
 *  have time to await — the timer/close commit). Returns ok/err. */
async function commitViaApi(batch: PendingBatch): Promise<void> {
  if (batch.committed) return;
  batch.committed = true;
  // Clear the pending slot up front so a concurrent schedule() doesn't try to
  // flush this same batch again.
  if (pending === batch) setPending(null);

  const results = await Promise.allSettled(
    batch.issues.map((i) => deleteIssue(i.id)),
  );
  const failed = results.filter(
    (r) => r.status === "rejected" || (r.status === "fulfilled" && !r.value.ok),
  ).length;

  if (failed > 0) {
    // Some (or all) deletes didn't take — put the rows back rather than
    // leaving the UI claiming they're gone when the server still has them.
    toast(
      failed === batch.issues.length
        ? `Couldn't delete ${label(batch.issues)} — restored`
        : `Couldn't delete ${failed} of ${batch.issues.length} issues — restored`,
      { kind: "error" },
    );
    batch.onRestore();
  } else {
    batch.onCommit?.();
  }
}

/** Synchronous best-effort commit for the unload path. `fetch(..., {
 *  keepalive: true })` lets an authed DELETE outlive the page (sendBeacon
 *  can't set an Authorization header or a DELETE method). We can't await the
 *  responses during unload, so this is fire-and-forget; failures there are
 *  invisible (the tab is going away) but the far more common "closed the tab
 *  right after deleting" case now actually deletes instead of silently
 *  cancelling. Mirrors api.ts's token + header construction. */
function commitViaKeepalive(batch: PendingBatch): void {
  if (batch.committed) return;
  batch.committed = true;
  if (pending === batch) setPending(null);

  const token = localStorage.getItem("lific_token");
  const headers: Record<string, string> = {};
  if (token) headers["Authorization"] = `Bearer ${token}`;
  for (const i of batch.issues) {
    try {
      void fetch(`/api/issues/${i.id}`, {
        method: "DELETE",
        headers,
        keepalive: true,
      });
    } catch {
      // Unload path — nothing we can do, and nothing is watching.
    }
  }
}

function label(issues: Issue[]): string {
  return issues.length === 1 ? issues[0].identifier : `${issues.length} issues`;
}

export interface ScheduleOptions {
  /** Reinsert the optimistically-removed rows (Undo, or commit failure). */
  onRestore: () => void;
  /** Optional: run after a clean commit (e.g. refresh counts). */
  onCommit?: () => void;
}

/**
 * Schedule a deferred delete for `issues`. The caller must have ALREADY
 * removed them from its local state (this module only handles timing + the
 * API). Shows a "Deleted N issue(s)" toast with Undo; the DELETE fires when
 * that toast closes without Undo.
 *
 * If a previous batch is still pending it is committed first (flushed) — only
 * one batch is ever outstanding.
 */
export function scheduleDelete(issues: Issue[], opts: ScheduleOptions): void {
  if (issues.length === 0) return;

  // Flush any outstanding batch before taking over the single pending slot.
  if (pending && !pending.committed) {
    void commitViaApi(pending);
  }

  const batch: PendingBatch = {
    issues,
    onRestore: opts.onRestore,
    onCommit: opts.onCommit,
    committed: false,
  };
  setPending(batch);

  toast(`Deleted ${label(issues)}`, {
    kind: "info",
    action: {
      label: "Undo",
      fn: () => {
        // Undo: cancel the commit entirely and put the rows back. The toast's
        // onClose still fires (with didAction=true) but the guard below makes
        // it a no-op.
        if (batch.committed) return;
        batch.committed = true;
        if (pending === batch) setPending(null);
        batch.onRestore();
        toast(`Restored ${label(issues)}`, { kind: "info", duration: 3000 });
      },
    },
    onClose: (didAction) => {
      // Closed without Undo (timeout, close button, or eviction) → commit.
      // didAction=true means the Undo branch above already ran.
      if (didAction) return;
      void commitViaApi(batch);
    },
  });
}

/** Immediately commit any pending batch via keepalive (unload path). Exposed
 *  for tests / explicit flushes; the listeners below call it. */
export function flushPendingDeletes(): void {
  if (pending && !pending.committed) commitViaKeepalive(pending);
}

// One-time global listener: commit synchronously when the page is going away
// so a tab close / navigation doesn't cancel a pending delete. pagehide is
// the "page is being unloaded" signal. Deliberately NOT listening to
// visibilitychange→hidden: that fires on a plain tab switch, which would
// commit early and silently turn the still-visible Undo button into a no-op.
if (typeof window !== "undefined") {
  window.addEventListener("pagehide", flushPendingDeletes);
}
