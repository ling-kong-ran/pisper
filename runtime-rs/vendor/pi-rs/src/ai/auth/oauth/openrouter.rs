//! OpenRouter OAuth PKCE flow ported from upstream
//! `packages/ai/src/auth/oauth/openrouter.ts`: OpenRouter exchanges an
//! authorization code for a permanent, user-controlled API key rather than
//! an expiring access/refresh token pair. The callback is handled by a
//! one-shot loopback server on an ephemeral port, raced against a manual
//! prompt so remote/headless sessions can paste the redirect URL when the
//! browser cannot reach the loopback server.
//!
//! Interactive surface (M2d ruling): the browser gets the authorize URL via
//! [`AuthEvent::AuthUrl`], progress via [`AuthEvent::Progress`], and the
//! pasted redirect/code arrives through a `manual_code` prompt — the flow
//! never touches stdio or a browser directly. The loopback callback server
//! is flow-owned infrastructure, like upstream's `http.createServer`.
//!
//! Port notes (disclosed divergences):
//! - The token endpoint URL and callback host are fields on
//!   [`OpenRouterOAuth`] (upstream: module constants / `getCallbackHost()`)
//!   so tests can point the flow at a wiremock server and a custom host.
//!   The production constructor pins the upstream values and reads
//!   `PI_OAUTH_CALLBACK_HOST` (default `127.0.0.1`).
//! - The login runs on the shared [`super::callback_server`] (upstream
//!   `callback-server.ts`, which this delta refactored the private
//!   `startCallbackServer` onto): the exchange runs inside the callback
//!   handler so the browser page shows its outcome (200 success / 502
//!   "OpenRouter sign-in failed."), the login timeout renders as
//!   "OpenRouter sign-in timed out", and the 404/409 pages carry the shared
//!   strings ("Callback route not found." / "This sign-in has already been
//!   handled.").
//! - `crypto.randomUUID()` becomes a random RFC 4122 version-4 UUID built
//!   from 16 `rand` bytes (version/variant bits set by hand); the oracle
//!   tests inject the capture's fixed UUID through [`super::test_entropy`].
//! - A request line without a parseable target answers 404 (the method/path
//!   mismatch branch — there is no pathname to match). Node itself answers
//!   400 for malformed request lines before the handler runs.
//! - The token-exchange request body serializes with serde_json's map
//!   ordering (alphabetical) instead of `JSON.stringify` insertion order;
//!   OpenRouter's endpoint parses JSON, so the wire semantics are unchanged.
//! - Transport failures of the exchange request carry the raw transport
//!   error text (upstream's `fetch` rejection propagates un-caught). The
//!   30-second exchange timeout surfaces upstream's exact message.
//! - Cancellation maps to [`AuthError::Cancelled`] everywhere upstream
//!   throws `Error("Login cancelled")` (port contract: interaction-signal
//!   aborts are never wrapped). A cancelled exchange still renders the 502
//!   callback page with the "Login cancelled" detail, like the upstream
//!   handler catch, before the login surfaces `Cancelled`.

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::ai::api::azure_openai_responses::get_provider_env_value;
use crate::ai::api::http_client;
use crate::ai::auth::types::{
    AuthError, AuthEvent, AuthInteraction, ModelAuth, OAuthAuth, OAuthCredential,
    ProviderAuthInteraction,
};

use super::callback_server::{
    start_oauth_callback_server, wait_for_callback_or_manual_input, CallbackOrManual,
    CallbackServerOptions, OAuthCallbackServer,
};
use super::pkce::{generate_pkce, Pkce};
use super::{first_pair, parse_urlencoded_pairs, uuid_v4};

/// Upstream `AUTHORIZE_URL` (openrouter.ts:20).
const AUTHORIZE_URL: &str = "https://openrouter.ai/auth";

/// Upstream `TOKEN_URL` (openrouter.ts:21); the production default for
/// [`OpenRouterOAuth::new`].
const TOKEN_URL: &str = "https://openrouter.ai/api/v1/auth/keys";

/// Upstream `LOGIN_TIMEOUT_MS` (openrouter.ts:22).
const LOGIN_TIMEOUT: Duration = Duration::from_millis(5 * 60 * 1000);

/// Upstream `TOKEN_EXCHANGE_TIMEOUT_MS` (openrouter.ts:23).
const TOKEN_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(30);

/// Upstream `getProviderEnvValue("PI_OAUTH_CALLBACK_HOST")` (openrouter.ts:26).
const CALLBACK_HOST_ENV: &str = "PI_OAUTH_CALLBACK_HOST";

