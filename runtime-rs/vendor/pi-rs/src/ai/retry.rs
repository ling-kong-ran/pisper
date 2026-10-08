//! Retry ports from upstream `packages/ai/src/utils/retry.ts` and
//! `packages/ai/src/utils/provider-retry.ts`.
//!
//! Two independent mechanisms live upstream (and here):
//!
//! 1. **Provider-request retry** ([`retry_provider_request`],
//!    [`ProviderError`]) — reproduces the pinned OpenAI/Anthropic SDK retry
//!    policy for the INITIAL HTTP request: retryable statuses (408, 409, 429,
//!    and 500 or higher), the `x-should-retry` header override, transport
//!    failures (no status), `retry-after-ms`/`retry-after` delay honoring
//!    capped by `maxRetryDelayMs` (default 60000 ms, `0` disables the cap),
//!    and the SDK's exponential fallback with `1 - random() * 0.25` jitter.
//!    The pi SDKs are invoked with `maxRetries: 0` and wrapped so the backoff
//!    sleep is interruptible upstream; only the request setup is retried —
//!    once stream bytes flow, a mid-flight error surfaces as the stream's
//!    `error` event with no retry (upstream wraps only
//!    `client.*.create(...).withResponse()`).
//! 2. **Assistant-call retry** ([`retry_assistant_call`], [`RetryPolicy`],
//!    [`is_retryable_assistant_error`], [`retry_delay_ms`]) — the agent-level
//!    bounded retry over finished [`AssistantMessage`]s: exponential backoff
//!    (`baseDelayMs * 2^(attempt-1)`) capped by `maxAgentDelayMs`
//!    (default 60000), the provider error-message classifier (quota/billing
//!    errors never retry), and the callback protocol.
//!
//! Deviations from upstream, all structural:
//! - The classifier's patterns upstream are one big case-insensitive regex
//!   joined from alternatives; no regex crate is a dependency, so the exact
//!   pattern subset upstream uses (literals, `.` = any single character, `?` =
//!   optional previous character) is matched by a small interpreter
//!   ([`pattern_matches`]). Case folding is ASCII; every pattern is ASCII.
//! - The abort signal is the port's [`CancellationToken`] (upstream
//!   `AbortSignal`, `Option` because upstream signals are optional):
//!   `retry_provider_request` fails fast with the `"Request aborted"` abort
//!   error when the token is already cancelled (upstream's SDK rejects
//!   inside the attempt instead — the observable outcome is identical), and
//!   both loops' backoff sleeps race the token (`abortableSleep`). The
//!   assistant-call retry normalizes a backoff abort to the final error
//!   message with `stopReason: "aborted"` and the errorMessage stripped,
//!   exactly like upstream's `RetrySleepAbortError` handling.
//! - Jitter uses a process-random source (std `RandomState`), not
//!   `Math.random()`; the distribution shape `1 - u * 0.25` is identical and
//!   no oracle pins its magnitude.
//! - Delays are integer milliseconds (`u64`); fractional upstream delays
//!   truncate toward zero exactly like `setTimeout` input, and overflowed
//!   exponential delays saturate at `Number.MAX_SAFE_INTEGER` like upstream's
//!   `Number.isSafeInteger` guard.

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::ai::api::REQUEST_ABORTED;
use crate::ai::now_ms;
use crate::ai::types::message::AssistantMessage;
use crate::ai::types::primitives::StopReason;

// =============================================================================
// Provider-request retry (utils/provider-retry.ts)
// =============================================================================

/// Upstream `DEFAULT_MAX_RETRY_DELAY_MS` (provider-retry.ts:1).
pub const DEFAULT_MAX_RETRY_DELAY_MS: u64 = 60_000;

/// JS `Number.MAX_SAFE_INTEGER` — the upstream saturation ceiling for
/// overflowed exponential delays (retry.ts:115).
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// Upstream `ProviderError` (provider-retry.ts:9-12): an error carrying the
/// HTTP status and response headers the SDK attaches. `status: None` models a
/// transport/setup failure (upstream `undefined`), which the policy always
/// retries.
///
/// `headers` is boxed to keep the struct small (40 bytes): the header map is
/// heap-backed anyway, and an inline `HeaderMap` pushes `Result<T,
/// ProviderError>` over clippy's `result_large_err` threshold for every
/// provider call site.
#[derive(Debug, Clone)]
pub struct ProviderError {
    pub status: Option<u16>,
    pub headers: Option<Box<reqwest::header::HeaderMap>>,
    pub message: String,
}

impl ProviderError {
    /// A transport/setup failure: no HTTP status, no headers.
    pub fn transport(message: impl Into<String>) -> Self {
        ProviderError {
            status: None,
            headers: None,
            message: message.into(),
        }
    }

