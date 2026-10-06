//! Port of the public AgentHarness shell,
//! `packages/agent/src/harness/agent-harness.ts` (622 lines, upstream SHA256
//! `f00e89cdf1412e4193db0207c9358d89116e240fba1823c9db81eb6e8f1f85f5`): the
//! full public type vocabulary (result aliases, operation/admission shapes,
//! the complete [`HarnessEvent`] union, hook and listener surfaces,
//! [`AgentLane`]/[`AgentHarnessOptions`]/[`AgentHarness`]) plus the runtime
//! bindings the 622-line module exports:
//!
//! - `AgentHarness.create = createAgentHarness`
//!   (`runtime/harness.ts`, ported in [`harness_impl`], 408 lines), and
//! - the lane-bound drive dispatcher `driveOperation`
//!   (`runtime/drive.ts`, ported in [`drive_operation`], 106 lines) — the
//!   public entry that ties the landed drive procedures together.
//!
//! Upstream export → Rust mapping (the closed [`TaggedError`] enum stands in
//! for the per-class error exports; `matchError` is a plain `match`):
//!
//! | upstream export | Rust |
//! |---|---|
//! | `Closed`/`HarnessClosed`/`HarnessFault`/`InvalidLane`/`InvalidMessage`/`InvalidNavigation`/`LaneBusy`/`NoActiveOperation`/`NoActiveRun`/`NothingToCompact`/`NothingToResume`/`OperationMismatch`/`UnknownSkill`/`UnknownTarget`/`UnknownTemplate` | [`TaggedError`] variants ([`HarnessClosed`]/[`HarnessFault`] are structs); see [`result`] |
//! | `SliceNotImplemented` | [`SliceNotImplemented`] |
//! | `SuspendedRun` | [`SuspendedRun`] |
//! | `RunResult`/`CompactionResult`/`NavigationResult`/`ResumeResult`/`QueueResult`/`CancelQueuedResult`/`AbortResult`/`RecordUsageResult`/`DriveResult`/`AbortRequestResult`/`OperationAdmissionResult` | the same-named aliases over [`std::result::Result`] with [`TaggedError`] as the error channel (the documented `Result<T,E>` substitution); value-side unions are [`RunOutcome`], [`CompactionOutcome`], [`NavigationOutcome`], [`QueueOutcome`], [`CancelQueuedOutcome`], [`AbortOutcome`], [`RecordUsageOutcome`], [`AbortRequestOutcome`] |
//! | `NavigateOptions`/`OperationRequest`/`OperationAdmission`/`OperationAdmissionError`/`DriveOptions`/`DriveOutcome` | [`NavigateOptions`], [`OperationRequest`], [`OperationAdmission`] (re-export), the `OperationAdmissionError` doc set on [`OperationAdmissionResult`], [`DriveOptions`]/[`DriveOutcome`] (re-exports) |
//! | `ModelIdentity`/`OperationStatus`/`CurrentOperationInfo`/`LaneExecutionInfo`/`DriveOutcome`/`AbortRequestResult`/`WatchHandle`/`LaneInfo`/`LaneSnapshotTool`/`OpenOperation`/`LaneQueuedItem`/`LaneSnapshot`/`SessionSnapshot` | [`ModelIdentity`], [`OperationStatus`] (re-export), [`CurrentOperationInfo`], [`LaneExecutionInfo`], [`WatchHandle`] (re-export), [`LaneInfo`], [`LaneSnapshotTool`]/[`LaneQueuedItem`]/[`LaneSnapshot`] (re-exports of the projection ports), [`OpenOperation`], [`SessionSnapshot`] |
//! | `HarnessEventPayload`/`SpecialEventPayload`/`LaneEventPayload`/`ConfigEventPayload`/`LaneConfigEventPayload`/`GlobalConfigEventPayload`/`HandlerErrorPayload`/`HarnessEvent` | the single closed enum [`HarnessEvent`] (payload discriminators become variants; the special/lane/config payload sub-unions become [`ValueUpdatePayload`] and [`PublicConfigProperty`]) |
//! | `HarnessEventType`/`EventListener`/`Events` | [`BusEvent::event_type`], [`EventListener`], [`Events`] |
//! | `LaneTranscriptSnapshot`/`LaneWatchEvent` | [`LaneSnapshot`] / [`HarnessEvent`] strict-JSON forms (same serde wires) |
//! | `HookMap`/`HookName`/`HookInvocation`/`HookHandler`/`Hooks` | [`hooks`](super::hooks) ports ([`HookEvent`]/[`HookResult`]/[`HookInvocation`]/[`HookHandler`]), aliased as [`Hooks`] |
//! | `Resources`/`EntryProjector` | [`Resources`], [`EntryProjector`] |
//! | `AgentHarnessOptions`/`AgentLane`/`AcquireLaneOptions`/`AgentHarness`/`AgentHarnessConstructor` | [`AgentHarnessOptions`], [`AgentLane`], [`AcquireLaneOptions`], [`AgentHarness`], [`create_agent_harness`] |
//!
//! Disclosed substitutions (per-item notes at the types):
//! - Upstream `Result<TValue, TError>` value unions become closed enums; the
//!   error channel is [`TaggedError`] with each alias's doc comment naming
//!   the exact upstream variant set.
//! - The lane-scoped event envelopes carry `recovery` exactly on the
//!   variants the landed drive slices stamp ([`runtime::events::HarnessEvent`]
//!   declarations), which is the subset upstream recovery paths emit.
//! - `AgentHarness.lane()` hands out the concrete runtime
//!   [`runtime::lane::Lane`] (which implements [`AgentLane`]); upstream also
//!   hands out the concrete `Lane` instance and the runtime tests assert
//!   `instanceof Lane`.
//! - [`AgentLane`] members delegate to the runtime [`runtime::lane::Lane`]
//!   (upstream `runtime/lane.ts`); the one remaining shell seam is
//!   `watchSession`, which returns [`SliceNotImplemented`] exactly like
//!   upstream (`runtime/harness.ts:305-307`).
//! - `Config.systemPrompt` carries the string form only (the
//!   [`runtime::lane::RuntimeConfig`] precedent; the callable form lands
//!   with the tools factories).

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};

use crate::agent_core::harness::context::Context;
use crate::agent_core::harness::events::{BusEvent, WatchHandle};
use crate::agent_core::harness::session::Session as _;
use crate::agent_core::harness::session::{LaneModel, OperationResultRecord};
use crate::agent_core::types::{AgentMessage, QueueMode, ThinkingLevel, ToolExecutionMode};
use crate::ai::types::content::ImageContent;
use crate::ai::types::options::DeferredHandle;
use crate::ai::types::primitives::Usage;

// ---------------------------------------------------------------------------
// Re-exports forming the upstream export surface
// ---------------------------------------------------------------------------

pub use crate::agent_core::harness::hooks::{
    HookEvent, HookHandler, HookInvocation, HookName, HookRegistry, HookResult, Resources,
};
pub use crate::agent_core::harness::result::TaggedError;
pub use crate::agent_core::harness::runtime::drive_pass::{
    Drive, DriveOptions, DriveOutcome, WaitingReason,
};
pub use crate::agent_core::harness::runtime::lane::OperationAdmission;
pub use crate::agent_core::harness::runtime::projection::{
    LaneQueuedItem, LaneSnapshot, LaneSnapshotTool, OperationKind, OperationStatus,
};
pub use crate::agent_core::harness::session::context::EntryProjector;
pub use crate::agent_core::harness::types::{
    AgentHarnessResources, AgentHarnessStreamOptions, AgentHarnessStreamOptionsPatch,
    AgentHarnessTool, PromptTemplate, Skill,
};

/// Upstream `Hooks` (`agent-harness.ts:512-514`): the port is the landed
/// [`HookRegistry`].
pub type Hooks = HookRegistry;
/// Upstream `Events` (`agent-harness.ts:419-424`): the port is the generic
/// harness event bus instantiated over the full [`HarnessEvent`] union.
pub type Events = crate::agent_core::harness::events::HarnessEventBus<HarnessEvent>;
/// Upstream `WatchHandle<T>` (`agent-harness.ts:181-186`), instantiated over
/// the full event union.
pub type HarnessWatchHandle<T> = WatchHandle<T, HarnessEvent>;
/// Upstream `ModelIdentity` (`agent-harness.ts:142-145`): structurally the
/// durable lane model (`LaneConfiguration["model"]`).
pub type ModelIdentity = LaneModel;

