//! Port of upstream `coding-agent/src/core/agent-session.ts` (3625 lines at
//! migration, sha256 `9a3cdcf299f4…`). W3.11 landed the upper
//! half (upstream lines 1-1987): module types (`ParsedSkillBlock`/
//! `parseSkillBlock`, the `AgentSessionEvent` union, config/options
//! interfaces), class construction and dependency assembly (constructor,
//! `_buildRuntime`, `_refreshToolRegistry`, `_bindExtensionCore`), the message
//! submission/append surface (`prompt`/`steer`/`followUp`/`sendCustomMessage`/
//! `sendUserMessage` and the queue surface), the event emission and
//! subscription face (`_handleAgentEvent`, `_emitExtensionEvent`, `subscribe`,
//! `dispose`), session wiring (message persistence on `message_end`,
//! `sessionFile`/`sessionId`/`sessionName`), model/thinking-level management,
//! and queue mode management.
//!
//! W3.12 landed the lower half (upstream lines 1989-3625): manual and
//! automatic compaction execution (`compact`, `_runAutoCompaction`,
//! `_runDefaultCompaction`), extension lifecycle (`bindExtensions`,
//! `reload`, `extendResourcesFromExtensions`), auto-retry (`_prepareRetry`,
//! `isRetrying`, the auto-retry/auto-compaction toggles), bash execution
//! (`executeBash`/`recordBashResult` over the vendored
//! [`bash_executor`]), tree navigation (`navigateTree`), the fork selector
//! (`getUserMessagesForForking`), statistics (`getSessionStats`,
//! `getContextUsage`), exports (`exportToHtml`, `exportToJsonl`), utilities
//! (`getLastAssistantText`), and `createReplacedSessionContext`.
//!
//! # Seams
//!
//! - **Architecture (JS class → Rust).** Upstream is a mutable singleton that
//!   subscribes to the agent inside its constructor; the port builds the
//!   struct, wraps it in an `Arc`, then subscribes. No event can fire before
//!   the constructor returns because the agent only emits during runs. Mutable
//!   fields become interior mutability; runner-bound closures hold a `Weak`
//!   so `dispose` breaks cycles.
//! - **Identity.** Upstream `_replaceMessageInPlace` mutates the message
//!   object in place (agent state, listeners, and persistence share the
//!   object); the port replaces the last value-equal message in agent state
//!   and overwrites the event value (value equality instead of object
//!   identity).
//! - **`agent.streamFunction === streamSimple`.** The port's request executor
//!   is the [`crate::ai::models::Models`] collection for every agent (see
//!   `agent.rs` docs), so `_getSummarizationRequestAuth` always takes the
//!   `streamSimple` branch and delegates to `_getRequiredRequestAuth`; the
//!   summarization transport is [`compaction::models_stream_fn`] over the
//!   agent's collection.
//! - **`extensionRunnerRef`.** The mutable ref the upstream Agent uses to
//!   reach the current runner has no counterpart in the ported `Agent`; not
//!   carried.
//! - **`cleanupSessionResources`.** The pi-ai per-session resource registry is
//!   not ported; `dispose` skips it (no-op).
//! - **Vendored dependency modules** (their own slices have not landed):
//!   `system-prompt.ts` ([`system_prompt`]), the expansion subset of
//!   `prompt-templates.ts` ([`prompt_templates`]),
//!   `createAllToolDefinitions` ([`base_tools`], native read/edit/write;
//!   five remaining tools still report unported errors). Tool-result images
//!   now use the native image backend: passthrough bytes are preserved, but
//!   re-encoding does not promise Photon-identical PNG/JPEG bytes.
//! - **S12 `bash-executor.ts`** ([`bash_executor`]): the executor core is a
//!   verbatim port (chunk buffering, sanitize/ANSI strip, temp-file spill,
//!   `truncateTail`, abort accounting). The concrete `BashOperations` backends
//!   are the tools slice: the default local backend spawns
//!   `{shellPath ?? "bash"} -c <command>` without the shell-config discovery,
//!   `PI_*` env scrubbing, spawn hooks, and PTY handling; remote backends
//!   inject [`BashOperationsHandle`].
//! - **S13 `export-html`**: `exportSessionToHtml` is not ported (no
//!   `tui/export-html` module). [`AgentSession::export_to_html`] keeps the
//!   exact session-level behavior — the `getThemeByName` validation seam has
//!   no theme registry yet, so `themeName` stays `None` until the themes
//!   slice lands; the session-file guard errors and data collection are
//!   exact, and the generation itself is delegated to the injectable
//!   [`HtmlExportFn`] handler ([`AgentSessionConfig::html_exporter`]).
//! - **`resetApiProviders`** (in `reload`): pi-ai keeps a module-global
//!   provider registry upstream; the ported ai layer has none (per-agent
//!   [`Models`](crate::ai::models::Models) collections), so there is nothing
//!   to reset.
//! - **`ReplacedSessionContext`**: upstream shallow-copies the command context
//!   and rebinds `sendMessage`/`sendUserMessage` to the (new) session. The
//!   ported [`ExtensionCommandContext`] routes through the runner's bound
//!   actions, which already target this session, so
//!   [`AgentSession::create_replaced_session_context`] returns
//!   `createCommandContext()` directly.
//! - **Native async events.** Extension handlers, agent event consumption and
//!   session lifecycle dispatch are awaited. `setModel` returns a shared eager
//!   task; the intentionally unawaited send/compact actions use Tokio and route
//!   errors to their existing sinks. JS Promise/microtask/throw identity, the
//!   synchronous tool wrappers and provider registry remain separate seams.
//! - **Extension event `signal` fields**: upstream attaches live `AbortSignal`
//!   objects to `session_before_compact`/`session_before_tree` events; the
//!   JSON event seam carries no signal (extensions receive JSON), so the
//!   field is omitted.
//! - **Tool-loop continuation.** The former repeated state-lock acquisition
//!   in `prepare_next_turn_with_context` is removed by taking one snapshot.
//!   Native CLI subprocess tests cover successful and failed read calls,
//!   their second local provider request, and terminal agent_end delivery.

pub mod base_tools;
pub mod bash_executor;
pub mod prompt_templates;
pub mod system_prompt;
pub mod tool_result_images;

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use futures::future::BoxFuture;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::agent_core::agent::{Agent, PromptInput, QueueMode};
use crate::agent_core::agent_loop::{
    AfterToolCallContext, AgentContext, AgentLoopTurnUpdate, BeforeToolCallContext,
    BeforeToolCallHook, BeforeToolCallOutcome, PrepareNextTurnContext, PrepareNextTurnHook,
    TransformContextFn,
};
use crate::agent_core::types::{
    AfterToolCallResult, AgentEvent, AgentMessage, AgentTool, CustomAgentMessage, ThinkingLevel,
};
use crate::ai::models::get_supported_thinking_levels;
use crate::ai::now_ms;
use crate::ai::overflow::{is_context_overflow, is_recoverable_length};
use crate::ai::retry::is_retryable_assistant_error;
use crate::ai::transcript::{content_text_with_separator, get_current_system_message};
use crate::ai::types::content::{ImageContent, TextContent, ToolCall};
use crate::ai::types::message::{
    AssistantBlock, AssistantMessage, Sections, StringOrBlocks, SystemMessage, TextOrImageBlock,
    ToolResultMessage, UserMessage,
};
use crate::ai::types::model::Model;
use crate::ai::types::options::ProviderHeaders;
use crate::ai::types::primitives::{StopReason, Usage};
use crate::coding_agent::core::auth_guidance::{
    format_no_api_key_found_message, format_no_model_selected_message,
};
#[allow(clippy::wildcard_imports)] // the seam enum mirrors the session-manager union
use crate::coding_agent::core::compaction::SessionEntry as CompactionSessionEntry;
use crate::coding_agent::core::compaction::{
    calculate_context_tokens, collect_entries_for_branch_summary, estimate_context_tokens,
    estimate_tokens, generate_branch_summary, prepare_compaction, should_compact,
    CollectEntriesResult, CompactionPreparation, CompactionResult, CompactionSettings,
    GenerateBranchSummaryOptions, ReadonlySessionManager,
};
use crate::coding_agent::core::defaults::{DEFAULT_THINKING_LEVEL, THINKING_LEVEL_OPTIONS};
use crate::coding_agent::core::messages::{BashExecutionMessage, CustomMessage};
use crate::coding_agent::core::model_registry::ModelRegistry;
use crate::coding_agent::core::model_resolver::models_are_equal;
use crate::coding_agent::core::model_runtime::model_thinking_level_from_wire;
use crate::coding_agent::core::model_runtime::ModelRuntime;
use crate::coding_agent::core::provider_composer::ProviderConfigInput;
use crate::coding_agent::core::resource_loader::prompt_templates::PromptTemplate;
use crate::coding_agent::core::resource_loader::{
    DefaultResourceLoader, ResourceExtensionPaths, ResourcePathEntry,
};
use crate::coding_agent::core::settings_manager::{
    BranchSummarySettings, CompactionSettings as SettingsCompactionSettings, RetrySettings,
    SettingsManager,
};
use crate::coding_agent::core::virtual_models::is_virtual_model;
use crate::coding_agent::extensions::runner::emit_session_shutdown_event;
use crate::coding_agent::extensions::runner::ErrorListenerUnsubscribe;
use crate::coding_agent::extensions::runner::ExtensionRunner;
use crate::coding_agent::extensions::types::{
    create_synthetic_source_info, AbortSignal, BuildSystemPromptOptions, CompactHandler,
    CompactOptions, ContextUsage, ExtensionActions, ExtensionCommandContext,
    ExtensionCommandContextActions, ExtensionError, ExtensionErrorListener, ExtensionMode,
    ExtensionUI, FlagValue, GetContextUsageHandler, GetModelHandler, GetScopedModelsHandler,
    GetSessionNameHandler, GetSystemPromptHandler, GetSystemPromptOptionsHandler,
    GetThinkingLevelHandler, InputSource, IsIdleHandler, IsProjectTrustedHandler,
    NormalizedBuildSystemPromptOptions, OrderedMap, ProviderRegistryHandle, RegisteredTool,
    ResolvedCommand, ResourcesDiscoverReason, SendMessageHandler, SendMessageOptions,
    SendUserMessageHandler, SendUserMessageOptions, SetActiveToolsHandler, SetLabelHandler,
    SetSessionNameHandler, SetThinkingLevelHandler, SourceInfo, StreamingDelivery, ToolExposure,
    ToolInfo,
};
use crate::coding_agent::extensions::wrapper::wrap_registered_tools;
use crate::coding_agent::package_manager::{PathMetadata, PathMetadataOrigin, SourceScope};
use crate::coding_agent::session_manager::{
    get_latest_compaction_entry, SessionEntry, SessionHeader, SessionManager, SessionManagerError,
    UsageEntry, CURRENT_SESSION_VERSION,
};
use crate::coding_agent::utils::frontmatter::strip_frontmatter;
use crate::coding_agent::utils::paths::resolve_path;

mod event_wire;
pub use event_wire::PreservedAgentSessionEvent;

#[cfg(test)]
mod lower_half_tests;
#[cfg(test)]
mod tests;

// ============================================================================
// Slice boundary (upstream runtime/types.ts:18-23 convention, mirroring
// crate::agent_core::harness::agent_harness::SliceNotImplemented)
// ============================================================================

/// Raised for surface whose agent-session slice lands with W3.12 (tool
/// execution loop, compaction, suspend/resume — upstream
/// `agent-session.ts:1989-3625`). The payload names the operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SliceNotImplemented(pub String);

impl SliceNotImplemented {
    /// Name the deferred operation.
    pub fn new(operation: impl Into<String>) -> Self {
        SliceNotImplemented(operation.into())
    }
}

impl std::fmt::Display for SliceNotImplemented {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SliceNotImplemented: {} (agent-session lower half, W3.12)",
            self.0
        )
    }
}

impl std::error::Error for SliceNotImplemented {}

/// Session-level operation error: either the slice boundary or the exact
/// upstream `new Error(...)` message text.
#[derive(Debug)]
pub enum AgentSessionError {
    /// W3.12 boundary — see [`SliceNotImplemented`].
    Slice(SliceNotImplemented),
    /// The exact upstream error message.
    Upstream(String),
}

impl From<SliceNotImplemented> for AgentSessionError {
    fn from(slice: SliceNotImplemented) -> Self {
        AgentSessionError::Slice(slice)
    }
}

impl std::fmt::Display for AgentSessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentSessionError::Slice(slice) => write!(f, "{slice}"),
            AgentSessionError::Upstream(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for AgentSessionError {}

// ============================================================================
// Skill Block Parsing (agent-session.ts:123-148)
// ============================================================================

/// Parsed skill block from a user message (`ParsedSkillBlock`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedSkillBlock {
    pub name: String,
    pub location: String,
    pub content: String,
    /// Upstream `match[4]?.trim() || undefined` — absent (not `null`) when no
    /// user message follows the block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_message: Option<String>,
}

/// Upstream `parseSkillBlock` (agent-session.ts:139-148). Returns `None` when
/// the text doesn't contain a skill block. Oracle:
/// `tests/fixtures/agent_session_oracle/oracle.json` (`skill_block` cases).
pub fn parse_skill_block(text: &str) -> Option<ParsedSkillBlock> {
    static SKILL_BLOCK: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r#"(?s)^<skill name="([^"]+)" location="([^"]+)">\n(.*?)\n</skill>(?:\n\n([\s\S]+))?$"#,
        )
        .expect("valid regex")
    });
    let captures = SKILL_BLOCK.captures(text)?;
    let user_message = captures
        .get(4)
        .map(|matched| matched.as_str().trim().to_string())
        .filter(|trimmed| !trimmed.is_empty());
    Some(ParsedSkillBlock {
        name: captures[1].to_string(),
        location: captures[2].to_string(),
        content: captures[3].to_string(),
        user_message,
    })
}

// ============================================================================
// Events (agent-session.ts:150-195)
// ============================================================================

/// Upstream `AgentSessionEvent`: the core `AgentEvent` union with `agent_end`
/// extended by `willRetry`, plus the session-specific additions. Wire `type`
/// values are the upstream literals.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AgentSessionEvent {
    /// `agent_start`.
    AgentStart,
    /// `agent_end` with the retry projection.
    AgentEnd {
        /// Messages the run produced.
        messages: Vec<AgentMessage>,
        /// Whether the session will auto-retry after this end.
        will_retry: bool,
    },
    /// `agent_settled`.
    AgentSettled,
    /// `queue_update`.
    QueueUpdate {
        /// Pending steering messages.
        steering: Vec<String>,
        /// Pending follow-up messages.
        follow_up: Vec<String>,
    },
    /// `turn_start`.
    TurnStart,
    /// `turn_end`.
    TurnEnd {
        /// The completed assistant message.
        message: AgentMessage,
        /// Tool result messages appended for the turn.
        tool_results: Vec<ToolResultMessage>,
    },
    /// `message_start`.
    MessageStart {
        /// The message that started.
        message: AgentMessage,
    },
    /// `message_update`.
    MessageUpdate {
        /// The partial assistant message.
        message: AgentMessage,
        /// The underlying assistant stream event (JSON; pi-ai slice type).
        assistant_message_event: Value,
    },
    /// `message_end`.
    MessageEnd {
        /// The completed message.
        message: AgentMessage,
    },
    /// `tool_execution_start`.
    ToolExecutionStart {
        /// Id of the assistant toolCall block.
        tool_call_id: String,
        /// Tool name.
        tool_name: String,
        /// Validated tool arguments.
        args: Value,
    },
    /// `tool_execution_update`.
    ToolExecutionUpdate {
        /// Id of the assistant toolCall block.
        tool_call_id: String,
        /// Tool name.
        tool_name: String,
        /// Validated tool arguments.
        args: Value,
        /// The partial result the tool streamed.
        partial_result: Value,
    },
    /// `tool_execution_end`.
    ToolExecutionEnd {
        /// Id of the assistant toolCall block.
        tool_call_id: String,
        /// Tool name.
        tool_name: String,
        /// Final tool result.
        result: Value,
        /// Whether the result is treated as an error.
        is_error: bool,
    },
    /// `compaction_start`.
    CompactionStart {
        /// Why compaction started.
        reason: CompactionReason,
    },
    /// `entry_appended`.
    EntryAppended {
        /// The appended session entry.
        entry: SessionEntry,
    },
    /// `session_info_changed`.
    SessionInfoChanged {
        /// The new session display name.
        name: Option<String>,
    },
    /// `thinking_level_changed`.
    ThinkingLevelChanged {
        /// The new thinking level.
        level: ThinkingLevel,
    },
    /// `compaction_end`.
    CompactionEnd {
        /// Why compaction ran.
        reason: CompactionReason,
        /// The result, when compaction completed.
        result: Option<Value>,
        /// Whether compaction was aborted.
        aborted: bool,
        /// Whether a turn will retry after compaction.
        will_retry: bool,
        /// The failure message, when compaction failed.
        error_message: Option<String>,
    },
    /// `auto_retry_start`.
    AutoRetryStart {
        /// The upcoming attempt (1-based).
        attempt: u32,
        /// The configured maximum.
        max_attempts: i64,
        /// Backoff delay before the retry.
        delay_ms: u64,
        /// The error being retried.
        error_message: String,
    },
    /// `auto_retry_end`.
    AutoRetryEnd {
        /// Whether the retried turn eventually succeeded.
        success: bool,
        /// The attempt that settled.
        attempt: u32,
        /// The final error, when `success` is false.
        final_error: Option<String>,
    },
    /// `summarization_retry_scheduled`.
    SummarizationRetryScheduled {
        /// The upcoming attempt.
        attempt: u32,
        /// The configured maximum.
        max_attempts: i64,
        /// Backoff delay before the retry.
        delay_ms: u64,
        /// The error being retried.
        error_message: String,
    },
    /// `summarization_retry_attempt_start`.
    SummarizationRetryAttemptStart {
        /// What is being summarized.
        source: SummarizationRetrySource,
    },
    /// `summarization_retry_finished`.
    SummarizationRetryFinished,
    /// `bash_execution_update`.
    BashExecutionUpdate {
        /// Optional caller identifier.
        id: Option<String>,
        /// The streamed output delta.
        delta: String,
    },
    /// Lossless JSON ingress bridged from AgentEvent. Match on `kind()` in
    /// typed listeners, and serialize the original event for wire output.
    #[serde(untagged)]
    Preserved(PreservedAgentSessionEvent),
}

/// Upstream `"manual" | "threshold" | "overflow"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionReason {
    Manual,
    Threshold,
    Overflow,
}

/// Upstream `summarization_retry_attempt_start`'s `source` union.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SummarizationRetrySource {
    BranchSummary,
    Compaction {
        /// Why compaction is running.
        reason: CompactionReason,
    },
}

/// Listener function for agent session events (`AgentSessionEventListener`).
pub type AgentSessionEventListener = dyn Fn(&AgentSessionEvent) + Send + Sync;

/// The shared listener list behind [`AgentSession`] and its unsubscribe
/// handles.
type ListenerList = Vec<(u64, Arc<AgentSessionEventListener>)>;

/// Handle returned by [`AgentSession::subscribe`] (the upstream unsubscribe
/// closure).
pub struct AgentSessionUnsubscribe {
    listeners: Arc<Mutex<ListenerList>>,
    id: u64,
}

impl AgentSessionUnsubscribe {
    /// Remove the listener (upstream invoking the returned function).
    pub fn unsubscribe(self) {
        if let Ok(mut listeners) = self.listeners.lock() {
            listeners.retain(|(existing, _)| *existing != self.id);
        }
    }
}

// ============================================================================
// Types (agent-session.ts:197-307)
// ============================================================================

/// Upstream `withoutDeletedHeaders`: drop `null` header values.
fn without_deleted_headers(
    headers: Option<&std::collections::BTreeMap<String, Option<String>>>,
) -> Option<std::collections::BTreeMap<String, String>> {
    headers.map(|headers| {
        headers
            .iter()
            .filter(|(_, value)| value.is_some())
            .map(|(name, value)| (name.clone(), value.clone().unwrap_or_default()))
            .collect()
    })
}

/// Upstream `AgentSessionConfig`. The port shares ownership of the agent and
/// session manager (`Arc`), and the resource loader is the ported
/// [`DefaultResourceLoader`] behind a mutex (`extendResources`/`reload` need
/// `&mut`).
pub struct AgentSessionConfig {
    pub agent: Arc<Agent>,
    pub session_manager: Arc<Mutex<SessionManager>>,
    pub settings_manager: SettingsManager,
    pub cwd: String,
    /// Models to cycle through with Ctrl+P (from --models flag).
    pub scoped_models: Vec<ScopedModel>,
    /// Resource loader for extensions, skills, prompts, themes, context files,
    /// and system prompt.
    pub resource_loader: Arc<Mutex<DefaultResourceLoader>>,
    /// SDK custom tools registered outside extensions.
    pub custom_tools: Vec<Arc<crate::coding_agent::extensions::types::ToolDefinition>>,
    /// Canonical model/auth runtime used by coding-agent internals.
    pub model_runtime: ModelRuntime,
    /// Keeps the prompt cache entry of the last session request warm
    /// (upstream `config.cacheWarmer` — constructed by the session factory).
    pub cache_warmer: Option<Arc<crate::coding_agent::core::cache_warmer::CacheWarmer>>,
    /// Initial active built-in tool names. Default: [read, bash, edit, write].
    pub initial_active_tool_names: Option<Vec<String>>,
    /// Whether the initial tools come from the `defaultTools` setting. When
    /// true, reload activates tools newly added to the setting. Tools removed
    /// from it stay active (v1.0.0 `usesDefaultTools`).
    pub uses_default_tools: Option<bool>,
    /// Optional allowlist of tool names. When provided, only these tool names
    /// are exposed.
    pub allowed_tool_names: Option<Vec<String>>,
    /// Optional denylist of tool names. When provided, these tool names are
    /// not exposed.
    pub excluded_tool_names: Option<Vec<String>>,
    /// Override base tools (useful for custom runtimes), keyed by tool name in
    /// insertion order (upstream `Record<string, AgentTool>`).
    pub base_tools_override: Vec<Arc<AgentTool>>,
    /// Session start event metadata emitted when extensions bind to this
    /// runtime (the `session_start` event JSON). Default:
    /// `{"type":"session_start","reason":"startup"}`.
    pub session_start_event: Option<Value>,
    /// HTML export backend for [`AgentSession::export_to_html`] (the
    /// export-html seam — see the module docs). `None` makes HTML exports
    /// fail with the seam error.
    pub html_exporter: Option<HtmlExportFn>,
}

/// Upstream scoped-model entry `{ model, thinkingLevel? }`.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopedModel {
    pub model: Model,
    pub thinking_level: Option<ThinkingLevel>,
}

/// Upstream `routedModel` (`{ model, thinkingLevel? }`).
#[derive(Debug, Clone, PartialEq)]
pub struct RoutedModel {
    pub model: Model,
    pub thinking_level: Option<crate::ai::types::primitives::ModelThinkingLevel>,
}

/// Convert the cache warmer's usage entry to the session entry the
/// `entry_appended` event carries.
fn usage_entry_to_session_entry(
    entry: &crate::coding_agent::core::cache_warmer::UsageEntry,
) -> UsageEntry {
    UsageEntry {
        id: entry.id.clone(),
        parent_id: entry.parent_id.clone(),
        timestamp: entry.timestamp.clone(),
        kind: entry.kind.clone(),
        provider: entry.provider.clone(),
        model: entry.model.clone(),
        usage: entry.usage,
        note: entry.note.clone(),
    }
}

/// Options for [`AgentSession::prompt`] (`PromptOptions`).
#[derive(Clone, Default)]
pub struct PromptOptions {
    /// Whether to dispatch extension commands and expand skill commands and
    /// prompt templates (default: true).
    pub expand_prompt_templates: Option<bool>,
    /// Image attachments.
    pub images: Option<Vec<ImageContent>>,
    /// When streaming, how to queue the message. Required if streaming.
    pub streaming_behavior: Option<StreamingDelivery>,
    /// Source of input for extension input event handlers.
    pub source: Option<InputSource>,
    /// Internal hook used by RPC mode to observe how an accepted prompt was
    /// dispatched. Not called if the prompt is rejected.
    pub preflight_result: Option<Arc<dyn Fn(PromptDisposition) + Send + Sync>>,
    /// Optional host cancellation retained through preflight and checked at
    /// the actual native run boundary. Existing callers retain default behavior.
    pub start_cancellation: Option<CancellationToken>,
}

/// How an accepted prompt was dispatched (`PromptDisposition` /
/// `QueuedInputDisposition`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptDisposition {
    /// A handler consumed the input; nothing is queued or started.
    Handled,
    /// The input joined the steering/follow-up queue.
    Queued,
    /// The input started an agent run.
    Started,
}

impl PromptDisposition {
    /// The upstream literal.
    pub fn as_str(&self) -> &'static str {
        match self {
            PromptDisposition::Handled => "handled",
            PromptDisposition::Queued => "queued",
            PromptDisposition::Started => "started",
        }
    }
}

/// Options for model/thinking mutations (`ModelMutationOptions`).
#[derive(Debug, Clone, Copy, Default)]
pub struct ModelMutationOptions {
    /// Persist the new value to global defaults. Defaults to session-only.
    pub persist: bool,
}

/// Result from [`AgentSession::cycle_model`] (`ModelCycleResult`).
#[derive(Debug, Clone, PartialEq)]
pub struct ModelCycleResult {
    pub model: Model,
    pub thinking_level: ThinkingLevel,
    /// Whether cycling through scoped models (--models flag) or all available.
    pub is_scoped: bool,
}

/// Per-tool registry entry (`ToolDefinitionEntry`).
#[derive(Clone)]
struct ToolDefinitionEntry {
    definition: Arc<crate::coding_agent::extensions::types::ToolDefinition>,
    source_info: SourceInfo,
}

fn estimate_messages_tokens(messages: &[AgentMessage]) -> u64 {
    messages.iter().map(estimate_tokens).sum()
}

// ============================================================================
// AgentSession Class
// ============================================================================

/// Core abstraction for agent lifecycle and session management — see the
/// module docs for the slice boundary and seams.
/// Pisper host seam: wraps executable registry entries after native extension wrapping.
/// The adapter must preserve the tool declaration, result/error, and cancellation signal.
pub type ToolExecutionWrapper = dyn Fn(AgentTool) -> AgentTool + Send + Sync;

pub struct AgentSession {
    /// The agent (upstream readonly field).
    pub agent: Arc<Agent>,
    /// The session manager (upstream readonly field). Shared via `Arc<Mutex>`
    /// because the runner handle and the event handler mutate it.
    pub session_manager: Arc<Mutex<SessionManager>>,
    /// The settings manager (upstream readonly field).
    pub settings_manager: SettingsManager,

    scoped_models: Mutex<Vec<ScopedModel>>,

    // Event subscription state
    unsubscribe_agent: Mutex<Option<crate::agent_core::agent::Unsubscribe>>,
    event_listeners: Arc<Mutex<ListenerList>>,
    next_listener_id: AtomicU64,
    is_agent_run_active: AtomicBool,
    /// Session-level idle signal (`_idleWaitPromise`/`_resolveIdleWait` as a
    /// watch channel; `true` = idle settled).
    idle_tx: watch::Sender<bool>,
    idle_rx: watch::Receiver<bool>,

    /// Tracks pending steering messages for UI display. Removed when
    /// delivered.
    steering_messages: Mutex<Vec<String>>,
    /// Tracks pending follow-up messages for UI display. Removed when
    /// delivered.
    follow_up_messages: Mutex<Vec<String>>,
    /// Includes consumed queued user input until its MessageEnd is persisted.
    in_flight_user_messages: Mutex<Vec<String>>,
    /// A failed append must not be mistaken for durable delivery or retried as
    /// another paid model request in this runtime.
    message_persistence_error: Mutex<Option<String>>,
    /// Messages queued to be included with the next user prompt ("asides").
    pending_next_turn_messages: Mutex<Vec<AgentMessage>>,
    /// Context-only custom messages queued during a run, flushed once the
    /// current turn's tool results are in.
    pending_custom_messages: Mutex<Vec<AgentMessage>>,

    // Compaction state
    compaction_abort: Mutex<Option<CancellationToken>>,
    auto_compaction_abort: Mutex<Option<CancellationToken>>,
    overflow_recovery_attempted: AtomicBool,

    // Branch summarization state
    branch_summary_abort: Mutex<Option<CancellationToken>>,

    // Retry state
    retry_abort: Mutex<Option<CancellationToken>>,
    retry_attempt: AtomicU32,
    /// Set by `abort()` while a run is active; the post-run loop checks it
    /// instead of continuing (upstream `_agentRunAbortRequested`).
    agent_run_abort_requested: AtomicBool,

    // Bash execution state
    bash_abort_controllers: Mutex<Vec<Arc<CancellationToken>>>,
    pending_bash_messages: Mutex<Vec<BashExecutionMessage>>,

    /// HTML export backend (the export-html seam; see module docs).
    html_exporter: Mutex<Option<HtmlExportFn>>,

    // Extension system
    extension_runner: Mutex<Option<ExtensionRunner>>,
    turn_index: AtomicU64,

    resource_loader: Arc<Mutex<DefaultResourceLoader>>,
    custom_tools: Vec<Arc<crate::coding_agent::extensions::types::ToolDefinition>>,
    base_tool_definitions:
        Mutex<OrderedMap<Arc<crate::coding_agent::extensions::types::ToolDefinition>>>,
    cwd: String,
    initial_active_tool_names: Option<Vec<String>>,
    /// Tools of the restored or reloaded loadout that are not registered yet,
    /// such as tools of MCP servers that are still connecting. They are
    /// activated when they are registered, and dropped when
    /// [`Self::set_active_tools_by_name`] deactivates a tool or the next
    /// agent run starts (v1.0.0 `_pendingToolNames`).
    pending_tool_names: Mutex<std::collections::BTreeSet<String>>,
    /// Upstream `_usesDefaultTools` (v1.0.0).
    uses_default_tools: bool,
    allowed_tool_names: Option<HashSet<String>>,
    excluded_tool_names: Option<HashSet<String>>,
    base_tools_override: Option<Vec<Arc<AgentTool>>>,
    /// Emitted by `bindExtensions` (upstream `_sessionStartEvent`).
    session_start_event: Value,
    extension_ui_context: Mutex<Option<Arc<dyn ExtensionUI>>>,
    extension_mode: Mutex<ExtensionMode>,
    extension_command_context_actions: Mutex<Option<ExtensionCommandContextActions>>,
    extension_abort_handler: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    extension_shutdown_handler: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    extension_error_listener: Mutex<Option<ExtensionErrorListener>>,
    extension_error_unsubscriber: Mutex<Option<ErrorListenerUnsubscribe>>,

    model_runtime: ModelRuntime,
    /// Cache warmer configured for this session (upstream `_cacheWarmer`).
    cache_warmer: Option<Arc<crate::coding_agent::core::cache_warmer::CacheWarmer>>,
    /// Set while the `agent_settled` extension/listener emission is running;
    /// prompts arriving in that window are deferred (upstream
    /// `_isEmittingAgentSettled` + `_deferredSettledActions`). The port stores
    /// the queued prompts and replays them on a blocking thread after the
    /// settled emission (the prompt future is not `Send`, so the replay
    /// cannot be awaited inline).
    is_emitting_agent_settled: AtomicBool,
    deferred_settled_prompts: Mutex<Vec<(String, PromptOptions)>>,

