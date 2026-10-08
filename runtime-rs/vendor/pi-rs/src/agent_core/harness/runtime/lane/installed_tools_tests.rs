//! Public-entry regressions for the native Lane tool bridge. These use real
//! Harness construction, acceptance and Lane's installed pass, never the
//! explicit-environment dispatcher as a substitute for production wiring.
//! Behavior authority: upstream runtime/harness.ts (live config store),
//! runtime/lane.ts (installed drive), and runtime/drive/tools.ts:342-348,
//! 655-692 (lazy per-batch context, live tools, cancellation and persistence).

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::{Barrier, Notify};

use super::Lane;
use crate::agent_core::harness::agent_harness::harness_impl::{create_agent_harness, Harness};
use crate::agent_core::harness::agent_harness::{
    AcquireLaneOptions, AgentHarness as _, AgentHarnessOptions, AgentLane, HarnessEvent,
    OperationRequest, PromptPayload, RunOutcome,
};
use crate::agent_core::harness::context::{background_context, create_context_key};
use crate::agent_core::harness::hooks::{HookHandler, HookName, HookResult};
use crate::agent_core::harness::runtime::drive::tools::ToolContextSource;
use crate::agent_core::harness::runtime::drive_pass::{DriveOptions, DriveOutcome};
use crate::agent_core::harness::runtime::durable::OperationPhase;
use crate::agent_core::harness::session::types::{SessionReader as _, Storage};
use crate::agent_core::harness::session::{
    Entry, MemoryStorage, MemoryStorageOptions, OperationResultRecord, SessionMetadata,
    StorageBackedSession, TerminalStatus,
};
use crate::agent_core::harness::types::{
    AgentHarnessTool, AgentHarnessToolUpdateOptions, HarnessExecuteFn,
};
use crate::agent_core::types::{AgentMessage, AgentToolResult, ToolExecutionMode, ToolReplay};
use crate::ai::models::faux::{
    faux_assistant_message, faux_provider, faux_tool_call, FauxMessageOptions, FauxProviderHandle,
    FauxProviderOptions, FauxResponseStep, FauxToolCallOptions,
};
use crate::ai::models::{create_models, CreateModelsOptions};
use crate::ai::types::message::{AssistantMessage, Message, TextOrImageBlock, ToolResultMessage};
use crate::ai::types::primitives::StopReason;
use crate::ai::types::TextContent;

async fn within<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("installed drive did not reach its expected boundary")
}

struct Fixture<T: Clone + Send + Sync + 'static> {
    harness: Harness<T>,
    lane: Arc<Lane>,
    faux: Arc<FauxProviderHandle>,
    session: Arc<StorageBackedSession>,
}

async fn fixture<T: Clone + Send + Sync + 'static>(
    id: &str,
    tools: Vec<AgentHarnessTool<T>>,
    tool_context: Option<ToolContextSource<T>>,
    mode: ToolExecutionMode,
) -> Fixture<T> {
    let faux = Arc::new(faux_provider(FauxProviderOptions::default()));
    let mut models = create_models(CreateModelsOptions::default());
    models.set_provider(Arc::clone(&faux.provider));
    let model = faux.get_model(None).expect("faux model");
    let session = Arc::new(StorageBackedSession::new(
        SessionMetadata {
            id: id.to_owned(),
            created_at: 1,
            storage_version: 1,
            ..Default::default()
        },
        Arc::new(MemoryStorage::new(MemoryStorageOptions::default())) as Arc<dyn Storage>,
    ));
    let (harness, open) = create_agent_harness(
        AgentHarnessOptions {
            session: Arc::clone(&session),
            models,
            model,
            thinking_level: None,
            active_tool_names: None,
            tools: Some(tools),
            tool_context,
            system_prompt: None,
            resources: None,
            stream_options: None,
            retry: None,
            compaction: None,
            steering_mode: None,
            follow_up_mode: None,
            tool_execution: Some(mode),
            to_provider_messages: None,
            entry_projectors: None,
        },
        background_context(),
    )
    .await
    .expect("public harness attaches");
    assert!(open.is_empty());
    let lane = harness
        .lane("main", AcquireLaneOptions::default(), background_context())
        .await
        .expect("public main lane");
    Fixture {
        harness,
        lane,
        faux,
        session,
    }
}

