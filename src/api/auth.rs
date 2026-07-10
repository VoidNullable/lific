use axum::{
    Extension,
    extract::{Json, Path, State},
    http::HeaderMap,
    response::IntoResponse,
};

use crate::db::{DbPool, models::*};
use crate::error::LificError;

use super::{require_admin, with_read, with_write};

/// Build a Set-Cookie header for the session token with security flags.
///
/// LIF-207: `secure` gates the `Secure` attribute. It's on by default and only
/// disabled for an explicitly-`http://` deployment, because browsers silently
/// drop a `Secure` cookie over plain HTTP — which would break the OAuth approve
/// flow (the one place the cookie is actually read) on a local-first install.
fn session_cookie(token: &str, expires_at: &str, secure: bool) -> String {
    use chrono::DateTime;
    // Parse expiry for Max-Age calculation; fall back to 30 days
    let max_age = DateTime::parse_from_rfc3339(expires_at)
        .map(|exp| {
            let exp_utc: DateTime<chrono::Utc> = exp.into();
            (exp_utc - chrono::Utc::now()).num_seconds().max(0)
        })
        .unwrap_or(30 * 24 * 3600);

    let secure_attr = if secure { "; Secure" } else { "" };
    format!("lific_token={token}; Path=/; Max-Age={max_age}; HttpOnly{secure_attr}; SameSite=Lax")
}

/// Build the Set-Cookie that clears the session cookie. Mirrors the `Secure`
/// flag of the set path so the browser reliably matches and removes it.
fn clear_cookie(secure: bool) -> String {
    let secure_attr = if secure { "; Secure" } else { "" };
    format!("lific_token=; Path=/; Max-Age=0; HttpOnly{secure_attr}; SameSite=Lax")
}

// ── Auth endpoints ───────────────────────────────────────────

/// Public signup request — intentionally excludes is_admin and is_bot
/// to prevent privilege escalation. Those can only be set via CLI.
#[derive(serde::Deserialize)]
pub(super) struct SignupRequest {
    username: String,
    email: String,
    password: String,
    display_name: Option<String>,
}

pub(super) async fn auth_signup(
    State(db): State<DbPool>,
    Extension(auth_cfg): Extension<crate::config::AuthConfig>,
    limiter: Option<Extension<std::sync::Arc<crate::ratelimit::RateLimiter>>>,
    headers: HeaderMap,
    Json(input): Json<SignupRequest>,
) -> Result<impl IntoResponse, LificError> {
    // Rate limit signups to prevent Argon2 CPU exhaustion. Key on TWO things
    // (LIF-138): the email AND the source IP. The attacker chooses the email,
    // so an email-only key is bypassed by rotating addresses — each request
    // still costing a full Argon2 hash. The per-IP key (same helper login
    // uses) is what actually caps the DoS. `check` records on pass and the
    // `||` short-circuits, so a rejected attempt never double-charges.
    let email_key = format!("signup:{}", input.email.to_lowercase());
    let ip_key = format!("signup_ip:{}", crate::ratelimit::client_ip(&headers));
    if let Some(Extension(ref rl)) = limiter
        && (!rl.check(&email_key) || !rl.check(&ip_key))
    {
        let retry = rl.retry_after(&email_key).max(rl.retry_after(&ip_key));
        return Err(LificError::BadRequest(format!(
            "too many signup attempts — try again in {retry} seconds"
        )));
    }

    let conn = db.write()?;
    let settings = crate::db::queries::settings::get(&conn)?;

    // Signup policy is now DB-backed (admin-editable), not TOML.
    if !settings.allow_signup {
        return Err(LificError::BadRequest(
            "signups are closed on this instance. Ask an admin to create your account.".into(),
        ));
    }
    // Optional email-domain allowlist for self-service signup.
    if !settings.signup_email_domains.is_empty() {
        let domain = input
            .email
            .rsplit('@')
            .next()
            .unwrap_or("")
            .trim()
            .to_lowercase();
        if !settings.signup_email_domains.contains(&domain) {
            return Err(LificError::BadRequest(format!(
                "signups on this instance are limited to: {}",
                settings.signup_email_domains.join(", ")
            )));
        }
    }

    let user = crate::db::queries::users::create_user(
        &conn,
        &CreateUser {
            username: input.username,
            email: input.email,
            password: input.password,
            display_name: input.display_name,
            is_admin: false,
            is_bot: false,
        },
    )?;
    let session = crate::db::queries::users::create_session(
        &conn,
        user.id,
        Some(settings.session_lifetime_days * 24),
    )?;

    let mut headers = HeaderMap::new();
    headers.insert(
        "set-cookie",
        session_cookie(&session.token, &session.expires_at, auth_cfg.secure_cookies)
            .parse()
            .unwrap(),
    );

    Ok((
        headers,
        Json(serde_json::json!({
            "user": {
                "id": user.id,
                "username": user.username,
                "email": user.email,
                "display_name": user.display_name,
                "is_admin": user.is_admin,
            },
            "token": session.token,
            "expires_at": session.expires_at,
        })),
    ))
}

