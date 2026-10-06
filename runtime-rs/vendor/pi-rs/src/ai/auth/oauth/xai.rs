//! xAI OAuth device-code flow ported from upstream
//! `packages/ai/src/auth/oauth/xai.ts`: the RFC 8628 device authorization
//! request, the token polling driven by the shared
//! [`super::device_code`] engine, and the refresh grant, exposed as the
//! [`XaiOAuth`] [`OAuthAuth`] implementation.
//!
//! Interactive surface (M2d ruling): the device code is reported via
//! [`AuthEvent::DeviceCode`] and the flow never prompts, touches stdio or a
//! browser directly.
//!
//! Port notes (disclosed divergences):
//! - The endpoint URLs are fields on [`XaiOAuth`] (upstream: module
//!   constants) so tests can point the flow at a wiremock server. The
//!   production constructor pins the upstream values.
//! - Poll schedules run on the shared device-code engine's tokio clock;
//!   `interval`/`expires_in` come from the server response, so the wire
//!   tests poll on short real-time intervals and the schedule shape is
//!   pinned by the engine's own tests.
//! - Cancellation maps to [`AuthError::Cancelled`] everywhere upstream
//!   throws `Error("Login cancelled")` (port contract: interaction-signal
//!   aborts are never wrapped). Transport failures carry the raw transport
//!   error text (upstream's `fetch` rejection propagates un-caught).
//! - A JSON response body that parses but is not an object reads as an
//!   empty object (upstream `body = {}`), so field validation reports the
//!   first missing field; a body that fails to parse errors as
//!   `xAI OAuth returned invalid JSON (HTTP {status})`.
//! - JSON re-serialization never appears in xAI failure messages (the
//!   upstream error paths interpolate only error/description strings), so
//!   the serde-ordering divergence of the other flows does not apply here.
//! - `AuthEvent::DeviceCode.interval_seconds` is `u64`: a fractional server
//!   interval reports truncated (the flow only forwards intervals > 0;
//!   intervals are integral in practice).

use std::collections::BTreeMap;

use futures::future::BoxFuture;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::ai::api::http_client;
use crate::ai::auth::types::{
    AuthError, AuthEvent, AuthInteraction, ModelAuth, OAuthAuth, OAuthCredential,
    ProviderAuthInteraction,
};
use crate::ai::now_ms;

use super::device_code::{poll_device_code_flow, PollOutcome};

/// Upstream `XAI_CLIENT_ID` (xai.ts:8).
const XAI_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";

/// Upstream `XAI_SCOPE` (xai.ts:9).
const XAI_SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";

/// Upstream `XAI_DEVICE_CODE_URL` (xai.ts:10); the production default for
/// [`XaiOAuth::new`].
const XAI_DEVICE_CODE_URL: &str = "https://auth.x.ai/oauth2/device/code";

/// Upstream `XAI_TOKEN_URL` (xai.ts:11).
const XAI_TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";

/// Upstream `REFRESH_SKEW_MS` (xai.ts:13): refresh slightly before the
/// reported expiry to avoid using a token that dies mid-request.
const REFRESH_SKEW_MS: i64 = 5 * 60 * 1000;

/// Upstream `DEFAULT_TOKEN_LIFETIME_SECONDS` (xai.ts:14).
const DEFAULT_TOKEN_LIFETIME_SECONDS: f64 = 3600.0;

/// The xAI OAuth auth surface (upstream `xaiOAuth`, xai.ts:229-239).
pub struct XaiOAuth {
    device_code_url: String,
    token_url: String,
}

impl Default for XaiOAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl XaiOAuth {
    /// Upstream module constants.
    pub fn new() -> Self {
        XaiOAuth {
            device_code_url: XAI_DEVICE_CODE_URL.to_string(),
            token_url: XAI_TOKEN_URL.to_string(),
        }
    }

    /// Test constructor: point the endpoints at a stub server (upstream
    /// tests stub the global `fetch`).
    #[cfg(test)]
    fn with_endpoints(device_code_url: String, token_url: String) -> Self {
        XaiOAuth {
            device_code_url,
            token_url,
        }
    }
}