#[path = "agent_harness/drive_operation.rs"]
pub mod drive_operation;
#[path = "agent_harness/harness_impl.rs"]
pub mod harness_impl;

#[cfg(test)]
#[path = "agent_harness/tests.rs"]
mod tests;

// ---------------------------------------------------------------------------
// SliceNotImplemented (upstream runtime/types.ts:18-23, exported by
// agent-harness.ts:52)
// ---------------------------------------------------------------------------

/// Upstream `SliceNotImplemented`: raised for surface whose AgentHarness
/// slice has not landed. The upstream `name` becomes the Display tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SliceNotImplemented(pub String);

impl SliceNotImplemented {
    /// Upstream constructor message:
    /// `` `{operation} is not implemented until its later AgentHarness slice` ``.
    pub fn new(operation: impl Into<String>) -> Self {
        SliceNotImplemented(operation.into())
    }

    /// The message form upstream `Error.prototype.message` carries.
    pub fn message(&self) -> String {
        format!(
            "{} is not implemented until its later AgentHarness slice",
            self.0
        )
    }
}

impl fmt::Display for SliceNotImplemented {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for SliceNotImplemented {}

// ---------------------------------------------------------------------------
// Result aliases (upstream agent-harness.ts:84-103, 134, 169, 171-179)
// ---------------------------------------------------------------------------

/// Convenience-only suspended run observation (upstream `SuspendedRun`,
/// `agent-harness.ts:77-82`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuspendedRun {
    pub operation_id: String,
    /// Upstream `status: "suspended"` literal.
    pub status: SuspendedStatus,
    pub deferred: DeferredHandle,
}

/// The `status: "suspended"` literal of [`SuspendedRun`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SuspendedStatus {
    #[serde(rename = "suspended")]
    Suspended,
}

/// Upstream `RunResult` value union
/// (`OperationResultRecord | SuspendedRun`).
#[derive(Debug, Clone, PartialEq)]
pub enum RunOutcome {
    Record(OperationResultRecord),
    Suspended(SuspendedRun),
}

/// Upstream `RunResult`
/// (`Result<OperationResultRecord | SuspendedRun, LaneBusy | InvalidMessage |
/// UnknownSkill | UnknownTemplate | Closed>`); the error channel is
/// [`TaggedError`] with exactly that variant set.
pub type RunResult = std::result::Result<RunOutcome, TaggedError>;

/// Upstream `CompactionResult` value
/// (`{ compaction: OperationResultRecord; run?: ... }`).
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionOutcome {
    pub compaction: OperationResultRecord,
    pub run: Option<RunOutcome>,
}

/// Upstream `CompactionResult`
/// (`Result<..., LaneBusy | NothingToCompact | Closed>`).
pub type CompactionResult = std::result::Result<CompactionOutcome, TaggedError>;

/// Upstream `NavigationResult` value
/// (`{ navigation: OperationResultRecord; run?: ... }`).
#[derive(Debug, Clone, PartialEq)]
pub struct NavigationOutcome {
    pub navigation: OperationResultRecord,
    pub run: Option<RunOutcome>,
}

/// Upstream `NavigationResult`
/// (`Result<..., LaneBusy | InvalidNavigation | UnknownTarget | Closed>`).
pub type NavigationResult = std::result::Result<NavigationOutcome, TaggedError>;

/// Upstream `ResumeResult`
/// (`Result<OperationResultRecord | SuspendedRun, NothingToResume | Closed>`).
pub type ResumeResult = std::result::Result<RunOutcome, TaggedError>;

/// Upstream `QueueResult` value (`{ entryId: string }`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueOutcome {
    pub entry_id: String,
}

/// Upstream `QueueResult` (`Result<{ entryId }, InvalidMessage | Closed>`).
pub type QueueResult = std::result::Result<QueueOutcome, TaggedError>;

/// Upstream `CancelQueuedResult` value union
/// (`"cancelled" | "already_consumed" | "not_found"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelQueuedOutcome {
    Cancelled,
    AlreadyConsumed,
    NotFound,
}

/// Upstream `CancelQueuedResult` (`Result<..., Closed>`).
pub type CancelQueuedResult = std::result::Result<CancelQueuedOutcome, TaggedError>;

/// Upstream `AbortResult` value
/// (`{ operationId, steer: AgentMessage[], followUp: AgentMessage[] }`).
#[derive(Debug, Clone, PartialEq)]
pub struct AbortOutcome {
    pub operation_id: String,
    pub steer: Vec<AgentMessage>,
    pub follow_up: Vec<AgentMessage>,
}

/// Upstream `AbortResult`
/// (`Result<..., NoActiveOperation | Closed>`).
pub type AbortResult = std::result::Result<AbortOutcome, TaggedError>;

/// Upstream `AbortRequestResult` value
/// (`{ operationId, newlyRequested: boolean, steer, followUp }`).
#[derive(Debug, Clone, PartialEq)]
pub struct AbortRequestOutcome {
    pub operation_id: String,
    pub newly_requested: bool,
    pub steer: Vec<AgentMessage>,
    pub follow_up: Vec<AgentMessage>,
}

/// Upstream `AbortRequestResult`
/// (`Result<..., OperationMismatch | Closed>`).
pub type AbortRequestResult = std::result::Result<AbortRequestOutcome, TaggedError>;

/// Upstream `RecordUsageResult` value (`{ usageId: string }`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordUsageOutcome {
    pub usage_id: String,
}

/// Upstream `RecordUsageResult` (`Result<{ usageId }, Closed>`).
pub type RecordUsageResult = std::result::Result<RecordUsageOutcome, TaggedError>;

/// Upstream `OperationAdmissionResult`
/// (`Result<OperationAdmission, LaneBusy | InvalidMessage | UnknownSkill |
/// UnknownTemplate | NothingToCompact | InvalidNavigation | UnknownTarget |
/// Closed>`); the error channel is [`TaggedError`].
pub type OperationAdmissionResult = std::result::Result<OperationAdmission, TaggedError>;

/// Upstream `DriveResult`
/// (`Result<DriveOutcome, OperationMismatch | Closed>`).
pub type DriveResult = std::result::Result<DriveOutcome, TaggedError>;

// ---------------------------------------------------------------------------
// Operation shapes (upstream agent-harness.ts:105-155)
// ---------------------------------------------------------------------------

/// Upstream `NavigateOptions` (`agent-harness.ts:105-109`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NavigateOptions {
    pub summarize: Option<bool>,
    pub label: Option<String>,
    pub custom_instructions: Option<String>,
}

/// Upstream `compact(options)` argument
/// (`{ customInstructions?: string } | undefined`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompactOptions {
    pub custom_instructions: Option<String>,
}

/// Upstream `recordUsage` options
/// (`{ entryId?: string; details?: JsonValue } | undefined`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecordUsageOptions {
    pub entry_id: Option<String>,
    pub details: Option<serde_json::Value>,
}

/// Upstream `AgentMessage` prompt input
/// (`prompt: AgentMessage | AgentMessage[]`).
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // One carries the full message, matching the upstream union's unboxed shape.
pub enum PromptMessages {
    One(AgentMessage),
    Many(Vec<AgentMessage>),
}

/// Upstream `OperationRequest` (`agent-harness.ts:111-117`), the discriminated
/// admission vocabulary (`kind` tag). The upstream two `prompt` arms (string
/// with images vs message input without) map to [`PromptPayload`].
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // Prompt carries the message payload; the upstream union is similarly unboxed.
pub enum OperationRequest {
    Prompt {
        operation_id: Option<String>,
        payload: PromptPayload,
    },
    Skill {
        operation_id: Option<String>,
        name: String,
        additional_instructions: Option<String>,
    },
    PromptTemplate {
        operation_id: Option<String>,
        name: String,
        args: Option<Vec<String>>,
    },
    Compaction {
        operation_id: Option<String>,
        custom_instructions: Option<String>,
    },
    Navigation {
        operation_id: Option<String>,
        target_id: Option<String>,
        options: Option<NavigateOptions>,
    },
}

