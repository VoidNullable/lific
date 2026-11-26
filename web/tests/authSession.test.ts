import { afterEach, beforeEach, describe, expect, test } from "bun:test";

// The session is the HttpOnly `lific_token` cookie, which page script cannot
// read. These pin down what api.ts keeps instead (signed in or not, and a key
// that changes with every session) and that it never holds a token itself.
class MemoryStorage {
  store = new Map<string, string>();
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

type Reply = { status: number; body: unknown } | { status: number; text: string };

let storage: MemoryStorage;
let calls: { url: string; auth: string | null }[];
let reply: Reply;
const originalFetch = globalThis.fetch;

beforeEach(() => {
  storage = new MemoryStorage();
  (globalThis as { localStorage: unknown }).localStorage = storage;
  (globalThis as { window?: unknown }).window ??= {
    location: { origin: "http://localhost" },
    addEventListener() {},
    removeEventListener() {},
  };
  calls = [];
  reply = { status: 200, body: {} };
  globalThis.fetch = (async (url: string, options: RequestInit = {}) => {
    calls.push({ url, auth: new Headers(options.headers).get("Authorization") });
    const current = reply;
    return {
      ok: current.status >= 200 && current.status < 300,
      status: current.status,
      json: async () => {
        if ("text" in current) throw new SyntaxError("Unexpected token");
        return current.body;
      },
    };
  }) as unknown as typeof fetch;
});

afterEach(() => {
  globalThis.fetch = originalFetch;
});

async function api() {
  return await import("../src/lib/api");
}

const USER = { id: 7, username: "blake", email: "b@x", display_name: "Blake", is_admin: false };

describe("signing in", () => {
  test("login sends no script-held credential and the answer carries no token", async () => {
    const { login, saveSession, hasSession, me } = await api();
    reply = { status: 200, body: { user: USER, expires_at: "2099-01-01T00:00:00Z" } };
    const result = await login("blake", "pw");
    expect(result.ok).toBe(true);
    if (result.ok) expect("token" in result.data).toBe(false);
    saveSession();
    expect(hasSession()).toBe(true);

    await me();
    expect(calls.every((call) => call.auth === null)).toBe(true);
  });

  test("a token left in localStorage by an older version is removed on load", async () => {
    storage.setItem("lific_token", "lific_sess_left_over");
    storage.setItem("lific:sidebar", "kept");
    await import("../src/lib/api.ts?leftover");
    expect(storage.getItem("lific_token")).toBeNull();
    expect(storage.getItem("lific:sidebar")).toBe("kept");
  });
});

describe("probeSession", () => {
  test("a live cookie signs the tab in", async () => {
    const { probeSession, clearSession, hasSession } = await api();
    clearSession();
    reply = { status: 200, body: USER };
    const result = await probeSession();
    expect(result.ok).toBe(true);
    expect(hasSession()).toBe(true);
  });

  test("a plain-text 401 from the auth layer signs the tab out", async () => {
    const { probeSession, saveSession, hasSession } = await api();
    saveSession();
    reply = { status: 401, text: "Invalid or expired session" };
    const result = await probeSession();
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.status).toBe(401);
    expect(hasSession()).toBe(false);
  });

  test("an unreachable server leaves the tab as it was", async () => {
    const { probeSession, saveSession, hasSession } = await api();
    saveSession();
    globalThis.fetch = (async () => {
      throw new TypeError("network down");
    }) as unknown as typeof fetch;
    const result = await probeSession();
    expect(result.ok).toBe(false);
    expect(hasSession()).toBe(true);
  });
});

describe("changePassword", () => {
  test("moves to the replacement session the server set as the cookie", async () => {
    const { changePassword, saveSession, sessionKey } = await api();
    saveSession();
    const before = sessionKey();
    reply = { status: 200, body: { ok: true, expires_at: "2099-01-01T00:00:00Z" } };

    const result = await changePassword({ current_password: "old", new_password: "new" });

    expect(result.ok).toBe(true);
    expect(sessionKey()).not.toBeNull();
    expect(sessionKey()).not.toBe(before);
  });

  test("leaves the current session alone when the change is rejected", async () => {
    const { changePassword, saveSession, sessionKey } = await api();
    saveSession();
    const before = sessionKey();
    reply = { status: 400, body: { error: "current password is incorrect" } };

    const result = await changePassword({ current_password: "wrong", new_password: "new" });

    expect(result.ok).toBe(false);
    expect(sessionKey()).toBe(before);
  });
});

describe("revokeAllSessions", () => {
  test("signs the tab out once the server confirms the revocation", async () => {
    const { revokeAllSessions, saveSession, hasSession } = await api();
    saveSession();
    reply = { status: 200, body: { revoked: true } };

    const result = await revokeAllSessions();

    expect(result.ok).toBe(true);
    expect(hasSession()).toBe(false);
  });

  test("stays signed in when the request fails, so a retry still works", async () => {
    const { revokeAllSessions, saveSession, hasSession } = await api();
    saveSession();
    reply = { status: 500, body: { error: "database error" } };

    const result = await revokeAllSessions();

    expect(result.ok).toBe(false);
    expect(hasSession()).toBe(true);
  });

  test("stays signed in when the server is unreachable", async () => {
    const { revokeAllSessions, saveSession, hasSession } = await api();
    saveSession();
    globalThis.fetch = (async () => {
      throw new TypeError("network down");
    }) as unknown as typeof fetch;

    const result = await revokeAllSessions();

    expect(result.ok).toBe(false);
    expect(hasSession()).toBe(true);
  });
});
