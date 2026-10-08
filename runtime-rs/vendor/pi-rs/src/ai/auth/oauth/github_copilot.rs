//! GitHub Copilot OAuth flow ported from upstream
//! `packages/ai/src/auth/oauth/github-copilot.ts`: the device-code login
//! (`https://{domain}/login/device/code` + `/login/oauth/access_token`), the
//! Copilot access-token exchange (`/copilot_internal/v2/token`), the model
//! catalog fetch with rate-limit retry (`/models`), the policy-enable batch
//! (`/models/{id}/policy`), and the [`GitHubCopilotOAuth`] [`OAuthAuth`]
//! implementation.
//!
//! Interactive surface (M2d ruling): the enterprise-domain question is a
//! `text` prompt, the device code is reported via [`AuthEvent::DeviceCode`],
//! the enable step via [`AuthEvent::Progress`] — the flow never touches
//! stdio or a browser directly. [`OAuthAuth::to_auth`] derives the
//! per-credential proxy base URL from the token's `proxy-ep` claim (the
//! reason `to_auth` exists on the trait).
//!
//! Port notes (disclosed divergences):
//! - The request URLs are fields (upstream: derived per call from the
//!   enterprise domain) so tests can point the flow at a wiremock server.
//!   [`GitHubCopilotOAuth::new`] pins the upstream derivation
//!   (`https://{domain}/login/device/code`, `https://{domain}/login/oauth/
//!   access_token`, `https://api.{domain}/copilot_internal/v2/token`); the
//!   overrides exist only for tests and every derivation stays faithful when
//!   they are unset. The `/models` + `/policy` base and the policy-fallback
//!   gate (`baseUrl === "https://api.individual.githubcopilot.com"`) are
//!   always computed from the real token/domain logic.
//! - Upstream filters `policyModelIds` by membership in the static
//!   `GITHUB_COPILOT_MODELS` catalog (`Object.hasOwn`, provider data from
//!   `providers/data/github-copilot.json`). The catalog is provider scope
//!   (auto-generated; lands with the provider wiring), so the membership
//!   check is an injected predicate, `known_model_ids`, defaulting to
//!   "nothing is known": with the default, `policyModelIds` is always empty
//!   and login never enables models — exactly upstream's behavior for models
//!   outside its bundled catalog. The picker/policy-fallback logic of
//!   `availableModelIds` carries no catalog dependency and is fully ported.
//! - Upstream `pollOAuthDeviceCodeFlow` (device-code.ts) is the shared
//!   [`super::device_code::poll_device_code_flow`] engine (the private copy
//!   this module initially carried was absorbed into it); the Copilot flow
//!   waits before the first poll and adopts the server-provided `slow_down`
//!   interval. [`super::device_code::abortable_sleep`] also backs this
//!   module's rate-limit retry backoff.
//! - Poll deadlines and schedules are measured on tokio's clock
//!   ([`tokio::time::Instant`]): wall-clock in production, pause-able in
//!   tests. Upstream uses `Date.now()`.
//! - Cancellation maps to [`AuthError::Cancelled`] everywhere upstream
//!   throws `Error("Login cancelled")` or propagates an abort (port
//!   contract: interaction-signal aborts are never wrapped). Login also
//!   short-circuits when the signal is already cancelled after the prompt.
//! - Transport failures, the rate-limit-retry path's per-attempt timeout and
//!   its retry-budget deadline carry port-invented error texts where upstream
//!   surfaces raw `fetch` rejections / DOMExceptions. Timeout scoping is
//!   faithful: upstream's `AbortSignal.timeout(5000)` is created inside the
//!   `fetchWithRateLimitRetry` loop (github-copilot.ts:150), so only the
//!   `/models` and `/policy` requests carry a per-attempt cap — the
//!   `fetchJson` endpoints (device-code start, access-token poll, Copilot
//!   token exchange) are plain fetches that wait on the caller's signal
//!   alone, and the port keeps exactly that split. Raw response bodies in
//!   failure messages are preserved byte-for-byte; JSON parse failures
//!   surface the raw serde error text where upstream surfaces the
//!   `SyntaxError`.
//! - Failure-message JSON re-serialization order and stricter non-string
//!   field handling follow the T4 conventions (see `openai_codex.rs`).
//! - `AuthEvent::DeviceCode.interval_seconds` is `u64`: a fractional server
//!   interval reports truncated, a negative one clamps to 0 (intervals are
//!   positive integers in practice).
//! - `Retry-After` parsing follows upstream: `parseFloat` prefix semantics
//!   first ([`js_parse_float`]), then an IMF-fixdate HTTP-date (upstream
//!   `Date.parse` also accepts obsolete RFC 850 / asctime forms, which
//!   Retry-After senders do not use). A non-finite or unparseable value
//!   gives up the retry like upstream's `!Number.isFinite` branch.
//! - CLIENT_ID is the decoded literal of upstream's `atob`-obfuscated
//!   constant (`SXYxLmI1MDdhMDhjODdlY2ZlOTg=` → `Iv1.b507a08c87ecfe98`),
//!   pinned by test.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::ai::api::http_client;
use crate::ai::api::openai_codex_responses::parse_http_date;
use crate::ai::auth::types::{
    AuthError, AuthEvent, AuthInteraction, AuthPrompt, AuthPromptKind, ModelAuth, OAuthAuth,
    OAuthCredential, ProviderAuthInteraction,
};
use crate::ai::now_ms;

use super::device_code::{abortable_sleep, poll_device_code_flow, PollOutcome};

/// Upstream `CLIENT_ID` (github-copilot.ts:11, the `atob`-decoded literal).
const CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";

/// Upstream `COPILOT_HEADERS["User-Agent"]` (also the device endpoints') (github-copilot.ts:14).
const USER_AGENT: &str = "GitHubCopilotChat/0.35.0";

/// Upstream `COPILOT_API_VERSION` (github-copilot.ts:19).
const COPILOT_API_VERSION: &str = "2026-06-01";

/// Upstream `enterpriseDomain || "github.com"` (github-copilot.ts:318).
const DEFAULT_DOMAIN: &str = "github.com";

/// Upstream `getGitHubCopilotBaseUrl` fallback (github-copilot.ts:86); also
/// the allowPolicyFallback gate (github-copilot.ts:177).
const INDIVIDUAL_API_BASE: &str = "https://api.individual.githubcopilot.com";

/// Upstream `getBaseUrlFromToken` marker (github-copilot.ts:70).
const PROXY_EP_MARKER: &str = "proxy-ep=";

/// Upstream per-attempt `AbortSignal.timeout(5000)` created inside the
/// `fetchWithRateLimitRetry` loop (github-copilot.ts:150) — the rate-limit
/// retry path only. The `fetchJson` endpoints have no per-request timeout
/// upstream and none here.
const REQUEST_TIMEOUT_MS: u64 = 5000;

/// Port-invented text for the retry-budget deadline firing mid-request
/// (upstream: the budget `AbortSignal.timeout` aborts the fetch).
const RETRY_BUDGET_EXHAUSTED: &str =
    "rate-limit retry budget exhausted before the request completed";

/// The GitHub Copilot OAuth auth surface (upstream `githubCopilotOAuth`,
/// github-copilot.ts:493-507). [`GitHubCopilotOAuth::new`] pins the upstream
/// URL derivation and the default (empty) known-models predicate; tests
/// inject wiremock URLs via [`GitHubCopilotOAuth::with_endpoints`].
pub struct GitHubCopilotOAuth {
    /// Test-only override of the `https://{domain}` device-endpoint base.
    github_base_override: Option<String>,
    /// Test-only override of the `https://api.{domain}/copilot_internal/v2/token` URL.
    copilot_token_url_override: Option<String>,
    /// Test-only override of the `/models` + `/policy` fetch base (the
    /// policy-fallback gate always uses the real derivation).
    api_base_override: Option<String>,
    /// Upstream `Object.hasOwn(GITHUB_COPILOT_MODELS, model.id)` — which
    /// unconfigured account models the login may enable. Defaults to
    /// "nothing is known" until the provider catalog lands (see module docs).
    known_model_ids: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    /// The rate-limit-retry path's per-attempt timeout (upstream 5000ms).
    /// Never applied to the `fetchJson` endpoints. Test-overridable via
    /// [`GitHubCopilotOAuth::with_request_timeout`].
    request_timeout: Duration,
}

impl Default for GitHubCopilotOAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl GitHubCopilotOAuth {
    /// Upstream module constants and derivation. No overrides, no known
    /// models (module docs: the static catalog is provider scope).
    pub fn new() -> Self {
        GitHubCopilotOAuth {
            github_base_override: None,
            copilot_token_url_override: None,
            api_base_override: None,
            known_model_ids: Arc::new(|_| false),
            request_timeout: Duration::from_millis(REQUEST_TIMEOUT_MS),
        }
    }

    /// Test constructor: point the endpoints at a stub server and inject the
    /// known-models predicate (upstream tests stub the global `fetch` and
    /// rely on the real catalog for the same effect).
    #[cfg(test)]
    fn with_endpoints(
        github_base_override: Option<String>,
        copilot_token_url_override: Option<String>,
        api_base_override: Option<String>,
        known_model_ids: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    ) -> Self {
        GitHubCopilotOAuth {
            github_base_override,
            copilot_token_url_override,
            api_base_override,
            known_model_ids,
            request_timeout: Duration::from_millis(REQUEST_TIMEOUT_MS),
        }
    }

    /// Test-only: shorten the rate-limit-retry path's per-attempt timeout so
    /// delayed-response tests can discriminate capped endpoints from
    /// uncapped ones without waiting out the production 5s.
    #[cfg(test)]
    fn with_request_timeout(mut self, request_timeout: Duration) -> Self {
        self.request_timeout = request_timeout;
        self
    }

    /// Upstream `getUrls` device-host base: `https://{domain}`.
    fn site_base(&self, domain: &str) -> String {
        self.github_base_override
            .clone()
            .unwrap_or_else(|| format!("https://{domain}"))
    }

    /// Upstream `getUrls().deviceCodeUrl` (github-copilot.ts:58).
    fn device_code_url(&self, domain: &str) -> String {
        format!("{}/login/device/code", self.site_base(domain))
    }

    /// Upstream `getUrls().accessTokenUrl` (github-copilot.ts:59).
    fn access_token_url(&self, domain: &str) -> String {
        format!("{}/login/oauth/access_token", self.site_base(domain))
    }

    /// Upstream `getUrls().copilotTokenUrl` (github-copilot.ts:60).
    fn copilot_token_url(&self, domain: &str) -> String {
        self.copilot_token_url_override
            .clone()
            .unwrap_or_else(|| format!("https://api.{domain}/copilot_internal/v2/token"))
    }
}

/// Upstream `normalizeDomain` (github-copilot.ts:41-50): trimmed input
/// parsed as a URL (an `https://` scheme prepended when absent), yielding
/// the hostname. `None` for empty or unparseable input.
fn normalize_domain(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    Url::parse(&candidate)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
}

/// Upstream `getBaseUrlFromToken` (github-copilot.ts:69-76): the first
/// `proxy-ep=` cookie pair's value ([^;]+, so a non-empty run up to the next
/// `;`), mapped from `proxy.` to `api.`.
fn get_base_url_from_token(token: &str) -> Option<String> {
    let mut offset = 0;
    while let Some(position) = token[offset..].find(PROXY_EP_MARKER) {
        let start = offset + position + PROXY_EP_MARKER.len();
        let rest = &token[start..];
        let host = &rest[..rest.find(';').unwrap_or(rest.len())];
        if !host.is_empty() {
            // Upstream replaces only a leading `proxy.` with `api.`
            // (regex ^proxy\.).
            let api_host = match host.strip_prefix("proxy.") {
                Some(rest) => format!("api.{rest}"),
                None => host.to_string(),
            };
            return Some(format!("https://{api_host}"));
        }
        offset = start;
    }
    None
}

/// Upstream `getGitHubCopilotBaseUrl` (github-copilot.ts:78-87): the token's
/// proxy-ep first, then the enterprise domain, then the Individual default.
fn get_github_copilot_base_url(token: Option<&str>, enterprise_domain: Option<&str>) -> String {
    if let Some(url) = token.and_then(get_base_url_from_token) {
        return url;
    }
    if let Some(domain) = enterprise_domain {
        return format!("https://copilot-api.{domain}");
    }
    INDIVIDUAL_API_BASE.to_string()
}