/// The upstream `prompt` arms: string-plus-images or agent message(s).
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // Messages carries the full payload like the upstream union.
pub enum PromptPayload {
    Text {
        prompt: String,
        images: Option<Vec<ImageContent>>,
    },
    Messages(PromptMessages),
}

impl PromptPayload {
    /// The upstream `kind` discriminator of the request this payload belongs
    /// to (`"prompt"`).
    pub fn kind(&self) -> &'static str {
        "prompt"
    }
}

impl OperationRequest {
    /// The upstream `kind` literal.
    pub fn kind(&self) -> &'static str {
        match self {
            OperationRequest::Prompt { .. } => "prompt",
            OperationRequest::Skill { .. } => "skill",
            OperationRequest::PromptTemplate { .. } => "prompt_template",
            OperationRequest::Compaction { .. } => "compaction",
            OperationRequest::Navigation { .. } => "navigation",
        }
    }

    /// The optional caller-supplied operation id.
    pub fn operation_id(&self) -> Option<&str> {
        match self {
            OperationRequest::Prompt { operation_id, .. }
            | OperationRequest::Skill { operation_id, .. }
            | OperationRequest::PromptTemplate { operation_id, .. }
            | OperationRequest::Compaction { operation_id, .. }
            | OperationRequest::Navigation { operation_id, .. } => operation_id.as_deref(),
        }
    }
}

/// Upstream `steer`/`followUp`/`nextRun` message input
/// (`string | AgentMessage`).
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // Message carries the full message, matching the upstream unboxed union.
pub enum QueueInput {
    Text(String),
    Message(AgentMessage),
}

// ---------------------------------------------------------------------------
// Observation shapes (upstream agent-harness.ts:147-253)
// ---------------------------------------------------------------------------

/// Upstream `OperationStatus` (`"running" | "open" | "aborting"`) is the
/// re-exported [`OperationStatus`]; upstream `CurrentOperationInfo`
/// (`agent-harness.ts:149-155`).
#[derive(Debug, Clone, PartialEq)]
pub struct CurrentOperationInfo {
    pub id: String,
    pub kind: OperationKind,
    pub started_at: i64,
    pub status: OperationStatus,
    pub captured_model: Option<ModelIdentity>,
}

/// Upstream `LaneExecutionInfo` (`agent-harness.ts:157-163`).
#[derive(Debug, Clone, PartialEq)]
pub struct LaneExecutionInfo {
    pub lane: String,
    pub tip_id: Option<String>,
    pub configured_model: ModelIdentity,
    pub current: Option<CurrentOperationInfo>,
    pub last_operation_id: Option<String>,
}

/// Upstream `LaneInfo` (`agent-harness.ts:188-192`).
#[derive(Debug, Clone, PartialEq)]
pub struct LaneInfo {
    pub name: String,
    pub tip_id: Option<String>,
    pub operation: Option<CurrentOperationInfo>,
}

/// Upstream `OpenOperation` (`agent-harness.ts:211-217`): one operation found
/// open at attach time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenOperation {
    pub lane: String,
    pub operation_id: String,
    pub kind: OperationKind,
    pub started_at: i64,
    /// Upstream `aborting?: true`; absent unless cancel was requested.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub aborting: bool,
}

/// Upstream `SessionSnapshot` (`agent-harness.ts:250-253`).
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSnapshot {
    pub lanes: Vec<LaneInfo>,
    pub faulted: bool,
}

// ---------------------------------------------------------------------------
// HarnessEvent (upstream agent-harness.ts:255-397): the complete public union
// ---------------------------------------------------------------------------

/// The `value` payload of `value_update` events
/// (upstream `agent-harness.ts:333-336`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "value",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ValueUpdatePayload {
    #[serde(rename = "session_name")]
    SessionName {
        /// Upstream `string | undefined` — `None` serializes omitted, matching
        /// the deleted state.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    #[serde(rename = "entry_label")]
    EntryLabel {
        target_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
}

/// The `property` payload of `config_update` events
/// (upstream `ConfigEventPayload`, `agent-harness.ts:337-355`). Lane-owned
/// properties (`model`/`thinkingLevel`/`activeTools`) arrive with a `lane`
/// envelope on [`HarnessEvent::ConfigUpdate`]; global properties arrive
/// without one.
///
/// `retryPolicy` value pairs carry a serde shim ([`retry_policy_json`]):
/// `ai::retry::RetryPolicy` does not implement serde upstream-free, and its
/// wire object is `{ enabled, maxRetries, baseDelayMs, maxAgentDelayMs }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "property",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PublicConfigProperty {
    Model {
        value: LaneModel,
        /// Upstream `previous: unknown`.
        previous: serde_json::Value,
    },
    ThinkingLevel {
        value: ThinkingLevel,
        previous: ThinkingLevel,
    },
    ActiveTools {
        value: Vec<String>,
        previous: Vec<String>,
    },
    /// Upstream `{ property: "tools" | "resources" }` — no value payload.
    Tools,
    Resources,
    StreamOptions {
        value: AgentHarnessStreamOptions,
        previous: AgentHarnessStreamOptions,
    },
    RetryPolicy {
        #[serde(with = "retry_policy_json")]
        value: crate::ai::retry::RetryPolicy,
        #[serde(with = "retry_policy_json")]
        previous: crate::ai::retry::RetryPolicy,
    },
    CompactionSettings {
        value: crate::agent_core::harness::config::CompactionSettings,
        previous: crate::agent_core::harness::config::CompactionSettings,
    },
    SteeringMode {
        value: QueueMode,
        previous: QueueMode,
    },
    FollowUpMode {
        value: QueueMode,
        previous: QueueMode,
    },
}

/// The `{ enabled, maxRetries, baseDelayMs, maxAgentDelayMs }` wire shim for
/// `ai::retry::RetryPolicy` (the field set and casing of the upstream
/// `RetryPolicy` object).
mod retry_policy_json {
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Wire {
        enabled: bool,
        max_retries: u32,
        base_delay_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_agent_delay_ms: Option<u64>,
    }

    pub fn serialize<S: serde::Serializer>(
        policy: &crate::ai::retry::RetryPolicy,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        Wire {
            enabled: policy.enabled,
            max_retries: policy.max_retries,
            base_delay_ms: policy.base_delay_ms,
            max_agent_delay_ms: policy.max_agent_delay_ms,
        }
        .serialize(serializer)
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<crate::ai::retry::RetryPolicy, D::Error> {
        let wire = Wire::deserialize(deserializer)?;
        Ok(crate::ai::retry::RetryPolicy {
            enabled: wire.enabled,
            max_retries: wire.max_retries,
            base_delay_ms: wire.base_delay_ms,
            max_agent_delay_ms: wire.max_agent_delay_ms,
        })
    }
}

/// The `handler_error` `kind` union (upstream `agent-harness.ts:265-268`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandlerErrorKind {
    Hook,
    Event,
}

