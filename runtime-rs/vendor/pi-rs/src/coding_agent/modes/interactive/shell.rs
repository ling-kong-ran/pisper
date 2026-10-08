// The interactive UI glue mirrors upstream callback signatures whose types
// are inherently wide; factoring each into an alias adds no information.
#![allow(clippy::type_complexity)]
//! The interactive session shell (upstream `class InteractiveMode`, ported
//! from the byte-verbatim oracle bodies in
//! `tests/fixtures/interactive_r18_oracle/` + `tests/fixtures/interactive_r20_oracle/`).
//!
//! Decision and data-shape parity is enforced by replaying both node-captured
//! oracles byte-for-byte (`interactive_tests.rs`, `shell_oracle` and
//! `lower_oracle` groups). The collaborator recording surface:
//!
//! - containers/components/indicators go through the typed [`ShellView`]
//!   methods (S1);
//! - every non-decision collaborator call (`footer.*`, `terminal.*`,
//!   `settings.setX`, `themeController.*`, `process.*`, …) travels the
//!   [`ShellView::emit`] tuple pump (S1a);
//! - the lower-half command handlers are the [`CommandSink`] (S3); the real
//!   wiring ([`super::shell_lower::WiredCommands`]) dispatches to the shell
//!   bodies.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use serde_json::{json, Value};

use super::interactive_mode::AuthProviderOption;
use super::interactive_mode::{
    cache_miss_notice_text, compaction_cost_notice_text, count_dropped_thinking_blocks,
    is_anthropic_subscription_auth_key, CacheMiss, CompactionCostKind, CompactionCostNotice,
    ComponentKind, ComponentRef, ContainerId, EditorBorder, FocusTarget, InteractiveModeOptions,
    ModelRef, QueueMode, QueueSnapshot, ShellClock, ShellEditor, ShellHost, ShellPlatform,
    ShellSession, ShellSessionManager, ShellSettings, ShellView,
    ANTHROPIC_SUBSCRIPTION_AUTH_WARNING, INPUT_RING_ACTIONS,
};
use super::shell_lower::{
    assistant_diagnostics, assistant_error_message, assistant_set_error_message,
    assistant_stop_reason, assistant_tool_calls, create_summary_custom_message,
    first_changelog_version, parse_skill_block, reason_str, thinking_level_lower,
    tool_result_call_id, RenderSessionItem,
};
use super::theme::Theme;
use crate::agent_core::types::{AgentMessage, ThinkingLevel};
use crate::ai::types::primitives::Usage;
use crate::coding_agent::agent_session::{
    AgentSessionError, AgentSessionEvent, CompactionReason, CycleDirection,
    SummarizationRetrySource,
};
use crate::coding_agent::extensions::types::StreamingDelivery;
use crate::coding_agent::package_manager::PackageUpdate;
use crate::coding_agent::session_manager::{session_entry_to_context_messages, SessionEntry};

// ===========================================================================
// Shell wiring
// ===========================================================================

/// Upstream changelog helpers (changelog.ts).
pub trait ChangelogSource: Send + Sync {
    /// `parseChangelog(getChangelogPath())` → `[(version, content)]`.
    fn entries(&self) -> Vec<(String, String)>;
    /// `getNewEntries(entries, lastVersion)`.
    fn new_entries(&self, last_version: &str) -> Vec<(String, String)>;
    /// `normalizeChangelogLinks(content, entry)`.
    fn normalize_links(&self, content: &str) -> String;
}

/// The `checkForPackageUpdates` npm probe: upstream constructs
/// `new DefaultPackageManager({cwd, agentDir, settingsManager})` inline and
/// awaits `checkForAvailableUpdates()`; the shell projects `displayName`
/// and collapses failures to the empty vector (upstream `catch`). The
/// `DefaultPackageManager` port supplies the product implementation once the
/// settings-manager bridge lands; tests inject the scripted transport.
pub trait PackageUpdatesSource: Send + Sync {
    /// `cwd` is `sessionManager.getCwd()`, `agent_dir` is `getAgentDir()`.
    fn check_for_available_updates(
        &self,
        cwd: &str,
        agent_dir: &str,
    ) -> Result<Vec<PackageUpdate>, String>;
}

/// The collaborators bundle (upstream constructor dependency assembly).
pub struct ShellIo {
    pub session: Arc<dyn ShellSession>,
    pub session_manager: Arc<dyn ShellSessionManager>,
    pub settings: Arc<dyn ShellSettings>,
    pub view: Arc<dyn ShellView>,
    pub host: Arc<dyn ShellHost>,
    pub commands: Arc<dyn CommandSink>,
    pub clock: Arc<dyn ShellClock>,
    /// The OS/process seam (S7).
    pub platform: Arc<dyn ShellPlatform>,
    pub default_editor: Arc<dyn ShellEditor>,
    /// `defaultModelPerProvider` projection (`provider → default model id`),
    /// in upstream declaration order.
    pub default_model_per_provider: Vec<(String, String)>,
    /// `getAuthPath()`.
    pub auth_path: String,
    /// `getDocsPath()`.
    pub docs_path: String,
    /// `getDebugLogPath()`.
    pub debug_log_path: String,
    /// `APP_NAME`.
    pub app_name: String,
    /// `APP_TITLE`.
    pub app_title: String,
    /// `VERSION`.
    pub version: String,
    /// `os.homedir()`.
    pub home: String,
    /// The changelog fs probes.
    pub changelog: Box<dyn ChangelogSource + Send + Sync>,
    /// The `checkForPackageUpdates` npm probe (see [`PackageUpdatesSource`]).
    pub package_updates: Arc<dyn PackageUpdatesSource>,
    /// `keyText`/`keyDisplayText` over the keybindings manager (S19 hints).
    pub key_display: Box<dyn Fn(&str) -> String + Send + Sync>,
    /// The r17 theme singleton (`theme` upstream), swappable at runtime.
    pub theme: RwLock<Theme>,
    /// Cache-stats projection seam (`computeCacheWaste` /
    /// `getUsageCostBreakdown`): `None` falls back to the empty defaults.
    pub cache_stats: Option<(
        super::interactive_mode::CacheWaste,
        Vec<super::interactive_mode::UsageCostRow>,
    )>,
    /// The chalk `dim` styler (D2). The default is the fixed enabled-level
    /// form ([`default_chalk_dim`]); oracle drivers install the harness's
    /// fake chalk (literal brackets, no escape bytes).
    pub chalk_styler: Box<dyn Fn(&str) -> String + Send + Sync>,
}

/// The fixed chalk-level `dim` form (`\x1b[2m…\x1b[22m`).
pub fn default_chalk_dim(text: &str) -> String {
    format!("\x1b[2m{text}\x1b[22m")
}

/// Handle to a pending `getUserInput` waiter (upstream
/// `this.onInputCallback`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputSlot(pub u64);

/// Handle to an agent subscription (upstream `this.unsubscribe`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubscriptionSlot(pub u64);

/// Handle to a registered cleanup (signal/stdout/stderr listeners).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanupId(pub u64);

/// Upstream `this.activeSelectorToken` — a fresh per-show identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectorToken(pub u64);

/// Extension widget content (`setWidget`).
#[derive(Debug, Clone, PartialEq)]
pub enum WidgetContent {
    Lines(Vec<String>),
    Component(ComponentRef),
}

/// Extension widget placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetPlacement {
    AboveEditor,
    BelowEditor,
}

/// One pending extension dialog waiter (selector/input/editor share the
/// `resolve(Option<String>)` shape; confirm resolves through the selector).
#[allow(missing_debug_implementations)]
pub struct ExtensionDialog {
    pub component: ComponentRef,
    /// The abort signal probe (upstream `opts.signal?.aborted`).
    pub aborted: bool,
    pub resolve: tokio::sync::oneshot::Sender<Option<String>>,
}

/// All upstream private state fields, at their upstream initial values.
#[derive(Default)]
pub struct ShellState {
    pub is_initialized: bool,
    pub on_input_callback: Option<InputSlot>,
    pub pending_user_inputs: Vec<String>,
    pub active_status_indicator: Option<(String, ComponentRef)>,
    pub active_working_indicator_embedded: bool,
    pub working_message: Option<String>,
    pub working_visible: bool,
    pub working_indicator_options: Option<Value>,
    pub last_sigint_time: i64,
    pub last_escape_time: i64,
    pub changelog_markdown: Option<String>,
    pub startup_notices_shown: bool,
    pub anthropic_subscription_warning_shown: bool,
    /// `(chat container version, text id, spacer id)` at the last `showStatus`
    /// add; upstream compares the identity of the last two chat children.
    pub last_status: Option<(u64, u64, u64)>,
    /// The `/bug` hint is shown at most once per session so error output
    /// stays readable (upstream `bugReportHintShown`).
    pub bug_report_hint_shown: bool,
    /// Entry ids already rendered by the boundary-compaction re-render; the
    /// later `entry_appended` event for them is skipped (upstream
    /// `entriesRenderedByBoundaryCompaction`).
    pub entries_rendered_by_boundary_compaction: std::collections::HashSet<String>,
    pub managed_tool_status_started: bool,
    pub streaming_component: Option<ComponentRef>,
    pub streaming_message: Option<AgentMessage>,
    pub pending_tools: std::collections::HashMap<String, ComponentRef>,
    pub tool_output_expanded: bool,
    pub hide_thinking_block: bool,
    pub output_pad: i64,
    pub skill_commands: BTreeMap<String, String>,
    pub unsubscribe: Option<SubscriptionSlot>,
    pub signal_cleanup_handlers: Vec<CleanupId>,
    pub is_bash_mode: bool,
    pub bash_component: Option<ComponentRef>,
    pub pending_bash_components: Vec<ComponentRef>,
    pub auto_compaction_escape_handler_active: bool,
    pub retry_escape_handler_active: bool,
    pub compaction_queued_messages: Vec<super::interactive_mode::CompactionQueuedMessage>,
    pub shutdown_requested: bool,
    pub is_shutting_down: bool,
    pub extension_selector_active: bool,
    pub extension_input_active: bool,
    pub extension_editor_active: bool,
    pub extension_terminal_input_subscriptions: Vec<u64>,
    pub extension_widgets_above: Vec<(String, ComponentRef)>,
    pub extension_widgets_below: Vec<(String, ComponentRef)>,
    pub custom_footer_active: bool,
    pub custom_footer: Option<ComponentRef>,
    pub built_in_header: Option<ComponentRef>,
    pub custom_header: Option<ComponentRef>,
    /// The built-in footer component (`this.footer`).
    pub built_in_footer: Option<ComponentRef>,
    pub active_selector: Option<(SelectorToken, Option<ComponentRef>)>,
    pub fd_path: Option<String>,
    pub editor_is_custom: bool,
    /// The custom editor component while `editor_is_custom`.
    pub custom_editor: Option<Arc<dyn ShellEditor>>,
    /// `hiddenThinkingLabel` (`defaultHiddenThinkingLabel` when unset).
    pub hidden_thinking_label: Option<String>,
    pub autocomplete_provider_wrappers: usize,
    pub editor_component_factory: Option<Value>,
    pub extension_selector: Option<ComponentRef>,
    pub extension_input: Option<ComponentRef>,
    pub extension_editor: Option<ComponentRef>,
    /// Pending extension dialog waiters (selector/input/editor), newest last.
    pub extension_dialog: Option<ExtensionDialog>,
    // -- open selector components (lower half) --------------------------------
    pub settings_selector: Option<ComponentRef>,
    pub thinking_selector: Option<ComponentRef>,
    pub model_selector: Option<ComponentRef>,
    pub user_message_selector: Option<ComponentRef>,
    pub tree_selector: Option<ComponentRef>,
    pub session_selector: Option<ComponentRef>,
    pub trust_selector: Option<ComponentRef>,
    pub login_auth_selector: Option<(ComponentRef, Vec<String>, String)>,
    /// The Radius option of the top-level auth-type selector (v1.0.0): the
    /// full option label and the provider option it starts.
    pub login_auth_radius: Option<(String, AuthProviderOption)>,
    pub login_auth_selected: Option<String>,
    pub login_provider_selector: Option<ComponentRef>,
    pub logout_selector: Option<ComponentRef>,
    pub models_selector_models: Option<Vec<ModelRef>>,
    pub models_selector_enabled: Option<Vec<String>>,
    /// The open scoped-models selector component (for its cancel path).
    pub models_selector: Option<ComponentRef>,
    pub transcript_scroll_view: Option<ComponentRef>,
    pub fullscreen_layout_root: Option<Value>,
    pub main_screen_render_state: Option<Value>,
}

/// Read-only projection of the shell state for assertions.
#[derive(Debug, Clone)]
pub struct ShellStateSnapshot {
    pub is_bash_mode: bool,
    pub tool_output_expanded: bool,
    pub hide_thinking_block: bool,
    pub compaction_queued_messages: Vec<super::interactive_mode::CompactionQueuedMessage>,
    pub shutdown_requested: bool,
    pub is_shutting_down: bool,
    pub bug_report_hint_shown: bool,
    pub entries_rendered_by_boundary_compaction: Vec<String>,
    pub last_sigint_time: i64,
    pub last_escape_time: i64,
    pub pending_user_inputs: Vec<String>,
    pub is_initialized: bool,
    pub anthropic_subscription_warning_shown: bool,
    pub working_message: Option<String>,
    pub skill_commands: Vec<String>,
}

/// One upstream command invocation (`cmd.<name>` in the oracle log).
#[derive(Debug, Clone, PartialEq)]
pub enum ShellCommand {
    Settings,
    ScopedModels,
    Model(Option<String>),
    Thinking(Option<String>),
    Export(String),
    Import(String),
    Share,
    Bug(Option<String>),
    Copy {
        flash_confirmation: bool,
        prefer_selection: bool,
    },
    Name(String),
    Session,
    Changelog,
    Hotkeys,
    UserMessageSelector,
    Clone,
    Tree,
    Trust,
    Login(Option<String>),
    OAuthLogout,
    Clear,
    Compact(Option<String>),
    Reload,
    Debug,
    ArminSaysHi,
    DementedDelves,
    SessionSelector,
    Bash {
        command: String,
        exclude_from_context: bool,
    },
    TreeSelector,
    ModelSelector,
    /// `init()` (events arriving before initialization trigger init first).
    Init,
}

impl ShellCommand {
    /// The oracle `cmd.<name>` key for this invocation.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Settings => "showSettingsSelector",
            Self::ScopedModels => "showModelsSelector",
            Self::Model(_) => "handleModelCommand",
            Self::Thinking(_) => "handleThinkingCommand",
            Self::Export(_) => "handleExportCommand",
            Self::Import(_) => "handleImportCommand",
            Self::Share => "handleShareCommand",
            Self::Bug(_) => "handleBugCommand",
            Self::Copy { .. } => "handleCopyCommand",
            Self::Name(_) => "handleNameCommand",
            Self::Session => "handleSessionCommand",
            Self::Changelog => "handleChangelogCommand",
            Self::Hotkeys => "handleHotkeysCommand",
            Self::UserMessageSelector => "showUserMessageSelector",
            Self::Clone => "handleCloneCommand",
            Self::Tree => "showTreeSelector",
            Self::Trust => "showTrustSelector",
            Self::Login(_) => "handleLoginCommand",
            Self::OAuthLogout => "showOAuthSelector",
            Self::Clear => "handleClearCommand",
            Self::Compact(_) => "handleCompactCommand",
            Self::Reload => "handleReloadCommand",
            Self::Debug => "handleDebugCommand",
            Self::ArminSaysHi => "handleArminSaysHi",
            Self::DementedDelves => "handleDementedDelves",
            Self::SessionSelector => "showSessionSelector",
            Self::Bash { .. } => "handleBashCommand",
            Self::TreeSelector => "showTreeSelector",
            Self::ModelSelector => "showModelSelector",
            Self::Init => "init",
        }
    }
}

