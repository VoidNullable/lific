// Right (issues) sidebar collapsed persistence. Mirrors the left-nav
// helpers in ../sidebarWidth.ts (LIF-360) but under its own storage key so
// the two sidebars collapse independently. Every accessor swallows storage
// failures so private-mode and quota errors fall back to expanded.

const collapsedKey = "lific:rightSidebar:collapsed";

/** Defaults to expanded, so a storage failure never hides the sidebar. */
export function loadRightSidebarCollapsed(): boolean {
  try {
    return localStorage.getItem(collapsedKey) === "1";
  } catch {
    return false;
  }
}

/** Persist the collapsed state. Silently no-ops on storage failure. */
export function saveRightSidebarCollapsed(collapsed: boolean): void {
  try {
    localStorage.setItem(collapsedKey, collapsed ? "1" : "0");
  } catch {
    // ignore
  }
}
