import { clearSnapshots } from "./persistence";

// undefined means the session has not been verified. Never hydrate private
// rows until the server identifies the account, including with cookie auth.
let account: number | null | undefined;
let revision = 0;
const listeners = new Set<() => void>();
let channel: BroadcastChannel | null = null;
let listening = false;

export function readModelAccount(): number | null | undefined {
  return account;
}

export function readModelAccountRevision(): number {
  return revision;
}

export function setReadModelAccount(id: number | null | undefined): void {
  if (id !== null && id !== undefined && !Number.isSafeInteger(id)) return;
  if (account === id) return;
  account = id;
  revision++;
  for (const listener of listeners) listener();
  if (id !== undefined) channel?.postMessage({ account: id });
}

export function onReadModelAccountChange(listener: () => void): () => void {
  listeners.add(listener);
  const unsubscribe = () => { listeners.delete(listener); };
  if (listening || typeof window === "undefined") return unsubscribe;
  listening = true;
  if (typeof BroadcastChannel === "function") {
    channel = new BroadcastChannel("lific-read-model-account");
    channel.onmessage = event => {
      // The message invalidates identity; only our own /auth/me may adopt it.
      if (event.data?.account !== account) setReadModelAccount(undefined);
    };
  }
  window.addEventListener("storage", event => {
    if (event.key === "lific_token" || event.key === null) setReadModelAccount(undefined);
  });
  return unsubscribe;
}

/** Clear memory immediately; the durable epoch also stops other tabs' old writes. */
export function clearReadModelAccount(): Promise<void> {
  account = null;
  revision++;
  for (const listener of listeners) listener();
  channel?.postMessage({ account: null });
  return clearSnapshots();
}
