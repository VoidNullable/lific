# Changelog

## Unreleased

### `lific init` now sets up everything — including a service that survives reboot

The 60-second setup used to end with a server tied to an open terminal: close it (or log out) and your agents' "missing memory" was gone. `lific init` is now the whole onboarding story:

- **One command**: writes `lific.toml` (kept if present), creates and migrates the database, mints and prints your initial API key, installs a background service, starts it, and verifies the server actually answers before claiming success. Re-running is safe and repairs whatever is missing.
- **Real service integration**: a systemd user unit on Linux (`~/.config/systemd/user/lific.service`, with best-effort `loginctl enable-linger` so it outlives logout) or a launchd LaunchAgent on macOS (`~/Library/LaunchAgents/dev.lific.plist`). Starts on boot, restarts on failure.
- **`lific service install | uninstall | status | stop | restart`** manages the service afterward; `status` exits nonzero when the service isn't running, so scripts and agents can gate on it.
- **Honest failure modes**: no service manager (containers, WSL without systemd) falls back to clear `lific start` instructions; a port squatted by another process is diagnosed as such instead of reported as success (init cross-checks the unit's own state against the health probe, so a stranger answering on :3456 can't fake a working install).
- **The API key prints during `init`, in your terminal** — not into a service journal nobody reads. The old box-drawing around the key (which rendered broken) is gone; `lific start` remains for foreground use (`lific init --no-service` skips service setup entirely).
- The README's 60-second setup now matches reality, and its `lific user promote <username>` example is corrected to the actual `--username` syntax.

### Authorization on by default for fresh installs

2.0 shipped project-scoped authorization opt-in (`authz_enforced` defaulted off), which meant a brand-new install had no authorization at all — any valid bearer token could read, mutate, or delete every project. Fresh installs now **enforce by default**, without breaking the zero-user `init → start → connect` flow.

- **Install-dependent seed.** On the first run that creates the settings row, `authz_enforced` is seeded from whether the database has any users yet: a fresh install (zero users) enforces by default; an instance upgraded from an earlier version (users already exist) stays off. The row is authoritative once written — later starts never re-evaluate or flip it, and an admin who turns enforcement off stays off.
- **Operator-key trust rule.** The agent-first flow runs on user-unbound API keys, which resolve to no effective user and would be default-denied under enforcement. Such keys can only be minted with shell access to the server (`lific start`'s auto-key, `lific key create`, `connect`'s fresh-install path), so in enforced mode they are now treated as **operator-trusted** (admin-equivalent). The signal is credential-type-specific and set only on the unbound-API-key auth path — a legacy pre-binding OAuth token also resolves to no user but is **not** granted operator power and stays default-denied (covered by explicit regression tests on both REST and MCP).
- **Unbound API keys bypass authorization by design.** Audit them with `lific key list`. Prefer per-tool bot identities (what `lific connect` mints once you have a user account), which inherit their owner's project access and are attributed by name.

### Upgrading

- The database upgrades itself automatically on first launch; there is no migration for this change. **Existing instances are unaffected** — because they already have users (and already have the settings row), enforcement stays exactly where you left it. Only brand-new installs get enforcement on by default.
- If you run a public instance and want authorization on, set it explicitly: `lific instance set --authz-enforced true`.
- Unbound API keys are operator-trusted and bypass authorization in enforced mode. Review them with `lific key list` and revoke any you don't recognize.

## v2.0.0 (2026-07-02)

Lific 2.0 is three releases in one. The web UI moves from complete to fast, personal, and pleasant — a real home page, analytics, saved views, undo, a peek panel, full keyboard control, and a theming system. Underneath it, Lific gets real authorization: project-scoped membership and roles, enforced identically across the REST API and every MCP tool. And around it, a new CLI connects any of 11 AI clients in one command, with health checks, device-flow login, and per-tool agent identities. No breaking API changes — authorization enforcement is opt-in, and existing setups keep working bit-for-bit.

### Project membership and roles

Until now, authentication was a door with no rooms behind it: any logged-in account — and any connected agent — could read, edit, or delete content in every project. 2.0 adds project-scoped membership and roles, so an agent holds exactly the authority its owner granted it and nothing more.

