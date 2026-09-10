// community.lific.dev: reverse proxy for the community Lific instance on Fly.
// The browser stays on this hostname; every request, WebSockets included, is
// forwarded to the Fly origin with the public hostname as Host so the app
// generates links, cookies and OAuth metadata for the address people see.
const ORIGIN = "https://lific-community.fly.dev";
const PUBLIC_HOST = "community.lific.dev";

export default {
  async fetch(request) {
    const url = new URL(request.url);
    if (url.hostname !== PUBLIC_HOST) {
      return new Response("not found", { status: 404 });
    }
    const target = ORIGIN + url.pathname + url.search;
    const headers = new Headers(request.headers);
    headers.set("host", PUBLIC_HOST);
    headers.set("x-forwarded-host", PUBLIC_HOST);
    headers.set("x-forwarded-proto", "https");
    // The app trusts forwarding headers only from Fly's proxy, which appends
    // the real chain itself; nothing here should pretend to be the client.
    headers.delete("x-forwarded-for");
    headers.delete("x-real-ip");

    const upgrade = request.headers.get("upgrade");
    if (upgrade && upgrade.toLowerCase() === "websocket") {
      return fetch(target, { headers, method: request.method });
    }
    return fetch(target, {
      method: request.method,
      headers,
      body: request.body,
      redirect: "manual",
    });
  },
};
