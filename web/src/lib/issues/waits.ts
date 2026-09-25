// LIF-484/485: user and date blockers ("waits") for the list, board and
// detail views.
//
// The server stamps each wait with a `state`, but that answer is only true on
// the day it was read. A list row can sit in the read model across midnight,
// so every surface recomputes the state here from `earliest`/`latest` against
// the browser's local day. The rule matches the server's:
//
//   holding  before `earliest` (a user wait always holds until cleared)
//   due      from `earliest` through `latest`
//   overdue  after `latest`

import type { IssueWait, WaitState } from "../api";

/** `YYYY-MM-DD` for the local calendar day of `now`. */
export function localDay(now: Date = new Date()): string {
  const y = now.getFullYear();
  const m = String(now.getMonth() + 1).padStart(2, "0");
  const d = String(now.getDate()).padStart(2, "0");
  return `${y}-${m}-${d}`;
}

/** A wait's standing on `today` (`YYYY-MM-DD`). */
export function waitState(wait: IssueWait, today: string = localDay()): WaitState {
  if (wait.kind === "user") return "holding";
  if (wait.earliest && today < wait.earliest) return "holding";
  if (wait.latest && today > wait.latest) return "overdue";
  return "due";
}

/** Parse `YYYY-MM-DD` as a local date (never UTC, which would shift a day
 *  west of Greenwich). */
function parseDay(day: string): Date | null {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(day);
  if (!match) return null;
  return new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]));
}

/** The day after `day`, for "overdue since". */
export function dayAfter(day: string): string {
  const date = parseDay(day);
  if (!date) return day;
  date.setDate(date.getDate() + 1);
  return localDay(date);
}

/** `Sep 28`, or `Sep 28, 2027` when the year is not `thisYear`. */
export function formatDay(day: string, thisYear: number = new Date().getFullYear()): string {
  const date = parseDay(day);
  if (!date) return day;
  return date.toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    ...(date.getFullYear() === thisYear ? {} : { year: "numeric" }),
  });
}

/** `Sep 28`, `Sep 28 to 29`, `Sep 28 to Oct 2`. */
export function formatWindow(
  earliest: string,
  latest: string,
  thisYear: number = new Date().getFullYear(),
): string {
  if (earliest === latest) return formatDay(earliest, thisYear);
  const a = parseDay(earliest);
  const b = parseDay(latest);
  if (a && b && a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth()) {
    return `${formatDay(earliest, thisYear)} to ${b.getDate()}`;
  }
  return `${formatDay(earliest, thisYear)} to ${formatDay(latest, thisYear)}`;
}

/** Who a user wait names: `@blake`. */
export function waitHandle(wait: IssueWait): string {
  return `@${wait.username ?? "deleted user"}`;
}

/** One wait in words, for the detail view and tooltips. */
export function describeWait(
  wait: IssueWait,
  today: string = localDay(),
): { state: WaitState; headline: string } {
  const state = waitState(wait, today);
  if (wait.kind === "user") {
    const name = wait.display_name && wait.display_name !== wait.username
      ? `${wait.display_name} (${waitHandle(wait)})`
      : waitHandle(wait);
    return { state, headline: `Waiting on ${name}` };
  }
  const earliest = wait.earliest ?? "";
  const latest = wait.latest ?? earliest;
  const span = formatWindow(earliest, latest);
  switch (state) {
    case "holding":
      return { state, headline: `Waiting until ${span}` };
    case "due":
      return {
        state,
        headline:
          earliest === latest
            ? `Due to check since ${formatDay(earliest)}`
            : `Due to check since ${formatDay(earliest)}, expected by ${formatDay(latest)}`,
      };
    case "overdue":
      return { state, headline: `Overdue since ${formatDay(dayAfter(latest))}, expected ${span}` };
  }
}

/** Which wait a one-chip indicator shows first: overdue, then anything
 *  still blocking, then due. */
const PROMINENCE: Record<WaitState, number> = { overdue: 0, holding: 1, due: 2 };

export interface WaitSummary {
  wait: IssueWait;
  state: WaitState;
  /** Short chip text: `@blake`, `until Sep 28`, `due Sep 28`, `overdue`. */
  label: string;
  /** Every wait on the issue, one per line, for a tooltip. */
  title: string;
  /** Waits beyond the one shown. */
  more: number;
}

/** The compact indicator for a list row or board card, or null when the
 *  issue waits on nothing. */
export function summarizeWaits(
  waits: IssueWait[] | undefined,
  today: string = localDay(),
): WaitSummary | null {
  if (!waits || waits.length === 0) return null;
  const ranked = waits
    .map((wait) => ({ wait, state: waitState(wait, today) }))
    .sort((a, b) => PROMINENCE[a.state] - PROMINENCE[b.state]);
  const { wait, state } = ranked[0];
  let label: string;
  if (wait.kind === "user") label = waitHandle(wait);
  else if (state === "holding") label = `until ${formatDay(wait.earliest ?? "")}`;
  else if (state === "due") label = `due ${formatDay(wait.earliest ?? "")}`;
  else label = `overdue ${formatDay(dayAfter(wait.latest ?? ""))}`;
  const title = ranked
    .map(({ wait: w }) => {
      const { headline } = describeWait(w, today);
      return w.note ? `${headline} (${w.note})` : headline;
    })
    .join("\n");
  return { wait, state, label, title, more: waits.length - 1 };
}
