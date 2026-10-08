//! Port of `packages/agent/src/harness/pico3/kinds/collapse.ts` (274
//! lines): the `pi.collapse` kind — summarization with stale-head
//! protection, hook-driven decline/summary/instructions overrides, durable
//! retry — plus `chooseThrough`, the shared exchange-window selector.

use std::sync::Arc;

use futures::future::BoxFuture;
use futures::FutureExt;
use futures::StreamExt;
use serde_json::{json, Value};

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::pico3::session::Tx;
use crate::agent_core::harness::pico3::types::{
    task_contract_fault, AnyKind, BasicKind, Completion, Entry, Id, KindConfig, NewEntry, Task,
    ViewEvent,
};
use crate::ai::types::events::{AssistantMessageEvent, PartialAssistant};
use crate::ai::types::message::AssistantMessage;
use crate::ai::types::primitives::StopReason;

use super::estimate::estimate_context_tokens;
use super::generation::{retry_decision, RetryDecision};
use crate::agent_core::harness::pico3::runtime::{
    AbortClosure, HookApi, HookHandlers, Runtime, Step,
};
use crate::agent_core::harness::pico3::system::effective_tools;

// ---------------------------------------------------------------------------
// Hooks (`CollapseHooks`, collapse.ts:47-55)
// ---------------------------------------------------------------------------

/// Upstream `CollapseHooks` (`collapse.ts:47-55`).
#[derive(Default)]
pub struct CollapseHandlers {
    /// `beforeCollapse(reason, through, entries, info, ctx)`: first
    /// decision wins.
    pub before_collapse: Option<BeforeCollapseFn>,
}

impl HookHandlers for CollapseHandlers {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// The decision shape: `{ decline: true }` or `{ instructions?, summary? }`.
pub enum CollapseDecision {
    Decline,
    Override {
        instructions: Option<String>,
        summary: Option<String>,
    },
}

/// `beforeCollapse` handler type.
pub type BeforeCollapseFn = Arc<
    dyn Fn(
            &str,
            Id,
            &[Entry],
            &HookApi,
            Context,
        ) -> BoxFuture<'static, anyhow::Result<Option<CollapseDecision>>>
        + Send
        + Sync,
>;

// ---------------------------------------------------------------------------
// Config (collapse.ts:56)
// ---------------------------------------------------------------------------

/// Upstream `collapseConfig` (`collapse.ts:56`).
pub fn collapse_config() -> KindConfig {
    KindConfig {
        rewindable: json!({ "threshold": 0, "keepRecent": 20000 })
            .as_object()
            .cloned()
            .expect("object"),
        sticky: Default::default(),
        declared_absent: Default::default(),
    }
}

/// Upstream `Base` (`collapse.ts:31-38`).
fn base_of(checkpoint: &Value) -> Value {
    let mut base = checkpoint.as_object().cloned().unwrap_or_default();
    base.shift_remove("phase");
    base.shift_remove("untilMs");
    base.shift_remove("lastError");
    base.shift_remove("summary");
    Value::Object(base)
}

/// The registered kind instance (`collapse.ts:80-163`).
pub struct CollapseKind;

impl CollapseKind {
    pub fn new() -> Arc<Self> {
        Arc::new(CollapseKind)
    }
}

impl crate::agent_core::harness::pico3::runtime::Kind for CollapseKind {
    fn metadata(&self) -> Arc<dyn AnyKind> {
        Arc::new(
            BasicKind::new("pi.collapse")
                .config(collapse_config())
                .inflight(vec!["summarizing".to_owned()]),
        )
    }

