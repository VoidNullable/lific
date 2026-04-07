# Changelog

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