    // Tool registry for extension getTools/setTools
    tool_registry: Mutex<OrderedMap<Arc<AgentTool>>>,
    tool_execution_wrapper: Mutex<Option<Arc<ToolExecutionWrapper>>>,
    tool_definitions: Mutex<OrderedMap<ToolDefinitionEntry>>,
    tool_prompt_snippets: Mutex<OrderedMap<String>>,
    tool_prompt_guidelines: Mutex<OrderedMap<Vec<String>>>,

    base_system_prompt_options: Mutex<NormalizedBuildSystemPromptOptions>,
    /// Prompt options after before_agent_start mutations for the active run.
    run_system_prompt_options: Mutex<Option<NormalizedBuildSystemPromptOptions>>,

    /// Track the last assistant message for auto-compaction check.
    last_assistant_message: Mutex<Option<AssistantMessage>>,

    /// Projected message → persisted entry id (upstream
    /// `_entryIdsByMessage` WeakMap; the port keys by message value).
    persisted_entry_ids_by_message: Mutex<Vec<(AgentMessage, String)>>,

    /// This session's own `Arc` (set by [`AgentSession::new`]; runner-bound
    /// closures capture clones of it so `dispose` breaks cycles).
    self_weak: Mutex<Weak<AgentSession>>,
}

impl AgentSession {
    /// A cloned weak handle to this session.
    fn self_weak(&self) -> Weak<AgentSession> {
        self.self_weak.lock().expect("self weak lock").clone()
    }
}

/// `_buildRuntime(options)` parameter struct (upstream object parameter).
#[derive(Debug, Default)]
struct BuildRuntimeOptions {
    active_tool_names: Option<Vec<String>>,
    flag_values: Option<OrderedMap<FlagValue>>,
    include_all_extension_tools: bool,
}

/// `_refreshToolRegistry(options)` parameter struct.
#[derive(Debug, Default)]
struct RefreshToolRegistryOptions {
    active_tool_names: Option<Vec<String>>,
    include_all_extension_tools: Option<bool>,
}

impl AgentSession {
    /// Upstream `constructor(config)` plus the installs it performs: the
    /// agent-event subscription, tool hooks, next-turn refresh, forced prompt
    /// projection, the runtime build, and transcript tool restore. Returns the
    /// `Arc` the async machinery needs (see the architecture seam in the
    /// module docs).
    pub fn new(config: AgentSessionConfig) -> Result<Arc<Self>, anyhow::Error> {
        let AgentSessionConfig {
            agent,
            session_manager,
            settings_manager,
            cwd,
            scoped_models,
            resource_loader,
            custom_tools,
            model_runtime,
            cache_warmer,
            initial_active_tool_names,
            uses_default_tools,
            allowed_tool_names,
            excluded_tool_names,
            base_tools_override,
            session_start_event,
            html_exporter,
        } = config;

        let (idle_tx, idle_rx) = watch::channel(false);
        let session = Arc::new(Self {
            agent,
            session_manager,
            settings_manager,
            scoped_models: Mutex::new(scoped_models),
            unsubscribe_agent: Mutex::new(None),
            event_listeners: Arc::new(Mutex::new(Vec::new())),
            next_listener_id: AtomicU64::new(0),
            is_agent_run_active: AtomicBool::new(false),
            idle_tx,
            idle_rx,
            steering_messages: Mutex::new(Vec::new()),
            follow_up_messages: Mutex::new(Vec::new()),
            in_flight_user_messages: Mutex::new(Vec::new()),
            message_persistence_error: Mutex::new(None),
            pending_next_turn_messages: Mutex::new(Vec::new()),
            pending_custom_messages: Mutex::new(Vec::new()),
            compaction_abort: Mutex::new(None),
            auto_compaction_abort: Mutex::new(None),
            overflow_recovery_attempted: AtomicBool::new(false),
            branch_summary_abort: Mutex::new(None),
            retry_abort: Mutex::new(None),
            retry_attempt: AtomicU32::new(0),
            agent_run_abort_requested: AtomicBool::new(false),
            bash_abort_controllers: Mutex::new(Vec::new()),
            pending_bash_messages: Mutex::new(Vec::new()),
            html_exporter: Mutex::new(html_exporter),
            extension_runner: Mutex::new(None),
            turn_index: AtomicU64::new(0),
            resource_loader,
            custom_tools,
            base_tool_definitions: Mutex::new(OrderedMap::new()),
            cwd,
            initial_active_tool_names,
            pending_tool_names: Mutex::new(std::collections::BTreeSet::new()),
            uses_default_tools: uses_default_tools.unwrap_or(false),
            allowed_tool_names: allowed_tool_names.map(|names| names.into_iter().collect()),
            excluded_tool_names: excluded_tool_names.map(|names| names.into_iter().collect()),
            base_tools_override: if base_tools_override.is_empty() {
                None
            } else {
                Some(base_tools_override)
            },
            session_start_event: session_start_event
                .unwrap_or_else(|| json!({"type": "session_start", "reason": "startup"})),
            extension_ui_context: Mutex::new(None),
            extension_mode: Mutex::new(ExtensionMode::Print),
            extension_command_context_actions: Mutex::new(None),
            extension_abort_handler: Mutex::new(None),
            extension_shutdown_handler: Mutex::new(None),
            extension_error_listener: Mutex::new(None),
            extension_error_unsubscriber: Mutex::new(None),
            model_runtime,
            cache_warmer,
            is_emitting_agent_settled: AtomicBool::new(false),
            deferred_settled_prompts: Mutex::new(Vec::new()),
            tool_registry: Mutex::new(OrderedMap::new()),
            tool_execution_wrapper: Mutex::new(None),
            tool_definitions: Mutex::new(OrderedMap::new()),
            tool_prompt_snippets: Mutex::new(OrderedMap::new()),
            tool_prompt_guidelines: Mutex::new(OrderedMap::new()),
            base_system_prompt_options: Mutex::new(
                system_prompt::normalize_build_system_prompt_options(
                    &BuildSystemPromptOptions::with_cwd(""),
                ),
            ),
            run_system_prompt_options: Mutex::new(None),
            last_assistant_message: Mutex::new(None),
            persisted_entry_ids_by_message: Mutex::new(Vec::new()),
            self_weak: Mutex::new(Weak::new()),
        });
        *session.self_weak.lock().expect("self weak lock") = Arc::downgrade(&session);

        // Upstream: `this._cacheWarmer.onWarmed = (entry) => this._emit({
        // type: "entry_appended", entry })`. The warmer's usage entry converts
        // to the session entry the event carries.
        if let Some(cache_warmer) = &session.cache_warmer {
            let session_weak = session.self_weak();
            cache_warmer.set_on_warmed(Box::new(move |entry| {
                if let Some(session) = session_weak.upgrade() {
                    session.emit(AgentSessionEvent::EntryAppended {
                        entry: SessionEntry::Usage(usage_entry_to_session_entry(entry)),
                    });
                }
            }));
        }

        // Always subscribe to agent events for internal handling (session
        // persistence, extensions, auto-compaction, retry logic).
        *session.unsubscribe_agent.lock().expect("unsubscribe lock") =
            Some(subscribe_agent_handler(&session));
        session.install_agent_tool_hooks();
        session.install_agent_next_turn_refresh();
        session.install_agent_forced_prompt_projection();

        session.build_runtime(BuildRuntimeOptions {
            active_tool_names: session.initial_active_tool_names.clone(),
            flag_values: None,
            include_all_extension_tools: true,
        });
        if session.initial_active_tool_names.is_none() {
            session.restore_tools_from_transcript();
        }

        Ok(session)
    }

    /// Upstream `get modelRuntime`.
    pub fn model_runtime(&self) -> &ModelRuntime {
        &self.model_runtime
    }

    /// The extension runner built by `_buildRuntime` (upstream
    /// `get extensionRunner`).
    pub fn extension_runner(&self) -> ExtensionRunner {
        self.extension_runner
            .lock()
            .expect("extension runner lock")
            .clone()
            .expect("extension runner built by the constructor")
    }

    /// Upstream `refreshContext`: refresh the public finalized transcript
    /// from the canonical session projection.
    pub fn refresh_context(&self) {
        self.refresh_finalized_context();
    }

    /// Upstream `_refreshFinalizedContext`: rebuild `agent.state.messages`
    /// from the session projection, recording each projected message's source
    /// entry id.
    fn refresh_finalized_context(&self) {
        let projection = {
            let manager = self.session_manager.lock().expect("session lock");
            manager.build_session_projection()
        };
        *self
            .persisted_entry_ids_by_message
            .lock()
            .expect("entry id map lock") = projection
            .entries
            .iter()
            .flat_map(|entry| {
                entry.messages.iter().map(|message| {
                    (
                        message.clone(),
                        entry.source_entry.id().unwrap_or_default().to_string(),
                    )
                })
            })
            .collect();
        self.agent.state().messages = projection.messages;
    }

    /// Upstream `get cacheWarmingStatus`: current cache-warming state and the
    /// policy inputs that produced it.
    pub fn cache_warming_status(
        &self,
    ) -> Option<crate::coding_agent::core::cache_warmer::CacheWarmingStatus> {
        self.cache_warmer
            .as_ref()
            .map(|cache_warmer| cache_warmer.status())
    }

    /// Upstream `_recordSelection`: record the selection on the current
    /// branch when the branch implies another one, so a resume restores it.
    fn record_selection(&self) {
        let Some(model) = self.model() else {
            return;
        };
        let runtime = self.model_runtime.clone();
        let get_model =
            move |provider: &str, model_id: &str| runtime.get_model_sync(provider, model_id);
        let branch = {
            let manager = self.session_manager.lock().expect("session lock");
            manager.get_branch(None)
        };
        let Some(recorded) =
            crate::coding_agent::core::virtual_models::get_branch_selection(&branch, &get_model)
        else {
            return;
        };
        if recorded.provider == model.provider && recorded.model_id == model.id {
            return;
        }
        let recorded_model = self
            .model_runtime
            .get_model_sync(&recorded.provider, &recorded.model_id);
        if !is_virtual_model(&model)
            && !recorded_model
                .as_ref()
                .map(is_virtual_model)
                .unwrap_or(false)
        {
            return;
        }
        let _ = self
            .session_manager
            .lock()
            .expect("session lock")
            .append_model_change(&model.provider, &model.id);
    }

    /// Upstream `_modelForMessage`: the model whose limits apply to
    /// `message`, or `None` when the message came from another model. Under a
    /// virtual selection, that is the physical model that produced it.
    fn model_for_message(&self, message: &AssistantMessage) -> Option<Model> {
        let model = self.model()?;
        if is_virtual_model(&model) {
            return self
                .model_runtime
                .get_physical_model(&message.provider, &message.model);
        }
        if model.provider == message.provider && model.id == message.model {
            Some(model)
        } else {
            None
        }
    }

    /// Upstream `_limitsModel`: the model whose limits apply to the
    /// conversation (the routed physical model under a virtual selection).
    fn limits_model(&self) -> Option<Model> {
        self.routed_model()
            .map(|routed| routed.model)
            .or_else(|| self.model())
    }

    /// Upstream `get routedModel`: under a virtual selection, the physical
    /// model and thinking level of the latest successful response.
    pub fn routed_model(&self) -> Option<RoutedModel> {
        let model = self.model()?;
        if !is_virtual_model(&model) {
            return None;
        }
        let latest = {
            let state = self.agent.state();
            crate::coding_agent::core::virtual_models::find_latest_response(&state.messages)?
                .clone()
        };
        let physical = self
            .model_runtime
            .get_physical_model(&latest.provider, &latest.model)?;
        Some(RoutedModel {
            model: physical,
            thinking_level: latest
                .provider_thinking_level
                .as_deref()
                .and_then(model_thinking_level_from_wire),
        })
    }

    /// Upstream `_findPersistedMessageEntryId`.
    fn find_persisted_message_entry_id(&self, message: &AgentMessage) -> Option<String> {
        {
            let persisted = self
                .persisted_entry_ids_by_message
                .lock()
                .expect("entry id map lock");
            if let Some((_, mapped)) = persisted.iter().find(|(candidate, _)| candidate == message)
            {
                return Some(mapped.clone());
            }
        }
        {
            let manager = self.session_manager.lock().expect("session lock");
            for entry in manager.get_branch(None).iter().rev() {
                if let SessionEntry::Message(entry_message) = entry {
                    if &entry_message.message == message {
                        return entry.id().map(str::to_string);
                    }
                }
            }
        }
        let message_index = self
            .agent
            .state()
            .messages
            .iter()
            .position(|candidate| candidate == message)?;
        let projection = {
            let manager = self.session_manager.lock().expect("session lock");
            manager.build_session_projection()
        };
        let mut projected_index = 0usize;
        for entry in &projection.entries {
            for _ in entry.messages.iter() {
                if projected_index == message_index {
                    return entry.source_entry.id().map(str::to_string);
                }
                projected_index += 1;
            }
        }
        None
    }

    /// Upstream `_omitRecoveryAttempt`: durably omit a failed attempt (and
    /// its tool results) from the model projection with context edits.
    fn omit_recovery_attempt(
        &self,
        message: &AgentMessage,
        tool_results: &[AgentMessage],
    ) -> Result<(), AgentSessionError> {
        let mut targets = vec![message.clone()];
        targets.extend(tool_results.iter().cloned());
        let target_ids: Vec<Option<String>> = targets
            .iter()
            .map(|target| self.find_persisted_message_entry_id(target))
            .collect();
        let state_contains = |target: &AgentMessage| self.agent.state().messages.contains(target);
        let unresolved = targets
            .iter()
            .zip(&target_ids)
            .any(|(target, target_id)| target_id.is_none() && state_contains(target));
        if unresolved {
            return Err(AgentSessionError::Upstream(String::from(
                "Cannot persist recovery omission because a projected message has no source entry",
            )));
        }
        for target_id in target_ids.into_iter().flatten() {
            let entry = {
                let mut manager = self.session_manager.lock().expect("session lock");
                let edit_id = manager
                    .append_context_edit(&target_id, None)
                    .map_err(session_manager_error)?;
                manager.get_entry(&edit_id).cloned()
            };
            if let Some(entry) = entry {
                self.emit(AgentSessionEvent::EntryAppended { entry });
            }
        }
        self.refresh_finalized_context();
        Ok(())
    }

    /// Upstream `_finishCancelledRetry`: close out an in-flight retry counter
    /// after an abort.
    fn finish_cancelled_retry(&self) {
        let attempt = self.retry_attempt.load(Ordering::SeqCst);
        if attempt == 0 {
            return;
        }
        self.retry_attempt.store(0, Ordering::SeqCst);
        self.emit(AgentSessionEvent::AutoRetryEnd {
            success: false,
            attempt,
            final_error: Some("Retry cancelled".to_string()),
        });
    }

    /// Upstream `summarizeForBugReport`: ask the current model to describe
    /// what went wrong in this session for a bug report.
    pub async fn summarize_for_bug_report(
        &self,
        hint: Option<&str>,
        signal: CancellationToken,
    ) -> Result<String, AgentSessionError> {
        let Some(model) = self.model() else {
            return Err(AgentSessionError::Upstream(String::from(
                "No model selected",
            )));
        };
        let auth = self.get_summarization_request_auth(&model).await?;
        let messages = self.messages();
        // The session's summarization headers carry plain string values; the
        // bug-report seam takes the provider header shape (`string | null`
        // values).
        let report_headers: Option<std::collections::BTreeMap<String, Option<String>>> =
            auth.headers.as_ref().map(|headers| {
                headers
                    .iter()
                    .map(|(name, value)| (name.clone(), Some(value.clone())))
                    .collect()
            });
        crate::coding_agent::core::bug_report::generate_bug_report_summary(
            crate::coding_agent::core::bug_report::GenerateBugReportSummaryOptions {
                messages: &messages,
                hint,
                model: &auth.model,
                api_key: auth.api_key.as_deref(),
                headers: report_headers.as_ref(),
                env: auth.env.as_ref(),
                signal,
                thinking_level: None,
                stream_fn: None,
                retry: None,
                session_id: Some(&self.session_id()),
            },
        )
        .await
        .map_err(AgentSessionError::Upstream)
    }

