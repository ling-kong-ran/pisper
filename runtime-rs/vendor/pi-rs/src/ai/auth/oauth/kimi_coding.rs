//! Kimi Code (subscription) OAuth flow ported from upstream
//! `packages/ai/src/auth/oauth/kimi-coding.ts`: the RFC 8628 device
//! authorization request against `https://auth.kimi.com` (JSON responses),
//! the token polling driven by the shared [`super::device_code`] engine, and
//! the refresh grant with retry/backoff, exposed as the [`KimiCodingOAuth`]
//! [`OAuthAuth`] implementation.
//!
//! Interactive surface (M2d ruling): the device code is reported via
//! [`AuthEvent::DeviceCode`] and the flow never prompts, touches stdio or a
//! browser directly.
//!
//! Port notes (disclosed divergences):
//! - The OAuth host is a field on [`KimiCodingOAuth`] (upstream: resolved per
//!   call by `getOauthHost()`); [`KimiCodingOAuth::new`] resolves the
//!   `KIMI_CODE_OAUTH_HOST` / `KIMI_OAUTH_HOST` overrides once and the test
//!   constructor pins a host directly.
//! - The per-request 30-second timeout (upstream
//!   `AbortSignal.timeout(REQUEST_TIMEOUT_MS)`) races the response; on
//!   timeout the port surfaces `Kimi Code OAuth request timed out` where
//!   upstream surfaces the runtime's timeout DOMException text.
//! - The refresh backoff base (upstream constant 1000ms) is a field so tests
//!   can shrink the retry sleeps, like the GitHub Copilot request-timeout
//!   injection. The exponential shape (`base * 2^(attempt-1)`) is upstream.
//! - Poll deadlines run on the shared device-code engine's tokio clock; the
//!   wire tests use 1-second server intervals (oracle: 5s + fake timers) and
//!   the exact schedule is pinned by the engine's own tests.
//! - Cancellation maps to [`AuthError::Cancelled`] everywhere upstream throws
//!   `Error("Kimi Code token refresh aborted")` or rejects the sleep
//!   (port contract: interaction-signal aborts are never wrapped).
//! - `JSON.stringify(json)` in error messages re-serializes with serde's map
//!   ordering (alphabetical) instead of insertion order; a body that failed
//!   to parse (or was not an object) reads as `null`, which
//!   `JSON.stringify(null)` also produces.
//! - `AuthEvent::DeviceCode.interval_seconds`/`expires_in_seconds` are `u64`:
//!   fractional server values report truncated (values are integral in
//!   practice; the flow only forwards finite positive numbers).

use std::collections::BTreeMap;

use futures::future::BoxFuture;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::ai::api::azure_openai_responses::get_provider_env_value;
use crate::ai::api::http_client;
use crate::ai::auth::types::{
    AuthError, AuthEvent, AuthInteraction, ModelAuth, OAuthAuth, OAuthCredential,
    ProviderAuthInteraction,
};
use crate::ai::now_ms;
use crate::ai::types::ProviderHeaders;

use super::device_code::{abortable_sleep, poll_device_code_flow, PollOutcome};

/// Upstream `CLIENT_ID` (kimi-coding.ts:14).
const CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";

/// Upstream `DEFAULT_OAUTH_HOST` (kimi-coding.ts:15).
const DEFAULT_OAUTH_HOST: &str = "https://auth.kimi.com";

/// Upstream `DEVICE_CODE_TIMEOUT_SECONDS` (kimi-coding.ts:16): the default
/// `expires_in` when the device authorization response omits it.
const DEVICE_CODE_TIMEOUT_SECONDS: f64 = 15.0 * 60.0;

/// Upstream `DEFAULT_POLL_INTERVAL_SECONDS` (kimi-coding.ts:17): the default
/// `interval` when the device authorization response omits it.
const DEFAULT_POLL_INTERVAL_SECONDS: f64 = 5.0;

/// Upstream `REQUEST_TIMEOUT_MS` (kimi-coding.ts:18).
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Upstream `REFRESH_MAX_RETRIES` (kimi-coding.ts:19): retries after the
/// initial attempt (4 requests total).
const REFRESH_MAX_RETRIES: u32 = 3;

/// Upstream `getProviderEnvValue("KIMI_CODE_OAUTH_HOST")` (kimi-coding.ts:37).
const KIMI_CODE_OAUTH_HOST_ENV: &str = "KIMI_CODE_OAUTH_HOST";

/// Upstream `getProviderEnvValue("KIMI_OAUTH_HOST")` (kimi-coding.ts:37).
const KIMI_OAUTH_HOST_ENV: &str = "KIMI_OAUTH_HOST";

/// The Kimi Code OAuth auth surface (upstream `kimiCodingOAuth`,
/// kimi-coding.ts:281-296).
pub struct KimiCodingOAuth {
    oauth_host: String,
    /// Test-only override of the 1000ms exponential-refresh-backoff base.
    refresh_backoff: std::time::Duration,
}

impl Default for KimiCodingOAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl KimiCodingOAuth {
    /// Upstream `getOauthHost()` resolved once: the `KIMI_CODE_OAUTH_HOST`,
    /// then the `KIMI_OAUTH_HOST` provider env value, else the default host,
    /// with trailing slashes stripped.
    pub fn new() -> Self {
        KimiCodingOAuth {
            oauth_host: resolve_oauth_host(),
            refresh_backoff: std::time::Duration::from_millis(1000),
        }
    }

    /// Test constructor: point the endpoints at a stub server and shrink the
    /// refresh backoff (upstream tests stub the global `fetch` and fake
    /// timers).
    #[cfg(test)]
    fn with_host(oauth_host: String, refresh_backoff: std::time::Duration) -> Self {
        KimiCodingOAuth {
            oauth_host,
            refresh_backoff,
        }
    }
}

