//! Radius gateway OAuth flow ported from upstream
//! `packages/ai/src/auth/oauth/radius.ts`: Radius is a pi-messages gateway
//! whose OAuth client APIs live on the configured gateway; only the
//! interactive browser authorization endpoint is discovered from
//! `/v1/oauth`. Login offers a browser PKCE callback (loopback server on
//! 127.0.0.1:1456) and a device-code fallback, exposed through
//! [`create_radius_oauth`] as a per-provider [`RadiusOAuth`] [`OAuthAuth`].
//!
//! Interactive surface (M2d ruling): the login method is chosen through the
//! interaction's `select` prompt, the browser gets the authorize URL via
//! [`AuthEvent::AuthUrl`] and progress via [`AuthEvent::Progress`] — the
//! flow never touches stdio or a browser directly. The loopback callback
//! server is flow-owned infrastructure, like upstream's `http.createServer`.
//!
//! Port notes (disclosed divergences):
//! - The browser login runs on the shared [`super::callback_server`]
//!   (upstream `callback-server.ts`, which this delta refactored the private
//!   `startOAuthCallbackServer` onto). The token exchange moved INSIDE the
//!   callback handler (`complete`), so the browser page shows its outcome:
//!   200 "Signed in to Radius. You may now close this page." or 502
//!   "Radius sign-in failed." with the exchange error. A provider-reported
//!   redirect error now fails the login with "Radius authorization failed:
//!   {description}", and a bind failure propagates (upstream awaited the
//!   server promise) instead of degrading to a "OAuth callback did not
//!   complete." error.
//! - The callback port is a field on [`RadiusOAuth`] (upstream constant
//!   1456) so tests can bind a free port; the production constructor pins
//!   the upstream value.
//! - Upstream trusts the token response shape (`as { access_token: string,
//!   ... }`) and stores `undefined` when fields are missing; the port names
//!   the first missing field as an operation error instead — the port's
//!   credential type cannot hold `undefined`.
//! - Upstream device-authorization truthiness checks (`!data.device_code`)
//!   accept any truthy value; the port requires non-empty strings and a
//!   positive numeric `expires_in` (same missing-fields error).
//! - `new URL(authorizationEndpoint)` on a discovery endpoint that is not a
//!   URL throws `Invalid URL` upstream; the port surfaces `Invalid Radius
//!   OAuth authorization endpoint: {raw}`.
//! - Cancellation maps to [`AuthError::Cancelled`] everywhere upstream
//!   throws `Error("Login cancelled")` (port contract: interaction-signal
//!   aborts are never wrapped).
//! - `crypto.randomUUID()` becomes a random RFC 4122 version-4 UUID built
//!   from 16 `rand` bytes (see [`super::uuid_v4`]); the oracle tests inject
//!   the capture's fixed UUID through [`super::test_entropy`].

use std::collections::BTreeMap;
use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::ai::api::http_client;
use crate::ai::auth::types::{
    AuthError, AuthEvent, AuthInteraction, AuthPrompt, AuthPromptKind, AuthPromptOption, ModelAuth,
    OAuthAuth, OAuthCredential, ProviderAuthInteraction,
};
use crate::ai::now_ms;

use super::callback_server::{start_oauth_callback_server, CallbackServerOptions};
use super::device_code::{poll_device_code_flow, PollOutcome};
use super::pkce::{generate_pkce, Pkce};
use super::uuid_v4;

/// Upstream `CALLBACK_HOST` (radius.ts:26).
const CALLBACK_HOST: &str = "127.0.0.1";

/// Upstream `CALLBACK_PORT` (radius.ts:27); the production default for
/// [`RadiusOAuth::callback_port`].
const CALLBACK_PORT: u16 = 1456;

/// Upstream `CALLBACK_PATH` (radius.ts:28).
const CALLBACK_PATH: &str = "/oauth/callback";

/// Upstream `TOKEN_EXPIRY_SKEW_MS` (radius.ts:30).
const TOKEN_EXPIRY_SKEW_MS: i64 = 60_000;

/// Upstream `LOGIN_METHOD_BROWSER` (radius.ts:31).
const LOGIN_METHOD_BROWSER: &str = "browser";

/// Upstream `LOGIN_METHOD_DEVICE_CODE` (radius.ts:32).
const LOGIN_METHOD_DEVICE_CODE: &str = "device-code";

/// Upstream `OAUTH_CLIENT_ID` (radius.ts:33).
const OAUTH_CLIENT_ID: &str = "pi-gateway";

/// Upstream `OAUTH_SCOPE` (radius.ts:34).
const OAUTH_SCOPE: &str = "gateway offline_access";

/// Upstream `OAUTH_DEVICE_CODE_GRANT_TYPE` (radius.ts:35).
const OAUTH_DEVICE_CODE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// Upstream `RadiusOAuthOptions` (radius.ts:352-355).
pub struct RadiusOAuthOptions {
    pub name: String,
    pub gateway: String,
}

/// The Radius gateway OAuth auth surface for one provider (upstream
/// `createRadiusOAuth`, radius.ts:357-403).
pub struct RadiusOAuth {
    name: String,
    gateway: String,
    /// Test-only override of the fixed 1456 callback port.
    callback_port: u16,
}

/// Upstream `normalizeRadiusGatewayUrl`
/// (`providers/radius-config.ts:52-55`): default the scheme to https and
/// strip trailing slashes. The full radius-config port lands with provider
/// wiring; the OAuth flow needs exactly this one helper.
pub(crate) fn normalize_radius_gateway_url(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    let with_scheme = if lower.starts_with("http://") || lower.starts_with("https://") {
        value.to_string()
    } else {
        format!("https://{value}")
    };
    with_scheme.trim_end_matches('/').to_string()
}

/// Upstream `createRadiusOAuth(options)` (radius.ts:357): normalize the
/// gateway once, then build the auth surface.
pub fn create_radius_oauth(options: RadiusOAuthOptions) -> RadiusOAuth {
    let gateway = normalize_radius_gateway_url(&options.gateway);
    RadiusOAuth {
        name: options.name,
        gateway,
        callback_port: CALLBACK_PORT,
    }
}

