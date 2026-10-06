//! The loopback OAuth redirect receiver, ported from upstream
//! `packages/mcp/src/oauth/callback.ts`: a local HTTP server that resolves
//! per-state `waitForCallback` promises with the authorization code, answers
//! the browser with a plain-text (or caller-rendered HTML) page, and rejects
//! pending waiters on timeout or `close()`.
//!
//! Port notes (disclosed divergences):
//! - The listener is a tokio TCP listener; one task per connection answers
//!   exactly one HTTP/1.1 request and closes the socket. Node's response
//!   framing (`transfer-encoding: chunked`, `Date`) is not reproducible; the
//!   pinned contract is status, content type, and the byte-exact body (the
//!   same ruling as the `ai` package's loopback callback server).
//! - `renderPage` maps to a boxed renderer; the plain-text default matches
//!   the upstream `plainText` strings byte for byte.
//! - The capture's `agent: false` note (Connection: close so `server.close()`
//!   is not blocked by keep-alive sockets) is inherent here: every connection
//!   closes after exactly one response.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

use crate::ai::auth::oauth::{
    first_pair, parse_urlencoded_pairs, read_request_head, request_target,
};

/// Upstream `OAuthCallback`.
#[derive(Debug, Clone, PartialEq)]
pub struct OAuthCallback {
    pub code: String,
    pub state: String,
    pub iss: Option<String>,
}

/// Outcome shown on the browser page after the redirect (upstream
/// `OAuthCallbackPage`).
#[derive(Debug, Clone, PartialEq)]
pub enum OAuthCallbackPage {
    Ok,
    Error {
        message: String,
        details: Option<String>,
    },
}

/// Upstream `plainText(page)`.
pub fn plain_text(page: &OAuthCallbackPage) -> String {
    match page {
        OAuthCallbackPage::Ok => "Authorization complete. You may close this window.".to_string(),
        OAuthCallbackPage::Error { message, details } => match details {
            Some(details) => format!("{message}\n\n{details}"),
            None => message.clone(),
        },
    }
}

/// Upstream `OAuthCallbackServerOptions`.
#[derive(Clone, Default)]
pub struct OAuthCallbackServerOptions {
    /// Address to listen on. Default: `127.0.0.1`.
    pub host: Option<String>,
    /// Host name in `redirectUrl`, for example `localhost` for a client
    /// registered with it while listening on `127.0.0.1`. Default: `host`.
    pub redirect_host: Option<String>,
    pub port: Option<u16>,
    pub path: Option<String>,
    /// More paths that receive the callback, for example a server-specific
    /// path of a redirect URI (v1.0.0).
    pub extra_paths: Option<Vec<String>>,
    pub timeout_ms: Option<u64>,
    /// Render the browser page as HTML. Default: a plain-text message.
    pub render_page: Option<RenderPage>,
}

/// The pending map: state -> the sender resolving that waiter, with the
/// exact path the waiter expects (v1.0.0; `None` accepts any listed path).
type PendingMap = HashMap<
    String,
    (
        tokio::sync::oneshot::Sender<Result<OAuthCallback, String>>,
        Option<String>,
    ),
>;

/// The `onRedirect`-style page renderer (upstream `renderPage`).
pub type RenderPage = Arc<dyn Fn(&OAuthCallbackPage) -> String + Send + Sync>;

