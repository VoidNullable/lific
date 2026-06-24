use std::sync::Arc;

use axum::{
    Router,
    extract::{Json, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use hmac::{Hmac, Mac};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::db::DbPool;
use crate::ratelimit::RateLimiter;

type HmacSha256 = Hmac<Sha256>;

/// Per-process CSRF secret, generated randomly on startup.
static CSRF_SECRET: std::sync::LazyLock<[u8; 32]> =
    std::sync::LazyLock::new(rand::random);

/// Generate a CSRF token bound to the approving session: `timestamp.hmac(ts || binding)`.
///
/// SECURITY: the token MUST be bound to the credential (`binding`) the request
/// carries. The authorize page is served unauthenticated (`GET /oauth/authorize`),
/// so an attacker can freely mint a token there; without binding, that harvested
/// token would validate against a *victim's* cross-site POST, defeating the whole
/// defense. Binding to the session means a token minted with no/attacker session
/// (`binding=""` or the attacker's own) won't validate against the victim's
/// session presented on the forged POST. `binding` is HMAC *input*, never echoed,
/// so passing the raw session token here does not leak it.
fn generate_csrf_token(binding: &str) -> String {
    let ts = chrono::Utc::now().timestamp();
    let mut mac = HmacSha256::new_from_slice(&*CSRF_SECRET).unwrap();
    mac.update(ts.to_le_bytes().as_ref());
    mac.update(b".");
    mac.update(binding.as_bytes());
    let sig = hex_encode(&mac.finalize().into_bytes());
    format!("{ts}.{sig}")
}

/// Validate a CSRF token against the binding it must have been issued for.
/// Returns true only if the HMAC matches AND the token is not older than 10 minutes.
fn validate_csrf_token(token: &str, binding: &str) -> bool {
    let Some((ts_str, sig)) = token.split_once('.') else {
        return false;
    };
    let Ok(ts) = ts_str.parse::<i64>() else {
        return false;
    };
    // Check expiry (10 minutes)
    let now = chrono::Utc::now().timestamp();
    if now - ts > 600 || ts > now + 60 {
        return false;
    }
    // Verify HMAC over (timestamp || binding). LIF-208: use the MAC's own
    // constant-time `verify_slice` rather than `expected == sig` on the hex
    // strings, which short-circuits on the first mismatched byte and leaks a
    // timing oracle. Decode the presented hex first; malformed hex is a reject.
    let Ok(sig_bytes) = hex_decode(sig) else {
        return false;
    };
    let mut mac = HmacSha256::new_from_slice(&*CSRF_SECRET).unwrap();
    mac.update(ts.to_le_bytes().as_ref());
    mac.update(b".");
    mac.update(binding.as_bytes());
    mac.verify_slice(&sig_bytes).is_ok()
}

/// Extract the session credential a browser would present: the `Authorization:
/// Bearer` header first, then the `lific_token` cookie. Returns an empty string
/// when neither is present, so the CSRF binding is still well-defined for the
/// unauthenticated case. Used to bind a CSRF token to its session both when the
/// authorize page is rendered and when the approval is submitted.
fn session_credential(headers: &HeaderMap) -> String {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.trim().to_string())
        .or_else(|| {
            headers
                .get("cookie")
                .and_then(|v| v.to_str().ok())
                .and_then(|cookies| {
                    cookies.split(';').find_map(|c| {
                        c.trim()
                            .strip_prefix("lific_token=")
                            .map(|v| v.trim().to_string())
                    })
                })
        })
        .unwrap_or_default()
}

#[derive(Clone)]
pub struct OAuthState {
    pub db: DbPool,
    pub issuer: String, // e.g. https://fedora.tailb93ac8.ts.net/lific
    /// Per-IP rate limiter for the unauthenticated /oauth/register endpoint.
    /// Prevents anyone from flooding the server with throwaway clients.
    pub register_limiter: Arc<RateLimiter>,
}

/// Validate a redirect URI submitted to dynamic client registration.
///
/// We only accept absolute `http://` or `https://` URLs. This explicitly
/// rejects schemes that have been used in past OAuth attacks (e.g.
/// `javascript:`, `data:`, `file:`, `vbscript:`, `blob:`, `about:`,
/// custom app schemes, and bare scheme-less strings).
///
/// Note: we deliberately do NOT block private/loopback hosts because
/// `http://localhost/callback` is the standard pattern for desktop
/// OAuth clients.
pub(crate) fn validate_redirect_uri(uri: &str) -> Result<(), &'static str> {
    let trimmed = uri.trim();
    if trimmed.is_empty() {
        return Err("redirect_uri must not be empty");
    }
    // Lowercase the scheme prefix only; the rest of the URI is case-sensitive.
    let lower_prefix: String = trimmed
        .chars()
        .take_while(|c| *c != ':')
        .flat_map(char::to_lowercase)
        .collect();
    match lower_prefix.as_str() {
        "http" | "https" => {}
        _ => return Err("redirect_uri must use http or https scheme"),
    }
    // Require the scheme to be followed by `://` (rejects e.g. `http:evil`).
    let after_scheme = &trimmed[lower_prefix.len()..];
    if !after_scheme.starts_with("://") {
        return Err("redirect_uri must be an absolute URL (scheme://host/...)");
    }
    // Require some host after `://`.
    let rest = &after_scheme[3..];
    let host_end = rest
        .find(['/', '?', '#'])
        .unwrap_or(rest.len());
    if rest[..host_end].is_empty() {
        return Err("redirect_uri must include a host");
    }
    Ok(())
}

pub fn router(state: OAuthState) -> Router {
    Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(protected_resource_metadata),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(authorization_server_metadata),
        )
        // some clients append the resource path
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(protected_resource_metadata),
        )
        .route("/oauth/register", post(register_client))
        .route(
            "/oauth/authorize",
            get(authorize_page).post(authorize_approve),
        )
        .route("/oauth/token", post(token_exchange))
        .route("/oauth/revoke", post(revoke_token))
        // Claude.ai strips /oauth/ prefix (known bug anthropics/claude-ai-mcp#82)
        .route("/register", post(register_client))
        .route("/authorize", get(authorize_page).post(authorize_approve))
        .route("/token", post(token_exchange))
        .route("/revoke", post(revoke_token))
        .with_state(state)
}