/// JS `typeof value === "object"` (true for arrays too; the callers' `raw &&`
/// truthiness already excludes null, which serde maps to `Value::Null`).
fn js_is_object(value: &Value) -> bool {
    matches!(value, Value::Object(_) | Value::Array(_))
}

/// JS `Number.parseFloat`: optional sign, digits with an optional single dot
/// and exponent, as the longest valid prefix (leading whitespace skipped).
/// `Infinity` literals parse to infinity; anything without digits is `None`
/// (JS NaN).
fn js_parse_float(value: &str) -> Option<f64> {
    let trimmed = value.trim_start();
    let negative = trimmed.starts_with('-');
    let unsigned = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
    if unsigned.starts_with("Infinity") {
        return Some(if negative {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        });
    }
    let bytes = unsigned.as_bytes();
    let mut end = 0;
    let mut seen_digit = false;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
        seen_digit = true;
    }
    if end < bytes.len() && bytes[end] == b'.' {
        end += 1;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
            seen_digit = true;
        }
    }
    if !seen_digit {
        return None;
    }
    let mut full = end;
    if full < bytes.len() && (bytes[full] == b'e' || bytes[full] == b'E') {
        let mut exponent = full + 1;
        if exponent < bytes.len() && (bytes[exponent] == b'+' || bytes[exponent] == b'-') {
            exponent += 1;
        }
        let digits = exponent;
        while exponent < bytes.len() && bytes[exponent].is_ascii_digit() {
            exponent += 1;
        }
        if exponent > digits {
            full = exponent;
        }
    }
    unsigned[..full]
        .parse::<f64>()
        .ok()
        .map(|parsed| if negative { -parsed } else { parsed })
}

/// Upstream `fetchWithRateLimitRetry`'s `Retry-After` reading
/// (github-copilot.ts:154-159): delay-seconds (parseFloat) first, then an
/// HTTP-date minus now; `None` = non-finite, give up like upstream's
/// `!Number.isFinite(delayMs)` branch.
fn parse_retry_after_ms(value: &str, now_ms: i64) -> Option<f64> {
    if let Some(seconds) = js_parse_float(value) {
        return if seconds.is_finite() {
            Some(seconds * 1000.0)
        } else {
            None
        };
    }
    let epoch_ms = i64::try_from(parse_http_date(value)?).ok()?;
    let delay = (epoch_ms - now_ms) as f64;
    delay.is_finite().then_some(delay)
}

/// Upstream `COPILOT_HEADERS` (github-copilot.ts:13-18).
fn copilot_headers() -> Vec<(&'static str, String)> {
    vec![
        ("User-Agent", USER_AGENT.to_string()),
        ("Editor-Version", "vscode/1.107.0".to_string()),
        ("Editor-Plugin-Version", "copilot-chat/0.35.0".to_string()),
        ("Copilot-Integration-Id", "vscode-chat".to_string()),
    ]
}

/// The two HTTP methods the GitHub Copilot OAuth flow uses. Upstream builds
/// `init.method` strings; the enum makes [`send_once`]'s request-builder
/// match exhaustive instead of a `_ => post` catch-all that would silently
/// send POST on an unknown method.
enum Method {
    Get,
    Post,
}

/// One HTTP request description; rebuilt per retry attempt (upstream passes
/// `init` to every `fetch`).
struct RequestSpec {
    method: Method,
    url: String,
    headers: Vec<(&'static str, String)>,
    body: Option<String>,
}

/// A completed response. `retry_after` is read off the headers for the
/// rate-limit retry; `reason` is upstream `response.statusText`.
struct WireResponse {
    status: u16,
    ok: bool,
    reason: String,
    body: String,
    retry_after: Option<String>,
}

/// Failures of [`send_once`]. `Cancelled` never reaches the upstream-style
/// wrappers: the port surfaces cancellation as [`AuthError::Cancelled`].
enum SendError {
    Cancelled,
    /// Transport failure, per-request timeout, or retry-budget deadline
    /// (upstream: a `fetch` rejection).
    Transport(String),
}

/// Resolves when an optional deadline passes; never when it is `None` (the
/// select arm then stays out of the way, like an uncapped request).
async fn deadline_sleep(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// One HTTP request attempt. The retry-budget and per-request deadlines are
/// `None` on the `fetchJson` endpoints, which wait on the interaction signal
/// alone (upstream: plain `fetch` without `AbortSignal.timeout`); the
/// rate-limit-retry path passes both, where upstream races
/// `AbortSignal.any([signal, budgetSignal, AbortSignal.timeout(5000)])` and
/// recreates the per-attempt timeout inside its loop.
async fn send_once(
    spec: &RequestSpec,
    budget_deadline: Option<tokio::time::Instant>,
    request_timeout: Option<Duration>,
    signal: &CancellationToken,
) -> Result<WireResponse, SendError> {
    let request_deadline = request_timeout.map(|timeout| tokio::time::Instant::now() + timeout);
    let mut request = match spec.method {
        Method::Get => http_client().get(&spec.url),
        Method::Post => http_client().post(&spec.url),
    };
    for (name, value) in &spec.headers {
        request = request.header(*name, value);
    }
    if let Some(body) = &spec.body {
        request = request.body(body.clone());
    }

    let response = tokio::select! {
        biased;
        _ = signal.cancelled() => return Err(SendError::Cancelled),
        _ = deadline_sleep(budget_deadline) => {
            return Err(SendError::Transport(RETRY_BUDGET_EXHAUSTED.to_string()));
        }
        _ = deadline_sleep(request_deadline) => {
            let timeout_ms = request_timeout.map_or(0, |timeout| timeout.as_millis());
            return Err(SendError::Transport(format!(
                "request timed out after {timeout_ms}ms: {}",
                spec.url
            )));
        }
        response = request.send() => match response {
            Ok(response) => response,
            Err(error) => return Err(SendError::Transport(error.to_string())),
        },
    };
    let status = response.status();
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = tokio::select! {
        biased;
        _ = signal.cancelled() => return Err(SendError::Cancelled),
        _ = deadline_sleep(budget_deadline) => {
            return Err(SendError::Transport(RETRY_BUDGET_EXHAUSTED.to_string()));
        }
        _ = deadline_sleep(request_deadline) => {
            let timeout_ms = request_timeout.map_or(0, |timeout| timeout.as_millis());
            return Err(SendError::Transport(format!(
                "request timed out after {timeout_ms}ms: {}",
                spec.url
            )));
        }
        body = response.text() => match body {
            Ok(body) => body,
            Err(error) => return Err(SendError::Transport(error.to_string())),
        },
    };
    Ok(WireResponse {
        status: status.as_u16(),
        ok: status.is_success(),
        reason: status.canonical_reason().unwrap_or("").to_string(),
        body,
        retry_after,
    })
}

// device-code.ts `abortableSleep` lives in `super::device_code`; this module
// reuses it for the rate-limit retry backoff below.

/// Upstream `fetchWithRateLimitRetry` (github-copilot.ts:135-166): 429
/// responses retry up to `max_retries` times honoring `Retry-After`
/// (default backoff 500 * 2^retry), but a delay that would outlast the
/// `max_elapsed_ms` budget (when both are positive) returns the 429. Every
/// attempt carries the `request_timeout` cap (upstream recreates
/// `AbortSignal.timeout(5000)` inside the loop).
async fn fetch_with_rate_limit_retry(
    spec: &RequestSpec,
    max_retries: u32,
    max_elapsed_ms: u64,
    request_timeout: Duration,
    signal: &CancellationToken,
) -> Result<WireResponse, AuthError> {
    let budget_deadline = (max_retries > 0 && max_elapsed_ms > 0)
        .then(|| tokio::time::Instant::now() + Duration::from_millis(max_elapsed_ms));
    let mut retry: u32 = 0;
    loop {
        let response = send_once(spec, budget_deadline, Some(request_timeout), signal)
            .await
            .map_err(|error| match error {
                SendError::Cancelled => AuthError::Cancelled,
                SendError::Transport(text) => AuthError::Operation(text),
            })?;
        if response.status != 429 || retry == max_retries {
            return Ok(response);
        }

        // Default backoff, overridden by Retry-After; unparseable gives up.
        let mut delay_ms = 500.0 * 2f64.powi(i32::try_from(retry).unwrap_or(i32::MAX));
        if let Some(header) = response
            .retry_after
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            let Some(parsed) = parse_retry_after_ms(header, now_ms()) else {
                return Ok(response);
            };
            delay_ms = parsed;
        }
        let delay_ms = delay_ms.max(0.0);
        if let Some(budget) = budget_deadline {
            let remaining = budget
                .saturating_duration_since(tokio::time::Instant::now())
                .as_millis() as f64;
            if delay_ms >= remaining {
                return Ok(response);
            }
        }
        abortable_sleep(Duration::from_millis(delay_ms as u64), signal).await?;
        retry += 1;
    }
}

/// Upstream `fetchJson` (github-copilot.ts:197-204): non-2xx →
/// `{status} {statusText}: {body}`; 2xx → the parsed JSON (parse failures
/// propagate with the raw serde text where upstream surfaces SyntaxError).
/// A plain fetch upstream: no per-request timeout, only the caller's signal.
async fn fetch_json(spec: &RequestSpec, signal: &CancellationToken) -> Result<Value, AuthError> {
    let response = send_once(spec, None, None, signal)
        .await
        .map_err(|error| match error {
            SendError::Cancelled => AuthError::Cancelled,
            SendError::Transport(text) => AuthError::Operation(text),
        })?;
    if !response.ok {
        return Err(AuthError::Operation(format!(
            "{} {}: {}",
            response.status, response.reason, response.body
        )));
    }
    serde_json::from_str(&response.body).map_err(|error| AuthError::Operation(error.to_string()))
}

/// Upstream `DeviceCodeResponse` (github-copilot.ts:21-27). `interval` is
/// `undefined` when the server omits it; `expires_in` is required.
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    interval: Option<f64>,
    expires_in: f64,
}

/// Upstream `startDeviceFlow` (github-copilot.ts:206-261): POST the device
/// code request, validate the response fields, and force the
/// `verification_uri` through a URL parse restricted to http(s) so a
/// malicious enterprise server cannot hand the browser launcher a non-URL
/// ("Untrusted verification_uri in device code response"). The normalized
/// href is what reaches the device-code event.
async fn start_device_flow(
    oauth: &GitHubCopilotOAuth,
    domain: &str,
    signal: &CancellationToken,
) -> Result<DeviceCodeResponse, AuthError> {
    // `new URLSearchParams({client_id, scope})` — insertion order.
    let body = {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("client_id", CLIENT_ID);
        query.append_pair("scope", "read:user");
        query.finish()
    };
    let spec = RequestSpec {
        method: Method::Post,
        url: oauth.device_code_url(domain),
        headers: vec![
            ("Accept", "application/json".to_string()),
            (
                "Content-Type",
                "application/x-www-form-urlencoded".to_string(),
            ),
            ("User-Agent", USER_AGENT.to_string()),
        ],
        body: Some(body),
    };
    let json = fetch_json(&spec, signal).await?;

    if !js_is_object(&json) {
        return Err(AuthError::Operation(
            "Invalid device code response".to_string(),
        ));
    }

    let device_code = json.get("device_code").and_then(Value::as_str);
    let user_code = json.get("user_code").and_then(Value::as_str);
    let verification_uri = json.get("verification_uri").and_then(Value::as_str);
    // `interval !== undefined && typeof interval !== "number"`: a missing
    // interval is fine, a null/string/bool one is invalid.
    let interval = match json.get("interval") {
        None => None,
        Some(Value::Number(number)) => Some(number.as_f64().unwrap_or_default()),
        Some(_) => {
            return Err(AuthError::Operation(
                "Invalid device code response fields".to_string(),
            ))
        }
    };
    let expires_in = match json.get("expires_in") {
        Some(Value::Number(number)) => number.as_f64().unwrap_or_default(),
        _ => {
            return Err(AuthError::Operation(
                "Invalid device code response fields".to_string(),
            ))
        }
    };
    let (Some(device_code), Some(user_code), Some(verification_uri)) =
        (device_code, user_code, verification_uri)
    else {
        return Err(AuthError::Operation(
            "Invalid device code response fields".to_string(),
        ));
    };

    // The verification URI is opened in the user's browser and to prevent
    // `open` from opening an executable or similar, we force it to be a URL.
    let parsed = Url::parse(verification_uri).map_err(|_| {
        AuthError::Operation("Untrusted verification_uri in device code response".to_string())
    })?;
    if parsed.scheme() != "https" && parsed.scheme() != "http" {
        return Err(AuthError::Operation(
            "Untrusted verification_uri in device code response".to_string(),
        ));
    }

    Ok(DeviceCodeResponse {
        device_code: device_code.to_string(),
        user_code: user_code.to_string(),
        verification_uri: parsed.to_string(),
        interval,
        expires_in,
    })
}

