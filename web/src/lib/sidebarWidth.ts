// Global docked-sidebar width + collapsed persistence (LIF-309, LIF-360).
// Every storage accessor swallows failures so private-mode and quota errors
// fall back to defaults.

const storageKey = "lific:sidebar:width";
const collapsedKey = "lific:sidebar:collapsed";

export const SIDEBAR_DEFAULT_WIDTH = 230;
export const SIDEBAR_MIN_WIDTH = 180;
export const SIDEBAR_MAX_WIDTH = 400;

export function clampSidebarWidth(width: number, min = SIDEBAR_MIN_WIDTH, max = SIDEBAR_MAX_WIDTH): number {
  return Math.min(max, Math.max(min, width));
}

/** Existing values remain CSS pixels. Null means follow the text-scaled default. */
export function loadSidebarWidthPreference(): number | null {
  try {
    const raw = localStorage.getItem(storageKey);
    if (raw !== null) {
      const width = Number(raw);
      if (raw.trim() && Number.isFinite(width)) return clampSidebarWidth(width);
    }
  } catch {
    // ignore
  }
  return null;
}

/** Retain the pixel-only reader used by the public shell. */
export function loadSidebarWidth(): number {
  return loadSidebarWidthPreference() ?? SIDEBAR_DEFAULT_WIDTH;
}

/** Persist deliberate resizing only. Null restores the proportional default. */
export function saveSidebarWidth(width: number | null): void {
  try {
    if (width === null) localStorage.removeItem(storageKey);
    else localStorage.setItem(storageKey, String(clampSidebarWidth(width)));
  } catch {
    // ignore
  }
}

/** A temporary text-size constraint must never replace the user's saved width. */
export function sidebarSizing(preferred: number | null, rootFontSize: number) {
  const scale = Number.isFinite(rootFontSize) && rootFontSize > 0 ? rootFontSize / 16 : 1;
  const min = SIDEBAR_MIN_WIDTH * Math.max(1, scale);
  const max = Math.max(SIDEBAR_MAX_WIDTH, min);
  return { min, max, width: clampSidebarWidth(preferred ?? SIDEBAR_DEFAULT_WIDTH * scale, min, max) };
}

/** Observe a 1rem probe so live text preferences and browser font settings agree. */
export function observeSidebarFontSize(node: HTMLElement, update: (size: number) => void) {
  update(parseFloat(getComputedStyle(document.documentElement).fontSize));
  const observer = new ResizeObserver(([entry]) => {
    // display:none while collapsed must not reset the scale.
    if (entry.contentRect.width > 0) update(entry.contentRect.width);
  });
  observer.observe(node);
  return { destroy: () => observer.disconnect() };
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
