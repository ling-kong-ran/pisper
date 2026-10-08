//! Behavior tests for [`reconcile_operation`], ported from
//! `pi/packages/agent/test/harness/runtime/drive-reconcile.test.ts` (747
//! lines) as the behavior authority.
//!
//! Ported substitutions (disclosed):
//! - The upstream suite drives every case through `driveOperation`, the
//!   unported lane-bound dispatcher (`drive/mod.rs`: "The Lane-bound
//!   dispatcher is not implemented"). The ported cases loop
//!   [`reconcile_operation`] until it settles — the reconcile loop is the
//!   only behavior `driveOperation` adds for cancelled operations: it
//!   dispatches straight to `reconcileOperation` without ordinary hook
//!   admission, and the "no ordinary hook admission" assertion is kept.
//! - The four upstream cases that need `lane.requestOperationAbort`
//!   ("durably drains abortable input once…", "marks cancellation without
//!   installing a Drive", "cancels an admitted retry timer…", "drops a
//!   stale structural hook result…") are not ported here: the lane abort
//!   surface is "Not ported in this slice" (lane.rs module docs) and is
//!   another slice's scope.

use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use super::{reconcile_operation, run_summary_reason, DeferredCancelFn, DeferredCancelRequest};
use crate::agent_core::harness::hooks::{HookHandler, HookName, HookRegistry, HookResult};
use crate::agent_core::harness::runtime::drive_pass::{Drive, DriveOptions, ProcedureResult};
use crate::agent_core::harness::runtime::events::HarnessEvent;
use crate::agent_core::harness::runtime::lane::{EmitBatch, Lane, RuntimeConfig};
use crate::agent_core::harness::runtime::restore::restore_lane;
use crate::agent_core::harness::session::commit::insert_entry;
use crate::agent_core::harness::session::testing::InstrumentedStorage;
use crate::agent_core::harness::session::types::{NewEntry, TerminalStatus};
use crate::agent_core::harness::session::values::{
    append_list, operation_meta, operation_preparation_prefix, operation_result, operation_state,
    operation_tool_args_prefix, operation_tool_memo_prefix, pending_assistant_frames, set_value,
};
use crate::agent_core::harness::session::{
    self as session, MemoryStorage, MemoryStorageOptions, Session as _, SessionMetadata,
    SessionReader as _, Storage, StorageBackedSession, Write,
};
use crate::agent_core::harness::{background_context, DEFAULT_COMPACTION_SETTINGS};
use crate::agent_core::types::{QueueMode, ToolExecutionMode};
use crate::ai::models::faux::{
    faux_assistant_message, faux_provider, faux_tool_call, FauxContent, FauxMessageOptions,
    FauxProviderHandle, FauxProviderOptions, FauxToolCallOptions,
};
use crate::ai::models::{create_models, CreateModelsOptions};
use crate::ai::types::primitives::StopReason;
use serde_json::{json, Value};

const OPERATION_ID: &str = "01950000-0000-7000-8000-000000000001";

struct Fixture {
    lane: Arc<Lane>,
    session: Arc<StorageBackedSession>,
    #[allow(dead_code)] // retained for parity with the upstream fixture
    storage: Arc<InstrumentedStorage>,
    faux: Arc<FauxProviderHandle>,
    events: Arc<Mutex<Vec<HarnessEvent>>>,
    hook_calls: Arc<Mutex<Vec<&'static str>>>,
}

fn cancelled_control() -> Value {
    json!({"status": "cancel_requested", "requestedAt": 10})
}

fn scope() -> Value {
    json!({
        "control": cancelled_control(),
        "settings": {
            "compaction": DEFAULT_COMPACTION_SETTINGS,
            "steeringMode": "all",
            "followUpMode": "all",
            "toolExecution": "parallel",
        },
        "latestAssistantEntryId": null,
    })
}

fn state(phase: Value) -> Value {
    let mut value = scope();
    value
        .as_object_mut()
        .expect("scope object")
        .extend(phase.as_object().expect("phase object").clone());
    value
}

fn configuration() -> Value {
    json!({
        "model": {"provider": "faux", "modelId": "faux-1"},
        "thinkingLevel": "off", "activeToolNames": [],
    })
}

