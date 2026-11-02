// Multi-select issue list filters. Each dimension holds a list of chosen
// values: an issue passes a dimension when it matches ANY of them, and must
// pass every dimension that has a selection. An empty list means no filter.
//
// Views and per-project state saved before multi-select stored one string
// per dimension ("" for none); `toFilterList` reads both shapes.

import type { Issue, Module } from "../api";
import { STATUSES, STATUS_UNRESOLVED, isUnresolved } from "./grouping";
import { matchesAssigneeFilter, normalizeAssigneeFilter } from "./assignees";

export type IssueFilters = {
  status: string[];
  priority: string[];
  label: string[];
  module: string[];
  assignee: string[];
};

/** Read a stored filter value: a legacy single string or a list. Drops
 *  empty and non-string entries and duplicates, keeping first-seen order. */
export function toFilterList(value: unknown): string[] {
  const raw = Array.isArray(value) ? value : [value];
  const out: string[] = [];
  for (const v of raw) {
    if (typeof v === "string" && v && !out.includes(v)) out.push(v);
  }
  return out;
}

/** Assignee lists keep only values this build understands. */
export function toAssigneeFilterList(value: unknown): string[] {
  return toFilterList(toFilterList(value).map(normalizeAssigneeFilter));
}

/** `list` with `value` added, or removed when already present. */
export function toggleFilterValue(list: string[], value: string): string[] {
  return list.includes(value) ? list.filter((v) => v !== value) : [...list, value];
}

/** The concrete statuses a status filter admits, or null for no filter.
 *  "Unresolved" expands to every non-terminal status. */
export function statusFilterSet(filter: string[]): Set<string> | null {
  if (filter.length === 0) return null;
  const out = new Set<string>();
  for (const v of filter) {
    if (v === STATUS_UNRESOLVED) {
      for (const s of STATUSES) if (isUnresolved(s)) out.add(s);
    } else {
      out.add(v);
    }
  }
  return out;
}

/** Order-insensitive equality, for "this saved view has unsaved changes". */
export function sameFilterList(a: string[], b: string[]): boolean {
  if (a.length !== b.length) return false;
  const sa = [...a].sort();
  const sb = [...b].sort();
  return sa.every((v, i) => v === sb[i]);
}

type FilterableIssue = Pick<Issue, "status" | "priority" | "labels" | "module_id"> &
  Parameters<typeof matchesAssigneeFilter>[0];

export function applyIssueFilters<T extends FilterableIssue>(
  issues: T[],
  filters: IssueFilters,
  modules: Pick<Module, "id" | "name">[],
  me: { id: number; username: string } | null,
): T[] {
  let out = issues;
  const statuses = statusFilterSet(filters.status);
  if (statuses) out = out.filter((i) => statuses.has(i.status));
  if (filters.priority.length) out = out.filter((i) => filters.priority.includes(i.priority));
  if (filters.label.length) {
    out = out.filter((i) => i.labels.some((l) => filters.label.includes(l)));
  }
  if (filters.module.length) {
    // Module filters are stored by name. A name with no loaded module
    // matches issues without a module, as the single-select filter did.
    const ids = new Set(
      filters.module.map((name) => modules.find((m) => m.name === name)?.id ?? null),
    );
    out = out.filter((i) => ids.has(i.module_id ?? null));
  }
  if (filters.assignee.length) {
    out = out.filter((i) => filters.assignee.some((f) => matchesAssigneeFilter(i, f, me)));
  }
  return out;
}
