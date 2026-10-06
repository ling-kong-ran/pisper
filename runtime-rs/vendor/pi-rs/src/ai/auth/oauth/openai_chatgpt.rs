//! OpenAI Responses API token sharing through Sign in with ChatGPT, ported
//! from upstream `packages/ai/src/auth/oauth/openai-chatgpt.ts` (the auth
//! delta): the dynamic-client authorization flow, the flow-local loopback
//! callback server on `127.0.0.1:1455`, the manual redirect-URL fallback, the
//! token exchange/refresh requests and the [`OpenAIChatGptOAuth`]
//! [`OAuthAuth`] implementation.
//!
//! This public-client flow uses no client secret and sends the resulting user
//! access token directly to api.openai.com. Every login registers a new
//! client with the fixed `dynamic_agent_client` ID; OpenAI returns the issued
//! client ID in the callback, and it is stored on the credential (upstream
//! `clientId`, the port's `extra` map) and reused by refresh.
//!
//! Interactive surface (M2d ruling): the browser gets the authorize URL via
//! [`AuthEvent::AuthUrl`], the user's pasted final redirect URL arrives
//! through a `manual_code` prompt (its signal is the
//! `AbortSignal.any([manualAbort, interaction])` combination), progress is
//! reported as events, and a failed callback-server bind degrades to
//! manual-only login through an [`AuthEvent::Info`] notice. The flow never
//! touches stdio or a browser directly.
//!
//! Port notes (disclosed divergences):
//! - The token endpoint URL and callback host/port are fields on
//!   [`OpenAIChatGptOAuth`] (upstream: module constants) so tests can point
//!   the flow at a wiremock server and free ports. `REDIRECT_URI` is derived
//!   from the callback port and stays `http://127.0.0.1:1455/auth/callback`
//!   in production — the URI is pinned to `127.0.0.1` even when
//!   `PI_OAUTH_CALLBACK_HOST` moves the bind address, like upstream.
//! - The token request sends lowercase `accept`/`content-type` header names
//!   (as upstream's fetch does); the landing pages send only
//!   `Content-Type: text/html; charset=utf-8` (this flow's private
//!   `sendHtml` carries no `cache-control`, unlike the shared callback
//!   server).
//! - JSON re-serialization in failure messages and response-shape checks
//!   uses serde_json's map ordering (alphabetical) instead of the JS
//!   `JSON.stringify` insertion order; raw response bodies in failure
//!   messages are preserved byte-for-byte. A non-JSON token response
//!   surfaces the raw serde error text where upstream surfaces the raw
//!   `SyntaxError`.
//! - A failed callback-server bind surfaces the port's OS error text inside
//!   the `Could not listen on …` info notice, where upstream carries Node's
//!   `listen EADDRINUSE: …` message text.
//! - Upstream `server.on("error", rejectResult)` (listener failures after
//!   bind) has no analog: tokio surfaces per-connection accept errors, which
//!   are ignored like transient browser failures upstream.
//! - Upstream `server.closeAllConnections()` (final-block teardown killing
//!   browser spare connections so a stale callback cannot land on the next
//!   login's state check) has no analog: the port answers exactly one request
//!   per connection and closes it, so no spare connection survives the
//!   server.
//! - Randomness (`randomBytes(32)` for state/nonce, `crypto.getRandomValues`
//!   inside PKCE) draws OS entropy via `rand`; the oracle tests inject the
//!   capture's deterministic byte stream through [`super::test_entropy`].

use std::collections::BTreeMap;

use futures::future::BoxFuture;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::ai::api::azure_openai_responses::get_provider_env_value;
use crate::ai::api::http_client;
use crate::ai::auth::types::{
    AuthError, AuthEvent, AuthInteraction, AuthPrompt, AuthPromptKind, ModelAuth, OAuthAuth,
    OAuthCredential, ProviderAuthInteraction,
};
use crate::ai::now_ms;

use super::oauth_page::{oauth_error_html, oauth_success_html};
use super::pkce::{base64url_encode, generate_pkce, Pkce};
#[cfg(test)]
use super::test_entropy;
use super::{first_pair, read_request_head, request_target, Waiter, HTML_CONTENT_TYPE};

/// Upstream `DYNAMIC_CLIENT_ID`: every login registers a new client with this
/// ID; OpenAI returns the issued client ID in the callback.
const DYNAMIC_CLIENT_ID: &str = "dynamic_agent_client";

/// Upstream `AGENT_NAME_HINT`.
const AGENT_NAME_HINT: &str = "Pi";

/// Upstream `AUTHORIZE_URL`.
const AUTHORIZE_URL: &str = "https://auth.openai.com/api/accounts/authorize";

/// Upstream `TOKEN_URL`; the production default for
/// [`OpenAIChatGptOAuth::new`].
const TOKEN_URL: &str = "https://auth.openai.com/api/accounts/oauth/token";

/// Upstream `RESOURCE`.
const RESOURCE: &str = "https://api.openai.com/v1";

/// Upstream `getProviderEnvValue("PI_OAUTH_CALLBACK_HOST")`.
const CALLBACK_HOST_ENV: &str = "PI_OAUTH_CALLBACK_HOST";

/// Upstream `|| "127.0.0.1"` fallback for the callback bind host.
const DEFAULT_CALLBACK_HOST: &str = "127.0.0.1";

/// Upstream `CALLBACK_PORT`.
const CALLBACK_PORT: u16 = 1455;

/// Upstream `CALLBACK_PATH`.
const CALLBACK_PATH: &str = "/auth/callback";

/// Upstream `DIRECT_TOKEN_SCOPE`: the grant is rejected without it.
const DIRECT_TOKEN_SCOPE: &str = "chatgpt.tokens.use.direct";

/// Upstream `SCOPE`.
const SCOPE: &str = "openid profile email offline_access resource.invoke \
                     chatgpt.tokens.use.direct";

/// Upstream `EXPIRY_MARGIN_MS` (3 minutes): refresh long before the real
/// expiry so a request never starts with a token about to expire.
const EXPIRY_MARGIN_MS: i64 = 3 * 60 * 1000;

/// The `{ code, clientId }` pair the callback delivers (upstream
/// `AuthorizationResult`).
#[derive(Clone, Debug)]
struct AuthorizationResult {
    code: String,
    client_id: String,
}

/// Upstream `UUID_PATTERN` over the device ID
/// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i`).
fn is_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate() {
        match index {
            8 | 13 | 18 | 23 => {
                if *byte != b'-' {
                    return false;
                }
            }
            _ => {
                if !byte.is_ascii_hexdigit() {
                    return false;
                }
            }
        }
    }
    true
}

/// Upstream `randomValue`: 32 random bytes, base64url.
fn random_value() -> String {
    let mut bytes = [0u8; 32];
    #[cfg(test)]
    if let Some(injected) = test_entropy::take(32) {
        bytes.copy_from_slice(&injected);
        return base64url_encode(&bytes);
    }
    rand::fill(&mut bytes);
    base64url_encode(&bytes)
}

/// Upstream `agentHostId`: OpenAI identifies each installation ("agent host")
/// by a stable URI such as `urn:uuid:<uuid>`.
fn agent_host_id(device_id: Option<&str>) -> Result<String, AuthError> {
    match device_id {
        Some(device_id) if is_uuid(device_id) => {
            Ok(format!("urn:uuid:{}", device_id.to_lowercase()))
        }
        _ => Err(AuthError::Operation(
            "Sign in with ChatGPT requires a device ID (UUID) for this installation".to_string(),
        )),
    }
}

/// Upstream `authorizationResultFromCallback` over the parsed query pairs.
fn authorization_result_from_callback(
    params: &[(String, String)],
    expected_state: &str,
) -> Result<AuthorizationResult, AuthError> {
    let get = |name: &str| first_pair(params, name);
    // `if (!code)` truthiness.
    let code = get("code").filter(|code| !code.is_empty());
    let Some(code) = code else {
        return Err(AuthError::Operation(
            "Missing authorization code".to_string(),
        ));
    };
    let state = get("state").filter(|state| !state.is_empty());
    let Some(state) = state else {
        return Err(AuthError::Operation("Missing OAuth state".to_string()));
    };
    if state != expected_state {
        return Err(AuthError::Operation("OAuth state mismatch".to_string()));
    }
    // `url.searchParams.get("client_id")?.trim()` — whitespace-only is empty.
    let client_id = get("client_id")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let Some(client_id) = client_id else {
        return Err(AuthError::Operation(
            "OpenAI OAuth registration callback did not contain an issued client ID".to_string(),
        ));
    };
    Ok(AuthorizationResult { code, client_id })
}

