// The project overview's activity digest. The raw audit feed has one row
// per changed field, so a single edit session reads as a wall of
// "created plan_step LIF-PLAN-2". This turns rows into sentences and folds
// a run of changes by one person to one thing into a single line.

import type { Activity } from "../api";
import { assignActivity } from "../issues/assignees";
import { statusLabel, priorityLabel } from "../issues/grouping";

export interface ActivityLine {
  /** The newest folded row's id. */
  id: number;
  actor: string;
  isBot: boolean;
  /** "closed", "commented on", "moved". */
  verb: string;
  /** The thing acted on: an identifier, a module or label name. */
  target: string | null;
  /** What it became: "to Active", "bug". */
  detail: string | null;
  /** A comment's opening words, when the line is a comment. */
  quote: string | null;
  /** Rows folded into this line, itself included. */
  count: number;
  /** Newest row's timestamp. */
  ts: string;
  entityType: string;
  /** The row the line is built from, for links. */
  source: Activity;
}

/** Rows further apart than this start a new line even for the same thing. */
export const FOLD_GAP_MS = 30 * 60 * 1000;

function time(ts: string): number {
  return new Date(ts.replace(" ", "T") + (ts.endsWith("Z") ? "" : "Z")).getTime();
}

function actorName(a: Activity): string {
  return a.actor_display_name || a.actor_username || (a.actor_is_bot ? "An agent" : "Lific");
}

/** What a row is about, so rows about the same thing can fold. Comments,
 *  labels and relations belong to their issue or page. */
export function subjectKey(a: Activity): string {
  // A step that mirrors an issue carries that issue's id, but it is still
  // an edit to the plan.
  if ((a.entity_type === "plan_step" || a.entity_type === "plan") && a.entity_label) {
    return `plan:${a.entity_label}`;
  }
  if (a.issue_id !== null) return `issue:${a.issue_id}`;
  if (a.page_id !== null) return `page:${a.page_id}`;
  return `${a.entity_type}:${a.entity_id}`;
}

/** A short quote of what a comment said, or null. */
export function excerpt(a: Activity, max = 140): string | null {
  if (a.entity_type !== "comment" || a.action !== "create" || !a.new_value) return null;
  const flat = a.new_value.replace(/\s+/g, " ").trim();
  return flat.length > max ? `${flat.slice(0, max - 1).trimEnd()}…` : flat;
}

/** How much a row says about what happened, so a folded line leads with
 *  the closing rather than the label that came with it. */
function weight(a: Activity): number {
  if (a.action === "create" && a.entity_type !== "comment" && a.entity_type !== "plan_step") return 6;
  if (a.action === "delete") return 5;
  if (a.field === "status") return 4;
  if (a.entity_type === "comment") return 3;
  if (a.action === "assign" || a.action === "unassign" || a.action === "wait") return 2;
  return 1;
}

const THING: Record<string, string> = {
  page: "page",
  plan: "plan",
  module: "module",
  label: "label",
  folder: "folder",
  project: "project",
};

function relation(kind: string | null, other: string | null): string | null {
  if (!other) return null;
  return kind ? `${kind.replace(/_/g, " ")} ${other}` : other;
}

