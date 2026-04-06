use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use rusqlite::{Connection, params};

use crate::db::models::*;
use crate::error::LificError;

// ── Password hashing ─────────────────────────────────────────

/// Hash a password with argon2 using a random salt.
pub fn hash_password(password: &str) -> Result<String, LificError> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| LificError::Internal(format!("password hashing failed: {e}")))?;
    Ok(hash.to_string())
}

/// Verify a password against an argon2 hash.
pub fn verify_password(password: &str, hash: &str) -> Result<bool, LificError> {
    let parsed = PasswordHash::new(hash)
        .map_err(|e| LificError::Internal(format!("invalid password hash: {e}")))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

// ── User CRUD ────────────────────────────────────────────────

pub fn create_user(conn: &Connection, input: &CreateUser) -> Result<User, LificError> {
    // Validate
    let username = input.username.trim();
    let email = input.email.trim().to_lowercase();

    if username.is_empty() {
        return Err(LificError::BadRequest("username cannot be empty".into()));
    }
    if email.is_empty() || !email.contains('@') {
        return Err(LificError::BadRequest("invalid email address".into()));
    }
    if input.password.len() < 8 {
        return Err(LificError::BadRequest(
            "password must be at least 8 characters".into(),
        ));
    }

    let password_hash = hash_password(&input.password)?;
    let display_name = input
        .display_name
        .as_deref()
        .unwrap_or(username)
        .to_string();

    conn.execute(
        "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            username,
            email,
            password_hash,
            display_name,
            input.is_admin,
            input.is_bot,
        ],
    )
    .map_err(|e| match e {
        rusqlite::Error::SqliteFailure(err, _)
            if err.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            LificError::BadRequest("an account with this username or email already exists".into())
        }
        other => other.into(),
    })?;

    let id = conn.last_insert_rowid();
    get_user_by_id(conn, id)
}

pub fn get_user_by_id(conn: &Connection, id: i64) -> Result<User, LificError> {
    conn.query_row(
        "SELECT id, username, email, password_hash, display_name, is_admin, is_bot, created_at, updated_at
         FROM users WHERE id = ?1",
        params![id],
        row_to_user,
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => LificError::NotFound(format!("user {id} not found")),
        other => other.into(),
    })
}

pub fn get_user_by_username(conn: &Connection, username: &str) -> Result<User, LificError> {
    conn.query_row(
        "SELECT id, username, email, password_hash, display_name, is_admin, is_bot, created_at, updated_at
         FROM users WHERE username = ?1 COLLATE NOCASE",
        params![username],
        row_to_user,
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            LificError::NotFound(format!("user '{username}' not found"))
        }
        other => other.into(),
    })
}

pub fn get_user_by_email(conn: &Connection, email: &str) -> Result<User, LificError> {
    let email = email.trim().to_lowercase();
    conn.query_row(
        "SELECT id, username, email, password_hash, display_name, is_admin, is_bot, created_at, updated_at
         FROM users WHERE email = ?1 COLLATE NOCASE",
        params![email],
        row_to_user,
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            LificError::NotFound(format!("user with email '{email}' not found"))
        }
        other => other.into(),
    })
}

/// Look up a user by username or email and verify their password.
/// Returns the user on success, or an error on wrong credentials.
pub fn authenticate(conn: &Connection, identity: &str, password: &str) -> Result<User, LificError> {
    // Try username first, then email
    let user = get_user_by_username(conn, identity)
        .or_else(|_| get_user_by_email(conn, identity))
        .map_err(|_| LificError::BadRequest("invalid username/email or password".into()))?;

    if !verify_password(password, &user.password_hash)? {
        return Err(LificError::BadRequest(
            "invalid username/email or password".into(),
        ));
    }

    Ok(user)
}

