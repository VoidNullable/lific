use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use rusqlite::{params, Connection};

use crate::db::models::*;
use crate::error::LificError;

pub(crate) const INVALID_SESSION_MESSAGE: &str = "invalid or expired session";

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
    if input.password.len() > 1024 {
        return Err(LificError::BadRequest(
            "password must be 1024 characters or fewer".into(),
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

/// Pre-computed Argon2 hash of a dummy password, used to normalize timing
/// when the requested user doesn't exist. This ensures login attempts for
/// non-existent users take the same time as attempts with wrong passwords.
const DUMMY_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAa$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

/// Look up a user by username or email and verify their password.
/// Returns the user on success, or an error on wrong credentials.
pub fn authenticate(conn: &Connection, identity: &str, password: &str) -> Result<User, LificError> {
    // Reject oversized passwords early to prevent Argon2 CPU DoS
    if password.len() > 1024 {
        return Err(LificError::BadRequest(
            "invalid username/email or password".into(),
        ));
    }
    // Try username first, then email.
    // If the user doesn't exist, still run Argon2 against a dummy hash
    // to prevent timing side-channel enumeration of valid usernames.
    let user = get_user_by_username(conn, identity).or_else(|_| get_user_by_email(conn, identity));

    let (user, hash) = match user {
        Ok(u) => {
            let h = u.password_hash.clone();
            (Some(u), h)
        }
        Err(_) => (None, DUMMY_HASH.to_string()),
    };

    let password_ok = verify_password(password, &hash).unwrap_or(false);

    match user {
        Some(u) if password_ok => Ok(u),
        _ => Err(LificError::BadRequest(
            "invalid username/email or password".into(),
        )),
    }
}

/// LIF-190: update the authenticated user's profile fields. Each field is
/// optional so the caller can PATCH just one. Returns the refreshed user.
pub fn update_profile(
    conn: &Connection,
    user_id: i64,
    display_name: Option<&str>,
    email: Option<&str>,
) -> Result<User, LificError> {
    if let Some(dn) = display_name {
        let dn = dn.trim();
        if dn.is_empty() {
            return Err(LificError::BadRequest("display name cannot be empty".into()));
        }
        if dn.chars().count() > 100 {
            return Err(LificError::BadRequest(
                "display name must be 100 characters or fewer".into(),
            ));
        }
        conn.execute(
            "UPDATE users SET display_name = ?1 WHERE id = ?2",
            params![dn, user_id],
        )?;
    }
    if let Some(em) = email {
        let em = em.trim().to_lowercase();
        if em.is_empty() || !em.contains('@') {
            return Err(LificError::BadRequest("invalid email address".into()));
        }
        conn.execute("UPDATE users SET email = ?1 WHERE id = ?2", params![em, user_id])
            .map_err(|e| match e {
                rusqlite::Error::SqliteFailure(err, _)
                    if err.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    LificError::BadRequest("that email is already in use".into())
                }
                other => other.into(),
            })?;
    }
    get_user_by_id(conn, user_id)
}

/// LIF-190: replace the user's password. Caller is responsible for verifying
/// the current password first. Enforces the same length bounds as signup.
pub fn update_password(
    conn: &Connection,
    user_id: i64,
    new_password: &str,
) -> Result<(), LificError> {
    if new_password.len() < 8 {
        return Err(LificError::BadRequest(
            "password must be at least 8 characters".into(),
        ));
    }
    if new_password.len() > 1024 {
        return Err(LificError::BadRequest(
            "password must be 1024 characters or fewer".into(),
        ));
    }
    let hash = hash_password(new_password)?;
    conn.execute(
        "UPDATE users SET password_hash = ?1 WHERE id = ?2",
        params![hash, user_id],
    )?;
    Ok(())
}

pub fn list_users(conn: &Connection) -> Result<Vec<User>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, username, email, password_hash, display_name, is_admin, is_bot, created_at, updated_at
         FROM users ORDER BY created_at",
    )?;
    let rows = stmt.query_map([], row_to_user)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// True if at least one human (non-bot) account exists.
///
/// Backs the public `GET /api/instance` endpoint so the auth screen can tell a
/// brand-new instance ("be the first account") from an established one ("join
/// this instance") without leaking any user data. Bot identities are excluded
/// because a connected tool is not a person who has signed up.
pub fn has_human_users(conn: &Connection) -> Result<bool, LificError> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM users WHERE is_bot = 0)",
        [],
        |row| row.get(0),
    )?;
    Ok(exists)
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

