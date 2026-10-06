//! Tests for the `hooks.ts` port. The primary oracle is the
//! `describe("HookRegistry")` block of upstream
//! `test/harness/execution-primitives.test.ts:54-367` (ten tests, ported
//! one-to-one with the adaptations noted per test); the remaining tests cover
//! the aggregate paths the oracle exercises only through the runtime
//! (`before_run_end`, `before_payload`, `after_response`, `before_compaction`)
//! and the stream-options patch functions.
//!
//! Port adaptations, all disclosed in the module docs of `hooks.rs`:
//! - `createGate`/`AbortRequested` (upstream `effect-gate.ts`, M3b Task 6)
//!   stand in as a local [`TestGate`] implementing the ported [`Gate`] trait
//!   with the same observable admission/signal timing.
//! - `AbortSignal` reasons become the fixed `"the operation was aborted"`
//!   message (`CancellationToken` carries no reason).
//! - The telemetry-span assertions of the "child context of the active hook
//!   span" oracle test are deferred to the telemetry task; the context-value
//!   preservation half is ported here.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use serde_json::json;

use super::*;
use crate::agent_core::harness::compaction::FileOperations;
use crate::agent_core::harness::{
    background_context, create_context_key, with_abort_signal, with_context_value, Context,
    DEFAULT_COMPACTION_SETTINGS,
};
use crate::agent_core::types::AgentMessage;
use crate::ai::types::content::TextContent;
use crate::ai::types::message::{AssistantMessage, TextOrImageBlock, UserMessage};
use crate::ai::types::model::{Model, ModelInput};
use crate::ai::types::primitives::{ModelCost, StopReason};
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A minimal [`BranchSummaryResult`] fixture standing in for the upstream
/// tests' `{ summary, readFiles, modifiedFiles }` JSON literals.
fn branch_summary_result(summary: &str) -> BranchSummaryResult {
    BranchSummaryResult {
        summary: summary.to_string(),
        usage: None,
        read_files: vec![],
        modified_files: vec![],
    }
}

/// A minimal [`CompactResult`] fixture for the upstream
/// `{ "summary": "plain" }` JSON literal.
fn compaction_result(summary: &str) -> CompactResult {
    CompactResult {
        summary: summary.to_string(),
        tokens_before: 0,
        usage: None,
        retained_tail: vec![],
        details: None,
    }
}

/// Error collector standing in for the upstream `errors: Error[]` reporter.
type ErrorLog = Arc<Mutex<Vec<String>>>;

fn reporter(log: ErrorLog) -> HookErrorReporter {
    Arc::new(move |error: anyhow::Error, _hook, _lane, _context| {
        let log = Arc::clone(&log);
        Box::pin(async move {
            log.lock().unwrap().push(error.to_string());
        })
    })
}

fn error_log() -> ErrorLog {
    Arc::new(Mutex::new(Vec::new()))
}

fn logged_errors(log: &ErrorLog) -> Vec<String> {
    log.lock().unwrap().clone()
}

fn counter() -> Arc<AtomicUsize> {
    Arc::new(AtomicUsize::new(0))
}

fn calls_log() -> Arc<Mutex<Vec<&'static str>>> {
    Arc::new(Mutex::new(Vec::new()))
}

fn push_call(log: &Arc<Mutex<Vec<&'static str>>>, call: &'static str) {
    log.lock().unwrap().push(call);
}

fn calls(log: &Arc<Mutex<Vec<&'static str>>>) -> Vec<&'static str> {
    log.lock().unwrap().clone()
}

fn user_message(text: &str, timestamp: i64) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: crate::ai::types::message::StringOrBlocks::Text(text.to_string()),
        timestamp,
    })
}

fn user_message_content(message: &AgentMessage) -> String {
    match message {
        AgentMessage::User(user) => match &user.content {
            crate::ai::types::message::StringOrBlocks::Text(text) => text.clone(),
            crate::ai::types::message::StringOrBlocks::Blocks(_) => panic!("expected text"),
        },
        _ => panic!("expected user message"),
    }
}

fn text_block(text: &str) -> TextOrImageBlock {
    TextOrImageBlock::Text(TextContent {
        text: text.to_string(),
        text_signature: None,
    })
}

fn text_block_text(block: &TextOrImageBlock) -> String {
    match block {
        TextOrImageBlock::Text(content) => content.text.clone(),
        TextOrImageBlock::Image(_) => panic!("expected text block"),
    }
}

