//! Port of upstream `coding-agent/src/cli/startup-ui.ts` (sha256
//! 109f7896fbc8…): shared startup-TUI construction and the first-time-setup
//! gate.
//!
//! Seam (disclosed): the actual TUI screens
//! (`TuiMainScreen`/`ExtensionSelectorComponent`/`ExtensionInputComponent`/
//! `FirstTimeSetupComponent`, plus terminal theme detection) belong to the
//! interactive session shell, which is not yet ported. The port therefore
//! exposes an injectable [`StartupUiHost`] and keeps every deterministic
//! decision upstream makes outside the TUI:
//! - [`is_official_distribution`] / [`should_run_first_time_setup`] (the
//!   first-time-setup gate),
//! - [`load_themes`]' enable-and-dedupe-by-name rule,
//! - [`StartupSelectorOption`] label/value plumbing.
//!
//! The theme-loading pipeline (`loadThemeFromPath`) is part of the
//! interactive/theme slice; [`load_themes`] operates on already-loaded
//! `(name, enabled)` entries to keep its dedupe rule testable.

use crate::coding_agent::cli::{APP_NAME, CONFIG_DIR_NAME, ENV_AGENT_DIR};
use crate::coding_agent::package_manager::cli::PACKAGE_NAME;

const OFFICIAL_PACKAGE_NAME: &str = "@earendil-works/pi-coding-agent";
const OFFICIAL_APP_NAME: &str = "pi";
const OFFICIAL_CONFIG_DIR_NAME: &str = ".pi";

/// Upstream `DistributionMetadata`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistributionMetadata {
    pub package_name: String,
    pub app_name: String,
    pub config_dir_name: String,
}

/// Upstream `isOfficialDistribution`.
pub fn is_official_distribution(metadata: &DistributionMetadata) -> bool {
    metadata.package_name == OFFICIAL_PACKAGE_NAME
        && metadata.app_name == OFFICIAL_APP_NAME
        && metadata.config_dir_name == OFFICIAL_CONFIG_DIR_NAME
}

/// Upstream `areExperimentalFeaturesEnabled` (`core/experimental.ts`).
pub fn are_experimental_features_enabled() -> bool {
    std::env::var("PI_EXPERIMENTAL").as_deref() == Ok("1")
}

/// Upstream `shouldRunFirstTimeSetup`.
///
/// First-time setup runs when all of these hold:
/// - this is the official Pi distribution (not a fork/rebrand)
/// - experimental features are enabled (`PI_EXPERIMENTAL=1`)
/// - the default agent directory is used (no custom agent dir override)
/// - setup was not completed before (`settings.json` does not exist)
pub fn should_run_first_time_setup(settings_path: Option<&str>) -> bool {
    if !is_official_distribution(&DistributionMetadata {
        package_name: PACKAGE_NAME.to_string(),
        app_name: APP_NAME.to_string(),
        config_dir_name: CONFIG_DIR_NAME.to_string(),
    }) {
        return false;
    }
    if !are_experimental_features_enabled() {
        return false;
    }
    if std::env::var(ENV_AGENT_DIR)
        .map(|value| !value.is_empty())
        .unwrap_or(false)
    {
        return false;
    }
    let settings_path = match settings_path {
        Some(path) => path.to_string(),
        None => crate::coding_agent::core::path_join(
            &crate::coding_agent::core::get_agent_dir(),
            "settings.json",
        ),
    };
    !std::path::Path::new(&settings_path).exists()
}

/// A loaded theme entry as [`load_themes`] sees it (`(name, enabled)`; the
/// name is `None` when the theme file failed to parse a name).
pub type LoadedThemeEntry = (Option<String>, bool);

/// Upstream `loadThemes`' deterministic core: skip disabled resources, skip
/// entries without a name, and dedupe by name keeping the first occurrence.
/// Broken theme files never fail startup (upstream swallows in the same
/// place).
pub fn load_themes(resources: &[LoadedThemeEntry]) -> Vec<Option<String>> {
    let mut themes: Vec<Option<String>> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (name, enabled) in resources {
        if !enabled {
            continue;
        }
        let Some(name) = name else { continue };
        if seen.contains(name) {
            continue;
        }
        seen.insert(name.clone());
        themes.push(Some(name.clone()));
    }
    themes
}

/// An option of the startup selector (upstream
/// `Array<{ label: string; value: T }>`).
#[derive(Debug, Clone, PartialEq)]
pub struct StartupSelectorOption<T = String> {
    pub label: String,
    pub value: T,
}

/// The injectable TUI seam (String-valued options, the shape every in-repo
/// caller uses). Implementations run the real screens once the interactive
/// shell lands.
pub trait StartupUiHost {
    /// Upstream `showStartupSelector`.
    fn show_selector(
        &self,
        title: &str,
        options: Vec<StartupSelectorOption<String>>,
    ) -> Option<String>;

    /// Upstream `showStartupInput`.
    fn show_input(&self, title: &str, placeholder: Option<&str>) -> Option<String>;
}

/// Upstream `showStartupSelector` for `String`-valued options (the shape
/// `project-trust.ts` and `config-selector.ts` use), routed through the
/// process-global host when one is installed.
///
/// The upstream `settingsManager` parameter feeds `createStartupTui`
/// (theme/cursor/keybinding setup) — part of the unported interactive shell —
/// so the port drops it until that slice lands.
pub fn show_startup_selector(
    title: &str,
    options: Vec<StartupSelectorOption<String>>,
) -> Option<String> {
    global_host().and_then(|host| host.show_selector(title, options))
}

/// Upstream `showStartupInput`, routed through the process-global host (same
/// settings-manager note as [`show_startup_selector`]).
pub fn show_startup_input(title: &str, placeholder: Option<&str>) -> Option<String> {
    global_host().and_then(|host| host.show_input(title, placeholder))
}

type GlobalHost = std::sync::RwLock<Option<std::sync::Arc<dyn StartupUiHost + Send + Sync>>>;

static GLOBAL_HOST: std::sync::OnceLock<GlobalHost> = std::sync::OnceLock::new();

fn global_host() -> Option<std::sync::Arc<dyn StartupUiHost + Send + Sync>> {
    GLOBAL_HOST
        .get_or_init(|| std::sync::RwLock::new(None))
        .read()
        .ok()?
        .clone()
}

/// Install the process-global startup UI host (called by the interactive
/// shell once ported).
pub fn set_startup_ui_host(host: Option<std::sync::Arc<dyn StartupUiHost + Send + Sync>>) {
    let lock = GLOBAL_HOST.get_or_init(|| std::sync::RwLock::new(None));
    if let Ok(mut slot) = lock.write() {
        *slot = host;
    }
}

#[cfg(test)]
#[path = "startup_ui_tests.rs"]
mod tests;
