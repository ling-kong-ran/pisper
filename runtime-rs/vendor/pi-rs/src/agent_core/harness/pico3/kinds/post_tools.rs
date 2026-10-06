//! Port of `packages/agent/src/harness/pico3/kinds/post-tools.ts` (165
//! lines): the `pi.post_tools` kind — afterTools hooks, tool-control
//! application (addTools/terminate/handoff), synthesized missing results,
//! and the postTools boundary that admits queued triggers.

use std::sync::Arc;

use futures::future::BoxFuture;
use futures::FutureExt;
use serde_json::{json, Value};

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::pico3::session::Tx;
use crate::agent_core::harness::pico3::types::{
    task_contract_fault, AnyKind, BasicKind, Completion, Id, KindConfig, NewEntry, Task, ViewEvent,
};

use crate::agent_core::harness::pico3::runtime::{
    AbortClosure, HookApi, HookHandlers, Runtime, Step,
};

/// Upstream `PostToolsHooks` (`post-tools.ts:8-10`).
#[derive(Default)]
pub struct PostToolsHandlers {
    /// `afterTools(assistant, results, info, ctx)`: observer.
    pub after_tools: Option<AfterToolsFn>,
}

impl HookHandlers for PostToolsHandlers {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub type AfterToolsFn = Arc<
    dyn Fn(Id, &[Id], &HookApi, Context) -> BoxFuture<'static, anyhow::Result<()>> + Send + Sync,
>;

/// Upstream `postToolsConfig` (`post-tools.ts:11-16`).
pub fn post_tools_config() -> KindConfig {
    KindConfig {
        rewindable: Default::default(),
        sticky: json!({
            "steeringMode": "one-at-a-time",
            "followUpMode": "one-at-a-time",
        })
        .as_object()
        .cloned()
        .expect("object"),
        declared_absent: Default::default(),
    }
}

/// The registered kind instance (`post-tools.ts:18-165`).
pub struct PostToolsKind;

impl PostToolsKind {
    pub fn new() -> Arc<Self> {
        Arc::new(PostToolsKind)
    }
}

/// One tool row collected in the first commit (`post-tools.ts:39-62`).
#[derive(Clone)]
struct ToolRow {
    call: Value,
    entry: Option<Id>,
    control: Option<Value>,
    missing: Option<&'static str>,
}

impl crate::agent_core::harness::pico3::runtime::Kind for PostToolsKind {
    fn metadata(&self) -> Arc<dyn AnyKind> {
        Arc::new(
            BasicKind::new("pi.post_tools")
                .turn(true)
                .config(post_tools_config()),
        )
    }

