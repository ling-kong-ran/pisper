//! Port of `packages/agent/test/harness/execution-tools.test.ts` (the
//! executable spec for the tool-phase primitives). Disclosed port deltas vs
//! the oracle, both following the established infallible-closure precedent:
//! - "preparation throws" is unrepresentable (`prepareArguments` is an
//!   infallible closure in the port, per the agent-loop port), so the
//!   immediate-error oracle covers the unknown-tool and invalid-arguments
//!   branches.
//! - "synchronous vs asynchronous tool throws" is one JS split; the port
//!   exercises both shapes of the same `Err` return.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::*;
use crate::agent_core::harness::context::background_context;
use crate::agent_core::harness::execution::effect_gate::create_gate;
use crate::agent_core::harness::types::AgentHarnessTool;
use crate::ai::types::content::ToolCall;
use crate::ai::types::message::TextOrImageBlock;
use serde_json::json;

/// The oracle's `parameters` fixture (`execution-tools.test.ts:16`).
const PARAMETERS: &str =
    r#"{"type":"object","properties":{"value":{"type":"string"}},"required":["value"]}"#;

/// The oracle's `call()` fixture (`execution-tools.test.ts:18-21`).
fn call(arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "call-1".into(),
        name: "echo".into(),
        arguments,
        thought_signature: None,
        namespace: None,
    }
}

fn text_block(text: &str) -> TextOrImageBlock {
    TextOrImageBlock::Text(crate::ai::types::TextContent {
        text: text.into(),
        text_signature: None,
    })
}

type TestExecute = Arc<
    dyn Fn(
            String,
            serde_json::Value,
            Arc<AgentHarnessToolUpdateCallback>,
        ) -> futures::future::BoxFuture<'static, anyhow::Result<AgentToolResult>>
        + Send
        + Sync,
>;

/// The oracle's `tool()` fixture (`execution-tools.test.ts:23-38`), with
/// optional prepare/execute overrides.
fn tool_with(
    prepare: Option<Arc<crate::agent_core::types::PrepareArgumentsFn>>,
    execute: Option<TestExecute>,
) -> AgentHarnessTool<()> {
    let default_execute: TestExecute = Arc::new(|_tool_call_id, args, _on_update| {
        Box::pin(async move {
            let value = args["value"].as_str().unwrap_or("").to_string();
            Ok(AgentToolResult {
                content: vec![text_block(&value)],
                details: Some(json!({ "value": value })),
                usage: None,
                terminate: None,
            })
        })
    });
    // Wrap the 3-argument test core into the full harness executor signature.
    let core = execute.unwrap_or(default_execute);
    let execute: Arc<crate::agent_core::harness::types::HarnessExecuteFn<()>> = Arc::new(
        move |tool_call_id, args, on_update, _tool_context, _invocation, _context| {
            core(tool_call_id, args, on_update)
        },
    );
    AgentHarnessTool {
        name: "echo".into(),
        label: "Echo".into(),
        description: "Echo input".into(),
        parameters: serde_json::from_str(PARAMETERS).unwrap(),
        constrained_sampling: None,
        execute,
        prepare_arguments: prepare,
        replay: None,
        execution_mode: None,
    }
}

/// The oracle's `text()` helper (`execution-tools.test.ts:40-42`).
fn text(result: &AgentToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            TextOrImageBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The oracle's `clearPrepared` helper (`execution-tools.test.ts:50-55`).
fn clear_prepared(outcome: PrepareOutcome<()>) -> ClearedToolCall<()> {
    let prepared = match outcome {
        PrepareOutcome::Prepared(prepared) => prepared,
        PrepareOutcome::Immediate(immediate) => {
            panic!(
                "expected prepared call, got immediate: {}",
                text(&immediate.result)
            )
        }
    };
    match apply_before_tool_decision(prepared, None) {
        Ok(cleared) => cleared,
        Err(immediate) => panic!("expected cleared call: {}", text(&immediate.result)),
    }
}

fn noop_update() -> Arc<AgentHarnessToolUpdateCallback> {
    Arc::new(|_, _| {})
}

/// The oracle's `invocation` fixture (`execution-tools.test.ts:61-67`).
struct TestInvocation;

impl AgentHarnessToolInvocation for TestInvocation {
    fn invocation_id(&self) -> &str {
        "result-1"
    }
    fn operation_id(&self) -> &str {
        "operation-1"
    }
    fn turn_id(&self) -> &str {
        "turn-1"
    }
    fn get_memo<'a>(
        &'a self,
        _name: &'a str,
    ) -> futures::future::BoxFuture<'a, Option<serde_json::Value>> {
        Box::pin(async { None })
    }
    fn set_memo<'a>(
        &'a self,
        _name: &'a str,
        _value: Option<serde_json::Value>,
    ) -> futures::future::BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}