/// Upstream `getOauthHost` (kimi-coding.ts:36-39): `KIMI_CODE_OAUTH_HOST`
/// then `KIMI_OAUTH_HOST`, else the default, trailing slashes stripped.
fn resolve_oauth_host() -> String {
    let host = get_provider_env_value(KIMI_CODE_OAUTH_HOST_ENV, None)
        .or_else(|| get_provider_env_value(KIMI_OAUTH_HOST_ENV, None))
        .unwrap_or_else(|| DEFAULT_OAUTH_HOST.to_string());
    host.trim_end_matches('/').to_string()
}

/// One completed form POST: the status plus the raw body text (both the 5xx
/// message branch and the JSON branches consume it).
struct FormResponse {
    status: u16,
    ok: bool,
    text: String,
}

/// Upstream `requestSignal(signal)` +
/// `fetch` (kimi-coding.ts:41-43): POST with the form headers, racing the
/// interaction signal and the 30-second request timeout. Cancellation stays
/// [`AuthError::Cancelled`]; transport and timeout failures carry their raw
/// error text (upstream's `fetch` rejection).
async fn post_form(
    url: &str,
    body: String,
    signal: &CancellationToken,
) -> Result<FormResponse, AuthError> {
    let deadline = tokio::time::Instant::now() + REQUEST_TIMEOUT;
    let request = http_client()
        .post(url)
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body);

    let response = tokio::select! {
        biased;
        _ = signal.cancelled() => return Err(AuthError::Cancelled),
        _ = tokio::time::sleep_until(deadline) => {
            return Err(AuthError::Operation("Kimi Code OAuth request timed out".to_string()));
        }
        response = request.send() => match response {
            Ok(response) => response,
            Err(error) => return Err(AuthError::Operation(error.to_string())),
        },
    };
    let status = response.status();
    let text = tokio::select! {
        biased;
        _ = signal.cancelled() => return Err(AuthError::Cancelled),
        _ = tokio::time::sleep_until(deadline) => {
            return Err(AuthError::Operation("Kimi Code OAuth request timed out".to_string()));
        }
        text = response.text() => match text {
            Ok(text) => text,
            Err(error) => return Err(AuthError::Operation(error.to_string())),
        },
    };
    Ok(FormResponse {
        status: status.as_u16(),
        ok: status.is_success(),
        text,
    })
}

/// Upstream `readJson` (kimi-coding.ts:49-56): a body that fails to parse or
/// is not a JSON object reads as `None` (`null`), never an error. Upstream
/// `typeof json === "object"` accepts arrays too (a JS array is an object), so
/// the port keeps the raw [`Value`]: field access on an array misses like
/// upstream's `undefined` property reads, and [`stringify_json`] renders `[]`
/// like upstream's `JSON.stringify`.
fn parse_json_object(text: &str) -> Option<Value> {
    match serde_json::from_str::<Value>(text) {
        Ok(json @ Value::Object(_)) | Ok(json @ Value::Array(_)) => Some(json),
        _ => None,
    }
}

/// Upstream `JSON.stringify(json)` over a [`parse_json_object`] result:
/// `null` for a missing body, otherwise re-serialized (serde map ordering;
/// see the module port notes).
fn stringify_json(json: Option<&Value>) -> String {
    match json {
        Some(json) => json.to_string(),
        None => "null".to_string(),
    }
}

/// `` `${text ? `: ${text}` : ""}` `` over a raw body text.
fn text_suffix(text: &str) -> String {
    if text.is_empty() {
        String::new()
    } else {
        format!(": {text}")
    }
}

/// Upstream `trustedHttpUrl` (kimi-coding.ts:59-68): a non-empty string that
/// parses as an absolute http(s) URL. The verification URI is opened in the
/// user's browser; only http(s) URLs are trusted.
fn trusted_http_url(value: &Value) -> bool {
    let Some(text) = value.as_str() else {
        return false;
    };
    if text.is_empty() {
        return false;
    }
    matches!(
        url::Url::parse(text).map(|url| url.scheme().to_string()),
        Ok(scheme) if scheme == "https" || scheme == "http"
    )
}

/// Upstream `DeviceAuthorization` (kimi-coding.ts:21-28). The base
/// `verification_uri` is validated against `trustedHttpUrl` but never read:
/// the event reports `verification_uri_complete` (upstream keeps the field
/// in the response shape; the port drops the unread value).
struct DeviceAuthorization {
    device_code: String,
    user_code: String,
    verification_uri_complete: String,
    interval_seconds: f64,
    expires_in_seconds: f64,
}

/// Upstream `startDeviceAuthorization` (kimi-coding.ts:70-118).
async fn start_device_authorization(
    oauth_host: &str,
    signal: &CancellationToken,
) -> Result<DeviceAuthorization, AuthError> {
    let body = {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("client_id", CLIENT_ID);
        query.finish()
    };
    let response = post_form(
        &format!("{oauth_host}/api/oauth/device_authorization"),
        body,
        signal,
    )
    .await?;
    if !response.ok {
        return Err(AuthError::Operation(format!(
            "Kimi Code device authorization failed with status {}{}",
            response.status,
            text_suffix(&response.text)
        )));
    }

    let json = parse_json_object(&response.text);
    let json_ref = json.as_ref();
    let field_is_string = |name: &str| {
        json_ref
            .and_then(|json| json.get(name))
            .is_some_and(Value::is_string)
    };
    let field_url = |name: &str| {
        json_ref
            .and_then(|json| json.get(name))
            .is_some_and(trusted_http_url)
    };
    let valid = field_is_string("device_code")
        && field_is_string("user_code")
        && field_is_string("verification_uri")
        && field_is_string("verification_uri_complete")
        && field_url("verification_uri_complete")
        && field_url("verification_uri");
    if !valid {
        return Err(AuthError::Operation(format!(
            "Invalid Kimi Code device authorization response: {}",
            stringify_json(json_ref)
        )));
    }
    let json = json_ref.expect("validated above");
    // `typeof interval === "number" && Number.isFinite(interval) && interval
    // > 0 ? interval : DEFAULT`.
    let positive_or = |name: &str, default: f64| {
        json.get(name)
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite() && *value > 0.0)
            .unwrap_or(default)
    };
    Ok(DeviceAuthorization {
        device_code: json["device_code"].as_str().expect("string").to_string(),
        user_code: json["user_code"].as_str().expect("string").to_string(),
        verification_uri_complete: json["verification_uri_complete"]
            .as_str()
            .expect("string")
            .to_string(),
        interval_seconds: positive_or("interval", DEFAULT_POLL_INTERVAL_SECONDS),
        expires_in_seconds: positive_or("expires_in", DEVICE_CODE_TIMEOUT_SECONDS),
    })
}