/// The complete public `HarnessEvent` union (upstream `agent-harness.ts:388-
/// 397`): every lane-scoped payload carries the `lane` envelope, plus
/// `recovery: true` on exactly the variants the upstream recovery paths stamp
/// (the landed [`runtime::events::HarnessEvent`] declarations). The special
/// payloads (`fault`, `value_update`, global `config_update`) and `usage`
/// carry no recovery flag, `handler_error` optionally carries a lane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
#[allow(clippy::large_enum_variant)] // Owns full messages/entries like the upstream union and transcript.rs.
pub enum HarnessEvent {
    // --- lane-scoped payloads ------------------------------------------------
    RunStart {
        run_id: String,
        started_at: i64,
        lane: String,
    },
    RunResume {
        lane: String,
        run_id: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        recovery: bool,
    },
    RunSuspend {
        lane: String,
        run_id: String,
        deferred: DeferredHandle,
        poll: u64,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        recovery: bool,
    },
    OperationAbort {
        operation_id: String,
        steer: Vec<AgentMessage>,
        follow_up: Vec<AgentMessage>,
        lane: String,
    },
    RunEnd {
        lane: String,
        run_id: String,
        status: crate::agent_core::harness::runtime::events::RunEndStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<crate::agent_core::harness::session::OperationError>,
        from_tip_id: Option<String>,
        tip_id: Option<String>,
        ended_at: i64,
    },
    TurnStart {
        lane: String,
        run_id: String,
        turn_id: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        recovery: bool,
    },
    TurnEnd {
        lane: String,
        run_id: String,
        turn_id: String,
        message: AgentMessage,
        tool_results: Vec<crate::ai::types::message::ToolResultMessage>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        recovery: bool,
    },
    RetryScheduled {
        lane: String,
        run_id: String,
        step: String,
        attempt: u32,
        max_attempts: u32,
        delay_ms: i64,
        not_before: i64,
        error_message: String,
    },
    RetryStart {
        lane: String,
        run_id: String,
        step: String,
        attempt: u32,
    },
    RetryEnd {
        lane: String,
        run_id: String,
        step: String,
        attempt: u32,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        final_error: Option<String>,
    },
    MessageStart {
        lane: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        recovery: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
        message: AgentMessage,
    },
    MessageUpdate {
        lane: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        recovery: bool,
        run_id: String,
        message: AgentMessage,
        event: crate::ai::types::events::AssistantMessageEvent,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame: Option<crate::ai::frame::AssistantMessageFrame>,
    },
    MessageEnd {
        lane: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        recovery: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
        message: AgentMessage,
        entry_id: String,
    },
    ToolStart {
        lane: String,
        run_id: String,
        turn_id: String,
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        recovery: bool,
    },
    ToolUpdate {
        lane: String,
        run_id: String,
        turn_id: String,
        tool_call_id: String,
        tool_name: String,
        partial_result: crate::agent_core::types::AgentToolResult,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        recovery: bool,
    },
    ToolEnd {
        lane: String,
        run_id: String,
        turn_id: String,
        tool_call_id: String,
        tool_name: String,
        result: crate::agent_core::types::AgentToolResult,
        is_error: bool,
        terminate: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        recovery: bool,
    },
    EntryAdded {
        lane: String,
        entry: crate::agent_core::harness::session::Entry,
    },
    QueueUpdate {
        lane: String,
        queues: Vec<LaneQueuedItem>,
    },
    CompactionStart {
        lane: String,
        run_id: String,
        reason: crate::agent_core::harness::runtime::durable::SummaryReason,
        started_at: i64,
    },
    CompactionEnd {
        lane: String,
        run_id: String,
        reason: crate::agent_core::harness::runtime::durable::SummaryReason,
        status: crate::agent_core::harness::runtime::events::CompactionEndStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entry_id: Option<String>,
        ended_at: i64,
    },
    NavigationStart {
        lane: String,
        run_id: String,
        target_id: Option<String>,
        started_at: i64,
    },
    NavigationEnd {
        lane: String,
        run_id: String,
        status: crate::agent_core::harness::runtime::events::RunEndStatus,
        from_tip_id: Option<String>,
        tip_id: Option<String>,
        ended_at: i64,
    },
    LaneCreated {
        lane: String,
        /// Upstream `at: string | null` — serializes `null` for a fresh lane.
        at: Option<String>,
    },
    Usage {
        lane: String,
        row: crate::agent_core::harness::session::UsageRow,
        totals: Usage,
    },
    // --- special payloads ----------------------------------------------------
    /// Upstream `{ type: "fault"; code: string; message: string }` — no lane
    /// envelope.
    Fault { code: String, message: String },
    /// Upstream `value_update` — no lane envelope.
    ValueUpdate {
        #[serde(flatten)]
        update: ValueUpdatePayload,
    },
    /// Upstream `config_update`: lane-owned properties carry `lane` (and may
    /// carry `recovery`), global properties carry neither.
    ConfigUpdate {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lane: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        recovery: bool,
        #[serde(flatten)]
        property: PublicConfigProperty,
    },
    /// Upstream `handler_error`: either lane-scoped or global.
    HandlerError {
        kind: HandlerErrorKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hook: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event: Option<String>,
        error: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stack: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lane: Option<String>,
    },
}

impl HarnessEvent {
    /// Upstream `HarnessEventType` (`agent-harness.ts:413`): the wire
    /// `type` discriminator.
    pub fn event_type(&self) -> &'static str {
        match self {
            HarnessEvent::RunStart { .. } => "run_start",
            HarnessEvent::RunResume { .. } => "run_resume",
            HarnessEvent::RunSuspend { .. } => "run_suspend",
            HarnessEvent::OperationAbort { .. } => "operation_abort",
            HarnessEvent::RunEnd { .. } => "run_end",
            HarnessEvent::TurnStart { .. } => "turn_start",
            HarnessEvent::TurnEnd { .. } => "turn_end",
            HarnessEvent::RetryScheduled { .. } => "retry_scheduled",
            HarnessEvent::RetryStart { .. } => "retry_start",
            HarnessEvent::RetryEnd { .. } => "retry_end",
            HarnessEvent::MessageStart { .. } => "message_start",
            HarnessEvent::MessageUpdate { .. } => "message_update",
            HarnessEvent::MessageEnd { .. } => "message_end",
            HarnessEvent::ToolStart { .. } => "tool_start",
            HarnessEvent::ToolUpdate { .. } => "tool_update",
            HarnessEvent::ToolEnd { .. } => "tool_end",
            HarnessEvent::EntryAdded { .. } => "entry_added",
            HarnessEvent::QueueUpdate { .. } => "queue_update",
            HarnessEvent::CompactionStart { .. } => "compaction_start",
            HarnessEvent::CompactionEnd { .. } => "compaction_end",
            HarnessEvent::NavigationStart { .. } => "navigation_start",
            HarnessEvent::NavigationEnd { .. } => "navigation_end",
            HarnessEvent::LaneCreated { .. } => "lane_created",
            HarnessEvent::Usage { .. } => "usage",
            HarnessEvent::Fault { .. } => "fault",
            HarnessEvent::ValueUpdate { .. } => "value_update",
            HarnessEvent::ConfigUpdate { .. } => "config_update",
            HarnessEvent::HandlerError { .. } => "handler_error",
        }
    }
}

impl BusEvent for HarnessEvent {
    fn event_type(&self) -> &str {
        HarnessEvent::event_type(self)
    }

    fn lane(&self) -> Option<&str> {
        match self {
            HarnessEvent::RunStart { lane, .. }
            | HarnessEvent::RunResume { lane, .. }
            | HarnessEvent::RunSuspend { lane, .. }
            | HarnessEvent::OperationAbort { lane, .. }
            | HarnessEvent::RunEnd { lane, .. }
            | HarnessEvent::TurnStart { lane, .. }
            | HarnessEvent::TurnEnd { lane, .. }
            | HarnessEvent::RetryScheduled { lane, .. }
            | HarnessEvent::RetryStart { lane, .. }
            | HarnessEvent::RetryEnd { lane, .. }
            | HarnessEvent::MessageStart { lane, .. }
            | HarnessEvent::MessageUpdate { lane, .. }
            | HarnessEvent::MessageEnd { lane, .. }
            | HarnessEvent::ToolStart { lane, .. }
            | HarnessEvent::ToolUpdate { lane, .. }
            | HarnessEvent::ToolEnd { lane, .. }
            | HarnessEvent::EntryAdded { lane, .. }
            | HarnessEvent::QueueUpdate { lane, .. }
            | HarnessEvent::CompactionStart { lane, .. }
            | HarnessEvent::CompactionEnd { lane, .. }
            | HarnessEvent::NavigationStart { lane, .. }
            | HarnessEvent::NavigationEnd { lane, .. }
            | HarnessEvent::LaneCreated { lane, .. }
            | HarnessEvent::Usage { lane, .. } => Some(lane),
            HarnessEvent::ConfigUpdate { lane, .. } => lane.as_deref(),
            HarnessEvent::HandlerError { lane, .. } => lane.as_deref(),
            HarnessEvent::Fault { .. } | HarnessEvent::ValueUpdate { .. } => None,
        }
    }

    fn handler_error(event_type: String, error: String, lane: Option<String>) -> Self {
        HarnessEvent::HandlerError {
            kind: HandlerErrorKind::Event,
            hook: None,
            event: Some(event_type),
            error,
            stack: None,
            lane,
        }
    }
}