fn test_model() -> Model {
    Model {
        id: "claude-test".to_string(),
        name: "Claude Test".to_string(),
        api: "anthropic-messages".to_string(),
        provider: "anthropic".to_string(),
        base_url: "https://api.anthropic.com".to_string(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![ModelInput::Text],
        cost: ModelCost::default(),
        context_window: 100_000,
        max_tokens: 4096,
        sampling_params: None,
        sampling_params_by_thinking_level: None,
        headers: None,
        compat: None,
        r#type: None,
        prompt_cache: None,
        input_limits: None,
    }
}

/// A settled assistant message (upstream `stopReason` is narrowed to
/// non-`pending` values; the port keeps the invariant by convention).
fn settled_message(text: &str) -> AssistantMessage {
    AssistantMessage {
        content: vec![crate::ai::types::message::AssistantBlock::Text(
            TextContent {
                text: text.to_string(),
                text_signature: None,
            },
        )],
        api: "anthropic-messages".to_string(),
        provider: "anthropic".to_string(),
        model: "claude-test".to_string(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        diagnostics: None,
        usage: crate::ai::types::primitives::Usage::default(),
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 1,
    }
}

fn simple_handler<F>(f: F) -> HookHandler
where
    F: Fn(HookInvocation, Context) -> BoxFuture<'static, anyhow::Result<HookResult>>
        + Send
        + Sync
        + 'static,
{
    Arc::new(f)
}

/// Local stand-in for the upstream `createGate().gate` (`effect-gate.ts:31-64`,
/// M3b Task 6): the same observable admission/signal timing — `beginAbort`
/// refuses admission without cancelling the signal until `signalAbort`, and
/// `close` refuses admission with its message and cancels the signal.
#[derive(Clone)]
struct TestGate {
    state: Arc<Mutex<TestGateState>>,
    signal: CancellationToken,
}

#[derive(Clone, PartialEq)]
enum TestGateState {
    Open,
    Aborting,
    Closed(String),
}

impl TestGate {
    fn new() -> Arc<Self> {
        Arc::new(TestGate {
            state: Arc::new(Mutex::new(TestGateState::Open)),
            signal: CancellationToken::new(),
        })
    }

    fn begin_abort(&self) {
        let mut state = self.state.lock().unwrap();
        if *state == TestGateState::Open {
            *state = TestGateState::Aborting;
        }
    }

    fn signal_abort(&self) {
        if *self.state.lock().unwrap() == TestGateState::Aborting && !self.signal.is_cancelled() {
            self.signal.cancel();
        }
    }

    fn close(&self, message: &str) {
        {
            let mut state = self.state.lock().unwrap();
            if matches!(&*state, TestGateState::Closed(_)) {
                return;
            }
            *state = TestGateState::Closed(message.to_string());
        }
        if !self.signal.is_cancelled() {
            self.signal.cancel();
        }
    }
}

impl Gate for TestGate {
    fn signal(&self) -> CancellationToken {
        self.signal.clone()
    }

    fn admit(&self) -> anyhow::Result<()> {
        match &*self.state.lock().unwrap() {
            TestGateState::Open => Ok(()),
            TestGateState::Aborting => Err(anyhow::anyhow!("abort requested")),
            TestGateState::Closed(message) => Err(anyhow::anyhow!(message.clone())),
        }
    }
}

fn before_drive_invocation(operation: DriveOperation) -> HookInvocation {
    HookInvocation {
        lane: "main".to_string(),
        run_id: "run".to_string(),
        event: HookEvent::BeforeDrive(BeforeDriveEvent { operation }),
    }
}

fn before_run_invocation() -> HookInvocation {
    HookInvocation {
        lane: "main".to_string(),
        run_id: "run".to_string(),
        event: HookEvent::BeforeRun(BeforeRunEvent {
            prompt: vec![user_message("prompt", 1)],
            resources: Resources::default(),
        }),
    }
}

fn before_tool_invocation() -> HookInvocation {
    HookInvocation {
        lane: "main".to_string(),
        run_id: "run".to_string(),
        event: HookEvent::BeforeTool(BeforeToolEvent {
            tool_call_id: "call".to_string(),
            tool_name: "tool".to_string(),
            args: json!({}),
        }),
    }
}

fn after_tool_invocation() -> HookInvocation {
    HookInvocation {
        lane: "main".to_string(),
        run_id: "run".to_string(),
        event: HookEvent::AfterTool(AfterToolEvent {
            tool_call_id: "call".to_string(),
            tool_name: "tool".to_string(),
            args: json!({}),
            content: vec![text_block("raw")],
            details: None,
            is_error: true,
            usage: None,
        }),
    }
}

fn transform_context_invocation() -> HookInvocation {
    HookInvocation {
        lane: "main".to_string(),
        run_id: "run".to_string(),
        event: HookEvent::TransformContext(TransformContextEvent {
            messages: vec![user_message("original", 1)],
            system_prompt: "base".to_string(),
        }),
    }
}

fn before_request_invocation() -> HookInvocation {
    HookInvocation {
        lane: "main".to_string(),
        run_id: "run".to_string(),
        event: HookEvent::BeforeRequest(BeforeRequestEvent {
            model: test_model(),
            step: RequestStep::Assistant,
            attempt: 1,
            stream_options: AgentHarnessStreamOptions {
                headers: Some(BTreeMap::from([
                    ("a".to_string(), "1".to_string()),
                    ("b".to_string(), "2".to_string()),
                ])),
                metadata: Some(BTreeMap::from([("x".to_string(), json!(1))])),
                ..AgentHarnessStreamOptions::default()
            },
        }),
    }
}

fn before_navigation_invocation() -> HookInvocation {
    HookInvocation {
        lane: "main".to_string(),
        run_id: "run".to_string(),
        event: HookEvent::BeforeNavigation(BeforeNavigationEvent {
            target_id: "target".to_string(),
            preparation: BranchPreparation {
                messages: vec![],
                file_ops: FileOperations::new(),
                total_tokens: 0,
            },
            custom_instructions: None,
        }),
    }
}

/// Wait for a predicate by yielding, mirroring the busy-wait the context
/// tests use for linker-task propagation.
async fn wait_for(predicate: impl Fn() -> bool) {
    for _ in 0..10_000 {
        if predicate() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("condition not reached");
}

// ---------------------------------------------------------------------------
// Oracle: describe("HookRegistry") (execution-primitives.test.ts:54-367)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn before_run_aggregates_messages_in_registration_order_with_prior_output_visible() {
    // execution-primitives.test.ts:55-87.
    let errors = error_log();
    let hooks = HookRegistry::new(reporter(errors.clone()));
    hooks
        .on(
            HookName::BeforeRun,
            simple_handler(|_invocation, _context| {
                Box::pin(async move {
                    Ok(HookResult::BeforeRun(Some(BeforeRunHookResult {
                        messages: Some(vec![user_message("first", 2)]),
                    })))
                })
            }),
            None,
        )
        .expect("registration before close");
    let second_saw = Arc::new(Mutex::new(Vec::new()));
    let saw = Arc::clone(&second_saw);
    hooks
        .on(
            HookName::BeforeRun,
            simple_handler(move |invocation, _context| {
                let saw = Arc::clone(&saw);
                Box::pin(async move {
                    match &invocation.event {
                        HookEvent::BeforeRun(event) => {
                            saw.lock().unwrap().extend(
                                event
                                    .prompt
                                    .iter()
                                    .map(user_message_content)
                                    .collect::<Vec<_>>(),
                            );
                        }
                        _ => panic!("expected before_run event"),
                    }
                    Ok(HookResult::BeforeRun(Some(BeforeRunHookResult {
                        messages: Some(vec![user_message("second", 3)]),
                    })))
                })
            }),
            None,
        )
        .expect("registration before close");

    let result = hooks
        .run_with_gate(
            before_run_invocation(),
            TestGate::new(),
            background_context(),
        )
        .await
        .expect("pipeline succeeds");

    match result {
        HookResult::BeforeRun(Some(result)) => {
            let messages = result.messages.expect("aggregate carries messages");
            assert_eq!(user_message_content(&messages[0]), "first");
            assert_eq!(user_message_content(&messages[1]), "second");
        }
        other => panic!("expected before_run aggregate, got {:?}", other.hook_name()),
    }
    assert_eq!(*second_saw.lock().unwrap(), vec!["prompt", "first"]);
    assert!(logged_errors(&errors).is_empty());
}

#[tokio::test]
async fn gate_refuses_before_the_pipeline_and_late_registrations_miss_the_running_pipeline() {
    // execution-primitives.test.ts:89-120. The upstream synchronous-throw
    // assertion maps to: the refused run resolves to the gate error without
    // starting any handler.
    let hooks = HookRegistry::new(Arc::new(|_, _, _, _| Box::pin(async {})));
    let log = calls_log();
    let (sender, receiver) = tokio::sync::watch::channel(false);
    let first_log = Arc::clone(&log);
    hooks
        .on(
            HookName::BeforeRun,
            simple_handler(move |_invocation, _context| {
                let log = Arc::clone(&first_log);
                let receiver = receiver.clone();
                Box::pin(async move {
                    push_call(&log, "first:start");
                    while !*receiver.borrow() {
                        tokio::task::yield_now().await;
                    }
                    push_call(&log, "first:end");
                    Ok(HookResult::BeforeRun(None))
                })
            }),
            None,
        )
        .expect("registration before close");
    let second_log = Arc::clone(&log);
    hooks
        .on(
            HookName::BeforeRun,
            simple_handler(move |_invocation, _context| {
                let log = Arc::clone(&second_log);
                Box::pin(async move {
                    push_call(&log, "second");
                    Ok(HookResult::BeforeRun(None))
                })
            }),
            None,
        )
        .expect("registration before close");

    let closed_gate = TestGate::new();
    closed_gate.close("closed");
    let refused = hooks
        .run_with_gate(before_run_invocation(), closed_gate, background_context())
        .await;
    assert_eq!(refused.unwrap_err().to_string(), "closed");
    assert!(calls(&log).is_empty());

    let running_hooks = hooks.clone();
    let running = tokio::spawn(async move {
        running_hooks
            .run_with_gate(
                before_run_invocation(),
                TestGate::new(),
                background_context(),
            )
            .await
    });
    wait_for(|| calls(&log) == vec!["first:start"]).await;
    hooks
        .on(
            HookName::BeforeRun,
            simple_handler(|_invocation, _context| {
                Box::pin(async {
                    Ok(HookResult::BeforeRun(None)) // late: never joined this pipeline
                })
            }),
            None,
        )
        .expect("registration before close");
    hooks.close(anyhow::anyhow!("closed"));
    let _ = sender.send(true);
    let result = running
        .await
        .expect("task joins")
        .expect("in-flight pipeline completes");
    assert!(matches!(result, HookResult::BeforeRun(None)));
    assert_eq!(calls(&log), vec!["first:start", "first:end", "second"]);
}

#[tokio::test]
async fn pre_aborted_context_rejects_before_handlers_and_admitted_hooks_join_the_gate_signal() {
    // execution-primitives.test.ts:122-156.
    let hooks = HookRegistry::new(Arc::new(|_, _, _, _| Box::pin(async {})));
    let handler_calls = counter();
    let calls = Arc::clone(&handler_calls);
    hooks
        .on(
            HookName::BeforeDrive,
            simple_handler(move |_invocation, _context| {
                let calls = Arc::clone(&calls);
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(HookResult::BeforeDrive)
                })
            }),
            None,
        )
        .expect("registration before close");
    let gate: Arc<dyn Gate> = TestGate::new();
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let aborted_context = with_abort_signal(cancelled, background_context());

    let refused = hooks
        .run_with_gate(
            before_drive_invocation(DriveOperation::Run),
            Arc::clone(&gate),
            aborted_context,
        )
        .await;
    assert_eq!(
        refused.unwrap_err().to_string(),
        "the operation was aborted"
    );
    assert_eq!(handler_calls.load(Ordering::SeqCst), 0);
    // The gate itself was never the problem: it still admits.
    assert!(gate.admit().is_ok());

    // Admitted hooks run with a live signal that follows the gate's signal.
    let admitted_hooks = HookRegistry::new(Arc::new(|_, _, _, _| Box::pin(async {})));
    let admitted_gate = TestGate::new();
    let admitted: Arc<dyn Gate> = admitted_gate.clone();
    let captured: Arc<Mutex<Option<CancellationToken>>> = Arc::new(Mutex::new(None));
    let capture = Arc::clone(&captured);
    admitted_hooks
        .on(
            HookName::BeforeDrive,
            simple_handler(move |_invocation, context| {
                let capture = Arc::clone(&capture);
                Box::pin(async move {
                    *capture.lock().unwrap() = context.abort_signal();
                    Ok(HookResult::BeforeDrive)
                })
            }),
            None,
        )
        .expect("registration before close");
    admitted_hooks
        .run_with_gate(
            before_drive_invocation(DriveOperation::Run),
            Arc::clone(&admitted),
            background_context(),
        )
        .await
        .expect("pipeline succeeds");
    let admitted_signal = captured
        .lock()
        .unwrap()
        .clone()
        .expect("handler captured its signal");
    assert!(!admitted_signal.is_cancelled());

    admitted_gate.close("gate closed");
    assert!(admitted_signal.is_cancelled());
}