fn tool<T: Send + Sync + 'static>(execute: Arc<HarnessExecuteFn<T>>) -> AgentHarnessTool<T> {
    AgentHarnessTool {
        name: "native".into(),
        label: "Native tool".into(),
        description: "Calls application code with its native context".into(),
        parameters: json!({"type": "object", "properties": {"n": {"type": "integer"}}, "required": ["n"]}),
        constrained_sampling: None,
        execute,
        prepare_arguments: None,
        replay: Some(ToolReplay::Safe),
        execution_mode: None,
    }
}

fn result(text: impl Into<String>) -> AgentToolResult {
    AgentToolResult {
        content: vec![TextOrImageBlock::Text(TextContent {
            text: text.into(),
            text_signature: None,
        })],
        ..Default::default()
    }
}

fn tool_turn(calls: &[(&str, Value)]) -> AssistantMessage {
    faux_assistant_message(
        calls
            .iter()
            .map(|(id, arguments)| {
                faux_tool_call(
                    "native",
                    arguments.clone(),
                    FauxToolCallOptions {
                        id: Some((*id).into()),
                    },
                )
            })
            .collect::<Vec<_>>(),
        FauxMessageOptions {
            stop_reason: Some(StopReason::ToolUse),
            ..Default::default()
        },
    )
}

fn done() -> AssistantMessage {
    faux_assistant_message("done", FauxMessageOptions::default())
}

fn text(message: &ToolResultMessage) -> &str {
    match message.content.as_slice() {
        [TextOrImageBlock::Text(text)] => &text.text,
        other => panic!("expected one text result, got {other:?}"),
    }
}

fn provider_results(messages: &[Message]) -> Vec<&ToolResultMessage> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some(result),
            _ => None,
        })
        .collect()
}

async fn persisted_results(lane: &Lane) -> Vec<(String, ToolResultMessage)> {
    lane.find_entries(None, background_context())
        .await
        .unwrap()
        .into_iter()
        .rev()
        .filter_map(|entry| match entry {
            Entry::Message {
                id,
                message: AgentMessage::ToolResult(result),
                ..
            } => Some((id, result)),
            _ => None,
        })
        .collect()
}

fn completed(outcome: RunOutcome) -> OperationResultRecord {
    let RunOutcome::Record(record) = outcome else {
        panic!("unexpected suspension: {outcome:?}")
    };
    assert_eq!(record.status, TerminalStatus::Completed, "{record:?}");
    record
}

#[derive(Clone)]
struct ApplicationContext {
    identity: Arc<AtomicUsize>,
    callback: Arc<dyn Fn(u64) -> u64 + Send + Sync>,
}

