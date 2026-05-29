# Changelog

## v1.4.0 — unreleased

First properly-published GitHub release since v1.1.3. (v1.2.0 through v1.3.1 shipped on crates.io but stranded as GitHub drafts — root cause and fix in LIF-120 below.) Headline: pages gain comments, labels, and search; issues gain fuzzy search; a module-management UI lands; plus auth/OAuth hardening and a fixed release pipeline.

### Features

- **Threaded comments on pages** (LIF-106): Pages now support threaded comments like issues — backend, web UI, and MCP (`add_comment` / `list_comments` accept page identifiers such as `LIF-DOC-3`). (migration 012)
- **Labels on pages** (LIF-105): Pages can carry labels, with full UI and backend parity with issue labels. (migration 013)
- **Fuzzy full-text search for issues** (LIF-119): Search across issue title, identifier, and description via SQLite FTS5.
- **Fuzzy full-text search for pages** (LIF-118): Search across page title, identifier, and content.
- **Page-list search** (LIF-117): Instant client-side filtering in the page list.
- **Module management UI** (LIF-121): List, detail, and sidebar navigation for modules in the web UI.
- **MCP `edit_issue` / `edit_page`** (LIF-113): Surgical find-and-replace editing of issue descriptions and page content, so agents needn't resend the whole field.

### Web UI

- **Explicit Edit/Preview toggle** (LIF-109): Page and issue bodies now use an explicit Edit/Preview control (floating pill + toolbar button + `E` shortcut) instead of click/double-click-to-edit. Reading is fully passive, so selecting and copying text no longer trips into edit mode.
- **Unified topbar** chrome across all main-area routes; **Export** button aligned to the mode-toggle visual family.
- **App version** surfaced under the sidebar brand and on login/signup; brand/logo links to the GitHub repo.
- **Display-options dropdown hidden** (LIF-104): the Group/Density popover was a non-functional "coming soon" placeholder and is hidden until the feature ships (grouping/density tracked in LIF-104).

### Security Fixes

- **Login rate-limiting hardened** (LIF-75): Login was rate-limited only by identity (enabling targeted account-lockout DoS) and double-counted each failure (halving the real budget). Added per-IP limiting alongside per-identity and fixed the double-count, so a failed login costs one slot, not two.
- **OAuth tokens bound to a user** (LIF-79): OAuth access tokens carried no user identity, so OAuth-authenticated clients were effectively anonymous and could not perform user-scoped actions. Authorization codes and tokens are now bound to the approving user. Backward compatible (pre-existing tokens stay anonymous). (migration 014)

### Bug Fixes

- **Project edits blocked without a lead** (LIF-102): Projects with no lead could not be edited; the lead now defaults to the creator. (migration 011)
- **Cannot clear project lead/emoji** (LIF-103): `PATCH /api/projects` could not reset `lead_user_id` or `emoji` back to NULL. Now supported.
- **Issue search crash** (LIF-119): Fixed a Svelte `state_unsafe_mutation` that broke issue search in the web UI.
- **MCP `add_comment` token waste** (LIF-115): The response echoed the full comment body (which the caller already supplied). Now returns just the comment id + author + timestamp.
- **Page tree width**: the page tree now fills the available width.

### MCP

- `edit_issue` / `edit_page` tools added (LIF-113); `add_comment` response trimmed (LIF-115); page identifiers accepted by the comment tools (LIF-106).

### CI / Release

- **Stuck-draft release bug fixed** (LIF-120): Every release v1.2.0–v1.3.1 stranded as a GitHub *draft*. Root cause: `release.yml` created the version tag on the GitHub side, and the magi→GitHub push mirror (`git push --mirror`, prune semantics) deleted that github-only tag on the next sync, which auto-demotes a tagless release to a draft. Fixed by switching to **tag-triggered releases**: the tag now originates on magi, so the mirror replicates it and never prunes it. Added a job that fails fast if the tag and `Cargo.toml` version disagree.

### Upgrade Notes