#[tokio::test]
async fn run_tool_with_gate_passes_caller_context_values_through_to_handlers() {
    // execution-primitives.test.ts:158-191, telemetry half: the hook span
    // wrapper lands with the telemetry task; the value-preservation half is
    // ported here (the handler observes the caller's context values through
    // the admitted context).
    let key = create_context_key::<String>("hook.test.value");
    let hooks = HookRegistry::new(Arc::new(|_, _, _, _| Box::pin(async {})));
    let received: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&received);
    let handler_key = key.clone();
    hooks
        .on(
            HookName::BeforeTool,
            simple_handler(move |_invocation, context| {
                let slot = Arc::clone(&slot);
                let handler_key = handler_key.clone();
                Box::pin(async move {
                    *slot.lock().unwrap() = context.get(&handler_key).map(|value| (*value).clone());
                    Ok(HookResult::BeforeTool(None))
                })
            }),
            None,
        )
        .expect("registration before close");

    let context = with_context_value(&key, "preserved".to_string(), background_context());
    hooks
        .run_tool_with_gate(before_tool_invocation(), TestGate::new(), context)
        .await
        .expect("pipeline succeeds");
    assert_eq!(*received.lock().unwrap(), Some("preserved".to_string()));
}

#[tokio::test]
async fn run_tool_with_gate_rejects_non_tool_invocations() {
    // Port extra: upstream enforces the before_tool/after_tool restriction in
    // the type system; the port guards it at runtime.
    let hooks = HookRegistry::new(Arc::new(|_, _, _, _| Box::pin(async {})));
    let refused = hooks
        .run_tool_with_gate(
            before_run_invocation(),
            TestGate::new(),
            background_context(),
        )
        .await;
    assert!(refused.is_err());
}

