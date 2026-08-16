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
    // Unusable hash: never arithmetically a login password, just fills the NOT
    // NULL column. Same guarantee as `create_bot_user`.
    let password_hash = unusable_password_hash()?;
    insert_first_admin(conn, display_name, password_hash)
}

/// Create the first human admin with a real password — the `Passwords` mode of
/// the `lific init` auth-mode menu (LIFIC-25).
///
/// Same username/email derivation as [`create_passwordless_admin`], but the
/// stored hash is a real argon2 hash of `password`, so the operator can sign in
/// on the web. This is the counterpart to passwordless mode: the operator
/// still reaches the instance without an admin prompt, but through the password
/// gate rather than browser auto-login.
pub fn create_first_admin_with_password(
    conn: &Connection,
    display_name: &str,
    password: &str,
) -> Result<User, LificError> {
    let display_name = display_name.trim();
    if display_name.is_empty() {
        return Err(LificError::BadRequest(
            "operator name cannot be empty".into(),
        ));
    }
    if password.is_empty() {
        return Err(LificError::BadRequest(
            "operator password cannot be empty".into(),
        ));
    }
    let password_hash = hash_password(password)?;
    insert_first_admin(conn, display_name, password_hash)
}

/// Shared insert for the first human admin (LIFIC-22/25). Derives the unique
/// username from `display_name`, fills the NOT NULL email with a synthetic
/// `{username}@local` placeholder, and stores the given `password_hash`. Both
/// passwordless mode (unusable hash) and password mode (real argon2 hash) land
/// here, so the derivation and constraint handling live in exactly one place.
fn insert_first_admin(
    conn: &Connection,
    display_name: &str,
    password_hash: String,
) -> Result<User, LificError> {
    let username = derive_username(conn, display_name)?;

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
    crate::auth::sha256_hex(token.as_bytes())
}

/// Sweep expired session rows.
///
/// LIF-139: this used to run inside `validate_session`, which put a write on
/// the hot path of every session-authenticated request and forced the auth
/// middleware to take the exclusive writer mutex just to read. The sweep now
/// piggybacks on the session writes that already hold the writer — login
/// (`create_session`) and logout (`delete_session`) — so validation is a pure
/// read. Expiry itself is enforced by the `expires_at` predicate in
/// `validate_session`, never by this cleanup; the sweep only reclaims rows.
///
/// Best-effort: a failure here must never fail the login/logout it rides on.
fn purge_expired_sessions(conn: &Connection) {
    let _ = conn.execute(
        "DELETE FROM sessions WHERE expires_at < datetime('now')",
        [],
    );
}

