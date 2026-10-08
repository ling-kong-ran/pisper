//! Tests for the ported `coding-agent/src/core/http-dispatcher.ts`.
//!
//! The parse/format tables come from
//! `tests/fixtures/core_oracle_model/http_dispatcher.oracle.json` (real upstream
//! module under node; generator `oracle_http_dispatcher.mjs`). The proxy-env
//! scenarios mirror the upstream `http proxy settings` describe-block,
//! including the save/restore discipline around the process environment.

use super::*;
use serde_json::Value;

/// Upstream `http-dispatcher.ts` parse/format tables (real upstream module
/// under node; generator `tests/fixtures/core_oracle_model/oracle_http_dispatcher.mjs`).
const ORACLE: &str =
    include_str!("../../../tests/fixtures/core_oracle_model/http_dispatcher.oracle.json");

/// Serializes the process-env mutation in [`apply_http_proxy_settings`] tests
/// (cargo runs test targets in threads; upstream tests are per-file workers).
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn oracle() -> Value {
    serde_json::from_str(ORACLE).unwrap()
}

/// Upstream parity: the parse battery (string and non-string branches) must
/// match the oracle entry for entry, `null` meaning `None`.
#[test]
fn parse_tables_match_the_upstream_oracle() {
    let oracle = oracle();
    for case in oracle["parse"].as_array().unwrap() {
        let value = &case["value"];
        let expected = case["parsed"].as_u64();
        let actual = parse_http_idle_timeout_ms_str(value.as_str().unwrap());
        assert_eq!(actual, expected, "string case {value}");
    }
    for case in oracle["parseNonString"].as_array().unwrap() {
        let expected = case["parsed"].as_u64();
        // serde_json cannot hold non-finite floats or undefined; the f64
        // branch covers them directly (NaN/inf), objects/bools via Value.
        match case["value"].as_str() {
            Some("Infinity") => {
                assert_eq!(parse_http_idle_timeout_ms_number(f64::INFINITY), expected);
                assert_eq!(parse_http_idle_timeout_ms_str("Infinity"), expected);
            }
            Some("NaN") => {
                assert_eq!(parse_http_idle_timeout_ms_number(f64::NAN), expected);
            }
            Some("undefined") => {
                assert_eq!(parse_http_idle_timeout_ms(&Value::Null), expected);
            }
            Some(marker) => panic!("unhandled oracle marker {marker}"),
            None => {
                let value = &case["value"];
                if value.is_number() {
                    let number = value.as_f64().unwrap();
                    assert_eq!(
                        parse_http_idle_timeout_ms_number(number),
                        expected,
                        "number case {number}"
                    );
                    assert_eq!(
                        parse_http_idle_timeout_ms(&serde_json::json!(number)),
                        expected
                    );
                } else {
                    assert_eq!(
                        parse_http_idle_timeout_ms(value),
                        expected,
                        "value case {value}"
                    );
                }
            }
        }
    }
}

#[test]
fn format_table_matches_the_upstream_oracle() {
    let oracle = oracle();
    for case in oracle["format"].as_array().unwrap() {
        let timeout_ms = case["timeoutMs"].as_u64().unwrap();
        assert_eq!(
            format_http_idle_timeout_ms(timeout_ms),
            case["label"].as_str().unwrap(),
            "format case {timeout_ms}"
        );
    }
}

#[test]
fn defaults_and_choices_match_the_upstream_oracle() {
    let oracle = oracle();
    assert_eq!(
        oracle["defaultHttpIdleTimeoutMs"].as_u64().unwrap(),
        DEFAULT_HTTP_IDLE_TIMEOUT_MS
    );
    let choices: Vec<(String, u64)> = oracle["idleTimeoutChoices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|choice| {
            (
                choice["label"].as_str().unwrap().to_string(),
                choice["timeoutMs"].as_u64().unwrap(),
            )
        })
        .collect();
    let actual: Vec<(String, u64)> = HTTP_IDLE_TIMEOUT_CHOICES
        .iter()
        .map(|choice| (choice.label.to_string(), choice.timeout_ms))
        .collect();
    assert_eq!(actual, choices);
}

