// Importance heuristic shared by the project overview's "Needs attention"
// list and Home's agent queue (LIF-507):
//
//   score = (priorityWeight + age * 0.5 + idle * 0.6) * statusMultiplier
//
// over open issues only. Cheap, O(n), and honest: an old urgent todo that
// hasn't moved floats to the top. Neither surface shows the number, only
// its causes (priority plus an age and idle cue) or a heat bar.

import type { Issue } from "../api";

const PRIORITY_WEIGHT: Record<string, number> = { urgent: 100, high: 55, medium: 25, low: 10, none: 4 };
const STATUS_MULT: Record<string, number> = { todo: 1.25, active: 1.15, backlog: 1.0 };

/** Whole days since a server timestamp (UTC, no zone suffix). */
export function daysSince(iso: string, now = Date.now()): number {
  const t = new Date(iso.replace(" ", "T") + "Z").getTime();
  if (Number.isNaN(t)) return 0;
  return Math.max(0, Math.floor((now - t) / 86400000));
}

export function importanceScore(
  issue: Pick<Issue, "priority" | "status" | "created_at" | "updated_at">,
  now = Date.now(),
): number {
  const pw = PRIORITY_WEIGHT[issue.priority] ?? 4;
  const sm = STATUS_MULT[issue.status] ?? 1;
  return (pw + daysSince(issue.created_at, now) * 0.5 + daysSince(issue.updated_at, now) * 0.6) * sm;
}

/** A score mapped onto 0..1 for a heat bar. Saturates gently, so a fresh
 *  urgent todo reads hot and a half-year-old medium reads hotter still. */
export function heat(score: number): number {
  return 1 - Math.exp(-Math.max(0, score) / 90);
}

/** "today", "12d", "3mo". */
export function ageLabel(days: number): string {
  if (days >= 60) return `${Math.round(days / 30)}mo`;
  if (days >= 1) return `${days}d`;
  return "today";
}
