//! Port of `packages/agent/src/harness/types.ts` (407 lines): the harness
//! foundation vocabulary — resources ([`Skill`], [`PromptTemplate`]), the
//! harness-native tool shape ([`AgentHarnessTool`]), curated stream options,
//! the filesystem/execution capability traits ([`FileSystem`], [`Shell`],
//! [`ExecutionEnv`]) and their stable error codes.
//!
//! Disclosed substitutions:
//! - **`Result<TValue, TError>` (`types.ts:8-41`).** The union maps onto
//!   [`std::result::Result`]: `ok(value)`/`err(error)` are `Ok`/`Err`,
//!   `getOrThrow` is `unwrap`/`expect` (tests and adapter boundaries),
//!   `getOrUndefined` is `.ok()`, and `toError(unknown)` normalizes thrown
//!   values at JS throw/catch boundaries that do not exist in Rust — typed
//!   errors flow through `Result` directly. Nothing is re-exported; the
//!   `FileSystem`/`Shell` methods below return `std Result` values.
//! - **TypeBox generics.** `TParameters`/`TDetails` erase to
//!   `serde_json::Value` at this boundary (the same precedent as
//!   `agent_core::types::AgentTool`); typed tools deserialize on top.
//! - **`FileError`/`ExecutionError`/`CompactionError`/`BranchSummaryError`.**
//!   Upstream `Error` subclasses become plain structs implementing
//!   [`std::error::Error`] with `code`, `message`, optional `path`/`cause`.
//! - **Capability traits.** `FileSystem`, `TextLineReader`, `Shell` are
//!   object-safe traits returning boxed futures (the repo's ported-interface
//!   convention); `ExecutionEnv` is their combined supertrait
//!   (`types.ts:407`).
//! - Wire format for the data types is the upstream camelCase JSON with
//!   optional fields omitted, so `FileInfo`/`ShellOutput*` payloads
//!   round-trip through tool details and events unchanged.

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};

use crate::agent_core::chord_support::Context;
use crate::agent_core::types::{
    AgentToolResult, PrepareArgumentsFn, ToolExecutionMode, ToolReplay,
};
use crate::ai::types::options::DeferredFlag;
use crate::ai::types::primitives::{CacheRetention, Transport};
use crate::ai::types::tool::{ConstrainedSampling, Tool};

/// Resources made available to explicit invocation methods and system-prompt
/// callbacks (upstream `AgentHarnessResources`, `types.ts:72-81`). Generic
/// over the skill/template types like upstream, defaulting to the two
/// foundation shapes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentHarnessResources<S = Skill, P = PromptTemplate> {
    /// Prompt templates available for explicit invocation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_templates: Option<Vec<P>>,
    /// Skills available to the model and explicit skill invocation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<S>>,
}

/// Upstream `Skill` (`types.ts:49-60`): loaded from a `SKILL.md` file or
/// provided by an application. `name`, `description`, and `filePath` are
/// inserted into the system prompt in an XML-formatted block as suggested by
/// agentskills.io.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    /// Stable skill name used for lookup and model-visible listings.
    pub name: String,
    /// Short model-visible description of when to use the skill.
    pub description: String,
    /// Full skill instructions.
    pub content: String,
    /// Absolute path to the skill file; used for model-visible location and
    /// resolving relative references (upstream `filePath`).
    pub file_path: String,
    /// Exclude this skill from model-visible skill lists while still allowing
    /// explicit application invocation (`types.ts:58-59`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_model_invocation: Option<bool>,
}

/// Upstream `PromptTemplate` (`types.ts:62-70`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptTemplate {
    /// Stable template name used for lookup or application command routing.
    pub name: String,
    /// Optional description for command lists or autocomplete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Template content. Argument placeholders are formatted by
    /// `formatPromptTemplateInvocation` (M3b Task 3).
    pub content: String,
}

