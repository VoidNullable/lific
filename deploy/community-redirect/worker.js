// community.lific.dev: a redirect to the public LIF tracker on Fly.
// Any path is sent to the public project view; nothing is proxied.
const TARGET = "https://lific-community.fly.dev/#/public/LIF";

export default {
  fetch(request) {
    return new Response(null, {
      status: 302,
      headers: {
        location: TARGET,
        "cache-control": "no-store",
        "referrer-policy": "no-referrer",
        "x-content-type-options": "nosniff",
      },
    });
  },
};