impl RadiusOAuth {
    /// Test constructor: bind a specific callback port (upstream tests never
    /// exercise the browser path; the port tests need a free port).
    #[cfg(test)]
    pub(crate) fn with_callback_port(name: String, gateway: String, callback_port: u16) -> Self {
        RadiusOAuth {
            name,
            gateway: normalize_radius_gateway_url(&gateway),
            callback_port,
        }
    }

    /// Upstream `REDIRECT_URI` (radius.ts:29).
    fn redirect_uri(&self) -> String {
        format!(
            "http://{CALLBACK_HOST}:{}{CALLBACK_PATH}",
            self.callback_port
        )
    }
}

/// `application/x-www-form-urlencoded` body from `new URLSearchParams({...})`
/// insertion order.
fn form_body(pairs: &[(&str, &str)]) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    for (name, value) in pairs {
        query.append_pair(name, value);
    }
    query.finish()
}

/// Resolves a gateway-relative endpoint (`new URL(path, gateway)`); the raw
/// parse error text is the upstream throw.
fn gateway_join(gateway: &str, path: &str) -> Result<url::Url, String> {
    let base = url::Url::parse(gateway).map_err(|error| error.to_string())?;
    base.join(path).map_err(|error| error.to_string())
}

/// Upstream `OAuthResponseError` (radius.ts:68-82): the parsed `error` code
/// and the composed `"{message}: {detail}"` message. Upstream also carries
/// `status` for its callers; the port's callers read only the message and
/// code, so it is not kept.
struct OAuthResponseError {
    oauth_error: Option<String>,
    message: String,
}

/// Upstream `readOAuthResponseError` (radius.ts:84-100): the body's
/// `error`/`error_description` JSON strings, else the raw body text as the
/// description, composed as `error: description` / `error` / description /
/// status.
fn oauth_response_error(status: u16, text: &str, message: &str) -> OAuthResponseError {
    let (oauth_error, mut description) = if text.is_empty() {
        (None, None)
    } else {
        match serde_json::from_str::<Value>(text) {
            // `JSON.parse` succeeded: the string-typed fields win (a
            // non-object body carries neither).
            Ok(data) => (
                data.get("error")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                data.get("error_description")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            ),
            // Parse failure: the raw text becomes the description.
            Err(_) => (None, Some(text.to_string())),
        }
    };
    // `description ? ... : ...` and `description || String(status)` are
    // truthiness checks: an empty description reads as absent.
    if description.as_deref().is_some_and(str::is_empty) {
        description = None;
    }
    let detail = match &oauth_error {
        Some(oauth_error) => match &description {
            Some(description) => format!("{oauth_error}: {description}"),
            None => oauth_error.clone(),
        },
        None => description.unwrap_or_else(|| status.to_string()),
    };
    OAuthResponseError {
        oauth_error,
        message: format!("{message}: {detail}"),
    }
}

/// The failure channels of [`request_oauth_token`] (upstream: the caught
/// fetch rejection, the uncaught `response.json()` rejection, and the
/// thrown `OAuthResponseError` are distinguished by the device-code poll).
enum TokenRequestError {
    /// Upstream catch: `signal.aborted` → "Login cancelled".
    Cancelled,
    /// Upstream catch: everything else rethrows (transport), and
    /// `new URL(...)` throws before the request.
    Transport(String),
    /// Upstream `await response.json()` rejection (SyntaxError).
    Body(String),
    /// Upstream `!response.ok` → `OAuthResponseError`.
    Response(OAuthResponseError),
}

impl From<TokenRequestError> for AuthError {
    fn from(error: TokenRequestError) -> Self {
        match error {
            TokenRequestError::Cancelled => AuthError::Cancelled,
            TokenRequestError::Transport(text) | TokenRequestError::Body(text) => {
                AuthError::Operation(text)
            }
            TokenRequestError::Response(error) => AuthError::Operation(error.message),
        }
    }
}

/// Upstream `requestOAuthToken` (radius.ts:102-140): POST the form body to
/// `{gateway}/v1/oauth/token`, racing the interaction signal.
async fn request_oauth_token(
    gateway: &str,
    body: String,
    signal: &CancellationToken,
) -> Result<OAuthCredential, TokenRequestError> {
    let url = gateway_join(gateway, "/v1/oauth/token").map_err(TokenRequestError::Transport)?;
    let request = http_client()
        .post(url.as_str())
        .header("accept", "application/json")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body);

    let response = tokio::select! {
        biased;
        _ = signal.cancelled() => return Err(TokenRequestError::Cancelled),
        response = request.send() => match response {
            Ok(response) => response,
            Err(error) => return Err(TokenRequestError::Transport(error.to_string())),
        },
    };

    if !response.status().is_success() {
        let status = response.status().as_u16();
        // `await response.text().catch(() => "")`.
        let text = response.text().await.unwrap_or_default();
        return Err(TokenRequestError::Response(oauth_response_error(
            status,
            &text,
            "Radius OAuth token request failed",
        )));
    }

    let text = response
        .text()
        .await
        .map_err(|error| TokenRequestError::Transport(error.to_string()))?;
    let data: Value =
        serde_json::from_str(&text).map_err(|error| TokenRequestError::Body(error.to_string()))?;
    // Upstream trusts the shape and would store `undefined`; the port names
    // the first missing field (module port notes).
    let missing = |field: &str| {
        TokenRequestError::Body(format!(
            "Radius OAuth token response missing field: {field}"
        ))
    };
    let access = data
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| missing("access_token"))?;
    let refresh = data
        .get("refresh_token")
        .and_then(Value::as_str)
        .ok_or_else(|| missing("refresh_token"))?;
    let expires_in = data
        .get("expires_in")
        .and_then(Value::as_f64)
        .ok_or_else(|| missing("expires_in"))?;

    let mut extra = BTreeMap::new();
    // `scope: data.scope` — `undefined` drops from the credential, `null`
    // stays.
    if let Some(scope) = data.get("scope").filter(|scope| !scope.is_null()) {
        extra.insert("scope".to_string(), scope.clone());
    }
    Ok(OAuthCredential {
        access: access.to_string(),
        refresh: refresh.to_string(),
        expires: now_ms() + (expires_in * 1000.0) as i64 - TOKEN_EXPIRY_SKEW_MS,
        extra,
    })
}

