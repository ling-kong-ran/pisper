//! Auth-provider seam, ported from upstream
//! `packages/mcp/src/auth-provider.ts`.
//!
//! Upstream `McpFetch` is `(input, init) => Promise<Response>` over the
//! platform fetch; the port models the needed slice explicitly: a request
//! (URL, method, ordered headers, string body, cancellation token), a
//! response (status, headers, byte stream), and an error classified as
//! network-level (upstream `TypeError`) or other. The default fetch is
//! reqwest with `no_proxy()` — upstream undici never consults `*_PROXY`
//! environment variables, so the port keeps that behavior.

use std::sync::Arc;

use futures::future::BoxFuture;
use futures::stream::{Stream, StreamExt};
use tokio_util::sync::CancellationToken;

/// A byte stream: owned chunks, `io::Error` on stream failures.
pub type ByteStream = std::pin::Pin<Box<dyn Stream<Item = std::io::Result<Vec<u8>>> + Send>>;

/// An outbound HTTP request issued by the transports or the OAuth flows.
#[derive(Clone)]
pub struct FetchRequest {
    pub url: url::Url,
    pub method: String,
    /// Header name/value pairs in construction order (names sent as given).
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    /// Upstream `AbortSignal` (`StreamableHttpTransport`'s controller).
    pub cancel: CancellationToken,
}

/// A response with a streaming body.
pub struct FetchResponse {
    pub status: u16,
    /// Lowercased header names; lookup helpers are case-insensitive.
    pub headers: Vec<(String, String)>,
    pub body: ByteStream,
}

impl FetchResponse {
    /// First header value for `name` (case-insensitive), or `None`.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Upstream `response.text()`: read the whole body as UTF-8 (lossy, like
    /// the Web API's UTF-8 decode with replacement).
    pub fn into_text(self) -> BoxFuture<'static, Result<String, String>> {
        Box::pin(async move {
            let mut bytes = Vec::new();
            let mut body = self.body;
            while let Some(chunk) = body.next().await {
                bytes.extend_from_slice(&chunk.map_err(|error| error.to_string())?);
            }
            Ok(String::from_utf8_lossy(&bytes).into_owned())
        })
    }

    /// Upstream `response.body?.cancel().catch(() => {})`: drop the stream,
    /// closing the connection or discarding buffered data.
    pub fn discard(self) {}
}

/// Upstream fetch failures: `TypeError` (network-level, retried by the
/// streamable HTTP transport) versus any other error.
#[derive(Debug, Clone)]
pub struct FetchError {
    pub network: bool,
    pub message: String,
}

impl FetchError {
    pub fn network(message: impl Into<String>) -> Self {
        FetchError {
            network: true,
            message: message.into(),
        }
    }

    pub fn other(message: impl Into<String>) -> Self {
        FetchError {
            network: false,
            message: message.into(),
        }
    }
}

/// Upstream `McpFetch`.
pub type McpFetch = Arc<
    dyn Fn(FetchRequest) -> BoxFuture<'static, Result<FetchResponse, FetchError>> + Send + Sync,
>;

/// The default `globalThis.fetch` equivalent: reqwest with no proxy (undici
/// ignores proxy environment variables), no request timeout, and no idle
/// connection reuse beyond reqwest's pool.
pub fn default_fetch() -> McpFetch {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    let client = CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("default MCP fetch client must build")
        })
        .clone();
    Arc::new(move |request: FetchRequest| {
        let client = client.clone();
        Box::pin(async move {
            let method = reqwest::Method::from_bytes(request.method.as_bytes())
                .map_err(|error| FetchError::other(error.to_string()))?;
            let mut builder = client.request(method, request.url.clone());
            for (name, value) in &request.headers {
                builder = builder.header(name, value);
            }
            if let Some(body) = request.body {
                builder = builder.body(body);
            }
            let future = builder.send();
            tokio::select! {
                _ = request.cancel.cancelled() => Err(FetchError::other("This operation was aborted")),
                response = future => match response {
                    Ok(response) => {
                        let status = response.status().as_u16();
                        let mut headers = Vec::new();
                        for (name, value) in response.headers() {
                            headers.push((name.as_str().to_ascii_lowercase(), value.to_str().unwrap_or("").to_string()));
                        }
                        let stream = response.bytes_stream();
                        let body: ByteStream = Box::pin(futures::stream::unfold(
                            (stream, false),
                            |(mut stream, mut done)| async move {
                                if done {
                                    return None;
                                }
                                match futures::StreamExt::next(&mut stream).await {
                                    Some(Ok(chunk)) => Some((Ok(chunk.to_vec()), (stream, false))),
                                    Some(Err(error)) => {
                                        done = true;
                                        Some((Err(std::io::Error::other(error.to_string())), (stream, done)))
                                    }
                                    None => None,
                                }
                            },
                        ));
                        Ok(FetchResponse { status, headers, body })
                    }
                    Err(error) => {
                        // Network-level failures (connect, send, body transfer,
                        // decode, timeout) map to upstream TypeError; builder
                        // errors cannot occur for executed requests.
                        if error.is_builder() {
                            Err(FetchError::other(error.to_string()))
                        } else {
                            Err(FetchError::network(error.to_string()))
                        }
                    }
                },
            }
        })
    })
}

/// Upstream `UnauthorizedContext`: what a 401 (or a 403 step-up challenge)
/// hands to the auth provider.
pub struct UnauthorizedContext {
    /// The 401 response, or a 403 response whose challenge reports
    /// `insufficient_scope`.
    pub response: FetchResponse,
    pub server_url: url::Url,
    pub fetch: McpFetch,
    /// Access token the rejected request carried, if any. A different current
    /// token means another request already refreshed it.
    pub token: Option<String>,
}

/// Supplies bearer tokens to an MCP HTTP transport and may refresh them after
/// a 401 response. Upstream `AuthProvider`.
pub trait AuthProvider: Send + Sync {
    /// Upstream `token()`.
    fn token(&self) -> BoxFuture<'_, Option<String>>;

    /// Whether the transport should hand 401/403-step-up responses to
    /// [`AuthProvider::on_unauthorized`]. Upstream models this as the method
    /// simply being absent from the provider object.
    fn handles_unauthorized(&self) -> bool {
        false
    }

    /// Upstream optional `onUnauthorized(context)`.
    fn on_unauthorized<'a>(
        &'a self,
        _context: UnauthorizedContext,
    ) -> BoxFuture<'a, Result<(), crate::mcp::protocol::jsonrpc::McpClientError>> {
        Box::pin(async { Ok(()) })
    }
}

/// Convenience alias for auth-provider implementations.
pub type SharedAuthProvider = Arc<dyn AuthProvider>;
