# Public projects

A project can be published so that anyone can read its issues without an
account. Publication is per project, off by default, and reversible.

The published address is derived from the project identifier:

```
https://your-instance.example/public/LIF
```

There is no token in it. This is a genuinely public page, not a secret link:
search engines, scrapers and anyone who is sent the URL reach it the same way.
If you need "readable by a specific group", add those people as project
members instead.

## What becomes public

Publishing `LIF` exposes, to everyone:

- every **current issue** in the project: title, description, status,
  priority, module, labels, and the created/updated timestamps;
- every **current comment** on those issues;
- every **attachment linked to** one of those issues or comments, downloadable
  by anyone who can reach the instance.

"Current" means not in the trash. A deleted issue, a deleted comment and any
attachment whose only link was to one of them drop out of the public view
immediately, without waiting for the retention sweep.

## What stays private

The public read path excludes the following. This does not secure other routes
if instance authentication is disabled:

- pages and page comments (including attachments linked only to a page);
- plans and plan steps;
- activity, audit history and status transitions;
- the trash;
- the member roster and every kind of account metadata, including **comment
  authors**: a published comment shows its text and its timestamp, never who
  wrote it;
- other projects, published or not;
- global search, exports, delta sync, the REST API and MCP, all of which keep
  the authentication they had.

Issue relations are not published either. A public issue does not list what it
blocks or duplicates, because the other end of a relation can live in a project
that was never published.

## Publishing a project

Project overview → **Public issue view**. You need to be the project **lead**
or an instance **admin**; the same gate that guards renaming a project and
naming its lead. Unlike the rest of the project settings, the panel does not
appear for everyone when `authz_enforced` is off: publication is the one
capability legacy mode still restricts, so the UI restricts it too rather than
offering a disclosure button to someone the server would refuse.

The panel asks you to tick an acknowledgement before the publish button
becomes usable. That is deliberate. Publishing exposes content that already
exists, not just content written afterwards, so the useful moment to review an
old issue body for an internal hostname or a customer name is before you press
the button, not after.

Every publish and unpublish is written to the project's activity log with the
account that did it.

## Unpublishing

The same panel, one press, no confirmation: the safe direction is never gated.

Unpublishing takes effect on the next request. Every public read (the list,
an issue, its comments, an attachment download) re-checks the flag in the
same query that fetches the content, so there is no window where a page that
was open a second ago keeps working. Public responses are served `no-store`,
so nothing sits in a browser or proxy cache either.

**It cannot recall what has already been downloaded.** A file someone saved,
or a page a crawler indexed, is out. Treat unpublishing as closing a door, not
as an undo.

Republishing later restores the *same* address. The URL is derived from the
identifier rather than minted, so old links start working again.

## The public HTTP surface

Five read-only endpoints, all under `/public/api`, all `GET`. Any other method
is `405`. None of them accepts or reads a credential of any kind.

| Endpoint | Answers |
| --- | --- |
| `GET /public/api/projects/{PROJ}` | the project's name, identifier, description, emoji |
| `GET /public/api/projects/{PROJ}/issues` | one page of current issues (`?limit=` up to 100, `?offset=`, with `has_more`) |
| `GET /public/api/projects/{PROJ}/issues/{PROJ-42}` | one issue with its body and attachment list |
| `GET /public/api/projects/{PROJ}/issues/{PROJ-42}/comments` | one page of current comments (`?limit=` up to 50, `?offset=`, with `total` and `has_more`) |
| `GET /public/api/projects/{PROJ}/attachments/{id}` | the file's bytes |

Both lists are paginated the same way. `limit` defaults to its ceiling (100
issues, 50 comments) and is **clamped**, not rejected, so `?limit=99999` returns
a full page rather than an error. `limit` and `offset` are echoed back, and
`has_more` is derived from a row fetched past the end of the page, so a project
holding exactly one page reports `has_more: false`. Walk either list by adding
the returned page length to `offset` until `has_more` is false; the comment
response also carries `total`, so a client can show what paging will reach.

Each JSON response has a **512 KiB limit**, including project metadata, labels,
attachment metadata and the response envelope. Before loading text into Rust,
SQLite checks a conservative bound: six times each field's UTF-8 byte length
(`length(CAST(field AS BLOB))`), plus fixed allowances for JSON keys, punctuation,
nulls and numbers. This counts embedded NULs and covers JSON escaping. It can
reject content whose actual serialized size would fit; reported sizes are
estimates, not exact JSON lengths. Preflight and reads share one snapshot.

A list returns the longest consecutive prefix that fits this estimate. Add the
number of returned issues or comments to `offset` to continue. If the first row
cannot fit with its complete metadata and envelope, the response is `413`.
Oversized project metadata or issue details also return `413`. Attachment lists
are complete, never truncated: each attachment reserves at least 256 bytes of
the budget, and each label reserves at least 32. File downloads are streamed
separately and do not have the JSON size limit.

