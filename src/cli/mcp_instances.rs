//! `lific mcp --instances <FILE>`: one stdio MCP connection, several
//! separately authenticated Lific servers (LIF-466, LIF-DOC-30).
//!
//! Invariants:
//! - A call selects a configured alias, never a URL or credential, so it can
//!   only reach servers the operator configured and no alias's token can be
//!   sent to another's server.
//! - Backends must advertise matching normalized contracts. Never unioned,
//!   never version-guessed: a mismatch fails the launch.
//! - Routing errors are raised before anything is forwarded, and nothing ever
//!   falls back to another instance. There is no mutable active instance.
//! - Provenance is stamped by this process, so a backend cannot claim to be a
//!   different one.
//!
//! `initialize`, `tools/list`, `ping` and `list_instances` are answered
//! locally. Resources, prompts, completions and logging are refused with
//! -32601: their URIs and cursors are per-server and mean nothing once several
//! servers are behind one connection.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::path::Path;

use reqwest::StatusCode;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde::Deserialize;
use serde_json::{Map, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, BufReader};

use super::mcp_proxy::{
    ForwardError, encode, inject_bound_project, internal_error_response, parse_error_response,
    tidy, write_line,
};

// Limits. Each bounds something attacker- or accident-controlled.

/// Largest response body accepted from a backend, per request.
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
/// Largest single JSON-RPC line accepted from the client on stdin.
const MAX_REQUEST_LINE_BYTES: usize = 1024 * 1024;
/// Largest instances config file.
const MAX_CONFIG_BYTES: u64 = 256 * 1024;
/// How many `tools/list` pages a backend may serve before discovery gives up.
const MAX_DISCOVERY_PAGES: usize = 50;
/// How many tools one backend may advertise in total.
const MAX_TOOLS: usize = 512;
/// How many aliases one config file may declare.
const MAX_INSTANCES: usize = 16;
/// Longest alias name.
const MAX_ALIAS_LEN: usize = 32;
/// Floor for redaction, so a short value cannot redact every message.
const MIN_REDACTABLE_SECRET_LEN: usize = 8;

/// Per-request wall clock for a routed call.
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
/// Shorter than a routed call: a slow backend fails the launch, not the
/// client waiting on `initialize`.
const DISCOVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const STARTUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Fallback when the client asks for a version we do not recognize.
const PROTOCOL_VERSION: &str = "2025-06-18";
/// Versions this proxy will echo back to a client that requested one.
const KNOWN_PROTOCOL_VERSIONS: [&str; 3] = ["2024-11-05", "2025-03-26", "2025-06-18"];

/// Authoritative source alias, written last by this process.
const PROVENANCE_META_KEY: &str = "dev.lific/instance";
/// Human-visible provenance marker. Backend-supplied copies are stripped.
const PROVENANCE_PREFIX: &str = "lific:instance=";

/// The argument this proxy injects, consumes, and strips.
const SELECTOR: &str = "instance";
/// The synthetic discovery tool.
const LIST_INSTANCES: &str = "list_instances";

/// Tools that may use the configured default when `instance` is omitted.
///
/// Explicit, and deliberately not derived from `readOnlyHint`: that annotation
/// is supplied by the server being routed to, so trusting it would let a
/// backend widen what routes without an explicit alias. Unknown tools are
/// outside the list.
const READONLY_TOOLS: [&str; 12] = [
    "export",
    "get_activity",
    "get_attachment",
    "get_board",
    "get_issue",
    "get_page",
    "get_plan",
    "list_attachments",
    "list_comments",
    "list_issues",
    "list_resources",
    "search",
];

/// Whether an omitted `instance` may fall back to the configured default.
#[must_use]
pub(crate) fn is_readonly_tool(name: &str) -> bool {
    READONLY_TOOLS.contains(&name)
}

// configuration

