import { writable } from "svelte/store";
import type { AuthUser } from "./api";

const user = writable<AuthUser | null>(null);
let revision = 0;
let value: AuthUser | null = null;

/** Shared identity for the shell and Settings. Clear on sign-out. */
export const currentUser = {
  subscribe: user.subscribe,
  set(next: AuthUser | null) {
    revision += 1;
    value = next;
    user.set(next);
  },
  update(updater: (previous: AuthUser | null) => AuthUser | null) {
    currentUser.set(updater(value));
  },
};

export function getUserRevision(): number {
  return revision;
}

/** Capture the revision before me(); reject late loads after another publication. */
export function publishUser(next: AuthUser | null, expectedRevision?: number): boolean {
  if (expectedRevision !== undefined && expectedRevision !== revision) return false;
  currentUser.set(next);
  return true;
}