/// Create a new session for a user. Returns the session with the plaintext token
/// (shown once to the client). The SHA-256 hash is stored in the database.
/// Sessions expire after `duration_hours` (default 24 * 7 = 1 week).
pub fn create_session(
    conn: &Connection,
    user_id: i64,
    duration_hours: Option<i64>,
) -> Result<Session, LificError> {
    // LIF-139: login already holds the writer — sweep expired rows here
    // instead of on every validation.
    purge_expired_sessions(conn);

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
/// exists and has not expired. The incoming plaintext token is hashed with
/// SHA-256 before lookup.
///
/// LIF-139: read-only. Expiry is enforced by the `expires_at > datetime('now')`
/// predicate below, so an expired row is rejected whether or not it has been
/// swept yet. The sweep moved to `create_session`/`delete_session`
/// (see [`purge_expired_sessions`]), which lets the auth middleware validate on
/// a pooled read connection rather than serializing on the single writer.
pub fn validate_session(conn: &Connection, token: &str) -> Result<User, LificError> {
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
    // LIF-139: logout holds the writer too — reclaim expired rows here.
    purge_expired_sessions(conn);
    Ok(())
}

/// Delete all sessions for a user.
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

/// Rebind an existing API key to a user.
///
/// LIF-391: this is no longer part of key creation. `auth::create_api_key`
/// takes the owner and writes the binding in the same insert, so nothing
/// creates a key unbound and patches it afterwards. The one remaining caller
/// is `lific key assign`, which rebinds a key that already exists.
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


// ── Bots (connected tools) ───────────────────────────────────

/// Create a bot user owned by the given human user.
/// Returns the bot user. API key creation is handled separately by the caller
/// using `auth::create_api_key`, which binds the new key to the bot.
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

/// Whether a failure is SQLite rejecting a write because it broke a constraint
/// (UNIQUE, CHECK, foreign key). LIF-367 leans on this to tell "another
/// connect got here first" apart from a genuine database failure.
fn is_constraint_violation(err: &LificError) -> bool {
    matches!(
        err,
        LificError::Database(rusqlite::Error::SqliteFailure(e, _))
            if e.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

/// Create a bot user owned by `owner_id`, with its `tool_id` set in the same
/// statement.
///
/// LIF-367: `idx_users_owner_tool` makes `(owner_id, tool_id)` unique for
/// bots, so the pair has to land atomically. Minting with `tool_id` NULL and
/// patching it in a follow-up UPDATE leaves a window in which a concurrent
/// connect sees no bot for the pair and mints a second one. Pass `None` only
/// for identities that genuinely have no tool behind them.
///
/// Every constraint the row can break — the unique username, the unique
/// (owner, tool) pair — comes back as [`LificError::BadRequest`], and that is
/// the *only* thing that produces that variant here. [`ensure_bot`] relies on
/// that to tell "somebody else already connected this tool" from a real
/// failure.
pub fn create_bot_user(
    conn: &Connection,
    owner_id: i64,
    bot_username: &str,
    display_name: &str,
    tool_id: Option<&str>,
) -> Result<crate::db::models::User, LificError> {
    let password_hash = unusable_password_hash()?;

    conn.execute(
        "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot, owner_id, tool_id)
         VALUES (?1, ?2, ?3, ?4, 0, 1, ?5, ?6)",
        params![
            bot_username,
            format!("{bot_username}@bot.local"),
            password_hash,
            display_name,
            owner_id,
            tool_id,
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

/// Find a bot by its stable (owner, tool) pairing (LIFIC-17).
///
/// This is the key that survives an owner rename, where the derived
/// `{tool}-{owner.username}` username does not.
pub fn find_bot_by_owner_and_tool(
    conn: &Connection,
    owner_id: i64,
    tool_id: &str,
) -> Result<Option<crate::db::models::User>, LificError> {
    match conn.query_row(
        "SELECT id, username, email, password_hash, display_name, is_admin, is_bot, created_at, updated_at
         FROM users WHERE owner_id = ?1 AND tool_id = ?2 AND is_bot = 1 LIMIT 1",
        params![owner_id, tool_id],
        row_to_user,
    ) {
        Ok(user) => Ok(Some(user)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Find a legacy bot (tool_id NULL, minted before LIFIC-17) by its owner and
/// its tool *prefix*.
///
/// Legacy bots were keyed by the `{tool}-{owner.username}` username, which
/// embeds the owner's name at mint time. After a rename that prefix is stale,
/// so the lookup must not depend on the current owner username — it matches on
/// the stable `owner_id` and the tool prefix alone, which a rename never
/// touches. `GLOB '{tool_id}-*'` ties the match to the exact tool prefix (tool
/// slugs are `[a-z0-9-]`, so no `*`/`?` need escaping).
pub fn find_bot_legacy_by_tool_prefix(
    conn: &Connection,
    owner_id: i64,
    tool_id: &str,
) -> Result<Option<crate::db::models::User>, LificError> {
    match conn.query_row(
        "SELECT id, username, email, password_hash, display_name, is_admin, is_bot, created_at, updated_at
         FROM users WHERE owner_id = ?1 AND is_bot = 1 AND tool_id IS NULL
              AND username GLOB ?2 LIMIT 1",
        params![owner_id, format!("{tool_id}-*")],
        row_to_user,
    ) {
        Ok(user) => Ok(Some(user)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Check if a bot has any active (non-revoked) API keys.
/// Whether a bot has standing access — an active (non-revoked) API key, or a
/// non-revoked OAuth token. Mirrors the Connected Tools "connected" state
/// (LIFIC-13): access is granted until explicitly revoked/disconnected,
/// independent of OAuth token expiry (the agent self-heals via re-auth).
/// Used to refuse re-connecting a tool that's already connected via either door.
pub fn bot_is_connected(conn: &Connection, bot_id: i64) -> Result<bool, LificError> {
    let has: bool = conn
        .query_row(
            "SELECT
                EXISTS(SELECT 1 FROM api_keys WHERE user_id = ?1 AND revoked = 0)
                OR EXISTS(SELECT 1 FROM oauth_tokens WHERE user_id = ?1 AND revoked = 0)",
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
///
/// LIFIC-17: dedupe keys on the stable `(owner_id, tool_id)` pair, not the
/// derived username, so renaming the owner never orphans the agent. Legacy
/// bots minted before the `tool_id` column existed (tool_id NULL) are found
/// by their owner and tool prefix and backfilled in place — safe even when the
/// owner renamed in the meantime, since the prefix match skips the stale owner
/// name embedded in their username.
/// Re-resolve the bot for `(owner_id, tool_id)` after a write was rejected by
/// a constraint.
///
/// LIF-367: with `idx_users_owner_tool` in place, the loser of a concurrent
/// `ensure_bot` no longer silently mints a duplicate — its write fails. That
/// is the right outcome for the data and the wrong one for the caller, who
/// asked for "the bot for this tool" and should get the winner's row rather
/// than a 500. So look the pair up again: if somebody won the race, their bot
/// is the answer. If the lookup still misses, the constraint that fired was
/// something else (most likely the derived username colliding with an
/// unrelated account), and `rejection` — the error the write actually
/// produced — stands.
fn resolve_bot_conflict(
    conn: &Connection,
    owner_id: i64,
    tool_id: &str,
    rejection: LificError,
) -> Result<User, LificError> {
    match find_bot_by_owner_and_tool(conn, owner_id, tool_id)? {
        Some(winner) => Ok(winner),
        None => Err(rejection),
    }
}

pub fn ensure_bot(
    conn: &Connection,
    owner_id: i64,
    tool_id: &str,
    display_name: &str,
) -> Result<User, LificError> {
    // Structured dedupe first: stable across owner renames.
    if let Some(existing) = find_bot_by_owner_and_tool(conn, owner_id, tool_id)? {
        return Ok(existing);
    }
    // Legacy bot: pre-migration, tool_id NULL, keyed only by owner + tool.
    // Reuse and backfill it.
    if let Some(legacy) = find_bot_legacy_by_tool_prefix(conn, owner_id, tool_id)? {
        return match conn.execute(
            "UPDATE users SET tool_id = ?1 WHERE id = ?2",
            params![tool_id, legacy.id],
        ) {
            Ok(_) => Ok(legacy),
            // A concurrent connect claimed the pair between our lookup and
            // this backfill; the legacy row stays legacy and the winner wins.
            Err(e) => {
                let e: LificError = e.into();
                if is_constraint_violation(&e) {
                    resolve_bot_conflict(conn, owner_id, tool_id, e)
                } else {
                    Err(e)
                }
            }
        };
    }
    let owner_username = get_user_by_id(conn, owner_id)?.username;
    let bot_username = format!("{tool_id}-{owner_username}");
    match create_bot_user(conn, owner_id, &bot_username, display_name, Some(tool_id)) {
        Ok(bot) => Ok(bot),
        // The one thing that makes create_bot_user return BadRequest is SQLite
        // rejecting the row, which after LIF-367 usually means a concurrent
        // connect already minted this exact agent.
        Err(e @ LificError::BadRequest(_)) => resolve_bot_conflict(conn, owner_id, tool_id, e),
        Err(e) => Err(e),
    }
}

/// List all bots owned by a specific user.
pub fn list_bots(
    conn: &Connection,
    owner_id: i64,
) -> Result<Vec<crate::db::models::Bot>, LificError> {
    let mut stmt = conn.prepare_cached(
        "SELECT u.id, u.username, u.display_name, u.owner_id, u.created_at,
                EXISTS(
                    SELECT 1 FROM api_keys k WHERE k.user_id = u.id AND k.revoked = 0
                    UNION
                    SELECT 1 FROM oauth_tokens t WHERE t.user_id = u.id AND t.revoked = 0
                ) as connected
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
            connected: row.get(5)?,
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
    // Kill in-flight OAuth handshakes too (PR #23 review): an approved but
    // not-yet-exchanged device code or auth code would otherwise mint a
    // fresh token for the bot the owner just disconnected.
    conn.execute(
        "UPDATE oauth_device_codes SET status = 'denied' \
         WHERE user_id = ?1 AND status IN ('pending', 'approved')",
        params![bot_id],
    )?;
    conn.execute(
        "UPDATE oauth_codes SET used = 1 WHERE user_id = ?1 AND used = 0",
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
    // And its in-flight OAuth handshakes (PR #23 review): a pending device or
    // auth code bound to a deleted identity must not stay exchangeable.
    conn.execute(
        "DELETE FROM oauth_device_codes WHERE user_id = ?1",
        params![bot_id],
    )?;
    conn.execute("DELETE FROM oauth_codes WHERE user_id = ?1", params![bot_id])?;

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

    // ── stable dedupe across owner rename (LIFIC-17) ──────────

    // The bot identity is keyed on (owner_id, tool_id), not the derived
    // `{tool}-{owner}` username string. Renaming the owner changes the string
    // but not the (owner_id, tool_id) pair, so a re-connect must reuse the
    // original bot rather than mint a duplicate.
    #[test]
    fn ensure_bot_reuses_existing_bot_after_owner_rename() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let owner = test_create_user(&conn); // username "blake"

        let first = ensure_bot(&conn, owner.id, "opencode", "OpenCode").unwrap().id;

        // Simulate the owner renaming their account: username changes, id stays.
        conn.execute(
            "UPDATE users SET username = ?1 WHERE id = ?2",
            params!["renamed-blake", owner.id],
        )
        .unwrap();

        let second = ensure_bot(&conn, owner.id, "opencode", "OpenCode").unwrap().id;
        assert_eq!(first, second, "renaming the owner must not orphan the agent");
    }

    // Bots minted before the tool_id column existed (tool_id NULL) are still
    // found by their legacy `{tool}-{owner}` username and backfilled, so an
    // existing install does not duplicate agents on the first post-upgrade
    // reconnect.
    #[test]
    fn ensure_bot_backfills_legacy_bot_by_username() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let owner = test_create_user(&conn); // username "blake"
        // A pre-migration bot: username "opencode-blake", tool_id NULL.
        let legacy = create_bot_user(&conn, owner.id, "opencode-blake", "OpenCode", None)
            .unwrap();

        let reused = ensure_bot(&conn, owner.id, "opencode", "OpenCode").unwrap();
        assert_eq!(
            reused.id, legacy.id,
            "a legacy bot keyed by username must be reused, not duplicated"
        );
        let stored: Option<String> = conn
            .query_row(
                "SELECT tool_id FROM users WHERE id = ?1",
                params![legacy.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored.as_deref(), Some("opencode"), "legacy bot tool_id backfilled");
    }

    // The legacy backfill holds even when the owner renamed *before* the
    // reconnect: the legacy username embeds the old owner name, so the match
    // keys on owner id + tool prefix, never the current owner username.
    #[test]
    fn ensure_bot_backfills_legacy_bot_even_after_owner_rename() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let owner = test_create_user(&conn); // username "blake"
        // A pre-migration bot whose username still has the old owner name.
        let legacy = create_bot_user(&conn, owner.id, "opencode-oldname", "OpenCode", None)
            .unwrap();
        // The owner renames before ever reconnecting.
        conn.execute(
            "UPDATE users SET username = ?1 WHERE id = ?2",
            params!["new-name", owner.id],
        )
        .unwrap();

        let reused = ensure_bot(&conn, owner.id, "opencode", "OpenCode").unwrap();
        assert_eq!(
            reused.id, legacy.id,
            "renaming before a legacy reconnect must still reuse, not duplicate"
        );
    }

    // ── (owner_id, tool_id) uniqueness in the schema (LIF-367) ───

    /// Insert a bot row straight into the table, bypassing every
    /// application-level dedupe, so the schema is the only thing that can
    /// object. Returns the raw rusqlite result.
    fn raw_insert_bot(
        conn: &Connection,
        username: &str,
        owner_id: i64,
        tool_id: Option<&str>,
    ) -> Result<usize, rusqlite::Error> {
        conn.execute(
            "INSERT INTO users (username, email, password_hash, display_name, is_admin, is_bot, owner_id, tool_id)
             VALUES (?1, ?2, 'x', 'Agent', 0, 1, ?3, ?4)",
            params![username, format!("{username}@bot.local"), owner_id, tool_id],
        )
    }

    fn is_constraint_err(err: &rusqlite::Error) -> bool {
        matches!(
            err,
            rusqlite::Error::SqliteFailure(e, _)
                if e.code == rusqlite::ErrorCode::ConstraintViolation
        )
    }

    // The pairing used to be enforced by `ensure_bot` reading before it wrote,
    // which two concurrent connects can both win. The database now refuses the
    // second row outright.
    #[test]
    fn second_bot_for_the_same_owner_and_tool_is_rejected_by_the_schema() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let owner = test_create_user(&conn);
        ensure_bot(&conn, owner.id, "opencode", "OpenCode").unwrap();

        // A distinct username, so the users.username UNIQUE is not what fires:
        // only idx_users_owner_tool can reject this.
        let err = raw_insert_bot(&conn, "opencode-blake-2", owner.id, Some("opencode"))
            .expect_err("duplicate (owner_id, tool_id) bot must be rejected");
        assert!(
            is_constraint_err(&err),
            "expected a constraint violation, got {err:?}"
        );
        let bots: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM users WHERE is_bot = 1 AND owner_id = ?1",
                params![owner.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bots, 1, "the rejected insert left no row behind");
    }

    // The index is partial on purpose. Humans and legacy bots awaiting lazy
    // backfill both carry tool_id NULL and must not collide with each other,
    // and one owner may connect any number of *different* tools.
    #[test]
    fn bot_uniqueness_ignores_null_tool_ids_and_distinct_tools() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let owner = test_create_user(&conn);

        raw_insert_bot(&conn, "legacy-one", owner.id, None).unwrap();
        raw_insert_bot(&conn, "legacy-two", owner.id, None)
            .expect("two legacy bots with tool_id NULL are allowed");
        raw_insert_bot(&conn, "opencode-blake", owner.id, Some("opencode")).unwrap();
        raw_insert_bot(&conn, "claude-code-blake", owner.id, Some("claude-code"))
            .expect("a different tool for the same owner is allowed");
    }

    // Whatever order the connects land in, the owner ends up with exactly one
    // agent for the tool and every caller gets that same identity back.
    #[test]
    fn ensure_bot_is_idempotent_across_repeated_connects() {
        let pool = test_db();
        let owner_id = {
            let conn = pool.write().unwrap();
            test_create_user(&conn).id
        };

        let ids: Vec<i64> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    let pool = pool.clone();
                    scope.spawn(move || {
                        let conn = pool.write().unwrap();
                        ensure_bot(&conn, owner_id, "opencode", "OpenCode")
                            .expect("ensure_bot must not fail on a repeat connect")
                            .id
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        assert!(
            ids.windows(2).all(|w| w[0] == w[1]),
            "every connect must resolve to the same agent, got {ids:?}"
        );
        let conn = pool.write().unwrap();
        let bots: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM users WHERE is_bot = 1 AND owner_id = ?1 AND tool_id = 'opencode'",
                params![owner_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bots, 1, "no duplicate agent for the pair");
    }

    // The recovery path itself: a mint rejected by the index means somebody
    // else already minted the pair, and the caller wants that winner, not an
    // error.
    #[test]
    fn a_rejected_mint_resolves_to_the_bot_that_won_the_race() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let owner = test_create_user(&conn);
        let winner = ensure_bot(&conn, owner.id, "opencode", "OpenCode").unwrap();

        let resolved = resolve_bot_conflict(
            &conn,
            owner.id,
            "opencode",
            LificError::BadRequest("rejected".into()),
        )
        .unwrap();
        assert_eq!(resolved.id, winner.id, "the winner's bot is the answer");
    }

    // ...but a constraint that fired for some other reason must not be
    // reported as a successful connect. Here an unrelated account already
    // holds the username the bot would take, so there is no winner to return.
    #[test]
    fn a_rejected_mint_with_no_winner_stays_an_error() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let owner = test_create_user(&conn); // "blake"
        create_user(
            &conn,
            &CreateUser {
                username: "opencode-blake".into(),
                email: "squatter@example.com".into(),
                password: "securepassword123".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap();

        let err = ensure_bot(&conn, owner.id, "opencode", "OpenCode")
            .expect_err("a username collision is still a failure");
        assert!(
            matches!(err, LificError::BadRequest(ref m) if m.contains("already connected")),
            "expected the connect-conflict message, got {err:?}"
        );
    }

    // ── migration 038: dedupe of rows minted before the index (LIF-367) ──

    // Rewinds the pool to the schema-037 shape, seeds the duplicate an
    // unguarded `ensure_bot` could produce along with rows referencing the
    // loser, then applies 038 verbatim and checks the survivor absorbed
    // everything.
    //
    // The runner (`migrate::run`) only ever applies migrations *newer* than
    // the highest recorded version, so it cannot be asked to replay one in
    // isolation; the migration SQL is applied directly instead, which is the
    // same statements in the same order.
    #[test]
    fn migration_038_keeps_the_oldest_bot_and_repoints_every_reference() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        conn.execute_batch("DROP INDEX idx_users_owner_tool;").unwrap();

        let owner = test_create_user(&conn);
        raw_insert_bot(&conn, "opencode-blake", owner.id, Some("opencode")).unwrap();
        let survivor = conn.last_insert_rowid();
        raw_insert_bot(&conn, "opencode-oldname", owner.id, Some("opencode")).unwrap();
        let loser = conn.last_insert_rowid();
        assert!(loser > survivor, "the loser is the newer row");

        // A bot for a different tool, and a legacy NULL-tool bot: both must be
        // left exactly where they are.
        raw_insert_bot(&conn, "claude-code-blake", owner.id, Some("claude-code")).unwrap();
        let untouched = conn.last_insert_rowid();

        conn.execute_batch(
            "INSERT INTO projects (name, identifier) VALUES ('Lific', 'LIF');
             INSERT INTO issues (project_id, sequence, title) VALUES (1, 1, 'An issue');",
        )
        .unwrap();

        // Every user-referencing column in the schema, pointed at the loser.
        conn.execute(
            "INSERT INTO api_keys (name, key_hash, user_id) VALUES ('k', 'h', ?1)",
            params![loser],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (token, user_id, expires_at) VALUES ('t', ?1, datetime('now', '+1 day'))",
            params![loser],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO comments (issue_id, user_id, content) VALUES (1, ?1, 'hi')",
            params![loser],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO comment_mentions (comment_id, user_id) VALUES (1, ?1)",
            params![loser],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO attachments (sha256, filename, mime, size_bytes, uploader_id)
             VALUES ('abc', 'f.png', 'image/png', 1, ?1)",
            params![loser],
        )
        .unwrap();
        conn.execute("UPDATE projects SET lead_user_id = ?1", params![loser])
            .unwrap();
        conn.execute(
            "INSERT INTO oauth_clients (client_id, client_name, redirect_uris)
             VALUES ('c', 'Test', '[\"http://localhost\"]')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO oauth_tokens (access_token, client_id, expires_at, user_id)
             VALUES ('tok', 'c', datetime('now', '+1 hour'), ?1)",
            params![loser],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO oauth_codes (code, client_id, redirect_uri, code_challenge, expires_at, user_id)
             VALUES ('code', 'c', 'http://localhost', 'ch', datetime('now', '+1 hour'), ?1)",
            params![loser],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO oauth_device_codes (device_code_hash, user_code, expires_at, user_id)
             VALUES ('dh', 'ABCD-EFGH', datetime('now', '+1 hour'), ?1)",
            params![loser],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO audit_log (actor_user_id, transport, entity_type, entity_id, action, field)
             VALUES (?1, 'mcp', 'issue', 1, 'create', 'seeded')",
            params![loser],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO saved_views (project_id, user_id, name, config) VALUES (1, ?1, 'Mine', '{}')",
            params![loser],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO project_groups (user_id, name) VALUES (?1, 'Work')",
            params![loser],
        )
        .unwrap();
        // Both rows are members of the same project, at different roles, and
        // the *loser* holds the stronger one. Collapsing the pair must keep
        // the privilege, not whichever row happened to survive.
        conn.execute(
            "INSERT INTO project_members (project_id, user_id, role) VALUES (1, ?1, 'viewer')",
            params![survivor],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO project_members (project_id, user_id, role) VALUES (1, ?1, 'lead')",
            params![loser],
        )
        .unwrap();

        conn.execute_batch(include_str!("../../../migrations/038_bot_identity_unique.sql"))
            .unwrap();

        // The loser is gone, the survivor and the unrelated bot are not.
        let remaining: Vec<i64> = conn
            .prepare("SELECT id FROM users WHERE is_bot = 1 ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(remaining, vec![survivor, untouched]);

        let owns = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(owns("SELECT user_id FROM api_keys"), survivor);
        assert_eq!(owns("SELECT user_id FROM sessions"), survivor);
        assert_eq!(owns("SELECT user_id FROM comments"), survivor);
        assert_eq!(owns("SELECT user_id FROM comment_mentions"), survivor);
        assert_eq!(owns("SELECT uploader_id FROM attachments"), survivor);
        assert_eq!(owns("SELECT lead_user_id FROM projects"), survivor);
        assert_eq!(owns("SELECT user_id FROM oauth_tokens"), survivor);
        assert_eq!(owns("SELECT user_id FROM oauth_codes"), survivor);
        assert_eq!(owns("SELECT user_id FROM oauth_device_codes"), survivor);
        assert_eq!(
            owns("SELECT actor_user_id FROM audit_log WHERE field = 'seeded'"),
            survivor
        );
        assert_eq!(owns("SELECT user_id FROM saved_views"), survivor);
        assert_eq!(owns("SELECT user_id FROM project_groups"), survivor);
        assert_eq!(
            owns(&format!(
                "SELECT COUNT(*) FROM audit_log WHERE actor_user_id = {loser}"
            )),
            0,
            "nothing is still attributed to the deleted row"
        );

        // One membership left, carrying the stronger of the two roles.
        let members: Vec<(i64, String)> = conn
            .prepare("SELECT user_id, role FROM project_members")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(members, vec![(survivor, "lead".to_string())]);

        // And the constraint is now in force.
        let err = raw_insert_bot(&conn, "opencode-third", owner.id, Some("opencode"))
            .expect_err("038 leaves the pair unique");
        assert!(is_constraint_err(&err), "got {err:?}");
    }

    // Where a row cannot simply be repointed because the survivor already
    // holds one for the same unique key, nothing may be silently thrown away:
    // roles merge upward, group items are reparented, and views that only
    // share a name are renamed rather than dropped. Three duplicate bots, so
    // the loser-versus-loser collisions are covered too.
    #[test]
    fn migration_038_merges_colliding_rows_instead_of_dropping_them() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        conn.execute_batch("DROP INDEX idx_users_owner_tool;").unwrap();

        let owner = test_create_user(&conn);
        raw_insert_bot(&conn, "opencode-blake", owner.id, Some("opencode")).unwrap();
        let survivor = conn.last_insert_rowid();
        raw_insert_bot(&conn, "opencode-second", owner.id, Some("opencode")).unwrap();
        let loser_a = conn.last_insert_rowid();
        raw_insert_bot(&conn, "opencode-third", owner.id, Some("opencode")).unwrap();
        let loser_b = conn.last_insert_rowid();

        conn.execute_batch(
            "INSERT INTO projects (name, identifier) VALUES ('One', 'ONE'), ('Two', 'TWO');",
        )
        .unwrap();

        // Roles: the survivor is a viewer where a loser leads, and a loser
        // holds the only membership of the second project.
        for (project, user, role) in [
            (1, survivor, "viewer"),
            (1, loser_a, "lead"),
            (2, loser_b, "maintainer"),
        ] {
            conn.execute(
                "INSERT INTO project_members (project_id, user_id, role) VALUES (?1, ?2, ?3)",
                params![project, user, role],
            )
            .unwrap();
        }

        // Groups: 'Work' exists three times over, each holding items; 'Solo'
        // belongs to a loser alone.
        for (id, user, name) in [
            (100, survivor, "Work"),
            (200, loser_a, "Work"),
            (300, loser_a, "Solo"),
            (400, loser_b, "Work"),
        ] {
            conn.execute(
                "INSERT INTO project_groups (id, user_id, name) VALUES (?1, ?2, ?3)",
                params![id, user, name],
            )
            .unwrap();
        }
        conn.execute_batch(
            "INSERT INTO project_group_items (group_id, project_id)
             VALUES (100, 1), (200, 1), (200, 2), (300, 2), (400, 2);",
        )
        .unwrap();

        // Views: one name, three different configs, plus a loser-only view.
        for (id, user, name, config) in [
            (1, survivor, "Mine", "{\"a\":1}"),
            (2, loser_a, "Mine", "{\"b\":2}"),
            (3, loser_a, "Solo", "{}"),
            (4, loser_b, "Mine", "{\"c\":3}"),
        ] {
            conn.execute(
                "INSERT INTO saved_views (id, project_id, user_id, name, config)
                 VALUES (?1, 1, ?2, ?3, ?4)",
                params![id, user, name, config],
            )
            .unwrap();
        }

        conn.execute_batch(include_str!("../../../migrations/038_bot_identity_unique.sql"))
            .unwrap();

        // The strongest role wins per project; nothing is dropped.
        let members: Vec<(i64, i64, String)> = conn
            .prepare("SELECT project_id, user_id, role FROM project_members ORDER BY project_id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            members,
            vec![
                (1, survivor, "lead".to_string()),
                (2, survivor, "maintainer".to_string()),
            ]
        );

        // The three 'Work' groups collapse into the survivor's, and every
        // project that was in any of them is still in the one that remains.
        let groups: Vec<(i64, i64, String)> = conn
            .prepare("SELECT id, user_id, name FROM project_groups ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            groups,
            vec![
                (100, survivor, "Work".to_string()),
                (300, survivor, "Solo".to_string()),
            ]
        );
        let items: Vec<(i64, i64)> = conn
            .prepare("SELECT group_id, project_id FROM project_group_items ORDER BY group_id, project_id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            items,
            vec![(100, 1), (100, 2), (300, 2)],
            "the reparented item survived and the duplicate collapsed"
        );

        // Every view survives with its own config; the name clash is resolved
        // by suffixing the newer rows, not by deleting them.
        let views: Vec<(i64, i64, String, String)> = conn
            .prepare("SELECT id, user_id, name, config FROM saved_views ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            views,
            vec![
                (1, survivor, "Mine".to_string(), "{\"a\":1}".to_string()),
                (
                    2,
                    survivor,
                    "Mine (merged 2)".to_string(),
                    "{\"b\":2}".to_string()
                ),
                (3, survivor, "Solo".to_string(), "{}".to_string()),
                (
                    4,
                    survivor,
                    "Mine (merged 4)".to_string(),
                    "{\"c\":3}".to_string()
                ),
            ]
        );

        // Both losers are gone and no reference dangles.
        let bots: Vec<i64> = conn
            .prepare("SELECT id FROM users WHERE is_bot = 1")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(bots, vec![survivor], "{loser_a} and {loser_b} merged away");
        let dangling: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(dangling, 0, "no foreign key left pointing at a deleted row");
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

    /// Seed an in-flight OAuth handshake pair (approved device code + unused
    /// auth code) bound to `user_id`, for the disconnect/delete revocation
    /// tests (PR #23 review).
    fn insert_pending_handshakes_for(conn: &Connection, user_id: i64) {
        conn.execute(
            "INSERT INTO oauth_device_codes
                (device_code_hash, user_code, expires_at, status, user_id)
             VALUES ('devhash', 'BCDF-GHJK', datetime('now', '+1 hour'), 'approved', ?1)",
            params![user_id],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO oauth_clients (client_id, client_name, redirect_uris)
             VALUES ('c1', 'Test', '[]')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO oauth_codes
                (code, client_id, redirect_uri, code_challenge, expires_at, user_id)
             VALUES ('code1', 'c1', 'http://localhost/cb', 'ch', datetime('now', '+1 hour'), ?1)",
            params![user_id],
        )
        .unwrap();
    }

    #[test]
    fn disconnect_bot_kills_in_flight_oauth_handshakes() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let owner = test_create_user(&conn);
        let bot = ensure_bot(&conn, owner.id, "claude-code", "Claude Code").unwrap();
        insert_pending_handshakes_for(&conn, bot.id);

        disconnect_bot(&conn, bot.id, owner.id, false).unwrap();

        let device_status: String = conn
            .query_row(
                "SELECT status FROM oauth_device_codes WHERE user_id = ?1",
                params![bot.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(device_status, "denied", "approved device code denied");
        let code_used: i64 = conn
            .query_row(
                "SELECT used FROM oauth_codes WHERE user_id = ?1",
                params![bot.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(code_used, 1, "pending auth code burned");
    }

    #[test]
    fn delete_bot_removes_in_flight_oauth_handshakes() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let owner = test_create_user(&conn);
        let bot = ensure_bot(&conn, owner.id, "opencode", "OpenCode").unwrap();
        insert_pending_handshakes_for(&conn, bot.id);

        delete_bot(&conn, bot.id, owner.id, false).unwrap();

        let device_rows: usize = conn
            .query_row(
                "SELECT COUNT(*) FROM oauth_device_codes WHERE user_id = ?1",
                params![bot.id],
                |r| r.get(0),
            )
            .unwrap();
        let code_rows: usize = conn
            .query_row(
                "SELECT COUNT(*) FROM oauth_codes WHERE user_id = ?1",
                params![bot.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            (device_rows, code_rows),
            (0, 0),
            "no exchangeable handshakes survive bot deletion"
        );
    }

    // ── list_bots / connected semantics (LIFIC-13 OAuth bots) ──

    #[test]
    fn bot_with_oauth_token_lists_as_connected() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let owner = test_create_user(&conn);
        let bot = ensure_bot(&conn, owner.id, "opencode", "OpenCode").unwrap();
        // No API key — connected purely by an OAuth token (LIFIC-13 path).
        insert_oauth_token_for(&conn, bot.id);

        let listed = list_bots(&conn, owner.id).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, bot.id);
        assert!(
            listed[0].connected,
            "an OAuth-connected bot must list as connected (no API key involved)"
        );
        assert!(bot_is_connected(&conn, bot.id).unwrap());
    }

    #[test]
    fn bot_with_only_revoked_credentials_lists_as_disconnected() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let owner = test_create_user(&conn);
        let bot = ensure_bot(&conn, owner.id, "opencode", "OpenCode").unwrap();
        insert_oauth_token_for(&conn, bot.id);
        // Revoke the token — the bot is no longer connected.
        conn.execute(
            "UPDATE oauth_tokens SET revoked = 1 WHERE user_id = ?1",
            params![bot.id],
        )
        .unwrap();

        let listed = list_bots(&conn, owner.id).unwrap();
        assert!(
            !listed[0].connected,
            "a bot with only revoked credentials must list as disconnected"
        );
        assert!(!bot_is_connected(&conn, bot.id).unwrap());
    }

    #[test]
    fn api_key_connected_bot_still_lists_as_connected() {
        let pool = test_db();
        let (owner, bot) = {
            let conn = pool.write().unwrap();
            let owner = test_create_user(&conn);
            let bot = ensure_bot(&conn, owner.id, "claude-code", "Claude Code").unwrap();
            (owner.id, bot.id)
        };
        // The classic `lific connect` path: an active API key, no OAuth token.
        let name = format!("claude-code-{}", {
            let conn = pool.read().unwrap();
            get_user_by_id(&conn, owner).unwrap().username
        });
        let manager = crate::auth::create_key_manager().unwrap();
        let _ = crate::auth::create_api_key(&pool, &manager, &name, Some(bot)).unwrap();

        let listed = {
            let conn = pool.read().unwrap();
            list_bots(&conn, owner).unwrap()
        };
        assert!(
            listed.iter().any(|b| b.id == bot && b.connected),
            "API-key-connected bot (legacy path) still lists as connected"
        );
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

    // ── LIF-139: validation is read-only, cleanup rides the writers ──

    /// Insert an already-expired session row directly and return its token.
    fn insert_expired_session(conn: &Connection, user_id: i64) -> String {
        let token = generate_session_token();
        conn.execute(
            "INSERT INTO sessions (token, user_id, expires_at)
             VALUES (?1, ?2, datetime('now', '-1 hour'))",
            params![hash_session_token(&token), user_id],
        )
        .unwrap();
        token
    }

    fn session_row_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .unwrap()
    }

    // The expiry check lives in the SELECT, not in a cleanup DELETE. An
    // expired token must be refused even while its row is still on disk.
    #[test]
    fn expired_session_rejected_without_being_swept_first() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let user = test_create_user(&conn);
        let token = insert_expired_session(&conn, user.id);

        assert!(
            validate_session(&conn, &token).is_err(),
            "an expired session must be rejected on the predicate alone"
        );
        assert_eq!(
            session_row_count(&conn),
            1,
            "validation must not write — the expired row is still there"
        );
        // Still rejected on a second look, i.e. the first call didn't rely on
        // having deleted the row.
        assert!(validate_session(&conn, &token).is_err());
    }

    // Login already holds the writer, so it is where expired rows get reaped.
    #[test]
    fn creating_a_session_purges_expired_rows() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let user = test_create_user(&conn);
        insert_expired_session(&conn, user.id);
        assert_eq!(session_row_count(&conn), 1);

        let fresh = create_session(&conn, user.id, None).unwrap();

        assert_eq!(
            session_row_count(&conn),
            1,
            "login sweeps the expired row, leaving only the new session"
        );
        assert!(
            validate_session(&conn, &fresh.token).is_ok(),
            "the freshly minted session survives the sweep"
        );
    }

    // Logout is the other writer-holding touchpoint.
    #[test]
    fn deleting_a_session_purges_expired_rows() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let user = test_create_user(&conn);
        let live = create_session(&conn, user.id, None).unwrap();
        insert_expired_session(&conn, user.id);
        assert_eq!(session_row_count(&conn), 2);

        delete_session(&conn, &live.token).unwrap();

        assert_eq!(
            session_row_count(&conn),
            0,
            "logout removes its own session and sweeps expired ones"
        );
    }

    // The middleware now validates on a pooled read connection, which is
    // read-only at the SQLite level: a stray write would surface as an error
    // rather than a silent no-op.
    #[test]
    fn session_validates_over_a_read_connection() {
        let pool = test_db();
        let (user_id, token) = {
            let conn = pool.write().unwrap();
            let user = test_create_user(&conn);
            let session = create_session(&conn, user.id, None).unwrap();
            (user.id, session.token)
        };

        let conn = pool.read().unwrap();
        let validated = validate_session(&conn, &token).expect("read-only validation works");
        assert_eq!(validated.id, user_id);
        assert!(validate_session(&conn, "lific_sess_nope").is_err());
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

        let owner = |conn: &Connection| -> Option<i64> {
            conn.query_row(
                "SELECT user_id FROM api_keys WHERE id = ?1",
                params![key_id],
                |row| row.get(0),
            )
            .unwrap()
        };

        // Before assignment: no user
        assert!(owner(&conn).is_none());

        // Assign
        assign_key_to_user(&conn, "opencode", user.id).unwrap();

        // After assignment: the key points at the user
        assert_eq!(owner(&conn), Some(user.id));
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

    // ── create_first_admin_with_password (LIFIC-25) ──────────

    #[test]
    fn password_admin_is_admin_and_authenticates() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let admin = create_first_admin_with_password(&conn, "Blake Smith", "hunter22").unwrap();

        assert!(admin.is_admin, "first admin is an admin");
        assert_eq!(admin.username, "blake-smith");
        assert!(!admin.is_bot);
        let got = authenticate(&conn, "blake-smith", "hunter22").unwrap();
        assert_eq!(got.id, admin.id, "correct password logs in as the admin");
    }

    #[test]
    fn password_admin_rejects_wrong_password() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        create_first_admin_with_password(&conn, "Blake", "correcthorse1").unwrap();
        assert!(
            authenticate(&conn, "blake", "wrongpassword").is_err(),
            "wrong password must be rejected"
        );
    }

    #[test]
    fn password_admin_rejects_empty_password() {
        let pool = test_db();
        let conn = pool.write().unwrap();
        let err = create_first_admin_with_password(&conn, "Blake", "").unwrap_err();
        assert!(
            matches!(err, LificError::BadRequest(_)),
            "an empty password must be rejected, got {err:?}"
        );
    }
}
