//! The [`HarnessEvent`] subset emitted by the ported runtime paths, from
//! upstream `agent-harness.ts`. The remaining variants (turns, tools,
//! compaction/navigation ends, fault, usage, value/handler updates) join this
//! enum as their runtime slices land; wire names follow the upstream union
//! exactly so fixtures stay comparable.

use crate::agent_core::harness::runtime::projection::LaneQueuedItem;
use crate::agent_core::harness::runtime::transcript::EntryLifecycleEvent;
use crate::agent_core::harness::session::{Entry, LaneModel, UsageRow};
use crate::agent_core::types::{AgentMessage, ThinkingLevel};
use crate::ai::frame::AssistantMessageFrame;
use crate::ai::types::events::AssistantMessageEvent;
use crate::ai::types::message::ToolResultMessage;
use crate::ai::types::options::DeferredHandle;
use crate::ai::types::primitives::Usage;
use serde::{Deserialize, Serialize};

/// `property` payload of `config_update` events, upstream
/// `ConfigEventPayload` narrowed to `LaneConfigEventPayload` (the lane-owned
/// properties this slice mutates). `model.previous` is upstream `unknown`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "property",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ConfigUpdateProperty {
    #[serde(rename = "model")]
    Model {
        value: LaneModel,
        previous: serde_json::Value,
    },
    #[serde(rename = "thinkingLevel")]
    ThinkingLevel {
        value: ThinkingLevel,
        previous: ThinkingLevel,
    },
    #[serde(rename = "activeTools")]
    ActiveTools {
        value: Vec<String>,
        previous: Vec<String>,
    },
}

/// Upstream run_end `status` union (`agent-harness.ts:260-262`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunEndStatus {
    Completed,
    Aborted,
    Failed,
}

/// Upstream compaction_end `status` union (`agent-harness.ts:363-365`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionEndStatus {
    Completed,
    Declined,
    Aborted,
    Failed,
}

