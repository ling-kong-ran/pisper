//! Port of upstream `coding-agent/src/core/http-dispatcher.ts`.
//!
//! Deterministic surface (idle-timeout parse/format tables, proxy-env
//! application) is byte-pinned against the real upstream module under node in
//! `tests/fixtures/core_oracle_model/http_dispatcher.oracle.json` (generator
//! `oracle_http_dispatcher.mjs`).
//!
//! Disclosure (transport seam): upstream `configureHttpDispatcher` installs a
//! global undici `EnvHttpProxyAgent` (`allowH2: false`, `proxyTunnel: true`,
//! body/headers timeout = the parsed value, CONNECT
//! `autoSelectFamilyAttemptTimeout` = 2000ms) and re-points `globalThis.fetch`
//! at undici. reqwest has no global dispatcher to swap, and the port's shared
//! client is built once in [`crate::ai::api::http_client`]; so the port keeps
//! the *validated configuration* in a process-global slot
//! ([`configure_http_dispatcher`] → [`global_http_dispatcher_config`]) for the
//! transport seam to consume, and performs the same validation (including the
//! exact upstream error text for an invalid timeout). The undici-only knobs
//! (error-listener suppression, the `installedGlobalFetch` guard) have no
//! Rust analogue and are disclosed as not ported.
//!
//! `parseHttpIdleTimeoutMs` accepts JS `Number(string)` inputs; the port
//! splits the string and number branches explicitly (upstream recurses into
//! itself with `Number(trimmed)`). `Number` accepts a wider literal set than
//! [`f64::from_str`] (e.g. `"0x10"` → 16); settings values are decimal, so
//! the residual divergence is disclosed rather than emulated.

use serde_json::Value;
use std::sync::RwLock;

/// Upstream `DEFAULT_HTTP_IDLE_TIMEOUT_MS`.
pub const DEFAULT_HTTP_IDLE_TIMEOUT_MS: u64 = 300_000;

/// Node's 250ms default can terminate valid connection attempts on
/// high-latency routes (upstream `DEFAULT_AUTO_SELECT_FAMILY_ATTEMPT_TIMEOUT_MS`).
pub const DEFAULT_AUTO_SELECT_FAMILY_ATTEMPT_TIMEOUT_MS: u64 = 2_000;

/// Upstream `HTTP_IDLE_TIMEOUT_CHOICES` entry (label + ms pairs, in order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpIdleTimeoutChoice {
    pub label: &'static str,
    pub timeout_ms: u64,
}

/// Upstream `HTTP_IDLE_TIMEOUT_CHOICES`.
pub const HTTP_IDLE_TIMEOUT_CHOICES: &[HttpIdleTimeoutChoice] = &[
    HttpIdleTimeoutChoice {
        label: "30 sec",
        timeout_ms: 30_000,
    },
    HttpIdleTimeoutChoice {
        label: "1 min",
        timeout_ms: 60_000,
    },
    HttpIdleTimeoutChoice {
        label: "2 min",
        timeout_ms: 120_000,
    },
    HttpIdleTimeoutChoice {
        label: "5 min",
        timeout_ms: 300_000,
    },
    HttpIdleTimeoutChoice {
        label: "disabled",
        timeout_ms: 0,
    },
];

/// The string branch of upstream `parseHttpIdleTimeoutMs`.
pub fn parse_http_idle_timeout_ms_str(value: &str) -> Option<u64> {
    let trimmed = value.trim();
    if trimmed.eq_ignore_ascii_case("disabled") {
        return Some(0);
    }
    if trimmed.is_empty() {
        return None;
    }
    // upstream: parseHttpIdleTimeoutMs(Number(trimmed)); non-numeric input
    // is NaN and falls through the number branch's finite check.
    parse_http_idle_timeout_ms_number(trimmed.parse::<f64>().unwrap_or(f64::NAN))
}

/// The number branch of upstream `parseHttpIdleTimeoutMs`.
pub fn parse_http_idle_timeout_ms_number(value: f64) -> Option<u64> {
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    Some(value.floor() as u64)
}

/// Upstream `parseHttpIdleTimeoutMs(value: unknown)`: strings recurse through
/// [`parse_http_idle_timeout_ms_str`], numbers through
/// [`parse_http_idle_timeout_ms_number`], everything else is `None`.
pub fn parse_http_idle_timeout_ms(value: &Value) -> Option<u64> {
    match value {
        Value::String(text) => parse_http_idle_timeout_ms_str(text),
        Value::Number(number) => {
            parse_http_idle_timeout_ms_number(number.as_f64().unwrap_or(f64::NAN))
        }
        _ => None,
    }
}

