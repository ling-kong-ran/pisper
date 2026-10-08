//! Port of `packages/agent/src/harness/pico3/kinds/plugin.ts` (43 lines):
//! the `pi.plugin` kind — run a registered handler with the task api; the
//! durable `started` checkpoint precedes the call.

use std::sync::Arc;

use futures::future::BoxFuture;
use futures::FutureExt;
use serde_json::Value;

use crate::agent_core::chord_support::Context;
use crate::agent_core::harness::pico3::session::Tx;
use crate::agent_core::harness::pico3::types::{
    task_contract_fault, AnyKind, BasicKind, Completion, Task,
};

use crate::agent_core::harness::pico3::runtime::{AbortClosure, Runtime, Step, ToolApi};

/// Upstream `PluginInput` (`plugin.ts:5`): `{ handler, input }`.
fn handler_name(task: &Task) -> String {
    task.input
        .get("handler")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// The registered kind instance (`plugin.ts:9-25`).
pub struct PluginKind;

impl PluginKind {
    pub fn new() -> Arc<Self> {
        Arc::new(PluginKind)
    }
}

impl crate::agent_core::harness::pico3::runtime::Kind for PluginKind {
    fn metadata(&self) -> Arc<dyn AnyKind> {
        Arc::new(BasicKind::new("pi.plugin").inflight(vec!["started".to_owned()]))
    }

    fn initial(
        &self,
        task: Arc<Task>,
        rt: Arc<Runtime>,
        ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<Step>> {
        async move {
            rt.commit_erased_with(
                ctx.clone(),
                Box::new(|tx: &mut Tx, _current: Task, _ctx: Context| {
                    async move {
                        tx.checkpoint(json_checkpoint_started())?;
                        Ok(Value::Null)
                    }
                    .boxed()
                }),
            )
            .await?;
            run(task, rt, ctx).await
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
                "started" => run(task, rt, ctx).await,
                other => Err(task_contract_fault(
                    "pi.plugin",
                    &format!("no handler for phase {other}"),
                )),
            }
        }
        .boxed()
    }

    fn abort(
        &self,
        _task: Arc<Task>,
        _rt: Arc<Runtime>,
        _ctx: Context,
    ) -> BoxFuture<'static, anyhow::Result<AbortClosure>> {
        async move {
            // `async abort() { return async () => null }` (`plugin.ts:21-23`).
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

fn json_checkpoint_started() -> Value {
    serde_json::json!({ "phase": "started" })
}

/// Upstream `run` (`plugin.ts:27-43`).
async fn run(task: Arc<Task>, rt: Arc<Runtime>, ctx: Context) -> anyhow::Result<Step> {
    let handler = handler_name(&task);
    let Some(handler_fn) = rt.plugin(&handler) else {
        return Ok(done(Completion {
            status: "failed".to_owned(),
            result: None,
            failure: Some(serde_json::json!({
                "reason": "missing_handler",
                "detail": handler,
            })),
        }));
    };
    let api = ToolApi::base(rt.clone(), task.id, task.conversation_id);
    let input = task.input.get("input").cloned().unwrap_or(Value::Null);
    match handler_fn(input, api, ctx.clone()).await {
        Ok(result) => {
            // `toStored(await handler(...))`.
            let stored: Value = serde_json::from_value(result).unwrap_or(Value::Null);
            Ok(done(Completion::completed(stored)))
        }
        Err(error) => {
            let aborted = ctx
                .abort_signal()
                .is_some_and(|signal| signal.is_cancelled());
            if aborted {
                return Err(error);
            }
            Ok(done(Completion {
                status: "failed".to_owned(),
                result: None,
                failure: Some(serde_json::json!({
                    "reason": "threw",
                    "detail": format!("{error}"),
                })),
            }))
        }
    }
}

fn done(completion: Completion) -> Step {
    Step::Done(Box::new(
        move |_tx: &mut Tx, _current: Task, _ctx: Context| {
            let completion = completion.clone();
            async move { Ok(completion) }.boxed()
        },
    ))
}