    /// An HTTP error response with its status and headers.
    pub fn http(
        status: u16,
        headers: reqwest::header::HeaderMap,
        message: impl Into<String>,
    ) -> Self {
        ProviderError {
            status: Some(status),
            headers: Some(Box::new(headers)),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Upstream `isRetryableProviderError` (provider-retry.ts:23-35): the pinned
/// OpenAI/Anthropic SDK retry policy. `x-should-retry: true/false` overrides;
/// a missing status (transport failure) retries; otherwise 408/409/429 and
/// everything >= 500 retry.
pub fn is_retryable_provider_error(error: &ProviderError) -> bool {
    let should_retry = error
        .headers
        .as_ref()
        .and_then(|headers| header_str(headers, "x-should-retry"));
    match should_retry {
        Some("true") => return true,
        Some("false") => return false,
        _ => {}
    }
    match error.status {
        None => true,
        Some(status) => status == 408 || status == 409 || status == 429 || status >= 500,
    }
}

/// Upstream `validateServerRetryDelayMs` (provider-retry.ts:37-49): a
/// server-requested delay above the cap fails the request (`maxRetryDelayMs`
/// defaults to 60000; `0` disables the limit). The error message reproduces
/// the upstream template, including the `Math.ceil` to seconds.
fn validate_server_retry_delay_ms(
    delay_ms: u64,
    max_retry_delay_ms: Option<u64>,
    provider_error_message: &str,
) -> Result<u64, String> {
    let max_delay_ms = max_retry_delay_ms.unwrap_or(DEFAULT_MAX_RETRY_DELAY_MS);
    if max_delay_ms > 0 && delay_ms > max_delay_ms {
        return Err(format!(
            "Server requested {}s retry delay (max: {}s). {provider_error_message}",
            delay_ms.div_ceil(1000),
            max_delay_ms.div_ceil(1000),
        ));
    }
    Ok(delay_ms)
}

/// Upstream `getRetryDelayMs` (provider-retry.ts:51-72): `retry-after-ms`,
/// then `retry-after` (numeric seconds, then HTTP-date via `Date.parse`),
/// then the SDK's exponential fallback with jitter. Header values that parse
/// to nothing — or to a non-finite delay (the 2bbfcca43 tightening) — skip
/// the branch and fall through to the exponential backoff.
fn get_retry_delay_ms(
    error: &ProviderError,
    retry_index: u32,
    max_retry_delay_ms: Option<u64>,
) -> Result<u64, String> {
    let headers = error.headers.as_ref();
    // Upstream `if (retryAfterMs)` truthiness: an empty header value skips.
    let retry_after_ms = headers
        .and_then(|headers| header_str(headers, "retry-after-ms"))
        .filter(|value| !value.is_empty());
    if let Some(value) = retry_after_ms.and_then(parse_float_header) {
        // Upstream `Number.isFinite(value)` (was `!Number.isNaN`): an
        // overflowed parse (`1e999`) skips to the exponential fallback
        // instead of validating an infinite delay.
        if value.is_finite() {
            return validate_server_retry_delay_ms(
                value.max(0.0) as u64,
                max_retry_delay_ms,
                &error.message,
            );
        }
    }
    let retry_after = headers
        .and_then(|headers| header_str(headers, "retry-after"))
        .filter(|value| !value.is_empty());
    if let Some(text) = retry_after {
        let seconds = parse_float_header(text);
        let delay_ms = match seconds {
            // Upstream `seconds * 1000` (overflowing to Infinity for huge
            // finite parses).
            Some(seconds) => Some(seconds * 1000.0),
            // Upstream `Date.parse` branch: an HTTP-date names an absolute
            // time, so the delay is the time remaining until it (negative
            // for a past date; sleep inputs clamp to zero).
            None => http_date_to_ms(text).map(|date_ms| date_ms as f64 - now_ms().max(0) as f64),
        };
        // Upstream `Number.isFinite(delayMs)`: a non-finite delay (Infinity
        // from the seconds branch, NaN from an unparseable HTTP-date) skips
        // to the exponential fallback instead of sleeping ~0.
        if let Some(delay_ms) = delay_ms.filter(|value| value.is_finite()) {
            return validate_server_retry_delay_ms(
                delay_ms.max(0.0) as u64,
                max_retry_delay_ms,
                &error.message,
            );
        }
    }
    let delay = exponential_delay_ms(retry_index, pseudo_random_fraction());
    Ok(delay)
}

/// `Number.parseFloat` prefix semantics for a delay header: skip leading
/// whitespace, then take the longest prefix that forms a decimal number —
/// `12abc` parses as 12, `1e3x` as 1000, `abc`/empty is `None` (upstream
/// `NaN`). Rust's `str::parse` demands the whole string, so scan prefixes
/// longest-first. Letters never start a parseFloat number, so `inf`/`nan`
/// spellings are rejected up front (`1e999` still overflows to infinity,
/// which the callers' `Number.isFinite` checks skip).
fn parse_float_header(text: &str) -> Option<f64> {
    let trimmed = text.trim_start();
    match trimmed.chars().next() {
        Some(first) if first.is_ascii_alphabetic() => return None,
        Some(_) => {}
        None => return None,
    }
    let mut end = trimmed.len();
    loop {
        if trimmed.is_char_boundary(end) {
            if let Ok(value) = trimmed[..end].parse::<f64>() {
                return Some(value);
            }
        }
        if end == 0 {
            return None;
        }
        end -= 1;
    }
}

/// Upstream `Date.parse` for the `retry-after` HTTP-date branch, restricted
/// to the RFC 7231 IMF-fixdate form (`Sun, 06 Nov 1994 08:49:37 GMT`) — the
/// only form servers emit in practice. Other inputs return `None` (upstream
/// `NaN`). The weekday name is accepted but not checked against the date,
/// like `Date.parse`.
fn http_date_to_ms(text: &str) -> Option<u64> {
    let mut parts = text.trim().split(' ');
    parts.next()?; // weekday (ignored)
    let day: u32 = parts.next()?.trim_end_matches(',').parse().ok()?;
    let month = month_number(parts.next()?)?;
    let year: i64 = parts.next()?.parse().ok()?;
    let mut clock = parts.next()?.split(':');
    let hour: i64 = clock.next()?.parse().ok()?;
    let minute: i64 = clock.next()?.parse().ok()?;
    let second: i64 = clock.next()?.parse().ok()?;
    if parts.next()? != "GMT" || parts.next().is_some() {
        return None;
    }
    if !(1..=31).contains(&day)
        || !(0..24).contains(&hour)
        || !(0..60).contains(&minute)
        || !(0..=60).contains(&second)
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second;
    u64::try_from(seconds).ok().map(|seconds| seconds * 1000)
}

/// Gregorian-calendar day number (Hinnant's `days_from_civil`), epoch
/// 1970-01-01. `month` is 1..=12.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let (y, m) = if month <= 2 {
        (year - 1, month as i64 + 9)
    } else {
        (year, month as i64 - 3)
    };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn month_number(name: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let lowercase = name.to_ascii_lowercase();
    MONTHS
        .iter()
        .position(|month| *month == lowercase)
        .map(|index| index as u32 + 1)
}

/// Case-insensitive single-header lookup (`HeaderMap::get` folds case; this
/// wrapper converts to `&str`).
fn header_str<'a>(headers: &'a reqwest::header::HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// Upstream exponential fallback (provider-retry.ts:65): `min(0.5 * 2^i, 8)s`
/// scaled by `1 - random() * 0.25`. Split out pure so the formula is testable
/// with an injected jitter fraction.
fn exponential_delay_ms(retry_index: u32, jitter_fraction: f64) -> u64 {
    let seconds = (0.5 * 2.0f64.powi(retry_index as i32)).min(8.0);
    let millis = seconds * 1000.0 * (1.0 - jitter_fraction * 0.25);
    (millis.max(0.0) as u64).min(MAX_SAFE_INTEGER)
}

/// A process-random fraction in `[0, 1)` standing in for `Math.random()`.
/// `RandomState` derives per-instance keys from thread-local randomness, so
/// hashing a fixed payload yields an unpredictable value.
fn pseudo_random_fraction() -> f64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(0x0007_0161);
    (hasher.finish() >> 11) as f64 / (1u64 << 53) as f64
}

/// Upstream `retryProviderRequest` (provider-retry.ts:105-125): run the
/// request-producing closure with bounded retries over
/// [`is_retryable_provider_error`] failures. Each retry is a fresh request
/// (the closure re-sends), so upstream's `X-Stainless-Retry-Count: 0`
/// invariant holds trivially. The signal (upstream
/// `options.signal`, [`REQUEST_ABORTED`] is the abort error) wins over any
/// outcome: a token already cancelled rejects with the abort error before the
/// first dial (upstream rejects inside the attempt via the SDK's signal
/// check), a failure with the token cancelled rejects with the abort error
/// instead of the attempt's error (upstream's catch-top check), and the
/// backoff sleep races the token (`abortableSleep`).
///
/// `max_retries` is upstream `options.maxRetries ?? 0` — the initial call
/// never counts as a retry, and `0` disables retrying.
pub async fn retry_provider_request<T, F, Fut>(
    max_retries: u32,
    max_retry_delay_ms: Option<u64>,
    signal: Option<&CancellationToken>,
    mut request: F,
) -> Result<T, ProviderError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, ProviderError>>,
{
    // Upstream checks `options.signal?.aborted` at the top of the catch; a
    // token already cancelled when the call starts rejects identically.
    if signal.is_some_and(CancellationToken::is_cancelled) {
        return Err(ProviderError::transport(REQUEST_ABORTED));
    }
    let mut retries_remaining = max_retries;
    loop {
        match request().await {
            Ok(value) => return Ok(value),
            Err(error) => {
                if signal.is_some_and(CancellationToken::is_cancelled) {
                    return Err(ProviderError::transport(REQUEST_ABORTED));
                }
                if retries_remaining == 0 || !is_retryable_provider_error(&error) {
                    return Err(error);
                }
                let retry_index = max_retries - retries_remaining;
                retries_remaining -= 1;
                let delay = get_retry_delay_ms(&error, retry_index, max_retry_delay_ms)
                    .map_err(ProviderError::transport)?;
                match signal {
                    Some(signal) => tokio::select! {
                        biased;
                        _ = signal.cancelled() => {
                            return Err(ProviderError::transport(REQUEST_ABORTED));
                        }
                        _ = tokio::time::sleep(Duration::from_millis(delay)) => {}
                    },
                    None => tokio::time::sleep(Duration::from_millis(delay)).await,
                }
            }
        }
    }
}

// =============================================================================
// Assistant-call retry (utils/retry.ts)
// =============================================================================

/// Upstream `DEFAULT_MAX_AGENT_RETRY_DELAY_MS` (retry.ts:111).
pub const DEFAULT_MAX_AGENT_RETRY_DELAY_MS: u64 = 60_000;

/// Upstream `NON_RETRYABLE_PROVIDER_LIMIT_ERROR_PATTERN` alternatives
/// (retry.ts:7-24): subscription/account limits are not transient throttles.
const NON_RETRYABLE_PROVIDER_LIMIT_PATTERNS: [&str; 9] = [
    "GoUsageLimitError",
    "FreeUsageLimitError",
    "Monthly usage limit reached",
    "available balance",
    "insufficient_quota",
    "out of budget",
    "quota exceeded",
    "billing",
    // Sign in with ChatGPT: the subscription's shared usage limit, which
    // resets after hours rather than seconds.
    "subscription_sharing_usage_limit_exceeded",
];

/// Upstream `RETRYABLE_PROVIDER_ERROR_PATTERN` alternatives (retry.ts:26-93):
/// generic provider load, HTTP-status, transport, and stream-truncation
/// failures, plus explicit retry guidance. Each entry uses only literals,
/// `.` (any character), and `?` (optional previous character).
const RETRYABLE_PROVIDER_ERROR_PATTERNS: [&str; 46] = [
    "overloaded",
    "currently experiencing high demand",
    // Generic provider capacity error (v1.0.0).
    "model is at capacity",
    "rate.?limit",
    "too many requests",
    "429",
    "500",
    "502",
    "503",
    "504",
    "520",
    "524",
    "service.?unavailable",
    "server.?error",
    "internal.?error",
    "provider.?returned.?error",
    "exceeded request buffer limit while retrying upstream",
    "network.?error",
    "connection.?error",
    "connection.?refused",
    "connection.?lost",
    "other side closed",
    "fetch failed",
    "getaddrinfo",
    "ENOTFOUND",
    "EAI_AGAIN",
    "upstream.?connect",
    "reset before headers",
    "socket hang up",
    "socket connection was closed",
    "timed? out",
    "timeout",
    "terminated",
    "websocket.?closed",
    "websocket.?error",
    "ended without",
    "stream ended before message_stop",
    "stream ended before a terminal response event",
    "http2 request did not get a response",
    "retry delay",
    "you can retry your request",
    "try your request again",
    "please retry your request",
    "ResourceExhausted",
    // Sign in with ChatGPT: usage or user data temporarily unavailable. Usage
    // failures can arrive mid-stream without an HTTP 503 in the message.
    "subscription_sharing_usage_unavailable",
    "subscription_sharing_user_unavailable",
];

/// Upstream `RetryPolicy` (retry.ts:101-109): bounded attempts with
/// exponential backoff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPolicy {
    pub enabled: bool,
    /// Max retry attempts (0 = no retries). The initial call never counts.
    pub max_retries: u32,
    /// Base delay in ms; per-attempt delay is `baseDelayMs * 2^(attempt-1)`.
    pub base_delay_ms: u64,
    /// Optional cap for agent-level retry delays in ms (default 60000).
    pub max_agent_delay_ms: Option<u64>,
}

