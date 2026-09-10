# Public projects

A project can be published so that anyone can read its issues and pages
without an account. Publication is per project, off by default, and
reversible.

The published address is derived from the project identifier:

```
https://your-instance.example/public/LIF
```

There is no token in it. This is a genuinely public page, not a secret link:
search engines, scrapers and anyone who is sent the URL reach it the same way.
If you need "readable by a specific group", add those people as project
members instead.

The public page is the same issue list, board, issue view, page tree and page
view a signed-in reader gets, in a shell that holds only that one project. The
filters, grouping, search and sort all work. Nothing can be edited, created,
commented on or deleted, and the controls for those are not offered.

## What becomes public

Publishing `LIF` exposes, to everyone:

- every **current issue** in the project: title, description, status,
  priority, module, labels, dates, and its relations to other issues **in the
  same project**;
- every **current page** in the project: title, content, status, folder,
  labels;
- the project's **modules, labels and folders** (names and descriptions);
- every **current comment** on those issues and pages, with the commenter's
  **display name**;
- every **attachment linked to** one of those issues, pages or comments,
  downloadable by anyone who can reach the instance.

"Current" means not in the trash. A deleted issue, page or comment, and any
attachment whose only link was to one of them, drop out of the public view
immediately, without waiting for the retention sweep.

## What stays private

The public read path excludes the following. This does not secure other routes
if instance authentication is disabled:

- plans and plan steps;
- activity, audit history and status transitions;
- the trash;
- the member roster, the project lead, and account metadata: comments carry
  a display name and nothing else (no username, no user id, no email);
  attachments carry no uploader;
- an issue's import provenance (`source`), and any relation whose other end
  is in a different project, published or not;
- workspace pages (pages that belong to no project);
- other projects, published or not;
- global search, exports, the realtime socket, the REST API and MCP, all of
  which keep the authentication they had.

## Publishing a project

Project overview → **Public view**. You need to be the project **lead**
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

Every route is under `/public/api/projects/{PROJ}` and is `GET`; any other
method is `405`, any other path under `/public/api` is `404`. None of them
accepts or reads a credential of any kind. Each one is a private route with
`/api` replaced by that prefix, answering with the same JSON shape (minus the
fields listed above) and, for comments, the same paging headers, which is what
lets the web app run its ordinary components against it.

| Public | Mirrors | Answers |
| --- | --- | --- |
| `/public/api/projects/{PROJ}` | `GET /api/projects/{id}` | the project (`lead_user_id` is `null`) |
| `…/index` | `GET /api/projects/{id}/index` | every live issue and page, skinny rows, plus a resume cursor |
| `…/changes?since=&limit=` | `GET /api/projects/{id}/changes` | rows above the cursor; comment rows are omitted |
| `…/modules`, `…/labels`, `…/folders` | `GET /api/modules?project_id=` etc. | the project's structure |
| `…/issues/resolve/{PROJ-42}` | `GET /api/issues/resolve/{ident}` | one issue with its body and in-project relations |
| `…/issues/{id}` | `GET /api/issues/{id}` | the same, by row id |
| `…/issues/{id}/comments` | `GET /api/issues/{id}/comments` | comments, with the `x-comment-*` paging headers |
| `…/pages/{id}` | `GET /api/pages/{id}` | one page with its content |
| `…/pages/{id}/comments` | `GET /api/pages/{id}/comments` | comments on it |
| `…/attachments?entity_type=&entity_id=` | `GET /api/attachments?…` | attachment metadata for a live issue, page or comment |
| `…/attachments/{id}` | `GET /api/attachments/{id}` | the file's bytes, streamed |
| `…/attachments/{id}/thumbnail`, `…/preview` | the same routes under `/api` | a webp preview; a structured archive/database preview |

The sync cursor in `/index` is the highest sequence number among the project's
own rows, not the instance-wide counter the private route returns, so a
stranger polling it learns when *this* project changed and nothing about the
others. Row sequence numbers are the global ones; gaps between them are
visible. `/changes` carries tombstones for deleted issues and pages of the
project (a numeric id and a sequence number, never a title or body), because
a replica needs them to drop rows; treat "issue #N once existed here" as
public once the project is.

Thumbnails and structured previews are only derived for attachments up to
32 MiB; a larger file still downloads, but its `/thumbnail` and `/preview`
answer `404` unless a thumbnail was already generated by a signed-in reader.

Five properties worth knowing if you are building against these:

- **A private or nonexistent project answers identically.** Both are a `404`
  with the same body. So does an issue, page, comment or attachment that
  exists but belongs to another project, is in the trash, or is not linked
  from anything live in this one. You cannot use this surface to find out
  what an instance holds.
- **The project is part of every path, including the attachment one.** An
  issue identifier that names a different project (`/public/api/projects/DEMO/issues/resolve/PRIV-1`)
  is rejected before anything is read, and an attachment id that is not
  reachable from a live issue, page or comment in *this* published project is a
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

`/index` is not size-bounded beyond the project itself, exactly like its
private twin: a project with many thousands of issues serves them all in one
response. The concurrency permit and the rate limit are what bound anonymous
load.

## How the public page renders content

The public page is the signed-in app in a *public scope*: every request the
ordinary components make is rewritten onto the mirror above and sent with no
credential (no bearer token, no cookie), so a signed-in maintainer who opens a
public link sees exactly what a stranger sees. A request with no public mirror
is refused in the browser rather than sent to `/api`. Identity, role, history
and mention lookups are answered locally as "nobody, read-only, nothing".

Markdown renders through the same renderer and sanitizer as the signed-in
app (DOMPurify), with a stricter configuration in public scope. Issue
identifiers auto-link and show their live status, resolved through the public
mirror, and every generated link stays under `/public/…`. Attachment
references in bodies load through the public download route, so one that is
not published simply fails to load.

**A public body cannot make a reader's browser fetch from a third party.**
Elements that fetch on their own (`video`, `audio`, `source`, `picture`,
`iframe`, `object`, `embed`, `style`, `link`, `svg`, forms) are dropped, as
are `style`, `srcset`, `poster`, `background` and `ping` attributes, and an
`<img>` keeps its `src` only when it points at one of this instance's
attachments (`/api/attachments/{id}`); any other image renders as its alt
text. Ordinary links survive: a destination the reader chooses to click is
theirs to choose.

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