struct CallbackServerShared {
    redirect_url: String,
    /// The paths that receive the callback (v1.0.0): `path` plus
    /// `extraPaths`.
    paths: Vec<String>,
    timeout_ms: u64,
    render_page: Option<RenderPage>,
    pending: std::sync::Mutex<PendingMap>,
    /// Stops the accept loop (upstream `server.close()`).
    shutdown: tokio_util::sync::CancellationToken,
    /// The accept-loop task, joined by `close()`.
    accept_loop: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// Upstream `OAuthCallbackServer`.
pub struct OAuthCallbackServer {
    shared: Arc<CallbackServerShared>,
    redirect_url: String,
}

impl OAuthCallbackServer {
    /// Upstream `OAuthCallbackServer.listen(options)`.
    pub async fn listen(
        options: OAuthCallbackServerOptions,
    ) -> Result<OAuthCallbackServer, String> {
        let host = options.host.unwrap_or_else(|| "127.0.0.1".to_string());
        let redirect_host = options.redirect_host.unwrap_or_else(|| host.clone());
        let path = options.path.unwrap_or_else(|| "/callback".to_string());
        let listener = tokio::net::TcpListener::bind((host.as_str(), options.port.unwrap_or(0)))
            .await
            .map_err(|error| error.to_string())?;
        let port = listener
            .local_addr()
            .map_err(|error| error.to_string())?
            .port();
        // Upstream brackets IPv6 redirect hosts.
        let host_in_url = if redirect_host.contains(':') {
            format!("[{redirect_host}]")
        } else {
            redirect_host
        };
        let redirect_url = format!("http://{host_in_url}:{port}{path}");
        let mut paths = vec![path];
        if let Some(extra_paths) = &options.extra_paths {
            paths.extend(extra_paths.iter().cloned());
        }
        let shared = Arc::new(CallbackServerShared {
            redirect_url,
            paths,
            timeout_ms: options.timeout_ms.unwrap_or(5 * 60_000),
            render_page: options.render_page,
            pending: std::sync::Mutex::new(HashMap::new()),
            shutdown: tokio_util::sync::CancellationToken::new(),
            accept_loop: std::sync::Mutex::new(None),
        });
        let accept_shared = Arc::clone(&shared);
        let accept_loop = tokio::spawn(async move {
            run_accept_loop(listener, accept_shared).await;
        });
        *shared
            .accept_loop
            .lock()
            .expect("accept loop cannot be poisoned") = Some(accept_loop);
        Ok(OAuthCallbackServer {
            redirect_url: shared.redirect_url.clone(),
            shared,
        })
    }

    /// Upstream `redirectUrl`.
    pub fn redirect_url(&self) -> &str {
        &self.redirect_url
    }

    /// Upstream `waitForCallback(state, path?)` (v1.0.0): resolves with the
    /// browser redirect for `state`, or errors after the timeout or on
    /// `close()`. With `path`, a response on another path fails, so a
    /// server-specific redirect URI can tell authorization servers apart
    /// (RFC 9700 section 4.4.2.2). Registration is synchronous (upstream
    /// throws for a duplicate state at call time); the returned future is
    /// 'static.
    pub fn wait_for_callback(
        &self,
        state: &str,
        path: Option<String>,
    ) -> BoxFuture<'static, Result<OAuthCallback, String>> {
        let shared = Arc::clone(&self.shared);
        let state = state.to_string();
        let receiver = {
            let mut pending = shared
                .pending
                .lock()
                .expect("pending map cannot be poisoned");
            if pending.contains_key(&state) {
                return Box::pin(async { Err("OAuth state is already pending".to_string()) });
            }
            let (sender, receiver) = tokio::sync::oneshot::channel();
            pending.insert(state.clone(), (sender, path));
            receiver
        };
        Box::pin(async move {
            // The per-state timeout (upstream `setTimeout(..., timeoutMs)`).
            match tokio::time::timeout(Duration::from_millis(shared.timeout_ms), receiver).await {
                Ok(Ok(result)) => result,
                // The sender was dropped without settling: `close()` already
                // rejected through the map drain, so treat this as closed.
                Ok(Err(_)) => Err("OAuth callback server closed".to_string()),
                Err(_) => {
                    shared
                        .pending
                        .lock()
                        .expect("pending map cannot be poisoned")
                        .remove(&state);
                    Err("OAuth callback timed out".to_string())
                }
            }
        })
    }

    /// Upstream `close()`: reject pending waiters and stop the listener.
    pub async fn close(self) -> Result<(), String> {
        self.shared.shutdown.cancel();
        {
            let mut pending = self
                .shared
                .pending
                .lock()
                .expect("pending map cannot be poisoned");
            for (_, (sender, _)) in pending.drain() {
                let _ = sender.send(Err("OAuth callback server closed".to_string()));
            }
        }
        let accept_loop = self
            .shared
            .accept_loop
            .lock()
            .expect("accept loop cannot be poisoned")
            .take();
        if let Some(accept_loop) = accept_loop {
            let _ = accept_loop.await;
        }
        Ok(())
    }
}

