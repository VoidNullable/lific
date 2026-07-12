// Global docked-sidebar width persistence (LIF-309). Every storage accessor
// swallows failures so private-mode and quota errors fall back to defaults.

const storageKey = "lific:sidebar:width";

export const SIDEBAR_DEFAULT_WIDTH = 230;
export const SIDEBAR_MIN_WIDTH = 180;
export const SIDEBAR_MAX_WIDTH = 400;

export function clampSidebarWidth(width: number): number {
  return Math.min(SIDEBAR_MAX_WIDTH, Math.max(SIDEBAR_MIN_WIDTH, width));
}

/** Read the persisted docked-sidebar width, clamping stale or invalid bounds. */
export function loadSidebarWidth(): number {
  try {
    const raw = localStorage.getItem(storageKey);
    if (raw !== null) {
      const width = Number(raw);
      if (Number.isFinite(width)) return clampSidebarWidth(width);
    }
  } catch {
    // ignore
  }
  return SIDEBAR_DEFAULT_WIDTH;
}

/** Persist the docked-sidebar width. Silently no-ops on storage failure. */
export function saveSidebarWidth(width: number): void {
  try {
    localStorage.setItem(storageKey, String(clampSidebarWidth(width)));
  } catch {
    // ignore
  }
}