/// Upstream `RadiusOAuthDiscovery` (radius.ts:37-39): only the interactive
/// browser authorization endpoint is discovered.
async fn load_radius_oauth_discovery(
    gateway: &str,
    signal: &CancellationToken,
) -> Result<String, AuthError> {
    let url = gateway_join(gateway, "/v1/oauth").map_err(AuthError::Operation)?;
    let request = http_client()
        .get(url.as_str())
        .header("accept", "application/json");
    let response = tokio::select! {
        biased;
        _ = signal.cancelled() => return Err(AuthError::Cancelled),
        response = request.send() => match response {
            Ok(response) => response,
            Err(error) => return Err(AuthError::Operation(error.to_string())),
        },
    };

    if !response.status().is_success() {
        let status = response.status().as_u16();
        // `${await response.text()}` — a read failure propagates.
        let text = response
            .text()
            .await
            .map_err(|error| AuthError::Operation(error.to_string()))?;
        return Err(AuthError::Operation(format!(
            "Could not load Radius OAuth config from {gateway}: {status} {text}"
        )));
    }

    let text = response
        .text()
        .await
        .map_err(|error| AuthError::Operation(error.to_string()))?;
    let data: Value =
        serde_json::from_str(&text).map_err(|error| AuthError::Operation(error.to_string()))?;
    data.get("authorizationEndpoint")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AuthError::Operation(format!("Invalid Radius OAuth config from {gateway}")))
}

/// Upstream `loginWithBrowser` (radius.ts:148-190): PKCE against the
/// discovered authorization endpoint, with the token exchange running inside
/// the shared callback handler so the browser page shows its outcome.
async fn login_with_browser(
    oauth: &RadiusOAuth,
    authorization_endpoint: &str,
    interaction: ProviderAuthInteraction,
) -> Result<OAuthCredential, AuthError> {
    let Pkce {
        verifier,
        challenge,
    } = generate_pkce();
    let state = uuid_v4();
    let redirect_uri = oauth.redirect_uri();
    let authorize_url = {
        // `new URL(authorizationEndpoint)`; `authorizeUrl.search = ...`
        // replaces any existing query.
        let mut url = url::Url::parse(authorization_endpoint).map_err(|_| {
            AuthError::Operation(format!(
                "Invalid Radius OAuth authorization endpoint: {authorization_endpoint}"
            ))
        })?;
        let query = form_body(&[
            ("response_type", "code"),
            ("client_id", OAUTH_CLIENT_ID),
            ("redirect_uri", &redirect_uri),
            ("scope", OAUTH_SCOPE),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("handoff", "url"),
            ("state", &state),
        ]);
        url.set_query(Some(&query));
        url.to_string()
    };

    // Upstream `complete: (code) => requestOAuthToken(gateway,
    // new URLSearchParams({ grant_type: "authorization_code", ... }),
    // interaction.signal)` — the browser page shows the exchange outcome.
    let exchange_gateway = Arc::new(oauth.gateway.clone());
    let exchange_verifier = Arc::new(verifier.clone());
    let exchange_redirect = Arc::new(redirect_uri.clone());
    let exchange_signal = interaction.signal.clone();
    let callback = start_oauth_callback_server(CallbackServerOptions {
        provider_name: "Radius".to_string(),
        host: CALLBACK_HOST.to_string(),
        port: oauth.callback_port,
        path: CALLBACK_PATH.to_string(),
        redirect_host: None,
        state: Some(state),
        complete: Arc::new(move |code| {
            let gateway = Arc::clone(&exchange_gateway);
            let verifier = Arc::clone(&exchange_verifier);
            let redirect = Arc::clone(&exchange_redirect);
            let signal = exchange_signal.clone();
            Box::pin(async move {
                let body = form_body(&[
                    ("grant_type", "authorization_code"),
                    ("client_id", OAUTH_CLIENT_ID),
                    ("redirect_uri", &redirect),
                    ("code", &code),
                    ("code_verifier", &verifier),
                ]);
                request_oauth_token(&gateway, body, &signal)
                    .await
                    .map_err(AuthError::from)
            })
        }),
        signal: interaction.signal.clone(),
        timeout_ms: None,
    })
    .await?;
    interaction.notify(AuthEvent::Progress {
        message: format!("Listening for OAuth callback on {redirect_uri}"),
    });
    interaction.notify(AuthEvent::AuthUrl {
        url: authorize_url,
        instructions: Some("Continue in your browser.".to_string()),
    });

    let result = async {
        // Upstream `const credential = await callback.wait(); if (!credential)
        // throw new Error("OAuth callback did not complete.")` — the null
        // settle is unreachable through this flow (nothing calls `cancel`).
        let credential = callback.wait().await;
        match credential {
            // The null settle cannot happen here; a cancelled interaction
            // signal surfaces as the cancellation (upstream: the "Login
            // cancelled" rejection).
            Ok(Some(credential)) => Ok(credential),
            Ok(None) | Err(AuthError::Cancelled) => Err(AuthError::Cancelled),
            Err(AuthError::Operation(message)) => Err(AuthError::Operation(message)),
            Err(error) => Err(error),
        }
    }
    .await;

    // Upstream `finally { callback.close() }`.
    callback.close().await;
    result
}

/// Upstream `DeviceAuthorizationResponse` (radius.ts:41-47).
struct DeviceAuthorization {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: f64,
    interval: Option<f64>,
}

/// Upstream `requestDeviceAuthorization` (radius.ts:271-303).
async fn request_device_authorization(
    gateway: &str,
    signal: &CancellationToken,
) -> Result<DeviceAuthorization, AuthError> {
    let url = gateway_join(gateway, "/v1/oauth/device").map_err(AuthError::Operation)?;
    let request = http_client()
        .post(url.as_str())
        .header("accept", "application/json")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form_body(&[
            ("client_id", OAUTH_CLIENT_ID),
            ("scope", OAUTH_SCOPE),
        ]));

    let response = tokio::select! {
        biased;
        _ = signal.cancelled() => return Err(AuthError::Cancelled),
        response = request.send() => match response {
            Ok(response) => response,
            Err(error) => return Err(AuthError::Operation(error.to_string())),
        },
    };

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        return Err(AuthError::Operation(
            oauth_response_error(status, &text, "Radius OAuth device authorization failed").message,
        ));
    }

    let text = response
        .text()
        .await
        .map_err(|error| AuthError::Operation(error.to_string()))?;
    let data: Value =
        serde_json::from_str(&text).map_err(|error| AuthError::Operation(error.to_string()))?;
    // `!data.device_code || !data.user_code || !data.verification_uri ||
    // !data.expires_in` truthiness; the port requires non-empty strings and
    // a positive number (module port notes).
    let missing = || {
        AuthError::Operation(
            "Radius OAuth device authorization response is missing required fields".to_string(),
        )
    };
    let device_code = data
        .get("device_code")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(missing)?;
    let user_code = data
        .get("user_code")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(missing)?;
    let verification_uri = data
        .get("verification_uri")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(missing)?;
    let expires_in = data
        .get("expires_in")
        .and_then(Value::as_f64)
        .filter(|value| *value > 0.0)
        .ok_or_else(missing)?;
    Ok(DeviceAuthorization {
        device_code: device_code.to_string(),
        user_code: user_code.to_string(),
        verification_uri: verification_uri.to_string(),
        expires_in,
        interval: data.get("interval").and_then(Value::as_f64),
    })
}

