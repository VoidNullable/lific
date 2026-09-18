import { afterEach, beforeEach, describe, expect, spyOn, test } from "bun:test";
import { REALTIME_INVALIDATE_EVENT } from "../src/lib/autoRefresh.svelte";

class MemoryStorage {
  private store = new Map<string, string>();

  getItem(key: string) {
    return this.store.get(key) ?? null;
  }

  setItem(key: string, value: string) {
    this.store.set(key, value);
  }
}

class BrowserWindow extends EventTarget {
  location = { origin: "http://localhost" };
  private listeners = new Map<
    string,
    Map<EventListenerOrEventListenerObject, Set<boolean>>
  >();

  private capture(
    options?: AddEventListenerOptions | EventListenerOptions | boolean,
  ): boolean {
    return typeof options === "boolean" ? options : (options?.capture ?? false);
  }

  override addEventListener(
    type: string,
    callback: EventListenerOrEventListenerObject | null,
    options?: AddEventListenerOptions | boolean,
  ) {
    super.addEventListener(type, callback, options);
    if (!callback) return;
    const callbacks = this.listeners.get(type) ?? new Map();
    const captures = callbacks.get(callback) ?? new Set();
    captures.add(this.capture(options));
    callbacks.set(callback, captures);
    this.listeners.set(type, callbacks);
  }

  override removeEventListener(
    type: string,
    callback: EventListenerOrEventListenerObject | null,
    options?: EventListenerOptions | boolean,
  ) {
    super.removeEventListener(type, callback, options);
    if (!callback) return;
    const callbacks = this.listeners.get(type);
    const captures = callbacks?.get(callback);
    captures?.delete(this.capture(options));
    if (captures?.size === 0) callbacks?.delete(callback);
    if (callbacks?.size === 0) this.listeners.delete(type);
  }

  listenerCount(type: string): number {
    let count = 0;
    for (const captures of this.listeners.get(type)?.values() ?? [])
      count += captures.size;
    return count;
  }
}

class BrowserDocument extends EventTarget {
  hidden = false;
}

let fetchIssueCached: typeof import("../src/lib/references").fetchIssueCached;
let fetchModuleCached: typeof import("../src/lib/references").fetchModuleCached;
let invalidateReferenceCache: typeof import("../src/lib/references").invalidateReferenceCache;
let subscribeIssueStatus: typeof import("../src/lib/references").subscribeIssueStatus;

let storage: MemoryStorage;
let status = "todo";
let moduleName = "Module A";
let calls = 0;
let active = 0;
let peakActive = 0;
let fail = false;
let failureStatus = 503;
let gate: Promise<void> | null = null;
let releaseGate: (() => void) | null = null;
let gatePath: string | null = null;
let subscriptionCleanups: Array<() => void> = [];
let browserWindow: BrowserWindow;
const originalFetch = globalThis.fetch;
const originalWindow = globalThis.window;
const originalDocument = globalThis.document;
const originalLocalStorage = globalThis.localStorage;

function subscribe(
  identifier: string,
  subscriber: Parameters<typeof subscribeIssueStatus>[1],
): () => void {
  const stop = subscribeIssueStatus(identifier, subscriber);
  subscriptionCleanups.push(stop);
  return stop;
}

async function waitForGate(signal: AbortSignal | undefined): Promise<void> {
  const currentGate = gate;
  if (!currentGate) return;
  if (signal?.aborted) throw new DOMException("Aborted", "AbortError");
  await new Promise<void>((resolve, reject) => {
    const onAbort = () => {
      signal?.removeEventListener("abort", onAbort);
      reject(new DOMException("Aborted", "AbortError"));
    };
    signal?.addEventListener("abort", onAbort, { once: true });
    currentGate.then(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve();
    }, reject);
  });
}

