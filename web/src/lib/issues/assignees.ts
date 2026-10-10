// LIF-147: who an issue is for, as the web UI shows, edits and filters it.
//
// An issue is in exactly one of three states: unassigned (any agent may
// work it), marked for any person, or assigned to named people. The wire
// shape is `needs_human` plus `assignees`; writes send a whole replacement
// as `[]`, `["human"]` or usernames. Everything here is pure so the list
// filter and the labels can be unit tested without a component.

import type { Issue, IssueAssignee } from "../api";

export type AssignmentKind = "agents" | "human" | "people";

/** The reserved write value that marks an issue for any person. */
export const HUMAN = "human";

/** A person the picker can offer or a marker can draw. */
export interface Person {
  user_id: number;
  username: string;
  display_name?: string | null;
}

type Assignable = Pick<Issue, "needs_human" | "assignees">;

export function assignmentKind(issue: Assignable): AssignmentKind {
  if (!issue.needs_human) return "agents";
  return issue.assignees?.length ? "people" : "human";
}

/** The `assignees` write value that reproduces this issue's assignment. */
export function assignmentNames(issue: Assignable): string[] {
  switch (assignmentKind(issue)) {
    case "agents":
      return [];
    case "human":
      return [HUMAN];
    case "people":
      return (issue.assignees ?? []).map((a) => a.username);
  }
}

export function personName(person: Person): string {
  return person.display_name?.trim() || person.username;
}

export function initials(name: string): string {
  return name
    .split(/[\s_-]+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((w) => w[0]?.toUpperCase() ?? "")
    .join("");
}

/** One line for tooltips, toasts and the sidebar: "Any agent", "Any
 *  person", or the named people. */
export function describeAssignment(issue: Assignable): string {
  switch (assignmentKind(issue)) {
    case "agents":
      return "Any agent";
    case "human":
      return "Any person";
    case "people":
      return (issue.assignees ?? []).map(personName).join(", ");
  }
}

/** Same as {@link describeAssignment}, from a write value. Names resolve
 *  through `people` when known and fall back to `@username`. */
export function describeNames(names: string[], people: Person[] = []): string {
  if (names.length === 0) return "Any agent";
  if (names.length === 1 && names[0] === HUMAN) return "Any person";
  return names
    .map((n) => {
      const p = people.find((x) => x.username.toLowerCase() === n.toLowerCase());
      return p ? personName(p) : `@${n}`;
    })
    .join(", ");
}

/** The issue fields a successful write of `names` produces, for stamping a
 *  local copy before the server's row arrives. Names that `people` does not
 *  know are skipped, so prefer the server's answer when there is one. */
export function assignmentFields(
  names: string[],
  people: Person[] = [],
): { needs_human: boolean; assignees?: IssueAssignee[] } {
  if (names.length === 0) return { needs_human: false, assignees: undefined };
  if (names.length === 1 && names[0] === HUMAN) return { needs_human: true, assignees: undefined };
  const assignees: IssueAssignee[] = [];
  for (const n of names) {
    const p = people.find((x) => x.username.toLowerCase() === n.toLowerCase());
    if (!p) continue;
    const a: IssueAssignee = { user_id: p.user_id, username: p.username };
    if (p.display_name) a.display_name = p.display_name;
    assignees.push(a);
  }
  return { needs_human: true, assignees: assignees.length ? assignees : undefined };
}

/** Whether two write values mean the same assignment (order and case of
 *  usernames do not matter). */
export function sameNames(a: string[], b: string[]): boolean {
  const norm = (xs: string[]) => [...new Set(xs.map((x) => x.toLowerCase()))].sort().join("\n");
  return norm(a) === norm(b);
}

/** Order picker candidates with the signed-in person first, then by name.
 *  Adds `me` when the roster leaves them out (an admin who is not a member
 *  can still be assigned). */
export function orderPeople(people: Person[], me: Person | null): Person[] {
  const rest = people
    .filter((p) => p.user_id !== me?.user_id)
    .sort((a, b) => personName(a).localeCompare(personName(b), undefined, { sensitivity: "base" }));
  return me ? [me, ...rest] : rest;
}

/** Case-insensitive match on username or display name. */
export function matchesPerson(person: Person, query: string): boolean {
  const q = query.trim().toLowerCase().replace(/^@/, "");
  if (!q) return true;
  return (
    person.username.toLowerCase().includes(q) ||
    (person.display_name ?? "").toLowerCase().includes(q)
  );
}

// ── List filter ──────────────────────────────────────────────────────
//
// The stored value mirrors the REST `assignee` filter, with a prefix on
// usernames so a person called "me" or "none" cannot collide:
//   ""        no filter
//   "none"    unassigned (any agent)
//   "human"   a person must do it: anyone or named people
//   "me"      names the signed-in person (portable across users, so a
//             saved view called "Mine" works for everyone)
//   "@alice"  names alice

export const ASSIGNEE_FILTER_NONE = "none";
export const ASSIGNEE_FILTER_HUMAN = "human";
export const ASSIGNEE_FILTER_ME = "me";

export function personFilter(username: string): string {
  return `@${username}`;
}

/** The username a `@name` filter value names, or null. */
export function filterUsername(value: string): string | null {
  return value.startsWith("@") && value.length > 1 ? value.slice(1) : null;
}

/** Drop stored values this build does not understand, so an old or
 *  hand-edited saved view never filters everything out by accident. */
export function normalizeAssigneeFilter(value: unknown): string {
  if (typeof value !== "string") return "";
  if (
    value === ASSIGNEE_FILTER_NONE ||
    value === ASSIGNEE_FILTER_HUMAN ||
    value === ASSIGNEE_FILTER_ME ||
    filterUsername(value) !== null
  ) {
    return value;
  }
  return "";
}

export function matchesAssigneeFilter(
  issue: Assignable,
  filter: string,
  me: { id: number; username: string } | null,
): boolean {
  if (!filter) return true;
  if (filter === ASSIGNEE_FILTER_NONE) return !issue.needs_human;
  if (filter === ASSIGNEE_FILTER_HUMAN) return !!issue.needs_human;
  const named = issue.assignees ?? [];
  if (filter === ASSIGNEE_FILTER_ME) return !!me && named.some((a) => a.user_id === me.id);
  const username = filterUsername(filter);
  if (username === null) return true;
  const lower = username.toLowerCase();
  return named.some((a) => a.username.toLowerCase() === lower);
}

/** Everyone named on any of `issues`, once each, sorted by name. Feeds the
 *  filter's "specific people" list without another request. */
export function assigneesIn(issues: Assignable[]): IssueAssignee[] {
  const seen = new Map<number, IssueAssignee>();
  for (const issue of issues) {
    for (const a of issue.assignees ?? []) if (!seen.has(a.user_id)) seen.set(a.user_id, a);
  }
  return [...seen.values()].sort((a, b) =>
    personName(a).localeCompare(personName(b), undefined, { sensitivity: "base" }),
  );
}

/** Activity wording for an `assign` / `unassign` entry. The stored value
 *  is "@username" or "human". */
export function assignActivity(
  action: string,
  value: string | null,
): { verb: string; subject: string | null } {
  const human = value === HUMAN;
  if (action === "assign") {
    return human ? { verb: "marked for any person", subject: null } : { verb: "assigned", subject: value };
  }
  return human
    ? { verb: "removed the any-person mark", subject: null }
    : { verb: "unassigned", subject: value };
}