    fn initial(
        &self,
        task: Arc<Task>,
        rt: Arc<Runtime>,
        ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<Step>> {
        async move {
            let input_assistant = task.input.get("assistant").and_then(Value::as_i64).unwrap_or(0);
            let input_tools: Vec<Id> = task
                .input
                .get("tools")
                .and_then(Value::as_array)
                .map(|array| array.iter().filter_map(Value::as_i64).collect())
                .unwrap_or_default();
            let input_inputs: Vec<Id> = task
                .input
                .get("inputs")
                .and_then(Value::as_array)
                .map(|array| array.iter().filter_map(Value::as_i64).collect())
                .unwrap_or_default();
            // First commit: collect rows (`post-tools.ts:33-63`).
            let rows: Vec<ToolRow> = rt
                .commit_typed(
                    ctx.clone(),
                    move |tx: &mut Tx, _current: Task, _ctx: Context| {
                        let input_tools = input_tools.clone();
                        let input_assistant = input_assistant;
                        async move {
                            let mut rows = Vec::new();
                            let assistant = tx.entry(input_assistant).await?;
                            let calls: Vec<Value> = assistant
                                .and_then(|entry| entry.model)
                                .and_then(|model| model.first().cloned())
                                .and_then(|message| message.get("content").and_then(Value::as_array).cloned())
                                .map(|content| {
                                    content
                                        .into_iter()
                                        .filter(|block| block.get("type").and_then(Value::as_str) == Some("toolCall"))
                                        .collect()
                                })
                                .unwrap_or_default();
                            for (index, id) in input_tools.iter().enumerate() {
                                let tool_task = tx.task(*id).await?
                                    .ok_or_else(|| anyhow::anyhow!("tool task {id} not found"))?;
                                let call = calls.get(index).cloned().unwrap_or(Value::Null);
                                match &tool_task.outcome {
                                    Some(outcome) if outcome.status == "completed" => {
                                        rows.push(ToolRow {
                                            call,
                                            entry: outcome
                                                .result
                                                .as_ref()
                                                .and_then(|result| result.get("entry"))
                                                .and_then(Value::as_i64),
                                            control: outcome
                                                .result
                                                .as_ref()
                                                .and_then(|result| result.get("control"))
                                                .cloned()
                                                .filter(|control| !control.is_null()),
                                            missing: None,
                                        });
                                    }
                                    Some(outcome) if outcome.status == "aborted" => {
                                        let aborted_entry = outcome
                                            .result
                                            .as_ref()
                                            .and_then(|result| result.get("entry"))
                                            .and_then(Value::as_i64);
                                        rows.push(ToolRow {
                                            call,
                                            entry: aborted_entry,
                                            control: None,
                                            missing: if aborted_entry.is_none() {
                                                Some("aborted")
                                            } else {
                                                None
                                            },
                                        });
                                    }
                                    _ => {
                                        rows.push(ToolRow {
                                            call,
                                            entry: None,
                                            control: None,
                                            missing: Some("orphaned"),
                                        });
                                    }
                                }
                            }
                            Ok(rows)
                        }.boxed()
                    },
                )
                .await?;
            let results: Vec<Id> = rows.iter().filter_map(|row| row.entry).collect();
            // afterTools hooks (`post-tools.ts:65`).
            let handlers: Vec<(AfterToolsFn, HookApi)> = rt
                .hooks
                .bindings()
                .iter()
                .filter_map(|binding| {
                    binding
                        .handlers
                        .downcast_ref::<PostToolsHandlers>()
                        .and_then(|handlers| handlers.after_tools.clone())
                        .map(|handler| (handler, binding.api.clone()))
                })
                .collect();
            for (handler, api) in &handlers {
                handler(input_assistant, &results, api, ctx.clone()).await?;
            }

            let task_for_done = task.clone();
            Ok(Step::Done(Box::new(
                move |tx: &mut Tx, current: Task, _ctx: Context| {
                    let rows = rows.clone();
                    let input_inputs = input_inputs.clone();
                    let input_assistant = input_assistant;
                    let task = task_for_done.clone();
                    async move {
                        let conversation_id = current.conversation_id;
                        let head = tx.newest_entry(conversation_id, None, true).await?;
                        let state = tx.snapshot(
                            crate::agent_core::harness::pico3::types::DocRef::Rewindable {
                                conversation_id,
                            },
                        )?;
                        let mut selected_tools: Vec<String> = state
                            .get("selectedTools")
                            .and_then(Value::as_array)
                            .map(|array| {
                                array
                                    .iter()
                                    .filter_map(Value::as_str)
                                    .map(str::to_owned)
                                    .collect()
                            })
                            .unwrap_or_default();
                        let original_len = selected_tools.len();
                        let mut terminate = false;
                        let mut handoff: Option<String> = None;
                        for row in &rows {
                            if let Some(control) = &row.control {
                                if let Some(add) = control.get("addTools").and_then(Value::as_array) {
                                    for name in add {
                                        if let Some(name) = name.as_str() {
                                            if !selected_tools.iter().any(|existing| existing == name) {
                                                selected_tools.push(name.to_owned());
                                            }
                                        }
                                    }
                                }
                                if control.get("terminate").and_then(Value::as_bool) == Some(true) {
                                    terminate = true;
                                }
                                if let Some(value) = control.get("handoff").and_then(Value::as_str) {
                                    handoff = Some(value.to_owned());
                                }
                            }
                        }
                        if selected_tools.len() != original_len {
                            tx.rewindable_set(
                                conversation_id,
                                "selectedTools",
                                Value::Array(
                                    selected_tools
                                        .iter()
                                        .map(|name| Value::String(name.clone()))
                                        .collect(),
                                ),
                            )?;
                            tx.emit_event(ViewEvent::ConfigChanged {
                                keys: vec!["selectedTools".to_owned()],
                            })?;
                        }
                        for row in &rows {
                            let Some(missing) = row.missing else {
                                continue;
                            };
                            tx.append_entry(
                                conversation_id,
                                NewEntry {
                                    kind: "pi.tool_result".to_owned(),
                                    model: Some(vec![json!({
                                        "role": "toolResult",
                                        "toolCallId": row.call.get("id"),
                                        "toolName": row.call.get("name"),
                                        "content": [{ "type": "text", "text": format!("Tool result unavailable: task {missing}.") }],
                                        "isError": true,
                                        "timestamp": tx.session().now(),
                                    })]),
                                    data: Some(object_of(json!({
                                        "diagnostics": [
                                            { "severity": "error", "message": missing, "code": missing },
                                        ],
                                    }))),
                                    ..Default::default()
                                },
                            )?;
                            tx.emit_event(ViewEvent::Warning {
                                source: "post_tools".to_owned(),
                                message: format!("synthesized {missing} tool result for {}",
                                    row.call.get("id").and_then(Value::as_str).unwrap_or_default()),
                            })?;
                        }
                        let mut head_boundary = head.as_ref().map(|entry| entry.id);
                        if let Some(handoff) = &handoff {
                            head_boundary = Some(tx.append_entry(
                                conversation_id,
                                NewEntry {
                                    kind: "pi.handoff".to_owned(),
                                    head: Some(crate::agent_core::harness::pico3::types::Head::Self_),
                                    model: Some(vec![json!({
                                        "role": "user",
                                        "content": handoff,
                                        "timestamp": tx.session().now(),
                                    })]),
                                    ..Default::default()
                                },
                            )?);
                        }
                        if handoff.is_some() || terminate {
                            tx.sticky_set(conversation_id, "turn", json!({ "tools": [] }))?;
                            tx.resolve_inputs(
                                &input_inputs,
                                &crate::agent_core::harness::pico3::session::Resolution::Done {
                                    answer: input_assistant,
                                },
                            )
                            .await?;
                            tx.emit_event(ViewEvent::TurnEndedDone {
                                inputs: input_inputs.clone(),
                                answer: input_assistant,
                            })?;
                            let boundary = tx
                                .boundary(conversation_id, "final", head_boundary)
                                .await?;
                            if !boundary.triggers.is_empty() {
                                tx.emit_event(ViewEvent::TurnStarted {
                                    inputs: boundary.triggers.clone(),
                                })?;
                            }
                            let ended = if handoff.is_some() { "handoff" } else { "terminate" };
                            return Ok(Completion::completed(if boundary.triggers.is_empty() {
                                json!({ "ended": ended })
                            } else {
                                let successor = tx.create_task(
                                    crate::agent_core::harness::pico3::types::TaskSpec {
                                        kind: "pi.generation".to_owned(),
                                        conversation_id: Some(conversation_id),
                                        input: json!({ "inputs": boundary.triggers }),
                                        after: Vec::new(),
                                        background: false,
                                    },
                                )?;
                                json!({ "ended": ended, "successor": successor })
                            }));
                        }
                        let boundary = tx
                            .boundary(conversation_id, "postTools", head_boundary)
                            .await?;
                        if boundary.terminated {
                            tx.sticky_set(conversation_id, "turn", json!({ "tools": [] }))?;
                            tx.resolve_inputs(
                                &input_inputs,
                                &crate::agent_core::harness::pico3::session::Resolution::Unanswered {
                                    reason: "terminated".to_owned(),
                                    detail: None,
                                },
                            )
                            .await?;
                            tx.emit_event(ViewEvent::TurnEndedUnanswered {
                                inputs: input_inputs.clone(),
                                reason: "terminated".to_owned(),
                                detail: None,
                            })?;
                            if !boundary.triggers.is_empty() {
                                tx.emit_event(ViewEvent::TurnStarted {
                                    inputs: boundary.triggers.clone(),
                                })?;
                            }
                            return Ok(Completion::completed(if boundary.triggers.is_empty() {
                                json!({})
                            } else {
                                let successor = tx.create_task(
                                    crate::agent_core::harness::pico3::types::TaskSpec {
                                        kind: "pi.generation".to_owned(),
                                        conversation_id: Some(conversation_id),
                                        input: json!({ "inputs": boundary.triggers }),
                                        after: Vec::new(),
                                        background: false,
                                    },
                                )?;
                                json!({ "successor": successor })
                            }));
                        }
                        let successor = tx.create_task(crate::agent_core::harness::pico3::types::TaskSpec {
                            kind: "pi.generation".to_owned(),
                            conversation_id: Some(conversation_id),
                            input: {
                                    let mut inputs = input_inputs.clone();
                                    inputs.extend(boundary.triggers.iter().copied());
                                    json!({ "inputs": inputs })
                                },
                            after: Vec::new(),
                            background: false,
                        })?;
                        let _ = &task;
                        Ok(Completion::completed(json!({ "successor": successor })))
                    }.boxed()
                },
            )))
        }.boxed()
    }

    fn phases(&self) -> Vec<String> {
        Vec::new()
    }

    fn phase(
        &self,
        phase: &str,
        _task: Arc<Task>,
        _rt: Arc<Runtime>,
        _ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<Step>> {
        let phase = phase.to_owned();
        async move {
            Err(task_contract_fault(
                "pi.post_tools",
                &format!("no handler for phase {phase}"),
            ))
        }
        .boxed()
    }

    fn abort(
        &self,
        task: Arc<Task>,
        _rt: Arc<Runtime>,
        _ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<AbortClosure>> {
        async move {
            let input_inputs: Vec<Id> = task
                .input
                .get("inputs")
                .and_then(Value::as_array)
                .map(|array| array.iter().filter_map(Value::as_i64).collect())
                .unwrap_or_default();
            Ok(
                crate::agent_core::harness::pico3::runtime::abort_closure_from(
                    move |tx: &mut Tx, current: Task, _ctx: Context| {
                        let input_inputs = input_inputs.clone();
                        async move {
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
                            Ok(Value::Null)
                        }
                        .boxed()
                    },
                ),
            )
        }
        .boxed()
    }
}

fn object_of(value: Value) -> serde_json::Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}