pub(super) async fn auth_login(
    State(db): State<DbPool>,
    Extension(auth_cfg): Extension<crate::config::AuthConfig>,
    limiter: Option<Extension<std::sync::Arc<crate::ratelimit::RateLimiter>>>,
    headers: HeaderMap,
    Json(input): Json<LoginRequest>,
) -> Result<impl IntoResponse, LificError> {
    // Rate limit logins on TWO independent keys (LIF-75):
    //   • per-identity — slows targeted credential guessing for one account
    //   • per-IP       — stops one host from spraying many usernames, and
    //                    keeps a single attacker from being the only thing
    //                    needed to lock a victim out
    // We peek() (non-recording) here and record exactly one failure per
    // failed attempt below, so a failed login costs one slot, not two — the
    // old code called check() (records on pass) *and* record_failure(),
    // halving the effective limit.
    let id_key = format!("login_id:{}", input.identity.to_lowercase());
    let ip_key = format!("login_ip:{}", crate::ratelimit::client_ip(&headers));
    if let Some(Extension(ref rl)) = limiter
        && (!rl.peek(&id_key) || !rl.peek(&ip_key))
    {
        let retry = rl.retry_after(&id_key).max(rl.retry_after(&ip_key));
        return Err(LificError::BadRequest(format!(
            "too many login attempts — try again in {retry} seconds"
        )));
    }

    let conn = db.write()?;
    let user =
        match crate::db::queries::users::authenticate(&conn, &input.identity, &input.password) {
            Ok(u) => u,
            Err(e) => {
                // Record one failure against both the identity and IP buckets.
                if let Some(Extension(ref rl)) = limiter {
                    rl.record_failure(&id_key);
                    rl.record_failure(&ip_key);
                }
                return Err(e);
            }
        };
    let lifetime_days = crate::db::queries::settings::get(&conn)
        .map(|s| s.session_lifetime_days)
        .unwrap_or(30);
    let session =
        crate::db::queries::users::create_session(&conn, user.id, Some(lifetime_days * 24))?;

    let mut headers = HeaderMap::new();
    headers.insert(
        "set-cookie",
        session_cookie(&session.token, &session.expires_at, auth_cfg.secure_cookies)
            .parse()
            .unwrap(),
    );

    Ok((
        headers,
        Json(serde_json::json!({
            "user": {
                "id": user.id,
                "username": user.username,
                "email": user.email,
                "display_name": user.display_name,
                "is_admin": user.is_admin,
            },
            "token": session.token,
            "expires_at": session.expires_at,
        })),
    ))
}

/// POST /api/auth/auto-login — single-user mode (LIF-215).
///
/// When the instance has `web_auto_login` enabled, mint a session for the
/// first admin account *without a password* so the web UI can sign in
/// automatically and a solo operator never sees a login screen. Returns the
/// same shape as `/api/auth/login`.
///
/// SECURITY: this endpoint is unauthenticated by design — it is the thing that
/// *produces* a session — so the **only** gate is the instance flag. It is
/// therefore default-deny: `Forbidden` whenever `web_auto_login` is off. It is
/// also strictly a browser convenience; REST and MCP still require real bearer
/// tokens. On a publicly-reachable instance this is equivalent to handing
/// admin to anyone who can load the page, which is why it is off by default and
/// surfaced with a warning in the admin UI.
pub(super) async fn auth_auto_login(
    State(db): State<DbPool>,
    Extension(auth_cfg): Extension<crate::config::AuthConfig>,
) -> Result<impl IntoResponse, LificError> {
    let conn = db.write()?;
    let settings = crate::db::queries::settings::get(&conn)?;
    if !settings.web_auto_login {
        return Err(LificError::Forbidden(
            "single-user auto-login is not enabled on this instance".into(),
        ));
    }

    let admin = crate::db::queries::users::first_admin(&conn)?
        .ok_or_else(|| LificError::BadRequest("no admin account exists to sign in as".into()))?;

    let session = crate::db::queries::users::create_session(
        &conn,
        admin.id,
        Some(settings.session_lifetime_days * 24),
    )?;

    let mut headers = HeaderMap::new();
    headers.insert(
        "set-cookie",
        session_cookie(&session.token, &session.expires_at, auth_cfg.secure_cookies)
            .parse()
            .unwrap(),
    );

    Ok((
        headers,
        Json(serde_json::json!({
            "user": {
                "id": admin.id,
                "username": admin.username,
                "display_name": admin.display_name,
                "is_admin": admin.is_admin,
            },
            "token": session.token,
            "expires_at": session.expires_at,
        })),
    ))
}

pub(super) async fn auth_logout(
    State(db): State<DbPool>,
    Extension(auth_cfg): Extension<crate::config::AuthConfig>,
    headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse, LificError> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v: &str| v.strip_prefix("Bearer "))
        .map(|s: &str| s.trim())
        .ok_or_else(|| LificError::BadRequest("missing authorization header".into()))?;

    if token.starts_with("lific_sess_") {
        let conn = db.write()?;
        crate::db::queries::users::delete_session(&conn, token)?;
    }

    // Clear the session cookie
    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        "set-cookie",
        clear_cookie(auth_cfg.secure_cookies).parse().unwrap(),
    );

    Ok((resp_headers, Json(serde_json::json!({"logged_out": true}))))
}

/// GET /api/instance — public instance metadata for the auth screen.
///
/// Unauthenticated by design: it gates what the login/signup page can show
/// BEFORE anyone has a session. It returns only non-sensitive booleans, never
/// any user data:
///   - `allow_signup`: whether self-service signup is open (so the signup page
///     can show a real "ask an admin" state instead of submitting then erroring)
///   - `has_users`: whether any human account exists yet (so signup can say
///     "be the first account" vs "join this instance" without ever claiming the
///     new account owns or administers the instance — admin is granted out of
///     band via the CLI, never by web signup).
pub(super) async fn instance_info(
    State(db): State<DbPool>,
) -> Result<Json<serde_json::Value>, LificError> {
    let (settings, has_users) = with_read(&db, |conn| {
        let settings = crate::db::queries::settings::get(conn)?;
        let has_users = crate::db::queries::users::has_human_users(conn)?;
        Ok((settings, has_users))
    })?;
    // Public surface: only non-sensitive fields the auth screen needs. The
    // domain allowlist and session lifetime stay behind the admin endpoint.
    Ok(Json(serde_json::json!({
        "allow_signup": settings.allow_signup,
        "has_users": has_users,
        "instance_name": settings.instance_name,
        "login_message": settings.login_message,
        // LIF-215: tells the unauthenticated web app to silently sign in as the
        // admin (single-user mode) instead of showing the login form.
        "web_auto_login": settings.web_auto_login,
    })))
}