/// Oracle "prepares arguments before validation and preserves the provider
/// call" (`execution-tools.test.ts:70-85`).
#[test]
fn prepares_arguments_before_validation_and_preserves_the_provider_call() {
    let provider_call = call(json!({ "legacy": "prepared" }));
    let original_arguments = provider_call.arguments.clone();
    let outcome = prepare_tool_call(
        provider_call.clone(),
        &[tool_with(
            Some(Arc::new(|args| json!({ "value": args["legacy"] }))),
            None,
        )],
    );
    let PrepareOutcome::Prepared(prepared) = outcome else {
        panic!("expected prepared");
    };
    assert_eq!(prepared.tool_call, provider_call);
    assert_eq!(prepared.tool_call.arguments, original_arguments);
    assert_eq!(prepared.args, json!({ "value": "prepared" }));
}

/// Oracle "returns immediate errors for unknown tools, preparation throws,
/// and invalid arguments" (`execution-tools.test.ts:87-102`; the preparation
/// branch is disclosed in the module docs).
#[test]
fn returns_immediate_errors_for_unknown_tools_and_invalid_arguments() {
    let unknown = prepare_tool_call::<()>(call(json!({ "value": "input" })), &[]);
    let invalid = prepare_tool_call(call(json!({})), &[tool_with(None, None)]);

    let PrepareOutcome::Immediate(unknown) = unknown else {
        panic!("expected immediate");
    };
    assert_eq!(text(&unknown.result), r#"Tool "echo" is unavailable"#);
    assert_eq!(unknown.result.details, None);
    assert!(unknown.is_error);

    let PrepareOutcome::Immediate(invalid) = invalid else {
        panic!("expected immediate");
    };
    assert!(
        text(&invalid.result).contains(r#"Validation failed for tool "echo""#),
        "{}",
        text(&invalid.result)
    );
}

/// Oracle "blocks calls and revalidates replacement arguments"
/// (`execution-tools.test.ts:104-118`).
#[test]
fn blocks_calls_and_revalidates_replacement_arguments() {
    let outcome = prepare_tool_call(call(json!({ "value": "input" })), &[tool_with(None, None)]);
    let prepared = match outcome {
        PrepareOutcome::Prepared(prepared) => prepared,
        PrepareOutcome::Immediate(immediate) => panic!("expected prepared: {immediate:?}"),
    };

    let blocked = apply_before_tool_decision(
        prepared.clone(),
        Some(&BeforeToolDecision {
            args: None,
            block: Some(hooks::ToolBlock {
                reason: "denied".into(),
                terminate: Some(true),
            }),
        }),
    )
    .expect_err("blocked");
    assert!(blocked.is_error);
    assert!(blocked.terminate);
    assert_eq!(text(&blocked.result), "denied");

    let replaced = apply_before_tool_decision(
        prepared.clone(),
        Some(&BeforeToolDecision {
            args: Some(json!({ "value": "replacement" })),
            block: None,
        }),
    )
    .expect("replaced");
    assert_eq!(replaced.args, json!({ "value": "replacement" }));

    let invalid = apply_before_tool_decision(
        prepared,
        Some(&BeforeToolDecision {
            args: Some(json!({})),
            block: None,
        }),
    );
    assert!(invalid.is_err());
}

/// Oracle "executes with updates, passes the signal, and ignores late
/// updates" (`execution-tools.test.ts:120-149`). The `context.abortSignal ===
/// gate.signal` identity assert becomes a behavior assert: cancelling the
/// gate signal cancels the admitted context's (linked) signal.
#[tokio::test]
async fn executes_with_updates_passes_the_signal_and_ignores_late_updates() {
    let (gate, _control) = create_gate();
    let late_update: Arc<Mutex<Option<Arc<AgentHarnessToolUpdateCallback>>>> =
        Arc::new(Mutex::new(None));
    let updates: Arc<Mutex<Vec<AgentToolResult>>> = Arc::new(Mutex::new(Vec::new()));
    let updates_writer = Arc::clone(&updates);
    let on_update: Arc<AgentHarnessToolUpdateCallback> = Arc::new(move |partial, _options| {
        updates_writer.lock().unwrap().push(partial.clone());
    });

    let late_writer = Arc::clone(&late_update);
    let gate_signal = gate.signal();
    let mut tool = tool_with(None, None);
    tool.execute = Arc::new(
        move |_tool_call_id,
              args,
              on_update: Arc<AgentHarnessToolUpdateCallback>,
              _tool_context,
              _invocation,
              context: Context| {
            let late_writer = Arc::clone(&late_writer);
            let gate_signal = gate_signal.clone();
            let value = args["value"].as_str().unwrap_or("").to_string();
            Box::pin(async move {
                // `expect(context.abortSignal).toBe(gate.signal)` — the
                // admitted context carries a signal linked to the gate's.
                let context_signal = context
                    .abort_signal()
                    .expect("admitted context carries a signal");
                assert!(!context_signal.is_cancelled());
                assert!(!gate_signal.is_cancelled());
                gate_signal.cancel();
                context_signal.cancelled().await;
                assert!(context_signal.is_cancelled());

                on_update(
                    &AgentToolResult {
                        content: vec![text_block("partial")],
                        details: Some(json!({ "value": value })),
                        usage: None,
                        terminate: None,
                    },
                    AgentHarnessToolUpdateOptions::default(),
                );
                // The wrapper handed to the tool is what a late update would
                // flow through; capture it.
                *late_writer.lock().unwrap() = Some(on_update);
                Ok(AgentToolResult {
                    content: vec![text_block("done")],
                    details: Some(json!({ "value": value })),
                    usage: None,
                    terminate: None,
                })
            })
        },
    );

    let cleared = clear_prepared(prepare_tool_call(
        call(json!({ "value": "input" })),
        &[tool],
    ));
    let runner = execute_tool_call(
        cleared,
        &gate,
        on_update,
        (),
        Arc::new(TestInvocation),
        background_context(),
    )
    .expect("admitted");
    let result = runner.await;
    if let Some(late) = late_update.lock().unwrap().as_ref() {
        late(
            &AgentToolResult {
                content: vec![text_block("late")],
                details: Some(json!({ "value": "late" })),
                usage: None,
                terminate: None,
            },
            AgentHarnessToolUpdateOptions::default(),
        );
    }

    assert!(!result.is_error);
    assert_eq!(text(&result.result), "done");
    let updates = updates.lock().unwrap();
    assert_eq!(updates.len(), 1, "late updates are ignored");
    assert_eq!(text(&updates[0]), "partial");
}

/// Oracle "converts synchronous/asynchronous tool throws to error output"
/// (`execution-tools.test.ts:151-171`) — both are the same `Err` return in
/// the port; two shapes are exercised.
#[tokio::test]
async fn converts_tool_throws_to_error_output() {
    for shape in ["sync", "async"] {
        let (gate, _control) = create_gate();
        let tool = tool_with(
            None,
            Some(Arc::new(move |_id, _args, _on_update| {
                let sync = shape == "sync";
                Box::pin(async move {
                    if sync {
                        Err(anyhow::anyhow!("tool failed"))
                    } else {
                        tokio::task::yield_now().await;
                        Err(anyhow::anyhow!("tool failed"))
                    }
                })
            })),
        );
        let cleared = clear_prepared(prepare_tool_call(
            call(json!({ "value": "input" })),
            &[tool],
        ));
        let runner = execute_tool_call(
            cleared,
            &gate,
            noop_update(),
            (),
            Arc::new(TestInvocation),
            background_context(),
        )
        .expect("admitted");
        let result = runner.await;
        assert!(result.is_error);
        assert_eq!(text(&result.result), "tool failed");
    }
}

/// Oracle "lets abort-first gate refusal escape without invoking the tool"
/// (`execution-tools.test.ts:173-183`).
#[tokio::test]
async fn lets_abort_first_gate_refusal_escape_without_invoking_the_tool() {
    let (gate, control) = create_gate();
    control.begin_abort(tokio_util::sync::CancellationToken::new());
    let executed = Arc::new(AtomicBool::new(false));
    let executed_writer = Arc::clone(&executed);
    let tool = tool_with(
        None,
        Some(Arc::new(move |_id, _args, _on_update| {
            let executed_writer = Arc::clone(&executed_writer);
            Box::pin(async move {
                executed_writer.store(true, Ordering::SeqCst);
                Ok(AgentToolResult::default())
            })
        })),
    );
    let cleared = clear_prepared(prepare_tool_call(
        call(json!({ "value": "input" })),
        &[tool],
    ));
    let outcome = execute_tool_call(
        cleared,
        &gate,
        noop_update(),
        (),
        Arc::new(TestInvocation),
        background_context(),
    );
    assert!(matches!(outcome, Err(GateRejection::AbortRequested(_))));
    assert!(!executed.load(Ordering::SeqCst));
}

/// Oracle "applies patches field by field and constructs the tool-result
/// message" (`execution-tools.test.ts:185-233`).
#[test]
fn applies_patches_field_by_field_and_constructs_the_tool_result_message() {
    let cleared = clear_prepared(prepare_tool_call(
        call(json!({ "value": "input" })),
        &[tool_with(None, None)],
    ));
    let original_usage = crate::ai::types::primitives::Usage {
        input: 1,
        output: 2,
        cache_read: 0,
        cache_write: 0,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: 3,
        cost: Default::default(),
    };
    let replacement_usage = crate::ai::types::primitives::Usage {
        input: 5,
        total_tokens: 7,
        ..original_usage
    };
    let executed = ExecutedToolCall {
        result: AgentToolResult {
            content: vec![text_block("original")],
            details: Some(json!({ "original": true })),
            usage: Some(original_usage),
            terminate: None,
        },
        is_error: true,
    };

    let finalized = finalize_tool_call(
        &cleared,
        executed,
        Some(&AfterToolPatch {
            content: Some(vec![text_block("patched")]),
            details: Some(json!({ "patched": true })),
            usage: Some(replacement_usage),
            is_error: Some(false),
            terminate: Some(true),
        }),
    );
    let before = crate::ai::now_ms();
    let message = create_tool_result_message(&finalized);

    assert!(!finalized.is_error);
    assert!(finalized.terminate);
    assert_eq!(finalized.result.content, vec![text_block("patched")]);
    assert_eq!(finalized.result.details, Some(json!({ "patched": true })));
    assert_eq!(finalized.result.usage, Some(replacement_usage));
    assert_eq!(finalized.result.terminate, Some(true));
    assert_eq!(message.tool_call_id, "call-1");
    assert_eq!(message.tool_name, "echo");
    assert_eq!(message.content, vec![text_block("patched")]);
    assert_eq!(message.details, Some(json!({ "patched": true })));
    assert_eq!(message.usage, Some(replacement_usage));
    assert!(!message.is_error);
    assert!(message.timestamp >= before);
}

/// Oracle "preserves unusual JSON object keys in tool-result details"
/// (`execution-tools.test.ts:235-243`).
#[test]
fn preserves_unusual_json_object_keys_in_tool_result_details() {
    let cleared = clear_prepared(prepare_tool_call(
        call(json!({ "value": "input" })),
        &[tool_with(None, None)],
    ));
    let details: serde_json::Value =
        serde_json::from_str(r#"{"__proto__":{"preserved":true}}"#).unwrap();
    let finalized = finalize_tool_call(
        &cleared,
        ExecutedToolCall {
            result: AgentToolResult {
                content: Vec::new(),
                details: Some(details.clone()),
                usage: None,
                terminate: None,
            },
            is_error: false,
        },
        None,
    );
    let message = create_tool_result_message(&finalized);
    assert_eq!(message.details, Some(details));
}

/// Oracle "normalizes missing content from untyped tools"
/// (`execution-tools.test.ts:245-254`). Upstream normalizes `content:
/// undefined` to `[]`; the port's `AgentToolResult.content` is always a
/// `Vec`, so the assertion pins the same output shape for empty content.
#[test]
fn normalizes_missing_content_from_untyped_tools() {
    let cleared = clear_prepared(prepare_tool_call(
        call(json!({ "value": "input" })),
        &[tool_with(None, None)],
    ));
    let finalized = finalize_tool_call(
        &cleared,
        ExecutedToolCall {
            result: AgentToolResult {
                content: Vec::new(),
                details: Some(json!({})),
                usage: None,
                terminate: None,
            },
            is_error: false,
        },
        None,
    );
    assert!(create_tool_result_message(&finalized).content.is_empty());
}

/// The reconstruction helper (`tools.ts:184-191`), not covered by a
/// dedicated oracle case upstream — pinned here.
#[test]
fn tool_result_from_message_reconstructs_the_canonical_result() {
    let message = crate::ai::types::message::ToolResultMessage {
        tool_call_id: "call-1".into(),
        tool_name: "echo".into(),
        content: vec![text_block("done")],
        details: Some(json!({ "value": "done" })),
        usage: Some(crate::ai::types::primitives::Usage::default()),
        is_error: true,
        timestamp: 1,
    };
    let result = tool_result_from_message(&message, true);
    assert_eq!(result.content, vec![text_block("done")]);
    assert_eq!(result.details, Some(json!({ "value": "done" })));
    assert_eq!(result.terminate, Some(true));
    let result = tool_result_from_message(&message, false);
    assert_eq!(result.terminate, None);
}