/// The shared `{ path, source }` input record of the two upstream
/// `loadSourced*` signatures (`skills.ts:88`, `prompt-templates.ts:74`).
/// Source values are preserved exactly and attached to every loaded resource
/// and diagnostic; the agent package does not interpret source values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourcedInput<TSource> {
    /// Directory or file path to load.
    pub path: String,
    /// Application-defined provenance value.
    pub source: TSource,
}

/// Options for one live harness tool progress update
/// (upstream `AgentHarnessToolUpdateOptions`, `types.ts:83-87`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentHarnessToolUpdateOptions {
    /// Request replacement of this invocation's durable recovery checkpoint
    /// (upstream `checkpoint?: true`).
    pub checkpoint: bool,
}

/// Upstream `AgentToolResult` at the harness boundary: the M3a type with
/// `TDetails` erased to JSON.
pub type HarnessToolResult = AgentToolResult;

/// Synchronous full-snapshot progress callback supplied to harness-native
/// tools (upstream `AgentHarnessToolUpdateCallback`, `types.ts:89-93`); sync
/// like the M3a `AgentToolUpdateCallback`.
pub type AgentHarnessToolUpdateCallback =
    dyn Fn(&AgentToolResult, AgentHarnessToolUpdateOptions) + Send + Sync;

/// Stable harness identity for one logical tool call, unchanged during safe
/// replay (upstream `AgentHarnessToolInvocation`, `types.ts:95-105`).
/// Implemented by the harness runtime (M3b Task 10).
pub trait AgentHarnessToolInvocation: Send + Sync {
    /// Opaque session-unique id equal to the call's reserved result-entry id.
    fn invocation_id(&self) -> &str;
    fn operation_id(&self) -> &str;
    fn turn_id(&self) -> &str;
    /// Read one invocation-scoped durable replay memo (`JsonValue |
    /// undefined` upstream).
    fn get_memo<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Option<serde_json::Value>>;
    /// Set or delete one invocation-scoped durable replay memo.
    fn set_memo<'a>(&'a self, name: &'a str, value: Option<serde_json::Value>)
        -> BoxFuture<'a, ()>;
}

/// The future returned by [`AgentHarnessTool`] executors. Tool failures are
/// `Err` (upstream `execute` throws); content-level errors stay in the result.
pub type HarnessToolFuture = Pin<Box<dyn Future<Output = anyhow::Result<AgentToolResult>> + Send>>;

/// Upstream `AgentHarnessTool.execute` (`types.ts:113-122`) erased to JSON
/// arguments: `(toolCallId, params, onUpdate, toolContext, invocation,
/// context)`. `onUpdate` is the sync full-snapshot callback (always present,
/// like upstream's required parameter).
pub type HarnessExecuteFn<TContext> = dyn Fn(
        String,
        serde_json::Value,
        Arc<AgentHarnessToolUpdateCallback>,
        TContext,
        Arc<dyn AgentHarnessToolInvocation>,
        Context,
    ) -> HarnessToolFuture
    + Send
    + Sync;

/// Tool definition executed by the harness with an application-defined
/// context (upstream `AgentHarnessTool`, `types.ts:107-122`):
/// `Omit<AgentTool, "execute">` plus the harness executor signature. The
/// declaration fields mirror `agent_core::types::AgentTool`; use
/// [`AgentHarnessTool::declaration`] where the LLM-facing `Tool` is needed.
pub struct AgentHarnessTool<TContext: Send + Sync + 'static> {
    /// Tool name the model calls.
    pub name: String,
    /// Human-readable label for UI display.
    pub label: String,
    /// Description sent to the model.
    pub description: String,
    /// JSON Schema object describing the parameters (upstream TypeBox
    /// `TSchema`).
    pub parameters: serde_json::Value,
    /// Optional constrained-sampling configuration.
    pub constrained_sampling: Option<ConstrainedSampling>,
    /// Execute the tool call with the context resolved for the current turn
    /// snapshot.
    pub execute: Arc<HarnessExecuteFn<TContext>>,
    /// Optional compatibility shim for raw tool-call arguments before schema
    /// validation.
    pub prepare_arguments: Option<Arc<PrepareArgumentsFn>>,
    /// Recovery policy for effects with unknown outcomes (upstream `replay`).
    pub replay: Option<ToolReplay>,
    /// Per-tool execution mode override.
    pub execution_mode: Option<ToolExecutionMode>,
}