/// The lower-half command handlers (S3). The shell decides when each fires;
/// [`super::shell_lower::WiredCommands`] dispatches to the bodies.
pub trait CommandSink: Send + Sync {
    fn run(&self, command: ShellCommand);
}

/// `Ctrl+C`/`Ctrl+D` decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtrlCOutcome {
    Shutdown,
    ClearEditor,
}

/// `Ctrl+Z` decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuspendDecision {
    Suspend,
    Unsupported,
}

/// `MAX_WIDGET_LINES`.
pub const MAX_WIDGET_LINES: usize = 10;

/// The interactive session shell (upstream `InteractiveMode`).
pub struct InteractiveMode {
    pub(crate) io: ShellIo,
    state: Mutex<ShellState>,
    options: Mutex<InteractiveModeOptions>,
    next_id: AtomicU64,
}

impl InteractiveMode {
    /// Upstream `constructor(runtimeHost, options)`: options normalization
    /// (`tuiMode` default), hide-thinking/output-pad preload, and the theme
    /// initialized from the built-in dark document (upstream resolves the
    /// color mode via the terminal capability probe — S5).
    pub fn new(io: ShellIo, options: InteractiveModeOptions) -> Self {
        let hide_thinking_block = io.settings.hide_thinking_block();
        let output_pad = io.settings.output_pad();
        let mut normalized = options.clone();
        normalized.tui_mode = Some(
            options
                .tui_mode
                .clone()
                .unwrap_or_else(|| "regular".to_string()),
        );
        Self {
            io,
            state: Mutex::new(ShellState {
                hide_thinking_block,
                output_pad,
                hidden_thinking_label: Some("Thinking...".to_string()),
                // Upstream constructor: `this.workingVisible = true`.
                working_visible: true,
                ..ShellState::default()
            }),
            options: Mutex::new(normalized),
            next_id: AtomicU64::new(1),
        }
    }

    // -- accessors -----------------------------------------------------------