fn generation_context() -> Value {
    json!({
        "stepId": "step",
        "triggerEntryId": "tip",
        "configuration": configuration(),
        "streamOptions": {},
        "retryPolicy": {"maxAttempts": 2, "baseDelayMs": 10, "maxAgentDelayMs": 30_000},
        "overflowRecoveryUsed": false,
    })
}

fn summary_context() -> Value {
    json!({
        "resultEntryId": "summary-entry",
        "configuration": configuration(),
        "streamOptions": {},
        "retryPolicy": {"maxAttempts": 2, "baseDelayMs": 10, "maxAgentDelayMs": 30_000},
    })
}

fn user_entry(id: &str, content: &str) -> NewEntry {
    NewEntry::Message {
        id: id.to_string(),
        parent_id: None,
        message: serde_json::from_value(
            json!({"role": "user", "content": content, "timestamp": 1}),
        )
        .expect("user message parses"),
        terminate: None,
    }
}

/// Upstream `deferredOperation`'s source entry: a deferred assistant message
/// carrying the faux handle.
fn deferred_source_entry() -> NewEntry {
    NewEntry::Message {
        id: "deferred-source".to_string(),
        parent_id: None,
        message: serde_json::from_value(json!({
            "role": "assistant", "content": [], "api": "faux",
            "provider": "faux", "model": "faux-1", "stopReason": "deferred",
            "deferred": {"provider": "faux", "modelId": "faux-1", "api": "faux", "id": "deferred-job"},
            "usage": {"input": 1, "output": 0, "cacheRead": 0, "cacheWrite": 0,
                "totalTokens": 1, "cost": {"input": 0, "output": 0, "cacheRead": 0,
                "cacheWrite": 0, "total": 0}},
            "timestamp": 7
        }))
        .expect("deferred assistant message parses"),
        terminate: None,
    }
}

fn tool_assistant_entry() -> NewEntry {
    NewEntry::Message {
        id: "assistant".to_string(),
        parent_id: None,
        message: crate::agent_core::types::AgentMessage::Assistant(faux_assistant_message(
            FauxContent::Block(faux_tool_call(
                "tool",
                json!({}),
                FauxToolCallOptions::default(),
            )),
            FauxMessageOptions {
                stop_reason: Some(StopReason::ToolUse),
                ..FauxMessageOptions::default()
            },
        )),
        terminate: None,
    }
}

/// One installed cancelled operation (upstream `InstalledOperation`).
struct InstalledOperation {
    state: Value,
    intent: Value,
    entries: Vec<NewEntry>,
    writes: Vec<Write>,
    terminal_events: Vec<&'static str>,
}

