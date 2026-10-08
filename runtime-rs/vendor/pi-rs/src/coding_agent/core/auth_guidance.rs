//! Port of upstream `coding-agent/src/core/auth-guidance.ts`: the
//! provider-login guidance texts surfaced when no models / no model / no API
//! key is available.
//!
//! Seam (upstream `../config.ts`, outside this slice): `getDocsPath()` is
//! `resolve(join(getPackageDir(), "docs"))`, where `getPackageDir()` honors
//! the upstream `PI_PACKAGE_DIR` env override and otherwise discovers the
//! installed node package root from `import.meta.url`. The port reproduces
//! the env override (`normalizePath` → `join(…, "docs")` → node-style
//! `resolve` against the process cwd, all host-platform like upstream); the
//! no-env fallback has no Rust install-layout equivalent, so it resolves the
//! bare `docs` segment against the cwd (disclosed divergence — the override
//! is the path every oracle/test pins). The captured texts
//! (`tests/fixtures/core_oracle_w37/auth_guidance.oracle.json`) are the Windows
//! platform pin (same convention as `keybindings_win32.oracle.json`).

use crate::coding_agent::core::path_join;
use crate::coding_agent::utils::paths::{normalize_path, resolve_path_auto_base};

/// Upstream `UNKNOWN_PROVIDER`.
pub const UNKNOWN_PROVIDER: &str = "unknown";

/// Upstream `getPackageDir`'s env override (`config.ts`).
pub const ENV_PACKAGE_DIR: &str = "PI_PACKAGE_DIR";

/// Upstream `getDocsPath()`: `resolve(join(getPackageDir(), "docs"))`.
pub fn get_docs_path() -> String {
    let package_dir = match std::env::var(ENV_PACKAGE_DIR) {
        Ok(env_dir) if !env_dir.is_empty() => normalize_path(&env_dir).unwrap_or(env_dir),
        // Upstream default: the node package root discovered from
        // `import.meta.url`; the port has no install layout to discover and
        // falls back to the bare segment (see module docs).
        _ => String::from("docs"),
    };
    let joined = path_join(&package_dir, "docs");
    resolve_path_auto_base(&joined).unwrap_or(joined)
}

/// Upstream `getProviderLoginHelp()`.
pub fn get_provider_login_help() -> String {
    [
        "Use /login to log into a provider via OAuth or API key. See:".to_string(),
        format!("  {}", path_join(&get_docs_path(), "providers.md")),
        format!("  {}", path_join(&get_docs_path(), "models.md")),
    ]
    .join("\n")
}

/// Upstream `formatNoModelsAvailableMessage()`.
pub fn format_no_models_available_message() -> String {
    format!("No models available. {}", get_provider_login_help())
}

/// Upstream `formatNoModelSelectedMessage()`.
pub fn format_no_model_selected_message() -> String {
    format!(
        "No model selected.\n\n{}\n\nThen use /model to select a model.",
        get_provider_login_help()
    )
}

/// Upstream `formatNoApiKeyFoundMessage(provider)`.
pub fn format_no_api_key_found_message(provider: &str) -> String {
    let provider_display = if provider == UNKNOWN_PROVIDER {
        "the selected model"
    } else {
        provider
    };
    format!(
        "No API key found for {provider_display}.\n\n{}",
        get_provider_login_help()
    )
}

#[cfg(test)]
#[path = "auth_guidance_tests.rs"]
mod tests;