#[tokio::test]
async fn registration_ids_are_metadata_and_before_drive_fails_closed() {
    // execution-primitives.test.ts:193-219.
    let errors = error_log();
    let hooks = HookRegistry::new(reporter(errors.clone()));
    let failure = "prerequisite failed";
    hooks
        .on(
            HookName::BeforeDrive,
            simple_handler(|_invocation, _context| {
                Box::pin(async { Err(anyhow::anyhow!("prerequisite failed")) })
            }),
            Some("duplicate".to_string()),
        )
        .expect("registration before close");
    let later_calls = counter();
    let calls = Arc::clone(&later_calls);
    hooks
        .on(
            HookName::BeforeDrive,
            simple_handler(move |_invocation, _context| {
                let calls = Arc::clone(&calls);
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(HookResult::BeforeDrive)
                })
            }),
            Some("duplicate".to_string()),
        )
        .expect("registration before close");

    let result = hooks
        .run_with_gate(
            before_drive_invocation(DriveOperation::Run),
            TestGate::new(),
            background_context(),
        )
        .await;
    assert_eq!(result.unwrap_err().to_string(), failure);
    assert_eq!(later_calls.load(Ordering::SeqCst), 0);
    assert_eq!(logged_errors(&errors), vec![failure.to_string()]);
}

#[tokio::test]
async fn transform_context_chains_output_and_isolates_handler_failures() {
    // execution-primitives.test.ts:221-254.
    let errors = error_log();
    let hooks = HookRegistry::new(reporter(errors.clone()));
    hooks
        .on(
            HookName::TransformContext,
            simple_handler(|_invocation, _context| {
                Box::pin(async move {
                    Ok(HookResult::TransformContext(Some(TransformContextPatch {
                        messages: Some(vec![user_message("transformed", 2)]),
                        system_prompt: Some("first".to_string()),
                    })))
                })
            }),
            None,
        )
        .expect("registration before close");
    let first_observation = Arc::new(Mutex::new(Vec::new()));
    let observe = Arc::clone(&first_observation);
    hooks
        .on(
            HookName::TransformContext,
            simple_handler(move |invocation, _context| {
                let observe = Arc::clone(&observe);
                Box::pin(async move {
                    match &invocation.event {
                        HookEvent::TransformContext(event) => {
                            let mut seen = observe.lock().unwrap();
                            seen.push(event.system_prompt.clone());
                            seen.extend(event.messages.iter().map(user_message_content));
                        }
                        _ => panic!("expected transform_context event"),
                    }
                    Err(anyhow::anyhow!("ignored transform failure"))
                })
            }),
            None,
        )
        .expect("registration before close");
    let second_observation = Arc::new(Mutex::new(Vec::new()));
    let observe = Arc::clone(&second_observation);
    hooks
        .on(
            HookName::TransformContext,
            simple_handler(move |invocation, _context| {
                let observe = Arc::clone(&observe);
                Box::pin(async move {
                    match &invocation.event {
                        HookEvent::TransformContext(event) => {
                            let mut seen = observe.lock().unwrap();
                            seen.push(event.system_prompt.clone());
                            seen.extend(event.messages.iter().map(user_message_content));
                        }
                        _ => panic!("expected transform_context event"),
                    }
                    Ok(HookResult::TransformContext(Some(TransformContextPatch {
                        messages: None,
                        system_prompt: Some("final".to_string()),
                    })))
                })
            }),
            None,
        )
        .expect("registration before close");

    let result = hooks
        .run_with_gate(
            transform_context_invocation(),
            TestGate::new(),
            background_context(),
        )
        .await
        .expect("pipeline succeeds");

    match result {
        HookResult::TransformContext(Some(patch)) => {
            assert_eq!(
                patch.messages.map(|messages| messages.len()),
                Some(1),
                "messages unchanged by the second/third handlers"
            );
            assert_eq!(patch.system_prompt.as_deref(), Some("final"));
        }
        other => panic!(
            "expected transform_context aggregate, got {:?}",
            other.hook_name()
        ),
    }
    // Both later handlers observed the chained output of the first.
    assert_eq!(
        *first_observation.lock().unwrap(),
        vec!["first".to_string(), "transformed".to_string()]
    );
    assert_eq!(
        *second_observation.lock().unwrap(),
        vec!["first".to_string(), "transformed".to_string()]
    );
    assert_eq!(
        logged_errors(&errors),
        vec!["ignored transform failure".to_string()]
    );
}

#[tokio::test]
async fn preserves_clear_all_before_request_patches_across_later_handlers() {
    // execution-primitives.test.ts:256-283.
    let hooks = HookRegistry::new(Arc::new(|_, _, _, _| Box::pin(async {})));
    hooks
        .on(
            HookName::BeforeRequest,
            simple_handler(|_invocation, _context| {
                Box::pin(async move {
                    Ok(HookResult::BeforeRequest(Some(BeforeRequestHookResult {
                        stream_options: AgentHarnessStreamOptionsPatch {
                            headers: Some(None),
                            metadata: Some(None),
                            ..AgentHarnessStreamOptionsPatch::default()
                        },
                    })))
                })
            }),
            None,
        )
        .expect("registration before close");
    let cleared_observation = Arc::new(Mutex::new(None));
    let observe = Arc::clone(&cleared_observation);
    hooks
        .on(
            HookName::BeforeRequest,
            simple_handler(move |invocation, _context| {
                let observe = Arc::clone(&observe);
                Box::pin(async move {
                    match &invocation.event {
                        HookEvent::BeforeRequest(event) => {
                            *observe.lock().unwrap() = Some((
                                event.stream_options.headers.clone(),
                                event.stream_options.metadata.clone(),
                            ));
                        }
                        _ => panic!("expected before_request event"),
                    }
                    Ok(HookResult::BeforeRequest(Some(BeforeRequestHookResult {
                        stream_options: AgentHarnessStreamOptionsPatch {
                            headers: Some(Some(BTreeMap::from([(
                                "c".to_string(),
                                Some("3".to_string()),
                            )]))),
                            metadata: Some(Some(BTreeMap::from([(
                                "y".to_string(),
                                Some(json!(2)),
                            )]))),
                            ..AgentHarnessStreamOptionsPatch::default()
                        },
                    })))
                })
            }),
            None,
        )
        .expect("registration before close");

    let result = hooks
        .run_with_gate(
            before_request_invocation(),
            TestGate::new(),
            background_context(),
        )
        .await
        .expect("pipeline succeeds");

    // The later handler observed the cleared options.
    assert_eq!(
        *cleared_observation.lock().unwrap(),
        Some((None, None)),
        "clear-all patches apply before later handlers run"
    );
    match result {
        HookResult::BeforeRequest(Some(result)) => {
            assert_eq!(
                result.stream_options.headers,
                Some(Some(BTreeMap::from([
                    ("a".to_string(), None),
                    ("b".to_string(), None),
                    ("c".to_string(), Some("3".to_string())),
                ]))),
                "created patch deletes dropped keys and sets the new one"
            );
            assert_eq!(
                result.stream_options.metadata,
                Some(Some(BTreeMap::from([
                    ("x".to_string(), None),
                    ("y".to_string(), Some(json!(2))),
                ]))),
            );
            assert_eq!(result.stream_options.timeout_ms, None);
        }
        other => panic!(
            "expected before_request aggregate, got {:?}",
            other.hook_name()
        ),
    }
}