/// The thirteen durable leaves (upstream `cases()`).
fn cases() -> Vec<InstalledOperation> {
    let run_task = json!({
        "taskId": "run-summary",
        "reason": "threshold",
        "boundary": {
            "kind": "resume_checkpoint",
            "resumeAfter": {
                "continuation": {"kind": "need_assistant", "overflowRecoveryUsed": false},
                "triggerEntryId": "tip",
            },
        },
    });
    let compaction_task =
        json!({"taskId": "compaction", "reason": "manual", "boundary": {"kind": "finish"}});
    let navigation_task = json!({
        "taskId": "navigation",
        "boundary": {"kind": "commit_navigation", "targetId": "target"},
    });
    let run_intent = json!({"kind": "run", "promptEntryIds": ["tip"]});
    let empty_run_intent = json!({"kind": "run", "promptEntryIds": []});
    let suspended = state(json!({
        "at": "deferred.suspended", "stepId": "step", "sourceEntryId": "deferred-source",
        "poll": 0, "configuration": configuration(), "streamOptions": {},
    }));
    let deferred_effect = state(json!({
        "at": "deferred.effect_pending", "stepId": "step", "sourceEntryId": "deferred-source",
        "poll": 1, "configuration": configuration(), "streamOptions": {},
        "responseEntryId": "deferred-response", "usageId": "deferred-usage",
    }));
    vec![
        InstalledOperation {
            state: state(json!({"at": "starting"})),
            intent: run_intent.clone(),
            entries: vec![user_entry("tip", "history")],
            writes: vec![],
            terminal_events: vec!["run_end"],
        },
        InstalledOperation {
            state: state(json!({
                "at": "checkpoint",
                "continuation": {"kind": "need_assistant", "overflowRecoveryUsed": false},
                "triggerEntryId": "tip",
            })),
            intent: run_intent.clone(),
            entries: vec![user_entry("tip", "history")],
            writes: vec![],
            terminal_events: vec!["run_end"],
        },
        InstalledOperation {
            state: state(json!({
                "at": "assistant.ready", "generationContext": generation_context(),
                "nextAttempt": 1,
            })),
            intent: run_intent.clone(),
            entries: vec![user_entry("tip", "history")],
            writes: vec![],
            terminal_events: vec!["run_end"],
        },
        InstalledOperation {
            state: state(json!({
                "at": "assistant.effect_pending", "generationContext": generation_context(),
                "attempt": 1, "responseEntryId": "assistant-response", "usageId": "assistant-usage",
                "intendedOutputLimit": 100, "contextWindow": 1_000,
            })),
            intent: run_intent.clone(),
            entries: vec![user_entry("tip", "history")],
            writes: vec![append_list(
                &pending_assistant_frames(OPERATION_ID, "assistant-response"),
                json!({"type": "text_delta", "contentIndex": 0, "delta": "partial"}),
            )],
            terminal_events: vec!["run_end"],
        },
        InstalledOperation {
            state: state(json!({
                "at": "assistant.retry_wait", "generationContext": generation_context(),
                "nextAttempt": 2, "notBefore": crate::ai::now_ms() + 100_000,
                "errorMessage": "retry",
            })),
            intent: run_intent.clone(),
            entries: vec![user_entry("tip", "history")],
            writes: vec![],
            terminal_events: vec!["run_end"],
        },
        InstalledOperation {
            state: state(json!({
                "at": "tools",
                "batch": {
                    "assistantEntryId": "assistant",
                    "configuration": configuration(),
                    "turnId": "turn",
                    "calls": [{"status": "planned", "sourceIndex": 0, "resultEntryId": "tool-result"}],
                },
            })),
            intent: empty_run_intent.clone(),
            entries: vec![tool_assistant_entry()],
            writes: vec![],
            terminal_events: vec!["run_end"],
        },
        InstalledOperation {
            state: suspended,
            intent: empty_run_intent.clone(),
            entries: vec![deferred_source_entry()],
            writes: vec![],
            terminal_events: vec!["run_end"],
        },
        InstalledOperation {
            state: deferred_effect,
            intent: empty_run_intent,
            entries: vec![deferred_source_entry()],
            writes: vec![],
            terminal_events: vec!["run_end"],
        },
        InstalledOperation {
            state: state(json!({"at": "summary.deciding", "task": run_task.clone()})),
            intent: run_intent.clone(),
            entries: vec![user_entry("tip", "history")],
            writes: vec![],
            terminal_events: vec!["compaction_end", "run_end"],
        },
        InstalledOperation {
            state: state(json!({
                "at": "summary.ready", "task": compaction_task,
                "summaryContext": summary_context(), "nextAttempt": 1,
            })),
            intent: json!({"kind": "compaction"}),
            entries: vec![user_entry("tip", "history")],
            writes: vec![],
            terminal_events: vec!["compaction_end"],
        },
        InstalledOperation {
            state: state(json!({
                "at": "summary.effect_pending", "task": navigation_task,
                "summaryContext": summary_context(), "attempt": 1,
                "request": {"index": 0, "usageId": "usage"}, "usageIds": [],
            })),
            intent: json!({"kind": "navigation", "targetId": "target", "summarize": true}),
            entries: vec![user_entry("tip", "history")],
            writes: vec![],
            terminal_events: vec!["navigation_end"],
        },
        InstalledOperation {
            state: state(json!({
                "at": "summary.retry_wait", "task": run_task,
                "summaryContext": summary_context(), "nextAttempt": 2,
                "notBefore": crate::ai::now_ms() + 100_000, "errorMessage": "retry",
            })),
            intent: run_intent,
            entries: vec![user_entry("tip", "history")],
            writes: vec![],
            terminal_events: vec!["compaction_end", "run_end"],
        },
        InstalledOperation {
            state: state(json!({"at": "navigation.ready_to_commit", "targetId": "target"})),
            intent: json!({"kind": "navigation", "targetId": "target", "summarize": false}),
            entries: vec![user_entry("tip", "history")],
            writes: vec![],
            terminal_events: vec!["navigation_end"],
        },
    ]
}

