//! Port of `packages/agent/src/harness/pico3/kinds/tool.ts` (479 lines):
//! the `pi.tool` kind — offered-set check, lookup, schema validation,
//! beforeTool hooks (the one point where a throwing handler blocks), the
//! durable `started` checkpoint, kernel-owned streaming with bounded
//! output, afterTool chaining, and the result-closing entry.
#![allow(clippy::type_complexity)]

use std::sync::Arc;

use futures::future::BoxFuture;
use futures::FutureExt;
use serde_json::{json, Value};

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::pico3::session::Tx;
use crate::agent_core::harness::pico3::types::{
    task_contract_fault, AnyKind, BasicKind, Completion, Id, NewEntry, Task, ViewEvent,
};

use crate::agent_core::harness::pico3::runtime::{
    invalid_arguments, AbortClosure, BeforeToolApi, HookApi, HookHandlers, OutputBounds, Runtime,
    Step, StreamChunk, ToolApi, ToolDeclaration, ToolResult,
};

// ---------------------------------------------------------------------------
// Hooks (`ToolHooks`, tool.ts:40-54)
// ---------------------------------------------------------------------------

/// Upstream `ToolHooks` (`tool.ts:40-54`).
#[derive(Default)]
pub struct ToolHandlers {
    /// `beforeTool(call, api, ctx)`: chain on `call`; `{ block }` stops it;
    /// a throw blocks.
    pub before_tool: Option<BeforeToolFn>,
    /// `afterTool(call, result, api, ctx)`: chain; may replace the result.
    pub after_tool: Option<AfterToolFn>,
}

impl HookHandlers for ToolHandlers {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// The beforeTool outcome: a possibly-rewritten call or a block reason.
pub enum BeforeToolOutcome {
    Call(Value),
    Block(String),
}

pub type BeforeToolFn = Arc<
    dyn Fn(
            &Value,
            &BeforeToolApi,
            Context,
        ) -> BoxFuture<'static, anyhow::Result<Option<BeforeToolOutcome>>>
        + Send
        + Sync,
>;

/// `afterTool(call, result, api, ctx)`: receives the result built so far;
/// a returned value replaces it (upstream mutates the result object — the
/// chain result is the same).
pub type AfterToolFn = Arc<
    dyn Fn(
            &Value,
            ToolResult,
            &HookApi,
            Context,
        ) -> BoxFuture<'static, anyhow::Result<Option<ToolResult>>>
        + Send
        + Sync,
>;

/// The registered kind instance (`tool.ts:73-183`).
pub struct ToolKind;

impl ToolKind {
    pub fn new() -> Arc<Self> {
        Arc::new(ToolKind)
    }
}

impl crate::agent_core::harness::pico3::runtime::Kind for ToolKind {
    fn metadata(&self) -> Arc<dyn AnyKind> {
        Arc::new(
            BasicKind::new("pi.tool")
                .turn(true)
                .inflight(vec!["started".to_owned()]),
        )
    }