- **Migrations 011–014** apply automatically on first launch (default project lead, page comments, page labels, OAuth user binding). All additive and savepoint-wrapped; upgrading from any 1.x is safe.
- **Maintainers — release process changed**: releases are now cut by pushing a `vX.Y.Z` tag to magi (`git tag vX.Y.Z && git push origin vX.Y.Z`), not by bumping `Cargo.toml` alone. See AGENTS.md "Releasing a new version."

## v1.3.1 — 2026-05-17

Shipped on crates.io; the GitHub release stranded as a draft (see LIF-120) and was cleaned up during the v1.4.0 prep.

### Bug Fixes

- **Cross-project relation identifiers** (LIF-101): Relations between issues in different projects rendered the wrong identifier. Now preserved correctly.
- Issue list/board view state is preserved across navigation into and back from issue detail.
- Page content switched to double-click-to-edit (later superseded by the explicit Edit/Preview toggle in v1.4.0, LIF-109).

## v1.3.0 — 2026-05-14

Shipped on crates.io; GitHub release stranded (LIF-120). Major web UI release.

### Features

- **Web UI overhaul**: redesigned interface with a kanban **board view** and **drag-and-drop** status changes (svelte-dnd-action).

### Bug Fixes

- **Browser MCP connectivity**: added a top-level CORS layer so browser-based MCP clients can connect.

## v1.2.1 — 2026-05-03

Shipped on crates.io; GitHub release stranded (LIF-120).

### Bug Fixes

- **MCP comment attribution**: `add_comment` attributes to the first admin user for stdio/local (no-auth) sessions.

### CI

- First (incomplete) attempt at fixing the GitHub release workflow — the real fix landed in v1.4.0 (LIF-120). Tightened clippy lint coverage.

## v1.2.0 — 2026-05-02

Shipped on crates.io; GitHub release stranded (LIF-120).

### Features

- **CLI CRUD**: full create/read/update/delete commands for issues, projects, pages, and resources.
- **Markdown export** (LIF-98): export issues, pages, and projects as markdown.
- **MCP pagination** (LIF-21): offset-based pagination for MCP list tools.

### Security Fixes

- **OAuth client registration hardening** (LIF-64): rate limiting + redirect-URI validation on dynamic client registration.

### Bug Fixes

- MCP compatibility fixes for Zed and reverse proxies.

### CI

- Dropped Windows build targets from the release pipeline.

## v1.1.3 — 2026-04-06

Security hardening release closing the remaining vulnerabilities identified in the auth audit.

### Security Fixes

- **CSRF on OAuth authorize form** (LIF-63): The OAuth approval form had no CSRF protection. An attacker could auto-submit the form from a malicious page, tricking a logged-in user into granting a 30-day access token. Added HMAC-SHA256 CSRF tokens with 10-minute expiry.
- **Signup CPU exhaustion** (LIF-67): The signup endpoint had no rate limiting, allowing attackers to burn CPU by spamming Argon2 password hashing. Added rate limiting keyed by email.
- **CORS allows any origin** (LIF-74): CORS was hardcoded to `Any`. Added `server.cors_origins` config option. Falls back to `Any` for development if unset.
- **Session tokens stored plaintext** (LIF-77): Session tokens were stored as-is in the database. A database leak (backup, disk access) exposed all active sessions. Now stored as SHA-256 hashes.
- **OAuth revocation unauthenticated** (LIF-81): Anyone could revoke any OAuth token without authentication. Now requires a valid Bearer token.
- **Username enumeration via timing** (LIF-82): Login for non-existent users returned faster than wrong-password logins (no Argon2 computation). Added dummy Argon2 verification to normalize timing.

### CI

- Unified auto-tag and release into a single workflow to fix cross-workflow token permission issues.

### Upgrade Notes

- **Existing sessions are invalidated**: Sessions created before this version used plaintext storage and will no longer validate against the new SHA-256 lookup. Users will need to log in again.
- New config option: `server.cors_origins` (array of allowed origins). If unset, CORS allows all origins (previous behavior). Set this in production.

## v1.1.2 — 2026-04-06

Security and correctness fixes for auth endpoints, cookies, and server hardening.

### Security Fixes

