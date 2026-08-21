import { afterEach, beforeEach, describe, expect, test } from "bun:test";

// api.ts reads `localStorage` and `fetch` off the global at call time, so a
// plain object and a stub function are enough; no DOM is involved.
class MemoryStorage {
  private store = new Map<string, string>();
  getItem(key: string) {
    return this.store.has(key) ? this.store.get(key)! : null;
  }
  setItem(key: string, value: string) {
    this.store.set(key, String(value));
  }
  removeItem(key: string) {
    this.store.delete(key);
  }
}

type Reply = { status: number; body: unknown };

let storage: MemoryStorage;
let calls: { url: string; auth: string | undefined }[];
let reply: Reply;
const originalFetch = globalThis.fetch;

beforeEach(() => {
  storage = new MemoryStorage();
  (globalThis as { localStorage: unknown }).localStorage = storage;
  // api.ts derives the MCP URL from `window.location.origin` at module load.
  (globalThis as { window?: unknown }).window ??= {
    location: { origin: "http://localhost" },
  };
  calls = [];
  reply = { status: 200, body: {} };
  globalThis.fetch = (async (url: string, options: RequestInit = {}) => {
    const headers = (options.headers ?? {}) as Record<string, string>;
    calls.push({ url, auth: headers["Authorization"] });
    return {
      ok: reply.status >= 200 && reply.status < 300,
      status: reply.status,
      json: async () => reply.body,
    };
  }) as unknown as typeof fetch;
});

afterEach(() => {
  globalThis.fetch = originalFetch;
});

async function api() {
  return await import("../src/lib/api");
}

describe("changePassword", () => {
  test("adopts the replacement session so the next request is authenticated", async () => {
    const { changePassword, me } = await api();
    storage.setItem("lific_token", "lific_sess_old");
    reply = {
      status: 200,
      body: { ok: true, token: "lific_sess_new", expires_at: "2099-01-01T00:00:00Z" },
    };

    const result = await changePassword({
      current_password: "old",
      new_password: "new",
    });

    expect(result.ok).toBe(true);
    expect(storage.getItem("lific_token")).toBe("lific_sess_new");

    // The old token was revoked server-side by this very call, so the very
    // next request has to carry the replacement.
    reply = { status: 200, body: {} };
    await me();
    expect(calls.at(-1)?.auth).toBe("Bearer lific_sess_new");
  });

  test("leaves the current session alone when the change is rejected", async () => {
    const { changePassword } = await api();
    storage.setItem("lific_token", "lific_sess_old");
    reply = { status: 400, body: { error: "current password is incorrect" } };

    const result = await changePassword({
      current_password: "wrong",
      new_password: "new",
    });

    expect(result.ok).toBe(false);
    expect(storage.getItem("lific_token")).toBe("lific_sess_old");
  });
});

describe("revokeAllSessions", () => {
  test("clears the local session once the server confirms the revocation", async () => {
    const { revokeAllSessions } = await api();
    storage.setItem("lific_token", "lific_sess_old");
    reply = { status: 200, body: { revoked: true } };

    const result = await revokeAllSessions();

    expect(result.ok).toBe(true);
    expect(storage.getItem("lific_token")).toBeNull();
  });

  test("keeps the local session when the request fails, so the retry has a credential", async () => {
    const { revokeAllSessions } = await api();
    storage.setItem("lific_token", "lific_sess_old");
    reply = { status: 500, body: { error: "database error" } };

    const result = await revokeAllSessions();

    expect(result.ok).toBe(false);
    expect(storage.getItem("lific_token")).toBe("lific_sess_old");
  });

  test("keeps the local session when the server is unreachable", async () => {
    const { revokeAllSessions } = await api();
    storage.setItem("lific_token", "lific_sess_old");
    globalThis.fetch = (async () => {
      throw new TypeError("network down");
    }) as unknown as typeof fetch;

    const result = await revokeAllSessions();

    expect(result.ok).toBe(false);
    expect(storage.getItem("lific_token")).toBe("lific_sess_old");
  });
});
