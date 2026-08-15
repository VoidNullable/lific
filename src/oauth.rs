use std::{net::SocketAddr, sync::Arc};

use axum::{
    Router,
    extract::{ConnectInfo, Json, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use hmac::{Hmac, Mac};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::db::models::AuthUser;
use tracing::{info, warn};

use crate::auth::{hex_encode, sha256_hex};
use crate::db::DbPool;
use crate::error::LificError;
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
    match crate::links::parse_http_authority(host_header) {
        Some(authority)
            if crate::links::authority_is_allowlisted(&authority, &state.allowed_hosts) =>
        {
            // Allowlisted hosts are loopback names (a proxy host only enters
            // the allowlist via public_url, which makes the issuer explicit),
            // so plain http matches what the client dialed.
            format!("http://{authority}")
        }
        Some(_) => {
            warn!(
                host = %host_header,
                issuer = %state.issuer,
                "request Host does not match the advertised OAuth issuer; \
                 set server.public_url for proxied deployments"
            );
            state.issuer.clone()
        }
        None => state.issuer.clone(),
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
        // LIF-415: `none` only. Lific issues no client secrets (registration
        // returns a client_id and nothing else) and the token endpoint never
        // looks for one, so advertising `client_secret_post` described an
        // authentication method that does not exist here. A client that took
        // the metadata at its word and sent a secret would have it silently
        // ignored, which reads as "authenticated" when it isn't.
        "token_endpoint_auth_methods_supported": ["none"],
        "code_challenge_methods_supported": ["S256"]
    }))
}

// ── Dynamic Client Registration ──────────────────────────────────────────

#[derive(Deserialize)]
struct RegisterRequest {
    redirect_uris: Vec<String>,
    client_name: Option<String>,
    // LIF-415: a submitted `token_endpoint_auth_method` is deliberately not
    // captured. Unknown fields are ignored by serde, and the registration
    // response always reports `none` because that is the only method this
    // server implements.
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
                "error_description": crate::ratelimit::retry_after_message(
                    "too many client registrations",
                    retry,
                )
            })),
        )
            .into_response();
        if retry > 0 && let Ok(v) = retry.to_string().parse() {
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
            // LIF-415: RFC 7591 §3.2.1 — the response states the metadata the
            // server actually registered, not the client's wish. Every client
            // here is public: no secret is issued, so echoing back a requested
            // `client_secret_post` would claim a registration we did not make.
            "token_endpoint_auth_method": "none",
            "grant_types": req.grant_types.unwrap_or_else(|| vec!["authorization_code".into()]),
            "response_types": req.response_types.unwrap_or_else(|| vec!["code".into()])
        })),
    )
        .into_response()
}