- **Comment auth bypass** (LIF-72): `add_comment` silently fell back to the first admin user when no auth context was present. Now requires authentication and returns an error.
- **OAuth client_id not required** (LIF-73): Token exchange accepted requests without `client_id`, violating OAuth 2.1 for public clients. Now required.
- **Argon2 CPU DoS via password length** (LIF-84): No max password length was enforced. A multi-MB password would burn CPU in Argon2. Added 1024-character max on both signup and login.
- **Session cookie missing security flags** (LIF-83): Session cookies lacked HttpOnly, Secure, and SameSite attributes. Added `HttpOnly; Secure; SameSite=Lax` to login/signup/logout cookie handling.
- **World-readable backups** (LIF-78): Backup files were created with default permissions (0644). Now set to 0600 (owner-only) on Unix.
- **No request body size limit** (LIF-65): No limit on request body size allowed memory exhaustion via large POSTs. Added 2MB default limit.

## v1.1.1 — 2026-04-06

Stability and data integrity fixes.

### Security Fixes

- **SQL injection via table name** (LIF-76): `get_resource_project_id` interpolated the table name directly into SQL. Added whitelist validation for allowed table names.

### Bug Fixes

- **Mutex poison crash** (LIF-71): Rate limiter panicked on poisoned Mutex, crashing the process. Now recovers with `unwrap_or_else`.
- **OAuth writes silently discarded** (LIF-66): Five DB write operations in OAuth used `let _ =` to silently discard errors. Now propagated with proper error responses.
- **Non-atomic multi-statement writes** (LIF-69): Update operations for issues, projects, modules, labels, and pages ran multiple SQL statements without transactions. A failure mid-way left partial state. Wrapped in SQLite savepoints.
- **Migrations not atomic** (LIF-70): Each migration's SQL and tracking INSERT ran without a transaction. Wrapped in savepoints so partial failures roll back.
- **Rate limiter memory leak** (LIF-68): The rate limiter's HashMap never evicted expired keys, growing without bound. Added periodic sweep when key count exceeds threshold.

### CI

- Fixed auto-tag workflow (missing git identity for annotated tags).
- Fixed crates.io publish (verification build failed without `web/dist/`).

## v1.1.0 — 2026-04-06

Security release closing 6 critical authentication and authorization vulnerabilities.

### Security Fixes

- **Privilege escalation via missing auth check** (LIF-56): `require_admin` and `require_project_lead` returned `Ok(())` when no user was associated with the request (OAuth tokens, legacy API keys). Any unauthenticated-but-authorized request had full admin privileges. Now default-deny.
- **OAuth PKCE bypass** (LIF-58): The `plain` PKCE method was accepted despite OAuth 2.1 requiring S256 only. Sending empty challenge/verifier with `method=plain` fully bypassed PKCE. Removed `plain`; reject empty values.
- **OAuth redirect_uri not validated at token exchange** (LIF-59): The `redirect_uri` from the token request was never compared against the one stored with the authorization code. An attacker who intercepted an auth code could exchange it from any URI. Now validated per OAuth 2.1 Section 4.1.3.
- **OAuth access tokens stored plaintext** (LIF-60): OAuth tokens were stored and looked up by raw value. A database leak exposed all active tokens. Now stored as SHA-256 hashes; raw token returned only once at issuance.
- **MCP identity confusion under concurrency** (LIF-61): A global `Mutex<Option<AuthUser>>` stored the current MCP user. Concurrent requests could overwrite each other's identity, and a panic would poison the mutex permanently. Replaced with serialized request handling via `tokio::sync::Mutex` with poison recovery.
- **Database errors leaked to clients** (LIF-62): Raw SQLite error messages (table names, column names, constraint details, file paths) were returned directly in API responses. Now returns generic "internal server error" and logs details server-side.

### Upgrade Notes

- **OAuth tokens are invalidated**: Existing plaintext OAuth tokens in the database will no longer validate since the lookup now expects SHA-256 hashes. Clients will need to re-authorize. This is intentional — plaintext tokens should not remain valid.
- No database migration required. No config changes.
