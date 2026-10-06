//! Port of upstream `coding-agent/src/core/extensions/types.ts` (extension
//! system types; upstream SHA256 at migration time:
//! `e764ac48ba9c9795a9320a59b48e23851b6197497163800c98646439f839f5b8`).
//!
//! The static shape table (events, results, registrations, runtime state) is
//! ported as serde types so serialized field names/order match the upstream
//! JSON. The dynamic surface is ported with the same seams as the W3.3
//! [`crate::coding_agent::core::event_bus`] port, disclosed here:
//!
//! - **Handler seam**: upstream handlers are JS functions
//!   `(event, ctx) => Promise<R | void> | R | void`. The port stores
//!   [`HandlerFn`] closures receiving the event as `&mut serde_json::Value`
//!   (`&mut` because `tool_call` and `before_provider_headers` handlers mutate
//!   the event in place upstream) and returning a borrowed future. Its result
//!   is `Ok(Some(value))` for a handler
//!   result, `Ok(None)` for `undefined`, or `Err(message)` for a thrown
//!   `Error` (the message is `error.message`; the JS stack is not portable and
//!   is never attached).
//! - **Event payloads at the seam**: per-event serde structs below construct
//!   the payloads; the dispatcher passes `serde_json::Value` so the
//!   duck-typed result reads (`result.cancel`, `result.message.role`, …)
//!   keep upstream's presence-based semantics.
//! - **Function-bearing seam types**: `UserBashEventResult.operations` carries
//!   a `BashOperations` value with an `exec` function upstream; the port uses
//!   [`HandlerResult::UserBashOperations`]. Everything else travels as JSON.
//! - **TUI-bound members** (`Theme`, `Component`, `TUI`, `OverlayOptions`,
//!   `AutocompleteProvider`, `EditorTheme`/`EditorComponent`, `KeyId`,
//!   `KeybindingsManager`, tool `renderCall`/`renderResult` component
//!   factories, `ToolRenderContext`) are not representable before the TUI and
//!   tools slices land: the UI context is the [`ExtensionUI`] trait with the
//!   full `noOpUIContext` method set as default no-ops, and the component
//!   factory members are omitted from [`ToolDefinition`]. Shortcut keys stay
//!   plain strings (upstream `KeyId` is a string literal type).
//! - **Unported collaborator types**: `SessionManager`
//!   ([`SessionManagerHandle`]), `ModelRegistry` ([`ProviderRegistryHandle`]),
//!   `ScopedModel`/`SlashCommandInfo`/`Provider` (JSON at the seam),
//!   `AbortSignal` (vendored minimal [`AbortSignal`]), and `ExecOptions` /
//!   `ExecResult` (ported in [`crate::coding_agent::extensions::loader`]).
//! - General extension event dispatch awaits native borrowed handler futures.
//!   The separate event_bus, factory and UI host remain native seams.
//!   Command-context actions and setup/withSession callbacks return eager
//!   [`CommandFuture`] handles; immediate throws and async rejections stay distinct.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use super::command_future::CommandFuture;

/// Upstream `ThinkingLevel` (re-exported from pi-agent-core via
/// `@earendil-works/pi-agent-core`; ported as the existing ai primitive).
pub type ThinkingLevel = crate::ai::types::ModelThinkingLevel;

/// Upstream handler `Error` at the seam: the `error.message` string.
pub type HandlerError = String;

/// A native handler may borrow the event/context until its result settles.
/// The dispatcher awaits handlers in registration order and holds no registry
/// lock across this boundary. Mutations made before rejection are not lost.
/// This Rust future is poll-driven, not an eager JavaScript Promise; a future
/// JS host must provide its own Promise/microtask and cancellation semantics.
pub type HandlerFuture<'a> =
    futures::future::BoxFuture<'a, Result<Option<HandlerResult>, HandlerError>>;

/// Upstream `HandlerFn`, including asynchronous extension handlers.
pub type HandlerFn =
    Arc<dyn for<'a> Fn(&'a mut Value, &'a ExtensionContext) -> HandlerFuture<'a> + Send + Sync>;

/// Adapt a synchronous native handler without moving its invocation into a
/// future: the synchronous prefix runs when the dispatcher calls the handler.
pub fn sync_handler<F>(handler: F) -> HandlerFn
where
    F: Fn(&mut Value, &ExtensionContext) -> Result<Option<HandlerResult>, HandlerError>
        + Send
        + Sync
        + 'static,
{
    Arc::new(move |event, ctx| Box::pin(std::future::ready(handler(event, ctx))))
}

/// A handler result. JSON for everything except `user_bash` operations,
/// which carry an `exec` function upstream and use the typed variant.
#[derive(Clone)]
pub enum HandlerResult {
    Json(Value),
    UserBashOperations(BashOperations),
}

impl HandlerResult {
    /// The JSON view (`None` for the typed operations variant; those only
    /// carry meaning for `user_bash`).
    pub fn as_json(&self) -> Option<&Value> {
        match self {
            Self::Json(value) => Some(value),
            Self::UserBashOperations(_) => None,
        }
    }
}

// ============================================================================
// Handler/action function aliases (upstream `*Handler` type names)
// ============================================================================

pub type CompleteCallback = Arc<dyn Fn(&Value) + Send + Sync>;
pub type ErrorCallback = Arc<dyn Fn(&str) + Send + Sync>;
pub type SessionSetupHandler =
    Arc<dyn Fn(&SessionManagerHandle) -> Result<CommandFuture<()>, HandlerError> + Send + Sync>;
pub type WithSessionHandler =
    Arc<dyn Fn(&ExtensionCommandContext) -> Result<CommandFuture<()>, HandlerError> + Send + Sync>;
pub type PrepareArgumentsShim = Arc<dyn Fn(&Value) -> Value + Send + Sync>;
pub type ToolExecuteHandler = Arc<
    dyn Fn(
            &str,
            &Value,
            Option<&Arc<AbortSignal>>,
            Option<&AgentToolUpdateCallbackValue>,
            &ExtensionContext,
        ) -> Result<AgentToolResultValue, HandlerError>
        + Send
        + Sync,
>;
/// Native async counterpart of execute. Owned inputs keep the callback future
/// independent of the registry lock; no nested executor or blocking Tokio worker.
pub type AsyncToolExecuteHandler = Arc<
    dyn Fn(
            String,
            Value,
            Option<Arc<AbortSignal>>,
            Option<AgentToolUpdateCallbackValue>,
            ExtensionContext,
        )
            -> futures::future::BoxFuture<'static, Result<AgentToolResultValue, HandlerError>>
        + Send
        + Sync,
>;
pub type BashExecHandler = crate::coding_agent::agent_session::bash_executor::OperationsExec;
pub type ArgumentCompletionsHandler = Arc<dyn Fn(&str) -> Option<Vec<Value>> + Send + Sync>;
pub type ShortcutHandler = Arc<dyn Fn(&ExtensionContext) -> Result<(), HandlerError> + Send + Sync>;

pub type SendMessageHandler = Arc<dyn Fn(&Value, &SendMessageOptions) + Send + Sync>;
pub type SendUserMessageHandler = Arc<dyn Fn(&Value, &SendUserMessageOptions) + Send + Sync>;
pub type AppendEntryHandler = Arc<dyn Fn(&str, Option<&Value>) + Send + Sync>;
pub type SetSessionNameHandler = Arc<dyn Fn(&str) + Send + Sync>;
pub type GetSessionNameHandler = Arc<dyn Fn() -> Option<String> + Send + Sync>;
pub type SetLabelHandler = Arc<dyn Fn(&str, Option<&str>) + Send + Sync>;
pub type GetActiveToolsHandler = Arc<dyn Fn() -> Vec<String> + Send + Sync>;
pub type GetAllToolsHandler = Arc<dyn Fn() -> Vec<ToolInfo> + Send + Sync>;
/// Upstream `GetSettingsHandler = () => Settings`: a copy of the effective
/// settings (global and project settings merged, with overrides). The
/// `Settings` shape travels as JSON at the seam (the settings-manager slice
/// exposes typed getters).
pub type GetSettingsHandler = Arc<dyn Fn() -> Value + Send + Sync>;
pub type SetActiveToolsHandler = Arc<dyn Fn(&[String]) + Send + Sync>;
pub type RefreshToolsHandler = Arc<dyn Fn() + Send + Sync>;
pub type GetCommandsHandler = Arc<dyn Fn() -> Vec<Value> + Send + Sync>;
pub type SetModelHandler =
    Arc<dyn Fn(&Value) -> Result<CommandFuture<bool>, HandlerError> + Send + Sync>;
pub type GetThinkingLevelHandler = Arc<dyn Fn() -> ThinkingLevel + Send + Sync>;
pub type SetThinkingLevelHandler = Arc<dyn Fn(ThinkingLevel) + Send + Sync>;

pub type GetModelHandler = Arc<dyn Fn() -> Option<Value> + Send + Sync>;
pub type GetScopedModelsHandler = Arc<dyn Fn() -> Vec<ScopedModelValue> + Send + Sync>;
pub type IsIdleHandler = Arc<dyn Fn() -> bool + Send + Sync>;
pub type IsProjectTrustedHandler = Arc<dyn Fn() -> bool + Send + Sync>;
pub type GetSignalHandler = Arc<dyn Fn() -> Option<Arc<AbortSignal>> + Send + Sync>;
pub type AbortHandler = Arc<dyn Fn() + Send + Sync>;
pub type HasPendingMessagesHandler = Arc<dyn Fn() -> bool + Send + Sync>;
pub type ShutdownHandler = Arc<dyn Fn() + Send + Sync>;
pub type GetContextUsageHandler = Arc<dyn Fn() -> Option<ContextUsage> + Send + Sync>;
pub type CompactHandler = Arc<dyn Fn(Option<CompactOptions>) + Send + Sync>;
pub type GetSystemPromptHandler = Arc<dyn Fn() -> String + Send + Sync>;
pub type GetSystemPromptOptionsHandler = Arc<dyn Fn() -> BuildSystemPromptOptions + Send + Sync>;
/// Command actions invoke synchronously. `Err` is an immediate throw; `Ok` is
/// the handler's original awaitable Promise, whose result may itself reject.
pub type WaitForIdleHandler =
    Arc<dyn Fn() -> Result<CommandFuture<()>, HandlerError> + Send + Sync>;
pub type NewSessionHandler = Arc<
    dyn Fn(Option<NewSessionOptions>) -> Result<CommandFuture<Cancelled>, HandlerError>
        + Send
        + Sync,