- **Three roles per project**: `viewer` (read + comment), `maintainer` (full content and structure CRUD), and `lead` (everything, plus settings, membership, and project deletion). Multiple leads per project are supported; global admins override everything as the break-glass path.
- **Default-deny, reads included.** With enforcement on, a non-member sees nothing — projects vanish from lists and search, and direct reads are refused. There is no implicit access floor.
- **One enforcement layer, two transports.** REST handlers and all 29 MCP tools call the same `authz` module, so the web UI and agents can never drift apart. Cross-project operations (issue relations, plan-step issue links) require the role on every project touched.
- **Agents inherit their owner.** A bot acts with its owning user's memberships and can never exceed them; OAuth-token requests resolve to their real user end to end. A token-backed agent that is a member keeps working under default-deny — verified by explicit lockout-regression tests on both transports.
- **Safe, reversible rollout.** Enforcement is a runtime instance setting (`authz_enforced`, default off — flip it in Instance Settings or `lific instance set --authz-enforced true`). Legacy mode preserves pre-2.0 behavior bit-for-bit; existing project leads are backfilled as `lead` members automatically.
- **Membership management** in Project Settings: list members with role badges, add by name, change roles inline, remove with confirmation — lead-gated, with last-lead protection so a project can't be orphaned. Every membership change lands in the audit log with actor attribution.
- **Enumeration-derived coverage.** The test suite extracts every REST route and every MCP tool and fails if any surface is missing an authorization classification, so future endpoints can't ship ungated. The suite now stands at 931 tests.

### Connect an agent in one command

- **`lific connect <tool>`** writes working MCP config into 11 AI clients — OpenCode, Claude Code, Claude Desktop, Cursor, VS Code, Codex, Zed, Gemini CLI, Windsurf, Goose, and Crush — globally or per-project, over stdio or HTTP. Each connected tool gets its own bot identity, so the audit log shows *which* agent did what; `--oauth` connects native-auth clients without minting a key.
- **`lific doctor`** health-checks config, database, backups, server reachability, OAuth, and MCP wiring, with actionable fix hints.
- **`lific login` / `logout`**: two-step device-flow auth (RFC 8628) with keyring-backed credential storage — no pasting API keys.
- **`lific agents-md`** writes a maintained Lific section into a repo's AGENTS.md so agents learn the house conventions.
- **Terminal citizenship**: shell completions for bash/zsh/fish, TTY-aware output (auto-JSON when piped, prompts never hang non-interactive runs), and piped output can no longer panic on SIGPIPE.
- **For agents over MCP**: the server's instructions now teach Lific workflow conventions, cold read tools nudge self-onboarding on a zero-project instance, and the repo ships an MCP Registry manifest and publish runbook.

### Agent tooling (MCP)

- **Edit and delete comments over MCP**: new `edit_comment` and `delete_comment` tools, enforcing the same author-or-admin ownership rules as their REST counterparts.
- **Batch issue edits in one call**: `bulk_update` applies a status/priority/module change to every issue matching a filter (capped at 500) and returns the affected count — triage that was N round-trips is now one.
- **Schedule issues over MCP**: `create_issue` and `update_issue` now accept `start_date` and `target_date`, which already existed everywhere but the MCP layer.
- **Clear fields, not just set them**: MCP can unassign an issue's module, move a page back to the folder root, and set or clear project and module emoji (empty string clears; omitted still skips).
- **Find what's stuck**: `list_issues` gains a `blocked=true` filter — the inverse of `workable` — surfacing each blocked issue's unresolved blockers.
- **Comments join full-text search**: comment threads are now indexed alongside issues and pages across search, MCP, and the web UI, with hits linking back to their parent issue or page.
- **Duplicate relations are visible**: issues linked as `duplicate` now show that relation in `get_issue`, MCP output, and markdown export — it was previously write-only.
- **Page listings paginate**: `list_resources(page)` honors the `limit`/`offset` it always documented, with the same over-fetch has-more hint as issue listings.

### Account and instance settings

- **Account settings**: profile editing (display name, email), change password, and sign-out-everywhere. Changing your password revokes every other session — a stolen token dies the moment you rotate — while your current browser stays signed in.
- **Instance settings**: a DB-backed, admin-gated settings surface — name your instance, open or close signup, toggle authorization enforcement, and enable single-user auto-login (skip the login screen entirely on a personal single-account instance). Editable in the UI or via `lific instance set`.
- **Connected-tools flow redesigned**: a stepped connect modal with per-OS config paths, masked keys, copyable command chips, and real brand logos for every supported client.