impl From<crate::agent_core::harness::runtime::events::HarnessEvent> for HarnessEvent {
    /// Lift a lane-scoped runtime event into the public union (envelopes are
    /// carried verbatim; recovery flags pass through).
    fn from(event: crate::agent_core::harness::runtime::events::HarnessEvent) -> Self {
        use crate::agent_core::harness::runtime::events::HarnessEvent as RuntimeEvent;
        match event {
            RuntimeEvent::RunStart {
                run_id,
                started_at,
                lane,
            } => HarnessEvent::RunStart {
                lane,
                run_id,
                started_at,
            },
            RuntimeEvent::MessageStart {
                lane,
                recovery,
                run_id,
                message,
            } => HarnessEvent::MessageStart {
                lane,
                recovery,
                run_id,
                message,
            },
            RuntimeEvent::MessageEnd {
                lane,
                recovery,
                run_id,
                message,
                entry_id,
            } => HarnessEvent::MessageEnd {
                lane,
                recovery,
                run_id,
                message,
                entry_id,
            },
            RuntimeEvent::MessageUpdate {
                lane,
                recovery,
                run_id,
                message,
                event,
                frame,
            } => HarnessEvent::MessageUpdate {
                lane,
                recovery,
                run_id,
                message,
                event,
                frame,
            },
            RuntimeEvent::EntryAdded { lane, entry } => HarnessEvent::EntryAdded { lane, entry },
            RuntimeEvent::QueueUpdate { lane, queues } => {
                HarnessEvent::QueueUpdate { lane, queues }
            }
            RuntimeEvent::TurnEnd {
                lane,
                run_id,
                turn_id,
                message,
                tool_results,
                recovery,
            } => HarnessEvent::TurnEnd {
                lane,
                run_id,
                turn_id,
                message,
                tool_results,
                recovery,
            },
            RuntimeEvent::Usage { lane, row, totals } => HarnessEvent::Usage { lane, row, totals },
            RuntimeEvent::RetryScheduled {
                lane,
                run_id,
                step,
                attempt,
                max_attempts,
                delay_ms,
                not_before,
                error_message,
            } => HarnessEvent::RetryScheduled {
                lane,
                run_id,
                step,
                attempt,
                max_attempts,
                delay_ms,
                not_before,
                error_message,
            },
            RuntimeEvent::RetryEnd {
                lane,
                run_id,
                step,
                attempt,
                success,
                final_error,
            } => HarnessEvent::RetryEnd {
                lane,
                run_id,
                step,
                attempt,
                success,
                final_error,
            },
            RuntimeEvent::RunSuspend {
                lane,
                run_id,
                deferred,
                poll,
                recovery,
            } => HarnessEvent::RunSuspend {
                lane,
                run_id,
                deferred,
                poll,
                recovery,
            },
            RuntimeEvent::CompactionStart {
                lane,
                run_id,
                reason,
                started_at,
            } => HarnessEvent::CompactionStart {
                lane,
                run_id,
                reason,
                started_at,
            },
            RuntimeEvent::OperationAbort {
                operation_id,
                steer,
                follow_up,
                lane,
            } => HarnessEvent::OperationAbort {
                operation_id,
                steer,
                follow_up,
                lane,
            },
            RuntimeEvent::NavigationStart {
                lane,
                run_id,
                target_id,
                started_at,
            } => HarnessEvent::NavigationStart {
                lane,
                run_id,
                target_id,
                started_at,
            },
            RuntimeEvent::ToolStart {
                lane,
                run_id,
                turn_id,
                tool_call_id,
                tool_name,
                args,
                recovery,
            } => HarnessEvent::ToolStart {
                lane,
                run_id,
                turn_id,
                tool_call_id,
                tool_name,
                args,
                recovery,
            },
            RuntimeEvent::ToolUpdate {
                lane,
                run_id,
                turn_id,
                tool_call_id,
                tool_name,
                partial_result,
                recovery,
            } => HarnessEvent::ToolUpdate {
                lane,
                run_id,
                turn_id,
                tool_call_id,
                tool_name,
                partial_result,
                recovery,
            },
            RuntimeEvent::ToolEnd {
                lane,
                run_id,
                turn_id,
                tool_call_id,
                tool_name,
                result,
                is_error,
                terminate,
                recovery,
            } => HarnessEvent::ToolEnd {
                lane,
                run_id,
                turn_id,
                tool_call_id,
                tool_name,
                result,
                is_error,
                terminate,
                recovery,
            },
            RuntimeEvent::RunResume {
                lane,
                run_id,
                recovery,
            } => HarnessEvent::RunResume {
                lane,
                run_id,
                recovery,
            },
            RuntimeEvent::TurnStart {
                lane,
                run_id,
                turn_id,
                recovery,
            } => HarnessEvent::TurnStart {
                lane,
                run_id,
                turn_id,
                recovery,
            },
            RuntimeEvent::RetryStart {
                lane,
                run_id,
                step,
                attempt,
            } => HarnessEvent::RetryStart {
                lane,
                run_id,
                step,
                attempt,
            },
            RuntimeEvent::CompactionEnd {
                lane,
                run_id,
                reason,
                status,
                entry_id,
                ended_at,
            } => HarnessEvent::CompactionEnd {
                lane,
                run_id,
                reason,
                status,
                entry_id,
                ended_at,
            },
            RuntimeEvent::NavigationEnd {
                lane,
                run_id,
                status,
                from_tip_id,
                tip_id,
                ended_at,
            } => HarnessEvent::NavigationEnd {
                lane,
                run_id,
                status,
                from_tip_id,
                tip_id,
                ended_at,
            },
            RuntimeEvent::RunEnd {
                lane,
                run_id,
                status,
                error,
                from_tip_id,
                tip_id,
                ended_at,
            } => HarnessEvent::RunEnd {
                lane,
                run_id,
                status,
                error,
                from_tip_id,
                tip_id,
                ended_at,
            },
            RuntimeEvent::ConfigUpdate { lane, property } => HarnessEvent::ConfigUpdate {
                lane: Some(lane),
                recovery: false,
                property: property.into(),
            },
        }
    }
}

impl From<crate::agent_core::harness::runtime::events::ConfigUpdateProperty>
    for PublicConfigProperty
{
    fn from(property: crate::agent_core::harness::runtime::events::ConfigUpdateProperty) -> Self {
        use crate::agent_core::harness::runtime::events::ConfigUpdateProperty as RuntimeProperty;
        match property {
            RuntimeProperty::Model { value, previous } => {
                PublicConfigProperty::Model { value, previous }
            }
            RuntimeProperty::ThinkingLevel { value, previous } => {
                PublicConfigProperty::ThinkingLevel { value, previous }
            }
            RuntimeProperty::ActiveTools { value, previous } => {
                PublicConfigProperty::ActiveTools { value, previous }
            }
        }
    }
}

impl From<crate::agent_core::harness::runtime::drive::tools::ToolEvent> for HarnessEvent {
    /// Lift a tool event (the drive tool slice's delivery vocabulary) into
    /// the public union; field order matches the upstream `tool_*` literals.
    fn from(event: crate::agent_core::harness::runtime::drive::tools::ToolEvent) -> Self {
        use crate::agent_core::harness::runtime::drive::tools::ToolEvent;
        match event {
            ToolEvent::ToolStart {
                lane,
                run_id,
                turn_id,
                tool_call_id,
                tool_name,
                args,
                recovery,
            } => HarnessEvent::ToolStart {
                lane,
                run_id,
                turn_id,
                tool_call_id,
                tool_name,
                args,
                recovery,
            },
            ToolEvent::ToolUpdate {
                lane,
                run_id,
                turn_id,
                tool_call_id,
                tool_name,
                partial_result,
                recovery,
            } => HarnessEvent::ToolUpdate {
                lane,
                run_id,
                turn_id,
                tool_call_id,
                tool_name,
                partial_result,
                recovery,
            },
            ToolEvent::ToolEnd {
                lane,
                run_id,
                turn_id,
                tool_call_id,
                tool_name,
                result,
                is_error,
                terminate,
                recovery,
            } => HarnessEvent::ToolEnd {
                lane,
                run_id,
                turn_id,
                tool_call_id,
                tool_name,
                result,
                is_error,
                terminate,
                recovery,
            },
        }
    }
}

/// Upstream `EventListener` (`agent-harness.ts:414-417`): receives the event
/// snapshot and the chord context.
pub type EventListener =
    Arc<dyn Fn(Arc<HarnessEvent>, Context) -> BoxFuture<'static, ()> + Send + Sync>;

