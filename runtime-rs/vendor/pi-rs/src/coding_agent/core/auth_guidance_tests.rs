//! Tests for the ported `coding-agent/src/core/auth-guidance.ts`.
//!
//! Source of truth: `tests/fixtures/core_oracle_w37/auth_guidance.oracle.json`
//! (captured from the real upstream module under node with
//! `PI_PACKAGE_DIR=/pi-oracle-w37-pkg`; win32 join/resolve semantics — the
//! Windows platform pin).

use super::{
    format_no_api_key_found_message, format_no_model_selected_message,
    format_no_models_available_message, get_provider_login_help,
};
use crate::coding_agent::oracle_scrub::scrub_str;
use serde_json::Value;

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn oracle() -> serde_json::Value {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/core_oracle_w37/auth_guidance.oracle.json"
    ))
    .unwrap()
}

/// environment-anchored: both sides normalized. The captured docs paths are
/// rooted at the capture machine's package dir (`C:\pi-oracle-w37-pkg\...`);
/// the root-relative `PI_PACKAGE_DIR` resolves against the live drive (and
/// stays separator-native on POSIX), so both sides go through the shared
/// anchor scrub before comparing.
fn scrubbed(value: &Value) -> String {
    scrub_str(value.as_str().expect("oracle string"))
}

#[test]
fn guidance_texts_match_the_oracle_capture() {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let previous = std::env::var("PI_PACKAGE_DIR").ok();
    std::env::set_var("PI_PACKAGE_DIR", "/pi-oracle-w37-pkg");

    let capture = oracle();
    assert_eq!(
        scrub_str(&get_provider_login_help()),
        scrubbed(&capture["provider_login_help"])
    );
    assert_eq!(
        scrub_str(&format_no_models_available_message()),
        scrubbed(&capture["no_models_available"])
    );
    assert_eq!(
        scrub_str(&format_no_model_selected_message()),
        scrubbed(&capture["no_model_selected"])
    );
    assert_eq!(
        scrub_str(&format_no_api_key_found_message("anthropic")),
        scrubbed(&capture["no_api_key"])
    );
    assert_eq!(
        scrub_str(&format_no_api_key_found_message("unknown")),
        scrubbed(&capture["no_api_key_unknown"])
    );

    match previous {
        Some(value) => std::env::set_var("PI_PACKAGE_DIR", value),
        None => std::env::remove_var("PI_PACKAGE_DIR"),
    }
}
