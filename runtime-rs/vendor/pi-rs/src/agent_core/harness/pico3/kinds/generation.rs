//! Port of `packages/agent/src/harness/pico3/kinds/generation.ts` (641
//! lines): the `pi.generation` turn kind — preparation (§12.5), request
//! derivation, streaming into the turn view, classification into tool
//! placement or turn settlement, durable retry, deferred polling, and
//! overflow/threshold collapse triggers.
//!
//! Disclosed substitutions:
//! - **Streaming.** Upstream coalesces `AssistantMessageFrameEncoder` frames
//!   into the turn view (`kinds/frames.ts`); the pi-ai port replaced the
//!   frame channel with the [`PartialAssistant`] accumulator
//!   (`src/ai/types/events.rs` module docs), so the port applies each event
//!   to the accumulator and flushes its `message()` snapshot — the same
//!   observable turn-view contents with the port's event protocol. Size
//!   (256 bytes) and time (100ms) flush thresholds and the first-content
//!   immediate flush are unchanged.
//! - The config's `model` key is routed-but-absent
//!   ([`KindConfig::declared_absent`]), matching upstream's
//!   `model: undefined` declaration.

use std::sync::Arc;

use futures::future::BoxFuture;
use futures::FutureExt;
use futures::StreamExt;
use serde_json::{json, Map, Value};

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::pico3::session::Tx;
use crate::agent_core::harness::pico3::types::{
    task_contract_fault, AnyKind, BasicKind, Completion, Id, KindConfig, ModelRef, NewEntry, Task,
    ViewEvent,
};
use crate::ai::retry::is_retryable_assistant_error;
use crate::ai::types::events::{AssistantMessageEvent, PartialAssistant, SuccessReason};
use crate::ai::types::message::AssistantMessage;
use crate::ai::types::primitives::StopReason;

use super::collapse::choose_through;
use super::estimate::estimate_context_tokens;
use crate::agent_core::harness::pico3::runtime::{
    AbortClosure, HookApi, HookHandlers, ModelInfo, Next, Runtime, Step,
};

// ---------------------------------------------------------------------------
// Hooks (`GenerationHooks`, generation.ts:64-75)
// ---------------------------------------------------------------------------

/// Upstream `GenerationHooks` (`generation.ts:64-75`): the per-kind handlers
/// struct downcast through the hook runner.
#[derive(Default)]
pub struct GenerationHandlers {
    /// §12.5: edit the draft; optionally override the tool loadout.
    pub system_instructions: Option<super::super::system::SystemInstructionsFn>,
    /// Before every request (also on retry and recovery). Chain.
    pub before_request: Option<BeforeRequestFn>,
    /// Every terminal provider message, before classification. Observer.
    pub after_response: Option<AfterResponseFn>,
    /// On a final answer with nothing queued. First `{ continue }` wins.
    pub on_yield: Option<OnYieldFn>,
}

impl HookHandlers for GenerationHandlers {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// `beforeRequest(request, info & { cutoff }, ctx)`: receives the request
/// built so far; a returned value replaces it (upstream mutates the request
/// object in place — the chain result is the same).
pub type BeforeRequestFn = Arc<
    dyn Fn(&Value, &HookApi, Id, Context) -> BoxFuture<'static, anyhow::Result<Option<Value>>>
        + Send
        + Sync,
>;

/// `afterResponse(message, info & { attempt }, ctx)`: observer.
pub type AfterResponseFn = Arc<
    dyn Fn(&Value, &HookApi, i64, Context) -> BoxFuture<'static, anyhow::Result<()>> + Send + Sync,
>;

/// `onYield(answer, info, ctx)`: first `{ continue }` wins.
pub type OnYieldFn = Arc<
    dyn Fn(&Value, &HookApi, Context) -> BoxFuture<'static, anyhow::Result<Option<String>>>
        + Send
        + Sync,
>;

// ---------------------------------------------------------------------------
// Config (generation.ts:78-86)
// ---------------------------------------------------------------------------

/// Upstream `generationConfig` (`generation.ts:78-86`).
pub fn generation_config() -> KindConfig {
    KindConfig {
        rewindable: json!({
            "thinkingLevel": "off",
            "selectedTools": [],
            "profile": "default",
        })
        .as_object()
        .cloned()
        .expect("object"),
        sticky: json!({
            "retry": {
                "enabled": true,
                "maxRetries": 3,
                "baseDelayMs": 2000,
                "maxAgentDelayMs": 60000,
            },
        })
        .as_object()
        .cloned()
        .expect("object"),
        declared_absent: super::super::types::DeclaredAbsent {
            rewindable: vec!["model".to_owned()],
            sticky: Vec::new(),
        },
    }
}

// ---------------------------------------------------------------------------
// The kind (generation.ts:132-310)
// ---------------------------------------------------------------------------

/// Upstream `generation` (`generation.ts:132`): the registered kind instance.
pub struct GenerationKind;

impl GenerationKind {
    /// The registered kind instance.
    pub fn new() -> Arc<Self> {
        Arc::new(GenerationKind)
    }
}

impl crate::agent_core::harness::pico3::runtime::Kind for GenerationKind {
    fn metadata(&self) -> Arc<dyn AnyKind> {
        Arc::new(
            BasicKind::new("pi.generation")
                .turn(true)
                .config(generation_config())
                .inflight(vec!["requesting".to_owned()]),
        )
    }