/// Wire envelopes for the events the runtime emits itself (`run_start`, entry
/// lifecycles, queue and lane-config updates).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
#[allow(clippy::large_enum_variant)] // Preserve the owned Entry/AgentMessage API like transcript.rs.
pub enum HarnessEvent {
    RunStart {
        run_id: String,
        started_at: i64,
        lane: String,
    },
    MessageStart {
        lane: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        recovery: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
        message: AgentMessage,
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
    /// Upstream `message_update` (`agent-harness.ts:297-303`): one streamed
    /// update with the event and, when the encoder produced one, its frame.
    MessageUpdate {
        lane: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        recovery: bool,
        run_id: String,
        message: AgentMessage,
        event: AssistantMessageEvent,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame: Option<AssistantMessageFrame>,
    },
    EntryAdded {
        lane: String,
        entry: Entry,
    },
    QueueUpdate {
        lane: String,
        queues: Vec<LaneQueuedItem>,
    },
    /// Upstream `turn_end` (`agent-harness.ts:269-276` plus the lane/recovery
    /// envelope): one assistant turn settled, with its tool results.
    TurnEnd {
        lane: String,
        run_id: String,
        turn_id: String,
        message: AgentMessage,
        tool_results: Vec<ToolResultMessage>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        recovery: bool,
    },
    /// Upstream `usage` (`agent-harness.ts:373`): one committed usage row
    /// with the post-commit session totals.
    Usage {
        lane: String,
        row: UsageRow,
        totals: Usage,
    },
    /// Upstream `retry_scheduled` (`agent-harness.ts:278-287`): one bounded
    /// retry queued with its backoff.
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
    /// Upstream `retry_end` (`agent-harness.ts:287-296`): one retry attempt
    /// settled, successfully or with its final error.
    RetryEnd {
        lane: String,
        run_id: String,
        step: String,
        attempt: u32,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        final_error: Option<String>,
    },
    /// Upstream `run_suspend` (`agent-harness.ts:257-259`): one run parked on
    /// a deferred handle.
    RunSuspend {
        lane: String,
        run_id: String,
        deferred: DeferredHandle,
        poll: u64,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        recovery: bool,
    },
    /// Upstream `compaction_start` (`agent-harness.ts:356-361`).
    CompactionStart {
        lane: String,
        run_id: String,
        reason: crate::agent_core::harness::runtime::durable::SummaryReason,
        started_at: i64,
    },
    /// Upstream `operation_abort` (lane.ts `requestOperationAbort` event
    /// literal): one durable cancellation request with the consumed queue
    /// payloads.
    OperationAbort {
        operation_id: String,
        steer: Vec<AgentMessage>,
        follow_up: Vec<AgentMessage>,
        lane: String,
    },
    /// Upstream `navigation_start` (lane.ts `acceptNavigation` event
    /// literal).
    NavigationStart {
        lane: String,
        run_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_id: Option<String>,
        started_at: i64,
    },
    /// Upstream `tool_start` (`agent-harness.ts` literal, `drive/tools.ts`
    /// delivery): one tool execution began.
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
    /// Upstream `tool_update`: one tool execution reported a partial result.
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
    /// Upstream `tool_end`: one tool execution settled.
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
    /// Upstream `run_resume` (`agent-harness.ts:257`): one suspended run
    /// resumed polling.
    RunResume {
        lane: String,
        run_id: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        recovery: bool,
    },
    /// Upstream `turn_start` (`agent-harness.ts:269`): one assistant turn
    /// began.
    TurnStart {
        lane: String,
        run_id: String,
        turn_id: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        recovery: bool,
    },
    /// Upstream `retry_start` (`agent-harness.ts:287`): one retry attempt
    /// began after its wait.
    RetryStart {
        lane: String,
        run_id: String,
        step: String,
        attempt: u32,
    },
    /// Upstream `compaction_end` (`agent-harness.ts:362-366`): one
    /// compaction attempt settled.
    CompactionEnd {
        lane: String,
        run_id: String,
        reason: crate::agent_core::harness::runtime::durable::SummaryReason,
        status: CompactionEndStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entry_id: Option<String>,
        ended_at: i64,
    },
    /// Upstream `navigation_end` (`agent-harness.ts:367-371`): one
    /// navigation settled.
    NavigationEnd {
        lane: String,
        run_id: String,
        status: RunEndStatus,
        from_tip_id: Option<String>,
        tip_id: Option<String>,
        ended_at: i64,
    },
    /// Upstream `run_end` (`agent-harness.ts:260-263` plus the lane
    /// envelope): one durable run finished.
    RunEnd {
        lane: String,
        run_id: String,
        status: RunEndStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<crate::agent_core::harness::session::OperationError>,
        from_tip_id: Option<String>,
        tip_id: Option<String>,
        ended_at: i64,
    },
    ConfigUpdate {
        lane: String,
        #[serde(flatten)]
        property: ConfigUpdateProperty,
    },
}

impl From<EntryLifecycleEvent> for HarnessEvent {
    fn from(event: EntryLifecycleEvent) -> Self {
        match event {
            EntryLifecycleEvent::MessageStart {
                lane,
                run_id,
                message,
            } => Self::MessageStart {
                recovery: false,
                lane,
                run_id,
                message,
            },
            EntryLifecycleEvent::MessageEnd {
                lane,
                run_id,
                message,
                entry_id,
            } => Self::MessageEnd {
                recovery: false,
                lane,
                run_id,
                message,
                entry_id,
            },
            EntryLifecycleEvent::EntryAdded { lane, entry } => Self::EntryAdded { lane, entry },
        }
    }
}

impl From<crate::agent_core::harness::runtime::drive::tools::ToolEvent> for HarnessEvent {
    /// Lift one tool-delivery event into the runtime union (field order
    /// matches the upstream `tool_*` literals).
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
            } => Self::ToolStart {
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
            } => Self::ToolUpdate {
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
            } => Self::ToolEnd {
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
