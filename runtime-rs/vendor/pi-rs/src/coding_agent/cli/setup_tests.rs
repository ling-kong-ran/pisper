//! Tests for the ported `coding-agent/src/cli/setup.ts` (env-var writes and
//! dispatcher configuration; `process.title` skipped — divergence 2).

use crate::coding_agent::cli::setup::setup_cli;

#[test]
fn setup_cli_sets_the_upstream_env_markers() {
    // Remove first so the assertion proves the write happened.
    std::env::remove_var("PI_CODING_AGENT");
    std::env::remove_var("AI_AGENT");
    setup_cli();
    assert_eq!(std::env::var("PI_CODING_AGENT").as_deref(), Ok("true"));
    assert_eq!(std::env::var("AI_AGENT").as_deref(), Ok("pi"));
}

#[test]
fn setup_cli_configures_the_http_dispatcher() {
    setup_cli();
    let config = crate::coding_agent::core::http_dispatcher::global_http_dispatcher_config()
        .expect("dispatcher configured");
    assert_eq!(
        config.body_timeout_ms,
        crate::coding_agent::core::http_dispatcher::DEFAULT_HTTP_IDLE_TIMEOUT_MS
    );
    assert!(config.proxy_tunnel);
    assert!(!config.allow_h2);
}
