import { afterEach, beforeEach, describe, expect, test } from "bun:test";

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
let calls: string[];
let bodies: (string | undefined)[];
/** Replies keyed by request path suffix, consumed in order per path. */
let replies: Map<string, Reply[]>;
const originalFetch = globalThis.fetch;

function reply(path: string, ...queued: Reply[]) {
  replies.set(path, queued);
}

beforeEach(() => {
  storage = new MemoryStorage();
  (globalThis as { localStorage: unknown }).localStorage = storage;
  (globalThis as { window?: unknown }).window ??= {
    location: { origin: "http://localhost" },
  };
  calls = [];
  bodies = [];
  replies = new Map();
  globalThis.fetch = (async (url: string, options: RequestInit = {}) => {
    calls.push(url);
    bodies.push(options.body as string | undefined);
    const queued = [...replies.entries()].find(([path]) => url.endsWith(path))?.[1];
    const next = queued && queued.length > 1 ? queued.shift()! : queued?.[0];
    const chosen = next ?? { status: 200, body: {} };
    return {
      ok: chosen.status >= 200 && chosen.status < 300,
      status: chosen.status,
      json: async () => chosen.body,
    };
  }) as unknown as typeof fetch;
});

afterEach(() => {
  globalThis.fetch = originalFetch;
});

async function mod() {
  return await import("../src/lib/reauth");
}

const STALE = { status: 403, body: { error: "recent authentication required" } };
const session = (id: number, token: string) => ({
  status: 200,
  body: { user: { id, username: "blake" }, token, expires_at: "2099-01-01T00:00:00Z" },
});

describe("needsReauth", () => {
  test("matches only the staleness refusal, not other 403s", async () => {
    const { needsReauth } = await mod();
    expect(
      needsReauth({ ok: false, error: "recent authentication required", status: 403 }),
    ).toBe(true);
    expect(needsReauth({ ok: false, error: "only an admin can do this", status: 403 })).toBe(
      false,
    );
    expect(needsReauth({ ok: false, error: "authentication required", status: 403 })).toBe(
      false,
    );
    expect(
      needsReauth({ ok: false, error: "recent authentication required", status: 401 }),
    ).toBe(false);
  });
});

describe("password re-authentication", () => {
  test("saves the fresh session so the retry carries it", async () => {
    const { reauthenticateWithPassword } = await mod();
    storage.setItem("lific_token", "lific_sess_stale");
    reply("/auth/me/refresh", session(7, "lific_sess_fresh"));

    const outcome = await reauthenticateWithPassword("hunter2", 7);

    expect(outcome.ok).toBe(true);
    expect(storage.getItem("lific_token")).toBe("lific_sess_fresh");
  });

  test("a wrong password leaves the existing session in place", async () => {
    const { reauthenticateWithPassword } = await mod();
    storage.setItem("lific_token", "lific_sess_stale");
    reply("/auth/me/refresh", { status: 401, body: { error: "invalid credentials" } });

    const outcome = await reauthenticateWithPassword("wrong", 7);

    expect(outcome).toEqual({
      ok: false,
      error: "invalid credentials",
      // The human already typed a password; re-offering the same prompt with
      // no error shown would be a loop.
      recoverable: false,
    });
    expect(storage.getItem("lific_token")).toBe("lific_sess_stale");
  });

  test("a network failure leaves the existing session in place", async () => {
    const { reauthenticateWithPassword } = await mod();
    storage.setItem("lific_token", "lific_sess_stale");
    globalThis.fetch = (async () => {
      throw new TypeError("network down");
    }) as unknown as typeof fetch;

    const outcome = await reauthenticateWithPassword("hunter2", 7);

    expect(outcome.ok).toBe(false);
    expect(storage.getItem("lific_token")).toBe("lific_sess_stale");
  });
});