/// Where one alias's bearer token comes from.
///
/// No inline-token variant (a secret in the config file is a secret in
/// dotfiles and backups) and no implicit default, so two aliases cannot share
/// a credential just for resolving to the same origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CredentialSource {
    /// Read the token from this environment variable.
    Env(String),
    /// Use the credential `lific login` stored for this alias's base URL.
    Login,
    /// Send no `Authorization` header at all (an auth-optional instance).
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstanceSpec {
    pub(crate) alias: String,
    /// Base URL, normalized: scheme + host + port + path, no trailing slash,
    /// no userinfo, no query, no fragment.
    pub(crate) url: String,
    pub(crate) credential: CredentialSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstancesConfig {
    pub(crate) instances: Vec<InstanceSpec>,
    pub(crate) default: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    default: Option<String>,
    #[serde(default)]
    instances: BTreeMap<String, RawInstance>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInstance {
    url: String,
    token_env: Option<String>,
    credential: Option<String>,
}

/// The keys a config file may carry, quoted back at the operator on a parse
/// error. A misspelled key is by far the likeliest way to write a file that
/// looks right and routes wrong.
const CONFIG_SHAPE: &str = "expected:\n\
    \x20 default = \"private\"            # optional; allowlisted reads only\n\
    \x20 [instances.private]\n\
    \x20 url = \"https://lific.example\"\n\
    \x20 token_env = \"LIFIC_PRIVATE_TOKEN\"   # or credential = \"login\" | \"none\"";

/// Parse and validate an instances file. Every rejection names the alias.
pub(crate) fn parse_instances(text: &str) -> Result<InstancesConfig, String> {
    // toml itself rejects duplicate table keys, so two `[instances.private]`
    // blocks fail here rather than silently collapsing to one.
    let raw: RawFile = toml::from_str(text).map_err(|error| {
        format!(
            "invalid instances config: {}\n{CONFIG_SHAPE}",
            tidy(&error.to_string())
        )
    })?;

    if raw.instances.is_empty() {
        return Err(format!(
            "the instances config declares no instances.\n{CONFIG_SHAPE}"
        ));
    }
    if raw.instances.len() > MAX_INSTANCES {
        return Err(format!(
            "the instances config declares {} instances; the limit is {MAX_INSTANCES}",
            raw.instances.len()
        ));
    }

    let mut instances = Vec::with_capacity(raw.instances.len());
    for (alias, entry) in &raw.instances {
        validate_alias(alias)?;
        let url = validate_instance_url(alias, &entry.url)?;
        let credential = match (entry.token_env.as_deref(), entry.credential.as_deref()) {
            (Some(_), Some(_)) => {
                return Err(format!(
                    "instance '{alias}' sets both token_env and credential; pick one"
                ));
            }
            (Some(var), None) => {
                let var = var.trim();
                if var.is_empty() {
                    return Err(format!("instance '{alias}' has an empty token_env"));
                }
                CredentialSource::Env(var.to_owned())
            }
            (None, Some("login")) => CredentialSource::Login,
            (None, Some("none")) => CredentialSource::None,
            (None, Some(other)) => {
                return Err(format!(
                    "instance '{alias}' has credential = \"{}\"; expected \"login\" or \"none\"",
                    tidy(other)
                ));
            }
            (None, None) => {
                return Err(format!(
                    "instance '{alias}' names no credential. Set token_env = \"SOME_VAR\", or \
                     credential = \"login\" to use the token `lific login` stored for {url}, or \
                     credential = \"none\" for an instance that takes no authentication."
                ));
            }
        };
        instances.push(InstanceSpec {
            alias: alias.clone(),
            url,
            credential,
        });
    }

    let default = match raw.default {
        Some(name) => {
            let name = name.trim().to_owned();
            if !instances.iter().any(|spec| spec.alias == name) {
                return Err(format!(
                    "default = \"{}\" names no configured instance; configured: {}",
                    tidy(&name),
                    alias_list(&instances)
                ));
            }
            Some(name)
        }
        None => None,
    };

    Ok(InstancesConfig { instances, default })
}

fn alias_list(instances: &[InstanceSpec]) -> String {
    instances
        .iter()
        .map(|spec| spec.alias.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Alias syntax: short, lowercase, ASCII. Narrow because the alias reaches
/// error strings, log lines, the injected schema's `enum` and the provenance
/// marker.
fn validate_alias(alias: &str) -> Result<(), String> {
    if alias.is_empty() {
        return Err("an instance alias is empty".to_owned());
    }
    if alias.len() > MAX_ALIAS_LEN {
        return Err(format!(
            "instance alias '{}' is longer than {MAX_ALIAS_LEN} characters",
            tidy(alias)
        ));
    }
    let mut chars = alias.chars();
    let first = chars.next().unwrap_or('-');
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(format!(
            "instance alias '{}' must start with a lowercase letter or digit",
            tidy(alias)
        ));
    }
    if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') {
        return Err(format!(
            "instance alias '{}' may only contain lowercase letters, digits, '-' and '_'",
            tidy(alias)
        ));
    }
    Ok(())
}

/// Normalize and vet one base URL. Userinfo and query strings are refused
/// rather than stripped: both carry secrets, and this URL gets logged, printed
/// by `list_instances` and quoted in errors.
fn validate_instance_url(alias: &str, url: &str) -> Result<String, String> {
    let url = url.trim();
    let parsed = reqwest::Url::parse(url)
        .map_err(|error| format!("instance '{alias}' has an invalid url: {error}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(format!(
                "instance '{alias}' has url scheme {other}://; expected http:// or https://"
            ));
        }
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err(format!("instance '{alias}' has a url with no host"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(format!(
            "instance '{alias}' has credentials embedded in its url. Remove the user:password@ \
             part and set token_env or credential instead."
        ));
    }
    if parsed.query().is_some() {
        return Err(format!(
            "instance '{alias}' has a query string in its url; the base url must be the server \
             root, with no ?parameters"
        ));
    }
    if parsed.fragment().is_some() {
        return Err(format!(
            "instance '{alias}' has a fragment in its url; the base url must be the server root"
        ));
    }
    Ok(url.trim_end_matches('/').to_owned())
}

/// The CLI-wide rule (`http.rs`, `mcp_proxy::run`): a bearer credential never
/// crosses plaintext http to a non-loopback host. Applied per alias.
fn check_plaintext_policy(spec: &InstanceSpec, has_credential: bool) -> Result<(), String> {
    let Ok(parsed) = reqwest::Url::parse(&spec.url) else {
        return Ok(());
    };
    let host = parsed.host_str().unwrap_or_default().to_owned();
    let non_loopback = !super::http::is_loopback_host(&host);
    if parsed.scheme() == "http" && non_loopback {
        if has_credential {
            return Err(format!(
                "instance '{}': refusing to send bearer credentials over plaintext http to {host}",
                spec.alias
            ));
        }
        eprintln!(
            "warning: instance '{}' connects over unencrypted http to {host}",
            spec.alias
        );
    }
    Ok(())
}

/// Resolve one alias's credential. Failure is fatal: a proxy that starts with
/// a missing token would answer every call to that alias with an auth error,
/// which reads to an agent as "the tracker is broken".
fn load_credential(spec: &InstanceSpec) -> Result<Option<String>, String> {
    match &spec.credential {
        CredentialSource::Env(var) => {
            let value = std::env::var(var)
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    format!(
                        "instance '{}' expects its token in ${var}, which is unset or empty",
                        spec.alias
                    )
                })?;
            Ok(Some(value))
        }
        CredentialSource::Login => {
            // `load_stored`, not `load`: the latter accepts an ambient
            // LIFIC_TOKEN whose LIFIC_URL matches, which would hand one
            // variable to every alias on that origin.
            let stored = super::credentials::load_stored(&spec.url).map_err(|error| {
                format!(
                    "instance '{}': could not read the stored credential for {}: {error}",
                    spec.alias, spec.url
                )
            })?;
            let stored = stored
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty());
            if stored.is_none() {
                return Err(format!(
                    "instance '{}' uses credential = \"login\" but no token is stored for {}. \
                     Run `lific login --url {}`, or point token_env at an API key from the web \
                     UI's Connected Tools page.",
                    spec.alias, spec.url, spec.url
                ));
            }
            Ok(stored)
        }
        CredentialSource::None => Ok(None),
    }
}

// redaction

/// Removes configured secrets from anything bound for stdout or stderr, so a
/// backend echoing its own token cannot put a live credential into a log file
/// or an agent transcript.
#[derive(Debug, Clone, Default)]
pub(crate) struct Redactor {
    secrets: Vec<String>,
}

impl Redactor {
    pub(crate) fn new(secrets: impl IntoIterator<Item = String>) -> Self {
        let mut secrets: Vec<String> = secrets
            .into_iter()
            .filter(|secret| secret.len() >= MIN_REDACTABLE_SECRET_LEN)
            .collect();
        // Longest first, so a token that contains another token's prefix is
        // replaced whole rather than leaving a tail behind.
        secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
        secrets.dedup();
        Self { secrets }
    }