pub fn list_users(conn: &Connection) -> Result<Vec<User>, LificError> {
    let mut stmt = conn.prepare(
        "SELECT id, username, email, password_hash, display_name, is_admin, is_bot, created_at, updated_at
         FROM users ORDER BY created_at",
    )?;
    let rows = stmt.query_map([], row_to_user)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn row_to_user(row: &rusqlite::Row) -> Result<User, rusqlite::Error> {
    Ok(User {
        id: row.get(0)?,
        username: row.get(1)?,
        email: row.get(2)?,
        password_hash: row.get(3)?,
        display_name: row.get(4)?,
        is_admin: row.get(5)?,
        is_bot: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

// ── Sessions ─────────────────────────────────────────────────

/// Create a new session for a user. Returns the session with a generated token.
/// Sessions expire after `duration_hours` (default 24 * 7 = 1 week).
pub fn create_session(
    conn: &Connection,
    user_id: i64,
    duration_hours: Option<i64>,
) -> Result<Session, LificError> {
    let hours = duration_hours.unwrap_or(24 * 7); // 1 week default
    let token = generate_session_token();

    conn.execute(
        "INSERT INTO sessions (token, user_id, expires_at)
         VALUES (?1, ?2, datetime('now', ?3))",
        params![token, user_id, format!("+{hours} hours")],
    )?;

    conn.query_row(
        "SELECT token, user_id, expires_at, created_at FROM sessions WHERE token = ?1",
        params![token],
        row_to_session,
    )
    .map_err(Into::into)
}

/// Validate a session token. Returns the associated user if the session
/// exists and has not expired. Expired sessions are cleaned up lazily.
pub fn validate_session(conn: &Connection, token: &str) -> Result<User, LificError> {
    // Delete expired sessions while we're here (lazy cleanup)
    let _ = conn.execute(
        "DELETE FROM sessions WHERE expires_at < datetime('now')",
        [],
    );

    let user_id: i64 = conn
        .query_row(
            "SELECT user_id FROM sessions WHERE token = ?1 AND expires_at > datetime('now')",
            params![token],
            |row| row.get(0),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                LificError::BadRequest("invalid or expired session".into())
            }
            other => other.into(),
        })?;

    get_user_by_id(conn, user_id)
}

/// Delete a session (logout).
pub fn delete_session(conn: &Connection, token: &str) -> Result<(), LificError> {
    conn.execute("DELETE FROM sessions WHERE token = ?1", params![token])?;
    Ok(())
}

/// Delete all sessions for a user.
#[allow(dead_code)]
pub fn delete_all_sessions(conn: &Connection, user_id: i64) -> Result<(), LificError> {
    conn.execute("DELETE FROM sessions WHERE user_id = ?1", params![user_id])?;
    Ok(())
}

fn row_to_session(row: &rusqlite::Row) -> Result<Session, rusqlite::Error> {
    Ok(Session {
        token: row.get(0)?,
        user_id: row.get(1)?,
        expires_at: row.get(2)?,
        created_at: row.get(3)?,
    })
}

/// Generate a session token with the lific_sess_ prefix.
fn generate_session_token() -> String {
    let bytes: [u8; 32] = rand::random();
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("lific_sess_{hex}")
}

// ── API key ownership ────────────────────────────────────────

/// Assign an existing API key to a user.
pub fn assign_key_to_user(
    conn: &Connection,
    key_name: &str,
    user_id: i64,
) -> Result<(), LificError> {
    let changed = conn.execute(
        "UPDATE api_keys SET user_id = ?1 WHERE name = ?2 AND revoked = 0",
        params![user_id, key_name],
    )?;
    if changed == 0 {
        return Err(LificError::NotFound(format!(
            "no active key named '{key_name}'"
        )));
    }
    Ok(())
}

/// Get the user_id associated with an API key (by hash match).
/// Returns None if the key has no user_id assigned.
#[allow(dead_code)]
pub fn get_user_for_api_key(conn: &Connection, key_id: i64) -> Result<Option<User>, LificError> {
    let user_id: Option<i64> = conn
        .query_row(
            "SELECT user_id FROM api_keys WHERE id = ?1",
            params![key_id],
            |row| row.get(0),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                LificError::NotFound("api key not found".into())
            }
            other => other.into(),
        })?;

    match user_id {
        Some(uid) => Ok(Some(get_user_by_id(conn, uid)?)),
        None => Ok(None),
    }
}

// ── Bots (connected tools) ───────────────────────────────────

/// Create a bot user owned by the given human user.
/// Returns the bot user. API key creation is handled separately by the caller
/// using `auth::create_api_key` + `assign_key_to_user`.
pub fn create_bot_user(
    conn: &Connection,
    owner_id: i64,
    bot_username: &str,
    display_name: &str,
) -> Result<crate::db::models::User, LificError> {
    // Bot users get a random password (never used for login)
    let random_pw: [u8; 32] = rand::random();
    let random_pw_hex: String = random_pw.iter().map(|b| format!("{b:02x}")).collect();
    let password_hash = hash_password(&random_pw_hex)?;

    conn.execute(
        "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot, owner_id)
         VALUES (?1, ?2, ?3, ?4, 0, 1, ?5)",
        params![
            bot_username,
            format!("{bot_username}@bot.local"),
            password_hash,
            display_name,
            owner_id,
        ],
    )
    .map_err(|e| match e {
        rusqlite::Error::SqliteFailure(err, _)
            if err.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            LificError::BadRequest(format!(
                "this tool is already connected (bot '{bot_username}' exists)"
            ))
        }
        other => other.into(),
    })?;

    let bot_user_id = conn.last_insert_rowid();
    get_user_by_id(conn, bot_user_id)
}