impl<TContext: Send + Sync + 'static> Clone for AgentHarnessTool<TContext> {
    fn clone(&self) -> Self {
        AgentHarnessTool {
            name: self.name.clone(),
            label: self.label.clone(),
            description: self.description.clone(),
            parameters: self.parameters.clone(),
            constrained_sampling: self.constrained_sampling.clone(),
            execute: Arc::clone(&self.execute),
            prepare_arguments: self.prepare_arguments.clone(),
            replay: self.replay,
            execution_mode: self.execution_mode,
        }
    }
}

impl<TContext: Send + Sync + 'static> fmt::Debug for AgentHarnessTool<TContext> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentHarnessTool")
            .field("name", &self.name)
            .field("label", &self.label)
            .field("description", &self.description)
            .field("parameters", &self.parameters)
            .field("constrained_sampling", &self.constrained_sampling)
            .field("prepare_arguments", &self.prepare_arguments.is_some())
            .field("replay", &self.replay)
            .field("execution_mode", &self.execution_mode)
            .finish_non_exhaustive()
    }
}

impl<TContext: Send + Sync + 'static> AgentHarnessTool<TContext> {
    /// The tool declaration sent to providers / carried by transcript system
    /// messages (the `Omit<AgentTool, "execute">` part).
    pub fn declaration(&self) -> Tool {
        Tool {
            name: self.name.clone(),
            description: self.description.clone(),
            parameters: self.parameters.clone(),
            constrained_sampling: self.constrained_sampling.clone(),
        }
    }
}

/// Upstream `AgentHarnessToolContextSource` (`types.ts:124-127`): static tool
/// context or provider resolved for each turn snapshot. The sync branch of
/// the upstream union folds into the boxed future.
pub enum AgentHarnessToolContextSource<TContext: Send + Sync + 'static> {
    Static(TContext),
    Provider(Arc<dyn Fn(Context) -> BoxFuture<'static, TContext> + Send + Sync>),
}

impl<TContext: Send + Sync + 'static> fmt::Debug for AgentHarnessToolContextSource<TContext> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Static(_) => f.debug_tuple("Static").field(&"..").finish(),
            Self::Provider(_) => f.debug_tuple("Provider").field(&"..").finish(),
        }
    }
}

/// Curated provider request options owned by the harness and snapshotted per
/// turn (upstream `AgentHarnessStreamOptions`, `types.ts:129-147`). The
/// harness deliberately does not expose the run's abort signal — that is the
/// upstream `signal`-shaped field the type omits.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentHarnessStreamOptions {
    /// Preferred transport forwarded to the stream function.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<Transport>,
    /// Provider request timeout in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Maximum provider retry attempts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
    /// Optional cap for provider-requested retry delays.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_retry_delay_ms: Option<u64>,
    /// Additional request headers merged with auth and lifecycle headers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, String>>,
    /// Provider metadata forwarded with requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, serde_json::Value>>,
    /// Provider cache retention hint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_retention: Option<CacheRetention>,
    /// Ask a capable provider to continue generation asynchronously
    /// (`boolean | { window?: "15m" | "1h" | "24h" }`, `types.ts:146`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deferred: Option<DeferredFlag>,
}

