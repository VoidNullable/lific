// Keyed "since you were last here" for workspace pages. Same visit rule as
// Home (lib/home/lastVisit.ts), under a separate storage key per page, so
// opening Home does not clear the Inbox's new markers and vice versa.

import { nextVisit, type VisitState } from "../home/lastVisit";

export interface VisitStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

function defaultStorage(): VisitStorage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

function read(storage: VisitStorage, key: string): VisitState | null {
  try {
    const parsed = JSON.parse(storage.getItem(key) ?? "null");
    if (parsed && typeof parsed.seen === "number") {
      return { seen: parsed.seen, since: typeof parsed.since === "number" ? parsed.since : null };
    }
  } catch {
    // ignore: private mode or a hand-edited value
  }
  return null;
}

/** Record a view of the page stored under `key` and return when the
 *  current visit is measured from, or null on a first visit. */
export function recordVisit(
  key: string,
  now = Date.now(),
  storage: VisitStorage | null = defaultStorage(),
): number | null {
  if (!storage) return null;
  const next = nextVisit(read(storage, key), now);
  try {
    storage.setItem(key, JSON.stringify(next));
  } catch {
    // ignore
  }
  return next.since;
}

export const INBOX_VISIT_KEY = "lific_inbox_visit";