/// Upstream `TokenResponse` (kimi-coding.ts:30-34).
struct TokenResponse {
    access: String,
    refresh: String,
    expires: i64,
}

/// Upstream `parseTokenResponse` (kimi-coding.ts:120-140): all three fields
/// are required (non-empty strings, finite positive `expires_in`); the raw
/// upstream message is returned so the poll can turn it into a `failed`
/// poll result.
fn parse_token_response(json: Option<&Value>, operation: &str) -> Result<TokenResponse, String> {
    let invalid = || {
        format!(
            "Kimi Code token {operation} response missing fields: {}",
            stringify_json(json)
        )
    };
    let json = json.ok_or_else(invalid)?;
    let access = json
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(invalid)?;
    let refresh = json
        .get("refresh_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(invalid)?;
    let expires_in = json
        .get("expires_in")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or_else(invalid)?;
    Ok(TokenResponse {
        access: access.to_string(),
        refresh: refresh.to_string(),
        expires: now_ms() + (expires_in * 1000.0) as i64,
    })
}

/// One poll of the token endpoint (the `poll` closure of upstream
/// `pollForToken`, kimi-coding.ts:152-206).
async fn poll_once(
    oauth_host: &str,
    device_code: &str,
    signal: &CancellationToken,
) -> Result<PollOutcome<TokenResponse>, AuthError> {
    let body = {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("client_id", CLIENT_ID);
        query.append_pair("device_code", device_code);
        query.append_pair("grant_type", "urn:ietf:params:oauth:grant-type:device_code");
        query.finish()
    };
    let response = post_form(&format!("{oauth_host}/api/oauth/token"), body, signal).await?;

    if response.status >= 500 {
        return Ok(PollOutcome::Failed(format!(
            "Kimi Code device token request failed with status {}{}",
            response.status,
            text_suffix(&response.text)
        )));
    }

    let json = parse_json_object(&response.text);
    if response.ok
        && json
            .as_ref()
            .and_then(|json| json.get("access_token"))
            .is_some_and(Value::is_string)
    {
        // Upstream catch: a token body missing fields becomes a `failed`
        // poll result carrying the parse error message.
        return match parse_token_response(json.as_ref(), "poll") {
            Ok(token) => Ok(PollOutcome::Complete(token)),
            Err(message) => Ok(PollOutcome::Failed(message)),
        };
    }

    let error = json.as_ref().and_then(|json| json.get("error"));
    match error.and_then(Value::as_str) {
        Some("authorization_pending") => Ok(PollOutcome::Pending),
        // `typeof interval === "number" && interval > 0 ? interval :
        // undefined` — the engine applies the finiteness filter.
        Some("slow_down") => Ok(PollOutcome::SlowDown(
            json.as_ref()
                .and_then(|json| json.get("interval"))
                .and_then(Value::as_f64)
                .filter(|interval| *interval > 0.0),
        )),
        Some("expired_token") => Ok(PollOutcome::Failed(
            "Kimi Code device authorization expired. Please restart login.".to_string(),
        )),
        Some("access_denied") => Ok(PollOutcome::Failed(
            "Kimi Code login was denied.".to_string(),
        )),
        _ => Ok(PollOutcome::Failed(match error {
            Some(Value::String(error)) => {
                let description = json
                    .as_ref()
                    .and_then(|json| json.get("error_description"))
                    .and_then(Value::as_str);
                format!(
                    "Kimi Code device token request failed (status {}): {error}{}",
                    response.status,
                    description
                        .map(|description| format!(": {description}"))
                        .unwrap_or_default()
                )
            }
            _ => format!(
                "Kimi Code device token request failed (status {})",
                response.status
            ),
        })),
    }
}

/// Upstream `pollForToken` (kimi-coding.ts:142-208): waits before the first
/// poll and follows the shared engine's schedule until the device code
/// expires.
async fn poll_for_token(
    oauth_host: &str,
    device: &DeviceAuthorization,
    signal: &CancellationToken,
) -> Result<TokenResponse, AuthError> {
    let oauth_host = oauth_host.to_string();
    let device_code = device.device_code.clone();
    poll_device_code_flow(
        Some(device.interval_seconds),
        Some(device.expires_in_seconds),
        true,
        signal,
        || poll_once(&oauth_host, &device_code, signal),
    )
    .await
}

/// Upstream `refreshToken` (kimi-coding.ts:214-265): up to
/// [`REFRESH_MAX_RETRIES`] retries after the initial attempt with
/// exponential backoff (base [`KimiCodingOAuth::refresh_backoff`]);
/// transport failures retry, `401`/`403`/`invalid_grant` fail immediately as
/// unauthorized, other statuses fail with the serialized body.
async fn refresh_token(
    oauth: &KimiCodingOAuth,
    refresh_token_value: &str,
    signal: &CancellationToken,
) -> Result<TokenResponse, AuthError> {
    let mut last_error: Option<AuthError> = None;
    for attempt in 0..=REFRESH_MAX_RETRIES {
        if attempt > 0 {
            // `sleep(1000 * 2 ** (attempt - 1), signal)`.
            let backoff = oauth.refresh_backoff.mul_f64(2f64.powi(attempt as i32 - 1));
            abortable_sleep(backoff, signal).await?;
        }
        if signal.is_cancelled() {
            return Err(AuthError::Cancelled);
        }

        let body = {
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            query.append_pair("client_id", CLIENT_ID);
            query.append_pair("grant_type", "refresh_token");
            query.append_pair("refresh_token", refresh_token_value);
            query.finish()
        };
        let response = match post_form(
            &format!("{}/api/oauth/token", oauth.oauth_host),
            body,
            signal,
        )
        .await
        {
            Ok(response) => response,
            Err(error @ AuthError::Cancelled) => return Err(error),
            // Upstream catch: a failed request is retried.
            Err(error) => {
                last_error = Some(error);
                continue;
            }
        };

        let json = parse_json_object(&response.text);
        if response.ok {
            return parse_token_response(json.as_ref(), "refresh").map_err(AuthError::Operation);
        }

        // Unauthorized: the stored credential is dead; Models clears it and
        // prompts re-login.
        let error_code = json
            .as_ref()
            .and_then(|json| json.get("error"))
            .and_then(Value::as_str);
        if response.status == 401 || response.status == 403 || error_code == Some("invalid_grant") {
            let description = json
                .as_ref()
                .and_then(|json| json.get("error_description"))
                .and_then(Value::as_str);
            return Err(AuthError::Operation(format!(
                "Kimi Code token refresh unauthorized (status {}){}",
                response.status,
                description
                    .map(|description| format!(": {description}"))
                    .unwrap_or_default()
            )));
        }

        if (response.status == 429 || response.status >= 500) && attempt < REFRESH_MAX_RETRIES {
            last_error = Some(AuthError::Operation(format!(
                "Kimi Code token refresh failed with status {}",
                response.status
            )));
            continue;
        }

        return Err(AuthError::Operation(format!(
            "Kimi Code token refresh failed with status {}: {}",
            response.status,
            stringify_json(json.as_ref())
        )));
    }

    Err(last_error
        .unwrap_or_else(|| AuthError::Operation("Kimi Code token refresh failed".to_string())))
}

/// Upstream `loginKimiCoding` (kimi-coding.ts:267-279).
async fn login_kimi_coding(
    oauth: &KimiCodingOAuth,
    interaction: ProviderAuthInteraction,
) -> Result<OAuthCredential, AuthError> {
    if interaction.signal.is_cancelled() {
        return Err(AuthError::Cancelled);
    }
    let device = start_device_authorization(&oauth.oauth_host, &interaction.signal).await?;
    interaction.notify(AuthEvent::DeviceCode {
        user_code: device.user_code.clone(),
        verification_uri: device.verification_uri_complete.clone(),
        // The event fields are u64: fractional values truncate (disclosed);
        // the parse only forwards finite positive numbers.
        interval_seconds: Some(device.interval_seconds as u64),
        expires_in_seconds: Some(device.expires_in_seconds as u64),
    });
    let token = poll_for_token(&oauth.oauth_host, &device, &interaction.signal).await?;
    Ok(OAuthCredential {
        refresh: token.refresh,
        access: token.access,
        expires: token.expires,
        extra: BTreeMap::new(),
    })
}

impl OAuthAuth for KimiCodingOAuth {
    /// Upstream `name` (kimi-coding.ts:282).
    fn name(&self) -> &str {
        "Kimi Code (subscription)"
    }

    /// Upstream `isSubscription: true` (kimi-coding.ts:283).
    fn is_subscription(&self) -> bool {
        true
    }

    /// Upstream `loginLabel` (kimi-coding.ts:284).
    fn login_label(&self) -> Option<&str> {
        Some("Sign in with Kimi Code")
    }

    /// Upstream `login` (kimi-coding.ts:286).
    fn login<'a>(
        &'a self,
        interaction: ProviderAuthInteraction,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(login_kimi_coding(self, interaction))
    }

    /// Upstream `refresh` (kimi-coding.ts:288-291).
    fn refresh<'a>(
        &'a self,
        credential: OAuthCredential,
        options: &'a crate::ai::auth::types::AuthOperationOptions,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move {
            let signal = options.signal.clone().unwrap_or_default();
            let token = refresh_token(self, &credential.refresh, &signal).await?;
            Ok(OAuthCredential {
                refresh: token.refresh,
                access: token.access,
                expires: token.expires,
                extra: BTreeMap::new(),
            })
        })
    }

    /// Upstream `toAuth` (kimi-coding.ts:293-295):
    /// `{ headers: { Authorization: "Bearer " + access } }`.
    fn to_auth<'a>(
        &'a self,
        credential: OAuthCredential,
    ) -> BoxFuture<'a, Result<ModelAuth, AuthError>> {
        Box::pin(async move {
            Ok(ModelAuth {
                headers: Some(ProviderHeaders::from([(
                    "Authorization".to_string(),
                    Some(format!("Bearer {}", credential.access)),
                )])),
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
    use tokio_util::sync::CancellationToken;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::ai::auth::types::{AuthInteraction, AuthOperationOptions, AuthPrompt};

    type Respond =
        Box<dyn Fn(AuthPrompt) -> BoxFuture<'static, Result<String, AuthError>> + Send + Sync>;

    /// Minimal interaction: records events and prompts, answers prompts
    /// through the injected responder, and mirrors `device_code` events into
    /// a slot the tests can read while login is in flight.
    struct FakeInteraction {
        device_code: Arc<Mutex<Option<AuthEvent>>>,
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
            if matches!(event, AuthEvent::DeviceCode { .. }) {
                *self.device_code.lock().unwrap() = Some(event.clone());
            }
            self.events.lock().unwrap().push(event);
        }
    }

    fn fake_interaction(respond: Respond) -> (Arc<FakeInteraction>, ProviderAuthInteraction) {
        let fake = Arc::new(FakeInteraction {
            device_code: Arc::new(Mutex::new(None)),
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

    /// The Kimi Code flow never prompts (the oracle's prompt throws).
    fn never_prompt_interaction() -> (Arc<FakeInteraction>, ProviderAuthInteraction) {
        fake_interaction(Box::new(|prompt| {
            Box::pin(async move { panic!("unexpected prompt: {:?}", prompt.kind) })
        }))
    }

    fn device_code_body(overrides: &str) -> String {
        let mut body = String::from(
            "{\"user_code\":\"ABCD-1234\",\"device_code\":\"device-code-123\",\
             \"verification_uri\":\"https://www.kimi.com/code\",\
             \"verification_uri_complete\":\"https://www.kimi.com/code?user_code=ABCD-1234\",\
             \"interval\":1,\"expires_in\":600",
        );
        body.push_str(overrides);
        body.push('}');
        body
    }

    fn token_body(overrides: &str) -> String {
        format!(
            r#"{{"access_token":"access-token","refresh_token":"refresh-token","expires_in":3600{overrides}}}"#
        )
    }

    /// Serves the queued `(status, body)` responses in order — the wiremock
    /// analog of the oracle's `pollResponses.shift()`. A poll past the queue
    /// gets a 500, failing the test loudly.
    async fn mount_token_queue(server: &MockServer, responses: Vec<(u16, String)>) {
        let queue = Arc::new(Mutex::new(std::collections::VecDeque::from(responses)));
        Mock::given(method("POST"))
            .and(path("/api/oauth/token"))
            .respond_with(move |_request: &wiremock::Request| {
                let mut queue = queue.lock().unwrap();
                let (status, body) = queue
                    .pop_front()
                    .unwrap_or_else(|| (500, r#"{"error":"unexpected extra poll"}"#.to_string()));
                ResponseTemplate::new(status).set_body_raw(body, "application/json")
            })
            .mount(server)
            .await;
    }

    async fn mount_device_authorization(server: &MockServer, body: String) {
        Mock::given(method("POST"))
            .and(path("/api/oauth/device_authorization"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .expect(1)
            .mount(server)
            .await;
    }

    fn flow_with(server: &MockServer) -> KimiCodingOAuth {
        KimiCodingOAuth::with_host(server.uri(), Duration::from_millis(1000))
    }

    fn device_code_event(interaction: &FakeInteraction) -> AuthEvent {
        interaction
            .device_code
            .lock()
            .unwrap()
            .clone()
            .expect("login must emit a device_code event")
    }

    const DEVICE_AUTH_BODY: &str = "client_id=17e5f671-d194-4dfb-9706-5516cb48c098";
    const POLL_BODY: &str = "client_id=17e5f671-d194-4dfb-9706-5516cb48c098&\
device_code=device-code-123&grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code";
    const REFRESH_BODY: &str = "client_id=17e5f671-d194-4dfb-9706-5516cb48c098&\
grant_type=refresh_token&refresh_token=old-refresh";

    // ---- Oracle ports (packages/ai/test/kimi-coding-oauth.test.ts) ----

    /// Oracle: "logs in with the device authorization flow". Short real-time
    /// intervals (oracle: 5s + fake timers); the exact poll schedule is
    /// pinned by the shared engine's tests, so here we pin that the first
    /// poll waits out the interval (`waitBeforeFirstPoll`) and the wire
    /// shapes.
    #[tokio::test]
    async fn logs_in_with_the_device_authorization_flow() {
        let server = MockServer::start().await;
        mount_device_authorization(&server, device_code_body("")).await;
        mount_token_queue(
            &server,
            vec![
                (400, r#"{"error":"authorization_pending"}"#.to_string()),
                (200, token_body("")),
            ],
        )
        .await;
        let oauth = flow_with(&server);
        let (fake, interaction) = never_prompt_interaction();

        let started = std::time::Instant::now();
        let credential = tokio::time::timeout(Duration::from_secs(15), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap();
        let elapsed = started.elapsed();

        assert_eq!(credential.access, "access-token");
        assert_eq!(credential.refresh, "refresh-token");
        // expires = token response time + expires_in, with no skew.
        assert!((credential.expires - (now_ms() + 3_600_000)).abs() <= 2_000);

        // waitBeforeFirstPoll: the first poll happens after the interval.
        assert!(elapsed >= Duration::from_millis(1000), "{elapsed:?}");

        // The device_code event carries the complete verification URI.
        assert_eq!(
            device_code_event(&fake),
            AuthEvent::DeviceCode {
                user_code: "ABCD-1234".to_string(),
                verification_uri: "https://www.kimi.com/code?user_code=ABCD-1234".to_string(),
                interval_seconds: Some(1),
                expires_in_seconds: Some(600),
            }
        );

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 3);
        // Byte-pinned request bodies (upstream URLSearchParams insertion
        // order) and headers.
        assert_eq!(requests[0].url.path(), "/api/oauth/device_authorization");
        assert_eq!(
            String::from_utf8(requests[0].body.clone()).unwrap(),
            DEVICE_AUTH_BODY
        );
        assert_eq!(
            requests[0]
                .headers
                .get("Content-Type")
                .unwrap()
                .to_str()
                .unwrap(),
            "application/x-www-form-urlencoded"
        );
        assert_eq!(
            requests[0].headers.get("Accept").unwrap().to_str().unwrap(),
            "application/json"
        );
        for request in &requests[1..3] {
            assert_eq!(request.url.path(), "/api/oauth/token");
            assert_eq!(String::from_utf8(request.body.clone()).unwrap(), POLL_BODY);
        }
    }

    /// Oracle: "fails when the device code expires".
    #[tokio::test]
    async fn fails_when_the_device_code_expires() {
        let server = MockServer::start().await;
        mount_device_authorization(&server, device_code_body("")).await;
        mount_token_queue(
            &server,
            vec![(400, r#"{"error":"expired_token"}"#.to_string())],
        )
        .await;
        let oauth = flow_with(&server);
        let (_fake, interaction) = never_prompt_interaction();

        let error = tokio::time::timeout(Duration::from_secs(10), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation(
                "Kimi Code device authorization expired. Please restart login.".to_string()
            )
        );
    }

    /// Oracle: "fails when the user denies the login".
    #[tokio::test]
    async fn fails_when_the_user_denies_the_login() {
        let server = MockServer::start().await;
        mount_device_authorization(&server, device_code_body("")).await;
        mount_token_queue(
            &server,
            vec![(400, r#"{"error":"access_denied"}"#.to_string())],
        )
        .await;
        let oauth = flow_with(&server);
        let (_fake, interaction) = never_prompt_interaction();

        let error = tokio::time::timeout(Duration::from_secs(10), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("Kimi Code login was denied.".to_string())
        );
    }

    // ---- Poll-branch port coverage beyond the oracle ----

    /// Upstream `readJson` accepts arrays (`typeof [] === "object"`): a JSON
    /// array body reads as `Some`, field access on it misses like upstream's
    /// `undefined` property reads, and `JSON.stringify` renders `[]` — not
    /// `null` like the pre-parity port.
    #[test]
    fn parse_json_object_accepts_arrays_and_stringify_renders_them() {
        assert_eq!(stringify_json(parse_json_object("[]").as_ref()), "[]");
        assert_eq!(
            stringify_json(parse_json_object(r#"["a",1]"#).as_ref()),
            r#"["a",1]"#
        );
        // Field access on an array misses, like upstream property reads.
        assert_eq!(parse_json_object("[]").unwrap().get("device_code"), None);
        // Non-object scalars, `null` and unparseable bodies still read as None
        // (upstream `json && typeof json === "object"` falsy/typed out).
        for body in ["null", "\"text\"", "42", "true", "not json"] {
            assert_eq!(parse_json_object(body), None, "{body}");
            assert_eq!(stringify_json(parse_json_object(body).as_ref()), "null");
        }
    }

    /// A 5xx poll status fails the flow with the status and body text
    /// (upstream `response.status >= 500` branch).
    #[tokio::test]
    async fn server_error_poll_fails_with_the_status_and_text() {
        let server = MockServer::start().await;
        mount_device_authorization(&server, device_code_body("")).await;
        mount_token_queue(&server, vec![(502, "boom".to_string())]).await;
        let oauth = flow_with(&server);
        let (_fake, interaction) = never_prompt_interaction();

        let error = tokio::time::timeout(Duration::from_secs(10), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation(
                "Kimi Code device token request failed with status 502: boom".to_string()
            )
        );
    }

    /// An unknown error string carries the code and description; a missing
    /// or non-string error reports only the status (upstream final branch).
    #[tokio::test]
    async fn unknown_poll_errors_carry_the_code_and_description() {
        for (body, expected) in [
            (
                r#"{"error":"unusual_client","error_description":"nope"}"#.to_string(),
                "Kimi Code device token request failed (status 400): unusual_client: nope"
                    .to_string(),
            ),
            (
                r#"{"error":"unusual_client"}"#.to_string(),
                "Kimi Code device token request failed (status 400): unusual_client".to_string(),
            ),
            (
                "{}".to_string(),
                "Kimi Code device token request failed (status 400)".to_string(),
            ),
            (
                "not json".to_string(),
                "Kimi Code device token request failed (status 400)".to_string(),
            ),
        ] {
            let server = MockServer::start().await;
            mount_device_authorization(&server, device_code_body("")).await;
            mount_token_queue(&server, vec![(400, body.clone())]).await;
            let oauth = flow_with(&server);
            let (_fake, interaction) = never_prompt_interaction();

            let error = tokio::time::timeout(Duration::from_secs(10), oauth.login(interaction))
                .await
                .unwrap()
                .unwrap_err();
            assert_eq!(error, AuthError::Operation(expected), "{body}");
        }
    }

    /// A 200 token body with a string `access_token` but a missing
    /// `refresh_token` is a failed poll naming the fields.
    #[tokio::test]
    async fn incomplete_token_body_fails_the_poll() {
        let server = MockServer::start().await;
        mount_device_authorization(&server, device_code_body("")).await;
        mount_token_queue(&server, vec![(200, r#"{"access_token":"a"}"#.to_string())]).await;
        let oauth = flow_with(&server);
        let (_fake, interaction) = never_prompt_interaction();

        let error = tokio::time::timeout(Duration::from_secs(10), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation(
                r#"Kimi Code token poll response missing fields: {"access_token":"a"}"#.to_string()
            )
        );
    }

    /// A device authorization response with an untrusted verification URI
    /// fails with the serialized-body message (upstream `trustedHttpUrl`).
    #[tokio::test]
    async fn untrusted_verification_uris_fail_device_authorization() {
        for (field, value) in [
            ("verification_uri", "\"ftp://www.kimi.com/code\""),
            ("verification_uri_complete", "\"javascript:alert(1)\""),
            ("verification_uri", "\"not a url\""),
        ] {
            let server = MockServer::start().await;
            mount_device_authorization(&server, device_code_body(&format!(r#",{field}:{value}"#)))
                .await;
            let oauth = flow_with(&server);
            let (fake, interaction) = never_prompt_interaction();

            let error = oauth.login(interaction).await.unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("Invalid Kimi Code device authorization response"),
                "{field}: {error}"
            );
            assert!(fake.events.lock().unwrap().is_empty());
        }
    }

    /// Missing `interval`/`expires_in` fall back to the 5s/15min defaults in
    /// the emitted event (upstream `DEFAULT_POLL_INTERVAL_SECONDS` /
    /// `DEVICE_CODE_TIMEOUT_SECONDS`).
    #[tokio::test]
    async fn missing_interval_and_expiry_fall_back_to_the_defaults() {
        let server = MockServer::start().await;
        mount_device_authorization(
            &server,
            r#"{"user_code":"ABCD-1234","device_code":"device-code-123","verification_uri":"https://www.kimi.com/code","verification_uri_complete":"https://www.kimi.com/code?user_code=ABCD-1234"}"#
                .to_string(),
        )
        .await;
        mount_token_queue(&server, vec![(200, token_body(""))]).await;
        let oauth = flow_with(&server);
        let (fake, interaction) = never_prompt_interaction();

        tokio::time::timeout(Duration::from_secs(15), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            device_code_event(&fake),
            AuthEvent::DeviceCode {
                user_code: "ABCD-1234".to_string(),
                verification_uri: "https://www.kimi.com/code?user_code=ABCD-1234".to_string(),
                interval_seconds: Some(5),
                expires_in_seconds: Some(900),
            }
        );
    }

    // ---- Host override (oracle: "honors the KIMI_CODE_OAUTH_HOST override")

    /// Process env is process-global; serialize env-mutating tests and
    /// restore the saved values on drop (upstream `stubEnv`/`unstubAllEnvs`).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct TestEnv {
        _lock: std::sync::MutexGuard<'static, ()>,
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl TestEnv {
        /// Sets `settings`, removes `cleared`, restoring everything on drop.
        fn apply(settings: &[(&'static str, String)], cleared: &[&'static str]) -> Self {
            let lock = ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut saved = Vec::new();
            for (name, value) in settings {
                saved.push((*name, std::env::var(name).ok()));
                std::env::set_var(name, value);
            }
            for name in cleared {
                saved.push((*name, std::env::var(name).ok()));
                std::env::remove_var(name);
            }
            TestEnv { _lock: lock, saved }
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            for (name, value) in &self.saved {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    /// Oracle: "honors the KIMI_CODE_OAUTH_HOST override" — the override
    /// (with a trailing slash, exercising the strip) points every request at
    /// the stub server.
    #[tokio::test]
    async fn honors_the_kimi_code_oauth_host_override() {
        let server = MockServer::start().await;
        mount_device_authorization(&server, device_code_body("")).await;
        mount_token_queue(&server, vec![(200, token_body(""))]).await;
        let _env = TestEnv::apply(
            &[("KIMI_CODE_OAUTH_HOST", format!("{}/", server.uri()))],
            &["KIMI_OAUTH_HOST"],
        );

        let oauth = KimiCodingOAuth::new();
        let (_fake, interaction) = never_prompt_interaction();
        let credential = tokio::time::timeout(Duration::from_secs(15), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(credential.access, "access-token");
        assert_eq!(credential.refresh, "refresh-token");

        let requests = server.received_requests().await.unwrap();
        let paths: Vec<_> = requests
            .iter()
            .map(|request| request.url.path().to_string())
            .collect();
        assert_eq!(
            paths,
            vec![
                "/api/oauth/device_authorization".to_string(),
                "/api/oauth/token".to_string(),
            ]
        );
    }

    /// `KIMI_CODE_OAUTH_HOST` wins over `KIMI_OAUTH_HOST`.
    #[tokio::test]
    async fn kimi_code_oauth_host_wins_over_kimi_oauth_host() {
        let server = MockServer::start().await;
        mount_device_authorization(&server, device_code_body("")).await;
        mount_token_queue(&server, vec![(200, token_body(""))]).await;
        let decoy = MockServer::start().await;
        let _env = TestEnv::apply(
            &[
                ("KIMI_CODE_OAUTH_HOST", server.uri()),
                ("KIMI_OAUTH_HOST", decoy.uri()),
            ],
            &[],
        );

        let oauth = KimiCodingOAuth::new();
        let (_fake, interaction) = never_prompt_interaction();
        tokio::time::timeout(Duration::from_secs(15), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
        assert!(decoy.received_requests().await.unwrap().is_empty());
    }

    /// Only `KIMI_OAUTH_HOST` set: it is the fallback override.
    #[tokio::test]
    async fn kimi_oauth_host_is_the_fallback_override() {
        let server = MockServer::start().await;
        mount_device_authorization(&server, device_code_body("")).await;
        mount_token_queue(&server, vec![(200, token_body(""))]).await;
        let _env = TestEnv::apply(
            &[("KIMI_OAUTH_HOST", server.uri())],
            &["KIMI_CODE_OAUTH_HOST"],
        );

        let oauth = KimiCodingOAuth::new();
        let (_fake, interaction) = never_prompt_interaction();
        tokio::time::timeout(Duration::from_secs(15), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    // ---- Refresh (oracle: "refreshes tokens and returns a Bearer header"
    // and "retries refresh on 429 and fails unauthorized on invalid_grant")

    #[tokio::test]
    async fn refreshes_tokens_and_returns_a_bearer_header_for_requests() {
        let server = MockServer::start().await;
        mount_token_queue(
            &server,
            vec![(
                200,
                r#"{"access_token":"new-access","refresh_token":"new-refresh","expires_in":3600}"#
                    .to_string(),
            )],
        )
        .await;
        let oauth = flow_with(&server);

        let before = now_ms();
        let credential = oauth
            .refresh(
                OAuthCredential {
                    refresh: "old-refresh".to_string(),
                    access: "old-access".to_string(),
                    expires: before,
                    extra: Default::default(),
                },
                &AuthOperationOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(credential.access, "new-access");
        assert_eq!(credential.refresh, "new-refresh");
        assert!(credential.expires >= before + 3_600_000);

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].url.path(), "/api/oauth/token");
        assert_eq!(
            String::from_utf8(requests[0].body.clone()).unwrap(),
            REFRESH_BODY
        );

        let auth = oauth.to_auth(credential).await.unwrap();
        assert_eq!(
            auth,
            ModelAuth {
                api_key: None,
                headers: Some(ProviderHeaders::from([(
                    "Authorization".to_string(),
                    Some("Bearer new-access".to_string())
                )])),
                base_url: None,
            }
        );
    }

    /// Oracle: 429 once, then success — one retry after the 1s backoff.
    #[tokio::test]
    async fn retries_refresh_on_429() {
        let server = MockServer::start().await;
        mount_token_queue(
            &server,
            vec![
                (429, r#"{"error":"temporarily_unavailable"}"#.to_string()),
                (
                    200,
                    r#"{"access_token":"a","refresh_token":"r","expires_in":60}"#.to_string(),
                ),
            ],
        )
        .await;
        let oauth = flow_with(&server);

        let credential = oauth
            .refresh(
                OAuthCredential {
                    refresh: "old-refresh".to_string(),
                    access: "old-access".to_string(),
                    expires: 0,
                    extra: Default::default(),
                },
                &AuthOperationOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(credential.access, "a");
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    /// Oracle: invalid_grant is not retried — unauthorized carries the
    /// status (and description when present).
    #[tokio::test]
    async fn invalid_grant_fails_unauthorized_without_retrying() {
        let server = MockServer::start().await;
        mount_token_queue(
            &server,
            vec![(400, r#"{"error":"invalid_grant"}"#.to_string())],
        )
        .await;
        let oauth = flow_with(&server);
        let credential = OAuthCredential {
            refresh: "old-refresh".to_string(),
            access: "old-access".to_string(),
            expires: 0,
            extra: Default::default(),
        };

        let error = oauth
            .refresh(credential.clone(), &AuthOperationOptions::default())
            .await
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("Kimi Code token refresh unauthorized (status 400)".to_string())
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);

        // 403 with a description.
        let server = MockServer::start().await;
        mount_token_queue(
            &server,
            vec![(
                403,
                r#"{"error":"invalid_grant","error_description":"revoked"}"#.to_string(),
            )],
        )
        .await;
        let oauth = flow_with(&server);
        let error = oauth
            .refresh(credential, &AuthOperationOptions::default())
            .await
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation(
                "Kimi Code token refresh unauthorized (status 403): revoked".to_string()
            )
        );
    }

    /// Retryable 500s exhaust the retry budget (three retries after the
    /// initial attempt) and surface the final status with the serialized
    /// body. The backoff base is injected small so the test stays fast.
    #[tokio::test]
    async fn refresh_retry_budget_exhaustion_reports_the_last_status() {
        let server = MockServer::start().await;
        mount_token_queue(
            &server,
            vec![
                (500, r#"{"error":"temporarily_unavailable"}"#.to_string()),
                (500, r#"{"error":"temporarily_unavailable"}"#.to_string()),
                (500, r#"{"error":"temporarily_unavailable"}"#.to_string()),
                (500, r#"{"error":"temporarily_unavailable"}"#.to_string()),
            ],
        )
        .await;
        let oauth = KimiCodingOAuth::with_host(server.uri(), Duration::from_millis(1));

        let error = oauth
            .refresh(
                OAuthCredential {
                    refresh: "old-refresh".to_string(),
                    access: "old-access".to_string(),
                    expires: 0,
                    extra: Default::default(),
                },
                &AuthOperationOptions::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation(
                r#"Kimi Code token refresh failed with status 500: {"error":"temporarily_unavailable"}"#
                    .to_string()
            )
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 4);
    }

    // ---- Metadata, entry cancellation ----

    #[tokio::test]
    async fn metadata_matches_upstream() {
        let oauth = KimiCodingOAuth::new();
        assert_eq!(oauth.name(), "Kimi Code (subscription)");
        assert!(oauth.is_subscription());
        assert_eq!(oauth.login_label(), Some("Sign in with Kimi Code"));
    }

    #[test]
    fn production_constants_match_upstream() {
        assert_eq!(CLIENT_ID, "17e5f671-d194-4dfb-9706-5516cb48c098");
        assert_eq!(DEFAULT_OAUTH_HOST, "https://auth.kimi.com");
        assert_eq!(REQUEST_TIMEOUT, Duration::from_secs(30));
        assert_eq!(REFRESH_MAX_RETRIES, 3);
        assert_eq!(DEVICE_CODE_TIMEOUT_SECONDS, 900.0);
        assert_eq!(DEFAULT_POLL_INTERVAL_SECONDS, 5.0);
    }

    /// Entry cancellation: a pre-cancelled interaction signal never reaches
    /// the network (the port's login-entry short-circuit).
    #[tokio::test]
    async fn entry_cancelled_signal_short_circuits_login() {
        let server = MockServer::start().await;
        let oauth = flow_with(&server);
        let (_fake, interaction) = never_prompt_interaction();
        interaction.signal.cancel();

        let result = oauth.login(interaction).await;
        assert_eq!(result, Err(AuthError::Cancelled));
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
