use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, Method, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use rusqlite::params;
use tracing::{info, warn};

use api_keys_simplified::{ApiKeyManagerV0, Environment, ExposeSecret, KeyStatus};

use crate::db::DbPool;
use crate::db::models::AuthUser;

#[derive(Clone)]
pub struct AuthState {
    pub db: DbPool,
    pub manager: ApiKeyManagerV0,
    pub public_url: String,
    /// LIF-294: mirror of `[auth] required`. When false, a request with no
    /// credential at all passes as operator-equivalent; see `require_api_key`.
    pub required: bool,
}

/// Encode bytes as a lowercase hex string.
///
/// LIF-383: the one hex encoder in the tree. Four copies of this loop used to
/// live in oauth.rs, mcp/mod.rs, auth.rs and db/queries/users.rs.
pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 a byte slice and return the lowercase hex digest. This is how every
/// bearer credential (session tokens, OAuth access tokens, device codes) is
/// stored and looked up.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex_encode(&Sha256::digest(bytes))
}

/// Create the API key manager with our prefix.
pub fn create_key_manager() -> Result<ApiKeyManagerV0, String> {
    ApiKeyManagerV0::init_default_config("lific_sk")
        .map_err(|e| format!("failed to init key manager: {e}"))
}

/// Generate a new API key, store the hash, return the plaintext (shown once).
///
/// `user_id` binds the key to a user in the same insert (LIF-391). `None`
/// leaves it unbound, which resolves as the operator identity, so pass a user
/// whenever the key belongs to one.
pub fn create_api_key(
    db: &DbPool,
    manager: &ApiKeyManagerV0,
    name: &str,
    user_id: Option<i64>,
) -> Result<String, crate::error::LificError> {
    create_api_key_with_expiry(db, manager, name, None, user_id)
}

/// Like [`create_api_key`] but writes an optional `expires_at` (ISO 8601). Once
/// past, the auth path (LIF-131) refuses the key. `None` means never expires.
pub fn create_api_key_with_expiry(
    db: &DbPool,
    manager: &ApiKeyManagerV0,
    name: &str,
    expires_at: Option<&str>,
    user_id: Option<i64>,
) -> Result<String, crate::error::LificError> {
    let prepared = PreparedApiKey::generate(manager)?;
    // One immediate transaction, not a bare `write()`. `insert` reads the
    // name's existing rows, deletes a matching tombstone and inserts: three
    // statements that must be one atomic unit, and that must serialize against
    // an account lockdown running in *another process* (the CLI resetting a
    // password while the server is up). A bare writer guard only serializes
    // within this process.
    db.transaction(|tx| prepared.insert(tx, name, expires_at, user_id))
}

/// Key material generated but not yet written.
///
/// Splitting generation from the insert lets a caller do the expensive,
/// side-effect-free half (CSPRNG draw plus the manager's hashing) *outside* the
/// writer, then take the writer once and revalidate its authorization in the
/// same transaction that stores the key. Nothing here is persisted until
/// [`PreparedApiKey::insert`] runs, so an abandoned preparation leaves no trace
/// and no live credential.
///
/// The plaintext lives in this struct and is handed to the caller exactly once,
/// by `insert`. It is never written to the database, which stores only the
/// hash and the derived lookup id.
pub struct PreparedApiKey {
    plaintext: String,
    hash: String,
    key_id: String,
}

impl PreparedApiKey {
    /// Draw fresh key material. Touches no connection.
    pub fn generate(manager: &ApiKeyManagerV0) -> Result<Self, crate::error::LificError> {
        let api_key = manager.generate(Environment::production()).map_err(|e| {
            crate::error::LificError::Internal(format!("key generation failed: {e}"))
        })?;
        Ok(Self {
            plaintext: api_key.key().expose_secret().to_string(),
            hash: api_key.expose_hash().hash().to_string(),
            key_id: api_key.expose_hash().key_id().to_string(),
        })
    }

    /// Claim the name and store the key on `conn`, returning the plaintext.
    ///
    /// Takes a borrowed connection rather than the pool so the caller controls
    /// the transaction: claiming the name and inserting are several statements
    /// and must not be separable, and a caller that revalidated its
    /// authorization moments earlier needs that revalidation to be inside the
    /// same transaction as this write.
    ///
    /// `api_keys.name` is globally `UNIQUE`, not unique-per-active-key, so a
    /// revoked row keeps its name reserved forever. After an account lockdown
    /// revoked `opencode-blake`, reconnecting that tool hit the UNIQUE
    /// constraint and failed with an opaque database error, and the account
    /// could not get its tools back without shell access. Names are therefore
    /// reusable, but only on terms narrow enough that reuse is never a way to
    /// reach somebody else's row:
    ///
    /// - any **active** row with this exact name is refused, the rule every
    ///   caller has always had;
    /// - a **revoked** row with this exact name is reusable only when its
    ///   ownership matches the key being created: same `user_id`, or both
    ///   `NULL` for the unbound operator key. `NULL` and `Some(id)` are
    ///   different owners, in both directions.
    /// - anything else is refused without touching the row.
    ///
    /// The owner check matters because the name is the whole handle here. Bot
    /// usernames are derived (`{tool}-{owner}`), so `opencode-blake`'s
    /// tombstone is exactly what a second account would have to displace to
    /// take that name, and deleting another owner's row is a write on their
    /// account's history that the caller has no claim to. Refusing is also
    /// what stops name reuse becoming an oracle: the caller learns the name is
    /// taken, not by whom.
    ///
    /// The delete is exact-name (never a `LIKE` or prefix), revoked-only, and
    /// owner-matched, so it cannot touch a live credential or another
    /// account's. It carries no key material anywhere: the row is dropped, not
    /// copied. The audit rows that recorded the revocation survive it,
    /// deliberately, and nothing references `api_keys` by foreign key, so there
    /// is nothing else to cascade.
    ///
    /// LIF-391: the user binding is part of the insert, not a follow-up
    /// update. A key is never briefly on disk unbound, so a crash mid-creation
    /// cannot leave behind an orphan key that resolves as the operator.
    pub fn insert(
        self,
        conn: &rusqlite::Connection,
        name: &str,
        expires_at: Option<&str>,
        user_id: Option<i64>,
    ) -> Result<String, crate::error::LificError> {
        let mut stmt = conn.prepare("SELECT user_id, revoked FROM api_keys WHERE name = ?1")?;
        let existing = stmt
            .query_map(params![name], |row| {
                Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, bool>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);

        if existing.iter().any(|(_, revoked)| !revoked) {
            return Err(crate::error::LificError::BadRequest(format!(
                "an active key named '{name}' already exists"
            )));
        }
        if existing.iter().any(|(owner, _)| *owner != user_id) {
            return Err(crate::error::LificError::BadRequest(format!(
                "the key name '{name}' is already reserved by another owner"
            )));
        }

        if !existing.is_empty() {
            conn.execute(
                "DELETE FROM api_keys WHERE name = ?1 AND revoked = 1",
                params![name],
            )?;
        }

        conn.execute(
            "INSERT INTO api_keys (name, key_hash, key_id, expires_at, user_id) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![name, self.hash, self.key_id, expires_at, user_id],
        )?;

        Ok(self.plaintext)
    }
}

/// The error every failure of [`recent_session_token`] /
/// [`revalidate_recent_session`] returns. One string on purpose: which of
/// "no token", "not a session token", "expired", "too old" or "wrong user"
/// applies is not something an unauthorized caller should get to probe for.
fn recent_auth_required() -> crate::error::LificError {
    crate::error::LificError::Forbidden("recent authentication required".into())
}

/// Pull the browser session token out of an `Authorization: Bearer` header for
/// a route that mints durable credentials.
///
/// Minting an API key or connecting a tool is an account-level action, so it
/// requires the thing a human just typed a password into: a browser session.
/// An API key, an OAuth access token, a cookie, or no credential at all is
/// refused here regardless of what the authentication middleware made of it.
/// This is a *shape* check only; validity, freshness and ownership are
/// re-established by [`revalidate_recent_session`] inside the write
/// transaction, because anything checked before that transaction opens can be
/// revoked out from under the write.
pub fn recent_session_token(headers: &HeaderMap) -> Result<String, crate::error::LificError> {
    session_bearer_token(headers).map_err(|_| recent_auth_required())
}

/// The browser session token from an `Authorization: Bearer` header, or a
/// `Forbidden` naming what is missing.
///
/// Shape only: this says the caller presented something session-shaped, not
/// that it is live or whose it is. Split out from [`recent_session_token`] so
/// `POST /api/auth/me/refresh` can require a session without also requiring it
/// to be recent, which is the one thing it exists to fix.
pub fn session_bearer_token(headers: &HeaderMap) -> Result<String, crate::error::LificError> {
    let token = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .ok_or_else(|| {
            crate::error::LificError::Forbidden(
                "this action requires a signed-in browser session".into(),
            )
        })?;

    if !token.starts_with("lific_sess_") {
        return Err(crate::error::LificError::Forbidden(
            "this action requires a signed-in browser session".into(),
        ));
    }
    Ok(token.to_string())
}

/// Re-establish, on `conn`, that `token` is a live browser session belonging to
/// `expected_user_id` and created inside the recent-authentication window, and
/// hand back **the user as the database has them right now**.
///
/// Callers run this as the first statement of the writer transaction that
/// grants something. SQLite serializes writers, so an account lockdown is
/// either wholly before this check (which then fails, because the session row
/// is gone) or wholly after the write (which then revokes what was granted).
/// There is no interleaving that leaves a live credential behind a revoked
/// session.
///
/// `expected_user_id` is the identity the authentication middleware resolved.
/// Comparing it to the session's own user closes the gap where a token is
/// swapped between the middleware's read and this write.
///
/// **Use the returned user, not the middleware's copy, for any authorization
/// decision this transaction makes.** The `AuthUser` the middleware attached
/// was read before the request was routed; `is_admin` on it is a snapshot, and
/// a demotion committed since is invisible to it. The value returned here was
/// read inside this transaction and is the only trustworthy one. Callers that
/// need nothing but "the session belongs to this person" may ignore it.
pub fn revalidate_recent_session(
    conn: &rusqlite::Connection,
    token: &str,
    expected_user_id: i64,
) -> Result<crate::db::models::User, crate::error::LificError> {
    let user = crate::db::queries::users::validate_session(conn, token)
        .map_err(|_| recent_auth_required())?;
    if user.id != expected_user_id {
        return Err(recent_auth_required());
    }
    if !crate::db::queries::users::session_is_recent(conn, token)? {
        return Err(recent_auth_required());
    }
    Ok(user)
}

/// The freshly-read user as an `AuthUser`, for re-running an authorization gate
/// inside the transaction that is about to write.
pub fn fresh_auth_user(user: &crate::db::models::User) -> AuthUser {
    AuthUser {
        id: user.id,
        username: user.username.clone(),
        display_name: user.display_name.clone(),
        is_admin: user.is_admin,
    }
}

/// The freshly-read user as a `ResolvedIdentity` for `authz::require_role_conn`.
///
/// A session caller is always a human, so there is no bot-to-owner resolution
/// to redo: the session's own user *is* the effective user.
pub fn fresh_identity(
    user: &crate::db::models::User,
    transport: crate::actor::Transport,
) -> crate::resolve_caller::ResolvedIdentity {
    crate::resolve_caller::ResolvedIdentity {
        user: fresh_auth_user(user),
        transport,
    }
}

/// Re-read the calling user inside a transaction, and refuse if the account
/// can no longer authenticate.
///
/// The destructive routes (revoke a key, disconnect or delete a tool, demote,
/// deactivate) deliberately do NOT require a recent sign-in: they only ever
/// take access away, and they are what an admin reaches for mid-incident. They
/// do still need their *authorization* to be current, because
/// `AuthUser.is_admin` came off a snapshot taken before the request was
/// routed, and "can this caller act on somebody else's credential" is decided
/// by that flag.
pub fn fresh_caller(
    conn: &rusqlite::Connection,
    caller_id: i64,
) -> Result<crate::db::models::User, crate::error::LificError> {
    let user = crate::db::queries::users::get_user_by_id(conn, caller_id)
        .map_err(|_| crate::error::LificError::Forbidden("authentication required".into()))?;
    if !crate::db::queries::users::credential_is_live(conn, &user)? {
        return Err(crate::error::LificError::Forbidden(
            "authentication required".into(),
        ));
    }
    Ok(user)
}

/// Inside a granting transaction, require that the freshly-read session user
/// still holds instance admin.
///
/// The handler's `require_admin` preflight runs on the middleware's snapshot so
/// an unauthorized caller gets a clean 403 without touching the writer. This is
/// the authoritative one.
pub fn require_fresh_admin(user: &crate::db::models::User) -> Result<(), crate::error::LificError> {
    if user.is_admin {
        Ok(())
    } else {
        Err(crate::error::LificError::Forbidden(
            "only an admin can do this".into(),
        ))
    }
}

/// List all API keys (never returns the key itself, just metadata).
pub fn list_api_keys(db: &DbPool) -> Result<Vec<ApiKeyInfo>, crate::error::LificError> {
    let conn = db.read()?;
    let mut stmt = conn.prepare(
        "SELECT id, name, created_at, expires_at, revoked FROM api_keys ORDER BY created_at",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ApiKeyInfo {
            id: row.get(0)?,
            name: row.get(1)?,
            created_at: row.get(2)?,
            expires_at: row.get(3)?,
            revoked: row.get(4)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(crate::error::LificError::Database)
}

/// Revoke a key by name.
pub fn revoke_api_key(db: &DbPool, name: &str) -> Result<(), crate::error::LificError> {
    let conn = db.write()?;
    let changed = conn.execute(
        "UPDATE api_keys SET revoked = 1 WHERE name = ?1 AND revoked = 0",
        params![name],
    )?;
    if changed == 0 {
        return Err(crate::error::LificError::NotFound(format!(
            "no active key named '{name}'"
        )));
    }
    info!(name, "API key revoked");
    Ok(())
}

/// Rotate a key: delete the old one, create a new one, return the new plaintext.
/// The old key's user binding carries over to the new key (LIF-132) — rotating
/// a bot/user key must not silently de-attribute it.
pub fn rotate_api_key(
    db: &DbPool,
    manager: &ApiKeyManagerV0,
    name: &str,
) -> Result<String, crate::error::LificError> {
    rotate_api_key_bound(db, manager, name, None)
}

/// Like [`rotate_api_key`], but binds the new key to `user_id` rather than
/// carrying the old binding over. `None` keeps the old binding. Used where
/// the caller already knows the owner, e.g. `lific connect` re-minting a
/// bot's key.
pub fn rotate_api_key_bound(
    db: &DbPool,
    manager: &ApiKeyManagerV0,
    name: &str,
    user_id: Option<i64>,
) -> Result<String, crate::error::LificError> {
    // Draw the material first, outside any lock, then do the lookup, the
    // delete and the insert in ONE transaction. This used to be a `write()`
    // guard that read and deleted, then dropped the guard and called
    // `create_api_key`, which took the writer again: two commits with a gap in
    // between where the key simply did not exist. A crash or a competing write
    // landing in that gap left the tool with no credential at all and no way
    // to tell that from a rotation that had not started.
    let prepared = PreparedApiKey::generate(manager)?;
    db.transaction(|tx| {
        // Capture the user binding before deleting so it can be re-applied.
        // If multiple rows share the name (revoked leftovers), prefer the
        // binding of an active row.
        let previous_user_id: Option<i64> = tx
            .query_row(
                "SELECT user_id FROM api_keys WHERE name = ?1 ORDER BY revoked ASC, id DESC LIMIT 1",
                params![name],
                |row| row.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    crate::error::LificError::NotFound(format!("no key named '{name}'"))
                }
                other => other.into(),
            })?;

        // Delete old key entirely (not just revoke) so the name can be reused.
        // This clears every row for the name, so `insert`'s active-name and
        // owner checks see a free name: rotation is explicitly allowed to
        // replace a live key, which is the whole point of it.
        tx.execute("DELETE FROM api_keys WHERE name = ?1", params![name])?;

        prepared.insert(tx, name, None, user_id.or(previous_user_id))
    })
}

/// Check if any API keys exist.
pub fn has_any_keys(db: &DbPool) -> bool {
    if let Ok(conn) = db.read() {
        conn.query_row("SELECT COUNT(*) FROM api_keys", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap_or(0)
            > 0
    } else {
        false
    }
}

/// Whether a first human (non-bot) operator exists yet.
pub fn has_human_operator(db: &DbPool) -> bool {
    if let Ok(conn) = db.read() {
        crate::db::queries::users::has_human_users(&conn).unwrap_or(false)
    } else {
        false
    }
}

/// LIFIC-9: whether to auto-mint the "default" unbound API key at startup.
///
/// This is the single decision both `lific init` and `lific start` share. Under
/// the new design a human operator is created at `init` (passwordless mode), so
/// an unbound operator-style key should no longer be minted as the default path
/// — the operator *is* a real user now. The "default" key is still available on
/// demand via `lific key create`. We mint it only for the genuinely empty
/// bootstrap (no users at all, no keys) so a headless/agent-first install can
/// still get a credential before any human exists — and once a human exists we
/// never auto-mint it again, even if all keys were later revoked.
pub fn should_mint_initial_key(db: &DbPool) -> bool {
    !has_any_keys(db) && !has_human_operator(db)
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct ApiKeyInfo {
    pub id: i64,
    pub name: String,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub revoked: bool,
}

/// LIF-267: parse the `lific_token` session cookie a browser sends on same-site
/// GETs. Splits the `Cookie` header on `;`, trims each pair, and returns the
/// `lific_token` value ONLY when it looks like a session token (`lific_sess_`
/// prefix). API keys (`lific_sk`) and OAuth tokens (`lific_at_`) are never
/// accepted via cookie — the cookie path authenticates the browser session and
/// nothing else. Returns `None` when the header/cookie is absent or the value
/// isn't a session token.
fn session_cookie_token(headers: &HeaderMap) -> Option<String> {
    let cookies = headers.get("cookie").and_then(|v| v.to_str().ok())?;
    let value = cookies.split(';').find_map(|c| {
        c.trim()
            .strip_prefix("lific_token=")
            .map(|v| v.trim().to_string())
    })?;
    value.starts_with("lific_sess_").then_some(value)
}

/// LIF-267: is this request a `GET /api/attachments/{id}` download, where `{id}`
/// is a single numeric segment (trailing slash tolerated)? Only this exact
/// shape is eligible for the session-cookie fallback: it's the browser-native
/// `<img src>` subresource path. The list route `/api/attachments` (no id) and
/// any deeper path like `/api/attachments/5/extra` are excluded, so the
/// fallback never widens beyond a single read-only download.
///
/// LIF-418 adds one sibling, `/api/attachments/{id}/thumbnail`. It is the same
/// read of the same blob, downscaled, and it is consumed the same way: an
/// `<img src>` cannot set an Authorization header, so without the fallback
/// every thumbnail in the UI would 401. Nothing else under `{id}/` is
/// eligible, so `/links` and `/preview` still require a real credential.
fn is_attachment_download(method: &Method, path: &str) -> bool {
    if method != Method::GET {
        return false;
    }
    let Some(rest) = path.strip_prefix("/api/attachments/") else {
        return false;
    };
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let id = rest.strip_suffix("/thumbnail").unwrap_or(rest);
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())
}

/// LIF-403: which kind of credential authenticated the current request.
///
/// Inserted into the request extensions at every success branch of
/// [`require_api_key`], next to `Option<AuthUser>` and the resolved identity.
/// Downstream code that needs to treat a credential *kind* differently — the
/// OAuth deny-list below is the first such case — reads this instead of
/// re-sniffing the `Authorization` header or the request path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    /// Browser session token (`lific_sess_`), from the header or the
    /// `lific_token` cookie.
    Session,
    /// API key (`lific_sk`), including the unbound operator key.
    ApiKey,
    /// OAuth 2.1 access token (`lific_at_`) issued by the connector flow.
    Oauth,
    /// No credential at all — the `[auth] required = false` path (LIF-294).
    Anonymous,
}

/// LIF-403: the REST surfaces an OAuth access token may not reach.
///
/// An OAuth token is the MCP connector credential: it expires, it is
/// revocable from Connected Tools, and it is bound to a per-tool bot. Those
/// properties are the entire point of it, and they evaporate the moment the
/// token can mint a credential that outlives it. Until this gate existed, a
/// bot token could `POST /api/auth/keys` (`api::auth::create_key`) and walk
/// away with a permanent, never-expiring API key — a leash traded for a
/// forever key.
///
/// OAuth tokens remain accepted on ordinary REST routes on purpose: that is
/// parity with what the same token can already do through the MCP tools.
/// Only credential-management and account/user administration is closed off.
/// Returns the reason a route is on the list, or `None` when the request is
/// allowed. Each rule is a path prefix (so `{id}` sub-routes are covered
/// without re-implementing route matching) plus the methods it applies to.
fn oauth_blocked_reason(method: &Method, path: &str) -> Option<&'static str> {
    // Normalize one trailing slash so `/api/auth/keys/` can't slip past a
    // comparison the router itself would still resolve.
    let path = path.strip_suffix('/').unwrap_or(path);

