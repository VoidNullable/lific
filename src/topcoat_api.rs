//! Typed access to Lific's existing JSON API for the Topcoat frontend.

use reqwest::{
    Method, RequestBuilder, Response, StatusCode,
    header::{CONTENT_DISPOSITION, CONTENT_TYPE, HeaderMap},
    multipart::{Form, Part},
};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Frontend DTOs mirror the public JSON contract; they do not use DB models.
pub(crate) mod dto {
    pub(crate) mod project {
        use serde::{Deserialize, Serialize};

        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub(crate) struct Project {
            pub(crate) id: i64,
            pub(crate) name: String,
            pub(crate) identifier: String,
            pub(crate) description: String,
            #[serde(deserialize_with = "deserialize_required_nullable")]
            pub(crate) emoji: Option<String>,
            #[serde(deserialize_with = "deserialize_required_nullable")]
            pub(crate) lead_user_id: Option<i64>,
            pub(crate) sort_order: i64,
            pub(crate) created_at: String,
            pub(crate) updated_at: String,
            pub(crate) is_public: bool,
        }

        fn deserialize_required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
        where
            D: serde::Deserializer<'de>,
            T: serde::Deserialize<'de>,
        {
            Option::<T>::deserialize(deserializer)
        }
    }

    pub(crate) mod issue {
        use serde::{Deserialize, Serialize};

        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub(crate) struct Issue {
            pub(crate) id: i64,
            pub(crate) project_id: i64,
            pub(crate) sequence: i64,
            pub(crate) identifier: String,
            pub(crate) title: String,
            pub(crate) description: String,
            pub(crate) status: String,
            pub(crate) priority: String,
            #[serde(deserialize_with = "deserialize_required_nullable")]
            pub(crate) module_id: Option<i64>,
            pub(crate) sort_order: f64,
            #[serde(deserialize_with = "deserialize_required_nullable")]
            pub(crate) start_date: Option<String>,
            #[serde(deserialize_with = "deserialize_required_nullable")]
            pub(crate) target_date: Option<String>,
            pub(crate) created_at: String,
            pub(crate) updated_at: String,
            pub(crate) seq: i64,
            #[serde(skip_serializing_if = "Option::is_none")]
            pub(crate) source: Option<String>,
            pub(crate) labels: Vec<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            pub(crate) blocks: Option<Vec<String>>,
            #[serde(skip_serializing_if = "Option::is_none")]
            pub(crate) blocked_by: Option<Vec<String>>,
            #[serde(skip_serializing_if = "Option::is_none")]
            pub(crate) relates_to: Option<Vec<String>>,
            #[serde(skip_serializing_if = "Option::is_none")]
            pub(crate) duplicates: Option<Vec<String>>,
            #[serde(skip_serializing_if = "Option::is_none")]
            pub(crate) duplicated_by: Option<Vec<String>>,
            #[serde(skip_serializing_if = "Option::is_none")]
            pub(crate) waits: Option<Vec<IssueWait>>,
        }

        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub(crate) struct IssueWait {
            pub(crate) id: i64,
            pub(crate) issue_id: i64,
            pub(crate) kind: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            pub(crate) user_id: Option<i64>,
            #[serde(skip_serializing_if = "Option::is_none")]
            pub(crate) username: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            pub(crate) display_name: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            pub(crate) earliest: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            pub(crate) latest: Option<String>,
            pub(crate) note: String,
            pub(crate) state: String,
            pub(crate) created_at: String,
        }

        impl Issue {
            /// The issue's current sequence is the value sent as `expected_seq`
            /// when a later mutation opts into stale-write protection.
            pub(crate) fn expected_seq(&self) -> i64 {
                self.seq
            }
        }

        fn deserialize_required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
        where
            D: serde::Deserializer<'de>,
            T: serde::Deserialize<'de>,
        {
            Option::<T>::deserialize(deserializer)
        }
    }
}

/// A decoded API failure. Conflict responses retain `current` verbatim so a
/// caller can reconcile an `expected_seq` update without another GET.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ApiError {
    status: Option<StatusCode>,
    message: String,
    code: Option<String>,
    current: Option<Value>,
}

