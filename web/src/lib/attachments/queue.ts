// LIF-418: a tiny concurrency limiter shared by every composer's upload
// controller.
//
// Uploads used to run strictly one after another, so dropping six screenshots
// meant watching them trickle in. Running all six at once is the other failure
// mode: the browser caps sockets per origin anyway, byte progress becomes
// meaningless, and a slow instance gets hammered. Three in flight is the sweet
// spot - the first chips finish while the rest are visibly queued.
//
// Deliberately DOM-free and dependency-free so it can be unit tested with
// plain promises.

export interface ConcurrencyQueue {
  /** Maximum tasks allowed to run at once. */
  readonly limit: number;
  /** Tasks currently running. */
  readonly active: number;
  /** Tasks admitted but not yet started. */
  readonly waiting: number;
  /** Run `task` as soon as a slot frees up; resolves/rejects with its result. */
  add<T>(task: () => Promise<T>): Promise<T>;
}

export function createConcurrencyQueue(limit: number): ConcurrencyQueue {
  const max = Math.max(1, Math.floor(limit));
  let active = 0;
  const waiting: Array<() => void> = [];

  function pump(): void {
    while (active < max && waiting.length > 0) {
      const start = waiting.shift();
      start?.();
    }
  }

  function add<T>(task: () => Promise<T>): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      const start = () => {
        active += 1;
        let running: Promise<T>;
        try {
          running = Promise.resolve(task());
        } catch (e) {
          running = Promise.reject(e);
        }
        // Free the slot and start the next task in the SAME microtask that
        // settles this one. Chaining a `.finally` instead would let an awaiting
        // caller observe a slot that is still counted as busy.
        const settle = (deliver: () => void) => {
          active -= 1;
          deliver();
          pump();
        };
        running.then(
          (value) => settle(() => resolve(value)),
          (err) => settle(() => reject(err)),
        );
      };
      waiting.push(start);
      pump();
    });
  }

  return {
    get limit() {
      return max;
    },
    get active() {
      return active;
    },
    get waiting() {
      return waiting.length;
    },
    add,
  };
}