#[tokio::test]
async fn preserves_earlier_after_tool_fields_when_a_later_patch_returns_undefined() {
    // execution-primitives.test.ts:285-304.
    let hooks = HookRegistry::new(Arc::new(|_, _, _, _| Box::pin(async {})));
    hooks
        .on(
            HookName::AfterTool,
            simple_handler(|_invocation, _context| {
                Box::pin(async move {
                    Ok(HookResult::AfterTool(Some(AfterToolHookResult {
                        content: Some(vec![text_block("patched")]),
                        ..AfterToolHookResult::default()
                    })))
                })
            }),
            None,
        )
        .expect("registration before close");
    hooks
        .on(
            HookName::AfterTool,
            simple_handler(|_invocation, _context| {
                Box::pin(async move {
                    Ok(HookResult::AfterTool(Some(AfterToolHookResult {
                        content: None,
                        is_error: Some(false),
                        ..AfterToolHookResult::default()
                    })))
                })
            }),
            None,
        )
        .expect("registration before close");

    let result = hooks
        .run_with_gate(
            after_tool_invocation(),
            TestGate::new(),
            background_context(),
        )
        .await
        .expect("pipeline succeeds");

    match result {
        HookResult::AfterTool(Some(result)) => {
            assert_eq!(
                result
                    .content
                    .map(|content| content.iter().map(text_block_text).collect::<Vec<_>>()),
                Some(vec!["patched".to_string()]),
            );
            assert_eq!(result.is_error, Some(false));
            assert_eq!(result.details, None);
            assert_eq!(result.usage, None);
            assert_eq!(result.terminate, None);
        }
        other => panic!("expected after_tool aggregate, got {:?}", other.hook_name()),
    }
}

#[tokio::test]
async fn accepts_explicit_false_structural_declines_and_rejects_true_conflicts() {
    // execution-primitives.test.ts:306-334.
    let errors = error_log();
    let hooks = HookRegistry::new(reporter(errors.clone()));
    let ignored = branch_summary_result("ignored");
    let selected = branch_summary_result("selected");
    let ignored_for_handler = ignored.clone();
    hooks
        .on(
            HookName::BeforeNavigation,
            simple_handler(move |_invocation, _context| {
                let ignored = ignored_for_handler.clone();
                Box::pin(async move {
                    Ok(HookResult::BeforeNavigation(Some(
                        BeforeNavigationHookResult {
                            decline: Some(true),
                            summary: Some(ignored),
                        },
                    )))
                })
            }),
            None,
        )
        .expect("registration before close");
    let selected_for_handler = selected.clone();
    hooks
        .on(
            HookName::BeforeNavigation,
            simple_handler(move |_invocation, _context| {
                let selected = selected_for_handler.clone();
                Box::pin(async move {
                    Ok(HookResult::BeforeNavigation(Some(
                        BeforeNavigationHookResult {
                            decline: Some(false),
                            summary: Some(selected),
                        },
                    )))
                })
            }),
            None,
        )
        .expect("registration before close");

    let result = hooks
        .run_with_gate(
            before_navigation_invocation(),
            TestGate::new(),
            background_context(),
        )
        .await
        .expect("pipeline succeeds");

    match result {
        HookResult::BeforeNavigation(Some(result)) => {
            assert_eq!(result.decline, Some(false));
            assert_eq!(result.summary, Some(selected));
        }
        other => panic!(
            "expected before_navigation aggregate, got {:?}",
            other.hook_name()
        ),
    }
    assert_eq!(logged_errors(&errors).len(), 1);
    assert!(
        logged_errors(&errors)[0].contains("cannot return both decline and summary"),
        "conflict message names the hook and field, got: {}",
        logged_errors(&errors)[0]
    );
}

#[tokio::test]
async fn admits_the_complete_before_drive_pipeline_as_one_gated_effect() {
    // execution-primitives.test.ts:336-366.
    let hooks = HookRegistry::new(Arc::new(|_, _, _, _| Box::pin(async {})));
    let log = calls_log();
    let (sender, receiver) = tokio::sync::watch::channel(false);
    let first_log = Arc::clone(&log);
    hooks
        .on(
            HookName::BeforeDrive,
            simple_handler(move |_invocation, _context| {
                let log = Arc::clone(&first_log);
                let receiver = receiver.clone();
                Box::pin(async move {
                    push_call(&log, "first:start");
                    while !*receiver.borrow() {
                        tokio::task::yield_now().await;
                    }
                    push_call(&log, "first:end");
                    Ok(HookResult::BeforeDrive)
                })
            }),
            None,
        )
        .expect("registration before close");
    let second_log = Arc::clone(&log);
    hooks
        .on(
            HookName::BeforeDrive,
            simple_handler(move |_invocation, _context| {
                let log = Arc::clone(&second_log);
                Box::pin(async move {
                    push_call(&log, "second");
                    Ok(HookResult::BeforeDrive)
                })
            }),
            None,
        )
        .expect("registration before close");

    // Abort committed before admission: the run is refused without starting.
    let abort_first_gate = TestGate::new();
    abort_first_gate.begin_abort();
    let refused_gate: Arc<dyn Gate> = abort_first_gate.clone();
    let refused = hooks
        .run_with_gate(
            before_drive_invocation(DriveOperation::Run),
            refused_gate,
            background_context(),
        )
        .await;
    assert_eq!(refused.unwrap_err().to_string(), "abort requested");
    assert!(calls(&log).is_empty());

    // Abort signalled mid-flight: the admitted pipeline runs to completion.
    let start_first_gate = TestGate::new();
    let running_gate: Arc<dyn Gate> = start_first_gate.clone();
    let running_hooks = hooks.clone();
    let running = tokio::spawn(async move {
        running_hooks
            .run_with_gate(
                before_drive_invocation(DriveOperation::Run),
                running_gate,
                background_context(),
            )
            .await
    });
    wait_for(|| calls(&log) == vec!["first:start"]).await;
    start_first_gate.begin_abort();
    start_first_gate.signal_abort();
    assert!(start_first_gate.signal().is_cancelled());
    let _ = sender.send(true);
    let result = running
        .await
        .expect("task joins")
        .expect("admitted pipeline completes");
    assert!(matches!(result, HookResult::BeforeDrive));
    assert_eq!(calls(&log), vec!["first:start", "first:end", "second"]);
}

