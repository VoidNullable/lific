import { afterAll, afterEach, beforeEach, describe, expect, test } from "bun:test";
import { readModelAccount, setReadModelAccount, readModelScope } from "../src/lib/sync/account";
import { isSignedOut, setSignedOut } from "../src/lib/signedOut";

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
  setReadModelAccount(undefined);
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

describe("read-model account identity", () => {
  test("an ordinary identity read keeps the session key used by its caller", async () => {
    const { me, sessionKey } = await api();
    const before = sessionKey();
    reply = { status: 200, body: { id: 7 } };
    await me();
    expect(readModelAccount()).toBe(7);
    expect(sessionKey()).toBe(before);
  });

  test("a different confirmed account invalidates the old session's views", async () => {
    const { me, saveSession, sessionKey } = await api();
    setReadModelAccount(7);
    saveSession();
    const before = sessionKey();
    reply = { status: 200, body: { id: 8 } };
    await me();
    expect(readModelAccount()).toBe(8);
    expect(sessionKey()).not.toBe(before);
  });

  test("signed-out state and the disk scope share the confirmed account", async () => {
    const { probeSession, saveSession } = await api();
    setSignedOut(true);
    expect(readModelAccount()).toBeNull();
    expect(readModelScope()).toBe("signed-out:");
    expect(isSignedOut()).toBe(readModelAccount() === null);
    reply = { status: 200, body: { id: 8 } };
    await probeSession();
    expect(readModelScope()).toBe("private:8:");
    expect(isSignedOut()).toBe(false);
    saveSession();
    expect(readModelScope()).toBe("private:8:");
    reply = { status: 200, body: { id: 9 } };
    await probeSession();
    expect(readModelScope()).toBe("private:9:");
    expect(isSignedOut()).toBe(readModelAccount() === null);
  });

  test("only adopts the server-confirmed account", async () => {
    const { me } = await api();
    expect(readModelAccount()).toBeUndefined();
    reply = { status: 200, body: { id: 7 } };
    await me();
    expect(readModelAccount()).toBe(7);
  });

  test("an expired session clears the account", async () => {
    const { me } = await api();
    setReadModelAccount(7);
    reply = { status: 401, body: { error: "expired" } };
    await me();
    expect(readModelAccount()).toBeNull();
    expect(isSignedOut()).toBe(true);
  });

  test("a late identity response cannot undo sign-out", async () => {
    const { me, clearSession } = await api();
    setReadModelAccount(7);
    let release!: (response: Response) => void;
    globalThis.fetch = (() => new Promise<Response>(resolve => { release = resolve; })) as typeof fetch;
    const pending = me();
    await clearSession();
    release(new Response(JSON.stringify({ id: 7 })));
    await pending;
    expect(readModelAccount()).toBeNull();
  });
});

afterEach(() => {
  globalThis.fetch = originalFetch;
});

async function api() {
  return await import("../src/lib/api");
}

const USER = { id: 7, username: "blake", email: "b@x", display_name: "Blake", is_admin: false };

