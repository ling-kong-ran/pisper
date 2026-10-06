//! Port of `src/harness/tool.ts`: the built-in `pi.tool` task — it resolves
//! the called tool from its phase snapshot, validates, runs `beforeTool`,
//! records intent, executes, runs `afterTool`, and appends the result, all in
//! one `call` handler so nothing separates resolution from settlement.
//! `execute` is reached only by recovery and applies the replay rule.
//!
//! Divergences (structural, disclosed):
//! - arguments are validated against the pi-ai tool declaration through the
//!   port's [`crate::ai::validation`] surface (upstream
//!   `validateToolArguments`);
//! - `ToolDiagnostic` serializes `{severity, code?, message}` (D14) and the
//!   result entry's message carries the rendered diagnostics block, so the
//!   stored bytes match the upstream literal construction.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};

use crate::agent_core::chord_support::context::Context;
use crate::ai::types::{
    AssistantBlock, Message, TextContent, TextOrImageBlock, ToolCall, ToolResultMessage,
};
use crate::ai::validation;

use super::super::entries::tool_result_entry;
use super::super::errors::PlainError;
use super::super::harness::json::assign_json;
use super::super::harness::live::{
    clear_progress, finish_slot, live_doc, push_slot_diagnostics, set_slot_field, set_slot_running,
    slot_index_of, tools_of_draft, ToolSlot,
};
use super::super::harness::output::{bound_output, OutputBuffer, OutputLimits, Progress};
use super::super::harness::types::{
    OutputRetain, ToolDiagnostic, ToolDiagnosticSeverity, ToolExecutionApiLike,
    ToolExecutionResult, ToolReplay,
};
use super::super::harness::usage::{js_number_value, record_usage};
use super::super::ids::{ConversationId, TaskId};
use super::super::session::transaction::Transaction;
use super::super::tasks::{
    define_task, NextTaskState, PhaseArgs, PhaseFn, PlainFailure, TaskDefinition, TaskRuntimeLike,
};
use super::super::truncate::{utf8_byte_length, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES};
use super::super::types::{EntryDraft, TaskOptions, TaskOutcome, TaskOutcomeError};
use crate::chord::delta::overlap;

type Seg = crate::chord::delta::Seg;

fn failure(error: impl std::fmt::Display) -> PlainFailure {
    PlainFailure::new(error.to_string())
}

fn plain(error: impl std::fmt::Display) -> PlainError {
    PlainError::new(error.to_string())
}

/// The built-in tool task (`ToolTask`).
pub fn tool_task() -> super::super::tasks::TaskToken {
    let mut phases: std::collections::BTreeMap<String, PhaseFn> = std::collections::BTreeMap::new();
    phases.insert(String::from("call"), boxed(call_phase));
    phases.insert(String::from("execute"), boxed(execute_phase));
    define_task(TaskDefinition {
        name: String::from("pi.tool"),
        version: 1,
        initial: Arc::new(|| {
            let mut checkpoint = Map::new();
            checkpoint.insert(String::from("phase"), Value::from("call"));
            checkpoint
        }),
        phases,
        abort: Some(boxed(abort_phase)),
        migrate: None,
    })
}

