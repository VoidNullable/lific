// LIF-488: the pure half of "Move to project". The modal owns the fetching;
// these decide which projects it offers and what the toast says afterwards.

import type { MovedIssue, Project } from "../api";

/** Projects an issue in `currentProjectId` can move to: every other project
 *  the caller may write to, in the order given. `canWrite` answers per
 *  project; the server still checks both ends. */
export function moveTargets(
  projects: Project[],
  currentProjectId: number,
  canWrite: (projectId: number) => boolean,
): Project[] {
  return projects.filter((p) => p.id !== currentProjectId && canWrite(p.id));
}

/** Projects whose name or identifier contains `query`, case-insensitively. */
export function filterTargets(projects: Project[], query: string): Project[] {
  const q = query.trim().toLowerCase();
  if (!q) return projects;
  return projects.filter(
    (p) => p.identifier.toLowerCase().includes(q) || p.name.toLowerCase().includes(q),
  );
}

/** One line for the toast: the rename, then whatever did not come along. */
export function moveNotice(moved: MovedIssue): string {
  const parts = [`Moved ${moved.previous_identifier} to ${moved.issue.identifier}.`];
  if (moved.dropped_labels?.length) {
    const noun = moved.dropped_labels.length === 1 ? "label" : "labels";
    parts.push(`Dropped ${noun} ${moved.dropped_labels.join(", ")}.`);
  }
  if (moved.dropped_module) parts.push(`Cleared module ${moved.dropped_module}.`);
  if (moved.dropped_assignees?.length) {
    parts.push(`Unassigned ${moved.dropped_assignees.join(", ")}.`);
  }
  return parts.join(" ");
}