/// JS `url.origin` for the origins this flow compares: `scheme://host:port`
/// with default ports omitted.
fn url_origin(url: &url::Url) -> String {
    let port = url.port_or_known_default();
    match (url.host_str(), port) {
        (Some(host), Some(port)) => format!("{}://{host}:{port}", url.scheme()),
        (Some(host), None) => format!("{}://{host}", url.scheme()),
        _ => format!("{}://", url.scheme()),
    }
}

/// Upstream `authorizationResultFromManualInput`: parse the pasted final
/// redirect URL, require the callback origin/path, then the callback fields.
fn authorization_result_from_manual_input(
    input: &str,
    expected_state: &str,
    redirect_uri: &str,
) -> Result<AuthorizationResult, AuthError> {
    let trimmed = input.trim();
    let url = url::Url::parse(trimmed).map_err(|_| {
        AuthError::Operation("Paste the full callback URL from the browser".to_string())
    })?;
    let expected = url::Url::parse(redirect_uri).expect("REDIRECT_URI must parse");
    if url_origin(&url) != url_origin(&expected) || url.path() != expected.path() {
        return Err(AuthError::Operation(format!(
            "The pasted callback URL must start with {redirect_uri}"
        )));
    }
    // `const error = url.searchParams.get("error"); if (error) throw` —
    // error_description is not consulted here (unlike the shared callback
    // server).
    if let Some(error) = url
        .query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect::<Vec<(String, String)>>()
        .iter()
        .find(|(name, _)| name == "error")
        .map(|(_, value)| value.clone())
        .filter(|value| !value.is_empty())
    {
        return Err(AuthError::Operation(format!(
            "ChatGPT authorization failed: {error}"
        )));
    }
    let params: Vec<(String, String)> = url
        .query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    authorization_result_from_callback(&params, expected_state)
}

/// The flow-local loopback callback server (upstream `startCallbackServer`,
/// openai-chatgpt.ts:113-176): routes and page strings differ from the shared
/// [`super::callback_server`] (no method check, no 409 handling, the state
/// validation happens in [`authorization_result_from_callback`]), so it stays
/// its own listener like upstream.
struct ChatGptCallbackServer {
    /// Upstream `result`: settles once with the delivered authorization
    /// result or the provider-error rejection.
    result: Waiter<Result<AuthorizationResult, AuthError>>,
    shutdown: CancellationToken,
    accept_loop: Option<tokio::task::JoinHandle<()>>,
}

impl ChatGptCallbackServer {
    async fn start(
        expected_state: String,
        callback_host: &str,
        callback_port: u16,
    ) -> Result<ChatGptCallbackServer, AuthError> {
        let listener = tokio::net::TcpListener::bind((callback_host, callback_port))
            .await
            .map_err(
                // An already-bound port is surfaced as AuthError::AddressInUse
                // for the v1.0.0 targeted login failure (upstream checks
                // error.code === EADDRINUSE).
                |error| {
                    if error.kind() == std::io::ErrorKind::AddrInUse {
                        AuthError::AddressInUse(error.to_string())
                    } else {
                        AuthError::Operation(error.to_string())
                    }
                },
            )?;
        let result = Waiter::new();
        let shutdown = CancellationToken::new();
        let task_shutdown = shutdown.clone();
        let loop_result = result.clone();
        let loop_state = expected_state;
        let accept_loop = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = task_shutdown.cancelled() => break,
                    // Transient accept errors must not kill the capture;
                    // upstream's server keeps listening too.
                    accepted = listener.accept() => match accepted {
                        Ok((stream, _)) => {
                            let result = loop_result.clone();
                            let expected_state = loop_state.clone();
                            tokio::spawn(handle_connection(
                                stream,
                                expected_state,
                                result,
                            ));
                        }
                        Err(_) => continue,
                    },
                }
            }
        });
        Ok(ChatGptCallbackServer {
            result,
            shutdown,
            accept_loop: Some(accept_loop),
        })
    }

    /// Upstream `result` (the raced promise).
    async fn wait(&self) -> Result<AuthorizationResult, AuthError> {
        match self.result.wait().await {
            // The settled value: a delivery or the provider-error rejection.
            Some(Ok(result)) => Ok(result),
            Some(Err(error)) => Err(error),
            // The waiter never drops its sender; unreachable in practice.
            None => Err(AuthError::Cancelled),
        }
    }

    /// Upstream `server.close()` in the final block: stop accepting and wait
    /// for the listener to drop.
    async fn close(mut self) {
        self.shutdown.cancel();
        if let Some(accept_loop) = self.accept_loop.take() {
            let _ = accept_loop.await;
        }
    }
}

/// `sendHtml` (openai-chatgpt.ts:110-113): one HTML response with only the
/// content type (no cache-control, unlike the shared callback server).
async fn send_html(stream: &mut tokio::net::TcpStream, status: u16, reason: &str, body: &str) {
    use tokio::io::AsyncWriteExt as _;
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {HTML_CONTENT_TYPE}\r\nContent-Length: \
         {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(body.as_bytes()).await;
    let _ = stream.flush().await;
    let _ = stream.shutdown().await;
}

/// One handled browser request (upstream request handler,
/// openai-chatgpt.ts:119-160). Every matching callback re-processes — the
/// settle slot makes later resolutions no-ops, and each request still gets
/// the success page.
async fn handle_connection(
    mut stream: tokio::net::TcpStream,
    expected_state: String,
    result: Waiter<Result<AuthorizationResult, AuthError>>,
) {
    let Some(request_line) = read_request_head(&mut stream).await else {
        // No readable request head: nothing to answer (upstream: an abandoned
        // browser request never completes either).
        return;
    };
    let target = request_target(&request_line);
    // `new URL(request.url || "", REDIRECT_URI)`: a relative target resolves
    // against the redirect URI.
    let url = match target {
        Some(target) => url::Url::options()
            .base_url(Some(
                &url::Url::parse("http://127.0.0.1:1455/auth/callback")
                    .expect("REDIRECT_URI base must parse"),
            ))
            .parse(target),
        None => Err(url::ParseError::RelativeUrlWithoutBase),
    };
    let Ok(url) = url else {
        // The upstream handler catch-all: `new URL` failures land in 500.
        send_html(
            &mut stream,
            500,
            "Internal Server Error",
            &oauth_error_html("Internal error while processing the callback.", None),
        )
        .await;
        return;
    };
    if url.path() != CALLBACK_PATH {
        send_html(
            &mut stream,
            404,
            "Not Found",
            &oauth_error_html("Callback route not found.", None),
        )
        .await;
        return;
    }

    let params: Vec<(String, String)> = url
        .query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    // `const error = url.searchParams.get("error"); if (error)` — truthiness.
    if let Some(error) = first_pair(&params, "error").filter(|value| !value.is_empty()) {
        send_html(
            &mut stream,
            400,
            "Bad Request",
            &oauth_error_html(
                "ChatGPT was not connected.",
                Some(&format!("Error: {error}")),
            ),
        )
        .await;
        result.settle(Some(Err(AuthError::Operation(format!(
            "ChatGPT authorization failed: {error}"
        )))));
        return;
    }

    // Callback-field validation failures render their message and keep the
    // login waiting (no rejection).
    match authorization_result_from_callback(&params, &expected_state) {
        Ok(authorization_result) => {
            send_html(
                &mut stream,
                200,
                "OK",
                &oauth_success_html("ChatGPT authentication completed. You can close this window."),
            )
            .await;
            result.settle(Some(Ok(authorization_result)));
        }
        Err(error) => {
            let AuthError::Operation(message) = error else {
                unreachable!("authorization_result_from_callback only reports operations");
            };
            send_html(
                &mut stream,
                400,
                "Bad Request",
                &oauth_error_html(&message, None),
            )
            .await;
        }
    }
}

/// The token endpoint response fields upstream reads (all optional; the
/// validators turn absence into the exact upstream error strings).
#[derive(Debug, Clone, Default)]
struct TokenResponse {
    access_token: Option<Value>,
    refresh_token: Option<Value>,
    expires_in: Option<Value>,
    id_token: Option<Value>,
    scope: Option<Value>,
}

/// Upstream `requestToken`: POST the form body to the token endpoint with the
/// lowercase `accept`/`content-type` headers, racing the interaction signal.
async fn request_token(
    token_url: &str,
    body: String,
    signal: &CancellationToken,
) -> Result<TokenResponse, AuthError> {
    let request = http_client()
        .post(token_url)
        .header("accept", "application/json")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body);

    let response = tokio::select! {
        biased;
        _ = signal.cancelled() => return Err(AuthError::Cancelled),
        response = request.send() => match response {
            Ok(response) => response,
            // Upstream's uncaught fetch rejection: the raw error text propagates.
            Err(error) => return Err(AuthError::Operation(error.to_string())),
        },
    };
    let status = response.status();
    let response_body = tokio::select! {
        biased;
        _ = signal.cancelled() => return Err(AuthError::Cancelled),
        body = response.text() => match body {
            Ok(body) => body,
            Err(error) => return Err(AuthError::Operation(error.to_string())),
        },
    };

    // `if (!response.ok)`: `responseBody || response.statusText`.
    if !status.is_success() {
        let detail = if response_body.is_empty() {
            status.canonical_reason().unwrap_or("")
        } else {
            response_body.as_str()
        };
        return Err(AuthError::Operation(format!(
            "OpenAI OAuth token request failed ({}): {detail}",
            status.as_u16()
        )));
    }
    // `await response.json()` — a parse failure propagates un-wrapped.
    let data: Value = serde_json::from_str(&response_body)
        .map_err(|error| AuthError::Operation(error.to_string()))?;
    // `typeof data !== "object" || data === null || Array.isArray(data)`.
    if !data.is_object() {
        return Err(AuthError::Operation(
            "OpenAI OAuth token response must be an object".to_string(),
        ));
    }
    Ok(TokenResponse {
        access_token: data.get("access_token").cloned(),
        refresh_token: data.get("refresh_token").cloned(),
        expires_in: data.get("expires_in").cloned(),
        id_token: data.get("id_token").cloned(),
        scope: data.get("scope").cloned(),
    })
}