fn checkpoint_of(args: &PhaseArgs) -> Map<String, Value> {
    super::super::harness::scheduler::record_checkpoint(&args.record)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// How a tool task ends; the result entry is appended either way (`Ending`).
/// `failed` (execution threw or was interrupted) records cancellation intent
/// for the conversations the call owns; a result with `isError` still
/// completes.
#[derive(Debug, Clone)]
enum Ending {
    Completed,
    Aborted,
    Failed { message: String },
}

/// Read the stored checkpoint `{phase: "execute", arguments, replay}`.
fn execute_intent(checkpoint: &Map<String, Value>) -> Option<(Map<String, Value>, String)> {
    let arguments = checkpoint.get("arguments")?.as_object()?.clone();
    let replay = checkpoint
        .get("replay")
        .and_then(Value::as_str)
        .unwrap_or("unsafe")
        .to_owned();
    Some((arguments, replay))
}

// ─── Phase: call ────────────────────────────────────────────────────────────

async fn call_phase(args: PhaseArgs) -> Result<(), PlainFailure> {
    let runtime = Arc::clone(&args.runtime);
    let context = args.context.clone();
    let input = args.record.input.clone();
    let call = read_call(&runtime, &input, &context).await?;
    let registry = runtime.registry();
    let Some(tool) = registry.tool(&call.name) else {
        let error = harness_error(
            "tool_unavailable",
            format!("Tool {} is not available", call.name),
        );
        return settle(
            &runtime,
            &call,
            Ending::Completed,
            move |_slot| error,
            &context,
        )
        .await;
    };
    let prepared = prepare(
        &tool,
        call.arguments.as_object().cloned().unwrap_or_default(),
    );
    let checked = match prepared {
        Checked::Args(args) => validate(&tool, &call, args),
        Checked::Error(error) => Checked::Error(error),
    };
    let Checked::Args(prepared_args) = checked else {
        let Checked::Error(error) = checked else {
            unreachable!()
        };
        let result = invalid(error);
        return settle(
            &runtime,
            &call,
            Ending::Completed,
            move |_slot| result,
            &context,
        )
        .await;
    };
    let args = prepared_args.clone();
    let blocked: Arc<Mutex<Option<String>>> = Arc::default();
    {
        let runtime = Arc::clone(&runtime);
        let context = context.clone();
        let call = call.clone();
        let hook_args = Arc::new(Mutex::new(args.clone()));
        let closure_args = Arc::clone(&hook_args);
        let closure_blocked = Arc::clone(&blocked);
        let hook_runtime = Arc::clone(&runtime);
        let hook_context = context.clone();
        let hook_call = call.clone();
        runtime
            .hooks(
                "beforeTool",
                Arc::new(move |hook| {
                    let runtime = Arc::clone(&hook_runtime);
                    let context = hook_context.clone();
                    let call = hook_call.clone();
                    let args = Arc::clone(&closure_args);
                    let blocked_slot = Arc::clone(&closure_blocked);
                    let hook = Arc::clone(hook);
                    Box::pin(async move {
                        if blocked_slot
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .is_some()
                        {
                            return Ok(None);
                        }
                        let mut payload = Map::new();
                        payload.insert(String::from("id"), Value::from(call.id.clone()));
                        payload.insert(String::from("name"), Value::from(call.name.clone()));
                        payload.insert(
                            String::from("arguments"),
                            Value::Object(
                                args.lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                    .clone(),
                            ),
                        );
                        let decision =
                            hook(Value::Object(payload), hook_api(&runtime), context.clone())
                                .await?;
                        if let Some(decision) = decision {
                            if let Some(block) =
                                decision.get("block").filter(|value| !value.is_null())
                            {
                                *blocked_slot
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                    Some(error_text_value(block));
                            } else if let Some(next) =
                                decision.get("arguments").filter(|value| !value.is_null())
                            {
                                *args.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) =
                                    next.as_object().cloned().unwrap_or_default();
                            }
                        }
                        Ok(None)
                    })
                }),
            )
            .await
            .map_err(failure)?;
        let _args = {
            let guard = hook_args
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.clone()
        };
    }
    let block = blocked
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    if let Some(block) = block {
        let blocked = harness_error("blocked", format!("Tool call blocked: {block}"));
        return settle(
            &runtime,
            &call,
            Ending::Completed,
            move |_slot| blocked,
            &context,
        )
        .await;
    }
    let validated = validate(&tool, &call, args);
    let Checked::Args(final_args) = validated else {
        let Checked::Error(error) = validated else {
            unreachable!()
        };
        let result = invalid(error);
        return settle(
            &runtime,
            &call,
            Ending::Completed,
            move |_slot| result,
            &context,
        )
        .await;
    };
    let conversation_id = runtime.conversation_id();
    let task_id = runtime.task_id();
    let replay = match tool.replay {
        Some(ToolReplay::Safe) => "safe",
        None | Some(ToolReplay::Unsafe) => "unsafe",
    };
    let intent_args = final_args.clone();
    runtime
        .commit(
            Box::new(move |tx, _current| {
                let live = live_doc();
                let draft = tx.doc(&live.definition, Some(conversation_id), None, None)?;
                if let Some(index) = slot_index_of(&draft, task_id)? {
                    set_slot_running(&draft, index)?;
                }
                let mut intent = Map::new();
                intent.insert(String::from("phase"), Value::from("execute"));
                intent.insert(String::from("arguments"), Value::Object(intent_args));
                intent.insert(String::from("replay"), Value::from(replay));
                Ok(Some(NextTaskState::Running {
                    checkpoint: Value::Object(intent),
                }))
            }),
            context.clone(),
        )
        .await
        .map_err(failure)?;
    run(&runtime, &call, &tool, final_args, &context).await
}

// ─── Phase: execute (recovery) ──────────────────────────────────────────────

/// Recovery after intent: rerun only when the stored and the current policy
/// both say `safe`.
async fn execute_phase(args: PhaseArgs) -> Result<(), PlainFailure> {
    let runtime = Arc::clone(&args.runtime);
    let context = args.context.clone();
    let checkpoint = checkpoint_of(&args);
    let Some((arguments, replay)) = execute_intent(&checkpoint) else {
        return Err(failure("Tool execute checkpoint carries no intent"));
    };
    let input = args.record.input.clone();
    let call = read_call(&runtime, &input, &context).await?;
    let registry = runtime.registry();
    let tool = registry.tool(&call.name);
    if replay == "safe"
        && tool
            .as_ref()
            .is_some_and(|tool| tool.replay == Some(ToolReplay::Safe))
    {
        // The rerun reports from scratch; clear what the interrupted attempt
        // published.
        let conversation_id = runtime.conversation_id();
        let task_id = runtime.task_id();
        runtime
            .commit(
                Box::new(move |tx, _current| {
                    let live = live_doc();
                    let draft = tx.doc(&live.definition, Some(conversation_id), None, None)?;
                    if let Some(index) = slot_index_of(&draft, task_id)? {
                        clear_progress(&draft, index)?;
                    }
                    Ok(None)
                }),
                context.clone(),
            )
            .await
            .map_err(failure)?;
        let Some(tool) = tool else {
            return Err(failure("Tool vanished between replay checks"));
        };
        return run(&runtime, &call, &tool, arguments, &context).await;
    }
    let message = format!(
        "Tool {} was interrupted and may have partially run",
        call.name
    );
    // `failed` records cancellation intent, so the call's owned conversations,
    // left unsupervised, are aborted.
    let ending = Ending::Failed {
        message: message.clone(),
    };
    settle(
        &runtime,
        &call,
        ending,
        move |slot| from_slot(slot, "interrupted", &message),
        &context,
    )
    .await
}

