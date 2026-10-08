//! The loopback OAuth redirect handler shared by the browser sign-in flows,
//! ported from upstream `packages/ai/src/auth/oauth/callback-server.ts` (the
//! auth delta): [`start_oauth_callback_server`] opens the listener and
//! [`wait_for_callback_or_manual_input`] races the browser callback against
//! the manual-paste prompt. The anthropic, openai-codex, openrouter and
//! radius flows all run on this server after the upstream refactor folded
//! their private `http.createServer` copies into it.
//!
//! Interactive surface (M2d ruling): the server is flow-owned infrastructure,
//! like upstream's `node:http` server — it never touches stdio or a browser.
//!
//! Port notes (disclosed divergences):
//! - The listener is a tokio TCP listener; one task per connection answers
//!   exactly one HTTP/1.1 request and closes the socket (upstream keep-alive
//!   spare connections have no Rust counterpart, so upstream's
//!   `closeAllConnections()` teardown is unnecessary here).
//! - `writeHead(status, {"content-type", "cache-control"})` becomes a fixed
//!   head with `Content-Type: text/html; charset=utf-8` and
//!   `Cache-Control: no-store` plus `Content-Length`/`Connection: close`.
//!   Full response-byte equality with Node is impossible anyway (Node emits
//!   `Date` and chunked framing); the pinned contract is status, content
//!   type, cache control and the byte-exact HTML body.
//! - Upstream `server.on("error", finish)` (listener-level failures after
//!   bind) has no analog: tokio surfaces per-connection `accept` errors,
//!   which are ignored like transient browser failures upstream.
//! - A request line without a parseable target never reaches upstream's
//!   handler (Node answers 400 first); the port answers the 404 page, like
//!   the pre-refactor callback ports.
//! - Cancellation maps to [`AuthError::Cancelled`] everywhere upstream throws
//!   `Error("Login cancelled")` (port contract: interaction-signal aborts are
//!   never wrapped). The 502 failure page renders a cancelled exchange as the
//!   literal detail "Login cancelled", mirroring `failure.message`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use crate::ai::auth::types::{
    AuthError, AuthInteraction, AuthPrompt, AuthPromptKind, ProviderAuthInteraction,
};

use super::oauth_page::{oauth_error_html, oauth_success_html};
use super::{
    first_pair, parse_urlencoded_pairs, read_request_head, request_target, Waiter,
    HTML_CONTENT_TYPE,
};

/// Upstream `complete: (code) => Promise<T>`: finishes the sign-in with the
/// received code before the browser page is sent, so the page can show
/// exchange failures.
pub(crate) type CompleteFn<T> =
    Arc<dyn Fn(String) -> BoxFuture<'static, Result<T, AuthError>> + Send + Sync>;

/// Upstream `OAuthCallbackServerOptions<T>` (callback-server.ts:11-30).
pub(crate) struct CallbackServerOptions<T> {
    /// Provider name used on the browser page, for example `OpenAI`.
    pub provider_name: String,
    /// Address to listen on.
    pub host: String,
    /// Port to listen on; `0` picks a free port.
    pub port: u16,
    pub path: String,
    /// Host in `redirectUri` when it differs from `host`, for example
    /// `localhost`.
    pub redirect_host: Option<String>,
    /// Expected `state` parameter. `None` when the provider does not send
    /// one.
    pub state: Option<String>,
    /// Finishes the sign-in with the received code before the browser page is
    /// sent.
    pub complete: CompleteFn<T>,
    /// Upstream `signal?: AbortSignal` (the interaction signal).
    pub signal: CancellationToken,
    /// Upstream `timeoutMs?: number`.
    pub timeout_ms: Option<u64>,
}

/// How the wait settles (upstream `finish` argument shapes): a delivered
/// value, the hand-over to manual entry (`cancel()`), or an error.
type Settle<T> = Result<Option<T>, AuthError>;

/// State shared by the accept loop, the handler tasks, the timeout and the
/// abort watch (upstream `startOAuthCallbackServer`'s closure variables).
struct Shared<T> {
    options_path: String,
    expected_state: Option<String>,
    provider_name: String,
    complete: CompleteFn<T>,
    /// Upstream `claimed`: a callback has been accepted and its code is being
    /// completed (further callbacks get 409; `cancel()` no-ops).
    claimed: AtomicBool,
    /// Upstream `settled`: `finish` ran; later settles are no-ops.
    settled: AtomicBool,
    /// Stops the timer and abort-watch tasks when `finish` runs (upstream
    /// `clearTimeout` / `removeEventListener`), distinct from the listener
    /// shutdown below.
    finished: CancellationToken,
    waiter: Waiter<Settle<T>>,
    /// Stops the accept loop (upstream `server.close()`), only reached
    /// through [`OAuthCallbackServer::close`].
    shutdown: CancellationToken,
    /// The accept-loop task, joined by `close()` (whichever handle closes
    /// first).
    accept_loop: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl<T: Clone + Send + Sync + 'static> Shared<T> {
    /// Upstream `finish`: first settle wins; the timer and abort-watch tasks
    /// stop with it. The listener keeps accepting until `close()` so late
    /// browser requests get the 409 page.
    fn finish(&self, settle: Settle<T>) {
        if self.settled.swap(true, Ordering::SeqCst) {
            return;
        }
        self.finished.cancel();
        self.waiter.settle(Some(settle));
    }
}