// ── Discovery ────────────────────────────────────────────────────────────

/// Derive the issuer URL from request headers, accommodating both local and
/// network deployments.
///
/// Fallback chain:
/// 1. X-Forwarded-Proto + X-Forwarded-Host (for reverse proxies)
/// 2. Host header (direct connections)
/// 3. Fall back to the configured issuer (explicit public_url or default)
fn derive_issuer_from_request(headers: &HeaderMap, fallback: &str) -> String {
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_lowercase())
        .unwrap_or_else(|| if fallback.starts_with("https") { String::from("https") } else { String::from("http") });

    if let Some(host) = headers
        .get("x-forwarded-host")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim()) {
        if !host.is_empty() {
            return format!("{scheme}://{host}");
        }
    }

    if let Some(host) = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim()) {
        if !host.is_empty() {
            return format!("{scheme}://{host}");
        }
    }

    fallback.to_string()
}

async fn protected_resource_metadata(
    State(state): State<OAuthState>,
    headers: HeaderMap,
) -> Json<serde_json::Value> {
    let issuer = derive_issuer_from_request(&headers, &state.issuer);
    let resource = format!("{}/mcp", issuer.trim_end_matches('/'));
    Json(serde_json::json!({
        "resource": resource,
        "authorization_servers": [issuer],
        "scopes_supported": ["mcp"],
        "bearer_methods_supported": ["header"]
    }))
}

async fn authorization_server_metadata(
    State(state): State<OAuthState>,
    headers: HeaderMap,
) -> Json<serde_json::Value> {
    let issuer = derive_issuer_from_request(&headers, &state.issuer);
    Json(serde_json::json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{}/oauth/authorize", issuer),
        "token_endpoint": format!("{}/oauth/token", issuer),
        "registration_endpoint": format!("{}/oauth/register", issuer),
        "revocation_endpoint": format!("{}/oauth/revoke", issuer),
        "scopes_supported": ["mcp"],
        "response_types_supported": ["code"],
        "response_modes_supported": ["query"],
        "grant_types_supported": ["authorization_code"],
        "token_endpoint_auth_methods_supported": ["client_secret_post", "none"],
        "code_challenge_methods_supported": ["S256"]
    }))
}

// ── Dynamic Client Registration ──────────────────────────────────────────

#[derive(Deserialize)]
struct RegisterRequest {
    redirect_uris: Vec<String>,
    client_name: Option<String>,
    #[serde(default)]
    token_endpoint_auth_method: Option<String>,
    #[serde(default)]
    grant_types: Option<Vec<String>>,
    #[serde(default)]
    response_types: Option<Vec<String>>,
}

async fn register_client(
    State(state): State<OAuthState>,
    headers: HeaderMap,
    Json(req): Json<RegisterRequest>,
) -> Response {
    // ── Rate limit per source IP ──
    // /oauth/register is unauthenticated by spec (RFC 7591), so without this
    // anyone on the internet can mint unlimited clients.
    let ip = crate::ratelimit::client_ip(&headers);
    let key = format!("oauth_register:{ip}");
    if !state.register_limiter.check(&key) {
        let retry = state.register_limiter.retry_after(&key);
        warn!(ip = %ip, "oauth client registration rate limited");
        let mut resp = (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "error": "too_many_requests",
                "error_description": format!("too many client registrations — try again in {retry} seconds")
            })),
        )
            .into_response();
        if let Ok(v) = retry.to_string().parse() {
            resp.headers_mut().insert("retry-after", v);
        }
        return resp;
    }

    if req.redirect_uris.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "invalid_redirect_uri",
                "error_description": "at least one redirect_uri is required"
            })),
        )
            .into_response();
    }

    // ── Validate every submitted redirect_uri ──
    for uri in &req.redirect_uris {
        if let Err(reason) = validate_redirect_uri(uri) {
            warn!(ip = %ip, uri = %uri, reason = %reason, "rejected oauth registration");
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "invalid_redirect_uri",
                    "error_description": reason
                })),
            )
                .into_response();
        }
    }

    let client_id = uuid_v4();
    let client_name = req.client_name.unwrap_or_else(|| "MCP Client".into());
    let redirect_uris_json =
        serde_json::to_string(&req.redirect_uris).unwrap_or_else(|_| "[]".into());

    let db = state.db.clone();
    let conn = match db.write() {
        Ok(c) => c,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response(),
    };
    if let Err(e) = conn.execute(
        "INSERT INTO oauth_clients (client_id, client_name, redirect_uris) VALUES (?1, ?2, ?3)",
        params![client_id, client_name, redirect_uris_json],
    ) {
        tracing::error!(error = %e, "failed to register OAuth client");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    info!(client_id = %client_id, name = %client_name, "OAuth client registered");

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "client_id": client_id,
            "client_name": client_name,
            "redirect_uris": req.redirect_uris,
            "token_endpoint_auth_method": req.token_endpoint_auth_method.unwrap_or_else(|| "none".into()),
            "grant_types": req.grant_types.unwrap_or_else(|| vec!["authorization_code".into()]),
            "response_types": req.response_types.unwrap_or_else(|| vec!["code".into()])
        })),
    )
        .into_response()
}

// ── Authorization ────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct AuthorizeParams {
    client_id: String,
    redirect_uri: String,
    response_type: String,
    state: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    scope: Option<String>,
}