/// One poll's response mapped to the engine outcome (the `poll` closure of
/// upstream `pollForGitHubAccessToken`, github-copilot.ts:274-309).
/// `fetchJson` failures (transport, non-2xx) propagate un-wrapped like the
/// upstream rejection.
fn device_poll_outcome(raw: &Value) -> PollOutcome<String> {
    if !js_is_object(raw) {
        return PollOutcome::Failed("Invalid device token response".to_string());
    }
    if let Some(token) = raw.get("access_token").and_then(Value::as_str) {
        return PollOutcome::Complete(token.to_string());
    }
    if let Some(error) = raw.get("error").and_then(Value::as_str) {
        if error == "authorization_pending" {
            return PollOutcome::Pending;
        }
        if error == "slow_down" {
            return PollOutcome::SlowDown(raw.get("interval").and_then(Value::as_f64));
        }
        let suffix = raw
            .get("error_description")
            .and_then(Value::as_str)
            .filter(|description| !description.is_empty())
            .map(|description| format!(": {description}"))
            .unwrap_or_default();
        return PollOutcome::Failed(format!("Device flow failed: {error}{suffix}"));
    }
    PollOutcome::Failed("Invalid device token response".to_string())
}

/// Upstream `pollForGitHubAccessToken` (github-copilot.ts:263-311): waits
/// before the first poll, polls `/login/oauth/access_token` until it sees an
/// `access_token` string, `authorization_pending`, `slow_down` (with an
/// optional server interval) or a hard failure.
async fn poll_for_github_access_token(
    oauth: &GitHubCopilotOAuth,
    domain: &str,
    device: &DeviceCodeResponse,
    signal: &CancellationToken,
) -> Result<String, AuthError> {
    let body = {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("client_id", CLIENT_ID);
        query.append_pair("device_code", &device.device_code);
        query.append_pair("grant_type", "urn:ietf:params:oauth:grant-type:device_code");
        query.finish()
    };
    let spec = RequestSpec {
        method: Method::Post,
        url: oauth.access_token_url(domain),
        headers: vec![
            ("Accept", "application/json".to_string()),
            (
                "Content-Type",
                "application/x-www-form-urlencoded".to_string(),
            ),
            ("User-Agent", USER_AGENT.to_string()),
        ],
        body: Some(body),
    };
    let spec = &spec;
    poll_device_code_flow(
        device.interval,
        Some(device.expires_in),
        true,
        signal,
        || async move {
            let raw = fetch_json(spec, signal).await?;
            Ok(device_poll_outcome(&raw))
        },
    )
    .await
}

/// The Copilot access-token exchange result (upstream
/// `refreshGitHubCopilotAccessToken`'s credential, github-copilot.ts:341-347):
/// `expires` carries the 5-minute shave.
struct CopilotToken {
    refresh: String,
    access: String,
    expires: i64,
}

/// Upstream `refreshGitHubCopilotAccessToken` (github-copilot.ts:313-348):
/// GET `/copilot_internal/v2/token` with the GitHub token as Bearer.
async fn refresh_copilot_access_token(
    oauth: &GitHubCopilotOAuth,
    github_token: &str,
    enterprise_domain: Option<&str>,
    signal: &CancellationToken,
) -> Result<CopilotToken, AuthError> {
    let domain = enterprise_domain.unwrap_or(DEFAULT_DOMAIN);
    let mut headers: Vec<(&'static str, String)> = vec![
        ("Accept", "application/json".to_string()),
        ("Authorization", format!("Bearer {github_token}")),
    ];
    headers.extend(copilot_headers());
    let spec = RequestSpec {
        method: Method::Get,
        url: oauth.copilot_token_url(domain),
        headers,
        body: None,
    };
    let json = fetch_json(&spec, signal).await?;

    if !js_is_object(&json) {
        return Err(AuthError::Operation(
            "Invalid Copilot token response".to_string(),
        ));
    }
    let token = json.get("token").and_then(Value::as_str);
    let expires_at = json.get("expires_at").and_then(Value::as_f64);
    let (Some(token), Some(expires_at)) = (token, expires_at) else {
        return Err(AuthError::Operation(
            "Invalid Copilot token response fields".to_string(),
        ));
    };
    Ok(CopilotToken {
        refresh: github_token.to_string(),
        access: token.to_string(),
        // `expiresAt * 1000 - 5 * 60 * 1000`.
        expires: (expires_at * 1000.0 - 5.0 * 60.0 * 1000.0) as i64,
    })
}

/// Upstream `parseGitHubCopilotModelCatalog`'s result (github-copilot.ts:93-133).
#[derive(Debug)]
struct CopilotModelCatalog {
    available_model_ids: Vec<String>,
    policy_model_ids: Vec<String>,
}

/// Upstream `parseGitHubCopilotModelCatalog` (github-copilot.ts:93-133).
/// `known` is upstream's `Object.hasOwn(GITHUB_COPILOT_MODELS, model.id)`
/// (see the module docs for the injected-predicate disclosure).
fn parse_github_copilot_model_catalog(
    raw: &Value,
    allow_policy_fallback: bool,
    known: &dyn Fn(&str) -> bool,
) -> Result<CopilotModelCatalog, AuthError> {
    let Some(data) = raw.get("data").and_then(Value::as_array) else {
        return Err(AuthError::Operation(
            "Invalid Copilot models response".to_string(),
        ));
    };

    // flatMap over the entries: a non-object entry or a non-string id drops
    // it; `supports.tool_calls === false` (strict) drops it too.
    let mut account_models: Vec<(&str, bool, Option<&str>)> = Vec::new();
    for item in data {
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            continue;
        };
        let tool_calls_disabled = item
            .get("capabilities")
            .and_then(|capabilities| capabilities.get("supports"))
            .and_then(|supports| supports.get("tool_calls"))
            == Some(&Value::Bool(false));
        if tool_calls_disabled {
            continue;
        }
        let picker_enabled = item.get("model_picker_enabled") == Some(&Value::Bool(true));
        let policy_state = item
            .get("policy")
            .and_then(|policy| policy.get("state"))
            .and_then(Value::as_str);
        account_models.push((id, picker_enabled, policy_state));
    }

    let picker_model_ids: Vec<&str> = account_models
        .iter()
        .filter(|(_, picker_enabled, policy_state)| {
            *picker_enabled && *policy_state != Some("disabled")
        })
        .map(|(id, _, _)| *id)
        .collect();
    let use_policy_fallback = allow_policy_fallback && picker_model_ids.is_empty();
    let available_model_ids: Vec<String> = if !picker_model_ids.is_empty() || !allow_policy_fallback
    {
        picker_model_ids
            .iter()
            .map(|id| (*id).to_string())
            .collect()
    } else {
        account_models
            .iter()
            .filter(|(_, _, policy_state)| *policy_state == Some("enabled"))
            .map(|(id, _, _)| (*id).to_string())
            .collect()
    };
    let policy_model_ids: Vec<String> = account_models
        .iter()
        .filter(|(id, picker_enabled, policy_state)| {
            *policy_state == Some("unconfigured")
                && known(id)
                && (*picker_enabled || use_policy_fallback)
        })
        .map(|(id, _, _)| (*id).to_string())
        .collect();
    Ok(CopilotModelCatalog {
        available_model_ids,
        policy_model_ids,
    })
}

/// Upstream `fetchGitHubCopilotModels` (github-copilot.ts:168-195). The
/// policy fallback is limited to the Individual endpoint (the derived base —
/// never the test override).
async fn fetch_models(
    oauth: &GitHubCopilotOAuth,
    copilot_token: &str,
    enterprise_domain: Option<&str>,
    signal: &CancellationToken,
    max_retries: u32,
    max_elapsed_ms: u64,
) -> Result<CopilotModelCatalog, AuthError> {
    let derived_base = get_github_copilot_base_url(Some(copilot_token), enterprise_domain);
    // Some Individual accounts return false for every picker flag despite
    // explicit enabled policies. Limit the fallback to that endpoint so other
    // account types keep strict picker semantics.
    let allow_policy_fallback = derived_base == INDIVIDUAL_API_BASE;
    let mut headers: Vec<(&'static str, String)> = vec![
        ("Accept", "application/json".to_string()),
        ("Authorization", format!("Bearer {copilot_token}")),
    ];
    headers.extend(copilot_headers());
    headers.push(("X-GitHub-Api-Version", COPILOT_API_VERSION.to_string()));
    let spec = RequestSpec {
        method: Method::Get,
        url: format!(
            "{}/models",
            oauth.api_base_override.as_deref().unwrap_or(&derived_base)
        ),
        headers,
        body: None,
    };
    let response = fetch_with_rate_limit_retry(
        &spec,
        max_retries,
        max_elapsed_ms,
        oauth.request_timeout,
        signal,
    )
    .await?;
    if !response.ok {
        return Err(AuthError::Operation(format!(
            "{} {}: {}",
            response.status, response.reason, response.body
        )));
    }
    let json: Value = serde_json::from_str(&response.body)
        .map_err(|error| AuthError::Operation(error.to_string()))?;
    parse_github_copilot_model_catalog(&json, allow_policy_fallback, oauth.known_model_ids.as_ref())
}