// ---------------------------------------------------------------------------
// Registry surface (on/has/close/unsubscribe) and aggregate paths outside the
// oracle block
// ---------------------------------------------------------------------------

#[tokio::test]
async fn has_reports_liveness_and_unsubscribe_removes_the_registration() {
    let hooks = HookRegistry::new(Arc::new(|_, _, _, _| Box::pin(async {})));
    assert!(!hooks.has(HookName::BeforeRun));
    let unsubscribe = hooks
        .on(
            HookName::BeforeRun,
            simple_handler(|_i, _c| Box::pin(async { Ok(HookResult::BeforeRun(None)) })),
            None,
        )
        .expect("registration before close");
    assert!(hooks.has(HookName::BeforeRun));
    unsubscribe.unsubscribe();
    // Idempotent (upstream `indexOf !== -1` guard).
    unsubscribe.unsubscribe();
    assert!(!hooks.has(HookName::BeforeRun));
}

#[tokio::test]
async fn on_fails_with_the_first_close_error_and_close_keeps_the_first_error() {
    let hooks = HookRegistry::new(Arc::new(|_, _, _, _| Box::pin(async {})));
    hooks.close(anyhow::anyhow!("first close"));
    hooks.close(anyhow::anyhow!("second close"));
    let refused = hooks.on(
        HookName::BeforeRun,
        simple_handler(|_i, _c| Box::pin(async { Ok(HookResult::BeforeRun(None)) })),
        None,
    );
    assert_eq!(refused.unwrap_err().to_string(), "first close");
    // A closed registry refuses runs even without registrations.
    let run = hooks
        .run_with_gate(
            before_run_invocation(),
            TestGate::new(),
            background_context(),
        )
        .await;
    assert_eq!(run.unwrap_err().to_string(), "first close");
}

#[tokio::test]
async fn before_run_end_uses_the_last_follow_up_and_ignores_empty_results() {
    let errors = error_log();
    let hooks = HookRegistry::new(reporter(errors.clone()));
    hooks
        .on(
            HookName::BeforeRunEnd,
            simple_handler(|_i, _c| {
                Box::pin(async {
                    Ok(HookResult::BeforeRunEnd(Some(FollowUpHookResult {
                        follow_up: Some("first".to_string()),
                    })))
                })
            }),
            None,
        )
        .expect("registration before close");
    hooks
        .on(
            HookName::BeforeRunEnd,
            simple_handler(|_i, _c| Box::pin(async { Ok(HookResult::BeforeRunEnd(None)) })),
            None,
        )
        .expect("registration before close");
    hooks
        .on(
            HookName::BeforeRunEnd,
            simple_handler(|_i, _c| {
                Box::pin(async {
                    Ok(HookResult::BeforeRunEnd(Some(FollowUpHookResult {
                        follow_up: Some("second".to_string()),
                    })))
                })
            }),
            None,
        )
        .expect("registration before close");

    let result = hooks
        .run_with_gate(
            HookInvocation {
                lane: "main".to_string(),
                run_id: "run".to_string(),
                event: HookEvent::BeforeRunEnd(BeforeRunEndEvent {
                    messages: vec![user_message("done", 1)],
                }),
            },
            TestGate::new(),
            background_context(),
        )
        .await
        .expect("pipeline succeeds");
    match result {
        HookResult::BeforeRunEnd(Some(result)) => {
            assert_eq!(result.follow_up.as_deref(), Some("second"));
        }
        other => panic!(
            "expected before_run_end aggregate, got {:?}",
            other.hook_name()
        ),
    }
    assert!(logged_errors(&errors).is_empty());
}

#[tokio::test]
async fn before_payload_and_after_response_pass_the_current_value_through() {
    let hooks = HookRegistry::new(Arc::new(|_, _, _, _| Box::pin(async {})));
    hooks
        .on(
            HookName::BeforePayload,
            simple_handler(|_i, _c| {
                Box::pin(async {
                    Ok(HookResult::BeforePayload(Some(PayloadHookResult {
                        payload: json!({"patched": true}),
                    })))
                })
            }),
            None,
        )
        .expect("registration before close");
    hooks
        .on(
            HookName::AfterResponse,
            simple_handler(|_i, _c| {
                Box::pin(async {
                    Ok(HookResult::AfterResponse(Some(MessageHookResult {
                        message: settled_message("replacement"),
                    })))
                })
            }),
            None,
        )
        .expect("registration before close");

    let payload_result = hooks
        .run_with_gate(
            HookInvocation {
                lane: "main".to_string(),
                run_id: "run".to_string(),
                event: HookEvent::BeforePayload(BeforePayloadEvent {
                    model: test_model(),
                    payload: json!({"original": true}),
                }),
            },
            TestGate::new(),
            background_context(),
        )
        .await
        .expect("payload pipeline succeeds");
    match payload_result {
        HookResult::BeforePayload(Some(result)) => {
            assert_eq!(result.payload, json!({"patched": true}));
        }
        other => panic!(
            "expected before_payload aggregate, got {:?}",
            other.hook_name()
        ),
    }

    let response_result = hooks
        .run_with_gate(
            HookInvocation {
                lane: "main".to_string(),
                run_id: "run".to_string(),
                event: HookEvent::AfterResponse(AfterResponseEvent {
                    status: Some(200),
                    headers: None,
                    message: settled_message("original"),
                }),
            },
            TestGate::new(),
            background_context(),
        )
        .await
        .expect("response pipeline succeeds");
    match response_result {
        HookResult::AfterResponse(Some(result)) => {
            let texts: Vec<String> = result
                .message
                .content
                .iter()
                .map(|block| match block {
                    crate::ai::types::message::AssistantBlock::Text(content) => {
                        content.text.clone()
                    }
                    _ => panic!("expected text block"),
                })
                .collect();
            assert_eq!(texts, vec!["replacement".to_string()]);
        }
        other => panic!(
            "expected after_response aggregate, got {:?}",
            other.hook_name()
        ),
    }
}