### A place to land

- **My Work home dashboard**: the new default landing page — your active issues grouped by project, recently viewed items, pinned pages, a cross-project activity digest, and quick actions. Login and signup land here now.
- **Insights**: a per-project analytics tab — created-vs-closed weekly trends (hand-rolled SVG, reopen-aware closure counting), current status/priority/module distributions, and most-active actors, with a 4/12/26/52-week window.

### A faster issue surface

- **Saved views**: persist any filter/group/sort/layout combo as a named per-user view, switchable from the topbar, with a default view that auto-applies per project. Private to each user, project-visibility enforced.
- **Board v2**: swimlanes by module or priority (drag across a lane updates both status and the lane field in one move), collapsible columns that stay valid drop targets, and proper scroll-snap columns on mobile.
- **Issue peek panel**: preview an issue in a slide-over (bottom sheet on mobile) without leaving the list or board — quick status/priority/module edits included. Cmd/ctrl-click a board card or use the row's hover affordance.
- **Keyboard-first navigation**: j/k focus that survives refetches, x to select, enter to open, space to peek, s/p/m open pickers on the focused row (shift+S/P keep the old quick-cycle), and a `?` help overlay generated from a single shortcut registry so it can't drift from reality.
- **Undo**: status, priority, and module changes — from the list, board drags, detail view, and bulk operations — now confirm with a toast carrying a real Undo action. One unified toast system across the app (accessible live regions, hover/focus pauses dismissal).

### Everywhere else

- **Issue references come alive**: bare identifiers (LIF-42, PROJ-DOC-3) auto-link in all rendered markdown (code blocks correctly excluded), show rich hover preview cards, and autocomplete in every editor via `#` or an identifier prefix at the caret. Issue chips learned tricks too: shift-click opens the peek panel, right-click offers preview and open-in-new-tab.
- **Path-style deep links**: plain URLs like `/LIF/issues/LIF-42` resolve into the app at boot, so links from dashboards, chats, and agents land directly on the right view.
- **Appearance system**: six accent presets (all AA-verified in both modes, including a fix to the stock indigo dark-mode contrast), comfortable/compact density, three font scales, and a reduced-motion preference that every animation in the app honors — applied before first paint, no flash.
- **Motion & loading polish**: content-shaped skeletons replace spinners on every heavy route, list rows and board cards glide on reorder, routes fade in quietly, and transition durations are normalized app-wide.
- **Edit and merge labels.** Labels can now be renamed and recolored in place, and duplicate labels can be merged (issues and pages re-tagged, source label removed) — with a full label manager and color picker in Project Settings.
- **Pinned pages** stay at the top of the page list.

### Design and mobile

- **Login and signup redesigned** around the brand — and meet Lizzy, the mascot who now staffs the empty states, error pages, and the sign-in screen.
- **Real error pages**: a 404 and a global error boundary that recover gracefully without leaking internals.
- **Light theme contrast overhaul** and a typography token system (display through micro) replacing ad-hoc pixel sizes app-wide.
- **Mobile pass**: off-canvas navigation drawer, reflowed topbars, issue rows, and detail views, board snap-scroll columns, and touch-reachable actions.
- **Topbar filters consolidated** into a single Filter popover; projects reorder by drag in the sidebar, with collapsible per-project sub-navigation.

### Security fixes

- **Password changes revoke all other sessions** — a stolen session token no longer survives a password rotation.
- **The session cookie's `Secure` flag is now gated on the request scheme**, fixing broken logins on plain-http and localhost deploys.
- **OAuth approval CSRF tokens are bound to the approving session** (previously forgeable across users), the CSRF MAC comparison is constant-time, and token revocation validates its bearer before acting.
- **API key expiry is now enforced.** `expires_at` existed in the schema and was shown by `lific key list`, but the auth path never checked it — an expired key authenticated forever. Both key lookups now reject expired keys, and `lific key create` gains `--expires`.

### Performance

- Issue list label hydration is O(1) — one query instead of one per row.
- Hot read paths cache prepared statements.
- `list_plans` is 2x faster via page-then-aggregate.

### Upgrading