    #[must_use]
    pub(crate) fn scrub(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for secret in &self.secrets {
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), "[redacted]");
            }
        }
        out
    }

    fn is_noop(&self) -> bool {
        self.secrets.is_empty()
    }

    /// Scrub a parsed document in place: every string and every object key, at
    /// every depth.
    ///
    /// Scrubbing raw bytes alone is defeated by `"\u0073\u0065..."`, which
    /// matches no substring on the wire but decodes to the secret. Keys count
    /// too: `{"<token>": 1}` leaks just as well as a value.
    pub(crate) fn scrub_value(&self, value: &mut Value) {
        if self.is_noop() {
            return;
        }
        match value {
            Value::String(text) => {
                if self
                    .secrets
                    .iter()
                    .any(|secret| text.contains(secret.as_str()))
                {
                    *text = self.scrub(text);
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.scrub_value(item);
                }
            }
            Value::Object(map) => {
                let needs_rekey = map.keys().any(|key| {
                    self.secrets
                        .iter()
                        .any(|secret| key.contains(secret.as_str()))
                });
                if needs_rekey {
                    let mut rebuilt = Map::with_capacity(map.len());
                    for (key, mut item) in std::mem::replace(map, Map::new()) {
                        self.scrub_value(&mut item);
                        rebuilt.insert(self.scrub(&key), item);
                    }
                    *map = rebuilt;
                } else {
                    for (_, item) in map.iter_mut() {
                        self.scrub_value(item);
                    }
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }

    /// Serialize for stdout, scrubbing the decoded document and then the bytes
    /// it produces.
    #[must_use]
    pub(crate) fn encode_scrubbed(&self, value: &Value) -> String {
        if self.is_noop() {
            return encode(value);
        }
        let mut value = value.clone();
        self.scrub_value(&mut value);
        self.scrub(&encode(&value))
    }
}

// transport

/// One POST to one named backend. A trait so the pump can be exercised without
/// a socket, and so the alias is the only addressing the pump ever performs:
/// there is no method here that takes a URL.
pub(crate) trait InstanceTransport: Sync {
    fn call(
        &self,
        alias: &str,
        body: String,
    ) -> impl std::future::Future<Output = Result<String, ForwardError>> + Send;
}

struct HttpBackend {
    client: reqwest::Client,
    endpoint: String,
    credential: Option<String>,
}

/// The real transport: one `reqwest::Client` per alias, each built with its
/// own redirect policy and holding only its own credential.
pub(crate) struct HttpBackends {
    backends: BTreeMap<String, HttpBackend>,
}

impl HttpBackends {
    async fn post(backend: &HttpBackend, body: String) -> Result<String, ForwardError> {
        let mut request = backend
            .client
            .post(&backend.endpoint)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .body(body);
        if let Some(credential) = &backend.credential {
            request = request.bearer_auth(credential);
        }

        let response = request.send().await.map_err(ForwardError::unreachable)?;
        let status = response.status();

        // The client uses `redirect::Policy::none()`, so a 3xx arrives here
        // instead of being followed. Following it would re-send the bearer
        // token to a Location the backend chose.
        if status.is_redirection() {
            return Err(ForwardError::unreachable(format!(
                "refused to follow a redirect from {} (HTTP {status}); credentials are never \
                 re-sent to a redirect target",
                backend.endpoint
            )));
        }
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(ForwardError::rejected(status));
        }
        if !status.is_success() {
            let detail = read_capped(response).await.unwrap_or_default();
            return Err(ForwardError::unreachable(format!(
                "HTTP {status} from {}: {}",
                backend.endpoint,
                tidy(&detail)
            )));
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        if !content_type.contains("application/json") {
            return Err(ForwardError::unreachable(format!(
                "expected a JSON response from {}, got content-type {}",
                backend.endpoint,
                if content_type.is_empty() {
                    "(none)"
                } else {
                    &content_type
                }
            )));
        }

        read_capped(response).await
    }
}

/// Read a body a chunk at a time, refusing to buffer more than
/// [`MAX_RESPONSE_BYTES`]. `response.text()` would happily allocate whatever a
/// hostile or broken backend sent, in a process the agent cannot restart.
async fn read_capped(response: reqwest::Response) -> Result<String, ForwardError> {
    let mut response = response;
    let mut buffer: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(ForwardError::unreachable)? {
        if buffer.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(ForwardError::unreachable(format!(
                "response exceeded the {MAX_RESPONSE_BYTES} byte limit"
            )));
        }
        buffer.extend_from_slice(&chunk);
    }
    String::from_utf8(buffer).map_err(|_| ForwardError::unreachable("response was not UTF-8"))
}

impl InstanceTransport for HttpBackends {
    async fn call(&self, alias: &str, body: String) -> Result<String, ForwardError> {
        // Unreachable through the pump, which validates the alias first. Kept
        // as an error rather than a panic because "no backend" must never
        // become "some other backend".
        let Some(backend) = self.backends.get(alias) else {
            return Err(ForwardError::unreachable(format!(
                "no backend is configured for instance '{}'",
                tidy(alias)
            )));
        };
        Self::post(backend, body).await
    }
}

// backend envelope validation

/// Check that a backend's answer is the response to the request we sent.
///
/// Without this a backend can answer one call with another call's body and the
/// client attributes it to the wrong call. Checked before anything is relayed,
/// and during startup discovery too.
pub(crate) fn validate_envelope(response: &Value, expected_id: &Value) -> Result<(), String> {
    let Some(envelope) = response.as_object() else {
        return Err("response was not a JSON-RPC object".to_owned());
    };
    match envelope.get("jsonrpc") {
        Some(Value::String(version)) if version == "2.0" => {}
        Some(other) => {
            return Err(format!(
                "response declared jsonrpc {}, expected \"2.0\"",
                tidy(&other.to_string())
            ));
        }
        None => return Err("response is missing the jsonrpc member".to_owned()),
    }

    let Some(id) = envelope.get("id") else {
        return Err("response is missing the id member".to_owned());
    };
    if id != expected_id {
        return Err(format!(
            "response id {} does not match the request id {}; refusing to attribute one call's \
             answer to another",
            tidy(&id.to_string()),
            tidy(&expected_id.to_string())
        ));
    }

    match (envelope.get("result"), envelope.get("error")) {
        (Some(_), Some(_)) => Err(
            "response carried both result and error, which JSON-RPC forbids and which makes the \
             outcome of the call ambiguous"
                .to_owned(),
        ),
        (None, None) => Err("response carried neither result nor error".to_owned()),
        (None, Some(error)) => {
            if error.get("code").and_then(Value::as_i64).is_none()
                || error.get("message").and_then(Value::as_str).is_none()
            {
                return Err("response error requires an integer code and string message".to_owned());
            }
            Ok(())
        }
        (Some(result), None) if result.is_object() => Ok(()),
        (Some(_), None) => Err("MCP response result must be an object".to_owned()),
    }
}

// contract normalization

/// Metadata instructs the model too. A backend cannot replace another's tool
/// descriptions or annotations merely by sorting first in the alias list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Contract {
    definition: Value,
}

fn contract_of(tool: &Value) -> Option<(String, Contract)> {
    serde_json::from_value::<rmcp::model::Tool>(tool.clone()).ok()?;
    let schema = tool.get("inputSchema")?.as_object()?;
    if schema.get("type")?.as_str()? != "object"
        || schema
            .get("properties")
            .is_some_and(|properties| !properties.is_object() || properties.get(SELECTOR).is_some())
        || schema.get("required").is_some_and(|required| {
            !required.is_array()
                || required
                    .as_array()
                    .is_some_and(|values| values.iter().any(|v| v == SELECTOR))
        })
    {
        return None;
    }
    let name = tool.get("name")?.as_str()?.to_owned();
    Some((
        name,
        Contract {
            definition: tool.clone(),
        },
    ))
}

