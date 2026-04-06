use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
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

    conn.execute(
        "INSERT INTO api_keys (name, key_hash) VALUES (?1, ?2)",
        params![name, hash],
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
pub fn rotate_api_key(
    db: &DbPool,
    manager: &ApiKeyManagerV0,
    name: &str,
) -> Result<String, crate::error::LificError> {
    // Delete old key entirely (not just revoke) so the name can be reused
    let conn = db.write()?;
    let changed = conn.execute("DELETE FROM api_keys WHERE name = ?1", params![name])?;
    if changed == 0 {
        return Err(crate::error::LificError::NotFound(format!(
            "no key named '{name}'"
        )));
    }
    drop(conn);

    create_api_key(db, manager, name)
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

    let www_auth = format!(
        "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource\"",
        auth.public_url
    );

    let Some(token) = token else {
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
                request.extensions_mut().insert(Some(auth_user));
                return next.run(request).await;
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
            // OAuth tokens don't have user association yet
            request
                .extensions_mut()
                .insert(None::<crate::db::models::AuthUser>);
            return next.run(request).await;
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

    // Load all active key hashes with their user_id
    let keys: Vec<ApiKeyRow> = {
        let conn = match auth.db.read() {
            Ok(c) => c,
            Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response(),
        };
        let mut stmt = match conn
            .prepare("SELECT id, key_hash, user_id FROM api_keys WHERE revoked = 0")
        {
            Ok(s) => s,
            Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response(),
        };
        match stmt.query_map([], |row| {
            Ok(ApiKeyRow {
                id: row.get(0)?,
                hash: row.get(1)?,
                user_id: row.get(2)?,
            })
        }) {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response(),
        }
    };

    if keys.is_empty() {
        warn!("no API keys configured — rejecting request");
        return (
            StatusCode::UNAUTHORIZED,
            [("WWW-Authenticate", www_auth.as_str())],
            "No API keys configured. Create one with: lific key create --name <name>",
        )
            .into_response();
    }

    for key in &keys {
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
                request.extensions_mut().insert(auth_user);
                return next.run(request).await;
            }
            Ok(KeyStatus::Invalid) => continue,
            Err(_) => continue,
        }
    }

    warn!("rejected invalid API key or OAuth token");
    (
        StatusCode::UNAUTHORIZED,
        [("WWW-Authenticate", www_auth.as_str())],
        "Invalid API key",
    )
        .into_response()
}

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
        match status {
            Ok(KeyStatus::Valid) => panic!("wrong key should not validate"),
            _ => {} // Invalid or Error -- both correct
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
}