async fn authorize_page(headers: HeaderMap, Query(params): Query<AuthorizeParams>) -> Html<String> {
    // Bind the CSRF token to the session the browser presents when loading this
    // page (sent on the top-level GET navigation under SameSite=Lax). The POST
    // approval must carry the same session for the token to validate.
    let csrf_token = generate_csrf_token(&session_credential(&headers));
    Html(format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <title>Lific - Authorize</title>
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <style>
        body {{ font-family: system-ui, sans-serif; max-width: 400px; margin: 80px auto; padding: 0 20px; background: #0a0a0a; color: #e0e0e0; }}
        h1 {{ font-size: 1.4em; margin-bottom: 0.5em; }}
        p {{ color: #888; line-height: 1.5; }}
        .client {{ color: #fff; font-weight: 600; }}
        form {{ margin-top: 2em; }}
        button {{ background: #2563eb; color: white; border: none; padding: 12px 32px; border-radius: 6px; font-size: 1em; cursor: pointer; width: 100%; }}
        button:hover {{ background: #1d4ed8; }}
    </style>
</head>
<body>
    <h1>Authorize access to Lific</h1>
    <p>An application wants to access your Lific issue tracker.</p>
    <form method="POST" action="/oauth/authorize">
        <input type="hidden" name="client_id" value="{client_id}">
        <input type="hidden" name="redirect_uri" value="{redirect_uri}">
        <input type="hidden" name="response_type" value="{response_type}">
        <input type="hidden" name="state" value="{state}">
        <input type="hidden" name="code_challenge" value="{code_challenge}">
        <input type="hidden" name="code_challenge_method" value="{code_challenge_method}">
        <input type="hidden" name="scope" value="{scope}">
        <input type="hidden" name="csrf_token" value="{csrf_token}">
        <button type="submit">Approve</button>
    </form>
</body>
</html>"#,
        client_id = html_escape(&params.client_id),
        redirect_uri = html_escape(&params.redirect_uri),
        response_type = html_escape(&params.response_type),
        state = html_escape(params.state.as_deref().unwrap_or("")),
        code_challenge = html_escape(params.code_challenge.as_deref().unwrap_or("")),
        code_challenge_method =
            html_escape(params.code_challenge_method.as_deref().unwrap_or("S256")),
        scope = html_escape(params.scope.as_deref().unwrap_or("mcp")),
        csrf_token = html_escape(&csrf_token),
    ))
}

#[derive(Deserialize)]
struct ApproveForm {
    client_id: String,
    redirect_uri: String,
    #[allow(dead_code)]
    response_type: String,
    state: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    #[allow(dead_code)]
    scope: Option<String>,
    csrf_token: Option<String>,
}

async fn authorize_approve(
    State(oauth): State<OAuthState>,
    headers: axum::http::HeaderMap,
    axum::Form(form): axum::Form<ApproveForm>,
) -> Response {
    // The credential presented on this POST (Bearer header or lific_token
    // cookie). The CSRF token must have been minted for this same credential.
    let credential = session_credential(&headers);

    // Validate CSRF token, BOUND to the presenting session, to prevent
    // cross-site form submission attacks. A token harvested from the
    // unauthenticated authorize page (bound to no/attacker session) will not
    // match a victim's session presented here.
    match &form.csrf_token {
        Some(token) if validate_csrf_token(token, &credential) => {}
        _ => {
            return (
                StatusCode::FORBIDDEN,
                Html("<h1>Invalid or expired form</h1><p>Please go back and try again. <a href=\"/#/\">Return to Lific</a></p>".to_string()),
            )
                .into_response();
        }
    }

    // Require authentication -- the person approving must be identified.
    // `credential` is the session token from the Authorization header or cookie;
    // it is validated against the database below to ensure it's real and unexpired.
    let Some(token) = (!credential.is_empty()).then_some(credential) else {
        return (
            StatusCode::UNAUTHORIZED,
            Html("<h1>Authentication required</h1><p>You must be signed in to approve OAuth access. <a href=\"/#/login\">Sign in</a></p>".to_string()),
        )
            .into_response();
    };

    // Validate the token against the database AND capture the approving
    // user's identity so it can be bound to the issued code (LIF-79). OAuth
    // routes bypass the auth middleware, so we validate here.
    //
    // auth_outcome:
    //   None           -> token invalid -> reject
    //   Some(None)     -> authenticated but no resolvable user (a legacy
    //                     OAuth token issued before LIF-79) -> proceed and
    //                     bind no user, preserving the old behavior
    //   Some(Some(id)) -> authenticated as user `id` -> bind it to the code
    let auth_outcome: Option<Option<i64>> = if token.starts_with("lific_sess_") {
        let conn = match oauth.db.write() {
            Ok(c) => c,
            Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response(),
        };
        crate::db::queries::users::validate_session(&conn, &token)
            .ok()
            .map(|u| Some(u.id))
    } else if token.starts_with("lific_at_") {
        // OAuth tokens can also approve (valid authenticated identity).
        if validate_oauth_token(&oauth.db, &token) {
            Some(oauth_token_user_id(&oauth.db, &token))
        } else {
            None
        }
    } else {
        None
    };

    let Some(approving_user_id) = auth_outcome else {
        return (
            StatusCode::UNAUTHORIZED,
            Html("<h1>Invalid session</h1><p>Your session has expired or is invalid. <a href=\"/#/login\">Sign in again</a></p>".to_string()),
        )
            .into_response();
    };

    // Validate the redirect_uri against the client's registered URIs
    let redirect_ok = if let Ok(conn) = oauth.db.read() {
        let registered: Result<String, _> = conn.query_row(
            "SELECT redirect_uris FROM oauth_clients WHERE client_id = ?1",
            params![form.client_id],
            |row| row.get(0),
        );
        match registered {
            Ok(uris_json) => {
                let uris: Vec<String> = serde_json::from_str(&uris_json).unwrap_or_default();
                uris.iter().any(|u| u == &form.redirect_uri)
            }
            Err(_) => false,
        }
    } else {
        false
    };

    if !redirect_ok {
        return (
            StatusCode::BAD_REQUEST,
            Html("Invalid client_id or redirect_uri does not match registered URIs.".to_string()),
        )
            .into_response();
    }

    let code = uuid_v4();
    let expires = chrono::Utc::now() + chrono::Duration::minutes(10);
    let scope = form.scope.as_deref().unwrap_or("mcp");

    let conn = match oauth.db.write() {
        Ok(c) => c,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response(),
    };
    if let Err(e) = conn.execute(
        "INSERT INTO oauth_codes (code, client_id, redirect_uri, code_challenge, code_challenge_method, expires_at, scope, user_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            code,
            form.client_id,
            form.redirect_uri,
            form.code_challenge.unwrap_or_default(),
            form.code_challenge_method.unwrap_or_else(|| "S256".into()),
            expires.to_rfc3339(),
            scope,
            approving_user_id,
        ],
    ) {
        tracing::error!(error = %e, "failed to store OAuth authorization code");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    let mut redirect_url = form.redirect_uri.clone();
    redirect_url.push_str(if redirect_url.contains('?') { "&" } else { "?" });
    redirect_url.push_str(&format!("code={code}"));
    if let Some(state) = &form.state
        && !state.is_empty()
    {
        let encoded = urlencoding::encode(state);
        redirect_url.push_str(&format!("&state={encoded}"));
    }

    info!(client_id = %form.client_id, "OAuth authorization approved");
    Redirect::to(&redirect_url).into_response()
}

// ── Token Exchange ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct TokenRequest {
    grant_type: String,
    code: Option<String>,
    redirect_uri: Option<String>,
    client_id: Option<String>,
    code_verifier: Option<String>,
    #[allow(dead_code)]
    refresh_token: Option<String>,
}

#[derive(Serialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
    expires_in: u64,
    scope: String,
}

async fn token_exchange(
    State(state): State<OAuthState>,
    axum::Form(req): axum::Form<TokenRequest>,
) -> Response {
    if req.grant_type != "authorization_code" {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "unsupported_grant_type"})),
        )
            .into_response();
    }

    let Some(code) = &req.code else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid_request", "error_description": "missing code"})),
        )
            .into_response();
    };

    let Some(code_verifier) = &req.code_verifier else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid_request", "error_description": "missing code_verifier"})),
        )
            .into_response();
    };

    // Look up the authorization code
    let conn = match state.db.write() {
        Ok(c) => c,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response(),
    };

    // Named row type keeps the query_row result readable and avoids
    // clippy::type_complexity on the 7-column tuple (LIF-79 added user_id).
    struct AuthCodeRow {
        client_id: String,
        redirect_uri: String,
        code_challenge: String,
        challenge_method: String,
        used: i64,
        scope: String,
        user_id: Option<i64>,
    }

    let code_row: Result<AuthCodeRow, _> = conn.query_row(
        "SELECT client_id, redirect_uri, code_challenge, code_challenge_method, used, scope, user_id FROM oauth_codes WHERE code = ?1 AND expires_at > datetime('now')",
        params![code],
        |row| {
            Ok(AuthCodeRow {
                client_id: row.get(0)?,
                redirect_uri: row.get(1)?,
                code_challenge: row.get(2)?,
                challenge_method: row.get(3)?,
                used: row.get(4)?,
                scope: row.get(5)?,
                user_id: row.get(6)?,
            })
        },
    );

    let AuthCodeRow {
        client_id: stored_client_id,
        redirect_uri: stored_redirect_uri,
        code_challenge,
        challenge_method,
        used,
        scope,
        user_id: code_user_id,
    } = match code_row {
        Ok(row) => row,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_grant"})),
            )
                .into_response();
        }
    };

    if used != 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid_grant", "error_description": "code already used"})),
        )
            .into_response();
    }

    // Validate client_id — required per OAuth 2.1 for public clients
    let Some(client_id) = &req.client_id else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid_request", "error_description": "missing client_id"})),
        )
            .into_response();
    };
    if *client_id != stored_client_id {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid_grant"})),
        )
            .into_response();
    }

    // Validate redirect_uri matches the one used during authorization (OAuth 2.1 Section 4.1.3)
    match &req.redirect_uri {
        Some(uri) if *uri != stored_redirect_uri => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_grant", "error_description": "redirect_uri mismatch"})),
            )
                .into_response();
        }
        None => {
            // redirect_uri is required when it was included in the authorization request
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid_request", "error_description": "missing redirect_uri"})),
            )
                .into_response();
        }
        _ => {} // matches — continue
    }

    // Validate PKCE
    if !validate_pkce(code_verifier, &code_challenge, &challenge_method) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid_grant", "error_description": "PKCE verification failed"})),
        )
            .into_response();
    }

    // Mark code as used
    if let Err(e) = conn.execute(
        "UPDATE oauth_codes SET used = 1 WHERE code = ?1",
        params![code],
    ) {
        tracing::error!(error = %e, "failed to mark OAuth code as used");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    // Generate access token — store SHA-256 hash, return raw token only once
    let access_token = format!("lific_at_{}", uuid_v4());
    let token_hash = hex_encode(&Sha256::digest(access_token.as_bytes()));
    let expires_in: u64 = 3600 * 24 * 30; // 30 days
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(expires_in as i64);

    if let Err(e) = conn.execute(
        "INSERT INTO oauth_tokens (access_token, client_id, expires_at, scope, user_id) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![token_hash, stored_client_id, expires_at.to_rfc3339(), scope, code_user_id],
    ) {
        tracing::error!(error = %e, "failed to store OAuth token");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    info!(client_id = %stored_client_id, scope = %scope, "OAuth token issued");

    Json(TokenResponse {
        access_token,
        token_type: "Bearer".into(),
        expires_in,
        scope,
    })
    .into_response()
}

// ── Token Revocation (RFC 7009) ──────────────────────────────────────────

#[derive(Deserialize)]
struct RevokeRequest {
    token: String,
    #[allow(dead_code)]
    token_type_hint: Option<String>,
}

async fn revoke_token(
    State(state): State<OAuthState>,
    headers: axum::http::HeaderMap,
    axum::Form(req): axum::Form<RevokeRequest>,
) -> Response {
    // Require authentication -- only authenticated users/tokens can revoke.
    let caller_token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.trim().to_string());

    let is_authenticated = match &caller_token {
        Some(t) if t.starts_with("lific_sess_") => {
            match state.db.write() {
                Ok(conn) => crate::db::queries::users::validate_session(&conn, t).is_ok(),
                Err(_) => false,
            }
        }
        Some(t) if t.starts_with("lific_at_") => validate_oauth_token(&state.db, t),
        // LIF-208: default-deny unknown bearer shapes. The previous
        // `Some(_) => true` treated *any* other string (including arbitrary
        // garbage) as authenticated, which is sloppier than the rest of the
        // file. The OAuth router doesn't run the API-key middleware and has no
        // key manager, so it can't validate `lific_sk` keys here; a legitimate
        // caller revoking a token presents a session or the OAuth token itself.
        Some(_) => false,
        None => false,
    };

    if !is_authenticated {
        return (StatusCode::UNAUTHORIZED, "authentication required").into_response();
    }

    // RFC 7009 says the server MUST respond with 200 even if the token
    // is invalid, already revoked, or unrecognized -- to prevent token scanning.
    // Hash the token before lookup since we store SHA-256 hashes.
    let token_hash = hex_encode(&Sha256::digest(req.token.as_bytes()));
    // RFC 7009: always return 200, but log DB errors instead of silently discarding
    match state.db.write() {
        Ok(conn) => {
            if let Err(e) = conn.execute(
                "UPDATE oauth_tokens SET revoked = 1 WHERE access_token = ?1",
                params![token_hash],
            ) {
                tracing::error!(error = %e, "failed to revoke OAuth token");
            }
        }
        Err(e) => tracing::error!(error = %e, "failed to acquire DB lock for token revocation"),
    }

    StatusCode::OK.into_response()
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn validate_pkce(verifier: &str, challenge: &str, method: &str) -> bool {
    // OAuth 2.1 requires S256 only. Reject empty challenges/verifiers.
    if verifier.is_empty() || challenge.is_empty() {
        return false;
    }
    match method {
        "S256" => {
            let hash = Sha256::digest(verifier.as_bytes());
            let computed = base64_url_encode(&hash);
            computed == challenge
        }
        _ => false, // Only S256 is accepted per OAuth 2.1
    }
}

/// Encode bytes as lowercase hex string.
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Decode a lowercase/uppercase hex string into bytes. Returns `Err(())` on
/// odd length or any non-hex digit. Used to parse a presented CSRF MAC before
/// constant-time verification (LIF-208).
fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    if !s.len().is_multiple_of(2) {
        return Err(());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| ()))
        .collect()
}

