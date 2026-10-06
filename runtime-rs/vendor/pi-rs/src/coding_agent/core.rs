//! Port of upstream `coding-agent` `src/core` leaf modules (slice W3.3).
//!
//! Provenance map (upstream file → submodule), upstream SHA256 at migration
//! time:
//!
//! | upstream                 | submodule                          | sha256 (first 12) |
//! |--------------------------|------------------------------------|-------------------|
//! | defaults.ts              | [`defaults`]                       | e05e32e56130      |
//! | diagnostics.ts           | [`diagnostics`]                    | 1914431098db      |
//! | event-bus.ts             | [`event_bus`]                      | fe5c39a57081      |
//! | keybindings.ts           | [`keybindings`]                    | e140384fc16b      |
//! | cache-stats.ts           | [`cache_stats`]                    | 993ceca6dd28      |
//! | messages.ts              | [`messages`]                       | 5397e3c96c95      |
//! | model-config.ts          | [`model_config`]                   | 9f50d407ea12      |
//! | models-store.ts          | [`models_store`]                   | 9a93afb3ad91      |
//! | footer-data-provider.ts  | [`footer_data_provider`]           | 024ae5b3f50a      |
//!
//! Deterministic outputs (event dispatch order, keybinding defaults table,
//! message serialization, migration ordering, models-store file bytes,
//! models.json schema-error rendering) were captured from the real upstream
//! TypeScript sources under node (type stripping) into
//! `tests/fixtures/core_oracle/*.oracle.json` and are pinned in [`oracle_data`]
//! (test-only) with per-module byte-comparison tests.
//!
//! Shared seams (upstream imports modules outside this slice; disclosed
//! per submodule as well):
//!
//! - `getAgentDir` (upstream `../config.ts`, not yet ported): the minimal
//!   [`get_agent_dir`] resolution is vendored here — the
//!   `PI_CODING_AGENT_DIR` env override (upstream `ENV_AGENT_DIR`) followed
//!   by `~/.pi/agent`. Tests override it through the same env var, exactly
//!   like the upstream suites.
//! - `SessionEntry` (upstream `./session-manager.ts`, not yet ported):
//!   [`cache_stats`] scans a local [`crate::coding_agent::core::cache_stats::SessionEntry`]
//!   seam enum instead.
//! - `AuthStorageBackend` / `FileAuthStorageBackend` (upstream
//!   `./auth-storage.ts`, not yet ported): [`models_store`] vendors the
//!   `withLockAsync` file-lock subset it needs, including the proper-lockfile
//!   `<path>.lock` directory protocol.

pub mod agent_session_runtime;
pub mod agent_session_services;
pub mod auth_guidance;
pub mod auth_storage;
pub mod bug_report;
pub mod cache_stats;
pub mod cache_warmer;
pub mod compaction;
pub mod crash_log;
pub mod defaults;
pub mod diagnostics;
pub mod event_bus;
pub mod footer_data_provider;
pub mod http_dispatcher;
pub mod keybindings;
pub mod mcp_servers;
pub mod messages;
pub mod model_config;
pub mod model_registry;
pub mod model_resolver;
pub mod model_runtime;
pub mod models_store;
pub mod nested_tool_calls;
pub mod output_guard;
pub mod project_trust;
pub mod provider_attribution;
pub mod provider_composer;
pub mod resolve_config_value;
pub mod resource_loader;
pub mod runtime_credentials;
pub mod sdk;
pub mod session_cwd;
pub mod settings_diagnostics;
pub mod settings_manager;
pub mod skills;
pub mod trust_manager;
pub mod usage_totals;
pub mod virtual_models;

#[cfg(test)]
pub mod oracle_data;

use crate::coding_agent::utils::paths::normalize_path;

/// Host-platform path join standing in for node's `path.join` (the port's
/// `node_path` module exposes per-platform variants).
pub(crate) fn path_join(base: &str, segment: &str) -> String {
    if cfg!(windows) {
        crate::coding_agent::utils::node_path::win32_join(&[base, segment])
    } else {
        crate::coding_agent::utils::node_path::posix_join(&[base, segment])
    }
}

/// Upstream `ENV_AGENT_DIR` (`${APP_NAME.toUpperCase()}_CODING_AGENT_DIR`
/// with the default `pi` app name).
pub const ENV_AGENT_DIR: &str = "PI_CODING_AGENT_DIR";

/// Upstream `CONFIG_DIR_NAME` default (`pkg.piConfig?.configDir || ".pi"`).
pub const CONFIG_DIR_NAME: &str = ".pi";

/// Seam for upstream `getAgentDir()` (config.ts, outside this slice): the
/// `PI_CODING_AGENT_DIR` env override (tilde-expanded) or `~/.pi/agent`.
pub fn get_agent_dir() -> String {
    if let Ok(env_dir) = std::env::var(ENV_AGENT_DIR) {
        if !env_dir.is_empty() {
            // upstream: expandTildePath(envDir) === normalizePath(envDir)
            return normalize_path(&env_dir).unwrap_or(env_dir);
        }
    }
    let home = dirs::home_dir()
        .map(|p| {
            p.to_string_lossy()
                .trim_end_matches(['/', '\\'])
                .to_string()
        })
        .unwrap_or_default();
    format!("{home}/{CONFIG_DIR_NAME}/agent")
}

#[cfg(test)]
mod trust_test_support;

pub mod tools;
