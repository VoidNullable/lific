# Changelog

## Unreleased

### Only `lific init` creates a database

Lific used to create a database out of thin air whenever it could not find one, which sounds convenient and was in practice a way to quietly hand people the wrong tracker.

- **A first run no longer leaves a stray database in your repository.** The MCP registry launches Lific as a bare `lific mcp` with no database argument, using whatever directory your AI client is working in. With no config file anywhere, that resolved to a relative `lific.db` in the current directory, so the server created one there, migrated it, and served an empty tracker. A new user got no projects, three untracked files next to their source code, and no explanation. Local commands did the same thing: `lific project list` or `lific user list` in an unconfigured directory answered `[]` and left a database behind. Every command that reads a local database now refuses to run against one that does not exist, and the error names the exact file and points at `lific init`.
- **`init --db` writes the database it actually used.** Passing `--db` seeded that database with your admin account and then wrote a config file naming a different, default path. The next command that read only the config created that second, empty database and reported no users, with both files on disk and nothing saying they had diverged. The config now records the path `init` really used, absolute, so the database it sets up and the database everything else reads are the same file. Running `init` against a config that already names a different database says so instead of silently seeding one and reading the other.

### Agent guidance

- **`lific agents-md` stops calling the CLI a fallback.** The block it writes into a repository used to rank the two surfaces, "Preferred access — MCP" against "CLI fallback (works without MCP)", which is misleading in the common case where the MCP server runs on another host. It cannot see your filesystem, working directory, or git history, so local work is not something the CLI does worse, it is something only the CLI can do. The block now describes the two as roles: MCP for tracker data, the CLI for anything involving the machine you are on.

### Development

- **The MCP tool surface is checked against itself.** Three tests now read the live tool router the way a client does: every advertised parameter must be snake_case (so the published schema can never disagree with what the server actually deserializes), every required parameter must have a definition, and every registered tool must be named in the README. The tool count had drifted to three different numbers in three places before anything checked.

### Upgrading

- **Lific will no longer create a database implicitly.** `lific init` is the only command that creates one. Everything that opens a local database now exits with an error when the file does not exist rather than creating it: `mcp`, `start`, `dump`, `import`, `service install`, the data commands (`issue`, `project`, `page`, `comment`, `module`, `label`, `folder`, `search`, `export`), and the administrative ones (`instance`, `key`, `user`, `member`). This applies to an explicit `--db` or `--config` too, matching what `lific connect` has always done. Unaffected, because they must keep working without a database: `init`, `restore`, `doctor` (which reports the missing database instead of dying on it), `login`, `logout`, `connect`, `agents-md`, `completion`, and `service uninstall`/`stop`/`status`/`restart`. Commands run with `--backend http` never needed a local database and are untouched. If you have automation that relied on a bare command conjuring a database, run `lific init --db <path>` first.

## v2.8.0 (2026-08-30)