// ─── Abort handler ──────────────────────────────────────────────────────────

async fn abort_phase(args: PhaseArgs) -> Result<(), PlainFailure> {
    let runtime = Arc::clone(&args.runtime);
    let context = args.context.clone();
    let input = args.record.input.clone();
    let call = read_call(&runtime, &input, &context).await?;
    let message = format!("Tool {} was aborted", call.name);
    let ending = Ending::Aborted;
    settle(
        &runtime,
        &call,
        ending,
        move |slot| from_slot(slot, "aborted", &message),
        &context,
    )
    .await
}

// ─── Shared helpers ─────────────────────────────────────────────────────────

/// The tool call `call_id` of the assistant entry (`readCall`).
async fn read_call(
    runtime: &Arc<dyn TaskRuntimeLike>,
    input: &Value,
    context: &Context,
) -> Result<ToolCall, PlainFailure> {
    let assistant = input.get("assistant").and_then(Value::as_i64).unwrap_or(0);
    let call_id = input
        .get("callId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let entry = runtime
        .entry(
            Some(String::from("pi.assistant")),
            assistant,
            context.clone(),
        )
        .await
        .map_err(failure)?;
    let call = entry
        .and_then(|entry| entry.model)
        .and_then(|messages| messages.into_iter().next())
        .and_then(|message| match message {
            Message::Assistant(assistant) => {
                assistant.content.into_iter().find_map(|block| match block {
                    AssistantBlock::ToolCall(call) if call.id == call_id => Some(call),
                    _ => None,
                })
            }
            _ => None,
        });
    call.ok_or_else(|| failure(format!("Entry {assistant} has no tool call {call_id}")))
}

/// Arguments, or why they are invalid (`Checked`).
enum Checked {
    Args(Map<String, Value>),
    Error(String),
}

/// The call's arguments as repaired by the tool; a throwing repair makes them
/// invalid (`prepare`).
fn prepare(
    tool: &Arc<super::super::harness::types::ToolRegistration>,
    args: Map<String, Value>,
) -> Checked {
    match &tool.prepare_arguments {
        None => Checked::Args(args),
        Some(repair) => {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| repair(&args))) {
                Ok(repaired) => Checked::Args(repaired),
                Err(_) => Checked::Error("prepareArguments failed".to_owned()),
            }
        }
    }
}

/// Arguments validated and coerced against the implementation's schema
/// (`validate`).
fn validate(
    tool: &Arc<super::super::harness::types::ToolRegistration>,
    call: &ToolCall,
    args: Map<String, Value>,
) -> Checked {
    let mut call = call.clone();
    call.arguments = Value::Object(args);
    match validation::validate_tool_arguments(&tool.tool, &call) {
        Ok(Value::Object(args)) => Checked::Args(args),
        Ok(Value::Null) => Checked::Args(Map::new()),
        Ok(other) => match other.as_object() {
            Some(args) => Checked::Args(args.clone()),
            None => Checked::Args(Map::new()),
        },
        Err(error) => Checked::Error(error),
    }
}

fn invalid(message: String) -> ToolExecutionResult {
    harness_error("invalid_arguments", message)
}

/// What a running tool reported through its api: output, the last details,
/// and diagnostics (`Reported`).
struct Reported {
    output: Arc<OutputBuffer>,
    limits: OutputLimits,
    diagnostics: Arc<Mutex<Vec<ToolDiagnostic>>>,
    details: Arc<Mutex<Option<Value>>>,
}

/// Execute with the resolved implementation, then settle its result (`run`).
async fn run(
    runtime: &Arc<dyn TaskRuntimeLike>,
    call: &ToolCall,
    tool: &Arc<super::super::harness::types::ToolRegistration>,
    args: Map<String, Value>,
    context: &Context,
) -> Result<(), PlainFailure> {
    let limits = OutputLimits {
        max_bytes: tool
            .output_limits
            .and_then(|limits| limits.max_bytes)
            .unwrap_or(DEFAULT_MAX_BYTES),
        max_lines: tool
            .output_limits
            .and_then(|limits| limits.max_lines)
            .unwrap_or(DEFAULT_MAX_LINES),
        retain: tool
            .output_limits
            .and_then(|limits| limits.retain)
            .unwrap_or(OutputRetain::Head),
    };
    let reported = Arc::new(Reported {
        output: Arc::new(OutputBuffer::new(limits)),
        limits,
        diagnostics: Arc::new(Mutex::new(Vec::new())),
        details: Arc::new(Mutex::new(None)),
    });
    let progress = publish_progress(runtime, &reported, context);
    let conversation_id = runtime.conversation_id();
    let task_id = runtime.task_id();
    let call_id = call.id.clone();
    let models = runtime.models();
    let env = runtime.env();
    let session = runtime.session();
    let ended = Arc::new(AtomicBool::new(false));
    let api: Arc<dyn ToolExecutionApiLike> = Arc::new(ToolApi {
        task_id,
        conversation_id,
        call_id,
        env,
        models,
        session,
        reported: Arc::clone(&reported),
        progress: Arc::clone(&progress),
        ended: Arc::clone(&ended),
        memo_runtime: Arc::clone(runtime),
    });
    let mut ending = Ending::Completed;
    let outcome = (tool.execute)(args, Arc::clone(&api), context.clone()).await;
    let result = match outcome {
        Ok(result) => result,
        Err(error) => {
            if runtime.aborted() {
                ended.store(true, Ordering::SeqCst);
                for waiter in progress.stop().await {
                    let _ = waiter.send(Err(error.message.clone()));
                }
                return Err(failure(error.message));
            }
            let mut diagnostics = reported
                .diagnostics
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            diagnostics.push(tool_diagnostic("tool_error", error.message.clone()));
            drop(diagnostics);
            // A throw ends the task `failed`, which cancels what the call
            // owned; it no longer supervises it. The error text is already in
            // the result entry.
            ending = Ending::Failed {
                message: format!("Tool {} threw", call.name),
            };
            ToolExecutionResult {
                content: None,
                is_error: Some(true),
                details: None,
                diagnostics: None,
                usage: None,
                control: None,
            }
        }
    };
    ended.store(true, Ordering::SeqCst);
    reported.output.end();
    // Details still waiting for a progress commit settle with the terminal
    // commit, the final flush.
    let pending = progress.stop().await;
    let settled = final_result(runtime, call, result, &reported, context).await;
    match settled {
        Ok(settled) => {
            let settle_outcome = settle(runtime, call, ending, move |_slot| settled, context).await;
            for waiter in pending {
                let _ = waiter.send(Ok(()));
            }
            settle_outcome
        }
        Err(error) => {
            for waiter in pending {
                let _ = waiter.send(Err(error.message.clone()));
            }
            Err(failure(error.message))
        }
    }
}