impl ApiError {
    pub(crate) fn status(&self) -> Option<StatusCode> {
        self.status
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    pub(crate) fn current(&self) -> Option<&Value> {
        self.current.as_ref()
    }

    pub(crate) fn is_update_conflict(&self) -> bool {
        self.code.as_deref() == Some("update_conflict")
    }

    fn from_response(status: StatusCode, body: &[u8]) -> Self {
        #[derive(serde::Deserialize)]
        struct ErrorBody {
            error: Option<String>,
            code: Option<String>,
            current: Option<Value>,
        }

        let decoded = serde_json::from_slice::<ErrorBody>(body).ok();
        Self {
            status: Some(status),
            message: decoded
                .as_ref()
                .and_then(|body| body.error.clone())
                .unwrap_or_else(|| format!("HTTP {status}")),
            code: decoded.as_ref().and_then(|body| body.code.clone()),
            current: decoded.and_then(|body| body.current),
        }
    }

    fn transport(error: reqwest::Error) -> Self {
        Self {
            status: None,
            message: error.to_string(),
            code: None,
            current: None,
        }
    }

    fn response_body(status: StatusCode, error: impl std::fmt::Display) -> Self {
        Self {
            status: Some(status),
            message: format!("Could not read HTTP response body: {error}"),
            code: Some("response_body".into()),
            current: None,
        }
    }

    fn decode(status: StatusCode, error: serde_json::Error) -> Self {
        Self {
            status: Some(status),
            message: format!("Invalid API response: {error}"),
            code: Some("invalid_response".into()),
            current: None,
        }
    }
}

/// Small HTTP client for Lific's existing `/api` routes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApiBaseUrl(reqwest::Url);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum ApiBaseUrlError {
    #[error("invalid API base URL")]
    InvalidUrl,
    #[error("API base URL must use HTTPS or loopback HTTP")]
    InsecureRemoteHttp,
    #[error("API base URL scheme must be HTTP or HTTPS")]
    UnsupportedScheme,
    #[error("API base URL must not contain credentials, a query, or a fragment")]
    CredentialsOrSuffixNotAllowed,
}

impl ApiBaseUrl {
    pub(crate) fn parse(base_url: &str) -> Result<Self, ApiBaseUrlError> {
        let mut url = reqwest::Url::parse(base_url).map_err(|_| ApiBaseUrlError::InvalidUrl)?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(ApiBaseUrlError::CredentialsOrSuffixNotAllowed);
        }

        match url.scheme() {
            "https" => {}
            "http" if is_loopback_host(&url) => {}
            "http" => return Err(ApiBaseUrlError::InsecureRemoteHttp),
            _ => return Err(ApiBaseUrlError::UnsupportedScheme),
        }

        let base_path = url.path().trim_end_matches('/').to_owned();
        url.set_path(if base_path.is_empty() {
            "/"
        } else {
            &base_path
        });
        Ok(Self(url))
    }

    fn request_url(&self, request_path: &str) -> reqwest::Url {
        let (path, query) = request_path.split_once('?').unwrap_or((request_path, ""));
        let path = path.trim_start_matches('/');
        let base_path = self.0.path().trim_end_matches('/');
        let joined_path = if path.is_empty() {
            if base_path.is_empty() {
                "/".to_owned()
            } else {
                base_path.to_owned()
            }
        } else if base_path.is_empty() {
            format!("/{path}")
        } else {
            format!("{base_path}/{path}")
        };

        let mut url = self.0.clone();
        url.set_path(&joined_path);
        url.set_query((!query.is_empty()).then_some(query));
        url
    }
}

fn is_loopback_host(url: &reqwest::Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    host.strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host)
        .parse::<std::net::IpAddr>()
        .map_or_else(
            |_| host.eq_ignore_ascii_case("localhost"),
            |address| address.is_loopback(),
        )
}

#[derive(Debug, Clone)]
pub(crate) struct ApiClient {
    client: reqwest::Client,
    base_url: ApiBaseUrl,
    bearer_token: Option<String>,
}