#[tokio::test]
async fn native_callbacks_updates_hooks_and_invocation_survive_installed_drive() {
    let identity = Arc::new(AtomicUsize::new(0));
    let callback: Arc<dyn Fn(u64) -> u64 + Send + Sync> = Arc::new(|n| n + 7);
    let expected_identity = Arc::clone(&identity);
    let expected_callback = Arc::clone(&callback);
    let invocation_ids = Arc::new(Mutex::new(None));
    let seen_ids = Arc::clone(&invocation_ids);
    let marker = create_context_key::<String>("native installed caller");
    let tool_marker = marker.clone();
    let mut native = tool(Arc::new(
        move |id, args, update, context: ApplicationContext, invocation, caller| {
            assert_eq!(id, "native-call");
            assert_eq!(args, json!({"n": 5}));
            assert!(Arc::ptr_eq(&context.identity, &expected_identity));
            assert!(Arc::ptr_eq(&context.callback, &expected_callback));
            assert_eq!(
                caller.get(&tool_marker).as_deref().map(String::as_str),
                Some("caller-value")
            );
            *seen_ids.lock().unwrap() = Some((
                invocation.invocation_id().to_owned(),
                invocation.operation_id().to_owned(),
            ));
            Box::pin(async move {
                context.identity.fetch_add(1, Ordering::SeqCst);
                invocation.set_memo("effect", Some(json!(12))).await;
                assert_eq!(invocation.get_memo("effect").await, Some(json!(12)));
                update(
                    &result("working"),
                    AgentHarnessToolUpdateOptions { checkpoint: true },
                );
                let mut output = result(format!(
                    "native:{}",
                    (context.callback)(args["n"].as_u64().unwrap())
                ));
                output.details = Some(json!({"native": true}));
                Ok(output)
            })
        },
    ));
    native.prepare_arguments = Some(Arc::new(|args| json!({"n": args["raw"]})));
    let fixture = fixture(
        "installed-native-value",
        vec![native],
        Some(ToolContextSource::Value(ApplicationContext {
            identity: Arc::clone(&identity),
            callback,
        })),
        ToolExecutionMode::Sequential,
    )
    .await;
    let hook_calls = Arc::new(AtomicUsize::new(0));
    for hook in [HookName::BeforeTool, HookName::AfterTool] {
        let calls = Arc::clone(&hook_calls);
        let handler: HookHandler = Arc::new(move |_invocation, _context| {
            calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                Ok(match hook {
                    HookName::BeforeTool => HookResult::BeforeTool(None),
                    HookName::AfterTool => HookResult::AfterTool(None),
                    _ => unreachable!(),
                })
            })
        });
        fixture.harness.hooks().on(hook, handler, None).unwrap();
    }
    let events = Arc::new(Mutex::new(Vec::new()));
    for name in ["tool_start", "tool_update", "tool_end"] {
        let events = Arc::clone(&events);
        fixture
            .harness
            .events()
            .on(name, move |event, _context| {
                events.lock().unwrap().push(event);
                Box::pin(async {})
            })
            .unwrap();
    }
    let continued = Arc::new(AtomicUsize::new(0));
    let continuation = Arc::clone(&continued);
    fixture.faux.set_responses(vec![
        tool_turn(&[("native-call", json!({"raw": 5}))]).into(),
        FauxResponseStep::Factory(Arc::new(move |args| {
            let results = provider_results(args.context.messages());
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].tool_call_id, "native-call");
            assert_eq!(text(results[0]), "native:12");
            assert!(!results[0].is_error);
            continuation.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(done()) })
        })),
    ]);
    let record = completed(
        within(fixture.lane.prompt(
            "use the native callback",
            None,
            background_context().with_value(&marker, "caller-value".to_owned()),
        ))
        .await
        .unwrap()
        .unwrap(),
    );
    let results = persisted_results(&fixture.lane).await;
    assert_eq!(results.len(), 1);
    assert_eq!(text(&results[0].1), "native:12");
    assert_eq!(results[0].1.details, Some(json!({"native": true})));
    assert_eq!(
        invocation_ids.lock().unwrap().as_ref(),
        Some(&(results[0].0.clone(), record.operation_id.clone()))
    );
    assert_eq!(identity.load(Ordering::SeqCst), 1);
    assert_eq!(continued.load(Ordering::SeqCst), 1);
    assert_eq!(hook_calls.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.faux.state().lock().unwrap().call_count, 2);
    assert_eq!(
        fixture
            .lane
            .get_result(&record.operation_id, background_context())
            .await
            .unwrap(),
        Some(record)
    );
    {
        let events = events.lock().unwrap();
        assert!(
            matches!(events.first().map(AsRef::as_ref), Some(HarnessEvent::ToolStart { tool_call_id, .. }) if tool_call_id == "native-call")
        );
        assert!(events.iter().any(|event| matches!(event.as_ref(), HarnessEvent::ToolUpdate { partial_result, .. } if partial_result.content == result("working").content)));
        assert!(matches!(
            events.last().map(AsRef::as_ref),
            Some(HarnessEvent::ToolEnd {
                is_error: false,
                ..
            })
        ));
    }
    fixture.harness.close(background_context()).await.unwrap();
}