async fn commit(session: &StorageBackedSession, writes: Vec<Write>) {
    session
        .mutate(
            move |reader, context| {
                Box::pin(async move { reader.commit(writes, context).await.map(|_| ()) })
            },
            background_context(),
        )
        .await
        .expect("fixture commit");
}

async fn create_fixture(installed: &InstalledOperation) -> Fixture {
    let storage = InstrumentedStorage::new(Arc::new(MemoryStorage::new(
        MemoryStorageOptions::default(),
    )));
    let sess = Arc::new(StorageBackedSession::new(
        SessionMetadata {
            id: "reconcile-test".into(),
            created_at: 1,
            storage_version: 1,
            ..Default::default()
        },
        Arc::clone(&storage) as Arc<dyn Storage>,
    ));
    let entries = if installed.entries.is_empty() {
        vec![user_entry("tip", "history")]
    } else {
        installed.entries.clone()
    };
    let tip_id = entries
        .last()
        .map(|entry| entry.id().to_owned())
        .unwrap_or_default();
    let mut writes: Vec<Write> = entries
        .iter()
        .map(|entry| insert_entry(entry.clone()))
        .collect();
    writes.push(set_value(
        &session::branch_tip("main"),
        Value::String(tip_id.clone()),
    ));
    writes.push(set_value(
        &session::lane_config("main"),
        session::lane_configuration_value(
            &serde_json::from_value(configuration()).expect("lane configuration"),
        ),
    ));
    writes.extend(installed.writes.iter().cloned());
    writes.push(set_value(
        &operation_meta(OPERATION_ID),
        json!({
            "operationId": OPERATION_ID, "lane": "main", "sourceTipId": tip_id,
            "startedAt": 1, "intent": installed.intent,
        }),
    ));
    writes.push(set_value(
        &operation_state(OPERATION_ID),
        installed.state.clone(),
    ));
    writes.push(set_value(
        &session::lane_state("main"),
        json!({"currentOperationId": OPERATION_ID, "lastOperationId": null, "inbox": []}),
    ));
    commit(sess.as_ref(), writes).await;

    let faux = Arc::new(faux_provider(FauxProviderOptions {
        api: Some("faux".into()),
        ..FauxProviderOptions::default()
    }));
    let mut models = create_models(CreateModelsOptions::default());
    models.set_provider(Arc::clone(&faux.provider));
    let state = restore_lane(sess.as_ref(), "main", background_context())
        .await
        .expect("lane restores");
    let events: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&events);
    let emit: EmitBatch = Arc::new(move |batch, _context| {
        let recorded = Arc::clone(&recorded);
        Box::pin(async move {
            recorded
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend(batch);
            Ok(())
        })
    });
    let hook_calls: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let hooks = HookRegistry::new(Arc::new(
        |_error: anyhow::Error, _hook: HookName, _message: String, _context| Box::pin(async {}),
    ));
    let lane = Lane::new(
        "main",
        Arc::clone(&sess),
        models,
        hooks,
        state,
        Arc::new(|error: anyhow::Error| error),
        emit,
        Arc::new(move || RuntimeConfig {
            compaction: DEFAULT_COMPACTION_SETTINGS,
            retry_policy: crate::agent_core::harness::config::DEFAULT_RETRY_POLICY,
            system_prompt: None,
            tools: Vec::new(),
            native_tools: Default::default(),
            to_provider_messages: None,
            stream_options: Default::default(),
            resources: Default::default(),
            steering_mode: QueueMode::All,
            follow_up_mode: QueueMode::All,
            tool_execution: ToolExecutionMode::Parallel,
            entry_projectors: None,
        }),
    );
    Fixture {
        lane,
        session: sess,
        storage,
        faux,
        events,
        hook_calls,
    }
}

/// A no-op tool-event emitter (these tests do not observe tool-event
/// delivery; the tools suite owns that seam).
fn tool_emit() -> crate::agent_core::harness::runtime::drive::tools::ToolEventEmit {
    Arc::new(|_events, _context| Box::pin(async { Ok(()) }))
}

/// The faux-backed remote-cancel capability: upstream's faux provider
/// records the cancelled handle (`faux.state.cancelledDeferred`).
fn faux_cancel_fn(faux: &Arc<FauxProviderHandle>) -> Arc<DeferredCancelFn> {
    let faux = Arc::clone(faux);
    Arc::new(move |request: DeferredCancelRequest| {
        let faux = Arc::clone(&faux);
        let handle = request.handle;
        Box::pin(async move {
            faux.cancel_deferred(&handle).await;
            Ok(())
        })
    })
}

