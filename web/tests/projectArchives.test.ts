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

test("archive fetch returns bytes without saving and forwards cancellation and session", async () => {
  const { downloadProjectArchive } = await import("../src/lib/api");
  localStorage.setItem("lific_token", "lific_sess_test");
  const controller = new AbortController();
  globalThis.fetch = (async (url: string, options: RequestInit) => {
    expect(url).toBe("/api/project-archives/ARC");
    expect(options.signal).toBe(controller.signal);
    expect(options.cache).toBe("no-store");
    expect(new Headers(options.headers).get("Authorization")).toBe("Bearer lific_sess_test");
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
  const { onSessionChange, saveSession, clearSession } = await import("../src/lib/api");
  const seen: (string | null)[] = [];
  const unsubscribe = onSessionChange(() => seen.push(localStorage.getItem("lific_token")));
  try {
    saveSession("first");
    saveSession("refreshed");
    clearSession();
    expect(seen).toEqual(["first", "refreshed", null]);
  } finally { unsubscribe(); }
  saveSession("later");
  expect(seen).toHaveLength(3);
});