/// Upstream `enableGitHubCopilotModel` (github-copilot.ts:373-408): POST the
/// policy update with a 2-retry/5s budget. Transport/budget failures count
/// as not-enabled (`false`); a surviving 429 errors; aborts propagate.
async fn enable_github_copilot_model(
    oauth: &GitHubCopilotOAuth,
    token: &str,
    model_id: &str,
    enterprise_domain: Option<&str>,
    signal: &CancellationToken,
) -> Result<bool, AuthError> {
    let derived_base = get_github_copilot_base_url(Some(token), enterprise_domain);
    let mut headers: Vec<(&'static str, String)> = vec![
        ("Content-Type", "application/json".to_string()),
        ("Authorization", format!("Bearer {token}")),
    ];
    headers.extend(copilot_headers());
    headers.push(("openai-intent", "chat-policy".to_string()));
    headers.push(("x-interaction-type", "chat-policy".to_string()));
    let spec = RequestSpec {
        method: Method::Post,
        url: format!(
            "{}/models/{model_id}/policy",
            oauth.api_base_override.as_deref().unwrap_or(&derived_base)
        ),
        headers,
        body: Some(r#"{"state":"enabled"}"#.to_string()),
    };
    let response =
        match fetch_with_rate_limit_retry(&spec, 2, 5000, oauth.request_timeout, signal).await {
            Ok(response) => response,
            Err(AuthError::Cancelled) => return Err(AuthError::Cancelled),
            // Upstream catch: `if (signal.aborted) throw error; return false`.
            Err(_) => return Ok(false),
        };
    if response.status == 429 {
        return Err(AuthError::Operation(format!(
            "{} {}: {}",
            response.status, response.reason, response.body
        )));
    }
    Ok(response.ok)
}

/// Upstream `enableGitHubCopilotModels` (github-copilot.ts:414-432): best
/// effort, in order; a hard failure stops the batch, aborts propagate.
async fn enable_github_copilot_models(
    oauth: &GitHubCopilotOAuth,
    token: &str,
    model_ids: &[String],
    enterprise_domain: Option<&str>,
    signal: &CancellationToken,
) -> Result<Vec<String>, AuthError> {
    let mut enabled_model_ids = Vec::new();
    for model_id in model_ids {
        match enable_github_copilot_model(oauth, token, model_id, enterprise_domain, signal).await {
            Ok(true) => enabled_model_ids.push(model_id.clone()),
            Ok(false) => {}
            Err(AuthError::Cancelled) => return Err(AuthError::Cancelled),
            // Upstream: `if (signal.aborted) throw error; break`.
            Err(_) => break,
        }
    }
    Ok(enabled_model_ids)
}

/// The credential extension fields upstream spreads onto the OAuth
/// credential: `enterpriseUrl` (when set) and `availableModelIds`.
fn copilot_credential_extra(
    enterprise_domain: Option<&str>,
    available_model_ids: Vec<String>,
) -> BTreeMap<String, Value> {
    let mut extra = BTreeMap::new();
    if let Some(domain) = enterprise_domain {
        extra.insert(
            "enterpriseUrl".to_string(),
            Value::String(domain.to_string()),
        );
    }
    extra.insert(
        "availableModelIds".to_string(),
        Value::Array(available_model_ids.into_iter().map(Value::String).collect()),
    );
    extra
}

/// Upstream `copilotEnterpriseDomain` (github-copilot.ts:487-491): the
/// stored `enterpriseUrl` extension field, re-normalized.
fn copilot_enterprise_domain(credential: &OAuthCredential) -> Option<String> {
    credential
        .extra
        .get("enterpriseUrl")
        .and_then(Value::as_str)
        .filter(|domain| !domain.is_empty())
        .and_then(normalize_domain)
}

/// Upstream `loginGitHubCopilot` (github-copilot.ts:434-485): prompt for the
/// enterprise domain, run the device flow, exchange for the Copilot token,
/// fetch the model catalog, enable the known unconfigured models.
async fn login_github_copilot(
    oauth: &GitHubCopilotOAuth,
    interaction: ProviderAuthInteraction,
) -> Result<OAuthCredential, AuthError> {
    let input = interaction
        .prompt(AuthPrompt {
            signal: None,
            kind: AuthPromptKind::Text {
                message: "GitHub Enterprise URL/domain (blank for github.com)".to_string(),
                placeholder: Some("company.ghe.com".to_string()),
            },
        })
        .await?;
    if interaction.signal.is_cancelled() {
        return Err(AuthError::Cancelled);
    }

    let trimmed = input.trim();
    let enterprise_domain = normalize_domain(&input);
    if !trimmed.is_empty() && enterprise_domain.is_none() {
        return Err(AuthError::Operation(
            "Invalid GitHub Enterprise URL/domain".to_string(),
        ));
    }
    let domain = enterprise_domain
        .clone()
        .unwrap_or_else(|| DEFAULT_DOMAIN.to_string());
    let domain_arg = enterprise_domain.as_deref();

    let device = start_device_flow(oauth, &domain, &interaction.signal).await?;
    interaction.notify(AuthEvent::DeviceCode {
        user_code: device.user_code.clone(),
        verification_uri: device.verification_uri.clone(),
        // The event field is u64: fractional intervals truncate, negatives
        // clamp (disclosed in the module docs).
        interval_seconds: device.interval.map(|seconds| seconds.max(0.0) as u64),
        expires_in_seconds: Some(device.expires_in.max(0.0) as u64),
    });

    let github_access_token =
        poll_for_github_access_token(oauth, &domain, &device, &interaction.signal).await?;
    let credentials =
        refresh_copilot_access_token(oauth, &github_access_token, domain_arg, &interaction.signal)
            .await?;
    let models = fetch_models(
        oauth,
        &credentials.access,
        domain_arg,
        &interaction.signal,
        2,
        5000,
    )
    .await?;
    let mut enabled_model_ids = Vec::new();
    if !models.policy_model_ids.is_empty() {
        interaction.notify(AuthEvent::Progress {
            message: "Enabling models...".to_string(),
        });
        enabled_model_ids = enable_github_copilot_models(
            oauth,
            &credentials.access,
            &models.policy_model_ids,
            domain_arg,
            &interaction.signal,
        )
        .await?;
    }

    // `[...new Set([...models.availableModelIds, ...enabledModelIds])]`:
    // first occurrence wins.
    let mut available_model_ids = models.available_model_ids;
    for model_id in enabled_model_ids {
        if !available_model_ids.contains(&model_id) {
            available_model_ids.push(model_id);
        }
    }
    Ok(OAuthCredential {
        refresh: credentials.refresh,
        access: credentials.access,
        expires: credentials.expires,
        extra: copilot_credential_extra(domain_arg, available_model_ids),
    })
}

/// Upstream `refreshGitHubCopilotToken` (github-copilot.ts:353-367): the
/// token exchange plus a zero-budget catalog fetch producing
/// `availableModelIds`.
async fn refresh_github_copilot_token(
    oauth: &GitHubCopilotOAuth,
    refresh_token: &str,
    enterprise_domain: Option<&str>,
    signal: &CancellationToken,
) -> Result<OAuthCredential, AuthError> {
    let credentials =
        refresh_copilot_access_token(oauth, refresh_token, enterprise_domain, signal).await?;
    let models = fetch_models(oauth, &credentials.access, enterprise_domain, signal, 0, 0).await?;
    Ok(OAuthCredential {
        refresh: credentials.refresh,
        access: credentials.access,
        expires: credentials.expires,
        extra: copilot_credential_extra(enterprise_domain, models.available_model_ids),
    })
}

impl OAuthAuth for GitHubCopilotOAuth {
    /// Upstream `name` (github-copilot.ts:494).
    fn name(&self) -> &str {
        "GitHub Copilot"
    }

    /// Upstream `isSubscription: true` (github-copilot.ts:495).
    fn is_subscription(&self) -> bool {
        true
    }

    /// Upstream `login: loginGitHubCopilot` (github-copilot.ts:496).
    fn login<'a>(
        &'a self,
        interaction: ProviderAuthInteraction,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(login_github_copilot(self, interaction))
    }

    /// Upstream `refresh` (github-copilot.ts:497-498): the refresh token is
    /// the stored credential's, the domain its `enterpriseUrl` field.
    fn refresh<'a>(
        &'a self,
        credential: OAuthCredential,
        options: &'a crate::ai::auth::types::AuthOperationOptions,
    ) -> BoxFuture<'a, Result<OAuthCredential, AuthError>> {
        Box::pin(async move {
            let signal = options.signal.clone().unwrap_or_default();
            let enterprise_domain = copilot_enterprise_domain(&credential);
            refresh_github_copilot_token(
                self,
                &credential.refresh,
                enterprise_domain.as_deref(),
                &signal,
            )
            .await
        })
    }

    /// Upstream `toAuth` (github-copilot.ts:500-506): the access token as
    /// api key, the token/enterprise-derived proxy endpoint as baseUrl.
    fn to_auth<'a>(
        &'a self,
        credential: OAuthCredential,
    ) -> BoxFuture<'a, Result<ModelAuth, AuthError>> {
        Box::pin(async move {
            let enterprise_domain = copilot_enterprise_domain(&credential);
            let base_url =
                get_github_copilot_base_url(Some(&credential.access), enterprise_domain.as_deref());
            Ok(ModelAuth {
                api_key: Some(credential.access),
                headers: None,
                base_url: Some(base_url),
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

    use super::super::{read_request_head, write_response};
    use super::*;
    use crate::ai::auth::oauth::device_code::SLOW_DOWN_TIMEOUT_MESSAGE;
    use crate::ai::auth::types::{AuthInteraction, AuthOperationOptions};
    use crate::ai::now_ms;

    const TEST_COPILOT_ACCESS_TOKEN: &str =
        "tid=test;exp=9999999999;proxy-ep=proxy.individual.githubcopilot.com;";

    // ---- Interaction fakes (T3/T4 rig) ----

    type Respond =
        Box<dyn Fn(AuthPrompt) -> BoxFuture<'static, Result<String, AuthError>> + Send + Sync>;

    struct FakeInteraction {
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
            self.events.lock().unwrap().push(event);
        }
    }

    fn fake_interaction(respond: Respond) -> (Arc<FakeInteraction>, ProviderAuthInteraction) {
        let fake = Arc::new(FakeInteraction {
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

    /// Answers the enterprise-domain text prompt with a fixed answer.
    fn login_interaction(answer: &'static str) -> (Arc<FakeInteraction>, ProviderAuthInteraction) {
        fake_interaction(Box::new(move |prompt| {
            Box::pin(async move {
                match &prompt.kind {
                    AuthPromptKind::Text { .. } => Ok(answer.to_string()),
                    other => panic!("unexpected prompt: {other:?}"),
                }
            })
        }))
    }

    // ---- wiremock helpers ----

    fn enterprise_flow(
        server: &MockServer,
        known: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    ) -> GitHubCopilotOAuth {
        GitHubCopilotOAuth::with_endpoints(
            Some(server.uri()),
            Some(format!("{}/copilot_internal/v2/token", server.uri())),
            Some(server.uri()),
            known,
        )
    }

    fn default_known() -> Arc<dyn Fn(&str) -> bool + Send + Sync> {
        Arc::new(|_| false)
    }

    fn known_ids(ids: &[&str]) -> Arc<dyn Fn(&str) -> bool + Send + Sync> {
        let owned: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
        Arc::new(move |id| owned.iter().any(|known| known == id))
    }

    async fn mount_json(
        server: &MockServer,
        http_method: &str,
        route: &str,
        body: &str,
        status: u16,
    ) {
        Mock::given(method(http_method))
            .and(path(route))
            .respond_with(
                ResponseTemplate::new(status).set_body_raw(body.to_string(), "application/json"),
            )
            .mount(server)
            .await;
    }

    /// Serves the queued `(status, body)` JSON responses in order — the
    /// wiremock analog of the oracle's `pollResponses.shift()`. A poll past
    /// the queue gets a 500, failing the test loudly.
    async fn mount_json_queue(
        server: &MockServer,
        http_method: &str,
        route: &str,
        responses: Vec<(u16, String)>,
    ) {
        use std::collections::VecDeque;
        let queue = Arc::new(Mutex::new(VecDeque::from(responses)));
        Mock::given(method(http_method))
            .and(path(route))
            .respond_with(move |_request: &wiremock::Request| {
                let mut queue = queue.lock().unwrap();
                let (status, body) = queue
                    .pop_front()
                    .unwrap_or((500, "unexpected extra poll".to_string()));
                ResponseTemplate::new(status).set_body_raw(body, "application/json")
            })
            .mount(server)
            .await;
    }

    fn device_code_ok() -> String {
        r#"{"device_code":"device-code","user_code":"ABCD-EFGH","verification_uri":"https://github.com/login/device","interval":1,"expires_in":900}"#.to_string()
    }

    fn copilot_token_ok(token: &str) -> String {
        format!(
            r#"{{"token":{},"expires_at":9999999999}}"#,
            serde_json::to_string(token).unwrap()
        )
    }

    fn oauth_credential(access: &str, refresh: &str) -> OAuthCredential {
        OAuthCredential {
            refresh: refresh.to_string(),
            access: access.to_string(),
            expires: 0,
            extra: Default::default(),
        }
    }

    fn header_value(request: &wiremock::Request, name: &str) -> Option<String> {
        request
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    // ---- Oracle ports (packages/ai/test/github-copilot-oauth.test.ts) ----

    /// Oracle: "filters models to the authenticated account picker catalog".
    /// The store/getAvailable half of the oracle is M2e (Models collection).
    #[tokio::test]
    async fn filters_models_to_the_authenticated_account_picker_catalog() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "GET",
            "/copilot_internal/v2/token",
            &copilot_token_ok(TEST_COPILOT_ACCESS_TOKEN),
            200,
        )
        .await;
        mount_json(
            &server,
            "GET",
            "/models",
            r#"{"data":[
                {"id":"picker-model","model_picker_enabled":true,"capabilities":{"supports":{"tool_calls":true}}},
                {"id":"disabled-model","model_picker_enabled":true,"policy":{"state":"disabled"},"capabilities":{"supports":{"tool_calls":true}}},
                {"id":"hidden-model","model_picker_enabled":false,"policy":{"state":"enabled"},"capabilities":{"supports":{"tool_calls":true}}}
            ]}"#,
            200,
        )
        .await;
        let oauth = enterprise_flow(&server, default_known());

        let credential = oauth
            .refresh(
                oauth_credential("old-access-token", "ghu_refresh_token"),
                &AuthOperationOptions::default(),
            )
            .await
            .unwrap();

        assert_eq!(
            credential.extra.get("availableModelIds"),
            Some(&serde_json::json!(["picker-model"]))
        );
        // The refresh token is preserved (upstream `refresh: refreshToken`).
        assert_eq!(credential.refresh, "ghu_refresh_token");
        assert_eq!(credential.access, TEST_COPILOT_ACCESS_TOKEN);
        // expires = expires_at * 1000 - 5 minutes.
        assert!(
            (credential.expires - (9_999_999_999_i64 * 1000 - 5 * 60 * 1000)).abs() <= 2000,
            "expires {}",
            credential.expires
        );
    }

    /// Oracle: "falls back to explicitly enabled policy models when the
    /// picker catalog is empty".
    #[tokio::test]
    async fn falls_back_to_policy_enabled_models_when_the_picker_catalog_is_empty() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "GET",
            "/copilot_internal/v2/token",
            &copilot_token_ok(TEST_COPILOT_ACCESS_TOKEN),
            200,
        )
        .await;
        mount_json(
            &server,
            "GET",
            "/models",
            r#"{"data":[
                {"id":"enabled-model","model_picker_enabled":false,"policy":{"state":"enabled"},"capabilities":{"supports":{"tool_calls":true}}},
                {"id":"policy-disabled-model","model_picker_enabled":false,"policy":{"state":"disabled"},"capabilities":{"supports":{"tool_calls":true}}},
                {"id":"unconfigured-model","model_picker_enabled":false,"capabilities":{"supports":{"tool_calls":true}}},
                {"id":"tool-incapable-model","model_picker_enabled":false,"policy":{"state":"enabled"},"capabilities":{"supports":{"tool_calls":false}}}
            ]}"#,
            200,
        )
        .await;
        let oauth = enterprise_flow(&server, default_known());

        let credential = oauth
            .refresh(
                oauth_credential("old-access-token", "ghu_refresh_token"),
                &AuthOperationOptions::default(),
            )
            .await
            .unwrap();

        assert_eq!(
            credential.extra.get("availableModelIds"),
            Some(&serde_json::json!(["enabled-model"]))
        );
    }

    /// Oracle: "does not fall back to policy models for non-Individual
    /// accounts".
    #[tokio::test]
    async fn does_not_fall_back_to_policy_models_for_non_individual_accounts() {
        let server = MockServer::start().await;
        let business_token = "tid=test;exp=9999999999;proxy-ep=proxy.business.githubcopilot.com;";
        mount_json(
            &server,
            "GET",
            "/copilot_internal/v2/token",
            &copilot_token_ok(business_token),
            200,
        )
        .await;
        mount_json(
            &server,
            "GET",
            "/models",
            r#"{"data":[
                {"id":"gpt-4.1","model_picker_enabled":false,"policy":{"state":"enabled"},"capabilities":{"supports":{"tool_calls":true}}}
            ]}"#,
            200,
        )
        .await;
        let oauth = enterprise_flow(&server, default_known());

        let credential = oauth
            .refresh(
                oauth_credential("old-access-token", "ghu_refresh_token"),
                &AuthOperationOptions::default(),
            )
            .await
            .unwrap();

        assert_eq!(
            credential.extra.get("availableModelIds"),
            Some(&serde_json::json!([]))
        );
    }

    /// Oracle: "does not retry model catalog throttling during credential
    /// refresh" (refresh uses a zero retry budget).
    #[tokio::test]
    async fn does_not_retry_model_catalog_throttling_during_credential_refresh() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "GET",
            "/copilot_internal/v2/token",
            &copilot_token_ok(TEST_COPILOT_ACCESS_TOKEN),
            200,
        )
        .await;
        mount_json(
            &server,
            "GET",
            "/models",
            r#"{"error":"too many requests"}"#,
            429,
        )
        .await;
        let oauth = enterprise_flow(&server, default_known());

        let error = oauth
            .refresh(
                oauth_credential("old-access-token", "ghu_refresh_token"),
                &AuthOperationOptions::default(),
            )
            .await
            .unwrap_err();

        assert!(
            error.to_string().contains("429"),
            "expected a 429 error, got {error}"
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2, "exactly one catalog request");
    }

    /// Oracle: "reports device-code details through onDeviceCode" plus the
    /// wire-level request pins (bodies, headers) and the credential shape.
    #[tokio::test]
    async fn reports_device_code_details_through_on_device_code() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "POST",
            "/login/device/code",
            &device_code_ok(),
            200,
        )
        .await;
        mount_json(
            &server,
            "POST",
            "/login/oauth/access_token",
            r#"{"access_token":"ghu_refresh_token"}"#,
            200,
        )
        .await;
        mount_json(
            &server,
            "GET",
            "/copilot_internal/v2/token",
            &copilot_token_ok(TEST_COPILOT_ACCESS_TOKEN),
            200,
        )
        .await;
        mount_json(&server, "GET", "/models", r#"{"data":[]}"#, 200).await;
        let oauth = enterprise_flow(&server, default_known());
        let (fake, interaction) = login_interaction("");

        let credential = oauth.login(interaction).await.unwrap();

        let events = fake.events.lock().unwrap().clone();
        assert_eq!(
            events,
            vec![AuthEvent::DeviceCode {
                user_code: "ABCD-EFGH".to_string(),
                verification_uri: "https://github.com/login/device".to_string(),
                interval_seconds: Some(1),
                expires_in_seconds: Some(900),
            }]
        );

        assert_eq!(credential.refresh, "ghu_refresh_token");
        assert_eq!(credential.access, TEST_COPILOT_ACCESS_TOKEN);
        assert!(
            (credential.expires - (9_999_999_999_i64 * 1000 - 5 * 60 * 1000)).abs() <= 2000,
            "expires {}",
            credential.expires
        );
        assert_eq!(credential.extra.get("enterpriseUrl"), None);
        assert_eq!(
            credential.extra.get("availableModelIds"),
            Some(&serde_json::json!([]))
        );

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 4);
        // Device-code request: form body in URLSearchParams insertion order.
        assert_eq!(requests[0].method, "POST");
        assert_eq!(
            String::from_utf8(requests[0].body.clone()).unwrap(),
            "client_id=Iv1.b507a08c87ecfe98&scope=read%3Auser"
        );
        assert_eq!(
            header_value(&requests[0], "user-agent").as_deref(),
            Some("GitHubCopilotChat/0.35.0")
        );
        assert_eq!(
            header_value(&requests[0], "accept").as_deref(),
            Some("application/json")
        );
        assert_eq!(
            header_value(&requests[0], "content-type").as_deref(),
            Some("application/x-www-form-urlencoded")
        );
        // Access-token poll: client_id, device_code, grant_type in order.
        assert_eq!(
            String::from_utf8(requests[1].body.clone()).unwrap(),
            "client_id=Iv1.b507a08c87ecfe98&device_code=device-code&grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code"
        );
        // Copilot token exchange: Bearer github token + Copilot headers.
        assert_eq!(requests[2].method, "GET");
        assert_eq!(
            header_value(&requests[2], "authorization").as_deref(),
            Some("Bearer ghu_refresh_token")
        );
        assert_eq!(
            header_value(&requests[2], "editor-version").as_deref(),
            Some("vscode/1.107.0")
        );
        assert_eq!(
            header_value(&requests[2], "editor-plugin-version").as_deref(),
            Some("copilot-chat/0.35.0")
        );
        assert_eq!(
            header_value(&requests[2], "copilot-integration-id").as_deref(),
            Some("vscode-chat")
        );
        // Models catalog: Bearer copilot token + api version header.
        assert_eq!(requests[3].method, "GET");
        assert_eq!(
            header_value(&requests[3], "authorization").as_deref(),
            Some(format!("Bearer {TEST_COPILOT_ACCESS_TOKEN}").as_str())
        );
        assert_eq!(
            header_value(&requests[3], "x-github-api-version").as_deref(),
            Some("2026-06-01")
        );
    }

    /// Oracle: "updates only known, tool-capable, unconfigured account model
    /// policies" (the oracle's provider-catalog ids map to the injected
    /// known-models predicate here).
    #[tokio::test]
    async fn updates_only_known_tool_capable_unconfigured_model_policies() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "POST",
            "/login/device/code",
            &device_code_ok(),
            200,
        )
        .await;
        mount_json(
            &server,
            "POST",
            "/login/oauth/access_token",
            r#"{"access_token":"ghu_refresh_token"}"#,
            200,
        )
        .await;
        mount_json(
            &server,
            "GET",
            "/copilot_internal/v2/token",
            &copilot_token_ok(TEST_COPILOT_ACCESS_TOKEN),
            200,
        )
        .await;
        mount_json(
            &server,
            "GET",
            "/models",
            r#"{"data":[
                {"id":"configured-model","model_picker_enabled":true,"policy":{"state":"enabled"},"capabilities":{"supports":{"tool_calls":true}}},
                {"id":"unconfigured-model","model_picker_enabled":true,"policy":{"state":"unconfigured"},"capabilities":{"supports":{"tool_calls":true}}},
                {"id":"remote-only-model","model_picker_enabled":true,"policy":{"state":"unconfigured"},"capabilities":{"supports":{"tool_calls":true}}},
                {"id":"tool-incapable-model","model_picker_enabled":true,"policy":{"state":"unconfigured"},"capabilities":{"supports":{"tool_calls":false}}}
            ]}"#,
            200,
        )
        .await;
        let policy_requests = Arc::new(Mutex::new(Vec::new()));
        let requests = Arc::clone(&policy_requests);
        Mock::given(method("POST"))
            .and(path("/models/unconfigured-model/policy"))
            .respond_with(move |_request: &wiremock::Request| {
                requests.lock().unwrap().push(());
                ResponseTemplate::new(200)
            })
            .mount(&server)
            .await;
        let oauth = enterprise_flow(
            &server,
            known_ids(&[
                "configured-model",
                "unconfigured-model",
                "tool-incapable-model",
            ]),
        );
        let (fake, interaction) = login_interaction("");

        let credential = oauth.login(interaction).await.unwrap();

        assert_eq!(
            policy_requests.lock().unwrap().len(),
            1,
            "only the known, unconfigured, tool-capable model gets a policy update"
        );
        // Catalog fetched exactly once.
        let all = server.received_requests().await.unwrap();
        assert_eq!(
            all.iter()
                .filter(|request| request.url.path() == "/models")
                .count(),
            1,
            "catalog requested once"
        );
        // The enable step adds the newly enabled model to the merged catalog.
        assert_eq!(
            credential.extra.get("availableModelIds"),
            Some(&serde_json::json!([
                "configured-model",
                "unconfigured-model",
                "remote-only-model"
            ]))
        );
        let events = fake.events.lock().unwrap().clone();
        assert!(
            events.contains(&AuthEvent::Progress {
                message: "Enabling models...".to_string()
            }),
            "{events:?}"
        );
    }

    /// Oracle: "retries a throttled policy update after Retry-After".
    #[tokio::test]
    async fn retries_a_throttled_policy_update_after_retry_after() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "POST",
            "/login/device/code",
            &device_code_ok(),
            200,
        )
        .await;
        mount_json(
            &server,
            "POST",
            "/login/oauth/access_token",
            r#"{"access_token":"ghu_refresh_token"}"#,
            200,
        )
        .await;
        mount_json(
            &server,
            "GET",
            "/copilot_internal/v2/token",
            &copilot_token_ok(TEST_COPILOT_ACCESS_TOKEN),
            200,
        )
        .await;
        mount_json(
            &server,
            "GET",
            "/models",
            r#"{"data":[{"id":"model-a","model_picker_enabled":true,"policy":{"state":"unconfigured"}}]}"#,
            200,
        )
        .await;
        let policy_times: Arc<Mutex<Vec<std::time::Instant>>> = Arc::new(Mutex::new(Vec::new()));
        let times = Arc::clone(&policy_times);
        Mock::given(method("POST"))
            .and(path("/models/model-a/policy"))
            .respond_with(move |_request: &wiremock::Request| {
                let count = {
                    let mut times = times.lock().unwrap();
                    times.push(std::time::Instant::now());
                    times.len()
                };
                if count == 1 {
                    ResponseTemplate::new(429)
                        .insert_header("Retry-After", "1")
                        .set_body_raw(
                            r#"{"error":"too many requests"}"#.to_string(),
                            "application/json",
                        )
                } else {
                    ResponseTemplate::new(200)
                }
            })
            .mount(&server)
            .await;
        let oauth = enterprise_flow(&server, known_ids(&["model-a"]));
        let (_fake, interaction) = login_interaction("");

        let credential = oauth.login(interaction).await.unwrap();

        let times = policy_times.lock().unwrap().clone();
        assert_eq!(
            times.len(),
            2,
            "the throttled policy update is retried once"
        );
        let elapsed = times[1].duration_since(times[0]);
        assert!(
            elapsed >= Duration::from_millis(990),
            "the retry honors Retry-After (got {elapsed:?})"
        );
        // The lower bound pins the Retry-After honor; the loose upper bound
        // only rules out pathological waits (upstream pins this with fake
        // timers, which real-time integration tests cannot use).
        assert!(elapsed < Duration::from_secs(30), "{elapsed:?}");
        assert_eq!(
            credential.extra.get("availableModelIds"),
            Some(&serde_json::json!(["model-a"]))
        );
    }

    /// Oracle: "continues policy updates after a transport failure". The api
    /// base points at a raw TCP server: the first policy connection is
    /// dropped (transport failure), the second gets a 200.
    #[tokio::test]
    async fn continues_policy_updates_after_a_transport_failure() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "POST",
            "/login/device/code",
            &device_code_ok(),
            200,
        )
        .await;
        mount_json(
            &server,
            "POST",
            "/login/oauth/access_token",
            r#"{"access_token":"ghu_refresh_token"}"#,
            200,
        )
        .await;
        mount_json(
            &server,
            "GET",
            "/copilot_internal/v2/token",
            &copilot_token_ok(TEST_COPILOT_ACCESS_TOKEN),
            200,
        )
        .await;
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let models_body = r#"{"data":[
            {"id":"model-a","model_picker_enabled":false,"policy":{"state":"unconfigured"}},
            {"id":"model-b","model_picker_enabled":false,"policy":{"state":"unconfigured"}}
        ]}"#
        .to_string();
        tokio::spawn(async move {
            let mut first_policy = true;
            for _ in 0..3 {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let Some(request_line) = read_request_head(&mut stream).await else {
                    continue;
                };
                if request_line.starts_with("GET ") {
                    write_response(&mut stream, 200, "OK", "application/json", &models_body).await;
                } else if first_policy {
                    // Transport failure: accept the connection, then drop it.
                    first_policy = false;
                    drop(stream);
                } else {
                    write_response(&mut stream, 200, "OK", "application/json", "").await;
                }
            }
        });
        let oauth = GitHubCopilotOAuth::with_endpoints(
            Some(server.uri()),
            Some(format!("{}/copilot_internal/v2/token", server.uri())),
            Some(format!("http://{addr}")),
            known_ids(&["model-a", "model-b"]),
        );
        let (_fake, interaction) = login_interaction("");

        let credential = oauth.login(interaction).await.unwrap();

        // Both models were attempted despite the first transport failure, and
        // only the successful one lands in the merged catalog (the picker is
        // empty, so the merged list here is exactly the enabled set).
        assert_eq!(
            credential.extra.get("availableModelIds"),
            Some(&serde_json::json!(["model-b"]))
        );
    }

    /// Oracle: "stops policy updates and persists authentication when the
    /// retry delay exceeds the login budget". The store half is M2e; the
    /// port pins the login half (batch stops, credential still returned).
    #[tokio::test]
    async fn stops_policy_updates_when_the_retry_delay_exceeds_the_login_budget() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "POST",
            "/login/device/code",
            &device_code_ok(),
            200,
        )
        .await;
        mount_json(
            &server,
            "POST",
            "/login/oauth/access_token",
            r#"{"access_token":"ghu_refresh_token"}"#,
            200,
        )
        .await;
        mount_json(
            &server,
            "GET",
            "/copilot_internal/v2/token",
            &copilot_token_ok(TEST_COPILOT_ACCESS_TOKEN),
            200,
        )
        .await;
        mount_json(
            &server,
            "GET",
            "/models",
            r#"{"data":[
                {"id":"first-model","model_picker_enabled":true,"policy":{"state":"unconfigured"}},
                {"id":"second-model","model_picker_enabled":true,"policy":{"state":"unconfigured"}}
            ]}"#,
            200,
        )
        .await;
        let policy_requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        for model in ["first-model", "second-model"] {
            let requests = Arc::clone(&policy_requests);
            Mock::given(method("POST"))
                .and(path(format!("/models/{model}/policy")))
                .respond_with(move |request: &wiremock::Request| {
                    requests
                        .lock()
                        .unwrap()
                        .push(request.url.path().to_string());
                    ResponseTemplate::new(429)
                        .insert_header("Retry-After", "5")
                        .set_body_raw(
                            r#"{"error":"too many requests"}"#.to_string(),
                            "application/json",
                        )
                })
                .mount(&server)
                .await;
        }
        let oauth = enterprise_flow(&server, known_ids(&["first-model", "second-model"]));
        let (_fake, interaction) = login_interaction("");

        // Login still succeeds: the batch stops, the credential is returned.
        let credential = oauth.login(interaction).await.unwrap();

        assert_eq!(credential.access, TEST_COPILOT_ACCESS_TOKEN);
        let attempted = policy_requests.lock().unwrap().clone();
        assert_eq!(attempted, vec!["/models/first-model/policy".to_string()]);
    }

    /// Oracle: "rejects a non-http(s) verification_uri before it reaches
    /// onDeviceCode".
    #[tokio::test]
    async fn rejects_a_non_https_verification_uri() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "POST",
            "/login/device/code",
            r#"{"device_code":"device-code","user_code":"ABCD-EFGH","verification_uri":"$(id>/tmp/pwned)","interval":1,"expires_in":900}"#,
            200,
        )
        .await;
        let oauth = enterprise_flow(&server, default_known());
        let (fake, interaction) = login_interaction("");

        let error = oauth.login(interaction).await.unwrap_err();

        assert!(
            error.to_string().contains("Untrusted verification_uri"),
            "{error}"
        );
        let events = fake.events.lock().unwrap().clone();
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AuthEvent::DeviceCode { .. })),
            "no device-code event before validation: {events:?}"
        );
    }

    /// Oracle: "normalizes verification_uri before it reaches onDeviceCode".
    #[tokio::test]
    async fn normalizes_verification_uri_before_it_reaches_on_device_code() {
        let raw_verification_uri = "https://github.com/login/\x1b]8;;evil";
        let normalized = Url::parse(raw_verification_uri).unwrap().to_string();
        assert_ne!(normalized, raw_verification_uri);

        let server = MockServer::start().await;
        mount_json(
            &server,
            "POST",
            "/login/device/code",
            &format!(
                r#"{{"device_code":"device-code","user_code":"ABCD-EFGH","verification_uri":{},"interval":1,"expires_in":900}}"#,
                serde_json::to_string(raw_verification_uri).unwrap()
            ),
            200,
        )
        .await;
        mount_json(
            &server,
            "POST",
            "/login/oauth/access_token",
            r#"{"access_token":"ghu_refresh_token"}"#,
            200,
        )
        .await;
        mount_json(
            &server,
            "GET",
            "/copilot_internal/v2/token",
            &copilot_token_ok(TEST_COPILOT_ACCESS_TOKEN),
            200,
        )
        .await;
        mount_json(&server, "GET", "/models", r#"{"data":[]}"#, 200).await;
        let oauth = enterprise_flow(&server, default_known());
        let (fake, interaction) = login_interaction("");

        oauth.login(interaction).await.unwrap();

        let events = fake.events.lock().unwrap().clone();
        let Some(AuthEvent::DeviceCode {
            verification_uri, ..
        }) = events.first()
        else {
            panic!("expected a device-code event, got {events:?}");
        };
        assert_eq!(verification_uri, &normalized);
    }

    /// Oracle: "waits before polling and increases the interval after
    /// slow_down" (the wire-level body assertions live in the login test
    /// above; the schedule is pinned on the paused clock).
    #[tokio::test(start_paused = true)]
    async fn engine_waits_before_polling_and_adopts_the_slow_down_interval() {
        let signal = CancellationToken::new();
        let times = Arc::new(Mutex::new(Vec::new()));
        let outcomes = Arc::new(Mutex::new(std::collections::VecDeque::from(vec![
            PollOutcome::<()>::Pending,
            PollOutcome::<()>::SlowDown(Some(7.0)),
            PollOutcome::<()>::Complete(()),
        ])));
        let start = tokio::time::Instant::now();
        let poll = {
            let times = Arc::clone(&times);
            let outcomes = Arc::clone(&outcomes);
            move || {
                let times = Arc::clone(&times);
                let outcomes = Arc::clone(&outcomes);
                async move {
                    times.lock().unwrap().push(tokio::time::Instant::now());
                    Ok(outcomes
                        .lock()
                        .unwrap()
                        .pop_front()
                        .unwrap_or(PollOutcome::Pending))
                }
            }
        };

        poll_device_code_flow(Some(5.0), Some(900.0), true, &signal, poll)
            .await
            .unwrap();

        let times = times.lock().unwrap().clone();
        assert_eq!(
            times,
            vec![
                start + Duration::from_secs(5),
                start + Duration::from_secs(10),
                start + Duration::from_secs(17)
            ]
        );
    }

    /// Oracle: "times out after repeated slow_down responses".
    #[tokio::test(start_paused = true)]
    async fn engine_times_out_after_repeated_slow_down_responses() {
        let signal = CancellationToken::new();
        let times = Arc::new(Mutex::new(Vec::new()));
        let outcomes = Arc::new(Mutex::new(std::collections::VecDeque::from(vec![
            PollOutcome::<()>::SlowDown(None),
            PollOutcome::<()>::SlowDown(None),
            PollOutcome::<()>::Pending,
        ])));
        let start = tokio::time::Instant::now();
        let poll = {
            let times = Arc::clone(&times);
            let outcomes = Arc::clone(&outcomes);
            move || {
                let times = Arc::clone(&times);
                let outcomes = Arc::clone(&outcomes);
                async move {
                    times.lock().unwrap().push(tokio::time::Instant::now());
                    Ok(outcomes
                        .lock()
                        .unwrap()
                        .pop_front()
                        .unwrap_or(PollOutcome::Pending))
                }
            }
        };

        let error = poll_device_code_flow(Some(5.0), Some(25.0), true, &signal, poll)
            .await
            .unwrap_err();

        assert_eq!(
            error,
            AuthError::Operation(SLOW_DOWN_TIMEOUT_MESSAGE.to_string())
        );
        let times = times.lock().unwrap().clone();
        assert_eq!(
            times,
            vec![
                start + Duration::from_secs(5),
                start + Duration::from_secs(15)
            ]
        );
    }

    // ---- Engine: port-specific coverage for this module's copy ----

    #[tokio::test(start_paused = true)]
    async fn engine_polls_immediately_when_wait_before_first_poll_is_false() {
        let signal = CancellationToken::new();
        let times = Arc::new(Mutex::new(Vec::new()));
        let outcomes = Arc::new(Mutex::new(std::collections::VecDeque::from(vec![
            PollOutcome::<()>::Pending,
            PollOutcome::<()>::Complete(()),
        ])));
        let start = tokio::time::Instant::now();
        let poll = {
            let times = Arc::clone(&times);
            let outcomes = Arc::clone(&outcomes);
            move || {
                let times = Arc::clone(&times);
                let outcomes = Arc::clone(&outcomes);
                async move {
                    times.lock().unwrap().push(tokio::time::Instant::now());
                    Ok(outcomes
                        .lock()
                        .unwrap()
                        .pop_front()
                        .unwrap_or(PollOutcome::Pending))
                }
            }
        };

        poll_device_code_flow(Some(5.0), Some(900.0), false, &signal, poll)
            .await
            .unwrap();

        assert_eq!(
            times.lock().unwrap().clone(),
            vec![start, start + Duration::from_secs(5)]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn engine_cancelled_signal_aborts_before_and_during_the_first_wait() {
        // Pre-cancelled: no poll at all.
        let signal = CancellationToken::new();
        signal.cancel();
        let times = Arc::new(Mutex::new(Vec::new()));
        let error = poll_device_code_flow(Some(5.0), Some(900.0), true, &signal, {
            let times = Arc::clone(&times);
            move || {
                let times = Arc::clone(&times);
                async move {
                    times.lock().unwrap().push(tokio::time::Instant::now());
                    Ok(PollOutcome::<()>::Pending)
                }
            }
        })
        .await
        .unwrap_err();
        assert_eq!(error, AuthError::Cancelled);
        assert!(times.lock().unwrap().is_empty());

        // Cancelled during the wait-before-first-poll.
        let signal = CancellationToken::new();
        let driver_signal = signal.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            driver_signal.cancel();
        });
        let error = poll_device_code_flow(Some(5.0), Some(900.0), true, &signal, {
            let times = Arc::clone(&times);
            move || {
                let times = Arc::clone(&times);
                async move {
                    times.lock().unwrap().push(tokio::time::Instant::now());
                    Ok(PollOutcome::<()>::Pending)
                }
            }
        })
        .await
        .unwrap_err();
        assert_eq!(error, AuthError::Cancelled);
        assert!(times.lock().unwrap().is_empty());
    }

    // ---- Login surface ----

    /// Enterprise domain: stored on the credential (upstream `enterpriseUrl`)
    /// and routing still works through the injected endpoints. The token has
    /// no proxy-ep, so the derived (production) base would be the enterprise
    /// one — allowPolicyFallback stays false.
    #[tokio::test]
    async fn enterprise_login_stores_the_enterprise_url() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "POST",
            "/login/device/code",
            &device_code_ok(),
            200,
        )
        .await;
        mount_json(
            &server,
            "POST",
            "/login/oauth/access_token",
            r#"{"access_token":"ghu_refresh_token"}"#,
            200,
        )
        .await;
        let enterprise_token = "tid=test;exp=9999999999;";
        mount_json(
            &server,
            "GET",
            "/copilot_internal/v2/token",
            &copilot_token_ok(enterprise_token),
            200,
        )
        .await;
        mount_json(&server, "GET", "/models", r#"{"data":[]}"#, 200).await;
        let oauth = enterprise_flow(&server, default_known());
        let (_fake, interaction) = login_interaction("company.ghe.com");

        let credential = oauth.login(interaction).await.unwrap();

        assert_eq!(
            credential.extra.get("enterpriseUrl"),
            Some(&serde_json::json!("company.ghe.com"))
        );
    }

    #[tokio::test]
    async fn blank_prompt_answer_defaults_to_github_com_without_an_enterprise_url() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "POST",
            "/login/device/code",
            &device_code_ok(),
            200,
        )
        .await;
        mount_json(
            &server,
            "POST",
            "/login/oauth/access_token",
            r#"{"access_token":"ghu_refresh_token"}"#,
            200,
        )
        .await;
        mount_json(
            &server,
            "GET",
            "/copilot_internal/v2/token",
            &copilot_token_ok(TEST_COPILOT_ACCESS_TOKEN),
            200,
        )
        .await;
        mount_json(&server, "GET", "/models", r#"{"data":[]}"#, 200).await;
        let oauth = enterprise_flow(&server, default_known());
        let (_fake, interaction) = login_interaction("   ");

        let credential = oauth.login(interaction).await.unwrap();

        assert_eq!(credential.extra.get("enterpriseUrl"), None);
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 4);
    }

    #[tokio::test]
    async fn invalid_enterprise_domain_is_rejected_without_any_request() {
        let server = MockServer::start().await;
        let oauth = enterprise_flow(&server, default_known());
        let (_fake, interaction) = login_interaction("not a domain!!");

        let error = oauth.login(interaction).await.unwrap_err();

        assert_eq!(
            error,
            AuthError::Operation("Invalid GitHub Enterprise URL/domain".to_string())
        );
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn prompt_cancellation_propagates_as_cancelled() {
        let server = MockServer::start().await;
        let oauth = enterprise_flow(&server, default_known());
        let (_fake, interaction) = fake_interaction(Box::new(|_prompt| {
            Box::pin(async { Err(AuthError::Cancelled) })
        }));

        let result = oauth.login(interaction).await;
        assert_eq!(result, Err(AuthError::Cancelled));
    }

    #[tokio::test]
    async fn entry_cancelled_signal_short_circuits_login() {
        let server = MockServer::start().await;
        let oauth = enterprise_flow(&server, default_known());
        let (_fake, interaction) = login_interaction("");
        interaction.signal.cancel();

        let result = oauth.login(interaction).await;
        assert_eq!(result, Err(AuthError::Cancelled));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    // ---- Device endpoint errors and shapes ----

    #[tokio::test]
    async fn device_code_http_failure_carries_status_and_body() {
        let server = MockServer::start().await;
        mount_json(&server, "POST", "/login/device/code", "nope", 500).await;
        let oauth = enterprise_flow(&server, default_known());
        let (_fake, interaction) = login_interaction("");

        let error = oauth.login(interaction).await.unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("500 Internal Server Error: nope".to_string())
        );
    }

    #[tokio::test]
    async fn device_code_invalid_response_shapes_are_rejected() {
        let cases: Vec<(&str, &str)> = vec![
            // Non-object body -> "Invalid device code response".
            ("42", "Invalid device code response"),
            // An array passes the JS object check, then fails the fields.
            (r#"[]"#, "Invalid device code response fields"),
            // Missing expires_in.
            (
                r#"{"device_code":"d","user_code":"u","verification_uri":"https://github.com/login/device"}"#,
                "Invalid device code response fields",
            ),
            // interval as a string (typeof !== "number").
            (
                r#"{"device_code":"d","user_code":"u","verification_uri":"https://github.com/login/device","interval":"1","expires_in":900}"#,
                "Invalid device code response fields",
            ),
            // interval null (not undefined, not a number).
            (
                r#"{"device_code":"d","user_code":"u","verification_uri":"https://github.com/login/device","interval":null,"expires_in":900}"#,
                "Invalid device code response fields",
            ),
        ];
        for (body, expected) in cases {
            let server = MockServer::start().await;
            mount_json(&server, "POST", "/login/device/code", body, 200).await;
            let oauth = enterprise_flow(&server, default_known());
            let (_fake, interaction) = login_interaction("");
            let error = oauth.login(interaction).await.unwrap_err();
            assert_eq!(error, AuthError::Operation(expected.to_string()), "{body}");
        }
    }

    /// A missing `interval` is valid (RFC 8628 default of 5s applies) and the
    /// event omits it.
    #[tokio::test]
    async fn device_code_without_interval_uses_the_default_and_omits_it_from_the_event() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "POST",
            "/login/device/code",
            r#"{"device_code":"device-code","user_code":"ABCD-EFGH","verification_uri":"https://github.com/login/device","expires_in":900}"#,
            200,
        )
        .await;
        mount_json(
            &server,
            "POST",
            "/login/oauth/access_token",
            r#"{"access_token":"ghu_refresh_token"}"#,
            200,
        )
        .await;
        mount_json(
            &server,
            "GET",
            "/copilot_internal/v2/token",
            &copilot_token_ok(TEST_COPILOT_ACCESS_TOKEN),
            200,
        )
        .await;
        mount_json(&server, "GET", "/models", r#"{"data":[]}"#, 200).await;
        let oauth = enterprise_flow(&server, default_known());
        let (fake, interaction) = login_interaction("");

        oauth.login(interaction).await.unwrap();

        let events = fake.events.lock().unwrap().clone();
        assert_eq!(
            events,
            vec![AuthEvent::DeviceCode {
                user_code: "ABCD-EFGH".to_string(),
                verification_uri: "https://github.com/login/device".to_string(),
                interval_seconds: None,
                expires_in_seconds: Some(900),
            }]
        );
    }

    // ---- Access-token poll errors ----

    #[tokio::test]
    async fn access_token_error_access_denied_fails_the_flow() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "POST",
            "/login/device/code",
            &device_code_ok(),
            200,
        )
        .await;
        mount_json(
            &server,
            "POST",
            "/login/oauth/access_token",
            r#"{"error":"access_denied","error_description":"denied"}"#,
            200,
        )
        .await;
        let oauth = enterprise_flow(&server, default_known());
        let (_fake, interaction) = login_interaction("");

        let error = oauth.login(interaction).await.unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("Device flow failed: access_denied: denied".to_string())
        );
    }

    #[tokio::test]
    async fn access_token_error_without_description_has_no_suffix() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "POST",
            "/login/device/code",
            &device_code_ok(),
            200,
        )
        .await;
        mount_json(
            &server,
            "POST",
            "/login/oauth/access_token",
            r#"{"error":"access_denied"}"#,
            200,
        )
        .await;
        let oauth = enterprise_flow(&server, default_known());
        let (_fake, interaction) = login_interaction("");

        let error = oauth.login(interaction).await.unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("Device flow failed: access_denied".to_string())
        );
    }

    #[tokio::test]
    async fn access_token_invalid_body_fails_the_flow() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "POST",
            "/login/device/code",
            &device_code_ok(),
            200,
        )
        .await;
        mount_json(
            &server,
            "POST",
            "/login/oauth/access_token",
            r#"{"nope":true}"#,
            200,
        )
        .await;
        let oauth = enterprise_flow(&server, default_known());
        let (_fake, interaction) = login_interaction("");

        let error = oauth.login(interaction).await.unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("Invalid device token response".to_string())
        );
    }

    #[tokio::test]
    async fn access_token_http_error_fails_the_flow_with_the_fetch_json_message() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "POST",
            "/login/device/code",
            &device_code_ok(),
            200,
        )
        .await;
        mount_json(&server, "POST", "/login/oauth/access_token", "boom", 500).await;
        let oauth = enterprise_flow(&server, default_known());
        let (_fake, interaction) = login_interaction("");

        let error = oauth.login(interaction).await.unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("500 Internal Server Error: boom".to_string())
        );
    }

    /// slow_down with a server interval completes the flow (wire-level
    /// companion to the paused-clock engine tests).
    #[tokio::test]
    async fn access_token_slow_down_then_success_completes() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "POST",
            "/login/device/code",
            &device_code_ok(),
            200,
        )
        .await;
        mount_json_queue(
            &server,
            "POST",
            "/login/oauth/access_token",
            vec![
                (
                    200,
                    r#"{"error":"slow_down","error_description":"slow down","interval":1}"#
                        .to_string(),
                ),
                (200, r#"{"access_token":"ghu_refresh_token"}"#.to_string()),
            ],
        )
        .await;
        mount_json(
            &server,
            "GET",
            "/copilot_internal/v2/token",
            &copilot_token_ok(TEST_COPILOT_ACCESS_TOKEN),
            200,
        )
        .await;
        mount_json(&server, "GET", "/models", r#"{"data":[]}"#, 200).await;
        let oauth = enterprise_flow(&server, default_known());
        let (_fake, interaction) = login_interaction("");

        let credential = oauth.login(interaction).await.unwrap();
        assert_eq!(credential.refresh, "ghu_refresh_token");
        let polls = server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|request| request.url.path() == "/login/oauth/access_token")
            .count();
        assert_eq!(polls, 2);
    }

    // ---- Copilot token exchange ----

    #[tokio::test]
    async fn copilot_token_response_invalid_shapes_are_rejected() {
        let cases: Vec<(&str, &str)> = vec![
            // An array passes the JS object check, then fails the fields.
            (r#"[1,2]"#, "Invalid Copilot token response fields"),
            (
                r#"{"token":5,"expires_at":1}"#,
                "Invalid Copilot token response fields",
            ),
            (r#"{"token":"t"}"#, "Invalid Copilot token response fields"),
            (r#"null"#, "Invalid Copilot token response"),
        ];
        for (body, expected) in cases {
            let server = MockServer::start().await;
            mount_json(&server, "GET", "/copilot_internal/v2/token", body, 200).await;
            let oauth = enterprise_flow(&server, default_known());
            let error = oauth
                .refresh(
                    oauth_credential("old", "ghu_refresh_token"),
                    &AuthOperationOptions::default(),
                )
                .await
                .unwrap_err();
            assert_eq!(error, AuthError::Operation(expected.to_string()), "{body}");
        }

        // HTTP failure carries the status/body.
        let server = MockServer::start().await;
        mount_json(&server, "GET", "/copilot_internal/v2/token", "nope", 401).await;
        let oauth = enterprise_flow(&server, default_known());
        let error = oauth
            .refresh(
                oauth_credential("old", "ghu_refresh_token"),
                &AuthOperationOptions::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("401 Unauthorized: nope".to_string())
        );
    }

    // ---- Model catalog / policy units ----

    #[tokio::test]
    async fn models_http_failure_carries_status_and_body() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "GET",
            "/copilot_internal/v2/token",
            &copilot_token_ok(TEST_COPILOT_ACCESS_TOKEN),
            200,
        )
        .await;
        mount_json(&server, "GET", "/models", "denied", 403).await;
        let oauth = enterprise_flow(&server, default_known());

        let error = oauth
            .refresh(
                oauth_credential("old", "ghu_refresh_token"),
                &AuthOperationOptions::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error,
            AuthError::Operation("403 Forbidden: denied".to_string())
        );
    }

    /// The `fetchJson` endpoints (device-code start, access-token poll,
    /// Copilot token exchange) have no per-request timeout — upstream's
    /// `AbortSignal.timeout(5000)` lives only inside `fetchWithRateLimitRetry`
    /// (github-copilot.ts:150) — so a slow enterprise server waits on the
    /// interaction signal alone. Approach: shorten the retry-path cap to
    /// 100ms (test seam) and delay the device-code response past it; login
    /// must still succeed, proving the cap does not reach the fetchJson path
    /// (under the pre-fix wiring this test fails with the timeout text).
    #[tokio::test]
    async fn device_code_start_is_not_subject_to_the_rate_limit_request_timeout() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/login/device/code"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(device_code_ok(), "application/json")
                    .set_delay(Duration::from_millis(400)),
            )
            .mount(&server)
            .await;
        mount_json(
            &server,
            "POST",
            "/login/oauth/access_token",
            r#"{"access_token":"ghu_refresh_token"}"#,
            200,
        )
        .await;
        mount_json(
            &server,
            "GET",
            "/copilot_internal/v2/token",
            &copilot_token_ok(TEST_COPILOT_ACCESS_TOKEN),
            200,
        )
        .await;
        mount_json(&server, "GET", "/models", r#"{"data":[]}"#, 200).await;
        let oauth = enterprise_flow(&server, default_known())
            .with_request_timeout(Duration::from_millis(100));
        let (_fake, interaction) = login_interaction("");

        let started = std::time::Instant::now();
        let credential = oauth.login(interaction).await.unwrap();

        assert!(started.elapsed() >= Duration::from_millis(400));
        assert_eq!(credential.refresh, "ghu_refresh_token");
    }

    /// Companion to the uncapped-fetchJson test: the rate-limit-retry path
    /// (`/models`, `/policy`) enforces the per-attempt timeout — the same
    /// shortened cap fails a delayed models response with the timeout text,
    /// and a transport failure is not retried (one catalog request).
    #[tokio::test]
    async fn rate_limit_retry_path_enforces_the_per_request_timeout() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "GET",
            "/copilot_internal/v2/token",
            &copilot_token_ok(TEST_COPILOT_ACCESS_TOKEN),
            200,
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(r#"{"data":[]}"#.to_string(), "application/json")
                    .set_delay(Duration::from_millis(400)),
            )
            .mount(&server)
            .await;
        let oauth = enterprise_flow(&server, default_known())
            .with_request_timeout(Duration::from_millis(100));

        let error = oauth
            .refresh(
                oauth_credential("old-access-token", "ghu_refresh_token"),
                &AuthOperationOptions::default(),
            )
            .await
            .unwrap_err();

        assert!(
            error.to_string().contains("request timed out after 100ms"),
            "{error}"
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.url.path() == "/models")
                .count(),
            1
        );
    }

    #[test]
    fn parse_catalog_rejects_a_missing_or_non_array_data_field() {
        let known = |_id: &str| true;
        for raw in [serde_json::json!({}), serde_json::json!({"data": 5})] {
            let error = parse_github_copilot_model_catalog(&raw, true, &known).unwrap_err();
            assert_eq!(
                error,
                AuthError::Operation("Invalid Copilot models response".to_string())
            );
        }
    }

    #[test]
    fn parse_catalog_skips_non_string_ids_and_keeps_missing_tool_call_flags() {
        let raw = serde_json::json!({
            "data": [
                {"id": 5, "model_picker_enabled": true},
                {"model_picker_enabled": true},
                // A policy that is not an object carries no state.
                {"id": "policy-string", "model_picker_enabled": true, "policy": "unconfigured"},
                // tool_calls only skips on strict false.
                {"id": "kept-null-tool-calls", "model_picker_enabled": true,
                 "capabilities": {"supports": {"tool_calls": null}}}
            ]
        });
        let known = |_: &str| false;
        let catalog = parse_github_copilot_model_catalog(&raw, true, &known).unwrap();
        assert_eq!(
            catalog.available_model_ids,
            vec![
                "policy-string".to_string(),
                "kept-null-tool-calls".to_string()
            ]
        );
        assert_eq!(catalog.policy_model_ids, Vec::<String>::new());
    }

    /// The production default predicate never marks a model known, so no
    /// policy update is attempted for unconfigured models (the static
    /// GITHUB_COPILOT_MODELS catalog lands with the provider wiring).
    #[tokio::test]
    async fn default_known_model_ids_enable_nothing() {
        let server = MockServer::start().await;
        mount_json(
            &server,
            "POST",
            "/login/device/code",
            &device_code_ok(),
            200,
        )
        .await;
        mount_json(
            &server,
            "POST",
            "/login/oauth/access_token",
            r#"{"access_token":"ghu_refresh_token"}"#,
            200,
        )
        .await;
        mount_json(
            &server,
            "GET",
            "/copilot_internal/v2/token",
            &copilot_token_ok(TEST_COPILOT_ACCESS_TOKEN),
            200,
        )
        .await;
        mount_json(
            &server,
            "GET",
            "/models",
            r#"{"data":[{"id":"unconfigured-model","model_picker_enabled":true,"policy":{"state":"unconfigured"}}]}"#,
            200,
        )
        .await;
        let oauth = enterprise_flow(&server, default_known());
        let (_fake, interaction) = login_interaction("");

        let credential = oauth.login(interaction).await.unwrap();

        assert_eq!(
            credential.extra.get("availableModelIds"),
            Some(&serde_json::json!(["unconfigured-model"]))
        );
        let requests = server.received_requests().await.unwrap();
        assert!(
            !requests
                .iter()
                .any(|request| request.url.path().contains("/policy")),
            "no policy updates without a known-models catalog"
        );
    }

    // ---- to_auth ----

    #[tokio::test]
    async fn to_auth_derives_the_base_url_from_the_token() {
        let oauth = GitHubCopilotOAuth::new();

        // proxy-ep in the token wins.
        let auth = oauth
            .to_auth(oauth_credential(TEST_COPILOT_ACCESS_TOKEN, "r"))
            .await
            .unwrap();
        assert_eq!(
            auth,
            ModelAuth {
                api_key: Some(TEST_COPILOT_ACCESS_TOKEN.to_string()),
                headers: None,
                base_url: Some("https://api.individual.githubcopilot.com".to_string()),
            }
        );

        // Without proxy-ep the enterprise domain applies.
        let mut credential = oauth_credential("plain-token", "r");
        credential.extra.insert(
            "enterpriseUrl".to_string(),
            serde_json::json!("company.ghe.com"),
        );
        let auth = oauth.to_auth(credential).await.unwrap();
        assert_eq!(
            auth.base_url.as_deref(),
            Some("https://copilot-api.company.ghe.com")
        );

        // Without either, the individual default.
        let auth = oauth
            .to_auth(oauth_credential("plain-token", "r"))
            .await
            .unwrap();
        assert_eq!(
            auth.base_url.as_deref(),
            Some("https://api.individual.githubcopilot.com")
        );
    }

    #[tokio::test]
    async fn metadata_matches_upstream() {
        let oauth = GitHubCopilotOAuth::new();
        assert_eq!(oauth.name(), "GitHub Copilot");
        assert!(oauth.is_subscription());
        assert_eq!(oauth.login_label(), None);
    }

    // ---- URL derivation / pure helpers ----

    #[test]
    fn production_url_derivation_matches_upstream() {
        let oauth = GitHubCopilotOAuth::new();
        assert_eq!(
            oauth.device_code_url("github.com"),
            "https://github.com/login/device/code"
        );
        assert_eq!(
            oauth.access_token_url("github.com"),
            "https://github.com/login/oauth/access_token"
        );
        assert_eq!(
            oauth.copilot_token_url("github.com"),
            "https://api.github.com/copilot_internal/v2/token"
        );
        assert_eq!(
            oauth.device_code_url("company.ghe.com"),
            "https://company.ghe.com/login/device/code"
        );
        assert_eq!(
            oauth.access_token_url("company.ghe.com"),
            "https://company.ghe.com/login/oauth/access_token"
        );
        assert_eq!(
            oauth.copilot_token_url("company.ghe.com"),
            "https://api.company.ghe.com/copilot_internal/v2/token"
        );
    }

    #[test]
    fn normalize_domain_follows_the_upstream_semantics() {
        assert_eq!(
            normalize_domain("  company.ghe.com  "),
            Some("company.ghe.com".to_string())
        );
        assert_eq!(
            normalize_domain("https://company.ghe.com/x"),
            Some("company.ghe.com".to_string())
        );
        assert_eq!(
            normalize_domain("http://10.0.0.1:8443"),
            Some("10.0.0.1".to_string())
        );
        assert_eq!(normalize_domain(""), None);
        assert_eq!(normalize_domain("   "), None);
        assert_eq!(normalize_domain("$(id>/tmp/pwned)"), None);
        assert_eq!(normalize_domain("not a domain!!"), None);
    }

    #[test]
    fn base_url_derivation_follows_the_upstream_precedence() {
        // Token first, enterprise second, individual default last.
        assert_eq!(
            get_github_copilot_base_url(Some(TEST_COPILOT_ACCESS_TOKEN), Some("company.ghe.com")),
            "https://api.individual.githubcopilot.com"
        );
        assert_eq!(
            get_github_copilot_base_url(Some("tid=1;"), Some("company.ghe.com")),
            "https://copilot-api.company.ghe.com"
        );
        assert_eq!(
            get_github_copilot_base_url(Some("tid=1;"), None),
            "https://api.individual.githubcopilot.com"
        );
        assert_eq!(
            get_github_copilot_base_url(None, None),
            "https://api.individual.githubcopilot.com"
        );
    }

    #[test]
    fn get_base_url_from_token_parses_the_proxy_endpoint() {
        assert_eq!(
            get_base_url_from_token(TEST_COPILOT_ACCESS_TOKEN),
            Some("https://api.individual.githubcopilot.com".to_string())
        );
        // No proxy-ep -> None.
        assert_eq!(get_base_url_from_token("tid=1;exp=2;"), None);
        // Value up to the first semicolon.
        assert_eq!(
            get_base_url_from_token("proxy-ep=proxy.a.b;exp=2"),
            Some("https://api.a.b".to_string())
        );
        // Only a leading "proxy." is rewritten (upstream regex ^proxy\.).
        assert_eq!(
            get_base_url_from_token("proxy-ep=ghec.a.b;"),
            Some("https://ghec.a.b".to_string())
        );
        // An empty value does not match ([^;]+ needs one char).
        assert_eq!(get_base_url_from_token("proxy-ep=;x=1"), None);
    }

    #[test]
    fn js_parse_float_matches_the_prefix_semantics() {
        assert_eq!(js_parse_float("1"), Some(1.0));
        assert_eq!(js_parse_float("2.5x"), Some(2.5));
        assert_eq!(js_parse_float("  3.5s"), Some(3.5));
        assert_eq!(js_parse_float(".5"), Some(0.5));
        assert_eq!(js_parse_float("+1.5e2rest"), Some(150.0));
        assert_eq!(js_parse_float("-0.5"), Some(-0.5));
        assert_eq!(js_parse_float("0x10"), Some(0.0));
        assert_eq!(js_parse_float("1e999"), Some(f64::INFINITY));
        assert_eq!(js_parse_float("Infinity"), Some(f64::INFINITY));
        assert_eq!(js_parse_float("-Infinity"), Some(f64::NEG_INFINITY));
        assert_eq!(js_parse_float(""), None);
        assert_eq!(js_parse_float("abc"), None);
        assert_eq!(js_parse_float("-abc"), None);
    }

    #[test]
    fn retry_after_parsing_honors_seconds_dates_and_gives_up_on_garbage() {
        // Seconds (float).
        assert_eq!(parse_retry_after_ms("1", 1000), Some(1000.0));
        assert_eq!(parse_retry_after_ms("2.5", 1000), Some(2500.0));
        // HTTP-date in the future (IMF-fixdate, like the Retry-After senders
        // use; upstream Date.parse also accepts obsolete forms).
        let date = "Fri, 01 Jan 2100 00:00:00 GMT";
        let parsed = parse_retry_after_ms(date, now_ms()).unwrap();
        assert!(parsed > 0.0, "{parsed}");
        // A past date delays by at most zero.
        let parsed = parse_retry_after_ms("Sat, 01 Jan 2000 00:00:00 GMT", now_ms()).unwrap();
        assert!(parsed <= 0.0, "{parsed}");
        // Garbage -> give up.
        assert_eq!(parse_retry_after_ms("soon", 1000), None);
        // Infinity -> not finite -> give up.
        assert_eq!(parse_retry_after_ms("1e999", 1000), None);
    }
}
