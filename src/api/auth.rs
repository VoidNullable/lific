use axum::{
    Extension,
    extract::{ConnectInfo, Json, Path, State},
    http::HeaderMap,
    response::IntoResponse,
};
use std::{net::SocketAddr, sync::Arc};

use crate::db::{DbPool, models::*};
use crate::error::LificError;
use crate::realtime::{RealtimeEvent, RealtimeHub};

use super::{require_admin, require_user, with_read, with_write};

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
/// to prevent privilege escalation. Those can only be set via CLI, with one
/// carve-out: the first user on a zero-user instance is granted admin by the
/// handler itself (LIF-364), because a client-supplied flag and a
/// server-decided bootstrap are different threat models.
#[derive(serde::Deserialize)]
pub(super) struct SignupRequest {
    username: String,
    email: String,
    password: String,
    display_name: Option<String>,
}

/// Whether this instance's signup policy accepts a signup for `email`.
///
/// Two rules, both DB-backed and admin-editable (never TOML): signups can be
/// closed outright, and they can be limited to an email-domain allowlist.
/// Factored out because LIF-412's split runs it twice: once on a read
/// connection, to reject a closed instance before paying for Argon2, and
/// again under the writer, so the account is created against the policy that
/// is live at that moment.
fn signup_policy_allows(
    settings: &crate::db::queries::settings::InstanceSettings,
    email: &str,
) -> Result<(), LificError> {
    if !settings.allow_signup {
        return Err(LificError::BadRequest(
            "signups are closed on this instance. Ask an admin to create your account.".into(),
        ));
    }
    if !settings.signup_email_domains.is_empty() {
        let domain = email.rsplit('@').next().unwrap_or("").trim().to_lowercase();
        if !settings.signup_email_domains.contains(&domain) {
            return Err(LificError::BadRequest(format!(
                "signups on this instance are limited to: {}",
                settings.signup_email_domains.join(", ")
            )));
        }
    }
    Ok(())
}