/** One row as a sentence fragment. */
export function describe(a: Activity): Pick<ActivityLine, "verb" | "target" | "detail"> {
  const target = a.entity_label;
  const type = a.entity_type;

  if (type === "comment") {
    const verb = a.action === "create" ? "commented on" : a.action === "delete" ? "deleted a comment on" : "edited a comment on";
    return { verb, target, detail: null };
  }
  if (type === "plan_step") {
    if (a.field === "done") {
      return { verb: a.new_value === "1" || a.new_value === "true" ? "checked off a step in" : "reopened a step in", target, detail: null };
    }
    const verb = a.action === "create" ? "added a step to" : a.action === "delete" ? "removed a step from" : "edited a step in";
    return { verb, target, detail: null };
  }
  if (type === "member") {
    return { verb: a.action === "delete" ? "removed a member" : a.action === "create" ? "added a member" : "changed a member's role", target: null, detail: null };
  }

  switch (a.action) {
    case "create":
      if (type === "issue") return { verb: "opened", target, detail: null };
      return { verb: `created ${THING[type] ?? type}`, target, detail: null };
    case "delete":
      return { verb: `deleted ${THING[type] ?? type}`, target, detail: null };
    case "attach":
      return { verb: "labeled", target, detail: a.new_value };
    case "detach":
      return { verb: "unlabeled", target, detail: a.old_value };
    // `field` is the relation as seen from the target: blocks, relates_to.
    case "link":
      return { verb: "linked", target, detail: relation(a.field, a.new_value) };
    case "unlink":
      return { verb: "unlinked", target, detail: relation(a.field, a.old_value) };
    case "assign":
    case "unassign": {
      const { verb, subject } = assignActivity(a.action, a.action === "assign" ? a.new_value : a.old_value);
      return { verb, target, detail: subject };
    }
    case "wait":
      return { verb: "set a wait on", target, detail: null };
    case "unwait":
      return { verb: "cleared a wait on", target, detail: null };
  }

  // Updates.
  if (a.field === "status" && a.new_value) {
    if (a.new_value === "done") return { verb: "closed", target, detail: null };
    if (a.new_value === "cancelled") return { verb: "cancelled", target, detail: null };
    if (a.old_value === "done" || a.old_value === "cancelled") return { verb: "reopened", target, detail: null };
    if (type === "issue") return { verb: "moved", target, detail: `to ${statusLabel(a.new_value)}` };
    return { verb: `marked ${THING[type] ?? type}`, target, detail: a.new_value };
  }
  if (a.field === "priority" && a.new_value) {
    return { verb: "reprioritized", target, detail: `to ${priorityLabel(a.new_value)}` };
  }
  if (a.field === "title" || a.field === "name") return { verb: "renamed", target, detail: null };
  if (a.field === "description" || a.field === "content") return { verb: "edited", target, detail: null };
  if (a.field === "module") return { verb: "moved", target, detail: a.new_value ? `to ${a.new_value}` : "out of its module" };
  if (a.field === "start_date" || a.field === "target_date") return { verb: "rescheduled", target, detail: null };
  if (type === "project") return { verb: "updated the project", target: null, detail: null };
  return { verb: "updated", target, detail: null };
}

/** Sentences for a newest-first feed, folding runs by one actor on one
 *  subject that sit within [`FOLD_GAP_MS`] of each other. */
export function digest(items: Activity[]): ActivityLine[] {
  const out: ActivityLine[] = [];
  let run: Activity[] = [];

  const flush = () => {
    if (run.length === 0) return;
    const lead = run.reduce((best, a) => (weight(a) > weight(best) ? a : best), run[0]);
    const newest = run[0];
    out.push({
      id: newest.id,
      actor: actorName(newest),
      isBot: newest.actor_is_bot,
      ...describe(lead),
      // A folded line names the newest label for the subject, which the
      // lead row might predate (a rename inside the run).
      target: describe(lead).target ?? newest.entity_label,
      quote: excerpt(lead),
      count: run.length,
      ts: newest.ts,
      entityType: lead.entity_type,
      source: lead,
    });
    run = [];
  };

  for (const a of items) {
    const prev = run[run.length - 1];
    const same =
      prev &&
      prev.actor_user_id === a.actor_user_id &&
      subjectKey(prev) === subjectKey(a) &&
      Math.abs(time(prev.ts) - time(a.ts)) <= FOLD_GAP_MS;
    if (!same) flush();
    run.push(a);
  }
  flush();
  return out;
}

/** Local calendar day label for grouping: "Today", "Yesterday", "Mon, Nov 23". */
export function dayLabel(ts: string, now = new Date()): string {
  const d = new Date(time(ts));
  if (d.toDateString() === now.toDateString()) return "Today";
  const yesterday = new Date(now.getTime() - 86_400_000);
  if (d.toDateString() === yesterday.toDateString()) return "Yesterday";
  return d.toLocaleDateString("en-US", {
    weekday: "short",
    month: "short",
    day: "numeric",
    ...(d.getFullYear() !== now.getFullYear() ? { year: "numeric" } : {}),
  });
}

/** Lines grouped by day, newest first. Keyed by position as well as label:
 *  a feed ordered by id can revisit a day when timestamps were imported. */
export function byDay(lines: ActivityLine[], now = new Date()): { key: string; label: string; lines: ActivityLine[] }[] {
  const groups: { key: string; label: string; lines: ActivityLine[] }[] = [];
  for (const line of lines) {
    const label = dayLabel(line.ts, now);
    const last = groups[groups.length - 1];
    if (last && last.label === label) last.lines.push(line);
    else groups.push({ key: `${groups.length}:${label}`, label, lines: [line] });
  }
  return groups;
}