/// Full instance settings JSON (admin surface).
fn settings_json(s: &crate::db::queries::settings::InstanceSettings) -> serde_json::Value {
    serde_json::json!({
        "allow_signup": s.allow_signup,
        "instance_name": s.instance_name,
        "signup_email_domains": s.signup_email_domains,
        "session_lifetime_days": s.session_lifetime_days,
        "login_message": s.login_message,
        "web_auto_login": s.web_auto_login,
        // LIF-197: the operator toggle for epic LIF-194's project-scoped
        // authorization (src/authz.rs). Off by default — see that module's
        // doc comment for the full legacy-vs-enforced mode split.
        "authz_enforced": s.authz_enforced,
    })
}

/// GET /api/instance/settings — full settings, admin only.
pub(super) async fn instance_settings_get(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
) -> Result<Json<serde_json::Value>, LificError> {
    require_admin(&auth_user)?;
    let s = with_read(&db, crate::db::queries::settings::get)?;
    Ok(Json(settings_json(&s)))
}

#[derive(serde::Deserialize)]
pub(super) struct InstanceSettingsPatchReq {
    allow_signup: Option<bool>,
    instance_name: Option<String>,
    signup_email_domains: Option<Vec<String>>,
    session_lifetime_days: Option<i64>,
    login_message: Option<String>,
    web_auto_login: Option<bool>,
    /// LIF-197: the operator toggle for LIF-194's project-scoped
    /// authorization. Off by default; flipping it takes effect on the very
    /// next request (see `src/authz.rs`'s runtime-read doc comment).
    authz_enforced: Option<bool>,
}

/// PATCH /api/instance/settings — partial update, admin only.
pub(super) async fn instance_settings_patch(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<InstanceSettingsPatchReq>,
) -> Result<Json<serde_json::Value>, LificError> {
    require_admin(&auth_user)?;
    let patch = crate::db::queries::settings::InstanceSettingsPatch {
        allow_signup: input.allow_signup,
        instance_name: input.instance_name,
        signup_email_domains: input.signup_email_domains,
        session_lifetime_days: input.session_lifetime_days,
        login_message: input.login_message,
        web_auto_login: input.web_auto_login,
        authz_enforced: input.authz_enforced,
    };
    let s = with_write(&db, move |conn| {
        crate::db::queries::settings::update(conn, patch)
    })?;
    Ok(Json(settings_json(&s)))
}

pub(super) async fn auth_me(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
) -> Result<Json<serde_json::Value>, LificError> {
    let user = auth_user
        .ok_or_else(|| LificError::BadRequest("no user associated with this token".into()))?;

    // Fetch full user from DB to get all fields (email, etc.)
    let full = with_read(&db, |conn| {
        crate::db::queries::users::get_user_by_id(conn, user.id)
    })?;

    Ok(Json(serde_json::json!({
        "id": full.id,
        "username": full.username,
        "email": full.email,
        "display_name": full.display_name,
        "is_admin": full.is_admin,
    })))
}

#[derive(serde::Deserialize)]
pub(super) struct UpdateMeRequest {
    display_name: Option<String>,
    email: Option<String>,
}

/// PATCH /api/auth/me — update the signed-in user's profile (display name,
/// email). LIF-190.
pub(super) async fn update_me(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<UpdateMeRequest>,
) -> Result<Json<serde_json::Value>, LificError> {
    let user = auth_user.ok_or_else(|| LificError::BadRequest("authentication required".into()))?;
    let full = with_write(&db, |conn| {
        crate::db::queries::users::update_profile(
            conn,
            user.id,
            input.display_name.as_deref(),
            input.email.as_deref(),
        )
    })?;
    Ok(Json(serde_json::json!({
        "id": full.id,
        "username": full.username,
        "email": full.email,
        "display_name": full.display_name,
        "is_admin": full.is_admin,
    })))
}

#[derive(serde::Deserialize)]
pub(super) struct ChangePasswordRequest {
    current_password: String,
    new_password: String,
}

/// POST /api/auth/me/password — change password after verifying the current
/// one. LIF-190.
///
/// LIF-205: a password change invalidates **all** of the user's sessions
/// (the "I've been compromised, lock it down" expectation), then mints a
/// fresh session for the current browser so the legitimate caller stays
/// logged in instead of being bounced to /login. Any stolen `lific_sess_`
/// token is dead the moment this returns.
pub(super) async fn change_password(
    State(db): State<DbPool>,
    Extension(auth_cfg): Extension<crate::config::AuthConfig>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<ChangePasswordRequest>,
) -> Result<impl IntoResponse, LificError> {
    let user = auth_user.ok_or_else(|| LificError::BadRequest("authentication required".into()))?;
    let session = with_write(&db, |conn| {
        let full = crate::db::queries::users::get_user_by_id(conn, user.id)?;
        let ok = crate::db::queries::users::verify_password(
            &input.current_password,
            &full.password_hash,
        )?;
        if !ok {
            return Err(LificError::BadRequest(
                "current password is incorrect".into(),
            ));
        }
        crate::db::queries::users::update_password(conn, user.id, &input.new_password)?;
        // Kill every existing session (including any an attacker holds), then
        // issue a fresh one for this browser.
        crate::db::queries::users::delete_all_sessions(conn, user.id)?;
        crate::db::queries::users::create_session(conn, user.id, None)
    })?;

    let mut headers = HeaderMap::new();
    headers.insert(
        "set-cookie",
        session_cookie(&session.token, &session.expires_at, auth_cfg.secure_cookies)
            .parse()
            .unwrap(),
    );

    Ok((
        headers,
        Json(serde_json::json!({
            "ok": true,
            "token": session.token,
            "expires_at": session.expires_at,
        })),
    ))
}