>;
pub type ForkHandler = Arc<
    dyn Fn(&str, Option<ForkOptions>) -> Result<CommandFuture<Cancelled>, HandlerError>
        + Send
        + Sync,
>;
pub type NavigateTreeHandler = Arc<
    dyn Fn(&str, Option<NavigateTreeOptions>) -> Result<CommandFuture<Cancelled>, HandlerError>
        + Send
        + Sync,
>;
pub type SwitchSessionHandler = Arc<
    dyn Fn(&str, Option<SwitchSessionOptions>) -> Result<CommandFuture<Cancelled>, HandlerError>
        + Send
        + Sync,
>;
pub type ReloadHandler = Arc<dyn Fn() -> Result<CommandFuture<()>, HandlerError> + Send + Sync>;

pub type RegisterProviderHandler = Arc<dyn Fn(&str, &Value) -> Result<(), String> + Send + Sync>;
/// Native provider callbacks and object identity cannot cross a JSON value seam.
pub type NativeProvider = Arc<dyn crate::ai::models::Provider>;
pub type RegisterNativeProviderHandler =
    Arc<dyn Fn(&NativeProvider) -> Result<(), String> + Send + Sync>;
pub type UnregisterProviderHandler = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

// ============================================================================
// JS `Map` semantics
// ============================================================================

/// JavaScript `Map<K, V>` semantics the extension registry objects rely on:
/// insertion order preserved, `set` on an existing key replaces in place,
/// `delete` compacts. `K` is always a string (upstream keys are strings or
/// string-typed `KeyId`s).
#[derive(Debug, Clone, PartialEq)]
pub struct OrderedMap<V> {
    entries: Vec<(String, V)>,
}

impl<V> Default for OrderedMap<V> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<V> OrderedMap<V> {
    pub fn new() -> Self {
        Self::default()
    }

    /// JS `map.set(key, value)` (replace-in-place on existing keys).
    pub fn set(&mut self, key: impl Into<String>, value: V) {
        let key = key.into();
        match self
            .entries
            .iter_mut()
            .find(|(existing, _)| *existing == key)
        {
            Some(slot) => slot.1 = value,
            None => self.entries.push((key, value)),
        }
    }

    pub fn get(&self, key: &str) -> Option<&V> {
        self.entries
            .iter()
            .find(|(existing, _)| existing == key)
            .map(|(_, v)| v)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut V> {
        self.entries
            .iter_mut()
            .find(|(existing, _)| existing == key)
            .map(|(_, v)| v)
    }

    /// JS `map.delete(key)` — true when the key was present.
    pub fn delete(&mut self, key: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|(existing, _)| existing != key);
        self.entries.len() != before
    }

    pub fn has(&self, key: &str) -> bool {
        self.entries.iter().any(|(existing, _)| existing == key)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// JS `map.keys()` in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(k, _)| k.as_str())
    }

    /// JS `map.values()` in insertion order.
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.iter().map(|(_, v)| v)
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.entries.iter_mut().map(|(_, v)| v)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &V)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

// Consuming a snapshot must retain the same iteration order as JS `new Map(old)`.
impl<V> IntoIterator for OrderedMap<V> {
    type Item = (String, V);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

// ============================================================================
// UI Context
// ============================================================================

/// Options for extension UI dialogs (`ExtensionUIDialogOptions`).
#[derive(Debug, Clone, Default)]
pub struct ExtensionUiDialogOptions {
    /// AbortSignal to programmatically dismiss the dialog.
    pub signal: Option<Arc<AbortSignal>>,
    /// Timeout in milliseconds.
    pub timeout: Option<f64>,
}

/// Placement for extension widgets (`WidgetPlacement`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetPlacement {
    AboveEditor,
    BelowEditor,
}

impl WidgetPlacement {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AboveEditor => "aboveEditor",
            Self::BelowEditor => "belowEditor",
        }
    }
}

/// Options for extension widgets (`ExtensionWidgetOptions`).
#[derive(Debug, Clone, Default)]
pub struct ExtensionWidgetOptions {
    pub placement: Option<WidgetPlacement>,
}

/// Set-theme result (`{ success: boolean; error?: string }`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetThemeResult {
    pub success: bool,
    pub error: Option<String>,
}

/// Result payload of a `custom()` UI prompt: the ported seam keeps the
/// resolved value opaque ([`crate::coding_agent::extensions::types::ExtensionUI::custom`]).
pub type CustomPromptResult = Value;

/// Awaitable extension dialogs; native futures never block the executor thread.
pub type UiFuture<'a, T> = futures::future::BoxFuture<'a, Result<T, HandlerError>>;

/// The full `ExtensionUIContext` method set. Defaults replicate upstream
/// `noOpUIContext` exactly (runner.ts `noOpUIContext` literal), so a bare
/// `ExtensionUI` implementor behaves like the no-op context; `None` slots on
/// the runner use the same defaults.
///
/// Component-factory parameters (`TUI`, `Theme`, `Component`) are not
/// representable before the TUI slice: the factory-taking methods
/// (`setWidget`, `setFooter`, `setHeader`, `setEditorComponent`,
/// `addAutocompleteProvider`) accept an opaque [`Value`] handle instead, and
/// `theme` renders as a [`Value`].
pub trait ExtensionUI: Send + Sync {
    fn select<'a>(
        &'a self,
        _title: &'a str,
        _options: &'a [String],
        _opts: &'a ExtensionUiDialogOptions,
    ) -> UiFuture<'a, Option<String>> {
        Box::pin(async move { Ok(None) })
    }
    fn confirm<'a>(
        &'a self,
        _title: &'a str,
        _message: &'a str,
        _opts: &'a ExtensionUiDialogOptions,
    ) -> UiFuture<'a, bool> {
        Box::pin(async move { Ok(false) })
    }
    fn input<'a>(
        &'a self,
        _title: &'a str,
        _placeholder: Option<&'a str>,
        _opts: &'a ExtensionUiDialogOptions,
    ) -> UiFuture<'a, Option<String>> {
        Box::pin(async move { Ok(None) })
    }
    fn notify(&self, _message: &str, _type: Option<&str>) {}
    /// Upstream `onTerminalInput` returns an unsubscribe closure; the port
    /// returns an idempotent no-op handle.
    fn on_terminal_input(&self, _handler: Value) -> TerminalInputUnsubscribe {
        TerminalInputUnsubscribe::no_op()
    }
    fn set_status(&self, _key: &str, _text: Option<&str>) {}
    fn set_working_message(&self, _message: Option<&str>) {}
    fn set_working_visible(&self, _visible: bool) {}
    fn set_working_indicator(&self, _options: Option<&WorkingIndicatorOptions>) {}
    fn set_hidden_thinking_label(&self, _label: Option<&str>) {}
    fn set_widget(&self, _key: &str, _content: Option<&Value>, _options: &ExtensionWidgetOptions) {}
    fn set_footer(&self, _factory: Option<&Value>) {}
    fn set_header(&self, _factory: Option<&Value>) {}
    fn set_title(&self, _title: &str) {}
    fn custom<'a>(
        &'a self,
        _factory: &'a Value,
        _opts: &'a ExtensionUiDialogOptions,
    ) -> UiFuture<'a, Option<CustomPromptResult>> {
        Box::pin(async move { Ok(None) })
    }
    fn paste_to_editor(&self, _text: &str) {}
    fn set_editor_text(&self, _text: &str) {}
    fn get_editor_text(&self) -> String {
        String::new()
    }
    fn editor<'a>(
        &'a self,
        _title: &'a str,
        _prefill: Option<&'a str>,
    ) -> UiFuture<'a, Option<String>> {
        Box::pin(async move { Ok(None) })
    }
    fn add_autocomplete_provider(&self, _factory: &Value) {}
    fn set_editor_component(&self, _factory: Option<&Value>) {}
    fn get_editor_component(&self) -> Option<Value> {
        None
    }
    /// Upstream `get theme()`. The theme object is a TUI-slice type; the port
    /// hands through the configured handle (no-op default: `Null`).
    fn theme(&self) -> Value {
        Value::Null
    }
    fn get_all_themes(&self) -> Vec<(String, Option<String>)> {
        Vec::new()
    }
    fn get_theme(&self, _name: &str) -> Option<Value> {
        None
    }
    fn set_theme(&self, _theme: &Value) -> SetThemeResult {
        SetThemeResult {
            success: false,
            error: Some("UI not available".to_string()),
        }
    }
    fn get_tools_expanded(&self) -> bool {
        false
    }
    fn set_tools_expanded(&self, _expanded: bool) {}
}

/// Upstream `onTerminalInput` unsubscribe closure.
#[derive(Clone)]
pub struct TerminalInputUnsubscribe {
    inner: Arc<dyn Fn() + Send + Sync>,
}

impl TerminalInputUnsubscribe {
    pub fn no_op() -> Self {
        Self {
            inner: Arc::new(|| {}),
        }
    }
    pub fn new(inner: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self { inner }
    }
    pub fn unsubscribe(&self) {
        (self.inner)();
    }
}

/// Working indicator configuration (`WorkingIndicatorOptions`).
#[derive(Debug, Clone, Default)]
pub struct WorkingIndicatorOptions {
    pub frames: Option<Vec<String>>,
    pub interval_ms: Option<u64>,
}

// ============================================================================
// AbortSignal seam
// ============================================================================

