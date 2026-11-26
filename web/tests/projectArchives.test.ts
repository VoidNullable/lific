import { afterEach, beforeEach, expect, test } from "bun:test";

let restore: () => void;
beforeEach(() => {
  const fetch = globalThis.fetch, window = globalThis.window, storage = globalThis.localStorage, document = globalThis.document;
  const values = new Map<string, string>();
  Object.assign(globalThis, {
    window: Object.assign(new EventTarget(), { location: { origin: "http://localhost" } }),
    localStorage: {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => values.set(key, value),
      removeItem: (key: string) => values.delete(key),
    },
    document: { createElement: () => { throw new Error("Downloading bytes must not save a file"); } },
  });
  restore = () => Object.assign(globalThis, { fetch, window, localStorage: storage, document });
});
afterEach(() => restore());

test("archive fetch returns bytes without saving and forwards cancellation on the session cookie", async () => {
  const { downloadProjectArchive } = await import("../src/lib/api");
  const controller = new AbortController();
  globalThis.fetch = (async (url: string, options: RequestInit) => {
    expect(url).toBe("/api/project-archives/ARC");
    expect(options.signal).toBe(controller.signal);
    expect(options.cache).toBe("no-store");
    expect(new Headers(options.headers).get("Authorization")).toBeNull();
    return new Response("archive bytes", { headers: { "Content-Type": "application/gzip" } });
  }) as typeof fetch;
  const result = await downloadProjectArchive("ARC", controller.signal);
  expect(result.ok).toBe(true);
  if (result.ok) {
    expect(result.filename).toBe("ARC.lific.tar.gz");
    expect(await result.blob.text()).toBe("archive bytes");
  }
});

test("session observers see same-tab refresh and logout and unsubscribe cleanly", async () => {
  const { onSessionChange, saveSession, clearSession, sessionKey } = await import("../src/lib/api");
  const seen: (string | null)[] = [];
  const unsubscribe = onSessionChange(() => seen.push(sessionKey()));
  try {
    saveSession();
    saveSession();
    clearSession();
    expect(seen).toHaveLength(3);
    const [first, refreshed, signedOut] = seen;
    expect(first).not.toBeNull();
    expect(refreshed).not.toBeNull();
    expect(refreshed).not.toBe(first);
    expect(signedOut).toBeNull();
  } finally { unsubscribe(); }
  saveSession();
  expect(seen).toHaveLength(3);
});