#[tokio::test]
async fn set_tools_during_generation_updates_the_installed_pass_executor_and_schema() {
    let old_calls = Arc::new(AtomicUsize::new(0));
    let old_sink = Arc::clone(&old_calls);
    let old = tool(Arc::new(
        move |_id, _args, _update, (): (), _invocation, _caller| {
            old_sink.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(result("obsolete")) })
        },
    ));
    let fixture = fixture(
        "installed-live-config",
        vec![old],
        None,
        ToolExecutionMode::Parallel,
    )
    .await;
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let first_started = Arc::clone(&started);
    let first_release = Arc::clone(&release);
    fixture.faux.set_responses(vec![
        FauxResponseStep::Factory(Arc::new(move |_args| {
            let started = Arc::clone(&first_started);
            let release = Arc::clone(&first_release);
            Box::pin(async move {
                started.notify_one();
                release.notified().await;
                // Only the updated declaration accepts these arguments.
                Ok(tool_turn(&[("updated", json!({"updated": "live"}))]))
            })
        })),
        FauxResponseStep::Factory(Arc::new(|args| {
            let results = provider_results(args.context.messages());
            assert_eq!(results.len(), 1);
            assert_eq!(text(results[0]), "updated:live");
            assert!(!results[0].is_error);
            assert!(args.context.messages().iter().any(|message| match message {
                Message::System(system) => system
                    .tools_added
                    .as_ref()
                    .is_some_and(|tools| tools.iter().any(|tool| tool.name == "native"
                        && tool.parameters["required"] == json!(["updated"]))),
                _ => false,
            }));
            Box::pin(async { Ok(done()) })
        })),
    ]);
    let lane = Arc::clone(&fixture.lane);
    let running = tokio::spawn(async move {
        lane.prompt("update tools", None, background_context())
            .await
    });
    within(started.notified()).await;
    let new_calls = Arc::new(AtomicUsize::new(0));
    let new_sink = Arc::clone(&new_calls);
    let mut updated = tool(Arc::new(
        move |_id, args, _update, (): (), _invocation, _caller| {
            new_sink.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                Ok(result(format!(
                    "updated:{}",
                    args["updated"].as_str().unwrap()
                )))
            })
        },
    ));
    updated.parameters = json!({"type": "object", "properties": {"updated": {"type": "string"}}, "required": ["updated"]});
    fixture
        .harness
        .set_tools(vec![updated], background_context())
        .await
        .unwrap();
    assert_eq!(new_calls.load(Ordering::SeqCst), 0);
    release.notify_one();
    completed(within(running).await.unwrap().unwrap().unwrap());
    assert_eq!(old_calls.load(Ordering::SeqCst), 0);
    assert_eq!(new_calls.load(Ordering::SeqCst), 1);
    let results = persisted_results(&fixture.lane).await;
    assert_eq!(results.len(), 1);
    assert_eq!(text(&results[0].1), "updated:live");
    fixture.harness.close(background_context()).await.unwrap();
}

#[derive(Clone)]
struct BatchContext {
    number: usize,
    shared: Arc<AtomicUsize>,
}

