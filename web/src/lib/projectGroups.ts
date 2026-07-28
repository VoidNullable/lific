// Which sidebar project groups are collapsed. Per browser, not per account:
// the grouping itself lives in the database, but whether a group is folded
// away is the same class of state as the sidebar width. Every accessor
// swallows storage failures so private mode falls back to all-expanded.

const storageKey = "lific:sidebar:collapsed-groups";

/** Sentinel id standing in for "the group being created but not yet saved".
 *  Shared by the docked sidebar and the mobile nav, which both render the
 *  same inline name field against it. */
export const NEW_GROUP = -1;

export function loadCollapsedGroups(): Set<number> {
  try {
    const raw = localStorage.getItem(storageKey);
    if (raw !== null) {
      const ids: unknown = JSON.parse(raw);
      if (Array.isArray(ids)) {
        return new Set(ids.filter((id): id is number => typeof id === "number"));
      }
    }
  } catch {
    // ignore
  }
  return new Set();
}

export function saveCollapsedGroups(ids: Set<number>): void {
  try {
    localStorage.setItem(storageKey, JSON.stringify([...ids]));
  } catch {
    // ignore
  }
}