#[tokio::test]
async fn before_compaction_takes_the_first_admitted_result() {
    // hooks.ts:343-357 (`firstStructural`): the first handler returning a
    // decline or a result wins; later handlers are skipped. A result without
    // `decline` is admitted as-is (upstream admits any object carrying the
    // result field); returning both decline and result is reported and
    // skipped (covered by the before_navigation oracle test).
    let errors = error_log();
    let hooks = HookRegistry::new(reporter(errors.clone()));
    let plain = compaction_result("plain");
    hooks
        .on(
            HookName::BeforeCompaction,
            simple_handler(move |_i, _c| {
                let plain = plain.clone();
                Box::pin(async move {
                    Ok(HookResult::BeforeCompaction(Some(
                        BeforeCompactionHookResult {
                            decline: None,
                            compaction: Some(plain),
                        },
                    )))
                })
            }),
            None,
        )
        .expect("registration before close");
    let later_calls = counter();
    let calls = Arc::clone(&later_calls);
    hooks
        .on(
            HookName::BeforeCompaction,
            simple_handler(move |_i, _c| {
                let calls = Arc::clone(&calls);
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(HookResult::BeforeCompaction(Some(
                        BeforeCompactionHookResult {
                            decline: Some(true),
                            compaction: None,
                        },
                    )))
                })
            }),
            None,
        )
        .expect("registration before close");

    let result = hooks
        .run_with_gate(
            HookInvocation {
                lane: "main".to_string(),
                run_id: "run".to_string(),
                event: HookEvent::BeforeCompaction(BeforeCompactionEvent {
                    reason: CompactionReason::Threshold,
                    preparation: CompactionPreparation {
                        messages_to_summarize: vec![],
                        turn_prefix_messages: vec![],
                        retained_tail: vec![],
                        is_split_turn: false,
                        tokens_before: 0,
                        previous_summary: None,
                        file_ops: FileOperations::new(),
                        settings: DEFAULT_COMPACTION_SETTINGS,
                    },
                    custom_instructions: None,
                }),
            },
            TestGate::new(),
            background_context(),
        )
        .await
        .expect("pipeline succeeds");
    match result {
        HookResult::BeforeCompaction(Some(result)) => {
            assert_eq!(result.decline, None);
            assert_eq!(result.compaction, Some(compaction_result("plain")));
        }
        other => panic!(
            "expected before_compaction aggregate, got {:?}",
            other.hook_name()
        ),
    }
    assert_eq!(later_calls.load(Ordering::SeqCst), 0);
    assert!(logged_errors(&errors).is_empty());
}

#[tokio::test]
async fn before_tool_failures_report_and_block_with_the_error_message() {
    // hooks.ts:175-181: a before_tool handler failure reports and blocks with
    // the normalized message.
    let errors = error_log();
    let hooks = HookRegistry::new(reporter(errors.clone()));
    hooks
        .on(
            HookName::BeforeTool,
            simple_handler(|_i, _c| Box::pin(async { Err(anyhow::anyhow!("validation blew up")) })),
            None,
        )
        .expect("registration before close");
    let later_calls = counter();
    let calls = Arc::clone(&later_calls);
    hooks
        .on(
            HookName::BeforeTool,
            simple_handler(move |_i, _c| {
                let calls = Arc::clone(&calls);
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(HookResult::BeforeTool(None))
                })
            }),
            None,
        )
        .expect("registration before close");

    let result = hooks
        .run_tool_with_gate(
            before_tool_invocation(),
            TestGate::new(),
            background_context(),
        )
        .await
        .expect("aggregate resolves (failure is reported, not propagated)");
    match result {
        HookResult::BeforeTool(Some(result)) => {
            assert_eq!(result.args, None);
            let block = result.block.expect("failure blocks the tool");
            assert_eq!(block.reason, "validation blew up");
            assert_eq!(block.terminate, None);
        }
        other => panic!(
            "expected before_tool aggregate, got {:?}",
            other.hook_name()
        ),
    }
    assert_eq!(later_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        logged_errors(&errors),
        vec!["validation blew up".to_string()]
    );
}

#[tokio::test]
async fn mismatched_handler_results_are_a_port_bug_not_a_silent_cast() {
    // Upstream casts handler results blindly (`result as HookMap[TName]`);
    // the port surfaces a variant/name mismatch as an error.
    let hooks = HookRegistry::new(Arc::new(|_, _, _, _| Box::pin(async {})));
    hooks
        .on(
            HookName::BeforeRun,
            simple_handler(|_i, _c| Box::pin(async { Ok(HookResult::BeforeDrive) })),
            None,
        )
        .expect("registration before close");
    let result = hooks
        .run_with_gate(
            before_run_invocation(),
            TestGate::new(),
            background_context(),
        )
        .await;
    assert!(result.is_err(), "mismatched variant must surface");
}

// ---------------------------------------------------------------------------
// Stream-options patch functions (hooks.ts:446-533)
// ---------------------------------------------------------------------------

#[test]
fn apply_stream_options_patch_merges_partial_patches_without_touching_other_fields() {
    // hooks.ts:446-487.
    let base = AgentHarnessStreamOptions {
        transport: Some(crate::ai::types::primitives::Transport::Auto),
        timeout_ms: Some(1000),
        headers: Some(BTreeMap::from([
            ("keep".to_string(), "1".to_string()),
            ("drop".to_string(), "2".to_string()),
        ])),
        metadata: Some(BTreeMap::from([("kind".to_string(), json!("run"))])),
        ..AgentHarnessStreamOptions::default()
    };
    let next = apply_stream_options_patch(
        base.clone(),
        &AgentHarnessStreamOptionsPatch {
            timeout_ms: Some(Some(5000)),
            headers: Some(Some(BTreeMap::from([
                ("drop".to_string(), None),
                ("add".to_string(), Some("3".to_string())),
            ]))),
            ..AgentHarnessStreamOptionsPatch::default()
        },
    );
    assert_eq!(next.transport, base.transport, "absent field untouched");
    assert_eq!(next.timeout_ms, Some(5000));
    assert_eq!(
        next.headers,
        Some(BTreeMap::from([
            ("keep".to_string(), "1".to_string()),
            ("add".to_string(), "3".to_string()),
        ])),
    );
    assert_eq!(next.metadata, base.metadata);
}