/// One completed `postForm` response (upstream `OAuthHttpResponse`): the
/// status plus the body as a JSON object (a parsed non-object body reads as
/// an empty object, like the upstream `body = {}` fallback).
struct FormResponse {
    status: u16,
    ok: bool,
    body: serde_json::Map<String, Value>,
}

/// Upstream `postForm` (xai.ts:64-98): POST
/// `application/x-www-form-urlencoded` with `Accept: application/json`,
/// racing the interaction signal. A parse-failing body errors as invalid
/// JSON with the status; cancellation stays unwrapped.
async fn post_form(
    url: &str,
    body: String,
    signal: &CancellationToken,
) -> Result<FormResponse, AuthError> {
    let request = http_client()
        .post(url)
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body);

    let response = tokio::select! {
        biased;
        _ = signal.cancelled() => return Err(AuthError::Cancelled),
        response = request.send() => match response {
            Ok(response) => response,
            // Upstream catch: aborted → "Login cancelled", else rethrow.
            Err(error) => return Err(AuthError::Operation(error.to_string())),
        },
    };
    let status = response.status();
    let text = tokio::select! {
        biased;
        _ = signal.cancelled() => return Err(AuthError::Cancelled),
        text = response.text() => match text {
            Ok(text) => text,
            Err(error) => return Err(AuthError::Operation(error.to_string())),
        },
    };

    let body = match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => map,
        Ok(_) => serde_json::Map::new(),
        Err(_) => {
            return Err(AuthError::Operation(format!(
                "xAI OAuth returned invalid JSON (HTTP {})",
                status.as_u16()
            )));
        }
    };
    Ok(FormResponse {
        status: status.as_u16(),
        ok: status.is_success(),
        body,
    })
}

/// Upstream `requestFailure` (xai.ts:100-106): the `error` and
/// `error_description` strings joined with ": ", appended to the standard
/// failure prefix. Returns the message (the callers wrap or forward it).
fn request_failure(action: &str, response: &FormResponse) -> String {
    let error = response.body.get("error").and_then(Value::as_str);
    let description = response
        .body
        .get("error_description")
        .and_then(Value::as_str);
    let detail = [error, description]
        .into_iter()
        .flatten()
        .collect::<Vec<&str>>()
        .join(": ");
    format!(
        "xAI OAuth {action} failed (HTTP {}){}",
        response.status,
        if detail.is_empty() {
            String::new()
        } else {
            format!(": {detail}")
        }
    )
}

/// Upstream `requiredString` (xai.ts:33-39).
fn required_string(
    body: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<String, AuthError> {
    match body.get(field) {
        Some(Value::String(value)) if !value.is_empty() => Ok(value.clone()),
        _ => Err(AuthError::Operation(format!(
            "Invalid xAI OAuth response field: {field}"
        ))),
    }
}

/// Upstream `positiveNumber` (xai.ts:41-47).
fn positive_number(body: &serde_json::Map<String, Value>, field: &str) -> Result<f64, AuthError> {
    let value = body
        .get(field)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value > 0.0);
    match value {
        Some(value) => Ok(value),
        None => Err(AuthError::Operation(format!(
            "Invalid xAI OAuth response field: {field}"
        ))),
    }
}

/// The verification URI is opened in the user's browser; force it to be an
/// https URL so a malicious response cannot make `open` launch something
/// else (upstream `validateVerificationUri`, xai.ts:51-62). The normalized
/// href is what reaches the user.
fn validate_verification_uri(raw: &str) -> Result<String, AuthError> {
    let untrusted =
        || AuthError::Operation("Untrusted verification URI in xAI OAuth response".to_string());
    let url = Url::parse(raw).map_err(|_| untrusted())?;
    if url.scheme() != "https" {
        return Err(untrusted());
    }
    Ok(url.to_string())
}

/// Upstream `XaiDeviceCode` (xai.ts:24-31).
struct DeviceCode {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: Option<String>,
    interval_seconds: Option<f64>,
    expires_in_seconds: f64,
}

