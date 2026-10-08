//! OpenAI Codex (ChatGPT OAuth) flow ported from upstream
//! `packages/ai/src/auth/oauth/openai-codex.ts`: the browser login (PKCE
//! authorize URL, local redirect-capture server, manual paste fallback), the
//! device-code login (`/api/accounts/deviceauth`), the token
//! exchange/refresh requests, and the [`OpenAICodexOAuth`] [`OAuthAuth`]
//! implementation.
//!
//! Interactive surface (M2d ruling): a `select` prompt picks the login
//! method; the browser flow publishes the authorize URL via
//! [`AuthEvent::AuthUrl`] and reads the pasted redirect/code through a
//! `manual_code` prompt; the device flow reports the user code via
//! [`AuthEvent::DeviceCode`]. The flow never touches stdio or a browser
//! directly.
//!
//! Port notes (disclosed divergences):
//! - The endpoint URLs and callback port are fields on [`OpenAICodexOAuth`]
//!   (upstream: module constants) so tests can point the flow at a wiremock
//!   server and free ports. The production constructor pins the upstream
//!   values. Unlike the callback port, `REDIRECT_URI` keeps the upstream
//!   `localhost:1455` form everywhere (the authorize `redirect_uri` and the
//!   exchange `redirect_uri` must match server-side, which they do in both
//!   upstream and the port).
//! - Upstream `pollOAuthDeviceCodeFlow` (device-code.ts) is the shared
//!   [`super::device_code::poll_device_code_flow`] engine (the private copy
//!   this module initially carried was absorbed into it); the codex flow
//!   polls immediately (`wait_before_first_poll: false`) with the response's
//!   `interval` and the fixed device-code timeout as the deadline.
//! - Poll deadlines and schedules are measured on tokio's clock
//!   ([`tokio::time::Instant`]): wall-clock in production, pause-able in
//!   tests. Upstream uses `Date.now()`.
//! - Cancellation maps to [`AuthError::Cancelled`] everywhere upstream throws
//!   `Error("Login cancelled")` (port contract: interaction-signal aborts are
//!   never wrapped); the token-refresh request threads
//!   `AuthOperationOptions.signal` the same way — upstream passes the
//!   interaction `AbortSignal` into its `fetch` — so cancellation surfaces as
//!   [`AuthError::Cancelled`] rather than the "token refresh error" wrap.
//! - The browser flow runs on the shared [`super::callback_server`] (upstream
//!   `callback-server.ts`, which this delta refactored the private
//!   `startLocalOAuthServer` onto): wrong path/non-GET → 404 "Callback route
//!   not found.", a state mismatch → 400 "State mismatch." (keeping the
//!   login waiting), a claimed or settled sign-in → 409 "This sign-in has
//!   already been handled.", and the success page reads "Signed in to
//!   OpenAI. You may now close this page.".
//! - A callback-server bind failure degrades to manual-paste-only login
//!   (upstream `.catch(() => undefined)`); the bind error text itself never
//!   surfaces.
//! - Transport failures of the device/user-code and exchange requests carry
//!   the raw transport error text (upstream's `fetch` rejection propagates
//!   un-caught); refresh wraps them as "OpenAI Codex token refresh error: …"
//!   like upstream.
//! - JSON re-serialization in "missing fields"/"invalid … response" messages
//!   uses serde_json's map ordering (alphabetical) instead of the JS
//!   `JSON.stringify` insertion order; raw response bodies in failure
//!   messages are preserved byte-for-byte. A 200 JSON parse failure surfaces
//!   the raw serde error text where upstream surfaces the raw `SyntaxError`.
//! - The token response parse is stricter than upstream in absurd cases
//!   (e.g. a non-string `access_token` errors as missing fields where JS
//!   truthiness might admit it; the port never stores a corrupt credential).
//! - The device `interval` field parses with JS `Number(string)` semantics
//!   (trim, empty string is 0, optional trailing dot) except JS-only hex
//!   literals ("0x10"), which fail as an invalid response here.
//! - `AuthEvent::DeviceCode.interval_seconds` is `u64`, so a fractional
//!   server interval reports truncated (intervals are integral in practice).

use std::collections::BTreeMap;
use std::sync::Arc;

use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use crate::ai::api::azure_openai_responses::get_provider_env_value;
use crate::ai::api::http_client;
use crate::ai::api::openai_codex_responses::{decode_standard_base64, JWT_CLAIM_PATH};
use crate::ai::auth::types::{
    AuthError, AuthEvent, AuthInteraction, AuthPrompt, AuthPromptKind, AuthPromptOption, ModelAuth,
    OAuthAuth, OAuthCredential, ProviderAuthInteraction,
};
use crate::ai::now_ms;

use super::callback_server::{
    start_oauth_callback_server, wait_for_callback_or_manual_input, CallbackOrManual,
    CallbackServerOptions, OAuthCallbackServer,
};
use super::device_code::{poll_device_code_flow, PollOutcome};
use super::parse_authorization_input;
use super::pkce::{generate_pkce, Pkce};

/// Upstream `CLIENT_ID` (openai-codex.ts:26).
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// Upstream `AUTHORIZE_URL` (openai-codex.ts:28).
const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";

/// Upstream `TOKEN_URL` (openai-codex.ts:29); the production default for
/// [`OpenAICodexOAuth::new`].
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";

/// Upstream `REDIRECT_URI` (openai-codex.ts:30): the redirect host is always
/// `localhost`; the server binds `PI_OAUTH_CALLBACK_HOST` (default
/// `127.0.0.1`) separately.
const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";

/// Upstream `DEVICE_USER_CODE_URL` (openai-codex.ts:31).
const DEVICE_USER_CODE_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/usercode";

/// Upstream `DEVICE_TOKEN_URL` (openai-codex.ts:32).
const DEVICE_TOKEN_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/token";

/// Upstream `DEVICE_VERIFICATION_URI` (openai-codex.ts:33).
const DEVICE_VERIFICATION_URI: &str = "https://auth.openai.com/codex/device";

/// Upstream `DEVICE_REDIRECT_URI` (openai-codex.ts:34).
const DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";

/// Upstream `DEVICE_CODE_TIMEOUT_SECONDS` (openai-codex.ts:35).
const DEVICE_CODE_TIMEOUT_SECONDS: u64 = 15 * 60;

/// Upstream `OPENAI_CODEX_BROWSER_LOGIN_METHOD` (openai-codex.ts:36).
const BROWSER_LOGIN_METHOD: &str = "browser";

/// Upstream `OPENAI_CODEX_DEVICE_CODE_LOGIN_METHOD` (openai-codex.ts:37).
const DEVICE_CODE_LOGIN_METHOD: &str = "device_code";

/// Upstream `SCOPE` (openai-codex.ts:38).
const SCOPE: &str = "openid profile email offline_access";

/// Upstream `getProviderEnvValue("PI_OAUTH_CALLBACK_HOST")` (openai-codex.ts:45).
const CALLBACK_HOST_ENV: &str = "PI_OAUTH_CALLBACK_HOST";

/// Upstream `|| "127.0.0.1"` fallback for the callback host.
const DEFAULT_CALLBACK_HOST: &str = "127.0.0.1";

/// Upstream `server.listen(1455, …)` (openai-codex.ts:370).
const CALLBACK_PORT: u16 = 1455;

/// Upstream `url.pathname !== "/auth/callback"` (openai-codex.ts:338).
const CALLBACK_PATH: &str = "/auth/callback";

/// Upstream `originator` parameter default (openai-codex.ts:293).
const ORIGINATOR: &str = "pi";

/// The OpenAI Codex OAuth auth surface (upstream `openaiCodexOAuth`,
/// openai-codex.ts:515-544). [`OpenAICodexOAuth::new`] pins the upstream
/// endpoints; tests inject wiremock URLs and a free callback port.
pub struct OpenAICodexOAuth {
    token_url: String,
    device_user_code_url: String,
    device_token_url: String,
    callback_host: String,
    callback_port: u16,
}

impl Default for OpenAICodexOAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenAICodexOAuth {
    /// Upstream module constants, with the callback host resolved from
    /// `PI_OAUTH_CALLBACK_HOST` (default `127.0.0.1`), like the upstream
    /// `getCallbackHost()` call at server start.
    pub fn new() -> Self {
        let callback_host = get_provider_env_value(CALLBACK_HOST_ENV, None)
            .unwrap_or_else(|| DEFAULT_CALLBACK_HOST.to_string());
        OpenAICodexOAuth {
            token_url: TOKEN_URL.to_string(),
            device_user_code_url: DEVICE_USER_CODE_URL.to_string(),
            device_token_url: DEVICE_TOKEN_URL.to_string(),
            callback_host,
            callback_port: CALLBACK_PORT,
        }
    }

