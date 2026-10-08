//! Anthropic OAuth flow (Claude Pro/Max) ported from upstream
//! `packages/ai/src/auth/oauth/anthropic.ts`: the PKCE authorization-URL
//! build, the local redirect-capture server, the manual-code fallback, the
//! token exchange/refresh requests, and the [`AnthropicOAuth`] [`OAuthAuth`]
//! implementation.
//!
//! Interactive surface: the browser is pointed at the authorize URL through
//! the [`AuthEvent::AuthUrl`] event and the user's pasted redirect URL /
//! authorization code arrives through a `manual_code` prompt (M2d ruling) —
//! the flow never touches stdio. The local callback server is flow-owned
//! infrastructure, like upstream's `http.createServer`.
//!
//! Port notes (disclosed divergences):
//! - The token endpoint URL and callback port are fields on
//!   [`AnthropicOAuth`] (upstream: module constants) so tests can point the
//!   flow at a wiremock server and free ports. The production constructor
//!   pins the upstream values.
//! - Token request JSON bodies serialize with serde_json's map ordering
//!   (alphabetical) instead of upstream's `JSON.stringify` insertion order;
//!   OAuth token endpoints parse JSON, so the wire semantics are unchanged.
//! - `formatErrorDetails` (upstream lines 82-97) cannot see Node-only error
//!   fields (`code`/`errno`/`stack`); the port formats `Error: <display>`
//!   plus the `cause=<display>` chain of [`std::error::Error::source`].
//! - The flow keeps the port-wide pre-flight cancellation guard
//!   (`Err(Cancelled)` before any work when the interaction signal already
//!   fired). Upstream instead starts the callback server (whose entry check
//!   throws "Login cancelled", swallowed by `.catch(() => undefined)`) and
//!   continues into a manual prompt that an aborted interaction can never
//!   answer.
//! - The token response parse is stricter than upstream: a 200 response
//!   missing `access_token`/`refresh_token`/`expires_in` errors as invalid
//!   JSON, where upstream would silently build a credential with `undefined`
//!   fields. (Deliberate: the port never stores a corrupt credential.)

use std::sync::Arc;

use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use crate::ai::api::azure_openai_responses::get_provider_env_value;
use crate::ai::api::http_client;
use crate::ai::auth::types::{
    AuthError, AuthEvent, AuthInteraction, AuthPrompt, AuthPromptKind, AuthPromptOption, ModelAuth,
    OAuthAuth, OAuthCredential, ProviderAuthInteraction,
};
use crate::ai::now_ms;

use super::callback_server::{
    start_oauth_callback_server, wait_for_callback_or_manual_input, CallbackOrManual,
    CallbackServerOptions, OAuthCallbackServer,
};
use super::parse_authorization_input;
use super::pkce::{generate_pkce, Pkce};

/// Upstream `CLIENT_ID` (anthropic.ts:29): upstream decodes
/// `atob("OWQxYzI1MGEtZTYxYi00NGQ5LTg4ZWQtNTk0NGQxOTYyZjVl")` at module load;
/// the port stores the decoded value (pinned by a test).
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

/// Upstream `AUTHORIZE_URL` (anthropic.ts:30).
const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";

/// Upstream `TOKEN_URL` (anthropic.ts:31); the production default for
/// [`AnthropicOAuth::new`].
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";

/// Upstream `getProviderEnvValue("PI_OAUTH_CALLBACK_HOST")` (anthropic.ts:32).
const CALLBACK_HOST_ENV: &str = "PI_OAUTH_CALLBACK_HOST";

/// Upstream `|| "127.0.0.1"` fallback for the callback host.
const DEFAULT_CALLBACK_HOST: &str = "127.0.0.1";

/// Upstream `CALLBACK_PORT` (anthropic.ts:33).
const CALLBACK_PORT: u16 = 53692;

/// Upstream `CALLBACK_PATH` (anthropic.ts:34).
const CALLBACK_PATH: &str = "/callback";

/// Upstream `COPY_CODE_REDIRECT_URI` (v1.0.0): the headless login's redirect,
/// whose code the user copies from the Anthropic page.
const COPY_CODE_REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";

/// Upstream `ANTHROPIC_BROWSER_LOGIN_METHOD` (v1.0.0).
const ANTHROPIC_BROWSER_LOGIN_METHOD: &str = "browser";

/// Upstream `ANTHROPIC_COPY_CODE_LOGIN_METHOD` (v1.0.0).
const ANTHROPIC_COPY_CODE_LOGIN_METHOD: &str = "copy_code";

/// Upstream `SCOPES` (anthropic.ts:36-37).
const SCOPES: &str = "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";

/// Upstream `AbortSignal.timeout(30_000)` composed into every token request
/// (anthropic.ts:178).
const POST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Upstream `REDIRECT_URI` (anthropic.ts:35): the redirect host is always
/// `localhost`; the server binds `CALLBACK_HOST` separately.
fn redirect_uri(callback_port: u16) -> String {
    format!("http://localhost:{callback_port}{CALLBACK_PATH}")
}

/// The Anthropic OAuth auth surface (upstream `anthropicOAuth`, lines
/// 355-364). [`AnthropicOAuth::new`] pins the upstream endpoints; tests
/// inject a wiremock token URL and a free callback port.
pub struct AnthropicOAuth {
    token_url: String,
    callback_host: String,
    callback_port: u16,
}

impl Default for AnthropicOAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl AnthropicOAuth {
    /// Upstream module constants, with the callback host resolved from
    /// `PI_OAUTH_CALLBACK_HOST` (default `127.0.0.1`), like the upstream
    /// module-load read.
    pub fn new() -> Self {
        let callback_host = get_provider_env_value(CALLBACK_HOST_ENV, None)
            .unwrap_or_else(|| DEFAULT_CALLBACK_HOST.to_string());
        AnthropicOAuth {
            token_url: TOKEN_URL.to_string(),
            callback_host,
            callback_port: CALLBACK_PORT,
        }
    }