/// A remote-cancel capability that always fails (upstream's throwing
/// `cancelDeferred` override).
fn failing_cancel_fn() -> Arc<DeferredCancelFn> {
    Arc::new(|_request: DeferredCancelRequest| {
        Box::pin(async { Err(anyhow::anyhow!("remote cancellation failed")) })
    })
}

fn no_cancel_fn() -> Arc<DeferredCancelFn> {
    Arc::new(|_request: DeferredCancelRequest| Box::pin(async { Ok(()) }))
}

/// The port's stand-in for `driveOperation` over a cancelled operation: loop
/// [`reconcile_operation`] until it settles — one durable leaf per pass
/// (`assistant.effect_pending` recovers into `checkpoint` and needs a second
/// pass, exactly as upstream's driver loop re-enters).
async fn reconcile_until_settled(
    fixture: &Fixture,
    drive: &Arc<Drive>,
    cancel: &Arc<DeferredCancelFn>,
) -> ProcedureResult {
    let emit = tool_emit();
    for _ in 0..16 {
        match reconcile_operation(&fixture.lane, drive, &emit, cancel.as_ref())
            .await
            .expect("reconcile pass")
        {
            ProcedureResult::Continue => continue,
            settled => return settled,
        }
    }
    panic!("reconciliation did not settle within its bound");
}

fn event_type(event: &HarnessEvent) -> &'static str {
    match event {
        HarnessEvent::RunEnd { .. } => "run_end",
        HarnessEvent::CompactionEnd { .. } => "compaction_end",
        HarnessEvent::NavigationEnd { .. } => "navigation_end",
        HarnessEvent::TurnStart { .. } => "turn_start",
        HarnessEvent::TurnEnd { .. } => "turn_end",
        HarnessEvent::RunResume { .. } => "run_resume",
        HarnessEvent::RunSuspend { .. } => "run_suspend",
        _ => "other",
    }
}