/// Return the first admin user (by creation time), if any.
/// Used as a fallback author for MCP stdio sessions where no HTTP auth is present.
pub fn first_admin(conn: &Connection) -> Result<Option<AuthUser>, LificError> {
    match conn.query_row(
        "SELECT id, username, display_name, is_admin FROM users WHERE is_admin = 1 ORDER BY created_at LIMIT 1",
        [],
        |row| {
            Ok(AuthUser {
                id: row.get(0)?,
                username: row.get(1)?,
                display_name: row.get(2)?,
                is_admin: row.get(3)?,
            })
        },
    ) {
        Ok(user) => Ok(Some(user)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

// ── Passwordless admin (LIFIC-9) ────────────────────────────

/// Derive a usable, unique username from a display name. Keeps [a-z0-9-],
/// collapses runs of non-alphanumerics to a single `-`, and falls back to
/// `admin` if nothing survives; appends `-N` when the raw slug is taken.
fn derive_username(conn: &Connection, display_name: &str) -> Result<String, LificError> {
    let slug: String = display_name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let base = if slug.is_empty() { "admin".to_string() } else { slug };
    let mut candidate = base.clone();
    let mut n = 1;
    while get_user_by_username(conn, &candidate).is_ok() {
        candidate = format!("{base}-{n}");
        n += 1;
    }
    Ok(candidate)
}

/// Create the first human admin on a fresh install — a passwordless operator.
///
/// "Passwordless" means it can never be signed into by password: the stored
/// hash is a random value with no known plaintext, and the email is a synthetic
/// placeholder that satisfies the NOT NULL UNIQUE schema. The operator reaches
/// this identity through the browser auto-login / passwordless fallback in
/// `resolve_caller`, never through a password prompt.
///
/// LIFIC-9: this is what makes `[auth] required = false` "passwordless mode"
/// instead of "half-broken anonymous" — there is always a real admin to resolve
/// to from the moment the instance exists.
pub fn create_passwordless_admin(
    conn: &Connection,
    display_name: &str,
) -> Result<User, LificError> {
    let display_name = display_name.trim();
    if display_name.is_empty() {
        return Err(LificError::BadRequest(
            "operator name cannot be empty".into(),
        ));
    }
    let username = derive_username(conn, display_name)?;
    // Unusable hash: never arithmetically a login password, just fills the NOT
    // NULL column. Same guarantee as `create_bot_user`.
    let password_hash = unusable_password_hash()?;

    conn.execute(
        "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot)
         VALUES (?1, ?2, ?3, ?4, 1, 0)",
        params![
            username,
            format!("{username}@local"),
            password_hash,
            display_name,
        ],
    )
    .map_err(|e| match e {
        rusqlite::Error::SqliteFailure(err, _)
            if err.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            LificError::Internal("failed to create first admin (constraint)".into())
        }
        other => other.into(),
    })?;

    let id = conn.last_insert_rowid();
    get_user_by_id(conn, id)
}

// ── Sessions ─────────────────────────────────────────────────

/// Hash a session token with SHA-256 for storage.
fn hash_session_token(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(token.as_bytes());
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

/// Create a new session for a user. Returns the session with the plaintext token
/// (shown once to the client). The SHA-256 hash is stored in the database.
/// Sessions expire after `duration_hours` (default 24 * 7 = 1 week).
pub fn create_session(
    conn: &Connection,
    user_id: i64,
    duration_hours: Option<i64>,
) -> Result<Session, LificError> {
    let hours = duration_hours.unwrap_or(24 * 7); // 1 week default
    let token = generate_session_token();
    let token_hash = hash_session_token(&token);

    conn.execute(
        "INSERT INTO sessions (token, user_id, expires_at)
         VALUES (?1, ?2, datetime('now', ?3))",
        params![token_hash, user_id, format!("+{hours} hours")],
    )?;

    // Return the plaintext token to the caller (shown to the client once)
    Ok(Session {
        token,
        user_id,
        expires_at: conn.query_row(
            "SELECT expires_at FROM sessions WHERE token = ?1",
            params![token_hash],
            |row| row.get(0),
        )?,
        created_at: conn.query_row(
            "SELECT created_at FROM sessions WHERE token = ?1",
            params![token_hash],
            |row| row.get(0),
        )?,
    })
}

/// Validate a session token. Returns the associated user if the session
/// exists and has not expired. Expired sessions are cleaned up lazily.
/// The incoming plaintext token is hashed with SHA-256 before lookup.
pub fn validate_session(conn: &Connection, token: &str) -> Result<User, LificError> {
    // Delete expired sessions while we're here (lazy cleanup)
    let _ = conn.execute(
        "DELETE FROM sessions WHERE expires_at < datetime('now')",
        [],
    );

    let token_hash = hash_session_token(token);

    let user_id: i64 = conn
        .query_row(
            "SELECT user_id FROM sessions WHERE token = ?1 AND expires_at > datetime('now')",
            params![token_hash],
            |row| row.get(0),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                LificError::BadRequest(INVALID_SESSION_MESSAGE.into())
            }
            other => other.into(),
        })?;

    get_user_by_id(conn, user_id)
}

/// Delete a session (logout). Hashes the plaintext token before lookup.
pub fn delete_session(conn: &Connection, token: &str) -> Result<(), LificError> {
    let token_hash = hash_session_token(token);
    conn.execute("DELETE FROM sessions WHERE token = ?1", params![token_hash])?;
    Ok(())
}

/// Delete all sessions for a user.
#[allow(dead_code)]
pub fn delete_all_sessions(conn: &Connection, user_id: i64) -> Result<(), LificError> {
    conn.execute("DELETE FROM sessions WHERE user_id = ?1", params![user_id])?;
    Ok(())
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
/// Generate an unusable password hash: a random value with no known plaintext.
///
/// Used for identities that must never be signed into by password (passwordless
/// human admins, and bots) to satisfy the NOT NULL `password_hash` column while
/// guaranteeing `authenticate` can never succeed against it.
fn unusable_password_hash() -> Result<String, LificError> {
    let random_pw: [u8; 32] = rand::random();
    let random_pw_hex: String = random_pw.iter().map(|b| format!("{b:02x}")).collect();
    hash_password(&random_pw_hex)
}

pub fn create_bot_user(
    conn: &Connection,
    owner_id: i64,
    bot_username: &str,
    display_name: &str,
) -> Result<crate::db::models::User, LificError> {
    let password_hash = unusable_password_hash()?;

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

/// Ensure a per-tool bot exists for `owner_id`, reusing an existing one rather
/// than minting a duplicate. The bot username is `{tool_id}-{owner.username}`
/// — the same convention `lific connect` and the web UI's Connected Tools use,
/// so a bot minted at OAuth approval is indistinguishable from one connected
/// another way. Returns the bot user.
///
/// LIFIC-13: the single find-or-create decision all three doors (OAuth
/// approval, `lific connect`, web create_bot) share.
pub fn ensure_bot(
    conn: &Connection,
    owner_id: i64,
    tool_id: &str,
    display_name: &str,
) -> Result<User, LificError> {
    let owner_username = get_user_by_id(conn, owner_id)?.username;
    let bot_username = format!("{tool_id}-{owner_username}");
    match find_bot_by_username(conn, &bot_username)? {
        Some(existing) => Ok(existing),
        None => create_bot_user(conn, owner_id, &bot_username, display_name),
    }
}

/// List all bots owned by a specific user.
pub fn list_bots(
    conn: &Connection,
    owner_id: i64,
) -> Result<Vec<crate::db::models::Bot>, LificError> {
    let mut stmt = conn.prepare_cached(
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

/// Verify a bot both exists and is owned by `requester_id` (or the requester
/// is admin). Returns the bot's id on success. Shared by [`disconnect_bot`]
/// and [`delete_bot`], whose ownership rules are identical.
fn verify_bot_owner(
    conn: &Connection,
    bot_id: i64,
    requester_id: i64,
    is_admin: bool,
    action: &str,
) -> Result<(), LificError> {
    let owner_id: Option<i64> = conn
        .query_row(
            "SELECT owner_id FROM users WHERE id = ?1 AND is_bot = 1",
            params![bot_id],
            |row| row.get(0),
        )
        .map_err(|_| LificError::NotFound("bot not found".into()))?;

    if owner_id != Some(requester_id) && !is_admin {
        return Err(LificError::BadRequest(format!(
            "you can only {action} your own bots"
        )));
    }
    Ok(())
}

/// Disconnect a bot: revoke its credentials (API keys and OAuth tokens) so the
/// bot can no longer act. The bot's identity is kept — reconnecting later
/// reuses it. Only the owner or admin can do this.
pub fn disconnect_bot(
    conn: &Connection,
    bot_id: i64,
    requester_id: i64,
    is_admin: bool,
) -> Result<(), LificError> {
    verify_bot_owner(conn, bot_id, requester_id, is_admin, "disconnect")?;

    // Revoke all API keys for this bot
    conn.execute(
        "UPDATE api_keys SET revoked = 1 WHERE user_id = ?1 AND revoked = 0",
        params![bot_id],
    )?;
    // Revoke all OAuth tokens for this bot (LIFIC-13 follow-up): an
    // OAuth-connected agent has no API key, so without this "Disconnect"
    // would leave its access token live. Rows are kept — reconnectable bot.
    conn.execute(
        "UPDATE oauth_tokens SET revoked = 1 WHERE user_id = ?1 AND revoked = 0",
        params![bot_id],
    )?;

    Ok(())
}

/// Permanently delete a bot user, its API keys, its OAuth tokens, and the
/// comments it made. The identity is gone, so any OAuth token rows are shred
/// rather than revoked. Only the owner or an admin can do this.
pub fn delete_bot(
    conn: &Connection,
    bot_id: i64,
    requester_id: i64,
    is_admin: bool,
) -> Result<(), LificError> {
    verify_bot_owner(conn, bot_id, requester_id, is_admin, "delete")?;

    // Delete API keys first (FK constraint)
    conn.execute("DELETE FROM api_keys WHERE user_id = ?1", params![bot_id])?;
    // Delete the bot's OAuth tokens (LIFIC-13 follow-up): leaves no dangling
    // rows pointing at a removed identity.
    conn.execute("DELETE FROM oauth_tokens WHERE user_id = ?1", params![bot_id])?;

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
    let mut stmt = conn.prepare_cached(
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
    fn has_human_users_false_when_empty_then_true_after_signup() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        assert!(!has_human_users(&conn).unwrap(), "fresh db has no humans");

        test_create_user(&conn);
        assert!(has_human_users(&conn).unwrap(), "human signup flips it true");
    }

    #[test]
    fn has_human_users_ignores_bot_only_instances() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        // A connected tool (bot) is not a person who signed up.
        create_user(
            &conn,
            &CreateUser {
                username: "agent".into(),
                email: "agent@example.com".into(),
                password: "securepassword123".into(),
                display_name: None,
                is_admin: false,
                is_bot: true,
            },
        )
        .unwrap();
        assert!(
            !has_human_users(&conn).unwrap(),
            "a bot-only instance still reads as having no human accounts"
        );
    }

    // ── ensure_bot (LIFIC-13) ────────────────────────────────

    #[test]
    fn ensure_bot_creates_a_new_bot_for_the_owner() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let owner = test_create_user(&conn);

        let bot_id = ensure_bot(&conn, owner.id, "claude-code", "Claude Code")
            .unwrap()
            .id;
        let bot = get_user_by_id(&conn, bot_id).unwrap();
        assert!(bot.is_bot, "minted user is a bot");
        assert_eq!(bot.username, "claude-code-blake");
        assert_eq!(bot.display_name, "Claude Code");
        let listed = list_bots(&conn, owner.id).unwrap();
        assert_eq!(listed.len(), 1, "one bot owned by this user");
        assert_eq!(listed[0].owner_id, Some(owner.id));
    }

    #[test]
    fn ensure_bot_reuses_existing_bot_for_same_tool_and_owner() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let owner = test_create_user(&conn);

        let first = ensure_bot(&conn, owner.id, "opencode", "OpenCode").unwrap().id;
        let second = ensure_bot(&conn, owner.id, "opencode", "OpenCode").unwrap().id;
        assert_eq!(first, second, "re-approval must reuse, not duplicate");
    }

    #[test]
    fn ensure_bot_distinguishes_owners() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let owner_a = test_create_user(&conn);
        let owner_b = create_user(
            &conn,
            &CreateUser {
                username: "ada".into(),
                email: "ada@example.com".into(),
                password: "securepassword123".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap();

        let a = ensure_bot(&conn, owner_a.id, "opencode", "OpenCode").unwrap().id;
        let b = ensure_bot(&conn, owner_b.id, "opencode", "OpenCode").unwrap().id;
        assert_ne!(a, b, "each owner gets its own bot for the same tool");
    }

    // ── disconnect/delete bot credential revocation (LIFIC-13 follow-up) ──

    /// Insert an active (non-revoked) `oauth_tokens` row bound to `user_id`.
    fn insert_oauth_token_for(conn: &Connection, user_id: i64) -> i64 {
        let token_hash = format!("testtoken-{user_id}-{}", user_id);
        let client_id = "test-client";
        conn.execute(
            "INSERT INTO oauth_clients (client_id, client_name, redirect_uris) VALUES (?1, 'Test', '[\"http://localhost\"]')",
            params![client_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO oauth_tokens (access_token, client_id, expires_at, scope, user_id)
             VALUES (?1, ?2, datetime('now', '+1 hour'), 'mcp', ?3)",
            params![token_hash, client_id, user_id],
        )
        .unwrap();
        let id: i64 = conn
            .query_row(
                "SELECT rowid FROM oauth_tokens WHERE access_token = ?1",
                params![token_hash],
                |r| r.get(0),
            )
            .unwrap();
        id
    }

    #[test]
    fn disconnect_bot_revokes_bots_oauth_tokens_but_keeps_bot() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let owner = test_create_user(&conn);
        let bot = ensure_bot(&conn, owner.id, "claude-code", "Claude Code").unwrap();
        insert_oauth_token_for(&conn, bot.id);

        disconnect_bot(&conn, bot.id, owner.id, false).unwrap();

        // The bot and its OAuth tokens still exist (reconnectable), but tokens revoked.
        let revoked: usize = conn
            .query_row(
                "SELECT COUNT(*) FROM oauth_tokens WHERE user_id = ?1 AND revoked = 1",
                params![bot.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(revoked, 1, "bot's OAuth token revoked");
        let still_there: usize = conn
            .query_row(
                "SELECT COUNT(*) FROM oauth_tokens WHERE user_id = ?1",
                params![bot.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(still_there, 1, "token row kept — reconnectable bot");
        let bot_exists: usize = conn
            .query_row(
                "SELECT COUNT(*) FROM users WHERE id = ?1 AND is_bot = 1",
                params![bot.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bot_exists, 1, "bot identity kept after disconnect");
    }

    #[test]
    fn delete_bot_removes_its_oauth_token_rows() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let owner = test_create_user(&conn);
        let bot = ensure_bot(&conn, owner.id, "opencode", "OpenCode").unwrap();
        insert_oauth_token_for(&conn, bot.id);

        delete_bot(&conn, bot.id, owner.id, false).unwrap();

        let tokens: usize = conn
            .query_row(
                "SELECT COUNT(*) FROM oauth_tokens WHERE user_id = ?1",
                params![bot.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tokens, 0, "delete shreds the bot's OAuth token rows");
        let bot_rows: usize = conn
            .query_row(
                "SELECT COUNT(*) FROM users WHERE id = ?1",
                params![bot.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bot_rows, 0, "bot identity removed");
    }

    // ── LIF-190: profile + password updates ─────────────────

    #[test]
    fn update_profile_changes_display_name_and_email() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let user = test_create_user(&conn);

        let updated =
            update_profile(&conn, user.id, Some("Blake W"), Some("NEW@Example.com")).unwrap();
        assert_eq!(updated.display_name, "Blake W");
        assert_eq!(updated.email, "new@example.com"); // normalized lowercase
    }

    #[test]
    fn update_profile_partial_leaves_other_field() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let user = test_create_user(&conn);

        let updated = update_profile(&conn, user.id, Some("Renamed"), None).unwrap();
        assert_eq!(updated.display_name, "Renamed");
        assert_eq!(updated.email, "blake@example.com"); // untouched
    }

    #[test]
    fn update_profile_rejects_blank_name_and_bad_email() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let user = test_create_user(&conn);

        assert!(update_profile(&conn, user.id, Some("   "), None).is_err());
        assert!(update_profile(&conn, user.id, None, Some("not-an-email")).is_err());
    }

    #[test]
    fn update_profile_rejects_duplicate_email() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let _a = test_create_user(&conn);
        let b = create_user(
            &conn,
            &CreateUser {
                username: "other".into(),
                email: "other@example.com".into(),
                password: "securepassword123".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap();

        // Taking the first user's email must fail on the unique constraint.
        assert!(update_profile(&conn, b.id, None, Some("blake@example.com")).is_err());
    }

    #[test]
    fn update_password_rehashes_and_authenticates() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let user = test_create_user(&conn);

        update_password(&conn, user.id, "brand-new-password").unwrap();
        // Old password no longer works; new one does.
        assert!(authenticate(&conn, "blake", "securepassword123").is_err());
        assert!(authenticate(&conn, "blake", "brand-new-password").is_ok());
    }

    #[test]
    fn update_password_enforces_min_length() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let user = test_create_user(&conn);
        assert!(update_password(&conn, user.id, "short").is_err());
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
    fn oversized_password_rejected() {
        let pool = test_db();
        let conn = pool.write().unwrap();

        let long_pw = "a".repeat(1025);
        let result = create_user(
            &conn,
            &CreateUser {
                username: "test".into(),
                email: "test@example.com".into(),
                password: long_pw,
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("1024"));
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

    // ── create_passwordless_admin (LIFIC-9) ─────────────────

    #[test]
    fn operator_admin_is_not_a_connected_tool() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let admin = create_passwordless_admin(&conn, "Operator Blake").unwrap();

        assert!(admin.is_admin, "first admin is an admin");
        assert!(!admin.is_bot, "first admin is a person, not a connected tool");
        assert_eq!(admin.display_name, "Operator Blake");
    }

    #[test]
    fn operator_admin_resolves_as_first_admin() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let admin = create_passwordless_admin(&conn, "Operator Blake").unwrap();

        let resolved = first_admin(&conn).unwrap().expect("resolves as first admin");
        assert_eq!(resolved.id, admin.id);
        assert_eq!(resolved.username, admin.username);
    }

    #[test]
    fn operator_username_comes_from_their_name() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let admin = create_passwordless_admin(&conn, "Blake Smith").unwrap();
        assert_eq!(admin.username, "blake-smith");
    }

    #[test]
    fn same_named_operators_get_distinct_usernames() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let first = create_passwordless_admin(&conn, "Blake").unwrap();
        let second = create_passwordless_admin(&conn, "blake!").unwrap();

        assert_ne!(first.username, second.username, "usernames must not collide");
        assert!(!first.username.is_empty());
        assert!(!second.username.is_empty());
    }

    #[test]
    fn passwordless_admin_cannot_be_logged_into_by_password() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        create_passwordless_admin(&conn, "Blake").unwrap();
        // The random stored hash has no known plaintext, so password login
        // must always fail — there is no password, only passwordless identity.
        let result = authenticate(&conn, "blake", "anypassword123");
        assert!(
            result.is_err(),
            "passwordless admin must never authenticate by password"
        );
    }
}