/// Deserialize a patch field that distinguishes absent from explicit `null`:
/// an absent key takes the `None` default, `null` becomes `Some(None)`, a
/// value becomes `Some(Some(v))`. Plain `Option<Option<T>>` deserialization
/// collapses `null` to `None`, losing the upstream explicit-`undefined`
/// deletion state (the JS `"key" in patch` + `undefined` distinction the
/// `before_request` hooks rely on).
fn deserialize_explicit_undefined<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    serde::Deserialize::deserialize(deserializer).map(Some)
}

/// Per-request stream option patch returned by provider hooks
/// (upstream `AgentHarnessStreamOptionsPatch`, `types.ts:149-156`).
///
/// Every field is a double option because upstream distinguishes three states
/// per key that a plain `Option` cannot express (the `in`-check plus the
/// `undefined` value; exercised by the `before_request` hooks):
/// - `None` — field absent from the patch (upstream `!(key in patch)`): the
///   base value is untouched. This is the partial-patch shape most hooks
///   return.
/// - `Some(None)` — field present with an `undefined` value (upstream
///   explicit deletion): the scalar/map is removed; for `headers`/`metadata`
///   that clears the whole map ("explicit `headers: undefined` clears all
///   headers").
/// - `Some(Some(value))` — set to `value`. Map entries are themselves
///   delete-capable (`Record<string, string | undefined>`): an inner `None`
///   deletes that key from the base map.
///
/// Serde maps the three states to omitted / `null` / value, so upstream JSON
/// patches round-trip unchanged.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentHarnessStreamOptionsPatch {
    #[serde(
        default,
        deserialize_with = "deserialize_explicit_undefined",
        skip_serializing_if = "Option::is_none"
    )]
    pub transport: Option<Option<Transport>>,
    #[serde(
        default,
        deserialize_with = "deserialize_explicit_undefined",
        skip_serializing_if = "Option::is_none"
    )]
    pub timeout_ms: Option<Option<u64>>,
    #[serde(
        default,
        deserialize_with = "deserialize_explicit_undefined",
        skip_serializing_if = "Option::is_none"
    )]
    pub max_retries: Option<Option<u32>>,
    #[serde(
        default,
        deserialize_with = "deserialize_explicit_undefined",
        skip_serializing_if = "Option::is_none"
    )]
    pub max_retry_delay_ms: Option<Option<u64>>,
    /// Header patch: `Some(Some(map))` merges (deleting `None`-valued keys);
    /// `Some(None)` clears all headers; `None` leaves headers untouched.
    #[serde(
        default,
        deserialize_with = "deserialize_explicit_undefined",
        skip_serializing_if = "Option::is_none"
    )]
    pub headers: Option<Option<BTreeMap<String, Option<String>>>>,
    /// Metadata patch with the same three-state semantics as `headers`.
    #[serde(
        default,
        deserialize_with = "deserialize_explicit_undefined",
        skip_serializing_if = "Option::is_none"
    )]
    pub metadata: Option<Option<BTreeMap<String, Option<serde_json::Value>>>>,
    #[serde(
        default,
        deserialize_with = "deserialize_explicit_undefined",
        skip_serializing_if = "Option::is_none"
    )]
    pub cache_retention: Option<Option<CacheRetention>>,
    #[serde(
        default,
        deserialize_with = "deserialize_explicit_undefined",
        skip_serializing_if = "Option::is_none"
    )]
    pub deferred: Option<Option<DeferredFlag>>,
}

/// Upstream `FileKind` (`types.ts:159`). Symlinks are not followed
/// automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    File,
    Directory,
    Symlink,
}

/// Upstream `FileErrorCode` (`types.ts:162-170`): stable, backend-independent
/// file error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileErrorCode {
    Aborted,
    NotFound,
    PermissionDenied,
    NotDirectory,
    IsDirectory,
    Invalid,
    NotSupported,
    Unknown,
}

