//! Port of upstream `coding-agent/src/cli/project-trust.ts` (sha256
//! f53dace04498…): build the [`ProjectTrustContext`] handed to
//! `core/project-trust.ts`.
//!
//! Startup selectors still use the injectable host. The runtime bridge below
//! provides the real extensions ProjectTrustContext/async ExtensionUI contract
//! to core/project_trust without losing the independent hasUI flag.

use std::sync::Arc;

use crate::coding_agent::cli::startup_ui::{
    show_startup_input, show_startup_selector, StartupSelectorOption,
};
use crate::coding_agent::core::settings_manager::SettingsManager;

/// Upstream `AppMode` (`core/project-trust.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    Interactive,
    Print,
    Json,
    Rpc,
}

impl AppMode {
    /// Upstream `options.mode === "interactive" ? "tui" : options.mode`
    /// (`ProjectTrustContext.mode` uses the UI-level `"tui"` label).
    pub fn context_mode(self) -> &'static str {
        match self {
            AppMode::Interactive => "tui",
            AppMode::Print => "print",
            AppMode::Json => "json",
            AppMode::Rpc => "rpc",
        }
    }
}

/// Upstream notify `type` (`"info" | "warning" | "error"`, default `"info"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NotifyType {
    #[default]
    Info,
    Warning,
    Error,
}

/// Upstream `ctx.ui` (`select`/`confirm`/`input`/`notify`) as a trait.
pub trait ProjectTrustUi: Send + Sync {
    fn select(&self, title: &str, options: &[String]) -> Option<String>;
    fn confirm(&self, title: &str, message: &str) -> bool;
    fn input(&self, title: &str, placeholder: Option<&str>) -> Option<String>;
    fn notify(&self, message: &str, notify_type: NotifyType);
}

/// Upstream `ProjectTrustContext`.
#[derive(Clone)]
pub struct ProjectTrustContext {
    pub cwd: String,
    /// Upstream `mode`: `"tui"` for interactive, otherwise the CLI mode.
    pub mode: &'static str,
    pub has_ui: bool,
    pub ui: Arc<dyn ProjectTrustUi>,
}

/// Upstream `createProjectTrustContext`'s options.
pub struct CreateProjectTrustContextOptions {
    pub cwd: String,
    pub mode: AppMode,
    pub settings_manager: Arc<SettingsManager>,
    pub has_ui: bool,
}

/// The TUI-backed `ctx.ui`: delegates to the startup-UI seam exactly like
/// upstream's inline closures (gating on `hasUI` and interactive mode).
pub struct TuiProjectTrustUi {
    pub settings_manager: Arc<SettingsManager>,
    pub mode: AppMode,
    pub has_ui: bool,
}

impl ProjectTrustUi for TuiProjectTrustUi {
    fn select(&self, title: &str, options: &[String]) -> Option<String> {
        if !self.has_ui {
            return None;
        }
        if self.mode != AppMode::Interactive {
            return None;
        }
        show_startup_selector(
            title,
            options
                .iter()
                .map(|option| StartupSelectorOption {
                    label: option.clone(),
                    value: option.clone(),
                })
                .collect(),
        )
    }

    fn confirm(&self, title: &str, message: &str) -> bool {
        if !self.has_ui {
            return false;
        }
        if self.mode != AppMode::Interactive {
            return false;
        }
        show_startup_selector(
            &format!("{title}\n{message}"),
            vec![
                StartupSelectorOption {
                    label: "Yes".to_string(),
                    value: "Yes".to_string(),
                },
                StartupSelectorOption {
                    label: "No".to_string(),
                    value: "No".to_string(),
                },
            ],
        )
        .map(|selected| selected == "Yes")
        .unwrap_or(false)
    }

    fn input(&self, title: &str, placeholder: Option<&str>) -> Option<String> {
        if !self.has_ui {
            return None;
        }
        if self.mode != AppMode::Interactive {
            return None;
        }
        show_startup_input(title, placeholder)
    }

    fn notify(&self, message: &str, notify_type: NotifyType) {
        if self.mode != AppMode::Interactive {
            // Upstream: chalk red / yellow / cyan; rendered plain here (chalk
            // emits no codes without a TTY, divergence 1).
            let _color = match notify_type {
                NotifyType::Error => "red",
                NotifyType::Warning => "yellow",
                NotifyType::Info => "cyan",
            };
            eprintln!("{message}");
        }
    }
}

/// Upstream `createProjectTrustContext`.
pub fn create_project_trust_context(
    options: CreateProjectTrustContextOptions,
) -> ProjectTrustContext {
    ProjectTrustContext {
        cwd: options.cwd,
        mode: options.mode.context_mode(),
        has_ui: options.has_ui,
        ui: Arc::new(TuiProjectTrustUi {
            settings_manager: options.settings_manager,
            mode: options.mode,
            has_ui: options.has_ui,
        }),
    }
}

/// Build the context consumed by the real async core trust resolver.
pub fn create_runtime_project_trust_context(
    options: CreateProjectTrustContextOptions,
) -> crate::coding_agent::extensions::types::ProjectTrustContext {
    use crate::coding_agent::extensions::types::ExtensionMode;
    let mode = match options.mode {
        AppMode::Interactive => ExtensionMode::Tui,
        AppMode::Print => ExtensionMode::Print,
        AppMode::Json => ExtensionMode::Json,
        AppMode::Rpc => ExtensionMode::Rpc,
    };
    let context = create_project_trust_context(options);
    crate::coding_agent::extensions::types::ProjectTrustContext {
        cwd: context.cwd,
        mode,
        has_ui: context.has_ui,
        ui: Some(Arc::new(StartupTrustUiAdapter(context.ui))),
    }
}

struct StartupTrustUiAdapter(Arc<dyn ProjectTrustUi>);
impl crate::coding_agent::extensions::types::ExtensionUI for StartupTrustUiAdapter {
    fn select<'a>(
        &'a self,
        title: &'a str,
        options: &'a [String],
        _: &'a crate::coding_agent::extensions::types::ExtensionUiDialogOptions,
    ) -> crate::coding_agent::extensions::types::UiFuture<'a, Option<String>> {
        Box::pin(async move { Ok(self.0.select(title, options)) })
    }
    fn confirm<'a>(
        &'a self,
        title: &'a str,
        message: &'a str,
        _: &'a crate::coding_agent::extensions::types::ExtensionUiDialogOptions,
    ) -> crate::coding_agent::extensions::types::UiFuture<'a, bool> {
        Box::pin(async move { Ok(self.0.confirm(title, message)) })
    }
    fn input<'a>(
        &'a self,
        title: &'a str,
        placeholder: Option<&'a str>,
        _: &'a crate::coding_agent::extensions::types::ExtensionUiDialogOptions,
    ) -> crate::coding_agent::extensions::types::UiFuture<'a, Option<String>> {
        Box::pin(async move { Ok(self.0.input(title, placeholder)) })
    }
    fn notify(&self, message: &str, kind: Option<&str>) {
        self.0.notify(
            message,
            match kind {
                Some("error") => NotifyType::Error,
                Some("warning") => NotifyType::Warning,
                _ => NotifyType::Info,
            },
        );
    }
}

#[cfg(test)]
#[path = "project_trust_tests.rs"]
mod tests;