    // API keys — create / list / revoke. THE escalation this issue is about:
    // a credential that expires must not mint one that never does. Listing is
    // blocked too; key inventory is credential management, not app data.
    if path == "/api/auth/keys" || path.starts_with("/api/auth/keys/") {
        return Some("API key management is not available to OAuth-token callers");
    }

    // Connected tools (bots) — creating one mints an API key for it, and
    // disconnect/delete revoke *other* agents' credentials. Same class of
    // authority as the key routes above.
    if path == "/api/auth/bots" || path.starts_with("/api/auth/bots/") {
        return Some("connected-tool management is not available to OAuth-token callers");
    }

    // Password change and "sign every session out". A tool token acts for a
    // bot; rewriting the human's login credential, or logging them out
    // everywhere, is not within its remit.
    // `/me/refresh` joins them: it mints a browser session, which is the one
    // credential shape allowed to perform the granting actions an OAuth token
    // is kept out of. The handler refuses non-session bearers on its own; this
    // is the same rule stated where the rest of the credential surface is.
    if path == "/api/auth/me/password"
        || path == "/api/auth/me/sessions"
        || path == "/api/auth/me/refresh"
    {
        return Some("account credential changes are not available to OAuth-token callers");
    }

    // Profile mutation on the caller's own account. `GET /api/auth/me` stays
    // open — reading who you are is not an account change.
    if path == "/api/auth/me" && method != Method::GET {
        return Some("account changes are not available to OAuth-token callers");
    }

    // User administration — create an account, grant/revoke instance admin,
    // deactivate/reactivate. `GET /api/users` stays open: the roster read is
    // a plain read that drives assignee pick-lists.
    if (path == "/api/users" && method != Method::GET) || path.starts_with("/api/users/") {
        return Some("user administration is not available to OAuth-token callers");
    }

    // OAuth client and token administration. These live in a router merged
    // OUTSIDE this middleware today (`oauth::router` in src/server.rs), so
    // nothing reaches here; the rule is here so moving them behind auth later
    // cannot hand a token authority over the flow that issued it.
    if path == "/oauth" || path.starts_with("/oauth/") {
        return Some("OAuth client and token administration is not available to OAuth-token callers");
    }

    None
}

/// LIFIC-8: resolve the caller's identity and stamp it into the request
/// extension *alongside* the legacy `Option<AuthUser>`. Best-effort by design
/// during the expand step — a resolve failure (DB fault, read-lock poisoning)
/// is logged and degrades to `None` so auth itself never breaks. LIFIC-10
/// will make the downstream gates read this; until then nothing consumes it.
///
/// LIF-403: also inserts the [`CredentialKind`] marker, so every success
/// branch of `require_api_key` stamps the credential kind through this one
/// call and a new branch cannot forget it.
///
/// `default` is the transport the credential-type implies when the request
/// is NOT aimed at `/mcp` (session → web, key/oauth → api).
fn insert_identity_extensions(
    request: &mut Request<Body>,
    db: &DbPool,
    credential_user: Option<crate::db::models::AuthUser>,
    is_mcp_request: bool,
    default: crate::actor::Transport,
    kind: CredentialKind,
) {
    let transport = if is_mcp_request {
        crate::actor::Transport::Mcp
    } else {
        default
    };
    let resolved = match crate::resolve_caller::resolve_caller(db, credential_user, transport) {
        Ok(id) => id,
        Err(e) => {
            warn!(error = %e, "resolved-identity lookup failed; degrading to None");
            None
        }
    };
    request.extensions_mut().insert(resolved);
    request.extensions_mut().insert(kind);
}

