//! `lific mcp --remote --url <URL>`: a stdio MCP proxy.
//!
//! An AI client launches this process the same way it launches the local
//! stdio server, but instead of opening a SQLite database it forwards every
//! JSON-RPC request to a remote Lific instance's `/mcp` endpoint. That is what
//! gives a remote deployment a local presence: the client's config still names
//! a command, the data still lives on the server.
//!
//! Two framing facts drive the whole module:
//!
//! - MCP stdio is newline-delimited JSON-RPC, one message per line. Logs go to
//!   stderr only, because a stray stdout line corrupts the session.
//! - The remote endpoint is StreamableHTTP in stateless JSON mode, so a POSTed
//!   request answers with its own JSON-RPC response as `application/json`.
//!   There is no session to establish and nothing to notify, which is why
//!   notifications are dropped rather than forwarded.
//!
//! The loop never dies from a bad response. A remote that is down, throwing
//! 500s, or rejecting the credential turns into a JSON-RPC error carrying the
//! original request id, so the client sees a failed call instead of a dead
//! server. Only EOF on stdin ends the process.
//!
//! LIF-453: the proxy is also repository-aware, which is the one thing it does
//! that plain forwarding cannot. The local stdio server resolves the working
//! directory's binding against its own database at startup; a remote instance
//! has no idea what directory the client was launched in, so the proxy
//! resolves it here (once, before the pump, via `POST /api/repos/resolve`) and
//! then applies the binding on the wire: an omitted `project` on a
//! project-scoped `tools/call` is filled in on the way out, and the
//! `initialize` instructions say which project the session landed on. Every
//! step of the resolution is soft, because a proxy that refuses to start is
//! far worse than one that forwards verbatim.

use std::error::Error;

use reqwest::StatusCode;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};

/// A forward to the remote instance that produced no JSON-RPC response.
///
/// The message is already phrased for a human, because an agent will paste it
/// in front of one verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ForwardError {
    message: String,
}

impl ForwardError {
    /// The remote could not be reached, or answered with something that is not
    /// a JSON-RPC response.
    fn unreachable(detail: impl std::fmt::Display) -> Self {
        Self {
            message: format!("remote lific unreachable: {}", tidy(&detail.to_string())),
        }
    }

    /// The remote is up and said no. Name the two ways to fix a credential,
    /// since this string is the only thing the human will see.
    fn rejected(status: StatusCode) -> Self {
        Self {
            message: format!(
                "remote lific rejected the credential (HTTP {status}): run `lific login` for this \
                 server, or set LIFIC_API_KEY to a valid API key"
            ),
        }
    }
}

/// Collapse an error detail to one short single-line fragment. Error bodies
/// can be HTML pages or multi-line stack traces, and this string ends up
/// inside a single-line JSON-RPC message.
fn tidy(detail: &str) -> String {
    let mut cleaned: String = detail
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.chars().count() > 200 {
        cleaned = cleaned.chars().take(200).collect::<String>() + "…";
    }
    if cleaned.is_empty() {
        "no detail".to_owned()
    } else {
        cleaned
    }
}

/// Sends one raw JSON-RPC request body to the remote and returns its raw
/// response body. Abstracted so the pump can be tested without a server.
///
/// Written as an explicit `impl Future + Send` rather than `async fn` so the
/// returned future is `Send`; `clippy::future_not_send` is denied here.
trait Forwarder {
    fn forward(
        &self,
        body: String,
    ) -> impl std::future::Future<Output = Result<String, ForwardError>> + Send;
}

/// The real forwarder: one POST per request against `{url}/mcp`.
struct HttpForwarder {
    client: reqwest::Client,
    endpoint: String,
    credential: Option<String>,
}

impl Forwarder for HttpForwarder {
    async fn forward(&self, body: String) -> Result<String, ForwardError> {
        let mut request = self
            .client
            .post(&self.endpoint)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .body(body);
        if let Some(credential) = &self.credential {
            request = request.bearer_auth(credential);
        }

        let response = request.send().await.map_err(ForwardError::unreachable)?;
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(ForwardError::rejected(status));
        }
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            return Err(ForwardError::unreachable(format!(
                "HTTP {status} from {}: {}",
                self.endpoint,
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
                self.endpoint,
                if content_type.is_empty() {
                    "(none)"
                } else {
                    &content_type
                }
            )));
        }

        response.text().await.map_err(ForwardError::unreachable)
    }
}

