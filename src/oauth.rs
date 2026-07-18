use std::{net::SocketAddr, sync::Arc};

use axum::{
    Router,
    extract::{ConnectInfo, Json, Query, State},
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
    pub issuer: String, // e.g. https://lific.example.com/lific
    /// True when the issuer comes from an explicit `server.public_url`.
    /// An explicit issuer is advertised as-is; request-derived fallback
    /// (LIF-287) only applies when this is false.
    pub issuer_is_explicit: bool,
    /// Hostnames this server considers its own (the same allowlist the MCP
    /// DNS-rebinding check uses). Used to gate Host-derived issuer fallback.
    pub allowed_hosts: Arc<[String]>,
    /// Per-IP rate limiter for the unauthenticated /oauth/register endpoint.
    /// Prevents anyone from flooding the server with throwaway clients.
    pub register_limiter: Arc<RateLimiter>,
    /// Trusted reverse-proxy ranges parsed once at server startup.
    pub trusted_proxies: Arc<[crate::ratelimit::IpNetwork]>,
}

/// Resolve the issuer to advertise for this request (LIF-287).
///
/// When `server.public_url` is set, it always wins: metadata is static and
/// spoofed headers can't move the advertised endpoints. When it is unset, the
/// bind-derived issuer (e.g. `http://127.0.0.1:3456`) can mismatch the URL the
/// client actually dialed (`http://localhost:3456`), which fails the RFC 8707
/// audience check. In that case we derive the issuer from the request `Host`,
/// but ONLY when its hostname is in the same allowlist the MCP DNS-rebinding
/// check uses. Anything else falls back to the static issuer with a loud
/// warning, because an unrecognized Host means a proxied deployment that needs
/// `server.public_url` configured.
///
/// `X-Forwarded-Proto` / `X-Forwarded-Host` are deliberately ignored: these
/// are unauthenticated endpoints, and trusting forwarded headers here would
/// let any direct client control the advertised authorization/token endpoint
/// URLs (issuer spoofing).
fn effective_issuer(state: &OAuthState, headers: &HeaderMap) -> String {
    if state.issuer_is_explicit {
        return state.issuer.clone();
    }
    let Some(host_header) = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|h| !h.is_empty())
    else {
        return state.issuer.clone();
    };
    match host_header.parse::<axum::http::uri::Authority>() {
        Ok(authority) if authority.as_str() == host_header => {
            // Authority.host() keeps IPv6 brackets ("[::1]"); the allowlist
            // stores bare addresses ("::1").
            let host = authority.host().trim_start_matches('[').trim_end_matches(']');
            if state
                .allowed_hosts
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(host))
            {
                // Allowlisted hosts are loopback names (a proxy host only
                // enters the allowlist via public_url, which makes the issuer
                // explicit), so plain http matches what the client dialed.
                format!("http://{host_header}")
            } else {
                warn!(
                    host = %host_header,
                    issuer = %state.issuer,
                    "request Host does not match the advertised OAuth issuer; \
                     set server.public_url for proxied deployments"
                );
                state.issuer.clone()
            }
        }
        _ => state.issuer.clone(),
    }
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
        .route(
            "/oauth/device_authorization",
            post(device_authorization),
        )
        .route("/oauth/device", get(device_page).post(device_approve))
        .route("/oauth/token", post(token_exchange))
        .route("/oauth/revoke", post(revoke_token))
        // Claude.ai strips /oauth/ prefix (known bug anthropics/claude-ai-mcp#82)
        .route("/register", post(register_client))
        .route("/authorize", get(authorize_page).post(authorize_approve))
        .route("/device_authorization", post(device_authorization))
        .route("/device", get(device_page).post(device_approve))
        .route("/token", post(token_exchange))
        .route("/revoke", post(revoke_token))
        .with_state(state)
}

// ── Discovery ────────────────────────────────────────────────────────────