/// Axum middleware that validates Bearer tokens and resolves user identity.
///
/// After successful auth, inserts `Extension<Option<AuthUser>>` into the request:
/// - `Some(user)` if the token resolves to a user (session, or API key with user_id)
/// - `None` if the token is valid but has no user association (legacy keys, OAuth)
///
/// It also inserts `Extension<CredentialKind>` (LIF-403) so downstream code
/// can tell *how* the caller authenticated, and refuses an OAuth token that
/// is aimed at a credential-management or account-administration route (see
/// [`oauth_blocked_reason`]) with a 403.
pub async fn require_api_key(
    State(auth): State<AuthState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    // Extract Bearer token from Authorization header
    let token = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.trim().to_string());

    // Targeted diagnostics for the MCP endpoint only (keeps REST traffic quiet).
    // Lets us see, post-OAuth, whether Claude actually presents the bearer token
    // it was issued — distinguishing a server-side token rejection from the
    // documented claude.ai-web bug where the token is dropped and the
    // authenticated /mcp request is never sent.
    let is_mcp_request = request.uri().path() == "/mcp";
    if is_mcp_request {
        let token_kind = match token.as_deref() {
            Some(t) if t.starts_with("lific_sess_") => "session",
            Some(t) if t.starts_with("lific_at_") => "oauth",
            Some(t) if t.starts_with("lific_sk") => "api_key",
            Some(_) => "unknown",
            None => "none",
        };
        info!(method = %request.method(), token_kind, "/mcp request received");
    }

    // RFC 9728 §3.1: for a resource URL with a path component (`/mcp`), the
    // canonical protected-resource metadata lives at the path-aware well-known
    // location. Point Claude there so the `resource` it reads matches the URL
    // the user entered.
    let www_auth = format!(
        "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource/mcp\"",
        auth.public_url
    );

    let Some(token) = token else {
        // LIF-267: session-cookie fallback, scoped to GET /api/attachments/{id}.
        // A browser-native `<img src="/api/attachments/N">` cannot attach an
        // Authorization header, so inline attachment images arrived here
        // credential-less and 401'd. When (and only when) this is the
        // read-only attachment download route on a GET, accept the browser's
        // `lific_token` session cookie in lieu of the header and resolve it
        // exactly like a header-borne session token. This reopens NO CSRF
        // surface (GET is a safe method; every mutation stays header-only) and
        // leaks nothing cross-site (the cookie is SameSite=Lax, so it is never
        // sent on cross-site subresource requests). The download handler still
        // runs its own project-scoped `authorize_read`, so gating is unchanged
        // — this just lets the browser present the credential it can.
        if is_attachment_download(request.method(), request.uri().path())
            && let Some(cookie_token) = session_cookie_token(request.headers())
        {
            let user = {
                // LIF-139: validation is a pure read — take a pooled read
                // connection instead of the single writer mutex.
                let conn = match auth.db.read() {
                    Ok(c) => c,
                    Err(_) => {
                        return (StatusCode::INTERNAL_SERVER_ERROR, "database error")
                            .into_response();
                    }
                };
                crate::db::queries::users::validate_session(&conn, &cookie_token)
            };
            if let Ok(u) = user {
                let auth_user = crate::db::models::AuthUser {
                    id: u.id,
                    username: u.username,
                    display_name: u.display_name,
                    is_admin: u.is_admin,
                };
                let actor = crate::actor::ActorCtx {
                    user_id: Some(auth_user.id),
                    transport: crate::actor::Transport::Web,
                };
                insert_identity_extensions(
                    &mut request,
                    &auth.db,
                    Some(auth_user.clone()),
                    false,
                    crate::actor::Transport::Web,
                    CredentialKind::Session,
                );
                request.extensions_mut().insert(Some(auth_user));
                return crate::actor::scope(actor, next.run(request)).await;
            }
            // Missing/invalid/expired cookie session falls through to 401 below.
        }

        // LIF-294: `[auth] required = false` — a credential-less request is
        // the operator (same trust rail as an unbound API key, LIF-261).
        // ONLY this no-credential path is affected: a presented-but-invalid
        // token still falls through to the 401s below, so a broken client
        // config surfaces as an error instead of silently degrading to
        // anonymous-with-admin-powers.
        if !auth.required {
            let default = if is_mcp_request {
                crate::actor::Transport::Mcp
            } else {
                crate::actor::Transport::Api
            };
            let actor = crate::actor::ActorCtx {
                user_id: None,
                transport: default,
            };
            // LIFIC-8: resolve the passwordless identity (first-admin
            // fallback). resolve_caller handles the operator bypass — no
            // separate carrier needed (LIFIC-14 deleted the last of them).
            insert_identity_extensions(
                &mut request,
                &auth.db,
                None,
                is_mcp_request,
                default,
                CredentialKind::Anonymous,
            );
            request
                .extensions_mut()
                .insert(Option::<crate::db::models::AuthUser>::None);
            return crate::actor::scope(actor, next.run(request)).await;
        }

        if is_mcp_request {
            info!("/mcp rejected: no Authorization header (discovery probe or dropped token)");
        }
        return (
            StatusCode::UNAUTHORIZED,
            [("WWW-Authenticate", www_auth.as_str())],
            "Missing Authorization: Bearer <key> header",
        )
            .into_response();
    };

    // ── Session tokens (lific_sess_ prefix) ──────────────────────
    if token.starts_with("lific_sess_") {
        let user = {
            // LIF-139: session validation no longer writes, so every
            // session-authenticated request reads from the pool instead of
            // serializing on the exclusive writer.
            let conn = match auth.db.read() {
                Ok(c) => c,
                Err(_) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
                }
            };
            crate::db::queries::users::validate_session(&conn, &token)
        };

        match user {
            Ok(u) => {
                let auth_user = crate::db::models::AuthUser {
                    id: u.id,
                    username: u.username,
                    display_name: u.display_name,
                    is_admin: u.is_admin,
                };
                // LIF-155: session tokens are the browser — audit as 'web'
                // (or 'mcp' if a session token is ever pointed at /mcp).
                let actor = crate::actor::ActorCtx {
                    user_id: Some(auth_user.id),
                    transport: if is_mcp_request {
                        crate::actor::Transport::Mcp
                    } else {
                        crate::actor::Transport::Web
                    },
                };
                insert_identity_extensions(
                    &mut request,
                    &auth.db,
                    Some(auth_user.clone()),
                    is_mcp_request,
                    crate::actor::Transport::Web,
                    CredentialKind::Session,
                );
                request.extensions_mut().insert(Some(auth_user));
                return crate::actor::scope(actor, next.run(request)).await;
            }
            Err(_) => {
                return (
                    StatusCode::UNAUTHORIZED,
                    [("WWW-Authenticate", www_auth.as_str())],
                    "Invalid or expired session",
                )
                    .into_response();
            }
        }
    }

    // ── OAuth tokens (lific_at_ prefix) ──────────────────────────
    if token.starts_with("lific_at_") {
        // One resolution, one database snapshot (`resolve_oauth_credential`).
        // This used to be three calls on three pooled connections: is it
        // valid, whose is it, is that user live. A token revoked between the
        // first two answered "valid" and then "unbound", and an unbound OAuth
        // token takes the operator fallback, so revoking a tool's credential
        // could promote it. The typed outcome makes that state unrepresentable.
        match crate::oauth::resolve_oauth_credential(&auth.db, &token) {
            Ok(credential) => {
                if is_mcp_request {
                    info!("/mcp authorized: OAuth token accepted");
                }
                // A bound token carries its user. `LegacyUnbound` is a
                // pre-LIF-79 row with genuinely no binding, and keeps the
                // documented operator fallback (`None`); nothing issued since
                // can be in that state.
                let auth_user = match credential {
                    crate::oauth::OAuthCredential::Bound(user) => Some(user),
                    crate::oauth::OAuthCredential::LegacyUnbound => None,
                };
                // LIF-403: the credential authenticates, but it may not be
                // pointed at credential management or account administration.
                // Enforced here, in the middleware, rather than in the handlers:
                // one list next to the credential check beats a gate per route
                // that a new route can be added without.
                if let Some(reason) = oauth_blocked_reason(request.method(), request.uri().path()) {
                    warn!(
                        method = %request.method(),
                        path = %request.uri().path(),
                        "OAuth token refused on a credential-management route"
                    );
                    return (
                        StatusCode::FORBIDDEN,
                        format!(
                            "{reason}. Authenticate with a session or API key for this endpoint."
                        ),
                    )
                        .into_response();
                }
                // LIF-155: OAuth tokens are programmatic access — 'mcp' when
                // aimed at /mcp (the normal case), 'api' against REST.
                let actor = crate::actor::ActorCtx {
                    user_id: auth_user.as_ref().map(|u| u.id),
                    transport: if is_mcp_request {
                        crate::actor::Transport::Mcp
                    } else {
                        crate::actor::Transport::Api
                    },
                };
                insert_identity_extensions(
                    &mut request,
                    &auth.db,
                    auth_user.clone(),
                    is_mcp_request,
                    crate::actor::Transport::Api,
                    CredentialKind::Oauth,
                );
                request.extensions_mut().insert(auth_user);
                return crate::actor::scope(actor, next.run(request)).await;
            }
            Err(crate::oauth::OAuthReject::DeadBinding) => {
                // Bound to an identity that may not authenticate: the user is
                // gone, deactivated, or is a bot whose owner is deactivated. A
                // bot's authority is its owner's, so an owner switched off must
                // not keep acting through the tool token they approved
                // earlier. Rejected outright, never degraded to unbound, which
                // would hand it the operator fallback (PR #23 review).
                if is_mcp_request {
                    warn!("/mcp rejected: OAuth token bound to a missing or deactivated user");
                }
                return (
                    StatusCode::UNAUTHORIZED,
                    [("WWW-Authenticate", www_auth.as_str())],
                    "OAuth token bound to a missing or deactivated user",
                )
                    .into_response();
            }
            Err(crate::oauth::OAuthReject::Unavailable) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
            }
            Err(crate::oauth::OAuthReject::Invalid) => {
                if is_mcp_request {
                    warn!("/mcp rejected: OAuth token invalid or expired");
                }
                return (
                    StatusCode::UNAUTHORIZED,
                    [("WWW-Authenticate", www_auth.as_str())],
                    "Invalid or expired OAuth token",
                )
                    .into_response();
            }
        }
    }

    // ── API keys (lific_sk- prefix) ──────────────────────────────
    // LIFIC-18: shared with the stdio LIFIC_TOKEN resolver so the
    // checksum/lookup/backfill/hash logic lives in one place (see
    // `validate_api_key` just below `ApiKeyRow`).
    let auth_user = match validate_api_key(&auth.db, &auth.manager, &token) {
        Ok(user) => user,
        Err(ApiKeyReject::BadChecksum) => {
            warn!("rejected API key with invalid checksum");
            return (StatusCode::UNAUTHORIZED,
                    [("WWW-Authenticate", www_auth.as_str())], "Invalid API key").into_response();
        }
        Err(ApiKeyReject::Db) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
        Err(ApiKeyReject::NotFound) => {
            warn!("rejected invalid API key");
            return (StatusCode::UNAUTHORIZED,
                    [("WWW-Authenticate", www_auth.as_str())], "Invalid API key").into_response();
        }
        Err(ApiKeyReject::HashMismatch) => {
            warn!("API key hash verification failed");
            return (StatusCode::UNAUTHORIZED,
                    [("WWW-Authenticate", www_auth.as_str())], "Invalid API key").into_response();
        }
        Err(ApiKeyReject::Inactive) => {
            warn!("rejected API key: deactivated account, or a bot with a deactivated owner");
            return (
                StatusCode::UNAUTHORIZED,
                [("WWW-Authenticate", www_auth.as_str())],
                "This account is deactivated",
            )
                .into_response();
        }
    };

    // LIF-155: API keys are programmatic — 'mcp' on the /mcp path, 'api' for
    // direct REST usage. The LIF-261 operator bypass for an unbound key
    // (user_id = None) now lives entirely in `resolve_caller` (inserted above),
    // which falls back to the first admin — read as `identity.user.is_admin` by
    // the gates. The old credential-type-specific `OperatorCredential` marker
    // and `operator_scope` task-local are gone (LIFIC-14).
    let actor = crate::actor::ActorCtx {
        user_id: auth_user.as_ref().map(|u| u.id),
        transport: if is_mcp_request {
            crate::actor::Transport::Mcp
        } else {
            crate::actor::Transport::Api
        },
    };
    insert_identity_extensions(
        &mut request,
        &auth.db,
        auth_user.clone(),
        is_mcp_request,
        crate::actor::Transport::Api,
        CredentialKind::ApiKey,
    );
    request.extensions_mut().insert(auth_user);
    crate::actor::scope(actor, next.run(request)).await
}

/// Internal struct for loading API key rows during auth.
#[derive(Debug)]
struct ApiKeyRow {
    id: i64,
    hash: String,
    user_id: Option<i64>,
}

/// Why an API key failed to authenticate. Maps to both the HTTP response and
/// the stdio-resolver error, so both callers share one decision.
#[derive(Debug, Clone, Copy)]
enum ApiKeyReject {
    /// A database read/write failed (backend fault, not a bad key).
    Db,
    /// The key didn't pass the format checksum.
    BadChecksum,
    /// The key is well-formed but matches no active key.
    NotFound,
    /// A matching key exists but the stored hash didn't verify.
    HashMismatch,
    /// The key verified, but the identity it names may not authenticate: the
    /// account is deactivated, or it is a bot whose owner is deactivated
    /// (LIF-214 follow-up, see `queries::users::credential_is_live`).
    Inactive,
}

/// Shared API-key authentication for both the HTTP middleware and the stdio
/// `LIFIC_TOKEN` resolver. Verifies the checksum, resolves the key row by
/// derived key_id (with the pre-migration-010 scan-and-backfill fallback), and
/// verifies the stored hash — exactly one copy of that logic (LIFIC-18 review:
/// previously duplicated between `require_api_key` and `resolve_api_key_user`).
///
/// Returns `Ok(Some(user))` for a valid bound key, `Ok(None)` for a valid but
/// unbound key (the caller falls that back to the operator), and
/// `Err(reject)` when the key does not authenticate.
fn validate_api_key(
    db: &DbPool,
    manager: &ApiKeyManagerV0,
    token: &str,
) -> Result<Option<AuthUser>, ApiKeyReject> {
    use api_keys_simplified::SecureString;

    let secure_token = SecureString::from(token.to_string());

    // Fast checksum pre-check: reject malformed keys in ~20μs without touching DB.
    match manager.verify_checksum(&secure_token) {
        Ok(true) => {}
        _ => return Err(ApiKeyReject::BadChecksum),
    }

    // Compute deterministic key ID (BLAKE3, ~microseconds) for O(1) DB lookup.
    let key_id = manager.extract_key_id(&secure_token);

    // Look up the single matching key by key_id (indexed query).
    let key_row: Option<ApiKeyRow> = {
        let conn = db.read().map_err(|_| ApiKeyReject::Db)?;
        conn.query_row(
            "SELECT id, key_hash, user_id FROM api_keys WHERE key_id = ?1 AND revoked = 0 \
             AND (expires_at IS NULL OR datetime(expires_at) > datetime('now'))",
            params![key_id],
            |row| {
                Ok(ApiKeyRow {
                    id: row.get(0)?,
                    hash: row.get(1)?,
                    user_id: row.get(2)?,
                })
            },
        )
        .ok()
    };

    // Fallback: keys created before migration 010 have no key_id — scan those.
    let key_row = key_row.or_else(|| {
        let conn = db.read().ok()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, key_hash, user_id FROM api_keys WHERE key_id IS NULL AND revoked = 0 \
                 AND (expires_at IS NULL OR datetime(expires_at) > datetime('now'))",
            )
            .ok()?;
        let rows: Vec<ApiKeyRow> = stmt
            .query_map([], |row| {
                Ok(ApiKeyRow {
                    id: row.get(0)?,
                    hash: row.get(1)?,
                    user_id: row.get(2)?,
                })
            })
            .ok()?
            .filter_map(|r| r.ok())
            .collect();
        for row in rows {
            if let Ok(KeyStatus::Valid) = manager.verify(&secure_token, &row.hash) {
                // Backfill the key_id so future lookups are O(1).
                if let Ok(wconn) = db.write() {
                    let _ = wconn.execute(
                        "UPDATE api_keys SET key_id = ?1 WHERE id = ?2",
                        params![key_id, row.id],
                    );
                }
                return Some(row);
            }
        }
        None
    });

    let Some(key) = key_row else {
        return Err(ApiKeyReject::NotFound);
    };

    match manager.verify(&secure_token, &key.hash) {
        Ok(KeyStatus::Valid) => {
            // Resolve the user if the key has a user_id. A valid-but-unbound
            // key (legacy, or a fresh-install unassigned key) is Ok(None) — the
            // caller falls back to the operator.
            let auth_user = match key.user_id {
                None => None,
                Some(uid) => {
                    let conn = db.read().map_err(|_| ApiKeyReject::Db)?;
                    match crate::db::queries::users::get_user_by_id(&conn, uid) {
                        // LIF-214 follow-up: a key bound to a deactivated
                        // account, or to a bot whose *owner* is deactivated,
                        // is a dead credential. It must be rejected outright
                        // rather than degraded to `None`, which would hand it
                        // the unbound-key operator fallback (first admin) and
                        // turn deactivation into a promotion.
                        Ok(u) => match crate::db::queries::users::credential_is_live(&conn, &u) {
                            Ok(true) => Some(crate::db::models::AuthUser {
                                id: u.id,
                                username: u.username,
                                display_name: u.display_name,
                                is_admin: u.is_admin,
                            }),
                            Ok(false) => return Err(ApiKeyReject::Inactive),
                            Err(_) => return Err(ApiKeyReject::Db),
                        },
                        // Unchanged: a binding to a user row that no longer
                        // exists stays unresolved.
                        Err(_) => None,
                    }
                }
            };
            Ok(auth_user)
        }
        _ => Err(ApiKeyReject::HashMismatch),
    }
}

/// LIFIC-18: resolve an API key (e.g. the `LIFIC_TOKEN` carried by a stdio
/// agent) to its bound user, without an HTTP request context.
///
/// Returns `Some(user)` when the key is valid AND bound to a user; `Ok(None)`
/// for a valid-but-unbound key (the stdio session then falls back to the
/// operator). An invalid/unrecognized key is an error so the caller can warn
/// loudly and still degrade to the operator fallback.
pub fn resolve_api_key_user(
    db: &DbPool,
    manager: &ApiKeyManagerV0,
    token: &str,
) -> Result<Option<AuthUser>, String> {
    // Reuse the shared validator (same checksum/lookup/backfill/hash logic the
    // HTTP middleware runs). Mapping the typed rejection to a human string
    // keeps the stdio resolver vendoring nothing of its own.
    validate_api_key(db, manager, token).map_err(|reject| match reject {
        ApiKeyReject::Db => "database error".to_string(),
        ApiKeyReject::BadChecksum => "invalid API key checksum".to_string(),
        ApiKeyReject::NotFound => "invalid API key".to_string(),
        ApiKeyReject::HashMismatch => "API key hash verification failed".to_string(),
        ApiKeyReject::Inactive => "this account is deactivated".to_string(),
    })
}