    /// Upstream `_getRequiredRequestAuth`.
    async fn get_required_request_auth(
        &self,
        model: &Model,
    ) -> Result<RequiredRequestAuth, AgentSessionError> {
        let result = self
            .model_runtime
            .get_auth(
                crate::coding_agent::core::model_runtime::ProviderOrModel::Model(model),
                None,
            )
            .await;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                if error.to_string() == "authHeader requires a resolved API key" {
                    return Err(AgentSessionError::Upstream(
                        format_no_api_key_found_message(&model.provider),
                    ));
                }
                return Err(AgentSessionError::Upstream(error.to_string()));
            }
        };
        if let Some(result) = result {
            if result.auth.api_key.is_some() || result.auth.headers.is_some() {
                let mut request_model = model.clone();
                if let Some(base_url) = &result.auth.base_url {
                    request_model.base_url = base_url.clone();
                }
                return Ok(RequiredRequestAuth {
                    model: request_model,
                    api_key: result.auth.api_key.clone(),
                    headers: without_deleted_headers(result.auth.headers.as_ref()),
                    env: result.env.clone(),
                });
            }
        }

        let is_oauth = self.model_runtime.is_using_oauth(&model.provider);
        if is_oauth {
            return Err(AgentSessionError::Upstream(format!(
                "Authentication failed for \"{}\". Credentials may have expired or network is \
                 unavailable. Run '/login {}' to re-authenticate.",
                model.provider, model.provider
            )));
        }
        Err(AgentSessionError::Upstream(
            format_no_api_key_found_message(&model.provider),
        ))
    }

    /// Upstream `_getSummarizationRequestAuth`. The port's agents always use
    /// the default transport (see the `streamFunction` seam in the module
    /// docs), so this always takes the strict `streamSimple` branch.
    async fn get_summarization_request_auth(
        &self,
        model: &Model,
    ) -> Result<RequiredRequestAuth, AgentSessionError> {
        self.get_required_request_auth(model).await
    }

    // =========================================================================
    // Install hooks (agent-session.ts:482-598, 1141-1156)
    // =========================================================================

    /// Upstream `_installAgentToolHooks`: install the tool hooks once on the
    /// Agent instance. The callbacks read the extension runner at execution
    /// time, so extension reload swaps in the new runner without reinstalling
    /// hooks.
    fn install_agent_tool_hooks(&self) {
        let before_session = self.self_weak();
        let before_hook: Arc<BeforeToolCallHook> =
            Arc::new(move |context: BeforeToolCallContext| {
                let session = before_session.clone();
                Box::pin(async move {
                    match session.upgrade() {
                        Some(session) => {
                            session
                                .before_tool_call(&context.tool_call, &context.args)
                                .await
                        }
                        None => BeforeToolCallOutcome::default(),
                    }
                })
            });

        let after_session = self.self_weak();
        let after_hook: Arc<crate::agent_core::agent_loop::AfterToolCallHook> =
            Arc::new(move |context: AfterToolCallContext| {
                let session = after_session.clone();
                Box::pin(async move {
                    match session.upgrade() {
                        Some(session) => {
                            session
                                .after_tool_call(
                                    &context.tool_call,
                                    &context.args,
                                    &context.result,
                                    context.is_error,
                                )
                                .await
                        }
                        None => None,
                    }
                })
            });

        let mut runtime = self.agent.runtime();
        runtime.before_tool_call = Some(before_hook);
        runtime.after_tool_call = Some(after_hook);
    }

    /// The `beforeToolCall` hook body (upstream agent-session.ts:491-510).
    async fn before_tool_call(&self, tool_call: &ToolCall, args: &Value) -> BeforeToolCallOutcome {
        let runner = self.extension_runner();
        if !runner.has_handlers("tool_call") {
            return BeforeToolCallOutcome::default();
        }

        let mut event = json!({
            "type": "tool_call",
            "toolName": tool_call.name,
            "toolCallId": tool_call.id,
            "input": args,
        });
        match runner.emit_tool_call(&mut event).await {
            Ok(result) => {
                // Handlers may mutate `event.input` in place (upstream passes
                // the args object by reference).
                let mutated_args = event.get("input").cloned().filter(|input| input != args);
                let result = result.and_then(|value| {
                    serde_json::from_value::<crate::agent_core::types::BeforeToolCallResult>(value)
                        .ok()
                });
                BeforeToolCallOutcome {
                    args: mutated_args,
                    result,
                }
            }
            // Upstream rethrows Errors verbatim and wraps non-Error
            // throwables ("Extension failed, blocking execution: …"); the
            // hook carries no error channel, so blocking failures panic with
            // the exact message.
            Err(error) => panic!("{error}"),
        }
    }

    /// The `afterToolCall` hook body (upstream agent-session.ts:512-543).
    async fn after_tool_call(
        &self,
        tool_call: &ToolCall,
        args: &Value,
        result: &crate::agent_core::types::AgentToolResult,
        is_error: bool,
    ) -> Option<AfterToolCallResult> {
        let runner = self.extension_runner();
        let hook_result = if runner.has_handlers("tool_result") {
            let event = json!({
                "type": "tool_result",
                "toolName": tool_call.name,
                "toolCallId": tool_call.id,
                "input": args,
                "content": serde_json::to_value(&result.content).unwrap_or(Value::Null),
                "details": result.details.clone(),
                "isError": is_error,
                "usage": result.usage.clone(),
            });
            runner
                .emit_tool_result(&event)
                .await
                .and_then(|value| serde_json::from_value::<AfterToolCallResult>(value).ok())
        } else {
            None
        };

        let content = hook_result
            .as_ref()
            .and_then(|hook| hook.content.clone())
            .unwrap_or_else(|| result.content.clone());
        // Runs after the extension hook so images injected or replaced by
        // extensions are normalized too.
        let normalized_content = tool_result_images::normalize_tool_result_images(
            content,
            Some(tool_result_images::NormalizeToolResultImagesOptions {
                auto_resize_images: Some(self.settings_manager.get_image_auto_resize()),
            }),
        )
        .await;

        if hook_result.is_none() && normalized_content == result.content {
            return None;
        }

        Some(AfterToolCallResult {
            content: Some(normalized_content),
            details: hook_result.as_ref().and_then(|hook| hook.details.clone()),
            is_error: hook_result.as_ref().and_then(|hook| hook.is_error),
            usage: hook_result.as_ref().and_then(|hook| hook.usage),
            terminate: None,
        })
    }

    /// Upstream `_compactBeforeNextAssistantResponse` — the threshold check
    /// before the next assistant response.
    async fn compact_before_next_assistant_response(
        &self,
        context: AgentContext,
    ) -> Result<AgentContext, AgentSessionError> {
        let model = self.model();
        let settings = self.compaction_settings_for(model.as_ref())?;

        let should = match &model {
            Some(model) if model.context_window > 0 => {
                let estimate = estimate_context_tokens(&context.messages);
                should_compact(estimate.tokens, model.context_window, settings)
            }
            _ => false,
        };
        if !should {
            return Ok(context);
        }

        self.run_auto_compaction(CompactionReason::Threshold, false)
            .await?;
        Ok(AgentContext {
            tools: context.tools,
            messages: self.agent.state().messages.clone(),
        })
    }

    /// Upstream `_installAgentNextTurnRefresh`.
    fn install_agent_next_turn_refresh(&self) {
        let previous = self.agent.runtime().prepare_next_turn.clone();
        let session = self.self_weak();
        let hook: Arc<PrepareNextTurnHook> = Arc::new(move |turn: PrepareNextTurnContext| {
            let session = session.clone();
            let previous = previous.clone();
            Box::pin(async move {
                match session.upgrade() {
                    Some(session) => session.prepare_next_turn_with_context(turn, previous).await,
                    None => None,
                }
            })
        });
        self.agent.runtime().prepare_next_turn = Some(hook);
    }

    async fn prepare_next_turn_with_context(
        &self,
        turn: PrepareNextTurnContext,
        previous: Option<Arc<PrepareNextTurnHook>>,
    ) -> Option<AgentLoopTurnUpdate> {
        let context = match self
            .compact_before_next_assistant_response(turn.context.clone())
            .await
        {
            Ok(context) => context,
            // The loop hook has no error channel (upstream propagates the
            // exception through the loop); the W3.12 boundary stays loud.
            Err(error) => panic!("{error}"),
        };
        let previous_snapshot = match &previous {
            Some(previous) => {
                let mut forwarded = turn.clone();
                forwarded.context = context.clone();
                previous(forwarded).await
            }
            None => None,
        };
        let next_context = previous_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.context.clone())
            .unwrap_or(context);
        let run_options = self
            .run_system_prompt_options
            .lock()
            .expect("run options lock")
            .clone()
            .unwrap_or_else(|| self.base_system_prompt_options().clone());
        let base = self.base_system_prompt_options().clone();
        let mut merged = to_build_options(&run_options);
        merged.selected_tools = Some(self.get_active_tool_names());
        let mut tool_snippets = base.tool_snippets.clone();
        tool_snippets.extend(run_options.tool_snippets.clone());
        merged.tool_snippets = Some(tool_snippets);
        let mut tool_guidelines = base.tool_guidelines.clone();
        tool_guidelines.extend(run_options.tool_guidelines.clone());
        merged.tool_guidelines = Some(tool_guidelines);
        let options = system_prompt::normalize_build_system_prompt_options(&merged);
        let update_message =
            self.prepare_prompt_and_tool_loadout(&options, Some(&next_context.messages));
        // Keep session.systemPrompt and ctx.getSystemPrompt() in step with
        // what the provider sees.
        *self
            .run_system_prompt_options
            .lock()
            .expect("run options lock") = Some(options);
        // Take one snapshot. MutexGuard temporaries in a struct initializer
        // live until the whole expression ends; separately calling state()
        // for tools/model/thinking_level here deadlocks on the second lock.
        let (tools, model, thinking_level) = {
            let state = self.agent.state();
            (
                state.tools.clone(),
                state.model.clone(),
                state.thinking_level,
            )
        };
        Some(AgentLoopTurnUpdate {
            context: Some(AgentContext {
                tools,
                messages: next_context.messages.clone(),
            }),
            messages: match update_message {
                Some(message) => {
                    let mut messages = previous_snapshot
                        .as_ref()
                        .and_then(|snapshot| snapshot.messages.clone())
                        .unwrap_or_default();
                    messages.push(AgentMessage::System(message));
                    Some(messages)
                }
                None => previous_snapshot.and_then(|snapshot| snapshot.messages),
            },
            model: Some(model),
            thinking_level: Some(thinking_level),
        })
    }

    /// Upstream `_installAgentForcedPromptProjection`: send a forced prompt as
    /// the provider's leading system prompt without recording it. Runs after
    /// the `context` extension handlers.
    fn install_agent_forced_prompt_projection(&self) {
        let previous = self.agent.runtime().transform_context.clone();
        let session = self.self_weak();
        let hook: Arc<TransformContextFn> = Arc::new(move |messages: Vec<AgentMessage>| {
            let session = session.clone();
            let previous = previous.clone();
            Box::pin(async move {
                let transformed = match &previous {
                    Some(previous) => previous(messages).await,
                    None => messages,
                };
                let Some(session) = session.upgrade() else {
                    return transformed;
                };
                let forced = session
                    .run_system_prompt_options
                    .lock()
                    .expect("run options lock")
                    .as_ref()
                    .and_then(|options| options.force_system_prompt.clone());
                let Some(forced) = forced else {
                    return transformed;
                };
                let llm_messages: Vec<crate::ai::types::message::Message> = transformed
                    .iter()
                    .filter_map(AgentMessage::to_message)
                    .collect();
                let current = get_current_system_message(&llm_messages);
                let head = SystemMessage {
                    content: StringOrBlocks::Text(forced),
                    tools_added: current.as_ref().and_then(|current| {
                        current
                            .tools_added
                            .as_ref()
                            .filter(|tools| !tools.is_empty())
                            .cloned()
                    }),
                    sections: None,
                    tools_removed: None,
                    timestamp: current
                        .as_ref()
                        .map(|current| current.timestamp)
                        .unwrap_or_else(now_ms),
                };
                let mut projected = vec![AgentMessage::System(head)];
                projected.extend(
                    transformed
                        .into_iter()
                        .filter(|message| message.role() != "system"),
                );
                projected
            })
        });
        self.agent.runtime().transform_context = Some(hook);
    }

    // =========================================================================
    // Event Subscription (agent-session.ts:601-653, 869-915)
    // =========================================================================

    /// Upstream `_emit`: emit an event to all listeners.
    fn emit(&self, event: AgentSessionEvent) {
        let listeners = self.event_listeners.lock().expect("listeners lock").clone();
        for (_, listener) in &listeners {
            listener(&event);
        }
    }

    /// Upstream `_emitQueueUpdate`.
    fn emit_queue_update(&self) {
        self.emit(AgentSessionEvent::QueueUpdate {
            steering: self
                .steering_messages
                .lock()
                .expect("steering lock")
                .clone(),
            follow_up: self
                .follow_up_messages
                .lock()
                .expect("follow up lock")
                .clone(),
        });
    }

    /// Upstream `_emitSessionCompactFailed` (extension-only event).
    async fn emit_session_compact_failed(&self, mut event: Value) {
        let runner = self.extension_runner();
        if runner.has_handlers("session_compact_failed") {
            runner.emit(&mut event).await;
        }
    }

    /// Upstream `_resolveIdleWaitIfIdle` — release idle waiters when the
    /// session has nothing in flight.
    fn resolve_idle_wait_if_idle(&self) {
        if self.is_idle() {
            let _ = self.idle_tx.send(true);
        }
    }

    /// Upstream `_emitAgentSettled`.
    async fn emit_agent_settled(&self) {
        // Give the cache warmer a chance to schedule (or cancel) before the
        // settled observers run.
        if let Some(cache_warmer) = &self.cache_warmer {
            cache_warmer.on_agent_settled();
        }
        self.is_agent_run_active.store(false, Ordering::SeqCst);
        self.is_emitting_agent_settled.store(true, Ordering::SeqCst);
        {
            let runner = self.extension_runner();
            let mut event = json!({"type": "agent_settled"});
            runner.emit(&mut event).await;
        }
        self.emit(AgentSessionEvent::AgentSettled);
        self.is_emitting_agent_settled
            .store(false, Ordering::SeqCst);

        let deferred: Vec<_> = {
            let mut prompts = self.deferred_settled_prompts.lock().expect("deferred lock");
            std::mem::take(&mut *prompts)
        };
        if !deferred.is_empty() {
            if let Some(session) = self.self_weak().upgrade() {
                for (text, options) in deferred {
                    // The prompt future is not `Send` (mutual recursion with
                    // the settled emission); block_on on a blocking thread
                    // keeps the replay ordered without poisoning `Send`.
                    let task_session = session.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        futures::executor::block_on(task_session.prompt(text, Some(options)))
                    })
                    .await;
                }
            }
            self.resolve_idle_wait_if_idle();
            return;
        }
        self.resolve_idle_wait_if_idle();
    }

    /// Upstream `_handleAgentEvent` — internal handler for agent events, shared
    /// by subscribe and reconnect.
    async fn handle_agent_event(&self, mut event: AgentEvent) {
        // When a user message starts, check if it's from either queue and
        // remove it BEFORE emitting. This ensures the UI sees the updated
        // queue state.
        if let AgentEvent::MessageStart { message } = event.kind() {
            if message.role() == "user" {
                self.overflow_recovery_attempted
                    .store(false, Ordering::SeqCst);
                let message_text = match message {
                    AgentMessage::User(user) => content_text_with_separator(&user.content, ""),
                    _ => String::new(),
                };
                if !message_text.is_empty() {
                    self.in_flight_user_messages
                        .lock()
                        .expect("in-flight input lock")
                        .push(message_text.clone());
                    // Check steering queue first
                    let removed_from_steering = {
                        let mut steering = self.steering_messages.lock().expect("steering lock");
                        let position = steering.iter().position(|text| *text == message_text);
                        match position {
                            Some(position) => {
                                steering.remove(position);
                                true
                            }
                            None => false,
                        }
                    };
                    if removed_from_steering {
                        self.emit_queue_update();
                    } else {
                        // Check follow-up queue
                        let removed_from_follow_up = {
                            let mut follow_up =
                                self.follow_up_messages.lock().expect("follow up lock");
                            let position = follow_up.iter().position(|text| *text == message_text);
                            match position {
                                Some(position) => {
                                    follow_up.remove(position);
                                    true
                                }
                                None => false,
                            }
                        };
                        if removed_from_follow_up {
                            self.emit_queue_update();
                        }
                    }
                }
            }
        }

        // Emit to extensions first
        self.emit_extension_event(&mut event).await;

        // Notify all listeners
        let will_retry = match event.kind() {
            AgentEvent::AgentEnd { messages } => self.will_retry_after_agent_end(messages),
            _ => false,
        };
        self.emit(AgentSessionEvent::from_agent_event(&event, will_retry));

        // Handle session persistence
        if let AgentEvent::MessageEnd { message } = event.kind() {
            // Check if this is a custom message from extensions
            if message.role() == "custom" {
                // Persist as CustomMessageEntry
                if let Some(custom) = custom_message_from_agent_message(message) {
                    let _ = self
                        .session_manager
                        .lock()
                        .expect("session lock")
                        .append_custom_message_entry(
                            &custom.custom_type,
                            custom.content.clone(),
                            custom.display,
                            custom.details.clone(),
                        );
                }
            } else if matches!(
                message.role(),
                "system" | "user" | "assistant" | "toolResult"
            ) {
                // Regular LLM message - persist as SessionMessageEntry
                let persisted = self
                    .session_manager
                    .lock()
                    .expect("session lock")
                    .append_message(message.clone());
                match persisted {
                    Ok(_) => {
                        if let AgentMessage::User(user) = message {
                            let text = content_text_with_separator(&user.content, "");
                            let mut pending = self
                                .in_flight_user_messages
                                .lock()
                                .expect("in-flight input lock");
                            if let Some(index) = pending.iter().position(|input| input == &text) {
                                pending.remove(index);
                            }
                        }
                    }
                    Err(error) => {
                        *self
                            .message_persistence_error
                            .lock()
                            .expect("persistence error lock") = Some(error.to_string());
                        self.agent_run_abort_requested.store(true, Ordering::SeqCst);
                        self.agent.abort();
                    }
                }
            }
            // Other message types (bashExecution, compactionSummary,
            // branchSummary) are persisted elsewhere

            // Track assistant message for auto-compaction (checked on
            // agent_end)
            if let AgentMessage::Assistant(assistant) = message {
                *self.last_assistant_message.lock().expect("assistant lock") =
                    Some(assistant.clone());

                if !matches!(
                    assistant.stop_reason,
                    StopReason::Error | StopReason::Length
                ) {
                    self.overflow_recovery_attempted
                        .store(false, Ordering::SeqCst);
                }

                // Reset retry counter immediately on successful assistant
                // response. This prevents accumulation across multiple LLM
                // calls within a turn.
                let retry_attempt = self.retry_attempt.load(Ordering::SeqCst);
                if assistant.stop_reason != StopReason::Error && retry_attempt > 0 {
                    self.emit(AgentSessionEvent::AutoRetryEnd {
                        success: true,
                        attempt: retry_attempt,
                        final_error: None,
                    });
                    self.retry_attempt.store(0, Ordering::SeqCst);
                }
            }
        }

        // A turn ends after its assistant message and every tool result has
        // been appended, so this is the first point in the run where a
        // context-only custom message can be inserted without landing between
        // a tool call and its result. Flushing after the extension and
        // listener dispatch above also picks up messages that turn_end
        // handlers queued.
        if matches!(event.kind(), AgentEvent::TurnEnd { .. }) {
            self.flush_pending_custom_messages();
        }
    }

    /// Upstream `_willRetryAfterAgentEnd`.
    fn will_retry_after_agent_end(&self, messages: &[AgentMessage]) -> bool {
        let settings = self.retry_settings();
        let retry_attempt = self.retry_attempt.load(Ordering::SeqCst);
        if !settings.enabled || retry_attempt >= settings.max_retries as u32 {
            return false;
        }

        for message in messages.iter().rev() {
            if let AgentMessage::Assistant(assistant) = message {
                return self.is_retryable_error(assistant);
            }
        }
        false
    }

    /// Upstream `_findLastAssistantMessage` (including aborted ones).
    fn find_last_assistant_message(&self) -> Option<AssistantMessage> {
        let messages = self.agent.state().messages.clone();
        messages
            .into_iter()
            .rev()
            .find_map(|message| match message {
                AgentMessage::Assistant(assistant) => Some(assistant),
                _ => None,
            })
    }

    /// Upstream `_replaceMessageInPlace` — keep agent state, later turn/agent
    /// events, listeners, and the eventual persistence in sync with an
    /// extension's message replacement. See the identity seam in the module
    /// docs: value-equality replacement instead of object-identity in-place
    /// mutation.
    fn replace_message_in_place(&self, target: &AgentMessage, replacement: AgentMessage) {
        if *target == replacement {
            return;
        }
        let mut state = self.agent.state();
        if let Some(message) = state
            .messages
            .iter_mut()
            .rev()
            .find(|message| **message == *target)
        {
            *message = replacement;
        }
    }

    /// Upstream `_emitExtensionEvent`: emit extension events based on agent
    /// events. The `message_end` replacement flows back through `event`.
    async fn emit_extension_event(&self, event: &mut AgentEvent) {
        let runner = self.extension_runner();
        let wire = serde_json::to_value(&*event).unwrap_or(Value::Null);
        match event.kind() {
            AgentEvent::AgentStart => {
                self.turn_index.store(0, Ordering::SeqCst);
                let mut extension_event = json!({"type": "agent_start"});
                runner.emit(&mut extension_event).await;
            }
            AgentEvent::AgentEnd { .. } => {
                let mut extension_event = json!({
                    "type": "agent_end",
                    "messages": wire["messages"].clone(),
                });
                runner.emit(&mut extension_event).await;
            }
            AgentEvent::TurnStart => {
                let mut extension_event = json!({
                    "type": "turn_start",
                    "turnIndex": self.turn_index.load(Ordering::SeqCst),
                    "timestamp": now_ms(),
                });
                runner.emit(&mut extension_event).await;
            }
            AgentEvent::TurnEnd { .. } => {
                let mut extension_event = json!({
                    "type": "turn_end",
                    "turnIndex": self.turn_index.load(Ordering::SeqCst),
                    "message": wire["message"].clone(),
                    "toolResults": wire["toolResults"].clone(),
                });
                runner.emit(&mut extension_event).await;
                self.turn_index.fetch_add(1, Ordering::SeqCst);
            }
            AgentEvent::MessageStart { .. } => {
                let mut extension_event = json!({
                    "type": "message_start",
                    "message": wire["message"].clone(),
                });
                runner.emit(&mut extension_event).await;
            }
            AgentEvent::MessageUpdate { .. } => {
                let mut extension_event = json!({
                    "type": "message_update",
                    "message": wire["message"].clone(),
                    "assistantMessageEvent":
                        wire["assistantMessageEvent"].clone(),
                });
                runner.emit(&mut extension_event).await;
            }
            AgentEvent::MessageEnd { message } => {
                let original = message.clone();
                let extension_event = json!({
                    "type": "message_end",
                    "message": wire["message"].clone(),
                });
                let replacement = runner.emit_message_end(&extension_event).await;
                if let Some(replacement) = replacement {
                    // Untyped extension handlers can return messages with
                    // null/missing content; normalize so it never enters agent
                    // state or session history.
                    let replacement = normalize_replacement_message(replacement);
                    if let Ok(replacement) = event.replace_message_end(replacement) {
                        self.replace_message_in_place(&original, replacement.clone());
                        // Both the typed message and preserved wire have now
                        // changed; listeners cannot observe the old snapshot.
                    }
                }
            }
            AgentEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            } => {
                let mut extension_event = json!({
                    "type": "tool_execution_start",
                    "toolCallId": tool_call_id,
                    "toolName": tool_name,
                    "args": args,
                });
                runner.emit(&mut extension_event).await;
            }
            AgentEvent::ToolExecutionUpdate {
                tool_call_id,
                tool_name,
                args,
                partial_result,
            } => {
                let mut extension_event = json!({
                    "type": "tool_execution_update",
                    "toolCallId": tool_call_id,
                    "toolName": tool_name,
                    "args": args,
                    "partialResult": partial_result,
                });
                runner.emit(&mut extension_event).await;
            }
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error,
            } => {
                let mut extension_event = json!({
                    "type": "tool_execution_end",
                    "toolCallId": tool_call_id,
                    "toolName": tool_name,
                    "result": result,
                    "isError": is_error,
                });
                runner.emit(&mut extension_event).await;
            }
            AgentEvent::Preserved(_) => unreachable!("kind() unwraps ingress"),
        }
    }

    /// Upstream `subscribe`: subscribe to agent session events. Session
    /// persistence is handled internally (saves messages on message_end).
    /// Multiple listeners can be added.
    pub fn subscribe(&self, listener: Arc<AgentSessionEventListener>) -> AgentSessionUnsubscribe {
        let id = self.next_listener_id.fetch_add(1, Ordering::SeqCst);
        self.event_listeners
            .lock()
            .expect("listeners lock")
            .push((id, listener));
        AgentSessionUnsubscribe {
            listeners: Arc::clone(&self.event_listeners),
            id,
        }
    }

    /// Upstream `_disconnectFromAgent`.
    fn disconnect_from_agent(&self) {
        if let Some(unsubscribe) = self
            .unsubscribe_agent
            .lock()
            .expect("unsubscribe lock")
            .take()
        {
            unsubscribe.unsubscribe();
        }
    }

    /// Upstream `dispose`: remove all listeners and disconnect from the agent.
    /// Call this when completely done with the session.
    pub fn dispose(&self) {
        // Dispose must succeed even if an abort hook throws; the ported abort
        // paths are infallible.
        self.abort_retry();
        self.abort_compaction();
        self.abort_branch_summary();
        self.abort_bash();
        self.agent.abort();

        // Upstream `_disconnectFromAgent`: drop the warmer's listener, then
        // cancel it.
        if let Some(cache_warmer) = &self.cache_warmer {
            cache_warmer.set_on_warmed(Box::new(|_| {}));
            cache_warmer.cancel();
        }

        self.extension_runner().invalidate(Some(
            "This extension ctx is stale after session replacement or reload. Do not use a \
             captured pi or command ctx after ctx.newSession(), ctx.fork(), ctx.switchSession(), \
             or ctx.reload(). For newSession, fork, and switchSession, move post-replacement work \
             into withSession and use the ctx passed to withSession. For reload, do not use the \
             old ctx after await ctx.reload().",
        ));
        self.disconnect_from_agent();
        self.event_listeners.lock().expect("listeners lock").clear();
        // cleanupSessionResources seam: the pi-ai per-session resource
        // registry is not ported (module docs).
    }

    // =========================================================================
    // Read-only State Access (agent-session.ts:917-1053)
    // =========================================================================

    /// Upstream `get state` (full agent state).
    pub fn state(&self) -> std::sync::MutexGuard<'_, crate::agent_core::types::AgentState> {
        self.agent.state()
    }

    /// Upstream `get model`: current model (may be `None` if not yet
    /// selected; the ported agent state seeds the "unknown" placeholder
    /// upstream maps to its default-model fallback).
    pub fn model(&self) -> Option<Model> {
        let model = &self.agent.state().model;
        if is_unknown_model(model) {
            None
        } else {
            Some(model.clone())
        }
    }

    /// Upstream `get thinkingLevel`.
    pub fn thinking_level(&self) -> ThinkingLevel {
        self.agent.state().thinking_level
    }

    /// Upstream `get isStreaming`: whether the session is currently processing
    /// an agent run or post-run continuation.
    pub fn is_streaming(&self) -> bool {
        self.is_agent_run_active.load(Ordering::SeqCst)
    }

    /// Upstream `get isIdle`: no active agent run, compaction, branch summary,
    /// retry, or queued continuation.
    pub fn is_idle(&self) -> bool {
        !self.is_streaming() && !self.is_compacting()
    }

    /// Upstream `get systemPrompt`: the current effective system prompt,
    /// including changes not yet sent to the model.
    pub fn system_prompt(&self) -> String {
        let options = self
            .run_system_prompt_options
            .lock()
            .expect("run options lock")
            .clone()
            .unwrap_or_else(|| self.base_system_prompt_options().clone());
        system_prompt::build_system_prompt(&to_build_options(&options)).unwrap_or_default()
    }

    /// Upstream `get retryAttempt` (0 if not retrying).
    pub fn retry_attempt(&self) -> u32 {
        self.retry_attempt.load(Ordering::SeqCst)
    }

    /// Upstream `getActiveToolNames`: names of the tools currently set on the
    /// agent.
    pub fn get_active_tool_names(&self) -> Vec<String> {
        self.agent
            .state()
            .tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect()
    }

    /// Upstream `getAllTools`: all configured tools with name, description,
    /// parameter schema, prompt guidelines, and source metadata.
    pub fn get_all_tools(&self) -> Vec<ToolInfo> {
        self.tool_definitions
            .lock()
            .expect("tool definitions lock")
            .values()
            .map(|entry| ToolInfo {
                name: entry.definition.name.clone(),
                description: entry.definition.description.clone(),
                parameters: entry.definition.parameters.clone(),
                prompt_guidelines: entry.definition.prompt_guidelines.clone(),
                exposure: entry.definition.exposure,
                namespace: entry.definition.namespace.clone(),
                annotations: entry.definition.annotations.clone(),
                source_info: entry.source_info.clone(),
            })
            .collect()
    }

    /// Upstream `getToolDefinition`.
    pub fn get_tool_definition(
        &self,
        name: &str,
    ) -> Option<Arc<crate::coding_agent::extensions::types::ToolDefinition>> {
        self.tool_definitions
            .lock()
            .expect("tool definitions lock")
            .get(name)
            .map(|entry| Arc::clone(&entry.definition))
    }

    /// Upstream `setActiveToolsByName`: set active tools by name. Only tools
    /// in the registry can be enabled; unknown tool names are ignored. Also
    /// rebuilds the system prompt to reflect the new tool set; changes take
    /// effect on the next agent turn.
    pub fn set_active_tools_by_name(&self, tool_names: Vec<String>) {
        let previous = self.get_active_tool_names();
        self.set_active_tools_inner(tool_names);
        // A loadout that deactivates a tool replaces the restored one, whose
        // pending tools are dropped. One that only adds tools, like activating
        // tool_search, keeps them.
        let active: HashSet<String> = self.get_active_tool_names().into_iter().collect();
        if previous.iter().any(|name| !active.contains(name)) {
            self.pending_tool_names
                .lock()
                .expect("pending tools lock")
                .clear();
        }
    }

    /// Upstream `_setActiveTools`.
    fn set_active_tools_inner(&self, tool_names: Vec<String>) {
        let hidden = self.hidden_tool_names();
        let registry = self.tool_registry.lock().expect("tool registry lock");
        let mut tools: Vec<Arc<AgentTool>> = Vec::new();
        let mut valid_tool_names: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        for name in &tool_names {
            if hidden.contains(name) || !seen.insert(name.clone()) {
                continue;
            }
            if let Some(tool) = registry.get(name) {
                tools.push(Arc::clone(tool));
                valid_tool_names.push(name.clone());
            }
        }
        drop(registry);
        self.pending_tool_names
            .lock()
            .expect("pending tools lock")
            .retain(|name| !hidden.contains(name));
        for tool in &valid_tool_names {
            self.pending_tool_names
                .lock()
                .expect("pending tools lock")
                .remove(tool);
        }
        self.agent.state().tools = tools;
        self.rebuild_system_prompt(valid_tool_names);
    }

    // Pisper patch: exposure is an execution boundary, including restored and pending loadouts.
    fn hidden_tool_names(&self) -> HashSet<String> {
        self.tool_definitions
            .lock()
            .expect("tool definitions lock")
            .iter()
            .filter(|(_, entry)| entry.definition.exposure == ToolExposure::Hidden)
            .map(|(name, _)| name.to_string())
            .collect()
    }

    /// Upstream `_isAllowedTool`.
    fn is_allowed_tool(&self, name: &str) -> bool {
        (self
            .allowed_tool_names
            .as_ref()
            .is_none_or(|allowed| allowed.contains(name)))
            && !self
                .excluded_tool_names
                .as_ref()
                .is_some_and(|excluded| excluded.contains(name))
    }

    /// Upstream `get isCompacting`: whether compaction or branch
    /// summarization is currently running.
    pub fn is_compacting(&self) -> bool {
        self.auto_compaction_abort
            .lock()
            .expect("auto lock")
            .is_some()
            || self
                .compaction_abort
                .lock()
                .expect("compaction lock")
                .is_some()
            || self
                .branch_summary_abort
                .lock()
                .expect("branch lock")
                .is_some()
    }

    /// Upstream `get messages`: all messages including custom types like
    /// BashExecutionMessage.
    pub fn messages(&self) -> Vec<AgentMessage> {
        self.agent.state().messages.clone()
    }

    /// Upstream `get steeringMode`.
    pub fn steering_mode(&self) -> QueueMode {
        self.agent.steering_mode()
    }

    /// Upstream `get followUpMode`.
    pub fn follow_up_mode(&self) -> QueueMode {
        self.agent.follow_up_mode()
    }

    /// Upstream `get sessionFile`: current session file path, or `None` if
    /// sessions are disabled.
    pub fn session_file(&self) -> Option<String> {
        self.session_manager
            .lock()
            .expect("session lock")
            .get_session_file()
            .map(str::to_string)
    }

    /// Upstream `get sessionId`.
    pub fn session_id(&self) -> String {
        self.session_manager
            .lock()
            .expect("session lock")
            .get_session_id()
            .to_string()
    }

    /// Upstream `get sessionName`.
    pub fn session_name(&self) -> Option<String> {
        self.session_manager
            .lock()
            .expect("session lock")
            .get_session_name()
    }

    /// Upstream `get scopedModels` (scoped models for cycling, from --models
    /// flag).
    pub fn scoped_models(&self) -> Vec<ScopedModel> {
        self.scoped_models.lock().expect("scoped lock").clone()
    }

    /// Upstream `setScopedModels`.
    pub fn set_scoped_models(&self, scoped_models: Vec<ScopedModel>) {
        *self.scoped_models.lock().expect("scoped lock") = scoped_models;
    }

    /// Upstream `get promptTemplates`.
    pub fn prompt_templates(&self) -> Vec<PromptTemplate> {
        self.resource_loader
            .lock()
            .expect("resource loader lock")
            .get_prompts()
            .prompts
    }

    fn base_system_prompt_options(
        &self,
    ) -> std::sync::MutexGuard<'_, NormalizedBuildSystemPromptOptions> {
        self.base_system_prompt_options
            .lock()
            .expect("base options lock")
    }

    fn normalize_prompt_snippet(&self, text: Option<&String>) -> Option<String> {
        let text = text?;
        if text.is_empty() {
            return None;
        }
        // JS: replace /[\r\n]+/g with " ", then /\s+/g with " ", then trim.
        let mut one_line = String::new();
        let mut previous_was_space = false;
        for character in text.chars() {
            let is_newline = character == '\r' || character == '\n';
            if is_newline || character.is_whitespace() {
                if !previous_was_space {
                    one_line.push(' ');
                    previous_was_space = true;
                }
            } else {
                one_line.push(character);
                previous_was_space = false;
            }
        }
        let trimmed = one_line.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }

    fn normalize_prompt_guidelines(&self, guidelines: Option<&Vec<String>>) -> Vec<String> {
        let Some(guidelines) = guidelines else {
            return Vec::new();
        };
        if guidelines.is_empty() {
            return Vec::new();
        }
        let mut unique: Vec<String> = Vec::new();
        for guideline in guidelines {
            let normalized = guideline.trim();
            if !normalized.is_empty() && !unique.iter().any(|existing| existing == normalized) {
                unique.push(normalized.to_string());
            }
        }
        unique
    }

    /// Upstream `_rebuildSystemPrompt`.
    fn rebuild_system_prompt(&self, tool_names: Vec<String>) {
        let valid_tool_names: Vec<String> = {
            let registry = self.tool_registry.lock().expect("tool registry lock");
            tool_names
                .into_iter()
                .filter(|name| registry.has(name))
                .collect()
        };
        let mut tool_snippets = std::collections::BTreeMap::new();
        {
            let snippets = self.tool_prompt_snippets.lock().expect("snippets lock");
            let names: Vec<String> = self
                .tool_registry
                .lock()
                .expect("tool registry lock")
                .keys()
                .map(str::to_string)
                .collect();
            for name in names {
                if let Some(snippet) = snippets.get(&name) {
                    tool_snippets.insert(name, snippet.clone());
                }
            }
        }

        let loader = self.resource_loader.lock().expect("resource loader lock");
        let loader_system_prompt = loader.get_system_prompt();
        let loader_append_system_prompt = loader.get_append_system_prompt();
        let append_system_prompt = if !loader_append_system_prompt.is_empty() {
            loader_append_system_prompt.join("\n\n")
        } else {
            String::new()
        };
        let loaded_skills = loader.get_skills().skills;
        let loaded_context_files = loader.get_agents_files();
        drop(loader);

        let tool_guidelines: std::collections::BTreeMap<String, Vec<String>> = self
            .tool_prompt_guidelines
            .lock()
            .expect("guidelines lock")
            .iter()
            .map(|(name, guidelines)| (name.to_string(), guidelines.clone()))
            .collect();
        *self
            .base_system_prompt_options
            .lock()
            .expect("base options lock") =
            system_prompt::normalize_build_system_prompt_options(&BuildSystemPromptOptions {
                cwd: self.cwd.clone(),
                skills: Some(
                    loaded_skills
                        .iter()
                        .map(|skill| {
                            json!({
                                "name": skill.name,
                                "description": skill.description,
                                "filePath": skill.file_path,
                                "baseDir": skill.base_dir,
                                "disableModelInvocation": skill.disable_model_invocation,
                            })
                        })
                        .collect(),
                ),
                context_files: Some(
                    loaded_context_files
                        .iter()
                        .map(|file| (file.path.clone(), file.content.clone()))
                        .collect(),
                ),
                custom_prompt: loader_system_prompt,
                append_system_prompt: Some(append_system_prompt),
                selected_tools: Some(valid_tool_names),
                tool_snippets: Some(tool_snippets),
                tool_guidelines: Some(tool_guidelines),
                ..BuildSystemPromptOptions::default()
            });
    }

    /// Upstream `_preparePromptAndToolLoadout`: apply a prompt and tool loadout
    /// for the next request. Sets the executable tools and returns a system
    /// message patching the prompt sections the model currently has (replayed
    /// from `messages`), or `None` when the prompt is unchanged.
    fn prepare_prompt_and_tool_loadout(
        &self,
        options: &NormalizedBuildSystemPromptOptions,
        messages: Option<&[AgentMessage]>,
    ) -> Option<SystemMessage> {
        let unique_tools: Vec<String> = {
            let mut seen = HashSet::new();
            options
                .selected_tools
                .iter()
                .filter(|name| seen.insert((*name).clone()))
                .cloned()
                .collect()
        };
        let hidden = self.hidden_tool_names();
        let registry = self.tool_registry.lock().expect("tool registry lock");
        let selected_tools: Vec<String> = unique_tools
            .into_iter()
            .filter(|name| registry.has(name) && !hidden.contains(name))
            .collect();
        let tools: Vec<Arc<AgentTool>> = selected_tools
            .iter()
            .filter_map(|name| registry.get(name).cloned())
            .collect();
        drop(registry);
        self.agent.state().tools = tools;

        let messages = match messages {
            Some(messages) => messages,
            None => &self.agent.state().messages,
        };
        let llm_messages: Vec<crate::ai::types::message::Message> = messages
            .iter()
            .filter_map(AgentMessage::to_message)
            .collect();
        let previous_sections = get_current_system_message(&llm_messages)
            .and_then(|system| system.sections)
            .unwrap_or_default();
        let desired_sections =
            system_prompt::build_system_prompt_sections(&to_build_options(options)).ok()?;
        let sections =
            system_prompt::diff_system_prompt_sections(&previous_sections, &desired_sections)?;
        Some(SystemMessage {
            content: StringOrBlocks::Text(String::new()),
            sections: Some(Sections::new(sections)),
            tools_added: None,
            tools_removed: None,
            timestamp: now_ms(),
        })
    }

    /// Upstream `_restoreToolsFromTranscript`: restore the active tool loadout
    /// declared by the session transcript, if it declares one. Names the
    /// loadout declares but that are not registered yet (allowed only) stay
    /// pending until they register (v1.0.0).
    fn restore_tools_from_transcript(&self) {
        self.pending_tool_names
            .lock()
            .expect("pending tools lock")
            .clear();
        let context = self
            .session_manager
            .lock()
            .expect("session lock")
            .build_session_context();
        let llm_messages: Vec<crate::ai::types::message::Message> = context
            .messages
            .iter()
            .filter_map(AgentMessage::to_message)
            .collect();
        let Some(current) = get_current_system_message(&llm_messages) else {
            return;
        };
        let hidden = self.hidden_tool_names();
        let registry = self.tool_registry.lock().expect("tool registry lock");
        let tool_names: Vec<String> = current
            .tools_added
            .unwrap_or_default()
            .iter()
            .map(|tool| tool.name.clone())
            .filter(|name| registry.has(name) && !hidden.contains(name))
            .collect();
        let tools: Vec<Arc<AgentTool>> = tool_names
            .iter()
            .filter_map(|name| registry.get(name).cloned())
            .collect();
        drop(registry);
        *self.pending_tool_names.lock().expect("pending tools lock") = tool_names
            .iter()
            .filter(|name| self.is_allowed_tool(name))
            .cloned()
            .collect();
        self.agent.state().tools = tools;
        self.rebuild_system_prompt(tool_names);
    }

    // =========================================================================
    // Prompting (agent-session.ts:1172-1487)
    // =========================================================================

    /// Upstream `_runAgentPrompt`.
    async fn run_agent_prompt(&self, messages: Vec<AgentMessage>) -> Result<(), AgentSessionError> {
        self.run_agent_action(messages, false, None).await
    }

    /// Continue already queued inputs while retaining the session's native
    /// idle/abort, persistence, retry and compaction lifecycle.
    pub async fn continue_queued(&self) -> Result<(), AgentSessionError> {
        self.continue_queued_cancellable(None).await
    }

    pub async fn continue_queued_cancellable(
        &self,
        cancellation: Option<CancellationToken>,
    ) -> Result<(), AgentSessionError> {
        if self.is_streaming() {
            return Err(AgentSessionError::Upstream(
                "Session is already running".into(),
            ));
        }
        if self.pending_message_count() == 0 {
            return Err(AgentSessionError::Upstream(
                "No queued session inputs".into(),
            ));
        }
        self.run_agent_action(Vec::new(), true, cancellation).await
    }

    async fn run_agent_action(
        &self,
        messages: Vec<AgentMessage>,
        resume_queued: bool,
        cancellation: Option<CancellationToken>,
    ) -> Result<(), AgentSessionError> {
        if let Some(error) = self.message_persistence_error() {
            return Err(AgentSessionError::Upstream(format!(
                "Session persistence failed: {error}"
            )));
        }
        if cancellation
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(AgentSessionError::Upstream(
                "Session input was cancelled before starting".into(),
            ));
        }
        // Compaction before the prompt may have scheduled a retry; the new
        // prompt replaces it (upstream `_failedResponse = undefined`).
        self.agent_run_abort_requested
            .store(false, Ordering::SeqCst);
        self.record_selection();
        // The run records the loadout in the transcript; restored tools that
        // did not register by now are dropped, so a tool that never registers
        // does not stay pending (v1.0.0).
        self.pending_tool_names
            .lock()
            .expect("pending tools lock")
            .clear();
        self.is_agent_run_active.store(true, Ordering::SeqCst);
        let _ = self.idle_tx.send(false);
        let started = AtomicBool::new(false);
        let action = async {
            started.store(true, Ordering::SeqCst);
            if resume_queued {
                self.agent.continue_run().await?;
            } else {
                self.agent.prompt(PromptInput::Messages(messages)).await?;
            }
            // Upstream also consults the `agent_before_settle` boundary here
            // (deferred with that seam — see the module notes); without it the
            // post-run loop continues only for retry/compaction/queued work.
            while !self.agent_run_abort_requested.load(Ordering::SeqCst) {
                if self.handle_post_agent_run().await? {
                    if self.agent_run_abort_requested.load(Ordering::SeqCst) {
                        break;
                    }
                    self.agent.continue_run().await?;
                    continue;
                }
                break;
            }
            Ok(())
        };
        tokio::pin!(action);
        let result: Result<(), anyhow::Error> = if let Some(cancellation) = cancellation {
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => {
                    self.agent_run_abort_requested.store(true, Ordering::SeqCst);
                    self.agent.abort();
                    if started.load(Ordering::SeqCst) {
                        action.await
                    } else {
                        Err(anyhow::anyhow!("Session input was cancelled before starting"))
                    }
                }
                result = &mut action => result,
            }
        } else {
            action.await
        };
        // finally {
        if self.agent_run_abort_requested.load(Ordering::SeqCst) {
            self.finish_cancelled_retry();
        }
        *self
            .run_system_prompt_options
            .lock()
            .expect("run options lock") = None;
        self.flush_pending_bash_messages();
        self.flush_pending_custom_messages();
        self.emit_agent_settled().await;
        // }
        if let Some(error) = self.message_persistence_error() {
            return Err(AgentSessionError::Upstream(format!(
                "Session persistence failed: {error}"
            )));
        }
        result.map_err(|error| AgentSessionError::Upstream(error.to_string()))
    }

    /// Upstream `_handlePostAgentRun`: `Ok(true)` means the post-run loop
    /// should call `agent.continue()`.
    async fn handle_post_agent_run(&self) -> Result<bool, AgentSessionError> {
        if self.agent_run_abort_requested.load(Ordering::SeqCst) {
            self.finish_cancelled_retry();
            return Ok(false);
        }
        let msg = self
            .last_assistant_message
            .lock()
            .expect("assistant lock")
            .take();
        let Some(msg) = msg else {
            return Ok(self.agent.has_queued_messages());
        };

        if self.is_retryable_error(&msg) && self.prepare_retry(&msg).await? {
            if self.agent_run_abort_requested.load(Ordering::SeqCst) {
                self.finish_cancelled_retry();
                return Ok(false);
            }
            return Ok(true);
        }
        if self.agent_run_abort_requested.load(Ordering::SeqCst) {
            self.finish_cancelled_retry();
            return Ok(false);
        }

        let retry_attempt = self.retry_attempt.load(Ordering::SeqCst);
        if msg.stop_reason == StopReason::Error && retry_attempt > 0 {
            self.emit(AgentSessionEvent::AutoRetryEnd {
                success: false,
                attempt: retry_attempt,
                final_error: msg.error_message.clone(),
            });
            self.retry_attempt.store(0, Ordering::SeqCst);
        }

        if self.check_compaction(&msg, true).await? {
            return Ok(!self.agent_run_abort_requested.load(Ordering::SeqCst));
        }

        // The agent loop drains both queues before emitting agent_end. Any
        // messages here were queued by agent_end extension handlers and need a
        // continuation.
        Ok(!self.agent_run_abort_requested.load(Ordering::SeqCst)
            && self.agent.has_queued_messages())
    }

    /// Upstream `_runInputHandlers`.
    async fn run_input_handlers(
        &self,
        text: &str,
        images: Option<Vec<ImageContent>>,
        source: InputSource,
        streaming_behavior: Option<StreamingDelivery>,
    ) -> Option<(String, Option<Vec<ImageContent>>)> {
        let runner = self.extension_runner();
        if !runner.has_handlers("input") {
            return Some((text.to_string(), images));
        }

        let images_json = images.as_ref().map(|images| {
            images
                .iter()
                .map(|image| {
                    // Extensions receive the tagged upstream ImageContent, not
                    // the untagged Rust payload used inside message unions.
                    serde_json::to_value(TextOrImageBlock::Image(image.clone()))
                        .expect("image content is JSON serializable")
                })
                .collect::<Vec<_>>()
        });
        let input_result = runner
            .emit_input(text, images_json, source, streaming_behavior)
            .await;
        match input_result {
            crate::coding_agent::extensions::types::InputEventResult::Handled => None,
            crate::coding_agent::extensions::types::InputEventResult::Transform {
                text,
                images: transformed_images,
            } => {
                let parsed = transformed_images.and_then(|images| parse_image_content(&images));
                Some((text, parsed.or(images)))
            }
            crate::coding_agent::extensions::types::InputEventResult::Continue => {
                Some((text.to_string(), images))
            }
        }
    }

    /// Upstream `prompt`: send a prompt to the agent.
    ///
    /// - Handles extension commands (registered via pi.registerCommand)
    ///   immediately, even during streaming
    /// - Expands file-based prompt templates by default
    /// - During streaming, queues via steer() or followUp() based on
    ///   streamingBehavior option
    /// - Validates model and API key before sending (when not streaming)
    ///
    /// Errors carry the exact upstream messages.
    pub async fn prompt(
        &self,
        text: impl Into<String>,
        options: Option<PromptOptions>,
    ) -> Result<(), AgentSessionError> {
        let text = text.into();
        let options = options.unwrap_or_default();
        if let Some(error) = self.message_persistence_error() {
            return Err(AgentSessionError::Upstream(format!(
                "Session persistence failed: {error}"
            )));
        }
        if options
            .start_cancellation
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(AgentSessionError::Upstream(
                "Session input was cancelled before starting".into(),
            ));
        }
        let expand_prompt_templates = options.expand_prompt_templates.unwrap_or(true);
        let preflight_result = options.preflight_result.clone();
        // Upstream: a prompt arriving while `agent_settled` is still being
        // emitted is deferred until the emission and any earlier deferred
        // prompts finish.
        if self.is_emitting_agent_settled.load(Ordering::SeqCst) {
            self.deferred_settled_prompts
                .lock()
                .expect("deferred lock")
                .push((text, options));
            return Ok(());
        }
        let mut messages: Option<Vec<AgentMessage>> = None;

        let result: Result<(), AgentSessionError> = async {
            // Handle extension commands first (execute immediately, even
            // during streaming). Extension commands manage their own LLM
            // interaction via pi.sendMessage().
            if expand_prompt_templates && text.starts_with('/') {
                let handled = self.try_execute_extension_command(&text).await;
                if handled {
                    // Extension command executed, no prompt to send
                    if let Some(preflight) = &preflight_result {
                        preflight(PromptDisposition::Handled);
                    }
                    return Ok(());
                }
            }

            if self
                .compaction_abort
                .lock()
                .expect("compaction lock")
                .is_some()
            {
                return Err(AgentSessionError::Upstream(String::from(
                    "Cannot submit a prompt while compaction is in progress. Wait for compaction \
                     to finish and retry.",
                )));
            }

            // Emit input event for extension interception (before
            // skill/template expansion)
            let processed_input = self
                .run_input_handlers(
                    &text,
                    options.images.clone(),
                    options.source.unwrap_or(InputSource::Interactive),
                    if self.is_streaming() {
                        options.streaming_behavior
                    } else {
                        None
                    },
                )
                .await;
            let Some((current_text, current_images)) = processed_input else {
                if let Some(preflight) = &preflight_result {
                    preflight(PromptDisposition::Handled);
                }
                return Ok(());
            };

            // Expand skill commands (/skill:name args) and prompt templates
            // (/template args)
            let mut expanded_text = current_text.clone();
            if expand_prompt_templates {
                expanded_text = self.expand_skill_command(&expanded_text);
                expanded_text = prompt_templates::expand_prompt_template(
                    &expanded_text,
                    &self.prompt_templates(),
                );
            }

            // If streaming, queue via steer() or followUp() based on option
            if self.is_streaming() {
                let Some(streaming_behavior) = options.streaming_behavior else {
                    return Err(AgentSessionError::Upstream(String::from(
                        "Agent is already processing. Specify streamingBehavior ('steer' or \
                         'followUp') to queue the message.",
                    )));
                };
                if streaming_behavior == StreamingDelivery::FollowUp {
                    self.queue_follow_up(&expanded_text, current_images.clone());
                } else {
                    self.queue_steer(&expanded_text, current_images.clone());
                }
                if let Some(preflight) = &preflight_result {
                    preflight(PromptDisposition::Queued);
                }
                return Ok(());
            }

            // Flush any pending bash and custom messages before the new prompt
            self.flush_pending_bash_messages();
            self.flush_pending_custom_messages();

            // Validate model
            let Some(model) = self.model() else {
                return Err(AgentSessionError::Upstream(
                    format_no_model_selected_message(),
                ));
            };

            let has_configured_auth = self.model_runtime.has_configured_auth(&model.provider)
                || self
                    .model_runtime
                    .check_auth(&model.provider, None)
                    .await
                    .map_err(|error| AgentSessionError::Upstream(error.to_string()))?
                    .is_some();
            if !has_configured_auth {
                let is_oauth = self.model_runtime.is_using_oauth(&model.provider);
                if is_oauth {
                    return Err(AgentSessionError::Upstream(format!(
                        "Authentication failed for \"{}\". Credentials may have expired or \
                         network is unavailable. Run '/login {}' to re-authenticate.",
                        model.provider, model.provider
                    )));
                }
                return Err(AgentSessionError::Upstream(
                    format_no_api_key_found_message(&model.provider),
                ));
            }

            // Check if we need to compact before sending (catches aborted
            // responses). The user's new prompt is sent below, so do not call
            // agent.continue() here.
            if options
                .start_cancellation
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
            {
                return Err(AgentSessionError::Upstream(
                    "Session input was cancelled before starting".into(),
                ));
            }
            if let Some(last_assistant) = self.find_last_assistant_message() {
                self.check_compaction(&last_assistant, false).await?;
            }

            // Build messages array (custom message if any, then user message)
            let mut prompt_messages: Vec<AgentMessage> = Vec::new();

            // Add user message
            let mut user_content: Vec<TextOrImageBlock> =
                vec![TextOrImageBlock::Text(TextContent {
                    text: expanded_text.clone(),
                    text_signature: None,
                })];
            if let Some(images) = &current_images {
                user_content.extend(images.iter().cloned().map(TextOrImageBlock::Image));
            }
            prompt_messages.push(AgentMessage::User(UserMessage {
                content: StringOrBlocks::Blocks(user_content),
                timestamp: now_ms(),
            }));

            // Inject any pending "nextTurn" messages as context alongside the
            // user message
            let pending_next_turn = std::mem::take(
                &mut *self
                    .pending_next_turn_messages
                    .lock()
                    .expect("pending lock"),
            );
            prompt_messages.extend(pending_next_turn);

            // Emit before_agent_start extension event
            let selected_tools_before = self.base_system_prompt_options().selected_tools.clone();
            let runner = self.extension_runner();
            // Drop the options mutex guard before any extension can suspend or
            // reenter the session (including setActiveTools/getSystemPrompt).
            let build_options = to_build_options(&self.base_system_prompt_options().clone());
            let result = runner
                .emit_before_agent_start(
                    &expanded_text,
                    current_images.as_ref().map(|images| {
                        images
                            .iter()
                            .map(|image| {
                                // Extensions receive the tagged upstream ImageContent, not
                                // the untagged Rust payload used inside message unions.
                                serde_json::to_value(TextOrImageBlock::Image(image.clone()))
                                    .expect("image content is JSON serializable")
                            })
                            .collect()
                    }),
                    &build_options,
                    Arc::new(SessionSystemPromptRenderer),
                )
                .await
                .map_err(AgentSessionError::Upstream)?;
            // Handlers may edit event.systemPromptOptions.selectedTools or
            // call setActiveTools(), which updates the live loadout instead.
            // An explicit edit wins; otherwise the live loadout is
            // authoritative, so a setActiveTools() call is not undone here.
            let handler_edited_tools = result.system_prompt_options.selected_tools.len()
                != selected_tools_before.len()
                || result
                    .system_prompt_options
                    .selected_tools
                    .iter()
                    .zip(selected_tools_before.iter())
                    .any(|(name, before)| name != before);
            let mut system_prompt_options = result.system_prompt_options;
            if !handler_edited_tools {
                system_prompt_options.selected_tools = self.get_active_tool_names();
            }
            for message in &result.messages {
                prompt_messages.push(custom_agent_message_from_value(message.clone()));
            }
            let update_message = self.prepare_prompt_and_tool_loadout(&system_prompt_options, None);
            *self
                .run_system_prompt_options
                .lock()
                .expect("run options lock") = Some(system_prompt_options);
            if let Some(update) = update_message {
                prompt_messages.insert(0, AgentMessage::System(update));
            }
            messages = Some(prompt_messages);
            Ok(())
        }
        .await;

        match result {
            Ok(()) => {}
            // Upstream no longer reports a preflight disposition for rejected
            // prompts (the hook documents "Not called if the prompt is
            // rejected").
            Err(error) => return Err(error),
        }

        if let Some(prompt_messages) = messages {
            if let Some(preflight) = &preflight_result {
                preflight(PromptDisposition::Started);
            }
            self.run_agent_action(prompt_messages, false, options.start_cancellation.clone())
                .await?;
        }
        Ok(())
    }

    /// Upstream `_tryExecuteExtensionCommand`. Returns true if command was
    /// found and executed.
    async fn try_execute_extension_command(&self, text: &str) -> bool {
        // Parse command name and args
        let (command_name, args) = match text.find(' ') {
            Some(space_index) => (&text[1..space_index], &text[space_index + 1..]),
            None => (&text[1..], ""),
        };

        let runner = self.extension_runner();
        let Some(command) = runner.get_command(command_name) else {
            return false;
        };

        // Get command context from extension runner (includes session control
        // methods)
        let ctx = runner.create_command_context();

        let result = match (command.command.handler)(args, &ctx) {
            Ok(Some(promise)) => promise.await,
            Ok(None) => Ok(()),
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            // A command may reload/rebind while suspended. Like upstream's
            // this._extensionRunner lookup in catch, use the current runner.
            self.extension_runner().emit_error(ExtensionError {
                extension_path: format!("command:{command_name}"),
                event: "command".to_string(),
                error,
                stack: None,
            });
        }
        true
    }

    /// Upstream `_expandSkillCommand`: expand skill commands (/skill:name args)
    /// to their full content. Returns the expanded text, or the original text
    /// if not a skill command or skill not found. Emits errors via the
    /// extension runner if file read fails.
    fn expand_skill_command(&self, text: &str) -> String {
        if !text.starts_with("/skill:") {
            return text.to_string();
        }

        let rest = &text["/skill:".len()..];
        let (skill_name, args) = match rest.find(' ') {
            Some(space_index) => (&rest[..space_index], rest[space_index + 1..].trim()),
            None => (rest, ""),
        };

        let skill = self
            .resource_loader
            .lock()
            .expect("resource loader lock")
            .get_skills()
            .skills
            .into_iter()
            .find(|skill| skill.name == skill_name);
        let Some(skill) = skill else {
            return text.to_string(); // Unknown skill, pass through
        };

        match std::fs::read_to_string(&skill.file_path) {
            Ok(content) => {
                let body = strip_frontmatter(&content)
                    .unwrap_or(content)
                    .trim()
                    .to_string();
                let skill_block = format!(
                    "<skill name=\"{}\" location=\"{}\">\nReferences are relative to {}.\n\n{}\n</skill>",
                    skill.name, skill.file_path, skill.base_dir, body
                );
                if args.is_empty() {
                    skill_block
                } else {
                    format!("{skill_block}\n\n{args}")
                }
            }
            Err(error) => {
                // Emit error like extension commands do
                self.extension_runner().emit_error(ExtensionError {
                    extension_path: skill.file_path.clone(),
                    event: "skill_expansion".to_string(),
                    error: error.to_string(),
                    stack: None,
                });
                text.to_string() // Return original on error
            }
        }
    }

    /// Upstream `_queueUserInput`.
    async fn queue_user_input(
        &self,
        text: &str,
        images: Option<Vec<ImageContent>>,
        behavior: StreamingDelivery,
        source: InputSource,
    ) -> Result<PromptDisposition, AgentSessionError> {
        if text.starts_with('/') {
            self.throw_if_extension_command(text)?;
        }

        let Some((text, images)) = self
            .run_input_handlers(
                text,
                images,
                source,
                if self.is_streaming() {
                    Some(behavior)
                } else {
                    None
                },
            )
            .await
        else {
            return Ok(PromptDisposition::Handled);
        };

        let expanded_text = self.expand_skill_command(&text);
        let expanded_text =
            prompt_templates::expand_prompt_template(&expanded_text, &self.prompt_templates());

        if behavior == StreamingDelivery::Steer {
            self.queue_steer(&expanded_text, images);
        } else {
            self.queue_follow_up(&expanded_text, images);
        }
        Ok(PromptDisposition::Queued)
    }

    /// Upstream `steer`: queue a steering message while the agent is running;
    /// delivered after the current assistant turn finishes executing its tool
    /// calls, before the next LLM call. Expands skill commands and prompt
    /// templates. Errors on extension commands.
    pub async fn steer(
        &self,
        text: impl Into<String>,
        images: Option<Vec<ImageContent>>,
        source: Option<InputSource>,
    ) -> Result<PromptDisposition, AgentSessionError> {
        self.queue_user_input(
            &text.into(),
            images,
            StreamingDelivery::Steer,
            source.unwrap_or(InputSource::Interactive),
        )
        .await
    }

    /// Upstream `followUp`: queue a follow-up message to be processed after the
    /// agent finishes; delivered only when agent has no more tool calls or
    /// steering messages. Expands skill commands and prompt templates. Errors
    /// on extension commands.
    pub async fn follow_up(
        &self,
        text: impl Into<String>,
        images: Option<Vec<ImageContent>>,
        source: Option<InputSource>,
    ) -> Result<PromptDisposition, AgentSessionError> {
        self.queue_user_input(
            &text.into(),
            images,
            StreamingDelivery::FollowUp,
            source.unwrap_or(InputSource::Interactive),
        )
        .await
    }

    /// Upstream `_queueSteer` (already expanded, no extension command check).
    fn queue_steer(&self, text: &str, images: Option<Vec<ImageContent>>) {
        self.steering_messages
            .lock()
            .expect("steering lock")
            .push(text.to_string());
        self.emit_queue_update();
        self.agent
            .steer(user_message_with_text_and_images(text, images));
    }

    /// Upstream `_queueFollowUp` (already expanded, no extension command
    /// check).
    fn queue_follow_up(&self, text: &str, images: Option<Vec<ImageContent>>) {
        self.follow_up_messages
            .lock()
            .expect("follow up lock")
            .push(text.to_string());
        self.emit_queue_update();
        self.agent
            .follow_up(user_message_with_text_and_images(text, images));
    }

    /// Upstream `_throwIfExtensionCommand`.
    fn throw_if_extension_command(&self, text: &str) -> Result<(), AgentSessionError> {
        let command_name = match text.find(' ') {
            Some(space_index) => &text[1..space_index],
            None => &text[1..],
        };
        let command = self.extension_runner().get_command(command_name);
        if command.is_some() {
            return Err(AgentSessionError::Upstream(format!(
                "Extension command \"/{command_name}\" cannot be queued. Use prompt() or execute \
                 the command when not streaming."
            )));
        }
        Ok(())
    }

    // =========================================================================
    // Custom / user messages (agent-session.ts:1563-1674)
    // =========================================================================

    /// Upstream `sendCustomMessage`: send a custom message to the session
    /// (creates a CustomMessageEntry).
    ///
    /// - Streaming: queues message, processed when loop pulls from queue
    /// - Streaming + `trigger_turn == Some(false)`: appended to
    ///   state/session once the current turn ends
    /// - Not streaming + trigger: appends to state/session, starts new turn
    /// - Not streaming + no trigger: appends to state/session, no turn
    pub async fn send_custom_message(
        &self,
        message: CustomMessage,
        options: Option<SendCustomMessageOptions>,
    ) -> Result<(), AgentSessionError> {
        let options = options.unwrap_or_default();
        let app_message = custom_message_to_agent_message(message);
        if options.deliver_as == Some(CustomMessageDelivery::NextTurn) {
            self.pending_next_turn_messages
                .lock()
                .expect("pending lock")
                .push(app_message);
        } else if self.is_streaming() && options.trigger_turn != Some(false) {
            if options.deliver_as == Some(CustomMessageDelivery::FollowUp) {
                self.agent.follow_up(app_message);
            } else {
                self.agent.steer(app_message);
            }
        } else if options.trigger_turn == Some(true) {
            self.run_agent_prompt(vec![app_message]).await?;
        } else if self.is_streaming() {
            // Appending now would put the message between an assistant tool
            // call and its result, which providers that validate message order
            // reject on replay. Defer to the end of the turn. Nothing is
            // emitted yet: message events must not describe messages the
            // session tree does not contain.
            self.pending_custom_messages
                .lock()
                .expect("pending lock")
                .push(app_message);
        } else {
            self.append_custom_message(app_message);
        }
        Ok(())
    }

    /// Upstream `_appendCustomMessage`.
    fn append_custom_message(&self, app_message: AgentMessage) {
        if let Some(custom) = custom_message_from_agent_message(&app_message) {
            let _ = self
                .session_manager
                .lock()
                .expect("session lock")
                .append_custom_message_entry(
                    &custom.custom_type,
                    custom.content.clone(),
                    custom.display,
                    custom.details.clone(),
                );
        }
        self.refresh_finalized_context();
        self.emit(AgentSessionEvent::MessageStart {
            message: app_message.clone(),
        });
        self.emit(AgentSessionEvent::MessageEnd {
            message: app_message,
        });
    }

    /// Upstream `_flushPendingCustomMessages`: append custom messages queued
    /// while the agent was running; called once the current turn's tool
    /// results are in agent state and session history.
    fn flush_pending_custom_messages(&self) {
        let pending = {
            let mut pending = self.pending_custom_messages.lock().expect("pending lock");
            if pending.is_empty() {
                return;
            }
            std::mem::take(&mut *pending)
        };
        for app_message in pending {
            self.append_custom_message(app_message);
        }
    }

    /// Upstream `sendUserMessage`: send a user message to the agent. Always
    /// triggers a turn. When the agent is streaming, `deliver_as` specifies
    /// how to queue the message. `expand_prompt_templates` defaults to false.
    pub async fn send_user_message(
        &self,
        content: UserMessageContent,
        options: Option<SendUserMessageOptions>,
    ) -> Result<(), AgentSessionError> {
        let options = options.unwrap_or_default();
        // Normalize content to text string + optional images
        let (text, images) = match content {
            UserMessageContent::Text(text) => (text, None),
            UserMessageContent::Blocks(blocks) => {
                let mut text_parts: Vec<String> = Vec::new();
                let mut images: Vec<ImageContent> = Vec::new();
                for part in blocks {
                    match part {
                        TextOrImageBlock::Text(text) => text_parts.push(text.text),
                        TextOrImageBlock::Image(image) => images.push(image),
                    }
                }
                let text = text_parts.join("\n");
                let images = if images.is_empty() {
                    None
                } else {
                    Some(images)
                };
                (text, images)
            }
        };

        self.prompt(
            text,
            Some(PromptOptions {
                expand_prompt_templates: Some(options.expand_prompt_templates.unwrap_or(false)),
                streaming_behavior: options.deliver_as.as_deref().map(
                    |deliver_as| match deliver_as {
                        "followUp" => StreamingDelivery::FollowUp,
                        _ => StreamingDelivery::Steer,
                    },
                ),
                images,
                source: Some(InputSource::Extension),
                ..PromptOptions::default()
            }),
        )
        .await
    }

    // =========================================================================
    // Queue surface (agent-session.ts:1676-1726)
    // =========================================================================

    /// Upstream `clearQueue`: clear all queued messages and return them
    /// `(steering, followUp)`. Useful for restoring to editor when user
    /// aborts.
    pub fn clear_queue(&self) -> (Vec<String>, Vec<String>) {
        let steering = std::mem::take(&mut *self.steering_messages.lock().expect("steering lock"));
        let follow_up =
            std::mem::take(&mut *self.follow_up_messages.lock().expect("follow up lock"));
        self.agent.clear_all_queues();
        self.emit_queue_update();
        (steering, follow_up)
    }

    /// Upstream `get pendingMessageCount` (steering + follow-up).
    pub fn pending_message_count(&self) -> usize {
        self.steering_messages.lock().expect("steering lock").len()
            + self
                .follow_up_messages
                .lock()
                .expect("follow up lock")
                .len()
    }

    /// Upstream `getSteeringMessages`.
    pub fn get_steering_messages(&self) -> Vec<String> {
        self.steering_messages
            .lock()
            .expect("steering lock")
            .clone()
    }

    /// Upstream `getFollowUpMessages`.
    pub fn get_follow_up_messages(&self) -> Vec<String> {
        self.follow_up_messages
            .lock()
            .expect("follow up lock")
            .clone()
    }

    /// User inputs consumed by the native queue but not successfully appended.
    pub fn get_in_flight_user_messages(&self) -> Vec<String> {
        self.in_flight_user_messages
            .lock()
            .expect("in-flight input lock")
            .clone()
    }

    /// Latched for this runtime because its in-memory transcript may contain
    /// an entry that the session file never accepted.
    pub fn message_persistence_error(&self) -> Option<String> {
        self.message_persistence_error
            .lock()
            .expect("persistence error lock")
            .clone()
    }

    /// Upstream `abort`: abort current operation and wait for agent to become
    /// idle.
    pub async fn abort(&self) {
        if self.is_agent_run_active.load(Ordering::SeqCst) {
            self.agent_run_abort_requested.store(true, Ordering::SeqCst);
        }
        self.abort_retry();
        self.abort_compaction();
        self.abort_branch_summary();
        self.agent.abort();
        self.wait_for_idle().await;
    }

    /// Upstream `waitForIdle`.
    pub async fn wait_for_idle(&self) {
        if self.is_idle() {
            return;
        }
        let mut idle = self.idle_rx.clone();
        while !*idle.borrow_and_update() {
            if idle.changed().await.is_err() {
                return;
            }
        }
    }

    // =========================================================================
    // Model Management (agent-session.ts:1728-1875)
    // =========================================================================

    /// Upstream `_emitModelSelect`.
    async fn emit_model_select(
        &self,
        next_model: &Model,
        previous_model: Option<&Model>,
        source: ModelSelectSource,
    ) {
        if previous_model.is_some_and(|previous| models_are_equal(previous, next_model)) {
            return;
        }
        let runner = self.extension_runner();
        let mut event = json!({
            "type": "model_select",
            "model": serde_json::to_value(next_model).unwrap_or(Value::Null),
            "previousModel": previous_model
                .and_then(|model| serde_json::to_value(model).ok())
                .unwrap_or(Value::Null),
            "source": source.as_str(),
        });
        runner.emit(&mut event).await;
    }

    /// Upstream `setModel`: set model directly. Validates that auth is
    /// configured and saves to the session transcript. Persists to global
    /// defaults only when `options.persist` is true. Errors with the exact
    /// upstream message when no auth is configured for the model.
    pub async fn set_model(
        &self,
        model: Model,
        options: Option<ModelMutationOptions>,
    ) -> Result<(), AgentSessionError> {
        let options = options.unwrap_or_default();
        if self
            .model_runtime
            .check_auth(&model.provider, None)
            .await
            .map_err(|error| AgentSessionError::Upstream(error.to_string()))?
            .is_none()
        {
            return Err(AgentSessionError::Upstream(format!(
                "No API key for {}/{}",
                model.provider, model.id
            )));
        }

        let previous_model = self.model();
        let thinking_level = self.get_thinking_level_for_model_switch(Some(&model), None);
        self.agent.state().model = model.clone();
        let _ = self
            .session_manager
            .lock()
            .expect("session lock")
            .append_model_change(&model.provider, &model.id);
        if options.persist {
            self.settings_manager
                .set_default_model_and_provider(&model.provider, &model.id);
            self.add_persisted_default_to_non_empty_scope(&model);
        }

        // Apply thinking level for the new model. Per-model thinking level
        // overrides take priority over the global default. Model persistence
        // does not implicitly rewrite the global thinking default.
        self.set_thinking_level(thinking_level, Some(options));

        self.emit_model_select(&model, previous_model.as_ref(), ModelSelectSource::Set)
            .await;
        Ok(())
    }

    /// Upstream `_addPersistedDefaultToNonEmptyScope`.
    fn add_persisted_default_to_non_empty_scope(&self, model: &Model) {
        let mut scoped_models = self.scoped_models.lock().expect("scoped lock");
        if scoped_models.is_empty() {
            return;
        }
        if scoped_models
            .iter()
            .any(|scoped| models_are_equal(&scoped.model, model))
        {
            return;
        }

        scoped_models.push(ScopedModel {
            model: model.clone(),
            thinking_level: None,
        });

        let Some(enabled_models) = self.settings_manager.get_enabled_models() else {
            return;
        };
        if enabled_models.is_empty() {
            return;
        }

        let model_reference = format!("{}/{}", model.provider, model.id);
        if enabled_models
            .iter()
            .any(|pattern| pattern.to_lowercase() == model_reference.to_lowercase())
        {
            return;
        }
        let mut updated = enabled_models;
        updated.push(model_reference);
        self.settings_manager.set_enabled_models(Some(updated));
    }

    /// Upstream `cycleModel`: cycle to next/previous model. Uses scoped models
    /// (from --models flag) if available, otherwise all available models.
    pub async fn cycle_model(
        &self,
        direction: CycleDirection,
        options: Option<ModelMutationOptions>,
    ) -> Result<Option<ModelCycleResult>, AgentSessionError> {
        if !self.scoped_models.lock().expect("scoped lock").is_empty() {
            return self
                .cycle_scoped_model(direction, options.unwrap_or_default())
                .await;
        }
        self.cycle_available_model(direction, options.unwrap_or_default())
            .await
    }

    async fn cycle_scoped_model(
        &self,
        direction: CycleDirection,
        options: ModelMutationOptions,
    ) -> Result<Option<ModelCycleResult>, AgentSessionError> {
        let available_ids: HashSet<String> = self
            .model_runtime
            .get_available_snapshot()
            .into_iter()
            .map(|model| format!("{}\0{}", model.provider, model.id))
            .collect();
        let scoped_models: Vec<ScopedModel> = self
            .scoped_models
            .lock()
            .expect("scoped lock")
            .iter()
            .filter(|scoped| {
                available_ids.contains(&format!("{}\0{}", scoped.model.provider, scoped.model.id))
            })
            .cloned()
            .collect();
        if scoped_models.len() <= 1 {
            return Ok(None);
        }

        let current_model = self.model();
        let current_index = scoped_models
            .iter()
            .position(|scoped| {
                current_model
                    .as_ref()
                    .is_some_and(|model| models_are_equal(&scoped.model, model))
            })
            .unwrap_or(0);
        let len = scoped_models.len();
        let next_index = match direction {
            CycleDirection::Forward => (current_index + 1) % len,
            CycleDirection::Backward => (current_index + len - 1) % len,
        };
        let next = &scoped_models[next_index];
        let thinking_level =
            self.get_thinking_level_for_model_switch(Some(&next.model), next.thinking_level);

        // Apply model
        self.agent.state().model = next.model.clone();
        let _ = self
            .session_manager
            .lock()
            .expect("session lock")
            .append_model_change(&next.model.provider, &next.model.id);
        if options.persist {
            self.settings_manager
                .set_default_model_and_provider(&next.model.provider, &next.model.id);
            self.add_persisted_default_to_non_empty_scope(&next.model);
        }

        // Apply thinking level for the new model. Explicit scoped model
        // thinking level overrides defaults; per-model thinking level
        // overrides take priority over the global default. setThinkingLevel
        // clamps to model capabilities.
        self.set_thinking_level(thinking_level, Some(options));

        self.emit_model_select(
            &next.model,
            current_model.as_ref(),
            ModelSelectSource::Cycle,
        )
        .await;

        Ok(Some(ModelCycleResult {
            model: next.model.clone(),
            thinking_level: self.thinking_level(),
            is_scoped: true,
        }))
    }

    async fn cycle_available_model(
        &self,
        direction: CycleDirection,
        options: ModelMutationOptions,
    ) -> Result<Option<ModelCycleResult>, AgentSessionError> {
        let available_models = self.model_runtime.get_available_snapshot();
        if available_models.len() <= 1 {
            return Ok(None);
        }

        let current_model = self.model();
        let current_index = available_models
            .iter()
            .position(|model| {
                current_model
                    .as_ref()
                    .is_some_and(|current| models_are_equal(current, model))
            })
            .unwrap_or(0);
        let len = available_models.len();
        let next_index = match direction {
            CycleDirection::Forward => (current_index + 1) % len,
            CycleDirection::Backward => (current_index + len - 1) % len,
        };
        let next_model = &available_models[next_index];

        let thinking_level = self.get_thinking_level_for_model_switch(Some(next_model), None);
        self.agent.state().model = next_model.clone();
        let _ = self
            .session_manager
            .lock()
            .expect("session lock")
            .append_model_change(&next_model.provider, &next_model.id);
        if options.persist {
            self.settings_manager
                .set_default_model_and_provider(&next_model.provider, &next_model.id);
            self.add_persisted_default_to_non_empty_scope(next_model);
        }

        // Model persistence does not implicitly rewrite the global thinking
        // default.
        self.set_thinking_level(thinking_level, Some(options));

        self.emit_model_select(next_model, current_model.as_ref(), ModelSelectSource::Cycle)
            .await;

        Ok(Some(ModelCycleResult {
            model: next_model.clone(),
            thinking_level: self.thinking_level(),
            is_scoped: false,
        }))
    }

    // =========================================================================
    // Thinking Level Management (agent-session.ts:1877-1960)
    // =========================================================================

    /// Upstream `setThinkingLevel`: clamp to model capabilities based on
    /// available thinking levels; save the clamped level to the session
    /// transcript only if the level actually changes; persist the requested
    /// level to global defaults only when `options.persist` is true.
    pub fn set_thinking_level(&self, level: ThinkingLevel, options: Option<ModelMutationOptions>) {
        let options = options.unwrap_or_default();
        let available_levels = self.get_available_thinking_levels();
        let effective_level = if available_levels.contains(&level) {
            level
        } else {
            self.clamp_thinking_level_impl(level)
        };

        // Only persist if actually changing
        let previous_level = self.agent.state().thinking_level;
        let is_changing = effective_level != previous_level;

        self.agent.state().thinking_level = effective_level;

        if options.persist {
            // SEAM: the settings slice's typed setter only carries the
            // provider-level `ThinkingLevel` union (no `off` variant);
            // persisting `off` as the global default is the one upstream
            // behavior this call cannot express.
            if let Some(wire) = primitives_thinking_level(level) {
                self.settings_manager.set_default_thinking_level(wire);
            }
        }

        if is_changing {
            let _ = self
                .session_manager
                .lock()
                .expect("session lock")
                .append_thinking_level_change(thinking_level_str(effective_level));
            self.emit(AgentSessionEvent::ThinkingLevelChanged {
                level: effective_level,
            });
            // Upstream deliberately does not await this notification.
            let event = json!({
                "type": "thinking_level_select",
                "level": thinking_level_str(effective_level),
                "previousLevel": thinking_level_str(previous_level),
            });
            self.extension_runner().emit_detached(event);
        }
    }

    /// Upstream `cycleThinkingLevel`. Returns `None` if the model doesn't
    /// support thinking.
    pub fn cycle_thinking_level(
        &self,
        options: Option<ModelMutationOptions>,
    ) -> Option<ThinkingLevel> {
        if !self.supports_thinking() {
            return None;
        }

        let levels = self.get_available_thinking_levels();
        let current_index = levels
            .iter()
            .position(|level| *level == self.thinking_level())
            .unwrap_or(0);
        let next_index = (current_index + 1) % levels.len();
        let next_level = levels[next_index];

        self.set_thinking_level(next_level, options);
        Some(next_level)
    }

    /// Upstream `getAvailableThinkingLevels` for the current model. The
    /// provider will clamp to what the specific model supports internally.
    pub fn get_available_thinking_levels(&self) -> Vec<ThinkingLevel> {
        match self.model() {
            None => THINKING_LEVEL_OPTIONS.to_vec(),
            Some(model) => get_supported_thinking_levels(&model)
                .into_iter()
                .filter_map(parse_thinking_level)
                .collect(),
        }
    }

    /// Upstream `supportsThinking`: whether the current model supports
    /// thinking/reasoning.
    pub fn supports_thinking(&self) -> bool {
        self.model().is_some_and(|model| model.reasoning)
    }

    /// Upstream `_getThinkingLevelForModelSwitch`.
    fn get_thinking_level_for_model_switch(
        &self,
        target_model: Option<&Model>,
        explicit_level: Option<ThinkingLevel>,
    ) -> ThinkingLevel {
        if let Some(explicit) = explicit_level {
            return explicit;
        }
        // Per-model default takes priority when switching to a model that has
        // one
        if let Some(model) = target_model {
            if let Some(per_model) = self
                .settings_manager
                .get_model_thinking_level(&model.provider, &model.id)
            {
                return parse_thinking_level(&per_model).unwrap_or(DEFAULT_THINKING_LEVEL);
            }
        }
        self.settings_manager
            .get_default_thinking_level()
            .and_then(|default| parse_thinking_level(&default))
            .unwrap_or(self.thinking_level())
    }

    /// Upstream `_clampThinkingLevel`.
    fn clamp_thinking_level_impl(&self, level: ThinkingLevel) -> ThinkingLevel {
        match self.model() {
            Some(model) => clamp_thinking_level(&model, level),
            None => ThinkingLevel::Off,
        }
    }

    // =========================================================================
    // Queue Mode Management (agent-session.ts:1962-1987)
    // =========================================================================

    /// Upstream `syncQueueModesFromSettings`.
    #[allow(dead_code)] // consumed by `reload` (W3.12)
    fn sync_queue_modes_from_settings(&self) {
        self.agent.set_steering_mode(queue_mode_from_settings(
            &self.settings_manager.get_steering_mode(),
        ));
        self.agent.set_follow_up_mode(queue_mode_from_settings(
            &self.settings_manager.get_follow_up_mode(),
        ));
    }

    /// Upstream `setSteeringMode`: saves to settings.
    pub fn set_steering_mode(&self, mode: QueueMode) {
        self.agent.set_steering_mode(mode);
        self.settings_manager
            .set_steering_mode(queue_mode_str(mode));
    }

    /// Upstream `setFollowUpMode`: saves to settings.
    pub fn set_follow_up_mode(&self, mode: QueueMode) {
        self.agent.set_follow_up_mode(mode);
        self.settings_manager
            .set_follow_up_mode(queue_mode_str(mode));
    }

    // =========================================================================
    // Session Management (setSessionName; pulled up — see module docs)
    // =========================================================================

    /// Upstream `setSessionName`: set a display name for the current session.
    pub fn set_session_name(&self, name: &str) {
        let _ = self
            .session_manager
            .lock()
            .expect("session lock")
            .append_session_info(name);
        self.emit(AgentSessionEvent::SessionInfoChanged {
            name: self.session_name(),
        });
        let extension_event = json!({
            "type": "session_info_changed",
            "name": self.session_name(),
        });
        self.extension_runner().emit_detached(extension_event);
    }

    // =========================================================================
    // Runtime assembly (constructor reach: agent-session.ts:2766-2911)
    // =========================================================================

    /// Upstream `_refreshToolRegistry`.
    /// Rebuild from original definitions instead of composing wrappers. The adapter
    /// survives loadout changes, reload, and MCP's later refreshTools registrations.
    pub fn set_tool_execution_wrapper(&self, wrapper: Option<Arc<ToolExecutionWrapper>>) {
        *self
            .tool_execution_wrapper
            .lock()
            .expect("tool execution wrapper lock") = wrapper;
        self.refresh_tool_registry(RefreshToolRegistryOptions::default());
    }

    /// Upstream `_refreshToolRegistry`.
    fn refresh_tool_registry(&self, options: RefreshToolRegistryOptions) {
        let previous_hidden = self.hidden_tool_names();
        let previous_registry_names: HashSet<String> = self
            .tool_registry
            .lock()
            .expect("registry lock")
            .keys()
            .map(str::to_string)
            .collect();
        let previous_active_tool_names = self.get_active_tool_names();
        let allowed_tool_names = self.allowed_tool_names.clone();
        let excluded_tool_names = self.excluded_tool_names.clone();
        let is_allowed_tool = |name: &str| {
            allowed_tool_names
                .as_ref()
                .map(|allowed| allowed.contains(name))
                .unwrap_or(true)
                && !excluded_tool_names
                    .as_ref()
                    .map(|excluded| excluded.contains(name))
                    .unwrap_or(false)
        };

        let runner = self.extension_runner();
        let registered_tools = runner.get_all_registered_tools();
        let mut all_custom_tools: Vec<RegisteredTool> = registered_tools;
        for definition in &self.custom_tools {
            all_custom_tools.push(RegisteredTool {
                definition: Arc::clone(definition),
                source_info: create_synthetic_source_info(
                    &format!("<sdk:{}>", definition.name),
                    "sdk",
                    None,
                    None,
                    None,
                ),
            });
        }
        all_custom_tools.retain(|tool| is_allowed_tool(&tool.definition.name));

        let mut definition_registry: OrderedMap<ToolDefinitionEntry> = OrderedMap::new();
        for (name, definition) in self
            .base_tool_definitions
            .lock()
            .expect("base tools lock")
            .iter()
        {
            if is_allowed_tool(name) {
                definition_registry.set(
                    name.to_string(),
                    ToolDefinitionEntry {
                        definition: Arc::clone(definition),
                        source_info: create_synthetic_source_info(
                            &format!("<builtin:{name}>"),
                            "builtin",
                            None,
                            None,
                            None,
                        ),
                    },
                );
            }
        }
        for tool in &all_custom_tools {
            definition_registry.set(
                tool.definition.name.clone(),
                ToolDefinitionEntry {
                    definition: Arc::clone(&tool.definition),
                    source_info: tool.source_info.clone(),
                },
            );
        }

        let mut tool_prompt_snippets: OrderedMap<String> = OrderedMap::new();
        let mut tool_prompt_guidelines: OrderedMap<Vec<String>> = OrderedMap::new();
        for entry in definition_registry.values() {
            if let Some(snippet) =
                self.normalize_prompt_snippet(entry.definition.prompt_snippet.as_ref())
            {
                tool_prompt_snippets.set(entry.definition.name.clone(), snippet);
            }
            let guidelines =
                self.normalize_prompt_guidelines(entry.definition.prompt_guidelines.as_ref());
            if !guidelines.is_empty() {
                tool_prompt_guidelines.set(entry.definition.name.clone(), guidelines);
            }
        }
        *self.tool_definitions.lock().expect("definitions lock") = definition_registry;
        *self.tool_prompt_snippets.lock().expect("snippets lock") = tool_prompt_snippets;
        *self.tool_prompt_guidelines.lock().expect("guidelines lock") = tool_prompt_guidelines;

        let wrapped_extension_tools: Vec<AgentTool> =
            wrap_registered_tools(&all_custom_tools, &runner);
        let wrapped_built_in_tools: Vec<AgentTool> = wrap_registered_tools(
            &self.base_tool_definitions_filtered(&is_allowed_tool),
            &runner,
        );

        let mut tool_registry: OrderedMap<Arc<AgentTool>> = OrderedMap::new();
        let execution_wrapper = self
            .tool_execution_wrapper
            .lock()
            .expect("tool execution wrapper lock")
            .clone();
        let wrap_execution = |tool: AgentTool| match &execution_wrapper {
            Some(wrapper) => wrapper(tool),
            None => tool,
        };
        for tool in wrapped_built_in_tools {
            let tool = wrap_execution(tool);
            tool_registry.set(tool.name.clone(), Arc::new(tool));
        }
        for tool in wrapped_extension_tools {
            let tool = wrap_execution(tool);
            tool_registry.set(tool.name.clone(), Arc::new(tool));
        }
        *self.tool_registry.lock().expect("registry lock") = tool_registry;

        let mut next_active_tool_names: Vec<String> = match &options.active_tool_names {
            Some(names) => names.clone(),
            None => previous_active_tool_names,
        };
        next_active_tool_names.retain(|name| is_allowed_tool(name));

        if let Some(allowed) = &allowed_tool_names {
            for tool_name in self.tool_registry.lock().expect("registry lock").keys() {
                if allowed.contains(tool_name) {
                    next_active_tool_names.push(tool_name.to_string());
                }
            }
        } else if options.include_all_extension_tools == Some(true) {
            for tool in &all_custom_tools {
                next_active_tool_names.push(tool.definition.name.clone());
            }
        } else if options.active_tool_names.is_none() {
            for tool_name in self.tool_registry.lock().expect("registry lock").keys() {
                if !previous_registry_names.contains(tool_name)
                    || previous_hidden.contains(tool_name)
                {
                    next_active_tool_names.push(tool_name.to_string());
                }
            }
        }

        let mut unique: Vec<String> = {
            let mut seen = HashSet::new();
            next_active_tool_names
                .into_iter()
                .filter(|name| seen.insert(name.clone()))
                .collect()
        };
        // Pending tools that are registered now become active (v1.0.0).
        unique.extend(
            self.pending_tool_names
                .lock()
                .expect("pending tools lock")
                .iter()
                .cloned(),
        );
        self.set_active_tools_inner(unique);
    }

    fn base_tool_definitions_filtered(
        &self,
        is_allowed: &dyn Fn(&str) -> bool,
    ) -> Vec<RegisteredTool> {
        self.base_tool_definitions
            .lock()
            .expect("base tools lock")
            .iter()
            .filter(|(name, _)| is_allowed(name))
            .map(|(name, definition)| RegisteredTool {
                definition: Arc::clone(definition),
                source_info: create_synthetic_source_info(
                    &format!("<builtin:{name}>"),
                    "builtin",
                    None,
                    None,
                    None,
                ),
            })
            .collect()
    }

    /// Upstream `_buildRuntime`.
    fn build_runtime(&self, options: BuildRuntimeOptions) {
        let base_tool_definitions: Vec<
            Arc<crate::coding_agent::extensions::types::ToolDefinition>,
        > = match &self.base_tools_override {
            Some(override_tools) => override_tools
                .iter()
                .map(base_tools::create_tool_definition_from_agent_tool)
                .collect(),
            None => base_tools::create_all_tool_definitions_with_shell_options(
                &self.cwd,
                self.settings_manager.get_image_auto_resize(),
                crate::coding_agent::core::tools::bash::BashToolOptions {
                    command_prefix: self.settings_manager.get_shell_command_prefix(),
                    shell_path: self.settings_manager.get_shell_path(),
                    ..Default::default()
                },
            ),
        };

        let mut base_tool_map: OrderedMap<
            Arc<crate::coding_agent::extensions::types::ToolDefinition>,
        > = OrderedMap::new();
        for definition in base_tool_definitions {
            base_tool_map.set(definition.name.clone(), definition);
        }
        *self.base_tool_definitions.lock().expect("base tools lock") = base_tool_map;

        let extensions_result = self
            .resource_loader
            .lock()
            .expect("resource loader lock")
            .get_extensions();
        if let Some(flag_values) = &options.flag_values {
            for (name, value) in flag_values.iter() {
                extensions_result
                    .runtime
                    .set_flag_value(name, value.clone());
            }
        }

        let runner = ExtensionRunner::new(
            extensions_result.extensions,
            extensions_result.runtime,
            &self.cwd,
            Arc::clone(&self.session_manager)
                as crate::coding_agent::extensions::types::SessionManagerHandle,
            Arc::new(ModelRegistryHandle(Arc::new(ModelRegistry::new(
                self.model_runtime.clone(),
            )))) as Arc<dyn ProviderRegistryHandle>,
        );
        // Publish before _refreshToolRegistry (it reads registered tools
        // through the runner, like upstream's field assignment).
        *self.extension_runner.lock().expect("runner lock") = Some(runner.clone());

        self.bind_extension_core(&runner);
        self.apply_extension_bindings(&runner);

        let default_active_tool_names: Vec<String> = match &self.base_tools_override {
            Some(override_tools) => override_tools
                .iter()
                .map(|tool| tool.name.clone())
                .collect(),
            None => ["read", "bash", "edit", "write"]
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
        };
        let base_active_tool_names = options
            .active_tool_names
            .unwrap_or(default_active_tool_names);
        self.refresh_tool_registry(RefreshToolRegistryOptions {
            active_tool_names: Some(base_active_tool_names),
            include_all_extension_tools: Some(options.include_all_extension_tools),
        });
    }

    /// Upstream `_bindExtensionCore`.
    fn bind_extension_core(&self, runner: &ExtensionRunner) {
        let commands_session = self.self_weak();
        let get_commands: crate::coding_agent::extensions::types::GetCommandsHandler =
            Arc::new(move || match commands_session.upgrade() {
                Some(session) => session.get_commands(),
                None => Vec::new(),
            });

        let send_message: SendMessageHandler = {
            let runner = runner.clone();
            let session = self.self_weak();
            Arc::new(move |message: &Value, options: &SendMessageOptions| {
                let Some(session) = session.upgrade() else {
                    return;
                };
                let message = message.clone();
                let options = options.clone();
                let runner = runner.clone();
                spawn_extension_action(
                    async move {
                        let custom = serde_json::from_value::<CustomMessage>(message)
                            .map_err(|error| AgentSessionError::Upstream(error.to_string()))?;
                        session
                            .send_custom_message(
                                custom,
                                Some(SendCustomMessageOptions::from_extension(&options)),
                            )
                            .await
                    },
                    move |outcome| {
                        if let Err(error) = outcome {
                            runner.emit_error(ExtensionError {
                                extension_path: "<runtime>".to_string(),
                                event: "send_message".to_string(),
                                error,
                                stack: None,
                            });
                        }
                    },
                );
            })
        };

        let send_user_message: SendUserMessageHandler = {
            let runner = runner.clone();
            let session = self.self_weak();
            Arc::new(move |content: &Value, options: &SendUserMessageOptions| {
                let Some(session) = session.upgrade() else {
                    return;
                };
                let content = parse_user_message_content(content);
                let options = options.clone();
                let runner = runner.clone();
                spawn_extension_action(
                    async move { session.send_user_message(content, Some(options)).await },
                    move |outcome| {
                        if let Err(error) = outcome {
                            runner.emit_error(ExtensionError {
                                extension_path: "<runtime>".to_string(),
                                event: "send_user_message".to_string(),
                                error,
                                stack: None,
                            });
                        }
                    },
                );
            })
        };

        let append_session = self.self_weak();
        let append_entry: crate::coding_agent::extensions::types::AppendEntryHandler =
            Arc::new(move |custom_type: &str, data: Option<&Value>| {
                let Some(session) = append_session.upgrade() else {
                    return;
                };
                let Ok(entry_id) = session
                    .session_manager
                    .lock()
                    .expect("session lock")
                    .append_custom_entry(custom_type, data.cloned())
                else {
                    return;
                };
                let entry = session
                    .session_manager
                    .lock()
                    .expect("session lock")
                    .get_entry(&entry_id)
                    .cloned();
                if let Some(entry) = entry {
                    session.emit(AgentSessionEvent::EntryAppended { entry });
                }
            });

        let name_session = self.self_weak();
        let set_session_name: SetSessionNameHandler = Arc::new(move |name: &str| {
            if let Some(session) = name_session.upgrade() {
                session.set_session_name(name);
            }
        });
        let get_name_session = self.self_weak();
        let get_session_name: GetSessionNameHandler = Arc::new(move || {
            get_name_session
                .upgrade()
                .and_then(|session| session.session_name())
        });
        let label_session = self.self_weak();
        let set_label: SetLabelHandler = Arc::new(move |entry_id: &str, label: Option<&str>| {
            if let Some(session) = label_session.upgrade() {
                let _ = session
                    .session_manager
                    .lock()
                    .expect("session lock")
                    .append_label_change(entry_id, label);
            }
        });
        let active_tools_session = self.self_weak();
        let get_active_tools: crate::coding_agent::extensions::types::GetActiveToolsHandler =
            Arc::new(move || {
                active_tools_session
                    .upgrade()
                    .map(|session| session.get_active_tool_names())
                    .unwrap_or_default()
            });
        let all_tools_session = self.self_weak();
        let get_all_tools: crate::coding_agent::extensions::types::GetAllToolsHandler =
            Arc::new(move || {
                all_tools_session
                    .upgrade()
                    .map(|session| session.get_all_tools())
                    .unwrap_or_default()
            });
        let set_tools_session = self.self_weak();
        let set_active_tools: SetActiveToolsHandler = Arc::new(move |tool_names: &[String]| {
            if let Some(session) = set_tools_session.upgrade() {
                session.set_active_tools_by_name(tool_names.to_vec());
            }
        });
        let refresh_session = self.self_weak();
        let refresh_tools: crate::coding_agent::extensions::types::RefreshToolsHandler =
            Arc::new(move || {
                if let Some(session) = refresh_session.upgrade() {
                    session.refresh_tool_registry(RefreshToolRegistryOptions::default());
                }
            });
        let set_model_session = self.self_weak();
        let set_model: crate::coding_agent::extensions::types::SetModelHandler =
            Arc::new(move |model: &Value| {
                use crate::coding_agent::extensions::types::CommandFuture;
                let Some(session) = set_model_session.upgrade() else {
                    return Ok(CommandFuture::resolved(false));
                };
                let Ok(model) = serde_json::from_value::<Model>(model.clone()) else {
                    return Ok(CommandFuture::resolved(false));
                };
                if !session.model_runtime.has_configured_auth(&model.provider) {
                    return Ok(CommandFuture::resolved(false));
                }
                CommandFuture::spawn(async move {
                    session
                        .set_model(model, None)
                        .await
                        .map_err(|error| error.to_string())?;
                    Ok(true)
                })
            });
        let thinking_session = self.self_weak();
        let get_thinking_level: GetThinkingLevelHandler = Arc::new(move || {
            thinking_session
                .upgrade()
                .map(|session| session.thinking_level())
                .unwrap_or(ThinkingLevel::Off)
        });
        let set_thinking_session = self.self_weak();
        let set_thinking_level: SetThinkingLevelHandler = Arc::new(move |level: ThinkingLevel| {
            if let Some(session) = set_thinking_session.upgrade() {
                session.set_thinking_level(level, None);
            }
        });
        let settings_session = self.self_weak();
        let get_settings: crate::coding_agent::extensions::types::GetSettingsHandler =
            Arc::new(move || {
                let Some(session) = settings_session.upgrade() else {
                    return Value::Object(serde_json::Map::new());
                };
                // Upstream `getSettings()`: a copy of the effective settings
                // (global and project merged, project wins). The settings
                // slice exposes no merged-document getter yet, so the object
                // merge is restated here (disclosed seam).
                let global = serde_json::to_value(session.settings_manager.get_global_settings())
                    .unwrap_or(Value::Object(serde_json::Map::new()));
                let project = serde_json::to_value(session.settings_manager.get_project_settings())
                    .unwrap_or(Value::Object(serde_json::Map::new()));
                merge_settings_documents(&global, &project)
            });

        runner.bind_core(
            Arc::new(ExtensionActions {
                send_message,
                send_user_message,
                append_entry,
                set_session_name,
                get_session_name,
                set_label,
                get_active_tools,
                get_all_tools,
                get_settings,
                set_active_tools,
                refresh_tools,
                get_commands,
                set_model,
                get_thinking_level,
                set_thinking_level,
            }),
            Arc::new(self.context_actions()),
            None,
        );
    }

    fn get_commands(&self) -> Vec<Value> {
        let runner = self.extension_runner();
        let extension_commands: Vec<Value> = runner
            .get_registered_commands()
            .iter()
            .map(|command: &ResolvedCommand| {
                json!({
                    "name": command.invocation_name,
                    "description": command.description(),
                    "source": "extension",
                    "sourceInfo":
                        serde_json::to_value(&command.command.source_info).unwrap_or(Value::Null),
                })
            })
            .collect();
        let templates: Vec<Value> = self
            .prompt_templates()
            .iter()
            .map(|template| {
                json!({
                    "name": template.name,
                    "description": template.description,
                    "source": "prompt",
                    "sourceInfo": serde_json::to_value(&template.source_info).unwrap_or(Value::Null),
                })
            })
            .collect();
        let skills: Vec<Value> = self
            .resource_loader
            .lock()
            .expect("resource loader lock")
            .get_skills()
            .skills
            .iter()
            .map(|skill| {
                json!({
                    "name": format!("skill:{}", skill.name),
                    "description": skill.description,
                    "source": "skill",
                    "sourceInfo": serde_json::to_value(&skill.source_info).unwrap_or(Value::Null),
                })
            })
            .collect();
        let mut commands = extension_commands;
        commands.extend(templates);
        commands.extend(skills);
        commands
    }

    fn context_actions(&self) -> crate::coding_agent::extensions::types::ExtensionContextActions {
        let model_session = self.self_weak();
        let get_model: GetModelHandler = Arc::new(move || {
            model_session
                .upgrade()
                .and_then(|session| session.model())
                .and_then(|model| serde_json::to_value(model).ok())
        });
        let scoped_session = self.self_weak();
        let get_scoped_models: GetScopedModelsHandler = Arc::new(move || {
            scoped_session
                .upgrade()
                .map(|session| {
                    session
                        .scoped_models()
                        .iter()
                        .map(|scoped| {
                            json!({
                                "model": serde_json::to_value(&scoped.model).unwrap_or(Value::Null),
                                "thinkingLevel": scoped
                                    .thinking_level
                                    .map(|level| Value::String(
                                        thinking_level_str(level).to_string()
                                    ))
                                    .unwrap_or(Value::Null),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default()
        });
        let idle_session = self.self_weak();
        let is_idle: IsIdleHandler = Arc::new(move || {
            idle_session
                .upgrade()
                .is_some_and(|session| session.is_idle())
        });
        let trusted_session = self.self_weak();
        let is_project_trusted: IsProjectTrustedHandler = Arc::new(move || {
            trusted_session
                .upgrade()
                .map(|session| session.settings_manager.is_project_trusted())
                .unwrap_or(true)
        });
        let signal_session = self.self_weak();
        let get_signal: crate::coding_agent::extensions::types::GetSignalHandler =
            Arc::new(move || {
                let session = signal_session.upgrade()?;
                let token = session.agent.signal()?;
                let signal = Arc::new(AbortSignal::new());
                if token.is_cancelled() {
                    signal.abort();
                } else {
                    let forward = Arc::clone(&signal);
                    tokio::spawn(async move {
                        token.cancelled().await;
                        forward.abort();
                    });
                }
                Some(signal)
            });
        let abort_session = self.self_weak();
        let abort: crate::coding_agent::extensions::types::AbortHandler = Arc::new(move || {
            if let Some(session) = abort_session.upgrade() {
                let abort_handler = session
                    .extension_abort_handler
                    .lock()
                    .expect("abort handler lock")
                    .clone();
                if let Some(abort_handler) = abort_handler {
                    abort_handler();
                    return;
                }
                tokio::spawn(async move {
                    session.abort().await;
                });
            }
        });
        let pending_session = self.self_weak();
        let has_pending_messages: crate::coding_agent::extensions::types::HasPendingMessagesHandler =
            Arc::new(move || {
                pending_session
                    .upgrade()
                    .is_some_and(|session| session.pending_message_count() > 0)
            });
        let shutdown_session = self.self_weak();
        let shutdown: crate::coding_agent::extensions::types::ShutdownHandler =
            Arc::new(move || {
                if let Some(session) = shutdown_session.upgrade() {
                    if let Some(handler) = session
                        .extension_shutdown_handler
                        .lock()
                        .expect("shutdown handler lock")
                        .clone()
                    {
                        handler();
                    }
                }
            });
        let usage_session = self.self_weak();
        let get_context_usage: GetContextUsageHandler = Arc::new(move || {
            usage_session
                .upgrade()
                .and_then(|session| session.get_context_usage().ok().flatten())
        });
        let compact_session = self.self_weak();
        let compact: CompactHandler = Arc::new(move |options: Option<CompactOptions>| {
            // Upstream fires an async IIFE and routes the outcome to the
            // callbacks. Keep it on the native async executor so extension
            // handlers can await timers/I/O without a block_on thread.
            let Some(session) = compact_session.upgrade() else {
                return;
            };
            let custom_instructions = options
                .as_ref()
                .and_then(|options| options.custom_instructions.clone());
            let on_complete = options
                .as_ref()
                .and_then(|options| options.on_complete.clone());
            let on_error = options
                .as_ref()
                .and_then(|options| options.on_error.clone());
            spawn_extension_action(
                async move { session.compact(custom_instructions).await },
                move |result| match result {
                    Ok(result) => {
                        if let Some(on_complete) = on_complete {
                            on_complete(&result);
                        }
                    }
                    Err(error) => {
                        if let Some(on_error) = on_error {
                            on_error(&error);
                        }
                    }
                },
            );
        });
        let prompt_session = self.self_weak();
        let get_system_prompt: GetSystemPromptHandler = Arc::new(move || {
            prompt_session
                .upgrade()
                .map(|session| session.system_prompt())
                .unwrap_or_default()
        });
        let options_session = self.self_weak();
        let get_system_prompt_options: GetSystemPromptOptionsHandler = Arc::new(move || {
            options_session
                .upgrade()
                .map(|session| to_build_options(&session.base_system_prompt_options().clone()))
                .unwrap_or_else(|| BuildSystemPromptOptions::with_cwd(""))
        });

        crate::coding_agent::extensions::types::ExtensionContextActions {
            get_model,
            get_scoped_models,
            is_idle,
            is_project_trusted,
            get_signal,
            abort,
            has_pending_messages,
            shutdown,
            get_context_usage,
            compact,
            get_system_prompt,
            get_system_prompt_options: Some(get_system_prompt_options),
            execute_tool: None,
            get_callable_tools: None,
        }
    }

    /// Upstream `_applyExtensionBindings`.
    fn apply_extension_bindings(&self, runner: &ExtensionRunner) {
        runner.set_ui_context(
            self.extension_ui_context.lock().expect("ui lock").clone(),
            *self.extension_mode.lock().expect("mode lock"),
        );
        // The actions stay installed across reloads (upstream reads the field
        // without consuming it), so rebind a clone of the handlers.
        let command_actions = self
            .extension_command_context_actions
            .lock()
            .expect("command actions lock")
            .as_ref()
            .map(clone_command_context_actions);
        runner.bind_command_context(command_actions);

        if let Some(unsubscriber) = self
            .extension_error_unsubscriber
            .lock()
            .expect("unsubscriber lock")
            .take()
        {
            unsubscriber.unsubscribe();
        }
        let listener = self
            .extension_error_listener
            .lock()
            .expect("listener lock")
            .clone();
        *self
            .extension_error_unsubscriber
            .lock()
            .expect("unsubscriber lock") = listener.map(|listener| runner.on_error(listener));
    }

    // =========================================================================
    // Retry/compaction support shared with the upper half
    // =========================================================================

    /// Upstream `_isRetryableError` (overloaded, rate limit, server errors).
    /// Context overflow errors are NOT retryable (handled by compaction
    /// instead).
    fn is_retryable_error(&self, message: &AssistantMessage) -> bool {
        // Context overflow is handled by compaction, not retry.
        if is_context_overflow(
            message,
            self.model_for_message(message)
                .or_else(|| self.model())
                .map(|model| model.context_window),
        ) {
            return false;
        }
        is_retryable_assistant_error(message)
    }

    fn retry_settings(&self) -> RetrySettings {
        self.settings_manager.get_retry_settings()
    }

    /// Upstream `_checkCompaction` decision logic
    /// (agent-session.ts:2227-2331).
    ///
    /// Automatic cases:
    /// 1. Overflow with retry: compact and retry the turn once.
    /// 2. Overflow without retry: compact but preserve the completed response.
    /// 3. Threshold without retry: compact without retrying.
    async fn check_compaction(
        &self,
        assistant_message: &AssistantMessage,
        skip_aborted_check: bool,
    ) -> Result<bool, AgentSessionError> {
        let settings = self.compaction_settings_for(self.model().as_ref())?;
        if !settings.enabled {
            return Ok(false);
        }

        // Skip if message was aborted (user cancelled) - unless
        // skipAbortedCheck is false
        if skip_aborted_check && assistant_message.stop_reason == StopReason::Aborted {
            return Ok(false);
        }

        let context_window = self.model().map(|model| model.context_window).unwrap_or(0);

        // Skip overflow check if the message came from a different model. This
        // handles switching from a smaller-context model to a larger-context
        // model - the overflow error from the old model shouldn't trigger
        // compaction for the new model.
        let same_model = self.model().as_ref().is_some_and(|model| {
            assistant_message.provider == model.provider && assistant_message.model == model.id
        });

        // Skip compaction checks if this assistant message is older than the
        // latest compaction boundary. This prevents a stale pre-compaction
        // usage/error from retriggering compaction on the first prompt after
        // compaction. Upstream compares against
        // `new Date(entry.timestamp).getTime()`: an unparseable timestamp is
        // NaN and the comparison is false, so a missing or unparseable
        // boundary never skips.
        let compaction_timestamp = {
            let manager = self.session_manager.lock().expect("session lock");
            let branch = manager.get_branch(None);
            get_latest_compaction_entry(&branch).and_then(|entry| {
                crate::coding_agent::core::messages::parse_epoch_millis(entry.timestamp())
            })
        };
        let assistant_is_from_before_compaction = match compaction_timestamp {
            Some(timestamp) => assistant_message.timestamp <= timestamp,
            None => false,
        };
        if assistant_is_from_before_compaction {
            return Ok(false);
        }

        // Automatic cases 1 and 2: context overflow. A length stop is
        // recoverable when output ended below the model's original desired
        // limit.
        let context_overflow =
            same_model && is_context_overflow(assistant_message, Some(context_window));
        let recoverable_length = same_model
            && is_recoverable_length(
                assistant_message,
                self.model().map(|model| model.max_tokens).unwrap_or(0),
            );
        if context_overflow || recoverable_length {
            let will_retry = assistant_message.stop_reason != StopReason::Stop;

            // Case 2: the response completed successfully. Compact, but do not
            // retry because agent.continue() cannot continue from a completed
            // assistant response.
            if !will_retry {
                return self
                    .run_auto_compaction(CompactionReason::Overflow, false)
                    .await;
            }

            if self.overflow_recovery_attempted.load(Ordering::SeqCst) {
                let error_message = if context_overflow {
                    "Context overflow recovery failed after one compact-and-retry attempt. Try \
                     reducing context or switching to a larger-context model."
                } else {
                    "Truncated response recovery failed after one compact-and-retry attempt."
                };
                self.emit(AgentSessionEvent::CompactionEnd {
                    reason: CompactionReason::Overflow,
                    result: None,
                    aborted: false,
                    will_retry: false,
                    error_message: Some(error_message.to_string()),
                });
                self.emit_session_compact_failed(json!({
                    "reason": "overflow",
                    "errorMessage": error_message,
                    "aborted": false,
                    "willRetry": false,
                    "fromExtension": false,
                }))
                .await;
                return Ok(false);
            }

            // Case 1: remove the failed or truncated message from agent state,
            // compact, and retry once. The message remains in session history
            // but is excluded from retry context.
            self.overflow_recovery_attempted
                .store(true, Ordering::SeqCst);
            {
                let mut state = self.agent.state();
                if state
                    .messages
                    .last()
                    .is_some_and(|message| message.role() == "assistant")
                {
                    state.messages.pop();
                }
            }
            return self
                .run_auto_compaction(CompactionReason::Overflow, will_retry)
                .await;
        }

        // Case 3: threshold compaction without retry. For error messages or
        // all-zero usage messages, estimate from the last valid response. This
        // ensures sessions that hit persistent API errors (e.g. 529) or
        // malformed zero-usage responses can still compact and do not reset
        // context accounting.
        let direct_context_tokens = calculate_context_tokens(assistant_message.usage);
        let context_tokens = if assistant_message.stop_reason == StopReason::Error
            || direct_context_tokens == 0
        {
            let messages = self.agent.state().messages.clone();
            let estimate = estimate_context_tokens(&messages);
            if let Some(last_usage_index) = estimate.last_usage_index {
                // Verify the usage source is post-compaction. Kept
                // pre-compaction messages have stale usage reflecting the old
                // (larger) context and would falsely trigger compaction right
                // after one just finished.
                if let Some(AgentMessage::Assistant(usage_msg)) = messages.get(last_usage_index) {
                    if let Some(timestamp) = compaction_timestamp {
                        if usage_msg.timestamp <= timestamp {
                            return Ok(false);
                        }
                    }
                }
            }
            estimate.tokens
        } else {
            direct_context_tokens
        };
        if should_compact(context_tokens, context_window, settings) {
            return self
                .run_auto_compaction(CompactionReason::Threshold, false)
                .await;
        }
        Ok(false)
    }

    /// Upstream `_runDefaultCompaction`: generate Pi's built-in compaction
    /// summary for manual and automatic compaction.
    #[allow(clippy::too_many_arguments)]
    async fn run_default_compaction(
        &self,
        preparation: CompactionPreparation,
        request_model: &Model,
        api_key: Option<&str>,
        headers: Option<&std::collections::BTreeMap<String, String>>,
        env: Option<&crate::ai::types::options::ProviderEnv>,
        custom_instructions: Option<&str>,
        signal: &CancellationToken,
        reason: CompactionReason,
    ) -> Result<CompactionResult, AgentSessionError> {
        let retry_settings = self.retry_settings();
        let mut callbacks =
            self.summarization_retry_callbacks(SummarizationRetrySource::Compaction { reason });
        let models_stream_fn =
            crate::coding_agent::core::compaction::models_stream_fn(Arc::clone(&self.agent.models));
        let flattened_headers = headers.map(flatten_headers);
        let options = crate::coding_agent::core::compaction::CompactOptions {
            model: request_model,
            api_key,
            headers: flattened_headers.as_ref(),
            custom_instructions,
            signal: Some(signal),
            thinking_level: Some(self.thinking_level()),
            stream_fn: Some(models_stream_fn),
            env,
            retry: Some(&retry_policy_from_settings(&retry_settings)),
            callbacks: &mut callbacks,
            session_id: None,
        };
        crate::coding_agent::core::compaction::compact(preparation, options)
            .await
            .map_err(AgentSessionError::Upstream)
    }

    /// Upstream `_clearManualCompactionState`.
    fn clear_manual_compaction_state(&self) {
        *self.compaction_abort.lock().expect("compaction lock") = None;
        self.resolve_idle_wait_if_idle();
    }

    /// Upstream `_summarizationRetryCallbacks`: retry policy + callbacks shared
    /// by compaction and branch-summary summarization calls. The callbacks
    /// reach the session through a `Weak` handle (see the architecture seam).
    fn summarization_retry_callbacks(
        &self,
        source: SummarizationRetrySource,
    ) -> crate::ai::retry::RetryCallbacks {
        let scheduled = self.self_weak();
        let attempt_start = self.self_weak();
        let source_for_start = source.clone();
        let finished = self.self_weak();
        crate::ai::retry::RetryCallbacks {
            on_retry_scheduled: Some(Box::new(move |attempt, max_attempts, delay_ms, error| {
                if let Some(session) = scheduled.upgrade() {
                    session.emit(AgentSessionEvent::SummarizationRetryScheduled {
                        attempt,
                        max_attempts: max_attempts as i64,
                        delay_ms,
                        error_message: error.to_string(),
                    });
                }
            })),
            on_retry_attempt_start: Some(Box::new(move || {
                if let Some(session) = attempt_start.upgrade() {
                    session.emit(AgentSessionEvent::SummarizationRetryAttemptStart {
                        source: source_for_start.clone(),
                    });
                }
            })),
            on_retry_finished: Some(Box::new(move |_, _, _| {
                if let Some(session) = finished.upgrade() {
                    session.emit(AgentSessionEvent::SummarizationRetryFinished);
                }
            })),
        }
    }

    /// Upstream `_runAutoCompaction`: execute threshold or overflow compaction.
    /// Manual compaction uses [`AgentSession::compact`] instead. Mirrors the
    /// upstream control flow exactly: internal failures surface as
    /// `compaction_end` failure events (never propagated), the return value
    /// says whether the post-run loop should call `agent.continue()`.
    async fn run_auto_compaction(
        &self,
        reason: CompactionReason,
        will_retry: bool,
    ) -> Result<bool, AgentSessionError> {
        let reason_str = compaction_reason_str(reason);
        let mut started = false;
        let mut from_extension = false;

        let run: Result<bool, AgentSessionError> = async {
            let model = match self.model() {
                Some(model) => model,
                // Upstream `if (!model) return false`.
                None => return Ok(false),
            };
            let settings = self.compaction_settings_for(Some(&model))?;
            let auth = self.get_summarization_request_auth(&model).await?;

            let (branch_entries_json, path_entries) = {
                let manager = self.session_manager.lock().expect("session lock");
                let branch_entries_json =
                    serde_json::to_value(manager.get_branch(None)).unwrap_or(Value::Null);
                (branch_entries_json, compaction_path_entries(&manager))
            };
            let Some(preparation) = prepare_compaction(&path_entries, settings) else {
                return Ok(false);
            };

            self.emit(AgentSessionEvent::CompactionStart { reason });
            let token = CancellationToken::new();
            *self.auto_compaction_abort.lock().expect("auto lock") = Some(token.clone());
            started = true;

            let mut extension_compaction: Option<ExtensionCompaction> = None;
            let runner = self.extension_runner();
            if runner.has_handlers("session_before_compact") {
                let mut event = json!({
                    "type": "session_before_compact",
                    "preparation": preparation_json(&preparation),
                    "branchEntries": branch_entries_json,
                    "customInstructions": Value::Null,
                    "reason": reason_str,
                    "willRetry": will_retry,
                });
                let result = runner.emit(&mut event).await;
                if let Some(result) = result {
                    if result
                        .get("cancel")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        self.emit(AgentSessionEvent::CompactionEnd {
                            reason,
                            result: None,
                            aborted: true,
                            will_retry: false,
                            error_message: None,
                        });
                        self.emit_session_compact_failed(json!({
                            "reason": reason_str,
                            "aborted": true,
                            "willRetry": false,
                            "fromExtension": false,
                        }))
                        .await;
                        return Ok(false);
                    }
                    if let Some(compaction) = result.get("compaction").filter(|c| !c.is_null()) {
                        extension_compaction = Some(
                            serde_json::from_value::<ExtensionCompaction>(compaction.clone())
                                .map_err(|error| AgentSessionError::Upstream(error.to_string()))?,
                        );
                        from_extension = true;
                    }
                }
            }

            let (summary, first_kept_entry_id, tokens_before, usage, details) =
                match extension_compaction {
                    Some(extension) => extension.into_fields(),
                    None => {
                        let compact_result = self
                            .run_default_compaction(
                                preparation,
                                &auth.model,
                                auth.api_key.as_deref(),
                                auth.headers.as_ref(),
                                auth.env.as_ref(),
                                None,
                                &token,
                                reason,
                            )
                            .await?;
                        let details = compact_result
                            .details
                            .as_ref()
                            .map(|details| serde_json::to_value(details).unwrap_or(Value::Null));
                        (
                            compact_result.summary,
                            compact_result.first_kept_entry_id,
                            compact_result.tokens_before,
                            compact_result.usage,
                            details,
                        )
                    }
                };

            if token.is_cancelled() {
                self.emit(AgentSessionEvent::CompactionEnd {
                    reason,
                    result: None,
                    aborted: true,
                    will_retry: false,
                    error_message: None,
                });
                self.emit_session_compact_failed(json!({
                    "reason": reason_str,
                    "aborted": true,
                    "willRetry": false,
                    "fromExtension": from_extension,
                }))
                .await;
                return Ok(false);
            }

            self.append_compaction_and_rebuild(
                &summary,
                &first_kept_entry_id,
                tokens_before,
                details.clone(),
                from_extension,
                usage,
            )?;

            // Get the saved compaction entry for the extension event.
            let saved_compaction_entry = self
                .session_manager
                .lock()
                .expect("session lock")
                .get_entries()
                .into_iter()
                .rev()
                .find(|entry| {
                    matches!(entry, SessionEntry::Compaction(compaction)
                        if compaction.summary == summary)
                });
            if let Some(entry) = saved_compaction_entry {
                let mut event = json!({
                    "type": "session_compact",
                    "compactionEntry": serde_json::to_value(&entry).unwrap_or(Value::Null),
                    "fromExtension": from_extension,
                    "reason": reason_str,
                    "willRetry": will_retry,
                });
                runner.emit(&mut event).await;
            }

            let result = json!({
                "summary": summary,
                "firstKeptEntryId": first_kept_entry_id,
                "tokensBefore": tokens_before,
                "estimatedTokensAfter": estimate_messages_tokens(
                    &self.agent.state().messages,
                ),
                "usage": usage
                    .map(|usage| serde_json::to_value(usage).unwrap_or(Value::Null))
                    .unwrap_or(Value::Null),
                "details": details,
            });
            self.emit(AgentSessionEvent::CompactionEnd {
                reason,
                result: Some(result),
                aborted: false,
                will_retry,
                error_message: None,
            });

            if will_retry {
                let last_msg = self.agent.state().messages.last().cloned();
                // The overflow response was persisted on message_end before
                // _checkCompaction() removed it from agent state. Rebuilding
                // state from the new compaction can restore that kept entry,
                // leaving an assistant as the final message. agent.continue()
                // rejects that state, so remove the retriable error or
                // truncated-length response again before continuing the
                // interrupted turn.
                if let Some(AgentMessage::Assistant(last)) = last_msg {
                    if matches!(last.stop_reason, StopReason::Error | StopReason::Length) {
                        self.agent.state().messages.pop();
                    }
                }
                return Ok(true);
            }

            // Auto-compaction can complete while follow-up/steering/custom
            // messages are waiting. Continue once so queued messages are
            // delivered.
            Ok(self.agent.has_queued_messages())
        }
        .await;

        let outcome = match run {
            Ok(value) => Ok(value),
            Err(error) => {
                let error_message = error.to_string();
                if started {
                    let formatted_error_message = if reason == CompactionReason::Overflow {
                        format!("Context overflow recovery failed: {error_message}")
                    } else {
                        format!("Auto-compaction failed: {error_message}")
                    };
                    self.emit(AgentSessionEvent::CompactionEnd {
                        reason,
                        result: None,
                        aborted: false,
                        will_retry: false,
                        error_message: Some(formatted_error_message.clone()),
                    });
                    self.emit_session_compact_failed(json!({
                        "reason": reason_str,
                        "errorMessage": formatted_error_message,
                        "aborted": false,
                        "willRetry": false,
                        "fromExtension": from_extension,
                    }))
                    .await;
                }
                Ok(false)
            }
        };

        // finally {
        *self.auto_compaction_abort.lock().expect("auto lock") = None;
        self.resolve_idle_wait_if_idle();
        // }
        outcome
    }

    /// Upstream `_prepareRetry`: prepare a retryable error for continuation
    /// with exponential backoff. `Ok(true)` means the caller should continue
    /// the agent.
    async fn prepare_retry(&self, message: &AssistantMessage) -> Result<bool, AgentSessionError> {
        let settings = self.retry_settings();
        if !settings.enabled {
            return Ok(false);
        }

        let attempt = self.retry_attempt.fetch_add(1, Ordering::SeqCst) + 1;
        if attempt as i64 > settings.max_retries {
            // Preserve the completed attempt count so post-run handling can
            // emit the final failure.
            self.retry_attempt.fetch_sub(1, Ordering::SeqCst);
            return Ok(false);
        }

        let delay_ms = retry_delay_ms_from_settings(&settings, attempt);

        self.emit(AgentSessionEvent::AutoRetryStart {
            attempt,
            max_attempts: settings.max_retries,
            delay_ms,
            error_message: message
                .error_message
                .clone()
                .unwrap_or_else(|| "Unknown error".to_string()),
        });

        // Keep the failed attempt in raw history while durably omitting it
        // from the model projection.
        self.omit_recovery_attempt(&AgentMessage::Assistant(message.clone()), &[])?;

        // Wait with exponential backoff (abortable)
        let token = CancellationToken::new();
        *self.retry_abort.lock().expect("retry lock") = Some(token.clone());
        let aborted_during_sleep = tokio::select! {
            () = token.cancelled() => true,
            () = tokio::time::sleep(std::time::Duration::from_millis(delay_ms)) => false,
        };
        *self.retry_abort.lock().expect("retry lock") = None;

        if aborted_during_sleep {
            // Aborted during sleep - emit end event so UI can clean up
            self.finish_cancelled_retry();
            return Ok(false);
        }

        Ok(true)
    }

    /// Shared tail of `compact`/`_runAutoCompaction` (upstream inline):
    /// append the compaction entry and rebuild agent state from the session
    /// context.
    fn append_compaction_and_rebuild(
        &self,
        summary: &str,
        first_kept_entry_id: &str,
        tokens_before: u64,
        details: Option<Value>,
        from_extension: bool,
        usage: Option<Usage>,
    ) -> Result<(), AgentSessionError> {
        {
            let mut manager = self.session_manager.lock().expect("session lock");
            manager
                .append_compaction(
                    summary,
                    Some(first_kept_entry_id),
                    tokens_before as i64,
                    details,
                    Some(from_extension),
                    usage,
                )
                .map_err(session_manager_error)?;
        }
        self.refresh_finalized_context();
        Ok(())
    }

    /// `settingsManager.getCompactionSettings(model)` — errors carry the
    /// upstream message (invalid setting strings throw upstream).
    fn compaction_settings_for(
        &self,
        model: Option<&Model>,
    ) -> Result<CompactionSettings, AgentSessionError> {
        let settings: Result<SettingsCompactionSettings, String> = match model {
            Some(model) => self
                .settings_manager
                .get_compaction_settings_for(&model.provider, &model.id),
            None => self.settings_manager.get_compaction_settings(),
        };
        let settings = settings.map_err(AgentSessionError::Upstream)?;
        Ok(CompactionSettings {
            enabled: settings.enabled,
            reserve_tokens: settings.reserve_tokens.max(0) as u64,
            keep_recent_tokens: settings.keep_recent_tokens.max(0) as u64,
        })
    }

    // =========================================================================
    // Abort plumbing (upstream aborts; execution is W3.12)
    // =========================================================================

    /// Upstream `abortRetry`: cancel in-progress retry.
    pub fn abort_retry(&self) {
        if let Some(controller) = self.retry_abort.lock().expect("retry lock").as_ref() {
            controller.cancel();
        }
    }

    /// Upstream `abortCompaction`: cancel in-progress compaction (manual or
    /// auto).
    pub fn abort_compaction(&self) {
        if let Some(controller) = self
            .compaction_abort
            .lock()
            .expect("compaction lock")
            .as_ref()
        {
            controller.cancel();
        }
        if let Some(controller) = self
            .auto_compaction_abort
            .lock()
            .expect("auto lock")
            .as_ref()
        {
            controller.cancel();
        }
    }

    /// Upstream `abortBranchSummary`: cancel in-progress branch summarization.
    pub fn abort_branch_summary(&self) {
        if let Some(controller) = self
            .branch_summary_abort
            .lock()
            .expect("branch lock")
            .as_ref()
        {
            controller.cancel();
        }
    }

    /// Upstream `abortBash`: cancel running bash command.
    pub fn abort_bash(&self) {
        for controller in self
            .bash_abort_controllers
            .lock()
            .expect("bash lock")
            .clone()
        {
            controller.cancel();
        }
    }
    /// Upstream `get isBashRunning`.
    pub fn is_bash_running(&self) -> bool {
        !self
            .bash_abort_controllers
            .lock()
            .expect("bash lock")
            .is_empty()
    }

    /// Upstream `get hasPendingBashMessages`.
    pub fn has_pending_bash_messages(&self) -> bool {
        !self
            .pending_bash_messages
            .lock()
            .expect("bash pending lock")
            .is_empty()
    }

    /// Upstream `_flushPendingBashMessages`: flush pending bash messages to
    /// agent state and session. Called after agent turn completes to maintain
    /// proper message ordering.
    fn flush_pending_bash_messages(&self) {
        let pending = {
            let mut pending = self
                .pending_bash_messages
                .lock()
                .expect("bash pending lock");
            if pending.is_empty() {
                return;
            }
            std::mem::take(&mut *pending)
        };
        for bash_message in pending {
            let message = AgentMessage::Custom(bash_execution_agent_message(&bash_message));
            // Save to session (the finalized context is rebuilt once below).
            let _ = self
                .session_manager
                .lock()
                .expect("session lock")
                .append_message(message);
        }
        self.refresh_finalized_context();
    }
}

// ============================================================================
// Support types and helpers
// ============================================================================

/// Upstream `_getRequiredRequestAuth` result.
#[allow(dead_code)]
struct RequiredRequestAuth {
    #[allow(dead_code)]
    model: Model,
    #[allow(dead_code)]
    api_key: Option<String>,
    #[allow(dead_code)]
    headers: Option<std::collections::BTreeMap<String, String>>,
    #[allow(dead_code)]
    env: Option<crate::ai::types::options::ProviderEnv>,
}

/// Upstream `cycleModel` direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CycleDirection {
    Forward,
    Backward,
}

/// Upstream `sendCustomMessage` `deliverAs` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomMessageDelivery {
    Steer,
    FollowUp,
    NextTurn,
}

/// Upstream `sendCustomMessage` options.
#[derive(Debug, Clone, Default)]
pub struct SendCustomMessageOptions {
    pub trigger_turn: Option<bool>,
    pub deliver_as: Option<CustomMessageDelivery>,
}

impl SendCustomMessageOptions {
    fn from_extension(options: &SendMessageOptions) -> Self {
        Self {
            trigger_turn: options.trigger_turn,
            deliver_as: options
                .deliver_as
                .as_deref()
                .map(|deliver_as| match deliver_as {
                    "nextTurn" => CustomMessageDelivery::NextTurn,
                    "followUp" => CustomMessageDelivery::FollowUp,
                    _ => CustomMessageDelivery::Steer,
                }),
        }
    }
}

/// Upstream `sendUserMessage` content union: `string |
/// (TextContent | ImageContent)[]`.
#[derive(Debug, Clone)]
pub enum UserMessageContent {
    Text(String),
    Blocks(Vec<TextOrImageBlock>),
}

/// Upstream `_emitModelSelect` source values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelSelectSource {
    Set,
    Cycle,
    #[allow(dead_code)]
    Restore,
}

impl ModelSelectSource {
    fn as_str(&self) -> &'static str {
        match self {
            ModelSelectSource::Set => "set",
            ModelSelectSource::Cycle => "cycle",
            ModelSelectSource::Restore => "restore",
        }
    }
}