/// Reduce every backend's tool list to one advertised surface, or name which
/// alias disagrees about what. No union and no best effort: a mismatch fails
/// the launch rather than working until an agent calls the tool that differs.
pub(crate) fn unify_surfaces(surfaces: &[(String, Vec<Value>)]) -> Result<Vec<Value>, String> {
    let Some((base_alias, base_tools)) = surfaces.first() else {
        return Err("no instances to unify".to_owned());
    };

    let mut base: BTreeMap<String, Contract> = BTreeMap::new();
    let mut ordered: BTreeMap<String, Value> = BTreeMap::new();
    for tool in base_tools {
        let Some((name, contract)) = contract_of(tool) else {
            return Err(format!(
                "instance '{base_alias}' advertised an invalid tool or reserved instance argument"
            ));
        };
        if name == SELECTOR || name == LIST_INSTANCES {
            return Err(format!(
                "instance '{base_alias}' advertises a tool named '{name}', which collides with \
                 the proxy's own surface"
            ));
        }
        if base.insert(name.clone(), contract).is_some() {
            return Err(format!(
                "instance '{base_alias}' advertised the tool '{name}' twice"
            ));
        }
        ordered.insert(name, tool.clone());
    }
    if base.is_empty() {
        return Err(format!("instance '{base_alias}' advertises no tools"));
    }

    for (alias, tools) in surfaces.iter().skip(1) {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for tool in tools {
            let Some((name, contract)) = contract_of(tool) else {
                return Err(format!(
                    "instance '{alias}' advertised an invalid tool or reserved instance argument"
                ));
            };
            let Some(expected) = base.get(&name) else {
                return Err(format!(
                    "tool surfaces differ: instance '{alias}' advertises '{name}', which \
                     instance '{base_alias}' does not. Bring the two servers to the same Lific \
                     version, or run them as separate MCP entries."
                ));
            };
            if &contract != expected {
                return Err(format!(
                    "tool contracts differ for '{name}': instance '{alias}' does not match \
                     instance '{base_alias}'. Schemas are never merged; bring the two servers to \
                     the same Lific version, or run them as separate MCP entries."
                ));
            }
            if !seen.insert(name.clone()) {
                return Err(format!(
                    "instance '{alias}' advertised the tool '{name}' twice"
                ));
            }
        }
        if let Some(missing) = base.keys().find(|name| !seen.contains(*name)) {
            return Err(format!(
                "tool surfaces differ: instance '{base_alias}' advertises '{missing}', which \
                 instance '{alias}' does not. Bring the two servers to the same Lific version, \
                 or run them as separate MCP entries."
            ));
        }
    }

    Ok(ordered.into_values().collect())
}

/// Inject the `instance` selector into every unified tool and append the
/// discovery tool. The advertised schema is the proxy's, not any server's.
pub(crate) fn advertise(
    tools: Vec<Value>,
    aliases: &[String],
    default: Option<&str>,
) -> Vec<Value> {
    let enum_values: Vec<Value> = aliases
        .iter()
        .map(|alias| Value::String(alias.clone()))
        .collect();
    let description = match default {
        Some(default) if aliases.len() > 1 => format!(
            "Which configured Lific instance to run this against. One of: {}. Optional for \
             read-only tools, which fall back to the configured default '{default}'; REQUIRED \
             for every tool that writes.",
            aliases.join(", ")
        ),
        _ if aliases.len() > 1 => format!(
            "Which configured Lific instance to run this against. One of: {}. Required on every \
             tool: no default instance is configured.",
            aliases.join(", ")
        ),
        _ => format!(
            "Which configured Lific instance to run this against. Only '{}' is configured.",
            aliases.join(", ")
        ),
    };
    let selector_schema = serde_json::json!({
        "type": "string",
        "enum": enum_values,
        "description": description,
    });

    let mut advertised: Vec<Value> = tools
        .into_iter()
        .map(|mut tool| {
            let name = tool
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            // Must mirror `Router::select` exactly, or the schema promises
            // what the router refuses. Optional only where omission has a
            // defined answer: one backend, or an allowlisted read with a
            // configured default.
            let required_here =
                aliases.len() > 1 && !(is_readonly_tool(&name) && default.is_some());
            let schema = tool
                .as_object_mut()
                .and_then(|tool| {
                    tool.entry("inputSchema")
                        .or_insert_with(|| serde_json::json!({ "type": "object" }));
                    tool.get_mut("inputSchema")
                })
                .and_then(Value::as_object_mut);
            if let Some(schema) = schema {
                schema.insert("type".to_owned(), Value::String("object".to_owned()));
                let properties = schema
                    .entry("properties")
                    .or_insert_with(|| Value::Object(Map::new()));
                if let Some(properties) = properties.as_object_mut() {
                    properties.insert(SELECTOR.to_owned(), selector_schema.clone());
                }
                // `additionalProperties: false` is preserved: adding
                // `instance` to `properties` already makes it legal, and
                // relaxing it would let every typo'd argument through.
                if required_here {
                    let required = schema
                        .entry("required")
                        .or_insert_with(|| Value::Array(Vec::new()));
                    if !required.is_array() {
                        *required = Value::Array(Vec::new());
                    }
                    if let Some(required) = required.as_array_mut()
                        && !required.iter().any(|value| value == SELECTOR)
                    {
                        required.push(Value::String(SELECTOR.to_owned()));
                    }
                }
            }
            tool
        })
        .collect();

    // Discovery is the proxy's own tool and is the one call that is never
    // routed, so it takes no selector at all and accepts nothing else.
    advertised.push(serde_json::json!({
        "name": LIST_INSTANCES,
        "description": "List the Lific instances this connection can route to: their alias, \
                        host, which one is the default for read-only tools, and the project each \
                        one resolved for this repository. Credentials are never included.",
        "inputSchema": {
            "type": "object",
            "properties": {},
            "additionalProperties": false,
        },
    }));
    advertised
}

// routing

/// One backend, as the pump sees it.
#[derive(Debug, Clone)]
pub(crate) struct RoutedInstance {
    pub(crate) alias: String,
    /// Host and port only; enough to tell two instances apart in
    /// `list_instances` without republishing a full URL.
    pub(crate) host: String,
    /// This backend's own repository binding, resolved against this backend.
    pub(crate) bound_project: Option<String>,
    pub(crate) authenticated: bool,
}

pub(crate) struct Router {
    instances: Vec<RoutedInstance>,
    default: Option<String>,
    tools: Vec<Value>,
    redactor: Redactor,
}

/// What a `tools/call` resolved to, before anything leaves the process.
#[derive(Debug, PartialEq, Eq)]
enum Route {
    /// Answer here; never touch a backend.
    Local,
    /// Forward the (rewritten) message to exactly this alias.
    Remote { alias: String, body: Value },
}

impl Router {
    pub(crate) fn new(
        instances: Vec<RoutedInstance>,
        default: Option<String>,
        tools: Vec<Value>,
        redactor: Redactor,
    ) -> Self {
        Self {
            instances,
            default,
            tools,
            redactor,
        }
    }

    fn aliases(&self) -> Vec<String> {
        self.instances
            .iter()
            .map(|instance| instance.alias.clone())
            .collect()
    }

    fn knows(&self, alias: &str) -> bool {
        self.instances.iter().any(|entry| entry.alias == alias)
    }

