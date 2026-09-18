// LIF-243: unified toast system. A single global, rune-based store — no
// context/provider wiring needed, so any module (route, helper function,
// deep child component) can call `toast()` directly and <Toaster/> (mounted
// once in App.svelte) picks it up. Being a plain module-level singleton
// also means toasts (and their Undo actions) survive route navigation: the
// store doesn't unmount when the component that created a toast does.
//
// Undo affordance: pass `action: { label, fn }` and the toast renders a
// button that calls `fn`. Callers own what "undo" means (re-applying a
// captured prior value via the same update function) — this module only
// owns display, stacking, and dismiss timing.

export type ToastKind = "success" | "error" | "info";

export interface ToastAction {
  label: string;
  fn: () => void | Promise<void>;
}

export interface ToastOptions {
  kind?: ToastKind;
  action?: ToastAction;
  /** Explicit auto-dismiss duration in ms. Omit to use the kind default
   *  (5s success/info, 8s error). 0 disables auto-dismiss entirely. */
  duration?: number;
  /** LIF-283: fired exactly once when the toast leaves the stack, with
   *  `didAction` = whether it left because its action button was pressed
   *  (true) vs. it expired / was closed / was evicted without the action
   *  (false). This is what lets a deferred-delete toast tell "Undo pressed"
   *  from "timed out → commit the delete." */
  onClose?: (didAction: boolean) => void;
}

export interface ToastItem {
  id: number;
  message: string;
  kind: ToastKind;
  action?: ToastAction;
  duration: number;
  /** True while hovered/focused — dismiss timer is paused. Read by
   *  <Toaster/> to skip the countdown visually if it ever wants to. */
  paused: boolean;
  /** LIF-283: see ToastOptions.onClose. Held on the item so `dismiss` can
   *  fire it. Not reactive UI state — Toaster never reads it. */
  onClose?: (didAction: boolean) => void;
}

/** Toasts visible at once. The (n+1)th push evicts the oldest so the stack
 *  never grows unbounded during a burst of mutations (e.g. a bulk action
 *  followed immediately by more edits). */
const MAX_TOASTS = 4;
const DEFAULT_DURATION = 5000;
const ERROR_DURATION = 8000;

let nextId = 1;

interface TimerState {
  timeoutId: ReturnType<typeof setTimeout>;
  /** Wall-clock ms when the current countdown segment started. */
  startedAt: number;
  /** ms left in the countdown, updated whenever the timer is paused. */
  remaining: number;
}

class ToastStore {
  toasts = $state<ToastItem[]>([]);

  #timers = new Map<number, TimerState>();

  /** Push a toast. Returns its id (rarely needed — `dismiss` is usually
   *  driven by the timer or the close button, not the caller). */
  push(message: string, opts: ToastOptions = {}): number {
    const kind = opts.kind ?? "info";
    const duration =
      opts.duration ?? (kind === "error" ? ERROR_DURATION : DEFAULT_DURATION);
    const id = nextId++;
    const item: ToastItem = {
      id,
      message,
      kind,
      action: opts.action,
      duration,
      paused: false,
      onClose: opts.onClose,
    };

    let next = [...this.toasts, item];
    if (next.length > MAX_TOASTS) {
      // Oldest collapses out of the stack (evicted, not just visually
      // hidden) — its timer is cleaned up so it can't fire a dismiss for
      // an id nothing references anymore. Eviction counts as a close
      // without the action (LIF-283) so a pending deferred delete still
      // commits if its toast gets pushed out by a burst of later toasts.
      const evicted = next[0];
      next = next.slice(1);
      this.#clearTimer(evicted.id);
      this.#fireClose(evicted, false);
    }
    this.toasts = next;

    if (duration > 0) this.#schedule(id, duration);
    return id;
  }

  /** Remove a toast. `didAction` records whether its action button drove
   *  the removal (LIF-283) so `onClose` can be told; the timer and the
   *  close button pass false, the action path (`runAction` in Toaster)
   *  passes true. */
  dismiss(id: number, didAction = false): void {
    this.#clearTimer(id);
    const item = this.toasts.find((t) => t.id === id);
    this.toasts = this.toasts.filter((t) => t.id !== id);
    if (item) this.#fireClose(item, didAction);
  }

  /** Invoke a toast's onClose exactly once. Guards against double-fire
   *  (e.g. eviction racing a stale timer) by clearing the ref first. */
  #fireClose(item: ToastItem, didAction: boolean): void {
    const cb = item.onClose;
    if (!cb) return;
    item.onClose = undefined;
    cb(didAction);
  }

  /** Pause the auto-dismiss countdown (hover/focus). No-op for toasts with
   *  no timer (duration 0) or that are already paused. */
  pause(id: number): void {
    const timer = this.#timers.get(id);
    if (!timer) return;
    clearTimeout(timer.timeoutId);
    timer.remaining = Math.max(
      0,
      timer.remaining - (Date.now() - timer.startedAt),
    );
    const item = this.toasts.find((t) => t.id === id);
    if (item) item.paused = true;
  }

  /** Resume a paused countdown from wherever it left off. */
  resume(id: number): void {
    const timer = this.#timers.get(id);
    if (!timer) return;
    timer.startedAt = Date.now();
    timer.timeoutId = setTimeout(() => this.dismiss(id), timer.remaining);
    const item = this.toasts.find((t) => t.id === id);
    if (item) item.paused = false;
  }

  #schedule(id: number, duration: number): void {
    const timeoutId = setTimeout(() => this.dismiss(id), duration);
    this.#timers.set(id, {
      timeoutId,
      startedAt: Date.now(),
      remaining: duration,
    });
  }

  #clearTimer(id: number): void {
    const t = this.#timers.get(id);
    if (t) {
      clearTimeout(t.timeoutId);
      this.#timers.delete(id);
    }
  }
}

/** Singleton — import `toast`/`dismiss` for the common case; the store
 *  itself is exported for <Toaster/> to read `.toasts` from and for
 *  pause/resume on hover. */
export const toastStore = new ToastStore();

export function toast(message: string, opts?: ToastOptions): number {
  return toastStore.push(message, opts);
}

export function dismiss(id: number): void {
  toastStore.dismiss(id);
}