/// Upstream `FileError` (`types.ts:173-185`): the error returned by
/// [`FileSystem`] operations.
#[derive(Debug)]
pub struct FileError {
    /// Backend-independent error code.
    pub code: FileErrorCode,
    pub message: String,
    /// Absolute addressed path associated with the failure, when available.
    pub path: Option<String>,
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl FileError {
    /// `new FileError(code, message, path?, cause?)`.
    pub fn new(code: FileErrorCode, message: impl Into<String>, path: Option<String>) -> Self {
        FileError {
            code,
            message: message.into(),
            path,
            cause: None,
        }
    }

    /// Attach the upstream constructor's `cause` argument.
    pub fn with_cause(mut self, cause: Option<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        self.cause = cause;
        self
    }
}

impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for FileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause.as_ref().map(|cause| &**cause as _)
    }
}

/// Upstream `ExecutionErrorCode` (`types.ts:188-195`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionErrorCode {
    Aborted,
    Timeout,
    ShellUnavailable,
    SpawnError,
    CallbackError,
    Unknown,
}

/// Upstream `ExecutionError` (`types.ts:197-206`): the error returned by
/// [`Shell::exec`].
#[derive(Debug)]
pub struct ExecutionError {
    pub code: ExecutionErrorCode,
    pub message: String,
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl ExecutionError {
    /// `new ExecutionError(code, message, cause?)`.
    pub fn new(code: ExecutionErrorCode, message: impl Into<String>) -> Self {
        ExecutionError {
            code,
            message: message.into(),
            cause: None,
        }
    }

    /// Attach the upstream constructor's `cause` argument.
    pub fn with_cause(mut self, cause: Option<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        self.cause = cause;
        self
    }
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ExecutionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause.as_ref().map(|cause| &**cause as _)
    }
}

/// Upstream `CompactionErrorCode` (`types.ts:209`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionErrorCode {
    Aborted,
    SummarizationFailed,
}

/// Upstream `CompactionError` (`types.ts:212-221`).
#[derive(Debug)]
pub struct CompactionError {
    pub code: CompactionErrorCode,
    pub message: String,
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl CompactionError {
    /// `new CompactionError(code, message, cause?)`.
    pub fn new(code: CompactionErrorCode, message: impl Into<String>) -> Self {
        CompactionError {
            code,
            message: message.into(),
            cause: None,
        }
    }

    /// Attach the upstream constructor's `cause` argument.
    pub fn with_cause(mut self, cause: Option<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        self.cause = cause;
        self
    }
}

impl fmt::Display for CompactionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CompactionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause.as_ref().map(|cause| &**cause as _)
    }
}

/// Upstream `BranchSummaryErrorCode` (`types.ts:224`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchSummaryErrorCode {
    Aborted,
    SummarizationFailed,
}

/// Upstream `BranchSummaryError` (`types.ts:227-236`).
#[derive(Debug)]
pub struct BranchSummaryError {
    pub code: BranchSummaryErrorCode,
    pub message: String,
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl BranchSummaryError {
    /// `new BranchSummaryError(code, message, cause?)`.
    pub fn new(code: BranchSummaryErrorCode, message: impl Into<String>) -> Self {
        BranchSummaryError {
            code,
            message: message.into(),
            cause: None,
        }
    }

    /// Attach the upstream constructor's `cause` argument.
    pub fn with_cause(mut self, cause: Option<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        self.cause = cause;
        self
    }
}

impl fmt::Display for BranchSummaryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for BranchSummaryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause.as_ref().map(|cause| &**cause as _)
    }
}

/// Upstream `FileInfo` (`types.ts:239-250`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileInfo {
    /// Basename of `path`.
    pub name: String,
    /// Absolute, syntactically normalized addressed path; symlinks are not
    /// followed.
    pub path: String,
    /// Object kind; symlink targets are not followed.
    pub kind: FileKind,
    /// Size in bytes for the addressed filesystem object.
    pub size: u64,
    /// Modification time as milliseconds since Unix epoch. `f64` because the
    /// upstream `number` admits fractional milliseconds (stat precision).
    pub mtime_ms: f64,
}