    /// Test constructor: point the endpoints at a stub server and use a
    /// callback port that does not collide with other tests (upstream tests
    /// stub the global `fetch` and keep port 1455 because they run
    /// sequentially).
    #[cfg(test)]
    pub(crate) fn with_endpoints(
        token_url: String,
        device_user_code_url: String,
        device_token_url: String,
        callback_host: String,
        callback_port: u16,
    ) -> Self {
        OpenAICodexOAuth {
            token_url,
            device_user_code_url,
            device_token_url,
            callback_host,
            callback_port,
        }
    }
}

/// Upstream `createState` (openai-codex.ts:66-71): 16 random bytes, hex.
/// The oracle tests inject the capture's deterministic byte stream through
/// [`super::test_entropy`].
fn create_state() -> String {
    let mut bytes = [0u8; 16];
    #[cfg(test)]
    if let Some(injected) = super::test_entropy::take(16) {
        bytes.copy_from_slice(&injected);
        return bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    }
    rand::fill(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

// Upstream `parseAuthorizationInput` lives in `super` (byte-identical to the
// anthropic copy).

/// One poll of the device-token endpoint (the `poll` closure passed to
/// [`super::device_code::poll_device_code_flow`] by [`login_device_code`]).
///
/// - 2xx with `authorization_code` + `code_verifier` →
///   [`PollOutcome::Complete`]
/// - 2xx missing either field → [`PollOutcome::Failed`] (invalid response)
/// - 403/404 → [`PollOutcome::Pending`] regardless of body
/// - error-code `deviceauth_authorization_pending` → pending
/// - error-code `slow_down` → [`PollOutcome::SlowDown`]
/// - anything else → [`PollOutcome::Failed`] with status and raw body
async fn poll_device_auth(
    device_token_url: &str,
    device_auth_id: &str,
    user_code: &str,
    signal: &CancellationToken,
) -> Result<PollOutcome<DeviceTokenSuccess>, AuthError> {
    let body = serde_json::to_string(&serde_json::json!({
        "device_auth_id": device_auth_id,
        "user_code": user_code,
    }))
    .expect("device poll body must serialize");
    let response = match post(device_token_url, "application/json", body, signal).await {
        Ok(response) => response,
        // Upstream's uncaught fetch rejection: the raw error text propagates.
        Err(PostError::Cancelled) => return Err(AuthError::Cancelled),
        Err(PostError::Transport(error)) => {
            return Err(AuthError::Operation(error.to_string()));
        }
    };

    if response.ok {
        let json: serde_json::Value = serde_json::from_str(&response.body)
            .map_err(|error| AuthError::Operation(error.to_string()))?;
        let authorization_code = json
            .get("authorization_code")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty());
        let code_verifier = json
            .get("code_verifier")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty());
        return match (authorization_code, code_verifier) {
            (Some(authorization_code), Some(code_verifier)) => {
                Ok(PollOutcome::Complete(DeviceTokenSuccess {
                    authorization_code: authorization_code.to_string(),
                    code_verifier: code_verifier.to_string(),
                }))
            }
            _ => Ok(PollOutcome::Failed(format!(
                "Invalid OpenAI Codex device auth token response: {}",
                serde_json::to_string(&json).unwrap_or_default()
            ))),
        };
    }

    if response.status == 403 || response.status == 404 {
        return Ok(PollOutcome::Pending);
    }

    // `typeof error === "object" ? error?.code : error` — a null error is
    // object-typed in JS and `null?.code` is undefined, matching the
    // fall-through arm here.
    let error_code: Option<String> = serde_json::from_str::<serde_json::Value>(&response.body)
        .ok()
        .and_then(|json| json.get("error").cloned())
        .and_then(|error| match error {
            serde_json::Value::String(text) => Some(text),
            serde_json::Value::Object(_) => error
                .get("code")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            _ => None,
        });

    if error_code.as_deref() == Some("deviceauth_authorization_pending") {
        return Ok(PollOutcome::Pending);
    }
    if error_code.as_deref() == Some("slow_down") {
        return Ok(PollOutcome::SlowDown(None));
    }

    let detail = if response.body.is_empty() {
        String::new()
    } else {
        format!(": {}", response.body)
    };
    Ok(PollOutcome::Failed(format!(
        "OpenAI Codex device auth failed with status {}{detail}",
        response.status
    )))
}

/// Upstream `startOpenAICodexDeviceAuth` (openai-codex.ts:191-233): request a
/// device user code and validate the response shape (device_auth_id,
/// user_code, numeric interval ≥ 0, JS-`Number`-parsed when a string).
async fn start_device_auth(
    user_code_url: &str,
    signal: &CancellationToken,
) -> Result<DeviceAuthInfo, AuthError> {
    let body = serde_json::to_string(&serde_json::json!({ "client_id": CLIENT_ID }))
        .expect("device user-code body must serialize");
    let response = match post(user_code_url, "application/json", body, signal).await {
        Ok(response) => response,
        // Upstream's uncaught fetch rejection: the raw error text propagates.
        Err(PostError::Cancelled) => return Err(AuthError::Cancelled),
        Err(PostError::Transport(error)) => {
            return Err(AuthError::Operation(error.to_string()));
        }
    };

    if !response.ok {
        if response.status == 404 {
            return Err(AuthError::Operation(
                "OpenAI Codex device code login is not enabled for this server. Use browser \
                 login or verify the server URL."
                    .to_string(),
            ));
        }
        let detail = if response.body.is_empty() {
            String::new()
        } else {
            format!(": {}", response.body)
        };
        return Err(AuthError::Operation(format!(
            "OpenAI Codex device code request failed with status {}{detail}",
            response.status
        )));
    }

    // Upstream `await response.json()`: a parse failure propagates un-wrapped.
    let json: serde_json::Value = serde_json::from_str(&response.body)
        .map_err(|error| AuthError::Operation(error.to_string()))?;
    let interval_seconds = match json.get("interval") {
        Some(serde_json::Value::String(text)) => Some(js_number(text)),
        Some(serde_json::Value::Number(number)) => number.as_f64(),
        _ => None,
    };
    let device_auth_id = json
        .get("device_auth_id")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty());
    let user_code = json
        .get("user_code")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty());
    let interval_valid =
        interval_seconds.is_some_and(|seconds| seconds.is_finite() && seconds >= 0.0);

    match (device_auth_id, user_code, interval_valid) {
        (Some(device_auth_id), Some(user_code), true) => Ok(DeviceAuthInfo {
            device_auth_id: device_auth_id.to_string(),
            user_code: user_code.to_string(),
            interval_seconds: interval_seconds.unwrap_or_default(),
        }),
        _ => Err(AuthError::Operation(format!(
            "Invalid OpenAI Codex device code response: {}",
            serde_json::to_string(&json).unwrap_or_default()
        ))),
    }
}

struct DeviceAuthInfo {
    device_auth_id: String,
    user_code: String,
    interval_seconds: f64,
}

struct DeviceTokenSuccess {
    authorization_code: String,
    code_verifier: String,
}

/// Failures of [`post`]. `Cancelled` never reaches the upstream-style
/// wrappers: the port surfaces cancellation as [`AuthError::Cancelled`].
enum PostError {
    Cancelled,
    /// `fetch` rejection: transport failure (no request timeout — upstream
    /// codex relies on the interaction signal alone).
    Transport(reqwest::Error),
}

struct Response {
    status: u16,
    ok: bool,
    body: String,
    reason: &'static str,
}

/// POST with the given `Content-Type` and body, racing the interaction
/// signal (upstream `fetchWithLoginCancellation`).
async fn post(
    url: &str,
    content_type: &str,
    body: String,
    signal: &CancellationToken,
) -> Result<Response, PostError> {
    let request = http_client()
        .post(url)
        .header("Content-Type", content_type)
        .body(body);

    let response = tokio::select! {
        biased;
        _ = signal.cancelled() => return Err(PostError::Cancelled),
        response = request.send() => match response {
            Ok(response) => response,
            Err(error) => return Err(PostError::Transport(error)),
        },
    };
    let status = response.status();
    let body = tokio::select! {
        biased;
        _ = signal.cancelled() => return Err(PostError::Cancelled),
        body = response.text() => match body {
            Ok(body) => body,
            Err(error) => return Err(PostError::Transport(error)),
        },
    };
    Ok(Response {
        status: status.as_u16(),
        ok: status.is_success(),
        body,
        // `response.statusText`.
        reason: status.canonical_reason().unwrap_or(""),
    })
}

/// JS `Number(string)` semantics for the device interval field: trim, an
/// empty string is 0, a single trailing dot is dropped. JS-only hex literals
/// ("0x10") parse to NaN here (disclosed).
fn js_number(value: &str) -> f64 {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return 0.0;
    }
    let normalized = trimmed.strip_suffix('.').unwrap_or(trimmed);
    normalized.parse::<f64>().unwrap_or(f64::NAN)
}

/// A successful token endpoint response (upstream `OAuthToken`,
/// openai-codex.ts:41). `expires` is epoch millis with the full `expires_in` —
/// codex does not shave a safety margin (unlike the anthropic flow).
struct CodexToken {
    access: String,
    refresh: String,
    expires: i64,
}

