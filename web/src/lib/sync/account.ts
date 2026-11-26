import { clearPrivateSnapshots } from "./persistence";

// undefined means the session has not been verified. Never hydrate private
// rows until the server identifies the account, including with cookie auth.
let account: number | null | undefined;
let revision = 0;
const listeners = new Set<() => void>();
const projectListeners = new Set<(scope: string, projects: number[]) => void>();

export function readModelAccount(): number | null | undefined {
  return account;
}

export function readModelAccountRevision(): number {
  return revision;
}

export function readModelScope(): string {
  return account === null ? "signed-out:" : `private:${account ?? "unverified"}:`;
}

export function onReadModelProjects(listener: (scope: string, projects: number[]) => void): void {
  projectListeners.add(listener);
}

export function reconcileReadModelProjects(scope: string, projects: number[]): void {
  for (const listener of projectListeners) listener(scope, projects);
}

export function setReadModelAccount(id: number | null | undefined): void {
  if (id !== null && id !== undefined && !Number.isSafeInteger(id)) return;
  if (account === id) return;
  account = id;
  revision++;
  for (const listener of listeners) listener();
}

export function onReadModelAccountChange(listener: () => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

/** Cookie notices invalidate identity; only /auth/me establishes the next account. */
export function invalidateReadModelAccount(): Promise<void> {
  account = undefined;
  revision++;
  for (const listener of listeners) listener();
  return Promise.resolve();
}

/** Clear memory immediately; the durable epoch also stops other tabs' old writes. */
export function clearReadModelAccount(): Promise<void> {
  account = null;
  revision++;
  for (const listener of listeners) listener();
  return clearPrivateSnapshots();
}