/// LIFIC-15: read the tool a registered client has been mapped to, if any.
///
/// Remembering the tool per client means a reconnect pre-fills (or skips) the
/// approval pick-list instead of re-asking — the choice is a stable attribute
/// of the persistent DCR client, not re-derived on every visit. Returns `None`
/// for clients that have never been approved. (Writing happens inside
/// [`resolve_approval_bot`], which owns the same DB handle.)
fn client_tool_id(db: &DbPool, client_id: &str) -> Option<String> {
    let conn = db.read().ok()?;
    conn.query_row(
        "SELECT tool_id FROM oauth_clients WHERE client_id = ?1",
        params![client_id],
        |row| row.get(0),
    )
    .ok()
    .flatten()
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

/// Validate the authorization request shape before rendering consent or
/// issuing an authorization code. Lific supports one capability and requires
/// PKCE for every authorization-code flow.
fn valid_authorize_request(
    response_type: &str,
    scope: Option<&str>,
    code_challenge: Option<&str>,
    code_challenge_method: Option<&str>,
) -> bool {
    response_type == "code"
        && scope == Some("mcp")
        && code_challenge.is_some_and(|challenge| !challenge.is_empty())
        && code_challenge_method == Some("S256")
}

async fn authorize_page(
    State(oauth): State<OAuthState>,
    headers: HeaderMap,
    Query(params): Query<AuthorizeParams>,
) -> Response {
    let requested_scope = params.scope.as_deref().unwrap_or_default();
    if !valid_authorize_request(
        &params.response_type,
        params.scope.as_deref(),
        params.code_challenge.as_deref(),
        params.code_challenge_method.as_deref(),
    ) {
        return (
            StatusCode::BAD_REQUEST,
            Html("<h1>Unsupported OAuth request</h1><p>Only authorization-code access to the MCP capability is supported.</p>".to_string()),
        )
            .into_response();
    }

    // Resolve the registered client before showing consent. A generic
    // "application" prompt trains users to approve phishing clients and gives
    // no meaningful capability disclosure. The redirect URI is checked here
    // as well as on POST so a crafted GET cannot produce a misleading screen.
    let client_name = oauth
        .db
        .read()
        .ok()
        .and_then(|conn| {
            conn.query_row(
                "SELECT client_name, redirect_uris FROM oauth_clients WHERE client_id = ?1",
                params![params.client_id],
                |row| {
                    let name: String = row.get(0)?;
                    let uris_json: String = row.get(1)?;
                    Ok((name, uris_json))
                },
            )
            .ok()
        })
        .and_then(|(name, uris_json)| {
            let uris: Vec<String> = serde_json::from_str(&uris_json).ok()?;
            uris.iter()
                .any(|uri| uri == &params.redirect_uri)
                .then_some(name)
        });
    let Some(client_name) = client_name else {
        return (
            StatusCode::BAD_REQUEST,
            Html("<h1>Invalid OAuth client</h1><p>The client or redirect URI is not registered.</p>".to_string()),
        )
            .into_response();
    };

    // Bind the CSRF token to the session the browser presents when loading this
    // page (sent on the top-level GET navigation under SameSite=Lax). The POST
    // approval must carry the same session for the token to validate.
    let csrf_token = generate_csrf_token(&session_credential(&headers));
    // LIFIC-13: the approval screen asks which tool is connecting so the audit
    // log can attribute requests to a per-tool bot. Options come from the same
    // Connected Tools registry `lific connect` uses; a free-text field covers
    // unrecognized tools. LIFIC-15: if this client is already remembered,
    // pre-select that tool instead of re-asking on a reconnect.
    let preset_id = client_tool_id(&oauth.db, &params.client_id);
    let tool_pick_list = tool_pick_list_html(preset_id.as_deref());

    (
        StatusCode::OK,
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
        .destination {{ color: #ddd; overflow-wrap: anywhere; }}
        label {{ display: block; margin-top: 1em; color: #aaa; font-size: 0.9em; }}
        select, input {{ width: 100%%; padding: 10px; margin-top: 4px; border-radius: 6px; border: 1px solid #333; background: #141414; color: #e0e0e0; box-sizing: border-box; }}
        form {{ margin-top: 2em; }}
        button {{ background: #2563eb; color: white; border: none; padding: 12px 32px; border-radius: 6px; font-size: 1em; cursor: pointer; width: 100%; margin-top: 1.5em; }}
        button:hover {{ background: #1d4ed8; }}
    </style>
</head>
<body>
    <h1>Authorize access to Lific</h1>
    <p><span class="client">{client_name}</span> wants access to your Lific issue tracker.</p>
    <p>Capability requested: <span class="client">MCP issue-tracker access</span>.</p>
    <p>After approval, you will be redirected to:<br><span class="destination">{redirect_uri}</span></p>
    <form method="POST" action="/oauth/authorize">
        <input type="hidden" name="client_id" value="{client_id}">
        <input type="hidden" name="redirect_uri" value="{redirect_uri}">
        <input type="hidden" name="response_type" value="{response_type}">
        <input type="hidden" name="state" value="{state}">
        <input type="hidden" name="code_challenge" value="{code_challenge}">
        <input type="hidden" name="code_challenge_method" value="{code_challenge_method}">
        <input type="hidden" name="scope" value="{scope}">
        <input type="hidden" name="csrf_token" value="{csrf_token}">
        {tool_pick_list}
        <button type="submit" name="decision" value="approve">Approve</button>
        <button type="submit" name="decision" value="deny" style="background:#444">Deny</button>
    </form>
</body>
</html>"#,
        client_id = html_escape(&params.client_id),
        client_name = html_escape(&client_name),
        redirect_uri = html_escape(&params.redirect_uri),
        response_type = html_escape(&params.response_type),
        state = html_escape(params.state.as_deref().unwrap_or("")),
        code_challenge = html_escape(params.code_challenge.as_deref().unwrap_or("")),
        code_challenge_method =
            html_escape(params.code_challenge_method.as_deref().unwrap_or("S256")),
        scope = html_escape(requested_scope),
        csrf_token = html_escape(&csrf_token),
        tool_pick_list = tool_pick_list,
        )),
    )
        .into_response()
}

#[derive(Deserialize)]
struct ApproveForm {
    client_id: String,
    redirect_uri: String,
    /// Round-tripped from the authorize form so the POST body stays a valid
    /// OAuth request, but the value is fixed at `code` and never branched on.
    #[allow(dead_code)]
    response_type: String,
    state: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    scope: Option<String>,
    csrf_token: Option<String>,
    /// LIFIC-13: which tool is connecting — a Connected Tools registry id, or
    /// empty meaning `tool_custom` holds a free-text name.
    tool: Option<String>,
    /// Free-text tool name when `tool` is unset (an unrecognized tool).
    tool_custom: Option<String>,
    decision: Option<String>,
}

/// The one page both approval handlers render when the presented credential is
/// not a live browser session: absent, wrong shape, expired, or revoked out
/// from under the form between rendering and submitting. Deliberately one
/// message for all of those.
fn invalid_session_page() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Html("<h1>Invalid session</h1><p>Your session has expired or is invalid. <a href=\"/#/login\">Sign in again</a></p>".to_string()),
    )
        .into_response()
}

/// The page shown when the session is real but was not authenticated recently
/// enough to hand out a durable credential.
///
/// Distinct copy from [`invalid_session_page`] because the fix is different:
/// nothing is wrong with the session, it is simply older than the window.
/// Telling them "expired or invalid" would send them looking for a problem that
/// is not there.
///
/// The copy is deliberately literal about what has to happen, and deliberately
/// does not promise a retry. This request is a form POST carrying a PKCE
/// challenge, a redirect URI and a CSRF token bound to the presenting session.
/// Replaying it after a fresh sign-in would need a server-side continuation
/// store keyed on something the browser can carry back, which is a real feature
/// with its own security surface, not a redirect. It also does not send them to
/// `/#/login`, because a stored session that is merely old still satisfies the
/// login screen: it would show them the app, they would come back, and it would
/// be just as stale. Signing **out** is what actually clears it.
fn stale_session_page() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Html(
            "<h1>Your sign-in is too old to connect a tool</h1>\
             <p>Connecting a tool gives it lasting access to your account, so Lific requires a \
             sign-in from the last 15 minutes. Yours is older than that. Nothing has been \
             connected and nothing has changed.</p>\
             <p>To continue:</p>\
             <ol>\
             <li>Open Lific and <strong>sign out</strong>. Simply reloading or revisiting the \
             login page will not help, because your existing sign-in is still valid, just old.</li>\
             <li>Sign back in. (On an instance that signs in without a password, signing out and \
             reloading is enough.)</li>\
             <li>Start the connection again from your MCP client. This page cannot resume it for \
             you.</li>\
             </ol>\
             <p><a href=\"/#/settings\">Open Lific settings</a></p>"
                .to_string(),
        ),
    )
        .into_response()
}

/// Establish, on `conn`, that `token` is a live browser session authenticated
/// inside the recent-authentication window, and return whose it is.
///
/// Approving an OAuth grant mints a 30-day credential for a tool, which is the
/// same authority as minting an API key, so it carries the same 15-minute rule
/// that `POST /api/auth/keys` does. Without it a session token stolen from a
/// browser that was signed in last week is enough to attach a permanent tool
/// credential to the account.
///
/// Callers run this as the first statement of the write transaction that
/// resolves the bot and stores the grant, so a lockdown cannot land between
/// the check and the write.
fn recent_approver(
    conn: &rusqlite::Connection,
    token: &str,
) -> Result<crate::db::models::User, ApprovalRefusal> {
    let user = crate::db::queries::users::validate_session(conn, token)
        .map_err(|_| ApprovalRefusal::InvalidSession)?;
    match crate::db::queries::users::session_is_recent(conn, token) {
        Ok(true) => Ok(user),
        Ok(false) => Err(ApprovalRefusal::StaleSession),
        Err(e) => {
            tracing::error!(error = %e, "failed to check session recency");
            Err(ApprovalRefusal::Database)
        }
    }
}

/// Why an approval was refused before anything was written. Kept as a small
/// enum rather than a built `Response` so the error half of
/// [`recent_approver`]'s result stays cheap to move.
enum ApprovalRefusal {
    InvalidSession,
    StaleSession,
    Database,
}

impl ApprovalRefusal {
    fn into_response(self) -> Response {
        match self {
            Self::InvalidSession => invalid_session_page(),
            Self::StaleSession => stale_session_page(),
            Self::Database => (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response(),
        }
    }
}

async fn authorize_approve(
    State(oauth): State<OAuthState>,
    headers: axum::http::HeaderMap,
    axum::Form(form): axum::Form<ApproveForm>,
) -> Response {
    if !valid_authorize_request(
        &form.response_type,
        form.scope.as_deref(),
        form.code_challenge.as_deref(),
        form.code_challenge_method.as_deref(),
    ) {
        return (
            StatusCode::BAD_REQUEST,
            Html("<h1>Unsupported OAuth request</h1><p>Only authorization-code access to the MCP capability is supported.</p>".to_string()),
        )
            .into_response();
    }

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

    // Approving an OAuth grant is an account-level act, so it requires a
    // browser session and nothing else. An OAuth access token used to be
    // accepted here, which let one connected tool authorize another and
    // survive a lockdown by re-minting through the grant it already held.
    if !token.starts_with("lific_sess_") {
        return invalid_session_page();
    }

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

    // Denial must never create an authorization code. The redirect was already
    // checked against the registered client, and the CSRF check above binds
    // the browser action to the session that rendered the consent page.
    if form.decision.as_deref() == Some("deny") {
        let mut redirect_url = form.redirect_uri.clone();
        redirect_url.push_str(if redirect_url.contains('?') { "&" } else { "?" });
        redirect_url.push_str("error=access_denied");
        if let Some(state) = &form.state
            && !state.is_empty()
        {
            let encoded = urlencoding::encode(state);
            redirect_url.push_str(&format!("&state={encoded}"));
        }
        info!(client_id = %form.client_id, "OAuth authorization denied");
        return Redirect::to(&redirect_url).into_response();
    }

    let code = uuid_v4();
    let expires = chrono::Utc::now() + chrono::Duration::minutes(10);
    let scope = form.scope.as_deref().unwrap_or("mcp");

    // One transaction for the authorization decision and everything it
    // produces: revalidate the approving session, mint or reuse the tool's
    // bot, and store the code. SQLite serializes writers, so an account
    // lockdown either commits first (and this transaction finds no session) or
    // commits after (and burns the code this one wrote). There is no order in
    // which an approval outlives the session that authorized it.
    let conn = match oauth.db.write() {
        Ok(c) => c,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response(),
    };
    let tx = match rusqlite::Transaction::new_unchecked(&conn, TransactionBehavior::Immediate) {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = %e, "failed to open OAuth authorization transaction");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Validated inside the transaction, not before it: anything checked
    // earlier can be revoked between the check and this write. The approving
    // user's identity is bound to the issued code (LIF-79), and the session
    // must have been authenticated recently, because what this hands out is a
    // durable credential.
    let approver = match recent_approver(&tx, &token) {
        Ok(user) => user,
        Err(refusal) => return refusal.into_response(),
    };

    // LIFIC-13: pick which tool is connecting, then ensure (or reuse) its bot
    // so the issued credential attributes to the tool, not the approving human.
    // The bot inherits the human's permissions via authz's bot→owner resolution.
    let bot_id = match resolve_approval_bot(
        &tx,
        &form.tool,
        &form.tool_custom,
        Some(approver.id),
        Some(&form.client_id),
    ) {
        Ok(id) => id,
        Err((status, msg)) => {
            return (status, Html(msg)).into_response();
        }
    };

    if let Err(e) = tx.execute(
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
            bot_id,
        ],
    ) {
        tracing::error!(error = %e, "failed to store OAuth authorization code");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    if let Err(e) = tx.commit() {
        tracing::error!(error = %e, "failed to commit OAuth authorization");
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

/// Tool ids that a human can't claim as a free-text tool — they'd collide with
/// real internal identities (`admin`, `system`) or are meaningless.
const RESERVED_TOOL_IDS: &[&str] = &["admin", "system"];

/// Resolve the connecting tool from the approval form's choice, returning its
/// `(tool_id, display_name)`.
///
/// LIFIC-13: known tools come from the Connected Tools registry
/// (`cli::connect::clients::all_clients`) so the approval pick-list and the
/// bot's display name match what `lific connect` writes. Unknown tools fall to
/// free text: lowercased, non-alphanumerics collapsed to `-`, then rejected if
/// they hit a reserved id.
fn resolve_tool(raw: &str) -> Result<(String, String), LificError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(LificError::BadRequest("tool cannot be empty".into()));
    }
    // Known registry client: keep its canonical display name.
    if let Some(client) = crate::cli::connect::clients::find_client(trimmed) {
        return Ok((client.id.to_string(), client.display.to_string()));
    }

    // Free text: sanitize down to a slug id, fall back to the humanized text.
    let slug: String = trimmed
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        return Err(LificError::BadRequest("tool id cannot be empty".into()));
    }
    if RESERVED_TOOL_IDS.contains(&slug.as_str()) {
        return Err(LificError::BadRequest(format!(
            "tool id '{slug}' is reserved"
        )));
    }
    Ok((slug.clone(), trimmed.to_string()))
}

/// HTML `<option>` value that means "this isn't a known tool — let me type it".
/// The approval form selects this to reveal the free-text tool-name field.
const CUSTOM_TOOL_OPTION: &str = "__custom__";

/// Render the whole "Which tool is connecting?" widget shared by the auth-code
/// and device approval pages: the Connected Tools pick-list (from the same
/// registry `lific connect` writes), a hidden free-text field for unrecognized
/// tools, and the small inline script that reveals that field when the
/// `Custom tool…` option is chosen. Known tools come first; the pick-list and
/// the free-text input are never both live at once.
///
/// The reveal script compares against [`CUSTOM_TOOL_OPTION`] interpolated from
/// the Rust constant, so the option value lives in exactly one place.
/// Render the shared Connected Tools pick-list widget, pre-selecting the given
/// remembered tool when present (LIFIC-15).
///
/// `preset_id` is the `tool_id` a registered client already maps to, from
/// [`client_tool_id`]. When it's a known registry tool, that option is
/// pre-selected; when it's a free-text tool (a slug not in the registry), the
/// `Custom tool…` option is pre-selected and the free-text field revealed and
/// pre-filled. When `preset_id` is `None` (new client, or the device flow which
/// keys no persistent client), the pick-list starts blank for a fresh choice.
fn tool_pick_list_html(preset_id: Option<&str>) -> String {
    // A remembered tool is "custom" iff it isn't a known Connected Tool (and
    // isn't the sentinel itself). One guard, used for both the option and the
    // reveal+fill decision, so they can't diverge.
    let is_custom = preset_id.is_some_and(|id| {
        id != CUSTOM_TOOL_OPTION && crate::cli::connect::clients::find_client(id).is_none()
    });

    let mut options = String::new();
    for c in crate::cli::connect::clients::all_clients() {
        let selected = preset_id == Some(c.id);
        let sel_attr = if selected { " selected" } else { "" };
        options.push_str(&format!(
            "<option value=\"{}\"{sel_attr}>{}</option>",
            html_escape(c.id),
            html_escape(c.display)
        ));
    }
    let custom_option = if is_custom { " selected" } else { "" };
    options.push_str(&format!(
        "<option value=\"{}\"{custom_option}>Custom tool&hellip;</option>",
        CUSTOM_TOOL_OPTION
    ));

    // The placeholder is only the selected placeholder when there's no remembered
    // tool — the remembered option (or Custom) must win, not the placeholder.
    let placeholder_sel = if preset_id.is_some() { "" } else { " selected" };
    let (custom_visible, custom_value) = if is_custom {
        ("block", html_escape(preset_id.unwrap_or_default()))
    } else {
        ("none", String::new())
    };

    format!(
        "<label for=\"tool\">Which tool is connecting?</label>
        <select name=\"tool\" id=\"tool\">
            <option value=\"\"{placeholder_sel} disabled>Select a tool&hellip;</option>
            {options}
        </select>
        <div id=\"custom_tool\" style=\"display:{custom_visible};\">
            <label for=\"tool_custom\">Custom tool name</label>
            <input type=\"text\" name=\"tool_custom\" id=\"tool_custom\" placeholder=\"e.g. my-agent\" value=\"{custom_value}\">
        </div>
        <script>
        var tool = document.getElementById('tool');
        var custom = document.getElementById('custom_tool');
        tool.addEventListener('change', function () {{
            custom.style.display = tool.value === '{custom_option_value}' ? 'block' : 'none';
        }});
        </script>",
        custom_option_value = CUSTOM_TOOL_OPTION,
    )
}

/// The shared LIFIC-13 tool-resolution + bot-mint step used by both the
/// auth-code and device approval doors.
///
/// Resolves which tool is connecting from the form's `(tool, tool_custom)`
/// pair, validates it, then mints (or reuses) the per-tool bot owned by the
/// approving human. Returns the bot's user id on success, or a small
/// `(StatusCode, message)` the caller renders as its error page (missing tool,
/// unsanitizable/reserved id, no resolvable owner, or DB failure).
///
/// Takes the caller's connection rather than reaching for the pool itself, so
/// the bot it mints lands in the same transaction as the grant that names it.
/// It used to take its own write lock, which forced both approval handlers to
/// resolve the bot *before* opening their own write and left a window where a
/// recovery could revoke the approver between the two.
///
/// When `client_id` is `Some`, the resolved `tool_id` is remembered on that
/// client (LIFIC-15) so a reconnect pre-fills the pick-list instead of
/// re-asking. The device flow passes `None` — it has no persistent client.
fn resolve_approval_bot(
    conn: &rusqlite::Connection,
    tool: &Option<String>,
    tool_custom: &Option<String>,
    approving_user_id: Option<i64>,
    client_id: Option<&str>,
) -> Result<i64, (StatusCode, String)> {
    let tool_text = match (tool, tool_custom) {
        // A specific known tool chosen from the pick-list.
        (Some(id), _)
            if !id.trim().is_empty() && id.trim() != CUSTOM_TOOL_OPTION =>
        {
            id.clone()
        }
        // "Custom tool…" chosen — the free-text name is required.
        (Some(id), Some(name))
            if id.trim() == CUSTOM_TOOL_OPTION && !name.trim().is_empty() =>
        {
            name.clone()
        }
        _ => return Err((StatusCode::BAD_REQUEST, "Pick which tool is connecting".into())),
    };
    let (tool_id, display_name) = match resolve_tool(&tool_text) {
        Ok(v) => v,
        Err(e) => return Err((StatusCode::BAD_REQUEST, e.to_string())),
    };
    let Some(owner_id) = approving_user_id else {
        return Err((
            StatusCode::BAD_REQUEST,
            "No operator to attribute to — sign in as a human first".into(),
        ));
    };
    // LIFIC-15: remember the tool on the client (same conn, best-effort).
    if let Some(client_id) = client_id
        && let Err(e) = conn.execute(
            "UPDATE oauth_clients SET tool_id = ?1 WHERE client_id = ?2",
            params![tool_id, client_id],
        )
    {
        tracing::error!(error = %e, client_id, "failed to remember client tool");
    }
    match crate::db::queries::users::ensure_bot(conn, owner_id, &tool_id, &display_name) {
        Ok(bot) => Ok(bot.id),
        Err(e) => {
            tracing::error!(error = %e, "failed to mint OAuth tool bot");
            Err((StatusCode::INTERNAL_SERVER_ERROR, "database error".into()))
        }
    }
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
            // `datetime(expires_at)`: device codes are stored as RFC 3339
            // ('2026-08-20T12:00:00+00:00'), which does NOT compare correctly
            // against SQLite's own 'YYYY-MM-DD HH:MM:SS' form as raw text. The
            // 'T' sorts after every digit, so a same-day RFC 3339 timestamp
            // reads as later than it is and an expired code looks live.
            "DELETE FROM oauth_device_codes WHERE datetime(expires_at) <= datetime('now')",
            [],
        );
    }
}

#[derive(Deserialize)]
struct DeviceAuthRequest {
    #[serde(default)]
    client_name: Option<String>,
    /// Accepted per RFC 8628 so conforming clients are not rejected, but
    /// device grants always issue the fixed `mcp` scope.
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
                "error_description": crate::ratelimit::retry_after_message(
                    "too many device authorization requests",
                    retry,
                )
            })),
        )
            .into_response();
        if retry > 0 && let Ok(v) = retry.to_string().parse() {
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
    let device_code_hash = sha256_hex(device_code.as_bytes());

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
    // LIFIC-13: same Connected Tools pick-list as the authorize screen. The
    // device flow has no persistent client to key a remembered tool on (device
    // codes are one-time handshakes), so it always asks.
    let tool_pick_list = tool_pick_list_html(None);
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
        input[type=text], select {{ width: 100%%; box-sizing: border-box; margin-top: 0.4em; padding: 12px; border-radius: 6px; border: 1px solid #333; background: #111; color: #fff; }}
        input[type=text] {{ font-size: 1.2em; letter-spacing: 0.15em; text-align: center; text-transform: uppercase; }}
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
        {tool_pick_list}
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
        tool_pick_list = tool_pick_list,
    ))
}