#[tokio::test]
async fn context_is_lazy_once_per_batch_and_shared_by_parallel_native_tools() {
    let resolutions = Arc::new(AtomicUsize::new(0));
    let shared_contexts = Arc::new(Mutex::new(Vec::new()));
    let resolve_count = Arc::clone(&resolutions);
    let contexts = Arc::clone(&shared_contexts);
    let marker = create_context_key::<String>("provider caller");
    let context_marker = marker.clone();
    let source = ToolContextSource::Provider(Arc::new(move |caller| {
        assert_eq!(
            caller.get(&context_marker).as_deref().map(String::as_str),
            Some("per-drive")
        );
        let number = resolve_count.fetch_add(1, Ordering::SeqCst) + 1;
        let shared = Arc::new(AtomicUsize::new(0));
        contexts.lock().unwrap().push(Arc::clone(&shared));
        Box::pin(async move { BatchContext { number, shared } })
    }));
    let barrier = Arc::new(Barrier::new(2));
    let native = tool(Arc::new(
        move |id, _args, _update, context: BatchContext, _invocation, _caller| {
            let barrier = Arc::clone(&barrier);
            Box::pin(async move {
                let expected = if id == "c" { 2 } else { 1 };
                assert_eq!(context.number, expected);
                context.shared.fetch_add(1, Ordering::SeqCst);
                if context.number == 1 {
                    within(barrier.wait()).await;
                }
                Ok(result(format!("batch:{}", context.number)))
            })
        },
    ));
    let fixture = fixture(
        "installed-batch-context",
        vec![native],
        Some(source),
        ToolExecutionMode::Parallel,
    )
    .await;
    assert_eq!(
        resolutions.load(Ordering::SeqCst),
        0,
        "attach/acquire must not resolve context"
    );
    let before_generation = Arc::clone(&resolutions);
    let between_batches = Arc::clone(&resolutions);
    let after_batches = Arc::clone(&resolutions);
    fixture.faux.set_responses(vec![
        FauxResponseStep::Factory(Arc::new(move |_args| {
            assert_eq!(
                before_generation.load(Ordering::SeqCst),
                0,
                "acceptance and generation must not resolve tool context"
            );
            Box::pin(async { Ok(tool_turn(&[("a", json!({"n": 1})), ("b", json!({"n": 2}))])) })
        })),
        FauxResponseStep::Factory(Arc::new(move |args| {
            assert_eq!(between_batches.load(Ordering::SeqCst), 1);
            let results = provider_results(args.context.messages());
            assert_eq!(
                results
                    .iter()
                    .map(|result| result.tool_call_id.as_str())
                    .collect::<Vec<_>>(),
                ["a", "b"]
            );
            assert!(results
                .iter()
                .all(|result| !result.is_error && text(result) == "batch:1"));
            Box::pin(async { Ok(tool_turn(&[("c", json!({"n": 3}))])) })
        })),
        FauxResponseStep::Factory(Arc::new(move |args| {
            assert_eq!(after_batches.load(Ordering::SeqCst), 2);
            assert_eq!(provider_results(args.context.messages()).len(), 3);
            Box::pin(async { Ok(done()) })
        })),
    ]);
    completed(
        within(fixture.lane.prompt(
            "two batches",
            None,
            background_context().with_value(&marker, "per-drive".to_owned()),
        ))
        .await
        .unwrap()
        .unwrap(),
    );
    assert_eq!(resolutions.load(Ordering::SeqCst), 2);
    {
        let contexts = shared_contexts.lock().unwrap();
        assert_eq!(contexts.len(), 2);
        assert!(!Arc::ptr_eq(&contexts[0], &contexts[1]));
        assert_eq!(contexts[0].load(Ordering::SeqCst), 2);
        assert_eq!(contexts[1].load(Ordering::SeqCst), 1);
    }
    let results = persisted_results(&fixture.lane).await;
    assert_eq!(
        results
            .iter()
            .map(|(_, result)| result.tool_call_id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
    assert_eq!(fixture.faux.state().lock().unwrap().call_count, 3);
    fixture.harness.close(background_context()).await.unwrap();
}

#[tokio::test]
async fn omitted_unit_context_executes_and_persists_success_or_executor_error() {
    for fail in [false, true] {
        let executions = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&executions);
        let native = tool(Arc::new(
            move |_id, _args, _update, (): (), _invocation, _caller| {
                calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    if fail {
                        anyhow::bail!("native executor failed");
                    }
                    Ok(result("unit executed"))
                })
            },
        ));
        let fixture = fixture(
            "installed-unit-or-error",
            vec![native],
            None,
            ToolExecutionMode::Sequential,
        )
        .await;
        fixture.faux.set_responses(vec![
            tool_turn(&[("unit", json!({"n": 1}))]).into(),
            FauxResponseStep::Factory(Arc::new(move |args| {
                let results = provider_results(args.context.messages());
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].is_error, fail);
                assert!(text(results[0]).contains(if fail {
                    "native executor failed"
                } else {
                    "unit executed"
                }));
                Box::pin(async { Ok(done()) })
            })),
        ]);
        completed(
            within(fixture.lane.prompt("unit tool", None, background_context()))
                .await
                .unwrap()
                .unwrap(),
        );
        assert_eq!(executions.load(Ordering::SeqCst), 1);
        let results = persisted_results(&fixture.lane).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1.is_error, fail);
        assert!(text(&results[0].1).contains(if fail {
            "native executor failed"
        } else {
            "unit executed"
        }));
        assert_eq!(fixture.faux.state().lock().unwrap().call_count, 2);
        fixture.harness.close(background_context()).await.unwrap();
    }
}

