// LIF-507: "since you were last here" for Home and each project overview, kept per browser like
// recents. A visit is a stretch of Home views with no gap longer than
// VISIT_GAP_MS; the line compares against the end of the previous visit, so
// flipping back to Home ten minutes later does not reset it to "nothing new".

export const VISIT_GAP_MS = 30 * 60 * 1000;

export interface VisitState {
  /** Epoch ms of the most recent Home view. */
  seen: number;
  /** Epoch ms this visit is measured from, or null on a first visit. */
  since: number | null;
}

/** The state after viewing Home at `now`, given what was stored. */
export function nextVisit(prev: VisitState | null, now: number): VisitState {
  if (!prev) return { seen: now, since: null };
  if (now - prev.seen > VISIT_GAP_MS) return { seen: now, since: prev.seen };
  return { seen: now, since: prev.since };
}

const KEY = "lific_home_visit";

function read(key: string): VisitState | null {
  try {
    const parsed = JSON.parse(localStorage.getItem(key) ?? "null");
    if (parsed && typeof parsed.seen === "number") {
      return { seen: parsed.seen, since: typeof parsed.since === "number" ? parsed.since : null };
    }
  } catch {
    // ignore: private mode or a hand-edited value
  }
  return null;
}

function record(key: string, now: number): number | null {
  const next = nextVisit(read(key), now);
  try {
    localStorage.setItem(key, JSON.stringify(next));
  } catch {
    // ignore
  }
  return next.since;
}

/** Record a Home view and return when the current visit is measured from. */
export function recordHomeVisit(now = Date.now()): number | null {
  return record(KEY, now);
}

/** The same, for one project's overview. Keyed by id so a renamed
 *  identifier keeps its history. */
export function recordProjectVisit(projectId: number, now = Date.now()): number | null {
  return record(`lific_project_visit:${projectId}`, now);
}
