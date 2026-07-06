use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, Method, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use rusqlite::params;
use tracing::{info, warn};

use api_keys_simplified::{ApiKeyManagerV0, Environment, ExposeSecret, KeyStatus, SecureString};

use crate::db::DbPool;

#[derive(Clone)]
pub struct AuthState {
    pub db: DbPool,
    pub manager: ApiKeyManagerV0,
    pub public_url: String,
    /// LIF-294: mirror of `[auth] required`. When false, a request with no
    /// credential at all passes as operator-equivalent; see `require_api_key`.
    pub required: bool,
}

/// Create the API key manager with our prefix.
pub fn create_key_manager() -> Result<ApiKeyManagerV0, String> {
    ApiKeyManagerV0::init_default_config("lific_sk")
        .map_err(|e| format!("failed to init key manager: {e}"))
}

/// Generate a new API key, store the hash, return the plaintext (shown once).
pub fn create_api_key(
    db: &DbPool,
    manager: &ApiKeyManagerV0,
    name: &str,
) -> Result<String, crate::error::LificError> {
    create_api_key_with_expiry(db, manager, name, None)
}

/// Like [`create_api_key`] but writes an optional `expires_at` (ISO 8601). Once
/// past, the auth path (LIF-131) refuses the key. `None` means never expires.
pub fn create_api_key_with_expiry(
    db: &DbPool,
    manager: &ApiKeyManagerV0,
    name: &str,
    expires_at: Option<&str>,
) -> Result<String, crate::error::LificError> {
    let conn = db.write()?;

    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM api_keys WHERE name = ?1 AND revoked = 0",
            params![name],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if exists {
        return Err(crate::error::LificError::BadRequest(format!(
            "an active key named '{name}' already exists"
        )));
    }

    let api_key = manager
        .generate(Environment::production())
        .map_err(|e| crate::error::LificError::Internal(format!("key generation failed: {e}")))?;

    let plaintext = api_key.key().expose_secret().to_string();
    let hash = api_key.expose_hash().hash().to_string();
    let key_id = api_key.expose_hash().key_id().to_string();

    conn.execute(
        "INSERT INTO api_keys (name, key_hash, key_id, expires_at) VALUES (?1, ?2, ?3, ?4)",
        params![name, hash, key_id, expires_at],
    )?;

    Ok(plaintext)
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
    // Capture the user binding before deleting so it can be re-applied.
    // If multiple rows share the name (revoked leftovers), prefer the
    // binding of an active row.
    let conn = db.write()?;
    let user_id: Option<i64> = conn
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

    // Delete old key entirely (not just revoke) so the name can be reused
    conn.execute("DELETE FROM api_keys WHERE name = ?1", params![name])?;
    drop(conn);

    let plaintext = create_api_key(db, manager, name)?;

    if let Some(uid) = user_id {
        let conn = db.write()?;
        crate::db::queries::users::assign_key_to_user(&conn, name, uid)?;
    }

    Ok(plaintext)
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
fn is_attachment_download(method: &Method, path: &str) -> bool {
    if method != Method::GET {
        return false;
    }
    let Some(rest) = path.strip_prefix("/api/attachments/") else {
        return false;
    };
    let id = rest.strip_suffix('/').unwrap_or(rest);
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())
}