/// The `ToolExecutionApi` the tool's execute receives (`api` in `run`).
struct ToolApi {
    task_id: TaskId,
    conversation_id: ConversationId,
    call_id: String,
    // Retained for wrapper passthrough: a wrapped tool may supply its own
    // env/models surfaces.
    env: Option<Arc<dyn super::super::env::ExecutionEnv>>,
    #[allow(dead_code)]
    models: Option<Arc<dyn super::super::harness::types::ModelsHandle>>,
    // The session read surface the erased commits use.
    #[allow(dead_code)]
    session: Arc<super::super::session::session::Session>,
    reported: Arc<Reported>,
    progress: Arc<Progress>,
    ended: Arc<AtomicBool>,
    memo_runtime: Arc<dyn TaskRuntimeLike>,
}

impl ToolApi {
    fn assert_live(&self) -> Result<(), PlainError> {
        if self.ended.load(Ordering::SeqCst) {
            return Err(plain("The tool call has settled"));
        }
        Ok(())
    }
}

impl ToolExecutionApiLike for ToolApi {
    fn task_id(&self) -> TaskId {
        self.task_id
    }

    fn conversation_id(&self) -> ConversationId {
        self.conversation_id
    }

    fn call_id(&self) -> &str {
        &self.call_id
    }

    fn env(&self) -> Option<Arc<dyn super::super::env::ExecutionEnv>> {
        self.env.clone()
    }

    fn output(&self, chunk: &[u8]) -> Result<(), PlainError> {
        self.assert_live()?;
        if self.reported.output.push(chunk) {
            self.progress.mark();
        }
        Ok(())
    }

    fn diagnostic(&self, diagnostic: ToolDiagnostic) -> Result<(), PlainError> {
        self.assert_live()?;
        let wire = serde_json::to_value(&diagnostic).map_err(plain)?;
        let value: ToolDiagnostic = serde_json::from_value(wire).map_err(plain)?;
        self.reported
            .diagnostics
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(value);
        self.progress.mark();
        Ok(())
    }

    fn details(
        &self,
        value: Value,
        context: Context,
    ) -> super::super::harness::types::ApiFuture<()> {
        let reported = Arc::clone(&self.reported);
        let progress = Arc::clone(&self.progress);
        let ended = Arc::clone(&self.ended);
        Box::pin(async move {
            if ended.load(Ordering::SeqCst) {
                return Err(plain("The tool call has settled"));
            }
            if let Some(signal) = context.abort_signal() {
                if signal.is_cancelled() {
                    return Err(plain("The operation was aborted"));
                }
            }
            *reported
                .details
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(value);
            let committed = progress.mark_and_wait();
            // Cancelling the wait leaves the update in place; the commit's own
            // outcome stays observed.
            match context.abort_signal() {
                Some(signal) => {
                    tokio::select! {
                        result = committed => match result {
                            Ok(status) => status.map_err(plain),
                            Err(_) => Ok(()),
                        },
                        _ = signal.cancelled() => Ok(()),
                    }
                }
                None => {
                    let status = committed
                        .await
                        .map_err(|_| plain("The tool call has settled"));
                    match status {
                        Ok(status) => status.map_err(plain),
                        Err(error) => Err(error),
                    }
                }
            }
        })
    }

    fn commit(
        &self,
        change: super::super::harness::types::ErasedCommitChange,
        context: Context,
    ) -> super::super::harness::types::ApiFuture<Value> {
        let runtime = Arc::clone(&self.memo_runtime);
        let ended = Arc::clone(&self.ended);
        Box::pin(async move {
            if ended.load(Ordering::SeqCst) {
                return Err(plain("The tool call has settled"));
            }
            let result: Arc<Mutex<Option<Value>>> = Arc::default();
            let slot = Arc::clone(&result);
            let commit: super::super::tasks::CommitChange = Box::new(move |tx, _current| {
                *slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(change(tx)?);
                Ok(None)
            });
            runtime.commit(commit, context).await?;
            let taken = result
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
                .unwrap_or(Value::Null);
            Ok(taken)
        })
    }