Five properties worth knowing if you are building against these:

- **A private or nonexistent project answers identically.** Both are a `404`
  with the same body. You cannot use this surface to find out which projects
  an instance holds.
- **The project is part of every path, including the attachment one.** An
  issue identifier that names a different project (`/public/DEMO/issues/PRIV-1`)
  is rejected before anything is read, and an attachment id that is not
  reachable from a live issue or comment in *this* published project is a
  `404` even if the id exists.
- **Responses carry `Cache-Control: no-store`**, plus `nosniff`,
  `Referrer-Policy: no-referrer`, `X-Frame-Options: DENY` and
  `Cross-Origin-Resource-Policy: same-origin`. Downloads additionally carry
  `Content-Security-Policy: default-src 'none'; sandbox`, and anything that is
  not a plain raster image or a media container is served as an attachment
  rather than rendered in place (an SVG is served as
  `application/octet-stream`, since an SVG is a document that can run script).
- **The ordinary API is untouched.** `/api/...` keeps its existing authentication
  settings; the public routes are a separate router mounted outside the auth
  middleware rather than a carve-out inside it.
- **Public reads have their own limits:** 240 reads per
  minute per client IP (`429`, with `Retry-After`), and 4 concurrent public
  requests instance-wide (`503`, with `Retry-After`) so anonymous readers
  cannot crowd the signed-in app out of the database. A download holds its
  concurrency slot while its producer reads and queues file data. The queue
  holds one 64 KiB chunk; memory use does not grow with file size. The producer
  stops after 15 seconds blocked on a full queue, or after 5 minutes total,
  even if the client stops polling. Either timeout releases the slot and ends
  the body with an error, not a successful end-of-file. A small file can finish
  queuing and release its slot before the client consumes the body.
  The client IP is resolved
  the same way login rate-limiting resolves it, which means a forwarding header
  is believed only when the request arrives from a configured
  `server.trusted_proxies` address. Set that if you run behind a reverse
  proxy, or every visitor will share one bucket.

## How the public page renders content

The public view uses its own Markdown renderer, not the one the signed-in app
uses. Three differences matter:

- **Issue identifiers are not auto-linked.** The signed-in app turns `LIF-42`
  into a link and fetches that issue to decorate it with a live status. On a
  public page that would be a request to a protected endpoint, so it does not
  happen at all.
- **Only published attachments are linked.** An `/api/attachments/{id}`
  reference in an issue body is rewritten to the public download URL only when
  that id is in the attachment list the server returned for that issue or
  comment. Any other reference renders as plain text and produces no request.
- **Bodies cannot trigger automatic third-party requests.** Authorized attachment
  images still load from this instance. Markdown bodies may contain raw HTML,
  so the public renderer parses the rendered HTML
  into an inert document and rebuilds it against an allowlist. Tags that can
  fetch or execute (`script`, `style`, `iframe`, `object`, `embed`, `video`,
  `audio`, `source`, `svg`, `link`, forms) are dropped with their
  contents, and **no attribute is ever copied from the input**: every surviving
  element is created fresh and given only attributes the renderer writes. That
  is what rules out `style="background:url(…)"`, `srcset`, `poster`,
  `background` and the rest as a class, rather than one at a time.

The only URLs that survive are ones rebuilt from scratch: a published
attachment, a link to another issue **in the same published project**, or an
ordinary `http(s)`/`mailto:` link, which keeps working but carries
`rel="noopener noreferrer nofollow"` and sends no referrer. A remote image is
replaced by its alt text, so a public issue body cannot be used as a tracking
pixel against its readers.

## Operational notes

- Before exposing the instance publicly, set `auth.required = true` in the server
  config and `web_auto_login = false` in instance settings. Publication does not
  override these settings; either unsafe setting can give visitors admin access
  outside the published project.
- Configure reverse-proxy bandwidth and concurrent-connection limits too. Public
  request limits do not protect against distributed slow readers holding network
  connections after a download producer releases its slot.
- Keep the attachment storage directory and its parents protected from untrusted
  writes. Unix downloads use `openat` anchored to an open directory with
  `O_NOFOLLOW`; Windows checks reparse points, but has not been runtime-tested.
- Publication is one column, `projects.is_public`, added by migration 051 and
  defaulting to `0`. No existing project is published by the upgrade.
- Nothing else in Lific writes that column: not the CLI, not MCP, not the
  importer. `PUT /api/projects/{id}` with `{"is_public": true}` from a lead or
  admin is the only path.
- Publishing is unrelated to the instance's `authz_enforced` setting. The gate
  is the Lead gate in both modes, and public reads never consult project roles
  at all.
- A connected tool (a bot) acts with its owner's permissions here as
  everywhere else, so an agent can only publish a project its owner could.