/// `sendUserMessage` content parsed from an extension `Value`.
fn parse_user_message_content(content: &Value) -> UserMessageContent {
    if let Some(text) = content.as_str() {
        return UserMessageContent::Text(text.to_string());
    }
    let blocks =
        serde_json::from_value::<Vec<TextOrImageBlock>>(content.clone()).unwrap_or_default();
    UserMessageContent::Blocks(blocks)
}

/// The unknown-placeholder model stands in for upstream `model === undefined`
/// (the ported agent state seeds it; see `unknown_model`).
fn is_unknown_model(model: &Model) -> bool {
    model.id == "unknown" && model.provider == "unknown"
}

/// `ThinkingLevel` → its upstream wire spelling.
fn thinking_level_str(level: ThinkingLevel) -> &'static str {
    match level {
        ThinkingLevel::Off => "off",
        ThinkingLevel::Minimal => "minimal",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        ThinkingLevel::Xhigh => "xhigh",
        ThinkingLevel::Max => "max",
    }
}

/// Wire spelling → `ThinkingLevel`.
fn parse_thinking_level(level: &str) -> Option<ThinkingLevel> {
    match level {
        "off" => Some(ThinkingLevel::Off),
        "minimal" => Some(ThinkingLevel::Minimal),
        "low" => Some(ThinkingLevel::Low),
        "medium" => Some(ThinkingLevel::Medium),
        "high" => Some(ThinkingLevel::High),
        "xhigh" => Some(ThinkingLevel::Xhigh),
        "max" => Some(ThinkingLevel::Max),
        _ => None,
    }
}

