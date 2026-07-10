// Auto-refresh keeps mounted views close to server state. It owns no data:
// callers decide when a refresh is relevant and when local UI state is too
// busy to disturb.

export interface AutoRefreshOptions {
  /** Re-fetch the view's data. Should be safe to call repeatedly; the
   *  caller is responsible for not clobbering in-flight optimistic state
   *  (typically by vetoing via `isBusy`). May be async. */
  refresh: () => void | Promise<void>;
  /** Return true to skip a tick — drag in progress, popover/menu open,
   *  inline editor active, mutation in flight, etc. Checked on both the
   *  interval tick and the focus/visibility revalidate. */
  isBusy?: () => boolean;
  /** Background interval in ms. Pass 0/undefined for focus-only (no
   *  timer) — used by the page detail view, where the body editor makes
   *  a periodic poll more disruptive than it's worth. */
  intervalMs?: number;
  /** Return true when a realtime event is relevant to this mounted view. */
  shouldRefresh?: (event: RealtimeEvent) => boolean;
}

export const REALTIME_INVALIDATE_EVENT = "lific:realtime";
const BUSY_RETRY_MS = 2000;

export type RealtimeEvent = {
  type: string;
  [key: string]: unknown;
};

/**
 * Start an auto-refresh loop. Returns a cleanup function that clears the
 * timer and unbinds listeners — wire it up inside an `$effect` so it
 * tears down on unmount / dependency change:
 *
 * ```ts
 * $effect(() => startAutoRefresh({ refresh, isBusy }));
 * ```
 */
export function startAutoRefresh(opts: AutoRefreshOptions): () => void {
  // SSR / non-browser guard — nothing to bind to.
  if (typeof document === "undefined" || typeof window === "undefined") {
    return () => {};
  }

  const { refresh, isBusy, intervalMs, shouldRefresh } = opts;

  let timer: ReturnType<typeof setInterval> | null = null;
  let eagerDebounce: ReturnType<typeof setTimeout> | null = null;
  let retryDebounce: ReturnType<typeof setTimeout> | null = null;
  let disposed = false;
  let refreshing = false;
  let pending = false;

  function schedulePendingRetry() {
    if (!disposed && !retryDebounce) {
      retryDebounce = setTimeout(() => {
        retryDebounce = null;
        if (pending) void runRefresh();
      }, BUSY_RETRY_MS);
    }
  }

  async function runRefresh() {
    const busy = isBusy?.() ?? false;
    const waiting = disposed || document.hidden || busy || refreshing;

    pending ||= busy || refreshing;
    if (busy) schedulePendingRetry();

    if (!waiting) {
      pending = false;
      refreshing = true;
      try {
        await refresh();
      } catch (err) {
        console.warn("auto-refresh failed", err);
      } finally {
        refreshing = false;
      }
      if (pending) {
        pending = false;
        void runRefresh();
      }
    }
  }

  // Visibility/focus revalidate, debounced so the visibilitychange +
  // window.focus pair that fires on tab-switch-back is a single fetch.
  function scheduleEager() {
    if (!disposed && !document.hidden) {
      if (eagerDebounce) clearTimeout(eagerDebounce);
      eagerDebounce = setTimeout(() => {
        eagerDebounce = null;
        void runRefresh();
      }, 50);
    }
  }

  function onVisibility() {
    if (!document.hidden) {
      scheduleEager();
    }
  }

  function onRealtime(event: Event) {
    const detail = (event as CustomEvent<RealtimeEvent>).detail;
    if (detail && shouldRefresh?.(detail)) {
      scheduleEager();
    }
  }

  document.addEventListener("visibilitychange", onVisibility);
  window.addEventListener("focus", scheduleEager);
  if (shouldRefresh) {
    window.addEventListener(REALTIME_INVALIDATE_EVENT, onRealtime);
  }

  if (intervalMs && intervalMs > 0) {
    timer = setInterval(() => void runRefresh(), intervalMs);
  }

  return () => {
    disposed = true;
    if (timer) clearInterval(timer);
    if (eagerDebounce) clearTimeout(eagerDebounce);
    if (retryDebounce) clearTimeout(retryDebounce);
    document.removeEventListener("visibilitychange", onVisibility);
    window.removeEventListener("focus", scheduleEager);
    if (shouldRefresh) {
      window.removeEventListener(REALTIME_INVALIDATE_EVENT, onRealtime);
    }
  };
}