/// DELETE /api/auth/me/sessions — sign out of every session (this one too).
/// Clears the cookie so the current browser drops to logged-out. LIF-190.
pub(super) async fn revoke_all_sessions(
    State(db): State<DbPool>,
    Extension(auth_cfg): Extension<crate::config::AuthConfig>,
    Extension(auth_user): Extension<Option<AuthUser>>,
) -> Result<impl IntoResponse, LificError> {
    let user = auth_user.ok_or_else(|| LificError::BadRequest("authentication required".into()))?;
    with_write(&db, |conn| {
        crate::db::queries::users::delete_all_sessions(conn, user.id)
    })?;
    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        "set-cookie",
        clear_cookie(auth_cfg.secure_cookies).parse().unwrap(),
    );
    Ok((resp_headers, Json(serde_json::json!({ "revoked": true }))))
}

// ── Key management endpoints ─────────────────────────────────

pub(super) async fn list_keys(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
) -> Result<Json<Vec<UserApiKey>>, LificError> {
    let user = auth_user.ok_or_else(|| LificError::BadRequest("authentication required".into()))?;

    with_read(&db, |conn| {
        crate::db::queries::users::list_user_keys(conn, user.id)
    })
    .map(Json)
}

#[derive(serde::Deserialize)]
pub(super) struct CreateKeyRequest {
    name: String,
}

pub(super) async fn create_key(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Extension(manager): Extension<std::sync::Arc<api_keys_simplified::ApiKeyManagerV0>>,
    Json(input): Json<CreateKeyRequest>,
) -> Result<Json<serde_json::Value>, LificError> {
    let user = auth_user.ok_or_else(|| LificError::BadRequest("authentication required".into()))?;

    let name = input.name.trim().to_string();
    if name.is_empty() {
        return Err(LificError::BadRequest("key name cannot be empty".into()));
    }

    // Create the key and assign it to the user in one go
    let plaintext = crate::auth::create_api_key(&db, &manager, &name)?;
    let conn = db.write()?;
    crate::db::queries::users::assign_key_to_user(&conn, &name, user.id)?;

    Ok(Json(serde_json::json!({
        "name": name,
        "key": plaintext,
    })))
}

pub(super) async fn revoke_key(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Extension(auth_user): Extension<Option<AuthUser>>,
) -> Result<Json<serde_json::Value>, LificError> {
    let user = auth_user.ok_or_else(|| LificError::BadRequest("authentication required".into()))?;

    let conn = db.write()?;
    crate::db::queries::users::revoke_user_key(&conn, id, user.id, user.is_admin)?;

    Ok(Json(serde_json::json!({"revoked": true})))
}

// ── Bot (connected tool) endpoints ───────────────────────────

pub(super) async fn list_bots(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
) -> Result<Json<Vec<Bot>>, LificError> {
    let user = auth_user.ok_or_else(|| LificError::BadRequest("authentication required".into()))?;

    with_read(&db, |conn| {
        crate::db::queries::users::list_bots(conn, user.id)
    })
    .map(Json)
}

#[derive(serde::Deserialize)]
pub(super) struct CreateBotRequest {
    /// Tool identifier (e.g. "opencode", "cursor", "claude", "codex", "pi",
    /// "vscode", "zed")
    tool: String,
}

pub(super) async fn create_bot(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Extension(manager): Extension<std::sync::Arc<api_keys_simplified::ApiKeyManagerV0>>,
    Json(input): Json<CreateBotRequest>,
) -> Result<Json<serde_json::Value>, LificError> {
    let user = auth_user.ok_or_else(|| LificError::BadRequest("authentication required".into()))?;

    let tool = input.tool.trim().to_lowercase();
    let display_name = match tool.as_str() {
        "opencode" => "OpenCode",
        "cursor" => "Cursor",
        "claude-code" => "Claude Code",
        "claude" => "Claude Desktop",
        "codex" => "Codex",
        "pi" => "Pi",
        "vscode" => "VS Code",
        "zed" => "Zed",
        _ => return Err(LificError::BadRequest(format!("unknown tool: {tool}"))),
    };

    let bot_username = format!("{tool}-{}", user.username);

    // Check if a disconnected bot already exists — reconnect it instead of creating new
    let existing_bot = with_read(&db, |conn| {
        crate::db::queries::users::find_bot_by_username(conn, &bot_username)
    })
    .ok()
    .flatten();

    let bot_user = if let Some(existing) = existing_bot {
        // Bot exists — check if it already has an active key
        let has_key = with_read(&db, |conn| {
            crate::db::queries::users::bot_has_active_key(conn, existing.id)
        })?;

        if has_key {
            return Err(LificError::BadRequest(format!(
                "{display_name} is already connected"
            )));
        }

        existing
    } else {
        // Create fresh bot user
        with_write(&db, |conn| {
            crate::db::queries::users::create_bot_user(conn, user.id, &bot_username, display_name)
        })?
    };

    // Generate a new API key for the bot
    let plaintext_key = crate::auth::create_api_key(&db, &manager, &bot_username)?;

    // Assign the key to the bot user
    let conn = db.write()?;
    crate::db::queries::users::assign_key_to_user(&conn, &bot_username, bot_user.id)?;

    Ok(Json(serde_json::json!({
        "bot": {
            "id": bot_user.id,
            "username": bot_user.username,
            "display_name": bot_user.display_name,
        },
        "key": plaintext_key,
        "tool": tool,
    })))
}

pub(super) async fn disconnect_bot(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Extension(auth_user): Extension<Option<AuthUser>>,
) -> Result<Json<serde_json::Value>, LificError> {
    let user = auth_user.ok_or_else(|| LificError::BadRequest("authentication required".into()))?;

    let conn = db.write()?;
    crate::db::queries::users::disconnect_bot(&conn, id, user.id, user.is_admin)?;

    Ok(Json(serde_json::json!({"disconnected": true})))
}