    fn memo(
        &self,
        name: &str,
        candidate: Option<Value>,
        context: Context,
    ) -> super::super::harness::types::ApiFuture<Option<Value>> {
        let runtime = Arc::clone(&self.memo_runtime);
        let name = name.to_owned();
        Box::pin(async move { runtime.memo(&name, candidate, context).await })
    }

    fn create_task(
        &self,
        task: super::super::tasks::TaskToken,
        input: Value,
        options: TaskOptions,
        context: Context,
    ) -> super::super::harness::types::ApiFuture<TaskId> {
        let runtime = Arc::clone(&self.memo_runtime);
        Box::pin(async move {
            let token = task;
            let slot: Arc<Mutex<Option<TaskId>>> = Arc::new(Mutex::new(None));
            let slot_for_task = Arc::clone(&slot);
            runtime
                .commit(
                    Box::new(move |tx, _current| {
                        *slot_for_task
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                            Some(tx.create_task(&token, input, options)?);
                        Ok(None)
                    }),
                    context,
                )
                .await?;
            let created = slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            created.ok_or_else(|| plain("The tool call has settled"))
        })
    }

    fn get_task(
        &self,
        id: TaskId,
        context: Context,
    ) -> super::super::harness::types::ApiFuture<Option<super::super::types::TaskRecord>> {
        let runtime = Arc::clone(&self.memo_runtime);
        Box::pin(async move { runtime.get_task(id, context).await })
    }

    fn wait_for_task(
        &self,
        id: TaskId,
        context: Context,
    ) -> super::super::harness::types::ApiFuture<super::super::types::TaskRecord> {
        let runtime = Arc::clone(&self.memo_runtime);
        Box::pin(async move { runtime.wait_for_task(id, context).await })
    }

    fn conversation(
        &self,
        id: ConversationId,
        context: Context,
    ) -> super::super::harness::types::ApiFuture<
        Option<super::super::harness::types::ConversationHandle>,
    > {
        let runtime = Arc::clone(&self.memo_runtime);
        Box::pin(async move { runtime.conversation(id, context).await })
    }
}

/// Throttled commits of what the tool reported into its `pi.live.tools` slot,
/// each writing only what changed since the last one (`publishProgress`).
fn publish_progress(
    runtime: &Arc<dyn TaskRuntimeLike>,
    reported: &Arc<Reported>,
    context: &Context,
) -> Arc<Progress> {
    let written: Arc<Mutex<ProgressWritten>> = Arc::new(Mutex::new(ProgressWritten::default()));
    let progress_runtime = Arc::clone(runtime);
    let write_runtime = Arc::clone(&progress_runtime);
    let progress_reported = Arc::clone(reported);
    let progress_context = context.clone();
    let conversation_id = runtime.conversation_id();
    let task_id = runtime.task_id();
    Progress::new(
        Box::new(move || {
            let runtime = Arc::clone(&write_runtime);
            let reported = Arc::clone(&progress_reported);
            let written = Arc::clone(&written);
            let context = progress_context.clone();
            Box::pin(async move {
                // Capture everything synchronously: the tool keeps reporting
                // while the commit is in flight.
                let snapshot = reported.output.snapshot();
                let current_details = reported
                    .details
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                let current_diagnostics = reported
                    .diagnostics
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .len();
                let (written_before, added): (ProgressWritten, Vec<Value>) = {
                    let written = written
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let diagnostics = reported
                        .diagnostics
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    (
                        written.clone(),
                        diagnostics
                            .iter()
                            .skip(written.diagnostics)
                            .take(current_diagnostics.saturating_sub(written.diagnostics))
                            .map(|diagnostic| {
                                serde_json::to_value(diagnostic).unwrap_or(Value::Null)
                            })
                            .collect(),
                    )
                };
                let details_changed = current_details != written_before.details;
                // What the commit writes, as Chord diffs the string: an
                // append, a trim plus an append of what follows the shared
                // part, or the whole window when its bounded overlap search
                // finds nothing.
                let mut bytes = 0;
                if snapshot.text != written_before.text {
                    let shared = if snapshot.text.starts_with(&written_before.text) {
                        written_before.text.len()
                    } else {
                        overlap(&written_before.text, &snapshot.text, 65_536)
                    };
                    bytes += utf8_byte_length(&snapshot.text[shared..]);
                }
                if details_changed {
                    bytes += utf8_byte_length(
                        &serde_json::to_string(&current_details.clone().unwrap_or(Value::Null))
                            .unwrap_or_default(),
                    );
                }
                if !added.is_empty() {
                    bytes += utf8_byte_length(&serde_json::to_string(&added).unwrap_or_default());
                }
                let commit_bytes = bytes;
                let commit_text = snapshot.text.clone();
                let commit_dropped_bytes = snapshot.dropped_bytes;
                let commit_dropped_lines = snapshot.dropped_lines;
                let commit_details = current_details.clone();
                let commit_added = added.clone();
                let details_changed_commit = details_changed;
                if runtime
                    .commit(
                        Box::new(move |tx, _current| {
                            let live = live_doc();
                            let draft =
                                tx.doc(&live.definition, Some(conversation_id), None, None)?;
                            let Some(index) = slot_index_of(&draft, task_id)? else {
                                return Ok(None);
                            };
                            // REMINDER: assign `output` as one string field.
                            // Chord then diffs it into an append, or a trim
                            // plus an append for a sliding tail; replacing the
                            // slot object would record the whole window on
                            // every commit.
                            let stored_output = tools_of_draft(&draft)?
                                .get(index)
                                .and_then(|slot| slot.output.clone())
                                .unwrap_or_default();
                            if stored_output != commit_text {
                                if commit_text.is_empty() {
                                    super::super::harness::live::delete_slot_field(
                                        &draft, index, "output",
                                    )?;
                                } else {
                                    set_slot_field(
                                        &draft,
                                        index,
                                        "output",
                                        Value::from(commit_text),
                                    )?;
                                }
                            }
                            if commit_dropped_bytes > 0 {
                                set_slot_field(
                                    &draft,
                                    index,
                                    "droppedBytes",
                                    js_number_value(commit_dropped_bytes as f64),
                                )?;
                            }
                            if commit_dropped_lines > 0 {
                                set_slot_field(
                                    &draft,
                                    index,
                                    "droppedLines",
                                    js_number_value(commit_dropped_lines as f64),
                                )?;
                            }
                            // Diff details leaf by leaf and append new
                            // diagnostics, so each commit writes only what
                            // changed.
                            if details_changed_commit {
                                if let Some(details) = &commit_details {
                                    assign_json(&draft, &slot_path_of(index, "details"), details)?;
                                } else {
                                    super::super::harness::live::delete_slot_field(
                                        &draft, index, "details",
                                    )?;
                                }
                            }
                            push_slot_diagnostics(&draft, index, commit_added)?;
                            Ok(None)
                        }),
                        context,
                    )
                    .await
                    .is_err()
                {
                    // A failed pass keeps the written state so the next one
                    // still writes the whole window; the port's Progress
                    // reports failures through the sentinel byte count.
                    return usize::MAX;
                }
                let next_written = ProgressWritten {
                    text: snapshot.text,
                    details: current_details,
                    diagnostics: current_diagnostics,
                };
                *written
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = next_written;
                commit_bytes
            })
        }),
        {
            let error_runtime = Arc::clone(&progress_runtime);
            Box::new(move |message| {
                // Rejections after an abort mark or close are expected; the
                // committed state stays consistent.
                if !error_runtime.aborted() {
                    error_runtime.report(&PlainError::new(message));
                }
            })
        },
    )
}