/// The terminal-event suffix assertion (upstream filters the recorded
/// events by the expected types and compares the trailing slice).
fn assert_terminal_events(fixture: &Fixture, expected: &[&'static str], case: usize) {
    let events = fixture
        .events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let matching: Vec<&'static str> = events
        .iter()
        .map(event_type)
        .filter(|kind| expected.contains(kind))
        .collect();
    let tail = &matching[matching.len() - expected.len()..];
    assert_eq!(tail, expected, "case {case} terminal events");
}

fn deferred_case(state_at: &str) -> bool {
    matches!(state_at, "deferred.suspended" | "deferred.effect_pending")
}

#[tokio::test]
async fn reconciles_every_durable_leaf_without_ordinary_hook_admission() {
    for index in 0..13 {
        let installed = &cases()[index];
        let fixture = create_fixture(installed).await;
        // before_drive must never be consulted by reconciliation.
        let hook_calls = Arc::clone(&fixture.hook_calls);
        let handler: HookHandler = Arc::new(move |_invocation, _context| {
            let hook_calls = Arc::clone(&hook_calls);
            Box::pin(async move {
                hook_calls
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push("before_drive");
                Ok(HookResult::BeforeDrive)
            })
        });
        fixture
            .lane
            .hooks()
            .on(HookName::BeforeDrive, handler, None)
            .expect("hook registers");

        let drive = Arc::new(Drive::new(
            &DriveOptions {
                operation_id: OPERATION_ID.to_string(),
                wait_for_retry: None,
                poll_deferred: None,
            },
            background_context(),
        ));
        let cancel = if deferred_case(installed.state["at"].as_str().unwrap_or("")) {
            faux_cancel_fn(&fixture.faux)
        } else {
            no_cancel_fn()
        };

        let ProcedureResult::Settled { outcome } =
            reconcile_until_settled(&fixture, &drive, &cancel).await
        else {
            panic!("case {index} did not settle");
        };
        assert_eq!(
            outcome.operation_id, OPERATION_ID,
            "case {index} outcome id"
        );
        assert_eq!(
            outcome.status,
            TerminalStatus::Aborted,
            "case {index} status"
        );
        assert!(
            fixture
                .hook_calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty(),
            "case {index} admitted ordinary hooks"
        );
        assert!(
            fixture.lane.state().operation.is_none(),
            "case {index} left an operation installed"
        );
        let result = fixture
            .session
            .get_value(&operation_result(OPERATION_ID), background_context())
            .await
            .expect("result read")
            .expect("result settled");
        assert_eq!(result.value["status"], "aborted", "case {index} result");
        for address in [operation_meta(OPERATION_ID), operation_state(OPERATION_ID)] {
            assert!(
                fixture
                    .session
                    .get_value(&address, background_context())
                    .await
                    .expect("value read")
                    .is_none(),
                "case {index} left {address:?} behind"
            );
        }
        for prefix in [
            operation_tool_args_prefix(OPERATION_ID, None),
            operation_tool_memo_prefix(OPERATION_ID, None),
            operation_preparation_prefix(OPERATION_ID),
        ] {
            assert!(
                fixture
                    .session
                    .scan_values(&prefix, background_context())
                    .await
                    .expect("scan")
                    .is_empty(),
                "case {index} left {prefix:?} populated"
            );
        }
        if deferred_case(installed.state["at"].as_str().unwrap_or("")) {
            assert_eq!(
                fixture
                    .faux
                    .state()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .cancelled_deferred
                    .len(),
                1,
                "case {index} did not cancel the deferred handle"
            );
        }
        assert_terminal_events(&fixture, installed.terminal_events.as_slice(), index);
    }
}

#[tokio::test]
async fn ignores_deferred_provider_cancellation_failure() {
    // Upstream wraps the provider's cancelDeferred in a throwing override;
    // the port injects the equivalent failing capability.
    let installed = &cases()[6]; // deferred.suspended
    let fixture = create_fixture(installed).await;
    let drive = Arc::new(Drive::new(
        &DriveOptions {
            operation_id: OPERATION_ID.to_string(),
            wait_for_retry: None,
            poll_deferred: None,
        },
        background_context(),
    ));
    let cancel = failing_cancel_fn();
    let ProcedureResult::Settled { outcome } =
        reconcile_until_settled(&fixture, &drive, &cancel).await
    else {
        panic!("reconciliation did not settle");
    };
    assert_eq!(outcome.status, TerminalStatus::Aborted);
    assert!(fixture.lane.state().operation.is_none());
}

#[tokio::test]
async fn can_crash_between_cancelled_response_settlement_and_terminal_cleanup() {
    // Upstream `cases()[3]`: assistant.effect_pending with staged frames.
    let installed = &cases()[3];
    let fixture = create_fixture(installed).await;
    let drive = Arc::new(Drive::new(
        &DriveOptions {
            operation_id: OPERATION_ID.to_string(),
            wait_for_retry: None,
            poll_deferred: None,
        },
        background_context(),
    ));
    let cancel = no_cancel_fn();

    let emit = tool_emit();
    let first = reconcile_operation(&fixture.lane, &drive, &emit, cancel.as_ref())
        .await
        .expect("first reconcile pass");
    assert!(matches!(first, ProcedureResult::Continue));
    let operation = fixture
        .lane
        .state()
        .operation
        .expect("operation still installed");
    assert_eq!(operation.state.at(), "checkpoint");
    let entry = fixture
        .session
        .get_entry("assistant-response", background_context())
        .await
        .expect("entry read")
        .expect("assistant response entry");
    let crate::agent_core::harness::session::types::Entry::Message {
        message: crate::agent_core::types::AgentMessage::Assistant(message),
        ..
    } = entry
    else {
        panic!("assistant-response is not an assistant message");
    };
    assert_eq!(message.stop_reason, StopReason::Aborted);
    assert!(
        fixture
            .session
            .get_value(&operation_result(OPERATION_ID), background_context())
            .await
            .expect("result read")
            .is_none(),
        "no terminal result yet"
    );
    assert!(
        fixture
            .session
            .read_list(
                &pending_assistant_frames(OPERATION_ID, "assistant-response"),
                None,
                background_context(),
            )
            .await
            .expect("frames read")
            .is_empty(),
        "frames drained"
    );

    // The resumed drive settles the remaining checkpoint leaf.
    let resumed = Arc::new(Drive::new(
        &DriveOptions {
            operation_id: OPERATION_ID.to_string(),
            wait_for_retry: None,
            poll_deferred: None,
        },
        background_context(),
    ));
    let ProcedureResult::Settled { outcome } =
        reconcile_until_settled(&fixture, &resumed, &cancel).await
    else {
        panic!("resumed reconciliation did not settle");
    };
    assert_eq!(outcome.status, TerminalStatus::Aborted);
}

#[tokio::test]
async fn keeps_the_deferred_cleanup_signal_separate_from_operation_abort() {
    let drive = Drive::new(
        &DriveOptions {
            operation_id: OPERATION_ID.to_string(),
            wait_for_retry: None,
            poll_deferred: None,
        },
        background_context(),
    );
    // `drive.beginAbort(Promise.resolve()); drive.signalAbort();`
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    drive.begin_abort(cancellation);
    drive.signal_abort();
    assert!(drive.gate().signal().is_cancelled());
    assert!(!drive.close_signal().is_cancelled());
    let closed: crate::agent_core::harness::runtime::drive_pass::DriveError =
        Arc::new(std::io::Error::other("closed"));
    drive.close_gate(Arc::clone(&closed));
    assert!(drive.close_signal().is_cancelled());
}

#[tokio::test]
async fn reconcile_rejects_missing_and_uncancelled_operations() {
    // A drive id with no matching operation (the fixture installs a
    // cancelled operation under OPERATION_ID).
    let installed = &cases()[0]; // starting
    let fixture = create_fixture(installed).await;
    let cancel = no_cancel_fn();
    let emit = tool_emit();
    let drive = Arc::new(Drive::new(
        &DriveOptions {
            operation_id: "op_missing".to_string(),
            wait_for_retry: None,
            poll_deferred: None,
        },
        background_context(),
    ));
    let error = reconcile_operation(&fixture.lane, &drive, &emit, cancel.as_ref())
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("has no matching operation to reconcile"));

    // A matching operation whose control is not cancelled.
    let running = InstalledOperation {
        state: json!({
            "control": {"status": "running"},
            "settings": {
                "compaction": DEFAULT_COMPACTION_SETTINGS,
                "steeringMode": "all",
                "followUpMode": "all",
                "toolExecution": "parallel",
            },
            "latestAssistantEntryId": null,
            "at": "starting",
        }),
        intent: json!({"kind": "run", "promptEntryIds": ["tip"]}),
        entries: vec![user_entry("tip", "history")],
        writes: vec![],
        terminal_events: vec!["run_end"],
    };
    let fixture = create_fixture(&running).await;
    let drive = Arc::new(Drive::new(
        &DriveOptions {
            operation_id: OPERATION_ID.to_string(),
            wait_for_retry: None,
            poll_deferred: None,
        },
        background_context(),
    ));
    let error = reconcile_operation(&fixture.lane, &drive, &emit, cancel.as_ref())
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains(&format!("Operation {OPERATION_ID} is not cancelled")));
}