/// Native AbortSignal with a wakeable cancellation token and synchronous
/// listeners. A listener registered after abort fires immediately; callbacks
/// run outside the listener lock so they may remove themselves or re-enter.
#[derive(Default)]
pub struct AbortSignal {
    state: Mutex<AbortSignalState>,
    token: tokio_util::sync::CancellationToken,
}
#[derive(Default)]
struct AbortSignalState {
    aborted: bool,
    next_id: u64,
    listeners: std::collections::BTreeMap<u64, Arc<dyn Fn() + Send + Sync>>,
}
impl std::fmt::Debug for AbortSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AbortSignal")
            .field("aborted", &self.is_aborted())
            .finish_non_exhaustive()
    }
}
pub struct AbortSubscription {
    signal: std::sync::Weak<AbortSignal>,
    id: u64,
}
impl Drop for AbortSubscription {
    fn drop(&mut self) {
        if let Some(signal) = self.signal.upgrade() {
            signal
                .state
                .lock()
                .expect("abort signal")
                .listeners
                .remove(&self.id);
        }
    }
}
impl AbortSignal {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn abort(&self) {
        let callbacks = {
            let mut state = self.state.lock().expect("abort signal");
            if state.aborted {
                return;
            }
            state.aborted = true;
            state.listeners.keys().copied().collect::<Vec<_>>()
        };
        // A listener may unsubscribe a later listener while abort dispatch is
        // in progress. Remove each live callback just before invoking it.
        for id in callbacks {
            let callback = self
                .state
                .lock()
                .expect("abort signal")
                .listeners
                .remove(&id);
            if let Some(callback) = callback {
                callback();
            }
        }
        self.token.cancel();
    }
    pub fn is_aborted(&self) -> bool {
        self.state.lock().expect("abort signal").aborted
    }
    pub async fn cancelled(&self) {
        self.token.cancelled().await;
    }
    pub fn on_abort(self: &Arc<Self>, callback: Arc<dyn Fn() + Send + Sync>) -> AbortSubscription {
        let (id, aborted) = {
            let mut state = self.state.lock().expect("abort signal");
            let id = state.next_id;
            state.next_id += 1;
            if !state.aborted {
                state.listeners.insert(id, callback.clone());
            }
            (id, state.aborted)
        };
        if aborted {
            callback();
        }
        AbortSubscription {
            signal: Arc::downgrade(self),
            id,
        }
    }
}

impl PartialEq for AbortSignal {
    /// Identity equality (JS object identity).
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

// ============================================================================
// Source info (vendored subset of `core/source-info.ts`)
// ============================================================================

/// Upstream `SourceScope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceScope {
    #[serde(rename = "user")]
    User,
    #[serde(rename = "project")]
    Project,
    #[serde(rename = "temporary")]
    Temporary,
}

/// Upstream `SourceOrigin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceOrigin {
    #[serde(rename = "package")]
    Package,
    #[serde(rename = "top-level")]
    TopLevel,
}

/// Upstream `SourceInfo` (the `createSyntheticSourceInfo` subset the loader
/// uses; the metadata-driven `createSourceInfo` lives with the package
/// manager slice).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    pub path: String,
    pub source: String,
    pub scope: SourceScope,
    pub origin: SourceOrigin,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_dir: Option<String>,
}

/// Upstream `createSyntheticSourceInfo(path, options)` (source-info.ts:29).
pub fn create_synthetic_source_info(
    path: &str,
    source: &str,
    scope: Option<SourceScope>,
    origin: Option<SourceOrigin>,
    base_dir: Option<String>,
) -> SourceInfo {
    SourceInfo {
        path: path.to_string(),
        source: source.to_string(),
        scope: scope.unwrap_or(SourceScope::Temporary),
        origin: origin.unwrap_or(SourceOrigin::TopLevel),
        base_dir,
    }
}

/// Upstream `BUILTIN_PATH_PREFIX` (source-info.ts): prefix of built-in tool and
/// extension paths, such as `builtin:read` or `builtin:mcp`.
pub const BUILTIN_PATH_PREFIX: &str = "builtin:";

/// Upstream `getSyntheticPathSource(path)` (source-info.ts): `builtin` for
/// `builtin:<name>`, or the prefix of an angle-bracket path such as `inline`
/// for `<inline:name>`. `None` for file paths.
pub fn get_synthetic_path_source(path: &str) -> Option<String> {
    if path.starts_with(BUILTIN_PATH_PREFIX) {
        return Some("builtin".to_string());
    }
    if path.starts_with('<') && path.ends_with('>') {
        let inner = &path[1..path.len() - 1];
        let first = inner.split(':').next().unwrap_or("");
        return Some(if first.is_empty() {
            "temporary".to_string()
        } else {
            first.to_string()
        });
    }
    None
}

/// Upstream `isSyntheticPath(path)`.
pub fn is_synthetic_path(path: &str) -> bool {
    path.starts_with(BUILTIN_PATH_PREFIX) || path.starts_with('<')
}

// ============================================================================
// Context
// ============================================================================

/// Upstream `ContextUsage`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsage {
    pub tokens: Option<i64>,
    pub context_window: i64,
    pub percent: Option<f64>,
}

/// Upstream `CompactOptions`.
#[derive(Clone, Default)]
pub struct CompactOptions {
    pub custom_instructions: Option<String>,
    pub on_complete: Option<CompleteCallback>,
    pub on_error: Option<ErrorCallback>,
}

/// Upstream `ExtensionMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExtensionMode {
    Tui,
    Rpc,
    Json,
    #[default]
    Print,
}

impl ExtensionMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Tui => "tui",
            Self::Rpc => "rpc",
            Self::Json => "json",
            Self::Print => "print",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "tui" => Some(Self::Tui),
            "rpc" => Some(Self::Rpc),
            "json" => Some(Self::Json),
            "print" => Some(Self::Print),
            _ => None,
        }
    }
}

/// Upstream `SessionManager` handle: the runner only stores it and exposes it
/// on contexts; the session-manager slice will replace this.
pub type SessionManagerHandle = Arc<dyn std::any::Any + Send + Sync>;

/// Upstream `ModelRegistry` surface the runner needs (provider registration
/// fallback in `bindCore`). The model-registry slice will replace this.
pub trait ProviderRegistryHandle: Send + Sync {
    fn register_provider(&self, _name: &str, _config: &Value) -> Result<(), HandlerError> {
        Ok(())
    }
    fn register_native_provider(&self, _provider: &NativeProvider) -> Result<(), HandlerError> {
        Ok(())
    }
    fn unregister_provider(&self, _name: &str) -> Result<(), HandlerError> {
        Ok(())
    }
    /// Upstream `modelRegistry.registerVirtualModel(definition)` fallback for
    /// virtual models registered without provider actions. The catalog
    /// definition (minus `route`) travels as JSON; the route function stays
    /// on the [`VirtualModelDefinitionHandle`] at the runtime seam.
    fn register_virtual_model(&self, _definition: &Value) -> Result<(), HandlerError> {
        Ok(())
    }
    fn unregister_virtual_model(&self, _provider: &str, _id: &str) -> Result<(), HandlerError> {
        Ok(())
    }
    /// Upstream `modelRegistry.getApiKeyForProvider(provider)` (v1.0.0): the
    /// current API key of a pi provider, which MCP servers with
    /// `auth.provider` send instead of OAuth tokens. `None` when the provider
    /// has no key.
    fn get_api_key_for_provider(
        self: std::sync::Arc<Self>,
        _provider: String,
    ) -> futures::future::BoxFuture<'static, Option<String>> {
        Box::pin(async { None })
    }
}

/// Registry-less stand-in (no-op defaults).
pub struct NoopProviderRegistry;

impl ProviderRegistryHandle for NoopProviderRegistry {}

/// Upstream `Provider` / `ScopedModel` / `SlashCommandInfo` travel as JSON at
/// the seam until their slices land.
pub type ScopedModelValue = Value;

/// The extension event context (`ExtensionContext`). Values are read live
/// from the runner (upstream lazy getters), and every access asserts the
/// context is not stale (`Result::Err` carries the stale message, the
/// upstream thrown `Error`).
///
/// Upstream `ExtensionToolContext` is the same context object with two extra
/// members (`tools`, `executeTool`) defined on it by
/// [`super::runner::ExtensionRunner::create_tool_context`]; the port carries
/// them as the [`ToolContextState`] payload below.
#[derive(Clone)]
pub struct ExtensionContext {
    pub(crate) inner: Arc<super::runner::RunnerInner>,
    /// `ctx.getSystemPrompt()` override used by
    /// `emitBeforeAgentStart` (upstream rebinds the method on the context
    /// object to keep it in sync with chained handler updates).
    pub(crate) system_prompt_override: Option<Arc<dyn Fn() -> String + Send + Sync>>,
    /// `ExtensionToolContext` members, present only on contexts created by
    /// `createToolContext` (upstream defines the properties on the object).
    pub(crate) tool: Option<ToolContextState>,
}

/// Backing state of the `ExtensionToolContext` members (upstream
/// `Object.defineProperties` in `createToolContext`).
#[derive(Clone)]
pub(crate) struct ToolContextState {
    /// The calling tool's id; nested calls get `<calling id>/<n>`.
    pub tool_call_id: String,
    /// Default signal of nested calls (the calling tool's signal).
    pub default_signal: Option<Arc<AbortSignal>>,
    /// Backs `ExtensionToolContext.tools` (upstream `getCallableToolsFn`).
    pub callable_tools: Option<CallableToolsFn>,
    /// Backs `ExtensionToolContext.executeTool` (upstream `executeToolFn`).
    pub execute_tool: Option<ExecuteToolFn>,
}

/// Upstream `ExtensionContextActions.getCallableTools`: the tools
/// `executeTool` can call.
pub type CallableToolsFn = Arc<dyn Fn() -> Vec<crate::agent_core::types::AgentTool> + Send + Sync>;

/// Upstream `ExtensionContextActions.executeTool(callerId, name, args,
/// options)`: runs another tool through the same validation, hooks, and
/// permission checks as model-issued calls. The outcome travels as JSON at
/// the seam ([`AgentToolCallOutcome`]); `Err` is an unexpected throw (tool
/// failures upstream never reject — they return `isError: true`).
pub type ExecuteToolFn = Arc<
    dyn Fn(
            String,
            String,
            Value,
            ExecuteToolOptions,
        ) -> futures::future::BoxFuture<'static, Result<Value, String>>
        + Send
        + Sync,
>;

/// Upstream `AgentToolCallOutcome` (pi-agent-core) at the JSON seam.
pub type AgentToolCallOutcome = Value;

/// Options for `ExtensionToolContext.executeTool` (upstream
/// `ExecuteToolOptions`).
#[derive(Default, Clone)]
pub struct ExecuteToolOptions {
    /// Defaults to the calling tool's signal.
    pub signal: Option<Arc<AbortSignal>>,
    /// Receives partial results of the nested tool, in addition to
    /// `tool_execution_update` events.
    pub on_update: Option<AgentToolUpdateCallbackValue>,
}

/// The upstream `createToolContext` fallback outcome for nested calls when no
/// `executeTool` action is bound (verbatim text).
pub fn nested_tool_calls_unavailable_outcome(tool_call_id: &str, name: &str) -> Value {
    serde_json::json!({
        "toolCall": {
            "type": "toolCall",
            "id": format!("{tool_call_id}/0"),
            "name": name,
            "arguments": {},
        },
        "result": {
            "content": [{ "type": "text", "text": "Nested tool calls are not available in this context" }],
            "details": {},
        },
        "isError": true,
    })
}