/// The last written slot state of one tool's progress publisher.
#[derive(Default, Clone)]
struct ProgressWritten {
    text: String,
    details: Option<Value>,
    diagnostics: usize,
}

fn slot_path_of(index: usize, field: &str) -> Vec<Seg> {
    vec![
        Seg::Key(String::from("tools")),
        Seg::Index(index),
        Seg::Key(field.to_owned()),
    ]
}

/// The settled result: the tool's result with the retained output and last
/// details as fallbacks, its diagnostics after those reported through the
/// api, `afterTool` applied, and explicit text bounded, with the Harness's
/// truncation diagnostic last (`finalResult`).
async fn final_result(
    runtime: &Arc<dyn TaskRuntimeLike>,
    call: &ToolCall,
    result: ToolExecutionResult,
    reported: &Reported,
    context: &Context,
) -> Result<ToolExecutionResult, PlainFailure> {
    let mut harness: Vec<ToolDiagnostic> = Vec::new();
    let snapshot = if result.content.is_none() {
        Some(reported.output.snapshot())
    } else {
        None
    };
    let content: Vec<TextOrImageBlock> = match &snapshot {
        None => result.content.clone().unwrap_or_default(),
        Some(snapshot) => {
            if snapshot.text.is_empty() {
                Vec::new()
            } else {
                vec![TextOrImageBlock::Text(TextContent {
                    text: snapshot.text.clone(),
                    text_signature: None,
                })]
            }
        }
    };
    let mut final_result = ToolExecutionResult {
        content: Some(content.clone()),
        is_error: result.is_error,
        details: match &result.details {
            Some(details) => Some(details.clone()),
            None => reported
                .details
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
        },
        diagnostics: {
            let reported_diagnostics = reported
                .diagnostics
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            Some(
                reported_diagnostics
                    .into_iter()
                    .chain(result.diagnostics.clone().unwrap_or_default())
                    .collect(),
            )
        },
        usage: result.usage,
        control: result.control.clone(),
    };
    {
        let runtime = Arc::clone(runtime);
        let context = context.clone();
        let call = call.clone();
        let current = Arc::new(Mutex::new(final_result.clone()));
        let closure_runtime = Arc::clone(&runtime);
        let closure_context = context.clone();
        let closure_call = call.clone();
        let closure_current = Arc::clone(&current);
        let retained_snapshot = snapshot.clone();
        let retain = reported.limits.retain;
        runtime
            .hooks(
                "afterTool",
                Arc::new(move |hook| {
                    let runtime = Arc::clone(&closure_runtime);
                    let context = closure_context.clone();
                    let _call = closure_call.clone();
                    let current = Arc::clone(&closure_current);
                    let hook = Arc::clone(hook);
                    Box::pin(async move {
                        let payload = serde_json::to_value(
                            &*current
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()),
                        )
                        .map_err(plain)?;
                        let replaced = hook(payload, hook_api(&runtime), context.clone()).await?;
                        if let Some(value) = replaced {
                            let parsed: ToolExecutionResult =
                                serde_json::from_value(value).map_err(plain)?;
                            *current
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner()) = parsed;
                        }
                        Ok(None)
                    })
                }),
            )
            .await
            .map_err(failure)?;
        final_result = current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let _ = retained_snapshot;
        let _ = retain;
    }
    // The retained output's truncation applies only while afterTool kept that
    // content.
    if let Some(snapshot) = &snapshot {
        if snapshot.dropped_bytes > 0 && final_result.content.as_ref() == Some(&content) {
            harness.push(truncated(
                snapshot.dropped_lines,
                snapshot.dropped_bytes,
                Some(reported.limits.retain),
            ));
        }
    }
    let (bounded_content, bounded_bytes, bounded_lines) = bound_content(
        final_result.content.clone().unwrap_or_default(),
        reported.limits,
    );
    if bounded_bytes > 0 {
        harness.push(truncated(
            bounded_lines,
            bounded_bytes,
            Some(reported.limits.retain),
        ));
    }
    final_result.content = Some(bounded_content);
    let mut diagnostics = final_result.diagnostics.take().unwrap_or_default();
    diagnostics.extend(harness);
    final_result.diagnostics = Some(diagnostics);
    Ok(final_result)
}