/// Upstream pi-ai `clampThinkingLevel` (models.ts:935-954) over the ported
/// `get_supported_thinking_levels`.
fn clamp_thinking_level(model: &Model, level: ThinkingLevel) -> ThinkingLevel {
    let available_levels = get_supported_thinking_levels(model);
    let level_str = thinking_level_str(level);
    if available_levels.contains(&level_str) {
        return level;
    }

    let extended: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];
    let Some(requested_index) = extended
        .iter()
        .position(|candidate| *candidate == level_str)
    else {
        return available_levels
            .first()
            .and_then(|candidate| parse_thinking_level(candidate))
            .unwrap_or(ThinkingLevel::Off);
    };

    for candidate in &extended[requested_index..] {
        if available_levels.contains(candidate) {
            return parse_thinking_level(candidate).unwrap_or(ThinkingLevel::Off);
        }
    }
    for candidate in extended[..requested_index].iter().rev() {
        if available_levels.contains(candidate) {
            return parse_thinking_level(candidate).unwrap_or(ThinkingLevel::Off);
        }
    }
    available_levels
        .first()
        .and_then(|candidate| parse_thinking_level(candidate))
        .unwrap_or(ThinkingLevel::Off)
}

#[allow(dead_code)] // consumed by `syncQueueModesFromSettings` (W3.12 reload)
fn queue_mode_from_settings(mode: &str) -> QueueMode {
    if mode == "all" {
        QueueMode::All
    } else {
        QueueMode::OneAtATime
    }
}