    fn binding(&self, alias: &str) -> Option<&str> {
        self.instances
            .iter()
            .find(|entry| entry.alias == alias)
            .and_then(|entry| entry.bound_project.as_deref())
    }

    /// Decide which alias a call belongs to, from the tool name, the caller's
    /// `instance` argument and the configuration. Every failure happens before
    /// anything is forwarded, so a typo never reaches a tracker at all.
    fn select(&self, tool: &str, arguments: Option<&Map<String, Value>>) -> Result<String, String> {
        let selector = arguments.and_then(|arguments| arguments.get(SELECTOR));
        let explicit = match selector {
            None => None,
            // Null is a type error, not an omission: the schema says string,
            // and an explicit null must not silently mean "the default".
            Some(Value::String(value)) => {
                let value = value.trim();
                if value.is_empty() {
                    return Err(format!(
                        "the '{SELECTOR}' argument was empty. Name one of: {}.",
                        self.aliases().join(", ")
                    ));
                }
                Some(value.to_owned())
            }
            Some(other) => {
                return Err(format!(
                    "the '{SELECTOR}' argument must be a string naming a configured instance, \
                     got {}. Name one of: {}.",
                    json_type_name(other),
                    self.aliases().join(", ")
                ));
            }
        };

        if let Some(alias) = explicit {
            if !self.knows(&alias) {
                return Err(format!(
                    "unknown instance '{}'. Configured: {}. Nothing was sent to any instance.",
                    tidy(&alias),
                    self.aliases().join(", ")
                ));
            }
            return Ok(alias);
        }

        // Exactly one backend: the selector is redundant and the whole
        // ambiguity this feature guards against does not exist.
        if self.instances.len() == 1 {
            return Ok(self.instances[0].alias.clone());
        }

        if is_readonly_tool(tool) {
            if let Some(default) = &self.default {
                return Ok(default.clone());
            }
            return Err(format!(
                "'{tool}' needs an explicit '{SELECTOR}': {} instances are configured and none \
                 is marked as the default. Pass {SELECTOR} = one of: {}.",
                self.instances.len(),
                self.aliases().join(", ")
            ));
        }

        Err(format!(
            "'{tool}' requires an explicit '{SELECTOR}' because {} instances are configured. \
             Writes and unrecognized tools never use the default instance. Pass {SELECTOR} = one \
             of: {}.",
            self.instances.len(),
            self.aliases().join(", ")
        ))
    }

    /// Build the outbound message for one `tools/call`, or the error the
    /// client gets instead.
    fn plan(&self, message: &Value) -> Result<Route, String> {
        let params = message.get("params").and_then(Value::as_object);
        let Some(params) = params else {
            return Err("tools/call needs a params object with a tool name".to_owned());
        };
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return Err("tools/call needs a string 'name'".to_owned());
        };
        // A non-object `arguments` must not read as "no arguments": that would
        // silently route a malformed call to the default instance.
        let arguments = match params.get("arguments") {
            None | Some(Value::Null) => None,
            Some(Value::Object(arguments)) => Some(arguments),
            Some(other) => {
                return Err(format!(
                    "tools/call 'arguments' must be an object, got {}",
                    json_type_name(other)
                ));
            }
        };

        if name == LIST_INSTANCES {
            // Advertised with `additionalProperties: false`; enforce it here.
            if arguments.is_some_and(|arguments| !arguments.is_empty()) {
                return Err(format!("{LIST_INSTANCES} takes no arguments"));
            }
            return Ok(Route::Local);
        }

        let alias = self.select(name, arguments)?;

        // A backend must never see the selector: it is not in its schema.
        let mut outbound = message.clone();
        if arguments.is_none() {
            outbound["params"]["arguments"] = serde_json::json!({});
        }
        if let Some(arguments) = outbound
            .get_mut("params")
            .and_then(|params| params.get_mut("arguments"))
            .and_then(Value::as_object_mut)
        {
            arguments.remove(SELECTOR);
        }

        // This backend's own binding, resolved against this backend at
        // startup. `private` and `community` can disagree about what this
        // directory is, and each answer stays with its own server.
        if let Some(rewritten) = inject_bound_project(&outbound, self.binding(&alias))
            && let Ok(rewritten) = serde_json::from_str::<Value>(&rewritten)
        {
            outbound = rewritten;
        }

        Ok(Route::Remote {
            alias,
            body: outbound,
        })
    }

    fn initialize_result(&self, id: &Value, request: &Value) -> Value {
        let requested = request
            .get("params")
            .and_then(|params| params.get("protocolVersion"))
            .and_then(Value::as_str)
            .filter(|version| KNOWN_PROTOCOL_VERSIONS.contains(version))
            .unwrap_or(PROTOCOL_VERSION);
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id.clone(),
            "result": {
                "protocolVersion": requested,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": {
                    "name": "lific-multi-instance-proxy",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "instructions": self.instructions(),
            },
        })
    }

    fn instructions(&self) -> String {
        let mut text = String::from(
            "This connection fans out to several separately authenticated Lific instances. Every \
             tool takes an extra 'instance' argument naming which one to use. Call \
             list_instances to see them.",
        );
        if self.instances.len() > 1 {
            match &self.default {
                Some(default) => text.push_str(&format!(
                    " Read-only tools default to '{default}' when 'instance' is omitted; every \
                     tool that writes requires an explicit 'instance'."
                )),
                None => text.push_str(
                    " No default instance is configured, so every tool requires an explicit \
                     'instance'.",
                ),
            }
        }
        for instance in &self.instances {
            match &instance.bound_project {
                Some(project) => text.push_str(&format!(
                    " On '{}' this repository is bound to project {project}.",
                    instance.alias
                )),
                None => text.push_str(&format!(
                    " On '{}' this repository is unbound; run 'lific bind' against that server \
                     to bind it.",
                    instance.alias
                )),
            }
        }
        text
    }

    fn list_instances_result(&self, id: &Value) -> Value {
        let rows: Vec<Value> = self
            .instances
            .iter()
            .map(|instance| {
                serde_json::json!({
                    "instance": instance.alias,
                    "host": instance.host,
                    "default_for_reads": self.default.as_deref() == Some(instance.alias.as_str()),
                    "authenticated": instance.authenticated,
                    "bound_project": instance.bound_project,
                })
            })
            .collect();
        let mut text = String::new();
        for instance in &self.instances {
            let marker = if self.default.as_deref() == Some(instance.alias.as_str()) {
                " (default for read-only tools)"
            } else {
                ""
            };
            text.push_str(&format!(
                "{}: {}{}, {}, repository binding: {}\n",
                instance.alias,
                instance.host,
                marker,
                if instance.authenticated {
                    "authenticated"
                } else {
                    "no credential"
                },
                instance.bound_project.as_deref().unwrap_or("none"),
            ));
        }
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id.clone(),
            "result": {
                "content": [{ "type": "text", "text": text }],
                "structuredContent": { "instances": rows },
            },
        })
    }
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

// provenance