- The database upgrades itself automatically on first launch. Upgrading from any 1.x is safe and needs no manual steps.
- Authorization enforcement ships **off** by default: existing instances behave exactly as before until an admin flips `authz_enforced` in Instance Settings or runs `lific instance set --authz-enforced true`. Project leads are backfilled as members automatically, so flipping it on does not lock anyone out of their own projects.

## v1.6.0 (2026-06-15)

Lific gets a planning layer. Plans turn a goal into an ordered, arbitrarily-nestable tree of steps that persists across sessions and context compaction — the thing that separates an issue tracker from a project planner. Steps can mirror issues, so closing an issue checks its step and completing a step closes its issue, all recorded in the audit log.

### Plans

- **Persisted, nestable step trees.** A plan is a first-class, project-scoped tree of steps (steps containing steps, any depth) that survives across agent sessions and compaction. Issues stay flat and lateral; the hierarchy lives on the plan.
- **Steps mirror issues, both ways.** Link a step to an issue and the two stay in sync: closing the issue checks the step (anywhere it appears), and marking a step done closes its issue. Reopening an issue reopens its steps in active plans, stamped with the reason. Closing a plan's anchor issue auto-archives the plan. Done flows down from issues, never silently up from plans.
- **Authored in one call.** Four MCP tools: `create_plan` builds a full nested tree at once, `get_plan` rehydrates it for the next session, and `edit_plan_step` / `update_plan_step` handle surgical edits, done toggles, issue links, and structure changes — with every side effect reported back in the result.
- **First-class in the web UI.** A Plans tab alongside Issues, Board, Modules, and Pages: a list grouped by status and a detail view with a real nested tree — done toggles, per-step markdown descriptions, issue chips with provenance, an anchor issue, a progress bar, and an activity timeline. Built on the same shell as the issue and page views.
- **Fully audited.** Every plan and step mutation lands in the audit log with actor attribution, including the issue-driven cascades (recorded as system-driven via the triggering issue). A new `/api/plans/{id}/activity` surface and plan support across `list_resources` and `delete`.
- **REST + CLI.** Full `/api/plans` CRUD plus step operations, identifiers as `PROJ-PLAN-n`.

### Issue list

- **Accurate per-status tallies in the topbar.** The count was previously `filteredIssues.length` over a fetch capped at 200, so it silently undercounted once a project grew past that. A new `count_issues_by_status` query (a single indexed `GROUP BY`) and `GET /api/projects/{id}/issue-counts` endpoint return true per-status counts and a real total.
- **Click a status count to toggle that filter**, with narrowed views rendering "shown of total" so the number is always honest.
- **List fetch limit raised 200 → 1000** so rows don't truncate as early.

## v1.5.0 (2026-06-10)

Lific learns to remember and to listen. Every change is now recorded in an audit log — who did it, what changed, and whether it came through the web UI, an agent over MCP, the API, or the CLI — with activity surfaces across the app to read that history. A command palette puts every issue, page, project, and action one keystroke away. The issue list gains multi-select with bulk editing, connected tools get much richer query controls, and a sweep of UI fixes lands across every view.

### Audit log and activity

- **Every mutation is recorded**: issue, page, project, module, label, folder, and comment changes land in an append-only audit log with per-field old → new values. Edits to titles, descriptions, statuses, priorities, modules, labels, relations, and more are captured individually — no opaque blobs.
- **Full actor attribution**: each entry records who acted and through which door — a person in the web UI, an agent over MCP (shown as its bot identity, e.g. `opencode-blake · agent · via mcp`), a direct API call, or the CLI. Trustworthy answers to "did the agent do this, or did I?"
- **Capture is at the database layer**, so every write path is covered uniformly — including future ones. History survives entity deletion (deleted issues keep their identifier in the log), module/folder/lead changes record names rather than ids, and rolled-back transactions are never recorded.
- **Project Activity page**: a new "Activity" view in each project's sidebar shows everything that happened, newest first, grouped by day. Entries link to their entities, expand to show exact timestamps (local and UTC), full old → new values, and the actor's standing in the project ("412 actions · 2nd most active · last seen 3m ago"). An actor rail ranks everyone who has touched the project — humans and agents — by action count, each a one-click feed filter. The feed updates live.
- **Activity timelines on issue and page detail**: a quiet history between the description and comments — status and priority changes with their icons, expandable description-diff blocks, label and relation events, agent badges, and "via web/mcp/api/cli" attribution. Updates immediately after your own edits.
- **For integrations**: a new `get_activity` tool answers "what changed while I was gone" for any issue, page, or whole project, and the REST API gains `/activity` endpoints for issues, pages, and projects plus a per-project actor rollup.