#[derive(Deserialize)]
struct DeviceApproveForm {
    user_code: String,
    decision: Option<String>,
    csrf_token: Option<String>,
    /// LIFIC-13: which tool is connecting — a registry id, or empty meaning
    /// `tool_custom` holds a free-text name.
    tool: Option<String>,
    /// Free-text tool name when `tool` is unset.
    tool_custom: Option<String>,
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

    // Same rule as the authorization-code flow: only a browser session may
    // approve a device. An OAuth access token is a tool's credential, not a
    // human at a keyboard.
    if !token.starts_with("lific_sess_") {
        return invalid_session_page();
    }

    let normalized = normalize_user_code(&form.user_code);
    let deny = form.decision.as_deref() == Some("deny");
    let new_status = if deny { "denied" } else { "approved" };

    // One transaction: revalidate the approving session, resolve the tool's
    // bot, and move the device code out of `pending`. `resolve_approval_bot`
    // used to take its own write lock, which is why this handler had to
    // resolve the bot before opening its own, and why a lockdown could land
    // between the two.
    let conn = match oauth.db.write() {
        Ok(c) => c,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response(),
    };
    let tx = match rusqlite::Transaction::new_unchecked(&conn, TransactionBehavior::Immediate) {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = %e, "failed to open device approval transaction");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    // Denying is a refusal, not a grant. It creates nothing, hands out
    // nothing, and is the thing a person does when a device they do not
    // recognise is asking for access, which is exactly the moment not to make
    // them sign in again first. So a *live* session is required either way,
    // and the 15-minute freshness rule applies only to approval.
    let approver = if deny {
        match crate::db::queries::users::validate_session(&tx, &token) {
            Ok(user) => user,
            Err(_) => return invalid_session_page(),
        }
    } else {
        match recent_approver(&tx, &token) {
            Ok(user) => user,
            Err(refusal) => return refusal.into_response(),
        }
    };

    // LIFIC-13: on approval, pick which tool is connecting and mint (or reuse)
    // its bot, binding the device code to the bot so the exchanged token
    // attributes to the tool. Deny resolves no tool and mints no bot: a
    // refused device must not leave a connected-tool identity behind as a
    // side effect of refusing it.
    let target_user_id: Option<i64> = if deny {
        Some(approver.id)
    } else {
        match resolve_approval_bot(&tx, &form.tool, &form.tool_custom, Some(approver.id), None) {
            Ok(id) => Some(id),
            Err((status, msg)) => return (status, Html(msg)).into_response(),
        }
    };

    let updated = tx
        .execute(
            "UPDATE oauth_device_codes
             SET status = ?1, user_id = ?2
             WHERE user_code = ?3 AND status = 'pending'
               AND datetime(expires_at) > datetime('now')",
            params![new_status, target_user_id, normalized],
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

    if let Err(e) = tx.commit() {
        tracing::error!(error = %e, "failed to commit device approval");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
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
    /// Parsed so a `refresh_token` grant deserializes rather than 422s, and
    /// is then refused by the `grant_type` match: we issue no refresh tokens.
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

    // The whole exchange is one transaction: read the code, validate it, check
    // that the identity it names may still authenticate, burn it, and insert
    // the token. Splitting the read from the burn is what let a recovery land
    // in between and hand out a 30-day token against a code it had already
    // invalidated.
    let conn = match state.db.write() {
        Ok(c) => c,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response(),
    };
    let conn = match rusqlite::Transaction::new_unchecked(&conn, TransactionBehavior::Immediate) {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = %e, "failed to open OAuth token transaction");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
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
        // `datetime(expires_at)` for the same reason as the device codes: the
        // column holds RFC 3339, and raw text comparison against
        // `datetime('now')` mis-orders it within the same day.
        "SELECT client_id, redirect_uri, code_challenge, code_challenge_method, used, scope, user_id \
         FROM oauth_codes WHERE code = ?1 AND datetime(expires_at) > datetime('now')",
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

    // The code must name an identity, and that identity must still be one that
    // may authenticate.
    //
    // A NULL `user_id` is a pre-LIF-79 legacy row. Every approval since binds
    // the per-tool bot, so nothing issues one any more, and exchanging one
    // produced an *unbound* access token: a credential that names nobody,
    // which the caller resolution then treats as the operator. That is a
    // silent privilege escalation, and no lockdown can revoke it either, since
    // a lockdown scopes by user id. Fail closed instead.
    //
    // A bound code still has to resolve a live identity: the bot may have been
    // deleted between approval and exchange (PR #23 review), and its owner may
    // have been deactivated or locked down since. `credential_is_live` is the
    // same predicate every other bearer credential is judged by.
    let Some(code_user_id) = code_user_id else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid_grant", "error_description": "this authorization is not bound to an identity; reconnect to authorize again"})),
        )
            .into_response();
    };
    let live = crate::db::queries::users::get_user_by_id(&conn, code_user_id)
        .and_then(|user| crate::db::queries::users::credential_is_live(&conn, &user))
        .unwrap_or(false);
    if !live {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid_grant", "error_description": "authorizing user is no longer active"})),
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
    let token_hash = sha256_hex(access_token.as_bytes());
    let expires_in: u64 = 3600 * 24 * 30; // 30 days
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(expires_in as i64);

    if let Err(e) = conn.execute(
        "INSERT INTO oauth_tokens (access_token, client_id, expires_at, scope, user_id) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![token_hash, stored_client_id, expires_at.to_rfc3339(), scope, code_user_id],
    ) {
        tracing::error!(error = %e, "failed to store OAuth token");
        return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
    }

    if let Err(e) = conn.commit() {
        tracing::error!(error = %e, "failed to commit OAuth token exchange");
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
    let device_code_hash = sha256_hex(device_code.as_bytes());

    // LIF-370 extended: the read, the status decision, the liveness check, the
    // token insert and the consumed transition are all one transaction. The
    // approved-row read used to sit outside the transaction that consumed it,
    // so a recovery that denied the grant between the two still handed the
    // polling device a token.
    let conn = match state.db.write() {
        Ok(c) => c,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response(),
    };
    let conn = match rusqlite::Transaction::new_unchecked(&conn, TransactionBehavior::Immediate) {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error = %e, "failed to open device token transaction");
            return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
        }
    };

    /// Commit the bookkeeping a non-issuing outcome still needs to persist
    /// (the expiry sweep, the poll timestamp) and hand back the response. A
    /// failed commit is not worth failing the poll over: the client simply
    /// retries, so log it and answer as decided.
    macro_rules! finish {
        ($conn:expr, $response:expr) => {{
            if let Err(e) = $conn.commit() {
                tracing::error!(error = %e, "failed to commit device poll bookkeeping");
            }
            return $response;
        }};
    }

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
        finish!(
            conn,
            device_error(StatusCode::BAD_REQUEST, "expired_token", None)
        );
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
            finish!(
                conn,
                device_error(StatusCode::BAD_REQUEST, "slow_down", None)
            );
        }
    }

    // Record this poll time (used for the next slow_down check).
    let _ = conn.execute(
        "UPDATE oauth_device_codes SET last_polled_at = ?1 WHERE device_code_hash = ?2",
        params![now.to_rfc3339(), device_code_hash],
    );

    match row.status.as_str() {
        "pending" => finish!(
            conn,
            device_error(StatusCode::BAD_REQUEST, "authorization_pending", None)
        ),
        "denied" => finish!(
            conn,
            device_error(StatusCode::BAD_REQUEST, "access_denied", None)
        ),
        "consumed" => finish!(
            conn,
            device_error(
                StatusCode::BAD_REQUEST,
                "invalid_grant",
                Some("device code already used")
            )
        ),
        "approved" => {
            // Mint the access token bound to the approving user, then mark the
            // code consumed (single use).
            //
            // The approval must name an identity. A NULL `user_id` on an
            // approved row is a pre-LIF-79 legacy grant; exchanging it minted
            // an *unbound* access token, which resolves as the operator and
            // which no lockdown can revoke, because a lockdown scopes by user
            // id. Nothing issues one any more, so fail closed. Same reasoning
            // and same wording as the authorization-code path.
            //
            // A bound approval still has to resolve a live identity: the bot
            // may have been deleted between approval and this poll (PR #23
            // review), and its owner may have been deactivated or locked down
            // since. `credential_is_live` is the predicate every other bearer
            // credential is judged by.
            let Some(approved_user_id) = row.user_id else {
                finish!(
                    conn,
                    device_error(
                        StatusCode::BAD_REQUEST,
                        "invalid_grant",
                        Some(
                            "this authorization is not bound to an identity; reconnect to \
                             authorize again"
                        ),
                    )
                );
            };
            let live = crate::db::queries::users::get_user_by_id(&conn, approved_user_id)
                .and_then(|user| crate::db::queries::users::credential_is_live(&conn, &user))
                .unwrap_or(false);
            if !live {
                finish!(
                    conn,
                    device_error(
                        StatusCode::BAD_REQUEST,
                        "invalid_grant",
                        Some("authorizing user is no longer active"),
                    )
                );
            }
            let scope = "mcp";
            let client_id = "device";

            let access_token = format!("lific_at_{}", uuid_v4());
            let token_hash = sha256_hex(access_token.as_bytes());
            let expires_in: u64 = 3600 * 24 * 30; // 30 days
            let expires_at = now + chrono::Duration::seconds(expires_in as i64);

            // LIF-370: minting the token and burning the device code are one
            // atomic step. Previously the consumed-UPDATE was `let _ =`, so a
            // failed write handed out a token while leaving the code
            // `approved` and replayable for as many tokens as the client cared
            // to poll for. Either both writes land or neither does, and since
            // LIF-PR32 the approved-row read is inside the same transaction, so
            // a denial that lands first wins cleanly instead of being read
            // stale.
            let tx = conn;

            // Ensure a client row exists so the FK on oauth_tokens is satisfied.
            let _ = tx.execute(
                "INSERT OR IGNORE INTO oauth_clients (client_id, client_name, redirect_uris)
                 VALUES ('device', 'Device Authorization', '[]')",
                [],
            );

            if let Err(e) = tx.execute(
                "INSERT INTO oauth_tokens (access_token, client_id, expires_at, scope, user_id)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    token_hash,
                    client_id,
                    expires_at.to_rfc3339(),
                    scope,
                    approved_user_id
                ],
            ) {
                tracing::error!(error = %e, "failed to store device OAuth token");
                return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
            }

            // Single-use: mark consumed so a replay returns invalid_grant. The
            // `status = 'approved'` guard means exactly one exchange can win;
            // anything other than one row changed rolls the token back.
            match tx.execute(
                "UPDATE oauth_device_codes SET status = 'consumed'
                 WHERE device_code_hash = ?1 AND status = 'approved'",
                params![device_code_hash],
            ) {
                Ok(1) => {}
                Ok(n) => {
                    tracing::error!(
                        rows = n,
                        "device code not consumed exactly once; refusing to issue token"
                    );
                    return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
                }
                Err(e) => {
                    tracing::error!(error = %e, "failed to consume device code");
                    return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
                }
            }

            if let Err(e) = tx.commit() {
                tracing::error!(error = %e, "failed to commit device token exchange");
                return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response();
            }

            info!(scope = %scope, "OAuth device token issued");
            Json(TokenResponse {
                access_token,
                token_type: "Bearer".into(),
                expires_in,
                scope: scope.into(),
            })
            .into_response()
        }
        _ => finish!(
            conn,
            device_error(StatusCode::BAD_REQUEST, "invalid_grant", None)
        ),
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
    /// RFC 7009 explicitly makes this a hint the server MAY ignore. We do:
    /// there is only one revocable token store (`oauth_tokens`), so there is
    /// nothing for the hint to disambiguate.
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
            match state.db.read() {
                Ok(conn) => crate::db::queries::users::validate_session(&conn, t).is_ok(),
                Err(_) => false,
            }
        }
        Some(t) if t.starts_with("lific_at_") => authenticate_oauth_token(&state.db, t).is_some(),
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
    let token_hash = sha256_hex(req.token.as_bytes());
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