/// Stamp the source alias onto a relayed response. Applied unconditionally
/// after the backend answered, with forged copies removed first, so a backend
/// cannot claim to be another instance.
pub(crate) fn stamp_provenance(response: &mut Value, alias: &str) {
    if let Some(result) = response.get_mut("result").and_then(Value::as_object_mut) {
        let meta = result
            .entry("_meta")
            .or_insert_with(|| Value::Object(Map::new()));
        if !meta.is_object() {
            *meta = Value::Object(Map::new());
        }
        if let Some(meta) = meta.as_object_mut() {
            meta.insert(
                PROVENANCE_META_KEY.to_owned(),
                Value::String(alias.to_owned()),
            );
        }

        if let Some(content) = result.get_mut("content").and_then(Value::as_array_mut) {
            content.retain(|item| {
                !item
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| text.trim_start().starts_with(PROVENANCE_PREFIX))
            });
            content.push(serde_json::json!({
                "type": "text",
                "text": format!("{PROVENANCE_PREFIX}{alias}"),
            }));
        }
        return;
    }

    if let Some(error) = response.get_mut("error").and_then(Value::as_object_mut) {
        if let Some(Value::String(message)) = error.get_mut("message") {
            *message = format!("[instance {alias}] {message}");
        }
        let data = error
            .entry("data")
            .or_insert_with(|| Value::Object(Map::new()));
        if !data.is_object() {
            *data = Value::Object(Map::new());
        }
        if let Some(data) = data.as_object_mut() {
            data.insert(SELECTOR.to_owned(), Value::String(alias.to_owned()));
        }
    }
}

// the pump

/// Methods refused rather than routed: resource URIs, prompt names and list
/// cursors are per-server, so answering from "some instance" hands the client
/// an identifier it cannot re-resolve.
fn unsupported_method(method: &str) -> Option<&'static str> {
    match method {
        "resources/list"
        | "resources/templates/list"
        | "resources/read"
        | "resources/subscribe"
        | "resources/unsubscribe" => Some(
            "this multi-instance proxy does not serve MCP resources: resource URIs and list \
             cursors are per-server and cannot be shared across instances. Use tools, or add a \
             single-instance `lific mcp --remote` entry for the server you need resources from.",
        ),
        "prompts/list" | "prompts/get" => Some(
            "this multi-instance proxy does not serve MCP prompts: prompt names are per-server \
             and cannot be shared across instances.",
        ),
        "completion/complete" => Some(
            "this multi-instance proxy does not serve completions: they are scoped to a single \
             server's resources and prompts.",
        ),
        "logging/setLevel" => {
            Some("this multi-instance proxy does not forward logging control to backends.")
        }
        _ => None,
    }
}

/// One frame read off stdin.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Frame {
    Line(String),
    /// A line longer than the limit. Carries how many bytes were seen; the
    /// bytes themselves were discarded as they arrived.
    TooLong(usize),
    /// A line that was not UTF-8.
    NotText,
    Eof,
}

/// Read one newline-delimited frame, never buffering more than `limit` bytes.
///
/// `AsyncBufReadExt::lines` grows one `String` until it finds a newline, so a
/// peer sending no `\n` makes the proxy allocate without bound; the limit has
/// to be enforced while reading. On overflow the rest of the line is consumed
/// and dropped so the next frame is still the next real message.
pub(crate) async fn read_frame<R: AsyncBufRead + Unpin + Send>(
    input: &mut R,
    limit: usize,
) -> std::io::Result<Frame> {
    let mut buffer: Vec<u8> = Vec::new();
    let mut discarded: Option<usize> = None;

    loop {
        let available = input.fill_buf().await?;
        if available.is_empty() {
            return Ok(match discarded {
                Some(seen) => Frame::TooLong(seen),
                None if buffer.is_empty() => Frame::Eof,
                None => finish(buffer),
            });
        }

        let newline = available.iter().position(|byte| *byte == b'\n');
        let keep = newline.unwrap_or(available.len());
        let consume = newline.map_or(available.len(), |at| at + 1);

        match &mut discarded {
            Some(seen) => *seen += keep,
            None if buffer.len() + keep > limit => {
                // Release what was already buffered: past the limit the
                // content is not going to be used, only counted.
                discarded = Some(buffer.len() + keep);
                buffer = Vec::new();
            }
            None => buffer.extend_from_slice(&available[..keep]),
        }
        input.consume(consume);

        if newline.is_some() {
            return Ok(match discarded {
                Some(seen) => Frame::TooLong(seen),
                None => finish(buffer),
            });
        }
    }
}

/// Turn a completed frame's bytes into a line, tolerating CRLF.
fn finish(mut buffer: Vec<u8>) -> Frame {
    if buffer.last() == Some(&b'\r') {
        buffer.pop();
    }
    match String::from_utf8(buffer) {
        Ok(line) => Frame::Line(line),
        Err(_) => Frame::NotText,
    }
}

fn error_response(id: &Value, code: i64, message: &str) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id.clone(),
        "error": { "code": code, "message": message },
    })
}

/// Read newline-delimited JSON-RPC from `input`, answer or route each request,
/// and write exactly one line to `output` per request. Returns on EOF.
pub(crate) async fn pump<R, W, T>(
    input: R,
    output: W,
    router: &Router,
    transport: &T,
) -> std::io::Result<()>
where
    R: AsyncBufRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
    T: InstanceTransport,
{
    let mut input = input;
    let mut output = output;

    loop {
        let line = match read_frame(&mut input, MAX_REQUEST_LINE_BYTES).await? {
            Frame::Eof => break,
            Frame::Line(line) => line,
            Frame::TooLong(seen) => {
                write_frame(
                    &mut output,
                    router,
                    &parse_error_response(&format!(
                        "request line of at least {seen} bytes exceeds the \
                         {MAX_REQUEST_LINE_BYTES} byte limit; it was discarded unread"
                    )),
                )
                .await?;
                continue;
            }
            Frame::NotText => {
                write_frame(
                    &mut output,
                    router,
                    &parse_error_response("request line was not valid UTF-8"),
                )
                .await?;
                continue;
            }
        };

        if line.trim().is_empty() {
            continue;
        }

        let message: Value = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(error) => {
                write_frame(
                    &mut output,
                    router,
                    &parse_error_response(&error.to_string()),
                )
                .await?;
                continue;
            }
        };

        // No id means a notification. Nothing here is stateful across
        // backends, so notifications are dropped rather than fanned out (which
        // would multiply one client event into N server events).
        let Some(id) = message.get("id").cloned() else {
            continue;
        };

        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();

        let outcome = match method.as_str() {
            "initialize" => router.initialize_result(&id, &message),
            "ping" => serde_json::json!({
                "jsonrpc": "2.0", "id": id, "result": {},
            }),
            "tools/list" => {
                // The whole surface fits in one page, and it is this
                // process's surface, not any backend's. A cursor could only
                // have come from somewhere else.
                if message
                    .get("params")
                    .and_then(|params| params.get("cursor"))
                    .is_some_and(|cursor| !cursor.is_null())
                {
                    error_response(
                        &id,
                        -32602,
                        "this proxy returns its whole tool surface in one page; no cursor is \
                         valid here",
                    )
                } else {
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "tools": router.tools },
                    })
                }
            }
            "tools/call" => match router.plan(&message) {
                Ok(Route::Local) => router.list_instances_result(&id),
                Ok(Route::Remote { alias, body }) => {
                    forward(&mut output, router, transport, &id, &alias, body).await?;
                    continue;
                }
                // -32602, not a tool error: the call was never made, and the
                // client must be able to tell routing failure from a refusal
                // by the tracker.
                Err(reason) => error_response(&id, -32602, &reason),
            },
            other => {
                let message = unsupported_method(other).map_or_else(
                    || format!("unsupported method '{}'", tidy(other)),
                    str::to_owned,
                );
                error_response(&id, -32601, &message)
            }
        };
        write_frame(&mut output, router, &outcome).await?;
    }

    Ok(())
}