/// Upstream `readTokenResponse` (openai-codex.ts:126-147): non-2xx →
/// `OpenAI Codex token {operation} failed ({status}): {body || statusText}`;
/// a 2xx body missing any of the three fields → `… response missing fields`.
async fn read_token_response(response: Response, operation: &str) -> Result<CodexToken, AuthError> {
    if !response.ok {
        // `text || response.statusText`.
        let detail = if response.body.is_empty() {
            response.reason
        } else {
            response.body.as_str()
        };
        return Err(AuthError::Operation(format!(
            "OpenAI Codex token {operation} failed ({}): {detail}",
            response.status
        )));
    }

    // Upstream `await response.json()`: a parse failure propagates un-wrapped.
    let json: serde_json::Value = serde_json::from_str(&response.body)
        .map_err(|error| AuthError::Operation(error.to_string()))?;
    let access = json
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty());
    let refresh = json
        .get("refresh_token")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty());
    let expires_in = json.get("expires_in").and_then(serde_json::Value::as_f64);

    match (access, refresh, expires_in) {
        (Some(access), Some(refresh), Some(expires_in)) => Ok(CodexToken {
            access: access.to_string(),
            refresh: refresh.to_string(),
            expires: now_ms() + (expires_in * 1000.0) as i64,
        }),
        _ => Err(AuthError::Operation(format!(
            "OpenAI Codex token {operation} response missing fields: {}",
            serde_json::to_string(&json).unwrap_or_default()
        ))),
    }
}

/// Upstream `exchangeAuthorizationCode` (openai-codex.ts:149-169): a
/// form-urlencoded POST (unlike the anthropic flow's JSON) with grant_type,
/// client_id, code, code_verifier, redirect_uri — in that order.
async fn exchange_authorization_code(
    token_url: &str,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
    signal: &CancellationToken,
) -> Result<CodexToken, AuthError> {
    // `new URLSearchParams({...}).toString()` — insertion order preserved,
    // form-urlencoded serialization. Built before any await: the serializer
    // is not `Send` and the login future must be.
    let body = {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("grant_type", "authorization_code");
        query.append_pair("client_id", CLIENT_ID);
        query.append_pair("code", code);
        query.append_pair("code_verifier", verifier);
        query.append_pair("redirect_uri", redirect_uri);
        query.finish()
    };
    let response = match post(token_url, "application/x-www-form-urlencoded", body, signal).await {
        Ok(response) => response,
        // Upstream's uncaught fetch rejection: the raw error text propagates.
        Err(PostError::Cancelled) => return Err(AuthError::Cancelled),
        Err(PostError::Transport(error)) => {
            return Err(AuthError::Operation(error.to_string()));
        }
    };
    read_token_response(response, "exchange").await
}

/// Upstream `refreshAccessToken` (openai-codex.ts:171-189): form-urlencoded
/// grant_type/refresh_token/client_id; transport failures wrap as
/// "OpenAI Codex token refresh error: …".
async fn refresh_access_token(
    token_url: &str,
    refresh_token: &str,
    signal: &CancellationToken,
) -> Result<CodexToken, AuthError> {
    let body = {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("grant_type", "refresh_token");
        query.append_pair("refresh_token", refresh_token);
        query.append_pair("client_id", CLIENT_ID);
        query.finish()
    };
    let response = match post(token_url, "application/x-www-form-urlencoded", body, signal).await {
        Ok(response) => response,
        Err(PostError::Cancelled) => return Err(AuthError::Cancelled),
        Err(PostError::Transport(error)) => {
            return Err(AuthError::Operation(format!(
                "OpenAI Codex token refresh error: {error}"
            )));
        }
    };
    read_token_response(response, "refresh").await
}

/// Upstream `getAccountId` (openai-codex.ts:396-401) + `decodeJwt`
/// (103-113): the JWT payload's `https://api.openai.com/auth` claim's
/// `chatgpt_account_id`, when a non-empty string. Reuses the codex API
/// port's `atob` decoder and claim-path constant.
fn get_account_id(access_token: &str) -> Option<String> {
    let parts: Vec<&str> = access_token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let payload = decode_standard_base64(parts[1])?;
    let parsed: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    let account_id = parsed
        .get(JWT_CLAIM_PATH)?
        .get("chatgpt_account_id")?
        .as_str()
        .filter(|value| !value.is_empty())?;
    Some(account_id.to_string())
}

/// Upstream `credentialsFromToken` (openai-codex.ts:403-416): the account id
/// rides in the credential's `accountId` extension field (the port's
/// `extra` map, upstream index signature).
fn credentials_from_token(token: CodexToken) -> Result<OAuthCredential, AuthError> {
    let account_id = get_account_id(&token.access).ok_or_else(|| {
        AuthError::Operation("Failed to extract accountId from token".to_string())
    })?;
    Ok(OAuthCredential {
        refresh: token.refresh,
        access: token.access,
        expires: token.expires,
        extra: BTreeMap::from([(
            "accountId".to_string(),
            serde_json::Value::String(account_id),
        )]),
    })
}

/// Upstream `loginOpenAICodexDeviceCode` (openai-codex.ts:427-443).
async fn login_device_code(
    token_url: &str,
    user_code_url: &str,
    device_token_url: &str,
    interaction: &ProviderAuthInteraction,
) -> Result<OAuthCredential, AuthError> {
    if interaction.signal.is_cancelled() {
        return Err(AuthError::Cancelled);
    }
    let device = start_device_auth(user_code_url, &interaction.signal).await?;
    interaction.notify(AuthEvent::DeviceCode {
        user_code: device.user_code.clone(),
        verification_uri: DEVICE_VERIFICATION_URI.to_string(),
        interval_seconds: Some(device.interval_seconds as u64),
        expires_in_seconds: Some(DEVICE_CODE_TIMEOUT_SECONDS),
    });
    let code = poll_device_code_flow(
        Some(device.interval_seconds),
        Some(DEVICE_CODE_TIMEOUT_SECONDS as f64),
        false,
        &interaction.signal,
        || {
            poll_device_auth(
                device_token_url,
                &device.device_auth_id,
                &device.user_code,
                &interaction.signal,
            )
        },
    )
    .await?;
    credentials_from_token(
        exchange_authorization_code(
            token_url,
            &code.authorization_code,
            &code.code_verifier,
            DEVICE_REDIRECT_URI,
            &interaction.signal,
        )
        .await?,
    )
}

/// Upstream `loginOpenAICodex` (openai-codex.ts:359-403): open the shared
/// callback server on port 1455 (shared with the Codex CLI; when it is
/// taken, the login falls back to the pasted redirect URL), publish the
/// authorize URL, race the manual paste against the callback, then exchange
/// the code.
async fn login_browser(
    token_url: &str,
    callback_host: &str,
    callback_port: u16,
    interaction: &ProviderAuthInteraction,
) -> Result<OAuthCredential, AuthError> {
    // Port divergence (module port notes): the pre-flight cancellation guard.
    if interaction.signal.is_cancelled() {
        return Err(AuthError::Cancelled);
    }
    let Pkce {
        verifier,
        challenge,
    } = generate_pkce();
    let state = create_state();
    // Upstream `.catch(() => undefined)`: a taken port means
    // manual-paste-only sign-in, not a failed login.
    let callback: Option<OAuthCallbackServer<String>> =
        start_oauth_callback_server(CallbackServerOptions {
            provider_name: "OpenAI".to_string(),
            host: callback_host.to_string(),
            port: callback_port,
            path: CALLBACK_PATH.to_string(),
            // The shared server's redirectUri is unused here: the flow keeps
            // the upstream `localhost:1455` REDIRECT_URI for the authorize
            // URL and the exchange.
            redirect_host: None,
            state: Some(state.clone()),
            // Upstream `complete: async (code) => code`: the page reports
            // success immediately and the exchange happens after the wait.
            complete: Arc::new(|code| Box::pin(async move { Ok(code) })),
            signal: interaction.signal.clone(),
            timeout_ms: None,
        })
        .await
        .ok();

    let result = async {
        // `new URL(AUTHORIZE_URL)` + `searchParams.set` — insertion order
        // preserved, form-urlencoded serialization. Built (and the
        // serializer dropped) before any await: the serializer is not `Send`
        // and the login future must be.
        let auth_url = {
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            query.append_pair("response_type", "code");
            query.append_pair("client_id", CLIENT_ID);
            query.append_pair("redirect_uri", REDIRECT_URI);
            query.append_pair("scope", SCOPE);
            query.append_pair("code_challenge", &challenge);
            query.append_pair("code_challenge_method", "S256");
            query.append_pair("state", &state);
            query.append_pair("id_token_add_organizations", "true");
            query.append_pair("codex_cli_simplified_flow", "true");
            query.append_pair("originator", ORIGINATOR);
            format!("{AUTHORIZE_URL}?{}", query.finish())
        };
        interaction.notify(AuthEvent::AuthUrl {
            url: auth_url,
            instructions: Some(
                "A browser window should open. Complete login to finish.".to_string(),
            ),
        });

        let result = wait_for_callback_or_manual_input(
            interaction,
            callback.as_ref(),
            "Complete login in your browser, or paste the authorization code / redirect URL here:",
            REDIRECT_URI,
        )
        .await?;
        let code = match result {
            // The callback server validated the state.
            CallbackOrManual::Callback(code) => code,
            CallbackOrManual::Manual(input) => {
                let parsed = parse_authorization_input(&input);
                // `if (parsed.state && parsed.state !== state)` — JS
                // truthiness on the state, then a mismatch.
                if parsed
                    .state
                    .as_deref()
                    .is_some_and(|past| !past.is_empty() && past != state)
                {
                    return Err(AuthError::Operation("State mismatch".to_string()));
                }
                // Upstream truthiness check (`if (!code)`).
                parsed.code.unwrap_or_default()
            }
        };
        if code.is_empty() {
            return Err(AuthError::Operation(
                "Missing authorization code".to_string(),
            ));
        }

        exchange_authorization_code(
            token_url,
            &code,
            &verifier,
            REDIRECT_URI,
            &interaction.signal,
        )
        .await
        .and_then(credentials_from_token)
    }
    .await;

    // Upstream `finally { callback?.close() }`.
    if let Some(callback) = callback {
        callback.close().await;
    }
    result
}