/// Upstream `loginWithDeviceCode` (radius.ts:305-350): the first poll is
/// immediate; `authorization_pending`/`slow_down` keep polling, the RFC
/// 8628 terminal errors fail with the upstream messages.
async fn login_with_device_code(
    oauth: &RadiusOAuth,
    interaction: ProviderAuthInteraction,
) -> Result<OAuthCredential, AuthError> {
    let device = request_device_authorization(&oauth.gateway, &interaction.signal).await?;
    interaction.notify(AuthEvent::DeviceCode {
        user_code: device.user_code.clone(),
        verification_uri: device.verification_uri.clone(),
        // The event fields are u64: fractional values truncate (disclosed;
        // values are integral in practice).
        interval_seconds: device.interval.map(|interval| interval as u64),
        expires_in_seconds: Some(device.expires_in as u64),
    });

    let gateway = oauth.gateway.as_str();
    let signal = &interaction.signal;
    let grant_body = form_body(&[
        ("grant_type", OAUTH_DEVICE_CODE_GRANT_TYPE),
        ("client_id", OAUTH_CLIENT_ID),
        ("device_code", &device.device_code),
    ]);
    poll_device_code_flow(
        device.interval,
        Some(device.expires_in),
        false,
        &interaction.signal,
        || async {
            match request_oauth_token(gateway, grant_body.clone(), signal).await {
                Ok(credentials) => Ok(PollOutcome::Complete(credentials)),
                Err(TokenRequestError::Response(error)) => {
                    match error.oauth_error.as_deref() {
                        Some("authorization_pending") => Ok(PollOutcome::Pending),
                        Some("slow_down") => Ok(PollOutcome::SlowDown(None)),
                        Some("expired_token") => Ok(PollOutcome::Failed(
                            "Device authorization expired.".to_string(),
                        )),
                        Some("access_denied") => Ok(PollOutcome::Failed(
                            "Device authorization was denied.".to_string(),
                        )),
                        // Upstream rethrows other OAuth errors.
                        _ => Err(AuthError::Operation(error.message.clone())),
                    }
                }
                Err(other) => Err(AuthError::from(other)),
            }
        },
    )
    .await
}

/// Upstream `login` (radius.ts:363-384): prompt for the method, then dispatch.
async fn login_radius(
    oauth: &RadiusOAuth,
    interaction: ProviderAuthInteraction,
) -> Result<OAuthCredential, AuthError> {
    if interaction.signal.is_cancelled() {
        return Err(AuthError::Cancelled);
    }
    let login_method = interaction
        .prompt(AuthPrompt {
            signal: None,
            kind: AuthPromptKind::Select {
                message: format!("Sign in to {}:", oauth.name),
                options: vec![
                    AuthPromptOption {
                        id: LOGIN_METHOD_BROWSER.to_string(),
                        label: "Sign in with browser (recommended)".to_string(),
                        description: None,
                    },
                    AuthPromptOption {
                        id: LOGIN_METHOD_DEVICE_CODE.to_string(),
                        label: "Sign in with device code (when signing in from another device)"
                            .to_string(),
                        description: None,
                    },
                ],
            },
        })
        .await?;

    match login_method.as_str() {
        LOGIN_METHOD_DEVICE_CODE => login_with_device_code(oauth, interaction).await,
        LOGIN_METHOD_BROWSER => {
            let discovery =
                load_radius_oauth_discovery(&oauth.gateway, &interaction.signal).await?;
            login_with_browser(oauth, &discovery, interaction).await
        }
        other => Err(AuthError::Operation(format!(
            "Unknown {} sign-in method: {other}",
            oauth.name
        ))),
    }
}

impl OAuthAuth for RadiusOAuth {
    /// Upstream `name: options.name` (radius.ts:360).
    fn name(&self) -> &str {
        &self.name
    }