/// LIFIC-18: resolve the `LIFIC_TOKEN` a stdio agent carries into its bound
/// user. `Ok(None)` when the token is absent, empty, or valid-but-unbound —
/// the session runs as the operator. `Err` when a token was present but
/// invalid (checksum/DB/hash failure), so the stdio entrypoint can emit a
/// distinct warning while still degrading to the operator fallback.
pub fn resolve_stdio_token(
    db: &DbPool,
    manager: &ApiKeyManagerV0,
) -> Result<Option<AuthUser>, String> {
    let raw = std::env::var("LIFIC_TOKEN").unwrap_or_default();
    let token = raw.trim();
    if token.is_empty() {
        return Ok(None);
    }
    resolve_api_key_user(db, manager, token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use api_keys_simplified::SecureString;
    use axum::{Extension, Router, middleware, routing::get};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_db() -> db::DbPool {
        db::open_memory().expect("test db")
    }

    // Serializes env mutation (LIFIC_TOKEN) across the stdio-token tests —
    // the process env is global, so every test that touches it must share one
    // lock, not declare its own.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    // ── Expiry is compared as a datetime, not as text ────────
    //
    // `expires_at` accepts ISO 8601, and `chrono`'s `to_rfc3339` writes
    // '2026-08-20T12:00:00+00:00'. SQLite's `datetime('now')` produces
    // '2026-08-20 12:00:00'. Compared as raw text those disagree within the
    // same day, because 'T' (0x54) sorts after every digit: a same-day RFC
    // 3339 timestamp reads as *later* than any SQLite-format one, so an
    // expired key looked live and a live one could look expired. Wrapping the
    // column in `datetime()` normalizes both sides.
    //
    // Times are a minute either side of now, never seconds, so the tests do
    // not sit on a boundary.

    fn rfc3339_from_now(minutes: i64) -> String {
        (chrono::Utc::now() + chrono::Duration::minutes(minutes)).to_rfc3339()
    }

    /// The indexed lookup path (`key_id` present).
    #[test]
    fn an_iso8601_expiry_is_honoured_on_the_indexed_lookup() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();

        let live = create_api_key_with_expiry(
            &pool,
            &manager,
            "live",
            Some(&rfc3339_from_now(1)),
            None,
        )
        .unwrap();
        let dead = create_api_key_with_expiry(
            &pool,
            &manager,
            "dead",
            Some(&rfc3339_from_now(-1)),
            None,
        )
        .unwrap();

        assert!(
            validate_api_key(&pool, &manager, &live).is_ok(),
            "a key expiring in a minute must still authenticate"
        );
        assert!(
            validate_api_key(&pool, &manager, &dead).is_err(),
            "a key that expired a minute ago must not authenticate"
        );
    }

    /// The pre-migration-010 fallback path, which scans rows with a NULL
    /// `key_id`. It carries its own copy of the predicate, so it needs its own
    /// proof.
    #[test]
    fn an_iso8601_expiry_is_honoured_on_the_legacy_null_key_id_path() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();

        let live = create_api_key_with_expiry(
            &pool,
            &manager,
            "legacy-live",
            Some(&rfc3339_from_now(1)),
            None,
        )
        .unwrap();
        let dead = create_api_key_with_expiry(
            &pool,
            &manager,
            "legacy-dead",
            Some(&rfc3339_from_now(-1)),
            None,
        )
        .unwrap();
        // Make both look like pre-010 rows so the scan is the only way in.
        pool.write()
            .unwrap()
            .execute("UPDATE api_keys SET key_id = NULL", [])
            .unwrap();

        assert!(
            validate_api_key(&pool, &manager, &live).is_ok(),
            "the legacy scan must honour a live ISO 8601 expiry"
        );
        assert!(
            validate_api_key(&pool, &manager, &dead).is_err(),
            "the legacy scan must reject an expired ISO 8601 key"
        );
    }

    /// A key with no expiry is unaffected either way.
    #[test]
    fn a_key_without_an_expiry_still_authenticates() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let key = create_api_key(&pool, &manager, "forever", None).unwrap();
        assert!(validate_api_key(&pool, &manager, &key).is_ok());
    }

    #[test]
    fn create_key_returns_valid_format() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let key = create_api_key(&pool, &manager, "test-key", None).unwrap();
        assert!(key.starts_with("lific_sk-live-"));
    }

    // ── Name reuse after revocation, scoped to the owner ────────
    //
    // `api_keys.name` is globally UNIQUE, so a revoked row keeps its name
    // reserved. An account lockdown revokes `opencode-blake`; without reuse in
    // `PreparedApiKey::insert`, reconnecting that tool hits the UNIQUE
    // constraint and the account cannot get its tools back.
    //
    // Reuse is owner-scoped, so the matrix below is the actual contract: same
    // owner reuses, any other owner is refused and the tombstone is left
    // exactly as it was. `NULL` (the unbound operator key) is its own owner in
    // both directions.

    /// `api_keys.user_id` is a real foreign key, so a binding needs a real row.
    fn seed_key_owner(pool: &db::DbPool, username: &str) -> i64 {
        let conn = pool.write().unwrap();
        crate::db::queries::users::create_user(
            &conn,
            &crate::db::models::CreateUser {
                username: username.into(),
                email: format!("{username}@test.local"),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap()
        .id
    }

    /// Full snapshot of a name's row, so a refusal can be shown to have
    /// changed nothing at all: not the id, not the stored hash, not the owner,
    /// not the creation timestamp.
    #[derive(Debug, PartialEq)]
    struct KeyRow {
        id: i64,
        user_id: Option<i64>,
        revoked: bool,
        key_hash: String,
        created_at: String,
    }

    fn key_rows(pool: &db::DbPool, name: &str) -> Vec<KeyRow> {
        let conn = pool.read().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, revoked, key_hash, created_at
                 FROM api_keys WHERE name = ?1 ORDER BY id",
            )
            .unwrap();
        let rows = stmt
            .query_map(params![name], |r| {
                Ok(KeyRow {
                    id: r.get(0)?,
                    user_id: r.get(1)?,
                    revoked: r.get(2)?,
                    key_hash: r.get(3)?,
                    created_at: r.get(4)?,
                })
            })
            .unwrap();
        rows.map(Result::unwrap).collect()
    }

    fn revoke_all(pool: &db::DbPool) {
        pool.write()
            .unwrap()
            .execute("UPDATE api_keys SET revoked = 1", [])
            .unwrap();
    }

    /// The reconnect case this whole rule exists for: a lockdown revoked the
    /// bot's key, and the bot claims its own derived name back.
    #[test]
    fn the_same_bot_reclaims_its_own_revoked_name() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let bot = seed_key_owner(&pool, "opencode-blake");
        let old = create_api_key(&pool, &manager, "opencode-blake", Some(bot)).unwrap();
        revoke_all(&pool);

        let fresh = create_api_key(&pool, &manager, "opencode-blake", Some(bot))
            .expect("a bot reclaims its own revoked name");
        assert_ne!(fresh, old);

        let rows = key_rows(&pool, "opencode-blake");
        assert_eq!(rows.len(), 1, "the dead row was swept, not accumulated");
        assert_eq!(rows[0].user_id, Some(bot));
        assert!(!rows[0].revoked);
        // The old plaintext is gone for good; only the new one authenticates.
        assert!(validate_api_key(&pool, &manager, &old).is_err());
        assert!(validate_api_key(&pool, &manager, &fresh).is_ok());
    }

    #[test]
    fn a_human_reclaims_their_own_revoked_name() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let blake = seed_key_owner(&pool, "blake");
        create_api_key(&pool, &manager, "laptop", Some(blake)).unwrap();
        revoke_all(&pool);

        let fresh = create_api_key(&pool, &manager, "laptop", Some(blake))
            .expect("the same human reuses their own name");
        assert!(validate_api_key(&pool, &manager, &fresh).is_ok());
        assert_eq!(key_rows(&pool, "laptop")[0].user_id, Some(blake));
    }

    #[test]
    fn an_unbound_key_reclaims_its_own_revoked_name() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        create_api_key(&pool, &manager, "default", None).unwrap();
        revoke_all(&pool);

        let fresh = create_api_key(&pool, &manager, "default", None)
            .expect("the operator key reuses its own name");
        assert!(validate_api_key(&pool, &manager, &fresh).is_ok());
        let rows = key_rows(&pool, "default");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].user_id, None);
    }

    /// Every way one owner could try to take a name another owner's tombstone
    /// still holds. In each case the refusal must leave the tombstone byte for
    /// byte as it was and write no new row.
    #[test]
    fn a_revoked_name_belonging_to_another_owner_is_refused_untouched() {
        for claimant in ["different-human", "unbound", "bound"] {
            let pool = test_db();
            let manager = create_key_manager().unwrap();
            let blake = seed_key_owner(&pool, "blake");
            let mallory = seed_key_owner(&pool, "mallory");

            // Who holds the tombstone, and who tries to take it.
            let (holder, claimer) = match claimant {
                // A second human cannot displace Blake's revoked key.
                "different-human" => (Some(blake), Some(mallory)),
                // A human cannot displace the unbound operator key's name,
                // which would hand them a name the operator still owns.
                "unbound" => (None, Some(mallory)),
                // Nor the reverse: minting an unbound operator key must not
                // consume a name a real account is still holding.
                "bound" => (Some(blake), None),
                _ => unreachable!(),
            };

            create_api_key(&pool, &manager, "contested", holder).unwrap();
            revoke_all(&pool);
            let before = key_rows(&pool, "contested");

            let err = match create_api_key(&pool, &manager, "contested", claimer) {
                Ok(_) => panic!("{claimant}: another owner's tombstone must not be claimable"),
                Err(err) => err,
            };
            match err {
                crate::error::LificError::BadRequest(message) => assert!(
                    message.contains("reserved by another owner"),
                    "{claimant}: {message}"
                ),
                other => panic!("{claimant}: expected BadRequest, got {other:?}"),
            }

            assert_eq!(
                key_rows(&pool, "contested"),
                before,
                "{claimant}: the refusal must not touch the row or add one"
            );
        }
    }

    #[test]
    fn an_active_name_is_still_refused_and_leaves_the_live_key_alone() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let owner = seed_key_owner(&pool, "blake");
        let other = seed_key_owner(&pool, "mallory");
        let live = create_api_key(&pool, &manager, "opencode-blake", Some(owner)).unwrap();
        let before = key_rows(&pool, "opencode-blake");

        // Refused for both a different owner and the same one: an active name
        // is taken regardless of who is asking, and the message says only that.
        for claimer in [Some(other), Some(owner), None] {
            let err = create_api_key(&pool, &manager, "opencode-blake", claimer)
                .expect_err("an active name is taken");
            match err {
                crate::error::LificError::BadRequest(message) => {
                    assert!(message.contains("already exists"), "{message}");
                }
                other => panic!("expected BadRequest, got {other:?}"),
            }
        }

        assert_eq!(
            key_rows(&pool, "opencode-blake"),
            before,
            "the refusal wrote nothing and did not rebind the live key"
        );
        assert!(
            validate_api_key(&pool, &manager, &live).is_ok(),
            "the live key still authenticates"
        );
    }

    #[test]
    fn reuse_matches_the_exact_name_only() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let owner = seed_key_owner(&pool, "blake");
        create_api_key(&pool, &manager, "opencode-blake", Some(owner)).unwrap();
        create_api_key(&pool, &manager, "opencode-blake-laptop", Some(owner)).unwrap();
        let neighbour = create_api_key(&pool, &manager, "cursor-blake", Some(owner)).unwrap();
        pool.write()
            .unwrap()
            .execute(
                "UPDATE api_keys SET revoked = 1 WHERE name = 'opencode-blake'",
                [],
            )
            .unwrap();

        create_api_key(&pool, &manager, "opencode-blake", Some(owner)).unwrap();

        assert_eq!(key_rows(&pool, "opencode-blake").len(), 1);
        assert_eq!(
            key_rows(&pool, "opencode-blake-laptop").len(),
            1,
            "a name this one is a prefix of is untouched"
        );
        assert!(validate_api_key(&pool, &manager, &neighbour).is_ok());
    }

    /// The revocation is a historical fact and stays in the log after the row
    /// it described is swept. Nothing joins the two, so nothing breaks, and no
    /// key material was ever in the log to leak.
    #[test]
    fn sweeping_a_revoked_row_leaves_its_audit_trail_intact() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let user_id = {
            let conn = pool.write().unwrap();
            let user = crate::db::queries::users::create_user(
                &conn,
                &crate::db::models::CreateUser {
                    username: "blake".into(),
                    email: "blake@test.local".into(),
                    password: "testpassword1".into(),
                    display_name: None,
                    is_admin: true,
                    is_bot: false,
                },
            )
            .unwrap();
            user.id
        };
        create_api_key(&pool, &manager, "opencode-blake", Some(user_id)).unwrap();
        {
            let conn = pool.write().unwrap();
            crate::db::queries::users::lock_down_account(&conn, user_id).unwrap();
        }

        create_api_key(&pool, &manager, "opencode-blake", Some(user_id)).unwrap();

        let conn = pool.read().unwrap();
        let audited: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM audit_log
                 WHERE entity_type = 'api_key' AND action = 'revoke'
                   AND entity_label = 'opencode-blake'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(audited, 1, "the revocation is still on the record");
        let leaked: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM audit_log WHERE new_value LIKE 'lific_sk%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(leaked, 0);
    }

    /// Rotation is the one path allowed to replace a *live* key, and it must
    /// still carry the owner over. It is now one transaction, so the name is
    /// never briefly absent.
    #[test]
    fn rotation_replaces_a_live_key_and_keeps_its_owner() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let bot = seed_key_owner(&pool, "opencode-blake");
        let old = create_api_key(&pool, &manager, "opencode-blake", Some(bot)).unwrap();

        let fresh = rotate_api_key(&pool, &manager, "opencode-blake").unwrap();
        assert_ne!(fresh, old);
        let rows = key_rows(&pool, "opencode-blake");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].user_id, Some(bot), "the binding carried over");
        assert!(validate_api_key(&pool, &manager, &old).is_err());
        assert!(validate_api_key(&pool, &manager, &fresh).is_ok());
    }

    #[test]
    fn rotation_of_a_missing_name_is_not_found_and_writes_nothing() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let err = rotate_api_key(&pool, &manager, "never-existed").expect_err("nothing to rotate");
        assert!(matches!(err, crate::error::LificError::NotFound(_)));
        assert!(key_rows(&pool, "never-existed").is_empty());
    }

    /// Credential creation and account lockdown, actually racing.
    ///
    /// Everything else about this interaction is asserted sequentially, which
    /// can only show that each order produces the right outcome. What is
    /// claimed on top of that is *linearizability*: that no interleaving
    /// exists, because both sides run as one SQLite `BEGIN IMMEDIATE`
    /// transaction and SQLite admits one writer at a time. That claim needs
    /// two real connections to a real file, and this is where it is tested.
    ///
    /// Setup: two `DbPool`s opened separately on one tempfile database, which
    /// is the same separation two processes have (the CLI resetting a password
    /// while the server is running). Sequencing is by channel; there are no
    /// sleeps and no timing assumptions anywhere.
    ///
    /// Exclusion is observed rather than assumed. While one pool holds its
    /// transaction open, a third raw connection with `busy_timeout = 0` tries
    /// `BEGIN IMMEDIATE` and must be refused *now* rather than made to wait.
    /// That is SQLite telling us directly that the writer is held.
    mod lockdown_race {
        use super::*;
        use std::sync::mpsc;

        /// A human with a fresh session, the shape a REST credential mint
        /// authorizes against.
        fn seed(pool: &db::DbPool) -> (i64, String) {
            let conn = pool.write().unwrap();
            let user = crate::db::queries::users::create_user(
                &conn,
                &crate::db::models::CreateUser {
                    username: "blake".into(),
                    email: "blake@test.local".into(),
                    password: "testpassword1".into(),
                    display_name: None,
                    is_admin: true,
                    is_bot: false,
                },
            )
            .unwrap();
            let session = crate::db::queries::users::create_session(&conn, user.id, None).unwrap();
            (user.id, session.token)
        }

        /// Whether the database's single write slot is currently taken.
        ///
        /// Opens its own connection and disables the busy handler, so
        /// `BEGIN IMMEDIATE` resolves immediately either way: it takes the
        /// lock (nobody is writing) or returns SQLITE_BUSY (somebody is). No
        /// waiting, so no flakiness.
        fn writer_is_held(path: &std::path::Path) -> bool {
            let probe = rusqlite::Connection::open(path).expect("probe connection");
            probe
                .busy_timeout(std::time::Duration::ZERO)
                .expect("disable the busy handler");
            match probe.execute_batch("BEGIN IMMEDIATE") {
                Ok(()) => {
                    probe.execute_batch("ROLLBACK").expect("release the probe");
                    false
                }
                // Only contention counts. Any other failure means the probe
                // itself is broken (a missing file, a corrupt database), and
                // reading that as "somebody is writing" would let this whole
                // test pass for the wrong reason.
                Err(rusqlite::Error::SqliteFailure(error, _))
                    if matches!(
                        error.code,
                        rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                    ) =>
                {
                    true
                }
                Err(other) => panic!("probe failed for a reason other than contention: {other:?}"),
            }
        }

        fn active_keys(pool: &db::DbPool, name: &str) -> i64 {
            pool.read()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM api_keys WHERE name = ?1 AND revoked = 0",
                    params![name],
                    |r| r.get(0),
                )
                .unwrap()
        }

        /// Mint first: the lockdown cannot begin until the mint commits, and
        /// then it revokes the key the mint made. No interleaving leaves a
        /// live key behind a locked-down account.
        #[test]
        fn a_lockdown_cannot_slip_inside_a_credential_mint() {
            let dir = tempfile::tempdir().expect("scratch dir");
            let path = dir.path().join("lific.db");
            let minting = db::open(&path).expect("minting pool");
            let recovering = db::open(&path).expect("recovering pool");
            let (user_id, session) = seed(&minting);
            let manager = create_key_manager().unwrap();
            let prepared = PreparedApiKey::generate(&manager).unwrap();
            assert!(
                !writer_is_held(&path),
                "control: with nobody writing, the probe must be able to take the lock"
            );

            let (inserted_tx, inserted_rx) = mpsc::channel::<()>();
            let (probe_tx, probe_rx) = mpsc::channel::<bool>();

            let (minting, session) = (&minting, &session);
            let (key, held_during_mint) = std::thread::scope(|scope| {
                let mint = scope.spawn(move || {
                    let held = std::cell::Cell::new(false);
                    // Exactly the transaction `POST /api/auth/keys` runs.
                    let key = minting.transaction(|tx| {
                        revalidate_recent_session(tx, session, user_id)?;
                        let plaintext = prepared.insert(tx, "racing", None, Some(user_id))?;
                        inserted_tx.send(()).expect("announce the insert");
                        // Still inside the transaction: hold it open until the
                        // other side has been told it cannot get in.
                        held.set(probe_rx.recv().expect("probe result"));
                        Ok(plaintext)
                    });
                    (key.expect("the mint commits"), held.get())
                });

                inserted_rx.recv().expect("the mint reached its insert");
                probe_tx
                    .send(writer_is_held(&path))
                    .expect("hand the probe result back");
                mint.join().expect("mint thread")
            });

            assert!(
                held_during_mint,
                "the write slot must be taken for the whole mint transaction"
            );
            assert_eq!(active_keys(minting, "racing"), 1, "the mint committed");

            // Only now, on the other pool, can the recovery run.
            recovering
                .transaction(|tx| crate::db::queries::users::lock_down_account(tx, user_id))
                .expect("the lockdown commits once the writer is free");

            assert!(
                validate_api_key(&recovering, &manager, &key).is_err(),
                "a key minted immediately before a lockdown is inside its blast radius"
            );
            assert_eq!(active_keys(&recovering, "racing"), 0);
        }

        /// Lockdown first: the mint cannot begin until the lockdown commits,
        /// and then its in-transaction revalidation finds no session and
        /// refuses. No interleaving lets a key through on a dead session.
        #[test]
        fn a_credential_mint_cannot_slip_inside_a_lockdown() {
            let dir = tempfile::tempdir().expect("scratch dir");
            let path = dir.path().join("lific.db");
            let minting = db::open(&path).expect("minting pool");
            let recovering = db::open(&path).expect("recovering pool");
            let (user_id, session) = seed(&minting);
            let manager = create_key_manager().unwrap();
            let prepared = PreparedApiKey::generate(&manager).unwrap();
            assert!(
                !writer_is_held(&path),
                "control: with nobody writing, the probe must be able to take the lock"
            );

            let (locked_tx, locked_rx) = mpsc::channel::<()>();
            let (probe_tx, probe_rx) = mpsc::channel::<bool>();

            let recovering_ref = &recovering;
            let held_during_lockdown = std::thread::scope(|scope| {
                let recovery = scope.spawn(move || {
                    let held = std::cell::Cell::new(false);
                    recovering_ref
                        .transaction(|tx| {
                            crate::db::queries::users::lock_down_account(tx, user_id)?;
                            locked_tx.send(()).expect("announce the lockdown");
                            held.set(probe_rx.recv().expect("probe result"));
                            Ok(())
                        })
                        .expect("the lockdown commits");
                    held.get()
                });

                locked_rx.recv().expect("the lockdown reached its writes");
                probe_tx
                    .send(writer_is_held(&path))
                    .expect("hand the probe result back");
                recovery.join().expect("recovery thread")
            });

            assert!(
                held_during_lockdown,
                "the write slot must be taken for the whole lockdown transaction"
            );

            // The mint now runs against the committed post-lockdown state.
            let outcome = minting.transaction(|tx| {
                revalidate_recent_session(tx, &session, user_id)?;
                prepared.insert(tx, "racing", None, Some(user_id))
            });
            assert!(
                matches!(outcome, Err(crate::error::LificError::Forbidden(_))),
                "revalidation must refuse a session the lockdown deleted: {outcome:?}"
            );
            assert_eq!(
                active_keys(&minting, "racing"),
                0,
                "the refused mint wrote nothing"
            );
        }
    }

    #[test]
    fn verify_key_succeeds() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let key = create_api_key(&pool, &manager, "test-key", None).unwrap();

        // Load the hash and verify
        let keys = list_api_keys(&pool).unwrap();
        assert_eq!(keys.len(), 1);

        let secure_key = SecureString::from(key);
        let conn = pool.read().unwrap();
        let hash: String = conn
            .query_row(
                "SELECT key_hash FROM api_keys WHERE name = 'test-key'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        let status = manager.verify(&secure_key, &hash).unwrap();
        assert!(matches!(status, KeyStatus::Valid));
    }

    #[test]
    fn wrong_key_fails() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        create_api_key(&pool, &manager, "test-key", None).unwrap();

        let conn = pool.read().unwrap();
        let hash: String = conn
            .query_row(
                "SELECT key_hash FROM api_keys WHERE name = 'test-key'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        let wrong_key = SecureString::from(
            "lific_sk-live-AAAAAAAAAAAAAAAAAAAAAAAAAAAA.0000000000000000".to_string(),
        );
        let status = manager.verify(&wrong_key, &hash);
        // Either returns Invalid or an error (checksum mismatch) -- both mean rejection
        if let Ok(KeyStatus::Valid) = status {
            panic!("wrong key should not validate");
        }
    }

    #[test]
    fn revoke_key_works() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        create_api_key(&pool, &manager, "revoke-me", None).unwrap();

        revoke_api_key(&pool, "revoke-me").unwrap();

        let keys = list_api_keys(&pool).unwrap();
        assert!(keys[0].revoked);
    }

    #[test]
    fn rotate_key_replaces_old() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let old_key = create_api_key(&pool, &manager, "rotate-me", None).unwrap();
        let new_key = rotate_api_key(&pool, &manager, "rotate-me").unwrap();

        assert_ne!(old_key, new_key);
        assert!(new_key.starts_with("lific_sk-live-"));

        // Old key deleted, only new key remains
        let keys = list_api_keys(&pool).unwrap();
        assert_eq!(keys.len(), 1);
        assert!(!keys[0].revoked);
    }

    // LIF-391: a key that belongs to a user is bound by the insert that
    // creates it. There is no window where the row exists unbound, which is
    // what used to let a crash mid-creation strand a key that resolves as
    // the operator.
    #[test]
    fn created_key_is_bound_by_the_insert() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let user_id = {
            let conn = pool.write().unwrap();
            conn.execute(
                "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot)
                 VALUES ('owner', 'owner@test.local', 'x', 'Owner', 0, 0)",
                [],
            )
            .unwrap();
            conn.last_insert_rowid()
        };
        create_api_key(&pool, &manager, "owned", Some(user_id)).unwrap();
        create_api_key_with_expiry(&pool, &manager, "dated", Some("2030-06-01"), Some(user_id))
            .unwrap();

        let conn = pool.read().unwrap();
        for name in ["owned", "dated"] {
            let bound: Option<i64> = conn
                .query_row(
                    "SELECT user_id FROM api_keys WHERE name = ?1",
                    params![name],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(bound, Some(user_id), "key '{name}' must be created bound");
        }

        // No binding asked for, none written.
        create_api_key(&pool, &manager, "anonymous", None).unwrap();
        let bound: Option<i64> = conn
            .query_row(
                "SELECT user_id FROM api_keys WHERE name = 'anonymous'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(bound, None);
    }

    // LIF-391: `lific connect` re-minting a bot's key rotates an existing
    // name; the new key must land on the bot even when the old row was
    // unbound, since the rotate path no longer patches the binding after.
    #[test]
    fn rotate_bound_overrides_the_previous_binding() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        create_api_key(&pool, &manager, "claude-code-owner", None).unwrap();
        let bot_id = {
            let conn = pool.write().unwrap();
            conn.execute(
                "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot)
                 VALUES ('claude-code', 'bot@test.local', 'x', 'Claude Code', 0, 1)",
                [],
            )
            .unwrap();
            conn.last_insert_rowid()
        };

        rotate_api_key_bound(&pool, &manager, "claude-code-owner", Some(bot_id)).unwrap();

        let conn = pool.read().unwrap();
        let bound: Option<i64> = conn
            .query_row(
                "SELECT user_id FROM api_keys WHERE name = 'claude-code-owner' AND revoked = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(bound, Some(bot_id));
    }

    // LIF-132: rotation must carry the user binding over to the new key.
    // Previously the old row was deleted (user_id and all) and the new key
    // was created unbound, silently de-attributing bot/user keys.
    #[test]
    fn rotate_key_preserves_user_binding() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        // A key bound to a user at creation.
        let user_id = {
            let conn = pool.write().unwrap();
            conn.execute(
                "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot)
                 VALUES ('bot', 'bot@test.local', 'x', 'Bot', 0, 1)",
                [],
            )
            .unwrap();
            conn.last_insert_rowid()
        };
        create_api_key(&pool, &manager, "bot-key", Some(user_id)).unwrap();

        rotate_api_key(&pool, &manager, "bot-key").unwrap();

        let conn = pool.read().unwrap();
        let bound: Option<i64> = conn
            .query_row(
                "SELECT user_id FROM api_keys WHERE name = 'bot-key' AND revoked = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            bound,
            Some(user_id),
            "rotated key must keep its user binding"
        );
    }

    // LIF-132: rotating an unbound key still works and stays unbound.
    #[test]
    fn rotate_unbound_key_stays_unbound() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        create_api_key(&pool, &manager, "plain", None).unwrap();
        rotate_api_key(&pool, &manager, "plain").unwrap();

        let conn = pool.read().unwrap();
        let bound: Option<i64> = conn
            .query_row(
                "SELECT user_id FROM api_keys WHERE name = 'plain' AND revoked = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(bound, None);
    }

    #[test]
    fn duplicate_name_rejected() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        create_api_key(&pool, &manager, "unique", None).unwrap();
        let result = create_api_key(&pool, &manager, "unique", None);
        assert!(result.is_err());
    }

    #[test]
    fn has_any_keys_works() {
        let pool = test_db();
        assert!(!has_any_keys(&pool));

        let manager = create_key_manager().unwrap();
        create_api_key(&pool, &manager, "first", None).unwrap();
        assert!(has_any_keys(&pool));
    }

    // LIFIC-9: the initial-key decision is shared by `init` and `start`.
    #[test]
    fn should_mint_initial_key_empty_bootstrap_mints() {
        let pool = test_db();
        // No humans, no keys: the genuinely empty bootstrap.
        assert!(should_mint_initial_key(&pool));
    }

    #[test]
    fn should_mint_initial_key_false_once_a_human_operator_exists() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        crate::db::queries::users::create_passwordless_admin(&conn, "Blake").unwrap();
        drop(conn);
        // A human exists even with zero keys: passwordless mode, no mint.
        assert!(!should_mint_initial_key(&pool));
    }

    #[test]
    fn should_mint_initial_key_false_when_any_key_exists() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        create_api_key(&pool, &manager, "first", None).unwrap();
        assert!(!should_mint_initial_key(&pool));
    }

    #[test]
    fn create_key_stores_key_id() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let key = create_api_key(&pool, &manager, "id-test", None).unwrap();

        let conn = pool.read().unwrap();
        let stored_key_id: Option<String> = conn
            .query_row(
                "SELECT key_id FROM api_keys WHERE name = 'id-test'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        // key_id should be stored and be a 32-char hex string
        let key_id = stored_key_id.expect("key_id should be stored");
        assert_eq!(key_id.len(), 32);
        assert!(key_id.chars().all(|c| c.is_ascii_hexdigit()));

        // Extracting key_id from the plaintext should match
        let secure_key = SecureString::from(key);
        let extracted_id = manager.extract_key_id(&secure_key);
        assert_eq!(extracted_id, key_id);
    }

    #[test]
    fn key_id_lookup_finds_correct_key() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();

        // Create multiple keys
        let key1 = create_api_key(&pool, &manager, "key-1", None).unwrap();
        let _key2 = create_api_key(&pool, &manager, "key-2", None).unwrap();

        // Extract key_id from key1 and look it up
        let secure_key = SecureString::from(key1.clone());
        let key_id = manager.extract_key_id(&secure_key);

        let conn = pool.read().unwrap();
        let found_name: String = conn
            .query_row(
                "SELECT name FROM api_keys WHERE key_id = ?1 AND revoked = 0",
                params![key_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(found_name, "key-1");
    }

    #[test]
    fn legacy_key_without_key_id_still_verifiable() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let key = create_api_key(&pool, &manager, "legacy", None).unwrap();

        // Simulate a pre-migration key by clearing key_id
        let conn = pool.write().unwrap();
        conn.execute(
            "UPDATE api_keys SET key_id = NULL WHERE name = 'legacy'",
            [],
        )
        .unwrap();
        drop(conn);

        // Verify still works by scanning NULL key_id rows
        let secure_key = SecureString::from(key);
        let conn = pool.read().unwrap();
        let hash: String = conn
            .query_row(
                "SELECT key_hash FROM api_keys WHERE name = 'legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        let status = manager.verify(&secure_key, &hash).unwrap();
        assert!(matches!(status, KeyStatus::Valid));
    }

    // ── LIF-204: OAuth-token user_id -> resolved AuthUser (REST middleware) ──
    //
    // `require_api_key` already resolves an OAuth token's bound user_id into a
    // full `AuthUser` (LIF-79) and inserts it as `Extension<Option<AuthUser>>`.
    // These tests exercise that resolution end-to-end through the actual
    // middleware (rather than just the lower-level `oauth::oauth_token_user_id`
    // helper, already covered in oauth.rs) to prove the request path shared by
    // every REST handler and the /mcp route.

    fn test_auth_state(pool: &db::DbPool) -> AuthState {
        AuthState {
            db: pool.clone(),
            manager: create_key_manager().unwrap(),
            public_url: "https://example.com".into(),
            required: true,
        }
    }

    /// Minimal router: `require_api_key` in front of a handler that echoes
    /// back whatever `Extension<Option<AuthUser>>` the middleware resolved.
    /// Lets tests assert on the resolved identity without a full REST route.
    fn echo_app(auth_state: AuthState) -> Router {
        async fn echo(
            Extension(auth_user): Extension<Option<crate::db::models::AuthUser>>,
        ) -> String {
            match auth_user {
                Some(u) => format!("user:{}:{}:{}", u.id, u.username, u.is_admin),
                None => "none".to_string(),
            }
        }
        Router::new()
            .route("/echo", get(echo))
            .layer(middleware::from_fn_with_state(auth_state, require_api_key))
    }

    /// Insert an `oauth_tokens` row directly, bound to `user_id` (or
    /// unbound if `None`), bypassing the full authorize/token-exchange dance
    /// (already covered end-to-end in oauth.rs). Returns the raw bearer token.
    fn insert_oauth_token(pool: &db::DbPool, suffix: &str, user_id: Option<i64>) -> String {
        let token = format!("lific_at_test-{suffix}");
        let hash = sha256_hex(token.as_bytes());
        let expires = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        let client_id = format!("client-{suffix}");
        let conn = pool.write().unwrap();
        conn.execute(
            "INSERT INTO oauth_clients (client_id, client_name, redirect_uris) VALUES (?1, 'Test', '[\"http://localhost\"]')",
            params![client_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO oauth_tokens (access_token, client_id, expires_at, scope, user_id) VALUES (?1, ?2, ?3, 'mcp', ?4)",
            params![hash, client_id, expires, user_id],
        )
        .unwrap();
        token
    }

    #[tokio::test]
    async fn oauth_token_rest_request_resolves_to_correct_auth_user() {
        let pool = test_db();
        let user_id = {
            let conn = pool.write().unwrap();
            crate::db::queries::users::create_user(
                &conn,
                &crate::db::models::CreateUser {
                    username: "tokenuser".into(),
                    email: "tokenuser@test.com".into(),
                    password: "testpassword1".into(),
                    display_name: Some("Token User".into()),
                    is_admin: false,
                    is_bot: false,
                },
            )
            .unwrap()
            .id
        };
        let token = insert_oauth_token(&pool, "resolves", Some(user_id));

        let resp = echo_app(test_auth_state(&pool))
            .oneshot(
                Request::builder()
                    .uri("/echo")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            bytes.as_ref(),
            format!("user:{user_id}:tokenuser:false").as_bytes(),
            "OAuth token must resolve to the bound user, not None"
        );
    }

    #[tokio::test]
    async fn oauth_token_bound_to_bot_resolves_to_the_bot_identity() {
        // LIFIC-13: at OAuth approval Lific mints a per-tool bot and binds the
        // issued token to it. The middleware must resolve that token to the bot
        // (which authz then raises to the bot's owner for permissions).
        let pool = test_db();
        let bot_id = {
            let conn = pool.write().unwrap();
            crate::db::queries::users::create_user(
                &conn,
                &crate::db::models::CreateUser {
                    username: "claude-code-blake".into(),
                    email: "claude-code-blake@bot.local".into(),
                    password: "testpassword1".into(),
                    display_name: Some("Claude Code".into()),
                    is_admin: false,
                    is_bot: true,
                },
            )
            .unwrap()
            .id
        };
        let token = insert_oauth_token(&pool, "bot", Some(bot_id));

        let resp = identity_echo_app(test_auth_state(&pool))
            .oneshot(
                Request::builder()
                    .uri("/echo")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.as_ref().to_vec()).unwrap();
        assert!(
            body.contains(&format!("id:{bot_id}:claude-code-blake")),
            "OAuth token must resolve to the per-tool bot, got: {body}"
        );
    }

    #[tokio::test]
    async fn legacy_api_key_without_user_resolves_to_none_via_middleware() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let key = create_api_key(&pool, &manager, "legacy-plain", None).unwrap();

        let resp = echo_app(test_auth_state(&pool))
            .oneshot(
                Request::builder()
                    .uri("/echo")
                    .header("authorization", format!("Bearer {key}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            bytes.as_ref(),
            b"none",
            "a legacy key with no bound user must stay unresolved (default-deny)"
        );
    }

    // ── Owner deactivation reaches the bots (LIF-214 follow-up) ──
    //
    // A bot is a separate `users` row holding its own API key and OAuth
    // token, and `authz::effective_user` resolves it to its owner before any
    // permission check. Deactivating the owner therefore has to stop the
    // bot's credentials too, or the switched-off account keeps acting through
    // its agents. Enforcement lives on the read path, so these also assert
    // that reactivating the owner brings the bots straight back with the same
    // credentials.

    /// Owner (human, non-admin so the last-admin guard stays out of the way)
    /// plus one bot they own. Returns `(pool, owner_id, bot_id)`.
    fn owner_and_bot(pool: &db::DbPool) -> (i64, i64) {
        let conn = pool.write().unwrap();
        // An admin has to exist so the owner is never the last one.
        crate::db::queries::users::create_user(
            &conn,
            &crate::db::models::CreateUser {
                username: "instance-admin".into(),
                email: "instance-admin@test.com".into(),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: true,
                is_bot: false,
            },
        )
        .unwrap();
        let owner = crate::db::queries::users::create_user(
            &conn,
            &crate::db::models::CreateUser {
                username: "botowner".into(),
                email: "botowner@test.com".into(),
                password: "testpassword1".into(),
                display_name: Some("Bot Owner".into()),
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap();
        let bot =
            crate::db::queries::users::ensure_bot(&conn, owner.id, "opencode", "OpenCode").unwrap();
        (owner.id, bot.id)
    }

    fn set_owner_active(pool: &db::DbPool, owner_id: i64, active: bool) {
        let conn = pool.write().unwrap();
        crate::db::queries::users::set_active(&conn, owner_id, active).unwrap();
    }

    async fn echo_with_bearer(pool: &db::DbPool, token: &str) -> (StatusCode, String) {
        let resp = echo_app(test_auth_state(pool))
            .oneshot(
                Request::builder()
                    .uri("/echo")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn bot_api_key_stops_working_while_its_owner_is_deactivated() {
        let pool = test_db();
        let (owner_id, bot_id) = owner_and_bot(&pool);
        let manager = create_key_manager().unwrap();
        let key = create_api_key(&pool, &manager, "opencode-key", Some(bot_id)).unwrap();

        let (status, body) = echo_with_bearer(&pool, &key).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, format!("user:{bot_id}:opencode-botowner:false"));

        set_owner_active(&pool, owner_id, false);
        let (status, body) = echo_with_bearer(&pool, &key).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "a bot must not outlive its owner's access: {body}"
        );
        assert_ne!(
            body, "none",
            "and must never degrade to the unbound-key operator fallback"
        );

        set_owner_active(&pool, owner_id, true);
        let (status, body) = echo_with_bearer(&pool, &key).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "reactivating the owner restores the bot without re-minting: {body}"
        );
        assert_eq!(body, format!("user:{bot_id}:opencode-botowner:false"));
    }

    #[tokio::test]
    async fn bot_oauth_token_stops_working_while_its_owner_is_deactivated() {
        let pool = test_db();
        let (owner_id, bot_id) = owner_and_bot(&pool);
        let token = insert_oauth_token(&pool, "ownerdeact", Some(bot_id));

        let (status, body) = echo_with_bearer(&pool, &token).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, format!("user:{bot_id}:opencode-botowner:false"));

        set_owner_active(&pool, owner_id, false);
        let (status, body) = echo_with_bearer(&pool, &token).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "the bot's OAuth token dies with its owner's access: {body}"
        );

        set_owner_active(&pool, owner_id, true);
        let (status, body) = echo_with_bearer(&pool, &token).await;
        assert_eq!(status, StatusCode::OK, "restored on reactivation: {body}");
        assert_eq!(body, format!("user:{bot_id}:opencode-botowner:false"));
    }

    /// The read-path check is not bot-specific: a key bound to a deactivated
    /// human is refused even if the revocation `set_active` performs never
    /// happened (simulated here by flipping the flag directly).
    #[tokio::test]
    async fn api_key_bound_to_a_deactivated_human_is_refused_even_unrevoked() {
        let pool = test_db();
        let (owner_id, _bot_id) = owner_and_bot(&pool);
        let manager = create_key_manager().unwrap();
        let key = create_api_key(&pool, &manager, "human-key", Some(owner_id)).unwrap();

        {
            let conn = pool.write().unwrap();
            conn.execute(
                "UPDATE users SET is_active = 0 WHERE id = ?1",
                params![owner_id],
            )
            .unwrap();
        }

        let (status, body) = echo_with_bearer(&pool, &key).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_ne!(body, "none", "never falls back to the operator");
    }

    /// An ownerless bot (`owner_id IS NULL`) inherits nothing, so nothing
    /// gates it. `effective_user` evaluates it as itself; so does this.
    #[tokio::test]
    async fn ownerless_bot_key_is_unaffected_by_the_owner_check() {
        let pool = test_db();
        let bot_id = {
            let conn = pool.write().unwrap();
            crate::db::queries::users::create_user(
                &conn,
                &crate::db::models::CreateUser {
                    username: "orphan-bot".into(),
                    email: "orphan-bot@bot.local".into(),
                    password: "testpassword1".into(),
                    display_name: Some("Orphan".into()),
                    is_admin: false,
                    is_bot: true,
                },
            )
            .unwrap()
            .id
        };
        let manager = create_key_manager().unwrap();
        let key = create_api_key(&pool, &manager, "orphan-key", Some(bot_id)).unwrap();

        let (status, body) = echo_with_bearer(&pool, &key).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, format!("user:{bot_id}:orphan-bot:false"));
    }

    // ── LIF-294: [auth] required = false ─────────────────────────

    /// Router whose handler reports whether the request runs with the
    /// operator bypass: with authz enforcement ON, `visible_project_ids`
    /// for a `None` user returns unrestricted (None) only inside an
    /// operator context.
    fn operator_probe_app(auth_state: AuthState, pool: db::DbPool) -> Router {
        Router::new()
            .route(
                "/probe",
                get(
                    move |Extension(identity): Extension<
                        Option<crate::resolve_caller::ResolvedIdentity>,
                    >| {
                        let pool = pool.clone();
                        async move {
                            match crate::authz::visible_project_ids(&pool, &identity).unwrap() {
                                None => "unrestricted".to_string(),
                                Some(ids) => format!("restricted:{}", ids.len()),
                            }
                        }
                    },
                ),
            )
            .layer(middleware::from_fn_with_state(auth_state, require_api_key))
    }

    #[tokio::test]
    async fn auth_not_required_credentialless_request_passes_as_operator() {
        let pool = test_db();
        seed_admin(&pool, "admin"); // resolve_caller needs a first_admin to resolve to
        enable_enforcement(&pool);
        let mut state = test_auth_state(&pool);
        state.required = false;

        let resp = operator_probe_app(state, pool.clone())
            .oneshot(Request::builder().uri("/probe").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            bytes.as_ref(),
            b"unrestricted",
            "with [auth] required=false, an anonymous request must carry the operator bypass"
        );
    }

    #[tokio::test]
    async fn auth_required_default_credentialless_request_still_401s() {
        let pool = test_db();
        let resp = echo_app(test_auth_state(&pool)) // required: true
            .oneshot(Request::builder().uri("/echo").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // THE critical negative: optional auth must never mask a bad credential.
    // A client that DOES send a token is asking to be authenticated; if that
    // token is garbage the request fails loudly instead of silently running
    // with anonymous operator powers.
    #[tokio::test]
    async fn auth_not_required_presented_invalid_tokens_still_401() {
        let pool = test_db();
        let mut state = test_auth_state(&pool);
        state.required = false;

        for bad in [
            "lific_sk-garbage",          // malformed API key
            "lific_sess_expiredorfake",  // unknown session
            "lific_at_neverissued",      // unknown OAuth token
        ] {
            let resp = echo_app(state.clone())
                .oneshot(
                    Request::builder()
                        .uri("/echo")
                        .header("authorization", format!("Bearer {bad}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "presented-but-invalid credential '{bad}' must 401 even with auth optional"
            );
        }
    }

    // A real credential presented while auth is optional authenticates
    // normally — identity resolution is unchanged.
    #[tokio::test]
    async fn auth_not_required_valid_session_still_resolves_user() {
        let pool = test_db();
        let (token, user_id) = {
            let conn = pool.write().unwrap();
            let user = crate::db::queries::users::create_user(
                &conn,
                &crate::db::models::CreateUser {
                    username: "optionaluser".into(),
                    email: "optional@test.com".into(),
                    password: "testpassword1".into(),
                    display_name: None,
                    is_admin: false,
                    is_bot: false,
                },
            )
            .unwrap();
            let token = crate::db::queries::users::create_session(&conn, user.id, None)
                .unwrap()
                .token;
            (token, user.id)
        };
        let mut state = test_auth_state(&pool);
        state.required = false;

        let resp = echo_app(state)
            .oneshot(
                Request::builder()
                    .uri("/echo")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(bytes.as_ref(), format!("user:{user_id}:optionaluser:false").as_bytes());
    }

    #[tokio::test]
    async fn oauth_token_for_deleted_user_is_rejected() {
        let pool = test_db();
        let user_id = {
            let conn = pool.write().unwrap();
            let id = crate::db::queries::users::create_user(
                &conn,
                &crate::db::models::CreateUser {
                    username: "ghost".into(),
                    email: "ghost@test.com".into(),
                    password: "testpassword1".into(),
                    display_name: None,
                    is_admin: false,
                    is_bot: false,
                },
            )
            .unwrap()
            .id;
            // Simulate the user having since been deleted; oauth_tokens.user_id
            // has no FK constraint so this dangling reference is possible.
            conn.execute("DELETE FROM users WHERE id = ?1", params![id])
                .unwrap();
            id
        };
        let token = insert_oauth_token(&pool, "ghost", Some(user_id));

        let resp = echo_app(test_auth_state(&pool))
            .oneshot(
                Request::builder()
                    .uri("/echo")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // PR #23 review: a token bound to a user that no longer exists is a
        // dead credential, not an anonymous one. Anonymous would flow into
        // the first-admin fallback — deleting a bot must never escalate its
        // leftover token to operator.
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // ── LIF-131: api_keys.expires_at must be enforced at auth time ──────────
    //
    // The column existed (migration 003) and `lific key list` showed it, but
    // the auth path never checked it, so an expired key authenticated forever.
    // These drive the real `require_api_key` middleware: a 401 means the key
    // was refused, a 200 means it authenticated (body "none" = no bound user).

    /// Overwrite a key's expires_at directly (bypassing the CLI/date parsing)
    /// so enforcement can be exercised deterministically.
    fn set_key_expiry(pool: &db::DbPool, name: &str, expires_at: &str) {
        let conn = pool.write().unwrap();
        conn.execute(
            "UPDATE api_keys SET expires_at = ?1 WHERE name = ?2",
            params![expires_at, name],
        )
        .unwrap();
    }

    async fn auth_status(pool: &db::DbPool, key: &str) -> StatusCode {
        echo_app(test_auth_state(pool))
            .oneshot(
                Request::builder()
                    .uri("/echo")
                    .header("authorization", format!("Bearer {key}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn expired_key_id_lookup_is_rejected() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let key = create_api_key(&pool, &manager, "expired", None).unwrap();
        // Expire it well in the past.
        set_key_expiry(&pool, "expired", "2000-01-01T00:00:00Z");

        assert_eq!(
            auth_status(&pool, &key).await,
            StatusCode::UNAUTHORIZED,
            "an expired key must not authenticate (key_id lookup path)"
        );
    }

    #[tokio::test]
    async fn unexpired_key_authenticates() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let key = create_api_key(&pool, &manager, "future", None).unwrap();
        // Far-future expiry: still valid.
        set_key_expiry(&pool, "future", "2999-12-31T23:59:59Z");

        assert_eq!(
            auth_status(&pool, &key).await,
            StatusCode::OK,
            "a key with a future expiry must still authenticate"
        );
    }

    #[tokio::test]
    async fn null_expiry_authenticates() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        // Default create leaves expires_at NULL — the never-expires case.
        let key = create_api_key(&pool, &manager, "forever", None).unwrap();

        assert_eq!(
            auth_status(&pool, &key).await,
            StatusCode::OK,
            "a NULL expires_at means the key never expires (unchanged behavior)"
        );
    }

    #[tokio::test]
    async fn expired_legacy_key_without_key_id_is_rejected() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let key = create_api_key(&pool, &manager, "legacy-expired", None).unwrap();
        // Simulate a pre-migration key (NULL key_id) that has also expired,
        // exercising the fallback scan path.
        {
            let conn = pool.write().unwrap();
            conn.execute(
                "UPDATE api_keys SET key_id = NULL, expires_at = '2000-01-01T00:00:00Z' \
                 WHERE name = 'legacy-expired'",
                [],
            )
            .unwrap();
        }

        assert_eq!(
            auth_status(&pool, &key).await,
            StatusCode::UNAUTHORIZED,
            "an expired legacy key must not authenticate (NULL key_id scan path)"
        );
    }

    #[test]
    fn create_api_key_with_expiry_writes_column() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        create_api_key_with_expiry(&pool, &manager, "dated", Some("2030-06-01"), None).unwrap();

        let conn = pool.read().unwrap();
        let stored: Option<String> = conn
            .query_row(
                "SELECT expires_at FROM api_keys WHERE name = 'dated'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored.as_deref(), Some("2030-06-01"));
    }

    // ── LIF-261 / LIFIC-7: operator-key trust rule, end-to-end through the middleware ──
    //
    // resolve_caller maps any credential that authenticates but resolves no
    // user (unbound API key, legacy unbound OAuth token, "auth off" request)
    // to the first admin — so an unbound key passes the gate via
    // `identity.user.is_admin`. These drive a real route that runs
    // `authz::require_role(.., Viewer)` in enforced mode behind the real
    // `require_api_key`: a 200 means the gate passed, a 403 means it denied.

    fn enable_enforcement(pool: &db::DbPool) {
        let conn = pool.write().unwrap();
        crate::db::queries::settings::update(
            &conn,
            crate::db::queries::settings::InstanceSettingsPatch {
                authz_enforced: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    }

    fn seed_project_id(pool: &db::DbPool, ident: &str) -> i64 {
        let conn = pool.write().unwrap();
        crate::db::queries::create_project(
            &conn,
            &crate::db::models::CreateProject {
                name: format!("Project {ident}"),
                identifier: ident.into(),
                description: String::new(),
                emoji: None,
                lead_user_id: None,
            },
        )
        .unwrap()
        .id
    }

    /// A route that Viewer-gates a fixed project via `authz::require_role`
    /// behind the real `require_api_key`. 200 = allowed, 403 = Forbidden.
    fn gate_app(auth_state: AuthState, pool: db::DbPool, project_id: i64) -> Router {
        async fn gate(
            State((pool, project_id)): State<(db::DbPool, i64)>,
            Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
        ) -> Result<String, crate::error::LificError> {
            crate::authz::require_role(
                &pool,
                &identity,
                project_id,
                crate::db::models::Role::Viewer,
            )?;
            Ok("allowed".into())
        }
        Router::new()
            .route("/gate", get(gate))
            .with_state((pool, project_id))
            .layer(middleware::from_fn_with_state(auth_state, require_api_key))
    }

    async fn gate_status(app: Router, key: &str) -> StatusCode {
        app.oneshot(
            Request::builder()
                .uri("/gate")
                .header("authorization", format!("Bearer {key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
    }

    #[tokio::test]
    async fn enforced_operator_unbound_key_passes_viewer_gate_via_middleware() {
        let pool = test_db();
        seed_admin(&pool, "admin"); // resolve_caller needs a first_admin to resolve to
        let manager = create_key_manager().unwrap();
        let key = create_api_key(&pool, &manager, "operator", None).unwrap(); // unbound
        let project = seed_project_id(&pool, "OPM");
        enable_enforcement(&pool);

        let app = gate_app(test_auth_state(&pool), pool.clone(), project);
        assert_eq!(
            gate_status(app, &key).await,
            StatusCode::OK,
            "an unbound (operator) API key must pass the Viewer gate in enforced mode"
        );
    }

    // THE test: a legacy pre-binding OAuth token also resolves to None, but it
    // is NOT an operator credential — it must stay Forbidden even in the exact
    // same enforced-mode Viewer gate the operator key just passed.
    #[tokio::test]
    async fn enforced_legacy_unbound_oauth_token_is_forbidden_via_middleware() {
        let pool = test_db();
        let project = seed_project_id(&pool, "OAM");
        enable_enforcement(&pool);
        // Unbound OAuth token (user_id = None) — the LIF-204 legacy case.
        // No admin is seeded, so resolve_caller returns None and the enforced
        // gate default-denies. (With an admin present, resolve_caller would
        // resolve it to first_admin — the operator bypass is "the first admin
        // is trusted," not credential-type-specific.)
        let token = insert_oauth_token(&pool, "legacy-unbound", None);

        let app = gate_app(test_auth_state(&pool), pool.clone(), project);
        assert_eq!(
            gate_status(app, &token).await,
            StatusCode::FORBIDDEN,
            "with no admin to resolve to, a credential-less request stays default-denied"
        );
    }

    // A key bound to a real (non-member) user is NOT an operator: even though
    // it isn't None, it must be denied in enforced mode (no membership row),
    // proving the operator bypass keys off the unbound binding, not the key
    // type in general.
    #[tokio::test]
    async fn enforced_user_bound_key_nonmember_is_forbidden_via_middleware() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let key = {
            // A key bound to a fresh non-admin user at creation.
            let uid = {
                let conn = pool.write().unwrap();
                crate::db::queries::users::create_user(
                    &conn,
                    &crate::db::models::CreateUser {
                        username: "bounduser".into(),
                        email: "bound@test.local".into(),
                        password: "testpassword1".into(),
                        display_name: None,
                        is_admin: false,
                        is_bot: false,
                    },
                )
                .unwrap()
                .id
            };
            create_api_key(&pool, &manager, "bound", Some(uid)).unwrap()
        };
        let project = seed_project_id(&pool, "BNM");
        enable_enforcement(&pool);

        let app = gate_app(test_auth_state(&pool), pool.clone(), project);
        assert_eq!(
            gate_status(app, &key).await,
            StatusCode::FORBIDDEN,
            "a user-bound key for a non-member must be denied — it is not an operator credential"
        );
    }

    // ── LIF-267: attachment-download matcher + session-cookie parsing ───────

    #[test]
    fn is_attachment_download_matches_numeric_id_get() {
        assert!(is_attachment_download(
            &Method::GET,
            "/api/attachments/5"
        ));
        assert!(is_attachment_download(
            &Method::GET,
            "/api/attachments/12345"
        ));
    }

    #[test]
    fn is_attachment_download_tolerates_trailing_slash() {
        assert!(is_attachment_download(
            &Method::GET,
            "/api/attachments/7/"
        ));
    }

    #[test]
    fn is_attachment_download_excludes_list_route() {
        // The list route (no id) must stay header-only.
        assert!(!is_attachment_download(&Method::GET, "/api/attachments"));
        assert!(!is_attachment_download(&Method::GET, "/api/attachments/"));
    }

    #[test]
    fn is_attachment_download_excludes_non_numeric_and_deeper_paths() {
        assert!(!is_attachment_download(
            &Method::GET,
            "/api/attachments/abc"
        ));
        assert!(!is_attachment_download(
            &Method::GET,
            "/api/attachments/5/extra"
        ));
        assert!(!is_attachment_download(
            &Method::GET,
            "/api/attachments/5x"
        ));
    }

    /// LIF-418: a thumbnail is loaded by an `<img>` exactly like the full
    /// image, so it needs the same cookie fallback. The other two derived
    /// routes are XHR-only and must not get it.
    #[test]
    fn is_attachment_download_includes_the_thumbnail_variant() {
        assert!(is_attachment_download(
            &Method::GET,
            "/api/attachments/5/thumbnail"
        ));
        assert!(is_attachment_download(
            &Method::GET,
            "/api/attachments/5/thumbnail/"
        ));
        assert!(!is_attachment_download(
            &Method::GET,
            "/api/attachments/5/links"
        ));
        assert!(!is_attachment_download(
            &Method::GET,
            "/api/attachments/5/preview"
        ));
        assert!(!is_attachment_download(
            &Method::GET,
            "/api/attachments/abc/thumbnail"
        ));
        assert!(!is_attachment_download(
            &Method::POST,
            "/api/attachments/5/thumbnail"
        ));
    }

    #[test]
    fn is_attachment_download_excludes_non_get_methods() {
        assert!(!is_attachment_download(
            &Method::DELETE,
            "/api/attachments/5"
        ));
        assert!(!is_attachment_download(
            &Method::POST,
            "/api/attachments/5"
        ));
    }

    #[test]
    fn session_cookie_token_extracts_only_session_prefix() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "cookie",
            "foo=bar; lific_token=lific_sess_abc123; baz=qux".parse().unwrap(),
        );
        assert_eq!(
            session_cookie_token(&headers).as_deref(),
            Some("lific_sess_abc123")
        );
    }

    #[test]
    fn session_cookie_token_rejects_non_session_values() {
        // An API key or OAuth token in the cookie is never accepted.
        for value in ["lific_sk-live-xxx", "lific_at_xxx", "garbage"] {
            let mut headers = HeaderMap::new();
            headers.insert("cookie", format!("lific_token={value}").parse().unwrap());
            assert_eq!(
                session_cookie_token(&headers),
                None,
                "non-session cookie value must be rejected: {value}"
            );
        }
    }

    #[test]
    fn session_cookie_token_none_when_absent() {
        let headers = HeaderMap::new();
        assert_eq!(session_cookie_token(&headers), None);
        let mut headers = HeaderMap::new();
        headers.insert("cookie", "other=1; another=2".parse().unwrap());
        assert_eq!(session_cookie_token(&headers), None);
    }

    // ── LIFIC-8: middleware inserts ResolvedIdentity alongside Option<AuthUser> ──
    //
    // `require_api_key` now also inserts `Extension<ResolvedIdentity>` (when a
    // user can be resolved) at every success branch. These drive the real
    // middleware through a handler that echoes the identity back, asserting
    // the resolved user AND the transport for each credential type — including
    // the first-admin passwordless fallback for the credential-less and
    // unbound-credential paths.

    fn seed_admin(pool: &db::DbPool, username: &str) -> i64 {
        let conn = pool.write().unwrap();
        let u = crate::db::queries::users::create_user(
            &conn,
            &crate::db::models::CreateUser {
                username: username.into(),
                email: format!("{username}@local.test"),
                password: "adminpass123".into(),
                display_name: Some(format!("Admin {username}")),
                is_admin: true,
                is_bot: false,
            },
        )
        .unwrap();
        u.id
    }

    /// Echo handler that reports the `ResolvedIdentity` the middleware
    /// resolved, mirroring `echo_app` but for the new identity type.
    fn identity_echo_app(auth_state: AuthState) -> Router {
        async fn echo(
            Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
        ) -> String {
            match identity {
                Some(id) => format!(
                    "id:{}:{}:{}:{}",
                    id.user.id,
                    id.user.username,
                    id.user.is_admin,
                    id.transport.as_str()
                ),
                None => "none".to_string(),
            }
        }
        Router::new()
            .route("/echo", get(echo))
            .layer(middleware::from_fn_with_state(auth_state, require_api_key))
    }

    async fn identity_body(app: Router, auth: Option<&str>) -> String {
        let mut req = Request::builder().uri("/echo");
        if let Some(token) = auth {
            req = req.header("authorization", format!("Bearer {token}"));
        }
        let resp = app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.as_ref().to_vec()).unwrap()
    }

    #[tokio::test]
    async fn resolved_identity_session_token_is_the_user_on_web_transport() {
        let pool = test_db();
        let (token, user_id) = {
            let conn = pool.write().unwrap();
            let u = crate::db::queries::users::create_user(
                &conn,
                &crate::db::models::CreateUser {
                    username: "sessuser".into(),
                    email: "sessuser@test.com".into(),
                    password: "testpassword1".into(),
                    display_name: None,
                    is_admin: false,
                    is_bot: false,
                },
            )
            .unwrap();
            let token = crate::db::queries::users::create_session(&conn, u.id, None)
                .unwrap()
                .token;
            (token, u.id)
        };

        let body = identity_body(identity_echo_app(test_auth_state(&pool)), Some(&token)).await;
        assert_eq!(
            body,
            format!("id:{user_id}:sessuser:false:web"),
            "session token must resolve to its user on the web transport"
        );
    }

    #[tokio::test]
    async fn resolved_identity_bound_api_key_resolves_to_bound_user() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let user_id = {
            let conn = pool.write().unwrap();
            crate::db::queries::users::create_user(
                &conn,
                &crate::db::models::CreateUser {
                    username: "keyuser".into(),
                    email: "keyuser@test.com".into(),
                    password: "testpassword1".into(),
                    display_name: None,
                    is_admin: false,
                    is_bot: false,
                },
            )
            .unwrap()
            .id
        };
        let key = create_api_key(&pool, &manager, "bound", Some(user_id)).unwrap();

        let body = identity_body(identity_echo_app(test_auth_state(&pool)), Some(&key)).await;
        assert_eq!(
            body,
            format!("id:{user_id}:keyuser:false:api"),
            "a user-bound API key must resolve to that user on the api transport"
        );
    }

    #[tokio::test]
    async fn resolved_identity_bound_oauth_token_resolves_to_bound_user() {
        let pool = test_db();
        let user_id = {
            let conn = pool.write().unwrap();
            crate::db::queries::users::create_user(
                &conn,
                &crate::db::models::CreateUser {
                    username: "oauthuser".into(),
                    email: "oauthuser@test.com".into(),
                    password: "testpassword1".into(),
                    display_name: None,
                    is_admin: false,
                    is_bot: false,
                },
            )
            .unwrap()
            .id
        };
        let token = insert_oauth_token(&pool, "bound-oauth", Some(user_id));

        let body = identity_body(identity_echo_app(test_auth_state(&pool)), Some(&token)).await;
        assert_eq!(
            body,
            format!("id:{user_id}:oauthuser:false:api"),
            "a user-bound OAuth token must resolve to that user on the api transport"
        );
    }

    // The passwordless fallback: a legacy unbound OAuth token carries no user,
    // so resolve_caller falls back to the first admin. The legacy
    // Option<AuthUser> stays None (proven by the existing middleware tests);
    // only the new ResolvedIdentity sees the fallback user.
    #[tokio::test]
    async fn resolved_identity_legacy_unbound_oauth_falls_back_to_first_admin() {
        let pool = test_db();
        let admin_id = seed_admin(&pool, "admin");
        let token = insert_oauth_token(&pool, "legacy-unbound-id", None);

        let body = identity_body(identity_echo_app(test_auth_state(&pool)), Some(&token)).await;
        assert_eq!(
            body,
            format!("id:{admin_id}:admin:true:api"),
            "a legacy unbound OAuth token must resolve to the first admin via the fallback"
        );
    }

    // An operator-trusted unbound API key likewise resolves to the first admin
    // in the new identity (its admin-ness comes from first_admin, not a
    // separate operator flag).
    #[tokio::test]
    async fn resolved_identity_unbound_api_key_falls_back_to_first_admin() {
        let pool = test_db();
        let admin_id = seed_admin(&pool, "admin");
        let manager = create_key_manager().unwrap();
        let key = create_api_key(&pool, &manager, "operator", None).unwrap(); // unbound

        let body = identity_body(identity_echo_app(test_auth_state(&pool)), Some(&key)).await;
        assert_eq!(
            body,
            format!("id:{admin_id}:admin:true:api"),
            "an unbound (operator) API key must resolve to the first admin"
        );
    }

    // Auth-off: a credential-less request is the passwordless case par
    // excellence — resolve_caller supplies the first admin so identity is
    // always known even with no credential presented.
    #[tokio::test]
    async fn resolved_identity_auth_off_credentialless_falls_back_to_first_admin() {
        let pool = test_db();
        let admin_id = seed_admin(&pool, "admin");
        let mut state = test_auth_state(&pool);
        state.required = false;

        let body = identity_body(identity_echo_app(state), None).await;
        assert_eq!(
            body,
            format!("id:{admin_id}:admin:true:api"),
            "auth-off credential-less request must resolve to the first admin"
        );
    }

    // The degenerate case: no credential AND no admin exists → no
    // ResolvedIdentity is inserted. The legacy Option<AuthUser> path is
    // unchanged (the request still passes as operator-equivalent).
    #[tokio::test]
    async fn resolved_identity_auth_off_zero_users_inserts_no_identity() {
        let pool = test_db(); // no users at all
        let mut state = test_auth_state(&pool);
        state.required = false;

        let body = identity_body(identity_echo_app(state), None).await;
        assert_eq!(body, "none", "zero-user bootstrap inserts no identity");
    }

    // ── LIFIC-18: stdio token resolution (auth::resolve_stdio_token) ───────

    #[test]
    fn resolve_api_key_user_bound_key_returns_user() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let uid = {
            let conn = pool.write().unwrap();
            crate::db::queries::users::create_user(
                &conn,
                &crate::db::models::CreateUser {
                    username: "agent-user".into(),
                    email: "agent-user@local.test".into(),
                    password: "testpassword1".into(),
                    display_name: None,
                    is_admin: false,
                    is_bot: true,
                },
            )
            .unwrap()
            .id
        };
        let key = create_api_key(&pool, &manager, "opencode-agent", Some(uid)).unwrap();
        let resolved = resolve_api_key_user(&pool, &manager, &key).unwrap();
        let user = resolved.expect("bound key resolves to a user");
        assert_eq!(user.id, uid);
        assert_eq!(user.username, "agent-user");
    }

    #[test]
    fn resolve_api_key_user_unbound_key_is_ok_none() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let key = create_api_key(&pool, &manager, "unbound", None).unwrap();
        let resolved = resolve_api_key_user(&pool, &manager, &key).unwrap();
        assert!(
            resolved.is_none(),
            "a valid-but-unbound key must be Ok(None) — operator fallback"
        );
    }

    #[test]
    fn resolve_api_key_user_invalid_key_is_err() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let err = resolve_api_key_user(&pool, &manager, "lific_sk-live-NOTAREALKEY")
            .expect_err("a bogus key must be an error");
        assert!(!err.is_empty());
    }

    #[test]
    fn resolve_stdio_token_without_env_is_ok_none() {
        // No LIFIC_TOKEN in the environment → Ok(None): the operator fallback.
        let _guard = ENV_LOCK.lock().unwrap();
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        // SAFETY: guarded by ENV_LOCK.
        unsafe { std::env::remove_var("LIFIC_TOKEN") };
        let resolved = resolve_stdio_token(&pool, &manager).unwrap();
        assert!(resolved.is_none(), "absent token must be Ok(None)");
    }

    #[test]
    fn resolve_stdio_token_valid_env_resolves_bound_user() {
        let _guard = ENV_LOCK.lock().unwrap();
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let uid = {
            let conn = pool.write().unwrap();
            crate::db::queries::users::create_user(
                &conn,
                &crate::db::models::CreateUser {
                    username: "stdio-agent".into(),
                    email: "stdio-agent@local.test".into(),
                    password: "testpassword1".into(),
                    display_name: None,
                    is_admin: false,
                    is_bot: true,
                },
            )
            .unwrap()
            .id
        };
        let key = create_api_key(&pool, &manager, "codex-agent", Some(uid)).unwrap();
        // SAFETY: guarded by ENV_LOCK; restored every path below.
        unsafe { std::env::set_var("LIFIC_TOKEN", &key) };
        let resolved = resolve_stdio_token(&pool, &manager).unwrap();
        unsafe { std::env::remove_var("LIFIC_TOKEN") };
        assert_eq!(resolved.unwrap().id, uid);
    }

    // ── LIF-403: OAuth tokens are barred from credential management ────────
    //
    // An OAuth token stays a first-class REST credential (parity with what it
    // can do through MCP), but it may not touch the routes that mint, list or
    // revoke credentials, nor account/user administration. Enforcement lives
    // in this middleware, so these drive the REAL `api::router` behind the
    // REAL `require_api_key` — the escalation being closed (POST
    // /api/auth/keys minting a permanent key from an expiring token) is only
    // meaningful against the actual route.

    /// `api::router` with the extensions `server.rs` supplies, behind the
    /// real auth middleware.
    fn real_api_app(pool: &db::DbPool) -> Router {
        let state = test_auth_state(pool);
        let manager = std::sync::Arc::new(state.manager.clone());
        crate::api::router(pool.clone(), &[])
            .layer(Extension(manager))
            .layer(Extension(crate::realtime::RealtimeHub::new()))
            .layer(Extension(crate::config::AuthConfig {
                allow_signup: true,
                required: true,
                secure_cookies: false,
            }))
            .layer(middleware::from_fn_with_state(state, require_api_key))
    }

    async fn send(
        app: Router,
        method: &str,
        uri: &str,
        body: Option<serde_json::Value>,
        token: &str,
    ) -> (StatusCode, String) {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Bearer {token}"));
        let body = match body {
            Some(json) => {
                builder = builder.header("content-type", "application/json");
                Body::from(serde_json::to_vec(&json).unwrap())
            }
            None => Body::empty(),
        };
        let resp = app.oneshot(builder.body(body).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    fn seed_user(pool: &db::DbPool, username: &str) -> i64 {
        let conn = pool.write().unwrap();
        crate::db::queries::users::create_user(
            &conn,
            &crate::db::models::CreateUser {
                username: username.into(),
                email: format!("{username}@test.local"),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap()
        .id
    }

    // THE test: the LIF-403 escalation. An expiring, revocable OAuth token
    // must not be able to trade itself for a permanent API key.
    #[tokio::test]
    async fn oauth_token_cannot_mint_an_api_key() {
        let pool = test_db();
        let uid = seed_user(&pool, "toolowner");
        let token = insert_oauth_token(&pool, "mint", Some(uid));

        let (status, body) = send(
            real_api_app(&pool),
            "POST",
            "/api/auth/keys",
            Some(serde_json::json!({ "name": "stolen" })),
            &token,
        )
        .await;

        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "an OAuth token must not reach the key-mint route: {body}"
        );
        assert!(
            body.contains("API key management"),
            "the 403 must say why: {body}"
        );
        assert!(
            list_api_keys(&pool).unwrap().is_empty(),
            "no key may have been created"
        );
    }

    // The rest of the credential-management and account-administration
    // surface, through the real router.
    #[tokio::test]
    async fn oauth_token_is_refused_on_every_credential_management_route() {
        let pool = test_db();
        let uid = seed_user(&pool, "toolowner2");
        let token = insert_oauth_token(&pool, "surface", Some(uid));

        let cases: [(&str, &str, Option<serde_json::Value>); 9] = [
            ("GET", "/api/auth/keys", None),
            ("DELETE", "/api/auth/keys/1", None),
            ("GET", "/api/auth/bots", None),
            ("POST", "/api/auth/bots", Some(serde_json::json!({}))),
            ("POST", "/api/auth/bots/1/disconnect", None),
            ("DELETE", "/api/auth/bots/1", None),
            (
                "POST",
                "/api/auth/me/password",
                Some(serde_json::json!({})),
            ),
            ("DELETE", "/api/auth/me/sessions", None),
            ("POST", "/api/users/1/promote", None),
        ];

        for (method, uri, body) in cases {
            let (status, resp) = send(real_api_app(&pool), method, uri, body, &token).await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{method} {uri} must be closed to an OAuth token: {resp}"
            );
        }
    }

    // The other half of the design decision: OAuth tokens keep working on
    // ordinary REST routes. A regression here would be a functional break for
    // every connector that reads through REST.
    #[tokio::test]
    async fn oauth_token_still_works_on_ordinary_rest_routes() {
        let pool = test_db();
        let uid = seed_user(&pool, "reader");
        let token = insert_oauth_token(&pool, "reads", Some(uid));

        for uri in ["/api/projects", "/api/issues", "/api/users", "/api/auth/me"] {
            let (status, body) = send(real_api_app(&pool), "GET", uri, None, &token).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "GET {uri} must stay open to an OAuth token: {body}"
            );
        }
    }

    // An API key authenticates ordinary REST routes but may no longer mint a
    // second credential. Previously it could, which made one leaked key a key
    // factory: the attacker minted a spare, and the account lockdown the
    // victim then ran revoked a credential the attacker had already replaced.
    // Minting now belongs to a browser session the human authenticated
    // recently, and nothing else.
    #[tokio::test]
    async fn api_key_credential_may_no_longer_mint_a_key() {
        let pool = test_db();
        let uid = seed_user(&pool, "keyholder");
        let manager = create_key_manager().unwrap();
        let key = create_api_key(&pool, &manager, "existing", Some(uid)).unwrap();

        let (status, body) = send(real_api_app(&pool), "GET", "/api/auth/me", None, &key).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "the key still authenticates: {body}"
        );

        let (status, body) = send(
            real_api_app(&pool),
            "POST",
            "/api/auth/keys",
            Some(serde_json::json!({ "name": "fresh" })),
            &key,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(!body.contains("lific_sk"), "no key was returned: {body}");
    }

    #[tokio::test]
    async fn session_credential_can_still_mint_a_key() {
        let pool = test_db();
        let uid = seed_user(&pool, "browseruser");
        let session = {
            let conn = pool.write().unwrap();
            crate::db::queries::users::create_session(&conn, uid, None)
                .unwrap()
                .token
        };

        let (status, body) = send(
            real_api_app(&pool),
            "POST",
            "/api/auth/keys",
            Some(serde_json::json!({ "name": "from-browser" })),
            &session,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "session path unaffected: {body}");
        assert!(body.contains("lific_sk"), "a key was returned: {body}");
    }

    // /mcp is untouched: the deny-list is a REST-route list, and the MCP
    // endpoint is what OAuth tokens exist for.
    #[tokio::test]
    async fn mcp_endpoint_is_unaffected_by_the_oauth_deny_list() {
        let pool = test_db();
        let uid = seed_user(&pool, "mcpbot");
        let token = insert_oauth_token(&pool, "mcp", Some(uid));

        let app = Router::new()
            .route("/mcp", axum::routing::post(|| async { "mcp-ok" }))
            .layer(middleware::from_fn_with_state(
                test_auth_state(&pool),
                require_api_key,
            ));

        let (status, body) = send(app, "POST", "/mcp", None, &token).await;
        assert_eq!(status, StatusCode::OK, "/mcp must still accept the token");
        assert_eq!(body, "mcp-ok");
    }

    // The audit transport (`ActorCtx`) and the resolved identity's transport
    // must agree for an OAuth token, on both doors. They are computed in two
    // places — the `ActorCtx` literal in the OAuth branch, and
    // `insert_identity_extensions`, which applies the same `/mcp` conditional
    // to the `default` it is handed — so this pins them together.
    #[tokio::test]
    async fn oauth_transport_matches_between_actor_and_resolved_identity() {
        let pool = test_db();
        let uid = seed_user(&pool, "transportuser");
        let token = insert_oauth_token(&pool, "transport", Some(uid));

        async fn probe(
            Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
        ) -> String {
            let actor = crate::actor::current();
            format!(
                "{}|{}",
                actor.transport.as_str(),
                identity
                    .map(|i| i.transport.as_str().to_string())
                    .unwrap_or_else(|| "none".into())
            )
        }

        for (uri, expected) in [("/mcp", "mcp|mcp"), ("/echo", "api|api")] {
            let app = Router::new()
                .route("/mcp", axum::routing::post(probe))
                .route("/echo", axum::routing::post(probe))
                .layer(middleware::from_fn_with_state(
                    test_auth_state(&pool),
                    require_api_key,
                ));
            let (status, body) = send(app, "POST", uri, None, &token).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(
                body, expected,
                "{uri}: audit transport and resolved-identity transport must agree"
            );
        }
    }

    // ── The deny-list itself ───────────────────────────────────────────────

    #[test]
    fn oauth_blocked_reason_covers_the_credential_surface() {
        for (method, path) in [
            (Method::GET, "/api/auth/keys"),
            (Method::POST, "/api/auth/keys"),
            (Method::POST, "/api/auth/keys/"),
            (Method::DELETE, "/api/auth/keys/42"),
            (Method::GET, "/api/auth/bots"),
            (Method::POST, "/api/auth/bots"),
            (Method::POST, "/api/auth/bots/7/disconnect"),
            (Method::DELETE, "/api/auth/bots/7"),
            (Method::POST, "/api/auth/me/password"),
            (Method::DELETE, "/api/auth/me/sessions"),
            (Method::POST, "/api/auth/me/refresh"),
            (Method::PATCH, "/api/auth/me"),
            (Method::POST, "/api/users"),
            (Method::POST, "/api/users/3/promote"),
            (Method::POST, "/api/users/3/demote"),
            (Method::POST, "/api/users/3/deactivate"),
            (Method::POST, "/api/users/3/reactivate"),
            (Method::POST, "/oauth/token"),
            (Method::POST, "/oauth/register"),
        ] {
            assert!(
                oauth_blocked_reason(&method, path).is_some(),
                "{method} {path} must be closed to OAuth tokens"
            );
        }
    }

    #[test]
    fn oauth_blocked_reason_leaves_ordinary_routes_open() {
        for (method, path) in [
            (Method::GET, "/api/auth/me"),
            (Method::GET, "/api/users"),
            (Method::GET, "/api/projects"),
            (Method::POST, "/api/projects"),
            (Method::POST, "/api/issues"),
            (Method::PUT, "/api/issues/5"),
            (Method::GET, "/api/search"),
            (Method::POST, "/mcp"),
            (Method::GET, "/api/health"),
            // Near-misses that must not be caught by the prefix rules.
            (Method::GET, "/api/auth/keysomething"),
            (Method::GET, "/api/usersomething"),
        ] {
            assert_eq!(
                oauth_blocked_reason(&method, path),
                None,
                "{method} {path} must stay open to OAuth tokens"
            );
        }
    }
}