#[test]
fn run_summary_reason_validates_the_resume_checkpoint_boundary() {
    // A run summary with a finish boundary is an invariant violation.
    let finish_task = json!({
        "taskId": "task", "reason": "manual",
        "boundary": {"kind": "finish"},
        "at": "summary.deciding",
    });
    let mut phase = finish_task.clone();
    phase["boundary"] = json!({"kind": "finish"});
    let mut state_value = scope();
    state_value["at"] = json!("summary.deciding");
    state_value["task"] = phase.clone();
    let parsed: crate::agent_core::harness::runtime::durable::OperationState =
        serde_json::from_value(state_value.clone()).expect("state parses");
    let error = run_summary_reason(&parsed).unwrap_err();
    assert!(error
        .to_string()
        .contains("Cancelled run summary has an invalid result boundary"));

    // A resume_checkpoint boundary with a reason passes.
    state_value["task"]["boundary"] = json!({
        "kind": "resume_checkpoint",
        "resumeAfter": {
            "continuation": {"kind": "need_assistant", "overflowRecoveryUsed": false},
            "triggerEntryId": "tip",
        },
    });
    let parsed: crate::agent_core::harness::runtime::durable::OperationState =
        serde_json::from_value(state_value).expect("state parses");
    assert_eq!(
        run_summary_reason(&parsed).expect("valid boundary"),
        Some(crate::agent_core::harness::runtime::durable::SummaryReason::Manual)
    );
}
