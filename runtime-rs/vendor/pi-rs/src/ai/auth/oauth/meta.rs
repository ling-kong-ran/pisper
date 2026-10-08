//! Meta Model API OAuth flow, ported from upstream
//! `packages/ai/src/auth/oauth/meta.ts` (the auth delta): an RFC 8628 device
//! authorization grant against https://auth.meta.com (JSON responses),
//! followed by a Model API key minted from the identity token, exposed as
//! the [`MetaOAuth`] [`OAuthAuth`] implementation.
//!
//! Meta splits identity from API access: the identity token is not accepted
//! for inference, so it is exchanged for a Model API key via the Muse Code
//! key-mint endpoint (minted keys live about a day). The identity token is
//! stored as `refresh` and the minted key as `access`, so the standard OAuth
//! scheduler re-mints the key when it expires with no bespoke renewal
//! machinery. The identity token itself is not renewable (auth.meta.com
//! answers grant_type=refresh_token with 404 and issues no refresh_token), so
//! a 401/403 from mint means the session is dead and the user must sign in
//! again.
//!
//! Interactive surface (M2d ruling): the device code reaches the UI through
//! the [`AuthEvent::DeviceCode`] event, progress through
//! [`AuthEvent::Progress`] — the flow prompts nothing and never touches
//! stdio or a browser directly.
//!
//! Port notes (disclosed divergences):
//! - The endpoint URLs are fields on [`MetaOAuth`] (upstream: module
//!   constants) so tests can point the flow at a wiremock server. The
//!   production constructor pins the upstream values.
//! - JSON re-serialization in "invalid response" messages uses serde_json's
//!   map ordering (alphabetical) instead of the JS `JSON.stringify`
//!   insertion order (raw bodies in failure messages are preserved).
//! - The 30-second per-request budget (`AbortSignal.timeout(30_000)`) rides
//!   the reqwest per-request timeout; transport failures carry the port's
//!   transport error text where upstream carries Node's fetch rejection
//!   text.
//! - `AuthEvent::DeviceCode.interval_seconds`/`expires_in_seconds` are
//!   `u64`, so fractional server values report truncated (integers in
//!   practice). Test injections may scale the polling schedule down without
//!   changing the reported values (see [`MetaOAuth::with_interval_scale`]).

use std::collections::BTreeMap;
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::ai::api::http_client;
use crate::ai::auth::types::{
    AuthError, AuthEvent, AuthInteraction, ModelAuth, OAuthAuth, OAuthCredential,
    ProviderAuthInteraction,
};
use crate::ai::now_ms;

use super::device_code::{poll_device_code_flow, PollOutcome};

/// Muse Code CLI client id (upstream `CLIENT_ID`).
const CLIENT_ID: &str = "1031625952748946";

/// Upstream `AUTH_HOST` (referenced by the endpoint constants below; pinned
/// by the tests).
#[allow(dead_code)]
const AUTH_HOST: &str = "https://auth.meta.com";

/// Upstream `DEVICE_AUTHORIZATION_URL`; the production default.
const DEVICE_AUTHORIZATION_URL: &str = "https://auth.meta.com/oidc/device/authorization/";

/// Upstream `DEVICE_TOKEN_URL`; the production default.
const DEVICE_TOKEN_URL: &str = "https://auth.meta.com/oidc/device/token/";

/// Upstream `API_KEY_MINT_URL`; the production default.
const API_KEY_MINT_URL: &str = "https://api.meta.ai/muse-code/key";

/// Upstream `API_KEY_LIFETIME_MS` (minted keys live about a day).
const API_KEY_LIFETIME_MS: i64 = 24 * 60 * 60 * 1000;

/// Upstream `REQUEST_TIMEOUT_MS` (every request carries the 30s budget).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The device authorization offer (upstream `DeviceAuthorization`).
struct DeviceAuthorization {
    device_code: String,
    user_code: String,
    verification_uri: String,
    interval_seconds: Option<f64>,
    expires_in_seconds: Option<f64>,
}