describe("passwordless re-authentication", () => {
  test("saves the fresh session on a passwordless instance", async () => {
    const { reauthenticateWithoutPassword } = await mod();
    storage.setItem("lific_token", "lific_sess_stale");
    reply("/auth/me/refresh", session(7, "lific_sess_fresh"));

    const outcome = await reauthenticateWithoutPassword(7);

    expect(outcome.ok).toBe(true);
    expect(storage.getItem("lific_token")).toBe("lific_sess_fresh");
  });

  /** Auto-login mints a session for *the instance's* admin, which on a
   *  multi-admin instance need not be the admin using this tab. Saving it
   *  would silently swap identity, and the pending grant would then be
   *  performed by someone else. */
  test("refuses a session belonging to a different admin", async () => {
    const { reauthenticateWithoutPassword } = await mod();
    storage.setItem("lific_token", "lific_sess_stale");
    reply("/auth/me/refresh", session(99, "lific_sess_other_admin"));

    const outcome = await reauthenticateWithoutPassword(7);

    expect(outcome.ok).toBe(false);
    // The other admin's token must never be adopted.
    expect(storage.getItem("lific_token")).toBe("lific_sess_stale");
  });

  test("the same identity check applies to the password path", async () => {
    const { reauthenticateWithPassword } = await mod();
    storage.setItem("lific_token", "lific_sess_stale");
    reply("/auth/me/refresh", session(99, "lific_sess_other"));

    const outcome = await reauthenticateWithPassword("hunter2", 7);

    expect(outcome.ok).toBe(false);
    expect(storage.getItem("lific_token")).toBe("lific_sess_stale");
  });
});

describe("retryOnceAfterReauth", () => {
  test("passes a successful attempt straight through without re-authenticating", async () => {
    const { retryOnceAfterReauth } = await mod();
    let attempts = 0;
    let reauths = 0;

    const result = await retryOnceAfterReauth(
      async () => {
        attempts += 1;
        return { ok: true as const, data: "minted" };
      },
      async () => {
        reauths += 1;
        return { ok: true as const };
      },
    );

    expect(result).toEqual({ ok: true, data: "minted" });
    expect(attempts).toBe(1);
    expect(reauths).toBe(0);
  });

  test("re-authenticates once and retries once", async () => {
    const { retryOnceAfterReauth } = await mod();
    let attempts = 0;
    let reauths = 0;

    const result = await retryOnceAfterReauth(
      async () => {
        attempts += 1;
        if (attempts === 1) {
          return { ok: false as const, error: "recent authentication required", status: 403 };
        }
        return { ok: true as const, data: "minted" };
      },
      async () => {
        reauths += 1;
        return { ok: true as const };
      },
    );

    expect(result).toEqual({ ok: true, data: "minted" });
    expect(attempts).toBe(2);
    expect(reauths).toBe(1);
  });

  /** The loop guard: a second staleness refusal is returned, not retried. */
  test("never retries more than once, even if it is refused again", async () => {
    const { retryOnceAfterReauth } = await mod();
    let attempts = 0;
    let reauths = 0;

    const result = await retryOnceAfterReauth(
      async () => {
        attempts += 1;
        return { ok: false as const, error: "recent authentication required", status: 403 };
      },
      async () => {
        reauths += 1;
        return { ok: true as const };
      },
    );

    expect(result.ok).toBe(false);
    expect(attempts).toBe(2);
    expect(reauths).toBe(1);
  });

  test("does not retry when re-authentication itself fails", async () => {
    const { retryOnceAfterReauth } = await mod();
    let attempts = 0;

    const result = await retryOnceAfterReauth(
      async () => {
        attempts += 1;
        return { ok: false as const, error: "recent authentication required", status: 403 };
      },
      async () => ({ ok: false as const, error: "invalid credentials" }),
    );

    expect(result).toEqual({ ok: false, error: "invalid credentials", status: 403 });
    expect(attempts).toBe(1);
  });

  test("a non-staleness failure is returned untouched", async () => {
    const { retryOnceAfterReauth } = await mod();
    let reauths = 0;

    const result = await retryOnceAfterReauth(
      async () => ({ ok: false as const, error: "only an admin can do this", status: 403 }),
      async () => {
        reauths += 1;
        return { ok: true as const };
      },
    );

    expect(result).toEqual({ ok: false, error: "only an admin can do this", status: 403 });
    expect(reauths).toBe(0);
  });
});