/// Axum middleware that validates Bearer tokens and resolves user identity.
///
/// After successful auth, inserts `Extension<Option<AuthUser>>` into the request:
/// - `Some(user)` if the token resolves to a user (session, or API key with user_id)
/// - `None` if the token is valid but has no user association (legacy keys, OAuth)
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
                let conn = match auth.db.write() {
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
            let actor = crate::actor::ActorCtx {
                user_id: None,
                transport: if is_mcp_request {
                    crate::actor::Transport::Mcp
                } else {
                    crate::actor::Transport::Api
                },
            };
            request
                .extensions_mut()
                .insert(Option::<crate::db::models::AuthUser>::None);
            request.extensions_mut().insert(OperatorCredential);
            return crate::actor::scope(
                actor,
                crate::authz::operator_scope(true, next.run(request)),
            )
            .await;
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
            let conn = match auth.db.write() {
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
        if crate::oauth::validate_oauth_token(&auth.db, &token) {
            if is_mcp_request {
                info!("/mcp authorized: OAuth token accepted");
            }
            // Resolve the user bound to this token at approval time (LIF-79).
            // Tokens issued before user binding existed have no user_id and
            // stay anonymous (None), preserving the previous behavior.
            let auth_user = crate::oauth::oauth_token_user_id(&auth.db, &token)
                .and_then(|uid| {
                    let conn = auth.db.read().ok()?;
                    crate::db::queries::users::get_user_by_id(&conn, uid).ok()
                })
                .map(|u| crate::db::models::AuthUser {
                    id: u.id,
                    username: u.username,
                    display_name: u.display_name,
                    is_admin: u.is_admin,
                });
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
            request.extensions_mut().insert(auth_user);
            return crate::actor::scope(actor, next.run(request)).await;
        }
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

    // ── API keys (lific_sk- prefix) ──────────────────────────────
    let secure_token = SecureString::from(token);

    // Fast checksum pre-check: reject malformed keys in ~20μs without touching DB
    match auth.manager.verify_checksum(&secure_token) {
        Ok(true) => {} // valid checksum, proceed to DB lookup
        _ => {
            warn!("rejected API key with invalid checksum");
            return (
                StatusCode::UNAUTHORIZED,
                [("WWW-Authenticate", www_auth.as_str())],
                "Invalid API key",
            )
                .into_response();
        }
    }

    // Compute deterministic key ID (BLAKE3, ~microseconds) for O(1) DB lookup
    let key_id = auth.manager.extract_key_id(&secure_token);

    // Look up the single matching key by key_id (indexed query)
    let key_row: Option<ApiKeyRow> = {
        let conn = match auth.db.read() {
            Ok(c) => c,
            Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response(),
        };
        conn.query_row(
            "SELECT id, key_hash, user_id FROM api_keys WHERE key_id = ?1 AND revoked = 0 \
             AND (expires_at IS NULL OR expires_at > datetime('now'))",
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

    // Fallback: keys created before migration 010 have no key_id — scan those
    let key_row = key_row.or_else(|| {
        let conn = auth.db.read().ok()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, key_hash, user_id FROM api_keys WHERE key_id IS NULL AND revoked = 0 \
                 AND (expires_at IS NULL OR expires_at > datetime('now'))",
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
            if let Ok(KeyStatus::Valid) = auth.manager.verify(&secure_token, &row.hash) {
                // Backfill the key_id so future lookups are O(1)
                if let Ok(wconn) = auth.db.write() {
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
        warn!("rejected invalid API key");
        return (
            StatusCode::UNAUTHORIZED,
            [("WWW-Authenticate", www_auth.as_str())],
            "Invalid API key",
        )
            .into_response();
    };

    // Verify the key against the stored Argon2 hash
    match auth.manager.verify(&secure_token, &key.hash) {
        Ok(KeyStatus::Valid) => {
            // Resolve user if the key has a user_id
            let auth_user = key.user_id.and_then(|uid| {
                let conn = auth.db.read().ok()?;
                crate::db::queries::users::get_user_by_id(&conn, uid)
                    .ok()
                    .map(|u| crate::db::models::AuthUser {
                        id: u.id,
                        username: u.username,
                        display_name: u.display_name,
                        is_admin: u.is_admin,
                    })
            });
            // LIF-261: an API key with NO user binding is operator-trusted —
            // it can only be minted with shell access to the server, so it's
            // admin-equivalent in enforced mode. This is the ONE credential
            // path that sets the operator signal; OAuth/session tokens never
            // do, so a legacy unbound OAuth token (also `AuthUser = None`)
            // stays default-denied. Keyed off the DB binding, not the resolved
            // `auth_user`, so a key bound to a since-deleted user does NOT
            // silently become an operator.
            let is_operator = key.user_id.is_none();
            // LIF-155: API keys are programmatic — 'mcp' on the /mcp
            // path, 'api' for direct REST usage.
            let actor = crate::actor::ActorCtx {
                user_id: auth_user.as_ref().map(|u| u.id),
                transport: if is_mcp_request {
                    crate::actor::Transport::Mcp
                } else {
                    crate::actor::Transport::Api
                },
            };
            request.extensions_mut().insert(auth_user);
            // The /mcp route reads this marker to pass the operator flag into
            // `with_request_identity`; REST reads the task-local scoped below.
            if is_operator {
                request.extensions_mut().insert(OperatorCredential);
            }
            crate::actor::scope(
                actor,
                crate::authz::operator_scope(is_operator, next.run(request)),
            )
            .await
        }
        _ => {
            warn!("API key hash verification failed");
            (
                StatusCode::UNAUTHORIZED,
                [("WWW-Authenticate", www_auth.as_str())],
                "Invalid API key",
            )
                .into_response()
        }
    }
}

/// LIF-261: request-extension marker inserted by [`require_api_key`] when the
/// authenticated credential is an operator-trusted unbound API key. The `/mcp`
/// route reads it (via `request.extensions().get::<OperatorCredential>()`) to
/// forward the operator flag into `mcp::with_request_identity`. REST handlers
/// don't read it — they see the operator signal through the task-local scoped
/// by `authz::operator_scope` around the same request.
#[derive(Clone, Copy)]
pub struct OperatorCredential;

/// Internal struct for loading API key rows during auth.
#[derive(Debug)]
struct ApiKeyRow {
    #[allow(dead_code)]
    id: i64,
    hash: String,
    user_id: Option<i64>,
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

    #[test]
    fn create_key_returns_valid_format() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let key = create_api_key(&pool, &manager, "test-key").unwrap();
        assert!(key.starts_with("lific_sk-live-"));
    }

    #[test]
    fn verify_key_succeeds() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let key = create_api_key(&pool, &manager, "test-key").unwrap();

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
        create_api_key(&pool, &manager, "test-key").unwrap();

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
        create_api_key(&pool, &manager, "revoke-me").unwrap();

        revoke_api_key(&pool, "revoke-me").unwrap();

        let keys = list_api_keys(&pool).unwrap();
        assert!(keys[0].revoked);
    }

    #[test]
    fn rotate_key_replaces_old() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let old_key = create_api_key(&pool, &manager, "rotate-me").unwrap();
        let new_key = rotate_api_key(&pool, &manager, "rotate-me").unwrap();

        assert_ne!(old_key, new_key);
        assert!(new_key.starts_with("lific_sk-live-"));

        // Old key deleted, only new key remains
        let keys = list_api_keys(&pool).unwrap();
        assert_eq!(keys.len(), 1);
        assert!(!keys[0].revoked);
    }

    // LIF-132: rotation must carry the user binding over to the new key.
    // Previously the old row was deleted (user_id and all) and the new key
    // was created unbound, silently de-attributing bot/user keys.
    #[test]
    fn rotate_key_preserves_user_binding() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        create_api_key(&pool, &manager, "bot-key").unwrap();

        // Bind the key to a user.
        let user_id = {
            let conn = pool.write().unwrap();
            conn.execute(
                "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot)
                 VALUES ('bot', 'bot@test.local', 'x', 'Bot', 0, 1)",
                [],
            )
            .unwrap();
            let uid = conn.last_insert_rowid();
            crate::db::queries::users::assign_key_to_user(&conn, "bot-key", uid).unwrap();
            uid
        };

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
        create_api_key(&pool, &manager, "plain").unwrap();
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
        create_api_key(&pool, &manager, "unique").unwrap();
        let result = create_api_key(&pool, &manager, "unique");
        assert!(result.is_err());
    }

    #[test]
    fn has_any_keys_works() {
        let pool = test_db();
        assert!(!has_any_keys(&pool));

        let manager = create_key_manager().unwrap();
        create_api_key(&pool, &manager, "first").unwrap();
        assert!(has_any_keys(&pool));
    }

    #[test]
    fn create_key_stores_key_id() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let key = create_api_key(&pool, &manager, "id-test").unwrap();

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
        let key1 = create_api_key(&pool, &manager, "key-1").unwrap();
        let _key2 = create_api_key(&pool, &manager, "key-2").unwrap();

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
        let key = create_api_key(&pool, &manager, "legacy").unwrap();

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

    fn test_hex_encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

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
        use sha2::{Digest, Sha256};
        let token = format!("lific_at_test-{suffix}");
        let hash = test_hex_encode(&Sha256::digest(token.as_bytes()));
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
    async fn legacy_api_key_without_user_resolves_to_none_via_middleware() {
        let pool = test_db();
        let manager = create_key_manager().unwrap();
        let key = create_api_key(&pool, &manager, "legacy-plain").unwrap();

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

    // ── LIF-294: [auth] required = false ─────────────────────────

    /// Router whose handler reports whether the request runs with the
    /// operator bypass: with authz enforcement ON, `visible_project_ids`
    /// for a `None` user returns unrestricted (None) only inside an
    /// operator context.
    fn operator_probe_app(auth_state: AuthState, pool: db::DbPool) -> Router {
        Router::new()
            .route(
                "/probe",
                get(move || {
                    let pool = pool.clone();
                    async move {
                        match crate::authz::visible_project_ids(&pool, &None).unwrap() {
                            None => "unrestricted".to_string(),
                            Some(ids) => format!("restricted:{}", ids.len()),
                        }
                    }
                }),
            )
            .layer(middleware::from_fn_with_state(auth_state, require_api_key))
    }

    #[tokio::test]
    async fn auth_not_required_credentialless_request_passes_as_operator() {
        let pool = test_db();
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
    async fn oauth_token_for_deleted_user_resolves_to_none_not_panic() {
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
        // Must not panic, and must not resolve to a phantom user.
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(bytes.as_ref(), b"none");
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
        let key = create_api_key(&pool, &manager, "expired").unwrap();
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
        let key = create_api_key(&pool, &manager, "future").unwrap();
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
        let key = create_api_key(&pool, &manager, "forever").unwrap();

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
        let key = create_api_key(&pool, &manager, "legacy-expired").unwrap();
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
        create_api_key_with_expiry(&pool, &manager, "dated", Some("2030-06-01")).unwrap();

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

    // ── LIF-261: operator-key trust rule, end-to-end through the middleware ──
    //
    // The auth middleware sets `authz::operator_scope(true, ..)` ONLY on the
    // unbound-API-key path. These drive a real route that runs
    // `authz::require_role(.., Viewer)` in enforced mode behind the real
    // `require_api_key`, so a 200 means the gate passed and a 403 means it
    // denied — proving the credential-type signal reaches authz and that a
    // legacy unbound OAuth token (also `None`) does NOT get the bypass.

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
            Extension(auth_user): Extension<Option<crate::db::models::AuthUser>>,
        ) -> Result<String, crate::error::LificError> {
            crate::authz::require_role(
                &pool,
                &auth_user,
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
        let manager = create_key_manager().unwrap();
        let key = create_api_key(&pool, &manager, "operator").unwrap(); // unbound
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
        let token = insert_oauth_token(&pool, "legacy-unbound", None);

        let app = gate_app(test_auth_state(&pool), pool.clone(), project);
        assert_eq!(
            gate_status(app, &token).await,
            StatusCode::FORBIDDEN,
            "a legacy unbound OAuth token must NOT gain operator power — it stays default-denied"
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
        create_api_key(&pool, &manager, "bound").unwrap();
        let key = {
            // Bind the key to a fresh non-admin user, then re-read plaintext by
            // rotating is overkill; instead create bound key by assigning.
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
            let conn = pool.write().unwrap();
            crate::db::queries::users::assign_key_to_user(&conn, "bound", uid).unwrap();
            drop(conn);
            // Rotate to obtain a usable plaintext bound to that user (rotation
            // carries the binding over — LIF-132).
            rotate_api_key(&pool, &manager, "bound").unwrap()
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
}
