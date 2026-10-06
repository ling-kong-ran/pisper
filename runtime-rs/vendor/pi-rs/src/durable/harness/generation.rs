//! Port of `src/harness/generation.ts`: the built-in `pi.generation` task —
//! it prepares the positional system prompt and tool loadout, requests or
//! polls the model, retries, and classifies the response. The run's inputs
//! live in `pi.live.run`.
//!
//! Divergences (structural, disclosed):
//! - upstream phases are async closures over a typed runtime; the port's
//!   phases are [`PhaseFn`] closures over the erased
//!   [`TaskRuntimeLike`](crate::durable::tasks::TaskRuntimeLike), with
//!   checkpoints as strict-JSON objects in the upstream construction order
//!   (D2).
//! - **D23 (partial throttle).** The upstream partial flush schedules one
//!   `setTimeout(PARTIAL_THROTTLE_MS)` per flush; the port keeps the same
//!   100 ms minimum interval and one-commit-in-flight rule on
//!   [`tokio::time`]. Streams that never retain a partial (the byte-oracle
//!   shape) commit none either way, so transcript bytes do not depend on
//!   flush timing.
//! - pi-ai `streamSimple`'s event stream arrives through the abstract
//!   [`ModelsHandle`] surface (D15); `result()` is the stream's terminal
//!   future.
//! - Upstream builds hook payloads as fresh objects; the port serializes the
//!   same shapes with `serde_json`, and hook handlers run through the
//!   generic [`HookHandlerFn`] signature.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};

use crate::agent_core::chord_support::context::Context;
use crate::ai::retry::{is_retryable_assistant_error, retry_delay_ms, RetryPolicy};
use crate::ai::transcript::get_current_tools;
use crate::ai::types::{
    AssistantBlock, AssistantMessage, Message, StopReason, StringOrBlocks, ToolCall, UserMessage,
};

use super::super::errors::PlainError;
use super::super::harness::config::{
    conversation_config, default_retry_policy, ConversationConfigState,
};
use super::super::harness::inbox::{apply_boundary, prepare_boundary, At};
use super::super::harness::json::assign_json;
use super::super::harness::live::{
    delete_generation, delete_tools, end_run, generation_of_draft, live_doc, run_of_draft,
    set_generation, set_slot_task_id, set_tools, tools_of_draft, ToolSlot,
};
use super::super::harness::prompt::{
    desired_tools, plan_system_entries, render_sections, replay_sections,
};
use super::super::harness::types::{
    ConversationStreamOptions, HookApi, ModelRef, PromptInput, SimpleStreamRequest, ToolControl,
    ToolExecutionMode,
};
use super::super::harness::usage::{js_number_value, record_usage};
use super::super::ids::{ConversationId, EntryId, SubmissionId, TaskId};
use super::super::session::transaction::{DocumentDraft, Transaction};
use super::super::tasks::{
    define_task, CommitChange, NextTaskState, PhaseArgs, PhaseFn, PlainFailure, TaskDefinition,
    TaskRuntimeLike,
};
use super::super::types::{
    EntryDraft, EntryHead, JoinPolicy, SubmissionSettlement, TaskOptions, TaskOutcome,
    TaskOutcomeError, TaskOwnership,
};
use super::tool::{append_tool_result, harness_error, tool_task};

/// Throttle pause between committed partials (`PARTIAL_THROTTLE_MS`).
pub const PARTIAL_THROTTLE_MS: u64 = 100;
/// Default deferred poll delay (`DEFAULT_POLL_AFTER_MS`).
pub const DEFAULT_POLL_AFTER_MS: u64 = 5000;

type Seg = crate::chord::delta::Seg;

fn failure(error: impl std::fmt::Display) -> PlainFailure {
    PlainFailure::new(error.to_string())
}

fn plain(error: impl std::fmt::Display) -> PlainError {
    PlainError::new(error.to_string())
}

fn wire_text<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn wire_parse<T: serde::de::DeserializeOwned>(text: &str) -> Option<T> {
    serde_json::from_value(Value::String(text.to_owned())).ok()
}

/// Model reference as a strict-JSON value (`{provider, modelId}`).
fn model_value(reference: &ModelRef) -> Value {
    let mut model = Map::new();
    model.insert(
        String::from("provider"),
        Value::from(reference.provider.clone()),
    );
    model.insert(
        String::from("modelId"),
        Value::from(reference.model_id.clone()),
    );
    Value::Object(model)
}