pub(super) async fn auth_signup(
    State(db): State<DbPool>,
    Extension(auth_cfg): Extension<crate::config::AuthConfig>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Extension(trusted_proxies): Extension<Arc<[crate::ratelimit::IpNetwork]>>,
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
    let ip_key = format!(
        "signup_ip:{}",
        crate::ratelimit::client_ip(peer.ip(), &headers, &trusted_proxies)
    );
    if let Some(Extension(ref rl)) = limiter
        && (!rl.check(&email_key) || !rl.check(&ip_key))
    {
        let retry = rl.retry_after(&email_key).max(rl.retry_after(&ip_key));
        return Err(LificError::BadRequest(format!(
            "too many signup attempts — try again in {retry} seconds"
        )));
    }

    // The instance's signup policy. Read on a pooled connection so a closed
    // instance rejects the request without ever touching the writer, and
    // re-checked under the writer below (`signup_policy_allows`) so the
    // decision that actually creates the account is atomic with the insert.
    let settings = with_read(&db, crate::db::queries::settings::get)?;
    signup_policy_allows(&settings, &input.email)?;

    let mut new_user = CreateUser {
        username: input.username,
        email: input.email,
        password: input.password,
        display_name: input.display_name,
        // Decided under the writer below (LIF-364).
        is_admin: false,
        is_bot: false,
    };
    crate::db::queries::users::validate_new_user(&new_user)?;

    // LIF-412: Argon2 is deliberately expensive, so it runs on the blocking
    // pool with NO database lock held. Hashing while holding the exclusive
    // writer stalled every other write in the process for the duration of
    // each signup, and a burst of signups stalled the whole instance.
    let password = new_user.password.clone();
    let password_hash = tokio::task::spawn_blocking(move || {
        crate::db::queries::users::hash_password(&password)
    })
    .await
    .map_err(|e| LificError::Internal(format!("password hashing task failed: {e}")))??;

    let (user, session) = {
        let conn = db.write()?;
        // Authoritative re-check: the policy could have been changed by an
        // admin between the read above and this write.
        let settings = crate::db::queries::settings::get(&conn)?;
        signup_policy_allows(&settings, &new_user.email)?;

        // LIF-364: the first account on a zero-user instance becomes the
        // instance admin (the standard self-hosted bootstrap: Immich,
        // Portainer, Grafana all do this). Without it a web-signup-only
        // instance has NO admin and no way to mint one short of `lific user
        // promote` on the server's shell, which is how dr.leech ended up
        // unable to see his own agents' projects under enforced authz.
        // LIF-209's "signup never grants admin" rationale (privilege
        // escalation by a stranger) doesn't apply to user #1 on an empty
        // instance: whoever reaches an open-signup empty instance first owns
        // it in every way that matters. Any pre-existing user (CLI-created
        // included) disables the grant, and we're inside the single write
        // connection so the count can't race a concurrent signup.
        new_user.is_admin = conn.query_row("SELECT COUNT(*) = 0 FROM users", [], |r| r.get(0))?;

        let user = crate::db::queries::users::insert_user_with_hash(
            &conn,
            &new_user,
            &password_hash,
        )?;
        let session = crate::db::queries::users::create_session(
            &conn,
            user.id,
            Some(settings.session_lifetime_days * 24),
        )?;
        (user, session)
    };

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

/// Verify a login without holding a database lock across Argon2 (LIF-412).
///
/// The verify is the expensive part of a login: tens of milliseconds of CPU
/// by design. Running it on the exclusive write connection, as this handler
/// used to, serialized every writer in the process behind each attempt, so a
/// burst of wrong-password attempts stalled unrelated API traffic. Here the
/// lookup takes a pooled *read* connection and gives it straight back, the
/// verify runs on the blocking pool with no lock held at all, and the writer
/// is taken afterwards, only to mint the session.
///
/// Behaviour is unchanged, dummy-hash verify for an unknown user included:
/// see [`crate::db::queries::users::PasswordChallenge`].
async fn authenticate_off_writer(
    db: &DbPool,
    identity: &str,
    password: &str,
) -> Result<User, LificError> {
    crate::db::queries::users::reject_oversized_password(password)?;

    // The read connection is released with this binding, before the verify.
    let challenge = with_read(db, |conn| {
        Ok(crate::db::queries::users::password_challenge(conn, identity))
    })?;

    let password = password.to_string();
    let hash = challenge.hash().to_string();
    let password_ok = tokio::task::spawn_blocking(move || {
        crate::db::queries::users::verify_password(&password, &hash).unwrap_or(false)
    })
    .await
    .map_err(|e| LificError::Internal(format!("password verification task failed: {e}")))?;

    challenge.finish(password_ok)
}

pub(super) async fn auth_login(
    State(db): State<DbPool>,
    Extension(auth_cfg): Extension<crate::config::AuthConfig>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Extension(trusted_proxies): Extension<Arc<[crate::ratelimit::IpNetwork]>>,
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
    let ip_key = format!(
        "login_ip:{}",
        crate::ratelimit::client_ip(peer.ip(), &headers, &trusted_proxies)
    );
    if let Some(Extension(ref rl)) = limiter
        && (!rl.peek(&id_key) || !rl.peek(&ip_key))
    {
        let retry = rl.retry_after(&id_key).max(rl.retry_after(&ip_key));
        return Err(LificError::BadRequest(format!(
            "too many login attempts — try again in {retry} seconds"
        )));
    }

    let user = match authenticate_off_writer(&db, &input.identity, &input.password).await {
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

    let session = {
        let conn = db.write()?;
        let lifetime_days = crate::db::queries::settings::get(&conn)
            .map(|s| s.session_lifetime_days)
            .unwrap_or(30);
        crate::db::queries::users::create_session(&conn, user.id, Some(lifetime_days * 24))?
    };

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
    // LIF-297: `[auth] required = false` implies single-user mode for the
    // browser too — an instance that lets anonymous API callers act as the
    // operator has no business showing its own operator a login form. The
    // config key shares web_auto_login's threat model (both hand out admin
    // to whoever can reach the page) and auth-off already refuses to start
    // with a non-localhost public_url.
    if !settings.web_auto_login && auth_cfg.required {
        return Err(LificError::Forbidden(
            "single-user auto-login is not enabled on this instance".into(),
        ));
    }

    // LIFIC-8: the "no credential → first admin" fallback is consolidated in
    // `resolve_caller`. Auto-login has no credential (it is the thing that
    // *produces* a session), so the passwordless fallback applies.
    let admin = crate::resolve_caller::resolve_caller_conn(
        &conn,
        None,
        crate::actor::Transport::Web,
    )?
    .ok_or_else(|| LificError::BadRequest("no admin account exists to sign in as".into()))?;
    let admin = admin.user;

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
    Extension(auth_cfg): Extension<crate::config::AuthConfig>,
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
        // LIF-297: `[auth] required = false` activates the same rail — this is
        // the SPA's bootstrap signal, not the stored setting (the admin
        // settings endpoint keeps reporting the real DB flag).
        "web_auto_login": settings.web_auto_login || !auth_cfg.required,
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
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
) -> Result<Json<serde_json::Value>, LificError> {
    require_admin(&identity)?;
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
    Extension(realtime): Extension<RealtimeHub>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    reachability: Option<Extension<crate::server::Reachability>>,
    Json(input): Json<InstanceSettingsPatchReq>,
) -> Result<Json<serde_json::Value>, LificError> {
    require_admin(&identity)?;

    // LIF-406: `lific start` refuses to boot with web auto-login enabled on a
    // publicly reachable instance, but that check only ever ran at startup,
    // so an admin could switch the flag on afterwards and hand an admin
    // session to anyone who could load the page. Same guard, same question,
    // asked again here before anything is persisted.
    if input.web_auto_login == Some(true)
        && let Some(Extension(reach)) = reachability.as_ref()
        && let Some(exposure) = reach.public_exposure()
    {
        return Err(LificError::BadRequest(format!(
            "single-user web auto-login cannot be enabled while {exposure}. Anyone \
             who can reach this instance would get an admin session without a \
             password. Bind to 127.0.0.1 and remove the public URL first."
        )));
    }

    let patch = crate::db::queries::settings::InstanceSettingsPatch {
        allow_signup: input.allow_signup,
        instance_name: input.instance_name,
        signup_email_domains: input.signup_email_domains,
        session_lifetime_days: input.session_lifetime_days,
        login_message: input.login_message,
        web_auto_login: input.web_auto_login,
        authz_enforced: input.authz_enforced,
    };
    let authz_enforced = input.authz_enforced;
    let (s, authz_changed) = with_write(&db, move |conn| {
        let previous_authz_enforced = crate::db::queries::settings::get(conn)?.authz_enforced;
        let settings = crate::db::queries::settings::update(conn, patch)?;
        let authz_changed =
            authz_enforced.is_some_and(|_| settings.authz_enforced != previous_authz_enforced);
        Ok((settings, authz_changed))
    })?;
    if authz_changed {
        realtime.send(RealtimeEvent::ResyncRequired);
    }
    Ok(Json(settings_json(&s)))
}

pub(super) async fn auth_me(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
) -> Result<Json<serde_json::Value>, LificError> {
    let user = require_user(&identity)?;

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
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Json(input): Json<UpdateMeRequest>,
) -> Result<Json<serde_json::Value>, LificError> {
    let user = require_user(&identity)?;
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
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Json(input): Json<ChangePasswordRequest>,
) -> Result<impl IntoResponse, LificError> {
    let user = require_user(&identity)?;
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
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
) -> Result<impl IntoResponse, LificError> {
    let user = require_user(&identity)?;
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
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
) -> Result<Json<Vec<UserApiKey>>, LificError> {
    let user = require_user(&identity)?;

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
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Extension(manager): Extension<std::sync::Arc<api_keys_simplified::ApiKeyManagerV0>>,
    Json(input): Json<CreateKeyRequest>,
) -> Result<Json<serde_json::Value>, LificError> {
    let user = require_user(&identity)?;

    let name = input.name.trim().to_string();
    if name.is_empty() {
        return Err(LificError::BadRequest("key name cannot be empty".into()));
    }

    // LIF-391: the key is created already bound to the caller, in one write.
    let plaintext = crate::auth::create_api_key(&db, &manager, &name, Some(user.id))?;

    Ok(Json(serde_json::json!({
        "name": name,
        "key": plaintext,
    })))
}

pub(super) async fn revoke_key(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
) -> Result<Json<serde_json::Value>, LificError> {
    let user = require_user(&identity)?;

    let conn = db.write()?;
    crate::db::queries::users::revoke_user_key(&conn, id, user.id, user.is_admin)?;

    Ok(Json(serde_json::json!({"revoked": true})))
}

// ── Bot (connected tool) endpoints ───────────────────────────

pub(super) async fn list_bots(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
) -> Result<Json<Vec<Bot>>, LificError> {
    let user = require_user(&identity)?;

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
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Extension(manager): Extension<std::sync::Arc<api_keys_simplified::ApiKeyManagerV0>>,
    Json(input): Json<CreateBotRequest>,
) -> Result<Json<serde_json::Value>, LificError> {
    let user = require_user(&identity)?;

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

    // Reuse the shared find-or-create seam (LIFIC-13) so a web-connected bot is
    // indistinguishable from one minted at OAuth approval or via `lific connect`.
    let bot_user = with_write(&db, |conn| {
        crate::db::queries::users::ensure_bot(conn, user.id, &tool, display_name)
    })?;

    // If the bot already has a live credential (API key or OAuth token) it's
    // already connected — refuse rather than silently minting a fresh
    // credential for an active tool.
    let connected = with_read(&db, |conn| {
        crate::db::queries::users::bot_is_connected(conn, bot_user.id)
    })?;
    if connected {
        return Err(LificError::BadRequest(format!(
            "{display_name} is already connected"
        )));
    }

    // Generate a new API key, bound to the bot user by the same insert (LIF-391)
    let plaintext_key =
        crate::auth::create_api_key(&db, &manager, &bot_username, Some(bot_user.id))?;

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
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
) -> Result<Json<serde_json::Value>, LificError> {
    let user = require_user(&identity)?;

    let conn = db.write()?;
    crate::db::queries::users::disconnect_bot(&conn, id, user.id, user.is_admin)?;

    Ok(Json(serde_json::json!({"disconnected": true})))
}

pub(super) async fn delete_bot(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
) -> Result<Json<serde_json::Value>, LificError> {
    let user = require_user(&identity)?;

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
    /// LIF-214: false for a deactivated account. Deactivated users stay in
    /// the list so an admin can find and restore them; clients that are
    /// picking someone to hand work to (the project-member picker) filter
    /// them out.
    is_active: bool,
    created_at: String,
}

impl From<User> for UserListItem {
    fn from(u: User) -> Self {
        UserListItem {
            id: u.id,
            username: u.username,
            display_name: u.display_name,
            is_admin: u.is_admin,
            is_active: u.is_active,
            created_at: u.created_at,
        }
    }
}

pub(super) async fn list_users(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
) -> Result<Json<Vec<UserListItem>>, LificError> {
    // Defense in depth (PR #29 report): the auth middleware already rejects
    // unauthenticated requests to this route, but the roster is the kind of
    // endpoint a future routing change could accidentally expose, so the
    // handler refuses to serve without a resolved user of its own accord.
    // Any authenticated member may read it (project leads need the roster
    // for the member picker); mutations stay admin-gated below.
    require_user(&identity)?;
    with_read(&db, |conn| {
        let users = crate::db::queries::users::list_users(conn)?;
        Ok(users
            .into_iter()
            .filter(|u| !u.is_bot)
            .map(UserListItem::from)
            .collect())
    })
    .map(Json)
}

// ── Instance-admin roster management (LIF-214) ───────────────
//
// Four admin-gated mutations behind the member roster on the Instance
// settings page: create an account, promote it, demote it, switch it off (and
// back on). This is the instance-admin axis only. Project-scoped roles live
// in `api::members` and are a separate thing entirely.
//
// The guard rails are NOT here. `require_admin` is the only check this module
// makes; "you cannot strand the instance with no admin" and "you cannot point
// any of this at a bot" are enforced in `db::queries::users`
// (`set_admin_guarded` / `set_active`), so the CLI and any future caller get
// them too rather than trusting a handler to remember.

#[derive(serde::Deserialize)]
pub(super) struct CreateUserRequest {
    username: String,
    password: String,
    /// Optional. A local instance rarely has a real address for a teammate,
    /// and `users.email` is NOT NULL UNIQUE, so an omitted one becomes the
    /// same `{username}@local` placeholder `create_passwordless_admin` uses.
    email: Option<String>,
    display_name: Option<String>,
    /// Create the account already holding instance admin. Off by default;
    /// only an admin can reach this endpoint at all, so this is the same
    /// authority `lific user create --admin` has.
    #[serde(default)]
    is_admin: bool,
}

/// POST /api/users: create a local account. Admin only.
///
/// Never mints a bot: `is_bot` is not part of the request at all. Connected
/// tools come from `POST /api/auth/bots` and the OAuth flow, which own the
/// (owner, tool) identity rules.
pub(super) async fn create_user_handler(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Json(input): Json<CreateUserRequest>,
) -> Result<Json<UserListItem>, LificError> {
    require_admin(&identity)?;

    let username = input.username.trim().to_string();
    let email = match input.email.as_deref().map(str::trim) {
        Some(e) if !e.is_empty() => e.to_string(),
        _ => format!("{username}@local"),
    };

    let user = with_write(&db, |conn| {
        crate::db::queries::users::create_user(
            conn,
            &CreateUser {
                username: username.clone(),
                email,
                password: input.password.clone(),
                display_name: input.display_name.clone(),
                is_admin: input.is_admin,
                is_bot: false,
            },
        )
    })?;

    Ok(Json(user.into()))
}

/// POST /api/users/{id}/promote: grant instance admin. Admin only.
pub(super) async fn promote_user(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
) -> Result<Json<UserListItem>, LificError> {
    require_admin(&identity)?;
    let user = with_write(&db, |conn| {
        crate::db::queries::users::set_admin_guarded(conn, id, true)
    })?;
    Ok(Json(user.into()))
}

/// POST /api/users/{id}/demote: revoke instance admin. Admin only. 409 if
/// this would leave the instance without one.
pub(super) async fn demote_user(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
) -> Result<Json<UserListItem>, LificError> {
    require_admin(&identity)?;
    let user = with_write(&db, |conn| {
        crate::db::queries::users::set_admin_guarded(conn, id, false)
    })?;
    Ok(Json(user.into()))
}

/// POST /api/users/{id}/deactivate: switch an account off without deleting
/// the history it owns. Admin only. 409 if it is the last admin who can sign
/// in.
pub(super) async fn deactivate_user(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
) -> Result<Json<UserListItem>, LificError> {
    require_admin(&identity)?;
    let user = with_write(&db, |conn| {
        crate::db::queries::users::set_active(conn, id, false)
    })?;
    Ok(Json(user.into()))
}

/// POST /api/users/{id}/reactivate: restore a deactivated account. Admin
/// only. The credentials torn down at deactivation are not restored; the user
/// signs in again.
pub(super) async fn reactivate_user(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
) -> Result<Json<UserListItem>, LificError> {
    require_admin(&identity)?;
    let user = with_write(&db, |conn| {
        crate::db::queries::users::set_active(conn, id, true)
    })?;
    Ok(Json(user.into()))
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

    /// A router over a genuinely EMPTY database — no seeded admin fixture —
    /// because the LIF-364 bootstrap tests are about what happens on an
    /// instance where web signup is the first thing that ever creates a user.
    fn zero_user_app(db: crate::db::DbPool) -> axum::Router {
        with_client_ip_test_layers(
            with_attachment_layers(crate::api::router(db, &[])),
            test_peer(),
        )
        .layer(axum::Extension(crate::realtime::RealtimeHub::new()))
        .layer(axum::Extension(crate::config::AuthConfig {
            allow_signup: true,
            required: false,
            secure_cookies: false,
        }))
    }

    /// PR #29 report: the roster route is already auth-gated by the outer
    /// middleware, but the handler now refuses on its own too, so a future
    /// routing change cannot silently expose every username on the instance.
    /// This exercises the handler directly with a resolved identity of None.
    #[tokio::test]
    async fn user_roster_handler_refuses_without_an_identity() {
        let app = zero_user_app(crate::db::open_memory().expect("test db")).layer(
            axum::Extension(None::<crate::resolve_caller::ResolvedIdentity>),
        );
        let resp = json_get(&app, "/api/users").await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    /// LIF-364: the first signup on a zero-user instance bootstraps as
    /// instance admin; every later signup is a plain user. Without this a
    /// web-signup-only instance has no admin at all, and under the
    /// enforced-authz default (LIF-261 seeds it ON for fresh installs) the
    /// operator can't even see projects their other users create.
    #[tokio::test]
    async fn first_signup_bootstraps_admin_second_does_not() {
        let app = zero_user_app(crate::db::open_memory().expect("test db"));

        let resp = json_post(
            &app,
            "/api/auth/signup",
            serde_json::json!({
                "username": "operator",
                "email": "op@test.com",
                "password": "securepass123"
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        assert_eq!(data["user"]["is_admin"], true, "first user is the admin");

        let resp = json_post(
            &app,
            "/api/auth/signup",
            serde_json::json!({
                "username": "agent",
                "email": "agent@test.com",
                "password": "securepass123"
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        assert_eq!(data["user"]["is_admin"], false, "second user is plain");
    }

    /// LIF-364 guard: a pre-existing user (e.g. CLI-created) disables the
    /// first-signup admin grant — the bootstrap is strictly for instances
    /// where signup is the ONLY thing that has ever created a user.
    #[tokio::test]
    async fn signup_after_cli_user_gets_no_admin() {
        let db = crate::db::open_memory().expect("test db");
        {
            let conn = db.write().unwrap();
            crate::db::queries::users::create_user(
                &conn,
                &crate::db::models::CreateUser {
                    username: "clifirst".into(),
                    email: "cli@test.com".into(),
                    password: "securepass123".into(),
                    display_name: None,
                    is_admin: false,
                    is_bot: false,
                },
            )
            .unwrap();
        }
        let app = zero_user_app(db);

        let resp = json_post(
            &app,
            "/api/auth/signup",
            serde_json::json!({
                "username": "weblater",
                "email": "web@test.com",
                "password": "securepass123"
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        assert_eq!(data["user"]["is_admin"], false);
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
        let app = with_client_ip_test_layers(crate::api::router(db, &[]), test_peer()).layer(
            axum::Extension(crate::config::AuthConfig {
                allow_signup: true,
                required: true,
                secure_cookies: false,
            }),
        );

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
        let app = with_client_ip_test_layers(crate::api::router(db, &[]), test_peer()).layer(
            axum::Extension(crate::config::AuthConfig {
                allow_signup: true,
                required: true,
                secure_cookies: false,
            }),
        );

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
        let app = with_client_ip_test_layers(crate::api::router(db, &[]), test_peer()).layer(
            axum::Extension(crate::config::AuthConfig {
                allow_signup: true,
                required: true,
                secure_cookies: false,
            }),
        );

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
    async fn changing_authz_enforcement_emits_resync_required() {
        let test = test_app_with_realtime();
        let mut events = test.realtime.subscribe();

        let resp = json_patch(
            &test.app,
            "/api/instance/settings",
            serde_json::json!({ "authz_enforced": true }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();
        let axum::extract::ws::Message::Text(text) = event.message else {
            panic!("expected text realtime event");
        };
        let event: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(event["type"], "resync.required");
    }

    #[tokio::test]
    async fn patching_authz_enforcement_to_its_current_value_emits_nothing() {
        let test = test_app_with_realtime();
        let mut events = test.realtime.subscribe();

        // Fresh instances default to authz_enforced = false; patching the
        // same value is a no-op and must not trigger a fleet-wide resync.
        let resp = json_patch(
            &test.app,
            "/api/instance/settings",
            serde_json::json!({ "authz_enforced": false }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        assert!(
            events.try_recv().is_err(),
            "no realtime event should be emitted for a no-op authz patch"
        );
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

    // ── LIF-297: [auth] required = false implies web auto-login ──

    #[tokio::test]
    async fn auth_optional_instance_reports_auto_login() {
        // The DB flag stays false; the config alone must flip the SPA's
        // bootstrap signal so the shipped frontend skips the login form.
        let app = test_app_with_auth(false);
        let resp = json_get(&app, "/api/instance").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        assert_eq!(
            data["web_auto_login"], true,
            "auth-optional instances must advertise auto-login to the web app: {data}"
        );

        // Control: with auth required (default), the signal reflects the DB
        // flag, which is off.
        let resp = json_get(&test_app(), "/api/instance").await;
        assert_eq!(parse_json(resp).await["web_auto_login"], false);
    }

    #[tokio::test]
    async fn auth_optional_auto_login_mints_admin_session_without_db_flag() {
        let app = test_app_with_auth(false); // web_auto_login stays false in the DB
        let resp = json_post(&app, "/api/auth/auto-login", serde_json::json!({})).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        assert!(
            data["token"].as_str().unwrap().starts_with("lific_sess_"),
            "auto-login must mint a real session under auth-optional: {data}"
        );
        assert_eq!(data["user"]["username"], "test-admin");
        assert_eq!(data["user"]["is_admin"], true);
    }

    #[tokio::test]
    async fn auth_optional_admin_settings_surface_keeps_real_flag() {
        // The OR only applies to the public bootstrap signal; the admin
        // settings endpoint must keep showing the stored value so the toggle
        // in the settings UI reflects reality.
        let app = test_app_with_auth(false);
        let resp = json_get(&app, "/api/instance/settings").await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(parse_json(resp).await["web_auto_login"], false);
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

    // ── LIF-406: the reachability guard runs on the settings update too ──

    /// An admin app whose instance reports the given bind host and public
    /// URL, the way `build_app` layers it in production.
    fn app_with_reachability(host: &str, public_url: Option<&str>) -> axum::Router {
        test_app().layer(axum::Extension(crate::server::Reachability {
            host: host.to_string(),
            public_url: public_url.map(str::to_string),
        }))
    }

    /// The runtime hole: `lific start` refuses to boot a publicly reachable
    /// instance with web auto-login on, but nothing stopped an admin turning
    /// it on afterwards, which hands an admin session to anyone who can load
    /// the page. The refusal must also persist nothing.
    #[tokio::test]
    async fn enabling_auto_login_is_refused_on_a_publicly_reachable_instance() {
        let app = app_with_reachability("127.0.0.1", Some("https://magi.tailb93ac8.ts.net"));

        let resp = json_patch(
            &app,
            "/api/instance/settings",
            serde_json::json!({ "web_auto_login": true }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let data = parse_json(resp).await;
        assert!(
            data["error"]
                .as_str()
                .unwrap_or("")
                .contains("web auto-login cannot be enabled"),
            "the refusal must say why: {data}"
        );

        let stored = parse_json(json_get(&app, "/api/instance/settings").await).await;
        assert_eq!(
            stored["web_auto_login"], false,
            "a refused patch must persist nothing: {stored}"
        );
        let public = parse_json(json_get(&app, "/api/instance").await).await;
        assert_eq!(public["web_auto_login"], false);
    }

    /// The same PATCH on a genuinely local instance is exactly as allowed as
    /// it has always been. This is the single-user install the feature is
    /// for, and LIF-406 must not break it.
    #[tokio::test]
    async fn enabling_auto_login_still_works_on_a_loopback_instance() {
        let app = app_with_reachability("127.0.0.1", Some("http://127.0.0.1:3456"));

        let resp = json_patch(
            &app,
            "/api/instance/settings",
            serde_json::json!({ "web_auto_login": true }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(parse_json(resp).await["web_auto_login"], true);

        let stored = parse_json(json_get(&app, "/api/instance/settings").await).await;
        assert_eq!(stored["web_auto_login"], true, "persisted: {stored}");

        // And auto-login itself now works, so the guard gates the flag
        // rather than the feature.
        let resp = json_post(&app, "/api/auth/auto-login", serde_json::json!({})).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// The guard is about one flag. Every other setting still patches on a
    /// publicly reachable instance, and turning auto-login OFF is never
    /// refused (that would strand an instance in the unsafe state).
    #[tokio::test]
    async fn the_reachability_guard_only_blocks_turning_auto_login_on() {
        let app = app_with_reachability("0.0.0.0", None);

        let resp = json_patch(
            &app,
            "/api/instance/settings",
            serde_json::json!({ "instance_name": "Acme Eng", "web_auto_login": false }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        assert_eq!(data["instance_name"], "Acme Eng");
        assert_eq!(data["web_auto_login"], false);
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
        let app = with_client_ip_test_layers(crate::api::router(db, &[]), test_peer()).layer(
            axum::Extension(crate::config::AuthConfig {
                allow_signup: true,
                required: true,
                secure_cookies: false,
            }),
        );

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

    // ── LIF-412: login verifies off the database writer ──────

    fn login_app(db: crate::db::DbPool) -> axum::Router {
        with_client_ip_test_layers(crate::api::router(db, &[]), test_peer()).layer(
            axum::Extension(crate::config::AuthConfig {
                allow_signup: true,
                required: true,
                secure_cookies: false,
            }),
        )
    }

    async fn login(app: &axum::Router, identity: &str, password: &str) -> (StatusCode, serde_json::Value) {
        let resp = json_post(
            app,
            "/api/auth/login",
            serde_json::json!({ "identity": identity, "password": password }),
        )
        .await;
        let status = resp.status();
        (status, parse_json(resp).await)
    }

    /// The whole login path now runs the Argon2 verify on the blocking pool
    /// with no database lock held, and mints the session afterwards. Every
    /// answer it can give has to survive that move: the right password logs
    /// in, the wrong one and an unknown account are both refused with the
    /// same message (no user enumeration), and an oversized password is
    /// refused without ever reaching Argon2.
    #[tokio::test]
    async fn login_answers_correctly_through_the_offloaded_verify() {
        let app = login_app(crate::db::open_memory().expect("test db"));
        let signup = serde_json::json!({
            "username": "offloaded",
            "email": "offloaded@test.com",
            "password": "securepass123",
        });
        assert_eq!(
            json_post(&app, "/api/auth/signup", signup).await.status(),
            StatusCode::OK,
            "signup hashes off the writer too"
        );

        let (status, data) = login(&app, "offloaded", "securepass123").await;
        assert_eq!(status, StatusCode::OK, "correct password: {data}");
        assert_eq!(data["user"]["username"], "offloaded");
        assert!(data["token"].as_str().unwrap().starts_with("lific_sess_"));

        // Email works as the identity too.
        let (status, data) = login(&app, "offloaded@test.com", "securepass123").await;
        assert_eq!(status, StatusCode::OK, "login by email: {data}");

        let (status, wrong) = login(&app, "offloaded", "not-the-password").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, unknown) = login(&app, "no-such-user", "not-the-password").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            wrong["error"], unknown["error"],
            "a wrong password and an unknown account must be indistinguishable"
        );

        let (status, oversized) = login(&app, "offloaded", &"x".repeat(2000)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            oversized["error"], wrong["error"],
            "an oversized password is refused with the same opaque message"
        );
    }

    /// LIF-214's deactivation message is produced after the verify, so it
    /// has to come back through the spawn_blocking path unchanged.
    #[tokio::test]
    async fn login_still_reports_a_deactivated_account() {
        let db = crate::db::open_memory().expect("test db");
        {
            let conn = db.write().unwrap();
            let user = crate::db::queries::users::create_user(
                &conn,
                &CreateUser {
                    username: "switched-off".into(),
                    email: "off@test.com".into(),
                    password: "securepass123".into(),
                    display_name: None,
                    is_admin: false,
                    is_bot: false,
                },
            )
            .unwrap();
            crate::db::queries::users::set_active(&conn, user.id, false).unwrap();
        }
        let app = login_app(db);

        let (status, data) = login(&app, "switched-off", "securepass123").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            data["error"].as_str().unwrap_or("").contains("deactivated"),
            "the deactivated-account message must survive the offload: {data}"
        );
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

    /// LIF-396: GET /api/auth/me was the one endpoint that escaped LIF-372's
    /// consolidation, still answering 400 "no user associated with this token"
    /// where every other gate answers 403 "authentication required".
    #[tokio::test]
    async fn auth_me_without_identity_is_403_authentication_required() {
        use tower::ServiceExt;

        let db = crate::db::open_memory().expect("test db");
        let app = with_client_ip_test_layers(crate::api::router(db, &[]), test_peer())
            .layer(axum::Extension(crate::realtime::RealtimeHub::new()))
            .layer(axum::Extension(crate::config::AuthConfig {
                allow_signup: true,
                required: true,
                secure_cookies: false,
            }))
            .layer(axum::Extension(None::<crate::db::models::AuthUser>))
            .layer(axum::Extension(
                None::<crate::resolve_caller::ResolvedIdentity>,
            ));

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/auth/me")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let data = parse_json(resp).await;
        assert_eq!(
            data["error"], "authentication required",
            "auth_me must use the shared require_user gate's error: {data}"
        );
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
    fn login_app_with_limiter(max: usize, peer: std::net::SocketAddr) -> axum::Router {
        let db = crate::db::open_memory().expect("test db");
        let limiter = std::sync::Arc::new(crate::ratelimit::RateLimiter::new(
            max,
            std::time::Duration::from_secs(15 * 60),
        ));
        with_client_ip_test_layers(crate::api::router(db, &[]), peer)
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
        let app = login_app_with_limiter(5, test_peer());
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
        let app = login_app_with_limiter(5, test_peer());
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
    async fn login_rate_limit_ignores_spoofed_xff_from_untrusted_peer() {
        // Regression for LIF-206: a directly connected attacker can rotate
        // XFF on every request, but must still consume one peer-IP bucket.
        let peer = std::net::SocketAddr::from(([203, 0, 113, 5], 4242));
        let app = login_app_with_limiter(2, peer);
        for (i, spoofed_xff) in ["198.51.100.1", "198.51.100.2"].iter().enumerate() {
            let (status, body) = login_attempt(&app, &format!("user{i}"), spoofed_xff).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert!(
                !is_rate_limited(&body),
                "attempt {i} should consume, but not exceed, the peer-IP budget: {body}"
            );
        }

        let (status, body) = login_attempt(&app, "third-user", "198.51.100.3").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            is_rate_limited(&body),
            "rotating spoofed XFF must not evade the untrusted peer's bucket: {body}"
        );
    }

    #[tokio::test]
    async fn login_rate_limit_isolates_distinct_ips() {
        // A victim identity is NOT locked out for an attacker on a different
        // IP, as long as the victim comes from their own IP and the identity
        // budget hasn't been exhausted. Sanity check that buckets are keyed
        // independently and the IP key is actually in play.
        let app = login_app_with_limiter(3, test_peer());
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
        let (status, other) = signup_attempt(&app, 60, "198.51.100.8").await;
        assert!(
            status == StatusCode::OK,
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
        let app = login_app_with_limiter(5, test_peer());
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
        let app = login_app_with_limiter(3, test_peer());
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

    // ── Instance-admin roster management (LIF-214) ───────────

    use crate::db::DbPool;
    use crate::db::models::{CreateUser, User};
    use serde_json::json;

    /// An instance with two admins, a plain member, and a bot owned by the
    /// first admin. Two admins by default so the last-admin guard is out of
    /// the way for the happy-path tests; the guard tests demote one first.
    fn roster_db() -> (DbPool, User, User, User, User) {
        let db = crate::db::open_memory().expect("test db");
        let conn = db.write().unwrap();

        let mk = |username: &str, is_admin: bool| {
            crate::db::queries::users::create_user(
                &conn,
                &CreateUser {
                    username: username.into(),
                    email: format!("{username}@test.com"),
                    password: "testpassword1".into(),
                    display_name: None,
                    is_admin,
                    is_bot: false,
                },
            )
            .unwrap()
        };

        let admin = mk("admin", true);
        let other_admin = mk("second-admin", true);
        let member = mk("member", false);
        let bot =
            crate::db::queries::users::ensure_bot(&conn, admin.id, "opencode", "OpenCode").unwrap();

        drop(conn);
        (db, admin, other_admin, member, bot)
    }

    fn reload(db: &DbPool, id: i64) -> User {
        let conn = db.read().unwrap();
        crate::db::queries::users::get_user_by_id(&conn, id).unwrap()
    }

    #[tokio::test]
    async fn admin_can_create_a_user_from_the_roster() {
        let (db, admin, _other, _member, _bot) = roster_db();
        let app = app_as_user(db.clone(), &admin);

        let resp = json_post(
            &app,
            "/api/users",
            serde_json::json!({ "username": "newcomer", "password": "securepass123" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let data = parse_json(resp).await;
        assert_eq!(data["username"], "newcomer");
        assert_eq!(data["is_admin"], false);
        assert_eq!(data["is_active"], true);

        // A username-only form still produces a schema-valid account, and it
        // is a human one.
        let created = reload(&db, data["id"].as_i64().unwrap());
        assert_eq!(created.email, "newcomer@local");
        assert!(!created.is_bot, "the roster never mints a bot");
    }

    #[tokio::test]
    async fn admin_can_promote_and_demote_a_member() {
        let (db, admin, _other, member, _bot) = roster_db();
        let app = app_as_user(db.clone(), &admin);

        let resp = json_post(&app, &format!("/api/users/{}/promote", member.id), json!({})).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(parse_json(resp).await["is_admin"], true);
        assert!(reload(&db, member.id).is_admin);

        let resp = json_post(&app, &format!("/api/users/{}/demote", member.id), json!({})).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(parse_json(resp).await["is_admin"], false);
        assert!(!reload(&db, member.id).is_admin);
    }

    #[tokio::test]
    async fn admin_can_deactivate_and_restore_a_member() {
        let (db, admin, _other, member, _bot) = roster_db();
        let app = app_as_user(db.clone(), &admin);

        let resp = json_post(
            &app,
            &format!("/api/users/{}/deactivate", member.id),
            json!({}),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(parse_json(resp).await["is_active"], false);
        assert!(!reload(&db, member.id).is_active);

        // Still on the roster, so an admin can find and restore them.
        let listed = parse_json(json_get(&app, "/api/users").await).await;
        assert!(
            listed
                .as_array()
                .unwrap()
                .iter()
                .any(|u| u["id"] == member.id && u["is_active"] == false),
            "a deactivated member stays visible: {listed}"
        );

        let resp = json_post(
            &app,
            &format!("/api/users/{}/reactivate", member.id),
            json!({}),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(parse_json(resp).await["is_active"], true);
        assert!(reload(&db, member.id).is_active);
    }

    /// Deactivation has to end access, not just paint a badge: the account's
    /// existing session dies with the flag.
    #[tokio::test]
    async fn deactivation_kills_the_accounts_sessions() {
        let (db, admin, _other, member, _bot) = roster_db();
        let token = {
            let conn = db.write().unwrap();
            crate::db::queries::users::create_session(&conn, member.id, None)
                .unwrap()
                .token
        };
        let app = app_as_user(db.clone(), &admin);

        let resp = json_post(
            &app,
            &format!("/api/users/{}/deactivate", member.id),
            json!({}),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let conn = db.read().unwrap();
        assert!(
            crate::db::queries::users::validate_session(&conn, &token).is_err(),
            "the deactivated user's session must no longer authenticate"
        );
    }

    /// Every mutation is admin-only. A plain member gets 403 and nothing
    /// moves.
    #[tokio::test]
    async fn a_non_admin_cannot_reach_any_roster_mutation() {
        let (db, admin, _other, member, _bot) = roster_db();
        let app = app_as_user(db.clone(), &member);

        let resp = json_post(
            &app,
            "/api/users",
            serde_json::json!({ "username": "smuggled", "password": "securepass123" }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "create is admin-only");

        for action in ["promote", "demote", "deactivate", "reactivate"] {
            let resp = json_post(&app, &format!("/api/users/{}/{action}", admin.id), json!({})).await;
            assert_eq!(
                resp.status(),
                StatusCode::FORBIDDEN,
                "{action} must be admin-only"
            );
        }

        // The target admin is untouched by any of it.
        let still = reload(&db, admin.id);
        assert!(still.is_admin && still.is_active);
        assert!(
            crate::db::queries::users::get_user_by_username(&db.read().unwrap(), "smuggled")
                .is_err(),
            "the rejected create wrote nothing"
        );
    }

    /// The instance must never end up with nobody who can administer it.
    #[tokio::test]
    async fn the_last_admin_cannot_be_demoted_or_deactivated() {
        let (db, admin, other, _member, _bot) = roster_db();
        let app = app_as_user(db.clone(), &admin);

        // Down to a single admin.
        let resp = json_post(&app, &format!("/api/users/{}/demote", other.id), json!({})).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            {
                let conn = db.read().unwrap();
                crate::db::queries::users::count_active_admins(&conn).unwrap()
            },
            1
        );

        for action in ["demote", "deactivate"] {
            let resp =
                json_post(&app, &format!("/api/users/{}/{action}", admin.id), json!({})).await;
            assert_eq!(
                resp.status(),
                StatusCode::CONFLICT,
                "{action} of the last admin must be refused"
            );
            let data = parse_json(resp).await;
            assert!(
                data["error"].as_str().unwrap().contains("last instance admin"),
                "{action}: {data}"
            );
        }

        let still = reload(&db, admin.id);
        assert!(
            still.is_admin && still.is_active,
            "the last admin survived both attempts"
        );
    }

    /// A connected tool is not a roster member. None of these endpoints may
    /// be pointed at one, least of all `promote`, which would hand an agent's
    /// API key the run of the instance.
    #[tokio::test]
    async fn bot_identities_are_not_roster_targets() {
        let (db, admin, _other, _member, bot) = roster_db();
        let app = app_as_user(db.clone(), &admin);

        for action in ["promote", "demote", "deactivate", "reactivate"] {
            let resp = json_post(&app, &format!("/api/users/{}/{action}", bot.id), json!({})).await;
            assert_eq!(
                resp.status(),
                StatusCode::BAD_REQUEST,
                "{action} must refuse a bot target"
            );
            let data = parse_json(resp).await;
            assert!(
                data["error"].as_str().unwrap().contains("Connected Tools"),
                "{action}: {data}"
            );
        }

        let still = reload(&db, bot.id);
        assert!(!still.is_admin && still.is_active, "the bot is unchanged");
    }
}
