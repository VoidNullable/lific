// Per-project persistence for resource-view sub tabs (LIF-305), keyed
// lific:subtab:<view>:<projectId>. Mirrors the conventions of
// lib/issues/persistence.ts: every accessor swallows storage errors
// (private mode / quota) and degrades to in-memory defaults.

const key = (view: string, projectId: string) =>
  `lific:subtab:${view}:${projectId}`;

/** Read the persisted sub tab for a view+project. Returns the stored id
 *  only when it's still one of `valid` (tab sets can change across
 *  releases); otherwise null so the caller can apply its own default —
 *  including "smart" defaults like falling back to All when the preferred
 *  tab would be empty. */
export function loadSubTab(
  view: string,
  projectId: string,
  valid: readonly string[],
): string | null {
  try {
    const raw = localStorage.getItem(key(view, projectId));
    if (raw && valid.includes(raw)) return raw;
  } catch {
    // ignore
  }
  return null;
}

/** Persist the selected sub tab. Silently no-ops on storage failure. */
export function saveSubTab(view: string, projectId: string, id: string): void {
  try {
    localStorage.setItem(key(view, projectId), id);
  } catch {
    // ignore
  }
}