describe("passwordless failure falls back to the password prompt", () => {
  test("a refused passwordless refresh is recoverable so callers can prompt", async () => {
    const { reauthenticateWithoutPassword } = await mod();
    storage.setItem("lific_token", "lific_sess_stale");
    reply("/auth/me/refresh", { status: 400, body: { error: "your password is required to confirm this" } });

    const outcome = await reauthenticateWithoutPassword(7);

    expect(outcome).toEqual({
      ok: false,
      error: "your password is required to confirm this",
      recoverable: true,
    });
    expect(storage.getItem("lific_token")).toBe("lific_sess_stale");
  });

  test("a refresh naming another account is recoverable, and the token is untouched", async () => {
    const { reauthenticateWithoutPassword } = await mod();
    storage.setItem("lific_token", "lific_sess_stale");
    reply("/auth/me/refresh", session(99, "lific_sess_other_admin"));

    const outcome = await reauthenticateWithoutPassword(7);

    expect(outcome.ok).toBe(false);
    expect(outcome.ok === false && outcome.recoverable).toBe(true);
    expect(storage.getItem("lific_token")).toBe("lific_sess_stale");
  });

  /** The decision callers make: a recoverable failure comes back out of
   *  `retryOnceAfterReauth` still looking like a staleness refusal, so their
   *  `needsReauth` branch fires and shows the password prompt. */
  test("a recoverable failure surfaces as staleness so the caller prompts", async () => {
    const { retryOnceAfterReauth, needsReauth } = await mod();

    const result = await retryOnceAfterReauth(
      async () => ({ ok: false as const, error: "recent authentication required", status: 403 }),
      async () => ({
        ok: false as const,
        error: "your password is required to confirm this",
        recoverable: true,
      }),
    );

    expect(result.ok).toBe(false);
    expect(result.ok === false && needsReauth(result)).toBe(true);
  });

  test("an unrecoverable failure surfaces its own message instead", async () => {
    const { retryOnceAfterReauth, needsReauth } = await mod();

    const result = await retryOnceAfterReauth(
      async () => ({ ok: false as const, error: "recent authentication required", status: 403 }),
      async () => ({ ok: false as const, error: "invalid credentials", recoverable: false }),
    );

    expect(result).toEqual({ ok: false, error: "invalid credentials", status: 403 });
    expect(result.ok === false && needsReauth(result)).toBe(false);
  });

  /** The whole recovery arc: the passwordless refresh is refused (this
   *  instance wants a password after all), the human types one, and the
   *  pending operation runs exactly once more. */
  test("a refused passwordless refresh then a successful password path retries once", async () => {
    const { reauthenticateWithoutPassword, reauthenticateWithPassword, retryOnceAfterReauth } =
      await mod();
    storage.setItem("lific_token", "lific_sess_stale");
    // First call to the refresh endpoint is refused, the second succeeds.
    reply(
      "/auth/me/refresh",
      { status: 400, body: { error: "your password is required to confirm this" } },
      session(7, "lific_sess_mine"),
    );

    let attempts = 0;
    const pending = async () => {
      attempts += 1;
      return storage.getItem("lific_token") === "lific_sess_mine"
        ? { ok: true as const, data: "granted" }
        : { ok: false as const, error: "recent authentication required", status: 403 };
    };

    // Passwordless route first: refused, token untouched, caller must prompt.
    const auto = await retryOnceAfterReauth(pending, () => reauthenticateWithoutPassword(7));
    expect(auto.ok).toBe(false);
    expect(storage.getItem("lific_token")).toBe("lific_sess_stale");
    expect(attempts).toBe(1);

    // Password route: adopted, and the pending operation runs once more.
    const verified = await reauthenticateWithPassword("hunter2", 7);
    expect(verified.ok).toBe(true);
    expect(storage.getItem("lific_token")).toBe("lific_sess_mine");

    const retried = await pending();
    expect(retried).toEqual({ ok: true, data: "granted" });
    expect(attempts).toBe(2);
  });

  /** Both routes must hit the same-user refresh endpoint, never `/auth/login`
   *  or `/auth/auto-login`, which can sign in as somebody else. */
  test("both routes call the same-user refresh endpoint", async () => {
    const { reauthenticateWithoutPassword, reauthenticateWithPassword } = await mod();
    reply("/auth/me/refresh", session(7, "lific_sess_fresh"));

    await reauthenticateWithoutPassword(7);
    await reauthenticateWithPassword("hunter2", 7);

    expect(calls).toEqual(["/api/auth/me/refresh", "/api/auth/me/refresh"]);
    expect(bodies).toEqual([JSON.stringify({}), JSON.stringify({ password: "hunter2" })]);
  });
});