impl ApiClient {
    pub(crate) fn new(base_url: ApiBaseUrl, bearer_token: Option<&str>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            bearer_token: bearer_token.map(str::to_owned),
        }
    }

    pub(crate) fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let request = self.client.request(method, self.base_url.request_url(path));
        match self.bearer_token.as_deref() {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    pub(crate) fn multipart(
        &self,
        path: &str,
        field: &str,
        filename: &str,
        bytes: Vec<u8>,
    ) -> RequestBuilder {
        let form = Form::new().part(
            field.to_owned(),
            Part::bytes(bytes).file_name(filename.to_owned()),
        );
        self.request(Method::POST, path).multipart(form)
    }

    pub(crate) fn download_request(&self, path: &str) -> RequestBuilder {
        self.request(Method::GET, path)
    }

    pub(crate) async fn send_json<T: DeserializeOwned>(
        &self,
        request: RequestBuilder,
    ) -> Result<T, ApiError> {
        self.send_json_with_headers(request)
            .await
            .map(|response| response.data)
    }

    pub(crate) async fn send_json_with_headers<T: DeserializeOwned>(
        &self,
        request: RequestBuilder,
    ) -> Result<ApiResponse<T>, ApiError> {
        let response = request.send().await.map_err(ApiError::transport)?;
        decode_json(response).await
    }

    pub(crate) async fn download(&self, path: &str) -> Result<Download, ApiError> {
        let response = self
            .download_request(path)
            .send()
            .await
            .map_err(ApiError::transport)?;
        if !response.status().is_success() {
            return Err(error_response(response).await);
        }
        let status = response.status();
        let content_type = header_string(response.headers(), CONTENT_TYPE);
        let filename = header_string(response.headers(), CONTENT_DISPOSITION);
        let body = response
            .bytes()
            .await
            .map_err(|error| ApiError::response_body(status, error))?
            .to_vec();
        Ok(Download {
            status,
            content_type,
            content_disposition: filename,
            body,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ApiResponse<T> {
    pub(crate) status: StatusCode,
    pub(crate) headers: HeaderMap,
    pub(crate) data: T,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Download {
    pub(crate) status: StatusCode,
    pub(crate) content_type: Option<String>,
    pub(crate) content_disposition: Option<String>,
    pub(crate) body: Vec<u8>,
}

async fn decode_json<T: DeserializeOwned>(response: Response) -> Result<ApiResponse<T>, ApiError> {
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .bytes()
        .await
        .map_err(|error| ApiError::response_body(status, error))?;
    if !status.is_success() {
        return Err(ApiError::from_response(status, &bytes));
    }
    let data = serde_json::from_slice(&bytes).map_err(|error| ApiError::decode(status, error))?;
    Ok(ApiResponse {
        status,
        headers,
        data,
    })
}

async fn error_response(response: Response) -> ApiError {
    let status = response.status();
    match response.bytes().await {
        Ok(bytes) => ApiError::from_response(status, &bytes),
        Err(error) => ApiError::response_body(status, error),
    }
}

fn header_string(
    headers: &reqwest::header::HeaderMap,
    name: reqwest::header::HeaderName,
) -> Option<String> {
    headers.get(name)?.to_str().ok().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::{
        ApiBaseUrl, ApiBaseUrlError, ApiClient, ApiError,
        dto::{issue::Issue, project::Project},
    };
    use reqwest::{Method, StatusCode};
    use serde_json::{Value, json};

    fn client(base_url: impl AsRef<str>, bearer_token: Option<&str>) -> ApiClient {
        ApiClient::new(ApiBaseUrl::parse(base_url.as_ref()).unwrap(), bearer_token)
    }

    #[test]
    fn api_base_url_accepts_https_and_loopback_http() {
        assert!(ApiBaseUrl::parse("https://lific.example/proxy").is_ok());
        assert!(ApiBaseUrl::parse("http://127.0.0.1:8080").is_ok());
        assert!(ApiBaseUrl::parse("http://[::1]:8080").is_ok());
        assert!(ApiBaseUrl::parse("http://localhost:8080").is_ok());
    }

    #[test]
    fn api_base_url_rejects_remote_cleartext_http() {
        assert_eq!(
            ApiBaseUrl::parse("http://lific.example"),
            Err(ApiBaseUrlError::InsecureRemoteHttp)
        );
        assert_eq!(
            ApiBaseUrl::parse("http://192.0.2.10:8080"),
            Err(ApiBaseUrlError::InsecureRemoteHttp)
        );
    }

    #[test]
    fn request_path_cannot_change_authority_or_drop_base_path() {
        let client = client("http://127.0.0.1:8123/proxy/api-root", Some("token"));
        let request = client
            .request(
                Method::GET,
                "//attacker.example/steal?search=one%20two&sort=recent",
            )
            .build()
            .unwrap();

        assert_eq!(request.url().scheme(), "http");
        assert_eq!(request.url().host_str(), Some("127.0.0.1"));
        assert_eq!(request.url().port(), Some(8123));
        assert_eq!(
            request.url().path(),
            "/proxy/api-root/attacker.example/steal"
        );
        let query: std::collections::HashMap<_, _> = request
            .url()
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        assert_eq!(query["search"], "one two");
        assert_eq!(query["sort"], "recent");
        assert_eq!(
            request.headers()[reqwest::header::AUTHORIZATION],
            "Bearer token"
        );

        let absolute = client
            .request(Method::GET, "https://attacker.example/steal")
            .build()
            .unwrap();
        assert_eq!(absolute.url().host_str(), Some("127.0.0.1"));
        assert_eq!(absolute.url().port(), Some(8123));
        assert!(
            absolute
                .url()
                .path()
                .contains("https://attacker.example/steal")
        );
    }

    #[test]
    fn project_and_issue_json_decode_without_changing_the_wire_fields() {
        let project: Project = serde_json::from_value(json!({
            "id": 7,
            "name": "Engine",
            "identifier": "ENG",
            "description": "",
            "emoji": null,
            "lead_user_id": 12,
            "sort_order": 0,
            "created_at": "2026-10-02T12:00:00Z",
            "updated_at": "2026-10-02T12:00:00Z",
            "is_public": false
        }))
        .unwrap();
        assert_eq!(project.identifier, "ENG");
        assert_eq!(serde_json::to_value(project).unwrap()["is_public"], false);

        let issue: Issue = serde_json::from_value(json!({
            "id": 31,
            "project_id": 7,
            "sequence": 4,
            "identifier": "ENG-4",
            "title": "Ship the adapter",
            "description": "",
            "status": "started",
            "priority": "high",
            "module_id": null,
            "sort_order": 0,
            "start_date": null,
            "target_date": null,
            "created_at": "2026-10-02T12:00:00Z",
            "updated_at": "2026-10-02T12:00:00Z",
            "seq": 9,
            "source": null,
            "labels": [],
            "blocks": [],
            "blocked_by": [],
            "relates_to": [],
            "duplicates": [],
            "duplicated_by": [],
            "waits": []
        }))
        .unwrap();
        assert_eq!(issue.expected_seq(), 9);
        assert_eq!(serde_json::to_value(issue).unwrap()["identifier"], "ENG-4");
    }

    #[test]
    fn incompatible_missing_dto_fields_fail_deserialization() {
        let error = serde_json::from_value::<Project>(json!({ "id": 7 })).unwrap_err();
        assert!(error.to_string().contains("name"));

        let error = serde_json::from_value::<Project>(json!({
            "id": 7,
            "name": "Engine",
            "identifier": "ENG",
            "description": "",
            "lead_user_id": null,
            "sort_order": 0,
            "created_at": "2026-10-02T12:00:00Z",
            "updated_at": "2026-10-02T12:00:00Z",
            "is_public": false
        }))
        .unwrap_err();
        assert!(error.to_string().contains("emoji"));
    }

    #[test]
    fn error_response_keeps_status_server_message_and_conflict_entity() {
        let current = json!({ "id": 31, "sequence": 9, "title": "New title" });
        let error = ApiError::from_response(
            StatusCode::CONFLICT,
            br#"{"error":"expected_seq 4 is stale","code":"update_conflict","current":{"id":31,"sequence":9,"title":"New title"}}"#,
        );

        assert_eq!(error.status(), Some(StatusCode::CONFLICT));
        assert_eq!(error.message(), "expected_seq 4 is stale");
        assert!(error.is_update_conflict());
        assert_eq!(error.current(), Some(&current));
    }

    #[test]
    fn response_body_errors_retain_the_received_status() {
        let error = ApiError::response_body(StatusCode::SERVICE_UNAVAILABLE, "stream reset");

        assert_eq!(error.status(), Some(StatusCode::SERVICE_UNAVAILABLE));
        assert_eq!(error.code.as_deref(), Some("response_body"));
        assert!(error.message().contains("stream reset"));
    }

    #[tokio::test]
    async fn json_response_retains_pagination_headers() {
        use axum::{Json, Router, http::header::HeaderName, routing::get};

        let app = Router::new().route(
            "/api/comments",
            get(|| async {
                (
                    [(HeaderName::from_static("x-comment-has-more"), "true")],
                    Json(json!([])),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app.into_make_service())
                .await
                .unwrap();
        });

        let client = client(format!("http://{address}"), None);
        let response = client
            .send_json_with_headers::<Vec<Value>>(client.request(Method::GET, "/api/comments"))
            .await
            .unwrap();
        server.abort();

        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.data, Vec::<Value>::new());
        assert_eq!(response.headers["x-comment-has-more"], "true");
    }

    #[tokio::test]
    async fn body_read_failures_keep_the_http_status_for_json_and_downloads() {
        use axum::{
            Router,
            body::{Body, Bytes},
            http::Response,
            routing::get,
        };

        fn broken_response(status: StatusCode) -> Response<Body> {
            let body = Body::from_stream(futures_util::stream::once(async {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                Err::<Bytes, _>(std::io::Error::other("stream reset"))
            }));
            Response::builder().status(status).body(body).unwrap()
        }

        let app = Router::new()
            .route(
                "/api/json-error",
                get(|| async { broken_response(StatusCode::SERVICE_UNAVAILABLE) }),
            )
            .route(
                "/api/download",
                get(|| async { broken_response(StatusCode::OK) }),
            )
            .route(
                "/api/error-download",
                get(|| async { broken_response(StatusCode::SERVICE_UNAVAILABLE) }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app.into_make_service())
                .await
                .unwrap();
        });
        let client = client(format!("http://{address}"), None);

        let json_error = client
            .send_json::<Value>(client.request(Method::GET, "/api/json-error"))
            .await
            .unwrap_err();
        assert_eq!(json_error.status(), Some(StatusCode::SERVICE_UNAVAILABLE));
        assert_eq!(json_error.code.as_deref(), Some("response_body"));

        let download_error = client.download("/api/download").await.unwrap_err();
        assert_eq!(download_error.status(), Some(StatusCode::OK));
        assert_eq!(download_error.code.as_deref(), Some("response_body"));

        let error_body = client.download("/api/error-download").await.unwrap_err();
        assert_eq!(error_body.status(), Some(StatusCode::SERVICE_UNAVAILABLE));
        assert_eq!(error_body.code.as_deref(), Some("response_body"));

        server.abort();
    }

    #[test]
    fn request_builder_encodes_query_and_bearer_auth() {
        let client = client("https://lific.example", Some("token value"));
        let request = client
            .request(Method::GET, "/api/issues")
            .query(&[("project_id", "7"), ("search", "A & B")])
            .build()
            .unwrap();

        assert_eq!(
            request.headers()[reqwest::header::AUTHORIZATION],
            "Bearer token value"
        );
        let query: std::collections::HashMap<_, _> = request
            .url()
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        assert_eq!(query["project_id"], "7");
        assert_eq!(query["search"], "A & B");
    }

    #[test]
    fn json_body_preserves_expected_seq_and_json_content_type() {
        let client = client("https://lific.example", None);
        let request = client
            .request(Method::PUT, "/api/issues/31")
            .json(&json!({ "title": "Updated", "expected_seq": 4 }))
            .build()
            .unwrap();

        assert_eq!(
            request.headers()[reqwest::header::CONTENT_TYPE],
            "application/json"
        );
        let body = request.body().unwrap().as_bytes().unwrap();
        let body: Value = serde_json::from_slice(body).unwrap();
        assert_eq!(body["expected_seq"], 4);
        assert_eq!(body["title"], "Updated");
    }

    #[tokio::test]
    async fn multipart_boundary_header_matches_the_reqwest_encoded_body() {
        use axum::{Json, Router, body::Bytes, http::HeaderMap, routing::post};

        let (received_tx, mut received_rx) = tokio::sync::mpsc::unbounded_channel();
        let handler = move |headers: HeaderMap, body: Bytes| {
            let received_tx = received_tx.clone();
            async move {
                received_tx
                    .send((
                        headers[reqwest::header::CONTENT_TYPE]
                            .to_str()
                            .unwrap()
                            .to_owned(),
                        headers[reqwest::header::AUTHORIZATION]
                            .to_str()
                            .unwrap()
                            .to_owned(),
                        body.to_vec(),
                    ))
                    .unwrap();
                Json(json!({ "received": true }))
            }
        };
        let app = Router::new().route("/api/project-archives", post(handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app.into_make_service())
                .await
                .unwrap();
        });

        let client = client(format!("http://{address}"), Some("token"));
        let response: Value = client
            .send_json(client.multipart(
                "/api/project-archives",
                "archive",
                "project.lific.tar.gz",
                b"archive bytes".to_vec(),
            ))
            .await
            .unwrap();
        server.abort();

        assert_eq!(response["received"], true);
        let (content_type, authorization, body) = received_rx.recv().await.unwrap();
        assert_eq!(authorization, "Bearer token");
        assert!(content_type.starts_with("multipart/form-data; boundary="));
        let boundary = content_type.split("boundary=").nth(1).unwrap();
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains(&format!("--{boundary}")));
        assert!(body.contains("filename=\"project.lific.tar.gz\""));
        assert!(body.contains("archive bytes"));
    }

    #[test]
    fn download_request_uses_authenticated_get_and_keeps_response_headers() {
        let client = client("https://lific.example", Some("token"));
        let request = client
            .download_request("/api/export/projects/ENG")
            .build()
            .unwrap();

        assert_eq!(request.method(), Method::GET);
        assert_eq!(
            request.headers()[reqwest::header::AUTHORIZATION],
            "Bearer token"
        );
        assert!(request.url().path().ends_with("/api/export/projects/ENG"));
    }
}