/// Upstream `OAuthCallbackServer<T>` (callback-server.ts:32-46). Cloning is
/// a new handle on the same listener (upstream hands the object to callers
/// that race `wait()` against other futures); `close()` on any handle tears
/// the listener down once.
pub(crate) struct OAuthCallbackServer<T> {
    shared: Arc<Shared<T>>,
    redirect_uri: String,
}

impl<T> std::fmt::Debug for OAuthCallbackServer<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthCallbackServer")
            .field("redirect_uri", &self.redirect_uri)
            .finish()
    }
}

impl<T: Clone + Send + Sync + 'static> Clone for OAuthCallbackServer<T> {
    fn clone(&self) -> Self {
        OAuthCallbackServer {
            shared: Arc::clone(&self.shared),
            redirect_uri: self.redirect_uri.clone(),
        }
    }
}

impl<T: Clone + Send + Sync + 'static> OAuthCallbackServer<T> {
    /// Upstream `redirectUri`:
    /// `http://${redirectHost}:${address.port}${path}` with IPv6 hosts
    /// bracketed.
    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    /// Upstream `wait()`: resolves with the result of `complete`, or `None`
    /// after `cancel()`. Errors when the provider redirects with an error,
    /// `complete` fails, the signal aborts, or the timeout elapses.
    pub async fn wait(&self) -> Settle<T> {
        self.shared.waiter.wait().await.unwrap_or(Ok(None))
    }

    /// Upstream `cancel()`: stop waiting for the browser unless a callback is
    /// already being completed.
    pub fn cancel(&self) {
        if !self.shared.claimed.load(Ordering::SeqCst) {
            self.shared.finish(Ok(None));
        }
    }

    /// Upstream `close()`: settle the wait (unless already settled) and stop
    /// accepting.
    pub async fn close(self) {
        self.shared.finish(Err(AuthError::Operation(
            "OAuth callback server closed".to_string(),
        )));
        self.shared.shutdown.cancel();
        let accept_loop = self.shared.accept_loop.lock().unwrap().take();
        if let Some(accept_loop) = accept_loop {
            let _ = accept_loop.await;
        }
    }
}

/// Resource safety upstream does not need (its event loop reclaims closed
/// servers): dropping the last handle stops the accept loop and timer tasks
/// so the listener port is not pinned by an unreachable task. Waiting
/// callers see the wait settle only through an explicit `close()`, never the
/// drop.
impl<T> Drop for OAuthCallbackServer<T> {
    fn drop(&mut self) {
        self.shared.finished.cancel();
        self.shared.shutdown.cancel();
    }
}

/// Upstream `sendPage` plus the fixed success/error status choices: one HTML
/// response with the content type and no-store cache header.
async fn send_page(stream: &mut tokio::net::TcpStream, status: u16, reason: &str, body: &str) {
    use tokio::io::AsyncWriteExt as _;
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {HTML_CONTENT_TYPE}\r\nCache-Control: \
         no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(body.as_bytes()).await;
    let _ = stream.flush().await;
    let _ = stream.shutdown().await;
}

/// The rendered failure detail of a failed `complete` (upstream
/// `const failure = error instanceof Error ? error : new Error(String(error))`):
/// the error's own message, where a cancelled exchange reads as the literal
/// "Login cancelled" it throws upstream.
fn failure_detail(error: &AuthError) -> String {
    match error {
        AuthError::Cancelled => "Login cancelled".to_string(),
        AuthError::Operation(message) => message.clone(),
        other => other.to_string(),
    }
}