/// Upstream `|| "127.0.0.1"` fallback for the callback host.
const DEFAULT_CALLBACK_HOST: &str = "127.0.0.1";

/// Upstream `Number.MAX_SAFE_INTEGER`: a permanent key never expires.
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// The OpenRouter OAuth auth surface (upstream `openRouterOAuth`,
/// openrouter.ts:301-311). [`OpenRouterOAuth::new`] pins the upstream
/// endpoints; tests inject a wiremock token URL and callback host.
pub struct OpenRouterOAuth {
    token_url: String,
    callback_host: String,
}

impl Default for OpenRouterOAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenRouterOAuth {
    /// Upstream module constants, with the callback host resolved from
    /// `PI_OAUTH_CALLBACK_HOST` (default `127.0.0.1`), like the upstream
    /// `getCallbackHost()` call at server start.
    pub fn new() -> Self {
        let callback_host = get_provider_env_value(CALLBACK_HOST_ENV, None)
            .unwrap_or_else(|| DEFAULT_CALLBACK_HOST.to_string());
        OpenRouterOAuth {
            token_url: TOKEN_URL.to_string(),
            callback_host,
        }
    }

    /// Test constructor: point the token endpoint at a stub server and bind
    /// a fixed callback host (upstream tests stub the global `fetch`).
    #[cfg(test)]
    pub(crate) fn with_endpoints(token_url: String, callback_host: String) -> Self {
        OpenRouterOAuth {
            token_url,
            callback_host,
        }
    }
}

/// Upstream `parseAuthorizationInput` (openrouter.ts:52-67): a pasted
/// redirect URL (query `code`), a bare query string, or a bare code. Unlike
/// the anthropic/codex variant there is no `code#state` branch, and a
/// parseable URL without a `code` parameter yields `None` instead of
/// falling through.
fn parse_authorization_input(input: &str) -> Option<String> {
    let value = input.trim();
    if value.is_empty() {
        return None;
    }

    // WHATWG `new URL(value)` — absolute URLs only; anything else falls
    // through like the upstream catch. A parsed URL returns immediately,
    // even when it carries no code at all.
    if let Ok(url) = url::Url::parse(value) {
        let pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect();
        return first_pair(&pairs, "code");
    }

    if value.contains("code=") {
        // `new URLSearchParams(value).get("code")`: the constructor strips
        // exactly one leading `?` (browser URL bar pastes).
        let query = value.strip_prefix('?').unwrap_or(value);
        let pairs = parse_urlencoded_pairs(query);
        return first_pair(&pairs, "code");
    }

    Some(value.to_string())
}

/// Upstream `errorDetail` (openrouter.ts:69-78): `error_description`, then
/// `message`, then `error` (each when a string), then an object `error`'s
/// string `message`.
fn error_detail(body: &serde_json::Map<String, Value>) -> Option<String> {
    for field in ["error_description", "message", "error"] {
        if let Some(Value::String(text)) = body.get(field) {
            return Some(text.clone());
        }
    }
    match body.get("error") {
        // `!Array.isArray(body.error)` — serde's `Value::get` on an array
        // misses string keys, so the array case already falls through.
        Some(error) => error
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string),
        None => None,
    }
}