    pub(crate) fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    pub fn theme(&self) -> std::sync::RwLockReadGuard<'_, Theme> {
        self.io.theme.read().expect("theme lock")
    }

    pub fn options(&self) -> InteractiveModeOptions {
        self.options.lock().expect("options lock").clone()
    }

    fn lock_options_tui_mode(&self, mode: &str) {
        self.options.lock().expect("options lock").tui_mode = Some(mode.to_string());
    }

    /// Test fixtures start at upstream `isInitialized: true` (the oracle
    /// harness builds the shell after `init()`).
    #[cfg(test)]
    pub fn force_initialized(&self) {
        self.lock().is_initialized = true;
    }

    pub fn state_snapshot(&self) -> ShellStateSnapshot {
        let state = self.lock();
        ShellStateSnapshot {
            is_bash_mode: state.is_bash_mode,
            tool_output_expanded: state.tool_output_expanded,
            hide_thinking_block: state.hide_thinking_block,
            compaction_queued_messages: state.compaction_queued_messages.clone(),
            shutdown_requested: state.shutdown_requested,
            is_shutting_down: state.is_shutting_down,
            bug_report_hint_shown: state.bug_report_hint_shown,
            entries_rendered_by_boundary_compaction: state
                .entries_rendered_by_boundary_compaction
                .iter()
                .cloned()
                .collect(),
            last_sigint_time: state.last_sigint_time,
            last_escape_time: state.last_escape_time,
            pending_user_inputs: state.pending_user_inputs.clone(),
            is_initialized: state.is_initialized,
            anthropic_subscription_warning_shown: state.anthropic_subscription_warning_shown,
            working_message: state.working_message.clone(),
            skill_commands: state.skill_commands.keys().cloned().collect(),
        }
    }

    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, ShellState> {
        self.state.lock().expect("state lock")
    }

    /// Raw collaborator tuple (S1a).
    pub(crate) fn ev(&self, tuple: Value) {
        self.io.view.emit(tuple);
    }

    /// The fixed chalk-level dim styler (D2), over the swappable styler.
    pub fn chalk_dim(&self, text: &str) -> String {
        (self.io.chalk_styler)(text)
    }

    /// `theme.fg(color, text)` over the live theme singleton.
    pub(crate) fn fg(&self, color: &str, text: &str) -> String {
        self.theme().fg(color, text).unwrap_or_default()
    }

    /// `getAppKeyDisplay(action)` / `getEditorKeyDisplay(action)`.
    pub fn get_app_key_display(&self, action: &str) -> String {
        (self.io.key_display)(action)
    }

    /// The key the current border color resolves to
    /// (`theme.getThinkingBorderColor(this.session.thinkingLevel || "off")`).
    pub(crate) fn current_border(&self) -> EditorBorder {
        EditorBorder::Thinking(self.io.session.thinking_level())
    }

    /// `this.editor` (the possibly-custom editor).
    pub(crate) fn editor(&self) -> Arc<dyn ShellEditor> {
        let state = self.lock();
        if state.editor_is_custom {
            // The custom editor is registered through `set_editor`.
            state
                .custom_editor
                .clone()
                .unwrap_or_else(|| self.io.default_editor.clone())
        } else {
            self.io.default_editor.clone()
        }
    }

    // -- constructor wiring --------------------------------------------------

    /// Upstream constructor body: `runtimeHost.setBeforeSessionInvalidate(()
    /// => this.resetExtensionUI())` and `setRebindSession(...)`.
    pub fn bind_runtime_hooks(self: &Arc<Self>) {
        let weak_invalidate = Arc::downgrade(self);
        self.io
            .host
            .set_before_session_invalidate(Some(Box::new(move || {
                if let Some(shell) = weak_invalidate.upgrade() {
                    shell.reset_extension_ui();
                }
            })));
        let weak_rebind = Arc::downgrade(self);
        self.io.host.set_rebind_session(Some(Box::new(move || {
            let weak_rebind = weak_rebind.clone();
            Box::pin(async move {
                if let Some(shell) = weak_rebind.upgrade() {
                    shell.rebind_current_session().await;
                }
            })
        })));
    }

    /// Upstream `getStartupExpansionState`.
    pub fn get_startup_expansion_state(&self) -> bool {
        self.options().verbose || self.state_snapshot().tool_output_expanded
    }

    // -- status / notifications ----------------------------------------------

    /// Upstream `showManagedToolStatus`.
    pub fn show_managed_tool_status(&self, is_warning: bool, message: &str) {
        {
            let mut state = self.lock();
            if !state.managed_tool_status_started {
                self.io.view.container_add_spacer(ContainerId::Chat);
                state.managed_tool_status_started = true;
            }
        }
        let display = if is_warning {
            format!("Warning: {message}")
        } else {
            message.to_string()
        };
        let color = if is_warning { "warning" } else { "dim" };
        let text = self.fg(color, &display);
        self.io
            .view
            .container_add_text(ContainerId::Chat, &text, 1, 0, false);
        self.lock().last_status = None;
        self.io.view.request_render(None);
    }

    /// Upstream `showStatus` (immediately-sequential updates coalesce into a
    /// text mutation instead of appending rows).
    pub fn show_status(&self, message: &str) {
        let version = self.io.view.container_version(ContainerId::Chat);
        let coalesce = {
            let state = self.lock();
            state
                .last_status
                .is_some_and(|(added_version, spacer, text)| {
                    added_version == version && spacer != 0 && text != 0
                })
        };
        if coalesce {
            let (_, _, id) = self.lock().last_status.expect("coalesce ids");
            let text = self.fg("dim", message);
            self.io
                .view
                .container_set_text(ContainerId::Chat, id, &text);
            self.io.view.request_render(None);
            return;
        }
        let spacer = self.io.view.container_add_spacer(ContainerId::Chat);
        let text = self.fg("dim", message);
        let id = self
            .io
            .view
            .container_add_text(ContainerId::Chat, &text, 1, 0, false);
        let version = self.io.view.container_version(ContainerId::Chat);
        self.lock().last_status = Some((version, spacer, id));
        self.io.view.request_render(None);
    }

    /// Upstream `showError`.
    pub fn show_error(&self, error_message: &str) {
        let pad = self.lock().output_pad;
        self.io.view.container_add_spacer(ContainerId::Chat);
        let text = self.fg("error", &format!("Error: {error_message}"));
        self.io
            .view
            .container_add_text(ContainerId::Chat, &text, pad, 0, false);
        self.io.view.request_render(None);
    }

    /// Upstream `showWarning`.
    pub fn show_warning(&self, warning_message: &str) {
        self.io.view.container_add_spacer(ContainerId::Chat);
        let text = self.fg("warning", &format!("Warning: {warning_message}"));
        self.io
            .view
            .container_add_text(ContainerId::Chat, &text, 1, 0, false);
        self.io.view.request_render(None);
    }

    /// Upstream `clearEditor`.
    pub fn clear_editor(&self) {
        self.editor().set_text("");
        self.io.view.request_render(None);
    }

    /// Upstream `showNewVersionNotification`.
    pub fn show_new_version_notification(&self, version: &str, note: Option<&str>) {
        self.io.view.container_add_spacer(ContainerId::Chat);
        self.io
            .view
            .container_add_border(ContainerId::Chat, Some("warning"));
        let head = self.fg("warning", "Update Available");
        let body = format!("{}\n{}", self.theme().bold(&head), {
            let muted = self.fg(
                "muted",
                &format!("New version {version} is available. Run "),
            );
            let accent = self.fg("accent", "pi update");
            format!("{muted}{accent}")
        });
        self.io
            .view
            .container_add_text(ContainerId::Chat, &body, 1, 0, false);
        if let Some(note) = note.map(str::trim).filter(|n| !n.is_empty()) {
            self.io.view.container_add_spacer(ContainerId::Chat);
            self.io.view.container_add_markdown(
                ContainerId::Chat,
                note,
                1,
                0,
                &self.get_markdown_theme_with_settings(),
            );
            self.io.view.container_add_spacer(ContainerId::Chat);
        }
        let changelog_line = format!(
            "{}{}",
            self.fg("muted", "Changelog: "),
            self.fg("accent", "https://pi.dev/changelog")
        );
        self.io
            .view
            .container_add_text(ContainerId::Chat, &changelog_line, 1, 0, false);
        self.io
            .view
            .container_add_border(ContainerId::Chat, Some("warning"));
        self.io.view.request_render(None);
    }

    /// Upstream `showPackageUpdateNotification`.
    pub fn show_package_update_notification(&self, packages: &[String]) {
        self.io.view.container_add_spacer(ContainerId::Chat);
        self.io
            .view
            .container_add_border(ContainerId::Chat, Some("warning"));
        let head = self.fg("warning", "Package Updates Available");
        let body = format!(
            "{}\n{}{}\n{}\n{}",
            self.theme().bold(&head),
            self.fg("muted", "Package updates are available. Run "),
            self.fg("accent", "pi update --extensions"),
            self.fg("muted", "Packages:"),
            packages
                .iter()
                .map(|pkg| format!("- {pkg}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        self.io
            .view
            .container_add_text(ContainerId::Chat, &body, 1, 0, false);
        self.io
            .view
            .container_add_border(ContainerId::Chat, Some("warning"));
        self.io.view.request_render(None);
    }

    /// Upstream `checkForPackageUpdates`: the `PI_OFFLINE` gate, the npm
    /// probe through [`ShellIo::package_updates`], and the `displayName`
    /// projection (`catch` → no updates).
    pub fn check_for_package_updates(&self, offline: bool) -> Vec<String> {
        if offline {
            return Vec::new();
        }
        match self
            .io
            .package_updates
            .check_for_available_updates(&self.io.session_manager.cwd(), &self.io.host.agent_dir())
        {
            Ok(updates) => updates
                .into_iter()
                .map(|update| update.display_name)
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Upstream `checkTmuxKeyboardSetup` decision over probed tmux values.
    pub fn tmux_keyboard_warning(
        extended_keys: Option<&str>,
        extended_keys_format: Option<&str>,
    ) -> Option<String> {
        let extended_keys = extended_keys?;
        if extended_keys != "on" && extended_keys != "always" {
            return Some("tmux extended-keys is off. Modified Enter keys may not work. Add `set -g extended-keys on` to ~/.tmux.conf and restart tmux.".to_string());
        }
        if extended_keys_format == Some("xterm") {
            return Some("tmux extended-keys-format is xterm. Pi works best with csi-u. Add `set -g extended-keys-format csi-u` to ~/.tmux.conf and restart tmux.".to_string());
        }
        None
    }

    // -- queues ---------------------------------------------------------------

    /// Upstream `isExtensionCommand`.
    pub fn is_extension_command(&self, text: &str) -> bool {
        if !text.starts_with('/') {
            return false;
        }
        let rest = &text[1..];
        let command_name = match rest.split_once(' ') {
            Some((name, _)) => name,
            None => rest,
        };
        self.io.session.extensions().has_command(command_name)
    }

    /// Upstream `getAllQueuedMessages` (read-only merge of session queue and
    /// compaction queue, session messages first).
    pub fn get_all_queued_messages(&self) -> QueueSnapshot {
        let state = self.lock();
        QueueSnapshot {
            steering: [
                self.io.session.steering_messages(),
                state
                    .compaction_queued_messages
                    .iter()
                    .filter(|m| m.mode == QueueMode::Steer)
                    .map(|m| m.text.clone())
                    .collect(),
            ]
            .concat(),
            follow_up: [
                self.io.session.follow_up_messages(),
                state
                    .compaction_queued_messages
                    .iter()
                    .filter(|m| m.mode == QueueMode::FollowUp)
                    .map(|m| m.text.clone())
                    .collect(),
            ]
            .concat(),
        }
    }

    /// Upstream `clearAllQueues` (clearing merge; no display update).
    pub fn clear_all_queues(&self) -> QueueSnapshot {
        let (steering, follow_up) = self.io.session.clear_queue();
        let mut state = self.lock();
        let compaction_steering: Vec<String> = state
            .compaction_queued_messages
            .iter()
            .filter(|m| m.mode == QueueMode::Steer)
            .map(|m| m.text.clone())
            .collect();
        let compaction_follow_up: Vec<String> = state
            .compaction_queued_messages
            .iter()
            .filter(|m| m.mode == QueueMode::FollowUp)
            .map(|m| m.text.clone())
            .collect();
        state.compaction_queued_messages.clear();
        drop(state);
        QueueSnapshot {
            steering: [steering, compaction_steering].concat(),
            follow_up: [follow_up, compaction_follow_up].concat(),
        }
    }

    /// Upstream `updatePendingMessagesDisplay`.
    pub fn update_pending_messages_display(&self) {
        self.io.view.container_clear(ContainerId::PendingMessages);
        let snapshot = self.get_all_queued_messages();
        if snapshot.steering.is_empty() && snapshot.follow_up.is_empty() {
            return;
        }
        self.io
            .view
            .container_add_spacer(ContainerId::PendingMessages);
        let theme = self.theme();
        for message in &snapshot.steering {
            let text = theme
                .fg("dim", &format!("Steering: {message}"))
                .unwrap_or_default();
            self.io
                .view
                .container_add_text(ContainerId::PendingMessages, &text, 1, 0, true);
        }
        for message in &snapshot.follow_up {
            let text = theme
                .fg("dim", &format!("Follow-up: {message}"))
                .unwrap_or_default();
            self.io
                .view
                .container_add_text(ContainerId::PendingMessages, &text, 1, 0, true);
        }
        let dequeue_hint = self.get_app_key_display("app.message.dequeue");
        let hint_text = theme
            .fg(
                "dim",
                &format!("↳ {dequeue_hint} to edit all queued messages"),
            )
            .unwrap_or_default();
        self.io
            .view
            .container_add_text(ContainerId::PendingMessages, &hint_text, 1, 0, true);
    }

    /// Upstream `restoreQueuedMessagesToEditor`. `current_text` mirrors
    /// `options.currentText` (None = `this.editor.getText()`).
    pub fn restore_queued_messages_to_editor(
        &self,
        abort: bool,
        current_text: Option<&str>,
    ) -> usize {
        let snapshot = self.clear_all_queues();
        let all_queued: Vec<String> = [snapshot.steering, snapshot.follow_up].concat();
        if all_queued.is_empty() {
            self.update_pending_messages_display();
            if abort {
                self.io.session.abort();
            }
            return 0;
        }
        let queued_text = all_queued.join("\n\n");
        let current = current_text
            .map(str::to_string)
            .unwrap_or_else(|| self.editor().get_text());
        let combined_text = [queued_text, current]
            .into_iter()
            .filter(|t| !t.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        self.editor().set_text(&combined_text);
        self.update_pending_messages_display();
        if abort {
            self.io.session.abort();
        }
        all_queued.len()
    }

    /// Upstream `queueCompactionMessage`.
    pub fn queue_compaction_message(&self, text: &str, mode: QueueMode) {
        self.lock().compaction_queued_messages.push(
            super::interactive_mode::CompactionQueuedMessage {
                text: text.to_string(),
                mode,
            },
        );
        let editor = self.editor();
        editor.add_to_history(text);
        editor.set_text("");
        self.update_pending_messages_display();
        self.show_status("Queued message for after compaction");
    }

    /// Upstream `flushPendingBashComponents`.
    pub fn flush_pending_bash_components(&self) {
        let pending = std::mem::take(&mut self.lock().pending_bash_components);
        for component in pending {
            self.io
                .view
                .container_remove_component(ContainerId::PendingMessages, &component);
            self.io
                .view
                .container_add_component(ContainerId::Chat, &component);
        }
    }

    /// Routes one queued message: extension commands prompt immediately,
    /// everything else follows the message's queue mode.
    pub(crate) fn route_queued_message(
        &self,
        message: &super::interactive_mode::CompactionQueuedMessage,
    ) -> futures::future::BoxFuture<'_, Result<(), AgentSessionError>> {
        match (self.is_extension_command(&message.text), message.mode) {
            (true, _) => self.io.session.prompt(message.text.clone(), None),
            (false, QueueMode::FollowUp) => self.io.session.follow_up(message.text.clone()),
            (false, QueueMode::Steer) => self.io.session.steer(message.text.clone()),
        }
    }

    /// Upstream `flushCompactionQueue`. The first prompt's failure restores
    /// after the remaining messages are queued but before the trailing
    /// display update (the upstream microtask order captured by the oracle);
    /// other failures restore immediately without the trailing update.
    pub async fn flush_compaction_queue(&self, will_retry: bool) {
        let queued: Vec<super::interactive_mode::CompactionQueuedMessage> = {
            let mut state = self.lock();
            if state.compaction_queued_messages.is_empty() {
                return;
            }
            std::mem::take(&mut state.compaction_queued_messages)
        };
        let count = queued.len();
        self.update_pending_messages_display();

        let restore = |shell: &Self,
                       messages: Vec<super::interactive_mode::CompactionQueuedMessage>,
                       error: String| {
            shell.io.session.clear_queue();
            shell.lock().compaction_queued_messages = messages;
            shell.update_pending_messages_display();
            shell.show_error(&format!(
                "Failed to send queued message{}: {}",
                if count > 1 { "s" } else { "" },
                error
            ));
        };

        if will_retry {
            // When retry is pending, queue messages for the retry turn.
            for message in &queued {
                if let Err(error) = self.route_queued_message(message).await {
                    restore(self, queued, error.to_string());
                    return;
                }
            }
            self.update_pending_messages_display();
            return;
        }

        // Find first non-extension-command message to use as prompt.
        let first_prompt_index = queued
            .iter()
            .position(|message| !self.is_extension_command(&message.text));
        let Some(first_prompt_index) = first_prompt_index else {
            // All extension commands — execute them all.
            for message in &queued {
                if let Err(error) = self.io.session.prompt(message.text.clone(), None).await {
                    restore(self, queued, error.to_string());
                    return;
                }
            }
            return;
        };

        // Execute any extension commands before the first prompt.
        for message in &queued[..first_prompt_index] {
            if let Err(error) = self.io.session.prompt(message.text.clone(), None).await {
                restore(self, queued, error.to_string());
                return;
            }
        }

        // Start a prompt when idle, or queue it into a run still finishing
        // compaction; the first prompt's failure restores the whole queue.
        let first_prompt = &queued[first_prompt_index];
        let streaming = match first_prompt.mode {
            QueueMode::FollowUp => Some(StreamingDelivery::FollowUp),
            QueueMode::Steer => Some(StreamingDelivery::Steer),
        };
        let mut first_prompt_error: Option<String> = None;
        if let Err(error) = self
            .io
            .session
            .prompt(first_prompt.text.clone(), streaming)
            .await
        {
            first_prompt_error = Some(error.to_string());
        }

        // Queue remaining messages.
        for message in &queued[first_prompt_index + 1..] {
            if let Err(error) = self.route_queued_message(message).await {
                restore(self, queued, error.to_string());
                return;
            }
        }

        if let Some(error) = first_prompt_error {
            restore(self, queued.clone(), error);
        }
        self.update_pending_messages_display();
    }

    // -- follow-up / dequeue ---------------------------------------------------

    /// Upstream `handleFollowUp`.
    pub async fn handle_follow_up(&self) {
        let text = self.editor().get_expanded_text().trim().to_string();
        if text.is_empty() {
            return;
        }
        if self.io.session.is_compacting() {
            if self.is_extension_command(&text) {
                let editor = self.editor();
                editor.add_to_history(&text);
                editor.set_text("");
                let _ = self.io.session.prompt(text, None).await;
            } else {
                self.queue_compaction_message(&text, QueueMode::FollowUp);
            }
            return;
        }
        if self.io.session.is_streaming() {
            let editor = self.editor();
            editor.add_to_history(&text);
            editor.set_text("");
            let _ = self
                .io
                .session
                .prompt(text, Some(StreamingDelivery::FollowUp))
                .await;
            self.update_pending_messages_display();
            self.io.view.request_render(None);
        } else {
            // Not streaming: Alt+Enter acts like regular Enter (onSubmit).
            self.editor().set_text("");
            self.submit(&text).await;
        }
    }

    /// Upstream `handleDequeue`.
    pub fn handle_dequeue(&self) {
        let restored = self.restore_queued_messages_to_editor(false, None);
        if restored == 0 {
            self.show_status("No queued messages to restore");
        } else {
            self.show_status(&format!(
                "Restored {} queued message{} to editor",
                restored,
                if restored > 1 { "s" } else { "" }
            ));
        }
    }

    // -- input ring --------------------------------------------------------------

    /// Upstream `setupKeyHandlers` registration half: the editor seam records
    /// the app-action registrations in upstream order plus the escape /
    /// ctrl-d / change / paste-image / extension-shortcut handler slots.
    pub fn setup_key_handlers(&self) {
        for action in INPUT_RING_ACTIONS {
            self.io.default_editor.on_action(action);
        }
        self.io.default_editor.set_on_escape();
        self.io.default_editor.set_on_ctrl_d();
        self.io.default_editor.set_on_change();
        self.io.default_editor.set_on_paste_image();
        self.io.default_editor.set_on_extension_shortcut(false);
    }

    /// Upstream `setupEditorSubmitHandler` registration half.
    pub fn setup_editor_submit_handler(&self) {
        self.io.default_editor.set_on_submit();
    }

    /// The upstream `defaultEditor.onEscape` body.
    pub async fn on_escape_pressed(&self) {
        let text = self.editor().get_text();
        if self.io.session.is_streaming() {
            self.restore_queued_messages_to_editor(true, None);
        } else if self.io.session.is_bash_running() {
            self.io.session.abort_bash();
        } else {
            // Bind the flag read first — an `if`-condition self.lock()
            // temporary would hold the state mutex across the body.
            let is_bash_mode = self.lock().is_bash_mode;
            if is_bash_mode {
                self.editor().set_text("");
                self.lock().is_bash_mode = false;
                self.update_editor_border_color();
            } else if text.trim().is_empty() {
                // Double-escape with empty editor triggers /tree, /fork, or
                // nothing based on the setting.
                let action = self.io.settings.double_escape_action();
                if action != "none" {
                    let now = self.io.clock.now_ms();
                    let last = self.lock().last_escape_time;
                    if now - last < 500 {
                        if action == "tree" {
                            self.io.commands.run(ShellCommand::TreeSelector);
                        } else {
                            self.io.commands.run(ShellCommand::UserMessageSelector);
                        }
                        self.lock().last_escape_time = 0;
                    } else {
                        self.lock().last_escape_time = now;
                    }
                }
            }
        }
    }

    /// The upstream `handleCtrlC` body (500ms double-press window).
    pub async fn handle_ctrl_c(&self) -> CtrlCOutcome {
        let now = self.io.clock.now_ms();
        if now - self.lock().last_sigint_time < 500 {
            self.shutdown(false).await;
            CtrlCOutcome::Shutdown
        } else {
            self.clear_editor();
            self.lock().last_sigint_time = now;
            CtrlCOutcome::ClearEditor
        }
    }

    /// The upstream `handleCtrlD` body (only called with an empty editor).
    pub async fn handle_ctrl_d(&self) -> CtrlCOutcome {
        self.shutdown(false).await;
        CtrlCOutcome::Shutdown
    }

    /// The upstream `handleCtrlZ` decision. The posix suspend choreography
    /// (interval keep-alive, SIGINT ignore, SIGCONT restart) is an OS
    /// presentation seam (S7); win32 shows the status line.
    pub fn handle_ctrl_z(&self) -> SuspendDecision {
        if self.io.platform.is_windows() {
            self.show_status("Suspend to background is not supported on Windows");
            return SuspendDecision::Unsupported;
        }
        SuspendDecision::Suspend
    }

    // -- submit ladder -----------------------------------------------------------

    /// Upstream `setupEditorSubmitHandler`'s onSubmit body — the full command
    /// ladder in upstream order. The handler bodies dispatch through the
    /// [`CommandSink`] (S3; the real wiring invokes the shell bodies).
    pub async fn submit(&self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }

        // Command ladder (upstream order). The clear-editor placement is
        // per-command upstream (handler first for the selector-style
        // commands, editor clear first for the awaited-handlers).
        macro_rules! exact {
            ($prefix:expr, $command:expr) => {
                if text == $prefix {
                    self.editor().set_text("");
                    self.io.commands.run($command);
                    return;
                }
            };
        }
        /// Handler first, then the editor clear (upstream `/settings` shape).
        macro_rules! exact_handler_first {
            ($prefix:expr, $command:expr) => {
                if text == $prefix {
                    self.io.commands.run($command);
                    self.editor().set_text("");
                    return;
                }
            };
        }
        macro_rules! with_arg {
            ($prefix:expr, $command:expr) => {
                if text == $prefix || text.starts_with(concat!($prefix, " ")) {
                    let arg = text.strip_prefix(concat!($prefix, " ")).map(str::trim);
                    self.editor().set_text("");
                    self.io.commands.run($command(arg.map(str::to_string)));
                    return;
                }
            };
        }

        exact_handler_first!("/settings", ShellCommand::Settings);
        exact!("/scoped-models", ShellCommand::ScopedModels);
        with_arg!("/model", ShellCommand::Model);
        with_arg!("/thinking", ShellCommand::Thinking);

        if text == "/export" || text.starts_with("/export ") {
            self.io.commands.run(ShellCommand::Export(text.to_string()));
            self.editor().set_text("");
            return;
        }
        if text == "/import" || text.starts_with("/import ") {
            self.io.commands.run(ShellCommand::Import(text.to_string()));
            self.editor().set_text("");
            return;
        }
        exact_handler_first!("/share", ShellCommand::Share);
        if text == "/bug" || text.starts_with("/bug ") {
            let hint = text
                .strip_prefix("/bug")
                .map(str::trim)
                .filter(|h| !h.is_empty());
            self.editor().set_text("");
            self.io
                .commands
                .run(ShellCommand::Bug(hint.map(str::to_string)));
            return;
        }
        exact_handler_first!(
            "/copy",
            ShellCommand::Copy {
                flash_confirmation: false,
                prefer_selection: false
            }
        );
        if text == "/name" || text.starts_with("/name ") {
            self.io.commands.run(ShellCommand::Name(text.to_string()));
            self.editor().set_text("");
            return;
        }
        exact_handler_first!("/session", ShellCommand::Session);
        exact_handler_first!("/changelog", ShellCommand::Changelog);
        exact_handler_first!("/hotkeys", ShellCommand::Hotkeys);
        exact_handler_first!("/fork", ShellCommand::UserMessageSelector);
        exact!("/clone", ShellCommand::Clone);
        exact_handler_first!("/tree", ShellCommand::TreeSelector);
        exact_handler_first!("/trust", ShellCommand::Trust);
        if text == "/login" || text.starts_with("/login ") {
            let provider_ref = text.strip_prefix("/login ").map(str::trim);
            self.editor().set_text("");
            self.io
                .commands
                .run(ShellCommand::Login(provider_ref.map(str::to_string)));
            return;
        }
        exact_handler_first!("/logout", ShellCommand::OAuthLogout);
        exact!("/new", ShellCommand::Clear);
        if text == "/compact" || text.starts_with("/compact ") {
            let custom = text.strip_prefix("/compact ").map(str::trim);
            self.editor().set_text("");
            self.io
                .commands
                .run(ShellCommand::Compact(custom.map(str::to_string)));
            return;
        }
        exact!("/reload", ShellCommand::Reload);
        exact_handler_first!("/debug", ShellCommand::Debug);
        exact_handler_first!("/arminsayshi", ShellCommand::ArminSaysHi);
        exact_handler_first!("/dementedelves", ShellCommand::DementedDelves);
        exact_handler_first!("/resume", ShellCommand::SessionSelector);
        if text == "/quit" {
            self.editor().set_text("");
            self.shutdown(false).await;
            return;
        }

        // Bash command (! for normal, !! for excluded from context).
        if let Some(rest) = text.strip_prefix('!') {
            let is_excluded = text.starts_with("!!");
            let command = if is_excluded {
                rest.strip_prefix('!').map(str::trim)
            } else {
                Some(rest.trim())
            };
            if let Some(command) = command.filter(|c| !c.is_empty()) {
                if self.io.session.is_bash_running() {
                    self.show_warning(
                        "A bash command is already running. Press Esc to cancel it first.",
                    );
                    self.editor().set_text(text);
                    return;
                }
                self.editor().add_to_history(text);
                self.io.commands.run(ShellCommand::Bash {
                    command: command.to_string(),
                    exclude_from_context: is_excluded,
                });
                self.lock().is_bash_mode = false;
                self.update_editor_border_color();
                return;
            }
        }

        // Queue input during compaction (extension commands execute
        // immediately).
        if self.io.session.is_compacting() {
            if self.is_extension_command(text) {
                let editor = self.editor();
                editor.add_to_history(text);
                editor.set_text("");
                let _ = self.io.session.prompt(text.to_string(), None).await;
            } else {
                self.queue_compaction_message(text, QueueMode::Steer);
            }
            return;
        }

        // If streaming, use prompt() with steer behavior (extension commands,
        // prompt template expansion, and queueing all flow through prompt()).
        if self.io.session.is_streaming() {
            let editor = self.editor();
            editor.add_to_history(text);
            editor.set_text("");
            let _ = self
                .io
                .session
                .prompt(text.to_string(), Some(StreamingDelivery::Steer))
                .await;
            self.update_pending_messages_display();
            self.io.view.request_render(None);
            return;
        }

        // Normal message submission: first move any pending bash components
        // to chat, then hand the text to the waiting input loop.
        self.flush_pending_bash_components();

        {
            let mut state = self.lock();
            if let Some(slot) = state.on_input_callback.take() {
                drop(state);
                self.io.view.user_input_resolved(slot.0, text);
                self.editor().add_to_history(text);
                return;
            }
            state.pending_user_inputs.push(text.to_string());
        }
        self.editor().add_to_history(text);
    }

    /// Upstream `handleStartupSubmit`.
    pub fn handle_startup_submit(&self, text: &str) {
        self.editor().set_text(text);
        self.show_status("Startup is still in progress");
    }

    /// Upstream `getUserInput`: queued inputs drain first; otherwise the
    /// first caller installs the onInput callback and waits.
    pub fn get_user_input(&self) -> Option<String> {
        let mut state = self.lock();
        if !state.pending_user_inputs.is_empty() {
            return Some(state.pending_user_inputs.remove(0));
        }
        if state.on_input_callback.is_none() {
            state.on_input_callback = Some(InputSlot(self.next_id()));
        }
        None
    }

    /// Whether a `getUserInput` waiter is pending (upstream: the promise is
    /// unresolved until the editor submits).
    pub fn has_input_waiter(&self) -> bool {
        self.lock().on_input_callback.is_some()
    }

    // -- editor border / cycles / toggles -----------------------------------------

    /// Upstream `updateEditorBorderColor`.
    pub fn update_editor_border_color(&self) {
        let border = if self.lock().is_bash_mode {
            EditorBorder::BashMode
        } else {
            self.current_border()
        };
        self.editor().set_border_color(border);
        if let Some((_, indicator)) = self.lock().active_status_indicator.clone() {
            self.io
                .view
                .update_component(&indicator, "invalidate", Value::Null);
        }
        self.io.view.request_render(None);
    }

    /// Upstream `cycleThinkingLevel`.
    pub fn cycle_thinking_level(&self) {
        match self.io.session.cycle_thinking_level() {
            None => self.show_status("Current model does not support thinking"),
            Some(new_level) => {
                self.ev(json!(["footer.invalidate"]));
                self.update_editor_border_color();
                self.show_status(&format!(
                    "Thinking level: {}",
                    thinking_level_lower(&new_level)
                ));
            }
        }
    }

    /// Upstream `cycleModel`.
    pub async fn cycle_model(&self, direction: CycleDirection) {
        match self.io.session.cycle_model(direction).await {
            Err(error) => {
                self.show_error(&error.to_string());
            }
            Ok(None) => {
                let message = if !self.io.session.scoped_models().is_empty() {
                    "Only one model in scope"
                } else {
                    "Only one model available"
                };
                self.show_status(message);
            }
            Ok(Some(result)) => {
                self.ev(json!(["footer.invalidate"]));
                self.update_editor_border_color();
                let thinking =
                    if result.model.reasoning && result.thinking_level != ThinkingLevel::Off {
                        format!(
                            " (thinking: {})",
                            thinking_level_lower(&result.thinking_level)
                        )
                    } else {
                        String::new()
                    };
                let name = if result.model.name.is_empty() {
                    result.model.id.clone()
                } else {
                    result.model.name.clone()
                };
                self.show_status(&format!("Switched to {name}{thinking}"));
                self.io
                    .session
                    .maybe_warn_anthropic_subscription_auth(Some(result.model.provider.as_str()));
            }
        }
    }

    /// Upstream `toggleToolOutputExpansion`.
    pub fn toggle_tool_output_expansion(&self) {
        let expanded = !self.lock().tool_output_expanded;
        self.set_tools_expanded(expanded);
    }

    /// Upstream `setToolsExpanded`.
    pub fn set_tools_expanded(&self, expanded: bool) {
        if expanded == self.lock().tool_output_expanded {
            return;
        }
        self.lock().tool_output_expanded = expanded;
        // Bind both header reads under one acquisition — an `.or()` inline
        // self.lock() temporary would hold the state mutex across the second
        // lock (upstream reads these synchronously).
        let active_header = {
            let state = self.lock();
            state
                .custom_header
                .clone()
                .or(state.built_in_header.clone())
        };
        if let Some(header) = active_header {
            self.io
                .view
                .update_component(&header, "setExpanded", Value::Bool(expanded));
        }
        for container in [ContainerId::LoadedResources, ContainerId::Chat] {
            for component in self.io.view.container_components(container) {
                self.io
                    .view
                    .update_component(&component, "setExpanded", Value::Bool(expanded));
            }
        }
        self.show_status(&format!(
            "Tool output: {}",
            if expanded { "expanded" } else { "collapsed" }
        ));
    }

    /// Upstream `updateThinkingBlockVisibility`.
    pub fn update_thinking_block_visibility(&self) {
        let hide = self.lock().hide_thinking_block;
        for component in self.io.view.container_components(ContainerId::Chat) {
            if component.kind == "AssistantMessageComponent" {
                self.io.view.update_component(
                    &component,
                    "setHideThinkingBlock",
                    Value::Bool(hide),
                );
            }
        }
        self.io.view.request_render(None);
    }

    /// Upstream `toggleThinkingBlockVisibility`.
    pub fn toggle_thinking_block_visibility(&self) {
        let hide = !self.lock().hide_thinking_block;
        self.lock().hide_thinking_block = hide;
        self.io.settings.set_hide_thinking_block(hide);
        self.update_thinking_block_visibility();
        self.show_status(&format!(
            "Thinking blocks: {}",
            if hide { "hidden" } else { "visible" }
        ));
    }

    /// Upstream `setHiddenThinkingLabel`.
    pub fn set_hidden_thinking_label(&self, label: Option<&str>) {
        let label = label
            .map(str::to_string)
            .unwrap_or_else(|| "Thinking...".to_string());
        {
            let mut state = self.lock();
            state.hidden_thinking_label = Some(label.clone());
        }
        for component in self.io.view.container_components(ContainerId::Chat) {
            if component.kind == "AssistantMessageComponent" {
                self.io.view.update_component(
                    &component,
                    "setHiddenThinkingLabel",
                    Value::String(label.clone()),
                );
            }
        }
        if let Some(streaming) = self.lock().streaming_component.clone() {
            self.io.view.update_component(
                &streaming,
                "setHiddenThinkingLabel",
                Value::String(label),
            );
        }
        self.io.view.request_render(None);
    }

    // -- status indicators ----------------------------------------------------------

    /// Upstream `setExtensionStatus`.
    pub fn set_extension_status(&self, key: &str, text: Option<&str>) {
        // The cleared status is JS `undefined` — the recorded tuple carries
        // the harness `"undefined"` sentinel.
        self.ev(json!([
            "footerDataProvider.setExtensionStatus",
            key,
            text.map(Value::from).unwrap_or(json!("undefined"))
        ]));
        self.io.view.request_render(None);
    }

    /// Upstream `setEditorWorkingStatusIndicator` — returns whether the
    /// editor embedded the indicator.
    pub(crate) fn set_editor_working_status_indicator(
        &self,
        indicator: Option<&ComponentRef>,
    ) -> bool {
        self.io.default_editor.set_working_status_indicator(None);
        if !self.editor().embeds_working_status() {
            return false;
        }
        self.editor()
            .set_working_status_indicator(indicator.cloned());
        true
    }

    /// Upstream `showStatusIndicator`.
    pub fn show_status_indicator(&self, indicator: ComponentRef, kind: &str) {
        if let Some((_, previous)) = self.lock().active_status_indicator.clone() {
            self.io
                .view
                .update_component(&previous, "dispose", Value::Null);
        }
        {
            let mut state = self.lock();
            state.active_status_indicator = Some((kind.to_string(), indicator.clone()));
            state.active_working_indicator_embedded = false;
        }
        self.io.view.container_clear(ContainerId::Status);
        self.set_editor_working_status_indicator(None);
        if self.set_editor_working_status_indicator(Some(&indicator)) {
            self.lock().active_working_indicator_embedded = true;
            return;
        }
        self.io
            .view
            .container_add_component(ContainerId::Status, &indicator);
    }

    /// Upstream `clearStatusIndicator`.
    pub fn clear_status_indicator(&self, kind: Option<&str>) {
        {
            let state = self.lock();
            if let Some(kind) = kind {
                if state
                    .active_status_indicator
                    .as_ref()
                    .map(|(k, _)| k.as_str())
                    != Some(kind)
                {
                    return;
                }
            }
        }
        let cleared = self.lock().active_status_indicator.clone();
        let was_embedded = self.lock().active_working_indicator_embedded;
        if let Some((_, indicator)) = &cleared {
            self.io
                .view
                .update_component(indicator, "dispose", Value::Null);
        }
        {
            let mut state = self.lock();
            state.active_status_indicator = None;
            state.active_working_indicator_embedded = false;
        }
        self.io.view.container_clear(ContainerId::Status);
        self.set_editor_working_status_indicator(None);
        if cleared.is_some()
            && !was_embedded
            && self.options().tui_mode.as_deref() == Some("regular")
            && self.io.view.get_clear_on_shrink()
        {
            let idle = self.io.view.idle_status_component();
            self.io
                .view
                .container_add_component(ContainerId::Status, &idle);
        }
    }

    /// Upstream `showWorkingStatusIndicator`.
    pub fn show_working_status_indicator(&self) {
        let working_message = {
            let state = self.lock();
            state
                .working_message
                .clone()
                .unwrap_or_else(|| "Working".to_string())
        };
        let options = self.lock().working_indicator_options.clone();
        // Upstream ctor: `new WorkingStatusIndicator(this.ui, message,
        // options, colorFn)` — the ui handle rides the `__describe` marker;
        // the color fn exists only when the editor embeds working status.
        let color_fn = if self.editor().embeds_working_status() {
            Value::String("function".to_string())
        } else {
            Value::Null
        };
        let args = serde_json::json!([
            { "__describe": "ui" },
            working_message,
            options.unwrap_or(Value::Null),
            color_fn,
        ]);
        let indicator = self
            .io
            .view
            .new_component(ComponentKind::WorkingStatusIndicator, args);
        self.show_status_indicator(indicator, "working");
    }

    /// Upstream `setWorkingVisible`.
    pub fn set_working_visible(&self, visible: bool) {
        self.lock().working_visible = visible;
        if !visible {
            self.clear_status_indicator(Some("working"));
            self.io.view.request_render(None);
            return;
        }
        if self.io.session.is_streaming() {
            let kind = self
                .lock()
                .active_status_indicator
                .as_ref()
                .map(|(kind, _)| kind.clone());
            if kind.as_deref() != Some("working") {
                self.show_working_status_indicator();
            }
        }
        self.io.view.request_render(None);
    }

    /// Upstream `setWorkingIndicator`.
    pub fn set_working_indicator(&self, options: Option<Value>) {
        self.lock().working_indicator_options = options.clone();
        let kind = self
            .lock()
            .active_status_indicator
            .as_ref()
            .map(|(kind, _)| kind.clone());
        if kind.as_deref() == Some("working") {
            let (_, indicator) = self
                .lock()
                .active_status_indicator
                .clone()
                .expect("working");
            self.io.view.update_component(
                &indicator,
                "setIndicator",
                options.unwrap_or(Value::Null),
            );
        }
        self.io.view.request_render(None);
    }

    // -- clipboard paste ------------------------------------------------------------

    /// Upstream `handleClipboardPaste`. The image branch's tmp-file write is
    /// the fs/os/crypto presentation seam (S9); the port inserts the joined
    /// path with a shell-minted id (oracle-unverified branch).
    pub async fn handle_clipboard_paste(&self) {
        // Handle clipboard paste (triggered on Ctrl+V). Copied files use
        // their original paths, images are attached via temporary files, and
        // plain text is the final fallback.
        let paste = self.paste_clipboard_payload().await;
        match paste {
            Ok(()) => {}
            Err(message) => self.show_error(&format!("Failed to paste from clipboard: {message}")),
        }
    }

    /// The [`Self::handle_clipboard_paste`] body; `Err` carries the upstream
    /// thrown-error message (surfaced through `showError`).
    async fn paste_clipboard_payload(&self) -> Result<(), String> {
        if let Some(file_paths) = self.io.platform.read_clipboard_file_paths().await {
            // Upstream throws on control characters in any path.
            if file_paths
                .iter()
                .any(|path| path.chars().any(char::is_control))
            {
                return Err("Clipboard file path contains control characters".to_string());
            }
            let is_bash_mode = self.lock().is_bash_mode;
            let paths = if is_bash_mode {
                file_paths
                    .iter()
                    .map(|path| super::interactive_mode::quote_if_needed(path))
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                file_paths.join("\n")
            };
            // Upstream pads the insertion against the non-whitespace
            // characters around the cursor.
            let cursor = self.editor().get_cursor();
            let text = self.editor().get_text();
            let current_line = cursor
                .as_ref()
                .and_then(|(line, _)| text.split('\n').nth(*line))
                .unwrap_or("");
            let (line_idx, col) = cursor.unwrap_or((0, 0));
            let _ = line_idx;
            let character_before_cursor = if col > 0 {
                current_line.chars().nth(col - 1)
            } else {
                None
            };
            let character_after_cursor = current_line.chars().nth(col);
            let leading_space = match character_before_cursor {
                Some(c) if !c.is_whitespace() => " ",
                _ => "",
            };
            let trailing_space = match character_after_cursor {
                Some(c) if !c.is_whitespace() => " ",
                _ => "",
            };
            self.editor()
                .insert_text_at_cursor(&format!("{leading_space}{paths}{trailing_space}"));
            self.io.view.request_render(None);
            return Ok(());
        }

        if let Some((mime, _bytes)) = self.io.platform.read_clipboard_image().await {
            let extension = match mime.as_str() {
                "image/jpeg" => "jpg",
                "image/gif" => "gif",
                "image/webp" => "webp",
                _ => "png",
            };
            let file_name = format!("pi-clipboard-{}.{}", self.next_id(), extension);
            let file_path = self.io.platform.join_path(&["/tmp", &file_name]);
            self.editor().insert_text_at_cursor(&file_path);
            self.io.view.request_render(None);
            return Ok(());
        }
        if let Some(text) = self.io.platform.read_clipboard_text().await {
            self.editor().insert_text_at_cursor(&text);
            self.io.view.request_render(None);
        }
        Ok(())
    }

    /// Upstream `handleRightClickPaste`: bracketed-paste the clipboard into
    /// the focused renderer component's `handleInput`.
    pub async fn handle_right_click_paste(&self) {
        let Some(target) = self.io.view.renderer_focused_component() else {
            return;
        };
        let Some(text) = self.io.platform.read_clipboard_text().await else {
            return;
        };
        if self.io.view.renderer_focused_component().as_ref() != Some(&target) {
            return;
        }
        if self
            .io
            .view
            .component_handle_input(&target, &format!("\x1b[200~{text}\x1b[201~"))
        {
            self.io.view.request_render(None);
        }
    }

    // -- markdown / transformers / tool definitions -------------------------------

    /// Upstream `getMarkdownThemeWithSettings`.
    pub fn get_markdown_theme_with_settings(&self) -> Value {
        serde_json::json!({
            "tag": "markdownTheme",
            "codeBlockIndent": self.io.settings.code_block_indent(),
        })
    }

    /// Upstream `getMarkdownTransformers`: the mermaid transformer first, then
    /// the extension transformers.
    pub fn get_markdown_transformers(&self) -> Vec<Value> {
        let mut transformers = vec![serde_json::json!({ "tag": "mermaid" })];
        transformers.extend(
            self.io
                .session
                .extensions()
                .markdown_transformers()
                .into_iter()
                .map(|tag| serde_json::json!({ "tag": tag })),
        );
        transformers
    }

    // -- event → UI bridge -----------------------------------------------------------

    /// Upstream `subscribeToAgent`.
    pub fn subscribe_to_agent(&self) {
        let slot = SubscriptionSlot(self.io.session.subscribe());
        self.lock().unsubscribe = Some(slot);
    }

    /// Upstream `handleEvent` — the full AgentSessionEvent switch. Events
    /// arriving before initialization trigger init first (modeled through the
    /// [`CommandSink`] as `cmd.init`).
    pub async fn handle_event(&self, event: &AgentSessionEvent) {
        if !self.lock().is_initialized {
            self.io.commands.run(ShellCommand::Init);
        }

        self.ev(json!(["footer.invalidate"]));

        match event {
            AgentSessionEvent::AgentStart => {
                self.lock().pending_tools.clear();
                // Restore main escape handler if retry handler is still
                // active (retry success event fires later, but we need the
                // main handler now).
                // Bind the flag read first — an `if`-condition self.lock()
                // temporary would hold the state mutex across the body.
                let retry_handler_active = self.lock().retry_escape_handler_active;
                if retry_handler_active {
                    self.io.default_editor.set_on_escape();
                    self.lock().retry_escape_handler_active = false;
                }
            }
            AgentSessionEvent::TurnStart => {
                if self.io.settings.show_terminal_progress() {
                    self.ev(json!(["terminal.setProgress", true]));
                }
                let working_visible = self.lock().working_visible;
                if working_visible {
                    let kind = self
                        .lock()
                        .active_status_indicator
                        .as_ref()
                        .map(|(kind, _)| kind.clone());
                    if kind.as_deref() != Some("working") {
                        self.show_working_status_indicator();
                    }
                } else {
                    self.clear_status_indicator(None);
                }
                self.io.view.request_render(None);
            }
            AgentSessionEvent::QueueUpdate { .. } => {
                self.update_pending_messages_display();
                self.io.view.request_render(None);
            }
            AgentSessionEvent::EntryAppended { entry } => {
                // Upstream skips entries the boundary-compaction re-render
                // already put on screen (`delete(event.entry.id)`).
                let entry_id = entry.id().map(str::to_string);
                if entry_id.as_deref().is_some_and(|id| {
                    self.lock()
                        .entries_rendered_by_boundary_compaction
                        .remove(id)
                }) {
                    return;
                }
                match entry {
                    SessionEntry::Custom(_) => {
                        self.add_custom_entry_to_chat(entry);
                        self.io.view.request_render(None);
                    }
                    SessionEntry::Usage(usage) if usage.kind == "cache_warm" => {
                        self.add_cache_warming_usage(usage);
                        self.io.view.request_render(None);
                    }
                    SessionEntry::CustomMessage(custom) if custom.display => {
                        // Upstream `addMessageToChat(createCustomMessage(...))`;
                        // the session-manager projection is exactly that
                        // createCustomMessage call.
                        for message in
                            crate::coding_agent::session_manager::session_entry_to_context_messages(
                                entry,
                            )
                        {
                            self.add_message_to_chat(&message, false);
                        }
                        self.io.view.request_render(None);
                    }
                    SessionEntry::Compaction(compaction) => {
                        if let Some(entry_id) = entry_id {
                            self.render_boundary_compaction(entry_id, compaction);
                        }
                    }
                    _ => {}
                }
            }
            AgentSessionEvent::SessionInfoChanged { .. } => {
                self.update_terminal_title();
                self.ev(json!(["footer.invalidate"]));
                self.io.view.request_render(None);
            }
            AgentSessionEvent::ThinkingLevelChanged { .. } => {
                self.ev(json!(["footer.invalidate"]));
                self.update_editor_border_color();
            }
            AgentSessionEvent::MessageStart { message } => {
                let role = message.role().to_string();
                match role.as_str() {
                    "custom" => {
                        self.add_message_to_chat(message, false);
                        self.io.view.request_render(None);
                    }
                    "user" => {
                        self.add_message_to_chat(message, false);
                        self.update_pending_messages_display();
                        self.io.view.request_render(None);
                    }
                    "assistant" => {
                        // Bind the state reads first — a json! inline
                        // self.lock() temporary would hold the state mutex
                        // across the later locks.
                        let (hide_thinking_block, hidden_thinking_label, output_pad) = {
                            let state = self.lock();
                            (
                                state.hide_thinking_block,
                                state.hidden_thinking_label.clone(),
                                state.output_pad,
                            )
                        };
                        let args = serde_json::json!([
                            Value::Null,
                            hide_thinking_block,
                            self.get_markdown_theme_with_settings(),
                            hidden_thinking_label,
                            output_pad,
                            self.get_markdown_transformers(),
                        ]);
                        let component = self
                            .io
                            .view
                            .new_component(ComponentKind::AssistantMessage, args);
                        self.io
                            .view
                            .container_add_component(ContainerId::Chat, &component);
                        self.lock().streaming_component = Some(component.clone());
                        self.lock().streaming_message = Some(message.clone());
                        self.io.view.update_component(
                            &component,
                            "updateContent",
                            serde_json::json!([message, true]),
                        );
                        self.io.view.request_render(None);
                    }
                    _ => {}
                }
            }
            AgentSessionEvent::MessageUpdate { message, .. } => {
                let streaming = self.lock().streaming_component.clone();
                if let (Some(component), "assistant") = (streaming.as_ref(), message.role()) {
                    self.lock().streaming_message = Some(message.clone());
                    self.io.view.update_component(
                        component,
                        "updateContent",
                        serde_json::json!([message, true]),
                    );

                    for (tool_id, tool_name, tool_args) in assistant_tool_calls(message) {
                        let existing = self.lock().pending_tools.get(&tool_id).cloned();
                        match existing {
                            None => {
                                let args = serde_json::json!([
                                    tool_name,
                                    tool_id,
                                    tool_args,
                                    {
                                        "showImages": self.io.settings.show_images(),
                                        "imageWidthCells": self.io.settings.image_width_cells(),
                                    },
                                    {
                                        "name": &tool_name,
                                        "def": self.io.session.tool_definition(&tool_name),
                                        "via": "builtInRenderers",
                                    },
                                    { "__describe": "ui" },
                                    self.io.session_manager.cwd(),
                                ]);
                                let component = self
                                    .io
                                    .view
                                    .new_component(ComponentKind::ToolExecution, args);
                                self.io.view.update_component(
                                    &component,
                                    "setExpanded",
                                    Value::Bool(self.lock().tool_output_expanded),
                                );
                                self.io
                                    .view
                                    .container_add_component(ContainerId::Chat, &component);
                                self.lock().pending_tools.insert(tool_id, component);
                            }
                            Some(component) => {
                                self.io
                                    .view
                                    .update_component(&component, "updateArgs", tool_args);
                            }
                        }
                    }
                    self.io.view.request_render(None);
                }
            }
            AgentSessionEvent::MessageEnd { message } => {
                if message.role() == "user" {
                    // user messages are rendered on message_start
                } else {
                    let streaming = self.lock().streaming_component.clone();
                    if let (Some(component), "assistant") = (streaming.as_ref(), message.role()) {
                        self.lock().streaming_message = Some(message.clone());
                        let stop_reason = Self::message_stop_reason(message);
                        let mut error_message: Option<String> = None;
                        if stop_reason.as_deref() == Some("aborted") {
                            let retry_attempt = self.io.session.retry_attempt();
                            error_message = Some(if retry_attempt > 0 {
                                format!(
                                    "Aborted after {} retry attempt{}",
                                    retry_attempt,
                                    if retry_attempt > 1 { "s" } else { "" }
                                )
                            } else {
                                "Operation aborted".to_string()
                            });
                        }
                        if let (Some(error), Some(streaming_message)) = (
                            error_message.as_ref(),
                            self.lock().streaming_message.as_mut(),
                        ) {
                            assistant_set_error_message(streaming_message, error);
                        }
                        // Upstream mutates event.message in place; the record
                        // carries the mutated object.
                        let recorded_message = self
                            .lock()
                            .streaming_message
                            .clone()
                            .unwrap_or_else(|| message.clone());
                        self.io.view.update_component(
                            component,
                            "updateContent",
                            serde_json::json!([recorded_message, false]),
                        );

                        if stop_reason.as_deref() == Some("aborted")
                            || stop_reason.as_deref() == Some("error")
                        {
                            let error_text = error_message.unwrap_or_else(|| {
                                assistant_error_message(message)
                                    .unwrap_or_else(|| "Error".to_string())
                            });
                            let pending: Vec<ComponentRef> =
                                self.lock().pending_tools.values().cloned().collect();
                            for pending_component in &pending {
                                self.io.view.update_component(
                                    pending_component,
                                    "updateResult",
                                    serde_json::json!([
                                        { "content": [{ "type": "text", "text": error_text }], "isError": true },
                                        false,
                                    ]),
                                );
                            }
                            self.lock().pending_tools.clear();
                            // Upstream `maybeSuggestBugReport(this.streamingMessage)`.
                            let streaming_message = self.lock().streaming_message.clone();
                            if let Some(streaming_message) = streaming_message {
                                self.maybe_suggest_bug_report(&streaming_message);
                            }
                        } else {
                            // Args are now complete — trigger diff computation
                            // for edit tools.
                            let pending: Vec<ComponentRef> =
                                self.lock().pending_tools.values().cloned().collect();
                            for pending_component in &pending {
                                self.io.view.update_component(
                                    pending_component,
                                    "setArgsComplete",
                                    Value::Null,
                                );
                            }
                            self.maybe_show_thinking_drop_notice(message);
                            self.maybe_show_cache_miss_notice(message);
                        }
                        self.lock().streaming_component = None;
                        self.lock().streaming_message = None;
                        self.ev(json!(["footer.invalidate"]));
                    }
                    self.io.view.request_render(None);
                }
            }
            AgentSessionEvent::TurnEnd { .. } => {
                // Turn-end rendering happens through message_end.
            }
            AgentSessionEvent::BashExecutionUpdate { .. } => {
                // The bash execution callback handles TUI output rendering.
            }
            AgentSessionEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            } => {
                let existing = self.lock().pending_tools.get(tool_call_id).cloned();
                let component = match existing {
                    Some(component) => component,
                    None => {
                        let ctor = serde_json::json!([
                            tool_name,
                            tool_call_id,
                            args,
                            {
                                "showImages": self.io.settings.show_images(),
                                "imageWidthCells": self.io.settings.image_width_cells(),
                            },
                            {
                                "name": tool_name,
                                "def": self.io.session.tool_definition(tool_name),
                                "via": "builtInRenderers",
                            },
                            { "__describe": "ui" },
                            self.io.session_manager.cwd(),
                        ]);
                        let component = self
                            .io
                            .view
                            .new_component(ComponentKind::ToolExecution, ctor);
                        self.io.view.update_component(
                            &component,
                            "setExpanded",
                            Value::Bool(self.lock().tool_output_expanded),
                        );
                        self.io
                            .view
                            .container_add_component(ContainerId::Chat, &component);
                        self.lock()
                            .pending_tools
                            .insert(tool_call_id.clone(), component.clone());
                        component
                    }
                };
                self.io
                    .view
                    .update_component(&component, "markExecutionStarted", Value::Null);
                self.io.view.request_render(None);
            }
            AgentSessionEvent::ToolExecutionUpdate {
                tool_call_id,
                partial_result,
                ..
            } => {
                let pending = self.lock().pending_tools.get(tool_call_id).cloned();
                if let Some(component) = pending {
                    // Upstream spreads `{...partialResult, isError: false}`.
                    let mut projected = match partial_result.clone() {
                        Value::Object(map) => map,
                        other => {
                            let mut map = serde_json::Map::new();
                            map.insert("value".to_string(), other);
                            map
                        }
                    };
                    projected.insert("isError".to_string(), Value::Bool(false));
                    self.io.view.update_component(
                        &component,
                        "updateResult",
                        serde_json::json!([Value::Object(projected), true]),
                    );
                    self.io.view.request_render(None);
                }
            }
            AgentSessionEvent::ToolExecutionEnd {
                tool_call_id,
                result,
                is_error,
                ..
            } => {
                let pending = self.lock().pending_tools.get(tool_call_id).cloned();
                if let Some(component) = pending {
                    // Upstream spreads `{...result, isError: event.isError}`.
                    let mut projected = match result.clone() {
                        Value::Object(map) => map,
                        other => {
                            let mut map = serde_json::Map::new();
                            map.insert("value".to_string(), other);
                            map
                        }
                    };
                    projected.insert("isError".to_string(), Value::Bool(*is_error));
                    self.io.view.update_component(
                        &component,
                        "updateResult",
                        serde_json::json!([Value::Object(projected)]),
                    );
                    self.lock().pending_tools.remove(tool_call_id);
                    self.io.view.request_render(None);
                }
            }
            AgentSessionEvent::AgentEnd { .. } => {
                if self.io.settings.show_terminal_progress() {
                    self.ev(json!(["terminal.setProgress", false]));
                }
                self.clear_status_indicator(Some("working"));
                let streaming = self.lock().streaming_component.clone();
                if let Some(streaming) = streaming {
                    self.io
                        .view
                        .container_remove_component(ContainerId::Chat, &streaming);
                    let mut state = self.lock();
                    state.streaming_component = None;
                    state.streaming_message = None;
                }
                self.lock().pending_tools.clear();
                self.io.view.request_render(None);
            }
            AgentSessionEvent::AgentSettled => {
                self.check_shutdown_requested().await;
            }
            AgentSessionEvent::CompactionStart { reason } => {
                if self.io.settings.show_terminal_progress() {
                    self.ev(json!(["terminal.setProgress", true]));
                }
                // Keep editor active; submissions are queued during compaction.
                self.lock().auto_compaction_escape_handler_active = true;
                self.io.default_editor.set_on_escape();
                let args = serde_json::json!([{ "__describe": "ui" }, reason_str(*reason)]);
                let indicator = self
                    .io
                    .view
                    .new_component(ComponentKind::CompactionStatusIndicator, args);
                self.show_status_indicator(indicator, "compaction");
                self.io.view.request_render(None);
            }
            AgentSessionEvent::CompactionEnd {
                result,
                aborted,
                reason,
                error_message,
                will_retry,
            } => {
                if self.io.settings.show_terminal_progress() {
                    self.ev(json!(["terminal.setProgress", false]));
                }
                // Bind the flag read first — an `if`-condition self.lock()
                // temporary would hold the state mutex across the body.
                let escape_handler_active = self.lock().auto_compaction_escape_handler_active;
                if escape_handler_active {
                    self.lock().auto_compaction_escape_handler_active = false;
                    self.io.default_editor.set_on_escape();
                }
                self.clear_status_indicator(Some("compaction"));
                if *aborted {
                    if *reason == CompactionReason::Manual {
                        self.show_error("Compaction cancelled");
                    } else {
                        self.show_status("Auto-compaction cancelled");
                    }
                } else if let Some(result) = result {
                    let entries = self.io.session_manager.build_context_entries();
                    if !matches!(entries.first(), Some(SessionEntry::Compaction(_))) {
                        panic!("Completed compaction is missing from the session context");
                    }
                    self.io.view.container_clear(ContainerId::Chat);
                    // The latest compaction is prepended for model context;
                    // append it below at its chronological position.
                    self.render_session_entries(&entries[1..], false, false);
                    let summary_message = create_summary_custom_message(
                        "compactionSummary",
                        result
                            .get("summary")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                        result
                            .get("tokensBefore")
                            .and_then(Value::as_i64)
                            .unwrap_or_default(),
                        self.io.clock.now_ms(),
                    );
                    self.add_message_to_chat(&summary_message, false);
                    if let Some(usage) = result.get("usage") {
                        if let Ok(usage) = serde_json::from_value::<Usage>(usage.clone()) {
                            self.add_compaction_cost_notice(&CompactionCostNotice {
                                kind: CompactionCostKind::Compaction,
                                usage,
                            });
                        }
                    }
                    self.ev(json!(["footer.invalidate"]));
                } else if let Some(error) = error_message {
                    if *reason == CompactionReason::Manual {
                        self.show_error(error);
                    } else {
                        self.io.view.container_add_spacer(ContainerId::Chat);
                        let text = self.fg("error", error);
                        self.io
                            .view
                            .container_add_text(ContainerId::Chat, &text, 1, 0, false);
                    }
                }
                self.flush_compaction_queue(*will_retry).await;
                self.io.view.request_render(None);
            }
            AgentSessionEvent::AutoRetryStart {
                attempt,
                max_attempts,
                delay_ms,
                ..
            } => {
                // Set up escape to abort retry.
                self.lock().retry_escape_handler_active = true;
                self.io.default_editor.set_on_escape();
                let args =
                    serde_json::json!([{ "__describe": "ui" }, attempt, max_attempts, delay_ms]);
                let indicator = self
                    .io
                    .view
                    .new_component(ComponentKind::RetryStatusIndicator, args);
                self.show_status_indicator(indicator, "retry");
                self.io.view.request_render(None);
            }
            AgentSessionEvent::AutoRetryEnd {
                success,
                attempt,
                final_error,
            } => {
                // Restore escape handler. Bind the flag read first — an
                // `if`-condition self.lock() temporary would hold the state
                // mutex across the body.
                let retry_handler_active = self.lock().retry_escape_handler_active;
                if retry_handler_active {
                    self.io.default_editor.set_on_escape();
                    self.lock().retry_escape_handler_active = false;
                }
                self.clear_status_indicator(Some("retry"));
                // Show error only on final failure (success shows the normal
                // response).
                if !success {
                    self.show_error(&format!(
                        "Retry failed after {} attempts: {}",
                        attempt,
                        final_error
                            .clone()
                            .unwrap_or_else(|| "Unknown error".to_string())
                    ));
                }
                self.io.view.request_render(None);
            }
            AgentSessionEvent::SummarizationRetryScheduled {
                attempt,
                max_attempts,
                delay_ms,
                error_message,
            } => {
                self.show_error(error_message);
                let args =
                    serde_json::json!([{ "__describe": "ui" }, attempt, max_attempts, delay_ms]);
                let indicator = self
                    .io
                    .view
                    .new_component(ComponentKind::RetryStatusIndicator, args);
                self.show_status_indicator(indicator, "retry");
                self.io.view.request_render(None);
            }
            AgentSessionEvent::SummarizationRetryAttemptStart { source } => {
                self.clear_status_indicator(Some("retry"));
                let (kind, args, kind_str) = match source {
                    SummarizationRetrySource::BranchSummary => (
                        ComponentKind::BranchSummaryStatusIndicator,
                        serde_json::json!([{ "__describe": "ui" }]),
                        "branchSummary",
                    ),
                    SummarizationRetrySource::Compaction { reason } => (
                        ComponentKind::CompactionStatusIndicator,
                        serde_json::json!([{ "__describe": "ui" }, reason_str(*reason)]),
                        "compaction",
                    ),
                };
                let indicator = self.io.view.new_component(kind, args);
                self.show_status_indicator(indicator, kind_str);
                self.io.view.request_render(None);
            }
            AgentSessionEvent::SummarizationRetryFinished => {
                self.clear_status_indicator(Some("retry"));
                self.io.view.request_render(None);
            }
            AgentSessionEvent::Preserved(_) => {
                // Lossless ingress events do not drive the interactive shell.
            }
        }
    }

    // -- session rendering ------------------------------------------------------

    /// Upstream `getUserMessageText`.
    /// The duck-typed `message.stopReason` read: the typed variant
    /// serializes its enum, the custom capture carries the wire string.
    pub(crate) fn message_stop_reason(message: &AgentMessage) -> Option<String> {
        match message {
            AgentMessage::Assistant(assistant) => Some(
                serde_json::to_value(assistant.stop_reason)
                    .ok()?
                    .as_str()?
                    .to_string(),
            ),
            AgentMessage::Custom(custom) => custom
                .data
                .get("stopReason")
                .and_then(Value::as_str)
                .map(str::to_string),
            _ => None,
        }
    }

    pub fn get_user_message_text(message: &AgentMessage) -> String {
        let AgentMessage::User(user) = message else {
            return String::new();
        };
        match &user.content {
            crate::ai::types::message::StringOrBlocks::Text(text) => text.clone(),
            crate::ai::types::message::StringOrBlocks::Blocks(blocks) => blocks
                .iter()
                .filter_map(|block| match block {
                    crate::ai::types::message::TextOrImageBlock::Text(text) => {
                        Some(text.text.clone())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        }
    }

    /// Upstream `addCustomEntryToChat`.
    pub fn add_custom_entry_to_chat(&self, entry: &SessionEntry) {
        let SessionEntry::Custom(custom) = entry else {
            return;
        };
        if !self
            .io
            .session
            .extensions()
            .has_entry_renderer(&custom.custom_type)
        {
            return;
        }
        let args = serde_json::json!([entry, { "customType": custom.custom_type }]);
        let component = self.io.view.new_component(ComponentKind::CustomEntry, args);
        self.io.view.update_component(
            &component,
            "setExpanded",
            Value::Bool(self.lock().tool_output_expanded),
        );
        // hasContent() is checked by the component; an empty entry adds
        // nothing (recorded through the view seam in the component args).
        self.io
            .view
            .container_add_component(ContainerId::Chat, &component);
    }

    /// Upstream `addMessageToChat`. Assistant-shaped messages route by role:
    /// the typed variant and the custom capture (the lossless event ingress)
    /// render identically.
    pub fn add_message_to_chat(&self, message: &AgentMessage, populate_history: bool) {
        if message.role() == "assistant" {
            // Bind the state reads first — a json! inline
            // self.lock() temporary would hold the state mutex
            // across the later locks.
            let (hide_thinking_block, hidden_thinking_label, output_pad) = {
                let state = self.lock();
                (
                    state.hide_thinking_block,
                    state.hidden_thinking_label.clone(),
                    state.output_pad,
                )
            };
            let args = serde_json::json!([
                message,
                hide_thinking_block,
                self.get_markdown_theme_with_settings(),
                hidden_thinking_label,
                output_pad,
                self.get_markdown_transformers(),
            ]);
            let component = self
                .io
                .view
                .new_component(ComponentKind::AssistantMessage, args);
            self.io
                .view
                .container_add_component(ContainerId::Chat, &component);
            return;
        }
        match message {
            AgentMessage::System(_) => {}
            AgentMessage::User(user) => {
                let text = Self::get_user_message_text(message);
                if !text.is_empty() {
                    if self.io.view.container_children_len(ContainerId::Chat) > 0 {
                        self.io.view.container_add_spacer(ContainerId::Chat);
                    }
                    if let Some(skill_block) = parse_skill_block(&text) {
                        // Render skill block (collapsible).
                        let args = serde_json::json!([
                            skill_block,
                            self.get_markdown_theme_with_settings()
                        ]);
                        let component = self
                            .io
                            .view
                            .new_component(ComponentKind::SkillInvocationMessage, args);
                        self.io.view.update_component(
                            &component,
                            "setExpanded",
                            Value::Bool(self.lock().tool_output_expanded),
                        );
                        self.io
                            .view
                            .container_add_component(ContainerId::Chat, &component);
                        // Render the user message separately if present.
                        if let Some(user_message) = skill_block.user_message {
                            self.io.view.container_add_spacer(ContainerId::Chat);
                            let args = serde_json::json!([
                                user_message,
                                self.get_markdown_theme_with_settings(),
                                self.lock().output_pad,
                                self.get_markdown_transformers(),
                            ]);
                            let user_component =
                                self.io.view.new_component(ComponentKind::UserMessage, args);
                            self.io
                                .view
                                .container_add_component(ContainerId::Chat, &user_component);
                        }
                    } else {
                        let args = serde_json::json!([
                            text,
                            self.get_markdown_theme_with_settings(),
                            self.lock().output_pad,
                            self.get_markdown_transformers(),
                        ]);
                        let user_component =
                            self.io.view.new_component(ComponentKind::UserMessage, args);
                        self.io
                            .view
                            .container_add_component(ContainerId::Chat, &user_component);
                    }
                    if populate_history {
                        self.editor().add_to_history(&text);
                    }
                }
                let _ = user;
            }
            AgentMessage::Assistant(_) => {}
            AgentMessage::ToolResult(_) => {
                // Tool results render inline with tool calls, handled separately.
            }
            AgentMessage::Custom(custom) => match custom.role.as_str() {
                "bashExecution" => {
                    let command = custom
                        .data
                        .get("command")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    // `new BashExecutionComponent(command, this.ui, message.excludeFromContext)`
                    let exclude = custom
                        .data
                        .get("excludeFromContext")
                        .cloned()
                        .unwrap_or(Value::Null);
                    let args = serde_json::json!([
                        command,
                        { "__describe": "ui" },
                        exclude,
                    ]);
                    let component = self
                        .io
                        .view
                        .new_component(ComponentKind::BashExecution, args);
                    if let Some(output) = custom.data.get("output").and_then(Value::as_str) {
                        self.io.view.update_component(
                            &component,
                            "appendOutput",
                            Value::String(output.to_string()),
                        );
                    }
                    self.io.view.update_component(
                        &component,
                        "setComplete",
                        serde_json::json!([
                            custom.data.get("exitCode").cloned().unwrap_or(Value::Null),
                            custom.data.get("cancelled").cloned().unwrap_or(Value::Null),
                            Value::Null,
                            custom
                                .data
                                .get("fullOutputPath")
                                .cloned()
                                .unwrap_or(Value::Null),
                        ]),
                    );
                    self.io
                        .view
                        .container_add_component(ContainerId::Chat, &component);
                }
                "custom" => {
                    let display = custom
                        .data
                        .get("display")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    if display {
                        let args = serde_json::json!([
                            message,
                            Value::Null,
                            self.get_markdown_theme_with_settings(),
                            self.lock().output_pad,
                        ]);
                        let component = self
                            .io
                            .view
                            .new_component(ComponentKind::CustomMessage, args);
                        self.io.view.update_component(
                            &component,
                            "setExpanded",
                            Value::Bool(self.lock().tool_output_expanded),
                        );
                        self.io
                            .view
                            .container_add_component(ContainerId::Chat, &component);
                    }
                }
                "compactionSummary" | "branchSummary" => {
                    self.io.view.container_add_spacer(ContainerId::Chat);
                    let kind = if custom.role == "compactionSummary" {
                        ComponentKind::CompactionSummaryMessage
                    } else {
                        ComponentKind::BranchSummaryMessage
                    };
                    let args =
                        serde_json::json!([message, self.get_markdown_theme_with_settings()]);
                    let component = self.io.view.new_component(kind, args);
                    self.io.view.update_component(
                        &component,
                        "setExpanded",
                        Value::Bool(self.lock().tool_output_expanded),
                    );
                    self.io
                        .view
                        .container_add_component(ContainerId::Chat, &component);
                }
                _ => {}
            },
        }
    }

    /// Upstream `renderSessionItems` (assistant tool calls, cache-miss
    /// notices, tool result matching).
    pub fn render_session_items(&self, items: &[RenderSessionItem]) {
        self.render_session_items_with_options(items, false);
    }

    /// `renderSessionItems(items, { populateHistory })` — user messages push
    /// into the editor history when the caller asks for it.
    fn render_session_items_with_options(
        &self,
        items: &[RenderSessionItem],
        populate_history: bool,
    ) {
        self.lock().pending_tools.clear();
        let mut rendered_pending_tools: Vec<(String, ComponentRef)> = Vec::new();

        for item in items {
            match item {
                RenderSessionItem::CustomEntry(entry) => {
                    self.add_custom_entry_to_chat(entry);
                }
                RenderSessionItem::UsageEntry(entry) => {
                    self.add_cache_warming_usage(entry);
                }
                RenderSessionItem::CostNotice(notice) => {
                    self.add_compaction_cost_notice(notice);
                }
                RenderSessionItem::Message(message) => {
                    if message.role() == "assistant" {
                        self.add_message_to_chat(message, false);
                        for (tool_id, tool_name, tool_args) in assistant_tool_calls(message) {
                            let ctor = serde_json::json!([
                                tool_name,
                                tool_id,
                                tool_args,
                                {
                                    "showImages": self.io.settings.show_images(),
                                    "imageWidthCells": self.io.settings.image_width_cells(),
                                },
                                {
                                    "name": &tool_name,
                                    "def": self.io.session.tool_definition(&tool_name),
                                    "via": "builtInRenderers",
                                },
                                { "__describe": "ui" },
                                self.io.session_manager.cwd(),
                            ]);
                            let component = self
                                .io
                                .view
                                .new_component(ComponentKind::ToolExecution, ctor);
                            self.io.view.update_component(
                                &component,
                                "setExpanded",
                                Value::Bool(self.lock().tool_output_expanded),
                            );
                            self.io
                                .view
                                .container_add_component(ContainerId::Chat, &component);
                            match assistant_stop_reason(message) {
                                Some("aborted") | Some("error") => {
                                    let error_message =
                                        if assistant_stop_reason(message) == Some("aborted") {
                                            let retry_attempt = self.io.session.retry_attempt();
                                            if retry_attempt > 0 {
                                                format!(
                                                    "Aborted after {} retry attempt{}",
                                                    retry_attempt,
                                                    if retry_attempt > 1 { "s" } else { "" }
                                                )
                                            } else {
                                                "Operation aborted".to_string()
                                            }
                                        } else {
                                            assistant_error_message(message)
                                                .unwrap_or_else(|| "Error".to_string())
                                        };
                                    self.io.view.update_component(
                                        &component,
                                        "updateResult",
                                        serde_json::json!([
                                            { "content": [{ "type": "text", "text": error_message }], "isError": true },
                                            false,
                                        ]),
                                    );
                                }
                                _ => {
                                    rendered_pending_tools.push((tool_id, component));
                                }
                            }
                        }
                    } else if message.role() == "toolResult" {
                        let tool_call_id = tool_result_call_id(message);
                        if let Some(position) = rendered_pending_tools
                            .iter()
                            .position(|(id, _)| *id == tool_call_id)
                        {
                            let (_, component) = rendered_pending_tools.remove(position);
                            self.io.view.update_component(
                                &component,
                                "updateResult",
                                serde_json::json!([message, false]),
                            );
                        }
                    } else {
                        self.add_message_to_chat(message, populate_history);
                    }
                }
            }
        }

        for (tool_call_id, component) in rendered_pending_tools {
            self.lock().pending_tools.insert(tool_call_id, component);
        }
        self.io.view.request_render(None);
    }

    /// Upstream `renderSessionEntries`: entries → items, appending the
    /// compaction/branch cost notice after the summarized messages.
    pub fn render_session_entries(
        &self,
        entries: &[SessionEntry],
        update_footer: bool,
        populate_history: bool,
    ) {
        if update_footer {
            self.ev(json!(["footer.invalidate"]));
            self.update_editor_border_color();
        }
        let mut items: Vec<RenderSessionItem> = Vec::new();
        for entry in entries {
            match entry {
                SessionEntry::Custom(custom) => {
                    items.push(RenderSessionItem::CustomEntry(entry.clone()));
                    let _ = custom;
                    continue;
                }
                SessionEntry::Usage(usage) if usage.kind == "cache_warm" => {
                    items.push(RenderSessionItem::UsageEntry(usage.clone()));
                    continue;
                }
                entry => {
                    let messages = session_entry_to_context_messages(entry);
                    if messages.is_empty() {
                        continue;
                    }
                    items.extend(messages.into_iter().map(RenderSessionItem::Message));
                    let (usage, kind) = match entry {
                        SessionEntry::Compaction(compaction) => {
                            (compaction.usage, CompactionCostKind::Compaction)
                        }
                        SessionEntry::BranchSummary(summary) => {
                            (summary.usage, CompactionCostKind::BranchSummary)
                        }
                        _ => continue,
                    };
                    if let Some(usage) = usage {
                        items.push(RenderSessionItem::CostNotice(CompactionCostNotice {
                            kind,
                            usage,
                        }));
                    }
                }
            }
        }
        self.render_session_items_with_options(&items, populate_history);
    }

    /// Upstream `addCompactionCostNotice`.
    pub fn add_compaction_cost_notice(&self, notice: &CompactionCostNotice) {
        if !self.io.settings.show_cache_miss_notices() {
            return;
        }
        self.io.view.container_add_spacer(ContainerId::Chat);
        let text = compaction_cost_notice_text(notice, &self.theme());
        self.io
            .view
            .container_add_text(ContainerId::Chat, &text, 1, 0, false);
    }

    /// Upstream `addCacheWarmingUsage`: the one-line transcript notice for a
    /// persisted cache-warming usage entry.
    pub fn add_cache_warming_usage(
        &self,
        entry: &crate::coding_agent::session_manager::UsageEntry,
    ) {
        if !self.io.settings.show_cache_miss_notices() {
            return;
        }
        self.io.view.container_add_spacer(ContainerId::Chat);
        let warmer_entry = crate::coding_agent::core::cache_warmer::UsageEntry {
            entry_type: "usage".to_string(),
            id: entry.id.clone(),
            parent_id: entry.parent_id.clone(),
            timestamp: entry.timestamp.clone(),
            kind: entry.kind.clone(),
            provider: entry.provider.clone(),
            model: entry.model.clone(),
            usage: entry.usage,
            note: entry.note.clone(),
        };
        let usage =
            crate::coding_agent::core::cache_warmer::format_cache_warming_usage(&warmer_entry);
        let text = self.theme().fg("dim", &usage).unwrap_or_default();
        self.io
            .view
            .container_add_text(ContainerId::Chat, &text, 1, 0, false);
    }

    /// Upstream `maybeSuggestBugReport` (the once-per-session latch rides
    /// [`ShellState::bug_report_hint_shown`]).
    pub fn maybe_suggest_bug_report(&self, message: &AgentMessage) {
        let retryable = match message {
            AgentMessage::Assistant(assistant) => {
                crate::ai::retry::is_retryable_assistant_error(assistant)
            }
            _ => false,
        };
        if !super::interactive_mode::should_suggest_bug_report(
            assistant_stop_reason(message),
            assistant_error_message(message).as_deref(),
            retryable,
        ) {
            return;
        }
        self.suggest_bug_report();
    }

    /// Upstream `suggestBugReport`: the `/bug` hint, shown at most once.
    pub fn suggest_bug_report(&self) {
        {
            let mut state = self.lock();
            if state.bug_report_hint_shown {
                return;
            }
            state.bug_report_hint_shown = true;
        }
        let output_pad = self.lock().output_pad;
        let text = self
            .theme()
            .fg("muted", &super::interactive_mode::bug_report_hint_text())
            .unwrap_or_default();
        self.io.view.container_add_spacer(ContainerId::Chat);
        self.io
            .view
            .container_add_text(ContainerId::Chat, &text, output_pad, 0, false);
        self.io.view.request_render(None);
    }

    /// Upstream `entry_appended`'s compaction branch: rebuild the visible
    /// transcript around a boundary compaction (entries the compaction
    /// retained first, the compaction summary, then the post-compaction
    /// entries — which are marked as already rendered).
    fn render_boundary_compaction(
        &self,
        entry_id: String,
        compaction: &crate::coding_agent::session_manager::CompactionEntry,
    ) {
        let entries = self.io.session_manager.build_context_entries();
        if entries.first().and_then(|entry| entry.id()) != Some(entry_id.as_str()) {
            return;
        }
        self.io.view.container_clear(ContainerId::Chat);
        let branch = self.io.session_manager.branch();
        let compaction_index = branch
            .iter()
            .position(|entry| entry.id() == Some(entry_id.as_str()));
        let Some(compaction_index) = compaction_index else {
            return;
        };
        let entries_after_compaction: std::collections::HashSet<String> = branch
            [compaction_index + 1..]
            .iter()
            .filter_map(|entry| entry.id().map(str::to_string))
            .collect();
        let retained_entries = entries[1..].to_vec();
        let before: Vec<SessionEntry> = retained_entries
            .iter()
            .filter(|entry| {
                entry
                    .id()
                    .is_none_or(|id| !entries_after_compaction.contains(id))
            })
            .cloned()
            .collect();
        self.render_session_entries(&before, false, false);
        if let Some(summary_message) =
            crate::coding_agent::core::messages::create_compaction_summary_message(
                &compaction.summary,
                compaction.tokens_before,
                &compaction.timestamp,
            )
        {
            self.add_summary_message_to_chat(&summary_message);
        }
        if let Some(usage) = compaction.usage {
            self.add_compaction_cost_notice(&CompactionCostNotice {
                kind: CompactionCostKind::Compaction,
                usage,
            });
        }
        let after: Vec<SessionEntry> = retained_entries
            .iter()
            .filter(|entry| {
                entry
                    .id()
                    .is_some_and(|id| entries_after_compaction.contains(id))
            })
            .cloned()
            .collect();
        self.render_session_entries(&after, false, false);
        {
            let mut state = self.lock();
            for id in &entries_after_compaction {
                state
                    .entries_rendered_by_boundary_compaction
                    .insert(id.clone());
            }
        }
        self.ev(json!(["footer.invalidate"]));
        self.io.view.request_render(None);
    }

    /// Mount a compaction summary message component (the `addMessageToChat`
    /// compactionSummary arm: spacer, component, expanded state).
    fn add_summary_message_to_chat(
        &self,
        summary: &crate::coding_agent::core::messages::CompactionSummaryMessage,
    ) {
        let message_value = serde_json::to_value(summary).unwrap_or(Value::Null);
        self.io.view.container_add_spacer(ContainerId::Chat);
        let args = serde_json::json!([message_value, self.get_markdown_theme_with_settings()]);
        let component = self
            .io
            .view
            .new_component(ComponentKind::CompactionSummaryMessage, args);
        self.io.view.update_component(
            &component,
            "setExpanded",
            Value::Bool(self.lock().tool_output_expanded),
        );
        self.io
            .view
            .container_add_component(ContainerId::Chat, &component);
    }

    /// Upstream `maybeShowThinkingDropNotice`.
    pub fn maybe_show_thinking_drop_notice(&self, message: &AgentMessage) {
        if !self.io.settings.show_cache_miss_notices() {
            return;
        }
        let diagnostics = assistant_diagnostics(message);
        let dropped_count = count_dropped_thinking_blocks(diagnostics.as_ref());
        if dropped_count == 0 {
            return;
        }
        // message_end reaches the UI before the current message is persisted,
        // so the branch's last assistant message is the previous response.
        let branch = self.io.session_manager.branch();
        let mut previous_dropped_count = 0;
        for entry in branch.iter().rev() {
            if let SessionEntry::Message(entry_message) = entry {
                if entry_message.message.role() == "assistant" {
                    previous_dropped_count = count_dropped_thinking_blocks(
                        assistant_diagnostics(&entry_message.message).as_ref(),
                    );
                    break;
                }
            }
        }
        if dropped_count <= previous_dropped_count {
            return;
        }
        let noun = if dropped_count == 1 {
            "thinking block"
        } else {
            "thinking blocks"
        };
        self.io.view.container_add_spacer(ContainerId::Chat);
        let text = self.fg(
            "warning",
            &format!("Anthropic dropped {dropped_count} {noun} (details in session)"),
        );
        self.io
            .view
            .container_add_text(ContainerId::Chat, &text, 1, 0, false);
    }

    /// Upstream `maybeShowCacheMissNotice`.
    pub fn maybe_show_cache_miss_notice(&self, message: &AgentMessage) {
        if !self.io.settings.show_cache_miss_notices() {
            return;
        }
        // Entries don't contain `message` yet: message_end fires before
        // persistence. The miss detection is the cache-stats core; the shell
        // receives the miss through the session probe (S4).
        if let Some(miss) = self.io.session.detect_cache_miss(message) {
            self.add_cache_miss_notice(&miss);
        }
    }

    /// Upstream `addCacheMissNotice`.
    pub fn add_cache_miss_notice(&self, miss: &CacheMiss) {
        if miss.missed_tokens < 20_000.0 && miss.missed_cost < 0.1 {
            return;
        }
        let text = cache_miss_notice_text(
            miss.missed_tokens,
            miss.missed_cost,
            miss.model_changed,
            miss.idle_ms,
            &self.theme(),
        );
        if text.is_empty() {
            return;
        }
        self.io.view.container_add_spacer(ContainerId::Chat);
        self.io
            .view
            .container_add_text(ContainerId::Chat, &text, 1, 0, false);
    }

    /// Upstream `renderInitialMessages`.
    pub fn render_initial_messages(&self) {
        let entries = self.io.session_manager.build_context_entries();
        self.render_session_entries(&entries, true, true);
        self.render_project_trust_warning_if_needed();

        // Show compaction info if the session was compacted.
        let all_entries = self.io.session_manager.entries();
        let compaction_count = all_entries
            .iter()
            .filter(|e| matches!(e, SessionEntry::Compaction(_)))
            .count();
        if compaction_count > 0 {
            let times = if compaction_count == 1 {
                "1 time".to_string()
            } else {
                format!("{compaction_count} times")
            };
            self.show_status(&format!("Session compacted {times}"));
        }
    }

    /// Upstream `renderProjectTrustWarningIfNeeded`.
    pub fn render_project_trust_warning_if_needed(&self) {
        if self.io.settings.project_trusted()
            || !self
                .io
                .platform
                .has_trust_requiring_project_resources(&self.io.session_manager.cwd())
        {
            return;
        }
        if self.io.view.container_children_len(ContainerId::Chat) > 0 {
            self.io.view.container_add_spacer(ContainerId::Chat);
        }
        let text = self.fg(
            "warning",
            &format!(
                "This project is not trusted. Project {} resources and packages are ignored. Use /trust to save a trust decision, then restart pi.",
                super::shell_lower::CONFIG_DIR_NAME
            ),
        );
        self.io
            .view
            .container_add_text(ContainerId::Chat, &text, 1, 0, false);
    }

    /// Upstream `showStartupNoticesIfNeeded`.
    pub fn show_startup_notices_if_needed(&self) {
        if self.lock().startup_notices_shown {
            return;
        }
        self.lock().startup_notices_shown = true;

        let changelog_markdown = match self.lock().changelog_markdown.clone() {
            Some(markdown) => markdown,
            None => return,
        };

        if self.io.view.container_children_len(ContainerId::Chat) > 0 {
            self.io.view.container_add_spacer(ContainerId::Chat);
        }
        self.io.view.container_add_border(ContainerId::Chat, None);
        if self.io.settings.collapse_changelog() {
            // `##\s+\[?(\d+\.\d+\.\d+)\]?` — the first version heading.
            let latest_version = first_changelog_version(&changelog_markdown)
                .unwrap_or_else(|| self.io.version.clone());
            // The r18 harness's chalk has no color support; the recorded
            // line carries only the theme-bold marker.
            let condensed = format!(
                "Updated to v{latest_version}. Use {} to view full changelog.",
                self.theme().bold("/changelog")
            );
            let text = condensed;
            self.io
                .view
                .container_add_text(ContainerId::Chat, &text, 1, 0, false);
        } else {
            let text = self.theme().bold(&self.fg("accent", "What's New"));
            self.io
                .view
                .container_add_text(ContainerId::Chat, &text, 1, 0, false);
            self.io.view.container_add_spacer(ContainerId::Chat);
            self.io.view.container_add_markdown(
                ContainerId::Chat,
                changelog_markdown.trim(),
                1,
                0,
                &self.get_markdown_theme_with_settings(),
            );
            self.io.view.container_add_spacer(ContainerId::Chat);
        }
        self.io.view.container_add_border(ContainerId::Chat, None);
    }

    // -- lifecycle -----------------------------------------------------------------

    /// Upstream `updateTerminalTitle`.
    pub fn update_terminal_title(&self) {
        let cwd_basename = self.io.platform.basename(&self.io.session_manager.cwd());
        let session_name = self.io.session_manager.session_name();
        let title = match session_name {
            Some(name) => format!("{} - {name} - {}", self.io.app_title, cwd_basename),
            None => format!("{} - {}", self.io.app_title, cwd_basename),
        };
        self.ev(json!(["terminal.setTitle", title]));
    }

    /// Upstream `getChangelogForDisplay`.
    pub fn get_changelog_for_display(&self) -> Option<String> {
        // Skip the changelog for resumed/continued sessions.
        if !self.io.session.messages().is_empty() {
            return None;
        }
        match self.io.settings.last_changelog_version() {
            None => {
                // Fresh install — record the version, don't show the changelog
                // (upstream returns undefined).
                self.io
                    .settings
                    .set_last_changelog_version(&self.io.version.clone());
                None
            }
            Some(last_version) => {
                let new_entries = self.io.changelog.new_entries(&last_version);
                if !new_entries.is_empty() {
                    self.io
                        .settings
                        .set_last_changelog_version(&self.io.version.clone());
                    return Some(
                        new_entries
                            .iter()
                            // Entries are (version, content); upstream joins
                            // the contents (interactive-mode.ts:1238).
                            .map(|(_, content)| self.io.changelog.normalize_links(content))
                            .collect::<Vec<_>>()
                            .join("\n\n"),
                    );
                }
                None
            }
        }
    }

    /// Upstream `updateAvailableProviderCount`.
    pub fn update_available_provider_count(&self) {
        let scoped = self.io.session.scoped_models();
        let models: Vec<String> = if !scoped.is_empty() {
            scoped.iter().map(|s| s.model.provider.clone()).collect()
        } else {
            self.io
                .session
                .model_runtime()
                .available_snapshot()
                .iter()
                .map(|m| m.provider.clone())
                .collect()
        };
        let providers: std::collections::HashSet<&String> = models.iter().collect();
        self.ev(json!([
            "footerDataProvider.setAvailableProviderCount",
            providers.len()
        ]));
    }

    /// Upstream `maybeWarnAboutAnthropicSubscriptionAuth`.
    pub async fn maybe_warn_about_anthropic_subscription_auth(&self, model: Option<&ModelRef>) {
        if self.anthropic_warn_probe(model).await {
            self.lock().anthropic_subscription_warning_shown = true;
            self.show_warning(ANTHROPIC_SUBSCRIPTION_AUTH_WARNING);
        }
    }

    /// The probe half of `maybeWarnAboutAnthropicSubscriptionAuth`: runs the
    /// async auth checks and reports whether the subscription warning fires,
    /// without showing it. Upstream fires the notice from a void-async
    /// continuation; the login-completion path linearizes the interleave by
    /// probing before the catalog refresh and showing after it (r21 seam).
    pub(crate) async fn anthropic_warn_probe(&self, model: Option<&ModelRef>) -> bool {
        if !self.io.settings.warnings_anthropic_extra_usage() {
            return false;
        }
        if self.lock().anthropic_subscription_warning_shown {
            return false;
        }
        let Some(model) = model else {
            return false;
        };
        if model.provider != "anthropic" {
            return false;
        }

        let runtime = self.io.session.model_runtime();
        if runtime.check_auth("anthropic").await.as_deref() == Some("oauth") {
            return true;
        }
        let api_key = runtime.get_auth_api_key(&model.provider).await;
        is_anthropic_subscription_auth_key(api_key.as_deref())
    }

    /// Upstream `checkShutdownRequested`.
    pub async fn check_shutdown_requested(&self) {
        if !self.lock().shutdown_requested {
            return;
        }
        self.shutdown(false).await;
    }

    /// Upstream `shutdown`. `from_signal` mirrors the SIGTERM/SIGHUP path
    /// (extension teardown before terminal restore). `process.exit` is the
    /// platform seam (S7) — the shell performs the ordered teardown and
    /// returns the resume command.
    pub async fn shutdown(&self, from_signal: bool) -> Option<String> {
        if self.lock().is_shutting_down {
            return None;
        }
        self.lock().is_shutting_down = true;

        if from_signal {
            // Emit extension cleanup (session_shutdown) BEFORE touching the
            // terminal.
            self.io.host.dispose().await;
            self.ev(json!(["themeController.disableAutoSync"]));
            self.ev(json!(["terminal.drainInput", 1000]));
            self.stop(&self.io.settings.fullscreen_exit_output());
            if !self.io.platform.exit(0) {
                return None;
            }
            // A fake exit that returns falls through to the interactive quit
            // path (the r18 oracle's double-teardown tail); a real exit never
            // gets here.
        }

        // Interactive quit (Ctrl+D, Ctrl+C, /quit, extension shutdown()).
        // Drain in-flight Kitty key release events before stopping.
        self.ev(json!(["themeController.disableAutoSync"]));
        self.ev(json!(["terminal.drainInput", 1000]));
        self.stop(&self.io.settings.fullscreen_exit_output());
        self.io.host.dispose().await;

        let resume_command = super::interactive_mode::format_resume_command(
            self.io.session_manager.as_ref(),
            &self.io.app_name.clone(),
            self.io.platform.stdout_is_tty(),
            |path| self.io.platform.file_exists(path),
        );
        if let Some(resume_command) = &resume_command {
            self.ev(json!([
                "process.stdout.write",
                format!(
                    "{} {resume_command}\n",
                    self.chalk_dim("To resume this session:")
                )
            ]));
        }
        let _ = self.io.platform.exit(0);
        resume_command
    }

    /// Upstream `stop`.
    pub fn stop(&self, fullscreen_exit_output: &str) {
        self.dispose_active_selector();
        if self.io.settings.show_terminal_progress() {
            self.ev(json!(["terminal.setProgress", false]));
        }
        self.clear_status_indicator(None);
        self.ev(json!(["themeController.disableAutoSync"]));
        self.clear_extension_terminal_input_listeners();
        self.ev(json!(["footer.dispose"]));
        self.ev(json!(["footerDataProvider.dispose"]));
        if let Some(subscription) = self.lock().unsubscribe.take() {
            self.io.session.unsubscribe(subscription.0);
        }
        // Bind the flag read first — an `if`-condition self.lock()
        // temporary would hold the state mutex across the body.
        let is_initialized = self.lock().is_initialized;
        if is_initialized {
            self.stop_interactive_tui(fullscreen_exit_output);
            self.lock().is_initialized = false;
        }
        self.unregister_signal_handlers();
    }

    /// Upstream `emergencyTerminalExit`.
    pub fn emergency_terminal_exit(&self) -> ! {
        self.lock().is_shutting_down = true;
        self.unregister_signal_handlers();
        self.io.platform.kill_tracked_detached_children();
        // The terminal is gone. Do not run normal shutdown because TUI and
        // extension cleanup can write restore sequences and re-trigger EIO.
        let _ = self.io.platform.exit(129);
        unreachable!("platform exit")
    }

    /// Upstream `uncaughtCrash`.
    pub fn uncaught_crash(&self, error: &str) -> ! {
        let _ = error;
        if self.lock().is_shutting_down {
            let _ = self.io.platform.exit(1);
            unreachable!("platform exit")
        }
        self.lock().is_shutting_down = true;
        self.unregister_signal_handlers();
        self.io.platform.kill_tracked_detached_children();
        // `this.ui.stop()` — no options object (the crash path preserves
        // nothing and passes no `preserveScreen`).
        self.ev(json!(["ui.stop"]));
        // Upstream's two `console.error` lines are stderr output, outside the
        // collaborator-recording vocabulary (r21 seam disclosure).
        let _ = self.io.platform.exit(1);
        unreachable!("platform exit")
    }

    /// Upstream `registerSignalHandlers` — registration bookkeeping; the OS
    /// signal wiring is the platform seam (S7).
    pub fn register_signal_handlers(&self) {
        self.unregister_signal_handlers();
        let ids = self.io.platform.register_signal_handlers();
        let mut state = self.lock();
        state.signal_cleanup_handlers = ids.into_iter().map(CleanupId).collect();
    }

    /// Upstream `unregisterSignalHandlers`.
    pub fn unregister_signal_handlers(&self) {
        let ids: Vec<u64> = self
            .lock()
            .signal_cleanup_handlers
            .iter()
            .map(|c| c.0)
            .collect();
        self.io.platform.unregister_signal_handlers(&ids);
        self.lock().signal_cleanup_handlers.clear();
    }

    /// Upstream `stopInteractiveTui`.
    fn stop_interactive_tui(&self, fullscreen_exit_output: &str) {
        if self.io.view.renderer_mode() == "fullscreen" && fullscreen_exit_output == "transcript" {
            while self.io.view.has_overlay_entries() {
                self.io.view.renderer_hide_overlay();
            }
            self.switch_tui_mode("regular", false, false);
            self.io.view.render_now();
        }
        self.io
            .view
            .stop(self.io.view.renderer_mode() == "fullscreen");
    }

    /// Upstream `mountInteractiveTui` — mounts the shell-owned containers
    /// into the live renderer (container names, renderer seam).
    pub fn mount_interactive_tui(&self, components: &[&str]) {
        for component in components {
            self.io.view.renderer_add_child_by_name(component);
        }
    }

    /// Upstream `switchTuiMode`. Returns whether the switch happened; the
    /// renderer construction/capture is the view seam (S1/S5).
    pub fn switch_tui_mode(
        &self,
        mode: &str,
        restore_progress: bool,
        start_renderer: bool,
    ) -> bool {
        let previous_mode = self.io.view.renderer_mode();
        if mode == previous_mode {
            return true;
        }
        if self.io.view.has_overlay_entries() {
            return false;
        }
        let focus = self.io.view.renderer_focused_component();
        let terminal = self.io.view.renderer_terminal_id();
        let show_hardware_cursor = self.io.view.renderer_show_hardware_cursor();
        let clear_on_shrink = self.io.view.get_clear_on_shrink();

        self.io.view.stop(true);
        self.io.view.renderer_capture_render_state();
        // `previousUi.setFocus(null)` before clearing (upstream order).
        self.io.view.set_focus(FocusTarget::None);
        self.io.view.renderer_clear();

        let next = self.io.view.renderer_create(mode, terminal);
        self.io.view.renderer_become(next);
        self.lock_options_tui_mode(mode);
        self.io.view.set_clear_on_shrink(clear_on_shrink);
        self.io.view.renderer_invalidate();
        self.io.view.renderer_set_focus_none();
        if let Some(focus) = focus {
            self.io.view.set_focus(FocusTarget::Component(focus));
        }
        if !start_renderer {
            return true;
        }
        self.io.view.renderer_start();
        self.ev(json!(["themeController.rebindTui"]));
        self.rebind_extension_terminal_input_listeners();
        if restore_progress
            && self.io.settings.show_terminal_progress()
            && (self.io.session.is_streaming() || self.io.session.is_compacting())
        {
            self.ev(json!(["terminal.setProgress", true]));
        }
        let _ = show_hardware_cursor;
        true
    }

    /// Upstream `rebindCurrentSession`.
    pub async fn rebind_current_session(&self) {
        self.rebind_current_session_with(false).await;
    }

    /// `rebindCurrentSession(options)`; `render_before_bind` mirrors
    /// `options.renderBeforeBind` (render + subscribe before the bind).
    pub async fn rebind_current_session_with(&self, render_before_bind: bool) {
        let session_before = self.io.view.session_identity();

        if let Some(slot) = self.lock().unsubscribe.take() {
            self.io.session.unsubscribe(slot.0);
        }
        self.apply_runtime_settings();

        if render_before_bind {
            self.render_current_session_state();
            self.subscribe_to_agent();
        }

        self.bind_current_session_extensions().await;

        if self.io.view.session_identity() != session_before {
            return;
        }

        if !render_before_bind {
            self.subscribe_to_agent();
        }

        self.update_available_provider_count();
        self.update_editor_border_color();
        self.update_terminal_title();
    }

    /// Upstream `handleFatalRuntimeError`. The module-level
    /// `stopThemeWatcher()` seam can be absent in the recording harness (the
    /// ReferenceError unwinds the rest of the body, so neither `stop` nor
    /// `process.exit` runs); the platform reports that as `Err`. The fake
    /// `process.exit` of the harness that defines the watcher records and
    /// throws, so the post-exit statements stay unreachable.
    pub async fn handle_fatal_runtime_error(&self, prefix: &str, error: &str) {
        self.show_error(&format!("{prefix}: {error}"));
        if self.io.platform.stop_theme_watcher().is_err() {
            return;
        }
        self.stop("transcript");
        let _ = self.io.platform.exit(1);
    }

    /// Upstream `renderCurrentSessionState`.
    pub fn render_current_session_state(&self) {
        self.io.view.container_clear(ContainerId::LoadedResources);
        self.io.view.container_clear(ContainerId::Chat);
        self.io.view.container_clear(ContainerId::PendingMessages);
        {
            let mut state = self.lock();
            state.compaction_queued_messages.clear();
            state.streaming_component = None;
            state.streaming_message = None;
            state.pending_tools.clear();
        }
        self.render_initial_messages();
    }

    /// Upstream `applyRuntimeSettings`.
    pub fn apply_runtime_settings(&self) {
        self.ev(json!(["setCapabilityOverrides", {}]));
        self.ev(json!([
            "configureHttpDispatcher",
            self.io
                .settings
                .http_idle_timeout_ms()
                .map(Value::from)
                .unwrap_or(Value::Null)
        ]));
        // (The transcript scrollbar rides the fullscreen layout seam; the
        // recorded runtime-settings pass carries no scrollbar entry.)
        // `footer.setSession(session)` — the describe of passing the session
        // object (r21 seam: recorded as its harness typeof projection).
        self.ev(json!(["footer.setSession", "object"]));
        self.ev(json!([
            "footer.setAutoCompactEnabled",
            self.io.session.auto_compaction_enabled()
        ]));
        self.ev(json!([
            "footerDataProvider.setCwd",
            self.io.session_manager.cwd()
        ]));
        {
            let mut state = self.lock();
            state.hide_thinking_block = self.io.settings.hide_thinking_block();
            state.output_pad = self.io.settings.output_pad();
        }
        self.ev(json!([
            "ui.setShowHardwareCursor",
            self.io.settings.show_hardware_cursor()
        ]));
        let clear_on_shrink = self.io.settings.clear_on_shrink();
        self.io.view.set_clear_on_shrink(clear_on_shrink);
        if !clear_on_shrink && self.lock().active_status_indicator.is_none() {
            self.io.view.container_clear(ContainerId::Status);
        }
        let editor_padding_x = self.io.settings.editor_padding_x();
        let autocomplete_max_visible = self.io.settings.autocomplete_max_visible();
        self.io.default_editor.set_padding_x(editor_padding_x);
        self.io
            .default_editor
            .set_autocomplete_max_visible(autocomplete_max_visible);
        // Bind the flag read first — an `if`-condition self.lock()
        // temporary would hold the state mutex across the `self.editor()`
        // re-lock in the body.
        let editor_is_custom = self.lock().editor_is_custom;
        if editor_is_custom {
            self.editor().set_padding_x(editor_padding_x);
            self.editor()
                .set_autocomplete_max_visible(autocomplete_max_visible);
        }
    }

    /// Upstream `bindCurrentSessionExtensions` (ordering core; the extension
    /// UI context itself is the r19 component seam — S3/S5).
    pub async fn bind_current_session_extensions(&self) {
        // Upstream passes the full options object: `{uiContext, mode: "tui",
        // abortHandler, commandContextActions, shutdownHandler, onError}`.
        // Callback-valued members render as their harness `"function"`
        // describe; the uiContext theme projects the live theme singleton.
        let mut ui_context = self.extension_ui_context();
        if let Some(ui) = ui_context.as_object_mut() {
            ui.insert(
                "theme".to_string(),
                serde_json::json!({
                    "name": self.theme().name.clone().unwrap_or_default(),
                    "fg": "function", "bg": "function", "bold": "function",
                    "italic": "function",
                    "getThinkingBorderColor": "function",
                    "getBashModeBorderColor": "function",
                }),
            );
        }
        let options = serde_json::json!({
            "uiContext": ui_context,
            "mode": "tui",
            "abortHandler": "function",
            "commandContextActions": {
                "waitForIdle": "function", "newSession": "function",
                "fork": "function", "navigateTree": "function",
                "switchSession": "function", "reload": "function",
            },
            "shutdownHandler": "function",
            "onError": "function",
        });
        self.io.session.bind_extensions(options).await;

        self.ev(json!(["setRegisteredThemes", []]));
        self.setup_autocomplete_provider();

        self.setup_extension_shortcuts();
        self.show_loaded_resources(false, true);
        self.show_startup_notices_if_needed();
    }
}