/// Set or unset admin status on a user.
pub fn set_admin(conn: &Connection, username: &str, is_admin: bool) -> Result<(), LificError> {
    let changed = conn.execute(
        "UPDATE users SET is_admin = ?1, updated_at = datetime('now') WHERE username = ?2 COLLATE NOCASE",
        params![is_admin, username],
    )?;
    if changed == 0 {
        return Err(LificError::NotFound(format!("user '{username}' not found")));
    }
    Ok(())
}

/// Find a bot user by username (for reconnection checks).
pub fn find_bot_by_username(
    conn: &Connection,
    username: &str,
) -> Result<Option<crate::db::models::User>, LificError> {
    match conn.query_row(
        "SELECT id, username, email, password_hash, display_name, is_admin, is_bot, created_at, updated_at
         FROM users WHERE username = ?1 AND is_bot = 1",
        params![username],
        row_to_user,
    ) {
        Ok(user) => Ok(Some(user)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Check if a bot has any active (non-revoked) API keys.
pub fn bot_has_active_key(conn: &Connection, bot_id: i64) -> Result<bool, LificError> {
    let has: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM api_keys WHERE user_id = ?1 AND revoked = 0",
            params![bot_id],
            |row| row.get(0),
        )
        .unwrap_or(false);
    Ok(has)
}

/// List all bots owned by a specific user.
pub fn list_bots(
    conn: &Connection,
    owner_id: i64,
) -> Result<Vec<crate::db::models::Bot>, LificError> {
    let mut stmt = conn.prepare(
        "SELECT u.id, u.username, u.display_name, u.owner_id, u.created_at,
                EXISTS(SELECT 1 FROM api_keys k WHERE k.user_id = u.id AND k.revoked = 0) as has_key
         FROM users u
         WHERE u.is_bot = 1 AND u.owner_id = ?1
         ORDER BY u.created_at DESC",
    )?;
    let rows = stmt.query_map(params![owner_id], |row| {
        Ok(crate::db::models::Bot {
            id: row.get(0)?,
            username: row.get(1)?,
            display_name: row.get(2)?,
            owner_id: row.get(3)?,
            created_at: row.get(4)?,
            has_active_key: row.get(5)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// Disconnect a bot: revoke its API key(s). Only the owner or admin can do this.
pub fn disconnect_bot(
    conn: &Connection,
    bot_id: i64,
    requester_id: i64,
    is_admin: bool,
) -> Result<(), LificError> {
    // Verify ownership
    let owner_id: Option<i64> = conn
        .query_row(
            "SELECT owner_id FROM users WHERE id = ?1 AND is_bot = 1",
            params![bot_id],
            |row| row.get(0),
        )
        .map_err(|_| LificError::NotFound("bot not found".into()))?;

    if owner_id != Some(requester_id) && !is_admin {
        return Err(LificError::BadRequest(
            "you can only disconnect your own bots".into(),
        ));
    }

    // Revoke all API keys for this bot
    conn.execute(
        "UPDATE api_keys SET revoked = 1 WHERE user_id = ?1 AND revoked = 0",
        params![bot_id],
    )?;

    Ok(())
}

/// Permanently delete a bot user and all its API keys.
/// Only the owner or an admin can do this.
pub fn delete_bot(
    conn: &Connection,
    bot_id: i64,
    requester_id: i64,
    is_admin: bool,
) -> Result<(), LificError> {
    // Verify ownership
    let owner_id: Option<i64> = conn
        .query_row(
            "SELECT owner_id FROM users WHERE id = ?1 AND is_bot = 1",
            params![bot_id],
            |row| row.get(0),
        )
        .map_err(|_| LificError::NotFound("bot not found".into()))?;

    if owner_id != Some(requester_id) && !is_admin {
        return Err(LificError::BadRequest(
            "you can only delete your own bots".into(),
        ));
    }

    // Delete API keys first (FK constraint)
    conn.execute("DELETE FROM api_keys WHERE user_id = ?1", params![bot_id])?;

    // Delete any comments made by this bot (or reassign — deleting for now)
    conn.execute("DELETE FROM comments WHERE user_id = ?1", params![bot_id])?;

    // Delete the bot user
    let changed = conn.execute(
        "DELETE FROM users WHERE id = ?1 AND is_bot = 1",
        params![bot_id],
    )?;

    if changed == 0 {
        return Err(LificError::NotFound("bot not found".into()));
    }

    Ok(())
}

/// List API keys belonging to a specific user.
pub fn list_user_keys(
    conn: &Connection,
    user_id: i64,
) -> Result<Vec<crate::db::models::UserApiKey>, LificError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, created_at, expires_at, revoked
         FROM api_keys WHERE user_id = ?1
         ORDER BY created_at DESC",
    )?;
    let rows = stmt.query_map(params![user_id], |row| {
        Ok(crate::db::models::UserApiKey {
            id: row.get(0)?,
            name: row.get(1)?,
            created_at: row.get(2)?,
            expires_at: row.get(3)?,
            revoked: row.get(4)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// Revoke an API key, but only if it belongs to the given user (or user is admin).
pub fn revoke_user_key(
    conn: &Connection,
    key_id: i64,
    user_id: i64,
    is_admin: bool,
) -> Result<(), LificError> {
    let changed = if is_admin {
        conn.execute(
            "UPDATE api_keys SET revoked = 1 WHERE id = ?1 AND revoked = 0",
            params![key_id],
        )?
    } else {
        conn.execute(
            "UPDATE api_keys SET revoked = 1 WHERE id = ?1 AND user_id = ?2 AND revoked = 0",
            params![key_id, user_id],
        )?
    };

    if changed == 0 {
        return Err(LificError::NotFound(
            "key not found or already revoked".into(),
        ));
    }
    Ok(())
}

// ── Tests ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn test_db() -> db::DbPool {
        db::open_memory().expect("test db")
    }

    fn test_create_user(conn: &Connection) -> User {
        create_user(
            conn,
            &CreateUser {
                username: "blake".into(),
                email: "blake@example.com".into(),
                password: "securepassword123".into(),
                display_name: Some("Blake".into()),
                is_admin: true,
                is_bot: false,
            },
        )
        .expect("create user")
    }

    #[test]
    fn password_hash_roundtrip() {
        let hash = hash_password("my-secret-pass").unwrap();
        assert!(hash.starts_with("$argon2"));
        assert!(verify_password("my-secret-pass", &hash).unwrap());
        assert!(!verify_password("wrong-pass", &hash).unwrap());
    }

    #[test]
    fn create_and_get_user() {
        let pool = test_db();
        let conn = pool.write().unwrap();

        let user = test_create_user(&conn);
        assert_eq!(user.username, "blake");
        assert_eq!(user.email, "blake@example.com");
        assert_eq!(user.display_name, "Blake");
        assert!(user.is_admin);
        assert!(!user.is_bot);

        // password_hash should be argon2
        assert!(user.password_hash.starts_with("$argon2"));

        // Get by ID
        let fetched = get_user_by_id(&conn, user.id).unwrap();
        assert_eq!(fetched.username, "blake");

        // Get by username (case insensitive)
        let fetched = get_user_by_username(&conn, "Blake").unwrap();
        assert_eq!(fetched.id, user.id);

        // Get by email
        let fetched = get_user_by_email(&conn, "BLAKE@EXAMPLE.COM").unwrap();
        assert_eq!(fetched.id, user.id);
    }

    #[test]
    fn duplicate_username_rejected() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        test_create_user(&conn);

        let result = create_user(
            &conn,
            &CreateUser {
                username: "blake".into(),
                email: "other@example.com".into(),
                password: "anotherpassword1".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    #[test]
    fn duplicate_email_rejected() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        test_create_user(&conn);

        let result = create_user(
            &conn,
            &CreateUser {
                username: "other".into(),
                email: "blake@example.com".into(),
                password: "anotherpassword1".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn short_password_rejected() {
        let pool = test_db();
        let conn = pool.write().unwrap();

        let result = create_user(
            &conn,
            &CreateUser {
                username: "test".into(),
                email: "test@example.com".into(),
                password: "short".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("8 characters"));
    }

    #[test]
    fn authenticate_correct_password() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        test_create_user(&conn);

        // By username
        let user = authenticate(&conn, "blake", "securepassword123").unwrap();
        assert_eq!(user.username, "blake");

        // By email
        let user = authenticate(&conn, "blake@example.com", "securepassword123").unwrap();
        assert_eq!(user.username, "blake");
    }

    #[test]
    fn authenticate_wrong_password_rejected() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        test_create_user(&conn);

        let result = authenticate(&conn, "blake", "wrongpassword123");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid"));
    }

    #[test]
    fn authenticate_nonexistent_user_rejected() {
        let pool = test_db();
        let conn = pool.write().unwrap();

        let result = authenticate(&conn, "nobody", "password12345678");
        assert!(result.is_err());
    }

    #[test]
    fn list_users_returns_all() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        test_create_user(&conn);

        create_user(
            &conn,
            &CreateUser {
                username: "ada".into(),
                email: "ada@example.com".into(),
                password: "adaspassword123".into(),
                display_name: Some("Ada".into()),
                is_admin: false,
                is_bot: true,
            },
        )
        .unwrap();

        let users = list_users(&conn).unwrap();
        assert_eq!(users.len(), 2);
    }

    #[test]
    fn display_name_defaults_to_username() {
        let pool = test_db();
        let conn = pool.write().unwrap();

        let user = create_user(
            &conn,
            &CreateUser {
                username: "noname".into(),
                email: "noname@example.com".into(),
                password: "password12345678".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap();

        assert_eq!(user.display_name, "noname");
    }

    // ── Session tests ────────────────────────────────────────

    #[test]
    fn session_create_and_validate() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let user = test_create_user(&conn);

        let session = create_session(&conn, user.id, None).unwrap();
        assert!(session.token.starts_with("lific_sess_"));
        assert_eq!(session.user_id, user.id);

        // Validate returns the user
        let validated_user = validate_session(&conn, &session.token).unwrap();
        assert_eq!(validated_user.id, user.id);
        assert_eq!(validated_user.username, "blake");
    }

    #[test]
    fn session_invalid_token_rejected() {
        let pool = test_db();
        let conn = pool.write().unwrap();

        let result = validate_session(&conn, "lific_sess_nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn session_expired_rejected() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let user = test_create_user(&conn);

        // Create a session that already expired (negative duration trick)
        let token = generate_session_token();
        conn.execute(
            "INSERT INTO sessions (token, user_id, expires_at)
             VALUES (?1, ?2, datetime('now', '-1 hour'))",
            params![token, user.id],
        )
        .unwrap();

        let result = validate_session(&conn, &token);
        assert!(result.is_err());
    }

    #[test]
    fn session_delete_logout() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let user = test_create_user(&conn);

        let session = create_session(&conn, user.id, None).unwrap();
        assert!(validate_session(&conn, &session.token).is_ok());

        delete_session(&conn, &session.token).unwrap();
        assert!(validate_session(&conn, &session.token).is_err());
    }

    #[test]
    fn session_delete_all_for_user() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let user = test_create_user(&conn);

        let s1 = create_session(&conn, user.id, None).unwrap();
        let s2 = create_session(&conn, user.id, None).unwrap();

        delete_all_sessions(&conn, user.id).unwrap();
        assert!(validate_session(&conn, &s1.token).is_err());
        assert!(validate_session(&conn, &s2.token).is_err());
    }

    // ── API key ownership tests ──────────────────────────────

    #[test]
    fn assign_key_to_user_works() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let user = test_create_user(&conn);

        // Create an API key manually
        conn.execute(
            "INSERT INTO api_keys (name, key_hash) VALUES ('opencode', 'fakehash')",
            [],
        )
        .unwrap();

        let key_id: i64 = conn
            .query_row(
                "SELECT id FROM api_keys WHERE name = 'opencode'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        // Before assignment: no user
        let owner = get_user_for_api_key(&conn, key_id).unwrap();
        assert!(owner.is_none());

        // Assign
        assign_key_to_user(&conn, "opencode", user.id).unwrap();

        // After assignment: user returned
        let owner = get_user_for_api_key(&conn, key_id).unwrap();
        assert!(owner.is_some());
        assert_eq!(owner.unwrap().username, "blake");
    }

    #[test]
    fn assign_nonexistent_key_fails() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let user = test_create_user(&conn);

        let result = assign_key_to_user(&conn, "nope", user.id);
        assert!(result.is_err());
    }
}
