//! Port of upstream `coding-agent/src/cli/config-selector.ts` (sha256
//! 780107f9a3a2…): TUI config selector for the `pi config` command.
//!
//! Seam (disclosed): the selector screen itself is
//! `modes/interactive/components/config-selector.ts` — part of the
//! interactive session shell, which is not yet ported. The port keeps the
//! call contract ([`ConfigSelectorOptions`], field-for-field with upstream's
//! `ConfigSelectorOptions`) and routes the screen through the
//! [`ConfigSelectorUi`] seam; behavior completes when the interactive slice
//! lands.

use std::sync::Arc;

use crate::coding_agent::core::settings_manager::SettingsManager;

/// Upstream `ScopedResolvedPaths` (`modes/interactive/components/
/// config-selector.ts`); carried opaquely by the port until that slice lands.
/// The upstream shape is the resolved package-resource path sets per scope;
/// the interactive slice will replace this opaque stand-in.
#[derive(Debug, Clone, Default)]
pub struct ScopedResolvedPaths;

/// Upstream `ConfigSelectorOptions`.
#[derive(Clone)]
pub struct ConfigSelectorOptions {
    pub resolved_paths: Arc<ScopedResolvedPaths>,
    pub settings_manager: Arc<SettingsManager>,
    pub cwd: String,
    pub agent_dir: String,
    pub write_scope: WriteScope,
    pub project_mode_available: bool,
}

/// Upstream `writeScope: "global" | "project"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteScope {
    Global,
    Project,
}

/// The TUI seam (upstream builds a `TuiMainScreen`, `initTheme`s, mounts a
/// `ConfigSelectorComponent` and resolves when the selector closes).
pub trait ConfigSelectorUi {
    fn select_config(&self, options: &ConfigSelectorOptions);
}

/// Upstream `selectConfig`: initialize the theme before showing the TUI, then
/// show the selector and return when it closes. The theme init and screen
/// are the seam; the option contract is complete.
pub fn select_config(ui: &dyn ConfigSelectorUi, options: &ConfigSelectorOptions) {
    // Upstream: initTheme(options.settingsManager.getTheme(), true) before the
    // TUI mounts (theme watcher lives in the interactive slice).
    ui.select_config(options);
}