fn checkpoint_of(args: &PhaseArgs) -> Map<String, Value> {
    super::super::harness::scheduler::record_checkpoint(&args.record)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// `GenerationResult`: the final answer entry (`{entryId}`).
pub fn generation_result(entry_id: EntryId) -> Value {
    let mut result = Map::new();
    result.insert(String::from("entryId"), Value::from(entry_id));
    Value::Object(result)
}

/// What classification needs from the request that produced a message
/// (`Request`).
struct Request {
    attempt: i64,
    model: ModelRef,
    tool_execution: ToolExecutionMode,
    cutoff: EntryId,
    /// Committed model context through `cutoff`, when the phase already
    /// derived it.
    messages: Option<Vec<Message>>,
    /// Set when the message came from polling, so a still deferred result
    /// polls strictly later.
    poll_at: Option<f64>,
}

/// The built-in generation task (`GenerationTask`).
pub fn generation_task() -> super::super::tasks::TaskToken {
    let mut phases: std::collections::BTreeMap<String, PhaseFn> = std::collections::BTreeMap::new();
    fn boxed<F, Fut>(phase: F) -> PhaseFn
    where
        F: Fn(PhaseArgs) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(), PlainFailure>> + Send + 'static,
    {
        Arc::new(move |args: PhaseArgs| Box::pin(phase(args)))
    }
    phases.insert(String::from("prepare"), boxed(prepare_phase));
    phases.insert(String::from("request"), boxed(request_phase));
    phases.insert(String::from("retry"), boxed(retry_phase));
    phases.insert(String::from("poll"), boxed(poll_phase));
    phases.insert(String::from("tools"), boxed(tools_phase));
    define_task(TaskDefinition {
        name: String::from("pi.generation"),
        version: 1,
        initial: Arc::new(|| {
            let mut checkpoint = Map::new();
            checkpoint.insert(String::from("phase"), Value::from("prepare"));
            checkpoint.insert(String::from("attempt"), Value::from(1));
            checkpoint
        }),
        phases,
        abort: Some(boxed(abort_phase)),
        migrate: None,
    })
}

// ─── Phase: prepare ─────────────────────────────────────────────────────────

async fn prepare_phase(args: PhaseArgs) -> Result<(), PlainFailure> {
    let runtime = Arc::clone(&args.runtime);
    let context = args.context.clone();
    let conversation_id = runtime.conversation_id();
    let registry = runtime.registry();
    for failure in registry.failures() {
        runtime.report(&failure.error);
    }
    let config_definition = conversation_config();
    let config = runtime
        .snapshot(
            &config_definition.definition,
            Some(conversation_id),
            None,
            context.clone(),
        )
        .await
        .map_err(failure)?
        .and_then(|value| ConversationConfigState::from_json(&value).ok())
        .unwrap_or_else(ConversationConfigState::initial);
    let Some(model) = config.model.clone() else {
        return Err(fail_no_model(&runtime, None, &context).await);
    };
    let resolved = runtime
        .models()
        .and_then(|models| models.get_model(&model.provider, &model.model_id));
    if resolved.is_none() {
        return Err(fail_no_model(&runtime, Some(&model), &context).await);
    }
    let view = runtime
        .context(conversation_id, context.clone(), None)
        .await
        .map_err(failure)?;
    let shown = replay_sections(&view.messages);
    let tools = desired_tools(&config.active_tools, &|name| registry.tool(name));
    let input = PromptInput {
        conversation_id,
        tools: tools.clone(),
        shown: shown.iter().cloned().collect(),
        model: Some(model.clone()),
        thinking_level: config.thinking_level,
        read: runtime.session(),
    };
    let desired = {
        let runtime = Arc::clone(&runtime);
        let report = move |error: &PlainError| runtime.report(error);
        render_sections(&registry.sections(), &input, &shown, &report, &context)
            .await
            .map_err(failure)?
    };
    let declarations: Vec<crate::ai::types::Tool> =
        tools.iter().map(|tool| tool.tool.clone()).collect();
    let entries = plan_system_entries(&view, &desired, &declarations, runtime.now());
    let attempt = checkpoint_of(&args)
        .get("attempt")
        .and_then(Value::as_i64)
        .unwrap_or(1);
    let tool_execution = config.tool_execution.unwrap_or(ToolExecutionMode::Parallel);
    let stream_options = config.stream_options.clone().unwrap_or_default();
    let thinking_level = config.thinking_level;
    runtime
        .commit(
            Box::new(move |tx, _current| {
                prepare_commit(
                    tx,
                    conversation_id,
                    &entries,
                    attempt,
                    &model,
                    thinking_level,
                    &stream_options,
                    tool_execution,
                )
            }),
            context,
        )
        .await
        .map_err(failure)
}

/// The prepare commit body: append the planned `pi.system` entries and move
/// to `request` with the fixed configuration.
#[allow(clippy::too_many_arguments)]
fn prepare_commit(
    tx: &Transaction,
    conversation_id: ConversationId,
    entries: &[EntryDraft],
    attempt: i64,
    model: &ModelRef,
    thinking_level: crate::ai::types::ModelThinkingLevel,
    stream_options: &ConversationStreamOptions,
    tool_execution: ToolExecutionMode,
) -> Result<Option<NextTaskState>, PlainError> {
    let page = tx.scan_entries(
        super::super::types::EntryQuery {
            conversation_id,
            min_entry_id: None,
            max_entry_id: None,
        },
        1,
        None,
    )?;
    let mut cutoff = page.items.first().map(|entry| entry.id);
    for entry in entries {
        cutoff = Some(tx.append_entry(conversation_id, entry.clone())?.id);
    }
    let Some(cutoff) = cutoff else {
        return Err(plain(format!(
            "Conversation {conversation_id} has no entries to send"
        )));
    };
    let mut checkpoint = Map::new();
    checkpoint.insert(String::from("phase"), Value::from("request"));
    checkpoint.insert(String::from("attempt"), Value::from(attempt));
    checkpoint.insert(String::from("model"), model_value(model));
    checkpoint.insert(
        String::from("thinkingLevel"),
        Value::from(wire_text(&thinking_level)),
    );
    checkpoint.insert(
        String::from("streamOptions"),
        serde_json::to_value(stream_options).map_err(plain)?,
    );
    checkpoint.insert(
        String::from("toolExecution"),
        Value::from(wire_text(&tool_execution)),
    );
    checkpoint.insert(String::from("cutoff"), Value::from(cutoff));
    Ok(Some(NextTaskState::Running {
        checkpoint: Value::Object(checkpoint),
    }))
}

// ─── Phase: request ─────────────────────────────────────────────────────────

async fn request_phase(args: PhaseArgs) -> Result<(), PlainFailure> {
    let runtime = Arc::clone(&args.runtime);
    let context = args.context.clone();
    let checkpoint = checkpoint_of(&args);
    let attempt = checkpoint
        .get("attempt")
        .and_then(Value::as_i64)
        .unwrap_or(1);
    let reference: ModelRef =
        serde_json::from_value(checkpoint.get("model").cloned().unwrap_or(Value::Null))
            .map_err(failure)?;
    let thinking_level = checkpoint
        .get("thinkingLevel")
        .and_then(Value::as_str)
        .and_then(wire_parse::<crate::ai::types::ModelThinkingLevel>)
        .unwrap_or(crate::ai::types::ModelThinkingLevel::Off);
    let stream_options: ConversationStreamOptions = serde_json::from_value(
        checkpoint
            .get("streamOptions")
            .cloned()
            .unwrap_or(Value::Null),
    )
    .map_err(failure)?;
    let tool_execution = checkpoint
        .get("toolExecution")
        .and_then(Value::as_str)
        .and_then(wire_parse::<ToolExecutionMode>)
        .unwrap_or(ToolExecutionMode::Parallel);
    let cutoff = checkpoint
        .get("cutoff")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let conversation_id = runtime.conversation_id();
    // Mark the attempt on the live document before requesting.
    runtime
        .commit(
            Box::new(move |tx, _current| {
                let live = live_draft(tx, conversation_id)?;
                convert_partial(tx, &live, conversation_id)?;
                let mut generation = Map::new();
                generation.insert(String::from("attempt"), Value::from(attempt));
                set_generation(&live, generation)?;
                Ok(None)
            }),
            context.clone(),
        )
        .await
        .map_err(failure)?;
    let model = runtime
        .models()
        .and_then(|models| models.get_model(&reference.provider, &reference.model_id));
    let Some(model) = model else {
        return Err(fail_no_model(&runtime, Some(&reference), &context).await);
    };
    let view = runtime
        .context(conversation_id, context.clone(), Some(cutoff))
        .await
        .map_err(failure)?;
    let view_messages = view.messages.clone();
    let messages = {
        let hook_runtime = Arc::clone(&runtime);
        let context = context.clone();
        let messages: Arc<Mutex<Vec<Message>>> = Arc::new(Mutex::new(view.messages));
        let hook_messages = Arc::clone(&messages);
        let hook_context = context.clone();
        let capture_runtime = Arc::clone(&hook_runtime);
        hook_runtime
            .hooks(
                "beforeRequest",
                Arc::new(move |hook| {
                    let runtime = Arc::clone(&capture_runtime);
                    let context = hook_context.clone();
                    let messages = Arc::clone(&hook_messages);
                    let hook = Arc::clone(hook);
                    Box::pin(async move {
                        let payload = serde_json::json!({
                            "messages": messages
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .clone()
                        });
                        let replaced = hook(payload, hook_api(&runtime), context.clone()).await?;
                        if let Some(value) = replaced {
                            if let Some(next) = value.get("messages") {
                                *messages
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                    serde_json::from_value::<Vec<Message>>(next.clone())
                                        .map_err(plain)?;
                            }
                        }
                        Ok(None)
                    })
                }),
            )
            .await
            .map_err(failure)?;
        let final_messages = messages
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        final_messages
    };
    let mut request = SimpleStreamRequest {
        options: stream_options,
        reasoning: None,
        // Upstream `sessionId: await ensureProviderSessionId(runtime,
        // context)` (generation.ts @ 200387122): the `pi.provider` identity,
        // created by one migration commit for a legacy conversation.
        session_id: Some(
            super::provider::ensure_provider_session_id(&runtime, &context)
                .await
                .map_err(failure)?,
        ),
        signal: Some(runtime.signal()),
    };
    if thinking_level != crate::ai::types::ModelThinkingLevel::Off {
        request.reasoning = Some(thinking_level);
    }
    let message = stream_response(&runtime, model, messages, request, attempt, &context).await?;
    classify(
        &runtime,
        Request {
            attempt,
            model: reference,
            tool_execution,
            cutoff,
            messages: Some(view_messages),
            poll_at: None,
        },
        message,
        &context,
    )
    .await
}

// ─── Phase: retry ───────────────────────────────────────────────────────────

async fn retry_phase(args: PhaseArgs) -> Result<(), PlainFailure> {
    let runtime = Arc::clone(&args.runtime);
    let context = args.context.clone();
    let checkpoint = checkpoint_of(&args);
    let attempt = checkpoint
        .get("attempt")
        .and_then(Value::as_i64)
        .unwrap_or(1);
    let until = checkpoint
        .get("until")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let conversation_id = runtime.conversation_id();
    runtime
        .sleep(until, context.clone())
        .await
        .map_err(failure)?;
    runtime
        .commit(
            Box::new(move |tx, _current| {
                let live = live_draft(tx, conversation_id)?;
                let mut generation = Map::new();
                generation.insert(String::from("attempt"), Value::from(attempt + 1));
                set_generation(&live, generation)?;
                let mut checkpoint = Map::new();
                checkpoint.insert(String::from("phase"), Value::from("prepare"));
                checkpoint.insert(String::from("attempt"), Value::from(attempt + 1));
                Ok(Some(NextTaskState::Running {
                    checkpoint: Value::Object(checkpoint),
                }))
            }),
            context,
        )
        .await
        .map_err(failure)
}

// ─── Phase: poll ────────────────────────────────────────────────────────────

async fn poll_phase(args: PhaseArgs) -> Result<(), PlainFailure> {
    let runtime = Arc::clone(&args.runtime);
    let context = args.context.clone();
    let checkpoint = checkpoint_of(&args);
    let attempt = checkpoint
        .get("attempt")
        .and_then(Value::as_i64)
        .unwrap_or(1);
    let reference: ModelRef =
        serde_json::from_value(checkpoint.get("model").cloned().unwrap_or(Value::Null))
            .map_err(failure)?;
    let tool_execution = checkpoint
        .get("toolExecution")
        .and_then(Value::as_str)
        .and_then(wire_parse::<ToolExecutionMode>)
        .unwrap_or(ToolExecutionMode::Parallel);
    let cutoff = checkpoint
        .get("cutoff")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let handle: crate::ai::types::DeferredHandle =
        serde_json::from_value(checkpoint.get("handle").cloned().unwrap_or(Value::Null))
            .map_err(failure)?;
    let poll_at = checkpoint.get("pollAt").and_then(Value::as_f64);
    let model = runtime
        .models()
        .and_then(|models| models.get_model(&reference.provider, &reference.model_id));
    let Some(model) = model else {
        return Err(fail_no_model(&runtime, Some(&reference), &context).await);
    };
    if let Some(poll_at) = poll_at {
        runtime
            .sleep(poll_at, context.clone())
            .await
            .map_err(failure)?;
    }
    let message = runtime
        .models()
        .expect("models resolve above")
        .fetch_deferred(model, handle, Some(runtime.signal()))
        .await
        .map_err(failure)?;
    classify(
        &runtime,
        Request {
            attempt,
            model: reference,
            tool_execution,
            cutoff,
            messages: None,
            poll_at,
        },
        message,
        &context,
    )
    .await
}

// ─── Phase: tools ───────────────────────────────────────────────────────────

async fn tools_phase(args: PhaseArgs) -> Result<(), PlainFailure> {
    let runtime = Arc::clone(&args.runtime);
    let context = args.context.clone();
    let checkpoint = checkpoint_of(&args);
    let assistant = checkpoint
        .get("assistant")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let tools: Vec<TaskId> = checkpoint
        .get("tools")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default();
    let pending: Vec<String> = checkpoint
        .get("pending")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let Some(next) = pending.first().cloned() else {
        return finish_tool_round(&runtime, assistant, tools, &context).await;
    };
    let rest = pending[1..].to_vec();
    // Sequential round: start the next call and wait for it.
    let conversation_id = runtime.conversation_id();
    let task_token = tool_task();
    let runtime_task_id = runtime.task_id();
    runtime
        .commit(
            Box::new(move |tx, _current| {
                let live = live_draft(tx, conversation_id)?;
                let task_id = create_tool_task(tx, &task_token, runtime_task_id, assistant, &next)?;
                // Bind the pending slot to the task it started
                // (`slot.taskId = taskId` for the call's slot).
                if let Some(index) = find_slot_by_call(&live, &next)? {
                    let slots = tools_of_draft(&live)?;
                    if slots.get(index).is_some_and(|slot| slot.task_id.is_none()) {
                        set_slot_task_id(&live, index, task_id)?;
                    }
                }
                let mut tools = tools;
                tools.push(task_id);
                let mut checkpoint = Map::new();
                checkpoint.insert(String::from("phase"), Value::from("tools"));
                checkpoint.insert(String::from("assistant"), Value::from(assistant));
                checkpoint.insert(
                    String::from("tools"),
                    Value::Array(tools.iter().map(|id| Value::from(*id)).collect()),
                );
                checkpoint.insert(
                    String::from("pending"),
                    Value::Array(rest.iter().map(|item| Value::from(item.as_str())).collect()),
                );
                Ok(Some(NextTaskState::Waiting {
                    checkpoint: Value::Object(checkpoint),
                    on: vec![task_id],
                    policy: JoinPolicy::AllSettled,
                }))
            }),
            context,
        )
        .await
        .map_err(failure)
}

// ─── Abort handler ──────────────────────────────────────────────────────────

async fn abort_phase(args: PhaseArgs) -> Result<(), PlainFailure> {
    let runtime = Arc::clone(&args.runtime);
    let context = args.context.clone();
    let checkpoint = checkpoint_of(&args);
    let phase = checkpoint
        .get("phase")
        .and_then(Value::as_str)
        .unwrap_or("");
    let conversation_id = runtime.conversation_id();
    if phase == "poll" {
        let reference: Option<ModelRef> = checkpoint
            .get("model")
            .cloned()
            .map(|value| serde_json::from_value(value).map_err(failure))
            .transpose()?;
        let handle: Option<crate::ai::types::DeferredHandle> = checkpoint
            .get("handle")
            .cloned()
            .map(|value| serde_json::from_value(value).map_err(failure))
            .transpose()?;
        if let (Some(reference), Some(handle)) = (&reference, &handle) {
            if let Some(models) = runtime.models() {
                if let Some(model) = models.get_model(&reference.provider, &reference.model_id) {
                    if let Err(error) = models
                        .cancel_deferred(model, handle.clone(), Some(runtime.signal()))
                        .await
                    {
                        runtime.report(&error);
                    }
                }
            }
        }
    }
    // Runs after the round's tool tasks are terminal; calls never started get
    // `aborted` results (spec §8.5).
    let unstarted = if phase == "tools" {
        let assistant = checkpoint
            .get("assistant")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let pending: Vec<String> = checkpoint
            .get("pending")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        read_calls(&runtime, assistant, &pending, &context).await?
    } else {
        Vec::new()
    };
    let now = runtime.now();
    runtime
        .commit(
            Box::new(move |tx, current| {
                let live = live_draft(tx, conversation_id)?;
                convert_partial(tx, &live, conversation_id)?;
                for call in &unstarted {
                    let result =
                        harness_error("aborted", format!("Tool {} was aborted", call.name));
                    append_tool_result(tx, conversation_id, call, &result, now)?;
                }
                end_run(
                    tx,
                    &live,
                    current.id,
                    SubmissionSettlement::Unanswered {
                        reason: String::from("aborted"),
                        detail: None,
                    },
                )?;
                Ok(Some(NextTaskState::Terminal {
                    outcome: TaskOutcome::Aborted {
                        reason: None,
                        result: None,
                    },
                }))
            }),
            context,
        )
        .await
        .map_err(failure)
}

// ─── Shared helpers ─────────────────────────────────────────────────────────

/// The live document draft of a conversation.
fn live_draft(
    tx: &Transaction,
    conversation_id: ConversationId,
) -> Result<DocumentDraft, PlainError> {
    let live = live_doc();
    tx.doc(&live.definition, Some(conversation_id), None, None)
}

/// The calls `call_ids` of the assistant entry, in the given order
/// (`readCalls`).
async fn read_calls(
    runtime: &Arc<dyn TaskRuntimeLike>,
    assistant: EntryId,
    call_ids: &[String],
    context: &Context,
) -> Result<Vec<ToolCall>, PlainFailure> {
    let entry = runtime
        .entry(
            Some(String::from("pi.assistant")),
            assistant,
            context.clone(),
        )
        .await
        .map_err(failure)?;
    let calls: Vec<ToolCall> = entry
        .and_then(|entry| entry.model)
        .and_then(|messages| messages.into_iter().next())
        .map(|message| match message {
            Message::Assistant(assistant) => assistant
                .content
                .iter()
                .filter_map(|block| match block {
                    AssistantBlock::ToolCall(call) => Some(call.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .unwrap_or_default();
    let mut selected: Vec<ToolCall> = Vec::new();
    for id in call_ids {
        if let Some(call) = calls.iter().find(|call| &call.id == id) {
            selected.push(call.clone());
        }
    }
    Ok(selected)
}

/// A tool task for call `call_id`, owned by the generation (`createToolTask`).
fn create_tool_task(
    tx: &Transaction,
    token: &super::super::tasks::TaskToken,
    runtime_task_id: TaskId,
    assistant: EntryId,
    call_id: &str,
) -> Result<TaskId, PlainError> {
    let mut input = Map::new();
    input.insert(String::from("assistant"), Value::from(assistant));
    input.insert(String::from("callId"), Value::from(call_id));
    tx.create_task(
        token,
        Value::Object(input),
        TaskOptions {
            ownership: TaskOwnership::Task {
                task_id: runtime_task_id,
            },
            conversation_id: None,
            background: None,
        },
    )
}

/// Settle the run's inputs `unanswered` with `no_model` and fail
/// (`failNoModel`).
async fn fail_no_model(
    runtime: &Arc<dyn TaskRuntimeLike>,
    reference: Option<&ModelRef>,
    context: &Context,
) -> PlainFailure {
    let message = match reference {
        None => String::from("No model is configured"),
        Some(reference) => format!(
            "Model {}/{} is not available",
            reference.provider, reference.model_id
        ),
    };
    let conversation_id = runtime.conversation_id();
    let outcome = TaskOutcome::Failed {
        error: TaskOutcomeError {
            message: message.clone(),
            detail: Some(serde_json::json!({"reason": "no_model"})),
        },
        result: None,
    };
    if let Err(error) = runtime
        .commit(
            Box::new(move |tx, current| {
                let live = live_draft(tx, conversation_id)?;
                end_run(
                    tx,
                    &live,
                    current.id,
                    SubmissionSettlement::Unanswered {
                        reason: String::from("no_model"),
                        detail: None,
                    },
                )?;
                Ok(Some(NextTaskState::Terminal { outcome }))
            }),
            context.clone(),
        )
        .await
    {
        return failure(error);
    }
    failure(message)
}

/// Append a committed partial left by an interrupted, aborted, faulted, or
/// orphaned attempt as an aborted assistant entry; the caller replaces or
/// removes `generation` (`convertPartial`).
pub fn convert_partial(
    tx: &Transaction,
    live: &DocumentDraft,
    conversation_id: ConversationId,
) -> Result<(), PlainError> {
    let Some(partial) =
        generation_of_draft(live)?.and_then(|generation| generation.get("message").cloned())
    else {
        return Ok(());
    };
    let mut message: AssistantMessage = serde_json::from_value(partial).map_err(plain)?;
    message.stop_reason = StopReason::Aborted;
    append_assistant(tx, conversation_id, &message)?;
    Ok(())
}

/// Stream one request and return the terminal message (`streamResponse`).
/// Partials commit as trailing writes at most every 100 ms with one commit in
/// flight (D23).
async fn stream_response(
    runtime: &Arc<dyn TaskRuntimeLike>,
    model: Arc<dyn super::super::harness::types::ModelHandle>,
    messages: Vec<Message>,
    options: SimpleStreamRequest,
    attempt: i64,
    context: &Context,
) -> Result<AssistantMessage, PlainFailure> {
    let models = runtime.models().ok_or_else(|| failure("no model access"))?;
    let conversation_id = runtime.conversation_id();
    let stream = models.stream_simple(model, messages, options);
    let mut events = stream.events;
    let pending: Arc<Mutex<Option<AssistantMessage>>> = Arc::default();
    let in_flight: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>> = Arc::default();
    let stopped = AtomicBool::new(false);

    let outcome = async {
        let mut next_flush: Option<tokio::time::Instant> = None;
        loop {
            tokio::select! {
                biased;
                maybe_event = futures::StreamExt::next(&mut events), if !stopped.load(Ordering::SeqCst) => {
                    let Some(event) = maybe_event else { break };
                    let Ok(event) = event else { break };
                    // A partial without content, such as pi-ai's opening
                    // `start` event, shows nothing; a deferred response never
                    // gets past it, so it never leaves a partial.
                    if event.partial.content.is_empty() {
                        continue;
                    }
                    *pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) =
                        Some(event.partial);
                    let timer_empty = next_flush.is_none();
                    let in_flight_empty = in_flight
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .is_none();
                    if timer_empty && in_flight_empty {
                        next_flush = Some(
                            tokio::time::Instant::now()
                                + std::time::Duration::from_millis(PARTIAL_THROTTLE_MS),
                        );
                    }
                }
                _ = async {
                    match next_flush {
                        Some(at) => tokio::time::sleep_until(at).await,
                        None => std::future::pending().await,
                    }
                }, if next_flush.is_some() => {
                    next_flush = None;
                    let partial = pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
                    if let Some(message) = partial {
                        if !stopped.load(Ordering::SeqCst) {
                            let runtime = Arc::clone(runtime);
                            let context = context.clone();
                            let in_flight_slot = Arc::clone(&in_flight);
                            let handle = tokio::spawn(async move {
                                let commit: CommitChange = Box::new(move |tx, _current| {
                                    let live = live_draft(tx, conversation_id)?;
                                    let generation = generation_of_draft(&live)?.unwrap_or_default();
                                    if !generation.contains_key("attempt") {
                                        // `live.generation ??= {attempt}`.
                                        let mut fresh = Map::new();
                                        fresh.insert(String::from("attempt"), Value::from(attempt));
                                        set_generation(&live, fresh)?;
                                    }
                                    assign_json(
                                        &live,
                                        &[
                                            Seg::Key(String::from("generation")),
                                            Seg::Key(String::from("message")),
                                        ],
                                        &serde_json::to_value(&message).map_err(plain)?,
                                    )?;
                                    Ok(None)
                                });
                                if let Err(error) = runtime.commit(commit, context).await {
                                    // Rejections after an abort mark or close
                                    // are expected; the committed state stays
                                    // consistent.
                                    if !runtime.aborted() {
                                        runtime.report(&error);
                                    }
                                }
                                *in_flight_slot
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
                            });
                            *in_flight.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                Some(handle);
                        }
                    }
                }
            }
        }
        stream.result.await
    };
    let result = outcome.await;
    // `finally`: stop the throttle and await the commit in flight, so no
    // stale partial lands after the outcome.
    stopped.store(true, Ordering::SeqCst);
    let handle = in_flight
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(handle) = handle {
        let _ = handle.await;
    }
    result.map_err(failure)
}

/// Classify a terminal provider message in one commit that also clears the
/// partial (`classify`).
async fn classify(
    runtime: &Arc<dyn TaskRuntimeLike>,
    request: Request,
    message: AssistantMessage,
    context: &Context,
) -> Result<(), PlainFailure> {
    // An abort mark or close: the abort invocation or the reopened run
    // handles the committed state.
    runtime.throw_if_aborted().map_err(failure)?;
    let conversation_id = runtime.conversation_id();
    if message.stop_reason == StopReason::Deferred {
        if let Some(handle) = message.deferred.clone() {
            let poll_after = handle
                .poll_after_ms
                .map(|value| value as f64)
                .unwrap_or(DEFAULT_POLL_AFTER_MS as f64);
            let base = runtime.now() + poll_after;
            let poll_at = match request.poll_at {
                Some(previous) => base.max(previous + 1.0),
                None => base,
            };
            let attempt = request.attempt;
            let reference = request.model.clone();
            let tool_execution = request.tool_execution;
            let cutoff = request.cutoff;
            runtime
                .commit(
                    Box::new(move |tx, _current| {
                        let live = live_draft(tx, conversation_id)?;
                        let mut generation = Map::new();
                        generation.insert(String::from("attempt"), Value::from(attempt));
                        let mut deferred = Map::new();
                        deferred.insert(String::from("pollAt"), js_number_value(poll_at));
                        generation.insert(String::from("deferred"), Value::Object(deferred));
                        set_generation(&live, generation)?;
                        let mut checkpoint = Map::new();
                        checkpoint.insert(String::from("phase"), Value::from("poll"));
                        checkpoint.insert(String::from("attempt"), Value::from(attempt));
                        checkpoint.insert(String::from("model"), model_value(&reference));
                        checkpoint.insert(
                            String::from("toolExecution"),
                            Value::from(wire_text(&tool_execution)),
                        );
                        checkpoint.insert(String::from("cutoff"), Value::from(cutoff));
                        checkpoint.insert(
                            String::from("handle"),
                            serde_json::to_value(&handle).map_err(plain)?,
                        );
                        checkpoint.insert(String::from("pollAt"), js_number_value(poll_at));
                        Ok(Some(NextTaskState::Running {
                            checkpoint: Value::Object(checkpoint),
                        }))
                    }),
                    context.clone(),
                )
                .await
                .map_err(failure)?;
            return Ok(());
        }
    }
    {
        let hook_runtime = Arc::clone(runtime);
        let hook_context = context.clone();
        let message = message.clone();
        let capture_runtime = Arc::clone(&hook_runtime);
        hook_runtime
            .hooks(
                "afterResponse",
                Arc::new(move |hook| {
                    let runtime = Arc::clone(&capture_runtime);
                    let context = hook_context.clone();
                    let message = message.clone();
                    let hook = Arc::clone(hook);
                    Box::pin(async move {
                        hook(
                            serde_json::to_value(&message).map_err(plain)?,
                            hook_api(&runtime),
                            context.clone(),
                        )
                        .await?;
                        Ok(None)
                    })
                }),
            )
            .await
            .map_err(failure)?;
    }
    let calls: Vec<ToolCall> = message
        .content
        .iter()
        .filter_map(|block| match block {
            AssistantBlock::ToolCall(call) => Some(call.clone()),
            _ => None,
        })
        .collect();
    if message.stop_reason == StopReason::ToolUse && !calls.is_empty() {
        return start_tool_round(runtime, request, message, calls, context).await;
    }
    if matches!(
        message.stop_reason,
        StopReason::Stop | StopReason::Length | StopReason::ToolUse
    ) {
        return answer(runtime, message, context).await;
    }
    // The retry policy governs the next attempt, so it is read now rather
    // than pinned at preparation.
    let config_definition = conversation_config();
    let policy = runtime
        .snapshot(
            &config_definition.definition,
            Some(conversation_id),
            None,
            context.clone(),
        )
        .await
        .map_err(failure)?
        .and_then(|value| ConversationConfigState::from_json(&value).ok())
        .and_then(|config| config.retry)
        .unwrap_or_else(default_retry_policy);
    let retry = message.stop_reason == StopReason::Error
        && is_retryable_assistant_error(&message)
        && policy.enabled
        && request.attempt <= policy.max_retries;
    let until = if retry {
        runtime.now() + retry_delay_ms(&to_ai_policy(&policy), request.attempt.max(1) as u32) as f64
    } else {
        0.0
    };
    let attempt = request.attempt;
    runtime
        .commit(
            Box::new(move |tx, current| {
                let live = live_draft(tx, conversation_id)?;
                append_assistant(tx, conversation_id, &message)?;
                if retry {
                    let mut generation = Map::new();
                    generation.insert(String::from("attempt"), Value::from(attempt));
                    let mut retry_state = Map::new();
                    retry_state.insert(String::from("at"), js_number_value(until));
                    retry_state.insert(
                        String::from("error"),
                        Value::from(message.error_message.clone().unwrap_or_default()),
                    );
                    generation.insert(String::from("retry"), Value::Object(retry_state));
                    set_generation(&live, generation)?;
                    let mut checkpoint = Map::new();
                    checkpoint.insert(String::from("phase"), Value::from("retry"));
                    checkpoint.insert(String::from("attempt"), Value::from(attempt));
                    checkpoint.insert(String::from("until"), js_number_value(until));
                    return Ok(Some(NextTaskState::Running {
                        checkpoint: Value::Object(checkpoint),
                    }));
                }
                let text = message.error_message.clone().unwrap_or_else(|| {
                    format!(
                        "Model response ended with stop reason {}",
                        wire_text(&message.stop_reason)
                    )
                });
                end_run(
                    tx,
                    &live,
                    current.id,
                    SubmissionSettlement::Unanswered {
                        reason: String::from("model_error"),
                        detail: Some(Value::String(text.clone())),
                    },
                )?;
                Ok(Some(NextTaskState::Terminal {
                    outcome: TaskOutcome::Failed {
                        error: TaskOutcomeError {
                            message: text,
                            detail: Some(serde_json::json!({"reason": "model_error"})),
                        },
                        result: None,
                    },
                }))
            }),
            context.clone(),
        )
        .await
        .map_err(failure)
}

/// The port's retry-policy JSON shape as the pi-ai delay calculator's input.
fn to_ai_policy(policy: &super::super::harness::types::ConversationRetryPolicy) -> RetryPolicy {
    RetryPolicy {
        enabled: policy.enabled,
        max_retries: policy.max_retries.max(0) as u32,
        base_delay_ms: policy.base_delay_ms.max(0.0) as u64,
        max_agent_delay_ms: policy.max_agent_delay_ms.map(|value| value.max(0.0) as u64),
    }
}

/// A final answer (`answer`); the final boundary places queued items (spec
/// §6). The first `onYield` continuation appends a user message and hands the
/// run to a successor generation, but only when the boundary selected no user
/// item and no reset. Otherwise the run's inputs settle `done`, and selected
/// user items start the next run.
async fn answer(
    runtime: &Arc<dyn TaskRuntimeLike>,
    message: AssistantMessage,
    context: &Context,
) -> Result<(), PlainFailure> {
    let continuation: Arc<Mutex<Option<StringOrBlocks>>> = Arc::default();
    {
        let hook_runtime = Arc::clone(runtime);
        let hook_context = context.clone();
        let message = message.clone();
        let continuation = Arc::clone(&continuation);
        let capture_runtime = Arc::clone(&hook_runtime);
        hook_runtime
            .hooks(
                "onYield",
                Arc::new(move |hook| {
                    let runtime = Arc::clone(&capture_runtime);
                    let context = hook_context.clone();
                    let message = message.clone();
                    let continuation = Arc::clone(&continuation);
                    let hook = Arc::clone(hook);
                    Box::pin(async move {
                        if continuation
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .is_some()
                        {
                            return Ok(None);
                        }
                        let payload = serde_json::to_value(&message).map_err(plain)?;
                        let replaced = hook(payload, hook_api(&runtime), context.clone()).await?;
                        if let Some(value) =
                            replaced.and_then(|value| value.get("continue").cloned())
                        {
                            let parsed: StringOrBlocks =
                                serde_json::from_value(value).map_err(plain)?;
                            *continuation
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(parsed);
                        }
                        Ok(None)
                    })
                }),
            )
            .await
            .map_err(failure)?;
    }
    let continuation = continuation
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    let conversation_id = runtime.conversation_id();
    let now = runtime.now();
    runtime
        .commit(
            Box::new(move |tx, current| {
                let mut boundary = prepare_boundary(tx, conversation_id)?;
                let live = live_draft(tx, conversation_id)?;
                let entry = append_assistant(tx, conversation_id, &message)?;
                let result = Some(NextTaskState::Terminal {
                    outcome: TaskOutcome::Completed {
                        result: generation_result(entry.id),
                    },
                });
                let placed = apply_boundary(tx, &mut boundary, At::Final, now)?;
                if let Some(continuation) = &continuation {
                    if placed.users.is_empty() && !placed.reset {
                        let mut draft =
                            EntryDraft::new(super::super::entries::USER_ENTRY_ENTRY_KIND);
                        draft.model = Some(vec![Message::User(UserMessage {
                            content: continuation.clone(),
                            timestamp: now as i64,
                        })]);
                        tx.append_entry(conversation_id, draft)?;
                        hand_over(&live, current.id, create_generation(tx, conversation_id)?)?;
                        delete_generation(&live)?;
                        return Ok(result);
                    }
                }
                end_run(
                    tx,
                    &live,
                    current.id,
                    SubmissionSettlement::Done { answer: entry.id },
                )?;
                if !placed.users.is_empty() {
                    start_run(tx, conversation_id, &live, placed.users)?;
                }
                Ok(result)
            }),
            context.clone(),
        )
        .await
        .map_err(failure)
}

/// Append the tool-calling answer and start its tool round in one commit
/// (spec §8.3, `startToolRound`).
async fn start_tool_round(
    runtime: &Arc<dyn TaskRuntimeLike>,
    request: Request,
    message: AssistantMessage,
    calls: Vec<ToolCall>,
    context: &Context,
) -> Result<(), PlainFailure> {
    let conversation_id = runtime.conversation_id();
    let messages = match request.messages.clone() {
        Some(messages) => messages,
        None => {
            runtime
                .context(conversation_id, context.clone(), Some(request.cutoff))
                .await
                .map_err(failure)?
                .messages
        }
    };
    let offered: BTreeSet<String> = get_current_tools(&messages)
        .into_iter()
        .map(|tool| tool.name)
        .collect();
    let registry = runtime.registry();
    let sequential = request.tool_execution == ToolExecutionMode::Sequential
        || calls.iter().any(|call| {
            offered.contains(&call.name)
                && registry
                    .tool(&call.name)
                    .is_some_and(|tool| tool.execution_mode == Some(ToolExecutionMode::Sequential))
        });
    let now = runtime.now();
    let runtime_task_id = runtime.task_id();
    let task_token = tool_task();
    runtime
        .commit(
            Box::new(move |tx, _current| {
                let live = live_draft(tx, conversation_id)?;
                let entry = append_assistant(tx, conversation_id, &message)?;
                let mut slots: Vec<ToolSlot> = Vec::new();
                let mut tools: Vec<TaskId> = Vec::new();
                let mut pending: Vec<String> = Vec::new();
                for call in &calls {
                    if !offered.contains(&call.name) {
                        let unavailable = harness_error(
                            "tool_unavailable",
                            format!("Tool {} is not available", call.name),
                        );
                        let result =
                            append_tool_result(tx, conversation_id, call, &unavailable, now)?;
                        slots.push(ToolSlot {
                            call_id: call.id.clone(),
                            name: call.name.clone(),
                            task_id: None,
                            status: super::super::harness::live::SlotStatus::Done,
                            entry: Some(result.id),
                            ..Default::default()
                        });
                        continue;
                    }
                    if sequential && !tools.is_empty() {
                        pending.push(call.id.clone());
                        slots.push(ToolSlot {
                            call_id: call.id.clone(),
                            name: call.name.clone(),
                            task_id: None,
                            status: super::super::harness::live::SlotStatus::Pending,
                            ..Default::default()
                        });
                        continue;
                    }
                    let task_id =
                        create_tool_task(tx, &task_token, runtime_task_id, entry.id, &call.id)?;
                    tools.push(task_id);
                    slots.push(ToolSlot {
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                        task_id: Some(task_id),
                        status: super::super::harness::live::SlotStatus::Pending,
                        ..Default::default()
                    });
                }
                delete_generation(&live)?;
                set_tools(&live, &slots)?;
                let mut checkpoint = Map::new();
                checkpoint.insert(String::from("phase"), Value::from("tools"));
                checkpoint.insert(String::from("assistant"), Value::from(entry.id));
                checkpoint.insert(
                    String::from("tools"),
                    Value::Array(tools.iter().map(|id| Value::from(*id)).collect()),
                );
                checkpoint.insert(
                    String::from("pending"),
                    Value::Array(
                        pending
                            .iter()
                            .map(|item| Value::from(item.as_str()))
                            .collect(),
                    ),
                );
                Ok(Some(NextTaskState::Waiting {
                    checkpoint: Value::Object(checkpoint),
                    on: tools,
                    policy: JoinPolicy::AllSettled,
                }))
            }),
            context.clone(),
        )
        .await
        .map_err(failure)
}

/// The round's tools are terminal: apply their controls and either end the
/// run at the final boundary (`terminate`, `handoff`, or a queued reset) or
/// hand it to the next generation at the `postTools` boundary (spec §8.5,
/// `finishToolRound`).
async fn finish_tool_round(
    runtime: &Arc<dyn TaskRuntimeLike>,
    assistant: EntryId,
    tools: Vec<TaskId>,
    context: &Context,
) -> Result<(), PlainFailure> {
    let conversation_id = runtime.conversation_id();
    let outcomes = runtime
        .outcomes(tools.clone(), context.clone())
        .await
        .map_err(failure)?;
    let controls: Vec<Option<ToolControl>> = outcomes
        .iter()
        .map(|outcome| match outcome {
            TaskOutcome::Completed { result } => result
                .get("control")
                .cloned()
                .and_then(|control| serde_json::from_value(control).ok()),
            _ => None,
        })
        .collect();
    let control_of = |task_id: TaskId| -> Option<ToolControl> {
        tools
            .iter()
            .position(|id| *id == task_id)
            .and_then(|index| controls.get(index).cloned().flatten())
    };
    let live_definition = live_doc();
    let slots = runtime
        .snapshot(
            &live_definition.definition,
            Some(conversation_id),
            None,
            context.clone(),
        )
        .await
        .map_err(failure)?
        .map(|live| {
            live.get("tools")
                .cloned()
                .map(|tools| parse_slots(&tools))
                .unwrap_or_default()
        })
        .unwrap_or_default();
    let results: Vec<EntryId> = slots.iter().filter_map(|slot| slot.entry).collect();
    {
        let hook_runtime = Arc::clone(runtime);
        let hook_context = context.clone();
        let results_for_hook = results.clone();
        let capture_runtime = Arc::clone(&hook_runtime);
        hook_runtime
            .hooks(
                "afterTools",
                Arc::new(move |hook| {
                    let runtime = Arc::clone(&capture_runtime);
                    let context = hook_context.clone();
                    let results = results_for_hook.clone();
                    let hook = Arc::clone(hook);
                    Box::pin(async move {
                        let payload = serde_json::json!({
                            "assistant": assistant,
                            "results": results,
                        });
                        hook(payload, hook_api(&runtime), context.clone()).await?;
                        Ok(None)
                    })
                }),
            )
            .await
            .map_err(failure)?;
    }
    // Every call of the round, including those answered without a task, must
    // ask to terminate.
    let terminate = !slots.is_empty()
        && slots.iter().all(|slot| {
            slot.task_id.is_some_and(|task_id| {
                control_of(task_id).is_some_and(|control| control.terminate == Some(true))
            })
        });
    let mut added: Vec<String> = Vec::new();
    for control in controls.iter().flatten() {
        if let Some(add_tools) = &control.add_tools {
            added.extend(add_tools.iter().cloned());
        }
    }
    // The last handoff in call order wins.
    let handoff = controls
        .iter()
        .rev()
        .flatten()
        .find_map(|control| control.handoff.clone());
    let now = runtime.now();
    runtime
        .commit(
            Box::new(move |tx, current| {
                let mut boundary = prepare_boundary(tx, conversation_id)?;
                if !added.is_empty() {
                    let config = conversation_config();
                    let draft = tx.doc(&config.definition, Some(conversation_id), None, None)?;
                    for name in &added {
                        let active = &[crate::chord::delta::Seg::Key(String::from("activeTools"))];
                        let existing: Vec<String> = draft
                            .read(active)
                            .map_err(|error| plain(error.message()))?
                            .and_then(|value| value.as_array().cloned())
                            .map(|items| {
                                items
                                    .into_iter()
                                    .filter_map(|item| item.as_str().map(str::to_owned))
                                    .collect()
                            })
                            .unwrap_or_default();
                        if !existing.contains(name) {
                            draft
                                .push(active, vec![Value::from(name.clone())])
                                .map_err(|error| plain(error.message()))?;
                        }
                    }
                }
                let live = live_draft(tx, conversation_id)?;
                if terminate || handoff.is_some() {
                    if let Some(handoff) = &handoff {
                        let mut draft = EntryDraft::new(super::super::entries::RESET_ENTRY_KIND);
                        draft.head = Some(EntryHead::Self_);
                        draft.model = Some(vec![Message::User(UserMessage {
                            content: StringOrBlocks::Text(handoff.clone()),
                            timestamp: now as i64,
                        })]);
                        let entry = tx.append_entry(conversation_id, draft)?;
                        boundary.head = Some(entry.id);
                    }
                    let placed = apply_boundary(tx, &mut boundary, At::Final, now)?;
                    end_run(
                        tx,
                        &live,
                        current.id,
                        SubmissionSettlement::Done { answer: assistant },
                    )?;
                    if !placed.users.is_empty() {
                        start_run(tx, conversation_id, &live, placed.users)?;
                    }
                } else {
                    let placed = apply_boundary(tx, &mut boundary, At::PostTools, now)?;
                    if placed.reset {
                        // The queued reset cut the run's context before an
                        // answer.
                        end_run(
                            tx,
                            &live,
                            current.id,
                            SubmissionSettlement::Unanswered {
                                reason: String::from("reset"),
                                detail: None,
                            },
                        )?;
                        if !placed.users.is_empty() {
                            start_run(tx, conversation_id, &live, placed.users)?;
                        }
                    } else {
                        delete_tools(&live)?;
                        if let Some((run_task_id, mut inputs)) = run_of_draft(&live)? {
                            if run_task_id == current.id {
                                inputs.extend(placed.users.iter().copied());
                                write_run(&live, run_task_id, inputs)?;
                            }
                        }
                        hand_over(&live, current.id, create_generation(tx, conversation_id)?)?;
                    }
                }
                Ok(Some(NextTaskState::Terminal {
                    outcome: TaskOutcome::Completed {
                        result: generation_result(assistant),
                    },
                }))
            }),
            context.clone(),
        )
        .await
        .map_err(failure)
}

/// Parse a stored `tools` array.
fn parse_slots(value: &Value) -> Vec<ToolSlot> {
    value
        .as_array()
        .map(|slots| slots.iter().filter_map(ToolSlot::from_json).collect())
        .unwrap_or_default()
}

/// Append a provider result and add its usage to `pi.usage` in the same
/// commit (`appendAssistant`). REMINDER: every built-in writer of assistant
/// entries goes through here, so the usage ledger stays complete.
pub fn append_assistant(
    tx: &Transaction,
    conversation_id: ConversationId,
    message: &AssistantMessage,
) -> Result<super::super::types::EntryRecord, PlainError> {
    record_usage(
        tx,
        conversation_id,
        "models",
        &format!("{}/{}", message.provider, message.model),
        &message.usage,
    )?;
    let assistant = super::super::entries::assistant_entry();
    let mut draft = EntryDraft::new(assistant.kind.clone());
    draft.model = Some(vec![Message::Assistant(message.clone())]);
    tx.append_entry(conversation_id, draft)
}

/// Start a run for `inputs`, placed input submissions: a new generation takes
/// `pi.live.run` (`startRun`).
pub fn start_run(
    tx: &Transaction,
    conversation_id: ConversationId,
    live: &DocumentDraft,
    inputs: Vec<SubmissionId>,
) -> Result<(), PlainError> {
    let task_id = create_generation(tx, conversation_id)?;
    write_run(live, task_id, inputs)
}

/// Write `run = {taskId, inputs}` (the `live.run = {...}` literal).
fn write_run(
    live: &DocumentDraft,
    task_id: TaskId,
    inputs: Vec<SubmissionId>,
) -> Result<(), PlainError> {
    let mut run = Map::new();
    run.insert(String::from("taskId"), Value::from(task_id));
    run.insert(
        String::from("inputs"),
        Value::Array(inputs.iter().map(|id| Value::from(*id)).collect()),
    );
    live.set(&[Seg::Key(String::from("run"))], Value::Object(run))
        .map_err(|error| plain(error.message()))
}

/// A generation owned by its conversation (`createGeneration`).
fn create_generation(
    tx: &Transaction,
    conversation_id: ConversationId,
) -> Result<TaskId, PlainError> {
    let token = generation_task();
    tx.create_task(
        &token,
        Value::Object(Map::new()),
        TaskOptions {
            ownership: TaskOwnership::Conversation,
            conversation_id: Some(conversation_id),
            background: None,
        },
    )
}

/// Hand run control from `from` to `to`; the run's inputs move with it
/// (`handOver`).
pub fn hand_over(live: &DocumentDraft, from: TaskId, to: TaskId) -> Result<(), PlainError> {
    if run_of_draft(live)?.map(|(task_id, _)| task_id) == Some(from) {
        live.set(
            &[
                Seg::Key(String::from("run")),
                Seg::Key(String::from("taskId")),
            ],
            Value::from(to),
        )
        .map_err(|error| plain(error.message()))?;
    }
    Ok(())
}

/// The `HookApi` hooks receive, built from the invocation runtime.
fn hook_api(runtime: &Arc<dyn TaskRuntimeLike>) -> HookApi {
    let memo_runtime = Arc::clone(runtime);
    HookApi {
        task_id: runtime.task_id(),
        conversation_id: runtime.conversation_id(),
        memo: Arc::new(move |name, candidate, context| {
            let runtime = Arc::clone(&memo_runtime);
            let name = name.to_owned();
            Box::pin(async move { runtime.memo(&name, candidate, context).await })
        }),
        read: runtime.session(),
    }
}

/// Find a slot index by call id (`live.tools?.find(slot => slot.callId === …)`).
fn find_slot_by_call(live: &DocumentDraft, call_id: &str) -> Result<Option<usize>, PlainError> {
    Ok(tools_of_draft(live)?
        .iter()
        .position(|slot| slot.call_id == call_id))
}