/// Upstream `retryDelayMs` (retry.ts:113-117): `baseDelayMs * 2^(attempt-1)`
/// saturated at `Number.MAX_SAFE_INTEGER`, capped by `maxAgentDelayMs`
/// (default 60000; a 0 cap zeroes the delay).
pub fn retry_delay_ms(policy: &RetryPolicy, attempt: u32) -> u64 {
    let exponent = attempt.saturating_sub(1).min(63);
    let factor = 1u64 << exponent;
    let delay = policy
        .base_delay_ms
        .checked_mul(factor)
        .unwrap_or(MAX_SAFE_INTEGER)
        .min(MAX_SAFE_INTEGER);
    delay.min(
        policy
            .max_agent_delay_ms
            .unwrap_or(DEFAULT_MAX_AGENT_RETRY_DELAY_MS),
    )
}

/// `onRetryScheduled` hook: `(attempt, maxAttempts, delayMs, errorMessage)`.
pub type OnRetryScheduled = Box<dyn FnMut(u32, u32, u64, &str) + Send>;
/// `onRetryAttemptStart` hook.
pub type OnRetryAttemptStart = Box<dyn FnMut() + Send>;
/// `onRetryFinished` hook: `(success, attempt, finalError?)`.
pub type OnRetryFinished = Box<dyn FnMut(bool, u32, Option<&str>) + Send>;

/// Upstream `RetryCallbacks` (retry.ts:120-132): hooks emitted around each
/// retry. All fields optional; defaults to no-op.
#[derive(Default)]
pub struct RetryCallbacks {
    /// Before the backoff sleep of each retry attempt (1-indexed).
    pub on_retry_scheduled: Option<OnRetryScheduled>,
    /// After the sleep, immediately before the retried call starts.
    pub on_retry_attempt_start: Option<OnRetryAttemptStart>,
    /// Once when the loop ends: success if a later call completed normally.
    pub on_retry_finished: Option<OnRetryFinished>,
}

/// Upstream `isRetryableAssistantError` (retry.ts:237-242): whether a failed
/// assistant message looks like a transient provider/transport error. Quota/
/// billing exhaustion (the non-retryable list) wins over the retryable list.
pub fn is_retryable_assistant_error(message: &AssistantMessage) -> bool {
    if message.stop_reason != StopReason::Error {
        return false;
    }
    let Some(error_message) = &message.error_message else {
        return false;
    };
    if NON_RETRYABLE_PROVIDER_LIMIT_PATTERNS
        .into_iter()
        .any(|pattern| pattern_matches(pattern, error_message))
    {
        return false;
    }
    RETRYABLE_PROVIDER_ERROR_PATTERNS
        .into_iter()
        .any(|pattern| pattern_matches(pattern, error_message))
}

/// Case-insensitive existence matcher for the upstream pattern subset:
/// literal characters, `.` (any single character), and a trailing `?` making
/// the previous character optional (upstream uses `.?` gaps and one `d?`).
/// Shared with the openai-codex-responses endpoint, whose transport retry
/// classifiers are the same regex family (openai-codex-responses.ts:123-136).
pub fn pattern_matches(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    (0..=text.len()).any(|start| match_here(&pattern, 0, &text, start))
}