/// Commit the tool's terminal state: append its result entry, mark its slot
/// done, and complete or end aborted with the entry ID. `build` receives the
/// slot so interruption and abort can report the durable partial output
/// (`settle`).
async fn settle(
    runtime: &Arc<dyn TaskRuntimeLike>,
    call: &ToolCall,
    ending: Ending,
    build: impl FnOnce(Option<&ToolSlot>) -> ToolExecutionResult + Send + 'static,
    context: &Context,
) -> Result<(), PlainFailure> {
    let conversation_id = runtime.conversation_id();
    let task_id = runtime.task_id();
    let call = call.clone();
    let now = runtime.now();
    runtime
        .commit(
            Box::new(move |tx, _current| {
                let live = live_doc();
                let draft = tx.doc(&live.definition, Some(conversation_id), None, None)?;
                let slot: Option<ToolSlot> = slot_index_of(&draft, task_id)?
                    .and_then(|index| tools_of_draft(&draft).ok()?.get(index).cloned());
                // `build` receives the stored slot so interruption and abort
                // report the durable partial output.
                let result = build(slot.as_ref());
                let entry = append_tool_result(tx, conversation_id, &call, &result, now)?;
                if let Ok(Some(index)) = slot_index_of(&draft, task_id) {
                    finish_slot(&draft, index, Some(entry.id))?;
                }
                let entry_id = entry.id;
                match &ending {
                    Ending::Aborted => {
                        return Ok(Some(NextTaskState::Terminal {
                            outcome: TaskOutcome::Aborted {
                                reason: None,
                                result: Some({
                                    let mut map = Map::new();
                                    map.insert(String::from("entryId"), Value::from(entry_id));
                                    Value::Object(map)
                                }),
                            },
                        }));
                    }
                    Ending::Failed { message } => {
                        return Ok(Some(NextTaskState::Terminal {
                            outcome: TaskOutcome::Failed {
                                error: TaskOutcomeError {
                                    message: message.clone(),
                                    detail: None,
                                },
                                result: Some({
                                    let mut map = Map::new();
                                    map.insert(String::from("entryId"), Value::from(entry_id));
                                    Value::Object(map)
                                }),
                            },
                        }));
                    }
                    Ending::Completed => {}
                }
                // Tools build control objects freely; drop keys set to
                // undefined so the task result is strict JSON.
                let mut map = Map::new();
                map.insert(String::from("entryId"), Value::from(entry_id));
                if let Some(control) = &result.control {
                    map.insert(
                        String::from("control"),
                        serde_json::to_value(control).map_err(plain)?,
                    );
                }
                Ok(Some(NextTaskState::Terminal {
                    outcome: TaskOutcome::Completed {
                        result: Value::Object(map),
                    },
                }))
            }),
            context.clone(),
        )
        .await
        .map_err(failure)
}

/// An error result from the slot's durable partial output, details, and
/// diagnostics (`fromSlot`).
fn from_slot(slot: Option<&ToolSlot>, code: &str, message: &str) -> ToolExecutionResult {
    let mut diagnostics: Vec<ToolDiagnostic> = slot
        .map(|slot| slot.diagnostics.clone().unwrap_or_default())
        .unwrap_or_default()
        .iter()
        .filter_map(|diagnostic| serde_json::from_value(diagnostic.clone()).ok())
        .collect();
    let dropped_bytes = slot.and_then(|slot| slot.dropped_bytes).unwrap_or(0.0) as usize;
    if dropped_bytes > 0 {
        let dropped_lines = slot.and_then(|slot| slot.dropped_lines).unwrap_or(0.0) as usize;
        diagnostics.push(truncated(dropped_lines, dropped_bytes, None));
    }
    diagnostics.push(tool_diagnostic(code, message));
    let output = slot
        .and_then(|slot| slot.output.clone())
        .unwrap_or_default();
    ToolExecutionResult {
        content: Some(if output.is_empty() {
            Vec::new()
        } else {
            vec![TextOrImageBlock::Text(TextContent {
                text: output,
                text_signature: None,
            })]
        }),
        is_error: Some(true),
        details: slot.and_then(|slot| slot.details.clone()),
        diagnostics: Some(diagnostics),
        usage: None,
        control: None,
    }
}

/// An error result the Harness writes itself: no content and one `error`
/// diagnostic with `code` (`harnessError`).
pub fn harness_error(code: impl Into<String>, message: impl Into<String>) -> ToolExecutionResult {
    ToolExecutionResult {
        content: Some(Vec::new()),
        is_error: Some(true),
        details: None,
        diagnostics: Some(vec![tool_diagnostic(&code.into(), message.into())]),
        usage: None,
        control: None,
    }
}