/// What an OAuth bearer token resolved to.
///
/// The middleware used to answer this with three separate calls: "is it
/// valid", "whose is it", "is that user live". Each took its own pooled
/// connection, so the three answers came from three different snapshots of the
/// database. A token revoked between the first and the second read as valid,
/// then as unbound, and an unbound OAuth token is the operator: revoking a
/// tool's credential could *promote* it. That is the whole reason this is one
/// function and one connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OAuthCredential {
    /// Valid, and bound to a user who may authenticate right now.
    Bound(AuthUser),
    /// Valid, and genuinely carries no user binding.
    ///
    /// Only rows issued before user binding existed (pre-LIF-79) can be in
    /// this state; every approval since binds the per-tool bot, and the token
    /// exchange refuses an unbound grant outright. It is kept because such
    /// rows may still exist in an upgraded database, and it resolves to the
    /// operator fallback, which is the documented pre-LIF-79 behaviour.
    LegacyUnbound,
}

/// Why an OAuth bearer token did not authenticate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthReject {
    /// Wrong prefix, no such row, revoked, or expired.
    Invalid,
    /// The row is fine but names an identity that may not authenticate: the
    /// user is gone, deactivated, or is a bot whose owner is deactivated.
    ///
    /// Deliberately distinct from [`OAuthReject::Invalid`] at the type level so
    /// it can never be collapsed into "unbound". A dead binding is a dead
    /// credential, not an anonymous one.
    DeadBinding,
    /// The database could not be read. Fails closed.
    Unavailable,
}

/// Resolve an OAuth bearer token with one SQL statement.
///
/// A connection alone is not a snapshot in SQLite autocommit mode: each
/// statement starts its own read transaction. Keep token validity, its nullable
/// binding, the bound user, and the bot owner's liveness in one joined query so
/// revocation cannot land between those decisions.
pub fn resolve_oauth_credential(db: &DbPool, token: &str) -> Result<OAuthCredential, OAuthReject> {
    if !token.starts_with("lific_at_") {
        return Err(OAuthReject::Invalid);
    }
    let token_hash = sha256_hex(token.as_bytes());
    let conn = db.read().map_err(|error| {
        tracing::error!(%error, "OAuth token lookup could not read the database");
        OAuthReject::Unavailable
    })?;

    struct CredentialRow {
        bound_user_id: Option<i64>,
        user_id: Option<i64>,
        username: Option<String>,
        display_name: Option<String>,
        is_admin: Option<bool>,
        is_active: Option<bool>,
        is_bot: Option<bool>,
        owner_id: Option<i64>,
        owner_is_active: Option<bool>,
    }

    let row = conn
        .query_row(
            "SELECT token.user_id,
                    user.id, user.username, user.display_name, user.is_admin,
                    user.is_active, user.is_bot, user.owner_id, owner.is_active
             FROM oauth_tokens token
             LEFT JOIN users user ON user.id = token.user_id
             LEFT JOIN users owner ON owner.id = user.owner_id
             WHERE token.access_token = ?1 AND token.revoked = 0
               AND datetime(token.expires_at) > datetime('now')",
            params![token_hash],
            |row| {
                Ok(CredentialRow {
                    bound_user_id: row.get(0)?,
                    user_id: row.get(1)?,
                    username: row.get(2)?,
                    display_name: row.get(3)?,
                    is_admin: row.get(4)?,
                    is_active: row.get(5)?,
                    is_bot: row.get(6)?,
                    owner_id: row.get(7)?,
                    owner_is_active: row.get(8)?,
                })
            },
        )
        .optional()
        .map_err(|error| {
            tracing::error!(%error, "OAuth token lookup failed");
            OAuthReject::Unavailable
        })?
        .ok_or(OAuthReject::Invalid)?;

    let Some(bound_user_id) = row.bound_user_id else {
        return Ok(OAuthCredential::LegacyUnbound);
    };

    let (Some(user_id), Some(username), Some(display_name), Some(is_admin), Some(is_active), Some(is_bot)) = (
        row.user_id,
        row.username,
        row.display_name,
        row.is_admin,
        row.is_active,
        row.is_bot,
    ) else {
        return Err(OAuthReject::DeadBinding);
    };
    debug_assert_eq!(user_id, bound_user_id);

    // Match `credential_is_live`: an inactive user is dead; an owned bot is
    // dead when its owner exists and is inactive. Ownerless and dangling-owner
    // bots retain the existing fallback of being evaluated as themselves.
    if !is_active || (is_bot && row.owner_id.is_some() && row.owner_is_active == Some(false)) {
        return Err(OAuthReject::DeadBinding);
    }

    Ok(OAuthCredential::Bound(AuthUser {
        id: user_id,
        username,
        display_name,
        is_admin,
    }))
}