/// Upstream `requestSignal`: the per-request 30s budget composed with the
/// interaction signal. The budget rides the reqwest timeout; the signal is
/// selected against.
fn request_deadline() -> tokio::time::Instant {
    tokio::time::Instant::now() + REQUEST_TIMEOUT
}

/// Upstream `readJson`: a JSON object body, `None` on any parse failure
/// (tolerated — the error paths render `null`).
async fn read_json(response: reqwest::Response) -> Result<Option<Value>, AuthError> {
    let text = response
        .text()
        .await
        .map_err(|error| AuthError::Operation(error.to_string()))?;
    match serde_json::from_str::<Value>(&text) {
        Ok(value @ Value::Object(_)) => Ok(Some(value)),
        Ok(_) => Ok(None),
        Err(_) => Ok(None),
    }
}

/// Upstream `errorDetail`: the first non-empty-trimmed string of
/// `error_description`, `detail`, `message`, `error`, rendered as
/// `: {value}`.
fn error_detail(json: Option<&Value>) -> String {
    if let Some(json) = json {
        for key in ["error_description", "detail", "message", "error"] {
            if let Some(Value::String(value)) = json.get(key) {
                if !value.trim().is_empty() {
                    return format!(": {}", value.trim());
                }
            }
        }
    }
    String::new()
}

/// Upstream `trustedHttpUrl`: only http(s) URLs are trusted for the
/// browser-facing verification URI.
fn trusted_http_url(value: Option<&Value>) -> Option<String> {
    let value = value?.as_str()?;
    if value.is_empty() {
        return None;
    }
    let url = url::Url::parse(value).ok()?;
    if url.scheme() != "https" && url.scheme() != "http" {
        return None;
    }
    Some(url.to_string())
}

/// Upstream `positiveNumber`.
fn positive_number(value: Option<&Value>) -> Option<f64> {
    match value.and_then(Value::as_f64) {
        Some(number) if number.is_finite() && number > 0.0 => Some(number),
        _ => None,
    }
}