    /// Test constructor: point the token endpoint at a stub server and use a
    /// port that does not collide with other tests (upstream tests stub the
    /// global `fetch` and keep port 53692 because they run sequentially).
    #[cfg(test)]
    pub(crate) fn with_endpoints(
        token_url: String,
        callback_host: String,
        callback_port: u16,
    ) -> Self {
        AnthropicOAuth {
            token_url,
            callback_host,
            callback_port,
        }
    }
}

/// Failures of [`post_json`]. `Cancelled` never reaches the upstream-style
/// wrappers: the port surfaces cancellation as [`AuthError::Cancelled`].
enum PostJsonError {
    Cancelled,
    /// `fetch` rejection: transport or timeout.
    Transport(reqwest::Error),
    /// Non-2xx response: upstream
    /// `HTTP request failed. status=…; url=…; body=…`.
    Status(String),
}

impl PostJsonError {
    /// Upstream `formatErrorDetails(error)` of the thrown error: an `Error`
    /// renders as `Error: <message>`.
    fn details(&self) -> String {
        match self {
            PostJsonError::Transport(error) => format_error_details(error),
            PostJsonError::Status(message) => format!("Error: {message}"),
            PostJsonError::Cancelled => String::new(),
        }
    }
}

/// Upstream `formatErrorDetails` (anthropic.ts:82-97) over a Rust error
/// chain: `Error: <display>` plus `cause=<display>` per source. Node-only
/// fields (`code`, `errno`, `stack`) have no Rust counterpart.
fn format_error_details(error: &(dyn std::error::Error + 'static)) -> String {
    let mut details = vec![format!("Error: {error}")];
    let mut source = error.source();
    while let Some(cause) = source {
        details.push(format!("cause={cause}"));
        source = cause.source();
    }
    details.join("; ")
}

/// Upstream `postJson` (anthropic.ts:170-188): POST the JSON body with
/// `Content-Type`/`Accept: application/json` and a 30s budget, returning the
/// raw response text (status errors carry the body in the message).
async fn post_json(
    token_url: &str,
    body: serde_json::Value,
    signal: &CancellationToken,
) -> Result<String, PostJsonError> {
    let body = serde_json::to_string(&body).expect("token request body must serialize");
    let request = http_client()
        .post(token_url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .body(body)
        .timeout(POST_TIMEOUT);

    let response = tokio::select! {
        biased;
        _ = signal.cancelled() => return Err(PostJsonError::Cancelled),
        response = request.send() => match response {
            Ok(response) => response,
            Err(error) => return Err(PostJsonError::Transport(error)),
        },
    };
    let status = response.status();
    let response_body = tokio::select! {
        biased;
        _ = signal.cancelled() => return Err(PostJsonError::Cancelled),
        body = response.text() => match body {
            Ok(body) => body,
            Err(error) => return Err(PostJsonError::Transport(error)),
        },
    };

    // `if (!response.ok)`: `as_u16()` matches the upstream
    // `status=${response.status}` interpolation (a bare number).
    if !status.is_success() {
        return Err(PostJsonError::Status(format!(
            "HTTP request failed. status={}; url={token_url}; body={response_body}",
            status.as_u16()
        )));
    }
    Ok(response_body)
}

/// The token endpoint response fields (upstream exchange/refresh parse).
#[derive(serde::Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
}

/// Upstream `expires: Date.now() + expires_in * 1000 - 5 * 60 * 1000`
/// (anthropic.ts:230, 351): shave five minutes for clock skew.
fn credential_from_tokens(tokens: TokenResponse) -> OAuthCredential {
    OAuthCredential {
        refresh: tokens.refresh_token,
        access: tokens.access_token,
        expires: now_ms() + tokens.expires_in * 1000 - 5 * 60 * 1000,
        extra: Default::default(),
    }
}

/// Upstream `exchangeAuthorizationCode` (anthropic.ts:190-232).
async fn exchange_authorization_code(
    token_url: &str,
    code: &str,
    state: &str,
    verifier: &str,
    redirect_uri: &str,
    signal: &CancellationToken,
) -> Result<OAuthCredential, AuthError> {
    let body = serde_json::json!({
        "grant_type": "authorization_code",
        "client_id": CLIENT_ID,
        "code": code,
        "state": state,
        "redirect_uri": redirect_uri,
        "code_verifier": verifier,
    });

    let response_body = match post_json(token_url, body, signal).await {
        Ok(response_body) => response_body,
        Err(PostJsonError::Cancelled) => return Err(AuthError::Cancelled),
        Err(error) => {
            return Err(AuthError::Operation(format!(
                "Token exchange request failed. url={token_url}; redirect_uri={redirect_uri}; \
                 response_type=authorization_code; details={}",
                error.details()
            )));
        }
    };

    match serde_json::from_str::<TokenResponse>(&response_body) {
        Ok(tokens) => Ok(credential_from_tokens(tokens)),
        Err(error) => Err(AuthError::Operation(format!(
            "Token exchange returned invalid JSON. url={token_url}; body={response_body}; \
             details=Error: {error}"
        ))),
    }
}

/// Upstream `refreshAnthropicToken` (anthropic.ts:317-353): the refresh
/// request carries no scope (pinned by the oracle).
async fn refresh_anthropic_token(
    token_url: &str,
    refresh_token: &str,
    signal: &CancellationToken,
) -> Result<OAuthCredential, AuthError> {
    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "client_id": CLIENT_ID,
        "refresh_token": refresh_token,
    });

    let response_body = match post_json(token_url, body, signal).await {
        Ok(response_body) => response_body,
        Err(PostJsonError::Cancelled) => return Err(AuthError::Cancelled),
        Err(error) => {
            return Err(AuthError::Operation(format!(
                "Anthropic token refresh request failed. url={token_url}; details={}",
                error.details()
            )));
        }
    };

    match serde_json::from_str::<TokenResponse>(&response_body) {
        Ok(tokens) => Ok(credential_from_tokens(tokens)),
        Err(error) => Err(AuthError::Operation(format!(
            "Anthropic token refresh returned invalid JSON. url={token_url}; body={response_body}; \
             details=Error: {error}"
        ))),
    }
}