    fn initial(
        &self,
        task: Arc<Task>,
        rt: Arc<Runtime>,
        ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<Step>> {
        async move {
            let _checkpoint = checkpoint_of(&task);
            let state = rt.rewindable(task.conversation_id, ctx.clone()).await?;
            if state.get("model").map(|model| model.is_null()).unwrap_or(true) {
                return Ok(done_failed("no_model", "no model configured"));
            }
            let sticky = rt.sticky(task.conversation_id, ctx.clone()).await?;
            let head = rt
                .newest_entry(task.conversation_id, None, true, ctx.clone())
                .await?;
            let through = task
                .input
                .get("through")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let context = rt
                .context(task.conversation_id, Some(through), ctx.clone())
                .await?;
            // `beforeCollapse` hooks: first decision wins
            // (`collapse.ts:100-108`).
            let mut decision: Option<CollapseDecision> = None;
            let handlers: Vec<(BeforeCollapseFn, HookApi)> = rt
                .hooks
                .bindings()
                .iter()
                .filter_map(|binding| {
                    binding
                        .handlers
                        .downcast_ref::<CollapseHandlers>()
                        .and_then(|handlers| handlers.before_collapse.clone())
                        .map(|handler| (handler, binding.api.clone()))
                })
                .collect();
            for (handler, api) in &handlers {
                let reason = task
                    .input
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("manual")
                    .to_owned();
                match handler(&reason, through, &context.entries, api, ctx.clone()).await {
                    Ok(Some(value)) => {
                        decision = Some(value);
                        break;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        let aborted =
                            ctx.abort_signal().is_some_and(|signal| signal.is_cancelled());
                        if aborted {
                            return Err(error);
                        }
                    }
                }
            }
            if let Some(CollapseDecision::Decline) = decision {
                return Ok(done_failed("declined", "declined by beforeCollapse"));
            }
            let (choice_instructions, choice_summary) = match &decision {
                Some(CollapseDecision::Override {
                    instructions,
                    summary,
                }) => (instructions.clone(), summary.clone()),
                _ => (None, None),
            };
            let input_instructions = task
                .input
                .get("instructions")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let instructions = choice_instructions.or(input_instructions);
            let mut base = object_of(json!({
                "expectedHead": head.as_ref().map(|entry| entry.id),
                "model": state.get("model").cloned().unwrap_or(Value::Null),
                "thinkingLevel": state.get("thinkingLevel").cloned().unwrap_or(Value::Null),
                "retry": sticky.get("retry").cloned().unwrap_or(Value::Null),
                "attempt": 1,
            }));
            if let Some(instructions) = &instructions {
                base.insert("instructions".to_owned(), Value::String(instructions.clone()));
            }
            let base = Value::Object(base);
            if let Some(summary) = choice_summary {
                // `next: async (tx, current) => …` (`collapse.ts:122-128`).
                return Ok(Step::Next(crate::agent_core::harness::pico3::runtime::Next::Defer(
                    Box::new(move |tx: &mut Tx, current: Task, _ctx: Context| {
                        let base = base.clone();
                        let summary = summary.clone();
                        async move {
                            if head_moved(tx, current.conversation_id, &base).await? {
                                return Ok(crate::agent_core::harness::pico3::runtime::Next::Completion(
                                    failed("stale", "head moved during beforeCollapse"),
                                ));
                            }
                            let mut next = base.as_object().cloned().unwrap_or_default();
                            next.insert("phase".to_owned(), Value::String("prepared".to_owned()));
                            next.insert("summary".to_owned(), Value::String(summary));
                            Ok(crate::agent_core::harness::pico3::runtime::Next::Checkpoint(
                                next,
                            ))
                        }.boxed()
                    }),
                )));
            }
            summarize_now(&task, base, rt, ctx, true).await
        }.boxed()
    }

    fn phases(&self) -> Vec<String> {
        vec![
            "summarizing".to_owned(),
            "retrying".to_owned(),
            "prepared".to_owned(),
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
                "summarizing" => {
                    // `afterFailure(baseOf(checkpoint), null, "interrupted")`
                    // (`collapse.ts:133-135`).
                    let checkpoint = checkpoint_of(&task);
                    Ok(after_failure(&base_of(&checkpoint), None, "interrupted", &rt))
                }
                "retrying" => {
                    let checkpoint = checkpoint_of(&task);
                    let until_ms = checkpoint
                        .get("untilMs")
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    rt.sleep(until_ms, ctx.clone()).await?;
                    let mut base = base_of(&checkpoint);
                    if let Some(object) = base.as_object_mut() {
                        let attempt = object
                            .get("attempt")
                            .and_then(Value::as_i64)
                            .unwrap_or(0)
                            + 1;
                        object.insert("attempt".to_owned(), json!(attempt));
                    }
                    summarize_now(&task, base, rt, ctx, false).await
                }
                "prepared" => {
                    let checkpoint = checkpoint_of(&task);
                    let base = base_of(&checkpoint);
                    let summary = checkpoint
                        .get("summary")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let through = task
                        .input
                        .get("through")
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    let now = rt.now();
                    Ok(Step::Done(Box::new(
                        move |tx: &mut Tx, current: Task, _ctx: Context| {
                            let base = base.clone();
                            let summary = summary.clone();
                            async move {
                                let context = tx.context(current.conversation_id, None).await?;
                                if context.head.as_ref().map(|entry| entry.id)
                                    != base.get("expectedHead").and_then(Value::as_i64)
                                {
                                    return Ok(failed("stale", "head moved during collapse"));
                                }
                                let retained = context
                                    .entries
                                    .iter()
                                    .find(|entry| entry.id > through)
                                    .map(|entry| entry.id);
                                let id = tx.append_entry(
                                    current.conversation_id,
                                    NewEntry {
                                        kind: "pi.summary".to_owned(),
                                        data: Some(object_of(json!({ "through": through }))),
                                        model: Some(vec![json!({
                                            "role": "user",
                                            "content": summary,
                                            "timestamp": now,
                                        })]),
                                        head: Some(match retained {
                                            Some(id) =>
                                                crate::agent_core::harness::pico3::types::Head::Id(id),
                                            None => crate::agent_core::harness::pico3::types::Head::Self_,
                                        }),
                                        ..Default::default()
                                    },
                                )?;
                                Ok(Completion::completed(json!({ "summary": id })))
                            }.boxed()
                        },
                    )))
                }
                other => Err(task_contract_fault(
                    "pi.collapse",
                    &format!("no handler for phase {other}"),
                )),
            }
        }.boxed()
    }