#[tokio::test]
async fn text_only_generation_never_resolves_the_tool_context() {
    let resolutions = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&resolutions);
    let source = ToolContextSource::Provider(Arc::new(move |_caller| {
        count.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {})
    }));
    let native = tool(Arc::new(
        |_id, _args, _update, (): (), _invocation, _caller| {
            panic!("text-only generation must not execute tools")
        },
    ));
    let fixture = fixture(
        "installed-no-tool-turn",
        vec![native],
        Some(source),
        ToolExecutionMode::Parallel,
    )
    .await;
    fixture.faux.set_responses(vec![done().into()]);
    completed(
        within(
            fixture
                .lane
                .prompt("just answer", None, background_context()),
        )
        .await
        .unwrap()
        .unwrap(),
    );
    assert_eq!(resolutions.load(Ordering::SeqCst), 0);
    assert!(persisted_results(&fixture.lane).await.is_empty());
    fixture.harness.close(background_context()).await.unwrap();
}

#[tokio::test]
async fn durable_abort_before_installation_never_resolves_context_or_executes() {
    let resolutions = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&resolutions);
    let source = ToolContextSource::Provider(Arc::new(move |_caller| {
        count.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {})
    }));
    let native = tool(Arc::new(
        |_id, _args, _update, (): (), _invocation, _caller| {
            panic!("pre-cancelled run must not execute tools")
        },
    ));
    let fixture = fixture(
        "installed-pre-cancel",
        vec![native],
        Some(source),
        ToolExecutionMode::Parallel,
    )
    .await;
    AgentLane::accept(
        &*fixture.lane,
        &OperationRequest::Prompt {
            operation_id: Some("cancel-before-install".into()),
            payload: PromptPayload::Text {
                prompt: "cancel me".into(),
                images: None,
            },
        },
        background_context(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(resolutions.load(Ordering::SeqCst), 0);
    fixture
        .lane
        .request_abort("cancel-before-install", background_context())
        .await
        .unwrap()
        .unwrap();
    let outcome = within(fixture.lane.drive(
        &DriveOptions {
            operation_id: "cancel-before-install".into(),
            wait_for_retry: None,
            poll_deferred: None,
        },
        background_context(),
    ))
    .await
    .unwrap()
    .unwrap();
    assert!(
        matches!(outcome, DriveOutcome::Settled { outcome } if outcome.status == TerminalStatus::Aborted)
    );
    assert_eq!(resolutions.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.faux.state().lock().unwrap().call_count, 0);
    assert!(persisted_results(&fixture.lane).await.is_empty());
    fixture.harness.close(background_context()).await.unwrap();
}

#[tokio::test]
async fn abort_during_pending_context_waits_for_resolution_but_never_executes() {
    for mode in [ToolExecutionMode::Sequential, ToolExecutionMode::Parallel] {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let resolved = Arc::new(AtomicUsize::new(0));
        let provider_started = Arc::clone(&started);
        let provider_release = Arc::clone(&release);
        let provider_resolved = Arc::clone(&resolved);
        let source = ToolContextSource::Provider(Arc::new(move |_caller| {
            let started = Arc::clone(&provider_started);
            let release = Arc::clone(&provider_release);
            let resolved = Arc::clone(&provider_resolved);
            Box::pin(async move {
                started.notify_one();
                release.notified().await;
                resolved.fetch_add(1, Ordering::SeqCst);
            })
        }));
        let executions = Arc::new(AtomicUsize::new(0));
        let called = Arc::clone(&executions);
        let native = tool(Arc::new(
            move |_id, _args, _update, (): (), _invocation, _caller| {
                called.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok(result("must not run")) })
            },
        ));
        let fixture = fixture(
            "installed-pending-context-cancel",
            vec![native],
            Some(source),
            mode,
        )
        .await;
        fixture
            .faux
            .set_responses(vec![tool_turn(&[("cancelled", json!({"n": 1}))]).into()]);
        let lane = Arc::clone(&fixture.lane);
        let running = tokio::spawn(async move {
            lane.prompt("cancel context wait", None, background_context())
                .await
        });
        within(started.notified()).await;
        let operation = fixture
            .lane
            .state()
            .operation
            .expect("tool operation remains open");
        assert!(matches!(
            operation.state.phase,
            OperationPhase::Tools { .. }
        ));
        within(
            fixture
                .lane
                .request_abort(&operation.meta.operation_id, background_context()),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(resolved.load(Ordering::SeqCst), 0);
        assert!(
            !running.is_finished(),
            "abort must not drop the pending context future"
        );
        release.notify_one();
        let outcome = within(running).await.unwrap().unwrap().unwrap();
        assert!(
            matches!(outcome, RunOutcome::Record(record) if record.status == TerminalStatus::Aborted)
        );
        assert_eq!(resolved.load(Ordering::SeqCst), 1);
        assert_eq!(executions.load(Ordering::SeqCst), 0);
        let results = persisted_results(&fixture.lane).await;
        assert_eq!(results.len(), 1);
        assert!(results[0].1.is_error);
        assert!(text(&results[0].1).contains("cancelled"));
        assert_eq!(fixture.faux.state().lock().unwrap().call_count, 1);
        fixture.harness.close(background_context()).await.unwrap();
    }
}