/// Upstream `isTerminalRateLimitError` (openai-codex-responses.ts:123-127):
/// subscription/billing exhaustion is not a transient throttle. The
/// alternative set is identical to [`NON_RETRYABLE_PROVIDER_LIMIT_PATTERNS`]
/// (retry.ts:7-24), so the same matcher drives both.
pub fn is_non_retryable_provider_limit_error(text: &str) -> bool {
    NON_RETRYABLE_PROVIDER_LIMIT_PATTERNS
        .into_iter()
        .any(|pattern| pattern_matches(pattern, text))
}

fn match_here(pattern: &[char], pattern_index: usize, text: &[char], text_index: usize) -> bool {
    if pattern_index >= pattern.len() {
        return true;
    }
    let current = pattern[pattern_index];
    let optional = pattern.get(pattern_index + 1) == Some(&'?');
    if optional {
        let next = pattern_index + 2;
        if text_index < text.len()
            && char_matches(current, text[text_index])
            && match_here(pattern, next, text, text_index + 1)
        {
            return true;
        }
        return match_here(pattern, next, text, text_index);
    }
    if text_index < text.len() && char_matches(current, text[text_index]) {
        return match_here(pattern, pattern_index + 1, text, text_index + 1);
    }
    false
}

fn char_matches(pattern_char: char, text_char: char) -> bool {
    pattern_char == '.' || pattern_char.eq_ignore_ascii_case(&text_char)
}