async fn protected_resource_metadata(
    State(state): State<OAuthState>,
    headers: HeaderMap,
) -> Json<serde_json::Value> {
    let issuer = effective_issuer(&state, &headers);
    // RFC 9728 / Claude connector requirement: the `resource` field MUST match
    // the MCP server URL the user enters in Claude *including the path component*
    // (`/mcp`). Claude derives the RFC 8707 audience from the URL it was given
    // (`https://host/mcp`) and rejects the issued token if the protected-resource
    // metadata advertises a different resource (e.g. the bare origin). Returning
    // the bare issuer here is what surfaced as "Authorization with the MCP server
    // failed" on claude.ai web even though the token exchange succeeded.
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
    let issuer = effective_issuer(&state, &headers);
    Json(serde_json::json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/oauth/authorize"),
        "token_endpoint": format!("{issuer}/oauth/token"),
        "registration_endpoint": format!("{issuer}/oauth/register"),
        "revocation_endpoint": format!("{issuer}/oauth/revoke"),
        "device_authorization_endpoint": format!("{issuer}/oauth/device_authorization"),
        "scopes_supported": ["mcp"],
        "response_types_supported": ["code"],
        "response_modes_supported": ["query"],
        "grant_types_supported": [
            "authorization_code",
            "urn:ietf:params:oauth:grant-type:device_code"
        ],
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
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<RegisterRequest>,
) -> Response {
    // ── Rate limit per source IP ──
    // /oauth/register is unauthenticated by spec (RFC 7591), so without this
    // anyone on the internet can mint unlimited clients.
    let ip = crate::ratelimit::client_ip(peer.ip(), &headers, &state.trusted_proxies);
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

// ── Device Authorization (RFC 8628) ──────────────────────────────────────

/// The RFC 8628 device-code grant type string.
const DEVICE_CODE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// Device code lifetime in seconds (RFC 8628 `expires_in`).
const DEVICE_CODE_EXPIRES_IN: u64 = 900;

/// Default minimum polling interval in seconds (RFC 8628 `interval`).
const DEVICE_CODE_INTERVAL: i64 = 5;

/// Unambiguous alphabet for the human-typed `user_code` — no vowels (avoids
/// spelling words), no 0/O/1/I/L-style confusables. 20 characters.
const USER_CODE_ALPHABET: &[u8] = b"BCDFGHJKLMNPQRSTVWXZ";

/// Generate an 8-character user code formatted `XXXX-XXXX`.
fn generate_user_code() -> String {
    let pick = |buf: &mut String| {
        for _ in 0..4 {
            let idx = (rand::random::<u8>() as usize) % USER_CODE_ALPHABET.len();
            buf.push(USER_CODE_ALPHABET[idx] as char);
        }
    };
    let mut out = String::with_capacity(9);
    pick(&mut out);
    out.push('-');
    pick(&mut out);
    out
}

/// Normalize a user code the human may have typed with lowercase letters,
/// spaces, or a missing dash: uppercase, strip everything but the alphabet,
/// then re-insert the dash after 4 chars. `bcdf ghjk` and `bcdfghjk` both
/// normalize to `BCDF-GHJK`.
fn normalize_user_code(input: &str) -> String {
    let cleaned: String = input
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if cleaned.len() == 8 {
        format!("{}-{}", &cleaned[..4], &cleaned[4..])
    } else {
        cleaned
    }
}

/// Best-effort cleanup of expired device codes. Called opportunistically on new
/// device_authorization requests so no background task is needed.
fn cleanup_expired_device_codes(db: &DbPool) {
    if let Ok(conn) = db.write() {
        let _ = conn.execute(
            "DELETE FROM oauth_device_codes WHERE expires_at <= datetime('now')",
            [],
        );
    }
}

#[derive(Deserialize)]
struct DeviceAuthRequest {
    #[serde(default)]
    client_name: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    scope: Option<String>,
}

/// `POST /oauth/device_authorization` (RFC 8628 §3.1/§3.2). Accepts form OR
/// JSON. Rate-limited per source IP like `/oauth/register`.
async fn device_authorization(
    State(state): State<OAuthState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    // ── Rate limit per source IP (reuse the register limiter) ──
    let ip = crate::ratelimit::client_ip(peer.ip(), &headers, &state.trusted_proxies);
    let key = format!("oauth_device_authorization:{ip}");
    if !state.register_limiter.check(&key) {
        let retry = state.register_limiter.retry_after(&key);
        warn!(ip = %ip, "oauth device authorization rate limited");
        let mut resp = (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "error": "too_many_requests",
                "error_description": format!("too many device authorization requests — try again in {retry} seconds")
            })),
        )
            .into_response();
        if let Ok(v) = retry.to_string().parse() {
            resp.headers_mut().insert("retry-after", v);
        }
        return resp;
    }

    // Parse client_name from either form-encoded or JSON body (both optional).
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let req: DeviceAuthRequest = if content_type.contains("application/json") {
        serde_json::from_slice(&body).unwrap_or(DeviceAuthRequest {
            client_name: None,
            scope: None,
        })
    } else {
        // application/x-www-form-urlencoded (default)
        serde_urlencoded::from_bytes(&body).unwrap_or(DeviceAuthRequest {
            client_name: None,
            scope: None,
        })
    };

    // Opportunistic housekeeping.
    cleanup_expired_device_codes(&state.db);

    // High-entropy device code — return raw once, store only its hash.
    let device_code = format!("{}{}", uuid_v4(), uuid_v4()).replace('-', "");
    let device_code_hash = hex_encode(&Sha256::digest(device_code.as_bytes()));

    // Generate a unique user code (retry a few times on the rare collision).
    let mut user_code = generate_user_code();
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(DEVICE_CODE_EXPIRES_IN as i64);

    let conn = match state.db.write() {
        Ok(c) => c,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response(),
    };
    let mut inserted = false;
    for _ in 0..5 {
        let res = conn.execute(
            "INSERT INTO oauth_device_codes
                (device_code_hash, user_code, client_name, expires_at, interval_seconds, status)
             VALUES (?1, ?2, ?3, ?4, ?5, 'pending')",
            params![
                device_code_hash,
                user_code,
                req.client_name,
                expires_at.to_rfc3339(),
                DEVICE_CODE_INTERVAL,
            ],
        );
        match res {
            Ok(_) => {
                inserted = true;
                break;
            }
            Err(_) => {
                // user_code UNIQUE collision — regenerate and retry.
                user_code = generate_user_code();
            }
        }
    }
    if !inserted {
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    let verification_uri = format!(
        "{}/oauth/device",
        effective_issuer(&state, &headers).trim_end_matches('/')
    );
    let verification_uri_complete = format!(
        "{verification_uri}?user_code={}",
        urlencoding::encode(&user_code)
    );

    info!(user_code = %user_code, "OAuth device authorization issued");

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "device_code": device_code,
            "user_code": user_code,
            "verification_uri": verification_uri,
            "verification_uri_complete": verification_uri_complete,
            "expires_in": DEVICE_CODE_EXPIRES_IN,
            "interval": DEVICE_CODE_INTERVAL,
        })),
    )
        .into_response()
}

