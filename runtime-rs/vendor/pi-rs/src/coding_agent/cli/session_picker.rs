//! Port of upstream `coding-agent/src/cli/session-picker.ts` (sha256
//! 3418d62b3792…): TUI session selector for the `--resume` flag.
//!
//! Seam (disclosed): `SessionSelectorComponent` belongs to the interactive
//! session shell (`modes/interactive/components/session-selector.ts`), which
//! is not yet ported. The port keeps the loader contract
//! ([`SessionsLoader`], upstream `(onProgress?) => Promise<SessionInfo[]>`
//! over the ported `session_manager` types) and the
//! select/cancel/exit/requestRender wiring surface; behavior completes when
//! the interactive slice lands.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::coding_agent::core::settings_manager::SettingsManager;

/// Upstream `SessionsLoader`: resolves the session list, optionally
/// reporting progress.
pub type SessionsLoader = Arc<
    dyn Fn(Option<SessionListProgress>) -> Pin<Box<dyn Future<Output = Vec<SessionInfo>> + Send>>
        + Send
        + Sync,
>;

/// Upstream `SessionInfo` (`core/session-manager.ts`, not yet ported — the
/// picker only passes these values through).
#[derive(Debug, Clone, PartialEq)]
pub struct SessionInfo {
    pub path: String,
}

/// Upstream `SessionListProgress` callback payload.
pub type SessionListProgress = Arc<dyn Fn(usize, usize) + Send + Sync>;

/// Callbacks of the selector screen (select a path, cancel, hard-exit the
/// process, request a render).
pub struct SessionSelectorCallbacks {
    pub on_select: Box<dyn Fn(String) + Send + Sync>,
    pub on_cancel: Box<dyn Fn() + Send + Sync>,
    pub on_exit: Box<dyn Fn() + Send + Sync>,
    pub request_render: Box<dyn Fn() + Send + Sync>,
}

/// Options tail of `SessionSelectorComponent` (`{ showRenameHint: false,
/// keybindings }` — the port surfaces the flag; keybindings ride the
/// interactive slice).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionSelectorOptions {
    pub show_rename_hint: bool,
}

/// Upstream `selectSession`'s contract: show the TUI session selector over
/// the two session loaders (current-project and all) and return the selected
/// session path, or `None` when cancelled.
///
/// Seam: the TUI screen is injectable via [`SessionSelectorUi`]; once the
/// interactive shell lands, the real component plugs in here.
pub fn select_session(
    ui: Arc<dyn SessionSelectorUi>,
    current_sessions_loader: SessionsLoader,
    all_sessions_loader: SessionsLoader,
    settings_manager: Arc<SettingsManager>,
    callbacks: SessionSelectorCallbacks,
    options: SessionSelectorOptions,
) -> Pin<Box<dyn Future<Output = Option<String>> + Send>> {
    let _ = (
        current_sessions_loader,
        all_sessions_loader,
        settings_manager,
        options,
    );
    Box::pin(async move { ui.show(callbacks).await })
}

/// The TUI seam.
pub trait SessionSelectorUi: Send + Sync {
    fn show(
        &self,
        callbacks: SessionSelectorCallbacks,
    ) -> Pin<Box<dyn Future<Output = Option<String>> + Send>>;
}

#[cfg(test)]
#[path = "session_picker_tests.rs"]
mod tests;