/// Upstream `TextLine` (`types.ts:253-257`): one UTF-8 line read from a text
/// file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextLine {
    pub text: String,
    /// Whether the line ended with `\n`; callers use this to discard a torn
    /// final record.
    pub terminated: bool,
}

/// Upstream `TextLineReader` (`types.ts:260-264`): pull-based UTF-8 line
/// reader that preserves final-line termination. Implemented by the
/// execution-environment backends (M3b Task 6).
pub trait TextLineReader: Send + Sync {
    fn read_line<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, Result<Option<TextLine>, FileError>>;
    /// Release the open file. Must be best-effort and must not fail.
    fn close<'a>(&'a self, context: Context) -> BoxFuture<'a, ()>;
}

/// Upstream `{ maxLines?: number }` (`types.ts:288`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadTextLinesOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_lines: Option<u32>,
}

/// Upstream `{ recursive?: boolean }` (`types.ts:311`, defaults `true`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDirOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recursive: Option<bool>,
}

/// Upstream `{ recursive?: boolean; force?: boolean }` (`types.ts:316`,
/// defaults `false`/`false`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recursive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force: Option<bool>,
}

/// Upstream `{ prefix?: string; suffix?: string }` (`types.ts:324`,
/// defaults `""`/`""`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TempFileOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suffix: Option<String>,
}

/// Upstream `string | Uint8Array` (`types.ts:296, 298`): content for
/// [`FileSystem::write_file`] / [`FileSystem::append_file`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileContent {
    Text(String),
    Binary(Vec<u8>),
}

/// Upstream `FileSystem` (`types.ts:275-331`): the filesystem capability used
/// by the harness.
///
/// Paths passed to methods may be absolute or relative to [`FileSystem::cwd`].
/// Paths returned by file operations are addressed paths, not canonicalized
/// through symlinks unless returned by [`FileSystem::canonical_path`].
///
/// The upstream invariant is preserved: operation methods must never panic or
/// reject — all failures, including unexpected backend failures, are encoded
/// in the returned `Result`.
pub trait FileSystem: Send + Sync {
    /// Current working directory for relative paths.
    fn cwd(&self) -> &str;

    /// Return an absolute addressed path without requiring it to exist and
    /// without resolving symlinks.
    fn absolute_path<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>>;
    /// Join path segments in the filesystem namespace without requiring the
    /// result to exist.
    fn join_path<'a>(
        &'a self,
        parts: &[String],
        context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>>;
    /// Read a UTF-8 text file.
    fn read_text_file<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>>;
    /// Open a UTF-8 text file for pull-based line reading.
    fn open_text_line_reader<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<Arc<dyn TextLineReader>, FileError>>;
    /// Read UTF-8 text lines; stop once `maxLines` lines have been read.
    fn read_text_lines<'a>(
        &'a self,
        path: &str,
        options: Option<&ReadTextLinesOptions>,
        context: Context,
    ) -> BoxFuture<'a, Result<Vec<String>, FileError>>;
    /// Read a binary file.
    fn read_binary_file<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<Vec<u8>, FileError>>;
    /// Create or overwrite a file, creating parent directories when
    /// supported.
    fn write_file<'a>(
        &'a self,
        path: &str,
        content: FileContent,
        context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>>;
    /// Create or append to a file, creating parent directories when
    /// supported.
    fn append_file<'a>(
        &'a self,
        path: &str,
        content: FileContent,
        context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>>;
    /// Atomically rename a file, replacing the destination when it exists.
    /// Does not copy across filesystems.
    fn rename_file<'a>(
        &'a self,
        source_path: &str,
        destination_path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>>;
    /// Return metadata for the addressed path without following symlinks.
    fn file_info<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<FileInfo, FileError>>;
    /// List direct children of a directory without following symlinks.
    fn list_dir<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<Vec<FileInfo>, FileError>>;
    /// Return the canonical path for an existing path, resolving symlinks
    /// where supported.
    fn canonical_path<'a>(
        &'a self,
        path: &str,
        context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>>;
    /// Return `Ok(false)` for missing paths; other errors (permission
    /// failures, ...) return a [`FileError`].
    fn exists<'a>(&'a self, path: &str, context: Context)
        -> BoxFuture<'a, Result<bool, FileError>>;
    /// Create a directory. Defaults to `recursive: true`.
    fn create_dir<'a>(
        &'a self,
        path: &str,
        options: Option<&CreateDirOptions>,
        context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>>;
    /// Remove a file or directory. Defaults to `recursive: false` and
    /// `force: false`.
    fn remove<'a>(
        &'a self,
        path: &str,
        options: Option<&RemoveOptions>,
        context: Context,
    ) -> BoxFuture<'a, Result<(), FileError>>;
    /// Create a temporary directory and return its absolute path. Defaults to
    /// `prefix: "tmp-"`.
    fn create_temp_dir<'a>(
        &'a self,
        prefix: Option<&str>,
        context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>>;
    /// Create a temporary file and return its absolute path. Defaults to
    /// `prefix: ""` and `suffix: ""`.
    fn create_temp_file<'a>(
        &'a self,
        options: Option<&TempFileOptions>,
        context: Context,
    ) -> BoxFuture<'a, Result<String, FileError>>;

    /// Release filesystem resources. Must be best-effort and must not fail.
    fn cleanup<'a>(&'a self, context: Context) -> BoxFuture<'a, ()>;
}