fn queue_mode_str(mode: QueueMode) -> &'static str {
    match mode {
        QueueMode::All => "all",
        QueueMode::OneAtATime => "one-at-a-time",
    }
}

/// Normalized → upstream input options (the normalize round-trip preserves
/// every field the builders read).
fn to_build_options(options: &NormalizedBuildSystemPromptOptions) -> BuildSystemPromptOptions {
    BuildSystemPromptOptions {
        custom_prompt: options.custom_prompt.clone(),
        force_system_prompt: options.force_system_prompt.clone(),
        selected_tools: Some(options.selected_tools.clone()),
        tool_snippets: Some(options.tool_snippets.clone()),
        tool_guidelines: Some(options.tool_guidelines.clone()),
        prompt_guidelines: Some(options.prompt_guidelines.clone()),
        append_system_prompt: Some(options.append_system_prompt.clone()),
        sections: Some(options.sections.clone()),
        cwd: options.cwd.clone(),
        context_files: Some(options.context_files.clone()),
        skills: Some(options.skills.clone()),
    }
}

/// Serialize a typed [`CustomMessage`] into the transcript's custom-message
/// representation (`AgentMessage::Custom` with role "custom" — the wire shape
/// `core::messages::convert_to_llm` reads back).
pub fn custom_message_to_agent_message(message: CustomMessage) -> AgentMessage {
    let mut data = serde_json::to_value(&message)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    // Untyped extensions can pass null/missing content; normalize at
    // ingestion: `content ?? []`.
    let content_absent = data.get("content").map(Value::is_null).unwrap_or(true);
    if content_absent {
        data.insert("content".to_string(), Value::Array(Vec::new()));
    }
    AgentMessage::Custom(CustomAgentMessage {
        role: "custom".to_string(),
        data,
    })
}

/// Recover a typed [`CustomMessage`] from the transcript representation.
pub fn custom_message_from_agent_message(message: &AgentMessage) -> Option<CustomMessage> {
    match message {
        AgentMessage::Custom(custom) if custom.role == "custom" => {
            serde_json::from_value(Value::Object(custom.data.clone())).ok()
        }
        _ => None,
    }
}

/// Build a transcript custom message from an extension `before_agent_start`
/// message payload (untyped extensions can pass null/missing content;
/// normalized at ingestion). The custom role is the message's `customType`
/// when it doubles as a custom role, else "custom".
fn custom_agent_message_from_value(message: Value) -> AgentMessage {
    let mut object = message.as_object().cloned().unwrap_or_default();
    let role = object
        .get("customType")
        .and_then(Value::as_str)
        .unwrap_or("custom")
        .to_string();
    let content_absent = object.get("content").map(Value::is_null).unwrap_or(true);
    if content_absent {
        object.insert("content".to_string(), Value::Array(Vec::new()));
    }
    AgentMessage::Custom(CustomAgentMessage { role, data: object })
}

/// `BashExecutionMessage` transcript representation (role "bashExecution").
fn bash_execution_agent_message(message: &BashExecutionMessage) -> CustomAgentMessage {
    let data = serde_json::to_value(message)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    CustomAgentMessage {
        role: "bashExecution".to_string(),
        data,
    }
}

/// Normalize an extension's `message_end` replacement: null/missing content on
/// the four standard roles becomes `[]` (upstream `{ ...replacement,
/// content: [] }`).
fn normalize_replacement_message(mut replacement: Value) -> Value {
    let role = replacement.get("role").and_then(Value::as_str);
    let needs_content = matches!(role, Some("user" | "assistant" | "toolResult" | "custom"))
        && replacement
            .get("content")
            .map(Value::is_null)
            .unwrap_or(true);
    if needs_content {
        if let Some(object) = replacement.as_object_mut() {
            object.insert("content".to_string(), Value::Array(Vec::new()));
        }
    }
    replacement
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else {
        "unknown error".to_string()
    }
}

fn user_message_with_text_and_images(
    text: &str,
    images: Option<Vec<ImageContent>>,
) -> AgentMessage {
    let mut content: Vec<TextOrImageBlock> = vec![TextOrImageBlock::Text(TextContent {
        text: text.to_string(),
        text_signature: None,
    })];
    if let Some(images) = images {
        content.extend(images.into_iter().map(TextOrImageBlock::Image));
    }
    AgentMessage::User(UserMessage {
        content: StringOrBlocks::Blocks(content),
        timestamp: now_ms(),
    })
}

fn parse_image_content(images: &[Value]) -> Option<Vec<ImageContent>> {
    images
        .iter()
        .map(|image| serde_json::from_value(image.clone()))
        .collect::<Result<Vec<ImageContent>, _>>()
        .ok()
}

/// Subscribe the session's `_handleAgentEvent` to the agent.
fn subscribe_agent_handler(session: &Arc<AgentSession>) -> crate::agent_core::agent::Unsubscribe {
    let weak = Arc::downgrade(session);
    session
        .agent
        .subscribe(move |event: AgentEvent, _token: CancellationToken| {
            let weak = weak.clone();
            Box::pin(async move {
                if let Some(session) = weak.upgrade() {
                    session.handle_agent_event(event).await;
                }
            }) as BoxFuture<'static, ()>
        })
}

impl From<&AgentEvent> for AgentSessionEvent {
    fn from(event: &AgentEvent) -> Self {
        Self::from_agent_event(event, false)
    }
}

/// The `before_agent_start` renderer over the vendored `system-prompt.ts`
/// port (upstream `buildSystemPrompt`).
pub struct SessionSystemPromptRenderer;

impl crate::coding_agent::extensions::types::NormalizedSystemPromptRenderer
    for SessionSystemPromptRenderer
{
    fn build(&self, normalized: &NormalizedBuildSystemPromptOptions) -> String {
        if let Some(force) = &normalized.force_system_prompt {
            return force.clone();
        }
        system_prompt::build_system_prompt(&to_build_options(normalized)).unwrap_or_default()
    }
}

// ============================================================================
// Lower half (upstream agent-session.ts:1989-3625)
// ============================================================================

impl AgentSession {
    // =========================================================================
    // Compaction (agent-session.ts:1989-2539)
    // =========================================================================

