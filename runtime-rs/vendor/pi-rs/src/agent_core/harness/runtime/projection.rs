//! Snapshot contract and the reducer's read-only event projection from
//! agent-harness.ts:194-248. SnapshotEvent consumes either native-event JSON or
//! strict-JSON watch events; fields the reducer never reads are intentionally
//! ignored. This is not the still-unported full native HarnessEvent API.
use crate::agent_core::harness::session::{
    Entry, InboxItemKind, LaneConfiguration, LaneModel, OperationError, OperationResultRecord,
    SessionStats, TerminalStatus,
};
use crate::agent_core::types::{AgentMessage, AgentToolResult, ThinkingLevel};
use crate::ai::types::{DeferredHandle, Usage};
use crate::serde_support::present_json;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Run,
    Compaction,
    Navigation,
}
impl OperationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Compaction => "compaction",
            Self::Navigation => "navigation",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    Running,
    Open,
    Aborting,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaneSnapshotTool {
    pub tool_call_id: String,
    pub tool_name: String,
    pub args: Value,
    #[serde(flatten)]
    pub state: SnapshotToolState,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum SnapshotToolState {
    Running {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<AgentToolResult>,
    },
    Settled {
        result: AgentToolResult,
        is_error: bool,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrySnapshot {
    pub attempt: u32,
    pub max_attempts: u32,
    pub next_attempt_at: i64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeferredSnapshot {
    pub handle: DeferredHandle,
    pub poll: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaneOperationSnapshot {
    pub id: String,
    pub kind: OperationKind,
    pub started_at: i64,
    pub from_tip_id: Option<String>,
    pub status: OperationStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetrySnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferred: Option<DeferredSnapshot>,
    /// AgentMessage retains the role tag; the reducer only installs Assistant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub streaming_message: Option<AgentMessage>,
    pub running_tools: Vec<LaneSnapshotTool>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteKind {
    Write,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
#[allow(clippy::large_enum_variant)] // Message preserves AgentMessage's established unboxed wire representation.
pub enum LaneQueuedItem {
    Message {
        entry_id: String,
        kind: InboxItemKind,
        message: AgentMessage,
    },
    Custom {
        entry_id: String,
        kind: WriteKind,
        custom_type: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present_json"
        )]
        data: Option<Value>,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaneSnapshot {
    pub lane: String,
    pub transcript: Vec<Entry>,
    pub tip_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_result: Option<OperationResultRecord>,
    pub configuration: LaneConfiguration,
    pub stats: SessionStats,
    pub operation: Option<LaneOperationSnapshot>,
    pub queues: Vec<LaneQueuedItem>,
    pub faulted: bool,
}
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SnapshotEvent {
    #[serde(default)]
    pub lane: Option<String>,
    #[serde(flatten)]
    pub payload: SnapshotEventPayload,
}
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "property")]
pub enum SnapshotConfigChange {
    #[serde(rename = "model")]
    Model { value: LaneModel },
    #[serde(rename = "thinkingLevel")]
    ThinkingLevel { value: ThinkingLevel },
    #[serde(rename = "activeTools")]
    ActiveTools { value: Vec<String> },
    #[serde(other)]
    Other,
}
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
#[allow(clippy::large_enum_variant)] // Event projection owns full messages and entries, like the upstream union.
pub enum SnapshotEventPayload {
    RunStart {
        run_id: String,
        started_at: i64,
    },
    CompactionStart {
        run_id: String,
        started_at: i64,
    },
    NavigationStart {
        run_id: String,
        started_at: i64,
    },
    OperationAbort {
        operation_id: String,
    },
    RunResume {
        run_id: String,
    },
    RunSuspend {
        run_id: String,
        deferred: DeferredHandle,
        poll: u64,
    },
    RetryScheduled {
        run_id: String,
        attempt: u32,
        max_attempts: u32,
        not_before: i64,
    },
    RetryStart {
        run_id: String,
    },
    RetryEnd {
        run_id: String,
    },
    MessageStart {
        #[serde(default)]
        run_id: Option<String>,
        message: AgentMessage,
    },
    MessageUpdate {
        run_id: String,
        message: AgentMessage,
    },
    MessageEnd {
        #[serde(default)]
        run_id: Option<String>,
    },
    ToolStart {
        run_id: String,
        tool_call_id: String,
        tool_name: String,
        args: Value,
    },
    ToolUpdate {
        run_id: String,
        tool_call_id: String,
        partial_result: AgentToolResult,
    },
    ToolEnd {
        run_id: String,
        tool_call_id: String,
        tool_name: String,
        result: AgentToolResult,
        is_error: bool,
    },
    EntryAdded {
        entry: Entry,
    },
    QueueUpdate {
        queues: Vec<LaneQueuedItem>,
    },
    Usage {
        totals: Usage,
    },
    ConfigUpdate {
        #[serde(flatten)]
        change: SnapshotConfigChange,
    },
    RunEnd {
        run_id: String,
        status: TerminalStatus,
        #[serde(default)]
        error: Option<OperationError>,
        from_tip_id: Option<String>,
        tip_id: Option<String>,
        ended_at: i64,
    },
    CompactionEnd {
        run_id: String,
        status: TerminalStatus,
        #[serde(default)]
        error: Option<OperationError>,
        ended_at: i64,
    },
    NavigationEnd,
    Fault,
    HandlerError,
    TurnStart,
    TurnEnd,
    ValueUpdate,
    LaneCreated,
}