// ---------------------------------------------------------------------------
// HookMap surface (upstream agent-harness.ts:430-514): the per-hook event and
// result types are the landed hooks.rs ports (HookEvent/HookResult); the
// registry is HookRegistry.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Options and interfaces (upstream agent-harness.ts:516-619)
// ---------------------------------------------------------------------------

/// Upstream `AgentHarnessOptions` (`agent-harness.ts:518-536`).
///
/// Seams against not-yet-landed factories: `system_prompt` carries the string
/// form only (the [`runtime::lane::RuntimeConfig`] precedent — the callable
/// form lands with the tools factories).
pub struct AgentHarnessOptions<TContext: Clone + Send + Sync + 'static> {
    pub session: Arc<crate::agent_core::harness::session::StorageBackedSession>,
    pub models: crate::ai::models::Models,
    pub model: crate::ai::types::model::Model,
    pub thinking_level: Option<ThinkingLevel>,
    pub active_tool_names: Option<Vec<String>>,
    pub tools: Option<Vec<AgentHarnessTool<TContext>>>,
    /// Upstream `toolContext: TContext | ((context) => TContext | Promise)`:
    /// the resolved value or provider.
    pub tool_context:
        Option<crate::agent_core::harness::runtime::drive::tools::ToolContextSource<TContext>>,
    pub system_prompt: Option<String>,
    pub resources: Option<Resources>,
    pub stream_options: Option<AgentHarnessStreamOptions>,
    pub retry: Option<crate::ai::retry::RetryPolicy>,
    pub compaction: Option<crate::agent_core::harness::config::CompactionSettings>,
    pub steering_mode: Option<QueueMode>,
    pub follow_up_mode: Option<QueueMode>,
    pub tool_execution: Option<ToolExecutionMode>,
    /// Upstream `toProviderMessages`; `None` uses the default
    /// AgentMessage-to-provider conversion (`convertToLlm`).
    pub to_provider_messages:
        Option<Arc<crate::agent_core::harness::execution::assistant::ToProviderMessagesFn>>,
    pub entry_projectors: Option<BTreeMap<String, EntryProjector>>,
}

/// Upstream `AgentLane` (`agent-harness.ts:538-580`): the public contract of
/// one configured lane. The concrete runtime [`Lane`] implements the landed
/// subset; members whose upstream behavior lives in the unported
/// `runtime/lane.ts` slices return [`SliceNotImplemented`].
pub trait AgentLane: Send + Sync {
    /// Upstream readonly `name`.
    fn name(&self) -> &str;

    fn get_tip_id<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<Option<String>>>;
    fn find_entries<'a>(
        &'a self,
        query: Option<&crate::agent_core::harness::session::BranchScan>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<crate::agent_core::harness::session::Entry>>>;
    fn find_entry<'a>(
        &'a self,
        query: Option<&crate::agent_core::harness::session::BranchScan>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<crate::agent_core::harness::session::Entry>>>;
    fn append_message<'a>(
        &'a self,
        message: AgentMessage,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<String>>;
    fn append_custom_entry<'a>(
        &'a self,
        custom_type: String,
        data: Option<serde_json::Value>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<String>>;
    fn get_result<'a>(
        &'a self,
        operation_id: &str,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<OperationResultRecord>>>;
    /// Upstream `accept`: the inner `Err` carries the expected admission
    /// rejection; the outer error is a fault/sealed lane.
    fn accept<'a>(
        &'a self,
        request: &OperationRequest,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<OperationAdmissionResult>>;
    fn drive<'a>(
        &'a self,
        options: &DriveOptions,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<DriveResult>>;
    fn request_abort<'a>(
        &'a self,
        operation_id: &str,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<AbortRequestResult>>;
    fn inspect_execution<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<LaneExecutionInfo>>;
    fn prompt<'a>(
        &'a self,
        text: &str,
        images: Option<Vec<ImageContent>>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<RunResult>>;
    fn prompt_messages<'a>(
        &'a self,
        messages: PromptMessages,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<RunResult>>;
    fn skill<'a>(
        &'a self,
        name: &str,
        additional_instructions: Option<String>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<RunResult>>;
    fn prompt_from_template<'a>(
        &'a self,
        name: &str,
        args: Option<Vec<String>>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<RunResult>>;
    fn compact<'a>(
        &'a self,
        options: Option<&CompactOptions>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<CompactionResult>>;
    fn navigate_tree<'a>(
        &'a self,
        target_id: Option<String>,
        options: Option<&NavigateOptions>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<NavigationResult>>;
    fn resume<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<ResumeResult>>;
    fn abort<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<AbortResult>>;
    fn steer<'a>(
        &'a self,
        message: QueueInput,
        images: Option<Vec<ImageContent>>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<QueueResult>>;
    fn follow_up<'a>(
        &'a self,
        message: QueueInput,
        images: Option<Vec<ImageContent>>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<QueueResult>>;
    fn next_run<'a>(
        &'a self,
        message: QueueInput,
        images: Option<Vec<ImageContent>>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<QueueResult>>;
    fn cancel_queued<'a>(
        &'a self,
        entry_id: &str,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<CancelQueuedResult>>;
    fn record_usage<'a>(
        &'a self,
        usage: Usage,
        options: Option<&RecordUsageOptions>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<RecordUsageResult>>;
    fn wait_for_idle<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<()>>;
    fn run_when_idle<'a>(
        &'a self,
        callback: IdleCallback,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    fn get_model<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<crate::ai::types::model::Model>>>;
    fn set_model<'a>(
        &'a self,
        model: ModelIdentity,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    fn get_thinking_level<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<ThinkingLevel>>;
    fn set_thinking_level<'a>(
        &'a self,
        level: ThinkingLevel,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    fn get_active_tools<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<String>>>;
    fn set_active_tools<'a>(
        &'a self,
        names: Vec<String>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    fn watch<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<HarnessWatchHandle<LaneSnapshot>>>;
}

/// Upstream `AgentLane.runWhenIdle` callback
/// (`(context: Context) => void | Promise<void>`); a panic in the callback is
/// the port's throwing form.
pub type IdleCallback =
    Arc<dyn Fn(Context) -> BoxFuture<'static, anyhow::Result<()>> + Send + Sync>;

/// Upstream `AcquireLaneOptions` (`agent-harness.ts:582-584`).
///
/// Upstream `createAt?: string | null` — absent and `null` both default the
/// fresh-lane tip to `null` inside `lane()`, so `Option<String>` preserves the
/// observable behavior.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AcquireLaneOptions {
    pub create_at: Option<String>,
}

/// Upstream `AgentHarness` (`agent-harness.ts:586-612`). The runtime
/// implementation is [`harness_impl::Harness`], constructed with
/// [`create_agent_harness`] (upstream `AgentHarness.create`).
pub trait AgentHarness<TContext: Clone + Send + Sync + 'static>: Send + Sync {
    /// Upstream `lane(name)` / `lane(name, options)` overloads: one method
    /// with default options.
    fn lane<'a>(
        &'a self,
        name: &str,
        options: AcquireLaneOptions,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Arc<crate::agent_core::harness::runtime::lane::Lane>>>;
    fn lanes<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<Vec<LaneInfo>>>;
    fn get_name<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<Option<String>>>;
    fn set_name<'a>(
        &'a self,
        name: Option<String>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    fn get_label<'a>(
        &'a self,
        target_id: &str,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<String>>>;
    fn set_label<'a>(
        &'a self,
        target_id: &str,
        label: Option<String>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    fn get_tools<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<AgentHarnessTool<TContext>>>>;
    fn set_tools<'a>(
        &'a self,
        tools: Vec<AgentHarnessTool<TContext>>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    fn get_resources<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<Resources>>;
    fn set_resources<'a>(
        &'a self,
        resources: Resources,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    fn get_stream_options<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<AgentHarnessStreamOptions>>;
    fn set_stream_options<'a>(
        &'a self,
        options: AgentHarnessStreamOptions,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    fn get_retry_policy<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<crate::ai::retry::RetryPolicy>>;
    fn set_retry_policy<'a>(
        &'a self,
        policy: crate::ai::retry::RetryPolicy,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    fn get_compaction_settings<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<crate::agent_core::harness::config::CompactionSettings>>;
    fn set_compaction_settings<'a>(
        &'a self,
        settings: crate::agent_core::harness::config::CompactionSettings,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    fn get_steering_mode<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<QueueMode>>;
    fn set_steering_mode<'a>(
        &'a self,
        mode: QueueMode,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    fn get_follow_up_mode<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<QueueMode>>;
    fn set_follow_up_mode<'a>(
        &'a self,
        mode: QueueMode,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>>;
    /// Upstream `watchSession` — throws `SliceNotImplemented` upstream
    /// (`runtime/harness.ts:305-307`); the port returns the same error.
    fn watch_session<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<HarnessWatchHandle<SessionSnapshot>>>;
    /// Upstream readonly `hooks`.
    fn hooks(&self) -> &HookRegistry;
    /// Upstream readonly `events`.
    fn events(&self) -> &Events;
    fn close<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<()>>;
}

