// Per-project "last seen" snapshot for the issue list (LIF-153).
//
// The list already background-polls every 15 seconds and swaps rows in
// silently, so an issue an agent touched two minutes ago looks exactly like
// one nobody has touched in a month. This module stores, per project, the
// `updated_at` each issue carried the last time the user actually looked at
// it. Anything whose current `updated_at` is newer than its stored value
// (or which has no stored value at all, i.e. it was created since) counts as
// changed, and the list marks it.
//
// Shape (one localStorage key per project, `lific:seen:<identifier>`):
//
//   { "41": "2026-08-16T18:22:03Z", "57": "2026-08-14T09:10:00Z" }
//
// Keys are issue ids as strings, because JSON object keys always are;
// storing them that way keeps the round trip lossless instead of quietly
// changing type on reload. Values are the raw `updated_at` strings straight off the
// API; they're ISO-8601, so a plain lexicographic compare orders them
// correctly (the same assumption `sort.ts` and the Recent sub-tab already
// make with `localeCompare`).
//
// Everything here is pure + storage-only, deliberately mirroring
// `issues/persistence.ts`: the reactive half lives in the component that
// owns the issue data. Every accessor swallows storage errors (private
// mode / quota) so a failed write degrades to in-memory behavior rather
// than breaking the list.

/** issue id (as a string) → the `updated_at` the user last saw for it. */
export type SeenMap = Record<string, string>;

/** The only two fields this module needs off an issue. */
export type SeenIssue = { id: number; updated_at: string };

const seenKey = (projectId: string) => `lific:seen:${projectId}`;

/** Read a project's stored snapshot.
 *
 *  Returns `null`, distinct from `{}`, when the project has never been
 *  visited on this device. That distinction is the whole first-visit rule:
 *  an empty map means "seen, and it had no issues", while `null` means
 *  "no baseline yet", which the caller seeds silently so a project opened
 *  for the first time doesn't light up every row at once. */
export function loadSeen(projectId: string): SeenMap | null {
  try {
    const raw = localStorage.getItem(seenKey(projectId));
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return null;
    const out: SeenMap = {};
    for (const [id, at] of Object.entries(parsed as Record<string, unknown>)) {
      if (typeof at === "string") out[id] = at;
    }
    return out;
  } catch {
    return null;
  }
}

/** Persist a project's snapshot. Silently no-ops on storage failure. */
export function saveSeen(projectId: string, seen: SeenMap): void {
  try {
    localStorage.setItem(seenKey(projectId), JSON.stringify(seen));
  } catch {
    // ignore
  }
}

/** Snapshot every issue at its current `updated_at`: the first-visit seed. */
export function snapshotOf(issues: readonly SeenIssue[]): SeenMap {
  const out: SeenMap = {};
  for (const issue of issues) out[String(issue.id)] = issue.updated_at;
  return out;
}

/** True when the user has already seen this issue at its current revision.
 *  An issue missing from the map is NOT seen. That is how issues created
 *  since the snapshot get counted as changed without any extra bookkeeping. */
export function isSeen(seen: SeenMap, issue: SeenIssue): boolean {
  const at = seen[String(issue.id)];
  return at !== undefined && issue.updated_at <= at;
}

/** Record one issue at its current revision.
 *
 *  Returns the same map reference when nothing would change, so callers can
 *  use the identity check to skip a redundant reactive write + storage
 *  round trip on every row they pass through. */
export function withSeen(seen: SeenMap, issue: SeenIssue): SeenMap {
  if (isSeen(seen, issue)) return seen;
  return { ...seen, [String(issue.id)]: issue.updated_at };
}

/** Drop entries for issues the project no longer has, so a long-lived
 *  project's map can't grow without bound as issues are deleted. Returns
 *  the same reference when nothing is stale. */
export function pruneSeen(seen: SeenMap, issues: readonly SeenIssue[]): SeenMap {
  const live = new Set(issues.map((issue) => String(issue.id)));
  const keys = Object.keys(seen);
  if (keys.every((id) => live.has(id))) return seen;
  const out: SeenMap = {};
  for (const id of keys) {
    if (live.has(id)) out[id] = seen[id];
  }
  return out;
}