#[derive(Deserialize)]
struct DevicePageQuery {
    #[serde(default)]
    user_code: Option<String>,
}

/// `GET /oauth/device` — server-rendered verification page. Mirrors
/// `authorize_page`'s style + CSRF pattern. The page is served regardless of
/// auth state (matching `authorize_page`, which also renders unauthenticated);
/// the POST handler is what enforces a valid session before approving.
async fn device_page(headers: HeaderMap, Query(q): Query<DevicePageQuery>) -> Html<String> {
    let csrf_token = generate_csrf_token(&session_credential(&headers));
    let prefill = q
        .user_code
        .as_deref()
        .map(normalize_user_code)
        .unwrap_or_default();
    Html(format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <title>Lific - Device Login</title>
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <style>
        body {{ font-family: system-ui, sans-serif; max-width: 400px; margin: 80px auto; padding: 0 20px; background: #0a0a0a; color: #e0e0e0; }}
        h1 {{ font-size: 1.4em; margin-bottom: 0.5em; }}
        p {{ color: #888; line-height: 1.5; }}
        label {{ display: block; margin-top: 1.5em; color: #aaa; font-size: 0.9em; }}
        input[type=text] {{ width: 100%; box-sizing: border-box; margin-top: 0.4em; padding: 12px; border-radius: 6px; border: 1px solid #333; background: #111; color: #fff; font-size: 1.2em; letter-spacing: 0.15em; text-align: center; text-transform: uppercase; }}
        .buttons {{ display: flex; gap: 12px; margin-top: 2em; }}
        button {{ flex: 1; color: white; border: none; padding: 12px 24px; border-radius: 6px; font-size: 1em; cursor: pointer; }}
        button.approve {{ background: #2563eb; }}
        button.approve:hover {{ background: #1d4ed8; }}
        button.deny {{ background: #444; }}
        button.deny:hover {{ background: #555; }}
    </style>
</head>
<body>
    <h1>Connect a device to Lific</h1>
    <p>Enter the code shown on the device or terminal that's signing in, then approve.</p>
    <form method="POST" action="/oauth/device">
        <label for="user_code">Device code</label>
        <input type="text" id="user_code" name="user_code" value="{user_code}" autocomplete="off" autocapitalize="characters" spellcheck="false" required>
        <input type="hidden" name="csrf_token" value="{csrf_token}">
        <div class="buttons">
            <button type="submit" name="decision" value="approve" class="approve">Approve</button>
            <button type="submit" name="decision" value="deny" class="deny">Deny</button>
        </div>
    </form>
</body>
</html>"#,
        user_code = html_escape(&prefill),
        csrf_token = html_escape(&csrf_token),
    ))
}

#[derive(Deserialize)]
struct DeviceApproveForm {
    user_code: String,
    decision: Option<String>,
    csrf_token: Option<String>,
}

/// `POST /oauth/device` — validate CSRF + session, then mark the device code
/// approved (binding the approving user) or denied.
async fn device_approve(
    State(oauth): State<OAuthState>,
    headers: HeaderMap,
    axum::Form(form): axum::Form<DeviceApproveForm>,
) -> Response {
    let credential = session_credential(&headers);

    // CSRF, bound to the presenting session (identical policy to authorize).
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

    // The approver must be signed in.
    let Some(token) = (!credential.is_empty()).then_some(credential) else {
        return (
            StatusCode::UNAUTHORIZED,
            Html("<h1>Authentication required</h1><p>You must be signed in to approve a device. <a href=\"/#/login\">Sign in</a></p>".to_string()),
        )
            .into_response();
    };

    let auth_outcome: Option<Option<i64>> = if token.starts_with("lific_sess_") {
        let conn = match oauth.db.write() {
            Ok(c) => c,
            Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response(),
        };
        crate::db::queries::users::validate_session(&conn, &token)
            .ok()
            .map(|u| Some(u.id))
    } else if token.starts_with("lific_at_") {
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

    let normalized = normalize_user_code(&form.user_code);
    let deny = form.decision.as_deref() == Some("deny");

    let conn = match oauth.db.write() {
        Ok(c) => c,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response(),
    };

    // Only pending, unexpired codes can be acted on.
    let new_status = if deny { "denied" } else { "approved" };
    let updated = conn
        .execute(
            "UPDATE oauth_device_codes
             SET status = ?1, user_id = ?2
             WHERE user_code = ?3 AND status = 'pending' AND expires_at > datetime('now')",
            params![new_status, approving_user_id, normalized],
        )
        .unwrap_or(0);

    if updated == 0 {
        return (
            StatusCode::BAD_REQUEST,
            Html(format!(
                "<h1>Unknown or expired code</h1><p>The code <code>{}</code> was not found, has expired, or was already used. <a href=\"/oauth/device\">Try again</a></p>",
                html_escape(&normalized)
            )),
        )
            .into_response();
    }

    info!(user_code = %normalized, decision = %new_status, "OAuth device verification");

    if deny {
        (
            StatusCode::OK,
            Html("<h1>Access denied</h1><p>The device will not be connected. You can close this page.</p>".to_string()),
        )
            .into_response()
    } else {
        (
            StatusCode::OK,
            Html("<h1>Device approved</h1><p>You're all set. Return to the device or terminal — it will finish signing in automatically.</p>".to_string()),
        )
            .into_response()
    }
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
    /// RFC 8628 device grant: the opaque device_code returned by
    /// /oauth/device_authorization.
    device_code: Option<String>,
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
    if req.grant_type == DEVICE_CODE_GRANT {
        return device_token_exchange(&state, &req);
    }
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

/// RFC 8628 §3.4/§3.5 device-code token exchange. Looks up the device code by
/// hash, enforces the polling interval (`slow_down`), and returns the
/// per-status error (`authorization_pending` / `access_denied` /
/// `expired_token`) or, on approval, mints and returns an access token.
fn device_token_exchange(state: &OAuthState, req: &TokenRequest) -> Response {
    let Some(device_code) = req.device_code.as_deref().filter(|c| !c.is_empty()) else {
        return device_error(StatusCode::BAD_REQUEST, "invalid_request", Some("missing device_code"));
    };
    let device_code_hash = hex_encode(&Sha256::digest(device_code.as_bytes()));

    let conn = match state.db.write() {
        Ok(c) => c,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response(),
    };

    struct DeviceRow {
        status: String,
        user_id: Option<i64>,
        expires_at: String,
        interval_seconds: i64,
        last_polled_at: Option<String>,
    }

    let row: Result<DeviceRow, _> = conn.query_row(
        "SELECT status, user_id, expires_at, interval_seconds, last_polled_at
         FROM oauth_device_codes WHERE device_code_hash = ?1",
        params![device_code_hash],
        |r| {
            Ok(DeviceRow {
                status: r.get(0)?,
                user_id: r.get(1)?,
                expires_at: r.get(2)?,
                interval_seconds: r.get(3)?,
                last_polled_at: r.get(4)?,
            })
        },
    );

    let row = match row {
        Ok(r) => r,
        // Unknown device_code → invalid_grant per RFC 8628 §3.5.
        Err(_) => return device_error(StatusCode::BAD_REQUEST, "invalid_grant", None),
    };

    let now = chrono::Utc::now();

    // Expiry check first (RFC 8628: expired_token).
    let expired = chrono::DateTime::parse_from_rfc3339(&row.expires_at)
        .map(|t| now >= t.with_timezone(&chrono::Utc))
        .unwrap_or(true);
    if expired {
        let _ = conn.execute(
            "DELETE FROM oauth_device_codes WHERE device_code_hash = ?1",
            params![device_code_hash],
        );
        return device_error(StatusCode::BAD_REQUEST, "expired_token", None);
    }

    // slow_down: reject if polled faster than `interval` since the last poll.
    if let Some(last) = &row.last_polled_at
        && let Ok(last_t) = chrono::DateTime::parse_from_rfc3339(last)
    {
        let elapsed = now
            .signed_duration_since(last_t.with_timezone(&chrono::Utc))
            .num_seconds();
        if elapsed < row.interval_seconds {
            // Do NOT update last_polled_at here — an early poll shouldn't push
            // the window out; the client is told to slow down.
            return device_error(StatusCode::BAD_REQUEST, "slow_down", None);
        }
    }

    // Record this poll time (used for the next slow_down check).
    let _ = conn.execute(
        "UPDATE oauth_device_codes SET last_polled_at = ?1 WHERE device_code_hash = ?2",
        params![now.to_rfc3339(), device_code_hash],
    );

    match row.status.as_str() {
        "pending" => device_error(StatusCode::BAD_REQUEST, "authorization_pending", None),
        "denied" => device_error(StatusCode::BAD_REQUEST, "access_denied", None),
        "consumed" => device_error(StatusCode::BAD_REQUEST, "invalid_grant", Some("device code already used")),
        "approved" => {
            // Mint the access token bound to the approving user, then mark the
            // code consumed (single use).
            let scope = "mcp";
            let client_id = "device";
            // Ensure a client row exists so the FK on oauth_tokens is satisfied.
            let _ = conn.execute(
                "INSERT OR IGNORE INTO oauth_clients (client_id, client_name, redirect_uris)
                 VALUES ('device', 'Device Authorization', '[]')",
                [],
            );

            let access_token = format!("lific_at_{}", uuid_v4());
            let token_hash = hex_encode(&Sha256::digest(access_token.as_bytes()));
            let expires_in: u64 = 3600 * 24 * 30; // 30 days
            let expires_at = now + chrono::Duration::seconds(expires_in as i64);

            if let Err(e) = conn.execute(
                "INSERT INTO oauth_tokens (access_token, client_id, expires_at, scope, user_id)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![token_hash, client_id, expires_at.to_rfc3339(), scope, row.user_id],
            ) {
                tracing::error!(error = %e, "failed to store device OAuth token");
                return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
            }

            // Single-use: mark consumed so a replay returns invalid_grant.
            let _ = conn.execute(
                "UPDATE oauth_device_codes SET status = 'consumed' WHERE device_code_hash = ?1",
                params![device_code_hash],
            );

            info!(scope = %scope, "OAuth device token issued");
            Json(TokenResponse {
                access_token,
                token_type: "Bearer".into(),
                expires_in,
                scope: scope.into(),
            })
            .into_response()
        }
        _ => device_error(StatusCode::BAD_REQUEST, "invalid_grant", None),
    }
}

/// Build an RFC 8628 §3.5 JSON error response body.
fn device_error(status: StatusCode, error: &str, description: Option<&str>) -> Response {
    let body = match description {
        Some(d) => serde_json::json!({"error": error, "error_description": d}),
        None => serde_json::json!({"error": error}),
    };
    (status, Json(body)).into_response()
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
    use axum::extract::connect_info::MockConnectInfo;
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
            issuer_is_explicit: true,
            allowed_hosts: test_allowed_hosts(),
            register_limiter: Arc::new(RateLimiter::new(cap, std::time::Duration::from_secs(3600))),
            trusted_proxies: default_trusted_proxies(),
        };
        (
            router(state).layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242)))),
            db,
        )
    }

    fn test_allowed_hosts() -> Arc<[String]> {
        vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
            "::1".to_string(),
        ]
        .into()
    }

    fn default_trusted_proxies() -> Arc<[crate::ratelimit::IpNetwork]> {
        Arc::<[crate::ratelimit::IpNetwork]>::from(
            crate::config::ServerConfig::default()
                .trusted_proxy_ranges()
                .expect("default trusted proxy ranges must parse"),
        )
    }

    /// Build a test OAuth router the way `lific start` does when
    /// `server.public_url` is UNSET: bind-derived issuer, not explicit.
    /// Exercises the LIF-287 Host-derived issuer fallback.
    fn test_oauth_app_implicit_issuer() -> Router {
        let db = crate::db::open_memory().expect("test db");
        let state = OAuthState {
            db,
            issuer: "http://127.0.0.1:3456".into(),
            issuer_is_explicit: false,
            allowed_hosts: test_allowed_hosts(),
            register_limiter: Arc::new(RateLimiter::new(
                1000,
                std::time::Duration::from_secs(3600),
            )),
            trusted_proxies: default_trusted_proxies(),
        };
        router(state).layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))))
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

    // ── LIF-287: Host-derived issuer fallback ────────────────
    // When public_url is unset the advertised issuer may be replaced by the
    // request Host, but only for allowlisted (loopback) hosts. An explicit
    // public_url is never overridden, and forwarded headers are ignored.

    /// GET a metadata path and parse the JSON body.
    async fn get_metadata(
        app: &Router,
        path: &str,
        headers: &[(&str, &str)],
    ) -> serde_json::Value {
        let mut builder = Request::builder().uri(path);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let resp = app
            .clone()
            .oneshot(builder.body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "path {path}");
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn metadata_derives_issuer_from_allowlisted_host() {
        let app = test_oauth_app_implicit_issuer();

        let val = get_metadata(
            &app,
            "/.well-known/oauth-authorization-server",
            &[("host", "localhost:3456")],
        )
        .await;
        assert_eq!(val["issuer"], "http://localhost:3456");
        assert_eq!(
            val["token_endpoint"],
            "http://localhost:3456/oauth/token"
        );

        let val = get_metadata(
            &app,
            "/.well-known/oauth-protected-resource",
            &[("host", "localhost:3456")],
        )
        .await;
        assert_eq!(val["resource"], "http://localhost:3456/mcp");
        assert_eq!(val["authorization_servers"][0], "http://localhost:3456");
    }

    #[tokio::test]
    async fn metadata_derives_issuer_from_ipv6_loopback_host() {
        let app = test_oauth_app_implicit_issuer();
        let val = get_metadata(
            &app,
            "/.well-known/oauth-authorization-server",
            &[("host", "[::1]:3456")],
        )
        .await;
        assert_eq!(val["issuer"], "http://[::1]:3456");
    }

    #[tokio::test]
    async fn metadata_falls_back_to_static_issuer_for_unallowlisted_host() {
        let app = test_oauth_app_implicit_issuer();
        let val = get_metadata(
            &app,
            "/.well-known/oauth-authorization-server",
            &[("host", "evil.example.com")],
        )
        .await;
        assert_eq!(
            val["issuer"], "http://127.0.0.1:3456",
            "unallowlisted Host must not control the advertised issuer"
        );

        let val = get_metadata(
            &app,
            "/.well-known/oauth-protected-resource",
            &[("host", "evil.example.com")],
        )
        .await;
        assert_eq!(val["resource"], "http://127.0.0.1:3456/mcp");
    }

    #[tokio::test]
    async fn metadata_ignores_forwarded_headers() {
        let app = test_oauth_app_implicit_issuer();
        let val = get_metadata(
            &app,
            "/.well-known/oauth-authorization-server",
            &[
                ("host", "localhost:3456"),
                ("x-forwarded-host", "evil.example.com"),
                ("x-forwarded-proto", "https"),
            ],
        )
        .await;
        assert_eq!(
            val["issuer"], "http://localhost:3456",
            "X-Forwarded-* must never influence the advertised issuer"
        );
    }

    #[tokio::test]
    async fn explicit_public_url_issuer_is_never_overridden_by_host() {
        // test_oauth_app() marks the issuer explicit (public_url set).
        let (app, _) = test_oauth_app();
        for host in ["localhost:3456", "evil.example.com"] {
            let val = get_metadata(
                &app,
                "/.well-known/oauth-authorization-server",
                &[("host", host)],
            )
            .await;
            assert_eq!(val["issuer"], "https://example.com", "host {host}");
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

    // ── LIF-252: device authorization flow (RFC 8628) ────────────────────

    /// POST /oauth/device_authorization and return the parsed JSON.
    async fn request_device_code(app: &Router, client_name: Option<&str>) -> serde_json::Value {
        let body = match client_name {
            Some(n) => format!("client_name={}", urlencoding::encode(n)),
            None => String::new(),
        };
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/device_authorization")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// POST the device grant to /oauth/token and return (status, json).
    async fn poll_device_token(
        app: &Router,
        device_code: &str,
    ) -> (StatusCode, serde_json::Value) {
        let body = format!(
            "grant_type={}&device_code={}",
            urlencoding::encode("urn:ietf:params:oauth:grant-type:device_code"),
            urlencoding::encode(device_code),
        );
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/token")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let val = serde_json::from_slice(&bytes).unwrap_or(serde_json::json!({}));
        (status, val)
    }

    #[tokio::test]
    async fn device_authorization_returns_wellformed_response() {
        let (app, _db) = test_oauth_app();
        let v = request_device_code(&app, Some("My CLI")).await;
        assert!(v["device_code"].as_str().is_some());
        let user_code = v["user_code"].as_str().unwrap();
        // Format XXXX-XXXX from the unambiguous alphabet.
        assert_eq!(user_code.len(), 9);
        assert_eq!(&user_code[4..5], "-");
        for c in user_code.chars().filter(|c| *c != '-') {
            assert!(
                USER_CODE_ALPHABET.contains(&(c as u8)),
                "user_code char {c} not in alphabet"
            );
        }
        assert_eq!(v["expires_in"], 900);
        assert_eq!(v["interval"], 5);
        let vuri = v["verification_uri"].as_str().unwrap();
        assert!(vuri.ends_with("/oauth/device"));
        let vuc = v["verification_uri_complete"].as_str().unwrap();
        assert!(vuc.contains("user_code="));
    }

    #[tokio::test]
    async fn device_code_stored_only_as_hash() {
        let (app, db) = test_oauth_app();
        let v = request_device_code(&app, None).await;
        let device_code = v["device_code"].as_str().unwrap();
        let hash = hex_encode(&Sha256::digest(device_code.as_bytes()));
        let conn = db.read().unwrap();
        // The raw code must NOT be in the table; only its hash.
        let by_hash: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM oauth_device_codes WHERE device_code_hash = ?1",
                params![hash],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(by_hash, 1);
        let by_raw: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM oauth_device_codes WHERE device_code_hash = ?1",
                params![device_code],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(by_raw, 0, "raw device_code must not be stored");
    }

    #[tokio::test]
    async fn device_metadata_advertises_endpoint_and_grant() {
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
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            v["device_authorization_endpoint"]
                .as_str()
                .unwrap()
                .ends_with("/oauth/device_authorization")
        );
        let grants = v["grant_types_supported"].as_array().unwrap();
        assert!(
            grants
                .iter()
                .any(|g| g == "urn:ietf:params:oauth:grant-type:device_code"),
            "metadata must advertise the device grant"
        );
    }

    #[tokio::test]
    async fn device_polling_pending_then_approved_end_to_end() {
        let (app, db) = test_oauth_app();
        let session_token = create_test_session(&db); // user "oauthtest"
        let user_id: i64 = {
            let conn = db.read().unwrap();
            conn.query_row(
                "SELECT id FROM users WHERE username = 'oauthtest'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };

        let v = request_device_code(&app, Some("laptop")).await;
        let device_code = v["device_code"].as_str().unwrap().to_string();
        let user_code = v["user_code"].as_str().unwrap().to_string();

        let device_hash = hex_encode(&Sha256::digest(device_code.as_bytes()));

        // First poll: pending.
        let (status, body) = poll_device_token(&app, &device_code).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "authorization_pending");

        // Simulate the client having waited the interval before its next poll,
        // so the slow_down guard doesn't fire (this test drives polls
        // back-to-back with no real delay).
        let reset_last_poll = |db: &DbPool| {
            let conn = db.write().unwrap();
            conn.execute(
                "UPDATE oauth_device_codes SET last_polled_at = NULL WHERE device_code_hash = ?1",
                params![device_hash],
            )
            .unwrap();
        };

        // Approve via the verification page (signed-in session, CSRF-bound).
        let csrf = generate_csrf_token(&session_token);
        let approve_body = format!(
            "user_code={}&decision=approve&csrf_token={}",
            urlencoding::encode(&user_code),
            urlencoding::encode(&csrf),
        );
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/device")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("cookie", format!("lific_token={session_token}"))
                    .body(axum::body::Body::from(approve_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "approval should succeed");

        // The device row now binds the approver.
        {
            let conn = db.read().unwrap();
            let (st, uid): (String, Option<i64>) = conn
                .query_row(
                    "SELECT status, user_id FROM oauth_device_codes WHERE user_code = ?1",
                    params![user_code],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(st, "approved");
            assert_eq!(uid, Some(user_id));
        }

        // Next poll: approved → returns a token bound to the approver.
        reset_last_poll(&db);
        let (status, body) = poll_device_token(&app, &device_code).await;
        assert_eq!(status, StatusCode::OK, "expected token, got {body}");
        let access_token = body["access_token"].as_str().unwrap();
        assert!(access_token.starts_with("lific_at_"));
        assert_eq!(oauth_token_user_id(&db, access_token), Some(user_id));

        // Single-use: a replay poll now fails (consumed → invalid_grant).
        reset_last_poll(&db);
        let (status, body) = poll_device_token(&app, &device_code).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid_grant");
    }

    #[tokio::test]
    async fn device_polling_slow_down_when_too_fast() {
        let (app, _db) = test_oauth_app();
        let v = request_device_code(&app, None).await;
        let device_code = v["device_code"].as_str().unwrap().to_string();

        // First poll registers last_polled_at (pending).
        let (_, body) = poll_device_token(&app, &device_code).await;
        assert_eq!(body["error"], "authorization_pending");

        // Immediate second poll (< interval seconds) → slow_down.
        let (status, body) = poll_device_token(&app, &device_code).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "slow_down");
    }

    #[tokio::test]
    async fn device_expired_token_after_expiry() {
        let (app, db) = test_oauth_app();
        let v = request_device_code(&app, None).await;
        let device_code = v["device_code"].as_str().unwrap().to_string();
        let hash = hex_encode(&Sha256::digest(device_code.as_bytes()));

        // Force expiry by rewriting expires_at into the past.
        {
            let conn = db.write().unwrap();
            let past = (chrono::Utc::now() - chrono::Duration::minutes(1)).to_rfc3339();
            conn.execute(
                "UPDATE oauth_device_codes SET expires_at = ?1 WHERE device_code_hash = ?2",
                params![past, hash],
            )
            .unwrap();
        }

        let (status, body) = poll_device_token(&app, &device_code).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "expired_token");
    }

    #[tokio::test]
    async fn device_denied_path() {
        let (app, db) = test_oauth_app();
        let session_token = create_test_session(&db);
        let v = request_device_code(&app, None).await;
        let device_code = v["device_code"].as_str().unwrap().to_string();
        let user_code = v["user_code"].as_str().unwrap().to_string();

        let csrf = generate_csrf_token(&session_token);
        let deny_body = format!(
            "user_code={}&decision=deny&csrf_token={}",
            urlencoding::encode(&user_code),
            urlencoding::encode(&csrf),
        );
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/device")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("cookie", format!("lific_token={session_token}"))
                    .body(axum::body::Body::from(deny_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let (status, body) = poll_device_token(&app, &device_code).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "access_denied");
    }

    #[tokio::test]
    async fn device_verification_requires_login() {
        let (app, _db) = test_oauth_app();
        let v = request_device_code(&app, None).await;
        let user_code = v["user_code"].as_str().unwrap().to_string();

        // CSRF bound to the empty (unauthenticated) session so we get past the
        // CSRF gate and exercise the auth-required branch.
        let csrf = generate_csrf_token("");
        let body = format!(
            "user_code={}&decision=approve&csrf_token={}",
            urlencoding::encode(&user_code),
            urlencoding::encode(&csrf),
        );
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/device")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn device_approve_rejects_unbound_csrf() {
        // A CSRF minted for no session must not approve with a victim cookie.
        let (app, db) = test_oauth_app();
        let session_token = create_test_session(&db);
        let v = request_device_code(&app, None).await;
        let user_code = v["user_code"].as_str().unwrap().to_string();

        let csrf = generate_csrf_token(""); // unbound
        let body = format!(
            "user_code={}&decision=approve&csrf_token={}",
            urlencoding::encode(&user_code),
            urlencoding::encode(&csrf),
        );
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/device")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("cookie", format!("lific_token={session_token}"))
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn device_invalid_user_code_returns_error_page() {
        let (app, db) = test_oauth_app();
        let session_token = create_test_session(&db);
        let csrf = generate_csrf_token(&session_token);
        let body = format!(
            "user_code={}&decision=approve&csrf_token={}",
            "ZZZZ-ZZZZ",
            urlencoding::encode(&csrf),
        );
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/device")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("cookie", format!("lific_token={session_token}"))
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn device_unknown_device_code_is_invalid_grant() {
        let (app, _db) = test_oauth_app();
        let (status, body) = poll_device_token(&app, "totally-unknown-device-code").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid_grant");
    }

    #[tokio::test]
    async fn device_page_prefills_user_code_from_query() {
        let (app, _db) = test_oauth_app();
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/oauth/device?user_code=bcdfghjk")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let html = String::from_utf8_lossy(&bytes);
        // Normalized + uppercased + dash-inserted into the input value.
        assert!(html.contains("value=\"BCDF-GHJK\""), "prefill missing: {html}");
    }

    #[test]
    fn normalize_user_code_handles_spacing_and_case() {
        assert_eq!(normalize_user_code("bcdf-ghjk"), "BCDF-GHJK");
        assert_eq!(normalize_user_code("bcdf ghjk"), "BCDF-GHJK");
        assert_eq!(normalize_user_code("BCDFGHJK"), "BCDF-GHJK");
        assert_eq!(normalize_user_code("  bcdfghjk  "), "BCDF-GHJK");
    }

    #[test]
    fn generate_user_code_is_wellformed() {
        for _ in 0..50 {
            let c = generate_user_code();
            assert_eq!(c.len(), 9);
            assert_eq!(&c[4..5], "-");
            for ch in c.chars().filter(|c| *c != '-') {
                assert!(USER_CODE_ALPHABET.contains(&(ch as u8)));
            }
        }
    }
}
