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
async fn pump<R, W, F>(input: R, output: W, forwarder: &F) -> std::io::Result<()>
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

        let outcome = match forwarder.forward(line).await {
            // Validate before relaying: a body that is not JSON is a broken
            // remote, and the client deserves an error carrying its own id
            // rather than a garbage frame.
            Ok(body) => match serde_json::from_str::<Value>(&body) {
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
    let endpoint = format!("{}/mcp", url.trim_end_matches('/'));
    let forwarder = HttpForwarder {
        client: reqwest::Client::builder().build()?,
        endpoint,
        credential,
    };

    tracing::info!(
        endpoint = %forwarder.endpoint,
        authenticated = forwarder.credential.is_some(),
        "lific MCP proxy started (stdio)"
    );

    pump(
        BufReader::new(tokio::io::stdin()),
        tokio::io::stdout(),
        &forwarder,
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
        let mut output: Vec<u8> = Vec::new();
        pump(BufReader::new(input.as_bytes()), &mut output, forwarder)
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

        let result = pump(BufReader::new(&b""[..]), &mut output, &forwarder).await;

        assert!(result.is_ok());
        assert!(output.is_empty());
        assert!(forwarder.received().is_empty());
    }
}