/// The single exit for everything written to stdout. Scrubbing here makes the
/// no-token-on-stdout guarantee a property of one function.
async fn write_frame<W>(output: &mut W, router: &Router, payload: &Value) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin + Send,
{
    write_line(output, &router.redactor.encode_scrubbed(payload)).await
}

/// Forward one planned call and write its stamped answer.
async fn forward<W, T>(
    output: &mut W,
    router: &Router,
    transport: &T,
    id: &Value,
    alias: &str,
    body: Value,
) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin + Send,
    T: InstanceTransport,
{
    let mut outcome = match transport.call(alias, encode(&body)).await {
        Ok(raw) => {
            // Scrub the wire bytes first: this is the only pass that can see a
            // secret sitting in a body that never parses as JSON at all.
            let raw = router.redactor.scrub(&raw);
            match serde_json::from_str::<Value>(&raw) {
                Ok(mut value) => {
                    // Scrub again on the decoded document, which is where an
                    // escaped `\u0073ecret` finally becomes a matchable
                    // substring, and do it before anything reads the response.
                    router.redactor.scrub_value(&mut value);
                    let validation = validate_envelope(&value, id).and_then(|()| {
                        if let Some(result) = value.get("result") {
                            serde_json::from_value::<rmcp::model::CallToolResult>(result.clone())
                                .map(|_| ())
                                .map_err(|_| "invalid tools/call result".to_owned())
                        } else {
                            Ok(())
                        }
                    });
                    match validation {
                        Ok(()) => value,
                        Err(reason) => {
                            internal_error_response(id, &ForwardError::unreachable(reason).message)
                        }
                    }
                }
                Err(error) => internal_error_response(
                    id,
                    &ForwardError::unreachable(format!("response was not JSON ({error})")).message,
                ),
            }
        }
        Err(error) => {
            // A failing backend produces a failing call, never a call
            // somewhere else. There is no retry and no second alias.
            internal_error_response(id, &error.message)
        }
    };
    stamp_provenance(&mut outcome, alias);
    write_frame(output, router, &outcome).await
}

// startup

/// Ask one backend what it can do: `initialize`, then `tools/list` until the
/// cursor runs out. Every failure names the alias, because "the server said
/// no" is useless when there are two servers.
async fn discover<T: InstanceTransport>(
    transport: &T,
    alias: &str,
    redactor: &Redactor,
) -> Result<Vec<Value>, String> {
    /// Send one request and return its validated, scrubbed response. Startup
    /// gets the same envelope discipline as a routed call.
    async fn ask<T: InstanceTransport>(
        transport: &T,
        alias: &str,
        redactor: &Redactor,
        request: &Value,
        what: &str,
    ) -> Result<Value, String> {
        let raw = transport
            .call(alias, encode(request))
            .await
            .map_err(|error| format!("instance '{alias}': {}", redactor.scrub(&error.message)))?;
        let raw = redactor.scrub(&raw);
        let mut parsed: Value = serde_json::from_str(&raw)
            .map_err(|error| format!("instance '{alias}': {what} was not JSON ({error})"))?;
        redactor.scrub_value(&mut parsed);
        let expected = request.get("id").cloned().unwrap_or(Value::Null);
        validate_envelope(&parsed, &expected)
            .map_err(|reason| format!("instance '{alias}': {what} {reason}"))?;
        if let Some(error) = parsed.get("error") {
            return Err(format!(
                "instance '{alias}' refused {what}: {}",
                tidy(&error.to_string())
            ));
        }
        Ok(parsed)
    }

    let init = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "lific-multi-instance-proxy", "version": env!("CARGO_PKG_VERSION") },
        },
    });
    ask(transport, alias, redactor, &init, "initialize").await?;

    let mut tools: Vec<Value> = Vec::new();
    let mut surface_bytes = 0usize;
    let mut cursor: Option<String> = None;
    for page in 0..MAX_DISCOVERY_PAGES {
        let params = match &cursor {
            Some(cursor) => serde_json::json!({ "cursor": cursor }),
            None => serde_json::json!({}),
        };
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2 + page,
            "method": "tools/list",
            "params": params,
        });
        let parsed = ask(transport, alias, redactor, &request, "tools/list").await?;
        let Some(page_tools) = parsed
            .get("result")
            .and_then(|result| result.get("tools"))
            .and_then(Value::as_array)
        else {
            return Err(format!(
                "instance '{alias}': tools/list did not return a tools array"
            ));
        };
        for tool in page_tools {
            surface_bytes = surface_bytes
                .saturating_add(serde_json::to_vec(tool).map_err(|e| e.to_string())?.len());
            if surface_bytes > MAX_RESPONSE_BYTES {
                return Err(format!(
                    "instance '{alias}': combined tool surface exceeds the size limit"
                ));
            }
            tools.push(tool.clone());
        }
        if tools.len() > MAX_TOOLS {
            return Err(format!(
                "instance '{alias}' advertises more than {MAX_TOOLS} tools"
            ));
        }
        let next = parsed
            .get("result")
            .and_then(|result| result.get("nextCursor"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        match next {
            // A backend that keeps handing back the same cursor would
            // otherwise pin this loop until MAX_DISCOVERY_PAGES.
            Some(next) if Some(&next) == cursor.as_ref() => {
                return Err(format!(
                    "instance '{alias}' repeated the same tools/list cursor"
                ));
            }
            Some(next) => cursor = Some(next),
            None => return Ok(tools),
        }
    }
    Err(format!(
        "instance '{alias}' paginated tools/list past {MAX_DISCOVERY_PAGES} pages"
    ))
}