impl ExtensionContext {
    /// Whether this context carries the `ExtensionToolContext` members.
    pub fn is_tool_context(&self) -> bool {
        self.tool.is_some()
    }

    /// Upstream `ExtensionToolContext.tools`: the tools `executeTool` can
    /// call. Empty without a bound `getCallableTools` action.
    pub fn callable_tools(&self) -> Vec<crate::agent_core::types::AgentTool> {
        match &self.tool {
            Some(state) => state
                .callable_tools
                .as_ref()
                .map(|get| get())
                .unwrap_or_default(),
            None => Vec::new(),
        }
    }

    /// Upstream `ExtensionToolContext.executeTool(name, args, options)`.
    /// Without a bound `executeTool` action the upstream fallback outcome is
    /// returned (`isError: true`, "Nested tool calls are not available in
    /// this context"); on a plain (non-tool) context this is an `Err` — the
    /// upstream object simply has no such method.
    pub async fn execute_tool(
        &self,
        name: &str,
        args: &Value,
        options: ExecuteToolOptions,
    ) -> Result<Value, String> {
        let Some(state) = &self.tool else {
            return Err("Nested tool calls are not available in this context".to_string());
        };
        let Some(execute_tool) = &state.execute_tool else {
            return Ok(nested_tool_calls_unavailable_outcome(
                &state.tool_call_id,
                name,
            ));
        };
        let mut options = options;
        if options.signal.is_none() {
            options.signal = state.default_signal.clone();
        }
        execute_tool(
            state.tool_call_id.clone(),
            name.to_string(),
            args.clone(),
            options,
        )
        .await
    }
}

/// Upstream `ReplacedSessionContext` — the command context handed to
/// `withSession` callbacks. The port keeps the same shape as
/// [`ExtensionCommandContext`] plus the two send methods (which already exist
/// there via the runtime).
#[derive(Clone)]
pub struct ExtensionCommandContext {
    pub(crate) base: ExtensionContext,
}

impl std::ops::Deref for ExtensionCommandContext {
    type Target = ExtensionContext;
    fn deref(&self) -> &ExtensionContext {
        &self.base
    }
}

/// Upstream `NewSessionHandler` options.
#[derive(Default, Clone)]
pub struct NewSessionOptions {
    pub parent_session: Option<String>,
    /// Upstream `setup(sessionManager)`.
    pub setup: Option<SessionSetupHandler>,
    /// Upstream `withSession(ctx)`.
    pub with_session: Option<WithSessionHandler>,
}

/// Upstream `ForkHandler` options.
#[derive(Default, Clone)]
pub struct ForkOptions {
    pub position: Option<TreePosition>,
    pub with_session: Option<WithSessionHandler>,
}

/// Upstream `"before" | "at"` fork position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreePosition {
    Before,
    At,
}

/// Upstream `NavigateTreeHandler` options.
#[derive(Default, Clone)]
pub struct NavigateTreeOptions {
    pub summarize: Option<bool>,
    pub custom_instructions: Option<String>,
    pub replace_instructions: Option<bool>,
    pub label: Option<String>,
}

/// Upstream `SwitchSessionHandler` options.
#[derive(Default, Clone)]
pub struct SwitchSessionOptions {
    pub with_session: Option<WithSessionHandler>,
}

/// Upstream `{ cancelled: boolean }` session-control results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled {
    pub cancelled: bool,
}

// ============================================================================
// Tool Types
// ============================================================================

/// Rendering options for tool results (`ToolRenderResultOptions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolRenderResultOptions {
    pub expanded: bool,
    pub is_partial: bool,
}

/// Upstream `AgentToolResult` (pi-agent-core) travels as JSON at the seam.
pub type AgentToolResultValue = Value;

/// Upstream `AgentToolUpdateCallback`.
pub type AgentToolUpdateCallbackValue = Arc<dyn Fn(&AgentToolResultValue) + Send + Sync>;

/// Upstream `ToolDefinition`. The `renderCall` / `renderResult` component
/// factories and the `ToolRenderContext` state are TUI-slice types and are
/// omitted (see module docs); `parameters` keeps the raw TypeBox JSON schema.
#[derive(Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub label: String,
    pub description: String,
    /// Optional one-line snippet for the Available tools section.
    pub prompt_snippet: Option<String>,
    /// Optional guideline bullets appended to the default system prompt.
    pub prompt_guidelines: Option<Vec<String>>,
    /// Parameter schema (TypeBox JSON). Must be an object to register.
    pub parameters: Value,
    /// Optional provider-side constrained sampling request; `Some(false)`
    /// disables explicitly (upstream `false | ConstrainedSamplingConfig`).
    pub constrained_sampling: Option<Value>,
    /// `"default" | "self"` — `ToolExecutionComponent` shell control.
    pub render_shell: Option<String>,
    /// Optional compatibility shim preparing raw args before validation.
    pub prepare_arguments: Option<PrepareArgumentsShim>,
    /// `"sequential" | "parallel"` per-tool override.
    pub execution_mode: Option<String>,
    /// JSON Schema of `structuredContent` in successful results (TypeBox
    /// JSON). Tools that declare it should always set `structuredContent`.
    pub output_schema: Option<Value>,
    /// How the model reaches the tool. Default [`ToolExposure::Direct`].
    pub exposure: ToolExposure,
    /// Group the tool belongs to, for example its MCP server.
    pub namespace: Option<ToolNamespace>,
    /// Hints about what the tool does, for example from an MCP server.
    pub annotations: Option<ToolAnnotations>,
    /// Whether registering the tool activates it. `None` is the upstream
    /// default: `true` for `direct` and `model-only` tools, never for the
    /// other exposures.
    pub default_active: Option<bool>,
    /// Adjust how the loadout is presented to the model while this tool is
    /// active (upstream `prepareLoadout(loadout)`).
    pub prepare_loadout: Option<PrepareLoadoutHandler>,
    /// Upstream `execute(toolCallId, params, signal, onUpdate, ctx)`.
    pub execute: Option<ToolExecuteHandler>,
    /// Awaitable native implementation. Preferred over the legacy sync callback.
    pub execute_async: Option<AsyncToolExecuteHandler>,
}

impl ToolDefinition {
    pub fn new(name: &str, label: &str, description: &str, parameters: Value) -> Self {
        Self {
            name: name.to_string(),
            label: label.to_string(),
            description: description.to_string(),
            prompt_snippet: None,
            prompt_guidelines: None,
            parameters,
            constrained_sampling: None,
            render_shell: None,
            prepare_arguments: None,
            execution_mode: None,
            output_schema: None,
            exposure: ToolExposure::Direct,
            namespace: None,
            annotations: None,
            default_active: None,
            prepare_loadout: None,
            execute: None,
            execute_async: None,
        }
    }

    /// Upstream `defaultActive` normalization: registering activates the tool
    /// only for `direct`/`model-only` exposures unless overridden.
    pub fn activates_on_registration(&self) -> bool {
        match self.default_active {
            Some(default_active) => default_active,
            None => matches!(
                self.exposure,
                ToolExposure::Direct | ToolExposure::ModelOnly
            ),
        }
    }
}

/// Upstream `ToolExposure`: how the model reaches a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolExposure {
    /// Declared to the model while active, and callable while active.
    #[default]
    Direct,
    /// Declared to the model while active, never callable.
    ModelOnly,
    /// Callable whenever registered; declared only when explicitly activated.
    /// Codemode tools list it in their description.
    Codemode,
    /// Like [`ToolExposure::Codemode`], but codemode tools do not list it;
    /// tool search can find it.
    Deferred,
    /// Registered but unreachable. Activating it has no effect.
    Hidden,
}

impl ToolExposure {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::ModelOnly => "model-only",
            Self::Codemode => "codemode",
            Self::Deferred => "deferred",
            Self::Hidden => "hidden",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "direct" => Some(Self::Direct),
            "model-only" => Some(Self::ModelOnly),
            "codemode" => Some(Self::Codemode),
            "deferred" => Some(Self::Deferred),
            "hidden" => Some(Self::Hidden),
            _ => None,
        }
    }
}

/// Upstream `ToolAnnotations`: MCP-style, author-supplied, unverified hints.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolAnnotations {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_world_hint: Option<bool>,
}

/// Upstream `ToolNamespace`: a group of related tools, such as the tools of
/// one MCP server. Codemode tools list them together.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolNamespace {
    /// For example `mcp__docs`.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

/// Upstream `ToolLoadout`: the tools of a session as
/// `ToolDefinition.prepareLoadout` sees them.
#[derive(Clone)]
pub struct ToolLoadout {
    /// Tools declared to the model (the active tools), in order, with their
    /// original descriptions (upstream `AgentTool[]`).
    pub declared: Vec<crate::agent_core::types::AgentTool>,
    /// Tools callable through `ctx.executeTool()`.
    pub callable: Vec<crate::agent_core::types::AgentTool>,
    /// Every registered tool.
    pub registered: Vec<crate::agent_core::types::AgentTool>,
    exposures: OrderedMap<ToolExposure>,
    namespaces: OrderedMap<ToolNamespace>,
}

impl ToolLoadout {
    pub fn new(
        declared: Vec<crate::agent_core::types::AgentTool>,
        callable: Vec<crate::agent_core::types::AgentTool>,
        registered: Vec<crate::agent_core::types::AgentTool>,
        exposures: OrderedMap<ToolExposure>,
        namespaces: OrderedMap<ToolNamespace>,
    ) -> Self {
        Self {
            declared,
            callable,
            registered,
            exposures,
            namespaces,
        }
    }

    /// Upstream `loadout.getExposure(name)`.
    pub fn get_exposure(&self, name: &str) -> ToolExposure {
        self.exposures
            .get(name)
            .copied()
            .unwrap_or(ToolExposure::Direct)
    }

    /// Upstream `loadout.getNamespace(name)`.
    pub fn get_namespace(&self, name: &str) -> Option<ToolNamespace> {
        self.namespaces.get(name).cloned()
    }
}

/// Upstream `ToolLoadoutChanges`: changes `prepareLoadout` makes to what the
/// model sees.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolLoadoutChanges {
    /// Model-facing descriptions of declared tools, by tool name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descriptions: Option<std::collections::BTreeMap<String, String>>,
    /// Declared tools whose declarations requests leave out. They stay active
    /// and callable, and the transcript still declares them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden_declarations: Option<Vec<String>>,
}