// ---------------------------------------------------------------------------
// impl AgentLane for the runtime Lane
// ---------------------------------------------------------------------------

/// Bridge the public [`OperationRequest`] onto the runtime lane's
/// `accept_with_id`.
fn accept_runtime<'a>(
    lane: &'a crate::agent_core::harness::runtime::lane::Lane,
    request: crate::agent_core::harness::runtime::lane::OperationRequest,
    requested_id: Option<String>,
    context: Context,
) -> BoxFuture<'a, anyhow::Result<OperationAdmissionResult>> {
    Box::pin(async move {
        crate::agent_core::harness::runtime::lane::Lane::accept_with_id(
            lane,
            &request,
            requested_id,
            context,
        )
        .await
    })
}

fn operation_kind_of(
    intent: &crate::agent_core::harness::runtime::durable::OperationIntent,
) -> OperationKind {
    match intent {
        crate::agent_core::harness::runtime::durable::OperationIntent::Run { .. } => {
            OperationKind::Run
        }
        crate::agent_core::harness::runtime::durable::OperationIntent::Compaction { .. } => {
            OperationKind::Compaction
        }
        crate::agent_core::harness::runtime::durable::OperationIntent::Navigation { .. } => {
            OperationKind::Navigation
        }
    }
}

/// Upstream `capturedModel` (`runtime/lane.ts:199-216`): the model identity
/// captured by the operation's current phase.
fn captured_model(
    state: &crate::agent_core::harness::runtime::durable::OperationState,
) -> Option<ModelIdentity> {
    use crate::agent_core::harness::runtime::durable::OperationPhase;
    match &state.phase {
        OperationPhase::AssistantReady {
            generation_context, ..
        }
        | OperationPhase::AssistantEffectPending {
            generation_context, ..
        }
        | OperationPhase::AssistantRetryWait {
            generation_context, ..
        } => Some(generation_context.configuration.model.clone()),
        OperationPhase::Tools { batch } => Some(batch.configuration.model.clone()),
        OperationPhase::DeferredSuspended { deferred }
        | OperationPhase::DeferredEffectPending { deferred, .. } => {
            Some(deferred.configuration.model.clone())
        }
        OperationPhase::SummaryReady {
            summary_context, ..
        }
        | OperationPhase::SummaryEffectPending {
            summary_context, ..
        }
        | OperationPhase::SummaryRetryWait {
            summary_context, ..
        } => Some(summary_context.configuration.model.clone()),
        OperationPhase::Starting
        | OperationPhase::Checkpoint { .. }
        | OperationPhase::SummaryDeciding { .. }
        | OperationPhase::NavigationReadyToCommit { .. } => None,
    }
}

impl AgentLane for crate::agent_core::harness::runtime::lane::Lane {
    fn name(&self) -> &str {
        crate::agent_core::harness::runtime::lane::Lane::name(self)
    }

    fn get_tip_id<'a>(
        &'a self,
        _context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<String>>> {
        Box::pin(async move {
            self.assert_open()?;
            Ok(self.state().tip_id)
        })
    }

    fn find_entries<'a>(
        &'a self,
        query: Option<&crate::agent_core::harness::session::BranchScan>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<crate::agent_core::harness::session::Entry>>> {
        use crate::agent_core::harness::session::{
            BranchScan as BranchScanQuery, BranchScanOrder, StorageBranchScan,
        };
        let query = query.cloned().unwrap_or_else(BranchScanQuery::default);
        Box::pin(async move {
            // Upstream `findEntries` (`runtime/lane.ts:1900-1907`): the start
            // defaults to the lane tip; the order defaults to newest-first.
            self.assert_open()?;
            let query = query;
            let start = match query.start.clone() {
                Some(start) => start,
                None => match self.state().tip_id {
                    Some(tip) => tip,
                    None => return Ok(Vec::new()),
                },
            };
            let mut scan = StorageBranchScan::from_branch_scan(&query, start);
            scan.order = Some(query.order.unwrap_or(BranchScanOrder::NewestFirst));
            self.session().scan_branch(&scan, context).await
        })
    }

    fn find_entry<'a>(
        &'a self,
        query: Option<&crate::agent_core::harness::session::BranchScan>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<crate::agent_core::harness::session::Entry>>> {
        use crate::agent_core::harness::session::BranchScan as BranchScanQuery;
        let query = query.cloned().unwrap_or_else(BranchScanQuery::default);
        Box::pin(async move {
            // Upstream `findEntry` (`runtime/lane.ts:1909-1914`): findEntries
            // with the limit clamped to one.
            let mut limited = query;
            limited.limit = Some(limited.limit.map_or(1, |limit| limit.min(1)));
            Ok(AgentLane::find_entries(self, Some(&limited), context)
                .await?
                .into_iter()
                .next())
        })
    }

    fn append_message<'a>(
        &'a self,
        message: AgentMessage,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<String>> {
        Box::pin(
            crate::agent_core::harness::runtime::lane::Lane::append_message(self, message, context),
        )
    }

    fn append_custom_entry<'a>(
        &'a self,
        custom_type: String,
        data: Option<serde_json::Value>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<String>> {
        Box::pin(
            crate::agent_core::harness::runtime::lane::Lane::append_custom_entry(
                self,
                custom_type,
                data,
                context,
            ),
        )
    }

    fn get_result<'a>(
        &'a self,
        operation_id: &str,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<OperationResultRecord>>> {
        use crate::agent_core::harness::session::{operation_result, Session as _, StoredValue};
        let operation_id = operation_id.to_owned();
        Box::pin(async move {
            // Upstream `getResult` (`runtime/lane.ts:271-274`): the stored
            // `pi.result` value payload.
            self.assert_open()?;
            let stored: Option<StoredValue> = self
                .session()
                .get_value(&operation_result(&operation_id), context)
                .await?;
            Ok(stored.and_then(|stored| serde_json::from_value(stored.value).ok()))
        })
    }

    fn accept<'a>(
        &'a self,
        request: &OperationRequest,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<OperationAdmissionResult>> {
        use crate::agent_core::harness::runtime::lane::{
            NavigationOptions as RuntimeNavigationOptions, OperationRequest as RuntimeRequest,
        };
        let runtime_request = match request {
            OperationRequest::Prompt {
                operation_id,
                payload: PromptPayload::Text { prompt, images },
            } => {
                let request = match images {
                    None => RuntimeRequest::Prompt {
                        prompt: prompt.clone(),
                    },
                    Some(images) if images.is_empty() => RuntimeRequest::Prompt {
                        prompt: prompt.clone(),
                    },
                    Some(images) => RuntimeRequest::PromptWithImages {
                        prompt: prompt.clone(),
                        images: images.clone(),
                    },
                };
                return accept_runtime(self, request, operation_id.clone(), context);
            }
            OperationRequest::Prompt {
                operation_id,
                payload: PromptPayload::Messages(messages),
            } => {
                let messages = match messages {
                    PromptMessages::One(message) => vec![message.clone()],
                    PromptMessages::Many(messages) => messages.clone(),
                };
                return accept_runtime(
                    self,
                    RuntimeRequest::PromptMessages { messages },
                    operation_id.clone(),
                    context,
                );
            }
            OperationRequest::Skill {
                name,
                additional_instructions,
                ..
            } => RuntimeRequest::Skill {
                name: name.clone(),
                additional_instructions: additional_instructions.clone(),
            },
            OperationRequest::PromptTemplate { name, args, .. } => RuntimeRequest::PromptTemplate {
                name: name.clone(),
                args: args.clone(),
            },
            OperationRequest::Compaction {
                custom_instructions,
                ..
            } => RuntimeRequest::Compaction {
                custom_instructions: custom_instructions.clone(),
            },
            OperationRequest::Navigation {
                target_id, options, ..
            } => RuntimeRequest::Navigation {
                target_id: target_id.clone(),
                options: options.as_ref().map(|options| RuntimeNavigationOptions {
                    summarize: options.summarize,
                    label: options.label.clone(),
                    custom_instructions: options.custom_instructions.clone(),
                }),
            },
        };
        let requested_id = request.operation_id().map(str::to_owned);
        accept_runtime(self, runtime_request, requested_id, context)
    }

