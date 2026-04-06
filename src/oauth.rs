use axum::{
    Router,
    extract::{Json, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::info;

use crate::db::DbPool;

#[derive(Clone)]
pub struct OAuthState {
    pub db: DbPool,
    pub issuer: String, // e.g. https://fedora.tailb93ac8.ts.net/lific
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

async fn protected_resource_metadata(State(state): State<OAuthState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "resource": state.issuer,
        "authorization_servers": [state.issuer],
        "scopes_supported": ["mcp"],
        "bearer_methods_supported": ["header"]
    }))
}

async fn authorization_server_metadata(State(state): State<OAuthState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "issuer": state.issuer,
        "authorization_endpoint": format!("{}/oauth/authorize", state.issuer),
        "token_endpoint": format!("{}/oauth/token", state.issuer),
        "registration_endpoint": format!("{}/oauth/register", state.issuer),
        "revocation_endpoint": format!("{}/oauth/revoke", state.issuer),
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
    Json(req): Json<RegisterRequest>,
) -> Response {
    if req.redirect_uris.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid_redirect_uri"})),
        )
            .into_response();
    }

    let client_id = uuid_v4();
    let client_name = req.client_name.unwrap_or_else(|| "MCP Client".into());
    let redirect_uris_json =
        serde_json::to_string(&req.redirect_uris).unwrap_or_else(|_| "[]".into());

    let db = state.db.clone();
    if let Ok(conn) = db.write() {
        let _ = conn.execute(
            "INSERT INTO oauth_clients (client_id, client_name, redirect_uris) VALUES (?1, ?2, ?3)",
            params![client_id, client_name, redirect_uris_json],
        );
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

async fn authorize_page(Query(params): Query<AuthorizeParams>) -> Html<String> {
    // Simple approval page -- single user, just click approve
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
}

async fn authorize_approve(
    State(oauth): State<OAuthState>,
    headers: axum::http::HeaderMap,
    axum::Form(form): axum::Form<ApproveForm>,
) -> Response {
    // Require authentication — the person approving must be identified.
    // Extract a session token from either the Authorization header or a cookie,
    // then validate it against the database to ensure it's a real, non-expired session.
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.trim().to_string())
        .or_else(|| {
            // For the browser form flow, extract the token from the lific_token cookie
            headers
                .get("cookie")
                .and_then(|v| v.to_str().ok())
                .and_then(|cookies| {
                    cookies.split(';').find_map(|c| {
                        let c = c.trim();
                        c.strip_prefix("lific_token=").map(|v| v.trim().to_string())
                    })
                })
        });

    let Some(token) = token else {
        return (
            StatusCode::UNAUTHORIZED,
            Html("<h1>Authentication required</h1><p>You must be signed in to approve OAuth access. <a href=\"/#/login\">Sign in</a></p>".to_string()),
        )
            .into_response();
    };

    // Actually validate the token against the database.
    // OAuth routes bypass the auth middleware, so we must validate here.
    let is_valid = if token.starts_with("lific_sess_") {
        let conn = match oauth.db.write() {
            Ok(c) => c,
            Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response(),
        };
        crate::db::queries::users::validate_session(&conn, &token).is_ok()
    } else if token.starts_with("lific_at_") {
        // OAuth tokens can also approve (valid authenticated identity)
        validate_oauth_token(&oauth.db, &token)
    } else {
        false
    };

    if !is_valid {
        return (
            StatusCode::UNAUTHORIZED,
            Html("<h1>Invalid session</h1><p>Your session has expired or is invalid. <a href=\"/#/login\">Sign in again</a></p>".to_string()),
        )
            .into_response();
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

    let code = uuid_v4();
    let expires = chrono::Utc::now() + chrono::Duration::minutes(10);
    let scope = form.scope.as_deref().unwrap_or("mcp");

    if let Ok(conn) = oauth.db.write() {
        let _ = conn.execute(
            "INSERT INTO oauth_codes (code, client_id, redirect_uri, code_challenge, code_challenge_method, expires_at, scope)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                code,
                form.client_id,
                form.redirect_uri,
                form.code_challenge.unwrap_or_default(),
                form.code_challenge_method.unwrap_or_else(|| "S256".into()),
                expires.to_rfc3339(),
                scope,
            ],
        );
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
    #[allow(dead_code)]
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

    let code_row: Result<(String, String, String, i64, String), _> = conn.query_row(
        "SELECT client_id, code_challenge, code_challenge_method, used, scope FROM oauth_codes WHERE code = ?1 AND expires_at > datetime('now')",
        params![code],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
    );

    let (stored_client_id, code_challenge, challenge_method, used, scope) = match code_row {
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

    // Validate client_id matches
    if let Some(client_id) = &req.client_id
        && *client_id != stored_client_id
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid_grant"})),
        )
            .into_response();
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
    let _ = conn.execute(
        "UPDATE oauth_codes SET used = 1 WHERE code = ?1",
        params![code],
    );

    // Generate access token
    let access_token = format!("lific_at_{}", uuid_v4());
    let expires_in: u64 = 3600 * 24 * 30; // 30 days
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(expires_in as i64);

    let _ = conn.execute(
        "INSERT INTO oauth_tokens (access_token, client_id, expires_at, scope) VALUES (?1, ?2, ?3, ?4)",
        params![access_token, stored_client_id, expires_at.to_rfc3339(), scope],
    );

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
    axum::Form(req): axum::Form<RevokeRequest>,
) -> Response {
    // RFC 7009 says the server MUST respond with 200 even if the token
    // is invalid, already revoked, or unrecognized — to prevent token scanning.
    if let Ok(conn) = state.db.write() {
        let _ = conn.execute(
            "UPDATE oauth_tokens SET revoked = 1 WHERE access_token = ?1",
            params![req.token],
        );
    }

    StatusCode::OK.into_response()
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn validate_pkce(verifier: &str, challenge: &str, method: &str) -> bool {
    match method {
        "S256" => {
            let hash = Sha256::digest(verifier.as_bytes());
            let computed = base64_url_encode(&hash);
            computed == challenge
        }
        "plain" => verifier == challenge,
        _ => false,
    }
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
pub fn validate_oauth_token_with_scope(db: &DbPool, token: &str) -> Option<String> {
    if !token.starts_with("lific_at_") {
        return None;
    }
    let conn = db.read().ok()?;
    conn.query_row(
        "SELECT scope FROM oauth_tokens
         WHERE access_token = ?1 AND revoked = 0 AND expires_at > datetime('now')",
        params![token],
        |row| row.get(0),
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_oauth_app() -> (Router, DbPool) {
        let db = crate::db::open_memory().expect("test db");
        let state = OAuthState {
            db: db.clone(),
            issuer: "https://example.com".into(),
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

    // ── LIF-48: authorize_approve validates tokens ───────────

    #[tokio::test]
    async fn authorize_rejects_missing_auth() {
        let (app, _db) = test_oauth_app();
        let client_id = register_client_helper(&app, "http://localhost/callback").await;

        let body = format!(
            "client_id={}&redirect_uri={}&response_type=code&code_challenge=abc&code_challenge_method=S256&scope=mcp",
            client_id,
            urlencoding::encode("http://localhost/callback"),
        );
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

        let body = format!(
            "client_id={}&redirect_uri={}&response_type=code&code_challenge=abc&code_challenge_method=S256&scope=mcp",
            client_id,
            urlencoding::encode("http://localhost/callback"),
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

        let body = format!(
            "client_id={}&redirect_uri={}&response_type=code&code_challenge=abc&code_challenge_method=S256&scope=mcp",
            client_id,
            urlencoding::encode("http://localhost/callback"),
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

        let body = format!(
            "client_id={}&redirect_uri={}&response_type=code&code_challenge=abc&code_challenge_method=S256&scope=mcp",
            client_id,
            urlencoding::encode("http://localhost/callback"),
        );
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

        let body = format!(
            "client_id={}&redirect_uri={}&response_type=code&code_challenge=abc&code_challenge_method=S256&scope=mcp",
            client_id,
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
        assert!(
            resp.status().is_redirection(),
            "expected redirect, got {}",
            resp.status()
        );
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

    // ── LIF-50: token revocation ─────────────────────────────

    #[tokio::test]
    async fn revoke_token_invalidates_access() {
        let (app, db) = test_oauth_app();

        // Manually insert a token to revoke
        let token = "lific_at_test-revoke-token";
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
                params![token, expires],
            ).unwrap();
        }

        // Token should be valid
        assert!(validate_oauth_token(&db, token));

        // Revoke it
        let body = format!("token={token}");
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
        assert_eq!(resp.status(), StatusCode::OK);

        // Token should now be invalid
        assert!(!validate_oauth_token(&db, token));
    }

    #[tokio::test]
    async fn revoke_unknown_token_returns_200() {
        let (app, _) = test_oauth_app();

        // RFC 7009: always return 200, even for unknown tokens
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
        let expires = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        {
            let conn = db.write().unwrap();
            conn.execute(
                "INSERT INTO oauth_clients (client_id, client_name, redirect_uris) VALUES ('scope-client', 'Test', '[\"http://localhost\"]')",
                [],
            ).unwrap();
            conn.execute(
                "INSERT INTO oauth_tokens (access_token, client_id, expires_at, scope) VALUES (?1, 'scope-client', ?2, 'mcp')",
                params![token, expires],
            ).unwrap();
        }

        let scope = validate_oauth_token_with_scope(&db, token);
        assert_eq!(scope, Some("mcp".to_string()));
    }

    #[tokio::test]
    async fn revoked_token_has_no_scope() {
        let (_, db) = test_oauth_app();

        let token = "lific_at_revoked-scope-test";
        let expires = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        {
            let conn = db.write().unwrap();
            conn.execute(
                "INSERT INTO oauth_clients (client_id, client_name, redirect_uris) VALUES ('rev-client', 'Test', '[\"http://localhost\"]')",
                [],
            ).unwrap();
            conn.execute(
                "INSERT INTO oauth_tokens (access_token, client_id, expires_at, scope, revoked) VALUES (?1, 'rev-client', ?2, 'mcp', 1)",
                params![token, expires],
            ).unwrap();
        }

        assert_eq!(validate_oauth_token_with_scope(&db, token), None);
        assert!(!validate_oauth_token(&db, token));
    }
}