beforeEach(async () => {
  browserWindow = new BrowserWindow();
  (globalThis as { window: unknown }).window = browserWindow;
  (globalThis as { document: unknown }).document = new BrowserDocument();
  ({
    fetchIssueCached,
    fetchModuleCached,
    invalidateReferenceCache,
    subscribeIssueStatus,
  } = await import("../src/lib/references"));
  storage = new MemoryStorage();
  (globalThis as { localStorage: unknown }).localStorage = storage;
  calls = 0;
  active = 0;
  peakActive = 0;
  fail = false;
  failureStatus = 503;
  moduleName = "Module A";
  gate = null;
  releaseGate = null;
  gatePath = null;
  subscriptionCleanups = [];
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    calls += 1;
    active += 1;
    peakActive = Math.max(peakActive, active);
    const responseStatus = status;
    const responseModuleName = moduleName;
    try {
      if (gate && (!gatePath || String(input).includes(gatePath))) {
        await waitForGate(init?.signal ?? undefined);
      }
      if (fail) {
        return {
          ok: false,
          status: failureStatus,
          json: async () => ({ error: "busy" }),
        };
      }
      return {
        ok: true,
        status: 200,
        json: async () =>
          String(input).includes("/modules/")
            ? { id: 1, name: responseModuleName }
            : { identifier: "LIF-1", status: responseStatus },
      };
    } finally {
      active -= 1;
    }
  }) as unknown as typeof fetch;
});

afterEach(() => {
  for (const stop of subscriptionCleanups.splice(0)) stop();
  globalThis.fetch = originalFetch;
  (globalThis as { window: unknown }).window = originalWindow;
  (globalThis as { document: unknown }).document = originalDocument;
  (globalThis as { localStorage: unknown }).localStorage = originalLocalStorage;
});