/// `{"jsonrpc":"2.0","id":null,"error":{"code":-32700,…}}`
fn parse_error_response(detail: &str) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": Value::Null,
        "error": { "code": -32700, "message": format!("parse error: {}", tidy(detail)) },
    })
}

/// `{"jsonrpc":"2.0","id":<id>,"error":{"code":-32603,…}}`
fn internal_error_response(id: &Value, message: &str) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id.clone(),
        "error": { "code": -32603, "message": message },
    })
}

/// Write one stdio frame: the payload flattened to a single line, then `\n`,
/// then a flush so the client sees it immediately.
///
/// Stripping raw CR/LF cannot corrupt the JSON, because a literal newline is
/// illegal inside a JSON string; only insignificant whitespace between tokens
/// can carry one. So a pretty-printed remote response is passed through byte
/// for byte apart from its line breaks, rather than re-serialized (which would
/// reorder keys and renormalize numbers on their way to the client).
async fn write_line<W: AsyncWrite + Unpin + Send>(
    output: &mut W,
    payload: &str,
) -> std::io::Result<()> {
    let mut line: String = payload
        .chars()
        .filter(|c| *c != '\n' && *c != '\r')
        .collect();
    line.push('\n');
    output.write_all(line.as_bytes()).await?;
    output.flush().await
}

// ── Repository binding (LIF-453) ─────────────────────────────

/// What the proxy appends to the relayed `initialize` instructions when this
/// directory resolved to no project.
///
/// The in-process stdio server deliberately stays silent when unbound: it is
/// launched from the repository it serves, and an unbound directory there is
/// usually a deliberate choice. The proxy is the opposite case. It is the
/// entry point people reach for when their data lives on a server they did
/// not set up, so "you could bind this" is news, and one sentence is cheap
/// next to an agent guessing project identifiers.
const UNBOUND_BINDING_NOTE: &str = " No repository binding resolved for this directory; run \
     'lific bind' here to bind it to a project.";

/// The wire name for an alias kind, matching what `/api/repos/resolve` takes.
fn alias_kind(kind: &crate::repo_identity::AliasKind) -> &'static str {
    match kind {
        crate::repo_identity::AliasKind::Remote => "remote",
        crate::repo_identity::AliasKind::Root => "root",
    }
}

/// Read `/api/repos/resolve`'s answer. Only `"one"` binds.
///
/// `"none"` is the common, uninteresting case (an unbound checkout) and stays
/// quiet. `"conflict"` is a state only a human can settle, so it says so.
fn binding_from_resolution(resolved: &Value) -> Option<String> {
    match resolved["resolution"].as_str().unwrap_or_default() {
        "one" => resolved["project"]["identifier"]
            .as_str()
            .map(str::to_owned),
        "conflict" => {
            tracing::warn!(
                "this repository is bound to more than one visible project; the session will \
                 proceed unbound — run `lific bind <PROJECT>` here to settle it"
            );
            None
        }
        _ => None,
    }
}

