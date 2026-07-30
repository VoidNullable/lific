// Global docked-sidebar width + collapsed persistence (LIF-309, LIF-360).
// Every storage accessor swallows failures so private-mode and quota errors
// fall back to defaults.

const storageKey = "lific:sidebar:width";
const collapsedKey = "lific:sidebar:collapsed";

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

/** LIF-360: whether the md+ docked sidebar is collapsed out of the layout.
 *  Defaults to expanded, so a storage failure never hides navigation. */
export function loadSidebarCollapsed(): boolean {
  try {
    return localStorage.getItem(collapsedKey) === "1";
  } catch {
    return false;
  }
}

/** Persist the collapsed state. Silently no-ops on storage failure. */
export function saveSidebarCollapsed(collapsed: boolean): void {
  try {
    localStorage.setItem(collapsedKey, collapsed ? "1" : "0");
  } catch {
    // ignore
  }
}
