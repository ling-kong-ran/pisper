//! Runtime-owned operation leaves from upstream harness/session/types.ts:75-342.
//! Session storage remains JSON-opaque; these types interpret values only at the
//! runtime boundary. Serde flattens scope and phase to the original wire shape.
use crate::agent_core::harness::config::CompactionSettings;
use crate::agent_core::harness::session::{Control, InboxItem, LaneConfiguration};
use crate::agent_core::harness::types::AgentHarnessStreamOptions;
use crate::agent_core::types::{QueueMode, ToolExecutionMode, ToolReplay};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationMeta {
    pub operation_id: String,
    pub lane: String,
    pub source_tip_id: Option<String>,
    pub started_at: i64,
    pub intent: OperationIntent,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum OperationIntent {
    Run {
        prompt_entry_ids: Vec<String>,
    },
    Compaction {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        custom_instructions: Option<String>,
    },
    Navigation {
        target_id: Option<String>,
        summarize: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        custom_instructions: Option<String>,
    },
}
impl OperationIntent {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Run { .. } => "run",
            Self::Compaction { .. } => "compaction",
            Self::Navigation { .. } => "navigation",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Continuation {
    NeedAssistant { overflow_recovery_used: bool },
    MayFinish { include_final_assistant: bool },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointData {
    pub continuation: Continuation,
    pub trigger_entry_id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedRetryPolicy {
    pub max_attempts: u32,
    pub base_delay_ms: u64,
    pub max_agent_delay_ms: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationContext {
    pub step_id: String,
    pub trigger_entry_id: String,
    pub configuration: LaneConfiguration,
    pub stream_options: AgentHarnessStreamOptions,
    pub retry_policy: NormalizedRetryPolicy,
    pub overflow_recovery_used: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    /// Index in the complete assistant content, not a filtered tool ordinal.
    pub source_index: usize,
    pub result_entry_id: String,
    #[serde(flatten)]
    pub state: ToolCallState,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ToolCallState {
    Planned,
    EffectPending { replay: ToolReplay },
    OutcomeReady { terminate: bool },
    Completed { terminate: bool },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolBatch {
    pub assistant_entry_id: String,
    pub configuration: LaneConfiguration,
    pub turn_id: String,
    pub calls: Vec<ToolCall>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryContext {
    pub result_entry_id: String,
    pub configuration: LaneConfiguration,
    pub stream_options: AgentHarnessStreamOptions,
    pub retry_policy: NormalizedRetryPolicy,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSettings {
    pub compaction: CompactionSettings,
    pub steering_mode: QueueMode,
    pub follow_up_mode: QueueMode,
    pub tool_execution: ToolExecutionMode,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationScope {
    pub control: Control,
    pub settings: RunSettings,
    pub latest_assistant_entry_id: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryWait {
    pub next_attempt: u32,
    pub not_before: i64,
    pub error_message: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ResultBoundary {
    ResumeCheckpoint {
        resume_after: CheckpointData,
    },
    Finish,
    CommitNavigation {
        target_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryReason {
    Manual,
    Threshold,
    Overflow,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryTask {
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<SummaryReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_instructions: Option<String>,
    pub boundary: ResultBoundary,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryRequest {
    pub index: usize,
    pub usage_id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeferredScope {
    pub step_id: String,
    pub source_entry_id: String,
    pub poll: u64,
    pub configuration: LaneConfiguration,
    pub stream_options: AgentHarnessStreamOptions,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperationState {
    #[serde(flatten)]
    pub scope: OperationScope,
    #[serde(flatten)]
    pub phase: OperationPhase,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "at", rename_all_fields = "camelCase")]
pub enum OperationPhase {
    #[serde(rename = "starting")]
    Starting,
    #[serde(rename = "checkpoint")]
    Checkpoint {
        #[serde(flatten)]
        checkpoint: CheckpointData,
    },
    #[serde(rename = "assistant.ready")]
    AssistantReady {
        generation_context: GenerationContext,
        next_attempt: u32,
    },
    #[serde(rename = "assistant.effect_pending")]
    AssistantEffectPending {
        generation_context: GenerationContext,
        attempt: u32,
        response_entry_id: String,
        usage_id: String,
        intended_output_limit: u64,
        context_window: u64,
    },
    #[serde(rename = "assistant.retry_wait")]
    AssistantRetryWait {
        generation_context: GenerationContext,
        #[serde(flatten)]
        retry: RetryWait,
    },
    #[serde(rename = "tools")]
    Tools { batch: ToolBatch },
    #[serde(rename = "deferred.suspended")]
    DeferredSuspended {
        #[serde(flatten)]
        deferred: DeferredScope,
    },
    #[serde(rename = "deferred.effect_pending")]
    DeferredEffectPending {
        #[serde(flatten)]
        deferred: DeferredScope,
        response_entry_id: String,
        usage_id: String,
    },
    #[serde(rename = "summary.deciding")]
    SummaryDeciding { task: SummaryTask },
    #[serde(rename = "summary.ready")]
    SummaryReady {
        task: SummaryTask,
        summary_context: SummaryContext,
        next_attempt: u32,
    },
    #[serde(rename = "summary.effect_pending")]
    SummaryEffectPending {
        task: SummaryTask,
        summary_context: SummaryContext,
        attempt: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request: Option<SummaryRequest>,
        usage_ids: Vec<String>,
    },
    #[serde(rename = "summary.retry_wait")]
    SummaryRetryWait {
        task: SummaryTask,
        summary_context: SummaryContext,
        #[serde(flatten)]
        retry: RetryWait,
    },
    #[serde(rename = "navigation.ready_to_commit")]
    NavigationReadyToCommit {
        target_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
}
impl OperationState {
    pub fn at(&self) -> &'static str {
        self.phase.at()
    }
    /// Copy exactly the family-neutral scope, not phase-specific fields.
    pub fn operation_scope(&self) -> OperationScope {
        self.scope.clone()
    }
}
impl OperationPhase {
    pub fn at(&self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Checkpoint { .. } => "checkpoint",
            Self::AssistantReady { .. } => "assistant.ready",
            Self::AssistantEffectPending { .. } => "assistant.effect_pending",
            Self::AssistantRetryWait { .. } => "assistant.retry_wait",
            Self::Tools { .. } => "tools",
            Self::DeferredSuspended { .. } => "deferred.suspended",
            Self::DeferredEffectPending { .. } => "deferred.effect_pending",
            Self::SummaryDeciding { .. } => "summary.deciding",
            Self::SummaryReady { .. } => "summary.ready",
            Self::SummaryEffectPending { .. } => "summary.effect_pending",
            Self::SummaryRetryWait { .. } => "summary.retry_wait",
            Self::NavigationReadyToCommit { .. } => "navigation.ready_to_commit",
        }
    }
    pub fn summary_task(&self) -> Option<&SummaryTask> {
        match self {
            Self::SummaryDeciding { task }
            | Self::SummaryReady { task, .. }
            | Self::SummaryEffectPending { task, .. }
            | Self::SummaryRetryWait { task, .. } => Some(task),
            _ => None,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Operation {
    pub meta: OperationMeta,
    pub state: OperationState,
}
/// Process-local restored projection (runtime/types.ts), not pi.lane.state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaneState {
    pub tip_id: Option<String>,
    pub configuration: LaneConfiguration,
    pub inbox: Vec<InboxItem>,
    pub last_operation_id: Option<String>,
    pub operation: Option<Operation>,
}