pub(super) async fn delete_bot(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Extension(auth_user): Extension<Option<AuthUser>>,
) -> Result<Json<serde_json::Value>, LificError> {
    let user = auth_user.ok_or_else(|| LificError::BadRequest("authentication required".into()))?;

    let conn = db.write()?;
    crate::db::queries::users::delete_bot(&conn, id, user.id, user.is_admin)?;

    Ok(Json(serde_json::json!({"deleted": true})))
}

// ── User endpoints ──────────────────────────────────────────

#[derive(serde::Serialize)]
pub(super) struct UserListItem {
    id: i64,
    username: String,
    display_name: String,
    is_admin: bool,
    created_at: String,
}

pub(super) async fn list_users(
    State(db): State<DbPool>,
) -> Result<Json<Vec<UserListItem>>, LificError> {
    with_read(&db, |conn| {
        let users = crate::db::queries::users::list_users(conn)?;
        Ok(users
            .into_iter()
            .filter(|u| !u.is_bot)
            .map(|u| UserListItem {
                id: u.id,
                username: u.username,
                display_name: u.display_name,
                is_admin: u.is_admin,
                created_at: u.created_at,
            })
            .collect())
    })
    .map(Json)
}

#[cfg(test)]
mod tests {
    use crate::api::test_helpers::*;
    use axum::http::StatusCode;

    // LIF-207: the Secure attribute is gated; everything else stays constant.
    #[test]
    fn session_cookie_gates_secure_flag() {
        let secure = super::session_cookie("lific_sess_x", "2099-01-01T00:00:00Z", true);
        assert!(secure.contains("; Secure"));
        assert!(secure.contains("HttpOnly"));
        assert!(secure.contains("SameSite=Lax"));

        let insecure = super::session_cookie("lific_sess_x", "2099-01-01T00:00:00Z", false);
        assert!(
            !insecure.contains("Secure"),
            "http deploy must omit Secure: {insecure}"
        );
        assert!(insecure.contains("HttpOnly"));
        assert!(insecure.contains("SameSite=Lax"));
    }

    #[test]
    fn clear_cookie_mirrors_secure_flag() {
        assert!(super::clear_cookie(true).contains("; Secure"));
        assert!(!super::clear_cookie(false).contains("Secure"));
        assert!(super::clear_cookie(true).contains("Max-Age=0"));
    }

    #[tokio::test]
    async fn auth_signup_creates_user_and_returns_session() {
        let app = test_app();
        let body = serde_json::json!({
            "username": "blake",
            "email": "blake@test.com",
            "password": "securepass123"
        });
        let resp = json_post(&app, "/api/auth/signup", body).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let data = parse_json(resp).await;
        assert_eq!(data["user"]["username"], "blake");
        assert!(data["token"].as_str().unwrap().starts_with("lific_sess_"));
        assert!(data["expires_at"].as_str().is_some());
    }