/// One handled browser request (upstream request handler,
/// callback-server.ts:84-131). The route order is load-bearing: method/path,
/// then state, then claimed/settled, then the provider error, then the code;
/// the exchange runs before the landing page is written so the browser sees
/// the flow's outcome.
async fn handle_connection<T: Clone + Send + Sync + 'static>(
    mut stream: tokio::net::TcpStream,
    shared: Arc<Shared<T>>,
) {
    let Some(request_line) = read_request_head(&mut stream).await else {
        // No readable request head: nothing to answer (upstream: an abandoned
        // browser request never completes either).
        return;
    };
    let method = request_line.split_whitespace().next().unwrap_or("");
    let target = request_target(&request_line);
    let (path, params) = match target {
        Some(target) => match target.split_once('?') {
            Some((path, query)) => (path, parse_urlencoded_pairs(query)),
            None => (target, Vec::new()),
        },
        // Node never reaches the handler without a target; the port answers
        // the route-not-found page (module port notes).
        None => ("/", Vec::new()),
    };

    // `request.method !== "GET" || url.pathname !== options.path`.
    if method != "GET" || path != shared.options_path {
        send_page(
            &mut stream,
            404,
            "Not Found",
            &oauth_error_html("Callback route not found.", None),
        )
        .await;
        return;
    }
    // `options.state !== undefined && url.searchParams.get("state") !==
    // options.state` — an absent or empty param never equals the expected
    // state.
    if let Some(expected_state) = &shared.expected_state {
        if first_pair(&params, "state").as_deref() != Some(expected_state.as_str()) {
            send_page(
                &mut stream,
                400,
                "Bad Request",
                &oauth_error_html("State mismatch.", None),
            )
            .await;
            return;
        }
    }
    // `claimed || settled` — every later browser request after the first hit.
    if shared.claimed.load(Ordering::SeqCst) || shared.settled.load(Ordering::SeqCst) {
        send_page(
            &mut stream,
            409,
            "Conflict",
            &oauth_error_html("This sign-in has already been handled.", None),
        )
        .await;
        return;
    }
    // `const error = url.searchParams.get("error"); if (error)` — truthiness:
    // an empty error param is not an error.
    if let Some(error) = first_pair(&params, "error").filter(|value| !value.is_empty()) {
        // `url.searchParams.get("error_description") ?? error` — no truthiness
        // check, an empty description stays empty.
        let description = first_pair(&params, "error_description").unwrap_or(error);
        send_page(
            &mut stream,
            400,
            "Bad Request",
            &oauth_error_html(
                &format!("{} authorization failed.", shared.provider_name),
                Some(&description),
            ),
        )
        .await;
        shared.finish(Err(AuthError::Operation(format!(
            "{} authorization failed: {description}",
            shared.provider_name
        ))));
        return;
    }
    // `if (!code)` truthiness: a missing code keeps the login waiting.
    let Some(code) = first_pair(&params, "code").filter(|value| !value.is_empty()) else {
        send_page(
            &mut stream,
            400,
            "Bad Request",
            &oauth_error_html("Missing authorization code.", None),
        )
        .await;
        return;
    };
    shared.claimed.store(true, Ordering::SeqCst);

    match (shared.complete)(code).await {
        Ok(value) => {
            send_page(
                &mut stream,
                200,
                "OK",
                &oauth_success_html(&format!(
                    "Signed in to {}. You may now close this page.",
                    shared.provider_name
                )),
            )
            .await;
            shared.finish(Ok(Some(value)));
        }
        Err(failure) => {
            send_page(
                &mut stream,
                502,
                "Bad Gateway",
                &oauth_error_html(
                    &format!("{} sign-in failed.", shared.provider_name),
                    Some(&failure_detail(&failure)),
                ),
            )
            .await;
            shared.finish(Err(failure));
        }
    }
}

/// Upstream `startOAuthCallbackServer<T>` (callback-server.ts:66-227).
pub(crate) async fn start_oauth_callback_server<T: Clone + Send + Sync + 'static>(
    options: CallbackServerOptions<T>,
) -> Result<OAuthCallbackServer<T>, AuthError> {
    // `if (signal?.aborted) throw new Error("Login cancelled")`.
    if options.signal.is_cancelled() {
        return Err(AuthError::Cancelled);
    }

    // `server.listen(options.port, options.host)`; the promise rejects on a
    // bind error. An already-bound port is surfaced as
    // [`AuthError::AddressInUse`] so the login flows can give the v1.0.0
    // targeted failure (upstream checks `error.code === "EADDRINUSE"`).
    let listener = tokio::net::TcpListener::bind((options.host.as_str(), options.port))
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AddrInUse {
                AuthError::AddressInUse(format!(
                    "Failed to start the OAuth callback server on {}:{}: {error}",
                    options.host, options.port
                ))
            } else {
                AuthError::Operation(format!(
                    "Failed to start the OAuth callback server on {}:{}: {error}",
                    options.host, options.port
                ))
            }
        })?;
    let address = listener.local_addr().map_err(|_| {
        // Upstream `throw new Error("OAuth callback server did not bind to TCP")`
        // when `server.address()` is not a TCP address.
        AuthError::Operation("OAuth callback server did not bind to TCP".to_string())
    })?;
    let port = address.port();

    let shared = Arc::new(Shared {
        options_path: options.path.clone(),
        expected_state: options.state.clone(),
        provider_name: options.provider_name.clone(),
        complete: options.complete.clone(),
        claimed: AtomicBool::new(false),
        settled: AtomicBool::new(false),
        finished: CancellationToken::new(),
        waiter: Waiter::new(),
        shutdown: CancellationToken::new(),
        accept_loop: std::sync::Mutex::new(None),
    });

    let loop_shared = Arc::clone(&shared);
    let loop_shutdown = shared.shutdown.clone();
    let accept_loop = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = loop_shutdown.cancelled() => break,
                // Transient accept errors must not kill the capture; upstream's
                // server keeps listening too.
                accepted = listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        let shared = Arc::clone(&loop_shared);
                        tokio::spawn(handle_connection(stream, shared));
                    }
                    Err(_) => continue,
                },
            }
        }
    });

    // Upstream `signal?.addEventListener("abort", onAbort, { once: true })`
    // with `onAbort = () => finish({ error: new Error("Login cancelled") })`.
    {
        let shared = Arc::clone(&shared);
        let finished = shared.finished.clone();
        let signal = options.signal.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = finished.cancelled() => {}
                _ = signal.cancelled() => shared.finish(Err(AuthError::Cancelled)),
            }
        });
    }

    // Upstream `if (options.timeoutMs !== undefined) timer = setTimeout(...)`
    // with `finish({ error: new Error(`${providerName} sign-in timed out`) })`.
    if let Some(timeout_ms) = options.timeout_ms {
        let shared = Arc::clone(&shared);
        let finished = shared.finished.clone();
        let provider_name = options.provider_name.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = finished.cancelled() => {}
                _ = tokio::time::sleep(Duration::from_millis(timeout_ms)) => {
                    shared.finish(Err(AuthError::Operation(format!(
                        "{provider_name} sign-in timed out"
                    ))));
                }
            }
        });
    }

    // Upstream `const redirectHost = options.redirectHost ?? options.host` and
    // the IPv6 bracketing in the redirectUri template.
    let redirect_host = options.redirect_host.unwrap_or(options.host);
    let host_display = if redirect_host.contains(':') {
        format!("[{redirect_host}]")
    } else {
        redirect_host
    };
    *shared.accept_loop.lock().unwrap() = Some(accept_loop);
    Ok(OAuthCallbackServer {
        shared,
        redirect_uri: format!("http://{host_display}:{port}{}", options.path),
    })
}