    fn drive<'a>(
        &'a self,
        options: &DriveOptions,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<DriveResult>> {
        let options = options.clone();
        Box::pin(async move {
            crate::agent_core::harness::runtime::lane::Lane::drive(self, &options, context).await
        })
    }

    fn request_abort<'a>(
        &'a self,
        operation_id: &str,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<AbortRequestResult>> {
        let operation_id = operation_id.to_owned();
        Box::pin(async move {
            crate::agent_core::harness::runtime::lane::Lane::request_abort(
                self,
                &operation_id,
                context,
            )
            .await
        })
    }

    fn inspect_execution<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<LaneExecutionInfo>> {
        let name = self.name().to_owned();
        Box::pin(async move {
            // Upstream `inspectExecution` (`runtime/lane.ts:1106-1131`) is a
            // read on the serialized mutation line.
            self.read_lane(
                move |state: &crate::agent_core::harness::runtime::durable::LaneState,
                      _reader: &dyn crate::agent_core::harness::session::SessionMutator| {
                    let name = name.clone();
                    Box::pin(async move {
                        use crate::agent_core::harness::session::Control;
                        let current =
                            state.operation.as_ref().map(|operation| CurrentOperationInfo {
                                id: operation.meta.operation_id.clone(),
                                kind: operation_kind_of(&operation.meta.intent),
                                started_at: operation.meta.started_at,
                                status: if matches!(
                                    operation.state.scope.control,
                                    Control::CancelRequested { .. }
                                ) {
                                    OperationStatus::Aborting
                                } else {
                                    OperationStatus::Open
                                },
                                captured_model: captured_model(&operation.state),
                            });
                        Ok(LaneExecutionInfo {
                            lane: name,
                            tip_id: state.tip_id.clone(),
                            configured_model: state.configuration.model.clone(),
                            current,
                            last_operation_id: state.last_operation_id.clone(),
                        })
                    })
                },
                context,
            )
            .await
        })
    }

    fn prompt<'a>(
        &'a self,
        text: &str,
        images: Option<Vec<ImageContent>>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<RunResult>> {
        let text = text.to_owned();
        Box::pin(async move {
            crate::agent_core::harness::runtime::lane::Lane::prompt(self, &text, images, context)
                .await
        })
    }

    fn prompt_messages<'a>(
        &'a self,
        messages: PromptMessages,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<RunResult>> {
        let messages = match messages {
            PromptMessages::One(message) => vec![message],
            PromptMessages::Many(messages) => messages,
        };
        Box::pin(
            crate::agent_core::harness::runtime::lane::Lane::prompt_messages(
                self, messages, context,
            ),
        )
    }

    fn skill<'a>(
        &'a self,
        name: &str,
        additional_instructions: Option<String>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<RunResult>> {
        let name = name.to_owned();
        Box::pin(async move {
            crate::agent_core::harness::runtime::lane::Lane::skill(
                self,
                &name,
                additional_instructions,
                context,
            )
            .await
        })
    }

    fn prompt_from_template<'a>(
        &'a self,
        name: &str,
        args: Option<Vec<String>>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<RunResult>> {
        let name = name.to_owned();
        Box::pin(async move {
            crate::agent_core::harness::runtime::lane::Lane::prompt_from_template(
                self, &name, args, context,
            )
            .await
        })
    }

    fn compact<'a>(
        &'a self,
        options: Option<&CompactOptions>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<CompactionResult>> {
        let custom_instructions = options.and_then(|options| options.custom_instructions.clone());
        Box::pin(async move {
            crate::agent_core::harness::runtime::lane::Lane::compact(
                self,
                custom_instructions,
                context,
            )
            .await
        })
    }

    fn navigate_tree<'a>(
        &'a self,
        target_id: Option<String>,
        options: Option<&NavigateOptions>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<NavigationResult>> {
        let options =
            options.map(
                |options| crate::agent_core::harness::runtime::lane::NavigationOptions {
                    summarize: options.summarize,
                    label: options.label.clone(),
                    custom_instructions: options.custom_instructions.clone(),
                },
            );
        Box::pin(async move {
            crate::agent_core::harness::runtime::lane::Lane::navigate_tree(
                self, target_id, options, context,
            )
            .await
        })
    }

    fn resume<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<ResumeResult>> {
        Box::pin(crate::agent_core::harness::runtime::lane::Lane::resume(
            self, context,
        ))
    }

    fn abort<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<AbortResult>> {
        Box::pin(crate::agent_core::harness::runtime::lane::Lane::abort(
            self, context,
        ))
    }

    fn steer<'a>(
        &'a self,
        message: QueueInput,
        images: Option<Vec<ImageContent>>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<QueueResult>> {
        Box::pin(crate::agent_core::harness::runtime::lane::Lane::steer(
            self, message, images, context,
        ))
    }

    fn follow_up<'a>(
        &'a self,
        message: QueueInput,
        images: Option<Vec<ImageContent>>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<QueueResult>> {
        Box::pin(crate::agent_core::harness::runtime::lane::Lane::follow_up(
            self, message, images, context,
        ))
    }

    fn next_run<'a>(
        &'a self,
        message: QueueInput,
        images: Option<Vec<ImageContent>>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<QueueResult>> {
        Box::pin(crate::agent_core::harness::runtime::lane::Lane::next_run(
            self, message, images, context,
        ))
    }

    fn cancel_queued<'a>(
        &'a self,
        entry_id: &str,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<CancelQueuedResult>> {
        let entry_id = entry_id.to_owned();
        Box::pin(async move {
            crate::agent_core::harness::runtime::lane::Lane::cancel_queued(self, &entry_id, context)
                .await
        })
    }

    fn record_usage<'a>(
        &'a self,
        usage: Usage,
        options: Option<&RecordUsageOptions>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<RecordUsageResult>> {
        let options = options.cloned();
        Box::pin(
            crate::agent_core::harness::runtime::lane::Lane::record_usage(
                self, usage, options, context,
            ),
        )
    }

    fn wait_for_idle<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(crate::agent_core::harness::runtime::lane::Lane::wait_for_idle(self, context))
    }

    fn run_when_idle<'a>(
        &'a self,
        callback: IdleCallback,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(
            crate::agent_core::harness::runtime::lane::Lane::run_when_idle(self, callback, context),
        )
    }

    fn get_model<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<crate::ai::types::model::Model>>> {
        Box::pin(crate::agent_core::harness::runtime::lane::Lane::get_model(
            self, context,
        ))
    }

    fn set_model<'a>(
        &'a self,
        model: ModelIdentity,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(crate::agent_core::harness::runtime::lane::Lane::set_model(
            self, model, context,
        ))
    }

    fn get_thinking_level<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<ThinkingLevel>> {
        Box::pin(crate::agent_core::harness::runtime::lane::Lane::get_thinking_level(self, context))
    }

    fn set_thinking_level<'a>(
        &'a self,
        level: ThinkingLevel,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(
            crate::agent_core::harness::runtime::lane::Lane::set_thinking_level(
                self, level, context,
            ),
        )
    }

    fn get_active_tools<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<String>>> {
        Box::pin(crate::agent_core::harness::runtime::lane::Lane::get_active_tools(self, context))
    }

    fn set_active_tools<'a>(
        &'a self,
        names: Vec<String>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(
            crate::agent_core::harness::runtime::lane::Lane::set_active_tools(self, names, context),
        )
    }

    fn watch<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<HarnessWatchHandle<LaneSnapshot>>> {
        Box::pin(crate::agent_core::harness::runtime::lane::Lane::watch(
            self, context,
        ))
    }
}