/// Upstream `requireTokenString`: a non-empty-after-trim string.
fn require_token_string(value: Option<&Value>, field: &str) -> Result<String, AuthError> {
    let text = value
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty());
    match text {
        Some(text) => Ok(text.to_string()),
        None => Err(AuthError::Operation(format!(
            "OpenAI OAuth token response has invalid {field}"
        ))),
    }
}

/// Upstream `credentialFromTokenResponse`: validate every field, require the
/// direct-token scope, and build the stored credential with `clientId` and
/// `scopes` on the `extra` map (upstream index signature).
fn credential_from_token_response(
    token: &TokenResponse,
    client_id: &str,
) -> Result<OAuthCredential, AuthError> {
    let access = require_token_string(token.access_token.as_ref(), "access_token")?;
    let refresh = require_token_string(token.refresh_token.as_ref(), "refresh_token")?;
    let scope = require_token_string(token.scope.as_ref(), "scope")?;
    let expires_in = match token.expires_in.as_ref().and_then(Value::as_f64) {
        Some(expires_in) if expires_in.is_finite() && expires_in > 0.0 => expires_in,
        _ => {
            return Err(AuthError::Operation(
                "OpenAI OAuth token response has invalid expires_in".to_string(),
            ));
        }
    };
    // `scope.trim().split(/\s+/).filter(Boolean)` — split_whitespace
    // already skips leading/trailing runs, so no trim call.
    let scopes: Vec<String> = scope.split_whitespace().map(str::to_string).collect();
    if !scopes.iter().any(|granted| granted == DIRECT_TOKEN_SCOPE) {
        return Err(AuthError::Operation(format!(
            "OpenAI OAuth grant did not include {DIRECT_TOKEN_SCOPE}"
        )));
    }
    let mut extra = BTreeMap::new();
    extra.insert("clientId".to_string(), Value::String(client_id.to_string()));
    extra.insert(
        "scopes".to_string(),
        Value::Array(scopes.into_iter().map(Value::String).collect()),
    );
    Ok(OAuthCredential {
        refresh,
        access,
        expires: now_ms() + (expires_in * 1000.0) as i64 - EXPIRY_MARGIN_MS,
        extra,
    })
}

/// Upstream `exchangeAuthorizationCode`: the grant carries grant_type,
/// client_id, code, code_verifier, redirect_uri and resource — in that
/// order — and the response must carry an ID token (presence check only; Pi
/// does not read profile data from it).
async fn exchange_authorization_code(
    token_url: &str,
    code: &str,
    verifier: &str,
    client_id: &str,
    redirect_uri: &str,
    signal: &CancellationToken,
) -> Result<OAuthCredential, AuthError> {
    let body = {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("grant_type", "authorization_code");
        query.append_pair("client_id", client_id);
        query.append_pair("code", code);
        query.append_pair("code_verifier", verifier);
        query.append_pair("redirect_uri", redirect_uri);
        query.append_pair("resource", RESOURCE);
        query.finish()
    };
    let token = request_token(token_url, body, signal).await?;
    // `typeof token.id_token !== "string" || token.id_token.trim().length === 0`.
    let id_token = token
        .id_token
        .as_ref()
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    if id_token.is_none() {
        return Err(AuthError::Operation(
            "OpenAI OAuth token response did not contain an ID token".to_string(),
        ));
    }
    credential_from_token_response(&token, client_id)
}

/// Upstream `refreshAccessToken`: refresh with the stored issued client ID.
async fn refresh_access_token(
    token_url: &str,
    credential: &OAuthCredential,
    signal: &CancellationToken,
) -> Result<OAuthCredential, AuthError> {
    let client_id = credential
        .extra
        .get("clientId")
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let Some(client_id) = client_id else {
        return Err(AuthError::Operation(
            "Stored OpenAI OAuth credential does not contain an issued client ID; reconnect \
             ChatGPT"
                .to_string(),
        ));
    };
    let body = {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("grant_type", "refresh_token");
        query.append_pair("client_id", &client_id);
        query.append_pair("refresh_token", &credential.refresh);
        query.append_pair("resource", RESOURCE);
        query.finish()
    };
    let token = request_token(token_url, body, signal).await?;
    credential_from_token_response(&token, &client_id)
}