/// Upstream `startDeviceAuthorization` (meta.ts:86-114).
async fn start_device_authorization(
    device_authorization_url: &str,
    signal: &CancellationToken,
) -> Result<DeviceAuthorization, AuthError> {
    let deadline = request_deadline();
    let body = {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("client_id", CLIENT_ID);
        query.finish()
    };
    let request = http_client()
        .post(device_authorization_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .body(body)
        .timeout(REQUEST_TIMEOUT);
    let response = tokio::select! {
        biased;
        _ = signal.cancelled() => return Err(AuthError::Cancelled),
        _ = tokio::time::sleep_until(deadline) => {
            return Err(AuthError::Operation(
                "Meta device authorization timed out".to_string(),
            ))
        }
        response = request.send() => match response {
            Ok(response) => response,
            Err(error) => return Err(AuthError::Operation(error.to_string())),
        },
    };
    let status = response.status().as_u16();
    let json = read_json(response).await?;
    if !(200..300).contains(&status) {
        return Err(AuthError::Operation(format!(
            "Meta device authorization failed with status {status}{}",
            error_detail(json.as_ref())
        )));
    }
    let device_code = json
        .as_ref()
        .and_then(|json| json.get("device_code"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let user_code = json
        .as_ref()
        .and_then(|json| json.get("user_code"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    // `trustedHttpUrl(verification_uri_complete) ?? trustedHttpUrl(verification_uri)`.
    let verification_uri = json.as_ref().and_then(|json| {
        trusted_http_url(json.get("verification_uri_complete"))
            .or_else(|| trusted_http_url(json.get("verification_uri")))
    });
    let (Some(device_code), Some(user_code), Some(verification_uri)) =
        (device_code, user_code, verification_uri)
    else {
        return Err(AuthError::Operation(format!(
            "Invalid Meta device authorization response: {}",
            serde_json::to_string(&json).unwrap_or_else(|_| "null".to_string())
        )));
    };
    Ok(DeviceAuthorization {
        device_code: device_code.to_string(),
        user_code: user_code.to_string(),
        verification_uri,
        // The raw server values (reported verbatim in the device-code
        // event); the polling schedule applies [`MetaOAuth`]'s test-only
        // scale separately.
        interval_seconds: positive_number(json.as_ref().and_then(|json| json.get("interval"))),
        expires_in_seconds: positive_number(json.as_ref().and_then(|json| json.get("expires_in"))),
    })
}

/// One device-token poll (the `poll` closure for
/// [`poll_device_code_flow`], upstream meta.ts:117-166).
async fn poll_identity_token(
    device_token_url: &str,
    device_code: &str,
    interval_scale: f64,
    signal: &CancellationToken,
) -> Result<PollOutcome<String>, AuthError> {
    let deadline = request_deadline();
    let body = {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("grant_type", "urn:ietf:params:oauth:grant-type:device_code");
        query.append_pair("device_code", device_code);
        query.append_pair("client_id", CLIENT_ID);
        query.finish()
    };
    let request = http_client()
        .post(device_token_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .body(body)
        .timeout(REQUEST_TIMEOUT);
    let response = tokio::select! {
        biased;
        _ = signal.cancelled() => return Err(AuthError::Cancelled),
        _ = tokio::time::sleep_until(deadline) => {
            return Err(AuthError::Operation(
                "Meta device token request timed out".to_string(),
            ))
        }
        response = request.send() => match response {
            Ok(response) => response,
            Err(error) => return Err(AuthError::Operation(error.to_string())),
        },
    };
    let status = response.status().as_u16();
    let json = read_json(response).await?;

    let access_token = json
        .as_ref()
        .and_then(|json| json.get("access_token"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    if (200..300).contains(&status) {
        if let Some(access_token) = access_token {
            return Ok(PollOutcome::Complete(access_token.to_string()));
        }
    }
    // `switch (json?.error)`.
    let error = json
        .as_ref()
        .and_then(|json| json.get("error"))
        .and_then(Value::as_str);
    match error {
        Some("authorization_pending") => Ok(PollOutcome::Pending),
        Some("slow_down") => Ok(PollOutcome::SlowDown(
            positive_number(json.as_ref().and_then(|json| json.get("interval")))
                .map(|seconds| seconds * interval_scale),
        )),
        Some("access_denied") => Ok(PollOutcome::Failed("Meta login was denied.".to_string())),
        Some("expired_token") => Ok(PollOutcome::Failed(
            "Meta device authorization expired. Please restart login.".to_string(),
        )),
        _ => Ok(PollOutcome::Failed(format!(
            "Meta device token request failed with status {status}{}",
            error_detail(json.as_ref())
        ))),
    }
}

/// Upstream `mintApiKey` (meta.ts:178-216): exchange the identity token for a
/// Model API key (valid for about a day).
async fn mint_api_key(
    mint_url: &str,
    identity_token: &str,
    signal: &CancellationToken,
) -> Result<OAuthCredential, AuthError> {
    let deadline = request_deadline();
    let request = http_client()
        .post(mint_url)
        .header("Accept", "application/json")
        .header("Authorization", format!("Bearer {identity_token}"))
        .header("Content-Type", "application/json")
        .header("x-api-version", "1.0.0")
        .body("{}")
        .timeout(REQUEST_TIMEOUT);
    let response = tokio::select! {
        biased;
        _ = signal.cancelled() => return Err(AuthError::Cancelled),
        _ = tokio::time::sleep_until(deadline) => {
            return Err(AuthError::Operation(
                "Meta API key mint timed out".to_string(),
            ))
        }
        response = request.send() => match response {
            Ok(response) => response,
            Err(error) => return Err(AuthError::Operation(error.to_string())),
        },
    };
    let status = response.status().as_u16();
    let json = read_json(response).await?;

    if status == 401 || status == 403 {
        // Identity token is not renewable (see the module notes); only a
        // fresh device flow helps.
        return Err(AuthError::Operation(format!(
            "Meta session expired (status {status}). Run `/login meta` to sign in again.{}",
            error_detail(json.as_ref())
        )));
    }
    if !(200..300).contains(&status) {
        return Err(AuthError::Operation(format!(
            "Meta API key mint failed with status {status}{}",
            error_detail(json.as_ref())
        )));
    }
    let api_key = json
        .as_ref()
        .and_then(|json| json.get("api_key"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let Some(api_key) = api_key else {
        let action_url = json
            .as_ref()
            .and_then(|json| trusted_http_url(json.get("action_url")));
        return Err(AuthError::Operation(match action_url {
            Some(action_url) => {
                format!("Meta did not issue an API key. Complete setup at {action_url}")
            }
            None => "Meta did not issue an API key.".to_string(),
        }));
    };
    Ok(OAuthCredential {
        refresh: identity_token.to_string(),
        access: api_key.to_string(),
        expires: now_ms() + API_KEY_LIFETIME_MS,
        extra: BTreeMap::new(),
    })
}

/// Upstream `loginMeta` (meta.ts:219-236).
async fn login_meta(
    oauth: &MetaOAuth,
    interaction: ProviderAuthInteraction,
) -> Result<OAuthCredential, AuthError> {
    let outcome = async {
        let device =
            start_device_authorization(&oauth.device_authorization_url, &interaction.signal)
                .await?;
        interaction.notify(AuthEvent::DeviceCode {
            user_code: device.user_code.clone(),
            verification_uri: device.verification_uri.clone(),
            interval_seconds: device.interval_seconds.map(|seconds| seconds as u64),
            expires_in_seconds: device.expires_in_seconds.map(|seconds| seconds as u64),
        });
        // The polling schedule scales only under test (see
        // [`MetaOAuth::with_interval_scale`]); the event values above stay
        // the server's own.
        let scheduled_interval = device
            .interval_seconds
            .map(|seconds| seconds * oauth.interval_scale);
        let scheduled_expires = device
            .expires_in_seconds
            .map(|seconds| seconds * oauth.interval_scale);
        let identity_token = poll_device_code_flow(
            scheduled_interval,
            scheduled_expires,
            true,
            &interaction.signal,
            || {
                poll_identity_token(
                    &oauth.device_token_url,
                    &device.device_code,
                    oauth.interval_scale,
                    &interaction.signal,
                )
            },
        )
        .await?;
        interaction.notify(AuthEvent::Progress {
            message: "Enabling Meta Model API access...".to_string(),
        });
        mint_api_key(
            &oauth.api_key_mint_url,
            &identity_token,
            &interaction.signal,
        )
        .await
    }
    .await;
    // An in-flight fetch rejects on abort; the login UI matches on the
    // cancelled surface (upstream: `if (interaction.signal.aborted) throw new
    // Error("Login cancelled")`).
    match outcome {
        Ok(credential) => Ok(credential),
        Err(error) => {
            if interaction.signal.is_cancelled() {
                Err(AuthError::Cancelled)
            } else {
                Err(error)
            }
        }
    }
}

/// The Meta OAuth auth surface (upstream `metaOAuth`, meta.ts:238-248).
/// [`MetaOAuth::new`] pins the upstream endpoints; tests inject wiremock
/// URLs.
pub struct MetaOAuth {
    device_authorization_url: String,
    device_token_url: String,
    api_key_mint_url: String,
    /// Test-only scaling of the polling schedule (the reported event values
    /// stay unscaled), so oracle tests replay the fixture without real
    /// seconds of waiting.
    interval_scale: f64,
}

impl Default for MetaOAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl MetaOAuth {
    /// Upstream module constants.
    pub fn new() -> Self {
        MetaOAuth {
            device_authorization_url: DEVICE_AUTHORIZATION_URL.to_string(),
            device_token_url: DEVICE_TOKEN_URL.to_string(),
            api_key_mint_url: API_KEY_MINT_URL.to_string(),
            interval_scale: 1.0,
        }
    }

    /// Test constructor: point the endpoints at a stub server (upstream
    /// tests stub the global `fetch`).
    #[cfg(test)]
    pub(crate) fn with_endpoints(
        device_authorization_url: String,
        device_token_url: String,
        api_key_mint_url: String,
    ) -> Self {
        MetaOAuth {
            device_authorization_url,
            device_token_url,
            api_key_mint_url,
            interval_scale: 1.0,
        }
    }

    /// Test-only polling-schedule scale (see the struct field notes).
    #[cfg(test)]
    pub(crate) fn with_interval_scale(mut self, scale: f64) -> Self {
        self.interval_scale = scale;
        self
    }
}

impl OAuthAuth for MetaOAuth {
    /// Upstream `name` (meta.ts:239).
    fn name(&self) -> &str {
        "Meta (Muse subscription)"
    }

    /// Upstream `isSubscription: true` (meta.ts:240).
    fn is_subscription(&self) -> bool {
        true
    }

    /// Upstream `loginLabel` (meta.ts:241).
    fn login_label(&self) -> Option<&str> {
        Some("Sign in with Meta")
    }

    /// Upstream `login` (meta.ts:243).
    fn login<'a>(
        &'a self,
        interaction: ProviderAuthInteraction,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(login_meta(self, interaction))
    }

    /// Upstream `refresh` (meta.ts:245): re-mint the key from the stored
    /// identity token.
    fn refresh<'a>(
        &'a self,
        credential: OAuthCredential,
        options: &'a crate::ai::auth::types::AuthOperationOptions,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move {
            let signal = options.signal.clone().unwrap_or_default();
            mint_api_key(&self.api_key_mint_url, &credential.refresh, &signal).await
        })
    }

    /// Upstream `toAuth` (meta.ts:246-248): `{ apiKey: credential.access }`.
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
    use crate::ai::auth::types::{AuthInteraction, AuthOperationOptions, AuthPrompt};
    use std::sync::Arc;
    use std::sync::Mutex;

    struct RecordingInteraction {
        events: Mutex<Vec<AuthEvent>>,
        signal: CancellationToken,
    }

    impl AuthInteraction for RecordingInteraction {
        fn signal(&self) -> Option<CancellationToken> {
            Some(self.signal.clone())
        }

        fn prompt(&self, _prompt: AuthPrompt) -> BoxFuture<'_, Result<String, AuthError>> {
            Box::pin(async {
                Err(AuthError::Operation(
                    "Meta login should not prompt".to_string(),
                ))
            })
        }

        fn notify(&self, event: AuthEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    fn interaction() -> (Arc<RecordingInteraction>, ProviderAuthInteraction) {
        let fake = Arc::new(RecordingInteraction {
            events: Mutex::new(Vec::new()),
            signal: CancellationToken::new(),
        });
        let normalized = ProviderAuthInteraction::new(
            Arc::clone(&fake) as Arc<dyn AuthInteraction>,
            fake.signal.clone(),
        );
        (fake, normalized)
    }

    #[tokio::test]
    async fn production_endpoints_match_upstream() {
        let oauth = MetaOAuth::new();
        assert_eq!(oauth.name(), "Meta (Muse subscription)");
        assert!(oauth.is_subscription());
        assert_eq!(oauth.login_label(), Some("Sign in with Meta"));
        assert_eq!(CLIENT_ID, "1031625952748946");
        assert_eq!(AUTH_HOST, "https://auth.meta.com");
        assert_eq!(
            DEVICE_AUTHORIZATION_URL,
            "https://auth.meta.com/oidc/device/authorization/"
        );
        assert_eq!(DEVICE_TOKEN_URL, "https://auth.meta.com/oidc/device/token/");
        assert_eq!(API_KEY_MINT_URL, "https://api.meta.ai/muse-code/key");
        assert_eq!(API_KEY_LIFETIME_MS, 24 * 60 * 60 * 1000);
        assert_eq!(REQUEST_TIMEOUT, Duration::from_secs(30));
    }

    #[test]
    fn error_detail_follows_the_key_precedence() {
        let json = serde_json::json!({
            "error": "e",
            "message": "m",
            "detail": "d",
            "error_description": "ed",
        });
        assert_eq!(error_detail(Some(&json)), ": ed");
        let json = serde_json::json!({ "error": "e", "message": "  " });
        assert_eq!(error_detail(Some(&json)), ": e");
        let json = serde_json::json!({ "detail": "  d  " });
        assert_eq!(error_detail(Some(&json)), ": d");
        assert_eq!(error_detail(Some(&serde_json::json!(null))), "");
        assert_eq!(error_detail(None), "");
    }

    #[test]
    fn trusted_http_url_rejects_non_http_schemes_and_garbage() {
        assert_eq!(
            trusted_http_url(Some(&Value::String(
                "https://auth.meta.com/device/?code=X".to_string()
            ))),
            Some("https://auth.meta.com/device/?code=X".to_string())
        );
        assert_eq!(
            trusted_http_url(Some(&Value::String("javascript:alert(1)".to_string()))),
            None
        );
        assert_eq!(
            trusted_http_url(Some(&Value::String("not a url".to_string()))),
            None
        );
        assert_eq!(trusted_http_url(Some(&Value::String(String::new()))), None);
        assert_eq!(trusted_http_url(Some(&Value::Bool(true))), None);
        assert_eq!(trusted_http_url(None), None);
    }

    #[test]
    fn positive_number_requires_finite_positive() {
        assert_eq!(positive_number(Some(&Value::from(5))), Some(5.0));
        assert_eq!(positive_number(Some(&Value::from(0))), None);
        assert_eq!(positive_number(Some(&Value::from(-1))), None);
        assert_eq!(positive_number(Some(&Value::from("5"))), None);
        assert_eq!(positive_number(None), None);
    }

    #[tokio::test]
    async fn to_auth_derives_the_request_api_key_from_the_minted_key() {
        let oauth = MetaOAuth::new();
        let auth = oauth
            .to_auth(OAuthCredential {
                refresh: "identity-token".to_string(),
                access: "LLM|key".to_string(),
                expires: 1,
                extra: BTreeMap::new(),
            })
            .await
            .unwrap();
        assert_eq!(auth.api_key.as_deref(), Some("LLM|key"));
    }

    #[tokio::test]
    async fn mint_errors_carry_the_upstream_messages() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let base = reqwest::Url::parse(&server.uri()).unwrap();
        // 401 → the session-expired message, with the detail suffix.
        Mock::given(method("POST"))
            .and(path("/muse-code/key"))
            .respond_with(
                ResponseTemplate::new(401)
                    .set_body_string(r#"{"error":"invalid_token","error_description":"expired"}"#),
            )
            .mount(&server)
            .await;
        let oauth = MetaOAuth::with_endpoints(
            format!("{base}nothing"),
            format!("{base}nothing"),
            format!("{}/muse-code/key", server.uri()),
        );
        let signal = CancellationToken::new();
        let error = mint_api_key(&oauth.api_key_mint_url, "identity", &signal)
            .await
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation(
                "Meta session expired (status 401). Run `/login meta` to sign in again.: expired"
                    .to_string()
            )
        );
    }

    #[tokio::test]
    async fn mint_reports_the_setup_url_when_no_key_is_issued() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let base = reqwest::Url::parse(&server.uri()).unwrap();
        Mock::given(method("POST"))
            .and(path("/muse-code/key"))
            .and(header("authorization", "Bearer identity-token"))
            .and(header("x-api-version", "1.0.0"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"require_payment":true,"action_url":"https://dev.meta.ai/billing"}"#,
            ))
            .mount(&server)
            .await;
        let oauth = MetaOAuth::with_endpoints(
            format!("{base}nothing"),
            format!("{base}nothing"),
            format!("{}/muse-code/key", server.uri()),
        );
        let error = mint_api_key(
            &oauth.api_key_mint_url,
            "identity-token",
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation(
                "Meta did not issue an API key. Complete setup at https://dev.meta.ai/billing"
                    .to_string()
            )
        );
        // The minted-key happy path stores the identity token as refresh.
        let server = MockServer::start().await;
        let base = reqwest::Url::parse(&server.uri()).unwrap();
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(r#"{"api_key":"LLM|minted-key"}"#),
            )
            .mount(&server)
            .await;
        let oauth = MetaOAuth::with_endpoints(
            format!("{base}nothing"),
            format!("{base}nothing"),
            format!("{}/key", server.uri()),
        );
        let credential = mint_api_key(
            &oauth.api_key_mint_url,
            "identity-token",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(credential.refresh, "identity-token");
        assert_eq!(credential.access, "LLM|minted-key");
        assert!((credential.expires - (now_ms() + API_KEY_LIFETIME_MS)).abs() <= 2_000);
    }

    /// Oracle: "logs in with the device flow and mints a Model API key" —
    /// pending poll, then the identity token, then the mint; the device-code
    /// event carries the complete verification URI.
    #[tokio::test]
    async fn logs_in_through_the_device_flow_and_mints_a_key() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let base = reqwest::Url::parse(&server.uri()).unwrap();
        Mock::given(method("POST"))
            .and(path("/oidc/device/authorization/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"device_code":"device-code-123","user_code":"ABCD-1234","verification_uri":"https://auth.meta.com/oauth/device/","verification_uri_complete":"https://auth.meta.com/oauth/device/?code=ABCD-1234","interval":5,"expires_in":600}"#,
            ))
            .mount(&server)
            .await;
        // First poll pending, second completes.
        let poll_state = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let poll_state_for_mount = Arc::clone(&poll_state);
        Mock::given(method("POST"))
            .and(path("/oidc/device/token/"))
            .respond_with(move |_request: &wiremock::Request| {
                let count = poll_state_for_mount.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if count == 0 {
                    ResponseTemplate::new(400)
                        .set_body_string(r#"{"error":"authorization_pending"}"#)
                } else {
                    ResponseTemplate::new(200).set_body_string(
                        r#"{"access_token":"identity-token","token_type":"Bearer"}"#,
                    )
                }
            })
            .mount(&server)
            .await;
        let mint_base = reqwest::Url::parse(&server.uri()).unwrap();
        let (fake, interaction) = interaction();
        let oauth = MetaOAuth::with_endpoints(
            format!("{base}oidc/device/authorization/"),
            format!("{base}oidc/device/token/"),
            format!("{mint_base}muse-code/key"),
        )
        .with_interval_scale(0.05);
        Mock::given(method("POST"))
            .and(path("/muse-code/key"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(r#"{"api_key":"LLM|minted-key"}"#),
            )
            .mount(&server)
            .await;

        let credential = tokio::time::timeout(Duration::from_secs(10), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap();

        let events = fake.events.lock().unwrap().clone();
        assert_eq!(
            events[0],
            AuthEvent::DeviceCode {
                user_code: "ABCD-1234".to_string(),
                verification_uri: "https://auth.meta.com/oauth/device/?code=ABCD-1234".to_string(),
                interval_seconds: Some(5),
                expires_in_seconds: Some(600),
            }
        );
        assert!(matches!(
            events[1],
            AuthEvent::Progress { ref message }
                if message == "Enabling Meta Model API access..."
        ));
        assert_eq!(credential.refresh, "identity-token");
        assert_eq!(credential.access, "LLM|minted-key");
        // Two polls happened (pending, then complete).
        assert_eq!(poll_state.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    /// Oracle: "re-mints the API key from the stored identity token on
    /// refresh".
    #[tokio::test]
    async fn refresh_re_mints_from_the_stored_identity_token() {
        use wiremock::matchers::{header, method};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("authorization", "Bearer identity-token"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(r#"{"api_key":"LLM|fresh-key"}"#),
            )
            .mount(&server)
            .await;
        let base = reqwest::Url::parse(&server.uri()).unwrap();
        let oauth = MetaOAuth::with_endpoints(
            format!("{base}nothing"),
            format!("{base}nothing"),
            format!("{base}muse-code/key"),
        );

        let credential = oauth
            .refresh(
                OAuthCredential {
                    refresh: "identity-token".to_string(),
                    access: "LLM|old-key".to_string(),
                    expires: 1,
                    extra: BTreeMap::new(),
                },
                &AuthOperationOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(credential.refresh, "identity-token");
        assert_eq!(credential.access, "LLM|fresh-key");
        assert!((credential.expires - (now_ms() + API_KEY_LIFETIME_MS)).abs() <= 2_000);
    }

    /// Access denied and expired device codes surface their upstream
    /// messages through the poll engine.
    #[tokio::test]
    async fn device_poll_failures_carry_the_upstream_messages() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        for (body, expected) in [
            (r#"{"error":"access_denied"}"#, "Meta login was denied."),
            (
                r#"{"error":"expired_token"}"#,
                "Meta device authorization expired. Please restart login.",
            ),
            (
                r#"{"error":"weird_thing","message":"boom"}"#,
                "Meta device token request failed with status 400: boom",
            ),
        ] {
            let server = MockServer::start().await;
            let base = reqwest::Url::parse(&server.uri()).unwrap();
            Mock::given(method("POST"))
                .and(path("/oidc/device/authorization/"))
                .respond_with(ResponseTemplate::new(200).set_body_string(
                    r#"{"device_code":"d","user_code":"U","verification_uri":"https://meta.example/device","interval":5,"expires_in":600}"#,
                ))
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/oidc/device/token/"))
                .respond_with(ResponseTemplate::new(400).set_body_string(body))
                .mount(&server)
                .await;
            let (fake, interaction) = interaction();
            let oauth = MetaOAuth::with_endpoints(
                format!("{base}oidc/device/authorization/"),
                format!("{base}oidc/device/token/"),
                "https://mint.example/key".to_string(),
            )
            .with_interval_scale(0.05);
            let error = tokio::time::timeout(Duration::from_secs(5), oauth.login(interaction))
                .await
                .unwrap()
                .unwrap_err();
            assert_eq!(error, AuthError::Operation(expected.to_string()), "{body}");
            let _ = &fake;
        }
    }

    /// A denied/cancelled login normalizes to the cancelled surface.
    #[tokio::test]
    async fn cancelled_interaction_signal_surfaces_as_cancelled() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let base = reqwest::Url::parse(&server.uri()).unwrap();
        Mock::given(method("POST"))
            .and(path("/oidc/device/authorization/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"device_code":"d","user_code":"U","verification_uri":"https://meta.example/device","interval":5,"expires_in":600}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/oidc/device/token/"))
            .respond_with(
                ResponseTemplate::new(400).set_body_string(r#"{"error":"authorization_pending"}"#),
            )
            .mount(&server)
            .await;
        let (fake, interaction) = interaction();
        {
            let signal = fake.signal.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                signal.cancel();
            });
        }
        let oauth = MetaOAuth::with_endpoints(
            format!("{base}oidc/device/authorization/"),
            format!("{base}oidc/device/token/"),
            "https://mint.example/key".to_string(),
        )
        .with_interval_scale(0.05);
        let error = tokio::time::timeout(Duration::from_secs(5), oauth.login(interaction))
            .await
            .unwrap();
        assert_eq!(error, Err(AuthError::Cancelled));
    }
}
