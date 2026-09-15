//! Tolerate SEP-2575 stateless discovery on the stdio transport.
//!
//! rmcp 1.x's serving loop answers only `ping` before `initialize`; any other
//! request (a modern client's `server/discover` probe included) makes it return
//! `ServerInitializeError::ExpectedInitializeRequest` and the session dies with
//! no reply, so a discovery-capable client reads EOF mid-handshake instead of a
//! JSON-RPC error it could fall back from (`client is closing: EOF`). See
//! `BUG-MCP-server-discover-crash.md`.
//!
//! This transport hands rmcp a stream that swallows pre-initialize traffic rmcp
//! would abort on, and answers unsupported requests itself with `-32601`
//! (method not found). rmcp still sees the real `initialize` and serves the rest
//! of the session normally.

use rmcp::model::{
    ClientJsonRpcMessage, ClientRequest, ErrorCode, ErrorData, ServerJsonRpcMessage,
};
use rmcp::service::{RoleServer, RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::{
    IntoTransport, Transport, async_rw::TransportAdapterAsyncRW,
};

/// Wraps a [`Transport`] so rmcp's `serve` loop never sees a pre-initialize
/// message it would reject the session for. Unsupported requests are answered
/// with `-32601` and everything else is dropped until a real `initialize`
/// arrives; after that every message is forwarded untouched.
pub(crate) struct PreInitGuard<T>
where
    T: Transport<RoleServer> + Send + 'static,
{
    inner: T,
    initialized: bool,
}

impl<T> PreInitGuard<T>
where
    T: Transport<RoleServer> + Send + 'static,
{
    pub(crate) fn new(inner: T) -> Self {
        Self { inner, initialized: false }
    }
}

/// A stdio transport that tolerates stateless discovery: build it around the
/// same `(stdin, stdout)` pair `rmcp::transport::io::stdio()` returns and hand
/// it straight to `ServiceExt::serve`.
pub(crate) fn guard_stdio(
    reader: tokio::io::Stdin,
    writer: tokio::io::Stdout,
) -> impl Transport<RoleServer, Error = std::io::Error> + Send + 'static {
    let inner = IntoTransport::<
        RoleServer,
        std::io::Error,
        TransportAdapterAsyncRW
    >::into_transport((reader, writer));
    PreInitGuard::new(inner)
}

impl<T> Transport<RoleServer> for PreInitGuard<T>
where
    T: Transport<RoleServer> + Send + 'static,
{
    type Error = T::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleServer>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        self.inner.send(item)
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        loop {
            match self.inner.receive().await {
                None => return None,
                Some(msg) => {
                    if self.initialized {
                        return Some(msg);
                    }
                    match msg {
                        ClientJsonRpcMessage::Request(req) => match req.request {
                            ClientRequest::PingRequest(_) => return Some(
                                ClientJsonRpcMessage::Request(req),
                            ),
                            ClientRequest::InitializeRequest(_) => {
                                self.initialized = true;
                                return Some(ClientJsonRpcMessage::Request(req));
                            }
                            _ => {
                                // rmcp would abort on this request; answer it
                                // ourselves so a discovery client falls back to
                                // `initialize` instead of seeing EOF.
                                self.inner
                                    .send(ServerJsonRpcMessage::error(
                                        ErrorData::new(
                                            ErrorCode::METHOD_NOT_FOUND,
                                            "Method not found",
                                            None,
                                        ),
                                        req.id.clone(),
                                    ))
                                    .await
                                    .ok();
                                continue
                            }
                        },
                        _ => {
                            // A pre-initialize notification, response or error
                            // has no request to answer; drop it rather than let
                            // rmcp's loop abort the session on it.
                            continue
                        }
                    }
                }
            }
        }
    }

    fn close(
        &mut self,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.close()
    }
}