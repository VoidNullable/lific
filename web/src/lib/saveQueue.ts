/** A serialized, last-write-wins save queue for per-field autosave.
 *
 *  Instance settings autosave on blur and on toggle, and the old guard was
 *  `if (saving) return`. That silently **dropped** the second edit: flip two
 *  toggles quickly, or blur a field while a toggle is still in flight, and one
 *  of them never reached the server while the UI happily showed both as
 *  applied. The user gets no error, because nothing failed; the write simply
 *  never happened.
 *
 *  This queue keeps the one-at-a-time property, which the server wants anyway
 *  (each patch is a transaction on the single writer), and drops nothing:
 *
 *  - a patch arriving while one is in flight is queued rather than discarded;
 *  - queued patches are **coalesced per field**, last write wins, so mashing a
 *    toggle four times sends the final state once instead of four times;
 *  - fields not mentioned by a later patch survive the merge, so a queued
 *    `allow_signup` and a queued `instance_name` go together in one request.
 *
 *  Deliberately free of Svelte and of the API client: it takes a `send`
 *  function and a merge, which is what makes it testable without a DOM.
 */

export type QueueState = "idle" | "sending";

export interface SaveQueueOptions<P> {
  /** Perform one save. Resolves with whether it landed. */
  send: (patch: P) => Promise<boolean>;
  /** Merge a newer patch over a queued one. Later keys win. */
  merge?: (queued: P, next: P) => P;
  /** Called whenever the queue starts or stops working. */
  onStateChange?: (state: QueueState) => void;
}

export interface SaveQueue<P> {
  /** Queue a patch. Resolves once *this* patch has been attempted. */
  push: (patch: P) => Promise<boolean>;
  /** Whether a save is in flight right now. */
  readonly busy: boolean;
  /** The patch waiting behind the in-flight one, if any. */
  readonly pending: P | null;
}

/** Shallow merge, later keys winning. The default for plain patch objects. */
function shallowMerge<P extends object>(queued: P, next: P): P {
  return { ...queued, ...next };
}

export function createSaveQueue<P extends object>(
  options: SaveQueueOptions<P>,
): SaveQueue<P> {
  const merge = options.merge ?? shallowMerge;
  let busy = false;
  let pending: P | null = null;
  // Everyone who pushed into the current `pending` is waiting on this.
  let pendingWaiters: ((landed: boolean) => void)[] = [];

  async function drain() {
    if (busy) return;
    busy = true;
    options.onStateChange?.("sending");
    try {
      while (pending !== null) {
        const patch = pending;
        const waiters = pendingWaiters;
        // Cleared before the await, so anything arriving during the request
        // starts a fresh batch instead of being folded into one already sent.
        pending = null;
        pendingWaiters = [];
        let landed = false;
        try {
          landed = await options.send(patch);
        } finally {
          for (const resolve of waiters) resolve(landed);
        }
      }
    } finally {
      busy = false;
      options.onStateChange?.("idle");
    }
  }

  return {
    push(patch: P) {
      pending = pending === null ? patch : merge(pending, patch);
      const settled = new Promise<boolean>((resolve) => {
        pendingWaiters.push(resolve);
      });
      void drain();
      return settled;
    },
    get busy() {
      return busy;
    },
    get pending() {
      return pending;
    },
  };
}