/// Authenticate an OAuth access token as an *approving identity*.
///
/// Returns:
/// - `None` when the token does not authenticate at all: invalid, revoked,
///   expired, or bound to a user who may no longer authenticate (deactivated,
///   or a bot whose owner is deactivated — LIF-214 follow-up, see
///   `queries::users::credential_is_live`).
/// - `Some(None)` for a valid legacy token carrying no user binding.
/// - `Some(Some(id))` for a valid token bound to a live user.
///
/// This is the OAuth-token twin of [`crate::db::queries::users::validate_session`],
/// which applies the same liveness rule to session tokens.
fn authenticate_oauth_token(db: &DbPool, token: &str) -> Option<Option<i64>> {
    match resolve_oauth_credential(db, token) {
        Ok(OAuthCredential::Bound(user)) => Some(Some(user.id)),
        Ok(OAuthCredential::LegacyUnbound) => Some(None),
        Err(_) => None,
    }
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
            crate::ratelimit::parse_trusted_proxies(&["127.0.0.0/8".into()])
                .expect("test trusted proxy range must parse"),
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

    /// The user an access token resolves to, or `None` for anything that does
    /// not authenticate as a bound identity.
    fn bound_user(db: &DbPool, token: &str) -> Option<i64> {
        match resolve_oauth_credential(db, token) {
            Ok(OAuthCredential::Bound(user)) => Some(user.id),
            _ => None,
        }
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
            "client_id={}&redirect_uri={}&response_type=code&code_challenge=abc&code_challenge_method=S256&scope=mcp&csrf_token={}&tool=claude-code",
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
    async fn authorize_consent_identifies_client_and_capability() {
        let (app, _db) = test_oauth_app();
        let client_id = register_client_helper(&app, "http://localhost/callback").await;
        let uri = format!(
            "/oauth/authorize?client_id={client_id}&redirect_uri={}&response_type=code&scope=mcp&code_challenge=abc&code_challenge_method=S256",
            urlencoding::encode("http://localhost/callback")
        );
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = String::from_utf8(resp.into_body().collect().await.unwrap().to_bytes().to_vec())
            .unwrap();
        assert!(body.contains("Test Client"));
        assert!(body.contains("MCP issue-tracker access"));
        assert!(body.contains("http://localhost/callback"));
        assert!(!body.contains("An application wants"));
    }

    #[tokio::test]
    async fn authorize_consent_rejects_unknown_capability_or_client() {
        let (app, _db) = test_oauth_app();
        let client_id = register_client_helper(&app, "http://localhost/callback").await;
        let bad_scope = format!(
            "/oauth/authorize?client_id={client_id}&redirect_uri={}&response_type=code&scope=admin",
            urlencoding::encode("http://localhost/callback")
        );
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(bad_scope).body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let unknown = "/oauth/authorize?client_id=unknown&redirect_uri=http%3A%2F%2Flocalhost%2Fcallback&response_type=code&scope=mcp";
        let resp = app
            .oneshot(Request::builder().uri(unknown).body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn authorize_consent_rejects_missing_scope_or_pkce() {
        let (app, _db) = test_oauth_app();
        let client_id = register_client_helper(&app, "http://localhost/callback").await;
        let base = format!(
            "/oauth/authorize?client_id={client_id}&redirect_uri={}&response_type=code",
            urlencoding::encode("http://localhost/callback")
        );

        for suffix in [
            "&code_challenge=abc&code_challenge_method=S256",
            "&scope=mcp&code_challenge_method=S256",
            "&scope=mcp&code_challenge=abc",
            "&scope=mcp&code_challenge=&code_challenge_method=S256",
            "&scope=mcp&code_challenge=abc&code_challenge_method=plain",
        ] {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("{base}{suffix}"))
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "suffix={suffix}");
        }
    }

    #[tokio::test]
    async fn authorize_deny_redirects_without_issuing_code() {
        let (app, db) = test_oauth_app();
        let session_token = create_test_session(&db);
        let client_id = register_client_helper(&app, "http://localhost/callback").await;
        let csrf = generate_csrf_token(&session_token);
        let body = format!(
            "client_id={client_id}&redirect_uri={}&response_type=code&scope=mcp&code_challenge=abc&code_challenge_method=S256&state=opaque-state&csrf_token={csrf}&decision=deny",
            urlencoding::encode("http://localhost/callback"),
        );
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
        if !resp.status().is_redirection() {
            let status = resp.status();
            let body = resp.into_body().collect().await.unwrap().to_bytes();
            panic!("deny status={status}, body={}", String::from_utf8_lossy(&body));
        }
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(location.starts_with("http://localhost/callback?error=access_denied"));
        assert!(location.contains("state=opaque-state"));
        let conn = db.read().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM oauth_codes", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0, "denial must not mint an authorization code");
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

    // ── LIF-415: only public-client auth is advertised ──────────
    //
    // No client secret is ever issued (registration returns a client_id and
    // nothing else) and `token_exchange` never looks for one, so advertising
    // `client_secret_post` promised an authentication method that does not
    // exist. A client that sent a secret would have it silently ignored.

    #[tokio::test]
    async fn metadata_advertises_only_public_client_auth() {
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

        let methods = val["token_endpoint_auth_methods_supported"]
            .as_array()
            .unwrap();
        assert_eq!(
            methods,
            &vec![serde_json::Value::from("none")],
            "only `none` is implemented, so only `none` may be advertised"
        );
    }

    #[tokio::test]
    async fn register_reports_none_auth_method_even_when_a_secret_is_requested() {
        let (app, _) = test_oauth_app();
        let body = serde_json::json!({
            "redirect_uris": ["http://localhost/callback"],
            "client_name": "Test",
            "token_endpoint_auth_method": "client_secret_post"
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

        assert_eq!(
            val["token_endpoint_auth_method"], "none",
            "the response states what was registered, not what was asked for"
        );
        assert!(
            val.get("client_secret").is_none(),
            "no client secret is ever issued"
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
        let token_hash = sha256_hex(token.as_bytes());
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
        assert!(resolve_oauth_credential(&db, token).is_ok());

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
        assert!(resolve_oauth_credential(&db, token).is_err());
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
        let auth_hash = sha256_hex(auth_token.as_bytes());
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

    // ── LIF-51 / LIF-384: scope is stored and advertised, never enforced ──
    //
    // The two tests that lived here exercised `validate_oauth_token_with_scope`,
    // which is gone: nothing compared its return value against a required
    // scope. Revoked tokens failing validation is covered by
    // `revoke_token_invalidates_access`, and the `scope` field clients read off
    // the token response is pinned in
    // `device_consumed_code_cannot_mint_a_second_token`.

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
            "client_id={}&redirect_uri={}&response_type=code&code_challenge={}&code_challenge_method=S256&scope=mcp&csrf_token={}&tool=claude-code",
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

        // LIFIC-13: the issued credential binds to the per-tool BOT, not the
        // approving human, so the audit log distinguishes which tool acted.
        let (bot_id, bot_username): (i64, String) = {
            let conn = db.read().unwrap();
            conn.query_row(
                "SELECT id, username FROM users WHERE username = 'claude-code-oauthtest'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
        };
        assert!(!bot_username.is_empty());
        {
            let conn = db.read().unwrap();
            let code_user: Option<i64> = conn
                .query_row(
                    "SELECT user_id FROM oauth_codes WHERE code = ?1",
                    params![code],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                code_user,
                Some(bot_id),
                "code should bind the tool bot, not the approver"
            );
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

        // The middleware resolves this token to the tool bot, not the human.
        assert_eq!(bound_user(&db, access_token), Some(bot_id));
        assert_ne!(bot_id, user_id, "bot must differ from the approving human");
    }

    #[tokio::test]
    async fn reapproval_reuses_the_same_tool_bot() {
        let (app, db) = test_oauth_app();
        let session_token = create_test_session(&db); // user "oauthtest"
        let client_id = register_client_helper(&app, "http://localhost/callback").await;

        let approve = |app: Router, csrf: &str| {
            let body = format!(
                "client_id={}&redirect_uri={}&response_type=code&code_challenge=abc&code_challenge_method=S256&scope=mcp&csrf_token={}&tool=opencode",
                client_id,
                urlencoding::encode("http://localhost/callback"),
                urlencoding::encode(csrf),
            );
            app.oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/authorize")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("cookie", format!("lific_token={session_token}"))
                    .body(axum::body::Body::from(body))
                    .unwrap(),
            )
        };

        assert!(approve(app.clone(), &generate_csrf_token(&session_token))
            .await
            .unwrap()
            .status()
            .is_redirection());
        let first_id: i64 = {
            let conn = db.read().unwrap();
            conn.query_row(
                "SELECT id FROM users WHERE username = 'opencode-oauthtest'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        // Re-approval of the same tool+owner must reuse the same bot.
        assert!(approve(app.clone(), &generate_csrf_token(&session_token))
            .await
            .unwrap()
            .status()
            .is_redirection());
        let second_id: i64 = {
            let conn = db.read().unwrap();
            conn.query_row(
                "SELECT id FROM users WHERE username = 'opencode-oauthtest'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(
            first_id, second_id,
            "re-approval must reuse the same bot, not mint a duplicate"
        );
    }

    // ── LIFIC-15: remember tool per client, pre-fill on reconnect ──

    #[tokio::test]
    async fn approve_persists_remembered_tool_on_client() {
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
        assert!(resp.status().is_redirection());

        // The approved tool choice is remembered on the registered client.
        let tool_id: Option<String> = {
            let conn = db.read().unwrap();
            conn.query_row(
                "SELECT tool_id FROM oauth_clients WHERE client_id = ?1",
                params![client_id],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(tool_id.as_deref(), Some("claude-code"));
    }

    #[tokio::test]
    async fn authorize_page_prefills_remembered_known_tool() {
        let (app, db) = test_oauth_app();
        let client_id = register_client_helper(&app, "http://localhost/callback").await;

        // Remember the tool on the client directly (as an approval would).
        {
            let conn = db.write().unwrap();
            conn.execute(
                "UPDATE oauth_clients SET tool_id = 'opencode' WHERE client_id = ?1",
                params![client_id],
            )
            .unwrap();
        }

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/oauth/authorize?client_id={client_id}&redirect_uri={}&response_type=code&scope=mcp&code_challenge=abc&code_challenge_method=S256", urlencoding::encode("http://localhost/callback")))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let html = String::from_utf8_lossy(&bytes);
        // Reconnect: the known tool is pre-selected (real browser behavior —
        // the placeholder must NOT also carry selected, or it wins in tree order).
        assert!(
            html.contains("value=\"opencode\" selected"),
            "known tool should be pre-selected, html={html}"
        );
        assert!(
            !html.contains("value=\"\" selected"),
            "placeholder must not also be selected when a tool is remembered, html={html}"
        );
    }

    #[tokio::test]
    async fn authorize_page_prefills_remembered_custom_tool() {
        let (app, db) = test_oauth_app();
        let client_id = register_client_helper(&app, "http://localhost/callback").await;

        // A free-text tool: stored tool_id is a slug not in the registry.
        {
            let conn = db.write().unwrap();
            conn.execute(
                "UPDATE oauth_clients SET tool_id = 'my-editor' WHERE client_id = ?1",
                params![client_id],
            )
            .unwrap();
        }

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/oauth/authorize?client_id={client_id}&redirect_uri={}&response_type=code&scope=mcp&code_challenge=abc&code_challenge_method=S256", urlencoding::encode("http://localhost/callback")))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let html = String::from_utf8_lossy(&bytes);
        // Reconnect: the custom field is revealed and pre-filled with the slug.
        assert!(
            html.contains("display:block"),
            "custom tool field should be revealed, html={html}"
        );
        assert!(
            html.contains("value=\"my-editor\""),
            "custom field should prefill the remembered slug, html={html}"
        );
    }

    #[tokio::test]
    async fn authorize_requires_a_tool_choice() {
        let (app, db) = test_oauth_app();
        let session_token = create_test_session(&db);
        let client_id = register_client_helper(&app, "http://localhost/callback").await;
        let csrf = generate_csrf_token(&session_token);
        // No tool, no tool_custom → must be rejected, not silently attributed.
        let body = format!(
            "client_id={}&redirect_uri={}&response_type=code&code_challenge=abc&code_challenge_method=S256&scope=mcp&csrf_token={}",
            client_id,
            urlencoding::encode("http://localhost/callback"),
            urlencoding::encode(&csrf),
        );
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
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn authorize_rejects_reserved_free_text_tool() {
        let (app, db) = test_oauth_app();
        let session_token = create_test_session(&db);
        let client_id = register_client_helper(&app, "http://localhost/callback").await;
        let csrf = generate_csrf_token(&session_token);
        let body = format!(
            "client_id={}&redirect_uri={}&response_type=code&code_challenge=abc&code_challenge_method=S256&scope=mcp&csrf_token={}&tool=__custom__&tool_custom=admin",
            client_id,
            urlencoding::encode("http://localhost/callback"),
            urlencoding::encode(&csrf),
        );
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
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn authorize_custom_tool_choice_mints_a_sanitized_bot() {
        // Selecting "Custom tool…" reveals a free-text field; a real custom
        // tool name sanitizes into the bot's username and mints it.
        let (app, db) = test_oauth_app();
        let session_token = create_test_session(&db);
        let client_id = register_client_helper(&app, "http://localhost/callback").await;
        let csrf = generate_csrf_token(&session_token);
        let body = format!(
            "client_id={}&redirect_uri={}&response_type=code&code_challenge=abc&code_challenge_method=S256&scope=mcp&csrf_token={}&tool=__custom__&tool_custom=My Editor",
            client_id,
            urlencoding::encode("http://localhost/callback"),
            urlencoding::encode(&csrf),
        );
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
            "custom free-text tool should approve, got {}",
            resp.status()
        );
        let bot: (String, String) = {
            let conn = db.read().unwrap();
            conn.query_row(
                "SELECT username, display_name FROM users WHERE username = 'my-editor-oauthtest'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
        };
        assert_eq!(bot.0, "my-editor-oauthtest");
        assert_eq!(bot.1, "My Editor");
    }

    #[tokio::test]
    async fn legacy_token_without_user_resolves_to_none() {
        // Tokens issued before LIF-79 have NULL user_id and must keep working,
        // resolving to no user (anonymous) rather than erroring.
        let (_, db) = test_oauth_app();
        let token = "lific_at_legacy-no-user-binding";
        let token_hash = sha256_hex(token.as_bytes());
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
        assert!(
            resolve_oauth_credential(&db, token).is_ok(),
            "token still valid"
        );
        assert_eq!(
            bound_user(&db, token),
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
        let hash = sha256_hex(device_code.as_bytes());
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

        let device_hash = sha256_hex(device_code.as_bytes());

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
            "user_code={}&decision=approve&csrf_token={}&tool=claude-code",
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

        // LIFIC-13: the device row binds the per-tool BOT, not the approver.
        let bot_id: i64 = {
            let conn = db.read().unwrap();
            conn.query_row(
                "SELECT id FROM users WHERE username = 'claude-code-oauthtest'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
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
            assert_eq!(uid, Some(bot_id));
        }

        // Next poll: approved → returns a token bound to the tool bot.
        reset_last_poll(&db);
        let (status, body) = poll_device_token(&app, &device_code).await;
        assert_eq!(status, StatusCode::OK, "expected token, got {body}");
        let access_token = body["access_token"].as_str().unwrap();
        assert!(access_token.starts_with("lific_at_"));
        assert_eq!(bound_user(&db, access_token), Some(bot_id));
        assert_ne!(bot_id, user_id, "bot must differ from the approving human");

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
        let hash = sha256_hex(device_code.as_bytes());

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

    // ── LIF-370: token mint + code consumption are one transaction ───────

    /// Approve a device code directly in the DB (the verification-page dance
    /// is covered end-to-end above) and clear `last_polled_at` so the next
    /// poll isn't answered with `slow_down`.
    ///
    /// Binds a real user, because the real approval path always does and an
    /// approved row that names nobody is refused at exchange time (see
    /// `grant_lifetime::a_legacy_unbound_device_approval_cannot_be_exchanged`).
    /// These tests are about the consume/mint transaction, not about that.
    fn approve_device_code(db: &DbPool, device_hash: &str) {
        let conn = db.write().unwrap();
        let approver = crate::db::queries::users::create_user(
            &conn,
            &crate::db::models::CreateUser {
                username: "device-approver".into(),
                email: "device-approver@test.com".into(),
                password: "testpassword1".into(),
                display_name: None,
                is_admin: false,
                is_bot: false,
            },
        )
        .unwrap();
        conn.execute(
            "UPDATE oauth_device_codes
             SET status = 'approved', user_id = ?2, last_polled_at = NULL
             WHERE device_code_hash = ?1",
            params![device_hash, approver.id],
        )
        .unwrap();
    }

    fn device_status(db: &DbPool, device_hash: &str) -> String {
        let conn = db.read().unwrap();
        conn.query_row(
            "SELECT status FROM oauth_device_codes WHERE device_code_hash = ?1",
            params![device_hash],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn token_count(db: &DbPool) -> i64 {
        let conn = db.read().unwrap();
        conn.query_row("SELECT COUNT(*) FROM oauth_tokens", [], |r| r.get(0))
            .unwrap()
    }

    #[tokio::test]
    async fn device_consumed_code_cannot_mint_a_second_token() {
        let (app, db) = test_oauth_app();
        let v = request_device_code(&app, None).await;
        let device_code = v["device_code"].as_str().unwrap().to_string();
        let hash = sha256_hex(device_code.as_bytes());

        approve_device_code(&db, &hash);
        let (status, body) = poll_device_token(&app, &device_code).await;
        assert_eq!(status, StatusCode::OK, "expected token, got {body}");
        // Clients read `scope` off the token response; it stays on the wire.
        assert_eq!(body["scope"], "mcp");
        assert_eq!(token_count(&db), 1);
        assert_eq!(device_status(&db, &hash), "consumed");

        // Replay the same code (interval waited): no second token, ever.
        {
            let conn = db.write().unwrap();
            conn.execute(
                "UPDATE oauth_device_codes SET last_polled_at = NULL WHERE device_code_hash = ?1",
                params![hash],
            )
            .unwrap();
        }
        let (status, body) = poll_device_token(&app, &device_code).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid_grant");
        assert_eq!(token_count(&db), 1, "replay must not mint a second token");
    }

    #[tokio::test]
    async fn device_token_is_rolled_back_when_the_code_cannot_be_consumed() {
        // A failing consume-UPDATE used to be swallowed (`let _ = ...`),
        // handing out a token while leaving the code approved and replayable.
        // Now the whole exchange fails and nothing is written.
        let (app, db) = test_oauth_app();
        let v = request_device_code(&app, None).await;
        let device_code = v["device_code"].as_str().unwrap().to_string();
        let hash = sha256_hex(device_code.as_bytes());

        approve_device_code(&db, &hash);
        {
            let conn = db.write().unwrap();
            conn.execute_batch(
                "CREATE TRIGGER block_consume
                 BEFORE UPDATE OF status ON oauth_device_codes
                 WHEN NEW.status = 'consumed'
                 BEGIN SELECT RAISE(ABORT, 'consume blocked'); END;",
            )
            .unwrap();
        }

        let (status, _) = poll_device_token(&app, &device_code).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(token_count(&db), 0, "no token may survive a failed consume");
        assert_eq!(device_status(&db, &hash), "approved");
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

    // ── resolve_tool (LIFIC-13) ────────────────────────────────

    #[test]
    fn resolve_tool_known_registry_id_keeps_display_name() {
        // A pick from the Connected Tools registry maps to its display name.
        let (id, display) = resolve_tool("claude-code").unwrap();
        assert_eq!(id, "claude-code");
        assert_eq!(display, "Claude Code");
    }

    #[test]
    fn resolve_tool_unregistered_tool_is_sanitized() {
        // Free text gets lowercased and stripped to the id, display falls back
        // to the same humanized text.
        let (id, display) = resolve_tool("My Editor").unwrap();
        assert_eq!(id, "my-editor");
        assert_eq!(display, "My Editor");
    }

    #[test]
    fn resolve_tool_rejects_reserved_words() {
        for reserved in ["admin", "system"] {
            assert!(
                resolve_tool(reserved).is_err(),
                "{reserved} is a reserved tool id"
            );
        }
    }

    #[test]
    fn resolve_tool_rejects_empty_or_only_symbols() {
        assert!(resolve_tool("").is_err());
        assert!(resolve_tool("   ").is_err());
    }

    /// One snapshot, one answer.
    ///
    /// The middleware used to ask three questions on three pooled connections:
    /// is the token valid, whose is it, is that user live. Between the first
    /// two, a revocation could land, and the pair then read "valid" and
    /// "unbound". An unbound OAuth token takes the operator fallback, so
    /// revoking a tool's credential could promote it to the first admin.
    /// `resolve_oauth_credential` makes that unrepresentable: the outcome is
    /// one value from one connection.
    mod credential_resolution {
        use super::*;

        struct Fixture {
            db: DbPool,
            owner_id: i64,
            bot_id: i64,
            token: String,
        }

        fn fixture() -> Fixture {
            let db = crate::db::open_memory().unwrap();
            let (owner_id, bot_id) = {
                let conn = db.write().unwrap();
                let owner = crate::db::queries::users::create_user(
                    &conn,
                    &crate::db::models::CreateUser {
                        username: "owner".into(),
                        email: "owner@test.local".into(),
                        password: "testpassword1".into(),
                        display_name: None,
                        is_admin: true,
                        is_bot: false,
                    },
                )
                .unwrap();
                let bot = crate::db::queries::users::create_bot_user(
                    &conn,
                    owner.id,
                    "zed-owner",
                    "Zed",
                    Some("zed"),
                )
                .unwrap();
                conn.execute(
                    "INSERT INTO oauth_clients (client_id, client_name, redirect_uris)
                     VALUES ('c', 'Test', '[\"http://localhost\"]')",
                    [],
                )
                .unwrap();
                (owner.id, bot.id)
            };
            let token = insert_token(&db, "bound", Some(bot_id));
            Fixture {
                db,
                owner_id,
                bot_id,
                token,
            }
        }

        fn insert_token(db: &DbPool, suffix: &str, user_id: Option<i64>) -> String {
            let token = format!("lific_at_{suffix}");
            let hash = sha256_hex(token.as_bytes());
            let expires = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
            db.write()
                .unwrap()
                .execute(
                    "INSERT INTO oauth_tokens (access_token, client_id, expires_at, scope, user_id)
                     VALUES (?1, 'c', ?2, 'mcp', ?3)",
                    params![hash, expires, user_id],
                )
                .unwrap();
            token
        }

        #[test]
        fn a_live_bound_token_resolves_to_its_user() {
            let f = fixture();
            assert_eq!(
                resolve_oauth_credential(&f.db, &f.token),
                Ok(OAuthCredential::Bound(crate::db::models::AuthUser {
                    id: f.bot_id,
                    username: "zed-owner".into(),
                    display_name: "Zed".into(),
                    is_admin: false,
                })),
            );
        }

        /// The escalation this consolidation exists to prevent. Whatever the
        /// state, a bound token can never come back as unbound.
        #[test]
        fn a_bound_token_never_degrades_to_unbound() {
            for (label, mutate) in [
                (
                    "revoked",
                    Box::new(|f: &Fixture| {
                        f.db.write()
                            .unwrap()
                            .execute("UPDATE oauth_tokens SET revoked = 1", [])
                            .unwrap();
                    }) as Box<dyn Fn(&Fixture)>,
                ),
                (
                    "expired",
                    Box::new(|f: &Fixture| {
                        let past = (chrono::Utc::now() - chrono::Duration::minutes(1)).to_rfc3339();
                        f.db.write()
                            .unwrap()
                            .execute("UPDATE oauth_tokens SET expires_at = ?1", params![past])
                            .unwrap();
                    }),
                ),
                (
                    "bot deleted",
                    Box::new(|f: &Fixture| {
                        f.db.write()
                            .unwrap()
                            .execute("DELETE FROM users WHERE id = ?1", params![f.bot_id])
                            .unwrap();
                    }),
                ),
                (
                    "owner deactivated",
                    Box::new(|f: &Fixture| {
                        let conn = f.db.write().unwrap();
                        crate::db::queries::users::create_user(
                            &conn,
                            &crate::db::models::CreateUser {
                                username: "spare".into(),
                                email: "spare@test.local".into(),
                                password: "testpassword1".into(),
                                display_name: None,
                                is_admin: true,
                                is_bot: false,
                            },
                        )
                        .unwrap();
                        crate::db::queries::users::set_active(&conn, f.owner_id, false).unwrap();
                    }),
                ),
            ] {
                let f = fixture();
                mutate(&f);
                let outcome = resolve_oauth_credential(&f.db, &f.token);
                assert!(
                    outcome.is_err(),
                    "{label}: must not authenticate, got {outcome:?}"
                );
                assert_ne!(
                    outcome,
                    Ok(OAuthCredential::LegacyUnbound),
                    "{label}: a dead binding must never read as unbound, which is the operator"
                );
            }
        }

        /// The documented pre-LIF-79 behaviour, kept for rows that predate
        /// user binding. Nothing issued since can be in this state.
        #[test]
        fn a_genuinely_unbound_legacy_token_still_resolves_to_the_operator_fallback() {
            let f = fixture();
            let legacy = insert_token(&f.db, "legacy", None);
            assert_eq!(
                resolve_oauth_credential(&f.db, &legacy),
                Ok(OAuthCredential::LegacyUnbound)
            );
        }

        #[test]
        fn an_unknown_or_wrong_shaped_token_is_invalid() {
            let f = fixture();
            for token in ["lific_at_never-issued", "lific_sess_wrong-shape", ""] {
                assert_eq!(
                    resolve_oauth_credential(&f.db, token),
                    Err(OAuthReject::Invalid),
                    "{token}"
                );
            }
        }

        /// Through the real middleware, which is where the escalation would
        /// have happened: a revoked bot token must 401, not arrive as the
        /// first admin.
        #[tokio::test]
        async fn a_revoked_bot_token_is_refused_by_the_middleware_not_promoted() {
            use tower::ServiceExt;
            let f = fixture();
            let auth_state = crate::auth::AuthState {
                db: f.db.clone(),
                manager: crate::auth::create_key_manager().unwrap(),
                public_url: "https://example.com".into(),
                required: true,
            };
            let app = crate::api::router(f.db.clone(), &[])
                .layer(axum::Extension(crate::realtime::RealtimeHub::new()))
                .layer(axum::Extension(crate::config::AuthConfig {
                    allow_signup: true,
                    required: true,
                    secure_cookies: false,
                }))
                .layer(axum::middleware::from_fn_with_state(
                    auth_state,
                    crate::auth::require_api_key,
                ));

            let call = |token: String| {
                let app = app.clone();
                async move {
                    app.oneshot(
                        Request::builder()
                            .uri("/api/auth/me")
                            .header("authorization", format!("Bearer {token}"))
                            .body(axum::body::Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap()
                }
            };

            assert_eq!(call(f.token.clone()).await.status(), StatusCode::OK);

            f.db.write()
                .unwrap()
                .execute("UPDATE oauth_tokens SET revoked = 1", [])
                .unwrap();

            let resp = call(f.token.clone()).await;
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "a revoked token must not authenticate at all, let alone as the operator"
            );
        }
    }

    /// Every OAuth expiry column is written with `to_rfc3339`, and SQLite's
    /// `datetime('now')` is not that format. Compared as raw text they
    /// disagree within the same day: 'T' sorts after every digit, so
    /// '2026-08-20T11:59:00+00:00' reads as later than '2026-08-20 12:00:00'
    /// and an expired grant looks live. Every predicate wraps the column in
    /// `datetime()`; these prove it, a minute either side of now so nothing
    /// rests on a second boundary.
    mod expiry_is_compared_as_a_datetime {
        use super::*;

        fn rfc3339_from_now(minutes: i64) -> String {
            (chrono::Utc::now() + chrono::Duration::minutes(minutes)).to_rfc3339()
        }

        fn seeded_db() -> DbPool {
            let db = crate::db::open_memory().unwrap();
            db.write()
                .unwrap()
                .execute(
                    "INSERT INTO oauth_clients (client_id, client_name, redirect_uris)
                     VALUES ('c', 'Test', '[\"http://localhost\"]')",
                    [],
                )
                .unwrap();
            db
        }

        fn insert_token(db: &DbPool, name: &str, minutes: i64) -> String {
            let token = format!("lific_at_{name}");
            db.write()
                .unwrap()
                .execute(
                    "INSERT INTO oauth_tokens (access_token, client_id, expires_at, scope)
                     VALUES (?1, 'c', ?2, 'mcp')",
                    params![sha256_hex(token.as_bytes()), rfc3339_from_now(minutes)],
                )
                .unwrap();
            token
        }

        #[test]
        fn an_access_token_expiring_in_a_minute_is_still_valid() {
            let db = seeded_db();
            let token = insert_token(&db, "live", 1);
            assert!(resolve_oauth_credential(&db, &token).is_ok());
        }

        #[test]
        fn an_access_token_that_expired_a_minute_ago_is_refused() {
            let db = seeded_db();
            let token = insert_token(&db, "dead", -1);
            assert_eq!(
                resolve_oauth_credential(&db, &token),
                Err(OAuthReject::Invalid)
            );
        }

        fn insert_code(db: &DbPool, code: &str, minutes: i64) {
            db.write()
                .unwrap()
                .execute(
                    "INSERT INTO oauth_codes
                        (code, client_id, redirect_uri, code_challenge, expires_at, user_id)
                     VALUES (?1, 'c', 'http://localhost', 'x', ?2, NULL)",
                    params![code, rfc3339_from_now(minutes)],
                )
                .unwrap();
        }

        fn code_is_visible(db: &DbPool, code: &str) -> bool {
            db.read()
                .unwrap()
                .query_row(
                    "SELECT 1 FROM oauth_codes
                     WHERE code = ?1 AND datetime(expires_at) > datetime('now')",
                    params![code],
                    |_| Ok(()),
                )
                .is_ok()
        }

        #[test]
        fn an_authorization_code_expiring_in_a_minute_is_still_exchangeable() {
            let db = seeded_db();
            insert_code(&db, "live-code", 1);
            assert!(code_is_visible(&db, "live-code"));
        }

        #[test]
        fn an_authorization_code_that_expired_a_minute_ago_is_gone() {
            let db = seeded_db();
            insert_code(&db, "dead-code", -1);
            assert!(!code_is_visible(&db, "dead-code"));
        }

        fn insert_device_code(db: &DbPool, hash: &str, user_code: &str, minutes: i64) {
            db.write()
                .unwrap()
                .execute(
                    "INSERT INTO oauth_device_codes
                        (device_code_hash, user_code, expires_at, status)
                     VALUES (?1, ?2, ?3, 'pending')",
                    params![hash, user_code, rfc3339_from_now(minutes)],
                )
                .unwrap();
        }

        fn device_codes(db: &DbPool) -> i64 {
            db.read()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM oauth_device_codes", [], |r| r.get(0))
                .unwrap()
        }

        /// The opportunistic sweep must remove the expired one and keep the
        /// live one. Reading these as text does the opposite within a day.
        #[test]
        fn the_device_code_sweep_removes_only_the_expired_one() {
            let db = seeded_db();
            insert_device_code(&db, "live", "BCDF-GHJK", 1);
            insert_device_code(&db, "dead", "BCDF-GHJL", -1);
            assert_eq!(device_codes(&db), 2);

            cleanup_expired_device_codes(&db);

            assert_eq!(device_codes(&db), 1, "only the expired grant is swept");
            let survivor: String = db
                .read()
                .unwrap()
                .query_row("SELECT device_code_hash FROM oauth_device_codes", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(survivor, "live");
        }

        /// Approval only acts on a code that has not expired.
        #[test]
        fn only_an_unexpired_device_code_can_be_approved() {
            let db = seeded_db();
            insert_device_code(&db, "live", "BCDF-GHJK", 1);
            insert_device_code(&db, "dead", "BCDF-GHJL", -1);

            let approve = |user_code: &str| -> usize {
                db.write()
                    .unwrap()
                    .execute(
                        "UPDATE oauth_device_codes SET status = 'approved'
                         WHERE user_code = ?1 AND status = 'pending'
                           AND datetime(expires_at) > datetime('now')",
                        params![user_code],
                    )
                    .unwrap()
            };
            assert_eq!(approve("BCDF-GHJK"), 1, "the live grant is approvable");
            assert_eq!(approve("BCDF-GHJL"), 0, "the expired grant is not");
        }
    }

    // ── Grants may not outlive the authorization that produced them ──
    //
    // Every test here is about ordering: a grant that was legitimately issued,
    // then invalidated by an account recovery before the client got round to
    // exchanging it. The approval and the exchange each run as one
    // transaction, so "before" and "after" are the only two orders that exist;
    // these pin what each one produces.
    mod grant_lifetime {
        use super::*;

        const VERIFIER: &str = "test_verifier_abcdefghijklmnopqrstuvwxyz_0123456789";

        fn challenge() -> String {
            base64_url_encode(&Sha256::digest(VERIFIER.as_bytes()))
        }

        fn owner_id(db: &DbPool) -> i64 {
            db.read()
                .unwrap()
                .query_row(
                    "SELECT id FROM users WHERE username = 'oauthtest'",
                    [],
                    |r| r.get(0),
                )
                .unwrap()
        }

        /// Run the authorize POST with a live session cookie and return the
        /// issued code from the redirect.
        async fn approve_code(app: &Router, client_id: &str, session: &str) -> String {
            let body = format!(
                "client_id={}&redirect_uri={}&response_type=code&code_challenge={}&code_challenge_method=S256&scope=mcp&csrf_token={}&tool=claude-code",
                client_id,
                urlencoding::encode("http://localhost/callback"),
                urlencoding::encode(&challenge()),
                urlencoding::encode(&generate_csrf_token(session)),
            );
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/oauth/authorize")
                        .header("content-type", "application/x-www-form-urlencoded")
                        .header("cookie", format!("lific_token={session}"))
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
            location
                .split("code=")
                .nth(1)
                .unwrap()
                .split('&')
                .next()
                .unwrap()
                .to_string()
        }

        async fn exchange_code(
            app: &Router,
            client_id: &str,
            code: &str,
        ) -> (StatusCode, serde_json::Value) {
            let body = format!(
                "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}",
                code,
                urlencoding::encode("http://localhost/callback"),
                client_id,
                VERIFIER,
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
            (
                status,
                serde_json::from_slice(&bytes).unwrap_or(serde_json::json!({})),
            )
        }

        #[tokio::test]
        async fn an_ordinary_authorization_code_flow_still_works() {
            let (app, db) = test_oauth_app();
            let session = create_test_session(&db);
            let client_id = register_client_helper(&app, "http://localhost/callback").await;

            let code = approve_code(&app, &client_id, &session).await;
            let (status, body) = exchange_code(&app, &client_id, &code).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert!(
                body["access_token"]
                    .as_str()
                    .unwrap()
                    .starts_with("lific_at_")
            );
        }

        #[tokio::test]
        async fn a_code_approved_before_a_lockdown_cannot_mint_after_it() {
            let (app, db) = test_oauth_app();
            let session = create_test_session(&db);
            let client_id = register_client_helper(&app, "http://localhost/callback").await;
            let code = approve_code(&app, &client_id, &session).await;

            {
                let conn = db.write().unwrap();
                crate::db::queries::users::lock_down_account(&conn, owner_id(&db)).unwrap();
            }

            let (status, body) = exchange_code(&app, &client_id, &code).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(body["error"], "invalid_grant");

            let tokens: i64 = db
                .read()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM oauth_tokens", [], |r| r.get(0))
                .unwrap();
            assert_eq!(tokens, 0, "no token was minted");
        }

        #[tokio::test]
        async fn a_code_whose_bot_owner_went_inactive_cannot_be_exchanged() {
            let (app, db) = test_oauth_app();
            let session = create_test_session(&db);
            let client_id = register_client_helper(&app, "http://localhost/callback").await;
            let code = approve_code(&app, &client_id, &session).await;

            {
                let conn = db.write().unwrap();
                crate::db::queries::users::set_active(&conn, owner_id(&db), false).unwrap();
            }

            let (status, body) = exchange_code(&app, &client_id, &code).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(body["error"], "invalid_grant");
            assert_eq!(
                body["error_description"],
                "authorizing user is no longer active"
            );
        }

        /// A code with `user_id IS NULL` is a pre-LIF-79 legacy row. Nothing
        /// issues one any more; exchanging one used to mint an *unbound*
        /// access token, which resolves as the operator and which no account
        /// lockdown can revoke, because a lockdown scopes by user id.
        #[tokio::test]
        async fn a_legacy_unbound_code_cannot_be_exchanged() {
            let (app, db) = test_oauth_app();
            let client_id = register_client_helper(&app, "http://localhost/callback").await;

            // Otherwise valid in every respect: unexpired, unused, correct
            // client and redirect, and a PKCE challenge the verifier matches.
            db.write()
                .unwrap()
                .execute(
                    "INSERT INTO oauth_codes
                        (code, client_id, redirect_uri, code_challenge, code_challenge_method,
                         expires_at, scope, user_id)
                     VALUES ('legacy-code', ?1, 'http://localhost/callback', ?2, 'S256',
                             ?3, 'mcp', NULL)",
                    params![
                        client_id,
                        challenge(),
                        (chrono::Utc::now() + chrono::Duration::minutes(10)).to_rfc3339(),
                    ],
                )
                .unwrap();

            let (status, body) = exchange_code(&app, &client_id, "legacy-code").await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(body["error"], "invalid_grant");

            let conn = db.read().unwrap();
            let tokens: i64 = conn
                .query_row("SELECT COUNT(*) FROM oauth_tokens", [], |r| r.get(0))
                .unwrap();
            assert_eq!(tokens, 0, "no unbound token was minted");
            let used: i64 = conn
                .query_row(
                    "SELECT used FROM oauth_codes WHERE code = 'legacy-code'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(used, 0, "the refused exchange rolled back cleanly");
        }

        /// The device twin of the above: an approved row that names nobody.
        #[tokio::test]
        async fn a_legacy_unbound_device_approval_cannot_be_exchanged() {
            let (app, db) = test_oauth_app();
            let v = request_device_code(&app, Some("My CLI")).await;
            db.write()
                .unwrap()
                .execute(
                    "UPDATE oauth_device_codes
                     SET status = 'approved', user_id = NULL, last_polled_at = NULL",
                    [],
                )
                .unwrap();

            let (status, body) = poll_device_token(&app, v["device_code"].as_str().unwrap()).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(body["error"], "invalid_grant");

            let conn = db.read().unwrap();
            let tokens: i64 = conn
                .query_row("SELECT COUNT(*) FROM oauth_tokens", [], |r| r.get(0))
                .unwrap();
            assert_eq!(tokens, 0, "no unbound token was minted");
            let status: String = conn
                .query_row("SELECT status FROM oauth_device_codes", [], |r| r.get(0))
                .unwrap();
            assert_eq!(
                status, "approved",
                "the grant is not consumed by a refused exchange"
            );
            // The poll bookkeeping the refusal still owes the client did
            // commit, so the next poll is rate-limited as usual rather than
            // being treated as a first poll.
            let polled: Option<String> = conn
                .query_row("SELECT last_polled_at FROM oauth_device_codes", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert!(polled.is_some());
        }

        /// Approving a connection mints a 30-day tool credential, so it needs
        /// the same recent sign-in `POST /api/auth/keys` needs. A session
        /// token lifted from a browser that signed in last week must not be
        /// enough to attach a permanent credential to the account.
        #[tokio::test]
        async fn an_aged_session_may_not_approve_an_authorization_request() {
            let (app, db) = test_oauth_app();
            let session = create_test_session(&db);
            let client_id = register_client_helper(&app, "http://localhost/callback").await;
            db.write()
                .unwrap()
                .execute(
                    "UPDATE sessions SET created_at = datetime('now', '-16 minutes')",
                    [],
                )
                .unwrap();

            let body = format!(
                "client_id={}&redirect_uri={}&response_type=code&code_challenge={}&code_challenge_method=S256&scope=mcp&csrf_token={}&tool=claude-code",
                client_id,
                urlencoding::encode("http://localhost/callback"),
                urlencoding::encode(&challenge()),
                urlencoding::encode(&generate_csrf_token(&session)),
            );
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/oauth/authorize")
                        .header("content-type", "application/x-www-form-urlencoded")
                        .header("cookie", format!("lific_token={session}"))
                        .body(axum::body::Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
            let page = String::from_utf8(
                resp.into_body()
                    .collect()
                    .await
                    .unwrap()
                    .to_bytes()
                    .to_vec(),
            )
            .unwrap();
            // The copy has to be actionable and honest: name the window, say
            // that signing out is what clears the stale session, and say the
            // connection must be restarted from the client. It must NOT claim
            // the approval will be retried automatically, because it will not.
            for phrase in [
                "15 minutes",
                "sign out",
                "Sign back in",
                "Start the connection again from your MCP client",
                "nothing has changed",
            ] {
                assert!(
                    page.to_lowercase().contains(&phrase.to_lowercase()),
                    "the page must say {phrase:?}: {page}"
                );
            }
            for forbidden in ["automatically", "will be retried", "try again shortly"] {
                assert!(
                    !page.to_lowercase().contains(forbidden),
                    "the page must not promise a retry it does not do: {page}"
                );
            }

            let conn = db.read().unwrap();
            let codes: i64 = conn
                .query_row("SELECT COUNT(*) FROM oauth_codes", [], |r| r.get(0))
                .unwrap();
            assert_eq!(codes, 0, "no code was issued");
            let bots: i64 = conn
                .query_row("SELECT COUNT(*) FROM users WHERE is_bot = 1", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(bots, 0, "no bot was minted either");
            // The session itself is untouched; only this action needed more.
            assert!(crate::db::queries::users::validate_session(&conn, &session).is_ok());
        }

        #[tokio::test]
        async fn an_aged_session_may_not_approve_a_device() {
            let (app, db) = test_oauth_app();
            let session = create_test_session(&db);
            let v = request_device_code(&app, Some("My CLI")).await;
            db.write()
                .unwrap()
                .execute(
                    "UPDATE sessions SET created_at = datetime('now', '-16 minutes')",
                    [],
                )
                .unwrap();

            let body = format!(
                "user_code={}&decision=approve&csrf_token={}&tool=claude-code",
                urlencoding::encode(v["user_code"].as_str().unwrap()),
                urlencoding::encode(&generate_csrf_token(&session)),
            );
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/oauth/device")
                        .header("content-type", "application/x-www-form-urlencoded")
                        .header("cookie", format!("lific_token={session}"))
                        .body(axum::body::Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

            let conn = db.read().unwrap();
            let pending: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM oauth_device_codes WHERE status = 'pending'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(pending, 1, "the grant is still waiting, not approved");
            let bots: i64 = conn
                .query_row("SELECT COUNT(*) FROM users WHERE is_bot = 1", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(bots, 0);
        }


        /// Refusing a device is not a grant, so it must not be made harder
        /// than approving one. Someone who sees a code they do not recognise
        /// should be able to deny it immediately, from whatever session they
        /// already have open.
        #[tokio::test]
        async fn an_aged_session_may_still_deny_a_device() {
            let (app, db) = test_oauth_app();
            let session = create_test_session(&db);
            let v = request_device_code(&app, Some("Unknown device")).await;
            db.write()
                .unwrap()
                .execute(
                    "UPDATE sessions SET created_at = datetime('now', '-16 minutes')",
                    [],
                )
                .unwrap();

            let body = format!(
                "user_code={}&decision=deny&csrf_token={}",
                urlencoding::encode(v["user_code"].as_str().unwrap()),
                urlencoding::encode(&generate_csrf_token(&session)),
            );
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/oauth/device")
                        .header("content-type", "application/x-www-form-urlencoded")
                        .header("cookie", format!("lific_token={session}"))
                        .body(axum::body::Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);

            let conn = db.read().unwrap();
            let denied: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM oauth_device_codes WHERE status = 'denied'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(denied, 1, "the grant is refused");
            let bots: i64 = conn
                .query_row("SELECT COUNT(*) FROM users WHERE is_bot = 1", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(bots, 0, "denying resolves no tool and mints no bot");

            // The exchange sees the denial.
            drop(conn);
            db.write()
                .unwrap()
                .execute("UPDATE oauth_device_codes SET last_polled_at = NULL", [])
                .unwrap();
            let (status, body) = poll_device_token(&app, v["device_code"].as_str().unwrap()).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(body["error"], "access_denied");
        }

        /// A dead session may not deny either: the page still has to know who
        /// is refusing.
        #[tokio::test]
        async fn a_revoked_session_may_not_deny_a_device() {
            let (app, db) = test_oauth_app();
            let session = create_test_session(&db);
            let v = request_device_code(&app, None).await;
            db.write()
                .unwrap()
                .execute("DELETE FROM sessions", [])
                .unwrap();

            let body = format!(
                "user_code={}&decision=deny&csrf_token={}",
                urlencoding::encode(v["user_code"].as_str().unwrap()),
                urlencoding::encode(&generate_csrf_token(&session)),
            );
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/oauth/device")
                        .header("content-type", "application/x-www-form-urlencoded")
                        .header("cookie", format!("lific_token={session}"))
                        .body(axum::body::Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
            let pending: i64 = db
                .read()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM oauth_device_codes WHERE status = 'pending'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(pending, 1);
        }

        #[tokio::test]
        async fn an_oauth_token_may_not_approve_an_authorization_request() {
            let (app, db) = test_oauth_app();
            let session = create_test_session(&db);
            let client_id = register_client_helper(&app, "http://localhost/callback").await;

            // A perfectly valid access token for the same account.
            let access_token = {
                let token = "lific_at_tool-held-token".to_string();
                let hash = crate::auth::sha256_hex(token.as_bytes());
                let expires = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
                let conn = db.write().unwrap();
                conn.execute(
                    "INSERT INTO oauth_tokens (access_token, client_id, expires_at, scope, user_id)
                     VALUES (?1, ?2, ?3, 'mcp', ?4)",
                    params![hash, client_id, expires, owner_id(&db)],
                )
                .unwrap();
                token
            };
            let _ = session;

            let body = format!(
                "client_id={}&redirect_uri={}&response_type=code&code_challenge={}&code_challenge_method=S256&scope=mcp&csrf_token={}&tool=claude-code",
                client_id,
                urlencoding::encode("http://localhost/callback"),
                urlencoding::encode(&challenge()),
                urlencoding::encode(&generate_csrf_token(&access_token)),
            );
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/oauth/authorize")
                        .header("content-type", "application/x-www-form-urlencoded")
                        .header("authorization", format!("Bearer {access_token}"))
                        .body(axum::body::Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

            let codes: i64 = db
                .read()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM oauth_codes", [], |r| r.get(0))
                .unwrap();
            assert_eq!(codes, 0, "no code was issued");
        }

        #[tokio::test]
        async fn a_revoked_session_cannot_approve_and_mints_no_bot() {
            let (app, db) = test_oauth_app();
            let session = create_test_session(&db);
            let client_id = register_client_helper(&app, "http://localhost/callback").await;
            db.write()
                .unwrap()
                .execute("DELETE FROM sessions", [])
                .unwrap();

            let body = format!(
                "client_id={}&redirect_uri={}&response_type=code&code_challenge={}&code_challenge_method=S256&scope=mcp&csrf_token={}&tool=claude-code",
                client_id,
                urlencoding::encode("http://localhost/callback"),
                urlencoding::encode(&challenge()),
                urlencoding::encode(&generate_csrf_token(&session)),
            );
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/oauth/authorize")
                        .header("content-type", "application/x-www-form-urlencoded")
                        .header("cookie", format!("lific_token={session}"))
                        .body(axum::body::Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

            let conn = db.read().unwrap();
            let codes: i64 = conn
                .query_row("SELECT COUNT(*) FROM oauth_codes", [], |r| r.get(0))
                .unwrap();
            assert_eq!(codes, 0);
            let bots: i64 = conn
                .query_row("SELECT COUNT(*) FROM users WHERE is_bot = 1", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(bots, 0, "a refused approval mints no bot either");
        }

        // ── Device grant ────────────────────────────────────────

        /// Approve a device code through the real verification page, bound to
        /// the tool bot, then clear `last_polled_at` so the next poll is not
        /// answered with `slow_down`.
        async fn approve_device(app: &Router, db: &DbPool, session: &str, user_code: &str) {
            let body = format!(
                "user_code={}&decision=approve&csrf_token={}&tool=claude-code",
                urlencoding::encode(user_code),
                urlencoding::encode(&generate_csrf_token(session)),
            );
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/oauth/device")
                        .header("content-type", "application/x-www-form-urlencoded")
                        .header("cookie", format!("lific_token={session}"))
                        .body(axum::body::Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            db.write()
                .unwrap()
                .execute("UPDATE oauth_device_codes SET last_polled_at = NULL", [])
                .unwrap();
        }

        #[tokio::test]
        async fn an_ordinary_device_flow_still_works() {
            let (app, db) = test_oauth_app();
            let session = create_test_session(&db);
            let v = request_device_code(&app, Some("My CLI")).await;
            approve_device(&app, &db, &session, v["user_code"].as_str().unwrap()).await;

            let (status, body) = poll_device_token(&app, v["device_code"].as_str().unwrap()).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert!(
                body["access_token"]
                    .as_str()
                    .unwrap()
                    .starts_with("lific_at_")
            );
        }

        #[tokio::test]
        async fn a_device_approved_before_a_lockdown_is_denied_after_it() {
            let (app, db) = test_oauth_app();
            let session = create_test_session(&db);
            let v = request_device_code(&app, Some("My CLI")).await;
            approve_device(&app, &db, &session, v["user_code"].as_str().unwrap()).await;

            {
                let conn = db.write().unwrap();
                crate::db::queries::users::lock_down_account(&conn, owner_id(&db)).unwrap();
            }

            let (status, body) = poll_device_token(&app, v["device_code"].as_str().unwrap()).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(body["error"], "access_denied", "the denial wins cleanly");

            let tokens: i64 = db
                .read()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM oauth_tokens", [], |r| r.get(0))
                .unwrap();
            assert_eq!(tokens, 0);
        }

        #[tokio::test]
        async fn a_device_bound_to_a_bot_with_an_inactive_owner_cannot_exchange() {
            let (app, db) = test_oauth_app();
            let session = create_test_session(&db);
            let v = request_device_code(&app, Some("My CLI")).await;
            approve_device(&app, &db, &session, v["user_code"].as_str().unwrap()).await;

            {
                let conn = db.write().unwrap();
                crate::db::queries::users::set_active(&conn, owner_id(&db), false).unwrap();
            }

            let (status, body) = poll_device_token(&app, v["device_code"].as_str().unwrap()).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(body["error"], "invalid_grant");
            assert_eq!(
                body["error_description"],
                "authorizing user is no longer active"
            );
        }

        #[tokio::test]
        async fn an_oauth_token_may_not_approve_a_device() {
            let (app, db) = test_oauth_app();
            let session = create_test_session(&db);
            let v = request_device_code(&app, Some("My CLI")).await;

            let access_token = {
                let token = "lific_at_device-approver".to_string();
                let hash = crate::auth::sha256_hex(token.as_bytes());
                let expires = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
                let conn = db.write().unwrap();
                conn.execute(
                    "INSERT OR IGNORE INTO oauth_clients (client_id, client_name, redirect_uris)
                     VALUES ('device', 'Device Authorization', '[]')",
                    [],
                )
                .unwrap();
                conn.execute(
                    "INSERT INTO oauth_tokens (access_token, client_id, expires_at, scope, user_id)
                     VALUES (?1, 'device', ?2, 'mcp', ?3)",
                    params![hash, expires, owner_id(&db)],
                )
                .unwrap();
                token
            };
            let _ = session;

            let body = format!(
                "user_code={}&decision=approve&csrf_token={}&tool=claude-code",
                urlencoding::encode(v["user_code"].as_str().unwrap()),
                urlencoding::encode(&generate_csrf_token(&access_token)),
            );
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/oauth/device")
                        .header("content-type", "application/x-www-form-urlencoded")
                        .header("authorization", format!("Bearer {access_token}"))
                        .body(axum::body::Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

            let still_pending: i64 = db
                .read()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM oauth_device_codes WHERE status = 'pending'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(still_pending, 1);
        }
    }
}