/// Upstream `formatHttpIdleTimeoutMs`: the choice label, else
/// `` `${timeoutMs / 1000} sec` `` (JS number rendering: `1.5 sec`).
pub fn format_http_idle_timeout_ms(timeout_ms: u64) -> String {
    if let Some(choice) = HTTP_IDLE_TIMEOUT_CHOICES
        .iter()
        .find(|item| item.timeout_ms == timeout_ms)
    {
        return choice.label.to_string();
    }
    format!("{} sec", timeout_ms as f64 / 1000.0)
}

/// Upstream `applyHttpProxySettings`: trim; empty is a no-op; otherwise set
/// `HTTP_PROXY`/`HTTPS_PROXY` only when unset (JS `??=` — an existing empty
/// value is "defined" and stays).
pub fn apply_http_proxy_settings(http_proxy: Option<&str>) {
    let Some(proxy) = http_proxy.map(str::trim).filter(|proxy| !proxy.is_empty()) else {
        return;
    };
    if std::env::var("HTTP_PROXY").is_err() {
        // SAFETY-free: std::env::set_var; tests serialize process-env access.
        std::env::set_var("HTTP_PROXY", proxy);
    }
    if std::env::var("HTTPS_PROXY").is_err() {
        std::env::set_var("HTTPS_PROXY", proxy);
    }
}

/// The retained configuration of [`configure_http_dispatcher`] (upstream: the
/// fields of the installed undici `EnvHttpProxyAgent`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpDispatcherConfig {
    pub body_timeout_ms: u64,
    pub headers_timeout_ms: u64,
    pub auto_select_family_attempt_timeout_ms: u64,
    /// Always `false` (upstream `allowH2: false`).
    pub allow_h2: bool,
    /// Always `true` (upstream `proxyTunnel: true` — HTTP origins stay on
    /// CONNECT tunnels as before Undici 8.7).
    pub proxy_tunnel: bool,
}

static GLOBAL_DISPATCHER_CONFIG: RwLock<Option<HttpDispatcherConfig>> = RwLock::new(None);

/// The last configuration installed by [`configure_http_dispatcher`], if any
/// (upstream `undici.getGlobalDispatcher()` reads the equivalent state).
pub fn global_http_dispatcher_config() -> Option<HttpDispatcherConfig> {
    *GLOBAL_DISPATCHER_CONFIG
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Upstream `configureHttpDispatcher(timeoutMs = DEFAULT)`: validate the
/// timeout (upstream error text verbatim on failure) and install the
/// dispatcher configuration in the process-global slot.
///
/// Not ported (disclosed, no Rust analogue): the undici error-listener
/// suppression, `undici.install()`'s fetch re-pointing, and the
/// `installedGlobalFetch` override-preservation guard.
pub fn configure_http_dispatcher(timeout_ms: f64) -> Result<HttpDispatcherConfig, String> {
    let Some(normalized_timeout_ms) = parse_http_idle_timeout_ms_number(timeout_ms) else {
        return Err(format!(
            "Invalid HTTP idle timeout: {}",
            js_number_to_string(timeout_ms)
        ));
    };
    let config = HttpDispatcherConfig {
        body_timeout_ms: normalized_timeout_ms,
        headers_timeout_ms: normalized_timeout_ms,
        auto_select_family_attempt_timeout_ms: DEFAULT_AUTO_SELECT_FAMILY_ATTEMPT_TIMEOUT_MS,
        allow_h2: false,
        proxy_tunnel: true,
    };
    *GLOBAL_DISPATCHER_CONFIG
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(config);
    Ok(config)
}

/// JS `String(number)` for the finite values settings can carry (integral
/// renders without a fraction, otherwise the shortest round-trip form, which
/// matches Rust's default float rendering for these magnitudes).
fn js_number_to_string(value: f64) -> String {
    match value {
        v if v.is_nan() => "NaN".to_string(),
        v if v == f64::INFINITY => "Infinity".to_string(),
        v if v == f64::NEG_INFINITY => "-Infinity".to_string(),
        v if v.fract() == 0.0 && v.abs() < 1e21 => format!("{}", v as i64),
        v => format!("{v}"),
    }
}

#[cfg(test)]
#[path = "http_dispatcher_tests.rs"]
mod tests;