describe("signing in", () => {
  test("login confirms the account before saveSession publishes the new session", async () => {
    const { login, clearSession, saveSession, hasSession } = await api();
    await clearSession();
    globalThis.fetch = (async (url: string) => new Response(JSON.stringify(
      url.endsWith("/auth/me") ? USER : { user: USER, expires_at: "2099-01-01" },
    ))) as typeof fetch;
    const result = await login("blake", "pw");
    expect(result.ok).toBe(true);
    expect(readModelAccount()).toBe(USER.id);
    expect(hasSession()).toBe(false);
    saveSession();
    expect(hasSession()).toBe(true);
    expect(readModelAccount()).toBe(USER.id);
  });

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
    // A second copy of the module. Without a channel of its own, so it
    // cannot answer the cross-tab notices the tests below send.
    const channel = globalThis.BroadcastChannel;
    (globalThis as { BroadcastChannel?: unknown }).BroadcastChannel = undefined;
    try {
      await import("../src/lib/api.ts?leftover");
    } finally {
      globalThis.BroadcastChannel = channel;
    }
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

describe("logout", () => {
  test("signs the tab out once the server confirms", async () => {
    const { logout, saveSession, hasSession } = await api();
    saveSession();
    reply = { status: 200, body: { logged_out: true } };

    const result = await logout();

    expect(result.ok).toBe(true);
    expect(hasSession()).toBe(false);
  });

  test("a 401 means there was no session left, so the tab is signed out", async () => {
    const { logout, saveSession, hasSession } = await api();
    saveSession();
    reply = { status: 401, text: "Invalid or expired session" };

    const result = await logout();

    expect(result.ok).toBe(true);
    expect(hasSession()).toBe(false);
  });

  test("a failed sign-out keeps the tab signed in and reports the error", async () => {
    const { logout, saveSession, hasSession, sessionKey } = await api();
    saveSession();
    const before = sessionKey();
    reply = { status: 503, body: { error: "service unavailable" } };

    const result = await logout();

    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.status).toBe(503);
    expect(hasSession()).toBe(true);
    expect(sessionKey()).toBe(before);
  });

  test("an unreachable server keeps the tab signed in", async () => {
    const { logout, saveSession, hasSession } = await api();
    saveSession();
    globalThis.fetch = (async () => {
      throw new TypeError("network down");
    }) as unknown as typeof fetch;

    const result = await logout();

    expect(result.ok).toBe(false);
    expect(hasSession()).toBe(true);
  });
});

describe("another tab's session notice", () => {
  // Replies to `/api/auth/me` that the test releases one at a time.
  let pending: ((reply: Reply) => void)[];

  beforeEach(() => {
    pending = [];
    globalThis.fetch = (async (url: string) => {
      calls.push({ url, auth: null });
      const current = await new Promise<Reply>((resolve) => pending.push(resolve));
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

  async function until(condition: () => boolean) {
    for (let i = 0; i < 200 && !condition(); i++) await Bun.sleep(1);
    expect(condition()).toBe(true);
  }

  // Another tab. Closing a channel right after posting can drop the message,
  // so it stays open for the file.
  const otherTab = new BroadcastChannel("lific:session");
  afterAll(() => otherTab.close());

  function announce(message: unknown) {
    otherTab.postMessage(message);
  }

  test("is not taken at its word: the tab asks the server", async () => {
    const { saveSession, hasSession, sessionKey } = await api();
    saveSession();
    const before = sessionKey();

    announce("signed-out");
    await until(() => pending.length === 1);
    expect(calls.at(-1)?.url).toBe("/api/auth/me");
    // Unknown until the server answers: no work may start under the old key.
    expect(sessionKey()).toBeNull();

    pending.shift()!({ status: 200, body: USER });
    await until(() => sessionKey() !== null);
    expect(hasSession()).toBe(true);
    expect(sessionKey()).not.toBe(before);
  });

  test("a claimed sign-in the server does not back signs the tab out", async () => {
    const { clearSession, hasSession, sessionKey } = await api();
    clearSession();

    announce("signed-in");
    await until(() => pending.length === 1);
    pending.shift()!({ status: 401, text: "Missing credentials" });
    await Bun.sleep(5);

    expect(hasSession()).toBe(false);
    expect(sessionKey()).toBeNull();
  });

  test("an answer overtaken by a newer notice is dropped", async () => {
    const { saveSession, hasSession, sessionKey } = await api();
    saveSession();

    announce("changed");
    await until(() => pending.length === 1);
    announce("changed");
    await until(() => pending.length === 2);
    const [first, second] = pending.splice(0);

    // The newer probe says signed out; the older one, answering late, must
    // not sign the tab back in.
    second({ status: 401, text: "Invalid or expired session" });
    await until(() => !hasSession());
    first({ status: 200, body: USER });
    await Bun.sleep(5);

    expect(hasSession()).toBe(false);
    expect(sessionKey()).toBeNull();
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