/// Upstream `prepareLoadout(loadout)`; `None` is upstream `undefined`.
pub type PrepareLoadoutHandler =
    Arc<dyn Fn(&ToolLoadout) -> Option<ToolLoadoutChanges> + Send + Sync>;

/// Upstream `defineTool` is a TypeScript inference helper (identity at
/// runtime); the port exposes the identity for source parity.
pub fn define_tool<T>(tool: T) -> T {
    tool
}

// ============================================================================
// Startup/Resource Events
// ============================================================================

/// Upstream `ProjectTrustEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTrustEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "project_trust"
    pub cwd: String,
}

/// Upstream `ProjectTrustEventDecision`.
pub type ProjectTrustEventDecision = &'static str; // "yes" | "no" | "undecided"
pub const PROJECT_TRUST_YES: &str = "yes";
pub const PROJECT_TRUST_NO: &str = "no";
pub const PROJECT_TRUST_UNDECIDED: &str = "undecided";

/// Upstream `ProjectTrustEventResult`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTrustEventResult {
    pub trusted: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remember: Option<bool>,
}

/// Upstream `ProjectTrustContext`.
#[derive(Clone)]
pub struct ProjectTrustContext {
    pub cwd: String,
    pub mode: ExtensionMode,
    pub has_ui: bool,
    /// The `Pick<ExtensionUIContext, "select" | "confirm" | "input" | "notify">`
    /// subset, as an [`ExtensionUI`] implementor.
    pub ui: Option<Arc<dyn ExtensionUI>>,
}

/// Upstream `ResourcesDiscoverEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourcesDiscoverEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "resources_discover"
    pub cwd: String,
    pub reason: ResourcesDiscoverReason,
}

/// Upstream `"startup" | "reload"` discover reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourcesDiscoverReason {
    #[serde(rename = "startup")]
    Startup,
    #[serde(rename = "reload")]
    Reload,
}

/// Result from resources_discover event handlers (`ResourcesDiscoverResult`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourcesDiscoverResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_paths: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_paths: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme_paths: Option<Vec<String>>,
}

/// Upstream `McpServersChangeEvent`: fired when an extension registers or
/// unregisters an MCP server after the extensions are bound. Servers
/// registered while extensions load are read with `pi.getMcpServers()` on
/// `session_start`. Handling this event marks an extension as the one that
/// connects registered servers. `servers` holds every registered server after
/// the change (upstream `RegisteredMcpServer[]`, JSON at the seam).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServersChangeEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "mcp_servers_change"
    pub servers: Vec<Value>,
}

// ============================================================================
// Session Events
// ============================================================================

/// Upstream `SessionStartEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStartEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "session_start"
    pub reason: SessionStartReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_session_file: Option<String>,
}

/// Upstream `"startup" | "reload" | "new" | "resume" | "fork"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStartReason {
    Startup,
    Reload,
    New,
    Resume,
    Fork,
}

/// Upstream `SessionInfoChangedEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfoChangedEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "session_info_changed"
    pub name: Option<String>,
}

/// Upstream `SessionBeforeSwitchEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBeforeSwitchEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "session_before_switch"
    pub reason: SessionSwitchReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_session_file: Option<String>,
}

/// Upstream `"new" | "resume"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSwitchReason {
    New,
    Resume,
}

/// Upstream `SessionBeforeForkEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBeforeForkEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "session_before_fork"
    pub entry_id: String,
    pub position: TreePosition,
}

/// Upstream `CompactionPreparation` (compaction slice type) at the JSON seam.
pub type CompactionPreparationValue = Value;
/// Upstream `CompactionResult` (compaction slice type) at the JSON seam.
pub type CompactionResultValue = Value;
/// Upstream `CompactionEntry` (session-manager slice type) at the JSON seam.
pub type CompactionEntryValue = Value;
/// Upstream `SessionEntry` at the JSON seam.
pub type SessionEntryValue = Value;
/// Upstream `BranchSummaryEntry` at the JSON seam.
pub type BranchSummaryEntryValue = Value;

/// Upstream `SessionBeforeCompactEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBeforeCompactEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "session_before_compact"
    pub preparation: CompactionPreparationValue,
    pub branch_entries: Vec<SessionEntryValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_instructions: Option<String>,
    pub reason: CompactReason,
    pub will_retry: bool,
    /// Not serialized (the DOM `AbortSignal` seam); carried for handlers.
    #[serde(skip)]
    pub signal: Option<Arc<AbortSignal>>,
}

/// Upstream `"manual" | "threshold" | "overflow"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactReason {
    Manual,
    Threshold,
    Overflow,
}

/// Upstream `SessionCompactEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCompactEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "session_compact"
    pub compaction_entry: CompactionEntryValue,
    pub from_extension: bool,
    pub reason: CompactReason,
    pub will_retry: bool,
}

/// Upstream `SessionCompactFailedEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCompactFailedEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "session_compact_failed"
    pub reason: CompactReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub aborted: bool,
    pub will_retry: bool,
    pub from_extension: bool,
}

/// Upstream `SessionShutdownEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionShutdownEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "session_shutdown"
    pub reason: SessionShutdownReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_session_file: Option<String>,
}

/// Upstream `"quit" | "reload" | "new" | "resume" | "fork"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionShutdownReason {
    Quit,
    Reload,
    New,
    Resume,
    Fork,
}

/// Upstream `TreePreparation`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreePreparation {
    pub target_id: String,
    pub old_leaf_id: Option<String>,
    pub common_ancestor_id: Option<String>,
    pub entries_to_summarize: Vec<SessionEntryValue>,
    pub user_wants_summary: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replace_instructions: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Upstream `SessionBeforeTreeEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBeforeTreeEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "session_before_tree"
    pub preparation: TreePreparation,
    /// Not serialized (DOM `AbortSignal` seam).
    #[serde(skip)]
    pub signal: Option<Arc<AbortSignal>>,
}

/// Upstream `SessionTreeEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTreeEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "session_tree"
    pub new_leaf_id: Option<String>,
    pub old_leaf_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_entry: Option<BranchSummaryEntryValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_extension: Option<bool>,
}

// ============================================================================
// Agent Events
// ============================================================================

/// Upstream `ContextEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "context"
    pub messages: Vec<Value>,
}

/// Upstream `ContextWithSystemEvent`: fired before each LLM call, after every
/// `context` handler has run and Pi has restored the prompt and tool state.
/// `messages` is the full transcript including system messages, and the
/// result is sent as returned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextWithSystemEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "context_with_system"
    pub messages: Vec<Value>,
}

/// Upstream `ProviderStreamEvent`: fired for a parsed provider stream event
/// before Pi normalizes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStreamEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "provider_stream_event"
    pub provider: String,
    pub api: String,
    pub model: String,
    pub data: Value,
}

/// Upstream `BeforeProviderRequestEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeforeProviderRequestEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "before_provider_request"
    pub payload: Value,
}

/// Upstream `BeforeProviderHeadersEvent`. `headers` is the mutable
/// `ProviderHeaders` record (`null` deletes a header).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeforeProviderHeadersEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "before_provider_headers"
    pub headers: Value,
}

/// Upstream `AfterProviderResponseEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AfterProviderResponseEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "after_provider_response"
    pub status: i64,
    pub headers: Value,
}

/// Upstream `CacheWarmingAction` (cache-warmer.ts): `"warm" | "stop"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheWarmingAction {
    #[serde(rename = "warm")]
    Warm,
    #[serde(rename = "stop")]
    Stop,
}

impl CacheWarmingAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Warm => "warm",
            Self::Stop => "stop",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "warm" => Some(Self::Warm),
            "stop" => Some(Self::Stop),
            _ => None,
        }
    }
}

/// Upstream `CacheWarmingDecisionEvent` (cache-warmer.ts): fired before each
/// refresh with pi's decision filled in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheWarmingDecisionEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "cache_warming_decision"
    /// Price of this refresh: a cache read of the prompt plus one output token.
    pub warm_cost: f64,
    /// Extra price of the next real request if the cache entry is lost.
    pub miss_cost: f64,
    /// Estimated chance that a real request arrives before the entry expires.
    pub continuation_probability: f64,
    /// Pi's decision: "warm" when `expectedSavings` is at least $0.05.
    pub action: CacheWarmingAction,
}

/// Upstream `CacheWarmingDecisionEventResult`: override whether this refresh
/// is sent. "stop" ends warming until the next real request.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheWarmingDecisionEventResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<CacheWarmingAction>,
}

/// Upstream `ImageContent` at the JSON seam.
pub type ImageContentValue = Value;
/// Upstream `AgentMessage` at the JSON seam.
pub type AgentMessageValue = Value;
/// Upstream `AssistantMessageEvent` at the JSON seam.
pub type AssistantMessageEventValue = Value;

/// Upstream `BeforeAgentStartEvent`. `systemPrompt` is a getter upstream; the
/// dispatcher serializes the current rendered prompt into the payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeforeAgentStartEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "before_agent_start"
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<ImageContentValue>>,
    pub system_prompt: String,
    pub system_prompt_options: Value,
}

/// Upstream `AgentStartEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStartEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "agent_start"
}

/// Upstream `AgentEndEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEndEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "agent_end"
    pub messages: Vec<AgentMessageValue>,
}

/// Upstream `AgentSettledEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSettledEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "agent_settled"
}

/// Upstream `AgentActivityOutcome`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentActivityOutcome {
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "aborted")]
    Aborted,
    #[serde(rename = "error")]
    Error,
}

impl AgentActivityOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Aborted => "aborted",
            Self::Error => "error",
        }
    }
}

/// Upstream `SessionBoundaryDraft` at the JSON seam: one of
/// `CustomEntryDraft` (`type: "custom"`), `CustomMessageEntryDraft`
/// (`type: "custom_message"`), `ContextEditEntryDraft`
/// (`type: "context_edit"`), or `CompactionEntryDraft` (`type:
/// "compaction"`). The entry shapes are session-manager slice types.
pub type SessionBoundaryDraft = Value;

/// Upstream `BoundaryContextPreview` at the JSON seam: `{ contextEntries:
/// ProjectedSessionEntry[], contextMessages, llmMessages, pendingMessages,
/// canContinue }`.
pub type BoundaryContextPreview = Value;

/// Upstream `BoundaryResult`: changes a boundary handler makes. Omitted
/// fields stay as they are.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundaryResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entries: Option<Vec<SessionBoundaryDraft>>,
    #[serde(rename = "continue", default, skip_serializing_if = "Option::is_none")]
    pub r#continue: Option<bool>,
}