/// Upstream `loginOpenAIChatGPT` (openai-chatgpt.ts:237-305).
async fn login_openai_chatgpt(
    oauth: &OpenAIChatGptOAuth,
    interaction: ProviderAuthInteraction,
) -> Result<OAuthCredential, AuthError> {
    let redirect_uri = oauth.redirect_uri();
    // `agentHostId(options?.getDeviceId?.())` — before any authorization work.
    let host_id = agent_host_id(interaction.device_id().as_deref())?;
    let Pkce {
        verifier,
        challenge,
    } = generate_pkce();
    let state = random_value();
    let nonce = random_value();

    // v1.0.0: without this server, the browser's callback would reach
    // whatever else holds the port (another pending login or the Codex CLI),
    // which rejects it as a state mismatch. Fail with a clear error instead
    // of degrading to manual-only login.
    let callback: ChatGptCallbackServer = match ChatGptCallbackServer::start(
        state.clone(),
        &oauth.callback_host,
        oauth.callback_port,
    )
    .await
    {
        Ok(server) => server,
        Err(AuthError::AddressInUse(_)) => {
            // Upstream: `Port ${CALLBACK_PORT} is in use, probably by an
            // unfinished login in another pi session or by the Codex CLI.
            // Cancel that login and try again.` The port here is the
            // injected listener's.
            return Err(AuthError::Operation(format!(
                "Port {} is in use, probably by an unfinished login in another pi session or \
                 by the Codex CLI. Cancel that login and try again.",
                oauth.callback_port
            )));
        }
        Err(error) => return Err(error),
    };

    // `authorizationUrl.search = new URLSearchParams({...}).toString()` —
    // insertion order preserved, form-urlencoded serialization.
    let authorization_url = {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("client_id", DYNAMIC_CLIENT_ID);
        query.append_pair("agent_name_hint", AGENT_NAME_HINT);
        query.append_pair("ext_agent_host_id", &host_id);
        query.append_pair("response_type", "code");
        query.append_pair("redirect_uri", &redirect_uri);
        query.append_pair("resource", RESOURCE);
        query.append_pair("scope", SCOPE);
        query.append_pair("state", &state);
        query.append_pair("code_challenge", &challenge);
        query.append_pair("code_challenge_method", "S256");
        query.append_pair("nonce", &nonce);
        format!("{AUTHORIZE_URL}?{}", query.finish())
    };
    interaction.notify(AuthEvent::AuthUrl {
        url: authorization_url,
        instructions: Some(
            "Complete sign-in in your browser. If the callback does not complete, paste the final \
             redirect URL here."
                .to_string(),
        ),
    });

    // Upstream's `manualAbort` controller; the prompt signal is the
    // `AbortSignal.any([manualAbort.signal, interaction.signal])` combination.
    let manual_abort = CancellationToken::new();
    let prompt_signal = manual_abort.child_token();
    {
        let prompt_signal = prompt_signal.clone();
        let signal = interaction.signal.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = prompt_signal.cancelled() => {}
                _ = signal.cancelled() => prompt_signal.cancel(),
            }
        });
    }
    // `.then((input) => authorizationResultFromManualInput(input, state))`.
    let mut manual_code = Box::pin(async {
        let input = interaction
            .prompt(AuthPrompt {
                signal: Some(prompt_signal),
                kind: AuthPromptKind::ManualCode {
                    message: "Complete login in your browser, or paste the final redirect URL \
                              here:"
                        .to_string(),
                    placeholder: Some(redirect_uri.to_string()),
                },
            })
            .await?;
        authorization_result_from_manual_input(&input, &state, &redirect_uri)
    });

    // `await (callback ? Promise.race([callback.result, manualCode]) :
    // manualCode)` — first settle wins, no interaction-signal arm (cancellation
    // surfaces through the server wait or the combined prompt signal).
    let outcome = async {
        // The server always exists now (v1.0.0); the race is server vs
        // manual paste.
        let callback_result = callback.wait();
        tokio::pin!(callback_result);
        let result: Result<AuthorizationResult, AuthError> = tokio::select! {
            biased;
            settled = &mut callback_result => settled,
            parsed = &mut manual_code => parsed,
        };
        let result = result?;
        interaction.notify(AuthEvent::Progress {
            message: "Exchanging authorization code for tokens...".to_string(),
        });
        exchange_authorization_code(
            &oauth.token_url,
            &result.code,
            &verifier,
            &result.client_id,
            &redirect_uri,
            &interaction.signal,
        )
        .await
    }
    .await;
    // Upstream catch: `if (interaction.signal.aborted) throw new Error("Login
    // cancelled")`.
    let outcome = match outcome {
        Ok(credential) => Ok(credential),
        Err(error) => {
            if interaction.signal.is_cancelled() {
                Err(AuthError::Cancelled)
            } else {
                Err(error)
            }
        }
    };

    // Upstream finally: abort the manual prompt, close the server (and its
    // spare browser connections — see the module port notes).
    manual_abort.cancel();
    callback.close().await;
    outcome
}

/// The OpenAI ChatGPT OAuth auth surface (upstream `openaiChatGPTOAuth`,
/// openai-chatgpt.ts:307-315). [`OpenAIChatGptOAuth::new`] pins the upstream
/// endpoints; tests inject a wiremock token URL and callback host/port.
pub struct OpenAIChatGptOAuth {
    token_url: String,
    callback_host: String,
    callback_port: u16,
}

impl Default for OpenAIChatGptOAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenAIChatGptOAuth {
    /// Upstream module constants, with the callback host resolved from
    /// `PI_OAUTH_CALLBACK_HOST` (default `127.0.0.1`), like the upstream
    /// module-load read.
    pub fn new() -> Self {
        let callback_host = get_provider_env_value(CALLBACK_HOST_ENV, None)
            .unwrap_or_else(|| DEFAULT_CALLBACK_HOST.to_string());
        OpenAIChatGptOAuth {
            token_url: TOKEN_URL.to_string(),
            callback_host,
            callback_port: CALLBACK_PORT,
        }
    }

    /// Test constructor: point the token endpoint at a stub server and use a
    /// callback host/port that does not collide with other tests (upstream
    /// tests stub the global `fetch` and keep port 1455 because they run
    /// sequentially).
    #[cfg(test)]
    pub(crate) fn with_endpoints(
        token_url: String,
        callback_host: String,
        callback_port: u16,
    ) -> Self {
        OpenAIChatGptOAuth {
            token_url,
            callback_host,
            callback_port,
        }
    }

    /// The production redirect URI — upstream pins the constant
    /// `http://127.0.0.1:1455/auth/callback` (openai-chatgpt.ts:25): the
    /// authorize URL, the exchange body and the manual-input origin check all
    /// say 1455 no matter where the test-only listener binds.
    fn redirect_uri(&self) -> String {
        "http://127.0.0.1:1455/auth/callback".to_string()
    }
}

/// Oracle helper: rebuilds the authorization URL from the capture's
/// parameter map in the upstream insertion order (the emitted URL must be
/// byte-identical).
#[cfg(test)]
pub(crate) fn authorize_url_for_test(params: &serde_json::Value) -> String {
    const ORDER: [&str; 11] = [
        "client_id",
        "agent_name_hint",
        "ext_agent_host_id",
        "response_type",
        "redirect_uri",
        "resource",
        "scope",
        "state",
        "code_challenge",
        "code_challenge_method",
        "nonce",
    ];
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    for name in ORDER {
        query.append_pair(name, params[name].as_str().expect(name));
    }
    format!("{AUTHORIZE_URL}?{}", query.finish())
}

impl OAuthAuth for OpenAIChatGptOAuth {
    /// Upstream `name` (openai-chatgpt.ts:308).
    fn name(&self) -> &str {
        "OpenAI (ChatGPT subscription)"
    }

    /// Upstream `isSubscription: true` (openai-chatgpt.ts:309).
    fn is_subscription(&self) -> bool {
        true
    }

    /// Upstream `loginLabel` (openai-chatgpt.ts:310).
    fn login_label(&self) -> Option<&str> {
        Some("Sign in with ChatGPT")
    }