fn tool_diagnostic(code: &str, message: impl Into<String>) -> ToolDiagnostic {
    ToolDiagnostic {
        severity: ToolDiagnosticSeverity::Error,
        code: Some(code.to_owned()),
        message: message.into(),
    }
}

/// The Harness's truncation diagnostic; `retain` is unknown when rebuilt from
/// a slot after recovery (`truncated`).
fn truncated(
    dropped_lines: usize,
    dropped_bytes: usize,
    retain: Option<OutputRetain>,
) -> ToolDiagnostic {
    let kept = match retain {
        None => String::new(),
        Some(OutputRetain::Head) => String::from(" to its beginning"),
        Some(OutputRetain::Tail) => String::from(" to its end"),
    };
    ToolDiagnostic {
        severity: ToolDiagnosticSeverity::Warn,
        code: Some(String::from("truncated")),
        message: format!(
            "Output truncated{kept}: {} lines, {} bytes dropped",
            js_number_value(dropped_lines as f64),
            js_number_value(dropped_bytes as f64)
        ),
    }
}

/// Append a `pi.tool-result` entry. The content ends with the rendered
/// diagnostics, so the stored message is exactly what the model sees; `data`
/// keeps the structured list. A result's usage is added to `pi.usage` in the
/// same commit (`appendToolResult`).
pub fn append_tool_result(
    tx: &Transaction,
    conversation_id: ConversationId,
    call: &ToolCall,
    result: &ToolExecutionResult,
    timestamp: f64,
) -> Result<super::super::types::EntryRecord, PlainError> {
    let diagnostics = result.diagnostics.clone().unwrap_or_default();
    let mut content: Vec<TextOrImageBlock> = result.content.clone().unwrap_or_default();
    if !diagnostics.is_empty() {
        content.push(TextOrImageBlock::Text(TextContent {
            text: render_diagnostics(&diagnostics),
            text_signature: None,
        }));
    }
    let message = ToolResultMessage {
        tool_call_id: call.id.clone(),
        tool_name: call.name.clone(),
        content,
        details: result.details.clone(),
        usage: result.usage,
        is_error: result.is_error.unwrap_or(false),
        timestamp: timestamp as i64,
    };
    if let Some(usage) = &result.usage {
        record_usage(tx, conversation_id, "tools", &call.name, usage)?;
    }
    let entry = tool_result_entry();
    let mut draft = EntryDraft::new(entry.kind.clone());
    draft.model = Some(vec![Message::ToolResult(message)]);
    draft.data = Some(serde_json::to_value(&diagnostics).map_err(plain)?);
    tx.append_entry(conversation_id, draft)
}

fn render_diagnostics(diagnostics: &[ToolDiagnostic]) -> String {
    let rendered: Vec<String> = diagnostics
        .iter()
        .map(|diagnostic| {
            format!(
                "[{}] {}",
                wire_severity(&diagnostic.severity),
                diagnostic.message
            )
        })
        .collect();
    format!("<harness>\n{}\n</harness>", rendered.join("\n"))
}

fn wire_severity(severity: &ToolDiagnosticSeverity) -> &'static str {
    match severity {
        ToolDiagnosticSeverity::Info => "info",
        ToolDiagnosticSeverity::Warn => "warn",
        ToolDiagnosticSeverity::Error => "error",
    }
}

/// Bound the text of result content. When the joined text exceeds the limits,
/// the text items are replaced by one bounded item at the position of the
/// first (head) or last (tail) text item; other content is kept
/// (`boundContent`).
fn bound_content(
    content: Vec<TextOrImageBlock>,
    limits: OutputLimits,
) -> (Vec<TextOrImageBlock>, usize, usize) {
    let texts: Vec<&TextContent> = content
        .iter()
        .filter_map(|item| match item {
            TextOrImageBlock::Text(text) => Some(text),
            _ => None,
        })
        .collect();
    let joined: String = texts.iter().map(|text| text.text.as_str()).collect();
    let bounded = bound_output(&joined, &limits);
    if bounded.dropped_bytes == 0 {
        return (content, 0, 0);
    }
    let keep = match limits.retain {
        OutputRetain::Head => texts.first().map(|text| text.text.clone()),
        OutputRetain::Tail => texts.last().map(|text| text.text.clone()),
    };
    let mut result: Vec<TextOrImageBlock> = Vec::new();
    for item in content {
        match &item {
            TextOrImageBlock::Text(text) => {
                if Some(text.text.clone()) == keep {
                    result.push(TextOrImageBlock::Text(TextContent {
                        text: bounded.text.clone(),
                        text_signature: text.text_signature.clone(),
                    }));
                }
            }
            other => result.push(other.clone()),
        }
    }
    (result, bounded.dropped_bytes, bounded.dropped_lines)
}

fn error_text_value(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}

/// The `HookApi` hooks receive, built from the invocation runtime.
fn hook_api(runtime: &Arc<dyn TaskRuntimeLike>) -> super::super::harness::types::HookApi {
    let memo_runtime = Arc::clone(runtime);
    super::super::harness::types::HookApi {
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

/// Wrap an async phase function as a [`PhaseFn`].
fn boxed<F, Fut>(phase: F) -> PhaseFn
where
    F: Fn(PhaseArgs) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<(), PlainFailure>> + Send + 'static,
{
    Arc::new(move |args: PhaseArgs| Box::pin(phase(args)))
}