/// Upstream `loginAnthropicCopyCode` (v1.0.0): headless login — publish the
/// copy-code authorize URL, read the pasted `code#state`, then exchange with
/// the copy-code redirect URI.
async fn login_anthropic_copy_code(
    token_url: &str,
    interaction: ProviderAuthInteraction,
) -> Result<OAuthCredential, AuthError> {
    if interaction.signal.is_cancelled() {
        return Err(AuthError::Cancelled);
    }
    let Pkce {
        verifier,
        challenge,
    } = generate_pkce();
    let auth_url = {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("code", "true");
        query.append_pair("client_id", CLIENT_ID);
        query.append_pair("response_type", "code");
        query.append_pair("redirect_uri", COPY_CODE_REDIRECT_URI);
        query.append_pair("scope", SCOPES);
        query.append_pair("code_challenge", &challenge);
        query.append_pair("code_challenge_method", "S256");
        query.append_pair("state", &verifier);
        format!("{AUTHORIZE_URL}?{}", query.finish())
    };
    interaction.notify(AuthEvent::AuthUrl {
        url: auth_url,
        instructions: Some(
            "Complete login in your browser, then copy the code Anthropic shows and paste it here."
                .to_string(),
        ),
    });

    let input = interaction
        .prompt(AuthPrompt {
            signal: Some(interaction.signal.clone()),
            kind: AuthPromptKind::ManualCode {
                message: "Paste the code Anthropic shows after you sign in:".to_string(),
                placeholder: Some("code#state".to_string()),
            },
        })
        .await?;
    let parsed = parse_authorization_input(&input);
    if parsed
        .state
        .as_deref()
        .is_some_and(|state| !state.is_empty() && state != verifier)
    {
        return Err(AuthError::Operation("OAuth state mismatch".to_string()));
    }
    let Some(code) = parsed.code.filter(|code| !code.is_empty()) else {
        return Err(AuthError::Operation(
            "Missing authorization code".to_string(),
        ));
    };
    interaction.notify(AuthEvent::Progress {
        message: "Exchanging authorization code for tokens...".to_string(),
    });
    exchange_authorization_code(
        token_url,
        &code,
        &parsed.state.unwrap_or_else(|| verifier.clone()),
        &verifier,
        COPY_CODE_REDIRECT_URI,
        &interaction.signal,
    )
    .await
}

/// Upstream `loginAnthropic` (anthropic.ts:134-191): open the shared
/// callback server (a failed bind degrades to manual-only login through
/// `.catch(() => undefined)`), publish the authorize URL, race the manual
/// paste against the callback, then exchange the code.
async fn login_anthropic(
    token_url: &str,
    callback_host: &str,
    callback_port: u16,
    interaction: ProviderAuthInteraction,
) -> Result<OAuthCredential, AuthError> {
    // Port divergence (module port notes): the pre-flight cancellation guard.
    if interaction.signal.is_cancelled() {
        return Err(AuthError::Cancelled);
    }
    let redirect_uri = redirect_uri(callback_port);
    let Pkce {
        verifier,
        challenge,
    } = generate_pkce();
    // `.catch(() => undefined)`: a taken port means manual-paste-only
    // sign-in, not a failed login.
    let callback: Option<OAuthCallbackServer<String>> =
        start_oauth_callback_server(CallbackServerOptions {
            provider_name: "Anthropic".to_string(),
            host: callback_host.to_string(),
            port: callback_port,
            path: CALLBACK_PATH.to_string(),
            // The shared server's redirectUri is unused here: the flow keeps
            // its own `localhost` REDIRECT_URI for the authorize URL and the
            // exchange (upstream anthropic.ts:35).
            redirect_host: None,
            state: Some(verifier.clone()),
            // Upstream `complete: async (code) => code`: the page reports
            // success immediately and the exchange happens after the wait.
            complete: Arc::new(|code| Box::pin(async move { Ok(code) })),
            signal: interaction.signal.clone(),
            timeout_ms: None,
        })
        .await
        .ok();

    let result = async {
        // `new URLSearchParams({...}).toString()` — insertion order preserved,
        // form-urlencoded serialization (spaces become `+`). Built (and the
        // serializer dropped) before any await: the serializer is not `Send`
        // and the login future must be.
        let auth_url = {
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            query.append_pair("code", "true");
            query.append_pair("client_id", CLIENT_ID);
            query.append_pair("response_type", "code");
            query.append_pair("redirect_uri", &redirect_uri);
            query.append_pair("scope", SCOPES);
            query.append_pair("code_challenge", &challenge);
            query.append_pair("code_challenge_method", "S256");
            query.append_pair("state", &verifier);
            format!("{AUTHORIZE_URL}?{}", query.finish())
        };
        interaction.notify(AuthEvent::AuthUrl {
            url: auth_url,
            instructions: Some(
                "Complete login in your browser. If the browser is on another machine, paste the \
                 final redirect URL here."
                    .to_string(),
            ),
        });

        let result = wait_for_callback_or_manual_input(
            &interaction,
            callback.as_ref(),
            "Complete login in your browser, or paste the authorization code / redirect URL here:",
            &redirect_uri,
        )
        .await?;
        let (code, state) = match result {
            // The callback server validated the state against the verifier.
            CallbackOrManual::Callback(code) => (code, verifier.clone()),
            CallbackOrManual::Manual(input) => {
                let parsed = parse_authorization_input(&input);
                // `if (parsed.state && parsed.state !== verifier)` — JS
                // truthiness on the state, then a mismatch.
                if parsed
                    .state
                    .as_deref()
                    .is_some_and(|state| !state.is_empty() && state != verifier)
                {
                    return Err(AuthError::Operation("OAuth state mismatch".to_string()));
                }
                // `state = parsed.state ?? verifier` — nullish, so an empty
                // pasted state is kept.
                let state = parsed.state.unwrap_or_else(|| verifier.clone());
                (parsed.code.unwrap_or_default(), state)
            }
        };

        // Upstream truthiness check (`if (!code)`); the old "Missing OAuth
        // state" error is gone with the refactor.
        if code.is_empty() {
            return Err(AuthError::Operation(
                "Missing authorization code".to_string(),
            ));
        }

        interaction.notify(AuthEvent::Progress {
            message: "Exchanging authorization code for tokens...".to_string(),
        });
        exchange_authorization_code(
            token_url,
            &code,
            &state,
            &verifier,
            &redirect_uri,
            &interaction.signal,
        )
        .await
    }
    .await;

    // Upstream `finally { callback?.close() }`.
    if let Some(callback) = callback {
        callback.close().await;
    }
    result
}