/// Upstream `parseDeviceCode` (xai.ts:108-126). RFC 8628 allows interval 0
/// (no minimum wait); a non-positive or malformed interval falls back to the
/// poller's default instead of failing.
fn parse_device_code(body: &serde_json::Map<String, Value>) -> Result<DeviceCode, AuthError> {
    let interval_seconds = match body.get("interval") {
        Some(Value::Number(_)) => body
            .get("interval")
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite() && *value > 0.0),
        _ => None,
    };
    let verification_uri_complete = match body.get("verification_uri_complete") {
        Some(Value::String(value)) if !value.is_empty() => Some(validate_verification_uri(value)?),
        _ => None,
    };
    Ok(DeviceCode {
        device_code: required_string(body, "device_code")?,
        user_code: required_string(body, "user_code")?,
        verification_uri: validate_verification_uri(&required_string(body, "verification_uri")?)?,
        verification_uri_complete,
        interval_seconds,
        expires_in_seconds: positive_number(body, "expires_in")?,
    })
}

/// Upstream `credentialsFromTokenResponse` (xai.ts:128-143): xAI may omit
/// `refresh_token` on refresh when the token is not rotated; `expires_in`
/// defaults to one hour; `expires` carries the 5-minute skew shave.
fn credentials_from_token_response(
    body: &serde_json::Map<String, Value>,
    previous_refresh_token: Option<&str>,
) -> Result<OAuthCredential, AuthError> {
    let access = required_string(body, "access_token")?;
    let refresh = match body.get("refresh_token") {
        // `body.refresh_token === undefined && previousRefreshToken`.
        None => match previous_refresh_token.filter(|token| !token.is_empty()) {
            Some(previous) => previous.to_string(),
            None => required_string(body, "refresh_token")?,
        },
        Some(_) => required_string(body, "refresh_token")?,
    };
    let expires_in_seconds = match body.get("expires_in") {
        None => DEFAULT_TOKEN_LIFETIME_SECONDS,
        Some(_) => positive_number(body, "expires_in")?,
    };
    Ok(OAuthCredential {
        refresh,
        access,
        expires: now_ms() + (expires_in_seconds * 1000.0) as i64 - REFRESH_SKEW_MS,
        extra: BTreeMap::new(),
    })
}

/// Upstream `requestDeviceCode` (xai.ts:145-159).
async fn request_device_code(
    oauth: &XaiOAuth,
    signal: &CancellationToken,
) -> Result<DeviceCode, AuthError> {
    // `new URLSearchParams({client_id, scope, referrer})` — insertion order.
    let body = {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("client_id", XAI_CLIENT_ID);
        query.append_pair("scope", XAI_SCOPE);
        query.append_pair("referrer", "pi");
        query.finish()
    };
    let response = post_form(&oauth.device_code_url, body, signal).await?;
    if !response.ok {
        return Err(AuthError::Operation(request_failure(
            "device authorization",
            &response,
        )));
    }
    parse_device_code(&response.body)
}

/// One poll of the token endpoint (the `poll` closure of upstream
/// `pollForTokens`, xai.ts:167-198).
async fn poll_once(
    token_url: &str,
    device_code: &str,
    signal: &CancellationToken,
) -> Result<PollOutcome<OAuthCredential>, AuthError> {
    let body = {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("grant_type", "urn:ietf:params:oauth:grant-type:device_code");
        query.append_pair("client_id", XAI_CLIENT_ID);
        query.append_pair("device_code", device_code);
        query.finish()
    };
    let response = post_form(token_url, body, signal).await?;

    if response.ok {
        return Ok(PollOutcome::Complete(credentials_from_token_response(
            &response.body,
            None,
        )?));
    }
    match response.body.get("error").and_then(Value::as_str) {
        Some("authorization_pending") => Ok(PollOutcome::Pending),
        // `typeof interval === "number" ? interval : undefined` — the engine
        // applies the finiteness/positivity filter.
        Some("slow_down") => Ok(PollOutcome::SlowDown(
            response.body.get("interval").and_then(Value::as_f64),
        )),
        Some("access_denied" | "authorization_denied") => Ok(PollOutcome::Failed(
            "xAI device authorization was denied".to_string(),
        )),
        Some("expired_token") => Ok(PollOutcome::Failed("xAI device code expired".to_string())),
        _ => Ok(PollOutcome::Failed(request_failure(
            "device token polling",
            &response,
        ))),
    }
}

