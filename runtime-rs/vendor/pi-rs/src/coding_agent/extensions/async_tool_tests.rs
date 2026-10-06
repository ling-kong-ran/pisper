use super::{
    loader::ExtensionRuntime, runner::ExtensionRunner, types::*, wrapper::wrap_registered_tool,
};
use crate::agent_core::types::{AgentTool, AgentToolResult, ExecuteFn};
use crate::coding_agent::agent_session::base_tools::create_tool_definition_from_agent_tool;
use serde_json::json;
use std::{
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio_util::sync::CancellationToken;
fn runner() -> ExtensionRunner {
    ExtensionRunner::new(
        Vec::new(),
        ExtensionRuntime::new(),
        "/workspace",
        Arc::new(()),
        Arc::new(NoopProviderRegistry),
    )
}
fn native(execute: Arc<ExecuteFn>) -> Arc<AgentTool> {
    Arc::new(AgentTool {
        name: "native".into(),
        label: "native".into(),
        description: "test".into(),
        parameters: json!({"type":"object"}),
        constrained_sampling: None,
        execute,
        prepare_arguments: None,
        replay: None,
        execution_mode: None,
    })
}
fn wrap(definition: Arc<ToolDefinition>) -> AgentTool {
    wrap_registered_tool(
        &RegisteredTool {
            definition,
            source_info: create_synthetic_source_info("test", "test", None, None, None),
        },
        &runner(),
    )
}
#[tokio::test(flavor = "current_thread")]
async fn native_async_override_yields_and_roundtrips_updates_through_both_wrappers() {
    let token_slot = Arc::new(Mutex::new(None));
    let slot = token_slot.clone();
    let tool = native(Arc::new(move |id, args, signal, update| {
        let slot = slot.clone();
        Box::pin(async move {
            assert_eq!(id, "call");
            assert_eq!(args, json!({"input":7}));
            *slot.lock().unwrap() = signal;
            tokio::time::sleep(Duration::from_millis(2)).await;
            let partial: AgentToolResult = serde_json::from_value(
                json!({"content":[{"type":"text","text":"partial"}],"details":{"step":1}}),
            )
            .unwrap();
            update.unwrap()(&partial);
            Ok(partial)
        })
    }));
    let wrapped = wrap(create_tool_definition_from_agent_tool(&tool));
    let updates = Arc::new(Mutex::new(Vec::new()));
    let out = updates.clone();
    let cancellation = CancellationToken::new();
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        (wrapped.execute)(
            "call".into(),
            json!({"input":7}),
            Some(cancellation.clone()),
            Some(Arc::new(move |r| out.lock().unwrap().push(r.clone()))),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(updates.lock().unwrap().as_slice(), [result]);
    tokio::task::yield_now().await;
    cancellation.cancel();
    tokio::task::yield_now().await;
    assert!(
        !token_slot.lock().unwrap().as_ref().unwrap().is_cancelled(),
        "completed execution must detach cancellation"
    );
}
#[tokio::test(flavor = "current_thread")]
async fn abort_signal_reaches_native_token_before_and_during_execution() {
    for preabort in [true, false] {
        let entered = Arc::new(tokio::sync::Notify::new());
        let started = entered.clone();
        let tool = native(Arc::new(move |_, _, signal, _| {
            let started = started.clone();
            Box::pin(async move {
                started.notify_one();
                signal.unwrap().cancelled().await;
                Err(anyhow::anyhow!("Operation aborted"))
            })
        }));
        let definition = create_tool_definition_from_agent_tool(&tool);
        let signal = Arc::new(AbortSignal::new());
        if preabort {
            signal.abort();
        }
        let s = signal.clone();
        let abort = tokio::spawn(async move {
            entered.notified().await;
            s.abort();
        });
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            definition.execute_async.as_ref().unwrap()(
                "call".into(),
                json!({}),
                Some(signal),
                None,
                runner().create_context(),
            ),
        )
        .await
        .unwrap();
        assert_eq!(result.unwrap_err(), "Operation aborted");
        abort.await.unwrap();
    }
}
#[tokio::test(flavor = "current_thread")]
async fn direct_definition_completion_drops_abort_subscription() {
    let saved = Arc::new(Mutex::new(None));
    let out = saved.clone();
    let tool = native(Arc::new(move |_, _, signal, _| {
        let out = out.clone();
        Box::pin(async move {
            *out.lock().unwrap() = signal;
            Ok(AgentToolResult::default())
        })
    }));
    let definition = create_tool_definition_from_agent_tool(&tool);
    let signal = Arc::new(AbortSignal::new());
    definition.execute_async.as_ref().unwrap()(
        "call".into(),
        json!({}),
        Some(signal.clone()),
        None,
        runner().create_context(),
    )
    .await
    .unwrap();
    signal.abort();
    assert!(!saved.lock().unwrap().as_ref().unwrap().is_cancelled());
}
#[tokio::test(flavor = "current_thread")]
async fn dropping_wrapper_future_cleans_forwarder_and_native_signal() {
    let saved = Arc::new(Mutex::new(Weak::<AbortSignal>::new()));
    let out = saved.clone();
    let entered = Arc::new(tokio::sync::Notify::new());
    let started = entered.clone();
    let mut def = ToolDefinition::new("async", "async", "", json!({}));
    def.execute_async = Some(Arc::new(move |_, _, signal, _, _| {
        let (out, started) = (out.clone(), started.clone());
        Box::pin(async move {
            *out.lock().unwrap() = Arc::downgrade(signal.as_ref().unwrap());
            started.notify_one();
            signal.unwrap().cancelled().await;
            Err("Operation aborted".into())
        })
    }));
    let tool = wrap(Arc::new(def));
    let token = CancellationToken::new();
    let task = tokio::spawn((tool.execute)("c".into(), json!({}), Some(token), None));
    entered.notified().await;
    assert!(saved.lock().unwrap().upgrade().is_some());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::task::yield_now().await;
    assert!(saved.lock().unwrap().upgrade().is_none());
}
#[tokio::test(flavor = "current_thread")]
async fn wrapper_honors_already_cancelled_token_without_background_task() {
    let mut def = ToolDefinition::new("async", "async", "", json!({}));
    def.execute_async = Some(Arc::new(|_, _, signal, _, _| {
        Box::pin(async move {
            assert!(signal.unwrap().is_aborted());
            Err("Operation aborted".into())
        })
    }));
    let tool = wrap(Arc::new(def));
    let token = CancellationToken::new();
    token.cancel();
    assert_eq!(
        (tool.execute)("c".into(), json!({}), Some(token), None)
            .await
            .unwrap_err()
            .to_string(),
        "Operation aborted"
    );
}