/// Upstream `login` (openai-codex.ts:519-537): select the method, then
/// dispatch.
async fn login_openai_codex(
    oauth: &OpenAICodexOAuth,
    interaction: ProviderAuthInteraction,
) -> Result<OAuthCredential, AuthError> {
    let method = interaction
        .prompt(AuthPrompt {
            signal: None,
            kind: AuthPromptKind::Select {
                message: "Select OpenAI Codex login method:".to_string(),
                options: vec![
                    AuthPromptOption {
                        id: BROWSER_LOGIN_METHOD.to_string(),
                        label: "Browser login (default)".to_string(),
                        description: None,
                    },
                    AuthPromptOption {
                        id: DEVICE_CODE_LOGIN_METHOD.to_string(),
                        label: "Device code login (headless)".to_string(),
                        description: None,
                    },
                ],
            },
        })
        .await?;

    match method.as_str() {
        DEVICE_CODE_LOGIN_METHOD => {
            login_device_code(
                &oauth.token_url,
                &oauth.device_user_code_url,
                &oauth.device_token_url,
                &interaction,
            )
            .await
        }
        BROWSER_LOGIN_METHOD => {
            login_browser(
                &oauth.token_url,
                &oauth.callback_host,
                oauth.callback_port,
                &interaction,
            )
            .await
        }
        other => Err(AuthError::Operation(format!(
            "Unknown OpenAI Codex login method: {other}"
        ))),
    }
}

impl OAuthAuth for OpenAICodexOAuth {
    /// Upstream `name` (openai-codex.ts:516).
    fn name(&self) -> &str {
        "OpenAI (ChatGPT Plus/Pro)"
    }

    /// Upstream `isSubscription: true` (openai-codex.ts:517).
    fn is_subscription(&self) -> bool {
        true
    }