/// Upstream `pollForTokens` (xai.ts:161-199): waits before the first poll
/// and follows the shared engine's schedule.
async fn poll_for_tokens(
    oauth: &XaiOAuth,
    device: &DeviceCode,
    signal: &CancellationToken,
) -> Result<OAuthCredential, AuthError> {
    let token_url = oauth.token_url.clone();
    let device_code = device.device_code.clone();
    poll_device_code_flow(
        device.interval_seconds,
        Some(device.expires_in_seconds),
        true,
        signal,
        || poll_once(&token_url, &device_code, signal),
    )
    .await
}

/// Upstream `loginXai` (xai.ts:201-211).
async fn login_xai(
    oauth: &XaiOAuth,
    interaction: ProviderAuthInteraction,
) -> Result<OAuthCredential, AuthError> {
    if interaction.signal.is_cancelled() {
        return Err(AuthError::Cancelled);
    }
    let device = request_device_code(oauth, &interaction.signal).await?;
    interaction.notify(AuthEvent::DeviceCode {
        user_code: device.user_code.clone(),
        verification_uri: device
            .verification_uri_complete
            .clone()
            .unwrap_or_else(|| device.verification_uri.clone()),
        // The event field is u64: fractional intervals truncate (disclosed);
        // parse_device_code only forwards positive intervals.
        interval_seconds: device.interval_seconds.map(|seconds| seconds as u64),
        expires_in_seconds: Some(device.expires_in_seconds as u64),
    });
    poll_for_tokens(oauth, &device, &interaction.signal).await
}

/// Upstream `refreshXaiToken` (xai.ts:213-227).
async fn refresh_xai_token(
    oauth: &XaiOAuth,
    refresh_token: &str,
    signal: &CancellationToken,
) -> Result<OAuthCredential, AuthError> {
    let body = {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("grant_type", "refresh_token");
        query.append_pair("client_id", XAI_CLIENT_ID);
        query.append_pair("refresh_token", refresh_token);
        query.finish()
    };
    let response = post_form(&oauth.token_url, body, signal).await?;
    if !response.ok {
        return Err(AuthError::Operation(request_failure(
            "token refresh",
            &response,
        )));
    }
    credentials_from_token_response(&response.body, Some(refresh_token))
}

impl OAuthAuth for XaiOAuth {
    /// Upstream `name` (xai.ts:230).
    fn name(&self) -> &str {
        "xAI (Grok/X subscription)"
    }

    /// Upstream `isSubscription: true` (xai.ts:231).
    fn is_subscription(&self) -> bool {
        true
    }

    /// Upstream `loginLabel` (xai.ts:232).
    fn login_label(&self) -> Option<&str> {
        Some("Sign in with SuperGrok or X Premium")
    }

