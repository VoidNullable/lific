use axum::{
    Extension,
    extract::{Json, Path, State},
    http::HeaderMap,
    response::IntoResponse,
};

use crate::db::{DbPool, models::*};
use crate::error::LificError;

use super::{with_read, with_write};

/// Build a Set-Cookie header for the session token with security flags.
fn session_cookie(token: &str, expires_at: &str) -> String {
    use chrono::DateTime;
    // Parse expiry for Max-Age calculation; fall back to 30 days
    let max_age = DateTime::parse_from_rfc3339(expires_at)
        .map(|exp| {
            let exp_utc: DateTime<chrono::Utc> = exp.into();
            (exp_utc - chrono::Utc::now()).num_seconds().max(0)
        })
        .unwrap_or(30 * 24 * 3600);

    format!(
        "lific_token={token}; Path=/; Max-Age={max_age}; HttpOnly; Secure; SameSite=Lax"
    )
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
    Json(input): Json<SignupRequest>,
) -> Result<impl IntoResponse, LificError> {
    if !auth_cfg.allow_signup {
        return Err(LificError::BadRequest(
            "signup is disabled — contact an admin to create your account".into(),
        ));
    }

    // Rate limit signups to prevent Argon2 CPU exhaustion
    let key = format!("signup:{}", input.email.to_lowercase());
    if let Some(Extension(ref rl)) = limiter
        && !rl.check(&key)
    {
        let retry = rl.retry_after(&key);
        return Err(LificError::BadRequest(format!(
            "too many signup attempts — try again in {retry} seconds"
        )));
    }

    let conn = db.write()?;
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
    let session = crate::db::queries::users::create_session(&conn, user.id, None)?;

    let mut headers = HeaderMap::new();
    headers.insert(
        "set-cookie",
        session_cookie(&session.token, &session.expires_at)
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
    let session = crate::db::queries::users::create_session(&conn, user.id, None)?;

    let mut headers = HeaderMap::new();
    headers.insert(
        "set-cookie",
        session_cookie(&session.token, &session.expires_at)
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

pub(super) async fn auth_logout(
    State(db): State<DbPool>,
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
        "lific_token=; Path=/; Max-Age=0; HttpOnly; Secure; SameSite=Lax"
            .parse()
            .unwrap(),
    );

    Ok((resp_headers, Json(serde_json::json!({"logged_out": true}))))
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
pub(super) async fn change_password(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
    Json(input): Json<ChangePasswordRequest>,
) -> Result<Json<serde_json::Value>, LificError> {
    let user = auth_user.ok_or_else(|| LificError::BadRequest("authentication required".into()))?;
    with_write(&db, |conn| {
        let full = crate::db::queries::users::get_user_by_id(conn, user.id)?;
        let ok = crate::db::queries::users::verify_password(
            &input.current_password,
            &full.password_hash,
        )?;
        if !ok {
            return Err(LificError::BadRequest("current password is incorrect".into()));
        }
        crate::db::queries::users::update_password(conn, user.id, &input.new_password)
    })?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// DELETE /api/auth/me/sessions — sign out of every session (this one too).
/// Clears the cookie so the current browser drops to logged-out. LIF-190.
pub(super) async fn revoke_all_sessions(
    State(db): State<DbPool>,
    Extension(auth_user): Extension<Option<AuthUser>>,
) -> Result<impl IntoResponse, LificError> {
    let user = auth_user.ok_or_else(|| LificError::BadRequest("authentication required".into()))?;
    with_write(&db, |conn| {
        crate::db::queries::users::delete_all_sessions(conn, user.id)
    })?;
    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        "set-cookie",
        "lific_token=; Path=/; Max-Age=0; HttpOnly; Secure; SameSite=Lax"
            .parse()
            .unwrap(),
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
    /// Tool identifier (e.g. "opencode", "cursor", "claude", "codex")
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
        let db = crate::db::open_memory().expect("test db");
        let app = crate::api::router(db, &[]).layer(axum::Extension(crate::config::AuthConfig {
            allow_signup: false,
        }));

        let body = serde_json::json!({
            "username": "blocked",
            "email": "blocked@test.com",
            "password": "securepass123"
        });
        let resp = json_post(&app, "/api/auth/signup", body).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let data = parse_json(resp).await;
        assert!(data["error"].as_str().unwrap().contains("disabled"));
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

        let wrong =
            serde_json::json!({ "current_password": "totally-wrong", "new_password": "newpassword123" });
        let resp = json_post(&app, "/api/auth/me/password", wrong).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let right =
            serde_json::json!({ "current_password": "originalpass123", "new_password": "newpassword123" });
        let resp = json_post(&app, "/api/auth/me/password", right).await;
        assert_eq!(resp.status(), StatusCode::OK);
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
        assert!(is_rate_limited(&attacker), "attacker IP should be capped: {attacker}");
        // A different IP is unaffected.
        let (_, other) = login_attempt(&app, "someone", "198.51.100.2").await;
        assert!(!is_rate_limited(&other), "distinct IP should not be limited: {other}");
    }
}