    fn initial(
        &self,
        task: Arc<Task>,
        rt: Arc<Runtime>,
        ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<Step>> {
        async move { initial_phase(task, rt, ctx).await }.boxed()
    }

    fn phases(&self) -> Vec<String> {
        vec![
            "prepared".to_owned(),
            "requesting".to_owned(),
            "retrying".to_owned(),
            "deferred".to_owned(),
        ]
    }

    fn phase(
        &self,
        phase: &str,
        task: Arc<Task>,
        rt: Arc<Runtime>,
        ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<Step>> {
        let phase = phase.to_owned();
        async move {
            match phase.as_str() {
                "prepared" => prepared_phase(task, rt, ctx).await,
                "requesting" => requesting_phase(task, rt, ctx).await,
                "retrying" => retrying_phase(task, rt, ctx).await,
                "deferred" => deferred_phase(task, rt, ctx).await,
                other => Err(task_contract_fault(
                    "pi.generation",
                    &format!("no handler for phase {other}"),
                )),
            }
        }
        .boxed()
    }

    fn abort(
        &self,
        task: Arc<Task>,
        rt: Arc<Runtime>,
        ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<AbortClosure>> {
        async move { abort_handler(task, rt, ctx).await }.boxed()
    }
}

/// Step 1 (§12.5): snapshot S on the line; systemInstructions off the line;
/// then in one commit re-take S′, retry if it moved, else append the managed
/// entry and checkpoint `prepared` with the cutoff (`generation.ts:149-192`).
async fn initial_phase(task: Arc<Task>, rt: Arc<Runtime>, ctx: Context) -> anyhow::Result<Step> {
    let sections = rt.sections.clone();
    let tools = rt.tools_registry.clone();
    let (snapshot, canonical, seed) = rt
        .commit(
            move |tx: &mut Tx, current: Task, _ctx: Context| {
                let sections = sections.clone();
                let tools = tools.clone();
                async move {
                    super::super::system::take_snapshot(
                        tx,
                        current.conversation_id,
                        &sections,
                        &tools,
                    )
                    .await
                }
                .boxed()
            },
            ctx.clone(),
        )
        .await?;
    if snapshot.settings.model.is_none() {
        return Ok(fail_step("no_model", "no model configured"));
    }
    let sticky = rt.sticky(task.conversation_id, ctx.clone()).await?;
    let retry = sticky.get("retry").cloned().unwrap_or(Value::Null);

    let warnings: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    // `prepareDraft(rt, …)` (`generation.ts:158-166`).
    let bindings: Vec<(super::super::system::SystemInstructionsFn, HookApi)> = rt
        .hooks
        .bindings()
        .iter()
        .filter_map(|binding| {
            binding
                .handlers
                .downcast_ref::<GenerationHandlers>()
                .and_then(|handlers| handlers.system_instructions.clone())
                .map(|handler| (handler, binding.api.clone()))
        })
        .collect();
    let sections_for_draft = rt.sections.clone();
    let (desired, tools) = super::super::system::prepare_draft(
        bindings,
        &sections_for_draft,
        &|name| rt.tool(name),
        &canonical,
        seed.as_deref(),
        &snapshot.settings,
        &|message: String| warnings.lock().expect("warnings").push(message),
    )
    .await?;
    let model = snapshot.settings.model.clone().expect("checked above");
    let thinking_level = snapshot.settings.thinking_level.clone();

    let snapshot = Arc::new(snapshot);
    let canonical = Arc::new(canonical);
    let desired = Arc::new(desired);
    let tools = Arc::new(tools);
    let warnings = warnings.clone();
    Ok(Step::Next(Next::Defer(Box::new(
        move |tx: &mut Tx, current: Task, _ctx: Context| {
            let snapshot = snapshot.clone();
            let canonical = canonical.clone();
            let desired = desired.clone();
            let tools = tools.clone();
            let retry = retry.clone();
            let model = model.clone();
            let thinking_level = thinking_level.clone();
            let warnings = warnings.clone();
            let rt = rt.clone();
            async move {
                let c = current.conversation_id;
                let sections = rt.sections.clone();
                let registry_tools = rt.tools_registry.clone();
                let (again, _, _) =
                    super::super::system::take_snapshot(tx, c, &sections, &registry_tools).await?;
                if !super::super::system::same_snapshot(&again, &snapshot) {
                    return Ok(Next::Retry);
                }
                let plan = super::super::system::plan_managed_entry(
                    tx,
                    c,
                    &snapshot,
                    &canonical,
                    &desired,
                    &tools,
                    rt.now(),
                )
                .await?;
                for message in warnings.lock().expect("warnings").iter() {
                    tx.emit_event(ViewEvent::Warning {
                        source: "generation".to_owned(),
                        message: message.clone(),
                    })?;
                }
                let system: Option<Id> = match plan {
                    Some(plan) => {
                        let entry = NewEntry {
                            kind: "pi.system".to_owned(),
                            model: if plan.model.is_empty() {
                                None
                            } else {
                                Some(plan.model.clone())
                            },
                            data: Some(object_of(plan.data.to_value())),
                            edits: if plan.edits.is_empty() {
                                None
                            } else {
                                Some(plan.edits)
                            },
                            ..Default::default()
                        };
                        Some(tx.append_entry(c, entry)?)
                    }
                    None => None,
                };
                let cutoff = match system {
                    Some(id) => id,
                    None => tx
                        .newest_entry(c, None, false)
                        .await?
                        .map(|entry| entry.id)
                        .ok_or_else(|| anyhow::anyhow!("conversation has no entries"))?,
                };
                // A new generation starts a fresh turn view
                // (`generation.ts:179`).
                tx.sticky_set(c, "turn", json!({ "tools": [] }))?;
                Ok(Next::Checkpoint(object_of(json!({
                    "phase": "prepared",
                    "cutoff": cutoff,
                    "system": system,
                    "model": model,
                    "thinkingLevel": thinking_level,
                    "tools": tools.iter().map(|t| Value::String(t.name.clone())).collect::<Vec<_>>(),
                    "retry": retry,
                    "attempt": 0,
                }))))
            }.boxed()
        },
    ))))
}

/// Step 2: derive the request from the cutoff (hooks rerun), overflow check,
/// write the in-flight checkpoint, call the provider (`generation.ts:198-221`).
async fn prepared_phase(task: Arc<Task>, rt: Arc<Runtime>, ctx: Context) -> anyhow::Result<Step> {
    let cp = checkpoint_of(&task);
    let model_ref: ModelRef = serde_json::from_value(prep(&cp, "model"))?;
    let Some(model) = rt.models.resolve(&model_ref) else {
        return Ok(fail_step(
            "no_model",
            &format!(
                "model {}/{} unavailable",
                model_ref.provider, model_ref.model_id
            ),
        ));
    };
    let derived = derive(&task, &cp, &rt, ctx.clone()).await?;
    if estimate_request(&derived) > model.context_window - model.max_tokens {
        return Ok(overflow());
    }
    let attempt = cp.get("attempt").and_then(Value::as_i64).unwrap_or(0) + 1;
    // Before the effect (`generation.ts:206-209`).
    {
        let cp = cp.clone();
        rt.commit_erased_with(
            ctx.clone(),
            Box::new(move |tx: &mut Tx, current: Task, _ctx: Context| {
                let cp = cp.clone();
                async move {
                    let mut next = cp.as_object().cloned().unwrap_or_default();
                    next.insert("phase".to_owned(), Value::String("requesting".to_owned()));
                    next.insert("attempt".to_owned(), json!(attempt));
                    tx.checkpoint(Value::Object(next))?;
                    tx.emit_event(ViewEvent::GenerationStarted {
                        task_id: current.id,
                        attempt,
                    })?;
                    Ok(Value::Null)
                }
                .boxed()
            }),
        )
        .await?;
    }
    // Upstream passes `{ ...cp, attempt }` into classify
    // (`generation.ts:220`).
    let mut cp_with_attempt = cp.as_object().cloned().unwrap_or_default();
    cp_with_attempt.insert("attempt".to_owned(), json!(attempt));
    let streamed = stream(&cp, &model, derived, &rt, ctx.clone()).await?;
    if let Some(handle) = streamed.deferred {
        let poll_at = rt.now() + 5000;
        return Ok(Step::Next(Next::Defer(Box::new(
            move |tx: &mut Tx, current: Task, _ctx: Context| {
                let cp = cp.clone();
                let handle = handle.clone();
                async move {
                    tx.emit_event(ViewEvent::GenerationDeferred {
                        task_id: current.id,
                        poll_at,
                    })?;
                    let mut next = cp.as_object().cloned().unwrap_or_default();
                    next.insert("phase".to_owned(), Value::String("deferred".to_owned()));
                    next.insert("attempt".to_owned(), json!(attempt));
                    next.insert("handle".to_owned(), handle);
                    next.insert("pollAt".to_owned(), json!(poll_at));
                    Ok(Next::Checkpoint(next))
                }
                .boxed()
            },
        ))));
    }
    let terminal = streamed
        .terminal
        .expect("stream returned a terminal message");
    classify(
        &task,
        &Value::Object(cp_with_attempt),
        attempt,
        terminal,
        &rt,
        ctx,
    )
    .await
}

/// Only reached by the scheduler after a crash: the call may have happened
/// and there is no result lookup (`generation.ts:225-245`).
async fn requesting_phase(
    task: Arc<Task>,
    rt: Arc<Runtime>,
    _ctx: Context,
) -> anyhow::Result<Step> {
    let cp = checkpoint_of(&task);
    match retry_decision(&cp, None, rt.now()) {
        RetryDecision::Fail(reason) => Ok(fail_step(&reason, "interrupted")),
        RetryDecision::Retry { until_ms } => Ok(Step::Next(Next::Defer(Box::new(
            move |tx: &mut Tx, current: Task, _ctx: Context| {
                let cp = cp.clone();
                async move {
                    let attempt = cp.get("attempt").and_then(Value::as_i64).unwrap_or(0);
                    tx.append_entry(
                        current.conversation_id,
                        NewEntry {
                            kind: "pi.usage".to_owned(),
                            data: Some(object_of(json!({
                                "attempt": attempt,
                                "error": "interrupted",
                            }))),
                            ..Default::default()
                        },
                    )?;
                    tx.emit_event(ViewEvent::GenerationRetrying {
                        task_id: current.id,
                        attempt,
                        retry_at: until_ms,
                        error: "interrupted".to_owned(),
                    })?;
                    let mut next = cp.as_object().cloned().unwrap_or_default();
                    next.insert("phase".to_owned(), Value::String("retrying".to_owned()));
                    next.insert("untilMs".to_owned(), json!(until_ms));
                    next.insert(
                        "lastError".to_owned(),
                        Value::String("interrupted".to_owned()),
                    );
                    Ok(Next::Checkpoint(next))
                }
                .boxed()
            },
        )))),
    }
}

/// Durable backoff, then back to step 2 (`generation.ts:248-258`).
async fn retrying_phase(task: Arc<Task>, rt: Arc<Runtime>, ctx: Context) -> anyhow::Result<Step> {
    let cp = checkpoint_of(&task);
    let until_ms = cp.get("untilMs").and_then(Value::as_i64).unwrap_or(0);
    rt.sleep(until_ms, ctx).await?;
    Ok(Step::Next(Next::Defer(Box::new(
        move |tx: &mut Tx, current: Task, _ctx: Context| {
            let cp = cp.clone();
            async move {
                // `tx.sticky(current.conversationId).turn.message = undefined`
                // (`generation.ts:254`).
                clear_turn_message(tx, current.conversation_id)?;
                let mut next = cp.as_object().cloned().unwrap_or_default();
                next.shift_remove("untilMs");
                next.shift_remove("lastError");
                next.insert("phase".to_owned(), Value::String("prepared".to_owned()));
                Ok(Next::Checkpoint(next))
            }
            .boxed()
        },
    ))))
}

/// Provider-side async. Poll; not the retry backoff (`generation.ts:261-282`).
async fn deferred_phase(task: Arc<Task>, rt: Arc<Runtime>, ctx: Context) -> anyhow::Result<Step> {
    let cp = checkpoint_of(&task);
    let model_ref: ModelRef = serde_json::from_value(prep(&cp, "model"))?;
    let Some(model) = rt.models.resolve(&model_ref) else {
        return Ok(fail_step("no_model", "model disappeared"));
    };
    let poll_at = cp.get("pollAt").and_then(Value::as_i64).unwrap_or(0);
    rt.sleep(poll_at, ctx.clone()).await?;
    let handle = cp.get("handle").cloned().unwrap_or(Value::Null);
    let result = rt.models.fetch_deferred(model, handle, ctx.clone()).await?;
    if result
        .get("deferred")
        .map(|d| !d.is_null())
        .unwrap_or(false)
    {
        let next_handle = result.get("deferred").cloned().unwrap_or(Value::Null);
        let poll_at = rt.now() + 5000;
        return Ok(Step::Next(Next::Defer(Box::new(
            move |tx: &mut Tx, current: Task, _ctx: Context| {
                let cp = cp.clone();
                let next_handle = next_handle.clone();
                async move {
                    tx.emit_event(ViewEvent::GenerationDeferred {
                        task_id: current.id,
                        poll_at,
                    })?;
                    let mut next = cp.as_object().cloned().unwrap_or_default();
                    next.insert("handle".to_owned(), next_handle);
                    next.insert("pollAt".to_owned(), json!(poll_at));
                    Ok(Next::Checkpoint(next))
                }
                .boxed()
            },
        ))));
    }
    let message: AssistantMessage = serde_json::from_value(result)?;
    // `tx.sticky(c).turn.message = toStored(message)` (`generation.ts:277-279`).
    let stored = serde_json::to_value(&message)?;
    rt.commit_erased_with(
        ctx.clone(),
        Box::new(move |tx: &mut Tx, current: Task, _ctx: Context| {
            let stored = stored.clone();
            async move {
                set_turn_message(tx, current.conversation_id, stored)?;
                Ok(Value::Null)
            }
            .boxed()
        }),
    )
    .await?;
    let mut prep = cp.as_object().cloned().unwrap_or_default();
    prep.shift_remove("handle");
    prep.shift_remove("pollAt");
    let attempt = prep.get("attempt").and_then(Value::as_i64).unwrap_or(0);
    classify(&task, &Value::Object(prep), attempt, message, &rt, ctx).await
}

/// Abort (`generation.ts:285-309`): display-only partial, inputs
/// unanswered/aborted.
async fn abort_handler(
    task: Arc<Task>,
    rt: Arc<Runtime>,
    ctx: Context,
) -> anyhow::Result<AbortClosure> {
    let cp = checkpoint_of(&task);
    if cp.get("phase").and_then(Value::as_str) == Some("deferred") {
        if let Ok(model_ref) = serde_json::from_value::<ModelRef>(prep(&cp, "model")) {
            if let Some(model) = rt.models.resolve(&model_ref) {
                let handle = cp.get("handle").cloned().unwrap_or(Value::Null);
                // `.catch(() => {})` (`generation.ts:289`).
                let _ = rt.models.cancel_deferred(model, handle, ctx.clone()).await;
            }
        }
    }
    let input_inputs = task_input_inputs(&task);
    Ok(Box::new(
        move |tx: &mut Tx, current: Task, _ctx: Context| {
            let input_inputs = input_inputs.clone();
            let cp = cp.clone();
            async move {
                let partial = tx
                    .snapshot(crate::agent_core::harness::pico3::types::DocRef::Sticky {
                        conversation_id: current.conversation_id,
                    })?
                    .get("turn")
                    .and_then(|turn| turn.get("message"))
                    .cloned()
                    .filter(|message| !message.is_null());
                let mut assistant: Option<Id> = None;
                // Display-only: the partial goes in `data`, never in `model`
                // (`generation.ts:294-303`).
                let content_len = partial
                    .as_ref()
                    .and_then(|message| message.get("content"))
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0);
                if let Some(mut display) = partial {
                    if content_len > 0 {
                        display["stopReason"] = Value::String("aborted".to_owned());
                        assistant = Some(tx.append_entry(
                            current.conversation_id,
                            NewEntry {
                                kind: "pi.assistant".to_owned(),
                                data: Some(object_of(json!({
                                    "attempt": cp.get("attempt").and_then(Value::as_i64).unwrap_or(0),
                                    "display": display,
                                    "reason": "aborted",
                                }))),
                                ..Default::default()
                            },
                        )?);
                    }
                }
                tx.sticky_set(current.conversation_id, "turn", json!({ "tools": [] }))?;
                tx.resolve_inputs(
                    &input_inputs,
                    &crate::agent_core::harness::pico3::session::Resolution::Unanswered {
                        reason: "aborted".to_owned(),
                        detail: None,
                    },
                )
                .await?;
                tx.emit_event(ViewEvent::TurnEndedUnanswered {
                    inputs: input_inputs,
                    reason: "aborted".to_owned(),
                    detail: None,
                })?;
                Ok(match assistant {
                    Some(id) => json!({ "assistant": id }),
                    None => json!({}),
                })
            }.boxed()
        },
    ))
}

// ---------------------------------------------------------------------------
// Request derivation and streaming (generation.ts:312-410)
// ---------------------------------------------------------------------------

/// Upstream `derive` (`generation.ts:316-327`).
async fn derive(task: &Task, cp: &Value, rt: &Arc<Runtime>, ctx: Context) -> anyhow::Result<Value> {
    let cutoff = cp.get("cutoff").and_then(Value::as_i64);
    let context = rt
        .context(task.conversation_id, cutoff, ctx.clone())
        .await?;
    let mut request = json!({ "messages": context.messages });
    let before_request: Vec<(BeforeRequestFn, HookApi)> = rt
        .hooks
        .bindings()
        .iter()
        .filter_map(|binding| {
            binding
                .handlers
                .downcast_ref::<GenerationHandlers>()
                .and_then(|handlers| handlers.before_request.clone())
                .map(|handler| (handler, binding.api.clone()))
        })
        .collect();
    let cutoff = cp.get("cutoff").and_then(Value::as_i64).unwrap_or(0);
    for (handler, api) in &before_request {
        match handler(&request, api, cutoff, ctx.clone()).await {
            Ok(Some(replacement)) => request = replacement,
            Ok(None) => {}
            Err(error) => {
                let aborted = ctx
                    .abort_signal()
                    .is_some_and(|signal| signal.is_cancelled());
                if aborted {
                    return Err(error);
                }
            }
        }
    }
    Ok(request)
}

/// Upstream `estimate` (`generation.ts:110-118`): pi-ai drops system
/// messages and aborted/error assistant messages before sending; estimate
/// what is sent.
fn estimate_request(request: &Value) -> i64 {
    let messages = request
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let filtered: Vec<Value> = messages
        .into_iter()
        .filter(|message| {
            let role = message.get("role").and_then(Value::as_str);
            !(role == Some("system")
                || (role == Some("assistant")
                    && matches!(
                        message.get("stopReason").and_then(Value::as_str),
                        Some("aborted") | Some("error")
                    )))
        })
        .collect();
    estimate_context_tokens(&filtered).tokens
}

/// The stream outcome (`generation.ts:330-336`).
struct StreamOutcome {
    terminal: Option<AssistantMessage>,
    deferred: Option<Value>,
}

/// Upstream `stream` (`generation.ts:329-410`): stream, coalescing into the
/// turn view. Flush on size or time.
async fn stream(
    cp: &Value,
    model: &ModelInfo,
    request: Value,
    rt: &Arc<Runtime>,
    ctx: Context,
) -> anyhow::Result<StreamOutcome> {
    let messages = request
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let thinking_level = cp
        .get("thinkingLevel")
        .and_then(Value::as_str)
        .unwrap_or("off")
        .to_owned();
    let mut event_stream = rt.models.stream(
        model.clone(),
        crate::agent_core::harness::pico3::runtime::GenerationRequest {
            messages,
            thinking_level,
        },
        ctx.clone(),
    );
    let mut partial = PartialAssistant::new();
    let mut pending_bytes: i64 = 0;
    let mut pending = false;
    let mut flushed_content = false;
    let mut terminal: Option<AssistantMessage> = None;
    let mut deferred: Option<Value> = None;

    // The flush (`generation.ts:344-355`): write the current partial into
    // the turn view.
    async fn flush(
        partial: &PartialAssistant,
        rt: &Arc<Runtime>,
        ctx: Context,
    ) -> anyhow::Result<()> {
        let message = partial.message().cloned();
        rt.commit_erased_with(
            ctx,
            Box::new(move |tx: &mut Tx, current: Task, _ctx: Context| {
                let message = message.clone();
                async move {
                    if let Some(message) = message {
                        let stored = serde_json::to_value(&message)?;
                        set_turn_message(tx, current.conversation_id, stored)?;
                    }
                    Ok(Value::Null)
                }
                .boxed()
            }),
        )
        .await
        .map(|_| ())
    }

    loop {
        #[allow(clippy::large_enum_variant)]
        enum Pick {
            Event(Option<anyhow::Result<AssistantMessageEvent>>),
            Flush,
        }
        let pick = if !pending {
            Pick::Event(event_stream.next().await)
        } else {
            let deadline = tokio::time::Instant::from_std(
                std::time::Instant::now() + std::time::Duration::from_millis(100),
            );
            tokio::select! {
                result = event_stream.next() => Pick::Event(result),
                _ = tokio::time::sleep_until(deadline) => Pick::Flush,
                _ = async {
                    if let Some(signal) = ctx.abort_signal() {
                        signal.cancelled().await;
                    }
                } => {
                    // `rt.sleep(…, ctx)` rejects on abort
                    // (`generation.ts:367`); the catch rethrows when aborted.
                    anyhow::bail!("aborted");
                }
            }
        };
        match pick {
            Pick::Flush => {
                flush(&partial, rt, ctx.clone()).await?;
                pending_bytes = 0;
                pending = false;
            }
            Pick::Event(None) => break,
            Pick::Event(Some(Err(error))) => {
                // `catch (error) { if (aborted) throw; terminal = error
                // assistant }` (`generation.ts:393-406`).
                let aborted = ctx
                    .abort_signal()
                    .is_some_and(|signal| signal.is_cancelled());
                if aborted {
                    return Err(error);
                }
                terminal = Some(error_assistant(model, &format!("{error}"), rt.now()));
                break;
            }
            Pick::Event(Some(Ok(event))) => match event {
                AssistantMessageEvent::Done { reason, message } => {
                    if reason == SuccessReason::Deferred && message.deferred.is_some() {
                        deferred = Some(serde_json::to_value(message.deferred)?);
                    } else {
                        terminal = Some(message);
                    }
                    break;
                }
                AssistantMessageEvent::Error { error, .. } => {
                    terminal = Some(error);
                    break;
                }
                event => {
                    let is_delta = matches!(
                        event,
                        AssistantMessageEvent::TextDelta { .. }
                            | AssistantMessageEvent::ThinkingDelta { .. }
                            | AssistantMessageEvent::ToolcallDelta { .. }
                    );
                    let delta_len = match &event {
                        AssistantMessageEvent::TextDelta { delta, .. } => delta.len() as i64,
                        AssistantMessageEvent::ThinkingDelta { delta, .. } => delta.len() as i64,
                        AssistantMessageEvent::ToolcallDelta { delta, .. } => delta.len() as i64,
                        _ => 64,
                    };
                    if let Err(error) = partial.apply(&event) {
                        anyhow::bail!("{error}");
                    }
                    pending_bytes += delta_len;
                    pending = true;
                    if pending_bytes >= 256 || (!flushed_content && is_delta) {
                        flush(&partial, rt, ctx.clone()).await?;
                        if is_delta {
                            flushed_content = true;
                        }
                        pending_bytes = 0;
                        pending = false;
                    }
                }
            },
        }
    }
    if pending {
        // Final flush (`generation.ts:407`).
        flush(&partial, rt, ctx.clone()).await?;
    }
    if terminal.is_none() && deferred.is_none() {
        anyhow::bail!("stream ended without a terminal message");
    }
    Ok(StreamOutcome { terminal, deferred })
}

/// Upstream error assistant construction (`generation.ts:395-405`).
fn error_assistant(model: &ModelInfo, message: &str, timestamp: i64) -> AssistantMessage {
    AssistantMessage {
        content: Vec::new(),
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        usage: empty_usage(),
        stop_reason: StopReason::Error,
        deferred: None,
        error_message: Some(message.to_owned()),
        raw_stop_reason: None,
        end_turn: None,
        timestamp,
    }
}

/// Upstream `emptyUsage` (`generation.ts:119-126`).
fn empty_usage() -> crate::ai::types::Usage {
    serde_json::from_value(json!({
        "input": 0,
        "output": 0,
        "cacheRead": 0,
        "cacheWrite": 0,
        "totalTokens": 0,
        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0 },
    }))
    .expect("usage shape")
}

// ---------------------------------------------------------------------------
// Steps 3–5: classify the terminal message (generation.ts:416-552)
// ---------------------------------------------------------------------------

async fn classify(
    task: &Task,
    cp: &Value,
    attempt: i64,
    message: AssistantMessage,
    rt: &Arc<Runtime>,
    ctx: Context,
) -> anyhow::Result<Step> {
    // afterResponse hooks (`generation.ts:417`).
    let after_response: Vec<(AfterResponseFn, HookApi)> = rt
        .hooks
        .bindings()
        .iter()
        .filter_map(|binding| {
            binding
                .handlers
                .downcast_ref::<GenerationHandlers>()
                .and_then(|handlers| handlers.after_response.clone())
                .map(|handler| (handler, binding.api.clone()))
        })
        .collect();
    let message_value = serde_json::to_value(&message)?;
    for (handler, api) in &after_response {
        handler(&message_value, api, attempt, ctx.clone()).await?;
    }

    if message.stop_reason == StopReason::Error {
        match retry_decision(cp, Some(&message), rt.now()) {
            RetryDecision::Retry { until_ms } => {
                let error_message = message
                    .error_message
                    .clone()
                    .unwrap_or_else(|| "provider error".to_owned());
                let usage = serde_json::to_value(message.usage)?;
                let cp = cp.clone();
                return Ok(Step::Next(Next::Defer(Box::new(
                    move |tx: &mut Tx, current: Task, _ctx: Context| {
                        let cp = cp.clone();
                        let usage = usage.clone();
                        let error_message = error_message.clone();
                        async move {
                            let attempt = cp.get("attempt").and_then(Value::as_i64).unwrap_or(0);
                            let mut data = object_of(json!({
                                "attempt": attempt,
                                "error": error_message,
                            }));
                            data.insert("usage".to_owned(), usage);
                            tx.append_entry(
                                current.conversation_id,
                                NewEntry {
                                    kind: "pi.usage".to_owned(),
                                    data: Some(data),
                                    ..Default::default()
                                },
                            )?;
                            tx.emit_event(ViewEvent::GenerationRetrying {
                                task_id: current.id,
                                attempt,
                                retry_at: until_ms,
                                error: error_message.clone(),
                            })?;
                            let mut next = cp.as_object().cloned().unwrap_or_default();
                            next.insert("phase".to_owned(), Value::String("retrying".to_owned()));
                            next.insert("untilMs".to_owned(), json!(until_ms));
                            next.insert("lastError".to_owned(), Value::String(error_message));
                            Ok(Next::Checkpoint(next))
                        }
                        .boxed()
                    },
                ))));
            }
            RetryDecision::Fail(reason) => {
                return Ok(terminal_error(task, cp, &message, &reason));
            }
        }
    }
    if message.stop_reason == StopReason::Aborted {
        return Ok(terminal_error(task, cp, &message, "provider"));
    }

    // Upstream stores the typed assistant message (with its `role`);
    // the pi-ai struct does not carry `role`, so it is injected here to
    // keep the stored message a valid stored message (`session.ts:1186`).
    let mut stored = serde_json::to_value(&message)?;
    if let Some(object) = stored.as_object_mut() {
        object.insert("role".to_owned(), Value::String("assistant".to_owned()));
    }
    let calls: Vec<Value> = message
        .content
        .iter()
        .filter_map(|block| match block {
            crate::ai::types::AssistantBlock::ToolCall(call) => serde_json::to_value(call).ok(),
            _ => None,
        })
        .collect();
    if !calls.is_empty() {
        let offered: Vec<String> = cp
            .get("tools")
            .and_then(Value::as_array)
            .map(|array| {
                array
                    .iter()
                    .filter_map(|name| name.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let input_inputs = task_input_inputs(task);
        return Ok(Step::Done(Box::new(
            move |tx: &mut Tx, current: Task, _ctx: Context| {
                let calls = calls.clone();
                let stored = stored.clone();
                let offered = offered.clone();
                let input_inputs = input_inputs.clone();
                async move {
                    let c = current.conversation_id;
                    // Read BEFORE the append (`generation.ts:458`).
                    let collapse_through = threshold_collapse_through(tx, c, &stored).await?;
                    let assistant = tx.append_entry(
                        c,
                        NewEntry {
                            kind: "pi.assistant".to_owned(),
                            model: Some(vec![stored]),
                            data: Some(object_of(json!({ "attempt": attempt }))),
                            ..Default::default()
                        },
                    )?;
                    // `turn.message = undefined; turn.tools = calls.map(...)`
                    // (`generation.ts:464-471`).
                    let tools_slots: Vec<Value> = calls
                        .iter()
                        .map(|call| {
                            json!({
                                "callId": call.get("id"),
                                "name": call.get("name"),
                                "args": call.get("arguments"),
                                "status": "pending",
                            })
                        })
                        .collect();
                    tx.sticky_set(c, "turn", json!({ "tools": tools_slots }))?;
                    let mut tool_ids: Vec<Id> = Vec::new();
                    for (index, call) in calls.iter().enumerate() {
                        let id =
                            tx.create_task(crate::agent_core::harness::pico3::types::TaskSpec {
                                kind: "pi.tool".to_owned(),
                                conversation_id: Some(c),
                                input: json!({
                                    "assistant": assistant,
                                    "call": call,
                                    "offered": offered,
                                    "index": index,
                                }),
                                after: Vec::new(),
                                background: false,
                            })?;
                        tool_ids.push(id);
                    }
                    let post_tools =
                        tx.create_task(crate::agent_core::harness::pico3::types::TaskSpec {
                            kind: "pi.post_tools".to_owned(),
                            conversation_id: Some(c),
                            input: json!({
                                "inputs": input_inputs,
                                "assistant": assistant,
                                "tools": tool_ids,
                            }),
                            after: tool_ids.clone(),
                            background: false,
                        })?;
                    if let Some(through) = collapse_through {
                        tx.create_task(crate::agent_core::harness::pico3::types::TaskSpec {
                            kind: "pi.collapse".to_owned(),
                            conversation_id: Some(c),
                            input: json!({ "reason": "threshold", "through": through }),
                            after: Vec::new(),
                            background: false,
                        })?;
                    }
                    tx.emit_event(ViewEvent::GenerationCompleted {
                        task_id: current.id,
                        entry: assistant,
                        tool_calls: calls.len() as i64,
                    })?;
                    Ok(Completion::completed(json!({
                        "assistant": assistant,
                        "tools": tool_ids,
                        "postTools": post_tools,
                    })))
                }
                .boxed()
            },
        )));
    }

    // onYield hooks (`generation.ts:488-497`).
    let on_yield: Vec<(OnYieldFn, HookApi)> = rt
        .hooks
        .bindings()
        .iter()
        .filter_map(|binding| {
            binding
                .handlers
                .downcast_ref::<GenerationHandlers>()
                .and_then(|handlers| handlers.on_yield.clone())
                .map(|handler| (handler, binding.api.clone()))
        })
        .collect();
    let mut continue_text: Option<String> = None;
    for (handler, api) in &on_yield {
        match handler(&stored, api, ctx.clone()).await {
            Ok(Some(text)) => {
                continue_text = Some(text);
                break;
            }
            Ok(None) => {}
            Err(error) => {
                let aborted = ctx
                    .abort_signal()
                    .is_some_and(|signal| signal.is_cancelled());
                if aborted {
                    return Err(error);
                }
            }
        }
    }

    let input_inputs = task_input_inputs(task);
    Ok(Step::Done(Box::new(
        move |tx: &mut Tx, current: Task, _ctx: Context| {
            let stored = stored.clone();
            let input_inputs = input_inputs.clone();
            let continue_text = continue_text.clone();
            async move {
                let c = current.conversation_id;
                let collapse_through = threshold_collapse_through(tx, c, &stored).await?;
                let head = tx.newest_entry(c, None, true).await?;
                let assistant = tx.append_entry(
                    c,
                    NewEntry {
                        kind: "pi.assistant".to_owned(),
                        model: Some(vec![stored]),
                        data: Some(object_of(json!({ "attempt": attempt }))),
                        ..Default::default()
                    },
                )?;
                tx.emit_event(ViewEvent::GenerationCompleted {
                    task_id: current.id,
                    entry: assistant,
                    tool_calls: 0,
                })?;
                tx.sticky_set(c, "turn", json!({ "tools": [] }))?;
                if let Some(through) = collapse_through {
                    tx.create_task(crate::agent_core::harness::pico3::types::TaskSpec {
                        kind: "pi.collapse".to_owned(),
                        conversation_id: Some(c),
                        input: json!({ "reason": "threshold", "through": through }),
                        after: Vec::new(),
                        background: false,
                    })?;
                }
                let head_boundary = head.as_ref().map(|entry| entry.id);
                let boundary = tx.boundary(c, "final", head_boundary).await?;
                if let Some(text) = &continue_text {
                    if boundary.triggers.is_empty() && !boundary.terminated {
                        // Continuation (`generation.ts:509-517`).
                        tx.append_entry(
                            c,
                            NewEntry {
                                kind: "pi.user".to_owned(),
                                model: Some(vec![json!({
                                    "role": "user",
                                    "content": text,
                                    "timestamp": tx.session().now(),
                                })]),
                                data: Some(object_of(json!({
                                    "continuation": true,
                                    "from": assistant,
                                }))),
                                ..Default::default()
                            },
                        )?;
                        let successor =
                            tx.create_task(crate::agent_core::harness::pico3::types::TaskSpec {
                                kind: "pi.generation".to_owned(),
                                conversation_id: Some(c),
                                input: json!({ "inputs": input_inputs }),
                                after: Vec::new(),
                                background: false,
                            })?;
                        return Ok(Completion::completed(json!({
                            "assistant": assistant,
                            "tools": [],
                            "successor": successor,
                        })));
                    }
                }
                tx.resolve_inputs(
                    &input_inputs,
                    &crate::agent_core::harness::pico3::session::Resolution::Done {
                        answer: assistant,
                    },
                )
                .await?;
                tx.emit_event(ViewEvent::TurnEndedDone {
                    inputs: input_inputs.clone(),
                    answer: assistant,
                })?;
                if !boundary.triggers.is_empty() {
                    let successor =
                        tx.create_task(crate::agent_core::harness::pico3::types::TaskSpec {
                            kind: "pi.generation".to_owned(),
                            conversation_id: Some(c),
                            input: json!({ "inputs": boundary.triggers }),
                            after: Vec::new(),
                            background: false,
                        })?;
                    tx.emit_event(ViewEvent::TurnStarted {
                        inputs: boundary.triggers,
                    })?;
                    return Ok(Completion::completed(json!({
                        "assistant": assistant,
                        "tools": [],
                        "successor": successor,
                    })));
                }
                Ok(Completion::completed(json!({
                    "assistant": assistant,
                    "tools": [],
                })))
            }
            .boxed()
        },
    )))
}

/// Upstream `terminalError` (`generation.ts:531-552`): a display-only
/// assistant entry (no `model`), inputs unanswered.
fn terminal_error(task: &Task, cp: &Value, message: &AssistantMessage, reason: &str) -> Step {
    let reason = reason.to_owned();
    let detail = message
        .error_message
        .clone()
        .unwrap_or_else(|| "provider error".to_owned());
    let stored = match serde_json::to_value(message) {
        Ok(mut value) => {
            if let Some(object) = value.as_object_mut() {
                object.insert("role".to_owned(), Value::String("assistant".to_owned()));
            }
            value
        }
        Err(error) => Value::String(format!("{error}")),
    };
    let attempt = cp.get("attempt").and_then(Value::as_i64).unwrap_or(0);
    let display_reason = if message.stop_reason == StopReason::Aborted {
        "aborted"
    } else {
        "error"
    }
    .to_owned();
    let input_inputs = task_input_inputs(task);
    Step::Done(Box::new(
        move |tx: &mut Tx, current: Task, _ctx: Context| {
            let stored = stored.clone();
            let detail = detail.clone();
            let display_reason = display_reason.clone();
            let input_inputs = input_inputs.clone();
            async move {
                let head = tx.newest_entry(current.conversation_id, None, true).await?;
                let assistant = tx.append_entry(
                    current.conversation_id,
                    NewEntry {
                        kind: "pi.assistant".to_owned(),
                        data: Some(object_of(json!({
                            "attempt": attempt,
                            "display": stored,
                            "reason": display_reason,
                        }))),
                        ..Default::default()
                    },
                )?;
                tx.emit_event(ViewEvent::GenerationFailed {
                    task_id: current.id,
                    reason: reason.to_owned(),
                    detail: detail.clone(),
                    entry: Some(assistant),
                })?;
                settle_failed_turn(
                    tx,
                    &current,
                    &input_inputs,
                    &detail,
                    head.as_ref().map(|entry| entry.id),
                )
                .await?;
                Ok(Completion {
                    status: "failed".to_owned(),
                    result: None,
                    failure: Some(json!({
                        "reason": reason,
                        "detail": detail,
                        "assistant": assistant,
                    })),
                })
            }
            .boxed()
        },
    ))
}

/// Upstream `settleFailedTurn` (`generation.ts:554-569`).
async fn settle_failed_turn(
    tx: &mut Tx,
    current: &Task,
    inputs: &[Id],
    detail: &str,
    head_boundary: Option<Id>,
) -> anyhow::Result<()> {
    tx.sticky_set(current.conversation_id, "turn", json!({ "tools": [] }))?;
    tx.resolve_inputs(
        inputs,
        &crate::agent_core::harness::pico3::session::Resolution::Unanswered {
            reason: "failed".to_owned(),
            detail: Some(detail.to_owned()),
        },
    )
    .await?;
    tx.emit_event(ViewEvent::TurnEndedUnanswered {
        inputs: inputs.to_vec(),
        reason: "failed".to_owned(),
        detail: Some(detail.to_owned()),
    })?;
    let boundary = tx
        .boundary(current.conversation_id, "final", head_boundary)
        .await?;
    if !boundary.triggers.is_empty() {
        tx.create_task(crate::agent_core::harness::pico3::types::TaskSpec {
            kind: "pi.generation".to_owned(),
            conversation_id: Some(current.conversation_id),
            input: json!({ "inputs": boundary.triggers }),
            after: Vec::new(),
            background: false,
        })?;
        tx.emit_event(ViewEvent::TurnStarted {
            inputs: boundary.triggers,
        })?;
    }
    Ok(())
}

/// Upstream `RetryDecision` (`generation.ts:571`).
pub enum RetryDecision {
    Retry { until_ms: i64 },
    Fail(String),
}

/// Upstream `retryDecision` (`generation.ts:572-583`).
pub fn retry_decision(cp: &Value, message: Option<&AssistantMessage>, now: i64) -> RetryDecision {
    if let Some(message) = message {
        if !is_retryable_assistant_error(message) {
            return RetryDecision::Fail("provider".to_owned());
        }
    }
    let retry = cp.get("retry").cloned().unwrap_or(Value::Null);
    if !retry
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return RetryDecision::Fail("provider".to_owned());
    }
    let attempt = cp.get("attempt").and_then(Value::as_i64).unwrap_or(0);
    let max_retries = retry.get("maxRetries").and_then(Value::as_i64).unwrap_or(0);
    if attempt > max_retries {
        return RetryDecision::Fail("retries_exhausted".to_owned());
    }
    let base_delay_ms = retry
        .get("baseDelayMs")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let delay = base_delay_ms * 2f64.powi(i32::try_from((attempt - 1).max(0)).unwrap_or(0));
    let safe_delay: i64 = if delay.is_finite() && delay <= 9_007_199_254_740_991.0 {
        delay as i64
    } else {
        9_007_199_254_740_991
    };
    let max_agent_delay_ms = retry
        .get("maxAgentDelayMs")
        .and_then(Value::as_i64)
        .unwrap_or(60_000);
    RetryDecision::Retry {
        until_ms: now + safe_delay.min(max_agent_delay_ms),
    }
}

/// Upstream `thresholdCollapseThrough` (`generation.ts:590-606`): the
/// read-side of threshold collapse, computed before the assistant is
/// appended.
async fn threshold_collapse_through(
    tx: &mut Tx,
    conversation_id: Id,
    message: &Value,
) -> anyhow::Result<Option<Id>> {
    let live_collapses = tx
        .tasks(&crate::agent_core::harness::pico3::types::TaskScan {
            conversation_id: Some(conversation_id),
            kind: Some("pi.collapse".to_owned()),
            status: Some(vec![
                crate::agent_core::harness::pico3::types::TaskStatus::Pending,
                crate::agent_core::harness::pico3::types::TaskStatus::Running,
            ]),
        })
        .await?;
    if !live_collapses.is_empty() {
        return Ok(None);
    }
    let state = tx.snapshot(
        crate::agent_core::harness::pico3::types::DocRef::Rewindable { conversation_id },
    )?;
    let threshold = state
        .get("threshold")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    if threshold <= 0.0 {
        return Ok(None);
    }
    let context = tx.context(conversation_id, None).await?;
    let usage_input = message
        .get("usage")
        .and_then(|usage| usage.get("input"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let usage_output = message
        .get("usage")
        .and_then(|usage| usage.get("output"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let mut projected = context.messages.clone();
    projected.push(message.clone());
    let used = i64::max(
        usage_input + usage_output,
        estimate_context_tokens(&projected).tokens,
    );
    if used as f64 <= threshold {
        return Ok(None);
    }
    let keep_recent = state
        .get("keepRecent")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    Ok(choose_through(&context.entries, keep_recent))
}

/// Upstream `overflow` (`generation.ts:608-641`).
fn overflow() -> Step {
    Step::Done(Box::new(
        move |tx: &mut Tx, current: Task, _ctx: Context| {
            async move {
                let c = current.conversation_id;
                let state = tx.snapshot(crate::agent_core::harness::pico3::types::DocRef::Rewindable {
                    conversation_id: c,
                })?;
                let context = tx.context(c, None).await?;
                let keep_recent = state
                    .get("keepRecent")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                let through = choose_through(&context.entries, keep_recent);
                tx.sticky_set(c, "turn", json!({ "tools": [] }))?;
                let head_boundary = context.head.as_ref().map(|entry| entry.id);
                let input_inputs = task_input_inputs(&current);
                match through {
                    None => {
                        tx.write(
                            c,
                            NewEntry {
                                kind: "pi.notice".to_owned(),
                                model: Some(vec![json!({
                                    "role": "user",
                                    "content": "Context too large; nothing to compact.",
                                    "timestamp": tx.session().now(),
                                })]),
                                ..Default::default()
                            },
                        )
                        .await?;
                        tx.emit_event(ViewEvent::GenerationFailed {
                            task_id: current.id,
                            reason: "overflow".to_owned(),
                            detail: "request exceeds context window and nothing is collapsible"
                                .to_owned(),
                            entry: None,
                        })?;
                        settle_failed_turn(tx, &current, &input_inputs, "overflow", head_boundary)
                            .await?;
                        Ok(Completion {
                            status: "failed".to_owned(),
                            result: None,
                            failure: Some(json!({
                                "reason": "overflow",
                                "detail": "request exceeds context window and nothing is collapsible",
                            })),
                        })
                    }
                    Some(through) => {
                        let collapse =
                            tx.create_task(crate::agent_core::harness::pico3::types::TaskSpec {
                                kind: "pi.collapse".to_owned(),
                                conversation_id: Some(c),
                                input: json!({ "reason": "overflow", "through": through }),
                                after: Vec::new(),
                                background: false,
                            })?;
                        tx.create_task(crate::agent_core::harness::pico3::types::TaskSpec {
                            kind: "pi.generation".to_owned(),
                            conversation_id: Some(c),
                            input: json!({ "inputs": input_inputs }),
                            after: vec![collapse],
                            background: false,
                        })?;
                        tx.emit_event(ViewEvent::GenerationFailed {
                            task_id: current.id,
                            reason: "overflow".to_owned(),
                            detail: format!("collapsing through {through}"),
                            entry: None,
                        })?;
                        Ok(Completion {
                            status: "failed".to_owned(),
                            result: None,
                            failure: Some(json!({
                                "reason": "overflow",
                                "detail": format!("collapsing through {through}"),
                            })),
                        })
                    }
                }
            }.boxed()
        },
    ))
}

// ---------------------------------------------------------------------------
// Small shared helpers
// ---------------------------------------------------------------------------

/// Upstream `failStep` (`generation.ts:102-109`).
fn fail_step(reason: &str, detail: &str) -> Step {
    let reason = reason.to_owned();
    let detail = detail.to_owned();
    Step::Done(Box::new(
        move |tx: &mut Tx, current: Task, _ctx: Context| {
            let reason = reason.clone();
            let detail = detail.clone();
            async move {
                let head = tx.newest_entry(current.conversation_id, None, true).await?;
                tx.emit_event(ViewEvent::GenerationFailed {
                    task_id: current.id,
                    reason: reason.clone(),
                    detail: detail.clone(),
                    entry: None,
                })?;
                settle_failed_turn(
                    tx,
                    &current,
                    &task_input_inputs(&current),
                    &detail,
                    head.as_ref().map(|entry| entry.id),
                )
                .await?;
                Ok(Completion {
                    status: "failed".to_owned(),
                    result: None,
                    failure: Some(json!({ "reason": reason, "detail": detail })),
                })
            }
            .boxed()
        },
    ))
}

/// `task.input.inputs` (`generation.ts:37`).
fn task_input_inputs(task: &Task) -> Vec<Id> {
    task.input
        .get("inputs")
        .and_then(Value::as_array)
        .map(|inputs| inputs.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default()
}

fn object_of(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

/// The task's checkpoint as a JSON value (`task.checkpoint`).
fn checkpoint_of(task: &Task) -> Value {
    task.checkpoint
        .as_ref()
        .map(|checkpoint| Value::Object(checkpoint.clone()))
        .unwrap_or(Value::Null)
}

fn prep(cp: &Value, key: &str) -> Value {
    cp.get(key).cloned().unwrap_or(Value::Null)
}

/// `tx.sticky(c).turn.message = stored` (targeted write with merge-back
/// under the line lock).
fn set_turn_message(tx: &mut Tx, conversation_id: Id, stored: Value) -> anyhow::Result<()> {
    let mut turn = turn_object(tx, conversation_id)?;
    turn.insert("message".to_owned(), stored);
    tx.sticky_set(conversation_id, "turn", Value::Object(turn))
}

/// `tx.sticky(c).turn.message = undefined`.
fn clear_turn_message(tx: &mut Tx, conversation_id: Id) -> anyhow::Result<()> {
    let mut turn = turn_object(tx, conversation_id)?;
    turn.shift_remove("message");
    tx.sticky_set(conversation_id, "turn", Value::Object(turn))
}

fn turn_object(tx: &mut Tx, conversation_id: Id) -> anyhow::Result<Map<String, Value>> {
    let sticky =
        tx.snapshot(crate::agent_core::harness::pico3::types::DocRef::Sticky { conversation_id })?;
    Ok(sticky
        .get("turn")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default())
}