/// Upstream `ShellOutputRetention` (`types.ts:334`): which portion of bounded
/// output survives after the limit is crossed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellOutputRetention {
    Head,
    Tail,
}

/// Upstream `ShellOutputLimits` (`types.ts:337-342`): source-side limits for
/// one combined shell output view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellOutputLimits {
    pub max_bytes: u64,
    pub max_lines: u64,
    /// Defaults to `Tail`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retain: Option<ShellOutputRetention>,
}

/// Upstream `ShellOutputCaptureOptions` (`types.ts:345-349`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellOutputCaptureOptions {
    pub limits: ShellOutputLimits,
    /// Preserve complete output in an execution-environment-local file after
    /// the limits are crossed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spill: Option<bool>,
}

/// Which independent limit a truncation hit (upstream
/// `TruncationResult.truncatedBy`: `"lines" | "bytes" | null`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TruncatedBy {
    Lines,
    Bytes,
}

/// Upstream `ShellOutputTruncation` (`types.ts:352`): `Omit<TruncationResult,
/// "content">` from `harness/utils/truncate.ts:15-31` — truncation metadata
/// without a duplicate copy of the retained text. The full `TruncationResult`
/// (with `content`) is defined with the truncate port; the bounded-view
/// consumers here only need this shape.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellOutputTruncation {
    /// Whether truncation occurred.
    pub truncated: bool,
    /// Which limit was hit; `None` when not truncated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated_by: Option<TruncatedBy>,
    /// Total number of lines in the original content.
    pub total_lines: u64,
    /// Total number of bytes in the original content.
    pub total_bytes: u64,
    /// Number of complete lines in the truncated output.
    pub output_lines: u64,
    /// Number of bytes in the truncated output.
    pub output_bytes: u64,
    /// Whether the last line was partially truncated (tail-truncation edge).
    pub last_line_partial: bool,
    /// Whether the first line exceeded the byte limit (head truncation).
    pub first_line_exceeds_limit: bool,
    /// The max lines limit that was applied.
    pub max_lines: u64,
    /// The max bytes limit that was applied.
    pub max_bytes: u64,
}

/// Upstream `ShellOutputMetadata` (`types.ts:355-359`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellOutputMetadata {
    pub truncation: ShellOutputTruncation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spill_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_line_bytes: Option<u64>,
}