/// Upstream `applies httpProxy to HTTP_PROXY and HTTPS_PROXY`.
#[test]
fn apply_http_proxy_settings_sets_both_vars() {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    std::env::remove_var("HTTP_PROXY");
    std::env::remove_var("HTTPS_PROXY");

    apply_http_proxy_settings(Some("http://127.0.0.1:7890"));

    assert_eq!(
        std::env::var("HTTP_PROXY").unwrap(),
        "http://127.0.0.1:7890"
    );
    assert_eq!(
        std::env::var("HTTPS_PROXY").unwrap(),
        "http://127.0.0.1:7890"
    );

    std::env::remove_var("HTTP_PROXY");
    std::env::remove_var("HTTPS_PROXY");
}

/// Upstream `does not override existing proxy env vars`.
#[test]
fn apply_http_proxy_settings_does_not_override_existing() {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    std::env::set_var("HTTP_PROXY", "http://env-http:8080");
    std::env::set_var("HTTPS_PROXY", "http://env-https:8080");

    apply_http_proxy_settings(Some("http://settings:7890"));

    assert_eq!(std::env::var("HTTP_PROXY").unwrap(), "http://env-http:8080");
    assert_eq!(
        std::env::var("HTTPS_PROXY").unwrap(),
        "http://env-https:8080"
    );

    std::env::remove_var("HTTP_PROXY");
    std::env::remove_var("HTTPS_PROXY");
}

/// Upstream `ignores empty values` (whitespace-only included). An existing
/// empty value is *defined* (JS `??=` keeps it), mirrored by the var staying
/// unset here.
#[test]
fn apply_http_proxy_settings_ignores_empty_values() {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    std::env::remove_var("HTTP_PROXY");
    std::env::remove_var("HTTPS_PROXY");

    apply_http_proxy_settings(Some("   "));

    assert!(std::env::var("HTTP_PROXY").is_err());
    assert!(std::env::var("HTTPS_PROXY").is_err());

    apply_http_proxy_settings(None);
    assert!(std::env::var("HTTP_PROXY").is_err());
}

/// Upstream `configureHttpDispatcher` accepts the defaults and stores the
/// undici-equivalent configuration; an invalid timeout fails with the exact
/// upstream error text.
#[test]
fn configure_http_dispatcher_validates_and_stores_config() {
    let config = configure_http_dispatcher(DEFAULT_HTTP_IDLE_TIMEOUT_MS as f64).unwrap();
    assert_eq!(
        config,
        HttpDispatcherConfig {
            body_timeout_ms: 300_000,
            headers_timeout_ms: 300_000,
            auto_select_family_attempt_timeout_ms: DEFAULT_AUTO_SELECT_FAMILY_ATTEMPT_TIMEOUT_MS,
            allow_h2: false,
            proxy_tunnel: true,
        }
    );
    assert_eq!(global_http_dispatcher_config(), Some(config));

    // Disabled (0) is a valid value.
    let disabled = configure_http_dispatcher(0.0).unwrap();
    assert_eq!(disabled.body_timeout_ms, 0);

    for invalid in [-1.0, f64::NAN, f64::INFINITY] {
        let error = configure_http_dispatcher(invalid).unwrap_err();
        assert_eq!(
            error,
            format!("Invalid HTTP idle timeout: {}", js_number_render(invalid)),
        );
    }
    // Fractional values floor (the oracle's parse table).
    assert_eq!(parse_http_idle_timeout_ms_number(12.9), Some(12));
}

/// JS `String(number)` rendering for the invalid-timeout error text.
fn js_number_render(value: f64) -> String {
    match value {
        v if v.is_nan() => "NaN".to_string(),
        v if v == f64::INFINITY => "Infinity".to_string(),
        v if v == f64::NEG_INFINITY => "-Infinity".to_string(),
        v if v.fract() == 0.0 => format!("{}", v as i64),
        v => format!("{v}"),
    }
}