#[test]
fn apply_stream_options_patch_deletes_scalars_and_clears_maps() {
    // hooks.ts:455-475: explicit `undefined` deletes.
    let base = AgentHarnessStreamOptions {
        transport: Some(crate::ai::types::primitives::Transport::Auto),
        timeout_ms: Some(1000),
        headers: Some(BTreeMap::from([("a".to_string(), "1".to_string())])),
        metadata: Some(BTreeMap::from([("x".to_string(), json!(1))])),
        ..AgentHarnessStreamOptions::default()
    };
    let next = apply_stream_options_patch(
        base,
        &AgentHarnessStreamOptionsPatch {
            transport: Some(None),
            headers: Some(None),
            ..AgentHarnessStreamOptionsPatch::default()
        },
    );
    assert_eq!(next.transport, None, "scalar deletion");
    assert_eq!(next.timeout_ms, Some(1000), "absent field untouched");
    assert_eq!(next.headers, None, "explicit clear-all");
    // An empty merge map over a present base map keeps the base entries
    // (upstream `{...next.metadata}` then merging nothing).
    assert_eq!(
        next.metadata,
        Some(BTreeMap::from([("x".to_string(), json!(1))]))
    );
}

#[test]
fn apply_stream_options_patch_merges_empty_maps_like_upstream_object_spreads() {
    // hooks.ts:466-474: `{...next.headers}` from undefined yields a defined
    // empty map; over a present map it keeps the entries.
    let baseless = AgentHarnessStreamOptions::default();
    let next = apply_stream_options_patch(
        baseless,
        &AgentHarnessStreamOptionsPatch {
            headers: Some(Some(BTreeMap::new())),
            ..AgentHarnessStreamOptionsPatch::default()
        },
    );
    assert_eq!(next.headers, Some(BTreeMap::new()));
}

#[test]
fn create_stream_options_patch_describes_the_base_to_value_diff() {
    // hooks.ts:489-533. Round trip: apply(base, create(base, value)) == value.
    let base = AgentHarnessStreamOptions {
        timeout_ms: Some(1000),
        max_retries: Some(2),
        headers: Some(BTreeMap::from([
            ("drop".to_string(), "1".to_string()),
            ("same".to_string(), "2".to_string()),
        ])),
        metadata: Some(BTreeMap::from([("x".to_string(), json!(1))])),
        ..AgentHarnessStreamOptions::default()
    };
    let value = AgentHarnessStreamOptions {
        timeout_ms: Some(2000),
        headers: Some(BTreeMap::from([("same".to_string(), "2".to_string())])),
        metadata: Some(BTreeMap::from([("y".to_string(), json!(2))])),
        ..AgentHarnessStreamOptions::default()
    };
    let patch = create_stream_options_patch(&base, &value);
    assert_eq!(patch.timeout_ms, Some(Some(2000)));
    assert_eq!(
        patch.max_retries,
        Some(None),
        "scalar deletion is expressible"
    );
    assert_eq!(
        patch.headers,
        Some(Some(BTreeMap::from([("drop".to_string(), None),]))),
        "same-valued keys are omitted, dropped keys delete"
    );
    assert_eq!(
        patch.metadata,
        Some(Some(BTreeMap::from([
            ("x".to_string(), None),
            ("y".to_string(), Some(json!(2))),
        ]))),
    );
    let round_tripped = apply_stream_options_patch(base, &patch);
    assert_eq!(round_tripped, value);
}

#[test]
fn create_stream_options_patch_expresses_clear_all_and_baseless_headers() {
    // hooks.ts:504-517: value.headers === undefined emits the explicit clear;
    // headers appearing where the base had none emits at least an empty map
    // when the diff is empty (upstream `patch.headers = {}`).
    let base = AgentHarnessStreamOptions {
        headers: Some(BTreeMap::from([("a".to_string(), "1".to_string())])),
        ..AgentHarnessStreamOptions::default()
    };
    let cleared = AgentHarnessStreamOptions::default();
    let patch = create_stream_options_patch(&base, &cleared);
    assert_eq!(patch.headers, Some(None));
    assert_eq!(apply_stream_options_patch(base, &patch), cleared);

    let no_base = AgentHarnessStreamOptions::default();
    let empty_headers = AgentHarnessStreamOptions {
        headers: Some(BTreeMap::new()),
        ..AgentHarnessStreamOptions::default()
    };
    let patch = create_stream_options_patch(&no_base, &empty_headers);
    assert_eq!(patch.headers, Some(Some(BTreeMap::new())));
    assert_eq!(apply_stream_options_patch(no_base, &patch), empty_headers);
}

#[test]
fn hook_names_serialize_to_upstream_literals() {
    // telemetry.ts HOOK_NAMES values.
    assert_eq!(
        serde_json::to_value(HookName::BeforeRun).unwrap(),
        "before_run"
    );
    assert_eq!(
        serde_json::to_value(HookName::BeforeDrive).unwrap(),
        "before_drive"
    );
    assert_eq!(
        serde_json::to_value(HookName::BeforeRunEnd).unwrap(),
        "before_run_end"
    );
    assert_eq!(
        serde_json::to_value(HookName::TransformContext).unwrap(),
        "transform_context"
    );
    assert_eq!(
        serde_json::to_value(HookName::BeforeRequest).unwrap(),
        "before_request"
    );
    assert_eq!(
        serde_json::to_value(HookName::BeforePayload).unwrap(),
        "before_payload"
    );
    assert_eq!(
        serde_json::to_value(HookName::AfterResponse).unwrap(),
        "after_response"
    );
    assert_eq!(
        serde_json::to_value(HookName::BeforeTool).unwrap(),
        "before_tool"
    );
    assert_eq!(
        serde_json::to_value(HookName::AfterTool).unwrap(),
        "after_tool"
    );
    assert_eq!(
        serde_json::to_value(HookName::BeforeCompaction).unwrap(),
        "before_compaction"
    );
    assert_eq!(
        serde_json::to_value(HookName::BeforeNavigation).unwrap(),
        "before_navigation"
    );
}