/// Upstream `TurnEndEventResult` / `AgentBeforeSettleEventResult`.
pub type BoundaryEventResult = BoundaryResult;

/// Upstream `UIPromptKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UIPromptKind {
    Select,
    Confirm,
    Input,
    Editor,
    Custom,
}

impl UIPromptKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Select => "select",
            Self::Confirm => "confirm",
            Self::Input => "input",
            Self::Editor => "editor",
            Self::Custom => "custom",
        }
    }
}

/// Upstream `UIPromptStartEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UIPromptStartEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "ui_prompt_start"
    pub reason: String, // "ui_prompt"
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Upstream `UIPromptEndEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UIPromptEndEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "ui_prompt_end"
    pub reason: String, // "ui_prompt"
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Upstream `TurnStartEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "turn_start"
    pub turn_index: i64,
    pub timestamp: i64,
}

/// Upstream `TurnEndEvent` (`BoundaryState` flattened: `entries`,
/// `continue`, `context`, `outcome` plus the turn fields). The preview and
/// drafts travel as JSON at the seam.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnEndEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "turn_end"
    pub turn_index: i64,
    pub message: AgentMessageValue,
    pub tool_results: Vec<Value>,
    pub message_entry_id: String,
    pub tool_result_entry_ids: Vec<String>,
    #[serde(default)]
    pub entries: Vec<SessionBoundaryDraft>,
    #[serde(rename = "continue", default)]
    pub r#continue: bool,
    #[serde(default)]
    pub context: BoundaryContextPreview,
    #[serde(default = "default_outcome")]
    pub outcome: String, // "completed" | "aborted" | "error"
}

fn default_outcome() -> String {
    "completed".to_string()
}

/// Upstream `AgentBeforeSettleEvent` (`BoundaryState` flattened): fired
/// before final settlement. May append entries and ensure one next provider
/// request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBeforeSettleEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "agent_before_settle"
    #[serde(default)]
    pub entries: Vec<SessionBoundaryDraft>,
    #[serde(rename = "continue", default)]
    pub r#continue: bool,
    #[serde(default)]
    pub context: BoundaryContextPreview,
    #[serde(default = "default_outcome")]
    pub outcome: String,
}

/// Upstream `MessageStartEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageStartEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "message_start"
    pub message: AgentMessageValue,
}

/// Upstream `MessageUpdateEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageUpdateEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "message_update"
    pub message: AgentMessageValue,
    pub assistant_message_event: AssistantMessageEventValue,
}

/// Upstream `MessageEndEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageEndEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "message_end"
    pub message: AgentMessageValue,
}

/// Upstream `ToolExecutionStartEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolExecutionStartEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "tool_execution_start"
    pub tool_call_id: String,
    pub tool_name: String,
    pub args: Value,
    /// Set when another tool (for example a codemode script) made this call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_tool_call_id: Option<String>,
}

/// Upstream `ToolExecutionUpdateEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolExecutionUpdateEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "tool_execution_update"
    pub tool_call_id: String,
    pub tool_name: String,
    pub args: Value,
    pub partial_result: Value,
    /// Set when another tool (for example a codemode script) made this call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_tool_call_id: Option<String>,
}

/// Upstream `ToolExecutionEndEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolExecutionEndEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "tool_execution_end"
    pub tool_call_id: String,
    pub tool_name: String,
    pub result: Value,
    pub is_error: bool,
    /// Set when another tool (for example a codemode script) made this call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_tool_call_id: Option<String>,
}

// ============================================================================
// Model Events
// ============================================================================

/// Upstream `ModelSelectSource`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSelectSource {
    Set,
    Cycle,
    Restore,
}

/// Upstream `ModelSelectEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelectEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "model_select"
    pub model: Value,
    pub previous_model: Option<Value>,
    pub source: ModelSelectSource,
}

/// Upstream `ThinkingLevelSelectEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingLevelSelectEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "thinking_level_select"
    pub level: ThinkingLevel,
    pub previous_level: ThinkingLevel,
}

// ============================================================================
// User Bash Events
// ============================================================================

/// Upstream `UserBashEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserBashEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "user_bash"
    pub command: String,
    pub exclude_from_context: bool,
    pub cwd: String,
}

/// Upstream `BashResult` (bash-executor slice) at the seam; the shape matches
/// the fields `isUserBashEventResult` validates.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BashResult {
    pub output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub cancelled: bool,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_output_path: Option<String>,
}

/// Upstream `BashOperations` (tools/bash.ts slice) at the seam: the `exec`
/// function the runner hands through opaquely.
#[derive(Clone)]
pub struct BashOperations {
    /// The actual asynchronous command/cwd/onData/cancellation interface.
    /// Extensions and AgentSession use the same callable, without JSON coercion.
    pub exec: BashExecHandler,
}

impl std::fmt::Debug for BashOperations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BashOperations")
    }
}

impl PartialEq for BashOperations {
    /// Identity equality (JS object identity).
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.exec, &other.exec)
    }
}

// ============================================================================
// Input Events
// ============================================================================

/// Upstream `InputSource`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputSource {
    Interactive,
    Rpc,
    Extension,
}

/// Upstream `"steer" | "followUp"` streaming delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StreamingDelivery {
    Steer,
    FollowUp,
}

/// Upstream `InputEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "input"
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<ImageContentValue>>,
    pub source: InputSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub streaming_behavior: Option<StreamingDelivery>,
}

/// Upstream `InputEventResult`.
#[derive(Debug, Clone, PartialEq)]
pub enum InputEventResult {
    Continue,
    Transform {
        text: String,
        images: Option<Vec<ImageContentValue>>,
    },
    Handled,
}

impl InputEventResult {
    /// Parse a handler-returned value into the upstream
    /// `{ action: "continue" | "transform" | "handled" }` shape.
    pub fn from_value(value: &Value) -> Option<Self> {
        let action = value.get("action")?.as_str()?;
        match action {
            "continue" => Some(Self::Continue),
            "handled" => Some(Self::Handled),
            "transform" => Some(Self::Transform {
                text: value.get("text")?.as_str()?.to_string(),
                images: value.get("images").and_then(|images| {
                    let images = images.as_array()?;
                    Some(images.to_vec())
                }),
            }),
            _ => None,
        }
    }
}

// ============================================================================
// Tool Events
// ============================================================================

/// Upstream `ToolCallEvent`. The per-tool input types
/// (`BashToolInput`, …) live in the tools slice, so `input` stays JSON; the
/// built-in tool names are pinned by [`ToolCallEventName`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "tool_call"
    /// The call's id. For calls another tool made (with `parentToolCallId`
    /// set), pi assigns `<parent id>/<n>`; such ids never appear as tool
    /// calls or tool results in the transcript.
    pub tool_call_id: String,
    pub tool_name: String,
    pub input: Value,
    /// Set when another tool (for example a codemode script) issued this call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_tool_call_id: Option<String>,
}

/// Upstream built-in tool names that narrow `ToolCallEvent`.
pub const TOOL_CALL_EVENT_BUILTINS: [&str; 8] = [
    "bash",
    "powershell",
    "read",
    "edit",
    "write",
    "grep",
    "find",
    "ls",
];

/// Upstream `ToolResultEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultEvent {
    #[serde(rename = "type")]
    pub event_type: String, // "tool_result"
    /// `<parent id>/<n>` for nested calls, see `ToolCallEvent`.
    pub tool_call_id: String,
    pub tool_name: String,
    pub input: Value,
    pub content: Vec<Value>,
    /// Set when another tool (for example a codemode script) issued this call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_tool_call_id: Option<String>,
    /// Machine-readable result for tools that declare an `outputSchema`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// Upstream `isToolCallEventType(toolName, event)`.
pub fn is_tool_call_event_type(tool_name: &str, event: &ToolCallEvent) -> bool {
    event.tool_name == tool_name
}

/// Upstream per-tool `ToolResultEvent` type guards (all compare `toolName`).
pub fn is_bash_tool_result(event: &ToolResultEvent) -> bool {
    event.tool_name == "bash"
}
pub fn is_power_shell_tool_result(event: &ToolResultEvent) -> bool {
    event.tool_name == "powershell"
}
pub fn is_read_tool_result(event: &ToolResultEvent) -> bool {
    event.tool_name == "read"
}
pub fn is_edit_tool_result(event: &ToolResultEvent) -> bool {
    event.tool_name == "edit"
}
pub fn is_write_tool_result(event: &ToolResultEvent) -> bool {
    event.tool_name == "write"
}
pub fn is_grep_tool_result(event: &ToolResultEvent) -> bool {
    event.tool_name == "grep"
}
pub fn is_find_tool_result(event: &ToolResultEvent) -> bool {
    event.tool_name == "find"
}
pub fn is_ls_tool_result(event: &ToolResultEvent) -> bool {
    event.tool_name == "ls"
}

// ============================================================================
// Event Results
// ============================================================================

/// Upstream `ContextEventResult`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextEventResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<AgentMessageValue>>,
}

/// Upstream `BeforeProviderRequestEventResult = unknown`.
pub type BeforeProviderRequestEventResult = Value;

/// Upstream `ToolCallEventResult`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallEventResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminate: Option<bool>,
}

/// Upstream `UserBashEventResult`.
#[derive(Debug, Clone, PartialEq)]
pub enum UserBashEventResult {
    /// Custom operations to use for execution (upstream `{ operations }`).
    Operations(BashOperations),
    /// Full replacement: the extension handled execution (upstream `{ result }`).
    Result(BashResult),
}

/// Upstream `ToolResultEventResult`. Omitted fields stay as they are, except
/// that replacing `content` without returning `structuredContent` drops the
/// structured content, because it may no longer match.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolResultEventResult {
    pub content: Option<Vec<Value>>,
    pub details: Option<Value>,
    pub structured_content: Option<Value>,
    pub is_error: Option<bool>,
    pub usage: Option<Value>,
}

/// Upstream `MessageEndEventResult`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MessageEndEventResult {
    pub message: Option<AgentMessageValue>,
}

/// Upstream `BeforeAgentStartEventResult`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeforeAgentStartEventResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
}

/// Upstream `SessionBeforeSwitchResult`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBeforeSwitchResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel: Option<bool>,
}

/// Upstream `SessionBeforeForkResult`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBeforeForkResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_conversation_restore: Option<bool>,
}

/// Upstream `SessionBeforeCompactResult`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBeforeCompactResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<CompactionResultValue>,
}