/// Resolve the current directory's project binding against the remote, once,
/// before the pump starts.
///
/// Every failure is soft and returns `None`: no git, not a repository, no
/// identity, an unreachable or unauthenticated server, a conflict. Each one
/// logs a single stderr line and the session proceeds unbound, behaving
/// exactly as the proxy did before this feature existed.
async fn resolve_binding(
    client: &reqwest::Client,
    url: &str,
    credential: Option<&str>,
) -> Option<String> {
    let dir = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(error) => {
            tracing::info!(%error, "unbound session: could not read the current directory");
            return None;
        }
    };
    let aliases = match crate::repo_identity::compute(&dir) {
        Ok(aliases) if aliases.is_empty() => {
            tracing::info!("unbound session: this directory has no repository identity");
            return None;
        }
        Ok(aliases) => aliases,
        Err(error) => {
            tracing::info!(%error, "unbound session: no repository identity for this directory");
            return None;
        }
    };

    let body = serde_json::json!({
        "aliases": aliases
            .iter()
            .map(|alias| serde_json::json!({
                "kind": alias_kind(&alias.kind),
                "value": alias.value,
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
                %endpoint,
                error = %tidy(&error.to_string()),
                "unbound session: could not ask the remote what this repository is bound to"
            );
            return None;
        }
    };
    if !response.status().is_success() {
        tracing::warn!(
            %endpoint,
            status = %response.status(),
            "unbound session: the remote refused the repository lookup"
        );
        return None;
    }
    let resolved: Value = match response.json().await {
        Ok(resolved) => resolved,
        Err(error) => {
            tracing::warn!(
                error = %tidy(&error.to_string()),
                "unbound session: the remote's repository lookup was not JSON"
            );
            return None;
        }
    };

    let bound = binding_from_resolution(&resolved);
    match &bound {
        Some(project) => tracing::info!(%project, "session bound to project"),
        None => tracing::debug!("session is unbound"),
    }
    bound
}

/// Fill an omitted `project` on an outbound `tools/call` from the binding.
///
/// Returns the rewritten request body, or `None` to forward the original line
/// untouched. Untouched covers everything that is not an injectable call:
/// a different method, an unbound session, a tool where omitting `project`
/// already means something (see
/// [`crate::mcp::tools::project_fallback_applies`]), an argument the client
/// set explicitly, and any params shape that is not what `tools/call`
/// declares. Malformed params are the server's to reject, not the proxy's.
fn inject_bound_project(message: &Value, bound: Option<&str>) -> Option<String> {
    let bound = bound?;
    if message.get("method").and_then(Value::as_str) != Some("tools/call") {
        return None;
    }

    let params = message.get("params")?.as_object()?;
    let name = params.get("name")?.as_str()?;
    let arguments = params.get("arguments")?.as_object()?;
    let resource_type = arguments.get("resource_type").and_then(Value::as_str);
    if !crate::mcp::tools::project_fallback_applies(name, resource_type) {
        return None;
    }
    // An explicit project always wins. `null` is treated as absent: it is what
    // a client sends for an unset optional field, and the server reads it the
    // same way.
    if !matches!(arguments.get("project"), None | Some(Value::Null)) {
        return None;
    }

    let mut patched = message.clone();
    patched["params"]["arguments"]["project"] = Value::String(bound.to_owned());
    Some(encode(&patched))
}

/// Append this session's binding status to a relayed `initialize` result.
///
/// Returns the rewritten response body, or `None` to relay the original bytes.
/// A result with no `instructions` string is relayed as-is rather than grown
/// one: the note is a footnote on the server's guidance, not a substitute.
fn augment_initialize(response: &Value, bound: Option<&str>) -> Option<String> {
    let instructions = response
        .get("result")?
        .get("instructions")?
        .as_str()?
        .to_owned();
    let note = match bound {
        Some(project) => crate::mcp::bound_project_note(project),
        None => UNBOUND_BINDING_NOTE.to_owned(),
    };
    let mut patched = response.clone();
    patched["result"]["instructions"] = Value::String(format!("{instructions}{note}"));
    Some(encode(&patched))
}

/// Serialize a response this module built. Constructed `json!` values always
/// serialize, but a proxy that dies on stdout is worse than one that says so.
fn encode(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| {
        r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"lific proxy could not serialize a response"}}"#
            .to_owned()
    })
}

/// Read newline-delimited JSON-RPC from `input`, forward requests, and write
/// exactly one line to `output` per request. Returns once `input` hits EOF.
///
/// `bound` is the project this working directory resolved to, if any. It is
/// the only reason a request is ever anything but verbatim.
async fn pump<R, W, F>(
    input: R,
    output: W,
    forwarder: &F,
    bound: Option<&str>,
) -> std::io::Result<()>
where
    R: AsyncBufRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
    F: Forwarder + Sync,
{
    let mut lines = input.lines();
    let mut output = output;

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let message: Value = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(error) => {
                write_line(
                    &mut output,
                    &encode(&parse_error_response(&error.to_string())),
                )
                .await?;
                continue;
            }
        };

        // No `id` member means a notification. The remote is stateless and
        // has nothing to do with it, so it is dropped rather than forwarded.
        let Some(id) = message.get("id").cloned() else {
            continue;
        };

        let is_initialize = message.get("method").and_then(Value::as_str) == Some("initialize");
        let request = inject_bound_project(&message, bound).unwrap_or(line);

        let outcome = match forwarder.forward(request).await {
            // Validate before relaying: a body that is not JSON is a broken
            // remote, and the client deserves an error carrying its own id
            // rather than a garbage frame.
            Ok(body) => match serde_json::from_str::<Value>(&body) {
                Ok(value) if is_initialize => augment_initialize(&value, bound).unwrap_or(body),
                Ok(_) => body,
                Err(error) => encode(&internal_error_response(
                    &id,
                    &ForwardError::unreachable(format!("response was not JSON ({error})")).message,
                )),
            },
            Err(error) => encode(&internal_error_response(&id, &error.message)),
        };
        write_line(&mut output, &outcome).await?;
    }

    Ok(())
}