### Command palette

- **`Cmd+K` or `Ctrl+P` from anywhere** opens a jump-to-anything palette covering projects, issues, pages, modules, and folders.
- **It understands identifiers**: `OMN156`, `omn 156`, and `OMN-156` all resolve to issue OMN-156; `lif doc 3` finds the page; a bare `156` is probed across every project and lists all hits.
- Free text searches issues and pages full-text, merged with fuzzy matches over projects, modules, and folders. The best-matching group leads the list, typing a project's name takes you to it, and an empty query doubles as a project switcher.
- **Context actions**: on an issue, the palette offers Set status, Set priority, Set module, Add or remove label (with current values shown), Rename, Edit description, and Add comment — submenus are filterable, rename turns the palette into a prefilled prompt, and every action lands in the audit log like any other edit. Pages get their lifecycle status and labels. Creating a project is available from every view.

### Issue list: multi-select and bulk editing

- Select with `x`, extend with `shift+↑/↓` (or `shift+j/k`), shift-click for ranges, ctrl/cmd-click to toggle — then apply status, priority, module, or a label to everything at once from a floating action bar, or delete behind a confirm. Triage that used to be N round-trips through the detail page is now one pass.
- Selection is keyboard-cheatsheet documented, pauses auto-refresh while active, and survives background updates.
- The board's per-column "+" now creates the issue in that column's status instead of silently defaulting to backlog.

### Integrations

- `search` supports filtering by result type (issue or page), relevance or most-recent sorting, and offset paging with has-more hints.
- `list_issues` supports created/updated date windows (`created_since`, `updated_until`, …) and explicit ordering by sort order, sequence, created, or updated — ascending or descending.
- Page listings gain the same ordering controls plus the status filter; page lines and `get_page` now include status, folder, and timestamps.
- `list_comments` can filter by author and sort in either direction.
- All ordering values are strictly whitelisted — invalid values error instead of being interpolated.

### Web fixes and polish

- Issue status icons are now one shared vocabulary everywhere — the new-issue form's mismatched colored dots are gone, and module pages use the same glyphs as the rest of the app.
- The high-priority orange and destructive-action colors are theme-aware tokens: "high" reads correctly in both modes, and red Delete buttons are no longer unreadable in dark mode.
- An issue's status now shows in the detail-page breadcrumb.
- Clicking a title to rename it shows the intended accent underline again, and priority icons in issue rows are properly sized.
- Pages list: the count matches what's shown when archived pages are hidden, the status pill only appears for non-default stages (Draft/Complete/Archived) instead of on every row, and the updated date is always visible — without jittering the status pill's position.
- Folders can no longer be dragged into each other — the move looked successful but was never persisted. Page drag-and-drop is unchanged.
- The breadcrumb says "Board" on the board view, board column visibility pills show their counts correctly, and shift-click range selection no longer sweeps text selection across rows.
- Signing in goes straight to Settings without a redirect flash, and ~450 lines of dead pre-1.4 UI code are gone.

### Upgrading

- The database upgrades itself automatically on first launch (one new migration). Upgrading from any 1.x is safe and needs no manual steps. Audit history begins at the moment of upgrade — earlier changes were not recorded and cannot be backfilled.

## v1.4.1 (2026-06-09)

A maintenance release: a sweep of correctness and security fixes across the database, auth, and MCP layers, plus server and web improvements that landed after v1.4.0.

### Fixes

- Creating an issue is now atomic — a failed label attach can no longer leave a half-created issue behind.
- Rotating an API key keeps its user binding, so rotated bot/tool keys no longer lose their comment attribution.
- Empty or whitespace-only search queries return no results instead of a database error.
- Project identifiers are validated on create and update: uppercase letters and digits, at most 5 characters, starting with a letter. Hyphenated, lowercase, or empty identifiers (which silently broke issue lookups) and the reserved word `DOC` are rejected.
- An issue can no longer be linked to itself — a self-"blocks" previously made it permanently unworkable.
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