fn base64_url_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    URL_SAFE_NO_PAD.encode(bytes)
}

fn uuid_v4() -> String {
    let bytes: [u8; 16] = rand::random();
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        u16::from_be_bytes([bytes[4], bytes[5]]),
        u16::from_be_bytes([bytes[6], bytes[7]]) & 0x0fff,
        u16::from_be_bytes([bytes[8], bytes[9]]) & 0x3fff | 0x8000,
        u64::from_be_bytes([
            0, 0, bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
        ])
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Check if a bearer token is a valid OAuth access token.
pub fn validate_oauth_token(db: &DbPool, token: &str) -> bool {
    validate_oauth_token_with_scope(db, token).is_some()
}

/// Validate an OAuth token and return its granted scope.
/// Tokens are stored as SHA-256 hashes; the incoming raw token is hashed before lookup.
pub fn validate_oauth_token_with_scope(db: &DbPool, token: &str) -> Option<String> {
    if !token.starts_with("lific_at_") {
        return None;
    }
    let token_hash = hex_encode(&Sha256::digest(token.as_bytes()));
    let conn = db.read().ok()?;
    conn.query_row(
        "SELECT scope FROM oauth_tokens
         WHERE access_token = ?1 AND revoked = 0 AND expires_at > datetime('now')",
        params![token_hash],
        |row| row.get(0),
    )
    .ok()
}

/// Resolve the user bound to a (valid, non-revoked, unexpired) OAuth access
/// token, if any (LIF-79). Returns `None` when the token is invalid OR when it
/// is a legacy token issued before user binding existed — callers treat both
/// as "no user identity." Tokens are stored as SHA-256 hashes.
pub fn oauth_token_user_id(db: &DbPool, token: &str) -> Option<i64> {
    if !token.starts_with("lific_at_") {
        return None;
    }
    let token_hash = hex_encode(&Sha256::digest(token.as_bytes()));
    let conn = db.read().ok()?;
    conn.query_row(
        "SELECT user_id FROM oauth_tokens
         WHERE access_token = ?1 AND revoked = 0 AND expires_at > datetime('now')",
        params![token_hash],
        |row| row.get::<_, Option<i64>>(0),
    )
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_oauth_app() -> (Router, DbPool) {
        test_oauth_app_with_register_limit(1000)
    }

    /// Build a test OAuth router with a configurable register-limit cap.
    /// Most tests need a generous cap so unrelated registrations don't
    /// trip the limiter; the rate-limit tests pass a small cap.
    fn test_oauth_app_with_register_limit(cap: usize) -> (Router, DbPool) {
        let db = crate::db::open_memory().expect("test db");
        let state = OAuthState {
            db: db.clone(),
            issuer: "https://example.com".into(),
            register_limiter: Arc::new(RateLimiter::new(cap, std::time::Duration::from_secs(3600))),
        };
        (router(state), db)
    }

    /// Register a client, returning the client_id.
    async fn register_client_helper(app: &Router, redirect_uri: &str) -> String {
        let body = serde_json::json!({
            "redirect_uris": [redirect_uri],
            "client_name": "Test Client"
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/register")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        val["client_id"].as_str().unwrap().to_string()
    }

    /// Create a user session for OAuth tests.
    fn create_test_session(db: &DbPool) -> String {
        let conn = db.write().unwrap();
        let user = crate::db::queries::users::create_user(
            &conn,
            &crate::db::models::CreateUser {
                username: "oauthtest".into(),
                email: "oauth@test.com".into(),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap();
        let session = crate::db::queries::users::create_session(&conn, user.id, None).unwrap();
        session.token
    }

    /// Build the form body for an authorize POST, including a CSRF token bound
    /// to `binding` (the session credential the POST will carry: a Bearer token,
    /// a cookie value, or "" for the unauthenticated case).
    fn authorize_body(client_id: &str, redirect_uri: &str, binding: &str) -> String {
        let csrf = generate_csrf_token(binding);
        format!(
            "client_id={}&redirect_uri={}&response_type=code&code_challenge=abc&code_challenge_method=S256&scope=mcp&csrf_token={}",
            client_id,
            urlencoding::encode(redirect_uri),
            urlencoding::encode(&csrf),
        )
    }

    // ── LIF-48: authorize_approve validates tokens ───────────

    #[tokio::test]
    async fn authorize_rejects_missing_auth() {
        let (app, _db) = test_oauth_app();
        let client_id = register_client_helper(&app, "http://localhost/callback").await;
        let body = authorize_body(&client_id, "http://localhost/callback", "");
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/authorize")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authorize_rejects_garbage_bearer_token() {
        let (app, _db) = test_oauth_app();
        let client_id = register_client_helper(&app, "http://localhost/callback").await;
        // CSRF bound to the (garbage) token actually presented, so we exercise
        // the session-validation path rather than tripping the CSRF check.
        let body = authorize_body(
            &client_id,
            "http://localhost/callback",
            "lific_sess_fake_garbage_token",
        );
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/authorize")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("authorization", "Bearer lific_sess_fake_garbage_token")
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authorize_rejects_fake_cookie_token() {
        let (app, _db) = test_oauth_app();
        let client_id = register_client_helper(&app, "http://localhost/callback").await;
        let body = authorize_body(
            &client_id,
            "http://localhost/callback",
            "lific_sess_fake_garbage_token",
        );
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/authorize")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("cookie", "lific_token=lific_sess_fake_garbage_token")
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authorize_accepts_valid_session_token() {
        let (app, db) = test_oauth_app();
        let session_token = create_test_session(&db);
        let client_id = register_client_helper(&app, "http://localhost/callback").await;
        let body = authorize_body(&client_id, "http://localhost/callback", &session_token);
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/authorize")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("authorization", format!("Bearer {session_token}"))
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        // Should redirect (303 or 302), not reject
        assert!(
            resp.status().is_redirection() || resp.status() == StatusCode::SEE_OTHER,
            "expected redirect, got {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn authorize_accepts_valid_cookie_session() {
        let (app, db) = test_oauth_app();
        let session_token = create_test_session(&db);
        let client_id = register_client_helper(&app, "http://localhost/callback").await;
        let body = authorize_body(&client_id, "http://localhost/callback", &session_token);
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/authorize")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("cookie", format!("lific_token={session_token}"))
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status().is_redirection(),
            "expected redirect, got {}",
            resp.status()
        );
    }

    /// CSRF regression: a token harvested from the unauthenticated authorize
    /// page (bound to no session, `binding=""`) must NOT validate when replayed
    /// against a victim's authenticated session. This is the exact cross-site
    /// attack the binding closes — without it, the harvested token would pass
    /// CSRF and the victim's cookie would drive an approval. Expect 403, not a
    /// redirect.
    #[tokio::test]
    async fn authorize_rejects_unbound_csrf_replayed_with_victim_session() {
        let (app, db) = test_oauth_app();
        let victim_session = create_test_session(&db);
        let client_id = register_client_helper(&app, "http://localhost/callback").await;
        // Attacker mints a CSRF from the public GET page → bound to "".
        let body = authorize_body(&client_id, "http://localhost/callback", "");
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/authorize")
                    .header("content-type", "application/x-www-form-urlencoded")
                    // Victim's session rides along (e.g. via cookie).
                    .header("cookie", format!("lific_token={victim_session}"))
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "harvested unbound CSRF must be rejected against a victim session"
        );
    }

    /// The authorize page binds the CSRF token to the session that loaded it, so
    /// a CSRF minted for one session must not authorize a different one.
    #[tokio::test]
    async fn authorize_rejects_csrf_bound_to_a_different_session() {
        let (app, db) = test_oauth_app();
        let victim_session = create_test_session(&db);
        let client_id = register_client_helper(&app, "http://localhost/callback").await;
        // CSRF bound to some OTHER session value than the one presented.
        let body = authorize_body(
            &client_id,
            "http://localhost/callback",
            "lific_sess_some_other_session",
        );
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/authorize")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("authorization", format!("Bearer {victim_session}"))
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    /// Unit-level proof the binding is enforced in the token primitives.
    #[test]
    fn csrf_token_is_bound_to_its_session() {
        let t = generate_csrf_token("session-A");
        assert!(validate_csrf_token(&t, "session-A"));
        assert!(!validate_csrf_token(&t, "session-B"));
        assert!(!validate_csrf_token(&t, ""));
    }

    // ── LIF-208: constant-time CSRF MAC verification ─────────
    // The validator now hex-decodes the presented signature and verifies it
    // with the MAC's own constant-time compare. These guard the new decode
    // path: a valid token still round-trips, and tampered / malformed
    // signatures are rejected rather than panicking or short-circuiting.
    #[test]
    fn csrf_rejects_tampered_and_malformed_signatures() {
        let t = generate_csrf_token("sess");
        assert!(validate_csrf_token(&t, "sess"), "honest token must validate");

        let (ts, sig) = t.split_once('.').unwrap();

        // Flip one hex nibble in the signature → MAC mismatch, must reject.
        let mut bad = sig.to_string();
        let first = bad.remove(0);
        let flipped = if first == '0' { '1' } else { '0' };
        bad.insert(0, flipped);
        assert!(!validate_csrf_token(&format!("{ts}.{bad}"), "sess"));

        // Non-hex characters in the signature → decode fails, must reject.
        assert!(!validate_csrf_token(&format!("{ts}.zzzz"), "sess"));

        // Odd-length hex → decode fails, must reject.
        assert!(!validate_csrf_token(&format!("{ts}.abc"), "sess"));

        // Empty signature → reject.
        assert!(!validate_csrf_token(&format!("{ts}."), "sess"));
    }

    #[test]
    fn hex_decode_roundtrips_and_rejects_bad_input() {
        assert_eq!(hex_decode("00ff10").unwrap(), vec![0x00, 0xff, 0x10]);
        assert_eq!(hex_decode(&hex_encode(b"lific")).unwrap(), b"lific");
        assert!(hex_decode("abc").is_err(), "odd length rejected");
        assert!(hex_decode("zz").is_err(), "non-hex rejected");
    }

    // ── LIF-49: metadata does not advertise refresh_token ────

    #[tokio::test]
    async fn metadata_does_not_advertise_refresh_token() {
        let (app, _) = test_oauth_app();
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/.well-known/oauth-authorization-server")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        let grants = val["grant_types_supported"].as_array().unwrap();
        assert!(
            !grants.iter().any(|g| g == "refresh_token"),
            "metadata should not advertise refresh_token grant"
        );
        assert!(grants.iter().any(|g| g == "authorization_code"));
    }

    #[tokio::test]
    async fn register_defaults_do_not_include_refresh_token() {
        let (app, _) = test_oauth_app();
        let body = serde_json::json!({
            "redirect_uris": ["http://localhost/callback"],
            "client_name": "Test"
        });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/register")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        let grants = val["grant_types"].as_array().unwrap();
        assert!(
            !grants.iter().any(|g| g == "refresh_token"),
            "client registration should not default to refresh_token"
        );
    }

    // ── Protected-resource metadata advertises the /mcp resource ──
    // Claude.ai derives the RFC 8707 audience from the MCP URL the user enters
    // (`https://host/mcp`) and rejects the issued token if the protected-resource
    // metadata's `resource` is the bare origin. Both the root and the path-aware
    // well-known routes must advertise the path-qualified resource.
    #[tokio::test]
    async fn protected_resource_metadata_resource_includes_mcp_path() {
        let (app, _) = test_oauth_app();
        for path in [
            "/.well-known/oauth-protected-resource",
            "/.well-known/oauth-protected-resource/mcp",
        ] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "path {path}");
            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(val["resource"], "https://example.com/mcp", "path {path}");
            assert_eq!(
                val["authorization_servers"][0], "https://example.com",
                "path {path}"
            );
        }
    }

    // ── LIF-50: token revocation ─────────────────────────────

    #[tokio::test]
    async fn revoke_token_invalidates_access() {
        let (app, db) = test_oauth_app();

        // Manually insert a token to revoke (stored as SHA-256 hash)
        let token = "lific_at_test-revoke-token";
        let token_hash = hex_encode(&Sha256::digest(token.as_bytes()));
        let expires = (chrono::Utc::now() + chrono::Duration::hours(24)).to_rfc3339();
        {
            let conn = db.write().unwrap();
            // Need a client first
            conn.execute(
                "INSERT INTO oauth_clients (client_id, client_name, redirect_uris) VALUES ('test-client', 'Test', '[\"http://localhost\"]')",
                [],
            ).unwrap();
            conn.execute(
                "INSERT INTO oauth_tokens (access_token, client_id, expires_at, scope) VALUES (?1, 'test-client', ?2, 'mcp')",
                params![token_hash, expires],
            ).unwrap();
        }

        // Token should be valid
        assert!(validate_oauth_token(&db, token));

        // Revoke it (must be authenticated)
        let body = format!("token={token}");
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/revoke")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Token should now be invalid
        assert!(!validate_oauth_token(&db, token));
    }

    #[tokio::test]
    async fn revoke_unauthenticated_returns_401() {
        let (app, _) = test_oauth_app();

        // Without auth, revoke should be rejected
        let body = "token=lific_at_nonexistent";
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/revoke")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn revoke_unknown_token_returns_200() {
        let (app, db) = test_oauth_app();

        // Create a valid token so we can authenticate the revoke request
        let auth_token = "lific_at_auth-for-revoke";
        let auth_hash = hex_encode(&Sha256::digest(auth_token.as_bytes()));
        let expires = (chrono::Utc::now() + chrono::Duration::hours(24)).to_rfc3339();
        {
            let conn = db.write().unwrap();
            conn.execute(
                "INSERT INTO oauth_clients (client_id, client_name, redirect_uris) VALUES ('revoke-test', 'Test', '[\"http://localhost\"]')",
                [],
            ).unwrap();
            conn.execute(
                "INSERT INTO oauth_tokens (access_token, client_id, expires_at, scope) VALUES (?1, 'revoke-test', ?2, 'mcp')",
                params![auth_hash, expires],
            ).unwrap();
        }

        // RFC 7009: always return 200, even for unknown tokens (when authenticated)
        let body = "token=lific_at_nonexistent";
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/revoke")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("authorization", format!("Bearer {auth_token}"))
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── LIF-51: metadata advertises revocation endpoint ──────

    #[tokio::test]
    async fn metadata_includes_revocation_endpoint() {
        let (app, _) = test_oauth_app();
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/.well-known/oauth-authorization-server")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert!(val["revocation_endpoint"].as_str().is_some());
        assert!(
            val["revocation_endpoint"]
                .as_str()
                .unwrap()
                .ends_with("/oauth/revoke")
        );
    }

    // ── LIF-51: scope is stored on tokens ────────────────────

    #[tokio::test]
    async fn validate_oauth_token_returns_scope() {
        let (_, db) = test_oauth_app();

        let token = "lific_at_scope-test-token";
        let token_hash = hex_encode(&Sha256::digest(token.as_bytes()));
        let expires = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        {
            let conn = db.write().unwrap();
            conn.execute(
                "INSERT INTO oauth_clients (client_id, client_name, redirect_uris) VALUES ('scope-client', 'Test', '[\"http://localhost\"]')",
                [],
            ).unwrap();
            conn.execute(
                "INSERT INTO oauth_tokens (access_token, client_id, expires_at, scope) VALUES (?1, 'scope-client', ?2, 'mcp')",
                params![token_hash, expires],
            ).unwrap();
        }

        let scope = validate_oauth_token_with_scope(&db, token);
        assert_eq!(scope, Some("mcp".to_string()));
    }

    #[tokio::test]
    async fn revoked_token_has_no_scope() {
        let (_, db) = test_oauth_app();

        let token = "lific_at_revoked-scope-test";
        let token_hash = hex_encode(&Sha256::digest(token.as_bytes()));
        let expires = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        {
            let conn = db.write().unwrap();
            conn.execute(
                "INSERT INTO oauth_clients (client_id, client_name, redirect_uris) VALUES ('rev-client', 'Test', '[\"http://localhost\"]')",
                [],
            ).unwrap();
            conn.execute(
                "INSERT INTO oauth_tokens (access_token, client_id, expires_at, scope, revoked) VALUES (?1, 'rev-client', ?2, 'mcp', 1)",
                params![token_hash, expires],
            ).unwrap();
        }

        assert_eq!(validate_oauth_token_with_scope(&db, token), None);
        assert!(!validate_oauth_token(&db, token));
    }

    // ── LIF-64: redirect_uri validation + register rate limit ─────────────

    #[test]
    fn validate_redirect_uri_accepts_http_and_https() {
        assert!(validate_redirect_uri("http://localhost/callback").is_ok());
        assert!(validate_redirect_uri("http://127.0.0.1:8080/cb").is_ok());
        assert!(validate_redirect_uri("https://app.example.com/oauth/callback").is_ok());
        assert!(validate_redirect_uri("HTTP://localhost/callback").is_ok());
        assert!(validate_redirect_uri("HTTPS://example.com/").is_ok());
    }

    #[test]
    fn validate_redirect_uri_rejects_dangerous_schemes() {
        for evil in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "file:///etc/passwd",
            "vbscript:msgbox()",
            "about:blank",
            "blob:https://evil/x",
            "ftp://example.com/",
            "myapp://callback",
        ] {
            assert!(
                validate_redirect_uri(evil).is_err(),
                "should reject: {evil}"
            );
        }
    }

    #[test]
    fn validate_redirect_uri_rejects_malformed() {
        assert!(validate_redirect_uri("").is_err());
        assert!(validate_redirect_uri("   ").is_err());
        assert!(validate_redirect_uri("http:evil").is_err());
        assert!(validate_redirect_uri("not-a-url").is_err());
        assert!(validate_redirect_uri("https://").is_err());
        assert!(validate_redirect_uri("http:///path").is_err());
    }

    #[tokio::test]
    async fn register_rejects_javascript_redirect_uri() {
        let (app, _) = test_oauth_app();
        let body = serde_json::json!({
            "redirect_uris": ["javascript:alert(1)"],
            "client_name": "Evil"
        });
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/register")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(val["error"], "invalid_redirect_uri");
    }

    #[tokio::test]
    async fn register_rejects_when_any_redirect_is_invalid() {
        // One good, one bad — must reject the whole request.
        let (app, _) = test_oauth_app();
        let body = serde_json::json!({
            "redirect_uris": ["http://localhost/cb", "data:text/html,x"],
            "client_name": "Mixed"
        });
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/register")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn register_rate_limits_after_cap() {
        // Cap at 2 registrations per IP (window from test_oauth_app helper).
        let (app, _) = test_oauth_app_with_register_limit(2);
        let body = serde_json::json!({
            "redirect_uris": ["http://localhost/callback"],
            "client_name": "RL Test"
        });
        let send = || {
            let app = app.clone();
            let body = body.clone();
            async move {
                app.oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/oauth/register")
                        .header("content-type", "application/json")
                        .header("x-forwarded-for", "192.0.2.42")
                        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        };

        assert_eq!(send().await.status(), StatusCode::CREATED);
        assert_eq!(send().await.status(), StatusCode::CREATED);
        let limited = send().await;
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(limited.headers().get("retry-after").is_some());
    }

    #[tokio::test]
    async fn register_rate_limit_is_per_ip() {
        // Distinct X-Forwarded-For values should each get their own bucket.
        let (app, _) = test_oauth_app_with_register_limit(1);
        let body = serde_json::json!({
            "redirect_uris": ["http://localhost/callback"],
            "client_name": "Per-IP Test"
        });
        let send = |ip: &'static str| {
            let app = app.clone();
            let body = body.clone();
            async move {
                app.oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/oauth/register")
                        .header("content-type", "application/json")
                        .header("x-forwarded-for", ip)
                        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        };

        // First IP: allowed.
        assert_eq!(send("198.51.100.1").await.status(), StatusCode::CREATED);
        // Same IP again: limited (cap=1).
        assert_eq!(
            send("198.51.100.1").await.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        // Different IP: allowed (independent bucket).
        assert_eq!(send("198.51.100.2").await.status(), StatusCode::CREATED);
    }

    // ── LIF-79: OAuth codes/tokens bound to approving user ───────────────

    #[tokio::test]
    async fn token_is_bound_to_approving_user() {
        let (app, db) = test_oauth_app();
        let session_token = create_test_session(&db); // creates user "oauthtest"
        let client_id = register_client_helper(&app, "http://localhost/callback").await;

        let user_id: i64 = {
            let conn = db.read().unwrap();
            conn.query_row(
                "SELECT id FROM users WHERE username = 'oauthtest'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };

        // A real PKCE pair so the later token exchange passes verification.
        let verifier = "test_verifier_abcdefghijklmnopqrstuvwxyz_0123456789";
        let challenge = base64_url_encode(&Sha256::digest(verifier.as_bytes()));
        // CSRF bound to the session presented on the approval (cookie below).
        let csrf = generate_csrf_token(&session_token);
        let body = format!(
            "client_id={}&redirect_uri={}&response_type=code&code_challenge={}&code_challenge_method=S256&scope=mcp&csrf_token={}",
            client_id,
            urlencoding::encode("http://localhost/callback"),
            urlencoding::encode(&challenge),
            urlencoding::encode(&csrf),
        );

        // Approve via the session cookie.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/authorize")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("cookie", format!("lific_token={session_token}"))
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status().is_redirection(),
            "approve should redirect, got {}",
            resp.status()
        );
        let location = resp
            .headers()
            .get("location")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let code = location
            .split("code=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap()
            .to_string();

        // The authorization code carries the approver's id.
        {
            let conn = db.read().unwrap();
            let code_user: Option<i64> = conn
                .query_row(
                    "SELECT user_id FROM oauth_codes WHERE code = ?1",
                    params![code],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(code_user, Some(user_id), "code should bind the approver");
        }

        // Exchange the code; the issued token must carry the same identity.
        let token_body = format!(
            "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}",
            code,
            urlencoding::encode("http://localhost/callback"),
            client_id,
            verifier,
        );
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/token")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(axum::body::Body::from(token_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "token exchange should succeed"
        );
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let val: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let access_token = val["access_token"].as_str().unwrap();

        // The middleware will resolve this token to the approving user.
        assert_eq!(oauth_token_user_id(&db, access_token), Some(user_id));
    }

    #[tokio::test]
    async fn legacy_token_without_user_resolves_to_none() {
        // Tokens issued before LIF-79 have NULL user_id and must keep working,
        // resolving to no user (anonymous) rather than erroring.
        let (_, db) = test_oauth_app();
        let token = "lific_at_legacy-no-user-binding";
        let token_hash = hex_encode(&Sha256::digest(token.as_bytes()));
        let expires = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        {
            let conn = db.write().unwrap();
            conn.execute(
                "INSERT INTO oauth_clients (client_id, client_name, redirect_uris) VALUES ('legacy-c', 'Test', '[\"http://localhost\"]')",
                [],
            )
            .unwrap();
            // user_id intentionally omitted → NULL
            conn.execute(
                "INSERT INTO oauth_tokens (access_token, client_id, expires_at, scope) VALUES (?1, 'legacy-c', ?2, 'mcp')",
                params![token_hash, expires],
            )
            .unwrap();
        }
        assert!(validate_oauth_token(&db, token), "token still valid");
        assert_eq!(
            oauth_token_user_id(&db, token),
            None,
            "legacy token has no bound user"
        );
    }
}