/// Resolve this directory's project binding against one backend.
///
/// Separate from [`super::mcp_proxy::resolve_binding`] because the body must go
/// through [`read_capped`] (the shared version calls `response.json()`, which
/// buffers without bound) and every logged message through the redactor.
/// Failure is soft: an unresolvable binding is an unbound session.
async fn resolve_binding_capped(
    client: &reqwest::Client,
    alias: &str,
    url: &str,
    credential: Option<&str>,
    redactor: &Redactor,
) -> Option<String> {
    let dir = std::env::current_dir().ok()?;
    let aliases = match crate::repo_identity::compute(&dir) {
        Ok(aliases) if aliases.is_empty() => {
            tracing::info!(%alias, "unbound: this directory has no repository identity");
            return None;
        }
        Ok(aliases) => aliases,
        Err(error) => {
            tracing::info!(%alias, error = %redactor.scrub(&error.to_string()), "unbound");
            return None;
        }
    };

    let body = serde_json::json!({
        "aliases": aliases
            .iter()
            .map(|entry| serde_json::json!({
                "kind": super::mcp_proxy::alias_kind(&entry.kind),
                "value": entry.value,
            }))
            .collect::<Vec<_>>(),
    });
    let endpoint = format!("{}/api/repos/resolve", url.trim_end_matches('/'));
    let mut request = client.post(&endpoint).json(&body);
    if let Some(credential) = credential {
        request = request.bearer_auth(credential);
    }

    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(
                %alias,
                error = %redactor.scrub(&tidy(&error.to_string())),
                "unbound: could not ask this instance what the repository is bound to"
            );
            return None;
        }
    };
    if response.status().is_redirection() {
        tracing::warn!(%alias, "unbound: refused to follow a redirect on the repository lookup");
        return None;
    }
    if !response.status().is_success() {
        tracing::warn!(%alias, status = %response.status(), "unbound: the instance refused the repository lookup");
        return None;
    }
    let raw = match read_capped(response).await {
        Ok(raw) => redactor.scrub(&raw),
        Err(error) => {
            tracing::warn!(
                %alias,
                error = %redactor.scrub(&error.message),
                "unbound: the instance's repository lookup could not be read"
            );
            return None;
        }
    };
    let mut resolved: Value = match serde_json::from_str(&raw) {
        Ok(resolved) => resolved,
        Err(error) => {
            tracing::warn!(%alias, error = %tidy(&error.to_string()), "unbound: the repository lookup was not JSON");
            return None;
        }
    };
    redactor.scrub_value(&mut resolved);

    let bound = super::mcp_proxy::binding_from_resolution(&resolved)
        .filter(|identifier| crate::db::queries::validate_identifier(identifier).is_ok());
    match &bound {
        Some(project) => tracing::info!(%alias, %project, "instance bound to project"),
        None => tracing::debug!(%alias, "instance is unbound"),
    }
    bound
}

/// Read the instances file, bounded, and with a readable error for the two
/// mistakes people actually make (wrong path, wrong shape).
fn read_config(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err("instances config must be a regular file".to_owned());
    }
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(format!(
            "{} is {} bytes; the limit is {MAX_CONFIG_BYTES}",
            path.display(),
            metadata.len()
        ));
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|error| error.to_string())?;
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("instances config must be a regular file".to_owned());
    }
    let mut text = String::new();
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    if text.len() as u64 > MAX_CONFIG_BYTES {
        return Err("instances config exceeds the size limit".to_owned());
    }
    Ok(text)
}

/// Run the multi-instance proxy against the instances file at `path`, pumping
/// this process's stdin and stdout.
pub async fn run(path: &Path) -> Result<(), Box<dyn Error>> {
    let config = parse_instances(&read_config(path)?)?;

    // Credentials first, and independently: an alias whose token is missing
    // fails the launch rather than silently answering every call with 401.
    let mut credentials: Vec<Option<String>> = Vec::with_capacity(config.instances.len());
    for spec in &config.instances {
        let credential = load_credential(spec)?;
        check_plaintext_policy(spec, credential.is_some())?;
        credentials.push(credential);
    }
    let redactor = Redactor::new(credentials.iter().flatten().cloned());

    let mut backends: BTreeMap<String, HttpBackend> = BTreeMap::new();
    let mut clients: BTreeMap<String, reqwest::Client> = BTreeMap::new();
    for (spec, credential) in config.instances.iter().zip(&credentials) {
        // One client per alias. Redirects refused, so a bearer token is never
        // replayed to a Location the backend picked; cookies off by default,
        // so no state crosses aliases.
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(CALL_TIMEOUT)
            .build()?;
        let discovery_client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(DISCOVERY_TIMEOUT)
            .build()?;
        clients.insert(spec.alias.clone(), discovery_client);
        backends.insert(
            spec.alias.clone(),
            HttpBackend {
                client,
                endpoint: format!("{}/mcp", spec.url),
                credential: credential.clone(),
            },
        );
    }

    // Discovery runs on its own shorter-timeout transport, then the pump uses
    // the long-timeout one. Same credentials, same endpoints, different clock.
    let discovery = HttpBackends {
        backends: config
            .instances
            .iter()
            .zip(&credentials)
            .map(|(spec, credential)| {
                (
                    spec.alias.clone(),
                    HttpBackend {
                        client: clients.remove(&spec.alias).expect("client per alias"),
                        endpoint: format!("{}/mcp", spec.url),
                        credential: credential.clone(),
                    },
                )
            })
            .collect(),
    };

    let mut surfaces: Vec<(String, Vec<Value>)> = Vec::with_capacity(config.instances.len());
    let mut routed: Vec<RoutedInstance> = Vec::with_capacity(config.instances.len());
    let deadline = tokio::time::Instant::now() + STARTUP_TIMEOUT;
    for (spec, credential) in config.instances.iter().zip(&credentials) {
        let tools = tokio::time::timeout_at(deadline, discover(&discovery, &spec.alias, &redactor))
            .await
            .map_err(|_| format!("instance '{}': startup deadline exceeded", spec.alias))?
            .map_err(|error| redactor.scrub(&error))?;
        surfaces.push((spec.alias.clone(), tools));

        // Each backend resolves this directory itself, against its own
        // database, with its own credential. Two instances may legitimately
        // disagree, and neither answer is allowed to leak into the other.
        let binding = tokio::time::timeout_at(
            deadline,
            resolve_binding_capped(
                &discovery.backends[&spec.alias].client,
                &spec.alias,
                &spec.url,
                credential.as_deref(),
                &redactor,
            ),
        )
        .await
        .map_err(|_| format!("instance '{}': startup deadline exceeded", spec.alias))?;
        let host = reqwest::Url::parse(&spec.url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_else(|| spec.url.clone());
        routed.push(RoutedInstance {
            alias: spec.alias.clone(),
            host,
            bound_project: binding,
            authenticated: credential.is_some(),
        });
    }

    let unified = unify_surfaces(&surfaces).map_err(|error| redactor.scrub(&error))?;
    let aliases: Vec<String> = routed
        .iter()
        .map(|instance| instance.alias.clone())
        .collect();
    let tools = advertise(unified, &aliases, config.default.as_deref());

    tracing::info!(
        instances = %aliases.join(", "),
        default = config.default.as_deref().unwrap_or("(none)"),
        tools = tools.len(),
        "lific multi-instance MCP proxy started (stdio)"
    );

    let router = Router::new(routed, config.default, tools, redactor);
    let transport = HttpBackends { backends };
    pump(
        BufReader::new(tokio::io::stdin()),
        tokio::io::stdout(),
        &router,
        &transport,
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests;