    /// Upstream `compact`: manually compact the session context. This is the
    /// manual entry point used by `/compact`, RPC, and extensions. Aborts the
    /// current agent operation first; never retries or continues the
    /// interrupted agent turn. Returns the `CompactionResult` JSON.
    pub async fn compact(
        &self,
        custom_instructions: Option<String>,
    ) -> Result<Value, AgentSessionError> {
        self.abort().await;
        let token = CancellationToken::new();
        *self.compaction_abort.lock().expect("compaction lock") = Some(token.clone());
        self.emit(AgentSessionEvent::CompactionStart {
            reason: CompactionReason::Manual,
        });
        let mut from_extension = false;

        let run: Result<Value, AgentSessionError> = async {
            let model = match self.model() {
                Some(model) => model,
                None => {
                    return Err(AgentSessionError::Upstream(
                        format_no_model_selected_message(),
                    ));
                }
            };

            let settings = self.compaction_settings_for(Some(&model))?;
            let auth = self.get_summarization_request_auth(&model).await?;

            let (branch_entries_json, path_entries) = {
                let manager = self.session_manager.lock().expect("session lock");
                let branch_entries_json =
                    serde_json::to_value(manager.get_branch(None)).unwrap_or(Value::Null);
                (branch_entries_json, compaction_path_entries(&manager))
            };

            let preparation = match prepare_compaction(&path_entries, settings) {
                Some(preparation) => preparation,
                None => {
                    // Check why we can't compact
                    let last_is_compaction = path_entries.last().is_some_and(|entry| {
                        matches!(entry, CompactionSessionEntry::Compaction { .. })
                    });
                    if last_is_compaction {
                        return Err(AgentSessionError::Upstream("Already compacted".to_string()));
                    }
                    return Err(AgentSessionError::Upstream(
                        "Nothing to compact (session too small)".to_string(),
                    ));
                }
            };

            let mut extension_compaction: Option<ExtensionCompaction> = None;
            let runner = self.extension_runner();
            if runner.has_handlers("session_before_compact") {
                let mut event = json!({
                    "type": "session_before_compact",
                    "preparation": preparation_json(&preparation),
                    "branchEntries": branch_entries_json,
                    "customInstructions": custom_instructions,
                    "reason": "manual",
                    "willRetry": false,
                });
                let result = runner.emit(&mut event).await;
                if let Some(result) = result {
                    if result
                        .get("cancel")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        return Err(AgentSessionError::Upstream(
                            "Compaction cancelled".to_string(),
                        ));
                    }
                    if let Some(compaction) = result.get("compaction").filter(|c| !c.is_null()) {
                        extension_compaction = Some(
                            serde_json::from_value::<ExtensionCompaction>(compaction.clone())
                                .map_err(|error| AgentSessionError::Upstream(error.to_string()))?,
                        );
                        from_extension = true;
                    }
                }
            }

            let (summary, first_kept_entry_id, tokens_before, usage, details) =
                match extension_compaction {
                    // Extension provided compaction content
                    Some(extension) => extension.into_fields(),
                    None => {
                        // Shared default summary generator, also used by
                        // automatic compaction.
                        let result = self
                            .run_default_compaction(
                                preparation,
                                &auth.model,
                                auth.api_key.as_deref(),
                                auth.headers.as_ref(),
                                auth.env.as_ref(),
                                custom_instructions.as_deref(),
                                &token,
                                CompactionReason::Manual,
                            )
                            .await?;
                        let details = result
                            .details
                            .as_ref()
                            .map(|details| serde_json::to_value(details).unwrap_or(Value::Null));
                        (
                            result.summary,
                            result.first_kept_entry_id,
                            result.tokens_before,
                            result.usage,
                            details,
                        )
                    }
                };

            if token.is_cancelled() {
                return Err(AgentSessionError::Upstream(
                    "Compaction cancelled".to_string(),
                ));
            }

            self.append_compaction_and_rebuild(
                &summary,
                &first_kept_entry_id,
                tokens_before,
                details.clone(),
                from_extension,
                usage,
            )?;
            let estimated_tokens_after = estimate_messages_tokens(&self.agent.state().messages);
            // Get the saved compaction entry for the extension event
            let saved_compaction_entry = self
                .session_manager
                .lock()
                .expect("session lock")
                .get_entries()
                .into_iter()
                .rev()
                .find(|entry| {
                    matches!(entry, SessionEntry::Compaction(compaction)
                        if compaction.summary == summary)
                });
            if let Some(entry) = saved_compaction_entry {
                let mut event = json!({
                    "type": "session_compact",
                    "compactionEntry": serde_json::to_value(&entry).unwrap_or(Value::Null),
                    "fromExtension": from_extension,
                    "reason": "manual",
                    "willRetry": false,
                });
                runner.emit(&mut event).await;
            }

            let compaction_result = json!({
                "summary": summary,
                "firstKeptEntryId": first_kept_entry_id,
                "tokensBefore": tokens_before,
                "estimatedTokensAfter": estimated_tokens_after,
                "usage": usage
                    .map(|usage| serde_json::to_value(usage).unwrap_or(Value::Null))
                    .unwrap_or(Value::Null),
                "details": details,
            });
            // compaction_end listeners may submit queued prompts, so expose
            // idle state before notifying them.
            self.clear_manual_compaction_state();
            self.emit(AgentSessionEvent::CompactionEnd {
                reason: CompactionReason::Manual,
                result: Some(compaction_result.clone()),
                aborted: false,
                will_retry: false,
                error_message: None,
            });
            Ok(compaction_result)
        }
        .await;

        match run {
            Ok(result) => {
                // finally {
                self.clear_manual_compaction_state();
                // }
                Ok(result)
            }
            Err(error) => {
                let message = error.to_string();
                let aborted = message == "Compaction cancelled";
                let error_message = if aborted {
                    None
                } else {
                    Some(format!("Compaction failed: {message}"))
                };
                self.clear_manual_compaction_state();
                self.emit(AgentSessionEvent::CompactionEnd {
                    reason: CompactionReason::Manual,
                    result: None,
                    aborted,
                    will_retry: false,
                    error_message: error_message.clone(),
                });
                self.emit_session_compact_failed(json!({
                    "reason": "manual",
                    "errorMessage": error_message,
                    "aborted": aborted,
                    "willRetry": false,
                    "fromExtension": from_extension,
                }))
                .await;
                // finally {
                self.clear_manual_compaction_state();
                // }
                Err(error)
            }
        }
    }

    /// Upstream `setAutoCompactionEnabled`: toggle auto-compaction setting.
    pub fn set_auto_compaction_enabled(&self, enabled: bool) -> Result<(), AgentSessionError> {
        self.settings_manager.set_compaction_enabled(enabled);
        Ok(())
    }

    /// Upstream `get autoCompactionEnabled`: whether auto-compaction is
    /// enabled.
    pub fn auto_compaction_enabled(&self) -> Result<bool, AgentSessionError> {
        Ok(self.settings_manager.get_compaction_enabled())
    }

    // =========================================================================
    // Extension lifecycle (agent-session.ts:2541-2938)
    // =========================================================================

    /// Upstream `bindExtensions`: install the mode-level bindings and start
    /// the extension session (`session_start` + resource discovery).
    pub async fn bind_extensions(
        &self,
        bindings: ExtensionBindings,
    ) -> Result<(), AgentSessionError> {
        if let Some(ui_context) = bindings.ui_context {
            *self.extension_ui_context.lock().expect("ui lock") = Some(ui_context);
        }
        if let Some(mode) = bindings.mode {
            *self.extension_mode.lock().expect("mode lock") = mode;
        }
        if let Some(command_context_actions) = bindings.command_context_actions {
            *self
                .extension_command_context_actions
                .lock()
                .expect("command actions lock") = Some(command_context_actions);
        }
        if let Some(abort_handler) = bindings.abort_handler {
            *self.extension_abort_handler.lock().expect("abort lock") = Some(abort_handler);
        }
        if let Some(shutdown_handler) = bindings.shutdown_handler {
            *self
                .extension_shutdown_handler
                .lock()
                .expect("shutdown lock") = Some(shutdown_handler);
        }
        if let Some(on_error) = bindings.on_error {
            *self.extension_error_listener.lock().expect("listener lock") = Some(on_error);
        }

        let runner = self.extension_runner();
        self.apply_extension_bindings(&runner);
        let mut session_start = self.session_start_event.clone();
        runner.emit(&mut session_start).await;
        runner.report_unhandled_mcp_servers();
        let reason = if session_start
            .get("reason")
            .and_then(Value::as_str)
            .is_some_and(|reason| reason == "reload")
        {
            ResourcesDiscoverReason::Reload
        } else {
            ResourcesDiscoverReason::Startup
        };
        self.extend_resources_from_extensions(reason).await;
        Ok(())
    }

    /// Upstream `extendResourcesFromExtensions`.
    async fn extend_resources_from_extensions(&self, reason: ResourcesDiscoverReason) {
        let runner = self.extension_runner();
        if !runner.has_handlers("resources_discover") {
            return;
        }

        let discovered = runner.emit_resources_discover(&self.cwd, reason).await;

        if discovered.skill_paths.is_empty()
            && discovered.prompt_paths.is_empty()
            && discovered.theme_paths.is_empty()
        {
            return;
        }

        let extension_paths = ResourceExtensionPaths {
            skill_paths: self.build_extension_resource_paths(&discovered.skill_paths),
            prompt_paths: self.build_extension_resource_paths(&discovered.prompt_paths),
            theme_paths: self.build_extension_resource_paths(&discovered.theme_paths),
        };

        self.resource_loader
            .lock()
            .expect("resource loader lock")
            .extend_resources(extension_paths);
        let active_tool_names = self.get_active_tool_names();
        self.rebuild_system_prompt(active_tool_names);
    }

    /// Upstream `buildExtensionResourcePaths`.
    fn build_extension_resource_paths(
        &self,
        entries: &[(String, String)],
    ) -> Vec<ResourcePathEntry> {
        entries
            .iter()
            .map(|(path, extension_path)| {
                let source = self.get_extension_source_label(extension_path);
                let base_dir = if extension_path.starts_with('<') {
                    None
                } else {
                    Some(node_dirname(extension_path))
                };
                ResourcePathEntry {
                    path: path.clone(),
                    metadata: PathMetadata {
                        source,
                        scope: SourceScope::Temporary,
                        origin: PathMetadataOrigin::TopLevel,
                        base_dir,
                    },
                }
            })
            .collect()
    }

    /// Upstream `getExtensionSourceLabel`.
    fn get_extension_source_label(&self, extension_path: &str) -> String {
        if extension_path.starts_with('<') {
            let stripped: String = extension_path
                .chars()
                .filter(|character| *character != '<' && *character != '>')
                .collect();
            return format!("extension:{stripped}");
        }
        let base = extension_path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(extension_path);
        let name = base
            .strip_suffix(".ts")
            .or_else(|| base.strip_suffix(".js"))
            .unwrap_or(base);
        format!("extension:{name}")
    }

    /// Upstream `reload`: reload extensions, settings, and resources, then
    /// rebuild the runtime. Emits `session_shutdown` on the old runner and,
    /// when mode bindings exist, `session_start` with `reason: "reload"` on
    /// the new one.
    pub async fn reload(&self, options: Option<ReloadOptions>) -> Result<(), AgentSessionError> {
        let old_runner = self.extension_runner();
        let previous_flag_values = old_runner.get_flag_values();
        emit_session_shutdown_event(
            &old_runner,
            &json!({"type": "session_shutdown", "reason": "reload"}),
        )
        .await;
        old_runner.invalidate(None);
        // v1.0.0: activate tools newly added to `defaultTools` (when the
        // initial tools came from it). Removed ones stay active, and tools
        // disabled during the session stay disabled unless the setting newly
        // adds them.
        const DEFAULT_TOOL_NAMES: [&str; 4] = ["read", "bash", "edit", "write"];
        let default_tool_names = || -> Vec<String> {
            self.settings_manager
                .get_default_tools()
                .unwrap_or_else(|| {
                    DEFAULT_TOOL_NAMES
                        .iter()
                        .map(|name| name.to_string())
                        .collect()
                })
        };
        let previous_default_tools: HashSet<String> = if self.uses_default_tools {
            default_tool_names().into_iter().collect()
        } else {
            HashSet::new()
        };
        self.settings_manager.reload();
        self.sync_queue_modes_from_settings();
        // `resetApiProviders` seam: pi-ai keeps a module-global provider
        // registry upstream; the ported ai layer has none (per-agent Models
        // collections), so there is nothing to reset.
        self.resource_loader
            .lock()
            .expect("resource loader lock")
            .reload_without_trust()
            .map_err(AgentSessionError::Upstream)?;
        let added_default_tools: Vec<String> = if self.uses_default_tools {
            default_tool_names()
                .into_iter()
                .filter(|name| !previous_default_tools.contains(name))
                .collect()
        } else {
            Vec::new()
        };
        // Tools the new extensions register later, such as MCP tools, are
        // pending until then.
        {
            let mut pending = self.pending_tool_names.lock().expect("pending tools lock");
            for name in self.get_active_tool_names() {
                pending.insert(name);
            }
        }
        let mut active_tool_names = self.get_active_tool_names();
        active_tool_names.extend(added_default_tools);
        self.build_runtime(BuildRuntimeOptions {
            active_tool_names: Some(active_tool_names),
            flag_values: Some(previous_flag_values),
            include_all_extension_tools: true,
        });

        let has_bindings = self.extension_ui_context.lock().expect("ui lock").is_some()
            || self
                .extension_command_context_actions
                .lock()
                .expect("command actions lock")
                .is_some()
            || self
                .extension_shutdown_handler
                .lock()
                .expect("shutdown lock")
                .is_some()
            || self
                .extension_error_listener
                .lock()
                .expect("listener lock")
                .is_some();
        if has_bindings {
            if let Some(before_session_start) = options
                .as_ref()
                .and_then(|options| options.before_session_start.as_ref())
            {
                before_session_start();
            }
            let runner = self.extension_runner();
            let mut event = json!({"type": "session_start", "reason": "reload"});
            runner.emit(&mut event).await;
            runner.report_unhandled_mcp_servers();
            self.extend_resources_from_extensions(ResourcesDiscoverReason::Reload)
                .await;
        }
        Ok(())
    }

    // =========================================================================
    // Auto-Retry observables (agent-session.ts:3044-3063)
    // =========================================================================

    /// Upstream `get isRetrying`: whether auto-retry is currently in progress.
    pub fn is_retrying(&self) -> bool {
        self.retry_abort.lock().expect("retry lock").is_some()
    }

    /// Upstream `get autoRetryEnabled`: whether auto-retry is enabled.
    pub fn auto_retry_enabled(&self) -> bool {
        self.settings_manager.get_retry_enabled()
    }

    /// Upstream `setAutoRetryEnabled`: toggle auto-retry setting.
    pub fn set_auto_retry_enabled(&self, enabled: bool) {
        self.settings_manager.set_retry_enabled(enabled);
    }

    // =========================================================================
    // Bash Execution (agent-session.ts:3065-3177)
    // =========================================================================

    /// Upstream `executeBash`: execute a bash command, add the result to agent
    /// context and session, and stream `bash_execution_update` events.
    pub async fn execute_bash(
        &self,
        command: &str,
        on_chunk: Option<OnBashChunk>,
        options: Option<ExecuteBashOptions>,
    ) -> Result<crate::coding_agent::extensions::types::BashResult, AgentSessionError> {
        let options = options.unwrap_or_default();
        let token = Arc::new(CancellationToken::new());
        self.bash_abort_controllers
            .lock()
            .expect("bash lock")
            .push(Arc::clone(&token));

        // Apply command prefix if configured (e.g., "shopt -s expand_aliases"
        // for alias support)
        let prefix = self.settings_manager.get_shell_command_prefix();
        let shell_path = self.settings_manager.get_shell_path();
        let resolved_command = match prefix {
            Some(prefix) if !prefix.is_empty() => format!("{prefix}\n{command}"),
            _ => command.to_string(),
        };

        let cwd = self
            .session_manager
            .lock()
            .expect("session lock")
            .get_cwd()
            .to_string();
        let operations = options
            .operations
            .clone()
            .unwrap_or_else(|| bash_executor::create_local_bash_operations(shell_path.as_deref()));
        let emit_update = {
            let session = self.self_weak();
            let id = options.id.clone();
            let on_chunk = on_chunk.clone();
            Arc::new(move |delta: &str| {
                if let Some(on_chunk) = &on_chunk {
                    on_chunk(delta);
                }
                if let Some(session) = session.upgrade() {
                    session.emit(AgentSessionEvent::BashExecutionUpdate {
                        id: id.clone(),
                        delta: delta.to_string(),
                    });
                }
            })
        };

        let outcome = bash_executor::execute_bash_with_operations(
            &resolved_command,
            &cwd,
            operations,
            bash_executor::BashExecutorOptions {
                on_chunk: Some(emit_update),
                signal: Some((*token).clone()),
            },
        )
        .await;

        self.bash_abort_controllers
            .lock()
            .expect("bash lock")
            .retain(|controller| !Arc::ptr_eq(controller, &token));

        let result = outcome.map_err(AgentSessionError::Upstream)?;
        self.record_bash_result(command, &result, options.exclude_from_context);
        Ok(result)
    }

    /// Upstream `recordBashResult`: record a bash execution result in session
    /// history. Used by executeBash and by extensions that handle bash
    /// execution themselves.
    pub fn record_bash_result(
        &self,
        command: &str,
        result: &crate::coding_agent::extensions::types::BashResult,
        exclude_from_context: Option<bool>,
    ) {
        let bash_message = BashExecutionMessage {
            command: command.to_string(),
            output: result.output.clone(),
            exit_code: result.exit_code,
            cancelled: result.cancelled,
            truncated: result.truncated,
            full_output_path: result.full_output_path.clone(),
            timestamp: now_ms(),
            exclude_from_context,
        };

        // If agent is streaming, defer adding to avoid breaking
        // tool_use/tool_result ordering
        if self.is_streaming() {
            // Queue for later - will be flushed on agent_end
            self.pending_bash_messages
                .lock()
                .expect("bash pending lock")
                .push(bash_message);
        } else {
            // Add to agent state immediately
            let message = AgentMessage::Custom(bash_execution_agent_message(&bash_message));
            self.agent.state().messages.push(message.clone());

            // Save to session
            let _ = self
                .session_manager
                .lock()
                .expect("session lock")
                .append_message(message);
        }
    }

    // =========================================================================
    // Tree Navigation (agent-session.ts:3193-3425)
    // =========================================================================

    /// Upstream `navigateTree`: navigate to a different node in the session
    /// tree. Unlike fork() which creates a new session file, this stays in the
    /// same file.
    pub async fn navigate_tree(
        &self,
        target_id: &str,
        options: NavigateTreeOptions,
    ) -> Result<NavigateTreeResult, AgentSessionError> {
        if self.is_streaming() {
            return Err(AgentSessionError::Upstream(
                "Wait for the current response to finish before navigating the session tree."
                    .to_string(),
            ));
        }
        if self.is_compacting() {
            return Err(AgentSessionError::Upstream(
                "Wait for the current compaction or tree navigation to finish before navigating \
                 the session tree."
                    .to_string(),
            ));
        }

        let old_leaf_id = self
            .session_manager
            .lock()
            .expect("session lock")
            .get_leaf_id()
            .map(str::to_string);

        // No-op if already at target
        if old_leaf_id.as_deref() == Some(target_id) {
            return Ok(NavigateTreeResult {
                editor_text: None,
                cancelled: false,
                aborted: None,
                summary_entry: None,
            });
        }

        // Model required for summarization
        let summarize = options.summarize.unwrap_or(false);
        if summarize && self.model().is_none() {
            return Err(AgentSessionError::Upstream(
                "No model available for summarization".to_string(),
            ));
        }

        let target_entry = self
            .session_manager
            .lock()
            .expect("session lock")
            .get_entry(target_id)
            .cloned();
        let Some(target_entry) = target_entry else {
            return Err(AgentSessionError::Upstream(format!(
                "Entry {target_id} not found"
            )));
        };

        // Collect entries to summarize (from old leaf to common ancestor)
        let CollectEntriesResult {
            entries: entries_to_summarize,
            common_ancestor_id,
        } = {
            let manager = self.session_manager.lock().expect("session lock");
            let adapter = BranchSummarySession(&manager);
            collect_entries_for_branch_summary(&adapter, old_leaf_id.as_deref(), target_id)
        };

        // Prepare event data - mutable so extensions can override
        let mut custom_instructions = options.custom_instructions.clone();
        let mut replace_instructions = options.replace_instructions;
        let mut label = options.label.clone();

        let preparation = json!({
            "targetId": target_id,
            "oldLeafId": old_leaf_id,
            "commonAncestorId": common_ancestor_id,
            "entriesToSummarize": entries_to_summarize
                .iter()
                .map(compaction_entry_json)
                .collect::<Vec<Value>>(),
            "userWantsSummary": summarize,
            "customInstructions": custom_instructions,
            "replaceInstructions": replace_instructions,
            "label": label,
        });

        // Set up abort controller for summarization
        let token = CancellationToken::new();
        *self.branch_summary_abort.lock().expect("branch lock") = Some(token.clone());

        let run: Result<NavigateTreeResult, AgentSessionError> = async {
            let runner = self.extension_runner();
            let mut extension_summary: Option<ExtensionBranchSummary> = None;
            let mut from_extension = false;

            // Emit session_before_tree event
            if runner.has_handlers("session_before_tree") {
                let mut event = json!({
                    "type": "session_before_tree",
                    "preparation": preparation,
                });
                let result = runner.emit(&mut event).await;
                if let Some(result) = result {
                    if result
                        .get("cancel")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        return Ok(NavigateTreeResult {
                            editor_text: None,
                            cancelled: true,
                            aborted: None,
                            summary_entry: None,
                        });
                    }

                    let summary = result.get("summary").filter(|summary| !summary.is_null());
                    if let Some(summary) = summary {
                        if summarize {
                            extension_summary =
                                Some(
                                    serde_json::from_value::<ExtensionBranchSummary>(
                                        summary.clone(),
                                    )
                                    .map_err(|error| {
                                        AgentSessionError::Upstream(error.to_string())
                                    })?,
                                );
                            from_extension = true;
                        }
                    }

                    // Allow extensions to override instructions and label
                    if let Some(overridden) =
                        result.get("customInstructions").and_then(Value::as_str)
                    {
                        custom_instructions = Some(overridden.to_string());
                    }
                    if let Some(overridden) =
                        result.get("replaceInstructions").and_then(Value::as_bool)
                    {
                        replace_instructions = Some(overridden);
                    }
                    if let Some(overridden) = result.get("label").and_then(Value::as_str) {
                        label = Some(overridden.to_string());
                    }
                }
            }

            // Run default summarizer if needed
            let mut summary_text: Option<String> = None;
            let mut summary_details: Option<Value> = None;
            let mut summary_usage: Option<Usage> = None;
            if summarize && !entries_to_summarize.is_empty() && extension_summary.is_none() {
                let model = self
                    .model()
                    .expect("model presence checked with the summarize flag");
                let auth = self.get_summarization_request_auth(&model).await?;
                let BranchSummarySettings { reserve_tokens, .. } =
                    self.settings_manager.get_branch_summary_settings();
                let retry_settings = self.retry_settings();
                let mut callbacks =
                    self.summarization_retry_callbacks(SummarizationRetrySource::BranchSummary);
                let flattened_headers = auth.headers.as_ref().map(flatten_headers);
                let result = generate_branch_summary(
                    &entries_to_summarize,
                    GenerateBranchSummaryOptions {
                        model: &auth.model,
                        api_key: auth.api_key.as_deref(),
                        headers: flattened_headers.as_ref(),
                        env: auth.env.as_ref(),
                        signal: Some(&token),
                        custom_instructions: custom_instructions.as_deref(),
                        replace_instructions: replace_instructions.unwrap_or(false),
                        reserve_tokens: Some(reserve_tokens.max(0) as u64),
                        stream_fn: Some(crate::coding_agent::core::compaction::models_stream_fn(
                            Arc::clone(&self.agent.models),
                        )),
                        retry: Some(&retry_policy_from_settings(&retry_settings)),
                        callbacks: &mut callbacks,
                    },
                )
                .await;
                if result.aborted.unwrap_or(false) {
                    return Ok(NavigateTreeResult {
                        editor_text: None,
                        cancelled: true,
                        aborted: Some(true),
                        summary_entry: None,
                    });
                }
                if let Some(error) = result.error {
                    return Err(AgentSessionError::Upstream(error));
                }
                summary_text = result.summary;
                summary_usage = result.usage;
                summary_details = Some(json!({
                    "readFiles": result.read_files.clone().unwrap_or_default(),
                    "modifiedFiles": result.modified_files.clone().unwrap_or_default(),
                }));
            } else if let Some(extension) = extension_summary {
                summary_text = Some(extension.summary);
                summary_details = extension.details;
                summary_usage = extension.usage;
            }

            // Determine the new leaf position based on target type
            let (new_leaf_id, editor_text) = navigate_target_decision(&target_entry, target_id);

            // Switch leaf (with or without summary)
            // Summary is attached at the navigation target position
            // (newLeafId), not the old branch
            let mut summary_entry: Option<SessionEntry> = None;
            if let Some(summary_text) = summary_text.as_ref() {
                // Create summary at target position (can be null for root)
                let summary_id = {
                    let mut manager = self.session_manager.lock().expect("session lock");
                    let summary_id = manager
                        .branch_with_summary(
                            new_leaf_id.as_deref(),
                            summary_text,
                            summary_details.clone(),
                            Some(from_extension),
                            summary_usage,
                        )
                        .map_err(session_manager_error)?;
                    if let Some(label) = label.as_ref() {
                        manager
                            .append_label_change(&summary_id, Some(label))
                            .map_err(session_manager_error)?;
                    }
                    summary_id
                };
                summary_entry = self
                    .session_manager
                    .lock()
                    .expect("session lock")
                    .get_entry(&summary_id)
                    .cloned();
            } else if new_leaf_id.is_none() {
                // No summary, navigating to root - reset leaf
                self.session_manager
                    .lock()
                    .expect("session lock")
                    .reset_leaf();
            } else {
                // No summary, navigating to non-root
                let mut manager = self.session_manager.lock().expect("session lock");
                manager
                    .branch(new_leaf_id.as_deref().expect("checked above"))
                    .map_err(session_manager_error)?;
            }

            // Attach label to target entry when not summarizing (no summary
            // entry to label)
            if let (Some(label), None) = (&label, &summary_text) {
                self.session_manager
                    .lock()
                    .expect("session lock")
                    .append_label_change(target_id, Some(label))
                    .map_err(session_manager_error)?;
            }

            // Update finalized context from the canonical session projection.
            self.refresh_finalized_context();
            self.restore_tools_from_transcript();

            // Emit session_tree event
            let mut event = json!({
                "type": "session_tree",
                "newLeafId": self
                    .session_manager
                    .lock()
                    .expect("session lock")
                    .get_leaf_id(),
                "oldLeafId": old_leaf_id,
                "summaryEntry": summary_entry
                    .as_ref()
                    .map(|entry| serde_json::to_value(entry).unwrap_or(Value::Null))
                    .unwrap_or(Value::Null),
                "fromExtension": if summary_text.is_some() {
                    Value::Bool(from_extension)
                } else {
                    Value::Null
                },
            });
            runner.emit(&mut event).await;

            Ok(NavigateTreeResult {
                editor_text,
                cancelled: false,
                aborted: None,
                summary_entry,
            })
        }
        .await;

        // finally {
        *self.branch_summary_abort.lock().expect("branch lock") = None;
        self.resolve_idle_wait_if_idle();
        // }
        run
    }

    /// Upstream `getUserMessagesForForking`: all user messages from session
    /// for the fork selector, as `(entryId, text)` pairs.
    pub fn get_user_messages_for_forking(
        &self,
    ) -> Result<Vec<(String, String)>, AgentSessionError> {
        let entries = self
            .session_manager
            .lock()
            .expect("session lock")
            .get_entries();
        let mut result: Vec<(String, String)> = Vec::new();

        for entry in entries {
            let SessionEntry::Message(message) = entry else {
                continue;
            };
            let AgentMessage::User(user) = &message.message else {
                continue;
            };

            let text = content_text_with_separator(&user.content, "");
            if !text.is_empty() {
                result.push((message.id, text));
            }
        }

        Ok(result)
    }

    // =========================================================================
    // Statistics (agent-session.ts:3427-3528)
    // =========================================================================

    /// Upstream `getSessionStats`: session statistics aggregated over ALL
    /// session entries (including history that was compacted away), so
    /// token/cost totals reflect what was actually billed across the session.
    pub fn get_session_stats(&self) -> Result<SessionStats, AgentSessionError> {
        let mut stats = SessionStats {
            session_file: self.session_file(),
            session_id: self.session_id(),
            ..SessionStats::default()
        };
        let mut usage_totals = UsageTotals::default();
        let entries = self
            .session_manager
            .lock()
            .expect("session lock")
            .get_entries();

        for entry in &entries {
            match entry {
                SessionEntry::Usage(usage_entry) => {
                    add_usage_to_totals(&mut usage_totals, &usage_entry.usage);
                }
                SessionEntry::BranchSummary(summary) => {
                    if let Some(usage) = &summary.usage {
                        add_usage_to_totals(&mut usage_totals, usage);
                    }
                }
                SessionEntry::Compaction(compaction) => {
                    if let Some(usage) = &compaction.usage {
                        add_usage_to_totals(&mut usage_totals, usage);
                    }
                }
                _ => {}
            }
            let SessionEntry::Message(message) = entry else {
                continue;
            };
            stats.total_messages += 1;
            match &message.message {
                AgentMessage::User(_) => {
                    stats.user_messages += 1;
                }
                AgentMessage::ToolResult(tool_result) => {
                    stats.tool_results += 1;
                    if let Some(usage) = &tool_result.usage {
                        add_usage_to_totals(&mut usage_totals, usage);
                    }
                }
                AgentMessage::Assistant(assistant) => {
                    stats.assistant_messages += 1;
                    stats.tool_calls += assistant
                        .content
                        .iter()
                        .filter(|block| matches!(block, AssistantBlock::ToolCall(_)))
                        .count() as u64;
                    add_usage_to_totals(&mut usage_totals, &assistant.usage);
                }
                _ => {}
            }
        }

        stats.tokens = SessionTokens {
            input: usage_totals.input,
            output: usage_totals.output,
            cache_read: usage_totals.cache_read,
            cache_write: usage_totals.cache_write,
            total: usage_totals.input
                + usage_totals.output
                + usage_totals.cache_read
                + usage_totals.cache_write,
        };
        stats.cost = usage_totals.cost;
        stats.context_usage = self.get_context_usage()?;
        Ok(stats)
    }

    /// Upstream `getContextUsage`.
    pub fn get_context_usage(&self) -> Result<Option<ContextUsage>, AgentSessionError> {
        // Under a virtual selection the routed physical model supplies the
        // limits (upstream `_limitsModel()`).
        let model = self.limits_model();
        let Some(model) = model else {
            return Ok(None);
        };

        let context_window = model.context_window;
        if context_window == 0 {
            return Ok(None);
        }

        // After compaction, the last assistant usage reflects pre-compaction
        // context size. We can only trust usage from an assistant that
        // responded after the latest compaction. If no such assistant exists,
        // context token count is unknown until the next LLM response.
        let branch_entries = self
            .session_manager
            .lock()
            .expect("session lock")
            .get_branch(None);
        let latest_compaction = get_latest_compaction_entry(&branch_entries);

        if let Some(latest_compaction) = latest_compaction {
            // Check if there's a valid assistant usage after the compaction
            // boundary
            let compaction_index = branch_entries
                .iter()
                .rposition(|entry| entry == latest_compaction);
            let mut has_post_compaction_usage = false;
            if let Some(compaction_index) = compaction_index {
                for entry in branch_entries[compaction_index + 1..].iter().rev() {
                    let SessionEntry::Message(message) = entry else {
                        continue;
                    };
                    let AgentMessage::Assistant(assistant) = &message.message else {
                        continue;
                    };
                    if !matches!(
                        assistant.stop_reason,
                        StopReason::Aborted | StopReason::Error
                    ) {
                        let context_tokens = calculate_context_tokens(assistant.usage);
                        if context_tokens > 0 {
                            has_post_compaction_usage = true;
                            break;
                        }
                    }
                }
            }

            if !has_post_compaction_usage {
                return Ok(Some(ContextUsage {
                    tokens: None,
                    context_window: context_window as i64,
                    percent: None,
                }));
            }
        }

        let estimate = estimate_context_tokens(&self.messages());
        let percent = (estimate.tokens as f64 / context_window as f64) * 100.0;

        Ok(Some(ContextUsage {
            tokens: Some(estimate.tokens as i64),
            context_window: context_window as i64,
            percent: Some(percent),
        }))
    }

    // =========================================================================
    // Exports (agent-session.ts:3530-3563)
    // =========================================================================

    /// Upstream `exportToHtml`: export session to HTML. See the export-html
    /// seam in the module docs: the session keeps the exact guard errors, data
    /// collection, and output-path defaulting; generation is delegated to the
    /// injected [`HtmlExportFn`].
    pub async fn export_to_html(
        &self,
        output_path: Option<String>,
        options: Option<ExportToHtmlOptions>,
    ) -> Result<String, AgentSessionError> {
        let options = options.unwrap_or_default();
        // Upstream: `[options.themeName, settings.getTheme()].find(candidate =>
        // candidate !== undefined && getThemeByName(candidate) !== undefined)`.
        // The theme registry (`getThemeByName`) is the themes slice; with no
        // registry every candidate fails validation, so the theme name stays
        // unset.
        let requested_theme_name = options.theme_name;
        let _ = requested_theme_name;
        let _ = self.settings_manager.get_theme();
        let theme_name: Option<String> = None;

        let exporter = self
            .html_exporter
            .lock()
            .expect("html exporter lock")
            .clone()
            .ok_or_else(|| {
                AgentSessionError::Upstream(
                    "HTML export is not available: no exporter bound (export-html slice not \
                     ported)"
                        .to_string(),
                )
            })?;

        let session_file = self.session_file().ok_or_else(|| {
            AgentSessionError::Upstream("Cannot export in-memory session to HTML".to_string())
        })?;
        if !std::path::Path::new(&session_file).exists() {
            return Err(AgentSessionError::Upstream(
                "Nothing to export yet - start a conversation first".to_string(),
            ));
        }

        let entries = self
            .session_manager
            .lock()
            .expect("session lock")
            .get_entries();
        let header = self
            .session_manager
            .lock()
            .expect("session lock")
            .get_header();
        let tools = {
            let state = self.agent.state();
            state
                .tools
                .iter()
                .map(|tool| ExportedTool {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    parameters: tool.parameters.clone(),
                })
                .collect()
        };
        // Upstream passes `state.systemPrompt`; the ported AgentState has no
        // prompt field — the session's effective prompt getter is the
        // equivalent value (base + run options, including unsent changes).
        let system_prompt = Some(self.system_prompt());
        let leaf_id = self
            .session_manager
            .lock()
            .expect("session lock")
            .get_leaf_id()
            .map(str::to_string);
        let cwd = self
            .session_manager
            .lock()
            .expect("session lock")
            .get_cwd()
            .to_string();

        // Upstream `exportSessionToHtml` defaults the output name to
        // `${APP_NAME}-session-${basename(sessionFile, ".jsonl")}.html`.
        let resolved_output_path = output_path.clone().unwrap_or_else(|| {
            let basename = node_basename(&session_file);
            let basename = basename.strip_suffix(".jsonl").unwrap_or(basename);
            format!("{}-session-{basename}.html", APP_NAME)
        });

        exporter(HtmlExportData {
            output_path,
            resolved_output_path,
            theme_name,
            session_file,
            header,
            entries,
            leaf_id,
            system_prompt,
            tools,
            cwd,
        })
        .await
        .map_err(AgentSessionError::Upstream)
    }

    /// Upstream `exportToJsonl`: export the current session branch to a JSONL
    /// file. Writes the session header followed by all entries on the current
    /// branch path. `output_path` is resolved against the process cwd when
    /// relative; when omitted, a timestamped file name is generated there.
    pub fn export_to_jsonl(
        &self,
        output_path: Option<String>,
    ) -> Result<String, AgentSessionError> {
        let base_dir = std::env::current_dir()
            .map(|dir| dir.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".to_string());
        let default_name = format!(
            "session-{}.jsonl",
            crate::agent_core::harness::session::jsonl::iso8601::format_iso8601_utc(now_ms())
                .replace([':', '.'], "-")
        );
        let file_path = resolve_path(output_path.as_deref().unwrap_or(&default_name), &base_dir)
            .map_err(|error| AgentSessionError::Upstream(error.to_string()))?;
        if let Some(dir) = std::path::Path::new(&file_path).parent() {
            let _ = std::fs::create_dir_all(dir);
        }

        let content = {
            let manager = self.session_manager.lock().expect("session lock");
            jsonl_document(
                manager.get_session_id(),
                manager.get_cwd(),
                &now_iso_stamp(),
                manager.get_branch(None),
            )?
        };
        std::fs::write(&file_path, content)
            .map_err(|error| AgentSessionError::Upstream(error.to_string()))?;
        Ok(file_path)
    }

    // =========================================================================
    // Utilities (agent-session.ts:3565-3610)
    // =========================================================================

    /// Upstream `getLastAssistantText`: text content of the last assistant
    /// message (skipping aborted messages with no content); useful for /copy.
    pub fn get_last_assistant_text(&self) -> Result<Option<String>, AgentSessionError> {
        let last_assistant = self
            .messages()
            .into_iter()
            .rev()
            .find(|message| match message {
                AgentMessage::Assistant(assistant) => {
                    // Skip aborted messages with no content
                    !(assistant.stop_reason == StopReason::Aborted && assistant.content.is_empty())
                }
                _ => false,
            });
        let Some(AgentMessage::Assistant(assistant)) = last_assistant else {
            return Ok(None);
        };

        let mut text = String::new();
        for block in &assistant.content {
            if let AssistantBlock::Text(content) = block {
                text += &content.text;
            }
        }

        let trimmed = text.trim();
        Ok(if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        })
    }

    // =========================================================================
    // Extension System (agent-session.ts:3598-3625)
    // =========================================================================

    /// Upstream `createReplacedSessionContext`: a fresh command context for a
    /// session that replaced this one (see the `ReplacedSessionContext` seam
    /// in the module docs).
    pub fn create_replaced_session_context(
        &self,
    ) -> Result<ExtensionCommandContext, AgentSessionError> {
        Ok(self.extension_runner().create_command_context())
    }

    /// Upstream `hasExtensionHandlers` (trivial passthrough).
    pub fn has_extension_handlers(&self, event_type: &str) -> bool {
        self.extension_runner().has_handlers(event_type)
    }
}