    /// Upstream `login` (xai.ts:233).
    fn login<'a>(
        &'a self,
        interaction: ProviderAuthInteraction,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(login_xai(self, interaction))
    }

    /// Upstream `refresh` (xai.ts:234).
    fn refresh<'a>(
        &'a self,
        credential: OAuthCredential,
        options: &'a crate::ai::auth::types::AuthOperationOptions,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move {
            let signal = options.signal.clone().unwrap_or_default();
            refresh_xai_token(self, &credential.refresh, &signal).await
        })
    }

    /// Upstream `toAuth` (xai.ts:236-238): `{ apiKey: credential.access }`.
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

    /// The xAI flow never prompts (the oracle's prompt throws "Unexpected
    /// prompt").
    fn never_prompt_interaction() -> (Arc<FakeInteraction>, ProviderAuthInteraction) {
        fake_interaction(Box::new(|prompt| {
            Box::pin(async move { panic!("unexpected prompt: {:?}", prompt.kind) })
        }))
    }

    fn flow_with(server: &MockServer) -> XaiOAuth {
        XaiOAuth::with_endpoints(
            format!("{}/oauth2/device/code", server.uri()),
            format!("{}/oauth2/token", server.uri()),
        )
    }

    /// Serves the queued `(status, body)` responses in order — the wiremock
    /// analog of the oracle's `tokenReplies.shift()`. A poll past the queue
    /// gets a 500, failing the test loudly.
    async fn mount_token_queue(server: &MockServer, responses: Vec<(u16, String)>) {
        let queue = Arc::new(Mutex::new(std::collections::VecDeque::from(responses)));
        Mock::given(method("POST"))
            .and(path("/oauth2/token"))
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

    fn device_code_body(overrides: &str) -> String {
        format!(
            r#"{{"device_code":"device-code","user_code":"ABCD-1234","verification_uri":"https://accounts.x.ai/oauth2/device","expires_in":900,"interval":1{overrides}}}"#
        )
    }

    fn token_body(overrides: &str) -> String {
        format!(
            r#"{{"access_token":"access-token","refresh_token":"refresh-token","expires_in":21600,"token_type":"Bearer"{overrides}}}"#
        )
    }

    fn device_code_event(interaction: &FakeInteraction) -> AuthEvent {
        interaction
            .device_code
            .lock()
            .unwrap()
            .clone()
            .expect("login must emit a device_code event")
    }

    // ---- Oracle ports (packages/ai/test/xai-oauth.test.ts) ----

    /// Oracle: "uses the device grant, delays polling, and handles pending
    /// and slow_down" (short real-time intervals; the exact 5s/10s/20s
    /// schedule is pinned by the shared engine's tests).
    #[tokio::test]
    async fn uses_the_device_grant_delays_polling_and_handles_pending_and_slow_down() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth2/device/code"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(device_code_body(""), "application/json"),
            )
            .expect(1)
            .mount(&server)
            .await;
        mount_token_queue(
            &server,
            vec![
                (400, r#"{"error":"authorization_pending"}"#.to_string()),
                (400, r#"{"error":"slow_down","interval":2}"#.to_string()),
                (200, token_body("")),
            ],
        )
        .await;
        let oauth = flow_with(&server);
        let (fake, interaction) = never_prompt_interaction();

        let credential = tokio::time::timeout(Duration::from_secs(10), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap();

        assert_eq!(credential.access, "access-token");
        assert_eq!(credential.refresh, "refresh-token");
        // expires = token response time + expires_in - 5-minute skew.
        assert!((credential.expires - (now_ms() + 21_600_000 - 300_000)).abs() <= 2_000);

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 4);
        // Byte-pinned request bodies (upstream URLSearchParams insertion order).
        assert_eq!(
            String::from_utf8(requests[0].body.clone()).unwrap(),
            "client_id=b1a00492-073a-47ea-816f-4c329264a828&\
             scope=openid+profile+email+offline_access+grok-cli%3Aaccess+api%3Aaccess&referrer=pi"
        );
        for request in &requests[1..4] {
            assert_eq!(request.url.path(), "/oauth2/token");
            assert_eq!(
                String::from_utf8(request.body.clone()).unwrap(),
                "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code&\
                 client_id=b1a00492-073a-47ea-816f-4c329264a828&device_code=device-code"
            );
        }

        // The device_code event carries the upstream payload (the base
        // verification URI: no verification_uri_complete was sent).
        assert_eq!(
            device_code_event(&fake),
            AuthEvent::DeviceCode {
                user_code: "ABCD-1234".to_string(),
                verification_uri: "https://accounts.x.ai/oauth2/device".to_string(),
                interval_seconds: Some(1),
                expires_in_seconds: Some(900),
            }
        );
    }

    /// Oracle: "falls back to the default poll interval when the response
    /// reports interval 0" — the event reports no interval and the engine
    /// waits out the RFC 8628 5-second default before the (successful) first
    /// poll. The default itself is pinned by the shared engine's tests.
    #[tokio::test]
    async fn interval_zero_falls_back_to_the_default_poll_interval() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth2/device/code"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(
                    r#"{"device_code":"device-code","user_code":"ABCD-1234","verification_uri":"https://accounts.x.ai/oauth2/device","expires_in":900,"interval":0}"#,
                    "application/json",
                ),
            )
            .mount(&server)
            .await;
        mount_token_queue(&server, vec![(200, token_body(""))]).await;
        let oauth = flow_with(&server);
        let (fake, interaction) = never_prompt_interaction();

        let credential = tokio::time::timeout(Duration::from_secs(15), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(credential.access, "access-token");
        assert_eq!(
            device_code_event(&fake),
            AuthEvent::DeviceCode {
                user_code: "ABCD-1234".to_string(),
                verification_uri: "https://accounts.x.ai/oauth2/device".to_string(),
                interval_seconds: None,
                expires_in_seconds: Some(900),
            }
        );
        // Exactly one poll: the engine's 5-second default preceded it.
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2);
    }

    /// Oracle: "prefers verification_uri_complete when the server provides
    /// it".
    #[tokio::test]
    async fn prefers_verification_uri_complete_when_the_server_provides_it() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth2/device/code"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(
                    device_code_body(
                        r#","verification_uri_complete":"https://accounts.x.ai/oauth2/device?user_code=ABCD-1234""#,
                    ),
                    "application/json",
                ),
            )
            .mount(&server)
            .await;
        mount_token_queue(&server, vec![(200, token_body(""))]).await;
        let oauth = flow_with(&server);
        let (fake, interaction) = never_prompt_interaction();

        tokio::time::timeout(Duration::from_secs(10), oauth.login(interaction))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            device_code_event(&fake),
            AuthEvent::DeviceCode {
                user_code: "ABCD-1234".to_string(),
                verification_uri: "https://accounts.x.ai/oauth2/device?user_code=ABCD-1234"
                    .to_string(),
                interval_seconds: Some(1),
                expires_in_seconds: Some(900),
            }
        );
    }

    /// Oracle: "rejects a non-https verification_uri_complete".
    #[tokio::test]
    async fn rejects_a_non_https_verification_uri_complete() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth2/device/code"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(
                    device_code_body(
                        r#","verification_uri_complete":"http://accounts.x.ai/oauth2/device?user_code=ABCD-1234""#,
                    ),
                    "application/json",
                ),
            )
            .mount(&server)
            .await;
        let oauth = flow_with(&server);
        let (fake, interaction) = never_prompt_interaction();

        let error = oauth.login(interaction).await.unwrap_err();
        assert!(
            error.to_string().contains("Untrusted verification URI"),
            "{error}"
        );
        assert!(fake.events.lock().unwrap().is_empty());
    }

    /// Oracle: "rejects a non-https verification URI" (http, file, not a
    /// URL).
    #[tokio::test]
    async fn rejects_non_https_verification_uris() {
        for verification_uri in [
            "http://accounts.x.ai/oauth2/device",
            "file:///etc/passwd",
            "not a url",
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/oauth2/device/code"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_raw(
                        format!(
                            r#"{{"device_code":"device-code","user_code":"ABCD-1234","verification_uri":{},"expires_in":900,"interval":1}}"#,
                            serde_json::to_string(verification_uri).unwrap()
                        ),
                        "application/json",
                    ),
                )
                .mount(&server)
                .await;
            let oauth = flow_with(&server);
            let (fake, interaction) = never_prompt_interaction();

            let error = oauth.login(interaction).await.unwrap_err();
            assert!(
                error.to_string().contains("Untrusted verification URI"),
                "{verification_uri}: {error}"
            );
            assert!(fake.events.lock().unwrap().is_empty());
        }
    }

    /// Oracle: "fails when device authorization is denied"
    /// (access_denied and authorization_denied).
    #[tokio::test]
    async fn fails_when_device_authorization_is_denied() {
        for error_code in ["access_denied", "authorization_denied"] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/oauth2/device/code"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_raw(device_code_body(""), "application/json"),
                )
                .mount(&server)
                .await;
            mount_token_queue(
                &server,
                vec![(400, format!(r#"{{"error":"{error_code}"}}"#))],
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
                AuthError::Operation("xAI device authorization was denied".to_string())
            );
        }
    }

    /// Port coverage for the remaining oracle poll branch: expired_token.
    #[tokio::test]
    async fn fails_when_the_device_code_has_expired() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth2/device/code"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(device_code_body(""), "application/json"),
            )
            .mount(&server)
            .await;
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
            AuthError::Operation("xAI device code expired".to_string())
        );
    }

    /// Oracle: "cancels while waiting for the first token poll" — only the
    /// device-code request is made.
    #[tokio::test]
    async fn cancels_while_waiting_for_the_first_token_poll() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth2/device/code"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(device_code_body(""), "application/json"),
            )
            .mount(&server)
            .await;
        let oauth = flow_with(&server);
        let (fake, interaction) = never_prompt_interaction();
        let signal = interaction.signal.clone();
        let slot = Arc::clone(&fake.device_code);
        let driver = tokio::spawn(async move {
            loop {
                if slot.lock().unwrap().is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            signal.cancel();
        });

        let result = tokio::time::timeout(Duration::from_secs(10), oauth.login(interaction))
            .await
            .unwrap();
        assert_eq!(result.unwrap_err(), AuthError::Cancelled);
        driver.await.unwrap();
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].url.path(), "/oauth2/device/code");
    }

    /// Oracle: "refreshes tokens and preserves an unrotated refresh token".
    #[tokio::test]
    async fn refreshes_tokens_and_preserves_an_unrotated_refresh_token() {
        let server = MockServer::start().await;
        let queue = Arc::new(Mutex::new(std::collections::VecDeque::from(vec![
            (
                r#"{"access_token":"new-access","refresh_token":"new-refresh","expires_in":21600,"token_type":"Bearer"}"#.to_string(),
                "old-refresh".to_string(),
            ),
            (
                // xAI omitted refresh_token: the previous one is preserved.
                r#"{"access_token":"newer-access","expires_in":21600,"token_type":"Bearer"}"#
                    .to_string(),
                "keep-refresh".to_string(),
            ),
        ])));
        let serve_queue = Arc::clone(&queue);
        Mock::given(method("POST"))
            .and(path("/oauth2/token"))
            .respond_with(move |request: &wiremock::Request| {
                let mut queue = serve_queue.lock().unwrap();
                let (body, expected_refresh) = queue.pop_front().expect("unexpected refresh call");
                // Byte-pinned request body (upstream URLSearchParams insertion
                // order).
                let form = String::from_utf8(request.body.clone()).unwrap();
                assert_eq!(
                    form,
                    format!(
                        "grant_type=refresh_token&\
                         client_id=b1a00492-073a-47ea-816f-4c329264a828&\
                         refresh_token={expected_refresh}"
                    )
                );
                ResponseTemplate::new(200).set_body_raw(body, "application/json")
            })
            .mount(&server)
            .await;
        let oauth = flow_with(&server);

        let credential = |refresh: &str| OAuthCredential {
            refresh: refresh.to_string(),
            access: "old-access".to_string(),
            expires: 0,
            extra: Default::default(),
        };
        let rotated = oauth
            .refresh(credential("old-refresh"), &AuthOperationOptions::default())
            .await
            .unwrap();
        let preserved = oauth
            .refresh(credential("keep-refresh"), &AuthOperationOptions::default())
            .await
            .unwrap();
        assert_eq!(rotated.access, "new-access");
        assert_eq!(rotated.refresh, "new-refresh");
        assert_eq!(preserved.access, "newer-access");
        // xAI omitted refresh_token: the previous one is preserved.
        assert_eq!(preserved.refresh, "keep-refresh");
    }

    /// Oracle: "assumes a one-hour lifetime when expires_in is missing".
    #[tokio::test]
    async fn assumes_a_one_hour_lifetime_when_expires_in_is_missing() {
        let server = MockServer::start().await;
        mount_token_queue(
            &server,
            vec![(
                200,
                r#"{"access_token":"access-token","refresh_token":"refresh-token"}"#.to_string(),
            )],
        )
        .await;
        let oauth = flow_with(&server);
        let credential = OAuthCredential {
            refresh: "old-refresh".to_string(),
            access: "old-access".to_string(),
            expires: 0,
            extra: Default::default(),
        };

        let refreshed = oauth
            .refresh(credential, &AuthOperationOptions::default())
            .await
            .unwrap();
        assert!((refreshed.expires - (now_ms() + 3_600_000 - 300_000)).abs() <= 2_000);
    }

    /// Oracle: "rejects token responses with missing fields".
    #[tokio::test]
    async fn rejects_token_responses_with_missing_fields() {
        let server = MockServer::start().await;
        mount_token_queue(
            &server,
            vec![(
                200,
                r#"{"refresh_token":"refresh-token","expires_in":21600}"#.to_string(),
            )],
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
            .refresh(credential, &AuthOperationOptions::default())
            .await
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("Invalid xAI OAuth response field: access_token".to_string())
        );
    }

    /// Oracle: "surfaces the upstream error code and description on refresh
    /// failure".
    #[tokio::test]
    async fn surfaces_the_upstream_error_code_and_description_on_refresh_failure() {
        let server = MockServer::start().await;
        mount_token_queue(
            &server,
            vec![(
                400,
                r#"{"error":"invalid_grant","error_description":"refresh token revoked"}"#
                    .to_string(),
            )],
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
            .refresh(credential, &AuthOperationOptions::default())
            .await
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation(
                "xAI OAuth token refresh failed (HTTP 400): invalid_grant: refresh token revoked"
                    .to_string()
            )
        );
    }

    /// With no `error`/`error_description` in the body the message carries no
    /// detail and therefore no trailing ": " separator (upstream
    /// `requestFailure`, xai.ts:100-106).
    #[tokio::test]
    async fn refresh_failure_without_detail_has_no_trailing_separator() {
        let server = MockServer::start().await;
        mount_token_queue(&server, vec![(400, "{}".to_string())]).await;
        let oauth = flow_with(&server);
        let credential = OAuthCredential {
            refresh: "old-refresh".to_string(),
            access: "old-access".to_string(),
            expires: 0,
            extra: Default::default(),
        };

        let error = oauth
            .refresh(credential, &AuthOperationOptions::default())
            .await
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("xAI OAuth token refresh failed (HTTP 400)".to_string())
        );
    }

    /// Port coverage for the device-code parse units (upstream
    /// requiredString/positiveNumber over the wire).
    #[tokio::test]
    async fn device_code_response_field_errors_are_named() {
        let cases = [
            (
                r#"{"user_code":"ABCD-1234","verification_uri":"https://accounts.x.ai/oauth2/device","expires_in":900}"#,
                "Invalid xAI OAuth response field: device_code",
            ),
            (
                r#"{"device_code":"d","verification_uri":"https://accounts.x.ai/oauth2/device","expires_in":900}"#,
                "Invalid xAI OAuth response field: user_code",
            ),
            (
                r#"{"device_code":"d","user_code":"u","verification_uri":"https://accounts.x.ai/oauth2/device","expires_in":0}"#,
                "Invalid xAI OAuth response field: expires_in",
            ),
        ];
        for (body, expected) in cases {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/oauth2/device/code"))
                .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
                .mount(&server)
                .await;
            let oauth = flow_with(&server);
            let (_fake, interaction) = never_prompt_interaction();

            let error = oauth.login(interaction).await.unwrap_err();
            assert_eq!(error, AuthError::Operation(expected.to_string()));
        }
    }

    /// Port coverage for the upstream invalid-JSON path over the wire.
    #[tokio::test]
    async fn invalid_json_responses_report_the_status() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth2/device/code"))
            .respond_with(ResponseTemplate::new(200).set_body_raw("not json", "text/plain"))
            .mount(&server)
            .await;
        let oauth = flow_with(&server);
        let (_fake, interaction) = never_prompt_interaction();

        let error = oauth.login(interaction).await.unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("xAI OAuth returned invalid JSON (HTTP 200)".to_string())
        );
    }

    // ---- Metadata ----

    #[tokio::test]
    async fn metadata_and_to_auth_match_upstream() {
        let oauth = XaiOAuth::new();
        assert_eq!(oauth.name(), "xAI (Grok/X subscription)");
        assert_eq!(
            oauth.login_label(),
            Some("Sign in with SuperGrok or X Premium")
        );
        assert!(oauth.is_subscription());
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

    #[test]
    fn production_endpoints_match_upstream() {
        assert_eq!(XAI_CLIENT_ID, "b1a00492-073a-47ea-816f-4c329264a828");
        assert_eq!(
            XAI_SCOPE,
            "openid profile email offline_access grok-cli:access api:access"
        );
        assert_eq!(XAI_DEVICE_CODE_URL, "https://auth.x.ai/oauth2/device/code");
        assert_eq!(XAI_TOKEN_URL, "https://auth.x.ai/oauth2/token");
        assert_eq!(REFRESH_SKEW_MS, 5 * 60 * 1000);
        assert_eq!(DEFAULT_TOKEN_LIFETIME_SECONDS, 3600.0);
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