    fn initial(
        &self,
        task: Arc<Task>,
        rt: Arc<Runtime>,
        ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<Step>> {
        async move {
            let call = task.input.get("call").cloned().unwrap_or(Value::Null);
            let name = call.get("name").and_then(Value::as_str).unwrap_or_default();
            let offered: Vec<String> = task
                .input
                .get("offered")
                .and_then(Value::as_array)
                .map(|array| {
                    array
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            if !offered.iter().any(|offered| offered == name) {
                return Ok(done_close(
                    &task,
                    synthetic(&format!("tool {name} was not offered"), "not_offered"),
                    None,
                    rt.now(),
                    None,
                    None,
                ));
            }
            let Some(declaration) = rt.tool(name) else {
                return Ok(done_close(
                    &task,
                    synthetic(&format!("tool {name} is not registered"), "missing_tool"),
                    None,
                    rt.now(),
                    None,
                    None,
                ));
            };
            if let Some(bad) = invalid_arguments(&declaration, &call) {
                return Ok(done_close(
                    &task,
                    synthetic(&format!("invalid arguments: {bad}"), "invalid_arguments"),
                    Some(&declaration),
                    rt.now(),
                    None,
                    None,
                ));
            }
            // beforeTool: the one point where a throwing handler blocks
            // (`tool.ts:96-100`, `186-245`).
            let hooked = match before_tool(&task, &rt, &call, &ctx).await? {
                BeforeToolLoop::Call(hooked) => hooked,
                BeforeToolLoop::Block(reason) => {
                    return Ok(done_close(
                        &task,
                        synthetic(&format!("blocked: {reason}"), "blocked"),
                        Some(&declaration),
                        rt.now(),
                        None,
                        None,
                    ));
                }
            };
            if !same_identity(&call, &hooked) {
                return Ok(done_close(
                    &task,
                    synthetic("blocked: call identity changed", "blocked"),
                    Some(&declaration),
                    rt.now(),
                    None,
                    None,
                ));
            }
            if let Some(bad) = invalid_arguments(&declaration, &hooked) {
                return Ok(done_close(
                    &task,
                    synthetic(
                        &format!("invalid arguments after hook: {bad}"),
                        "invalid_arguments",
                    ),
                    Some(&declaration),
                    rt.now(),
                    None,
                    None,
                ));
            }
            let final_call = hooked;
            // Durable before the effect (`tool.ts:112-118`).
            {
                let replay = declaration
                    .replay
                    .clone()
                    .unwrap_or_else(|| "unsafe".to_owned());
                let final_call = final_call.clone();
                let _task_id = task.id;
                rt.commit_erased_with(
                    ctx.clone(),
                    Box::new(move |tx: &mut Tx, current: Task, _ctx: Context| {
                        let final_call = final_call.clone();
                        async move {
                            tx.checkpoint(json!({
                                "phase": "started",
                                "replay": replay,
                                "call": final_call,
                            }))?;
                            update_tool_slot_for(tx, &current, |slot| {
                                slot["status"] = Value::String("running".to_owned());
                                if let Some(object) = slot.as_object_mut() {
                                    object.shift_remove("waitingOn");
                                }
                            })?;
                            tx.emit_event(ViewEvent::ToolStarted {
                                task_id: current.id,
                                call_id: final_call
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned(),
                                name: final_call
                                    .get("name")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned(),
                            })?;
                            Ok(Value::Null)
                        }
                        .boxed()
                    }),
                )
                .await?;
            }
            let done = invoke(&task, final_call, &declaration, &rt, ctx).await?;
            Ok(Step::Done(done))
        }
        .boxed()
    }

    fn phases(&self) -> Vec<String> {
        vec!["started".to_owned()]
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
                "started" => {
                    // Only entered by the scheduler after reopen: the stored
                    // final call is the evidence; beforeTool never reruns
                    // (`tool.ts:122-158`).
                    let cp = task
                        .checkpoint
                        .as_ref()
                        .map(|checkpoint| Value::Object(checkpoint.clone()))
                        .unwrap_or(Value::Null);
                    let call = cp.get("call").cloned().unwrap_or(Value::Null);
                    let replay = cp.get("replay").and_then(Value::as_str).unwrap_or("unsafe");
                    let name = call.get("name").and_then(Value::as_str).unwrap_or_default();
                    let Some(declaration) = rt.tool(name) else {
                        return Ok(done_close(
                            &task,
                            synthetic(
                                &format!("tool {name} unavailable after restart"),
                                "unavailable",
                            ),
                            None,
                            rt.now(),
                            Some(&call),
                            None,
                        ));
                    };
                    let declaration_replay = declaration
                        .replay
                        .clone()
                        .unwrap_or_else(|| "unsafe".to_owned());
                    if replay != "safe" || declaration_replay != "safe" {
                        return Ok(done_close(
                            &task,
                            synthetic(&format!("tool {name} was interrupted"), "interrupted"),
                            Some(&declaration),
                            rt.now(),
                            Some(&call),
                            None,
                        ));
                    }
                    if invalid_arguments(&declaration, &call).is_some() {
                        return Ok(done_close(
                            &task,
                            synthetic(
                                &format!(
                                    "tool {name} was interrupted; arguments no longer validate"
                                ),
                                "interrupted",
                            ),
                            Some(&declaration),
                            rt.now(),
                            Some(&call),
                            None,
                        ));
                    }
                    let done = invoke(&task, call, &declaration, &rt, ctx).await?;
                    Ok(Step::Done(done))
                }
                other => Err(task_contract_fault(
                    "pi.tool",
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
        async move {
            // Abort owned children first (`tool.ts:161-165`).
            for conversation_id in &task.owns {
                let children = rt
                    .commit_typed(
                        ctx.clone(),
                        {
                            let conversation_id = *conversation_id;
                            move |tx: &mut Tx, _current: Task, _ctx: Context| {
                                async move {
                                    tx.tasks(&crate::agent_core::harness::pico3::types::TaskScan {
                                        conversation_id: Some(conversation_id),
                                        status: Some(vec![
                                            crate::agent_core::harness::pico3::types::TaskStatus::Pending,
                                            crate::agent_core::harness::pico3::types::TaskStatus::Running,
                                        ]),
                                        kind: None,
                                    })
                                    .await
                                }.boxed()
                            }
                        },
                    )
                    .await?;
                for child in children {
                    if child.background != Some(true) {
                        let _ = rt.abort_task(child.id, ctx.clone()).await;
                    }
                }
            }
            let input_call = task.input.get("call").cloned().unwrap_or(Value::Null);
            let checkpoint_call = task
                .checkpoint
                .as_ref()
                .and_then(|checkpoint| checkpoint.get("call"))
                .cloned();
            let index = task
                .input
                .get("index")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            Ok(crate::agent_core::harness::pico3::runtime::abort_closure_from(
                move |tx: &mut Tx, current: Task, _ctx: Context| {
                    let input_call = input_call.clone();
                    let checkpoint_call = checkpoint_call.clone();
                    async move {
                        // `task.checkpoint?.call ?? task.input.call`
                        // (`tool.ts:167`).
                        let call = checkpoint_call.unwrap_or(input_call);
                        let entry_id = tx.append_entry(
                            current.conversation_id,
                            NewEntry {
                                kind: "pi.tool_result".to_owned(),
                                model: Some(vec![to_message(
                                    &call,
                                    &synthetic("tool aborted", "aborted"),
                                    tx.session().now(),
                                )]),
                                data: Some(object_of(json!({
                                    "diagnostics": [
                                        { "severity": "error", "message": "aborted", "code": "aborted" },
                                    ],
                                }))),
                                ..Default::default()
                            },
                        )?;
                        // The turn-tools slot (`tool.ts:173-178`).
                        let slot_updated = update_tool_slot_indexed(
                            tx,
                            current.conversation_id,
                            index as usize,
                            |slot| {
                                slot["status"] = Value::String("aborted".to_owned());
                                slot["entry"] = json!(entry_id);
                                if let Some(object) = slot.as_object_mut() {
                                    object.shift_remove("waitingOn");
                                }
                            },
                        );
                        if slot_updated.is_ok() {
                            tx.emit_event(ViewEvent::ToolAborted {
                                task_id: current.id,
                                call_id: call.get("id").and_then(Value::as_str).unwrap_or_default().to_owned(),
                                entry: entry_id,
                            })?;
                            return Ok(json!({ "entry": entry_id }));
                        }
                        Ok(json!({ "entry": entry_id }))
                    }.boxed()
                },
            ))
        }.boxed()
    }
}

/// The beforeTool loop outcome (`tool.ts:186-245`).
enum BeforeToolLoop {
    Call(Value),
    Block(String),
}

async fn before_tool(
    task: &Task,
    rt: &Arc<Runtime>,
    call: &Value,
    ctx: &Context,
) -> anyhow::Result<BeforeToolLoop> {
    let mut current = call.clone();
    let handlers: Vec<(
        ToolHandlers,
        HookApi,
        crate::agent_core::harness::pico3::types::Namespace,
    )> = rt
        .hooks
        .bindings()
        .iter()
        .filter_map(|binding| {
            binding
                .handlers
                .downcast_ref::<ToolHandlers>()
                .map(|handlers| {
                    (
                        clone_handlers(handlers),
                        binding.api.clone(),
                        binding.namespace.clone(),
                    )
                })
        })
        .collect();
    for (handlers, api, namespace) in &handlers {
        let Some(handler) = handlers.before_tool.clone() else {
            continue;
        };
        // The BeforeToolApi memo/waiting/emit surface (`tool.ts:194-233`).
        let hook_api = before_tool_api(rt, task, api, namespace);
        let outcome = handler(&current, &hook_api, ctx.clone()).await;
        match outcome {
            Ok(Some(BeforeToolOutcome::Block(block))) => return Ok(BeforeToolLoop::Block(block)),
            Ok(Some(BeforeToolOutcome::Call(rewritten))) => current = rewritten,
            Ok(None) => {}
            Err(error) => {
                // A throw blocks (`tool.ts:239-241`), unless aborted.
                let aborted = ctx
                    .abort_signal()
                    .is_some_and(|signal| signal.is_cancelled());
                if aborted {
                    return Err(error);
                }
                return Ok(BeforeToolLoop::Block(format!("hook threw: {error}")));
            }
        }
    }
    Ok(BeforeToolLoop::Call(current))
}

fn clone_handlers(handlers: &ToolHandlers) -> ToolHandlers {
    ToolHandlers {
        before_tool: handlers.before_tool.clone(),
        after_tool: handlers.after_tool.clone(),
    }
}

/// Only namespace-bound approval capabilities are exposed to beforeTool.
fn before_tool_api(
    rt: &Arc<Runtime>,
    task: &Task,
    api: &HookApi,
    namespace: &crate::agent_core::harness::pico3::types::Namespace,
) -> BeforeToolApi {
    let call_id = task
        .input
        .get("call")
        .and_then(|call| call.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let conversation_id = task.conversation_id;
    let index = task.input.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
    let waiting_hook = {
        let rt = rt.clone();
        let namespace = namespace.clone();
        let call_id = call_id.clone();
        Arc::new(
            move |ctx: Context| -> BoxFuture<'static, anyhow::Result<()>> {
                let rt = rt.clone();
                let namespace = namespace.clone();
                let call_id = call_id.clone();
                Box::pin(async move {
                    rt.commit_typed(ctx, move |tx: &mut Tx, current: Task, _ctx: Context| {
                        Box::pin(async move {
                            tx.plugins(&namespace)?;
                            let mut changed = false;
                            update_tool_slot_indexed(tx, conversation_id, index, |slot| {
                                if slot.get("waitingOn").and_then(Value::as_str)
                                    != Some(namespace.id.as_str())
                                {
                                    slot["waitingOn"] = json!(namespace.id);
                                    changed = true;
                                }
                            })?;
                            if changed {
                                tx.emit_event(ViewEvent::ToolWaiting {
                                    task_id: current.id,
                                    call_id,
                                    on: namespace.id,
                                })?;
                            }
                            Ok(())
                        })
                    })
                    .await
                })
            },
        )
    };
    let memo_hook = {
        let rt = rt.clone();
        let namespace = namespace.clone();
        Arc::new(
            move |name: &str,
                  candidate: Option<Value>,
                  ctx: Context|
                  -> BoxFuture<'static, anyhow::Result<Option<Value>>> {
                let rt = rt.clone();
                let namespace = namespace.clone();
                let key = format!("hook:{}:{name}", namespace.id);
                Box::pin(async move {
                    rt.commit_typed(ctx, move |tx: &mut Tx, _current: Task, _ctx: Context| {
                        Box::pin(async move {
                            tx.plugins(&namespace)?;
                            let mut result = None;
                            update_tool_slot_indexed(tx, conversation_id, index, |slot| {
                                result = slot_memo(slot, &key, candidate);
                            })?;
                            Ok(result)
                        })
                    })
                    .await
                })
            },
        )
    };
    let emit_hook = {
        let rt = rt.clone();
        let namespace = namespace.clone();
        Arc::new(
            move |name: &str,
                  data: Value,
                  ctx: Context|
                  -> BoxFuture<'static, anyhow::Result<()>> {
                let rt = rt.clone();
                let namespace = namespace.clone();
                let name = name.to_owned();
                Box::pin(async move {
                    rt.commit_typed(ctx, move |tx: &mut Tx, _current: Task, _ctx: Context| {
                        Box::pin(async move { tx.emit_plugin(&namespace, &name, data) })
                    })
                    .await
                })
            },
        )
    };
    BeforeToolApi::new(api.clone(), call_id, waiting_hook, memo_hook, emit_hook)
}

/// First-writer-wins with a distinct absence value, including stored JSON null.
fn slot_memo(slot: &mut Value, key: &str, candidate: Option<Value>) -> Option<Value> {
    let memos = slot
        .as_object_mut()
        .expect("tool slot object")
        .entry("memos")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .expect("tool slot memos object");
    if let Some(winner) = memos.get(key) {
        return Some(winner.clone());
    }
    if let Some(candidate) = candidate {
        memos.insert(key.to_owned(), candidate.clone());
        Some(candidate)
    } else {
        None
    }
}

/// Upstream `invoke` (`tool.ts:249-367`): kernel-owned stream, bounded
/// buffer, throttled flushes, afterTool chaining, and the closing entry.
async fn invoke(
    task: &Task,
    call: Value,
    declaration: &ToolDeclaration,
    rt: &Arc<Runtime>,
    ctx: Context,
) -> anyhow::Result<
    Box<
        dyn for<'tx> FnOnce(
                &'tx mut Tx,
                Task,
                Context,
            ) -> BoxFuture<'tx, anyhow::Result<Completion>>
            + Send,
    >,
> {
    let bounds = OutputBounds::with_overrides(declaration.output);
    let stream_state = Arc::new(std::sync::Mutex::new(ToolStreamState {
        buffer: crate::agent_core::harness::pico3::bounded::Bounded::new(
            bounds.max_bytes,
            bounds.max_lines,
            bounds.retain,
        ),
        streamed: false,
        last_flush: 0,
        pending: None,
    }));
    let stream_sink: Arc<dyn Fn(StreamChunk) + Send + Sync> = {
        let rt = rt.clone();
        let ctx = ctx.clone();
        let state = stream_state.clone();
        let conversation = task.conversation_id;
        let index = task.input.get("index").and_then(Value::as_i64).unwrap_or(0) as usize;
        Arc::new(move |chunk| {
            let mut state = state.lock().expect("tool stream");
            state.streamed = true;
            let bytes = match chunk {
                StreamChunk::Text(text) => text.into_bytes(),
                StreamChunk::Bytes(bytes) => bytes,
            };
            state.buffer.push(&bytes);
            if rt.now() - state.last_flush >= 100 {
                state.flush(rt.clone(), ctx.clone(), conversation, index);
            }
        })
    };

    // progress + memo over the tool slot (`tool.ts:279-320`).
    let progress_rt = rt.clone();
    let slot_conversation = task.conversation_id;
    let slot_index = task.input.get("index").and_then(Value::as_i64).unwrap_or(0) as usize;
    let progress_hook: Arc<
        dyn Fn(
                Box<dyn FnOnce(&mut Value) + Send>,
                Context,
            ) -> BoxFuture<'static, anyhow::Result<()>>
            + Send
            + Sync,
    > = Arc::new(move |update, ctx| {
        let rt = progress_rt.clone();
        let slot_conversation = slot_conversation;
        let slot_index = slot_index;
        Box::pin(async move {
            rt.commit_erased_with(
                ctx,
                Box::new(move |tx: &mut Tx, _current: Task, _ctx: Context| {
                    async move {
                        // Only the free fields; identity is protected
                        // (`tool.ts:279-294`).
                        update_tool_slot_indexed(tx, slot_conversation, slot_index, |slot| {
                            let free = json!({
                                "progress": slot.get("progress").cloned().unwrap_or(Value::Null),
                                "details": slot.get("details").cloned().unwrap_or(Value::Null),
                                "continuedBy": slot.get("continuedBy").cloned().unwrap_or(Value::Null),
                            });
                            let mut free = free;
                            update(&mut free);
                            let object = free.as_object().cloned().unwrap_or_default();
                            let slot_object = slot.as_object_mut().expect("slot object");
                            match object.get("progress") {
                                Some(value) if !value.is_null() => {
                                    slot_object.insert("progress".to_owned(), value.clone());
                                }
                                _ => {
                                    slot_object.shift_remove("progress");
                                }
                            }
                            match object.get("details") {
                                Some(value) if !value.is_null() => {
                                    slot_object.insert("details".to_owned(), value.clone());
                                }
                                _ => {
                                    slot_object.shift_remove("details");
                                }
                            }
                            match object.get("continuedBy") {
                                Some(value) if !value.is_null() => {
                                    slot_object.insert("continuedBy".to_owned(), value.clone());
                                }
                                _ => {
                                    slot_object.shift_remove("continuedBy");
                                }
                            }
                        })?;
                        Ok(Value::Null)
                    }.boxed()
                }),
            )
            .await
            .map(|_| ())
        })
    });
    let memo_rt = rt.clone();
    let memo_hook: Arc<
        dyn Fn(&str, Option<Value>, Context) -> BoxFuture<'static, anyhow::Result<Option<Value>>>
            + Send
            + Sync,
    > = Arc::new(move |name, candidate, ctx| {
        let rt = memo_rt.clone();
        let key = format!("tool:{name}");
        Box::pin(async move {
            rt.commit_typed(ctx, move |tx: &mut Tx, _current: Task, _ctx: Context| {
                Box::pin(async move {
                    let mut result = None;
                    update_tool_slot_indexed(tx, slot_conversation, slot_index, |slot| {
                        result = slot_memo(slot, &key, candidate);
                    })?;
                    Ok(result)
                })
            })
            .await
        })
    });

    let tool_api = ToolApi::invocation(
        rt.clone(),
        task.id,
        task.conversation_id,
        call.get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        stream_sink,
        progress_hook,
        memo_hook,
    );

    // Execute (`tool.ts:322-350`).
    let arguments = call.get("arguments").cloned().unwrap_or(Value::Null);
    let result: ToolResult = match (declaration.execute)(arguments, tool_api, ctx.clone()).await {
        Ok(result) => result,
        Err(error) => {
            let aborted = ctx
                .abort_signal()
                .is_some_and(|signal| signal.is_cancelled());
            if aborted {
                return Err(error);
            }
            synthetic(&format!("tool threw: {error}"), "threw")
        }
    };
    // Every stream, including a short one below the throttle interval, has a
    // final ordered persistence barrier before afterTool and terminalization.
    let (pending, streamed, text, dropped_bytes, dropped_lines) = {
        let mut state = stream_state.lock().expect("tool stream");
        if state.streamed {
            let index = task.input.get("index").and_then(Value::as_i64).unwrap_or(0) as usize;
            state.flush(rt.clone(), ctx.clone(), task.conversation_id, index);
        }
        (
            state.pending.take(),
            state.streamed,
            state.buffer.text(),
            state.buffer.dropped_bytes,
            state.buffer.dropped_lines,
        )
    };
    if let Some(pending) = pending {
        pending.await??;
    }
    let mut result = result;
    if streamed && result.content.is_none() {
        let content = vec![json!({ "type": "text", "text": text })];
        let mut diagnostics = result.diagnostics.clone().unwrap_or_default();
        if dropped_bytes > 0 || dropped_lines > 0 {
            diagnostics.push(json!({
                    "severity": "warn",
                    "code": "truncated",
                    "message": format!(
                        "{} bytes / {} lines dropped ({} {} retained)",
                        dropped_bytes, dropped_lines,
                        if bounds.retain == crate::agent_core::harness::pico3::bounded::Retain::Head { "head" } else { "tail" },
                        bounds.max_bytes,
                    ),
                }));
        }
        result.diagnostics = if diagnostics.is_empty() {
            None
        } else {
            Some(diagnostics)
        };
        result.content = Some(content);
    }

    // afterTool hooks: chain; last replacement wins (`tool.ts:351-358`).
    let handlers: Vec<(AfterToolFn, HookApi)> = rt
        .hooks
        .bindings()
        .iter()
        .filter_map(|binding| {
            binding
                .handlers
                .downcast_ref::<ToolHandlers>()
                .and_then(|handlers| handlers.after_tool.clone())
                .map(|handler| (handler, binding.api.clone()))
        })
        .collect();
    let call_for_hooks = call.clone();
    for (handler, api) in &handlers {
        let call_value = call_for_hooks.clone();
        match handler(&call_value, result.clone(), api, ctx.clone()).await {
            Ok(Some(replacement)) => result = replacement,
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

    let declaration_bounds = declaration.output;
    let stream_truncated = streamed.then_some((dropped_bytes, dropped_lines));
    let call_for_close = call.clone();
    let result_for_close = result;
    let task = task.clone();
    Ok(Box::new(
        move |tx: &mut Tx, current: Task, _ctx: Context| {
            let result = result_for_close.clone();
            let call = call_for_close.clone();
            async move {
                close(
                    tx,
                    &task,
                    current,
                    result,
                    declaration_bounds,
                    &call,
                    stream_truncated,
                )
            }
            .boxed()
        },
    ))
}

/// Ordered flush chain equivalent to upstream's flushing Promise. Holding
/// this state lock across enqueue preserves snapshot order even when a Rust
/// tool streams concurrently from stdout and stderr. No lock spans an await.
struct ToolStreamState {
    buffer: crate::agent_core::harness::pico3::bounded::Bounded,
    streamed: bool,
    last_flush: i64,
    pending: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
}

impl ToolStreamState {
    fn flush(&mut self, rt: Arc<Runtime>, ctx: Context, conversation: Id, index: usize) {
        self.last_flush = rt.now();
        let text = self.buffer.text();
        let previous = self.pending.take();
        self.pending = Some(tokio::spawn(async move {
            let first_error = match previous {
                Some(previous) => match previous.await {
                    Ok(result) => result.err(),
                    Err(error) => Some(error.into()),
                },
                None => None,
            };
            let result = rt
                .commit(
                    move |tx, _, _| {
                        async move {
                            update_tool_slot_indexed(tx, conversation, index, |slot| {
                                slot["output"] = Value::String(text);
                            })
                        }
                        .boxed()
                    },
                    ctx,
                )
                .await;
            // As with the upstream catch chain, later snapshots still flush,
            // but no later success can erase the first persistence error.
            match first_error {
                Some(error) => Err(error),
                None => result,
            }
        }));
    }
}

/// Upstream `close` (`tool.ts:369-418`): bound the output, store the exact
/// model message under the stored final call, rest as strict-JSON data.
#[allow(clippy::too_many_arguments)]
fn close(
    tx: &mut Tx,
    task: &Task,
    current: Task,
    raw: ToolResult,
    declared: Option<OutputBounds>,
    call: &Value,
    stream_truncated: Option<(usize, usize)>,
) -> anyhow::Result<Completion> {
    let (result, truncated) = bound(raw, declared);
    let bytes = truncated.as_ref().map(|t| t.0).unwrap_or(0)
        + stream_truncated.map(|(bytes, _)| bytes).unwrap_or(0);
    let lines = truncated.as_ref().map(|t| t.1).unwrap_or(0)
        + stream_truncated.map(|(_, lines)| lines).unwrap_or(0);
    let truncated = if bytes > 0 || lines > 0 {
        Some((bytes, lines))
    } else {
        None
    };
    let mut data = serde_json::Map::new();
    if let Some(details) = &result.details {
        data.insert("details".to_owned(), details.clone());
    }
    if let Some(diagnostics) = &result.diagnostics {
        data.insert("diagnostics".to_owned(), Value::Array(diagnostics.clone()));
    }
    if let Some(control) = &result.control {
        data.insert("control".to_owned(), control.clone());
    }
    if let Some((bytes, lines)) = truncated {
        data.insert(
            "truncated".to_owned(),
            json!({ "bytes": bytes, "lines": lines }),
        );
    }
    let entry = tx.append_entry(
        current.conversation_id,
        NewEntry {
            kind: "pi.tool_result".to_owned(),
            model: Some(vec![to_message(call, &result, tx.session().now())]),
            data: Some(data),
            ..Default::default()
        },
    )?;
    update_tool_slot_for(tx, task, |slot| {
        slot["status"] = Value::String(
            if result.is_error == Some(true) {
                "error"
            } else {
                "done"
            }
            .to_owned(),
        );
        slot["entry"] = json!(entry);
        if let Some(object) = slot.as_object_mut() {
            object.shift_remove("waitingOn");
        }
    })?;
    tx.emit_event(ViewEvent::ToolFinished {
        task_id: current.id,
        call_id: call
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        entry,
        is_error: result.is_error == Some(true),
        control: result.control.clone(),
    })?;
    if let Some((bytes, lines)) = truncated {
        tx.emit_event(ViewEvent::Warning {
            source: "tool".to_owned(),
            message: format!("tool output truncated: {bytes} bytes / {lines} lines dropped"),
        })?;
    }
    Ok(Completion {
        status: "completed".to_owned(),
        result: Some(json!({
            "entry": entry,
            "control": result.control.clone(),
        })),
        failure: None,
    })
}

/// Upstream `toMessage` (`tool.ts:420-429`).
fn to_message(call: &Value, result: &ToolResult, now: i64) -> Value {
    json!({
        "role": "toolResult",
        "toolCallId": call.get("id"),
        "toolName": call.get("name"),
        "content": result.content.clone().unwrap_or_default(),
        "isError": result.is_error.unwrap_or(false),
        "timestamp": now,
    })
}

/// Upstream `bound` (`tool.ts:431-477`): bound the aggregate text of the
/// result's content blocks, rewriting the last head-mode text block (the
/// only one) or the tail text block.
fn bound(
    result: ToolResult,
    declared: Option<OutputBounds>,
) -> (ToolResult, Option<(usize, usize)>) {
    let bounds = OutputBounds::with_overrides(declared);
    let blocks = result.content.clone().unwrap_or_default();
    let text: String = blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    let mut dropped_lines: usize = 0;
    let mut dropped_bytes: usize = 0;
    let lines: Vec<&str> = text.split('\n').collect::<Vec<_>>();
    let mut bounded_text = if lines.len() > bounds.max_lines {
        dropped_lines = lines.len() - bounds.max_lines;
        if bounds.retain == crate::agent_core::harness::pico3::bounded::Retain::Head {
            lines[..bounds.max_lines].join("\n")
        } else {
            lines[lines.len() - bounds.max_lines..].join("\n")
        }
    } else {
        text.clone()
    };
    let bytes = bounded_text.as_bytes().to_vec();
    if bytes.len() > bounds.max_bytes {
        dropped_bytes = bytes.len() - bounds.max_bytes;
        let sliced: Vec<u8> =
            if bounds.retain == crate::agent_core::harness::pico3::bounded::Retain::Head {
                bytes[..bounds.max_bytes].to_vec()
            } else {
                bytes[bytes.len() - bounds.max_bytes..].to_vec()
            };
        bounded_text = String::from_utf8_lossy(&sliced).into_owned();
    }
    let text = bounded_text;
    if dropped_bytes == 0 && dropped_lines == 0 {
        return (result, None);
    }
    // Rewrite the text blocks: head mode keeps the first text block; tail
    // mode keeps the last (`tool.ts:456-466`).
    let is_text = |block: &Value| block.get("type").and_then(Value::as_str) == Some("text");
    let text_index = match bounds.retain {
        crate::agent_core::harness::pico3::bounded::Retain::Head => blocks.iter().position(is_text),
        crate::agent_core::harness::pico3::bounded::Retain::Tail => {
            blocks.iter().rposition(is_text)
        }
    };
    let mut content: Vec<Value> = Vec::new();
    for (index, mut block) in blocks.into_iter().enumerate() {
        if block.get("type").and_then(Value::as_str) != Some("text") {
            content.push(block);
        } else if Some(index) == text_index {
            block["text"] = Value::String(text.clone());
            content.push(block);
        }
    }
    let mut diagnostics = result.diagnostics.clone().unwrap_or_default();
    diagnostics.push(json!({
        "severity": "warn",
        "code": "truncated",
        "message": format!("output truncated: {dropped_lines} lines, {dropped_bytes} bytes dropped"),
    }));
    (
        ToolResult {
            content: Some(content),
            diagnostics: Some(diagnostics),
            ..result
        },
        Some((dropped_bytes, dropped_lines)),
    )
}

/// Upstream `synthetic` (`tool.ts:60-64`).
fn synthetic(text: &str, code: &str) -> ToolResult {
    ToolResult {
        content: Some(vec![json!({ "type": "text", "text": text })]),
        is_error: Some(true),
        details: None,
        diagnostics: Some(vec![json!({
            "severity": "error",
            "message": text,
            "code": code,
        })]),
        control: None,
    }
}

/// Upstream `sameIdentity` (`tool.ts:70-71`).
fn same_identity(a: &Value, b: &Value) -> bool {
    a.get("id") == b.get("id")
        && a.get("name") == b.get("name")
        && a.get("namespace") == b.get("namespace")
}

/// A `done` step closing with a synthetic result.
#[allow(clippy::too_many_arguments)]
fn done_close(
    task: &Task,
    raw: ToolResult,
    declaration: Option<&ToolDeclaration>,
    _now: i64,
    call: Option<&Value>,
    _declaration_unused: Option<()>,
) -> Step {
    let declared = declaration.map(|declaration| declaration.output);
    let call = call
        .cloned()
        .unwrap_or_else(|| task.input.get("call").cloned().unwrap_or(Value::Null));
    let index = task.input.get("index").and_then(Value::as_i64).unwrap_or(0);
    Step::Done(Box::new(
        move |tx: &mut Tx, current: Task, _ctx: Context| {
            let raw = raw.clone();
            let call = call.clone();
            async move {
                let (result, truncated) = bound(raw, declared.unwrap_or(None));
                let mut data = serde_json::Map::new();
                if let Some(details) = &result.details {
                    data.insert("details".to_owned(), details.clone());
                }
                if let Some(diagnostics) = &result.diagnostics {
                    data.insert("diagnostics".to_owned(), Value::Array(diagnostics.clone()));
                }
                if let Some(control) = &result.control {
                    data.insert("control".to_owned(), control.clone());
                }
                if let Some((bytes, lines)) = truncated {
                    data.insert(
                        "truncated".to_owned(),
                        json!({ "bytes": bytes, "lines": lines }),
                    );
                }
                let entry = tx.append_entry(
                    current.conversation_id,
                    NewEntry {
                        kind: "pi.tool_result".to_owned(),
                        model: Some(vec![to_message(&call, &result, tx.session().now())]),
                        data: Some(data),
                        ..Default::default()
                    },
                )?;
                let _ =
                    update_tool_slot_indexed(tx, current.conversation_id, index as usize, |slot| {
                        slot["status"] = Value::String(
                            if result.is_error == Some(true) {
                                "error"
                            } else {
                                "done"
                            }
                            .to_owned(),
                        );
                        slot["entry"] = json!(entry);
                        if let Some(object) = slot.as_object_mut() {
                            object.shift_remove("waitingOn");
                        }
                    });
                tx.emit_event(ViewEvent::ToolFinished {
                    task_id: current.id,
                    call_id: call
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    entry,
                    is_error: result.is_error == Some(true),
                    control: result.control.clone(),
                })?;
                Ok(Completion {
                    status: "completed".to_owned(),
                    result: Some(json!({
                        "entry": entry,
                        "control": result.control.clone(),
                    })),
                    failure: None,
                })
            }
            .boxed()
        },
    ))
}

// ---------------------------------------------------------------------------
// Slot helpers (upstream `tx.toolSlot(task)` mutations)
// ---------------------------------------------------------------------------

/// Apply `f` to the task's turn-tools slot by the task's input index. The
/// slot object is the whole live value; identity fields ride along.
pub(crate) fn update_tool_slot_for(
    tx: &mut Tx,
    task: &Task,
    f: impl FnOnce(&mut Value),
) -> anyhow::Result<()> {
    let index = task.input.get("index").and_then(Value::as_i64).unwrap_or(0) as usize;
    update_tool_slot_indexed(tx, task.conversation_id, index, f)
}

/// Apply `f` to the turn-tools slot at `index` (upstream
/// `tx.toolSlot(task)` write paths).
pub(crate) fn update_tool_slot_indexed(
    tx: &mut Tx,
    conversation_id: Id,
    index: usize,
    f: impl FnOnce(&mut Value),
) -> anyhow::Result<()> {
    let sticky =
        tx.snapshot(crate::agent_core::harness::pico3::types::DocRef::Sticky { conversation_id })?;
    let mut turn = sticky
        .get("turn")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut tools = turn
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let Some(slot) = tools.get_mut(index) else {
        anyhow::bail!("no tool slot at index {index}");
    };
    f(slot);
    turn.insert("tools".to_owned(), Value::Array(tools));
    tx.sticky_set(conversation_id, "turn", Value::Object(turn))
}

fn object_of(value: Value) -> serde_json::Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

/// The upstream `Step` re-export (`tool.ts:479`).
pub type ToolStep = Step;
