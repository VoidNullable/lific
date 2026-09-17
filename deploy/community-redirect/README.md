# Community reverse proxy

This Worker forwards `community.lific.dev` to the Fly application. It replaces
caller-supplied identity headers with Cloudflare's `CF-Connecting-IP` and an
authenticated `X-Lific-Client-IP` / `X-Lific-Proxy-Secret` pair. Only
`GET /api/events/ws` accepts a WebSocket upgrade. Origin redirects are returned
to the caller without forwarding the shared secret to another destination.

## Shared secret and rollout order

Both the Worker and the Lific server process require the same nonempty
`LIFIC_TRUSTED_PROXY_SECRET`. Generate a high-entropy random value and install it
through each platform's secret manager. Keep it out of TOML, Wrangler variables,
source control, command-line arguments, and logs. Wrangler accepts it through
the interactive `bunx wrangler secret put LIFIC_TRUSTED_PROXY_SECRET` prompt.
The Lific process also needs the Fly proxy peers in `server.trusted_proxies`;
possession of the secret alone does not make an untrusted network peer trusted.

For the first upgrade, install the secret on Fly and deploy the Lific binary
that understands these headers **before deploying this Worker**. The existing
Worker remains compatible because requests without either dedicated header
continue to use the ordinary trusted-proxy forwarding chain. Then install the
same secret on the Worker and deploy it with `bunx wrangler deploy` from this
directory. A missing Worker secret makes all proxied requests return 503. A
missing or mismatched server secret makes requests that resolve proxy identity
return 503, including login and public API reads.

After deployment, check public API reads, password login, and the authenticated
events WebSocket through the public hostname. Caller-supplied dedicated headers
must be replaced by the Worker. A direct origin request with an invalid dedicated
header pair must fail with 503 on an IP-limited endpoint. Requests without the
pair still use the ordinary forwarding chain; the secret does not require all
traffic to pass through the Worker or replace application authentication.

Secret rotation requires coordinating both services: only one value is accepted
at a time, so changing one side first temporarily rejects proxied IP-limited
requests. Roll the Worker back before reverting the server to a binary that
does not understand authenticated proxy identity.

## Tests

Run `bun test ./deploy/community-redirect/worker.test.mjs` from the repository
root. The CI web job runs these tests on every pull request.