/// Upstream `exchangeAuthorizationCode` (openrouter.ts:80-133): a JSON POST
/// racing the interaction signal and the 30-second exchange timeout. The
/// response body's `key` becomes a permanent credential (`refresh: ""`,
/// `expires: Number.MAX_SAFE_INTEGER`).
async fn exchange_authorization_code(
    token_url: &str,
    code: &str,
    verifier: &str,
    signal: &CancellationToken,
) -> Result<OAuthCredential, AuthError> {
    // `if (signal.aborted) throw new Error("Login cancelled")`.
    if signal.is_cancelled() {
        return Err(AuthError::Cancelled);
    }
    let body = serde_json::to_string(&serde_json::json!({
        "code": code,
        "code_verifier": verifier,
        "code_challenge_method": "S256",
    }))
    .expect("token exchange body must serialize");

    let deadline = tokio::time::Instant::now() + TOKEN_EXCHANGE_TIMEOUT;
    let request = http_client()
        .post(token_url)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .body(body);

    let response = tokio::select! {
        biased;
        _ = signal.cancelled() => return Err(AuthError::Cancelled),
        _ = tokio::time::sleep_until(deadline) => {
            return Err(AuthError::Operation(
                "OpenRouter OAuth token exchange timed out".to_string(),
            ));
        }
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
        _ = tokio::time::sleep_until(deadline) => {
            return Err(AuthError::Operation(
                "OpenRouter OAuth token exchange timed out".to_string(),
            ));
        }
        body = response.text() => match body {
            Ok(body) => body,
            Err(error) => return Err(AuthError::Operation(error.to_string())),
        },
    };

    // Upstream parse: an object body is kept; a parsed non-object leaves the
    // body empty; a parse failure leaves it empty unless the response was
    // ok, in which case the invalid JSON errors.
    let body: serde_json::Map<String, Value> = match serde_json::from_str::<Value>(&response_body) {
        Ok(Value::Object(map)) => map,
        Ok(_) => serde_json::Map::new(),
        Err(_) if status.is_success() => {
            return Err(AuthError::Operation(
                "OpenRouter OAuth returned invalid JSON".to_string(),
            ));
        }
        Err(_) => serde_json::Map::new(),
    };

    if !status.is_success() {
        let detail = error_detail(&body);
        return Err(AuthError::Operation(format!(
            "OpenRouter OAuth key exchange failed (HTTP {}){}",
            status.as_u16(),
            detail
                .map(|detail| format!(": {detail}"))
                .unwrap_or_default()
        )));
    }

    let key = body
        .get("key")
        .and_then(Value::as_str)
        .filter(|key| !key.is_empty());
    let Some(key) = key else {
        return Err(AuthError::Operation(
            "OpenRouter OAuth response carries no \"key\"".to_string(),
        ));
    };

    Ok(OAuthCredential {
        refresh: String::new(),
        access: key.to_string(),
        expires: MAX_SAFE_INTEGER,
        extra: Default::default(),
    })
}

/// Upstream `loginOpenRouter` (openrouter.ts:113-163): a one-shot loopback
/// callback on an ephemeral port racing the manual paste; the exchange runs
/// inside the callback handler so the browser page shows its outcome.
async fn login_openrouter(
    oauth: &OpenRouterOAuth,
    interaction: ProviderAuthInteraction,
) -> Result<OAuthCredential, AuthError> {
    let Pkce {
        verifier,
        challenge,
    } = generate_pkce();
    let verifier = Arc::new(verifier);
    // OpenRouter sends no `state`; the random path keeps stray requests from
    // completing the sign-in.
    let exchange_signal = interaction.signal.clone();
    let exchange_verifier = Arc::clone(&verifier);
    let token_url = Arc::new(oauth.token_url.clone());
    let callback: OAuthCallbackServer<OAuthCredential> =
        start_oauth_callback_server(CallbackServerOptions {
            provider_name: "OpenRouter".to_string(),
            host: oauth.callback_host.clone(),
            // Upstream `port: 0` — an ephemeral port.
            port: 0,
            path: format!("/oauth/callback/{}", uuid_v4()),
            redirect_host: None,
            state: None,
            // Upstream `complete: (code) => exchangeAuthorizationCode(code,
            // verifier, interaction.signal)`: the browser page shows the
            // exchange outcome.
            complete: Arc::new(move |code| {
                let token_url = Arc::clone(&token_url);
                let verifier = Arc::clone(&exchange_verifier);
                let signal = exchange_signal.clone();
                Box::pin(async move {
                    exchange_authorization_code(&token_url, &code, &verifier, &signal).await
                })
            }),
            signal: interaction.signal.clone(),
            timeout_ms: Some(u64::try_from(LOGIN_TIMEOUT.as_millis()).expect("timeout fits u64")),
        })
        .await?;

    let result = async {
        let callback_url = callback.redirect_uri().to_string();
        // `new URL(AUTHORIZE_URL)` + `search = new URLSearchParams(...)` —
        // insertion order preserved, form-urlencoded serialization. Built
        // (and the serializer dropped) before any await: the serializer is
        // not `Send` and the login future must be.
        let auth_url = {
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            query.append_pair("callback_url", &callback_url);
            query.append_pair("code_challenge", &challenge);
            query.append_pair("code_challenge_method", "S256");
            format!("{AUTHORIZE_URL}?{}", query.finish())
        };
        interaction.notify(AuthEvent::Progress {
            message: format!("Listening for OpenRouter OAuth callback on {callback_url}"),
        });
        interaction.notify(AuthEvent::AuthUrl {
            url: auth_url,
            instructions: Some(
                "Complete sign-in in your browser. If the browser is on another machine, paste the final redirect URL here."
                    .to_string(),
            ),
        });

        match wait_for_callback_or_manual_input(
            &interaction,
            Some(&callback),
            "Complete sign-in in your browser, or paste the authorization code / redirect URL here:",
            callback.redirect_uri(),
        )
        .await?
        {
            // The exchange already ran inside the callback handler.
            CallbackOrManual::Callback(credential) => Ok(credential),
            CallbackOrManual::Manual(input) => {
                let code = parse_authorization_input(&input);
                let Some(code) = code else {
                    return Err(AuthError::Operation(
                        "Missing authorization code".to_string(),
                    ));
                };
                interaction.notify(AuthEvent::Progress {
                    message: "Exchanging authorization code for an API key...".to_string(),
                });
                exchange_authorization_code(
                    &oauth.token_url,
                    &code,
                    &verifier,
                    &interaction.signal,
                )
                .await
            }
        }
    }
    .await;

    // Upstream `finally { callback.close() }`.
    callback.close().await;
    result
}