describe("fetchIssueCached", () => {
  test("does not reuse an account's issue snapshot after the session changes", async () => {
    storage.setItem("lific_token", "account-a");
    status = "done";
    const first = await fetchIssueCached("LIF-1");
    expect(first.status).toBe("ok");
    if (first.status === "ok") expect(first.issue.status).toBe("done");

    storage.setItem("lific_token", "account-b");
    status = "todo";
    const second = await fetchIssueCached("LIF-1");
    expect(second.status).toBe("ok");
    if (second.status === "ok") expect(second.issue.status).toBe("todo");
    expect(calls).toBe(2);
  });

  test("retries a transient failure", async () => {
    storage.setItem("lific_token", "retry");
    fail = true;
    expect((await fetchIssueCached("LIF-2")).status).toBe("unavailable");

    fail = false;
    status = "active";
    const retry = await fetchIssueCached("LIF-2");
    expect(retry.status).toBe("ok");
    if (retry.status === "ok") expect(retry.issue.status).toBe("active");
    expect(calls).toBe(2);
  });

  test("caches stable not-found responses", async () => {
    storage.setItem("lific_token", "not-found");
    fail = true;
    failureStatus = 404;
    expect((await fetchIssueCached("LIF-404")).status).toBe("unavailable");
    expect((await fetchIssueCached("LIF-404")).status).toBe("unavailable");
    expect(calls).toBe(1);
  });

  test("re-fetches a status after realtime invalidation", async () => {
    storage.setItem("lific_token", "realtime");
    status = "todo";
    await fetchIssueCached("LIF-3");

    status = "done";
    invalidateReferenceCache();
    const refreshed = await fetchIssueCached("LIF-3");
    expect(refreshed.status).toBe("ok");
    if (refreshed.status === "ok") expect(refreshed.issue.status).toBe("done");
    expect(calls).toBe(2);
  });

  test("resolves a shared status once for every subscriber", async () => {
    const results: string[] = [];
    const seen = Promise.withResolvers<void>();
    const record = (result: { status: string }) => {
      results.push(result.status);
      if (results.length === 2) seen.resolve();
    };
    const stopFirst = subscribe("LIF-4", (result) => record(result));
    const stopSecond = subscribe("LIF-4", (result) => record(result));

    await seen.promise;

    expect(results).toEqual(["ok", "ok"]);
    expect(calls).toBe(1);
    stopFirst();
    stopSecond();
  });

  test("isolates subscriber exceptions", async () => {
    const logged = spyOn(console, "error").mockImplementation(() => {});
    const delivered = Promise.withResolvers<string>();
    const stopFirst = subscribe("LIF-EXCEPTION", () => {
      throw new Error("broken subscriber");
    });
    const stopSecond = subscribe("LIF-EXCEPTION", (result) =>
      delivered.resolve(result.status),
    );

    try {
      expect(await delivered.promise).toBe("ok");
      expect(logged).toHaveBeenCalledTimes(1);
    } finally {
      stopFirst();
      stopSecond();
      logged.mockRestore();
    }
  });

  test("returns but does not cache a direct response invalidated in the same session", async () => {
    storage.setItem("lific_token", "same-session");
    status = "todo";
    gate = new Promise<void>((resolve) => {
      releaseGate = resolve;
    });
    const controller = new AbortController();
    const inFlight = fetchIssueCached("LIF-5", controller.signal);
    await Promise.resolve();

    invalidateReferenceCache();
    status = "done";
    releaseGate!();

    const first = await inFlight;
    expect(first.status).toBe("ok");
    if (first.status === "ok") expect(first.issue.status).toBe("todo");
    const refreshed = await fetchIssueCached("LIF-5");
    expect(refreshed.status).toBe("ok");
    if (refreshed.status === "ok") expect(refreshed.issue.status).toBe("done");
    expect(calls).toBe(2);
  });

  test("suppresses a direct response from an old session", async () => {
    storage.setItem("lific_token", "direct-account-a");
    gatePath = "LIF-5";
    gate = new Promise<void>((resolve) => {
      releaseGate = resolve;
    });
    const stale = fetchIssueCached("LIF-5");
    await Promise.resolve();

    storage.setItem("lific_token", "direct-account-b");
    await fetchIssueCached("LIF-7");
    releaseGate!();

    expect((await stale).status).toBe("unavailable");
    expect(calls).toBe(2);
  });

  test("drops unsubscribed status work before it reaches the server", async () => {
    storage.setItem("lific_token", "cancelled");
    gate = new Promise<void>((resolve) => {
      releaseGate = resolve;
    });
    let settled = 0;
    const done = Promise.withResolvers<void>();
    const stops = Array.from({ length: 12 }, (_, i) =>
      subscribe(`LIF-${i + 20}`, () => {
        settled += 1;
        if (settled === 6) done.resolve();
      }),
    );

    for (const stop of stops.slice(6)) stop();
    releaseGate!();
    await done.promise;

    expect(calls).toBe(6);
    for (const stop of stops.slice(0, 6)) stop();
  });

  test("caps concurrent issue resolution", async () => {
    storage.setItem("lific_token", "bounded");
    gate = new Promise<void>((resolve) => {
      releaseGate = resolve;
    });
    let settled = 0;
    const done = Promise.withResolvers<void>();
    const stops = Array.from({ length: 12 }, (_, i) =>
      subscribe(`LIF-${i + 10}`, () => {
        settled += 1;
        if (settled === 12) done.resolve();
      }),
    );
    await Promise.resolve();
    expect(peakActive).toBe(6);
    releaseGate!();
    await done.promise;
    expect(calls).toBe(12);
    for (const stop of stops) stop();
  });

  test("releases active status work when its last subscriber leaves", async () => {
    storage.setItem("lific_token", "active-cancelled");
    gate = new Promise<void>((resolve) => {
      releaseGate = resolve;
    });
    const stops = Array.from({ length: 6 }, (_, i) =>
      subscribe(`LIF-${i + 60}`, () => {}),
    );
    await Promise.resolve();
    expect(calls).toBe(6);

    for (const stop of stops) stop();
    const delivered = Promise.withResolvers<void>();
    const stopNext = subscribe("LIF-99", () => delivered.resolve());
    for (let i = 0; i < 12 && calls < 7; i += 1) await Promise.resolve();

    expect(calls).toBe(7);
    releaseGate!();
    await delivered.promise;
    stopNext();
  });

  test("restarts a status request after immediate same-key resubscription", async () => {
    storage.setItem("lific_token", "status-resubscribe");
    gate = new Promise<void>((resolve) => {
      releaseGate = resolve;
    });
    const first = subscribe("LIF-98", () => {});
    await Promise.resolve();
    expect(calls).toBe(1);

    first();
    const results: string[] = [];
    const delivered = Promise.withResolvers<void>();
    const second = subscribe("LIF-98", (result) => {
      results.push(result.status);
      delivered.resolve();
    });

    releaseGate!();
    await delivered.promise;

    expect(results).toEqual(["ok"]);
    expect(calls).toBe(2);
    second();
  });

  test("an old cleanup cannot remove a replacement same-key subscription", async () => {
    storage.setItem("lific_token", "idempotent-cleanup");
    gate = new Promise<void>((resolve) => {
      releaseGate = resolve;
    });
    const first = subscribe("LIF-97", () => {});
    await Promise.resolve();

    first();
    const delivered = Promise.withResolvers<boolean>();
    const second = subscribe("LIF-97", () => delivered.resolve(true));
    first();
    expect(browserWindow.listenerCount(REALTIME_INVALIDATE_EVENT)).toBe(1);
    expect(browserWindow.listenerCount("focus")).toBe(1);
    releaseGate!();

    expect(await delivered.promise).toBe(true);
    second();
  });

  test("releases queue slots after abortable direct consumers leave", async () => {
    storage.setItem("lific_token", "direct-cancelled");
    gate = new Promise<void>((resolve) => {
      releaseGate = resolve;
    });
    const controllers = Array.from({ length: 6 }, () => new AbortController());
    const abandoned = controllers.map((controller, i) =>
      fetchIssueCached(`LIF-${i + 70}`, controller.signal),
    );
    await Promise.resolve();
    expect(calls).toBe(6);

    for (const controller of controllers) controller.abort();
    const nextController = new AbortController();
    const next = fetchIssueCached("LIF-99", nextController.signal);
    for (let i = 0; i < 12 && calls < 7; i += 1) await Promise.resolve();

    expect(calls).toBe(7);
    expect(
      (await Promise.all(abandoned)).map((result) => result.status),
    ).toEqual(Array(6).fill("unavailable"));
    releaseGate!();
    expect((await next).status).toBe("ok");
  });

  test("does not join an aborted queued request for the same identifier", async () => {
    storage.setItem("lific_token", "direct-same-key");
    gate = new Promise<void>((resolve) => {
      releaseGate = resolve;
    });
    const blockers = Array.from({ length: 6 }, (_, i) =>
      fetchIssueCached(`LIF-${i + 80}`, new AbortController().signal),
    );
    await Promise.resolve();
    expect(calls).toBe(6);

    const abandonedController = new AbortController();
    const abandoned = fetchIssueCached("LIF-99", abandonedController.signal);
    abandonedController.abort();
    const retry = fetchIssueCached("LIF-99", new AbortController().signal);

    releaseGate!();
    expect((await abandoned).status).toBe("unavailable");
    expect((await retry).status).toBe("ok");
    await Promise.all(blockers);
    expect(calls).toBe(7);
  });

  test("refreshes subscribers after the session changes during a request", async () => {
    storage.setItem("lific_token", "subscriber-account-a");
    status = "done";
    gatePath = "LIF-6";
    gate = new Promise<void>((resolve) => {
      releaseGate = resolve;
    });
    const results: Array<{ status: string; issueStatus?: string }> = [];
    const delivered = Promise.withResolvers<void>();
    const stop = subscribe("LIF-6", (result) => {
      results.push({
        status: result.status,
        issueStatus: result.status === "ok" ? result.issue.status : undefined,
      });
      delivered.resolve();
    });
    await Promise.resolve();

    storage.setItem("lific_token", "subscriber-account-b");
    status = "todo";
    await fetchIssueCached("LIF-7");
    releaseGate!();
    await delivered.promise;

    expect(results).toEqual([{ status: "ok", issueStatus: "todo" }]);
    expect(calls).toBe(3);
    stop();
  });

  test("shares one browser refresh listener and removes it after teardown", async () => {
    storage.setItem("lific_token", "listener");
    let deliveries = 0;
    let expectedDeliveries = 2;
    let delivered = Promise.withResolvers<void>();
    const record = () => {
      deliveries += 1;
      if (deliveries === expectedDeliveries) delivered.resolve();
    };
    const stopFirst = subscribe("LIF-8", () => record());
    const stopSecond = subscribe("LIF-8", () => record());
    await delivered.promise;
    expect(deliveries).toBe(2);
    expect(calls).toBe(1);
    expect(browserWindow.listenerCount(REALTIME_INVALIDATE_EVENT)).toBe(1);
    expect(browserWindow.listenerCount("focus")).toBe(1);

    status = "done";
    expectedDeliveries = 4;
    delivered = Promise.withResolvers<void>();
    const event = new Event(REALTIME_INVALIDATE_EVENT);
    Object.defineProperty(event, "detail", {
      value: { type: "issue.updated" },
    });
    window.dispatchEvent(event);
    await delivered.promise;
    expect(deliveries).toBe(4);
    expect(calls).toBe(2);

    stopFirst();
    expect(browserWindow.listenerCount(REALTIME_INVALIDATE_EVENT)).toBe(1);
    expect(browserWindow.listenerCount("focus")).toBe(1);
    stopSecond();
    expect(browserWindow.listenerCount(REALTIME_INVALIDATE_EVENT)).toBe(0);
    expect(browserWindow.listenerCount("focus")).toBe(0);
  });

  test("keeps a shared request alive for a direct caller after status teardown", async () => {
    storage.setItem("lific_token", "shared-direct");
    gate = new Promise<void>((resolve) => {
      releaseGate = resolve;
    });
    const stop = subscribe("LIF-9", () => {});
    await Promise.resolve();
    const direct = fetchIssueCached("LIF-9");

    stop();
    releaseGate!();

    expect((await direct).status).toBe("ok");
    expect(calls).toBe(1);
  });

  test("caps direct cached issue resolution", async () => {
    storage.setItem("lific_token", "direct-bounded");
    gate = new Promise<void>((resolve) => {
      releaseGate = resolve;
    });
    const pending = Array.from({ length: 12 }, (_, i) =>
      fetchIssueCached(`LIF-${i + 40}`),
    );

    await Promise.resolve();
    expect(peakActive).toBe(6);
    releaseGate!();
    await Promise.all(pending);
    expect(calls).toBe(12);
  });
});

describe("fetchModuleCached", () => {
  test("returns but does not cache module data invalidated in the same session", async () => {
    storage.setItem("lific_token", "same-session-module");
    moduleName = "Module A";
    gatePath = "/modules/";
    gate = new Promise<void>((resolve) => {
      releaseGate = resolve;
    });
    const inFlight = fetchModuleCached(1);
    await Promise.resolve();

    invalidateReferenceCache();
    moduleName = "Module B";
    releaseGate!();

    expect((await inFlight)?.name).toBe("Module A");
    expect((await fetchModuleCached(1))?.name).toBe("Module B");
    expect(calls).toBe(2);
  });

  test("does not return or cache module data from an old session", async () => {
    storage.setItem("lific_token", "account-a");
    gate = new Promise<void>((resolve) => {
      releaseGate = resolve;
    });
    const stale = fetchModuleCached(1);

    storage.setItem("lific_token", "account-b");
    moduleName = "Module B";
    const current = fetchModuleCached(1);
    releaseGate!();

    expect(await stale).toBeNull();
    expect((await current)?.name).toBe("Module B");
    expect(calls).toBe(2);
  });
});