impl OAuthAuth for AnthropicOAuth {
    /// Upstream `name` (anthropic.ts:356).
    fn name(&self) -> &str {
        "Anthropic (Claude Pro/Max)"
    }

    /// Upstream `isSubscription: true` (anthropic.ts:357).
    fn is_subscription(&self) -> bool {
        true
    }

    /// Upstream `login` (v1.0.0): pick the login method, then run the browser
    /// flow or the headless copy-code flow.
    fn login<'a>(
        &'a self,
        interaction: ProviderAuthInteraction,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move {
            // Pre-flight cancellation guard: upstream's select prompt carries
            // the interaction signal, so an already-cancelled login rejects
            // at the prompt; the port checks before recording it.
            if interaction.signal.is_cancelled() {
                return Err(AuthError::Cancelled);
            }
            let method = interaction
                .prompt(AuthPrompt {
                    signal: Some(interaction.signal.clone()),
                    kind: AuthPromptKind::Select {
                        message: "Select Anthropic login method:".to_string(),
                        options: vec![
                            AuthPromptOption {
                                id: ANTHROPIC_BROWSER_LOGIN_METHOD.to_string(),
                                label: "Browser login (default)".to_string(),
                                description: None,
                            },
                            AuthPromptOption {
                                id: ANTHROPIC_COPY_CODE_LOGIN_METHOD.to_string(),
                                label: "Copy code login (headless)".to_string(),
                                description: None,
                            },
                        ],
                    },
                })
                .await?;
            if method == ANTHROPIC_COPY_CODE_LOGIN_METHOD {
                return login_anthropic_copy_code(&self.token_url, interaction).await;
            }
            if method != ANTHROPIC_BROWSER_LOGIN_METHOD {
                return Err(AuthError::Operation(format!(
                    "Unknown Anthropic login method: {method}"
                )));
            }
            login_anthropic(
                &self.token_url,
                &self.callback_host,
                self.callback_port,
                interaction,
            )
            .await
        })
    }

    /// Upstream `refresh` (anthropic.ts:359).
    fn refresh<'a>(
        &'a self,
        credential: OAuthCredential,
        options: &'a crate::ai::auth::types::AuthOperationOptions,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move {
            let signal = options.signal.clone().unwrap_or_default();
            refresh_anthropic_token(&self.token_url, &credential.refresh, &signal).await
        })
    }

    /// Upstream `toAuth` (anthropic.ts:361-363): `{ apiKey: credential.access }`.
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
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::ai::auth::oauth::pkce::base64url_encode;
    use crate::ai::auth::types::{
        AuthInteraction, AuthOperationOptions, AuthPrompt, AuthPromptKind,
    };

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

    /// Answers immediately with a fixed string (the oracle tests' prompt).
    fn instant_interaction(
        answer: &'static str,
    ) -> (Arc<FakeInteraction>, ProviderAuthInteraction) {
        fake_interaction_with_slot(
            Arc::new(Mutex::new(None)),
            Box::new(move |_prompt| Box::pin(async move { Ok(answer.to_string()) })),
        )
    }

    /// v1.0.0: login starts with the method select. This fake answers the
    /// select with 'browser' and every later prompt with .
    fn browser_interaction(
        answer: &'static str,
    ) -> (Arc<FakeInteraction>, ProviderAuthInteraction) {
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count_move = Arc::clone(&count);
        fake_interaction_with_slot(
            Arc::new(Mutex::new(None)),
            Box::new(move |prompt| {
                let count = Arc::clone(&count_move);
                Box::pin(async move {
                    if count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                        let AuthPromptKind::Select { options, .. } = prompt.kind else {
                            panic!("expected the method select first, got {:?}", prompt.kind);
                        };
                        let browser = options
                            .iter()
                            .find(|option| option.id == ANTHROPIC_BROWSER_LOGIN_METHOD)
                            .expect("browser option");
                        return Ok(browser.id.clone());
                    }
                    Ok(answer.to_string())
                })
            }),
        )
    }
    /// Never answers; blocks on its prompt signal like a real pending UI
    /// prompt, so the callback-server path can win the race.
    fn hanging_interaction() -> (Arc<FakeInteraction>, ProviderAuthInteraction) {
        fake_interaction_with_slot(
            Arc::new(Mutex::new(None)),
            Box::new(move |prompt| {
                // v1.0.0: the first prompt is the login-method select; answer
                // it and hang on the later prompts like a real pending UI.
                if let AuthPromptKind::Select { options, .. } = prompt.kind {
                    if let Some(browser) = options
                        .iter()
                        .find(|option| option.id == ANTHROPIC_BROWSER_LOGIN_METHOD)
                    {
                        let id = browser.id.clone();
                        return Box::pin(async move { Ok(id) })
                            as BoxFuture<'static, Result<String, AuthError>>;
                    }
                }
                Box::pin(async move {
                    prompt.signal.unwrap_or_default().cancelled().await;
                    Err(AuthError::Cancelled)
                })
            }),
        )
    }

    /// Answers with `{redirect_uri}?{query(state)}`, reading the state and the
    /// redirect URI out of the emitted auth URL like a user pasting the final
    /// redirect. `port` must match the flow's callback port.
    fn redirect_url_interaction(
        port: u16,
        query_for_state: Arc<dyn Fn(&str) -> String + Send + Sync>,
    ) -> (Arc<FakeInteraction>, ProviderAuthInteraction) {
        let slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let respond_slot = Arc::clone(&slot);
        fake_interaction_with_slot(
            slot,
            Box::new(move |prompt| {
                let slot = Arc::clone(&respond_slot);
                let query_for_state = Arc::clone(&query_for_state);
                Box::pin(async move {
                    // v1.0.0: the first prompt is the login-method select.
                    if let AuthPromptKind::Select { options, .. } = prompt.kind {
                        let browser = options
                            .iter()
                            .find(|option| option.id == ANTHROPIC_BROWSER_LOGIN_METHOD)
                            .expect("browser option");
                        return Ok(browser.id.clone());
                    }
                    let auth_url = slot.lock().unwrap().clone().expect("auth_url emitted");
                    let url = Url::parse(&auth_url).unwrap();
                    let pair = |name: &str| {
                        url.query_pairs()
                            .find(|(key, _)| key == name)
                            .map(|(_, value)| value.into_owned())
                            .expect("missing auth URL parameter")
                    };
                    let state = pair("state");
                    let redirect_uri = pair("redirect_uri");
                    assert_eq!(
                        redirect_uri,
                        format!("http://localhost:{port}/callback"),
                        "the redirect_uri parameter must keep the localhost form"
                    );
                    Ok(format!("{redirect_uri}?{}", query_for_state(&state)))
                })
            }),
        )
    }

    fn free_callback_port() -> u16 {
        std::net::TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn flow_with(server: &MockServer, port: u16) -> AnthropicOAuth {
        AnthropicOAuth::with_endpoints(
            format!("{}/v1/oauth/token", server.uri()),
            "127.0.0.1".to_string(),
            port,
        )
    }

    async fn mount_token_endpoint(server: &MockServer, body: &str, status: u16, expected: u64) {
        Mock::given(method("POST"))
            .and(path("/v1/oauth/token"))
            .and(header("content-type", "application/json"))
            .and(header("accept", "application/json"))
            .respond_with(
                ResponseTemplate::new(status).set_body_raw(body.to_string(), "application/json"),
            )
            .expect(expected)
            .mount(server)
            .await;
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

    fn token_body(access: &str, refresh: &str) -> String {
        format!(r#"{{"access_token":"{access}","refresh_token":"{refresh}","expires_in":3600}}"#)
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

    // ---- Oracle ports (packages/ai/test/anthropic-oauth.test.ts) ----

    /// Oracle: "keeps the localhost redirect_uri for manual callback login".
    #[tokio::test]
    async fn login_resolves_through_the_manual_prompt_keeping_the_localhost_redirect_uri() {
        let server = MockServer::start().await;
        mount_token_endpoint(
            &server,
            &token_body("access-token", "refresh-token"),
            200,
            1,
        )
        .await;
        let port = free_callback_port();
        let oauth = flow_with(&server, port);
        let (fake, interaction) = redirect_url_interaction(
            port,
            Arc::new(|state| format!("code=manual-code&state={state}")),
        );

        let credential = oauth.login(interaction).await.unwrap();

        assert_eq!(credential.access, "access-token");
        assert_eq!(credential.refresh, "refresh-token");
        // expires = now + 3600s - 5min (upstream anthropic.ts:230).
        assert!((credential.expires - (now_ms() + 3_300_000)).abs() <= 2_000);

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        let state = auth_url_param(&auth_url_of(&fake), "state");
        assert_eq!(body["grant_type"], "authorization_code");
        assert_eq!(body["code"], "manual-code");
        assert_eq!(
            body["redirect_uri"],
            format!("http://localhost:{port}/callback")
        );
        assert_eq!(body["state"], state);
        // The state is the PKCE verifier; the exchange replays it.
        assert_eq!(body["code_verifier"], state);
        assert_eq!(body["client_id"], CLIENT_ID);
    }

    /// Oracle: "omits scope from refresh token requests".
    #[tokio::test]
    async fn refresh_requests_carry_only_the_refresh_fields_without_scope() {
        let server = MockServer::start().await;
        mount_token_endpoint(
            &server,
            r#"{"access_token":"new-access-token","refresh_token":"new-refresh-token","expires_in":3600,"scope":"user:profile"}"#,
            200,
            1,
        )
        .await;
        let oauth = flow_with(&server, free_callback_port());

        let credential = oauth
            .refresh(
                OAuthCredential {
                    refresh: "refresh-token".to_string(),
                    access: "old-access-token".to_string(),
                    expires: 0,
                    extra: Default::default(),
                },
                &AuthOperationOptions::default(),
            )
            .await
            .unwrap();

        assert_eq!(credential.access, "new-access-token");
        assert_eq!(credential.refresh, "new-refresh-token");
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        let body = body.as_object().unwrap();
        assert_eq!(body.len(), 3);
        assert_eq!(body["grant_type"], "refresh_token");
        assert_eq!(body["client_id"], CLIENT_ID);
        assert_eq!(body["refresh_token"], "refresh-token");
        assert!(body.get("scope").is_none());
    }

    /// Oracle: "anthropicOAuth.login resolves through the manual_code prompt
    /// and aborts it after settling".
    #[tokio::test]
    async fn login_resolves_through_the_manual_code_prompt_and_aborts_it_after_settling() {
        let server = MockServer::start().await;
        mount_token_endpoint(&server, &token_body("access", "refresh"), 200, 1).await;
        let port = free_callback_port();
        let oauth = flow_with(&server, port);
        let (fake, interaction) = browser_interaction("the-code");

        let credential = oauth.login(interaction).await.unwrap();

        assert_eq!(credential.access, "access");
        assert_eq!(credential.refresh, "refresh");
        let events = fake.events.lock().unwrap().clone();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AuthEvent::AuthUrl { .. })),
            "login must emit an auth_url event: {events:?}"
        );
        assert!(
            events.iter().any(|event| matches!(
                event,
                AuthEvent::Progress { message }
                    if message == "Exchanging authorization code for tokens..."
            )),
            "login must emit the exchange progress event: {events:?}"
        );
        let prompts = fake.prompts.lock().unwrap();
        // The method select precedes the manual prompt (v1.0.0).
        assert_eq!(prompts.len(), 2);
        assert!(matches!(&prompts[0].kind, AuthPromptKind::Select { .. }));
        let AuthPromptKind::ManualCode {
            message,
            placeholder,
        } = &prompts[1].kind
        else {
            panic!("expected a manual_code prompt, got {:?}", prompts[1].kind);
        };
        assert_eq!(
            message,
            "Complete login in your browser, or paste the authorization code / redirect URL here:"
        );
        assert_eq!(
            placeholder.as_deref(),
            Some(&format!("http://localhost:{port}/callback") as &str)
        );
        // The prompt's signal is aborted once login settles, so UIs can
        // dismiss it.
        assert!(prompts[1].signal.as_ref().unwrap().is_cancelled());
    }

    // ---- Flow details ----

    #[test]
    fn production_endpoints_match_upstream() {
        // atob("OWQxYzI1MGEtZTYxYi00NGQ5LTg4ZWQtNTk0NGQxOTYyZjVl")
        assert_eq!(CLIENT_ID, "9d1c250a-e61b-44d9-88ed-5944d1962f5e");
        assert_eq!(AUTHORIZE_URL, "https://claude.ai/oauth/authorize");
        assert_eq!(TOKEN_URL, "https://platform.claude.com/v1/oauth/token");
        assert_eq!(DEFAULT_CALLBACK_HOST, "127.0.0.1");
        assert_eq!(CALLBACK_PORT, 53692);
        assert_eq!(CALLBACK_PATH, "/callback");
        assert_eq!(
            redirect_uri(CALLBACK_PORT),
            "http://localhost:53692/callback"
        );
        assert_eq!(
            SCOPES,
            "org:create_api_key user:profile user:inference user:sessions:claude_code \
             user:mcp_servers user:file_upload"
        );
        assert_eq!(POST_TIMEOUT, Duration::from_secs(30));
    }

    #[tokio::test]
    async fn auth_url_carries_the_pkce_parameters_in_upstream_order() {
        let server = MockServer::start().await;
        mount_token_endpoint(&server, &token_body("a", "r"), 200, 1).await;
        let port = free_callback_port();
        let oauth = flow_with(&server, port);
        let (fake, interaction) = browser_interaction("any-code");

        oauth.login(interaction).await.unwrap();

        let (url, instructions) = fake
            .events
            .lock()
            .unwrap()
            .iter()
            .find_map(|event| match event {
                AuthEvent::AuthUrl { url, instructions } => {
                    Some((url.clone(), instructions.clone()))
                }
                _ => None,
            })
            .expect("auth_url event");
        assert_eq!(
            instructions.as_deref(),
            Some(
                "Complete login in your browser. If the browser is on another machine, paste the \
                 final redirect URL here."
            )
        );

        let url = Url::parse(&url).unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("claude.ai"));
        assert_eq!(url.path(), "/oauth/authorize");
        let pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        let state = pairs
            .iter()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.clone())
            .unwrap();
        // S256 of the state (= the verifier), base64url without padding.
        let expected_challenge = base64url_encode(&Sha256::digest(state.as_bytes()));
        assert_eq!(
            pairs,
            vec![
                ("code".to_string(), "true".to_string()),
                ("client_id".to_string(), CLIENT_ID.to_string()),
                ("response_type".to_string(), "code".to_string()),
                (
                    "redirect_uri".to_string(),
                    format!("http://localhost:{port}/callback")
                ),
                ("scope".to_string(), SCOPES.to_string()),
                ("code_challenge".to_string(), expected_challenge),
                ("code_challenge_method".to_string(), "S256".to_string()),
                ("state".to_string(), state),
            ]
        );
    }

    #[tokio::test]
    async fn manual_input_with_a_state_mismatch_is_rejected_without_exchanging() {
        let server = MockServer::start().await;
        mount_token_endpoint(&server, "{}", 200, 0).await;
        let port = free_callback_port();
        let oauth = flow_with(&server, port);
        let (fake, interaction) = redirect_url_interaction(
            port,
            Arc::new(|_state| "code=x&state=wrong-state".to_string()),
        );

        let error = oauth.login(interaction).await.unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("OAuth state mismatch".to_string())
        );
        // The method select precedes the manual prompt (v1.0.0).
        assert_eq!(fake.prompts.lock().unwrap().len(), 2);
    }

    /// An empty pasted state is not a mismatch (JS truthiness) and is sent
    /// verbatim as the exchange state (nullish coalescing keeps `""`).
    #[tokio::test]
    async fn empty_pasted_state_is_kept_verbatim() {
        let server = MockServer::start().await;
        mount_token_endpoint(&server, &token_body("a", "r"), 200, 1).await;
        let port = free_callback_port();
        let oauth = flow_with(&server, port);
        let (fake, interaction) =
            redirect_url_interaction(port, Arc::new(|_state| "code=bare-code&state=".to_string()));

        let credential = oauth.login(interaction).await.unwrap();
        assert_eq!(credential.access, "a");
        let requests = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["code"], "bare-code");
        assert_eq!(body["state"], "");
        let _ = auth_url_of(&fake);
    }

    #[tokio::test]
    async fn manual_input_state_defaults_to_the_verifier() {
        let server = MockServer::start().await;
        mount_token_endpoint(&server, &token_body("a", "r"), 200, 1).await;
        let oauth = flow_with(&server, free_callback_port());
        let (fake, interaction) = browser_interaction("bare-code");

        let credential = oauth.login(interaction).await.unwrap();
        assert_eq!(credential.access, "a");
        let verifier = auth_url_param(&auth_url_of(&fake), "state");
        let requests = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["code"], "bare-code");
        assert_eq!(body["state"], verifier);
    }

    #[tokio::test]
    async fn empty_manual_input_reports_a_missing_authorization_code() {
        let server = MockServer::start().await;
        mount_token_endpoint(&server, "{}", 200, 0).await;
        let oauth = flow_with(&server, free_callback_port());
        let (_fake, interaction) = browser_interaction("   ");

        let error = oauth.login(interaction).await.unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("Missing authorization code".to_string())
        );
    }

    #[tokio::test]
    async fn login_completes_through_the_local_callback_server() {
        let server = MockServer::start().await;
        mount_token_endpoint(&server, &token_body("cb-access", "cb-refresh"), 200, 1).await;
        let port = free_callback_port();
        let oauth = flow_with(&server, port);
        let (fake, interaction) = hanging_interaction();
        let auth_url_slot = Arc::clone(&fake.auth_url);

        let driver = tokio::spawn(async move {
            let auth_url = wait_for_auth_url(auth_url_slot).await;
            let state = auth_url_param(&auth_url, "state");
            http_get(port, &format!("/callback?code=cb-code&state={state}")).await
        });

        let credential = tokio::time::timeout(Duration::from_secs(10), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap();
        let response = driver.await.unwrap();

        assert_eq!(credential.access, "cb-access");
        assert_eq!(credential.refresh, "cb-refresh");
        // The shared server's success page (page bytes pinned against the
        // oracle fixture).
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("Content-Type: text/html; charset=utf-8"));
        assert!(response.contains("Cache-Control: no-store"));
        assert!(response.contains("<h1>Authentication successful</h1>"));
        assert!(response.contains("Signed in to Anthropic. You may now close this page."));
        let requests = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["code"], "cb-code");
        // The manual prompt (second: the method select precedes it) was
        // aborted by the finally block.
        assert!(fake.prompts.lock().unwrap()[1]
            .signal
            .as_ref()
            .unwrap()
            .is_cancelled());
    }

    /// A provider-reported redirect error fails the login with the upstream
    /// message (the shared server settles the wait with the error).
    #[tokio::test]
    async fn provider_error_redirect_fails_the_login() {
        let server = MockServer::start().await;
        mount_token_endpoint(&server, "{}", 200, 0).await;
        let port = free_callback_port();
        let oauth = flow_with(&server, port);
        let (fake, interaction) = hanging_interaction();
        let auth_url_slot = Arc::clone(&fake.auth_url);

        let driver = tokio::spawn(async move {
            let auth_url = wait_for_auth_url(auth_url_slot).await;
            let state = auth_url_param(&auth_url, "state");
            http_get(
                port,
                &format!(
                    "/callback?error=access_denied&error_description=User%20said%20no&state={state}"
                ),
            )
            .await
        });

        let error = tokio::time::timeout(Duration::from_secs(10), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap_err();
        let response = driver.await.unwrap();

        assert_eq!(
            error,
            AuthError::Operation("Anthropic authorization failed: User said no".to_string())
        );
        assert!(response.contains("Anthropic authorization failed."));
        assert!(response.contains("User said no"));
        assert_eq!(server.received_requests().await.unwrap().len(), 0);
    }

    /// A failed callback-server bind degrades to manual-only login (upstream
    /// `.catch(() => undefined)`), pinning the old bind failure is an error.
    #[tokio::test]
    async fn taken_callback_port_degrades_to_manual_only_login() {
        let server = MockServer::start().await;
        mount_token_endpoint(&server, &token_body("a", "r"), 200, 1).await;
        let blocker = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = blocker.local_addr().unwrap().port();
        let oauth = flow_with(&server, port);
        let (fake, interaction) = browser_interaction("the-code");

        let credential = tokio::time::timeout(Duration::from_secs(5), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap();
        drop(blocker);
        assert_eq!(credential.access, "a");
        // The method select precedes the manual prompt (v1.0.0).
        assert_eq!(fake.prompts.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn cancelled_interaction_signal_aborts_login_before_the_prompt() {
        let oauth = AnthropicOAuth::with_endpoints(
            "http://127.0.0.1:1/v1/oauth/token".to_string(),
            "127.0.0.1".to_string(),
            free_callback_port(),
        );
        let (fake, _) = instant_interaction("unused");
        let token = CancellationToken::new();
        token.cancel();
        let interaction =
            ProviderAuthInteraction::new(Arc::clone(&fake) as Arc<dyn AuthInteraction>, token);

        let result = oauth.login(interaction).await;
        assert_eq!(result, Err(AuthError::Cancelled));
        assert!(fake.prompts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn interaction_signal_cancelled_mid_login_aborts_the_wait() {
        let oauth = AnthropicOAuth::with_endpoints(
            "http://127.0.0.1:1/v1/oauth/token".to_string(),
            "127.0.0.1".to_string(),
            free_callback_port(),
        );
        let (fake, interaction) = hanging_interaction();
        let signal = interaction.signal.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            signal.cancel();
        });

        let result = tokio::time::timeout(Duration::from_secs(5), oauth.login(interaction))
            .await
            .unwrap();
        assert_eq!(result, Err(AuthError::Cancelled));
        // The method select precedes the manual prompt (v1.0.0); both were
        // shown before the wait was aborted.
        assert_eq!(fake.prompts.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn exchange_http_errors_carry_the_upstream_message_shape() {
        let server = MockServer::start().await;
        mount_token_endpoint(&server, "denied", 400, 1).await;
        let port = free_callback_port();
        let token_url = format!("{}/v1/oauth/token", server.uri());
        let oauth = flow_with(&server, port);
        let (_fake, interaction) = browser_interaction("the-code");

        let error = oauth.login(interaction).await.unwrap_err();
        let redirect = format!("http://localhost:{port}/callback");
        assert_eq!(
            error,
            AuthError::Operation(format!(
                "Token exchange request failed. url={token_url}; redirect_uri={redirect}; \
                 response_type=authorization_code; details=Error: HTTP request failed. \
                 status=400; url={token_url}; body=denied"
            ))
        );
    }

    #[tokio::test]
    async fn exchange_invalid_json_errors_carry_the_body() {
        let server = MockServer::start().await;
        mount_token_endpoint(&server, "not json", 200, 1).await;
        let token_url = format!("{}/v1/oauth/token", server.uri());
        let oauth = flow_with(&server, free_callback_port());
        let (_fake, interaction) = browser_interaction("the-code");

        let error = oauth.login(interaction).await.unwrap_err();
        match error {
            AuthError::Operation(message) => assert!(message.starts_with(
                format!(
                    "Token exchange returned invalid JSON. url={token_url}; body=not json; \
                     details=Error: "
                )
                .as_str()
            )),
            other => panic!("expected an operation error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn refresh_http_errors_carry_the_upstream_message_shape() {
        let server = MockServer::start().await;
        mount_token_endpoint(&server, "nope", 400, 1).await;
        let token_url = format!("{}/v1/oauth/token", server.uri());
        let oauth = flow_with(&server, free_callback_port());

        let error = oauth
            .refresh(
                OAuthCredential {
                    refresh: "r".to_string(),
                    access: "a".to_string(),
                    expires: 0,
                    extra: Default::default(),
                },
                &AuthOperationOptions::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation(format!(
                "Anthropic token refresh request failed. url={token_url}; details=Error: \
                 HTTP request failed. status=400; url={token_url}; body=nope"
            ))
        );
    }

    #[tokio::test]
    async fn refresh_invalid_json_errors_carry_the_body() {
        let server = MockServer::start().await;
        mount_token_endpoint(&server, "not json", 200, 1).await;
        let token_url = format!("{}/v1/oauth/token", server.uri());
        let oauth = flow_with(&server, free_callback_port());

        let error = oauth
            .refresh(
                OAuthCredential {
                    refresh: "r".to_string(),
                    access: "a".to_string(),
                    expires: 0,
                    extra: Default::default(),
                },
                &AuthOperationOptions::default(),
            )
            .await
            .unwrap_err();
        match error {
            AuthError::Operation(message) => assert!(message.starts_with(
                format!(
                    "Anthropic token refresh returned invalid JSON. url={token_url}; \
                     body=not json; details=Error: "
                )
                .as_str()
            )),
            other => panic!("expected an operation error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cancelled_signal_skips_the_refresh_request() {
        let server = MockServer::start().await;
        mount_token_endpoint(&server, "{}", 200, 0).await;
        let oauth = flow_with(&server, free_callback_port());
        let token = CancellationToken::new();
        token.cancel();

        let error = oauth
            .refresh(
                OAuthCredential {
                    refresh: "r".to_string(),
                    access: "a".to_string(),
                    expires: 0,
                    extra: Default::default(),
                },
                &AuthOperationOptions::new(token),
            )
            .await
            .unwrap_err();
        assert_eq!(error, AuthError::Cancelled);
    }

    #[tokio::test]
    async fn to_auth_derives_the_request_api_key_from_the_access_token() {
        let oauth = AnthropicOAuth::new();
        assert_eq!(oauth.name(), "Anthropic (Claude Pro/Max)");
        assert!(oauth.is_subscription());
        assert_eq!(oauth.login_label(), None);
        let auth = oauth
            .to_auth(OAuthCredential {
                refresh: "r".to_string(),
                access: "access-token".to_string(),
                expires: 42,
                extra: Default::default(),
            })
            .await
            .unwrap();
        assert_eq!(
            auth,
            ModelAuth {
                api_key: Some("access-token".to_string()),
                headers: None,
                base_url: None,
            }
        );
    }
}