/// The outcome of [`wait_for_callback_or_manual_input`] (upstream's
/// `{ type: "callback", value } | { type: "manual", input }` union).
#[derive(Debug)]
pub(crate) enum CallbackOrManual<T> {
    /// The browser callback completed.
    Callback(T),
    /// The user pasted the code or redirect URL.
    Manual(String),
}

/// Upstream `waitForCallbackOrManualInput` (callback-server.ts:233-261): wait
/// for the browser callback, or for the user to paste the code or redirect
/// URL when the browser cannot reach the loopback server (for example over
/// SSH). Without a callback server only the manual prompt is used — including
/// upstream's wait shape: the prompt carries only the internal abort
/// controller, so an aborted interaction signal surfaces through the server
/// wait (present) or not at all (absent), never through the prompt.
pub(crate) async fn wait_for_callback_or_manual_input<T: Clone + Send + Sync + 'static>(
    interaction: &ProviderAuthInteraction,
    callback: Option<&OAuthCallbackServer<T>>,
    prompt_message: &str,
    prompt_placeholder: &str,
) -> Result<CallbackOrManual<T>, AuthError> {
    // Upstream's `manualAbort` controller: aborts the pending prompt in the
    // finally block so UIs can dismiss it once login settles.
    let manual_abort = CancellationToken::new();
    let prompt = interaction.prompt(AuthPrompt {
        signal: Some(manual_abort.clone()),
        kind: AuthPromptKind::ManualCode {
            message: prompt_message.to_string(),
            placeholder: Some(prompt_placeholder.to_string()),
        },
    });
    tokio::pin!(prompt);

    // The `.then/.catch` tail of the manual prompt records its outcome and
    // cancels the server wait in both branches.
    let mut manual: Option<Result<String, AuthError>> = None;
    // `const value = await callback?.wait()`: with no server this resolves
    // `undefined` immediately; with one, it is raced against the prompt. The
    // `biased` order makes the port deterministic: cancellation, then the
    // prompt, then the settled wait. The guard disables the prompt arm once
    // it has settled so the loop can keep polling the wait.
    let settled: Settle<T> = match callback {
        None => Ok(None),
        Some(callback) => loop {
            tokio::select! {
                biased;
                _ = interaction.signal.cancelled() => break Err(AuthError::Cancelled),
                outcome = &mut prompt, if manual.is_none() => {
                    manual = Some(outcome);
                    callback.cancel();
                }
                settled = callback.wait() => break settled,
            }
        },
    };

    let result = match settled {
        // A rejected wait propagates as-is; upstream never consults the
        // manual error on this path.
        Err(error) => Err(error),
        Ok(Some(value)) => {
            // `if (manualError) throw manualError` after a resolved wait.
            if let Some(Err(error)) = &manual {
                return Err(error.clone());
            }
            Ok(CallbackOrManual::Callback(value))
        }
        Ok(None) => {
            // `const input = await manual` — the prompt when it already
            // settled (the only `cancel()` source), otherwise awaited here
            // (the no-server path); its rejection is the upstream
            // `manualError` throw.
            let input = match manual {
                Some(outcome) => outcome?,
                None => (&mut prompt).await?,
            };
            Ok(CallbackOrManual::Manual(input))
        }
    };

    // Upstream `finally { manualAbort.abort() }`.
    manual_abort.cancel();
    result
}
#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use futures::future::BoxFuture;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_util::sync::CancellationToken;

    use super::*;

    type Responder<T> =
        Box<dyn Fn(String) -> BoxFuture<'static, Result<T, AuthError>> + Send + Sync>;

    fn identity_complete<T>() -> CompleteFn<T>
    where
        T: Clone + Send + Sync + 'static + From<String>,
    {
        Arc::new(|code| Box::pin(async move { Ok(T::from(code)) }))
    }

    fn string_complete() -> CompleteFn<String> {
        Arc::new(|code| Box::pin(async move { Ok(format!("completed:{code}")) }))
    }

    fn failing_complete(message: &'static str) -> CompleteFn<String> {
        Arc::new(move |_| Box::pin(async move { Err(AuthError::Operation(message.to_string())) }))
    }

    fn options<T>(
        complete: CompleteFn<T>,
        overrides: impl FnOnce(&mut CallbackServerOptions<T>),
    ) -> CallbackServerOptions<T> {
        let mut options = CallbackServerOptions {
            provider_name: "Example".to_string(),
            host: "127.0.0.1".to_string(),
            port: 0,
            path: "/callback".to_string(),
            redirect_host: None,
            state: Some("expected-state".to_string()),
            complete,
            signal: CancellationToken::new(),
            timeout_ms: None,
        };
        overrides(&mut options);
        options
    }

    async fn start<T: Clone + Send + Sync + 'static>(
        complete: CompleteFn<T>,
        overrides: impl FnOnce(&mut CallbackServerOptions<T>),
    ) -> OAuthCallbackServer<T> {
        start_oauth_callback_server(options(complete, overrides))
            .await
            .unwrap()
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn callback_url(redirect_uri: &str, params: &[(&str, &str)]) -> String {
        let mut url = url::Url::parse(redirect_uri).unwrap();
        for (name, value) in params {
            url.query_pairs_mut().append_pair(name, value);
        }
        url.to_string()
    }

    /// One raw GET, returning the full response bytes.
    async fn http_get(port: u16, target: &str) -> String {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        stream
            .write_all(
                format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        String::from_utf8_lossy(&response).into_owned()
    }

    /// One raw request with an arbitrary method.
    async fn http_request(port: u16, request: &str) -> String {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        stream
            .write_all(format!("{request}\r\nConnection: close\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        String::from_utf8_lossy(&response).into_owned()
    }

    /// Oracle port (packages/ai/test/oauth-callback-server.test.ts "ignores
    /// stray requests and resolves with the completed code").
    #[tokio::test]
    async fn ignores_stray_requests_and_resolves_with_the_completed_code() {
        let server = start(string_complete(), |_| {}).await;
        assert!(server.redirect_uri().starts_with("http://127.0.0.1:"));
        assert!(server.redirect_uri().ends_with("/callback"));

        let redirect = server.redirect_uri().to_string();
        let wrong_path = http_get(
            url::Url::parse(&redirect).unwrap().port().unwrap(),
            "/other",
        )
        .await;
        assert!(wrong_path.starts_with("HTTP/1.1 404 Not Found\r\n"));

        let wrong_state = http_get(
            url::Url::parse(&redirect).unwrap().port().unwrap(),
            "/callback?code=c&state=other",
        )
        .await;
        assert!(wrong_state.starts_with("HTTP/1.1 400 Bad Request\r\n"));
        assert!(wrong_state.contains("Content-Type: text/html; charset=utf-8"));
        assert!(wrong_state.contains("State mismatch."));

        let post = http_request(
            url::Url::parse(&redirect).unwrap().port().unwrap(),
            "POST /callback?code=c&state=expected-state HTTP/1.1\r\nHost: 127.0.0.1",
        )
        .await;
        assert!(post.starts_with("HTTP/1.1 404 Not Found\r\n"));

        let missing_code = http_get(
            url::Url::parse(&redirect).unwrap().port().unwrap(),
            "/callback?state=expected-state",
        )
        .await;
        assert!(missing_code.starts_with("HTTP/1.1 400 Bad Request\r\n"));

        let success = http_get(
            url::Url::parse(&redirect).unwrap().port().unwrap(),
            "/callback?code=the-code&state=expected-state",
        )
        .await;
        assert!(success.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(success.contains("Content-Type: text/html; charset=utf-8"));
        assert!(success.contains("Cache-Control: no-store"));
        assert!(success.contains("Authentication successful"));
        assert!(success.contains("Signed in to Example."));

        let settled = tokio::time::timeout(Duration::from_secs(5), server.wait())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(settled, Some("completed:the-code".to_string()));
        server.close().await;
    }

    /// Oracle port: "uses the redirect host and skips the state check when
    /// none is expected".
    #[tokio::test]
    async fn uses_the_redirect_host_and_skips_the_state_check_when_none_is_expected() {
        let server = start(string_complete(), |options| {
            options.redirect_host = Some("localhost".to_string());
            options.state = None;
        })
        .await;
        assert!(server.redirect_uri().starts_with("http://localhost:"));
        let port = url::Url::parse(server.redirect_uri())
            .unwrap()
            .port()
            .unwrap();
        let response = http_get(port, "/callback?code=no-state").await;
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        let settled = tokio::time::timeout(Duration::from_secs(5), server.wait())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(settled, Some("completed:no-state".to_string()));
        server.close().await;
    }

    /// Oracle port: "shows completion failures on the page and rejects the
    /// wait".
    #[tokio::test]
    async fn shows_completion_failures_on_the_page_and_rejects_the_wait() {
        let server = start(failing_complete("token exchange failed"), |_| {}).await;
        let port = url::Url::parse(server.redirect_uri())
            .unwrap()
            .port()
            .unwrap();
        let failure = http_get(port, "/callback?code=c&state=expected-state").await;
        assert!(failure.starts_with("HTTP/1.1 502 Bad Gateway\r\n"));
        assert!(failure.contains("Example sign-in failed."));
        assert!(failure.contains("token exchange failed"));
        let error = tokio::time::timeout(Duration::from_secs(5), server.wait())
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("token exchange failed".to_string())
        );
        server.close().await;
    }

    /// Oracle port: "rejects the wait when the provider redirects with an
    /// error".
    #[tokio::test]
    async fn rejects_the_wait_when_the_provider_redirects_with_an_error() {
        let server = start(string_complete(), |_| {}).await;
        let port = url::Url::parse(server.redirect_uri())
            .unwrap()
            .port()
            .unwrap();
        let failure = http_get(
            port,
            "/callback?error=access_denied&error_description=User%20denied%20access&state=expected-state",
        )
        .await;
        assert!(failure.starts_with("HTTP/1.1 400 Bad Request\r\n"));
        assert!(failure.contains("Example authorization failed."));
        assert!(failure.contains("User denied access"));
        let error = tokio::time::timeout(Duration::from_secs(5), server.wait())
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("Example authorization failed: User denied access".to_string())
        );
        server.close().await;
    }

    /// Oracle port: "completes only the first callback" — a claimed callback
    /// keeps completing even when the caller switches to manual input.
    #[tokio::test]
    async fn completes_only_the_first_callback() {
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let release_rx = Arc::new(tokio::sync::Mutex::new(Some(release_rx)));
        let complete: CompleteFn<String> = Arc::new(move |code| {
            let release_rx = Arc::clone(&release_rx);
            Box::pin(async move {
                let mut guard = release_rx.lock().await;
                let release_rx = guard.take().expect("complete runs once");
                drop(guard);
                let _ = release_rx.await;
                Ok(format!("done:{code}"))
            })
        });
        let server = start(complete, |_| {}).await;
        let port = url::Url::parse(server.redirect_uri())
            .unwrap()
            .port()
            .unwrap();

        let first = tokio::spawn(http_get(port, "/callback?code=c&state=expected-state"));
        // Let the first callback claim the exchange.
        tokio::time::sleep(Duration::from_millis(150)).await;
        let second = http_get(port, "/callback?code=c&state=expected-state").await;
        assert!(second.starts_with("HTTP/1.1 409 Conflict\r\n"));
        assert!(second.contains("This sign-in has already been handled."));

        // A claimed callback keeps completing even after cancel().
        server.cancel();
        let _ = release_tx.send(());
        assert!(first.await.unwrap().starts_with("HTTP/1.1 200 OK\r\n"));
        let settled = tokio::time::timeout(Duration::from_secs(5), server.wait())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(settled, Some("done:c".to_string()));
        server.close().await;
    }

    /// Oracle port: "resolves with undefined after cancel" — and a late
    /// browser request gets the 409 page.
    #[tokio::test]
    async fn resolves_with_none_after_cancel() {
        let server = start(string_complete(), |_| {}).await;
        server.cancel();
        let settled = tokio::time::timeout(Duration::from_secs(5), server.wait())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(settled, None);
        let port = url::Url::parse(server.redirect_uri())
            .unwrap()
            .port()
            .unwrap();
        let late = http_get(port, "/callback?code=c&state=expected-state").await;
        assert!(late.starts_with("HTTP/1.1 409 Conflict\r\n"));
        server.close().await;
    }

    /// Oracle port: "rejects the wait on abort and on timeout".
    #[tokio::test]
    async fn rejects_the_wait_on_abort_and_on_timeout() {
        let signal = CancellationToken::new();
        let aborted = start(string_complete(), |options| {
            options.signal = signal.clone();
        })
        .await;
        signal.cancel();
        let error = tokio::time::timeout(Duration::from_secs(5), aborted.wait())
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(error, AuthError::Cancelled);
        aborted.close().await;

        let timed_out = start(string_complete(), |options| {
            options.timeout_ms = Some(10);
        })
        .await;
        let error = tokio::time::timeout(Duration::from_secs(5), timed_out.wait())
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("Example sign-in timed out".to_string())
        );
        timed_out.close().await;

        let already = CancellationToken::new();
        already.cancel();
        let error = start_oauth_callback_server::<String>(options(string_complete(), |options| {
            options.signal = already.clone()
        }))
        .await
        .unwrap_err();
        assert_eq!(error, AuthError::Cancelled);
    }

    /// Oracle port: "fails instead of picking another port when the
    /// requested port is taken".
    #[tokio::test]
    async fn fails_instead_of_picking_another_port_when_the_requested_port_is_taken() {
        let blocker = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = blocker.local_addr().unwrap().port();
        let error = start_oauth_callback_server::<String>(options(string_complete(), |options| {
            options.port = port
        }))
        .await
        .unwrap_err();
        // A taken port is the v1.0.0 AddressInUse variant; other bind
        // failures stay generic Operations.
        assert!(matches!(error, AuthError::AddressInUse(_)), "{error:?}");
        assert!(error.to_string().contains(&port.to_string()));
        drop(blocker);
    }

    // ---- waitForCallbackOrManualInput ----

    struct RecordingInteraction {
        events: Mutex<Vec<crate::ai::auth::types::AuthEvent>>,
        signal: CancellationToken,
        respond: Box<
            dyn Fn(
                    crate::ai::auth::types::AuthPrompt,
                ) -> BoxFuture<'static, Result<String, AuthError>>
                + Send
                + Sync,
        >,
        prompts: Mutex<Vec<crate::ai::auth::types::AuthPrompt>>,
    }

    impl crate::ai::auth::types::AuthInteraction for RecordingInteraction {
        fn signal(&self) -> Option<CancellationToken> {
            Some(self.signal.clone())
        }

        fn prompt(
            &self,
            prompt: crate::ai::auth::types::AuthPrompt,
        ) -> BoxFuture<'_, Result<String, AuthError>> {
            self.prompts.lock().unwrap().push(prompt.clone());
            (self.respond)(prompt)
        }

        fn notify(&self, event: crate::ai::auth::types::AuthEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    /// A prompt that stays open until its signal aborts, recording it.
    fn pending_prompt_interaction() -> (Arc<RecordingInteraction>, ProviderAuthInteraction) {
        let manual_signal: Arc<Mutex<Option<CancellationToken>>> = Arc::new(Mutex::new(None));
        let slot = Arc::clone(&manual_signal);
        let fake = Arc::new(RecordingInteraction {
            events: Mutex::new(Vec::new()),
            signal: CancellationToken::new(),
            prompts: Mutex::new(Vec::new()),
            respond: Box::new(move |prompt| {
                let slot = Arc::clone(&slot);
                Box::pin(async move {
                    *slot.lock().unwrap() = prompt.signal.clone();
                    prompt.signal.unwrap_or_default().cancelled().await;
                    Err(AuthError::Cancelled)
                })
            }),
        });
        let interaction = ProviderAuthInteraction::new(
            Arc::clone(&fake) as Arc<dyn crate::ai::auth::types::AuthInteraction>,
            fake.signal.clone(),
        );
        (fake, interaction)
    }

    /// Oracle port: "returns the browser callback and aborts the manual
    /// prompt".
    #[tokio::test]
    async fn returns_the_browser_callback_and_aborts_the_manual_prompt() {
        let server: OAuthCallbackServer<String> = start(string_complete(), |_| {}).await;
        let (fake, interaction) = pending_prompt_interaction();

        let redirect = server.redirect_uri().to_string();
        let port = url::Url::parse(&redirect).unwrap().port().unwrap();
        let handle = server.clone();
        let result =
            wait_for_callback_or_manual_input(&interaction, Some(&handle), "paste", &redirect);
        tokio::pin!(result);
        // Fire the browser callback once the prompt has started.
        let driver = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            http_get(port, "/callback?code=from-browser&state=expected-state").await
        });
        let outcome = tokio::time::timeout(Duration::from_secs(5), result)
            .await
            .unwrap()
            .unwrap();
        driver.await.unwrap();

        match outcome {
            CallbackOrManual::Callback(value) => assert_eq!(value, "completed:from-browser"),
            other => panic!("expected the callback path, got {other:?}"),
        }
        // The manual prompt was aborted in the finally block.
        assert!(fake.prompts.lock().unwrap()[0]
            .signal
            .as_ref()
            .unwrap()
            .is_cancelled());
        server.close().await;
    }

    /// Oracle port: "returns pasted input and stops waiting for the browser".
    #[tokio::test]
    async fn returns_pasted_input_and_stops_waiting_for_the_browser() {
        let server: OAuthCallbackServer<String> = start(string_complete(), |_| {}).await;
        let redirect = server.redirect_uri().to_string();
        let fake = Arc::new(RecordingInteraction {
            events: Mutex::new(Vec::new()),
            signal: CancellationToken::new(),
            prompts: Mutex::new(Vec::new()),
            respond: Box::new(|_prompt| Box::pin(async { Ok("pasted".to_string()) })),
        });
        let interaction = ProviderAuthInteraction::new(
            Arc::clone(&fake) as Arc<dyn crate::ai::auth::types::AuthInteraction>,
            fake.signal.clone(),
        );
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            wait_for_callback_or_manual_input(&interaction, Some(&server), "paste", &redirect),
        )
        .await
        .unwrap()
        .unwrap();
        match outcome {
            CallbackOrManual::Manual(input) => assert_eq!(input, "pasted"),
            other => panic!("expected the manual path, got {other:?}"),
        }
        server.close().await;
    }

    /// Oracle port: "uses only the manual prompt without a callback server".
    #[tokio::test]
    async fn uses_only_the_manual_prompt_without_a_callback_server() {
        let fake = Arc::new(RecordingInteraction {
            events: Mutex::new(Vec::new()),
            signal: CancellationToken::new(),
            prompts: Mutex::new(Vec::new()),
            respond: Box::new(|_prompt| Box::pin(async { Ok("pasted".to_string()) })),
        });
        let interaction = ProviderAuthInteraction::new(
            Arc::clone(&fake) as Arc<dyn crate::ai::auth::types::AuthInteraction>,
            fake.signal.clone(),
        );
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            wait_for_callback_or_manual_input::<String>(
                &interaction,
                None,
                "paste",
                "http://localhost/callback",
            ),
        )
        .await
        .unwrap()
        .unwrap();
        match outcome {
            CallbackOrManual::Manual(input) => assert_eq!(input, "pasted"),
            other => panic!("expected the manual path, got {other:?}"),
        }
    }

    /// Oracle port: "propagates manual prompt failures".
    #[tokio::test]
    async fn propagates_manual_prompt_failures() {
        let server: OAuthCallbackServer<String> = start(string_complete(), |_| {}).await;
        let redirect = server.redirect_uri().to_string();
        let fake = Arc::new(RecordingInteraction {
            events: Mutex::new(Vec::new()),
            signal: CancellationToken::new(),
            prompts: Mutex::new(Vec::new()),
            respond: Box::new(|_prompt| {
                Box::pin(async { Err(AuthError::Operation("prompt cancelled".to_string())) })
            }),
        });
        let interaction = ProviderAuthInteraction::new(
            Arc::clone(&fake) as Arc<dyn crate::ai::auth::types::AuthInteraction>,
            fake.signal.clone(),
        );
        let error = tokio::time::timeout(
            Duration::from_secs(5),
            wait_for_callback_or_manual_input(&interaction, Some(&server), "paste", &redirect),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert_eq!(error, AuthError::Operation("prompt cancelled".to_string()));
        server.close().await;
    }

    /// A delivered callback still loses to an already-settled manual error
    /// (upstream checks `manualError` after the resolved wait).
    #[tokio::test]
    async fn manual_error_wins_over_a_delivered_callback() {
        let server: OAuthCallbackServer<String> = start(string_complete(), |_| {}).await;
        let redirect = server.redirect_uri().to_string();
        let port = url::Url::parse(&redirect).unwrap().port().unwrap();
        let fake = Arc::new(RecordingInteraction {
            events: Mutex::new(Vec::new()),
            signal: CancellationToken::new(),
            prompts: Mutex::new(Vec::new()),
            respond: Box::new(move |_prompt| {
                let port = port;
                Box::pin(async move {
                    // The browser callback lands while the manual prompt is
                    // failing: upstream's `.catch` records the manual error
                    // and cancels the (unclaimed) wait first, so the late
                    // callback hits the settled guard and the manual error
                    // wins.
                    tokio::spawn(async move {
                        let _ = http_get(port, "/callback?code=browser-code&state=expected-state")
                            .await;
                    });
                    Err(AuthError::Operation("manual blew up".to_string()))
                })
            }),
        });
        let interaction = ProviderAuthInteraction::new(
            Arc::clone(&fake) as Arc<dyn crate::ai::auth::types::AuthInteraction>,
            fake.signal.clone(),
        );
        let error = tokio::time::timeout(
            Duration::from_secs(5),
            wait_for_callback_or_manual_input(
                &interaction,
                Some(&server.clone()),
                "paste",
                &redirect,
            ),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert_eq!(error, AuthError::Operation("manual blew up".to_string()));
        server.close().await;
    }

    /// close() settles an unset wait with the closed error and releases the
    /// port (a dropped server stops its tasks without settling, port notes).
    #[tokio::test]
    async fn close_settles_the_wait_and_releases_the_port() {
        let port = free_port();
        let server = start(string_complete(), |options| {
            options.port = port;
        })
        .await;
        let waiter = tokio::spawn({
            let handle = server.clone();
            async move { handle.wait().await }
        });
        server.close().await;
        let error = tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("OAuth callback server closed".to_string())
        );
        // The port is released.
        assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_ok());
    }

    /// The redirect URI brackets IPv6 hosts.
    #[tokio::test]
    async fn redirect_uri_brackets_ipv6_hosts() {
        // ::1 binds on loopback; the URI must bracket the host.
        let listener = match tokio::net::TcpListener::bind(("::1", 0)).await {
            Ok(listener) => listener,
            Err(_) => return, // no IPv6 loopback on the host
        };
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let server = start(string_complete(), |options| {
            options.host = "::1".to_string();
            options.port = port;
        })
        .await;
        assert!(server.redirect_uri().starts_with("http://[::1]:"));
        server.close().await;
    }

    /// Silence the unused-helper warning when only part of the oracle set
    /// runs (identity_complete is shared with flow modules through super).
    #[allow(dead_code)]
    fn unused() {
        let _ = identity_complete::<String>;
        let _ = callback_url;
        let _: Option<Responder<String>> = None;
    }
}