/// Upstream `SessionBeforeTreeResult`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBeforeTreeResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<BranchSummaryPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace_instructions: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Upstream `SessionBeforeTreeResult.summary`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummaryPatch {
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
}

// ============================================================================
// Message and Entry Rendering
// ============================================================================

/// Upstream `MessageRenderOptions`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRenderOptions {
    pub expanded: bool,
    pub output_pad: f64,
}

/// Upstream `MarkdownTransformContext`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkdownTransformContext {
    pub message_type: String, // "user" | "assistant" | "assistant-thinking"
    pub is_streaming: bool,
    pub available_width: f64,
}

/// Upstream `EntryRenderOptions`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryRenderOptions {
    pub expanded: bool,
}

/// Upstream `MessageRenderer`. The rendered `Component` is a TUI-slice type;
/// `None` = upstream returning `undefined` (default rendering). The `Theme`
/// parameter travels as an opaque [`Value`].
pub type MessageRenderer =
    Arc<dyn Fn(&Value, &MessageRenderOptions, &Value) -> Option<Value> + Send + Sync>;

/// Upstream `EntryRenderer`.
pub type EntryRenderer =
    Arc<dyn Fn(&Value, &EntryRenderOptions, &Value) -> Option<Value> + Send + Sync>;

/// Upstream `MarkdownTransformer`.
pub type MarkdownTransformer = Arc<dyn Fn(&str, &MarkdownTransformContext) -> String + Send + Sync>;

// ============================================================================
// Command Registration
// ============================================================================

/// Upstream `RegisteredCommand`.
#[derive(Clone)]
pub struct RegisteredCommand {
    pub name: String,
    pub source_info: SourceInfo,
    pub description: Option<String>,
    /// Upstream `getArgumentCompletions(argumentPrefix)`; the
    /// `AutocompleteItem` list (TUI slice) travels as JSON.
    pub get_argument_completions: Option<ArgumentCompletionsHandler>,
    pub handler: CommandHandler,
}

/// Upstream `RegisteredCommand.handler(args, ctx)` (`Promise<void> | void`).
/// `None` is synchronous void, `Some` is an awaitable Promise, and the outer
/// error is a synchronous throw. Consumers must await a returned Promise.
pub type CommandHandler = Arc<
    dyn Fn(&str, &ExtensionCommandContext) -> Result<Option<CommandFuture<()>>, HandlerError>
        + Send
        + Sync,
>;

/// Upstream `ResolvedCommand`.
#[derive(Clone)]
pub struct ResolvedCommand {
    pub command: RegisteredCommand,
    pub invocation_name: String,
}

impl ResolvedCommand {
    pub fn name(&self) -> &str {
        &self.command.name
    }
    pub fn description(&self) -> Option<&str> {
        self.command.description.as_deref()
    }
}

// ============================================================================
// Extension API (registration surface handed to factories)
// ============================================================================

/// Upstream `RegisteredTool`.
#[derive(Clone)]
pub struct RegisteredTool {
    pub definition: Arc<ToolDefinition>,
    pub source_info: SourceInfo,
}

/// Upstream `ExtensionFlag`.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtensionFlag {
    pub name: String,
    pub description: Option<String>,
    pub flag_type: FlagType,
    pub default: Option<FlagValue>,
    pub extension_path: String,
}

/// Upstream `"boolean" | "string"` flag types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagType {
    Boolean,
    String,
}

impl FlagType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Boolean => "boolean",
            Self::String => "string",
        }
    }
    /// Upstream `typeof options.default !== options.type` check.
    pub fn matches(&self, value: &FlagValue) -> bool {
        matches!(
            (self, value),
            (Self::Boolean, FlagValue::Bool(_)) | (Self::String, FlagValue::Str(_))
        )
    }
}

/// Upstream `boolean | string` flag values.
#[derive(Debug, Clone, PartialEq)]
pub enum FlagValue {
    Bool(bool),
    Str(String),
}

impl FlagValue {
    /// The JS `typeof` string, for error-message parity.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Bool(_) => "boolean",
            Self::Str(_) => "string",
        }
    }
}

/// Upstream `ExtensionShortcut`.
#[derive(Clone)]
pub struct ExtensionShortcut {
    pub shortcut: String,
    pub description: Option<String>,
    pub handler: ShortcutHandler,
    pub extension_path: String,
}

// ============================================================================
// Runtime state and actions
// ============================================================================

/// Upstream `SendMessageHandler` options.
#[derive(Default, Clone)]
pub struct SendMessageOptions {
    pub trigger_turn: Option<bool>,
    /// `"steer" | "followUp" | "nextTurn"`.
    pub deliver_as: Option<String>,
}

/// Upstream `SendUserMessageHandler` options.
#[derive(Default, Clone)]
pub struct SendUserMessageOptions {
    /// `"steer" | "followUp"`.
    pub deliver_as: Option<String>,
    pub expand_prompt_templates: Option<bool>,
}

/// Upstream `ToolInfo` (`Pick<ToolDefinition, …> & { sourceInfo }`).
#[derive(Debug, Clone, PartialEq)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub prompt_guidelines: Option<Vec<String>>,
    /// How the model reaches the tool (upstream `exposure`).
    pub exposure: ToolExposure,
    /// Group the tool belongs to, for example its MCP server.
    pub namespace: Option<ToolNamespace>,
    /// Hints about what the tool does, for example from an MCP server.
    pub annotations: Option<ToolAnnotations>,
    pub source_info: SourceInfo,
}

/// Upstream `ExtensionActions`: the action implementations for `pi.*` methods,
/// provided to `bindCore` and copied into the shared runtime.
#[allow(clippy::type_complexity)]
pub struct ExtensionActions {
    pub send_message: SendMessageHandler,
    pub send_user_message: SendUserMessageHandler,
    pub append_entry: AppendEntryHandler,
    pub set_session_name: SetSessionNameHandler,
    pub get_session_name: GetSessionNameHandler,
    pub set_label: SetLabelHandler,
    pub get_active_tools: GetActiveToolsHandler,
    pub get_all_tools: GetAllToolsHandler,
    pub get_settings: GetSettingsHandler,
    pub set_active_tools: SetActiveToolsHandler,
    pub refresh_tools: RefreshToolsHandler,
    pub get_commands: GetCommandsHandler,
    pub set_model: SetModelHandler,
    pub get_thinking_level: GetThinkingLevelHandler,
    pub set_thinking_level: SetThinkingLevelHandler,
}

/// Upstream `ExtensionContextActions` (required by all modes).
#[allow(clippy::type_complexity)]
#[derive(Clone)]
pub struct ExtensionContextActions {
    pub get_model: GetModelHandler,
    pub get_scoped_models: GetScopedModelsHandler,
    pub is_idle: IsIdleHandler,
    pub is_project_trusted: IsProjectTrustedHandler,
    pub get_signal: GetSignalHandler,
    pub abort: AbortHandler,
    pub has_pending_messages: HasPendingMessagesHandler,
    pub shutdown: ShutdownHandler,
    pub get_context_usage: GetContextUsageHandler,
    pub compact: CompactHandler,
    pub get_system_prompt: GetSystemPromptHandler,
    pub get_system_prompt_options: Option<GetSystemPromptOptionsHandler>,
    /// Backs `ExtensionToolContext.executeTool()`. Without it, nested calls
    /// fail with the upstream fallback outcome.
    pub execute_tool: Option<ExecuteToolFn>,
    /// Backs `ExtensionToolContext.tools`.
    pub get_callable_tools: Option<CallableToolsFn>,
}

/// Upstream `ExtensionCommandContextActions` (bound by interactive, print and RPC modes).
#[allow(clippy::type_complexity)]
pub struct ExtensionCommandContextActions {
    pub wait_for_idle: WaitForIdleHandler,
    pub new_session: NewSessionHandler,
    pub fork: ForkHandler,
    pub navigate_tree: NavigateTreeHandler,
    pub switch_session: SwitchSessionHandler,
    pub reload: ReloadHandler,
}

/// Queued provider registration (`pendingProviderRegistrations` entry).
#[derive(Clone)]
pub struct PendingProviderRegistration {
    pub name: String,
    pub config: Value,
    pub extension_path: String,
}

/// Queued native provider registration (`pendingNativeProviderRegistrations`).
#[derive(Clone)]
pub struct PendingNativeProviderRegistration {
    pub provider: NativeProvider,
    pub extension_path: String,
}

/// Upstream `ExtensionVirtualModel.route(request, ctx)` at the seam: the
/// request travels as JSON and the route result (`{ model, thinkingLevel,
/// state? }`) comes back as JSON; `Err` is the route's rejection.
pub type ExtensionVirtualModelRouteFn = Arc<
    dyn Fn(Value, ExtensionContext) -> futures::future::BoxFuture<'static, Result<Value, String>>
        + Send
        + Sync,
>;

/// The wrapped route (upstream `route: (request) =>
/// model.route(request, runtime.createContext())`): the extension context is
/// bound at registration time, resolved lazily per request.
pub type WrappedVirtualModelRouteFn =
    Arc<dyn Fn(Value) -> futures::future::BoxFuture<'static, Result<Value, String>> + Send + Sync>;

/// Post-bind virtual-model registration handler (provider action or the
/// model-registry fallback).
pub type RegisterVirtualModelHandler =
    Arc<dyn Fn(&VirtualModelDefinitionHandle) -> Result<(), String> + Send + Sync>;

/// Post-bind virtual-model unregistration handler.
pub type UnregisterVirtualModelHandler =
    Arc<dyn Fn(&str, &str) -> Result<(), String> + Send + Sync>;

/// Upstream `VirtualModelDefinition` as the runtime carries it: the catalog
/// fields (`{ provider, id, name, thinkingLevels?, contextWindow?,
/// maxTokens?, input? }`) as JSON plus the wrapped route. Registered via
/// `pi.registerVirtualModel()`; a selectable catalog entry that routes each
/// request to a physical model.
#[derive(Clone)]
pub struct VirtualModelDefinitionHandle {
    /// Catalog fields minus `route`.
    pub definition: Value,
    /// The wrapped route (request JSON → route result JSON).
    pub route: WrappedVirtualModelRouteFn,
    /// Path of the registering extension (upstream `extensionPath` on the
    /// pending entry / flush error).
    pub extension_path: String,
}

/// Queued virtual model registration (`pendingVirtualModelRegistrations`).
#[derive(Clone)]
pub struct PendingVirtualModelRegistration {
    pub definition: VirtualModelDefinitionHandle,
}