The web UI stops treating the server as something to ask again and again and starts treating it as something to stay in sync with: every change carries a sequence number, tabs replay what they missed, navigation renders from a live read model instead of refetching, and deletes became reversible on the server. Around that, the OAuth consent screen finally says who is asking for what, and [@mjc](https://github.com/mjc) contributed another hardening pass across imports, rendered content, restores, filesystem writes, and the toolchain itself.

### Live sync

- **Changes propagate live and survive disconnects.** Every issue, page, and comment change is stamped with a per-project sequence number. A tab that loses its connection resumes from its cursor and replays what it missed; if it was gone longer than the replay window holds, it backfills just the missing deltas through the new `/changes` endpoint instead of re-downloading the project. A fresh client bootstraps once through `/index` and syncs by cursor from then on.
- **Navigating a warm project stops refetching it.** The issue list, board, modules, and pages render from one in-memory read model kept current by the socket, so moving between views is instant instead of a skeleton flash and a round trip.
- **All your tabs share one WebSocket.** One tab wins leader election through Web Locks and fans events out to the rest, so ten open tabs cost the server one connection instead of ten.
- **Deletes are reversible on the server.** Deleting an issue, page, or comment now leaves a tombstone instead of removing the row. Maintainers can restore issues and pages through the REST API, and expired tombstones are purged after `retention.trash_days` (default 30 days). There is no trash UI yet; this release builds the shelf.
- **Updates can refuse to clobber.** An update may carry `expected_seq`, and if someone else got there first the server answers 409 with the current state instead of silently overwriting. Clients that do not send it keep the old last-writer-wins behavior.

### OAuth consent

Approving an OAuth client used to happen on a screen that could not tell you what you were approving. It said "An application wants to access your Lific issue tracker" no matter who was asking, and the device flow showed whatever name the requesting program had typed into its request. Both paths now resolve a registered client first and say who it is, and neither grants anything without an explicit yes. Most of this was contributed by [@mjc](https://github.com/mjc) in PR #41.

- **The authorization screen names what it is about.** It shows the registered client's name, the exact address you will be redirected to, the capability being granted, how long the resulting token lives, and which account is about to grant it. A request whose client or redirect URI is not registered is refused before the page renders, so a crafted link cannot produce a plausible screen for a client that does not exist.
- **Approve and Deny are both explicit.** A submission carrying neither is rejected instead of being read as consent. Denial redirects with `access_denied` and never creates an authorization code.
- **The consent form is bound to the request it displayed.** Its CSRF token now covers every field on the page rather than just the browser session, so altering the client, redirect, scope, or PKCE challenge between the screen you read and the form you submit invalidates the submission.
- **Connecting a device takes two steps.** Entering a code no longer approves anything: it looks the code up and shows a second page naming the registered client that is asking, the capability, the token lifetime, and the approving account. Only that second submission approves, and it requires a token bound to both the browser session and the device code. Denying still takes one step, because refusing access should never be the slower path.
- **Device grants record the client that asked for them.** A device login used to attribute its token to one shared internal client. The grant now stores the registered client and validated capability at authorization time, and the issued token carries that identity, so a device credential is attributable like any other.
- **PKCE is checked against the full RFC 7636 syntax.** Verifiers shorter than 43 characters, longer than 128, or containing anything outside the unreserved set are rejected, where only empty ones were before. Authorization requests must carry a well-formed S256 challenge before consent renders.
- **Redirect URIs containing a fragment are refused at registration.** OAuth forbids them, and appending parameters to one would have placed the code or error after the fragment, where the client never sees it.
- **A signed-out visitor gets a way in.** The authorization screen requires a session now, since it names the approving account, and an unauthenticated visit links to sign-in instead of dead-ending on a bare error.
- **Disconnecting a tool returns its client registration.** Dynamic registrations are capped per instance, and a client could not be reclaimed while any grant still referenced it, including revoked ones. Revoked grants are now cleared, so a revoked token releases its slot instead of holding it forever. Tokens that are only expired are left alone, matching how Connected Tools reports them.

### Security and robustness

Five more pull requests by [@mjc](https://github.com/mjc), each closing off a way the server could be made to do unbounded or unsafe work.

- **GitHub imports are bounded** (PR #40). Response bodies, issue and comment counts, retained memory, and concurrent import admission all have ceilings now, and hitting a deliberate ceiling returns 413 naming the limit instead of an opaque 500.
- **Anonymous OAuth state cannot grow without bound** (PR #39). Client registrations and device codes have hard caps, size limits on their metadata, and expiry cleanup, so an unauthenticated visitor cannot fill the database.
- **Rendered content is treated as hostile** (PR #38). Mermaid diagrams are size- and depth-bounded before rendering, label colors are sanitized before reaching CSS, and SVG attachments are served as downloads instead of rendering inline, since an inline SVG executes scripts.
- **Restore refuses tampered archives** (PR #42). A restore now validates archive metadata, database schema integrity, and every attachment blob's hash, MIME type, and content before transactionally swapping it in. An archive that fails any check is refused rather than installed.
- **Local filesystem writes are hardened** (PR #43). Config, service, database, and attachment writes reject symlink and path-traversal hazards, create files atomically with private permissions, and serialize storage operations across processes.
- **Database failures stop masquerading as defaults** (PR #46). Reading settings, deriving a username, or checking a bot connection now propagates a database error instead of silently reporting "not found" or substituting a default, so a failing disk looks like a failing disk.

### Web UI

- **Back from a deep link goes up, not out.** Opening an issue or page directly by URL now synthesizes its parent list in history, so the system Back button returns to the list instead of leaving the app.
- **The description editor matches the comment composer.** Same border, focus ring, attached toolbar, and proportions, so the two markdown inputs read as one control.

### Fixes

- **The server no longer slowly eats memory on long uptimes.** The backup task read the database through pooled connections whose 64 MB memory-map windows never close, pinning hundreds of megabytes of mappings forever; production sat at 944 MB resident against a 98 MB database after six days. Snapshots now run on a dedicated short-lived connection and retained heap is trimmed on the backup heartbeat. In an 8-cycle loop against the production snapshot, plateau RSS fell from 294 MB to 23 MB.
- **Attachment operations stopped racing the backup.** Uploads, deletes, dumps, restores, and cleanup now share one cross-process lock, and an operation that would have to wait returns 503 with `Retry-After: 2` instead of blocking or corrupting. Windows classifies its lock violations the same way.
- **Module statuses are validated.** REST, MCP, and CLI all refuse a module status outside the documented six (`backlog`, `planned`, `active`, `paused`, `done`, `cancelled`) instead of storing whatever arrived.

### Development

- **The toolchain now enforces what reviews used to catch.** CI and a pre-commit hook run `cargo fmt --check` and a repo-wide Clippy deny-list of selected pedantic and nursery lints (PRs #44 and #46 by [@mjc](https://github.com/mjc)). The whole-tree reformat is recorded in `.git-blame-ignore-revs`, so `git blame` still points at the commits that meant something.

### Upgrading

- **SVG attachments no longer render inline.** They are served with a download disposition. If you relied on hotlinking an attached SVG into rendered markdown, it now downloads instead.
- **A module status outside the documented six is refused.** Anything automated that wrote free-text module statuses will start getting validation errors.
- **Attachment routes can answer 503.** During a backup, dump, or restore, attachment uploads and deletes may return 503 with `Retry-After: 2`. Retry after the delay instead of treating it as fatal.
- **The WebSocket protocol grew, compatibly.** Events may carry `seq`, clients may resume with a cursor, and a `sync_required` message asks the client to backfill through `/changes`. Old clients can ignore all of it; deletes now being soft means a restore emits ordinary create and update events.
- **`POST /oauth/device_authorization` now requires a registered `client_id`.** RFC 8628 has always required one for public clients; Lific used to accept free-text `client_name` instead and display it on the approval screen. Register at `/oauth/register` and send the returned `client_id`. `scope` remains optional and defaults to `mcp`, but an explicitly different scope is refused rather than quietly downgraded.
- **`lific login` from a CLI older than this release will not work against a server running it.** Older CLIs send only `client_name` and get `invalid_request` back. Upgrade the CLI. The reverse direction still works: a current CLI registers a client when the server supports it and falls back to the old request shape when it does not, so upgrading the CLI first is safe.
- **`lific login` remembers the client it registered.** It is stored in `~/.config/lific/clients.json`, keyed by server URL, and reused on later logins so each login does not consume one of the instance's registration slots. The file holds no secrets and deleting it is harmless; the next login simply registers again.
- **Device logins left half-finished across the upgrade must be restarted.** A device code issued before the upgrade carries no registered client, so exchanging it afterwards returns `invalid_grant`. Device codes expire after 15 minutes, so this resolves itself.

## v2.7.1 (2026-08-23)

A hotfix for a crash that made v2.7.0 unusable. Upgrade immediately if you are on 2.7.0.

### Fixes

- **Opening any issue in the web UI crashed the page.** The issue view showed "Something went wrong" instead of the issue, on every issue, on every instance. Loading an issue clears the comments on screen and then reads how many are loaded, to decide whether a later background refresh should reconcile the whole thread or just fetch the newest page. Those two steps landed in the same reactive effect during the 2.7.0 comment-windowing work, so the effect depended on state it had just written, re-ran itself, and hit Svelte's update-depth limit before anything rendered. The count is now sampled without subscribing to it. Page detail was never affected, and no data was involved: nothing was written, lost, or corrupted.
- **A failed route no longer disappears without a trace.** The error boundary shows the reader a deliberately generic message and, until now, discarded the underlying exception entirely, so a route that failed on every load left no record in the console or anywhere else. It is still never shown in the UI; it is now logged for whoever is diagnosing it.

## v2.7.0 (2026-08-22)

Two things Lific only sketched are finished in this release. Attachments stop being download chips: you can annotate a screenshot before it uploads, read a text file or a diff or a zip or a database without leaving the issue, and see every file a project holds in one place. And a project's blocking structure becomes something you can look at and edit, on a graph canvas where dragging one issue onto another links them. Around those: instance admins can manage members without shell access, the activity feed shows real diffs, the issue list points at what moved while you were away, and the CLI's remote backend reaches parity with the local one.

The other half of this release is a security and robustness pass, most of it contributed by [@mjc](https://github.com/mjc) across eight pull requests. Account recovery now revokes everything an account can act through rather than just its browser sessions. Every surface that could return an unbounded result, comment threads and search and exports and websocket traffic, is now bounded. And a long list of authorization checks that were being made against the wrong thing are now made against the right one. Several of these change a default or refuse a request that used to succeed, so read the Upgrading section before deploying this one.

### Attachments

- **One attach composer, everywhere a markdown body is edited.** Issue descriptions, page content, comments, and now the new-issue form all share the same control: a file picker, drag and drop, paste, and an Attach button in the toolbar. Attaching on the new-issue form previously was not possible at all, so a screenshot had to wait until after the issue existed; uploads now start immediately, insert into the description as you go, and are linked to the issue when you create it. An upload that never gets referenced is swept after 24 hours.
- **Uploads run in parallel with real progress.** Up to three transfers at once, each chip showing byte-level progress with cancel and retry, and a failed upload stays on screen as a failure rather than disappearing. An image large enough to be near the instance's cap pauses on its chip before sending and offers to downscale to 2560px first, and remembers your answer for the rest of the session.
- **A screenshot can be annotated before it leaves the browser.** Paste, drop, or capture an image and a four-second prompt offers the editor: crop, pen, arrows, rectangles, undo, and a pixelation tool that destroys the underlying pixels rather than covering them, so a redaction survives being screenshotted back out. Annotation happens at full resolution on the original file.
- **Alt text, from the chip, after the upload settles.** Optional, on any image, without opening a dialog or hand-writing markdown.
- **On a phone, Attach offers Files, Camera, and Record voice.** Camera asks for the rear lens. Voice notes record through `MediaRecorder` with a timer, a level meter, and a preview before you commit, and the affordance is simply absent on a browser that cannot record rather than failing when tapped.
- **Files are readable in place instead of downloadable.** Text renders with ANSI colour, in-file search, line selection, and linkable line anchors; unified diffs render as diffs; CSV and TSV render as tables; JSON renders as a tree; a zip lists its entries; a SQLite database lists its tables and row counts; audio and video play inline with seeking, backed by real byte-range responses. Anything without a viewer still gets the download chip it always had.
- **Every project has a Files view** at `/{PROJECT}/files`, in both the desktop and mobile navigation. It lists every attachment the project holds with filters by type and uploader, sorting, a running total of bytes, and an expandable where-used list for each file. Files can be deleted from there or from an issue's attachment section, and a collapsed section at the bottom collects orphans, the uploads nothing references any more.
- **Attachments are searchable.** By filename everywhere, and by extracted contents for text files up to 512 KiB, through the same search as everything else.
- **Uploads are validated by magic bytes, not by what the client claims.** The allowlist is PNG, JPEG, GIF, WebP, SVG, PDF, plain text, ZIP, MP4, WebM video and audio, Ogg audio, MPEG audio, and SQLite. The default ceiling is 10 MiB per file, uploads are rate-limited to 30 per user per 10 minutes, and the route itself refuses a body over 64 MiB before reading it.
- **Images carry their dimensions and a thumbnail.** Rasters are decoded on upload for width and height, and a 480px long-edge WebP thumbnail is stored alongside, so a list of forty files does not mean downloading forty full-size images. Blobs stay content-addressed by SHA-256 and deduplicated, so the same screenshot attached to five issues is stored once.

### Dependency graph

- **Every project has a Graph view.** Issues render as clickable cards, blocking relations as edges, laid out left to right so blockers sit left of the work they hold up. The canvas pans and zooms (drag, pinch, ctrl+wheel, arrow keys), defaults to open issues with a toggle for closed ones, and follows dark and light mode.
- **The graph is also where you edit relations.** Drag from one card onto another and a menu asks which relation to create (blocks, blocked by, relates to, duplicate); click an edge to reverse or remove it. An Unlinked canvas collects the issues with no relations yet, ready to be wired in. Editing follows project roles, so viewers get a read-only canvas.
- **Hovering a node previews the issue**, a card with its description clamped to a few lines, so tracing a chain does not mean opening every issue along it. On touch, press and hold does the same for graph nodes, issue rows, and pages, which previously had no path to their hover previews at all.

### Instance administration

- **Manage members from the Instance settings roster.** Instance admins can now create users, promote and demote admins, and deactivate or restore accounts from the web UI, with server-side guard rails: the last remaining admin cannot be demoted or deactivated, and bot identities are not valid targets. Deactivation ends access immediately and atomically, tearing down the account's sessions, API keys, and OAuth tokens in one write, and any bots the account owns stop authenticating until their owner is restored. Five new admin-gated REST endpoints back the UI.
- **An instance set up entirely from the browser now has an admin.** Fresh installs enforce authorization, but web signup never granted the admin role; the only grant path was `lific user promote` on the server's shell, which nothing in the UI mentions. On an instance with zero users, the first signup now becomes the instance admin, the standard self-hosted bootstrap. Any pre-existing account, CLI-created included, disables the grant.
- **The audit log can be pruned.** A new `audit_retention_days` key in the `[backup]` section deletes audit entries older than the window during the existing backup cycle. Unset or 0 keeps everything forever, which remains the default.

### Web UI

- **The activity feed shows real diffs.** Description and content changes render as a per-line diff, added and removed lines tinted, long unchanged stretches folded behind a divider, instead of the previous two full-value blocks.
- **The issue list shows what moved while you were away.** Rows changed since your last visit get a small accent dot, and a toolbar chip counts them and cycles focus through them; viewing a row clears its dot. An agent closes three issues while you are at lunch, and the list points at exactly those three.
- **The issue-list toolbar fits on phones.** Below the small breakpoint it now keeps to a single row instead of wrapping into a fourth band of chrome, with saved views, sort, and display folded into an accessible overflow panel. Chrome above the first issue drops from 173px to under 90px on a 360px screen.
- **Bottom sheets dismiss by swiping down on their header.** The drag-handle pill on mobile sheets was decorative; it now tracks the finger, commits the dismiss past a threshold or on a downward flick, and springs back otherwise.

### CLI

- **`--backend http` now renders exactly like the local backend.** Human and JSON output are shared between the two paths for every data command, and remote exports unpack into the same on-disk markdown tree local exports write instead of leaving a ZIP behind. The client validates every server-supplied path before writing, refuses archive entries that try to escape the output directory, and caps archive entry counts and expanded size.

### MCP tools

- **Agents can work with attachments.** Three new tools, bringing the surface to 30:
  - `upload_attachment` takes a filename and base64 content, optionally links the result to an issue, page, or comment, and returns the attachment id, its metadata, and a markdown snippet ready to embed. Same 10 MiB ceiling and same magic-byte allowlist as the web UI. An upload left unlinked says so, and expires in 24 hours.
  - `get_attachment` reads one back. Text comes by line with `offset` and `limit`, defaulting to 200 lines and capped at 500, so a large log is paged rather than swallowed. A raster image (PNG, JPEG, GIF, WebP) comes back as viewable image content the model can actually look at. Everything else, SVG and PDF and audio and video included, returns its metadata and a download path rather than megabytes of base64.
  - `list_attachments` lists what one issue or page holds, or everything in a project, subject to the same project visibility as every other read.
- **`get_plan` returns full step descriptions.** They were truncated at 100 characters, which made a plan step with real notes in it unreadable through the tool that exists to rehydrate a plan. Multi-line descriptions now come back whole, indented under their step. The compact echo that mutations return is unchanged.
- **MCP errors no longer describe the database.** A failing tool call used to surface the raw SQLite error, which could carry table and column names, constraint text, and the database's path on disk. Tool errors are now generic, and the detail stays in the server's logs.
- **`bulk_update` is one transaction.** It used to commit each issue as it went, so a failure part-way through left some issues updated and some not, with no indication of where it stopped. The whole batch now lands or none of it does.

### Identifiers

- **Project identifiers resolve case-insensitively.** `lific issue list --project lif`, `get_issue("lif-42")`, and every other project, issue, or page lookup across the CLI, REST, and MCP now accept any casing, matching how modules, folders, and usernames have always behaved. As a consequence, creating a project `abc` when `ABC` exists is now rejected. Existing databases with case-colliding identifiers (only possible via raw SQL) are renamed deterministically on upgrade.

### Security: credential transport and imports (PR #27 by [@mjc](https://github.com/mjc))

- **The CLI refuses to send an API key over plaintext HTTP to a remote host.** What used to be a warning is now an error, and the error message never echoes the key. Loopback targets still work over plain HTTP, and unauthenticated plain-HTTP connections still warn rather than fail.
- **Jira import validates the site name before sending credentials anywhere.** The site slug becomes the hostname of every request, so a hostile value could have steered your Atlassian token to a host an attacker controls. It is now constrained to a single DNS-label-shaped slug, and the canonical slug is used consistently for both requests and imported identities.

### Security: account recovery (PR #32 by [@mjc](https://github.com/mjc))

Changing your password used to end your other browser sessions and nothing else. Every other way into the account survived it: the API keys, the OAuth sessions, the connected AI tools, and any long-running agent process already holding a token. Recovery now means what people assume it means.

- **Changing your password, or signing out everywhere, revokes everything the account can act through.** Sessions, API keys, OAuth access tokens, authorization codes that were issued but not yet exchanged, and device approvals that were granted but not yet collected, for you and for every tool you have connected. It commits as one write, so there is no moment where half of it has happened. A password change still leaves you signed in on the device you changed it from, with a fresh session. Unbound operator keys, which belong to whoever runs the server rather than to any account, are untouched.
- **`lific user set-password` carries the same weight.** An operator resetting a password for someone who has lost access now performs the same revocation, in one transaction with the password write, and says so in both its human and JSON output.
- **A long-running MCP agent stops at its next tool call, not at its next restart.** A stdio agent authenticates once with `LIFIC_TOKEN` and can then run for days. That token is now revalidated before every tool call, so revoking the key, resetting the owner's password, or deactivating the account takes effect immediately. The tool does not run, and nothing is written.
- **An API key can no longer mint another API key or connect a tool.** Creating a durable credential now requires a browser session that authenticated in the last 15 minutes. Previously a leaked key was a key factory: an attacker could mint a spare that outlived the revocation of the key they came in on. OAuth tokens are refused on those endpoints too, as of this release (see authorization boundaries below). Web UI behavior is unchanged, and `lific connect` is unaffected.
- **Only a recently signed-in browser session can approve an OAuth connection.** An OAuth access token used to be able to approve an authorization request or a device code, which let one connected tool authorize another and re-mint its way past a revocation. Approval now needs a browser session, and one that signed in within the last 15 minutes, since what it hands out is a 30-day credential.
- **Every action that hands out lasting access now needs the same recent sign-in.** Creating an account, granting instance admin, restoring a deactivated account, changing instance settings, adding someone to a project, raising an existing member's project role, naming a project lead (which grants that person a lead membership), and creating a project led by somebody other than yourself. Creating a project with no lead, or leading it yourself, is unchanged. Each of those leaves access behind that a lockdown on the credential which made it cannot reach, which made them the quiet way to keep a foothold after being locked out.
- **Taking access away never asks.** Demoting an admin, deactivating an account, lowering a project role, and removing a project member all still work from an API key with no recent sign-in, because those are what you reach for while an incident is in progress and they cannot be used to persist anything.
- **Refusing a device connection request never asks for anything.** Approving one still needs a recent sign-in, but denying is how you turn away a code you do not recognise, so it works from whatever session you have open and creates no connected-tool identity as a side effect.
- **A new endpoint refreshes your own session and only your own.** `POST /api/auth/me/refresh` takes the session you are holding and gives back a newer one for the same account, optionally after confirming your password. The web UI uses it to satisfy the recent sign-in requirement without sending you to the login screen. It replaces the previous approach of reusing `/auth/auto-login`, which mints a session for the *instance's first admin* and had already set a cookie by the time a multi-admin instance could notice the swap.
- **Changing your password is rate-limited too**, on its own budget separate from signing in. It is the most expensive request in the API (two password hashes), so attempts reserve their slot before that work starts; a change that lands refunds it, and a wrong current password does not.
- **Failed sign-ins are counted before the expensive part, not after.** The limiter used to check whether an attempt was under the limit and record the failure afterwards, which bounds nothing when requests arrive together: any number could pass the check in the gap before any of them recorded, and all of them would then run the deliberately slow password hash. Attempts now reserve their slot up front, so at most the configured number of verifies can be in flight at once. A successful sign-in gives its slot back, so logging in correctly never eats into the budget that exists to slow down guessing.
- **Changing your password no longer stalls the server while it hashes.** Both hashes (checking the old password, computing the new one) now run with no database lock held, as login and signup also now do (see Fixes). The change itself still commits atomically, and it now refuses if another password change landed while it was working, rather than overwriting it.
- **Signing in is atomic with checking your password.** The Argon2 verify deliberately runs without holding a database lock, and a password change landing during those milliseconds used to still produce a full-length session (30 days by default) for the old password. Login now re-checks, in the transaction that mints the session, that the account is still live and the stored password hash is still the one it just verified. Signup is likewise one transaction, so two people racing to be the first account on an empty instance cannot both become its admin, and a failure part-way leaves no account without a session.
- **The access-granting paths make their authorization decision from state read inside the transaction that acts on it.** An admin demoted, or a project lead removed, while a request was in flight can no longer expand access on the strength of the snapshot taken when the request arrived. The same applies to the destructive side: reaching another account's API key or connected tool, demoting somebody, and deactivating an account all re-check the caller's admin status at the moment they act.
- **Deactivating an account closes its live browser connections at once**, and those of every tool it owns, instead of leaving them to notice at their next periodic check.
- **The web UI asks rather than dead-ending.** When one of these actions is refused for a stale sign-in, the connect dialog, the Members roster and a project's member list each show a single password field, name which action is waiting, resume exactly that one once you confirm, and leave your inputs and your session alone if the password is wrong. Passwordless instances refresh silently, and fall back to the password prompt if that does not work or signs in as a different admin. The retry runs once, never in a loop, and a session belonging to another account is never adopted.
- **Grants cannot outlive the authorization that produced them.** Approving a connection and exchanging the resulting code are each a single transaction, and the exchange re-checks that the identity the grant names may still authenticate. A code or device approval that was invalidated by a recovery returns `invalid_grant` or `access_denied` instead of quietly minting a 30-day token.
- **An OAuth grant that names nobody can no longer be exchanged.** Authorization codes and device approvals predating 2.1 carry no identity, and exchanging one minted an access token that named nobody, which the server then treated as the operator and which no account recovery could revoke, because a recovery works by identity. Both exchanges now refuse an unbound grant outright.
- **Reconnecting a tool after a recovery works again.** API key names are unique across the whole instance, and a revoked key kept its name reserved, so reconnecting the tool whose key had just been revoked failed on a database constraint. A name held only by a revoked key is now released when its **own owner** claims it again. An active key of the same name is still refused, a name another account's revoked key still holds is refused rather than taken, and a connected tool's live credential is never rotated out from under it.
- **The web UI no longer strands itself after a password change.** It now adopts the replacement session the server returns, states the full consequence next to both actions, and asks for confirmation before signing out everywhere. Connected tools are re-read after a password change so they show as disconnected rather than claiming to still be connected.

### Security: authorization boundaries (PR #31 and PR #33 by [@mjc](https://github.com/mjc))

A set of checks that were being made against the wrong thing: the object rather than the project that owns it, the request's opening snapshot rather than the state at the moment of the write, or the bind address rather than where the instance can actually be reached from.

- **Editing or deleting a comment now requires access to the project it lives in**, not just authorship of the comment. Comment mutations checked who wrote it and whether the caller was an admin, and stopped there. Someone removed from a project kept the ability to edit and delete every comment they had left in it, and because comment ids are global, could also confirm a comment's existence in a project they had never been in. Both REST and MCP now resolve the parent project and apply the ordinary read gate before the ownership check.
- **An upload cannot link itself to something you cannot write to.** The entity an upload asked to attach to was taken on trust, so a caller could link a file to any issue or page by id. The target is now authorized before the bytes are stored (maintainer for issues and pages, viewer for comments, instance admin for a page with no project), and a refusal leaves no blob and no row behind. The check happens inside the transaction that writes the link, so it cannot be beaten by having access removed in the gap between the two.
- **Attachment references cannot cross a project boundary.** An issue, page, or comment body could reference an attachment belonging to a project the reader had no access to, and the where-used, thumbnail, and preview responses would then describe that project back to them. References are now confined to the owning project, and every linked-entity list is filtered by the caller's visibility.
- **A plan step cannot reach an issue in a project you cannot see.** Plan steps mirror issues, and the link was accepted for any issue id. Through REST, completing the step would then modify that issue; through MCP, reading the plan disclosed the issue's identifier and status. Linking now requires maintainer access to the linked issue's own project, checked both when the link is made and when the step completes, and a parent step from a different plan is rejected outright.
- **A search result cannot distinguish a project you cannot see from one that does not exist.** Filtering by a project identifier returned a different response depending on whether the project existed, which turns search into a way to enumerate the instance's projects. Both cases now return empty.
- **An OAuth token cannot manage credentials.** OAuth access tokens are what a connected AI tool holds, and they were accepted on the routes that create and revoke API keys, manage connected tools, change a password, revoke sessions, edit a profile, administer users, and manage OAuth clients and tokens. A tool granted access to your tracker could therefore mint itself a permanent API key that survived disconnecting it. Those routes now answer 403 to an OAuth token. Ordinary reads and writes through MCP are unaffected.
- **`LIFIC_TOKEN` is bound to an origin.** The token followed whatever URL won CLI resolution, so running a command in a directory whose `lific.toml` pointed at a different server sent your token to that server. It is now sent only when the target's origin matches `LIFIC_URL`; otherwise the CLI uses your stored credentials for that host and says why.
- **A Mermaid diagram cannot inject HTML through its own parse error.** The sanitizer runs on the diagram source, but a parse failure built the error node by interpolating the offending text into `innerHTML` afterwards, which put attacker-controlled markup on the page past the sanitizer. Anyone who could edit a markdown body could use it. The error node is now built with `createElement` and `textContent`.
- **Passwordless mode is guarded on reachability, not on the bind address.** Turning authentication off, or enabling web auto-login, was allowed whenever the bind host looked local. That is the wrong question when something in front of the server publishes it: an instance bound to `127.0.0.1` and served to the internet through Tailscale Funnel or a same-host reverse proxy passed the check while being fully public. The guard now considers `public_url` as well, and enabling auto-login on an instance that declares a public URL is refused with a 400 and not persisted.
- **Authorization is re-read inside the transaction that acts on it.** Both the granting side and the taking-away side: an admin demoted while a request was in flight can no longer complete an access-expanding write on the strength of the snapshot taken when the request arrived, and reaching another account's API key, demoting somebody, or deactivating an account all re-check the caller at the moment they act.

### Comments (PR #34 by [@mjc](https://github.com/mjc))

- **A comment body is capped at 256 KiB.** The limit is checked after Lific normalizes escaped newlines and tabs, so it bounds what actually lands in the database rather than what was typed. A create or edit past it is rejected and leaves the stored comment untouched. Comments already larger than the cap stay readable, and editing one down to a smaller body works.
- **No surface returns an unbounded comment thread any more.** Every comment list is a page of at most 500. REST keeps its documented `order=asc` default and now defaults to `limit=50`; the MCP `list_comments` tool and `lific comment list` default to the 50 *newest*, which is the half of a long thread anyone actually wants. The MCP tool and the CLI print the exact offset for the next page when one exists, and the CLI says so explicitly when a remote lookup could not determine whether more comments follow, rather than letting a full page read as a finished thread.
- **`get_issue` no longer loads a whole thread to print three comments.** Each comment mode now reads only the rows it renders: `none` loads none and reports the count, `recent` loads three, and `include_comments='all'` is capped at the most recent 500 with a header saying so and pointing at `list_comments`. Given that a single comment can be 256 KiB, the tool an agent calls most often was the worst place for an unbounded read.
- **Reading a busy issue in the web UI no longer means loading every comment.** Issue and page detail open on the newest 50, still in reading order, with a "Load older comments" control for the rest. Opening a link to a comment further back walks the pages in automatically, up to 250 comments, and scrolls to it; past that the reader continues by hand, so a link to a deleted comment cannot crawl a whole thread. The preview panel's comment count reads `50+` when it is showing a bounded page rather than passing a page length off as a total.
- **Comment lists accept a keyset cursor.** `before_created_at` plus `before_id` returns the comments strictly older than one you have already seen. Offsets drift under a thread that is being written to: one comment posted above a reader shifts every offset by one, so the next page repeats a comment or skips one. The cursor names a position instead, and the web UI pages with it. The parameters are optional, the response shape is unchanged, and existing `limit`/`offset` clients are untouched.
- **A background refresh reconciles every comment page on screen**, not just the newest one, so a comment somebody else edited or deleted further up a long thread stops being frozen at the moment you first loaded it. The refresh never fetches more than you already had loaded.

### Resource limits

The comment bounds above are one instance of a pattern this release works through everywhere: a request whose cost is set by the caller rather than by the server. Three more surfaces had the same shape.

**Live updates (PR #28 by [@mjc](https://github.com/mjc)).** A single account could open as many websockets as it liked, each one a task, a receiver, and a file descriptor, and per-account accounting would not have bounded the instance anyway. Inbound messages had no size limit, so a large frame was a large allocation in the parser. Worst of all, an outbound `send` had no timeout, so a peer that stopped reading held its socket, its task, and its permit open indefinitely, and even the close frame could hang.

- Sockets are capped at 16 per user and 1,024 for the instance, held as RAII permits and released when the connection actually ends. A refused connection gets a 429.
- A client message is capped at 16 KiB and a single frame at 4 KiB, and a client may send 64 messages per 10 seconds.
- Every outbound send, the close frame included, times out after 5 seconds and drops the connection.
- **Passive clients stop being disconnected.** Liveness used to depend on the application sending its own heartbeat, which nothing documented, so a client that simply listened was dropped after 120 seconds. The server now sends protocol-level pings every 30 seconds, and a conforming client stays connected without doing anything.

**Search (PR #30 by [@mjc](https://github.com/mjc)).** Visibility was applied after ranking and paging rather than before, and combined entity-plus-attachment paging fetched `offset + limit` rows from both indexes and merged them in memory, which grows with the offset the caller asks for.

- **Results that should not have been visible are gone.** Search could return hits from projects outside the caller's visible set, and an attachment linked to both a visible and a hidden project rendered the hidden project's identifier and title. Visibility is now part of the query, applied before ranking, sorting, and pagination.
- Page size defaults to 20 and is capped at 500. Offsets clamp at 100,000. A full-text query is capped at 4 KiB and a literal substring query at 256 bytes, with an explicit 400 rather than a slow failure past that.
- A literal search stops at 10,000 matches instead of scanning to the end of the table.
- Combined paging fetches only the shortfall it actually needs.

**Exports (PR #36 by [@mjc](https://github.com/mjc)).** An export built the whole tree in memory before sending any of it, and an HTTP client that stopped reading held its export slot open, so a couple of stalled downloads could stop anyone else exporting. On the receiving side, extraction trusted the archive's declared entry count and expanded size, which is the classic zip bomb.

- **Everything an export materializes is now counted.** It previously asked for at most 10,000 issues and then loaded every comment on each of them with no limit at all, so the real ceiling was however much memory the machine had. The bounds are now 10,000 files, 1,000 comments per issue, 100,000 comments per project, 50,000 metadata items, 8 MiB per file, and 128 MiB in total, and an export past them fails with an explicit error instead of trying.
- Two exports run at a time. The HTTP download streams in 64 KiB chunks and is dropped after 30 seconds idle or 30 minutes in total, so a stalled client is reaped rather than parked on a slot.
- A refused export answers 429 with an honest `Retry-After: 30` instead of leaving the client to guess.
- Extraction accepts at most 10,000 entries expanding to at most 512 MiB, and every output path is checked for containment and for symlinks before anything is written to it.

### Database and migrations

- **A database written by a newer Lific will not be opened.** Migrations only ever ran forwards, so pointing an older binary at an upgraded database found the version already stamped, applied nothing, and came up serving and writing a schema it was never compiled against. Startup now fails with an error naming both the database's schema version and the highest the binary knows, and telling you to use a binary that supports it or restore a backup from before the upgrade. Running the same binary again is still a no-op, as it always was.
- **Applied migrations are checksummed.** Each migration's SQL is hashed on application (with line endings normalized, so a checkout on Windows does not read as a modification), and a stored hash that no longer matches is a hard startup failure naming the migration and both digests. Editing a migration that has already run somewhere is the failure this catches, and it used to be silent. Existing databases have the column added and their hashes backfilled on first boot, so nothing fails on upgrade.
- **Two processes starting at once cannot both run the same migration.** Migration now takes an immediate transaction, so discovery and application are serialized across processes rather than racing. Foreign key enforcement is preserved across the rebuild migrations that have to disable it.
- **A backup schedule that would have deleted everything is refused.** `[backup] retain = 0` reads as "keep no history" but meant "keep nothing at all", because rotation runs at the end of each cycle and would delete the archive that cycle had just written. `interval_minutes = 0` was worse: it panicked the backup task on the first tick, and because the timer is built inside the spawned task, backups died silently while the server carried on serving. Both now fall back to the default (24 archives, every 60 minutes) with a warning naming the value that was ignored. Turning backups off is still `enabled = false`.

### Rate limiting and proxy trust

- **`server.trusted_proxies` now defaults to trusting nothing.** It shipped in 2.2 defaulting to loopback, which meant an `X-Forwarded-For` header from any process on the same machine was believed. The default is now empty, and a header is honoured only when the immediate TCP peer is in the list, walking the chain from the right. **This needs action from anyone whose proxy connects over loopback**, which includes Tailscale Serve and Funnel and a same-host nginx: list that proxy explicitly, or every client behind it shares one rate-limit bucket.
- **The rate-limit table is now actually bounded.** Its 10,000-identity constant was only a trigger to sweep expired entries, not a ceiling: the sweep ran and the new identity was then inserted regardless, so a stream of distinct attacker-chosen identities grew the table without limit. It is now a real cap. Expired entries are swept at most once per window, live entries are never evicted (so nobody can flush their own failed attempts by generating traffic), and a full table fails closed on the login path: an identity that cannot be admitted gets the ordinary "too many login attempts" response rather than an untracked free pass. Keys longer than 1,024 bytes are refused outright.
- **A rate-limit response with no known retry delay says "try again later"** rather than telling you to try again in 0 seconds.
- Login is limited to 5 attempts per 15 minutes, counted per identity and per source IP.

### API

- **`GET /api/auth/me` now answers 403 `authentication required` when unauthenticated**, like every other endpoint. It was the one endpoint that escaped the v2.6.0 consolidation and still returned a 400.
- **New attachment endpoints**, backing the viewers and the Files view: `GET` and `POST /api/attachments`, `GET`, `PATCH`, and `DELETE /api/attachments/{id}`, `GET /api/attachments/{id}/thumbnail`, `/links`, and `/preview`, and `GET /api/projects/{id}/attachments` and `/attachments/orphans`. Media requests answer byte ranges, so a video seeks instead of downloading whole.
- **Five admin-gated endpoints** back the instance member roster described above.
- **`POST /api/auth/me/refresh`** is new, described under account recovery.

### Fixes

- Duplicate agent identities can no longer be minted by two simultaneous connects: bot uniqueness per (owner, tool) is now enforced by the database, and upgrading merges any existing duplicates into the oldest bot without losing memberships, groups, or saved views.
- Session validation no longer takes the database's single writer lock on every request. Expired sessions are swept at login and logout instead, so authenticated traffic reads concurrently.
- Signing in and signing up no longer stall the server while they hash. Argon2 is deliberately slow, and it was running while the single writer lock was held, so a burst of logins blocked every unrelated write on the instance. The hash now runs on the blocking pool with no database connection held, and only the lookup and the session insert take the lock.
- Reversing a relation is a single server-side operation. The web UI did it by deleting the old edge and creating the reversed one as two separate calls, so a failure on the second left the relation gone entirely with nothing to undo from. It now swaps direction inside one savepoint, authorized as one action, and answers 404 if the edge is not there.
- A folder that fails to delete stays on screen. The page list removed the folder and reparented its pages locally before the server had agreed, so a refused delete left the sidebar showing a structure the server did not have. The tree now changes only after the write lands, and a failure raises a toast.
- The issue-list overflow control is a real disclosure. It announced itself as a menu without behaving like one, and focus never entered the panel it opened. It now carries disclosure semantics, moves focus into the panel, and returns focus to the trigger on close.
- A very large description no longer makes the activity diff expensive. The line diff bounds its comparison table by area rather than by line count, so a single enormous line still diffs while a pathological pair cannot allocate without limit.
- An IPv6 loopback bind (`host = "::1"`) now prints valid URLs like `http://[::1]:7777` instead of `http://::1:7777` in init output, `service status`, the doctor, and the OAuth issuer.
- A swipe-dismissed sheet no longer flashes back into place for a frame before unmounting.
- The single-page-app fallback is no longer served from a stale cache after an upgrade.
- The test suite creates its scratch directories through `tempfile`, so an aborted run cleans up after itself instead of poisoning the next one.

### Upgrading

**Take a backup first.** This release adds a downgrade guard, so once an instance has started on 2.7 an older binary will refuse to open its database. That is the intended behaviour, but it means rolling back is a restore rather than a swap.

- Five migrations run automatically on first start: the bot-identity unique constraint (merging any existing duplicates), case-insensitive project identifiers, the account active flag behind deactivation, attachment dimensions and alt text, and the attachment search index. The `_migrations` table also gains a checksum column and has its existing rows backfilled. No manual steps.
- **Review `server.trusted_proxies` before you start 2.7.** The default changes from trusting loopback peers to trusting nothing. If your proxy reaches Lific over loopback, which covers Tailscale Serve and Funnel and a reverse proxy on the same host, add it to the list explicitly. Left unset, `X-Forwarded-For` is ignored and every client behind that proxy is rate-limited as one identity.
- **An older binary will not open a 2.7 database.** Startup fails with an error naming both schema versions. Recover by going back to the 2.7 binary or restoring the backup you took.
- **A modified migration file is now a startup failure.** If you have ever hand-edited a migration that already ran on this database, 2.7 will refuse to start and name it. Restore the original file; corrections belong in a new migration.
- `[backup] retain = 0` and `[backup] interval_minutes = 0` now log a warning and use the defaults (24 archives, hourly) instead of silently deleting every backup or killing the backup task. Set `enabled = false` if you want backups off.
- If a script drives `--backend http` with an API key against a remote `http://` URL, it now exits with an error instead of proceeding past a warning. Switch the target to `https://` or a loopback address.
- `LIFIC_TOKEN` is only sent to the origin in `LIFIC_URL`. A script that sets the token and then overrides the target with `--url`, or runs inside a directory holding a `lific.toml` for a different server, now falls back to stored credentials for that host instead of forwarding the token. Set both variables to the same origin.
- Audit retention stays off unless you set `audit_retention_days`; nothing is deleted by default.
- `lific comment list` is now bounded and newest-first. A script that relied on it printing an entire thread oldest-first needs `--order asc` and its own `--offset` loop.
- A client reading `GET /api/issues/{id}/comments` without a `limit` now gets 50 comments instead of all of them. The ordering is unchanged (`asc`), so the page is the oldest 50; page with `offset`, or pass `order=desc` for the newest.
- `get_issue` with `include_comments='all'` returns at most the 500 most recent comments. Threads longer than that need `list_comments` to page the remainder.
- **`POST /api/auth/keys` and `POST /api/auth/bots` no longer accept an API key.** They require a browser session token created within the last 15 minutes. A script that minted keys by presenting an existing key now gets 403 `recent authentication required`. Mint keys with `lific key create` on the server, or from the web UI. `lific connect` writes to the database directly and is unaffected.
- **A password change or sign-out-everywhere now revokes API keys, OAuth sessions, and connected tools**, for the account and for the tools it owns. Reconnect each tool afterwards (`lific connect`, or the Connected tools section of Settings) and replace any API key a script depends on. The same is true of `lific user set-password`.
- **A running stdio MCP agent now fails its next tool call after its key is revoked**, with a message telling it to run `lific connect` and restart. Previously it kept working until the process was restarted.
- OAuth access tokens can no longer approve an authorization request or a device code, and the approving browser session must have signed in within the last 15 minutes. Approve from a signed-in browser.
- **Access-expanding endpoints now require a browser session created within the last 15 minutes** and answer `403 recent authentication required` to an API key or OAuth token. This covers key and bot creation, account creation/promotion/reactivation, instance settings, project-member additions and role increases, creating a project led by another user, and assigning a project lead later. Scripted provisioning moves to the corresponding local CLI commands. Reductions such as demotion, deactivation, role downgrade and member removal are unchanged.
- **An OAuth approval from a browser session older than 15 minutes is refused.** The page explains that you have to sign **out** and back in (revisiting the login page will not help, because the old session is still valid) and then restart the connection from your MCP client. The approval is not resumed for you.
- `POST /api/auth/me/refresh` is new. Nothing needs to change to adopt it; the web UI uses it automatically.
- An OAuth authorization code or device approval that is not bound to an identity, which only rows stored before 2.1 are, now returns `invalid_grant` at exchange. Codes expire within fifteen minutes, so this affects only a client caught mid-flow across the upgrade: authorize again. Access tokens already issued are unaffected; revoke any unbound one and reconnect the tool.
- **An OAuth access token is refused on the credential-management routes**, which is every route that creates or revokes API keys, manages connected tools, changes a password, revokes sessions, edits a profile, administers users, or manages OAuth clients and tokens. A connected tool that was calling those endpoints now gets a 403 and needs a browser session or a local CLI command instead. Ordinary reads and writes through MCP are unchanged.
- **Editing or deleting a comment now also requires access to its project.** A client acting on comment ids for a project it has since been removed from will start getting refusals. This applies to REST and MCP alike.
- **Search is bounded.** Page size defaults to 20 and caps at 500, offsets clamp at 100,000, a full-text query over 4 KiB or a literal query over 256 bytes returns 400, and a literal search stops at 10,000 matches. A client that paged past the clamp, or leaned on unbounded literal scans, needs narrower queries. Results from projects the caller cannot see are gone, which may legitimately shrink result counts.
- **Exports are bounded and concurrency-limited.** The per-project ceilings are 10,000 files, 1,000 comments per issue, 100,000 comments per project, 50,000 metadata items, 8 MiB per file, and 128 MiB total, all well above the previous 500-file and 500-comment limits. Only two exports run at once and the rest get a 429 carrying `Retry-After: 30`, so an export client needs to honour it. A stalled download is dropped after 30 seconds idle, and no export runs longer than 30 minutes.
- **Websocket clients have limits.** 16 sockets per user and 1,024 per instance, with a 429 past that; a 16 KiB message and 4 KiB frame cap; 64 messages per 10 seconds; and a 5-second send timeout. A custom client sending oversized or high-rate messages needs adjusting. Clients that only listen benefit: server-side pings every 30 seconds mean they are no longer dropped after 120 seconds for not sending an application heartbeat.
- **MCP tool errors are now generic.** A script matching on the text of a database error will no longer find it. The detail is in the server log.
- **Enabling web auto-login is refused on an instance that declares a `public_url`**, with a 400, and the setting is not persisted. Passwordless mode is for instances that are genuinely not reachable from elsewhere; a loopback bind published through a tunnel no longer qualifies.
- Uploads are validated against the file's actual bytes rather than the declared content type, capped at 10 MiB by default, and rate-limited to 30 per user per 10 minutes. A client uploading a type outside the allowlist will now be refused.

## v2.6.0 (2026-08-15)

Who is acting is now resolved one way everywhere: connected AI agents get identities of their own and act as themselves, login-free mode genuinely works, and `lific init` asks how you want to sign in instead of leaving that decision to a hand-edited config file. Alongside the identity work: two security fixes worth upgrading for on their own, comments you can edit and link to, and identifiers you can copy from wherever you see them.

### Sign-in, identity, and agents (PR #23 by [@zorro432](https://github.com/zorro432))

- **Login-free mode now works end to end.** Running with `required = false` used to be half-broken: project reads passed while admin endpoints answered 403, because the "no auth here" signal never reached the handler-level gates. Identity is now resolved in one place for REST, MCP, and CLI alike, every request carries a real user, and a login-free instance can administer itself.
- **`lific init` asks how you want to sign in.** On a fresh install it offers Login-free or Passwords, states plainly what login-free means (anyone who can reach the server can administer it), binds login-free instances to loopback so that promise actually holds, and persists the choice to the config file. `--auth-mode` and `--password` cover scripted setups.
- **Connected AI agents act as themselves.** Approving a tool over OAuth mints a per-tool bot identity, stdio agents connected via `lific connect --stdio` carry their own token, and the audit log attributes their writes to the tool rather than to you. Disconnecting or deleting a bot revokes its OAuth tokens too, and reconnecting remembers which tool a client picked last time.
- **`lific connect` grew a transport menu**, with stdio preselected and remote and OAuth on offer. The flags (`--stdio`, `--oauth`, `--url`) behave exactly as before for scripted runs.
- **MCP enforces the same authorization as REST.** An agent can no longer do more through MCP than the same account could through the web UI.
- **Hardening around the edges.** Failed credentials now fail instead of quietly falling back to the first admin's identity; a server with `web_auto_login` enabled refuses to start on a non-loopback bind; connector configs that embed credentials are written with `0600` permissions; a new API key is bound to its user in the same insert that creates it; OAuth device-code redemption is atomic with the token mint.

### Security

- **An uploaded SVG can no longer run script on your instance.** SVG is on the upload allowlist, and every image type was served with `Content-Disposition: inline`. Because an SVG is an XML document that may contain `<script>`, opening one directly (the "open image in new tab" path, not the rendered `<img>` in a comment) executed its script on Lific's own origin, where it could read the session token the web UI holds and act as whoever opened the file. Any account permitted to upload could use this to escalate against an admin who viewed the attachment. SVG uploads still work and still render in comments and pages; they now download rather than render when opened as a page of their own, and every attachment response carries `Content-Security-Policy: default-src 'none'; sandbox` as a second layer. Reported by [@mjc](https://github.com/mjc).

- **A malformed config file now stops the server instead of silently reverting to defaults.** A config that failed to parse, including one broken by a single typo, produced a warning on stderr and then booted from the built-in defaults. Those defaults bind `0.0.0.0`, allow self-service signup, and treat an empty `cors_origins` as "any origin", so an operator who had set `host = "127.0.0.1"` and `allow_signup = false` could be left running a materially more exposed instance with nothing but a log line to say so. A config file that exists but cannot be read or parsed is now a hard startup failure naming the file and the error. Unknown keys are rejected for the same reason: `allow_signupp = false` used to be ignored silently and now fails loudly. A missing config file is still perfectly fine and still starts on defaults. Reported by [@mjc](https://github.com/mjc).

### API

- **"Authentication required" is now always a 403.** Nineteen places in the REST API each wrote their own version of the "is anyone signed in?" check, and roughly half of them reported the failure as `400 Bad Request` while the rest reported `403 Forbidden`. The endpoints for your profile, password, sessions, API keys, connected tools, and comments were in the 400 group; they now answer 403 like everything else, with the message `authentication required`. If you have a client that treats a 400 from those endpoints as "you are signed out", it needs to look for 403 instead.

### Web UI

- **Comments have their own links, and `#N` becomes one.** A comment can be linked directly and the browser scrolls to it, and a bare `#12` in issue text resolves to that issue. (PR #24 by [@unger1984](https://github.com/unger1984))
- **Edit and delete your own comments.** Editing is inline with the full composer (markdown, mentions, quoting, attachments), deleting asks for confirmation in place, and an edited comment carries an `edited` label with the exact time in its tooltip. (PR #25 by [@unger1984](https://github.com/unger1984))
- **Home shows a live activity rate.** It seeds from the last 24 hours of activity you are allowed to see, then ticks along over the websocket instead of starting from a misleading zero. (PR #26 by [@mjc](https://github.com/mjc))
- **Copy an identifier from wherever you are looking at it**: the detail-page breadcrumbs, a board card, or a list row. (PR #22 by [@lardissone](https://github.com/lardissone)) The copy and peek buttons are reachable by keyboard, not hover alone, and pills name the identifier they copy so a screen reader announces something useful.
- **Copy selected text from the selection toolbar**, instead of reaching for the mouse to select and copy by hand.
- **The desktop sidebar collapses**, giving the board and the issue list the full width.
- **The status chip in the issue topbar is a picker.** Changing status no longer means opening the field below it.

### Fixes

- Starter label presets no longer vanish after you add the first one.
- Editing a label no longer reloads the entire settings page.
- `#N` inside a numeric HTML entity is left alone rather than linkified into nonsense.
- Deeply nested plan steps stay readable instead of squeezing themselves into an ever-narrower column.
- Attachment links stay in sync on every MCP write path, not just some of them.
- A restore whose rollback fails now names the path the old database survives at, instead of leaving you to hunt for it.
- `lific doctor` checks the router production actually serves.
- Test databases are named from a counter rather than the clock, so a fast machine no longer collides two of them in the same millisecond.

### Upgrading

- Two migrations run automatically on first start; they add the per-tool identity plumbing behind connected agents. No manual steps.
- **Check that your `lific.toml` parses before rolling this out**, because a config Lific previously ignored will now stop it from starting. `lific doctor` reports the config as a normal check and keeps running the rest, which makes it the right tool for this. The strictness is the fix, not a side effect: a config that fails to load is exactly the case that used to degrade quietly.
- If `web_auto_login` is enabled and `[server] host` is not loopback, the server now refuses to start rather than exposing a no-password login to the network. Bind `127.0.0.1` or turn auto-login off.
- If your config sets `secure_cookies` under `[auth]`, remove it. It has never been read from the file (it is derived from whether `server.public_url` is `https://`), and unknown keys are now an error rather than being ignored.
- Audit entries written by connected tools are now attributed to the tool's own identity rather than to the operator who connected it.

## v2.5.0 (2026-07-28)

Every identifier Lific prints is now something you can click. Windows gets a prebuilt binary and the same CI treatment Linux has always had. On a phone the web UI navigates like an application instead of a desktop layout squeezed sideways.

### Links to everything (PR #19 by [@mjc](https://github.com/mjc))

Issue ids, projects, pages, modules, plans, comments, search hits, activity entries, and relations render as Markdown links in MCP tool output, CLI output, and REST responses. An agent that reads `[PRO-42](https://tracker.example/PRO/issues/PRO-42)` can hand you something to click instead of an identifier you then have to go find.

- **A comment links to the comment.** The destination carries a `#comment-7` anchor and the browser scrolls to it, rather than dropping you at the top of a long issue.
- **The canonical id wins.** Ask for `pro-042` and the link still points at `PRO-42`.
- **Setting the base.** `server.public_url` in `lific.toml` pins it. Over HTTP, Lific otherwise derives the base from the request's own host, provided that host is allowlisted.
- **Instances behind a path prefix work.** A Lific served from `example.com/tracker/` produces links that resolve.

### Security

- **A hostile `public_url` can no longer inject a second link into every reference Lific generates.** A base path ending in the right punctuation could close the Markdown destination early and append an attacker-controlled `[label](javascript:...)` to the output. Bases carrying credentials, queries, or fragments are now rejected too, along with malformed `Host` headers and hosts outside the allowlist. This is operator-configuration hardening: reaching it requires someone who can already edit your config or terminate your requests.

### Windows

- **There is a Windows binary.** `lific-windows-x86_64.exe` ships on the releases page, checksummed in `sha256sums.txt` alongside the Linux and macOS builds.
- **CI runs the full suite on Windows**, the same clippy-with-warnings-as-errors and `cargo test --all-targets` that Linux gets. Nothing in CI or the release matrix had ever touched the platform, which is how v2.4.0 shipped unable to compile there at all: `service.rs` called the unix-only `libc::getuid()` with no platform guard. Reported in [#20](https://github.com/VoidNullable/lific/issues/20) by [@cuqz](https://github.com/cuqz).
- **`lific service` no longer fails with "HOME is not set"** in environments that do not export `HOME`, which includes some cron and service contexts on Linux. It resolves your home directory the way the rest of the CLI already did.

### Web UI

- **Navigation on a phone is a full-screen drilldown**, with swipe, back-button and Escape dismissal, safe-area insets, and 44px touch targets.
- **Search and the issue picker go full-screen**, so the on-screen keyboard stops covering the thing you are typing into. Filters and the shortcut help open as bottom sheets.
- **Detail and module topbars no longer overflow the screen.** Edit/Preview rendered twice at some widths and now renders once; breadcrumbs and export stay reachable when the bar compacts.

### Upgrading

- No migrations. Upgrading from any 2.x needs no manual steps.

## v2.4.0 (2026-07-27)

Projects can be filed into named groups in the sidebar, and the grouping belongs to you rather than to the instance. Alongside it, the last hover-only controls become reachable on touch, and the test suite stops depending on whatever is exported in your shell.

### Project groups in the sidebar (PR #17 by [@lardissone](https://github.com/lardissone))

A long project list had exactly one organizing tool: drag to reorder. Projects can now be collected into named, collapsible groups that render above the ungrouped list.

- **Create, rename, delete, and fill** from the sidebar itself. The Projects header offers New project or New group; right-clicking a project lists the groups it can move into, plus Remove from group when it is already in one; right-clicking a group row offers Rename and Delete. Deleting a group returns its projects to the ungrouped list rather than deleting anything.
- **The group is also a field on the project.** Both the create form and Project Settings carry a group selector, so filing a project does not require a trip to the sidebar. Settings treats it as one more autosaved field and rolls the selection back if the server rejects it.
- **Grouping is per-user, not instance-wide.** Project visibility is already per-user through project membership once authorization is enforced, so a shared group would render empty for anyone without access to the projects inside it. Your grouping is invisible to everyone else and never rearranges their sidebar.
- **A project belongs to at most one group**, and collapse state is remembered per browser, the same class of local preference as the sidebar width.
- **The API is identity-scoped.** `GET`/`POST /api/project-groups`, `PUT /api/project-groups/assign`, and `PATCH`/`DELETE /api/project-groups/{id}` filter every query on the caller. A group belonging to someone else returns 404 rather than 403, so the endpoint never confirms the id exists. Assignment is the one exception: its body names a project, so it takes the standard Viewer gate.
- **Revoked access does not leave debris.** The group listing drops project ids the caller can no longer view, so losing membership on a grouped project removes it from the sidebar instead of leaving an entry that 403s on click.
- **Reordering stays correct.** Projects sort by a single global rank, so a reorder payload derived from the grouped sidebar would have written one user's private arrangement into the order everyone else reads, and colliding ranks made the list fall back to name order. Reorder now sends the canonical order with exactly one project moved.

### Web UI

- **Five hover-only controls are reachable on touch.** The pinned-page unpin control had no alternative path at all on a touch device, making pinned pages permanently pinned; the Project Settings name and description pencils, the copy-identifier icons in Project Settings and the peek panel were cosmetic but equally invisible. A sweep for the same pattern elsewhere now reports none left.

### Contributing

- **`cargo test` no longer fails on a clean checkout** when `LIFIC_API_KEY` or `LIFIC_URL` is exported, which is the normal state for anyone running an instance. The CLI parse tests read those through clap's environment fallback and now isolate themselves. CI has a clean environment, so this only ever bit contributors.
- **Coverage for paths that had none**: invalid-but-well-formed API keys on REST, an issue driven through create/read/update/delete over the router, the MCP project/module/folder resolvers pinned to their real case sensitivity, and the two project-group paths that only existed at the query layer. The suite stands at 1,195 tests.

### Upgrading

- One new migration, applied automatically on first launch. Upgrading from any 2.x needs no manual steps.
- Project groups start empty for every user; the sidebar renders exactly as before until you create one.

## v2.3.0 (2026-07-20)

The CLI learns to talk to a running server over HTTP instead of requiring the database file, OAuth discovery works out of the box on localhost instances, and the documentation got a full contributed overhaul with a CI check that keeps it honest.

### CLI: run data commands against a remote instance (PR #15 by [@mjc](https://github.com/mjc))

Every data command previously opened the SQLite file directly, so the CLI only worked on the machine hosting the instance. A new selectable HTTP backend routes those commands - issues, projects, pages, search, export, comments, modules, labels, and folders - through the REST API of a running server instead:

- **Select it** with `--backend http`. The target URL comes from `--url` or `LIFIC_URL`, falling back to the configured `server.public_url`, then the local bind address.
- **Credentials**: `--api-key` or `LIFIC_API_KEY`, else the credential stored by `lific login` (env token, keyring, or credential file). Mutations are attributed to that credential's identity, exactly like any other API client. `comment add --user` remains SQLite-only - impersonation stays behind shell access to the database.
- **The transport is hardened**: a warning before sending a key over plain HTTP to a non-loopback host, redirects refused, a 30-second timeout, error bodies bounded, control characters sanitized out of server-supplied text, and export downloads written under safe filenames.
- **The default SQLite backend is untouched** - no flag, no behavior change.

Supporting it, a new `GET /api/pages/resolve/{identifier}` endpoint resolves page identifiers (project-scoped `PRO-DOC-3` and workspace `DOC-3` alike), the way `/api/issues/resolve` already did for issues.

### OAuth discovery works without `public_url`

With `server.public_url` unset, OAuth discovery metadata advertised an issuer derived from the bind address - a client connecting via `localhost` or the IPv6 loopback then failed token audience validation, which broke MCP OAuth flows (Claude among them) against local instances. The issuer and every advertised OAuth endpoint now derive from the request's Host header when that host is allowlisted; an explicit `public_url` remains authoritative, and forwarded headers are never trusted for this.

### Documentation, contributed and CI-enforced (PRs #6-#12 by [@mjc](https://github.com/mjc))

- **Contracts are written down**: the REST API surface and the MCP tools' output shapes are now documented, and a sweep corrected reference drift between docs and code.
- **Guides**: the web UI, upgrading, and repository layout each have one; connecting Codex is clarified; and the repo gains `SECURITY.md` and `CONTRIBUTING.md`.
- **CI keeps it honest**: a new docs check fails the build when documented commands, routes, or tool names and counts drift from what the code actually ships.

### Web UI

- **Page identifiers are visible**: the page detail breadcrumb now ends with the identifier in mono (matching how issues render), and every list surface - the folder tree, Recent/Drafts/Archived, pinned cards - shows it too. Previously it only appeared in search results.

### Ecosystem

- MCP directory listings: a `glama.json` manifest and a Dockerfile for directory indexers, plus internal infrastructure hostnames scrubbed from the public tree.

## v2.2.1 (2026-07-15)

The MCP tool surface gets smaller and cheaper: 27 tools (down from 29) at about 5.6k tokens of schema (down from 6.4k), measured with tiktoken o200k_base against `tools/list` output.

### One export tool instead of three

`export_issue`, `export_page`, and `export_project` merged into a single `export` that dispatches on identifier shape, the same way `get_activity` already did: `PRO-42` exports the issue, `PRO-DOC-3` the page, bare `PRO` the whole project. Same Viewer gating and outputs per branch. Clients calling the old tool names must switch to `export`.

### Leaner tool schemas

Tool and parameter descriptions were rewritten to stop repeating what tool outputs already show at call time (paging hints, provenance markers), what sibling parameters already state, and what other tools already define (the edit-by-string contract is now stated once, in `edit_issue`). Internal tracker references leaked into five field descriptions and are gone. Net: 6,436 to 5,641 schema tokens.

## v2.2.0 (2026-07-14)

The web UI goes realtime, MCP tool output slims down to respect agent context budgets, and a security fix stops clients from spoofing their IP to the rate limiter. This is also the first release with external contributions: realtime invalidation arrived as PR #4 ([@mjc](https://github.com/mjc)) and comments pagination as PR #5 ([@Joshuabaker2](https://github.com/Joshuabaker2)).

### Realtime web invalidation

Two browser tabs - or you and your agent - no longer drift apart. Every state-changing write routed through the running HTTP/MCP server pushes an invalidation event over a WebSocket and open views resync live: issues, pages, plans, comments, attachments, saved views, module/folder structure, plans' cross-project issue effects, and the authz toggle. (PR #4 by [@mjc](https://github.com/mjc), hardened and extended in review.)

CLI data commands and stdio MCP access SQLite directly. They do not publish into another running Lific process's in-memory realtime hub, so refresh the browser or wait for its normal revalidation after direct database changes.

- **The socket is a credentialed surface**: sessions are revalidated every 60 seconds (logout or expiry tears the connection down), connections are capped per user, and a no-op write emits no event.
- **Reconnects behave**: views resync after the socket comes back (nothing missed while offline), an expired session breaks the reconnect loop instead of hammering the server, and navigating away tears the socket down cleanly.

### Security: the rate limiter no longer trusts client-supplied X-Forwarded-For (LIF-206)

Per-IP rate-limit keys came from the leftmost `X-Forwarded-For` entry - which the client controls. A direct attacker could rotate XFF per request for a fresh bucket, spoof a victim's IP, or poison the key space with garbage. Now:

- **New `server.trusted_proxies` config** (CIDR list), defaulting to loopback-only - which preserves real-client-IP behavior behind Tailscale Funnel with zero config change. Invalid entries fail startup loudly. Add only proxy ranges you operate.
- **The genuine TCP peer is the key** unless that peer is a trusted proxy. For trusted peers, the full XFF chain is walked right-to-left skipping trusted hops and the first untrusted IP wins; malformed or all-trusted chains fail closed to the peer address. `X-Real-IP` is consulted only when XFF is absent, and header values must parse as strict IPs (with IPv4-mapped-IPv6 normalization).

### MCP tool output respects the context window

Agents pay for every token a tool returns, and the chattiest tools were spending that budget on things the agent didn't ask for. The defaults now return the working set, with explicit opt-ins for the full picture:

- **`get_board` omits done/cancelled issues by default** (LIF-300): status grouping shows closed columns as count-only stubs, priority/module grouping drops them with a trailing count. `include_closed=true` restores the old render; `max_per_column` caps each column with a `… +N more` tail.
- **`get_issue` defaults to the last 3 comments** (LIF-301) with a truncation header; `include_comments='all'` for the whole thread, `'none'` for a stub. `list_comments` gains a `limit`.
- **`list_comments` paginates** (LIF-326, PR #5 by [@Joshuabaker2](https://github.com/Joshuabaker2)): MCP and REST accept `limit`/`offset`. MCP output includes a continuation hint when more comments remain; REST returns the requested comment array without paging metadata. Unqualified calls still return the full thread in ascending order, exactly as before.
- **`update_plan_step` returns a compact receipt** (LIF-302) - side-effect notes plus a one-line progress summary instead of re-rendering the whole tree. `echo_tree=true` restores the old output.
- **`get_issue` relation lines carry the related issue's status** (LIF-303): `Blocked by: LIF-42 (done)` answers the follow-up before it's asked.

### MCP: search, resume flow, and discoverability

- **Literal search mode** (LIF-304): `mode='literal'` does a case-insensitive substring scan over issues, pages, and comments - finding punctuation-heavy needles like `core:sodom`, `[RequiredSpecs]`, or `--trace-plans` that FTS tokenizes away.
- **Resume-flow signals**: `update_issue` reports plan-step cascades (auto-completed/reopened steps) fired by closing a linked issue (LIF-324); `list_resources(type='project')` appends workable count, active plan count, and last-activity age, sorted most-recently-active first (LIF-325); the server instructions tell agents to check for an existing plan before creating a duplicate (LIF-322).
- **`list_issues` can sort by priority** (LIF-323): `order_by=priority` joins the whitelist.
- **`manage_resource` project updates are discoverable** (LIF-327): the schema now spells out that projects are targeted via `project=<IDENT>`, and `current_name` without `project` returns an instructive error instead of a generic one.
- **Regression coverage: tool outputs never HTML-escape stored text** (LIF-299).

### Web UI: sub tabs, sidebar, and touch

- **Sub tabs across every list view** (LIF-305, LIF-308): issues get All/Recent/Open/Closed, pages get Browse/Recent/Drafts/Archived (archived pages finally have a first-class home), plans get Active/Done/Archived/All, modules get Active/Backlog/Archive/All. Counts on every tab, per-project persistence.
- **Sidebar recents** (LIF-307): the five most recent items of the active section, one click away. Archived pages and plans stay out - recents are a jump-back-in affordance.
- **Drag-resizable sidebar width, persisted** (LIF-309) - including the fix for the Tailwind ordering bug where the resize work broke the mobile drawer.
- **Page re-parenting works on touch** (LIF-280): a Move-to-folder picker covers what desktop does by drag.
- **PWA manifest + icons** (LIF-321): add Lific to a phone home screen and it opens like an app.
- **Command palette results** stack title over snippet and render FTS `**match**` highlights as emphasis instead of raw markers (LIF-328).

### Cross-project integrity and pagination correctness

A field-report sweep hardened the seams between projects and the views that page over data:

- **Cross-project references are rejected everywhere they could sneak in**: an issue can't take another project's module (LIF-310), a page can't move into another project's folder (LIF-311), and a folder can't be parented under a folder from another project (LIF-312).
- **Page moves are transactional in the UI**: a failed move rolls back visually (LIF-313), concurrent moves are guarded (LIF-318), and a move no longer triggers stale reloads (LIF-320).
- **Lists page all the way**: plan lists use stable cursor pagination (LIF-316) and load every page (LIF-314); sidebar page recents paginate instead of truncating (LIF-315, LIF-317).

### Auth-optional now reaches the browser (LIF-297)

Fixes the 2.1.0 field report "auth required false isn't working": REST and MCP honored `[auth] required = false`, but the **web UI still showed a login screen** - the SPA decides signed-in state via `/api/auth/me` (400 for the anonymous operator) and only skips the login form when the instance advertises single-user auto-login. `GET /api/instance` now advertises auto-login when auth is off, and `POST /api/auth/auto-login` mints the first-admin session under `required = false` just as it does under `web_auto_login`. The browser goes straight to the dashboard, signed in as the first admin. With zero accounts the signup screen still appears once (there is nobody to sign in as); the two flags share a threat model, and auth-off already refuses to start with a non-localhost `public_url`.

### Everything else

- **Literal `\n`/`\t` in code blocks survive round-trips** (LIF-142): text unescaping now only fires on real control characters, so documentation about escape sequences stops being mangled into actual newlines.
- **Backup staging files can't accumulate** (LIF-329): a dump that fails mid-write now cleans up its partial `.tmp` archive, and the interval backup task sweeps stale staging leftovers from a crashed run - previously invisible to rotation and stranded forever.
- **A failed crates.io publish fails the release run** (LIF-288): the publish step swallowed every error, including the 403 that silently skipped v2.0.0's publish. Only the idempotent "version already uploaded" case is tolerated now, and duplicate detection matches narrowly (LIF-319).

## v2.1.0 (2026-07-06)

A release driven entirely by 2.0 field reports. The authorization default made project access a real concept, but the CLI had no way to manage it and no way to reset a password; `lific init`/`lific service` quietly ignored `--config`; and configuration lived wherever the command happened to run instead of where an OS keeps config. 2.1 closes all of it: project membership and password resets from the CLI, config discovered (and created) in standard OS locations, `--config` honored everywhere, and - for private local instances - the option to turn auth off entirely.

### `lific member` - manage project access from the CLI

With enforcement on (the 2.0 fresh-install default), a newly created user is a member of nothing and sees nothing. That was manageable only through the web UI's members page; now the CLI can do it:

- `lific member list --project <IDENT>` - members and their roles.
- `lific member add --project <IDENT> --user <name> [--role viewer|maintainer|lead]` - grant access (viewer by default). `--all` grants on every existing project at once, skipping projects where the user is already a member (their role is never silently overwritten).
- `lific member role -p <IDENT> -u <name> -r <role>` - change a role. The last-lead guard applies: you cannot demote a project's only lead.
- `lific member remove -p <IDENT> -u <name>` - revoke access (same last-lead guard; `projects.lead_user_id` is repaired when the removed user was the primary lead).

Membership changes are audit-logged like every other write (the `project_members` triggers from 2.0 cover CLI writes automatically). JSON output on `--json` or piped stdout, as everywhere.

### `lific user set-password` - operator password reset

There was no password reset at all: the web UI's change-password requires the current password, and a forgotten one meant SQL surgery. `lific user set-password --username <name>` sets a new one from the shell (masked prompt on a TTY, read-a-line when piped, `--password` for scripts). Shell access to the server is the trust boundary, same as `user create --admin`. Matching self-service semantics, a reset invalidates **all** of the user's sessions.

### `lific init` and `lific service install` honor `--config`

"lific service sets the wrong config path every time, even after passing the flag" - correct, it did. Both commands hardcoded `./lific.toml` from the invocation cwd when rendering the service definition, so the installed unit could point at a different config than the one you named.

- Both commands now root the instance at `--config <path>`: `init` creates the file there (parent directories included), and the service definition's `ExecStart`/`WorkingDirectory` derive from the config file's canonical location - a relative `database.path` resolves beside the config at runtime, exactly as `init` resolved it at setup time.
- `lific service install --config <missing path>` fails fast with the path it looked at instead of silently installing a unit for the wrong instance.

### Auth optional through config - `[auth] required = false`

For a private, local, single-user instance, authentication itself can now be turned off: with `required = false` under `[auth]`, a request that presents **no credential at all** is treated as operator-equivalent (the same trust rail 2.0 gave unbound API keys) on both REST and MCP. A presented-but-invalid token still 401s - a broken client config surfaces as an error instead of silently degrading to anonymous-with-admin-powers, and real credentials keep resolving to their real identity.

Deliberately a config-file key rather than a runtime instance setting: turning auth off requires shell access to the server, exactly like minting an operator key. Guard rails: `lific start` **refuses to boot** when auth is optional and `server.public_url` points anywhere but localhost (loopback IPs are verified as IPs - `127.evil.com` doesn't count), and otherwise logs a prominent warning that the default `0.0.0.0` bind makes the instance LAN-reachable.

### `lific init` uses standard OS directories by default

`init` used to create `lific.toml` + `lific.db` in whatever directory it ran from - run it in three directories, get three accidental instances. A bare `lific init` now roots the instance in your OS's standard locations: config at `~/.config/lific/lific.toml` and database at `~/.local/share/lific/lific.db` on Linux (macOS/Windows equivalents), with backups and attachments beside the database in the data dir. Since config discovery already probes the user config dir, every other command finds this instance from any directory with no flags.

- `lific init --here` keeps the old directory-local layout (`./lific.toml` + `./lific.db`) for repo-scoped instances.
- A `lific.toml` already in the current directory wins over the OS dirs: re-running bare `init` beside an existing directory-local instance repairs it rather than silently creating a second instance in XDG.
- `lific service install` without `--config` now discovers the config the same way `Config::load` does (cwd, then user config dir, then system config dir) instead of insisting on `./lific.toml`.

### Config discovered in standard system locations

The config search order gains the platform system config dir as a last-resort fallback, for one machine-level config shared by every invocation: `/etc/lific/lific.toml` on Linux/BSD, `/Library/Application Support/Lific/lific.toml` on macOS, `%ProgramData%\lific\lific.toml` on Windows. Full order: `--config` > `./lific.toml` > user config dir (`~/.config/lific/`, `$XDG_CONFIG_HOME` respected) > system config dir. First match wins; a relative `database.path` anchors to the config file's own directory regardless of where it was found.

## v2.0.0 (2026-07-04)

Lific 2.0 is three releases in one. The web UI moves from complete to fast, personal, and pleasant: a real home page, analytics, saved views, undo, a peek panel, full keyboard control, and a theming system. Underneath it, Lific gets real authorization. Project-scoped membership and roles are enforced identically across the REST API and every MCP tool, **on by default for fresh installs** and opt-in for instances upgrading from 1.x (existing setups keep working bit-for-bit). And around it, a new CLI onboards the whole thing in two commands: `lific init` builds a running, boot-persistent instance and `lific connect` wires any of 11 AI clients to it, with health checks, device-flow login, and per-tool agent identities.

### The CLI got a facelift - clack-style sessions and real selectors

Human-facing CLI output moved from bare `println!` walls to a proper prompt UI (via `cliclack`, the Rust implementation of the @clack/prompts look): a `┌ lific init` session header, a gutter connecting `◇` completed steps, boxed notes for things you must actually read (API keys, next steps, manual snippets), and a `└` outro.

- **`lific connect` has a real picker now.** The "type comma-separated numbers" prompt is gone: an arrow-key multi-select lists every client with installed ones marked `(detected)` and preselected - space toggles, enter confirms. The AGENTS.md question is a proper confirm prompt.
- **Sessions everywhere**: `init`, `connect`, `doctor` (`◇`/`▲`/`■` per check severity, summary as the outro), `login` (code in a note block, a live spinner while waiting for approval), `service`, `restore`, `dump`, plus key and user management output.
- **`lific user create`'s password prompt is masked now** - it previously echoed the password in plaintext.
- **Agents see zero change.** JSON output (explicit `--json` or piped stdout), non-TTY fail-fast prompts, and every machine-readable shape are byte-for-byte untouched; the pretty layer renders only for humans at a terminal.

### `lific connect` can no longer wire your tools to the wrong instance silently

Running `connect` from the wrong directory used to be a quiet disaster: it would silently create a brand-new empty `lific.db` in whatever directory you happened to be in, mint keys against it, and rewrite every selected client's `lific` entry to point there - replacing their config for your real instance without a word about which instance it was targeting.

- **Connect refuses to run where no instance exists.** If the resolved database file isn't there, it errors with directions (`run from the instance directory, pass --config/--db, or lific init`) instead of conjuring a fresh one.
- **The target is announced up front**: the session opens with `Instance: <url> (keys minted in <db path>)`, and the client picker itself asks "Which clients should connect to <url>?" - wiring tools to the wrong instance now requires ignoring two explicit statements of it.
- **`--config` works from anywhere now.** A relative `database.path` in a config file resolves against the config file's directory, not the process cwd - previously `lific --config /srv/lific/lific.toml <cmd>` run from elsewhere would look for (or create) the database in your cwd. Backups anchor the same way.

### `lific init` now sets up everything - including a service that survives reboot

The 60-second setup used to end with a server tied to an open terminal: close it (or log out) and your agents' "missing memory" was gone. `lific init` is now the whole onboarding story:

- **One command**: writes `lific.toml` (kept if present), creates and migrates the database, mints and prints your initial API key, installs a background service, starts it, and verifies the server actually answers before claiming success. Re-running is safe and repairs whatever is missing.
- **Real service integration**: a systemd user unit on Linux (`~/.config/systemd/user/lific.service`, with best-effort `loginctl enable-linger` so it outlives logout) or a launchd LaunchAgent on macOS (`~/Library/LaunchAgents/dev.lific.plist`). Starts on boot, restarts on failure.
- **`lific service install | uninstall | status | stop | restart`** manages the service afterward; `status` exits nonzero when the service isn't running, so scripts and agents can gate on it.
- **Honest failure modes**: no service manager (containers, WSL without systemd) falls back to clear `lific start` instructions; a port squatted by another process is diagnosed as such instead of reported as success (init cross-checks the unit's own state against the health probe, so a stranger answering on :3456 can't fake a working install).
- **The API key prints during `init`, in your terminal** - not into a service journal nobody reads. The old box-drawing around the key (which rendered broken) is gone; `lific start` remains for foreground use (`lific init --no-service` skips service setup entirely).
- The README's 60-second setup now matches reality, and its `lific user promote <username>` example is corrected to the actual `--username` syntax.

### Authorization on by default for fresh installs

Project-scoped authorization (detailed below) would be pointless shipped dark: a brand-new install with `authz_enforced` off has no authorization at all - any valid bearer token could read, mutate, or delete every project. So fresh installs **enforce by default**, without breaking the zero-user `init → connect` flow; instances upgrading from 1.x keep enforcement off until an admin opts in.

- **Install-dependent seed.** On the first run that creates the settings row, `authz_enforced` is seeded from whether the database has any users yet: a fresh install (zero users) enforces by default; an instance upgraded from an earlier version (users already exist) stays off. The row is authoritative once written - later starts never re-evaluate or flip it, and an admin who turns enforcement off stays off.
- **Operator-key trust rule.** The agent-first flow runs on user-unbound API keys, which resolve to no effective user and would be default-denied under enforcement. Such keys can only be minted with shell access to the server (`lific start`'s auto-key, `lific key create`, `connect`'s fresh-install path), so in enforced mode they are now treated as **operator-trusted** (admin-equivalent). The signal is credential-type-specific and set only on the unbound-API-key auth path - a legacy pre-binding OAuth token also resolves to no user but is **not** granted operator power and stays default-denied (covered by explicit regression tests on both REST and MCP).
- **Unbound API keys bypass authorization by design.** Audit them with `lific key list`. Prefer per-tool bot identities (what `lific connect` mints once you have a user account), which inherit their owner's project access and are attributed by name.

### Project membership and roles

Until now, authentication was a door with no rooms behind it: any logged-in account - and any connected agent - could read, edit, or delete content in every project. 2.0 adds project-scoped membership and roles, so an agent holds exactly the authority its owner granted it and nothing more.

- **Three roles per project**: `viewer` (read + comment), `maintainer` (full content and structure CRUD), and `lead` (everything, plus settings, membership, and project deletion). Multiple leads per project are supported; global admins override everything as the break-glass path.
- **Default-deny, reads included.** With enforcement on, a non-member sees nothing - projects vanish from lists and search, and direct reads are refused. There is no implicit access floor.
- **One enforcement layer, two transports.** REST handlers and all 29 MCP tools call the same `authz` module, so the web UI and agents can never drift apart. Cross-project operations (issue relations, plan-step issue links) require the role on every project touched.
- **Agents inherit their owner.** A bot acts with its owning user's memberships and can never exceed them; OAuth-token requests resolve to their real user end to end. A token-backed agent that is a member keeps working under default-deny - verified by explicit lockout-regression tests on both transports.
- **Safe, reversible rollout.** Enforcement is a runtime instance setting (`authz_enforced`, seeded on for fresh installs and off for upgrades; flip it anytime in Instance Settings or `lific instance set --authz-enforced true`). Legacy mode preserves pre-2.0 behavior bit-for-bit; existing project leads are backfilled as `lead` members automatically.
- **Membership management** in Project Settings: list members with role badges, add by name, change roles inline, remove with confirmation - lead-gated, with last-lead protection so a project can't be orphaned. Every membership change lands in the audit log with actor attribution.
- **Enumeration-derived coverage.** The test suite extracts every REST route and every MCP tool and fails if any surface is missing an authorization classification, so future endpoints can't ship ungated. The suite now stands at 931 tests.

### Connect an agent in one command

- **`lific connect <tool>`** writes working MCP config into 11 AI clients - OpenCode, Claude Code, Claude Desktop, Cursor, VS Code, Codex, Zed, Gemini CLI, Windsurf, Goose, and Crush - globally or per-project, over stdio or HTTP. Each connected tool gets its own bot identity, so the audit log shows *which* agent did what; `--oauth` connects native-auth clients without minting a key.
- **`lific doctor`** health-checks config, database, backups, server reachability, OAuth, and MCP wiring, with actionable fix hints.
- **`lific login` / `logout`**: two-step device-flow auth (RFC 8628) with keyring-backed credential storage - no pasting API keys.
- **`lific agents-md`** writes a maintained Lific section into a repo's AGENTS.md so agents learn the house conventions.
- **Terminal citizenship**: shell completions for bash/zsh/fish, TTY-aware output (auto-JSON when piped, prompts never hang non-interactive runs), and piped output can no longer panic on SIGPIPE.
- **For agents over MCP**: the server's instructions now teach Lific workflow conventions, cold read tools nudge self-onboarding on a zero-project instance, and the repo ships an MCP Registry manifest and publish runbook.

### Agent tooling (MCP)

- **Edit and delete comments over MCP**: new `edit_comment` and `delete_comment` tools, enforcing the same author-or-admin ownership rules as their REST counterparts.
- **Batch issue edits in one call**: `bulk_update` applies a status/priority/module change to every issue matching a filter (capped at 500) and returns the affected count - triage that was N round-trips is now one.
- **Schedule issues over MCP**: `create_issue` and `update_issue` now accept `start_date` and `target_date`, which already existed everywhere but the MCP layer.
- **Clear fields, not just set them**: MCP can unassign an issue's module, move a page back to the folder root, and set or clear project and module emoji (empty string clears; omitted still skips).
- **Find what's stuck**: `list_issues` gains a `blocked=true` filter - the inverse of `workable` - surfacing each blocked issue's unresolved blockers.
- **Comments join full-text search**: comment threads are now indexed alongside issues and pages across search, MCP, and the web UI, with hits linking back to their parent issue or page.
- **Duplicate relations are visible**: issues linked as `duplicate` now show that relation in `get_issue`, MCP output, and markdown export - it was previously write-only.
- **Page listings paginate**: `list_resources(page)` honors the `limit`/`offset` it always documented, with the same over-fetch has-more hint as issue listings.

### Account and instance settings

- **Account settings**: profile editing (display name, email), change password, and sign-out-everywhere. Changing your password revokes every other session - a stolen token dies the moment you rotate - while your current browser stays signed in.
- **Instance settings**: a DB-backed, admin-gated settings surface - name your instance, open or close signup, toggle authorization enforcement, and enable single-user auto-login (skip the login screen entirely on a personal single-account instance). Editable in the UI or via `lific instance set`.
- **Connected-tools flow redesigned**: a stepped connect modal with per-OS config paths, masked keys, copyable command chips, and real brand logos for every supported client.

### A place to land

- **My Work home dashboard**: the new default landing page - your active issues grouped by project, recently viewed items, pinned pages, a cross-project activity digest, and quick actions. Login and signup land here now.
- **Insights**: a per-project analytics tab - created-vs-closed weekly trends (hand-rolled SVG, reopen-aware closure counting), current status/priority/module distributions, and most-active actors, with a 4/12/26/52-week window.

### A faster issue surface

- **Saved views**: persist any filter/group/sort/layout combo as a named per-user view, switchable from the topbar, with a default view that auto-applies per project. Private to each user, project-visibility enforced.
- **Board v2**: swimlanes by module or priority (drag across a lane updates both status and the lane field in one move), collapsible columns that stay valid drop targets, and proper scroll-snap columns on mobile.
- **Issue peek panel**: preview an issue in a slide-over (bottom sheet on mobile) without leaving the list or board - quick status/priority/module edits included. Cmd/ctrl-click a board card or use the row's hover affordance.
- **Keyboard-first navigation**: j/k focus that survives refetches, x to select, enter to open, space to peek, s/p/m open pickers on the focused row (shift+S/P keep the old quick-cycle), and a `?` help overlay generated from a single shortcut registry so it can't drift from reality.
- **Undo**: status, priority, and module changes (from the list, board drags, detail view, and bulk operations) now confirm with a toast carrying a real Undo action. One unified toast system across the app (accessible live regions, hover/focus pauses dismissal).
- **Undo-able deletes.** Deleting issues (single or bulk) is deferred: rows vanish instantly, a toast offers Undo, and the actual delete only fires once the toast closes. Closing the tab flushes the pending delete instead of silently cancelling it.

### Everywhere else

- **Issue references come alive**: bare identifiers (LIF-42, PROJ-DOC-3) auto-link in all rendered markdown (code blocks correctly excluded), show rich hover preview cards, and autocomplete in every editor via `#` or an identifier prefix at the caret. Issue chips learned tricks too: shift-click opens the peek panel, right-click offers preview and open-in-new-tab.
- **Path-style deep links**: plain URLs like `/LIF/issues/LIF-42` resolve into the app at boot, so links from dashboards, chats, and agents land directly on the right view.
- **Appearance system**: six accent presets (all AA-verified in both modes, including a fix to the stock indigo dark-mode contrast), comfortable/compact density, three font scales, and a reduced-motion preference that every animation in the app honors - applied before first paint, no flash.
- **Motion & loading polish**: content-shaped skeletons replace spinners on every heavy route, list rows and board cards glide on reorder, routes fade in quietly, and transition durations are normalized app-wide.
- **Markdown formatting toolbar**: bold, italic, headings, lists, checklists, code, links, and quotes in every editor, with Cmd+B / Cmd+I / Cmd+Shift+K shortcuts. Transforms toggle cleanly and play nice with native undo.
- **Live timestamps**: relative times ("2m ago") tick as time passes instead of going stale, and hovering any of them shows the exact date.
- **Consistent breadcrumbs**: issue, page, module, and plan detail views share one breadcrumb trail (PROJ > Issues > LIF-42) instead of ad-hoc back arrows.
- **No silent failures**: saves, deletes, comments, and clipboard copies that used to fail without a word now surface an error toast; copy actions confirm.
- **Edit and merge labels.** Labels can now be renamed and recolored in place, and duplicate labels can be merged (issues and pages re-tagged, source label removed) - with a full label manager and color picker in Project Settings.
- **Pinned pages** stay at the top of the page list.

### Design and mobile

- **Login and signup redesigned** around the brand - and meet Lizzy, the mascot who now staffs the empty states, error pages, and the sign-in screen.
- **Real error pages**: a 404 and a global error boundary that recover gracefully without leaking internals.
- **Light theme contrast overhaul** and a typography token system (display through micro) replacing ad-hoc pixel sizes app-wide.
- **Mobile pass**: off-canvas navigation drawer, reflowed topbars, issue rows, and detail views, board snap-scroll columns, and touch-reachable actions.
- **Topbar filters consolidated** into a single Filter popover; projects reorder by drag in the sidebar, with collapsible per-project sub-navigation.

### Security fixes

- **Password changes revoke all other sessions** - a stolen session token no longer survives a password rotation.
- **The session cookie's `Secure` flag is now gated on the request scheme**, fixing broken logins on plain-http and localhost deploys.
- **OAuth approval CSRF tokens are bound to the approving session** (previously forgeable across users), the CSRF MAC comparison is constant-time, and token revocation validates its bearer before acting.
- **API key expiry is now enforced.** `expires_at` existed in the schema and was shown by `lific key list`, but the auth path never checked it - an expired key authenticated forever. Both key lookups now reject expired keys, and `lific key create` gains `--expires`.

### Performance

- Issue list label hydration is O(1) - one query instead of one per row.
- Hot read paths cache prepared statements.
- `list_plans` is 2x faster via page-then-aggregate.

### Upgrading

- The database upgrades itself automatically on first launch. Upgrading from any 1.x is safe and needs no manual steps.
- **Fresh installs enforce authorization by default; upgrades from 1.x keep it off.** An instance that already has users behaves exactly as before until an admin flips `authz_enforced` in Instance Settings or runs `lific instance set --authz-enforced true`. Project leads are backfilled as members automatically, so flipping it on does not lock anyone out of their own projects.
- Unbound API keys are operator-trusted and bypass authorization in enforced mode. Review them with `lific key list` and revoke any you don't recognize.

## v1.6.0 (2026-06-15)

Lific gets a planning layer. Plans turn a goal into an ordered, arbitrarily-nestable tree of steps that persists across sessions and context compaction - the thing that separates an issue tracker from a project planner. Steps can mirror issues, so closing an issue checks its step and completing a step closes its issue, all recorded in the audit log.

### Plans

- **Persisted, nestable step trees.** A plan is a first-class, project-scoped tree of steps (steps containing steps, any depth) that survives across agent sessions and compaction. Issues stay flat and lateral; the hierarchy lives on the plan.
- **Steps mirror issues, both ways.** Link a step to an issue and the two stay in sync: closing the issue checks the step (anywhere it appears), and marking a step done closes its issue. Reopening an issue reopens its steps in active plans, stamped with the reason. Closing a plan's anchor issue auto-archives the plan. Done flows down from issues, never silently up from plans.
- **Authored in one call.** Four MCP tools: `create_plan` builds a full nested tree at once, `get_plan` rehydrates it for the next session, and `edit_plan_step` / `update_plan_step` handle surgical edits, done toggles, issue links, and structure changes - with every side effect reported back in the result.
- **First-class in the web UI.** A Plans tab alongside Issues, Board, Modules, and Pages: a list grouped by status and a detail view with a real nested tree - done toggles, per-step markdown descriptions, issue chips with provenance, an anchor issue, a progress bar, and an activity timeline. Built on the same shell as the issue and page views.
- **Fully audited.** Every plan and step mutation lands in the audit log with actor attribution, including the issue-driven cascades (recorded as system-driven via the triggering issue). A new `/api/plans/{id}/activity` surface and plan support across `list_resources` and `delete`.
- **REST + CLI.** Full `/api/plans` CRUD plus step operations, identifiers as `PROJ-PLAN-n`.

### Issue list

- **Accurate per-status tallies in the topbar.** The count was previously `filteredIssues.length` over a fetch capped at 200, so it silently undercounted once a project grew past that. A new `count_issues_by_status` query (a single indexed `GROUP BY`) and `GET /api/projects/{id}/issue-counts` endpoint return true per-status counts and a real total.
- **Click a status count to toggle that filter**, with narrowed views rendering "shown of total" so the number is always honest.
- **List fetch limit raised 200 → 1000** so rows don't truncate as early.

## v1.5.0 (2026-06-10)

Lific learns to remember and to listen. Every change is now recorded in an audit log - who did it, what changed, and whether it came through the web UI, an agent over MCP, the API, or the CLI - with activity surfaces across the app to read that history. A command palette puts every issue, page, project, and action one keystroke away. The issue list gains multi-select with bulk editing, connected tools get much richer query controls, and a sweep of UI fixes lands across every view.

### Audit log and activity

- **Every mutation is recorded**: issue, page, project, module, label, folder, and comment changes land in an append-only audit log with per-field old → new values. Edits to titles, descriptions, statuses, priorities, modules, labels, relations, and more are captured individually - no opaque blobs.
- **Full actor attribution**: each entry records who acted and through which door - a person in the web UI, an agent over MCP (shown as its bot identity, e.g. `opencode-blake · agent · via mcp`), a direct API call, or the CLI. Trustworthy answers to "did the agent do this, or did I?"
- **Capture is at the database layer**, so every write path is covered uniformly - including future ones. History survives entity deletion (deleted issues keep their identifier in the log), module/folder/lead changes record names rather than ids, and rolled-back transactions are never recorded.
- **Project Activity page**: a new "Activity" view in each project's sidebar shows everything that happened, newest first, grouped by day. Entries link to their entities, expand to show exact timestamps (local and UTC), full old → new values, and the actor's standing in the project ("412 actions · 2nd most active · last seen 3m ago"). An actor rail ranks everyone who has touched the project - humans and agents - by action count, each a one-click feed filter. The feed updates live.
- **Activity timelines on issue and page detail**: a quiet history between the description and comments - status and priority changes with their icons, expandable description-diff blocks, label and relation events, agent badges, and "via web/mcp/api/cli" attribution. Updates immediately after your own edits.
- **For integrations**: a new `get_activity` tool answers "what changed while I was gone" for any issue, page, or whole project, and the REST API gains `/activity` endpoints for issues, pages, and projects plus a per-project actor rollup.

### Command palette

- **`Cmd+K` or `Ctrl+P` from anywhere** opens a jump-to-anything palette covering projects, issues, pages, modules, and folders.
- **It understands identifiers**: `OMN156`, `omn 156`, and `OMN-156` all resolve to issue OMN-156; `lif doc 3` finds the page; a bare `156` is probed across every project and lists all hits.
- Free text searches issues and pages full-text, merged with fuzzy matches over projects, modules, and folders. The best-matching group leads the list, typing a project's name takes you to it, and an empty query doubles as a project switcher.
- **Context actions**: on an issue, the palette offers Set status, Set priority, Set module, Add or remove label (with current values shown), Rename, Edit description, and Add comment - submenus are filterable, rename turns the palette into a prefilled prompt, and every action lands in the audit log like any other edit. Pages get their lifecycle status and labels. Creating a project is available from every view.

### Issue list: multi-select and bulk editing

- Select with `x`, extend with `shift+↑/↓` (or `shift+j/k`), shift-click for ranges, ctrl/cmd-click to toggle - then apply status, priority, module, or a label to everything at once from a floating action bar, or delete behind a confirm. Triage that used to be N round-trips through the detail page is now one pass.
- Selection is keyboard-cheatsheet documented, pauses auto-refresh while active, and survives background updates.
- The board's per-column "+" now creates the issue in that column's status instead of silently defaulting to backlog.

### Integrations

- `search` supports filtering by result type (issue or page), relevance or most-recent sorting, and offset paging with has-more hints.
- `list_issues` supports created/updated date windows (`created_since`, `updated_until`, …) and explicit ordering by sort order, sequence, created, or updated - ascending or descending.
- Page listings gain the same ordering controls plus the status filter; page lines and `get_page` now include status, folder, and timestamps.
- `list_comments` can filter by author and sort in either direction.
- All ordering values are strictly whitelisted - invalid values error instead of being interpolated.

### Web fixes and polish

- Issue status icons are now one shared vocabulary everywhere - the new-issue form's mismatched colored dots are gone, and module pages use the same glyphs as the rest of the app.
- The high-priority orange and destructive-action colors are theme-aware tokens: "high" reads correctly in both modes, and red Delete buttons are no longer unreadable in dark mode.
- An issue's status now shows in the detail-page breadcrumb.
- Clicking a title to rename it shows the intended accent underline again, and priority icons in issue rows are properly sized.
- Pages list: the count matches what's shown when archived pages are hidden, the status pill only appears for non-default stages (Draft/Complete/Archived) instead of on every row, and the updated date is always visible - without jittering the status pill's position.
- Folders can no longer be dragged into each other - the move looked successful but was never persisted. Page drag-and-drop is unchanged.
- The breadcrumb says "Board" on the board view, board column visibility pills show their counts correctly, and shift-click range selection no longer sweeps text selection across rows.
- Signing in goes straight to Settings without a redirect flash, and ~450 lines of dead pre-1.4 UI code are gone.

### Upgrading

- The database upgrades itself automatically on first launch (one new migration). Upgrading from any 1.x is safe and needs no manual steps. Audit history begins at the moment of upgrade - earlier changes were not recorded and cannot be backfilled.

## v1.4.1 (2026-06-09)

A maintenance release: a sweep of correctness and security fixes across the database, auth, and MCP layers, plus server and web improvements that landed after v1.4.0.

### Fixes

- Creating an issue is now atomic - a failed label attach can no longer leave a half-created issue behind.
- Rotating an API key keeps its user binding, so rotated bot/tool keys no longer lose their comment attribution.
- Empty or whitespace-only search queries return no results instead of a database error.
- Project identifiers are validated on create and update: uppercase letters and digits, at most 5 characters, starting with a letter. Hyphenated, lowercase, or empty identifiers (which silently broke issue lookups) and the reserved word `DOC` are rejected.
- An issue can no longer be linked to itself - a self-"blocks" previously made it permanently unworkable.
- Board columns follow workflow order (backlog → todo → active → done → cancelled) and priority severity, instead of alphabetical order.
- Auto-refresh no longer stacks duplicate fetches when navigating between views.
- OAuth protected-resource metadata advertises the `/mcp`-qualified resource so claude.ai web accepts issued tokens.

### Server and web

- Responses are gzip/brotli compressed and content-hashed assets are cached immutably, dramatically cutting first-load time on slow links.
- Issue list, board, and page views auto-refresh to reflect changes without a manual reload.
- Optional authless MCP endpoint at `/mcp/<token>` to work around claude.ai web's broken OAuth connector flow.
- Priority icons are now consistent across the UI.
- The root URL lands on Settings instead of the first project's issue list.

## v1.4.0 (2026-05-28)

The biggest release yet. Pages become first-class documents with comments, labels, lifecycle status, and search. Issues gain fuzzy search and activity-aware sorting. Modules get a real management UI and icons. The markdown renderer learns Mermaid diagrams and code-copy buttons, the commenting experience is rebuilt, and login and OAuth security are meaningfully hardened. (This is the first GitHub release since v1.1.3; the 1.2.x and 1.3.x line shipped on crates.io only.)

### Pages

- Threaded comments, the same as issues.
- Labels, with the same tagging and filtering as issues.
- A lifecycle status (Draft, Active, Complete, or Archived), shown and filterable in the page list and available everywhere pages are: web, API, CLI, and connected tools.
- Full-text search across title and content, plus instant filtering in the page list.

### Issues

- Fuzzy full-text search across title, identifier, and description.
- Sort by most recent activity. Adding a comment or changing labels now counts as activity, not just editing the issue itself.

### Modules

- A dedicated management UI: list, detail, and sidebar navigation.
- Icons, picked the same way as project icons (a built-in glyph or any emoji).

### Markdown and editing

- Mermaid diagrams render from fenced `mermaid` code blocks, anywhere markdown appears.
- A one-click copy button on code blocks.
- An explicit Edit/Preview toggle for page and issue bodies, replacing click-to-edit, so selecting and copying text no longer drops you into the editor. Press `E` to edit.
- Quote-to-comment: highlight text in a page or issue and quote it directly into a comment.

### Comments

- The comment thread was rebuilt from the ground up.

### Integrations

- New `edit_issue` and `edit_page` tools let agents make targeted find-and-replace edits to an issue description or page body, instead of resending the whole field.
- Pages are now fully accessible to connected tools, including their comments, status, and labels, and module icons are exposed too.
- Adding a comment returns a leaner response (an id and metadata) instead of echoing the whole comment back.

### Throughout

- Issue, page, and module detail pages now share one consistent layout.
- A unified top bar across the app, a refreshed New Issue panel, the app version shown in the sidebar and on the sign-in screen, and the logo now links to the project repository.
- Removed a "display options" dropdown that never did anything. Grouping and density controls are still planned.

### Security

- Login rate limiting now applies per source IP in addition to per username, closing a lockout vector where someone could lock you out just by guessing your username. A counting bug that effectively halved the limit was also fixed.
- OAuth access tokens are now tied to the user who approved them, so connected tools act as that user rather than an anonymous identity. Existing tokens keep working.

### Fixes

- Projects with no assigned lead can be edited again, and a project's lead or icon can be cleared.
- Fixed a crash in issue search.
- The page tree now fills the available width.

### Upgrading

- The database upgrades itself automatically on first launch. Upgrading from any 1.x is safe and needs no manual steps.

## v1.3.1 (2026-05-17)

Bug-fix release (crates.io).

- Relations between issues in different projects now show the correct identifier.
- Issue list and board view state is preserved when navigating into an issue and back.
- Page content moved to double-click-to-edit (later replaced by the Edit/Preview toggle in v1.4.0).

## v1.3.0 (2026-05-14)

Major web UI release (crates.io).

- A redesigned interface with a kanban board view and drag-and-drop status changes.
- Browser-based integrations can now connect (CORS).

## v1.2.1 (2026-05-03)

Bug-fix release (crates.io).

- Comments added through local/stdio integrations are attributed to the first admin user.

## v1.2.0 (2026-05-02)

Feature release (crates.io).

### Features

- Full command-line CRUD for issues, projects, pages, and resources.
- Markdown export for issues, pages, and projects.
- Pagination for integration list operations.

### Security Fixes

- Hardened OAuth client registration with rate limiting and redirect-URI validation.

### Bug Fixes

- Compatibility fixes for various integration clients and reverse proxies.

### CI

- Dropped Windows build targets from the release pipeline.

## v1.1.3 (2026-04-06)

Security hardening release closing the remaining vulnerabilities identified in the auth audit.

### Security Fixes

- **CSRF on OAuth authorize form**: The OAuth approval form had no CSRF protection. An attacker could auto-submit the form from a malicious page, tricking a logged-in user into granting a 30-day access token. Added HMAC-SHA256 CSRF tokens with 10-minute expiry.
- **Signup CPU exhaustion**: The signup endpoint had no rate limiting, allowing attackers to burn CPU by spamming Argon2 password hashing. Added rate limiting keyed by email.
- **CORS allows any origin**: CORS was hardcoded to `Any`. Added a `server.cors_origins` config option. Falls back to `Any` for development if unset.
- **Session tokens stored plaintext**: Session tokens were stored as-is in the database. A database leak (backup, disk access) exposed all active sessions. Now stored as SHA-256 hashes.
- **OAuth revocation unauthenticated**: Anyone could revoke any OAuth token without authentication. Now requires a valid Bearer token.
- **Username enumeration via timing**: Login for non-existent users returned faster than wrong-password logins (no Argon2 computation). Added dummy Argon2 verification to normalize timing.

### CI

- Unified auto-tag and release into a single workflow to fix cross-workflow token permission issues.

### Upgrade Notes

- **Existing sessions are invalidated**: Sessions created before this version used plaintext storage and will no longer validate against the new SHA-256 lookup. Users will need to log in again.
- New config option: `server.cors_origins` (array of allowed origins). If unset, CORS allows all origins (previous behavior). Set this in production.

## v1.1.2 (2026-04-06)

Security and correctness fixes for auth endpoints, cookies, and server hardening.

### Security Fixes

- **Comment auth bypass**: `add_comment` silently fell back to the first admin user when no auth context was present. Now requires authentication and returns an error.
- **OAuth client_id not required**: Token exchange accepted requests without `client_id`, violating OAuth 2.1 for public clients. Now required.
- **Argon2 CPU DoS via password length**: No max password length was enforced. A multi-megabyte password would burn CPU in Argon2. Added a 1024-character max on both signup and login.
- **Session cookie missing security flags**: Session cookies lacked HttpOnly, Secure, and SameSite attributes. Added `HttpOnly; Secure; SameSite=Lax` to login, signup, and logout cookie handling.
- **World-readable backups**: Backup files were created with default permissions (0644). Now set to 0600 (owner-only) on Unix.
- **No request body size limit**: No limit on request body size allowed memory exhaustion via large POSTs. Added a 2MB default limit.

## v1.1.1 (2026-04-06)

Stability and data integrity fixes.

### Security Fixes

- **SQL injection via table name**: `get_resource_project_id` interpolated the table name directly into SQL. Added whitelist validation for allowed table names.

### Bug Fixes

- **Mutex poison crash**: The rate limiter panicked on a poisoned Mutex, crashing the process. Now recovers gracefully.
- **OAuth writes silently discarded**: Five database write operations in OAuth discarded their errors. Now propagated with proper error responses.
- **Non-atomic multi-statement writes**: Update operations for issues, projects, modules, labels, and pages ran multiple SQL statements without transactions. A failure mid-way left partial state. Wrapped in SQLite savepoints.
- **Migrations not atomic**: Each migration's SQL and tracking insert ran without a transaction. Wrapped in savepoints so partial failures roll back.
- **Rate limiter memory leak**: The rate limiter's map never evicted expired keys, growing without bound. Added a periodic sweep when the key count exceeds a threshold.

### CI

- Fixed the auto-tag workflow (missing git identity for annotated tags).
- Fixed crates.io publish (verification build failed without `web/dist/`).

## v1.1.0 (2026-04-06)

Security release closing 6 critical authentication and authorization vulnerabilities.

### Security Fixes

- **Privilege escalation via missing auth check**: `require_admin` and `require_project_lead` returned success when no user was associated with the request (OAuth tokens, legacy API keys). Any unauthenticated but authorized request had full admin privileges. Now default-deny.
- **OAuth PKCE bypass**: The `plain` PKCE method was accepted despite OAuth 2.1 requiring S256 only. Sending an empty challenge and verifier with `method=plain` fully bypassed PKCE. Removed `plain` and reject empty values.
- **OAuth redirect_uri not validated at token exchange**: The `redirect_uri` from the token request was never compared against the one stored with the authorization code. An attacker who intercepted an auth code could exchange it from any URI. Now validated per OAuth 2.1.
- **OAuth access tokens stored plaintext**: OAuth tokens were stored and looked up by raw value. A database leak exposed all active tokens. Now stored as SHA-256 hashes, with the raw token returned only once at issuance.
- **MCP identity confusion under concurrency**: A global mutex stored the current MCP user. Concurrent requests could overwrite each other's identity, and a panic would poison the mutex permanently. Replaced with serialized request handling and poison recovery.
- **Database errors leaked to clients**: Raw SQLite error messages (table names, column names, constraint details, file paths) were returned directly in API responses. Now returns a generic error and logs details server-side.

### Upgrade Notes

- **OAuth tokens are invalidated**: Existing plaintext OAuth tokens will no longer validate since the lookup now expects SHA-256 hashes. Clients will need to re-authorize. This is intentional.
- No database migration required. No config changes.
