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
    // so an email-only key is bypassed by rotating addresses. Check the IP
    // first: a blocked source must not be able to allocate unlimited identity
    // keys while short-circuiting the later check.
    let email_key = format!("signup:{}", input.email.to_lowercase());
    let ip_key = format!(
        "signup_ip:{}",
        crate::ratelimit::client_ip(peer.ip(), &headers, &trusted_proxies)
    );
    if let Some(Extension(ref rl)) = limiter {
        if !rl.check(&ip_key) {
            let retry = rl.retry_after(&ip_key);
            return Err(LificError::BadRequest(
                crate::ratelimit::retry_after_message("too many signup attempts", retry),
            ));
        }
        if !rl.check(&email_key) {
            let retry = rl.retry_after(&email_key);
            return Err(LificError::BadRequest(
                crate::ratelimit::retry_after_message("too many signup attempts", retry),
            ));
        }
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
    let password_hash =
        tokio::task::spawn_blocking(move || crate::db::queries::users::hash_password(&password))
            .await
            .map_err(|e| LificError::Internal(format!("password hashing task failed: {e}")))??;

    // One transaction for the policy re-check, the first-admin decision, the
    // insert and the session. A bare writer guard ran each statement in its
    // own implicit transaction, so a failure after the insert (or a crash)
    // left an account nobody could sign in to, and the zero-user count was
    // only as atomic as the connection happened to be.
    let (user, session) = db.transaction(|conn| {
        // Authoritative re-check: the policy could have been changed by an
        // admin between the read above and this write.
        let settings = crate::db::queries::settings::get(conn)?;
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

        let user =
            crate::db::queries::users::insert_user_with_hash(conn, &new_user, &password_hash)?;
        let session = crate::db::queries::users::create_session(
            conn,
            user.id,
            Some(settings.session_lifetime_days * 24),
        )?;
        Ok((user, session))
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
    //
    // Both slots are *reserved* up front, before Argon2 runs, and refunded if
    // the login succeeds. The previous shape peeked and recorded a failure
    // afterwards, which bounds nothing under concurrency: a hundred requests
    // can all peek "under the limit" in the window before any of them records,
    // and then all hundred proceed into the deliberately expensive verify.
    // Reserving first means the limiter admits at most `max_attempts`
    // concurrent verifies, which is the property that actually protects the
    // CPU. Refunding on success keeps the cost where it belongs: a failed
    // login spends budget, a successful one does not.
    let id_key = format!("login_id:{}", input.identity.to_lowercase());
    let ip_key = format!(
        "login_ip:{}",
        crate::ratelimit::client_ip(peer.ip(), &headers, &trusted_proxies)
    );
    let reservation = match &limiter {
        Some(Extension(rl)) => match crate::ratelimit::Reservation::acquire(rl, &ip_key, &id_key) {
            Ok(reservation) => Some(reservation),
            Err(rejected) => {
                let retry = match rejected {
                    crate::ratelimit::ReservationRejection::First => rl.retry_after(&ip_key),
                    crate::ratelimit::ReservationRejection::Second => rl.retry_after(&id_key),
                };
                return Err(LificError::BadRequest(
                    crate::ratelimit::retry_after_message("too many login attempts", retry),
                ));
            }
        },
        None => None,
    };

    // From here to the refund, every failure path simply returns: the
    // reservation stands, which is what "this attempt was spent" means.
    let user = authenticate_off_writer(&db, &input.identity, &input.password).await?;

    // The verify above ran with no lock held, by design. Everything it
    // established has to be re-established inside the transaction that mints
    // the session, or a password change committed during those tens of
    // milliseconds hands out a week-long credential for the old password.
    let verified_hash = user.password_hash.clone();
    let (user, session) = db.transaction(|tx| {
        let user = crate::db::queries::users::finalize_login(tx, user.id, &verified_hash)?;
        let lifetime_days = crate::db::queries::settings::get(tx)
            .map(|s| s.session_lifetime_days)
            .unwrap_or(30);
        let session =
            crate::db::queries::users::create_session(tx, user.id, Some(lifetime_days * 24))?;
        Ok((user, session))
    })?;
    // Only now: the session exists, so this attempt was not an attack and
    // should not have cost the account or the address any budget. A
    // finalization failure (superseded hash, deactivated account) returns
    // above with the reservation intact, because from the limiter's point of
    // view that is a failed login.
    if let Some(reservation) = reservation {
        reservation.refund();
    }

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

/// PATCH /api/instance/settings: partial update, admin only, and only from a
/// recent browser session.
///
/// Every patch, not a classified subset. Several of these fields expand who can
/// reach the instance: `allow_signup` opens self-service registration,
/// `signup_email_domains` widens who may use it, `web_auto_login` hands an
/// admin session to anyone who loads the page, and `authz_enforced` off removes
/// project-scoped authorization wholesale. Trying to gate only the "expanding"
/// direction of each one means a per-field rule that has to be re-derived every
/// time a field is added, and a single miss reopens the hole. There are a
/// handful of these writes a week, made by a human at a keyboard, so the cost
/// of gating all of them is nil and the rule is one sentence.
pub(super) async fn instance_settings_patch(
    State(db): State<DbPool>,
    Extension(realtime): Extension<RealtimeHub>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    reachability: Option<Extension<crate::server::Reachability>>,
    headers: HeaderMap,
    Json(input): Json<InstanceSettingsPatchReq>,
) -> Result<Json<serde_json::Value>, LificError> {
    require_admin(&identity)?;
    let admin = require_user(&identity)?;
    let session_token = crate::auth::recent_session_token(&headers)?;

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
    // The reachability guard above is a precheck on the request; the
    // authorization check and the persisted mutation are one transaction, so a
    // lockdown cannot land between them.
    let (s, authz_changed) = db.transaction(move |tx| {
        let fresh = crate::auth::revalidate_recent_session(tx, &session_token, admin.id)?;
        crate::auth::require_fresh_admin(&fresh)?;
        let previous_authz_enforced = crate::db::queries::settings::get(tx)?.authz_enforced;
        let settings = crate::db::queries::settings::update(tx, patch)?;
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

#[derive(serde::Deserialize)]
pub(super) struct RefreshSessionRequest {
    /// Required in password mode, omitted on a passwordless instance.
    #[serde(default)]
    password: Option<String>,
}

/// POST /api/auth/me/refresh: swap this browser's session for a fresh one, for
/// the **same** account.
///
/// This exists because the recent-authentication rule on granting actions had
/// no honest way to be satisfied from a tab that had been open a while. The
/// only tools available were `/auth/login`, which mints a session for whatever
/// identity the credentials name, and `/auth/auto-login`, which mints one for
/// *the instance's first admin*. On a multi-admin instance the latter silently
/// swaps who the tab is, and both set a cookie before anyone has checked that
/// the resulting session belongs to the person already signed in. The client
/// then had to notice the swap after the fact, by which point the cookie was
/// already sent.
///
/// So: one endpoint whose entire contract is "same user, newer session".
///
/// - The caller must present a live `lific_sess_` bearer token. An API key,
///   an OAuth token or no credential is refused: those are precisely the
///   credentials the recency rule exists to keep out.
/// - In password mode a `password` is required, verified off the writer
///   (Argon2 is expensive), and then re-confirmed against the stored hash
///   inside the transaction, so a password changed mid-request cannot be used.
/// - In passwordless mode (`web_auto_login`, or `[auth] required = false`)
///   the password may be omitted, but the mode is re-read inside the
///   transaction, so an admin turning it off wins the race.
/// - Exactly one session is deleted: the one presented. Every other session
///   the account has is left alone, which is the difference between this and
///   the password-change lockdown.
/// - Nothing is written and no cookie is emitted until every check has passed.
///   A failure leaves the old bearer token and the old cookie working.
// Axum handlers take their dependencies as extractors, so the count is the
// dependency list, not a design smell to be refactored into a struct.
#[allow(clippy::too_many_arguments)]
pub(super) async fn refresh_session(
    State(db): State<DbPool>,
    Extension(auth_cfg): Extension<crate::config::AuthConfig>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Extension(trusted_proxies): Extension<Arc<[crate::ratelimit::IpNetwork]>>,
    limiter: Option<Extension<std::sync::Arc<crate::ratelimit::RateLimiter>>>,
    headers: HeaderMap,
    body: Option<Json<RefreshSessionRequest>>,
) -> Result<impl IntoResponse, LificError> {
    let caller = require_user(&identity)?;
    // Shape check only. Which user it names, and whether it is still live, is
    // established inside the transaction below.
    let session_token = crate::auth::session_bearer_token(&headers)?;
    let supplied_password = body.and_then(|Json(input)| input.password);

    // Read the mode and the current hash on a pooled connection so the
    // expensive verify happens with no lock held. Both are re-read
    // authoritatively inside the transaction.
    let (passwordless, current_hash) = with_read(&db, |conn| {
        let settings = crate::db::queries::settings::get(conn)?;
        let user = crate::db::queries::users::get_user_by_id(conn, caller.id)?;
        Ok((
            settings.web_auto_login || !auth_cfg.required,
            user.password_hash,
        ))
    })?;

    // A password-bearing refresh runs Argon2, so it is rate-limited on the
    // same reserve-then-refund terms as login, on its own key namespace: this
    // is an authenticated caller re-proving themselves, and it must not share
    // (or drain) the login budget for the same account. Both slots are taken
    // before the verify, so at most `max_attempts` verifies can be in flight.
    // A passwordless refresh does no expensive work and needs no reservation.
    let ip_key = format!(
        "reauth_ip:{}",
        crate::ratelimit::client_ip(peer.ip(), &headers, &trusted_proxies)
    );
    let user_key = format!("reauth_user:{}", caller.id);
    let reservation = match (&supplied_password, &limiter) {
        (Some(_), Some(Extension(rl))) => {
            match crate::ratelimit::Reservation::acquire(rl, &ip_key, &user_key) {
                Ok(reservation) => Some(reservation),
                Err(rejected) => {
                    let retry = match rejected {
                        crate::ratelimit::ReservationRejection::First => rl.retry_after(&ip_key),
                        crate::ratelimit::ReservationRejection::Second => {
                            rl.retry_after(&user_key)
                        }
                    };
                    return Err(LificError::BadRequest(
                        crate::ratelimit::retry_after_message(
                            "too many confirmation attempts",
                            retry,
                        ),
                    ));
                }
            }
        }
        _ => None,
    };

    let verified_hash = match supplied_password {
        Some(password) => {
            crate::db::queries::users::reject_oversized_password(&password)?;
            let hash = current_hash.clone();
            let ok = tokio::task::spawn_blocking(move || {
                crate::db::queries::users::verify_password(&password, &hash).unwrap_or(false)
            })
            .await
            .map_err(|e| LificError::Internal(format!("password verification task failed: {e}")))?;
            if !ok {
                return Err(LificError::BadRequest("incorrect password".into()));
            }
            Some(current_hash)
        }
        None => {
            if !passwordless {
                return Err(LificError::BadRequest(
                    "your password is required to confirm this".into(),
                ));
            }
            None
        }
    };

    let (user, session) = db.transaction(|tx| {
        // The presented session must still be live, and must still be this
        // caller's. Not `revalidate_recent_session`: an old session is exactly
        // what this endpoint is for.
        let user =
            crate::db::queries::users::validate_session(tx, &session_token).map_err(|_| {
                LificError::BadRequest(crate::db::queries::users::INVALID_SESSION_MESSAGE.into())
            })?;
        if user.id != caller.id {
            return Err(LificError::BadRequest(
                crate::db::queries::users::INVALID_SESSION_MESSAGE.into(),
            ));
        }
        if !crate::db::queries::users::credential_is_live(tx, &user)? {
            return Err(LificError::BadRequest(
                "this account has been deactivated. Ask an admin to restore it.".into(),
            ));
        }

        let settings = crate::db::queries::settings::get(tx)?;
        match &verified_hash {
            // Password path: the hash verified moments ago must still be the
            // stored one, or the password presented is the old one.
            Some(hash) => {
                if &user.password_hash != hash {
                    return Err(LificError::BadRequest("incorrect password".into()));
                }
            }
            // Passwordless path: re-read the mode, so an admin who has just
            // turned it off wins.
            None => {
                if !settings.web_auto_login && auth_cfg.required {
                    return Err(LificError::BadRequest(
                        "your password is required to confirm this".into(),
                    ));
                }
            }
        }

        // Only the presented session is replaced. Nothing here consults
        // `resolve_caller`, so there is no first-admin fallback to land on:
        // the new session is for `user.id` and can be for nobody else.
        crate::db::queries::users::delete_session(tx, &session_token)?;
        let session = crate::db::queries::users::create_session(
            tx,
            user.id,
            Some(settings.session_lifetime_days * 24),
        )?;
        Ok((user, session))
    })?;

    // The confirmation worked, so it was not an attack: give the slots back.
    // Every failure above returns with the reservation intact, which is what
    // makes repeated wrong passwords hit the limit.
    if let Some(reservation) = reservation {
        reservation.refund();
    }

    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        "set-cookie",
        session_cookie(&session.token, &session.expires_at, auth_cfg.secure_cookies)
            .parse()
            .unwrap(),
    );

    Ok((
        resp_headers,
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

/// POST /api/auth/me/password — change password after verifying the current
/// one. LIF-190.
///
/// LIF-205: a password change is the "I've been compromised, lock it down"
/// action, so it runs the full account lockdown
/// ([`crate::db::queries::users::lock_down_account`]): every session, API key,
/// OAuth token, in-flight authorization code and approved device grant for the
/// user **and every bot they own** dies. A fresh session is then minted for the
/// current browser so the legitimate caller stays logged in instead of being
/// bounced to /login, and that replacement token comes back in the response
/// body for clients that hold it outside the cookie.
///
/// Verification, lockdown and replacement all happen under one hold of the
/// writer, inside one savepoint. SQLite serializes writers, so a credential
/// creation racing this either lands entirely before it (and is revoked) or
/// entirely after it (and revalidates against the post-lockdown state).
// Axum handlers take their dependencies as extractors; the count is the
// dependency list, not a design smell.
#[allow(clippy::too_many_arguments)]
pub(super) async fn change_password(
    State(db): State<DbPool>,
    Extension(auth_cfg): Extension<crate::config::AuthConfig>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Extension(realtime): Extension<crate::realtime::RealtimeHub>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Extension(trusted_proxies): Extension<Arc<[crate::ratelimit::IpNetwork]>>,
    limiter: Option<Extension<std::sync::Arc<crate::ratelimit::RateLimiter>>>,
    headers: HeaderMap,
    Json(input): Json<ChangePasswordRequest>,
) -> Result<impl IntoResponse, LificError> {
    let user = require_user(&identity)?;
    crate::db::queries::users::reject_oversized_password(&input.current_password)?;

    // Two Argon2 operations run below (verify the old password, hash the new
    // one), which makes this the most expensive authenticated request in the
    // API. It is reserved on the same terms as login and refresh, on its own
    // key namespace so it neither shares nor drains theirs: an authenticated
    // caller changing their password is a different budget from someone
    // guessing their way in. Reserving before the work is what bounds how many
    // of these can be in flight at once.
    let ip_key = format!(
        "password_change_ip:{}",
        crate::ratelimit::client_ip(peer.ip(), &headers, &trusted_proxies)
    );
    let user_key = format!("password_change_user:{}", user.id);
    let reservation = match &limiter {
        Some(Extension(rl)) => {
            match crate::ratelimit::Reservation::acquire(rl, &ip_key, &user_key) {
                Ok(reservation) => Some(reservation),
                Err(rejected) => {
                    let retry = match rejected {
                        crate::ratelimit::ReservationRejection::First => rl.retry_after(&ip_key),
                        crate::ratelimit::ReservationRejection::Second => {
                            rl.retry_after(&user_key)
                        }
                    };
                    return Err(LificError::BadRequest(
                        crate::ratelimit::retry_after_message(
                            "too many password change attempts",
                            retry,
                        ),
                    ));
                }
            }
        }
        None => None,
    };

    // Capture the stored hash on a pooled read connection. Both Argon2 calls
    // (verify the old password, hash the new one) then run on the blocking
    // pool with NO lock held: this used to happen inside the writer, which
    // stalled every other write in the process for the duration of two
    // deliberately expensive hashes. Same reasoning as LIF-412 for login.
    let captured_hash = with_read(&db, |conn| {
        Ok(crate::db::queries::users::get_user_by_id(conn, user.id)?.password_hash)
    })?;

    let current_password = input.current_password.clone();
    let new_password = input.new_password.clone();
    let verify_hash = captured_hash.clone();
    let prepared_hash = tokio::task::spawn_blocking(move || {
        let ok = crate::db::queries::users::verify_password(&current_password, &verify_hash)?;
        if !ok {
            return Err(LificError::BadRequest(
                "current password is incorrect".into(),
            ));
        }
        // Policy check and hash together, so a rejected new password costs
        // nothing beyond the verify that was going to happen anyway.
        crate::db::queries::users::prepare_new_password(&new_password)
    })
    .await
    .map_err(|e| LificError::Internal(format!("password hashing task failed: {e}")))??;

    let session = db.transaction(|tx| {
        // Everything established off-writer is re-established here. A second
        // password change, or an operator reset, that committed while Argon2
        // was running means the "current password" just verified is already
        // the old one, and this must not overwrite the newer hash.
        let full = crate::db::queries::users::get_user_by_id(tx, user.id)?;
        if !crate::db::queries::users::credential_is_live(tx, &full)? {
            return Err(LificError::BadRequest(
                "this account has been deactivated. Ask an admin to restore it.".into(),
            ));
        }
        if full.password_hash != captured_hash {
            return Err(LificError::BadRequest(
                "current password is incorrect".into(),
            ));
        }
        crate::db::queries::users::update_password_hash(tx, user.id, &prepared_hash)?;
        // Sever everything the old password could still be reaching
        // through (including anything an attacker holds), then issue
        // exactly one fresh session for this browser.
        crate::db::queries::users::lock_down_account(tx, user.id)?;
        crate::db::queries::users::create_session(tx, user.id, None)
    })?;
    // Only after the whole change has committed. Every failure above returns
    // with the reservation intact: a wrong current password, a rejected new
    // password, a lost race against another change, or a hashing fault all
    // count as a spent attempt, which is the fail-safe direction.
    if let Some(reservation) = reservation {
        reservation.refund();
    }
    realtime.revoke_user(user.id);

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

/// DELETE /api/auth/me/sessions: sign out everywhere and revoke access.
///
/// Runs the same full lockdown a password change does
/// ([`crate::db::queries::users::lock_down_account`]) and mints no replacement:
/// every session, API key, OAuth token, in-flight authorization code and
/// approved device grant for this user and their bots is gone when this
/// returns, this browser's session included. The cookie is cleared so the
/// current browser drops to logged-out. LIF-190.
pub(super) async fn revoke_all_sessions(
    State(db): State<DbPool>,
    Extension(auth_cfg): Extension<crate::config::AuthConfig>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Extension(realtime): Extension<crate::realtime::RealtimeHub>,
) -> Result<impl IntoResponse, LificError> {
    let user = require_user(&identity)?;
    with_write(&db, |conn| {
        crate::db::queries::savepoint(conn, "revoke_all_sessions", || {
            crate::db::queries::users::lock_down_account(conn, user.id)
        })
    })?;
    realtime.revoke_user(user.id);
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

/// POST /api/auth/keys: mint an API key bound to the caller.
///
/// Requires a browser session created within the last 15 minutes, presented as
/// a bearer token. An API key may no longer mint another key: a leaked key is
/// then a leaked key, not a key factory that survives the account lockdown that
/// was supposed to kill it. OAuth tokens were already refused on this route by
/// the authentication middleware.
///
/// The key material is drawn before the writer is taken, then the session is
/// revalidated and the key inserted in one transaction. Because SQLite
/// serializes writers, an account lockdown either wins the race outright (this
/// transaction then finds no session and writes nothing) or loses it (and
/// revokes the key it finds). Neither order can leave a live key behind a
/// revoked session.
pub(super) async fn create_key(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Extension(manager): Extension<std::sync::Arc<api_keys_simplified::ApiKeyManagerV0>>,
    headers: HeaderMap,
    Json(input): Json<CreateKeyRequest>,
) -> Result<Json<serde_json::Value>, LificError> {
    let user = require_user(&identity)?;
    let session_token = crate::auth::recent_session_token(&headers)?;

    let name = input.name.trim().to_string();
    if name.is_empty() {
        return Err(LificError::BadRequest("key name cannot be empty".into()));
    }

    let prepared = crate::auth::PreparedApiKey::generate(&manager)?;
    let plaintext = db.transaction(|tx| {
        crate::auth::revalidate_recent_session(tx, &session_token, user.id)?;
        // LIF-391: the key is created already bound to the caller, in one write.
        prepared.insert(tx, &name, None, Some(user.id))
    })?;

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

    // No recency requirement: revoking a key only takes access away. But the
    // "am I an admin, so may I revoke somebody else's key" question is
    // answered from state read inside this transaction, not from the snapshot
    // the middleware attached before the request was routed.
    db.transaction(|tx| {
        let caller = crate::auth::fresh_caller(tx, user.id)?;
        crate::db::queries::users::revoke_user_key(tx, id, caller.id, caller.is_admin)
    })?;

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

/// POST /api/auth/bots: connect a tool. Finds or creates the caller's bot for
/// it and mint that bot's API key.
///
/// Same authorization rule as [`create_key`], and for the same reason: this
/// hands out a durable credential. The bot lookup/creation, the
/// already-connected check and the key insert are one transaction, so a failure
/// anywhere leaves neither a half-created identity nor a credential without an
/// identity.
pub(super) async fn create_bot(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    Extension(manager): Extension<std::sync::Arc<api_keys_simplified::ApiKeyManagerV0>>,
    headers: HeaderMap,
    Json(input): Json<CreateBotRequest>,
) -> Result<Json<serde_json::Value>, LificError> {
    let user = require_user(&identity)?;
    let session_token = crate::auth::recent_session_token(&headers)?;

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

    let prepared = crate::auth::PreparedApiKey::generate(&manager)?;
    let (bot_user, plaintext_key) = db.transaction(|tx| {
        crate::auth::revalidate_recent_session(tx, &session_token, user.id)?;

        // Reuse the shared find-or-create seam (LIFIC-13) so a web-connected
        // bot is indistinguishable from one minted at OAuth approval or via
        // `lific connect`.
        let bot_user = crate::db::queries::users::ensure_bot(tx, user.id, &tool, display_name)?;

        // If the bot already has a live credential (API key or OAuth token)
        // it's already connected, so refuse rather than silently minting a fresh
        // credential for an active tool. Inside the transaction, so a
        // concurrent connect cannot slip between the check and the insert.
        if crate::db::queries::users::bot_is_connected(tx, bot_user.id)? {
            return Err(LificError::BadRequest(format!(
                "{display_name} is already connected"
            )));
        }

        // Generate a new API key, bound to the bot user by the same insert
        // (LIF-391).
        let plaintext_key = prepared.insert(tx, &bot_username, None, Some(bot_user.id))?;
        Ok((bot_user, plaintext_key))
    })?;

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

    db.transaction(|tx| {
        let caller = crate::auth::fresh_caller(tx, user.id)?;
        crate::db::queries::users::disconnect_bot(tx, id, caller.id, caller.is_admin)
    })?;

    Ok(Json(serde_json::json!({"disconnected": true})))
}

pub(super) async fn delete_bot(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
) -> Result<Json<serde_json::Value>, LificError> {
    let user = require_user(&identity)?;

    db.transaction(|tx| {
        let caller = crate::auth::fresh_caller(tx, user.id)?;
        crate::db::queries::users::delete_bot(tx, id, caller.id, caller.is_admin)
    })?;

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

/// POST /api/users: create a local account. Admin only, and only from a
/// **recent browser session**, on exactly the same terms as minting an API key.
///
/// Creating an account, especially `is_admin: true`, is another way to walk out
/// of an account lockdown: an attacker holding a stolen key mints themselves a
/// second admin, and the password change that revoked the key leaves that
/// account untouched, because a lockdown scopes to one user's credentials and
/// this is a different user. Requiring a session the human authenticated
/// minutes ago closes it the same way it is closed for keys and bots. The
/// session is revalidated inside the write transaction, so a lockdown either
/// lands wholly before (and this fails) or wholly after (and the account is a
/// visible new row on the roster, not a silent one).
///
/// Never mints a bot: `is_bot` is not part of the request at all. Connected
/// tools come from `POST /api/auth/bots` and the OAuth flow, which own the
/// (owner, tool) identity rules.
pub(super) async fn create_user_handler(
    State(db): State<DbPool>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    headers: HeaderMap,
    Json(input): Json<CreateUserRequest>,
) -> Result<Json<UserListItem>, LificError> {
    require_admin(&identity)?;
    let admin = require_user(&identity)?;
    let session_token = crate::auth::recent_session_token(&headers)?;

    let username = input.username.trim().to_string();
    let email = match input.email.as_deref().map(str::trim) {
        Some(e) if !e.is_empty() => e.to_string(),
        _ => format!("{username}@local"),
    };

    let user = db.transaction(|tx| {
        // Authoritative: `require_admin` above ran on the middleware's
        // snapshot, which predates this transaction. A demotion committed
        // since is only visible here.
        let fresh = crate::auth::revalidate_recent_session(tx, &session_token, admin.id)?;
        crate::auth::require_fresh_admin(&fresh)?;
        crate::db::queries::users::create_user(
            tx,
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

/// POST /api/users/{id}/promote: grant instance admin. Admin only, and only
/// from a recent browser session.
///
/// Same reasoning as [`create_user_handler`]: promoting an existing account is
/// the other way to leave yourself an admin that survives the victim's
/// recovery. Demote, deactivate and reactivate are deliberately NOT gated this
/// way. They only ever *reduce* access, so they cannot be used to persist, and
/// they are exactly what an admin reaches for in a hurry while responding to a
/// compromise. Making them re-prompt would slow down the containment without
/// closing anything.
pub(super) async fn promote_user(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    headers: HeaderMap,
) -> Result<Json<UserListItem>, LificError> {
    require_admin(&identity)?;
    let admin = require_user(&identity)?;
    let session_token = crate::auth::recent_session_token(&headers)?;
    let user = db.transaction(|tx| {
        let fresh = crate::auth::revalidate_recent_session(tx, &session_token, admin.id)?;
        crate::auth::require_fresh_admin(&fresh)?;
        crate::db::queries::users::set_admin_guarded(tx, id, true)
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
    let caller = require_user(&identity)?;
    // Ungated on recency (this reduces access), but the admin check is
    // re-run inside the transaction: a caller demoted since the request
    // arrived must not be able to demote anyone else on the way out.
    let user = db.transaction(|tx| {
        let fresh = crate::auth::fresh_caller(tx, caller.id)?;
        crate::auth::require_fresh_admin(&fresh)?;
        crate::db::queries::users::set_admin_guarded(tx, id, false)
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
    Extension(realtime): Extension<crate::realtime::RealtimeHub>,
) -> Result<Json<UserListItem>, LificError> {
    require_admin(&identity)?;
    let caller = require_user(&identity)?;
    // Ungated on recency, admin re-checked inside the transaction. The scoped
    // ids are collected there too: `set_active` deletes the sessions of the
    // account *and* of every bot it owns, and a live websocket for any of them
    // has to be told, not left to notice on its next periodic revalidation.
    let (user, scoped) = db.transaction(|tx| {
        let fresh = crate::auth::fresh_caller(tx, caller.id)?;
        crate::auth::require_fresh_admin(&fresh)?;
        let user = crate::db::queries::users::set_active(tx, id, false)?;
        let scoped = crate::db::queries::users::owned_bot_ids(tx, user.id)?;
        Ok((user, scoped))
    })?;

    // After commit, so a socket that revalidates on the nudge reads the
    // deactivated state rather than racing it. The periodic check remains as
    // the fallback for anything that misses this.
    realtime.revoke_user(user.id);
    for bot_id in scoped {
        realtime.revoke_user(bot_id);
    }
    Ok(Json(user.into()))
}

/// POST /api/users/{id}/reactivate: restore a deactivated account. Admin
/// only, and only from a recent browser session.
///
/// Restoring an account is an *expansion* of access, and one with a
/// particularly convenient shape for an attacker: deactivate is the containment
/// action, so an attacker holding an admin API key could simply undo it, or
/// wake a dormant account they had already prepared. It therefore joins create
/// and promote behind the recent-session rule. The credentials torn down at
/// deactivation are still not restored; the user signs in again.
///
/// Its opposite, `deactivate`, stays ungated along with `demote`: those reduce
/// access, cannot be used to persist it, and are what an admin reaches for in a
/// hurry.
pub(super) async fn reactivate_user(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Extension(identity): Extension<Option<crate::resolve_caller::ResolvedIdentity>>,
    headers: HeaderMap,
) -> Result<Json<UserListItem>, LificError> {
    require_admin(&identity)?;
    let admin = require_user(&identity)?;
    let session_token = crate::auth::recent_session_token(&headers)?;
    let user = db.transaction(|tx| {
        let fresh = crate::auth::revalidate_recent_session(tx, &session_token, admin.id)?;
        crate::auth::require_fresh_admin(&fresh)?;
        crate::db::queries::users::set_active(tx, id, true)
    })?;
    Ok(Json(user.into()))
}

#[cfg(test)]
mod tests {
    use crate::api::test_helpers::*;
    use axum::http::StatusCode;

    // ── Account lockdown, over the real middleware stack ─────────
    //
    // `test_app`/`app_as_user` inject `Extension<Option<AuthUser>>` directly,
    // which is exactly the wrong shape for these: the whole question is what
    // the authentication middleware makes of a bearer token *after* a
    // lockdown has run. Everything below drives `api::router` behind the real
    // `auth::require_api_key`, fed real tokens, mirroring `server.rs`'s
    // `authed_routes` wiring.
    mod lockdown {
        use crate::api::test_helpers::{test_peer, with_client_ip_test_layers};
        use crate::db::DbPool;
        use axum::http::{Request, StatusCode};
        use http_body_util::BodyExt;
        use rusqlite::params;
        use tower::ServiceExt;

        const PASSWORD: &str = "correct horse battery";

        struct Fixture {
            db: DbPool,
            app: axum::Router,
            user_id: i64,
            bot_id: i64,
            /// A live browser session for the account, created just now.
            session: String,
            /// An API key bound to the human.
            human_key: String,
            /// An API key bound to the tool bot the human owns.
            bot_key: String,
            /// An OAuth access token bound to the human.
            human_oauth: String,
            /// An OAuth access token bound to the bot.
            bot_oauth: String,
            /// Another human account on the instance, and their API key.
            stranger_id: i64,
            stranger_key: String,
            /// An unbound operator key: it names no user.
            operator_key: String,
            /// Present only for the rate-limit tests.
            limiter: Option<std::sync::Arc<crate::ratelimit::RateLimiter>>,
        }

        fn real_stack_with_limiter(
            db: &DbPool,
            limiter: Option<std::sync::Arc<crate::ratelimit::RateLimiter>>,
        ) -> axum::Router {
            let app = real_stack(db);
            match limiter {
                Some(rl) => app.layer(axum::Extension(rl)),
                None => app,
            }
        }

        fn real_stack(db: &DbPool) -> axum::Router {
            let manager = crate::auth::create_key_manager().unwrap();
            let auth_state = crate::auth::AuthState {
                db: db.clone(),
                manager: manager.clone(),
                public_url: "https://example.com".into(),
                required: true,
            };
            // `with_client_ip_test_layers` supplies the peer and trusted-proxy
            // extensions the rate-limited routes take, exactly as `lific start`
            // does.
            with_client_ip_test_layers(crate::api::router(db.clone(), &[]), test_peer())
                .layer(axum::Extension(crate::realtime::RealtimeHub::new()))
                .layer(axum::Extension(crate::config::AuthConfig {
                    allow_signup: true,
                    required: true,
                    secure_cookies: false,
                }))
                .layer(axum::Extension(std::sync::Arc::new(manager)))
                .layer(axum::middleware::from_fn_with_state(
                    auth_state,
                    crate::auth::require_api_key,
                ))
        }

        fn insert_oauth_token(db: &DbPool, suffix: &str, user_id: i64) -> String {
            let token = format!("lific_at_{suffix}");
            let hash = crate::auth::sha256_hex(token.as_bytes());
            let expires = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
            let conn = db.write().unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO oauth_clients (client_id, client_name, redirect_uris)
                 VALUES ('test-client', 'Test', '[\"http://localhost\"]')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO oauth_tokens (access_token, client_id, expires_at, scope, user_id)
                 VALUES (?1, 'test-client', ?2, 'mcp', ?3)",
                params![hash, expires, user_id],
            )
            .unwrap();
            token
        }

        fn fixture() -> Fixture {
            fixture_inner(None)
        }

        /// A fixture whose router carries a rate limiter with `max` attempts.
        fn fixture_with_limiter(max: usize) -> Fixture {
            fixture_inner(Some(std::sync::Arc::new(
                crate::ratelimit::RateLimiter::new(max, std::time::Duration::from_secs(15 * 60)),
            )))
        }

        fn fixture_inner(
            limiter: Option<std::sync::Arc<crate::ratelimit::RateLimiter>>,
        ) -> Fixture {
            let db = crate::db::open_memory().unwrap();
            let manager = crate::auth::create_key_manager().unwrap();

            let (user_id, bot_id, stranger_id, session) = {
                let conn = db.write().unwrap();
                // Start closed, so "widen the instance" is a visible change.
                crate::db::queries::settings::update(
                    &conn,
                    crate::db::queries::settings::InstanceSettingsPatch {
                        allow_signup: Some(false),
                        ..Default::default()
                    },
                )
                .unwrap();
                let user = crate::db::queries::users::create_user(
                    &conn,
                    &crate::db::models::CreateUser {
                        username: "owner".into(),
                        email: "owner@test.local".into(),
                        password: PASSWORD.into(),
                        display_name: None,
                        is_admin: true,
                        is_bot: false,
                    },
                )
                .unwrap();
                let bot = crate::db::queries::users::create_bot_user(
                    &conn,
                    user.id,
                    "opencode-owner",
                    "OpenCode",
                    Some("opencode"),
                )
                .unwrap();
                let stranger = crate::db::queries::users::create_user(
                    &conn,
                    &crate::db::models::CreateUser {
                        username: "stranger".into(),
                        email: "stranger@test.local".into(),
                        password: PASSWORD.into(),
                        display_name: None,
                        is_admin: false,
                        is_bot: false,
                    },
                )
                .unwrap();
                let session =
                    crate::db::queries::users::create_session(&conn, user.id, None).unwrap();
                (user.id, bot.id, stranger.id, session.token)
            };

            let key = |name: &str, owner: Option<i64>| {
                crate::auth::create_api_key(&db, &manager, name, owner).unwrap()
            };

            Fixture {
                app: real_stack_with_limiter(&db, limiter.clone()),
                limiter,
                human_key: key("human", Some(user_id)),
                bot_key: key("bot", Some(bot_id)),
                stranger_key: key("stranger", Some(stranger_id)),
                operator_key: key("operator", None),
                human_oauth: insert_oauth_token(&db, "human", user_id),
                bot_oauth: insert_oauth_token(&db, "bot", bot_id),
                db,
                user_id,
                bot_id,
                stranger_id,
                session,
            }
        }

        async fn status_with(app: &axum::Router, token: &str) -> StatusCode {
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/auth/me")
                        .header("authorization", format!("Bearer {token}"))
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
                .status()
        }

        async fn send(
            app: &axum::Router,
            method: &str,
            uri: &str,
            token: &str,
            body: Option<serde_json::Value>,
        ) -> (StatusCode, serde_json::Value) {
            let builder = Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json");
            let request = match &body {
                Some(value) => builder
                    .body(axum::body::Body::from(serde_json::to_vec(value).unwrap()))
                    .unwrap(),
                None => builder.body(axum::body::Body::empty()).unwrap(),
            };
            let response = app.clone().oneshot(request).await.unwrap();
            let status = response.status();
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            let parsed = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
            (status, parsed)
        }

        /// Every credential the lockdown is supposed to kill, and the two it
        /// must not touch, asserted through real requests.
        async fn assert_blast_radius(f: &Fixture, replacement: Option<&str>) {
            assert_eq!(
                status_with(&f.app, &f.session).await,
                StatusCode::UNAUTHORIZED,
                "the session that made the request is gone too"
            );
            assert_eq!(
                status_with(&f.app, &f.human_key).await,
                StatusCode::UNAUTHORIZED,
                "the human's API key is revoked"
            );
            assert_eq!(
                status_with(&f.app, &f.bot_key).await,
                StatusCode::UNAUTHORIZED,
                "an owned bot's API key is revoked"
            );
            assert_eq!(
                status_with(&f.app, &f.human_oauth).await,
                StatusCode::UNAUTHORIZED,
                "the human's OAuth token is revoked"
            );
            assert_eq!(
                status_with(&f.app, &f.bot_oauth).await,
                StatusCode::UNAUTHORIZED,
                "an owned bot's OAuth token is revoked"
            );

            assert_eq!(
                status_with(&f.app, &f.stranger_key).await,
                StatusCode::OK,
                "another account's key is untouched"
            );
            assert_eq!(
                status_with(&f.app, &f.operator_key).await,
                StatusCode::OK,
                "the unbound operator key names nobody and survives"
            );

            if let Some(token) = replacement {
                assert_eq!(
                    status_with(&f.app, token).await,
                    StatusCode::OK,
                    "the replacement session works on the next request"
                );
            }
        }

        #[tokio::test]
        async fn password_change_locks_down_everything_and_returns_one_replacement_session() {
            let f = fixture();
            let (status, body) = send(
                &f.app,
                "POST",
                "/api/auth/me/password",
                &f.session,
                Some(serde_json::json!({
                    "current_password": PASSWORD,
                    "new_password": "a whole new password",
                })),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let replacement = body["token"].as_str().unwrap().to_string();
            assert_ne!(replacement, f.session);

            assert_blast_radius(&f, Some(&replacement)).await;

            let sessions: i64 =
                f.db.read()
                    .unwrap()
                    .query_row(
                        "SELECT COUNT(*) FROM sessions WHERE user_id = ?1",
                        params![f.user_id],
                        |r| r.get(0),
                    )
                    .unwrap();
            assert_eq!(sessions, 1, "exactly one replacement session is minted");
        }

        /// A password change that races another one must not clobber it.
        ///
        /// The Argon2 verify and the new hash are both computed with no lock
        /// held, so an operator reset (or a second tab) can commit in between.
        /// The transaction compares the stored hash to the one that was
        /// verified, so the loser is told its "current password" is wrong,
        /// which is exactly what it now is.
        #[tokio::test]
        async fn a_password_change_that_lost_a_race_does_not_overwrite_the_winner() {
            let f = fixture();
            let captured = {
                let conn = f.db.read().unwrap();
                crate::db::queries::users::get_user_by_id(&conn, f.user_id)
                    .unwrap()
                    .password_hash
            };
            // The winner commits while the loser is hashing.
            {
                let conn = f.db.write().unwrap();
                crate::db::queries::users::update_password(&conn, f.user_id, "winner password")
                    .unwrap();
            }
            let winner_hash = {
                let conn = f.db.read().unwrap();
                crate::db::queries::users::get_user_by_id(&conn, f.user_id)
                    .unwrap()
                    .password_hash
            };

            // The loser arrives with the old password it verified.
            let (status, body) = send(
                &f.app,
                "POST",
                "/api/auth/me/password",
                &f.session,
                Some(serde_json::json!({
                    "current_password": PASSWORD,
                    "new_password": "loser password",
                })),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(body["error"], "current password is incorrect");

            let stored = {
                let conn = f.db.read().unwrap();
                crate::db::queries::users::get_user_by_id(&conn, f.user_id)
                    .unwrap()
                    .password_hash
            };
            assert_eq!(stored, winner_hash, "the winner's password survived");
            assert_ne!(stored, captured);
        }

        /// The new password's policy is applied before anything is written,
        /// and a rejected one leaves the account exactly as it was.
        #[tokio::test]
        async fn a_new_password_that_fails_policy_changes_nothing() {
            let f = fixture();
            for candidate in ["short", &"x".repeat(1025)] {
                let (status, _) = send(
                    &f.app,
                    "POST",
                    "/api/auth/me/password",
                    &f.session,
                    Some(serde_json::json!({
                        "current_password": PASSWORD,
                        "new_password": candidate,
                    })),
                )
                .await;
                assert_eq!(status, StatusCode::BAD_REQUEST, "{candidate:?}");
            }
            // The old password still works, and nothing was locked down.
            assert_eq!(status_with(&f.app, &f.session).await, StatusCode::OK);
            assert_eq!(status_with(&f.app, &f.human_key).await, StatusCode::OK);
        }

        /// The writer must stay usable while the password work is happening.
        /// Hashing under the writer is what this restructuring removed, so
        /// assert the lock is genuinely free at the moment Argon2 would run.
        #[tokio::test]
        #[allow(clippy::await_holding_lock)]
        async fn the_writer_is_not_held_while_passwords_are_hashed() {
            let f = fixture();
            let db = f.db.clone();
            // Held for the whole request. If the handler took the writer
            // around its hashing, this would deadlock rather than complete.
            let writer = db.write().unwrap();

            let outcome = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                send(
                    &f.app,
                    "POST",
                    "/api/auth/me/password",
                    &f.session,
                    Some(serde_json::json!({
                        "current_password": "definitely wrong",
                        "new_password": "a whole new password",
                    })),
                ),
            )
            .await;
            drop(writer);

            let (status, body) = outcome.expect("a wrong password must answer without the writer");
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(body["error"], "current password is incorrect");
        }

        #[tokio::test]
        async fn a_wrong_current_password_changes_nothing() {
            let f = fixture();
            let (status, _) = send(
                &f.app,
                "POST",
                "/api/auth/me/password",
                &f.session,
                Some(serde_json::json!({
                    "current_password": "not it",
                    "new_password": "a whole new password",
                })),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);

            assert_eq!(status_with(&f.app, &f.session).await, StatusCode::OK);
            assert_eq!(status_with(&f.app, &f.human_key).await, StatusCode::OK);
            assert_eq!(status_with(&f.app, &f.bot_key).await, StatusCode::OK);
        }

        #[tokio::test]
        async fn sign_out_everywhere_locks_down_everything_with_no_replacement() {
            let f = fixture();
            let (status, _) =
                send(&f.app, "DELETE", "/api/auth/me/sessions", &f.session, None).await;
            assert_eq!(status, StatusCode::OK);

            assert_blast_radius(&f, None).await;

            let sessions: i64 =
                f.db.read()
                    .unwrap()
                    .query_row(
                        "SELECT COUNT(*) FROM sessions WHERE user_id = ?1",
                        params![f.user_id],
                        |r| r.get(0),
                    )
                    .unwrap();
            assert_eq!(sessions, 0, "sign-out-everywhere mints nothing");
        }

        /// `lific user set-password` is the operator's recovery door. The CLI
        /// entrypoint itself is driven end to end by
        /// `cli::user::tests::set_password_locks_the_account_down`; this
        /// mirrors the write it performs and then reads the consequences back
        /// through real authenticated requests, which is the half the CLI test
        /// has no router for.
        #[tokio::test]
        async fn operator_reset_carries_the_same_blast_radius() {
            let f = fixture();
            {
                let conn = f.db.write().unwrap();
                let user = crate::db::queries::users::get_user_by_username(&conn, "owner").unwrap();
                crate::db::queries::savepoint(&conn, "cli_set_password", || {
                    crate::db::queries::users::update_password(
                        &conn,
                        user.id,
                        "reset by operator",
                    )?;
                    crate::db::queries::users::lock_down_account(&conn, user.id)
                })
                .unwrap();
            }

            assert_blast_radius(&f, None).await;
            let conn = f.db.read().unwrap();
            let user = crate::db::queries::users::get_user_by_username(&conn, "owner").unwrap();
            assert!(
                crate::db::queries::users::verify_password(
                    "reset by operator",
                    &user.password_hash
                )
                .unwrap()
            );
        }

        // ── Credential creation requires a recent browser session ────

        #[tokio::test]
        async fn a_recent_session_may_mint_a_key_and_connect_a_tool() {
            let f = fixture();
            let (status, body) = send(
                &f.app,
                "POST",
                "/api/auth/keys",
                &f.session,
                Some(serde_json::json!({"name": "laptop"})),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert!(body["key"].as_str().unwrap().starts_with("lific_sk"));
            assert_eq!(
                status_with(&f.app, body["key"].as_str().unwrap()).await,
                StatusCode::OK
            );

            let (status, body) = send(
                &f.app,
                "POST",
                "/api/auth/bots",
                &f.session,
                Some(serde_json::json!({"tool": "zed"})),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert!(body["key"].as_str().unwrap().starts_with("lific_sk"));
        }

        #[tokio::test]
        async fn an_aged_session_may_not_mint_credentials() {
            let f = fixture();
            f.db.write()
                .unwrap()
                .execute(
                    "UPDATE sessions SET created_at = datetime('now', '-16 minutes')",
                    [],
                )
                .unwrap();

            for (uri, body) in [
                ("/api/auth/keys", serde_json::json!({"name": "laptop"})),
                ("/api/auth/bots", serde_json::json!({"tool": "zed"})),
            ] {
                let (status, _) = send(&f.app, "POST", uri, &f.session, Some(body)).await;
                assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
            }
            // The session still authenticates ordinary requests; only the
            // credential-minting routes require freshness.
            assert_eq!(status_with(&f.app, &f.session).await, StatusCode::OK);
        }

        #[tokio::test]
        async fn an_api_key_may_no_longer_mint_credentials() {
            let f = fixture();
            // The key authenticates fine, so this is a policy refusal, not an
            // authentication failure.
            assert_eq!(status_with(&f.app, &f.human_key).await, StatusCode::OK);

            for (uri, body) in [
                (
                    "/api/auth/keys",
                    serde_json::json!({"name": "minted-by-key"}),
                ),
                ("/api/auth/bots", serde_json::json!({"tool": "zed"})),
            ] {
                let (status, _) = send(&f.app, "POST", uri, &f.human_key, Some(body)).await;
                assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
            }

            let minted: i64 =
                f.db.read()
                    .unwrap()
                    .query_row(
                        "SELECT COUNT(*) FROM api_keys WHERE name = 'minted-by-key'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
            assert_eq!(minted, 0, "a refused mint writes nothing");
        }

        #[tokio::test]
        async fn an_oauth_token_may_not_mint_credentials() {
            let f = fixture();
            for (uri, body) in [
                (
                    "/api/auth/keys",
                    serde_json::json!({"name": "minted-by-oauth"}),
                ),
                ("/api/auth/bots", serde_json::json!({"tool": "zed"})),
            ] {
                let (status, _) = send(&f.app, "POST", uri, &f.human_oauth, Some(body)).await;
                assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
            }
        }

        #[tokio::test]
        async fn a_revoked_session_may_not_mint_credentials() {
            let f = fixture();
            f.db.write()
                .unwrap()
                .execute("DELETE FROM sessions", [])
                .unwrap();

            let (status, _) = send(
                &f.app,
                "POST",
                "/api/auth/keys",
                &f.session,
                Some(serde_json::json!({"name": "ghost"})),
            )
            .await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }

        /// The linearizability claim, exercised at the transaction seam rather
        /// than by racing threads: whichever order SQLite's writer lock picks,
        /// no live key survives a lockdown.
        ///
        /// Creation-then-lockdown is the order that actually needs proving.
        /// The reverse (lockdown first) is covered by
        /// `a_revoked_session_may_not_mint_credentials`, where the mint finds
        /// no session at all.
        #[tokio::test]
        async fn a_key_minted_immediately_before_a_lockdown_does_not_survive_it() {
            let f = fixture();
            let (status, body) = send(
                &f.app,
                "POST",
                "/api/auth/keys",
                &f.session,
                Some(serde_json::json!({"name": "racing"})),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let racing_key = body["key"].as_str().unwrap().to_string();
            assert_eq!(status_with(&f.app, &racing_key).await, StatusCode::OK);

            {
                let conn = f.db.write().unwrap();
                crate::db::queries::users::lock_down_account(&conn, f.user_id).unwrap();
            }

            assert_eq!(
                status_with(&f.app, &racing_key).await,
                StatusCode::UNAUTHORIZED,
                "a key created just before the lockdown is inside its blast radius"
            );
        }

        /// The whole point of surviving a lockdown: getting your tools back.
        ///
        /// The lockdown revokes the bot's key but leaves the row, and
        /// `api_keys.name` is globally UNIQUE, so reconnecting reuses the name
        /// `opencode-owner`. Before `PreparedApiKey::insert` swept revoked
        /// rows this hit the UNIQUE constraint and the account could not
        /// reconnect anything without shell access.
        #[tokio::test]
        async fn a_tool_can_be_reconnected_after_a_lockdown() {
            let f = fixture();
            let (status, body) = send(
                &f.app,
                "POST",
                "/api/auth/me/password",
                &f.session,
                Some(serde_json::json!({
                    "current_password": PASSWORD,
                    "new_password": "a whole new password",
                })),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let replacement = body["token"].as_str().unwrap().to_string();

            // Reconnect with the fresh replacement session, which is recent by
            // construction.
            let (status, body) = send(
                &f.app,
                "POST",
                "/api/auth/bots",
                &replacement,
                Some(serde_json::json!({"tool": "opencode"})),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(
                body["bot"]["id"].as_i64(),
                Some(f.bot_id),
                "the same bot identity is reused, not a duplicate"
            );

            let new_key = body["key"].as_str().unwrap();
            assert_eq!(status_with(&f.app, new_key).await, StatusCode::OK);
            assert_eq!(
                status_with(&f.app, &f.bot_key).await,
                StatusCode::UNAUTHORIZED,
                "the pre-lockdown key stays dead"
            );

            let conn = f.db.read().unwrap();
            let live: Vec<(String, Option<i64>)> = conn
                .prepare("SELECT name, user_id FROM api_keys WHERE revoked = 0 AND user_id = ?1")
                .unwrap()
                .query_map(params![f.bot_id], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            assert_eq!(live, vec![("opencode-owner".to_string(), Some(f.bot_id))]);
        }

        // ── Admin persistence: creating and promoting accounts ───
        //
        // A lockdown revokes one account's credentials. An attacker who can
        // still mint a *second admin account* walks straight out of it: the
        // new account is a different user, so the victim's password change
        // never touches it. Creating and promoting therefore sit behind the
        // same recent-session rule as key and bot minting.

        /// Every access-*expanding* admin action, attempted with one
        /// credential: create an account, grant admin, restore a deactivated
        /// account, and widen the instance settings.
        struct Expansions {
            create: StatusCode,
            promote: StatusCode,
            reactivate: StatusCode,
            settings: StatusCode,
        }

        async fn roster_attempt(f: &Fixture, token: &str) -> Expansions {
            let (create, _) = send(
                &f.app,
                "POST",
                "/api/users",
                token,
                Some(serde_json::json!({
                    "username": "smuggled-admin",
                    "password": "securepass123",
                    "is_admin": true,
                })),
            )
            .await;
            let (promote, _) = send(
                &f.app,
                "POST",
                &format!("/api/users/{}/promote", f.stranger_id),
                token,
                Some(serde_json::json!({})),
            )
            .await;
            // Deactivate first (ungated) so reactivate has something to do.
            let _ = send(
                &f.app,
                "POST",
                &format!("/api/users/{}/deactivate", f.stranger_id),
                &f.operator_key,
                Some(serde_json::json!({})),
            )
            .await;
            let (reactivate, _) = send(
                &f.app,
                "POST",
                &format!("/api/users/{}/reactivate", f.stranger_id),
                token,
                Some(serde_json::json!({})),
            )
            .await;
            let (settings, _) = send(
                &f.app,
                "PATCH",
                "/api/instance/settings",
                token,
                Some(serde_json::json!({ "allow_signup": true })),
            )
            .await;
            Expansions {
                create,
                promote,
                reactivate,
                settings,
            }
        }

        fn signup_is_open(f: &Fixture) -> bool {
            crate::db::queries::settings::get(&f.db.read().unwrap())
                .unwrap()
                .allow_signup
        }

        fn is_active(f: &Fixture, id: i64) -> bool {
            crate::db::queries::users::get_user_by_id(&f.db.read().unwrap(), id)
                .unwrap()
                .is_active
        }

        /// Assert that none of the expansions were allowed and none of them
        /// left a trace.
        fn assert_nothing_expanded(f: &Fixture, attempt: &Expansions, expected: StatusCode) {
            assert_eq!(attempt.create, expected, "create");
            assert_eq!(attempt.promote, expected, "promote");
            assert_eq!(attempt.reactivate, expected, "reactivate");
            assert_eq!(attempt.settings, expected, "instance settings");
            assert!(!smuggled_exists(f), "no account was created");
            assert!(!is_admin(f, f.stranger_id), "no admin flag was written");
            assert!(!is_active(f, f.stranger_id), "the account stayed off");
            assert!(!signup_is_open(f), "settings were not widened");
        }

        fn smuggled_exists(f: &Fixture) -> bool {
            crate::db::queries::users::get_user_by_username(&f.db.read().unwrap(), "smuggled-admin")
                .is_ok()
        }

        fn is_admin(f: &Fixture, id: i64) -> bool {
            crate::db::queries::users::get_user_by_id(&f.db.read().unwrap(), id)
                .unwrap()
                .is_admin
        }

        #[tokio::test]
        async fn a_recent_session_may_perform_every_expansion() {
            let f = fixture();
            let attempt = roster_attempt(&f, &f.session).await;
            assert_eq!(attempt.create, StatusCode::OK);
            assert_eq!(attempt.promote, StatusCode::OK);
            assert_eq!(attempt.reactivate, StatusCode::OK);
            assert_eq!(attempt.settings, StatusCode::OK);
            assert!(smuggled_exists(&f));
            assert!(is_admin(&f, f.stranger_id));
            assert!(is_active(&f, f.stranger_id));
            assert!(signup_is_open(&f));
        }

        #[tokio::test]
        async fn an_aged_session_may_perform_no_expansion() {
            let f = fixture();
            f.db.write()
                .unwrap()
                .execute(
                    "UPDATE sessions SET created_at = datetime('now', '-16 minutes')",
                    [],
                )
                .unwrap();

            let attempt = roster_attempt(&f, &f.session).await;
            assert_nothing_expanded(&f, &attempt, StatusCode::FORBIDDEN);
        }

        /// The owner's account is an admin in the fixture, so this key
        /// authenticates as an admin. That is precisely the credential the
        /// rule is aimed at.
        #[tokio::test]
        async fn an_admin_api_key_may_perform_no_expansion() {
            let f = fixture();
            assert_eq!(status_with(&f.app, &f.human_key).await, StatusCode::OK);

            let attempt = roster_attempt(&f, &f.human_key).await;
            assert_nothing_expanded(&f, &attempt, StatusCode::FORBIDDEN);
        }

        #[tokio::test]
        async fn an_oauth_token_may_perform_no_expansion() {
            let f = fixture();
            let attempt = roster_attempt(&f, &f.human_oauth).await;
            assert_nothing_expanded(&f, &attempt, StatusCode::FORBIDDEN);
        }

        #[tokio::test]
        async fn a_session_revoked_by_a_lockdown_may_perform_no_expansion() {
            let f = fixture();
            {
                let conn = f.db.write().unwrap();
                crate::db::queries::users::lock_down_account(&conn, f.user_id).unwrap();
            }
            let attempt = roster_attempt(&f, &f.session).await;
            assert_nothing_expanded(&f, &attempt, StatusCode::UNAUTHORIZED);
        }

        /// The reducing half is deliberately NOT gated: those are what an
        /// admin reaches for while containing a compromise, and none of them
        /// can be used to persist access.
        #[tokio::test]
        async fn reducing_roster_actions_do_not_require_recent_authentication() {
            let f = fixture();
            f.db.write()
                .unwrap()
                .execute(
                    "UPDATE sessions SET created_at = datetime('now', '-16 minutes')",
                    [],
                )
                .unwrap();

            for action in ["demote", "deactivate"] {
                let (status, body) = send(
                    &f.app,
                    "POST",
                    &format!("/api/users/{}/{action}", f.stranger_id),
                    &f.session,
                    Some(serde_json::json!({})),
                )
                .await;
                assert_eq!(status, StatusCode::OK, "{action}: {body}");
            }
        }

        // ── Same-user session refresh ────────────────────────────
        //
        // The endpoint the recent-authentication rule is satisfied through.
        // Its whole contract is "same user, newer session", so most of what
        // matters here is what it refuses.

        async fn refresh(
            f: &Fixture,
            token: &str,
            password: Option<&str>,
        ) -> (StatusCode, serde_json::Value) {
            send(
                &f.app,
                "POST",
                "/api/auth/me/refresh",
                token,
                Some(match password {
                    Some(p) => serde_json::json!({ "password": p }),
                    None => serde_json::json!({}),
                }),
            )
            .await
        }

        fn session_count(f: &Fixture, user_id: i64) -> i64 {
            f.db.read()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM sessions WHERE user_id = ?1",
                    params![user_id],
                    |r| r.get(0),
                )
                .unwrap()
        }

        #[tokio::test]
        async fn a_correct_password_swaps_this_session_for_a_fresh_one() {
            let f = fixture();
            // A second session for the same account, to prove only one dies.
            let other = {
                let conn = f.db.write().unwrap();
                crate::db::queries::users::create_session(&conn, f.user_id, None)
                    .unwrap()
                    .token
            };
            f.db.write()
                .unwrap()
                .execute(
                    "UPDATE sessions SET created_at = datetime('now', '-16 minutes')",
                    [],
                )
                .unwrap();

            let (status, body) = refresh(&f, &f.session, Some(PASSWORD)).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(
                body["user"]["id"].as_i64(),
                Some(f.user_id),
                "the refreshed session is for the same account"
            );
            let replacement = body["token"].as_str().unwrap().to_string();
            assert_ne!(replacement, f.session);

            assert_eq!(
                status_with(&f.app, &f.session).await,
                StatusCode::UNAUTHORIZED,
                "the presented session was consumed"
            );
            assert_eq!(
                status_with(&f.app, &other).await,
                StatusCode::OK,
                "every other session survives"
            );

            // And it is recent, so it can do what the old one could not.
            let (status, _) = send(
                &f.app,
                "POST",
                "/api/auth/keys",
                &replacement,
                Some(serde_json::json!({"name": "after-refresh"})),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
        }

        #[tokio::test]
        async fn a_wrong_password_leaves_the_old_session_working() {
            let f = fixture();
            let before = session_count(&f, f.user_id);

            let (status, _) = refresh(&f, &f.session, Some("not the password")).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(status_with(&f.app, &f.session).await, StatusCode::OK);
            assert_eq!(session_count(&f, f.user_id), before, "nothing was minted");
        }

        #[tokio::test]
        async fn a_password_change_between_verify_and_finalize_is_refused() {
            let f = fixture();
            // Stand in for the race: the stored hash is no longer the one the
            // verify would have matched. The transaction compares them.
            let stale_hash = {
                let conn = f.db.read().unwrap();
                crate::db::queries::users::get_user_by_id(&conn, f.user_id)
                    .unwrap()
                    .password_hash
            };
            {
                let conn = f.db.write().unwrap();
                crate::db::queries::users::update_password(&conn, f.user_id, "a whole new one")
                    .unwrap();
                let refreshed =
                    crate::db::queries::users::get_user_by_id(&conn, f.user_id).unwrap();
                assert_ne!(refreshed.password_hash, stale_hash);
                assert!(
                    crate::db::queries::users::finalize_login(&conn, f.user_id, &stale_hash)
                        .is_err(),
                    "a session must not be minted against a superseded hash"
                );
            }
            // The old password no longer refreshes either.
            let (status, _) = refresh(&f, &f.session, Some(PASSWORD)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(session_count(&f, f.user_id), 1, "no replacement was minted");
        }

        #[tokio::test]
        async fn a_password_is_required_when_the_instance_uses_passwords() {
            let f = fixture();
            let (status, body) = refresh(&f, &f.session, None).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert!(
                body["error"]
                    .as_str()
                    .unwrap()
                    .contains("password is required"),
                "{body}"
            );
        }

        #[tokio::test]
        async fn a_passwordless_instance_refreshes_the_caller_not_the_first_admin() {
            let f = fixture();
            // A second admin, older than the owner, so "first admin" and
            // "the caller" are different accounts.
            let (second_id, second_session) = {
                let conn = f.db.write().unwrap();
                crate::db::queries::settings::update(
                    &conn,
                    crate::db::queries::settings::InstanceSettingsPatch {
                        web_auto_login: Some(true),
                        ..Default::default()
                    },
                )
                .unwrap();
                let second = crate::db::queries::users::create_user(
                    &conn,
                    &crate::db::models::CreateUser {
                        username: "second-admin".into(),
                        email: "second@test.local".into(),
                        password: PASSWORD.into(),
                        display_name: None,
                        is_admin: true,
                        is_bot: false,
                    },
                )
                .unwrap();
                let session =
                    crate::db::queries::users::create_session(&conn, second.id, None).unwrap();
                (second.id, session.token)
            };
            assert_ne!(second_id, f.user_id);

            let (status, body) = refresh(&f, &second_session, None).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(
                body["user"]["id"].as_i64(),
                Some(second_id),
                "passwordless refresh must never hand back the first admin"
            );
        }

        #[tokio::test]
        async fn turning_passwordless_off_before_the_write_refuses_the_refresh() {
            let f = fixture();
            // `[auth] required` is true in this fixture and `web_auto_login`
            // is off, which is exactly the "mode is not enabled" state the
            // transaction re-reads.
            let (status, _) = refresh(&f, &f.session, None).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(session_count(&f, f.user_id), 1);
        }

        #[tokio::test]
        async fn api_keys_and_oauth_tokens_may_not_refresh_a_session() {
            let f = fixture();
            for token in [&f.human_key, &f.human_oauth] {
                let (status, _) = refresh(&f, token, Some(PASSWORD)).await;
                assert_eq!(status, StatusCode::FORBIDDEN);
            }
            assert_eq!(session_count(&f, f.user_id), 1, "nothing was minted");
        }

        #[tokio::test]
        async fn a_revoked_session_may_not_refresh_itself() {
            let f = fixture();
            {
                let conn = f.db.write().unwrap();
                crate::db::queries::users::lock_down_account(&conn, f.user_id).unwrap();
            }
            let (status, _) = refresh(&f, &f.session, Some(PASSWORD)).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(session_count(&f, f.user_id), 0);
        }

        // ── Password change is the most expensive request here ───

        async fn change_password_attempt(
            f: &Fixture,
            token: &str,
            current: &str,
            new: &str,
        ) -> (StatusCode, serde_json::Value) {
            send(
                &f.app,
                "POST",
                "/api/auth/me/password",
                token,
                Some(serde_json::json!({
                    "current_password": current,
                    "new_password": new,
                })),
            )
            .await
        }

        #[tokio::test]
        async fn repeated_wrong_current_passwords_exhaust_the_change_budget() {
            let f = fixture_with_limiter(2);
            for attempt in 0..2 {
                let (status, body) =
                    change_password_attempt(&f, &f.session, "wrong", "a whole new password").await;
                assert_eq!(status, StatusCode::BAD_REQUEST, "attempt {attempt}");
                assert_eq!(body["error"], "current password is incorrect");
            }

            let (status, body) =
                change_password_attempt(&f, &f.session, "wrong", "a whole new password").await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert!(
                body["error"]
                    .as_str()
                    .unwrap()
                    .contains("too many password change attempts"),
                "{body}"
            );
            // Even the correct password is refused: the limit is on reaching
            // the hashing at all.
            let (_, body) =
                change_password_attempt(&f, &f.session, PASSWORD, "a whole new password").await;
            assert!(
                body["error"]
                    .as_str()
                    .unwrap()
                    .contains("too many password change attempts"),
                "{body}"
            );
        }

        /// A rejected *new* password is still a spent attempt: the verify has
        /// already run by then, so it cost the same CPU an attacker's would.
        #[tokio::test]
        async fn a_rejected_new_password_still_spends_its_reservation() {
            let f = fixture_with_limiter(1);
            let limiter = f.limiter.clone().expect("limiter");

            let (status, _) = change_password_attempt(&f, &f.session, PASSWORD, "short").await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert!(
                limiter.contains_key(&format!("password_change_user:{}", f.user_id)),
                "the attempt was spent"
            );
        }

        /// A change that lands refunds what it took, so someone rotating their
        /// password a few times in a row does not lock themselves out. Each
        /// change consumes the presented session, so the replacement token is
        /// carried forward.
        #[tokio::test]
        async fn successful_password_changes_refund_their_reservations() {
            let f = fixture_with_limiter(1);
            let limiter = f.limiter.clone().expect("limiter");

            let mut token = f.session.clone();
            let mut current = PASSWORD.to_string();
            for round in 0..3 {
                let next = format!("password number {round}");
                let (status, body) = change_password_attempt(&f, &token, &current, &next).await;
                assert_eq!(status, StatusCode::OK, "round {round}: {body}");
                token = body["token"].as_str().unwrap().to_string();
                current = next;
            }
            assert!(
                !limiter.contains_key(&format!("password_change_user:{}", f.user_id)),
                "a clean run of password changes leaves no budget spent"
            );
        }

        /// The bound on concurrent hashing: slots reserved by in-flight
        /// requests refuse the next one before it reaches Argon2.
        #[tokio::test]
        async fn reservations_bound_how_many_password_changes_are_admitted() {
            let f = fixture_with_limiter(2);
            let limiter = f.limiter.clone().expect("limiter");
            let held: Vec<_> = (0..2)
                .map(|i| {
                    crate::ratelimit::Reservation::acquire(
                        &limiter,
                        &format!("password_change_ip:10.0.0.{i}"),
                        &format!("password_change_user:{}", f.user_id),
                    )
                    .expect("within budget")
                })
                .collect();

            let (status, body) =
                change_password_attempt(&f, &f.session, PASSWORD, "a whole new password").await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert!(
                body["error"]
                    .as_str()
                    .unwrap()
                    .contains("too many password change attempts"),
                "{body}"
            );
            assert!(
                body["error"].as_str().unwrap().contains("try again in"),
                "the refusal keeps its retry guidance: {body}"
            );

            // The password was not changed by the refused attempt.
            held.into_iter().next().unwrap().refund();
            let (status, _) =
                change_password_attempt(&f, &f.session, PASSWORD, "a whole new password").await;
            assert_eq!(status, StatusCode::OK, "the original password still works");
        }

        /// The refresh endpoint runs Argon2, so it is rate-limited on its own
        /// keys. Wrong passwords spend budget; a correct one refunds it.
        #[tokio::test]
        async fn repeated_wrong_passwords_exhaust_the_confirmation_budget() {
            let f = fixture_with_limiter(2);
            for attempt in 0..2 {
                let (status, body) = refresh(&f, &f.session, Some("wrong")).await;
                assert_eq!(status, StatusCode::BAD_REQUEST);
                assert_eq!(body["error"], "incorrect password", "attempt {attempt}");
            }

            let (status, body) = refresh(&f, &f.session, Some("wrong")).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert!(
                body["error"]
                    .as_str()
                    .unwrap()
                    .contains("too many confirmation attempts"),
                "{body}"
            );
            // Even the right password is refused once the budget is gone: the
            // limit is on reaching the verify at all.
            let (_, body) = refresh(&f, &f.session, Some(PASSWORD)).await;
            assert!(
                body["error"]
                    .as_str()
                    .unwrap()
                    .contains("too many confirmation attempts"),
                "{body}"
            );
        }

        #[tokio::test]
        async fn a_successful_confirmation_refunds_its_reservation() {
            let f = fixture_with_limiter(2);
            let limiter = f.limiter.clone().expect("limiter");

            // Several correct confirmations through a budget of two. Each one
            // consumes the session it presented, so chain the replacements.
            let mut token = f.session.clone();
            for attempt in 0..4 {
                let (status, body) = refresh(&f, &token, Some(PASSWORD)).await;
                assert_eq!(status, StatusCode::OK, "attempt {attempt}: {body}");
                token = body["token"].as_str().unwrap().to_string();
            }
            assert!(
                !limiter.contains_key(&format!("reauth_user:{}", f.user_id)),
                "successful confirmations leave no budget spent"
            );
        }

        /// One account exhausting its confirmation budget must not affect
        /// another account, and must not touch the login budget either.
        #[tokio::test]
        async fn the_confirmation_budget_is_scoped_to_the_account() {
            let f = fixture_with_limiter(1);
            let limiter = f.limiter.clone().expect("limiter");

            let (_, body) = refresh(&f, &f.session, Some("wrong")).await;
            assert_eq!(body["error"], "incorrect password");
            assert!(limiter.contains_key(&format!("reauth_user:{}", f.user_id)));
            assert!(
                !limiter.contains_key(&format!("reauth_user:{}", f.stranger_id)),
                "another account's budget is untouched"
            );
            assert!(
                !limiter.contains_key("login_id:owner"),
                "confirming must not drain the account's login budget"
            );
        }

        /// A passwordless refresh does no expensive work, so it takes no slot.
        #[tokio::test]
        async fn a_passwordless_confirmation_needs_no_reservation() {
            let f = fixture_with_limiter(1);
            let limiter = f.limiter.clone().expect("limiter");
            f.db.write()
                .unwrap()
                .execute("UPDATE instance_settings SET web_auto_login = 1", [])
                .unwrap();

            let mut token = f.session.clone();
            for attempt in 0..3 {
                let (status, body) = refresh(&f, &token, None).await;
                assert_eq!(status, StatusCode::OK, "attempt {attempt}: {body}");
                token = body["token"].as_str().unwrap().to_string();
            }
            assert!(!limiter.contains_key(&format!("reauth_user:{}", f.user_id)));
        }

        // ── Fresh authorization inside the granting transaction ──

        /// A demotion that lands while the caller is mid-flight. Here the
        /// middleware itself re-reads the user, so this proves the outcome;
        /// the *stale snapshot* case is
        /// `roster::a_stale_admin_snapshot_cannot_expand_access`, which is the
        /// one that needs the in-transaction check.
        #[tokio::test]
        async fn an_admin_demoted_after_the_request_arrived_may_not_expand() {
            let f = fixture();
            {
                // Another admin exists, so the last-admin guard allows this.
                let conn = f.db.write().unwrap();
                crate::db::queries::users::create_user(
                    &conn,
                    &crate::db::models::CreateUser {
                        username: "other-admin".into(),
                        email: "other@test.local".into(),
                        password: PASSWORD.into(),
                        display_name: None,
                        is_admin: true,
                        is_bot: false,
                    },
                )
                .unwrap();
                crate::db::queries::users::set_admin_guarded(&conn, f.user_id, false).unwrap();
            }

            let attempt = roster_attempt(&f, &f.session).await;
            assert_eq!(attempt.create, StatusCode::FORBIDDEN);
            assert_eq!(attempt.promote, StatusCode::FORBIDDEN);
            assert_eq!(attempt.reactivate, StatusCode::FORBIDDEN);
            assert_eq!(attempt.settings, StatusCode::FORBIDDEN);
            assert!(!smuggled_exists(&f));
            assert!(!signup_is_open(&f));
        }

        #[tokio::test]
        async fn a_failed_tool_connect_leaves_no_half_created_identity() {
            let f = fixture();
            // The bot already holds a live key, so the connect is refused
            // after `ensure_bot` has already run inside the transaction.
            let before: i64 =
                f.db.read()
                    .unwrap()
                    .query_row("SELECT COUNT(*) FROM users WHERE is_bot = 1", [], |r| {
                        r.get(0)
                    })
                    .unwrap();

            let (status, _) = send(
                &f.app,
                "POST",
                "/api/auth/bots",
                &f.session,
                Some(serde_json::json!({"tool": "opencode"})),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);

            let conn = f.db.read().unwrap();
            let after: i64 = conn
                .query_row("SELECT COUNT(*) FROM users WHERE is_bot = 1", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(before, after);
            let keys: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM api_keys WHERE user_id = ?1 AND revoked = 0",
                    params![f.bot_id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(keys, 1, "no second key was minted for the connected bot");
            drop(conn);
            assert_eq!(
                status_with(&f.app, &f.bot_key).await,
                StatusCode::OK,
                "the refusal did not rotate the tool's live credential out from under it"
            );
        }
    }

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

    fn signup_app_with_limiter(
        max: usize,
        peer: std::net::SocketAddr,
    ) -> (axum::Router, std::sync::Arc<crate::ratelimit::RateLimiter>) {
        let db = crate::db::open_memory().expect("test db");
        let limiter = std::sync::Arc::new(crate::ratelimit::RateLimiter::new(
            max,
            std::time::Duration::from_secs(15 * 60),
        ));
        let app = with_client_ip_test_layers(crate::api::router(db, &[]), peer)
            .layer(axum::Extension(crate::config::AuthConfig {
                allow_signup: true,
                required: true,
                secure_cookies: false,
            }))
            .layer(axum::Extension(limiter.clone()));
        (app, limiter)
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

    /// A successful login must not spend the account's failure budget.
    ///
    /// Reserve-then-refund is what makes this true: the slots are taken before
    /// Argon2 (so concurrent verifies are bounded) and given back once a
    /// session exists (so signing in correctly, repeatedly, never locks you
    /// out of your own account).
    #[tokio::test]
    async fn a_successful_login_refunds_its_reservation() {
        let db = crate::db::open_memory().expect("test db");
        {
            let conn = db.write().unwrap();
            crate::db::queries::users::create_user(
                &conn,
                &CreateUser {
                    username: "blake".into(),
                    email: "blake@test.local".into(),
                    password: "correct horse battery".into(),
                    display_name: None,
                    is_admin: false,
                    is_bot: false,
                },
            )
            .unwrap();
        }
        let limiter = std::sync::Arc::new(crate::ratelimit::RateLimiter::new(
            3,
            std::time::Duration::from_secs(15 * 60),
        ));
        let app = with_client_ip_test_layers(crate::api::router(db, &[]), test_peer())
            .layer(axum::Extension(crate::config::AuthConfig {
                allow_signup: true,
                required: true,
                secure_cookies: false,
            }))
            .layer(axum::Extension(limiter.clone()));

        // Six correct logins through a budget of three.
        for attempt in 0..6 {
            let resp = json_post(
                &app,
                "/api/auth/login",
                serde_json::json!({
                    "identity": "blake",
                    "password": "correct horse battery",
                }),
            )
            .await;
            assert_eq!(resp.status(), StatusCode::OK, "attempt {attempt}");
        }
        assert!(
            !limiter.contains_key("login_id:blake"),
            "a clean run of successful logins leaves no failure budget spent"
        );

        // And a wrong password still costs exactly one.
        let (status, body) = login_attempt(&app, "blake", "10.0.0.1").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(!is_rate_limited(&body));
        assert!(limiter.contains_key("login_id:blake"));
    }

    /// The property peek-then-record could not provide. Every slot is
    /// reserved *before* any verify runs, so the request that arrives once the
    /// budget is gone is refused without being admitted to Argon2 at all.
    #[tokio::test]
    async fn reservations_bound_how_many_verifies_can_be_admitted() {
        let limiter = std::sync::Arc::new(crate::ratelimit::RateLimiter::new(
            3,
            std::time::Duration::from_secs(15 * 60),
        ));
        // Stand in for three requests that have reached the reservation but
        // not yet finished verifying: exactly the state peek-then-record
        // could not represent, because nothing was recorded until afterwards.
        let held: Vec<_> = (0..3)
            .map(|i| {
                crate::ratelimit::Reservation::acquire(
                    &limiter,
                    &format!("login_ip:10.0.0.{i}"),
                    "login_id:victim",
                )
                .expect("within budget")
            })
            .collect();

        let app = login_app_with_limiter_arc(limiter.clone(), test_peer());
        let (status, body) = login_attempt(&app, "victim", "10.0.0.99").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            is_rate_limited(&body),
            "the fourth concurrent attempt must be refused, not admitted: {body}"
        );
        assert!(
            body["error"].as_str().unwrap().contains("try again in"),
            "the refusal keeps its retry guidance: {body}"
        );

        // Releasing one in-flight attempt frees exactly one slot.
        held.into_iter().next().unwrap().refund();
        let (_, body) = login_attempt(&app, "victim", "10.0.0.98").await;
        assert!(!is_rate_limited(&body), "one slot came back: {body}");
    }

    fn login_app_with_limiter_arc(
        limiter: std::sync::Arc<crate::ratelimit::RateLimiter>,
        peer: std::net::SocketAddr,
    ) -> axum::Router {
        let db = crate::db::open_memory().expect("test db");
        with_client_ip_test_layers(crate::api::router(db, &[]), peer)
            .layer(axum::Extension(crate::config::AuthConfig {
                allow_signup: true,
                required: true,
                secure_cookies: false,
            }))
            .layer(axum::Extension(limiter))
    }

    #[tokio::test]
    async fn login_grants_full_per_identity_budget() {
        // Regression for the double-counting bug: with max 5, exactly 5
        // failed attempts must be allowed before the 6th is blocked. The
        // original code recorded twice per attempt and only allowed ~3.
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

    #[tokio::test]
    async fn blocked_signup_does_not_allocate_a_new_email_key() {
        let (app, limiter) = signup_app_with_limiter(1, test_peer());
        let (status, _) = signup_attempt(&app, 0, "203.0.113.10").await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = signup_attempt(&app, 1, "203.0.113.10").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(is_signup_rate_limited(&body));
        assert!(!limiter.contains_key("signup:user1@test.com"));
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

    /// A session for `user`, created just now, so it satisfies the
    /// recent-authentication window that `create` and `promote` require.
    fn fresh_session(db: &DbPool, user: &User) -> String {
        let conn = db.write().unwrap();
        crate::db::queries::users::create_session(&conn, user.id, None)
            .unwrap()
            .token
    }

    /// `json_post` with a bearer token, for the two roster mutations that
    /// require a recent browser session rather than merely an admin identity.
    async fn json_post_as(
        app: &axum::Router,
        uri: &str,
        body: serde_json::Value,
        token: &str,
    ) -> axum::response::Response {
        use tower::ServiceExt;
        app.clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    fn reload(db: &DbPool, id: i64) -> User {
        let conn = db.read().unwrap();
        crate::db::queries::users::get_user_by_id(&conn, id).unwrap()
    }

    /// The authorization the *transaction* trusts must be read inside it.
    ///
    /// `app_as_user` injects a fixed `AuthUser` the way the middleware
    /// attaches one, which is precisely a snapshot: it is taken before the
    /// handler runs and never revisited. This test hands the router an
    /// `is_admin: true` snapshot for an account that has since been demoted.
    /// Every preflight gate reads that snapshot and is satisfied; only the
    /// re-read inside the granting transaction can refuse, so if these come
    /// back 200 the fresh check is not there.
    #[tokio::test]
    async fn a_stale_admin_snapshot_cannot_expand_access() {
        let (db, admin, other, member, _bot) = roster_db();
        // The snapshot: taken while they were still an admin.
        let app = app_as_user(db.clone(), &admin);
        let session = fresh_session(&db, &admin);
        {
            let conn = db.write().unwrap();
            crate::db::queries::users::set_admin_guarded(&conn, admin.id, false).unwrap();
        }
        assert!(!reload(&db, admin.id).is_admin);

        let create = json_post_as(
            &app,
            "/api/users",
            serde_json::json!({ "username": "ghost", "password": "securepass123" }),
            &session,
        )
        .await;
        assert_eq!(create.status(), StatusCode::FORBIDDEN, "create");

        let promote = json_post_as(
            &app,
            &format!("/api/users/{}/promote", member.id),
            json!({}),
            &session,
        )
        .await;
        assert_eq!(promote.status(), StatusCode::FORBIDDEN, "promote");

        {
            let conn = db.write().unwrap();
            crate::db::queries::users::set_active(&conn, member.id, false).unwrap();
        }
        let reactivate = json_post_as(
            &app,
            &format!("/api/users/{}/reactivate", member.id),
            json!({}),
            &session,
        )
        .await;
        assert_eq!(reactivate.status(), StatusCode::FORBIDDEN, "reactivate");

        let settings = {
            use tower::ServiceExt;
            app.clone()
                .oneshot(
                    axum::http::Request::builder()
                        .method("PATCH")
                        .uri("/api/instance/settings")
                        .header("content-type", "application/json")
                        .header("authorization", format!("Bearer {session}"))
                        .body(axum::body::Body::from(
                            serde_json::to_vec(&serde_json::json!({"allow_signup": true})).unwrap(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap()
        };
        assert_eq!(
            settings.status(),
            StatusCode::FORBIDDEN,
            "instance settings"
        );

        // Nothing landed.
        assert!(
            crate::db::queries::users::get_user_by_username(&db.read().unwrap(), "ghost").is_err()
        );
        assert!(!reload(&db, member.id).is_admin);
        assert!(!reload(&db, member.id).is_active);
        let _ = other;
    }

    /// Destructive routes are ungated on recency, but they must not act on a
    /// stale `is_admin`. Reaching *another account's* key or connected tool,
    /// and demoting or deactivating anyone, all hinge on that flag.
    #[tokio::test]
    async fn a_stale_admin_snapshot_cannot_reach_another_accounts_credentials() {
        let (db, admin, other, member, _bot) = roster_db();
        let app = app_as_user(db.clone(), &admin);

        // The victim's key and connected tool.
        let manager = crate::auth::create_key_manager().unwrap();
        let victim_bot = {
            let conn = db.write().unwrap();
            crate::db::queries::users::ensure_bot(&conn, member.id, "zed", "Zed").unwrap()
        };
        crate::auth::create_api_key(&db, &manager, "victim-key", Some(member.id)).unwrap();
        crate::auth::create_api_key(&db, &manager, "victim-bot-key", Some(victim_bot.id)).unwrap();
        let victim_key_id: i64 = db
            .read()
            .unwrap()
            .query_row(
                "SELECT id FROM api_keys WHERE name = 'victim-key'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        {
            let conn = db.write().unwrap();
            crate::db::queries::users::set_admin_guarded(&conn, admin.id, false).unwrap();
        }

        let revoke = json_delete(&app, &format!("/api/auth/keys/{victim_key_id}")).await;
        assert_ne!(revoke.status(), StatusCode::OK, "revoke another's key");
        let disconnect = json_post(
            &app,
            &format!("/api/auth/bots/{}/disconnect", victim_bot.id),
            json!({}),
        )
        .await;
        assert_ne!(
            disconnect.status(),
            StatusCode::OK,
            "disconnect another's tool"
        );
        let delete = json_delete(&app, &format!("/api/auth/bots/{}", victim_bot.id)).await;
        assert_ne!(delete.status(), StatusCode::OK, "delete another's tool");

        let demote = json_post(&app, &format!("/api/users/{}/demote", other.id), json!({})).await;
        assert_eq!(demote.status(), StatusCode::FORBIDDEN, "demote");
        let deactivate = json_post(
            &app,
            &format!("/api/users/{}/deactivate", other.id),
            json!({}),
        )
        .await;
        assert_eq!(deactivate.status(), StatusCode::FORBIDDEN, "deactivate");

        // Nothing moved.
        let conn = db.read().unwrap();
        let live: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM api_keys WHERE revoked = 0 AND name LIKE 'victim%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(live, 2, "the victim's credentials are untouched");
        drop(conn);
        assert!(reload(&db, other.id).is_admin);
        assert!(reload(&db, other.id).is_active);
    }

    /// Deactivation has to reach live websockets immediately, for the account
    /// and for every tool it owns: `set_active` deletes all of their sessions,
    /// so all of those sockets are now holding dead credentials.
    #[tokio::test]
    async fn deactivating_an_account_broadcasts_a_revocation_for_it_and_its_bots() {
        let (db, admin, _other, member, _bot) = roster_db();
        let victim_bot = {
            let conn = db.write().unwrap();
            crate::db::queries::users::ensure_bot(&conn, member.id, "zed", "Zed").unwrap()
        };

        let realtime = crate::realtime::RealtimeHub::new();
        let mut revocations = realtime.subscribe_revocations();
        let app = app_as_user_with_realtime(db.clone(), &admin, realtime);

        let resp = json_post(
            &app,
            &format!("/api/users/{}/deactivate", member.id),
            json!({}),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let mut told = Vec::new();
        while let Ok(id) = revocations.try_recv() {
            told.push(id);
        }
        told.sort_unstable();
        let mut expected = vec![member.id, victim_bot.id];
        expected.sort_unstable();
        assert_eq!(
            told, expected,
            "the account and every bot it owns are told at once"
        );
    }

    #[tokio::test]
    async fn admin_can_create_a_user_from_the_roster() {
        let (db, admin, _other, _member, _bot) = roster_db();
        let app = app_as_user(db.clone(), &admin);
        let session = fresh_session(&db, &admin);

        let resp = json_post_as(
            &app,
            "/api/users",
            serde_json::json!({ "username": "newcomer", "password": "securepass123" }),
            &session,
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
        let session = fresh_session(&db, &admin);

        let resp = json_post_as(
            &app,
            &format!("/api/users/{}/promote", member.id),
            json!({}),
            &session,
        )
        .await;
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
        let status = resp.status();
        let body = parse_json(resp).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["is_active"], true);
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
        let session = fresh_session(&db, &admin);

        for action in ["promote", "demote", "deactivate", "reactivate"] {
            let resp = json_post_as(
                &app,
                &format!("/api/users/{}/{action}", bot.id),
                json!({}),
                &session,
            )
            .await;
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