/// Upstream `retryAssistantCall` (retry.ts:176-226): run one
/// assistant-producing call with bounded retry on transient errors.
///
/// - A non-aborted, non-error response returns immediately (success).
/// - Aborts are terminal and never retried.
/// - Non-retryable errors (including quota/billing) fail fast.
/// - Otherwise retries up to `policy.max_retries` with [`retry_delay_ms`]
///   backoff, emitting the callbacks in upstream order.
///
/// `policy: None` or `enabled: false` returns the first response unchanged.
/// The signal (upstream `signal?: AbortSignal`) aborts the backoff sleep:
/// the final error message normalizes to `stopReason: "aborted"` with the
/// errorMessage stripped (upstream's `RetrySleepAbortError` arm), so callers
/// see the same message shape as a provider stream abort.
pub async fn retry_assistant_call<P, Fut>(
    mut produce: P,
    policy: Option<&RetryPolicy>,
    signal: Option<&CancellationToken>,
    callbacks: &mut RetryCallbacks,
) -> AssistantMessage
where
    P: FnMut() -> Fut,
    Fut: std::future::Future<Output = AssistantMessage>,
{
    // The active policy is `Some` exactly when `max_attempts > 0`, so the
    // delay computation below never observes the `None` fallback (the delay
    // line is unreachable when `attempt >= max_attempts`).
    let active_policy = policy.filter(|policy| policy.enabled);
    let max_attempts = active_policy.map(|policy| policy.max_retries).unwrap_or(0);

    let mut attempt: u32 = 0;
    let mut last_retry: Option<(u32, String)> = None;
    loop {
        let response = produce().await;

        // Abort: terminal but not successful. Never retry an aborted message.
        if response.stop_reason == StopReason::Aborted {
            if let Some((last_attempt, _)) = &last_retry {
                if let Some(callback) = callbacks.on_retry_finished.as_mut() {
                    callback(false, *last_attempt, None);
                }
            }
            return response;
        }

        // Success: non-error, non-abort responses return as-is.
        if response.stop_reason != StopReason::Error {
            if let Some((last_attempt, _)) = &last_retry {
                if let Some(callback) = callbacks.on_retry_finished.as_mut() {
                    callback(true, *last_attempt, None);
                }
            }
            return response;
        }

        // Non-retryable, or budget exhausted: return the final error message.
        if attempt >= max_attempts || !is_retryable_assistant_error(&response) {
            if let Some((last_attempt, _)) = &last_retry {
                if let Some(callback) = callbacks.on_retry_finished.as_mut() {
                    callback(false, *last_attempt, response.error_message.as_deref());
                }
            }
            return response;
        }

        attempt += 1;
        let error_message = response
            .error_message
            .clone()
            .unwrap_or_else(|| "Unknown error".to_string());
        last_retry = Some((attempt, error_message.clone()));
        let delay_ms = active_policy
            .map(|policy| retry_delay_ms(policy, attempt))
            .unwrap_or(0);
        if let Some(callback) = callbacks.on_retry_scheduled.as_mut() {
            callback(attempt, max_attempts, delay_ms, &error_message);
        }
        // Upstream normalize: a backoff abort settles the pending retry as an
        // aborted message carrying no errorMessage, reported through
        // onRetryFinished(false, attempt, lastRetry.errorMessage).
        let sleep = tokio::time::sleep(Duration::from_millis(delay_ms));
        let aborted = match signal {
            Some(signal) => {
                let mut aborted = false;
                tokio::select! {
                    biased;
                    _ = signal.cancelled() => aborted = true,
                    _ = sleep => {}
                }
                aborted
            }
            None => {
                sleep.await;
                false
            }
        };
        if aborted {
            if let Some(callback) = callbacks.on_retry_finished.as_mut() {
                callback(false, attempt, Some(error_message.as_str()));
            }
            let mut aborted_message = response;
            aborted_message.stop_reason = StopReason::Aborted;
            aborted_message.error_message = None;
            return aborted_message;
        }
        if let Some(callback) = callbacks.on_retry_attempt_start.as_mut() {
            callback();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::now_ms;
    use crate::ai::types::primitives::Usage;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    // ---- fixtures ----

    /// Finished-callback log entry: `(success, attempt, finalError?)`.
    type FinishedLog = std::sync::Arc<std::sync::Mutex<Vec<(bool, u32, Option<String>)>>>;

    fn message(stop_reason: StopReason, error: Option<&str>) -> AssistantMessage {
        AssistantMessage {
            content: Vec::new(),
            api: "faux".to_string(),
            provider: "faux".to_string(),
            model: "faux".to_string(),
            response_model: None,
            response_id: None,
            provider_thinking_level: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason,
            deferred: None,
            error_message: error.map(str::to_string),
            raw_stop_reason: None,
            end_turn: None,
            timestamp: now_ms(),
        }
    }

    fn error_msg(text: &str) -> AssistantMessage {
        message(StopReason::Error, Some(text))
    }

    fn provider_error(status: u16, headers: &[(&str, &str)]) -> ProviderError {
        let mut map = reqwest::header::HeaderMap::new();
        for (name, value) in headers {
            map.insert(
                reqwest::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                reqwest::header::HeaderValue::from_str(value).unwrap(),
            );
        }
        ProviderError::http(status, map, format!("Provider error: {status}"))
    }

    /// Drives `retry_provider_request` over a scripted FIFO outcome list;
    /// returns (result, attempt count, elapsed millis).
    async fn drive(
        max_retries: u32,
        max_retry_delay_ms: Option<u64>,
        outcomes: Vec<Result<&'static str, ProviderError>>,
    ) -> (Result<&'static str, String>, u32, u128) {
        let attempts = Arc::new(AtomicU32::new(0));
        let queue = Arc::new(Mutex::new(outcomes));
        let attempts_closure = attempts.clone();
        let start = std::time::Instant::now();
        let result = retry_provider_request(max_retries, max_retry_delay_ms, None, move || {
            let attempts = attempts_closure.clone();
            let queue = queue.clone();
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                queue.lock().unwrap().remove(0)
            }
        })
        .await
        .map_err(|error| error.message);
        (
            result,
            attempts.load(Ordering::SeqCst),
            start.elapsed().as_millis(),
        )
    }

    /// Drives `retry_assistant_call` over a scripted message list; returns
    /// (final message, produce-call count).
    async fn call(
        policy: Option<RetryPolicy>,
        outcomes: Vec<AssistantMessage>,
        callbacks: &mut RetryCallbacks,
    ) -> (AssistantMessage, u32) {
        let attempts = Arc::new(AtomicU32::new(0));
        let queue = Arc::new(Mutex::new(outcomes));
        let attempts_closure = attempts.clone();
        let final_message = retry_assistant_call(
            move || {
                let attempts = attempts_closure.clone();
                let queue = queue.clone();
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    let outcome = { queue.lock().unwrap().first().cloned() };
                    match outcome {
                        Some(outcome) => {
                            queue.lock().unwrap().remove(0);
                            outcome
                        }
                        // Repeat the last outcome when the script runs dry so
                        // "always fails" scripts need no padding.
                        None => queue
                            .lock()
                            .unwrap()
                            .last()
                            .cloned()
                            .expect("non-empty script"),
                    }
                }
            },
            policy.as_ref(),
            None,
            callbacks,
        )
        .await;
        (final_message, attempts.load(Ordering::SeqCst))
    }

    fn scheduled_callback(counter: Arc<AtomicU32>) -> RetryCallbacks {
        RetryCallbacks {
            on_retry_scheduled: Some(Box::new(move |_, _, _, _| {
                counter.fetch_add(1, Ordering::SeqCst);
            })),
            ..Default::default()
        }
    }

    // ---- provider-retry.test.ts ports ----

    #[tokio::test]
    async fn retries_retryable_provider_errors() {
        let (result, attempts, elapsed) = drive(
            1,
            None,
            vec![
                Err(provider_error(429, &[("retry-after-ms", "20")])),
                Ok("ok"),
            ],
        )
        .await;
        assert_eq!(result, Ok("ok"));
        assert_eq!(attempts, 2);
        // The provider-requested delay is honored before the second attempt.
        assert!(elapsed >= 15, "slept {elapsed}ms");
    }

    #[tokio::test]
    async fn does_not_retry_errors_the_provider_marks_as_non_retryable() {
        let error = provider_error(429, &[("x-should-retry", "false")]);
        let (result, attempts, _) = drive(2, None, vec![Err(error.clone())]).await;
        assert_eq!(result.unwrap_err(), error.message);
        assert_eq!(attempts, 1);
    }

    #[tokio::test]
    async fn x_should_retry_true_forces_retry_of_a_non_retryable_status() {
        let (result, attempts, _) = drive(
            1,
            None,
            vec![
                Err(provider_error(400, &[("x-should-retry", "true")])),
                Ok("ok"),
            ],
        )
        .await;
        assert_eq!(result, Ok("ok"));
        assert_eq!(attempts, 2);
    }

    #[tokio::test]
    async fn transport_failures_without_status_are_retryable() {
        let (result, attempts, _) = drive(
            2,
            None,
            vec![
                Err(ProviderError::transport("connection reset")),
                Err(ProviderError::transport("connection reset")),
                Ok("ok"),
            ],
        )
        .await;
        assert_eq!(result, Ok("ok"));
        assert_eq!(attempts, 3);
    }

    #[tokio::test]
    async fn rejects_a_provider_requested_retry_delay_above_the_limit() {
        let (result, attempts, _) = drive(
            1,
            Some(1000),
            vec![Err(provider_error(429, &[("retry-after", "277403")]))],
        )
        .await;
        let message = result.unwrap_err();
        assert!(
            message.starts_with("Server requested 277403s retry delay (max: 1s)."),
            "got: {message}"
        );
        assert_eq!(attempts, 1);
    }

    #[tokio::test]
    async fn default_cap_rejects_delays_above_60_seconds() {
        let (result, attempts, _) = drive(
            1,
            None,
            vec![Err(provider_error(429, &[("retry-after-ms", "61000")]))],
        )
        .await;
        assert!(result
            .unwrap_err()
            .starts_with("Server requested 61s retry delay (max: 60s)."),);
        assert_eq!(attempts, 1);
    }

    #[tokio::test]
    async fn allows_disabling_the_provider_requested_retry_delay_cap() {
        let (result, attempts, _) = drive(
            1,
            Some(0),
            vec![
                Err(provider_error(429, &[("retry-after-ms", "15")])),
                Ok("ok"),
            ],
        )
        .await;
        assert_eq!(result, Ok("ok"));
        assert_eq!(attempts, 2);
    }

    #[tokio::test]
    async fn retries_the_exact_retryable_status_set() {
        // 408, 409, 429 and everything >= 500 retry; other statuses fail fast.
        for (status, expected_attempts) in [
            (408u16, 2u32),
            (409, 2),
            (429, 2),
            (500, 2),
            (502, 2),
            (503, 2),
            (504, 2),
            (529, 2),
            (400, 1),
            (401, 1),
            (403, 1),
            (404, 1),
            (499, 1),
        ] {
            let (_, attempts, _) = drive(
                1,
                None,
                vec![
                    Err(provider_error(status, &[("retry-after-ms", "1")])),
                    Ok("ok"),
                ],
            )
            .await;
            assert_eq!(attempts, expected_attempts, "status {status}");
        }
    }

    #[tokio::test]
    async fn exhausts_retries_and_returns_the_last_error() {
        let error = provider_error(500, &[("retry-after-ms", "1")]);
        let (result, attempts, _) = drive(
            2,
            None,
            vec![Err(error.clone()), Err(error.clone()), Err(error)],
        )
        .await;
        assert!(result.is_err());
        assert_eq!(attempts, 3);
    }

    // ---- retry-after prefix and HTTP-date forms (M2f hardening) ----

    /// Upstream parses delay headers with `Number.parseFloat`, which consumes
    /// the longest numeric PREFIX (`12abc` -> 12) instead of rejecting the
    /// whole value like Rust's `str::parse`.
    #[test]
    fn delay_headers_parse_the_numeric_prefix_like_parse_float() {
        assert_eq!(parse_float_header("20"), Some(20.0));
        assert_eq!(parse_float_header("12abc"), Some(12.0));
        assert_eq!(parse_float_header("1e3x"), Some(1000.0));
        assert_eq!(parse_float_header(".5s"), Some(0.5));
        // JS parseFloat("0x10") stops at the 0 prefix.
        assert_eq!(parse_float_header("0x10"), Some(0.0));
        assert_eq!(parse_float_header("-3"), Some(-3.0));
        assert_eq!(parse_float_header("1e999"), Some(f64::INFINITY));
        assert_eq!(parse_float_header("abc"), None);
        assert_eq!(parse_float_header(""), None);
    }

    /// The RFC 7231 IMF-fixdate example: Date.parse gives 784111777000.
    #[test]
    fn http_date_parses_imf_fixdate() {
        assert_eq!(
            http_date_to_ms("Sun, 06 Nov 1994 08:49:37 GMT"),
            Some(784111777000)
        );
        assert_eq!(
            http_date_to_ms("Sat, 01 Jan 2000 00:00:00 GMT"),
            Some(946684800000)
        );
        // Weekday/date mismatches and other shapes: Date.parse-side rejection.
        assert_eq!(http_date_to_ms("not a date"), None);
        assert_eq!(http_date_to_ms("Sun, 32 Nov 1994 08:49:37 GMT"), None);
        assert_eq!(http_date_to_ms("Sun, 06 Nov 1994 08:49:37 UTC"), None);
    }

    /// A future HTTP-date names an absolute time: the delay is the remaining
    /// time until it, so a fixed far-future date always trips the cap. The
    /// message's second count is relative (Date.now-dependent), so only the
    /// prefix is pinned.
    #[tokio::test]
    async fn retry_after_http_date_in_the_future_is_honored() {
        let (result, attempts, _) = drive(
            1,
            Some(1000),
            vec![Err(provider_error(
                429,
                &[("retry-after", "Tue, 19 Jan 2038 03:14:08 GMT")],
            ))],
        )
        .await;
        let message = result.unwrap_err();
        assert!(
            message.starts_with("Server requested ") && message.contains("(max: 1s)."),
            "got: {message}"
        );
        assert_eq!(attempts, 1);
    }

    /// A past HTTP-date (and an unparseable value — upstream's NaN delay
    /// reaches `setTimeout`, which fires immediately) retries without the
    /// exponential fallback's >=375ms wait.
    #[tokio::test]
    async fn retry_after_past_date_sleeps_immediately_and_garbage_uses_exponential_backoff() {
        // Past date: the delay is negative but FINITE, so upstream still
        // returns it and the sleep clamps to zero — immediate retry.
        let (result, attempts, elapsed) = drive(
            1,
            None,
            vec![
                Err(provider_error(
                    429,
                    &[("retry-after", "Sat, 01 Jan 2000 00:00:00 GMT")],
                )),
                Ok("ok"),
            ],
        )
        .await;
        assert_eq!(result, Ok("ok"));
        assert_eq!(attempts, 2);
        assert!(elapsed < 300, "slept {elapsed}ms");

        // Garbage: NaN fails the upstream Number.isFinite gate (2bbfcca43)
        // and falls through to the exponential backoff (index 0: 0.5s *
        // (1 - random()*0.25) → 375–500ms with the jitter bounds).
        let (result, attempts, elapsed) = drive(
            1,
            None,
            vec![
                Err(provider_error(429, &[("retry-after", "garbage")])),
                Ok("ok"),
            ],
        )
        .await;
        assert_eq!(result, Ok("ok"));
        assert_eq!(attempts, 2);
        assert!(
            (350..550).contains(&elapsed),
            "slept {elapsed}ms (expected the 375-500ms exponential band)"
        );
    }

    // ---- provider-retry.test.ts ports: abort half ----

    /// Oracle: "aborts a provider-requested retry delay" — aborting during
    /// the backoff sleep rejects with the abort error without a second
    /// attempt. `maxRetryDelayMs: 0` disables the cap so the 277403s
    /// `retry-after` becomes one long sleep; a watcher cancels mid-sleep.
    #[tokio::test]
    async fn aborts_a_provider_requested_retry_delay() {
        let token = CancellationToken::new();
        let watcher_token = token.clone();
        let attempts = Arc::new(AtomicU32::new(0));
        let watcher_attempts = attempts.clone();
        let attempts_closure = attempts.clone();
        tokio::spawn(async move {
            // Wait until the first attempt failed and the backoff sleep
            // started, then abort it.
            while watcher_attempts.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            watcher_token.cancel();
        });
        let result = retry_provider_request(2, Some(0), Some(&token), move || {
            let attempts = attempts_closure.clone();
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err::<&'static str, _>(provider_error(429, &[("retry-after", "277403")]))
            }
        })
        .await;
        assert_eq!(result.unwrap_err().message, REQUEST_ABORTED);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    /// A token cancelled before (or between) attempts turns any failure into
    /// the abort error — upstream's catch-top
    /// `if (options.signal?.aborted) throw createAbortError()` — bypassing
    /// both the retryable classification and the remaining budget. Port
    /// deviation: the pre-cancelled check fires BEFORE the first dial (the
    /// upstream SDK rejects inside the attempt, so its request counter shows
    /// one call), so the port's attempt count stays 0 — the observable
    /// outcome (abort error, no retry) is identical.
    #[tokio::test]
    async fn pre_aborted_provider_request_fails_fast_without_retrying() {
        let token = CancellationToken::new();
        token.cancel();
        let (result, attempts, _) = drive_with_signal(
            Some(&token),
            5,
            None,
            vec![
                Err(provider_error(429, &[("retry-after-ms", "1")])),
                Ok("ok"),
            ],
        )
        .await;
        assert_eq!(result.unwrap_err(), REQUEST_ABORTED);
        assert_eq!(attempts, 0);
    }

    /// `drive` with an explicit signal.
    async fn drive_with_signal(
        signal: Option<&CancellationToken>,
        max_retries: u32,
        max_retry_delay_ms: Option<u64>,
        outcomes: Vec<Result<&'static str, ProviderError>>,
    ) -> (Result<&'static str, String>, u32, u128) {
        let attempts = Arc::new(AtomicU32::new(0));
        let queue = Arc::new(Mutex::new(outcomes));
        let attempts_closure = attempts.clone();
        let start = std::time::Instant::now();
        let result = retry_provider_request(max_retries, max_retry_delay_ms, signal, move || {
            let attempts = attempts_closure.clone();
            let queue = queue.clone();
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                queue.lock().unwrap().remove(0)
            }
        })
        .await
        .map_err(|error| error.message);
        (
            result,
            attempts.load(Ordering::SeqCst),
            start.elapsed().as_millis(),
        )
    }

    // ---- exponential fallback formula ----

    #[test]
    fn exponential_fallback_doubles_and_caps_at_eight_seconds() {
        // min(0.5 * 2^i, 8) * 1000, jitter fraction 0 => no reduction.
        assert_eq!(exponential_delay_ms(0, 0.0), 500);
        assert_eq!(exponential_delay_ms(1, 0.0), 1000);
        assert_eq!(exponential_delay_ms(2, 0.0), 2000);
        assert_eq!(exponential_delay_ms(3, 0.0), 4000);
        assert_eq!(exponential_delay_ms(4, 0.0), 8000);
        assert_eq!(exponential_delay_ms(9, 0.0), 8000);
        // Full jitter reduces by 25%.
        assert_eq!(exponential_delay_ms(0, 1.0), 375);
    }

    #[test]
    fn pseudo_random_fraction_is_in_the_unit_interval() {
        for _ in 0..32 {
            let fraction = pseudo_random_fraction();
            assert!((0.0..1.0).contains(&fraction), "fraction {fraction}");
        }
    }

    // ---- retry.test.ts ports: classification ----

    const OPENAI_EXPLICIT_RETRY_MESSAGE: &str = "An error occurred while processing your request. You can retry your request, or contact us through our help center at help.openai.com if the error persists. Please include the request ID req_******** in your message.";
    const BEDROCK_EXPLICIT_RETRY_MESSAGE: &str = "{\"message\":\"The system encountered an unexpected error during processing. Try your request again.\"}";
    const NVIDIA_NIM_RESOURCE_EXHAUSTED_MESSAGE: &str =
        "ResourceExhausted: Worker local total request limit reached (288/48)";
    const BUN_FETCH_SOCKET_CLOSED_MESSAGE: &str = "The socket connection was closed unexpectedly. For more information, pass `verbose: true` in the second argument to fetch()";
    const OPENAI_RESPONSES_EARLY_EOF_MESSAGE: &str =
        "OpenAI Responses stream ended before a terminal response event";
    const WRAPPED_DNS_LOOKUP_ERROR: &str = "The pending stream has been canceled (caused by: getaddrinfo ENOTFOUND bedrock-runtime.us-east-1.amazonaws.com)";
    const AZURE_PEAK_LOAD_ERROR: &str = "The system is currently experiencing high demand and cannot process your request. Your request exceeds the maximum usage size allowed during peak load. For improved capacity reliability, consider switching to Provisioned Throughput.";

    #[test]
    fn matches_explicit_provider_retry_guidance() {
        assert!(is_retryable_assistant_error(&error_msg(
            OPENAI_EXPLICIT_RETRY_MESSAGE
        )));
        assert!(is_retryable_assistant_error(&error_msg(
            BEDROCK_EXPLICIT_RETRY_MESSAGE
        )));
        assert!(is_retryable_assistant_error(&error_msg(
            NVIDIA_NIM_RESOURCE_EXHAUSTED_MESSAGE
        )));
    }

    #[test]
    fn matches_bun_fetch_socket_drop_wording() {
        assert!(is_retryable_assistant_error(&error_msg(
            BUN_FETCH_SOCKET_CLOSED_MESSAGE
        )));
    }

    #[test]
    fn matches_upstream_request_buffer_exhaustion_wording() {
        assert!(is_retryable_assistant_error(&error_msg(
            "Error: exceeded request buffer limit while retrying upstream"
        )));
    }

    #[test]
    fn matches_dns_transport_failure_wording() {
        for text in [
            WRAPPED_DNS_LOOKUP_ERROR,
            "connect ENOTFOUND api.example.com",
            "EAI_AGAIN api.example.com",
            "getaddrinfo failed for api.example.com",
        ] {
            assert!(is_retryable_assistant_error(&error_msg(text)), "{text}");
        }
    }

    #[test]
    fn matches_openai_responses_streams_that_end_before_terminal_events() {
        assert!(is_retryable_assistant_error(&error_msg(
            OPENAI_RESPONSES_EARLY_EOF_MESSAGE
        )));
    }

    #[test]
    fn matches_azure_peak_load_capacity_errors() {
        assert!(is_retryable_assistant_error(&error_msg(
            AZURE_PEAK_LOAD_ERROR
        )));
    }

    #[test]
    fn keeps_provider_limit_errors_non_retryable() {
        assert!(!is_retryable_assistant_error(&error_msg(
            "429 quota exceeded"
        )));
        assert!(!is_retryable_assistant_error(&error_msg(
            "insufficient_quota: billing cycle exhausted"
        )));
    }

    #[test]
    fn classifies_assistant_error_messages() {
        assert!(is_retryable_assistant_error(&error_msg("overloaded_error")));
        assert!(is_retryable_assistant_error(&error_msg(
            "520 status code (no body)"
        )));
        assert!(is_retryable_assistant_error(&error_msg(
            "524 status code (no body)"
        )));
        assert!(!is_retryable_assistant_error(&message(
            StopReason::Stop,
            None
        )));
    }

    #[test]
    fn pattern_subset_handles_optional_character_gaps() {
        assert!(pattern_matches("rate.?limit", "rate-limit"));
        assert!(pattern_matches("rate.?limit", "ratelimit"));
        assert!(pattern_matches("rate.?limit", "rate limit hit"));
        assert!(!pattern_matches("rate.?limit", "rate  limit"));
        // Upstream `timed? out`: optional literal `d`.
        assert!(pattern_matches("timed? out", "request timed out"));
        assert!(pattern_matches("timed? out", "request time out"));
        assert!(pattern_matches("TIMED? OUT", "timed out"));
        assert!(!pattern_matches("timed? out", "timeed out"));
    }

    // ---- retry.test.ts ports: retryDelayMs ----

    fn base_policy(base_delay_ms: u64, max_agent_delay_ms: Option<u64>) -> RetryPolicy {
        RetryPolicy {
            enabled: true,
            max_retries: 3,
            base_delay_ms,
            max_agent_delay_ms,
        }
    }

    #[test]
    fn caps_agent_retry_delay() {
        assert_eq!(retry_delay_ms(&base_policy(2000, None), 6), 60000);
        assert_eq!(retry_delay_ms(&base_policy(2000, Some(5000)), 5), 5000);
        assert_eq!(retry_delay_ms(&base_policy(2000, Some(0)), 5), 0);
    }

    // ---- retry.test.ts ports: retryAssistantCall ----

    fn policy(enabled: bool) -> RetryPolicy {
        RetryPolicy {
            enabled,
            max_retries: 3,
            base_delay_ms: 0,
            max_agent_delay_ms: None,
        }
    }

    #[tokio::test]
    async fn returns_a_successful_response_immediately_without_retrying() {
        let (result, attempts) = call(
            Some(policy(true)),
            vec![message(StopReason::Stop, None)],
            &mut RetryCallbacks::default(),
        )
        .await;
        assert_eq!(result.stop_reason, StopReason::Stop);
        assert_eq!(attempts, 1);
    }

    #[tokio::test]
    async fn does_not_retry_an_aborted_message() {
        let scheduled = Arc::new(AtomicU32::new(0));
        let (result, attempts) = call(
            Some(policy(true)),
            vec![message(StopReason::Aborted, None)],
            &mut scheduled_callback(scheduled.clone()),
        )
        .await;
        assert_eq!(result.stop_reason, StopReason::Aborted);
        assert_eq!(attempts, 1);
        assert_eq!(scheduled.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn does_not_retry_a_non_retryable_error() {
        let scheduled = Arc::new(AtomicU32::new(0));
        let scheduled_closure = scheduled.clone();
        let finished: FinishedLog = Arc::new(Mutex::new(Vec::new()));
        let finished_callback = finished.clone();
        let (result, attempts) = call(
            Some(policy(true)),
            vec![error_msg("insufficient_quota")],
            &mut RetryCallbacks {
                on_retry_scheduled: Some(Box::new(move |_, _, _, _| {
                    scheduled_closure.fetch_add(1, Ordering::SeqCst);
                })),
                on_retry_finished: Some(Box::new(move |success, attempt, error| {
                    finished_callback.lock().unwrap().push((
                        success,
                        attempt,
                        error.map(str::to_string),
                    ));
                })),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(result.stop_reason, StopReason::Error);
        assert_eq!(attempts, 1);
        assert_eq!(scheduled.load(Ordering::SeqCst), 0);
        assert!(finished.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn retries_a_transient_error_up_to_max_retries_then_returns_the_final_error() {
        let scheduled: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
        let finished: FinishedLog = Arc::new(Mutex::new(Vec::new()));
        let scheduled_callback_list = scheduled.clone();
        let finished_callback = finished.clone();
        let (result, attempts) = call(
            Some(policy(true)),
            vec![error_msg("terminated"); 4],
            &mut RetryCallbacks {
                on_retry_scheduled: Some(Box::new(move |attempt, max_attempts, _, _| {
                    scheduled_callback_list.lock().unwrap().push(attempt);
                    assert_eq!(max_attempts, 3);
                })),
                on_retry_finished: Some(Box::new(move |success, attempt, error| {
                    finished_callback.lock().unwrap().push((
                        success,
                        attempt,
                        error.map(str::to_string),
                    ));
                })),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(result.stop_reason, StopReason::Error);
        assert_eq!(attempts, 4); // 1 initial + 3 retries
        assert_eq!(*scheduled.lock().unwrap(), [1, 2, 3]);
        assert_eq!(
            *finished.lock().unwrap(),
            [(false, 3, Some("terminated".to_string()))]
        );
    }

    #[tokio::test]
    async fn reports_capped_retry_delays() {
        let delays: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
        let delays_callback = delays.clone();
        let capped = RetryPolicy {
            enabled: true,
            max_retries: 4,
            base_delay_ms: 10,
            max_agent_delay_ms: Some(15),
        };
        let (_, attempts) = call(
            Some(capped),
            vec![
                error_msg("terminated"),
                error_msg("terminated"),
                error_msg("terminated"),
                error_msg("terminated"),
                message(StopReason::Stop, None),
            ],
            &mut RetryCallbacks {
                on_retry_scheduled: Some(Box::new(move |_, _, delay, _| {
                    delays_callback.lock().unwrap().push(delay);
                })),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(attempts, 5);
        assert_eq!(*delays.lock().unwrap(), [10, 15, 15, 15]);
    }

    #[tokio::test]
    async fn stops_retrying_once_a_call_succeeds() {
        let finished: Arc<Mutex<Vec<(bool, u32)>>> = Arc::new(Mutex::new(Vec::new()));
        let finished_callback = finished.clone();
        let (result, attempts) = call(
            Some(policy(true)),
            vec![
                error_msg("terminated"),
                error_msg("terminated"),
                message(StopReason::Stop, None),
            ],
            &mut RetryCallbacks {
                on_retry_finished: Some(Box::new(move |success, attempt, _| {
                    finished_callback.lock().unwrap().push((success, attempt));
                })),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(result.stop_reason, StopReason::Stop);
        assert_eq!(attempts, 3);
        assert_eq!(*finished.lock().unwrap(), [(true, 2)]);
    }

    #[tokio::test]
    async fn reports_an_aborted_retried_call_as_unsuccessful() {
        let finished: Arc<Mutex<Vec<(bool, u32)>>> = Arc::new(Mutex::new(Vec::new()));
        let finished_callback = finished.clone();
        let (result, attempts) = call(
            Some(policy(true)),
            vec![error_msg("terminated"), message(StopReason::Aborted, None)],
            &mut RetryCallbacks {
                on_retry_finished: Some(Box::new(move |success, attempt, _| {
                    finished_callback.lock().unwrap().push((success, attempt));
                })),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(result.stop_reason, StopReason::Aborted);
        assert_eq!(attempts, 2);
        assert_eq!(*finished.lock().unwrap(), [(false, 1)]);
    }

    #[tokio::test]
    async fn does_not_retry_when_policy_is_disabled() {
        let scheduled = Arc::new(AtomicU32::new(0));
        let (result, attempts) = call(
            Some(policy(false)),
            vec![error_msg("terminated")],
            &mut scheduled_callback(scheduled.clone()),
        )
        .await;
        assert_eq!(result.stop_reason, StopReason::Error);
        assert_eq!(attempts, 1);
        assert_eq!(scheduled.load(Ordering::SeqCst), 0);
    }

    /// Oracle (retry.test.ts): "aborts backoff sleep via signal, returns an
    /// aborted message, and emits onRetryFinished(false)" — the backoff
    /// cancellation normalizes the last error message to
    /// `stopReason: "aborted"` with the errorMessage stripped.
    #[tokio::test]
    async fn aborts_backoff_sleep_and_returns_an_aborted_message() {
        let token = CancellationToken::new();
        let watcher_token = token.clone();
        let attempts = Arc::new(AtomicU32::new(0));
        let watcher_attempts = attempts.clone();
        let produce_attempts = attempts.clone();
        let finished: FinishedLog = Arc::new(Mutex::new(Vec::new()));
        let finished_callback = finished.clone();
        tokio::spawn(async move {
            // Let one error call resolve and the first backoff sleep start,
            // then abort.
            while watcher_attempts.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            watcher_token.cancel();
        });
        let enabled = RetryPolicy {
            enabled: true,
            max_retries: 5,
            base_delay_ms: 10_000,
            max_agent_delay_ms: None,
        };
        let result = retry_assistant_call(
            move || {
                let attempts = produce_attempts.clone();
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    error_msg("terminated")
                }
            },
            Some(&enabled),
            Some(&token),
            &mut RetryCallbacks {
                on_retry_finished: Some(Box::new(move |success, attempt, error| {
                    finished_callback.lock().unwrap().push((
                        success,
                        attempt,
                        error.map(str::to_string),
                    ));
                })),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(result.stop_reason, StopReason::Aborted);
        assert_eq!(result.error_message, None);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(
            *finished.lock().unwrap(),
            [(false, 1, Some("terminated".to_string()))]
        );
    }

    #[tokio::test]
    async fn does_not_retry_when_policy_is_absent() {
        let (result, attempts) = call(
            None,
            vec![error_msg("terminated")],
            &mut RetryCallbacks::default(),
        )
        .await;
        assert_eq!(result.stop_reason, StopReason::Error);
        assert_eq!(attempts, 1);
    }

    #[tokio::test]
    async fn emits_on_retry_attempt_start_after_backoff_before_each_retried_call() {
        let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let scheduled_events = events.clone();
        let attempt_start_events = events.clone();
        let (result, attempts) = call(
            Some(policy(true)),
            vec![
                error_msg("terminated"),
                error_msg("terminated"),
                message(StopReason::Stop, None),
            ],
            &mut RetryCallbacks {
                on_retry_scheduled: Some(Box::new(move |attempt, _, _, _| {
                    scheduled_events
                        .lock()
                        .unwrap()
                        .push(format!("retry:{attempt}"));
                })),
                on_retry_attempt_start: Some(Box::new(move || {
                    attempt_start_events
                        .lock()
                        .unwrap()
                        .push("attempt-start".to_string());
                })),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(result.stop_reason, StopReason::Stop);
        assert_eq!(attempts, 3);
        // Produce call markers interleave via the outcome script: two retries
        // means two scheduled/attempt-start pairs before the third produce.
        let log = events.lock().unwrap();
        assert_eq!(
            *log,
            [
                "retry:1".to_string(),
                "attempt-start".to_string(),
                "retry:2".to_string(),
                "attempt-start".to_string(),
            ]
        );
    }
}
