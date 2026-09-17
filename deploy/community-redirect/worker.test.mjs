import assert from "node:assert/strict";
import test from "node:test";

import worker from "./worker.js";

test("the Worker replaces spoofable identity headers with an authenticated client IP", async () => {
  let forwarded;
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (_target, init) => {
    forwarded = new Headers(init.headers);
    return new Response(null, { status: 204 });
  };

  try {
    const request = new Request("https://community.lific.dev/public/api/projects/LIF", {
      headers: {
        "cf-connecting-ip": "203.0.113.7",
        "x-forwarded-for": "192.0.2.1",
        "x-real-ip": "192.0.2.2",
        "x-lific-client-ip": "192.0.2.3",
        "x-lific-proxy-secret": "attacker-secret",
      },
    });

    assert.equal(
      (await worker.fetch(request, { LIFIC_TRUSTED_PROXY_SECRET: "correct-secret" })).status,
      204,
    );
    assert.equal(forwarded.get("x-lific-client-ip"), "203.0.113.7");
    assert.equal(forwarded.get("x-lific-proxy-secret"), "correct-secret");
    assert.equal(forwarded.get("x-forwarded-for"), null);
    assert.equal(forwarded.get("x-real-ip"), null);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("the Worker fails closed when its proxy secret is missing", async () => {
  let fetched = false;
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async () => {
    fetched = true;
    return new Response(null, { status: 204 });
  };

  try {
    const request = new Request("https://community.lific.dev/", {
      headers: { "cf-connecting-ip": "203.0.113.7" },
    });
    assert.equal((await worker.fetch(request, {})).status, 503);
    assert.equal(fetched, false);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("the Worker fails closed when Cloudflare's client identity is missing", async () => {
  let fetched = false;
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async () => {
    fetched = true;
    return new Response(null, { status: 204 });
  };

  try {
    const request = new Request("https://community.lific.dev/");
    assert.equal(
      (await worker.fetch(request, { LIFIC_TRUSTED_PROXY_SECRET: "correct-secret" })).status,
      503,
    );
    assert.equal(fetched, false);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("HTTP requests retain their body and return redirects without forwarding credentials again", async () => {
  const calls = [];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (target, init) => {
    calls.push({ target, init });
    return new Response(null, {
      status: 307,
      headers: { location: "https://elsewhere.example/" },
    });
  };

  try {
    const request = new Request("https://community.lific.dev/api/auth/login?next=issues", {
      method: "POST",
      headers: { "cf-connecting-ip": "203.0.113.7", "content-type": "application/json" },
      body: '{"username":"example"}',
    });
    const response = await worker.fetch(request, { LIFIC_TRUSTED_PROXY_SECRET: "correct-secret" });
    assert.equal(response.status, 307);
    assert.equal(response.headers.get("location"), "https://elsewhere.example/");
    assert.equal(calls.length, 1);
    assert.equal(calls[0].target, "https://lific-community.fly.dev/api/auth/login?next=issues");
    assert.equal(calls[0].init.method, "POST");
    assert.equal(calls[0].init.redirect, "manual");
    assert.equal(await new Response(calls[0].init.body).text(), '{"username":"example"}');
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("WebSockets are limited to the event route and never follow redirects", async () => {
  const calls = [];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (_target, init) => {
    calls.push(init);
    return new Response(null, { status: 204 });
  };

  try {
    const headers = {
      "cf-connecting-ip": "203.0.113.7",
      upgrade: "websocket",
    };
    const env = { LIFIC_TRUSTED_PROXY_SECRET: "correct-secret" };
    const websocket = new Request("https://community.lific.dev/api/events/ws", { headers });
    assert.equal((await worker.fetch(websocket, env)).status, 204);
    assert.equal(calls[0].redirect, "manual");

    const wrongRoute = new Request("https://community.lific.dev/oauth/register", { headers });
    assert.equal((await worker.fetch(wrongRoute, env)).status, 400);
    const wrongMethod = new Request("https://community.lific.dev/api/events/ws", {
      headers,
      method: "POST",
    });
    assert.equal((await worker.fetch(wrongMethod, env)).status, 400);
    assert.equal(calls.length, 1);
  } finally {
    globalThis.fetch = originalFetch;
  }
});