    fn abort(
        &self,
        _task: Arc<Task>,
        _rt: Arc<Runtime>,
        _ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<AbortClosure>> {
        // `async abort() { return async () => null }` (`collapse.ts:160-162`).
        async move {
            Ok(
                crate::agent_core::harness::pico3::runtime::abort_closure_from(
                    |_tx: &mut Tx, _current: Task, _ctx: Context| {
                        Box::pin(async move { Ok(Value::Null) })
                    },
                ),
            )
        }
        .boxed()
    }
}

/// Upstream `headMoved` (`collapse.ts:165-166`).
async fn head_moved(tx: &mut Tx, conversation_id: Id, base: &Value) -> anyhow::Result<bool> {
    Ok(tx
        .newest_entry(conversation_id, None, true)
        .await?
        .map(|entry| entry.id)
        != base.get("expectedHead").and_then(Value::as_i64))
}

/// Upstream `summarizeNow` (`collapse.ts:168-227`).
async fn summarize_now(
    task: &Task,
    base: Value,
    rt: Arc<Runtime>,
    ctx: Context,
    check_head: bool,
) -> anyhow::Result<Step> {
    // The stale check + in-flight checkpoint write (`collapse.ts:175-179`).
    {
        let base = base.clone();
        rt.commit_erased_with(
            ctx.clone(),
            Box::new(move |tx: &mut Tx, current: Task, _ctx: Context| {
                let base = base.clone();
                async move {
                    if check_head && head_moved(tx, current.conversation_id, &base).await? {
                        return Ok(Value::Bool(true));
                    }
                    let mut checkpoint = base.as_object().cloned().unwrap_or_default();
                    checkpoint.insert("phase".to_owned(), Value::String("summarizing".to_owned()));
                    tx.checkpoint(Value::Object(checkpoint))?;
                    Ok(Value::Bool(false))
                }
                .boxed()
            }),
        )
        .await?;
    }
    let model_ref: crate::agent_core::harness::pico3::types::ModelRef =
        serde_json::from_value(base.get("model").cloned().unwrap_or(Value::Null))?;
    let Some(model) = rt.models.resolve(&model_ref) else {
        return Ok(done_failed("no_model", "model unavailable"));
    };
    let through = task
        .input
        .get("through")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let context = rt
        .context(task.conversation_id, Some(through), ctx.clone())
        .await?;
    let tools = effective_tools(&context.messages);
    let mut request_messages = context.messages.clone();
    if !tools.is_empty() {
        request_messages.push(json!({
            "role": "system",
            "content": "",
            "toolsRemoved": tools,
            "timestamp": rt.now(),
        }));
    }
    let instructions = base
        .get("instructions")
        .and_then(Value::as_str)
        .unwrap_or("Summarize the conversation so far for continuation.");
    request_messages.push(json!({
        "role": "user",
        "content": format!("{instructions}\n\nRespond with the summary only."),
        "timestamp": rt.now(),
    }));
    let thinking_level = base
        .get("thinkingLevel")
        .and_then(Value::as_str)
        .unwrap_or("off")
        .to_owned();
    // Stream to the terminal message (`collapse.ts:197-215`).
    let mut message: Option<AssistantMessage> = None;
    let mut stream = rt.models.stream(
        model,
        crate::agent_core::harness::pico3::runtime::GenerationRequest {
            messages: request_messages,
            thinking_level,
        },
        ctx.clone(),
    );
    let mut partial = PartialAssistant::new();
    loop {
        match stream.next().await {
            Some(Ok(event)) => match event {
                AssistantMessageEvent::Done { message: done, .. } => {
                    message = Some(done);
                    break;
                }
                AssistantMessageEvent::Error { error, .. } => {
                    message = Some(error);
                    break;
                }
                other => {
                    if let Err(error) = partial.apply(&other) {
                        anyhow::bail!("{error}");
                    }
                }
            },
            Some(Err(error)) => {
                let aborted = ctx
                    .abort_signal()
                    .is_some_and(|signal| signal.is_cancelled());
                if aborted {
                    return Err(error);
                }
                return Ok(after_failure(&base, None, &format!("{error}"), &rt));
            }
            None => break,
        }
    }
    let Some(message) = message else {
        return Ok(after_failure(&base, None, "no message", &rt));
    };
    let has_tool_calls = message
        .content
        .iter()
        .any(|block| matches!(block, crate::ai::types::AssistantBlock::ToolCall(_)));
    if has_tool_calls {
        let mut errored = message.clone();
        errored.stop_reason = StopReason::Error;
        return Ok(after_failure(
            &base,
            Some(&errored),
            "summarizer returned tool calls",
            &rt,
        ));
    }
    if message.stop_reason == StopReason::Error {
        let detail = message
            .error_message
            .clone()
            .unwrap_or_else(|| "provider error".to_owned());
        return Ok(after_failure(&base, Some(&message), &detail, &rt));
    }
    let summary = message
        .content
        .iter()
        .filter_map(|block| match block {
            crate::ai::types::AssistantBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<String>();
    Ok(Step::Next(
        crate::agent_core::harness::pico3::runtime::Next::Defer(Box::new(
            move |_tx: &mut Tx, _current: Task, _ctx: Context| {
                let base = base.clone();
                let summary = summary.clone();
                async move {
                    let mut next = base.as_object().cloned().unwrap_or_default();
                    next.insert("phase".to_owned(), Value::String("prepared".to_owned()));
                    next.insert("summary".to_owned(), Value::String(summary));
                    Ok(crate::agent_core::harness::pico3::runtime::Next::Checkpoint(next))
                }
                .boxed()
            },
        )),
    ))
}

/// Upstream `afterFailure` (`collapse.ts:229-249`).
fn after_failure(
    base: &Value,
    message: Option<&AssistantMessage>,
    detail: &str,
    rt: &Arc<Runtime>,
) -> Step {
    match retry_decision(base, message, rt.now()) {
        RetryDecision::Fail(reason) => done_failed(&reason, detail),
        RetryDecision::Retry { until_ms } => {
            let detail = detail.to_owned();
            let base = base.clone();
            Step::Next(crate::agent_core::harness::pico3::runtime::Next::Defer(
                Box::new(move |tx: &mut Tx, current: Task, _ctx: Context| {
                    let base = base.clone();
                    let detail = detail.clone();
                    async move {
                        let attempt = base.get("attempt").and_then(Value::as_i64).unwrap_or(0);
                        tx.emit_event(ViewEvent::CompactionRetrying {
                            task_id: current.id,
                            attempt,
                            retry_at: until_ms,
                            error: detail.clone(),
                        })?;
                        let mut next = base.as_object().cloned().unwrap_or_default();
                        next.insert("phase".to_owned(), Value::String("retrying".to_owned()));
                        next.insert("untilMs".to_owned(), json!(until_ms));
                        next.insert("lastError".to_owned(), Value::String(detail));
                        Ok(crate::agent_core::harness::pico3::runtime::Next::Checkpoint(next))
                    }
                    .boxed()
                }),
            ))
        }
    }
}

/// Upstream `failed(...)` completion (`collapse.ts:61-64`).
fn failed(reason: &str, detail: &str) -> Completion {
    Completion {
        status: "failed".to_owned(),
        result: None,
        failure: Some(json!({ "reason": reason, "detail": detail })),
    }
}

/// A `done` step that immediately fails.
fn done_failed(reason: &str, detail: &str) -> Step {
    let completion = failed(reason, detail);
    Step::Done(Box::new(
        move |_tx: &mut Tx, _current: Task, _ctx: Context| {
            let completion = completion.clone();
            async move { Ok(completion) }.boxed()
        },
    ))
}

fn object_of(value: Value) -> serde_json::Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

fn checkpoint_of(task: &Task) -> Value {
    task.checkpoint
        .as_ref()
        .map(|checkpoint| Value::Object(checkpoint.clone()))
        .unwrap_or(Value::Null)
}

/// Upstream `chooseThrough` (`collapse.ts:251-274`): group entries into
/// assistant-led exchanges, retain the newest suffix within `keepRecent`
/// tokens, and return the cut point's entry id.
pub fn choose_through(entries: &[Entry], keep_recent: f64) -> Option<Id> {
    struct Exchange {
        last: Id,
        tokens: i64,
    }
    let mut exchanges: Vec<Exchange> = Vec::new();
    let mut open: Option<Exchange> = None;
    for entry in entries {
        let role = entry
            .model
            .as_ref()
            .and_then(|model| model.first())
            .and_then(|message| message.get("role"))
            .and_then(Value::as_str);
        let tokens = estimate_context_tokens(entry.model.as_deref().unwrap_or(&[])).tokens;
        match role {
            Some("assistant") => {
                let exchange = Exchange {
                    last: entry.id,
                    tokens,
                };
                open = Some(Exchange {
                    last: exchange.last,
                    tokens: exchange.tokens,
                });
                exchanges.push(exchange);
            }
            Some("toolResult") => {
                if let Some(exchange) = open.as_mut() {
                    exchange.last = entry.id;
                    exchange.tokens += tokens;
                }
            }
            _ => {
                open = None;
                exchanges.push(Exchange {
                    last: entry.id,
                    tokens,
                });
            }
        }
    }
    let mut retained: f64 = 0.0;
    let mut index = exchanges.len() as i64 - 1;
    while index >= 0 && retained + exchanges[index as usize].tokens as f64 <= keep_recent {
        retained += exchanges[index as usize].tokens as f64;
        index -= 1;
    }
    if index < 0 {
        return None;
    }
    if index == exchanges.len() as i64 - 1 {
        return exchanges
            .get(index as usize - 1)
            .map(|exchange| exchange.last);
    }
    exchanges.get(index as usize).map(|exchange| exchange.last)
}

#[cfg(test)]
mod json_order_tests {
    use super::*;

    #[test]
    fn json_order_checkpoint_rest_preserves_surviving_fields() {
        let cp =
            json!({"phase":"waiting","z":1,"untilMs":2,"a":3,"summary":"s","lastError":"e","b":4});
        let before = cp.to_string();
        assert_eq!(base_of(&cp).to_string(), r#"{"z":1,"a":3,"b":4}"#);
        assert_eq!(cp.to_string(), before);
    }
}