/// Upstream `ShellOutputView` (`types.ts:362-364`): complete bounded shell
/// output view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellOutputView {
    #[serde(flatten)]
    pub metadata: ShellOutputMetadata,
    pub text: String,
}

/// Upstream `ShellOutputUpdate` (`types.ts:367-371`): incremental source-side
/// change to one bounded shell output view, tagged by `kind`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ShellOutputUpdate {
    Replace {
        output: ShellOutputView,
    },
    Append {
        text: String,
        metadata: ShellOutputMetadata,
    },
    /// The window slid forward: `drop` leading characters left the view. The
    /// unit is the JS string length the upstream producer computes
    /// (`output-capture.ts:188`, `previous.text.length - shared`, applied via
    /// `text.slice(update.drop)`) — UTF-16 code units, not bytes; Rust
    /// consumers applying the update must convert for non-ASCII output.
    Slide {
        drop: u64,
        text: String,
        metadata: ShellOutputMetadata,
    },
    Metadata {
        metadata: ShellOutputMetadata,
    },
}

/// Upstream `ShellExecResult` (`types.ts:374-376`): bounded shell completion;
/// output text is delivered through the `onUpdate` callback.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellExecResult {
    #[serde(flatten)]
    pub metadata: ShellOutputMetadata,
    pub exit_code: i32,
}

/// Upstream `ShellExecOptions.onUpdate` (`types.ts:390-391`): called with
/// bounded output changes (sync, like the upstream callback).
pub type ShellUpdateCallback = dyn Fn(ShellOutputUpdate, &Context) + Send + Sync;

/// Upstream `ShellExecOptions` (`types.ts:379-392`). Runtime options (the
/// upstream object carries a callback), so no serde.
#[derive(Clone, Default)]
pub struct ShellExecOptions {
    /// Working directory for the command; relative paths resolve against
    /// [`FileSystem::cwd`] unless overridden. Defaults to the env's cwd.
    pub cwd: Option<String>,
    /// Environment variables for the command; values override inherited
    /// defaults when `inherit_env` is true.
    pub env: Option<BTreeMap<String, String>>,
    /// Whether to inherit the execution environment's default variables
    /// (defaults to true).
    pub inherit_env: Option<bool>,
    /// Timeout in seconds. Implementations return a timeout error when the
    /// command exceeds this duration; `None` means no timeout.
    pub timeout: Option<f64>,
    /// Source-side bounded capture. Output is discarded when this and
    /// `on_update` are both absent.
    pub capture: Option<ShellOutputCaptureOptions>,
    /// Called with bounded output changes (upstream sync callback).
    pub on_update: Option<Arc<ShellUpdateCallback>>,
}

impl fmt::Debug for ShellExecOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShellExecOptions")
            .field("cwd", &self.cwd)
            .field("env", &self.env)
            .field("inherit_env", &self.inherit_env)
            .field("timeout", &self.timeout)
            .field("capture", &self.capture)
            .field("on_update", &self.on_update.is_some())
            .finish()
    }
}

/// Upstream `Shell` (`types.ts:395-404`): the shell execution capability.
pub trait Shell: Send + Sync {
    /// Execute a shell command in the filesystem cwd unless
    /// [`ShellExecOptions::cwd`] is provided.
    fn exec<'a>(
        &'a self,
        command: &str,
        options: Option<&ShellExecOptions>,
        context: Context,
    ) -> BoxFuture<'a, Result<ShellExecResult, ExecutionError>>;
    /// Release shell resources. Must be best-effort and must not fail.
    fn cleanup<'a>(&'a self, context: Context) -> BoxFuture<'a, ()>;
}

/// Upstream `ExecutionEnv` (`types.ts:407`): the filesystem and process
/// execution environment used by the harness.
pub trait ExecutionEnv: FileSystem + Shell {}

#[cfg(test)]
mod tests;
