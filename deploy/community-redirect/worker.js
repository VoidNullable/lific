// community.lific.dev: reverse proxy for the community Lific instance on Fly.
// The browser stays on this hostname; every request, WebSockets included, is
// forwarded to the Fly origin with the public hostname as Host so the app
// generates links, cookies and OAuth metadata for the address people see.
const ORIGIN = "https://lific-community.fly.dev";
const PUBLIC_HOST = "community.lific.dev";

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.hostname !== PUBLIC_HOST) {
      return new Response("not found", { status: 404 });
    }
    const proxySecret = env.LIFIC_TRUSTED_PROXY_SECRET;
    const clientIp = request.headers.get("cf-connecting-ip");
    if (!proxySecret || !clientIp) {
      return new Response("service unavailable", { status: 503 });
    }
    const target = ORIGIN + url.pathname + url.search;
    const headers = new Headers(request.headers);
    headers.set("host", PUBLIC_HOST);
    headers.set("x-forwarded-host", PUBLIC_HOST);
    headers.set("x-forwarded-proto", "https");
    // Replace every caller-controlled identity header. Lific accepts the
    // dedicated client IP only from a trusted Fly peer with this secret.
    headers.delete("x-forwarded-for");
    headers.delete("x-real-ip");
    headers.set("x-lific-client-ip", clientIp);
    headers.set("x-lific-proxy-secret", proxySecret);

    const upgrade = request.headers.get("upgrade");
    if (upgrade && upgrade.toLowerCase() === "websocket") {
      if (request.method !== "GET" || url.pathname !== "/api/events/ws") {
        return new Response("invalid websocket route", { status: 400 });
      }
      return fetch(target, { headers, method: request.method, redirect: "manual" });
    }
    return fetch(target, {
      method: request.method,
      headers,
      body: request.body,
      redirect: "manual",
    });
  },
};