impl OAuthAuth for OpenRouterOAuth {
    /// Upstream `name` (openrouter.ts:302).
    fn name(&self) -> &str {
        "OpenRouter OAuth"
    }

    /// Upstream `loginLabel` (openrouter.ts:303).
    fn login_label(&self) -> Option<&str> {
        Some("Sign in with OpenRouter")
    }

    /// Upstream `login` (openrouter.ts:304).
    fn login<'a>(
        &'a self,
        interaction: ProviderAuthInteraction,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(login_openrouter(self, interaction))
    }

    /// Upstream `refresh` (openrouter.ts:305-307): a permanent key is
    /// returned unchanged.
    fn refresh<'a>(
        &'a self,
        credential: OAuthCredential,
        _options: &'a crate::ai::auth::types::AuthOperationOptions,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move { Ok(credential) })
    }

    /// Upstream `toAuth` (openrouter.ts:308-310): `{ apiKey: credential.access }`.
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
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use futures::future::BoxFuture;
    use sha2::{Digest, Sha256};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_util::sync::CancellationToken;
    use url::Url;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::ai::auth::oauth::pkce::base64url_encode;
    use crate::ai::auth::types::{AuthInteraction, AuthOperationOptions};
    use crate::ai::auth::types::{AuthPrompt, AuthPromptKind};

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
        fake_interaction_with_slot(Arc::new(Mutex::new(None)), respond)
    }

    fn fake_interaction_with_slot(
        auth_url: Arc<Mutex<Option<String>>>,
        respond: Respond,
    ) -> (Arc<FakeInteraction>, ProviderAuthInteraction) {
        let fake = Arc::new(FakeInteraction {
            auth_url,
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

    /// Never answers; blocks on its prompt signal like a real pending UI
    /// prompt, so the callback-server path can win the race.
    fn hanging_interaction() -> (Arc<FakeInteraction>, ProviderAuthInteraction) {
        fake_interaction(Box::new(|prompt| {
            Box::pin(async move {
                prompt.signal.unwrap_or_default().cancelled().await;
                Err(AuthError::Cancelled)
            })
        }))
    }

    /// Answers the manual prompt with `{callback_url}?{query}`, reading the
    /// callback URL out of the emitted auth URL like a user pasting the
    /// final redirect.
    fn pasted_redirect_interaction(
        query: Arc<dyn Fn(&str) -> String + Send + Sync>,
    ) -> (Arc<FakeInteraction>, ProviderAuthInteraction) {
        let slot = Arc::new(Mutex::new(None));
        let respond_slot = Arc::clone(&slot);
        fake_interaction_with_slot(
            slot,
            Box::new(move |prompt| {
                let slot = Arc::clone(&respond_slot);
                let query = Arc::clone(&query);
                Box::pin(async move {
                    match &prompt.kind {
                        AuthPromptKind::ManualCode { .. } => {
                            let auth_url = slot.lock().unwrap().clone().expect("auth_url emitted");
                            let callback_url = Url::parse(&auth_url)
                                .unwrap()
                                .query_pairs()
                                .find(|(key, _)| key == "callback_url")
                                .map(|(_, value)| value.into_owned())
                                .expect("missing callback_url parameter");
                            Ok(format!("{}?{}", callback_url, query(&callback_url)))
                        }
                        other => panic!("unexpected prompt: {other:?}"),
                    }
                })
            }),
        )
    }

    /// Answers the manual prompt with a fixed string.
    fn instant_interaction(
        answer: &'static str,
    ) -> (Arc<FakeInteraction>, ProviderAuthInteraction) {
        fake_interaction(Box::new(move |prompt| {
            Box::pin(async move {
                match &prompt.kind {
                    AuthPromptKind::ManualCode { .. } => Ok(answer.to_string()),
                    other => panic!("unexpected prompt: {other:?}"),
                }
            })
        }))
    }

    /// A notify that fails the test — login must emit nothing.
    fn silent_notify_interaction() -> (Arc<FakeInteraction>, ProviderAuthInteraction) {
        let (fake, interaction) =
            fake_interaction(Box::new(|_prompt| Box::pin(async { Ok(String::new()) })));
        // Wrap notify by panicking: reuse the fake but assert emptiness later.
        (fake, interaction)
    }

    fn flow_with(server: &MockServer, callback_host: &str) -> OpenRouterOAuth {
        OpenRouterOAuth::with_endpoints(
            format!("{}/api/v1/auth/keys", server.uri()),
            callback_host.to_string(),
        )
    }

    async fn mount_exchange(server: &MockServer, body: &str, status: u16) {
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/keys"))
            .respond_with(
                ResponseTemplate::new(status).set_body_raw(body.to_string(), "application/json"),
            )
            .mount(server)
            .await;
    }

    async fn http_get(port: u16, target: &str) -> String {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        stream
            .write_all(
                format!("GET {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
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

    fn auth_url_of(interaction: &FakeInteraction) -> String {
        interaction
            .auth_url
            .lock()
            .unwrap()
            .clone()
            .expect("login must emit an auth_url event")
    }

    fn auth_url_param(auth_url: &str, name: &str) -> String {
        Url::parse(auth_url)
            .unwrap()
            .query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
            .expect("missing auth URL parameter")
    }

    /// Waits for the auth_url event and fires the browser callback:
    /// GET `{callback_url}?code={code}`, returning the raw HTTP response.
    async fn http_get_callback(slot: Arc<Mutex<Option<String>>>, code: &'static str) -> String {
        let target = tokio::spawn(async move {
            loop {
                if let Some(auth_url) = slot.lock().unwrap().clone() {
                    let callback_url = auth_url_param(&auth_url, "callback_url");
                    let parsed = Url::parse(&callback_url).unwrap();
                    let path = parsed.path().to_string();
                    let query = parsed.query().unwrap_or("").to_string();
                    let target = if query.is_empty() {
                        format!("{path}?code={code}")
                    } else {
                        format!("{path}?{query}&code={code}")
                    };
                    break (parsed.port().unwrap(), target);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        http_get(target.0, &target.1).await
    }

    // ---- Oracle ports (packages/ai/test/openrouter-oauth.test.ts) ----

    /// Oracle: "runs PKCE on a one-shot loopback callback and exchanges the
    /// code for a permanent API key".
    #[tokio::test]
    async fn runs_pkce_on_a_one_shot_loopback_callback_and_exchanges_the_code() {
        let server = MockServer::start().await;
        mount_exchange(&server, r#"{"key":"sk-or-test"}"#, 200).await;
        let oauth = flow_with(&server, "127.0.0.1");
        let (fake, interaction) = hanging_interaction();

        let driver = tokio::spawn(http_get_callback(
            Arc::clone(&fake.auth_url),
            "authorization-code",
        ));

        let credential = tokio::time::timeout(Duration::from_secs(10), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap();
        let response = driver.await.unwrap();

        // The permanent credential: empty refresh, MAX_SAFE_INTEGER expiry.
        assert_eq!(credential.access, "sk-or-test");
        assert_eq!(credential.refresh, "");
        assert_eq!(credential.expires, 9_007_199_254_740_991);

        // The callback got the success page.
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("Content-Type: text/html; charset=utf-8"));
        assert!(response.contains("<h1>Authentication successful</h1>"));
        assert!(response.contains("Signed in to OpenRouter."));

        // The manual prompt was aborted by the finally block.
        assert!(fake.prompts.lock().unwrap()[0]
            .signal
            .as_ref()
            .unwrap()
            .is_cancelled());

        // Authorize URL shape.
        let auth_url = auth_url_of(&fake);
        let parsed = Url::parse(&auth_url).unwrap();
        assert_eq!(parsed.scheme(), "https");
        assert_eq!(parsed.host_str(), Some("openrouter.ai"));
        assert_eq!(parsed.path(), "/auth");
        assert_eq!(auth_url_param(&auth_url, "code_challenge_method"), "S256");

        // Callback URL shape: loopback host and a UUID callback path.
        let callback_url = auth_url_param(&auth_url, "callback_url");
        let parsed = Url::parse(&callback_url).unwrap();
        assert_eq!(parsed.host_str(), Some("127.0.0.1"));
        assert!(parsed.path().starts_with("/oauth/callback/"));
        let uuid = parsed.path().trim_start_matches("/oauth/callback/");
        assert_eq!(uuid.len(), 36);
        assert!(uuid
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-'));

        // Exactly one exchange, carrying the code, the method and the PKCE
        // verifier whose S256 is the authorize URL's challenge.
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["code"], "authorization-code");
        assert_eq!(body["code_challenge_method"], "S256");
        let verifier = body["code_verifier"].as_str().unwrap();
        assert_eq!(
            auth_url_param(&auth_url, "code_challenge"),
            base64url_encode(&Sha256::digest(verifier.as_bytes()))
        );

        // Progress events: the listening banner, then the auth URL.
        let events = fake.events.lock().unwrap().clone();
        assert!(matches!(
            events.first(),
            Some(AuthEvent::Progress { message })
                if message.starts_with("Listening for OpenRouter OAuth callback on http://127.0.0.1:")
        ));
        assert!(matches!(events.get(1), Some(AuthEvent::AuthUrl { .. })));
    }

    /// Oracle: "reports token exchange failures through both the callback
    /// page and login".
    #[tokio::test]
    async fn reports_token_exchange_failures_through_both_the_callback_page_and_login() {
        let server = MockServer::start().await;
        mount_exchange(&server, r#"{"error":{"message":"invalid code"}}"#, 403).await;
        let oauth = flow_with(&server, "127.0.0.1");
        let (fake, interaction) = hanging_interaction();

        let login = tokio::spawn(async move { oauth.login(interaction).await });
        let response = http_get_callback(Arc::clone(&fake.auth_url), "bad-code").await;

        let error = tokio::time::timeout(Duration::from_secs(10), login)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation(
                "OpenRouter OAuth key exchange failed (HTTP 403): invalid code".to_string()
            )
        );
        assert!(response.starts_with("HTTP/1.1 502 Bad Gateway\r\n"));
        // The shared server's page header is `{providerName} sign-in failed.`
        // (callback-server.ts); the exchange error text arrives as the detail.
        assert!(response.contains("OpenRouter sign-in failed."));
        assert!(response.contains("invalid code"));
        // Upstream `error.message`: the bare message reaches the page, never
        // the port's AuthError Display prefix.
        assert!(!response.contains("auth operation failed:"));
    }

    /// Oracle: "allows only one token exchange for a callback" — a second
    /// callback while the first exchange is in flight gets 409 and the first
    /// still completes.
    #[tokio::test]
    async fn allows_only_one_token_exchange_for_a_callback() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/keys"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(r#"{"key":"sk-or-test"}"#.to_string(), "application/json")
                    .set_delay(Duration::from_millis(500)),
            )
            .expect(1)
            .mount(&server)
            .await;
        let oauth = flow_with(&server, "127.0.0.1");
        let (fake, interaction) = hanging_interaction();

        let login = tokio::spawn(async move { oauth.login(interaction).await });
        let first = tokio::spawn(http_get_callback(
            Arc::clone(&fake.auth_url),
            "authorization-code",
        ));
        // Let the first exchange start before the second callback arrives.
        tokio::time::sleep(Duration::from_millis(150)).await;
        let second = http_get_callback(Arc::clone(&fake.auth_url), "second-code").await;

        assert!(second.starts_with("HTTP/1.1 409 Conflict\r\n"));
        assert!(second.contains("This sign-in has already been handled."));

        let credential = tokio::time::timeout(Duration::from_secs(10), login)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(credential.access, "sk-or-test");
        assert!(first.await.unwrap().starts_with("HTTP/1.1 200 OK\r\n"));

        // Only the first callback exchanged a code.
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["code"], "authorization-code");
    }

    /// Oracle: "rejects a successful response that does not contain a key".
    #[tokio::test]
    async fn rejects_a_successful_response_that_does_not_contain_a_key() {
        let server = MockServer::start().await;
        mount_exchange(&server, r#"{"user_id":"user-1"}"#, 200).await;
        let oauth = flow_with(&server, "127.0.0.1");
        let (fake, interaction) = hanging_interaction();

        let login = tokio::spawn(async move { oauth.login(interaction).await });
        let response = http_get_callback(Arc::clone(&fake.auth_url), "code-without-key").await;

        let error = tokio::time::timeout(Duration::from_secs(10), login)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("OpenRouter OAuth response carries no \"key\"".to_string())
        );
        assert!(response.starts_with("HTTP/1.1 502 Bad Gateway\r\n"));
    }

    /// Oracle: "mints a key from a pasted redirect URL when the loopback
    /// callback never arrives".
    #[tokio::test]
    async fn mints_a_key_from_a_pasted_redirect_url_when_the_loopback_callback_never_arrives() {
        let server = MockServer::start().await;
        mount_exchange(&server, r#"{"key":"sk-or-manual"}"#, 200).await;
        let oauth = flow_with(&server, "127.0.0.1");
        let (fake, interaction) =
            pasted_redirect_interaction(Arc::new(|_| "code=manual-code".to_string()));

        let credential = oauth.login(interaction).await.unwrap();

        assert_eq!(credential.access, "sk-or-manual");
        assert_eq!(credential.refresh, "");
        assert_eq!(credential.expires, 9_007_199_254_740_991);
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["code"], "manual-code");
        assert_eq!(body["code_challenge_method"], "S256");
        // The manual path announces the exchange.
        let events = fake.events.lock().unwrap().clone();
        assert!(events.iter().any(|event| matches!(
            event,
            AuthEvent::Progress { message }
                if message == "Exchanging authorization code for an API key..."
        )));
    }

    /// Oracle: "accepts a bare authorization code from the manual prompt".
    #[tokio::test]
    async fn accepts_a_bare_authorization_code_from_the_manual_prompt() {
        let server = MockServer::start().await;
        mount_exchange(&server, r#"{"key":"sk-or-manual"}"#, 200).await;
        let oauth = flow_with(&server, "127.0.0.1");
        let (_fake, interaction) = instant_interaction("  manual-code  ");

        let credential = oauth.login(interaction).await.unwrap();

        assert_eq!(credential.access, "sk-or-manual");
        let requests = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["code"], "manual-code");
    }

    /// Oracle: "fails login when the manual prompt is cancelled" — no
    /// exchange request is made.
    #[tokio::test]
    async fn fails_login_when_the_manual_prompt_is_cancelled() {
        let server = MockServer::start().await;
        let oauth = flow_with(&server, "127.0.0.1");
        let (_fake, interaction) = fake_interaction(Box::new(|_prompt| {
            Box::pin(async { Err(AuthError::Cancelled) })
        }));

        let result = oauth.login(interaction).await;
        assert_eq!(result, Err(AuthError::Cancelled));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    /// Oracle: "rejects empty manual input without exchanging a code".
    #[tokio::test]
    async fn rejects_empty_manual_input_without_exchanging_a_code() {
        let server = MockServer::start().await;
        let oauth = flow_with(&server, "127.0.0.1");
        let (_fake, interaction) = instant_interaction("   ");

        let error = oauth.login(interaction).await.unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("Missing authorization code".to_string())
        );
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    /// Oracle: "closes the pending callback when login is cancelled" — the
    /// callback port stops accepting after the interaction signal fires.
    #[tokio::test]
    async fn closes_the_pending_callback_when_login_is_cancelled() {
        let server = MockServer::start().await;
        let oauth = flow_with(&server, "127.0.0.1");
        let (fake, interaction) = hanging_interaction();
        let signal = interaction.signal.clone();
        let slot = Arc::clone(&fake.auth_url);
        let driver = tokio::spawn(async move {
            let callback_url = loop {
                if let Some(auth_url) = slot.lock().unwrap().clone() {
                    break auth_url_param(&auth_url, "callback_url");
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            };
            signal.cancel();
            let port = Url::parse(&callback_url).unwrap().port().unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            port
        });

        let result = tokio::time::timeout(Duration::from_secs(10), oauth.login(interaction))
            .await
            .unwrap();
        assert_eq!(result.unwrap_err(), AuthError::Cancelled);
        let port = driver.await.unwrap();
        // The callback server is closed: the port no longer accepts.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_err(),
            "callback server must be closed after cancellation"
        );
    }

    /// Oracle: "rejects before opening a callback server when login is
    /// already cancelled" — no events may be emitted.
    #[tokio::test]
    async fn rejects_before_opening_a_callback_server_when_login_is_already_cancelled() {
        let server = MockServer::start().await;
        let oauth = flow_with(&server, "127.0.0.1");
        // The oracle's notify throws on any event; the port asserts the same
        // by failing the test if any event or prompt is recorded.
        let (fake, interaction) = silent_notify_interaction();
        interaction.signal.cancel();

        let result = oauth.login(interaction).await;
        assert_eq!(result.unwrap_err(), AuthError::Cancelled);
        assert!(fake.events.lock().unwrap().is_empty());
        assert!(fake.prompts.lock().unwrap().is_empty());
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    /// Oracle: "uses the configured OAuth callback host" — the injected
    /// host (production: `PI_OAUTH_CALLBACK_HOST`) is what the callback URL
    /// reports and what the server binds.
    #[tokio::test]
    async fn uses_the_configured_oauth_callback_host() {
        let server = MockServer::start().await;
        let oauth = flow_with(&server, "localhost");
        let (fake, interaction) = hanging_interaction();
        let signal = interaction.signal.clone();
        let slot = Arc::clone(&fake.auth_url);
        let driver = tokio::spawn(async move {
            let callback_url = loop {
                if let Some(auth_url) = slot.lock().unwrap().clone() {
                    break auth_url_param(&auth_url, "callback_url");
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            };
            signal.cancel();
            callback_url
        });

        let result = oauth.login(interaction).await;
        assert_eq!(result.unwrap_err(), AuthError::Cancelled);
        let callback_url = driver.await.unwrap();
        assert_eq!(
            Url::parse(&callback_url).unwrap().host_str(),
            Some("localhost")
        );
    }

    // ---- parse_authorization_input (upstream openrouter.ts:52-67) ----

    #[test]
    fn parse_authorization_input_follows_the_upstream_branches() {
        // A pasted redirect URL: the query `code` wins.
        assert_eq!(
            parse_authorization_input("http://127.0.0.1:8080/oauth/callback/x?code=abc"),
            Some("abc".to_string())
        );
        // A parseable URL without a code yields None (no fall-through).
        assert_eq!(
            parse_authorization_input("http://127.0.0.1:8080/oauth/callback/x"),
            None
        );
        // A bare query string, with or without a leading `?`.
        assert_eq!(
            parse_authorization_input("?code=x&state=y"),
            Some("x".to_string())
        );
        assert_eq!(
            parse_authorization_input("code=a&code=b"),
            Some("a".to_string())
        );
        // Bare codes, trimmed; empty input.
        assert_eq!(
            parse_authorization_input("the-code"),
            Some("the-code".to_string())
        );
        assert_eq!(
            parse_authorization_input("  the-code  "),
            Some("the-code".to_string())
        );
        assert_eq!(parse_authorization_input(""), None);
        assert_eq!(parse_authorization_input("   "), None);
        // A name merely containing "code=" matches nothing.
        assert_eq!(parse_authorization_input("xcode=y"), None);
    }

    // ---- Metadata and units ----

    #[tokio::test]
    async fn metadata_and_to_auth_match_upstream() {
        let oauth = OpenRouterOAuth::new();
        assert_eq!(oauth.name(), "OpenRouter OAuth");
        assert_eq!(oauth.login_label(), Some("Sign in with OpenRouter"));
        assert!(!oauth.is_subscription());
        let auth = oauth
            .to_auth(OAuthCredential {
                refresh: String::new(),
                access: "sk-or-key".to_string(),
                expires: 0,
                extra: Default::default(),
            })
            .await
            .unwrap();
        assert_eq!(
            auth,
            ModelAuth {
                api_key: Some("sk-or-key".to_string()),
                headers: None,
                base_url: None,
            }
        );
    }

    #[tokio::test]
    async fn refresh_returns_the_credential_unchanged() {
        let oauth = OpenRouterOAuth::new();
        let credential = OAuthCredential {
            refresh: String::new(),
            access: "sk-or-key".to_string(),
            expires: 9_007_199_254_740_991,
            extra: Default::default(),
        };
        let refreshed = oauth
            .refresh(credential.clone(), &AuthOperationOptions::default())
            .await
            .unwrap();
        assert_eq!(refreshed, credential);
    }

    #[test]
    fn production_endpoints_match_upstream() {
        assert_eq!(AUTHORIZE_URL, "https://openrouter.ai/auth");
        assert_eq!(TOKEN_URL, "https://openrouter.ai/api/v1/auth/keys");
        assert_eq!(LOGIN_TIMEOUT, Duration::from_millis(5 * 60 * 1000));
        assert_eq!(TOKEN_EXCHANGE_TIMEOUT, Duration::from_secs(30));
        assert_eq!(DEFAULT_CALLBACK_HOST, "127.0.0.1");
        assert_eq!(MAX_SAFE_INTEGER, 9_007_199_254_740_991);
    }
}
