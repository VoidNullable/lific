// Shared live clock (LIF-285).
//
// Relative timestamps ("5m ago") used to render once and go stale forever.
// Rather than give every <TimeAgo> its own setInterval, we keep ONE module
// -level ticking `$state` that all instances read. When it advances every
// ~30s, every TimeAgo that derives from it re-renders together — one timer,
// no per-instance bookkeeping, no drift between surfaces.
//
// The clock is lazy + visibility-aware:
//   - The interval starts on first `now()` read (first subscriber to mount).
//   - While the tab is hidden it stops ticking (no wasted work on a
//     backgrounded tab); on becoming visible again it ticks immediately so
//     stale text snaps current the moment the user looks back.

const TICK_MS = 30_000;

// The reactive heartbeat. Reading it inside a $derived/$effect subscribes
// that computation to every tick.
let tick = $state(Date.now());

let interval: ReturnType<typeof setInterval> | null = null;
let started = false;

function beat() {
  tick = Date.now();
}

function startInterval() {
  if (interval !== null) return;
  interval = setInterval(beat, TICK_MS);
}

function stopInterval() {
  if (interval === null) return;
  clearInterval(interval);
  interval = null;
}

function onVisibilityChange() {
  if (document.visibilityState === "hidden") {
    stopInterval();
  } else {
    // Snap to current time the instant we return, then resume ticking.
    beat();
    startInterval();
  }
}

function ensureStarted() {
  if (started) return;
  started = true;
  // SSR / non-browser guard — Vite builds this SPA client-side, but be safe.
  if (typeof document === "undefined") return;
  if (document.visibilityState !== "hidden") startInterval();
  document.addEventListener("visibilitychange", onVisibilityChange);
}

/**
 * Reactive current-time getter. Call inside a reactive context ($derived /
 * $effect / template) so the consumer re-runs on every tick. Returns epoch
 * milliseconds. Starts the shared interval on first use.
 */
export function now(): number {
  ensureStarted();
  return tick;
}