    #[tokio::test]
    async fn auth_signup_duplicate_rejected() {
        let app = test_app();
        let body = serde_json::json!({
            "username": "dupe",
            "email": "dupe@test.com",
            "password": "securepass123"
        });
        let resp = json_post(&app, "/api/auth/signup", body.clone()).await;
        assert_eq!(resp.status(), StatusCode::OK);

        // Second signup with same username
        let resp = json_post(&app, "/api/auth/signup", body).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn auth_signup_disabled_rejects() {
        // Signup policy is DB-backed now: disable it in the settings store.
        let db = crate::db::open_memory().expect("test db");
        {
            let conn = db.write().unwrap();
            crate::db::queries::settings::update(
                &conn,
                crate::db::queries::settings::InstanceSettingsPatch {
                    allow_signup: Some(false),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        let app = crate::api::router(db, &[]).layer(axum::Extension(crate::config::AuthConfig {
            allow_signup: true,
            required: true,
            secure_cookies: false,
        }));

        let body = serde_json::json!({
            "username": "blocked",
            "email": "blocked@test.com",
            "password": "securepass123"
        });
        let resp = json_post(&app, "/api/auth/signup", body).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let data = parse_json(resp).await;
        assert!(data["error"].as_str().unwrap().contains("closed"));
    }

    // ── GET /api/instance: public state the auth screen reads ──

    #[tokio::test]
    async fn instance_reports_open_signup_and_existing_users() {
        // test_app() seeds a human admin and defaults allow_signup = true.
        let app = test_app();
        let resp = json_get(&app, "/api/instance").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        assert_eq!(data["allow_signup"], true);
        assert_eq!(data["has_users"], true, "seeded admin counts as a human");
    }

    #[tokio::test]
    async fn instance_reports_closed_signup_and_empty_when_fresh() {
        // Fresh db, no users, signup disabled via the settings store.
        let db = crate::db::open_memory().expect("test db");
        {
            let conn = db.write().unwrap();
            crate::db::queries::settings::update(
                &conn,
                crate::db::queries::settings::InstanceSettingsPatch {
                    allow_signup: Some(false),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        let app = crate::api::router(db, &[]).layer(axum::Extension(crate::config::AuthConfig {
            allow_signup: true,
            required: true,
            secure_cookies: false,
        }));

        let resp = json_get(&app, "/api/instance").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        assert_eq!(data["allow_signup"], false);
        assert_eq!(data["has_users"], false);
    }

    #[tokio::test]
    async fn instance_flips_has_users_after_first_signup() {
        // Open signup, fresh db: has_users is false until the first human signs
        // up, then true. This is the brand-new-instance transition the signup
        // page keys its copy off (without ever claiming the account is admin).
        let db = crate::db::open_memory().expect("test db");
        let app = crate::api::router(db, &[]).layer(axum::Extension(crate::config::AuthConfig {
            allow_signup: true,
            required: true,
            secure_cookies: false,
        }));

        let before = parse_json(json_get(&app, "/api/instance").await).await;
        assert_eq!(before["has_users"], false);

        let body = serde_json::json!({
            "username": "firsthuman",
            "email": "first@test.com",
            "password": "securepass123"
        });
        assert_eq!(
            json_post(&app, "/api/auth/signup", body).await.status(),
            StatusCode::OK
        );

        let after = parse_json(json_get(&app, "/api/instance").await).await;
        assert_eq!(after["has_users"], true);
    }

    // ── Instance settings (admin-gated GET/PATCH) ──

    #[tokio::test]
    async fn instance_settings_admin_can_read_and_patch() {
        let app = test_app(); // authed as admin

        let data = parse_json(json_get(&app, "/api/instance/settings").await).await;
        assert_eq!(data["allow_signup"], true);
        assert_eq!(data["session_lifetime_days"], 30);

        let patch = serde_json::json!({
            "instance_name": "Acme Eng",
            "allow_signup": false,
            "session_lifetime_days": 14,
            "signup_email_domains": ["acme.com"],
        });
        let resp = json_patch(&app, "/api/instance/settings", patch).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        assert_eq!(data["instance_name"], "Acme Eng");
        assert_eq!(data["allow_signup"], false);
        assert_eq!(data["session_lifetime_days"], 14);
        assert_eq!(data["signup_email_domains"][0], "acme.com");

        // The public endpoint reflects the live change.
        let pub_data = parse_json(json_get(&app, "/api/instance").await).await;
        assert_eq!(pub_data["allow_signup"], false);
        assert_eq!(pub_data["instance_name"], "Acme Eng");
    }

    // LIF-197: the operator toggle for LIF-194's project-scoped
    // authorization. Defaults off, round-trips through the admin PATCH, and
    // rides along in both the admin GET and the settings_json() shape.
    #[tokio::test]
    async fn instance_settings_exposes_authz_enforced_toggle() {
        let app = test_app();

        let data = parse_json(json_get(&app, "/api/instance/settings").await).await;
        assert_eq!(data["authz_enforced"], false, "off by default");

        let patch = json_patch(
            &app,
            "/api/instance/settings",
            serde_json::json!({ "authz_enforced": true }),
        )
            .await;
        assert_eq!(patch.status(), StatusCode::OK);
        assert_eq!(parse_json(patch).await["authz_enforced"], true);

        let data = parse_json(json_get(&app, "/api/instance/settings").await).await;
        assert_eq!(data["authz_enforced"], true, "persisted");
    }

    #[tokio::test]
    async fn instance_settings_forbidden_for_non_admin() {
        let db = crate::db::open_memory().expect("test db");
        let user = {
            let conn = db.write().unwrap();
            crate::db::queries::users::create_user(
                &conn,
                &crate::db::models::CreateUser {
                    username: "reg".into(),
                    email: "reg@test.com".into(),
                    password: "securepass123".into(),
                    display_name: None,
                    is_admin: false,
                    is_bot: false,
                },
            )
            .unwrap()
        };
        let app = crate::api::test_helpers::app_as_user(db, &user);
        assert_eq!(
            json_get(&app, "/api/instance/settings").await.status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            json_patch(
                &app,
                "/api/instance/settings",
                serde_json::json!({ "allow_signup": true })
            )
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }

    // ── LIF-215: single-user web auto-login ──

    #[tokio::test]
    async fn auto_login_disabled_by_default_is_forbidden() {
        // Default-deny: the flag is off until an admin enables it.
        let app = test_app();
        let resp = json_post(&app, "/api/auth/auto-login", serde_json::json!({})).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn auto_login_enabled_mints_admin_session() {
        let app = test_app(); // seeded admin + authed as admin for the PATCH
        let patch = json_patch(
            &app,
            "/api/instance/settings",
            serde_json::json!({ "web_auto_login": true }),
        )
        .await;
        assert_eq!(patch.status(), StatusCode::OK);
        assert_eq!(parse_json(patch).await["web_auto_login"], true);

        let resp = json_post(&app, "/api/auth/auto-login", serde_json::json!({})).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        assert!(
            data["token"].as_str().unwrap().starts_with("lific_sess_"),
            "auto-login must mint a real session token: {data}"
        );
        assert_eq!(data["user"]["is_admin"], true);
        assert_eq!(data["user"]["username"], "test-admin");
    }

    #[tokio::test]
    async fn instance_info_exposes_web_auto_login() {
        let app = test_app();
        let before = parse_json(json_get(&app, "/api/instance").await).await;
        assert_eq!(before["web_auto_login"], false, "off by default");

        json_patch(
            &app,
            "/api/instance/settings",
            serde_json::json!({ "web_auto_login": true }),
        )
        .await;

        let after = parse_json(json_get(&app, "/api/instance").await).await;
        assert_eq!(after["web_auto_login"], true);
    }

    #[tokio::test]
    async fn signup_enforces_email_domain_allowlist() {
        let db = crate::db::open_memory().expect("test db");
        {
            let conn = db.write().unwrap();
            crate::db::queries::settings::update(
                &conn,
                crate::db::queries::settings::InstanceSettingsPatch {
                    signup_email_domains: Some(vec!["acme.com".into()]),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        let app = crate::api::router(db, &[]).layer(axum::Extension(crate::config::AuthConfig {
            allow_signup: true,
            required: true,
            secure_cookies: false,
        }));

        // Disallowed domain is rejected.
        let resp = json_post(
            &app,
            "/api/auth/signup",
            serde_json::json!({ "username": "x", "email": "x@other.com", "password": "securepass123" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // Allowed domain succeeds.
        let resp = json_post(
            &app,
            "/api/auth/signup",
            serde_json::json!({ "username": "y", "email": "y@acme.com", "password": "securepass123" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_login_with_correct_password() {
        let app = test_app();

        // Signup first
        let body = serde_json::json!({
            "username": "logintest",
            "email": "login@test.com",
            "password": "securepass123"
        });
        json_post(&app, "/api/auth/signup", body).await;

        // Login by username
        let body = serde_json::json!({
            "identity": "logintest",
            "password": "securepass123"
        });
        let resp = json_post(&app, "/api/auth/login", body).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let data = parse_json(resp).await;
        assert_eq!(data["user"]["username"], "logintest");
        assert!(data["token"].as_str().unwrap().starts_with("lific_sess_"));
    }

    #[tokio::test]
    async fn auth_login_with_wrong_password() {
        let app = test_app();

        let body = serde_json::json!({
            "username": "wrongpw",
            "email": "wrongpw@test.com",
            "password": "securepass123"
        });
        json_post(&app, "/api/auth/signup", body).await;

        let body = serde_json::json!({
            "identity": "wrongpw",
            "password": "nope12345678"
        });
        let resp = json_post(&app, "/api/auth/login", body).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn auth_me_with_session() {
        let app = test_app();

        // Signup to get a session
        let body = serde_json::json!({
            "username": "metest",
            "email": "me@test.com",
            "password": "securepass123"
        });
        let resp = json_post(&app, "/api/auth/signup", body).await;
        let data = parse_json(resp).await;
        let token = data["token"].as_str().unwrap();

        assert_eq!(data["user"]["username"], "metest");
        assert!(token.starts_with("lific_sess_"));
    }

    // ── LIF-190: profile / password / session settings ──────

    #[tokio::test]
    async fn update_me_changes_display_name() {
        use tower::ServiceExt;
        let app = test_app();
        let body = serde_json::json!({ "display_name": "Renamed Admin" });
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("PATCH")
                    .uri("/api/auth/me")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        assert_eq!(data["display_name"], "Renamed Admin");
    }

    #[tokio::test]
    async fn change_password_requires_correct_current() {
        let db = crate::db::open_memory().expect("test db");
        let user = {
            let conn = db.write().unwrap();
            crate::db::queries::users::create_user(
                &conn,
                &crate::db::models::CreateUser {
                    username: "pwuser".into(),
                    email: "pwuser@test.com".into(),
                    password: "originalpass123".into(),
                    display_name: None,
                    is_admin: false,
                    is_bot: false,
                },
            )
            .unwrap()
        };
        let app = crate::api::test_helpers::app_as_user(db, &user);

        let wrong = serde_json::json!({ "current_password": "totally-wrong", "new_password": "newpassword123" });
        let resp = json_post(&app, "/api/auth/me/password", wrong).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let right = serde_json::json!({ "current_password": "originalpass123", "new_password": "newpassword123" });
        let resp = json_post(&app, "/api/auth/me/password", right).await;
        assert_eq!(resp.status(), StatusCode::OK);
        // LIF-205: a successful change returns a fresh session token so the
        // current browser stays logged in after the old sessions are killed.
        let data = parse_json(resp).await;
        assert!(
            data["token"]
                .as_str()
                .unwrap_or("")
                .starts_with("lific_sess_"),
            "password change should mint a new session token: {data}"
        );
        assert!(data["expires_at"].as_str().is_some());
    }

    // LIF-205: changing the password must invalidate every pre-existing
    // session, so a stolen token dies the moment the user "locks it down."
    // Exercised at the query layer because the test HTTP harness injects the
    // AuthUser directly and never runs the session-validating middleware.
    #[tokio::test]
    async fn change_password_invalidates_existing_sessions() {
        use crate::db::queries::users;
        let db = crate::db::open_memory().expect("test db");
        let user = {
            let conn = db.write().unwrap();
            users::create_user(
                &conn,
                &crate::db::models::CreateUser {
                    username: "rotate".into(),
                    email: "rotate@test.com".into(),
                    password: "originalpass123".into(),
                    display_name: None,
                    is_admin: false,
                    is_bot: false,
                },
            )
            .unwrap()
        };

        // An attacker's stolen session.
        let stolen = {
            let conn = db.write().unwrap();
            users::create_session(&conn, user.id, None).unwrap()
        };
        {
            let conn = db.write().unwrap();
            assert!(
                users::validate_session(&conn, &stolen.token).is_ok(),
                "session should be valid before the password change"
            );
        }

        let app = crate::api::test_helpers::app_as_user(db.clone(), &user);
        let body = serde_json::json!({
            "current_password": "originalpass123",
            "new_password": "newpassword123"
        });
        let resp = json_post(&app, "/api/auth/me/password", body).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        let fresh = data["token"].as_str().unwrap();

        let conn = db.write().unwrap();
        assert!(
            users::validate_session(&conn, &stolen.token).is_err(),
            "stolen session must be invalid after a password change"
        );
        assert!(
            users::validate_session(&conn, fresh).is_ok(),
            "the freshly-minted session must be usable"
        );
    }

    #[tokio::test]
    async fn revoke_all_sessions_ok() {
        use tower::ServiceExt;
        let app = test_app();
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("DELETE")
                    .uri("/api/auth/me/sessions")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        assert_eq!(data["revoked"], true);
    }

    // ── LIF-75: login rate limiting (per-identity + per-IP, no double-count) ──

    /// Build an app whose login route is guarded by a rate limiter capped
    /// at `max` attempts within a 15-minute window.
    fn login_app_with_limiter(max: usize) -> axum::Router {
        let db = crate::db::open_memory().expect("test db");
        let limiter = std::sync::Arc::new(crate::ratelimit::RateLimiter::new(
            max,
            std::time::Duration::from_secs(15 * 60),
        ));
        crate::api::router(db, &[])
            .layer(axum::Extension(crate::config::AuthConfig {
                allow_signup: true,
                required: true,
                secure_cookies: false,
            }))
            .layer(axum::Extension(limiter))
    }

    /// Fire one wrong-password login for `identity` from source IP `xff`.
    /// Returns the status and parsed JSON body so callers can distinguish an
    /// ordinary auth failure from a rate-limit rejection (both are 400).
    async fn login_attempt(
        app: &axum::Router,
        identity: &str,
        xff: &str,
    ) -> (StatusCode, serde_json::Value) {
        use tower::ServiceExt;
        let body = serde_json::json!({ "identity": identity, "password": "definitely-wrong-pw" });
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header("content-type", "application/json")
                    .header("x-forwarded-for", xff)
                    .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        (status, parse_json(resp).await)
    }

    fn is_rate_limited(body: &serde_json::Value) -> bool {
        body["error"]
            .as_str()
            .unwrap_or("")
            .contains("too many login attempts")
    }

    #[tokio::test]
    async fn login_grants_full_per_identity_budget() {
        // Regression for the double-counting bug: with max 5, exactly 5
        // failed attempts must be allowed before the 6th is blocked. The old
        // code (check() records + record_failure() records) only allowed ~3.
        // Distinct IP per attempt so only the per-identity bucket accrues.
        let app = login_app_with_limiter(5);
        for i in 0..5 {
            let (status, body) = login_attempt(&app, "victim", &format!("10.0.0.{i}")).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert!(
                !is_rate_limited(&body),
                "attempt {i} should be an auth failure, not rate-limited: {body}"
            );
        }
        // 6th attempt (fresh IP) trips the per-identity limit.
        let (status, body) = login_attempt(&app, "victim", "10.0.0.250").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            is_rate_limited(&body),
            "6th attempt should be rate-limited by the identity bucket: {body}"
        );
    }

    #[tokio::test]
    async fn login_rate_limit_applies_per_ip_across_identities() {
        // Per-IP limiting (new in LIF-75): one host spraying many usernames
        // gets throttled even though each identity is distinct. Previously
        // impossible — the limiter was keyed solely on identity.
        let app = login_app_with_limiter(5);
        for i in 0..5 {
            let (status, body) = login_attempt(&app, &format!("user{i}"), "203.0.113.5").await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert!(
                !is_rate_limited(&body),
                "attempt {i} should be an auth failure: {body}"
            );
        }
        // 6th attempt: same IP, brand-new username → blocked by the IP bucket.
        let (status, body) = login_attempt(&app, "user-brand-new", "203.0.113.5").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            is_rate_limited(&body),
            "6th attempt from the same IP should be rate-limited: {body}"
        );
    }

    #[tokio::test]
    async fn login_rate_limit_isolates_distinct_ips() {
        // A victim identity is NOT locked out for an attacker on a different
        // IP, as long as the victim comes from their own IP and the identity
        // budget hasn't been exhausted. Sanity check that buckets are keyed
        // independently and the IP key is actually in play.
        let app = login_app_with_limiter(3);
        // Attacker burns the identity budget would also block victim, so to
        // isolate the IP dimension we use distinct identities here.
        for i in 0..3 {
            let (_, body) = login_attempt(&app, &format!("a{i}"), "198.51.100.1").await;
            assert!(!is_rate_limited(&body), "setup attempt {i}: {body}");
        }
        // Attacker IP is now capped.
        let (_, attacker) = login_attempt(&app, "a-extra", "198.51.100.1").await;
        assert!(
            is_rate_limited(&attacker),
            "attacker IP should be capped: {attacker}"
        );
        // A different IP is unaffected.
        let (_, other) = login_attempt(&app, "someone", "198.51.100.2").await;
        assert!(
            !is_rate_limited(&other),
            "distinct IP should not be limited: {other}"
        );
    }

    // ── LIF-138: signup rate limiting must also key on source IP ──

    /// Fire one signup with a fresh username/email from source IP `xff`.
    async fn signup_attempt(
        app: &axum::Router,
        n: usize,
        xff: &str,
    ) -> (StatusCode, serde_json::Value) {
        use tower::ServiceExt;
        let body = serde_json::json!({
            "username": format!("user{n}"),
            "email": format!("user{n}@test.com"),
            "password": "securepass123",
        });
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/auth/signup")
                    .header("content-type", "application/json")
                    .header("x-forwarded-for", xff)
                    .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        (status, parse_json(resp).await)
    }

    fn is_signup_rate_limited(body: &serde_json::Value) -> bool {
        body["error"]
            .as_str()
            .unwrap_or("")
            .contains("too many signup attempts")
    }

    #[tokio::test]
    async fn signup_rate_limit_applies_per_ip_across_emails() {
        // The DoS LIF-138 fixes: an email-only key is bypassed by rotating
        // addresses, each request still paying a full Argon2 hash. Distinct
        // emails from ONE IP must now be throttled by the per-IP bucket.
        let app = login_app_with_limiter(5);
        for i in 0..5 {
            let (status, body) = signup_attempt(&app, i, "203.0.113.9").await;
            assert_eq!(status, StatusCode::OK, "signup {i} should succeed: {body}");
            assert!(
                !is_signup_rate_limited(&body),
                "signup {i} not yet limited: {body}"
            );
        }
        // 6th: same IP, brand-new email → blocked by the IP bucket.
        let (status, body) = signup_attempt(&app, 99, "203.0.113.9").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            is_signup_rate_limited(&body),
            "6th signup from the same IP should be rate-limited: {body}"
        );
    }

    #[tokio::test]
    async fn signup_rate_limit_isolates_distinct_ips() {
        // The per-IP cap must not leak across IPs: a fresh source can still
        // sign up after another IP is capped.
        let app = login_app_with_limiter(3);
        for i in 0..3 {
            let (status, _) = signup_attempt(&app, i, "198.51.100.7").await;
            assert_eq!(status, StatusCode::OK);
        }
        // Capping IP is now blocked.
        let (_, capped) = signup_attempt(&app, 50, "198.51.100.7").await;
        assert!(
            is_signup_rate_limited(&capped),
            "capped IP should be blocked: {capped}"
        );
        // A different IP is unaffected.
        let (status, other) = signup_attempt(&app, 60, "198.51.100.8").await;
        assert_eq!(
            status,
            StatusCode::OK,
            "distinct IP should not be limited: {other}"
        );
    }
}