#[derive(Debug)]
struct ContextFailure;

impl std::fmt::Display for ContextFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("native context provider failed")
    }
}

impl std::error::Error for ContextFailure {}

#[tokio::test]
async fn fallible_context_rejection_faults_the_harness_without_fabricating_a_result() {
    let executions = Arc::new(AtomicUsize::new(0));
    let called = Arc::clone(&executions);
    let native = tool(Arc::new(
        move |_id, _args, _update, (): (), _invocation, _caller| {
            called.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(result("must not run")) })
        },
    ));
    let source = ToolContextSource::FallibleProvider(Arc::new(|_caller| {
        Box::pin(async { Err(anyhow::Error::new(ContextFailure)) })
    }));
    let fixture = fixture(
        "installed-context-failure",
        vec![native],
        Some(source),
        ToolExecutionMode::Parallel,
    )
    .await;
    fixture.faux.set_responses(vec![
        tool_turn(&[("context-failed", json!({"n": 1}))]).into()
    ]);
    let error = within(
        fixture
            .lane
            .prompt("context fails", None, background_context()),
    )
    .await
    .expect_err("context failure must fault the installed pass");
    assert!(error.to_string().contains("fault"), "{error:#}");
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.faux.state().lock().unwrap().call_count, 1);
    // Query the public harness for the retained first fault rather than the
    // lane's sealed observation error; the original native cause must survive.
    let fault = fixture
        .harness
        .get_tools(background_context())
        .await
        .expect_err("the harness is faulted");
    assert!(
        fault
            .chain()
            .any(|cause| cause.downcast_ref::<ContextFailure>().is_some()),
        "native error identity was lost: {fault:#}"
    );
    let operation = fixture
        .lane
        .state()
        .operation
        .expect("failed tool batch stays durable for recovery");
    let OperationPhase::Tools { batch } = operation.state.phase else {
        panic!("context failure must not advance the tool batch")
    };
    for call in batch.calls {
        assert!(
            fixture
                .session
                .get_entry(&call.result_entry_id, background_context())
                .await
                .unwrap()
                .is_none(),
            "no fake successful tool result may be persisted"
        );
    }
    fixture.harness.close(background_context()).await.unwrap();
}