    /// Upstream `login` (openai-codex.ts:519).
    fn login<'a>(
        &'a self,
        interaction: ProviderAuthInteraction,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(login_openai_codex(self, interaction))
    }

    /// Upstream `refresh` (openai-codex.ts:539).
    fn refresh<'a>(
        &'a self,
        credential: OAuthCredential,
        options: &'a crate::ai::auth::types::AuthOperationOptions,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move {
            let signal = options.signal.clone().unwrap_or_default();
            let token = refresh_access_token(&self.token_url, &credential.refresh, &signal).await?;
            credentials_from_token(token)
        })
    }

    /// Upstream `toAuth` (openai-codex.ts:541-543): `{ apiKey: credential.access }`.
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
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use futures::future::BoxFuture;
    use sha2::{Digest, Sha256};
    use tokio_util::sync::CancellationToken;
    use url::Url;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::ai::auth::oauth::device_code::{SLOW_DOWN_TIMEOUT_MESSAGE, TIMEOUT_MESSAGE};
    use crate::ai::auth::oauth::pkce::base64url_encode;
    use crate::ai::auth::types::{AuthInteraction, AuthOperationOptions};

    // ---- Interaction fakes (mirroring the anthropic flow's test rig) ----

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

    /// Answers the method `select` with `device_code` (the oracle tests'
    /// prompt).
    fn device_code_interaction() -> (Arc<FakeInteraction>, ProviderAuthInteraction) {
        fake_interaction(Box::new(|prompt| {
            Box::pin(async move {
                match &prompt.kind {
                    AuthPromptKind::Select { .. } => Ok(DEVICE_CODE_LOGIN_METHOD.to_string()),
                    other => panic!("unexpected prompt: {other:?}"),
                }
            })
        }))
    }

    /// Answers the method `select` with `browser`, then any later prompt
    /// (the manual-code paste) with a fixed string.
    fn browser_interaction(
        answer: &'static str,
    ) -> (Arc<FakeInteraction>, ProviderAuthInteraction) {
        fake_interaction(Box::new(move |prompt| {
            Box::pin(async move {
                match &prompt.kind {
                    AuthPromptKind::Select { .. } => Ok(BROWSER_LOGIN_METHOD.to_string()),
                    AuthPromptKind::ManualCode { .. } => Ok(answer.to_string()),
                    other => panic!("unexpected prompt: {other:?}"),
                }
            })
        }))
    }

    /// Answers the method `select` with a fixed string (method dispatch
    /// tests).
    fn select_interaction(answer: &'static str) -> (Arc<FakeInteraction>, ProviderAuthInteraction) {
        fake_interaction(Box::new(move |prompt| {
            Box::pin(async move {
                match &prompt.kind {
                    AuthPromptKind::Select { .. } => Ok(answer.to_string()),
                    other => panic!("unexpected prompt: {other:?}"),
                }
            })
        }))
    }

    /// Never answers the manual prompt; blocks on its prompt signal like a
    /// real pending UI prompt, so the callback-server path can win the race.
    fn hanging_interaction() -> (Arc<FakeInteraction>, ProviderAuthInteraction) {
        fake_interaction(Box::new(|prompt| {
            Box::pin(async move {
                match &prompt.kind {
                    AuthPromptKind::Select { .. } => Ok(BROWSER_LOGIN_METHOD.to_string()),
                    AuthPromptKind::ManualCode { .. } => {
                        prompt.signal.unwrap_or_default().cancelled().await;
                        Err(AuthError::Cancelled)
                    }
                    other => panic!("unexpected prompt: {other:?}"),
                }
            })
        }))
    }

    /// Answers the select with `browser` and the manual prompt with the
    /// authorize URL's `redirect_uri` parameter plus `{query(state)}`, like a
    /// user pasting the final redirect URL.
    fn redirect_url_interaction(
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
                    match &prompt.kind {
                        AuthPromptKind::Select { .. } => Ok(BROWSER_LOGIN_METHOD.to_string()),
                        AuthPromptKind::ManualCode { .. } => {
                            let auth_url = slot.lock().unwrap().clone().expect("auth_url emitted");
                            let url = Url::parse(&auth_url).unwrap();
                            let pair = |name: &str| {
                                url.query_pairs()
                                    .find(|(key, _)| key == name)
                                    .map(|(_, value)| value.into_owned())
                                    .expect("missing auth URL parameter")
                            };
                            // The redirect_uri keeps the upstream
                            // localhost:1455 form even though the test's
                            // callback server binds another port (the
                            // constant is pinned in the constants test).
                            assert_eq!(pair("redirect_uri"), REDIRECT_URI);
                            Ok(format!(
                                "{}?{}",
                                pair("redirect_uri"),
                                query_for_state(&pair("state"))
                            ))
                        }
                        other => panic!("unexpected prompt: {other:?}"),
                    }
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

    fn flow_with(server: &MockServer, port: u16) -> OpenAICodexOAuth {
        OpenAICodexOAuth::with_endpoints(
            format!("{}/oauth/token", server.uri()),
            format!("{}/api/accounts/deviceauth/usercode", server.uri()),
            format!("{}/api/accounts/deviceauth/token", server.uri()),
            "127.0.0.1".to_string(),
            port,
        )
    }

    async fn mount_json(server: &MockServer, route: &'static str, body: &str, status: u16) {
        Mock::given(method("POST"))
            .and(path(route))
            .respond_with(
                ResponseTemplate::new(status).set_body_raw(body.to_string(), "application/json"),
            )
            .mount(server)
            .await;
    }

    /// Serves the queued `(status, content-type, body)` responses in order —
    /// the wiremock analog of the oracle's `pollResponses.shift()`. A poll
    /// past the queue gets a 500, failing the test loudly.
    async fn mount_json_queue(
        server: &MockServer,
        route: &'static str,
        responses: Vec<(u16, &'static str, String)>,
    ) {
        let queue = Arc::new(Mutex::new(VecDeque::from(responses)));
        Mock::given(method("POST"))
            .and(path(route))
            .respond_with(move |_request: &wiremock::Request| {
                let mut queue = queue.lock().unwrap();
                let (status, content_type, body) = queue.pop_front().unwrap_or_else(|| {
                    (500, "application/json", "unexpected extra poll".to_string())
                });
                ResponseTemplate::new(status).set_body_raw(body, content_type)
            })
            .mount(server)
            .await;
    }

    fn device_auth_pending_response() -> String {
        r#"{"error":{"message":"Device authorization is pending. Please try again.","type":"invalid_request_error","param":null,"code":"deviceauth_authorization_pending"}}"#.to_string()
    }

    fn device_code_ok(authorization_code: &str, code_verifier: &str) -> String {
        format!(
            r#"{{"authorization_code":"{authorization_code}","code_challenge":"device-code-challenge","code_verifier":"{code_verifier}"}}"#
        )
    }

    // ---- JWT test helper (oracle createAccessToken) ----

    fn b64(data: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
            let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
            let group = (b0 << 16) | (b1 << 8) | b2;
            out.push(ALPHABET[(group >> 18) as usize & 63] as char);
            out.push(ALPHABET[(group >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 {
                ALPHABET[(group >> 6) as usize & 63] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                ALPHABET[group as usize & 63] as char
            } else {
                '='
            });
        }
        out
    }

    fn create_access_token(account_id: &str) -> String {
        let header = b64(br#"{"alg":"none"}"#);
        let payload = b64(format!(
            r#"{{"{JWT_CLAIM_PATH}":{{"chatgpt_account_id":"{account_id}"}}}}"#
        )
        .as_bytes());
        format!("{header}.{payload}.signature")
    }

    fn oauth_credential(access: &str, refresh: &str) -> OAuthCredential {
        OAuthCredential {
            refresh: refresh.to_string(),
            access: access.to_string(),
            expires: 0,
            extra: Default::default(),
        }
    }

    async fn http_get(port: u16, target: &str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
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

    fn device_code_event_count(interaction: &FakeInteraction) -> usize {
        interaction
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| matches!(event, AuthEvent::DeviceCode { .. }))
            .count()
    }

    // ---- Oracle ports (packages/ai/test/openai-codex-oauth.test.ts) ----

    /// Oracle: "logs in with the OpenAI Codex device code flow" (real-time:
    /// one pending 403 poll, then completion on a 1s interval).
    #[tokio::test]
    async fn logs_in_with_the_openai_codex_device_code_flow() {
        let server = MockServer::start().await;
        let access_token = create_access_token("account-123");
        mount_json(
            &server,
            "/api/accounts/deviceauth/usercode",
            r#"{"device_auth_id":"device-auth-id","user_code":"ABCD-1234","interval":"1"}"#,
            200,
        )
        .await;
        mount_json_queue(
            &server,
            "/api/accounts/deviceauth/token",
            vec![
                (403, "application/json", device_auth_pending_response()),
                (
                    200,
                    "application/json",
                    device_code_ok("oauth-code", "device-code-verifier"),
                ),
            ],
        )
        .await;
        mount_json(
            &server,
            "/oauth/token",
            &format!(
                r#"{{"access_token":{},"refresh_token":"refresh-token","expires_in":3600}}"#,
                serde_json::to_string(&access_token).unwrap()
            ),
            200,
        )
        .await;
        let oauth = flow_with(&server, free_callback_port());
        let (fake, interaction) = device_code_interaction();

        let credential = oauth.login(interaction).await.unwrap();

        assert_eq!(credential.access, access_token);
        assert_eq!(credential.refresh, "refresh-token");
        // expires = token response time + full expires_in (no 5-minute shave).
        assert!((credential.expires - (now_ms() + 3_600_000)).abs() <= 2_000);
        assert_eq!(
            credential.extra.get("accountId").and_then(|v| v.as_str()),
            Some("account-123")
        );

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 4);
        // Byte-pinned request bodies (upstream JSON.stringify / URLSearchParams).
        assert_eq!(
            String::from_utf8(requests[0].body.clone()).unwrap(),
            r#"{"client_id":"app_EMoamEEZ73f0CkXaXp7hrann"}"#
        );
        assert_eq!(
            String::from_utf8(requests[1].body.clone()).unwrap(),
            r#"{"device_auth_id":"device-auth-id","user_code":"ABCD-1234"}"#
        );
        assert_eq!(
            String::from_utf8(requests[3].body.clone()).unwrap(),
            "grant_type=authorization_code&client_id=app_EMoamEEZ73f0CkXaXp7hrann&\
             code=oauth-code&code_verifier=device-code-verifier&\
             redirect_uri=https%3A%2F%2Fauth.openai.com%2Fdeviceauth%2Fcallback"
        );

        // The device_code event carries the upstream payload.
        let events = fake.events.lock().unwrap().clone();
        assert_eq!(
            events,
            vec![AuthEvent::DeviceCode {
                user_code: "ABCD-1234".to_string(),
                verification_uri: DEVICE_VERIFICATION_URI.to_string(),
                interval_seconds: Some(1),
                expires_in_seconds: Some(DEVICE_CODE_TIMEOUT_SECONDS),
            }]
        );
    }

    /// Oracle: "offers browser login first and uses the selected OpenAI Codex
    /// device code flow".
    #[tokio::test]
    async fn offers_browser_login_first_and_uses_the_selected_device_code_flow() {
        let server = MockServer::start().await;
        let access_token = create_access_token("account-456");
        mount_json(
            &server,
            "/api/accounts/deviceauth/usercode",
            r#"{"device_auth_id":"device-auth-id","user_code":"WXYZ-7890","interval":"5"}"#,
            200,
        )
        .await;
        mount_json(
            &server,
            "/api/accounts/deviceauth/token",
            &device_code_ok("oauth-code", "device-code-verifier"),
            200,
        )
        .await;
        mount_json(
            &server,
            "/oauth/token",
            &format!(
                r#"{{"access_token":{},"refresh_token":"refresh-token","expires_in":3600}}"#,
                serde_json::to_string(&access_token).unwrap()
            ),
            200,
        )
        .await;
        let oauth = flow_with(&server, free_callback_port());
        let (fake, interaction) = device_code_interaction();

        let credential = oauth.login(interaction).await.unwrap();

        assert_eq!(credential.refresh, "refresh-token");
        assert_eq!(
            credential.extra.get("accountId").and_then(|v| v.as_str()),
            Some("account-456")
        );
        // Browser login must not start: only the select prompt and the
        // device_code event.
        let prompts = fake.prompts.lock().unwrap();
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].signal.is_none());
        assert_eq!(
            prompts[0].kind,
            AuthPromptKind::Select {
                message: "Select OpenAI Codex login method:".to_string(),
                options: vec![
                    AuthPromptOption {
                        id: BROWSER_LOGIN_METHOD.to_string(),
                        label: "Browser login (default)".to_string(),
                        description: None,
                    },
                    AuthPromptOption {
                        id: DEVICE_CODE_LOGIN_METHOD.to_string(),
                        label: "Device code login (headless)".to_string(),
                        description: None,
                    },
                ],
            }
        );
        assert!(fake.auth_url.lock().unwrap().is_none());
        assert_eq!(device_code_event_count(&fake), 1);
    }

    /// Oracle: "cancels when OpenAI Codex login method selection is
    /// cancelled" (the port surfaces prompt cancellation as
    /// [`AuthError::Cancelled`]; upstream rejects "Login cancelled").
    #[tokio::test]
    async fn cancels_when_the_login_method_selection_is_cancelled() {
        let server = MockServer::start().await;
        let oauth = flow_with(&server, free_callback_port());
        let (_fake, interaction) = fake_interaction(Box::new(|_prompt| {
            Box::pin(async { Err(AuthError::Cancelled) })
        }));

        let result = oauth.login(interaction).await;
        assert_eq!(result, Err(AuthError::Cancelled));
    }

    /// Oracle: "cancels the OpenAI Codex device code flow while waiting".
    #[tokio::test]
    async fn cancels_the_device_code_flow_while_waiting() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "/api/accounts/deviceauth/usercode",
            r#"{"device_auth_id":"device-auth-id","user_code":"ABCD-1234","interval":"5"}"#,
            200,
        )
        .await;
        mount_json(
            &server,
            "/api/accounts/deviceauth/token",
            &device_auth_pending_response(),
            403,
        )
        .await;
        let oauth = flow_with(&server, free_callback_port());
        let (fake, interaction) = device_code_interaction();
        let signal = interaction.signal.clone();
        let driver = tokio::spawn(async move {
            // Cancel once the first poll landed (while the flow sleeps).
            loop {
                if server_received_poll(&server).await {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            signal.cancel();
            let _ = fake;
        });

        let result = oauth.login(interaction).await;
        assert_eq!(result, Err(AuthError::Cancelled));
        driver.await.unwrap();
    }

    async fn server_received_poll(server: &MockServer) -> bool {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|request| request.url.path() == "/api/accounts/deviceauth/token")
    }

    /// Oracle: "treats OpenAI Codex device auth 403 and 404 responses as
    /// pending" (real-time with the 1s test interval).
    #[tokio::test]
    async fn device_auth_403_and_404_responses_are_pending() {
        let server = MockServer::start().await;
        let access_token = create_access_token("account-403-404");
        mount_json(
            &server,
            "/api/accounts/deviceauth/usercode",
            r#"{"device_auth_id":"device-auth-id","user_code":"ABCD-1234","interval":"1"}"#,
            200,
        )
        .await;
        // First poll: 403 with a non-pending error body — still pending.
        // Second poll: 404 with a plain-text body — still pending.
        // Third poll: completion.
        mount_json_queue(
            &server,
            "/api/accounts/deviceauth/token",
            vec![
                (
                    403,
                    "application/json",
                    r#"{"error":"access_denied","error_description":"denied"}"#.to_string(),
                ),
                (404, "text/plain", "not ready".to_string()),
                (
                    200,
                    "application/json",
                    device_code_ok("oauth-code", "device-code-verifier"),
                ),
            ],
        )
        .await;
        mount_json(
            &server,
            "/oauth/token",
            &format!(
                r#"{{"access_token":{},"refresh_token":"refresh-token","expires_in":3600}}"#,
                serde_json::to_string(&access_token).unwrap()
            ),
            200,
        )
        .await;
        let oauth = flow_with(&server, free_callback_port());
        let (_fake, interaction) = device_code_interaction();

        let credential = oauth.login(interaction).await.unwrap();
        assert_eq!(
            credential.extra.get("accountId").and_then(|v| v.as_str()),
            Some("account-403-404")
        );
        let polls = server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|request| request.url.path() == "/api/accounts/deviceauth/token")
            .count();
        assert_eq!(polls, 3);
    }

    /// Oracle: "includes the response body in OpenAI Codex device auth poll
    /// failures".
    #[tokio::test]
    async fn device_auth_poll_failure_includes_the_response_body() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "/api/accounts/deviceauth/usercode",
            r#"{"device_auth_id":"device-auth-id","user_code":"ABCD-1234","interval":"5"}"#,
            200,
        )
        .await;
        mount_json(
            &server,
            "/api/accounts/deviceauth/token",
            r#"{"error":"server_error","error_description":"try again later"}"#,
            500,
        )
        .await;
        let oauth = flow_with(&server, free_callback_port());
        let (_fake, interaction) = device_code_interaction();

        let error = oauth.login(interaction).await.unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation(
                r#"OpenAI Codex device auth failed with status 500: {"error":"server_error","error_description":"try again later"}"#
                    .to_string()
            )
        );
    }

    /// Oracle: "does not write token refresh failures to stderr" — the port
    /// has no stderr channel; the pinned surface is the error message shape.
    #[tokio::test]
    async fn refresh_failure_message_carries_the_status_and_body() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "/oauth/token",
            r#"{"error":{"message":"Could not validate your token. Please try signing in again.","type":"invalid_request_error"}}"#,
            401
        )
        .await;
        let oauth = flow_with(&server, free_callback_port());

        let error = oauth
            .refresh(
                oauth_credential("invalid-access-token", "invalid-refresh-token"),
                &AuthOperationOptions::default(),
            )
            .await
            .unwrap_err();

        match error {
            AuthError::Operation(message) => {
                assert!(message.starts_with("OpenAI Codex token refresh failed (401): "));
                assert!(
                    message.contains("Could not validate your token"),
                    "{message}"
                );
            }
            other => panic!("expected an operation error, got {other:?}"),
        }
        // Byte-pinned refresh body (URLSearchParams insertion order).
        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            String::from_utf8(requests[0].body.clone()).unwrap(),
            "grant_type=refresh_token&refresh_token=invalid-refresh-token&\
             client_id=app_EMoamEEZ73f0CkXaXp7hrann"
        );
    }

    // ---- Browser flow ----

    #[tokio::test]
    async fn login_completes_through_the_local_callback_server() {
        let server = MockServer::start().await;
        let access_token = create_access_token("cb-account");
        mount_json(
            &server,
            "/oauth/token",
            &format!(
                r#"{{"access_token":{},"refresh_token":"cb-refresh","expires_in":3600}}"#,
                serde_json::to_string(&access_token).unwrap()
            ),
            200,
        )
        .await;
        let port = free_callback_port();
        let oauth = flow_with(&server, port);
        let (fake, interaction) = hanging_interaction();
        let auth_url_slot = Arc::clone(&fake.auth_url);

        let driver = tokio::spawn(async move {
            let auth_url = loop {
                if let Some(auth_url) = auth_url_slot.lock().unwrap().clone() {
                    break auth_url;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            };
            let state = auth_url_param(&auth_url, "state");
            http_get(port, &format!("/auth/callback?code=cb-code&state={state}")).await
        });

        let credential = tokio::time::timeout(Duration::from_secs(10), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap();
        let response = driver.await.unwrap();

        assert_eq!(credential.access, access_token);
        assert_eq!(credential.refresh, "cb-refresh");
        assert_eq!(
            credential.extra.get("accountId").and_then(|v| v.as_str()),
            Some("cb-account")
        );
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("Content-Type: text/html; charset=utf-8"));
        assert!(response.contains("<h1>Authentication successful</h1>"));
        assert!(response.contains("Cache-Control: no-store"));
        assert!(response.contains("Signed in to OpenAI. You may now close this page."));
        // The manual prompt was aborted by the finally block.
        assert!(fake.prompts.lock().unwrap()[1]
            .signal
            .as_ref()
            .unwrap()
            .is_cancelled());
    }

    #[tokio::test]
    async fn browser_login_resolves_through_the_manual_prompt() {
        let server = MockServer::start().await;
        let access_token = create_access_token("manual-account");
        mount_json(
            &server,
            "/oauth/token",
            &format!(
                r#"{{"access_token":{},"refresh_token":"refresh-token","expires_in":3600}}"#,
                serde_json::to_string(&access_token).unwrap()
            ),
            200,
        )
        .await;
        let oauth = flow_with(&server, free_callback_port());
        let (fake, interaction) =
            redirect_url_interaction(Arc::new(|state| format!("code=manual-code&state={state}")));

        let credential = oauth.login(interaction).await.unwrap();

        assert_eq!(credential.access, access_token);
        assert_eq!(
            credential.extra.get("accountId").and_then(|v| v.as_str()),
            Some("manual-account")
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let auth_url = auth_url_of(&fake);
        let body = String::from_utf8(requests[0].body.clone()).unwrap();
        // The PKCE verifier from the exchange hashes to the auth URL's
        // code_challenge (the state is random, so the verifier cannot be
        // pinned directly).
        let verifier = body
            .split('&')
            .find(|pair| pair.starts_with("code_verifier="))
            .unwrap()
            .strip_prefix("code_verifier=")
            .unwrap();
        assert_eq!(
            auth_url_param(&auth_url, "code_challenge"),
            base64url_encode(&Sha256::digest(verifier.as_bytes()))
        );
        assert_eq!(
            body,
            format!(
                "grant_type=authorization_code&client_id=app_EMoamEEZ73f0CkXaXp7hrann&\
                 code=manual-code&code_verifier={verifier}&\
                 redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback"
            )
        );
    }

    #[tokio::test]
    async fn manual_input_with_a_state_mismatch_is_rejected_without_exchanging() {
        let server = MockServer::start().await;
        mount_json(&server, "/oauth/token", "{}", 200).await;
        let oauth = flow_with(&server, free_callback_port());
        let (_fake, interaction) =
            redirect_url_interaction(Arc::new(|_state| "code=x&state=wrong-state".to_string()));

        let error = oauth.login(interaction).await.unwrap_err();
        assert_eq!(error, AuthError::Operation("State mismatch".to_string()));
    }

    #[tokio::test]
    async fn manual_bare_code_exchanges_without_a_state_check() {
        let server = MockServer::start().await;
        let access_token = create_access_token("bare-account");
        mount_json(
            &server,
            "/oauth/token",
            &format!(
                r#"{{"access_token":{},"refresh_token":"r","expires_in":3600}}"#,
                serde_json::to_string(&access_token).unwrap()
            ),
            200,
        )
        .await;
        let oauth = flow_with(&server, free_callback_port());
        let (_fake, interaction) = browser_interaction("bare-code");

        let credential = oauth.login(interaction).await.unwrap();
        assert_eq!(
            credential.extra.get("accountId").and_then(|v| v.as_str()),
            Some("bare-account")
        );
        let requests = server.received_requests().await.unwrap();
        let body = String::from_utf8(requests[0].body.clone()).unwrap();
        assert!(body.starts_with(
            "grant_type=authorization_code&client_id=app_EMoamEEZ73f0CkXaXp7hrann&code=bare-code&"
        ));
    }

    #[tokio::test]
    async fn empty_manual_input_reports_a_missing_authorization_code() {
        let server = MockServer::start().await;
        mount_json(&server, "/oauth/token", "{}", 200).await;
        let oauth = flow_with(&server, free_callback_port());
        let (_fake, interaction) = browser_interaction("   ");

        let error = oauth.login(interaction).await.unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("Missing authorization code".to_string())
        );
    }

    #[tokio::test]
    async fn auth_url_carries_the_pkce_parameters_in_upstream_order() {
        let server = MockServer::start().await;
        let access_token = create_access_token("url-account");
        mount_json(
            &server,
            "/oauth/token",
            &format!(
                r#"{{"access_token":{},"refresh_token":"r","expires_in":3600}}"#,
                serde_json::to_string(&access_token).unwrap()
            ),
            200,
        )
        .await;
        let oauth = flow_with(&server, free_callback_port());
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
            Some("A browser window should open. Complete login to finish.")
        );

        let parsed = Url::parse(&url).unwrap();
        assert_eq!(parsed.scheme(), "https");
        assert_eq!(parsed.host_str(), Some("auth.openai.com"));
        assert_eq!(parsed.path(), "/oauth/authorize");
        let state = auth_url_param(&url, "state");
        assert_eq!(state.len(), 16 * 2);
        assert!(state.bytes().all(|byte| byte.is_ascii_hexdigit()));
        // code_challenge is the S256 of the exchange's code_verifier.
        let requests = server.received_requests().await.unwrap();
        let body = String::from_utf8(requests[0].body.clone()).unwrap();
        let verifier = body
            .split('&')
            .find(|pair| pair.starts_with("code_verifier="))
            .unwrap()
            .strip_prefix("code_verifier=")
            .unwrap();
        assert_eq!(
            auth_url_param(&url, "code_challenge"),
            base64url_encode(&Sha256::digest(verifier.as_bytes()))
        );
        // The full query, in upstream insertion order.
        assert_eq!(
            parsed.query().unwrap(),
            format!(
                "response_type=code&client_id=app_EMoamEEZ73f0CkXaXp7hrann&\
                 redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback&\
                 scope=openid+profile+email+offline_access&code_challenge={}&\
                 code_challenge_method=S256&state={state}&\
                 id_token_add_organizations=true&codex_cli_simplified_flow=true&originator=pi",
                auth_url_param(&url, "code_challenge")
            )
        );
    }

    #[tokio::test]
    async fn unknown_login_method_is_rejected() {
        let server = MockServer::start().await;
        let oauth = flow_with(&server, free_callback_port());
        let (_fake, interaction) = select_interaction("carrier-pigeon");

        let error = oauth.login(interaction).await.unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("Unknown OpenAI Codex login method: carrier-pigeon".to_string())
        );
    }

    // ---- Token endpoint errors ----

    #[tokio::test]
    async fn exchange_http_failure_carries_status_and_body() {
        let server = MockServer::start().await;
        mount_json(&server, "/oauth/token", "denied", 400).await;
        let oauth = flow_with(&server, free_callback_port());
        let (_fake, interaction) = browser_interaction("the-code");

        let error = oauth.login(interaction).await.unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("OpenAI Codex token exchange failed (400): denied".to_string())
        );
    }

    #[tokio::test]
    async fn exchange_missing_fields_error_names_the_response() {
        let server = MockServer::start().await;
        mount_json(&server, "/oauth/token", "{}", 200).await;
        let oauth = flow_with(&server, free_callback_port());
        let (_fake, interaction) = browser_interaction("the-code");

        let error = oauth.login(interaction).await.unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation(
                "OpenAI Codex token exchange response missing fields: {}".to_string()
            )
        );
    }

    #[tokio::test]
    async fn refresh_success_carries_the_full_expiry_and_account_id() {
        let server = MockServer::start().await;
        let access_token = create_access_token("acc-refresh");
        mount_json(
            &server,
            "/oauth/token",
            &format!(
                r#"{{"access_token":{},"refresh_token":"new-refresh","expires_in":3600}}"#,
                serde_json::to_string(&access_token).unwrap()
            ),
            200,
        )
        .await;
        let oauth = flow_with(&server, free_callback_port());

        let credential = oauth
            .refresh(
                oauth_credential("old", "refresh-token"),
                &AuthOperationOptions::default(),
            )
            .await
            .unwrap();

        assert_eq!(credential.access, access_token);
        assert_eq!(credential.refresh, "new-refresh");
        // No 5-minute shave (upstream codex: Date.now() + expires_in * 1000).
        assert!((credential.expires - (now_ms() + 3_600_000)).abs() <= 2_000);
        assert_eq!(
            credential.extra.get("accountId").and_then(|v| v.as_str()),
            Some("acc-refresh")
        );
    }

    // ---- Device endpoint errors and shapes ----

    #[tokio::test]
    async fn usercode_404_reports_device_login_not_enabled() {
        let server = MockServer::start().await;
        mount_json(&server, "/api/accounts/deviceauth/usercode", "nope", 404).await;
        let oauth = flow_with(&server, free_callback_port());
        let (_fake, interaction) = device_code_interaction();

        let error = oauth.login(interaction).await.unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation(
                "OpenAI Codex device code login is not enabled for this server. Use browser \
                 login or verify the server URL."
                    .to_string()
            )
        );
    }

    #[tokio::test]
    async fn usercode_failure_includes_status_and_body() {
        let server = MockServer::start().await;
        mount_json(&server, "/api/accounts/deviceauth/usercode", "boom", 503).await;
        let oauth = flow_with(&server, free_callback_port());
        let (_fake, interaction) = device_code_interaction();

        let error = oauth.login(interaction).await.unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation(
                "OpenAI Codex device code request failed with status 503: boom".to_string()
            )
        );
    }

    #[tokio::test]
    async fn usercode_invalid_response_reports_missing_fields() {
        let server = MockServer::start().await;
        // `interval` missing entirely: invalid (unlike the generic device
        // flow, codex requires it).
        mount_json(
            &server,
            "/api/accounts/deviceauth/usercode",
            r#"{"device_auth_id":"device-auth-id","user_code":"ABCD-1234"}"#,
            200,
        )
        .await;
        let oauth = flow_with(&server, free_callback_port());
        let (_fake, interaction) = device_code_interaction();

        let error = oauth.login(interaction).await.unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation(
                r#"Invalid OpenAI Codex device code response: {"device_auth_id":"device-auth-id","user_code":"ABCD-1234"}"#
                    .to_string()
            )
        );
    }

    #[tokio::test]
    async fn usercode_interval_as_a_string_is_parsed_and_negative_rejected() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "/api/accounts/deviceauth/usercode",
            r#"{"device_auth_id":"device-auth-id","user_code":"ABCD-1234","interval":-1}"#,
            200,
        )
        .await;
        let oauth = flow_with(&server, free_callback_port());
        let (_fake, interaction) = device_code_interaction();

        let error = oauth.login(interaction).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Invalid OpenAI Codex device code response"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn device_poll_missing_fields_fails() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "/api/accounts/deviceauth/usercode",
            r#"{"device_auth_id":"device-auth-id","user_code":"ABCD-1234","interval":"5"}"#,
            200,
        )
        .await;
        mount_json(
            &server,
            "/api/accounts/deviceauth/token",
            r#"{"authorization_code":"oauth-code"}"#,
            200,
        )
        .await;
        let oauth = flow_with(&server, free_callback_port());
        let (_fake, interaction) = device_code_interaction();

        let error = oauth.login(interaction).await.unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation(
                r#"Invalid OpenAI Codex device auth token response: {"authorization_code":"oauth-code"}"#
                    .to_string()
            )
        );
    }

    // ---- Poll timing engine (device-code.ts pollOAuthDeviceCodeFlow, via
    // ---- the shared device_code engine; the T4 private copy's pins) ----

    type EngineOutcome = PollOutcome<()>;

    /// Builds an engine poll closure serving the queued outcomes and
    /// recording the poll instants.
    fn engine_polls(
        times: Arc<Mutex<Vec<tokio::time::Instant>>>,
        outcomes: Arc<Mutex<VecDeque<EngineOutcome>>>,
        on_poll: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> impl FnMut() -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<EngineOutcome, AuthError>> + Send>,
    > {
        move || {
            let times = Arc::clone(&times);
            let outcomes = Arc::clone(&outcomes);
            let on_poll = on_poll.clone();
            Box::pin(async move {
                times.lock().unwrap().push(tokio::time::Instant::now());
                if let Some(on_poll) = &on_poll {
                    on_poll();
                }
                let outcome = outcomes
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or(EngineOutcome::Pending);
                Ok(outcome)
            })
        }
    }

    fn instant() -> tokio::time::Instant {
        tokio::time::Instant::now()
    }

    #[tokio::test(start_paused = true)]
    async fn engine_polls_immediately_then_at_each_interval() {
        let token = CancellationToken::new();
        let times = Arc::new(Mutex::new(Vec::new()));
        let outcomes = Arc::new(Mutex::new(VecDeque::from(vec![
            PollOutcome::<()>::Pending,
            PollOutcome::<()>::Pending,
            PollOutcome::<()>::Complete(()),
        ])));
        let start = instant();

        poll_device_code_flow(
            Some(5.0),
            None,
            false,
            &token,
            engine_polls(Arc::clone(&times), outcomes, None),
        )
        .await
        .unwrap();

        let times = times.lock().unwrap().clone();
        assert_eq!(
            times,
            vec![
                start,
                start + Duration::from_secs(5),
                start + Duration::from_secs(10)
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn engine_times_out_after_the_deadline() {
        let token = CancellationToken::new();
        let times = Arc::new(Mutex::new(Vec::new()));
        let outcomes = Arc::new(Mutex::new(VecDeque::<EngineOutcome>::new()));

        let error = poll_device_code_flow(
            Some(60.0),
            Some(15.0 * 60.0),
            false,
            &token,
            engine_polls(Arc::clone(&times), outcomes, None),
        )
        .await
        .unwrap_err();

        assert_eq!(error, AuthError::Operation(TIMEOUT_MESSAGE.to_string()));
        // Polls at 0, 60, ..., 840; the 900s check breaks the loop.
        assert_eq!(times.lock().unwrap().len(), 15);
    }

    #[tokio::test(start_paused = true)]
    async fn engine_slow_down_bumps_the_interval_and_changes_the_timeout_message() {
        let token = CancellationToken::new();
        let times = Arc::new(Mutex::new(Vec::new()));
        let outcomes = Arc::new(Mutex::new(VecDeque::from(vec![
            PollOutcome::<()>::SlowDown(None),
            PollOutcome::<()>::SlowDown(None),
        ])));
        let start = instant();

        let error = poll_device_code_flow(
            Some(5.0),
            Some(30.0),
            false,
            &token,
            engine_polls(Arc::clone(&times), outcomes, None),
        )
        .await
        .unwrap_err();

        assert_eq!(
            error,
            AuthError::Operation(SLOW_DOWN_TIMEOUT_MESSAGE.to_string())
        );
        // 5s → slow_down → 10s → slow_down → 15s.
        assert_eq!(
            times.lock().unwrap().clone(),
            vec![
                start,
                start + Duration::from_secs(10),
                start + Duration::from_secs(25)
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn engine_uses_a_server_provided_slow_down_interval() {
        let token = CancellationToken::new();
        let times = Arc::new(Mutex::new(Vec::new()));
        let outcomes = Arc::new(Mutex::new(VecDeque::from(vec![
            PollOutcome::<()>::SlowDown(Some(2.0)),
            PollOutcome::<()>::Complete(()),
        ])));
        let start = instant();

        poll_device_code_flow(
            Some(5.0),
            None,
            false,
            &token,
            engine_polls(Arc::clone(&times), outcomes, None),
        )
        .await
        .unwrap();

        assert_eq!(
            times.lock().unwrap().clone(),
            vec![start, start + Duration::from_secs(2)]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn engine_interval_floors_to_the_one_second_minimum() {
        let token = CancellationToken::new();
        let times = Arc::new(Mutex::new(Vec::new()));
        let outcomes = Arc::new(Mutex::new(VecDeque::from(vec![
            PollOutcome::<()>::Pending,
            PollOutcome::<()>::Complete(()),
        ])));
        let start = instant();

        poll_device_code_flow(
            Some(0.0),
            None,
            false,
            &token,
            engine_polls(Arc::clone(&times), outcomes, None),
        )
        .await
        .unwrap();

        assert_eq!(
            times.lock().unwrap().clone(),
            vec![start, start + Duration::from_secs(1)]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn engine_cancelled_signal_aborts_before_and_between_polls() {
        // Pre-cancelled: no poll at all.
        let token = CancellationToken::new();
        token.cancel();
        let times = Arc::new(Mutex::new(Vec::new()));
        let outcomes = Arc::new(Mutex::new(VecDeque::<EngineOutcome>::new()));
        let error = poll_device_code_flow(
            Some(5.0),
            Some(900.0),
            false,
            &token,
            engine_polls(Arc::clone(&times), outcomes, None),
        )
        .await
        .unwrap_err();
        assert_eq!(error, AuthError::Cancelled);
        assert!(times.lock().unwrap().is_empty());

        // Cancelled between polls: the abortable sleep sees the fired token.
        let token = CancellationToken::new();
        let times = Arc::new(Mutex::new(Vec::new()));
        let signal_for_poll = token.clone();
        let on_poll: Arc<dyn Fn() + Send + Sync> = Arc::new(move || signal_for_poll.cancel());
        let error = poll_device_code_flow(
            Some(5.0),
            Some(900.0),
            false,
            &token,
            engine_polls(
                Arc::clone(&times),
                Arc::new(Mutex::new(VecDeque::<EngineOutcome>::new())),
                Some(on_poll),
            ),
        )
        .await
        .unwrap_err();
        assert_eq!(error, AuthError::Cancelled);
        assert_eq!(times.lock().unwrap().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn engine_failed_poll_surfaces_the_message() {
        let token = CancellationToken::new();
        let times = Arc::new(Mutex::new(Vec::new()));
        let outcomes = Arc::new(Mutex::new(VecDeque::from(vec![PollOutcome::<()>::Failed(
            "boom".to_string(),
        )])));

        let error = poll_device_code_flow(
            Some(5.0),
            None,
            false,
            &token,
            engine_polls(Arc::clone(&times), outcomes, None),
        )
        .await
        .unwrap_err();
        assert_eq!(error, AuthError::Operation("boom".to_string()));
        assert_eq!(times.lock().unwrap().len(), 1);
    }

    // ---- Units ----

    #[test]
    fn production_endpoints_match_upstream() {
        assert_eq!(CLIENT_ID, "app_EMoamEEZ73f0CkXaXp7hrann");
        assert_eq!(AUTHORIZE_URL, "https://auth.openai.com/oauth/authorize");
        assert_eq!(TOKEN_URL, "https://auth.openai.com/oauth/token");
        assert_eq!(REDIRECT_URI, "http://localhost:1455/auth/callback");
        assert_eq!(
            DEVICE_USER_CODE_URL,
            "https://auth.openai.com/api/accounts/deviceauth/usercode"
        );
        assert_eq!(
            DEVICE_TOKEN_URL,
            "https://auth.openai.com/api/accounts/deviceauth/token"
        );
        assert_eq!(
            DEVICE_VERIFICATION_URI,
            "https://auth.openai.com/codex/device"
        );
        assert_eq!(
            DEVICE_REDIRECT_URI,
            "https://auth.openai.com/deviceauth/callback"
        );
        assert_eq!(DEVICE_CODE_TIMEOUT_SECONDS, 900);
        assert_eq!(BROWSER_LOGIN_METHOD, "browser");
        assert_eq!(DEVICE_CODE_LOGIN_METHOD, "device_code");
        assert_eq!(SCOPE, "openid profile email offline_access");
        assert_eq!(DEFAULT_CALLBACK_HOST, "127.0.0.1");
        assert_eq!(CALLBACK_PORT, 1455);
        assert_eq!(CALLBACK_PATH, "/auth/callback");
        assert_eq!(ORIGINATOR, "pi");
    }

    #[test]
    fn get_account_id_follows_the_upstream_jwt_semantics() {
        assert_eq!(
            get_account_id(&create_access_token("acc-1")),
            Some("acc-1".to_string())
        );
        // Empty account id is not an account id.
        assert_eq!(get_account_id(&create_access_token("")), None);
        // Missing claim path.
        let header = b64(br#"{"alg":"none"}"#);
        let payload = b64(br#"{"other":1}"#);
        assert_eq!(get_account_id(&format!("{header}.{payload}.sig")), None);
        // Non-string claim.
        let payload = b64(br#"{"https://api.openai.com/auth":{"chatgpt_account_id":5}}"#);
        assert_eq!(get_account_id(&format!("{header}.{payload}.sig")), None);
        // Not a JWT.
        assert_eq!(get_account_id("garbage"), None);
        // Payload not base64/JSON.
        assert_eq!(get_account_id("a.!!!.c"), None);
    }

    #[tokio::test]
    async fn to_auth_and_metadata_match_upstream() {
        let oauth = OpenAICodexOAuth::new();
        assert_eq!(oauth.name(), "OpenAI (ChatGPT Plus/Pro)");
        assert!(oauth.is_subscription());
        assert_eq!(oauth.login_label(), None);
        let auth = oauth
            .to_auth(oauth_credential("access-token", "r"))
            .await
            .unwrap();
        assert_eq!(
            auth,
            ModelAuth {
                api_key: Some("access-token".to_string()),
                headers: None,
                base_url: None
            }
        );
    }
}