    /// Upstream `login` (radius.ts:363).
    fn login<'a>(
        &'a self,
        interaction: ProviderAuthInteraction,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(login_radius(self, interaction))
    }

    /// Upstream `refresh` (radius.ts:386-397): directly through the gateway,
    /// without discovery.
    fn refresh<'a>(
        &'a self,
        credential: OAuthCredential,
        options: &'a crate::ai::auth::types::AuthOperationOptions,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move {
            let signal = options.signal.clone().unwrap_or_default();
            let body = form_body(&[
                ("grant_type", "refresh_token"),
                ("client_id", OAUTH_CLIENT_ID),
                ("refresh_token", &credential.refresh),
            ]);
            request_oauth_token(&self.gateway, body, &signal)
                .await
                .map_err(AuthError::from)
        })
    }

    /// Upstream `toAuth` (radius.ts:399-401): `{ apiKey: credential.access }`.
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

    const GATEWAY: &str = "https://radius.example";

    type Respond =
        Box<dyn Fn(AuthPrompt) -> BoxFuture<'static, Result<String, AuthError>> + Send + Sync>;

    /// Minimal interaction: records events and prompts, answers prompts
    /// through the injected responder, and mirrors `auth_url` and
    /// `device_code` events into slots the tests can read while login is in
    /// flight.
    struct FakeInteraction {
        auth_url: Arc<Mutex<Option<String>>>,
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
            match &event {
                AuthEvent::AuthUrl { url, .. } => {
                    *self.auth_url.lock().unwrap() = Some(url.clone());
                }
                AuthEvent::DeviceCode { .. } => {
                    *self.device_code.lock().unwrap() = Some(event.clone());
                }
                _ => {}
            }
            self.events.lock().unwrap().push(event);
        }
    }

    fn fake_interaction(respond: Respond) -> (Arc<FakeInteraction>, ProviderAuthInteraction) {
        let fake = Arc::new(FakeInteraction {
            auth_url: Arc::new(Mutex::new(None)),
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

    /// The oracle's interaction: prompts resolve with the fixed login method.
    fn login_method_interaction(
        method: &'static str,
    ) -> (Arc<FakeInteraction>, ProviderAuthInteraction) {
        fake_interaction(Box::new(move |_prompt| {
            Box::pin(async move { Ok(method.to_string()) })
        }))
    }

    fn flow() -> RadiusOAuth {
        create_radius_oauth(RadiusOAuthOptions {
            name: "Radius".to_string(),
            gateway: GATEWAY.to_string(),
        })
    }

    fn flow_with_port(gateway: &str, port: u16) -> RadiusOAuth {
        RadiusOAuth::with_callback_port("Radius".to_string(), gateway.to_string(), port)
    }

    fn json_body(body: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_raw(body.to_string(), "application/json")
    }

    async fn mount_device(server: &MockServer, body: &str) {
        Mock::given(method("POST"))
            .and(path("/v1/oauth/device"))
            .respond_with(json_body(body))
            .mount(server)
            .await;
    }

    async fn mount_token(server: &MockServer, body: &str, status: u16) {
        Mock::given(method("POST"))
            .and(path("/v1/oauth/token"))
            .respond_with(
                ResponseTemplate::new(status).set_body_raw(body.to_string(), "application/json"),
            )
            .mount(server)
            .await;
    }

    /// Serves the queued `(status, body)` token responses in order — the
    /// wiremock analog of a shifting mock. A poll past the queue gets a 500.
    async fn mount_token_queue(server: &MockServer, responses: Vec<(u16, String)>) {
        let queue = Arc::new(Mutex::new(std::collections::VecDeque::from(responses)));
        Mock::given(method("POST"))
            .and(path("/v1/oauth/token"))
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

    async fn mount_discovery(server: &MockServer, body: &str, status: u16) {
        Mock::given(method("GET"))
            .and(path("/v1/oauth"))
            .respond_with(
                ResponseTemplate::new(status).set_body_raw(body.to_string(), "application/json"),
            )
            .mount(server)
            .await;
    }

    const TOKEN_SUCCESS: &str = r#"{"access_token":"access-token","refresh_token":"refresh-token","expires_in":3600,"scope":"gateway offline_access"}"#;

    /// A free loopback port for the injected callback server.
    fn free_port() -> u16 {
        std::net::TcpListener::bind((CALLBACK_HOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// GETs a raw HTTP target against the loopback callback server and
    /// returns the full response.
    async fn http_get(port: u16, target: &str) -> String {
        let mut stream = tokio::net::TcpStream::connect((CALLBACK_HOST, port))
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

    /// Waits for the auth_url event and returns the callback port and the
    /// minted state.
    async fn wait_for_auth_url(slot: Arc<Mutex<Option<String>>>) -> (u16, String) {
        loop {
            if let Some(auth_url) = slot.lock().unwrap().clone() {
                let parsed = Url::parse(&auth_url).unwrap();
                let state = parsed
                    .query_pairs()
                    .find(|(key, _)| key == "state")
                    .map(|(_, value)| value.into_owned())
                    .expect("missing state parameter");
                let redirect = parsed
                    .query_pairs()
                    .find(|(key, _)| key == "redirect_uri")
                    .map(|(_, value)| value.into_owned())
                    .expect("missing redirect_uri parameter");
                let redirect = Url::parse(&redirect).unwrap();
                break (redirect.port().unwrap(), state);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    // ---- Oracle ports (packages/ai/test/radius-oauth.test.ts) ----

    /// Oracle: "uses gateway endpoints directly for device login".
    #[tokio::test]
    async fn uses_gateway_endpoints_directly_for_device_login() {
        let server = MockServer::start().await;
        let oauth = create_radius_oauth(RadiusOAuthOptions {
            name: "Radius".to_string(),
            gateway: server.uri(),
        });
        mount_device(
            &server,
            r#"{"device_code":"device-code","user_code":"ABCD-1234","verification_uri":"https://radius-ui.example/pair","expires_in":600,"interval":5}"#,
        )
        .await;
        mount_token(&server, TOKEN_SUCCESS, 200).await;
        let (fake, interaction) = login_method_interaction(LOGIN_METHOD_DEVICE_CODE);

        let credential = tokio::time::timeout(Duration::from_secs(10), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap();

        assert_eq!(credential.access, "access-token");
        assert_eq!(credential.refresh, "refresh-token");
        // expires = response time + expires_in - 60s skew.
        assert!((credential.expires - (now_ms() + 3_600_000 - 60_000)).abs() <= 2_000);
        assert_eq!(
            credential.extra.get("scope").and_then(Value::as_str),
            Some("gateway offline_access")
        );

        // The device_code event carries the upstream payload.
        assert_eq!(
            fake.device_code.lock().unwrap().clone(),
            Some(AuthEvent::DeviceCode {
                user_code: "ABCD-1234".to_string(),
                verification_uri: "https://radius-ui.example/pair".to_string(),
                interval_seconds: Some(5),
                expires_in_seconds: Some(600),
            })
        );

        // Device login hits the gateway endpoints directly: no discovery.
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].url.path(), "/v1/oauth/device");
        assert_eq!(
            String::from_utf8(requests[0].body.clone()).unwrap(),
            "client_id=pi-gateway&scope=gateway+offline_access"
        );
        assert_eq!(requests[1].url.path(), "/v1/oauth/token");
        assert_eq!(
            String::from_utf8(requests[1].body.clone()).unwrap(),
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code&\
             client_id=pi-gateway&device_code=device-code"
        );
    }

    /// Oracle: "refreshes directly through the gateway without discovery".
    #[tokio::test]
    async fn refreshes_directly_through_the_gateway_without_discovery() {
        let server = MockServer::start().await;
        mount_token(
            &server,
            r#"{"access_token":"new-access","refresh_token":"new-refresh","expires_in":3600}"#,
            200,
        )
        .await;
        let oauth = create_radius_oauth(RadiusOAuthOptions {
            name: "Radius".to_string(),
            gateway: server.uri(),
        });

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
        assert_eq!(credential.access, "new-access");
        assert_eq!(credential.refresh, "new-refresh");

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].url.path(), "/v1/oauth/token");
        assert_eq!(
            String::from_utf8(requests[0].body.clone()).unwrap(),
            "grant_type=refresh_token&client_id=pi-gateway&refresh_token=old-refresh"
        );
    }

    /// Oracle: "discovers only the interactive browser authorization
    /// endpoint" — a discovery body without `authorizationEndpoint` fails
    /// login before any browser step.
    #[tokio::test]
    async fn discovers_only_the_browser_authorization_endpoint() {
        let server = MockServer::start().await;
        mount_discovery(&server, r#"{"issuer":"https://radius-ui.example"}"#, 200).await;
        let oauth = create_radius_oauth(RadiusOAuthOptions {
            name: "Radius".to_string(),
            gateway: server.uri(),
        });
        let (_fake, interaction) = login_method_interaction(LOGIN_METHOD_BROWSER);

        let error = tokio::time::timeout(Duration::from_secs(10), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation(format!("Invalid Radius OAuth config from {}", server.uri()))
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    // ---- Browser path port coverage (the oracle never exercises it) ----

    /// The full browser flow: discovery, PKCE authorize URL, loopback
    /// callback with code + state, and the gateway token exchange.
    #[tokio::test]
    async fn browser_flow_exchanges_the_callback_code_through_the_gateway() {
        let server = MockServer::start().await;
        mount_discovery(
            &server,
            r#"{"authorizationEndpoint":"https://radius-ui.example/authorize"}"#,
            200,
        )
        .await;
        mount_token(&server, TOKEN_SUCCESS, 200).await;
        let port = free_port();
        let oauth = flow_with_port(&server.uri(), port);
        let (fake, interaction) = login_method_interaction(LOGIN_METHOD_BROWSER);

        let login = tokio::spawn(async move { oauth.login(interaction).await });
        let (callback_port, state) = wait_for_auth_url(Arc::clone(&fake.auth_url)).await;
        assert_eq!(callback_port, port);
        let response = http_get(
            port,
            &format!("/oauth/callback?code=the-code&state={state}"),
        )
        .await;

        let credential = tokio::time::timeout(Duration::from_secs(10), login)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(credential.access, "access-token");
        assert_eq!(credential.refresh, "refresh-token");

        // The browser got the success page.
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        assert!(response.contains("Content-Type: text/html; charset=utf-8"));
        assert!(response.contains("<h1>Authentication successful</h1>"));
        assert!(response.contains("Signed in to Radius."));

        // The authorize URL shape: PKCE S256, the fixed client and scope,
        // the handoff flag and the redirect URI.
        let auth_url = auth_url_of(&fake);
        let parsed = Url::parse(&auth_url).unwrap();
        assert_eq!(parsed.scheme(), "https");
        assert_eq!(parsed.host_str(), Some("radius-ui.example"));
        assert_eq!(parsed.path(), "/authorize");
        let param = |name: &str| {
            parsed
                .query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
                .unwrap_or_default()
        };
        assert_eq!(param("response_type"), "code");
        assert_eq!(param("client_id"), "pi-gateway");
        assert_eq!(param("scope"), "gateway offline_access");
        assert_eq!(param("code_challenge_method"), "S256");
        assert_eq!(param("handoff"), "url");
        assert_eq!(
            param("redirect_uri"),
            format!("http://127.0.0.1:{port}/oauth/callback")
        );

        // The exchange carries the code and the verifier matching the
        // authorize URL's challenge; the events announce the listener and
        // the URL.
        let requests = server.received_requests().await.unwrap();
        let token_request = requests
            .iter()
            .find(|request| request.url.path() == "/v1/oauth/token")
            .unwrap();
        let form = String::from_utf8(token_request.body.clone()).unwrap();
        let verifier = form
            .split('&')
            .find(|pair| pair.starts_with("code_verifier="))
            .unwrap()
            .trim_start_matches("code_verifier=");
        assert!(form.starts_with(
            "grant_type=authorization_code&client_id=pi-gateway&\
             redirect_uri=http%3A%2F%2F127.0.0.1%3A"
        ));
        assert!(form.contains("&code=the-code&code_verifier="));
        assert_eq!(
            param("code_challenge"),
            base64url_encode(&Sha256::digest(verifier.as_bytes()))
        );
        let events = fake.events.lock().unwrap().clone();
        assert!(matches!(
            events.first(),
            Some(AuthEvent::Progress { message })
                if *message == format!("Listening for OAuth callback on http://127.0.0.1:{port}/oauth/callback")
        ));
        assert!(
            matches!(events.get(1), Some(AuthEvent::AuthUrl { instructions, .. })
                if instructions.as_deref() == Some("Continue in your browser."))
        );
    }

    /// A state mismatch gets the 400 page and the login keeps waiting.
    #[tokio::test]
    async fn state_mismatch_keeps_the_login_waiting() {
        let server = MockServer::start().await;
        mount_discovery(
            &server,
            r#"{"authorizationEndpoint":"https://radius-ui.example/authorize"}"#,
            200,
        )
        .await;
        let port = free_port();
        let oauth = flow_with_port(&server.uri(), port);
        let (fake, interaction) = login_method_interaction(LOGIN_METHOD_BROWSER);
        let signal = interaction.signal.clone();

        let login = tokio::spawn(async move { oauth.login(interaction).await });
        let (_, state) = wait_for_auth_url(Arc::clone(&fake.auth_url)).await;
        let response = http_get(
            port,
            &format!("/oauth/callback?code=the-code&state=wrong-{}", &state[..4]),
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 400 Bad Request\r\n"));
        assert!(response.contains("State mismatch."));

        // Still waiting: only cancellation settles the login.
        signal.cancel();
        let result = tokio::time::timeout(Duration::from_secs(10), login)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.unwrap_err(), AuthError::Cancelled);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    /// An OAuth error redirect gets the 400 page with the description and
    /// fails the login ("OAuth callback did not complete.").
    #[tokio::test]
    async fn oauth_error_redirect_fails_the_login() {
        let server = MockServer::start().await;
        mount_discovery(
            &server,
            r#"{"authorizationEndpoint":"https://radius-ui.example/authorize"}"#,
            200,
        )
        .await;
        let port = free_port();
        let oauth = flow_with_port(&server.uri(), port);
        let (fake, interaction) = login_method_interaction(LOGIN_METHOD_BROWSER);

        let login = tokio::spawn(async move { oauth.login(interaction).await });
        let (_, state) = wait_for_auth_url(Arc::clone(&fake.auth_url)).await;
        let response = http_get(
            port,
            &format!("/oauth/callback?error=access_denied&error_description=nope&state={state}"),
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 400 Bad Request\r\n"));
        assert!(response.contains("nope"));

        let result = tokio::time::timeout(Duration::from_secs(10), login)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            result.unwrap_err(),
            AuthError::Operation("Radius authorization failed: nope".to_string())
        );
    }

    /// A wrong callback route gets the 404 page and the login keeps waiting;
    /// after cancellation the callback port stops accepting.
    #[tokio::test]
    async fn wrong_route_answers_404_and_cancellation_closes_the_server() {
        let server = MockServer::start().await;
        mount_discovery(
            &server,
            r#"{"authorizationEndpoint":"https://radius-ui.example/authorize"}"#,
            200,
        )
        .await;
        let port = free_port();
        let oauth = flow_with_port(&server.uri(), port);
        let (fake, interaction) = login_method_interaction(LOGIN_METHOD_BROWSER);
        let signal = interaction.signal.clone();

        let login = tokio::spawn(async move { oauth.login(interaction).await });
        let (_, state) = wait_for_auth_url(Arc::clone(&fake.auth_url)).await;
        // Wrong path, wrong path with a valid state, and a missing code:
        // none of them settles the login.
        let not_found = http_get(port, "/elsewhere").await;
        assert!(not_found.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(not_found.contains("Callback route not found."));
        let missing_code = http_get(port, &format!("/oauth/callback?state={state}")).await;
        assert!(missing_code.starts_with("HTTP/1.1 400 Bad Request\r\n"));
        assert!(missing_code.contains("Missing authorization code."));

        signal.cancel();
        let result = tokio::time::timeout(Duration::from_secs(10), login)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.unwrap_err(), AuthError::Cancelled);
        // The finally-close freed the port.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            tokio::net::TcpStream::connect((CALLBACK_HOST, port))
                .await
                .is_err(),
            "callback server must be closed after cancellation"
        );
    }

    /// A bind failure now propagates (this delta's refactor: upstream
    /// awaits `startOAuthCallbackServer`, whose listen promise rejects —
    /// the old degenerate-server behavior is gone).
    #[tokio::test]
    async fn occupied_callback_port_fails_the_login() {
        let server = MockServer::start().await;
        mount_discovery(
            &server,
            r#"{"authorizationEndpoint":"https://radius-ui.example/authorize"}"#,
            200,
        )
        .await;
        let occupied = std::net::TcpListener::bind((CALLBACK_HOST, 0)).unwrap();
        let port = occupied.local_addr().unwrap().port();
        let oauth = flow_with_port(&server.uri(), port);
        let (fake, interaction) = login_method_interaction(LOGIN_METHOD_BROWSER);

        let result = tokio::time::timeout(Duration::from_secs(10), oauth.login(interaction))
            .await
            .unwrap();
        // The bind failure surfaces as AddressInUse since v1.0.0 (the
        // shared server classifies the taken port; upstream carries Node's
        // EADDRINUSE error object through the same channel).
        assert!(matches!(
            result.unwrap_err(),
            AuthError::Operation(_) | AuthError::AddressInUse(_)
        ));
        // The failure happens before the authorize URL is emitted.
        assert!(fake.auth_url.lock().unwrap().is_none());
    }

    // ---- Device-code poll branches ----

    /// authorization_pending keeps polling; the next poll completes.
    #[tokio::test]
    async fn pending_poll_keeps_polling_until_complete() {
        let server = MockServer::start().await;
        mount_device(
            &server,
            r#"{"device_code":"device-code","user_code":"ABCD-1234","verification_uri":"https://radius-ui.example/pair","expires_in":600,"interval":1}"#,
        )
        .await;
        mount_token_queue(
            &server,
            vec![
                (400, r#"{"error":"authorization_pending"}"#.to_string()),
                (200, TOKEN_SUCCESS.to_string()),
            ],
        )
        .await;
        let oauth = create_radius_oauth(RadiusOAuthOptions {
            name: "Radius".to_string(),
            gateway: server.uri(),
        });
        let (_fake, interaction) = login_method_interaction(LOGIN_METHOD_DEVICE_CODE);

        let credential = tokio::time::timeout(Duration::from_secs(10), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(credential.access, "access-token");
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }

    /// The RFC 8628 terminal error branches and the rethrow of other OAuth
    /// errors.
    #[tokio::test]
    async fn device_poll_error_branches_match_upstream_messages() {
        for (body, expected) in [
            (
                r#"{"error":"expired_token"}"#.to_string(),
                "Device authorization expired.".to_string(),
            ),
            (
                r#"{"error":"access_denied"}"#.to_string(),
                "Device authorization was denied.".to_string(),
            ),
            (
                r#"{"error":"server_error","error_description":"boom"}"#.to_string(),
                "Radius OAuth token request failed: server_error: boom".to_string(),
            ),
        ] {
            let server = MockServer::start().await;
            mount_device(
                &server,
                r#"{"device_code":"device-code","user_code":"ABCD-1234","verification_uri":"https://radius-ui.example/pair","expires_in":600,"interval":1}"#,
            )
            .await;
            mount_token_queue(&server, vec![(400, body.clone())]).await;
            let oauth = create_radius_oauth(RadiusOAuthOptions {
                name: "Radius".to_string(),
                gateway: server.uri(),
            });
            let (_fake, interaction) = login_method_interaction(LOGIN_METHOD_DEVICE_CODE);

            let error = tokio::time::timeout(Duration::from_secs(10), oauth.login(interaction))
                .await
                .unwrap()
                .unwrap_err();
            assert_eq!(error, AuthError::Operation(expected), "{body}");
        }
    }

    /// A device authorization response missing fields fails with the
    /// upstream message.
    #[tokio::test]
    async fn device_authorization_missing_fields_fail() {
        for body in [
            r#"{"user_code":"ABCD-1234","verification_uri":"https://r/pair","expires_in":600}"#,
            r#"{"device_code":"device-code","user_code":"ABCD-1234","verification_uri":"https://r/pair"}"#,
            r#"{"device_code":"device-code","user_code":"ABCD-1234","verification_uri":"https://r/pair","expires_in":0}"#,
        ] {
            let server = MockServer::start().await;
            mount_device(&server, body).await;
            let oauth = create_radius_oauth(RadiusOAuthOptions {
                name: "Radius".to_string(),
                gateway: server.uri(),
            });
            let (_fake, interaction) = login_method_interaction(LOGIN_METHOD_DEVICE_CODE);

            let error = oauth.login(interaction).await.unwrap_err();
            assert_eq!(
                error,
                AuthError::Operation(
                    "Radius OAuth device authorization response is missing required fields"
                        .to_string()
                ),
                "{body}"
            );
        }
    }

    /// A failing discovery request carries the gateway, status and body.
    #[tokio::test]
    async fn failing_discovery_reports_status_and_body() {
        let server = MockServer::start().await;
        mount_discovery(&server, "gateway exploded", 500).await;
        let oauth = create_radius_oauth(RadiusOAuthOptions {
            name: "Radius".to_string(),
            gateway: server.uri(),
        });
        let (_fake, interaction) = login_method_interaction(LOGIN_METHOD_BROWSER);

        let error = oauth.login(interaction).await.unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation(format!(
                "Could not load Radius OAuth config from {}: 500 gateway exploded",
                server.uri()
            ))
        );
    }

    /// A successful token response missing fields errors (the port cannot
    /// store `undefined`; module port notes).
    #[tokio::test]
    async fn token_response_missing_fields_error() {
        let server = MockServer::start().await;
        mount_device(
            &server,
            r#"{"device_code":"device-code","user_code":"ABCD-1234","verification_uri":"https://radius-ui.example/pair","expires_in":600}"#,
        )
        .await;
        mount_token(&server, r#"{"access_token":"a"}"#, 200).await;
        let oauth = create_radius_oauth(RadiusOAuthOptions {
            name: "Radius".to_string(),
            gateway: server.uri(),
        });
        let (_fake, interaction) = login_method_interaction(LOGIN_METHOD_DEVICE_CODE);

        let error = tokio::time::timeout(Duration::from_secs(10), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation(
                "Radius OAuth token response missing field: refresh_token".to_string()
            )
        );
    }

    // ---- Prompt surface ----

    /// The select prompt offers browser first, then device code, and an
    /// unknown answer fails with the upstream message.
    #[tokio::test]
    async fn select_prompt_offers_browser_then_device_code() {
        let oauth = flow();
        let (fake, interaction) = login_method_interaction("magic");
        let error = oauth.login(interaction).await.unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("Unknown Radius sign-in method: magic".to_string())
        );
        let prompts = fake.prompts.lock().unwrap().clone();
        assert_eq!(prompts.len(), 1);
        match &prompts[0].kind {
            AuthPromptKind::Select { message, options } => {
                assert_eq!(message, "Sign in to Radius:");
                assert_eq!(
                    options
                        .iter()
                        .map(|option| (option.id.as_str(), option.label.as_str()))
                        .collect::<Vec<_>>(),
                    vec![
                        ("browser", "Sign in with browser (recommended)"),
                        (
                            "device-code",
                            "Sign in with device code (when signing in from another device)"
                        ),
                    ]
                );
            }
            other => panic!("expected a select prompt: {other:?}"),
        }
    }

    // ---- Metadata, cancellation, gateway normalization ----

    #[tokio::test]
    async fn metadata_and_to_auth_match_upstream() {
        let oauth = flow();
        assert_eq!(oauth.name(), "Radius");
        assert!(!oauth.is_subscription());
        assert_eq!(oauth.login_label(), None);
        let auth = oauth
            .to_auth(OAuthCredential {
                refresh: "r".to_string(),
                access: "access-token".to_string(),
                expires: 0,
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

    /// Entry cancellation: a pre-cancelled interaction signal never prompts
    /// and never reaches the network.
    #[tokio::test]
    async fn entry_cancelled_signal_short_circuits_login() {
        let server = MockServer::start().await;
        let oauth = create_radius_oauth(RadiusOAuthOptions {
            name: "Radius".to_string(),
            gateway: server.uri(),
        });
        let (_fake, interaction) = login_method_interaction(LOGIN_METHOD_BROWSER);
        interaction.signal.cancel();

        let result = oauth.login(interaction).await;
        assert_eq!(result, Err(AuthError::Cancelled));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    /// Upstream `normalizeRadiusGatewayUrl`: default https scheme, trailing
    /// slashes stripped, existing scheme preserved.
    #[test]
    fn normalize_radius_gateway_url_follows_upstream() {
        assert_eq!(
            normalize_radius_gateway_url("radius.example"),
            "https://radius.example"
        );
        assert_eq!(
            normalize_radius_gateway_url("radius.example/"),
            "https://radius.example"
        );
        assert_eq!(
            normalize_radius_gateway_url("radius.example///"),
            "https://radius.example"
        );
        assert_eq!(
            normalize_radius_gateway_url("http://radius.example/base/"),
            "http://radius.example/base"
        );
        assert_eq!(
            normalize_radius_gateway_url("HTTPS://RADIUS.example"),
            "HTTPS://RADIUS.example"
        );
    }

    #[test]
    fn production_constants_match_upstream() {
        assert_eq!(CALLBACK_HOST, "127.0.0.1");
        assert_eq!(CALLBACK_PORT, 1456);
        assert_eq!(CALLBACK_PATH, "/oauth/callback");
        assert_eq!(TOKEN_EXPIRY_SKEW_MS, 60_000);
        assert_eq!(OAUTH_CLIENT_ID, "pi-gateway");
        assert_eq!(OAUTH_SCOPE, "gateway offline_access");
        assert_eq!(
            OAUTH_DEVICE_CODE_GRANT_TYPE,
            "urn:ietf:params:oauth:grant-type:device_code"
        );
    }
}
