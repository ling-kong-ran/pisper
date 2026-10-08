//! Port of upstream `coding-agent/src/cli/setup.ts` (sha256 94e76bf5f1f4…):
//! process-level CLI setup.
//!
//! Divergence 2: `process.title = APP_NAME` has no `std` equivalent and is
//! skipped (the env-var writes and the undici-equivalent dispatcher
//! configuration are ported).

use crate::coding_agent::core::http_dispatcher::{
    configure_http_dispatcher, DEFAULT_HTTP_IDLE_TIMEOUT_MS,
};

pub fn setup_cli() {
    // Upstream: process.title = APP_NAME — no std equivalent (divergence 2).
    std::env::set_var("PI_CODING_AGENT", "true");
    std::env::set_var("AI_AGENT", "pi");
    // Upstream: process.emitWarning = () => {} — Rust has no global warning
    // channel to silence (divergence 2).

    // Configure the HTTP dispatcher before provider SDKs issue requests.
    // Settings are applied once SettingsManager has loaded global/project
    // configuration; this call uses the upstream default timeout.
    let _ = configure_http_dispatcher(DEFAULT_HTTP_IDLE_TIMEOUT_MS as f64);
}

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;