/// Accept loop: one task per connection, one response per connection.
async fn run_accept_loop(listener: TcpListener, shared: Arc<CallbackServerShared>) {
    loop {
        let stream = tokio::select! {
            _ = shared.shutdown.cancelled() => return,
            accepted = listener.accept() => match accepted {
                Ok((stream, _address)) => stream,
                Err(_) => continue,
            },
        };
        let handler_shared = Arc::clone(&shared);
        tokio::spawn(async move {
            let mut stream = stream;
            if let Some(head) = read_request_head(&mut stream).await {
                let target = head.lines().next().and_then(request_target);
                handle_request(&handler_shared, &mut stream, target.unwrap_or("/")).await;
            }
            let _ = stream.shutdown().await;
        });
    }
}

/// Upstream `handle(rawUrl, response)`.
async fn handle_request(
    shared: &Arc<CallbackServerShared>,
    stream: &mut tokio::net::TcpStream,
    raw_target: &str,
) {
    // Upstream `new URL(rawUrl, this.redirectUrl)`.
    let base = url::Url::parse(&shared.redirect_url).expect("redirect URL is a valid URL");
    let url = match url::Url::options().base_url(Some(&base)).parse(raw_target) {
        Ok(url) => url,
        Err(_) => {
            reply(
                stream,
                shared,
                404,
                &OAuthCallbackPage::Error {
                    message: "Not found".to_string(),
                    details: None,
                },
            )
            .await;
            return;
        }
    };
    if !shared.paths.iter().any(|path| path == url.path()) {
        reply(
            stream,
            shared,
            404,
            &OAuthCallbackPage::Error {
                message: "Not found".to_string(),
                details: None,
            },
        )
        .await;
        return;
    }
    let pairs = parse_urlencoded_pairs(url.query().unwrap_or_default());
    let state = first_pair(&pairs, "state");
    let sender = {
        let mut pending = shared
            .pending
            .lock()
            .expect("pending map cannot be poisoned");
        state.as_ref().and_then(|state| pending.remove(state))
    };
    let Some((sender, expected_path)) = sender else {
        reply(
            stream,
            shared,
            400,
            &OAuthCallbackPage::Error {
                message: "Invalid or expired OAuth state".to_string(),
                details: None,
            },
        )
        .await;
        return;
    };
    // v1.0.0: a response on another path fails the waiter, so a
    // server-specific redirect URI can tell authorization servers apart.
    if let Some(expected_path) = &expected_path {
        if url.path() != expected_path {
            let _ = sender.send(Err(
                "The authorization response arrived on another redirect URI".to_string(),
            ));
            reply(
                stream,
                shared,
                400,
                &OAuthCallbackPage::Error {
                    message: "Unexpected redirect URI".to_string(),
                    details: None,
                },
            )
            .await;
            return;
        }
    }
    if let Some(error) = first_pair(&pairs, "error") {
        let description = first_pair(&pairs, "error_description").unwrap_or_else(|| error.clone());
        let _ = sender.send(Err(description.clone()));
        reply(
            stream,
            shared,
            200,
            &OAuthCallbackPage::Error {
                message: "Authorization failed. You may close this window.".to_string(),
                details: Some(description),
            },
        )
        .await;
        return;
    }
    let Some(code) = first_pair(&pairs, "code") else {
        let _ = sender.send(Err(
            "OAuth callback did not include an authorization code".to_string()
        ));
        reply(
            stream,
            shared,
            400,
            &OAuthCallbackPage::Error {
                message: "Missing authorization code".to_string(),
                details: None,
            },
        )
        .await;
        return;
    };
    let iss = first_pair(&pairs, "iss");
    let _ = sender.send(Ok(OAuthCallback {
        code,
        state: state.unwrap_or_default(),
        iss,
    }));
    reply(stream, shared, 200, &OAuthCallbackPage::Ok).await;
}

/// Upstream `reply(response, status, page)`: HTML rendering when configured,
/// the fixed plain-text form otherwise. Connection closes after the body.
async fn reply(
    stream: &mut tokio::net::TcpStream,
    shared: &Arc<CallbackServerShared>,
    status: u16,
    page: &OAuthCallbackPage,
) {
    let (content_type, cache_control, body) = match &shared.render_page {
        Some(render_page) => ("text/html; charset=utf-8", true, render_page(page)),
        None => ("text/plain; charset=utf-8", false, plain_text(page)),
    };
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "OK",
    };
    let mut head = format!("HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\n");
    if cache_control {
        head.push_str("Cache-Control: no-store\r\n");
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    ));
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(body.as_bytes()).await;
    let _ = stream.flush().await;
}
