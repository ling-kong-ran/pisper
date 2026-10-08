//! Port of `packages/agent/src/harness/execution/` (M3b Task 6): the
//! execution primitives one drive pass is built from —
//! [`effect_gate`] (upstream `execution/effect-gate.ts`), the tool-call
//! phase functions in [`tools`] (upstream `execution/tools.ts`), and the
//! assistant stream driver in [`assistant`] (upstream
//! `execution/assistant.ts`). Each item cites its upstream source and lines.
//!
//! The [`hooks::Gate`] trait (M3b Task 4) is implemented by
//! [`effect_gate::EffectGate`], as the hooks module docs disclose; the
//! concrete type renames and abort-signal substitutions are documented in
//! [`effect_gate`] module docs, the execution-shape substitutions in
//! [`tools`] and [`assistant`].

pub mod assistant;
pub mod effect_gate;
pub mod tools;

pub use assistant::{
    consume_assistant_stream, stream_harness_assistant, AfterResponseFn, AiContext,
    AssistantMessageEventStream, AssistantRequestFn, AssistantRequestOptions,
    AssistantResponseMetadata, AssistantStreamObserver, HarnessAssistantStreamConfig,
    HarnessRequestContext, PayloadCallback, ResponseCallback,
};
pub use effect_gate::{create_gate, AbortRequested, EffectGate, GateControl, GateRejection};
pub use tools::{
    apply_before_tool_decision, create_tool_result_message, execute_tool_call, finalize_tool_call,
    prepare_tool_call, tool_result_from_message, AfterToolPatch, BeforeToolDecision,
    ClearedToolCall, ExecutedToolCall, FinalizedToolCall, ImmediateToolOutcome, PrepareOutcome,
    PreparedToolCall,
};