    /// Upstream `login` (openai-chatgpt.ts:311).
    fn login<'a>(
        &'a self,
        interaction: ProviderAuthInteraction,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(login_openai_chatgpt(self, interaction))
    }

    /// Upstream `refresh` (openai-chatgpt.ts:312).
    fn refresh<'a>(
        &'a self,
        credential: OAuthCredential,
        options: &'a crate::ai::auth::types::AuthOperationOptions,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move {
            let signal = options.signal.clone().unwrap_or_default();
            refresh_access_token(&self.token_url, &credential, &signal).await
        })
    }

    /// Upstream `toAuth` (openai-chatgpt.ts:313-315): `{ apiKey:
    /// credential.access }`.
    fn to_auth<'a>(
        &'a self,
        credential: OAuthCredential,
    ) -> BoxFuture<'a, Result<ModelAuth, AuthError>> {
        Box::pin(async move {
            Ok(ModelAuth {
                api_key: Some(credential.access),
                ..ModelAuth::default()
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::auth::oauth::parse_urlencoded_pairs;
    use crate::ai::auth::oauth::pkce::base64url_encode;
    use crate::ai::auth::types::{AuthInteraction, AuthOperationOptions, LoginOptions};
    use sha2::{Digest, Sha256};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use url::Url;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    type Respond =
        Box<dyn Fn(AuthPrompt) -> BoxFuture<'static, Result<String, AuthError>> + Send + Sync>;

    /// Minimal interaction: records events and prompts, answers prompts
    /// through the injected responder, and mirrors `auth_url` events into a
    /// slot the tests (and responders) can read while login is in flight.
    struct FakeInteraction {
        auth_url: Arc<Mutex<Option<String>>>,
        events: Mutex<Vec<AuthEvent>>,
        prompts: Mutex<Vec<AuthPrompt>>,
        respond: Respond,
    }

    impl AuthInteraction for FakeInteraction {
        fn signal(&self) -> Option<CancellationToken> {
            None
        }

        fn prompt(&self, prompt: AuthPrompt) -> BoxFuture<'_, Result<String, AuthError>> {
            self.prompts.lock().unwrap().push(prompt.clone());
            (self.respond)(prompt)
        }

        fn notify(&self, event: AuthEvent) {
            if let AuthEvent::AuthUrl { url, .. } = &event {
                *self.auth_url.lock().unwrap() = Some(url.clone());
            }
            self.events.lock().unwrap().push(event);
        }
    }

    fn fake_interaction(respond: Respond) -> (Arc<FakeInteraction>, ProviderAuthInteraction) {
        let fake = Arc::new(FakeInteraction {
            auth_url: Arc::new(Mutex::new(None)),
            events: Mutex::new(Vec::new()),
            prompts: Mutex::new(Vec::new()),
            respond,
        });
        let interaction = ProviderAuthInteraction::new(
            Arc::clone(&fake) as Arc<dyn AuthInteraction>,
            CancellationToken::new(),
        );
        (fake, interaction)
    }

    /// Interaction carrying the upstream test's device ID through
    /// `LoginOptions.getDeviceId`.
    fn with_device_id(
        (fake, interaction): (Arc<FakeInteraction>, ProviderAuthInteraction),
    ) -> (Arc<FakeInteraction>, ProviderAuthInteraction) {
        let interaction = interaction.with_login_options(Some(Arc::new(LoginOptions {
            get_device_id: Some(Box::new(|| {
                "e61bbe28-07ef-466d-8e5d-a344f94ab305".to_string()
            })),
        })));
        (fake, interaction)
    }

    /// A prompt responder that never answers; blocks on its prompt signal
    /// like a real pending UI prompt.
    fn hanging_respond() -> Respond {
        Box::new(|prompt| {
            Box::pin(async move {
                prompt.signal.unwrap_or_default().cancelled().await;
                Err(AuthError::Cancelled)
            })
        })
    }

    /// The pasted-query suffix builder shared by the paste responders.
    type SuffixFn = Arc<dyn Fn(&str, &str) -> String + Send + Sync>;

    /// A prompt responder that pastes the final redirect URL built from the
    /// emitted authorize URL (the upstream tests' interaction shape), with
    /// the pasted query suffix.
    fn pasting_respond(auth_url_slot: Arc<Mutex<Option<String>>>, suffix: SuffixFn) -> Respond {
        Box::new(move |_prompt| {
            let slot = Arc::clone(&auth_url_slot);
            let suffix = Arc::clone(&suffix);
            Box::pin(async move {
                let auth_url = loop {
                    if let Some(auth_url) = slot.lock().unwrap().clone() {
                        break auth_url;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                };
                let redirect = auth_url_param(&auth_url, "redirect_uri");
                let state = auth_url_param(&auth_url, "state");
                Ok(suffix(&redirect, &state))
            })
        })
    }

    fn free_callback_port() -> u16 {
        std::net::TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn flow_with(server: &MockServer, port: u16) -> OpenAIChatGptOAuth {
        OpenAIChatGptOAuth::with_endpoints(
            format!("{}/api/accounts/oauth/token", server.uri()),
            "127.0.0.1".to_string(),
            port,
        )
    }

    async fn mount_token_endpoint(server: &MockServer, body: String, status: u16) {
        Mock::given(method("POST"))
            .and(path("/api/accounts/oauth/token"))
            .respond_with(ResponseTemplate::new(status).set_body_raw(body, "application/json"))
            .mount(server)
            .await;
    }

    fn token_body_json(scope: &str) -> String {
        serde_json::json!({
            "access_token": "access-token",
            "refresh_token": "refresh-token",
            "expires_in": 3600,
            "id_token": "id-token",
            "scope": scope,
        })
        .to_string()
    }

    const REQUIRED_SCOPE: &str =
        "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";

    fn auth_url_of(fake: &FakeInteraction) -> String {
        fake.auth_url
            .lock()
            .unwrap()
            .clone()
            .expect("auth_url emitted")
    }

    fn auth_url_param(auth_url: &str, name: &str) -> String {
        Url::parse(auth_url)
            .unwrap()
            .query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
            .unwrap_or_else(|| panic!("missing auth URL parameter {name}"))
    }

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

    /// Waits for the emitted authorize URL.
    async fn wait_for_auth_url(slot: Arc<Mutex<Option<String>>>) -> String {
        loop {
            if let Some(auth_url) = slot.lock().unwrap().clone() {
                return auth_url;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    // ---- Units ----

    #[test]
    fn production_endpoints_match_upstream() {
        assert_eq!(DYNAMIC_CLIENT_ID, "dynamic_agent_client");
        assert_eq!(AGENT_NAME_HINT, "Pi");
        assert_eq!(
            AUTHORIZE_URL,
            "https://auth.openai.com/api/accounts/authorize"
        );
        assert_eq!(
            TOKEN_URL,
            "https://auth.openai.com/api/accounts/oauth/token"
        );
        assert_eq!(RESOURCE, "https://api.openai.com/v1");
        assert_eq!(DEFAULT_CALLBACK_HOST, "127.0.0.1");
        assert_eq!(CALLBACK_PORT, 1455);
        assert_eq!(CALLBACK_PATH, "/auth/callback");
        assert_eq!(DIRECT_TOKEN_SCOPE, "chatgpt.tokens.use.direct");
        assert_eq!(
            SCOPE,
            "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct"
        );
        assert_eq!(EXPIRY_MARGIN_MS, 3 * 60 * 1000);
        let oauth = OpenAIChatGptOAuth::new();
        assert_eq!(oauth.redirect_uri(), "http://127.0.0.1:1455/auth/callback");
    }

    #[test]
    fn agent_host_id_validates_the_device_uuid() {
        assert_eq!(
            agent_host_id(Some("E61BBE28-07EF-466D-8E5D-A344F94AB305")).unwrap(),
            "urn:uuid:e61bbe28-07ef-466d-8e5d-a344f94ab305"
        );
        let expected = AuthError::Operation(
            "Sign in with ChatGPT requires a device ID (UUID) for this installation".to_string(),
        );
        assert_eq!(agent_host_id(None).unwrap_err(), expected);
        assert_eq!(agent_host_id(Some("not-a-uuid")).unwrap_err(), expected);
        assert_eq!(
            agent_host_id(Some("e61bbe28-07ef-466d-8e5d-a344f94ab305!")).unwrap_err(),
            expected
        );
    }

    #[test]
    fn manual_input_validation_follows_the_upstream_messages() {
        let redirect = "http://127.0.0.1:1455/auth/callback";
        let state = "the-state";
        // Not a URL.
        assert_eq!(
            authorization_result_from_manual_input("garbage", state, redirect).unwrap_err(),
            AuthError::Operation("Paste the full callback URL from the browser".to_string())
        );
        // Wrong origin (port) and wrong path share the same message.
        for pasted in [
            "http://127.0.0.1:9999/auth/callback?code=c&state=the-state",
            "http://127.0.0.1:1455/other?code=c&state=the-state",
        ] {
            assert_eq!(
                authorization_result_from_manual_input(pasted, state, redirect).unwrap_err(),
                AuthError::Operation(format!(
                    "The pasted callback URL must start with {redirect}"
                ))
            );
        }
        // Provider error, without consulting error_description.
        assert_eq!(
            authorization_result_from_manual_input(
                "http://127.0.0.1:1455/auth/callback?error=access_denied&error_description=nope",
                state,
                redirect
            )
            .unwrap_err(),
            AuthError::Operation("ChatGPT authorization failed: access_denied".to_string())
        );
        // Missing code / state / issued client id, and a state mismatch.
        assert_eq!(
            authorization_result_from_manual_input(
                "http://127.0.0.1:1455/auth/callback",
                state,
                redirect
            )
            .unwrap_err(),
            AuthError::Operation("Missing authorization code".to_string())
        );
        assert_eq!(
            authorization_result_from_manual_input(
                "http://127.0.0.1:1455/auth/callback?code=c",
                state,
                redirect
            )
            .unwrap_err(),
            AuthError::Operation("Missing OAuth state".to_string())
        );
        assert_eq!(
            authorization_result_from_manual_input(
                "http://127.0.0.1:1455/auth/callback?code=c&state=other",
                state,
                redirect
            )
            .unwrap_err(),
            AuthError::Operation("OAuth state mismatch".to_string())
        );
        assert_eq!(
            authorization_result_from_manual_input(
                "http://127.0.0.1:1455/auth/callback?code=c&state=the-state",
                state,
                redirect
            )
            .unwrap_err(),
            AuthError::Operation(
                "OpenAI OAuth registration callback did not contain an issued client ID"
                    .to_string()
            )
        );
        // Whitespace-only client ids trim away (upstream `?.trim()`).
        assert_eq!(
            authorization_result_from_manual_input(
                "http://127.0.0.1:1455/auth/callback?code=c&state=the-state&client_id=%20%20",
                state,
                redirect
            )
            .unwrap_err(),
            AuthError::Operation(
                "OpenAI OAuth registration callback did not contain an issued client ID"
                    .to_string()
            )
        );
        // The happy path pastes through.
        let result = authorization_result_from_manual_input(
            "http://127.0.0.1:1455/auth/callback?code=the-code&state=the-state&client_id=oaiapp_issued",
            state,
            redirect,
        )
        .unwrap();
        assert_eq!(result.code, "the-code");
        assert_eq!(result.client_id, "oaiapp_issued");
    }

    #[tokio::test]
    async fn token_request_errors_carry_the_upstream_message_shape() {
        let server = MockServer::start().await;
        let token_url = format!("{}/api/accounts/oauth/token", server.uri());
        mount_token_endpoint(&server, "denied".to_string(), 400).await;
        let signal = CancellationToken::new();

        let error = request_token(&token_url, "grant_type=x".to_string(), &signal)
            .await
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("OpenAI OAuth token request failed (400): denied".to_string())
        );
    }

    #[tokio::test]
    async fn token_response_must_be_an_object() {
        let server = MockServer::start().await;
        let token_url = format!("{}/api/accounts/oauth/token", server.uri());
        mount_token_endpoint(&server, "[1,2]".to_string(), 200).await;
        let signal = CancellationToken::new();

        let error = request_token(&token_url, "grant_type=x".to_string(), &signal)
            .await
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("OpenAI OAuth token response must be an object".to_string())
        );
    }

    #[tokio::test]
    async fn credential_validation_requires_each_field_and_the_direct_scope() {
        let full = TokenResponse {
            access_token: Some(Value::String("a".to_string())),
            refresh_token: Some(Value::String("r".to_string())),
            expires_in: Some(Value::from(3600)),
            id_token: Some(Value::String("i".to_string())),
            scope: Some(Value::String(REQUIRED_SCOPE.to_string())),
        };
        let credential = credential_from_token_response(&full, "oaiapp_issued").unwrap();
        assert_eq!(credential.access, "a");
        assert_eq!(credential.refresh, "r");
        assert!((credential.expires - (now_ms() + 3_600_000 - EXPIRY_MARGIN_MS)).abs() <= 2_000);
        assert_eq!(
            credential.extra.get("clientId").and_then(Value::as_str),
            Some("oaiapp_issued")
        );
        let scopes: Vec<Value> = REQUIRED_SCOPE.split(' ').map(Value::from).collect();
        assert_eq!(
            credential
                .extra
                .get("scopes")
                .and_then(Value::as_array)
                .unwrap(),
            &scopes
        );

        let mut no_refresh = full.clone();
        no_refresh.refresh_token = None;
        assert_eq!(
            credential_from_token_response(&no_refresh, "c").unwrap_err(),
            AuthError::Operation(
                "OpenAI OAuth token response has invalid refresh_token".to_string()
            )
        );
        let mut blank_access = full.clone();
        blank_access.access_token = Some(Value::String("   ".to_string()));
        assert_eq!(
            credential_from_token_response(&blank_access, "c").unwrap_err(),
            AuthError::Operation(
                "OpenAI OAuth token response has invalid access_token".to_string()
            )
        );
        let mut zero_expires = full.clone();
        zero_expires.expires_in = Some(Value::from(0));
        assert_eq!(
            credential_from_token_response(&zero_expires, "c").unwrap_err(),
            AuthError::Operation("OpenAI OAuth token response has invalid expires_in".to_string())
        );
        let mut narrow_scope = full.clone();
        narrow_scope.scope = Some(Value::String(
            "openid profile email offline_access resource.invoke".to_string(),
        ));
        assert_eq!(
            credential_from_token_response(&narrow_scope, "c").unwrap_err(),
            AuthError::Operation(
                "OpenAI OAuth grant did not include chatgpt.tokens.use.direct".to_string()
            )
        );
        // Whitespace-padded scope survives trim + whitespace split.
        let mut padded = full.clone();
        padded.scope = Some(Value::String(format!("  {}  ", REQUIRED_SCOPE)));
        assert!(credential_from_token_response(&padded, "c").is_ok());
    }

    #[tokio::test]
    async fn exchange_requires_an_id_token_in_the_response() {
        let server = MockServer::start().await;
        let token_url = format!("{}/api/accounts/oauth/token", server.uri());
        // Valid otherwise, but without id_token.
        mount_token_endpoint(
            &server,
            serde_json::json!({
                "access_token": "a", "refresh_token": "r", "expires_in": 3600,
                "scope": REQUIRED_SCOPE,
            })
            .to_string(),
            200,
        )
        .await;
        let signal = CancellationToken::new();

        let error = exchange_authorization_code(
            &token_url,
            "code",
            "verifier",
            "client",
            "http://127.0.0.1:1455/auth/callback",
            &signal,
        )
        .await
        .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation(
                "OpenAI OAuth token response did not contain an ID token".to_string()
            )
        );
    }

    #[tokio::test]
    async fn refresh_requires_the_issued_client_id() {
        let oauth = OpenAIChatGptOAuth::with_endpoints(
            "http://127.0.0.1:1/api/accounts/oauth/token".to_string(),
            "127.0.0.1".to_string(),
            free_callback_port(),
        );
        let error = oauth
            .refresh(
                OAuthCredential {
                    refresh: "r".to_string(),
                    access: "a".to_string(),
                    expires: 0,
                    extra: BTreeMap::new(),
                },
                &AuthOperationOptions::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation(
                "Stored OpenAI OAuth credential does not contain an issued client ID; reconnect \
                 ChatGPT"
                    .to_string()
            )
        );
    }

    // ---- Oracle ports (packages/ai/test/openai-chatgpt-oauth.test.ts) ----

    /// Oracle: "registers a user-owned client and stores its issued ID and
    /// granted scopes", driven through the real loopback callback server.
    #[tokio::test]
    async fn registers_a_user_owned_client_and_stores_its_issued_id_and_scopes() {
        let server = MockServer::start().await;
        mount_token_endpoint(&server, token_body_json(REQUIRED_SCOPE), 200).await;
        let port = free_callback_port();
        let oauth = flow_with(&server, port);
        let (fake, interaction) = with_device_id(fake_interaction(hanging_respond()));

        // Drive the loopback callback like the browser redirect.
        let auth_url_slot = Arc::clone(&fake.auth_url);
        let driver = tokio::spawn(async move {
            let auth_url = wait_for_auth_url(auth_url_slot).await;
            let state = auth_url_param(&auth_url, "state");
            let response = http_get(
                port,
                &format!(
                    "/auth/callback?code=authorization-code&state={state}&client_id=oaiapp_issued"
                ),
            )
            .await;
            (auth_url, response)
        });

        let credential = tokio::time::timeout(Duration::from_secs(10), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap();
        let (auth_url, response) = driver.await.unwrap();

        // The authorize URL carries the registration parameters; the
        // redirect_uri keeps the pinned 127.0.0.1:1455 form.
        assert_eq!(
            auth_url_param(&auth_url, "client_id"),
            "dynamic_agent_client"
        );
        assert_eq!(auth_url_param(&auth_url, "agent_name_hint"), "Pi");
        assert_eq!(
            auth_url_param(&auth_url, "ext_agent_host_id"),
            "urn:uuid:e61bbe28-07ef-466d-8e5d-a344f94ab305"
        );
        assert_eq!(auth_url_param(&auth_url, "response_type"), "code");
        assert_eq!(
            auth_url_param(&auth_url, "redirect_uri"),
            "http://127.0.0.1:1455/auth/callback"
        );
        assert_eq!(auth_url_param(&auth_url, "resource"), RESOURCE);
        assert_eq!(auth_url_param(&auth_url, "scope"), SCOPE);
        assert_eq!(auth_url_param(&auth_url, "code_challenge_method"), "S256");

        // The success page (bytes pinned against the oracle fixture; shape
        // here).
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("Content-Type: text/html; charset=utf-8"));
        assert!(response.contains("<h1>Authentication successful</h1>"));
        assert!(response.contains("ChatGPT authentication completed. You can close this window."));

        // The exchange request body: form-urlencoded fields in upstream
        // order, with the issued client ID and the S256 verifier.
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let body = String::from_utf8_lossy(&requests[0].body).into_owned();
        let params = parse_urlencoded_pairs(&body);
        let get = |name: &str| first_pair(&params, name).unwrap();
        assert_eq!(get("grant_type"), "authorization_code");
        assert_eq!(get("client_id"), "oaiapp_issued");
        assert_eq!(get("code"), "authorization-code");
        assert_eq!(get("redirect_uri"), "http://127.0.0.1:1455/auth/callback");
        assert_eq!(get("resource"), RESOURCE);
        let challenge = auth_url_param(&auth_url, "code_challenge");
        assert_eq!(
            base64url_encode(&Sha256::digest(get("code_verifier").as_bytes())),
            challenge
        );

        // The stored credential carries the issued ID and granted scopes.
        assert_eq!(credential.access, "access-token");
        assert_eq!(credential.refresh, "refresh-token");
        assert_eq!(
            credential.extra.get("clientId").and_then(Value::as_str),
            Some("oaiapp_issued")
        );
        let scopes: Vec<Value> = REQUIRED_SCOPE.split(' ').map(Value::from).collect();
        assert_eq!(
            credential
                .extra
                .get("scopes")
                .and_then(Value::as_array)
                .unwrap(),
            &scopes
        );
        // The manual prompt was aborted by the finally block.
        assert!(fake.prompts.lock().unwrap()[0]
            .signal
            .as_ref()
            .unwrap()
            .is_cancelled());
    }

    /// Oracle: "rejects registration without an issued client ID" — the
    /// browser redirect carries no client_id, the page shows the message and
    /// the login keeps waiting (no token request), and a later correct
    /// redirect completes.
    #[tokio::test]
    async fn rejects_registration_without_an_issued_client_id_then_completes() {
        let server = MockServer::start().await;
        mount_token_endpoint(&server, token_body_json(REQUIRED_SCOPE), 200).await;
        let port = free_callback_port();
        let oauth = flow_with(&server, port);
        let (fake, interaction) = with_device_id(fake_interaction(hanging_respond()));

        let auth_url_slot = Arc::clone(&fake.auth_url);
        let driver = tokio::spawn(async move {
            let auth_url = wait_for_auth_url(auth_url_slot).await;
            let state = auth_url_param(&auth_url, "state");
            let first = http_get(
                port,
                &format!("/auth/callback?code=authorization-code&state={state}"),
            )
            .await;
            let second = http_get(
                port,
                &format!(
                    "/auth/callback?code=authorization-code&state={state}&client_id=oaiapp_issued"
                ),
            )
            .await;
            (first, second)
        });

        let credential = tokio::time::timeout(Duration::from_secs(10), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap();
        let (first, second) = driver.await.unwrap();

        assert!(first.starts_with("HTTP/1.1 400 Bad Request\r\n"));
        assert!(first
            .contains("OpenAI OAuth registration callback did not contain an issued client ID"));
        assert!(second.starts_with("HTTP/1.1 200 OK\r\n"));
        // Exactly one token request: the first redirect never reached the
        // exchange.
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        assert_eq!(
            credential.extra.get("clientId").and_then(Value::as_str),
            Some("oaiapp_issued")
        );
    }

    /// Oracle: "rejects a token response that did not grant direct token
    /// use".
    #[tokio::test]
    async fn rejects_a_token_response_without_the_direct_token_scope() {
        let server = MockServer::start().await;
        mount_token_endpoint(
            &server,
            token_body_json("openid profile email offline_access resource.invoke"),
            200,
        )
        .await;
        let port = free_callback_port();
        let oauth = flow_with(&server, port);
        let (fake, interaction) = with_device_id(fake_interaction(hanging_respond()));

        let auth_url_slot = Arc::clone(&fake.auth_url);
        let driver = tokio::spawn(async move {
            let auth_url = wait_for_auth_url(auth_url_slot).await;
            let state = auth_url_param(&auth_url, "state");
            http_get(
                port,
                &format!(
                    "/auth/callback?code=authorization-code&state={state}&client_id=oaiapp_issued"
                ),
            )
            .await
        });

        let error = tokio::time::timeout(Duration::from_secs(10), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap_err();
        driver.await.unwrap();
        assert_eq!(
            error,
            AuthError::Operation(
                "OpenAI OAuth grant did not include chatgpt.tokens.use.direct".to_string()
            )
        );
        let _ = auth_url_of(&fake);
    }

    /// Oracle: "requires a device ID before starting authorization" — no
    /// authorize URL, no prompt, no network.
    #[tokio::test]
    async fn requires_a_device_id_before_starting_authorization() {
        let server = MockServer::start().await;
        mount_token_endpoint(&server, token_body_json(REQUIRED_SCOPE), 200).await;
        let oauth = flow_with(&server, free_callback_port());
        let (fake, interaction) = fake_interaction(hanging_respond());

        let error = oauth.login(interaction).await.unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation(
                "Sign in with ChatGPT requires a device ID (UUID) for this installation"
                    .to_string()
            )
        );
        assert!(
            fake.auth_url.lock().unwrap().is_none(),
            "no authorize URL may be emitted"
        );
        assert!(
            fake.prompts.lock().unwrap().is_empty(),
            "no prompt may start"
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 0);
    }

    /// Oracle: "requires refresh responses to rotate the refresh token".
    #[tokio::test]
    async fn requires_refresh_responses_to_rotate_the_refresh_token() {
        let server = MockServer::start().await;
        let response = serde_json::json!({
            "access_token": "access-token",
            "expires_in": 3600,
            "id_token": "id-token",
            "scope": REQUIRED_SCOPE,
        });
        mount_token_endpoint(&server, response.to_string(), 200).await;
        let oauth = OpenAIChatGptOAuth::with_endpoints(
            format!("{}/api/accounts/oauth/token", server.uri()),
            "127.0.0.1".to_string(),
            free_callback_port(),
        );

        let credential = OAuthCredential {
            refresh: "old-refresh".to_string(),
            access: "old-access".to_string(),
            expires: 0,
            extra: BTreeMap::from([(
                "clientId".to_string(),
                Value::String("oaiapp_existing".to_string()),
            )]),
        };
        let error = oauth
            .refresh(credential, &AuthOperationOptions::default())
            .await
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation(
                "OpenAI OAuth token response has invalid refresh_token".to_string()
            )
        );
    }

    /// Oracle: "refreshes with the credential's issued client ID and stores
    /// replacement scopes".
    #[tokio::test]
    async fn refreshes_with_the_credentials_issued_client_id_and_stores_replacement_scopes() {
        let server = MockServer::start().await;
        let response = serde_json::json!({
            "access_token": "new-access",
            "refresh_token": "new-refresh",
            "expires_in": 3600,
            "id_token": "id-token",
            "scope": REQUIRED_SCOPE,
        });
        mount_token_endpoint(&server, response.to_string(), 200).await;
        let oauth = OpenAIChatGptOAuth::with_endpoints(
            format!("{}/api/accounts/oauth/token", server.uri()),
            "127.0.0.1".to_string(),
            free_callback_port(),
        );

        let credential = OAuthCredential {
            refresh: "old-refresh".to_string(),
            access: "old-access".to_string(),
            expires: 0,
            extra: BTreeMap::from([(
                "clientId".to_string(),
                Value::String("oaiapp_existing".to_string()),
            )]),
        };
        let refreshed = oauth
            .refresh(credential, &AuthOperationOptions::default())
            .await
            .unwrap();

        // expires = now + 3600s - 180s (the early-refresh margin).
        assert!((refreshed.expires - (now_ms() + (3600 - 180) * 1000)).abs() <= 2_000);
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let params = parse_urlencoded_pairs(&String::from_utf8_lossy(&requests[0].body));
        let get = |name: &str| first_pair(&params, name).unwrap();
        assert_eq!(get("grant_type"), "refresh_token");
        assert_eq!(get("client_id"), "oaiapp_existing");
        assert_eq!(get("refresh_token"), "old-refresh");
        assert_eq!(get("resource"), RESOURCE);
        assert!(first_pair(&params, "scope").is_none());
        assert_eq!(refreshed.access, "new-access");
        assert_eq!(refreshed.refresh, "new-refresh");
        assert_eq!(
            refreshed.extra.get("clientId").and_then(Value::as_str),
            Some("oaiapp_existing")
        );
    }

    /// v1.0.0: a taken callback port fails the login with the targeted
    /// message instead of degrading to manual paste (the browser callback
    /// would hit whatever else holds the port, which rejects it as a state
    /// mismatch).
    #[tokio::test]
    async fn bind_failure_fails_the_login_when_the_port_is_taken() {
        let server = MockServer::start().await;
        mount_token_endpoint(&server, token_body_json(REQUIRED_SCOPE), 200).await;
        // Occupy the callback port so the bind fails.
        let blocker = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = blocker.local_addr().unwrap().port();
        let oauth = flow_with(&server, port);

        // One fake whose auth_url slot the paste responder shares.
        let auth_url_slot = Arc::new(Mutex::new(None));
        let paste_slot = Arc::clone(&auth_url_slot);
        let fake = Arc::new(FakeInteraction {
            auth_url: auth_url_slot,
            events: Mutex::new(Vec::new()),
            prompts: Mutex::new(Vec::new()),
            respond: pasting_respond(
                paste_slot,
                Arc::new(|redirect, state| {
                    format!(
                        "{redirect}?code=authorization-code&state={state}&client_id=oaiapp_issued"
                    )
                }),
            ),
        });
        let base_interaction = ProviderAuthInteraction::new(
            Arc::clone(&fake) as Arc<dyn AuthInteraction>,
            CancellationToken::new(),
        );
        let (_fake, interaction) = with_device_id((fake, base_interaction));

        // Upstream: `Port ${CALLBACK_PORT} is in use, probably by an
        // unfinished login in another pi session or by the Codex CLI. Cancel
        // that login and try again.`
        let error = tokio::time::timeout(Duration::from_secs(5), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation(format!(
                "Port {port} is in use, probably by an unfinished login in another pi session or by the Codex CLI. Cancel that login and try again."
            ))
        );
        // No token request happened (the callback server never bound).
        assert_eq!(server.received_requests().await.unwrap().len(), 0);
        drop(blocker);
    }

    /// Manual paste of a bare code (callback server healthy) loses the
    /// upstream parse: the bare code is not a URL, so the paste message
    /// rejects and the login fails without a token request.
    #[tokio::test]
    async fn bare_code_paste_rejects_with_the_paste_the_url_message() {
        let server = MockServer::start().await;
        mount_token_endpoint(&server, token_body_json(REQUIRED_SCOPE), 200).await;
        let oauth = flow_with(&server, free_callback_port());
        let (fake, interaction) = with_device_id(fake_interaction(Box::new(|_prompt| {
            Box::pin(async move { Ok("the-pasted-code".to_string()) })
        })));

        let result = tokio::time::timeout(Duration::from_secs(5), oauth.login(interaction))
            .await
            .unwrap();
        assert_eq!(
            result.unwrap_err(),
            AuthError::Operation("Paste the full callback URL from the browser".to_string())
        );
        assert!(fake
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, AuthEvent::AuthUrl { .. })));
        assert_eq!(server.received_requests().await.unwrap().len(), 0);
    }

    /// Cancelled interaction signal: the catch normalizes every error to the
    /// cancellation.
    #[tokio::test]
    async fn cancelled_interaction_signal_surfaces_as_cancelled() {
        let server = MockServer::start().await;
        mount_token_endpoint(&server, token_body_json(REQUIRED_SCOPE), 200).await;
        let oauth = flow_with(&server, free_callback_port());
        let (fake, interaction) = with_device_id(fake_interaction(hanging_respond()));
        let signal = interaction.signal.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            signal.cancel();
        });

        let result = tokio::time::timeout(Duration::from_secs(5), oauth.login(interaction))
            .await
            .unwrap();
        assert_eq!(result, Err(AuthError::Cancelled));
        let _ = auth_url_of(&fake);
    }

    /// The flow-local callback routes (page bytes pinned against the oracle
    /// fixture): wrong path, provider error, field validation keeps waiting,
    /// success settles.
    #[tokio::test]
    async fn callback_server_routes_follow_the_upstream_order() {
        let port = free_callback_port();
        let server = ChatGptCallbackServer::start("expected-state".to_string(), "127.0.0.1", port)
            .await
            .unwrap();

        // Wrong path: 404 "Callback route not found."
        let response = http_get(port, "/nope?code=c&state=expected-state").await;
        assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(response.contains("Callback route not found."));

        // Provider error: 400 "ChatGPT was not connected." + rejection.
        let response = http_get(
            port,
            "/auth/callback?error=access_denied&state=expected-state",
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 400 Bad Request\r\n"));
        assert!(response.contains("ChatGPT was not connected."));
        assert!(response.contains("Error: access_denied"));
        let error = tokio::time::timeout(Duration::from_secs(2), server.wait())
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("ChatGPT authorization failed: access_denied".to_string())
        );
        server.close().await;

        // A fresh server: field validation keeps waiting, success settles.
        let server = ChatGptCallbackServer::start("expected-state".to_string(), "127.0.0.1", port)
            .await
            .unwrap();
        let response = http_get(port, "/auth/callback?code=c&state=wrong").await;
        assert!(response.contains("OAuth state mismatch"));
        let response = http_get(port, "/auth/callback?state=expected-state").await;
        assert!(response.contains("Missing authorization code"));
        let response = http_get(port, "/auth/callback?code=c&state=expected-state").await;
        assert!(response.contains("did not contain an issued client ID"));
        let response = http_get(
            port,
            "/auth/callback?code=the-code&state=expected-state&client_id=oaiapp_issued",
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("ChatGPT authentication completed. You can close this window."));
        let result = tokio::time::timeout(Duration::from_secs(2), server.wait())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.code, "the-code");
        assert_eq!(result.client_id, "oaiapp_issued");
        server.close().await;
    }

    #[tokio::test]
    async fn to_auth_derives_the_request_api_key_from_the_access_token() {
        let oauth = OpenAIChatGptOAuth::new();
        assert_eq!(oauth.name(), "OpenAI (ChatGPT subscription)");
        assert!(oauth.is_subscription());
        assert_eq!(oauth.login_label(), Some("Sign in with ChatGPT"));
        let auth = oauth
            .to_auth(OAuthCredential {
                refresh: "r".to_string(),
                access: "access-token".to_string(),
                expires: 42,
                extra: BTreeMap::new(),
            })
            .await
            .unwrap();
        assert_eq!(auth.api_key.as_deref(), Some("access-token"));
        assert_eq!(auth.headers, None);
        assert_eq!(auth.base_url, None);
    }
}