/// Run the proxy against `url`, pumping this process's stdin and stdout.
///
/// `credential` is optional: an auth-optional instance takes requests with no
/// `Authorization` header at all, so `None` sends none rather than failing.
pub async fn run(url: String, credential: Option<String>) -> Result<(), Box<dyn Error>> {
    let client = reqwest::Client::builder().build()?;
    // Resolved before the pump, so the very first `initialize` already knows
    // the answer and no request is ever forwarded against a stale binding.
    let bound = resolve_binding(&client, &url, credential.as_deref()).await;

    let endpoint = format!("{}/mcp", url.trim_end_matches('/'));
    let forwarder = HttpForwarder {
        client,
        endpoint,
        credential,
    };

    tracing::info!(
        endpoint = %forwarder.endpoint,
        authenticated = forwarder.credential.is_some(),
        bound_project = bound.as_deref().unwrap_or("(none)"),
        "lific MCP proxy started (stdio)"
    );

    pump(
        BufReader::new(tokio::io::stdin()),
        tokio::io::stdout(),
        &forwarder,
        bound.as_deref(),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    type Reply = Box<dyn Fn(&str) -> Result<String, ForwardError> + Send + Sync>;

    struct MockForwarder {
        received: Mutex<Vec<String>>,
        reply: Reply,
    }

    impl MockForwarder {
        fn replying(body: &str) -> Self {
            let body = body.to_owned();
            Self::new(move |_| Ok(body.clone()))
        }

        fn failing(error: ForwardError) -> Self {
            Self::new(move |_| Err(error.clone()))
        }

        fn new(
            reply: impl Fn(&str) -> Result<String, ForwardError> + Send + Sync + 'static,
        ) -> Self {
            Self {
                received: Mutex::new(Vec::new()),
                reply: Box::new(reply),
            }
        }

        fn received(&self) -> Vec<String> {
            self.received.lock().unwrap().clone()
        }
    }

    impl Forwarder for MockForwarder {
        async fn forward(&self, body: String) -> Result<String, ForwardError> {
            let result = (self.reply)(&body);
            self.received.lock().unwrap().push(body);
            result
        }
    }

    /// Run the pump over `input` and return the stdout lines it produced.
    async fn run_pump(input: &str, forwarder: &MockForwarder) -> Vec<Value> {
        run_pump_bound(input, forwarder, None).await
    }

    /// [`run_pump`] for a session bound to `bound`.
    async fn run_pump_bound(
        input: &str,
        forwarder: &MockForwarder,
        bound: Option<&str>,
    ) -> Vec<Value> {
        let mut output: Vec<u8> = Vec::new();
        pump(
            BufReader::new(input.as_bytes()),
            &mut output,
            forwarder,
            bound,
        )
        .await
        .expect("pump should not fail on in-memory IO");
        String::from_utf8(output)
            .expect("proxy output is UTF-8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("each stdout line is one JSON value"))
            .collect()
    }

    const REQUEST: &str = r#"{"jsonrpc":"2.0","id":7,"method":"tools/list","params":{}}"#;

    #[tokio::test]
    async fn a_request_is_forwarded_verbatim_and_its_reply_lands_on_stdout() {
        let forwarder =
            MockForwarder::replying(r#"{"jsonrpc":"2.0","id":7,"result":{"tools":[]}}"#);

        let out = run_pump(&format!("{REQUEST}\n"), &forwarder).await;

        assert_eq!(forwarder.received(), vec![REQUEST.to_owned()]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["id"], serde_json::json!(7));
        assert_eq!(out[0]["result"]["tools"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn a_pretty_printed_reply_is_written_as_a_single_line() {
        let forwarder = MockForwarder::replying("{\n  \"jsonrpc\": \"2.0\",\n  \"id\": 7\n}");

        let mut output: Vec<u8> = Vec::new();
        pump(
            BufReader::new(format!("{REQUEST}\n").as_bytes()),
            &mut output,
            &forwarder,
            None,
        )
        .await
        .unwrap();

        let written = String::from_utf8(output).unwrap();
        assert_eq!(written.matches('\n').count(), 1, "exactly one frame");
        assert!(written.ends_with('\n'));
        assert_eq!(written, "{  \"jsonrpc\": \"2.0\",  \"id\": 7}\n");
    }

    #[tokio::test]
    async fn a_notification_is_swallowed_and_never_forwarded() {
        let forwarder = MockForwarder::replying(r#"{"jsonrpc":"2.0","id":1}"#);

        let out = run_pump(
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            &forwarder,
        )
        .await;

        assert!(forwarder.received().is_empty(), "nothing should be sent");
        assert!(out.is_empty(), "a notification produces no stdout line");
    }

    #[tokio::test]
    async fn an_unparseable_line_answers_minus_32700_and_the_loop_continues() {
        let forwarder =
            MockForwarder::replying(r#"{"jsonrpc":"2.0","id":7,"result":{"tools":[]}}"#);

        let out = run_pump(&format!("not json at all\n{REQUEST}\n"), &forwarder).await;

        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["error"]["code"], serde_json::json!(-32700));
        assert_eq!(out[0]["id"], Value::Null);
        assert_eq!(out[1]["id"], serde_json::json!(7));
        assert!(
            out[1].get("result").is_some(),
            "the next request still works"
        );
    }

    #[tokio::test]
    async fn a_forward_failure_answers_minus_32603_with_the_request_id_and_the_loop_continues() {
        let forwarder = MockForwarder::new(|body| {
            if body.contains("\"id\":7") {
                Err(ForwardError::unreachable("connection refused"))
            } else {
                Ok(r#"{"jsonrpc":"2.0","id":8,"result":{}}"#.to_owned())
            }
        });

        let out = run_pump(
            &format!("{REQUEST}\n{{\"jsonrpc\":\"2.0\",\"id\":8,\"method\":\"ping\"}}\n"),
            &forwarder,
        )
        .await;

        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["id"], serde_json::json!(7));
        assert_eq!(out[0]["error"]["code"], serde_json::json!(-32603));
        assert_eq!(
            out[0]["error"]["message"],
            serde_json::json!("remote lific unreachable: connection refused")
        );
        assert_eq!(out[1]["id"], serde_json::json!(8));
        assert!(out[1].get("result").is_some(), "the loop survived");
    }

    #[tokio::test]
    async fn an_auth_rejected_forward_names_lific_login_and_lific_api_key() {
        let forwarder = MockForwarder::failing(ForwardError::rejected(StatusCode::UNAUTHORIZED));

        let out = run_pump(&format!("{REQUEST}\n"), &forwarder).await;

        let message = out[0]["error"]["message"].as_str().unwrap();
        assert_eq!(out[0]["error"]["code"], serde_json::json!(-32603));
        assert!(message.contains("lific login"), "got: {message}");
        assert!(message.contains("LIFIC_API_KEY"), "got: {message}");
        assert!(message.contains("rejected"), "got: {message}");
    }

    #[tokio::test]
    async fn a_non_json_reply_answers_minus_32603_with_the_request_id() {
        let forwarder = MockForwarder::replying("<html>gateway timeout</html>");

        let out = run_pump(&format!("{REQUEST}\n"), &forwarder).await;

        assert_eq!(out[0]["id"], serde_json::json!(7));
        assert_eq!(out[0]["error"]["code"], serde_json::json!(-32603));
    }

    #[tokio::test]
    async fn end_of_input_ends_the_pump_cleanly() {
        let forwarder = MockForwarder::replying(r#"{"jsonrpc":"2.0","id":7}"#);
        let mut output: Vec<u8> = Vec::new();

        let result = pump(BufReader::new(&b""[..]), &mut output, &forwarder, None).await;

        assert!(result.is_ok());
        assert!(output.is_empty());
        assert!(forwarder.received().is_empty());
    }

    // ── LIF-453: the binding applied on the wire ─────────────

    /// One `tools/call` line, with `arguments` spelled exactly as given.
    fn call(name: &str, arguments: Value) -> String {
        serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments },
        }))
        .unwrap()
    }

    /// The `arguments` object of the single request the mock received.
    fn forwarded_arguments(forwarder: &MockForwarder) -> Value {
        let received = forwarder.received();
        assert_eq!(received.len(), 1, "exactly one request was forwarded");
        let sent: Value = serde_json::from_str(&received[0]).expect("forwarded body is JSON");
        sent["params"]["arguments"].clone()
    }

    const OK: &str = r#"{"jsonrpc":"2.0","id":1,"result":{}}"#;

    #[tokio::test]
    async fn a_bound_session_fills_in_the_omitted_project_on_list_issues() {
        let forwarder = MockForwarder::replying(OK);

        run_pump_bound(
            &format!("{}\n", call("list_issues", serde_json::json!({}))),
            &forwarder,
            Some("BND"),
        )
        .await;

        assert_eq!(
            forwarded_arguments(&forwarder),
            serde_json::json!({ "project": "BND" })
        );
    }

    #[tokio::test]
    async fn a_null_project_counts_as_omitted_and_is_filled_in() {
        let forwarder = MockForwarder::replying(OK);

        run_pump_bound(
            &format!(
                "{}\n",
                call(
                    "create_issue",
                    serde_json::json!({ "title": "T", "project": null })
                )
            ),
            &forwarder,
            Some("BND"),
        )
        .await;

        assert_eq!(forwarded_arguments(&forwarder)["project"], "BND");
    }

    #[tokio::test]
    async fn an_explicit_project_is_never_overwritten_by_the_binding() {
        let forwarder = MockForwarder::replying(OK);
        let request = call("list_issues", serde_json::json!({ "project": "OTH" }));

        run_pump_bound(&format!("{request}\n"), &forwarder, Some("BND")).await;

        assert_eq!(
            forwarder.received(),
            vec![request],
            "an explicit project forwards byte-identical"
        );
    }

    #[tokio::test]
    async fn an_unbound_session_forwards_a_project_less_call_untouched() {
        let forwarder = MockForwarder::replying(OK);
        let request = call("list_issues", serde_json::json!({}));

        run_pump_bound(&format!("{request}\n"), &forwarder, None).await;

        assert_eq!(forwarder.received(), vec![request]);
    }

    #[tokio::test]
    async fn search_and_create_page_are_never_narrowed_by_the_binding() {
        for request in [
            call("search", serde_json::json!({ "query": "anything" })),
            call(
                "create_page",
                serde_json::json!({ "title": "Workspace note" }),
            ),
            call("bulk_update", serde_json::json!({ "identifiers": ["A-1"] })),
            call(
                "delete",
                serde_json::json!({ "resource_type": "issue", "identifier": "A-1" }),
            ),
        ] {
            let forwarder = MockForwarder::replying(OK);

            run_pump_bound(&format!("{request}\n"), &forwarder, Some("BND")).await;

            assert_eq!(
                forwarder.received(),
                vec![request.clone()],
                "meaning must be preserved for: {request}"
            );
        }
    }

    #[tokio::test]
    async fn list_resources_takes_the_binding_for_issues_but_not_for_pages() {
        let forwarder = MockForwarder::replying(OK);
        run_pump_bound(
            &format!(
                "{}\n",
                call(
                    "list_resources",
                    serde_json::json!({ "resource_type": "issue" })
                )
            ),
            &forwarder,
            Some("BND"),
        )
        .await;
        assert_eq!(forwarded_arguments(&forwarder)["project"], "BND");

        let pages = call(
            "list_resources",
            serde_json::json!({ "resource_type": "page" }),
        );
        let forwarder = MockForwarder::replying(OK);
        run_pump_bound(&format!("{pages}\n"), &forwarder, Some("BND")).await;
        assert_eq!(forwarder.received(), vec![pages]);
    }

    #[tokio::test]
    async fn a_malformed_tools_call_is_forwarded_for_the_server_to_reject() {
        for request in [
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call"}"#.to_owned(),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":"nonsense"}"#.to_owned(),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_issues"}}"#
                .to_owned(),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":7,"arguments":{}}}"#
                .to_owned(),
        ] {
            let forwarder = MockForwarder::replying(OK);

            run_pump_bound(&format!("{request}\n"), &forwarder, Some("BND")).await;

            assert_eq!(forwarder.received(), vec![request.clone()], "{request}");
        }
    }

    const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;

    /// An `initialize` reply carrying the server's instructions verbatim.
    fn initialize_reply(instructions: &str) -> String {
        serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "protocolVersion": "2025-03-26", "instructions": instructions },
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn a_bound_session_appends_the_servers_own_binding_note_to_initialize() {
        let forwarder = MockForwarder::replying(&initialize_reply("Base guidance."));

        let out = run_pump_bound(&format!("{INITIALIZE}\n"), &forwarder, Some("BND")).await;

        assert_eq!(
            out[0]["result"]["instructions"],
            serde_json::json!(format!(
                "Base guidance.{}",
                crate::mcp::bound_project_note("BND")
            )),
            "the note is appended to the server's words, never a rewrite"
        );
        assert_eq!(
            out[0]["result"]["protocolVersion"],
            serde_json::json!("2025-03-26"),
            "the rest of the result survives"
        );
    }

    /// Parity with the in-process server: an agent must be told the same thing
    /// whether it reached Lific over stdio or through this proxy.
    #[tokio::test]
    async fn the_proxys_binding_note_is_the_servers_binding_note() {
        let forwarder = MockForwarder::replying(&initialize_reply(""));

        let out = run_pump_bound(&format!("{INITIALIZE}\n"), &forwarder, Some("LIF")).await;

        assert_eq!(
            out[0]["result"]["instructions"].as_str().unwrap(),
            crate::mcp::bound_project_note("LIF"),
            "the proxy must not carry its own copy of this sentence"
        );
    }

    #[tokio::test]
    async fn an_unbound_session_appends_the_hint_that_names_lific_bind() {
        let forwarder = MockForwarder::replying(&initialize_reply("Base guidance."));

        let out = run_pump_bound(&format!("{INITIALIZE}\n"), &forwarder, None).await;

        let instructions = out[0]["result"]["instructions"].as_str().unwrap();
        assert!(
            instructions.starts_with("Base guidance."),
            "got: {instructions}"
        );
        assert!(instructions.contains("lific bind"), "got: {instructions}");
        assert!(
            instructions.contains("No repository binding resolved"),
            "got: {instructions}"
        );
    }

    #[tokio::test]
    async fn an_initialize_result_without_instructions_is_relayed_untouched() {
        let reply = r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-03-26"}}"#;
        let forwarder = MockForwarder::replying(reply);

        let mut output: Vec<u8> = Vec::new();
        pump(
            BufReader::new(format!("{INITIALIZE}\n").as_bytes()),
            &mut output,
            &forwarder,
            Some("BND"),
        )
        .await
        .unwrap();

        assert_eq!(String::from_utf8(output).unwrap(), format!("{reply}\n"));
    }

    #[tokio::test]
    async fn a_tools_list_response_passes_through_byte_identical_when_bound() {
        let reply = r#"{"jsonrpc":"2.0","id":7,"result":{"instructions":"untouched","tools":[]}}"#;
        let forwarder = MockForwarder::replying(reply);

        let mut output: Vec<u8> = Vec::new();
        pump(
            BufReader::new(format!("{REQUEST}\n").as_bytes()),
            &mut output,
            &forwarder,
            Some("BND"),
        )
        .await
        .unwrap();

        assert_eq!(forwarder.received(), vec![REQUEST.to_owned()]);
        assert_eq!(String::from_utf8(output).unwrap(), format!("{reply}\n"));
    }

    // ── the startup resolution's reading of /api/repos/resolve ──

    #[test]
    fn only_a_single_resolution_binds_the_session() {
        assert_eq!(
            binding_from_resolution(&serde_json::json!({
                "resolution": "one",
                "project": { "identifier": "BND", "name": "Bound" },
            })),
            Some("BND".to_owned())
        );
        for answer in [
            serde_json::json!({ "resolution": "none" }),
            serde_json::json!({
                "resolution": "conflict",
                "projects": [{ "identifier": "A" }, { "identifier": "B" }],
            }),
            serde_json::json!({ "resolution": "one" }),
            serde_json::json!({}),
        ] {
            assert_eq!(binding_from_resolution(&answer), None, "{answer}");
        }
    }
}