/// Shared flag value cell access for the runtime (`flagValues`).
#[derive(Debug, Clone, Default)]
pub struct FlagValues(pub HashMap<String, FlagValue>);

/// Upstream `ExtensionError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionError {
    pub extension_path: String,
    pub event: String,
    pub error: String,
    pub stack: Option<String>,
}

/// Upstream `ExtensionErrorListener`.
pub type ExtensionErrorListener = Arc<dyn Fn(&ExtensionError) + Send + Sync>;

/// Upstream `{ path: string; error: string }` loader errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionLoadError {
    pub path: String,
    pub error: String,
}

/// Upstream `{ path: string; warning: string }` loader warnings
/// (`LoadExtensionsResult.warnings`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionLoadWarning {
    pub path: String,
    pub warning: String,
}

// ============================================================================
// System prompt options (vendored subset of `core/system-prompt.ts`)
// ============================================================================

/// Upstream `BuildSystemPromptOptions`. The section *renderer*
/// (`buildSystemPrompt`) needs the skills/config slices; the port vendors the
/// options and the `forceSystemPrompt` short-circuit, and the runner accepts
/// the renderer as a parameter ([`NormalizedSystemPromptRenderer`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BuildSystemPromptOptions {
    pub custom_prompt: Option<String>,
    pub force_system_prompt: Option<String>,
    pub selected_tools: Option<Vec<String>>,
    pub tool_snippets: Option<std::collections::BTreeMap<String, String>>,
    pub tool_guidelines: Option<std::collections::BTreeMap<String, Vec<String>>>,
    pub prompt_guidelines: Option<Vec<String>>,
    pub append_system_prompt: Option<String>,
    pub sections: Option<std::collections::BTreeMap<String, String>>,
    pub cwd: String,
    pub context_files: Option<Vec<(String, String)>>,
    pub skills: Option<Vec<Value>>,
}

impl BuildSystemPromptOptions {
    /// Upstream `normalizeBuildSystemPromptOptions({ cwd })` input.
    pub fn with_cwd(cwd: &str) -> Self {
        Self {
            cwd: cwd.to_string(),
            ..Default::default()
        }
    }
}

/// Upstream `NormalizedBuildSystemPromptOptions` (the normalize output).
#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedBuildSystemPromptOptions {
    pub custom_prompt: Option<String>,
    pub force_system_prompt: Option<String>,
    pub selected_tools: Vec<String>,
    pub tool_snippets: std::collections::BTreeMap<String, String>,
    pub tool_guidelines: std::collections::BTreeMap<String, Vec<String>>,
    pub prompt_guidelines: Vec<String>,
    pub append_system_prompt: String,
    pub sections: std::collections::BTreeMap<String, String>,
    pub cwd: String,
    pub context_files: Vec<(String, String)>,
    pub skills: Vec<Value>,
}

impl NormalizedBuildSystemPromptOptions {
    /// Serialize back to the upstream JSON shape (camelCase object).
    pub fn to_value(&self) -> Value {
        serde_json::json!({
            "customPrompt": self.custom_prompt,
            "forceSystemPrompt": self.force_system_prompt,
            "selectedTools": self.selected_tools,
            "toolSnippets": self.tool_snippets,
            "toolGuidelines": self.tool_guidelines,
            "promptGuidelines": self.prompt_guidelines,
            "appendSystemPrompt": self.append_system_prompt,
            "sections": self.sections,
            "cwd": self.cwd,
            "contextFiles": self.context_files.iter().map(|(path, content)| serde_json::json!({ "path": path, "content": content })).collect::<Vec<_>>(),
            "skills": self.skills,
        })
    }

    /// Parse back from the upstream JSON shape (handler mutations round-trip
    /// through this).
    pub fn from_value(value: &Value) -> Option<Self> {
        let map = |v: &Value, key: &str| -> Option<std::collections::BTreeMap<String, String>> {
            let obj = v.get(key)?.as_object()?;
            Some(
                obj.iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                    .collect(),
            )
        };
        Some(Self {
            custom_prompt: value
                .get("customPrompt")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            force_system_prompt: value
                .get("forceSystemPrompt")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            selected_tools: value
                .get("selectedTools")?
                .as_array()?
                .iter()
                .map(|v| v.as_str().unwrap_or_default().to_string())
                .collect(),
            tool_snippets: map(value, "toolSnippets").unwrap_or_default(),
            tool_guidelines: std::collections::BTreeMap::from_iter(
                value
                    .get("toolGuidelines")?
                    .as_object()?
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            v.as_array()
                                .map(|a| {
                                    a.iter()
                                        .map(|g| g.as_str().unwrap_or_default().to_string())
                                        .collect()
                                })
                                .unwrap_or_default(),
                        )
                    }),
            ),
            prompt_guidelines: value
                .get("promptGuidelines")?
                .as_array()?
                .iter()
                .map(|v| v.as_str().unwrap_or_default().to_string())
                .collect(),
            append_system_prompt: value.get("appendSystemPrompt")?.as_str()?.to_string(),
            sections: map(value, "sections").unwrap_or_default(),
            cwd: value.get("cwd")?.as_str()?.to_string(),
            context_files: value
                .get("contextFiles")?
                .as_array()?
                .iter()
                .map(|f| {
                    (
                        f.get("path")
                            .and_then(|p| p.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        f.get("content")
                            .and_then(|c| c.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect(),
            skills: value.get("skills")?.as_array()?.to_vec(),
        })
    }
}

/// Upstream `normalizeBuildSystemPromptOptions` (verbatim port of the field
/// defaults).
pub fn normalize_build_system_prompt_options(
    input: &BuildSystemPromptOptions,
) -> NormalizedBuildSystemPromptOptions {
    NormalizedBuildSystemPromptOptions {
        custom_prompt: input.custom_prompt.clone(),
        force_system_prompt: input.force_system_prompt.clone(),
        selected_tools: input.selected_tools.clone().unwrap_or_else(|| {
            ["read", "bash", "edit", "write"]
                .iter()
                .map(|s| s.to_string())
                .collect()
        }),
        tool_snippets: input.tool_snippets.clone().unwrap_or_default(),
        tool_guidelines: input.tool_guidelines.clone().unwrap_or_default(),
        prompt_guidelines: input.prompt_guidelines.clone().unwrap_or_default(),
        append_system_prompt: input.append_system_prompt.clone().unwrap_or_default(),
        sections: input.sections.clone().unwrap_or_default(),
        cwd: input.cwd.clone(),
        context_files: input.context_files.clone().unwrap_or_default(),
        skills: input.skills.clone().unwrap_or_default(),
    }
}

/// The `buildSystemPrompt` seam: full section rendering needs the
/// skills/config/pi-ai slices, so the runner takes the renderer as a
/// parameter ([`crate::coding_agent::extensions::runner::ExtensionRunner::emit_before_agent_start`]).
/// The `forceSystemPrompt` short-circuit
/// (`buildSystemPromptState`: return `input.forceSystemPrompt` when set) is
/// the pinned contract.
pub trait NormalizedSystemPromptRenderer: Send + Sync {
    fn build(&self, normalized: &NormalizedBuildSystemPromptOptions) -> String;
}

/// Renderer implementing just the pinned `forceSystemPrompt` short-circuit
/// (`buildSystemPromptState`); the section path renders a deterministic
/// fallback (it is the system-prompt slice's contract, not the runner's).
pub struct ForcePromptRenderer;

impl NormalizedSystemPromptRenderer for ForcePromptRenderer {
    fn build(&self, normalized: &NormalizedBuildSystemPromptOptions) -> String {
        if let Some(force) = &normalized.force_system_prompt {
            return force.clone();
        }
        // Section path placeholder (see trait docs).
        let _ = (&normalized.custom_prompt, &normalized.append_system_prompt);
        String::new()
    }
}

// ============================================================================
// Loaded Extension Types
// ============================================================================

/// Upstream `Extension`.
#[derive(Clone)]
pub struct Extension {
    pub path: String,
    pub resolved_path: String,
    pub hidden: bool,
    /// Upstream `replaceable`: leave this extension out when another
    /// extension registers a tool, command, or flag with a name it registers
    /// during loading, instead of reporting a conflict.
    pub replaceable: bool,
    pub source_info: SourceInfo,
    /// Shared so `on()` unsubscribe handles (which outlive the factory) keep
    /// mutating the same registration list the runner dispatches from —
    /// upstream holds one live `Map` per extension.
    pub handlers: SharedHandlerMap,
    pub tools: OrderedMap<RegisteredTool>,
    /// Registrations made after the factory returns must remain visible to
    /// cloned extension runners (Pisper's native MCP integration patch).
    pub live_tools: Arc<Mutex<OrderedMap<RegisteredTool>>>,
    pub message_renderers: OrderedMap<MessageRenderer>,
    pub markdown_transformer: Option<MarkdownTransformer>,
    pub entry_renderers: Option<OrderMapEntryRenderers>,
    pub commands: OrderedMap<RegisteredCommand>,
    pub flags: OrderedMap<ExtensionFlag>,
    pub shortcuts: OrderedMap<ExtensionShortcut>,
}

/// Alias kept for readability (upstream `entryRenderers?: Map<string,
/// EntryRenderer>`).
pub type OrderMapEntryRenderers = OrderedMap<EntryRenderer>;

/// The live handler registry of one extension (upstream
/// `Map<string, HandlerFn[]>`), shared between the loader's api, unsubscribe
/// handles, and every clone of the [`Extension`].
#[derive(Clone, Default)]
pub struct SharedHandlerMap(pub(crate) Arc<Mutex<OrderedMap<Vec<HandlerFn>>>>);

impl SharedHandlerMap {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(OrderedMap::new())))
    }
    pub fn get(&self, key: &str) -> Option<Vec<HandlerFn>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .cloned()
    }
    pub fn get_cloned_list(&self, key: &str) -> Vec<HandlerFn> {
        self.get(key).unwrap_or_default()
    }
    pub fn set(&self, key: impl Into<String>, value: Vec<HandlerFn>) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set(key, value);
    }
    pub fn has(&self, key: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .has(key)
    }
    pub fn len(&self) -> usize {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Upstream `LoadExtensionsResult`.
#[derive(Clone)]
pub struct LoadExtensionsResult {
    pub extensions: Vec<Extension>,
    pub errors: Vec<ExtensionLoadError>,
    pub warnings: Vec<ExtensionLoadWarning>,
    /// Shared runtime — actions are throwing stubs until `bindCore`.
    pub runtime: super::loader::ExtensionRuntime,
}