/// Synchronous extension-registration adapter over the shared native registry.
/// Registration is immediate; it never blocks a Tokio worker on an async mutex.
/// Native providers keep their callbacks and identity all the way to Models.
struct ModelRegistryHandle(Arc<ModelRegistry>);

impl ProviderRegistryHandle for ModelRegistryHandle {
    fn register_provider(&self, name: &str, config: &Value) -> Result<(), String> {
        let input = provider_config_from_value(config)?;
        self.0
            .register_provider_sync(name, input)
            .map_err(|error| error.0)
    }

    fn register_native_provider(
        &self,
        provider: &crate::coding_agent::extensions::types::NativeProvider,
    ) -> Result<(), String> {
        self.0
            .runtime()
            .register_native_provider_sync(Arc::clone(provider))
            .map_err(|error| error.0)
    }

    fn unregister_provider(&self, name: &str) -> Result<(), String> {
        self.0.runtime().unregister_provider_sync(name);
        Ok(())
    }

    fn get_api_key_for_provider(
        self: Arc<Self>,
        provider: String,
    ) -> futures::future::BoxFuture<'static, Option<String>> {
        Box::pin(async move { self.0.get_api_key_for_provider(&provider).await })
    }
}

/// JSON → `ProviderConfigInput` (the JSON-representable subset).
/// Extension models are not complete `Model`s: provider, api and baseUrl are
/// supplied by the composer. Decode their own shape without discarding invalid
/// entries; a malformed registration must fail as a whole, not become empty.
/// Callback-bearing providers use the separate typed native registration path.
pub(crate) fn provider_config_from_value(config: &Value) -> Result<ProviderConfigInput, String> {
    use crate::ai::types::{
        model::ModelInput,
        primitives::{ModelCost, ThinkingLevelMap},
    };
    use crate::coding_agent::core::provider_composer::{ExtensionModelDefinition, HeaderRecord};
    use std::collections::BTreeMap;

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct JsonModel {
        id: String,
        name: String,
        api: Option<String>,
        base_url: Option<String>,
        reasoning: bool,
        thinking_level_map: Option<ThinkingLevelMap>,
        input: Vec<ModelInput>,
        cost: ModelCost,
        context_window: u64,
        max_tokens: u64,
        sampling_params: Option<BTreeMap<String, Value>>,
        sampling_params_by_thinking_level:
            Option<std::collections::BTreeMap<String, BTreeMap<String, Value>>>,
        headers: Option<serde_json::Map<String, Value>>,
        #[serde(default, deserialize_with = "crate::serde_support::present_json")]
        compat: Option<Value>,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct JsonProvider {
        name: Option<String>,
        base_url: Option<String>,
        api_key: Option<String>,
        api: Option<String>,
        headers: Option<serde_json::Map<String, Value>>,
        auth_header: Option<bool>,
        models: Option<Vec<JsonModel>>,
    }

    fn headers(
        values: Option<serde_json::Map<String, Value>>,
        field: &str,
    ) -> Result<Option<HeaderRecord>, String> {
        values.map(|values| {
            let mut entries = values.into_iter().map(|(key, value)| {
                let text = value.as_str().ok_or_else(|| {
                    format!("Invalid extension provider configuration: {field}.{key} must be a string")
                })?;
                Ok((key, text.to_owned()))
            }).collect::<Result<Vec<_>, String>>()?;
            crate::serde_support::order_js_object_entries(&mut entries);
            Ok(entries)
        }).transpose()
    }

    let parsed: JsonProvider = serde_json::from_value(config.clone())
        .map_err(|error| format!("Invalid extension provider configuration: {error}"))?;
    let models = parsed
        .models
        .map(|models| {
            models
                .into_iter()
                .enumerate()
                .map(|(index, model)| {
                    Ok(ExtensionModelDefinition {
                        id: model.id,
                        name: model.name,
                        api: model.api,
                        base_url: model.base_url,
                        reasoning: model.reasoning,
                        thinking_level_map: model.thinking_level_map,
                        input: model.input,
                        cost: model.cost,
                        context_window: model.context_window,
                        max_tokens: model.max_tokens,
                        sampling_params: model.sampling_params,
                        sampling_params_by_thinking_level: model.sampling_params_by_thinking_level,
                        headers: headers(model.headers, &format!("models[{index}].headers"))?,
                        compat: model.compat,
                    })
                })
                .collect::<Result<Vec<_>, String>>()
        })
        .transpose()?;
    Ok(ProviderConfigInput {
        name: parsed.name,
        base_url: parsed.base_url,
        api_key: parsed.api_key,
        api: parsed.api,
        headers: headers(parsed.headers, "headers")?,
        auth_header: parsed.auth_header,
        models,
        ..ProviderConfigInput::default()
    })
}

/// Agent-level `ThinkingLevel` (with `off`) → the provider-level union the
/// settings slice stores. `off` has no representation there (`None`).
fn primitives_thinking_level(
    level: ThinkingLevel,
) -> Option<crate::ai::types::primitives::ThinkingLevel> {
    use crate::ai::types::primitives::ThinkingLevel as Primitive;
    match level {
        ThinkingLevel::Off => None,
        ThinkingLevel::Minimal => Some(Primitive::Minimal),
        ThinkingLevel::Low => Some(Primitive::Low),
        ThinkingLevel::Medium => Some(Primitive::Medium),
        ThinkingLevel::High => Some(Primitive::High),
        ThinkingLevel::Xhigh => Some(Primitive::Xhigh),
        ThinkingLevel::Max => Some(Primitive::Max),
    }
}

// ============================================================================
// Lower-half support types
// ============================================================================

/// Upstream `ExtensionBindings`: the mode-level bindings `bindExtensions`
/// installs. `None` fields keep the previously installed value.
#[derive(Default)]
pub struct ExtensionBindings {
    /// Upstream `uiContext`.
    pub ui_context: Option<Arc<dyn ExtensionUI>>,
    /// Upstream `mode`.
    pub mode: Option<ExtensionMode>,
    /// Upstream `commandContextActions`.
    pub command_context_actions: Option<ExtensionCommandContextActions>,
    /// Upstream `abortHandler`.
    pub abort_handler: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Upstream `shutdownHandler`.
    pub shutdown_handler: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Upstream `onError`.
    pub on_error: Option<ExtensionErrorListener>,
}

/// Upstream `reload` options.
#[derive(Default)]
pub struct ReloadOptions {
    /// Upstream `beforeSessionStart`.
    pub before_session_start: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// Upstream `navigateTree` options.
#[derive(Debug, Clone, Default)]
pub struct NavigateTreeOptions {
    /// Whether the user wants to summarize the abandoned branch.
    pub summarize: Option<bool>,
    /// Custom instructions for the summarizer.
    pub custom_instructions: Option<String>,
    /// If true, `custom_instructions` replaces the default prompt.
    pub replace_instructions: Option<bool>,
    /// Label to attach to the branch summary entry.
    pub label: Option<String>,
}

/// Upstream `navigateTree` result.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NavigateTreeResult {
    /// The editor text when the target was a user message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub editor_text: Option<String>,
    /// Whether the navigation was cancelled (extension or abort).
    pub cancelled: bool,
    /// Whether summarization was aborted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aborted: Option<bool>,
    /// The branch summary entry, when one was created.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_entry: Option<SessionEntry>,
}

/// Upstream `UsageTotals` (`core/usage-totals.ts:7-13`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct UsageTotals {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    cost: f64,
}

/// Upstream `addUsageToTotals` (`core/usage-totals.ts:23-30`).
fn add_usage_to_totals(totals: &mut UsageTotals, usage: &Usage) {
    totals.input += usage.input;
    totals.output += usage.output;
    totals.cache_read += usage.cache_read;
    totals.cache_write += usage.cache_write;
    totals.cost += usage.cost.total;
}

/// Upstream `SessionStats` token totals.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total: u64,
}

/// Upstream `SessionStats` (`getSessionStats` result).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStats {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_file: Option<String>,
    pub session_id: String,
    pub user_messages: u64,
    pub assistant_messages: u64,
    pub tool_calls: u64,
    pub tool_results: u64,
    pub total_messages: u64,
    pub tokens: SessionTokens,
    pub cost: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_usage: Option<ContextUsage>,
}

/// Options for [`AgentSession::export_to_html`] (upstream
/// `{ themeName?: string }`).
#[derive(Debug, Clone, Default)]
pub struct ExportToHtmlOptions {
    pub theme_name: Option<String>,
}

/// One exported tool declaration (upstream
/// `state.tools.map(t => ({name, description, parameters}))`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportedTool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// Everything [`HtmlExportFn`] needs to render the HTML (the data upstream
/// `exportSessionToHtml` collects from the session manager and agent state).
pub struct HtmlExportData {
    /// The caller-provided output path (`None` = session directory default).
    pub output_path: Option<String>,
    /// The output path after the session-level defaulting
    /// (`${APP_NAME}-session-${basename}.html`).
    pub resolved_output_path: String,
    /// Validated theme name (upstream `getThemeByName` registry).
    pub theme_name: Option<String>,
    pub session_file: String,
    pub header: Option<SessionHeader>,
    pub entries: Vec<SessionEntry>,
    pub leaf_id: Option<String>,
    pub system_prompt: Option<String>,
    pub tools: Vec<ExportedTool>,
    pub cwd: String,
}

/// The export-html seam handler (see the module docs): renders and writes the
/// HTML, returning the resolved output path.
pub type HtmlExportFn =
    Arc<dyn Fn(HtmlExportData) -> BoxFuture<'static, Result<String, String>> + Send + Sync>;

/// Streaming chunk callback for [`AgentSession::execute_bash`] (upstream
/// `onChunk`).
pub type OnBashChunk = Arc<dyn Fn(&str) + Send + Sync>;

/// Upstream `executeBash` options.
#[derive(Clone, Default)]
pub struct ExecuteBashOptions {
    /// If true, command output won't be sent to LLM (`!!` prefix).
    pub exclude_from_context: Option<bool>,
    /// Identifier included in bash execution update events.
    pub id: Option<String>,
    /// Custom operations for remote execution. Default: the local shell
    /// backend (see the bash-executor seam in the module docs).
    pub operations: Option<bash_executor::BashOperationsHandle>,
}

/// Upstream `SessionBeforeCompactResult.compaction`: the extension-provided
/// compaction content (`CompactionResult` at its JSON seam).
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExtensionCompaction {
    summary: String,
    first_kept_entry_id: String,
    #[serde(default)]
    tokens_before: u64,
    #[serde(default)]
    usage: Option<Usage>,
    #[serde(default)]
    details: Option<Value>,
}

impl ExtensionCompaction {
    /// The `(summary, firstKeptEntryId, tokensBefore, usage, details)` tuple
    /// the compaction flows destructure into.
    fn into_fields(self) -> (String, String, u64, Option<Usage>, Option<Value>) {
        (
            self.summary,
            self.first_kept_entry_id,
            self.tokens_before,
            self.usage,
            self.details,
        )
    }
}

/// Upstream `SessionBeforeTreeResult.summary`:
/// `{ summary: string, details?: unknown, usage?: Usage }`.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExtensionBranchSummary {
    summary: String,
    #[serde(default)]
    details: Option<Value>,
    #[serde(default)]
    usage: Option<Usage>,
}

/// [`ReadonlySessionManager`] adapter over the shared
/// [`SessionManager`](crate::coding_agent::session_manager::SessionManager)
/// guard (the two reads `collectEntriesForBranchSummary` makes).
struct BranchSummarySession<'a>(&'a SessionManager);

impl ReadonlySessionManager for BranchSummarySession<'_> {
    fn get_branch(&self, id: &str) -> Vec<CompactionSessionEntry> {
        self.0
            .get_branch(Some(id))
            .iter()
            .map(to_compaction_entry)
            .collect()
    }

    fn get_entry(&self, id: &str) -> Option<CompactionSessionEntry> {
        self.0.get_entry(id).map(to_compaction_entry)
    }
}

/// Upstream `RetryPolicy` construction from
/// `settingsManager.getRetrySettings()`.
fn retry_policy_from_settings(settings: &RetrySettings) -> crate::ai::retry::RetryPolicy {
    crate::ai::retry::RetryPolicy {
        enabled: settings.enabled,
        max_retries: settings.max_retries.clamp(0, u32::MAX as i64) as u32,
        base_delay_ms: settings.base_delay_ms.max(0) as u64,
        max_agent_delay_ms: Some(settings.max_agent_delay_ms.max(0) as u64),
    }
}

/// Upstream `retryDelayMs(settings, attempt)` over the retry settings.
fn retry_delay_ms_from_settings(settings: &RetrySettings, attempt: u32) -> u64 {
    crate::ai::retry::retry_delay_ms(&retry_policy_from_settings(settings), attempt)
}

/// `SessionManagerError` → the upstream thrown-message error.
fn session_manager_error(error: SessionManagerError) -> AgentSessionError {
    AgentSessionError::Upstream(error.to_string())
}

/// Clone the command-context actions (every field is an `Arc` handle; the
/// runner clones the arcs again when binding).
fn clone_command_context_actions(
    actions: &ExtensionCommandContextActions,
) -> ExtensionCommandContextActions {
    ExtensionCommandContextActions {
        wait_for_idle: Arc::clone(&actions.wait_for_idle),
        new_session: Arc::clone(&actions.new_session),
        fork: Arc::clone(&actions.fork),
        navigate_tree: Arc::clone(&actions.navigate_tree),
        switch_session: Arc::clone(&actions.switch_session),
        reload: Arc::clone(&actions.reload),
    }
}

/// Node `path.dirname` (the port hosts both separators; upstream's win32
/// dirname handles both).
fn node_dirname(path: &str) -> String {
    match path.rfind(['/', '\\']) {
        Some(0) => path[..1].to_string(),
        Some(index) => path[..index].to_string(),
        None => ".".to_string(),
    }
}

/// Node `path.basename` for the `{APP_NAME}-session-…` export default.
fn node_basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// The current wall-clock ISO stamp used by the JSONL export.
fn now_iso_stamp() -> String {
    crate::agent_core::harness::session::jsonl::iso8601::format_iso8601_utc(now_ms())
}

/// Upstream `exportSessionToJsonl`'s document body
/// (`session-export.ts:21-40`): the `session` header line followed by the
/// branch entries re-chained along the export path, newline-terminated. The
/// trailing `createTrailingEntries` hook has no session-level caller (it is
/// the session-share extension's) and is not exposed here.
fn jsonl_document(
    session_id: &str,
    cwd: &str,
    timestamp: &str,
    branch_entries: Vec<SessionEntry>,
) -> Result<String, AgentSessionError> {
    let header = format!(
        "{{\"type\":\"session\",\"version\":{},\"id\":{},\"timestamp\":{},\"cwd\":{}}}",
        CURRENT_SESSION_VERSION,
        serde_json::to_string(session_id)
            .map_err(|error| AgentSessionError::Upstream(error.to_string()))?,
        serde_json::to_string(timestamp)
            .map_err(|error| AgentSessionError::Upstream(error.to_string()))?,
        serde_json::to_string(cwd)
            .map_err(|error| AgentSessionError::Upstream(error.to_string()))?,
    );
    let mut lines = vec![header];

    let mut parent_id: Option<String> = None;
    for mut entry in branch_entries {
        set_entry_parent_id(&mut entry, parent_id.clone());
        lines.push(
            serde_json::to_string(&entry)
                .map_err(|error| AgentSessionError::Upstream(error.to_string()))?,
        );
        parent_id = entry.id().map(str::to_string);
    }

    Ok(format!("{}\n", lines.join("\n")))
}

/// Rewrite an entry's `parentId` in place (the JSONL export re-chains the
/// branch path). Field-position-preserving equivalent of upstream's
/// `{ ...entry, parentId }` spread.
fn set_entry_parent_id(entry: &mut SessionEntry, parent_id: Option<String>) {
    match entry {
        SessionEntry::Message(e) => e.parent_id = parent_id,
        SessionEntry::ThinkingLevelChange(e) => e.parent_id = parent_id,
        SessionEntry::ModelChange(e) => e.parent_id = parent_id,
        SessionEntry::Usage(e) => e.parent_id = parent_id,
        SessionEntry::ContextEdit(e) => e.parent_id = parent_id,
        SessionEntry::Compaction(e) => e.parent_id = parent_id,
        SessionEntry::BranchSummary(e) => e.parent_id = parent_id,
        SessionEntry::Custom(e) => e.parent_id = parent_id,
        SessionEntry::CustomMessage(e) => e.parent_id = parent_id,
        SessionEntry::Label(e) => e.parent_id = parent_id,
        SessionEntry::SessionInfo(e) => e.parent_id = parent_id,
        SessionEntry::Unparsed(value) => match parent_id {
            Some(parent) => value["parentId"] = Value::String(parent),
            None => {
                if let Some(object) = value.as_object_mut() {
                    object.shift_remove("parentId");
                }
            }
        },
    }
}

/// `contentText` over a custom message entry's content (upstream reads
/// `targetEntry.content` for `custom_message` targets).
fn custom_message_content_text(
    content: Option<&crate::coding_agent::core::messages::CustomMessageContent>,
) -> String {
    match content {
        Some(crate::coding_agent::core::messages::CustomMessageContent::Text(text)) => text.clone(),
        Some(crate::coding_agent::core::messages::CustomMessageContent::Blocks(blocks)) => blocks
            .iter()
            .filter_map(|block| match block {
                TextOrImageBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(""),
        None => String::new(),
    }
}

/// The `navigateTree` new-leaf decision (upstream agent-session.ts:3336-3351):
/// `(newLeafId, editorText)`. User and custom messages move the leaf to the
/// parent (null at root) with their text in the editor; other targets select
/// the node itself.
fn navigate_target_decision(
    target_entry: &SessionEntry,
    target_id: &str,
) -> (Option<String>, Option<String>) {
    match target_entry {
        SessionEntry::Message(message) if message.message.role() == "user" => {
            // User message: leaf = parent (null if root), text goes to editor
            let editor_text = match &message.message {
                AgentMessage::User(user) => content_text_with_separator(&user.content, ""),
                _ => String::new(),
            };
            (message.parent_id.clone(), Some(editor_text))
        }
        SessionEntry::CustomMessage(custom) => {
            // Custom message: leaf = parent (null if root), text goes to
            // editor
            let editor_text = custom_message_content_text(custom.content.as_ref());
            (custom.parent_id.clone(), Some(editor_text))
        }
        // Non-user message: leaf = selected node
        _ => (Some(target_id.to_string()), None),
    }
}

/// Upstream `APP_NAME` (`config.ts:502`, `piConfigName || "pi"`; the same S1
/// seam the session manager pins).
const APP_NAME: &str = "pi";

/// `Record<string,string>` → `ProviderHeaders` (the `string | null` wire map
/// the compaction options carry).
fn flatten_headers(headers: &std::collections::BTreeMap<String, String>) -> ProviderHeaders {
    headers
        .iter()
        .map(|(name, value)| (name.clone(), Some(value.clone())))
        .collect()
}

/// Effective settings for `pi.getSettings()`: the global document with the
/// project document merged over it (objects merge recursively, every other
/// value replaces — the settings slice's `deep_merge_objects` semantics,
/// restated here until that slice exposes a merged-document getter;
/// disclosed seam, see the `get_settings` action binding).
fn merge_settings_documents(global: &Value, project: &Value) -> Value {
    fn merge(base: &Value, overrides: &Value) -> Value {
        match (base, overrides) {
            (Value::Object(base), Value::Object(overrides)) => {
                let mut merged = base.clone();
                for (key, value) in overrides {
                    let merged_value = match merged.get(key) {
                        Some(existing) => merge(existing, value),
                        None => value.clone(),
                    };
                    merged.insert(key.clone(), merged_value);
                }
                Value::Object(merged)
            }
            (_, overrides) => overrides.clone(),
        }
    }
    merge(global, project)
}

/// The `"manual" | "threshold" | "overflow"` wire literal.
fn compaction_reason_str(reason: CompactionReason) -> &'static str {
    match reason {
        CompactionReason::Manual => "manual",
        CompactionReason::Threshold => "threshold",
        CompactionReason::Overflow => "overflow",
    }
}

/// Project a session-manager entry into the compaction seam's entry union
/// (the `From` bridge the compaction seam docs describe).
fn to_compaction_entry(entry: &SessionEntry) -> CompactionSessionEntry {
    match entry {
        SessionEntry::Message(message) => CompactionSessionEntry::Message {
            id: message.id.clone(),
            parent_id: message.parent_id.clone(),
            message: message.message.clone(),
        },
        SessionEntry::CustomMessage(custom) => CompactionSessionEntry::CustomMessage {
            id: custom.id.clone(),
            parent_id: custom.parent_id.clone(),
            custom_type: custom.custom_type.clone(),
            content: match &custom.content {
                Some(content) => content.clone(),
                None => {
                    crate::coding_agent::core::messages::CustomMessageContent::Text(String::new())
                }
            },
            display: custom.display,
            details: custom.details.clone(),
            timestamp: custom.timestamp.clone(),
        },
        SessionEntry::BranchSummary(summary) => CompactionSessionEntry::BranchSummary {
            id: summary.id.clone(),
            parent_id: summary.parent_id.clone(),
            from_id: summary.from_id.clone(),
            summary: summary.summary.clone(),
            details: summary.details.clone(),
            from_hook: summary.from_hook.unwrap_or(false),
            timestamp: summary.timestamp.clone(),
        },
        SessionEntry::Compaction(compaction) => CompactionSessionEntry::Compaction {
            id: compaction.id.clone(),
            parent_id: compaction.parent_id.clone(),
            summary: compaction.summary.clone(),
            first_kept_entry_id: compaction.first_kept_entry_id.clone().unwrap_or_default(),
            tokens_before: compaction.tokens_before,
            details: compaction.details.clone(),
            from_hook: compaction.from_hook.unwrap_or(false),
            system_message: compaction.system_message.clone(),
            timestamp: compaction.timestamp.clone(),
        },
        SessionEntry::ThinkingLevelChange(_)
        | SessionEntry::ModelChange(_)
        | SessionEntry::Usage(_)
        | SessionEntry::ContextEdit(_)
        | SessionEntry::Custom(_)
        | SessionEntry::Label(_)
        | SessionEntry::SessionInfo(_)
        | SessionEntry::Unparsed(_) => CompactionSessionEntry::Other,
    }
}

/// The compaction seam path entries, projected from the session manager.
fn compaction_path_entries(manager: &SessionManager) -> Vec<CompactionSessionEntry> {
    manager
        .get_branch(None)
        .iter()
        .map(to_compaction_entry)
        .collect()
}

/// JSON view of a compaction-seam entry (the session-manager entry's
/// serialized shape; the seam enum carries the same payload).
fn compaction_entry_json(entry: &CompactionSessionEntry) -> Value {
    match entry {
        CompactionSessionEntry::Message {
            id,
            parent_id,
            message,
        } => json!({
            "type": "message",
            "id": id,
            "parentId": parent_id,
            "message": serde_json::to_value(message).unwrap_or(Value::Null),
        }),
        CompactionSessionEntry::CustomMessage {
            id,
            parent_id,
            custom_type,
            content,
            display,
            details,
            timestamp,
        } => json!({
            "type": "custom_message",
            "id": id,
            "parentId": parent_id,
            "customType": custom_type,
            "content": serde_json::to_value(content).unwrap_or(Value::Null),
            "display": display,
            "details": details,
            "timestamp": timestamp,
        }),
        CompactionSessionEntry::BranchSummary {
            id,
            parent_id,
            from_id,
            summary,
            details,
            from_hook,
            timestamp,
        } => json!({
            "type": "branch_summary",
            "id": id,
            "parentId": parent_id,
            "fromId": from_id,
            "summary": summary,
            "details": details,
            "fromHook": from_hook,
            "timestamp": timestamp,
        }),
        CompactionSessionEntry::Compaction {
            id,
            parent_id,
            summary,
            first_kept_entry_id,
            tokens_before,
            details,
            from_hook,
            system_message,
            timestamp,
        } => json!({
            "type": "compaction",
            "id": id,
            "parentId": parent_id,
            "summary": summary,
            "firstKeptEntryId": first_kept_entry_id,
            "tokensBefore": tokens_before,
            "details": details,
            "fromHook": from_hook,
            "systemMessage": system_message
                .as_ref()
                .map(|system| serde_json::to_value(system).unwrap_or(Value::Null))
                .unwrap_or(Value::Null),
            "timestamp": timestamp,
        }),
        CompactionSessionEntry::Other => Value::Null,
    }
}

/// The `session_before_compact` event's `preparation` payload (upstream
/// serializes the live `CompactionPreparation` object; the file-op sets —
/// `JSON.stringify` views them as `{}` — are rendered as lists here so the
/// JSON seam keeps the information the live object carries).
fn preparation_json(preparation: &CompactionPreparation) -> Value {
    json!({
        "firstKeptEntryId": preparation.first_kept_entry_id,
        "messagesToSummarize": serde_json::to_value(&preparation.messages_to_summarize)
            .unwrap_or(Value::Null),
        "turnPrefixMessages": serde_json::to_value(&preparation.turn_prefix_messages)
            .unwrap_or(Value::Null),
        "isSplitTurn": preparation.is_split_turn,
        "tokensBefore": preparation.tokens_before,
        "previousSummary": preparation.previous_summary,
        "fileOps": {
            "read": preparation.file_ops.read.iter().cloned().collect::<Vec<String>>(),
            "written": preparation.file_ops.written.iter().cloned().collect::<Vec<String>>(),
            "edited": preparation.file_ops.edited.iter().cloned().collect::<Vec<String>>(),
        },
        "settings": serde_json::to_value(preparation.settings).unwrap_or(Value::Null),
    })
}

/// Native execution for upstream's deliberately unawaited async action IIFEs.
/// Errors (including a missing runtime) go to the action's existing sink. A
/// dropped caller does not cancel the work; no OS thread or block_on is used.
fn spawn_extension_action<T: Send + 'static>(
    work: impl std::future::Future<Output = Result<T, AgentSessionError>> + Send + 'static,
    complete: impl FnOnce(Result<T, String>) + Send + 'static,
) {
    match tokio::runtime::Handle::try_current() {
        Ok(runtime) => {
            runtime.spawn(async move {
                let result =
                    futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(work)).await;
                complete(match result {
                    Ok(result) => result.map_err(|error| error.to_string()),
                    Err(panic) => Err(panic_message(&panic)),
                });
            });
        }
        Err(_) => complete(Err("Extension async actions require a Tokio runtime".into())),
    }
}
